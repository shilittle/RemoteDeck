import { describe, expect, it } from 'vitest'
import { queuePendingTerminalOutput } from './terminal-buffer'

describe('terminal pre-render output buffer', () => {
  it('preserves ordered SSH prompt fragments until xterm is ready', () => {
    const pending = new Map<string, string>()
    queuePendingTerminalOutput(pending, 'session', 'alice@example password:')
    queuePendingTerminalOutput(pending, 'session', ' ')
    expect(pending.get('session')).toBe('alice@example password: ')
  })

  it('bounds output produced before the terminal tab renders', () => {
    const pending = new Map<string, string>()
    queuePendingTerminalOutput(pending, 'session', 'x'.repeat(1024 * 1024 + 100))
    const buffered = pending.get('session') ?? ''
    expect(buffered.length).toBeLessThanOrEqual(1024 * 1024)
    expect(buffered).toContain('Earlier terminal output was truncated')
    expect(buffered.endsWith('x'.repeat(100))).toBe(true)
  })
})
