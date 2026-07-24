import { randomUUID } from 'node:crypto'
import { EventEmitter } from 'node:events'
import { createReadStream, createWriteStream } from 'node:fs'
import { lstat as localLstat, mkdir as localMkdir, readdir as localReaddir, rename as localRename, rm as localRemove } from 'node:fs/promises'
import { basename, dirname, extname, join } from 'node:path'
import { posix } from 'node:path'
import { Transform } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import type { FileEntryWithStats, SFTPWrapper, Stats } from 'ssh2'
import type { TransferJob } from '../../protocol/domain'
import type { ConflictPolicy, TransferDownloadRequest, TransferUploadRequest } from '../../protocol/sftp'
import type { SftpService } from './sftp-service'

type TransferSpec = { direction: 'upload'; hostId: string; source: string; destination: string; policy: ConflictPolicy } | { direction: 'download'; hostId: string; source: string; destination: string; policy: ConflictPolicy }

export class TransferService extends EventEmitter {
  readonly #sftp: SftpService
  readonly #jobs = new Map<string, TransferJob>()
  readonly #specs = new Map<string, TransferSpec>()
  readonly #controllers = new Map<string, AbortController>()
  readonly #queue: string[] = []
  #active = 0

  constructor(sftp: SftpService) { super(); this.#sftp = sftp }

  list(): TransferJob[] { return [...this.#jobs.values()].sort((left, right) => right.createdAt.localeCompare(left.createdAt)) }

  cancelAll(): void { for (const job of this.#jobs.values()) if (job.state === 'queued' || job.state === 'running') this.cancel(job.id) }

  startUpload(request: TransferUploadRequest): TransferJob[] {
    return request.sources.map((source) => this.#enqueue({ direction: 'upload', hostId: request.hostId, source, destination: posix.join(request.remoteDirectory, basename(source)), policy: request.conflictPolicy }))
  }

  startDownload(request: TransferDownloadRequest): TransferJob[] {
    return request.sources.map((source) => this.#enqueue({ direction: 'download', hostId: request.hostId, source, destination: join(request.localDirectory, posix.basename(source)), policy: request.conflictPolicy }))
  }

  cancel(jobId: string): TransferJob {
    const job = this.#required(jobId)
    if (job.state === 'completed' || job.state === 'failed' || job.state === 'cancelled') return job
    if (job.state === 'queued') {
      const index = this.#queue.indexOf(jobId)
      if (index >= 0) this.#queue.splice(index, 1)
      return this.#update(jobId, { state: 'cancelled', error: '用户已取消。' })
    }
    this.#controllers.get(jobId)?.abort()
    return this.#update(jobId, { error: '正在取消并清理本任务的临时文件。' })
  }

  retry(jobId: string): TransferJob {
    const job = this.#required(jobId)
    if (job.state !== 'failed' && job.state !== 'cancelled') throw new Error('Only failed or cancelled transfers can be retried')
    this.#update(jobId, { state: 'queued', bytesTransferred: 0, totalBytes: null, speedBytesPerSecond: 0, error: undefined })
    this.#queue.push(jobId)
    this.#drain()
    return this.#required(jobId)
  }

  localPathFor(jobId: string): string {
    const job = this.#required(jobId)
    if (job.direction !== 'download' || job.state !== 'completed') throw new Error('Completed download is required')
    return job.destination
  }

  #enqueue(spec: TransferSpec): TransferJob {
    const now = new Date().toISOString()
    const job: TransferJob = { schemaVersion: 1, id: randomUUID(), hostId: spec.hostId, direction: spec.direction, source: spec.source, destination: spec.destination, state: 'queued', conflictPolicy: spec.policy, bytesTransferred: 0, totalBytes: null, speedBytesPerSecond: 0, createdAt: now, updatedAt: now }
    this.#jobs.set(job.id, job)
    this.#specs.set(job.id, spec)
    this.#queue.push(job.id)
    this.#emit(job)
    this.#drain()
    return job
  }

  #drain(): void {
    while (this.#active < 3 && this.#queue.length > 0) {
      const id = this.#queue.shift()
      if (!id) return
      this.#active += 1
      void this.#run(id).finally(() => { this.#active -= 1; this.#drain() })
    }
  }

  async #run(jobId: string): Promise<void> {
    const spec = this.#specs.get(jobId)
    if (!spec) throw new Error(`Missing transfer specification: ${jobId}`)
    const controller = new AbortController()
    this.#controllers.set(jobId, controller)
    const started = Date.now()
    this.#update(jobId, { state: 'running', bytesTransferred: 0, speedBytesPerSecond: 0, error: undefined })
    try {
      await this.#sftp.withSftp(spec.hostId, async (sftp) => {
        const total = spec.direction === 'upload' ? await localSize(spec.source) : await remoteSize(sftp, spec.source)
        this.#update(jobId, { totalBytes: total })
        const progress = (bytes: number): void => {
          const current = this.#required(jobId)
          const bytesTransferred = current.bytesTransferred + bytes
          this.#update(jobId, { bytesTransferred, speedBytesPerSecond: bytesTransferred / Math.max(0.001, (Date.now() - started) / 1000) })
        }
        if (spec.direction === 'upload') {
          const destination = await uploadItem(sftp, spec.source, spec.destination, spec.policy, jobId, controller.signal, progress)
          this.#update(jobId, { destination })
        } else {
          const destination = await downloadItem(sftp, spec.source, spec.destination, spec.policy, jobId, controller.signal, progress)
          this.#update(jobId, { destination })
        }
      })
      if (controller.signal.aborted) this.#update(jobId, { state: 'cancelled', speedBytesPerSecond: 0, error: '用户已取消。' })
      else this.#update(jobId, { state: 'completed', speedBytesPerSecond: 0, bytesTransferred: this.#required(jobId).totalBytes ?? this.#required(jobId).bytesTransferred, error: undefined })
    } catch (error) {
      const cancelled = controller.signal.aborted
      this.#update(jobId, { state: cancelled ? 'cancelled' : 'failed', speedBytesPerSecond: 0, error: cancelled ? '用户已取消；本任务临时文件已清理。' : messageOf(error) })
    } finally {
      this.#controllers.delete(jobId)
    }
  }

  #required(jobId: string): TransferJob { const job = this.#jobs.get(jobId); if (!job) throw new Error(`Unknown transfer job: ${jobId}`); return job }

  #update(jobId: string, patch: Partial<TransferJob>): TransferJob {
    const current = this.#required(jobId)
    const next = { ...current, ...patch, ...(patch.error === undefined ? { error: undefined } : {}), updatedAt: new Date().toISOString() } as TransferJob
    if (patch.error === undefined) delete next.error
    this.#jobs.set(jobId, next)
    this.#emit(next)
    return next
  }

  #emit(job: TransferJob): void { this.emit('event', { job }) }
}

async function uploadItem(sftp: SFTPWrapper, localPath: string, requestedTarget: string, policy: ConflictPolicy, jobId: string, signal: AbortSignal, progress: (bytes: number) => void): Promise<string> {
  throwIfAborted(signal)
  const stats = await localLstat(localPath)
  if (stats.isSymbolicLink()) throw new Error(`Symbolic-link upload is not followed: ${localPath}`)
  const target = await resolveRemoteConflict(sftp, requestedTarget, stats.isDirectory(), policy)
  if (!target) return requestedTarget
  if (stats.isDirectory()) {
    await remoteMkdir(sftp, target, stats.mode & 0o777 || 0o755)
    for (const name of await localReaddir(localPath)) await uploadItem(sftp, join(localPath, name), posix.join(target, name), policy, jobId, signal, progress)
    return target
  }
  const temporary = posix.join(posix.dirname(target), `.${posix.basename(target)}.remotedeck-${jobId}.upload`)
  await remoteUnlinkOptional(sftp, temporary)
  try {
    await pipeline(createReadStream(localPath), progressTransform(progress), sftp.createWriteStream(temporary, { flags: 'wx', mode: stats.mode & 0o777 || 0o600 }), { signal })
    await remoteChmod(sftp, temporary, stats.mode & 0o777 || 0o600)
    await remoteRename(sftp, temporary, target)
    return target
  } finally { await remoteUnlinkOptional(sftp, temporary) }
}

async function downloadItem(sftp: SFTPWrapper, remotePath: string, requestedTarget: string, policy: ConflictPolicy, jobId: string, signal: AbortSignal, progress: (bytes: number) => void): Promise<string> {
  throwIfAborted(signal)
  const stats = await remoteLstat(sftp, remotePath)
  if (stats.isSymbolicLink()) throw new Error(`Symbolic-link download is not followed: ${remotePath}`)
  const target = await resolveLocalConflict(requestedTarget, stats.isDirectory(), policy)
  if (!target) return requestedTarget
  if (stats.isDirectory()) {
    await localMkdir(target, { recursive: false, mode: stats.mode & 0o777 || 0o755 })
    for (const entry of await remoteReaddir(sftp, remotePath)) if (entry.filename !== '.' && entry.filename !== '..') await downloadItem(sftp, posix.join(remotePath, entry.filename), join(target, entry.filename), policy, jobId, signal, progress)
    return target
  }
  await localMkdir(dirname(target), { recursive: true })
  const temporary = `${target}.remotedeck-${jobId}.part`
  await localRemove(temporary, { force: true })
  try {
    await pipeline(sftp.createReadStream(remotePath), progressTransform(progress), createWriteStream(temporary, { flags: 'wx', mode: stats.mode & 0o777 || 0o600 }), { signal })
    await localRename(temporary, target)
    return target
  } finally { await localRemove(temporary, { force: true }) }
}

function progressTransform(progress: (bytes: number) => void): Transform { return new Transform({ transform(chunk: Buffer, _encoding, callback) { progress(chunk.length); callback(null, chunk) } }) }
function throwIfAborted(signal: AbortSignal): void { if (signal.aborted) throw new DOMException('Transfer aborted', 'AbortError') }

async function localSize(path: string): Promise<number> { const stats = await localLstat(path); if (stats.isSymbolicLink()) return 0; if (stats.isFile()) return stats.size; let total = 0; for (const name of await localReaddir(path)) total += await localSize(join(path, name)); return total }
async function remoteSize(sftp: SFTPWrapper, path: string): Promise<number> { const stats = await remoteLstat(sftp, path); if (stats.isSymbolicLink()) return 0; if (!stats.isDirectory()) return stats.size; let total = 0; for (const entry of await remoteReaddir(sftp, path)) if (entry.filename !== '.' && entry.filename !== '..') total += await remoteSize(sftp, posix.join(path, entry.filename)); return total }

async function resolveRemoteConflict(sftp: SFTPWrapper, path: string, directory: boolean, policy: ConflictPolicy): Promise<string | null> {
  const existing = await remoteLstatOptional(sftp, path)
  if (!existing) return path
  if (policy === 'skip') return null
  if (policy === 'rename') return await uniqueRemotePath(sftp, path)
  await remoteRemoveRecursive(sftp, path)
  return path
}

async function resolveLocalConflict(path: string, directory: boolean, policy: ConflictPolicy): Promise<string | null> {
  const existing = await localLstatOptional(path)
  if (!existing) return path
  if (policy === 'skip') return null
  if (policy === 'rename') return await uniqueLocalPath(path)
  await localRemove(path, { recursive: directory || existing.isDirectory(), force: true })
  return path
}

async function uniqueRemotePath(sftp: SFTPWrapper, path: string): Promise<string> { for (let index = 1; index < 10_000; index += 1) { const candidate = renamedPath(path, index, posix); if (!await remoteLstatOptional(sftp, candidate)) return candidate } throw new Error(`No available renamed destination for ${path}`) }
async function uniqueLocalPath(path: string): Promise<string> { for (let index = 1; index < 10_000; index += 1) { const candidate = renamedPath(path, index); if (!await localLstatOptional(candidate)) return candidate } throw new Error(`No available renamed destination for ${path}`) }
function renamedPath(path: string, index: number, pathApi: Pick<typeof posix, 'dirname' | 'basename' | 'extname' | 'join'> = { dirname, basename, extname, join }): string { const extension = pathApi.extname(path); const name = pathApi.basename(path, extension); return pathApi.join(pathApi.dirname(path), `${name} (${String(index)})${extension}`) }

function remoteLstat(sftp: SFTPWrapper, path: string): Promise<Stats> { return new Promise((resolve, reject) => sftp.lstat(path, (error, stats) => error ? reject(error) : resolve(stats))) }
async function remoteLstatOptional(sftp: SFTPWrapper, path: string): Promise<Stats | null> { try { return await remoteLstat(sftp, path) } catch (error) { if (isMissing(error)) return null; throw error } }
function remoteReaddir(sftp: SFTPWrapper, path: string): Promise<FileEntryWithStats[]> { return new Promise((resolve, reject) => sftp.readdir(path, (error, entries) => error ? reject(error) : resolve(entries))) }
function remoteMkdir(sftp: SFTPWrapper, path: string, mode: number): Promise<void> { return new Promise((resolve, reject) => sftp.mkdir(path, { mode }, (error) => error ? reject(error) : resolve())) }
function remoteRename(sftp: SFTPWrapper, source: string, target: string): Promise<void> { return new Promise((resolve, reject) => sftp.rename(source, target, (error) => error ? reject(error) : resolve())) }
function remoteChmod(sftp: SFTPWrapper, path: string, mode: number): Promise<void> { return new Promise((resolve, reject) => sftp.chmod(path, mode, (error) => error ? reject(error) : resolve())) }
function remoteUnlink(sftp: SFTPWrapper, path: string): Promise<void> { return new Promise((resolve, reject) => sftp.unlink(path, (error) => error ? reject(error) : resolve())) }
function remoteRmdir(sftp: SFTPWrapper, path: string): Promise<void> { return new Promise((resolve, reject) => sftp.rmdir(path, (error) => error ? reject(error) : resolve())) }
async function remoteUnlinkOptional(sftp: SFTPWrapper, path: string): Promise<void> { try { await remoteUnlink(sftp, path) } catch (error) { if (!isMissing(error)) throw error } }
async function remoteRemoveRecursive(sftp: SFTPWrapper, path: string): Promise<void> { const stats = await remoteLstat(sftp, path); if (stats.isDirectory() && !stats.isSymbolicLink()) { for (const entry of await remoteReaddir(sftp, path)) if (entry.filename !== '.' && entry.filename !== '..') await remoteRemoveRecursive(sftp, posix.join(path, entry.filename)); await remoteRmdir(sftp, path) } else await remoteUnlink(sftp, path) }
async function localLstatOptional(path: string): Promise<Awaited<ReturnType<typeof localLstat>> | null> { try { return await localLstat(path) } catch (error) { if (isMissing(error)) return null; throw error } }
function isMissing(error: unknown): boolean { const code = error instanceof Error && 'code' in error ? (error as Error & { code?: string | number }).code : undefined; return code === 2 || code === 'ENOENT' }
function messageOf(value: unknown): string { return value instanceof Error ? value.message : String(value) }
