import { z } from 'zod'
import { entityIdSchema, telemetrySnapshotSchema } from './domain'

export const collectorV1PayloadSchema = telemetrySnapshotSchema.omit({ hostId: true }).strict()

export const telemetryStateSchema = z.enum(['stopped', 'starting', 'online', 'recovering', 'dependency_missing', 'failed'])
export const telemetryStatusSchema = z.object({
  hostId: entityIdSchema,
  state: telemetryStateSchema,
  restartCount: z.number().int().nonnegative(),
  invalidLineCount: z.number().int().nonnegative(),
  sampleCount: z.number().int().nonnegative(),
  startedAt: z.iso.datetime().optional(),
  nextRetryAt: z.iso.datetime().optional(),
  lastSampleAt: z.iso.datetime().optional(),
  lastError: z.string().max(4096).optional()
})
export const telemetryStartRequestSchema = z.object({ hostId: entityIdSchema }).strict()
export const telemetryHostRequestSchema = telemetryStartRequestSchema
export const telemetrySignalRequestSchema = z.object({
  hostId: entityIdSchema,
  pid: z.number().int().positive(),
  signal: z.enum(['TERM', 'KILL']),
  expectedUser: z.string().min(1).max(256),
  expectedCommand: z.string().min(1).max(32_768),
  confirmKill: z.boolean().default(false)
}).strict()
export const telemetrySignalResultSchema = z.object({ delivered: z.literal(true), signal: z.enum(['TERM', 'KILL']), pid: z.number().int().positive() })

export const btopStatusSchema = z.object({
  hostId: entityIdSchema,
  installed: z.boolean(),
  version: z.string().nullable(),
  watchdogState: z.enum(['stopped', 'starting', 'running', 'recovering', 'unavailable']),
  restartCount: z.number().int().nonnegative(),
  rotationMinutes: z.number().int().min(1).max(1440),
  lastError: z.string().max(4096).optional()
})
export const btopWatchdogStartRequestSchema = z.object({ hostId: entityIdSchema, rotationMinutes: z.number().int().min(1).max(1440).default(60) }).strict()

export const telemetryEventSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('status'), status: telemetryStatusSchema }),
  z.object({ type: z.literal('snapshot'), snapshot: telemetrySnapshotSchema }),
  z.object({ type: z.literal('btop'), status: btopStatusSchema })
])

export type CollectorV1Payload = z.infer<typeof collectorV1PayloadSchema>
export type TelemetryStatus = z.infer<typeof telemetryStatusSchema>
export type TelemetrySignalRequest = z.infer<typeof telemetrySignalRequestSchema>
export type BtopStatus = z.infer<typeof btopStatusSchema>
export type TelemetryEvent = z.infer<typeof telemetryEventSchema>
