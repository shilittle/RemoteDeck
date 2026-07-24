import { useEffect, useMemo, useRef, useState } from 'react'
import { LineChart } from 'echarts/charts'
import { GridComponent, TooltipComponent } from 'echarts/components'
import * as echarts from 'echarts/core'
import type { EChartsCoreOption } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { Activity, Cpu, Gauge, MemoryStick, Play, RefreshCw, RotateCw, Search, Square, TerminalSquare, Thermometer, Zap } from 'lucide-react'
import type { TelemetrySnapshot } from '../../../protocol/domain'
import type { BtopStatus, TelemetryStatus } from '../../../protocol/telemetry'
import { useHostStore } from '../../host-store'
import { useAppStore } from '../../store'

echarts.use([LineChart, GridComponent, TooltipComponent, CanvasRenderer])

type ProcessItem = TelemetrySnapshot['processes'][number]
type SortKey = 'pid' | 'user' | 'cpuPercent' | 'memoryPercent' | 'state' | 'elapsed' | 'command'

export function TelemetryPanel(): React.JSX.Element {
  const hosts = useHostStore((state) => state.items)
  const selectedId = useHostStore((state) => state.selectedId)
  const selected = hosts.find((item) => item.host.id === selectedId)
  const [status, setStatus] = useState<TelemetryStatus>()
  const [history, setHistory] = useState<TelemetrySnapshot[]>([])
  const [btop, setBtop] = useState<BtopStatus>()
  const [rotationMinutes, setRotationMinutes] = useState('60')
  const [search, setSearch] = useState('')
  const [sortKey, setSortKey] = useState<SortKey>('cpuPercent')
  const [descending, setDescending] = useState(true)
  const [selectedProcess, setSelectedProcess] = useState<ProcessItem>()
  const [termAttempts, setTermAttempts] = useState<Set<string>>(new Set())
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState('')
  const [error, setError] = useState('')
  const latest = history.at(-1)

  useEffect(() => {
    setStatus(undefined)
    setHistory([])
    setBtop(undefined)
    setSelectedProcess(undefined)
    if (!selectedId) return
    void Promise.all([window.remoteDeck.telemetry.list(), window.remoteDeck.telemetry.history(selectedId)]).then(([statuses, samples]) => {
      setStatus(statuses.find((item) => item.hostId === selectedId))
      setHistory(samples)
    }).catch((reason: unknown) => setError(errorMessage(reason)))
  }, [selectedId])

  useEffect(() => window.remoteDeck.telemetry.onEvent((event) => {
    if (event.type === 'snapshot' && event.snapshot.hostId === selectedId) setHistory((current) => {
      const previous = current.at(-1)
      const compact = previous ? [...current.slice(0, -1), { ...previous, processes: [], gpuProcesses: [] }] : current
      return [...compact, event.snapshot].slice(-3600)
    })
    else if (event.type === 'status' && event.status.hostId === selectedId) setStatus(event.status)
    else if (event.type === 'btop' && event.status.hostId === selectedId) setBtop(event.status)
  }), [selectedId])

  const processes = useMemo(() => {
    const query = search.trim().toLowerCase()
    const values = (latest?.processes ?? []).filter((item) => !query || `${String(item.pid)} ${String(item.ppid)} ${item.user} ${item.state} ${item.command}`.toLowerCase().includes(query))
    return values.toSorted((left, right) => compareProcess(left, right, sortKey) * (descending ? -1 : 1))
  }, [descending, latest, search, sortKey])

  function run(action: () => Promise<void>): void {
    setBusy(true)
    setError('')
    setMessage('')
    void action().catch((reason: unknown) => setError(errorMessage(reason))).finally(() => setBusy(false))
  }

  function chooseSort(key: SortKey): void { if (sortKey === key) setDescending(!descending); else { setSortKey(key); setDescending(key === 'cpuPercent' || key === 'memoryPercent' || key === 'pid') } }

  async function sendSignal(signal: 'TERM' | 'KILL'): Promise<void> {
    if (!selectedId || !selectedProcess) return
    const key = processKey(selectedProcess)
    if (signal === 'KILL' && !window.confirm(`SIGKILL 将强制结束 PID ${String(selectedProcess.pid)}。\n\n${selectedProcess.command}\n\n确认执行二次强制操作？`)) return
    await window.remoteDeck.telemetry.signal({ hostId: selectedId, pid: selectedProcess.pid, signal, expectedUser: selectedProcess.user, expectedCommand: selectedProcess.command, confirmKill: signal === 'KILL' })
    if (signal === 'TERM') setTermAttempts((current) => new Set(current).add(key))
    else setTermAttempts((current) => { const next = new Set(current); next.delete(key); return next })
    setMessage(`${signal === 'TERM' ? 'SIGTERM' : 'SIGKILL'} 已发送；下一采样会刷新进程状态。`)
  }

  async function openBtop(): Promise<void> {
    if (!selected) return
    const probed = await window.remoteDeck.telemetry.btop.probe(selected.host.id)
    setBtop(probed)
    if (!probed.installed) throw new Error('远端未安装 btop；终端、文件、隧道和结构化遥测不受影响。')
    const terminal = await window.remoteDeck.terminals.create({ hostId: selected.host.id, cwd: selected.workspace?.remotePath ?? '~', cols: 120, rows: 40 })
    if (terminal.state !== 'online') throw new Error(terminal.error ?? 'btop 终端创建失败')
    await window.remoteDeck.terminals.write(terminal.id, 'exec btop\r')
    useAppStore.getState().setActivity('terminal')
  }

  if (!selected) return <section className="file-state"><Gauge size={42} /><h1>选择一台主机</h1><p>监控采集器只通过已验证的 SSH 连接流式运行。</p></section>
  const online = selected.state === 'online'
  const memoryPercent = latest && latest.memory.totalBytes ? latest.memory.usedBytes * 100 / latest.memory.totalBytes : 0

  return <section className="telemetry-workspace">
    <header className="telemetry-toolbar"><div><h1>系统监控</h1><p>{selected.host.alias} · collector v1 JSONL · {statusLabel(status?.state ?? 'stopped')}</p></div><div className="button-row wrap"><button className="primary" disabled={busy || !online || status?.state === 'online' || status?.state === 'starting'} onClick={() => run(async () => { setStatus(await window.remoteDeck.telemetry.start(selected.host.id)); setMessage('采集器 supervisor 已启动。') })}><Play size={14} />启动</button><button className="secondary" disabled={busy || status?.state === 'stopped'} onClick={() => run(async () => { setStatus(await window.remoteDeck.telemetry.stop(selected.host.id)); setMessage('监控已停止，已有历史仍保留在本次进程内。') })}><Square size={14} />停止</button><button className="secondary" disabled={busy || !online} onClick={() => run(async () => { setHistory(await window.remoteDeck.telemetry.history(selected.host.id)); setBtop(await window.remoteDeck.telemetry.btop.probe(selected.host.id)) })}><RefreshCw size={14} />刷新 / 探测 btop</button></div></header>
    {status?.state === 'dependency_missing' && <div className="dependency-warning"><strong>远端缺少 python3</strong><span>安装 Python 3 后重新启动监控；SSH 终端、SFTP 和隧道仍可正常使用。</span></div>}
    {status?.lastError && status.state !== 'dependency_missing' && <div className="telemetry-error">{status.lastError}{status.nextRetryAt ? ` · 将于 ${new Date(status.nextRetryAt).toLocaleTimeString()} 重试` : ''}</div>}
    {message && <div className="notice">{message}</div>}{error && <div className="error-banner" role="alert">{error}</div>}

    <div className="telemetry-summary">
      <Metric icon={Cpu} label="CPU" value={latest ? `${latest.cpu.totalPercent.toFixed(1)}%` : '—'} detail={latest ? `Load ${latest.cpu.loadAverage.map((value) => value.toFixed(2)).join(' / ')}` : '等待采样'} />
      <Metric icon={MemoryStick} label="内存" value={latest ? `${memoryPercent.toFixed(1)}%` : '—'} detail={latest ? `${formatBytes(latest.memory.usedBytes)} / ${formatBytes(latest.memory.totalBytes)}` : '等待采样'} />
      <Metric icon={Activity} label="网络" value={latest ? `↓ ${formatRate(latest.network.receiveBytesPerSecond)}` : '—'} detail={latest ? `↑ ${formatRate(latest.network.sendBytesPerSecond)}` : '等待采样'} />
      <Metric icon={Thermometer} label="温度 / 运行时间" value={latest?.cpu.temperatureC == null ? '无传感器' : `${latest.cpu.temperatureC.toFixed(1)} °C`} detail={latest ? formatDuration(latest.uptimeSeconds) : '等待采样'} />
    </div>

    <div className="telemetry-charts"><HistoryChart kind="cpu" history={history} /><HistoryChart kind="memory" history={history} /><HistoryChart kind="network" history={history} /></div>

    <div className="resource-grid"><section className="resource-card card"><h2>GPU</h2>{latest?.gpus.length ? latest.gpus.map((gpu) => <div className="gpu-card" key={gpu.index}><strong>GPU {gpu.index} · {gpu.name}</strong><span>利用率 {gpu.utilizationPercent.toFixed(1)}%</span><span>显存 {gpu.memoryUsedMiB.toFixed(0)} / {gpu.memoryTotalMiB.toFixed(0)} MiB</span><span>{gpu.temperatureC == null ? '无温度' : `${gpu.temperatureC.toFixed(0)} °C`} · {gpu.powerW == null ? '无功耗数据' : `${gpu.powerW.toFixed(1)} W`}</span>{latest.gpuProcesses.filter((item) => item.gpuIndex === gpu.index).map((item) => <small key={item.pid}>PID {item.pid} · {item.memoryUsedMiB.toFixed(0)} MiB</small>)}</div>) : <p className="muted">未检测到 NVIDIA GPU；监控已正常降级。</p>}</section><section className="resource-card card"><h2>磁盘</h2>{latest?.disks.length ? latest.disks.map((disk) => { const percent = disk.totalBytes ? disk.usedBytes * 100 / disk.totalBytes : 0; return <div className="disk-card" key={disk.mount}><div><strong>{disk.mount}</strong><span>{percent.toFixed(1)}%</span></div><div className="resource-bar"><i style={{ width: `${String(Math.min(100, percent))}%` }} /></div><small>{formatBytes(disk.usedBytes)} / {formatBytes(disk.totalBytes)} · 可用 {formatBytes(disk.availableBytes)}</small></div> }) : <p className="muted">等待磁盘采样。</p>}</section><section className="resource-card card btop-card"><h2>btop 辅助终端</h2><p>{btop ? btop.installed ? btop.version : '未安装（结构化监控不受影响）' : '尚未探测'}</p><div className="button-row wrap"><button className="secondary" disabled={busy || !online} onClick={() => run(openBtop)}><TerminalSquare size={14} />打开 btop 标签</button><label><span>轮换分钟</span><input type="number" min="1" max="1440" value={rotationMinutes} onChange={(event) => setRotationMinutes(event.target.value)} /></label><button className="secondary" disabled={busy || !online || btop?.watchdogState === 'running'} onClick={() => run(async () => setBtop(await window.remoteDeck.telemetry.btop.startWatchdog({ hostId: selected.host.id, rotationMinutes: Number(rotationMinutes) })))}><RotateCw size={14} />启用 watchdog</button><button className="secondary" disabled={busy || !btop || btop.watchdogState === 'stopped'} onClick={() => run(async () => setBtop(await window.remoteDeck.telemetry.btop.stopWatchdog(selected.host.id)))}><Square size={14} />停止 watchdog</button></div>{btop && <small>状态 {btop.watchdogState} · 重启 {btop.restartCount}{btop.lastError ? ` · ${btop.lastError}` : ''}</small>}</section></div>

    <section className="process-panel card"><div className="process-heading"><div><h2>进程</h2><p>{latest ? `当前用户 ${latest.currentUser} · ${String(latest.processes.length)} 条快照` : '等待采样'}</p></div><label className="process-search"><Search size={14} /><input aria-label="搜索进程" placeholder="PID、用户或命令" value={search} onChange={(event) => setSearch(event.target.value)} /></label><div className="button-row"><button className="secondary" disabled={busy || !selectedProcess || selectedProcess.user !== latest?.currentUser} onClick={() => run(() => sendSignal('TERM'))}><Zap size={14} />SIGTERM</button><button className="danger" disabled={busy || !selectedProcess || selectedProcess.user !== latest?.currentUser || !termAttempts.has(processKey(selectedProcess))} onClick={() => run(() => sendSignal('KILL'))}>SIGKILL（二次）</button></div></div><div className="process-table"><div className="process-row header">{([['pid', 'PID'], ['user', '用户'], ['cpuPercent', 'CPU %'], ['memoryPercent', '内存 %'], ['state', '状态'], ['elapsed', '运行秒数'], ['command', '命令']] as Array<[SortKey, string]>).map(([key, label]) => <button key={key} onClick={() => chooseSort(key)}>{label}{sortKey === key ? descending ? ' ↓' : ' ↑' : ''}</button>)}</div>{processes.map((item) => <button className={`process-row${selectedProcess && processKey(selectedProcess) === processKey(item) ? ' selected' : ''}`} key={processKey(item)} onClick={() => setSelectedProcess(item)}><span>{item.pid}</span><span className={item.user === latest?.currentUser ? 'own-process' : ''}>{item.user}</span><span>{item.cpuPercent.toFixed(1)}</span><span>{item.memoryPercent.toFixed(1)}</span><span>{item.state}</span><span>{item.elapsed}</span><span title={item.command}>{item.command}</span></button>)}{!processes.length && <p className="process-empty">没有匹配的进程。</p>}</div></section>
  </section>
}

