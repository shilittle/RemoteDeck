import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { SettingsService } from './settings-service'

const directories: string[] = []
afterEach(async () => Promise.all(directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true }))))

describe('SettingsService', () => {
  it('persists validated patches across service instances', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'remotedeck-settings-'))
    directories.push(directory)
    const file = join(directory, 'settings.json')
    const first = new SettingsService(file)
    await first.update({ terminalFontSize: 18, closeToTray: false })
    const second = new SettingsService(file)
    expect(await second.get()).toMatchObject({ schemaVersion: 1, terminalFontSize: 18, closeToTray: false })
  })

  it('rejects invalid patch values', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'remotedeck-settings-'))
    directories.push(directory)
    const service = new SettingsService(join(directory, 'settings.json'))
    await expect(service.update({ terminalFontSize: 100 })).rejects.toThrow()
  })
})

