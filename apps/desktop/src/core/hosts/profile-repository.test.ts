import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { ProfileRepository } from './profile-repository'

const directories: string[] = []
afterEach(async () => Promise.all(directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true }))))

const advanced = { connectTimeoutSeconds: 15, serverAliveIntervalSeconds: 30, serverAliveCountMax: 3, tcpKeepAlive: true, compression: false, identitiesOnly: false }

describe('host profile relationships', () => {
  it('enforces one jump level and clears references without deleting dependent hosts', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'remotedeck-profiles-'))
    directories.push(directory)
    const repository = new ProfileRepository(join(directory, 'profiles.json'))
    const gateway = await repository.create({ alias: 'gateway', hostname: 'gateway.test', port: 22, username: 'dev', groups: [], auth: { name: 'agent', method: 'agent', agent: 'windows_openssh' }, advanced })
    const worker = await repository.create({ alias: 'worker', hostname: 'worker.test', port: 22, username: 'dev', groups: [], jumpHostId: gateway.host.id, auth: { name: 'agent', method: 'agent', agent: 'windows_openssh' }, advanced })
    expect(worker.host.monitorEnabled).toBe(true)
    expect((await repository.update({ id: worker.host.id, patch: { monitorEnabled: false } })).host.monitorEnabled).toBe(false)
    await expect(repository.create({ alias: 'nested', hostname: 'nested.test', port: 22, username: 'dev', groups: [], jumpHostId: worker.host.id, auth: { name: 'agent', method: 'agent', agent: 'windows_openssh' }, advanced })).rejects.toThrow(/one ProxyJump level/)
    await expect(repository.update({ id: worker.host.id, patch: { jumpHostId: worker.host.id } })).rejects.toThrow(/itself/)

    await repository.delete(gateway.host.id)
    const remaining = await repository.list()
    expect(remaining).toHaveLength(1)
    expect(remaining[0]?.host).toMatchObject({ id: worker.host.id, alias: 'worker' })
    expect(remaining[0]?.host.jumpHostId).toBeUndefined()
  })

  it('allows an explicit edit back to a direct connection', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'remotedeck-profiles-direct-'))
    directories.push(directory)
    const repository = new ProfileRepository(join(directory, 'profiles.json'))
    const gateway = await repository.create({ alias: 'gateway', hostname: 'gateway.test', port: 22, username: 'dev', groups: [], auth: { name: 'agent', method: 'agent', agent: 'windows_openssh' }, advanced })
    const worker = await repository.create({ alias: 'worker', hostname: 'worker.test', port: 22, username: 'dev', groups: [], jumpHostId: gateway.host.id, auth: { name: 'agent', method: 'agent', agent: 'windows_openssh' }, advanced })
    const updated = await repository.update({ id: worker.host.id, patch: { jumpHostId: null } })
    expect(updated.host.jumpHostId).toBeUndefined()
  })
})
