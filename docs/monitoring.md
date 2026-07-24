# Structured monitoring and btop

RemoteDeck monitoring has two deliberately separate paths: a versioned read-only collector supplies structured charts and process data, while btop remains an optional interactive terminal program. RemoteDeck never parses btop's ANSI screen.

## Collector transport and schema

`packages/remote-collector/collector.py` is packaged as an Electron resource. For each enabled online host, main opens an SSH exec channel running `python3 -u -`, streams the source through stdin, and does not install a remote file. The collector emits strict v1 JSONL with:

- total and per-core CPU, load averages, and optional temperature;
- memory and swap totals/usage;
- network totals and receive/send rates;
- meaningful mounted filesystems;
- uptime;
- PID, PPID, user, CPU, memory, state, elapsed seconds, and full command snapshots;
- NVIDIA GPU identity, utilization, memory, temperature and power when `nvidia-smi` is available;
- NVIDIA compute-process PID and GPU-memory usage when available.

The main process validates every line with Zod before emitting it. A line over 4 MiB, malformed JSON, a schema mismatch, a sample timeout, channel error, collector exit, or network loss invalidates that generation and schedules bounded exponential restart. Generation checks discard stale output. A missing `python3` produces a stable dependency-missing state and an installation suggestion without affecting terminal, SFTP, or tunnels.

History is held only in application memory for the configured retention period. Old history samples discard process arrays after the next sample, keeping only the newest actionable process snapshot. IPC down-samples very large histories to at most 3,600 points for chart rendering. No telemetry is sent outside the user's SSH connection.

## Dashboard and process safety

The monitoring workspace renders CPU, memory, and network history with Apache ECharts, plus current temperature/uptime, GPU, disk, and searchable/sortable process views. Missing sensors, GPU, disks, or btop are normal empty states.

Signals are restricted to the SSH user's processes. Before sending a signal, main requires the selected PID, user, and full command to match the newest validated collector snapshot, then queries `ps` again over SSH and compares user and command exactly. A mismatch aborts the action to reduce PID-reuse risk. SIGTERM is the first action; SIGKILL additionally requires a visible second confirmation and a matching SIGTERM attempt within 60 seconds. Signal commands contain only the validated numeric PID and fixed signal name. Delivery is audited without recording process command text.

## btop

The btop probe runs `command -v btop` and reads its version. “Open btop” creates a normal xterm terminal tab and runs `exec btop`, so all keyboard and resize behavior stays in the existing PTY implementation.

The optional watchdog owns a separate PTY channel. It discards output without parsing it, restarts after exit or network recovery, supports a user-selected rotation period, and closes only its owned channel on stop, suspend, or application exit. If btop is absent, structured monitoring remains online.

## Verification

Unit tests cover valid/no-GPU data, malformed JSON, stale callbacks, crash, timeout, network recovery, missing Python, owner/command revalidation, TERM-before-KILL, btop absence, and watchdog restart/cleanup. Electron E2E streams collector JSONL through an in-process SSH server and verifies the real dashboard. The Docker OpenSSH job runs the packaged Python source against Linux `/proc`, verifies no-GPU degradation, probes and starts btop, and safely terminates an owned test process. Local Docker and WSL were unavailable on the development workstation, so that Linux acceptance remains an explicit CI gate.
