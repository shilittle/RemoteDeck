import { EventEmitter } from 'node:events'
import { readFile } from 'node:fs/promises'
import type { Duplex } from 'node:stream'
import ssh2 from 'ssh2'
import type { Client as SshClient } from 'ssh2'
import type { AuthProfile, ConnectionState, HostKeyRecord } from '../../protocol/domain'
import type { ConnectionCredentials, ConnectionSnapshot, HostKeyCandidate } from '../../protocol/ssh'
import type { ProfileRepository, ResolvedHostProfile } from '../hosts/profile-repository'
import { evaluateHostKey } from './host-key'

interface ConnectionEntry {
  generation: number
  state: ConnectionState
  client?: SshClient
  jumpClient?: SshClient
  lastError?: { code: string; message: string }
  capabilities?: ConnectionSnapshot['capabilities']
}

export interface DedicatedSshConnection {
  client: SshClient
  close: () => void
}

export class SshConnectionManager extends EventEmitter {
  readonly #repository: ProfileRepository
  readonly #connections = new Map<string, ConnectionEntry>()
  readonly #candidates = new Map<string, HostKeyCandidate>()

  constructor(repository: ProfileRepository) {
    super()
    this.#repository = repository
  }

  stateFor(hostId: string): ConnectionState {
    return this.#connections.get(hostId)?.state ?? 'idle'
  }

  snapshot(hostId: string): ConnectionSnapshot {
    const entry = this.#connections.get(hostId) ?? { generation: 0, state: 'idle' as const }
    return {
      hostId,
      generation: entry.generation,
      state: entry.state,
      ...(entry.capabilities ? { capabilities: entry.capabilities } : {}),
      ...(entry.lastError ? { errorCode: entry.lastError.code, errorMessage: entry.lastError.message } : {})
    }
  }

