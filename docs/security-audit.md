# RemoteDeck v1.0 security audit

Audit date: 2026-07-24. Scope: Electron process boundaries, IPC, SSH trust/authentication, local persistence, SFTP, tunnels, telemetry/process signals, command/Codex execution, migration, diagnostics, packaging and dependency inventory.

## Results

- Renderer: sandboxed, context-isolated, Node integration disabled, navigation/webviews/popups/permissions denied, restrictive production CSP. No renderer import of Node/Electron/ssh2 and no generic IPC bridge.
- IPC: every exposed method maps to a fixed `v1:*` channel; request and response Zod schemas run in preload and main; main verifies the exact BrowserWindow main frame.
- Packaged Electron: RunAsNode, NODE_OPTIONS, inspector CLI and file-protocol extra privileges disabled; cookie encryption and embedded ASAR integrity enabled; only the packaged ASAR may load.
- SSH: unknown host keys stop before authentication; changed keys hard-fail; jump and target are verified independently. Passwords, keyboard answers and passphrases are never persisted and are cleared after use.
- Files/resources: local paths originate from system pickers/drop handles; SFTP does not shell-quote paths; recursive deletion does not follow symlinks; job-owned temporary paths and channels bound cleanup.
- Execution: process signals revalidate user/PID/command; command risk is reclassified in main and cannot be lowered by imported legacy rules; legacy cleanup is disabled unless explicitly authorized; Codex uses fixed CLI operations and normal SSH PTYs.
- Logging/diagnostics: six structured categories are available; launch-segmented log files retain the newest 20. Recursive key/text redaction covers passwords, passphrases, authorization, private keys, URL credentials and OpenAI-style tokens. Diagnostics use anonymous summaries, bounded logs and a second whole-entry scan.
- Supply chain: lockfile installation is frozen; packaged Electron is taken from the exact local pnpm dependency; `pnpm audit --audit-level high` reported no known vulnerabilities at audit time. CI receives no application secrets.

## Findings closed during M9

1. Added bounded rolling log retention and expanded redaction tests for JSON, URLs, OpenAI tokens and passphrases.
2. Added one-click ZIP diagnostics with a native save boundary, data minimization, size bounds, redaction and final secret scan.
3. Removed the last production LabPulse port/name default from Clash tunnel creation.
4. Disabled Electron's `GrantFileProtocolExtraPrivileges` fuse and added packaged readback to the release checklist.
5. Added independent Windows portable and clean NSIS install/launch/uninstall gates.

## Residual user-controlled risks

Host fingerprints must be verified through a trusted channel. L1/L2 confirmations cannot make an arbitrary shell command safe. HTTP(S) terminal links can still lead to hostile websites. An unsigned EXE provides hashes but not publisher identity. These risks are visible and never silently bypassed.
