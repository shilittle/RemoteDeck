import { createHash, randomUUID } from 'node:crypto'
import type { HostKeyRecord } from '../../protocol/domain'
import type { HostKeyCandidate } from '../../protocol/ssh'

export interface HostKeyObservation {
  algorithm: string
  sha256Fingerprint: string
  publicKeyBase64: string
}
export function inspectHostKey(key: Buffer): HostKeyObservation {
  if (key.length < 5) throw new Error('Invalid SSH host-key blob')
  const length = key.readUInt32BE(0)
  if (length <= 0 || length + 4 > key.length) throw new Error('Invalid SSH host-key algorithm field')
  const algorithm = key.subarray(4, 4 + length).toString('ascii')
  const digest = createHash('sha256').update(key).digest('base64').replace(/=+$/, '')
  return { algorithm, sha256Fingerprint: `SHA256:${digest}`, publicKeyBase64: key.toString('base64') }
}

export function evaluateHostKey(
  hostId: string,
  host: string,
  port: number,
  key: Buffer,
  records: HostKeyRecord[]
): { trusted: boolean; candidate?: HostKeyCandidate } {
  const observation = inspectHostKey(key)
  const existing = records.find((record) => record.host.toLowerCase() === host.toLowerCase() && record.port === port)
  if (existing?.sha256Fingerprint === observation.sha256Fingerprint && existing.algorithm === observation.algorithm) return { trusted: true }
  const candidate: HostKeyCandidate = {
    id: randomUUID(),
    hostId,
    host,
    port,
    ...observation,
    ...(existing ? { previousFingerprint: existing.sha256Fingerprint } : {}),
    mismatch: existing !== undefined,
    observedAt: new Date().toISOString()
  }
  return { trusted: false, candidate }
}
