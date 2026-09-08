import { createServer } from 'node:net'
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { spawn, type ChildProcess } from 'node:child_process'

const webRoot = resolve(import.meta.dirname, '..')
const repositoryRoot = resolve(webRoot, '..', '..')
const runtimeFile = join(webRoot, '.e2e-runtime.json')

interface LaunchInfo { baseUrl: string; browserUrl: string }
interface Backend { executable: string }

export default async function globalSetup(): Promise<() => Promise<void>> {
  const dataDir = await mkdtemp(join(tmpdir(), 'remotedeck-e2e-'))
  // Keep host-trust discovery isolated from the developer's actual SSH files.
  await mkdir(join(dataDir, 'home', '.ssh'), { recursive: true })
  const launchFile = join(dataDir, 'launch.json')
  const port = await freePort()
  const viteMode = process.env.REMOTEDECK_E2E_VITE === '1'
  const backend = await prepareBackend(dataDir)
  const serverArgs = ['--data-dir', dataDir, '--port', String(port), '--no-open', '--launch-file', launchFile]
  if (viteMode) serverArgs.splice(6, 0, '--dev-origin', 'http://127.0.0.1:1420')
  const server = launchBackend(backend, serverArgs, 'pipe')
  const launch = await waitForLaunch(server, launchFile)
  const vite = viteMode
    ? spawn(process.execPath, [join(webRoot, 'node_modules', 'vite', 'bin', 'vite.js'), '--host', '127.0.0.1', '--port', '1420'], { cwd: webRoot, stdio: 'pipe', windowsHide: true, env: { ...process.env, REMOTEDECK_BACKEND_URL: launch.baseUrl } })
    : null
  if (vite) await waitForHttp('http://127.0.0.1:1420/')
  // Tests request their tickets immediately before navigation, so queued suites never receive
  // an expired one-time ticket. By default they load the assets embedded in the copied server.
  await writeFile(runtimeFile, JSON.stringify({ dataDir, executable: backend.executable, webUrl: vite ? 'http://127.0.0.1:1420' : null }), 'utf8')

  return async () => {
    vite?.kill()
    await stopServer(backend, dataDir)
    if (!await waitForExit(server, 10_000)) {
      server.kill()
      await waitForExit(server, 5_000)
    }
    if (vite) await waitForExit(vite, 5_000)
    await rm(runtimeFile, { force: true })
    await rm(dataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 })
  }
}

async function prepareBackend(dataDir: string): Promise<Backend> {
  const filename = process.platform === 'win32' ? 'RemoteDeck.exe' : 'RemoteDeck'
  const built = join(repositoryRoot, 'target', 'debug', filename)
  await buildWeb()
  await buildBackend()
  if (!existsSync(built)) throw new Error(`RemoteDeck test server binary was not built at ${built}.`)
  const executable = join(dataDir, `remotedeck-e2e-${filename}`)
  await copyFile(built, executable)
  return { executable }
}

async function buildBackend(): Promise<void> {
  await runBuild('cargo', ['build', '-p', 'remotedeck-server'], repositoryRoot, 'RemoteDeck test service')
}

async function buildWeb(): Promise<void> {
  await runBuild(process.execPath, [join(webRoot, 'node_modules', 'vite', 'bin', 'vite.js'), 'build', '--config', 'vite.config.ts'], webRoot, 'embedded WebUI')
}

async function runBuild(command: string, args: string[], cwd: string, label: string): Promise<void> {
  const child = spawn(command, args, { cwd, stdio: 'pipe', windowsHide: true })
  let output = ''
  const capture = (chunk: Buffer): void => { output = `${output}${chunk.toString('utf8')}`.slice(-12_000) }
  child.stdout.on('data', capture)
  child.stderr.on('data', capture)
  if (!await waitForExit(child, 120_000)) {
    child.kill()
    throw new Error(`${label} build timed out.\n${output}`)
  }
  if (child.exitCode !== 0) throw new Error(`${label} build failed.\n${output}`)
}

function launchBackend(backend: Backend, args: string[], stdio: 'pipe' | 'ignore'): ChildProcess {
  const dataDir = args[args.indexOf('--data-dir') + 1]
  const home = join(dataDir, 'home')
  return spawn(backend.executable, args, { cwd: repositoryRoot, stdio, windowsHide: true, env: { ...process.env, USERPROFILE: home, HOME: home } })
}

async function freePort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = createServer()
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const address = server.address()
      server.close((error) => error ? reject(error) : typeof address === 'object' && address ? resolvePort(address.port) : reject(new Error('unable to allocate E2E port')))
    })
  })
}

async function waitForLaunch(process: ChildProcess, path: string): Promise<LaunchInfo> {
  const deadline = Date.now() + 45_000
  while (Date.now() < deadline) {
    if (existsSync(path)) {
      const value = JSON.parse(await readFile(path, 'utf8')) as Partial<LaunchInfo>
      if (typeof value.baseUrl === 'string' && typeof value.browserUrl === 'string') return { baseUrl: value.baseUrl, browserUrl: value.browserUrl }
    }
    if (process.exitCode !== null) throw new Error('RemoteDeck test service exited before issuing a browser launch URL.')
    await delay(100)
  }
  throw new Error('RemoteDeck test service did not issue a browser launch URL within 45 seconds.')
}

async function waitForHttp(url: string): Promise<void> {
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try { if ((await fetch(url)).ok) return } catch { /* server is still starting */ }
    await delay(100)
  }
  throw new Error('Vite WebUI did not start within 30 seconds.')
}

async function stopServer(backend: Backend, dataDir: string): Promise<void> {
  const child = launchBackend(backend, ['--data-dir', dataDir, '--stop'], 'ignore')
  await waitForExit(child, 30_000)
}

async function waitForExit(child: ChildProcess, timeoutMs = 30_000): Promise<boolean> {
  if (child.exitCode !== null) return true
  return new Promise<boolean>((resolveExit) => {
    const timeout = setTimeout(() => resolveExit(false), timeoutMs)
    child.once('exit', () => { clearTimeout(timeout); resolveExit(true) })
    child.once('error', () => { clearTimeout(timeout); resolveExit(true) })
  })
}

function delay(milliseconds: number): Promise<void> { return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds)) }
