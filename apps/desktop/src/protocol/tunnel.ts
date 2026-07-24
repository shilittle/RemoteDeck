import { z } from 'zod'
import { entityIdSchema, nonEmptyTextSchema, portSchema, tunnelHealthCheckSchema, tunnelProfileSchema } from './domain'
import { connectionCredentialsSchema } from './ssh'

const legacyCleanupHookSchema = z.object({ command: z.string().min(1).max(16_384), authorized: z.boolean() })
const tunnelInputSchema = z.object({
  hostId: entityIdSchema,
  name: nonEmptyTextSchema,
  direction: z.enum(['local', 'remote']),
  bindAddress: nonEmptyTextSchema,
  sourcePort: portSchema,
  targetHost: nonEmptyTextSchema,
  targetPort: portSchema,
  autoStart: z.boolean().default(false),
  healthCheck: tunnelHealthCheckSchema.optional(),
  legacyCleanupHook: legacyCleanupHookSchema.optional()
}).strict()

export const tunnelCreateRequestSchema = tunnelInputSchema
export const tunnelUpdateRequestSchema = z.object({
  tunnelId: entityIdSchema,
  patch: tunnelInputSchema.omit({ hostId: true, healthCheck: true, legacyCleanupHook: true }).partial().extend({
    healthCheck: tunnelHealthCheckSchema.nullable().optional(),
    legacyCleanupHook: legacyCleanupHookSchema.nullable().optional()
  }).strict()
}).strict()
export const tunnelIdRequestSchema = z.object({ tunnelId: entityIdSchema }).strict()
export const tunnelStartRequestSchema = tunnelIdRequestSchema.extend({ credentials: connectionCredentialsSchema.default({}) }).strict()
export const tunnelListRequestSchema = z.object({ hostId: entityIdSchema.optional() }).strict()

export const tunnelRuntimeStateSchema = z.enum(['stopped', 'starting', 'online', 'waiting', 'failed'])
export const tunnelSnapshotSchema = z.object({
  profile: tunnelProfileSchema,
  state: tunnelRuntimeStateSchema,
  health: z.enum(['unknown', 'checking', 'healthy', 'unhealthy']),
  startedAt: z.iso.datetime().optional(),
  uptimeSeconds: z.number().int().nonnegative(),
  reconnectCount: z.number().int().nonnegative(),
  nextRetryAt: z.iso.datetime().optional(),
  lastError: z.string().max(4096).optional(),
  logs: z.array(z.object({ at: z.iso.datetime(), level: z.enum(['info', 'warn', 'error']), message: z.string().max(4096) })).max(200)
})
export const tunnelEventSchema = z.object({ snapshot: tunnelSnapshotSchema })

export const clashCandidateSchema = z.object({ processName: z.string(), pid: z.number().int().positive(), address: z.string(), port: portSchema, protocol: z.enum(['http-connect', 'socks5', 'tcp']), confidence: z.enum(['high', 'medium', 'low']), detail: z.string() })

export type TunnelCreateRequest = z.infer<typeof tunnelCreateRequestSchema>
export type TunnelUpdateRequest = z.infer<typeof tunnelUpdateRequestSchema>
export type TunnelSnapshot = z.infer<typeof tunnelSnapshotSchema>
export type ClashCandidate = z.infer<typeof clashCandidateSchema>
