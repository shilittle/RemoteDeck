import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { ProfileRepository } from '../hosts/profile-repository'
import { KeyService } from './key-service'

const directories: string[] = []
afterEach(async () => Promise.all(directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true }))))

describe('Ed25519 key generation', () => {
  it('writes an encrypted OpenSSH key pair and refuses overwrite', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'remotedeck-key-'))
    directories.push(directory)
    const privateKeyPath = join(directory, 'id_ed25519')
    const repository = new ProfileRepository(join(directory, 'profiles.json'))
    const service = new KeyService({} as never, repository)
    const generated = await service.generate({ privateKeyPath, comment: 'RemoteDeck test', passphrase: 'unit-passphrase' })
    expect(generated.fingerprint).toMatch(/^SHA256:/)
    expect(await readFile(privateKeyPath, 'utf8')).toContain('OPENSSH PRIVATE KEY')
    expect(await readFile(`${privateKeyPath}.pub`, 'utf8')).toMatch(/^ssh-ed25519 /)
    expect(await repository.listPrivateKeyMetadata()).toEqual([
      expect.objectContaining({ path: privateKeyPath, format: 'openssh', encrypted: true, fingerprint: generated.fingerprint })
    ])
    await expect(service.generate({ privateKeyPath, comment: 'again' })).rejects.toThrow()
  })
})
