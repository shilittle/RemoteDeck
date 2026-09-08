# RemoteDeck 2 release checklist

This checklist is evidence-driven. A checked item applies only to the exact commit and exact installer recorded with it.

## Source and dependency checks

- [x] Root package, `@remotedeck/web`, `remotedeck-core` and `remotedeck-server` versions are aligned at `2.1.0` for the `v2.1.0` release tag.
- [x] Frozen lockfile installation and `pnpm audit --audit-level high` pass; `cargo audit` also passes.
- [x] `pnpm lint`, `pnpm typecheck`, `pnpm test`, `pnpm test:integration`, `pnpm test:e2e`, and `pnpm build` pass.
- [x] `cargo fmt --all -- --check`, `cargo test --workspace --all-features`, and strict workspace Clippy pass.
- [ ] No production Electron, Tauri, WebView2, Node.js runtime, generic shell/filesystem/HTTP bridge, mock data or inert control remains.

## OpenSSH and behavior checks

- [ ] Record `scripts/run-openssh-integration.sh` for the exact release commit: the four Docker-backed tests cover first-use and changed-key refusal with a bounded command, saved ProxyJump scanning/connection, recursive Unicode SFTP upload/download with a pre-existing destination and normal overwrite path followed by cancellation/retry, and both forwarding directions. The current reported Windows runs pass these four tests plus the Windows-only `ssh -G` ProxyCommand parser test repeatedly; attach the exact command and log before checking this item.
- [ ] Add real disposable-host evidence for rollback after an injected transfer failure and reconnect recovery. The current five-test fixture suite does not cover either condition; ordinary Rust unit tests, API tests and browser contracts do not satisfy this item.
- [ ] Record the external monitoring and Agent conditions separately: remote collector tools, GPU/btop dependencies, provider installation/login and account state remain environment-dependent and are not certified by the OpenSSH fixture.
- [ ] The Windows inbox OpenSSH `ssh-keyscan`/`sntrup` limitation is recorded against [Win32-OpenSSH issue #2140](https://github.com/PowerShell/Win32-OpenSSH/issues/2140). The fixture-only Curve25519 KEX restriction is documented as a compatibility workaround and is not evidence that production interoperability is fixed.
- [x] The recorded local run passes all three fixture-backed browser workflows: host creation and trust, terminal input/replay, transport interruption, exclusive takeover, page close/reopen, real file transfer, command confirmation and error display. Authentication boundary cases are also covered by server tests; details are in [the validation record](refactor-validation.md).
- [ ] Refresh, SSE sequence gaps, multiple browser tabs, terminal takeover, service restart and request idempotency do not duplicate work or move state backward.
- [ ] Credentials and terminal/remote output are absent from logs, state and diagnostics; illegal Host/Origin/CSRF/WebSocket requests are rejected.

## Windows package checks

- [x] `pnpm dist:win` builds `RemoteDeck.exe` with `apps/web/dist` embedded and produces only `RemoteDeck-<version>-win-x64-setup.exe` under `dist`.
- [ ] CI installs the official NSIS package independently; the local script may also resolve `makensis.exe` from PATH, a standard NSIS install, or an existing Tauri cache. No tauri CLI is installed for packaging.
- [x] The installer is current-user scoped and below 40 MiB. No portable or WebView2 runtime artifact exists.
- [x] `scripts/verify-release.ps1` installs silently into a clean directory, launches with an isolated `--data-dir` and `--launch-file`, receives a real loopback `/health` response, verifies the started PID independently, then stops only that test-owned service through `--stop`.
- [x] Silent uninstall removes only the install directory, shortcuts, uninstall metadata and RemoteDeck startup entry. Pre-existing `%APPDATA%\io.github.shilittle.remotedeck` user data remains.
- [x] The release smoke test snapshots and restores the current user's RemoteDeck registry keys and desktop/Start Menu shortcuts around its temporary install.
- [x] Authenticode status, byte count and SHA-256 are recorded in `release-manifest.json` and `SHA256SUMS.txt`. The unsigned state is reported when observed; no signature is inferred.

## Publication boundary

- [ ] Release assets are independently downloaded and checksum-verified after an authorized publication.
- [x] The user explicitly authorized pushing the source and publishing a new GitHub Release. The release workflow itself still records a candidate artifact only; the authorized publication verifies the tag and independently downloads uploaded assets before making the draft public.

## Final release evidence

Fill these fields only from the exact source commit and exact candidate artifact. `PENDING` is a placeholder, not evidence of a completed check.

- Source: `v2.1.0` release candidate based on `7a88b4163abc4b6b27c41348f3eb0d03a49be5bc`; the exact commit is recorded by the annotated tag and the uploaded `release-manifest.json`.
- Frontend: lint/typecheck pass; 47 unit tests pass; 3 real browser workflows pass with disposable OpenSSH fixtures.
- Rust: format and strict Clippy pass; 170 core + 32 server + 2 Windows CLI tests pass. The 5 normally ignored fixture tests pass separately.
- OpenSSH: `pnpm test:e2e:openssh` equivalent runner with pinned official ECR fixture image; Windows OpenSSH 9.5p1 and fixture-only Curve25519 KEX. Five SSH cases and three browser workflows pass.
- Installer: `dist/RemoteDeck-2.1.0-win-x64-setup.exe`, 1,375,749 bytes, Authenticode `NotSigned`, SHA-256 `30786e6fdbbcc979081563ca8336a5a2a8b5df04bcf994628152a5ba0e9ff72b`.
- Installed runtime: `/health`, embedded assets, browser authentication/bootstrap, independent PID, clean `--stop`, uninstall, data retention and shell-state restoration pass in isolated directories containing spaces.
- Windowless process fix: native helpers and GUI-subsystem builds pass their regressions; two observed Windows acceptance intervals totaling about 13 minutes have zero console show events. The installed local binary was updated and the production service remains stopped. See [latest fix validation](windowless-processes.md).
- Remaining manual/external checks and original refactor evidence: [validation record](refactor-validation.md). This is a local candidate acceptance record, not proof of a remote CI or clean-VM pass.
- Version 2.1.0 local logs: `.cache/release-2.1.0-rust198-verify.log`, `.cache/release-2.1.0-rust198-openssh-browser.log`, `.cache/release-2.1.0-rust198-package.log` and `.cache/release-2.1.0-rust198-installer.log`. Advisory scans are recorded in the matching `pnpm-audit` and `cargo-audit` logs.

The first hosted Windows run exceeded the console probe's five-second PowerShell cold-start deadline. The probe now allows 30 seconds, keeps the `GetConsoleWindow() == 0` assertion, and terminates its child on timeout. Core tests and strict Clippy were rerun locally before rebuilding and rechecking the installer; the replacement CI run is linked from the final Release.

Local and CI release checks now use Rust 1.98.0 from `rust-toolchain.toml`. Its stricter byte-string and fixed-size chunk lints were addressed directly. The complete local verification, real OpenSSH/browser suite and exact rebuilt installer smoke passed with that toolchain. NSIS is resolved from its installation directory in CI, without depending on a freshly changed PATH.
