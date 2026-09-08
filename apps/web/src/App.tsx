import { useEffect, useMemo, useRef, useState } from 'react'
import { CircleAlert, Search } from 'lucide-react'
import { api, errorMessage, events, initializeSession } from './api'
import { OnboardingGuide } from './components/OnboardingGuide'
import { CommandPanel } from './panels/CommandPanel'
import { FilePanel } from './panels/FilePanel'
import { HostPanel } from './panels/HostPanel'
import { SettingsPanel } from './panels/SettingsPanel'
import { TaskPanel } from './panels/TaskPanel'
import { TelemetryPanel } from './panels/TelemetryPanel'
import { TerminalPanel } from './panels/TerminalPanel'
import { TunnelPanel } from './panels/TunnelPanel'
import { type Activity, useAppStore } from './store'

type PrimaryPage = 'hosts' | 'workspace' | 'tasks' | 'settings'
type WorkspaceTab = 'terminal' | 'files' | 'commands' | 'monitor' | 'tunnels'
const workspaceTabs: Array<{ id: WorkspaceTab; label: string }> = [
  { id: 'terminal', label: '终端' }, { id: 'files', label: '文件' }, { id: 'commands', label: '命令与 Agent' }, { id: 'monitor', label: '监控' }, { id: 'tunnels', label: '隧道' }
]

function pageFor(activity: Activity): PrimaryPage { return activity === 'hosts' || activity === 'settings' || activity === 'tasks' ? activity : 'workspace' }

