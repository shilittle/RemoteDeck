import { useCallback, useEffect, useRef, useState } from 'react'
import { FolderOpen, Play, RefreshCw, RotateCw, Square } from 'lucide-react'
import { api, errorMessage, events } from '../api'
import { useAppStore } from '../store'
import type { OperationSummary } from '../api'
import type { CommandJob, TelemetryRuntimeState, TerminalEvent, TerminalSnapshot, TransferJob, TunnelProfile, TunnelRuntimeState, TunnelSnapshot } from '../types'

type WorkspaceTab = 'terminal' | 'files' | 'commands' | 'monitor' | 'tunnels'

interface TaskPanelProps {
  onOpenWorkspace: (tab: WorkspaceTab) => void
}

export function TaskPanel({ onOpenWorkspace }: TaskPanelProps): React.JSX.Element {
  const hosts = useAppStore((state) => state.hosts)
  const tunnels = useAppStore((state) => state.tunnels)
  const tunnelStates = useAppStore((state) => state.tunnelStates)
  const transfers = useAppStore((state) => state.transfers)
  const telemetryStatuses = useAppStore((state) => state.telemetryStatuses)
  const commandJobs = useAppStore((state) => state.commandJobs)
  const selectHost = useAppStore((state) => state.selectHost)
  const applyTunnelState = useAppStore((state) => state.applyTunnelState)
  const setTransfers = useAppStore((state) => state.setTransfers)
  const applyTransfer = useAppStore((state) => state.applyTransfer)
  const setTelemetryStatuses = useAppStore((state) => state.setTelemetryStatuses)
  const applyTelemetry = useAppStore((state) => state.applyTelemetry)
  const setCommandJobs = useAppStore((state) => state.setCommandJobs)
  const applyCommandJob = useAppStore((state) => state.applyCommandJob)
  const [operations, setOperations] = useState<OperationSummary[]>([])
  const [terminals, setTerminals] = useState<TerminalSnapshot[]>([])
  const [error, setError] = useState('')
  const [refreshing, setRefreshing] = useState(false)
  const [busyAction, setBusyAction] = useState<string | null>(null)
  const requestTerminalRefresh = useRef<() => void>(() => {})
  const stateRefreshEpoch = useRef(0)

  const hostName = useCallback((hostId: string): string => hosts.find((host) => host.id === hostId)?.alias ?? '已移除的主机', [hosts])
  const openForHost = useCallback((hostId: string, tab: WorkspaceTab): void => {
    selectHost(hostId)
    onOpenWorkspace(tab)
  }, [onOpenWorkspace, selectHost])

  const openTerminal = useCallback((session: TerminalSnapshot): void => {
    openForHost(session.hostId, 'terminal')
    window.setTimeout(() => {
      window.dispatchEvent(new CustomEvent('remotedeck:focus-terminal', { detail: { sessionId: session.sessionId } }))
    }, 0)
  }, [openForHost])

  const refresh = useCallback(async (): Promise<void> => {
    const epoch = ++stateRefreshEpoch.current
    setRefreshing(true)
    const results = await Promise.allSettled([
      api.listOperations(),
      api.listTransfers(),
      api.listTunnels(),
      api.listTelemetry(),
      api.listCommandJobs()
    ])
    if (epoch !== stateRefreshEpoch.current) return
    const [operationResult, transferResult, tunnelResult, telemetryResult, commandResult] = results
    const failures = results.filter((result): result is PromiseRejectedResult => result.status === 'rejected')
    if (operationResult.status === 'fulfilled') setOperations(operationResult.value)
    if (transferResult.status === 'fulfilled') setTransfers(transferResult.value)
    if (tunnelResult.status === 'fulfilled') tunnelResult.value.forEach(applyTunnelState)
    if (telemetryResult.status === 'fulfilled') setTelemetryStatuses(telemetryResult.value)
    if (commandResult.status === 'fulfilled') setCommandJobs(commandResult.value)
    if (failures.length > 0) setError(errorMessage(failures[0].reason))
    setRefreshing(false)
  }, [applyTunnelState, setCommandJobs, setTelemetryStatuses, setTransfers])

  const reload = useCallback((): void => {
    void refresh()
    requestTerminalRefresh.current()
  }, [refresh])

  useEffect(() => {
    let disposed = false
    let epoch = 0
    let listingInFlight = false
    let buffered: TerminalEvent[] = []
    const refreshListing = async (): Promise<void> => {
      const requestEpoch = ++epoch
      buffered = []
      listingInFlight = true
      try {
        const sessions = await api.listTerminals()
        if (disposed || requestEpoch !== epoch) return
        const changes = buffered
        buffered = []
        let next = sessions
        for (const event of changes) next = mergeTerminalEvent(next, event)
        setTerminals(next)
      } catch (reason) {
        if (!disposed && requestEpoch === epoch) setError(errorMessage(reason))
      } finally {
        if (requestEpoch === epoch) listingInFlight = false
      }
    }
    requestTerminalRefresh.current = () => { void refreshListing() }
    const terminalCleanup = events.terminal((event) => {
      if (disposed) return
      if (listingInFlight) buffered.push(event)
      setTerminals((current) => mergeTerminalEvent(current, event))
    })
    const resyncCleanup = events.resync(() => { void refreshListing() })
    void refreshListing()
    return () => {
      disposed = true
      terminalCleanup()
      resyncCleanup()
      requestTerminalRefresh.current = () => {}
    }
  }, [])

  useEffect(() => {
    const operationCleanup = events.operation((item) => setOperations((current) => upsertOperation(current, item)))
    const resyncCleanup = events.resync(reload)
    reload()
    return () => {
      operationCleanup()
      resyncCleanup()
    }
  }, [reload])

  const runAction = async (key: string, action: () => Promise<void>): Promise<void> => {
    if (busyAction !== null) return
    setBusyAction(key)
    setError('')
    try {
      await action()
    } catch (reason) {
      setError(errorMessage(reason))
    } finally {
      setBusyAction(null)
    }
  }

  const activeTransfers = transfers.filter((item) => isActive(item.state)).length
  const activeCommands = commandJobs.filter((item) => isActive(item.state)).length
  const activeTerminals = terminals.filter((item) => item.state === 'starting' || item.state === 'running').length
  const activeTunnels = tunnels.filter((item) => ['starting', 'running', 'waiting'].includes(tunnelStates[item.id]?.state ?? 'stopped')).length
  const activeTelemetry = hosts.filter((host) => ['starting', 'online', 'degraded'].includes(telemetryStatuses[host.id]?.state ?? 'stopped')).length

  return <section className="panel task-panel">
    <header className="panel-heading">
      <div><h1>任务</h1><p>任务、会话、隧道和监控状态均来自本机服务；刷新页面不会重复执行已有工作。</p></div>
      <button disabled={refreshing || busyAction !== null} onClick={reload}><RefreshCw size={14} />{refreshing ? '刷新中' : '刷新'}</button>
    </header>
    {error && <div className="notice error" role="alert">{error}<button onClick={() => setError('')}>关闭</button></div>}
    <section className="task-summary" aria-label="活动工作摘要">
      <strong>{String(activeTransfers + activeCommands + activeTerminals + activeTunnels + activeTelemetry)} 项活动工作</strong>
      <span>{String(activeTerminals)} 个终端</span><span>{String(activeTransfers)} 个传输</span><span>{String(activeCommands)} 个命令</span><span>{String(activeTunnels)} 条隧道</span><span>{String(activeTelemetry)} 个监控</span>
    </section>
    <div className="task-page-columns">
      <TaskList title="SSH 终端会话" empty="暂无打开的 SSH 会话。" items={terminals} render={(session) => <article className="task-page-row" key={session.sessionId}>
        <State state={session.state} label={terminalStateLabel(session.state)} />
        <div><strong>{session.title || session.alias}</strong><small>{session.alias || hostName(session.hostId)} · {session.cwd || '~'} · {terminalStateLabel(session.state)}</small>{session.error && <small className="task-failure">{session.error}</small>}<details><summary>会话详情</summary><dl><Detail label="会话" value={session.sessionId} /><Detail label="主机" value={session.alias || hostName(session.hostId)} /><Detail label="目录" value={session.cwd || '~'} />{session.exitCode !== null && session.exitCode !== undefined && <Detail label="退出码" value={String(session.exitCode)} />}</dl></details></div>
        <div><button onClick={() => openTerminal(session)}>返回终端</button></div>
      </article>} />

      <TaskList title="文件传输" empty="暂无传输任务。" items={transfers.toReversed()} render={(job) => (
        <TransferRow key={job.id} job={job} hostName={hostName(job.hostId)} busy={busyAction !== null}
          onOpen={() => openForHost(job.hostId, 'files')}
          onCancel={() => { void runAction(`transfer-cancel:${job.id}`, async () => { applyTransfer(await api.cancelTransfer(job.id)) }) }}
          onRetry={() => { void runAction(`transfer-retry:${job.id}`, async () => { applyTransfer(await api.retryTransfer(job.id)) }) }}
          onLocate={() => { void runAction(`transfer-locate:${job.id}`, async () => { await api.showTransferInFolder(job.id) }) }} />
      )} />

      <TaskList title="后台命令" empty="暂无命令任务。" items={commandJobs.toReversed()} render={(job) => (
        <CommandRow key={job.id} job={job} hostName={hostName(job.hostId)} busy={busyAction !== null}
          onOpen={() => openForHost(job.hostId, 'commands')}
          onCancel={() => { void runAction(`command-cancel:${job.id}`, async () => { applyCommandJob(await api.cancelCommand(job.id)) }) }} />
      )} />

      <TaskList title="端口隧道" empty="暂无已保存的隧道。" items={tunnels} render={(profile) => (
        <TunnelRow key={profile.id} profile={profile} snapshot={tunnelStates[profile.id]} hostName={hostName(profile.hostId)} busy={busyAction !== null}
          onOpen={() => openForHost(profile.hostId, 'tunnels')}
          onStart={() => { void runAction(`tunnel-start:${profile.id}`, async () => { applyTunnelState(await api.startTunnel(profile.id)) }) }}
          onStop={() => { void runAction(`tunnel-stop:${profile.id}`, async () => { applyTunnelState(await api.stopTunnel(profile.id)) }) }}
          onRestart={() => { void runAction(`tunnel-restart:${profile.id}`, async () => { applyTunnelState(await api.restartTunnel(profile.id)) }) }} />
      )} />

      <TaskList title="监控采集" empty="暂无主机。" items={hosts} render={(host) => (
        <TelemetryRow key={host.id} hostId={host.id} hostName={host.alias} monitorEnabled={host.monitorEnabled} state={telemetryStatuses[host.id]?.state ?? 'stopped'} lastSampleAt={telemetryStatuses[host.id]?.lastSampleAt ?? null} error={telemetryStatuses[host.id]?.error ?? null} busy={busyAction !== null}
          onOpen={() => openForHost(host.id, 'monitor')}
          onStart={() => { void runAction(`telemetry-start:${host.id}`, async () => { applyTelemetry({ status: await api.startTelemetry(host.id), sample: null }) }) }}
          onStop={() => { void runAction(`telemetry-stop:${host.id}`, async () => { applyTelemetry({ status: await api.stopTelemetry(host.id), sample: null }) }) }} />
      )} />

      <TaskList title="服务操作" empty="暂无服务操作。" items={operations} render={(operation) => <article className="task-page-row" key={operation.id}>
        <State state={operation.state} label={operationStateLabel(operation.state)} />
        <div><strong>{operationName(operation.command)}</strong><small>{operation.hostId ? hostName(operation.hostId) : '本机服务'} · {formatDate(operation.createdAt)} · {operationStateLabel(operation.state)}</small>{operation.error && <small className="task-failure">{operation.error.code}：{operation.error.message}</small>}<details><summary>操作详情</summary><dl><Detail label="操作" value={operationName(operation.command)} /><Detail label="状态" value={operationStateLabel(operation.state)} />{operation.completedAt && <Detail label="完成时间" value={formatDate(operation.completedAt)} />}{operation.hostId && <Detail label="主机" value={hostName(operation.hostId)} />}</dl></details></div>
      </article>} />
    </div>
  </section>
}

