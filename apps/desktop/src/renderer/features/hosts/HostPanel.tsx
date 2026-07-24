import { useEffect, useState } from 'react'
import { Check, Copy, KeyRound, Link, Pencil, PlugZap, Plus, RefreshCw, Server, ShieldAlert, Trash2, Unplug, Upload } from 'lucide-react'
import type { AuthProfile } from '../../../protocol/domain'
import type { ConnectionCredentials, ConnectionSnapshot, HostCreateRequest, HostImportResult, HostListItem, PrivateKeyMetadata } from '../../../protocol/ssh'
import { useHostStore } from '../../host-store'
import { useAppStore } from '../../store'

const defaultAdvanced = { connectTimeoutSeconds: 15, serverAliveIntervalSeconds: 30, serverAliveCountMax: 3, tcpKeepAlive: true, compression: false, identitiesOnly: false }

export function HostPanel(): React.JSX.Element {
  const { items, selectedId, connection, applyConnection, load } = useHostStore()
  const selected = items.find((item) => item.host.id === selectedId)
  const [showCreate, setShowCreate] = useState(items.length === 0)
  const [editing, setEditing] = useState<HostListItem | null>(null)
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState('')
  const [password, setPassword] = useState('')
  const [passphrase, setPassphrase] = useState('')
  const [jumpPassword, setJumpPassword] = useState('')
  const [jumpPassphrase, setJumpPassphrase] = useState('')
  const [importPath, setImportPath] = useState('~/.ssh/config')
  const [keyPath, setKeyPath] = useState('~/.ssh/id_ed25519_remotedeck')
  const [trustedKeys, setTrustedKeys] = useState<Awaited<ReturnType<typeof window.remoteDeck.hostKeys.list>>>([])
  const [scannedKeys, setScannedKeys] = useState<PrivateKeyMetadata[]>([])
  const [unsupported, setUnsupported] = useState<HostImportResult['unsupported']>([])

  useEffect(() => {
    void Promise.all([window.remoteDeck.hostKeys.list(), window.remoteDeck.keys.listScanned()]).then(([hostKeys, privateKeys]) => {
      setTrustedKeys(hostKeys)
      setScannedKeys(privateKeys)
    })
  }, [])

  function run(action: () => Promise<void>): void {
    setBusy(true); setMessage('')
    void action().catch((error: unknown) => setMessage(error instanceof Error ? error.message : '操作失败')).finally(() => setBusy(false))
  }

  async function importConfig(): Promise<void> {
    const result = await window.remoteDeck.hosts.import({ configPath: importPath })
    await load()
    setUnsupported(result.unsupported)
    setMessage(result.duplicate ? '该配置内容已经导入，无重复写入。' : `已导入 ${String(result.imported.length)} 台；跳过 ${String(result.skippedAliases.length)} 台；${String(result.unsupported.length)} 台含只读原始指令。`)
  }

  const jumpProfile = selected?.host.jumpHostId ? items.find((item) => item.host.id === selected.host.jumpHostId) : undefined

  if (showCreate || editing) return <CreateHostForm hosts={items} {...(editing ? { initial: editing } : {})} busy={busy} onCancel={() => { setShowCreate(false); setEditing(null) }} onCreate={(request) => run(async () => { const { jumpHostId, ...base } = request; const saved = editing ? await window.remoteDeck.hosts.update({ id: editing.host.id, patch: { ...base, ...(jumpHostId !== undefined ? { jumpHostId } : {}) } }) : await window.remoteDeck.hosts.create({ ...base, ...(jumpHostId ? { jumpHostId } : {}) }); await load(); useHostStore.getState().select(saved.host.id); setShowCreate(false); setEditing(null); setMessage('主机已保存，并已更新 RemoteDeck 托管的 OpenSSH config。') })} />

  return (
    <section className="host-panel">
      <div className="panel-heading host-heading">
        <div><h1>{selected?.host.alias ?? '主机'}</h1><p>{selected ? `${selected.host.username}@${selected.host.hostname}:${String(selected.host.port)}` : '添加或导入 Linux SSH 主机。'}</p></div>
        <div className="button-row"><button className="secondary" onClick={() => setShowCreate(true)}><Plus size={15} />添加主机</button>{selected && <button className="secondary" onClick={() => setEditing(selected)}><Pencil size={15} />编辑</button>}{selected && <button className="secondary" disabled={busy} onClick={() => run(async () => { const alias = nextCopyAlias(selected.host.alias, items); const copy = await window.remoteDeck.hosts.create({ alias, hostname: selected.host.hostname, port: selected.host.port, username: selected.host.username, groups: selected.host.groups, monitorEnabled: selected.host.monitorEnabled, ...(selected.workspace ? { workspacePath: selected.workspace.remotePath } : {}), auth: { name: `${alias} 认证`, method: selected.auth.method, ...(selected.auth.identityFile ? { identityFile: selected.auth.identityFile } : {}), ...(selected.auth.agent ? { agent: selected.auth.agent } : {}) }, advanced: selected.host.advanced }); await load(); useHostStore.getState().select(copy.host.id); setMessage(`已复制为 ${alias}。`) })}><Copy size={15} />复制</button>}<button className="secondary" disabled={busy} onClick={() => { void load() }}><RefreshCw size={15} />刷新</button></div>
      </div>

      {message && <div className="notice">{message}</div>}
      {unsupported.length > 0 && <details className="card unsupported-config" open><summary>SSH config 中保留的只读原始指令（{String(unsupported.length)} 台）</summary>{unsupported.map((entry) => <div key={entry.alias}><strong>{entry.alias}</strong><pre>{entry.lines.join('\n')}</pre></div>)}</details>}
      {!selected ? (
        <div className="host-import-card"><h2>导入 OpenSSH config</h2><p>支持 Host、HostName、User、Port、IdentityFile、ProxyJump、保活、压缩和转发指令；不支持的原始行会明确列出。</p><div className="inline-form"><input aria-label="SSH config 路径" value={importPath} onChange={(event) => setImportPath(event.target.value)} /><button className="primary" disabled={busy} onClick={() => run(importConfig)}><Upload size={15} />导入</button></div></div>
      ) : (
        <div className="host-content-grid">
          <section className="card">
            <div className="card-title"><PlugZap size={17} /><h2>连接与认证</h2><StateBadge state={connection?.state ?? selected.state} /></div>
            <p className="muted">{authDescription(selected.auth)}</p>
            {(selected.auth.method === 'password' || selected.auth.method === 'keyboard_interactive') && <label><span>密码 / 交互回答</span><input type="password" autoComplete="off" value={password} onChange={(event) => setPassword(event.target.value)} /></label>}
            {selected.auth.method === 'private_key' && <label><span>私钥口令（如有）</span><input type="password" autoComplete="off" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} /></label>}
            {jumpProfile && <p className="muted">经由单级跳板 {jumpProfile.host.alias}（{authDescription(jumpProfile.auth)}）</p>}
            {jumpProfile && (jumpProfile.auth.method === 'password' || jumpProfile.auth.method === 'keyboard_interactive') && <label><span>跳板密码 / 交互回答</span><input type="password" autoComplete="off" value={jumpPassword} onChange={(event) => setJumpPassword(event.target.value)} /></label>}
            {jumpProfile?.auth.method === 'private_key' && <label><span>跳板私钥口令（如有）</span><input type="password" autoComplete="off" value={jumpPassphrase} onChange={(event) => setJumpPassphrase(event.target.value)} /></label>}
            <div className="button-row wrap">
              <button className="primary" disabled={busy} onClick={() => run(async () => { try { const snapshot = await window.remoteDeck.hosts.connect({ hostId: selected.host.id, credentials: credentials(selected.auth, password, passphrase, jumpProfile?.auth, jumpPassword, jumpPassphrase) }); applyConnection(snapshot); if (snapshot.state === 'online') useAppStore.getState().setActivity('terminal') } finally { clearSecretInputs(setPassword, setPassphrase, setJumpPassword, setJumpPassphrase) } })}><Link size={15} />连接</button>
              <button className="secondary" disabled={busy || (connection?.state ?? selected.state) !== 'online'} onClick={() => useAppStore.getState().setActivity('terminal')}><Server size={15} />打开终端</button>
              <button className="secondary" disabled={busy} onClick={() => run(async () => { try { const snapshot = await window.remoteDeck.hosts.test({ hostId: selected.host.id, credentials: credentials(selected.auth, password, passphrase, jumpProfile?.auth, jumpPassword, jumpPassphrase) }); applyConnection(snapshot); setMessage(snapshot.capabilities ? capabilityText(snapshot.capabilities) : snapshot.errorMessage ?? '测试完成') } finally { clearSecretInputs(setPassword, setPassphrase, setJumpPassword, setJumpPassphrase) } })}><Check size={15} />测试能力</button>
              <button className="secondary" disabled={busy} onClick={() => run(async () => { applyConnection(await window.remoteDeck.hosts.disconnect(selected.host.id)) })}><Unplug size={15} />断开</button>
              <button className="danger" disabled={busy} onClick={() => run(async () => { if (!window.confirm(`删除主机 ${selected.host.alias}？`)) return; await window.remoteDeck.hosts.delete({ hostId: selected.host.id, removeManagedConfig: true }); await load() })}><Trash2 size={15} />删除</button>
            </div>
            {connection?.errorMessage && <p className="error-text">{connection.errorMessage}</p>}
          </section>

          {connection?.hostKeyCandidate && <HostKeyCard candidate={connection.hostKeyCandidate} busy={busy} onAccept={() => run(async () => { await window.remoteDeck.hostKeys.accept(connection.hostKeyCandidate?.id ?? ''); setTrustedKeys(await window.remoteDeck.hostKeys.list()); setMessage('主机指纹已保存。请再次点击连接完成认证。'); applyConnection({ hostId: selected.host.id, generation: connection.generation, state: 'idle' }) })} onReject={() => run(async () => { await window.remoteDeck.hostKeys.reject(connection.hostKeyCandidate?.id ?? ''); applyConnection({ hostId: selected.host.id, generation: connection.generation, state: 'idle' }); setMessage('未信任该主机指纹。') })} />}

          <section className="card">
            <div className="card-title"><KeyRound size={17} /><h2>Ed25519 密钥闭环</h2></div>
            <label><span>新私钥路径</span><input value={keyPath} onChange={(event) => setKeyPath(event.target.value)} /></label>
            <div className="button-row wrap"><button className="secondary" disabled={busy} onClick={() => run(async () => { const results = await window.remoteDeck.keys.pickAndScan(); if (results[0]) setKeyPath(results[0].path); setScannedKeys(await window.remoteDeck.keys.listScanned()); setMessage(results.length > 0 ? `已扫描 ${String(results.length)} 个私钥文件；仅路径与元信息进入配置。` : '未选择私钥。') })}><KeyRound size={15} />选择并扫描已有私钥</button></div>
            {scannedKeys.length > 0 && <div className="scanned-key-list">{scannedKeys.map((key) => <button className={key.path === keyPath ? 'scanned-key selected' : 'scanned-key'} key={key.path} onClick={() => setKeyPath(key.path)}><strong>{key.algorithm ?? key.format}</strong><span>{key.path}</span><code>{key.fingerprint ?? (key.encrypted ? '已加密，需口令解析' : '无公钥指纹')}</code></button>)}</div>}
            <label><span>新密钥口令（可选）</span><input type="password" value={passphrase} onChange={(event) => setPassphrase(event.target.value)} /></label>
            <div className="button-row wrap"><button className="secondary" disabled={busy} onClick={() => run(async () => { try { const result = await window.remoteDeck.keys.generate({ privateKeyPath: keyPath, comment: `RemoteDeck ${selected.host.alias}`, ...(passphrase ? { passphrase } : {}) }); setMessage(`已生成 ${result.fingerprint}；私钥 ACL ${result.aclRestricted ? '已收紧' : '需人工复核'}。`) } finally { setPassphrase('') } })}><Plus size={15} />生成密钥</button><button className="primary" disabled={busy || connection?.state !== 'online'} onClick={() => run(async () => { try { const result = await window.remoteDeck.keys.deploy({ hostId: selected.host.id, privateKeyPath: keyPath, ...(passphrase ? { passphrase } : {}), makeDefault: true }); await load(); setMessage(result.message) } finally { setPassphrase('') } })}><Upload size={15} />部署、复验并设为默认</button></div>
          </section>

          <section className="card host-import-card"><h2>导入另一份 config</h2><div className="inline-form"><input aria-label="另一份 SSH config 路径" value={importPath} onChange={(event) => setImportPath(event.target.value)} /><button className="secondary" disabled={busy} onClick={() => run(importConfig)}><Upload size={15} />导入</button></div></section>
          <section className="card host-import-card"><h2>已信任主机指纹</h2>{trustedKeys.length === 0 ? <p>暂无已保存指纹。</p> : <div className="trusted-list">{trustedKeys.map((record) => <div key={record.id}><span><strong>{record.host}:{String(record.port)}</strong><code>{record.sha256Fingerprint}</code></span><button className="danger" disabled={busy} onClick={() => run(async () => { if (!window.confirm(`删除 ${record.host} 的已信任指纹？`)) return; await window.remoteDeck.hostKeys.remove(record.id); setTrustedKeys(await window.remoteDeck.hostKeys.list()); setMessage('信任记录已删除；下次连接必须重新确认指纹。') })}><Trash2 size={14} />删除信任</button></div>)}</div>}</section>
        </div>
      )}
    </section>
  )
}

