import { useState } from 'react'
import { FileSearch, FolderOpen, Import, ShieldAlert } from 'lucide-react'
import { api, errorMessage } from '../api'
import { useAppStore } from '../store'
import type { LegacyApplyRequest, LegacyPreview } from '../types'

export function MigrationPanel(): React.JSX.Element {
  const bootstrap = useAppStore((state) => state.bootstrap)
  const [sourcePath, setSourcePath] = useState('')
  const [preview, setPreview] = useState<LegacyPreview | null>(null)
  const [selection, setSelection] = useState<Omit<LegacyApplyRequest, 'sourcePath' | 'sourceHash'>>({ includeHosts: true, includeTunnels: true, includeCommands: true, includeSettings: true })
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [message, setMessage] = useState('')

  const inspect = async (): Promise<void> => {
    if (!sourcePath.trim()) { setError('请输入旧配置文件路径。'); return }
    setBusy(true); setError(''); setMessage('')
    try { setPreview(await api.legacyPreview(sourcePath.trim())) } catch (reason) { setError(errorMessage(reason)) } finally { setBusy(false) }
  }

  const choose = async (): Promise<void> => {
    setError('')
    try {
      const selected = await api.pickLocalPath(false)
      if (selected) { setSourcePath(selected); setPreview(null); setMessage('') }
    } catch (reason) { setError(errorMessage(reason)) }
  }

  const apply = async (): Promise<void> => {
    if (!preview) return
    setBusy(true); setError(''); setMessage('')
    try {
      const result = await api.legacyApply({ sourcePath: preview.sourcePath, sourceHash: preview.sourceHash, ...selection })
      setMessage(result.message || `已导入 ${String(result.importedHosts)} 台主机、${String(result.importedTunnels)} 条隧道和 ${String(result.importedCommands)} 个命令。`)
      await bootstrap()
      setPreview(await api.legacyPreview(preview.sourcePath))
    } catch (reason) { setError(errorMessage(reason)) } finally { setBusy(false) }
  }

  return (
    <section className="card migration-panel">
      <div className="card-title"><Import size={17} /><div><h2>旧版配置迁移</h2><p>兼容 LabPulse SSH 与 RemoteDeck v1；先预览、后显式导入，来源文件不会被修改。</p></div></div>
      <div className="legacy-source">
        <input aria-label="旧配置路径" value={sourcePath} onChange={(event) => { setSourcePath(event.target.value); setPreview(null) }} placeholder="旧版 config.json 或 RemoteDeck 数据文件" />
        <button disabled={busy} onClick={() => { void choose() }}><FolderOpen size={14} />选择</button>
        <button disabled={busy || !sourcePath.trim()} onClick={() => { void inspect() }}><FileSearch size={14} />预览</button>
      </div>
      {error && <p className="error-text" role="alert">{error}</p>}
      {message && <p className="migration-result" role="status">{message}</p>}
      {preview && <>
        <div className="migration-summary"><span>来源 <strong>{preview.appName}</strong></span><span>SHA-256 <code>{preview.sourceHash}</code></span><span className={preview.duplicate ? 'duplicate' : ''}>{preview.duplicate ? '已导入' : '可导入'}</span></div>
        <div className="migration-preview">
          <h3>导入内容</h3>
          <label><input type="checkbox" checked={selection.includeHosts} onChange={(event) => setSelection({ ...selection, includeHosts: event.target.checked })} />主机 {String(preview.hosts.length)} 台</label>
          <label><input type="checkbox" checked={selection.includeTunnels} disabled={preview.tunnelCount === 0} onChange={(event) => setSelection({ ...selection, includeTunnels: event.target.checked })} />隧道 {String(preview.tunnelCount)} 条</label>
          <label><input type="checkbox" checked={selection.includeCommands} disabled={preview.commandCount === 0} onChange={(event) => setSelection({ ...selection, includeCommands: event.target.checked })} />命令预设 {String(preview.commandCount)} 个</label>
          <label><input type="checkbox" checked={selection.includeSettings} disabled={!preview.settingsIncluded} onChange={(event) => setSelection({ ...selection, includeSettings: event.target.checked })} />兼容设置</label>
          {preview.hosts.map((host) => <p key={`${host.alias}-${host.hostname}`}>{host.alias} · {host.username}@{host.hostname}:{String(host.port)}</p>)}
          {preview.warnings.map((warning) => <p className="migration-warning" key={warning}><ShieldAlert size={13} />{warning}</p>)}
        </div>
        <button className="primary" disabled={busy || preview.duplicate || !Object.values(selection).some(Boolean)} onClick={() => { void apply() }}><Import size={14} />确认导入</button>
      </>}
    </section>
  )
}
