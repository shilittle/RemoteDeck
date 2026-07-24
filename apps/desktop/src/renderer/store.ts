import { create } from 'zustand'
import type { AppSettings, AppSettingsPatch } from '../protocol/settings'

export type Activity = 'hosts' | 'terminal' | 'files' | 'tunnels' | 'monitor' | 'commands' | 'settings'

interface AppState {
  activity: Activity
  settings: AppSettings | null
  appVersion: string
  loading: boolean
  saving: boolean
  error: string | null
  setActivity: (activity: Activity) => void
  bootstrap: () => Promise<void>
  updateSettings: (patch: AppSettingsPatch) => Promise<void>
}

export const useAppStore = create<AppState>((set) => ({
  activity: 'hosts',
  settings: null,
  appVersion: '',
  loading: true,
  saving: false,
  error: null,
  setActivity: (activity) => set({ activity }),
  bootstrap: async () => {
    set({ loading: true, error: null })
    try {
      const snapshot = await window.remoteDeck.app.bootstrap()
      set({ settings: snapshot.settings, appVersion: snapshot.appVersion, loading: false })
    } catch (error) {
      set({ error: getErrorMessage(error), loading: false })
    }
  },
  updateSettings: async (patch) => {
    set({ saving: true, error: null })
    try {
      const settings = await window.remoteDeck.settings.update(patch)
      set({ settings, saving: false })
    } catch (error) {
      set({ error: getErrorMessage(error), saving: false })
    }
  }
}))

function getErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : '发生未知错误'
}
