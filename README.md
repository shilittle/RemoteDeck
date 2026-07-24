# RemoteDeck

RemoteDeck is a Windows 10/11 x64 SSH workspace for Linux remote-development workflows. It brings multi-host SSH configuration, verified authentication, interactive terminals, SFTP, durable tunnels, structured monitoring, command safety, and the Codex CLI lifecycle into one desktop application.

RemoteDeck v1.0 is under active development on `codex/remotedeck-v1`. The preserved LabPulse SSH v0.1.0 release remains available in [`legacy/labpulse-v0.1.0`](legacy/labpulse-v0.1.0/README.md).

## Development

Requirements: Node.js 24 and pnpm 11.

```powershell
pnpm install --frozen-lockfile
pnpm dev
```

Quality gates:

```powershell
pnpm lint
pnpm typecheck
pnpm test
pnpm verify
pnpm dist:win
```

See [`docs/execution-plan.md`](docs/execution-plan.md), [`docs/ssh.md`](docs/ssh.md), [`docs/security.md`](docs/security.md), [`docs/baseline-v0.1.md`](docs/baseline-v0.1.md), and [`docs/scope-v1.md`](docs/scope-v1.md).
