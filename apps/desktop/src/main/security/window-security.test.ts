import { describe, expect, it } from 'vitest'
import { secureWindowOptions } from './window-security'

describe('secure BrowserWindow defaults', () => {
  it('isolates and sandboxes the renderer without Node integration', () => {
    const options = secureWindowOptions('C:\\app\\preload.mjs')
    expect(options.webPreferences).toMatchObject({
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      webSecurity: true,
      allowRunningInsecureContent: false,
      navigateOnDragDrop: false
    })
  })
})

