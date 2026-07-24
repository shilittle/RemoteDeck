# Commands and Codex CLI

## Command presets

RemoteDeck stores user-defined command presets in the versioned profile database. A preset can be global or scoped to one host and contains a name, description, group, command, optional working directory, risk declaration, PTY/sudo requirements, L2 confirmation text, and sort order. The command workspace supports create, edit, copy, delete, grouping, and sorting; built-ins are immutable but can be copied before customization.

The built-in library includes system overview, NVIDIA GPU status when available, CPU/memory processes, disk and inode usage, current-user tasks, listeners, Git status, Slurm `squeue -u "$USER"` when detected, btop in a real PTY, and workstation reboot. Dependency-bound built-ins are hidden from execution when their remote executable is absent.

Non-PTY jobs use a dedicated SSH exec channel, stream bounded stdout/stderr to the task drawer, and can cancel only their own channel. PTY/sudo jobs open a normal RemoteDeck terminal so interactive prompts and full-screen programs remain genuine. Working directories and sudo shell arguments are POSIX-quoted by main; the command body remains the exact user-authored executable text shown during confirmation.

## Risk gates

Risk enforcement lives in main/core and cannot be bypassed by renderer calls:

- L0 is limited to recognized read-only commands and runs directly.
- L1 displays the full final command, target alias, and working directory and requires one explicit confirmation.
- L2 covers reboot/shutdown, disk/filesystem tools, recursive forced deletion, raw block copying, and destructive Git operations. It requires confirmation plus an exact typed host alias or preset-specific phrase.

The classifier can only raise the preset's declared risk. Unknown custom commands conservatively become L1, while file redirection and in-place editors are treated as mutations. Main reloads the preset and host and repeats analysis at execution time, so stale or fabricated renderer analysis cannot authorize a run.

This policy applies to preset buttons only. RemoteDeck cannot reliably intercept arbitrary bytes typed into a free SSH terminal, and the UI discloses that boundary.

## Codex CLI lifecycle

RemoteDeck integrates only the official stable Codex CLI as a terminal program. It does not create a custom chat UI, parse TUI output, or launch the app-server protocol.

The fixed probe checks `command -v codex`, `codex --version`, `codex login status`, `tmux`, the associated workspace/Git state, and the installed version's `--help` output. It never reads, copies, displays, or transmits `~/.codex/auth.json`, an API key, or an access token.

The official Linux standalone install/update fallback is shown verbatim before confirmation:

```sh
curl -fsSL https://chatgpt.com/codex/install.sh | sh
```

The source is `https://chatgpt.com/codex/install.sh`. Self-update uses `codex update` only when the installed release advertises it; otherwise the confirmed official installer is used. Both paths require the user to review and confirm the update impact.

Remote/headless login prefers `codex login --device-auth` when the installed release advertises that flag, otherwise it opens `codex login` in the remote PTY. Login state comes only from `codex login status` and its documented exit status. Session resume uses `codex resume --last` only when present in the remote `codex resume --help`; older supported versions fall back to the interactive resume picker. No sandbox-bypass flag is supplied.

Ordinary start and resume commands first enter the host-associated workspace. Persistent mode derives a stable `remotedeck-<12 hex>` tmux name from the host ID and workspace path, then uses `tmux new-session -A`; both “start persistent” and “reattach” resolve to that same owned name. This provides tmux-backed persistence only. RemoteDeck does not claim that a normal SSH PTY survives transport loss.

Official behavior references: [Codex CLI install](https://developers.openai.com/codex/cli/), [CLI command reference](https://developers.openai.com/codex/cli/reference/), and [headless authentication](https://developers.openai.com/codex/auth/).

## Verification

Unit fixtures cover Codex absent, installed/logged-out, and installed/logged-in states plus device auth, resume, update, tmux naming, and credential-boundary assertions. Electron E2E exercises preset CRUD and an online L2 confirmation against an in-process SSH server. The Docker OpenSSH image contains a test-only `codex` executable that models the external account boundary without using a real account; the integration job runs a real SSH command and both mocked login states.

Run `tests/manual/m7-codex.ps1` for a real Linux host acceptance pass. Real login remains an explicit operator action.
