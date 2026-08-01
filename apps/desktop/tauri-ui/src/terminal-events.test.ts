import { describe, expect, it } from 'vitest'
import { mergeTerminalEvent, mergeTerminalListing } from './terminal-events'
import type { TerminalEvent, TerminalSnapshot } from './types'

function snapshot(sessionId: string, state: TerminalSnapshot['state']): TerminalSnapshot {
  return {
    sessionId,
    hostId: 'host',
    alias: 'lab',
    cwd: '~',
    title: sessionId,
    state,
    exitCode: null,
    error: null
  }
}

function event(sessionId: string, overrides: Partial<TerminalEvent>): TerminalEvent {
  return {
    sessionId,
    kind: 'output',
    data: null,
    exitCode: null,
    message: null,
    ...overrides
  }
}

describe('terminal initialization merges', () => {
  it('does not let a delayed listing overwrite a newer local snapshot', () => {
    const current = [snapshot('existing', 'running')]
    const listing = [snapshot('existing', 'offline'), snapshot('listed', 'closed')]

    expect(mergeTerminalListing(current, listing)).toEqual([
      snapshot('existing', 'running'),
      snapshot('listed', 'closed')
    ])
  })

  it('upserts a buffered started snapshot that was absent from the listing', () => {
    const started = snapshot('new-session', 'running')
    const merged = mergeTerminalEvent([], event('new-session', { kind: 'started', snapshot: started }))

    expect(merged).toEqual([started])
  })

  it('replays lifecycle events over the initial listing', () => {
    const listed = snapshot('session', 'running')
    const merged = mergeTerminalEvent(
      mergeTerminalListing([], [listed]),
      event('session', { kind: 'exit', exitCode: 17 })
    )

    expect(merged[0]).toMatchObject({ state: 'closed', exitCode: 17 })
  })
})
