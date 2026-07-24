# Known limitations — v1.0.0

- Windows 10/11 x64 is the supported desktop target. macOS, Linux desktop and ARM64 packages are outside v1.
- The current release candidate is not Authenticode-signed because no code-signing certificate was supplied. SmartScreen may warn; verify SHA-256 before running.
- ProxyJump is intentionally limited to one level. More complex chains can be represented in external OpenSSH configuration but are not managed by the v1 profile editor.
- Closing a plain SSH PTY ends that remote shell. Use the Codex tmux workflow or your own tmux/screen session for persistence.
- Structured monitoring requires remote `python3`. GPU fields depend on available vendor tools; btop is optional. Missing capabilities degrade explicitly.
- SFTP recursive operations deliberately do not follow symbolic links and do not implement rsync-style delta transfer.
- Codex account login is always interactive and requires the user's external account authorization. RemoteDeck does not store, copy or diagnose Codex credentials.
- A portable EXE carries the application runtime but, by default, stores settings in the current Windows user's normal application-data directory.
- Automatic application update and formal Release publishing are not part of v1. The branch may open a Draft PR, but is never auto-merged or auto-released.
- Real-host fingerprint ownership, real passwords/key passphrases, account login and code signing cannot be automated without external user authority.
