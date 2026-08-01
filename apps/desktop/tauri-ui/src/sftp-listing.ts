import type { SftpListing } from './types'

export interface SftpListingRequestIdentity {
  hostId: string
  generation: number
}

export function isSftpListingForHost(
  listing: SftpListing | null,
  hostId: string | null
): listing is SftpListing {
  return listing !== null && hostId !== null && listing.hostId === hostId
}

export function canCommitSftpListing(
  request: SftpListingRequestIdentity,
  activeHostId: string | null,
  activeGeneration: number,
  response: SftpListing
): boolean {
  return request.generation === activeGeneration &&
    request.hostId === activeHostId &&
    response.hostId === request.hostId
}
