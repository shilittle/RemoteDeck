import { randomUUID } from 'node:crypto'
import { existsSync } from 'node:fs'
import { readFile } from 'node:fs/promises'
import { spawn } from 'node:child_process'
import { join, resolve } from 'node:path'
import type { Page } from '@playwright/test'

const runtimeFile = resolve(import.meta.dirname, '..', '.e2e-runtime.json')

interface Runtime { dataDir: string; executable: string; webUrl: string | null }
interface LaunchInfo { baseUrl: string; browserUrl: string }

export async function openLaunchedPage(page: Page): Promise<void> {
  const runtime = JSON.parse(await readFile(runtimeFile, 'utf8')) as Partial<Runtime>
  if (typeof runtime.dataDir !== 'string' || typeof runtime.executable !== 'string' || (runtime.webUrl !== null && typeof runtime.webUrl !== 'string')) throw new Error('E2E service runtime is unavailable.')
  const browserUrl = await issueLaunchTicket(runtime as Runtime)
  const url = new URL(browserUrl)
  if (runtime.webUrl) {
    const webUrl = new URL(runtime.webUrl)
    url.protocol = webUrl.protocol
    url.host = webUrl.host
  }
  await page.goto(url.toString())
}

async function issueLaunchTicket(runtime: Runtime): Promise<string> {
  const launchFile = join(runtime.dataDir, `browser-${randomUUID()}.json`)
  const child = spawn(runtime.executable, ['--data-dir', runtime.dataDir, '--no-open', '--launch-file', launchFile], { stdio: 'ignore', windowsHide: true })
  const launch = await waitForLaunch(child, launchFile)
  await waitForExit(child)
  return launch.browserUrl
}

async function waitForLaunch(child: ReturnType<typeof spawn>, launchFile: string): Promise<LaunchInfo> {
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    if (existsSync(launchFile)) {
      const launch = JSON.parse(await readFile(launchFile, 'utf8')) as Partial<LaunchInfo>
      if (typeof launch.baseUrl === 'string' && typeof launch.browserUrl === 'string') return launch as LaunchInfo
    }
    if (child.exitCode !== null) throw new Error('RemoteDeck did not issue a one-time browser ticket.')
    await new Promise<void>((resolveDelay) => setTimeout(resolveDelay, 50))
  }
  throw new Error('RemoteDeck did not issue a browser ticket within 30 seconds.')
}

async function waitForExit(child: ReturnType<typeof spawn>): Promise<void> {
  if (child.exitCode !== null) return
  await new Promise<void>((resolveExit) => {
    child.once('exit', () => resolveExit())
    child.once('error', () => resolveExit())
  })
}
