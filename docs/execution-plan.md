# RemoteDeck v1.0 execution plan

The development specification is the highest-level contract. Milestones are sequential; each is committed only after its checks pass.

| Milestone | Scope | Status | Verification evidence | Commit |
| --- | --- | --- | --- | --- |
| M0 | Baseline audit, legacy preservation, branch, repository instructions, workspace shell | Complete | `pnpm lint`, `pnpm typecheck`, `pnpm test` (1), `pnpm build`; Electron 43.2.0 shell launch smoke; legacy PowerShell parse and blob-preservation checks | `4179e26` |
| M1 | Secure Electron boundaries, schemas, persistence, logging, base workbench, CI | Complete | `pnpm verify`; 13 unit, 1 integration, 1 Electron E2E; production build; settings restart persistence; sandbox/no-Node/narrow-IPC checks | `7c6807d` |
| M2 | Hosts, OpenSSH config, host keys, authentication, diagnostics, key lifecycle | Complete | `pnpm verify`; 24 unit, 4 real SSH integration, 1 Electron E2E; direct/two-server ProxyJump; Docker OpenSSH password → accepted fingerprint → encrypted Ed25519 deploy/dedup/fresh-key reconnect job | `2c44009` |
| M3 | xterm PTY, tabs, resize, Unicode/IME, search, links, reconnect | Complete | `pnpm verify`; 25 unit, 4 real SSH integration, 2 Electron E2E; real PTY keyboard/CJK/clipboard/Ctrl+C/tabs/resize/search/reconnect; Docker OpenSSH bash/vim/tmux/btop/Chinese-path acceptance job | `dd9face` |
| M4 | SFTP tree, CRUD, recursive transfers, conflict/cancel/progress, drag/drop | Complete | `pnpm verify`; 26 unit, 4 real SSH integration, 2 Electron E2E; production transfer engine with memory-SFTP Unicode/recursive/conflict/cancel coverage; Docker OpenSSH empty/CJK/100 MiB/symlink/cleanup acceptance job | `c768423` |
| M5 | Local/remote tunnels, dedicated sessions, health/recovery, proxy detection | Complete | `pnpm verify`; 31 unit, 4 real SSH integration, 2 Electron E2E; isolated LocalForward/RemoteForward, bind conflict, forced disconnect recovery, Clash protocol probes, UI profile round-trip; Docker OpenSSH bidirectional traffic/isolation job | `966babf` |
| M6 | Collector/supervisor, charts, processes/signals, GPU, btop/watchdog | Complete | `pnpm verify`; 38 unit, 4 real SSH integration, 2 Electron E2E; v1 JSONL streaming, invalid/crash/timeout/network/stale recovery, no-Python/no-GPU/no-btop degradation, process revalidation and two-stage signal, real dashboard; Docker OpenSSH collector/btop/process job | `112c0e8` |
| M7 | Command editor/risk gates, built-ins, Codex/tmux lifecycle | Complete | `pnpm verify`, `pnpm test:e2e`; 48 unit, 4 integration, 2 Electron E2E; CRUD/global/host presets, built-ins, core L0/L1/L2 revalidation, streamed exec/cancel/PTY, official Codex probe/install/login/start/resume/update, stable tmux association; Docker external-boundary fixture and real-host script | `523df7f` |
| M8 | Onboarding, legacy migration, task center, recovery, shortcuts, tray | Complete | `pnpm verify`, `pnpm test:e2e`; 49 unit, 4 integration, 2 Electron E2E; UI-only first-run host→SSH→Codex route, hashed preview/import with disabled cleanup hook and legacy risk rules, unified task recovery, shortcuts, login launch, native tray close/restore/quit, SSH preserved while hidden, responsive states | `e068160` |
| M9 | Security audit, redaction, Windows E2E, packaging, docs, release candidate | Complete | `pnpm verify`, `pnpm test:e2e`, `pnpm audit`, `pnpm dist:win`; 53 unit, 4 integration, 2 Electron E2E; two-container Docker direct/ProxyJump and complete SSH lifecycle; portable startup and clean NSIS install/launch/uninstall; packaged fuse readback; secret scan; final hashes and release docs | `1308435` |

## Final gates

- `pnpm verify` succeeds from a clean dependency install.
- `pnpm dist:win` produces Windows x64 NSIS and portable artifacts.
- Docker/OpenSSH integration tests pass in CI or an equivalent local Docker environment.
- The manual acceptance script covers a real Linux host, host-key acceptance/mismatch, credentials, terminal programs, SFTP edge cases, tunnel recovery, suspend/resume, native drag/drop fallback, Codex login, tray behavior, and clean Windows installation.
- No unexplained TODO, FIXME, placeholder/disabled feature control, mock production data, or hard-coded laboratory default remains.
- Documentation, changelog, artifact hashes, known limitations, commits, and Draft PR are ready.

## Post-release public/signing hardening — 2026-07-25

- [x] Rewrite the README and full user guide in Simplified Chinese and English.
- [x] Add a fail-closed Authenticode build, signer/timestamp verification, signing runbook, and private vulnerability-reporting policy.
- [x] Scan the full Git history and existing Actions logs for credential-shaped tokens, private keys, real user paths, and unsafe release artifacts before changing visibility.
- [x] Resolve CVE-2026-14257 by routing minimatch 3/5/9/10 through the patched upstream `brace-expansion` 5.0.8 implementation, then verify every compatibility shape.
- [x] Re-run frozen install, `pnpm verify`, Electron E2E, dependency audit, Windows packaging, portable launch, and clean NSIS installation.
- [ ] Supply a trusted Authenticode certificate and password, produce a signed build, and verify the publisher plus RFC 3161 timestamps.
- [ ] Publish the documentation/security commit, observe CI, enable private vulnerability reporting, and change the repository visibility to public.
