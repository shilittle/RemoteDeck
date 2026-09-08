# RemoteDeck 2 security model

RemoteDeck is a single-user Windows local service. The browser is an untrusted UI client of a loopback API; it is not granted local shell, filesystem, credential, or arbitrary network authority. The server and core enforce the same checks for WebUI requests, recovery, and background workers.

## Browser and local API

- The service binds only to IPv4 loopback and rejects unexpected Host and Origin values.
- Startup creates a short-lived, one-time bootstrap ticket. The browser exchanges it for an HttpOnly, SameSite session cookie and a CSRF token; tickets cannot be replayed.
- Every state-changing HTTP route checks the session and CSRF token. Terminal WebSockets additionally require a generation-bound, one-time ticket, exact path, Host and Origin, and bounded frames/connections.
- The browser can call only typed `/api/v1` business routes, the sequenced SSE stream, and the terminal WebSocket. There is no generic shell, filesystem, HTTP proxy, or embedded browser runtime.
- Terminal input/output is excluded from logs, diagnostics and persisted state. Each live session keeps at most 1 MiB of replay data in memory.

## SSH identity and credentials

The core uses an application-owned `known_hosts` file and never mutates the user's global OpenSSH files. At startup and when saving or importing hosts, it seeds endpoints without an application pin from the current user's existing `.ssh/known_hosts`. Bounded, windowless `ssh-keygen -F` lookups support exact address/port records and hashed tokens. This copies previously established local trust without contacting the server; it is not a network trust-on-first-use decision. An existing application record for any algorithm prevents automatic seeding of that endpoint. Matching revoked, certificate-authority, wildcard or other unsupported records are not converted into ordinary trust; preparation warnings are exposed in the WebUI.

The copied trust is a snapshot, not a live union of both files. Later edits to the global file never overwrite an existing application pin. Scanning only displays candidates. Acceptance requires an exact host token, algorithm, public key and SHA-256 match, followed by a fresh scan immediately before an atomic write. An existing changed key hard-fails until the user removes the old trust and independently verifies the replacement.

Every SSH, SFTP, command, telemetry, key-deployment and tunnel invocation uses system OpenSSH with external config disabled where required, strict host-key checking, the dedicated trust file, bounded output and validated arguments. ProxyJump is resolved from a saved direct host profile; arbitrary proxy commands and shell metacharacters are rejected.

Passwords, keyboard-interactive answers, private-key passphrases and Agent credentials remain inside the provider-owned interactive PTY. They are never sent as JSON command parameters, persisted, logged, or included in diagnostics. Non-interactive background work requires a usable key or ssh-agent.

## Process and remote-operation safety

The service registers every child it starts and may stop only that owned child. Tunnel, telemetry, btop and command workers use generation and revision checks so late output cannot revive deleted work or overwrite newer state. Windows startup and uninstall remove only the RemoteDeck startup value and installed files; they do not terminate arbitrary processes.

Command risk is recalculated from the final command immediately before execution. L0 read-only operations can run directly; L1 state-changing operations require target confirmation; L2 destructive, privileged, reboot and signal operations require exact confirmation text. Agent install/update actions are explicit and never add auto-approval or sandbox-bypass flags.

SFTP rejects traversal, ambiguous roots, controls, oversized trees and unsafe recursive symlink operations. Transfer overwrite writes a complete owned temporary result, moves the existing destination to an owned backup, promotes the result and rolls back on failure. Cancellation cleans only temporary files bearing the current job's valid ownership suffix. Process signals revalidate host, PID, user, command and Linux start ticks immediately before TERM/KILL.

## Persistence and release

State and migration writes use validate/clone/atomic-replace transactions. The v2 app-data root, backup, dedicated trust file and btop ownership marker are retained across browser closure, service restart and uninstall. Diagnostics include redacted state, capabilities and trusted fingerprints only.

The production graph excludes Electron, Chromium, WebView2, production Node.js and Tauri. The Windows release is a current-user NSIS installer below 40 MiB. The repository does not sign binaries; CI records the actual Authenticode status and SHA-256 for each candidate. This document does not assert that a candidate has passed until the [release checklist](release-checklist.md) contains evidence.
