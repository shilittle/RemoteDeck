import { useEffect, useMemo, useState } from 'react'
import { Activity as ActivityIcon, Bot, CircleAlert, Files, HardDrive, Search, Server, Settings, TerminalSquare } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { events, api } from './api'
import { type Activity, useAppStore } from './store'
import { OnboardingGuide } from './components/OnboardingGuide'
import { TaskCenter } from './components/TaskCenter'
import { CommandPanel } from './panels/CommandPanel'
import { FilePanel } from './panels/FilePanel'
import { HostPanel } from './panels/HostPanel'
import { SettingsPanel } from './panels/SettingsPanel'
import { TelemetryPanel } from './panels/TelemetryPanel'
import { TerminalPanel } from './panels/TerminalPanel'
import { TunnelPanel } from './panels/TunnelPanel'

const activities: Array<{ id: Activity; label: string; icon: LucideIcon }> = [
  { id: 'hosts', label: '主机', icon: Server },
  { id: 'terminal', label: '终端', icon: TerminalSquare },
  { id: 'files', label: '文件', icon: Files },
  { id: 'tunnels', label: '隧道', icon: HardDrive },
  { id: 'monitor', label: '监控', icon: ActivityIcon },
  { id: 'commands', label: '命令与 Agent', icon: Bot },
  { id: 'settings', label: '设置', icon: Settings }
]

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
  const selectedHost = hosts.find((host) => host.id === selectedHostId) ?? null
  const filteredHosts = useMemo(() => {
    const query = hostSearch.trim().toLowerCase()
    return hosts.filter((host) => !query || `${host.alias} ${host.hostname} ${host.username} ${host.groups.join(' ')}`.toLowerCase().includes(query))
  }, [hostSearch, hosts])

  useEffect(() => {
    const cleanups: Array<() => void> = []
    const lifecycle = { disposed: false }
    const isDisposed = (): boolean => lifecycle.disposed
    void (async () => {
      const subscriptions = await Promise.allSettled([
        events.tunnel(applyTunnelState),
        events.transfer(({ job }) => applyTransfer(job)),
        events.telemetry(applyTelemetry),
        events.command(({ job }) => applyCommandJob(job))
      ])
      for (const subscription of subscriptions) {
        if (subscription.status === 'fulfilled') {
          if (lifecycle.disposed) subscription.value()
          else cleanups.push(subscription.value)
        } else if (!lifecycle.disposed) {
          reportError(subscription.reason)
        }
      }
      if (isDisposed()) return
      await bootstrap()
      if (isDisposed()) return
      const [tunnelSnapshots, transfers, telemetry, commands] = await Promise.allSettled([api.listTunnels(), api.listTransfers(), api.listTelemetry(), api.listCommandJobs()])
      if (tunnelSnapshots.status === 'fulfilled') tunnelSnapshots.value.forEach(applyTunnelState)
      if (transfers.status === 'fulfilled') setTransfers(transfers.value)
      if (telemetry.status === 'fulfilled') setTelemetryStatuses(telemetry.value)
      if (commands.status === 'fulfilled') setCommandJobs(commands.value)
    })().catch((reason: unknown) => { if (!lifecycle.disposed) reportError(reason) })
    return () => { lifecycle.disposed = true; cleanups.forEach((cleanup) => cleanup()) }
  }, [applyCommandJob, applyTelemetry, applyTransfer, applyTunnelState, bootstrap, reportError, setCommandJobs, setTelemetryStatuses, setTransfers])

  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (!event.ctrlKey || event.altKey) return
      const mapping: Partial<Record<string, Activity>> = { '1': 'hosts', '2': 'terminal', '3': 'files', '4': 'tunnels', '5': 'monitor', '6': 'commands', '7': 'settings' }
      const next = mapping[event.key]
      if (next) { event.preventDefault(); setActivity(next); return }
      if (event.shiftKey && event.key.toLowerCase() === 't') { event.preventDefault(); setActivity('terminal'); window.dispatchEvent(new Event('remotedeck:new-terminal')); return }
      if (event.key.toLowerCase() === 'j') { event.preventDefault(); window.dispatchEvent(new Event('remotedeck:toggle-tasks')); return }
      if (event.key === ',') { event.preventDefault(); setActivity('settings') }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [setActivity])

  if (loading) return <main className="state-screen"><div className="loading-ring" /><h1>RemoteDeck</h1><p>正在启动 Tauri 2 本地核心…</p></main>

  return (
    <div className="app-shell">
      <header className="titlebar">
        <div className="brand"><span className="brand-mark">R</span><span>RemoteDeck</span><small>Tauri 2</small></div>
        <div className="context-line"><span>{selectedHost?.alias ?? '未选择主机'}</span><span>/</span><span>{selectedHost?.defaultWorkspace ?? '~'}</span><span>/</span><span>{activities.find((item) => item.id === activity)?.label}</span></div>
        <span className="version">v{appVersion}</span>
      </header>

      <nav className="activity-rail" aria-label="工作区导航">
        {activities.map(({ id, label, icon: Icon }, index) => <button key={id} className={activity === id ? 'activity-button active' : 'activity-button'} onClick={() => setActivity(id)} title={`${label} · Ctrl+${String(index + 1)}`} aria-label={label}><Icon size={20} strokeWidth={1.7} /></button>)}
      </nav>

      <aside className="sidebar">
        <div className="sidebar-heading"><span>主机</span><small>{String(hosts.length)}</small></div>
        <label className="sidebar-search"><SearchIcon /><input aria-label="搜索主机" placeholder="搜索主机或分组" value={hostSearch} onChange={(event) => setHostSearch(event.target.value)} /></label>
        <div className="host-list">
          {filteredHosts.map((host) => <button key={host.id} className={host.id === selectedHostId ? 'host-item selected' : 'host-item'} onClick={() => selectHost(host.id)}><span className="status-dot" /><span><strong>{host.alias}</strong><small>{host.groups.length ? host.groups.join(' · ') : `${host.username}@${host.hostname}`}</small></span></button>)}
          {hosts.length === 0 && <p className="empty-copy">先添加一台 Linux SSH 主机。</p>}
          {hosts.length > 0 && filteredHosts.length === 0 && <p className="empty-copy">没有匹配的主机。</p>}
        </div>
        <div className="runtime-card"><span className={capabilities?.sshPath ? 'runtime-ok' : 'runtime-bad'}>{capabilities?.sshPath ? 'OpenSSH 已发现' : '未发现 ssh.exe'}</span><small>{capabilities?.sshPath ?? '请安装 Windows OpenSSH Client'}</small></div>
      </aside>

      <main className="workspace">
        {error && <div className="error-banner" role="alert"><CircleAlert size={18} /><span>{error}</span><button onClick={clearError}>关闭</button></div>}
        {activity === 'hosts' && <HostPanel />}
        <TerminalPanel hidden={activity !== 'terminal'} />
        {activity === 'files' && <FilePanel />}
        {activity === 'tunnels' && <TunnelPanel />}
        {activity === 'monitor' && <TelemetryPanel />}
        {activity === 'commands' && <CommandPanel />}
        {activity === 'settings' && <SettingsPanel />}
      </main>
      <OnboardingGuide />
      <TaskCenter />
    </div>
  )
}

function SearchIcon(): React.JSX.Element { return <Search size={13} /> }
