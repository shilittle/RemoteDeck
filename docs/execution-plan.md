# RemoteDeck v1.0 execution plan

The development specification is the highest-level contract. Milestones are sequential; each is committed only after its checks pass.

| Milestone | Scope | Status | Verification evidence | Commit |
| --- | --- | --- | --- | --- |
| M0 | Baseline audit, legacy preservation, branch, repository instructions, workspace shell | Complete | `pnpm lint`, `pnpm typecheck`, `pnpm test` (1), `pnpm build`; Electron 43.2.0 shell launch smoke; legacy PowerShell parse and blob-preservation checks | `4179e26` |
| M1 | Secure Electron boundaries, schemas, persistence, logging, base workbench, CI | Complete | `pnpm verify`; 13 unit, 1 integration, 1 Electron E2E; production build; settings restart persistence; sandbox/no-Node/narrow-IPC checks | `7c6807d` |
| M2 | Hosts, OpenSSH config, host keys, authentication, diagnostics, key lifecycle | Complete | `pnpm verify`; 24 unit, 4 real SSH integration, 1 Electron E2E; direct/two-server ProxyJump; Docker OpenSSH password → accepted fingerprint → encrypted Ed25519 deploy/dedup/fresh-key reconnect job | `feat(m2): implement verified SSH host and key lifecycle` |
| M3 | xterm PTY, tabs, resize, Unicode/IME, search, links, reconnect | In progress | Pending | Pending |
| M4 | SFTP tree, CRUD, recursive transfers, conflict/cancel/progress, drag/drop | Pending | Pending | Pending |
| M5 | Local/remote tunnels, dedicated sessions, health/recovery, proxy detection | Pending | Pending | Pending |
| M6 | Collector/supervisor, charts, processes/signals, GPU, btop/watchdog | Pending | Pending | Pending |
| M7 | Command editor/risk gates, built-ins, Codex/tmux lifecycle | Pending | Pending | Pending |
| M8 | Onboarding, legacy migration, task center, recovery, shortcuts, tray | Pending | Pending | Pending |
| M9 | Security audit, redaction, Windows E2E, packaging, docs, release candidate | Pending | Pending | Pending |

## Final gates

- `pnpm verify` succeeds from a clean dependency install.
- `pnpm dist:win` produces Windows x64 NSIS and portable artifacts.
- Docker/OpenSSH integration tests pass in CI or an equivalent local Docker environment.
- The manual acceptance script covers a real Linux host, host-key acceptance/mismatch, credentials, terminal programs, SFTP edge cases, tunnel recovery, suspend/resume, native drag/drop fallback, Codex login, tray behavior, and clean Windows installation.
- No unexplained TODO, FIXME, placeholder/disabled feature control, mock production data, or hard-coded laboratory default remains.
- Documentation, changelog, artifact hashes, known limitations, commits, and Draft PR are ready.
