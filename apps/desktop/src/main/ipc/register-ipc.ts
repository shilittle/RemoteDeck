import type { BrowserWindow, IpcMain, IpcMainInvokeEvent } from 'electron'
import type { Logger } from 'pino'
import type { ZodType } from 'zod'
import { ipcContracts } from '../../protocol/ipc'
import type { SettingsService } from '../../core/settings-service'

interface IpcDependencies {
  ipcMain: IpcMain
  window: BrowserWindow
  settings: SettingsService
  logger: Logger
  appVersion: string
}

export function registerIpcHandlers(dependencies: IpcDependencies): () => void {
  const { ipcMain, window, settings, logger, appVersion } = dependencies
  const channels: string[] = []

  register(ipcContracts.bootstrap, async () => ({
    protocolVersion: 1,
    appVersion,
    platform: 'win32' as const,
    settings: await settings.get()
  }))
  register(ipcContracts.settingsGet, () => settings.get())
  register(ipcContracts.settingsUpdate, (patch) => settings.update(patch))

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
  }
}

function assertTrustedSender(event: IpcMainInvokeEvent, window: BrowserWindow): void {
  if (event.sender !== window.webContents || event.senderFrame !== window.webContents.mainFrame) {
    throw new Error('IPC sender is not the RemoteDeck main frame')
  }
}

