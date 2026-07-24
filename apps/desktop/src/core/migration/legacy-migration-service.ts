import { createHash } from 'node:crypto'
import { readFile, stat } from 'node:fs/promises'
import { z } from 'zod'
import type { LegacyMigrationApply, LegacyMigrationPreview, LegacyMigrationResult } from '../../protocol/migration'
import type { HostService } from '../hosts/host-service'
import type { ProfileRepository } from '../hosts/profile-repository'
import type { SettingsService } from '../settings-service'

const legacyPresetSchema = z.object({
  name: z.string().trim().min(1).max(512),
  description: z.string().max(2048).default(''),
  risk: z.enum(['safe', 'danger']).default('danger'),
  confirmToken: z.string().max(256).optional(),
  command: z.string().min(1).max(32_768)
})
const legacyConfigSchema = z.object({
  appName: z.string().max(512).default('LabPulse SSH'),
  sshHost: z.string().trim().min(1).max(64).regex(/^[A-Za-z0-9_.-]+$/),
  forwardHost: z.string().trim().min(1).max(64).regex(/^[A-Za-z0-9_.-]+$/).optional(),
  telemetryIntervalSeconds: z.number().int().min(1).max(60).default(3),
  btop: z.object({ enabled: z.boolean().default(true), autoRestart: z.boolean().default(false), command: z.string().max(32_768).default('btop'), restartCycleSeconds: z.number().int().min(60).max(86_400).default(900) }).default({ enabled: true, autoRestart: false, command: 'btop', restartCycleSeconds: 900 }),
  forward: z.object({
    autoStart: z.boolean().default(false),
    remoteBindAddress: z.string().min(1).max(512).default('127.0.0.1'),
    remotePort: z.number().int().min(1).max(65_535),
    localTargetAddress: z.string().min(1).max(512).default('127.0.0.1'),
    localTargetPort: z.number().int().min(1).max(65_535),
    releaseCommand: z.string().min(1).max(16_384).optional()
  }).optional(),
  dangerousCommandPatterns: z.array(z.string().min(1).max(1024)).max(128).default([]),
  presets: z.array(legacyPresetSchema).max(256).default([])
})

export class LegacyMigrationService {
  readonly #profiles: ProfileRepository
  readonly #hosts: HostService
  readonly #settings: SettingsService

  constructor(profiles: ProfileRepository, hosts: HostService, settings: SettingsService) {
    this.#profiles = profiles
    this.#hosts = hosts
    this.#settings = settings
  }

  async preview(sourcePath: string): Promise<LegacyMigrationPreview> {
    const { config, hash } = await loadLegacy(sourcePath)
    const warnings: string[] = []
    if (config.forwardHost) warnings.push(`旧版转发别名 ${config.forwardHost} 将迁移为主机 ${config.sshHost} 上的 RemoteForward；请在导入前核对端点。`)
    if (config.forward?.releaseCommand) warnings.push('旧版端口释放命令将以禁用的高风险 legacyCleanupHook 导入，不会自动执行。')
    if (config.btop.command !== 'btop') warnings.push(`旧版 btop 命令仅用于预览；v1 使用真实 btop PTY，不执行自定义参数：${config.btop.command}`)
    return {
      schemaVersion: 1,
      sourcePath,
      sourceHash: hash,
      duplicate: await this.#profiles.hasImport(hash),
      appName: config.appName,
      sshAlias: config.sshHost,
      forwardAlias: config.forwardHost ?? null,
      telemetryIntervalSeconds: config.telemetryIntervalSeconds,
      btop: { enabled: config.btop.enabled, autoRestart: config.btop.autoRestart, rotationMinutes: Math.max(1, Math.round(config.btop.restartCycleSeconds / 60)), command: config.btop.command },
      tunnel: config.forward ? { bindAddress: config.forward.remoteBindAddress, sourcePort: config.forward.remotePort, targetHost: config.forward.localTargetAddress, targetPort: config.forward.localTargetPort, autoStart: config.forward.autoStart, cleanupCommand: config.forward.releaseCommand ?? null } : null,
      commands: config.presets.map((preset, index) => ({
        name: preset.name,
        description: preset.description,
        group: 'Legacy migration',
        command: preset.command,
        risk: legacyRisk(preset.risk),
        requiresPty: legacyPty[preset.risk],
        requiresSudo: false,
        ...(preset.confirmToken ? { confirmationText: preset.confirmToken } : {}),
        sortOrder: index
      })),
      legacyRiskRules: config.dangerousCommandPatterns,
      warnings
    }
  }