type HostFormValue = Omit<HostCreateRequest, 'jumpHostId'> & { jumpHostId?: string | null }

function CreateHostForm({ hosts, initial, busy, onCancel, onCreate }: { hosts: HostListItem[]; initial?: HostListItem; busy: boolean; onCancel: () => void; onCreate: (request: HostFormValue) => void }): React.JSX.Element {
  const [alias, setAlias] = useState(initial?.host.alias ?? '')
  const [hostname, setHostname] = useState(initial?.host.hostname ?? '')
  const [port, setPort] = useState(initial?.host.port ?? 22)
  const [username, setUsername] = useState(initial?.host.username ?? '')
  const [workspacePath, setWorkspacePath] = useState(initial?.workspace?.remotePath ?? '~')
  const [method, setMethod] = useState<AuthProfile['method']>(initial?.auth.method ?? 'password')
  const [identityFile, setIdentityFile] = useState(initial?.auth.identityFile ?? '')
  const [agent, setAgent] = useState<'windows_openssh' | 'pageant'>(initial?.auth.agent ?? 'windows_openssh')
  const [groups, setGroups] = useState(initial?.host.groups.join(', ') ?? '')
  const [jumpHostId, setJumpHostId] = useState(initial?.host.jumpHostId ?? '')
  const [connectTimeoutSeconds, setConnectTimeoutSeconds] = useState(initial?.host.advanced.connectTimeoutSeconds ?? defaultAdvanced.connectTimeoutSeconds)
  const [serverAliveIntervalSeconds, setServerAliveIntervalSeconds] = useState(initial?.host.advanced.serverAliveIntervalSeconds ?? defaultAdvanced.serverAliveIntervalSeconds)
  const [serverAliveCountMax, setServerAliveCountMax] = useState(initial?.host.advanced.serverAliveCountMax ?? defaultAdvanced.serverAliveCountMax)
  const [tcpKeepAlive, setTcpKeepAlive] = useState(initial?.host.advanced.tcpKeepAlive ?? defaultAdvanced.tcpKeepAlive)
  const [compression, setCompression] = useState(initial?.host.advanced.compression ?? defaultAdvanced.compression)
  const [identitiesOnly, setIdentitiesOnly] = useState(initial?.host.advanced.identitiesOnly ?? defaultAdvanced.identitiesOnly)
  const [monitorEnabled, setMonitorEnabled] = useState(initial?.host.monitorEnabled ?? true)
  const [scanning, setScanning] = useState(false)
  const valid = alias.length > 0 && hostname.length > 0 && username.length > 0 && Number.isInteger(port) && port > 0 && port <= 65_535 && (method !== 'private_key' || identityFile.length > 0)
  const jumpCandidates = hosts.filter((item) => item.host.id !== initial?.host.id && !item.host.jumpHostId)
  async function pickPrivateKey(): Promise<void> {
    setScanning(true)
    try {
      const results = await window.remoteDeck.keys.pickAndScan()
      if (results[0]) setIdentityFile(results[0].path)
    } finally {
      setScanning(false)
    }
  }
  return (
    <section className="host-panel create-host">
      <div className="panel-heading"><div><h1>{initial ? '编辑 SSH 主机' : '添加 SSH 主机'}</h1><p>密码与私钥口令不会写入配置。连接参数与客户端保活分别管理。</p></div></div>
      <div className="form-grid">
        <label><span>别名</span><input value={alias} onChange={(event) => setAlias(event.target.value)} placeholder="gpu-workstation" /></label>
        <label><span>地址</span><input value={hostname} onChange={(event) => setHostname(event.target.value)} placeholder="192.0.2.10" /></label>
        <label><span>端口</span><input type="number" min="1" max="65535" value={port} onChange={(event) => setPort(Number(event.target.value))} /></label>
        <label><span>用户名</span><input value={username} onChange={(event) => setUsername(event.target.value)} /></label>
        <label><span>默认远程目录</span><input value={workspacePath} onChange={(event) => setWorkspacePath(event.target.value)} /></label>
        <label><span>分组（逗号分隔）</span><input value={groups} onChange={(event) => setGroups(event.target.value)} /></label>
        <label><span>认证方式</span><select value={method} onChange={(event) => setMethod(event.target.value as AuthProfile['method'])}><option value="password">密码</option><option value="keyboard_interactive">Keyboard-interactive</option><option value="private_key">私钥</option><option value="agent">SSH agent / Pageant</option></select></label>
        {method === 'private_key' && <label className="wide"><span>私钥路径</span><div className="input-with-button"><input value={identityFile} onChange={(event) => setIdentityFile(event.target.value)} /><button className="secondary" type="button" disabled={busy || scanning} onClick={() => { void pickPrivateKey() }}>选择并扫描</button></div></label>}
        {method === 'agent' && <label><span>Agent</span><select value={agent} onChange={(event) => setAgent(event.target.value as typeof agent)}><option value="windows_openssh">Windows OpenSSH agent</option><option value="pageant">Pageant</option></select></label>}
        <label><span>ProxyJump（单级）</span><select value={jumpHostId} onChange={(event) => setJumpHostId(event.target.value)}><option value="">直连</option>{jumpCandidates.map((item) => <option key={item.host.id} value={item.host.id}>{item.host.alias}</option>)}</select></label>
      </div>
      <fieldset className="advanced-fields"><legend>客户端保活</legend><div className="form-grid"><label><span>ServerAliveInterval（秒）</span><input type="number" min="0" value={serverAliveIntervalSeconds} onChange={(event) => setServerAliveIntervalSeconds(Number(event.target.value))} /></label><label><span>ServerAliveCountMax</span><input type="number" min="1" value={serverAliveCountMax} onChange={(event) => setServerAliveCountMax(Number(event.target.value))} /></label><label className="check-label"><input type="checkbox" checked={tcpKeepAlive} onChange={(event) => setTcpKeepAlive(event.target.checked)} />TCPKeepAlive</label></div></fieldset>
      <fieldset className="advanced-fields"><legend>连接参数</legend><div className="form-grid"><label><span>ConnectTimeout（秒）</span><input type="number" min="1" value={connectTimeoutSeconds} onChange={(event) => setConnectTimeoutSeconds(Number(event.target.value))} /></label><label className="check-label"><input type="checkbox" checked={compression} onChange={(event) => setCompression(event.target.checked)} />Compression</label><label className="check-label"><input type="checkbox" checked={identitiesOnly} onChange={(event) => setIdentitiesOnly(event.target.checked)} />IdentitiesOnly</label><label className="check-label"><input type="checkbox" checked={monitorEnabled} onChange={(event) => setMonitorEnabled(event.target.checked)} />连接后自动启动结构化监控</label></div></fieldset>
      <div className="button-row"><button className="primary" disabled={!valid || busy || scanning} onClick={() => { onCreate({ alias, hostname, port, username, groups: groups.split(',').map((value) => value.trim()).filter(Boolean), workspacePath, monitorEnabled, ...(jumpHostId ? { jumpHostId } : initial?.host.jumpHostId ? { jumpHostId: null } : {}), auth: { name: `${alias} 认证`, method, ...(method === 'private_key' ? { identityFile } : {}), ...(method === 'agent' ? { agent } : {}) }, advanced: { connectTimeoutSeconds, serverAliveIntervalSeconds, serverAliveCountMax, tcpKeepAlive, compression, identitiesOnly } }) }}><Server size={15} />保存主机</button><button className="secondary" onClick={onCancel}>取消</button></div>
    </section>
  )
}

