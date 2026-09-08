import { useEffect, useMemo, useRef, useState } from 'react'
import {
  Check,
  Copy,
  FileKey,
  Fingerprint,
  FolderOpen,
  KeyRound,
  Plus,
  RefreshCw,
  Save,
  Server,
  ShieldAlert,
  Trash2,
  Upload
} from 'lucide-react'
import { api, errorMessage } from '../api'
import { classifyConnectionFailure, connectionFailureHint, hasTrustedEndpoint } from '../host-trust'
import { retainOrSelectKeyPath } from '../key-selection'
import { useAppStore } from '../store'
import type {
  ConnectionTestResult,
  HostDraft,
  HostKeyCandidate,
  HostKeyRecord,
  HostProfile,
  PrivateKeyRecord
} from '../types'
import { validateHostDraft } from '../validation'

const emptyHost: HostDraft = {
  alias: '',
  hostname: '',
  port: 22,
  username: '',
  authMethod: 'agent',
  identityFile: '',
  proxyJump: '',
  defaultWorkspace: '~',
  groups: [],
  advanced: {
    connectTimeoutSeconds: 15,
    serverAliveIntervalSeconds: 30,
    serverAliveCountMax: 3,
    tcpKeepAlive: true,
    compression: false,
    identitiesOnly: false
  },
  monitorEnabled: true
}

function draftFromHost(host: HostProfile | null): HostDraft {
  if (!host) return { ...emptyHost, groups: [], advanced: { ...emptyHost.advanced } }
  return {
    id: host.id,
    alias: host.alias,
    hostname: host.hostname,
    port: host.port,
    username: host.username,
    authMethod: host.authMethod ?? (host.identityFile ? 'private_key' : 'agent'),
    identityFile: host.identityFile ?? '',
    proxyJump: host.proxyJump ?? '',
    defaultWorkspace: host.defaultWorkspace,
    groups: [...host.groups],
    advanced: { ...host.advanced },
    monitorEnabled: host.monitorEnabled
  }
}

