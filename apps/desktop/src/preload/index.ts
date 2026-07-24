import { contextBridge, ipcRenderer } from 'electron'
import type { IpcRendererEvent } from 'electron'
import type { RemoteDeckApi } from '../protocol/ipc'
import type { AppSettingsPatch } from '../protocol/settings'
import type { HostCreateRequest, HostUpdateRequest, KeyDeployRequest, KeyGenerateRequest } from '../protocol/ssh'
import { hostStateEventSchema } from '../protocol/ssh'
import { terminalEventSchema } from '../protocol/terminal'
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
  })
})

contextBridge.exposeInMainWorld('remoteDeck', api)
