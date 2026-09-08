# RemoteDeck user guide

## Install and start

RemoteDeck supports Windows 10/11 x64. Install the Windows OpenSSH Client and use a Linux host running OpenSSH Server. The installer starts a per-user local service and opens the default browser; the service binds only to `127.0.0.1`. Launching RemoteDeck again reuses the existing service and opens its page.

User data lives at `%APPDATA%\io.github.shilittle.remotedeck`. Uninstall removes the install directory, shortcuts, uninstall metadata and the RemoteDeck startup entry, while retaining host profiles, trust and migration data. Export anything needed before manually deleting the data directory for a full purge.

## First connection

1. In **Hosts**, add an alias, address, port, user and default workspace, or import explicit hosts from an OpenSSH config.
2. Hosts already trusted by local SSH reuse the matching address/port record from the current user's `.ssh/known_hosts`. Open **Workspace → Terminal** directly; no repeated scan is needed. Startup also prepares profiles imported by older versions.
3. Only when no existing trust is available, choose **Scan fingerprint** and verify the SHA-256 through an independent trusted channel before accepting it. Changed keys remain blocked until you verify the rotation and replace the old pin explicitly.
4. Enter passwords, keyboard-interactive answers or key passphrases in the real SSH/ConPTY session. RemoteDeck does not store them. An existing usable private key does not need regeneration or redeployment.

Background SFTP, tunnels, monitoring, commands and Agent probes require a usable private key or ssh-agent. For a password-only host, authenticate in the terminal and deploy a public key first.

## Workspaces

The Workspace always shows the selected host and opens on Terminal by default. Refreshing or reopening the browser restores state from the service.

- **Terminal** opens, switches, renames, searches, copies, resizes, accepts Unicode input and reconnects SSH sessions. The service owns each PTY, so closing the browser does not stop it. Reattachment replays up to 1 MiB of recent output and marks any truncation.
- **Files** browses, creates, renames and deletes remote items, and uploads/downloads files or directories. Conflict policies are ask, overwrite, skip and automatic rename. Overwrite writes a RemoteDeck-owned temporary result and preserves a backup for rollback; the task center reports progress, cancellation and retry.
- **Commands and Agents** reclassifies the final command immediately before execution. L0/L1/L2 actions receive the required confirmation. Codex, Claude, Gemini and OpenCode support probe, install/update, login, start and tmux resume; each provider retains ownership of account state and permissions.
- **Monitoring and Tunnels** shows CPU, memory, network, disks, GPUs, processes, btop and history, and controls LocalForward/RemoteForward health, reconnect and bounded logs. Missing remote tools are reported as degraded states.

## Hosts, ProxyJump and keys

Hosts can be searched, grouped and imported. ProxyJump selects one saved direct-connect host. Both endpoints reuse existing local trust; if absent, trust the jump first, then scan and independently verify the target through it. Nested chains, arbitrary ProxyCommand values, wildcard hosts and recursive Include files are not imported.

The key view discovers local private keys, generates Ed25519 keys and deploys public keys only after user confirmation to a trusted host. RemoteDeck always uses its own `known_hosts` and never edits the global OpenSSH files. Reuse supports exact addresses, non-default ports and hashed records, and never overwrites any existing app pin. Custom `UserKnownHostsFile`, `HostKeyAlias` and certificate trust are outside automatic reuse and need separate review.

## Tasks, settings and lifetime

**Tasks** aggregates transfers, commands, monitoring and tunnels across hosts. Long operations return an ID immediately; refresh cannot create a duplicate. Open details, cancel, retry or return to the owning workspace.

**Settings** controls terminal preferences, monitoring retention, default workspace, launch at login, migration, diagnostics and service shutdown. Stop Service rejects new work first and then cleans only children and remote watchdogs owned by this instance within a bound; it never kills arbitrary processes.

Diagnostics contain redacted state, capabilities and trusted fingerprints only. Passwords, keys, tokens, terminal content and remote command output are excluded. The service sends sequenced SSE state; a reconnect that detects a gap requests a fresh snapshot.

## Shortcuts

- `Ctrl+1`: Hosts
- `Ctrl+2`: Terminal
- `Ctrl+3`: Files
- `Ctrl+4`: Commands and Agents
- `Ctrl+5`: Monitoring and Tunnels
- `Ctrl+6`: Tasks
- `Ctrl+,`: Settings
- `Ctrl+Shift+T`: new terminal
