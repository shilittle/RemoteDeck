import { createHash, randomUUID } from 'node:crypto'
import { readdir, readFile, stat, writeFile } from 'node:fs/promises'
import { basename, join } from 'node:path'
import { redactSecrets, redactText } from '../logging/redaction'
import type { DiagnosticsExportResult } from '../../protocol/diagnostics'

interface DiagnosticsServiceOptions {
  appVersion: string
  logDirectory: string
  getSettings: () => Promise<unknown>
  getProfileSummary: () => Promise<unknown>
  getCapabilities: () => Promise<unknown>
  now?: () => Date
}

interface ZipEntry {
  name: string
  contents: Buffer
}

const maximumLogFiles = 5
const maximumLogBytes = 512 * 1024

export class DiagnosticsService {
  readonly #options: DiagnosticsServiceOptions

  constructor(options: DiagnosticsServiceOptions) {
    this.#options = options
  }

  async exportTo(destinationPath: string): Promise<DiagnosticsExportResult> {
    const generatedAt = (this.#options.now ?? (() => new Date()))()
    const entries: ZipEntry[] = [
      jsonEntry('manifest.json', {
        formatVersion: 1,
        application: 'RemoteDeck',
        appVersion: this.#options.appVersion,
        generatedAt: generatedAt.toISOString(),
        diagnosticId: randomUUID(),
        notice: 'Secrets and direct endpoint identifiers are intentionally excluded. Terminal and SFTP contents are never collected.'
      }),
      jsonEntry('settings.json', await this.#options.getSettings()),
      jsonEntry('profiles-summary.json', await this.#options.getProfileSummary()),
      jsonEntry('capabilities.json', await this.#options.getCapabilities()),
      {
        name: 'README.txt',
        contents: Buffer.from('RemoteDeck diagnostics bundle\r\n\r\nThis archive contains a redacted configuration summary, runtime capabilities, and bounded recent application logs. It does not contain passwords, passphrases, private keys, Codex tokens, terminal bytes, or remote file contents.\r\n', 'utf8')
      },
      ...await this.#recentLogs()
    ]
    for (const entry of entries) assertSanitized(entry.name, entry.contents.toString('utf8'))
    const archive = createStoredZip(entries, generatedAt)
    await writeFile(destinationPath, archive, { mode: 0o600 })
    return {
      exported: true,
      path: destinationPath,
      bytes: archive.byteLength,
      sha256: createHash('sha256').update(archive).digest('hex'),
      entries: entries.length
    }
  }

  async #recentLogs(): Promise<ZipEntry[]> {
    let names: string[]
    try {
      names = await readdir(this.#options.logDirectory)
    } catch (error) {
      if (error instanceof Error && 'code' in error && error.code === 'ENOENT') return []
      throw error
    }
    const candidates = await Promise.all(names.filter((name) => /^remotedeck-[\w.-]+\.log$/i.test(name)).map(async (name) => {
      const path = join(this.#options.logDirectory, name)
      return { name, path, info: await stat(path) }
    }))
    candidates.sort((left, right) => right.info.mtimeMs - left.info.mtimeMs)
    return Promise.all(candidates.slice(0, maximumLogFiles).map(async ({ name, path, info }) => {
      const raw = await readFile(path)
      const bounded = info.size > maximumLogBytes ? raw.subarray(raw.byteLength - maximumLogBytes) : raw
      return { name: `logs/${basename(name)}`, contents: Buffer.from(redactText(bounded.toString('utf8')), 'utf8') }
    }))
  }
}

function jsonEntry(name: string, value: unknown): ZipEntry {
  return { name, contents: Buffer.from(`${JSON.stringify(redactSecrets(value), null, 2)}\n`, 'utf8') }
}

function assertSanitized(name: string, text: string): void {
  if (redactText(text) !== text) throw new Error(`Diagnostics secret scan rejected ${name}`)
}

export function createStoredZip(entries: ZipEntry[], timestamp = new Date()): Buffer {
  const localParts: Buffer[] = []
  const centralParts: Buffer[] = []
  let offset = 0
  const { date, time } = dosDateTime(timestamp)
  for (const entry of entries) {
    const name = Buffer.from(entry.name.replaceAll('\\', '/'), 'utf8')
    const crc = crc32(entry.contents)
    const local = Buffer.alloc(30)
    local.writeUInt32LE(0x04034b50, 0)
    local.writeUInt16LE(20, 4)
    local.writeUInt16LE(0x0800, 6)
    local.writeUInt16LE(0, 8)
    local.writeUInt16LE(time, 10)
    local.writeUInt16LE(date, 12)
    local.writeUInt32LE(crc, 14)
    local.writeUInt32LE(entry.contents.byteLength, 18)
    local.writeUInt32LE(entry.contents.byteLength, 22)
    local.writeUInt16LE(name.byteLength, 26)
    local.writeUInt16LE(0, 28)
    localParts.push(local, name, entry.contents)

    const central = Buffer.alloc(46)
    central.writeUInt32LE(0x02014b50, 0)
    central.writeUInt16LE(20, 4)
    central.writeUInt16LE(20, 6)
    central.writeUInt16LE(0x0800, 8)
    central.writeUInt16LE(0, 10)
    central.writeUInt16LE(time, 12)
    central.writeUInt16LE(date, 14)
    central.writeUInt32LE(crc, 16)
    central.writeUInt32LE(entry.contents.byteLength, 20)
    central.writeUInt32LE(entry.contents.byteLength, 24)
    central.writeUInt16LE(name.byteLength, 28)
    central.writeUInt16LE(0, 30)
    central.writeUInt16LE(0, 32)
    central.writeUInt16LE(0, 34)
    central.writeUInt16LE(0, 36)
    central.writeUInt32LE(0, 38)
    central.writeUInt32LE(offset, 42)
    centralParts.push(central, name)
    offset += local.byteLength + name.byteLength + entry.contents.byteLength
  }
  const centralDirectory = Buffer.concat(centralParts)
  const end = Buffer.alloc(22)
  end.writeUInt32LE(0x06054b50, 0)
  end.writeUInt16LE(0, 4)
  end.writeUInt16LE(0, 6)
  end.writeUInt16LE(entries.length, 8)
  end.writeUInt16LE(entries.length, 10)
  end.writeUInt32LE(centralDirectory.byteLength, 12)
  end.writeUInt32LE(offset, 16)
  end.writeUInt16LE(0, 20)
  return Buffer.concat([...localParts, centralDirectory, end])
}

function crc32(contents: Buffer): number {
  let crc = 0xffffffff
  for (const byte of contents) {
    crc ^= byte
    for (let bit = 0; bit < 8; bit += 1) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0)
  }
  return (crc ^ 0xffffffff) >>> 0
}

function dosDateTime(value: Date): { date: number; time: number } {
  const year = Math.max(1980, value.getFullYear())
  return {
    date: ((year - 1980) << 9) | ((value.getMonth() + 1) << 5) | value.getDate(),
    time: (value.getHours() << 11) | (value.getMinutes() << 5) | Math.floor(value.getSeconds() / 2)
  }
}
