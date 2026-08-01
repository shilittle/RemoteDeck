import { create } from 'zustand'
import { api, errorMessage } from './api'
import type {
  AppSettings,
  CommandJob,
  HostDraft,
  HostProfile,
  RuntimeCapabilities,
  TelemetryEvent,
  TelemetryStatus,
  TransferJob,
  TunnelDraft,
  TunnelProfile,
  TunnelSnapshot
} from './types'

export type Activity = 'hosts' | 'terminal' | 'files' | 'tunnels' | 'monitor' | 'commands' | 'settings'

export const defaultSettings: AppSettings = {
  schemaVersion: 2,
  terminalFontFamily: 'Cascadia Mono, Consolas, monospace',
  terminalFontSize: 14,
  telemetryIntervalSeconds: 3,
  telemetryRetentionMinutes: 30,
  downloadDirectory: '',
  autoReconnect: true,
  closeToTray: true,
  launchAtLogin: false,
  btopWatchdogEnabled: false,
  btopRotationMinutes: 15,
  logLevel: 'info',
  onboardingCompleted: false,
  defaultAgent: 'codex'
}

interface AppStore {
  activity: Activity
  loading: boolean
  busy: boolean
  error: string | null
  appVersion: string
  settings: AppSettings
  capabilities: RuntimeCapabilities | null
  hosts: HostProfile[]
  tunnels: TunnelProfile[]
  tunnelStates: Partial<Record<string, TunnelSnapshot>>
  transfers: TransferJob[]
  telemetryStatuses: Partial<Record<string, TelemetryStatus>>
  commandJobs: CommandJob[]
  selectedHostId: string | null
  setActivity: (activity: Activity) => void
  selectHost: (hostId: string) => void
  clearError: () => void
  reportError: (error: unknown) => void
  bootstrap: () => Promise<void>
  saveHost: (draft: HostDraft) => Promise<HostProfile>
  deleteHost: (hostId: string) => Promise<void>
  replaceHosts: (hosts: HostProfile[]) => void
  saveTunnel: (draft: TunnelDraft) => Promise<TunnelProfile>
  deleteTunnel: (tunnelId: string) => Promise<void>
  applyTunnelState: (snapshot: TunnelSnapshot) => void
  setTransfers: (jobs: TransferJob[]) => void
  applyTransfer: (job: TransferJob) => void
  setTelemetryStatuses: (statuses: TelemetryStatus[]) => void
  applyTelemetry: (event: TelemetryEvent) => void
  setCommandJobs: (jobs: CommandJob[]) => void
  applyCommandJob: (job: CommandJob) => void
  updateSettings: (patch: Partial<AppSettings>) => Promise<void>
}

function replaceById<T extends { id: string }>(items: T[], replacement: T): T[] {
  return items.some((item) => item.id === replacement.id)
    ? items.map((item) => item.id === replacement.id ? replacement : item)
    : [...items, replacement]
}

const MAX_RETAINED_TASKS = 512
const MAX_RETAINED_COMMAND_OUTPUT_BYTES = 16 * 1024 * 1024

function retainBoundedTasks<T>(items: T[], isTerminal: (item: T) => boolean): T[] {
  const retained = [...items]
  while (retained.length > MAX_RETAINED_TASKS) {
    const terminalIndex = retained.findIndex(isTerminal)
    if (terminalIndex === -1) break
    retained.splice(terminalIndex, 1)
  }
  return retained
}

function replaceTransfer(items: TransferJob[], replacement: TransferJob): TransferJob[] {
  return retainBoundedTasks(
    replaceById(items, replacement),
    (job) => ['completed', 'cancelled', 'failed'].includes(job.state)
  )
}

function replaceCommandJob(items: CommandJob[], replacement: CommandJob): CommandJob[] {
  return retainCommandOutputBudget(retainBoundedTasks(
    replaceById(items, replacement),
    commandIsTerminal
  ))
}

function commandIsTerminal(job: CommandJob): boolean {
  return ['completed', 'cancelled', 'failed'].includes(job.state)
}

