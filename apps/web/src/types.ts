export type Identifier = string

export interface SshAdvancedOptions {
  connectTimeoutSeconds: number
  serverAliveIntervalSeconds: number
  serverAliveCountMax: number
  tcpKeepAlive: boolean
  compression: boolean
  identitiesOnly: boolean
}

export type AuthMethod = 'interactive' | 'private_key' | 'agent'

export interface HostProfile {
  schemaVersion: number
  id: Identifier
  alias: string
  hostname: string
  port: number
  username: string
  authMethod?: AuthMethod
  identityFile: string | null
  proxyJump: string | null
  defaultWorkspace: string
  groups: string[]
  advanced: SshAdvancedOptions
  monitorEnabled: boolean
  createdAt: string
  updatedAt: string
}

export interface HostDraft {
  id?: Identifier
  alias: string
  hostname: string
  port: number
  username: string
  authMethod?: AuthMethod
  identityFile?: string | null
  proxyJump?: string | null
  defaultWorkspace?: string | null
  groups: string[]
  advanced?: Partial<SshAdvancedOptions>
  monitorEnabled?: boolean
}

export interface HostKeyCandidate {
  hostToken: string
  algorithm: string
  publicKeyBase64: string
  sha256Fingerprint: string
  rawLine: string
  trusted: boolean
  mismatch: boolean
  previousFingerprint: string | null
}

export interface HostKeyRecord {
  id: Identifier
  hostId: Identifier | null
  hostToken: string
  hostname: string
  port: number
  algorithm: string
  publicKeyBase64: string
  sha256Fingerprint: string
  acceptedAt: string
}

export interface PrivateKeyRecord {
  path: string
  algorithm: string
  fingerprint: string | null
  encrypted: boolean
  comment: string | null
}

export interface KeyGenerateRequest {
  privateKeyPath: string
  comment: string
}

export interface KeyDeployRequest {
  hostId: Identifier
  privateKeyPath: string
  makeDefault: boolean
}

export interface KeyOperationResult {
  success: boolean
  message: string
  privateKeyPath: string | null
  publicKeyPath: string | null
  fingerprint: string | null
}

export interface SshImportResult {
  imported: HostProfile[]
  skipped: string[]
  warnings: string[]
}

export interface RuntimeCapabilities {
  sshPath: string | null
  keyscanPath: string | null
  keygenPath: string | null
  pty: boolean
  localForward: boolean
  remoteForward: boolean
  sftp?: boolean
  telemetry?: boolean
  processSignals?: boolean
  agents?: boolean
}

export interface AppSettings {
  schemaVersion: number
  terminalFontFamily: string
  terminalFontSize: number
  telemetryIntervalSeconds: number
  telemetryRetentionMinutes: number
  downloadDirectory: string
  autoReconnect: boolean
  closeToTray: boolean
  launchAtLogin: boolean
  btopWatchdogEnabled: boolean
  btopRotationMinutes: number
  logLevel: 'debug' | 'info' | 'warn' | 'error'
  onboardingCompleted: boolean
  defaultAgent: AgentKind
}

export interface BootstrapPayload {
  configRevision: number
  appVersion: string
  settings: AppSettings
  hosts: HostProfile[]
  tunnels: TunnelProfile[]
  capabilities: RuntimeCapabilities
}

export interface ConnectionTestResult {
  success: boolean
  latencyMs: number
  serverLine: string | null
  error: string | null
}

export type TerminalState = 'starting' | 'running' | 'offline' | 'closed' | 'failed'
export type TerminalEventKind = 'started' | 'output' | 'exit' | 'error' | 'state' | 'replayTruncated'

export interface TerminalSnapshot {
  sessionId: Identifier
  generation: number
  revision: number
  hostId: Identifier
  alias: string
  cwd: string
  title: string
  state: TerminalState
  exitCode?: number | null
  error?: string | null
}

export interface TerminalEvent {
  sessionId: Identifier
  kind: TerminalEventKind
  data: string | null
  exitCode: number | null
  message: string | null
  snapshot?: TerminalSnapshot
}