export function App(): React.JSX.Element {
  const activity = useAppStore((state) => state.activity)
  const setActivity = useAppStore((state) => state.setActivity)
  const bootstrap = useAppStore((state) => state.bootstrap)
  const loading = useAppStore((state) => state.loading)
  const error = useAppStore((state) => state.error)
  const clearError = useAppStore((state) => state.clearError)
  const reportError = useAppStore((state) => state.reportError)
  const appVersion = useAppStore((state) => state.appVersion)
  const hosts = useAppStore((state) => state.hosts)
  const selectedHostId = useAppStore((state) => state.selectedHostId)
  const selectHost = useAppStore((state) => state.selectHost)
  const capabilities = useAppStore((state) => state.capabilities)
  const applyTunnelState = useAppStore((state) => state.applyTunnelState)
  const applyTransfer = useAppStore((state) => state.applyTransfer)
  const setTransfers = useAppStore((state) => state.setTransfers)
  const applyTelemetry = useAppStore((state) => state.applyTelemetry)
  const setTelemetryStatuses = useAppStore((state) => state.setTelemetryStatuses)
  const applyCommandJob = useAppStore((state) => state.applyCommandJob)
  const setCommandJobs = useAppStore((state) => state.setCommandJobs)
  const [hostSearch, setHostSearch] = useState('')
  const [sessionError, setSessionError] = useState('')
  const refreshEpoch = useRef(0)
  const selectedHost = hosts.find((host) => host.id === selectedHostId) ?? null
  const page = pageFor(activity)
  const filteredHosts = useMemo(() => {
    const query = hostSearch.trim().toLowerCase()
    return hosts.filter((host) => !query || `${host.alias} ${host.hostname} ${host.username} ${host.groups.join(' ')}`.toLowerCase().includes(query))
  }, [hostSearch, hosts])

  useEffect(() => {
    let disposed = false
    const refresh = async (): Promise<void> => {
      const epoch = ++refreshEpoch.current
      await bootstrap()
      const snapshots = await Promise.allSettled([api.listTunnels(), api.listTransfers(), api.listTelemetry(), api.listCommandJobs()])
      if (disposed || epoch !== refreshEpoch.current) return
      if (snapshots[0].status === 'fulfilled') snapshots[0].value.forEach(applyTunnelState)
      if (snapshots[1].status === 'fulfilled') setTransfers(snapshots[1].value)
      if (snapshots[2].status === 'fulfilled') setTelemetryStatuses(snapshots[2].value)
      if (snapshots[3].status === 'fulfilled') setCommandJobs(snapshots[3].value)
    }
    const cleanups: Array<() => void> = []
    void (async () => {
      try {
        await initializeSession()
        // Subscribe before snapshots so that a state change cannot be lost in between.
        cleanups.push(
          events.tunnel(applyTunnelState),
          events.transfer(({ job }) => applyTransfer(job)),
          events.telemetry(applyTelemetry),
          events.command(({ job }) => applyCommandJob(job)),
          events.resync(() => { void refresh().catch((reason: unknown) => { if (!disposed) reportError(reason) }) })
        )
        await refresh()
      } catch (reason) { setSessionError(errorMessage(reason)) }
    })()
    return () => { disposed = true; cleanups.forEach((cleanup) => cleanup()) }
  }, [applyCommandJob, applyTelemetry, applyTransfer, applyTunnelState, bootstrap, reportError, setCommandJobs, setTelemetryStatuses, setTransfers])

  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (!event.ctrlKey || event.altKey) return
      if (event.key === '1') { event.preventDefault(); setActivity('hosts'); return }
      if (event.key === '2') { event.preventDefault(); setActivity('terminal'); return }
      if (event.key === '3') { event.preventDefault(); setActivity('files'); return }
      if (event.key === '4') { event.preventDefault(); setActivity('commands'); return }
      if (event.key === '5') { event.preventDefault(); setActivity('monitor'); return }
      if (event.key === '6') { event.preventDefault(); setActivity('tunnels'); return }
      if (event.key.toLowerCase() === 'j') { event.preventDefault(); document.getElementById('tasks')?.click() }
      if (event.key === ',') { event.preventDefault(); setActivity('settings') }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [setActivity])

  if (sessionError) return <main className="state-screen"><h1>无法打开 RemoteDeck</h1><p>{sessionError}</p></main>
  if (loading && !appVersion) return <main className="state-screen"><div className="loading-ring" /><h1>RemoteDeck</h1><p>正在连接本机服务…</p></main>

  const openWorkspace = (tab: WorkspaceTab): void => setActivity(tab)
  return <div className="web-app-shell">
    <header className="web-header">
      <div className="brand"><span className="brand-mark">R</span><span>RemoteDeck</span><small>本机服务</small></div>
      <nav className="primary-nav" aria-label="主导航">
        <button className={page === 'hosts' ? 'active' : ''} onClick={() => setActivity('hosts')}>主机</button>
        <button className={page === 'workspace' ? 'active' : ''} onClick={() => openWorkspace('terminal')}>工作区</button>
        <button id="tasks" className={page === 'tasks' ? 'active' : ''} onClick={() => setActivity('tasks')}>任务</button>
        <button className={page === 'settings' ? 'active' : ''} onClick={() => setActivity('settings')}>设置</button>
      </nav>
      <span className="version">{appVersion ? `v${appVersion}` : '本机服务'}</span>
    </header>
    <aside className="sidebar web-sidebar">
      <div className="sidebar-heading"><span>当前主机</span><small>{String(hosts.length)}</small></div>
      <label className="sidebar-search"><Search size={13} /><input aria-label="搜索主机" placeholder="搜索主机或分组" value={hostSearch} onChange={(event) => setHostSearch(event.target.value)} /></label>
      <div className="host-list">{filteredHosts.map((host) => <button key={host.id} className={host.id === selectedHostId ? 'host-item selected' : 'host-item'} onClick={() => selectHost(host.id)}><span className="status-dot" /><span><strong>{host.alias}</strong><small>{host.groups.length ? host.groups.join(' · ') : `${host.username}@${host.hostname}`}</small></span></button>)}{hosts.length === 0 && <p className="empty-copy">先保存一台 SSH 主机。</p>}{hosts.length > 0 && filteredHosts.length === 0 && <p className="empty-copy">没有匹配的主机。</p>}</div>
      <div className="runtime-card"><span className={capabilities?.sshPath ? 'runtime-ok' : 'runtime-bad'}>{capabilities?.sshPath ? 'OpenSSH 已发现' : '未发现 ssh.exe'}</span><small>{capabilities?.sshPath ?? '请安装 Windows OpenSSH Client'}</small></div>
    </aside>
    <main className="workspace web-workspace">
      {error && <div className="error-banner" role="alert"><CircleAlert size={18} /><span>{error}</span><button onClick={clearError}>关闭</button></div>}
      {page === 'hosts' && <HostPanel key={selectedHostId ?? 'new'} />}
      {page === 'tasks' && <TaskPanel onOpenWorkspace={openWorkspace} />}
      {page === 'settings' && <SettingsPanel />}
      {page === 'workspace' && <>
        <div className="workspace-context"><span>{selectedHost ? `${selectedHost.alias} · ${selectedHost.defaultWorkspace || '~'}` : '未选择主机'}</span><nav aria-label="工作区工具">{workspaceTabs.map((tab) => <button key={tab.id} className={activity === tab.id ? 'active' : ''} onClick={() => openWorkspace(tab.id)}>{tab.label}</button>)}</nav></div>
        <TerminalPanel hidden={activity !== 'terminal'} />
        {activity === 'files' && <FilePanel />}
        {activity === 'commands' && <CommandPanel />}
        {activity === 'monitor' && <TelemetryPanel />}
        {activity === 'tunnels' && <TunnelPanel />}
      </>}
    </main>
    <OnboardingGuide />
  </div>
}
