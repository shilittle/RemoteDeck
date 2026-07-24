# SSH tunnels

RemoteDeck implements `LocalForward` and `RemoteForward` with `ssh2`; the tunnel screen is not a command generator. Every running tunnel owns a dedicated SSH connection so stopping, failing, or reconnecting one profile cannot terminate another tunnel, a terminal, or an SFTP session.

## Forwarding semantics

- `LocalForward` listens on the configured Windows bind address and source port, then opens an SSH `forwardOut` channel to the target host and port as seen by the SSH server.
- `RemoteForward` requests `forwardIn` on the SSH server. Each accepted channel is connected to the configured target as seen by the RemoteDeck desktop.
- Starting a local tunnel performs an operating-system bind preflight. A conflict is reported in the profile and retried with bounded exponential backoff; RemoteDeck never kills the owning process.
- Stop and shutdown close only the listener, remote bind, streams, timers, and dedicated connection owned by that profile. A remote bind is explicitly released with `unforwardIn` before its SSH connection closes.

The runtime reports state, TCP/HTTP health, uptime, reconnect count, next retry, last error, and the last 200 lifecycle log entries. A dedicated connection closing schedules retries with jitter around 1, 2, 4, 8, 16, then at most 30 seconds. Windows suspend releases owned resources and resume reconnects profiles that were active. Profiles marked auto-start are restored after application startup; password and key-passphrase secrets are deliberately not persisted, so secret-dependent profiles remain failed until the user starts them with credentials.

## Clash and Mihomo migration

The read-only Windows detector correlates listening loopback ports with process names matching Clash or Mihomo, then probes SOCKS5, HTTP CONNECT, and finally plain TCP. The UI displays the process, PID, address, port, detected protocol, confidence, and probe evidence. It never edits, stops, or restarts the proxy process and never silently chooses among candidates.

After the user explicitly selects a candidate, the LabPulse helper creates a draft equivalent to:

```text
RemoteForward 127.0.0.1:17890 -> 127.0.0.1:<selected Clash port>
```

The user must still review and save the draft. Multiple detected candidates remain a manual choice.

## Legacy cleanup boundary

Normal forwarding never invokes `fuser`, `kill`, or another cleanup command. A profile may contain an optional legacy cleanup hook only when the user enters the exact command and checks the high-risk authorization. It runs only after a remote-bind failure, is logged as an audited action, and is disabled by default. This compatibility escape hatch must not be used for new profiles.

## Verification

Unit tests exercise SOCKS5/HTTP/TCP detection, port conflicts, two isolated tunnels, forced dedicated-SSH disconnect recovery, remote-channel routing, and owned `unforwardIn` cleanup. The Docker OpenSSH job additionally sends real HTTP traffic through both directions, stops the local tunnel, and proves the remote tunnel remains operational. Manual suspend/resume and a real Windows Clash installation remain release acceptance items because they depend on the host operating system.
