import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { KeyService } from '../../src/core/keys/key-service'
import { ProfileRepository } from '../../src/core/hosts/profile-repository'
import { SshConnectionManager } from '../../src/core/ssh/connection-manager'
import { TerminalService } from '../../src/core/terminal/terminal-service'
import type { TerminalEvent } from '../../src/protocol/terminal'

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
    const terminals = new TerminalService(connections, repository)

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
    let output = ''
    terminals.on('event', (event: TerminalEvent) => { if (event.type === 'data') output += event.data })
    const terminal = await terminals.create({ hostId: profile.host.id, cwd: '/home/remotedeck', cols: 80, rows: 24 })
    expect(terminal.state).toBe('online')
    terminals.resize({ sessionId: terminal.id, cols: 120, rows: 40 })
    terminals.write(terminal.id, `bash -lc 'printf BASH_OK' && vim --clean -Nu NONE -n -es -c q && printf ' VIM_OK' && tmux -L remotedeck-m3 new-session -d -s m3 'sleep 2' && tmux -L remotedeck-m3 has-session -t m3 && printf ' TMUX_OK' && btop --version | head -n1; mkdir -p -- "$HOME/中文 路径"; cd -- "$HOME/中文 路径"; printf ' PATH_OK:%s ' "$PWD"; stty size; printf 'M3_COMMANDS_DONE\\n'\r`)
    await waitForOutput(() => output, 'M3_COMMANDS_DONE')
    expect(output).toContain('BASH_OK')
    expect(output).toContain('VIM_OK')
    expect(output).toContain('TMUX_OK')
    expect(output).toMatch(/btop version/i)
    expect(output).toContain('PATH_OK:/home/remotedeck/中文 路径')
    expect(output).toContain('40 120')
    terminals.write(terminal.id, 'sleep 30\r')
    await new Promise((resolve) => setTimeout(resolve, 250))
    terminals.write(terminal.id, '\u0003')
    terminals.write(terminal.id, `printf 'CTRL_C_OK\\n'\r`)
    await waitForOutput(() => output, 'CTRL_C_OK')
    terminals.closeAll()
    await connections.disconnectAll()
  })
})

async function waitForOutput(read: () => string, marker: string): Promise<void> {
  const deadline = Date.now() + 15_000
  while (!read().includes(marker)) {
    if (Date.now() >= deadline) throw new Error(`Timed out waiting for terminal marker ${marker}. Output: ${read().slice(-4000)}`)
    await new Promise((resolve) => setTimeout(resolve, 50))
  }
}
