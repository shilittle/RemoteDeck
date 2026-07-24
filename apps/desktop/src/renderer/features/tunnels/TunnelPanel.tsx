import { useEffect, useMemo, useState } from 'react'
import { Activity, AlertTriangle, Cable, Pencil, Play, PlugZap, Plus, Radar, RefreshCw, RotateCw, Save, Square, Trash2 } from 'lucide-react'
import type { AuthProfile, TunnelHealthCheck, TunnelProfile } from '../../../protocol/domain'
import type { ConnectionCredentials } from '../../../protocol/ssh'
import type { ClashCandidate, TunnelCreateRequest, TunnelSnapshot } from '../../../protocol/tunnel'
import { useHostStore } from '../../host-store'

type HealthKind = 'none' | 'tcp' | 'http'

interface TunnelDraft {
  name: string
  direction: 'local' | 'remote'
  bindAddress: string
  sourcePort: string
  targetHost: string
  targetPort: string
  autoStart: boolean
  healthKind: HealthKind
  healthInterval: string
  healthTimeout: string
  healthPath: string
  healthStatus: string
  cleanupCommand: string
  cleanupAuthorized: boolean
}

const emptyDraft: TunnelDraft = {
  name: '',
  direction: 'local',
  bindAddress: '127.0.0.1',
  sourcePort: '8080',
  targetHost: '127.0.0.1',
  targetPort: '8080',
  autoStart: false,
  healthKind: 'tcp',
  healthInterval: '10',
  healthTimeout: '2000',
  healthPath: '/',
  healthStatus: '200',
  cleanupCommand: '',
  cleanupAuthorized: false
}

