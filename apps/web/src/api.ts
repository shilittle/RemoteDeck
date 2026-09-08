import type {
  AgentCommandPlan, AgentKind, AgentSessionRequest, AgentSessionResult, AgentStatus, AppSettings,
  BootstrapPayload, BtopStatus, CommandAnalysis, CommandDefinition, CommandDraft, CommandEvent,
  CommandJob, CommandRunRequest, ConnectionTestResult, DiagnosticsResult, HostDraft,
  HostKeyCandidate, HostKeyRecord, HostProfile, KeyDeployRequest, KeyGenerateRequest,
  KeyOperationResult, LegacyApplyRequest, LegacyApplyResult, LegacyPreview, PrivateKeyRecord,
  ProcessSnapshot, SftpListing, SshImportResult, TelemetryEvent, TelemetrySnapshot,
  TelemetryStatus, TerminalEvent, TerminalSnapshot, TransferEvent, TransferJob, TransferRequest,
  TunnelDraft, TunnelProfile, TunnelSnapshot
} from './types'

export class ApiError extends Error {
  readonly code: string
  constructor(code: string, message: string) { super(message); this.name = 'ApiError'; this.code = code }
}

interface AuthSession { csrfToken: string }
interface OperationRecord {
  id: string; command: string; hostId?: string; state: 'queued' | 'running' | 'completed' | 'failed'
  createdAt: string; completedAt?: string; error?: { code: string; message: string }; result?: unknown
}
export type OperationSummary = Omit<OperationRecord, 'result'>
export interface TerminalInputEndpoint { url: string; generation: number; maxFrameBytes?: number; maxBufferedBytes?: number }
interface TerminalInputSocket { readonly readyState: number; readonly bufferedAmount: number; send: (data: ArrayBuffer) => void }
interface TerminalInputLimits { maxFrameBytes?: number; maxBufferedBytes?: number }
interface ServerEvent { sequence: number; event: string; payload: unknown }
type EventHandler = (payload: unknown) => void

let csrfToken: string | null = null
let sessionPromise: Promise<void> | null = null
let eventSource: EventSource | null = null
let reconnectTimer: number | null = null
const eventHandlers = new Map<string, Set<EventHandler>>()

export const MAX_TERMINAL_INPUT_FRAME_BYTES = 64 * 1024
export const MAX_TERMINAL_INPUT_BATCH_BYTES = 256 * 1024

