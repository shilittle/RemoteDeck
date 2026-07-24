const secretKeyPattern = /(?:password|passphrase|private[_-]?key|token|authorization|api[_-]?key|secret)/i
const privateKeyPattern = /-----BEGIN [^-]*PRIVATE KEY-----[\s\S]*?-----END [^-]*PRIVATE KEY-----/g
const bearerPattern = /\bBearer\s+[A-Za-z0-9._~+/-]+=*/gi
const assignmentPattern = /\b(password|passphrase|token|api[_-]?key|secret)\s*[:=]\s*([^\s,;]+)/gi
const jsonSecretPattern = /(["']?(?:password|passphrase|token|authorization|api[_-]?key|secret)["']?\s*:\s*["'])([^"'\r\n]+)(["'])/gi
const urlCredentialPattern = /\b([a-z][a-z0-9+.-]*:\/\/[^\s:/@]+:)([^\s/@]+)(@)/gi
const openAiTokenPattern = /\b(?:sk|sess)-(?:proj-|svcacct-)?[A-Za-z0-9_-]{8,}\b/g

export function redactSecrets(value: unknown, seen = new WeakSet<object>()): unknown {
  if (typeof value === 'string') return redactText(value)
  if (Array.isArray(value)) return value.map((item) => redactSecrets(item, seen))
  if (value === null || typeof value !== 'object') return value
  if (seen.has(value)) return '[Circular]'
  seen.add(value)
  const output: Record<string, unknown> = {}
  for (const [key, item] of Object.entries(value)) {
    output[key] = secretKeyPattern.test(key) ? '[Redacted]' : redactSecrets(item, seen)
  }
  return output
}

export function redactText(text: string): string {
  return text
    .replace(privateKeyPattern, '[Redacted private key]')
    .replace(bearerPattern, 'Bearer [Redacted]')
    .replace(jsonSecretPattern, '$1[Redacted]$3')
    .replace(urlCredentialPattern, '$1[Redacted]$3')
    .replace(openAiTokenPattern, '[Redacted token]')
    .replace(assignmentPattern, (_match, key: string) => `${key}=[Redacted]`)
}
