import { Duplex, PassThrough } from 'node:stream'
import { describe, expect, it } from 'vitest'
import type { CommandPreset } from '../../protocol/domain'
import { CommandService } from './command-service'

const hostId = '019f92f0-b87c-7c74-a668-54d5b18fb487'
const now = '2026-01-01T00:00:00.000Z'

class FakeChannel extends Duplex {
  readonly stderr = new PassThrough()
  closeCalled = false
  _read(): void { /* pushed by test */ }
  _write(_chunk: Buffer, _encoding: BufferEncoding, callback: (error?: Error | null) => void): void { callback() }
  close(): void { this.closeCalled = true; this.emit('close') }
}

function preset(id: string, command: string, risk: CommandPreset['risk'], options: Partial<CommandPreset> = {}): CommandPreset {
  return { schemaVersion: 1, id, hostId, name: id, description: '', group: '', command, risk, requiresPty: false, requiresSudo: false, sortOrder: 0, createdAt: now, updatedAt: now, ...options }
}

describe('command execution guardrails', () => {
  it('enforces L1/L2 confirmation in core and streams/cancels only the owned channel', async () => {
    const l1 = preset('20000000-0000-4000-8000-000000000001', 'touch /tmp/m7', 'L1')
    const l2 = preset('20000000-0000-4000-8000-000000000002', 'rm -rf /tmp/m7', 'L0')
    const commands = new Map([[l1.id, l1], [l2.id, l2]])
    const channels: FakeChannel[] = []
    const client = { exec: (_command: string, callback: (error: Error | undefined, channel: FakeChannel) => void) => { const channel = new FakeChannel(); channels.push(channel); callback(undefined, channel) } }
    const profiles = {
      get: () => Promise.resolve({ host: { id: hostId, alias: 'worker' }, workspace: { remotePath: '/srv/work tree' } }),
      getCommand: (id: string) => Promise.resolve(commands.get(id)),
      listCommands: () => Promise.resolve([...commands.values()]),
      listLegacyRiskRules: () => Promise.resolve([])
    }
    const service = new CommandService(profiles as never, { stateFor: () => 'offline', getOnlineClient: () => client } as never, {} as never)

    await expect(service.run({ hostId, presetId: l1.id, confirmed: false, confirmationInput: '' })).rejects.toThrow(/confirmation/)
    const first = await service.run({ hostId, presetId: l1.id, confirmed: true, confirmationInput: '' })
    await new Promise((resolve) => setImmediate(resolve))
    expect(channels, JSON.stringify(service.listJobs())).toHaveLength(1)
    channels[0]?.push('created\n')
    await new Promise((resolve) => setImmediate(resolve))
    expect(service.listJobs().find((item) => item.id === first.id)?.output).toContain('created')

    await expect(service.run({ hostId, presetId: l2.id, confirmed: true, confirmationInput: 'wrong' })).rejects.toThrow(/does not match/)
    const second = await service.run({ hostId, presetId: l2.id, confirmed: true, confirmationInput: 'worker' })
    await new Promise((resolve) => setImmediate(resolve))
    expect(channels).toHaveLength(2)
    service.cancel(first.id)
    expect(channels[0]?.closeCalled).toBe(true)
    expect(channels[1]?.closeCalled).toBe(false)
    expect(service.listJobs().find((item) => item.id === second.id)?.state).toBe('running')
  })

  it('opens PTY commands through TerminalService and preserves sudo/cwd quoting', async () => {
    const pty = preset('20000000-0000-4000-8000-000000000003', "printf '%s' okay", 'L1', { requiresPty: true, requiresSudo: true })
    const writes: string[] = []
    const profiles = { get: () => Promise.resolve({ host: { id: hostId, alias: 'worker' }, workspace: { remotePath: "/srv/it's work" } }), getCommand: () => Promise.resolve(pty), listCommands: () => Promise.resolve([pty]), listLegacyRiskRules: () => Promise.resolve([]) }
    const terminals = { create: () => Promise.resolve({ id: '30000000-0000-4000-8000-000000000001' }), write: (_id: string, data: string) => { writes.push(data) } }
    const service = new CommandService(profiles as never, { stateFor: () => 'offline' } as never, terminals as never)
    const job = await service.run({ hostId, presetId: pty.id, confirmed: true, confirmationInput: '' })
    expect(job).toMatchObject({ state: 'terminal', terminalSessionId: '30000000-0000-4000-8000-000000000001' })
    expect(writes[0]).toContain("sudo -- sh -lc")
    expect(writes[0]).toContain('printf')
  })
})