function retainCommandOutputBudget(items: CommandJob[]): CommandJob[] {
  const retained = [...items]
  let bytes = retained.reduce((total, job) => total + job.stdout.length + job.stderr.length + (job.error?.length ?? 0), 0)
  while (bytes > MAX_RETAINED_COMMAND_OUTPUT_BYTES) {
    const terminalIndex = retained.findIndex(commandIsTerminal)
    if (terminalIndex === -1) break
    const [removed] = retained.splice(terminalIndex, 1)
    bytes -= removed.stdout.length + removed.stderr.length + (removed.error?.length ?? 0)
  }
  return retained
}

export function mergeTransferSnapshots(current: TransferJob[], incoming: TransferJob[]): TransferJob[] {
  const merged = new Map(current.map((job) => [job.id, job]))
  for (const job of incoming) {
    const previous = merged.get(job.id)
    if (!previous || Date.parse(job.updatedAt) >= Date.parse(previous.updatedAt)) merged.set(job.id, job)
  }
  return retainBoundedTasks(
    [...merged.values()],
    (job) => ['completed', 'cancelled', 'failed'].includes(job.state)
  )
}

export function mergeCommandSnapshots(current: CommandJob[], incoming: CommandJob[]): CommandJob[] {
  const merged = new Map(current.map((job) => [job.id, job]))
  for (const job of incoming) {
    const previous = merged.get(job.id)
    if (!previous || commandProgress(job) >= commandProgress(previous)) merged.set(job.id, job)
  }
  return retainCommandOutputBudget(retainBoundedTasks(
    [...merged.values()],
    commandIsTerminal
  ))
}

export function mergeTunnelSnapshot(
  current: Partial<Record<string, TunnelSnapshot>>,
  incoming: TunnelSnapshot
): Partial<Record<string, TunnelSnapshot>> {
  const previous = current[incoming.tunnelId]
  if (previous && incoming.revision < previous.revision) return current
  const merged = previous && incoming.logs === undefined
    ? { ...incoming, logs: previous.logs }
    : incoming
  return { ...current, [incoming.tunnelId]: merged }
}

export function mergeTelemetryStatuses(
  current: Partial<Record<string, TelemetryStatus>>,
  incoming: TelemetryStatus[]
): Partial<Record<string, TelemetryStatus>> {
  let merged = current
  for (const status of incoming) {
    const previous = merged[status.hostId]
    if (previous && status.revision < previous.revision) continue
    if (merged === current) merged = { ...current }
    merged[status.hostId] = status
  }
  return merged
}

function commandProgress(job: CommandJob): number {
  const rank: Record<CommandJob['state'], number> = { queued: 0, running: 1, cancelling: 2, completed: 3, failed: 3, cancelled: 3 }
  return rank[job.state] * 10_000_000 + job.stdout.length + job.stderr.length + (job.error?.length ?? 0)
}

function normalizeSettings(settings: Partial<AppSettings> | null | undefined): AppSettings {
  return { ...defaultSettings, ...settings }
}

