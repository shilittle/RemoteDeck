import { useState } from 'react'
import { FileSearch, Import, ShieldAlert } from 'lucide-react'
import type { LegacyMigrationPreview } from '../../../protocol/migration'
import type { z } from 'zod'
import type { authInputSchema } from '../../../protocol/ssh'
import { useHostStore } from '../../host-store'
import { useAppStore } from '../../store'

type AuthInput = z.infer<typeof authInputSchema>

export function LegacyMigrationPanel(): React.JSX.Element {
  const [preview, setPreview] = useState<LegacyMigrationPreview | null>(null)
  const [sourcePath, setSourcePath] = useState('')
  const [alias, setAlias] = useState('')
  const [hostname, setHostname] = useState('')
  const [port, setPort] = useState(22)
  const [username, setUsername] = useState('')
  const [workspace, setWorkspace] = useState('~')
  const [authMethod, setAuthMethod] = useState<AuthInput['method']>('password')
  const [identityFile, setIdentityFile] = useState('')
  const [includeTunnel, setIncludeTunnel] = useState(true)
  const [includeCommands, setIncludeCommands] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [result, setResult] = useState('')

  async function choose(): Promise<void> {
    setError(''); setResult('')
    const selection = await window.remoteDeck.legacy.pick()
    if (!selection.path) return
    setSourcePath(selection.path)
    await inspect(selection.path)
  }

  async function inspect(path = sourcePath): Promise<void> {
    if (!path.trim()) return
    setBusy(true); setError(''); setResult('')
    try {
      const next = await window.remoteDeck.legacy.preview(path)
      setPreview(next); setAlias(next.sshAlias); setHostname(next.sshAlias)
    } catch (reason) { setError(messageOf(reason)); setPreview(null) }
    finally { setBusy(false) }
  }

  async function apply(): Promise<void> {
    if (!preview) return
    setBusy(true); setError('')
    try {
      const auth: AuthInput = authMethod === 'private_key' ? { name: 'Legacy import key', method: authMethod, identityFile } : authMethod === 'agent' ? { name: 'Legacy import agent', method: authMethod, agent: 'windows_openssh' } : { name: 'Legacy import authentication', method: authMethod }
      const imported = await window.remoteDeck.legacy.apply({ sourcePath: preview.sourcePath, sourceHash: preview.sourceHash, host: { alias, hostname, port, username, ...(workspace.trim() ? { workspacePath: workspace.trim() } : {}), auth }, includeTunnel, includeCommands })
      setResult(imported.duplicate ? '该配置以前已导入；本次没有创建重复数据。' : `导入完成：1 台主机、${String(imported.tunnelIds.length)} 条隧道、${String(imported.commandIds.length)} 个命令预设。`)
      await Promise.all([useHostStore.getState().load(), useAppStore.getState().updateSettings({})])
      setPreview(await window.remoteDeck.legacy.preview(preview.sourcePath))
    } catch (reason) { setError(messageOf(reason)) }
    finally { setBusy(false) }
  }

  return <section className="legacy-migration card"><div className="card-title"><Import size={16} /><div><h2>迁移 LabPulse SSH v0.1.0</h2><p>选择旧 config.json，先预览，再显式导入。相同内容不会重复导入。</p></div></div><div className="legacy-source"><input aria-label="旧版 config.json 路径" value={sourcePath} onChange={(event) => setSourcePath(event.target.value)} placeholder="选择旧版 config.json" /><button disabled={busy} onClick={() => { void choose() }}><FileSearch size={14} />选择</button><button disabled={busy || !sourcePath.trim()} onClick={() => { void inspect() }}>预览</button></div>{error && <p className="error-text">{error}</p>}{result && <p className="migration-result">{result}</p>}{preview && <><div className="migration-summary"><span>来源 <strong>{preview.appName}</strong></span><span>SHA-256 <code>{preview.sourceHash}</code></span><span className={preview.duplicate ? 'duplicate' : ''}>{preview.duplicate ? '已导入' : '可导入'}</span></div><div className="migration-grid"><label><span>RemoteDeck 别名</span><input value={alias} onChange={(event) => setAlias(event.target.value)} /></label><label><span>真实主机名 / IP</span><input value={hostname} onChange={(event) => setHostname(event.target.value)} /></label><label><span>端口</span><input type="number" min="1" max="65535" value={port} onChange={(event) => setPort(Number(event.target.value))} /></label><label><span>远端用户名</span><input value={username} onChange={(event) => setUsername(event.target.value)} /></label><label><span>工作目录</span><input value={workspace} onChange={(event) => setWorkspace(event.target.value)} /></label><label><span>认证方式</span><select value={authMethod} onChange={(event) => setAuthMethod(event.target.value as AuthInput['method'])}><option value="password">密码</option><option value="keyboard_interactive">交互问答</option><option value="private_key">私钥</option><option value="agent">Windows OpenSSH agent</option></select></label>{authMethod === 'private_key' && <label className="wide"><span>私钥路径</span><input value={identityFile} onChange={(event) => setIdentityFile(event.target.value)} /></label>}</div><div className="migration-preview"><h3>导入预览</h3><ul><li>SSH：{preview.sshAlias} → {alias || '待填写'}@{hostname || '待填写'}:{port}</li><li>监控：{preview.telemetryIntervalSeconds} 秒；btop watchdog {preview.btop.autoRestart ? '启用' : '关闭'}，轮换 {preview.btop.rotationMinutes} 分钟</li><li><label><input type="checkbox" checked={includeTunnel} disabled={!preview.tunnel} onChange={(event) => setIncludeTunnel(event.target.checked)} />隧道：{preview.tunnel ? `${preview.tunnel.bindAddress}:${String(preview.tunnel.sourcePort)} → ${preview.tunnel.targetHost}:${String(preview.tunnel.targetPort)}` : '无'}</label></li><li><label><input type="checkbox" checked={includeCommands} onChange={(event) => setIncludeCommands(event.target.checked)} />命令预设：{preview.commands.length} 个；旧风险规则：{preview.legacyRiskRules.length} 条</label></li></ul>{preview.tunnel?.cleanupCommand && <div className="legacy-cleanup-warning"><ShieldAlert size={15} /><div><strong>高风险旧版清理命令将以禁用状态导入</strong><code>{preview.tunnel.cleanupCommand}</code><p>只有你以后在隧道编辑器中再次阅读并授权后，恢复流程才可能执行它。</p></div></div>}{preview.warnings.map((warning) => <p className="migration-warning" key={warning}>{warning}</p>)}</div><button className="primary" disabled={busy || preview.duplicate || !alias.trim() || !hostname.trim() || !username.trim() || (authMethod === 'private_key' && !identityFile.trim())} onClick={() => { void apply() }}><Import size={14} />确认导入</button></>}</section>
}

function messageOf(value: unknown): string { return value instanceof Error ? value.message : String(value) }
