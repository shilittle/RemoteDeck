import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { createHmac } from 'node:crypto'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { expect, test, type Page } from '@playwright/test'
import { openLaunchedPage } from './helpers'

interface FixtureConfig { identity: string; directPort: number; tunnelPort: number }
interface HostResponse { id: string }
interface TerminalResponse { sessionId: string }
interface TransferResponse { id: string }
interface TransferState { id: string; state: string; error: string | null }

const fixture = loadFixtureConfig()

test.describe('real OpenSSH browser workflow', () => {
  test.skip(!fixture, 'requires REMOTEDECK_INTEGRATION_IDENTITY, REMOTEDECK_DIRECT_PORT, and REMOTEDECK_REMOTE_FORWARD_PORT from the disposable OpenSSH fixture')
  test('trusts a fixture host, attaches a terminal after refresh, runs a command, and transfers a real file', async ({ page }) => {
    test.setTimeout(120_000)
    if (!fixture) throw new Error('OpenSSH fixture configuration was not supplied.')

    const csrf = await openWithCsrf(page)
    const host = await savePrivateKeyHost(page, fixture, csrf)
    await verifyUntrustedConnectionIsRejectedInUi(page)
    await trustHostInUi(page)
    const sessionId = await verifyTerminalReconnect(page)
    await verifyLowRiskCommandInUi(page)
    await verifyTransferThroughAuthenticatedApi(page, csrf, host.id)
    await verifyLocalTunnelThroughAuthenticatedApi(page, csrf, host.id, fixture.tunnelPort)
    await browserApi<undefined>(page, csrf, 'close_terminal', { sessionId })
  })

  test('imports local SSH config with hashed existing trust and connects without repeated confirmation', async ({ page }) => {
    test.setTimeout(120_000)
    if (!fixture) throw new Error('OpenSSH fixture configuration was not supplied.')
    const csrf = await openWithCsrf(page)
    await browserApi(page, csrf, 'update_settings', { patch: { onboardingCompleted: true } })
    const probe = await browserApi<HostResponse>(page, csrf, 'save_host', { draft: {
      alias: 'local-trust-fixture-probe', hostname: '127.0.0.1', port: fixture.directPort,
      username: 'remotedeck', authMethod: 'private_key', identityFile: fixture.identity,
      monitorEnabled: false, groups: []
    } })
    // Fixture setup models an already-confirmed local SSH record. It writes only
    // the disposable service's fake home, never the developer's actual known_hosts.
    const candidates = await browserApi<Array<{ hostToken: string; algorithm: string; publicKeyBase64: string }>>(page, csrf, 'scan_host_keys', { hostId: probe.id })
    const key = candidates.find((candidate) => candidate.algorithm === 'ssh-ed25519')
    expect(key).toBeDefined()
    if (!key) throw new Error('Fixture did not return an Ed25519 key.')
    const records = await browserApi<Array<{ id: string; hostToken: string }>>(page, csrf, 'list_host_keys', {})
    for (const record of records.filter((record) => record.hostToken === key.hostToken)) {
      await browserApi(page, csrf, 'remove_host_key', { recordId: record.id })
    }
    const runtime = JSON.parse(await readFile(resolve(import.meta.dirname, '..', '.e2e-runtime.json'), 'utf8')) as { dataDir: string }
    const salt = Buffer.alloc(20, 7)
    const hash = createHmac('sha1', salt).update(key.hostToken).digest('base64')
    const source = join(runtime.dataDir, 'home', '.ssh', 'known_hosts')
    const sourceContents = `|1|${salt.toString('base64')}|${hash} ${key.algorithm} ${key.publicKeyBase64}\n`
    await writeFile(source, sourceContents)
    const configPath = join(runtime.dataDir, 'local-config')
    await writeFile(configPath, `Host local-config-direct\n HostName 127.0.0.1\n Port ${String(fixture.directPort)}\n User remotedeck\n IdentityFile "${fixture.identity.replaceAll('\\', '/')}"\n`)
    let confirmationRequests = 0
    page.on('request', (request) => {
      if (/\/api\/v1\/(?:scan_host_keys|accept_host_key)$/u.test(request.url())) confirmationRequests += 1
    })
    await page.getByRole('button', { name: '主机', exact: true }).click()
    await page.locator('.import-card input').fill(configPath)
    await page.locator('.import-card').getByRole('button', { name: '导入', exact: true }).click()
    await page.locator('.host-list .host-item').filter({ hasText: 'local-config-direct' }).click()
    await expect(page.locator('.trust-card')).toContainText('已有本地信任记录')
    await page.getByRole('button', { name: '测试连接', exact: true }).click()
    await expect(page.locator('.result-card.success')).toContainText('连接成功', { timeout: 30_000 })
    const sessionId = await verifyTerminalReconnect(page)
    await browserApi(page, csrf, 'close_terminal', { sessionId })
    expect(confirmationRequests).toBe(0)
    expect(await readFile(source, 'utf8')).toBe(sourceContents)
  })
})

