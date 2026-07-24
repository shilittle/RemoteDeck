import { z } from 'zod'

export const logLevelSchema = z.enum(['debug', 'info', 'warn', 'error'])

export const appSettingsSchema = z.object({
  schemaVersion: z.literal(1),
  launchAtLogin: z.boolean(),
  closeToTray: z.boolean(),
  terminalFontFamily: z.string().trim().min(1).max(256),
  terminalFontSize: z.number().int().min(9).max(32),
  telemetryIntervalSeconds: z.number().int().min(1).max(60),
  telemetryRetentionMinutes: z.number().int().min(1).max(1440),
  sshConfigPath: z.string().max(32_767),
  downloadDirectory: z.string().max(32_767),
  autoReconnect: z.boolean(),
  btopWatchdogEnabled: z.boolean().default(false),
  btopRotationMinutes: z.number().int().min(1).max(1440).default(15),
  logLevel: logLevelSchema,
  onboardingCompleted: z.boolean()
})

export const appSettingsPatchSchema = appSettingsSchema.omit({ schemaVersion: true }).partial().strict()

export const defaultAppSettings: AppSettings = {
  schemaVersion: 1,
  launchAtLogin: false,
  closeToTray: true,
  terminalFontFamily: 'Cascadia Mono',
  terminalFontSize: 14,
  telemetryIntervalSeconds: 3,
  telemetryRetentionMinutes: 30,
  sshConfigPath: '',
  downloadDirectory: '',
  autoReconnect: true,
  btopWatchdogEnabled: false,
  btopRotationMinutes: 15,
  logLevel: 'info',
  onboardingCompleted: false
}

export type AppSettings = z.infer<typeof appSettingsSchema>
export type AppSettingsPatch = z.infer<typeof appSettingsPatchSchema>
