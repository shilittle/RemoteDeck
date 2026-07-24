import { z } from 'zod'

export const diagnosticsExportResultSchema = z.object({
  exported: z.boolean(),
  path: z.string().max(32_767).nullable(),
  bytes: z.number().int().nonnegative(),
  sha256: z.string().regex(/^[a-f0-9]{64}$/).nullable(),
  entries: z.number().int().nonnegative()
})

export type DiagnosticsExportResult = z.infer<typeof diagnosticsExportResultSchema>
