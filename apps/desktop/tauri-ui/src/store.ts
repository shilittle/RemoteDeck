import { create } from 'zustand'
import { api, errorMessage } from './api'
import type {
  AppSettings,
  HostDraft,
  HostProfile,
  RuntimeCapabilities,
  TunnelDraft,
  TunnelProfile,
  TunnelSnapshot
} from './types'

export type Activity = 'hosts' | 'terminal' | 'tunnels' | 'commands' | 'settings'

interface AppStore {
  activity: Activity
  loading: boolean
  busy: boolean
  error: string | null
  appVersion: string
  settings: AppSettings | null
  capabilities: RuntimeCapabilities | null
  hosts: HostProfile[]
  tunnels: TunnelProfile[]
  tunnelStates: Record<string, TunnelSnapshot>
  selectedHostId: string | null
  setActivity(activity: Activity): void
  selectHost(hostId: string): void
  clearError(): void
  reportError(error: unknown): void
  bootstrap(): Promise<void>
  saveHost(draft: HostDraft): Promise<HostProfile>
  deleteHost(hostId: string): Promise<void>
  saveTunnel(draft: TunnelDraft): Promise<TunnelProfile>
  deleteTunnel(tunnelId: string): Promise<void>
  applyTunnelState(snapshot: TunnelSnapshot): void
  updateSettings(patch: Partial<AppSettings>): Promise<void>
}

function replaceById<T extends { id: string }>(items: T[], replacement: T): T[] {
  return items.some((item) => item.id === replacement.id)
    ? items.map((item) => item.id === replacement.id ? replacement : item)
    : [...items, replacement]
}

export const useAppStore = create<AppStore>((set, get) => ({
  activity: 'hosts',
  loading: true,
  busy: false,
  error: null,
  appVersion: '',
  settings: null,
  capabilities: null,
  hosts: [],
  tunnels: [],
  tunnelStates: {},
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
        settings: payload.settings,
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
          selectedHostId: state.selectedHostId === hostId ? hosts[0]?.id ?? null : state.selectedHostId
        }
      })
    } catch (error) {
      set({ busy: false, error: errorMessage(error) })
      throw error
    }
  },
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
      set((state) => ({
        busy: false,
        tunnels: state.tunnels.filter((tunnel) => tunnel.id !== tunnelId),
        tunnelStates: Object.fromEntries(Object.entries(state.tunnelStates).filter(([id]) => id !== tunnelId))
      }))
    } catch (error) {
      set({ busy: false, error: errorMessage(error) })
      throw error
    }
  },
  applyTunnelState: (snapshot) => set((state) => ({
    tunnelStates: { ...state.tunnelStates, [snapshot.tunnelId]: snapshot }
  })),
  updateSettings: async (patch) => {
    set({ busy: true, error: null })
    try {
      const settings = await api.updateSettings(patch)
      set({ busy: false, settings })
    } catch (error) {
      set({ busy: false, error: errorMessage(error) })
      throw error
    }
  }
}))
