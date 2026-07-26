import { useEffect, useMemo, useState } from 'react'
import { api, errorMessage } from '../api'
import { useAppStore } from '../store'
import type { TunnelDraft, TunnelProfile, TunnelRuntimeState } from '../types'
import { validateTunnelDraft } from '../validation'

function newTunnel(hostId: string | null): TunnelDraft {
  return { hostId: hostId ?? '', name: '', direction: 'local', bindAddress: '127.0.0.1', sourcePort: 8888, targetHost: '127.0.0.1', targetPort: 8888, autoStart: false }
}

export function TunnelPanel(): React.JSX.Element {
  const hosts = useAppStore((state) => state.hosts)
  const selectedHostId = useAppStore((state) => state.selectedHostId)
  const tunnels = useAppStore((state) => state.tunnels)
  const tunnelStates = useAppStore((state) => state.tunnelStates)
  const saveTunnel = useAppStore((state) => state.saveTunnel)
  const deleteTunnel = useAppStore((state) => state.deleteTunnel)
  const [draft, setDraft] = useState<TunnelDraft>(() => newTunnel(selectedHostId))
  const [message, setMessage] = useState('')
  const [actionBusy, setActionBusy] = useState<string | null>(null)
  const selectedTunnels = useMemo(() => tunnels.filter((tunnel) => tunnel.hostId === selectedHostId), [tunnels, selectedHostId])
  useEffect(() => { if (!draft.id) setDraft(newTunnel(selectedHostId)) }, [selectedHostId])
  const patch = <K extends keyof TunnelDraft>(key: K, value: TunnelDraft[K]): void => setDraft((current) => ({ ...current, [key]: value }))

  const save = async (): Promise<void> => {
    const next = { ...draft, hostId: draft.hostId || selectedHostId || '' }
    const validation = validateTunnelDraft(next)
    if (validation) { setMessage(validation); return }
    try { const saved = await saveTunnel(next); setDraft({ ...saved }); setMessage('隧道配置已保存。') }
    catch (error) { setMessage(errorMessage(error)) }
  }
  const toggle = async (tunnel: TunnelProfile): Promise<void> => {
    setActionBusy(tunnel.id)
    try {
      const running = ['running', 'starting'].includes(tunnelStates[tunnel.id]?.state ?? '')
      if (running) await api.stopTunnel(tunnel.id); else await api.startTunnel(tunnel.id)
    } catch (error) { setMessage(errorMessage(error)) }
    finally { setActionBusy(null) }
  }
  const remove = async (tunnel: TunnelProfile): Promise<void> => {
    if (!window.confirm(`删除隧道“${tunnel.name}”？`)) return
    try { await deleteTunnel(tunnel.id); setDraft(newTunnel(selectedHostId)) }
    catch (error) { setMessage(errorMessage(error)) }
  }

  return <section className="panel"><div className="panel-heading"><div><h1>端口隧道</h1><p>每条转发使用独立 ssh 进程；停止只影响本应用创建的进程。</p></div><div className="button-row"><button onClick={() => setDraft(newTunnel(selectedHostId))}>新建</button><button className="primary" onClick={() => void save()}>保存</button></div></div><div className="split-layout"><div className="tunnel-list">{selectedTunnels.map((tunnel) => { const runtime = tunnelStates[tunnel.id]; return <article className="tunnel-card" key={tunnel.id}><button className="card-main" onClick={() => setDraft({ ...tunnel })}><strong>{tunnel.name}</strong><code>{tunnel.direction === 'local' ? '-L' : '-R'} {tunnel.bindAddress}:{tunnel.sourcePort} → {tunnel.targetHost}:{tunnel.targetPort}</code><small className={`tunnel-state ${runtime?.state ?? 'stopped'}`}>{tunnelStateLabel(runtime?.state)}</small></button><div className="card-actions"><button disabled={actionBusy === tunnel.id} onClick={() => void toggle(tunnel)}>{runtime?.state === 'running' || runtime?.state === 'starting' ? '停止' : '启动'}</button><button className="danger-text" onClick={() => void remove(tunnel)}>删除</button></div>{runtime?.message && <p>{runtime.message}</p>}</article> })}{selectedTunnels.length === 0 && <p className="empty-copy">当前主机没有隧道。</p>}</div><div className="form-grid compact"><label className="wide"><span>主机</span><select value={draft.hostId || selectedHostId || ''} onChange={(event) => patch('hostId', event.target.value)}><option value="">请选择</option>{hosts.map((host) => <option key={host.id} value={host.id}>{host.alias}</option>)}</select></label><label className="wide"><span>名称</span><input value={draft.name} onChange={(event) => patch('name', event.target.value)} placeholder="Jupyter" /></label><label><span>方向</span><select value={draft.direction} onChange={(event) => patch('direction', event.target.value as TunnelDraft['direction'])}><option value="local">本地 -L</option><option value="remote">远程 -R</option></select></label><label><span>监听地址</span><input value={draft.bindAddress} onChange={(event) => patch('bindAddress', event.target.value)} /></label><label><span>源端口</span><input type="number" value={draft.sourcePort} onChange={(event) => patch('sourcePort', Number(event.target.value))} /></label><label><span>目标端口</span><input type="number" value={draft.targetPort} onChange={(event) => patch('targetPort', Number(event.target.value))} /></label><label className="wide"><span>目标主机</span><input value={draft.targetHost} onChange={(event) => patch('targetHost', event.target.value)} /></label></div></div>{message && <p className="inline-message">{message}</p>}</section>
}

function tunnelStateLabel(state: TunnelRuntimeState | undefined): string {
  if (state === 'starting') return '启动中'
  if (state === 'running') return '运行中'
  if (state === 'failed') return '失败'
  return '已停止'
}
