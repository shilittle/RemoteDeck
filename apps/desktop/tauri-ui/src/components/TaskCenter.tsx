import { Activity, ChevronUp, CircleAlert, RefreshCw, Square, X } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { api, errorMessage } from '../api'
import { useAppStore } from '../store'

export function TaskCenter(): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [error, setError] = useState('')
  const transfers = useAppStore((state) => state.transfers)
  const commandJobs = useAppStore((state) => state.commandJobs)
  const tunnels = useAppStore((state) => state.tunnels)
  const tunnelStates = useAppStore((state) => state.tunnelStates)
  const telemetry = useAppStore((state) => state.telemetryStatuses)
  const selected = useAppStore((state) => state.hosts.find((host) => host.id === state.selectedHostId) ?? null)
  const setActivity = useAppStore((state) => state.setActivity)
  const applyTransfer = useAppStore((state) => state.applyTransfer)
  const applyCommandJob = useAppStore((state) => state.applyCommandJob)
  const activeCount = transfers.filter((job) => job.state === 'queued' || job.state === 'running' || job.state === 'cancelling').length + commandJobs.filter((job) => job.state === 'queued' || job.state === 'running' || job.state === 'cancelling').length + Object.values(tunnelStates).filter((item) => item?.state === 'starting' || item?.state === 'waiting').length
  const problemCount = transfers.filter((job) => job.state === 'failed').length + commandJobs.filter((job) => job.state === 'failed').length + Object.values(tunnelStates).filter((item) => item?.state === 'failed').length + Object.values(telemetry).filter((item) => item?.state === 'failed').length
  const recentTransfers = useMemo(() => transfers.toReversed().slice(0, 12), [transfers])
  const recentCommands = useMemo(() => commandJobs.toReversed().slice(0, 12), [commandJobs])

  useEffect(() => {
    const toggle = (): void => setOpen((value) => !value)
    window.addEventListener('remotedeck:toggle-tasks', toggle)
    return () => window.removeEventListener('remotedeck:toggle-tasks', toggle)
  }, [])

  return <>
    <footer className="statusbar">
      <button className="task-toggle" onClick={() => setOpen((value) => !value)}>
        <ChevronUp className={open ? 'open' : ''} size={13} />任务 {String(activeCount)}
        {problemCount > 0 ? ` · ${String(problemCount)} 错误` : ''}
      </button>
      <span>SSH {selected ? selected.alias : '未选择'}</span>
      <span>Ctrl+J 任务中心</span>
    </footer>
    {open && <section className="task-center" aria-label="任务中心">
      <header className="task-center-head">
        <div><Activity size={15} /><strong>任务中心</strong><span>{String(activeCount)} 活动 · {String(problemCount)} 需处理</span></div>
        <button aria-label="关闭任务中心" onClick={() => setOpen(false)}><X size={14} /></button>
      </header>
      {error && <p className="task-error">{error}</p>}
      <div className="task-columns">
        <section>
          <h2>传输</h2>
          {recentTransfers.length === 0 ? <p>无传输任务</p> : recentTransfers.map((job) => <article className="task-row" key={job.id}>
            <State state={job.state} />
            <span><strong>{job.direction === 'upload' ? '上传' : '下载'} {tail(job.destination)}</strong><small>{formatBytes(job.bytesTransferred)} / {job.totalBytes === null ? '未知' : formatBytes(job.totalBytes)} · {job.error ?? job.state}</small></span>
            <div>{job.state === 'running' || job.state === 'queued'
              ? <button onClick={() => { void api.cancelTransfer(job.id).then(applyTransfer).catch((reason: unknown) => setError(errorMessage(reason))) }}><Square size={11} />取消</button>
              : job.state === 'cancelling'
                ? <button disabled><Square size={11} />取消中</button>
                : job.state === 'failed' || job.state === 'cancelled'
                  ? <button onClick={() => { void api.retryTransfer(job.id).then(applyTransfer).catch((reason: unknown) => setError(errorMessage(reason))) }}><RefreshCw size={11} />重试</button>
                  : <button onClick={() => { void api.showTransferInFolder(job.id).catch((reason: unknown) => setError(errorMessage(reason))) }}>定位</button>}
            </div>
          </article>)}
        </section>
        <section>
          <h2>命令</h2>
          {recentCommands.length === 0 ? <p>无命令任务</p> : recentCommands.map((job) => <article className="task-row" key={job.id}>
            <State state={job.state} />
            <span><strong>{job.name}</strong><small>{job.error ?? `${job.risk} · ${job.state}`}</small></span>
            <div>{job.state === 'running' || job.state === 'queued'
              ? <button onClick={() => { void api.cancelCommand(job.id).then(applyCommandJob).catch((reason: unknown) => setError(errorMessage(reason))) }}><Square size={11} />取消</button>
              : job.state === 'cancelling'
                ? <button disabled><Square size={11} />取消中</button>
                : <button onClick={() => { setActivity('commands'); setOpen(false) }}>查看</button>}
            </div>
          </article>)}
        </section>
        <section>
          <h2>隧道与监控</h2>
          {tunnels.length === 0 ? <p>无隧道</p> : tunnels.slice(0, 12).map((tunnel) => {
            const snapshot = tunnelStates[tunnel.id]
            return <article className="task-row" key={tunnel.id}>
              <State state={snapshot?.state ?? 'stopped'} />
              <span><strong>{tunnel.name}</strong><small>{snapshot?.message ?? `${tunnel.bindAddress}:${String(tunnel.sourcePort)} · ${snapshot?.state ?? 'stopped'}`}</small></span>
              <button onClick={() => { setActivity('tunnels'); setOpen(false) }}>查看</button>
            </article>
          })}
          {selected && telemetry[selected.id]?.state === 'failed' && <article className="task-row">
            <CircleAlert className="task-failed" size={13} />
            <span><strong>{selected.alias} 监控</strong><small>{telemetry[selected.id]?.error ?? '采集失败'}</small></span>
            <button onClick={() => { setActivity('monitor'); setOpen(false) }}>恢复</button>
          </article>}
        </section>
      </div>
    </section>}
  </>
}

function State({ state }: { state: string }): React.JSX.Element { return <span className={`task-state task-${state}`} title={state} /> }
function tail(path: string): string { return path.replace(/\\/gu, '/').split('/').filter(Boolean).at(-1) ?? path }
function formatBytes(value: number): string { if (value < 1024) return `${String(value)} B`; if (value < 1024 ** 2) return `${(value / 1024).toFixed(1)} KiB`; return `${(value / 1024 ** 2).toFixed(1)} MiB` }
