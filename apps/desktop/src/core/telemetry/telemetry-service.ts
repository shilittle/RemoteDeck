import { EventEmitter } from 'node:events'
import type { Logger } from 'pino'
import type { Client as SshClient, ClientChannel } from 'ssh2'
import { collectorV1PayloadSchema } from '../../protocol/telemetry'
import type { AppSettings } from '../../protocol/settings'
import type { TelemetrySnapshot } from '../../protocol/domain'
import type { TelemetryEvent, TelemetrySignalRequest, TelemetryStatus } from '../../protocol/telemetry'
import type { SshConnectionManager } from '../ssh/connection-manager'

interface TelemetryEntry {
  hostId: string
  active: boolean
  suspended: boolean
  generation: number
  state: TelemetryStatus['state']
  restartCount: number
  invalidLineCount: number
  sampleCount: number
  intervalSeconds: number
  retentionMinutes: number
  buffer: string
  stderr: string
  history: TelemetrySnapshot[]
  channel?: ClientChannel
  retryTimer?: ReturnType<typeof setTimeout>
  sampleTimer?: ReturnType<typeof setTimeout>
  startedAt?: string
  nextRetryAt?: string
  lastSampleAt?: string
  lastError?: string
}

interface TelemetryServiceOptions {
  restartBaseMs?: number
  timeoutFloorMs?: number
  intervalTimeoutFactorMs?: number
  random?: () => number
}

export class TelemetryService extends EventEmitter {
  readonly #connections: SshConnectionManager
  readonly #settings: () => Promise<AppSettings>
  readonly #loadCollector: () => Promise<string>
  readonly #logger: Logger
  readonly #entries = new Map<string, TelemetryEntry>()
  readonly #termGrants = new Map<string, { user: string; command: string; at: number }>()
  readonly #restartBaseMs: number
  readonly #timeoutFloorMs: number
  readonly #intervalTimeoutFactorMs: number
  readonly #random: () => number
  #collectorSource?: string

  constructor(connections: SshConnectionManager, settings: () => Promise<AppSettings>, loadCollector: () => Promise<string>, logger: Logger, options: TelemetryServiceOptions = {}) {
    super()
    this.#connections = connections
    this.#settings = settings
    this.#loadCollector = loadCollector
    this.#logger = logger
    this.#restartBaseMs = options.restartBaseMs ?? 1000
    this.#timeoutFloorMs = options.timeoutFloorMs ?? 10_000
    this.#intervalTimeoutFactorMs = options.intervalTimeoutFactorMs ?? 3000
    this.#random = options.random ?? Math.random
  }

