# Security model

## Process boundary

The renderer has `contextIsolation`, sandboxing, web security, and navigation restrictions enabled; Node integration and insecure content are disabled. A deny-by-default permission handler, denied popups/webviews/navigation, and a restrictive CSP prevent the renderer from becoming a general browser or local-code bridge. Production CSP permits only application resources and no remote scripts.

Preload exposes no generic `send`, `invoke`, filesystem, shell, socket, or channel parameters. Main accepts IPC only from the current BrowserWindow's main frame. Zod validates every request and return value on both sides of the bridge.

The packaging hook disables Electron RunAsNode, NODE_OPTIONS, and inspector CLI behavior; enables cookie encryption and ASAR integrity; and restricts loading to the packaged ASAR.

## Data and credentials

Settings and SSH profiles are schema-versioned and atomically persisted. SSH passwords, keyboard-interactive answers, and key passphrases remain in memory only and are outside all persisted domain models. Connection attempts clear credential fields and private-key buffers in `finally` paths. Private-key discovery uses a native user-selected file dialog and persists only path and non-secret metadata.

Every SSH handshake uses an explicit host verifier. Unknown keys stop before authentication and require affirmative acceptance; changed keys hard-fail and cannot replace trust from the connection dialog. ProxyJump verifies and authenticates the jump and target independently. SFTP public-key deployment uses an application-owned temporary file, restrictive permissions, material-based deduplication, and atomic rename, followed by a new key-authenticated connection before changing the default auth profile. See `docs/ssh.md` for the operational flow.

Owned resource cleanup, command confirmation, transfer temporary files, diagnostics, and Codex credential boundaries follow the invariants in `AGENTS.md`.

Pino writes structured category logs below `userData/logs`. The log hook recursively redacts secret-named fields plus inline passwords, passphrases, tokens, authorization bearer values, API keys, and private-key blocks. Terminal input and output are not application logs.

Interactive terminal bytes cross only the fixed terminal IPC contracts and are not persisted, replayed, or included in diagnostics. Clipboard access is limited to bounded text read/write operations. Terminal links accept only validated HTTP(S) URLs before the main process delegates to the operating system; `file:`, executable, and custom schemes are rejected.

SFTP paths and entries cross fixed schemas; remote mutations never invoke a shell. Recursive operations use `lstat` and do not follow symbolic links. Transfers write instance/job-owned temporary names and rename only after success. Cancellation cleanup targets only those exact paths. Native pickers and dropped `File` objects establish user intent for local filesystem sources and destinations; the renderer has no general path-reading API.

Tunnel profiles persist endpoints and health policy but never credentials. Each active tunnel owns its SSH connection, local listener or remote bind, streams, and timers. Cleanup closes only those resources; ordinary port conflicts cannot trigger process termination. Clash/Mihomo discovery is read-only and requires the user to select a candidate. The optional legacy remote cleanup hook is disabled by default, stores the exact visible command and an explicit authorization flag, runs only after a bind failure, and emits an audit log entry. See `docs/tunnels.md`.

## Trust boundaries still requiring user action

RemoteDeck cannot decide whether an unknown host key belongs to the intended server, provide a user's SSH secret, authorize a code-signing identity, or perform a real Codex account login. Those actions require explicit user confirmation or input; tests use local SSH fixtures and mocked external account boundaries.
