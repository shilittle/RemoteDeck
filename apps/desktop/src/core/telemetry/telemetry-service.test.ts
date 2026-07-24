import { Duplex, PassThrough } from 'node:stream'
import pino from 'pino'
import type { Client as SshClient } from 'ssh2'
import { afterEach, describe, expect, it } from 'vitest'
import type { AppSettings } from '../../protocol/settings'
import type { SshConnectionManager } from '../ssh/connection-manager'
import { BtopService } from './btop-service'
import { TelemetryService } from './telemetry-service'

const hostId = '11111111-1111-4111-8111-111111111111'
const services: TelemetryService[] = []
const btopServices: BtopService[] = []
const settings: AppSettings = { schemaVersion: 1, launchAtLogin: false, closeToTray: true, terminalFontFamily: 'monospace', terminalFontSize: 14, telemetryIntervalSeconds: 1, telemetryRetentionMinutes: 30, sshConfigPath: '', downloadDirectory: '', autoReconnect: true, logLevel: 'info', onboardingCompleted: false }

afterEach(() => {
  services.splice(0).forEach((service) => service.stopAll())
  btopServices.splice(0).forEach((service) => service.stopAll())
})

describe('TelemetryService supervisor', () => {
  it('accepts collector v1 samples with no GPU and keeps bounded history', async () => {
    const fixture = createFixture()
    await fixture.service.start(hostId)
    fixture.client.collectors[0]?.output(`${JSON.stringify(payload())}\n`)
    await waitFor(() => fixture.service.status(hostId).state === 'online')
    expect(fixture.client.collectors[0]?.written).toContain('# collector')
    expect(fixture.client.commands[0]).toContain('python3 -u - --interval 1')
    expect(fixture.service.history(hostId)).toEqual([expect.objectContaining({ hostId, currentUser: 'alice', gpus: [], gpuProcesses: [] })])
    expect(fixture.service.status(hostId)).toMatchObject({ state: 'online', sampleCount: 1, invalidLineCount: 0 })
  })

  it('restarts after invalid JSON and ignores stale channel output', async () => {
    const fixture = createFixture()
    await fixture.service.start(hostId)
    const stale = fixture.client.collectors[0]
    stale?.output('{not-json}\n')
    await waitFor(() => fixture.service.status(hostId).state === 'recovering')
    await waitFor(() => fixture.client.collectors.length === 2)
    stale?.output(`${JSON.stringify(payload())}\n`)
    expect(fixture.service.history(hostId)).toHaveLength(0)
    fixture.client.collectors[1]?.output(`${JSON.stringify(payload())}\n`)
    await waitFor(() => fixture.service.status(hostId).state === 'online')
    expect(fixture.service.status(hostId)).toMatchObject({ restartCount: 1, invalidLineCount: 1, sampleCount: 1 })
  })

  it('recovers collector crash, timeout, and network return', async () => {
    const fixture = createFixture({ timeoutFloorMs: 20, intervalTimeoutFactorMs: 10 })
    await fixture.service.start(hostId)
    fixture.client.collectors[0]?.exit(1)
    await waitFor(() => fixture.client.collectors.length === 2)
    await waitFor(() => fixture.service.status(hostId).restartCount >= 2, 300)
    fixture.manager.online = false
    fixture.service.networkOffline(hostId)
    await waitFor(() => fixture.service.status(hostId).state === 'recovering')
    fixture.manager.online = true
    fixture.service.wake(hostId)
    await waitFor(() => fixture.client.collectors.length >= 3)
    fixture.client.collectors.at(-1)?.output(`${JSON.stringify(payload())}\n`)
    await waitFor(() => fixture.service.status(hostId).state === 'online')
  })

  it('degrades without python3 instead of retrying forever', async () => {
    const fixture = createFixture()
    await fixture.service.start(hostId)
    const channel = fixture.client.collectors[0]
    channel?.errorOutput('python3 missing\n')
    channel?.exit(127)
    await waitFor(() => fixture.service.status(hostId).state === 'dependency_missing')
    await new Promise((resolve) => setTimeout(resolve, 40))
    expect(fixture.client.collectors).toHaveLength(1)
    expect(fixture.service.status(hostId).lastError).toMatch(/Install Python 3/)
  })

  it('revalidates owner and command, then requires TERM before KILL', async () => {
    const fixture = createFixture()
    await fixture.service.start(hostId)
    fixture.client.collectors[0]?.output(`${JSON.stringify(payload())}\n`)
    await waitFor(() => fixture.service.status(hostId).state === 'online')
    await expect(fixture.service.signal({ hostId, pid: 42, signal: 'TERM', expectedUser: 'bob', expectedCommand: 'sleep 60', confirmKill: false })).rejects.toThrow(/current SSH user/)
    await expect(fixture.service.signal({ hostId, pid: 42, signal: 'KILL', expectedUser: 'alice', expectedCommand: 'sleep 60', confirmKill: true })).rejects.toThrow(/SIGTERM/)
    await expect(fixture.service.signal({ hostId, pid: 42, signal: 'TERM', expectedUser: 'alice', expectedCommand: 'sleep 60', confirmKill: false })).resolves.toMatchObject({ delivered: true, signal: 'TERM' })
    await expect(fixture.service.signal({ hostId, pid: 42, signal: 'KILL', expectedUser: 'alice', expectedCommand: 'sleep 60', confirmKill: true })).resolves.toMatchObject({ delivered: true, signal: 'KILL' })
    expect(fixture.client.commands.filter((command) => command.startsWith('kill'))).toEqual(['kill -TERM 42', 'kill -KILL 42'])
  })
})

