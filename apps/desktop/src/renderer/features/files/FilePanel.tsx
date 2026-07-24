import { useEffect, useState } from 'react'
import { ChevronRight, Download, Eye, EyeOff, File, FilePlus, Folder, FolderOpen, FolderPlus, RefreshCw, RotateCcw, Square, Trash2, Upload, X } from 'lucide-react'
import type { TransferJob } from '../../../protocol/domain'
import type { ConflictPolicy, SftpEntry, SftpListResult } from '../../../protocol/sftp'
import { useHostStore } from '../../host-store'
import { useAppStore } from '../../store'

export function FilePanel(): React.JSX.Element {
  const hosts = useHostStore((state) => state.items)
  const selectedId = useHostStore((state) => state.selectedId)
  const host = hosts.find((item) => item.host.id === selectedId)
  const [listing, setListing] = useState<SftpListResult | null>(null)
  const [tree, setTree] = useState<Record<string, SftpEntry[]>>({})
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [showHidden, setShowHidden] = useState(false)
  const [policy, setPolicy] = useState<ConflictPolicy>('rename')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [message, setMessage] = useState('')
  const [createType, setCreateType] = useState<'file' | 'directory' | null>(null)
  const [nameDraft, setNameDraft] = useState('')
  const [renaming, setRenaming] = useState<SftpEntry | null>(null)
  const [jobs, setJobs] = useState<TransferJob[]>([])
  const currentPath = listing?.path ?? host?.workspace?.remotePath ?? '~'

  async function load(path = currentPath, includeHidden = showHidden): Promise<void> {
    if (!host || host.state !== 'online') return
    setBusy(true); setError('')
    try {
      const result = await window.remoteDeck.sftp.list({ hostId: host.host.id, path, showHidden: includeHidden })
      setListing(result)
      setTree((current) => ({ ...current, [result.path]: result.entries }))
      setExpanded((current) => new Set(current).add(result.path))
      setSelected(new Set())
    } catch (reason) { setError(messageOf(reason)) } finally { setBusy(false) }
  }

  useEffect(() => {
    setListing(null); setTree({}); setExpanded(new Set()); setSelected(new Set()); setError('')
    if (host?.state === 'online') void load(host.workspace?.remotePath ?? '~', showHidden)
  }, [host?.host.id, host?.state])

  useEffect(() => {
    void window.remoteDeck.sftp.transfers.list().then(setJobs).catch((reason: unknown) => setError(messageOf(reason)))
    return window.remoteDeck.sftp.transfers.onEvent(({ job }) => setJobs((current) => [job, ...current.filter((item) => item.id !== job.id)].sort((left, right) => right.createdAt.localeCompare(left.createdAt))))
  }, [])

  async function refresh(): Promise<void> { await load(currentPath, showHidden) }

  async function toggleTree(path: string): Promise<void> {
    if (!host) return
    if (expanded.has(path)) { setExpanded((current) => { const next = new Set(current); next.delete(path); return next }); return }
    if (!tree[path]) {
      const result = await window.remoteDeck.sftp.list({ hostId: host.host.id, path, showHidden })
      setTree((current) => ({ ...current, [result.path]: result.entries }))
      path = result.path
    }
    setExpanded((current) => new Set(current).add(path))
  }

  async function createEntry(): Promise<void> {
    if (!host || !createType || !nameDraft.trim()) return
    await perform(async () => {
      await window.remoteDeck.sftp.create({ hostId: host.host.id, parentPath: currentPath, name: nameDraft.trim(), type: createType })
      setCreateType(null); setNameDraft(''); setMessage(createType === 'directory' ? '文件夹已创建。' : '空文件已创建。'); await refresh()
    })
  }

  async function renameEntry(): Promise<void> {
    if (!host || !renaming || !nameDraft.trim()) return
    await perform(async () => { await window.remoteDeck.sftp.rename({ hostId: host.host.id, path: renaming.path, newName: nameDraft.trim() }); setRenaming(null); setNameDraft(''); setMessage('已重命名。'); await refresh() })
  }

  async function deleteSelected(): Promise<void> {
    if (!host || selected.size === 0 || !window.confirm(`递归删除选中的 ${String(selected.size)} 项？此操作不可撤销。`)) return
    await perform(async () => { await window.remoteDeck.sftp.delete({ hostId: host.host.id, paths: [...selected] }); setMessage('远程项目已删除。'); await refresh() })
  }

  async function pickUpload(kind: 'files' | 'directory'): Promise<void> {
    if (!host) return
    await perform(async () => { const result = await window.remoteDeck.sftp.pickUpload(kind); if (!result.canceled && result.paths.length > 0) { await window.remoteDeck.sftp.transfers.upload({ hostId: host.host.id, sources: result.paths, remoteDirectory: currentPath, conflictPolicy: policy }); setMessage(`已加入 ${String(result.paths.length)} 个上传任务。`) } })
  }

  async function uploadDropped(files: FileList): Promise<void> {
    if (!host) return
    const paths = Array.from(files).map((file) => window.remoteDeck.sftp.droppedPath(file)).filter(Boolean)
    if (paths.length === 0) { setError('平台未提供拖入项目的本地路径，请使用“上传文件/文件夹”。'); return }
    await perform(async () => { await window.remoteDeck.sftp.transfers.upload({ hostId: host.host.id, sources: paths, remoteDirectory: currentPath, conflictPolicy: policy }); setMessage(`已加入 ${String(paths.length)} 个拖入上传任务。`) })
  }

  async function downloadSelected(): Promise<void> {
    if (!host || selected.size === 0) return
    await perform(async () => { const destination = await window.remoteDeck.sftp.pickDownloadDirectory(); if (!destination.canceled && destination.paths[0]) { await window.remoteDeck.sftp.transfers.download({ hostId: host.host.id, sources: [...selected], localDirectory: destination.paths[0], conflictPolicy: policy }); setMessage(`已加入 ${String(selected.size)} 个下载任务。`) } })
  }

  async function perform(action: () => Promise<void>): Promise<void> {
    setBusy(true); setError(''); setMessage('')
    try { await action() } catch (reason) { setError(messageOf(reason)) } finally { setBusy(false) }
  }

  if (!host) return <FileState title="未选择主机" detail="请先在主机列表选择一台 Linux 主机。" />
  if (host.state !== 'online') return <FileState title={`${host.host.alias} 未连接`} detail="SFTP 需要已验证并在线的 SSH 连接。" action={() => useAppStore.getState().setActivity('hosts')} />
  const entries = listing?.entries ?? []
  const selectedEntries = entries.filter((entry) => selected.has(entry.path))
  return (
    <section className="file-workspace" onDragOver={(event) => { event.preventDefault(); event.dataTransfer.dropEffect = 'copy' }} onDrop={(event) => { event.preventDefault(); void uploadDropped(event.dataTransfer.files) }}>
      <header className="file-toolbar">
        <Breadcrumb path={listing?.path ?? host.workspace?.remotePath ?? '~'} onNavigate={(path) => { void load(path) }} />
        <button title="刷新" aria-label="刷新远程目录" disabled={busy} onClick={() => { void refresh() }}><RefreshCw size={15} /></button>
        <button title={showHidden ? '隐藏点文件' : '显示点文件'} aria-label={showHidden ? '隐藏点文件' : '显示点文件'} onClick={() => { const value = !showHidden; setShowHidden(value); void load(currentPath, value) }}>{showHidden ? <EyeOff size={15} /> : <Eye size={15} />}</button>
        <button onClick={() => { setCreateType('directory'); setRenaming(null); setNameDraft('') }}><FolderPlus size={15} />新建文件夹</button>
        <button onClick={() => { setCreateType('file'); setRenaming(null); setNameDraft('') }}><FilePlus size={15} />新建文件</button>
        <button disabled={selectedEntries.length !== 1} onClick={() => { const entry = selectedEntries[0]; if (entry) { setRenaming(entry); setCreateType(null); setNameDraft(entry.name) } }}>重命名</button>
        <button className="danger" disabled={selected.size === 0 || busy} onClick={() => { void deleteSelected() }}><Trash2 size={14} />删除</button>
      </header>
      <div className="file-transfer-toolbar"><span>同名策略</span><select value={policy} onChange={(event) => setPolicy(event.target.value as ConflictPolicy)}><option value="rename">自动重命名</option><option value="overwrite">覆盖</option><option value="skip">跳过</option></select><button disabled={busy} onClick={() => { void pickUpload('files') }}><Upload size={14} />上传文件</button><button disabled={busy} onClick={() => { void pickUpload('directory') }}><Upload size={14} />上传文件夹</button><button disabled={selected.size === 0 || busy} onClick={() => { void downloadSelected() }}><Download size={14} />下载到…</button><span className="drop-hint">也可拖入本地文件或文件夹</span></div>
      {(createType || renaming) && <div className="file-inline-editor"><span>{renaming ? `重命名 ${renaming.name}` : createType === 'directory' ? '新文件夹' : '新文件'}</span><input autoFocus aria-label="远程名称" value={nameDraft} onChange={(event) => setNameDraft(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') void (renaming ? renameEntry() : createEntry()); if (event.key === 'Escape') { setCreateType(null); setRenaming(null) } }} /><button className="primary" disabled={!nameDraft.trim() || busy} onClick={() => { void (renaming ? renameEntry() : createEntry()) }}>保存</button><button onClick={() => { setCreateType(null); setRenaming(null) }}><X size={14} />取消</button></div>}
      {error && <div className="file-error" role="alert">{error}</div>}{message && <div className="file-message">{message}</div>}
      <div className="file-main">
        <aside className="remote-tree"><h2>远程目录</h2>{listing && <TreeNode path={listing.path} label={posixName(listing.path)} depth={0} tree={tree} expanded={expanded} current={listing.path} onToggle={(path) => { void toggleTree(path) }} onNavigate={(path) => { void load(path) }} />}</aside>
        <div className="file-list" role="table" aria-label="远程文件列表"><div className="file-row header" role="row"><span /><span>名称</span><span>大小</span><span>修改时间</span><span>模式</span></div>{entries.length === 0 ? <div className="file-list-empty">此目录为空{showHidden ? '。' : '，点文件当前隐藏。'}</div> : entries.map((entry) => <div key={entry.path} className={selected.has(entry.path) ? 'file-row selected' : 'file-row'} role="row" onDoubleClick={() => { if (entry.type === 'directory') void load(entry.path) }}><input aria-label={`选择 ${entry.name}`} type="checkbox" checked={selected.has(entry.path)} onChange={(event) => setSelected((current) => { const next = new Set(current); if (event.target.checked) next.add(entry.path); else next.delete(entry.path); return next })} /> <button className="file-name" onClick={() => { if (entry.type === 'directory') void load(entry.path) }}><EntryIcon entry={entry} /><span>{entry.name}</span></button><span>{entry.type === 'directory' ? '—' : formatBytes(entry.size)}</span><span>{new Date(entry.modifiedAt).toLocaleString('zh-CN')}</span><code>{(entry.mode & 0o777).toString(8).padStart(3, '0')}</code></div>)}</div>
      </div>
      <TransferDrawer jobs={jobs} onCancel={(id) => { void window.remoteDeck.sftp.transfers.cancel(id) }} onRetry={(id) => { void window.remoteDeck.sftp.transfers.retry(id) }} onShow={(id) => { void window.remoteDeck.sftp.transfers.showInFolder(id) }} />
    </section>
  )
}

function TreeNode({ path, label, depth, tree, expanded, current, onToggle, onNavigate }: { path: string; label: string; depth: number; tree: Record<string, SftpEntry[]>; expanded: Set<string>; current: string; onToggle: (path: string) => void; onNavigate: (path: string) => void }): React.JSX.Element {
  const directories = (tree[path] ?? []).filter((entry) => entry.type === 'directory')
  return <div><div className={path === current ? 'tree-node current' : 'tree-node'} style={{ paddingLeft: 6 + depth * 14 }}><button aria-label={`${expanded.has(path) ? '折叠' : '展开'} ${label}`} onClick={() => onToggle(path)}><ChevronRight size={13} className={expanded.has(path) ? 'expanded' : ''} /></button><button onClick={() => onNavigate(path)}><FolderOpen size={13} />{label}</button></div>{expanded.has(path) && directories.map((entry) => <TreeNode key={entry.path} path={entry.path} label={entry.name} depth={depth + 1} tree={tree} expanded={expanded} current={current} onToggle={onToggle} onNavigate={onNavigate} />)}</div>
}

function TransferDrawer({ jobs, onCancel, onRetry, onShow }: { jobs: TransferJob[]; onCancel: (id: string) => void; onRetry: (id: string) => void; onShow: (id: string) => void }): React.JSX.Element {
  const visible = jobs.slice(0, 20)
  return <section className="transfer-drawer"><h2>传输任务 <span>{String(jobs.filter((job) => job.state === 'running' || job.state === 'queued').length)} 活动</span></h2>{visible.length === 0 ? <p>暂无传输任务。</p> : <div className="transfer-jobs">{visible.map((job) => { const percent = job.totalBytes ? Math.min(100, job.bytesTransferred / job.totalBytes * 100) : 0; return <div className="transfer-job" key={job.id}><span className="transfer-kind">{job.direction === 'upload' ? <Upload size={14} /> : <Download size={14} />}</span><span className="transfer-detail"><strong>{job.source}</strong><small>{job.destination}</small><span className="transfer-progress"><i style={{ width: `${String(percent)}%` }} /></span><small>{job.state} · {formatBytes(job.bytesTransferred)} / {job.totalBytes === null ? '计算中' : formatBytes(job.totalBytes)} · {formatBytes(job.speedBytesPerSecond)}/s{job.error ? ` · ${job.error}` : ''}</small></span>{(job.state === 'running' || job.state === 'queued') && <button aria-label={`取消任务 ${job.id}`} onClick={() => onCancel(job.id)}><Square size={13} />取消</button>}{(job.state === 'failed' || job.state === 'cancelled') && <button onClick={() => onRetry(job.id)}><RotateCcw size={13} />重试</button>}{job.state === 'completed' && job.direction === 'download' && <button onClick={() => onShow(job.id)}><FolderOpen size={13} />打开位置</button>}</div> })}</div>}</section>
}

function Breadcrumb({ path, onNavigate }: { path: string; onNavigate: (path: string) => void }): React.JSX.Element { const segments = path.split('/').filter(Boolean); return <nav className="breadcrumbs" aria-label="远程路径"><button onClick={() => onNavigate('/')}>/</button>{segments.map((segment, index) => { const target = `/${segments.slice(0, index + 1).join('/')}`; return <span key={target}><ChevronRight size={12} /><button onClick={() => onNavigate(target)}>{segment}</button></span> })}</nav> }
function EntryIcon({ entry }: { entry: SftpEntry }): React.JSX.Element { return entry.type === 'directory' ? <Folder size={15} /> : <File size={15} /> }
function FileState({ title, detail, action }: { title: string; detail: string; action?: () => void }): React.JSX.Element { return <section className="file-state"><FolderOpen size={40} /><h1>{title}</h1><p>{detail}</p>{action && <button className="primary" onClick={action}>返回主机</button>}</section> }
function posixName(path: string): string { return path === '/' ? '/' : path.split('/').filter(Boolean).at(-1) ?? path }
function formatBytes(bytes: number): string { if (bytes < 1024) return `${String(Math.round(bytes))} B`; const units = ['KiB', 'MiB', 'GiB', 'TiB']; let value = bytes / 1024; let index = 0; while (value >= 1024 && index < units.length - 1) { value /= 1024; index += 1 } return `${value.toFixed(value >= 10 ? 1 : 2)} ${units[index] ?? 'KiB'}` }
function messageOf(value: unknown): string { return value instanceof Error ? value.message : String(value) }
