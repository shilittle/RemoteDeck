import { readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'

const directory = dirname(fileURLToPath(import.meta.url))
const appRoot = resolve(directory, '..')
const [configRaw, capabilityRaw, cargoRaw] = await Promise.all([
  readFile(resolve(appRoot, 'src-tauri/tauri.conf.json'), 'utf8'),
  readFile(resolve(appRoot, 'src-tauri/capabilities/main.json'), 'utf8'),
  readFile(resolve(appRoot, 'src-tauri/Cargo.toml'), 'utf8')
])
const config = JSON.parse(configRaw)
const capability = JSON.parse(capabilityRaw)

const failures = []
if (config.app?.withGlobalTauri !== true) failures.push('withGlobalTauri must be enabled for the narrow in-app invoke bridge')
if (JSON.stringify(config.bundle?.targets) !== JSON.stringify(['nsis'])) failures.push('NSIS must be the only bundle target')
if (config.bundle?.windows?.webviewInstallMode?.type !== 'downloadBootstrapper') failures.push('WebView2 must use downloadBootstrapper')
if (!String(config.app?.security?.csp ?? '').includes("default-src 'self'")) failures.push('strict CSP is missing')
if (JSON.stringify(capability.windows) !== JSON.stringify(['main'])) failures.push('capability must be scoped to the main window')
if (JSON.stringify(capability.permissions) !== JSON.stringify(['core:default'])) failures.push('capability must not grant broad plugins')
for (const forbidden of ['tauri-plugin-shell', 'tauri-plugin-fs', 'tauri-plugin-http']) {
  if (cargoRaw.includes(forbidden)) failures.push(`${forbidden} must not be installed`)
}
if (!cargoRaw.includes('portable-pty')) failures.push('portable-pty is required for ConPTY')
if (config.bundle?.windows?.webviewInstallMode?.type === 'offlineInstaller') failures.push('offline WebView2 would recreate the 100+ MB installer')

if (failures.length) {
  for (const failure of failures) console.error(`- ${failure}`)
  process.exit(1)
}
console.log('Tauri command/capability/package boundary verified.')
