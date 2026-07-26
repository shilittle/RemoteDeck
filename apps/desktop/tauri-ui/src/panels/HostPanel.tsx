import { useEffect, useMemo, useState } from 'react'
import { api, errorMessage } from '../api'
import { useAppStore } from '../store'
import type { ConnectionTestResult, HostDraft, HostKeyCandidate, HostProfile } from '../types'
import { validateHostDraft } from '../validation'

const emptyHost: HostDraft = {
  alias: '', hostname: '', port: 22, username: '', identityFile: '', proxyJump: '', defaultWorkspace: '~', groups: [],
  advanced: { connectTimeoutSeconds: 15, serverAliveIntervalSeconds: 30, serverAliveCountMax: 3, tcpKeepAlive: true, compression: false, identitiesOnly: false },
  monitorEnabled: true
}

function draftFromHost(host: HostProfile | null): HostDraft {
  if (!host) return { ...emptyHost, groups: [], advanced: { ...emptyHost.advanced } }
  return {
    id: host.id, alias: host.alias, hostname: host.hostname, port: host.port, username: host.username,
    identityFile: host.identityFile ?? '', proxyJump: host.proxyJump ?? '', defaultWorkspace: host.defaultWorkspace,
    groups: [...host.groups], advanced: { ...host.advanced }, monitorEnabled: host.monitorEnabled
  }
}

export function HostPanel(): React.JSX.Element {
  const hosts = useAppStore((state) => state.hosts)
  const selectedHostId = useAppStore((state) => state.selectedHostId)
  const saveHost = useAppStore((state) => state.saveHost)
  const deleteHost = useAppStore((state) => state.deleteHost)
  const busy = useAppStore((state) => state.busy)
  const selected = useMemo(() => hosts.find((host) => host.id === selectedHostId) ?? null, [hosts, selectedHostId])
  const [draft, setDraft] = useState<HostDraft>(() => draftFromHost(selected))
  const [message, setMessage] = useState('')
  const [testResult, setTestResult] = useState<ConnectionTestResult | null>(null)
  const [candidates, setCandidates] = useState<HostKeyCandidate[]>([])
  const [actionBusy, setActionBusy] = useState(false)

  useEffect(() => { setDraft(draftFromHost(selected)); setCandidates([]); setTestResult(null); setMessage('') }, [selected])
  const patch = <K extends keyof HostDraft>(key: K, value: HostDraft[K]): void => setDraft((current) => ({ ...current, [key]: value }))

  const save = async (): Promise<void> => {
    const validation = validateHostDraft(draft)
    if (validation) { setMessage(validation); return }
    try { const host = await saveHost(draft); setDraft(draftFromHost(host)); setMessage('主机配置已保存。') }
    catch (error) { setMessage(errorMessage(error)) }
  }
  const scan = async (): Promise<void> => {
    if (!draft.id) { setMessage('先保存主机。'); return }
    setActionBusy(true); setCandidates([])
    try {
      const keys = await api.scanHostKeys(draft.id)
      setCandidates(keys)
      setMessage(keys.length ? '请通过独立可信渠道核对 SHA-256 指纹后再接受。' : '目标未返回可用 host key。')
    } catch (error) { setMessage(errorMessage(error)) }
    finally { setActionBusy(false) }
  }
  const accept = async (candidate: HostKeyCandidate): Promise<void> => {
    if (!draft.id) return
    setActionBusy(true)
    try { await api.acceptHostKey(draft.id, candidate); setCandidates([]); setMessage(`已信任 ${candidate.algorithm} ${candidate.sha256Fingerprint}`) }
    catch (error) { setMessage(errorMessage(error)) }
    finally { setActionBusy(false) }
  }
  const test = async (): Promise<void> => {
    if (!draft.id) { setMessage('先保存主机。'); return }
    setActionBusy(true); setTestResult(null)
    try { setTestResult(await api.testConnection(draft.id)) }
    catch (error) { setMessage(errorMessage(error)) }
    finally { setActionBusy(false) }
  }
  const remove = async (): Promise<void> => {
    if (!draft.id || !window.confirm(`删除主机“${draft.alias}”及其隧道配置？`)) return
    try { await deleteHost(draft.id); setDraft(draftFromHost(null)) }
    catch (error) { setMessage(errorMessage(error)) }
  }

  return (
    <section className="panel">
      <div className="panel-heading"><div><h1>{draft.id ? `编辑 ${draft.alias}` : '添加主机'}</h1><p>使用系统 OpenSSH；密码与密钥口令不写入配置。</p></div><div className="button-row">{draft.id && <button className="danger" disabled={busy || actionBusy} onClick={() => void remove()}>删除</button>}<button onClick={() => setDraft(draftFromHost(null))}>新建</button><button className="primary" disabled={busy || actionBusy} onClick={() => void save()}>保存</button></div></div>
      <div className="form-grid">
        <label><span>别名</span><input value={draft.alias} onChange={(event) => patch('alias', event.target.value)} placeholder="lab-gpu" /></label>
        <label><span>主机名 / IP</span><input value={draft.hostname} onChange={(event) => patch('hostname', event.target.value)} placeholder="10.0.0.2" /></label>
        <label><span>端口</span><input type="number" min={1} max={65535} value={draft.port} onChange={(event) => patch('port', Number(event.target.value))} /></label>
        <label><span>用户名</span><input value={draft.username} onChange={(event) => patch('username', event.target.value)} placeholder="researcher" /></label>
        <label className="wide"><span>私钥路径（可留空，使用 ssh-agent）</span><input value={draft.identityFile ?? ''} onChange={(event) => patch('identityFile', event.target.value)} placeholder="C:\\Users\\name\\.ssh\\id_ed25519" /></label>
        <label><span>ProxyJump（可选）</span><input value={draft.proxyJump ?? ''} onChange={(event) => patch('proxyJump', event.target.value)} placeholder="user@jump.example" /></label>
        <label><span>默认工作目录</span><input value={draft.defaultWorkspace ?? '~'} onChange={(event) => patch('defaultWorkspace', event.target.value)} placeholder="~/project" /></label>
        <label className="wide"><span>分组（逗号分隔）</span><input value={draft.groups.join(', ')} onChange={(event) => patch('groups', event.target.value.split(','))} /></label>
      </div>
      <div className="security-strip"><div><strong>Host key 信任链</strong><small>首次连接不会自动信任；变更后的 key 会被严格拒绝。</small></div><div className="button-row"><button disabled={!draft.id || actionBusy} onClick={() => void scan()}>扫描指纹</button><button disabled={!draft.id || actionBusy} onClick={() => void test()}>测试连接</button></div></div>
      {candidates.length > 0 && <div className="candidate-list">{candidates.map((candidate) => <div className="candidate" key={`${candidate.algorithm}-${candidate.sha256Fingerprint}`}><div><strong>{candidate.algorithm}</strong><code>{candidate.sha256Fingerprint}</code></div><button className="primary" disabled={actionBusy} onClick={() => void accept(candidate)}>接受此指纹</button></div>)}</div>}
      {testResult && <div className={testResult.success ? 'result-card success' : 'result-card failed'}><strong>{testResult.success ? `连接成功 · ${String(testResult.latencyMs)} ms` : '连接失败'}</strong><pre>{testResult.serverLine ?? testResult.error ?? ''}</pre></div>}
      {message && <p className="inline-message">{message}</p>}
    </section>
  )
}
