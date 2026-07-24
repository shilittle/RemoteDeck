import { contextBridge, ipcRenderer } from 'electron'
import type { RemoteDeckApi } from '../protocol/ipc'
import type { AppSettingsPatch } from '../protocol/settings'
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
    bootstrap: () => invoke(ipcContracts.bootstrap, {})
  }),
  settings: Object.freeze({
    get: () => invoke(ipcContracts.settingsGet, {}),
    update: (patch: AppSettingsPatch) => invoke(ipcContracts.settingsUpdate, patch)
  })
})

contextBridge.exposeInMainWorld('remoteDeck', api)
