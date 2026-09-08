# AGENTS.md instructions for C:\Users\11147\Documents\ChatGPT\RemoteDeck

<INSTRUCTIONS>
<!-- BEGIN GPT56_SUBAGENT_ROUTING -->

## GPT-5.6 subagent routing

The root thread uses gpt-5.6-sol with ultra reasoning and owns the overall
requirements, decomposition, architecture, coordination, integration, final
validation, and final answer.

Delegation should save main-thread context or create real parallelism. Do not
spawn agents for tiny work, tightly sequential work, or work whose coordination
cost exceeds its benefit.

Allowed child roles:

- luna_max
  Default child role. Use for bounded tasks with clear expected output and
  acceptance criteria, including exploration, research, code reading,
  implementation, testing, log analysis, documentation checks, extraction,
  and deterministic multi-file work.

- terra_xhigh
  Use when the subtask requires substantial judgment, ambiguous debugging,
  cross-component tracing, nontrivial implementation choices, rigorous review,
  or broad edge-case analysis.

- terra_max
  Use only when the subproblem itself is exceptionally difficult and bounded,
  including concurrency, security, subtle correctness, architecture risk,
  complex state interaction, numerical stability, or conflicting evidence.

Routing principles:

1. Start with luna_max when the task is clear, even if it is large.
2. Choose Terra because the reasoning is harder, not merely because there are
   more files or more mechanical work.
3. Use terra_xhigh for most complex judgment-heavy subtasks.
4. Escalate to terra_max only when deeper single-agent reasoning is materially
   useful.
5. Never spawn a Sol child.
6. Never assign Ultra to a child.
7. Never allow a child to spawn another child.
8. Use at most four concurrent children.
9. Prefer fewer well-scoped agents over many fragmented agents.
10. Do not assign overlapping scopes unless the parent explicitly wants
    independent adversarial review.
11. Do not run multiple agents that merely scan the same files in the same way.
12. Every delegated task must state its scope, expected deliverable, allowed
    edits, validation requirement, and stopping condition.
13. The root waits for relevant children, resolves conflicts, performs final
    integration, and runs final validation.
14. Use one of the named custom roles rather than unrestricted generic workers.
15. If a selected model or effort is unavailable, do not silently fall back to
    Sol or a different effort. Report the mismatch to the root thread.

<!-- END GPT56_SUBAGENT_ROUTING -->

--- project-doc ---

# RemoteDeck repository instructions

RemoteDeck 2 is a Windows 10/11 x64 local service with a browser WebUI. The
shipped program is the Rust `RemoteDeck.exe` binary from the
`remotedeck-server` Cargo package. It serves the Vite output from
`apps/web/dist`, binds only to an IPv4 loopback address, and opens the default
browser. The previous Electron and Tauri implementations are historical
fixtures only; they are not production source or current architecture.

## Architecture boundaries

- Business and process ownership lives in `crates/remotedeck-core`; HTTP,
  WebSocket/SSE routing, browser lifecycle, and Windows adapters live in
  `apps/server`.
- The browser client lives in `apps/web`. It may call only explicit `/api/v1`
  routes and the documented terminal WebSocket. There is no generic shell,
  filesystem, HTTP-proxy, or browser-extension bridge.
- The server owns Windows OpenSSH, ConPTY, SFTP, forwarding children, task
  cancellation, state recovery, and cleanup. Closing a browser tab does not
  stop a session or background task.
- Local SSH uses Windows OpenSSH. Interactive sessions use ConPTY through
  `portable-pty`; forwarding uses app-owned `ssh.exe` child processes.
- Windows NSIS is the only release target. The installer is current-user
  scoped, embeds no browser runtime, and never installs production Node.js.

## Security invariants

- First-use and changed SSH host keys are never trusted silently. Scan,
  display SHA-256 fingerprints, require explicit acceptance, rescan before
  persistence, and use `StrictHostKeyChecking=yes` on every connection.
- The app owns a dedicated `known_hosts` file and never mutates the user's
  global OpenSSH files.
- Passwords and private-key passphrases are never persisted, logged or passed
  through HTTP/JSON arguments. Interactive prompts remain in the PTY.
- Validate every value used as an OpenSSH argument. Stop only processes created
  by the current app instance; uninstall cleanup must not kill arbitrary
  processes.
- The browser session uses a short-lived one-time bootstrap ticket exchanged
  for an HttpOnly, SameSite session. Host/Origin/CSRF/WebSocket checks are
  required on every protected operation.
- Terminal output and input are not logged or persisted. Each terminal keeps a
  bounded in-memory replay buffer of at most 1 MiB.

## Required commands

```text
pnpm dev
pnpm lint
pnpm typecheck
pnpm test
pnpm test:integration
pnpm test:e2e
pnpm build
pnpm dist:win
pnpm verify
```

Rust gates:

`rust-toolchain.toml` pins Rust 1.98.0 with rustfmt and Clippy. Keep local and
CI validation on that same toolchain; do not silently substitute a floating
stable version for release checks.

```text
cargo fmt --all -- --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings -A linker-messages
```

The Docker/OpenSSH fixture is run by `scripts/run-openssh-integration.sh`.
Unavailable Docker or remote prerequisites must be reported as skipped; a
reader-facing success message is not evidence that the fixture ran.
The current fixture matrix has four Docker-backed tests plus one Windows-only
system `ssh -G` test. Its recursive SFTP case covers normal overwrite and
cancellation/retry, but not rollback after an injected transfer failure;
reconnect recovery and external monitoring/Agent conditions require separate
evidence. Do not mark browser E2E green until the exact final rerun result for
the ConPTY flow is recorded.

## Completion discipline

- A control performs a real operation or is absent. No fake telemetry, fake
  connection state, inert buttons or placeholder production data.
- Keep the NSIS installer below the 40 MiB CI ceiling.
- Every repository script that starts a child process must keep it windowless on
  Windows. Node `spawn` calls set `windowsHide: true` after caller options are
  merged, and PowerShell background/verification launches use
  `ProcessStartInfo.UseShellExecute = $false`, `CreateNoWindow = $true`, and
  `WindowStyle = Hidden`. Do not use `Start-Process -WindowStyle Hidden` as a
  substitute for the PowerShell helper. Build-tool output may inherit the
  terminal that invoked the script, but scripts must never create a new `cmd`
  window. See [the windowless-process policy](docs/windowless-processes.md).
- Ordinary native child processes in `crates/remotedeck-core` and `apps/server`
  must use the shared `process::command`/`process::async_command` helpers;
  direct `Command::new` and `creation_flags` calls are forbidden outside that
  helper. The only process-construction exception is the `portable-pty`
  ConPTY path, which owns its pseudo-console startup and must remain the sole
  interactive terminal path. Both debug and release server binaries use the
  Windows GUI subsystem so the service itself cannot open a console window.
- Preserve the complete native feature set: host profiles and trust, terminal,
  SFTP transfers, commands and task center, telemetry and btop, tunnels, key
  lifecycle, Agent sessions, settings, diagnostics and legacy-data migration.
- Service readiness must be checked through the loopback runtime descriptor and
  an actual API response, plus an independent process-liveness check. A process
  staying alive without a ready API is a failure.
- Release artifacts are unsigned unless a separately authorized signing step is
  performed. Do not claim a signature, publication, or external integration
  result that was not observed.

## Historical documents

Files whose names describe Electron, Tauri, v1, or an old execution plan are
retained for migration and audit history. They must be labelled historical and
must not be used as current implementation instructions. Current behavior is
defined by `README.md`, `docs/architecture.md`, `docs/security.md`, the user
guides, and the release checklist.

</INSTRUCTIONS>
