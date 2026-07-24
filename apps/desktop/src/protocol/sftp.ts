import { z } from 'zod'
import { entityIdSchema, nonEmptyTextSchema, transferJobSchema } from './domain'

const remotePathSchema = z.string().min(1).max(4096).refine((value) => !value.includes('\0'), 'Remote path contains NUL')
const localPathSchema = z.string().min(1).max(32_767).refine((value) => !value.includes('\0'), 'Local path contains NUL')
export const conflictPolicySchema = z.enum(['skip', 'overwrite', 'rename'])

export const sftpEntrySchema = z.object({
  name: nonEmptyTextSchema,
  path: remotePathSchema,
  type: z.enum(['file', 'directory', 'symlink', 'other']),
  size: z.number().int().nonnegative(),
  mode: z.number().int().nonnegative(),
  modifiedAt: z.iso.datetime(),
  uid: z.number().int().nonnegative(),
  gid: z.number().int().nonnegative()
})

export const sftpListRequestSchema = z.object({ hostId: entityIdSchema, path: remotePathSchema, showHidden: z.boolean().default(false) }).strict()
export const sftpListResultSchema = z.object({ path: remotePathSchema, entries: z.array(sftpEntrySchema) })
export const sftpCreateRequestSchema = z.object({ hostId: entityIdSchema, parentPath: remotePathSchema, name: nonEmptyTextSchema, type: z.enum(['file', 'directory']) }).strict()
export const sftpRenameRequestSchema = z.object({ hostId: entityIdSchema, path: remotePathSchema, newName: nonEmptyTextSchema }).strict()
export const sftpDeleteRequestSchema = z.object({ hostId: entityIdSchema, paths: z.array(remotePathSchema).min(1).max(1000) }).strict()
export const sftpOperationResultSchema = z.object({ success: z.literal(true), affected: z.array(remotePathSchema) })

export const transferUploadRequestSchema = z.object({ hostId: entityIdSchema, sources: z.array(localPathSchema).min(1).max(1000), remoteDirectory: remotePathSchema, conflictPolicy: conflictPolicySchema }).strict()
export const transferDownloadRequestSchema = z.object({ hostId: entityIdSchema, sources: z.array(remotePathSchema).min(1).max(1000), localDirectory: localPathSchema, conflictPolicy: conflictPolicySchema }).strict()
export const transferJobRequestSchema = z.object({ jobId: entityIdSchema }).strict()
export const transferStartResultSchema = z.object({ jobs: z.array(transferJobSchema) })
export const transferEventSchema = z.object({ job: transferJobSchema })
export const pickUploadRequestSchema = z.object({ kind: z.enum(['files', 'directory']) }).strict()
export const pickPathsResultSchema = z.object({ paths: z.array(localPathSchema), canceled: z.boolean() })

export type SftpEntry = z.infer<typeof sftpEntrySchema>
export type SftpListResult = z.infer<typeof sftpListResultSchema>
export type TransferUploadRequest = z.infer<typeof transferUploadRequestSchema>
export type TransferDownloadRequest = z.infer<typeof transferDownloadRequestSchema>
export type ConflictPolicy = z.infer<typeof conflictPolicySchema>
