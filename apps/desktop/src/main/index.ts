import { join } from 'node:path'
import { readFile } from 'node:fs/promises'
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
import { TelemetryService } from '../core/telemetry/telemetry-service'
import { BtopService } from '../core/telemetry/btop-service'
import { CommandService } from '../core/commands/command-service'
import { CodexService } from '../core/commands/codex-service'
import type { ConnectionSnapshot } from '../protocol/ssh'
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
    const collectorPath = app.isPackaged ? join(process.resourcesPath, 'remote-collector', 'collector.py') : join(app.getAppPath(), '..', '..', 'packages', 'remote-collector', 'collector.py')
    const telemetry = new TelemetryService(connections, () => settings.get(), () => readFile(collectorPath, 'utf8'), mainLogger)
    const btop = new BtopService(connections, mainLogger)
    const commands = new CommandService(profiles, connections, terminals)
    const codex = new CodexService(profiles, connections, terminals)
    const onConnectionState = (snapshot: ConnectionSnapshot): void => {
      if (snapshot.state === 'online') {
        telemetry.wake(snapshot.hostId)
        btop.wake(snapshot.hostId)
        void profiles.get(snapshot.hostId).then((profile) => { if (profile.host.monitorEnabled && telemetry.status(snapshot.hostId).state === 'stopped') void telemetry.start(snapshot.hostId) }).catch(() => undefined)
      } else if (snapshot.state === 'idle') {
        telemetry.stop(snapshot.hostId)
        btop.stop(snapshot.hostId)
      } else if (snapshot.state === 'offline' || snapshot.state === 'failed') {
        telemetry.networkOffline(snapshot.hostId)
        btop.networkOffline(snapshot.hostId)
      }
    }
    connections.on('state', onConnectionState)
    mainWindow = createWindow()
    const ipcDependencies = { ipcMain, settings, hosts, profiles, connections, keys, terminals, sftp, transfers, tunnels, telemetry, btop, commands, codex, logger: mainLogger, appVersion: app.getVersion() }
    disposeRuntime = () => { connections.off('state', onConnectionState); transfers.cancelAll(); commands.cancelAll(); terminals.closeAll(); telemetry.stopAll(); btop.stopAll(); void tunnels.stopAll(); void connections.disconnectAll() }
    disposeIpc = registerIpcHandlers({ ...ipcDependencies, window: mainWindow })
    mainWindow.on('closed', () => { mainWindow = null })
    app.on('activate', () => {
      if (!mainWindow) {
        mainWindow = createWindow()
        disposeIpc?.()
        disposeIpc = registerIpcHandlers({ ...ipcDependencies, window: mainWindow })
      }
    })
    powerMonitor.on('suspend', () => { telemetry.suspend(); btop.suspend(); void tunnels.suspend() })
    powerMonitor.on('resume', () => { telemetry.resume(); btop.resume(); tunnels.resume() })
    void tunnels.restoreAutoStart()
    logger.info({ development }, 'RemoteDeck started')
  })
}

app.on('window-all-closed', () => app.quit())
app.on('before-quit', () => { disposeIpc?.(); disposeRuntime?.() })
