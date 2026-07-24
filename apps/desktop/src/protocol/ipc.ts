import { z } from 'zod'
import { hostKeyRecordSchema, protocolVersion } from './domain'
import { appSettingsPatchSchema, appSettingsSchema } from './settings'
import {
  connectionSnapshotSchema,
  hostConnectRequestSchema,
  hostCreateRequestSchema,
  hostDeleteRequestSchema,
  hostIdRequestSchema,
  hostImportRequestSchema,
  hostImportResultSchema,
  hostKeyDecisionSchema,
  hostKeyRemoveSchema,
  hostListItemSchema,
  hostUpdateRequestSchema,
  keyDeployRequestSchema,
  keyGenerateRequestSchema,
  keyGenerateResultSchema,
  keyOperationResultSchema,
  privateKeyMetadataSchema,
  keyVerifyRequestSchema
} from './ssh'
import type { hostStateEventSchema } from './ssh'

export const IPC_CHANNELS = {
  bootstrap: 'v1:app.bootstrap',
  settingsGet: 'v1:settings.get',
  settingsUpdate: 'v1:settings.update',
  hostsList: 'v1:hosts.list',
  hostsCreate: 'v1:hosts.create',
  hostsUpdate: 'v1:hosts.update',
  hostsDelete: 'v1:hosts.delete',
  hostsImport: 'v1:hosts.import',
  hostsTest: 'v1:hosts.test',
  hostsConnect: 'v1:hosts.connect',
  hostsDisconnect: 'v1:hosts.disconnect',
  hostKeysAccept: 'v1:hostKeys.accept',
  hostKeysReject: 'v1:hostKeys.reject',
  hostKeysList: 'v1:hostKeys.list',
  hostKeysRemove: 'v1:hostKeys.remove',
  keysGenerate: 'v1:keys.generate',
  keysPickAndScan: 'v1:keys.pickAndScan',
  keysListScanned: 'v1:keys.listScanned',
  keysDeploy: 'v1:keys.deploy',
  keysVerify: 'v1:keys.verify',
  hostStateEvent: 'v1:event.hostState'
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
  settingsUpdate: { channel: IPC_CHANNELS.settingsUpdate, input: appSettingsPatchSchema, output: appSettingsSchema },
  hostsList: { channel: IPC_CHANNELS.hostsList, input: emptyRequestSchema, output: z.array(hostListItemSchema) },
  hostsCreate: { channel: IPC_CHANNELS.hostsCreate, input: hostCreateRequestSchema, output: hostListItemSchema },
  hostsUpdate: { channel: IPC_CHANNELS.hostsUpdate, input: hostUpdateRequestSchema, output: hostListItemSchema },
  hostsDelete: { channel: IPC_CHANNELS.hostsDelete, input: hostDeleteRequestSchema, output: z.object({ deleted: z.boolean() }) },
  hostsImport: { channel: IPC_CHANNELS.hostsImport, input: hostImportRequestSchema, output: hostImportResultSchema },
  hostsTest: { channel: IPC_CHANNELS.hostsTest, input: hostConnectRequestSchema, output: connectionSnapshotSchema },
  hostsConnect: { channel: IPC_CHANNELS.hostsConnect, input: hostConnectRequestSchema, output: connectionSnapshotSchema },
  hostsDisconnect: { channel: IPC_CHANNELS.hostsDisconnect, input: hostIdRequestSchema, output: connectionSnapshotSchema },
  hostKeysAccept: { channel: IPC_CHANNELS.hostKeysAccept, input: hostKeyDecisionSchema, output: hostKeyRecordSchema },
  hostKeysReject: { channel: IPC_CHANNELS.hostKeysReject, input: hostKeyDecisionSchema, output: z.object({ rejected: z.boolean() }) },
  hostKeysList: { channel: IPC_CHANNELS.hostKeysList, input: emptyRequestSchema, output: z.array(hostKeyRecordSchema) },
  hostKeysRemove: { channel: IPC_CHANNELS.hostKeysRemove, input: hostKeyRemoveSchema, output: z.object({ removed: z.boolean() }) },
  keysGenerate: { channel: IPC_CHANNELS.keysGenerate, input: keyGenerateRequestSchema, output: keyGenerateResultSchema },
  keysPickAndScan: { channel: IPC_CHANNELS.keysPickAndScan, input: emptyRequestSchema, output: z.array(privateKeyMetadataSchema) },
  keysListScanned: { channel: IPC_CHANNELS.keysListScanned, input: emptyRequestSchema, output: z.array(privateKeyMetadataSchema) },
  keysDeploy: { channel: IPC_CHANNELS.keysDeploy, input: keyDeployRequestSchema, output: keyOperationResultSchema },
  keysVerify: { channel: IPC_CHANNELS.keysVerify, input: keyVerifyRequestSchema, output: keyOperationResultSchema }
} as const

export interface RemoteDeckApi {
  app: {
    bootstrap(): Promise<z.infer<typeof bootstrapSnapshotSchema>>
  }
  settings: {
    get(): Promise<z.infer<typeof appSettingsSchema>>
    update(patch: z.infer<typeof appSettingsPatchSchema>): Promise<z.infer<typeof appSettingsSchema>>
  }
  hosts: {
    list(): Promise<z.infer<typeof ipcContracts.hostsList.output>>
    create(request: z.infer<typeof ipcContracts.hostsCreate.input>): Promise<z.infer<typeof ipcContracts.hostsCreate.output>>
    update(request: z.infer<typeof ipcContracts.hostsUpdate.input>): Promise<z.infer<typeof ipcContracts.hostsUpdate.output>>
    delete(request: z.infer<typeof ipcContracts.hostsDelete.input>): Promise<z.infer<typeof ipcContracts.hostsDelete.output>>
    import(request: z.infer<typeof ipcContracts.hostsImport.input>): Promise<z.infer<typeof ipcContracts.hostsImport.output>>
    test(request: z.infer<typeof ipcContracts.hostsTest.input>): Promise<z.infer<typeof ipcContracts.hostsTest.output>>
    connect(request: z.infer<typeof ipcContracts.hostsConnect.input>): Promise<z.infer<typeof ipcContracts.hostsConnect.output>>
    disconnect(hostId: string): Promise<z.infer<typeof ipcContracts.hostsDisconnect.output>>
    onState(callback: (event: z.infer<typeof hostStateEventSchema>) => void): () => void
  }
  hostKeys: {
    accept(candidateId: string): Promise<z.infer<typeof ipcContracts.hostKeysAccept.output>>
    reject(candidateId: string): Promise<z.infer<typeof ipcContracts.hostKeysReject.output>>
    list(): Promise<z.infer<typeof ipcContracts.hostKeysList.output>>
    remove(recordId: string): Promise<z.infer<typeof ipcContracts.hostKeysRemove.output>>
  }
  keys: {
    pickAndScan(): Promise<z.infer<typeof ipcContracts.keysPickAndScan.output>>
    listScanned(): Promise<z.infer<typeof ipcContracts.keysListScanned.output>>
    generate(request: z.infer<typeof ipcContracts.keysGenerate.input>): Promise<z.infer<typeof ipcContracts.keysGenerate.output>>
    deploy(request: z.infer<typeof ipcContracts.keysDeploy.input>): Promise<z.infer<typeof ipcContracts.keysDeploy.output>>
    verify(request: z.infer<typeof ipcContracts.keysVerify.input>): Promise<z.infer<typeof ipcContracts.keysVerify.output>>
  }
}
