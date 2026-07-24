import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { _electron as electron, expect, test } from '@playwright/test'
import type { ElectronApplication } from '@playwright/test'
import type { RemoteDeckApi } from '../../src/protocol/ipc'

test('renderer is sandboxed and settings round-trip through validated IPC', async () => {
  test.setTimeout(60_000)
  const appDirectory = resolve(import.meta.dirname, '../..')
  const userData = await mkdtemp(join(tmpdir(), 'remotedeck-e2e-'))
  let application: ElectronApplication | null = null
  try {
    application = await test.step('launch the packaged renderer build', () => electron.launch({
      args: [appDirectory],
      cwd: appDirectory,
      env: { ...process.env, REMOTEDECK_E2E_USER_DATA: userData, ELECTRON_DISABLE_SECURITY_WARNINGS: 'true' }
    }))
    await test.step('verify sandbox and validated settings IPC', async () => {
      if (!application) throw new Error('Electron application did not launch')
      const firstWindow = await application.firstWindow()
      await firstWindow.waitForLoadState('domcontentloaded')
      expect(await firstWindow.locator('body').innerText()).toContain('RemoteDeck')
      await firstWindow.getByRole('button', { name: '添加第一台主机' }).click()
      const boundary = await firstWindow.evaluate(() => ({
        nodeGlobalPresent: 'process' in globalThis,
        apiKeys: Object.keys((globalThis as unknown as { remoteDeck: RemoteDeckApi }).remoteDeck).sort()
      }))
      expect(boundary).toEqual({ nodeGlobalPresent: false, apiKeys: ['app', 'codex', 'commands', 'hostKeys', 'hosts', 'keys', 'legacy', 'settings', 'sftp', 'telemetry', 'terminals', 'tunnels'] })
      await firstWindow.evaluate(() => (globalThis as unknown as { remoteDeck: RemoteDeckApi }).remoteDeck.settings.update({ terminalFontSize: 17 }))
      const sshConfigPath = join(userData, '.ssh', 'config')
      await firstWindow.evaluate((configPath) => (globalThis as unknown as { remoteDeck: RemoteDeckApi }).remoteDeck.settings.update({ sshConfigPath: configPath }), sshConfigPath)
      const snapshot = await firstWindow.evaluate(() => (globalThis as unknown as { remoteDeck: RemoteDeckApi }).remoteDeck.app.bootstrap())
      expect(snapshot.settings.terminalFontSize).toBe(17)
      await firstWindow.getByLabel('别名').fill('e2e-host')
      await firstWindow.getByLabel('地址').fill('127.0.0.1')
      await firstWindow.getByLabel('用户名').fill('developer')
      await firstWindow.getByRole('button', { name: '保存主机' }).click()
      await expect(firstWindow.getByRole('heading', { name: 'e2e-host' })).toBeVisible()
      await firstWindow.getByRole('button', { name: '完成向导' }).click()
      expect(await readFile(sshConfigPath, 'utf8')).toContain('Include')
      expect(await readFile(join(userData, '.ssh', 'remotedeck.conf'), 'utf8')).toContain('Host e2e-host')
      await firstWindow.getByRole('button', { name: '文件' }).click()
      await expect(firstWindow.getByRole('heading', { name: 'e2e-host 未连接' })).toBeVisible()
      await firstWindow.getByRole('button', { name: '隧道' }).click()
      await firstWindow.getByRole('button', { name: '新建隧道' }).click()
      await firstWindow.getByLabel('名称').fill('e2e-local-forward')
      await firstWindow.getByLabel('本地监听端口').fill('18081')
      await firstWindow.getByLabel('目标端口').fill('8080')
      await firstWindow.getByRole('button', { name: '保存配置' }).click()
      await expect(firstWindow.getByRole('heading', { name: 'e2e-local-forward' })).toBeVisible()
      expect(await firstWindow.evaluate(() => (globalThis as unknown as { remoteDeck: RemoteDeckApi }).remoteDeck.tunnels.list())).toHaveLength(1)
      await firstWindow.getByRole('button', { name: '监控' }).click()
      await expect(firstWindow.getByRole('heading', { name: '系统监控' })).toBeVisible()
      await expect(firstWindow.getByText('collector v1 JSONL', { exact: false })).toBeVisible()
      await firstWindow.locator('.activity[aria-label="命令"]').click()
      await expect(firstWindow.getByRole('heading', { name: '命令与 Codex' })).toBeVisible()
      await firstWindow.getByRole('button', { name: '新建预设' }).click()
      const editor = firstWindow.locator('.preset-editor')
      await editor.getByLabel('名称').fill('E2E 全局预设')
      await editor.getByLabel('命令').fill('git status --short --branch')
      await editor.getByLabel('风险').selectOption('L0')
      await editor.getByText('全局预设').click()
      await editor.getByRole('button', { name: '保存' }).click()
      await expect(firstWindow.getByRole('heading', { name: 'E2E 全局预设' })).toBeVisible()
      const savedCommands = await firstWindow.evaluate(async () => {
        const api = (globalThis as unknown as { remoteDeck: RemoteDeckApi }).remoteDeck
        const host = (await api.hosts.list())[0]
        if (!host) throw new Error('Missing E2E host')
        return api.commands.list(host.host.id)
      })
      expect(savedCommands.find((item) => item.name === 'E2E 全局预设')?.hostId).toBeUndefined()
      await firstWindow.locator('.activity[aria-label="设置"]').click()
      const legacyPath = resolve(import.meta.dirname, '../../../../legacy/labpulse-v0.1.0/config.json')
      await firstWindow.getByLabel('旧版 config.json 路径').fill(legacyPath)
      await firstWindow.getByRole('button', { name: '预览' }).click()
      await expect(firstWindow.getByText('高风险旧版清理命令将以禁用状态导入')).toBeVisible()
      await firstWindow.getByLabel('RemoteDeck 别名').fill('legacy-e2e')
      await firstWindow.getByLabel('真实主机名 / IP').fill('127.0.0.1')
      await firstWindow.getByLabel('远端用户名').fill('legacy-user')
      await firstWindow.getByRole('button', { name: '确认导入' }).click()
      await expect(firstWindow.getByText('导入完成：1 台主机、1 条隧道、8 个命令预设。')).toBeVisible()
      await firstWindow.getByRole('button', { name: '预览' }).click()
      await expect(firstWindow.getByText('已导入', { exact: true })).toBeVisible()
    })
  } finally {
    if (application) await test.step('close Electron', () => application?.close())
    await test.step('remove isolated user data', () => rm(userData, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 }))
  }
})
