import { randomUUID } from 'node:crypto'
import { z } from 'zod'
import type { AuthProfile, CommandPreset, HostKeyRecord, HostProfile, TunnelProfile, WorkspaceProfile } from '../../protocol/domain'
import { authProfileSchema, commandPresetSchema, hostKeyRecordSchema, hostProfileSchema, tunnelProfileSchema, workspaceProfileSchema } from '../../protocol/domain'
import type { CommandPresetInput } from '../../protocol/command'
import type { HostCreateRequest, HostUpdateRequest } from '../../protocol/ssh'
import type { PrivateKeyMetadata } from '../../protocol/ssh'
import { privateKeyMetadataSchema } from '../../protocol/ssh'
import { AtomicJsonStore } from '../persistence/atomic-json-store'

const profileDatabaseSchema = z.object({
  schemaVersion: z.literal(1),
  hosts: z.array(hostProfileSchema),
  authProfiles: z.array(authProfileSchema),
  workspaces: z.array(workspaceProfileSchema),
  hostKeys: z.array(hostKeyRecordSchema),
  tunnels: z.array(tunnelProfileSchema).default([]),
  commands: z.array(commandPresetSchema).default([]),
  privateKeys: z.array(privateKeyMetadataSchema).default([]),
  imports: z.array(z.object({
    sourceHash: z.string().regex(/^[a-f0-9]{64}$/),
    unsupported: z.array(z.object({ alias: z.string(), lines: z.array(z.string()) })),
    legacyRiskRules: z.array(z.string().min(1).max(1024)).max(128).default([])
  })).default([])
})

type ProfileDatabase = z.infer<typeof profileDatabaseSchema>
const defaults: ProfileDatabase = { schemaVersion: 1, hosts: [], authProfiles: [], workspaces: [], hostKeys: [], tunnels: [], commands: [], privateKeys: [], imports: [] }

export interface ResolvedHostProfile {
  host: HostProfile
  auth: AuthProfile
  workspace?: WorkspaceProfile
}

export class ProfileRepository {
  readonly #store: AtomicJsonStore<ProfileDatabase>

  constructor(filePath: string) {
    this.#store = new AtomicJsonStore(filePath, profileDatabaseSchema, defaults)
  }

  async list(): Promise<ResolvedHostProfile[]> {
    const data = await this.#store.load()
    return data.hosts.map((host) => resolveHost(data, host))
  }

  async get(hostId: string): Promise<ResolvedHostProfile> {
    const data = await this.#store.load()
    const host = data.hosts.find((item) => item.id === hostId)
    if (!host) throw new Error(`Unknown host profile: ${hostId}`)
    return resolveHost(data, host)
  }

