import { mkdir, mkdtemp, open, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { createServer, type Socket } from 'node:net'
import pino from 'pino'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { KeyService } from '../../src/core/keys/key-service'
import { ProfileRepository } from '../../src/core/hosts/profile-repository'
import { SshConnectionManager } from '../../src/core/ssh/connection-manager'
import { TerminalService } from '../../src/core/terminal/terminal-service'
import type { TerminalEvent } from '../../src/protocol/terminal'
import { SftpService } from '../../src/core/sftp/sftp-service'
import { TransferService } from '../../src/core/sftp/transfer-service'
import type { TransferJob } from '../../src/protocol/domain'
import { TunnelService } from '../../src/core/tunnels/tunnel-service'
import { TelemetryService } from '../../src/core/telemetry/telemetry-service'
import { BtopService } from '../../src/core/telemetry/btop-service'
import { CommandService } from '../../src/core/commands/command-service'
import { CodexService } from '../../src/core/commands/codex-service'

const host = process.env['REMOTEDECK_OPENSSH_HOST']
const port = Number(process.env['REMOTEDECK_OPENSSH_PORT'])
const remoteForwardPort = Number(process.env['REMOTEDECK_REMOTE_FORWARD_PORT'] ?? 17890)
if (!host || !Number.isInteger(port) || port <= 0) throw new Error('Docker OpenSSH endpoint environment is required')

let directory = ''
beforeAll(async () => { directory = await mkdtemp(join(tmpdir(), 'remotedeck-openssh-')) })
afterAll(async () => rm(directory, { recursive: true, force: true }))

describe('Docker OpenSSH password-to-key lifecycle', () => {
  it('accepts the first fingerprint, deploys Ed25519, and reconnects with the key', async () => {
    const repository = new ProfileRepository(join(directory, 'profiles.json'))
    const profile = await repository.create({ alias: 'docker-openssh', hostname: host, port, username: 'remotedeck', groups: ['integration'], workspacePath: '/home/remotedeck', auth: { name: 'password', method: 'password' }, advanced: { connectTimeoutSeconds: 10, serverAliveIntervalSeconds: 30, serverAliveCountMax: 3, tcpKeepAlive: true, compression: false, identitiesOnly: false } })
    const connections = new SshConnectionManager(repository)
    const keys = new KeyService(connections, repository)
    const terminals = new TerminalService(connections, repository)
    const sftp = new SftpService(connections)
    const transfers = new TransferService(sftp)

    const first = await connections.connect(profile.host.id, { password: 'remotedeck-test-only' })
    expect(first.state).toBe('awaiting_host_key')
    if (!first.hostKeyCandidate) throw new Error('OpenSSH did not provide a host-key candidate')
    await connections.acceptCandidate(first.hostKeyCandidate.id)

    const passwordConnection = await connections.connect(profile.host.id, { password: 'remotedeck-test-only' })
    expect(passwordConnection).toMatchObject({ state: 'online', capabilities: { shell: true, sftp: true, python3: true, writableWorkspace: true } })

    const privateKeyPath = join(directory, 'id_ed25519')
    const generated = await keys.generate({ privateKeyPath, comment: 'RemoteDeck integration', passphrase: 'integration-passphrase' })
    expect(generated.fingerprint).toMatch(/^SHA256:/)
    const deployed = await keys.deploy({ hostId: profile.host.id, privateKeyPath, passphrase: 'integration-passphrase', makeDefault: true })
    expect(deployed).toMatchObject({ success: true, verified: true, alreadyPresent: false })
    const deduplicated = await keys.deploy({ hostId: profile.host.id, privateKeyPath, passphrase: 'integration-passphrase', makeDefault: true })
    expect(deduplicated).toMatchObject({ success: true, verified: true, alreadyPresent: true })

    await connections.disconnect(profile.host.id)
    const keyConnection = await connections.connect(profile.host.id, { passphrase: 'integration-passphrase' })
    expect(keyConnection.state).toBe('online')
    let output = ''
    terminals.on('event', (event: TerminalEvent) => { if (event.type === 'data') output += event.data })
    const terminal = await terminals.create({ hostId: profile.host.id, cwd: '/home/remotedeck', cols: 80, rows: 24 })
    expect(terminal.state).toBe('online')
    terminals.resize({ sessionId: terminal.id, cols: 120, rows: 40 })
    terminals.write(terminal.id, `bash -lc 'printf BASH_OK' && vim --clean -Nu NONE -n -es -c q && printf ' VIM_OK' && tmux -L remotedeck-m3 new-session -d -s m3 'sleep 2' && tmux -L remotedeck-m3 has-session -t m3 && printf ' TMUX_OK' && btop --version | head -n1; mkdir -p -- "$HOME/中文 路径"; cd -- "$HOME/中文 路径"; printf ' PATH_OK:%s ' "$PWD"; stty size; printf 'M3_COMMANDS_DONE\\n'\r`)
    await waitForOutput(() => output, 'M3_COMMANDS_DONE')
    expect(output).toContain('BASH_OK')
    expect(output).toContain('VIM_OK')
    expect(output).toContain('TMUX_OK')
    expect(output).toMatch(/btop version/i)
    expect(output).toContain('PATH_OK:/home/remotedeck/中文 路径')
    expect(output).toContain('40 120')
    terminals.write(terminal.id, 'sleep 30\r')
    await new Promise((resolve) => setTimeout(resolve, 250))
    terminals.write(terminal.id, '\u0003')
    terminals.write(terminal.id, `printf 'CTRL_C_OK\\n'\r`)
    await waitForOutput(() => output, 'CTRL_C_OK')

    const remoteRoot = await sftp.create(profile.host.id, '/home/remotedeck', 'm4-sftp', 'directory')
    const empty = await sftp.create(profile.host.id, remoteRoot, '空 文件.txt', 'file')
    const renamedEmpty = await sftp.rename(profile.host.id, empty, '已重命名 空.txt')
    expect((await sftp.list(profile.host.id, remoteRoot, true)).entries).toEqual(expect.arrayContaining([expect.objectContaining({ path: renamedEmpty, type: 'file', size: 0 })]))

    const uploadDirectory = join(directory, '上传 目录')
    await mkdir(join(uploadDirectory, 'nested'), { recursive: true })
    await writeFile(join(uploadDirectory, 'nested', '内容 文件.txt'), 'RemoteDeck SFTP 中文内容\n', 'utf8')
    await writeFile(join(uploadDirectory, 'empty.txt'), '', 'utf8')
    const largeFile = join(directory, 'large 100MiB.bin')
    const largeHandle = await open(largeFile, 'w')
    await largeHandle.truncate(100 * 1024 * 1024)
    await largeHandle.close()
    const uploadJobs = transfers.startUpload({ hostId: profile.host.id, sources: [uploadDirectory, largeFile], remoteDirectory: remoteRoot, conflictPolicy: 'overwrite' })
    await Promise.all(uploadJobs.map((job) => waitForJob(transfers, job.id, ['completed'])))
    expect((await sftp.list(profile.host.id, remoteRoot, true)).entries).toEqual(expect.arrayContaining([
      expect.objectContaining({ name: '上传 目录', type: 'directory' }),
      expect.objectContaining({ name: 'large 100MiB.bin', type: 'file', size: 100 * 1024 * 1024 })
    ]))
    const renamedConflict = transfers.startUpload({ hostId: profile.host.id, sources: [largeFile], remoteDirectory: remoteRoot, conflictPolicy: 'rename' })[0]
    if (!renamedConflict) throw new Error('Missing rename-conflict job')
    const renamedJob = await waitForJob(transfers, renamedConflict.id, ['completed'])
    expect(renamedJob.destination).toContain('large 100MiB (1).bin')

    const downloadDirectory = join(directory, 'downloads')
    await mkdir(downloadDirectory)
    const downloadJobs = transfers.startDownload({ hostId: profile.host.id, sources: [`${remoteRoot}/上传 目录`, `${remoteRoot}/large 100MiB.bin`, renamedEmpty], localDirectory: downloadDirectory, conflictPolicy: 'overwrite' })
    await Promise.all(downloadJobs.map((job) => waitForJob(transfers, job.id, ['completed'])))
    expect(await readFile(join(downloadDirectory, '上传 目录', 'nested', '内容 文件.txt'), 'utf8')).toBe('RemoteDeck SFTP 中文内容\n')
    expect((await stat(join(downloadDirectory, 'large 100MiB.bin'))).size).toBe(100 * 1024 * 1024)
    expect((await stat(join(downloadDirectory, '已重命名 空.txt'))).size).toBe(0)

    const cancelFile = join(directory, 'cancel 256MiB.bin')
    const cancelHandle = await open(cancelFile, 'w')
    await cancelHandle.truncate(256 * 1024 * 1024)
    await cancelHandle.close()
    const cancelJob = transfers.startUpload({ hostId: profile.host.id, sources: [cancelFile], remoteDirectory: remoteRoot, conflictPolicy: 'overwrite' })[0]
    if (!cancelJob) throw new Error('Missing cancellation job')
    transfers.cancel(cancelJob.id)
    await waitForJob(transfers, cancelJob.id, ['cancelled'])
    expect((await sftp.list(profile.host.id, remoteRoot, true)).entries.some((entry) => entry.name.includes(`remotedeck-${cancelJob.id}`))).toBe(false)

    terminals.write(terminal.id, `ln -s . '/home/remotedeck/m4-sftp/loop-link'; printf 'SYMLINK_DONE\\n'\r`)
    await waitForOutput(() => output, 'SYMLINK_DONE')
    expect((await sftp.list(profile.host.id, remoteRoot, true)).entries).toEqual(expect.arrayContaining([expect.objectContaining({ name: 'loop-link', type: 'symlink' })]))
    const symlinkJob = transfers.startDownload({ hostId: profile.host.id, sources: [`${remoteRoot}/loop-link`], localDirectory: downloadDirectory, conflictPolicy: 'overwrite' })[0]
    if (!symlinkJob) throw new Error('Missing symlink job')
    await waitForJob(transfers, symlinkJob.id, ['failed'])
    await sftp.delete(profile.host.id, [remoteRoot])
    expect((await sftp.list(profile.host.id, '/home/remotedeck', true)).entries.some((entry) => entry.name === 'm4-sftp')).toBe(false)

    const tunnels = new TunnelService(repository, connections, pino({ enabled: false }))
    const localForwardPort = await reservePort()
    const targetSockets = new Set<Socket>()
    const target = createServer((socket) => {
      targetSockets.add(socket)
      socket.once('close', () => targetSockets.delete(socket))
      socket.on('error', () => socket.destroy())
      socket.end('HTTP/1.1 200 OK\r\nContent-Length: 17\r\nConnection: close\r\n\r\nREMOTE_FORWARD_OK')
    })
    await new Promise<void>((resolve, reject) => { target.once('error', reject); target.listen(0, '127.0.0.1', () => resolve()) })
    const targetAddress = target.address()
    if (!targetAddress || typeof targetAddress === 'string') throw new Error('RemoteForward target has no TCP port')
    const localTunnel = await repository.addTunnel({ hostId: profile.host.id, name: 'Docker LocalForward', direction: 'local', bindAddress: '127.0.0.1', sourcePort: localForwardPort, targetHost: '127.0.0.1', targetPort: 18080, autoStart: false, healthCheck: { type: 'http', intervalSeconds: 2, timeoutMs: 2000, path: '/', expectedStatus: 200 } })
    const remoteTunnel = await repository.addTunnel({ hostId: profile.host.id, name: 'Docker RemoteForward', direction: 'remote', bindAddress: '0.0.0.0', sourcePort: remoteForwardPort, targetHost: '127.0.0.1', targetPort: targetAddress.port, autoStart: false, healthCheck: { type: 'tcp', intervalSeconds: 2, timeoutMs: 2000 } })
    try {
      await expect(tunnels.start(localTunnel.id, { passphrase: 'integration-passphrase' })).resolves.toMatchObject({ state: 'online' })
      await expect(tunnels.start(remoteTunnel.id, { passphrase: 'integration-passphrase' })).resolves.toMatchObject({ state: 'online' })
      expect((await fetch(`http://127.0.0.1:${String(localForwardPort)}/`)).status).toBe(200)
      expect(await (await fetch(`http://127.0.0.1:${String(remoteForwardPort)}/`)).text()).toBe('REMOTE_FORWARD_OK')
      await tunnels.stop(localTunnel.id)
      expect(await (await fetch(`http://127.0.0.1:${String(remoteForwardPort)}/`)).text()).toBe('REMOTE_FORWARD_OK')
      expect((await tunnels.list(profile.host.id)).find((item) => item.profile.id === localTunnel.id)?.state).toBe('stopped')
      expect((await tunnels.list(profile.host.id)).find((item) => item.profile.id === remoteTunnel.id)?.state).toBe('online')
    } finally {
      await tunnels.stopAll()
      targetSockets.forEach((socket) => socket.destroy())
      await new Promise<void>((resolve) => target.close(() => resolve()))
    }

    const collectorPath = resolve(import.meta.dirname, '../../../../packages/remote-collector/collector.py')
    const telemetry = new TelemetryService(connections, () => Promise.resolve({ schemaVersion: 1, launchAtLogin: false, closeToTray: true, terminalFontFamily: 'monospace', terminalFontSize: 14, telemetryIntervalSeconds: 1, telemetryRetentionMinutes: 5, sshConfigPath: '', downloadDirectory: '', autoReconnect: true, btopWatchdogEnabled: false, btopRotationMinutes: 15, logLevel: 'info', onboardingCompleted: true }), () => readFile(collectorPath, 'utf8'), pino({ enabled: false }))
    const btop = new BtopService(connections, pino({ enabled: false }))
    try {
      await telemetry.start(profile.host.id)
      await waitForTelemetry(telemetry, profile.host.id, (snapshot) => snapshot.cpu.perCorePercent.length > 0)
      const firstTelemetry = telemetry.history(profile.host.id).at(-1)
      expect(firstTelemetry).toMatchObject({ schemaVersion: 1, hostId: profile.host.id, currentUser: 'remotedeck', gpus: [], gpuProcesses: [] })
      expect(firstTelemetry?.memory.totalBytes).toBeGreaterThan(0)
      expect(firstTelemetry?.processes.length).toBeGreaterThan(0)

      await expect(btop.probe(profile.host.id)).resolves.toMatchObject({ installed: true })
      await expect(btop.start(profile.host.id, 5)).resolves.toMatchObject({ watchdogState: 'running' })
      expect(btop.stop(profile.host.id).watchdogState).toBe('stopped')

      const sleepResult = await execute(connections.getOnlineClient(profile.host.id), 'nohup sleep 60 >/dev/null 2>&1 & echo $!')
      expect(sleepResult.code).toBe(0)
      const sleepPid = Number(sleepResult.stdout.trim())
      expect(Number.isInteger(sleepPid) && sleepPid > 0).toBe(true)
      const sleepProcess = await waitForTelemetry(telemetry, profile.host.id, (snapshot) => snapshot.processes.some((item) => item.pid === sleepPid))
      const processSnapshot = sleepProcess.processes.find((item) => item.pid === sleepPid)
      if (!processSnapshot) throw new Error('Collector did not report the integration sleep process')
      await expect(telemetry.signal({ hostId: profile.host.id, pid: sleepPid, signal: 'TERM', expectedUser: processSnapshot.user, expectedCommand: processSnapshot.command, confirmKill: false })).resolves.toMatchObject({ delivered: true, signal: 'TERM' })
      await new Promise((resolve) => setTimeout(resolve, 100))
      expect((await execute(connections.getOnlineClient(profile.host.id), `kill -0 ${String(sleepPid)} 2>/dev/null`)).code).not.toBe(0)
    } finally {
      telemetry.stopAll()
      btop.stopAll()
    }

    const commands = new CommandService(repository, connections, terminals)
    const commandPreset = await repository.addCommand({ hostId: profile.host.id, name: 'M7 Docker command', description: '', group: 'integration', command: 'printf M7_COMMAND_OK', risk: 'L1', requiresPty: false, requiresSudo: false, sortOrder: 1 })
    await expect(commands.run({ hostId: profile.host.id, presetId: commandPreset.id, confirmed: false, confirmationInput: '' })).rejects.toThrow(/confirmation/)
    const commandJob = await commands.run({ hostId: profile.host.id, presetId: commandPreset.id, confirmed: true, confirmationInput: '' })
    await waitForCommandJob(commands, commandJob.id)
    expect(commands.listJobs().find((item) => item.id === commandJob.id)?.output).toContain('M7_COMMAND_OK')
    const dangerousPreset = await repository.addCommand({ hostId: profile.host.id, name: 'M7 gate only', description: '', group: 'integration', command: 'rm -rf /tmp/remotedeck-never-created', risk: 'L0', requiresPty: false, requiresSudo: false, sortOrder: 2 })
    await expect(commands.run({ hostId: profile.host.id, presetId: dangerousPreset.id, confirmed: true, confirmationInput: 'wrong' })).rejects.toThrow(/does not match/)

    const codex = new CodexService(repository, connections, terminals)
    await expect(codex.probe(profile.host.id)).resolves.toMatchObject({ installed: true, login: 'logged_out', capabilities: { deviceAuth: true, resumeLast: true, update: true } })
    expect((await execute(connections.getOnlineClient(profile.host.id), 'touch /tmp/remotedeck-codex-logged-in')).code).toBe(0)
    await expect(codex.probe(profile.host.id)).resolves.toMatchObject({ installed: true, login: 'logged_in' })
    expect((await execute(connections.getOnlineClient(profile.host.id), 'rm -f /tmp/remotedeck-codex-logged-in')).code).toBe(0)

    terminals.closeAll()
    await connections.disconnectAll()
  })
})

async function waitForOutput(read: () => string, marker: string): Promise<void> {
  const deadline = Date.now() + 15_000
  while (!read().includes(marker)) {
    if (Date.now() >= deadline) throw new Error(`Timed out waiting for terminal marker ${marker}. Output: ${read().slice(-4000)}`)
    await new Promise((resolve) => setTimeout(resolve, 50))
  }
}

async function waitForCommandJob(commands: CommandService, jobId: string): Promise<void> {
  const deadline = Date.now() + 10_000
  for (;;) {
    const job = commands.listJobs().find((item) => item.id === jobId)
    if (job?.state === 'completed') return
    if (job?.state === 'failed' || job?.state === 'cancelled') throw new Error(`Command job failed: ${JSON.stringify(job)}`)
    if (Date.now() >= deadline) throw new Error(`Timed out waiting for command job: ${JSON.stringify(job)}`)
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
}

async function waitForTelemetry(telemetry: TelemetryService, hostId: string, predicate: (snapshot: NonNullable<ReturnType<TelemetryService['history']>[number]>) => boolean): Promise<NonNullable<ReturnType<TelemetryService['history']>[number]>> {
  const deadline = Date.now() + 15_000
  for (;;) {
    const snapshot = telemetry.history(hostId).at(-1)
    if (snapshot && predicate(snapshot)) return snapshot
    const status = telemetry.status(hostId)
    if (status.state === 'dependency_missing' || status.state === 'failed') throw new Error(status.lastError ?? status.state)
    if (Date.now() >= deadline) throw new Error(`Timed out waiting for telemetry: ${JSON.stringify(status)}`)
    await new Promise((resolve) => setTimeout(resolve, 50))
  }
}

function execute(client: ReturnType<SshConnectionManager['getOnlineClient']>, command: string): Promise<{ stdout: string; stderr: string; code: number }> {
  return new Promise((resolve, reject) => client.exec(command, (error, channel) => {
    if (error) { reject(error); return }
    let stdout = ''
    let stderr = ''
    channel.setEncoding('utf8')
    channel.stderr.setEncoding('utf8')
    channel.on('data', (chunk: string) => { stdout += chunk })
    channel.stderr.on('data', (chunk: string) => { stderr += chunk })
    channel.once('error', reject)
    channel.once('close', (code?: number) => resolve({ stdout, stderr, code: code ?? 0 }))
  }))
}

async function reservePort(): Promise<number> {
  const server = createServer()
  await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', () => resolve()) })
  const address = server.address()
  if (!address || typeof address === 'string') throw new Error('LocalForward reservation has no TCP port')
  const reserved = address.port
  await new Promise<void>((resolve) => server.close(() => resolve()))
  return reserved
}

async function waitForJob(transfers: TransferService, jobId: string, states: TransferJob['state'][]): Promise<TransferJob> {
  const deadline = Date.now() + 120_000
  for (;;) {
    const job = transfers.list().find((item) => item.id === jobId)
    if (job && states.includes(job.state)) return job
    if (job?.state === 'failed') throw new Error(`Transfer ${jobId} failed: ${job.error ?? 'unknown error'}`)
    if (Date.now() >= deadline) throw new Error(`Timed out waiting for transfer ${jobId}: ${JSON.stringify(job)}`)
    await new Promise((resolve) => setTimeout(resolve, 50))
  }
}
