# Changelog

## 2.1.0 — 2026-09-08

### Security

- Updated the `brace-expansion` compatibility facade to 5.0.9 and pinned patched `nanoid` 3.3.18 and `postcss` 8.5.23 transitive dependencies, clearing GHSA-rgw5-rvv9-x895, GHSA-2v37-7h3g-55p8, and GHSA-fxqj-rqcc-2cmp from the pnpm audit.
- Moved strict peer-dependency policy into `pnpm-workspace.yaml`, where pnpm 11 actually reads non-registry project settings.

### Fixed

- Prevented console popups from SSH probes, telemetry, commands, key tools and other native helpers with a shared hidden-process constructor. Both debug and release services use the Windows GUI subsystem.
- Preserved live ConPTY terminals across browser reloads and connection loss, prevented replay from injecting terminal input, and confirmed actual child exit during cleanup.
- Made release verification wait for NSIS uninstallation to finish before restoring existing installation metadata, and preserved recovery backups on failure.

- Kept strict Clippy green on Rust 1.95 with an equivalent telemetry monitor-generation guard.
- Added push CI coverage for `codex/**` branches, exercised generated ProxyCommand syntax against Windows OpenSSH, and removed the stale 2.0.0 label from ordinary Windows CI artifacts.
- Fixed release validation ordering so `apps/web/dist` exists before every Rust server compile, checked the Linux `rfd` XDG portal dependency, installed NSIS independently on Windows runners, and restored pre-existing per-user shell state after an isolated installer smoke test.

### Changed

- Replaced the Tauri/WebView2 desktop shell with the Rust `remotedeck-server` local service and the `@remotedeck/web` browser client. The service owns SSH/ConPTY sessions and background tasks across browser closure and exposes explicit HTTP, SSE and terminal WebSocket boundaries.
- Switched the release target to a current-user NSIS installer containing `RemoteDeck.exe` with embedded `apps/web/dist`; the installer has no portable, Electron, Tauri, WebView2 or production Node.js payload and must remain below 40 MiB.
- Added runtime-descriptor and loopback-API readiness checks, bounded terminal replay, service lifecycle documentation, and a release candidate workflow that records artifacts without publishing them.

## 2.0.1 — 2026-08-01

### Security

- Updated `serde_with` from 3.17.0 to 3.21.0 to include the fix for GHSA-7gcf-g7xr-8hxj; the complete frontend, Rust, real OpenSSH, and Windows installer gates remain green.
- Triaged the `glib` 0.18 advisory as not used by the Windows x64 distribution: it exists only in the Linux GTK dependency graph, is not compiled into the installer, and no affected `VariantStrIter` call is present.

### Fixed

- Bound repository context explicitly in the no-checkout release publisher so draft creation, asset round-trip verification, and publication work reliably from the artifact-only job.

## 2.0.0 — 2026-08-01

### Changed

- Replaced the Electron/Node/`ssh2` desktop runtime with a new Tauri 2, Rust, React, xterm.js, Windows OpenSSH, and ConPTY architecture.
- Reduced distribution to one current-user NSIS installer using the WebView2 download bootstrapper, with a strict 40 MiB release gate and no portable target.
- Rebuilt every application workspace—hosts and trust, terminals, SFTP and transfers, tunnels, monitoring, commands, AI agents, settings, migration, diagnostics, task center, tray, and login lifecycle—behind typed Tauri commands.
- Added Codex, Claude Code, Gemini CLI, and OpenCode provider-neutral remote workflows.
- Added transactional migration from both LabPulse SSH v0.1.0 and RemoteDeck v1 state.

### Security

- All connections use Windows OpenSSH with `-F none`, an app-owned `known_hosts`, strict host-key checking, explicit rescan-before-acceptance, and hard failure on changed keys.
- Removed production Electron, Chromium, Node.js, `ssh2`, generic shell/filesystem/HTTP plugins, and portable packaging.
- Added conservative L0/L1/L2 command analysis, bounded output and concurrency, process-identity checks, atomic persistence and migration, recursive redaction, and owned-process/temp-file cleanup.
- Added command-surface parity checks, strict Rust Clippy, clean installer launch/uninstall tests, artifact size enforcement, SHA-256 manifests, and independent subsystem reviews.

## 1.0.1 — 2026-07-25

### Changed

- Added a bilingual Chinese/English README and complete user guides.
- Documented the intentionally unsigned Windows distribution and SHA-256 verification procedure.
- Published the source repository with a bilingual security policy and coordinated vulnerability-reporting instructions.
- Made Docker tunnel traffic probes resilient to bounded transient connection resets without weakening their failure timeout.

### Security

- Routed legacy and modern minimatch consumers through patched `brace-expansion` 5.0.8 compatibility exports to resolve CVE-2026-14257.

## 1.0.0 — 2026-07-25

### Added

- Independent Windows 10/11 x64 Electron desktop architecture while preserving LabPulse SSH v0.1.0 and Git history.
- Verified multi-host SSH with password, keyboard-interactive, encrypted private-key and agent authentication, one-level ProxyJump, managed OpenSSH Include and public-key deployment.
- Real multi-tab xterm PTYs, SFTP operations/transfers, LocalForward/RemoteForward, structured Linux telemetry, process controls and btop watchdog.
- Command library with L0/L1/L2 enforcement and remote Codex CLI install/login/start/resume/update workflow through standard SSH PTYs and tmux.
- First-run onboarding, explicit idempotent legacy migration, unified task center, keyboard navigation, tray keepalive/full quit, launch at login and redacted ZIP diagnostics.
- Windows NSIS and portable packaging, hardened Electron fuses, Docker OpenSSH integration, Playwright E2E, packaged startup and clean-install validation.

### Security

- Sandboxed/context-isolated renderer, fixed typed IPC, restrictive CSP, deny-by-default navigation/permissions, ASAR integrity and disabled runtime escape fuses.
- Host-key hard failure, memory-only credentials, owned-resource cleanup, conservative command classification, process identity revalidation and recursive secret redaction.
- Generated Ed25519 keys are validated as a matching parseable pair before they are written, with bounded regeneration and owned-file rollback.

### Compatibility

- Existing LabPulse files remain under `legacy/labpulse-v0.1.0`; users can preview and import its supported settings without modifying the source file.
- See `docs/known-limitations.md` for the intentionally bounded v1 scope.
