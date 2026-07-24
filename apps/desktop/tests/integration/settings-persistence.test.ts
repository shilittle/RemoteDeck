import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, describe, expect, it } from 'vitest'
import { SettingsService } from '../../src/core/settings-service'

const directories: string[] = []
afterAll(async () => Promise.all(directories.map((directory) => rm(directory, { recursive: true, force: true }))))

describe('settings persistence integration', () => {
  it('round-trips concurrent changes through a fresh service boundary', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'remotedeck-integration-'))
    directories.push(directory)
    const file = join(directory, 'settings.json')
    const writer = new SettingsService(file)
    await writer.update({ terminalFontFamily: 'JetBrains Mono' })
    await writer.update({ telemetryIntervalSeconds: 7 })
    const reader = new SettingsService(file)
    expect(await reader.get()).toMatchObject({ terminalFontFamily: 'JetBrains Mono', telemetryIntervalSeconds: 7 })
  })
})