function HostKeyCard({ candidate, busy, onAccept, onReject }: { candidate: NonNullable<ConnectionSnapshot['hostKeyCandidate']>; busy: boolean; onAccept: () => void; onReject: () => void }): React.JSX.Element {
  return <section className={candidate.mismatch ? 'card host-key danger-card' : 'card host-key'}><div className="card-title"><ShieldAlert size={18} /><h2>{candidate.mismatch ? '主机指纹已变化：连接被阻断' : '首次连接：确认主机指纹'}</h2></div>{candidate.previousFingerprint && <div className="fingerprint"><span>旧指纹</span><code>{candidate.previousFingerprint}</code></div>}<div className="fingerprint"><span>新指纹</span><code>{candidate.sha256Fingerprint}</code></div><p className="muted">算法 {candidate.algorithm} · {candidate.host}:{String(candidate.port)}</p><div className="button-row">{!candidate.mismatch && <button className="primary" disabled={busy} onClick={onAccept}>接受并保存</button>}<button className="secondary" disabled={busy} onClick={onReject}>拒绝</button></div>{candidate.mismatch && <p className="error-text">RemoteDeck 不允许从此处覆盖旧信任。请核验服务器后，在信任记录中明确删除旧指纹，再重新连接。</p>}</section>
}

function StateBadge({ state }: { state: string }): React.JSX.Element { return <span className={`state-badge state-${state}`}>{state}</span> }
function authDescription(auth: AuthProfile): string { return auth.method === 'private_key' ? `私钥：${auth.identityFile ?? '未设置'}` : auth.method === 'agent' ? `Agent：${auth.agent === 'pageant' ? 'Pageant' : 'Windows OpenSSH'}` : auth.method === 'keyboard_interactive' ? 'Keyboard-interactive 回答仅在当前连接期间使用。' : '密码仅在当前连接期间使用。' }
function credentials(auth: AuthProfile, password: string, passphrase: string, jumpAuth?: AuthProfile, jumpPassword = '', jumpPassphrase = ''): ConnectionCredentials {
  const direct = credentialValues(auth, password, passphrase)
  return jumpAuth ? { ...direct, jump: credentialValues(jumpAuth, jumpPassword, jumpPassphrase) } : direct
}
function credentialValues(auth: AuthProfile, password: string, passphrase: string): Omit<ConnectionCredentials, 'jump'> {
  return auth.method === 'keyboard_interactive' ? { keyboardInteractiveAnswers: [password], ...(password ? { password } : {}) } : auth.method === 'password' ? { password } : passphrase ? { passphrase } : {}
}
function clearSecretInputs(...setters: Array<(value: string) => void>): void { for (const setter of setters) setter('') }
function capabilityText(value: { shell: boolean; sftp: boolean; python3: boolean; writableWorkspace: boolean }): string { return `Shell ${value.shell ? '通过' : '失败'} · SFTP ${value.sftp ? '通过' : '失败'} · Python3 ${value.python3 ? '可用' : '缺失'} · 工作目录 ${value.writableWorkspace ? '可写' : '不可写'}` }
function nextCopyAlias(alias: string, items: HostListItem[]): string { const used = new Set(items.map((item) => item.host.alias.toLowerCase())); let index = 1; while (used.has(`${alias}-copy-${String(index)}`.toLowerCase())) index += 1; return `${alias}-copy-${String(index)}` }
