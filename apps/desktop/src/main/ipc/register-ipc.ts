import { dialog } from 'electron'
import type { BrowserWindow, IpcMain, IpcMainInvokeEvent } from 'electron'
import type { Logger } from 'pino'
import type { ZodType } from 'zod'
import { ipcContracts } from '../../protocol/ipc'
import type { SettingsService } from '../../core/settings-service'
import type { HostService } from '../../core/hosts/host-service'
import type { ProfileRepository } from '../../core/hosts/profile-repository'
import type { KeyService } from '../../core/keys/key-service'
import type { SshConnectionManager } from '../../core/ssh/connection-manager'
import { IPC_CHANNELS } from '../../protocol/ipc'

interface IpcDependencies {
  ipcMain: IpcMain
  window: BrowserWindow
  settings: SettingsService
  hosts: HostService
  profiles: ProfileRepository
  connections: SshConnectionManager
  keys: KeyService
  logger: Logger
  appVersion: string
}

export function registerIpcHandlers(dependencies: IpcDependencies): () => void {
  const { ipcMain, window, settings, hosts, profiles, connections, keys, logger, appVersion } = dependencies
  const channels: string[] = []

  register(ipcContracts.bootstrap, async () => ({
    protocolVersion: 1,
    appVersion,
    platform: 'win32' as const,
    settings: await settings.get()
  }))
  register(ipcContracts.settingsGet, () => settings.get())
  register(ipcContracts.settingsUpdate, (patch) => settings.update(patch))
  register(ipcContracts.hostsList, () => hosts.list())
  register(ipcContracts.hostsCreate, (request) => hosts.create(request))
  register(ipcContracts.hostsUpdate, (request) => hosts.update(request))
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

  const onHostState = (snapshot: unknown): void => {
    if (!window.isDestroyed()) window.webContents.send(IPC_CHANNELS.hostStateEvent, snapshot)
  }
  connections.on('state', onHostState)

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
  }
}

function assertTrustedSender(event: IpcMainInvokeEvent, window: BrowserWindow): void {
  if (event.sender !== window.webContents || event.senderFrame !== window.webContents.mainFrame) {
    throw new Error('IPC sender is not the RemoteDeck main frame')
  }
}
