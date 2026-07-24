import { z } from 'zod'

export const protocolVersion = 1 as const
export const entityIdSchema = z.uuid()
export const timestampSchema = z.iso.datetime()
export const nonEmptyTextSchema = z.string().trim().min(1).max(512)
export const hostAliasSchema = z.string().trim().min(1).max(64).regex(/^[A-Za-z0-9_.-]+$/)
export const portSchema = z.number().int().min(1).max(65_535)

export const connectionStateSchema = z.enum([
  'idle',
  'resolving',
  'connecting',
  'awaiting_host_key',
  'authenticating',
  'online',
  'degraded',
  'reconnecting',
  'offline',
  'failed'
])

export const tunnelStateSchema = z.enum([
  'stopped',
  'starting',
  'healthy',
  'degraded',
  'recovering',
  'failed'
])

export const authProfileSchema = z.object({
  schemaVersion: z.literal(1),
  id: entityIdSchema,
  name: nonEmptyTextSchema,
  method: z.enum(['password', 'private_key', 'agent']),
  identityFile: z.string().max(32_767).optional(),
  agent: z.enum(['windows_openssh', 'pageant']).optional(),
  createdAt: timestampSchema,
  updatedAt: timestampSchema
}).superRefine((value, context) => {
  if (value.method === 'private_key' && !value.identityFile) {
    context.addIssue({ code: 'custom', path: ['identityFile'], message: 'Private-key authentication requires a key path' })
  }
  if (value.method === 'agent' && !value.agent) {
    context.addIssue({ code: 'custom', path: ['agent'], message: 'Agent authentication requires an agent type' })
  }
})

export const sshAdvancedOptionsSchema = z.object({
  connectTimeoutSeconds: z.number().int().min(1).max(300).default(15),
  serverAliveIntervalSeconds: z.number().int().min(0).max(3600).default(30),
  serverAliveCountMax: z.number().int().min(1).max(100).default(3),
  tcpKeepAlive: z.boolean().default(true),
  compression: z.boolean().default(false),
  identitiesOnly: z.boolean().default(false)
})

export const workspaceProfileSchema = z.object({
  schemaVersion: z.literal(1),
  id: entityIdSchema,
  hostId: entityIdSchema,
  name: nonEmptyTextSchema,
  remotePath: z.string().min(1).max(4096),
  createdAt: timestampSchema,
  updatedAt: timestampSchema
})

export const hostProfileSchema = z.object({
  schemaVersion: z.literal(1),
  id: entityIdSchema,
  alias: hostAliasSchema,
  hostname: nonEmptyTextSchema,
  port: portSchema.default(22),
  username: nonEmptyTextSchema,
  authProfileId: entityIdSchema,
  jumpHostId: entityIdSchema.optional(),
  defaultWorkspaceId: entityIdSchema.optional(),
  groups: z.array(nonEmptyTextSchema).max(32).default([]),
  advanced: sshAdvancedOptionsSchema.default({
    connectTimeoutSeconds: 15,
    serverAliveIntervalSeconds: 30,
    serverAliveCountMax: 3,
    tcpKeepAlive: true,
    compression: false,
    identitiesOnly: false
  }),
  monitorEnabled: z.boolean().default(true),
  createdAt: timestampSchema,
  updatedAt: timestampSchema
})

export const hostKeyRecordSchema = z.object({
  schemaVersion: z.literal(1),
  id: entityIdSchema,
  host: nonEmptyTextSchema,
  port: portSchema,
  algorithm: nonEmptyTextSchema,
  sha256Fingerprint: z.string().regex(/^SHA256:[A-Za-z0-9+/]+={0,2}$/),
  publicKeyBase64: z.string().min(16),
  acceptedAt: timestampSchema
})

export const tunnelHealthCheckSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('tcp'), intervalSeconds: z.number().int().min(2).max(3600), timeoutMs: z.number().int().min(100).max(60_000) }),
  z.object({ type: z.literal('http'), intervalSeconds: z.number().int().min(2).max(3600), timeoutMs: z.number().int().min(100).max(60_000), path: z.string().startsWith('/').max(2048), expectedStatus: z.number().int().min(100).max(599) })
])

export const tunnelProfileSchema = z.object({
  schemaVersion: z.literal(1),
  id: entityIdSchema,
  hostId: entityIdSchema,
  name: nonEmptyTextSchema,
  direction: z.enum(['local', 'remote']),
  bindAddress: nonEmptyTextSchema,
  sourcePort: portSchema,
  targetHost: nonEmptyTextSchema,
  targetPort: portSchema,
  autoStart: z.boolean().default(false),
  healthCheck: tunnelHealthCheckSchema.optional(),
  legacyCleanupHook: z.object({ command: z.string().min(1).max(16_384), authorized: z.boolean().default(false) }).optional(),
  createdAt: timestampSchema,
  updatedAt: timestampSchema
})

