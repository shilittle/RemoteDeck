# RemoteDeck v1.0 scope

## In scope

RemoteDeck v1.0 is a packaged Windows x64 Electron application for Linux/OpenSSH hosts. It includes multi-host and OpenSSH-config management, explicit host-key trust, password/key/keyboard-interactive/agent authentication, Ed25519 generation and verified deployment, multi-tab xterm shells, full SFTP workflows, owned LocalForward and RemoteForward tunnels, proxy discovery, structured system/process/GPU telemetry, btop support, risk-gated command presets, official Codex CLI lifecycle actions, onboarding, legacy migration, tray lifecycle, diagnostics, and NSIS/portable packaging.

The center of the product is the interactive terminal. The application packages its runtime and does not require the end user to install Node.js or local OpenSSH.

## Non-goals for v1.0

- VS Code extensions, Code-OSS forks, Monaco, or a complete IDE
- macOS/Linux local clients or Windows remote hosts
- Multi-user/team sharing or cloud synchronization
- A Windows system service or survival across logout, shutdown, or process termination
- Editing remote `sshd_config`, persistent SSH passwords, or arbitrary remote process killing
- Codex TUI parsing, a custom Codex chat UI, or experimental `codex app-server`
- Automatic updater, formal code signing, formal release publication, or DynamicForward/SOCKS server support

These exclusions do not weaken the required Windows application, Linux SSH workflow, tests, installer, portable package, documentation, or manual acceptance coverage.

