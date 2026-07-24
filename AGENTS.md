# RemoteDeck repository instructions

This repository is implementing the RemoteDeck v1.0 Windows desktop application. The attached development specification is authoritative; `docs/execution-plan.md` records progress and verification evidence.

## Product and architecture boundaries

- Target Windows 10/11 x64 as the local platform and Linux with OpenSSH Server as the remote platform.
- Use Electron, strict TypeScript, React, Zustand, Zod, ssh2, xterm, ECharts, Pino, Vitest, Playwright Electron, and electron-builder.
- Keep `main`, `preload`, and `renderer` separate. The renderer has `contextIsolation: true`, `nodeIntegration: false`, and sandboxing enabled.
- The preload exposes only narrow, versioned APIs whose arguments and results are validated against shared Zod schemas. Never expose arbitrary IPC, filesystem, process, socket, or shell access.
- Core business modules do not import React and should avoid importing Electron. State transitions originate in main/core; the renderer only requests actions and subscribes to state.
- Keep the v0.1.0 PowerShell application unchanged in `legacy/labpulse-v0.1.0/`. The new app must not invoke it.
- Do not add Monaco, a VS Code extension, Code-OSS, a local system service, a custom Codex chat UI, DynamicForward, Windows remote hosts, or non-Windows desktop release targets in v1.0.

## Security invariants

- Never silently trust a first-use or changed SSH host key. Persist a key only after explicit acceptance; hard-block mismatches and display both fingerprints.
- Passwords and private-key passphrases are memory-only, are cleared after use when possible, and are never written to configuration, logs, CLI arguments, diagnostics, or renderer persistence.
- Do not read, copy, display, or transmit `~/.codex/auth.json` or OpenAI credentials.
- Do not log terminal input/output by default. Redact passwords, passphrases, private keys, tokens, authorization headers, and obvious secrets from logs and diagnostics.
- SFTP uploads and downloads use owned temporary files followed by rename; cleanup affects only files created by this app instance.
- Tunnel stop/cleanup affects only listeners and SSH resources owned by the current app instance. Never kill an arbitrary process merely because it occupies a port.
- The imported legacy cleanup command is disabled by default, displayed verbatim with a high-risk warning, and can run during recovery only after explicit one-time authorization.
- L1 commands show the full command and target and require confirmation. L2 commands also require typing the host alias or declared confirmation text. Free terminal input cannot be reliably intercepted; document this boundary.
- Do not default Codex to `--dangerously-bypass-approvals-and-sandbox` or parse Codex TUI output. Use stable CLI behavior detected from the installed version.
- OpenSSH config writes are idempotent, backed up, atomic, validated, and rolled back on failure. Preserve user comments and unsupported directives.
- Every connection generation rejects stale asynchronous callbacks. Reconnect uses capped exponential backoff with jitter; manual reconnect resets the backoff.

## Required root commands

```text
pnpm dev
pnpm lint
pnpm typecheck
pnpm test
pnpm test:integration
pnpm test:e2e
pnpm build
pnpm dist:win
pnpm verify
```

`pnpm verify` must run lint, typecheck, unit tests, integration tests, and the production build. Milestone-specific checks must pass before committing that milestone.

## Completion discipline

- Implement milestones M0 through M9 in order. Update `docs/execution-plan.md`, relevant documentation, and tests at every milestone.
- Each milestone ends with a focused, reversible Git commit after its lint, typecheck, tests, and build pass.
- Buttons must perform real actions or be absent. Do not ship mock data, placeholder UI, disabled feature shells, unexplained TODO/FIXME markers, or claims of session recovery that a plain SSH PTY cannot provide.
- Use Docker/OpenSSH fixtures and mocked external boundaries for automated validation; provide an explicit manual acceptance script for credentials, native drag/drop, suspend/resume, clean-machine packaging, and real Codex login.
- v1.0 is complete only when `pnpm verify` is green, Windows x64 NSIS and portable artifacts exist, documentation and release checks are complete, and no unexplained placeholders remain.
- If GitHub authentication is available, push `codex/remotedeck-v1` and open a Draft PR. Never merge it or publish a formal Release automatically.