export type SftpEntryKind = 'file' | 'directory' | 'symlink' | 'other'

export interface SftpEntry {
  name: string
  path: string
  kind: SftpEntryKind
  size: number
  modifiedAt: string | null
  permissions: string | null
}

export interface SftpListing {
  hostId: Identifier
  path: string
  parentPath: string | null
  entries: SftpEntry[]
}

export type TransferDirection = 'upload' | 'download'
export type TransferState = 'queued' | 'running' | 'cancelling' | 'completed' | 'failed' | 'cancelled'
export type TransferConflictPolicy = 'ask' | 'overwrite' | 'skip' | 'rename'

export interface TransferJob {
  id: Identifier
  revision: number
  hostId: Identifier
  direction: TransferDirection
  source: string
  destination: string
  state: TransferState
  bytesTransferred: number
  totalBytes: number | null
  error: string | null
  createdAt: string
  updatedAt: string
}

export interface TransferRequest {
  hostId: Identifier
  source: string
  destination: string
  conflictPolicy: TransferConflictPolicy
  recursive: boolean
}

export interface TransferEvent {
  job: TransferJob
}

export type TunnelDirection = 'local' | 'remote'

export interface TunnelHealthCheck {
  kind: 'none' | 'tcp'
  intervalSeconds: number
  timeoutSeconds: number
}

export interface TunnelProfile {
  schemaVersion: number
  id: Identifier
  hostId: Identifier
  name: string
  direction: TunnelDirection
  bindAddress: string
  sourcePort: number
  targetHost: string
  targetPort: number
  autoStart: boolean
  autoReconnect?: boolean
  healthCheck?: TunnelHealthCheck | null
  createdAt: string
  updatedAt: string
}

export interface TunnelDraft {
  id?: Identifier
  hostId: Identifier
  name: string
  direction: TunnelDirection
  bindAddress: string
  sourcePort: number
  targetHost: string
  targetPort: number
  autoStart?: boolean
  autoReconnect?: boolean
  healthCheck?: TunnelHealthCheck | null
}

export type TunnelRuntimeState = 'stopped' | 'starting' | 'running' | 'waiting' | 'failed'

export interface TunnelLogEntry {
  at: string
  level: 'info' | 'warn' | 'error'
  message: string
}

export interface TunnelSnapshot {
  tunnelId: Identifier
  revision: number
  profile?: TunnelProfile
  state: TunnelRuntimeState
  health?: 'unknown' | 'healthy' | 'degraded' | 'failed'
  uptimeSeconds?: number
  reconnectCount?: number
  message: string | null
  logs?: TunnelLogEntry[]
}

export type TelemetryRuntimeState = 'stopped' | 'starting' | 'online' | 'degraded' | 'failed'

export interface TelemetryStatus {
  hostId: Identifier
  revision: number
  state: TelemetryRuntimeState
  lastSampleAt: string | null
  error: string | null
}

export interface CpuSnapshot {
  percent: number
  load1: number
  load5: number
  load15: number
}

export interface MemorySnapshot {
  usedBytes: number
  totalBytes: number
  swapUsedBytes: number
  swapTotalBytes: number
}

export interface NetworkSnapshot {
  receiveBytesPerSecond: number
  sendBytesPerSecond: number
}

export interface DiskSnapshot {
  mount: string
  usedBytes: number
  totalBytes: number
  availableBytes: number
}

export interface GpuSnapshot {
  index: number
  name: string
  utilizationPercent: number
  memoryUsedMiB: number
  memoryTotalMiB: number
  temperatureC: number | null
  powerW: number | null
}

export interface ProcessSnapshot {
  pid: number
  startTicks: number
  user: string
  cpuPercent: number
  memoryPercent: number
  state: string
  elapsedSeconds: number
  command: string
}

export interface TelemetrySnapshot {
  hostId: Identifier
  sampledAt: string
  hostname: string
  currentUser: string
  cpu: CpuSnapshot
  memory: MemorySnapshot
  network: NetworkSnapshot
  disks: DiskSnapshot[]
  gpus: GpuSnapshot[]
  processes: ProcessSnapshot[]
}

