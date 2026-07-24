import { execFile } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { chmod, mkdir, open, readFile, rm, stat } from 'node:fs/promises'
import { homedir } from 'node:os'
import { dirname, join } from 'node:path'
import { promisify } from 'node:util'
import ssh2 from 'ssh2'
import type { Client, SFTPWrapper } from 'ssh2'
import type { KeyDeployRequest, KeyGenerateRequest, PrivateKeyMetadata } from '../../protocol/ssh'
import type { ProfileRepository } from '../hosts/profile-repository'
import type { SshConnectionManager } from '../ssh/connection-manager'
import { inspectHostKey } from '../ssh/host-key'
import { mergeAuthorizedKey } from './authorized-keys'

const execFileAsync = promisify(execFile)

export class KeyService {
  readonly #connections: SshConnectionManager
  readonly #repository: ProfileRepository
  readonly #onAuthUpdated: (() => Promise<void>) | undefined

  constructor(connections: SshConnectionManager, repository: ProfileRepository, onAuthUpdated?: () => Promise<void>) {
    this.#connections = connections
    this.#repository = repository
    this.#onAuthUpdated = onAuthUpdated
  }

  async scan(filePaths: string[]): Promise<PrivateKeyMetadata[]> {
    const results: PrivateKeyMetadata[] = []
    for (const requestedPath of [...new Set(filePaths)]) {
      const path = expandUserPath(requestedPath)
      const info = await stat(path)
      if (!info.isFile()) throw new Error(`SSH key path is not a file: ${path}`)
      if (info.size > 2 * 1024 * 1024) throw new Error(`SSH key file is unexpectedly large: ${path}`)
      const keyData = await readFile(path)
      const text = keyData.toString('utf8')
      if (!looksLikePrivateKey(text)) throw new Error(`Selected file is not a supported private key: ${path}`)
      const parsed = ssh2.utils.parseKey(keyData)
      let algorithm: string | undefined
      let fingerprint: string | undefined
      let comment: string | undefined
      let publicKeyPath: string | undefined
      if (!(parsed instanceof Error)) {
        algorithm = parsed.type
        comment = parsed.comment || undefined
        fingerprint = inspectHostKey(publicKeyBlob(parsed.getPublicSSH().toString())).sha256Fingerprint
      } else if (!/encrypted|passphrase/i.test(parsed.message)) {
        throw new Error(`Unable to parse SSH private key ${path}: ${parsed.message}`)
      }
      try {
        const companionPath = `${path}.pub`
        const publicText = await readFile(companionPath, 'utf8')
        const parsedPublic = ssh2.utils.parseKey(publicText)
        if (parsedPublic instanceof Error) throw parsedPublic
        publicKeyPath = companionPath
        algorithm = parsedPublic.type
        comment = parsedPublic.comment || comment
        fingerprint = inspectHostKey(publicKeyBlob(publicText)).sha256Fingerprint
      } catch (error) {
        if (!isMissingFile(error)) throw error
      }
      const metadata: PrivateKeyMetadata = {
        path,
        ...(publicKeyPath ? { publicKeyPath } : {}),
        format: detectKeyFormat(text),
        encrypted: parsed instanceof Error && /encrypted|passphrase/i.test(parsed.message),
        ...(algorithm ? { algorithm } : {}),
        ...(fingerprint ? { fingerprint } : {}),
        ...(comment ? { comment } : {}),
        sizeBytes: info.size,
        modifiedAt: info.mtime.toISOString(),
        scannedAt: new Date().toISOString()
      }
      results.push(await this.#repository.savePrivateKeyMetadata(metadata))
    }
    return results
  }

  listScanned(): Promise<PrivateKeyMetadata[]> {
    return this.#repository.listPrivateKeyMetadata()
  }

  async generate(request: KeyGenerateRequest): Promise<{ privateKeyPath: string; publicKeyPath: string; fingerprint: string; aclRestricted: boolean }> {
    const privateKeyPath = expandUserPath(request.privateKeyPath)
    const options = request.passphrase
      ? { comment: request.comment, passphrase: request.passphrase, cipher: 'aes256-ctr' as const, rounds: 16 }
      : { comment: request.comment }
    const pair = ssh2.utils.generateKeyPairSync('ed25519', options)
    const publicKeyPath = `${privateKeyPath}.pub`
    let privateCreated = false
    try {
      await mkdir(dirname(privateKeyPath), { recursive: true })
      await writeExclusive(privateKeyPath, pair.private, 0o600)
      privateCreated = true
      await writeExclusive(publicKeyPath, `${pair.public.trim()}\n`, 0o644)
    } catch (error) {
      if (privateCreated) await rm(privateKeyPath, { force: true })
      throw error
    } finally {
      if (request.passphrase) request.passphrase = ''
    }
    const aclRestricted = await restrictPrivateKey(privateKeyPath)
    const publicBlob = publicKeyBlob(pair.public)
    await this.scan([privateKeyPath])
    return { privateKeyPath, publicKeyPath, fingerprint: inspectHostKey(publicBlob).sha256Fingerprint, aclRestricted }
  }

  async deploy(request: KeyDeployRequest): Promise<{ success: boolean; fingerprint: string; alreadyPresent: boolean; verified: boolean; message: string }> {
    const privateKeyPath = expandUserPath(request.privateKeyPath)
    const client = this.#connections.getOnlineClient(request.hostId)
    const publicKey = await readFile(`${privateKeyPath}.pub`, 'utf8')
    const publicBlob = publicKeyBlob(publicKey)
    const fingerprint = inspectHostKey(publicBlob).sha256Fingerprint
    const sftp = await getSftp(client)
    const home = await realpath(sftp, '.')
    const sshDirectory = `${home.replace(/\/$/, '')}/.ssh`
    const authorizedKeys = `${sshDirectory}/authorized_keys`
    const temporary = `${sshDirectory}/.authorized_keys.remotedeck-${randomUUID()}.tmp`
    try {
      await ensureDirectory(sftp, sshDirectory)
      await chmodRemote(sftp, sshDirectory, 0o700)
      const existing = await readRemoteOptional(sftp, authorizedKeys)
      const merge = mergeAuthorizedKey(existing, publicKey)
      if (!merge.alreadyPresent) {
        try {
          await writeRemote(sftp, temporary, merge.content)
          await chmodRemote(sftp, temporary, 0o600)
          await renameRemote(sftp, temporary, authorizedKeys)
          await chmodRemote(sftp, authorizedKeys, 0o600)
        } catch (error) {
          await unlinkRemoteOptional(sftp, temporary)
          throw error
        }
      }
      const verified = await this.#connections.verifyPrivateKey(request.hostId, privateKeyPath, request.passphrase)
      if (verified && request.makeDefault) {
        const profile = await this.#repository.get(request.hostId)
        await this.#repository.update({
          id: request.hostId,
          patch: { auth: { name: profile.auth.name, method: 'private_key', identityFile: privateKeyPath } }
        })
        await this.#onAuthUpdated?.()
      }
      return { success: verified, fingerprint, alreadyPresent: merge.alreadyPresent, verified, message: verified ? '公钥部署并通过全新私钥连接验证。' : '公钥已写入，但复验失败。' }
    } finally {
      sftp.end()
      if (request.passphrase) request.passphrase = ''
    }
  }

  async verify(hostId: string, privateKeyPath: string, passphrase?: string): Promise<{ success: boolean; fingerprint: string; verified: boolean; message: string }> {
    privateKeyPath = expandUserPath(privateKeyPath)
    const publicKey = await readFile(`${privateKeyPath}.pub`, 'utf8')
    const fingerprint = inspectHostKey(publicKeyBlob(publicKey)).sha256Fingerprint
    const verified = await this.#connections.verifyPrivateKey(hostId, privateKeyPath, passphrase)
    return { success: verified, fingerprint, verified, message: verified ? '私钥认证验证成功。' : '私钥认证验证失败。' }
  }
}

