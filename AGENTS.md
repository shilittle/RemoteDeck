# RemoteDeck repository instructions

RemoteDeck 2 is a Windows 10/11 x64 desktop application built with Tauri 2, Rust, React and xterm.js. Historical user-data fixtures remain for migration tests, but the previous Electron implementation has been removed from the production source tree.

## Architecture boundaries

- The shipped process is a Tauri 2 Rust binary rendered by system WebView2. Do not reintroduce Electron, embedded Chromium, production Node.js, or a self-extracting portable target.
- Frontend code lives in `apps/desktop/tauri-ui`; native code lives in `apps/desktop/src-tauri`.
- The renderer may call only explicitly registered Tauri commands. Do not add broad shell, filesystem or HTTP plugins.
- Local SSH uses Windows OpenSSH. Interactive sessions use ConPTY through `portable-pty`; forwarding uses app-owned `ssh.exe` child processes.
- Windows NSIS is the only release target. Use WebView2 `downloadBootstrapper`; embedding offline or fixed runtimes recreates the original 100+ MB problem.

## Security invariants

- First-use and changed SSH host keys are never trusted silently. Scan, display SHA-256 fingerprints, require explicit acceptance, rescan before persistence, and use `StrictHostKeyChecking=yes` on every connection.
- The app owns a dedicated `known_hosts` file and never mutates the user's global OpenSSH files.
- Passwords and private-key passphrases are never persisted, logged or passed through Tauri invoke arguments. Interactive prompts remain in the PTY.
- Validate every value used as an OpenSSH argument. Stop only processes created by the current app instance.
- Do not log terminal input/output or remote command output by default.

## Required commands

```text
pnpm dev
pnpm lint
pnpm typecheck
pnpm test
pnpm test:integration
pnpm build
pnpm dist:win
pnpm verify
```

Rust gates:

```text
cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --all-features
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets --all-features -- -D warnings -A linker-messages
```

## Completion discipline

- A control performs a real operation or is absent. No fake telemetry, fake connection state, inert buttons or placeholder production data.
- Keep the NSIS installer below the 40 MiB CI ceiling.
- Preserve the complete native feature set: host profiles and trust, terminal, SFTP transfers, commands and task center, telemetry and btop, tunnels, key lifecycle, agent sessions, settings, diagnostics and legacy-data migration.
