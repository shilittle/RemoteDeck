import type { BrowserWindow, BrowserWindowConstructorOptions, Session } from 'electron'

export function secureWindowOptions(preload: string): BrowserWindowConstructorOptions {
  return {
    width: 1280,
    height: 760,
    minWidth: 960,
    minHeight: 640,
    show: false,
    backgroundColor: '#0d1117',
    autoHideMenuBar: true,
    webPreferences: {
      preload,
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      webSecurity: true,
      allowRunningInsecureContent: false,
      navigateOnDragDrop: false,
      spellcheck: false
    }
  }
}

export function hardenWindow(window: BrowserWindow): void {
  window.webContents.setWindowOpenHandler(() => ({ action: 'deny' }))
  window.webContents.on('will-navigate', (event) => event.preventDefault())
  window.webContents.on('will-attach-webview', (event) => event.preventDefault())
}

export function installSessionSecurity(session: Session, development: boolean): void {
  session.setPermissionRequestHandler((_webContents, _permission, callback) => callback(false))
  const script = development ? "script-src 'self' 'unsafe-eval' http://localhost:*" : "script-src 'self'"
  const connect = development ? "connect-src 'self' ws://localhost:* http://localhost:*" : "connect-src 'self'"
  const policy = `default-src 'none'; ${script}; style-src 'self'; img-src 'self' data:; font-src 'self'; ${connect}; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; object-src 'none'`
  session.webRequest.onHeadersReceived((details, callback) => {
    callback({ responseHeaders: { ...details.responseHeaders, 'Content-Security-Policy': [policy] } })
  })
}

