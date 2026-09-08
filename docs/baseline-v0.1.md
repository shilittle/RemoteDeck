# Archived: LabPulse SSH v0.1.0 baseline audit

> **历史基线，不是当前功能或安全声明。** Current behavior is documented in [architecture.md](architecture.md) and [known-limitations.md](known-limitations.md).

Audit source: Git commit `75301fee84e2b5d9c3409a207d712122d652200d` (`v0.1.0`). The exact tracked files were moved without content changes to `legacy/labpulse-v0.1.0/`; Git rename detection preserves history.

## Runtime and entry points

- `Start-LabPulseSSH.cmd` starts `LabPulseSSH.ps1` in hidden STA PowerShell; the debug launcher leaves the console visible.
- The application is a 1,268-line PowerShell/WPF file. It requires Windows PowerShell, WPF, and the system `%WINDIR%\System32\OpenSSH\ssh.exe`.
- `-SelfTest` resolves the configured SSH alias with `ssh -G`, checks TCP reachability, probes `btop` and `python3`, and verifies that the forwarding alias has a `RemoteForward` directive.
- `config.json` is the only configuration store. It hard-codes the laboratory aliases `lab` and `lab-codex`, local port 7890, and remote port 17890.

## Confirmed behavior

- A dark, Chinese WPF dashboard has performance, btop, command, forwarding, and log tabs.
- Telemetry starts `ssh -T` with redirected stdin, streams `remote_telemetry.py`, consumes line-delimited JSON, retains short histories, and restarts stale or exited collectors.
- The Python collector reads `/proc/stat`, `/proc/meminfo`, `/proc/net/dev`, `/proc/uptime`, `/` and `/data`, thermal zones, `ps`, `nvidia-smi`, btop process count, and whether the configured remote port is listening.
- btop uses a real TTY. Hidden watchdog and interactive modes restart btop after exit and rotate the process after the configured interval. Its ANSI output is not parsed.
- Safe command presets execute through SSH. Regex-classified dangerous commands show the target and command, require a second typed confirmation, and then open a separate interactive terminal for sudo/password entry.
- Reverse forwarding starts `ssh -NT` using an existing alias, `ExitOnForwardFailure`, keepalives, and a watchdog. Recovery runs the configured release command, then applies bounded restart delay.
- The WPF window minimizes normally, but closing it exits and tears down telemetry, btop, forwarding, and command processes. There is no system tray lifecycle.
- Logs are daily plain-text files. Terminal output is shown in UI; the log function does not intentionally record credentials.

## Security and architecture gaps to avoid carrying forward

- Nearly all UI, state, subprocess, SSH, telemetry, retry, and security logic shares one script and the UI dispatcher. There are no domain boundaries, typed contracts, or automated tests.
- Authentication and host-key verification are delegated to the local OpenSSH process. There is no first-use fingerprint UI, persisted trust store, mismatch workflow, credential-memory policy, multi-host CRUD, or connection generation guard.
- The application depends on user-installed OpenSSH and pre-existing aliases. It cannot provide an independent packaged runtime.
- The legacy forwarding recovery may run `fuser -k` or `sudo -n fuser -k` against the configured remote port. RemoteDeck may import this only as a disabled, explicitly authorized legacy hook and must never use it as generic cleanup.
- Configuration writes, schema validation, atomic persistence, secret redaction, diagnostics export, IPC isolation, CSP, and Electron sandboxing do not exist.
- There is no interactive general terminal, SFTP, LocalForward manager, health policy, proxy discovery, per-core/GPU-process telemetry, process identity guard, Codex lifecycle, onboarding, or migration UI.
- Several values are laboratory-specific. They may remain only in the preserved legacy fixture and migration tests; they are not valid defaults for RemoteDeck.

## Preservation and launch

The legacy files remain self-contained. From `legacy/labpulse-v0.1.0`:

```powershell
.\Start-LabPulseSSH.cmd
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\LabPulseSSH.ps1 -SelfTest
```

The self-test still requires the original SSH aliases and remote host. Archiving does not fabricate those external credentials or endpoints.
