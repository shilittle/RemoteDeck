import type { AppSettings, AppSettingsPatch } from '../protocol/settings'
import { appSettingsSchema, defaultAppSettings } from '../protocol/settings'
import { AtomicJsonStore } from './persistence/atomic-json-store'

export class SettingsService {
  readonly #store: AtomicJsonStore<AppSettings>

  constructor(filePath: string) {
    this.#store = new AtomicJsonStore(filePath, appSettingsSchema, defaultAppSettings)
  }

  get(): Promise<AppSettings> {
    return this.#store.load()
  }

  update(patch: AppSettingsPatch): Promise<AppSettings> {
    return this.#store.update((current) => appSettingsSchema.parse({ ...current, ...patch, schemaVersion: 1 }))
  }
}