function TaskList<T>({ title, empty, items, render }: { title: string; empty: string; items: readonly T[]; render: (item: T) => React.JSX.Element }): React.JSX.Element {
  return <section className="task-page-list"><h2>{title}</h2>{items.length === 0 ? <p>{empty}</p> : items.map(render)}</section>
}

function TransferRow({ job, hostName, busy, onOpen, onCancel, onRetry, onLocate }: { job: TransferJob; hostName: string; busy: boolean; onOpen: () => void; onCancel: () => void; onRetry: () => void; onLocate: () => void }): React.JSX.Element {
  const progress = job.totalBytes === null || job.totalBytes <= 0 ? null : Math.min(100, job.bytesTransferred * 100 / job.totalBytes)
  return <article className="task-page-row">
    <State state={job.state} label={transferStateLabel(job.state)} />
    <div><strong>{job.direction === 'upload' ? '上传' : '下载'} · {tail(job.source)} → {tail(job.destination)}</strong><small>{hostName} · {transferStateLabel(job.state)} · {formatBytes(job.bytesTransferred)} / {job.totalBytes === null ? '未知大小' : formatBytes(job.totalBytes)}</small>{progress !== null && <progress max={100} value={progress} aria-label={`传输进度 ${progress.toFixed(1)}%`} />}{job.error && <small className="task-failure">{job.error}</small>}<details><summary>传输详情</summary><dl><Detail label="来源" value={job.source} /><Detail label="目标" value={job.destination} /><Detail label="创建时间" value={formatDate(job.createdAt)} /><Detail label="更新时间" value={formatDate(job.updatedAt)} /></dl></details></div>
    <div className="button-row wrap">{isCancellable(job.state) ? <button disabled={busy} onClick={onCancel}><Square size={12} />取消</button> : job.state === 'cancelling' ? <button disabled><Square size={12} />取消中</button> : isRetryable(job.state) ? <button disabled={busy} onClick={onRetry}><RotateCw size={12} />重试</button> : <button disabled={busy} onClick={onLocate}><FolderOpen size={12} />定位</button>}<button disabled={busy} onClick={onOpen}>查看文件</button></div>
  </article>
}

