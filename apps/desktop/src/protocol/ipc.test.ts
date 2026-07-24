import { describe, expect, it } from 'vitest'
import { ipcContracts } from './ipc'

describe('narrow IPC contracts', () => {
  it('uses versioned channel names', () => {
    expect(Object.values(ipcContracts).every((contract) => contract.channel.startsWith('v1:'))).toBe(true)
  })

  it('rejects extra fields and invalid setting patches', () => {
    expect(ipcContracts.bootstrap.input.safeParse({ arbitrary: true }).success).toBe(false)
    expect(ipcContracts.settingsUpdate.input.safeParse({ terminalFontSize: 100 }).success).toBe(false)
    expect(ipcContracts.settingsUpdate.input.safeParse({ privateKey: 'secret' }).success).toBe(false)
  })
})

