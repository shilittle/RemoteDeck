import { randomUUID } from 'node:crypto'
import { EventEmitter } from 'node:events'
import type { ClientChannel } from 'ssh2'
import type { CommandAnalysis, CommandDefinition, CommandEvent, CommandJob, CommandPresetInput, CommandRunRequest } from '../../protocol/command'
import type { z } from 'zod'
import type { commandPresetUpdateSchema } from '../../protocol/command'
import type { CommandPreset } from '../../protocol/domain'
import type { ProfileRepository } from '../hosts/profile-repository'
import type { SshConnectionManager } from '../ssh/connection-manager'
import type { TerminalService } from '../terminal/terminal-service'
import { builtinCommands, getBuiltin } from './builtins'
import { inDirectory, remoteExec, shellQuote } from './remote-exec'
import { analyzeCommandRisk } from './risk-engine'

interface RunningJob { job: CommandJob; stream?: ClientChannel }

export class CommandService extends EventEmitter {
  readonly #profiles: ProfileRepository
  readonly #connections: SshConnectionManager
  readonly #terminals: TerminalService
  readonly #jobs = new Map<string, RunningJob>()

  constructor(profiles: ProfileRepository, connections: SshConnectionManager, terminals: TerminalService) {
    super()
    this.#profiles = profiles
    this.#connections = connections
    this.#terminals = terminals
  }

