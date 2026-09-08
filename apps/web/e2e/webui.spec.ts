import { expect, test } from '@playwright/test'
import { openLaunchedPage } from './helpers'

test('authenticates a launched browser, saves a host, restores it, and exposes the text navigation', async ({ page }) => {
  await openLaunchedPage(page)
  const aliasName = `e2e-host-${String(Date.now())}`
  const onboarding = page.getByRole('heading', { name: '欢迎使用 RemoteDeck' })
  if (await onboarding.isVisible().catch(() => false)) {
    await page.getByRole('button', { name: '添加第一台主机' }).click()
  } else {
    await page.getByRole('button', { name: '主机', exact: true }).click()
    await page.getByRole('button', { name: '新建', exact: true }).click()
  }
  const alias = page.locator('input[placeholder="lab-gpu"]')
  await alias.fill(aliasName)
  await expect(alias).toHaveValue(aliasName)
  await page.locator('input[placeholder="10.0.0.2"]').fill('192.0.2.10')
  await page.locator('input[placeholder="researcher"]').fill('tester')
  await page.getByRole('button', { name: '保存' }).click()
  await expect(page.getByRole('heading', { name: `主机 · ${aliasName}` })).toBeVisible()
  await expect(page).not.toHaveURL(/ticket=/)
  await page.reload()
  await expect(page.getByText(aliasName, { exact: true }).first()).toBeVisible()
  await page.locator('.host-list .host-item').filter({ hasText: aliasName }).click()
  await page.getByRole('button', { name: '工作区' }).click()
  await expect(page.getByRole('button', { name: '终端', exact: true })).toBeVisible()
  await expect(page.getByText(`${aliasName} · ~`)).toBeVisible()
  await page.getByRole('button', { name: '任务' }).click()
  await expect(page.getByRole('heading', { name: '任务' })).toBeVisible()
  await page.getByRole('button', { name: '设置' }).click()
  await expect(page.getByRole('heading', { name: '设置' })).toBeVisible()
})
