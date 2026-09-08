import { useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  Cpu,
  Gauge,
  MemoryStick,
  Play,
  RefreshCw,
  RotateCw,
  Search,
  Square,
  Thermometer,
  Zap,
} from "lucide-react";
import { api, errorMessage, events } from "../api";
import { useAppStore } from "../store";
import {
  appendTelemetrySample,
  retainTelemetryHistory,
} from "../telemetry-history";
import type { BtopStatus, ProcessSnapshot, TelemetrySnapshot } from "../types";

type SortKey =
  | "pid"
  | "user"
  | "cpuPercent"
  | "memoryPercent"
  | "state"
  | "elapsedSeconds"
  | "command";

export function TelemetryPanel(): React.JSX.Element {
  const selected = useAppStore(
    (state) =>
      state.hosts.find((host) => host.id === state.selectedHostId) ?? null,
  );
  const statuses = useAppStore((state) => state.telemetryStatuses);
  const setStatuses = useAppStore((state) => state.setTelemetryStatuses);
  const applyTelemetry = useAppStore((state) => state.applyTelemetry);
  const [history, setHistory] = useState<TelemetrySnapshot[]>([]);
  const [btop, setBtop] = useState<BtopStatus | null>(null);
  const [rotationMinutes, setRotationMinutes] = useState(15);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [search, setSearch] = useState("");
  const [sortKey, setSortKey] = useState<SortKey>("cpuPercent");
  const [descending, setDescending] = useState(true);
  const [selectedProcess, setSelectedProcess] =
    useState<ProcessSnapshot | null>(null);
  const [termAttempts, setTermAttempts] = useState<Set<string>>(
    () => new Set(),
  );
  const selectedHostIdRef = useRef<string | null>(selected?.id ?? null);
  selectedHostIdRef.current = selected?.id ?? null;
  const visibleHistory = useMemo(
    () => selected ? history.filter((sample) => sample.hostId === selected.id) : [],
    [history, selected?.id],
  );
  const latest = visibleHistory.at(-1) ?? null;
  const status = selected ? statuses[selected.id] : undefined;

  useEffect(() => {
    setHistory([]);
    setBtop(null);
    setSelectedProcess(null);
    setTermAttempts(new Set());
    if (!selected) return;
    const hostId = selected.id;
    const lifecycle = { disposed: false };
    let cleanup: (() => void) | undefined;
    void (async () => {
      try {
        const next = events.telemetry((event) => {
          if (lifecycle.disposed) return;
          applyTelemetry(event);
          const sample = event.sample;
          if (sample?.hostId === hostId)
            setHistory((current) => appendTelemetrySample(current, sample));
        });
        if (lifecycle.disposed) {
          next();
          return;
        }
        cleanup = next;
      } catch (reason) {
        if (!lifecycle.disposed) setError(`监控事件订阅失败：${errorMessage(reason)}`);
      }

      try {
        const [nextStatuses, samples, nextBtop] = await Promise.all([
          api.listTelemetry(),
          api.telemetryHistory(hostId),
          api.probeBtop(hostId),
        ]);
        if (lifecycle.disposed || selectedHostIdRef.current !== hostId) return;
        setStatuses(nextStatuses);
        setHistory((current) => retainTelemetryHistory([...samples, ...current]));
        setBtop(nextBtop);
        setRotationMinutes(nextBtop.rotationMinutes);
      } catch (reason) {
        if (!lifecycle.disposed) setError(errorMessage(reason));
      }
    })();
    return () => {
      lifecycle.disposed = true;
      cleanup?.();
    };
  }, [applyTelemetry, selected?.id, setStatuses]);

  const processes = useMemo(() => {
    const query = search.trim().toLowerCase();
    return (latest?.processes ?? [])
      .filter(
        (process) =>
          !query ||
          `${String(process.pid)} ${process.user} ${process.command} ${process.state}`
            .toLowerCase()
            .includes(query),
      )
      .toSorted((left, right) => {
        const a = left[sortKey];
        const b = right[sortKey];
        const result =
          typeof a === "number" && typeof b === "number"
            ? a - b
            : String(a).localeCompare(String(b));
        return descending ? -result : result;
      });
  }, [descending, latest?.processes, search, sortKey]);

  const run = async (operation: () => Promise<void>): Promise<void> => {
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await operation();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const signal = async (kind: "TERM" | "KILL"): Promise<void> => {
    if (!selected || !selectedProcess) return;
    const key = processKey(selectedProcess);
    await run(async () => {
      await api.signalProcess(
        selected.id,
        selectedProcess,
        kind,
      );
      if (kind === "TERM")
        setTermAttempts((current) => new Set(current).add(key));
      setMessage(
        `${kind === "TERM" ? "SIGTERM" : "SIGKILL"} 已发送到 PID ${String(selectedProcess.pid)}。`,
      );
    });
  };

  if (!selected)
    return (
      <section className="empty-state">
        <Gauge size={42} />
        <h1>选择一台主机</h1>
        <p>结构化监控只通过已验证的 SSH 连接运行。</p>
      </section>
    );

  return (
    <section className="telemetry-workspace">
      <header className="telemetry-toolbar">
        <div>
          <h1>系统监控</h1>
          <p>
            {selected.alias} · {statusLabel(status?.state ?? "stopped")}
            {selected.monitorEnabled ? " · 启动时自动" : " · 仅手动"}
            {status?.lastSampleAt
              ? ` · ${new Date(status.lastSampleAt).toLocaleTimeString()}`
              : ""}
          </p>
        </div>
        <div className="button-row wrap">
          <button
            className="primary"
            disabled={
              busy || status?.state === "online" || status?.state === "starting"
            }
            onClick={() => {
              void run(async () => {
                applyTelemetry({
                  status: await api.startTelemetry(selected.id),
                  sample: null,
                });
                setMessage("监控采集器已启动。");
              });
            }}
          >
            <Play size={14} />
            启动
          </button>
          <button
            disabled={busy || status?.state === "stopped"}
            onClick={() => {
              void run(async () => {
                applyTelemetry({
                  status: await api.stopTelemetry(selected.id),
                  sample: null,
                });
                setMessage("监控已停止。");
              });
            }}
          >
            <Square size={14} />
            停止
          </button>
          <button
            disabled={busy}
            onClick={() => {
              void run(async () => {
                const hostId = selected.id;
                const [samples, nextBtop] = await Promise.all([
                  api.telemetryHistory(hostId),
                  api.probeBtop(hostId),
                ]);
                if (selectedHostIdRef.current !== hostId) return;
                setHistory((current) => retainTelemetryHistory([...samples, ...current]));
                setBtop(nextBtop);
              });
            }}
          >
            <RefreshCw size={14} />
            刷新
          </button>
        </div>
      </header>
      {status?.error && <div className="notice error">{status.error}</div>}
      {error && (
        <div className="notice error" role="alert">
          {error}
          <button onClick={() => setError("")}>关闭</button>
        </div>
      )}
      {message && (
        <div className="notice success" role="status">
          {message}
        </div>
      )}

      <div className="telemetry-summary">
        <Metric
          icon={<Cpu size={18} />}
          label="CPU"
          value={latest ? `${latest.cpu.percent.toFixed(1)}%` : "—"}
          detail={
            latest
              ? `Load ${latest.cpu.load1.toFixed(2)} / ${latest.cpu.load5.toFixed(2)}`
              : "等待采样"
          }
        />
        <Metric
          icon={<MemoryStick size={18} />}
          label="内存"
          value={
            latest
              ? `${percentage(latest.memory.usedBytes, latest.memory.totalBytes).toFixed(1)}%`
              : "—"
          }
          detail={
            latest
              ? `${formatBytes(latest.memory.usedBytes)} / ${formatBytes(latest.memory.totalBytes)}`
              : "等待采样"
          }
        />
        <Metric
          icon={<Activity size={18} />}
          label="网络"
          value={
            latest
              ? `↓ ${formatRate(latest.network.receiveBytesPerSecond)}`
              : "—"
          }
          detail={
            latest
              ? `↑ ${formatRate(latest.network.sendBytesPerSecond)}`
              : "等待采样"
          }
        />
        <Metric
          icon={<Gauge size={18} />}
          label="进程"
          value={latest ? String(latest.processes.length) : "—"}
          detail={latest ? `当前用户 ${latest.currentUser}` : "等待采样"}
        />
      </div>

      <div className="telemetry-charts">
        <HistoryChart
          title="CPU %"
          values={visibleHistory.map((sample) => sample.cpu.percent)}
          color="#58a6ff"
        />
        <HistoryChart
          title="内存 %"
          values={visibleHistory.map((sample) =>
            percentage(sample.memory.usedBytes, sample.memory.totalBytes),
          )}
          color="#3fb950"
        />
        <HistoryChart
          title="网络接收 MiB/s"
          values={visibleHistory.map(
            (sample) => sample.network.receiveBytesPerSecond / 1024 / 1024,
          )}
          color="#d29922"
        />
      </div>

      <div className="resource-grid">
        <section className="card resource-card">
          <h2>GPU</h2>
          {latest?.gpus.length ? (
            latest.gpus.map((gpu) => (
              <article className="gpu-card" key={gpu.index}>
                <strong>
                  GPU {String(gpu.index)} · {gpu.name}
                </strong>
                <span>利用率 {gpu.utilizationPercent.toFixed(1)}%</span>
                <span>
                  显存 {gpu.memoryUsedMiB.toFixed(0)} /{" "}
                  {gpu.memoryTotalMiB.toFixed(0)} MiB
                </span>
                <span>
                  <Thermometer size={12} />
                  {gpu.temperatureC === null
                    ? "无温度"
                    : `${gpu.temperatureC.toFixed(0)} °C`}{" "}
                  ·{" "}
                  {gpu.powerW === null
                    ? "无功耗"
                    : `${gpu.powerW.toFixed(1)} W`}
                </span>
              </article>
            ))
          ) : (
            <p className="muted">未检测到 GPU；CPU/内存监控不受影响。</p>
          )}
        </section>
        <section className="card resource-card">
          <h2>磁盘</h2>
          {latest?.disks.length ? (
            latest.disks.map((disk) => {
              const used = percentage(disk.usedBytes, disk.totalBytes);
              return (
                <article className="disk-card" key={disk.mount}>
                  <div>
                    <strong>{disk.mount}</strong>
                    <span>{used.toFixed(1)}%</span>
                  </div>
                  <div className="resource-bar">
                    <i style={{ width: `${String(used)}%` }} />
                  </div>
                  <small>
                    {formatBytes(disk.usedBytes)} /{" "}
                    {formatBytes(disk.totalBytes)} · 可用{" "}
                    {formatBytes(disk.availableBytes)}
                  </small>
                </article>
              );
            })
          ) : (
            <p className="muted">等待磁盘采样。</p>
          )}
        </section>
        <section className="card resource-card btop-card">
          <h2>btop 辅助终端</h2>
          <p>
            {btop?.installed
              ? (btop.version ?? "已安装")
              : "未安装（结构化监控不受影响）"}
          </p>
          <label>
            <span>轮换分钟</span>
            <input
              type="number"
              min={1}
              max={1440}
              value={rotationMinutes}
              onChange={(event) =>
                setRotationMinutes(Number(event.target.value))
              }
            />
          </label>
          <div className="button-row wrap">
            <button
              disabled={
                busy ||
                !btop?.installed ||
                btop.watchdogState === "running" ||
                btop.watchdogState === "unavailable" ||
                btop.watchdogState === "conflict"
              }
              onClick={() => {
                void run(async () =>
                  setBtop(
                    await api.startBtopWatchdog(selected.id, rotationMinutes),
                  ),
                );
              }}
            >
              <RotateCw size={13} />
              启用 watchdog
            </button>
            <button
              disabled={busy || btop?.watchdogState !== "running"}
              onClick={() => {
                void run(async () =>
                  setBtop(await api.stopBtopWatchdog(selected.id)),
                );
              }}
            >
              <Square size={13} />
              停止
            </button>
          </div>
          {btop && (
            <small>
              状态 {btop.watchdogState} · 重启{" "}
              {btop.restartCount === null ? "—" : String(btop.restartCount)}
              {btop.lastError ? ` · ${btop.lastError}` : ""}
            </small>
          )}
        </section>
      </div>

      <section className="card process-panel">
        <div className="process-heading">
          <div>
            <h2>进程</h2>
            <p>
              {latest
                ? `${String(latest.processes.length)} 条快照`
                : "等待采样"}
            </p>
          </div>
          <label className="process-search">
            <Search size={14} />
            <input
              aria-label="搜索进程"
              placeholder="PID、用户或命令"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
            />
          </label>
          <div className="button-row">
            <button
              disabled={
                busy ||
                !selectedProcess ||
                selectedProcess.user !== latest?.currentUser
              }
              onClick={() => {
                void signal("TERM");
              }}
            >
              <Zap size={13} />
              SIGTERM
            </button>
            <button
              className="danger"
              disabled={
                busy ||
                !selectedProcess ||
                selectedProcess.user !== latest?.currentUser ||
                !termAttempts.has(processKey(selectedProcess))
              }
              onClick={() => {
                if (
                  selectedProcess &&
                  window.confirm(
                    `强制终止 PID ${String(selectedProcess.pid)}？`,
                  )
                )
                  void signal("KILL");
              }}
            >
              SIGKILL（二次）
            </button>
          </div>
        </div>
        <div className="process-table">
          <div className="process-row header">
            {(
              [
                ["pid", "PID"],
                ["user", "用户"],
                ["cpuPercent", "CPU %"],
                ["memoryPercent", "内存 %"],
                ["state", "状态"],
                ["elapsedSeconds", "秒"],
                ["command", "命令"],
              ] as Array<[SortKey, string]>
            ).map(([key, label]) => (
              <button
                key={key}
                onClick={() => {
                  if (sortKey === key) setDescending((value) => !value);
                  else {
                    setSortKey(key);
                    setDescending(true);
                  }
                }}
              >
                {label}
                {sortKey === key ? (descending ? " ↓" : " ↑") : ""}
              </button>
            ))}
          </div>
          {processes.map((process) => (
            <button
              className={
                selectedProcess &&
                processKey(selectedProcess) === processKey(process)
                  ? "process-row selected"
                  : "process-row"
              }
              key={processKey(process)}
              onClick={() => setSelectedProcess(process)}
            >
              <span>{String(process.pid)}</span>
              <span
                className={
                  process.user === latest?.currentUser ? "own-process" : ""
                }
              >
                {process.user}
              </span>
              <span>{process.cpuPercent.toFixed(1)}</span>
              <span>{process.memoryPercent.toFixed(1)}</span>
              <span>{process.state}</span>
              <span>{String(process.elapsedSeconds)}</span>
              <span title={process.command}>{process.command}</span>
            </button>
          ))}
          {processes.length === 0 && (
            <p className="process-empty">没有匹配的进程。</p>
          )}
        </div>
      </section>
    </section>
  );
}

function Metric({
  icon,
  label,
  value,
  detail,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
  detail: string;
}): React.JSX.Element {
  return (
    <article className="card metric-card">
      {icon}
      <div>
        <span>{label}</span>
        <strong>{value}</strong>
        <small>{detail}</small>
      </div>
    </article>
  );
}

function HistoryChart({
  title,
  values,
  color,
}: {
  title: string;
  values: number[];
  color: string;
}): React.JSX.Element {
  const samples = values.slice(-120);
  const max = Math.max(1, ...samples);
  const points = samples
    .map(
      (value, index) =>
        `${String(samples.length <= 1 ? 0 : (index * 100) / (samples.length - 1))},${String(40 - Math.min(40, (value * 40) / max))}`,
    )
    .join(" ");
  return (
    <article className="card history-chart">
      <h2>{title}</h2>
      <svg
        viewBox="0 0 100 40"
        preserveAspectRatio="none"
        role="img"
        aria-label={`${title} 历史曲线`}
      >
        <polyline
          points={points}
          fill="none"
          stroke={color}
          strokeWidth="1.4"
          vectorEffect="non-scaling-stroke"
        />
      </svg>
      <small>
        当前 {samples.at(-1)?.toFixed(2) ?? "—"} · 峰值{" "}
        {samples.length ? max.toFixed(2) : "—"}
      </small>
    </article>
  );
}

function processKey(process: ProcessSnapshot): string {
  return `${String(process.pid)}:${String(process.startTicks)}:${process.user}:${process.command}`;
}
function percentage(used: number, total: number): number {
  return total > 0 ? Math.min(100, (used * 100) / total) : 0;
}
function formatBytes(value: number): string {
  if (value < 1024) return `${String(value)} B`;
  if (value < 1024 ** 2) return `${(value / 1024).toFixed(1)} KiB`;
  if (value < 1024 ** 3) return `${(value / 1024 ** 2).toFixed(1)} MiB`;
  return `${(value / 1024 ** 3).toFixed(1)} GiB`;
}
function formatRate(value: number): string {
  return `${formatBytes(value)}/s`;
}
function statusLabel(
  value: "stopped" | "starting" | "online" | "degraded" | "failed",
): string {
  return {
    stopped: "已停止",
    starting: "启动中",
    online: "在线",
    degraded: "降级",
    failed: "失败",
  }[value];
}