function loadFixtureConfig(): FixtureConfig | null {
  const identity = process.env.REMOTEDECK_INTEGRATION_IDENTITY
  const directPort = parsePort(process.env.REMOTEDECK_DIRECT_PORT)
  const tunnelPort = parsePort(process.env.REMOTEDECK_REMOTE_FORWARD_PORT)
  return identity && directPort && tunnelPort ? { identity, directPort, tunnelPort } : null
}

function parsePort(value: string | undefined): number | null {
  if (!value || !/^\d+$/u.test(value)) return null
  const port = Number(value)
  return Number.isInteger(port) && port > 0 && port <= 65_535 ? port : null
}

async function openWithCsrf(page: Page): Promise<string> {
  let csrf = ''
  page.on('request', (request) => {
    const value = request.headers()['x-remotedeck-csrf']
    if (request.url().includes('/api/v1/') && value) csrf = value
  })
  await openLaunchedPage(page)
  await expect.poll(() => csrf, { timeout: 15_000 }).not.toBe('')
  return csrf
}

async function savePrivateKeyHost(page: Page, config: FixtureConfig, csrf: string): Promise<HostResponse> {
  const onboarding = page.getByRole('heading', { name: '欢迎使用 RemoteDeck' })
  if (await onboarding.isVisible().catch(() => false)) {
    await page.getByRole('button', { name: '添加第一台主机' }).click()
    await expect(onboarding).toBeHidden()
  } else {
    await page.getByRole('button', { name: '主机', exact: true }).click()
    await page.getByRole('button', { name: '新建', exact: true }).click()
  }
  await page.locator('input[placeholder="lab-gpu"]').fill('openssh-browser')
  await page.locator('input[placeholder="10.0.0.2"]').fill('127.0.0.1')
  await page.getByRole('spinbutton', { name: '端口' }).fill(String(config.directPort))
  await page.locator('input[placeholder="researcher"]').fill('remotedeck')
  await page.getByRole('combobox', { name: '认证方式' }).selectOption('private_key')
  await page.getByRole('textbox', { name: '私钥路径 选择文件', exact: true }).fill(config.identity)
  const saved = page.waitForResponse((response) => response.request().method() === 'POST' && response.url().endsWith('/api/v1/save_host'))
  await page.getByRole('button', { name: '保存' }).click()
  const response = await saved
  expect(response.ok()).toBe(true)
  expect(response.status()).toBe(202)
  await expect(page.getByRole('heading', { name: '主机 · openssh-browser', exact: true })).toBeVisible()
  const snapshot = await browserApi<{ hosts: Array<HostResponse & { alias: string }> }>(page, csrf, 'bootstrap', {})
  const host = snapshot.hosts.find((host) => host.alias === 'openssh-browser')
  expect(host?.id).toEqual(expect.any(String))
  if (!host) throw new Error('Saved fixture host was not returned by bootstrap.')
  return host
}

async function trustHostInUi(page: Page): Promise<void> {
  await page.getByRole('button', { name: '扫描指纹' }).click()
  const accept = page.getByRole('button', { name: '接受并保存' })
  await expect(accept).toBeVisible({ timeout: 30_000 })
  await accept.click()
  await expect(accept).toBeHidden({ timeout: 30_000 })
}

async function verifyUntrustedConnectionIsRejectedInUi(page: Page): Promise<void> {
  // The dedicated known_hosts starts empty. A real test before acceptance must fail rather than
  // implicitly trusting the fixture; it changes no persisted state and proves the UI reports it.
  await page.getByRole('button', { name: '测试连接' }).click()
  await expect(page.locator('.result-card.failed')).toContainText('连接失败', { timeout: 30_000 })
}

