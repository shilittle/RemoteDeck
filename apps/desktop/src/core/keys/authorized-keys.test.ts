import { describe, expect, it } from 'vitest'
import { mergeAuthorizedKey } from './authorized-keys'

const key = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIC6AYqgP+fixtureMaterialForUnitTest RemoteDeck'

describe('authorized_keys merge', () => {
  it('deduplicates by algorithm and key material regardless of comments', () => {
    const existing = `# user keys\n${key.replace('RemoteDeck', 'old-comment')}\n`
    const result = mergeAuthorizedKey(existing, key)
    expect(result.alreadyPresent).toBe(true)
    expect(result.content.match(/ssh-ed25519/g)).toHaveLength(1)
  })

  it('normalizes and appends a missing key exactly once', () => {
    const result = mergeAuthorizedKey('ssh-rsa QUFBQQ== old\r\n', key)
    expect(result.alreadyPresent).toBe(false)
    expect(result.content).toContain(key)
    expect(result.content.endsWith('\n')).toBe(true)
  })
})
