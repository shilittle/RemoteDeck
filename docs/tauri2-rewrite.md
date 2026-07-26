# RemoteDeck 2 — Tauri 2 rewrite

## Decision

RemoteDeck 1 bundled Electron 43 and produced roughly 113 MB Windows artifacts. Its portable target unpacked the whole application into a temporary directory on every launch. RemoteDeck 2 replaces that architecture instead of attempting another Electron packaging pass.

The new application consists of one Rust executable, static React assets and the Windows-provided WebView2 runtime. The installer uses Tauri's `downloadBootstrapper` mode, so WebView2 contributes no embedded runtime payload. Only NSIS is emitted; the self-extracting portable target is removed.

## Implemented vertical slice

- Atomic, versioned persistence for host, tunnel and terminal settings.
- Windows OpenSSH discovery without shipping another SSH implementation.
- First-use host-key scan with SHA-256 fingerprints, explicit acceptance and a mandatory rescan before writing the app-owned `known_hosts` file.
- `StrictHostKeyChecking=yes` on tests, commands, terminals and tunnels.
- Interactive OpenSSH through ConPTY via `portable-pty`, including input, output, resize and owned-process cleanup.
- One-shot commands with bounded output and optional remote working directory.
- Independent local (`-L`) and remote (`-R`) tunnel processes with `ExitOnForwardFailure=yes`.
- React workbench for profiles, trust, terminal, tunnels, commands and terminal settings.
- CI for frontend checks, Rust formatting/tests/clippy, NSIS construction and a hard 40 MiB ceiling.

## Not yet feature-complete

This first slice does not claim parity with Electron v1. SFTP, telemetry/GPU/process dashboards, btop supervision, command preset risk gates, Codex/tmux lifecycle, legacy migration, tray/login behavior and diagnostic export require native rewrites and acceptance tests. The old Electron source remains only as a behavioral reference and is excluded from compilation and packaging.

## Security model

The renderer has no generic shell, process, filesystem, socket or HTTP capability. It can invoke only commands registered in `src-tauri/src/lib.rs`. Passwords and key passphrases remain inside OpenSSH's PTY prompt and never cross the Tauri invoke boundary. Host-key records live in the application data directory, separate from global OpenSSH files.

## Build

Windows prerequisites: Node.js 24, pnpm 11, stable Rust, Microsoft C++ Build Tools, WebView2 and Windows OpenSSH Client.

```powershell
pnpm install --frozen-lockfile
cargo install tauri-cli --version 2.11.2 --locked
pnpm verify
pnpm dist:win
```

The installer is emitted under `apps/desktop/src-tauri/target/release/bundle/nsis/`.
