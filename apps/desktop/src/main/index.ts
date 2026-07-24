import { join } from 'node:path'
import { readFile } from 'node:fs/promises'
import { app, BrowserWindow, ipcMain, Menu, nativeImage, powerMonitor, session, Tray } from 'electron'
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
import { LegacyMigrationService } from '../core/migration/legacy-migration-service'
import type { ConnectionSnapshot } from '../protocol/ssh'
import type { AppSettings } from '../protocol/settings'
import { registerIpcHandlers } from './ipc/register-ipc'
import { categoryLogger, createAppLogger } from './logging/logger'
import { hardenWindow, installSessionSecurity, secureWindowOptions } from './security/window-security'

let mainWindow: BrowserWindow | null = null
let disposeIpc: (() => void) | undefined
let disposeRuntime: (() => void) | undefined
let tray: Tray | null = null
let isQuitting = false
let closeToTray = true
let launchHidden = process.argv.includes('--hidden')

const e2eUserData = process.env['REMOTEDECK_E2E_USER_DATA']
if (!app.isPackaged && e2eUserData) app.setPath('userData', e2eUserData)

function createWindow(): BrowserWindow {
  const window = new BrowserWindow(secureWindowOptions(join(__dirname, '../preload/index.cjs')))
  hardenWindow(window)
  const shouldShow = !launchHidden
  launchHidden = false
  if (shouldShow) window.once('ready-to-show', () => window.show())
  if (process.env['ELECTRON_RENDERER_URL']) {
    void window.loadURL(process.env['ELECTRON_RENDERER_URL'])
  } else {
    void window.loadFile(join(__dirname, '../renderer/index.html'))
  }
  return window
}

function showMainWindow(): void {
  if (!mainWindow) return
  if (mainWindow.isMinimized()) mainWindow.restore()
  mainWindow.show()
  mainWindow.focus()
}

function bindCloseToTray(window: BrowserWindow): void {
  window.on('close', (event) => {
    if (isQuitting || !closeToTray) return
    event.preventDefault()
    window.hide()
  })
}

function createTray(): Tray {
  const instance = new Tray(trayImage())
  instance.setToolTip('RemoteDeck')
  instance.setContextMenu(Menu.buildFromTemplate([
    { label: '显示 RemoteDeck', click: showMainWindow },
    { type: 'separator' },
    { label: '完全退出', click: () => { isQuitting = true; app.quit() } }
  ]))
  instance.on('click', showMainWindow)
  return instance
}

function trayImage(): Electron.NativeImage {
  const size = 16
  const bitmap = Buffer.alloc(size * size * 4)
  const white = new Set(['4,3', '5,3', '6,3', '7,3', '8,3', '9,3', '10,4', '10,5', '10,6', '9,7', '8,7', '7,7', '9,8', '10,9', '11,10', '5,4', '5,5', '5,6', '5,7', '5,8', '5,9', '5,10', '5,11'])
  for (let y = 0; y < size; y += 1) for (let x = 0; x < size; x += 1) {
    const offset = (y * size + x) * 4
    const foreground = white.has(`${String(x)},${String(y)}`)
    bitmap[offset] = foreground ? 255 : 247
    bitmap[offset + 1] = foreground ? 255 : 129
    bitmap[offset + 2] = foreground ? 255 : 47
    bitmap[offset + 3] = x < 1 || y < 1 || x > 14 || y > 14 ? 0 : 255
  }
  return nativeImage.createFromBitmap(bitmap, { width: size, height: size, scaleFactor: 1 })
}

function applyDesktopSettings(settings: AppSettings): void {
  closeToTray = settings.closeToTray
  app.setLoginItemSettings({ openAtLogin: settings.launchAtLogin, args: ['--hidden'] })
}

if (!app.requestSingleInstanceLock()) {
  app.quit()
} else {
  app.on('second-instance', () => {
    showMainWindow()
  })

  void app.whenReady().then(() => {
    const development = !app.isPackaged
    installSessionSecurity(session.defaultSession, development)
    const logger = createAppLogger(join(app.getPath('userData'), 'logs'))
    const settings = new SettingsService(join(app.getPath('userData'), 'settings.json'))
    settings.on('updated', applyDesktopSettings)
    void settings.get().then(applyDesktopSettings)
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
    const legacy = new LegacyMigrationService(profiles, hosts, settings)
    const onConnectionState = (snapshot: ConnectionSnapshot): void => {
      if (snapshot.state === 'online') {
        telemetry.wake(snapshot.hostId)
        btop.wake(snapshot.hostId)
        void profiles.get(snapshot.hostId).then((profile) => { if (profile.host.monitorEnabled && telemetry.status(snapshot.hostId).state === 'stopped') void telemetry.start(snapshot.hostId) }).catch(() => undefined)
        void settings.get().then((current) => { if (current.btopWatchdogEnabled) void btop.start(snapshot.hostId, current.btopRotationMinutes) }).catch(() => undefined)
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
    bindCloseToTray(mainWindow)
    tray = createTray()
    const ipcDependencies = { ipcMain, settings, hosts, profiles, connections, keys, terminals, sftp, transfers, tunnels, telemetry, btop, commands, codex, legacy, logger: mainLogger, appVersion: app.getVersion() }
    disposeRuntime = () => { settings.off('updated', applyDesktopSettings); connections.off('state', onConnectionState); transfers.cancelAll(); commands.cancelAll(); terminals.closeAll(); telemetry.stopAll(); btop.stopAll(); void tunnels.stopAll(); void connections.disconnectAll() }
    disposeIpc = registerIpcHandlers({ ...ipcDependencies, window: mainWindow })
    mainWindow.on('closed', () => { mainWindow = null })
    app.on('activate', () => {
      if (!mainWindow) {
        mainWindow = createWindow()
        bindCloseToTray(mainWindow)
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
app.on('before-quit', () => { isQuitting = true; tray?.destroy(); tray = null; disposeIpc?.(); disposeRuntime?.() })
