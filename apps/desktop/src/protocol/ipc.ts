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
import type { terminalEventSchema } from './terminal'
import {
  terminalCreateRequestSchema,
  terminalResizeRequestSchema,
  terminalSessionRequestSchema,
  terminalSessionSchema,
  terminalWriteRequestSchema
} from './terminal'

export const IPC_CHANNELS = {
  bootstrap: 'v1:app.bootstrap',
  appOpenExternal: 'v1:app.openExternal',
  appClipboardRead: 'v1:app.clipboardRead',
  appClipboardWrite: 'v1:app.clipboardWrite',
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
  terminalsList: 'v1:terminals.list',
  terminalsCreate: 'v1:terminals.create',
  terminalsWrite: 'v1:terminals.write',
  terminalsResize: 'v1:terminals.resize',
  terminalsClose: 'v1:terminals.close',
  terminalsReconnect: 'v1:terminals.reconnect',
  hostStateEvent: 'v1:event.hostState',
  terminalEvent: 'v1:event.terminal'
} as const

export const emptyRequestSchema = z.object({}).strict()
export const bootstrapSnapshotSchema = z.object({
  protocolVersion: z.literal(protocolVersion),
  appVersion: z.string(),
  platform: z.literal('win32'),
  settings: appSettingsSchema
})
const externalUrlSchema = z.url().max(16_384).refine((value) => value.startsWith('https://') || value.startsWith('http://'), 'Only HTTP(S) links can be opened')
const clipboardTextSchema = z.string().max(4 * 1024 * 1024)

export const ipcContracts = {
  bootstrap: { channel: IPC_CHANNELS.bootstrap, input: emptyRequestSchema, output: bootstrapSnapshotSchema },
  appOpenExternal: { channel: IPC_CHANNELS.appOpenExternal, input: z.object({ url: externalUrlSchema }).strict(), output: z.object({ opened: z.boolean() }) },
  appClipboardRead: { channel: IPC_CHANNELS.appClipboardRead, input: emptyRequestSchema, output: z.object({ text: clipboardTextSchema }) },
  appClipboardWrite: { channel: IPC_CHANNELS.appClipboardWrite, input: z.object({ text: clipboardTextSchema }).strict(), output: z.object({ written: z.literal(true) }) },
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
  keysVerify: { channel: IPC_CHANNELS.keysVerify, input: keyVerifyRequestSchema, output: keyOperationResultSchema },
  terminalsList: { channel: IPC_CHANNELS.terminalsList, input: emptyRequestSchema, output: z.array(terminalSessionSchema) },
  terminalsCreate: { channel: IPC_CHANNELS.terminalsCreate, input: terminalCreateRequestSchema, output: terminalSessionSchema },
  terminalsWrite: { channel: IPC_CHANNELS.terminalsWrite, input: terminalWriteRequestSchema, output: terminalSessionSchema },
  terminalsResize: { channel: IPC_CHANNELS.terminalsResize, input: terminalResizeRequestSchema, output: terminalSessionSchema },
  terminalsClose: { channel: IPC_CHANNELS.terminalsClose, input: terminalSessionRequestSchema, output: terminalSessionSchema },
  terminalsReconnect: { channel: IPC_CHANNELS.terminalsReconnect, input: terminalSessionRequestSchema, output: terminalSessionSchema }
} as const

export interface RemoteDeckApi {
  app: {
    bootstrap(): Promise<z.infer<typeof bootstrapSnapshotSchema>>
    openExternal(url: string): Promise<z.infer<typeof ipcContracts.appOpenExternal.output>>
    clipboardRead(): Promise<z.infer<typeof ipcContracts.appClipboardRead.output>>
    clipboardWrite(text: string): Promise<z.infer<typeof ipcContracts.appClipboardWrite.output>>
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
  terminals: {
    list(): Promise<z.infer<typeof ipcContracts.terminalsList.output>>
    create(request: z.infer<typeof ipcContracts.terminalsCreate.input>): Promise<z.infer<typeof ipcContracts.terminalsCreate.output>>
    write(sessionId: string, data: string): Promise<z.infer<typeof ipcContracts.terminalsWrite.output>>
    resize(request: z.infer<typeof ipcContracts.terminalsResize.input>): Promise<z.infer<typeof ipcContracts.terminalsResize.output>>
    close(sessionId: string): Promise<z.infer<typeof ipcContracts.terminalsClose.output>>
    reconnect(sessionId: string): Promise<z.infer<typeof ipcContracts.terminalsReconnect.output>>
    onEvent(callback: (event: z.infer<typeof terminalEventSchema>) => void): () => void
  }
}