  async apply(request: LegacyMigrationApply): Promise<LegacyMigrationResult> {
    const preview = await this.preview(request.sourcePath)
    if (preview.sourceHash !== request.sourceHash) throw new Error('Legacy config changed after preview; preview it again before importing')
    if (preview.duplicate) return { schemaVersion: 1, sourceHash: preview.sourceHash, duplicate: true, hostId: null, tunnelIds: [], commandIds: [], warnings: ['该源配置已导入，未创建重复项目。'], importedAt: new Date().toISOString() }
    const host = await this.#hosts.create({
      alias: request.host.alias, hostname: request.host.hostname, port: request.host.port, username: request.host.username,
      groups: ['legacy-import'], auth: request.host.auth,
      ...(request.host.workspacePath ? { workspacePath: request.host.workspacePath } : {}),
      advanced: { connectTimeoutSeconds: 15, serverAliveIntervalSeconds: 30, serverAliveCountMax: 3, tcpKeepAlive: true, compression: false, identitiesOnly: false }
    })
    const tunnelIds: string[] = []
    const commandIds: string[] = []
    if (request.includeTunnel && preview.tunnel) {
      const tunnel = await this.#profiles.addTunnel({
        hostId: host.host.id, name: `Legacy ${preview.forwardAlias ?? 'RemoteForward'}`, direction: 'remote',
        bindAddress: preview.tunnel.bindAddress, sourcePort: preview.tunnel.sourcePort, targetHost: preview.tunnel.targetHost, targetPort: preview.tunnel.targetPort,
        autoStart: preview.tunnel.autoStart,
        healthCheck: { type: 'tcp', intervalSeconds: 5, timeoutMs: 2000 },
        ...(preview.tunnel.cleanupCommand ? { legacyCleanupHook: { command: preview.tunnel.cleanupCommand, authorized: false } } : {})
      })
      tunnelIds.push(tunnel.id)
    }
    if (request.includeCommands) {
      for (const command of preview.commands) {
        const created = await this.#profiles.addCommand({ ...command, hostId: host.host.id })
        commandIds.push(created.id)
      }
    }
    await this.#settings.update({ telemetryIntervalSeconds: preview.telemetryIntervalSeconds, btopWatchdogEnabled: preview.btop.enabled && preview.btop.autoRestart, btopRotationMinutes: preview.btop.rotationMinutes })
    await this.#profiles.recordImport(preview.sourceHash, [], preview.legacyRiskRules)
    return { schemaVersion: 1, sourceHash: preview.sourceHash, duplicate: false, hostId: host.host.id, tunnelIds, commandIds, warnings: preview.warnings, importedAt: new Date().toISOString() }
  }
}

async function loadLegacy(sourcePath: string): Promise<{ config: z.infer<typeof legacyConfigSchema>; hash: string }> {
  const metadata = await stat(sourcePath)
  if (!metadata.isFile()) throw new Error('Legacy config path is not a file')
  if (metadata.size > 4 * 1024 * 1024) throw new Error('Legacy config exceeds the 4 MiB limit')
  const source = await readFile(sourcePath, 'utf8')
  let json: unknown
  try { json = JSON.parse(source) }
  catch { throw new Error('Legacy config is not valid JSON') }
  return { config: legacyConfigSchema.parse(json), hash: createHash('sha256').update(source).digest('hex') }
}

const legacyRisks = { safe: 'L0', danger: 'L2' } as const
const legacyPty = { safe: false, danger: true } as const
function legacyRisk(risk: keyof typeof legacyRisks): 'L0' | 'L2' { return legacyRisks[risk] }
