import { afterEach, describe, expect, it, vi } from 'vitest'

afterEach(() => {
  vi.resetModules()
  vi.unstubAllGlobals()
})

describe('CSRF rotation recovery', () => {
  it('refreshes only an invalid_csrf response and preserves the original idempotency key', async () => {
    vi.stubGlobal('window', { location: { hash: '', pathname: '/', search: '' }, history: { replaceState: vi.fn() } })
    vi.stubGlobal('crypto', { randomUUID: () => 'fixed-request-key' })
    let sessionReads = 0
    let saveAttempts = 0
    const calls: Array<{ url: string; init?: RequestInit }> = []
    vi.stubGlobal('fetch', vi.fn((input: string, init?: RequestInit) => {
      const url = input
      calls.push({ url, init })
      if (url === '/api/v1/auth/session') {
        sessionReads += 1
        return json({ csrfToken: sessionReads === 1 ? 'first-csrf' : 'rotated-csrf' })
      }
      if (url === '/api/v1/save_host') {
        saveAttempts += 1
        if (saveAttempts === 1) return json({ code: 'invalid_csrf', message: 'CSRF token changed.' }, 403)
        return json({ id: 'host-after-retry' })
      }
      throw new Error(`unexpected request ${url}`)
    }))

    const { api } = await import('./api')
    await expect(api.saveHost({ alias: 'csrf-host', hostname: '127.0.0.1', port: 22, username: 'tester', groups: [] })).resolves.toMatchObject({ id: 'host-after-retry' })

    const saves = calls.filter((call) => call.url === '/api/v1/save_host')
    expect(sessionReads).toBe(2)
    expect(saves).toHaveLength(2)
    expect(header(saves[0], 'Idempotency-Key')).toBe('fixed-request-key')
    expect(header(saves[1], 'Idempotency-Key')).toBe('fixed-request-key')
    expect(header(saves[0], 'X-RemoteDeck-CSRF')).toBe('first-csrf')
    expect(header(saves[1], 'X-RemoteDeck-CSRF')).toBe('rotated-csrf')
  })
})

function json(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } })
}

function header(call: { init?: RequestInit }, name: string): string | undefined {
  const headers = call.init?.headers
  return headers && typeof headers === 'object' && !Array.isArray(headers) ? (headers as Record<string, string>)[name] : undefined
}
