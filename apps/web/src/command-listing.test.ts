import { describe, expect, it } from 'vitest'
import { canCommitCommandListing, isCommandListingCurrent } from './command-listing'
import type { CommandDefinition } from './types'

function command(hostId: string | null): CommandDefinition {
  return {
    id: 'command', hostId, name: 'status', description: '', group: 'test', command: 'uptime',
    workingDirectory: null, risk: 'L0', requiresPty: false, requiresSudo: false,
    confirmationText: null, sortOrder: 0, builtin: false
  }
}

describe('command listing request identity', () => {
  it('accepts global and matching-host commands for the active generation', () => {
    expect(canCommitCommandListing(
      { hostId: 'host-a', generation: 4 },
      'host-a',
      4,
      [command(null), command('host-a')]
    )).toBe(true)
  })

  it('rejects stale generations, host switches, and mismatched command ownership', () => {
    const request = { hostId: 'host-a', generation: 4 }
    expect(canCommitCommandListing(request, 'host-a', 5, [command('host-a')])).toBe(false)
    expect(canCommitCommandListing(request, 'host-b', 4, [command('host-a')])).toBe(false)
    expect(canCommitCommandListing(request, 'host-a', 4, [command('host-b')])).toBe(false)
  })

  it('never exposes a listing under a different selected host', () => {
    expect(isCommandListingCurrent('host-a', 'host-a')).toBe(true)
    expect(isCommandListingCurrent('host-a', 'host-b')).toBe(false)
    expect(isCommandListingCurrent(null, 'host-a')).toBe(false)
  })
})
