import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { retainOrSelectKeyPath } from './key-selection'
import type { PrivateKeyRecord } from './types'

const listed: PrivateKeyRecord[] = [{
  path: 'C:\\Users\\tester\\.ssh\\id_ed25519',
  algorithm: 'ssh-ed25519',
  fingerprint: 'SHA256:test',
  encrypted: false,
  comment: null
}]

describe('private-key selection', () => {
  it('does not bind the native key inventory effect to key-path keystrokes', () => {
    const source = readFileSync(new URL('./panels/HostPanel.tsx', import.meta.url), 'utf8')
    expect(source).not.toContain('}, [keyPath])')
    expect(source).toContain('setKeyPath((current) => retainOrSelectKeyPath(current, nextPrivate))')
  })

  it('does not overwrite a path typed while the initial listing is pending', () => {
    expect(retainOrSelectKeyPath('D:\\keys\\new-key', listed)).toBe('D:\\keys\\new-key')
  })

  it('selects the first listed key only when the field is still empty', () => {
    expect(retainOrSelectKeyPath('', listed)).toBe(listed[0].path)
    expect(retainOrSelectKeyPath('', [])).toBe('')
  })
})