  list(): TelemetryStatus[] { return [...this.#entries.values()].map((entry) => this.#status(entry)) }
  status(hostId: string): TelemetryStatus { return this.#status(this.#entry(hostId)) }
  history(hostId: string): TelemetrySnapshot[] { return downsample(this.#entry(hostId).history, 3600) }

  async start(hostId: string): Promise<TelemetryStatus> {
    const entry = this.#entry(hostId)
    if (entry.active && (entry.state === 'online' || entry.state === 'starting')) return this.#status(entry)
    const settings = await this.#settings()
    entry.intervalSeconds = settings.telemetryIntervalSeconds
    entry.retentionMinutes = settings.telemetryRetentionMinutes
    entry.active = true
    entry.suspended = false
    if (entry.state === 'stopped' || entry.state === 'dependency_missing' || entry.state === 'failed') entry.restartCount = 0
    await this.#launch(entry)
    return this.#status(entry)
  }

  stop(hostId: string): TelemetryStatus {
    const entry = this.#entry(hostId)
    entry.active = false
    entry.suspended = false
    entry.generation += 1
    this.#clearTimers(entry)
    entry.channel?.close()
    delete entry.channel
    entry.state = 'stopped'
    delete entry.startedAt
    delete entry.nextRetryAt
    delete entry.lastError
    return this.#emitStatus(entry)
  }

  stopAll(): void { for (const hostId of this.#entries.keys()) this.stop(hostId) }

  networkOffline(hostId: string): void {
    const entry = this.#entries.get(hostId)
    if (!entry?.active || entry.suspended) return
    this.#fail(entry, entry.generation, new Error('SSH connection is offline'))
  }

  wake(hostId: string): void {
    const entry = this.#entries.get(hostId)
    if (!entry?.active || entry.suspended || entry.state === 'online' || entry.state === 'starting') return
    if (entry.retryTimer) clearTimeout(entry.retryTimer)
    delete entry.retryTimer
    void this.#launch(entry)
  }

  suspend(): void {
    for (const entry of this.#entries.values()) {
      if (!entry.active) continue
      entry.suspended = true
      entry.generation += 1
      this.#clearTimers(entry)
      entry.channel?.close()
      delete entry.channel
      entry.state = 'recovering'
      entry.lastError = 'Windows is suspended; telemetry will resume with the SSH connection.'
      this.#emitStatus(entry)
    }
  }

  resume(): void {
    for (const entry of this.#entries.values()) {
      if (!entry.active || !entry.suspended) continue
      entry.suspended = false
      void this.#launch(entry)
    }
  }

  async signal(request: TelemetrySignalRequest): Promise<{ delivered: true; signal: 'TERM' | 'KILL'; pid: number }> {
    const entry = this.#entry(request.hostId)
    const latest = entry.history.at(-1)
    if (!latest) throw new Error('A current telemetry snapshot is required before signaling a process')
    if (request.expectedUser !== latest.currentUser) throw new Error('Only processes owned by the current SSH user can be signaled')
    const process = latest.processes.find((item) => item.pid === request.pid)
    if (!process || process.user !== request.expectedUser || process.command !== request.expectedCommand) throw new Error('Process snapshot is stale; refresh before signaling')
    const key = `${request.hostId}:${String(request.pid)}`
    if (request.signal === 'KILL') {
      const grant = this.#termGrants.get(key)
      if (!request.confirmKill || !grant || Date.now() - grant.at > 60_000 || grant.user !== request.expectedUser || grant.command !== request.expectedCommand) throw new Error('SIGKILL requires a recent SIGTERM attempt for the same process snapshot')
    }
    const client = this.#connections.getOnlineClient(request.hostId)
    const probe = (await execRemote(client, `ps -p ${String(request.pid)} -o user:256= -o args=`)).stdout.trim()
    const separator = probe.search(/\s/)
    const actualUser = separator < 0 ? probe : probe.slice(0, separator)
    const actualCommand = separator < 0 ? '' : probe.slice(separator).trimStart()
    if (actualUser !== request.expectedUser || actualCommand !== request.expectedCommand) throw new Error('PID or command changed; signal was not sent')
    const result = await execRemote(client, `kill -${request.signal} ${String(request.pid)}`)
    if (result.code !== 0) throw new Error(result.stderr.trim() || `kill exited with ${String(result.code)}`)
    if (request.signal === 'TERM') this.#termGrants.set(key, { user: request.expectedUser, command: request.expectedCommand, at: Date.now() })
    else this.#termGrants.delete(key)
    this.#logger.info({ hostId: request.hostId, pid: request.pid, signal: request.signal }, 'Remote process signal delivered after snapshot revalidation')
    return { delivered: true, signal: request.signal, pid: request.pid }
  }

  async #launch(entry: TelemetryEntry): Promise<void> {
    if (!entry.active || entry.suspended) return
    this.#clearTimers(entry)
    entry.channel?.close()
    delete entry.channel
    const generation = ++entry.generation
    entry.state = entry.restartCount > 0 ? 'recovering' : 'starting'
    entry.buffer = ''
    entry.stderr = ''
    delete entry.nextRetryAt
    this.#emitStatus(entry)
    try {
      const client = this.#connections.getOnlineClient(entry.hostId)
      this.#collectorSource ??= await this.#loadCollector()
      const channel = await openCollector(client, entry.intervalSeconds)
      if (!this.#isCurrent(entry, generation)) { channel.close(); return }
      entry.channel = channel
      channel.setEncoding('utf8')
      channel.stderr.setEncoding('utf8')
      channel.stderr.on('data', (chunk: string) => { if (this.#isCurrent(entry, generation)) entry.stderr = `${entry.stderr}${chunk}`.slice(-4096) })
      channel.on('data', (chunk: string) => this.#onData(entry, generation, chunk))
      channel.once('close', (code?: number) => {
        if (!this.#isCurrent(entry, generation) || !entry.active) return
        if (code === 127 || /python3.*(?:not found|missing)/i.test(entry.stderr)) this.#dependencyMissing(entry, generation)
        else this.#fail(entry, generation, new Error(entry.stderr.trim() || `Collector exited (${String(code ?? 'unknown')})`))
      })
      channel.once('error', (error: Error) => this.#fail(entry, generation, error))
      channel.end(this.#collectorSource)
      this.#armTimeout(entry, generation)
    } catch (error) {
      this.#fail(entry, generation, error)
    }
  }

  #onData(entry: TelemetryEntry, generation: number, chunk: string): void {
    if (!this.#isCurrent(entry, generation)) return
    entry.buffer += chunk
    if (entry.buffer.length > 4 * 1024 * 1024) {
      entry.invalidLineCount += 1
      this.#fail(entry, generation, new Error('Collector JSONL line exceeded 4 MiB'))
      return
    }
    for (;;) {
      const newline = entry.buffer.indexOf('\n')
      if (newline < 0) break
      const line = entry.buffer.slice(0, newline).trim()
      entry.buffer = entry.buffer.slice(newline + 1)
      if (!line) continue
      try {
        const payload = collectorV1PayloadSchema.parse(JSON.parse(line))
        const snapshot: TelemetrySnapshot = { ...payload, hostId: entry.hostId }
        const previous = entry.history.at(-1)
        if (previous) { previous.processes = []; previous.gpuProcesses = [] }
        entry.history.push(snapshot)
        const cutoff = Date.now() - entry.retentionMinutes * 60_000
        entry.history = entry.history.filter((item) => Date.parse(item.capturedAt) >= cutoff)
        entry.sampleCount += 1
        entry.state = 'online'
        entry.lastSampleAt = snapshot.capturedAt
        entry.startedAt ??= snapshot.capturedAt
        delete entry.lastError
        delete entry.nextRetryAt
        this.#armTimeout(entry, generation)
        this.emit('event', { type: 'snapshot', snapshot } satisfies TelemetryEvent)
        this.#emitStatus(entry)
      } catch (error) {
        entry.invalidLineCount += 1
        this.#fail(entry, generation, new Error(`Collector emitted invalid v1 JSONL: ${messageOf(error)}`))
        return
      }
    }
  }

  #armTimeout(entry: TelemetryEntry, generation: number): void {
    if (entry.sampleTimer) clearTimeout(entry.sampleTimer)
    const timeout = Math.max(this.#timeoutFloorMs, entry.intervalSeconds * this.#intervalTimeoutFactorMs)
    entry.sampleTimer = setTimeout(() => this.#fail(entry, generation, new Error(`Collector sample timeout after ${String(timeout)} ms`)), timeout)
  }

  #dependencyMissing(entry: TelemetryEntry, generation: number): void {
    if (!this.#isCurrent(entry, generation)) return
    entry.generation += 1
    this.#clearTimers(entry)
    delete entry.channel
    entry.state = 'dependency_missing'
    entry.lastError = 'python3 is not installed on the remote host. Install Python 3 to enable monitoring; SSH, terminal, SFTP, and tunnels remain available.'
    this.#emitStatus(entry)
  }

  #fail(entry: TelemetryEntry, generation: number, error: unknown): void {
    if (!this.#isCurrent(entry, generation) || !entry.active || entry.suspended) return
    entry.generation += 1
    this.#clearTimers(entry)
    entry.channel?.close()
    delete entry.channel
    entry.restartCount += 1
    entry.state = 'recovering'
    entry.lastError = messageOf(error)
    const base = Math.min(30_000, this.#restartBaseMs * 2 ** Math.min(5, entry.restartCount - 1))
    const delay = Math.min(30_000, Math.round(base * (1 + this.#random() * 0.2)))
    entry.nextRetryAt = new Date(Date.now() + delay).toISOString()
    this.#logger.warn({ hostId: entry.hostId, restartCount: entry.restartCount, error: entry.lastError }, 'Telemetry collector restarting')
    this.#emitStatus(entry)
    entry.retryTimer = setTimeout(() => { delete entry.retryTimer; void this.#launch(entry) }, delay)
  }

  #entry(hostId: string): TelemetryEntry {
    const current = this.#entries.get(hostId)
    if (current) return current
    const entry: TelemetryEntry = { hostId, active: false, suspended: false, generation: 0, state: 'stopped', restartCount: 0, invalidLineCount: 0, sampleCount: 0, intervalSeconds: 3, retentionMinutes: 30, buffer: '', stderr: '', history: [] }
    this.#entries.set(hostId, entry)
    return entry
  }

  #isCurrent(entry: TelemetryEntry, generation: number): boolean { return entry.generation === generation }
  #clearTimers(entry: TelemetryEntry): void { if (entry.retryTimer) clearTimeout(entry.retryTimer); if (entry.sampleTimer) clearTimeout(entry.sampleTimer); delete entry.retryTimer; delete entry.sampleTimer }
  #status(entry: TelemetryEntry): TelemetryStatus { return { hostId: entry.hostId, state: entry.state, restartCount: entry.restartCount, invalidLineCount: entry.invalidLineCount, sampleCount: entry.sampleCount, ...(entry.startedAt ? { startedAt: entry.startedAt } : {}), ...(entry.nextRetryAt ? { nextRetryAt: entry.nextRetryAt } : {}), ...(entry.lastSampleAt ? { lastSampleAt: entry.lastSampleAt } : {}), ...(entry.lastError ? { lastError: entry.lastError } : {}) } }
  #emitStatus(entry: TelemetryEntry): TelemetryStatus { const status = this.#status(entry); this.emit('event', { type: 'status', status } satisfies TelemetryEvent); return status }
}

function openCollector(client: SshClient, intervalSeconds: number): Promise<ClientChannel> {
  return new Promise((resolve, reject) => client.exec(`command -v python3 >/dev/null 2>&1 || { echo 'python3 missing' >&2; exit 127; }; python3 -u - --interval ${String(intervalSeconds)}`, (error, channel) => error ? reject(error) : resolve(channel)))
}

function execRemote(client: SshClient, command: string): Promise<{ stdout: string; stderr: string; code: number }> {
  return new Promise((resolve, reject) => client.exec(command, (error, channel) => {
    if (error) { reject(error); return }
    let stdout = ''
    let stderr = ''
    channel.setEncoding('utf8')
    channel.stderr.setEncoding('utf8')
    channel.on('data', (chunk: string) => { stdout += chunk })
    channel.stderr.on('data', (chunk: string) => { stderr += chunk })
    channel.once('error', reject)
    channel.once('close', (code?: number) => resolve({ stdout, stderr, code: code ?? 0 }))
  }))
}

function messageOf(value: unknown): string { return value instanceof Error ? value.message : String(value) }
function downsample(values: TelemetrySnapshot[], limit: number): TelemetrySnapshot[] {
  if (values.length <= limit) return [...values]
  const step = (values.length - 1) / (limit - 1)
  return Array.from({ length: limit }, (_value, index) => values[Math.round(index * step)]).filter((value): value is TelemetrySnapshot => value !== undefined)
}