async function writeExclusive(path: string, content: string, mode: number): Promise<void> {
  const handle = await open(path, 'wx', mode)
  try { await handle.writeFile(content, 'utf8'); await handle.sync() } finally { await handle.close() }
}

async function restrictPrivateKey(path: string): Promise<boolean> {
  if (process.platform !== 'win32') { await chmod(path, 0o600); return true }
  try {
    const { stdout } = await execFileAsync('whoami.exe', ['/user', '/fo', 'csv', '/nh'], { windowsHide: true })
    const sid = /,"(S-[0-9-]+)"/.exec(stdout)?.[1]
    if (!sid) return false
    await execFileAsync('icacls.exe', [path, '/grant:r', `*${sid}:(F)`], { windowsHide: true })
    await execFileAsync('icacls.exe', [path, '/inheritance:r'], { windowsHide: true })
    return true
  } catch {
    return false
  }
}

function publicKeyBlob(publicKey: string): Buffer {
  const payload = publicKey.trim().split(/\s+/)[1]
  if (!payload) throw new Error('OpenSSH public key payload is missing')
  return Buffer.from(payload, 'base64')
}

function looksLikePrivateKey(value: string): boolean {
  return /-----BEGIN (?:OPENSSH |RSA |EC |DSA |ENCRYPTED )?PRIVATE KEY-----|^PuTTY-User-Key-File-/m.test(value)
}

