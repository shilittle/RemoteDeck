#!/usr/bin/env python3
"""RemoteDeck 2 read-only Linux telemetry collector streamed over SSH stdin."""

import argparse
import csv
import datetime
import getpass
import glob
import json
import math
import os
import socket
import subprocess
import time


def cpu_times():
    values = []
    with open("/proc/stat", "r", encoding="ascii") as handle:
        for line in handle:
            fields = line.split()
            if not fields or not fields[0].startswith("cpu"):
                break
            numbers = [int(value) for value in fields[1:]]
            idle = numbers[3] + (numbers[4] if len(numbers) > 4 else 0)
            values.append((sum(numbers), idle))
    return values


def cpu_percent(previous, current):
    result = []
    for before, after in zip(previous, current):
        total = max(1, after[0] - before[0])
        idle = max(0, after[1] - before[1])
        result.append(round(max(0.0, min(100.0, (1.0 - idle / total) * 100.0)), 1))
    return result


def meminfo():
    values = {}
    with open("/proc/meminfo", "r", encoding="ascii") as handle:
        for line in handle:
            key, value = line.split(":", 1)
            values[key] = int(value.strip().split()[0]) * 1024
    total = values.get("MemTotal", 0)
    available = values.get("MemAvailable", values.get("MemFree", 0))
    swap_total = values.get("SwapTotal", 0)
    return {
        "totalBytes": total,
        "usedBytes": max(0, total - available),
        "swapTotalBytes": swap_total,
        "swapUsedBytes": max(0, swap_total - values.get("SwapFree", 0)),
    }


def network_totals():
    received = 0
    sent = 0
    with open("/proc/net/dev", "r", encoding="ascii") as handle:
        for line in handle.readlines()[2:]:
            interface, raw = line.split(":", 1)
            if interface.strip() == "lo":
                continue
            fields = raw.split()
            received += int(fields[0])
            sent += int(fields[8])
    return received, sent


def temperatures():
    values = []
    for filename in glob.glob("/sys/class/thermal/thermal_zone*/temp"):
        try:
            with open(filename, "r", encoding="ascii") as handle:
                value = float(handle.read().strip())
            if value > 1000:
                value /= 1000.0
            if 0 < value < 150:
                values.append(value)
        except (OSError, ValueError):
            pass
    return round(max(values), 1) if values else None


def disks():
    ignored = {"proc", "sysfs", "devtmpfs", "devpts", "tmpfs", "cgroup", "cgroup2", "overlay", "squashfs", "tracefs", "securityfs", "pstore", "debugfs", "mqueue", "hugetlbfs", "fusectl", "configfs", "rpc_pipefs", "autofs", "binfmt_misc"}
    result = []
    seen = set()
    try:
        with open("/proc/self/mounts", "r", encoding="utf-8") as handle:
            mounts = [line.split() for line in handle]
    except OSError:
        mounts = []
    for fields in mounts:
        if len(fields) < 3 or fields[2] in ignored:
            continue
        mount = fields[1].replace("\\040", " ")
        try:
            info = os.statvfs(mount)
            device = os.stat(mount).st_dev
            if device in seen or info.f_blocks <= 0:
                continue
            seen.add(device)
            total = info.f_blocks * info.f_frsize
            available = info.f_bavail * info.f_frsize
            result.append({"mount": mount[:1024], "totalBytes": total, "usedBytes": max(0, total - info.f_bfree * info.f_frsize), "availableBytes": available})
        except OSError:
            pass
    return result[:128]


def process_start_ticks(pid):
    try:
        with open(f"/proc/{pid}/stat", "r", encoding="ascii") as handle:
            stat = handle.read(8192)
        fields_after_name = stat[stat.rfind(")") + 2 :].split()
        return max(0, int(fields_after_name[19]))
    except (OSError, ValueError, IndexError):
        return 0


def processes():
    command = ["ps", "-eo", "pid=,ppid=,user:256=,pcpu=,pmem=,stat=,etimes=,args=", "--sort=-pcpu"]
    try:
        output = subprocess.check_output(command, text=True, stderr=subprocess.DEVNULL, timeout=4)
    except (OSError, subprocess.SubprocessError):
        return []
    result = []
    for line in output.splitlines()[:256]:
        fields = line.strip().split(None, 7)
        if len(fields) != 8:
            continue
        try:
            pid = int(fields[0])
            result.append({"pid": pid, "startTicks": process_start_ticks(pid), "ppid": int(fields[1]), "user": fields[2][:256], "cpuPercent": max(0.0, float(fields[3])), "memoryPercent": max(0.0, float(fields[4])), "state": fields[5][:32], "elapsed": fields[6][:32], "command": fields[7][:512]})
        except ValueError:
            pass
    return result


