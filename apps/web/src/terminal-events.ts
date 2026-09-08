import type { TerminalEvent, TerminalSnapshot } from './types'

export function mergeTerminalListing(
  current: TerminalSnapshot[],
  listing: TerminalSnapshot[]
): TerminalSnapshot[] {
  const merged = new Map(current.map((snapshot) => [snapshot.sessionId, snapshot]))
  for (const incoming of listing) {
    const previous = merged.get(incoming.sessionId)
    if (!previous || isNewer(previous, incoming)) merged.set(incoming.sessionId, incoming)
  }
  return [...merged.values()]
}

function isNewer(previous: TerminalSnapshot, incoming: TerminalSnapshot): boolean {
  return incoming.generation > previous.generation
    || (incoming.generation === previous.generation && incoming.revision > previous.revision)
}

export function mergeTerminalEvent(
  current: TerminalSnapshot[],
  event: TerminalEvent
): TerminalSnapshot[] {
  const index = current.findIndex((snapshot) => snapshot.sessionId === event.sessionId)
  if (event.snapshot) {
    if (index < 0) return [...current, event.snapshot]
    if (!isNewer(current[index], event.snapshot)) return current
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