function CommandRow({ job, hostName, busy, onOpen, onCancel }: { job: CommandJob; hostName: string; busy: boolean; onOpen: () => void; onCancel: () => void }): React.JSX.Element {
  return <article className="task-page-row">
    <State state={job.state} label={commandStateLabel(job.state)} />
    <div><strong>{job.name}</strong><small>{hostName} · {job.risk} · {commandStateLabel(job.state)}{job.exitCode === null ? '' : ` · 退出码 ${String(job.exitCode)}`}</small>{job.error && <small className="task-failure">{job.error}</small>}<details><summary>输出与详情</summary><dl><Detail label="命令" value={job.command} /><Detail label="开始时间" value={job.startedAt ? formatDate(job.startedAt) : '尚未开始'} /><Detail label="结束时间" value={job.finishedAt ? formatDate(job.finishedAt) : '尚未结束'} /></dl>{job.stdout && <><h3>标准输出</h3><pre>{job.stdout}</pre></>}{job.stderr && <><h3>标准错误</h3><pre>{job.stderr}</pre></>}{!job.stdout && !job.stderr && <p>暂无命令输出。</p>}</details></div>
    <div className="button-row wrap">{isCancellable(job.state) ? <button disabled={busy} onClick={onCancel}><Square size={12} />取消</button> : job.state === 'cancelling' ? <button disabled><Square size={12} />取消中</button> : null}<button disabled={busy} onClick={onOpen}>返回命令</button></div>
  </article>
}

