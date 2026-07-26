import { useEffect } from 'react'
import { CircleAlert, Command, HardDrive, Server, Settings, TerminalSquare } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { listenTunnel } from './api'
import { type Activity, useAppStore } from './store'
import { HostPanel } from './panels/HostPanel'
import { TerminalPanel } from './panels/TerminalPanel'
import { TunnelPanel } from './panels/TunnelPanel'
import { CommandPanel } from './panels/CommandPanel'
import { SettingsPanel } from './panels/SettingsPanel'

const activities: Array<{ id: Activity; label: string; icon: LucideIcon }> = [
  { id: 'hosts', label: '主机', icon: Server },
  { id: 'terminal', label: '终端', icon: TerminalSquare },
  { id: 'tunnels', label: '隧道', icon: HardDrive },
  { id: 'commands', label: '命令', icon: Command },
  { id: 'settings', label: '设置', icon: Settings }
]

export function App(): React.JSX.Element {
  const activity = useAppStore((state) => state.activity)
  const setActivity = useAppStore((state) => state.setActivity)
  const bootstrap = useAppStore((state) => state.bootstrap)
  const applyTunnelState = useAppStore((state) => state.applyTunnelState)
  const loading = useAppStore((state) => state.loading)
  const error = useAppStore((state) => state.error)
  const clearError = useAppStore((state) => state.clearError)
  const appVersion = useAppStore((state) => state.appVersion)
  const hosts = useAppStore((state) => state.hosts)
  const selectedHostId = useAppStore((state) => state.selectedHostId)
  const selectHost = useAppStore((state) => state.selectHost)
  const capabilities = useAppStore((state) => state.capabilities)
  const selectedHost = hosts.find((host) => host.id === selectedHostId) ?? null

  useEffect(() => { void bootstrap() }, [bootstrap])
  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    void listenTunnel((event) => applyTunnelState(event)).then((cleanup) => {
      if (disposed) cleanup()
      else unlisten = cleanup
    }).catch(() => undefined)
    return () => { disposed = true; unlisten?.() }
  }, [applyTunnelState])

  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (!event.ctrlKey || event.altKey) return
      const mapping: Record<string, Activity> = { '1': 'hosts', '2': 'terminal', '3': 'tunnels', '4': 'commands', '5': 'settings' }
      const next = mapping[event.key]
      if (next) { event.preventDefault(); setActivity(next) }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [setActivity])

  if (loading) return <main className="state-screen"><div className="loading-ring" /><h1>RemoteDeck</h1><p>正在启动 Tauri 2 本地核心…</p></main>

  return (
    <div className="app-shell">
      <header className="titlebar">
        <div className="brand"><span className="brand-mark">R</span><span>RemoteDeck</span><small>Tauri 2</small></div>
        <div className="context-line"><span>{selectedHost?.alias ?? '未选择主机'}</span><span>/</span><span>{selectedHost?.defaultWorkspace ?? '~'}</span></div>
        <span className="version">v{appVersion}</span>
      </header>

      <nav className="activity-rail" aria-label="工作区导航">
        {activities.map(({ id, label, icon: Icon }, index) => (
          <button key={id} className={activity === id ? 'activity-button active' : 'activity-button'} onClick={() => setActivity(id)} title={`${label}  Ctrl+${index + 1}`} aria-label={label}>
            <Icon size={21} strokeWidth={1.7} />
          </button>
        ))}
      </nav>

      <aside className="sidebar">
        <div className="sidebar-heading">主机</div>
        <div className="host-list">
          {hosts.map((host) => (
            <button key={host.id} className={host.id === selectedHostId ? 'host-item selected' : 'host-item'} onClick={() => selectHost(host.id)}>
              <span className="status-dot" /><span><strong>{host.alias}</strong><small>{host.username}@{host.hostname}:{host.port}</small></span>
            </button>
          ))}
          {hosts.length === 0 && <p className="empty-copy">先添加一台 Linux 主机。</p>}
        </div>
        <div className="runtime-card">
          <span className={capabilities?.sshPath ? 'runtime-ok' : 'runtime-bad'}>{capabilities?.sshPath ? 'OpenSSH 已发现' : '未发现 ssh.exe'}</span>
          <small>{capabilities?.sshPath ?? '请安装 Windows OpenSSH Client'}</small>
        </div>
      </aside>

      <main className="workspace">
        {error && <div className="error-banner" role="alert"><CircleAlert size={18} /><span>{error}</span><button onClick={clearError}>关闭</button></div>}
        {activity === 'hosts' && <HostPanel />}
        <TerminalPanel hidden={activity !== 'terminal'} />
        {activity === 'tunnels' && <TunnelPanel />}
        {activity === 'commands' && <CommandPanel />}
        {activity === 'settings' && <SettingsPanel />}
      </main>
    </div>
  )
}