  async list(hostId: string): Promise<CommandDefinition[]> {
    await this.#profiles.get(hostId)
    const available = await this.#availableDependencies(hostId)
    const builtins = builtinCommands.map(({ dependency, ...item }) => ({
      ...item,
      builtin: true,
      available: dependency ? available.has(dependency) : true,
      ...(dependency && !available.has(dependency) ? { unavailableReason: `远端未检测到 ${dependency}` } : {})
    }))
    const custom = (await this.#profiles.listCommands(hostId)).map((item) => ({ ...item, builtin: false, available: true }))
    return [...builtins, ...custom].sort((left, right) => left.sortOrder - right.sortOrder || left.name.localeCompare(right.name))
  }

  create(input: CommandPresetInput): Promise<CommandPreset> { return this.#profiles.addCommand(input) }
  update(id: string, patch: z.infer<typeof commandPresetUpdateSchema>['patch']): Promise<CommandPreset> {
    const defined = Object.fromEntries(Object.entries(patch).filter((entry) => entry[1] !== undefined)) as Partial<CommandPresetInput>
    return this.#profiles.updateCommand(id, defined)
  }
  delete(id: string): Promise<boolean> {
    if (getBuiltin(id)) throw new Error('Built-in commands cannot be deleted')
    return this.#profiles.deleteCommand(id)
  }

  async analyze(hostId: string, presetId: string): Promise<CommandAnalysis> {
    const [profile, preset] = await Promise.all([this.#profiles.get(hostId), this.#preset(hostId, presetId)])
    const required = preset.confirmationText?.trim() || profile.host.alias
    const command = composeCommand(preset)
    const cwd = preset.workingDirectory ?? profile.workspace?.remotePath
    const legacyRules = await this.#profiles.listLegacyRiskRules()
    return { ...analyzeCommandRisk(command, preset.risk, required, legacyRules), displayCommand: inDirectory(command, cwd), targetAlias: profile.host.alias, ...(cwd ? { workingDirectory: cwd } : {}) }
  }

  listJobs(): CommandJob[] { return [...this.#jobs.values()].map(({ job }) => structuredClone(job)) }

  async run(request: CommandRunRequest): Promise<CommandJob> {
    const [profile, preset] = await Promise.all([this.#profiles.get(request.hostId), this.#preset(request.hostId, request.presetId)])
    const definition = (await this.list(request.hostId)).find((item) => item.id === preset.id)
    if (definition && !definition.available) throw new Error(definition.unavailableReason ?? 'Command is unavailable on this host')
    const command = composeCommand(preset)
    const required = preset.confirmationText?.trim() || profile.host.alias
    const analysis = analyzeCommandRisk(command, preset.risk, required, await this.#profiles.listLegacyRiskRules())
    enforceConfirmation(analysis, request.confirmed, request.confirmationInput)
    const cwd = preset.workingDirectory ?? profile.workspace?.remotePath
    const finalCommand = inDirectory(command, cwd)
    const job: CommandJob = {
      schemaVersion: 1,
      id: randomUUID(), hostId: request.hostId, presetId: preset.id, name: preset.name, command: finalCommand,
      state: 'running', risk: analysis.effectiveRisk, startedAt: new Date().toISOString(), output: ''
    }
    this.#jobs.set(job.id, { job })
    this.#emitJob(job)
    if (preset.requiresPty) {
      try {
        const terminal = await this.#terminals.create({ hostId: request.hostId, ...(cwd ? { cwd } : {}), cols: 120, rows: 34 })
        this.#terminals.write(terminal.id, `${command}\r`)
        job.state = 'terminal'
        job.terminalSessionId = terminal.id
        job.completedAt = new Date().toISOString()
        this.#emitJob(job)
      } catch (error) {
        failJob(job, error)
        this.#emitJob(job)
      }
      return structuredClone(job)
    }
    void this.#execute(job, finalCommand)
    return structuredClone(job)
  }

  cancel(jobId: string): CommandJob {
    const entry = this.#jobs.get(jobId)
    if (!entry) throw new Error(`Unknown command job: ${jobId}`)
    if (entry.job.state !== 'running') return structuredClone(entry.job)
    entry.job.state = 'cancelled'
    entry.job.completedAt = new Date().toISOString()
    entry.stream?.close()
    delete entry.stream
    this.#emitJob(entry.job)
    return structuredClone(entry.job)
  }

  cancelAll(): void {
    for (const [id, entry] of this.#jobs) if (entry.job.state === 'running') this.cancel(id)
  }

  async #execute(job: CommandJob, command: string): Promise<void> {
    try {
      const client = this.#connections.getOnlineClient(job.hostId)
      const stream = await new Promise<ClientChannel>((resolve, reject) => client.exec(command, (error, channel) => error ? reject(error) : resolve(channel)))
      const entry = this.#jobs.get(job.id)
      if (!entry || job.state !== 'running') { stream.close(); return }
      entry.stream = stream
      stream.setEncoding('utf8')
      stream.stderr.setEncoding('utf8')
      stream.on('data', (data: string) => this.#append(job, 'stdout', data))
      stream.stderr.on('data', (data: string) => this.#append(job, 'stderr', data))
      stream.once('exit', (code?: number, signal?: string) => { job.exitCode = code ?? null; job.exitSignal = signal ?? null })
      stream.once('error', (error: Error) => {
        if (job.state !== 'running') return
        failJob(job, error)
        this.#emitJob(job)
      })
      stream.once('close', () => {
        delete entry.stream
        if (job.state !== 'running') return
        job.state = job.exitCode === 0 ? 'completed' : 'failed'
        job.completedAt = new Date().toISOString()
        if (job.state === 'failed' && !job.error) job.error = `远端命令退出码 ${String(job.exitCode ?? 'unknown')}${job.exitSignal ? `，信号 ${job.exitSignal}` : ''}`
        this.#emitJob(job)
      })
    } catch (error) {
      if (job.state !== 'running') return
      failJob(job, error)
      this.#emitJob(job)
    }
  }

  #append(job: CommandJob, stream: 'stdout' | 'stderr', data: string): void {
    if (job.state !== 'running') return
    const prefix = stream === 'stderr' ? '[stderr] ' : ''
    job.output = appendBounded(job.output, prefix + data, 2 * 1024 * 1024)
    this.emit('event', { type: 'data', jobId: job.id, stream, data } satisfies CommandEvent)
  }

  #emitJob(job: CommandJob): void { this.emit('event', { type: 'job', job: structuredClone(job) } satisfies CommandEvent) }

  async #preset(hostId: string, presetId: string): Promise<CommandPreset> {
    const builtin = getBuiltin(presetId)
    if (builtin) return builtin
    const preset = await this.#profiles.getCommand(presetId)
    if (preset.hostId && preset.hostId !== hostId) throw new Error('This command preset belongs to a different host')
    return preset
  }

  async #availableDependencies(hostId: string): Promise<Set<string>> {
    if (this.#connections.stateFor(hostId) !== 'online') return new Set()
    try {
      const result = await remoteExec(this.#connections.getOnlineClient(hostId), 'for x in nvidia-smi ss git squeue btop; do command -v "$x" >/dev/null 2>&1 && printf "%s\\n" "$x"; done', 5000, 4096)
      return new Set(result.stdout.split(/\r?\n/).filter(Boolean))
    } catch { return new Set() }
  }
}

function composeCommand(preset: CommandPreset): string {
  return preset.requiresSudo ? `sudo -- sh -lc ${shellQuote(preset.command)}` : preset.command
}

function enforceConfirmation(analysis: CommandAnalysis, confirmed: boolean, input: string): void {
  if (analysis.effectiveRisk === 'L0') return
  if (!confirmed) throw new Error(`${analysis.effectiveRisk} command requires explicit confirmation`)
  if (analysis.effectiveRisk === 'L2' && input !== analysis.requiredConfirmation) throw new Error('L2 confirmation text does not match')
}

function appendBounded(current: string, data: string, maxBytes: number): string {
  const next = current + data
  if (Buffer.byteLength(next) <= maxBytes) return next
  return Buffer.from(next).subarray(-maxBytes).toString('utf8')
}

function failJob(job: CommandJob, error: unknown): void {
  job.state = 'failed'
  job.completedAt = new Date().toISOString()
  job.error = error instanceof Error ? error.message : String(error)
}