function TunnelRow({ profile, snapshot, hostName, busy, onOpen, onStart, onStop, onRestart }: { profile: TunnelProfile; snapshot: TunnelSnapshot | undefined; hostName: string; busy: boolean; onOpen: () => void; onStart: () => void; onStop: () => void; onRestart: () => void }): React.JSX.Element {
  const state = snapshot?.state ?? 'stopped'
  return <article className="task-page-row">
    <State state={state} label={tunnelStateLabel(state)} />
    <div><strong>{profile.name}</strong><small>{hostName} · {profile.direction === 'local' ? '本地转发' : '远端转发'} · {profile.bindAddress}:{String(profile.sourcePort)} → {profile.targetHost}:{String(profile.targetPort)}</small><small>{tunnelStateLabel(state)} · 健康 {tunnelHealthLabel(snapshot?.health)} · 已重连 {String(snapshot?.reconnectCount ?? 0)} 次</small>{snapshot?.message && <small className="task-failure">{snapshot.message}</small>}<details><summary>隧道详情与日志</summary><dl><Detail label="自动启动" value={profile.autoStart ? '是' : '否'} /><Detail label="自动恢复" value={profile.autoReconnect === false ? '否' : '是'} /><Detail label="运行时长" value={formatDuration(snapshot?.uptimeSeconds ?? 0)} /></dl>{snapshot?.logs?.length ? <ol>{snapshot.logs.toReversed().map((entry) => <li key={`${entry.at}-${entry.message}`}><time>{formatDate(entry.at)}</time> · {entry.message}</li>)}</ol> : <p>尚无运行日志。</p>}</details></div>
    <div className="button-row wrap"><button className="primary" disabled={busy || state === 'running' || state === 'starting'} onClick={onStart}><Play size={12} />启动</button><button disabled={busy || state === 'stopped'} onClick={onStop}><Square size={12} />停止</button><button disabled={busy} onClick={onRestart}><RotateCw size={12} />重启</button><button disabled={busy} onClick={onOpen}>管理隧道</button></div>
  </article>
}

