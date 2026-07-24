import { EventEmitter } from 'node:events'
import { mkdtemp, rm } from 'node:fs/promises'
import { connect, createServer } from 'node:net'
import type { AddressInfo, Server, Socket } from 'node:net'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import pino from 'pino'
import type { Client as SshClient } from 'ssh2'
import { afterEach, describe, expect, it } from 'vitest'
import { ProfileRepository } from '../hosts/profile-repository'
import type { SshConnectionManager } from '../ssh/connection-manager'
import { TunnelService } from './tunnel-service'

const directories: string[] = []
const servers: Server[] = []
const sockets = new Set<Socket>()
const advanced = { connectTimeoutSeconds: 5, serverAliveIntervalSeconds: 30, serverAliveCountMax: 3, tcpKeepAlive: true, compression: false, identitiesOnly: false }

afterEach(async () => {
  sockets.forEach((socket) => socket.destroy())
  sockets.clear()
  await Promise.all(servers.splice(0).map((server) => new Promise<void>((resolve) => server.close(() => resolve()))))
  await Promise.all(directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })))
})

describe('TunnelService', () => {
  it('runs isolated local forwards and reconnects a forced SSH disconnect', async () => {
    const targetPort = await echoServer()
    const localPortA = await freePort()
    const localPortB = await freePort()
    const fixture = await createFixture()
    const first = await fixture.repository.addTunnel(profileInput(fixture.hostId, 'local A', 'local', localPortA, targetPort))
    const second = await fixture.repository.addTunnel(profileInput(fixture.hostId, 'local B', 'local', localPortB, targetPort))

    await fixture.service.start(first.id, {})
    await fixture.service.start(second.id, {})
    await expect(exchange(localPortA, 'alpha')).resolves.toBe('alpha')
    await expect(exchange(localPortB, 'bravo')).resolves.toBe('bravo')

    await fixture.service.stop(first.id)
    await expect(exchange(localPortA, 'closed')).rejects.toThrow()
    await expect(exchange(localPortB, 'still-open')).resolves.toBe('still-open')

    fixture.clients.at(-1)?.emit('close')
    await waitFor(async () => (await fixture.service.list()).find((item) => item.profile.id === second.id)?.state === 'waiting')
    await waitFor(async () => (await fixture.service.list()).find((item) => item.profile.id === second.id)?.state === 'online', 2500)
    const recovered = (await fixture.service.list()).find((item) => item.profile.id === second.id)
    expect(recovered).toMatchObject({ state: 'online', reconnectCount: 1 })
    expect(fixture.clients).toHaveLength(3)
    await expect(exchange(localPortB, 'recovered')).resolves.toBe('recovered')
    await fixture.service.stopAll()
  })

  it('preflights a conflicting local port and cancels its retry on stop', async () => {
    const blockedPort = await listen(createServer())
    const fixture = await createFixture()
    const profile = await fixture.repository.addTunnel(profileInput(fixture.hostId, 'blocked', 'local', blockedPort, 9))
    const snapshot = await fixture.service.start(profile.id, {})
    expect(snapshot).toMatchObject({ state: 'waiting', reconnectCount: 1 })
    expect(snapshot.lastError).toMatch(/EADDRINUSE|address already in use/i)
    await fixture.service.stop(profile.id)
    await new Promise((resolve) => setTimeout(resolve, 1100))
    expect((await fixture.service.list())[0]).toMatchObject({ state: 'stopped', reconnectCount: 1 })
    expect(fixture.clients).toHaveLength(0)
  })

  it('routes RemoteForward channels and unregisters only its owned remote bind', async () => {
    const targetPort = await echoServer()
    const remotePort = await freePort()
    const fixture = await createFixture()
    const profile = await fixture.repository.addTunnel(profileInput(fixture.hostId, 'remote', 'remote', remotePort, targetPort))
    await fixture.service.start(profile.id, {})
    const client = fixture.clients[0]
    if (!client) throw new Error('Expected a dedicated SSH connection')
    expect(client.forwarded).toEqual([`127.0.0.1:${String(remotePort)}`])

    const bridgePort = await listen(createServer((socket) => {
      client.emit('tcp connection', { destIP: '127.0.0.1', destPort: remotePort }, () => socket, () => socket.destroy())
    }))
    await expect(exchange(bridgePort, 'reverse')).resolves.toBe('reverse')
    await fixture.service.stop(profile.id)
    expect(client.unforwarded).toEqual([`127.0.0.1:${String(remotePort)}`])
  })
})

