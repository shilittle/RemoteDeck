# Tunnels

RemoteDeck 2 starts one system `ssh.exe` child per LocalForward or RemoteForward. It does not generate a command for users to run manually. The child uses the selected profile, app-owned strict host trust, batch authentication, `ExitOnForwardFailure=yes`, and isolated validated arguments.

The registry serializes start, stop, restart, and remove transitions. A worker generation prevents late output/exit activity from an earlier child reviving a newer lifecycle, while each emitted snapshot carries a monotonically increasing revision so the renderer drops stale out-of-order state. Stop/removal disables reconnect before terminating and waiting on the exact owned child, so concurrent starts cannot create invisible forwards.

Unexpected exits can retry with exponential backoff capped at 30 seconds. Before every attempt, including reconnect, the worker resolves the current saved host profile rather than replaying stale SSH fields. Changing connection-critical fields on an active tunnel's target or direct ProxyJump route is rejected until the affected tunnel is stopped.

Log messages are truncated to 2 KiB, retained logs are limited by both a 50-entry cap and a per-tunnel byte budget, and the registry has a 16 MiB aggregate budget. Periodic uptime snapshots omit the log array; the renderer preserves the last full bounded log set while still applying the newer revision. LocalForward TCP health checks probe the configured local listener. RemoteForward cannot be proven through a local listener and therefore uses process/liveness health without a misleading local TCP check; external reachability also depends on the server's `GatewayPorts` policy.

Stopping or deleting a tunnel affects no terminal, transfer, telemetry collector, command, or other tunnel. Host deletion stops and retires that host's tunnel entries before persisting their removal. Full application quit stops locally owned forwarding children in parallel with bounded per-child waits under the overall application shutdown deadline.
