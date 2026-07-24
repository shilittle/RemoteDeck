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

## Trust boundaries still requiring user action

RemoteDeck cannot decide whether an unknown host key belongs to the intended server, provide a user's SSH secret, authorize a code-signing identity, or perform a real Codex account login. Those actions require explicit user confirmation or input; tests use local SSH fixtures and mocked external account boundaries.
