export interface SshAdvancedOptions {
  connectTimeoutSeconds: number
  serverAliveIntervalSeconds: number
  serverAliveCountMax: number
  tcpKeepAlive: boolean
  compression: boolean
  identitiesOnly: boolean
}

export interface HostProfile {
  schemaVersion: 2
  id: string
  alias: string
  hostname: string
  port: number
  username: string
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
  id?: string
  alias: string
  hostname: string
  port: number
  username: string
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
}

export interface RuntimeCapabilities {
  sshPath: string | null
  keyscanPath: string | null
  keygenPath: string | null
  pty: boolean
  localForward: boolean
  remoteForward: boolean
}

export interface AppSettings {
  schemaVersion: 2
  terminalFontFamily: string
  terminalFontSize: number
}

export interface BootstrapPayload {
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

export interface CommandResult {
  exitCode: number | null
  stdout: string
  stderr: string
  durationMs: number
}

export type TerminalState = 'running' | 'closed'
export type TerminalEventKind = 'started' | 'output' | 'exit' | 'error'

export interface TerminalSnapshot {
  sessionId: string
  hostId: string
  alias: string
  state: TerminalState
}

export interface TerminalEvent {
  sessionId: string
  kind: TerminalEventKind
  data: string | null
  exitCode: number | null
  message: string | null
}

export type TunnelDirection = 'local' | 'remote'

export interface TunnelProfile {
  schemaVersion: 2
  id: string
  hostId: string
  name: string
  direction: TunnelDirection
  bindAddress: string
  sourcePort: number
  targetHost: string
  targetPort: number
  autoStart: boolean
  createdAt: string
  updatedAt: string
}

export interface TunnelDraft {
  id?: string
  hostId: string
  name: string
  direction: TunnelDirection
  bindAddress: string
  sourcePort: number
  targetHost: string
  targetPort: number
  autoStart?: boolean
}

export type TunnelRuntimeState = 'stopped' | 'starting' | 'running' | 'failed'

export interface TunnelSnapshot {
  tunnelId: string
  state: TunnelRuntimeState
  message: string | null
}

export interface TauriEvent<T> {
  event: string
  id: number
  payload: T
}

interface TauriGlobal {
  core: {
    invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>
  }
  event: {
    listen<T>(event: string, handler: (event: TauriEvent<T>) => void): Promise<() => void>
  }
}

declare global {
  interface Window {
    __TAURI__?: TauriGlobal
  }
}
