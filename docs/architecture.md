# Architecture

RemoteDeck uses one packaged Electron application with four explicit layers.

1. `src/main` owns windows, lifecycle, trusted operating-system access, IPC registration, persistence wiring, and later SSH/background resources.
2. `src/preload` runs in Electron's isolated sandbox. It exposes one frozen `window.remoteDeck` object containing named methods only. Each request and response is parsed with the same versioned Zod contract.
3. `src/renderer` is a React application with a Zustand view store. It has no Node integration and cannot import filesystem, process, Electron, ssh2, or arbitrary IPC APIs.
4. `src/core` and `src/protocol` contain Electron-independent business utilities, state/domain models, and runtime contracts. Core code does not import React.

The renderer invokes a fixed `v1:*` channel. The preload validates the outgoing value, Electron main verifies the exact sender frame, main validates again, a service performs the action, and the return value is validated before crossing both boundaries. High-frequency terminal and telemetry traffic will use named, disposable event subscriptions rather than broad channel access.

Configuration is a versioned JSON document under Electron `userData`. Writes are serialized, schema-checked, written to an owned temporary file, flushed, backed up, and atomically renamed. Invalid persisted data is never silently trusted.

The main process is the sole producer of connection, tunnel, telemetry, and background-tool state. Each SSH or supervisor generation owns its resources and rejects callbacks from stale generations. Terminal, SFTP, tunnel, telemetry, command, and Codex services are separate consumers of a connection manager, with tunnels using dedicated SSH clients.

The Python collector is a packaged resource outside the Electron bundle. Main streams it through an already authenticated SSH channel to `python3 -u -`; it is not installed remotely. Strict v1 JSONL is parsed in main, retained in a bounded in-memory history, then emitted over one named telemetry event. btop uses an independent PTY channel and is never treated as a structured data source. See `docs/monitoring.md`.

The command service owns preset CRUD, main-side risk enforcement, bounded exec jobs, and per-channel cancellation. PTY-required commands are handed to the terminal service instead of emulating a terminal over exec. The separate Codex service performs only fixed capability probes and opens official CLI commands in those PTYs; tmux session association is derived from host and workspace identity. See `docs/commands-codex.md`.

First-run onboarding is a renderer workflow over the same host/Codex services used after setup. Legacy migration is a main/core service with a preview/apply contract and content-hash idempotency. The bottom task center is a projection of existing transfer, command, tunnel, and connection state rather than a second task engine. Tray close hides only the BrowserWindow; main owns the still-live background resources until Electron's explicit quit lifecycle runs. See `docs/experience-and-migration.md`.
