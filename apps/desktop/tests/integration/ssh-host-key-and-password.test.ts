import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Server } from 'ssh2'
import type { Connection } from 'ssh2'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { ProfileRepository } from '../../src/core/hosts/profile-repository'
import { generateUsableEd25519KeyPair } from '../../src/core/keys/ed25519-key-pair'
import { SshConnectionManager } from '../../src/core/ssh/connection-manager'

let directory = ''
let server: Server | undefined
let port = 0
const connections = new Set<Connection>()

beforeAll(async () => {
  directory = await mkdtemp(join(tmpdir(), 'remotedeck-ssh-integration-'))
  const keyPair = generateUsableEd25519KeyPair({ comment: 'integration' })
  const createdServer = new Server({ hostKeys: [keyPair.private] }, (client) => configureClient(client))
  server = createdServer
  await new Promise<void>((resolve, reject) => {
    createdServer.once('error', reject)
    createdServer.listen(0, '127.0.0.1', () => {
      const address = createdServer.address()
      if (!address || typeof address === 'string') { reject(new Error('SSH test server did not expose a TCP port')); return }
      port = address.port
      resolve()
    })
  })
})

afterAll(async () => {
  for (const connection of connections) connection.end()
  const activeServer = server
  if (activeServer) await new Promise<void>((resolve) => activeServer.close(() => resolve()))
  await rm(directory, { recursive: true, force: true })
})

describe('real ssh2 client/server authentication flow', () => {
  it('requires first-use acceptance, then authenticates with password', async () => {
    const repository = new ProfileRepository(join(directory, 'profiles.json'))
    const profile = await repository.create({ alias: 'integration', hostname: '127.0.0.1', port, username: 'developer', groups: [], auth: { name: 'password', method: 'password' }, advanced: { connectTimeoutSeconds: 5, serverAliveIntervalSeconds: 30, serverAliveCountMax: 3, tcpKeepAlive: true, compression: false, identitiesOnly: false } })
    const manager = new SshConnectionManager(repository)
    const first = await manager.connect(profile.host.id, { password: 'correct-horse' })
    expect(first).toMatchObject({ state: 'awaiting_host_key', errorCode: 'HOST_KEY_UNKNOWN', hostKeyCandidate: { mismatch: false } })
    const candidate = first.hostKeyCandidate
    if (!candidate) throw new Error('Expected host-key candidate')
    await manager.acceptCandidate(candidate.id)
    const second = await manager.connect(profile.host.id, { password: 'correct-horse' })
    expect(second).toMatchObject({ state: 'online', capabilities: { shell: true, sftp: false, python3: true, writableWorkspace: true } })
    expect((await repository.listHostKeys())[0]?.sha256Fingerprint).toBe(candidate.sha256Fingerprint)
    await manager.disconnectAll()
  })

  it('completes keyboard-interactive authentication with provided in-memory answers', async () => {
    const repository = new ProfileRepository(join(directory, 'profiles-keyboard.json'))
    const profile = await repository.create({ alias: 'keyboard', hostname: '127.0.0.1', port, username: 'interactive', groups: [], auth: { name: 'keyboard', method: 'keyboard_interactive' }, advanced: { connectTimeoutSeconds: 5, serverAliveIntervalSeconds: 30, serverAliveCountMax: 3, tcpKeepAlive: true, compression: false, identitiesOnly: false } })
    const manager = new SshConnectionManager(repository)
    const first = await manager.connect(profile.host.id, { keyboardInteractiveAnswers: ['kbd-secret'] })
    if (!first.hostKeyCandidate) throw new Error('Expected keyboard host-key candidate')
    await manager.acceptCandidate(first.hostKeyCandidate.id)
    const connected = await manager.connect(profile.host.id, { keyboardInteractiveAnswers: ['kbd-secret'] })
    expect(connected.state).toBe('online')
    await manager.disconnectAll()
  })
})

function configureClient(client: Connection): void {
  connections.add(client)
  client.on('error', () => undefined)
  client.once('close', () => connections.delete(client))
  client.on('authentication', (context) => {
    if (context.method === 'password' && context.username === 'developer' && context.password === 'correct-horse') context.accept()
    else if (context.method === 'keyboard-interactive' && context.username === 'interactive') {
      context.prompt([{ prompt: 'Verification code: ', echo: false }], (answers) => answers[0] === 'kbd-secret' ? context.accept() : context.reject())
    } else context.reject(['password', 'keyboard-interactive'])
  })
  client.on('ready', () => {
    client.on('session', (accept) => {
      const session = accept()
      session.on('pty', (acceptPty) => acceptPty())
      session.on('shell', (acceptShell) => { const stream = acceptShell(); stream.end() })
      session.on('sftp', (_acceptSftp, rejectSftp) => rejectSftp())
      session.on('exec', (acceptExec) => {
        const stream = acceptExec()
        stream.write('0 0')
        stream.exit(0)
        stream.end()
      })
    })
  })
}
