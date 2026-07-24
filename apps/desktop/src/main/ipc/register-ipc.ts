import { clipboard, dialog, shell } from 'electron'
import type { BrowserWindow, IpcMain, IpcMainInvokeEvent } from 'electron'
import type { Logger } from 'pino'
import type { ZodType } from 'zod'
import { ipcContracts } from '../../protocol/ipc'
import type { SettingsService } from '../../core/settings-service'
import type { HostService } from '../../core/hosts/host-service'
import type { ProfileRepository } from '../../core/hosts/profile-repository'
import type { KeyService } from '../../core/keys/key-service'
import type { SshConnectionManager } from '../../core/ssh/connection-manager'
import type { TerminalService } from '../../core/terminal/terminal-service'
import type { SftpService } from '../../core/sftp/sftp-service'
import type { TransferService } from '../../core/sftp/transfer-service'
import type { TunnelService } from '../../core/tunnels/tunnel-service'
import { detectClashCandidates } from '../../core/tunnels/clash-detector'
import type { TelemetryService } from '../../core/telemetry/telemetry-service'
import type { BtopService } from '../../core/telemetry/btop-service'
import type { CommandService } from '../../core/commands/command-service'
import type { CodexService } from '../../core/commands/codex-service'
import { IPC_CHANNELS } from '../../protocol/ipc'

interface IpcDependencies {
  ipcMain: IpcMain
  window: BrowserWindow
  settings: SettingsService
  hosts: HostService
  profiles: ProfileRepository
  connections: SshConnectionManager
  keys: KeyService
  terminals: TerminalService
  sftp: SftpService
  transfers: TransferService
  tunnels: TunnelService
  telemetry: TelemetryService
  btop: BtopService
  commands: CommandService
  codex: CodexService
  logger: Logger
  appVersion: string
}

