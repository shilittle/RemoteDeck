import { describe, expect, it } from 'vitest'
import { canCommitSftpListing, isSftpListingForHost } from './sftp-listing'
import type { SftpListing } from './types'

function listing(hostId: string): SftpListing {
  return { hostId, path: '/workspace', parentPath: '/', entries: [] }
}

describe('SFTP listing request identity', () => {
  it('accepts only the response for the current host and generation', () => {
    expect(canCommitSftpListing(
      { hostId: 'host-a', generation: 3 },
      'host-a',
      3,
      listing('host-a')
    )).toBe(true)
  })

  it('rejects a response after the selected host changes', () => {
    expect(canCommitSftpListing(
      { hostId: 'host-a', generation: 3 },
      'host-b',
      3,
      listing('host-a')
    )).toBe(false)
  })

  it('rejects stale generations and mismatched backend host identities', () => {
    expect(canCommitSftpListing(
      { hostId: 'host-a', generation: 2 },
      'host-a',
      3,
      listing('host-a')
    )).toBe(false)
    expect(canCommitSftpListing(
      { hostId: 'host-a', generation: 3 },
      'host-a',
      3,
      listing('host-b')
    )).toBe(false)
  })

  it('never exposes a listing under a different selected host', () => {
    expect(isSftpListingForHost(listing('host-a'), 'host-a')).toBe(true)
    expect(isSftpListingForHost(listing('host-a'), 'host-b')).toBe(false)
    expect(isSftpListingForHost(null, 'host-a')).toBe(false)
  })
})
