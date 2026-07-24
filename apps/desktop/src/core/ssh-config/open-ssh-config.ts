import { createHash, randomUUID } from 'node:crypto'
import { copyFile, mkdir, open, readFile, rename, rm } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { userInfo } from 'node:os'
import type { HostCreateRequest, HostImportResult, HostListItem } from '../../protocol/ssh'
import type { ResolvedHostProfile } from '../hosts/profile-repository'
import type { ProfileRepository } from '../hosts/profile-repository'

export interface ParsedOpenSshHost {
  alias: string
  hostname: string
  username: string
  port: number
  identityFile?: string
  identitiesOnly?: boolean
  proxyJump?: string
  serverAliveInterval?: number
  serverAliveCountMax?: number
  tcpKeepAlive?: boolean
  connectTimeout?: number
  compression?: boolean
  localForwards: string[]
  remoteForwards: string[]
  unsupported: string[]
}

export function parseOpenSshConfig(text: string): ParsedOpenSshHost[] {
  const hosts: ParsedOpenSshHost[] = []
  let current: ParsedOpenSshHost[] = []
  for (const rawLine of text.replace(/\r\n?/g, '\n').split('\n')) {
    const content = stripComment(rawLine).trim()
    if (!content) continue
    const match = /^(\S+)\s*[=\s]\s*(.*)$/.exec(content)
    if (!match?.[1]) continue
    const key = match[1].toLowerCase()
    const value = unquote(match[2] ?? '')
    if (key === 'host') {
      current = splitWords(value)
        .filter((alias) => alias && !alias.includes('*') && !alias.includes('?') && !alias.startsWith('!'))
        .map((alias) => ({ alias, hostname: alias, username: '', port: 22, localForwards: [], remoteForwards: [], unsupported: [] }))
      hosts.push(...current)
      continue
    }
    if (current.length === 0) continue
    for (const host of current) applyDirective(host, key, value, rawLine)
  }
  return hosts
}

export function renderManagedConfig(profiles: ResolvedHostProfile[]): string {
  const lines = ['# Managed by RemoteDeck. Manual edits may be replaced.', '']
  for (const { host, auth } of [...profiles].sort((a, b) => a.host.alias.localeCompare(b.host.alias))) {
    lines.push(`Host ${host.alias}`, `  HostName ${quote(host.hostname)}`, `  User ${quote(host.username)}`, `  Port ${String(host.port)}`)
    if (auth.method === 'private_key' && auth.identityFile) lines.push(`  IdentityFile ${quote(auth.identityFile)}`)
    if (host.advanced.identitiesOnly) lines.push('  IdentitiesOnly yes')
    if (host.jumpHostId) {
      const jump = profiles.find((item) => item.host.id === host.jumpHostId)
      if (jump) lines.push(`  ProxyJump ${jump.host.alias}`)
    }
    lines.push(
      `  ServerAliveInterval ${String(host.advanced.serverAliveIntervalSeconds)}`,
      `  ServerAliveCountMax ${String(host.advanced.serverAliveCountMax)}`,
      `  TCPKeepAlive ${host.advanced.tcpKeepAlive ? 'yes' : 'no'}`,
      `  ConnectTimeout ${String(host.advanced.connectTimeoutSeconds)}`,
      `  Compression ${host.advanced.compression ? 'yes' : 'no'}`,
      ''
    )
  }
  return `${lines.join('\n').trimEnd()}\n`
}

