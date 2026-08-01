import { useEffect, useMemo, useRef, useState } from 'react'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import {
  ArrowDownToLine,
  ArrowUpFromLine,
  ChevronRight,
  File as FileIcon,
  Folder,
  FolderOpen,
  FolderPlus,
  Home,
  RefreshCw,
  RotateCw,
  Square,
  Trash2
} from 'lucide-react'
import { api, errorMessage } from '../api'
import { canCommitSftpListing, isSftpListingForHost } from '../sftp-listing'
import { useAppStore } from '../store'
import type { SftpEntry, SftpListing, TransferConflictPolicy, TransferJob } from '../types'

export function FilePanel(): React.JSX.Element {
  const selected = useAppStore((state) => state.hosts.find((host) => host.id === state.selectedHostId) ?? null)
  const transfers = useAppStore((state) => state.transfers)
  const setTransfers = useAppStore((state) => state.setTransfers)
  const applyTransfer = useAppStore((state) => state.applyTransfer)
  const settings = useAppStore((state) => state.settings)
  const [listing, setListing] = useState<SftpListing | null>(null)
  const [selectedPath, setSelectedPath] = useState<string | null>(null)
  const [localPath, setLocalPath] = useState('')
  const [downloadDirectory, setDownloadDirectory] = useState(settings.downloadDirectory)
  const [conflictPolicy, setConflictPolicy] = useState<TransferConflictPolicy>('ask')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [message, setMessage] = useState('')
  const loadGeneration = useRef(0)
  const selectedHostId = useRef<string | null>(selected?.id ?? null)
  selectedHostId.current = selected?.id ?? null
  const listingIsCurrent = isSftpListingForHost(listing, selected?.id ?? null)
  const currentListing = listingIsCurrent ? listing : null
  const selectedEntry = currentListing?.entries.find((entry) => entry.path === selectedPath) ?? null
  const hostTransfers = useMemo(() => selected ? transfers.filter((job) => job.hostId === selected.id) : [], [selected, transfers])

  const load = async (path?: string): Promise<void> => {
    if (!selected) return
    const hostId = selected.id
    const request = { hostId, generation: ++loadGeneration.current }
    const requestedPath = (path ?? (isSftpListingForHost(listing, hostId) ? listing.path : selected.defaultWorkspace)) || '~'
    setBusy(true)
    setError('')
    try {
      const next = await api.listSftp(hostId, requestedPath)
      if (!canCommitSftpListing(request, selectedHostId.current, loadGeneration.current, next)) return
      setListing(next)
      setSelectedPath(null)
    } catch (reason) {
      if (request.generation === loadGeneration.current && selectedHostId.current === hostId) setError(errorMessage(reason))
    } finally {
      if (request.generation === loadGeneration.current && selectedHostId.current === hostId) setBusy(false)
    }
  }

  useEffect(() => {
    loadGeneration.current += 1
    setListing(null)
    setSelectedPath(null)
    if (selected) void load(selected.defaultWorkspace || '~')
  }, [selected?.id])

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    void getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === 'drop' && event.payload.paths.length > 0) {
        void uploadMany(event.payload.paths)
      }
    }).then((cleanup) => {
      if (disposed) cleanup()
      else unlisten = cleanup
    }).catch((reason: unknown) => setError(errorMessage(reason)))
    return () => { disposed = true; unlisten?.() }
  }, [selected?.id, currentListing?.path, conflictPolicy])

  useEffect(() => {
    if (!selected) return
    let disposed = false
    void api.listTransfers(selected.id).then((jobs) => {
      if (!disposed) setTransfers([...transfers.filter((job) => job.hostId !== selected.id), ...jobs])
    }).catch(() => undefined)
    return () => { disposed = true }
  }, [selected?.id])

  const run = async (operation: () => Promise<void>): Promise<void> => {
    setBusy(true)
    setError('')
    setMessage('')
    try { await operation() } catch (reason) { setError(errorMessage(reason)) } finally { setBusy(false) }
  }

  const createDirectory = async (): Promise<void> => {
    if (!selected || !listing || listing.hostId !== selected.id) return
    const hostId = selected.id
    const remotePath = listing.path
    const name = window.prompt('新目录名称')?.trim()
    if (!name || selectedHostId.current !== hostId) return
    await run(async () => { await api.createSftpDirectory(hostId, joinRemote(remotePath, name)); if (selectedHostId.current === hostId) await load(remotePath); setMessage('目录已创建。') })
  }

  const rename = async (): Promise<void> => {
    if (!selected || !listing || listing.hostId !== selected.id || !selectedEntry) return
    const hostId = selected.id
    const remotePath = listing.path
    const entry = selectedEntry
    const name = window.prompt('新名称', selectedEntry.name)?.trim()
    if (!name || name === entry.name || selectedHostId.current !== hostId) return
    await run(async () => { await api.renameSftp(hostId, entry.path, joinRemote(remotePath, name)); if (selectedHostId.current === hostId) await load(remotePath); setMessage('已重命名。') })
  }

  const remove = async (): Promise<void> => {
    if (!selected || !listing || listing.hostId !== selected.id || !selectedEntry) return
    const hostId = selected.id
    const remotePath = listing.path
    const entry = selectedEntry
    if (!window.confirm(`删除 ${entry.path}？${entry.kind === 'directory' ? ' 目录将递归删除。' : ''}`) || selectedHostId.current !== hostId) return
    await run(async () => { await api.deleteSftp(hostId, entry.path, entry.kind === 'directory'); if (selectedHostId.current === hostId) await load(remotePath); setMessage('已删除。') })
  }

  const uploadMany = async (sources: string[]): Promise<void> => {
    const normalized = sources.map((source) => source.trim()).filter(Boolean)
    if (!selected || !listing || listing.hostId !== selected.id || normalized.length === 0) { setError('请选择当前主机上的目标目录以及本地文件或目录。'); return }
    const hostId = selected.id
    const destination = listing.path
    await run(async () => {
      if (selectedHostId.current !== hostId) throw new Error('主机已切换，上传已取消。')
      for (const source of normalized) {
        const job = await api.upload({ hostId, source, destination, conflictPolicy, recursive: true })
        applyTransfer(job)
      }
      setLocalPath('')
      setMessage(`已创建 ${String(normalized.length)} 个上传任务。`)
    })
  }

  const upload = async (source = localPath): Promise<void> => uploadMany([source])

  const chooseUpload = async (directory: boolean): Promise<void> => {
    try {
      const path = await api.pickLocalPath(directory)
      if (path) setLocalPath(path)
    } catch (reason) { setError(errorMessage(reason)) }
  }

  const chooseDownloadDirectory = async (): Promise<void> => {
    try {
      const path = await api.pickLocalPath(true)
      if (path) setDownloadDirectory(path)
    } catch (reason) { setError(errorMessage(reason)) }
  }

  const download = async (): Promise<void> => {
    if (!selected || !listing || listing.hostId !== selected.id || !selectedEntry || !downloadDirectory.trim()) { setError('请选择当前主机上的远端条目并填写本地下载目录。'); return }
    const hostId = selected.id
    const entry = selectedEntry
    const destination = downloadDirectory.trim()
    await run(async () => {
      if (selectedHostId.current !== hostId) throw new Error('主机已切换，下载已取消。')
      const job = await api.download({ hostId, source: entry.path, destination, conflictPolicy, recursive: entry.kind === 'directory' })
      applyTransfer(job)
      setMessage('下载任务已创建。')
    })
  }

  if (!selected) return <section className="empty-state"><Folder size={42} /><h1>选择一台主机</h1><p>SFTP 操作只通过已验证的 SSH 主机执行。</p></section>

  const crumbs = breadcrumbs((currentListing?.path ?? selected.defaultWorkspace) || '~')
  return (
    <section className="file-workspace">
      <header className="file-toolbar">
        <button disabled={busy} onClick={() => { void load() }}><RefreshCw size={14} />刷新</button>
        <button disabled={busy} onClick={() => { void createDirectory() }}><FolderPlus size={14} />新建目录</button>
        <button disabled={busy || !selectedEntry} onClick={() => { void rename() }}>重命名</button>
        <button className="danger" disabled={busy || !selectedEntry} onClick={() => { void remove() }}><Trash2 size={14} />删除</button>
        <span className="file-host">{selected.alias}</span>
      </header>
      <nav className="breadcrumbs" aria-label="远端路径">
        <button disabled={busy} onClick={() => { void load('~') }}><Home size={13} /></button>
        {crumbs.map((crumb) => <span key={crumb.path}><ChevronRight size={12} /><button disabled={busy} onClick={() => { void load(crumb.path) }}>{crumb.label}</button></span>)}
      </nav>
      <div className="file-transfer-toolbar">
        <label><span>本地上传路径</span><input value={localPath} onChange={(event) => setLocalPath(event.target.value)} placeholder="C:\\data\\result.csv" /></label>
        <button disabled={busy} onClick={() => { void chooseUpload(false) }}><FolderOpen size={14} />选文件</button>
        <button disabled={busy} onClick={() => { void chooseUpload(true) }}><FolderOpen size={14} />选目录</button>
        <button className="primary" disabled={busy || !localPath.trim()} onClick={() => { void upload() }}><ArrowUpFromLine size={14} />上传</button>
        <label><span>下载目录</span><input value={downloadDirectory} onChange={(event) => setDownloadDirectory(event.target.value)} placeholder="C:\\Users\\name\\Downloads" /></label>
        <button disabled={busy} onClick={() => { void chooseDownloadDirectory() }}><FolderOpen size={14} />选择目录</button>
        <button disabled={busy || !selectedEntry || !downloadDirectory.trim()} onClick={() => { void download() }}><ArrowDownToLine size={14} />下载所选</button>
        <label className="compact-select"><span>冲突</span><select value={conflictPolicy} onChange={(event) => setConflictPolicy(event.target.value as TransferConflictPolicy)}><option value="ask">询问</option><option value="overwrite">覆盖</option><option value="skip">跳过</option><option value="rename">自动改名</option></select></label>
      </div>
      {error && <div className="file-error" role="alert">{error}<button onClick={() => setError('')}>关闭</button></div>}
      {message && <div className="file-message" role="status">{message}</div>}
      <div className="file-list" role="grid" aria-label="远端文件">
        <div className="file-row header" role="row"><span /><span>名称</span><span>大小</span><span>修改时间</span><span>权限</span></div>
        {currentListing?.parentPath && <button disabled={busy} className="file-row" onDoubleClick={() => { if (!busy) void load(currentListing.parentPath ?? '~') }} onClick={() => setSelectedPath(null)}><Folder size={15} /><span>..</span><span>—</span><span>—</span><span>—</span></button>}
        {currentListing?.entries.map((entry) => <button disabled={busy} key={entry.path} className={entry.path === selectedPath ? 'file-row selected' : 'file-row'} onClick={() => setSelectedPath(entry.path)} onDoubleClick={() => { if (!busy && entry.kind === 'directory') void load(entry.path) }}><EntryIcon entry={entry} /><span title={entry.path}>{entry.name}</span><span>{entry.kind === 'directory' ? '—' : formatBytes(entry.size)}</span><span>{entry.modifiedAt ? new Date(entry.modifiedAt).toLocaleString() : '—'}</span><span>{entry.permissions ?? '—'}</span></button>)}
        {!busy && currentListing && currentListing.entries.length === 0 && <p className="file-list-empty">此目录为空。</p>}
      </div>
      <TransferDrawer jobs={hostTransfers} onApply={applyTransfer} onError={setError} />
    </section>
  )
}

