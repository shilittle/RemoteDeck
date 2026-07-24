import { join } from 'node:path'
import { app, BrowserWindow, ipcMain, powerMonitor, session } from 'electron'
import { SettingsService } from '../core/settings-service'
import { HostService } from '../core/hosts/host-service'
import { ProfileRepository } from '../core/hosts/profile-repository'
import { OpenSshConfigManager } from '../core/ssh-config/open-ssh-config'
import { SshConnectionManager } from '../core/ssh/connection-manager'
import { KeyService } from '../core/keys/key-service'
import { TerminalService } from '../core/terminal/terminal-service'
import { SftpService } from '../core/sftp/sftp-service'
import { TransferService } from '../core/sftp/transfer-service'
import { TunnelService } from '../core/tunnels/tunnel-service'
import { registerIpcHandlers } from './ipc/register-ipc'
import { categoryLogger, createAppLogger } from './logging/logger'
import { hardenWindow, installSessionSecurity, secureWindowOptions } from './security/window-security'

let mainWindow: BrowserWindow | null = null
let disposeIpc: (() => void) | undefined
let disposeRuntime: (() => void) | undefined

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
    const terminals = new TerminalService(connections, profiles)
    const sftp = new SftpService(connections)
    const transfers = new TransferService(sftp)
    const mainLogger = categoryLogger(logger, 'main')
    const tunnels = new TunnelService(profiles, connections, mainLogger)
    mainWindow = createWindow()
    const ipcDependencies = { ipcMain, settings, hosts, profiles, connections, keys, terminals, sftp, transfers, tunnels, logger: mainLogger, appVersion: app.getVersion() }
    disposeRuntime = () => { transfers.cancelAll(); terminals.closeAll(); void tunnels.stopAll(); void connections.disconnectAll() }
    disposeIpc = registerIpcHandlers({ ...ipcDependencies, window: mainWindow })
    mainWindow.on('closed', () => { mainWindow = null })
    app.on('activate', () => {
      if (!mainWindow) {
        mainWindow = createWindow()
        disposeIpc?.()
        disposeIpc = registerIpcHandlers({ ...ipcDependencies, window: mainWindow })
      }
    })
    powerMonitor.on('suspend', () => { void tunnels.suspend() })
    powerMonitor.on('resume', () => { tunnels.resume() })
    void tunnels.restoreAutoStart()
    logger.info({ development }, 'RemoteDeck started')
  })
}

app.on('window-all-closed', () => app.quit())
app.on('before-quit', () => { disposeIpc?.(); disposeRuntime?.() })
