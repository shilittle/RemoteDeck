import { useEffect, useMemo, useState } from 'react'
import { Activity, Cable, Pencil, Play, Plus, RefreshCw, RotateCw, Save, Square, Trash2 } from 'lucide-react'
import { api, errorMessage } from '../api'
import { useAppStore } from '../store'
import type { TunnelDraft, TunnelProfile, TunnelRuntimeState, TunnelSnapshot } from '../types'
import { validateTunnelDraft } from '../validation'

function newTunnel(hostId: string | null): TunnelDraft {
  return {
    hostId: hostId ?? '',
    name: '',
    direction: 'local',
    bindAddress: '127.0.0.1',
    sourcePort: 8888,
    targetHost: '127.0.0.1',
    targetPort: 8888,
    autoStart: false,
    autoReconnect: true,
    healthCheck: { kind: 'tcp', intervalSeconds: 10, timeoutSeconds: 3 }
  }
}

export function TunnelPanel(): React.JSX.Element {
  const hosts = useAppStore((state) => state.hosts)
  const selectedHostId = useAppStore((state) => state.selectedHostId)
  const tunnels = useAppStore((state) => state.tunnels)
  const tunnelStates = useAppStore((state) => state.tunnelStates)
  const saveTunnel = useAppStore((state) => state.saveTunnel)
  const deleteTunnel = useAppStore((state) => state.deleteTunnel)
  const applyTunnelState = useAppStore((state) => state.applyTunnelState)
  const [draft, setDraft] = useState<TunnelDraft>(() => newTunnel(selectedHostId))
  const [editing, setEditing] = useState(false)
  const [message, setMessage] = useState('')
  const [error, setError] = useState('')
  const [busyId, setBusyId] = useState<string | null>(null)
  const selected = hosts.find((host) => host.id === selectedHostId) ?? null
  const selectedTunnels = useMemo(() => tunnels.filter((tunnel) => tunnel.hostId === selectedHostId), [tunnels, selectedHostId])

  useEffect(() => {
    setDraft(newTunnel(selectedHostId))
    setEditing(false)
    if (!selectedHostId) return
    let disposed = false
    void api.listTunnels(selectedHostId).then((snapshots) => { if (!disposed) snapshots.forEach(applyTunnelState) }).catch((reason: unknown) => { if (!disposed) setError(errorMessage(reason)) })
    return () => { disposed = true }
  }, [applyTunnelState, selectedHostId])

  const patch = <K extends keyof TunnelDraft>(key: K, value: TunnelDraft[K]): void => setDraft((current) => ({ ...current, [key]: value }))

  const run = async (id: string, operation: () => Promise<void>): Promise<void> => {
    setBusyId(id)
    setError('')
    setMessage('')
    try { await operation() } catch (reason) { setError(errorMessage(reason)) } finally { setBusyId(null) }
  }

  const save = async (): Promise<void> => {
    const next = { ...draft, hostId: draft.hostId || selectedHostId || '' }
    const validation = validateTunnelDraft(next)
    if (validation) { setError(validation); return }
    await run(next.id ?? 'new', async () => {
      const saved = await saveTunnel(next)
      setDraft({ ...saved })
      setEditing(false)
      setMessage('隧道配置已保存。')
    })
  }

  const changeState = async (profile: TunnelProfile, action: 'start' | 'stop' | 'restart'): Promise<void> => {
    await run(profile.id, async () => {
      if (action === 'stop') {
        applyTunnelState(await api.stopTunnel(profile.id))
      } else {
        const snapshot = action === 'restart' ? await api.restartTunnel(profile.id) : await api.startTunnel(profile.id)
        applyTunnelState(snapshot)
      }
    })
  }

  const remove = async (profile: TunnelProfile): Promise<void> => {
    if (!window.confirm(`删除隧道“${profile.name}”？`)) return
    await run(profile.id, async () => { await deleteTunnel(profile.id); setMessage('隧道已删除。') })
  }

  if (!selected) return <section className="empty-state"><Cable size={42} /><h1>选择一台主机</h1><p>每条转发由 RemoteDeck 自有的独立 OpenSSH 进程承载。</p></section>

  return (
    <section className="panel tunnel-workspace">
      <header className="panel-heading">
        <div><h1>端口隧道</h1><p>{selected.alias} · LocalForward / RemoteForward、健康检查与自动恢复</p></div>
        <div className="button-row wrap"><button disabled={busyId !== null} onClick={() => { void api.listTunnels(selected.id).then((snapshots) => snapshots.forEach(applyTunnelState)).catch((reason: unknown) => setError(errorMessage(reason))) }}><RefreshCw size={14} />刷新</button><button className="primary" onClick={() => { setDraft(newTunnel(selected.id)); setEditing(true) }}><Plus size={14} />新建隧道</button></div>
      </header>
      {error && <div className="notice error" role="alert">{error}<button onClick={() => setError('')}>关闭</button></div>}
      {message && <div className="notice success" role="status">{message}</div>}
      {editing && <TunnelEditor draft={draft} hosts={hosts} busy={busyId !== null} onPatch={patch} onSave={() => { void save() }} onCancel={() => setEditing(false)} />}
      <div className="tunnel-grid">
        {selectedTunnels.length === 0 && !editing
          ? <div className="tunnel-empty"><Cable size={38} /><h2>尚无隧道</h2><p>创建本地或远程端口转发后即可启动。</p></div>
          : selectedTunnels.map((profile) => <TunnelCard key={profile.id} profile={profile} snapshot={tunnelStates[profile.id]} busy={busyId === profile.id} onEdit={() => { setDraft({ ...profile }); setEditing(true) }} onStart={() => { void changeState(profile, 'start') }} onStop={() => { void changeState(profile, 'stop') }} onRestart={() => { void changeState(profile, 'restart') }} onDelete={() => { void remove(profile) }} />)}
      </div>
    </section>
  )
}