function requestId(): string { return crypto.randomUUID() }
function errorFromResponse(status: number, body: unknown): ApiError {
  if (body && typeof body === 'object') {
    const record = body as { code?: unknown; message?: unknown }
    if (typeof record.code === 'string' && typeof record.message === 'string') return new ApiError(record.code, record.message)
  }
  if (status === 401 || status === 403) return new ApiError('SESSION_UNAVAILABLE', '会话已失效，请从快捷方式重新打开 RemoteDeck。')
  return new ApiError('REQUEST_FAILED', `本地服务请求失败（HTTP ${String(status)}）。`)
}
async function responseBody(response: Response): Promise<unknown> {
  const text = await response.text(); if (!text) return null
  try { return JSON.parse(text) as unknown } catch { return text }
}
async function exchangeSession(): Promise<void> {
  const ticket = new URLSearchParams(window.location.hash.slice(1)).get('ticket')
  if (ticket) {
    window.history.replaceState(null, '', `${window.location.pathname}${window.location.search}`)
    const response = await fetch('/api/v1/auth/exchange', { method: 'POST', credentials: 'same-origin', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ ticket }) })
    const body = await responseBody(response); if (!response.ok) throw errorFromResponse(response.status, body)
    const session = body as Partial<AuthSession>
    if (typeof session.csrfToken !== 'string' || !session.csrfToken) throw new ApiError('INVALID_SESSION', '本地服务没有返回 CSRF 会话令牌。')
    csrfToken = session.csrfToken; return
  }
  const response = await fetch('/api/v1/auth/session', { credentials: 'same-origin' })
  const body = await responseBody(response); if (!response.ok) throw errorFromResponse(response.status, body)
  const session = body as Partial<AuthSession>
  if (typeof session.csrfToken !== 'string' || !session.csrfToken) throw new ApiError('INVALID_SESSION', '本地服务没有返回 CSRF 会话令牌。')
  csrfToken = session.csrfToken
}
async function refreshCsrfSession(): Promise<void> {
  const response = await fetch('/api/v1/auth/session', { credentials: 'same-origin' })
  const body = await responseBody(response); if (!response.ok) throw errorFromResponse(response.status, body)
  const session = body as Partial<AuthSession>
  if (typeof session.csrfToken !== 'string' || !session.csrfToken) throw new ApiError('INVALID_SESSION', '本地服务没有返回 CSRF 会话令牌。')
  csrfToken = session.csrfToken
  sessionPromise = Promise.resolve()
}
export function initializeSession(): Promise<void> {
  sessionPromise ??= exchangeSession().catch((error: unknown) => { sessionPromise = null; throw error })
  return sessionPromise
}
async function get<T>(path: string): Promise<T> {
  await initializeSession()
  const response = await fetch(path, { credentials: 'same-origin', headers: { 'X-RemoteDeck-CSRF': csrfToken ?? '' } })
  const body = await responseBody(response); if (!response.ok) throw errorFromResponse(response.status, body)
  return body as T
}
async function waitForOperation<T>(id: unknown): Promise<T> {
  if (typeof id !== 'string' || !id) throw new ApiError('INVALID_OPERATION', '本地服务没有返回长操作标识。')
  for (;;) {
    const operation = await get<OperationRecord>(`/api/v1/operations/${encodeURIComponent(id)}`)
    if (operation.state === 'completed') return operation.result as T
    if (operation.state === 'failed') throw new ApiError(operation.error?.code ?? 'OPERATION_FAILED', operation.error?.message ?? '后台操作失败。')
    await new Promise<void>((resolve) => window.setTimeout(resolve, 500))
  }
}
async function post<T>(command: string, args?: Record<string, unknown>, options?: { idempotent?: boolean; operation?: boolean }): Promise<T> {
  await initializeSession()
  const idempotencyKey = options?.idempotent ? requestId() : undefined
  const request = async (): Promise<{ response: Response; body: unknown }> => {
    const headers: Record<string, string> = { 'Content-Type': 'application/json', 'X-RemoteDeck-CSRF': csrfToken ?? '' }
    if (idempotencyKey) headers['Idempotency-Key'] = idempotencyKey
    const response = await fetch(`/api/v1/${command}`, { method: 'POST', credentials: 'same-origin', headers, body: JSON.stringify(args ?? {}) })
    return { response, body: await responseBody(response) }
  }
  let result = await request()
  // A simultaneous first launch can replace the browser's session cookie after this tab
  // obtained its CSRF token. The rejected request never enters admission, so one retry
  // with the same idempotency key is safe; other failures remain fail-closed.
  if (result.response.status === 403 && invalidCsrf(result.body)) {
    await refreshCsrfSession()
    result = await request()
  }
  const { response, body } = result; if (!response.ok) throw errorFromResponse(response.status, body)
  if (options?.operation && response.status === 202) return waitForOperation<T>((body as { operationId?: unknown }).operationId)
  return body as T
}

export function invalidCsrf(body: unknown): boolean {
  return Boolean(body && typeof body === 'object' && (body as { code?: unknown }).code === 'invalid_csrf')
}