export const useAppStore = create<AppStore>((set, get) => ({
  activity: 'hosts',
  loading: true,
  busy: false,
  error: null,
  appVersion: '',
  settings: defaultSettings,
  capabilities: null,
  hosts: [],
  tunnels: [],
  tunnelStates: {},
  transfers: [],
  telemetryStatuses: {},
  commandJobs: [],
  selectedHostId: null,
  setActivity: (activity) => set({ activity }),
  selectHost: (selectedHostId) => set({ selectedHostId }),
  clearError: () => set({ error: null }),
  reportError: (error) => set({ error: errorMessage(error) }),
  bootstrap: async () => {
    set({ loading: true, error: null })
    try {
      const payload = await api.bootstrap()
      const selected = get().selectedHostId
      set({
        loading: false,
        appVersion: payload.appVersion,
        settings: normalizeSettings(payload.settings),
        capabilities: payload.capabilities,
        hosts: payload.hosts,
        tunnels: payload.tunnels,
        selectedHostId: selected && payload.hosts.some((host) => host.id === selected)
          ? selected
          : payload.hosts[0]?.id ?? null
      })
    } catch (error) {
      set({ loading: false, error: errorMessage(error) })
    }
  },
  saveHost: async (draft) => {
    set({ busy: true, error: null })
    try {
      const host = await api.saveHost(draft)
      set((state) => ({ busy: false, hosts: replaceById(state.hosts, host), selectedHostId: host.id }))
      return host
    } catch (error) {
      set({ busy: false, error: errorMessage(error) })
      throw error
    }
  },
  deleteHost: async (hostId) => {
    set({ busy: true, error: null })
    try {
      await api.deleteHost(hostId)
      set((state) => {
        const hosts = state.hosts.filter((host) => host.id !== hostId)
        return {
          busy: false,
          hosts,
          tunnels: state.tunnels.filter((tunnel) => tunnel.hostId !== hostId),
          transfers: state.transfers.filter((job) => job.hostId !== hostId),
          commandJobs: state.commandJobs.filter((job) => job.hostId !== hostId),
          selectedHostId: state.selectedHostId === hostId ? hosts[0]?.id ?? null : state.selectedHostId
        }
      })
    } catch (error) {
      set({ busy: false, error: errorMessage(error) })
      throw error
    }
  },
  replaceHosts: (hosts) => set((state) => ({
    hosts,
    selectedHostId: state.selectedHostId && hosts.some((host) => host.id === state.selectedHostId)
      ? state.selectedHostId
      : hosts[0]?.id ?? null
  })),
  saveTunnel: async (draft) => {
    set({ busy: true, error: null })
    try {
      const tunnel = await api.saveTunnel(draft)
      set((state) => ({ busy: false, tunnels: replaceById(state.tunnels, tunnel) }))
      return tunnel
    } catch (error) {
      set({ busy: false, error: errorMessage(error) })
      throw error
    }
  },
  deleteTunnel: async (tunnelId) => {
    set({ busy: true, error: null })
    try {
      await api.deleteTunnel(tunnelId)
      set((state) => {
        const tunnelStates = Object.fromEntries(Object.entries(state.tunnelStates).filter(([id]) => id !== tunnelId))
        return {
          busy: false,
          tunnels: state.tunnels.filter((tunnel) => tunnel.id !== tunnelId),
          tunnelStates
        }
      })
    } catch (error) {
      set({ busy: false, error: errorMessage(error) })
      throw error
    }
  },
  applyTunnelState: (snapshot) => set((state) => {
    const tunnelStates = mergeTunnelSnapshot(state.tunnelStates, snapshot)
    if (tunnelStates === state.tunnelStates) return {}
    return {
      tunnelStates,
      tunnels: snapshot.profile ? replaceById(state.tunnels, snapshot.profile) : state.tunnels
    }
  }),
  setTransfers: (transfers) => set((state) => ({ transfers: mergeTransferSnapshots(state.transfers, transfers) })),
  applyTransfer: (job) => set((state) => ({ transfers: replaceTransfer(state.transfers, job) })),
  setTelemetryStatuses: (statuses) => set((state) => ({
    telemetryStatuses: mergeTelemetryStatuses(state.telemetryStatuses, statuses)
  })),
  applyTelemetry: (event) => set((state) => ({
    telemetryStatuses: mergeTelemetryStatuses(state.telemetryStatuses, [event.status])
  })),
  setCommandJobs: (commandJobs) => set((state) => ({ commandJobs: mergeCommandSnapshots(state.commandJobs, commandJobs) })),
  applyCommandJob: (job) => set((state) => ({ commandJobs: replaceCommandJob(state.commandJobs, job) })),
  updateSettings: async (patch) => {
    set({ busy: true, error: null })
    try {
      const settings = normalizeSettings(await api.updateSettings(patch))
      set({ busy: false, settings })
    } catch (error) {
      set({ busy: false, error: errorMessage(error) })
      throw error
    }
  }
}))
