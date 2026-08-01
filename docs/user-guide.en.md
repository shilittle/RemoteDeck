# RemoteDeck 2 user guide

## Install and verify

RemoteDeck 2 supports Windows 10/11 x64. Enable Windows OpenSSH Client under Optional Features and make sure the target Linux system runs OpenSSH Server.

After the RemoteDeck 2 artifact is formally published, download `RemoteDeck-<version>-win-x64-setup.exe` and `SHA256SUMS.txt` from this repository's GitHub Release, then run:

```powershell
Get-FileHash .\RemoteDeck-2.0.0-win-x64-setup.exe -Algorithm SHA256
```

The digest must match exactly. The repository currently configures no Authenticode signing; rely on the recorded signature status for the exact artifact, and expect an unsigned installer to trigger SmartScreen's unknown-publisher warning. The installer configuration is per-user; Microsoft's bootstrapper downloads WebView2 when the system does not already have it. Before publication, consult the unchecked [release checklist](release-checklist.md) and [pending artifact record](release-manifest.md) instead of treating this guide as evidence that an installer exists.

## First connection

1. Create a host with an alias, address, port, user, authentication method, and default workspace. You may instead import explicit ordinary hosts from an OpenSSH config.
2. Select Scan keys. Verify the displayed SHA-256 through the server console, an administrator, or another independent trusted channel.
3. Accept only an exact match. A later key change hard-fails. After confirming a legitimate server rotation, explicitly remove old trust, scan again, and verify the replacement.
4. Open Terminal and create a tab. Enter passwords, keyboard-interactive answers, and private-key passphrases directly in that terminal; they are never stored by RemoteDeck.

Connection tests, SFTP, tunnels, telemetry, and background commands use non-interactive OpenSSH. They require an available private key or ssh-agent. For a password-only host, authenticate in a terminal and deploy a public key before using background features.

## Workspaces

### Hosts and keys

- Search and group hosts; select a previously saved and trusted direct host for ProxyJump; configure timeouts, keepalive, compression, `IdentitiesOnly`, and the default directory. Trust the jump first, then scan and independently verify the target fingerprint through it.
- Generate Ed25519 keys, inspect discovered private keys, and deploy a public key to a verified host.
- Config import does not follow `Include` and ignores wildcard hosts, `Match`, and unsupported directives.
- A host referenced as another saved profile's ProxyJump cannot be deleted until that reference is removed. Other deletion first retires new SFTP, terminal, and command work and waits for owned tasks, then blocks new repository connections and cleans tunnels, telemetry, and tracked btop. Only successful cleanup is followed by persisted removal of the host and its attached tunnel/command presets.

### Terminal

- Each tab is a real `ssh.exe` + ConPTY session. Open tabs in parallel, double-click to rename, search scrollback, copy/paste, resize, or create a new shell to reconnect.
- xterm.js handles Unicode 11 and IME composition. `Ctrl+C` is sent to the remote process.
- Keystrokes reach ConPTY through an IPv4-loopback-only WebSocket with a short-lived single-use ticket bound to the allowed origin and live session generation; they are not Tauri invoke payloads. Reconnect, close, host deletion, and quit invalidate the old channel.
- Closing a tab stops only its owned SSH child. Hiding the window to the tray keeps sessions alive; tray Quit performs full cleanup.

### Files and transfers

- Browse directories; create, rename, and delete remote items. Directory deletion is recursive and confirmed.
- Upload with native file/folder selection or native drag-in paths. Select a remote item and local directory to download.
- Conflict policies are ask, overwrite, skip, and automatic rename. The task center shows progress, cancellation, and retry.
- Overwrite never deletes the destination first: the complete result is written to an owned temporary path, the old destination is moved to an owned backup, and promotion is rolled back on failure. If rollback also fails, the preserved backup path is reported.
- Retry re-resolves the current saved host/ProxyJump profile and revalidates the operation instead of reusing stale connection fields from the failed attempt.
- Cancellation removes only valid UUID-marked temporary files owned by that job. Roots, traversal, download-basename escape, oversized trees, and ambiguous destructive paths are rejected. Host deletion retires new SFTP work and waits for direct operations and transfer transactions; connection edits to an active transfer's target or jump require stopping that transfer first.

