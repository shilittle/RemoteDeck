import { StrictMode, useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Command, Files, HardDrive, MonitorCog, Save, Server, Settings, TerminalSquare } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { AppSettingsPatch } from '../protocol/settings'
import { type Activity, useAppStore } from './store'
import './styles.css'

const activities: Array<{ id: Activity; label: string; icon: LucideIcon }> = [
  { id: 'hosts', label: '主机', icon: Server },
  { id: 'files', label: '文件', icon: Files },
  { id: 'tunnels', label: '隧道', icon: HardDrive },
  { id: 'commands', label: '命令', icon: Command },
  { id: 'settings', label: '设置', icon: Settings }
]

function App(): React.JSX.Element {
  const { activity, setActivity, bootstrap, loading, error, appVersion } = useAppStore()
  useEffect(() => { void bootstrap() }, [bootstrap])

  if (loading) return <StateScreen title="正在加载 RemoteDeck" detail="正在读取本地设置与运行环境…" />

  return (
    <div className="workbench">
      <header className="titlebar">
        <span className="brand"><span className="brand-mark">R</span>RemoteDeck</span>
        <div className="title-context"><span>未选择主机</span><span className="separator">/</span><span>未连接</span></div>
        <span className="version">v{appVersion}</span>
      </header>
      <nav className="activity-rail" aria-label="工作区导航">
        {activities.map(({ id, label, icon: Icon }) => (
          <button key={id} className={activity === id ? 'activity active' : 'activity'} onClick={() => setActivity(id)} aria-label={label} title={label}>
            <Icon size={21} strokeWidth={1.7} />
          </button>
        ))}
      </nav>
      <aside className="sidebar">
        <div className="section-label">{activities.find((item) => item.id === activity)?.label}</div>
        <SidebarContent activity={activity} />
      </aside>
      <main className="workspace">
        {error && <div className="error-banner" role="alert">{error}</div>}
        <WorkspaceContent activity={activity} />
      </main>
      <footer className="statusbar">
        <span><MonitorCog size={14} /> 本地服务就绪</span>
        <span>SSH 未连接</span>
      </footer>
    </div>
  )
}

function SidebarContent({ activity }: { activity: Activity }): React.JSX.Element {
  if (activity === 'settings') return <p className="sidebar-copy">应用与连接默认行为</p>
  if (activity === 'hosts') return <p className="sidebar-copy">暂无主机。完成首次添加后会在此分组显示。</p>
  return <p className="sidebar-copy">选择并连接主机后，可在此访问{activity === 'files' ? '远程文件' : activity === 'tunnels' ? '端口隧道' : '命令预设'}。</p>
}

function WorkspaceContent({ activity }: { activity: Activity }): React.JSX.Element {
  const setActivity = useAppStore((state) => state.setActivity)
  if (activity === 'settings') return <SettingsPanel />
  const copy = {
    hosts: ['连接你的 Linux 工作站', 'RemoteDeck 会在首次连接时展示并要求确认服务器主机指纹。'],
    files: ['远程文件', '连接主机后可浏览、上传和下载 SFTP 文件。'],
    tunnels: ['隧道管理器', '连接主机后可创建独立的 LocalForward 与 RemoteForward。'],
    commands: ['命令预设', '连接主机后可执行带风险分级和确认策略的命令。']
  }[activity]
  return (
    <section className="empty-state">
      <TerminalSquare size={42} strokeWidth={1.35} />
      <h1>{copy[0]}</h1>
      <p>{copy[1]}</p>
      {activity !== 'hosts' && <button className="primary" onClick={() => setActivity('hosts')}>返回主机</button>}
      {activity === 'hosts' && <button className="secondary" onClick={() => setActivity('settings')}>查看默认设置</button>}
    </section>
  )
}

function SettingsPanel(): React.JSX.Element {
  const settings = useAppStore((state) => state.settings)
  const saving = useAppStore((state) => state.saving)
  const updateSettings = useAppStore((state) => state.updateSettings)
  const [draft, setDraft] = useState<AppSettingsPatch>({})
  if (!settings) return <StateScreen title="设置不可用" detail="本地设置尚未成功加载。" />
  const value = { ...settings, ...draft }
  return (
    <section className="settings-panel">
      <div className="panel-heading"><div><h1>设置</h1><p>这些选项会原子写入本地版本化配置。</p></div><button className="primary" disabled={saving || Object.keys(draft).length === 0} onClick={() => { void updateSettings(draft); setDraft({}) }}><Save size={16} />{saving ? '保存中…' : '保存'}</button></div>
      <div className="settings-grid">
        <label><span>终端字体</span><input value={value.terminalFontFamily} onChange={(event) => setDraft({ ...draft, terminalFontFamily: event.target.value })} /></label>
        <label><span>终端字号</span><input type="number" min="9" max="32" value={value.terminalFontSize} onChange={(event) => setDraft({ ...draft, terminalFontSize: Number(event.target.value) })} /></label>
        <label><span>监控采样（秒）</span><input type="number" min="1" max="60" value={value.telemetryIntervalSeconds} onChange={(event) => setDraft({ ...draft, telemetryIntervalSeconds: Number(event.target.value) })} /></label>
        <label><span>历史保留（分钟）</span><input type="number" min="1" max="1440" value={value.telemetryRetentionMinutes} onChange={(event) => setDraft({ ...draft, telemetryRetentionMinutes: Number(event.target.value) })} /></label>
        <label><span>日志级别</span><select value={value.logLevel} onChange={(event) => setDraft({ ...draft, logLevel: event.target.value as typeof value.logLevel })}><option value="debug">Debug</option><option value="info">Info</option><option value="warn">Warn</option><option value="error">Error</option></select></label>
        <label className="toggle"><input type="checkbox" checked={value.autoReconnect} onChange={(event) => setDraft({ ...draft, autoReconnect: event.target.checked })} /><span>网络恢复后自动重连</span></label>
        <label className="toggle"><input type="checkbox" checked={value.closeToTray} onChange={(event) => setDraft({ ...draft, closeToTray: event.target.checked })} /><span>关闭窗口时保留到托盘</span></label>
      </div>
    </section>
  )
}

function StateScreen({ title, detail }: { title: string; detail: string }): React.JSX.Element {
  return <main className="state-screen"><div className="loading-ring" /><h1>{title}</h1><p>{detail}</p></main>
}

const root = document.getElementById('root')
if (!root) throw new Error('Renderer root is missing')
createRoot(root).render(<StrictMode><App /></StrictMode>)
