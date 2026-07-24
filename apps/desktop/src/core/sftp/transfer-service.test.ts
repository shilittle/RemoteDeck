import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, posix } from 'node:path'
import { Readable, Writable } from 'node:stream'
import type { FileEntryWithStats, Stats } from 'ssh2'
import { afterEach, describe, expect, it } from 'vitest'
import { SftpService } from './sftp-service'
import { TransferService } from './transfer-service'

type Node = { type: 'file' | 'directory' | 'symlink'; data: Buffer; mode: number; mtime: number }
type ValueCallback<T> = (error: Error | undefined, value: T) => void
type VoidCallback = (error?: Error) => void

class MemorySftp {
  readonly nodes = new Map<string, Node>([['/', directoryNode()], ['/remote', directoryNode()]])
  end(): void { /* shared in-memory fixture remains available */ }
  realpath(path: string, callback: ValueCallback<string>): void { const resolved = path === '~' ? '/remote' : normalize(path); if (this.nodes.has(resolved)) callback(undefined, resolved); else callback(missing(), '') }
  lstat(path: string, callback: ValueCallback<Stats>): void { const node = this.nodes.get(normalize(path)); if (node) callback(undefined, stats(node)); else callback(missing(), stats(directoryNode())) }
  readdir(path: string, callback: ValueCallback<FileEntryWithStats[]>): void {
    const parent = normalize(path)
    if (this.nodes.get(parent)?.type !== 'directory') { callback(missing(), []); return }
    const entries: FileEntryWithStats[] = []
    for (const [entryPath, node] of this.nodes) if (entryPath !== parent && posix.dirname(entryPath) === parent) entries.push({ filename: posix.basename(entryPath), longname: posix.basename(entryPath), attrs: stats(node) })
    callback(undefined, entries)
  }
  mkdir(path: string, _attributes: unknown, callback: VoidCallback): void { const target = normalize(path); if (this.nodes.has(target)) callback(exists()); else { this.nodes.set(target, directoryNode()); callback() } }
  rmdir(path: string, callback: VoidCallback): void { const target = normalize(path); if ([...this.nodes.keys()].some((value) => value !== target && posix.dirname(value) === target)) callback(new Error('Directory not empty')); else { this.nodes.delete(target); callback() } }
  unlink(path: string, callback: VoidCallback): void { if (this.nodes.delete(normalize(path))) callback(); else callback(missing()) }
  chmod(path: string, mode: number, callback: VoidCallback): void { const node = this.nodes.get(normalize(path)); if (!node) callback(missing()); else { node.mode = mode; callback() } }
  rename(source: string, target: string, callback: VoidCallback): void {
    source = normalize(source); target = normalize(target)
    const node = this.nodes.get(source)
    if (!node) { callback(missing()); return }
    const moves = [...this.nodes].filter(([path]) => path === source || path.startsWith(`${source}/`))
    for (const [path] of moves) this.nodes.delete(path)
    for (const [path, value] of moves) this.nodes.set(`${target}${path.slice(source.length)}`, value)
    callback()
  }
  open(path: string, _flags: string, _mode: number, callback: ValueCallback<Buffer>): void { path = normalize(path); if (this.nodes.has(path)) callback(exists(), Buffer.alloc(0)); else { this.nodes.set(path, fileNode(Buffer.alloc(0))); callback(undefined, Buffer.from(path)) } }
  close(_handle: Buffer, callback: VoidCallback): void { callback() }
  createWriteStream(path: string): Writable {
    path = normalize(path)
    const chunks: Buffer[] = []
    return new Writable({ write(chunk: Buffer, _encoding, callback) { chunks.push(Buffer.from(chunk)); callback() }, final: (callback) => { this.nodes.set(path, fileNode(Buffer.concat(chunks))); callback() } })
  }
  createReadStream(path: string): Readable { const node = this.nodes.get(normalize(path)); if (!node || node.type !== 'file') return new Readable({ read() { this.destroy(missing()) } }); return Readable.from(node.data) }
}

const directories: string[] = []
afterEach(async () => Promise.all(directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true }))))

