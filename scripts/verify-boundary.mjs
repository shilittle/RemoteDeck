import { access, readFile, readdir } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('..', import.meta.url))
const read = path => readFile(resolve(root, path), 'utf8')
const rustFiles = async directory => {
  const entries = await readdir(resolve(root, directory), { withFileTypes: true })
  const files = []
  for (const entry of entries) {
    const relative = `${directory}/${entry.name}`
    if (entry.isDirectory()) files.push(...await rustFiles(relative))
    else if (entry.isFile() && entry.name.endsWith('.rs')) files.push(relative)
  }
  return files
}
const [rootRaw, webRaw, core, server, api, transport, webApi, lock] = await Promise.all([
  read('package.json'), read('apps/web/package.json'), read('crates/remotedeck-core/Cargo.toml'),
  read('apps/server/Cargo.toml'), read('apps/server/src/api.rs'), read('apps/server/src/transport.rs'),
  read('apps/web/src/api.ts'), read('Cargo.lock')
])
const failures = []
const serverRustFiles = [...await rustFiles('apps/server/src'), ...await rustFiles('apps/server/tests')]
const serverRustSources = await Promise.all(serverRustFiles.map(async path => [path, await read(path)]))
const rootPackage = JSON.parse(rootRaw)
const webPackage = JSON.parse(webRaw)
for (const [label, version] of [['web', webPackage.version], ['core', core.match(/^version\s*=\s*"([^"]+)"/m)?.[1]], ['server', server.match(/^version\s*=\s*"([^"]+)"/m)?.[1]]]) {
  if (version !== rootPackage.version) failures.push(`${label} version differs from root version`)
}
for (const name of ['electron', 'electron-builder', '@tauri-apps/api', 'ssh2', 'node-pty']) {
  if (name in { ...webPackage.dependencies, ...webPackage.devDependencies }) failures.push(`forbidden browser dependency: ${name}`)
}
for (const name of ['tauri', 'tauri-build', 'wry', 'webview2-com', 'webkit2gtk']) {
  if (new RegExp(`^name = "${name}"$`, 'm').test(lock)) failures.push(`desktop web runtime remains in Cargo.lock: ${name}`)
}
if (!core.includes('portable-pty')) failures.push('the real PTY implementation is missing')
if (!server.includes('axum') || !server.includes('remotedeck-core')) failures.push('server must use the dedicated native core and HTTP transport')
for (const [path, source] of serverRustSources) {
  if (/\bCommand::new\s*\(/.test(source)) failures.push(`${path} uses raw Command::new; use the core hidden-process helper`)
  if (/\.creation_flags\s*\(/.test(source)) failures.push(`${path} sets process creation flags outside the core hidden-process helper`)
}
const block = api.match(/const COMMANDS: &\[&str\] = &\[([\s\S]*?)\];/)?.[1] ?? ''
const registered = new Set([...block.matchAll(/"([a-z_]+)"/g)].map(match => match[1]))
for (const match of transport.matchAll(/\.route\("\/api\/v1\/([a-z_]+)"/g)) registered.add(match[1])
const used = new Set([...webApi.matchAll(/\bpost(?:<[^>]+>)?\(\s*['"]([a-z_]+)['"]/g)].map(match => match[1]))
for (const name of used) if (!registered.has(name)) failures.push(`unregistered browser operation: ${name}`)
for (const name of registered) if (!['events', 'operations'].includes(name) && !used.has(name)) failures.push(`operation without a typed browser client: ${name}`)
if (used.size < 50) failures.push('the complete business operation surface was not retained')
if (webApi.includes('@tauri-apps/') || webApi.includes('isTauri')) failures.push('browser still depends on Tauri')
try {
  await access(resolve(root, 'apps/desktop/src-tauri/Cargo.toml'))
  failures.push('old desktop production source still exists')
} catch (error) { if (error.code !== 'ENOENT') throw error }
if (failures.length) throw new Error(failures.join('\n'))
console.log(`Browser/native boundary verified: ${used.size} explicit operations, no desktop web runtime.`)
