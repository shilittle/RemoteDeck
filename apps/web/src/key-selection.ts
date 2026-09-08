import type { PrivateKeyRecord } from './types'

export function retainOrSelectKeyPath(current: string, keys: PrivateKeyRecord[]): string {
  return current || keys.at(0)?.path || ''
}