export function TunnelPanel(): React.JSX.Element {
  const hosts = useHostStore((state) => state.items)
  const selectedId = useHostStore((state) => state.selectedId)
  const selected = hosts.find((item) => item.host.id === selectedId)
  const jump = selected?.host.jumpHostId ? hosts.find((item) => item.host.id === selected.host.jumpHostId) : undefined
  const [items, setItems] = useState<TunnelSnapshot[]>([])
  const [editingId, setEditingId] = useState<string | null>(null)
  const [draft, setDraft] = useState<TunnelDraft>(emptyDraft)
  const [password, setPassword] = useState('')
  const [passphrase, setPassphrase] = useState('')
  const [jumpPassword, setJumpPassword] = useState('')
  const [jumpPassphrase, setJumpPassphrase] = useState('')
  const [candidates, setCandidates] = useState<ClashCandidate[]>([])
  const [candidateKey, setCandidateKey] = useState('')
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState('')
  const [error, setError] = useState('')

  useEffect(() => {
    setEditingId(null)
    setItems([])
    if (!selectedId) return
    void window.remoteDeck.tunnels.list(selectedId).then(setItems).catch((reason: unknown) => setError(errorMessage(reason)))
  }, [selectedId])

  useEffect(() => window.remoteDeck.tunnels.onEvent(({ snapshot }) => {
    setItems((current) => snapshot.profile.hostId !== selectedId ? current : current.some((item) => item.profile.id === snapshot.profile.id)
      ? current.map((item) => item.profile.id === snapshot.profile.id ? snapshot : item)
      : [...current, snapshot])
  }), [selectedId])

  const candidate = useMemo(() => candidates.find((item) => candidateId(item) === candidateKey), [candidateKey, candidates])

  function run(action: () => Promise<void>): void {
    setBusy(true)
    setError('')
    setMessage('')
    void action().catch((reason: unknown) => setError(errorMessage(reason))).finally(() => setBusy(false))
  }

  function openNew(): void {
    setDraft(emptyDraft)
    setEditingId('new')
  }

  function openEdit(snapshot: TunnelSnapshot): void {
    setDraft(draftFromProfile(snapshot.profile))
    setEditingId(snapshot.profile.id)
  }

  async function save(): Promise<void> {
    if (!selectedId) return
    const request = requestFromDraft(selectedId, draft)
    if (editingId === 'new') await window.remoteDeck.tunnels.create(request)
    else if (editingId) {
      const { hostId: _hostId, healthCheck, legacyCleanupHook, ...patch } = request
      void _hostId
      await window.remoteDeck.tunnels.update({ tunnelId: editingId, patch: { ...patch, healthCheck: healthCheck ?? null, legacyCleanupHook: legacyCleanupHook ?? null } })
    }
    setItems(await window.remoteDeck.tunnels.list(selectedId))
    setEditingId(null)
    setMessage('隧道配置已保存。')
  }

  async function start(snapshot: TunnelSnapshot, restart = false): Promise<void> {
    if (!selected) return
    try {
      const credentials = makeCredentials(selected.auth, password, passphrase, jump?.auth, jumpPassword, jumpPassphrase)
      const next = restart
        ? await window.remoteDeck.tunnels.restart({ tunnelId: snapshot.profile.id, credentials })
        : await window.remoteDeck.tunnels.start({ tunnelId: snapshot.profile.id, credentials })
      setItems((current) => current.map((item) => item.profile.id === next.profile.id ? next : item))
    } finally {
      setPassword('')
      setPassphrase('')
      setJumpPassword('')
      setJumpPassphrase('')
    }
  }

  if (!selected) return <section className="file-state"><Cable size={42} /><h1>选择一台主机</h1><p>每条转发使用独立 SSH 连接，不依赖终端会话。</p></section>

  return (
    <section className="tunnel-workspace">
      <header className="tunnel-toolbar">
        <div><h1>端口隧道</h1><p>{selected.host.alias} · 每条隧道独立连接与自动恢复</p></div>
        <div className="button-row wrap">
          <button className="secondary" disabled={busy} onClick={() => run(async () => setItems(await window.remoteDeck.tunnels.list(selected.host.id)))}><RefreshCw size={15} />刷新</button>
          <button className="secondary" disabled={busy} onClick={() => run(async () => { const found = await window.remoteDeck.tunnels.detectClash(); setCandidates(found); setCandidateKey(''); setMessage(found.length ? `检测到 ${String(found.length)} 个 Clash/Mihomo 候选，请手动选择。` : '未检测到 Clash/Mihomo 监听端口。') })}><Radar size={15} />检测 Clash</button>
          <button className="primary" disabled={busy} onClick={openNew}><Plus size={15} />新建隧道</button>
        </div>
      </header>

      {(selected.auth.method === 'password' || selected.auth.method === 'keyboard_interactive') && <div className="tunnel-credentials"><label><span>主机密码 / 交互回答（仅内存）</span><input type="password" autoComplete="off" value={password} onChange={(event) => setPassword(event.target.value)} /></label>{jump && (jump.auth.method === 'password' || jump.auth.method === 'keyboard_interactive') && <label><span>跳板密码 / 交互回答</span><input type="password" autoComplete="off" value={jumpPassword} onChange={(event) => setJumpPassword(event.target.value)} /></label>}</div>}
      {selected.auth.method === 'private_key' && <div className="tunnel-credentials"><label><span>私钥口令（如有，仅内存）</span><input type="password" autoComplete="off" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} /></label>{jump?.auth.method === 'private_key' && <label><span>跳板私钥口令（如有）</span><input type="password" autoComplete="off" value={jumpPassphrase} onChange={(event) => setJumpPassphrase(event.target.value)} /></label>}</div>}

      {candidates.length > 0 && <section className="clash-candidates card"><div><h2>Clash / Mihomo 候选</h2><p>探测只读，不会修改或重启代理进程。请明确选择要使用的端口。</p></div><div className="candidate-list">{candidates.map((item) => <label key={candidateId(item)}><input type="radio" name="clash-candidate" checked={candidateKey === candidateId(item)} onChange={() => setCandidateKey(candidateId(item))} /><span><strong>{item.processName} · {item.address}:{item.port}</strong><small>{item.protocol} · {item.confidence} · {item.detail}</small></span></label>)}</div><button className="secondary" disabled={!candidate} onClick={() => { if (!candidate) return; setDraft({ ...emptyDraft, name: 'LabPulse 代理反向隧道', direction: 'remote', sourcePort: '17890', targetPort: String(candidate.port), healthKind: 'tcp' }); setEditingId('new'); setMessage(`已将 ${candidate.processName}:${String(candidate.port)} 应用到草稿，仍需保存。`) }}>应用到 127.0.0.1:17890 RemoteForward</button></section>}
      {message && <div className="notice">{message}</div>}
      {error && <div className="error-banner" role="alert">{error}</div>}

      {editingId && <TunnelEditor draft={draft} setDraft={setDraft} busy={busy} editing={editingId !== 'new'} onSave={() => run(save)} onCancel={() => setEditingId(null)} />}

      <div className="tunnel-grid">
        {items.length === 0 && !editingId ? <div className="tunnel-empty"><PlugZap size={38} /><h2>尚无隧道</h2><p>创建 LocalForward 或 RemoteForward 后即可启动真实端口转发。</p></div> : items.map((snapshot) => <TunnelCard key={snapshot.profile.id} snapshot={snapshot} busy={busy} onEdit={() => openEdit(snapshot)} onStart={() => run(() => start(snapshot))} onStop={() => run(async () => { const next = await window.remoteDeck.tunnels.stop(snapshot.profile.id); setItems((current) => current.map((item) => item.profile.id === next.profile.id ? next : item)) })} onRestart={() => run(() => start(snapshot, true))} onDelete={() => run(async () => { if (!window.confirm(`删除隧道 ${snapshot.profile.name}？`)) return; await window.remoteDeck.tunnels.delete(snapshot.profile.id); setItems((current) => current.filter((item) => item.profile.id !== snapshot.profile.id)) })} />)}
      </div>
    </section>
  )
}