  async connect(hostId: string, credentials: ConnectionCredentials): Promise<ConnectionSnapshot> {
    const profile = await this.#repository.get(hostId)
    const previous = this.#connections.get(hostId)
    this.#dispose(previous)
    const entry: ConnectionEntry = { generation: (previous?.generation ?? 0) + 1, state: 'resolving' }
    this.#connections.set(hostId, entry)
    this.#emit(hostId, entry)
    const generation = entry.generation
    try {
      const records = await this.#repository.listHostKeys()
      let jumpClient: SshClient | undefined
      let socket: Duplex | undefined
      if (profile.host.jumpHostId) {
        const jumpProfile = await this.#repository.get(profile.host.jumpHostId)
        this.#setState(hostId, generation, 'connecting')
        jumpClient = await this.#connectClient(jumpProfile, credentials.jump ?? {}, records)
        socket = await forwardOut(jumpClient, profile.host.hostname, profile.host.port)
        entry.jumpClient = jumpClient
      }
      this.#setState(hostId, generation, 'connecting')
      const client = await this.#connectClient(profile, credentials, records, socket)
      if (!this.#isCurrent(hostId, generation)) { client.end(); jumpClient?.end(); return this.snapshot(hostId) }
      entry.client = client
      if (jumpClient) entry.jumpClient = jumpClient
      this.#setState(hostId, generation, 'online')
      entry.capabilities = await probeCapabilities(client, profile.workspace?.remotePath)
      this.#emit(hostId, entry)
      client.once('close', () => {
        if (!this.#isCurrent(hostId, generation) || entry.state === 'idle') return
        delete entry.client
        entry.state = 'offline'
        this.#emit(hostId, entry)
      })
      return this.snapshot(hostId)
    } catch (error) {
      const candidate = error instanceof HostKeyVerificationError ? error.candidate : [...this.#candidates.values()].find((item) => item.hostId === hostId)
      const normalized = normalizeSshError(error)
      entry.state = candidate && !candidate.mismatch ? 'awaiting_host_key' : 'failed'
      entry.lastError = normalized
      this.#emit(hostId, entry, candidate)
      return { ...this.snapshot(hostId), ...(candidate ? { hostKeyCandidate: candidate } : {}) }
    } finally {
      clearCredentials(credentials)
    }
  }

  async test(hostId: string, credentials: ConnectionCredentials): Promise<ConnectionSnapshot> {
    const result = await this.connect(hostId, credentials)
    if (result.state === 'online') await this.disconnect(hostId)
    return result
  }

  disconnect(hostId: string): Promise<ConnectionSnapshot> {
    const entry = this.#connections.get(hostId)
    if (!entry) return Promise.resolve(this.snapshot(hostId))
    entry.state = 'idle'
    delete entry.capabilities
    delete entry.lastError
    this.#dispose(entry)
    this.#emit(hostId, entry)
    return Promise.resolve(this.snapshot(hostId))
  }

  async disconnectAll(): Promise<void> {
    await Promise.all([...this.#connections.keys()].map((hostId) => this.disconnect(hostId)))
  }

  getOnlineClient(hostId: string): SshClient {
    const entry = this.#connections.get(hostId)
    if (!entry?.client || entry.state !== 'online') throw new Error('Host is not connected')
    return entry.client
  }

  async openDedicated(hostId: string, credentials: ConnectionCredentials): Promise<DedicatedSshConnection> {
    const profile = await this.#repository.get(hostId)
    const records = await this.#repository.listHostKeys()
    let jumpClient: SshClient | undefined
    try {
      let socket: Duplex | undefined
      if (profile.host.jumpHostId) {
        const jumpProfile = await this.#repository.get(profile.host.jumpHostId)
        jumpClient = await this.#connectClient(jumpProfile, credentials.jump ?? {}, records)
        socket = await forwardOut(jumpClient, profile.host.hostname, profile.host.port)
      }
      const client = await this.#connectClient(profile, credentials, records, socket)
      return { client, close: () => { client.end(); jumpClient?.end() } }
    } catch (error) {
      jumpClient?.end()
      throw error
    } finally {
      clearCredentials(credentials)
    }
  }

  getCandidate(candidateId: string): HostKeyCandidate | undefined {
    return this.#candidates.get(candidateId)
  }

  async acceptCandidate(candidateId: string): Promise<HostKeyRecord> {
    const candidate = this.#candidates.get(candidateId)
    if (!candidate) throw new Error('Host-key candidate has expired')
    if (candidate.mismatch) throw new Error('Changed host keys cannot be accepted without removing the old trusted key first')
    const record: HostKeyRecord = {
      schemaVersion: 1,
      id: candidate.id,
      host: candidate.host,
      port: candidate.port,
      algorithm: candidate.algorithm,
      sha256Fingerprint: candidate.sha256Fingerprint,
      publicKeyBase64: candidate.publicKeyBase64,
      acceptedAt: new Date().toISOString()
    }
    await this.#repository.addHostKey(record)
    this.#candidates.delete(candidateId)
    return record
  }

  rejectCandidate(candidateId: string): boolean {
    return this.#candidates.delete(candidateId)
  }

  async verifyPrivateKey(hostId: string, privateKeyPath: string, passphrase?: string): Promise<boolean> {
    const profile = await this.#repository.get(hostId)
    const privateKey = await readFile(privateKeyPath)
    const auth: AuthProfile = { ...profile.auth, method: 'private_key', identityFile: privateKeyPath }
    const credentials = { ...(passphrase ? { passphrase } : {}), privateKey }
    try {
      const client = await this.#connectClient({ ...profile, auth }, credentials, await this.#repository.listHostKeys())
      client.end()
      return true
    } finally {
      clearCredentials(credentials)
    }
  }

  async #connectClient(
    profile: ResolvedHostProfile,
    credentials: ConnectionCredentials & { privateKey?: Buffer },
    records: HostKeyRecord[],
    sock?: Duplex
  ): Promise<SshClient> {
    const { host, auth } = profile
    const client = new ssh2.Client()
    let observedCandidate: HostKeyCandidate | undefined
    const config = {
      host: host.hostname,
      port: host.port,
      username: host.username,
      readyTimeout: host.advanced.connectTimeoutSeconds * 1000,
      keepaliveInterval: host.advanced.serverAliveIntervalSeconds * 1000,
      keepaliveCountMax: host.advanced.serverAliveCountMax,
      compress: host.advanced.compression,
      tryKeyboard: auth.method === 'keyboard_interactive',
      ...(sock ? { sock } : {}),
      ...await authenticationConfig(auth, credentials),
      hostVerifier: (key: Buffer): boolean => {
        const result = evaluateHostKey(host.id, host.hostname, host.port, key, records)
        if (result.candidate) {
          observedCandidate = result.candidate
          this.#candidates.set(result.candidate.id, result.candidate)
        }
        return result.trusted
      }
    }
    if (auth.method === 'keyboard_interactive') {
      client.on('keyboard-interactive', (_name, _instructions, _language, prompts, finish) => {
        const answers = credentials.keyboardInteractiveAnswers ?? []
        finish(prompts.map((_prompt, index) => answers[index] ?? credentials.password ?? ''))
      })
    }
    return await new Promise<SshClient>((resolve, reject) => {
      const timer = setTimeout(() => { client.destroy(); reject(new Error('SSH connection timed out')) }, host.advanced.connectTimeoutSeconds * 1000 + 1000)
      client.once('ready', () => { clearTimeout(timer); resolve(client) })
      client.once('error', (error) => { clearTimeout(timer); reject(observedCandidate ? new HostKeyVerificationError(observedCandidate) : error) })
      client.connect(config)
    })
  }

  #setState(hostId: string, generation: number, state: ConnectionState): void {
    const entry = this.#connections.get(hostId)
    if (!entry || entry.generation !== generation) return
    entry.state = state
    delete entry.lastError
    this.#emit(hostId, entry)
  }

  #isCurrent(hostId: string, generation: number): boolean {
    return this.#connections.get(hostId)?.generation === generation
  }

  #emit(hostId: string, entry: ConnectionEntry, candidate?: HostKeyCandidate): void {
    const snapshot = { ...this.snapshot(hostId), ...(candidate ? { hostKeyCandidate: candidate } : {}) }
    this.emit('state', snapshot)
  }

  #dispose(entry: ConnectionEntry | undefined): void {
    entry?.client?.end()
    entry?.jumpClient?.end()
    if (entry) { delete entry.client; delete entry.jumpClient }
  }
}