function TelemetryRow({ hostId, hostName, monitorEnabled, state, lastSampleAt, error, busy, onOpen, onStart, onStop }: { hostId: string; hostName: string; monitorEnabled: boolean; state: TelemetryRuntimeState; lastSampleAt: string | null; error: string | null; busy: boolean; onOpen: () => void; onStart: () => void; onStop: () => void }): React.JSX.Element {
  return <article className="task-page-row">
    <State state={state} label={telemetryStateLabel(state)} />
    <div><strong>{hostName}</strong><small>{telemetryStateLabel(state)} · {monitorEnabled ? '随服务启动' : '仅手动启动'}{lastSampleAt ? ` · 最近采样 ${formatDate(lastSampleAt)}` : ''}</small>{error && <small className="task-failure">{error}</small>}<details><summary>监控详情</summary><dl><Detail label="主机" value={hostName} /><Detail label="主机标识" value={hostId} /><Detail label="最近采样" value={lastSampleAt ? formatDate(lastSampleAt) : '暂无'} /></dl></details></div>
    <div className="button-row wrap">{state === 'stopped' || state === 'failed' ? <button className="primary" disabled={busy} onClick={onStart}><Play size={12} />启动</button> : <button disabled={busy || state === 'starting'} onClick={onStop}><Square size={12} />停止</button>}<button disabled={busy} onClick={onOpen}>打开监控</button></div>
  </article>
}

function Detail({ label, value }: { label: string; value: string }): React.JSX.Element { return <><dt>{label}</dt><dd>{value}</dd></> }
function State({ state, label }: { state: string; label: string }): React.JSX.Element { return <span className={`task-state task-${state}`} title={label} aria-label={label} /> }

function mergeTerminalEvent(current: TerminalSnapshot[], event: TerminalEvent): TerminalSnapshot[] {
  const index = current.findIndex((item) => item.sessionId === event.sessionId)
  if (event.snapshot) return index < 0 ? [...current, event.snapshot] : current.map((item, itemIndex) => itemIndex === index ? { ...item, ...event.snapshot } : item)
  if (index < 0) return current
  return current.map((item, itemIndex) => {
    if (itemIndex !== index) return item
    if (event.kind === 'exit') return { ...item, state: 'closed', exitCode: event.exitCode }
    if (event.kind === 'error') return { ...item, state: 'failed', error: event.message }
    if (event.kind === 'started') return { ...item, state: 'running' }
    return item
  })
}

