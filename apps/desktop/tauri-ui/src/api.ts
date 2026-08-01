import { invoke as tauriInvoke, isTauri } from '@tauri-apps/api/core'
import { listen as tauriListen } from '@tauri-apps/api/event'
import type {
  AgentCommandPlan,
  AgentKind,
  AgentSessionRequest,
  AgentSessionResult,
  AgentStatus,
  AppSettings,
  BootstrapPayload,
  BtopStatus,
  CommandAnalysis,
  CommandDefinition,
  CommandDraft,
  CommandEvent,
  CommandJob,
  CommandRunRequest,
  ConnectionTestResult,
  DiagnosticsResult,
  HostDraft,
  HostKeyCandidate,
  HostKeyRecord,
  HostProfile,
  KeyDeployRequest,
  KeyGenerateRequest,
  KeyOperationResult,
  LegacyApplyRequest,
  LegacyApplyResult,
  LegacyPreview,
  PrivateKeyRecord,
  ProcessSnapshot,
  SftpListing,
  SshImportResult,
  TelemetryEvent,
  TelemetrySnapshot,
  TelemetryStatus,
  TerminalEvent,
  TerminalSnapshot,
  TransferEvent,
  TransferJob,
  TransferRequest,
  TunnelDraft,
  TunnelProfile,
  TunnelSnapshot
} from './types'

function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    return Promise.reject(new Error('当前页面不在 Tauri 运行时中，请使用 pnpm dev 启动桌面应用。'))
  }
  return tauriInvoke<T>(command, args)
}

async function listen(event: string, handler: (payload: unknown) => void): Promise<() => void> {
  if (!isTauri()) throw new Error('Tauri 事件 API 不可用。')
  return tauriListen<unknown>(event, ({ payload }) => handler(payload))
}

export interface TerminalInputEndpoint {
  url: string
}

interface TerminalInputSocket {
  readonly readyState: number
  readonly bufferedAmount: number
  send: (data: ArrayBuffer) => void
}

export const MAX_TERMINAL_INPUT_FRAME_BYTES = 64 * 1024
export const MAX_TERMINAL_INPUT_BATCH_BYTES = 256 * 1024

export function sendTerminalInput(socket: TerminalInputSocket, value: string): void {
  if (socket.readyState !== 1) throw new Error('安全终端输入通道尚未连接。')
  const bytes = new TextEncoder().encode(value)
  if (bytes.byteLength === 0) return
  if (bytes.byteLength > MAX_TERMINAL_INPUT_BATCH_BYTES) {
    throw new Error('单次终端输入不能超过 256 KiB。')
  }
  if (socket.bufferedAmount + bytes.byteLength > MAX_TERMINAL_INPUT_BATCH_BYTES) {
    throw new Error('安全终端输入通道繁忙，请稍后重试。')
  }
  for (let offset = 0; offset < bytes.byteLength; offset += MAX_TERMINAL_INPUT_FRAME_BYTES) {
    socket.send(bytes.slice(offset, offset + MAX_TERMINAL_INPUT_FRAME_BYTES).buffer)
  }
}