class HostKeyVerificationError extends Error {
  readonly candidate: HostKeyCandidate
  constructor(candidate: HostKeyCandidate) {
    super(candidate.mismatch ? `SSH host key changed: ${candidate.previousFingerprint ?? 'unknown'} -> ${candidate.sha256Fingerprint}` : `SSH host key requires acceptance: ${candidate.sha256Fingerprint}`)
    this.name = candidate.mismatch ? 'HostKeyMismatchError' : 'HostKeyUnknownError'
    this.candidate = candidate
  }
}

async function authenticationConfig(auth: AuthProfile, credentials: ConnectionCredentials & { privateKey?: Buffer }): Promise<Record<string, unknown>> {
  if (auth.method === 'password' || auth.method === 'keyboard_interactive') {
    if (!credentials.password && !credentials.keyboardInteractiveAnswers?.length) throw new Error('SSH password or keyboard-interactive answer is required')
    return credentials.password ? { password: credentials.password } : {}
  }
  if (auth.method === 'private_key') {
    if (!auth.identityFile && !credentials.privateKey) throw new Error('Private-key path is missing')
    const privateKey = credentials.privateKey ?? await readFile(auth.identityFile ?? '')
    credentials.privateKey = privateKey
    return { privateKey, ...(credentials.passphrase ? { passphrase: credentials.passphrase } : {}) }
  }
  return { agent: auth.agent === 'pageant' ? 'pageant' : '\\\\.\\pipe\\openssh-ssh-agent' }
}

async function forwardOut(client: SshClient, host: string, port: number): Promise<Duplex> {
  return await new Promise((resolve, reject) => client.forwardOut('127.0.0.1', 0, host, port, (error, stream) => error ? reject(error) : resolve(stream)))
}

async function probeCapabilities(client: SshClient, workspace?: string): Promise<NonNullable<ConnectionSnapshot['capabilities']>> {
  const [shell, sftp, dependency] = await Promise.all([
    new Promise<boolean>((resolve) => client.shell({ term: 'xterm-256color', cols: 80, rows: 24 }, (error, stream) => { if (!error) stream.end(); resolve(!error) })),
    new Promise<boolean>((resolve) => client.sftp((error, channel) => { if (!error) channel.end(); resolve(!error) })),
    execText(client, `command -v python3 >/dev/null 2>&1; py=$?; ${workspace && workspace !== '~' ? `test -w -- ${shellQuote(workspace)}` : 'test -w -- "$HOME"'}; wr=$?; printf '%s %s' "$py" "$wr"`)
  ])
  const [pythonCode, writableCode] = dependency.trim().split(/\s+/).map(Number)
  return { shell, sftp, python3: pythonCode === 0, writableWorkspace: writableCode === 0 }
}

function execText(client: SshClient, command: string): Promise<string> {
  return new Promise((resolve, reject) => client.exec(command, (error, stream) => {
    if (error) { reject(error); return }
    let output = ''
    stream.setEncoding('utf8')
    stream.on('data', (chunk: string) => { output += chunk })
    stream.once('close', () => resolve(output))
    stream.once('error', reject)
  }))
}

function shellQuote(value: string): string { return `'${value.replaceAll("'", `'"'"'`)}'` }
function normalizeSshError(error: unknown): { code: string; message: string } {
  const message = error instanceof Error ? error.message : String(error)
  if (error instanceof HostKeyVerificationError) return { code: error.name === 'HostKeyMismatchError' ? 'HOST_KEY_MISMATCH' : 'HOST_KEY_UNKNOWN', message }
  if (/timed out|ETIMEDOUT/i.test(message)) return { code: 'TCP_TIMEOUT', message: '连接超时；请检查地址、端口、防火墙和网络。' }
  if (/ENOTFOUND|getaddrinfo/i.test(message)) return { code: 'DNS_FAILED', message: 'DNS 解析失败；请检查主机地址。' }
  if (/ECONNREFUSED/i.test(message)) return { code: 'TCP_REFUSED', message: 'SSH 端口拒绝连接；请检查 sshd 和端口。' }
  if (/authentication/i.test(message)) return { code: 'AUTH_FAILED', message: 'SSH 认证失败；请检查认证方式和凭据。' }
  return { code: 'SSH_FAILED', message }
}
function clearCredentials(credentials: ConnectionCredentials & { privateKey?: Buffer }): void {
  if (credentials.password) credentials.password = ''
  if (credentials.passphrase) credentials.passphrase = ''
  credentials.keyboardInteractiveAnswers?.fill('')
  credentials.privateKey?.fill(0)
  if (credentials.jump) clearCredentials(credentials.jump)
}
