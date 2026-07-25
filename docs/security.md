# Security model

## Process boundary

The renderer has `contextIsolation`, sandboxing, web security, and navigation restrictions enabled; Node integration and insecure content are disabled. A deny-by-default permission handler, denied popups/webviews/navigation, and a restrictive CSP prevent the renderer from becoming a general browser or local-code bridge. Production CSP permits only application resources and no remote scripts.

Preload exposes no generic `send`, `invoke`, filesystem, shell, socket, or channel parameters. Main accepts IPC only from the current BrowserWindow's main frame. Zod validates every request and return value on both sides of the bridge.

The packaging hook disables Electron RunAsNode, NODE_OPTIONS, and inspector CLI behavior; enables cookie encryption and ASAR integrity; and restricts loading to the packaged ASAR.

## Data and credentials

Settings and SSH profiles are schema-versioned and atomically persisted. SSH passwords, keyboard-interactive answers, and key passphrases remain in memory only and are outside all persisted domain models. Connection attempts clear credential fields and private-key buffers in `finally` paths. Private-key discovery uses a native user-selected file dialog and persists only path and non-secret metadata.

Every SSH handshake uses an explicit host verifier. Unknown keys stop before authentication and require affirmative acceptance; changed keys hard-fail and cannot replace trust from the connection dialog. ProxyJump verifies and authenticates the jump and target independently. SFTP public-key deployment uses an application-owned temporary file, restrictive permissions, material-based deduplication, and atomic rename, followed by a new key-authenticated connection before changing the default auth profile. See `docs/ssh.md` for the operational flow.

Owned resource cleanup, command confirmation, transfer temporary files, diagnostics, and Codex credential boundaries follow the invariants in `AGENTS.md`.

Pino writes structured category logs below `userData/logs`; launch-segmented rotation retains at most 20 owned files. The log hook recursively redacts secret-named fields plus inline/JSON passwords, passphrases, tokens, authorization values, URL credentials, API keys, OpenAI-style tokens, and private-key blocks. Terminal input and output are not application logs. Diagnostics adds a second scan after redaction and before ZIP creation.

Interactive terminal bytes cross only the fixed terminal IPC contracts and are not persisted, replayed, or included in diagnostics. Clipboard access is limited to bounded text read/write operations. Terminal links accept only validated HTTP(S) URLs before the main process delegates to the operating system; `file:`, executable, and custom schemes are rejected.

SFTP paths and entries cross fixed schemas; remote mutations never invoke a shell. Recursive operations use `lstat` and do not follow symbolic links. Transfers write instance/job-owned temporary names and rename only after success. Cancellation cleanup targets only those exact paths. Native pickers and dropped `File` objects establish user intent for local filesystem sources and destinations; the renderer has no general path-reading API.

Tunnel profiles persist endpoints and health policy but never credentials. Each active tunnel owns its SSH connection, local listener or remote bind, streams, and timers. Cleanup closes only those resources; ordinary port conflicts cannot trigger process termination. Clash/Mihomo discovery is read-only and requires the user to select a candidate. The optional legacy remote cleanup hook is disabled by default, stores the exact visible command and an explicit authorization flag, runs only after a bind failure, and emits an audit log entry. See `docs/tunnels.md`.

Telemetry is read-only until the user explicitly selects an owned process and requests a signal. Collector output is size-bounded and schema-validated before it reaches the renderer. SIGTERM/SIGKILL use fixed commands with a numeric PID only after both the newest snapshot and a fresh remote `ps` query match the current SSH user and full command. SIGKILL also requires a recent matching SIGTERM plus a second confirmation. Commands are never taken from collector text. The packaged collector is streamed over stdin and does not create a persistent remote file. btop output is discarded rather than parsed or logged. See `docs/monitoring.md`.

Command presets are reloaded and classified in main at run time. Declared risk is a floor: readonly allowlisting may keep L0, unknown or mutating commands become at least L1, and destructive patterns become L2. L1 requires an affirmative confirmation after displaying the final command/target; L2 additionally requires exact typed text. Each non-PTY job owns one SSH exec channel, and cancellation cannot close another job or terminal. Free terminal input is deliberately outside this best-effort classifier. See `docs/commands-codex.md`.

Codex integration executes fixed probes and version-advertised stable CLI commands only. The official installer/update fallback is immutable and main requires explicit confirmation. RemoteDeck never requests a key/token, reads `auth.json`, parses TUI bytes, or supplies a sandbox-bypass flag. Login and all interactive Codex modes run in the normal SSH PTY; tmux persistence uses a deterministic app-owned session name.

Legacy migration reads only a native-picker path passed through its dedicated IPC contract, limits bytes, validates a narrow schema, and shows a full preview. Content hashes prevent duplicate import. Legacy cleanup commands are imported disabled, and legacy regex rules cannot lower risk; backreferences, lookbehind, nested quantified groups, invalid expressions, and oversized patterns are never executed. Closing to tray is distinct from quitting: hiding the BrowserWindow does not dispose background resources, while the `before-quit` path owns complete cleanup.

## Trust boundaries still requiring user action

RemoteDeck cannot decide whether an unknown host key belongs to the intended server, provide a user's SSH secret, or perform a real Codex account login. Those actions require explicit user confirmation or input; tests use local SSH fixtures and mocked external account boundaries.
