import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { _electron as electron, expect, test } from '@playwright/test'
import type { ElectronApplication } from '@playwright/test'
import type { RemoteDeckApi } from '../../src/protocol/ipc'
import ssh2 from 'ssh2'
import type { Connection, Server as SshServer } from 'ssh2'

const { Server, utils } = ssh2
let server: SshServer
let port = 0
let resizeEvents = 0
let shellCount = 0
const serverEvents: string[] = []
const connections = new Set<Connection>()

test.beforeAll(async () => {
  server = new Server({ hostKeys: [utils.generateKeyPairSync('ed25519').private] }, configureClient)
  port = await new Promise<number>((resolvePort, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const address = server.address()
      if (!address || typeof address === 'string') reject(new Error('SSH E2E server did not expose a port'))
      else resolvePort(address.port)
    })
  })
})

test.afterAll(async () => {
  for (const connection of connections) connection.end()
  await new Promise<void>((resolveClose) => server.close(() => resolveClose()))
})

test('real xterm PTY supports tabs, Unicode, Ctrl+C, search, resize, rename, and new-shell reconnect', async () => {
  test.setTimeout(60_000)
  const appDirectory = resolve(import.meta.dirname, '../..')
  const userData = await mkdtemp(join(tmpdir(), 'remotedeck-terminal-e2e-'))
  let application: ElectronApplication | null = null
  try {
    application = await electron.launch({ args: [appDirectory], cwd: appDirectory, env: { ...process.env, REMOTEDECK_E2E_USER_DATA: userData, ELECTRON_DISABLE_SECURITY_WARNINGS: 'true' } })
    const window = await application.firstWindow()
    const rendererErrors: string[] = []
    window.on('pageerror', (error) => rendererErrors.push(error.message))
    window.on('console', (message) => { if (message.type() === 'error' && !message.text().includes("frame-ancestors' is ignored")) rendererErrors.push(message.text()) })
    await window.waitForLoadState('domcontentloaded')
    await window.getByRole('button', { name: '添加第一台主机' }).click()
    const sshConfigPath = join(userData, '.ssh', 'config')
    await window.evaluate((configPath) => (globalThis as unknown as { remoteDeck: { settings: { update: (value: { sshConfigPath: string }) => Promise<unknown> } } }).remoteDeck.settings.update({ sshConfigPath: configPath }), sshConfigPath)
    await window.getByLabel('别名').fill('pty-e2e')
    await window.getByLabel('地址').fill('127.0.0.1')
    await window.getByLabel('端口').fill(String(port))
    await window.getByLabel('用户名').fill('terminal-user')
    await window.getByRole('button', { name: '保存主机' }).click()

    await window.getByLabel('密码 / 交互回答').fill('terminal-secret')
    await window.getByRole('button', { name: '连接', exact: true }).click()
    await window.getByRole('button', { name: '完成向导' }).click()
    await expect(window.getByRole('heading', { name: '首次连接：确认主机指纹' })).toBeVisible()
    await window.getByRole('button', { name: '接受并保存' }).click()
    await window.getByLabel('密码 / 交互回答').fill('terminal-secret')
    await window.getByRole('button', { name: '连接', exact: true }).click()

    await expect.poll(() => rendererErrors).toEqual([])
    await expect(window.locator('.xterm-screen')).toBeVisible()
    await expect.poll(async () => window.locator('.xterm-rows').innerText()).toContain('TERMINAL_READY 中文')
    const terminalSession = await window.evaluate(async () => {
      const api = (globalThis as unknown as { remoteDeck: { terminals: { list: () => Promise<Array<{ id: string }>>; write: (id: string, data: string) => Promise<unknown> } } }).remoteDeck
      const session = (await api.terminals.list())[0]
      if (!session) throw new Error('Missing terminal session')
      return session
    })
    if ((terminalSession as { state?: string }).state !== 'online') throw new Error(`Terminal closed unexpectedly: ${JSON.stringify(terminalSession)} ${JSON.stringify(serverEvents)}`)
    await window.evaluate(async (sessionId) => (globalThis as unknown as { remoteDeck: { terminals: { write: (id: string, data: string) => Promise<unknown> } } }).remoteDeck.terminals.write(sessionId, 'api-input'), terminalSession.id)
    await expect.poll(async () => window.locator('.xterm-rows').innerText()).toContain('api-input')
    await window.locator('.xterm-helper-textarea').click()
    await window.locator('.xterm-helper-textarea').pressSequentially('unicode-input')
    await expect.poll(async () => window.locator('.xterm-rows').innerText()).toContain('unicode-input')
    await window.keyboard.insertText('中文路径')
    await expect.poll(async () => window.locator('.xterm-rows').innerText()).toContain('中文路径')
    await window.evaluate(async () => (globalThis as unknown as { remoteDeck: { app: { clipboardWrite: (text: string) => Promise<unknown> } } }).remoteDeck.app.clipboardWrite('clipboard-input'))
    await window.getByRole('button', { name: '粘贴' }).click()
    await expect.poll(async () => window.locator('.xterm-rows').innerText()).toContain('clipboard-input')
    await window.locator('.xterm-helper-textarea').click()
    await window.keyboard.press('Control+C')
    await expect.poll(async () => window.locator('.xterm-rows').innerText()).toContain('CTRL_C_OK')
    await window.getByLabel('终端搜索').fill('CTRL_C_OK')
    await expect(window.getByRole('tab')).toHaveCount(1)
    await window.getByRole('button', { name: '新建终端' }).click()
    await expect(window.getByRole('tab')).toHaveCount(2)

    await window.getByRole('button', { name: '重命名标签' }).click()
    await window.getByLabel('终端标签名称').fill('交互终端')
    await window.getByLabel('终端标签名称').press('Enter')
    await expect(window.getByRole('tab', { name: /交互终端/ })).toBeVisible()
    await window.getByRole('button', { name: '新建 shell 重连' }).click()
    await expect.poll(async () => window.locator('.terminal-surface.active .xterm-rows').innerText()).toContain('RemoteDeck 正在建立新的 SSH shell')
    await expect.poll(() => resizeEvents).toBeGreaterThan(0)
    await window.getByRole('button', { name: '关闭终端 交互终端' }).click()
    await expect(window.getByRole('tab')).toHaveCount(1)
    await window.getByRole('button', { name: '监控' }).click()
    await expect(window.getByText('12.5%')).toBeVisible()
    await expect(window.getByText('未检测到 NVIDIA GPU；监控已正常降级。')).toBeVisible()
    const activeHost = await window.evaluate(async () => (await (globalThis as unknown as { remoteDeck: RemoteDeckApi }).remoteDeck.hosts.list())[0]?.host.id)
    if (!activeHost) throw new Error('Missing E2E host for command test')
    await window.evaluate(async ({ hostId }) => (globalThis as unknown as { remoteDeck: RemoteDeckApi }).remoteDeck.commands.create({ hostId, name: 'E2E 高风险命令', description: '验证 L2 主进程确认', group: 'E2E', command: 'rm -rf /tmp/remotedeck-e2e-never-created', risk: 'L0', requiresPty: false, requiresSudo: false, sortOrder: 1 }), { hostId: activeHost })
    await window.locator('.activity[aria-label="命令"]').click()
    const commandCard = window.locator('.command-card').filter({ hasText: 'E2E 高风险命令' })
    await commandCard.getByRole('button', { name: '运行' }).click()
    await expect(window.getByRole('heading', { name: '确认 L2 命令' })).toBeVisible()
    const confirmInput = window.locator('.command-modal input')
    await confirmInput.fill('wrong')
    await expect(window.getByRole('button', { name: '确认执行' })).toBeDisabled()
    await confirmInput.fill('pty-e2e')
    await window.getByRole('button', { name: '确认执行' }).click()
    await expect.poll(async () => window.locator('.command-jobs').innerText()).toContain('COMMAND_E2E_OK')
    await window.keyboard.press('Control+J')
    await expect(window.getByRole('region', { name: '任务中心' })).toBeVisible()
    await expect(window.getByRole('region', { name: '任务中心' })).toContainText('E2E 高风险命令')
    await window.keyboard.press('Control+J')
    await expect(window.getByText('已登录', { exact: true })).toBeVisible()
    await window.getByRole('button', { name: '普通 Codex 终端' }).click()
    await expect.poll(async () => window.locator('.terminal-surface.active .xterm-rows').innerText()).toContain('exec codex')
    await application.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.close())
    await expect.poll(() => application?.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.isVisible())).toBe(false)
    await expect.poll(async () => (await window.evaluate(() => (globalThis as unknown as { remoteDeck: RemoteDeckApi }).remoteDeck.hosts.list()))[0]?.state).toBe('online')
    await application.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.show())
  } finally {
    if (application) await application.close()
    await rm(userData, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 })
  }
})

