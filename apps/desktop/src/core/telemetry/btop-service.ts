import { EventEmitter } from 'node:events'
import type { Logger } from 'pino'
import type { Client as SshClient, ClientChannel } from 'ssh2'
import type { BtopStatus, TelemetryEvent } from '../../protocol/telemetry'
import type { SshConnectionManager } from '../ssh/connection-manager'

interface BtopEntry extends BtopStatus {
  active: boolean
  suspended: boolean
  generation: number
  channel?: ClientChannel
  retryTimer?: ReturnType<typeof setTimeout>
  rotationTimer?: ReturnType<typeof setTimeout>
}

export class BtopService extends EventEmitter {
  readonly #connections: SshConnectionManager
  readonly #logger: Logger
  readonly #entries = new Map<string, BtopEntry>()

  constructor(connections: SshConnectionManager, logger: Logger) { super(); this.#connections = connections; this.#logger = logger }

  status(hostId: string): BtopStatus { return snapshot(this.#entry(hostId)) }

  async probe(hostId: string): Promise<BtopStatus> {
    const entry = this.#entry(hostId)
    const result = await execRemote(this.#connections.getOnlineClient(hostId), "command -v btop >/dev/null 2>&1 && btop --version | head -n 1")
    entry.installed = result.code === 0 && Boolean(result.stdout.trim())
    entry.version = entry.installed ? result.stdout.trim().slice(0, 512) : null
    if (!entry.installed && entry.active) { entry.active = false; entry.watchdogState = 'unavailable'; entry.lastError = 'btop is not installed on the remote host.' }
    return this.#emit(entry)
  }

  async start(hostId: string, rotationMinutes: number): Promise<BtopStatus> {
    const entry = this.#entry(hostId)
    entry.rotationMinutes = rotationMinutes
    entry.active = true
    entry.suspended = false
    entry.restartCount = 0
    const status = await this.probe(hostId)
    if (!status.installed) { entry.active = false; entry.watchdogState = 'unavailable'; entry.lastError = 'btop is not installed; the watchdog was not started.'; return this.#emit(entry) }
    await this.#launch(entry)
    return snapshot(entry)
  }

  stop(hostId: string): BtopStatus {
    const entry = this.#entry(hostId)
    entry.active = false
    entry.suspended = false
    entry.generation += 1
    this.#clear(entry)
    entry.channel?.close()
    delete entry.channel
    entry.watchdogState = 'stopped'
    delete entry.lastError
    return this.#emit(entry)
  }

  stopAll(): void { for (const hostId of this.#entries.keys()) this.stop(hostId) }
  networkOffline(hostId: string): void { const entry = this.#entries.get(hostId); if (entry?.active && !entry.suspended) this.#recover(entry, entry.generation, new Error('SSH connection is offline')) }
  wake(hostId: string): void { const entry = this.#entries.get(hostId); if (!entry?.active || entry.suspended || entry.watchdogState === 'running' || entry.watchdogState === 'starting') return; this.#clear(entry); void this.#launch(entry) }
  suspend(): void { for (const entry of this.#entries.values()) if (entry.active) { entry.suspended = true; entry.generation += 1; this.#clear(entry); entry.channel?.close(); delete entry.channel; entry.watchdogState = 'recovering'; entry.lastError = 'Windows is suspended; btop watchdog will resume later.'; this.#emit(entry) } }
  resume(): void { for (const entry of this.#entries.values()) if (entry.active && entry.suspended) { entry.suspended = false; void this.#launch(entry) } }

  async #launch(entry: BtopEntry): Promise<void> {
    if (!entry.active || entry.suspended) return
    this.#clear(entry)
    entry.channel?.close()
    delete entry.channel
    const generation = ++entry.generation
    entry.watchdogState = 'starting'
    delete entry.lastError
    this.#emit(entry)
    try {
      const channel = await openBtop(this.#connections.getOnlineClient(entry.hostId))
      if (entry.generation !== generation) { channel.close(); return }
      entry.channel = channel
      entry.watchdogState = 'running'
      channel.resume()
      channel.stderr.resume()
      channel.once('error', (error: Error) => this.#recover(entry, generation, error))
      channel.once('close', (code?: number) => this.#recover(entry, generation, new Error(`btop exited (${String(code ?? 'unknown')})`)))
      entry.rotationTimer = setTimeout(() => {
        if (entry.generation !== generation || !entry.active) return
        entry.generation += 1
        channel.close()
        entry.restartCount += 1
        this.#logger.info({ hostId: entry.hostId }, 'Rotating RemoteDeck btop watchdog session')
        void this.#launch(entry)
      }, entry.rotationMinutes * 60_000)
      this.#emit(entry)
    } catch (error) {
      this.#recover(entry, generation, error)
    }
  }

  #recover(entry: BtopEntry, generation: number, error: unknown): void {
    if (entry.generation !== generation || !entry.active || entry.suspended) return
    entry.generation += 1
    this.#clear(entry)
    entry.channel?.close()
    delete entry.channel
    entry.restartCount += 1
    entry.watchdogState = 'recovering'
    entry.lastError = error instanceof Error ? error.message : String(error)
    this.#emit(entry)
    const delay = Math.min(30_000, 1000 * 2 ** Math.min(5, entry.restartCount - 1))
    entry.retryTimer = setTimeout(() => { delete entry.retryTimer; void this.#launch(entry) }, delay)
  }

  #entry(hostId: string): BtopEntry {
    const current = this.#entries.get(hostId)
    if (current) return current
    const entry: BtopEntry = { hostId, installed: false, version: null, watchdogState: 'stopped', restartCount: 0, rotationMinutes: 60, active: false, suspended: false, generation: 0 }
    this.#entries.set(hostId, entry)
    return entry
  }
  #clear(entry: BtopEntry): void { if (entry.retryTimer) clearTimeout(entry.retryTimer); if (entry.rotationTimer) clearTimeout(entry.rotationTimer); delete entry.retryTimer; delete entry.rotationTimer }
  #emit(entry: BtopEntry): BtopStatus { const status = snapshot(entry); this.emit('event', { type: 'btop', status } satisfies TelemetryEvent); return status }
}

function snapshot(entry: BtopEntry): BtopStatus { return { hostId: entry.hostId, installed: entry.installed, version: entry.version, watchdogState: entry.watchdogState, restartCount: entry.restartCount, rotationMinutes: entry.rotationMinutes, ...(entry.lastError ? { lastError: entry.lastError } : {}) } }
function openBtop(client: SshClient): Promise<ClientChannel> { return new Promise((resolve, reject) => client.exec('btop', { pty: { term: 'xterm-256color', cols: 120, rows: 40, width: 0, height: 0 } }, (error, channel) => error ? reject(error) : resolve(channel))) }
function execRemote(client: SshClient, command: string): Promise<{ stdout: string; code: number }> { return new Promise((resolve, reject) => client.exec(command, (error, channel) => { if (error) { reject(error); return } let stdout = ''; channel.setEncoding('utf8'); channel.on('data', (chunk: string) => { stdout += chunk }); channel.stderr.resume(); channel.once('error', reject); channel.once('close', (code?: number) => resolve({ stdout, code: code ?? 0 })) })) }
