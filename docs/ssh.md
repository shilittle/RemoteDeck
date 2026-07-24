# SSH hosts, trust, and keys

RemoteDeck owns its versioned host database and interoperates with OpenSSH config without making the local `ssh.exe` a runtime dependency. The Electron main process owns every socket, private-key read, config write, and SFTP operation; the sandboxed renderer can call only fixed, schema-validated operations.

## Host profiles and connections

The host editor supports direct connections and one ProxyJump level. Target and jump-host secrets are separate inputs and remain in the current IPC/connection call only. Supported authentication methods are password, keyboard-interactive, an existing private key with optional passphrase, Windows OpenSSH agent, and Pageant. Passwords, answers, passphrases, and in-memory private-key buffers are cleared after each attempt and are never persisted or logged.

Connection diagnostics normalize DNS failure, TCP refusal/timeout, host-key failure, and authentication failure. A successful capability test opens a PTY, probes SFTP, checks `python3`, and verifies write access to the selected workspace.

## Host-key trust

The first handshake is intentionally stopped before authentication. RemoteDeck shows the exact SHA-256 fingerprint and algorithm and stores it only after explicit acceptance. A later mismatch is a hard failure showing both old and new fingerprints. It cannot be accepted in the mismatch dialog: the user must verify the server out of band and explicitly remove the old trust record first.

Trust records bind hostname and port to the exact public-key blob. OpenSSH config imports do not import trust implicitly.

## OpenSSH config interoperability

RemoteDeck parses `Host`, `HostName`, `User`, `Port`, `IdentityFile`, `IdentitiesOnly`, `ProxyJump`, `ServerAliveInterval`, `ServerAliveCountMax`, `TCPKeepAlive`, `ConnectTimeout`, `Compression`, `LocalForward`, and `RemoteForward`. Unsupported directives are retained with the import record and displayed as read-only raw lines.

The app writes only `~/.ssh/remotedeck.conf` (or the sibling of a custom config path) and adds one normalized `Include` line to the user's config. Existing comments and complex blocks are not rewritten. Writes use an exclusive temporary file plus rename; an existing user config receives a timestamped backup and is restored if validation fails. Repeating the same import or managed write is idempotent.

Imported `LocalForward` and `RemoteForward` entries become tunnel profiles for M5. Imported ProxyJump aliases are resolved only to a valid direct host; nested jumps are rejected.

## Private-key lifecycle

The native file picker scans only files selected by the user. RemoteDeck persists the path, public metadata, size, modification time, format, encryption flag, and fingerprint where available—never private-key bytes. New keys are Ed25519 in OpenSSH format, may be passphrase protected, use exclusive creation so an existing file cannot be overwritten, and receive a best-effort current-user-only Windows ACL.

Deployment uses the already authenticated SFTP channel. It creates `~/.ssh` with mode `0700`, normalizes and deduplicates `authorized_keys` by algorithm and key material, writes a temporary file, renames it atomically, and sets mode `0600`. No shell-quoted `echo` is used. RemoteDeck then opens a fresh private-key connection; only a successful verification can change the host's default authentication profile and managed OpenSSH config.

## Integration verification

`pnpm test:integration` runs real in-process SSH servers for password, keyboard-interactive, and two-hop ProxyJump flows with independent credentials and host keys.

The CI OpenSSH acceptance test uses the fixture in `tests/fixtures/openssh`:

```powershell
docker build -t remotedeck-openssh tests/fixtures/openssh
docker run --rm -d --name remotedeck-openssh -p 127.0.0.1:22222:22 remotedeck-openssh
$env:REMOTEDECK_OPENSSH_HOST = '127.0.0.1'
$env:REMOTEDECK_OPENSSH_PORT = '22222'
pnpm --filter @remotedeck/desktop test:integration:openssh
docker stop remotedeck-openssh
```

It verifies password first login, explicit fingerprint acceptance, encrypted Ed25519 generation, SFTP deployment, deduplication, a fresh key-authenticated connection, and switching the default authentication method. The fixture credentials are test-only and must never be reused outside the isolated container.