function Metric({ icon: Icon, label, value, detail }: { icon: typeof Cpu; label: string; value: string; detail: string }): React.JSX.Element { return <article className="metric-card card"><Icon size={18} /><div><span>{label}</span><strong>{value}</strong><small>{detail}</small></div></article> }

interface HistoryLine { name: string; color: string; value: (snapshot: TelemetrySnapshot) => number }
type ChartKind = 'cpu' | 'memory' | 'network'
const historyCharts: Record<ChartKind, { title: string; lines: HistoryLine[] }> = {
  cpu: { title: 'CPU %', lines: [{ name: 'CPU', color: '#58a6ff', value: (item) => item.cpu.totalPercent }] },
  memory: { title: '内存 %', lines: [{ name: '内存', color: '#a371f7', value: (item) => item.memory.totalBytes ? item.memory.usedBytes * 100 / item.memory.totalBytes : 0 }] },
  network: { title: '网络 MiB/s', lines: [{ name: '接收', color: '#3fb950', value: (item) => item.network.receiveBytesPerSecond / 1024 / 1024 }, { name: '发送', color: '#d29922', value: (item) => item.network.sendBytesPerSecond / 1024 / 1024 }] }
}
function HistoryChart({ kind, history }: { kind: ChartKind; history: TelemetrySnapshot[] }): React.JSX.Element {
  const container = useRef<HTMLDivElement>(null)
  const definition = historyCharts[kind]
  useEffect(() => {
    if (!container.current) return
    const chart = echarts.init(container.current, undefined, { renderer: 'canvas' })
    const option: EChartsCoreOption = { animation: false, backgroundColor: 'transparent', grid: { left: 38, right: 10, top: 28, bottom: 24 }, tooltip: { trigger: 'axis' }, xAxis: { type: 'category', boundaryGap: false, data: history.map((item) => new Date(item.capturedAt).toLocaleTimeString()), axisLabel: { color: '#7d8590', fontSize: 8 }, axisLine: { lineStyle: { color: '#30363d' } } }, yAxis: { type: 'value', min: 0, axisLabel: { color: '#7d8590', fontSize: 8 }, splitLine: { lineStyle: { color: '#21262d' } } }, series: definition.lines.map((line) => ({ name: line.name, type: 'line', showSymbol: false, smooth: true, lineStyle: { width: 1.5, color: line.color }, areaStyle: { opacity: 0.08, color: line.color }, data: history.map(line.value) })) }
    chart.setOption(option)
    const resize = new ResizeObserver(() => chart.resize())
    resize.observe(container.current)
    return () => { resize.disconnect(); chart.dispose() }
  }, [definition, history])
  return <article className="history-chart card"><h2>{definition.title}</h2><div ref={container} /></article>
}

