# Interactive terminals

RemoteDeck terminal tabs are real SSH PTYs. The Electron main process opens `ssh2.shell()` channels and owns their lifetime; xterm in the sandboxed renderer receives validated UTF-8 events and sends bounded input/resize requests over fixed IPC channels. Terminal input and full output are never written to application logs or persisted in profile storage.

## Tab lifecycle

- A new tab opens on the selected online host and enters that host's default workspace. A custom workspace is applied by a shell-safe `cd -- ...` command; paths containing spaces, quotes, or Chinese characters are supported.
- Tabs can be created, selected by keyboard, renamed inline, closed, and reconnected. Closing a tab closes only its PTY channel, not sibling tabs or the underlying SSH connection.
- Resize events from xterm's fit addon call SSH `setWindow(rows, cols, height, width)` on the matching channel.
- Each reconnect increments a session generation so late bytes and close events from the old channel cannot corrupt the new one.
- An unexpected close marks the tab offline and retains the existing xterm scrollback. “Reconnect” always inserts a visible separator and creates a new shell; it never claims to restore the old process. Persistent workflows are delegated to tmux in M7.

The main process keeps only live channel references and terminal metadata. Reloading the renderer can recover tab metadata, but prior output is intentionally not recorded for replay.

## Input, text, search, and links

xterm uses the Unicode 11 width tables and its native composition textarea for CJK IME input. Ctrl+C is passed to the remote PTY. Ctrl+Shift+C copies the current selection; Ctrl+Shift+V and the paste button use fixed Electron clipboard operations, rather than exposing a generic Electron or OS bridge.

Search uses the xterm search addon and supports Enter/Shift+Enter traversal. HTTP(S) links are detected with the web-links addon and opened only through a validated main-process operation that rejects non-HTTP schemes. The renderer cannot open arbitrary local paths or commands.

## Automated acceptance

`tests/e2e/terminal.spec.ts` launches the production renderer in Electron against a real in-process SSH server and covers host-key acceptance, authentication, PTY output, keyboard input, CJK text insertion, clipboard paste, Ctrl+C, incremental search, multiple tabs, resize forwarding, inline rename, close, and new-shell reconnect.

The Docker OpenSSH job additionally runs an actual Linux shell and verifies:

- Bash command execution;
- headless Vim startup and exit;
- tmux server/session creation;
- btop availability;
- a working directory named `中文 路径`;
- `stty size` after a 120×40 resize;
- interrupting `sleep` with Ctrl+C.

Use the container commands in `docs/ssh.md`; the terminal checks are part of the same `test:integration:openssh` suite.
