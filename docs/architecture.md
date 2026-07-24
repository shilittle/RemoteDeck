# Architecture

RemoteDeck uses one packaged Electron application with four explicit layers.

1. `src/main` owns windows, lifecycle, trusted operating-system access, IPC registration, persistence wiring, and later SSH/background resources.
2. `src/preload` runs in Electron's isolated sandbox. It exposes one frozen `window.remoteDeck` object containing named methods only. Each request and response is parsed with the same versioned Zod contract.
3. `src/renderer` is a React application with a Zustand view store. It has no Node integration and cannot import filesystem, process, Electron, ssh2, or arbitrary IPC APIs.
4. `src/core` and `src/protocol` contain Electron-independent business utilities, state/domain models, and runtime contracts. Core code does not import React.

The renderer invokes a fixed `v1:*` channel. The preload validates the outgoing value, Electron main verifies the exact sender frame, main validates again, a service performs the action, and the return value is validated before crossing both boundaries. High-frequency terminal and telemetry traffic will use named, disposable event subscriptions rather than broad channel access.

Configuration is a versioned JSON document under Electron `userData`. Writes are serialized, schema-checked, written to an owned temporary file, flushed, backed up, and atomically renamed. Invalid persisted data is never silently trusted.

The main process is the sole producer of connection and tunnel state. Each SSH connection generation will own its resources and reject callbacks from stale generations. Terminal, SFTP, tunnel, telemetry, command, and Codex services are separate consumers of a connection manager, with tunnels using dedicated SSH clients.

