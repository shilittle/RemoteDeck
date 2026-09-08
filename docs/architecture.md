# RemoteDeck 2 architecture

RemoteDeck 2 is a Windows local service with a browser client. `RemoteDeck.exe` is the `remotedeck-server` Cargo binary; it binds an ephemeral IPv4 loopback port, publishes a short-lived bootstrap ticket, and opens the default browser. The browser is a client of the service and is not the owner of SSH processes or task state.

## Components

1. `crates/remotedeck-core` is renderer-independent Rust business logic. It owns the v2 repository, host profiles and trust, OpenSSH argument construction, ConPTY sessions, SFTP transactions, task controls, commands, Agents, telemetry, btop, tunnels, migration, diagnostics, and process ownership.
2. `apps/server` is the `remotedeck-server` package. It owns Tokio runtime setup, Windows paths and process adapters, Axum HTTP routing, SSE event delivery, terminal WebSocket attachment, browser launch, single-instance coordination, shutdown, and static asset embedding.
3. `apps/web` is `@remotedeck/web`. Vite emits `apps/web/dist`; the server embeds this output in the release binary. The client uses a typed API for `/api/v1`, a sequenced SSE stream for snapshots/events, and a generation-bound WebSocket for terminal input/output.

The root Cargo workspace contains `crates/remotedeck-core` and `apps/server`. There is no production Electron, Tauri, WebView2, Node.js runtime, `ssh2`, or generic shell/filesystem/HTTP plugin.

## State and recovery

The server uses the existing v2 application data root `%APPDATA%\io.github.shilittle.remotedeck`. State writes validate a cloned model, write an owned temporary file, flush and atomically replace the primary file, then update memory. The backup and migration rules remain unchanged, including a dedicated `known_hosts` and installation-scoped btop ownership marker.

The server writes a runtime descriptor after binding successfully. The descriptor contains the server PID, loopback URL/port, descriptor revision and the short-lived bootstrap information needed by the browser launcher. It is replaced atomically and removed during orderly shutdown. A stale descriptor is ignored after PID and API checks fail.

Browser state is reconstructed from a server snapshot followed by sequenced events. Each event carries a global sequence and resource revision. A subscription starts from the snapshot boundary; the server replays the missing bounded event range or instructs the client to resync when the range is no longer retained. Stale resource revisions are discarded. Refreshing the browser therefore cannot create duplicate work or move state backward.

## HTTP, SSE and terminal transport

The server exposes explicit `/api/v1` routes for hosts, trust, sessions, SFTP, transfers, commands, Agents, telemetry, tunnels, settings, migration, diagnostics, tasks and lifecycle. Long operations return a task/session identifier immediately. Request idempotency keys prevent a retry caused by a browser refresh from starting a second operation.

The SSE stream carries public snapshots and bounded state events. The terminal uses a bidirectional WebSocket whose ticket is single-use, short-lived, bound to the session generation and checked against the exact loopback Host/Origin/path. A new attachment takes write ownership; the previous attachment can continue to receive an explicit close event but cannot send input. Frames and connection counts are bounded.

Each terminal session is a real Windows OpenSSH child attached to ConPTY. Output is ordered and retained only in a bounded in-memory replay buffer of at most 1 MiB. A newly attached browser receives the current generation and recent output; when the buffer was truncated it receives an explicit truncation marker. Closing the browser leaves the PTY and all background work alive. Service shutdown performs bounded cleanup of children created by that instance. Remote sessions intended to survive a service restart are resumed through RemoteDeck-owned tmux sessions.

## Operations and security boundaries

All SSH, SFTP, tunnel, telemetry and background command paths resolve the current saved host profile, validate every OpenSSH argument, use the app-owned `known_hosts`, and enable strict host-key checking. Interactive credentials stay in a PTY. The browser never receives a private key or password and cannot ask the server to execute an arbitrary local process.

The service accepts only loopback requests with the expected Host and Origin. The bootstrap ticket is exchanged once for an HttpOnly, SameSite session cookie; state-changing routes also require the session CSRF token. WebSocket handshakes require the current session and terminal ticket. Diagnostics redact credentials, terminal data, remote output and likely secrets.

## Lifetime and packaging

There is one service instance per current-user v2 data root. A second launch discovers the existing runtime descriptor, verifies the live PID and health endpoint, and opens the existing URL. Login startup uses the same binary and a hidden launch mode; the settings page controls this entry. “Stop service” first rejects new work, drains and cancels owned tasks within a deadline, cleans only owned children/watchdogs, removes the descriptor and exits. It does not kill arbitrary processes.

The NSIS installer is current-user scoped at `%LOCALAPPDATA%\Programs\RemoteDeck`. User data at `%APPDATA%\io.github.shilittle.remotedeck` is outside the install directory and survives uninstall. The only release artifact is `RemoteDeck-<version>-win-x64-setup.exe`; CI enforces a size below 40 MiB and records Authenticode status and SHA-256 without fabricating a signature.
