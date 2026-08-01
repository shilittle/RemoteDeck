# RemoteDeck 2 security model

## Trust boundaries

- The React renderer is trusted application UI but has no Node.js runtime and no generic process, shell, filesystem, or HTTP plugin. It can call only the typed Tauri commands registered in `lib.rs`.
- Rust validates all persisted objects and every value used in OpenSSH argument vectors. Remote commands are generated from reviewed plans or reclassified immediately before execution.
- Passwords, keyboard-interactive answers, key passphrases, and Agent credentials remain confined to the provider-owned interactive terminal path. Terminal input is carried by a CSP-restricted IPv4-loopback WebSocket whose short-lived single-use ticket is bound to the exact host header, path, allowed origin, session, and generation; frames and pending connections are bounded. Output reaches only the owning terminal through bounded session events. Secret input is never accepted by an invoke payload, and terminal input/output is excluded from persistence, logs, and diagnostics.

## SSH host identity

RemoteDeck uses a dedicated `known_hosts` and never modifies the user's global trust files. Scanning only displays candidates. Acceptance requires an exact host token, algorithm, public key and SHA-256 match, followed by a fresh scan immediately before the atomic write. Existing mismatched keys hard-fail; users must remove the old record and independently verify the replacement.

All SSH, SFTP, tunnel, telemetry, background-command, key-deployment, and Agent probe paths use system OpenSSH with external config disabled and strict host-key checking enabled. Output and execution time are bounded. A ProxyJump must be a saved direct-connect host. RemoteDeck generates its inner proxy command from typed host fields, applies a second `-F none` plus the dedicated trust file and jump identity, rejects shell metacharacters in every expansion-capable value, and quotes every generated argument; raw user-supplied proxy commands are never executed.

## Destructive operations

- Command risk is recalculated from the final command. L1 requires target confirmation; L2 requires exact confirmation text. Unknown commands do not fall below L1.
- Recursive SFTP deletion rejects roots, traversal, ambiguous relative paths, controls, unsafe quoting, oversized trees, and symlink recursion. Transfer cleanup only removes temporary files bearing a valid RemoteDeck-owned UUID suffix. Overwrite preserves the existing target in an owned backup until temporary-file promotion succeeds and reports a preserved backup if rollback cannot finish.
- Process signals bind host, PID, Linux process start ticks, user, and command. The remote operation rechecks that identity immediately before signaling. KILL requires a recent successful TERM for the exact identity plus a native Yes/No warning.
- Tunnel and background-process cancellation targets only children registered by the current app instance. Registry transitions are serialized to prevent invisible orphan processes.
- Installer/update Agent actions require explicit confirmation and run as the current remote user without permission-bypass flags.

Host profile mutation and deletion are coordinated with runtime ownership. Connection-critical edits to a target or direct ProxyJump route are blocked while affected transfers, tunnels, telemetry collectors, or tracked btop watchdogs are active. Deletion rejects a host still referenced as ProxyJump, retires SFTP/terminal/command admission and waits for owned work, blocks new repository-resolved work, cleans tunnels/telemetry/watchdogs, and only then atomically persists removal of the host and its attached tunnel/preset records.

## Persistence and diagnostics

State writes and migration batches use clone/validate/single-persist/swap transactions. A failed disk write leaves both in-memory and persisted state unchanged. Legacy host trust is never migrated.

Diagnostics contain a manifest, redacted state, capabilities, and trusted-key fingerprints only. Recursive key-name and content checks reject likely credentials before an atomic ZIP is published. Terminal and remote-command output are excluded.

## Distribution

The production dependency graph excludes Electron, Chromium, `ssh2`, production Node.js, and generic Tauri shell/filesystem/HTTP plugins. The repository currently applies no Authenticode signature. This document does not assert that a release artifact exists: when one is published, users must inspect its recorded signature status, download it from the official GitHub repository, and verify `SHA256SUMS.txt`. Report vulnerabilities through [SECURITY.md](../SECURITY.md).