export interface TelemetryEvent {
  status: TelemetryStatus
  sample: TelemetrySnapshot | null
}

export interface BtopStatus {
  hostId: Identifier
  installed: boolean
  version: string | null
  watchdogState: 'stopped' | 'running' | 'unavailable' | 'conflict'
  rotationMinutes: number
  restartCount: number | null
  lastError: string | null
}

export type CommandRisk = 'L0' | 'L1' | 'L2'

export interface CommandDefinition {
  id: Identifier
  hostId: Identifier | null
  name: string
  description: string
  group: string
  command: string
  workingDirectory: string | null
  risk: CommandRisk
  requiresPty: boolean
  requiresSudo: boolean
  confirmationText: string | null
  sortOrder: number
  builtin: boolean
}

export interface CommandDraft {
  id?: Identifier
  hostId?: Identifier | null
  name: string
  description: string
  group: string
  command: string
  workingDirectory?: string | null
  risk: CommandRisk
  requiresPty: boolean
  requiresSudo: boolean
  confirmationText?: string | null
  sortOrder?: number
}

export interface CommandAnalysis {
  targetAlias: string
  displayCommand: string
  workingDirectory: string | null
  declaredRisk: CommandRisk
  effectiveRisk: CommandRisk
  reasons: string[]
  requiredConfirmation: string | null
}

export type CommandJobState = 'queued' | 'running' | 'cancelling' | 'completed' | 'failed' | 'cancelled'

export interface CommandJob {
  id: Identifier
  revision: number
  commandId: Identifier | null
  hostId: Identifier
  name: string
  command: string
  risk: CommandRisk
  state: CommandJobState
  stdout: string
  stderr: string
  exitCode: number | null
  error: string | null
  startedAt: string | null
  finishedAt: string | null
}

export interface CommandRunRequest {
  hostId: Identifier
  commandId?: Identifier | null
  command?: string
  workingDirectory?: string | null
  confirmation?: string | null
}

export interface CommandEvent {
  job: CommandJob
}

export type AgentKind = 'codex' | 'claude' | 'gemini' | 'opencode'
export type AgentAction = 'install' | 'login' | 'start' | 'resume' | 'update'

export interface AgentStatus {
  hostId: Identifier
  agent: AgentKind
  installed: boolean
  version: string | null
  authenticated: boolean | null
  tmuxAvailable: boolean
  resumableSessions: string[]
  installHint: string | null
  documentationUrl: string | null
}

export interface AgentSessionRequest {
  hostId: Identifier
  agent: AgentKind
  action: AgentAction
  workspace: string | null
  sessionName: string | null
  confirmation?: string | null
}

export interface AgentPlannedCommand {
  command: string
  mode: 'capture' | 'interactive_pty'
  purpose: string
  timeoutSeconds: number | null
}

export interface AgentCommandPlan {
  provider: AgentKind
  action: AgentAction
  displayName: string
  commands: AgentPlannedCommand[]
  requiresConfirmation: boolean
  sourceUrl: string | null
  tmuxSession: string | null
  notice: string
}

export interface AgentSessionResult {
  terminal: TerminalSnapshot
  message: string
}

export interface LegacyPreview {
  sourcePath: string
  sourceHash: string
  appName: string
  duplicate: boolean
  hosts: Array<{ alias: string; hostname: string; port: number; username: string }>
  tunnelCount: number
  commandCount: number
  settingsIncluded: boolean
  warnings: string[]
}

export interface LegacyApplyRequest {
  sourcePath: string
  sourceHash: string
  includeHosts: boolean
  includeTunnels: boolean
  includeCommands: boolean
  includeSettings: boolean
}

export interface LegacyApplyResult {
  importedHosts: number
  importedTunnels: number
  importedCommands: number
  settingsImported: boolean
  message: string
}

export interface DiagnosticsResult {
  exported: boolean
  path: string | null
  sha256: string | null
  entries: number
}