function TunnelEditor({ draft, setDraft, busy, editing, onSave, onCancel }: { draft: TunnelDraft; setDraft: (value: TunnelDraft) => void; busy: boolean; editing: boolean; onSave: () => void; onCancel: () => void }): React.JSX.Element {
  return <section className="tunnel-editor card">
    <div className="card-title"><Cable size={17} /><h2>{editing ? '编辑隧道' : '新建隧道'}</h2></div>
    <div className="tunnel-form-grid">
      <label><span>名称</span><input value={draft.name} onChange={(event) => setDraft({ ...draft, name: event.target.value })} /></label>
      <label><span>方向</span><select value={draft.direction} onChange={(event) => setDraft({ ...draft, direction: event.target.value as TunnelDraft['direction'] })}><option value="local">LocalForward（本地 → 远端）</option><option value="remote">RemoteForward（远端 → 本地）</option></select></label>
      <label><span>绑定地址</span><input value={draft.bindAddress} onChange={(event) => setDraft({ ...draft, bindAddress: event.target.value })} /></label>
      <label><span>{draft.direction === 'local' ? '本地监听端口' : '远端监听端口'}</span><input type="number" min="1" max="65535" value={draft.sourcePort} onChange={(event) => setDraft({ ...draft, sourcePort: event.target.value })} /></label>
      <label><span>目标主机</span><input value={draft.targetHost} onChange={(event) => setDraft({ ...draft, targetHost: event.target.value })} /></label>
      <label><span>目标端口</span><input type="number" min="1" max="65535" value={draft.targetPort} onChange={(event) => setDraft({ ...draft, targetPort: event.target.value })} /></label>
      <label><span>健康检查</span><select value={draft.healthKind} onChange={(event) => setDraft({ ...draft, healthKind: event.target.value as HealthKind })}><option value="none">关闭</option><option value="tcp">TCP</option><option value="http">HTTP</option></select></label>
      {draft.healthKind !== 'none' && <><label><span>间隔（秒）</span><input type="number" min="2" value={draft.healthInterval} onChange={(event) => setDraft({ ...draft, healthInterval: event.target.value })} /></label><label><span>超时（毫秒）</span><input type="number" min="100" value={draft.healthTimeout} onChange={(event) => setDraft({ ...draft, healthTimeout: event.target.value })} /></label></>}
      {draft.healthKind === 'http' && <><label><span>HTTP 路径</span><input value={draft.healthPath} onChange={(event) => setDraft({ ...draft, healthPath: event.target.value })} /></label><label><span>期望状态码</span><input type="number" min="100" max="599" value={draft.healthStatus} onChange={(event) => setDraft({ ...draft, healthStatus: event.target.value })} /></label></>}
      <label className="toggle"><input type="checkbox" checked={draft.autoStart} onChange={(event) => setDraft({ ...draft, autoStart: event.target.checked })} /><span>应用启动时自动连接（密码不会持久化）</span></label>
    </div>
    <details className="legacy-hook"><summary><AlertTriangle size={15} />旧版端口清理钩子（高风险，默认禁用）</summary><p>仅当 RemoteForward 绑定失败时执行下列原始远端命令。RemoteDeck 不会运行 fuser/kill 或清理非自身资源，除非你在此明确授权。</p><label><span>实际命令</span><textarea value={draft.cleanupCommand} onChange={(event) => setDraft({ ...draft, cleanupCommand: event.target.value, cleanupAuthorized: false })} /></label><label className="toggle danger-choice"><input type="checkbox" checked={draft.cleanupAuthorized} disabled={!draft.cleanupCommand.trim()} onChange={(event) => setDraft({ ...draft, cleanupAuthorized: event.target.checked })} /><span>我授权在绑定失败时执行上面的实际命令，并写入审计日志</span></label></details>
    <div className="button-row"><button className="primary" disabled={busy} onClick={onSave}><Save size={15} />保存配置</button><button className="secondary" disabled={busy} onClick={onCancel}>取消</button></div>
  </section>
}