export const commandPresetSchema = z.object({
  schemaVersion: z.literal(1),
  id: entityIdSchema,
  hostId: entityIdSchema.optional(),
  name: nonEmptyTextSchema,
  description: z.string().max(2048).default(''),
  group: z.string().max(128).default(''),
  command: z.string().min(1).max(32_768),
  workingDirectory: z.string().max(4096).optional(),
  risk: z.enum(['L0', 'L1', 'L2']),
  requiresPty: z.boolean().default(false),
  requiresSudo: z.boolean().default(false),
  confirmationText: z.string().max(256).optional(),
  sortOrder: z.number().int().default(0),
  createdAt: timestampSchema,
  updatedAt: timestampSchema
})

export const transferJobSchema = z.object({
  schemaVersion: z.literal(1),
  id: entityIdSchema,
  hostId: entityIdSchema,
  direction: z.enum(['upload', 'download']),
  source: z.string().min(1).max(32_767),
  destination: z.string().min(1).max(32_767),
  state: z.enum(['queued', 'running', 'completed', 'cancelled', 'failed']),
  conflictPolicy: z.enum(['skip', 'overwrite', 'rename']),
  bytesTransferred: z.number().int().nonnegative(),
  totalBytes: z.number().int().nonnegative().nullable(),
  speedBytesPerSecond: z.number().nonnegative(),
  createdAt: timestampSchema,
  updatedAt: timestampSchema,
  error: z.string().max(4096).optional()
})

const cpuSchema = z.object({
  totalPercent: z.number().min(0).max(100),
  perCorePercent: z.array(z.number().min(0).max(100)),
  loadAverage: z.tuple([z.number(), z.number(), z.number()]),
  temperatureC: z.number().nullable()
})

export const telemetrySnapshotSchema = z.object({
  schemaVersion: z.literal(1),
  hostId: entityIdSchema,
  capturedAt: timestampSchema,
  hostname: nonEmptyTextSchema,
  cpu: cpuSchema,
  memory: z.object({ totalBytes: z.number().int().nonnegative(), usedBytes: z.number().int().nonnegative(), swapTotalBytes: z.number().int().nonnegative(), swapUsedBytes: z.number().int().nonnegative() }),
  network: z.object({ receivedBytes: z.number().int().nonnegative(), sentBytes: z.number().int().nonnegative(), receiveBytesPerSecond: z.number().nonnegative(), sendBytesPerSecond: z.number().nonnegative() }),
  disks: z.array(z.object({ mount: z.string(), totalBytes: z.number().int().nonnegative(), usedBytes: z.number().int().nonnegative(), availableBytes: z.number().int().nonnegative() })),
  processes: z.array(z.object({ pid: z.number().int().positive(), ppid: z.number().int().nonnegative(), user: z.string(), cpuPercent: z.number().nonnegative(), memoryPercent: z.number().nonnegative(), state: z.string(), elapsed: z.string(), command: z.string() })),
  gpus: z.array(z.object({ index: z.number().int().nonnegative(), name: z.string(), utilizationPercent: z.number().min(0).max(100), memoryUsedMiB: z.number().nonnegative(), memoryTotalMiB: z.number().nonnegative(), temperatureC: z.number().nullable(), powerW: z.number().nullable() })),
  gpuProcesses: z.array(z.object({ gpuIndex: z.number().int().nonnegative(), pid: z.number().int().positive(), memoryUsedMiB: z.number().nonnegative() })),
  uptimeSeconds: z.number().int().nonnegative()
})

export const codexEnvironmentStatusSchema = z.object({
  schemaVersion: z.literal(1),
  hostId: entityIdSchema,
  installed: z.boolean(),
  version: z.string().nullable(),
  login: z.enum(['unknown', 'logged_out', 'logged_in']),
  tmuxInstalled: z.boolean(),
  workspacePath: z.string().nullable(),
  gitBranch: z.string().nullable(),
  gitDirty: z.boolean().nullable(),
  probedAt: timestampSchema
})

export const legacyImportResultSchema = z.object({
  schemaVersion: z.literal(1),
  sourceHash: z.string().regex(/^[a-f0-9]{64}$/),
  hostCount: z.number().int().nonnegative(),
  tunnelCount: z.number().int().nonnegative(),
  presetCount: z.number().int().nonnegative(),
  warnings: z.array(z.string()),
  importedAt: timestampSchema,
  duplicate: z.boolean()
})

export type HostProfile = z.infer<typeof hostProfileSchema>
export type AuthProfile = z.infer<typeof authProfileSchema>
export type HostKeyRecord = z.infer<typeof hostKeyRecordSchema>
export type WorkspaceProfile = z.infer<typeof workspaceProfileSchema>
export type SshAdvancedOptions = z.infer<typeof sshAdvancedOptionsSchema>
export type TunnelProfile = z.infer<typeof tunnelProfileSchema>
export type TunnelHealthCheck = z.infer<typeof tunnelHealthCheckSchema>
export type CommandPreset = z.infer<typeof commandPresetSchema>
export type TransferJob = z.infer<typeof transferJobSchema>
export type TelemetrySnapshot = z.infer<typeof telemetrySnapshotSchema>
export type CodexEnvironmentStatus = z.infer<typeof codexEnvironmentStatusSchema>
export type LegacyImportResult = z.infer<typeof legacyImportResultSchema>
export type ConnectionState = z.infer<typeof connectionStateSchema>
export type TunnelState = z.infer<typeof tunnelStateSchema>