function upsertOperation(current: OperationSummary[], incoming: OperationSummary): OperationSummary[] { return current.some((item) => item.id === incoming.id) ? current.map((item) => item.id === incoming.id ? incoming : item) : [incoming, ...current] }
function isActive(state: string): boolean { return state === 'queued' || state === 'running' || state === 'cancelling' }
function isCancellable(state: string): boolean { return state === 'queued' || state === 'running' }
function isRetryable(state: string): boolean { return state === 'failed' || state === 'cancelled' }
function tail(path: string): string { return path.replace(/\\/gu, '/').split('/').filter(Boolean).at(-1) ?? path }
function formatDate(value: string): string { const date = new Date(value); return Number.isNaN(date.getTime()) ? value : date.toLocaleString() }
function formatBytes(value: number): string { if (value < 1024) return `${String(value)} B`; if (value < 1024 ** 2) return `${(value / 1024).toFixed(1)} KiB`; if (value < 1024 ** 3) return `${(value / 1024 ** 2).toFixed(1)} MiB`; return `${(value / 1024 ** 3).toFixed(1)} GiB` }
function formatDuration(seconds: number): string { if (seconds < 60) return `${String(seconds)} 秒`; if (seconds < 3600) return `${String(Math.floor(seconds / 60))} 分`; return `${String(Math.floor(seconds / 3600))} 小时 ${String(Math.floor(seconds % 3600 / 60))} 分` }
function terminalStateLabel(state: TerminalSnapshot['state']): string { return { starting: '正在建立', running: '已连接', offline: '离线', closed: '已关闭', failed: '失败' }[state] }
function transferStateLabel(state: TransferJob['state']): string { return { queued: '排队中', running: '传输中', cancelling: '取消中', completed: '已完成', failed: '失败', cancelled: '已取消' }[state] }
function commandStateLabel(state: CommandJob['state']): string { return { queued: '排队中', running: '执行中', cancelling: '取消中', completed: '已完成', failed: '失败', cancelled: '已取消' }[state] }
function tunnelStateLabel(state: TunnelRuntimeState): string { return { stopped: '已停止', starting: '启动中', running: '运行中', waiting: '等待恢复', failed: '失败' }[state] }
function telemetryStateLabel(state: TelemetryRuntimeState): string { return { stopped: '已停止', starting: '启动中', online: '在线', degraded: '降级', failed: '失败' }[state] }
function tunnelHealthLabel(health: TunnelSnapshot['health']): string { return health ? { unknown: '未知', healthy: '正常', degraded: '降级', failed: '失败' }[health] : '未知' }
function operationStateLabel(state: OperationSummary['state']): string { return { queued: '排队中', running: '执行中', completed: '已完成', failed: '失败' }[state] }
function operationName(command: string): string {
  const labels: Record<string, string> = {
    bootstrap: '同步服务状态', update_settings: '保存设置', pick_local_path: '选择本机路径', pick_save_path: '选择保存位置', save_host: '保存主机', delete_host: '删除主机', import_ssh_config: '导入 OpenSSH 配置', scan_host_keys: '扫描主机指纹', accept_host_key: '确认主机指纹', list_host_keys: '读取受信任指纹', remove_host_key: '删除受信任指纹', test_connection: '测试 SSH 连接', list_keys: '读取密钥列表', generate_key: '生成密钥', deploy_key: '部署公钥', list_terminals: '读取终端会话', start_terminal: '打开终端', reconnect_terminal: '重新连接终端', resize_terminal: '同步终端尺寸', close_terminal: '关闭终端', sftp_list: '浏览远端文件', sftp_create_directory: '新建远端目录', sftp_rename: '重命名远端文件', sftp_delete: '删除远端文件', transfer_list: '读取传输任务', transfer_upload: '上传文件', transfer_download: '下载文件', transfer_cancel: '取消传输', transfer_retry: '重试传输', transfer_show_in_folder: '定位本机文件', list_tunnels: '读取隧道状态', save_tunnel: '保存隧道', delete_tunnel: '删除隧道', start_tunnel: '启动隧道', stop_tunnel: '停止隧道', restart_tunnel: '重启隧道', telemetry_list: '读取监控状态', telemetry_history: '读取监控历史', telemetry_start: '启动监控', telemetry_stop: '停止监控', signal_process: '向远端进程发送信号', btop_probe: '检测 btop', btop_start_watchdog: '启动 btop 监控', btop_stop_watchdog: '停止 btop 监控', list_commands: '读取命令预设', save_command: '保存命令预设', delete_command: '删除命令预设', analyze_command: '复核命令风险', run_command: '执行经复核的命令', list_command_jobs: '读取命令任务', cancel_command: '取消命令', probe_agent: '检测 Agent', agent_session_plan: '生成 Agent 操作计划', start_agent_session: '启动 Agent 会话', legacy_preview: '预览旧版迁移', legacy_apply: '执行旧版迁移', export_diagnostics: '导出诊断信息'
  }
  return labels[command] ?? '服务操作'
}