describe('BtopService', () => {
  it('probes, supervises a PTY, restarts crashes, and stops only its channel', async () => {
    const client = new FakeClient()
    const manager = new FakeManager(client)
    const service = new BtopService(manager as unknown as SshConnectionManager, pino({ enabled: false }))
    btopServices.push(service)
    await expect(service.probe(hostId)).resolves.toMatchObject({ installed: true, version: 'btop version 1.4.0' })
    await expect(service.start(hostId, 5)).resolves.toMatchObject({ watchdogState: 'running' })
    client.btopChannels[0]?.exit(0)
    await waitFor(() => service.status(hostId).watchdogState === 'recovering')
    await waitFor(() => client.btopChannels.length === 2, 1500)
    expect(service.stop(hostId)).toMatchObject({ watchdogState: 'stopped', restartCount: 1 })
    expect(client.btopChannels[1]?.wasClosed).toBe(true)
  })

  it('reports unavailable without treating btop as a telemetry dependency', async () => {
    const client = new FakeClient()
    client.btopInstalled = false
    const manager = new FakeManager(client)
    const service = new BtopService(manager as unknown as SshConnectionManager, pino({ enabled: false }))
    btopServices.push(service)
    await expect(service.start(hostId, 5)).resolves.toMatchObject({ installed: false, watchdogState: 'unavailable' })
    expect(client.btopChannels).toHaveLength(0)
  })
})

class FakeChannel extends Duplex {
  readonly stderr = new PassThrough()
  wasClosed = false
  written = ''
  _read(): void { /* output is pushed explicitly */ }
  _write(chunk: Buffer, _encoding: BufferEncoding, callback: (error?: Error | null) => void): void { this.written += chunk.toString('utf8'); callback() }
  output(value: string): void { if (!this.wasClosed) this.emit('data', value) }
  errorOutput(value: string): void { if (!this.wasClosed) this.stderr.write(value) }
  exit(code: number): void { if (this.wasClosed) return; this.wasClosed = true; this.stderr.end(); this.emit('close', code) }
  close(): void { this.exit(0) }
}

type ExecCallback = (error: Error | undefined, channel: FakeChannel) => void
interface ExecOptions { pty?: unknown }
class FakeClient {
  readonly collectors: FakeChannel[] = []
  readonly btopChannels: FakeChannel[] = []
  readonly commands: string[] = []
  btopInstalled = true

  exec(command: string, optionsOrCallback: ExecOptions | ExecCallback, callback?: ExecCallback): void {
    this.commands.push(command)
    const done = typeof optionsOrCallback === 'function' ? optionsOrCallback : callback
    if (!done) throw new Error('Missing exec callback')
    const channel = new FakeChannel()
    done(undefined, channel)
    if (command.includes('python3 -u -')) this.collectors.push(channel)
    else if (command.startsWith('ps -p ')) queueMicrotask(() => { channel.output('alice sleep 60\n'); channel.exit(0) })
    else if (command.startsWith('kill -')) queueMicrotask(() => channel.exit(0))
    else if (command.includes('btop --version')) queueMicrotask(() => { if (this.btopInstalled) channel.output('btop version 1.4.0\n'); channel.exit(this.btopInstalled ? 0 : 127) })
    else if (command === 'btop') this.btopChannels.push(channel)
  }
}

class FakeManager {
  online = true
  constructor(readonly client: FakeClient) {}
  getOnlineClient(): SshClient { if (!this.online) throw new Error('Host is not connected'); return this.client as unknown as SshClient }
}

function createFixture(options: { timeoutFloorMs?: number; intervalTimeoutFactorMs?: number } = {}): { service: TelemetryService; client: FakeClient; manager: FakeManager } {
  const client = new FakeClient()
  const manager = new FakeManager(client)
  const service = new TelemetryService(manager as unknown as SshConnectionManager, () => Promise.resolve(settings), () => Promise.resolve('# collector'), pino({ enabled: false }), { restartBaseMs: 10, random: () => 0, ...options })
  services.push(service)
  return { service, client, manager }
}

function payload(): Record<string, unknown> {
  return {
    schemaVersion: 1,
    capturedAt: new Date().toISOString(),
    hostname: 'fixture',
    currentUser: 'alice',
    cpu: { totalPercent: 12.5, perCorePercent: [10, 15], loadAverage: [0.1, 0.2, 0.3], temperatureC: null },
    memory: { totalBytes: 1024, usedBytes: 512, swapTotalBytes: 0, swapUsedBytes: 0 },
    network: { receivedBytes: 100, sentBytes: 50, receiveBytesPerSecond: 10, sendBytesPerSecond: 5 },
    disks: [{ mount: '/', totalBytes: 1000, usedBytes: 400, availableBytes: 600 }],
    processes: [{ pid: 42, ppid: 1, user: 'alice', cpuPercent: 1, memoryPercent: 0.5, state: 'S', elapsed: '60', command: 'sleep 60' }],
    gpus: [],
    gpuProcesses: [],
    uptimeSeconds: 100
  }
}

async function waitFor(predicate: () => boolean, timeoutMs = 500): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (predicate()) return
    await new Promise((resolve) => setTimeout(resolve, 5))
  }
  throw new Error('Timed out waiting for service state')
}
