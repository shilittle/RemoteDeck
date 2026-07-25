# Verification and test matrix

## Local gates

```powershell
pnpm lint
pnpm typecheck
pnpm test
pnpm test:integration
pnpm build
pnpm test:e2e
pnpm dist:win
```

`pnpm verify` combines the first five commands. `pnpm audit --audit-level high` is run for the release audit. The Windows release job additionally launches the portable artifact, silently installs/launches/uninstalls the NSIS artifact, and uploads EXE/blockmap files.

Signed publication uses a separate fail-closed gate:

```powershell
pnpm dist:win:signed
pnpm verify:signatures
```

`dist:win:signed` enables electron-builder `forceCodeSigning`, so missing credentials cannot silently produce an unsigned release. `verify:signatures` requires valid Authenticode signatures and RFC 3161 timestamps on packaged `RemoteDeck.exe`, the NSIS installer, and the portable EXE. The manual GitHub workflow repeats `verify`, Electron E2E, both packaged launch smokes, signature verification, and SHA-256 manifest generation before uploading its short-lived artifact. See [`code-signing.md`](code-signing.md).

## Required behavior coverage

| Requirement | Automated evidence | Additional acceptance |
| --- | --- | --- |
| OpenSSH `Include` idempotency, backup, rollback | `open-ssh-config.test.ts` | inspect user-selected config only |
| first-use key confirmation and changed-key hard failure | `host-key.test.ts`, `ssh-host-key-and-password.test.ts` | compare real-host fingerprint out of band |
| password, private key, keyboard-interactive, agent boundaries | SSH/core tests and Docker fixture | real credentials remain user supplied |
| `authorized_keys` material dedup and fresh-key reconnect | `authorized-keys.test.ts`, OpenSSH job | Docker key lifecycle |
| POSIX path/shell quoting | SFTP, terminal, command and OpenSSH tests | CJK/space path checklist |
| connection/tunnel recovery state | `tunnel-service.test.ts`, telemetry tests | suspend/resume checklist |
| owned-resource cleanup only | tunnel, transfer, terminal and command cancellation tests | full quit process check |
| SFTP CJK/space/large/cancel/conflict/symlink | `transfer-service.test.ts`, Docker OpenSSH job | 100 MiB fixture |
| telemetry schema/stale/crash/no-GPU | `telemetry-service.test.ts` | Docker collector/btop job |
| command risk and confirmations | `risk-engine.test.ts`, `command-service.test.ts` | L1/L2 UI check |
| secret-free logs/diagnostics | `redaction.test.ts`, `diagnostics-service.test.ts` | inspect exported ZIP |
| Clash multiple/none | `clash-detector.test.ts` | read-only local candidate check |
| migration idempotency | `legacy-migration-service.test.ts`, Electron E2E | old file remains unchanged |
| tray keepalive/full-quit cleanup | Electron E2E and `m8-tray.ps1` | real tunnel checklist |
| renderer without Node/generic IPC | `window-security.test.ts`, IPC tests, Electron E2E | packaged fuse readback |

## CI

- Ubuntu quality plus Docker OpenSSH: builds a local SSH server, exercises direct SSH, two-server ProxyJump, encrypted Ed25519 deployment/dedup, PTY programs, Unicode SFTP, 100 MiB cancellation, LocalForward/RemoteForward isolation, forced recovery, collector, btop, process signaling, and the mocked external Codex account boundary.
- Windows quality: lint, typecheck, unit/integration tests and Electron build.
- Windows release candidate: Electron Playwright E2E, NSIS + portable build, packaged startup, clean install/uninstall, artifact upload.

When Docker or real credentials are not available locally, the checked-in OpenSSH fixture, fake external boundaries, PowerShell package scripts, and [`release-checklist.md`](release-checklist.md) preserve the acceptance procedure. CI remains the authoritative Docker run.
