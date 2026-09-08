# RemoteDeck 2 testing

Tests target the Rust workspace, the `@remotedeck/web` package, the browser-service boundary and the exact Windows installer. The commands below are required gates; the exact final counts belong to the CI run for the release commit.

## Local gates

```powershell
pnpm install --frozen-lockfile
pnpm lint
pnpm typecheck
pnpm test
pnpm test:integration
pnpm test:e2e
pnpm build

cargo fmt --all -- --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings -A linker-messages

pnpm audit --audit-level high
pnpm dist:win
```

`linker-messages` is the only Clippy allowance for localized MSVC import-library status text. Link failures still fail the build. `pnpm test:e2e` must exercise a real browser against the loopback service, including first connection, terminal input/replay, a file task, command confirmation and an error response.

The recorded Windows working-tree run has 170 passing core tests, 32 server unit tests, two CLI tests and 47 WebUI tests. All five OpenSSH fixture tests and all three browser workflows passed in the separate fixture-backed run. Ordinary `pnpm verify` runs the basic browser workflow and skips the two fixture-dependent cases; run `pnpm test:e2e:openssh` for full coverage. See [the windowless-process validation](windowless-processes.md) for the latest fix and artifact, and [the refactor validation record](refactor-validation.md) for the remaining manual gates.

Windows process regressions verify that synchronous and Tokio children have no console window, that both debug and release service builds use the GUI subsystem, and that ordinary native processes use the common hidden-process constructors. `tests/windows/watch_console_windows.py` can observe `EVENT_OBJECT_SHOW` on an interactive Windows desktop while running the real suites. It records console-window classes and process identity without terminal titles or contents; zero events only describes the observed run.

## Rust and WebUI coverage

The ordinary Rust and WebUI gates cover v2 state validation and recovery, host-key trust and argument isolation, ProxyJump planning, key lifecycle, terminal generations and cleanup, 1 MiB output limits, transactional SFTP and rollback, task cancellation/retry, tunnel health/reconnect, telemetry degradation, btop ownership, process identity, Agent planning, diagnostics redaction and Windows lifecycle argument construction. WebUI tests cover API error decoding, snapshot/event merge and resync, host-bound asynchronous results, terminal input framing/backpressure, SFTP listing and command/Agent risk contracts. These are unit, in-process or browser-contract checks; they do not count as a real sshd integration result.

## OpenSSH integration

`scripts/run-openssh-integration.sh` runs the ignored `openssh_integration` tests serially against the Docker fixtures in `tests/fixtures/openssh`. Linux runs four Docker-backed host tests: first-use trust and changed-key refusal with a bounded command, saved ProxyJump scanning/connection, recursive Unicode SFTP upload/download with a pre-existing destination and normal overwrite path followed by cancellation/retry, and real local plus remote forwarding. Windows additionally runs `generated_proxy_command_is_accepted_by_system_ssh_g`, which asks the system client to parse the generated ProxyCommand with `ssh -G`; it is not part of the Linux Docker count. Thus a Windows run has five ignored fixture tests in total.

The fixture's sshd explicitly permits only Curve25519 KEX because the Windows inbox OpenSSH 9.5p1 `ssh-keyscan` may advertise unsupported `sntrup761x25519-sha512@openssh.com`; this is the known upstream [Win32-OpenSSH issue #2140](https://github.com/PowerShell/Win32-OpenSSH/issues/2140), and the fixture workaround is test-only. A production-server scan must still fail closed; a compatible fixture run must not be reported as proof that the Windows client issue is fixed.

The current reported compatible-fixture runs pass all five tests repeatedly when the Windows system-client test and the compatible Docker toolchain are available. The recursive SFTP test proves the normal backup-and-replace overwrite route and cancellation followed by retry; it does not inject a transfer failure to prove rollback after a failed promotion. Reconnect recovery over a disposable sshd and degraded monitoring/Agent behavior also remain outside this fixture suite. If Docker or the required compatible OpenSSH toolchain is unavailable, record the result as **blocked/skipped**, never as a pass. A real operator host is still needed for provider login and platform-specific credential prompts.

Linux CI installs and checks `libdbus-1-3`, `libdbus-1-dev` and `pkg-config` before compiling the server's `rfd` XDG Desktop Portal backend. The service remains a Windows product; this check keeps the Linux compile and fixture jobs honest about their native dialog dependency.

## Browser and recovery scenarios

The browser E2E suite must check:

- service bootstrap, session exchange, rejected bad Origin/Host, refresh and SSE sequence-gap recovery;
- save host → scan/verify fingerprint → open PTY → terminal input and reconnect;
- browser close/reopen while a terminal, transfer, command or tunnel continues;
- terminal takeover, stale generation rejection and bounded replay/truncation;
- SFTP selection, progress, cancel, retry and overwrite failure reporting;
- L0/L1/L2 command confirmation, Agent start/resume, task navigation and meaningful failures;
- settings, migration preview/import, diagnostics and stop-service behavior.

## Exact package acceptance

Against the exact candidate produced by `pnpm dist:win`:

1. Confirm the only generated release executable is the current-user NSIS installer and is below 40 MiB.
2. Install silently into a fresh per-run directory and verify `RemoteDeck.exe` exists.
3. Start the installed executable in an isolated data directory with `--no-open --launch-file <path>`. Read the one-time launch file (the normal runtime descriptor keeps its control token protected), request the actual loopback `/health` API, and independently verify the started PID remains alive. A process with no ready API fails.
4. Stop the test-owned service, run the uninstaller silently, and verify the install directory and startup Run value are gone while pre-existing `%APPDATA%\io.github.shilittle.remotedeck` data remains.
   The verifier snapshots and restores the current user's RemoteDeck registry keys and desktop/Start Menu shortcuts, so an existing installation in that profile is restored exactly after the smoke test.
5. Record the installer Authenticode status, SHA-256, byte count, `SHA256SUMS.txt` and `release-manifest.json`. Never infer signing or publication from a file name.

The release workflow uploads a candidate artifact for review. Publication requires a separate explicitly authorized action; this repository task does not publish a release.
