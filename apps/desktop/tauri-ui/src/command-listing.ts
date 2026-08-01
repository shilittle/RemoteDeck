import type { CommandDefinition } from './types'

export interface CommandListingRequestIdentity {
  hostId: string
  generation: number
}

export function canCommitCommandListing(
  request: CommandListingRequestIdentity,
  activeHostId: string | null,
  activeGeneration: number,
  commands: CommandDefinition[]
): boolean {
  return request.generation === activeGeneration &&
    request.hostId === activeHostId &&
    commands.every((command) => command.hostId === null || command.hostId === request.hostId)
}

export function isCommandListingCurrent(
  loadedHostId: string | null,
  activeHostId: string | null
): boolean {
  return loadedHostId !== null && loadedHostId === activeHostId
}
