# Onboarding, migration, tasks, and tray lifecycle

## First-run path

When `onboardingCompleted` is false, RemoteDeck opens a first-run guide instead of requiring a configuration file. The guide routes a new user to either add a Linux host or preview a LabPulse SSH configuration. A progress card then points to the next real action: save the host, confirm its SSH fingerprint, authenticate, and open the Commands/Codex workspace. The guide can be completed or skipped explicitly and never stores an SSH password.

The existing host and Codex panels remain the actual implementation behind the guide; onboarding does not use mock hosts or a separate demo path. Electron E2E covers the full UI route through a real in-process SSH server and a mocked external Codex account boundary.

## LabPulse SSH migration

The Settings page accepts a user-selected legacy `config.json`. Main limits the file to 4 MiB, parses a bounded Zod schema, hashes the original bytes with SHA-256, and returns a preview before any mutation. The renderer has no general file-reading capability.

Mappings are:

| Legacy value | RemoteDeck value |
| --- | --- |
| `sshHost` | Host alias; the user must confirm hostname/IP, username, port, workspace, and auth metadata |
| `forwardHost` plus `forward` | A RemoteForward on the imported host |
| `telemetryIntervalSeconds` | Telemetry sampling setting |
| `btop.enabled`, `autoRestart`, `restartCycleSeconds` | btop watchdog enabled/rotation settings |
| `presets` | Host-scoped command presets with safe→L0 and danger→L2 conversion |
| `dangerousCommandPatterns` | Persisted legacy risk rules that may only raise command risk; unsafe regex constructs are ignored at execution but retained for audit |
| `releaseCommand` | A visible, disabled `legacyCleanupHook` |

The release command is never authorized by migration. The preview displays it verbatim with a high-risk warning; enabling it still requires the separate explicit authorization in the tunnel editor. Imports are keyed by source hash, so reopening or applying the same bytes produces a duplicate result without creating another host, tunnel, or preset.

## Task center and recovery

The bottom task center aggregates transfer jobs, command jobs, tunnels, and failed/offline hosts. Event streams update it without broad renderer subscriptions. It offers owned-resource actions only: cancel/retry a transfer, cancel a command exec channel, open the related command/tunnel view, or return to the host authentication view. It never kills a process merely because a port or PID looks related.

The renderer is wrapped in an error boundary with an explicit reload action. Feature errors remain local where recovery is possible; failed resources are also surfaced in the task center.

## Shortcuts

- `Ctrl+1` … `Ctrl+7`: Hosts, Terminal, Files, Tunnels, Monitor, Commands, Settings.
- `Ctrl+Shift+T`: open the terminal workspace and create a real terminal tab.
- `Ctrl+J`: toggle the task center.
- `Ctrl+,`: open Settings.

Text fields retain ordinary editing shortcuts; navigation shortcuts are ignored while an input, textarea, or select owns focus.

## Tray and process lifetime

RemoteDeck creates a Windows tray item with Show and Fully Quit commands. With “close to tray” enabled, a window close hides the BrowserWindow and leaves main/core resources alive. The E2E test closes the window and confirms that the authenticated SSH state remains online. A tray click or second-instance launch restores and focuses the window.

Fully Quit sets the quitting state before closing windows, then releases transfer jobs, command channels, terminals, telemetry, btop watchdogs, tunnels, and SSH clients. Closing to tray does not promise survival across user sign-out, process termination, reboot, or an OS crash. “Launch at login” uses Electron's Windows login-item setting and starts the first window hidden; the tray remains available.

On suspend/resume, the existing telemetry, btop, and tunnel supervisors transition through their recovery paths. Use `tests/manual/m8-tray.ps1` to validate the native tray and suspend/resume behavior on a real Windows session.
