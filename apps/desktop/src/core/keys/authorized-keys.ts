export interface AuthorizedKeyMergeResult {
  content: string
  alreadyPresent: boolean
  fingerprintMaterial: string
}
export function mergeAuthorizedKey(existing: string, publicKey: string): AuthorizedKeyMergeResult {
  const candidate = parseAuthorizedKey(publicKey)
  if (!candidate) throw new Error('Generated public key is not a valid OpenSSH authorized-key entry')
  const lines = existing.replace(/\r\n?/g, '\n').split('\n')
  const alreadyPresent = lines.some((line) => parseAuthorizedKey(line)?.material === candidate.material)
  if (alreadyPresent) return { content: normalize(existing), alreadyPresent: true, fingerprintMaterial: candidate.material }
  const normalized = normalize(existing)
  return { content: `${normalized}${normalized ? '' : ''}${candidate.line}\n`, alreadyPresent: false, fingerprintMaterial: candidate.material }
}

function parseAuthorizedKey(line: string): { line: string; material: string } | undefined {
  const trimmed = line.trim()
  if (!trimmed || trimmed.startsWith('#')) return undefined
  const tokens = trimmed.split(/\s+/)
  const algorithmIndex = tokens.findIndex((token) => /^(?:ssh-|ecdsa-|sk-)/.test(token))
  const algorithm = tokens[algorithmIndex]
  const payload = tokens[algorithmIndex + 1]
  if (algorithmIndex < 0 || !algorithm || !payload || !/^[A-Za-z0-9+/]+={0,2}$/.test(payload)) return undefined
  return { line: `${algorithm} ${payload}${tokens.length > algorithmIndex + 2 ? ` ${tokens.slice(algorithmIndex + 2).join(' ')}` : ''}`, material: `${algorithm} ${payload}` }
}

function normalize(value: string): string {
  const lines = value.replace(/\r\n?/g, '\n').split('\n').map((line) => line.trimEnd())
  while (lines.at(-1) === '') lines.pop()
  return lines.length > 0 ? `${lines.join('\n')}\n` : ''
}
