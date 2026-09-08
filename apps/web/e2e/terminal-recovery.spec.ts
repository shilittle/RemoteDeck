import { expect, test, type Page, type WebSocketRoute } from '@playwright/test'
import { openLaunchedPage } from './helpers'

const identity = process.env.REMOTEDECK_INTEGRATION_IDENTITY
const port = Number(process.env.REMOTEDECK_DIRECT_PORT)

test('real PTY survives page closure and grants input to exactly one browser attachment', async ({ page, context }, testInfo) => {
  test.skip(!identity || !port, 'requires the disposable OpenSSH fixture')
  test.setTimeout(120_000)
  const connections: Array<{ browser: WebSocketRoute; server: WebSocketRoute }> = []
  await page.routeWebSocket(/\/api\/v1\/terminals\/.*\/stream\?/, (browser) => {
    // Forward all bytes to the real service; only the connection failure below is injected.
    connections.push({ browser, server: browser.connectToServer() })
  })
  await openLaunchedPage(page)
  await expect(page.getByRole('button', { name: '主机', exact: true })).toBeVisible()
  await operation(page, 'update_settings', { patch: { onboardingCompleted: true } })
  const alias = `recovery-${String(Date.now())}`
  const host = await operation<{ id: string }>(page, 'save_host', { draft: {
    alias, hostname: '127.0.0.1', port, username: 'remotedeck', authMethod: 'private_key',
    identityFile: identity, groups: [], defaultWorkspace: '~', monitorEnabled: false
  } })
  const candidates = await operation<unknown[]>(page, 'scan_host_keys', { hostId: host.id })
  expect(candidates.length).toBeGreaterThan(0)
  await operation(page, 'accept_host_key', { hostId: host.id, candidate: candidates[0] })
  await selectWorkspace(page, alias)
  await page.getByLabel('新建终端', { exact: true }).click()
  await expect(page.getByRole('status').filter({ hasText: '已获得输入控制权' })).toBeVisible()
  const original = await operation<Array<{ sessionId: string; hostId: string }>>(page, 'list_terminals', {})
  const session = original.find((item) => item.hostId === host.id)
  expect(session).toBeDefined()
  const firstMarker = `first-${String(Date.now())}`
  await echo(page, firstMarker)

  const interrupted = connections.at(-1)
  expect(interrupted).toBeDefined()
  // 1006 is a browser-side transport error and cannot be sent in a wire close frame.
  interrupted?.browser.onClose(() => {})
  await interrupted?.browser.close({ code: 1006, reason: 'Test transport interruption' })
  await interrupted?.server.close()
  await expect.poll(() => connections.length, { timeout: 15_000 }).toBe(2)
  await expect(page.getByRole('status').filter({ hasText: '已获得输入控制权' })).toBeVisible()
  expect(new URL(connections[1].browser.url()).searchParams.get('resumeLease')).toMatch(/^\d+$/u)
  await echo(page, `network-restored-${String(Date.now())}`)

  const other = await context.newPage()
  await openLaunchedPage(other)
  await selectWorkspace(other, alias)
  await expect(other.getByRole('status').filter({ hasText: '已获得输入控制权' })).toBeVisible()
  await expect(page.getByRole('status').filter({ hasText: '输入已由其他页面接管' })).toBeVisible()
  await echo(other, `second-${String(Date.now())}`)
  // Opening another launch URL must preserve the first tab's cookie and CSRF.
  expect((await operation<unknown[]>(page, 'list_terminals', {})).length).toBeGreaterThan(0)
  await page.getByRole('button', { name: '接管输入', exact: true }).click()
  await expect(page.getByRole('status').filter({ hasText: '已获得输入控制权' })).toBeVisible()
  await expect(other.getByRole('status').filter({ hasText: '输入已由其他页面接管' })).toBeVisible()
  const retained = await page.locator('.terminal-surface.active .xterm-rows').innerText()
  expect(retained.split(`__${firstMarker}__`).length - 1).toBe(1)
  await other.close()

  // Closing the controller does not close its PTY; a fresh page attaches to that ID.
  const baseUrl = new URL(page.url()).origin
  await page.close()
  const reopened = await context.newPage()
  await reopened.goto(baseUrl)
  await selectWorkspace(reopened, alias)
  await expect(reopened.getByRole('status').filter({ hasText: '已获得输入控制权' })).toBeVisible()
  const restored = await operation<Array<{ sessionId: string; hostId: string }>>(reopened, 'list_terminals', {})
  expect(restored.filter((item) => item.hostId === host.id).map((item) => item.sessionId)).toEqual([session?.sessionId])
  await echo(reopened, `reopened-${String(Date.now())}`)
  await reopened.screenshot({ path: testInfo.outputPath('terminal-recovered.png'), fullPage: true })
  await operation(reopened, 'close_terminal', { sessionId: session?.sessionId })
  await reopened.close()
})

async function selectWorkspace(page: Page, alias: string): Promise<void> {
  await expect(page.locator('.host-item').filter({ hasText: alias })).toBeVisible({ timeout: 20_000 })
  await page.locator('.host-item').filter({ hasText: alias }).click()
  await page.getByRole('button', { name: '工作区', exact: true }).click()
}

async function echo(page: Page, marker: string): Promise<void> {
  await page.locator('.terminal-surface.active .xterm-helper-textarea').focus()
  await page.keyboard.type(`printf '__%s__\\n' '${marker}'`)
  await page.keyboard.press('Enter')
  await expect(page.locator('.terminal-surface.active .xterm-rows')).toContainText(`__${marker}__`, { timeout: 20_000 })
}

async function operation<T = unknown>(page: Page, command: string, args: Record<string, unknown>): Promise<T> {
  return page.evaluate(async ({ command, args }) => {
    const auth = await fetch('/api/v1/auth/session').then((response) => response.json()) as { csrfToken: string }
    const response = await fetch(`/api/v1/${command}`, { method: 'POST', headers: {
      'Content-Type': 'application/json', 'X-RemoteDeck-CSRF': auth.csrfToken, 'Idempotency-Key': crypto.randomUUID()
    }, body: JSON.stringify(args) })
    const result = await response.json() as unknown
    if (!response.ok) throw new Error(`Operation ${command}: ${JSON.stringify(result)}`)
    if (response.status !== 202) return result
    const { operationId } = result as { operationId: string }
    for (;;) {
      const poll = await fetch(`/api/v1/operations/${operationId}`).then((response) => response.json()) as { state: string; result?: unknown; error?: { message: string } }
      if (poll.state === 'completed') return poll.result
      if (poll.state === 'failed') throw new Error(poll.error?.message ?? 'operation failed')
      await new Promise((resolve) => setTimeout(resolve, 200))
    }
  }, { command, args }) as Promise<T>
}