function TransferDrawer({ jobs, onApply, onError }: { jobs: TransferJob[]; onApply: (job: TransferJob) => void; onError: (message: string) => void }): React.JSX.Element {
  return <aside className="transfer-drawer"><h2>传输任务 <span>{String(jobs.filter((job) => job.state === 'running' || job.state === 'queued' || job.state === 'cancelling').length)} 个活动</span></h2>{jobs.length === 0 ? <p>暂无传输任务；也可将本地文件拖入窗口。</p> : jobs.toReversed().map((job) => { const progress = job.totalBytes ? Math.min(100, job.bytesTransferred * 100 / job.totalBytes) : 0; return <article className="transfer-job" key={job.id}><span className={`transfer-kind state-${job.state}`}>{job.direction === 'upload' ? <ArrowUpFromLine size={15} /> : <ArrowDownToLine size={15} />}</span><div className="transfer-detail"><strong>{tail(job.source)} → {tail(job.destination)}</strong><small>{formatBytes(job.bytesTransferred)} / {job.totalBytes === null ? '未知' : formatBytes(job.totalBytes)} · {transferStateLabel(job.state)}{job.error ? ` · ${job.error}` : ''}</small><div className="transfer-progress"><i style={{ width: `${String(progress)}%` }} /></div></div><div>{job.state === 'running' || job.state === 'queued' ? <button onClick={() => { void api.cancelTransfer(job.id).then(onApply).catch((reason: unknown) => onError(errorMessage(reason))) }}><Square size={12} />取消</button> : job.state === 'cancelling' ? <button disabled><Square size={12} />取消中</button> : job.state === 'failed' || job.state === 'cancelled' ? <button onClick={() => { void api.retryTransfer(job.id).then(onApply).catch((reason: unknown) => onError(errorMessage(reason))) }}><RotateCw size={12} />重试</button> : <button onClick={() => { void api.showTransferInFolder(job.id).catch((reason: unknown) => onError(errorMessage(reason))) }}>定位</button>}</div></article> })}</aside>
}