function TunnelCard({ snapshot, busy, onEdit, onStart, onStop, onRestart, onDelete }: { snapshot: TunnelSnapshot; busy: boolean; onEdit: () => void; onStart: () => void; onStop: () => void; onRestart: () => void; onDelete: () => void }): React.JSX.Element {
  const online = snapshot.state === 'online'
  return <article className="tunnel-card card">
    <div className="tunnel-card-head"><div><h2>{snapshot.profile.name}</h2><p>{snapshot.profile.direction === 'local' ? 'LocalForward' : 'RemoteForward'} · {snapshot.profile.bindAddress}:{snapshot.profile.sourcePort} → {snapshot.profile.targetHost}:{snapshot.profile.targetPort}</p></div><span className={`tunnel-state state-${snapshot.state}`}><i />{stateLabel(snapshot.state)}</span></div>
    <div className="tunnel-metrics"><span><Activity size={14} />健康 {snapshot.health}</span><span>运行 {formatDuration(snapshot.uptimeSeconds)}</span><span>重连 {snapshot.reconnectCount}</span>{snapshot.profile.autoStart && <span>自动启动</span>}</div>
    {snapshot.nextRetryAt && <p className="muted">下次尝试：{new Date(snapshot.nextRetryAt).toLocaleTimeString()}</p>}
    {snapshot.lastError && <div className="tunnel-error">{snapshot.lastError}</div>}
    <div className="button-row wrap"><button className="primary" disabled={busy || online || snapshot.state === 'starting'} onClick={onStart}><Play size={14} />启动</button><button className="secondary" disabled={busy || snapshot.state === 'stopped'} onClick={onStop}><Square size={14} />停止</button><button className="secondary" disabled={busy} onClick={onRestart}><RotateCw size={14} />重启</button><button className="secondary" disabled={busy} onClick={onEdit}><Pencil size={14} />编辑</button><button className="danger" disabled={busy} onClick={onDelete}><Trash2 size={14} />删除</button></div>
    <details className="tunnel-logs"><summary>运行日志（{snapshot.logs.length}）</summary>{snapshot.logs.length === 0 ? <p>尚无日志。</p> : <ol>{snapshot.logs.toReversed().map((entry) => <li key={`${entry.at}:${entry.message}`} className={`log-${entry.level}`}><time>{new Date(entry.at).toLocaleTimeString()}</time><span>{entry.message}</span></li>)}</ol>}</details>
  </article>
}