  async create(request: HostCreateRequest): Promise<ResolvedHostProfile> {
    let created: ResolvedHostProfile | undefined
    await this.#store.update((data) => {
      if (data.hosts.some((item) => item.alias.toLowerCase() === request.alias.toLowerCase())) throw new Error(`Host alias already exists: ${request.alias}`)
      assertValidJumpHost(data, request.jumpHostId)
      const now = new Date().toISOString()
      const auth: AuthProfile = authProfileSchema.parse({ schemaVersion: 1, id: randomUUID(), ...request.auth, createdAt: now, updatedAt: now })
      const workspace: WorkspaceProfile | undefined = request.workspacePath
        ? workspaceProfileSchema.parse({ schemaVersion: 1, id: randomUUID(), hostId: '00000000-0000-4000-8000-000000000000', name: '默认工作区', remotePath: request.workspacePath, createdAt: now, updatedAt: now })
        : undefined
      const hostId = randomUUID()
      const fixedWorkspace = workspace ? { ...workspace, hostId } : undefined
      const host: HostProfile = hostProfileSchema.parse({
        schemaVersion: 1,
        id: hostId,
        alias: request.alias,
        hostname: request.hostname,
        port: request.port,
        username: request.username,
        authProfileId: auth.id,
        ...(request.jumpHostId ? { jumpHostId: request.jumpHostId } : {}),
        ...(fixedWorkspace ? { defaultWorkspaceId: fixedWorkspace.id } : {}),
        groups: request.groups,
        advanced: request.advanced,
        monitorEnabled: request.monitorEnabled ?? true,
        createdAt: now,
        updatedAt: now
      })
      created = fixedWorkspace ? { host, auth, workspace: fixedWorkspace } : { host, auth }
      return { ...data, hosts: [...data.hosts, host], authProfiles: [...data.authProfiles, auth], workspaces: fixedWorkspace ? [...data.workspaces, fixedWorkspace] : data.workspaces }
    })
    if (!created) throw new Error('Host profile creation failed')
    return created
  }

  async update(request: HostUpdateRequest): Promise<ResolvedHostProfile> {
    let updated: ResolvedHostProfile | undefined
    await this.#store.update((data) => {
      const index = data.hosts.findIndex((item) => item.id === request.id)
      if (index < 0) throw new Error(`Unknown host profile: ${request.id}`)
      const current = data.hosts[index]
      if (!current) throw new Error(`Unknown host profile: ${request.id}`)
      const now = new Date().toISOString()
      const authIndex = data.authProfiles.findIndex((item) => item.id === current.authProfileId)
      const currentAuth = data.authProfiles[authIndex]
      if (!currentAuth) throw new Error(`Missing auth profile for host: ${current.alias}`)
      const patch = request.patch
      const nextAuth = patch.auth ? authProfileSchema.parse({ ...currentAuth, ...patch.auth, updatedAt: now }) : currentAuth
      assertValidJumpHost(data, patch.jumpHostId, current.id)
      const nextHostInput = {
        ...current,
        ...(patch.alias !== undefined ? { alias: patch.alias } : {}),
        ...(patch.hostname !== undefined ? { hostname: patch.hostname } : {}),
        ...(patch.port !== undefined ? { port: patch.port } : {}),
        ...(patch.username !== undefined ? { username: patch.username } : {}),
        ...(patch.groups !== undefined ? { groups: patch.groups } : {}),
        ...(patch.advanced !== undefined ? { advanced: patch.advanced } : {}),
        ...(patch.monitorEnabled !== undefined ? { monitorEnabled: patch.monitorEnabled } : {}),
        updatedAt: now
      }
      if (patch.jumpHostId === null) delete nextHostInput.jumpHostId
      else if (patch.jumpHostId !== undefined) nextHostInput.jumpHostId = patch.jumpHostId
      const nextHost = hostProfileSchema.parse(nextHostInput)
      if (data.hosts.some((item) => item.id !== current.id && item.alias.toLowerCase() === nextHost.alias.toLowerCase())) throw new Error(`Host alias already exists: ${nextHost.alias}`)
      const hosts = data.hosts.with(index, nextHost)
      const authProfiles = data.authProfiles.with(authIndex, nextAuth)
      let workspaces = data.workspaces
      let workspace = current.defaultWorkspaceId ? data.workspaces.find((item) => item.id === current.defaultWorkspaceId) : undefined
      if (patch.workspacePath !== undefined) {
        if (workspace) {
          const workspaceIndex = workspaces.findIndex((item) => item.id === workspace?.id)
          workspace = workspaceProfileSchema.parse({ ...workspace, remotePath: patch.workspacePath, updatedAt: now })
          workspaces = workspaces.with(workspaceIndex, workspace)
        } else {
          workspace = workspaceProfileSchema.parse({ schemaVersion: 1, id: randomUUID(), hostId: current.id, name: '默认工作区', remotePath: patch.workspacePath, createdAt: now, updatedAt: now })
          workspaces = [...workspaces, workspace]
          hosts[index] = hostProfileSchema.parse({ ...nextHost, defaultWorkspaceId: workspace.id })
        }
      }
      const finalHost = hosts[index]
      if (!finalHost) throw new Error('Updated host disappeared')
      updated = workspace ? { host: finalHost, auth: nextAuth, workspace } : { host: finalHost, auth: nextAuth }
      return { ...data, hosts, authProfiles, workspaces }
    })
    if (!updated) throw new Error('Host profile update failed')
    return updated
  }

  async delete(hostId: string): Promise<boolean> {
    let deleted = false
    await this.#store.update((data) => {
      const host = data.hosts.find((item) => item.id === hostId)
      if (!host) return data
      deleted = true
      const remainingHosts = data.hosts.filter((item) => item.id !== hostId).map((item) => {
        if (item.jumpHostId !== hostId) return item
        const copy = structuredClone(item)
        delete copy.jumpHostId
        return hostProfileSchema.parse({ ...copy, updatedAt: new Date().toISOString() })
      })
      return {
        ...data,
        hosts: remainingHosts,
        authProfiles: data.authProfiles.filter((item) => item.id !== host.authProfileId),
        workspaces: data.workspaces.filter((item) => item.hostId !== hostId),
        commands: data.commands.filter((item) => item.hostId !== hostId)
      }
    })
    return deleted
  }

  async listHostKeys(): Promise<HostKeyRecord[]> {
    return (await this.#store.load()).hostKeys
  }

  async addHostKey(record: HostKeyRecord): Promise<HostKeyRecord> {
    await this.#store.update((data) => ({
      ...data,
      hostKeys: [...data.hostKeys.filter((item) => !(item.host.toLowerCase() === record.host.toLowerCase() && item.port === record.port)), hostKeyRecordSchema.parse(record)]
    }))
    return record
  }

  async removeHostKey(recordId: string): Promise<boolean> {
    let removed = false
    await this.#store.update((data) => {
      const hostKeys = data.hostKeys.filter((item) => item.id !== recordId)
      removed = hostKeys.length !== data.hostKeys.length
      return { ...data, hostKeys }
    })
    return removed
  }

  async hasImport(sourceHash: string): Promise<boolean> {
    return (await this.#store.load()).imports.some((item) => item.sourceHash === sourceHash)
  }

  async recordImport(sourceHash: string, unsupported: Array<{ alias: string; lines: string[] }>, legacyRiskRules: string[] = []): Promise<void> {
    await this.#store.update((data) => data.imports.some((item) => item.sourceHash === sourceHash) ? data : { ...data, imports: [...data.imports, { sourceHash, unsupported, legacyRiskRules }] })
  }

  async listLegacyRiskRules(): Promise<string[]> {
    return [...new Set((await this.#store.load()).imports.flatMap((item) => item.legacyRiskRules))]
  }

  async diagnosticSummary(): Promise<unknown> {
    const data = await this.#store.load()
    return {
      schemaVersion: data.schemaVersion,
      counts: {
        hosts: data.hosts.length,
        workspaces: data.workspaces.length,
        knownHostKeys: data.hostKeys.length,
        tunnels: data.tunnels.length,
        commands: data.commands.length,
        privateKeyMetadata: data.privateKeys.length,
        legacyImports: data.imports.length
      },
      hosts: data.hosts.map((host, index) => {
        const auth = data.authProfiles.find((item) => item.id === host.authProfileId)
        return {
          hostIndex: index + 1,
          authMethod: auth?.method ?? 'missing',
          hasJumpHost: Boolean(host.jumpHostId),
          hasWorkspace: Boolean(host.defaultWorkspaceId),
          monitorEnabled: host.monitorEnabled,
          groupCount: host.groups.length,
          advanced: host.advanced
        }
      }),
      tunnels: data.tunnels.map((tunnel) => ({
        direction: tunnel.direction,
        autoStart: tunnel.autoStart,
        healthKind: tunnel.healthCheck?.type ?? 'none',
        legacyCleanupPresent: Boolean(tunnel.legacyCleanupHook),
        legacyCleanupAuthorized: tunnel.legacyCleanupHook?.authorized ?? false
      })),
      commands: data.commands.map((command) => ({
        scope: command.hostId ? 'host' : 'global',
        risk: command.risk,
        requiresPty: command.requiresPty,
        requiresSudo: command.requiresSudo
      }))
    }
  }

  async addTunnel(input: Omit<TunnelProfile, 'schemaVersion' | 'id' | 'createdAt' | 'updatedAt'>): Promise<TunnelProfile> {
    const now = new Date().toISOString()
    const tunnel = tunnelProfileSchema.parse({ schemaVersion: 1, id: randomUUID(), ...input, createdAt: now, updatedAt: now })
    await this.#store.update((data) => ({ ...data, tunnels: [...data.tunnels, tunnel] }))
    return tunnel
  }

  async listTunnels(): Promise<TunnelProfile[]> {
    return (await this.#store.load()).tunnels
  }

  async updateTunnel(tunnelId: string, patch: Partial<Omit<TunnelProfile, 'schemaVersion' | 'id' | 'hostId' | 'createdAt' | 'updatedAt' | 'healthCheck' | 'legacyCleanupHook'>> & { healthCheck?: TunnelProfile['healthCheck'] | null; legacyCleanupHook?: TunnelProfile['legacyCleanupHook'] | null }): Promise<TunnelProfile> {
    let updated: TunnelProfile | undefined
    await this.#store.update((data) => {
      const index = data.tunnels.findIndex((item) => item.id === tunnelId)
      const current = data.tunnels[index]
      if (!current) throw new Error(`Unknown tunnel profile: ${tunnelId}`)
      const merged: Record<string, unknown> = { ...current, ...patch, updatedAt: new Date().toISOString() }
      if (patch.healthCheck === null) delete merged['healthCheck']
      if (patch.legacyCleanupHook === null) delete merged['legacyCleanupHook']
      const next = tunnelProfileSchema.parse(merged)
      updated = next
      return { ...data, tunnels: data.tunnels.with(index, next) }
    })
    if (!updated) throw new Error('Tunnel update failed')
    return updated
  }

  async deleteTunnel(tunnelId: string): Promise<boolean> {
    let deleted = false
    await this.#store.update((data) => { const tunnels = data.tunnels.filter((item) => item.id !== tunnelId); deleted = tunnels.length !== data.tunnels.length; return { ...data, tunnels } })
    return deleted
  }

  async savePrivateKeyMetadata(metadata: PrivateKeyMetadata): Promise<PrivateKeyMetadata> {
    const parsed = privateKeyMetadataSchema.parse(metadata)
    await this.#store.update((data) => ({
      ...data,
      privateKeys: [...data.privateKeys.filter((item) => item.path.toLowerCase() !== parsed.path.toLowerCase()), parsed]
    }))
    return parsed
  }

  async listPrivateKeyMetadata(): Promise<PrivateKeyMetadata[]> {
    return (await this.#store.load()).privateKeys
  }

  async listCommands(hostId?: string): Promise<CommandPreset[]> {
    const commands = (await this.#store.load()).commands
    return commands.filter((item) => item.hostId === undefined || item.hostId === hostId).sort((left, right) => left.sortOrder - right.sortOrder || left.name.localeCompare(right.name))
  }

  async getCommand(commandId: string): Promise<CommandPreset> {
    const command = (await this.#store.load()).commands.find((item) => item.id === commandId)
    if (!command) throw new Error(`Unknown command preset: ${commandId}`)
    return command
  }

  async addCommand(input: CommandPresetInput): Promise<CommandPreset> {
    if (input.hostId) await this.get(input.hostId)
    const now = new Date().toISOString()
    const command = commandPresetSchema.parse({ schemaVersion: 1, id: randomUUID(), ...input, createdAt: now, updatedAt: now })
    await this.#store.update((data) => ({ ...data, commands: [...data.commands, command] }))
    return command
  }

  async updateCommand(commandId: string, patch: Partial<CommandPresetInput>): Promise<CommandPreset> {
    let updated: CommandPreset | undefined
    await this.#store.update((data) => {
      const index = data.commands.findIndex((item) => item.id === commandId)
      const current = data.commands[index]
      if (!current) throw new Error(`Unknown command preset: ${commandId}`)
      const next = commandPresetSchema.parse({ ...current, ...patch, updatedAt: new Date().toISOString() })
      if (next.hostId && !data.hosts.some((item) => item.id === next.hostId)) throw new Error(`Unknown host profile: ${next.hostId}`)
      updated = next
      return { ...data, commands: data.commands.with(index, next) }
    })
    if (!updated) throw new Error('Command preset update failed')
    return updated
  }

  async deleteCommand(commandId: string): Promise<boolean> {
    let deleted = false
    await this.#store.update((data) => {
      const commands = data.commands.filter((item) => item.id !== commandId)
      deleted = commands.length !== data.commands.length
      return { ...data, commands }
    })
    return deleted
  }
}

function assertValidJumpHost(data: ProfileDatabase, jumpHostId: string | null | undefined, currentHostId?: string): void {
  if (!jumpHostId) return
  if (jumpHostId === currentHostId) throw new Error('A host cannot use itself as its jump host')
  const jump = data.hosts.find((item) => item.id === jumpHostId)
  if (!jump) throw new Error(`Unknown jump host: ${jumpHostId}`)
  if (jump.jumpHostId) throw new Error('RemoteDeck v1 supports one ProxyJump level only')
}

function resolveHost(data: ProfileDatabase, host: HostProfile): ResolvedHostProfile {
  const auth = data.authProfiles.find((item) => item.id === host.authProfileId)
  if (!auth) throw new Error(`Missing auth profile for host: ${host.alias}`)
  const workspace = host.defaultWorkspaceId ? data.workspaces.find((item) => item.id === host.defaultWorkspaceId) : undefined
  return workspace ? { host, auth, workspace } : { host, auth }
}