export const api = {
  bootstrap: (): Promise<BootstrapPayload> => invoke('bootstrap'),
  updateSettings: (patch: Partial<AppSettings>): Promise<AppSettings> => invoke('update_settings', { patch }),
  pickLocalPath: (directory: boolean): Promise<string | null> => invoke('pick_local_path', { directory }),
  pickSavePath: (suggestedName: string): Promise<string | null> => invoke('pick_save_path', { suggestedName }),

  saveHost: (draft: HostDraft): Promise<HostProfile> => invoke('save_host', { draft }),
  deleteHost: (hostId: string): Promise<void> => invoke('delete_host', { hostId }),
  importSshConfig: (configPath: string): Promise<SshImportResult> => invoke('import_ssh_config', { configPath }),
  scanHostKeys: (hostId: string): Promise<HostKeyCandidate[]> => invoke('scan_host_keys', { hostId }),
  acceptHostKey: (hostId: string, candidate: HostKeyCandidate): Promise<void> =>
    invoke('accept_host_key', { hostId, candidate }),
  listHostKeys: (): Promise<HostKeyRecord[]> => invoke('list_host_keys'),
  removeHostKey: (recordId: string): Promise<void> => invoke('remove_host_key', { recordId }),
  testConnection: (hostId: string): Promise<ConnectionTestResult> => invoke('test_connection', { hostId }),
  listKeys: (): Promise<PrivateKeyRecord[]> => invoke('list_keys'),
  generateKey: (request: KeyGenerateRequest): Promise<KeyOperationResult> => invoke('generate_key', { request }),
  deployKey: (request: KeyDeployRequest): Promise<KeyOperationResult> => invoke('deploy_key', { request }),

  listTerminals: (): Promise<TerminalSnapshot[]> => invoke('list_terminals'),
  startTerminal: (hostId: string, rows: number, cols: number): Promise<TerminalSnapshot> =>
    invoke('start_terminal', { hostId, rows, cols }),
  reconnectTerminal: (sessionId: string, rows: number, cols: number): Promise<TerminalSnapshot> =>
    invoke('reconnect_terminal', { sessionId, rows, cols }),
  openTerminalInput: (sessionId: string): Promise<TerminalInputEndpoint> =>
    invoke('open_terminal_input', { sessionId }),
  resizeTerminal: (sessionId: string, rows: number, cols: number): Promise<void> =>
    invoke('resize_terminal', { sessionId, rows, cols }),
  closeTerminal: (sessionId: string): Promise<void> => invoke('close_terminal', { sessionId }),

  listSftp: (hostId: string, path: string): Promise<SftpListing> => invoke('sftp_list', { hostId, path }),
  createSftpDirectory: (hostId: string, path: string): Promise<void> => invoke('sftp_create_directory', { hostId, path }),
  renameSftp: (hostId: string, sourcePath: string, destinationPath: string): Promise<void> =>
    invoke('sftp_rename', { hostId, sourcePath, destinationPath }),
  deleteSftp: (hostId: string, path: string, recursive: boolean): Promise<void> =>
    invoke('sftp_delete', { hostId, path, recursive }),
  listTransfers: (hostId?: string): Promise<TransferJob[]> => invoke('transfer_list', hostId ? { hostId } : undefined),
  upload: (request: TransferRequest): Promise<TransferJob> => invoke('transfer_upload', { request }),
  download: (request: TransferRequest): Promise<TransferJob> => invoke('transfer_download', { request }),
  cancelTransfer: (jobId: string): Promise<TransferJob> => invoke('transfer_cancel', { jobId }),
  retryTransfer: (jobId: string): Promise<TransferJob> => invoke('transfer_retry', { jobId }),
  showTransferInFolder: (jobId: string): Promise<void> => invoke('transfer_show_in_folder', { jobId }),

  listTunnels: (hostId?: string): Promise<TunnelSnapshot[]> => invoke('list_tunnels', hostId ? { hostId } : undefined),
  saveTunnel: (draft: TunnelDraft): Promise<TunnelProfile> => invoke('save_tunnel', { draft }),
  deleteTunnel: (tunnelId: string): Promise<void> => invoke('delete_tunnel', { tunnelId }),
  startTunnel: (tunnelId: string): Promise<TunnelSnapshot> => invoke('start_tunnel', { tunnelId }),
  stopTunnel: (tunnelId: string): Promise<TunnelSnapshot> => invoke('stop_tunnel', { tunnelId }),
  restartTunnel: (tunnelId: string): Promise<TunnelSnapshot> => invoke('restart_tunnel', { tunnelId }),

  listTelemetry: (): Promise<TelemetryStatus[]> => invoke('telemetry_list'),
  telemetryHistory: (hostId: string): Promise<TelemetrySnapshot[]> => invoke('telemetry_history', { hostId }),
  startTelemetry: (hostId: string): Promise<TelemetryStatus> => invoke('telemetry_start', { hostId }),
  stopTelemetry: (hostId: string): Promise<TelemetryStatus> => invoke('telemetry_stop', { hostId }),
  signalProcess: (hostId: string, process: ProcessSnapshot, signal: 'TERM' | 'KILL'): Promise<void> =>
    invoke('signal_process', { hostId, pid: process.pid, expectedStartTicks: process.startTicks, expectedUser: process.user, expectedCommand: process.command, signal }),
  probeBtop: (hostId: string): Promise<BtopStatus> => invoke('btop_probe', { hostId }),
  startBtopWatchdog: (hostId: string, rotationMinutes: number): Promise<BtopStatus> =>
    invoke('btop_start_watchdog', { hostId, rotationMinutes }),
  stopBtopWatchdog: (hostId: string): Promise<BtopStatus> => invoke('btop_stop_watchdog', { hostId }),

  listCommands: (hostId?: string): Promise<CommandDefinition[]> => invoke('list_commands', hostId ? { hostId } : undefined),
  saveCommand: (draft: CommandDraft): Promise<CommandDefinition> => invoke('save_command', { draft }),
  deleteCommand: (commandId: string, hostId: string): Promise<void> => invoke('delete_command', { commandId, hostId }),
  analyzeCommand: (request: CommandRunRequest): Promise<CommandAnalysis> => invoke('analyze_command', { request }),
  runCommand: (request: CommandRunRequest): Promise<CommandJob> => invoke('run_command', { request }),
  listCommandJobs: (hostId?: string): Promise<CommandJob[]> => invoke('list_command_jobs', hostId ? { hostId } : undefined),
  cancelCommand: (jobId: string): Promise<CommandJob> => invoke('cancel_command', { jobId }),

  probeAgent: (hostId: string, agent: AgentKind): Promise<AgentStatus> => invoke('probe_agent', { hostId, agent }),
  agentSessionPlan: (request: AgentSessionRequest): Promise<AgentCommandPlan> =>
    invoke('agent_session_plan', { request }),
  startAgentSession: (request: AgentSessionRequest): Promise<AgentSessionResult> =>
    invoke('start_agent_session', { request }),

  legacyPreview: (sourcePath: string): Promise<LegacyPreview> => invoke('legacy_preview', { sourcePath }),
  legacyApply: (request: LegacyApplyRequest): Promise<LegacyApplyResult> => invoke('legacy_apply', { request }),
  exportDiagnostics: (): Promise<DiagnosticsResult> => invoke('export_diagnostics')
}

export const events = {
  terminal: (handler: (event: TerminalEvent) => void): Promise<() => void> => listen('terminal-event', (payload) => handler(payload as TerminalEvent)),
  tunnel: (handler: (event: TunnelSnapshot) => void): Promise<() => void> => listen('tunnel-event', (payload) => handler(payload as TunnelSnapshot)),
  transfer: (handler: (event: TransferEvent) => void): Promise<() => void> => listen('transfer-event', (payload) => handler(payload as TransferEvent)),
  telemetry: (handler: (event: TelemetryEvent) => void): Promise<() => void> => listen('telemetry-event', (payload) => handler(payload as TelemetryEvent)),
  command: (handler: (event: CommandEvent) => void): Promise<() => void> => listen('command-event', (payload) => handler(payload as CommandEvent))
}

export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  if (typeof error === 'string') return error
  if (error && typeof error === 'object') {
    const candidate = error as { message?: unknown; error?: unknown }
    if (typeof candidate.message === 'string') return candidate.message
    if (typeof candidate.error === 'string') return candidate.error
  }
  return '操作失败。'
}
