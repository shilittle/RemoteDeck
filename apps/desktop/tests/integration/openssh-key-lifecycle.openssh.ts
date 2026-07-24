import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { KeyService } from '../../src/core/keys/key-service'
import { ProfileRepository } from '../../src/core/hosts/profile-repository'
import { SshConnectionManager } from '../../src/core/ssh/connection-manager'

const host = process.env['REMOTEDECK_OPENSSH_HOST']
const port = Number(process.env['REMOTEDECK_OPENSSH_PORT'])
if (!host || !Number.isInteger(port) || port <= 0) throw new Error('Docker OpenSSH endpoint environment is required')

let directory = ''
beforeAll(async () => { directory = await mkdtemp(join(tmpdir(), 'remotedeck-openssh-')) })
afterAll(async () => rm(directory, { recursive: true, force: true }))

describe('Docker OpenSSH password-to-key lifecycle', () => {
  it('accepts the first fingerprint, deploys Ed25519, and reconnects with the key', async () => {
    const repository = new ProfileRepository(join(directory, 'profiles.json'))
    const profile = await repository.create({ alias: 'docker-openssh', hostname: host, port, username: 'remotedeck', groups: ['integration'], workspacePath: '/home/remotedeck', auth: { name: 'password', method: 'password' }, advanced: { connectTimeoutSeconds: 10, serverAliveIntervalSeconds: 30, serverAliveCountMax: 3, tcpKeepAlive: true, compression: false, identitiesOnly: false } })
    const connections = new SshConnectionManager(repository)
    const keys = new KeyService(connections, repository)

    const first = await connections.connect(profile.host.id, { password: 'remotedeck-test-only' })
    expect(first.state).toBe('awaiting_host_key')
    if (!first.hostKeyCandidate) throw new Error('OpenSSH did not provide a host-key candidate')
    await connections.acceptCandidate(first.hostKeyCandidate.id)

    const passwordConnection = await connections.connect(profile.host.id, { password: 'remotedeck-test-only' })
    expect(passwordConnection).toMatchObject({ state: 'online', capabilities: { shell: true, sftp: true, python3: true, writableWorkspace: true } })

    const privateKeyPath = join(directory, 'id_ed25519')
    const generated = await keys.generate({ privateKeyPath, comment: 'RemoteDeck integration', passphrase: 'integration-passphrase' })
    expect(generated.fingerprint).toMatch(/^SHA256:/)
    const deployed = await keys.deploy({ hostId: profile.host.id, privateKeyPath, passphrase: 'integration-passphrase', makeDefault: true })
    expect(deployed).toMatchObject({ success: true, verified: true, alreadyPresent: false })
    const deduplicated = await keys.deploy({ hostId: profile.host.id, privateKeyPath, passphrase: 'integration-passphrase', makeDefault: true })
    expect(deduplicated).toMatchObject({ success: true, verified: true, alreadyPresent: true })

    await connections.disconnect(profile.host.id)
    const keyConnection = await connections.connect(profile.host.id, { passphrase: 'integration-passphrase' })
    expect(keyConnection.state).toBe('online')
    await connections.disconnectAll()
  })
})