function dispatch(event: string, payload: unknown): void { for (const handler of eventHandlers.get(event) ?? []) handler(payload) }
function closeEventSource(): void {
  if (reconnectTimer !== null) { window.clearTimeout(reconnectTimer); reconnectTimer = null }
  eventSource?.close(); eventSource = null
}
function ensureEventSource(): void {
  if (eventSource || eventHandlers.size === 0) return
  const source = new EventSource('/api/v1/events', { withCredentials: true }); eventSource = source
  source.onmessage = (message) => {
    try {
      if (typeof message.data !== 'string') throw new Error('invalid event data')
      const envelope = JSON.parse(message.data) as unknown as ServerEvent
      if (!Number.isSafeInteger(envelope.sequence) || typeof envelope.event !== 'string') throw new Error('invalid event')
      dispatch(envelope.event, envelope.payload)
    } catch { dispatch('resync', undefined) }
  }
  source.onerror = () => {
    source.close(); if (eventSource === source) eventSource = null
    if (eventHandlers.size > 0 && reconnectTimer === null) reconnectTimer = window.setTimeout(() => { reconnectTimer = null; dispatch('resync', undefined); ensureEventSource() }, 1_000)
  }
}
function subscribe(event: string, handler: EventHandler): () => void {
  const handlers = eventHandlers.get(event) ?? new Set<EventHandler>(); handlers.add(handler); eventHandlers.set(event, handlers); ensureEventSource()
  return () => { handlers.delete(handler); if (handlers.size === 0) eventHandlers.delete(event); if (eventHandlers.size === 0) closeEventSource() }
}

export function sendTerminalInput(socket: TerminalInputSocket, value: string, limits?: TerminalInputLimits): void {
  if (socket.readyState !== 1) throw new Error('安全终端输入通道尚未连接。')
  const maxFrameBytes = boundedInputLimit(limits?.maxFrameBytes, MAX_TERMINAL_INPUT_FRAME_BYTES)
  const maxBufferedBytes = boundedInputLimit(limits?.maxBufferedBytes, MAX_TERMINAL_INPUT_BATCH_BYTES)
  const bytes = new TextEncoder().encode(value); if (bytes.byteLength === 0) return
  if (bytes.byteLength > maxBufferedBytes) throw new Error(`单次终端输入不能超过 ${String(maxBufferedBytes / 1024)} KiB。`)
  if (socket.bufferedAmount + bytes.byteLength > maxBufferedBytes) throw new Error('安全终端输入通道繁忙，请稍后重试。')
  for (let offset = 0; offset < bytes.byteLength; offset += maxFrameBytes) socket.send(bytes.slice(offset, offset + maxFrameBytes).buffer)
}
function boundedInputLimit(value: number | undefined, fallback: number): number { return typeof value === 'number' && Number.isSafeInteger(value) && value > 0 ? Math.min(value, fallback) : fallback }

