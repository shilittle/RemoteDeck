import type { RemoteDeckApi } from '../protocol/ipc'

declare global {
  interface Window {
    remoteDeck: RemoteDeckApi
  }
}

export {}

