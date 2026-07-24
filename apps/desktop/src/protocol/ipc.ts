import { z } from 'zod'
import { protocolVersion } from './domain'
import { appSettingsPatchSchema, appSettingsSchema } from './settings'

export const IPC_CHANNELS = {
  bootstrap: 'v1:app.bootstrap',
  settingsGet: 'v1:settings.get',
  settingsUpdate: 'v1:settings.update'
} as const

export const emptyRequestSchema = z.object({}).strict()
export const bootstrapSnapshotSchema = z.object({
  protocolVersion: z.literal(protocolVersion),
  appVersion: z.string(),
  platform: z.literal('win32'),
  settings: appSettingsSchema
})

export const ipcContracts = {
  bootstrap: { channel: IPC_CHANNELS.bootstrap, input: emptyRequestSchema, output: bootstrapSnapshotSchema },
  settingsGet: { channel: IPC_CHANNELS.settingsGet, input: emptyRequestSchema, output: appSettingsSchema },
  settingsUpdate: { channel: IPC_CHANNELS.settingsUpdate, input: appSettingsPatchSchema, output: appSettingsSchema }
} as const

export interface RemoteDeckApi {
  app: {
    bootstrap(): Promise<z.infer<typeof bootstrapSnapshotSchema>>
  }
  settings: {
    get(): Promise<z.infer<typeof appSettingsSchema>>
    update(patch: z.infer<typeof appSettingsPatchSchema>): Promise<z.infer<typeof appSettingsSchema>>
  }
}

