import { describe, expect, it } from 'vitest'
import { protocolVersion } from './version'

describe('protocol version', () => {
  it('starts the versioned desktop protocol at one', () => {
    expect(protocolVersion).toBe(1)
  })
})
