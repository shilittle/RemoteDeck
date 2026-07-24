import { contextBridge, ipcRenderer, webUtils } from 'electron'
import type { IpcRendererEvent } from 'electron'
import type { RemoteDeckApi } from '../protocol/ipc'
import type { AppSettingsPatch } from '../protocol/settings'
import type { HostCreateRequest, HostUpdateRequest, KeyDeployRequest, KeyGenerateRequest } from '../protocol/ssh'
import { hostStateEventSchema } from '../protocol/ssh'
import { terminalEventSchema } from '../protocol/terminal'
import { transferEventSchema } from '../protocol/sftp'
import { tunnelEventSchema } from '../protocol/tunnel'
import { telemetryEventSchema } from '../protocol/telemetry'
import { commandEventSchema } from '../protocol/command'
import { IPC_CHANNELS } from '../protocol/ipc'
import { ipcContracts } from '../protocol/ipc'

async function invoke<TOutput>(
  contract: { channel: string; input: { parse(value: unknown): unknown }; output: { parse(value: unknown): TOutput } },
  input: unknown
): Promise<TOutput> {
  const request = contract.input.parse(input)
  const response: unknown = await ipcRenderer.invoke(contract.channel, request)
  return contract.output.parse(response)
}

const api: RemoteDeckApi = Object.freeze({
  app: Object.freeze({
    bootstrap: () => invoke(ipcContracts.bootstrap, {}),
    openExternal: (url: string) => invoke(ipcContracts.appOpenExternal, { url }),
    clipboardRead: () => invoke(ipcContracts.appClipboardRead, {}),
    clipboardWrite: (text: string) => invoke(ipcContracts.appClipboardWrite, { text })
  }),
  settings: Object.freeze({
    get: () => invoke(ipcContracts.settingsGet, {}),
    update: (patch: AppSettingsPatch) => invoke(ipcContracts.settingsUpdate, patch)
  }),
  hosts: Object.freeze({
    list: () => invoke(ipcContracts.hostsList, {}),
    create: (request: HostCreateRequest) => invoke(ipcContracts.hostsCreate, request),
    update: (request: HostUpdateRequest) => invoke(ipcContracts.hostsUpdate, request),
    delete: (request: { hostId: string; removeManagedConfig?: boolean }) => invoke(ipcContracts.hostsDelete, request),
    import: (request: { configPath: string }) => invoke(ipcContracts.hostsImport, request),
    test: (request: Parameters<RemoteDeckApi['hosts']['test']>[0]) => invoke(ipcContracts.hostsTest, request),
    connect: (request: Parameters<RemoteDeckApi['hosts']['connect']>[0]) => invoke(ipcContracts.hostsConnect, request),
    disconnect: (hostId: string) => invoke(ipcContracts.hostsDisconnect, { hostId }),
    onState: (callback: Parameters<RemoteDeckApi['hosts']['onState']>[0]) => {
      const listener = (_event: IpcRendererEvent, raw: unknown): void => callback(hostStateEventSchema.parse(raw))
      ipcRenderer.on(IPC_CHANNELS.hostStateEvent, listener)
      return () => ipcRenderer.removeListener(IPC_CHANNELS.hostStateEvent, listener)
    }
  }),
  hostKeys: Object.freeze({
    accept: (candidateId: string) => invoke(ipcContracts.hostKeysAccept, { candidateId }),
    reject: (candidateId: string) => invoke(ipcContracts.hostKeysReject, { candidateId }),
    list: () => invoke(ipcContracts.hostKeysList, {}),
    remove: (recordId: string) => invoke(ipcContracts.hostKeysRemove, { recordId })
  }),
  keys: Object.freeze({
    pickAndScan: () => invoke(ipcContracts.keysPickAndScan, {}),
    listScanned: () => invoke(ipcContracts.keysListScanned, {}),
    generate: (request: KeyGenerateRequest) => invoke(ipcContracts.keysGenerate, request),
    deploy: (request: KeyDeployRequest) => invoke(ipcContracts.keysDeploy, request),
    verify: (request: Omit<KeyDeployRequest, 'makeDefault'>) => invoke(ipcContracts.keysVerify, request)
  }),
  terminals: Object.freeze({
    list: () => invoke(ipcContracts.terminalsList, {}),
    create: (request: Parameters<RemoteDeckApi['terminals']['create']>[0]) => invoke(ipcContracts.terminalsCreate, request),
    write: (sessionId: string, data: string) => invoke(ipcContracts.terminalsWrite, { sessionId, data }),
    resize: (request: Parameters<RemoteDeckApi['terminals']['resize']>[0]) => invoke(ipcContracts.terminalsResize, request),
    close: (sessionId: string) => invoke(ipcContracts.terminalsClose, { sessionId }),
    reconnect: (sessionId: string) => invoke(ipcContracts.terminalsReconnect, { sessionId }),
    onEvent: (callback: Parameters<RemoteDeckApi['terminals']['onEvent']>[0]) => {
      const listener = (_event: IpcRendererEvent, raw: unknown): void => callback(terminalEventSchema.parse(raw))
      ipcRenderer.on(IPC_CHANNELS.terminalEvent, listener)
      return () => ipcRenderer.removeListener(IPC_CHANNELS.terminalEvent, listener)
    }
  }),
  sftp: Object.freeze({
    list: (request: Parameters<RemoteDeckApi['sftp']['list']>[0]) => invoke(ipcContracts.sftpList, request),
    create: (request: Parameters<RemoteDeckApi['sftp']['create']>[0]) => invoke(ipcContracts.sftpCreate, request),
    rename: (request: Parameters<RemoteDeckApi['sftp']['rename']>[0]) => invoke(ipcContracts.sftpRename, request),
    delete: (request: Parameters<RemoteDeckApi['sftp']['delete']>[0]) => invoke(ipcContracts.sftpDelete, request),
    pickUpload: (kind: 'files' | 'directory') => invoke(ipcContracts.sftpPickUpload, { kind }),
    pickDownloadDirectory: () => invoke(ipcContracts.sftpPickDownloadDirectory, {}),
    droppedPath: (file: unknown) => webUtils.getPathForFile(file as Parameters<typeof webUtils.getPathForFile>[0]),
    transfers: Object.freeze({
      list: () => invoke(ipcContracts.transfersList, {}),
      upload: (request: Parameters<RemoteDeckApi['sftp']['transfers']['upload']>[0]) => invoke(ipcContracts.transfersUpload, request),
      download: (request: Parameters<RemoteDeckApi['sftp']['transfers']['download']>[0]) => invoke(ipcContracts.transfersDownload, request),
      cancel: (jobId: string) => invoke(ipcContracts.transfersCancel, { jobId }),
      retry: (jobId: string) => invoke(ipcContracts.transfersRetry, { jobId }),
      showInFolder: (jobId: string) => invoke(ipcContracts.transfersShowInFolder, { jobId }),
      onEvent: (callback: Parameters<RemoteDeckApi['sftp']['transfers']['onEvent']>[0]) => {
        const listener = (_event: IpcRendererEvent, raw: unknown): void => callback(transferEventSchema.parse(raw))
        ipcRenderer.on(IPC_CHANNELS.transferEvent, listener)
        return () => ipcRenderer.removeListener(IPC_CHANNELS.transferEvent, listener)
      }
    })
  }),
  tunnels: Object.freeze({
    list: (hostId?: string) => invoke(ipcContracts.tunnelsList, hostId ? { hostId } : {}),
    create: (request: Parameters<RemoteDeckApi['tunnels']['create']>[0]) => invoke(ipcContracts.tunnelsCreate, request),
    update: (request: Parameters<RemoteDeckApi['tunnels']['update']>[0]) => invoke(ipcContracts.tunnelsUpdate, request),
    delete: (tunnelId: string) => invoke(ipcContracts.tunnelsDelete, { tunnelId }),
    start: (request: Parameters<RemoteDeckApi['tunnels']['start']>[0]) => invoke(ipcContracts.tunnelsStart, request),
    stop: (tunnelId: string) => invoke(ipcContracts.tunnelsStop, { tunnelId }),
    restart: (request: Parameters<RemoteDeckApi['tunnels']['restart']>[0]) => invoke(ipcContracts.tunnelsRestart, request),
    detectClash: () => invoke(ipcContracts.tunnelsDetectClash, {}),
    onEvent: (callback: Parameters<RemoteDeckApi['tunnels']['onEvent']>[0]) => {
      const listener = (_event: IpcRendererEvent, raw: unknown): void => callback(tunnelEventSchema.parse(raw))
      ipcRenderer.on(IPC_CHANNELS.tunnelEvent, listener)
      return () => ipcRenderer.removeListener(IPC_CHANNELS.tunnelEvent, listener)
    }
  }),
  telemetry: Object.freeze({
    list: () => invoke(ipcContracts.telemetryList, {}),
    history: (hostId: string) => invoke(ipcContracts.telemetryHistory, { hostId }),
    start: (hostId: string) => invoke(ipcContracts.telemetryStart, { hostId }),
    stop: (hostId: string) => invoke(ipcContracts.telemetryStop, { hostId }),
    signal: (request: Parameters<RemoteDeckApi['telemetry']['signal']>[0]) => invoke(ipcContracts.telemetrySignal, request),
    btop: Object.freeze({
      probe: (hostId: string) => invoke(ipcContracts.btopProbe, { hostId }),
      startWatchdog: (request: Parameters<RemoteDeckApi['telemetry']['btop']['startWatchdog']>[0]) => invoke(ipcContracts.btopWatchdogStart, request),
      stopWatchdog: (hostId: string) => invoke(ipcContracts.btopWatchdogStop, { hostId })
    }),
    onEvent: (callback: Parameters<RemoteDeckApi['telemetry']['onEvent']>[0]) => {
      const listener = (_event: IpcRendererEvent, raw: unknown): void => callback(telemetryEventSchema.parse(raw))
      ipcRenderer.on(IPC_CHANNELS.telemetryEvent, listener)
      return () => ipcRenderer.removeListener(IPC_CHANNELS.telemetryEvent, listener)
    }
  }),
  commands: Object.freeze({
    list: (hostId: string) => invoke(ipcContracts.commandsList, { hostId }),
    create: (request: Parameters<RemoteDeckApi['commands']['create']>[0]) => invoke(ipcContracts.commandsCreate, request),
    update: (request: Parameters<RemoteDeckApi['commands']['update']>[0]) => invoke(ipcContracts.commandsUpdate, request),
    delete: (presetId: string) => invoke(ipcContracts.commandsDelete, { presetId }),
    analyze: (hostId: string, presetId: string) => invoke(ipcContracts.commandsAnalyze, { hostId, presetId }),
    run: (request: Parameters<RemoteDeckApi['commands']['run']>[0]) => invoke(ipcContracts.commandsRun, request),
    jobs: () => invoke(ipcContracts.commandsJobs, {}),
    cancel: (jobId: string) => invoke(ipcContracts.commandsCancel, { jobId }),
    onEvent: (callback: Parameters<RemoteDeckApi['commands']['onEvent']>[0]) => {
      const listener = (_event: IpcRendererEvent, raw: unknown): void => callback(commandEventSchema.parse(raw))
      ipcRenderer.on(IPC_CHANNELS.commandEvent, listener)
      return () => ipcRenderer.removeListener(IPC_CHANNELS.commandEvent, listener)
    }
  }),
  codex: Object.freeze({
    installPlan: () => invoke(ipcContracts.codexInstallPlan, {}),
    probe: (hostId: string) => invoke(ipcContracts.codexProbe, { hostId }),
    action: (request: Parameters<RemoteDeckApi['codex']['action']>[0]) => invoke(ipcContracts.codexAction, request)
  }),
  legacy: Object.freeze({
    pick: () => invoke(ipcContracts.legacyPick, {}),
    preview: (sourcePath: string) => invoke(ipcContracts.legacyPreview, { sourcePath }),
    apply: (request: Parameters<RemoteDeckApi['legacy']['apply']>[0]) => invoke(ipcContracts.legacyApply, request)
  })
})

contextBridge.exposeInMainWorld('remoteDeck', api)
