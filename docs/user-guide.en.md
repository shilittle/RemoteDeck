# RemoteDeck User Guide (English)

[简体中文](user-guide.zh-CN.md) · [Back to README](../README.md) · [Download releases](https://github.com/shilittle/RemoteDeck/releases)

## 1. Before installation

RemoteDeck supports Windows 10/11 x64 and includes its Electron/Node runtime. Remote targets must be Linux systems running OpenSSH Server. Structured monitoring requires `python3`; GPU and btop features degrade explicitly when their tools are unavailable.

Download either the NSIS installer or portable EXE from GitHub Releases, then perform both checks:

1. Confirm a valid signature on the file's **Digital Signatures** property page, or run:

   ```powershell
   Get-AuthenticodeSignature .\RemoteDeck-*-win-x64-setup.exe |
     Format-List Status,StatusMessage,SignerCertificate,TimeStamperCertificate
   ```

2. Calculate SHA-256 and compare it with the Release notes:

   ```powershell
   Get-FileHash .\RemoteDeck-*-win-x64-setup.exe -Algorithm SHA256
   ```

Treat a build as signed only when its Release explicitly says so and Authenticode reports `Valid`. The initial v1.0.0 assets are unsigned and may trigger a Windows SmartScreen unknown-publisher warning.

## 2. Installation and first run

- NSIS installer: installs per user, lets you choose the destination, and can create Start-menu/desktop shortcuts.
- Portable build: runs as one EXE. “Portable” means no installation; settings still use the current Windows user's application-data directory.

First-run onboarding offers two real paths:

- Add a new host.
- Migrate LabPulse SSH v0.1.0. RemoteDeck reads only the `config.json` selected through the native picker, previews it before importing, prevents duplicate imports, and keeps any legacy cleanup command disabled.

## 3. Hosts and authentication

Enter an alias, host/IP, port, username, and optional working directory. Authentication options are:

- Password: requested when needed and kept in memory only.
- Keyboard-interactive: answer server prompts in the connection dialog; answers remain memory-only.
- Private key: select it with the native file picker; encrypted-key passphrases remain memory-only.
- Windows OpenSSH agent: use keys from the current user's agent.

ProxyJump is intentionally limited to one level. The jump and target hosts are verified and authenticated separately.

The first connection shows the algorithm, SHA-256 fingerprint, and endpoint. Compare the fingerprint through a server console, administrator, or another independent trusted channel before accepting it. A changed key hard-fails and shows both fingerprints; investigate first, then remove the old record manually only when the change is legitimate.

**Deploy public key**:

1. Reads the selected public-key material.
2. Atomically updates remote `authorized_keys` through an app-owned temporary file.
3. Deduplicates by key material.
4. Opens a fresh connection using the new key.
5. Changes the default authentication method only after verification succeeds.

## 4. Terminal

Open **Terminal** after connecting and create a tab. Every tab is a real SSH PTY supporting bash, vim, tmux, btop, Unicode/IME, synchronized resizing, scrollback search, Ctrl+C, and text clipboard operations.

- Closing a plain PTY ends that remote shell and cannot recover its processes.
- Use tmux/screen for persistent work. The Codex workflow can use a deterministic RemoteDeck-owned tmux session.
- Only HTTP(S) links are accepted; inspect the destination before opening it.
- Free terminal input is outside the command classifier. L0/L1/L2 rules apply to the command library and fixed Codex operations.

Common shortcuts:

| Shortcut | Action |
| --- | --- |
| `Ctrl+1…7` | Switch activity |
| `Ctrl+Shift+T` | New terminal |
| `Ctrl+J` | Open task center |
| `Ctrl+,` | Open settings |

## 5. Files and transfers

The **Files** workspace uses SFTP and never constructs shell commands from paths. It supports:

- Hidden files, new folders/empty files, rename, and delete.
- Drag-and-drop or picker uploads and local-directory downloads.
- Recursive directory transfer with skip/overwrite/rename conflict policies.
- Byte progress, speed, cancellation, and retry for large files.

Recursive operations use `lstat` and do not follow symbolic links. Uploads and downloads first write app-instance/job-owned temporary files and rename only after success. Cancellation cleans up only the exact paths owned by that job.

## 6. SSH tunnels

Save LocalForward or RemoteForward profiles. Each active tunnel owns a dedicated SSH connection and does not consume a terminal. TCP/HTTP health checks, failure backoff, and network recovery are built in.

- A normal port conflict never terminates the process occupying that port.
- Clash/Mihomo discovery is read-only; multiple candidates require a user choice.
- A migrated legacy cleanup command remains disabled and is shown with a complete high-risk warning. It can run during recovery only after one-time explicit authorization.

## 7. Monitoring, btop, and processes

RemoteDeck streams its packaged Python collector to remote `python3 -u -` through an authenticated SSH channel. It does not install a remote file. Main applies size bounds and strict schema validation to JSONL samples.

- Missing `python3`, GPU tools, or btop produces an explicit degraded state, never fabricated data.
- The btop watchdog manages only sessions created by RemoteDeck and does not parse or log btop screen content.
- Signals can target only a process owned by the current SSH user that still matches a fresh `ps` check.
- SIGKILL requires a recent SIGTERM against the same process plus a second confirmation.

## 8. Command library and Codex

Presets can be global or host-specific. Main reloads the final command and calculates its effective risk immediately before execution:

- L0: allowlisted read-only command; runs directly.
- L1: displays the full target and command and requires confirmation.
- L2: also requires the host alias or declared confirmation text.

Declared risk is a floor and cannot downgrade main's classification. PTY commands open in a normal terminal; non-PTY jobs show bounded output in the task center and can be canceled.

The Codex panel runs fixed capability probes and stable commands advertised by the installed CLI version. The install plan displays the official script and requires confirmation. Login uses device authentication or an SSH PTY. RemoteDeck never reads `~/.codex/auth.json`, accepts a token, parses TUI bytes, or adds a sandbox-bypass flag.

## 9. Tray, quit, and login startup

By default, closing the window hides it to the tray while main continues to own SSH sessions, transfers, monitoring, and tunnels. Only **Quit completely** or the operating-system quit path disposes every app-owned resource.

Settings can:

- Disable tray keepalive.
- Start RemoteDeck after Windows login.
- Start with the window hidden.

## 10. Diagnostics and privacy

**Export diagnostics** uses a native save dialog to create a ZIP containing:

- Application version and runtime capabilities.
- Redacted settings and anonymized profile counts.
- Up to five recent category logs, each capped at 512 KiB.

The archive excludes terminal/SFTP content, endpoint identities, passwords, private keys, passphrases, and Codex tokens. A second secret scan runs before the ZIP is written. Inspect the archive yourself before sharing it with maintainers.

## 11. Troubleshooting

1. Read the explicit state and error in Hosts, Tunnels, or Task Center.
2. Never bypass a changed host key; verify the new fingerprint from the server console.
3. Confirm remote OpenSSH, `python3`, and required ports are available.
4. Run `Get-AuthenticodeSignature` for signing problems and `Get-FileHash` for download corruption.
5. Export and inspect diagnostics before attaching them to a GitHub Issue.
6. Review [known limitations](known-limitations.md) and the feature-specific documentation.

## 12. Uninstall

Uninstall the NSIS build through Windows **Installed apps**. User data is retained by default to avoid accidental profile loss. Export anything you need, then remove the RemoteDeck application-data directory manually as the current user if desired. Deleting the portable EXE does not automatically remove its user-data directory.
