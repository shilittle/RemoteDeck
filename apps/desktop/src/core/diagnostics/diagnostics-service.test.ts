import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { DiagnosticsService } from './diagnostics-service'

const temporaryDirectories: string[] = []

afterEach(async () => {
  await Promise.all(temporaryDirectories.splice(0).map((path) => rm(path, { recursive: true, force: true })))
})

describe('DiagnosticsService', () => {
  it('exports a valid bounded ZIP without persisted or logged secrets', async () => {
    const root = await mkdtemp(join(tmpdir(), 'remotedeck-diagnostics-'))
    temporaryDirectories.push(root)
    const logs = join(root, 'logs')
    await mkdir(logs)
    await writeFile(join(logs, 'remotedeck-test.log'), 'safe line\npassword=hunter2\nAuthorization: Bearer sk-proj-very-secret-token\n')
    const destination = join(root, 'bundle.zip')
    const service = new DiagnosticsService({
      appVersion: '1.0.0',
      logDirectory: logs,
      getSettings: () => Promise.resolve({ logLevel: 'info', password: 'not-persisted' }),
      getProfileSummary: () => Promise.resolve({ hosts: 1, privateKey: 'never-export-this' }),
      getCapabilities: () => Promise.resolve({ electron: '43.2.0' }),
      now: () => new Date('2026-07-24T10:00:00.000Z')
    })
    const result = await service.exportTo(destination)
    const archive = await readFile(destination)
    const extracted = readStoredZip(archive)
    const printable = [...extracted.values()].join('\n')
    expect(result).toMatchObject({ exported: true, path: destination, entries: 6 })
    expect(result.sha256).toMatch(/^[a-f0-9]{64}$/)
    expect(archive.readUInt32LE(0)).toBe(0x04034b50)
    expect(archive.readUInt32LE(archive.byteLength - 22)).toBe(0x06054b50)
    expect([...extracted.keys()]).toContain('profiles-summary.json')
    expect(printable).toContain('[Redacted]')
    expect(printable).not.toContain('hunter2')
    expect(printable).not.toContain('very-secret-token')
    expect(printable).not.toContain('never-export-this')
  })
})

function readStoredZip(archive: Buffer): Map<string, string> {
  const endOffset = archive.byteLength - 22
  expect(archive.readUInt32LE(endOffset)).toBe(0x06054b50)
  const count = archive.readUInt16LE(endOffset + 10)
  let centralOffset = archive.readUInt32LE(endOffset + 16)
  const entries = new Map<string, string>()
  for (let index = 0; index < count; index += 1) {
    expect(archive.readUInt32LE(centralOffset)).toBe(0x02014b50)
    const compressedSize = archive.readUInt32LE(centralOffset + 20)
    const nameLength = archive.readUInt16LE(centralOffset + 28)
    const extraLength = archive.readUInt16LE(centralOffset + 30)
    const commentLength = archive.readUInt16LE(centralOffset + 32)
    const localOffset = archive.readUInt32LE(centralOffset + 42)
    const name = archive.subarray(centralOffset + 46, centralOffset + 46 + nameLength).toString('utf8')
    expect(archive.readUInt32LE(localOffset)).toBe(0x04034b50)
    const localNameLength = archive.readUInt16LE(localOffset + 26)
    const localExtraLength = archive.readUInt16LE(localOffset + 28)
    const dataOffset = localOffset + 30 + localNameLength + localExtraLength
    entries.set(name, archive.subarray(dataOffset, dataOffset + compressedSize).toString('utf8'))
    centralOffset += 46 + nameLength + extraLength + commentLength
  }
  return entries
}