function requestFromDraft(hostId: string, draft: TunnelDraft): TunnelCreateRequest {
  const healthCheck = makeHealthCheck(draft)
  const cleanup = draft.cleanupCommand.trim()
  return {
    hostId,
    name: draft.name.trim(),
    direction: draft.direction,
    bindAddress: draft.bindAddress.trim(),
    sourcePort: Number(draft.sourcePort),
    targetHost: draft.targetHost.trim(),
    targetPort: Number(draft.targetPort),
    autoStart: draft.autoStart,
    ...(healthCheck ? { healthCheck } : {}),
    ...(cleanup ? { legacyCleanupHook: { command: cleanup, authorized: draft.cleanupAuthorized } } : {})
  }
}

function makeHealthCheck(draft: TunnelDraft): TunnelHealthCheck | undefined {
  if (draft.healthKind === 'none') return undefined
  const base = { intervalSeconds: Number(draft.healthInterval), timeoutMs: Number(draft.healthTimeout) }
  return draft.healthKind === 'tcp' ? { type: 'tcp', ...base } : { type: 'http', ...base, path: draft.healthPath, expectedStatus: Number(draft.healthStatus) }
}

function draftFromProfile(profile: TunnelProfile): TunnelDraft {
  return {
    name: profile.name,
    direction: profile.direction,
    bindAddress: profile.bindAddress,
    sourcePort: String(profile.sourcePort),
    targetHost: profile.targetHost,
    targetPort: String(profile.targetPort),
    autoStart: profile.autoStart,
    healthKind: profile.healthCheck?.type ?? 'none',
    healthInterval: String(profile.healthCheck?.intervalSeconds ?? 10),
    healthTimeout: String(profile.healthCheck?.timeoutMs ?? 2000),
    healthPath: profile.healthCheck?.type === 'http' ? profile.healthCheck.path : '/',
    healthStatus: profile.healthCheck?.type === 'http' ? String(profile.healthCheck.expectedStatus) : '200',
    cleanupCommand: profile.legacyCleanupHook?.command ?? '',
    cleanupAuthorized: profile.legacyCleanupHook?.authorized ?? false
  }
}

function makeCredentials(auth: AuthProfile, password: string, passphrase: string, jumpAuth?: AuthProfile, jumpPassword = '', jumpPassphrase = ''): ConnectionCredentials {
  const direct = auth.method === 'password' ? { password } : auth.method === 'keyboard_interactive' ? { keyboardInteractiveAnswers: password ? [password] : [] } : auth.method === 'private_key' && passphrase ? { passphrase } : {}
  return jumpAuth ? { ...direct, jump: makeCredentials(jumpAuth, jumpPassword, jumpPassphrase) } : direct
}

function candidateId(item: ClashCandidate): string { return `${String(item.pid)}:${String(item.port)}:${item.protocol}` }
function errorMessage(reason: unknown): string { return reason instanceof Error ? reason.message : '隧道操作失败' }
function formatDuration(seconds: number): string { const hours = Math.floor(seconds / 3600); const minutes = Math.floor((seconds % 3600) / 60); const rest = seconds % 60; return `${String(hours).padStart(2, '0')}:${String(minutes).padStart(2, '0')}:${String(rest).padStart(2, '0')}` }
function stateLabel(state: TunnelSnapshot['state']): string { return { stopped: '已停止', starting: '启动中', online: '在线', waiting: '等待重连', failed: '失败' }[state] }
