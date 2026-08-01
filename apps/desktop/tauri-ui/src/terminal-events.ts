import type { TerminalEvent, TerminalSnapshot } from './types'

export function mergeTerminalListing(
  current: TerminalSnapshot[],
  listing: TerminalSnapshot[]
): TerminalSnapshot[] {
  const currentIds = new Set(current.map((snapshot) => snapshot.sessionId))
  return [...current, ...listing.filter((snapshot) => !currentIds.has(snapshot.sessionId))]
}

export function mergeTerminalEvent(
  current: TerminalSnapshot[],
  event: TerminalEvent
): TerminalSnapshot[] {
  const index = current.findIndex((snapshot) => snapshot.sessionId === event.sessionId)
  if (event.snapshot) {
    if (index < 0) return [...current, event.snapshot]
    return current.map((snapshot, itemIndex) => itemIndex === index
      ? { ...snapshot, ...event.snapshot }
      : snapshot)
  }
  if (index < 0) return current
  return current.map((snapshot, itemIndex) => {
    if (itemIndex !== index) return snapshot
    if (event.kind === 'exit') return { ...snapshot, state: 'closed', exitCode: event.exitCode }
    if (event.kind === 'error') return { ...snapshot, state: 'failed', error: event.message }
    if (event.kind === 'started') return { ...snapshot, state: 'running' }
    return snapshot
  })
}