export function registerIpcHandlers(dependencies: IpcDependencies): () => void {
  const { ipcMain, window, settings, hosts, profiles, connections, keys, terminals, sftp, transfers, tunnels, telemetry, btop, commands, codex, logger, appVersion } = dependencies
  const channels: string[] = []

  register(ipcContracts.bootstrap, async () => ({
    protocolVersion: 1,
    appVersion,
    platform: 'win32' as const,
    settings: await settings.get()
  }))
  register(ipcContracts.appOpenExternal, async (request) => { await shell.openExternal(request.url); return { opened: true } })
  register(ipcContracts.appClipboardRead, () => ({ text: clipboard.readText() }))
  register(ipcContracts.appClipboardWrite, (request) => { clipboard.writeText(request.text); return { written: true as const } })
  register(ipcContracts.settingsGet, () => settings.get())
  register(ipcContracts.settingsUpdate, (patch) => settings.update(patch))
  register(ipcContracts.hostsList, () => hosts.list())
  register(ipcContracts.hostsCreate, (request) => hosts.create(request))
  register(ipcContracts.hostsUpdate, async (request) => {
    const updated = await hosts.update(request)
    if (request.patch.monitorEnabled === false) telemetry.stop(request.id)
    else if (request.patch.monitorEnabled === true && connections.stateFor(request.id) === 'online') await telemetry.start(request.id)
    return updated
  })
  register(ipcContracts.hostsDelete, async (request) => ({ deleted: await hosts.delete(request.hostId) }))
  register(ipcContracts.hostsImport, (request) => hosts.importFile(request.configPath))
  register(ipcContracts.hostsTest, (request) => connections.test(request.hostId, request.credentials))
  register(ipcContracts.hostsConnect, (request) => connections.connect(request.hostId, request.credentials))
  register(ipcContracts.hostsDisconnect, (request) => connections.disconnect(request.hostId))
  register(ipcContracts.hostKeysAccept, (request) => connections.acceptCandidate(request.candidateId))
  register(ipcContracts.hostKeysReject, (request) => ({ rejected: connections.rejectCandidate(request.candidateId) }))
  register(ipcContracts.hostKeysList, () => profiles.listHostKeys())
  register(ipcContracts.hostKeysRemove, async (request) => ({ removed: await profiles.removeHostKey(request.recordId) }))
  register(ipcContracts.keysGenerate, (request) => keys.generate(request))
  register(ipcContracts.keysPickAndScan, async () => {
    const selection = await dialog.showOpenDialog(window, {
      title: '选择 SSH 私钥',
      properties: ['openFile', 'multiSelections'],
      filters: [{ name: 'SSH private keys', extensions: ['pem', 'key', 'ppk'] }, { name: 'All files', extensions: ['*'] }]
    })
    return selection.canceled ? [] : keys.scan(selection.filePaths)
  })
  register(ipcContracts.keysListScanned, () => keys.listScanned())
  register(ipcContracts.keysDeploy, (request) => keys.deploy(request))
  register(ipcContracts.keysVerify, async (request) => {
    try { return await keys.verify(request.hostId, request.privateKeyPath, request.passphrase) }
    finally { if (request.passphrase) request.passphrase = '' }
  })
  register(ipcContracts.terminalsList, () => terminals.list())
  register(ipcContracts.terminalsCreate, (request) => terminals.create(request))
  register(ipcContracts.terminalsWrite, (request) => terminals.write(request.sessionId, request.data))
  register(ipcContracts.terminalsResize, (request) => terminals.resize(request))
  register(ipcContracts.terminalsClose, (request) => terminals.close(request.sessionId))
  register(ipcContracts.terminalsReconnect, (request) => terminals.reconnect(request.sessionId))
  register(ipcContracts.sftpList, (request) => sftp.list(request.hostId, request.path, request.showHidden))
  register(ipcContracts.sftpCreate, async (request) => ({ success: true as const, affected: [await sftp.create(request.hostId, request.parentPath, request.name, request.type)] }))
  register(ipcContracts.sftpRename, async (request) => ({ success: true as const, affected: [await sftp.rename(request.hostId, request.path, request.newName)] }))
  register(ipcContracts.sftpDelete, async (request) => ({ success: true as const, affected: await sftp.delete(request.hostId, request.paths) }))
  register(ipcContracts.sftpPickUpload, async (request) => {
    const selection = await dialog.showOpenDialog(window, { title: request.kind === 'directory' ? '选择要上传的文件夹' : '选择要上传的文件', properties: request.kind === 'directory' ? ['openDirectory'] : ['openFile', 'multiSelections'] })
    return { paths: selection.filePaths, canceled: selection.canceled }
  })
  register(ipcContracts.sftpPickDownloadDirectory, async () => {
    const current = await settings.get()
    const selection = await dialog.showOpenDialog(window, { title: '选择下载目录', ...(current.downloadDirectory ? { defaultPath: current.downloadDirectory } : {}), properties: ['openDirectory', 'createDirectory'] })
    return { paths: selection.filePaths, canceled: selection.canceled }
  })
  register(ipcContracts.transfersList, () => transfers.list())
  register(ipcContracts.transfersUpload, (request) => ({ jobs: transfers.startUpload(request) }))
  register(ipcContracts.transfersDownload, (request) => ({ jobs: transfers.startDownload(request) }))
  register(ipcContracts.transfersCancel, (request) => transfers.cancel(request.jobId))
  register(ipcContracts.transfersRetry, (request) => transfers.retry(request.jobId))
  register(ipcContracts.transfersShowInFolder, (request) => { shell.showItemInFolder(transfers.localPathFor(request.jobId)); return { shown: true as const } })
  register(ipcContracts.tunnelsList, (request) => tunnels.list(request.hostId))
  register(ipcContracts.tunnelsCreate, (request) => tunnels.create(request))
  register(ipcContracts.tunnelsUpdate, (request) => tunnels.update(request))
  register(ipcContracts.tunnelsDelete, async (request) => ({ deleted: await tunnels.delete(request.tunnelId) }))
  register(ipcContracts.tunnelsStart, (request) => tunnels.start(request.tunnelId, request.credentials))
  register(ipcContracts.tunnelsStop, (request) => tunnels.stop(request.tunnelId))
  register(ipcContracts.tunnelsRestart, (request) => tunnels.restart(request.tunnelId, request.credentials))
  register(ipcContracts.tunnelsDetectClash, () => detectClashCandidates())
  register(ipcContracts.telemetryList, () => telemetry.list())
  register(ipcContracts.telemetryHistory, (request) => telemetry.history(request.hostId))
  register(ipcContracts.telemetryStart, (request) => telemetry.start(request.hostId))
  register(ipcContracts.telemetryStop, (request) => telemetry.stop(request.hostId))
  register(ipcContracts.telemetrySignal, (request) => telemetry.signal(request))
  register(ipcContracts.btopProbe, (request) => btop.probe(request.hostId))
  register(ipcContracts.btopWatchdogStart, (request) => btop.start(request.hostId, request.rotationMinutes))
  register(ipcContracts.btopWatchdogStop, (request) => btop.stop(request.hostId))
  register(ipcContracts.commandsList, (request) => commands.list(request.hostId))
  register(ipcContracts.commandsCreate, (request) => commands.create(request))
  register(ipcContracts.commandsUpdate, (request) => commands.update(request.id, request.patch))
  register(ipcContracts.commandsDelete, async (request) => ({ deleted: await commands.delete(request.presetId) }))
  register(ipcContracts.commandsAnalyze, (request) => commands.analyze(request.hostId, request.presetId))
  register(ipcContracts.commandsRun, (request) => commands.run(request))
  register(ipcContracts.commandsJobs, () => commands.listJobs())
  register(ipcContracts.commandsCancel, (request) => commands.cancel(request.jobId))
  register(ipcContracts.codexInstallPlan, () => codex.installPlan())
  register(ipcContracts.codexProbe, (request) => codex.probe(request.hostId))
  register(ipcContracts.codexAction, (request) => codex.action(request))

  const onHostState = (snapshot: unknown): void => {
    if (!window.isDestroyed()) window.webContents.send(IPC_CHANNELS.hostStateEvent, snapshot)
  }
  connections.on('state', onHostState)
  const onTerminalEvent = (event: unknown): void => {
    if (!window.isDestroyed()) window.webContents.send(IPC_CHANNELS.terminalEvent, event)
  }
  terminals.on('event', onTerminalEvent)
  const onTransferEvent = (event: unknown): void => {
    if (!window.isDestroyed()) window.webContents.send(IPC_CHANNELS.transferEvent, event)
  }
  transfers.on('event', onTransferEvent)
  const onTunnelEvent = (event: unknown): void => {
    if (!window.isDestroyed()) window.webContents.send(IPC_CHANNELS.tunnelEvent, event)
  }
  tunnels.on('event', onTunnelEvent)
  const onTelemetryEvent = (event: unknown): void => {
    if (!window.isDestroyed()) window.webContents.send(IPC_CHANNELS.telemetryEvent, event)
  }
  telemetry.on('event', onTelemetryEvent)
  btop.on('event', onTelemetryEvent)
  const onCommandEvent = (event: unknown): void => {
    if (!window.isDestroyed()) window.webContents.send(IPC_CHANNELS.commandEvent, event)
  }
  commands.on('event', onCommandEvent)

  function register<TInput, TOutput>(
    contract: { channel: string; input: ZodType<TInput>; output: ZodType<TOutput> },
    handler: (input: TInput) => TOutput | Promise<TOutput>
  ): void {
    channels.push(contract.channel)
    ipcMain.handle(contract.channel, async (event: IpcMainInvokeEvent, raw: unknown) => {
      assertTrustedSender(event, window)
      try {
        const input = contract.input.parse(raw)
        return contract.output.parse(await handler(input))
      } catch (error) {
        logger.warn({ channel: contract.channel, error }, 'IPC request rejected')
        throw error
      }
    })
  }

  return () => {
    for (const channel of channels) ipcMain.removeHandler(channel)
    connections.off('state', onHostState)
    terminals.off('event', onTerminalEvent)
    transfers.off('event', onTransferEvent)
    tunnels.off('event', onTunnelEvent)
    telemetry.off('event', onTelemetryEvent)
    btop.off('event', onTelemetryEvent)
    commands.off('event', onCommandEvent)
  }
}

function assertTrustedSender(event: IpcMainInvokeEvent, window: BrowserWindow): void {
  if (event.sender !== window.webContents || event.senderFrame !== window.webContents.mainFrame) {
    throw new Error('IPC sender is not the RemoteDeck main frame')
  }
}
