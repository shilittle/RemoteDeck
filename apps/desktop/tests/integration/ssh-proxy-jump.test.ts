import { mkdtemp, rm } from 'node:fs/promises'
import { connect as connectTcp } from 'node:net'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Server, utils } from 'ssh2'
import type { Connection } from 'ssh2'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { ProfileRepository } from '../../src/core/hosts/profile-repository'
import { SshConnectionManager } from '../../src/core/ssh/connection-manager'

let directory = ''
let jumpServer: Server
let targetServer: Server
let jumpPort = 0
let targetPort = 0
const serverConnections = new Set<Connection>()

beforeAll(async () => {
  directory = await mkdtemp(join(tmpdir(), 'remotedeck-jump-integration-'))
  jumpServer = new Server({ hostKeys: [utils.generateKeyPairSync('ed25519').private] }, configureJump)
  targetServer = new Server({ hostKeys: [utils.generateKeyPairSync('ed25519').private] }, configureTarget)
  jumpPort = await listen(jumpServer)
  targetPort = await listen(targetServer)
})

afterAll(async () => {
  for (const connection of serverConnections) connection.end()
  await Promise.all([close(jumpServer), close(targetServer)])
  await rm(directory, { recursive: true, force: true })
})

describe('single-level ProxyJump', () => {
  it('verifies both host keys and uses separate in-memory credentials', async () => {
    const repository = new ProfileRepository(join(directory, 'profiles.json'))
    const advanced = { connectTimeoutSeconds: 5, serverAliveIntervalSeconds: 30, serverAliveCountMax: 3, tcpKeepAlive: true, compression: false, identitiesOnly: false }
    const jump = await repository.create({ alias: 'jump', hostname: '127.0.0.1', port: jumpPort, username: 'jump-user', groups: [], auth: { name: 'jump password', method: 'password' }, advanced })
    const target = await repository.create({ alias: 'target', hostname: '127.0.0.1', port: targetPort, username: 'target-user', groups: [], jumpHostId: jump.host.id, auth: { name: 'target password', method: 'password' }, advanced })
    const manager = new SshConnectionManager(repository)

    const first = await manager.connect(target.host.id, { password: 'target-secret', jump: { password: 'jump-secret' } })
    expect(first.hostKeyCandidate).toMatchObject({ hostId: jump.host.id, mismatch: false })
    if (!first.hostKeyCandidate) throw new Error('Expected jump host-key candidate')
    await manager.acceptCandidate(first.hostKeyCandidate.id)

    const second = await manager.connect(target.host.id, { password: 'target-secret', jump: { password: 'jump-secret' } })
    expect(second.hostKeyCandidate).toMatchObject({ hostId: target.host.id, mismatch: false })
    if (!second.hostKeyCandidate) throw new Error('Expected target host-key candidate')
    await manager.acceptCandidate(second.hostKeyCandidate.id)

    const connected = await manager.connect(target.host.id, { password: 'target-secret', jump: { password: 'jump-secret' } })
    expect(connected).toMatchObject({ state: 'online', capabilities: { shell: true, python3: true, writableWorkspace: true } })
    expect(await repository.listHostKeys()).toHaveLength(2)
    await manager.disconnectAll()
  })
})

function configureJump(client: Connection): void {
  track(client)
  client.on('authentication', (context) => context.method === 'password' && context.username === 'jump-user' && context.password === 'jump-secret' ? context.accept() : context.reject(['password']))
  client.on('ready', () => {
    client.on('tcpip', (accept, reject, info) => {
      const socket = connectTcp(info.destPort, info.destIP)
      socket.once('connect', () => {
        const channel = accept()
        socket.pipe(channel).pipe(socket)
      })
      socket.once('error', () => reject())
    })
  })
}

function configureTarget(client: Connection): void {
  track(client)
  client.on('authentication', (context) => context.method === 'password' && context.username === 'target-user' && context.password === 'target-secret' ? context.accept() : context.reject(['password']))
  client.on('ready', () => {
    client.on('session', (accept) => {
      const session = accept()
      session.on('pty', (acceptPty) => acceptPty())
      session.on('shell', (acceptShell) => { const stream = acceptShell(); stream.end() })
      session.on('sftp', (_acceptSftp, rejectSftp) => rejectSftp())
      session.on('exec', (acceptExec) => { const stream = acceptExec(); stream.write('0 0'); stream.exit(0); stream.end() })
    })
  })
}

function track(client: Connection): void {
  serverConnections.add(client)
  client.on('error', () => undefined)
  client.once('close', () => serverConnections.delete(client))
}

function listen(server: Server): Promise<number> {
  return new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const address = server.address()
      if (!address || typeof address === 'string') reject(new Error('SSH test server did not expose a TCP port'))
      else resolve(address.port)
    })
  })
}

function close(server: Server): Promise<void> { return new Promise((resolve) => server.close(() => resolve())) }
