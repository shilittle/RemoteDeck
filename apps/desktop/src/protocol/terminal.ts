import { z } from 'zod'
import { entityIdSchema, nonEmptyTextSchema } from './domain'

export const terminalStateSchema = z.enum(['opening', 'online', 'offline', 'failed', 'closed'])
export const terminalDimensionsSchema = z.object({ cols: z.number().int().min(2).max(500), rows: z.number().int().min(1).max(300) })

export const terminalCreateRequestSchema = terminalDimensionsSchema.extend({
  hostId: entityIdSchema,
  cwd: z.string().min(1).max(4096).optional()
}).strict()

export const terminalSessionRequestSchema = z.object({ sessionId: entityIdSchema }).strict()
export const terminalResizeRequestSchema = terminalSessionRequestSchema.extend(terminalDimensionsSchema.shape).strict()
export const terminalWriteRequestSchema = terminalSessionRequestSchema.extend({ data: z.string().max(65_536) }).strict()

export const terminalSessionSchema = terminalDimensionsSchema.extend({
  id: entityIdSchema,
  hostId: entityIdSchema,
  hostAlias: nonEmptyTextSchema,
  cwd: z.string().min(1).max(4096),
  generation: z.number().int().positive(),
  state: terminalStateSchema,
  openedAt: z.iso.datetime(),
  error: z.string().max(4096).optional(),
  exitCode: z.number().int().nullable().optional(),
  exitSignal: z.string().nullable().optional()
})

export const terminalEventSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('data'), sessionId: entityIdSchema, generation: z.number().int().positive(), data: z.string().max(1_048_576) }),
  z.object({ type: z.literal('state'), session: terminalSessionSchema })
])

export type TerminalCreateRequest = z.infer<typeof terminalCreateRequestSchema>
export type TerminalResizeRequest = z.infer<typeof terminalResizeRequestSchema>
export type TerminalSession = z.infer<typeof terminalSessionSchema>
export type TerminalEvent = z.infer<typeof terminalEventSchema>
