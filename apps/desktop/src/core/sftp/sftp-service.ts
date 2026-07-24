import { posix } from 'node:path'
import type { FileEntryWithStats, SFTPWrapper, Stats } from 'ssh2'
import type { SftpEntry, SftpListResult } from '../../protocol/sftp'
import type { SshConnectionManager } from '../ssh/connection-manager'

export class SftpService {
  readonly #connections: SshConnectionManager

  constructor(connections: SshConnectionManager) { this.#connections = connections }

  async list(hostId: string, path: string, showHidden: boolean): Promise<SftpListResult> {
    return await this.withSftp(hostId, async (sftp) => {
      const resolved = await realpath(sftp, path)
      const entries = (await readdir(sftp, resolved))
        .filter((entry) => entry.filename !== '.' && entry.filename !== '..' && (showHidden || !entry.filename.startsWith('.')))
        .map((entry) => toEntry(resolved, entry))
        .sort((left, right) => left.type === right.type ? left.name.localeCompare(right.name) : left.type === 'directory' ? -1 : right.type === 'directory' ? 1 : left.name.localeCompare(right.name))
      return { path: resolved, entries }
    })
  }

  async create(hostId: string, parentPath: string, name: string, type: 'file' | 'directory'): Promise<string> {
    validateName(name)
    return await this.withSftp(hostId, async (sftp) => {
      const parent = await realpath(sftp, parentPath)
      const target = posix.join(parent, name)
      await assertMissing(sftp, target)
      if (type === 'directory') await mkdir(sftp, target, 0o755)
      else await createEmptyFile(sftp, target)
      return target
    })
  }

  async rename(hostId: string, path: string, newName: string): Promise<string> {
    validateName(newName)
    return await this.withSftp(hostId, async (sftp) => {
      const resolved = await realpath(sftp, path)
      const target = posix.join(posix.dirname(resolved), newName)
      await assertMissing(sftp, target)
      await rename(sftp, resolved, target)
      return target
    })
  }

  async delete(hostId: string, paths: string[]): Promise<string[]> {
    return await this.withSftp(hostId, async (sftp) => {
      const affected: string[] = []
      for (const path of paths) {
        const resolved = await realpath(sftp, path)
        if (resolved === '/') throw new Error('Remote root cannot be deleted')
        await removeRecursive(sftp, resolved)
        affected.push(resolved)
      }
      return affected
    })
  }

  async withSftp<T>(hostId: string, action: (sftp: SFTPWrapper) => Promise<T>): Promise<T> {
    const sftp = await getSftp(this.#connections.getOnlineClient(hostId))
    try { return await action(sftp) } finally { sftp.end() }
  }
}

function toEntry(parent: string, entry: FileEntryWithStats): SftpEntry {
  const attrs = entry.attrs
  const type = attrs.isDirectory() ? 'directory' : attrs.isFile() ? 'file' : attrs.isSymbolicLink() ? 'symlink' : 'other'
  return { name: entry.filename, path: posix.join(parent, entry.filename), type, size: attrs.size, mode: attrs.mode, modifiedAt: new Date(attrs.mtime * 1000).toISOString(), uid: attrs.uid, gid: attrs.gid }
}

function validateName(name: string): void {
  if (name === '.' || name === '..' || name.includes('/') || name.includes('\0')) throw new Error('Remote name must be one path segment')
}

function getSftp(client: ReturnType<SshConnectionManager['getOnlineClient']>): Promise<SFTPWrapper> { return new Promise((resolve, reject) => client.sftp((error, sftp) => error ? reject(error) : resolve(sftp))) }
function realpath(sftp: SFTPWrapper, path: string): Promise<string> { return new Promise((resolve, reject) => sftp.realpath(path, (error, resolved) => error ? reject(error) : resolve(resolved))) }
function readdir(sftp: SFTPWrapper, path: string): Promise<FileEntryWithStats[]> { return new Promise((resolve, reject) => sftp.readdir(path, (error, entries) => error ? reject(error) : resolve(entries))) }
function lstat(sftp: SFTPWrapper, path: string): Promise<Stats> { return new Promise((resolve, reject) => sftp.lstat(path, (error, stats) => error ? reject(error) : resolve(stats))) }
function mkdir(sftp: SFTPWrapper, path: string, mode: number): Promise<void> { return new Promise((resolve, reject) => sftp.mkdir(path, { mode }, (error) => error ? reject(error) : resolve())) }
function rmdir(sftp: SFTPWrapper, path: string): Promise<void> { return new Promise((resolve, reject) => sftp.rmdir(path, (error) => error ? reject(error) : resolve())) }
function unlink(sftp: SFTPWrapper, path: string): Promise<void> { return new Promise((resolve, reject) => sftp.unlink(path, (error) => error ? reject(error) : resolve())) }
function rename(sftp: SFTPWrapper, source: string, target: string): Promise<void> { return new Promise((resolve, reject) => sftp.rename(source, target, (error) => error ? reject(error) : resolve())) }
function closeHandle(sftp: SFTPWrapper, handle: Buffer): Promise<void> { return new Promise((resolve, reject) => sftp.close(handle, (error) => error ? reject(error) : resolve())) }
function createEmptyFile(sftp: SFTPWrapper, path: string): Promise<void> { return new Promise((resolve, reject) => sftp.open(path, 'wx', 0o644, (error, handle) => { if (error) reject(error); else void closeHandle(sftp, handle).then(resolve, reject) })) }

async function assertMissing(sftp: SFTPWrapper, path: string): Promise<void> {
  try { await lstat(sftp, path); throw new Error(`Remote path already exists: ${path}`) } catch (error) { if (!isMissing(error)) throw error }
}

async function removeRecursive(sftp: SFTPWrapper, path: string): Promise<void> {
  const stats = await lstat(sftp, path)
  if (stats.isDirectory() && !stats.isSymbolicLink()) {
    for (const entry of await readdir(sftp, path)) if (entry.filename !== '.' && entry.filename !== '..') await removeRecursive(sftp, posix.join(path, entry.filename))
    await rmdir(sftp, path)
  } else await unlink(sftp, path)
}

function isMissing(error: unknown): boolean { const code = error instanceof Error && 'code' in error ? (error as Error & { code?: string | number }).code : undefined; return code === 2 || code === 'ENOENT' }