class FakeSshClient extends EventEmitter {
  readonly forwarded: string[] = []
  readonly unforwarded: string[] = []

  forwardOut(_sourceHost: string, _sourcePort: number, targetHost: string, targetPort: number, callback: (error: Error | undefined, stream?: Socket) => void): void {
    const socket = connect(targetPort, targetHost)
    socket.once('connect', () => callback(undefined, socket))
    socket.once('error', (error) => callback(error))
  }

  forwardIn(address: string, port: number, callback: (error: Error | undefined, actualPort: number) => void): void {
    this.forwarded.push(`${address}:${String(port)}`)
    callback(undefined, port)
  }

  unforwardIn(address: string, port: number, callback: (error?: Error) => void): void {
    this.unforwarded.push(`${address}:${String(port)}`)
    callback()
  }

  end(): void { queueMicrotask(() => this.emit('close')) }
}

async function createFixture(): Promise<{ repository: ProfileRepository; service: TunnelService; hostId: string; clients: FakeSshClient[] }> {
  const directory = await mkdtemp(join(tmpdir(), 'remotedeck-tunnel-'))
  directories.push(directory)
  const repository = new ProfileRepository(join(directory, 'profiles.json'))
  const host = await repository.create({ alias: 'tunnel-test', hostname: '127.0.0.1', port: 22, username: 'test', groups: [], auth: { name: 'agent', method: 'agent', agent: 'windows_openssh' }, advanced })
  const clients: FakeSshClient[] = []
  const connections = {
    openDedicated: () => {
      const client = new FakeSshClient()
      clients.push(client)
      return Promise.resolve({ client: client as unknown as SshClient, close: () => client.end() })
    }
  } as unknown as SshConnectionManager
  return { repository, service: new TunnelService(repository, connections, pino({ enabled: false })), hostId: host.host.id, clients }
}

function profileInput(hostId: string, name: string, direction: 'local' | 'remote', sourcePort: number, targetPort: number) {
  return { hostId, name, direction, bindAddress: '127.0.0.1', sourcePort, targetHost: '127.0.0.1', targetPort, autoStart: false, healthCheck: { type: 'tcp' as const, intervalSeconds: 2, timeoutMs: 500 } }
}

async function echoServer(): Promise<number> { return await listen(createServer((socket) => socket.pipe(socket))) }

async function listen(server: Server): Promise<number> {
  servers.push(server)
  server.on('connection', (socket) => { sockets.add(socket); socket.once('close', () => sockets.delete(socket)) })
  await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', () => resolve()) })
  return (server.address() as AddressInfo).port
}

async function freePort(): Promise<number> {
  const server = createServer()
  const port = await listen(server)
  servers.splice(servers.indexOf(server), 1)
  await new Promise<void>((resolve) => server.close(() => resolve()))
  return port
}

function exchange(port: number, value: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const socket = connect(port, '127.0.0.1')
    const chunks: Buffer[] = []
    socket.setTimeout(1500, () => socket.destroy(new Error('exchange timeout')))
    socket.once('error', reject)
    socket.on('data', (chunk: Buffer) => { chunks.push(chunk); if (Buffer.concat(chunks).toString('utf8').length >= value.length) { const result = Buffer.concat(chunks).toString('utf8'); socket.destroy(); resolve(result) } })
    socket.once('connect', () => socket.write(value))
  })
}

async function waitFor(predicate: () => Promise<boolean>, timeoutMs = 1500): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (await predicate()) return
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
  throw new Error('Timed out waiting for tunnel state')
}
