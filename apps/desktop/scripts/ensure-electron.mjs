import { existsSync, readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'
import { spawn } from 'node:child_process'

const require = createRequire(import.meta.url)
const packageDirectory = dirname(require.resolve('electron/package.json'))
const installScript = join(packageDirectory, 'install.js')
const pathFile = join(packageDirectory, 'path.txt')
const maximumAttempts = 5

function installed() {
  if (!existsSync(pathFile)) return false
  const executable = readFileSync(pathFile, 'utf8').trim()
  return executable.length > 0 && existsSync(join(packageDirectory, 'dist', executable))
}

function runInstaller() {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [installScript], {
      cwd: packageDirectory,
      env: process.env,
      stdio: 'inherit'
    })
    child.once('error', () => resolve(false))
    child.once('exit', (code) => resolve(code === 0))
  })
}

if (!installed()) {
  for (let attempt = 1; attempt <= maximumAttempts; attempt += 1) {
    process.stdout.write(`Preparing Electron binary (attempt ${attempt}/${maximumAttempts})...\n`)
    const succeeded = await runInstaller()
    if (succeeded && installed()) break
    if (attempt < maximumAttempts) await new Promise((resolve) => setTimeout(resolve, 2 ** attempt * 1_000))
  }
}

if (!installed()) throw new Error(`Electron binary is unavailable after ${maximumAttempts} attempts.`)
process.stdout.write('Electron binary is ready.\n')
