import { z } from 'zod'
import { codexEnvironmentStatusSchema, commandPresetSchema, entityIdSchema, hostAliasSchema } from './domain'
import { terminalSessionSchema } from './terminal'

export const commandRiskSchema = z.enum(['L0', 'L1', 'L2'])
export const commandPresetInputSchema = commandPresetSchema.omit({ schemaVersion: true, id: true, createdAt: true, updatedAt: true }).strict()
export const commandPresetUpdateSchema = z.object({
  id: entityIdSchema,
  patch: commandPresetInputSchema.partial().strict()
}).strict()
export const commandPresetListRequestSchema = z.object({ hostId: entityIdSchema }).strict()
export const commandPresetIdRequestSchema = z.object({ presetId: entityIdSchema }).strict()
export const commandDefinitionSchema = commandPresetSchema.extend({
  builtin: z.boolean(),
  available: z.boolean(),
  unavailableReason: z.string().max(2048).optional()
})

export const commandAnalysisSchema = z.object({
  declaredRisk: commandRiskSchema,
  detectedRisk: commandRiskSchema,
  effectiveRisk: commandRiskSchema,
  reasons: z.array(z.string().max(1024)).max(32),
  requiredConfirmation: z.string().max(256).nullable(),
  displayCommand: z.string().max(32_768).optional(),
  targetAlias: hostAliasSchema.optional(),
  workingDirectory: z.string().max(4096).optional()
})

export const commandRunRequestSchema = z.object({
  hostId: entityIdSchema,
  presetId: entityIdSchema,
  confirmed: z.boolean().default(false),
  confirmationInput: z.string().max(256).default('')
}).strict()

export const commandJobStateSchema = z.enum(['running', 'completed', 'cancelled', 'failed', 'terminal'])
export const commandJobSchema = z.object({
  schemaVersion: z.literal(1),
  id: entityIdSchema,
  hostId: entityIdSchema,
  presetId: entityIdSchema,
  name: z.string().min(1).max(512),
  command: z.string().min(1).max(32_768),
  state: commandJobStateSchema,
  risk: commandRiskSchema,
  startedAt: z.iso.datetime(),
  completedAt: z.iso.datetime().optional(),
  exitCode: z.number().int().nullable().optional(),
  exitSignal: z.string().nullable().optional(),
  output: z.string().max(2 * 1024 * 1024),
  error: z.string().max(4096).optional(),
  terminalSessionId: entityIdSchema.optional()
})
export const commandJobRequestSchema = z.object({ jobId: entityIdSchema }).strict()
export const commandEventSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('job'), job: commandJobSchema }),
  z.object({ type: z.literal('data'), jobId: entityIdSchema, stream: z.enum(['stdout', 'stderr']), data: z.string().max(1_048_576) })
])

export const codexCapabilitySchema = z.object({
  deviceAuth: z.boolean(),
  resume: z.boolean(),
  resumeLast: z.boolean(),
  update: z.boolean()
})
export const codexStatusSchema = codexEnvironmentStatusSchema.extend({ capabilities: codexCapabilitySchema })
export const codexProbeRequestSchema = z.object({ hostId: entityIdSchema }).strict()
export const codexInstallPlanSchema = z.object({
  command: z.literal('curl -fsSL https://chatgpt.com/codex/install.sh | sh'),
  sourceUrl: z.literal('https://chatgpt.com/codex/install.sh'),
  impact: z.string().min(1).max(2048)
})
export const codexActionSchema = z.enum(['install', 'login', 'update', 'start', 'resume', 'tmux', 'reattach'])
export const codexActionRequestSchema = z.object({
  hostId: entityIdSchema,
  action: codexActionSchema,
  confirmed: z.boolean().default(false)
}).strict()
export const codexActionResultSchema = z.object({
  action: codexActionSchema,
  command: z.string().min(1).max(32_768),
  terminal: terminalSessionSchema,
  tmuxSession: z.string().max(128).nullable()
})
export const codexExternalConnectionRequestSchema = z.object({ hostId: entityIdSchema }).strict()
export const codexExternalConnectionResultSchema = z.object({ opened: z.literal(true), alias: hostAliasSchema })

export type CommandPresetInput = z.infer<typeof commandPresetInputSchema>
export type CommandDefinition = z.infer<typeof commandDefinitionSchema>
export type CommandAnalysis = z.infer<typeof commandAnalysisSchema>
export type CommandRunRequest = z.infer<typeof commandRunRequestSchema>
export type CommandJob = z.infer<typeof commandJobSchema>
export type CommandEvent = z.infer<typeof commandEventSchema>
export type CodexStatus = z.infer<typeof codexStatusSchema>
export type CodexActionRequest = z.infer<typeof codexActionRequestSchema>
