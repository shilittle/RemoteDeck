#!/usr/bin/env python3
"""Read-only Linux workstation telemetry for LabPulse SSH.

The script is streamed over SSH and runs from stdin. It is not installed on
the workstation and does not modify remote files.
"""

import glob
import json
import os
import socket
import subprocess
import time


INTERVAL_SECONDS = float(os.environ.get("LAB_MONITOR_INTERVAL", "3"))
FORWARD_PORT = int(os.environ.get("LAB_MONITOR_FORWARD_PORT", "17890"))


def read_cpu():
    with open("/proc/stat", "r", encoding="ascii") as handle:
        values = [int(value) for value in handle.readline().split()[1:]]
    idle = values[3] + (values[4] if len(values) > 4 else 0)
    return sum(values), idle


def read_meminfo():
    result = {}
    with open("/proc/meminfo", "r", encoding="ascii") as handle:
        for line in handle:
            key, value = line.split(":", 1)
            result[key] = int(value.strip().split()[0])
    total = result.get("MemTotal", 0)
    available = result.get("MemAvailable", result.get("MemFree", 0))
    swap_total = result.get("SwapTotal", 0)
    swap_free = result.get("SwapFree", 0)
    return {
        "total_bytes": total * 1024,
        "used_bytes": max(0, total - available) * 1024,
        "percent": round((total - available) * 100.0 / total, 1) if total else 0,
        "swap_total_bytes": swap_total * 1024,
        "swap_used_bytes": max(0, swap_total - swap_free) * 1024,
    }


def read_network():
    received = 0
    sent = 0
    with open("/proc/net/dev", "r", encoding="ascii") as handle:
        for line in handle.readlines()[2:]:
            interface, values = line.split(":", 1)
            if interface.strip() == "lo":
                continue
            fields = values.split()
            received += int(fields[0])
            sent += int(fields[8])
    return received, sent


def read_disks():
    disks = []
    seen = set()
    for path in ("/", "/data"):
        if not os.path.exists(path):
            continue
        try:
            stat = os.statvfs(path)
            device = os.stat(path).st_dev
            if device in seen:
                continue
            seen.add(device)
            total = stat.f_blocks * stat.f_frsize
            available = stat.f_bavail * stat.f_frsize
            used = max(0, total - stat.f_bfree * stat.f_frsize)
            disks.append(
                {
                    "mount": path,
                    "total_bytes": total,
                    "used_bytes": used,
                    "available_bytes": available,
                    "percent": round(used * 100.0 / total, 1) if total else 0,
                }
            )
        except OSError:
            continue
    return disks


def read_gpus():
    command = [
        "nvidia-smi",
        "--query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw",
        "--format=csv,noheader,nounits",
    ]
    try:
        output = subprocess.check_output(
            command, text=True, stderr=subprocess.DEVNULL, timeout=4
        )
    except (OSError, subprocess.SubprocessError):
        return []
    gpus = []
    for line in output.splitlines():
        fields = [field.strip() for field in line.split(",")]
        if len(fields) < 7:
            continue
        try:
            gpus.append(
                {
                    "index": int(fields[0]),
                    "name": fields[1],
                    "util_percent": float(fields[2]),
                    "memory_used_mib": float(fields[3]),
                    "memory_total_mib": float(fields[4]),
                    "temperature_c": float(fields[5]),
                    "power_w": float(fields[6]) if fields[6] not in ("N/A", "[N/A]") else 0,
                }
            )
        except ValueError:
            continue
    return gpus


def read_processes():
    command = [
        "ps",
        "-eo",
        "pid=,user=,pcpu=,pmem=,stat=,etime=,comm=",
        "--sort=-pcpu",
    ]
    try:
        output = subprocess.check_output(
            command, text=True, stderr=subprocess.DEVNULL, timeout=4
        )
    except (OSError, subprocess.SubprocessError):
        return []
    processes = []
    for line in output.splitlines()[:30]:
        fields = line.split(None, 6)
        if len(fields) != 7:
            continue
        try:
            processes.append(
                {
                    "pid": int(fields[0]),
                    "user": fields[1],
                    "cpu": float(fields[2]),
                    "memory": float(fields[3]),
                    "state": fields[4],
                    "elapsed": fields[5],
                    "command": fields[6],
                }
            )
        except ValueError:
            continue
    return processes


def read_temperature():
    values = []
    for filename in glob.glob("/sys/class/thermal/thermal_zone*/temp"):
        try:
            value = float(open(filename, "r", encoding="ascii").read().strip())
            if value > 1000:
                value /= 1000.0
            if 0 < value < 150:
                values.append(value)
        except (OSError, ValueError):
            continue
    return round(max(values), 1) if values else None


def count_btop():
    try:
        output = subprocess.check_output(
            ["pgrep", "-cx", "btop"],
            text=True,
            stderr=subprocess.DEVNULL,
            timeout=2,
        ).strip()
        return int(output or "0")
    except (OSError, ValueError, subprocess.SubprocessError):
        return 0


def is_port_listening(port):
    target = f"{port:04X}"
    for filename in ("/proc/net/tcp", "/proc/net/tcp6"):
        try:
            with open(filename, "r", encoding="ascii") as handle:
                for line in handle.readlines()[1:]:
                    fields = line.split()
                    local_address = fields[1]
                    state = fields[3]
                    if local_address.rsplit(":", 1)[-1].upper() == target and state == "0A":
                        return True
        except OSError:
            continue
    return False


def read_uptime():
    with open("/proc/uptime", "r", encoding="ascii") as handle:
        return int(float(handle.read().split()[0]))


def emit_loop():
    previous_total, previous_idle = read_cpu()
    previous_received, previous_sent = read_network()
    previous_time = time.monotonic()
    time.sleep(0.25)

    while True:
        now = time.monotonic()
        total, idle = read_cpu()
        received, sent = read_network()
        elapsed = max(0.001, now - previous_time)
        total_delta = max(1, total - previous_total)
        idle_delta = max(0, idle - previous_idle)
        cpu_percent = max(0.0, min(100.0, (1.0 - idle_delta / total_delta) * 100.0))

        load_1, load_5, load_15 = os.getloadavg()
        payload = {
            "timestamp": int(time.time()),
            "hostname": socket.gethostname(),
            "cpu": {
                "percent": round(cpu_percent, 1),
                "cores": os.cpu_count() or 1,
                "load1": round(load_1, 2),
                "load5": round(load_5, 2),
                "load15": round(load_15, 2),
                "temperature_c": read_temperature(),
            },
            "memory": read_meminfo(),
            "network": {
                "receive_bytes_per_second": max(0, int((received - previous_received) / elapsed)),
                "send_bytes_per_second": max(0, int((sent - previous_sent) / elapsed)),
                "received_bytes": received,
                "sent_bytes": sent,
            },
            "disks": read_disks(),
            "gpus": read_gpus(),
            "processes": read_processes(),
            "uptime_seconds": read_uptime(),
            "btop_count": count_btop(),
            "forward_port": FORWARD_PORT,
            "forward_listening": is_port_listening(FORWARD_PORT),
        }
        print(json.dumps(payload, ensure_ascii=False, separators=(",", ":")), flush=True)

        previous_total, previous_idle = total, idle
        previous_received, previous_sent = received, sent
        previous_time = now
        time.sleep(INTERVAL_SECONDS)


if __name__ == "__main__":
    try:
        emit_loop()
    except (BrokenPipeError, KeyboardInterrupt):
        pass
