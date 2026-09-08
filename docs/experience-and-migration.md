# Experience, lifecycle, and migration

The first-run guide routes into the same real host, trust, terminal, and migration workspaces used afterward. The simple WebUI exposes four primary areas: Hosts, Workspace, Tasks, and Settings. Workspace tabs cover terminal, files, commands/Agents, monitoring and tunnels. The task center projects live transfer, command, tunnel and telemetry state rather than running a parallel task engine.

Closing the browser leaves the local service and its owned work running. Launch at login uses a fixed HKCU Run value with the exact installed executable and no shell. A second launch discovers the existing runtime descriptor, verifies its PID and health API, and opens the existing browser URL. Stop Service marks shutdown before draining and initiates bounded cleanup of only this instance's owned resources.

Migration supports LabPulse SSH v0.1.0 and RemoteDeck v1 JSON. Preview is bounded and read-only, reports unsupported mappings, and computes a content hash. Apply rechecks the hash, resolves selected categories and aliases, validates the entire batch on a cloned state, persists once, and swaps memory only after success. Duplicate hashes are idempotent, disk/validation failures leave no partial imports, source files are unchanged, and old host trust is never imported.