function EntryIcon({ entry }: { entry: SftpEntry }): React.JSX.Element { return entry.kind === 'directory' ? <Folder size={15} /> : <FileIcon size={15} /> }

function joinRemote(base: string, name: string): string { return base === '/' ? `/${name}` : `${base.replace(/\/$/u, '')}/${name}` }
function tail(path: string): string { return path.replace(/\\/gu, '/').split('/').filter(Boolean).at(-1) ?? path }
function formatBytes(value: number): string { if (value < 1024) return `${String(value)} B`; if (value < 1024 ** 2) return `${(value / 1024).toFixed(1)} KiB`; if (value < 1024 ** 3) return `${(value / 1024 ** 2).toFixed(1)} MiB`; return `${(value / 1024 ** 3).toFixed(1)} GiB` }
function transferStateLabel(state: TransferJob['state']): string { return { queued: '排队中', running: '传输中', cancelling: '取消中', completed: '已完成', failed: '失败', cancelled: '已取消' }[state] }

export function breadcrumbs(path: string): Array<{ label: string; path: string }> {
  if (path === '~') return [{ label: '~', path: '~' }]
  const absolute = path.startsWith('/')
  const parts = path.split('/').filter(Boolean)
  return parts.map((label, index) => ({ label, path: `${absolute ? '/' : ''}${parts.slice(0, index + 1).join('/')}` }))
}
