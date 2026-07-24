import { homedir } from 'node:os'
import { isAbsolute, join } from 'node:path'
import type { AppSettings } from '../../protocol/settings'
import type { HostCreateRequest, HostImportResult, HostListItem, HostUpdateRequest } from '../../protocol/ssh'
import type { SettingsService } from '../settings-service'
import type { OpenSshConfigManager } from '../ssh-config/open-ssh-config'
import type { SshConnectionManager } from '../ssh/connection-manager'
import type { ProfileRepository, ResolvedHostProfile } from './profile-repository'

export class HostService {
  readonly #repository: ProfileRepository
  readonly #connections: SshConnectionManager
  readonly #config: OpenSshConfigManager
  readonly #settings: SettingsService

  constructor(repository: ProfileRepository, connections: SshConnectionManager, config: OpenSshConfigManager, settings: SettingsService) {
    this.#repository = repository
    this.#connections = connections
    this.#config = config
    this.#settings = settings
  }

  async list(): Promise<HostListItem[]> {
    return (await this.#repository.list()).map((profile) => this.#toListItem(profile))
  }

  async create(request: HostCreateRequest): Promise<HostListItem> {
    const profile = await this.#repository.create(request)
    await this.#writeManagedConfig()
    return this.#toListItem(profile)
  }

  async update(request: HostUpdateRequest): Promise<HostListItem> {
    const profile = await this.#repository.update(request)
    await this.#writeManagedConfig()
    return this.#toListItem(profile)
  }

  async delete(hostId: string): Promise<boolean> {
    await this.#connections.disconnect(hostId)
    const deleted = await this.#repository.delete(hostId)
    if (deleted) await this.#writeManagedConfig()
    return deleted
  }

  async importFile(configPath: string): Promise<HostImportResult> {
    const result = await this.#config.importFile(expandUserPath(configPath), (hostId) => this.#connections.stateFor(hostId))
    if (!result.duplicate && result.imported.length > 0) await this.#writeManagedConfig()
    return result
  }

  syncManagedConfig(): Promise<void> {
    return this.#writeManagedConfig()
  }

  async #writeManagedConfig(): Promise<void> {
    const settings = await this.#settings.get()
    await this.#config.writeManaged(resolveConfigPath(settings))
  }

  #toListItem(profile: ResolvedHostProfile): HostListItem {
    const snapshot = this.#connections.snapshot(profile.host.id)
    return {
      ...profile,
      state: snapshot.state,
      ...(snapshot.errorMessage ? { lastError: snapshot.errorMessage } : {})
    }
  }
}

function resolveConfigPath(settings: AppSettings): string {
  return settings.sshConfigPath ? expandUserPath(settings.sshConfigPath) : join(homedir(), '.ssh', 'config')
}

function expandUserPath(value: string): string {
  if (value === '~') return homedir()
  if (value.startsWith('~/') || value.startsWith('~\\')) return join(homedir(), value.slice(2))
  return isAbsolute(value) ? value : value
}
