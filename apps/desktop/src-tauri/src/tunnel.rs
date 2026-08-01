use crate::{
    error::{AppError, AppResult},
    model::{
        HostProfile, TunnelDirection, TunnelHealthCheckKind, TunnelHealthState, TunnelLogEntry,
        TunnelLogLevel, TunnelProfile, TunnelRuntimeState, TunnelSnapshot,
    },
    ssh::SshRuntime,
};
use chrono::Utc;
use parking_lot::{Mutex, RwLock};
use std::{
    collections::{HashMap, VecDeque},
    ffi::OsString,
    io::{self, Read},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    path::Path,
    process::{Child, ChildStderr, ExitStatus, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};

const TUNNEL_EVENT: &str = "tunnel-event";
const STARTUP_GRACE: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const WAIT_SLICE: Duration = Duration::from_millis(100);
const STABLE_CONNECTION: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const MAX_LOG_ENTRIES: usize = 50;
const MAX_LOG_MESSAGE_BYTES: usize = 2_048;
const MAX_RUNTIME_LOG_BYTES: usize = 16 * 1024 * 1024;
const MAX_HEALTH_FAILURES: u8 = 3;
const CHILD_TERMINATION_TIMEOUT: Duration = Duration::from_secs(5);
const UPTIME_EVENT_INTERVAL: Duration = Duration::from_secs(1);
const MAX_ACTIVE_TUNNELS: usize = 64;
const MAX_RUNTIME_ENTRIES: usize = 256;
const MAX_LOG_BYTES_PER_TUNNEL: usize = MAX_RUNTIME_LOG_BYTES / MAX_RUNTIME_ENTRIES;
const LOG_ENTRY_OVERHEAD_BYTES: usize = 64;

#[derive(Clone, Default)]
pub struct TunnelRegistry {
    entries: Arc<RwLock<HashMap<String, Arc<TunnelEntry>>>>,
    operations: Arc<Mutex<()>>,
}

struct TunnelEntry {
    lifecycle: Mutex<()>,
    events: Mutex<()>,
    inner: Mutex<TunnelInner>,
}

struct TunnelInner {
    host_id: String,
    host_profile: Option<HostProfile>,
    profile: TunnelProfile,
    state: TunnelRuntimeState,
    health: TunnelHealthState,
    started_at: Option<Instant>,
    reconnect_count: u32,
    message: Option<String>,
    logs: VecDeque<TunnelLogEntry>,
    log_bytes: usize,
    last_process_error: Option<String>,
    desired_running: bool,
    worker_active: bool,
    retired: bool,
    generation: u64,
    revision: u64,
    child: Option<OwnedChild>,
}

impl TunnelEntry {
    fn new(profile: TunnelProfile) -> Self {
        Self {
            lifecycle: Mutex::new(()),
            events: Mutex::new(()),
            inner: Mutex::new(TunnelInner {
                host_id: profile.host_id.clone(),
                host_profile: None,
                profile,
                state: TunnelRuntimeState::Stopped,
                health: TunnelHealthState::Unknown,
                started_at: None,
                reconnect_count: 0,
                message: None,
                logs: VecDeque::new(),
                log_bytes: 0,
                last_process_error: None,
                desired_running: false,
                worker_active: false,
                retired: false,
                generation: 0,
                revision: 0,
                child: None,
            }),
        }
    }
}

impl TunnelRegistry {
    pub fn apply_host_update<T>(
        &self,
        previous: &HostProfile,
        next: &HostProfile,
        update: impl FnOnce() -> AppResult<T>,
    ) -> AppResult<T> {
        let _operation = self.operations.lock();
        if !same_connection_profile(previous, next)
            && self.entries.read().values().any(|entry| {
                let inner = entry.inner.lock();
                inner
                    .host_profile
                    .as_ref()
                    .is_some_and(|host| connection_uses_host(host, previous))
                    && (inner.desired_running || inner.worker_active || inner.child.is_some())
            })
        {
            return Err(AppError::State(
                "stop active tunnels before changing these SSH connection settings".to_owned(),
            ));
        }
        update()
    }

    pub fn start(
        &self,
        app: AppHandle,
        host: &HostProfile,
        tunnel: &TunnelProfile,
        runtime: &SshRuntime,
    ) -> AppResult<TunnelSnapshot> {
        let _operation = self.operations.lock();
        let host = runtime.current_host_profile(host)?;
        self.start_inner(app, &host, tunnel, runtime)
    }

    fn start_inner(
        &self,
        app: AppHandle,
        host: &HostProfile,
        tunnel: &TunnelProfile,
        runtime: &SshRuntime,
    ) -> AppResult<TunnelSnapshot> {
        if tunnel.host_id != host.id {
            return Err(AppError::Validation(
                "tunnel does not belong to the selected host".to_owned(),
            ));
        }
        runtime.tunnel_command(host, tunnel)?;
        let entry = {
            let mut entries = self.entries.write();
            entries.retain(|_, entry| {
                let inner = entry.inner.lock();
                !inner.retired
            });
            if !entries.contains_key(&tunnel.id) && entries.len() >= MAX_RUNTIME_ENTRIES {
                return Err(AppError::Validation(format!(
                    "at most {MAX_RUNTIME_ENTRIES} tunnel runtime records are retained"
                )));
            }
            let other_active = entries
                .iter()
                .filter(|(id, entry)| {
                    id.as_str() != tunnel.id && {
                        let inner = entry.inner.lock();
                        inner.desired_running || inner.worker_active || inner.child.is_some()
                    }
                })
                .count();
            if other_active >= MAX_ACTIVE_TUNNELS {
                return Err(AppError::Validation(format!(
                    "at most {MAX_ACTIVE_TUNNELS} tunnels may run at once"
                )));
            }
            entries
                .entry(tunnel.id.clone())
                .or_insert_with(|| Arc::new(TunnelEntry::new(tunnel.clone())))
                .clone()
        };
        let _lifecycle = entry.lifecycle.lock();
        {
            let inner = entry.inner.lock();
            if inner.retired {
                return Err(AppError::NotFound(format!(
                    "tunnel runtime {} was removed",
                    tunnel.id
                )));
            }
            if inner.desired_running
                && inner.profile == *tunnel
                && matches!(
                    inner.state,
                    TunnelRuntimeState::Starting
                        | TunnelRuntimeState::Running
                        | TunnelRuntimeState::Waiting
                )
            {
                return Ok(snapshot_locked(&inner));
            }
        }

        let stale_child = {
            let mut inner = entry.inner.lock();
            inner.generation = inner.generation.saturating_add(1);
            inner.child.take()
        };
        if let Some(mut child) = stale_child {
            child.terminate()?;
        }

        let (generation, starting) = {
            let mut inner = entry.inner.lock();
            inner.host_id = host.id.clone();
            inner.host_profile = Some(host.clone());
            inner.profile = tunnel.clone();
            inner.state = TunnelRuntimeState::Starting;
            inner.health = TunnelHealthState::Unknown;
            inner.started_at = None;
            inner.reconnect_count = 0;
            inner.message = None;
            inner.last_process_error = None;
            inner.desired_running = true;
            inner.worker_active = true;
            append_log_locked(&mut inner, TunnelLogLevel::Info, "Tunnel start requested");
            (inner.generation, pending_event_locked(&mut inner))
        };
        emit_pending(&app, &entry, &starting);
        if let Err(error) = spawn_worker(
            entry.clone(),
            app.clone(),
            host.clone(),
            tunnel.clone(),
            runtime.clone(),
            generation,
        ) {
            let failed = {
                let mut inner = entry.inner.lock();
                if inner.generation == generation {
                    inner.desired_running = false;
                    inner.worker_active = false;
                    inner.state = TunnelRuntimeState::Failed;
                    inner.health = TunnelHealthState::Failed;
                    inner.message = Some(format!("failed to start tunnel worker: {error}"));
                    append_log_locked(
                        &mut inner,
                        TunnelLogLevel::Error,
                        &format!("Failed to start tunnel worker: {error}"),
                    );
                }
                pending_event_locked(&mut inner)
            };
            emit_pending(&app, &entry, &failed);
            return Err(AppError::Io(error));
        }
        Ok(starting.snapshot)
    }

    pub fn restart(
        &self,
        app: AppHandle,
        host: &HostProfile,
        tunnel: &TunnelProfile,
        runtime: &SshRuntime,
    ) -> AppResult<TunnelSnapshot> {
        let _operation = self.operations.lock();
        self.stop_inner(&app, &tunnel.id, false)?;
        let host = runtime.current_host_profile(host)?;
        self.start_inner(app, &host, tunnel, runtime)
    }

    pub fn snapshot_for(&self, tunnel: &TunnelProfile) -> TunnelSnapshot {
        self.entries
            .read()
            .get(&tunnel.id)
            .and_then(|entry| {
                let inner = entry.inner.lock();
                if inner.retired {
                    return None;
                }
                let mut snapshot = snapshot_locked(&inner);
                if inner.profile != *tunnel {
                    snapshot.profile = Some(tunnel.clone());
                    snapshot.message.get_or_insert_with(|| {
                        "saved tunnel configuration changed; restart to apply it".to_owned()
                    });
                }
                Some(snapshot)
            })
            .unwrap_or_else(|| stopped_snapshot(tunnel.clone()))
    }

    pub fn stop(&self, app: &AppHandle, tunnel_id: &str) -> AppResult<()> {
        let _operation = self.operations.lock();
        self.stop_inner(app, tunnel_id, false)
    }

    fn stop_inner(&self, app: &AppHandle, tunnel_id: &str, retire: bool) -> AppResult<()> {
        let Some(entry) = self.entries.read().get(tunnel_id).cloned() else {
            return Ok(());
        };
        let _lifecycle = entry.lifecycle.lock();
        let (generation, mut child, already_stopped) = {
            let mut inner = entry.inner.lock();
            let already_stopped = !inner.desired_running
                && inner.child.is_none()
                && inner.state == TunnelRuntimeState::Stopped;
            inner.desired_running = false;
            inner.retired |= retire;
            inner.generation = inner.generation.saturating_add(1);
            inner.worker_active = false;
            (inner.generation, inner.child.take(), already_stopped)
        };
        let termination = if let Some(child) = child.as_mut() {
            child.terminate()
        } else {
            Ok(())
        };
        let pending = {
            let mut inner = entry.inner.lock();
            if inner.generation == generation && !inner.desired_running {
                inner.started_at = None;
                inner.health = TunnelHealthState::Unknown;
                if let Err(error) = &termination {
                    inner.child = child.take();
                    inner.state = TunnelRuntimeState::Failed;
                    inner.message = Some(format!("failed to stop OpenSSH tunnel: {error}"));
                    append_log_locked(
                        &mut inner,
                        TunnelLogLevel::Error,
                        &format!("Failed to stop OpenSSH tunnel: {error}"),
                    );
                } else {
                    inner.state = TunnelRuntimeState::Stopped;
                    inner.message = None;
                    if !already_stopped {
                        append_log_locked(&mut inner, TunnelLogLevel::Info, "Tunnel stopped");
                    }
                }
            }
            pending_event_locked(&mut inner)
        };
        if !already_stopped || termination.is_err() {
            emit_pending(app, &entry, &pending);
        }
        termination.map_err(AppError::from)
    }

    pub fn remove(&self, app: &AppHandle, tunnel_id: &str) -> AppResult<()> {
        let _operation = self.operations.lock();
        self.stop_inner(app, tunnel_id, true)?;
        self.entries.write().remove(tunnel_id);
        Ok(())
    }

    pub fn stop_for_host(&self, app: &AppHandle, host_id: &str) -> AppResult<()> {
        let _operation = self.operations.lock();
        let ids = self
            .entries
            .read()
            .iter()
            .filter(|(_, entry)| entry.inner.lock().host_id == host_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let mut errors = Vec::new();
        for id in ids {
            if let Err(error) = self.stop_inner(app, &id, true) {
                errors.push(format!("{id}: {error}"));
            } else {
                self.entries.write().remove(&id);
            }
        }
        stop_errors(errors)
    }

    pub fn stop_all(&self, app: &AppHandle) -> AppResult<()> {
        let _operation = self.operations.lock();
        let ids = self.entries.read().keys().cloned().collect::<Vec<_>>();
        let errors = thread::scope(|scope| {
            let handles = ids
                .into_iter()
                .map(|id| {
                    scope.spawn(move || {
                        self.stop_inner(app, &id, false)
                            .err()
                            .map(|error| format!("{id}: {error}"))
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok().flatten())
                .collect::<Vec<_>>()
        });
        stop_errors(errors)
    }
}

fn stop_errors(errors: Vec<String>) -> AppResult<()> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(AppError::Process(format!(
            "failed to stop one or more OpenSSH tunnels: {}",
            errors.join("; ")
        )))
    }
}

fn spawn_worker(
    entry: Arc<TunnelEntry>,
    app: AppHandle,
    host: HostProfile,
    tunnel: TunnelProfile,
    runtime: SshRuntime,
    generation: u64,
) -> io::Result<()> {
    thread::Builder::new()
        .name(format!("tunnel-{}", tunnel.id))
        .spawn(move || {
            run_worker(entry, app, host, tunnel, runtime, generation);
        })
        .map(|_| ())
}

fn run_worker(
    entry: Arc<TunnelEntry>,
    app: AppHandle,
    host: HostProfile,
    tunnel: TunnelProfile,
    runtime: SshRuntime,
    generation: u64,
) {
    let _worker_guard = WorkerGuard {
        entry: entry.clone(),
        app: app.clone(),
        generation,
    };
    let mut failure_streak = 0_u32;
    loop {
        if !mark_starting(&app, &entry, generation) {
            return;
        }
        let current_host = match runtime.current_host_profile(&host) {
            Ok(host) => host,
            Err(error) => {
                let Some(delay) = finish_attempt(
                    &app,
                    &entry,
                    generation,
                    &tunnel,
                    format!("could not resolve the current host profile: {error}"),
                    false,
                    &mut failure_streak,
                ) else {
                    return;
                };
                if !interruptible_wait(&entry, generation, delay) {
                    return;
                }
                continue;
            }
        };
        let (program, args) = match runtime.tunnel_command(&current_host, &tunnel) {
            Ok(specification) => specification,
            Err(error) => {
                let Some(delay) = finish_attempt(
                    &app,
                    &entry,
                    generation,
                    &tunnel,
                    format!("could not build OpenSSH tunnel command: {error}"),
                    false,
                    &mut failure_streak,
                ) else {
                    return;
                };
                if !interruptible_wait(&entry, generation, delay) {
                    return;
                }
                continue;
            }
        };
        let attempt_lifecycle = entry.lifecycle.lock();
        if !is_current(&entry, generation) {
            return;
        }
        let (child, stderr) = match spawn(&program, &args) {
            Ok(process) => process,
            Err(error) => {
                drop(attempt_lifecycle);
                let Some(delay) = finish_attempt(
                    &app,
                    &entry,
                    generation,
                    &tunnel,
                    format!("failed to spawn OpenSSH tunnel: {error}"),
                    false,
                    &mut failure_streak,
                ) else {
                    return;
                };
                if !interruptible_wait(&entry, generation, delay) {
                    return;
                }
                continue;
            }
        };
        let mut child = Some(child);
        let stored = {
            let mut inner = entry.inner.lock();
            if is_current_locked(&inner, generation) {
                inner.last_process_error = None;
                inner.child = child.take();
                true
            } else {
                false
            }
        };
        if !stored {
            if let Some(mut child) = child {
                let _ = child.terminate();
            }
            return;
        }
        drop(attempt_lifecycle);
        if let Some(stderr) = stderr {
            spawn_stderr_reader(stderr, entry.clone(), app.clone(), generation);
        }

        let attempt_started = Instant::now();
        let mut announced_running = false;
        let mut consecutive_health_failures = 0_u8;
        let mut next_health_check = Instant::now() + health_interval(&tunnel);
        let mut next_uptime_event = Instant::now() + UPTIME_EVENT_INTERVAL;
        let end = loop {
            match poll_child(&entry, generation) {
                ChildPoll::Cancelled => return,
                ChildPoll::Exited(status) => {
                    let detail = process_exit_message(&entry, generation, status);
                    break AttemptEnd {
                        message: detail,
                        clean: status.success(),
                    };
                }
                ChildPoll::Failed(error) => {
                    break AttemptEnd {
                        message: format!("failed to poll OpenSSH tunnel: {error}"),
                        clean: false,
                    };
                }
                ChildPoll::Running => {}
            }

            if !announced_running && attempt_started.elapsed() >= STARTUP_GRACE {
                announced_running =
                    mark_running(&app, &entry, generation, attempt_started, &tunnel);
                if !announced_running {
                    return;
                }
            }

            if announced_running
                && health_check_enabled(&tunnel)
                && Instant::now() >= next_health_check
            {
                let check = tunnel.health_check.as_ref().expect("checked above");
                let timeout = Duration::from_secs(check.timeout_seconds.clamp(1, 60));
                let (health_host, health_port) = health_target(&tunnel);
                let result = tcp_health_check(&health_host, health_port, timeout);
                next_health_check =
                    Instant::now() + Duration::from_secs(check.interval_seconds.clamp(1, 3600));
                match result {
                    Ok(()) => {
                        consecutive_health_failures = 0;
                        if !set_health(&app, &entry, generation, TunnelHealthState::Healthy, None) {
                            return;
                        }
                    }
                    Err(error) => {
                        consecutive_health_failures = consecutive_health_failures.saturating_add(1);
                        let failed = consecutive_health_failures >= MAX_HEALTH_FAILURES;
                        let health = if failed {
                            TunnelHealthState::Failed
                        } else {
                            TunnelHealthState::Degraded
                        };
                        let message = format!(
                            "TCP health check failed ({consecutive_health_failures}/{MAX_HEALTH_FAILURES}): {error}"
                        );
                        if !set_health(&app, &entry, generation, health, Some(message.clone())) {
                            return;
                        }
                        if failed {
                            let termination = terminate_generation_child(&entry, generation);
                            let message = match termination {
                                Ok(()) => message,
                                Err(error) => format!(
                                    "{message}; failed to terminate unhealthy OpenSSH child: {error}"
                                ),
                            };
                            break AttemptEnd {
                                message,
                                clean: false,
                            };
                        }
                    }
                }
            }
            if announced_running && Instant::now() >= next_uptime_event {
                if !emit_current(&app, &entry, generation) {
                    return;
                }
                next_uptime_event = Instant::now() + UPTIME_EVENT_INTERVAL;
            }
            thread::sleep(POLL_INTERVAL);
        };

        if attempt_started.elapsed() >= STABLE_CONNECTION {
            failure_streak = 0;
        }
        let Some(delay) = finish_attempt(
            &app,
            &entry,
            generation,
            &tunnel,
            end.message,
            end.clean,
            &mut failure_streak,
        ) else {
            return;
        };
        if !interruptible_wait(&entry, generation, delay) {
            return;
        }
    }
}

struct WorkerGuard {
    entry: Arc<TunnelEntry>,
    app: AppHandle,
    generation: u64,
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        let (pending, child) = {
            let mut inner = self.entry.inner.lock();
            if inner.generation != self.generation {
                return;
            }
            inner.worker_active = false;
            if !inner.desired_running {
                return;
            }
            inner.desired_running = false;
            inner.state = TunnelRuntimeState::Failed;
            inner.health = TunnelHealthState::Failed;
            inner.started_at = None;
            inner.message = Some("tunnel worker stopped unexpectedly".to_owned());
            append_log_locked(
                &mut inner,
                TunnelLogLevel::Error,
                "Tunnel worker stopped unexpectedly",
            );
            let child = inner.child.take();
            (pending_event_locked(&mut inner), child)
        };
        drop(child);
        emit_pending(&self.app, &self.entry, &pending);
    }
}

struct AttemptEnd {
    message: String,
    clean: bool,
}

fn mark_starting(app: &AppHandle, entry: &TunnelEntry, generation: u64) -> bool {
    let pending = {
        let mut inner = entry.inner.lock();
        if !is_current_locked(&inner, generation) {
            return false;
        }
        inner.state = TunnelRuntimeState::Starting;
        inner.health = TunnelHealthState::Unknown;
        inner.started_at = None;
        inner.message = None;
        append_log_locked(
            &mut inner,
            TunnelLogLevel::Info,
            "Starting OpenSSH tunnel process",
        );
        pending_event_locked(&mut inner)
    };
    emit_pending(app, entry, &pending);
    true
}

fn mark_running(
    app: &AppHandle,
    entry: &TunnelEntry,
    generation: u64,
    started_at: Instant,
    tunnel: &TunnelProfile,
) -> bool {
    let pending = {
        let mut inner = entry.inner.lock();
        if !is_current_locked(&inner, generation) {
            return false;
        }
        inner.state = TunnelRuntimeState::Running;
        let remote_health_unavailable = tunnel.direction == TunnelDirection::Remote
            && tunnel
                .health_check
                .as_ref()
                .is_some_and(|check| check.kind == TunnelHealthCheckKind::Tcp);
        inner.health = if health_check_enabled(tunnel) || remote_health_unavailable {
            TunnelHealthState::Unknown
        } else {
            TunnelHealthState::Healthy
        };
        inner.started_at = Some(started_at);
        inner.message = remote_health_unavailable.then(|| {
            "remote-forward TCP health cannot be verified from the local endpoint".to_owned()
        });
        append_log_locked(
            &mut inner,
            TunnelLogLevel::Info,
            "OpenSSH tunnel is running",
        );
        if remote_health_unavailable {
            append_log_locked(
                &mut inner,
                TunnelLogLevel::Warn,
                "Remote-forward TCP health check is unavailable locally; automatic health restarts are disabled",
            );
        }
        pending_event_locked(&mut inner)
    };
    emit_pending(app, entry, &pending);
    true
}

fn set_health(
    app: &AppHandle,
    entry: &TunnelEntry,
    generation: u64,
    health: TunnelHealthState,
    message: Option<String>,
) -> bool {
    let pending = {
        let mut inner = entry.inner.lock();
        if !is_current_locked(&inner, generation) {
            return false;
        }
        if inner.health == health && inner.message == message {
            return true;
        }
        inner.health = health;
        inner.message = message.clone();
        if let Some(message) = message {
            append_log_locked(&mut inner, TunnelLogLevel::Warn, &message);
        } else if health == TunnelHealthState::Healthy {
            append_log_locked(&mut inner, TunnelLogLevel::Info, "TCP health check passed");
        }
        pending_event_locked(&mut inner)
    };
    emit_pending(app, entry, &pending);
    true
}

fn emit_current(app: &AppHandle, entry: &TunnelEntry, generation: u64) -> bool {
    let pending = {
        let inner = entry.inner.lock();
        if !is_current_locked(&inner, generation) {
            return false;
        }
        current_event_locked(&inner)
    };
    emit_pending(app, entry, &pending);
    true
}

fn finish_attempt(
    app: &AppHandle,
    entry: &TunnelEntry,
    generation: u64,
    tunnel: &TunnelProfile,
    message: String,
    clean: bool,
    failure_streak: &mut u32,
) -> Option<Duration> {
    let (pending, retry, child) = {
        let mut inner = entry.inner.lock();
        if !is_current_locked(&inner, generation) {
            return None;
        }
        let child_still_running = inner.child.is_some();
        let child = if child_still_running {
            None
        } else {
            inner.child.take()
        };
        inner.started_at = None;
        inner.health = if clean {
            TunnelHealthState::Unknown
        } else {
            TunnelHealthState::Failed
        };
        if child_still_running {
            inner.desired_running = false;
            inner.state = TunnelRuntimeState::Failed;
            inner.message = Some(message.clone());
            append_log_locked(&mut inner, TunnelLogLevel::Error, &message);
            (pending_event_locked(&mut inner), None, None)
        } else if tunnel.auto_reconnect {
            let delay = backoff_delay(*failure_streak);
            *failure_streak = failure_streak.saturating_add(1);
            inner.reconnect_count = inner.reconnect_count.saturating_add(1);
            inner.state = TunnelRuntimeState::Waiting;
            inner.message = Some(format!(
                "{message}; reconnecting in {} seconds",
                delay.as_secs()
            ));
            append_log_locked(
                &mut inner,
                TunnelLogLevel::Warn,
                &format!("{message}; reconnecting in {} seconds", delay.as_secs()),
            );
            (pending_event_locked(&mut inner), Some(delay), child)
        } else {
            inner.desired_running = false;
            inner.state = if clean {
                TunnelRuntimeState::Stopped
            } else {
                TunnelRuntimeState::Failed
            };
            inner.message = Some(message.clone());
            append_log_locked(
                &mut inner,
                if clean {
                    TunnelLogLevel::Warn
                } else {
                    TunnelLogLevel::Error
                },
                &message,
            );
            (pending_event_locked(&mut inner), None, child)
        }
    };
    drop(child);
    emit_pending(app, entry, &pending);
    retry
}

enum ChildPoll {
    Running,
    Exited(ExitStatus),
    Failed(io::Error),
    Cancelled,
}

fn poll_child(entry: &TunnelEntry, generation: u64) -> ChildPoll {
    let mut inner = entry.inner.lock();
    if !is_current_locked(&inner, generation) {
        return ChildPoll::Cancelled;
    }
    let result = match inner.child.as_mut() {
        Some(child) => child.try_wait(),
        None => {
            return ChildPoll::Failed(io::Error::new(
                io::ErrorKind::NotFound,
                "owned OpenSSH child is unavailable",
            ));
        }
    };
    match result {
        Ok(Some(status)) => {
            let child = inner.child.take();
            drop(inner);
            drop(child);
            ChildPoll::Exited(status)
        }
        Ok(None) => ChildPoll::Running,
        Err(error) => {
            let child = inner.child.take();
            drop(inner);
            drop(child);
            ChildPoll::Failed(error)
        }
    }
}

fn terminate_generation_child(entry: &TunnelEntry, generation: u64) -> io::Result<()> {
    let child = {
        let mut inner = entry.inner.lock();
        if inner.generation != generation {
            return Ok(());
        }
        inner.child.take()
    };
    if let Some(mut child) = child {
        if let Err(error) = child.terminate() {
            let mut inner = entry.inner.lock();
            if inner.generation == generation && inner.child.is_none() {
                inner.child = Some(child);
            }
            return Err(error);
        }
    }
    Ok(())
}

fn process_exit_message(entry: &TunnelEntry, generation: u64, status: ExitStatus) -> String {
    thread::sleep(Duration::from_millis(20));
    let detail = {
        let inner = entry.inner.lock();
        (inner.generation == generation)
            .then(|| inner.last_process_error.clone())
            .flatten()
    };
    match detail {
        Some(detail) => format!("OpenSSH tunnel exited with {status}: {detail}"),
        None => format!("OpenSSH tunnel exited with {status}"),
    }
}

fn health_check_enabled(tunnel: &TunnelProfile) -> bool {
    tunnel.direction == TunnelDirection::Local
        && tunnel
            .health_check
            .as_ref()
            .is_some_and(|check| check.kind == TunnelHealthCheckKind::Tcp)
}

fn same_connection_profile(left: &HostProfile, right: &HostProfile) -> bool {
    left.id == right.id
        && left.hostname == right.hostname
        && left.port == right.port
        && left.username == right.username
        && left.auth_method == right.auth_method
        && left.identity_file == right.identity_file
        && left.proxy_jump == right.proxy_jump
        && left.advanced == right.advanced
}

fn connection_uses_host(connection: &HostProfile, profile: &HostProfile) -> bool {
    connection.id == profile.id
        || connection.proxy_jump.as_deref().is_some_and(|reference| {
            reference == profile.id || reference.eq_ignore_ascii_case(&profile.alias)
        })
}

fn health_interval(tunnel: &TunnelProfile) -> Duration {
    tunnel
        .health_check
        .as_ref()
        .map_or(Duration::from_secs(3600), |check| {
            Duration::from_secs(check.interval_seconds.clamp(1, 3600))
        })
}

fn health_target(tunnel: &TunnelProfile) -> (String, u16) {
    match tunnel.direction {
        TunnelDirection::Local => {
            let bind = tunnel.bind_address.trim();
            let host = match bind {
                "" | "*" | "0.0.0.0" => Ipv4Addr::LOCALHOST.to_string(),
                "::" | "[::]" => Ipv6Addr::LOCALHOST.to_string(),
                value => value.trim_matches(['[', ']']).to_owned(),
            };
            (host, tunnel.source_port)
        }
        TunnelDirection::Remote => (tunnel.target_host.clone(), tunnel.target_port),
    }
}

fn tcp_health_check(host: &str, port: u16, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let normalized = match host.to_ascii_lowercase().as_str() {
        "localhost" => IpAddr::V4(Ipv4Addr::LOCALHOST),
        _ => host.parse::<IpAddr>().map_err(|_| {
            format!("health-check bind address {host} must be an IP address or localhost")
        })?,
    };
    let addresses = vec![SocketAddr::new(normalized, port)];
    if addresses.is_empty() {
        return Err(format!("{host}:{port} did not resolve to an address"));
    }
    let address_count = addresses.len();
    let mut last_error = None;
    for (index, address) in addresses.into_iter().enumerate() {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let attempts_left = u32::try_from(address_count - index).unwrap_or(1);
        let attempt_timeout =
            (deadline.saturating_duration_since(now) / attempts_left).max(Duration::from_millis(1));
        match TcpStream::connect_timeout(&address, attempt_timeout) {
            Ok(stream) => {
                drop(stream);
                return Ok(());
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.map_or_else(
        || format!("connection to {host}:{port} timed out"),
        |error| format!("could not connect to {host}:{port}: {error}"),
    ))
}

fn interruptible_wait(entry: &TunnelEntry, generation: u64, duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    loop {
        if !is_current(entry, generation) {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        thread::sleep(remaining.min(WAIT_SLICE));
    }
}

fn is_current(entry: &TunnelEntry, generation: u64) -> bool {
    is_current_locked(&entry.inner.lock(), generation)
}

fn is_current_locked(inner: &TunnelInner, generation: u64) -> bool {
    inner.generation == generation && inner.desired_running
}

fn backoff_delay(failure_streak: u32) -> Duration {
    let exponent = failure_streak.min(5);
    Duration::from_secs((1_u64 << exponent).min(MAX_BACKOFF.as_secs()))
}

fn spawn(program: &Path, args: &[OsString]) -> AppResult<(OwnedChild, Option<ChildStderr>)> {
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    let stderr = child.stderr.take();
    Ok((OwnedChild(child), stderr))
}

struct OwnedChild(Child);

impl OwnedChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.0.try_wait()
    }

    fn terminate(&mut self) -> io::Result<()> {
        if self.0.try_wait()?.is_some() {
            return Ok(());
        }
        if let Err(error) = self.0.kill()
            && self.0.try_wait()?.is_none()
        {
            return Err(error);
        }
        self.wait_for_exit(CHILD_TERMINATION_TIMEOUT)
    }

    fn wait_for_exit(&mut self, duration: Duration) -> io::Result<()> {
        let deadline = Instant::now() + duration;
        loop {
            if self.0.try_wait()?.is_some() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "owned OpenSSH child did not exit after termination",
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.wait_for_exit(Duration::from_millis(250));
        }
    }
}

fn spawn_stderr_reader(
    stderr: ChildStderr,
    entry: Arc<TunnelEntry>,
    app: AppHandle,
    generation: u64,
) {
    let _ = thread::Builder::new()
        .name("tunnel-stderr".to_owned())
        .spawn(move || drain_stderr(stderr, &entry, Some(&app), generation));
}

fn drain_stderr<R: Read>(
    mut reader: R,
    entry: &TunnelEntry,
    app: Option<&AppHandle>,
    generation: u64,
) {
    let mut chunk = [0_u8; 4096];
    let mut line = Vec::new();
    let mut truncated = false;
    loop {
        let read = match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => return,
        };
        for byte in &chunk[..read] {
            if *byte == b'\n' {
                record_process_line(entry, app, generation, &line, truncated);
                line.clear();
                truncated = false;
            } else if line.len() < MAX_LOG_MESSAGE_BYTES {
                line.push(*byte);
            } else {
                truncated = true;
            }
        }
    }
    if !line.is_empty() || truncated {
        record_process_line(entry, app, generation, &line, truncated);
    }
}

fn record_process_line(
    entry: &TunnelEntry,
    app: Option<&AppHandle>,
    generation: u64,
    bytes: &[u8],
    truncated: bool,
) {
    let mut message = String::from_utf8_lossy(bytes)
        .trim_end_matches('\r')
        .trim()
        .to_owned();
    if message.is_empty() {
        return;
    }
    if truncated {
        message.push_str(" [truncated]");
    }
    let lower = message.to_ascii_lowercase();
    let level = if lower.contains("error")
        || lower.contains("failed")
        || lower.contains("denied")
        || lower.contains("timed out")
        || lower.contains("refused")
        || lower.contains("could not resolve")
        || lower.contains("host key")
        || lower.contains("connection closed")
    {
        TunnelLogLevel::Error
    } else if lower.contains("warning") || lower.contains("warn") {
        TunnelLogLevel::Warn
    } else {
        TunnelLogLevel::Info
    };
    let pending = {
        let mut inner = entry.inner.lock();
        if inner.generation != generation {
            return;
        }
        inner.last_process_error = Some(message.clone());
        append_log_locked(&mut inner, level, &message);
        pending_event_locked(&mut inner)
    };
    if let Some(app) = app {
        emit_pending(app, entry, &pending);
    }
}

fn append_log_locked(inner: &mut TunnelInner, level: TunnelLogLevel, message: &str) {
    let message = limit_message(message);
    if inner
        .logs
        .back()
        .is_some_and(|entry| entry.level == level && entry.message == message)
    {
        return;
    }
    inner.log_bytes = inner
        .log_bytes
        .saturating_add(message.len().saturating_add(LOG_ENTRY_OVERHEAD_BYTES));
    inner.logs.push_back(TunnelLogEntry {
        at: Utc::now(),
        level,
        message,
    });
    while inner.logs.len() > MAX_LOG_ENTRIES || inner.log_bytes > MAX_LOG_BYTES_PER_TUNNEL {
        let Some(removed) = inner.logs.pop_front() else {
            inner.log_bytes = 0;
            break;
        };
        inner.log_bytes = inner.log_bytes.saturating_sub(
            removed
                .message
                .len()
                .saturating_add(LOG_ENTRY_OVERHEAD_BYTES),
        );
    }
}

fn limit_message(message: &str) -> String {
    if message.len() <= MAX_LOG_MESSAGE_BYTES {
        return message.to_owned();
    }
    let mut end = MAX_LOG_MESSAGE_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} [truncated]", &message[..end])
}

struct PendingEvent {
    snapshot: TunnelSnapshot,
    revision: u64,
}

fn pending_event_locked(inner: &mut TunnelInner) -> PendingEvent {
    inner.revision = inner.revision.saturating_add(1);
    PendingEvent {
        snapshot: snapshot_locked(inner),
        revision: inner.revision,
    }
}

fn current_event_locked(inner: &TunnelInner) -> PendingEvent {
    PendingEvent {
        snapshot: TunnelSnapshot {
            tunnel_id: inner.profile.id.clone(),
            revision: inner.revision,
            profile: Some(inner.profile.clone()),
            state: inner.state,
            health: Some(inner.health),
            uptime_seconds: inner.started_at.map(|started| started.elapsed().as_secs()),
            reconnect_count: Some(inner.reconnect_count),
            message: inner.message.clone(),
            logs: None,
        },
        revision: inner.revision,
    }
}

fn emit_pending(app: &AppHandle, entry: &TunnelEntry, event: &PendingEvent) {
    let _events = entry.events.lock();
    if !event_is_current(entry, event) {
        return;
    }
    emit_raw(app, event.snapshot.clone());
}

fn event_is_current(entry: &TunnelEntry, event: &PendingEvent) -> bool {
    entry.inner.lock().revision == event.revision
}

fn snapshot_locked(inner: &TunnelInner) -> TunnelSnapshot {
    TunnelSnapshot {
        tunnel_id: inner.profile.id.clone(),
        revision: inner.revision,
        profile: Some(inner.profile.clone()),
        state: inner.state,
        health: Some(inner.health),
        uptime_seconds: inner.started_at.map(|started| started.elapsed().as_secs()),
        reconnect_count: Some(inner.reconnect_count),
        message: inner.message.clone(),
        logs: Some(inner.logs.iter().cloned().collect()),
    }
}

fn stopped_snapshot(profile: TunnelProfile) -> TunnelSnapshot {
    TunnelSnapshot {
        tunnel_id: profile.id.clone(),
        revision: 0,
        profile: Some(profile),
        state: TunnelRuntimeState::Stopped,
        health: Some(TunnelHealthState::Unknown),
        uptime_seconds: None,
        reconnect_count: Some(0),
        message: None,
        logs: Some(Vec::new()),
    }
}

fn emit_raw(app: &AppHandle, event: TunnelSnapshot) {
    let _ = app.emit(TUNNEL_EVENT, event);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{TunnelHealthCheck, TunnelHealthCheckKind};
    use chrono::Utc;
    use std::{io::Cursor, net::TcpListener};
    use uuid::Uuid;

    #[test]
    fn reconnect_backoff_is_exponential_and_capped() {
        assert_eq!(backoff_delay(0), Duration::from_secs(1));
        assert_eq!(backoff_delay(1), Duration::from_secs(2));
        assert_eq!(backoff_delay(4), Duration::from_secs(16));
        assert_eq!(backoff_delay(5), Duration::from_secs(30));
        assert_eq!(backoff_delay(100), Duration::from_secs(30));
    }

    #[test]
    fn tcp_health_check_connects_to_a_listening_endpoint() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind listener");
        let port = listener.local_addr().expect("listener address").port();
        assert!(tcp_health_check("127.0.0.1", port, Duration::from_secs(1)).is_ok());
        drop(listener);
        assert!(tcp_health_check("127.0.0.1", port, Duration::from_millis(200)).is_err());
        assert!(tcp_health_check("unbounded.example", port, Duration::from_secs(1)).is_err());
    }

    #[test]
    fn stderr_drain_is_bounded_and_preserves_recent_unicode_logs() {
        let entry = TunnelEntry::new(profile());
        {
            let mut inner = entry.inner.lock();
            inner.generation = 7;
        }
        let mut data = Vec::new();
        for index in 0..150 {
            data.extend_from_slice(format!("warning {index} 中文\n").as_bytes());
        }
        data.extend_from_slice(&vec![b'x'; MAX_LOG_MESSAGE_BYTES * 2]);
        data.push(b'\n');
        drain_stderr(Cursor::new(data), &entry, None, 7);
        let inner = entry.inner.lock();
        assert_eq!(inner.logs.len(), MAX_LOG_ENTRIES);
        assert!(inner.logs.iter().any(|log| log.message.contains("中文")));
        assert!(
            inner
                .logs
                .back()
                .expect("last log")
                .message
                .ends_with("[truncated]")
        );
        assert!(
            inner
                .logs
                .iter()
                .all(|log| log.message.len() <= MAX_LOG_MESSAGE_BYTES + 12)
        );
        assert!(inner.log_bytes <= MAX_LOG_BYTES_PER_TUNNEL);
    }

    #[test]
    fn stopped_snapshots_include_profile_and_runtime_metrics() {
        let profile = profile();
        let snapshot = stopped_snapshot(profile.clone());
        assert_eq!(snapshot.tunnel_id, profile.id);
        assert_eq!(snapshot.revision, 0);
        assert_eq!(snapshot.profile, Some(profile));
        assert_eq!(snapshot.state, TunnelRuntimeState::Stopped);
        assert_eq!(snapshot.health, Some(TunnelHealthState::Unknown));
        assert_eq!(snapshot.reconnect_count, Some(0));
        assert_eq!(snapshot.logs, Some(Vec::new()));
    }

    #[test]
    fn stale_events_and_retired_entries_cannot_resurrect_a_tunnel() {
        let profile = profile();
        let entry = Arc::new(TunnelEntry::new(profile.clone()));
        let stale = {
            let mut inner = entry.inner.lock();
            inner.state = TunnelRuntimeState::Running;
            pending_event_locked(&mut inner)
        };
        let current = {
            let mut inner = entry.inner.lock();
            inner.state = TunnelRuntimeState::Stopped;
            inner.retired = true;
            pending_event_locked(&mut inner)
        };
        assert!(!event_is_current(&entry, &stale));
        assert!(event_is_current(&entry, &current));

        let registry = TunnelRegistry::default();
        registry.entries.write().insert(profile.id.clone(), entry);
        let snapshot = registry.snapshot_for(&profile);
        assert_eq!(snapshot.state, TunnelRuntimeState::Stopped);
        assert_eq!(snapshot.reconnect_count, Some(0));
    }

    #[test]
    fn uptime_events_do_not_clone_retained_logs() {
        let entry = TunnelEntry::new(profile());
        let mut inner = entry.inner.lock();
        append_log_locked(&mut inner, TunnelLogLevel::Info, "connected");
        let summary = current_event_locked(&inner);
        assert_eq!(summary.snapshot.logs, None);
        assert_eq!(snapshot_locked(&inner).logs.expect("full logs").len(), 1);
    }

    #[test]
    fn remote_forward_health_does_not_restart_from_a_local_target_probe() {
        let mut profile = profile();
        profile.direction = TunnelDirection::Remote;
        assert!(!health_check_enabled(&profile));
        profile.direction = TunnelDirection::Local;
        assert!(health_check_enabled(&profile));
    }

    #[test]
    fn owned_child_termination_is_bounded_and_reaps_the_process() {
        #[cfg(windows)]
        let child = std::process::Command::new("cmd.exe")
            .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
            .spawn()
            .expect("spawn owned child");
        #[cfg(unix)]
        let child = std::process::Command::new("sh")
            .args(["-c", "sleep 30"])
            .spawn()
            .expect("spawn owned child");
        let mut child = OwnedChild(child);
        let started = Instant::now();
        child.terminate().expect("terminate owned child");
        assert!(started.elapsed() < CHILD_TERMINATION_TIMEOUT);
        assert!(child.try_wait().expect("poll terminated child").is_some());
    }

    fn profile() -> TunnelProfile {
        let now = Utc::now();
        TunnelProfile {
            schema_version: 2,
            id: Uuid::new_v4().to_string(),
            host_id: Uuid::new_v4().to_string(),
            name: "research".to_owned(),
            direction: TunnelDirection::Local,
            bind_address: "127.0.0.1".to_owned(),
            source_port: 22022,
            target_host: "127.0.0.1".to_owned(),
            target_port: 22,
            auto_start: false,
            auto_reconnect: true,
            health_check: Some(TunnelHealthCheck {
                kind: TunnelHealthCheckKind::Tcp,
                interval_seconds: 10,
                timeout_seconds: 1,
            }),
            created_at: now,
            updated_at: now,
        }
    }
}
