import type { AppSettings, AppSettingsPatch } from '../protocol/settings'
import { appSettingsSchema, defaultAppSettings } from '../protocol/settings'
import { AtomicJsonStore } from './persistence/atomic-json-store'

export class SettingsService extends EventEmitter {
  readonly #store: AtomicJsonStore<AppSettings>

  constructor(filePath: string) {
    super()
    this.#store = new AtomicJsonStore(filePath, appSettingsSchema, defaultAppSettings)
  }

  get(): Promise<AppSettings> {
    return this.#store.load()
  }

  async update(patch: AppSettingsPatch): Promise<AppSettings> {
    const settings = await this.#store.update((current) => appSettingsSchema.parse({ ...current, ...patch, schemaVersion: 1 }))
    this.emit('updated', settings)
    return settings
  }
}
import { EventEmitter } from 'node:events'
