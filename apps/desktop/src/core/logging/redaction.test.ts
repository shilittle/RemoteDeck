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

  it('redacts JSON fields, URL credentials, OpenAI tokens, and passphrases', () => {
    const text = '{"token":"sk-proj-abcdefgh12345678","authorization":"Basic dXNlcjpwYXNz"}\nssh://developer:hunter2@example.test passphrase: swordfish sess-abcdefgh12345678'
    const redacted = redactText(text)
    for (const secret of ['sk-proj-abcdefgh12345678', 'Basic dXNlcjpwYXNz', 'hunter2', 'swordfish', 'sess-abcdefgh12345678']) expect(redacted).not.toContain(secret)
  })
})
