import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { ProfileRepository } from '../hosts/profile-repository'
import { ensureManagedInclude, OpenSshConfigManager, parseOpenSshConfig, renderManagedConfig } from './open-ssh-config'

const directories: string[] = []
afterEach(async () => Promise.all(directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true }))))

describe('OpenSSH config interoperability', () => {
  it('parses supported directives and preserves unsupported lines', () => {
    const [host] = parseOpenSshConfig(`
Host gpu-lab
  HostName "gpu example.test"
  User dev
  Port 2222
  IdentityFile "C:/Keys/my key"
  IdentitiesOnly yes
  ProxyJump gateway
  ServerAliveInterval 20
  ServerAliveCountMax 4
  TCPKeepAlive no
  ConnectTimeout 9
  Compression yes
  LocalForward 9000 127.0.0.1:9000
  RemoteForward 17890 127.0.0.1:7890
  CanonicalizeHostname yes
`)
    expect(host).toMatchObject({ alias: 'gpu-lab', hostname: 'gpu example.test', username: 'dev', port: 2222, identityFile: 'C:/Keys/my key', proxyJump: 'gateway', compression: true, tcpKeepAlive: false })
    expect(host?.localForwards).toHaveLength(1)
    expect(host?.remoteForwards).toHaveLength(1)
    expect(host?.unsupported).toContain('  CanonicalizeHostname yes')
  })

  it('adds one normalized Include without replacing comments', () => {
    const source = '# keep this comment\nHost existing\n  User dev\n'
    const once = ensureManagedInclude(source, 'C:\\Users\\Dev User\\.ssh\\remotedeck.conf')
    const twice = ensureManagedInclude(once, 'c:/users/dev user/.ssh/remotedeck.conf')
    expect(twice).toBe(once)
    expect(twice).toContain('# keep this comment')
    expect(twice.match(/^Include /gm)).toHaveLength(1)
  })

  it('writes managed config, backs up user config, and stays idempotent', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'remotedeck-ssh-config-'))
    directories.push(directory)
    const repository = new ProfileRepository(join(directory, 'profiles.json'))
    await repository.create({ alias: 'lab', hostname: 'linux.test', port: 22, username: 'dev', groups: [], auth: { name: 'agent', method: 'agent', agent: 'windows_openssh' }, advanced: { connectTimeoutSeconds: 15, serverAliveIntervalSeconds: 30, serverAliveCountMax: 3, tcpKeepAlive: true, compression: false, identitiesOnly: false } })
    const manager = new OpenSshConfigManager(repository)
    const userConfig = join(directory, 'config')
    await writeFile(userConfig, '# preserved\n', 'utf8')
    const first = await manager.writeManaged(userConfig)
    expect(first.backupPath).toBeDefined()
    const afterFirst = await readFile(userConfig, 'utf8')
    const second = await manager.writeManaged(userConfig)
    expect(second.backupPath).toBeUndefined()
    expect(await readFile(userConfig, 'utf8')).toBe(afterFirst)
    const managed = await readFile(join(directory, 'remotedeck.conf'), 'utf8')
    expect(parseOpenSshConfig(managed)).toHaveLength(1)
    expect(renderManagedConfig(await repository.list())).toBe(managed)
  })

  it('imports ProxyJump and both forwarding directions without losing data', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'remotedeck-ssh-import-'))
    directories.push(directory)
    const repository = new ProfileRepository(join(directory, 'profiles.json'))
    const manager = new OpenSshConfigManager(repository)
    const config = join(directory, 'config')
    await writeFile(config, `
Host gateway
  HostName gateway.example.test
  User dev
Host worker
  HostName worker.example.test
  User dev
  ProxyJump gateway
  LocalForward 9000 127.0.0.1:9001
  RemoteForward 17890 127.0.0.1:7890
`, 'utf8')

    const result = await manager.importFile(config, () => 'idle')
    expect(result.imported).toHaveLength(2)
    const profiles = await repository.list()
    const gateway = profiles.find((item) => item.host.alias === 'gateway')
    const worker = profiles.find((item) => item.host.alias === 'worker')
    expect(worker?.host.jumpHostId).toBe(gateway?.host.id)
    expect(await repository.listTunnels()).toEqual(expect.arrayContaining([
      expect.objectContaining({ hostId: worker?.host.id, direction: 'local', sourcePort: 9000, targetHost: '127.0.0.1', targetPort: 9001 }),
      expect.objectContaining({ hostId: worker?.host.id, direction: 'remote', sourcePort: 17890, targetHost: '127.0.0.1', targetPort: 7890 })
    ]))
  })
})
