# RemoteDeck 2.0 security audit

Audit date: 2026-08-02. Scope: Tauri capabilities and command boundary, Windows OpenSSH invocation, host-key lifecycle, interactive terminals, SFTP/transfers, tunnels, telemetry and process signals, command/Agent execution, migration, diagnostics, persistence, process cleanup, packaging configuration, and dependency inventory. Artifact publication remains outside this source audit until the exact candidate completes the release checklist.

## Architecture boundary

- The configured production bundle contains one Tauri 2/Rust executable rendered by the system WebView2. Electron, Chromium, Node.js, Python, `ssh2`, and `node-pty` are not production dependencies or configured packaged runtimes.
- The main WebView is restricted by CSP and a main-window-only Tauri capability. No shell, filesystem, HTTP, or generic process plugin is installed. Each invoke operation maps to an explicitly registered, typed Rust command; an automated boundary test requires exact parity. Terminal keystrokes are the deliberate exception to invoke transport: they use an IPv4-loopback-only WebSocket with an exact origin/host/path check, a short-lived single-use random ticket, live session-generation binding, bounded frames/connections, and lifecycle invalidation.
- Native file/folder selection and WebView drag/drop return user-selected paths only. Remote operations remain dedicated Rust commands rather than a renderer filesystem bridge.

## SSH and secrets

- Every OpenSSH process uses `-F none`, `StrictHostKeyChecking=yes`, and RemoteDeck's dedicated `known_hosts`. The application never reads trust from or writes trust to the user's global OpenSSH configuration.
- First use scans candidates, displays SHA-256 fingerprints, and requires explicit acceptance. Changed keys hard-fail; deleting trust removes the exact host-token/algorithm/key record and preserves unrelated algorithms and aliases.
- Proxy jumps resolve to saved host profiles. Jump and destination trust, identity, username, and port are handled independently; a destination reachable only through a jump is scanned through that already trusted jump.
- Passwords and private-key passphrases are entered only in a ConPTY-backed OpenSSH prompt. They are not Tauri invoke arguments, persisted state, diagnostics, or logs.
- OpenSSH arguments, remote paths, forwarding specifications, process identities, and stored profiles are validated and bounded. Captured non-interactive output and timeouts are bounded.

## Operations and lifecycle

- SFTP uses the system client with structured batch input. Recursive operations do not follow remote symlinks; transfers use owned temporary and rollback-backup paths, transactional overwrite, conflict policies, cancellation, current-profile retry, and bounded job/tree state. A per-host barrier prevents deletion from racing a multi-step transfer or direct CRUD operation; deletion purges retained jobs so stale routes cannot be retried.
- Tunnel supervisors own only children created by this process. Generation checks prevent stale workers from reviving deleted tunnels; monotonic revisions prevent late state overwrites in the renderer. Reconnect resolves the current saved profile, active direct/ProxyJump routes reject critical edits, log entry/message/per-tunnel/global budgets are enforced, cleanup waits are bounded, and local versus remote forwarding health is evaluated according to direction.
- Telemetry is non-persistent on the remote host. Samples, strings, arrays, histories, and stderr are bounded; malformed or oversized input fails closed, and revised status events reject stale renderer updates. Process signals bind PID, `/proc` start ticks, user, and command, re-read identity atomically before signaling, and require a successful TERM for that exact identity plus a native dialog before KILL.
- btop watchdog ownership uses a stable no-clobber nonce in app data. Same-installation start can adopt a matching marked tmux session across process restarts; mismatched sessions are refused. A newly created session self-terminates if its exact owner marker is not set within the 30-second bootstrap lease, and cleanup rechecks the marker before killing.
- RemoteDeck-owned tmux names are deterministic and provider/host scoped. Agent installation and update show a backend-generated plan, source URL, and full commands, then require an exact host-alias confirmation; execution independently rebuilds and revalidates the plan.
- Host deletion first rejects ProxyJump references, retires SFTP/terminal/command admission and waits for their owned work, marks the host as deleting, cleans tunnels/telemetry/tracked watchdogs, and persists deletion only after cleanup succeeds. Connection-critical target/jump edits are also blocked while affected transfer, tunnel, monitor, or watchdog routes are active.
- Full application exit concurrently requests cleanup of owned terminals, transfers, command jobs, telemetry workers, tracked btop sessions, and tunnels. Telemetry watchdog cleanup is capped at 10 seconds inside a 12-second overall wait; this prevents unbounded exit but means remote cleanup after network failure is best effort. Window close may hide to the tray only when configured.

## Persistence, migration, and diagnostics

- State is schema-versioned and validated before use. Updates are written and flushed to a same-directory temporary file, atomically replace the primary on Windows, and retain the last valid backup. Recovery never overwrites a valid backup with a corrupt primary.
- Legacy imports are previewed, hashed for idempotency, preflighted in memory, persisted once, and swapped into live state only after the write succeeds. A failed record or disk write applies nothing.
- Diagnostic ZIP output is selected through a native save boundary, excludes terminal/remote command content and credentials, redacts sensitive keys and private-key material, applies entry/size bounds, and fails closed on a final secret scan.

## Release and residual risk

- Frozen lockfiles, strict Clippy, command-boundary checks, dependency audit, NSIS-only configuration, a 40 MiB installer ceiling, clean install/launch/uninstall smoke tests, and independently re-downloaded SHA-256 verification gate release. These are requirements, not claims that the pending installer or GitHub Release has passed them.
- The repository has no Authenticode signing configuration. For an eventual unsigned artifact, SHA-256 proves byte identity but not publisher identity; users must verify the release URL and checksum. Exact signature status, bytes, and digest remain pending in [release-manifest.md](release-manifest.md) until measured.
- Users must verify new fingerprints through a separate trusted channel. Explicit L1/L2 confirmations reduce mistakes but cannot make an arbitrary remote shell command safe. Agent authentication, remote package-manager trust, and availability of OpenSSH/WebView2/Python/tmux/btop remain external operator boundaries.
