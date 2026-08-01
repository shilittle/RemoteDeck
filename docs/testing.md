# RemoteDeck 2 testing

## Local gates

```powershell
pnpm install --frozen-lockfile
pnpm lint
pnpm typecheck
pnpm test
pnpm test:integration
pnpm build

cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --all-features
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets --all-features -- -D warnings -A linker-messages

pnpm audit --audit-level high
pnpm dist:win
```

`linker-messages` is the sole allow: Rust 1.97 reports localized MSVC import-library
status stdout through that lint on Windows. Link failures still fail the command, and
all Clippy and Rust warnings remain denied.

`pnpm verify` combines frontend lint, type checking, unit tests, the Tauri boundary test, and the production web build. The boundary test also checks version synchronization, NSIS-only bundling, WebView2 bootstrap mode, the restricted capability, forbidden dependencies/plugins, and exact frontend/backend command parity.

Rust tests cover state validation and recovery, transactional migration, host-key trust and argument isolation, authorized-key merging, terminal event ordering/cleanup and authenticated loopback input rejection/replay, bounded command jobs, SFTP path/conflict/transactional-rollback/current-profile-retry/host-retirement rules, tunnel health/backoff/generation/revision/log budgets, telemetry parsing/history/revisions/process start-tick identity, stable btop ownership/adoption/bootstrap commands, Agent planning, diagnostics redaction, and Windows lifecycle argument construction. Frontend tests cover stale-event merge behavior, tunnel summary-log preservation, terminal event/listing reconciliation, terminal-input CSP/frame/backpressure rules, host-bound async results, and risk/Agent contracts.

## Independent review

Subsystems are reviewed independently from their implementer. Release-blocking races or contract mismatches found by those reviews must be fixed and covered by regression tests before packaging. Reviews explicitly exercise delete/edit/reconnect races across direct and ProxyJump routes, lost-initial-event races, cancellation ownership, bounded shutdown, and remote identity reuse. Strict Clippy treats every warning as a failure; `git diff --check` and UTF-8 scans are additional gates. Test totals are intentionally not hard-coded here: the exact final command output belongs in release evidence for the final commit.

## Package acceptance

The Windows release candidate must pass all of the following against the exact produced installer:

1. Build the release Rust binary and NSIS bundle.
2. Launch the unpackaged release binary and observe a non-zero main-window handle without early exit.
3. Enforce an installer size below 40 MiB.
4. Install silently into a clean temporary per-user directory.
5. Launch the installed executable and observe its main window.
6. Run the installed uninstaller silently and verify success.
7. Record Authenticode status, compute SHA-256, and produce `SHA256SUMS.txt` plus `release-manifest.json` from that exact installer.
8. After upload, download the published asset independently and compare its byte count and SHA-256 with the local candidate and GitHub-reported metadata.

Linux-specific SSH behavior still requires a real trusted test host for final operator acceptance: changed-key refusal, interactive password/key-passphrase prompts, recursive Unicode SFTP with transactional overwrite/rollback, both forwarding directions and reconnect, telemetry degradation without Python/GPU/btop, process start-tick reuse refusal, watchdog ownership/adoption, and provider-owned Agent authentication. The Docker/OpenSSH suite exercises direct trust, ProxyJump, SFTP, and both forwarding directions when Docker is available; unavailable external prerequisites must be reported as skipped rather than presented as a pass. No test bypasses the strict host-key path.
