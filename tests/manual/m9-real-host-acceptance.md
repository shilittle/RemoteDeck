# M9 user-authorized real-host acceptance

This checklist requires a Linux host and credentials controlled by the tester. Do not paste passwords, private-key passphrases, Codex tokens or signing material into logs/issues.

1. On the host console, record `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub -E sha256`. Add the host in RemoteDeck and confirm the first-use fingerprint exactly. Change the test sshd host key, verify hard failure, restore it, explicitly remove the stale trust record, and re-accept.
2. Connect once with each authorized method: password, keyboard-interactive, encrypted Ed25519 key and Windows OpenSSH agent. Deploy the same public key twice and verify one material entry in `~/.ssh/authorized_keys` plus a fresh key-only reconnect.
3. In separate terminal tabs run bash, vim, tmux and btop. Enter Chinese text, resize, search, copy/paste, send Ctrl+C, close one tab and verify the others remain usable.
4. Create a remote directory named `RemoteDeck 验收 空格`; upload/download an empty file, a Chinese filename and a 100 MiB file. Exercise rename/skip/overwrite, cancel during transfer and verify no RemoteDeck temporary file or followed symlink remains.
5. Send traffic through one LocalForward and one RemoteForward. Force the network down/up and one bind conflict; verify observable degraded/recovering/online states and that no unrelated listener/process is killed.
6. Start monitoring. Verify CPU/memory/disk/process data, graceful no-GPU behavior, collector restart after forced channel close, btop open/watchdog, and SIGTERM then separately confirmed SIGKILL only for an owned test process.
7. Run an L0 command, confirm an L1 command, and type the exact L2 phrase for a disposable high-risk test. Verify cancellation affects only its command job.
8. From the Codex panel, review the official installer command, complete device login in the SSH PTY, start/resume a normal session, detach/reattach the stable tmux session and run update. Inspect that no credential entered application logs/diagnostics.
9. Keep traffic flowing through a tunnel, close the window to tray, and verify traffic continues. Restore from tray, choose full quit, then verify the owned port and SSH connections are released.
10. Export diagnostics, inspect every ZIP entry, and confirm it contains no endpoint identifier, terminal/file content, password, passphrase, private key or Codex token.

Record OS versions, sshd version, commands used, observed fingerprints (public fingerprint only), artifact SHA-256 and pass/fail evidence. Redact hostnames/usernames if the report will be public.