function detectKeyFormat(value: string): PrivateKeyMetadata['format'] {
  if (/-----BEGIN OPENSSH PRIVATE KEY-----/.test(value)) return 'openssh'
  if (/-----BEGIN (?:RSA |EC |DSA |ENCRYPTED )?PRIVATE KEY-----/.test(value)) return 'pem'
  if (/^PuTTY-User-Key-File-/m.test(value)) return 'putty'
  return 'unknown'
}

function isMissingFile(error: unknown): boolean {
  return error instanceof Error && 'code' in error && (error as NodeJS.ErrnoException).code === 'ENOENT'
}

function getSftp(client: Client): Promise<SFTPWrapper> { return new Promise((resolve, reject) => client.sftp((error, sftp) => error ? reject(error) : resolve(sftp))) }
function realpath(sftp: SFTPWrapper, path: string): Promise<string> { return new Promise((resolve, reject) => sftp.realpath(path, (error, resolved) => error ? reject(error) : resolve(resolved))) }
function ensureDirectory(sftp: SFTPWrapper, path: string): Promise<void> {
  return new Promise((resolve, reject) => sftp.stat(path, (statError) => {
    if (!statError) { resolve(); return }
    sftp.mkdir(path, { mode: 0o700 }, (mkdirError) => mkdirError ? reject(mkdirError) : resolve())
  }))
}
function chmodRemote(sftp: SFTPWrapper, path: string, mode: number): Promise<void> { return new Promise((resolve, reject) => sftp.chmod(path, mode, (error) => error ? reject(error) : resolve())) }
function writeRemote(sftp: SFTPWrapper, path: string, content: string): Promise<void> { return new Promise((resolve, reject) => sftp.writeFile(path, content, { mode: 0o600 }, (error) => error ? reject(error) : resolve())) }
function renameRemote(sftp: SFTPWrapper, oldPath: string, newPath: string): Promise<void> { return new Promise((resolve, reject) => sftp.rename(oldPath, newPath, (error) => error ? reject(error) : resolve())) }
function unlinkRemoteOptional(sftp: SFTPWrapper, path: string): Promise<void> { return new Promise((resolve) => sftp.unlink(path, () => resolve())) }
function readRemoteOptional(sftp: SFTPWrapper, path: string): Promise<string> { return new Promise((resolve, reject) => sftp.readFile(path, { encoding: 'utf8' }, (error, data) => error ? (isSftpMissing(error) ? resolve('') : reject(error)) : resolve(String(data)))) }
function isSftpMissing(error: Error): boolean { const code = (error as Error & { code?: string | number }).code; return code === 2 || code === 'ENOENT' }
function expandUserPath(value: string): string { return value.startsWith('~/') || value.startsWith('~\\') ? join(homedir(), value.slice(2)) : value }
