import { join } from 'node:path'
import { app, BrowserWindow, ipcMain, session } from 'electron'
import { SettingsService } from '../core/settings-service'
import { HostService } from '../core/hosts/host-service'
import { ProfileRepository } from '../core/hosts/profile-repository'
import { OpenSshConfigManager } from '../core/ssh-config/open-ssh-config'
import { SshConnectionManager } from '../core/ssh/connection-manager'
import { KeyService } from '../core/keys/key-service'
import { registerIpcHandlers } from './ipc/register-ipc'
import { categoryLogger, createAppLogger } from './logging/logger'
import { hardenWindow, installSessionSecurity, secureWindowOptions } from './security/window-security'

let mainWindow: BrowserWindow | null = null
let disposeIpc: (() => void) | undefined

const e2eUserData = process.env['REMOTEDECK_E2E_USER_DATA']
if (!app.isPackaged && e2eUserData) app.setPath('userData', e2eUserData)

function createWindow(): BrowserWindow {
  const window = new BrowserWindow(secureWindowOptions(join(__dirname, '../preload/index.cjs')))
  hardenWindow(window)
  window.once('ready-to-show', () => window.show())
  if (process.env['ELECTRON_RENDERER_URL']) {
    void window.loadURL(process.env['ELECTRON_RENDERER_URL'])
  } else {
    void window.loadFile(join(__dirname, '../renderer/index.html'))
  }
  return window
}

if (!app.requestSingleInstanceLock()) {
  app.quit()
} else {
  app.on('second-instance', () => {
    if (mainWindow) {
      if (mainWindow.isMinimized()) mainWindow.restore()
      mainWindow.show()
      mainWindow.focus()
    }
  })

  void app.whenReady().then(() => {
    const development = !app.isPackaged
    installSessionSecurity(session.defaultSession, development)
    const logger = createAppLogger(join(app.getPath('userData'), 'logs'))
    const settings = new SettingsService(join(app.getPath('userData'), 'settings.json'))
    const profiles = new ProfileRepository(join(app.getPath('userData'), 'profiles.json'))
    const connections = new SshConnectionManager(profiles)
    const openSshConfig = new OpenSshConfigManager(profiles)
    const hosts = new HostService(profiles, connections, openSshConfig, settings)
    const keys = new KeyService(connections, profiles, () => hosts.syncManagedConfig())
    mainWindow = createWindow()
    const ipcDependencies = { ipcMain, settings, hosts, profiles, connections, keys, logger: categoryLogger(logger, 'main'), appVersion: app.getVersion() }
    disposeIpc = registerIpcHandlers({ ...ipcDependencies, window: mainWindow })
    mainWindow.on('closed', () => { mainWindow = null })
    app.on('activate', () => {
      if (!mainWindow) {
        mainWindow = createWindow()
        disposeIpc?.()
        disposeIpc = registerIpcHandlers({ ...ipcDependencies, window: mainWindow })
      }
    })
    logger.info({ development }, 'RemoteDeck started')
  })
}

app.on('window-all-closed', () => app.quit())
app.on('before-quit', () => disposeIpc?.())
