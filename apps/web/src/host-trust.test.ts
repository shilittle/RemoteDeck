import { describe, expect, it } from 'vitest'
import { classifyConnectionFailure, connectionFailureHint, endpointKey, hasTrustedEndpoint } from './host-trust'

describe('local SSH trust matching', () => {
  it('matches aliases that resolve to the same saved endpoint', () => {
    const records = [{ hostname: 'LAB.EXAMPLE', port: 22, algorithm: 'ssh-ed25519' }]
    expect(endpointKey({ hostname: 'lab.example', port: 22 })).toBe('lab.example:22')
    expect(hasTrustedEndpoint({ hostname: 'lab.example', port: 22 }, records)).toBe(true)
    expect(hasTrustedEndpoint({ hostname: 'lab.example.', port: 22 }, records)).toBe(false)
    expect(hasTrustedEndpoint({ hostname: 'lab.example', port: 2222 }, records)).toBe(false)
  })

  it('classifies strict host-key failures without hiding the raw detail', () => {
    expect(classifyConnectionFailure('No ED25519 host key is known for [127.0.0.1]:61517 and you have requested strict checking.')).toBe('missing-host-key')
    expect(classifyConnectionFailure('WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!\nHost key verification failed.')).toBe('changed-host-key')
    expect(connectionFailureHint('missing-host-key')).toContain('扫描指纹')
    expect(classifyConnectionFailure('Permission denied (publickey).')).toBe('other')
  })
})