async function verifyTerminalReconnect(page: Page): Promise<string> {
  await page.getByRole('button', { name: '工作区' }).click()
  const started = page.waitForResponse((response) => response.request().method() === 'POST' && response.url().endsWith('/api/v1/start_terminal'))
  await page.getByLabel('新建终端').click()
  const startResponse = await started
  expect(startResponse.ok()).toBe(true)
  const terminal = await startResponse.json() as TerminalResponse
  expect(terminal.sessionId).toEqual(expect.any(String))
  await sendAndExpectTerminalEcho(page, `browser-terminal-${String(Date.now())}`)

  const listed = page.waitForResponse((response) => response.request().method() === 'POST' && response.url().endsWith('/api/v1/list_terminals'))
  await page.reload()
  await page.getByRole('button', { name: '工作区' }).click()
  const listResponse = await listed
  const sessions = await listResponse.json() as Array<TerminalResponse>
  expect(sessions.map((item) => item.sessionId)).toContain(terminal.sessionId)
  await sendAndExpectTerminalEcho(page, `browser-reconnect-${String(Date.now())}`)
  return terminal.sessionId
}

async function sendAndExpectTerminalEcho(page: Page, marker: string): Promise<void> {
  const surface = page.locator('.terminal-surface.active')
  await expect(surface).toHaveCount(1, { timeout: 30_000 })
  await expect(surface.getByRole('status').filter({ hasText: '已获得输入控制权' })).toBeVisible({ timeout: 30_000 })
  // xterm deliberately hides its helper textarea. Programmatic focus is the supported input path.
  const input = surface.locator('.xterm-helper-textarea')
  await input.focus()
  await page.keyboard.type(`printf '__%s__\\n' '${marker}'`)
  await page.keyboard.press('Enter')
  await expect(surface.locator('.xterm-rows')).toContainText(`__${marker}__`, { timeout: 30_000 })
}

async function verifyLowRiskCommandInUi(page: Page): Promise<void> {
  const marker = `browser-command-${String(Date.now())}`
  await page.getByRole('button', { name: '命令与 Agent', exact: true }).click()
  await page.locator('.quick-command textarea').fill(`printf '${marker}'`)
  await page.getByRole('button', { name: '分析并执行' }).click()
  await expect(page.getByRole('dialog')).toBeVisible({ timeout: 30_000 })
  await page.getByRole('button', { name: '确认执行' }).click()
  await expect(page.locator('.command-jobs pre')).toContainText(marker, { timeout: 30_000 })
}

async function verifyTransferThroughAuthenticatedApi(page: Page, csrf: string, hostId: string): Promise<void> {
  const sourceDirectory = await mkdtemp(join(tmpdir(), 'remotedeck-browser-transfer-source-'))
  const downloadDirectory = await mkdtemp(join(tmpdir(), 'remotedeck-browser-transfer-download-'))
  const marker = `browser-transfer-${String(Date.now())}`
  const filename = `${marker}.txt`
  const source = join(sourceDirectory, filename)
  try {
    await writeFile(source, marker, 'utf8')
    // Native system pickers cannot be driven in a headless browser. This still exercises the
    // real browser-authenticated transport API and the service-owned SFTP transfer workers.
    const upload = await browserApi<TransferResponse>(page, csrf, 'transfer_upload', { request: { hostId, source, destination: '/tmp', conflictPolicy: 'overwrite', recursive: false } })
    await waitForTransfer(page, csrf, hostId, upload.id)
    const listing = await browserApi<{ entries: Array<{ name: string }> }>(page, csrf, 'sftp_list', { hostId, path: '/tmp' })
    expect(listing.entries.map((entry) => entry.name)).toContain(filename)
    const download = await browserApi<TransferResponse>(page, csrf, 'transfer_download', { request: { hostId, source: `/tmp/${filename}`, destination: downloadDirectory, conflictPolicy: 'overwrite', recursive: false } })
    await waitForTransfer(page, csrf, hostId, download.id)
    await expect.poll(async () => readFile(join(downloadDirectory, filename), 'utf8'), { timeout: 30_000 }).toBe(marker)
    await browserApi<undefined>(page, csrf, 'sftp_delete', { hostId, path: `/tmp/${filename}`, recursive: false })
  } finally {
    await Promise.all([
      rm(sourceDirectory, { recursive: true, force: true }),
      rm(downloadDirectory, { recursive: true, force: true })
    ])
  }
}

