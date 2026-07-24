import { describe, expect, it } from 'vitest'
import { connectionStateSchema, hostProfileSchema, telemetrySnapshotSchema, tunnelStateSchema } from './domain'

const timestamp = '2026-07-24T08:00:00.000Z'

describe('versioned domain schemas', () => {
  it('normalizes a valid host profile and default SSH options', () => {
    const host = hostProfileSchema.parse({
      schemaVersion: 1,
      id: '019f92f0-b87c-7c74-a668-54d5b18fb487',
      alias: 'gpu-lab',
      hostname: 'gpu.example.test',
      port: 22,
      username: 'developer',
      authProfileId: '019f92f0-b87c-7c74-a668-54d5b18fb488',
      groups: ['research'],
      advanced: {},
      createdAt: timestamp,
      updatedAt: timestamp
    })
    expect(host.advanced.serverAliveIntervalSeconds).toBe(30)
    expect(host.monitorEnabled).toBe(true)
  })

  it('rejects unsafe aliases and out-of-range ports', () => {
    const result = hostProfileSchema.safeParse({
      schemaVersion: 1,
      id: '019f92f0-b87c-7c74-a668-54d5b18fb487',
      alias: 'gpu lab; reboot',
      hostname: 'host',
      port: 70_000,
      username: 'developer',
      authProfileId: '019f92f0-b87c-7c74-a668-54d5b18fb488',
      advanced: {},
      createdAt: timestamp,
      updatedAt: timestamp
    })
    expect(result.success).toBe(false)
  })

  it('enumerates the required connection and tunnel states', () => {
    expect(connectionStateSchema.options).toContain('awaiting_host_key')
    expect(connectionStateSchema.options).toContain('reconnecting')
    expect(tunnelStateSchema.options).toEqual(['stopped', 'starting', 'healthy', 'degraded', 'recovering', 'failed'])
  })

  it('rejects telemetry without per-core samples', () => {
    expect(telemetrySnapshotSchema.safeParse({ schemaVersion: 1 }).success).toBe(false)
  })
})

