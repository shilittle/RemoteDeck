import { z } from 'zod'
import {
  authProfileSchema,
  connectionStateSchema,
  entityIdSchema,
  hostAliasSchema,
  hostProfileSchema,
  nonEmptyTextSchema,
  portSchema,
  sshAdvancedOptionsSchema,
  timestampSchema,
  workspaceProfileSchema
} from './domain'

export const authInputSchema = z.object({
  name: nonEmptyTextSchema,
  method: z.enum(['password', 'keyboard_interactive', 'private_key', 'agent']),
  identityFile: z.string().max(32_767).optional(),
  agent: z.enum(['windows_openssh', 'pageant']).optional()
}).superRefine((value, context) => {
  if (value.method === 'private_key' && !value.identityFile) context.addIssue({ code: 'custom', path: ['identityFile'], message: '请选择私钥文件' })
  if (value.method === 'agent' && !value.agent) context.addIssue({ code: 'custom', path: ['agent'], message: '请选择 SSH agent' })
})

export const hostCreateRequestSchema = z.object({
  alias: hostAliasSchema,
  hostname: nonEmptyTextSchema,
  port: portSchema.default(22),
  username: nonEmptyTextSchema,
  groups: z.array(nonEmptyTextSchema).max(32).default([]),
  jumpHostId: entityIdSchema.optional(),
  workspacePath: z.string().min(1).max(4096).optional(),
  auth: authInputSchema,
  advanced: sshAdvancedOptionsSchema.default({
    connectTimeoutSeconds: 15,
    serverAliveIntervalSeconds: 30,
    serverAliveCountMax: 3,
    tcpKeepAlive: true,
    compression: false,
    identitiesOnly: false
  })
})

export const hostUpdateRequestSchema = z.object({
  id: entityIdSchema,
  patch: hostCreateRequestSchema.partial().extend({ jumpHostId: entityIdSchema.nullable().optional() }).strict()
})

export const hostListItemSchema = z.object({
  host: hostProfileSchema,
  auth: authProfileSchema,
  workspace: workspaceProfileSchema.optional(),
  state: connectionStateSchema,
  lastError: z.string().optional()
})

const connectionCredentialValuesSchema = z.object({
  password: z.string().max(4096).optional(),
  passphrase: z.string().max(4096).optional(),
  keyboardInteractiveAnswers: z.array(z.string().max(4096)).max(16).optional()
}).strict()

export const connectionCredentialsSchema = connectionCredentialValuesSchema.extend({
  jump: connectionCredentialValuesSchema.optional()
}).strict()

export const hostKeyCandidateSchema = z.object({
  id: entityIdSchema,
  hostId: entityIdSchema,
  host: nonEmptyTextSchema,
  port: portSchema,
  algorithm: nonEmptyTextSchema,
  sha256Fingerprint: z.string().startsWith('SHA256:'),
  publicKeyBase64: z.string().min(16),
  previousFingerprint: z.string().optional(),
  mismatch: z.boolean(),
  observedAt: timestampSchema
})

export const connectionSnapshotSchema = z.object({
  hostId: entityIdSchema,
  generation: z.number().int().nonnegative(),
  state: connectionStateSchema,
  capabilities: z.object({ shell: z.boolean(), sftp: z.boolean(), python3: z.boolean(), writableWorkspace: z.boolean() }).optional(),
  hostKeyCandidate: hostKeyCandidateSchema.optional(),
  errorCode: z.string().optional(),
  errorMessage: z.string().optional()
})

export const hostConnectRequestSchema = z.object({ hostId: entityIdSchema, credentials: connectionCredentialsSchema.default({}) })
export const hostIdRequestSchema = z.object({ hostId: entityIdSchema })
export const hostDeleteRequestSchema = z.object({ hostId: entityIdSchema, removeManagedConfig: z.boolean().default(true) })
export const hostImportRequestSchema = z.object({ configPath: z.string().min(1).max(32_767) })
export const hostImportResultSchema = z.object({
  imported: z.array(hostListItemSchema),
  skippedAliases: z.array(z.string()),
  unsupported: z.array(z.object({ alias: z.string(), lines: z.array(z.string()) })),
  duplicate: z.boolean(),
  sourceHash: z.string().regex(/^[a-f0-9]{64}$/)
})

export const hostKeyDecisionSchema = z.object({ candidateId: entityIdSchema })
export const hostKeyRemoveSchema = z.object({ recordId: entityIdSchema })

export const keyGenerateRequestSchema = z.object({
  privateKeyPath: z.string().min(1).max(32_767),
  comment: z.string().max(256).default('RemoteDeck'),
  passphrase: z.string().max(4096).optional()
})
export const keyGenerateResultSchema = z.object({ privateKeyPath: z.string(), publicKeyPath: z.string(), fingerprint: z.string(), aclRestricted: z.boolean() })
export const privateKeyMetadataSchema = z.object({
  path: z.string().min(1).max(32_767),
  publicKeyPath: z.string().min(1).max(32_767).optional(),
  format: z.enum(['openssh', 'pem', 'putty', 'unknown']),
  encrypted: z.boolean(),
  algorithm: z.string().optional(),
  fingerprint: z.string().startsWith('SHA256:').optional(),
  comment: z.string().max(4096).optional(),
  sizeBytes: z.number().int().nonnegative(),
  modifiedAt: timestampSchema,
  scannedAt: timestampSchema
})
export const keyDeployRequestSchema = z.object({ hostId: entityIdSchema, privateKeyPath: z.string().min(1).max(32_767), passphrase: z.string().max(4096).optional(), makeDefault: z.boolean().default(false) })
export const keyVerifyRequestSchema = keyDeployRequestSchema.omit({ makeDefault: true })
export const keyOperationResultSchema = z.object({ success: z.boolean(), fingerprint: z.string().optional(), alreadyPresent: z.boolean().optional(), verified: z.boolean().optional(), message: z.string() })

export const hostStateEventSchema = connectionSnapshotSchema

export type HostCreateRequest = z.infer<typeof hostCreateRequestSchema>
export type HostUpdateRequest = z.infer<typeof hostUpdateRequestSchema>
export type HostListItem = z.infer<typeof hostListItemSchema>
export type ConnectionCredentials = z.infer<typeof connectionCredentialsSchema>
export type ConnectionSnapshot = z.infer<typeof connectionSnapshotSchema>
export type HostKeyCandidate = z.infer<typeof hostKeyCandidateSchema>
export type HostImportResult = z.infer<typeof hostImportResultSchema>
export type KeyGenerateRequest = z.infer<typeof keyGenerateRequestSchema>
export type KeyDeployRequest = z.infer<typeof keyDeployRequestSchema>
export type PrivateKeyMetadata = z.infer<typeof privateKeyMetadataSchema>
