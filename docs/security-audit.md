# RemoteDeck 2 security audit scope

This checklist describes the evidence required for the current local-service architecture. It is a review scope, not a claim that every item has passed in this working tree.

## Review boundaries

- `crates/remotedeck-core` contains no UI runtime and exposes validated business operations, OpenSSH argument construction, host-key trust, process ownership, transactional SFTP, migration and diagnostics.
- `apps/server` owns loopback binding, bootstrap/session/CSRF handling, SSE sequencing, terminal WebSocket generations, runtime descriptor, browser launch, single-instance lifecycle and bounded shutdown.
- `apps/web` has only typed API and terminal transport calls. It has no Node.js bridge, generic shell/filesystem request, or embedded browser runtime.
- Release packaging contains one current-user NSIS installer with the Rust server and embedded `apps/web/dist`; no portable, Electron, Tauri, WebView2 or production Node.js payload is allowed.

## Required checks

1. Unauthenticated API calls, unexpected Host/Origin, CSRF failures, replayed/expired bootstrap tickets and stale WebSocket tickets are rejected.
2. Changed host keys are displayed and refused until explicit independent verification, fresh rescan and acceptance. Every connection uses the dedicated trust file and strict checking.
3. Passwords, key passphrases, terminal input/output and remote command output are absent from logs, persisted state and diagnostics.
4. Duplicate request identifiers do not create duplicate sessions or tasks. SSE sequence gaps trigger replay or a full resync; stale revisions cannot overwrite newer state.
5. Terminal takeover invalidates old write ownership. Output replay is capped at 1 MiB and marks truncation. Closing the browser leaves owned PTYs, transfers, commands and tunnels alive.
6. SFTP traversal, recursive root deletion, unsafe symlinks, overwrite rollback, cancellation and retry are covered. Process signals revalidate identity immediately before TERM/KILL.
7. Shutdown, second launch and uninstall affect only this installation's owned children, descriptor, startup entry and install directory. User data survives uninstall.
8. The release candidate passes Rust/frontend gates, real OpenSSH fixtures, browser E2E, loopback readiness, independent process liveness, clean silent install/uninstall and the 40 MiB size ceiling.

## Evidence discipline

The exact commands, versions, skipped prerequisites and output belong in the release checklist or CI run for the exact commit. Passing a reader-facing report, a live process without a ready API, or a generated manifest without the corresponding installer does not satisfy this audit.