export function ensureManagedInclude(source: string, managedPath: string): string {
  const portablePath = managedPath.replaceAll('\\', '/').replace(/\/$/, '')
  const normalized = normalizeIncludePath(portablePath)
  const lines = source.replace(/\r\n?/g, '\n').split('\n')
  const exists = lines.some((line) => {
    const match = /^\s*Include\s+(.+?)\s*(?:#.*)?$/i.exec(line)
    return match?.[1] ? normalizeIncludePath(unquote(match[1])) === normalized : false
  })
  if (exists) return source.endsWith('\n') ? source : `${source}\n`
  const prefix = source.length > 0 && !source.endsWith('\n') ? `${source}\n` : source
  return `${prefix}Include ${quote(portablePath)}\n`
}

export class OpenSshConfigManager {
  readonly #repository: ProfileRepository

  constructor(repository: ProfileRepository) {
    this.#repository = repository
  }

  async importFile(configPath: string, stateFor: (hostId: string) => HostListItem['state']): Promise<HostImportResult> {
    const text = await readFile(configPath, 'utf8')
    const sourceHash = createHash('sha256').update(text).digest('hex')
    if (await this.#repository.hasImport(sourceHash)) return { imported: [], skippedAliases: [], unsupported: [], duplicate: true, sourceHash }
    const parsed = parseOpenSshConfig(text)
    const existing = await this.#repository.list()
    const aliases = new Set(existing.map((item) => item.host.alias.toLowerCase()))
    const imported: HostListItem[] = []
    const createdByAlias = new Map<string, HostListItem>()
    const skippedAliases: string[] = []
    const unsupported: Array<{ alias: string; lines: string[] }> = []
    for (const entry of parsed) {
      if (aliases.has(entry.alias.toLowerCase())) {
        skippedAliases.push(entry.alias)
        continue
      }
      const request: HostCreateRequest = {
        alias: entry.alias,
        hostname: entry.hostname,
        port: entry.port,
        username: entry.username || userInfo().username,
        groups: [],
        auth: entry.identityFile
          ? { name: `${entry.alias} 私钥`, method: 'private_key', identityFile: entry.identityFile }
          : { name: `${entry.alias} SSH agent`, method: 'agent', agent: 'windows_openssh' },
        advanced: {
          connectTimeoutSeconds: entry.connectTimeout ?? 15,
          serverAliveIntervalSeconds: entry.serverAliveInterval ?? 30,
          serverAliveCountMax: entry.serverAliveCountMax ?? 3,
          tcpKeepAlive: entry.tcpKeepAlive ?? true,
          compression: entry.compression ?? false,
          identitiesOnly: entry.identitiesOnly ?? false
        }
      }
      const created = await this.#repository.create(request)
      aliases.add(entry.alias.toLowerCase())
      const listItem = { ...created, state: stateFor(created.host.id) }
      imported.push(listItem)
      createdByAlias.set(entry.alias.toLowerCase(), listItem)
      if (entry.unsupported.length > 0) unsupported.push({ alias: entry.alias, lines: entry.unsupported })
    }
    const allProfiles = await this.#repository.list()
    const byAlias = new Map(allProfiles.map((item) => [item.host.alias.toLowerCase(), item]))
    for (const entry of parsed) {
      const created = createdByAlias.get(entry.alias.toLowerCase())
      if (!created) continue
      if (entry.proxyJump) {
        const jumpAlias = entry.proxyJump.split(',')[0]?.trim().toLowerCase()
        const jump = jumpAlias ? byAlias.get(jumpAlias) : undefined
        if (jump) await this.#repository.update({ id: created.host.id, patch: { jumpHostId: jump.host.id } })
        else unsupported.push({ alias: entry.alias, lines: [`ProxyJump ${entry.proxyJump}`] })
      }
      for (const [index, value] of entry.localForwards.entries()) await this.#importForward(created.host.id, created.host.alias, 'local', value, index)
      for (const [index, value] of entry.remoteForwards.entries()) await this.#importForward(created.host.id, created.host.alias, 'remote', value, index)
    }
    await this.#repository.recordImport(sourceHash, unsupported)
    const finalProfiles = new Map((await this.#repository.list()).map((item) => [item.host.id, item]))
    for (let index = 0; index < imported.length; index += 1) {
      const current = imported[index]
      if (!current) continue
      const final = finalProfiles.get(current.host.id)
      if (final) imported[index] = { ...final, state: stateFor(final.host.id) }
    }
    return { imported, skippedAliases, unsupported, duplicate: false, sourceHash }
  }

  async #importForward(hostId: string, alias: string, direction: 'local' | 'remote', value: string, index: number): Promise<void> {
    const parsed = parseForward(value)
    if (!parsed) return
    await this.#repository.addTunnel({ hostId, name: `${alias} ${direction === 'local' ? 'LocalForward' : 'RemoteForward'} ${String(index + 1)}`, direction, bindAddress: parsed.bindAddress, sourcePort: parsed.sourcePort, targetHost: parsed.targetHost, targetPort: parsed.targetPort, autoStart: false })
  }

  async writeManaged(userConfigPath: string): Promise<{ managedPath: string; backupPath?: string }> {
    const managedPath = join(dirname(userConfigPath), 'remotedeck.conf')
    const profiles = await this.#repository.list()
    const managed = renderManagedConfig(profiles)
    const parsed = parseOpenSshConfig(managed)
    if (parsed.length !== profiles.length) throw new Error('Generated OpenSSH config failed parser validation')
    await atomicTextWrite(managedPath, managed)
    let existing = ''
    try { existing = await readFile(userConfigPath, 'utf8') } catch (error) { if (!isMissing(error)) throw error }
    const next = ensureManagedInclude(existing, managedPath)
    if (next === existing) return { managedPath }
    const backupPath = existing ? `${userConfigPath}.remotedeck-${new Date().toISOString().replace(/[:.]/g, '-')}.bak` : undefined
    if (backupPath) await copyFile(userConfigPath, backupPath)
    try {
      await atomicTextWrite(userConfigPath, next)
      if (!ensureManagedInclude(next, managedPath).includes('Include')) throw new Error('Include validation failed')
    } catch (error) {
      if (backupPath) await copyFile(backupPath, userConfigPath)
      throw error
    }
    return backupPath ? { managedPath, backupPath } : { managedPath }
  }
}

async function atomicTextWrite(filePath: string, text: string): Promise<void> {
  await mkdir(dirname(filePath), { recursive: true })
  const temporary = `${filePath}.${String(process.pid)}.${randomUUID()}.tmp`
  try {
    const handle = await open(temporary, 'wx', 0o600)
    try { await handle.writeFile(text, 'utf8'); await handle.sync() } finally { await handle.close() }
    await rename(temporary, filePath)
  } finally {
    await rm(temporary, { force: true })
  }
}

function applyDirective(host: ParsedOpenSshHost, key: string, value: string, rawLine: string): void {
  switch (key) {
    case 'hostname': host.hostname = value; break
    case 'user': host.username = value; break
    case 'port': host.port = parseInteger(value, 22); break
    case 'identityfile': if (!host.identityFile) host.identityFile = value; else host.unsupported.push(rawLine); break
    case 'identitiesonly': host.identitiesOnly = parseBoolean(value); break
    case 'proxyjump': host.proxyJump = value; break
    case 'serveraliveinterval': host.serverAliveInterval = parseInteger(value, 30); break
    case 'serveralivecountmax': host.serverAliveCountMax = parseInteger(value, 3); break
    case 'tcpkeepalive': host.tcpKeepAlive = parseBoolean(value); break
    case 'connecttimeout': host.connectTimeout = parseInteger(value, 15); break
    case 'compression': host.compression = parseBoolean(value); break
    case 'localforward': host.localForwards.push(value); break
    case 'remoteforward': host.remoteForwards.push(value); break
    default: host.unsupported.push(rawLine)
  }
}

function stripComment(line: string): string {
  let quoteCharacter = ''
  for (let index = 0; index < line.length; index += 1) {
    const character = line[index]
    if ((character === '"' || character === "'") && line[index - 1] !== '\\') quoteCharacter = quoteCharacter === character ? '' : quoteCharacter || character
    if (character === '#' && !quoteCharacter) return line.slice(0, index)
  }
  return line
}
function splitWords(value: string): string[] { return value.match(/"[^"]*"|'[^']*'|\S+/g)?.map(unquote) ?? [] }
function unquote(value: string): string { const trimmed = value.trim(); return ((trimmed.startsWith('"') && trimmed.endsWith('"')) || (trimmed.startsWith("'") && trimmed.endsWith("'"))) ? trimmed.slice(1, -1) : trimmed }
function quote(value: string): string { return /[\s#"]/.test(value) ? `"${value.replaceAll('"', '\\"')}"` : value }
function parseBoolean(value: string): boolean { return /^(yes|true|on|1)$/i.test(value) }
function parseInteger(value: string, fallback: number): number { const parsed = Number.parseInt(value, 10); return Number.isFinite(parsed) ? parsed : fallback }
function normalizeIncludePath(value: string): string { return value.replaceAll('\\', '/').replace(/\/$/, '').toLowerCase() }
function isMissing(error: unknown): boolean { return error instanceof Error && 'code' in error && error.code === 'ENOENT' }
function parseForward(value: string): { bindAddress: string; sourcePort: number; targetHost: string; targetPort: number } | undefined {
  const fields = splitWords(value)
  if (fields.length < 2) return undefined
  const source = fields[0]
  const target = fields[1]
  if (!source || !target) return undefined
  const sourceParts = source.split(':')
  const sourcePort = Number(sourceParts.at(-1))
  const targetParts = target.split(':')
  const targetPort = Number(targetParts.at(-1))
  const targetHost = targetParts.slice(0, -1).join(':')
  if (!Number.isInteger(sourcePort) || !Number.isInteger(targetPort) || !targetHost) return undefined
  return { bindAddress: sourceParts.length > 1 ? sourceParts.slice(0, -1).join(':') : '127.0.0.1', sourcePort, targetHost, targetPort }
}
