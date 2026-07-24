import { describe, expect, it } from 'vitest'
import type { HostKeyRecord } from '../../protocol/domain'
import { evaluateHostKey, inspectHostKey } from './host-key'

function blob(algorithm: string, payload: string): Buffer {
  const algorithmBytes = Buffer.from(algorithm)
  const payloadBytes = Buffer.from(payload)
  const value = Buffer.alloc(4 + algorithmBytes.length + payloadBytes.length)
  value.writeUInt32BE(algorithmBytes.length, 0)
  algorithmBytes.copy(value, 4)
  payloadBytes.copy(value, 4 + algorithmBytes.length)
  return value
}

describe('SSH host-key verification', () => {
  const first = blob('ssh-ed25519', 'first-key')
  const second = blob('ssh-ed25519', 'second-key')

  it('creates an explicit first-use candidate', () => {
    const result = evaluateHostKey('019f92f0-b87c-7c74-a668-54d5b18fb487', 'host.test', 22, first, [])
    expect(result.trusted).toBe(false)
    expect(result.candidate).toMatchObject({ mismatch: false, algorithm: 'ssh-ed25519', host: 'host.test' })
  })

  it('trusts an exact record and hard-blocks a changed fingerprint', () => {
    const observation = inspectHostKey(first)
    const record: HostKeyRecord = { schemaVersion: 1, id: '019f92f0-b87c-7c74-a668-54d5b18fb489', host: 'host.test', port: 22, ...observation, acceptedAt: '2026-07-24T08:00:00.000Z' }
    expect(evaluateHostKey('019f92f0-b87c-7c74-a668-54d5b18fb487', 'host.test', 22, first, [record]).trusted).toBe(true)
    const mismatch = evaluateHostKey('019f92f0-b87c-7c74-a668-54d5b18fb487', 'host.test', 22, second, [record])
    expect(mismatch).toMatchObject({ trusted: false, candidate: { mismatch: true, previousFingerprint: observation.sha256Fingerprint } })
  })
})
