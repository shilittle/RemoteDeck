import { randomUUID } from 'node:crypto'
import { EventEmitter } from 'node:events'
import type { ClientChannel } from 'ssh2'
import type { TerminalCreateRequest, TerminalEvent, TerminalResizeRequest, TerminalSession } from '../../protocol/terminal'
import type { ProfileRepository } from '../hosts/profile-repository'
import type { SshConnectionManager } from '../ssh/connection-manager'

interface TerminalEntry extends TerminalSession {
  stream?: ClientChannel
}

export class TerminalService extends EventEmitter {
  readonly #connections: SshConnectionManager
  readonly #profiles: ProfileRepository
  readonly #sessions = new Map<string, TerminalEntry>()

  constructor(connections: SshConnectionManager, profiles: ProfileRepository) {
    super()
    this.#connections = connections
    this.#profiles = profiles
  }

  list(): TerminalSession[] {
    return [...this.#sessions.values()].map((entry) => snapshot(entry))
  }

  async create(request: TerminalCreateRequest): Promise<TerminalSession> {
    const profile = await this.#profiles.get(request.hostId)
    const entry: TerminalEntry = {
      id: randomUUID(),
      hostId: request.hostId,
      hostAlias: profile.host.alias,
      cwd: request.cwd ?? profile.workspace?.remotePath ?? '~',
      cols: request.cols,
      rows: request.rows,
      generation: 1,
      state: 'opening',
      openedAt: new Date().toISOString()
    }
    this.#sessions.set(entry.id, entry)
    this.#emitState(entry)
    await this.#open(entry)
    return snapshot(entry)
  }

  async reconnect(sessionId: string): Promise<TerminalSession> {
    const entry = this.#required(sessionId)
    entry.stream?.end()
    delete entry.stream
    entry.generation += 1
    entry.state = 'opening'
    entry.openedAt = new Date().toISOString()
    delete entry.error
    delete entry.exitCode
    delete entry.exitSignal
    this.#emitState(entry)
    await this.#open(entry)
    return snapshot(entry)
  }

  write(sessionId: string, data: string): TerminalSession {
    const entry = this.#required(sessionId)
    if (entry.state !== 'online' || !entry.stream) throw new Error('Terminal shell is not online')
    entry.stream.write(data)
    return snapshot(entry)
  }

  resize(request: TerminalResizeRequest): TerminalSession {
    const entry = this.#required(request.sessionId)
    entry.cols = request.cols
    entry.rows = request.rows
    if (entry.state === 'online' && entry.stream) entry.stream.setWindow(request.rows, request.cols, 0, 0)
    return snapshot(entry)
  }

  close(sessionId: string): TerminalSession {
    const entry = this.#required(sessionId)
    entry.state = 'closed'
    entry.stream?.end()
    delete entry.stream
    this.#emitState(entry)
    const result = snapshot(entry)
    this.#sessions.delete(sessionId)
    return result
  }

  closeAll(): void {
    for (const id of [...this.#sessions.keys()]) this.close(id)
  }

  async #open(entry: TerminalEntry): Promise<void> {
    const generation = entry.generation
    try {
      const client = this.#connections.getOnlineClient(entry.hostId)
      const stream = await new Promise<ClientChannel>((resolve, reject) => {
        client.shell({ term: 'xterm-256color', cols: entry.cols, rows: entry.rows, width: 0, height: 0 }, (error, channel) => error ? reject(error) : resolve(channel))
      })
      if (!this.#isCurrent(entry.id, generation)) { stream.end(); return }
      entry.stream = stream
      entry.state = 'online'
      delete entry.error
      stream.setEncoding('utf8')
      stream.on('data', (data: string) => {
        if (this.#isCurrent(entry.id, generation)) this.emit('event', { type: 'data', sessionId: entry.id, generation, data } satisfies TerminalEvent)
      })
      stream.stderr.setEncoding('utf8')
      stream.stderr.on('data', (data: string) => {
        if (this.#isCurrent(entry.id, generation)) this.emit('event', { type: 'data', sessionId: entry.id, generation, data } satisfies TerminalEvent)
      })
      stream.once('exit', (code?: number, signal?: string) => {
        if (!this.#isCurrent(entry.id, generation)) return
        entry.exitCode = code ?? null
        entry.exitSignal = signal ?? null
      })
      stream.once('close', () => {
        if (!this.#isCurrent(entry.id, generation) || entry.state === 'closed') return
        delete entry.stream
        if (entry.state === 'failed') return
        entry.state = 'offline'
        entry.error = entry.exitSignal ? `远端 shell 因信号 ${entry.exitSignal} 结束。` : `远端 shell 已结束${entry.exitCode === null || entry.exitCode === undefined ? '' : `（退出码 ${String(entry.exitCode)}）`}。`
        this.#emitState(entry)
      })
      stream.once('error', (error: Error) => {
        if (!this.#isCurrent(entry.id, generation)) return
        entry.error = error.message
        entry.state = 'failed'
        this.#emitState(entry)
      })
      this.#emitState(entry)
      const changeDirectory = cdCommand(entry.cwd)
      if (changeDirectory) stream.write(`${changeDirectory}\r`)
    } catch (error) {
      if (!this.#isCurrent(entry.id, generation)) return
      entry.state = 'failed'
      entry.error = error instanceof Error ? error.message : String(error)
      this.#emitState(entry)
    }
  }

  #required(sessionId: string): TerminalEntry {
    const entry = this.#sessions.get(sessionId)
    if (!entry) throw new Error(`Unknown terminal session: ${sessionId}`)
    return entry
  }

  #isCurrent(sessionId: string, generation: number): boolean {
    return this.#sessions.get(sessionId)?.generation === generation
  }

  #emitState(entry: TerminalEntry): void {
    this.emit('event', { type: 'state', session: snapshot(entry) } satisfies TerminalEvent)
  }
}

function snapshot(entry: TerminalEntry): TerminalSession {
  return {
    id: entry.id,
    hostId: entry.hostId,
    hostAlias: entry.hostAlias,
    cwd: entry.cwd,
    cols: entry.cols,
    rows: entry.rows,
    generation: entry.generation,
    state: entry.state,
    openedAt: entry.openedAt,
    ...(entry.error ? { error: entry.error } : {}),
    ...(entry.exitCode !== undefined ? { exitCode: entry.exitCode } : {}),
    ...(entry.exitSignal !== undefined ? { exitSignal: entry.exitSignal } : {})
  }
}

function cdCommand(cwd: string): string {
  if (cwd === '~') return ''
  const target = cwd.startsWith('~/') ? `"$HOME"/${shellQuote(cwd.slice(2))}` : shellQuote(cwd)
  return `cd -- ${target} || printf '\\r\\nRemoteDeck: 无法进入工作目录 %s\\r\\n' ${shellQuote(cwd)}`
}

function shellQuote(value: string): string { return `'${value.replaceAll("'", `'"'"'`)}'` }
