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
| M6 | Collector/supervisor, charts, processes/signals, GPU, btop/watchdog | Complete | `pnpm verify`; 38 unit, 4 real SSH integration, 2 Electron E2E; v1 JSONL streaming, invalid/crash/timeout/network/stale recovery, no-Python/no-GPU/no-btop degradation, process revalidation and two-stage signal, real dashboard; Docker OpenSSH collector/btop/process job | `feat(m6): add supervised remote monitoring` |
| M7 | Command editor/risk gates, built-ins, Codex/tmux lifecycle | In progress | Pending | Pending |
| M8 | Onboarding, legacy migration, task center, recovery, shortcuts, tray | Pending | Pending | Pending |
| M9 | Security audit, redaction, Windows E2E, packaging, docs, release candidate | Pending | Pending | Pending |

## Final gates

- `pnpm verify` succeeds from a clean dependency install.
- `pnpm dist:win` produces Windows x64 NSIS and portable artifacts.
- Docker/OpenSSH integration tests pass in CI or an equivalent local Docker environment.
- The manual acceptance script covers a real Linux host, host-key acceptance/mismatch, credentials, terminal programs, SFTP edge cases, tunnel recovery, suspend/resume, native drag/drop fallback, Codex login, tray behavior, and clean Windows installation.
- No unexplained TODO, FIXME, placeholder/disabled feature control, mock production data, or hard-coded laboratory default remains.
- Documentation, changelog, artifact hashes, known limitations, commits, and Draft PR are ready.
