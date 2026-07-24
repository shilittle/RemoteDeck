import { EventEmitter } from 'node:events'
import { createHash } from 'node:crypto'
import { request as httpRequest } from 'node:http'
import { connect as connectTcp, createServer } from 'node:net'
import type { Server as NetServer, Socket } from 'node:net'
import type { Logger } from 'pino'
import type { Client as SshClient } from 'ssh2'
import type { TunnelProfile } from '../../protocol/domain'
import type { ConnectionCredentials } from '../../protocol/ssh'
import type { TunnelCreateRequest, TunnelSnapshot, TunnelUpdateRequest } from '../../protocol/tunnel'
import type { ProfileRepository } from '../hosts/profile-repository'
import type { DedicatedSshConnection, SshConnectionManager } from '../ssh/connection-manager'

interface RuntimeEntry {
  profile: TunnelProfile
  state: TunnelSnapshot['state']
  health: TunnelSnapshot['health']
  active: boolean
  paused: boolean
  credentials: ConnectionCredentials
  connection?: DedicatedSshConnection
  listener?: NetServer
  localSockets?: Set<Socket>
  startedAt?: string
  reconnectCount: number
  nextRetryAt?: string
  lastError?: string
  retryTimer?: ReturnType<typeof setTimeout>
  healthTimer?: ReturnType<typeof setInterval>
  logs: TunnelSnapshot['logs']
}

export class TunnelService extends EventEmitter {
  readonly #profiles: ProfileRepository
  readonly #connections: SshConnectionManager
  readonly #logger: Logger
  readonly #runtime = new Map<string, RuntimeEntry>()