export const api = {
  bootstrap: (): Promise<BootstrapPayload> => post('bootstrap'), updateSettings: (patch: Partial<AppSettings>): Promise<AppSettings> => post('update_settings', { patch }, { idempotent: true }),
  pickLocalPath: (directory: boolean): Promise<string | null> => post('pick_local_path', { directory }, { idempotent: true, operation: true }), pickSavePath: (suggestedName: string): Promise<string | null> => post('pick_save_path', { suggestedName }, { idempotent: true, operation: true }), listOperations: (): Promise<OperationSummary[]> => get('/api/v1/operations'),
  saveHost: (draft: HostDraft): Promise<HostProfile> => post('save_host', { draft }, { idempotent: true }), deleteHost: (hostId: string): Promise<void> => post('delete_host', { hostId }, { idempotent: true, operation: true }), importSshConfig: (configPath: string): Promise<SshImportResult> => post('import_ssh_config', { configPath }, { idempotent: true }), scanHostKeys: (hostId: string): Promise<HostKeyCandidate[]> => post('scan_host_keys', { hostId }, { idempotent: true, operation: true }), acceptHostKey: (hostId: string, candidate: HostKeyCandidate): Promise<void> => post('accept_host_key', { hostId, candidate }, { idempotent: true, operation: true }), listHostKeys: (): Promise<HostKeyRecord[]> => post('list_host_keys', {}, { idempotent: true, operation: true }), removeHostKey: (recordId: string): Promise<void> => post('remove_host_key', { recordId }, { idempotent: true, operation: true }), testConnection: (hostId: string): Promise<ConnectionTestResult> => post('test_connection', { hostId }, { idempotent: true, operation: true }), listKeys: (): Promise<PrivateKeyRecord[]> => post('list_keys', {}, { idempotent: true, operation: true }), generateKey: (request: KeyGenerateRequest): Promise<KeyOperationResult> => post('generate_key', { request }, { idempotent: true, operation: true }), deployKey: (request: KeyDeployRequest): Promise<KeyOperationResult> => post('deploy_key', { request }, { idempotent: true, operation: true }),
  listTerminals: (): Promise<TerminalSnapshot[]> => post('list_terminals'), startTerminal: (hostId: string, rows: number, cols: number): Promise<TerminalSnapshot> => post('start_terminal', { hostId, rows, cols }, { idempotent: true }), reconnectTerminal: (sessionId: string, rows: number, cols: number): Promise<TerminalSnapshot> => post('reconnect_terminal', { sessionId, rows, cols }, { idempotent: true }), openTerminalInput: (sessionId: string): Promise<TerminalInputEndpoint> => post('open_terminal_input', { sessionId }, { idempotent: true }), resizeTerminal: (sessionId: string, rows: number, cols: number, generation: number, lease: number): Promise<void> => post('resize_terminal', { sessionId, rows, cols, generation, lease }, { idempotent: true }), closeTerminal: (sessionId: string): Promise<void> => post('close_terminal', { sessionId }, { idempotent: true }),
  listSftp: (hostId: string, path: string): Promise<SftpListing> => post('sftp_list', { hostId, path }, { idempotent: true, operation: true }), createSftpDirectory: (hostId: string, path: string): Promise<void> => post('sftp_create_directory', { hostId, path }, { idempotent: true, operation: true }), renameSftp: (hostId: string, sourcePath: string, destinationPath: string): Promise<void> => post('sftp_rename', { hostId, sourcePath, destinationPath }, { idempotent: true, operation: true }), deleteSftp: (hostId: string, path: string, recursive: boolean): Promise<void> => post('sftp_delete', { hostId, path, recursive }, { idempotent: true, operation: true }), listTransfers: (hostId?: string): Promise<TransferJob[]> => post('transfer_list', hostId ? { hostId } : undefined), upload: (request: TransferRequest): Promise<TransferJob> => post('transfer_upload', { request }, { idempotent: true }), download: (request: TransferRequest): Promise<TransferJob> => post('transfer_download', { request }, { idempotent: true }), cancelTransfer: (jobId: string): Promise<TransferJob> => post('transfer_cancel', { jobId }, { idempotent: true }), retryTransfer: (jobId: string): Promise<TransferJob> => post('transfer_retry', { jobId }, { idempotent: true }), showTransferInFolder: (jobId: string): Promise<void> => post('transfer_show_in_folder', { jobId }, { idempotent: true }),
  listTunnels: (hostId?: string): Promise<TunnelSnapshot[]> => post('list_tunnels', hostId ? { hostId } : undefined), saveTunnel: (draft: TunnelDraft): Promise<TunnelProfile> => post('save_tunnel', { draft }, { idempotent: true }), deleteTunnel: (tunnelId: string): Promise<void> => post('delete_tunnel', { tunnelId }, { idempotent: true }), startTunnel: (tunnelId: string): Promise<TunnelSnapshot> => post('start_tunnel', { tunnelId }, { idempotent: true }), stopTunnel: (tunnelId: string): Promise<TunnelSnapshot> => post('stop_tunnel', { tunnelId }, { idempotent: true }), restartTunnel: (tunnelId: string): Promise<TunnelSnapshot> => post('restart_tunnel', { tunnelId }, { idempotent: true }),
  listTelemetry: (): Promise<TelemetryStatus[]> => post('telemetry_list'), telemetryHistory: (hostId: string): Promise<TelemetrySnapshot[]> => post('telemetry_history', { hostId }), startTelemetry: (hostId: string): Promise<TelemetryStatus> => post('telemetry_start', { hostId }, { idempotent: true }), stopTelemetry: (hostId: string): Promise<TelemetryStatus> => post('telemetry_stop', { hostId }, { idempotent: true, operation: true }), signalProcess: (hostId: string, process: ProcessSnapshot, signal: 'TERM' | 'KILL'): Promise<void> => post('signal_process', { hostId, pid: process.pid, expectedStartTicks: process.startTicks, expectedUser: process.user, expectedCommand: process.command, signal }, { idempotent: true, operation: true }), probeBtop: (hostId: string): Promise<BtopStatus> => post('btop_probe', { hostId }, { idempotent: true, operation: true }), startBtopWatchdog: (hostId: string, rotationMinutes: number): Promise<BtopStatus> => post('btop_start_watchdog', { hostId, rotationMinutes }, { idempotent: true, operation: true }), stopBtopWatchdog: (hostId: string): Promise<BtopStatus> => post('btop_stop_watchdog', { hostId }, { idempotent: true, operation: true }),
  listCommands: (hostId?: string): Promise<CommandDefinition[]> => post('list_commands', hostId ? { hostId } : undefined), saveCommand: (draft: CommandDraft): Promise<CommandDefinition> => post('save_command', { draft }, { idempotent: true }), deleteCommand: (commandId: string, hostId: string): Promise<void> => post('delete_command', { commandId, hostId }, { idempotent: true }), analyzeCommand: (request: CommandRunRequest): Promise<CommandAnalysis> => post('analyze_command', { request }), runCommand: (request: CommandRunRequest): Promise<CommandJob> => post('run_command', { request }, { idempotent: true }), listCommandJobs: (hostId?: string): Promise<CommandJob[]> => post('list_command_jobs', hostId ? { hostId } : undefined), cancelCommand: (jobId: string): Promise<CommandJob> => post('cancel_command', { jobId }, { idempotent: true }),
  probeAgent: (hostId: string, agent: AgentKind): Promise<AgentStatus> => post('probe_agent', { hostId, agent }, { idempotent: true, operation: true }), agentSessionPlan: (request: AgentSessionRequest): Promise<AgentCommandPlan> => post('agent_session_plan', { request }), startAgentSession: (request: AgentSessionRequest): Promise<AgentSessionResult> => post('start_agent_session', { request }, { idempotent: true }), legacyPreview: (sourcePath: string): Promise<LegacyPreview> => post('legacy_preview', { sourcePath }, { idempotent: true, operation: true }), legacyApply: (request: LegacyApplyRequest): Promise<LegacyApplyResult> => post('legacy_apply', { request }, { idempotent: true, operation: true }), exportDiagnostics: (): Promise<DiagnosticsResult> => post('export_diagnostics', {}, { idempotent: true, operation: true }), shutdown: (): Promise<void> => post('shutdown', {}, { idempotent: true })
}
export const events = {
  terminal: (handler: (event: TerminalEvent) => void): (() => void) => subscribe('terminal-event', (payload) => handler(payload as TerminalEvent)), tunnel: (handler: (event: TunnelSnapshot) => void): (() => void) => subscribe('tunnel-event', (payload) => handler(payload as TunnelSnapshot)), transfer: (handler: (event: TransferEvent) => void): (() => void) => subscribe('transfer-event', (payload) => handler(payload as TransferEvent)), telemetry: (handler: (event: TelemetryEvent) => void): (() => void) => subscribe('telemetry-event', (payload) => handler(payload as TelemetryEvent)), command: (handler: (event: CommandEvent) => void): (() => void) => subscribe('command-event', (payload) => handler(payload as CommandEvent)), operation: (handler: (event: OperationSummary) => void): (() => void) => subscribe('operation-event', (payload) => handler(payload as OperationSummary)), resync: (handler: () => void): (() => void) => subscribe('resync', () => handler())
}
export function terminalWebSocketUrl(endpoint: TerminalInputEndpoint): string {
  const url = new URL(endpoint.url, window.location.href)
  const sameOrigin = new URL(url)
  sameOrigin.protocol = sameOrigin.protocol === 'wss:' ? 'https:' : 'http:'
  if (sameOrigin.origin !== window.location.origin) throw new ApiError('INVALID_TERMINAL_ENDPOINT', '本地服务返回了跨域终端地址，已拒绝连接。')
  url.protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'; return url.toString()
}
export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  if (typeof error === 'string') return error
  if (error && typeof error === 'object') { const candidate = error as { message?: unknown; error?: unknown }; if (typeof candidate.message === 'string') return candidate.message; if (typeof candidate.error === 'string') return candidate.error }
  return '操作失败。'
}
