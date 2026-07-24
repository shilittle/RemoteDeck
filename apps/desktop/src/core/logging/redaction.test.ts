import { describe, expect, it } from 'vitest'
import { redactSecrets, redactText } from './redaction'

describe('secret redaction', () => {
  it('redacts nested credential fields without mutating safe metadata', () => {
    expect(redactSecrets({ host: 'lab', password: 'hunter2', nested: { apiToken: 'sk-secret' } })).toEqual({
      host: 'lab',
      password: '[Redacted]',
      nested: { apiToken: '[Redacted]' }
    })
  })

  it('redacts private keys, bearer headers, and inline assignments', () => {
    const text = 'Authorization: Bearer sk-abc password=hunter2\n-----BEGIN OPENSSH PRIVATE KEY-----\nsecret\n-----END OPENSSH PRIVATE KEY-----'
    const redacted = redactText(text)
    expect(redacted).not.toContain('sk-abc')
    expect(redacted).not.toContain('hunter2')
    expect(redacted).not.toContain('OPENSSH PRIVATE KEY')
  })
})

