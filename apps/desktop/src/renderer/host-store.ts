import { create } from 'zustand'
import type { ConnectionSnapshot, HostListItem } from '../protocol/ssh'

interface HostState {
  items: HostListItem[]
  selectedId: string | null
  connection: ConnectionSnapshot | null
  loading: boolean
  error: string | null
  load: () => Promise<void>
  select: (hostId: string) => void
  applyConnection: (snapshot: ConnectionSnapshot) => void
  clearError: () => void
}

export const useHostStore = create<HostState>((set) => ({
  items: [],
  selectedId: null,
  connection: null,
  loading: false,
  error: null,
  load: async () => {
    set({ loading: true, error: null })
    try {
      const items = await window.remoteDeck.hosts.list()
      set((state) => ({ items, loading: false, selectedId: state.selectedId && items.some((item) => item.host.id === state.selectedId) ? state.selectedId : items[0]?.host.id ?? null }))
    } catch (error) {
      set({ loading: false, error: errorMessage(error) })
    }
  },
  select: (selectedId) => set({ selectedId, connection: null }),
  applyConnection: (connection) => set((state) => ({ connection, items: state.items.map((item) => item.host.id === connection.hostId ? { ...item, state: connection.state, ...(connection.errorMessage ? { lastError: connection.errorMessage } : {}) } : item) })),
  clearError: () => set({ error: null })
}))

function errorMessage(error: unknown): string { return error instanceof Error ? error.message : '主机操作失败' }
