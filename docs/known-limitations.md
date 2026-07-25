# Known limitations — v1.0.1

- Windows 10/11 x64 is the supported desktop target. macOS, Linux desktop and ARM64 packages are outside v1.
- RemoteDeck v1.x Windows assets are intentionally not Authenticode-signed. SmartScreen may warn about an unknown publisher; download only from this repository's Releases page and verify the published SHA-256 before running.
- ProxyJump is intentionally limited to one level. More complex chains can be represented in external OpenSSH configuration but are not managed by the v1 profile editor.
- Closing a plain SSH PTY ends that remote shell. Use the Codex tmux workflow or your own tmux/screen session for persistence.
- Structured monitoring requires remote `python3`. GPU fields depend on available vendor tools; btop is optional. Missing capabilities degrade explicitly.
- SFTP recursive operations deliberately do not follow symbolic links and do not implement rsync-style delta transfer.
- Codex account login is always interactive and requires the user's external account authorization. RemoteDeck does not store, copy or diagnose Codex credentials.
- A portable EXE carries the application runtime but, by default, stores settings in the current Windows user's normal application-data directory.
- Automatic application update is not implemented in v1; download and install updates manually from GitHub Releases.
- Real-host fingerprint ownership, real passwords/key passphrases, and account login cannot be automated without external user authority.