export function HostPanel(): React.JSX.Element {
  const hosts = useAppStore((state) => state.hosts)
  const selectedHostId = useAppStore((state) => state.selectedHostId)
  const saveHost = useAppStore((state) => state.saveHost)
  const deleteHost = useAppStore((state) => state.deleteHost)
  const importSshConfig = useAppStore((state) => state.importSshConfig)
  const sshTrustWarnings = useAppStore((state) => state.sshTrustWarnings)
  const busy = useAppStore((state) => state.busy)
  const configRevision = useAppStore((state) => state.configRevision)
  const selected = useMemo(() => hosts.find((host) => host.id === selectedHostId) ?? null, [hosts, selectedHostId])
  const [draft, setDraft] = useState<HostDraft>(() => draftFromHost(selected))
  const [message, setMessage] = useState('')
  const [testResult, setTestResult] = useState<ConnectionTestResult | null>(null)
  const [testedAt, setTestedAt] = useState<string | null>(null)
  const [candidates, setCandidates] = useState<HostKeyCandidate[]>([])
  const [trustedKeys, setTrustedKeys] = useState<HostKeyRecord[]>([])
  const [trustedKeysState, setTrustedKeysState] = useState<'loading' | 'ready' | 'error'>('loading')
  const [trustedKeysError, setTrustedKeysError] = useState<string | null>(null)
  const [privateKeys, setPrivateKeys] = useState<PrivateKeyRecord[]>([])
  const [actionBusy, setActionBusy] = useState(false)
  const [importPath, setImportPath] = useState('')
  const [keyPath, setKeyPath] = useState('')
  const [keyComment, setKeyComment] = useState('RemoteDeck')
  const keyInventoryRevision = useRef(0)
  const trustedKeysRevision = useRef(0)
  const draftConnectionRef = useRef({ hostId: draft.id ?? null, signature: '', revision: 0 })
  const directJumpHosts = useMemo(
    () => hosts.filter((host) => host.id !== draft.id && !host.proxyJump),
    [draft.id, hosts],
  )
  const isUsedAsJump = Boolean(
    draft.id && hosts.some((host) => host.id !== draft.id && host.proxyJump === draft.id),
  )
  const unresolvedJump = Boolean(
    draft.proxyJump && !hosts.some((host) => host.id === draft.proxyJump),
  )
  const selectedJump = hosts.find((host) => host.id === draft.proxyJump)
  const draftConnectionSignature = JSON.stringify([
    draft.id ?? null,
    draft.hostname,
    draft.port,
    draft.username,
    draft.authMethod ?? null,
    draft.identityFile ?? null,
    draft.proxyJump ?? null,
    draft.advanced?.connectTimeoutSeconds ?? 15,
    draft.advanced?.serverAliveIntervalSeconds ?? 30,
    draft.advanced?.serverAliveCountMax ?? 3,
    draft.advanced?.tcpKeepAlive ?? true,
    draft.advanced?.compression ?? false,
    draft.advanced?.identitiesOnly ?? false,
  ])
  if (draftConnectionRef.current.signature !== draftConnectionSignature) {
    draftConnectionRef.current = {
      hostId: draft.id ?? null,
      signature: draftConnectionSignature,
      revision: draftConnectionRef.current.revision + 1,
    }
  }

  const savedConnectionMatchesDraft = Boolean(
    selected &&
    draft.id === selected.id &&
    draft.hostname === selected.hostname &&
    draft.port === selected.port &&
    draft.username === selected.username &&
    (draft.authMethod ?? (draft.identityFile ? 'private_key' : 'agent')) === (selected.authMethod ?? (selected.identityFile ? 'private_key' : 'agent')) &&
    (draft.identityFile ?? '') === (selected.identityFile ?? '') &&
    (draft.proxyJump ?? '') === (selected.proxyJump ?? '') &&
    (draft.advanced?.connectTimeoutSeconds ?? 15) === selected.advanced.connectTimeoutSeconds &&
    (draft.advanced?.serverAliveIntervalSeconds ?? 30) === selected.advanced.serverAliveIntervalSeconds &&
    (draft.advanced?.serverAliveCountMax ?? 3) === selected.advanced.serverAliveCountMax &&
    (draft.advanced?.tcpKeepAlive ?? true) === selected.advanced.tcpKeepAlive &&
    (draft.advanced?.compression ?? false) === selected.advanced.compression &&
    (draft.advanced?.identitiesOnly ?? false) === selected.advanced.identitiesOnly,
  )
  const endpointTrusted = savedConnectionMatchesDraft && trustedKeysState === 'ready' && hasTrustedEndpoint(draft, trustedKeys)
  const connectionFailureKind = classifyConnectionFailure(testResult?.error ?? null)
  const connectionHint = connectionFailureHint(connectionFailureKind)

  const refreshTrustedKeys = async (): Promise<void> => {
    const revision = ++trustedKeysRevision.current
    setTrustedKeysState('loading')
    setTrustedKeysError(null)
    try {
      const nextTrusted = await api.listHostKeys()
      if (trustedKeysRevision.current !== revision) return
      setTrustedKeys(nextTrusted)
      setTrustedKeysState('ready')
    } catch (error) {
      if (trustedKeysRevision.current !== revision) return
      const detail = errorMessage(error)
      setTrustedKeysError(detail)
      setTrustedKeysState('error')
    }
  }

  useEffect(() => {
    const lifecycle = { disposed: false }
    const revision = ++keyInventoryRevision.current
    void api.listKeys().then((nextPrivate) => {
      if (!lifecycle.disposed && keyInventoryRevision.current === revision) {
        setPrivateKeys(nextPrivate)
        setKeyPath((current) => retainOrSelectKeyPath(current, nextPrivate))
      }
    }).catch((error: unknown) => {
      if (!lifecycle.disposed) setMessage(errorMessage(error))
    })
    return () => { lifecycle.disposed = true }
  }, [])

  useEffect(() => {
    if (configRevision >= 0) void refreshTrustedKeys()
    return () => { trustedKeysRevision.current += 1 }
  }, [configRevision])

  useEffect(() => {
    setCandidates([])
    setTestResult(null)
    setTestedAt(null)
  }, [
    draft.id,
    draft.hostname,
    draft.port,
    draft.username,
    draft.authMethod,
    draft.identityFile,
    draft.proxyJump,
    draft.advanced?.connectTimeoutSeconds,
    draft.advanced?.serverAliveIntervalSeconds,
    draft.advanced?.serverAliveCountMax,
    draft.advanced?.tcpKeepAlive,
    draft.advanced?.compression,
    draft.advanced?.identitiesOnly,
  ])

  const patch = <K extends keyof HostDraft>(key: K, value: HostDraft[K]): void => {
    setDraft((current) => ({ ...current, [key]: value }))
  }
  const patchAdvanced = (key: keyof NonNullable<HostDraft['advanced']>, value: number | boolean): void => {
    setDraft((current) => ({ ...current, advanced: { ...current.advanced, [key]: value } }))
  }

  const run = async (operation: () => Promise<void>): Promise<void> => {
    setActionBusy(true)
    setMessage('')
    try { await operation() } catch (error) { setMessage(errorMessage(error)) } finally { setActionBusy(false) }
  }

  const ensureSavedConnection = (): boolean => {
    if (!draft.id) {
      setMessage('请先保存主机。')
      return false
    }
    if (!savedConnectionMatchesDraft) {
      setMessage('当前连接资料有未保存修改，请先保存后再扫描或测试，避免使用旧配置。')
      return false
    }
    return true
  }

  const save = async (): Promise<void> => {
    const validation = validateHostDraft(draft)
    if (validation) { setMessage(validation); return }
    await run(async () => {
      const host = await saveHost(draft)
      setDraft(draftFromHost(host))
      setCandidates([])
      setTestResult(null)
      setTestedAt(null)
      setMessage('主机配置已保存。已有本机 SSH 信任记录会直接复用。')
    })
  }

  const scan = async (): Promise<void> => {
    if (!ensureSavedConnection()) return
    const requestRevision = draftConnectionRef.current.revision
    await run(async () => {
      const keys = await api.scanHostKeys(draft.id ?? '')
      if (draftConnectionRef.current.revision !== requestRevision) return
      setCandidates(keys)
      setMessage(keys.length ? '请通过独立可信渠道核对 SHA-256 指纹。' : '目标没有返回可用主机密钥。')
    })
  }

  const accept = async (candidate: HostKeyCandidate): Promise<void> => {
    if (candidate.mismatch || !ensureSavedConnection()) return
    const requestRevision = draftConnectionRef.current.revision
    await run(async () => {
      await api.acceptHostKey(draft.id ?? '', candidate)
      await refreshTrustedKeys()
      if (draftConnectionRef.current.revision !== requestRevision) return
      setCandidates([])
      setTestResult(null)
      setTestedAt(null)
      setMessage(`已信任 ${candidate.algorithm} ${candidate.sha256Fingerprint}`)
    })
  }

  const testConnection = async (): Promise<void> => {
    if (!ensureSavedConnection()) return
    const requestRevision = draftConnectionRef.current.revision
    await run(async () => {
      const result = await api.testConnection(draft.id ?? '')
      if (draftConnectionRef.current.revision !== requestRevision) return
      setTestResult(result)
      setTestedAt(new Date().toLocaleString())
      if (!result.success && classifyConnectionFailure(result.error) === 'missing-host-key') {
        setMessage('连接被严格主机校验拒绝：当前端点还没有本地信任记录。已有本机 SSH known_hosts 会直接复用；没有记录时请先扫描并核对指纹。')
      }
    })
  }

  const remove = async (): Promise<void> => {
    if (!draft.id || !window.confirm(`删除主机“${draft.alias}”及其隧道配置？`)) return
    await run(async () => {
      await deleteHost(draft.id ?? '')
      setDraft(draftFromHost(null))
      setMessage('主机已删除。')
    })
  }

  const duplicate = (): void => {
    const aliases = new Set(hosts.map((host) => host.alias.toLowerCase()))
    let index = 2
    let alias = `${draft.alias || 'host'}-copy`
    while (aliases.has(alias.toLowerCase())) { alias = `${draft.alias || 'host'}-copy-${String(index)}`; index += 1 }
    setDraft({ ...draft, id: undefined, alias })
    setCandidates([])
    setTestResult(null)
    setMessage('已复制为新草稿，请确认后保存。')
  }

  const importConfig = async (): Promise<void> => {
    if (!importPath.trim()) { setMessage('请输入 OpenSSH config 路径。'); return }
    await run(async () => {
      const result = await importSshConfig(importPath.trim())
      const warning = result.warnings[0] ? ` ${result.warnings[0]}` : ''
      setCandidates([])
      setTestResult(null)
      setTestedAt(null)
      setMessage(`已导入 ${String(result.imported.length)} 台主机；跳过 ${String(result.skipped.length)} 项。已有本机 SSH 信任记录会直接复用。${warning}`)
    })
  }

  const chooseIdentityFile = async (): Promise<void> => {
    await run(async () => {
      const selected = await api.pickLocalPath(false)
      if (selected) {
        patch('identityFile', selected)
        setKeyPath(selected)
      }
    })
  }

  const chooseExistingKey = async (): Promise<void> => {
    await run(async () => {
      const selected = await api.pickLocalPath(false)
      if (selected) setKeyPath(selected)
    })
  }

  const chooseNewKeyPath = async (): Promise<void> => {
    await run(async () => {
      const selected = await api.pickSavePath('id_ed25519')
      if (selected) setKeyPath(selected)
    })
  }

  const chooseImportConfig = async (): Promise<void> => {
    await run(async () => {
      const selected = await api.pickLocalPath(false)
      if (selected) setImportPath(selected)
    })
  }

  const generateKey = async (): Promise<void> => {
    if (!keyPath.trim()) { setMessage('请输入新私钥保存路径。'); return }
    await run(async () => {
      const result = await api.generateKey({ privateKeyPath: keyPath.trim(), comment: keyComment.trim() || 'RemoteDeck' })
      const revision = ++keyInventoryRevision.current
      const nextPrivate = await api.listKeys()
      if (keyInventoryRevision.current === revision) setPrivateKeys(nextPrivate)
      setMessage(result.message)
    })
  }

  const deployKey = async (): Promise<void> => {
    if (!draft.id || !keyPath.trim()) { setMessage('请先选择主机和私钥。'); return }
    await run(async () => {
      const result = await api.deployKey({ hostId: draft.id ?? '', privateKeyPath: keyPath.trim(), makeDefault: true })
      setMessage(result.message)
      if (result.success) {
        const host = await saveHost({ ...draft, authMethod: 'private_key', identityFile: keyPath.trim() })
        setDraft(draftFromHost(host))
      }
    })
  }

  return (
    <section className="panel host-workspace">
      <header className="panel-heading">
        <div><h1>{draft.id ? `主机 · ${draft.alias}` : '添加 SSH 主机'}</h1><p>连接由系统 OpenSSH 执行；密码与私钥口令只在终端提示中输入。</p></div>
        <div className="button-row wrap">
          {draft.id && <button className="danger" disabled={busy || actionBusy} onClick={() => { void remove() }}><Trash2 size={15} />删除</button>}
          {draft.id && <button disabled={actionBusy} onClick={duplicate}><Copy size={15} />复制</button>}
          <button onClick={() => setDraft(draftFromHost(null))}><Plus size={15} />新建</button>
          <button className="primary" disabled={busy || actionBusy} onClick={() => { void save() }}><Save size={15} />保存</button>
        </div>
      </header>

      <div className="host-grid">
        <section className="card">
          <div className="card-title"><Server size={17} /><h2>连接资料</h2></div>
          <div className="form-grid">
            <label><span>别名</span><input value={draft.alias} onChange={(event) => patch('alias', event.target.value)} placeholder="lab-gpu" /></label>
            <label><span>主机名 / IP</span><input value={draft.hostname} onChange={(event) => patch('hostname', event.target.value)} placeholder="10.0.0.2" /></label>
            <label><span>端口</span><input type="number" min={1} max={65535} value={draft.port} onChange={(event) => patch('port', Number(event.target.value))} /></label>
            <label><span>用户名</span><input value={draft.username} onChange={(event) => patch('username', event.target.value)} placeholder="researcher" /></label>
            <label><span>认证方式</span><select value={draft.authMethod ?? 'agent'} onChange={(event) => { const authMethod = event.target.value as HostDraft['authMethod']; setDraft((current) => ({ ...current, authMethod, identityFile: authMethod === 'private_key' ? current.identityFile : '' })) }}><option value="agent">Windows OpenSSH agent</option><option value="private_key">私钥文件</option><option value="interactive">密码 / 键盘交互</option></select></label>
            <label><span>ProxyJump（已保存主机）</span><select value={draft.proxyJump ?? ''} disabled={isUsedAsJump} onChange={(event) => patch('proxyJump', event.target.value || null)}><option value="">不使用跳板</option>{unresolvedJump && <option value={draft.proxyJump ?? ''}>未解析：{draft.proxyJump}</option>}{directJumpHosts.map((host) => <option key={host.id} value={host.id}>{host.alias} · {host.username}@{host.hostname}:{String(host.port)}</option>)}</select></label>
            {draft.authMethod === 'private_key' && <label className="wide"><span>私钥路径</span><div className="inline-form"><input value={draft.identityFile ?? ''} onChange={(event) => { patch('identityFile', event.target.value); setKeyPath(event.target.value) }} placeholder={'C:\\Users\\name\\.ssh\\id_ed25519'} /><button disabled={actionBusy} onClick={() => { void chooseIdentityFile() }}><FolderOpen size={14} />选择文件</button></div></label>}
            <label><span>默认工作目录</span><input value={draft.defaultWorkspace ?? '~'} onChange={(event) => patch('defaultWorkspace', event.target.value)} placeholder="~/project" /></label>
            <label><span>分组（逗号分隔）</span><input value={draft.groups.join(', ')} onChange={(event) => patch('groups', event.target.value.split(',').map((value) => value.trim()).filter(Boolean))} /></label>
            <label className="toggle"><input type="checkbox" checked={draft.monitorEnabled ?? true} onChange={(event) => patch('monitorEnabled', event.target.checked)} /><span>应用启动时自动监控此主机</span></label>
          </div>
          <p className="muted">跳板必须先作为独立主机保存；已有本机 SSH 信任记录会直接复用，只有新端点或指纹变化时才需要扫描核对。RemoteDeck 会为跳板单独应用其端口、身份与专用 known_hosts；跳板认证必须能在批处理模式下完成。</p>
          {isUsedAsJump && <p className="inline-message">此主机正被其他配置用作跳板，因此必须保持直连。</p>}
          <details className="advanced-settings">
            <summary>高级 OpenSSH 选项</summary>
            <div className="form-grid compact">
              <label><span>连接超时（秒）</span><input type="number" min={1} max={300} value={draft.advanced?.connectTimeoutSeconds ?? 15} onChange={(event) => patchAdvanced('connectTimeoutSeconds', Number(event.target.value))} /></label>
              <label><span>保活间隔（秒）</span><input type="number" min={0} max={3600} value={draft.advanced?.serverAliveIntervalSeconds ?? 30} onChange={(event) => patchAdvanced('serverAliveIntervalSeconds', Number(event.target.value))} /></label>
              <label><span>最大保活失败</span><input type="number" min={1} max={20} value={draft.advanced?.serverAliveCountMax ?? 3} onChange={(event) => patchAdvanced('serverAliveCountMax', Number(event.target.value))} /></label>
              <label className="toggle"><input type="checkbox" checked={draft.advanced?.tcpKeepAlive ?? true} onChange={(event) => patchAdvanced('tcpKeepAlive', event.target.checked)} /><span>TCP KeepAlive</span></label>
              <label className="toggle"><input type="checkbox" checked={draft.advanced?.compression ?? false} onChange={(event) => patchAdvanced('compression', event.target.checked)} /><span>压缩</span></label>
              <label className="toggle"><input type="checkbox" checked={draft.advanced?.identitiesOnly ?? false} onChange={(event) => patchAdvanced('identitiesOnly', event.target.checked)} /><span>仅使用指定身份</span></label>
            </div>
          </details>
        </section>

        <section className="card trust-card">
          <div className="card-title"><Fingerprint size={17} /><h2>主机密钥信任</h2></div>
          <p className="muted">当前端点：<code>{draft.hostname || '未填写'}:{String(draft.port)}</code></p>
          <p className="muted">已有本机 SSH known_hosts 记录会直接复用；只有没有记录或指纹变化时，才需要扫描并通过独立可信渠道核对。</p>
          {sshTrustWarnings.length > 0 && <details className="advanced-settings"><summary>本机 SSH 配置提示</summary><div className="muted">{sshTrustWarnings.map((warning) => <p key={warning}>{warning}</p>)}</div></details>}
          {selectedJump && <p className="muted">目标候选将由已信任跳板“{selectedJump.alias}”扫描；下方显示和接受的仍是目标 {draft.hostname}:{String(draft.port)} 的密钥，不是跳板密钥。</p>}
          {!draft.id
            ? <p className="inline-message">请先保存当前主机资料，再读取本地信任或测试连接。</p>
            : !savedConnectionMatchesDraft
              ? <p className="inline-message">当前表单有未保存的连接修改；请先保存后再扫描或测试。</p>
              : trustedKeysState === 'loading'
                ? <p className="muted">正在读取本机 SSH 信任记录……</p>
                : trustedKeysState === 'error'
                  ? <p className="inline-message">读取本机 SSH 信任记录失败：{trustedKeysError ?? '未知错误'}。可以稍后重新读取。</p>
                  : endpointTrusted
                    ? <p className="inline-message">当前端点已有本地信任记录，可以直接测试连接或打开终端。</p>
                    : <p className="inline-message">当前端点还没有本地信任记录。先扫描指纹，核对后接受；认证私钥不会自动替代这一步。</p>}
          <div className="button-row wrap">
            <button disabled={!draft.id || !savedConnectionMatchesDraft || actionBusy} onClick={() => { void scan() }}><RefreshCw size={14} />扫描指纹</button>
            <button disabled={!draft.id || !savedConnectionMatchesDraft || actionBusy} onClick={() => { void testConnection() }}><Check size={14} />测试连接</button>
          </div>
          {candidates.map((candidate) => (
            <article className={candidate.mismatch ? 'fingerprint-card mismatch' : 'fingerprint-card'} key={`${candidate.algorithm}-${candidate.sha256Fingerprint}`}>
              <div><strong>{candidate.algorithm}</strong><span>{candidate.trusted ? '已信任' : candidate.mismatch ? '指纹已变化' : '待确认'}</span></div>
              {candidate.previousFingerprint && <code>旧：{candidate.previousFingerprint}</code>}
              <code>新：{candidate.sha256Fingerprint}</code>
              {candidate.mismatch
                ? <p><ShieldAlert size={14} />连接已阻断。请独立核验后，在下方删除旧信任，再重新扫描。</p>
                : !candidate.trusted && <button className="primary" disabled={actionBusy} onClick={() => { void accept(candidate) }}>接受并保存</button>}
            </article>
          ))}
          {testResult && <div className={testResult.success ? 'result-card success' : 'result-card failed'}><strong>{testResult.success ? `连接成功 · ${String(testResult.latencyMs)} ms` : '连接失败'}</strong><small>实际测试时间：{testedAt ?? '未知'}</small>{connectionHint && <p className="inline-message">{connectionHint}</p>}<pre>{testResult.serverLine ?? testResult.error ?? ''}</pre></div>}
        </section>

        <section className="card key-tools">
          <div className="card-title"><KeyRound size={17} /><h2>Ed25519 密钥工具</h2></div>
          <label><span>私钥路径</span><div className="inline-form path-picker"><input list="private-key-options" value={keyPath} onChange={(event) => setKeyPath(event.target.value)} placeholder={'C:\\Users\\name\\.ssh\\id_ed25519'} /><button disabled={actionBusy} onClick={() => { void chooseExistingKey() }}><FolderOpen size={14} />已有私钥</button><button disabled={actionBusy} onClick={() => { void chooseNewKeyPath() }}><FileKey size={14} />新私钥路径</button></div></label>
          <datalist id="private-key-options">{privateKeys.map((key) => <option key={key.path} value={key.path}>{key.fingerprint ?? key.algorithm}</option>)}</datalist>
          <label><span>注释</span><input value={keyComment} onChange={(event) => setKeyComment(event.target.value)} /></label>
          <div className="button-row wrap">
            <button disabled={actionBusy} onClick={() => { void generateKey() }}><FileKey size={14} />生成并校验</button>
            <button className="primary" disabled={!draft.id || actionBusy} onClick={() => { void deployKey() }}><Upload size={14} />部署、复验并设为默认</button>
          </div>
          {privateKeys.length > 0 && <div className="key-list">{privateKeys.map((key) => <button key={key.path} className={key.path === keyPath ? 'selected' : ''} onClick={() => setKeyPath(key.path)}><strong>{key.algorithm}</strong><span>{key.path}</span><code>{key.fingerprint ?? (key.encrypted ? '已加密' : '等待解析')}</code></button>)}</div>}
        </section>

        <section className="card import-card">
          <div className="card-title"><Upload size={17} /><h2>导入 OpenSSH config</h2></div>
          <p className="muted">导入明确支持的 Host、HostName、User、Port、IdentityFile、ProxyJump 与保活选项；已有本机 SSH known_hosts 的匹配记录会直接复用，新端点或指纹变化时才需要扫描核对。ProxyJump 必须能映射到同批或已保存的具体主机。</p>
          <div className="inline-form path-picker"><input value={importPath} onChange={(event) => setImportPath(event.target.value)} placeholder="C:\\Users\\name\\.ssh\\config" /><button disabled={actionBusy} onClick={() => { void chooseImportConfig() }}><FolderOpen size={14} />选择文件</button><button disabled={actionBusy} onClick={() => { void importConfig() }}>导入</button></div>
        </section>

        <section className="card trusted-list-card">
          <div className="card-title"><ShieldAlert size={17} /><h2>已信任指纹</h2></div>
          {trustedKeysState === 'loading'
            ? <p className="muted">正在读取本机 SSH 信任记录……</p>
            : trustedKeysState === 'error'
              ? <><p className="inline-message">读取失败：{trustedKeysError ?? '未知错误'}</p><button disabled={actionBusy} onClick={() => { void refreshTrustedKeys() }}><RefreshCw size={14} />重新读取</button></>
              : trustedKeys.length === 0
                ? <p className="muted">暂无信任记录。保存或导入主机后，匹配的本机 SSH known_hosts 会显示在这里。</p>
                : <div className="trusted-list">{trustedKeys.map((record) => <article key={record.id}><span><strong>{record.hostname}:{String(record.port)}</strong><code>{record.algorithm} · {record.sha256Fingerprint}</code></span><button className="danger" disabled={actionBusy} onClick={() => { void run(async () => { if (!window.confirm(`删除 ${record.hostname} 的信任记录？`)) return; await api.removeHostKey(record.id); await refreshTrustedKeys(); setCandidates([]); setTestResult(null); setTestedAt(null); setMessage('信任记录已删除。') }) }}><Trash2 size={13} />删除</button></article>)}</div>}
        </section>
      </div>
      {message && <p className="inline-message" role="status">{message}</p>}
    </section>
  )
}
