import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { ProfileRepository } from '../hosts/profile-repository'
import { SettingsService } from '../settings-service'
import { LegacyMigrationService } from './legacy-migration-service'

const directories: string[] = []
afterEach(async () => Promise.all(directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true }))))

describe('legacy LabPulse migration', () => {
  it('previews exact risky data, applies mappings, and remains idempotent by source hash', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'remotedeck-legacy-'))
    directories.push(directory)
    const profiles = new ProfileRepository(join(directory, 'profiles.json'))
    const settings = new SettingsService(join(directory, 'settings.json'))
    const existing = await profiles.create({ alias: 'imported-lab', hostname: '10.0.0.9', port: 22, username: 'researcher', groups: [], auth: { name: 'password', method: 'password' }, advanced: { connectTimeoutSeconds: 15, serverAliveIntervalSeconds: 30, serverAliveCountMax: 3, tcpKeepAlive: true, compression: false, identitiesOnly: false } })
    let creates = 0
    const hosts = { create: () => { creates += 1; return Promise.resolve({ ...existing, state: 'idle' }) } }
    const service = new LegacyMigrationService(profiles, hosts as never, settings)
    const sourcePath = resolve(import.meta.dirname, '../../../../../legacy/labpulse-v0.1.0/config.json')
    const preview = await service.preview(sourcePath)
    expect(preview).toMatchObject({ duplicate: false, sshAlias: 'lab', forwardAlias: 'lab-codex', telemetryIntervalSeconds: 3, btop: { autoRestart: true, rotationMinutes: 15 }, tunnel: { sourcePort: 17890, targetPort: 7890 } })
    expect(preview.tunnel?.cleanupCommand).toContain('fuser -k')
    expect(preview.commands).toHaveLength(8)
    expect(preview.legacyRiskRules.length).toBeGreaterThan(0)

    const request = { sourcePath, sourceHash: preview.sourceHash, host: { alias: 'imported-lab', hostname: '10.0.0.9', port: 22, username: 'researcher', workspacePath: '/srv/lab', auth: { name: 'password', method: 'password' as const } }, includeTunnel: true, includeCommands: true }
    const result = await service.apply(request)
    expect(result).toMatchObject({ duplicate: false, hostId: existing.host.id })
    expect(result.tunnelIds).toHaveLength(1)
    expect(result.commandIds).toHaveLength(8)
    expect((await profiles.listTunnels())[0]?.legacyCleanupHook).toMatchObject({ authorized: false })
    expect(await profiles.listLegacyRiskRules()).toEqual(preview.legacyRiskRules)
    expect(await settings.get()).toMatchObject({ telemetryIntervalSeconds: 3, btopWatchdogEnabled: true, btopRotationMinutes: 15 })

    await expect(service.apply(request)).resolves.toMatchObject({ duplicate: true, hostId: null, tunnelIds: [], commandIds: [] })
    expect(creates).toBe(1)
  })
})