describe('SFTP CRUD and transfer engine', () => {
  it('handles Unicode trees, atomically uploads/downloads, renames conflicts, and cleans cancellation files', async () => {
    const memory = new MemorySftp()
    const client = { sftp: (callback: ValueCallback<MemorySftp>) => callback(undefined, memory) }
    const service = new SftpService({ getOnlineClient: () => client } as never)
    const transfers = new TransferService(service)
    const remoteRoot = await service.create('019f92f0-b87c-7c74-a668-54d5b18fb487', '/remote', '测试 空格', 'directory')
    const empty = await service.create('019f92f0-b87c-7c74-a668-54d5b18fb487', remoteRoot, 'empty.txt', 'file')
    expect(await service.rename('019f92f0-b87c-7c74-a668-54d5b18fb487', empty, 'renamed.txt')).toBe('/remote/测试 空格/renamed.txt')

    const localRoot = await mkdtemp(join(tmpdir(), 'remotedeck-transfer-'))
    directories.push(localRoot)
    const source = join(localRoot, '上传 目录')
    await mkdir(join(source, 'nested'), { recursive: true })
    await writeFile(join(source, 'nested', '内容.txt'), '你好 SFTP', 'utf8')
    await writeFile(join(source, 'empty.txt'), '')
    const upload = transfers.startUpload({ hostId: '019f92f0-b87c-7c74-a668-54d5b18fb487', sources: [source], remoteDirectory: remoteRoot, conflictPolicy: 'overwrite' })[0]
    if (!upload) throw new Error('Missing upload job')
    expect((await waitForJob(transfers, upload.id, 'completed')).bytesTransferred).toBe(Buffer.byteLength('你好 SFTP'))
    expect((await service.list(upload.hostId, `${remoteRoot}/上传 目录/nested`, true)).entries[0]).toMatchObject({ name: '内容.txt', type: 'file' })

    const conflict = transfers.startUpload({ hostId: upload.hostId, sources: [source], remoteDirectory: remoteRoot, conflictPolicy: 'rename' })[0]
    if (!conflict) throw new Error('Missing conflict job')
    expect((await waitForJob(transfers, conflict.id, 'completed')).destination).toContain('上传 目录 (1)')

    const downloads = join(localRoot, 'downloads')
    await mkdir(downloads)
    const download = transfers.startDownload({ hostId: upload.hostId, sources: [`${remoteRoot}/上传 目录`], localDirectory: downloads, conflictPolicy: 'overwrite' })[0]
    if (!download) throw new Error('Missing download job')
    await waitForJob(transfers, download.id, 'completed')
    expect(await readFile(join(downloads, '上传 目录', 'nested', '内容.txt'), 'utf8')).toBe('你好 SFTP')

    const cancellationSource = join(localRoot, 'cancel.bin')
    await writeFile(cancellationSource, Buffer.alloc(2 * 1024 * 1024, 7))
    const cancellation = transfers.startUpload({ hostId: upload.hostId, sources: [cancellationSource], remoteDirectory: remoteRoot, conflictPolicy: 'overwrite' })[0]
    if (!cancellation) throw new Error('Missing cancellation job')
    transfers.cancel(cancellation.id)
    await waitForJob(transfers, cancellation.id, 'cancelled')
    expect([...memory.nodes.keys()].some((path) => path.includes(`remotedeck-${cancellation.id}`))).toBe(false)

    await service.delete(upload.hostId, [remoteRoot])
    expect((await service.list(upload.hostId, '/remote', true)).entries).toHaveLength(0)
  })
})

async function waitForJob(transfers: TransferService, id: string, state: 'completed' | 'cancelled'): Promise<ReturnType<TransferService['list']>[number]> {
  const deadline = Date.now() + 5000
  for (;;) {
    const job = transfers.list().find((item) => item.id === id)
    if (job?.state === state) return job
    if (job?.state === 'failed') throw new Error(job.error)
    if (Date.now() > deadline) throw new Error(`Timed out: ${JSON.stringify(job)}`)
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}

function normalize(path: string): string { const value = posix.normalize(path); return value.length > 1 ? value.replace(/\/$/, '') : value }
function directoryNode(): Node { return { type: 'directory', data: Buffer.alloc(0), mode: 0o755, mtime: Math.floor(Date.now() / 1000) } }
function fileNode(data: Buffer): Node { return { type: 'file', data, mode: 0o644, mtime: Math.floor(Date.now() / 1000) } }
function stats(node: Node): Stats { return { mode: node.mode, uid: 1000, gid: 1000, size: node.data.length, atime: node.mtime, mtime: node.mtime, isDirectory: () => node.type === 'directory', isFile: () => node.type === 'file', isBlockDevice: () => false, isCharacterDevice: () => false, isSymbolicLink: () => node.type === 'symlink', isFIFO: () => false, isSocket: () => false } }
function missing(): Error { return Object.assign(new Error('No such file'), { code: 2 }) }
function exists(): Error { return Object.assign(new Error('Already exists'), { code: 4 }) }
