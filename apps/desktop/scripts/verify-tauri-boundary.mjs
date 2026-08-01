import { access, readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'

const directory = dirname(fileURLToPath(import.meta.url))
const appRoot = resolve(directory, '..')
const repositoryRoot = resolve(appRoot, '../..')
const [configRaw, capabilityRaw, cargoRaw, apiRaw, libRaw, appPackageRaw, rootPackageRaw] = await Promise.all([
  readFile(resolve(appRoot, 'src-tauri/tauri.conf.json'), 'utf8'),
  readFile(resolve(appRoot, 'src-tauri/capabilities/main.json'), 'utf8'),
  readFile(resolve(appRoot, 'src-tauri/Cargo.toml'), 'utf8'),
  readFile(resolve(appRoot, 'tauri-ui/src/api.ts'), 'utf8'),
  readFile(resolve(appRoot, 'src-tauri/src/lib.rs'), 'utf8'),
  readFile(resolve(appRoot, 'package.json'), 'utf8'),
  readFile(resolve(repositoryRoot, 'package.json'), 'utf8')
])
const config = JSON.parse(configRaw)
const capability = JSON.parse(capabilityRaw)
const appPackage = JSON.parse(appPackageRaw)
const rootPackage = JSON.parse(rootPackageRaw)

const failures = []
const removedLegacyPaths = [
  'src',
  'tests/e2e',
  'tests/integration',
  'electron.vite.config.ts',
  'electron-builder.yml',
  'playwright.config.ts',
  'scripts/after-pack.cjs',
  'scripts/ensure-electron.mjs',
  'vitest.config.ts',
  'vitest.integration.config.ts',
  'vitest.openssh.config.ts',
  'tsconfig.node.json',
  'tsconfig.web.json'
]
for (const legacyPath of removedLegacyPaths) {
  try {
    await access(resolve(appRoot, legacyPath))
    failures.push(`removed Electron-era path returned: apps/desktop/${legacyPath}`)
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }
}
for (const legacyPath of ['tests/fixtures/openssh', 'tests/manual/m9-packaged-smoke.ps1', 'tests/manual/m9-clean-install.ps1']) {
  try {
    await access(resolve(repositoryRoot, legacyPath))
    failures.push(`removed Electron-era path returned: ${legacyPath}`)
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }
}
if (config.app?.withGlobalTauri === true) failures.push('the global Tauri bridge must remain disabled; use bundled typed API imports')
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

const cargoVersion = cargoRaw.match(/^version\s*=\s*"([^"]+)"/m)?.[1]
for (const [source, version] of [['root package', rootPackage.version], ['desktop package', appPackage.version], ['Cargo package', cargoVersion]]) {
  if (version !== config.version) failures.push(`${source} version ${String(version)} does not match Tauri ${String(config.version)}`)
}

const packageEntries = { ...appPackage.dependencies, ...appPackage.devDependencies }
for (const forbidden of ['electron', 'electron-builder', 'ssh2', 'node-pty']) {
  if (forbidden in packageEntries) failures.push(`${forbidden} must not be a desktop dependency`)
}
for (const script of Object.values(appPackage.scripts ?? {})) {
  if (/\belectron(?:-builder)?\b/i.test(String(script))) failures.push(`legacy Electron command remains in package scripts: ${String(script)}`)
}

const invokedCommands = new Set([...apiRaw.matchAll(/\binvoke(?:<[^>]+>)?\(\s*['"]([^'"]+)['"]/g)].map((match) => match[1]))
const handlerBlock = libRaw.match(/tauri::generate_handler!\[([\s\S]*?)\]\)/)?.[1] ?? ''
const registeredCommands = new Set(handlerBlock.split(',').map((name) => name.trim()).filter(Boolean))
for (const command of invokedCommands) {
  if (!registeredCommands.has(command)) failures.push(`frontend invokes unregistered command: ${command}`)
}
for (const command of registeredCommands) {
  if (!invokedCommands.has(command)) failures.push(`backend exposes command without a typed frontend method: ${command}`)
}
if (invokedCommands.size < 50) failures.push(`expected the complete command surface, found only ${invokedCommands.size} commands`)

if (failures.length) {
  for (const failure of failures) console.error(`- ${failure}`)
  process.exit(1)
}
console.log('Tauri command/capability/package boundary verified.')
