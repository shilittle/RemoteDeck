import { useEffect, useMemo, useState } from 'react'
import { Activity, ChevronUp, CircleAlert, RefreshCw, Square, X } from 'lucide-react'
import type { CommandJob } from '../../../protocol/command'
import type { TransferJob } from '../../../protocol/domain'
import type { TunnelSnapshot } from '../../../protocol/tunnel'
import { useHostStore } from '../../host-store'
import { useAppStore } from '../../store'

export function TaskCenter(): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [transfers, setTransfers] = useState<TransferJob[]>([])
  const [commands, setCommands] = useState<CommandJob[]>([])
  const [tunnels, setTunnels] = useState<TunnelSnapshot[]>([])
  const hosts = useHostStore((state) => state.items)
  const selectedId = useHostStore((state) => state.selectedId)
  const setActivity = useAppStore((state) => state.setActivity)

  async function load(): Promise<void> {
    const [nextTransfers, nextCommands, nextTunnels] = await Promise.all([window.remoteDeck.sftp.transfers.list(), window.remoteDeck.commands.jobs(), window.remoteDeck.tunnels.list()])
    setTransfers(nextTransfers); setCommands(nextCommands); setTunnels(nextTunnels)
  }

  useEffect(() => {
    void load()
    const offTransfer = window.remoteDeck.sftp.transfers.onEvent((event) => setTransfers((current) => upsert(current, event.job)))
    const offCommand = window.remoteDeck.commands.onEvent((event) => { if (event.type === 'job') setCommands((current) => upsert(current, event.job)) })
    const offTunnel = window.remoteDeck.tunnels.onEvent((event) => setTunnels((current) => upsertTunnel(current, event.snapshot)))
    const toggle = (): void => setOpen((current) => !current)
    window.addEventListener('remotedeck:toggle-task-center', toggle)
    return () => { offTransfer(); offCommand(); offTunnel(); window.removeEventListener('remotedeck:toggle-task-center', toggle) }
  }, [])

  const selected = hosts.find((item) => item.host.id === selectedId)
  const activeCount = transfers.filter((item) => item.state === 'queued' || item.state === 'running').length + commands.filter((item) => item.state === 'running').length + tunnels.filter((item) => ['starting', 'online', 'recovering', 'degraded'].includes(item.state)).length
  const problemCount = transfers.filter((item) => item.state === 'failed').length + commands.filter((item) => item.state === 'failed').length + tunnels.filter((item) => item.state === 'failed').length + hosts.filter((item) => item.state === 'failed').length
  const recentTransfers = useMemo(() => transfers.toSorted((a, b) => b.updatedAt.localeCompare(a.updatedAt)).slice(0, 20), [transfers])
  const recentCommands = useMemo(() => commands.toSorted((a, b) => b.startedAt.localeCompare(a.startedAt)).slice(0, 20), [commands])
  return <>
    {open && <section className="task-center" aria-label="任务中心"><div className="task-center-head"><div><Activity size={15} /><strong>任务中心</strong><span>{activeCount} 活动 · {problemCount} 需处理</span></div><div><button onClick={() => { void load() }}><RefreshCw size={13} />刷新</button><button aria-label="关闭任务中心" onClick={() => setOpen(false)}><X size={14} /></button></div></div><div className="task-columns"><TaskGroup title="传输" empty="无传输任务">{recentTransfers.map((job) => <div className="task-row" key={job.id}><State state={job.state} /><span><strong>{job.direction === 'upload' ? '上传' : '下载'} {tail(job.destination)}</strong><small>{formatBytes(job.bytesTransferred)} / {job.totalBytes === null ? '未知' : formatBytes(job.totalBytes)} · {job.error ?? job.state}</small></span><div>{job.state === 'running' || job.state === 'queued' ? <button onClick={() => { void window.remoteDeck.sftp.transfers.cancel(job.id) }}><Square size={11} />取消</button> : job.state === 'failed' || job.state === 'cancelled' ? <button onClick={() => { void window.remoteDeck.sftp.transfers.retry(job.id) }}>重试</button> : <button onClick={() => { void window.remoteDeck.sftp.transfers.showInFolder(job.id) }}>定位</button>}</div></div>)}</TaskGroup><TaskGroup title="命令" empty="无命令任务">{recentCommands.map((job) => <div className="task-row" key={job.id}><State state={job.state} /><span><strong>{job.name}</strong><small>{job.error ?? `${job.risk} · ${job.state}`}</small></span><div>{job.state === 'running' ? <button onClick={() => { void window.remoteDeck.commands.cancel(job.id) }}><Square size={11} />取消</button> : <button onClick={() => { setActivity('commands'); setOpen(false) }}>查看</button>}</div></div>)}</TaskGroup><TaskGroup title="隧道与连接" empty="无隧道"><>{hosts.filter((item) => item.state === 'failed' || item.state === 'offline').map((host) => <div className="task-row" key={host.host.id}><CircleAlert className="task-failed" size={13} /><span><strong>{host.host.alias} SSH</strong><small>{host.lastError ?? host.state}</small></span><button onClick={() => { useHostStore.getState().select(host.host.id); setActivity('hosts'); setOpen(false) }}>恢复</button></div>)}{tunnels.slice(0, 20).map((item) => <div className="task-row" key={item.profile.id}><State state={item.state} /><span><strong>{item.profile.name}</strong><small>{item.lastError ?? `${item.profile.bindAddress}:${String(item.profile.sourcePort)} · ${item.state}`}</small></span><button onClick={() => { useHostStore.getState().select(item.profile.hostId); setActivity('tunnels'); setOpen(false) }}>{item.state === 'failed' ? '恢复' : '查看'}</button></div>)}</></TaskGroup></div></section>}
    <footer className="statusbar"><button className="task-toggle" onClick={() => setOpen((current) => !current)}><ChevronUp className={open ? 'open' : ''} size={13} />任务 {activeCount}{problemCount > 0 ? ` · ${String(problemCount)} 错误` : ''}</button><span>SSH {selected?.state ?? '未连接'}</span><span>Ctrl+J 任务中心</span></footer>
  </>
}

function TaskGroup({ title, empty, children }: { title: string; empty: string; children: React.ReactNode }): React.JSX.Element { const array = Array.isArray(children) ? children : [children]; const hasContent = array.some(Boolean); return <section><h2>{title}</h2>{hasContent ? children : <p>{empty}</p>}</section> }
function State({ state }: { state: string }): React.JSX.Element { return <i className={`task-state task-${state}`} title={state} /> }
function upsert<T extends { id: string }>(items: T[], item: T): T[] { return items.some((current) => current.id === item.id) ? items.map((current) => current.id === item.id ? item : current) : [item, ...items] }
function upsertTunnel(items: TunnelSnapshot[], item: TunnelSnapshot): TunnelSnapshot[] { return items.some((current) => current.profile.id === item.profile.id) ? items.map((current) => current.profile.id === item.profile.id ? item : current) : [item, ...items] }
function tail(path: string): string { return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path }
function formatBytes(value: number): string { if (value < 1024) return `${String(value)} B`; if (value < 1024 ** 2) return `${(value / 1024).toFixed(1)} KiB`; return `${(value / 1024 ** 2).toFixed(1)} MiB` }
