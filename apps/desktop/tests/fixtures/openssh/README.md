# Real OpenSSH integration fixture

This fixture exercises RemoteDeck's production `ssh`, `ssh-keyscan`, `sftp`, and forwarding command paths against three disposable Ubuntu containers:

- `direct`: a directly published SSH endpoint;
- `jump`: a separately published and separately trusted jump host;
- `target`: reachable only through the fixture network and the saved `jump` profile.

The Ubuntu 24.04 amd64 image is pinned by manifest digest. The image installs `openssh-server`, `openssh-client`, `python3`, `tmux`, and `btop`; package versions intentionally follow Ubuntu Noble security updates when the digest-pinned base is rebuilt. Every run generates a temporary client key outside the repository and each container generates a fresh Ed25519 host key on startup. No private key, host key, or `known_hosts` file is committed or copied into the user's SSH directory.

Run on a Linux host with Docker Compose v2 and the Tauri Linux build prerequisites:

```bash
pnpm test:integration:openssh
```

The Rust tests are feature-gated and ignored by default. The wrapper starts the containers, runs only `openssh_integration` tests serially, bounds failure logs, and tears down the Compose project and temporary key material. The normal Windows development machine does not need Docker or a local `sshd`; GitHub CI owns this external-system boundary.
