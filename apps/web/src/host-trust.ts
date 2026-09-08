import type { HostKeyRecord } from './types'

export interface TrustedEndpoint {
  hostname: string
  port: number
}

export type ConnectionFailureKind = 'missing-host-key' | 'changed-host-key' | 'other'

function normalizedHostname(hostname: string): string {
  return hostname.trim().toLowerCase()
}

export function endpointKey(endpoint: TrustedEndpoint): string {
  return `${normalizedHostname(endpoint.hostname)}:${String(endpoint.port)}`
}

export function hasTrustedEndpoint(
  endpoint: TrustedEndpoint,
  records: Pick<HostKeyRecord, 'hostname' | 'port'>[],
): boolean {
  const key = endpointKey(endpoint)
  return records.some((record) => endpointKey(record) === key)
}

export function classifyConnectionFailure(error: string | null): ConnectionFailureKind | null {
  if (!error) return null
  if (/remote host identification has changed|offending .* key/iu.test(error)) return 'changed-host-key'
  if (/host key verification failed|no .* host key is known|strict checking/iu.test(error)) return 'missing-host-key'
  return 'other'
}

export function connectionFailureHint(kind: ConnectionFailureKind | null): string | null {
  if (kind === 'missing-host-key') {
    return '当前端点在 RemoteDeck 的本地 SSH 信任记录中没有匹配项。已有本机 known_hosts 记录会直接复用；没有记录时请先扫描指纹，独立核对后再接受。私钥和密码只负责登录，不能替代主机指纹核验。'
  }
  if (kind === 'changed-host-key') {
    return '当前端点的主机指纹与已保存记录不一致，连接已被阻断。请确认服务器是否重装或地址是否被复用；只有确认变更来源后，才能删除旧记录并重新扫描。'
  }
  return null
}