function TunnelEditor({ draft, hosts, busy, onPatch, onSave, onCancel }: { draft: TunnelDraft; hosts: Array<{ id: string; alias: string }>; busy: boolean; onPatch: <K extends keyof TunnelDraft>(key: K, value: TunnelDraft[K]) => void; onSave: () => void; onCancel: () => void }): React.JSX.Element {
  const health = draft.healthCheck ?? { kind: 'none' as const, intervalSeconds: 10, timeoutSeconds: 3 }
  return <section className="card tunnel-editor"><div className="card-title"><Cable size={17} /><h2>{draft.id ? '编辑隧道' : '新建隧道'}</h2></div><div className="form-grid compact"><label><span>主机</span><select value={draft.hostId} onChange={(event) => onPatch('hostId', event.target.value)}>{hosts.map((host) => <option key={host.id} value={host.id}>{host.alias}</option>)}</select></label><label><span>名称</span><input value={draft.name} onChange={(event) => onPatch('name', event.target.value)} placeholder="Jupyter" /></label><label><span>方向</span><select value={draft.direction} onChange={(event) => onPatch('direction', event.target.value as TunnelDraft['direction'])}><option value="local">LocalForward（本地 → 远端）</option><option value="remote">RemoteForward（远端 → 本地）</option></select></label><label><span>监听地址</span><input value={draft.bindAddress} onChange={(event) => onPatch('bindAddress', event.target.value)} /></label><label><span>源端口</span><input type="number" min={1} max={65535} value={draft.sourcePort} onChange={(event) => onPatch('sourcePort', Number(event.target.value))} /></label><label><span>目标端口</span><input type="number" min={1} max={65535} value={draft.targetPort} onChange={(event) => onPatch('targetPort', Number(event.target.value))} /></label><label className="wide"><span>目标主机</span><input value={draft.targetHost} onChange={(event) => onPatch('targetHost', event.target.value)} /></label><label><span>健康检查</span><select value={health.kind} onChange={(event) => onPatch('healthCheck', { ...health, kind: event.target.value as 'none' | 'tcp' })}><option value="none">关闭</option><option value="tcp">TCP 探测</option></select></label><label><span>探测间隔（秒）</span><input type="number" min={3} max={300} disabled={health.kind === 'none'} value={health.intervalSeconds} onChange={(event) => onPatch('healthCheck', { ...health, intervalSeconds: Number(event.target.value) })} /></label><label className="toggle"><input type="checkbox" checked={draft.autoStart ?? false} onChange={(event) => onPatch('autoStart', event.target.checked)} /><span>应用启动后自动启动</span></label><label className="toggle"><input type="checkbox" checked={draft.autoReconnect ?? true} onChange={(event) => onPatch('autoReconnect', event.target.checked)} /><span>异常退出后自动恢复</span></label></div><div className="button-row"><button className="primary" disabled={busy} onClick={onSave}><Save size={14} />保存</button><button disabled={busy} onClick={onCancel}>取消</button></div></section>
}

