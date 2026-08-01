# RemoteDeck 2.0.0 release checklist

Release preparation updated: 2026-08-02. Only boxes backed by the exact final commit and installer may be checked. All boxes remain evidence items; this file does not announce completion.

## Source and automated gates

- [ ] Versions match in root package, desktop package, Cargo, Tauri config, tag, and manifest.
- [ ] Frozen `pnpm install` and `pnpm audit --audit-level high` pass.
- [ ] `pnpm verify` passes lint, TypeScript, frontend unit/integration tests, command-boundary verification, and production build.
- [ ] `cargo fmt --check`, all-feature Rust tests, and strict all-target/all-feature Clippy pass.
- [ ] Independent terminal transport, SSH/SFTP, tunnel, Agent/migration, ProxyJump, telemetry/btop, host lifecycle, persistence, and frontend contract reviews have no open release blocker.
- [ ] Source audit has no production TODO/FIXME, mock data, inert control, broad Tauri plugin, Electron runtime dependency, or unexplained warning suppression.

## Exact package acceptance

- [ ] `pnpm dist:win` produces only the Windows x64 NSIS target.
- [ ] Installer is below 40 MiB and Authenticode status is recorded.
- [ ] The unpackaged release binary launches and exposes a non-zero main-window handle without early exit.
- [ ] The exact installer silently installs into a clean per-user directory.
- [ ] The installed executable launches and exposes a non-zero main-window handle.
- [ ] The installed uninstaller succeeds and removes the temporary installation.
- [ ] A visual smoke pass covers onboarding, all seven workspaces, host editor, native pickers, Agent plan confirmation, and no clipped/garbled text.
- [ ] `SHA256SUMS.txt` and `release-manifest.json` match the exact uploaded installer bytes.

## External SSH acceptance

- [ ] Direct and saved-profile ProxyJump connections refuse unknown/changed keys and succeed after independently verified acceptance.
- [ ] Interactive password/key-passphrase prompts remain inside the terminal and are absent from persisted state and diagnostics.
- [ ] Terminal passwords/passphrases remain inside ConPTY; loopback input rejects wrong host/path/origin, expired/replayed tickets, stale generations, oversized frames, and excess buffering.
- [ ] Unicode SFTP upload/download, transactional overwrite/rollback, conflict choices, cancellation/current-profile retry, host-retirement barriers, and recursive symlink-safe behavior pass on a trusted Linux host.
- [ ] Local and remote tunnels pass direction-correct health, current-profile reconnect, revision ordering, bounded-log, active-route edit, delete-during-start, and full-quit cleanup checks.
- [ ] Telemetry passes healthy, malformed, oversized, revised-event ordering, current-profile reconnect, no-Python/no-GPU/no-btop degradation, start-tick-bound TERM/KILL with native confirmation, stable-owner watchdog adoption/bootstrap lease, and bounded cleanup checks.
- [ ] Host deletion rejects referenced ProxyJump hosts, retires new SFTP/terminal/command work, waits for active operations, cleans tunnel/telemetry/btop state, and persists removal only after cleanup succeeds.
- [ ] Codex, Claude, Gemini, and OpenCode plans/probes are provider-specific; account login remains a user-controlled external step.

## Publication

- [ ] Final commit is pushed to the public repository and CI is green for that exact SHA.
- [ ] The release branch is merged to `main`; signed or annotated `v2.0.0` tag points to the tested merge commit.
- [ ] GitHub Release is non-draft, contains bilingual notes, installer, checksums, and JSON manifest.
- [ ] GitHub-reported asset bytes/digest and an independent post-upload download match local evidence.

Real passwords, private-key passphrases, host fingerprint approval, remote package-manager trust, and Agent account approval are intentionally never scripted.