function compareProcess(left: ProcessItem, right: ProcessItem, key: SortKey): number { const a = left[key]; const b = right[key]; return typeof a === 'number' && typeof b === 'number' ? a - b : String(a).localeCompare(String(b)) }
function processKey(item: ProcessItem): string { return `${String(item.pid)}:${item.user}:${item.command}` }
function statusLabel(state: TelemetryStatus['state']): string { return { stopped: '已停止', starting: '启动中', online: '在线', recovering: '恢复中', dependency_missing: '缺少 python3', failed: '失败' }[state] }
function errorMessage(reason: unknown): string { return reason instanceof Error ? reason.message : '监控操作失败' }
function formatBytes(bytes: number): string { if (bytes < 1024) return `${String(bytes)} B`; const units = ['KiB', 'MiB', 'GiB', 'TiB']; let value = bytes / 1024; let index = 0; while (value >= 1024 && index < units.length - 1) { value /= 1024; index += 1 } return `${value.toFixed(value >= 10 ? 1 : 2)} ${units[index] ?? 'TiB'}` }
function formatRate(bytes: number): string { return `${formatBytes(bytes)}/s` }
function formatDuration(seconds: number): string { const days = Math.floor(seconds / 86400); const hours = Math.floor((seconds % 86400) / 3600); return `${String(days)} 天 ${String(hours)} 小时` }
