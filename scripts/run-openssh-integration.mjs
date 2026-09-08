import { spawn } from 'node:child_process'
import { mkdtemp, rm, chmod } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createServer } from 'node:net'

const root = fileURLToPath(new URL('..', import.meta.url))
// An alternate registry may serve the same immutable fixture image.
if (process.env.REMOTEDECK_FIXTURE_BASE_IMAGE && !process.env.REMOTEDECK_FIXTURE_BASE_IMAGE.endsWith('@sha256:52df9b1ee71626e0088f7d400d5c6b5f7bb916f8f0c82b474289a4ece6cf3faf')) {
  throw new Error('The fixture base image must retain the pinned Ubuntu content digest.')
}
const keyDir = await mkdtemp(join(tmpdir(), 'remotedeck-openssh-key-'))
const freePort = () => new Promise((accept, reject) => {
  const server = createServer()
  server.once('error', reject)
  server.listen(0, '127.0.0.1', () => { const port = server.address().port; server.close(() => accept(port)) })
})
const env = { ...process.env,
  REMOTEDECK_DIRECT_PORT: process.env.REMOTEDECK_DIRECT_PORT ?? String(await freePort()),
  REMOTEDECK_JUMP_PORT: process.env.REMOTEDECK_JUMP_PORT ?? String(await freePort()),
  REMOTEDECK_REMOTE_FORWARD_PORT: process.env.REMOTEDECK_REMOTE_FORWARD_PORT ?? '23080',
  REMOTEDECK_INTEGRATION_IDENTITY: join(keyDir, 'id_ed25519'),
  REMOTEDECK_FIXTURE_AUTHORIZED_KEYS: join(keyDir, 'id_ed25519.pub'),
  COMPOSE_PROJECT_NAME: `remotedeck-test-${process.pid}-${Date.now()}`
}
const compose = ['compose', '--file', resolve(root, 'tests/fixtures/openssh/compose.yml')]
const run = (command, args, options = {}) => new Promise((accept, reject) => {
  // Fixture output remains attached to the invoking terminal. The final
  // windowsHide assignment is intentional: per-call options must not be able
  // to make a child create a visible Windows console.
  const child = spawn(command, args, { cwd: root, env, stdio: 'inherit', ...options, windowsHide: true })
  child.once('error', reject)
  child.once('exit', code => code === 0 ? accept() : reject(new Error(`${command} exited with status ${code}`)))
})
try {
  await run('docker', ['info', '--format', '{{.ServerVersion}}'])
  await run('docker', ['compose', 'version'])
  await run('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-f', env.REMOTEDECK_INTEGRATION_IDENTITY])
  if (process.platform !== 'win32') { await chmod(env.REMOTEDECK_INTEGRATION_IDENTITY, 0o600); await chmod(env.REMOTEDECK_FIXTURE_AUTHORIZED_KEYS, 0o644) }
  await run('docker', [...compose, 'build'])
  await run('docker', [...compose, 'up', '--detach', '--wait', '--wait-timeout', '120'])
  await run('cargo', ['test', '-p', 'remotedeck-core', '--locked', '--all-features', 'openssh_integration', '--', '--ignored', '--test-threads=1'])
  if (process.argv.includes('--browser')) {
    await run(process.execPath, [resolve(root, 'apps/web/node_modules/@playwright/test/cli.js'), 'test'], { cwd: resolve(root, 'apps/web') })
  }
} catch (error) {
  await run('docker', [...compose, 'logs', '--no-color', '--tail', '100']).catch(() => {})
  throw error
} finally {
  await run('docker', [...compose, 'down', '--volumes', '--remove-orphans']).catch(() => {})
  const target = resolve(keyDir)
  if (!target.startsWith(resolve(tmpdir()) + (process.platform === 'win32' ? '\\' : '/')) || !target.includes('remotedeck-openssh-key-')) throw new Error('refusing unexpected cleanup path')
  await rm(target, { recursive: true, force: true })
}