  constructor(profiles: ProfileRepository, connections: SshConnectionManager, logger: Logger) { super(); this.#profiles = profiles; this.#connections = connections; this.#logger = logger }

  async list(hostId?: string): Promise<TunnelSnapshot[]> {
    const profiles = (await this.#profiles.listTunnels()).filter((profile) => !hostId || profile.hostId === hostId)
    return profiles.map((profile) => this.#snapshot(this.#entry(profile)))
  }

  async create(request: TunnelCreateRequest): Promise<TunnelSnapshot> { const profile = await this.#profiles.addTunnel(request); return this.#snapshot(this.#entry(profile)) }

  async update(request: TunnelUpdateRequest): Promise<TunnelSnapshot> {
    const current = this.#runtime.get(request.tunnelId)
    if (current?.active) await this.stop(request.tunnelId)
    const patch = Object.fromEntries(Object.entries(request.patch).filter(([, value]) => value !== undefined)) as Parameters<ProfileRepository['updateTunnel']>[1]
    const profile = await this.#profiles.updateTunnel(request.tunnelId, patch)
    const entry = this.#entry(profile)
    entry.profile = profile
    return this.#emit(entry)
  }

  async delete(tunnelId: string): Promise<boolean> { if (this.#runtime.has(tunnelId)) await this.stop(tunnelId); this.#runtime.delete(tunnelId); return await this.#profiles.deleteTunnel(tunnelId) }

  async start(tunnelId: string, credentials: ConnectionCredentials): Promise<TunnelSnapshot> {
    const profile = (await this.#profiles.listTunnels()).find((item) => item.id === tunnelId)
    if (!profile) throw new Error(`Unknown tunnel profile: ${tunnelId}`)
    const entry = this.#entry(profile)
    if (entry.state === 'online') return this.#snapshot(entry)
    clearSecrets(entry.credentials)
    entry.credentials = structuredClone(credentials)
    entry.active = true
    entry.paused = false
    entry.reconnectCount = 0
    await this.#connect(entry)
    return this.#snapshot(entry)
  }

  async stop(tunnelId: string): Promise<TunnelSnapshot> {
    const entry = this.#runtime.get(tunnelId)
    if (!entry) { const profile = (await this.#profiles.listTunnels()).find((item) => item.id === tunnelId); if (!profile) throw new Error(`Unknown tunnel profile: ${tunnelId}`); return this.#snapshot(this.#entry(profile)) }
    entry.active = false; entry.paused = false
    this.#clearTimers(entry)
    await this.#closeResources(entry)
    clearSecrets(entry.credentials)
    entry.credentials = {}
    entry.state = 'stopped'; entry.health = 'unknown'; delete entry.startedAt; delete entry.nextRetryAt
    this.#log(entry, 'info', 'Tunnel stopped; only RemoteDeck-owned resources were closed.')
    return this.#emit(entry)
  }

  async restart(tunnelId: string, credentials: ConnectionCredentials): Promise<TunnelSnapshot> { await this.stop(tunnelId); return await this.start(tunnelId, credentials) }
  async restoreAutoStart(): Promise<void> { for (const profile of await this.#profiles.listTunnels()) if (profile.autoStart) void this.start(profile.id, {}).catch((error: unknown) => this.#logger.warn({ tunnelId: profile.id, error }, 'Auto-start tunnel failed')) }
  async stopAll(): Promise<void> { await Promise.all([...this.#runtime.keys()].map((id) => this.stop(id))) }

  async suspend(): Promise<void> {
    for (const entry of this.#runtime.values()) if (entry.active) { entry.paused = true; this.#clearTimers(entry); await this.#closeResources(entry); entry.state = 'waiting'; entry.health = 'unknown'; this.#log(entry, 'info', 'Windows suspended; tunnel resources released until resume.'); this.#emit(entry) }
  }

  resume(): void { for (const entry of this.#runtime.values()) if (entry.active && entry.paused) { entry.paused = false; this.#log(entry, 'info', 'Windows resumed; reconnecting tunnel.'); void this.#connect(entry) } }

  async #connect(entry: RuntimeEntry): Promise<void> {
    if (!entry.active || entry.paused) return
    this.#clearTimers(entry)
    await this.#closeResources(entry)
    entry.state = 'starting'; entry.health = 'unknown'; delete entry.lastError; delete entry.nextRetryAt
    this.#emit(entry)
    try {
      if (entry.profile.direction === 'local') await preflightPort(entry.profile.bindAddress, entry.profile.sourcePort)
      const connection = await this.#connections.openDedicated(entry.profile.hostId, structuredClone(entry.credentials))
      entry.connection = connection
      connection.client.once('close', () => { if (entry.active && !entry.paused && entry.connection === connection) { delete entry.connection; this.#scheduleReconnect(entry, new Error('Dedicated SSH connection closed')) } })
      if (entry.profile.direction === 'local') {
        const local = await startLocalForward(connection.client, entry.profile)
        entry.listener = local.listener
        entry.localSockets = local.sockets
      }
      else await this.#startRemoteForward(connection.client, entry)
      entry.state = 'online'; entry.health = 'unknown'; entry.startedAt = new Date().toISOString(); delete entry.lastError
      this.#log(entry, 'info', `${entry.profile.direction === 'local' ? 'LocalForward' : 'RemoteForward'} active on ${entry.profile.bindAddress}:${String(entry.profile.sourcePort)}.`)
      this.#startHealth(entry)
      this.#emit(entry)
    } catch (error) {
      await this.#closeResources(entry)
      const message = messageOf(error)
      entry.lastError = message
      if (/password or keyboard-interactive answer is required|passphrase|authentication/i.test(message) && Object.keys(entry.credentials).length === 0) {
        entry.state = 'failed'; this.#log(entry, 'error', `Tunnel requires in-memory credentials: ${message}`); this.#emit(entry)
      } else this.#scheduleReconnect(entry, error)
    }
  }

  async #startRemoteForward(client: SshClient, entry: RuntimeEntry): Promise<void> {
    const handler = (details: { destIP: string; destPort: number }, accept: () => NodeJS.ReadWriteStream, reject: () => void): void => {
      if (details.destPort !== entry.profile.sourcePort) { reject(); return }
      const target = connectTcp(entry.profile.targetPort, entry.profile.targetHost)
      target.once('connect', () => { const channel = accept(); target.pipe(channel).pipe(target) })
      target.once('error', () => { target.destroy(); reject() })
    }
    client.on('tcp connection', handler)
    try { await forwardIn(client, entry.profile.bindAddress, entry.profile.sourcePort) }
    catch (error) {
      if (!entry.profile.legacyCleanupHook?.authorized) throw error
      this.#logger.warn({ tunnelId: entry.profile.id, commandSha256: createHash('sha256').update(entry.profile.legacyCleanupHook.command).digest('hex') }, 'Executing explicitly authorized legacy tunnel cleanup hook')
      this.#log(entry, 'warn', 'Executing the explicitly authorized legacy cleanup hook; the command is omitted from runtime logs.')
      await execRemote(client, entry.profile.legacyCleanupHook.command)
      await forwardIn(client, entry.profile.bindAddress, entry.profile.sourcePort)
    }
  }

  #scheduleReconnect(entry: RuntimeEntry, error: unknown): void {
    if (!entry.active || entry.paused) return
    entry.reconnectCount += 1
    const baseDelay = Math.min(30_000, 1000 * 2 ** Math.min(5, entry.reconnectCount - 1))
    const delay = Math.min(30_000, Math.round(baseDelay * (1 + Math.random() * 0.2)))
    entry.state = 'waiting'; entry.health = 'unhealthy'; entry.lastError = messageOf(error); entry.nextRetryAt = new Date(Date.now() + delay).toISOString()
    this.#log(entry, 'warn', `Tunnel disconnected; retry ${String(entry.reconnectCount)} in ${String(delay)} ms: ${entry.lastError}`)
    this.#emit(entry)
    entry.retryTimer = setTimeout(() => { delete entry.retryTimer; void this.#connect(entry) }, delay)
  }

  #startHealth(entry: RuntimeEntry): void {
    const health = entry.profile.healthCheck
    if (!health) return
    const run = (): void => {
      if (!entry.active || entry.state !== 'online') return
      entry.health = 'checking'; this.#emit(entry)
      const endpoint = entry.profile.direction === 'local' ? { host: loopbackAddress(entry.profile.bindAddress), port: entry.profile.sourcePort } : { host: entry.profile.targetHost, port: entry.profile.targetPort }
      const check = health.type === 'tcp' ? tcpCheck(endpoint.host, endpoint.port, health.timeoutMs) : httpCheck(endpoint.host, endpoint.port, health.path, health.expectedStatus, health.timeoutMs)
      void check.then((healthy) => { entry.health = healthy ? 'healthy' : 'unhealthy'; if (healthy) delete entry.lastError; else entry.lastError = `${health.type.toUpperCase()} health check failed`; this.#emit(entry) }).catch((error: unknown) => { entry.health = 'unhealthy'; entry.lastError = messageOf(error); this.#emit(entry) })
    }
    run()
    entry.healthTimer = setInterval(run, health.intervalSeconds * 1000)
  }

  async #closeResources(entry: RuntimeEntry): Promise<void> {
    const listener = entry.listener; delete entry.listener
    const sockets = entry.localSockets; delete entry.localSockets
    sockets?.forEach((socket) => socket.destroy())
    if (listener) await new Promise<void>((resolve) => listener.close(() => resolve()))
    const connection = entry.connection; delete entry.connection
    if (connection && entry.profile.direction === 'remote') await unforwardIn(connection.client, entry.profile.bindAddress, entry.profile.sourcePort).catch(() => undefined)
    connection?.close()
  }

  #clearTimers(entry: RuntimeEntry): void { if (entry.retryTimer) clearTimeout(entry.retryTimer); if (entry.healthTimer) clearInterval(entry.healthTimer); delete entry.retryTimer; delete entry.healthTimer }
  #entry(profile: TunnelProfile): RuntimeEntry { const current = this.#runtime.get(profile.id); if (current) { current.profile = profile; return current } const entry: RuntimeEntry = { profile, state: 'stopped', health: 'unknown', active: false, paused: false, credentials: {}, reconnectCount: 0, logs: [] }; this.#runtime.set(profile.id, entry); return entry }
  #snapshot(entry: RuntimeEntry): TunnelSnapshot { return { profile: entry.profile, state: entry.state, health: entry.health, uptimeSeconds: entry.startedAt ? Math.max(0, Math.floor((Date.now() - Date.parse(entry.startedAt)) / 1000)) : 0, reconnectCount: entry.reconnectCount, logs: entry.logs, ...(entry.startedAt ? { startedAt: entry.startedAt } : {}), ...(entry.nextRetryAt ? { nextRetryAt: entry.nextRetryAt } : {}), ...(entry.lastError ? { lastError: entry.lastError } : {}) } }
  #emit(entry: RuntimeEntry): TunnelSnapshot { const snapshot = this.#snapshot(entry); this.emit('event', { snapshot }); return snapshot }
  #log(entry: RuntimeEntry, level: 'info' | 'warn' | 'error', message: string): void { entry.logs = [...entry.logs, { at: new Date().toISOString(), level, message }].slice(-200) }
}

function startLocalForward(client: SshClient, profile: TunnelProfile): Promise<{ listener: NetServer; sockets: Set<Socket> }> {
  const sockets = new Set<Socket>()
  const server = createServer((socket) => {
    sockets.add(socket)
    socket.once('close', () => sockets.delete(socket))
    client.forwardOut(socket.remoteAddress ?? '127.0.0.1', socket.remotePort ?? 0, profile.targetHost, profile.targetPort, (error, stream) => {
      if (error) { socket.destroy(error); return }
      socket.pipe(stream).pipe(socket)
    })
  })
  return new Promise((resolve, reject) => { server.once('error', reject); server.listen(profile.sourcePort, profile.bindAddress, () => { server.off('error', reject); server.on('error', () => undefined); resolve({ listener: server, sockets }) }) })
}
function preflightPort(address: string, port: number): Promise<void> {
  const server = createServer()
  return new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(port, address, () => server.close((error) => error ? reject(error) : resolve()))
  })
}
function forwardIn(client: SshClient, address: string, port: number): Promise<number> { return new Promise((resolve, reject) => client.forwardIn(address, port, (error, actualPort) => error ? reject(error) : resolve(actualPort))) }
function unforwardIn(client: SshClient, address: string, port: number): Promise<void> { return new Promise((resolve, reject) => client.unforwardIn(address, port, (error) => error ? reject(error) : resolve())) }
function execRemote(client: SshClient, command: string): Promise<void> { return new Promise((resolve, reject) => client.exec(command, (error, stream) => { if (error) { reject(error); return } let stderr = ''; stream.stderr.setEncoding('utf8'); stream.stderr.on('data', (value: string) => { stderr += value }); stream.once('close', (code: number) => code === 0 ? resolve() : reject(new Error(`Legacy cleanup failed (${String(code)}): ${stderr}`))) })) }
function tcpCheck(host: string, port: number, timeoutMs: number): Promise<boolean> { return new Promise((resolve) => { const socket = connectTcp(port, host); const finish = (value: boolean): void => { clearTimeout(timer); socket.destroy(); resolve(value) }; const timer = setTimeout(() => finish(false), timeoutMs); socket.once('connect', () => finish(true)); socket.once('error', () => finish(false)) }) }
function httpCheck(host: string, port: number, path: string, expectedStatus: number, timeoutMs: number): Promise<boolean> { return new Promise((resolve) => { const request = httpRequest({ host, port, path, method: 'GET', timeout: timeoutMs }, (response) => { response.resume(); resolve(response.statusCode === expectedStatus) }); request.once('timeout', () => { request.destroy(); resolve(false) }); request.once('error', () => resolve(false)); request.end() }) }
function loopbackAddress(value: string): string { return value === '0.0.0.0' || value === '::' ? '127.0.0.1' : value }
function messageOf(value: unknown): string { return value instanceof Error ? value.message : String(value) }
function clearSecrets(credentials: ConnectionCredentials): void { if (credentials.password) credentials.password = ''; if (credentials.passphrase) credentials.passphrase = ''; credentials.keyboardInteractiveAnswers?.fill(''); if (credentials.jump) clearSecrets(credentials.jump) }