def number(value):
    value = value.strip()
    if value in {"N/A", "[N/A]", "Not Supported"}:
        return None
    try:
        parsed = float(value)
        return parsed if math.isfinite(parsed) else None
    except ValueError:
        return None


def gpu_data():
    query = ["nvidia-smi", "--query-gpu=index,uuid,name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw", "--format=csv,noheader,nounits"]
    try:
        output = subprocess.check_output(query, text=True, stderr=subprocess.DEVNULL, timeout=4)
    except (OSError, subprocess.SubprocessError):
        return [], []
    gpus = []
    uuid_to_index = {}
    for raw_fields in csv.reader(output.splitlines()):
        fields = [field.strip() for field in raw_fields]
        if len(fields) != 8:
            continue
        try:
            index = int(fields[0])
            uuid_to_index[fields[1]] = index
            gpus.append({"index": index, "name": fields[2][:256], "utilizationPercent": number(fields[3]) or 0, "memoryUsedMiB": number(fields[4]) or 0, "memoryTotalMiB": number(fields[5]) or 0, "temperatureC": number(fields[6]), "powerW": number(fields[7])})
        except ValueError:
            pass
    applications = ["nvidia-smi", "--query-compute-apps=gpu_uuid,pid,used_memory", "--format=csv,noheader,nounits"]
    try:
        process_output = subprocess.check_output(applications, text=True, stderr=subprocess.DEVNULL, timeout=4)
    except (OSError, subprocess.SubprocessError):
        process_output = ""
    gpu_processes = []
    for raw_fields in csv.reader(process_output.splitlines()):
        fields = [field.strip() for field in raw_fields]
        try:
            if len(fields) == 3 and fields[0] in uuid_to_index:
                gpu_processes.append({"gpuIndex": uuid_to_index[fields[0]], "pid": int(fields[1]), "memoryUsedMiB": number(fields[2]) or 0})
        except ValueError:
            pass
    return gpus[:32], gpu_processes[:256]


def uptime():
    with open("/proc/uptime", "r", encoding="ascii") as handle:
        return max(0, int(float(handle.read().split()[0])))


def emit(interval):
    previous_cpu = cpu_times()
    previous_network = network_totals()
    previous_time = time.monotonic()
    time.sleep(min(0.25, interval))
    while True:
        now = time.monotonic()
        current_cpu = cpu_times()
        current_network = network_totals()
        elapsed = max(0.001, now - previous_time)
        cpu_values = cpu_percent(previous_cpu, current_cpu)
        load = os.getloadavg()
        gpus, gpu_processes = gpu_data()
        payload = {
            "schemaVersion": 1,
            "capturedAt": datetime.datetime.now(datetime.timezone.utc).isoformat().replace("+00:00", "Z"),
            "hostname": socket.gethostname(),
            "currentUser": getpass.getuser(),
            "cpu": {"totalPercent": cpu_values[0] if cpu_values else 0, "perCorePercent": cpu_values[1:], "loadAverage": [round(value, 2) for value in load], "temperatureC": temperatures()},
            "memory": meminfo(),
            "network": {"receivedBytes": current_network[0], "sentBytes": current_network[1], "receiveBytesPerSecond": max(0, (current_network[0] - previous_network[0]) / elapsed), "sendBytesPerSecond": max(0, (current_network[1] - previous_network[1]) / elapsed)},
            "disks": disks(),
            "processes": processes(),
            "gpus": gpus,
            "gpuProcesses": gpu_processes,
            "uptimeSeconds": uptime(),
        }
        print(json.dumps(payload, ensure_ascii=False, separators=(",", ":")), flush=True)
        previous_cpu = current_cpu
        previous_network = current_network
        previous_time = now
        time.sleep(interval)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--interval", type=float, default=3.0)
    arguments = parser.parse_args()
    try:
        emit(max(1.0, min(60.0, arguments.interval)))
    except (BrokenPipeError, KeyboardInterrupt):
        pass