async function waitForTransfer(page: Page, csrf: string, hostId: string, id: string): Promise<void> {
  await expect.poll(async () => {
    const jobs = await browserApi<TransferState[]>(page, csrf, 'transfer_list', { hostId })
    return jobs.find((job) => job.id === id) ?? null
  }, { timeout: 45_000 }).toMatchObject({ id, state: 'completed', error: null })
}

async function verifyLocalTunnelThroughAuthenticatedApi(page: Page, csrf: string, hostId: string, sourcePort: number): Promise<void> {
  const name = `browser-tunnel-${String(Date.now())}`
  const profile = await browserApi<{ id: string }>(page, csrf, 'save_tunnel', { draft: { hostId, name, direction: 'local', bindAddress: '127.0.0.1', sourcePort, targetHost: '127.0.0.1', targetPort: 18_080, autoStart: false, autoReconnect: false, healthCheck: { kind: 'tcp', intervalSeconds: 3, timeoutSeconds: 2 } } })
  try {
    await browserApi(page, csrf, 'start_tunnel', { tunnelId: profile.id })
    await expect.poll(async () => {
      const snapshots = await browserApi<Array<{ tunnelId: string; state: string }>>(page, csrf, 'list_tunnels', { hostId })
      return snapshots.find((item) => item.tunnelId === profile.id)?.state
    }, { timeout: 30_000 }).toBe('running')
    await expect.poll(async () => (await fetch(`http://127.0.0.1:${String(sourcePort)}/`)).status, { timeout: 15_000 }).toBe(200)
    await browserApi(page, csrf, 'stop_tunnel', { tunnelId: profile.id })
    await expect.poll(async () => {
      const snapshots = await browserApi<Array<{ tunnelId: string; state: string }>>(page, csrf, 'list_tunnels', { hostId })
      return snapshots.find((item) => item.tunnelId === profile.id)?.state
    }, { timeout: 30_000 }).toBe('stopped')
  } finally {
    await browserApi<undefined>(page, csrf, 'delete_tunnel', { tunnelId: profile.id })
  }
}

async function browserApi<T>(page: Page, csrf: string, command: string, args: Record<string, unknown>): Promise<T> {
  const response = await page.evaluate(async ({ csrfToken, operation, request }) => {
    const invoke = async (url: string, init: RequestInit): Promise<{ status: number; body: unknown }> => {
      const response = await fetch(url, init)
      const text = await response.text()
      let body: unknown = null
      if (text) body = JSON.parse(text) as unknown
      return { status: response.status, body }
    }
    const initial = await invoke(`/api/v1/${operation}`, { method: 'POST', credentials: 'same-origin', headers: { 'Content-Type': 'application/json', 'X-RemoteDeck-CSRF': csrfToken, 'Idempotency-Key': crypto.randomUUID() }, body: JSON.stringify(request) })
    if (initial.status !== 202) return initial
    const operationId = (initial.body as { operationId?: unknown }).operationId
    if (typeof operationId !== 'string') throw new Error('long operation response lacked operationId')
    for (;;) {
      const poll = await invoke(`/api/v1/operations/${encodeURIComponent(operationId)}`, { credentials: 'same-origin', headers: { 'X-RemoteDeck-CSRF': csrfToken } })
      if (poll.status >= 400) return poll
      const state = poll.body as { state?: unknown; result?: unknown; error?: { code?: unknown; message?: unknown } }
      if (state.state === 'completed') return { status: 200, body: state.result }
      if (state.state === 'failed') return { status: 500, body: state.error ?? { code: 'operation_failed', message: 'operation failed' } }
      await new Promise<void>((resolve) => window.setTimeout(resolve, 250))
    }
  }, { csrfToken: csrf, operation: command, request: args })
  if (response.status >= 400) {
    const error = response.body as { code?: unknown; message?: unknown }
    throw new Error(typeof error.message === 'string' ? error.message : `API ${command} failed with HTTP ${String(response.status)}`)
  }
  return response.body as T
}
