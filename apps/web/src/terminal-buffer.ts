const MAX_PENDING_TERMINAL_CHARS = 1024 * 1024
const TRUNCATED_OUTPUT_NOTICE = '\r\n[RemoteDeck] Earlier terminal output was truncated before the tab became ready.\r\n'

export function queuePendingTerminalOutput(pending: Map<string, string>, sessionId: string, data: string): void {
  const combined = `${pending.get(sessionId) ?? ''}${data}`
  if (combined.length <= MAX_PENDING_TERMINAL_CHARS) {
    pending.set(sessionId, combined)
    return
  }
  const keep = Math.max(0, MAX_PENDING_TERMINAL_CHARS - TRUNCATED_OUTPUT_NOTICE.length)
  pending.set(sessionId, `${TRUNCATED_OUTPUT_NOTICE}${combined.slice(-keep)}`)
}
