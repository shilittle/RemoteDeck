# Commands and AI Agents

Definitions can be global or host-scoped, and built-ins are immutable. Rust resolves the final command, working directory, sudo wrapper, declared risk, and built-in metadata, then conservatively reclassifies it. Unknown commands are at least L1. L1 requires the target alias; L2 requires exact generated or preset confirmation text.

Non-PTY jobs have bounded concurrency, per-stream 1 MiB capture, timeouts, ordered events, cancellation, and owned-child cleanup. PTY-required jobs are handed to the terminal registry so passwords and interactive prompts never cross invoke arguments.

Provider-neutral plans support OpenAI Codex CLI, Claude Code, Gemini CLI, and OpenCode. Probes use official CLI commands with bounded output. Login/start/resume run in standard PTY/tmux sessions; provider approval and sandbox systems remain enabled. Install/update plans display their source/impact, require explicit target confirmation in Rust, run as the remote user, and reject known permission-bypass flags.

Codex uses the official standalone installer, `codex login`/`codex login status`, `codex update`, and `codex resume` behavior documented by OpenAI. Headless login remains inside the remote CLI; device-code or explicit SSH callback forwarding can be used when the provider supports it.
