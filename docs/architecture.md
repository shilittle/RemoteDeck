# RemoteDeck 2 architecture

RemoteDeck 2's production architecture targets one Tauri 2 application for Windows x64. The configured bundle consists of a Rust executable and static React assets rendered by the system WebView2. Electron, Chromium, production Node.js, `ssh2`, generic shell/filesystem/HTTP plugins, and a self-extracting portable runtime are outside the production architecture.

## Layers

1. `apps/desktop/tauri-ui` contains the React workbench, xterm.js terminals, typed API client, and view state. It performs no SSH or local filesystem operations itself.
2. `apps/desktop/src-tauri/src/lib.rs` is the narrow Tauri command boundary. `scripts/verify-tauri-boundary.mjs` requires every frontend invoke to match one registered command and every registered command to have a typed frontend method.
3. Rust services own persistence, host trust, OpenSSH process creation, ConPTY sessions, SFTP transfers, tunnel supervision, telemetry, commands, Agent plans, migration, diagnostics, and desktop lifecycle.
4. Windows OpenSSH supplies `ssh.exe`, `sftp.exe`, `ssh-keyscan.exe`, and `ssh-keygen.exe`. Remote operations target Linux and never use an embedded SSH implementation.

## Data and trust

Tauri's application-data directory contains `state-v2.json`, `state-v2.json.bak`, an app-owned `known_hosts`, and the no-clobber `btop-owner-v2` installation nonce. State changes are validated against a cloned model, written to an owned temporary file, flushed, atomically replaced, and only then swapped into memory. Startup recovers from the validated backup when the primary file is damaged. The btop nonce is created once without overwriting a concurrently created value and binds remote watchdog ownership across application restarts.

Every SSH family command uses `-F none`, the app-owned trust file, `StrictHostKeyChecking=yes`, disabled global known-hosts and key updates, fixed argument vectors, bounded output, and timeouts. First-use acceptance rescans the key before persistence. A changed key cannot be overwritten implicitly.

Interactive authentication stays in a real SSH PTY backed by ConPTY. Rust sends terminal output as bounded, ordered events. Input does not use an invoke payload: a narrow command issues a short-lived, single-use ticket for an IPv4-loopback WebSocket bound to the current session generation. The handshake checks the exact host, path, allowed Tauri/development origin, ticket, and live generation; frames are bounded and the connection is invalidated on reconnect, close, host retirement, or shutdown. Passwords, keyboard-interactive answers, private-key passphrases, terminal input, and terminal output are not persisted or placed in diagnostics.

## Runtime ownership

- `TerminalRegistry` owns multi-tab PTY children, authenticated loopback input tickets/connections, ordered terminal events, and a host-retirement barrier.
- `TransferRegistry` owns bounded-concurrency SFTP jobs and cancellation tokens. SFTP CRUD and complete transfer transactions hold a per-host operation barrier; overwrite publication uses app-owned temporary/backup names and rollback rather than deleting the destination first.
- `TunnelRegistry` serializes start/stop/remove transitions, owns each forwarding child, refreshes the saved host profile before every connection attempt, emits monotonically revised state, drains bounded logs, checks health, and applies capped backoff.
- `TelemetryRegistry` streams the embedded collector to `python3 -u -` over verified SSH; the script is not installed remotely. History is bounded in memory, reconnects refresh the saved profile, and status events carry monotonic revisions.
- `CommandJobRegistry` reclassifies final commands in Rust, enforces confirmation, bounds concurrency/output, and owns cancellation.
- Agent actions produce validated provider-specific plans and always execute in ordinary SSH PTY/tmux sessions with provider permission systems intact.

Connection-critical edits to a direct host or a referenced ProxyJump are rejected while an active transfer, tunnel, telemetry collector, or tracked btop watchdog still depends on that route. Host deletion first rejects referenced jump hosts, retires new SFTP/terminal/command admission, cancels and waits for their owned work, then marks the repository host as deleting while tunnel, telemetry, and tracked watchdog cleanup runs. The host plus its attached tunnel/preset records is persisted as deleted only after coordinated cleanup succeeds; an aborted deletion reopens runtime admission.

Closing the main window may hide it to the tray. Tray Quit is the full-exit boundary: locally owned terminals, tunnel children, telemetry collectors, commands, transfers, and tracked btop watchdogs receive concurrent cleanup. Individual cleanup waits and the overall application wait are bounded, so an unreachable remote watchdog can outlive the local process and is reported as residual risk rather than making exit unbounded. A second application launch activates the existing window rather than starting another runtime.

## Packaging

Only the current-user NSIS target is enabled. WebView2 uses `downloadBootstrapper`; the exact installer must pass the 40 MiB release ceiling. The release workflow is configured to build on Windows and prepare checksums and a machine-readable manifest, but publication is permitted only after the exact candidate also passes unpackaged launch, clean silent install/launch/uninstall, Authenticode-status recording, and independent post-upload digest verification. See the unchecked [release checklist](release-checklist.md) for current evidence rather than inferring success from this architecture description.
