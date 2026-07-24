import { z } from 'zod'
import { commandPresetSchema, entityIdSchema, hostAliasSchema, portSchema } from './domain'
import { authInputSchema } from './ssh'

export const legacyMigrationPreviewSchema = z.object({
  schemaVersion: z.literal(1),
  sourcePath: z.string().min(1).max(32_767),
  sourceHash: z.string().regex(/^[a-f0-9]{64}$/),
  duplicate: z.boolean(),
  appName: z.string().max(512),
  sshAlias: hostAliasSchema,
  forwardAlias: hostAliasSchema.nullable(),
  telemetryIntervalSeconds: z.number().int().min(1).max(60),
  btop: z.object({ enabled: z.boolean(), autoRestart: z.boolean(), rotationMinutes: z.number().int().min(1).max(1440), command: z.string().max(32_768) }),
  tunnel: z.object({ bindAddress: z.string().min(1).max(512), sourcePort: portSchema, targetHost: z.string().min(1).max(512), targetPort: portSchema, autoStart: z.boolean(), cleanupCommand: z.string().max(16_384).nullable() }).nullable(),
  commands: z.array(commandPresetSchema.omit({ schemaVersion: true, id: true, hostId: true, createdAt: true, updatedAt: true })).max(256),
  legacyRiskRules: z.array(z.string().min(1).max(1024)).max(128),
  warnings: z.array(z.string().max(4096)).max(128)
})

export const legacyMigrationApplySchema = z.object({
  sourcePath: z.string().min(1).max(32_767),
  sourceHash: z.string().regex(/^[a-f0-9]{64}$/),
  host: z.object({
    alias: hostAliasSchema,
    hostname: z.string().trim().min(1).max(512),
    port: portSchema,
    username: z.string().trim().min(1).max(512),
    workspacePath: z.string().trim().min(1).max(4096).optional(),
    auth: authInputSchema
  }).strict(),
  includeTunnel: z.boolean(),
  includeCommands: z.boolean()
}).strict()

export const legacyMigrationResultSchema = z.object({
  schemaVersion: z.literal(1),
  sourceHash: z.string().regex(/^[a-f0-9]{64}$/),
  duplicate: z.boolean(),
  hostId: entityIdSchema.nullable(),
  tunnelIds: z.array(entityIdSchema),
  commandIds: z.array(entityIdSchema),
  warnings: z.array(z.string().max(4096)),
  importedAt: z.iso.datetime()
})

export type LegacyMigrationPreview = z.infer<typeof legacyMigrationPreviewSchema>
export type LegacyMigrationApply = z.infer<typeof legacyMigrationApplySchema>
export type LegacyMigrationResult = z.infer<typeof legacyMigrationResultSchema>