function TunnelCard({ profile, snapshot, busy, onEdit, onStart, onStop, onRestart, onDelete }: { profile: TunnelProfile; snapshot?: TunnelSnapshot; busy: boolean; onEdit: () => void; onStart: () => void; onStop: () => void; onRestart: () => void; onDelete: () => void }): React.JSX.Element {
  const state = snapshot?.state ?? 'stopped'
  return <article className="tunnel-card card"><div className="tunnel-card-head"><div><h2>{profile.name}</h2><p>{profile.direction === 'local' ? 'LocalForward' : 'RemoteForward'} · {profile.bindAddress}:{String(profile.sourcePort)} → {profile.targetHost}:{String(profile.targetPort)}</p></div><span className={`tunnel-state state-${state}`}><i />{stateLabel(state)}</span></div><div className="tunnel-metrics"><span><Activity size={14} />健康 {healthLabel(snapshot?.health)}</span><span>运行 {formatDuration(snapshot?.uptimeSeconds ?? 0)}</span><span>重连 {String(snapshot?.reconnectCount ?? 0)}</span>{profile.autoStart && <span>自动启动</span>}</div>{snapshot?.message && <div className="tunnel-error">{snapshot.message}</div>}<div className="button-row wrap"><button className="primary" disabled={busy || state === 'running' || state === 'starting'} onClick={onStart}><Play size={14} />启动</button><button disabled={busy || state === 'stopped'} onClick={onStop}><Square size={14} />停止</button><button disabled={busy} onClick={onRestart}><RotateCw size={14} />重启</button><button disabled={busy} onClick={onEdit}><Pencil size={14} />编辑</button><button className="danger" disabled={busy} onClick={onDelete}><Trash2 size={14} />删除</button></div><details className="tunnel-logs"><summary>运行日志（{String(snapshot?.logs?.length ?? 0)}）</summary>{snapshot?.logs?.length ? <ol>{snapshot.logs.toReversed().map((entry) => <li key={`${entry.at}-${entry.message}`} className={`log-${entry.level}`}><time>{new Date(entry.at).toLocaleTimeString()}</time><span>{entry.message}</span></li>)}</ol> : <p>尚无日志。</p>}</details></article>
}

function stateLabel(state: TunnelRuntimeState): string { return { stopped: '已停止', starting: '启动中', running: '在线', waiting: '等待恢复', failed: '失败' }[state] }
function healthLabel(value: TunnelSnapshot['health']): string { return value ? { unknown: '未知', healthy: '正常', degraded: '降级', failed: '失败' }[value] : '未知' }
function formatDuration(seconds: number): string { if (seconds < 60) return `${String(seconds)} 秒`; if (seconds < 3600) return `${String(Math.floor(seconds / 60))} 分`; return `${String(Math.floor(seconds / 3600))} 小时 ${String(Math.floor(seconds % 3600 / 60))} 分` }
