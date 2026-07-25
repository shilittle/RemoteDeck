# Changelog

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