function configureClient(client: Connection): void {
  connections.add(client)
  client.on('error', () => undefined)
  client.once('close', () => connections.delete(client))
  client.on('authentication', (context) => context.method === 'password' && context.username === 'terminal-user' && context.password === 'terminal-secret' ? context.accept() : context.reject(['password']))
  client.on('ready', () => {
    client.on('session', (accept) => {
      const session = accept()
      session.on('pty', (acceptPty) => acceptPty())
      session.on('window-change', () => { resizeEvents += 1 })
      session.on('sftp', (_acceptSftp, rejectSftp) => rejectSftp())
      session.on('exec', (acceptExec, _rejectExec, info) => {
        const stream = acceptExec()
        if (info.command.includes('python3 -u -')) {
          stream.resume()
          stream.once('end', () => stream.write(`${JSON.stringify(telemetryPayload())}\n`))
        } else if (info.command.includes('command -v codex')) { stream.write('/mock/bin/codex\n'); stream.exit(0); stream.end() }
        else if (info.command === 'codex --version') { stream.write('codex-cli 1.2.3\n'); stream.exit(0); stream.end() }
        else if (info.command === 'codex login status') { stream.write('Logged in using ChatGPT\n'); stream.exit(0); stream.end() }
        else if (info.command === 'codex --help') { stream.write('Commands: login resume update\n'); stream.exit(0); stream.end() }
        else if (info.command === 'codex login --help') { stream.write('Usage: codex login --device-auth\n'); stream.exit(0); stream.end() }
        else if (info.command === 'codex resume --help') { stream.write('Usage: codex resume --last\n'); stream.exit(0); stream.end() }
        else if (info.command === 'codex update --help') { stream.write('Usage: codex update\n'); stream.exit(0); stream.end() }
        else if (info.command.includes('command -v tmux')) { stream.write('/usr/bin/tmux\n'); stream.exit(0); stream.end() }
        else if (info.command.includes('remotedeck-e2e-never-created')) { stream.write('COMMAND_E2E_OK\n'); stream.exit(0); stream.end() }
        else { stream.write('0 0'); stream.exit(0); stream.end() }
      })
      session.on('shell', (acceptShell) => {
        shellCount += 1
        const shellId = shellCount
        const stream = acceptShell()
        serverEvents.push(`shell-${String(shellId)}-open`)
        stream.on('end', () => serverEvents.push(`shell-${String(shellId)}-end`))
        stream.on('close', () => serverEvents.push(`shell-${String(shellId)}-close`))
        stream.on('error', (error: Error) => serverEvents.push(`shell-${String(shellId)}-error:${error.message}`))
        stream.write('TERMINAL_READY 中文\r\nhttps://example.com/remotedeck\r\n')
        stream.on('data', (data: Buffer) => {
          if (data.includes(3)) stream.write('^C\r\nCTRL_C_OK\r\n')
          else stream.write(data)
        })
      })
    })
  })
}

function telemetryPayload(): Record<string, unknown> {
  return {
    schemaVersion: 1,
    capturedAt: new Date().toISOString(),
    hostname: 'e2e-linux',
    currentUser: 'terminal-user',
    cpu: { totalPercent: 12.5, perCorePercent: [10, 15], loadAverage: [0.1, 0.2, 0.3], temperatureC: null },
    memory: { totalBytes: 1024, usedBytes: 512, swapTotalBytes: 0, swapUsedBytes: 0 },
    network: { receivedBytes: 100, sentBytes: 50, receiveBytesPerSecond: 10, sendBytesPerSecond: 5 },
    disks: [{ mount: '/', totalBytes: 1000, usedBytes: 400, availableBytes: 600 }],
    processes: [{ pid: 42, ppid: 1, user: 'terminal-user', cpuPercent: 1, memoryPercent: 0.5, state: 'S', elapsed: '60', command: 'sleep 60' }],
    gpus: [], gpuProcesses: [], uptimeSeconds: 100
  }
}
