# Real OpenSSH integration fixture

This fixture exercises RemoteDeck's production `ssh`, `ssh-keyscan`, `sftp`, and forwarding command paths against three disposable Ubuntu containers:

- `direct`: a directly published SSH endpoint;
- `jump`: a separately published and separately trusted jump host;
- `target`: reachable only through the fixture network and the saved `jump` profile.

The Ubuntu 24.04 amd64 image is pinned by manifest digest. The image installs `openssh-server`, `openssh-client`, `python3`, `tmux`, and `btop`; package versions intentionally follow Ubuntu Noble security updates when the digest-pinned base is rebuilt. Every run generates a temporary client key outside the repository and each container generates a fresh Ed25519 host key on startup. No private key, host key, or `known_hosts` file is committed or copied into the user's SSH directory.

The fixture explicitly offers Curve25519 key exchange because the Windows inbox `ssh-keyscan` 9.5 advertises an unsupported sntrup method ([upstream issue](https://github.com/PowerShell/Win32-OpenSSH/issues/2140)). This setting applies only to the disposable test server. A scan failure against a production server still fails closed and may require updating the Windows OpenSSH toolchain; this fixture does not certify that upstream compatibility issue as fixed.

Run with Rust, Windows OpenSSH (or the Linux OpenSSH client), Node.js and Docker Compose:

```bash
pnpm test:integration:openssh
# Include the real browser workflow against the same isolated SSH hosts:
pnpm test:e2e:openssh
```

The Rust tests are ignored by default. The wrapper starts a uniquely named Compose project, runs `openssh_integration` tests serially, bounds failure logs, and removes only that project's containers and temporary key material. Docker is required for this acceptance gate; an unavailable daemon is a failed prerequisite, not a passing test.

If Docker Hub is unavailable, `REMOTEDECK_FIXTURE_BASE_IMAGE` can select a mirror, but the runner requires the exact pinned `@sha256` digest. For example, the same Ubuntu image is available as `public.ecr.aws/docker/library/ubuntu:24.04@sha256:52df9b1ee71626e0088f7d400d5c6b5f7bb916f8f0c82b474289a4ece6cf3faf`. No proxy or registry settings are changed by the runner.