### Tunnels

- LocalForward exposes a local listening port to a target reachable from the remote host.
- RemoteForward exposes a remote listener to a target reachable from the local machine; external reachability depends on server `GatewayPorts`.
- Each tunnel owns a separate SSH child and has autostart, recovery, capped backoff, revisioned health state, and bounded logs. Every reconnect resolves the current saved profile; connection edits to an active tunnel's target or jump require stopping the tunnel first. Stop/remove affects only that tunnel.

### Monitoring

- The embedded collector is streamed to remote `python3 -u -` over stdin and is never installed remotely.
- “Monitor this host automatically when the app starts” controls startup collection only; manual start remains available when unchecked. Background collection uses BatchMode and never opens a hidden password/passphrase prompt. Every reconnect resolves the current saved profile; connection edits to a target/jump used by an active collector or tracked btop watchdog require stopping that work first.
- View CPU/load, memory/swap, network, disks, NVIDIA GPUs, and processes. History is memory-only and uses the lowest of configured retention, 3,600 records/16 MiB per host, and 8,192 records/64 MiB globally.
- Each process row includes `/proc` start ticks. TERM/KILL atomically revalidates host, PID, start ticks, user, and command remotely; KILL is available only after a recent successful TERM for that exact identity and then shows a separate native Yes/No warning.
- “Start btop with startup monitoring” applies only to auto-monitored hosts. It requires remote `btop`, `tmux`, and `timeout`; restart counts come from tmux rather than a placeholder. Missing tools are explicit and do not break structured monitoring.
- btop ownership uses a stable installation nonce in app data. A later process from the same installation can adopt a matching marked session when btop start is requested (including startup); differently or unmarked sessions are refused. A new session self-terminates if the exact marker is not installed within 30 seconds. Explicit stop, host deletion, and quit clean only sessions tracked after creation/adoption and recheck the marker. Telemetry/btop cleanup is capped at 10 seconds inside the 12-second overall exit wait, so a network failure can require a later start-to-adopt followed by stop.

### Commands and AI Agents

- Rust reclassifies every one-off command and preset immediately before execution. L0 is read-only; L1 changes state and needs target confirmation; L2 covers destructive/privileged/reboot/signal operations and requires exact confirmation text.
- Output and concurrency are bounded. Interactive commands are handed to a standard SSH PTY. Cancellation targets only the job's child.
- Codex, Claude Code, Gemini CLI, and OpenCode support probe, install, login, start, resume, and update flows. Install/update is confirmed; account data remains inside each official CLI.
- RemoteDeck never adds yolo, auto-approval, or sandbox-bypass options.

## Settings, migration, and diagnostics

- Configure terminal fonts, telemetry interval/retention, download directory (manual entry or native folder picker), default Agent, tray behavior, and launch at login.
- Migration first previews LabPulse SSH v0.1.0 or RemoteDeck v1 data. Hosts, tunnels, commands, and settings are individually selectable. The batch persists once, failures cannot leave partial imports, a source hash prevents duplicates, and legacy host trust is never imported.
- Diagnostics default to Downloads and contain redacted state, capabilities, and trusted fingerprints. Passwords, keys, tokens, terminal content, and remote-command output are excluded.

## Shortcuts and uninstall

- `Ctrl+1` through `Ctrl+7`: switch workspaces.
- `Ctrl+Shift+T`: new terminal.
- `Ctrl+J`: task center.
- `Ctrl+,`: settings.

Uninstall through Windows Installed apps. User state is retained by default to prevent accidental profile loss. Export anything needed, then manually remove the RemoteDeck application-data directory as the current user if a complete purge is required.

See the [security model](security.md), [testing guide](testing.md), and [known limitations](known-limitations.md).
