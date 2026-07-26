import type {
  AppSettings,
  BootstrapPayload,
  CommandResult,
  ConnectionTestResult,
  HostDraft,
  HostKeyCandidate,
  HostProfile,
  TerminalEvent,
  TerminalSnapshot,
  TunnelDraft,
  TunnelProfile,
  TunnelSnapshot
} from './types'

function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const runtime = window.__TAURI__
  if (!runtime?.core.invoke) {
    return Promise.reject(new Error('当前页面不在 Tauri 运行时中。请使用 pnpm dev 启动桌面应用。'))
  }
  return runtime.core.invoke<T>(command, args)
}

export const api = {
  bootstrap: (): Promise<BootstrapPayload> => invoke('bootstrap'),
  saveHost: (draft: HostDraft): Promise<HostProfile> => invoke('save_host', { draft }),
  deleteHost: (hostId: string): Promise<void> => invoke('delete_host', { hostId }),
  scanHostKeys: (hostId: string): Promise<HostKeyCandidate[]> => invoke('scan_host_keys', { hostId }),
  acceptHostKey: (hostId: string, candidate: HostKeyCandidate): Promise<void> =>
    invoke('accept_host_key', { hostId, candidate }),
  testConnection: (hostId: string): Promise<ConnectionTestResult> => invoke('test_connection', { hostId }),
  runCommand: (hostId: string, command: string, workingDirectory?: string): Promise<CommandResult> =>
    invoke('run_command', { hostId, command, workingDirectory: workingDirectory || null }),
  startTerminal: (hostId: string, rows: number, cols: number): Promise<TerminalSnapshot> =>
    invoke('start_terminal', { hostId, rows, cols }),
  writeTerminal: (sessionId: string, data: string): Promise<void> =>
    invoke('write_terminal', { sessionId, data }),
  resizeTerminal: (sessionId: string, rows: number, cols: number): Promise<void> =>
    invoke('resize_terminal', { sessionId, rows, cols }),
  closeTerminal: (sessionId: string): Promise<void> => invoke('close_terminal', { sessionId }),
  saveTunnel: (draft: TunnelDraft): Promise<TunnelProfile> => invoke('save_tunnel', { draft }),
  deleteTunnel: (tunnelId: string): Promise<void> => invoke('delete_tunnel', { tunnelId }),
  startTunnel: (tunnelId: string): Promise<TunnelSnapshot> => invoke('start_tunnel', { tunnelId }),
  stopTunnel: (tunnelId: string): Promise<void> => invoke('stop_tunnel', { tunnelId }),
  updateSettings: (patch: Partial<AppSettings>): Promise<AppSettings> => invoke('update_settings', { patch })
}

export async function listenTerminal(handler: (event: TerminalEvent) => void): Promise<() => void> {
  const runtime = window.__TAURI__
  if (!runtime?.event.listen) throw new Error('Tauri event API is unavailable.')
  return runtime.event.listen<TerminalEvent>('terminal-event', ({ payload }) => handler(payload))
}

export async function listenTunnel(handler: (event: TunnelSnapshot) => void): Promise<() => void> {
  const runtime = window.__TAURI__
  if (!runtime?.event.listen) throw new Error('Tauri event API is unavailable.')
  return runtime.event.listen<TunnelSnapshot>('tunnel-event', ({ payload }) => handler(payload))
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
