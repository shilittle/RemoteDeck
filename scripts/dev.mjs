import { spawn } from 'node:child_process'
import { mkdir, copyFile, readFile, rm } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('..', import.meta.url))
const web = resolve(root, 'apps/web')
const vite = resolve(web, 'node_modules/vite/bin/vite.js')
const data = resolve(root, '.cache/development')
const executable = resolve(root, `.cache/development-bin/RemoteDeck-${process.pid}${process.platform === 'win32' ? '.exe' : ''}`)
const launchFile = resolve(data, `launch-${process.pid}.json`)
const children = new Set()
const run = (command, args, options = {}) => {
  // Keep the caller's terminal for development output, but never allow a
  // caller-specific option to re-enable a Windows console window.
  const child = spawn(command, args, { cwd: root, stdio: 'inherit', ...options, windowsHide: true })
  children.add(child)
  child.once('exit', () => children.delete(child))
  return child
}
const completed = child => new Promise((accept, reject) => {
  if (child.exitCode !== null) return child.exitCode === 0 ? accept() : reject(new Error(`Development child exited with status ${child.exitCode}`))
  child.once('error', reject)
  child.once('exit', code => code === 0 ? accept() : reject(new Error(`Development child exited with status ${code}`)))
})
const delay = ms => new Promise(resolve => setTimeout(resolve, ms))
let service
let ui
let stopping
async function stop() {
  if (stopping) return stopping
  stopping = (async () => {
    // The launcher exits immediately if this is an already-running instance.
    // Stop only the service process this development run actually started.
    if (service && service.exitCode === null) {
      await completed(run(executable, ['--data-dir', data, '--stop'])).catch(() => {})
      await Promise.race([completed(service).catch(() => {}), delay(15000)])
    }
    for (const child of children) child.kill()
    if (ui) await Promise.race([completed(ui).catch(() => {}), delay(2000)])
    await rm(launchFile, { force: true })
    await rm(executable, { force: true, maxRetries: 5, retryDelay: 100 }).catch(() => {})
  })()
  return stopping
}
process.once('SIGINT', () => { void stop() })
process.once('SIGTERM', () => { void stop() })
try {
  await completed(run(process.execPath, [vite, 'build'], { cwd: web }))
  await completed(run('cargo', ['build', '-p', 'remotedeck-server']))
  await mkdir(resolve(root, '.cache/development-bin'), { recursive: true })
  await mkdir(data, { recursive: true })
  await copyFile(resolve(root, `target/debug/RemoteDeck${process.platform === 'win32' ? '.exe' : ''}`), executable)
  service = run(executable, ['--data-dir', data, '--no-open', '--dev-origin', 'http://127.0.0.1:1420', '--launch-file', launchFile])
  let launch
  const deadline = Date.now() + 30000
  while (Date.now() < deadline) {
    try { launch = JSON.parse(await readFile(launchFile, 'utf8')); break } catch {}
    if (service.exitCode !== null && service.exitCode !== 0) throw new Error('Development service failed to start.')
    await delay(100)
  }
  if (!launch?.baseUrl) throw new Error('Development service did not become ready.')
  ui = run(process.execPath, [vite, '--host', '127.0.0.1', '--port', '1420', '--strictPort'], { cwd: web, env: { ...process.env, REMOTEDECK_BACKEND_URL: launch.baseUrl } })
  let ready = false
  const uiDeadline = Date.now() + 30000
  while (Date.now() < uiDeadline) {
    if (ui.exitCode !== null) throw new Error('Development WebUI could not bind port 1420.')
    try { if ((await fetch('http://127.0.0.1:1420/')).ok) { ready = true; break } } catch {}
    await delay(100)
  }
  if (!ready) throw new Error('Development WebUI did not become ready.')
  if (process.env.REMOTEDECK_DEV_SMOKE === '1') {
    if (!(await fetch(`${launch.baseUrl}/health`)).ok) throw new Error('Development API is not healthy.')
    console.log('Development WebUI and local API are ready; stopping this smoke-test instance.')
  } else {
    await completed(run(executable, ['--data-dir', data]))
    const work = [completed(ui)]
    if (service.exitCode === null) work.push(completed(service))
    await Promise.race(work)
  }
} finally { await stop() }
