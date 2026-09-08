use crate::events::EventSink;
use crate::{
    error::{AppError, AppResult},
    model::{CommandResult, HostProfile},
    process::command,
    ssh::SshRuntime,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    io::{BufRead, BufReader, Read, Write},
    mem::size_of,
    process::{Child, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

const TELEMETRY_EVENT: &str = "telemetry-event";
const COLLECTOR: &str = include_str!("../../../packages/remote-collector/collector.py");
const MAX_ACTIVE_MONITORS: usize = 32;
const MAX_SAMPLE_LINE_BYTES: usize = 512 * 1024;
const MAX_HISTORY_SAMPLES_PER_HOST: usize = 3_600;
const MAX_HISTORY_SAMPLES_GLOBAL: usize = 8_192;
const MAX_HISTORY_BYTES_PER_HOST: usize = 16 * 1024 * 1024;
const MAX_HISTORY_BYTES_GLOBAL: usize = 64 * 1024 * 1024;
const MAX_DISKS: usize = 128;
const MAX_GPUS: usize = 32;
const MAX_PROCESSES: usize = 256;
const MAX_HOSTNAME_BYTES: usize = 255;
const MAX_USERNAME_BYTES: usize = 256;
const MAX_MOUNT_BYTES: usize = 1_024;
const MAX_GPU_NAME_BYTES: usize = 256;
const MAX_PROCESS_STATE_BYTES: usize = 32;
const MAX_PROCESS_COMMAND_BYTES: usize = 1_024;
const MAX_CAPTURED_AT_BYTES: usize = 64;
const MAX_RATE: f64 = 1.0e18;
const MAX_LOAD: f64 = 1.0e6;
const MAX_PROCESS_CPU_PERCENT: f64 = 1.0e6;
const MAX_CONCURRENT_BTOP_OPERATIONS: usize = 8;
const MAX_TERM_ATTEMPTS: usize = 512;
const TERM_ATTEMPT_TTL: Duration = Duration::from_secs(5 * 60);
const BTOP_COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const BTOP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const MONITOR_STOP_TIMEOUT: Duration = Duration::from_secs(8);
const MONITOR_CHILD_CLEANUP_TIMEOUT: Duration = Duration::from_secs(3);
const BTOP_OWNER_PREFIX: &str = "remotedeck-v2-btop:";
const BTOP_OWNER_OPTION: &str = "@remotedeck_owner";
const BTOP_ROTATION_OPTION: &str = "@remotedeck_rotation_minutes";
const BTOP_RESTART_OPTION: &str = "@remotedeck_restart_count";

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TelemetryRuntimeState {
    Stopped,
    Starting,
    Online,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryStatus {
    pub host_id: String,
    pub revision: u64,
    pub state: TelemetryRuntimeState,
    pub last_sample_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuSnapshot {
    pub percent: f64,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySnapshot {
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSnapshot {
    pub receive_bytes_per_second: f64,
    pub send_bytes_per_second: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskSnapshot {
    pub mount: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuSnapshot {
    pub index: u32,
    pub name: String,
    pub utilization_percent: f64,
    pub memory_used_mi_b: f64,
    pub memory_total_mi_b: f64,
    pub temperature_c: Option<f64>,
    pub power_w: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessSnapshot {
    pub pid: u32,
    pub start_ticks: u64,
    pub user: String,
    pub cpu_percent: f64,
    pub memory_percent: f64,
    pub state: String,
    pub elapsed_seconds: u64,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySnapshot {
    pub host_id: String,
    pub sampled_at: String,
    pub hostname: String,
    pub current_user: String,
    pub cpu: CpuSnapshot,
    pub memory: MemorySnapshot,
    pub network: NetworkSnapshot,
    pub disks: Vec<DiskSnapshot>,
    pub gpus: Vec<GpuSnapshot>,
    pub processes: Vec<ProcessSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryEvent {
    pub status: TelemetryStatus,
    pub sample: Option<TelemetrySnapshot>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BtopStatus {
    pub host_id: String,
    pub installed: bool,
    pub version: Option<String>,
    pub watchdog_state: String,
    pub rotation_minutes: u64,
    pub restart_count: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct TelemetryRegistry {
    monitors: Arc<RwLock<HashMap<String, MonitorHandle>>>,
    statuses: Arc<RwLock<HashMap<String, TelemetryStatus>>>,
    history: Arc<RwLock<HistoryStore>>,
    next_monitor_generation: Arc<AtomicU64>,
    watchdogs: Arc<RwLock<HashMap<String, OwnedWatchdog>>>,
    watchdog_operations: Arc<Mutex<HashMap<String, OwnedWatchdog>>>,
    watchdog_owner_nonce: Arc<str>,
    btop_limiter: Arc<Semaphore>,
    term_attempts: Arc<Mutex<HashMap<ProcessIdentity, Instant>>>,
    shutting_down: Arc<AtomicBool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProcessIdentity {
    host_id: String,
    pid: u32,
    start_ticks: u64,
    user_digest: [u8; 32],
    command_digest: [u8; 32],
}

pub struct ProcessSignalRequest<'a> {
    pub pid: u32,
    pub expected_start_ticks: u64,
    pub expected_user: &'a str,
    pub expected_command: &'a str,
    pub signal: &'a str,
}

#[derive(Clone)]
struct MonitorHandle {
    generation: u64,
    host: HostProfile,
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    completion: Arc<MonitorCompletion>,
    cleanup_error: Arc<Mutex<Option<String>>>,
}

#[derive(Default)]
struct MonitorCompletion {
    finished: AtomicBool,
    notify: Notify,
}

struct MonitorCompletionGuard(Arc<MonitorCompletion>);

impl MonitorCompletion {
    fn mark_finished(&self) {
        self.finished.store(true, Ordering::Release);
        self.notify.notify_waiters();
        self.notify.notify_one();
    }

    async fn wait(&self) {
        while !self.finished.load(Ordering::Acquire) {
            self.notify.notified().await;
        }
    }
}

impl Drop for MonitorCompletionGuard {
    fn drop(&mut self) {
        self.0.mark_finished();
    }
}

#[derive(Debug, Default)]
struct HistoryStore {
    hosts: HashMap<String, HostHistory>,
    total_accounted_bytes: usize,
    total_samples: usize,
    next_sequence: u64,
}

#[derive(Debug, Default)]
struct HostHistory {
    samples: VecDeque<StoredSample>,
    accounted_bytes: usize,
}

#[derive(Debug)]
struct StoredSample {
    sequence: u64,
    accounted_bytes: usize,
    sample: TelemetrySnapshot,
}

#[derive(Clone)]
struct OwnedWatchdog {
    host: HostProfile,
    runtime: SshRuntime,
    rotation_minutes: u64,
    owner_nonce: Arc<str>,
}

struct WatchdogOperationGuard {
    host_id: String,
    operations: Arc<Mutex<HashMap<String, OwnedWatchdog>>>,
}

impl Default for TelemetryRegistry {
    fn default() -> Self {
        Self::with_owner_nonce(Arc::from(Uuid::new_v4().simple().to_string()))
    }
}

impl TelemetryRegistry {
    pub fn with_owner_nonce(owner_nonce: Arc<str>) -> Self {
        debug_assert!(
            owner_nonce.len() == 32
                && owner_nonce
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
        );
        Self {
            monitors: Arc::default(),
            statuses: Arc::default(),
            history: Arc::default(),
            next_monitor_generation: Arc::default(),
            watchdogs: Arc::default(),
            watchdog_operations: Arc::default(),
            watchdog_owner_nonce: owner_nonce,
            btop_limiter: Arc::new(Semaphore::new(MAX_CONCURRENT_BTOP_OPERATIONS)),
            term_attempts: Arc::default(),
            shutting_down: Arc::default(),
        }
    }
}

impl Drop for WatchdogOperationGuard {
    fn drop(&mut self) {
        self.operations.lock().remove(&self.host_id);
    }
}

impl HistoryStore {
    fn samples(&self, host_id: &str) -> Vec<TelemetrySnapshot> {
        self.hosts
            .get(host_id)
            .map(|history| {
                history
                    .samples
                    .iter()
                    .map(|entry| entry.sample.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn record(&mut self, sample: TelemetrySnapshot, max_samples: usize) {
        let host_id = sample.host_id.clone();
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        let accounted_bytes = sample_accounted_bytes(&sample);
        let history = self.hosts.entry(host_id.clone()).or_default();
        history.samples.push_back(StoredSample {
            sequence,
            accounted_bytes,
            sample,
        });
        history.accounted_bytes = history.accounted_bytes.saturating_add(accounted_bytes);
        self.total_accounted_bytes = self.total_accounted_bytes.saturating_add(accounted_bytes);
        self.total_samples = self.total_samples.saturating_add(1);

        while self.hosts.get(&host_id).is_some_and(|history| {
            history.samples.len() > max_samples
                || history.accounted_bytes > MAX_HISTORY_BYTES_PER_HOST
        }) {
            self.pop_front(&host_id);
        }

        while self.total_samples > MAX_HISTORY_SAMPLES_GLOBAL
            || self.total_accounted_bytes > MAX_HISTORY_BYTES_GLOBAL
        {
            let Some(oldest_host) = self
                .hosts
                .iter()
                .filter_map(|(host_id, history)| {
                    history
                        .samples
                        .front()
                        .map(|sample| (host_id, sample.sequence))
                })
                .min_by_key(|(_, sequence)| *sequence)
                .map(|(host_id, _)| host_id.clone())
            else {
                break;
            };
            self.pop_front(&oldest_host);
        }
    }

    fn remove_host(&mut self, host_id: &str) {
        if let Some(history) = self.hosts.remove(host_id) {
            self.total_samples = self.total_samples.saturating_sub(history.samples.len());
            self.total_accounted_bytes = self
                .total_accounted_bytes
                .saturating_sub(history.accounted_bytes);
        }
    }

    fn pop_front(&mut self, host_id: &str) {
        let mut remove_host = false;
        if let Some(history) = self.hosts.get_mut(host_id) {
            if let Some(removed) = history.samples.pop_front() {
                history.accounted_bytes = history
                    .accounted_bytes
                    .saturating_sub(removed.accounted_bytes);
                self.total_accounted_bytes = self
                    .total_accounted_bytes
                    .saturating_sub(removed.accounted_bytes);
                self.total_samples = self.total_samples.saturating_sub(1);
            }
            remove_host = history.samples.is_empty();
        }
        if remove_host {
            self.hosts.remove(host_id);
        }
    }
}

fn sample_accounted_bytes(sample: &TelemetrySnapshot) -> usize {
    const ALLOCATION_OVERHEAD: usize = 32;

    fn string_bytes(value: &String) -> usize {
        value.capacity().saturating_add(ALLOCATION_OVERHEAD)
    }

    let mut bytes = size_of::<StoredSample>()
        .saturating_add(string_bytes(&sample.host_id))
        .saturating_add(string_bytes(&sample.sampled_at))
        .saturating_add(string_bytes(&sample.hostname))
        .saturating_add(string_bytes(&sample.current_user));
    bytes = bytes
        .saturating_add(
            sample
                .disks
                .capacity()
                .saturating_mul(size_of::<DiskSnapshot>()),
        )
        .saturating_add(
            sample
                .gpus
                .capacity()
                .saturating_mul(size_of::<GpuSnapshot>()),
        )
        .saturating_add(
            sample
                .processes
                .capacity()
                .saturating_mul(size_of::<ProcessSnapshot>()),
        )
        .saturating_add(ALLOCATION_OVERHEAD.saturating_mul(3));
    for disk in &sample.disks {
        bytes = bytes.saturating_add(string_bytes(&disk.mount));
    }
    for gpu in &sample.gpus {
        bytes = bytes.saturating_add(string_bytes(&gpu.name));
    }
    for process in &sample.processes {
        bytes = bytes
            .saturating_add(string_bytes(&process.user))
            .saturating_add(string_bytes(&process.state))
            .saturating_add(string_bytes(&process.command));
    }
    bytes
}

#[derive(Debug, Clone, Copy)]
pub struct TelemetryOptions {
    pub interval_seconds: u64,
    pub retention_minutes: u64,
    pub auto_reconnect: bool,
}

impl TelemetryRegistry {
    pub fn apply_host_update<T>(
        &self,
        previous: &HostProfile,
        next: &HostProfile,
        update: impl FnOnce() -> AppResult<T>,
    ) -> AppResult<T> {
        let operations = self.watchdog_operations.lock();
        let watchdogs = self.watchdogs.read();
        let monitors = self.monitors.read();
        if !same_connection_profile(previous, next)
            && (monitors
                .values()
                .any(|monitor| connection_uses_host(&monitor.host, previous))
                || watchdogs
                    .values()
                    .chain(operations.values())
                    .any(|watchdog| watchdog_uses_host(watchdog, previous)))
        {
            return Err(AppError::State(
                "stop active monitoring and affected btop watchdogs before changing these SSH connection settings"
                    .to_owned(),
            ));
        }
        update()
    }

    pub fn list(&self) -> Vec<TelemetryStatus> {
        self.statuses.read().values().cloned().collect()
    }

    pub fn history(&self, host_id: &str) -> Vec<TelemetrySnapshot> {
        self.history.read().samples(host_id)
    }

    pub fn start(
        &self,
        app: EventSink,
        host: HostProfile,
        runtime: SshRuntime,
        options: TelemetryOptions,
    ) -> AppResult<TelemetryStatus> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AppError::State(
                "telemetry is shutting down and cannot start new work".to_owned(),
            ));
        }
        if !(1..=60).contains(&options.interval_seconds)
            || !(1..=1440).contains(&options.retention_minutes)
        {
            return Err(AppError::Validation(
                "telemetry interval or retention is invalid".to_owned(),
            ));
        }
        let mut monitors = self.monitors.write();
        let host = runtime.current_host_profile(&host)?;
        if monitors.contains_key(&host.id) {
            return Err(AppError::Validation(format!(
                "telemetry for '{}' is already running",
                host.alias
            )));
        }
        if monitors.len() >= MAX_ACTIVE_MONITORS {
            return Err(AppError::Validation(format!(
                "at most {MAX_ACTIVE_MONITORS} telemetry collectors may run at once"
            )));
        }
        let generation = self
            .next_monitor_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let stop = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None));
        let completion = Arc::new(MonitorCompletion::default());
        let cleanup_error = Arc::new(Mutex::new(None));
        monitors.insert(
            host.id.clone(),
            MonitorHandle {
                generation,
                host: host.clone(),
                stop: stop.clone(),
                child: child.clone(),
                completion: completion.clone(),
                cleanup_error: cleanup_error.clone(),
            },
        );
        drop(monitors);
        let initial = self.set_status(
            &app,
            TelemetryStatus {
                host_id: host.id.clone(),
                revision: 0,
                state: TelemetryRuntimeState::Starting,
                last_sample_at: None,
                error: None,
            },
            None,
        );

        let registry = self.clone();
        let host_id = host.id.clone();
        let worker_app = app.clone();
        let worker = thread::Builder::new()
            .name(format!("telemetry-{}", &host_id[..host_id.len().min(12)]))
            .spawn(move || {
                let _completion = MonitorCompletionGuard(completion);
                let mut attempt = 0_u32;
                loop {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    let result = runtime
                        .current_host_profile(&host)
                        .and_then(|current_host| {
                            run_collector(CollectorContext {
                                app: &worker_app,
                                registry: &registry,
                                host: &current_host,
                                runtime: &runtime,
                                options,
                                generation,
                                stop: &stop,
                                child_slot: &child,
                                cleanup_error: &cleanup_error,
                            })
                        });
                    if child.lock().is_some() {
                        let previous = registry.status_for(&host.id);
                        registry.set_status(
                            &worker_app,
                            TelemetryStatus {
                                host_id: host.id.clone(),
                                revision: 0,
                                state: TelemetryRuntimeState::Failed,
                                last_sample_at: previous.and_then(|status| status.last_sample_at),
                                error: Some(
                                    "telemetry collector child could not be safely reaped"
                                        .to_owned(),
                                ),
                            },
                            None,
                        );
                        break;
                    }
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    let error = result.err().unwrap_or_else(|| {
                        AppError::Process("telemetry collector exited unexpectedly".to_owned())
                    });
                    attempt = attempt.saturating_add(1);
                    let will_retry = options.auto_reconnect && attempt <= 8;
                    let previous = registry.status_for(&host.id);
                    registry.set_status(
                        &worker_app,
                        TelemetryStatus {
                            host_id: host.id.clone(),
                            revision: 0,
                            state: if will_retry {
                                TelemetryRuntimeState::Degraded
                            } else {
                                TelemetryRuntimeState::Failed
                            },
                            last_sample_at: previous.and_then(|status| status.last_sample_at),
                            error: Some(error.to_string()),
                        },
                        None,
                    );
                    if !will_retry {
                        break;
                    }
                    let delay = 1_u64 << attempt.min(5);
                    for _ in 0..delay * 10 {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                }
                let child_cleanup_pending =
                    child.lock().is_some() || cleanup_error.lock().is_some();
                let mut monitors = registry.monitors.write();
                if monitors
                    .get(&host.id)
                    .is_some_and(|handle| handle.generation == generation)
                {
                    if !child_cleanup_pending {
                        monitors.remove(&host.id);
                    }
                } else {
                    return;
                }
                if stop.load(Ordering::Acquire) && !child_cleanup_pending {
                    let last_sample_at = registry
                        .status_for(&host.id)
                        .and_then(|status| status.last_sample_at);
                    registry.set_status(
                        &worker_app,
                        TelemetryStatus {
                            host_id: host.id.clone(),
                            revision: 0,
                            state: TelemetryRuntimeState::Stopped,
                            last_sample_at,
                            error: None,
                        },
                        None,
                    );
                }
            });
        if let Err(error) = worker {
            self.monitors.write().remove(&host_id);
            let failed = TelemetryStatus {
                host_id,
                revision: 0,
                state: TelemetryRuntimeState::Failed,
                last_sample_at: None,
                error: Some(format!("failed to create telemetry worker: {error}")),
            };
            self.set_status(&app, failed, None);
            return Err(error.into());
        }
        Ok(initial)
    }

    pub async fn stop(&self, events: EventSink, host_id: &str) -> AppResult<TelemetryStatus> {
        let handle = self.monitors.read().get(host_id).cloned();
        let Some(handle) = handle else {
            return Ok(self.set_status(
                &events,
                TelemetryStatus {
                    host_id: host_id.to_owned(),
                    revision: 0,
                    state: TelemetryRuntimeState::Stopped,
                    last_sample_at: self
                        .status_for(host_id)
                        .and_then(|status| status.last_sample_at),
                    error: None,
                },
                None,
            ));
        };
        handle.stop.store(true, Ordering::Release);
        let initial_kill_error = request_monitor_child_kill(&handle).err();
        tokio::time::timeout(MONITOR_STOP_TIMEOUT, handle.completion.wait())
            .await
            .map_err(|_| {
                AppError::Timeout(initial_kill_error.map_or_else(
                    || "timed out waiting for the telemetry worker to stop".to_owned(),
                    |error| {
                        format!(
                            "telemetry child termination failed ({error}); the worker did not stop before the deadline"
                        )
                    },
                ))
            })?;

        let cleanup_handle = handle.clone();
        tokio::task::spawn_blocking(move || cleanup_monitor_child(&cleanup_handle))
            .await
            .map_err(|error| {
                AppError::Process(format!("telemetry cleanup worker failed: {error}"))
            })??;
        if let Some(error) = handle.cleanup_error.lock().clone() {
            self.set_status(
                &events,
                TelemetryStatus {
                    host_id: host_id.to_owned(),
                    revision: 0,
                    state: TelemetryRuntimeState::Failed,
                    last_sample_at: self
                        .status_for(host_id)
                        .and_then(|status| status.last_sample_at),
                    error: Some(error.clone()),
                },
                None,
            );
            return Err(AppError::ProcessCleanup(error));
        }
        {
            let mut monitors = self.monitors.write();
            if monitors
                .get(host_id)
                .is_some_and(|current| current.generation == handle.generation)
            {
                monitors.remove(host_id);
            }
        }
        Ok(self.set_status(
            &events,
            TelemetryStatus {
                host_id: host_id.to_owned(),
                revision: 0,
                state: TelemetryRuntimeState::Stopped,
                last_sample_at: self
                    .status_for(host_id)
                    .and_then(|status| status.last_sample_at),
                error: None,
            },
            None,
        ))
    }

    async fn stop_all_monitors(&self, app: &EventSink) {
        let ids = self.monitors.read().keys().cloned().collect::<Vec<_>>();
        let mut stops = tokio::task::JoinSet::new();
        for id in ids {
            let registry = self.clone();
            let app = app.clone();
            stops.spawn(async move { registry.stop(app, &id).await });
        }
        while stops.join_next().await.is_some() {}
    }

    fn status_for(&self, host_id: &str) -> Option<TelemetryStatus> {
        self.statuses.read().get(host_id).cloned()
    }

    fn set_status(
        &self,
        app: &EventSink,
        mut status: TelemetryStatus,
        sample: Option<TelemetrySnapshot>,
    ) -> TelemetryStatus {
        let mut statuses = self.statuses.write();
        status.revision = statuses
            .get(&status.host_id)
            .map_or(1, |previous| previous.revision.saturating_add(1));
        statuses.insert(status.host_id.clone(), status.clone());
        drop(statuses);
        let _ = app.emit(
            TELEMETRY_EVENT,
            TelemetryEvent {
                status: status.clone(),
                sample,
            },
        );
        status
    }

    fn record_sample(
        &self,
        app: &EventSink,
        sample: TelemetrySnapshot,
        options: TelemetryOptions,
        generation: u64,
        stop: &AtomicBool,
    ) {
        let monitors = self.monitors.read();
        if stop.load(Ordering::Acquire)
            || monitors
                .get(&sample.host_id)
                .is_none_or(|handle| handle.generation != generation)
        {
            return;
        }
        let max_samples = usize::try_from(
            options.retention_minutes.saturating_mul(60) / options.interval_seconds.max(1),
        )
        .unwrap_or(MAX_HISTORY_SAMPLES_PER_HOST)
        .clamp(1, MAX_HISTORY_SAMPLES_PER_HOST);
        self.history.write().record(sample.clone(), max_samples);
        self.set_status(
            app,
            TelemetryStatus {
                host_id: sample.host_id.clone(),
                revision: 0,
                state: TelemetryRuntimeState::Online,
                last_sample_at: Some(sample.sampled_at.clone()),
                error: None,
            },
            Some(sample),
        );
        drop(monitors);
    }
}

fn request_monitor_child_kill(handle: &MonitorHandle) -> AppResult<()> {
    let mut child_slot = handle.child.lock();
    let Some(child) = child_slot.as_mut() else {
        return Ok(());
    };
    match child.try_wait() {
        Ok(Some(_)) => Ok(()),
        Ok(None) => child.kill().map_err(|error| {
            AppError::Process(format!("failed to terminate telemetry collector: {error}"))
        }),
        Err(error) => Err(error.into()),
    }
}

fn cleanup_monitor_child(handle: &MonitorHandle) -> AppResult<()> {
    let mut kill_error = None;
    {
        let mut child_slot = handle.child.lock();
        let Some(child) = child_slot.as_mut() else {
            return Ok(());
        };
        match child.try_wait() {
            Ok(Some(_)) => {
                child_slot.take();
                return Ok(());
            }
            Ok(None) => {
                if let Err(error) = child.kill() {
                    kill_error = Some(error.to_string());
                }
            }
            Err(error) => return Err(error.into()),
        }
    }

    let deadline = Instant::now() + MONITOR_CHILD_CLEANUP_TIMEOUT;
    loop {
        {
            let mut child_slot = handle.child.lock();
            let Some(child) = child_slot.as_mut() else {
                return Ok(());
            };
            match child.try_wait() {
                Ok(Some(_)) => {
                    child_slot.take();
                    return Ok(());
                }
                Ok(None) => {}
                Err(error) => return Err(error.into()),
            }
        }
        if Instant::now() >= deadline {
            return Err(AppError::Timeout(kill_error.map_or_else(
                || "telemetry collector did not exit before the cleanup deadline".to_owned(),
                |error| {
                    format!(
                        "telemetry collector termination failed ({error}) and it remained active"
                    )
                },
            )));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

struct CollectorContext<'a> {
    app: &'a EventSink,
    registry: &'a TelemetryRegistry,
    host: &'a HostProfile,
    runtime: &'a SshRuntime,
    options: TelemetryOptions,
    generation: u64,
    stop: &'a AtomicBool,
    child_slot: &'a Mutex<Option<Child>>,
    cleanup_error: &'a Mutex<Option<String>>,
}

fn run_collector(context: CollectorContext<'_>) -> AppResult<()> {
    let CollectorContext {
        app,
        registry,
        host,
        runtime,
        options,
        generation,
        stop,
        child_slot,
        cleanup_error,
    } = context;
    let remote_command = format!("python3 -u - --interval {}", options.interval_seconds);
    let (program, args) = runtime.remote_command_spec(host, remote_command, true)?;
    let mut child = command(program);
    child
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = child.spawn()?;
    let Some(mut stdin) = child.stdin.take() else {
        terminate_child(&mut child);
        return Err(AppError::Process(
            "telemetry stdin is unavailable".to_owned(),
        ));
    };
    let Some(stdout) = child.stdout.take() else {
        terminate_child(&mut child);
        return Err(AppError::Process(
            "telemetry stdout is unavailable".to_owned(),
        ));
    };
    let Some(stderr) = child.stderr.take() else {
        terminate_child(&mut child);
        return Err(AppError::Process(
            "telemetry stderr is unavailable".to_owned(),
        ));
    };
    *child_slot.lock() = Some(child);
    if stop.load(Ordering::Acquire)
        && let Some(child) = child_slot.lock().as_mut()
    {
        let _ = child.kill();
    }
    if let Err(error) = stdin.write_all(COLLECTOR.as_bytes()) {
        if let Some(mut child) = child_slot.lock().take() {
            terminate_child(&mut child);
        }
        return Err(error.into());
    }
    drop(stdin);
    let stderr_text = Arc::new(Mutex::new(String::new()));
    let stderr_capture = stderr_text.clone();
    let (stderr_done_tx, stderr_done_rx) = mpsc::sync_channel(1);
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.take(64 * 1024).read_to_end(&mut bytes);
        *stderr_capture.lock() = String::from_utf8_lossy(&bytes).trim().to_owned();
        let _ = stderr_done_tx.send(());
    });
    if stop.load(Ordering::Acquire)
        && let Some(child) = child_slot.lock().as_mut()
    {
        let _ = child.kill();
    }
    let mut reader = BufReader::new(stdout);
    let stream_result: AppResult<()> = loop {
        if stop.load(Ordering::Acquire) {
            break Ok(());
        }
        let line = match read_bounded_line(&mut reader) {
            Ok(Some(line)) => line,
            Ok(None) => break Ok(()),
            Err(error) => break Err(error),
        };
        let sample = match parse_sample(&host.id, &line) {
            Ok(sample) => sample,
            Err(error) => break Err(error),
        };
        registry.record_sample(app, sample, options, generation, stop);
    };
    let mut child = child_slot
        .lock()
        .take()
        .ok_or_else(|| AppError::State("telemetry child disappeared".to_owned()))?;
    if stop.load(Ordering::Acquire) || stream_result.is_err() {
        match child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                if let Err(error) = child.kill() {
                    *child_slot.lock() = Some(child);
                    return Err(AppError::Process(format!(
                        "failed to terminate telemetry collector: {error}"
                    )));
                }
            }
            Err(error) => {
                *child_slot.lock() = Some(child);
                return Err(error.into());
            }
        }
    }
    let status = match wait_child_bounded(&mut child, Duration::from_secs(5)) {
        Ok(status) => status,
        Err(error) => {
            *child_slot.lock() = Some(child);
            return Err(error.into());
        }
    };
    if stderr_done_rx.recv_timeout(Duration::from_secs(2)).is_err() {
        let error = "telemetry stderr pipe did not close before the cleanup deadline".to_owned();
        *cleanup_error.lock() = Some(error.clone());
        return Err(AppError::ProcessCleanup(error));
    }
    if stderr_reader.join().is_err() {
        return Err(AppError::Process(
            "telemetry stderr reader panicked".to_owned(),
        ));
    }
    stream_result?;
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let stderr = stderr_text.lock().clone();
    Err(AppError::Process(if stderr.is_empty() {
        format!("telemetry collector exited with {status}")
    } else {
        stderr
    }))
}

fn read_bounded_line(reader: &mut impl BufRead) -> AppResult<Option<String>> {
    let mut bytes = Vec::with_capacity(8 * 1024);
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            break;
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            if bytes.len().saturating_add(newline) > MAX_SAMPLE_LINE_BYTES {
                return Err(AppError::Process(format!(
                    "telemetry sample exceeds {MAX_SAMPLE_LINE_BYTES} bytes"
                )));
            }
            bytes.extend_from_slice(&available[..newline]);
            reader.consume(newline + 1);
            break;
        }
        if bytes.len().saturating_add(available.len()) > MAX_SAMPLE_LINE_BYTES {
            return Err(AppError::Process(format!(
                "telemetry sample exceeds {MAX_SAMPLE_LINE_BYTES} bytes"
            )));
        }
        let consumed = available.len();
        bytes.extend_from_slice(available);
        reader.consume(consumed);
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| AppError::Process("telemetry sample is not valid UTF-8".to_owned()))
}

fn wait_child_bounded(
    child: &mut Child,
    duration: Duration,
) -> std::io::Result<std::process::ExitStatus> {
    let deadline = Instant::now() + duration;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            child.kill()?;
            return wait_child_after_kill(child, Duration::from_secs(2));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_child_after_kill(
    child: &mut Child,
    duration: Duration,
) -> std::io::Result<std::process::ExitStatus> {
    let deadline = Instant::now() + duration;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "child did not exit before the reap deadline",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = wait_child_bounded(child, Duration::from_secs(2));
}

fn parse_sample(host_id: &str, line: &str) -> AppResult<TelemetrySnapshot> {
    if host_id.is_empty() || host_id.len() > 128 || host_id.chars().any(char::is_control) {
        return Err(AppError::State(
            "telemetry host identity is invalid".to_owned(),
        ));
    }
    let value: Value = serde_json::from_str(line)?;
    let captured_at = bounded_text(&value, &["capturedAt"], MAX_CAPTURED_AT_BYTES)?;
    chrono::DateTime::parse_from_rfc3339(&captured_at)
        .map_err(|_| AppError::Process("telemetry capturedAt is invalid".to_owned()))?;
    let load = value
        .pointer("/cpu/loadAverage")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Process("telemetry loadAverage is invalid".to_owned()))?;
    if load.len() != 3 {
        return Err(AppError::Process(
            "telemetry loadAverage must contain exactly three values".to_owned(),
        ));
    }
    let disks = bounded_array(&value, "disks", MAX_DISKS)?
        .iter()
        .map(|disk| {
            let used_bytes = unsigned(disk, &["usedBytes"])?;
            let total_bytes = unsigned(disk, &["totalBytes"])?;
            let available_bytes = unsigned(disk, &["availableBytes"])?;
            if used_bytes > total_bytes || available_bytes > total_bytes {
                return Err(AppError::Process(
                    "telemetry disk capacity values are inconsistent".to_owned(),
                ));
            }
            Ok(DiskSnapshot {
                mount: bounded_text(disk, &["mount"], MAX_MOUNT_BYTES)?,
                used_bytes,
                total_bytes,
                available_bytes,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let gpus = bounded_array(&value, "gpus", MAX_GPUS)?
        .iter()
        .map(|gpu| {
            let memory_used_mi_b = bounded_number(gpu, &["memoryUsedMiB"], 0.0, 1.0e12)?;
            let memory_total_mi_b = bounded_number(gpu, &["memoryTotalMiB"], 0.0, 1.0e12)?;
            if memory_used_mi_b > memory_total_mi_b {
                return Err(AppError::Process(
                    "telemetry GPU memory values are inconsistent".to_owned(),
                ));
            }
            Ok(GpuSnapshot {
                index: u32::try_from(unsigned(gpu, &["index"])?)
                    .map_err(|_| AppError::Process("GPU index is invalid".to_owned()))?,
                name: bounded_text(gpu, &["name"], MAX_GPU_NAME_BYTES)?,
                utilization_percent: bounded_number(gpu, &["utilizationPercent"], 0.0, 100.0)?,
                memory_used_mi_b,
                memory_total_mi_b,
                temperature_c: optional_bounded_number(gpu, &["temperatureC"], -273.15, 10_000.0)?,
                power_w: optional_bounded_number(gpu, &["powerW"], 0.0, 1.0e9)?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let processes = bounded_array(&value, "processes", MAX_PROCESSES)?
        .iter()
        .map(|process| {
            let elapsed = bounded_text(process, &["elapsed"], 32)?
                .parse::<u64>()
                .map_err(|_| AppError::Process("process elapsed time is invalid".to_owned()))?;
            Ok(ProcessSnapshot {
                pid: u32::try_from(unsigned(process, &["pid"])?)
                    .map_err(|_| AppError::Process("process PID is invalid".to_owned()))?,
                start_ticks: unsigned(process, &["startTicks"])?,
                user: bounded_text(process, &["user"], MAX_USERNAME_BYTES)?,
                cpu_percent: bounded_number(
                    process,
                    &["cpuPercent"],
                    0.0,
                    MAX_PROCESS_CPU_PERCENT,
                )?,
                memory_percent: bounded_number(process, &["memoryPercent"], 0.0, 100.0)?,
                state: bounded_text(process, &["state"], MAX_PROCESS_STATE_BYTES)?,
                elapsed_seconds: elapsed,
                command: bounded_text(process, &["command"], MAX_PROCESS_COMMAND_BYTES)?,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let memory = MemorySnapshot {
        used_bytes: unsigned(&value, &["memory", "usedBytes"])?,
        total_bytes: unsigned(&value, &["memory", "totalBytes"])?,
        swap_used_bytes: unsigned(&value, &["memory", "swapUsedBytes"])?,
        swap_total_bytes: unsigned(&value, &["memory", "swapTotalBytes"])?,
    };
    if memory.used_bytes > memory.total_bytes || memory.swap_used_bytes > memory.swap_total_bytes {
        return Err(AppError::Process(
            "telemetry memory capacity values are inconsistent".to_owned(),
        ));
    }
    Ok(TelemetrySnapshot {
        host_id: host_id.to_owned(),
        sampled_at: captured_at,
        hostname: bounded_text(&value, &["hostname"], MAX_HOSTNAME_BYTES)?,
        current_user: bounded_text(&value, &["currentUser"], MAX_USERNAME_BYTES)?,
        cpu: CpuSnapshot {
            percent: bounded_number(&value, &["cpu", "totalPercent"], 0.0, 100.0)?,
            load1: bounded_value_number(&load[0], "cpu.loadAverage[0]", 0.0, MAX_LOAD)?,
            load5: bounded_value_number(&load[1], "cpu.loadAverage[1]", 0.0, MAX_LOAD)?,
            load15: bounded_value_number(&load[2], "cpu.loadAverage[2]", 0.0, MAX_LOAD)?,
        },
        memory,
        network: NetworkSnapshot {
            receive_bytes_per_second: bounded_number(
                &value,
                &["network", "receiveBytesPerSecond"],
                0.0,
                MAX_RATE,
            )?,
            send_bytes_per_second: bounded_number(
                &value,
                &["network", "sendBytesPerSecond"],
                0.0,
                MAX_RATE,
            )?,
        },
        disks,
        gpus,
        processes,
    })
}

async fn signal_remote_process(
    runtime: &SshRuntime,
    host: &HostProfile,
    request: &ProcessSignalRequest<'_>,
) -> AppResult<()> {
    validate_process_signal(
        request.pid,
        request.expected_start_ticks,
        request.expected_user,
        request.expected_command,
        request.signal,
    )?;
    let result = runtime
        .run_command(
            host,
            build_signal_command(
                request.pid,
                request.expected_start_ticks,
                request.expected_user,
                request.expected_command,
                request.signal,
            ),
            None,
        )
        .await?;
    match result.exit_code {
        Some(0) => Ok(()),
        Some(44) => Err(AppError::State("process no longer exists".to_owned())),
        Some(45) => Err(AppError::State(
            "process identity changed; refresh telemetry before signaling".to_owned(),
        )),
        _ => Err(AppError::Process(if result.stderr.trim().is_empty() {
            "remote signal command failed".to_owned()
        } else {
            result.stderr
        })),
    }
}

fn validate_process_signal(
    pid: u32,
    expected_start_ticks: u64,
    expected_user: &str,
    expected_command: &str,
    signal: &str,
) -> AppResult<()> {
    if pid == 0
        || expected_start_ticks == 0
        || expected_user.is_empty()
        || expected_user.len() > MAX_USERNAME_BYTES
        || expected_command.is_empty()
        || expected_command.len() > MAX_PROCESS_COMMAND_BYTES
        || expected_user.contains(['\0', '\r', '\n'])
        || expected_command.contains(['\0', '\r', '\n'])
        || expected_user.trim() != expected_user
        || expected_command.trim() != expected_command
    {
        return Err(AppError::Validation(
            "process identity is invalid".to_owned(),
        ));
    }
    if !matches!(signal, "TERM" | "KILL") {
        return Err(AppError::Validation(
            "process signal must be TERM or KILL".to_owned(),
        ));
    }
    Ok(())
}

fn build_signal_command(
    pid: u32,
    expected_start_ticks: u64,
    expected_user: &str,
    expected_command: &str,
    signal: &str,
) -> String {
    format!(
        "pid={pid}\nexpected_start={expected_start_ticks}\nexpected_user={}\nexpected_command={}\nactual_start=\"$(awk '{{print $22}}' \"/proc/$pid/stat\" 2>/dev/null)\" || exit 44\nactual_user=\"$(ps -p \"$pid\" -o user= 2>/dev/null | awk '{{$1=$1;print}}')\"\nactual_command=\"$(ps -p \"$pid\" -o args= 2>/dev/null)\"\n[ -n \"$actual_start\" ] || exit 44\n[ \"$actual_start\" = \"$expected_start\" ] || exit 45\n[ \"$actual_user\" = \"$expected_user\" ] || exit 45\n[ \"$actual_command\" = \"$expected_command\" ] || exit 45\ncurrent_start=\"$(awk '{{print $22}}' \"/proc/$pid/stat\" 2>/dev/null)\" || exit 44\n[ \"$current_start\" = \"$expected_start\" ] || exit 45\nkill -s {signal} -- \"$pid\"",
        posix_quote(expected_user),
        posix_quote(expected_command),
    )
}

fn posix_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn process_identity(
    host_id: &str,
    pid: u32,
    start_ticks: u64,
    user: &str,
    command: &str,
) -> ProcessIdentity {
    ProcessIdentity {
        host_id: host_id.to_owned(),
        pid,
        start_ticks,
        user_digest: Sha256::digest(user.as_bytes()).into(),
        command_digest: Sha256::digest(command.as_bytes()).into(),
    }
}

impl TelemetryRegistry {
    pub async fn signal_process(
        &self,
        runtime: &SshRuntime,
        host: &HostProfile,
        request: ProcessSignalRequest<'_>,
    ) -> AppResult<()> {
        validate_process_signal(
            request.pid,
            request.expected_start_ticks,
            request.expected_user,
            request.expected_command,
            request.signal,
        )?;
        let identity = process_identity(
            &host.id,
            request.pid,
            request.expected_start_ticks,
            request.expected_user,
            request.expected_command,
        );
        if request.signal == "KILL" {
            let now = Instant::now();
            let has_recent_term = {
                let mut attempts = self.term_attempts.lock();
                attempts.retain(|_, attempted_at| {
                    now.saturating_duration_since(*attempted_at) <= TERM_ATTEMPT_TTL
                });
                attempts.contains_key(&identity)
            };
            if !has_recent_term {
                return Err(AppError::State(
                    "send TERM to this exact process before escalating to KILL".to_owned(),
                ));
            }
        }

        signal_remote_process(runtime, host, &request).await?;

        let now = Instant::now();
        let mut attempts = self.term_attempts.lock();
        if request.signal == "TERM" {
            attempts.retain(|_, attempted_at| {
                now.saturating_duration_since(*attempted_at) <= TERM_ATTEMPT_TTL
            });
            if attempts.len() >= MAX_TERM_ATTEMPTS
                && let Some(oldest) = attempts
                    .iter()
                    .min_by_key(|(_, attempted_at)| **attempted_at)
                    .map(|(identity, _)| identity.clone())
            {
                attempts.remove(&oldest);
            }
            attempts.insert(identity, now);
        } else {
            attempts.remove(&identity);
        }
        Ok(())
    }

    pub async fn probe_btop(
        &self,
        runtime: &SshRuntime,
        host: &HostProfile,
        default_rotation_minutes: u64,
    ) -> AppResult<BtopStatus> {
        let _permit = acquire_btop_permit(self.btop_limiter.clone()).await?;
        let rotation_minutes = {
            let watchdogs = self.watchdogs.read();
            watchdogs
                .get(&host.id)
                .map_or(default_rotation_minutes, |watchdog| {
                    watchdog.rotation_minutes
                })
        };
        inspect_btop_status(runtime, host, rotation_minutes, &self.watchdog_owner_nonce).await
    }

    pub async fn start_btop_watchdog(
        &self,
        runtime: &SshRuntime,
        host: &HostProfile,
        rotation_minutes: u64,
    ) -> AppResult<BtopStatus> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AppError::State(
                "telemetry is shutting down and cannot start btop".to_owned(),
            ));
        }
        if !(1..=1440).contains(&rotation_minutes) {
            return Err(AppError::Validation(
                "btop rotation must be between 1 and 1440 minutes".to_owned(),
            ));
        }
        if self
            .watchdogs
            .read()
            .get(&host.id)
            .is_some_and(|tracked| !same_connection_profile(&tracked.host, host))
        {
            return Err(AppError::State(
                "stop the tracked btop watchdog before changing this host's SSH connection settings"
                    .to_owned(),
            ));
        }
        let watchdog = OwnedWatchdog {
            host: host.clone(),
            runtime: runtime.clone(),
            rotation_minutes,
            owner_nonce: self.watchdog_owner_nonce.clone(),
        };
        let _operation = self.reserve_watchdog_operation(watchdog.clone())?;
        let current_host = runtime.current_host_profile(host)?;
        if !same_connection_profile(host, &current_host) {
            return Err(AppError::State(
                "the host changed while btop was starting; retry with the saved profile".to_owned(),
            ));
        }
        let _permit = acquire_btop_permit(self.btop_limiter.clone()).await?;
        let result = run_btop_command(
            runtime,
            host,
            build_btop_start_command(host, rotation_minutes, &self.watchdog_owner_nonce),
        )
        .await?;
        if !matches!(result.exit_code, Some(0 | 47)) {
            return Err(btop_start_error(result));
        }
        let disposition = watchdog_start_disposition(result.exit_code);
        if disposition == WatchdogStartDisposition::Conflict {
            return Err(AppError::State(
                "refused to adopt a pre-existing tmux watchdog session".to_owned(),
            ));
        }
        let created = disposition == WatchdogStartDisposition::Created;
        if created {
            self.watchdogs
                .write()
                .insert(host.id.clone(), watchdog.clone());
        }
        if self.shutting_down.load(Ordering::Acquire) {
            let _ = stop_remote_owned_watchdog(&watchdog).await;
            self.watchdogs.write().remove(&host.id);
            return Err(AppError::State(
                "application shutdown interrupted btop startup".to_owned(),
            ));
        }
        let status =
            inspect_btop_status(runtime, host, rotation_minutes, &self.watchdog_owner_nonce)
                .await?;
        if status.watchdog_state != "running" {
            if created {
                let _ = stop_remote_owned_watchdog(&watchdog).await;
                self.watchdogs.write().remove(&host.id);
            }
            return Err(AppError::State(status.last_error.clone().unwrap_or_else(
                || "btop watchdog did not become ready".to_owned(),
            )));
        }
        if self.shutting_down.load(Ordering::Acquire) {
            let _ = stop_remote_owned_watchdog(&watchdog).await;
            self.watchdogs.write().remove(&host.id);
            return Err(AppError::State(
                "application shutdown interrupted btop startup".to_owned(),
            ));
        }
        self.watchdogs.write().insert(host.id.clone(), watchdog);
        Ok(status)
    }

    pub async fn stop_btop_watchdog(
        &self,
        _runtime: &SshRuntime,
        host: &HostProfile,
        _default_rotation_minutes: u64,
    ) -> AppResult<BtopStatus> {
        let watchdog = {
            let watchdogs = self.watchdogs.read();
            watchdogs.get(&host.id).cloned().ok_or_else(|| {
                AppError::State(
                    "refused to stop a btop session not created by this app instance".to_owned(),
                )
            })?
        };
        let rotation_minutes = watchdog.rotation_minutes;
        let _operation = self.reserve_watchdog_operation(watchdog.clone())?;
        let _permit = acquire_btop_permit(self.btop_limiter.clone()).await?;
        stop_remote_owned_watchdog_parts(&watchdog.runtime, &watchdog.host, &watchdog.owner_nonce)
            .await?;
        self.watchdogs.write().remove(&host.id);
        inspect_btop_status(
            &watchdog.runtime,
            &watchdog.host,
            rotation_minutes,
            &watchdog.owner_nonce,
        )
        .await
    }

    pub async fn remove_host(&self, events: EventSink, host_id: &str) -> AppResult<()> {
        self.stop(events, host_id).await?;
        if self.watchdog_operations.lock().contains_key(host_id) {
            return Err(AppError::State(format!(
                "btop operation for host {host_id} is still running"
            )));
        }
        let watchdog = {
            let watchdogs = self.watchdogs.read();
            watchdogs.get(host_id).cloned()
        };
        if let Some(watchdog) = watchdog {
            self.stop_btop_watchdog(&watchdog.runtime, &watchdog.host, watchdog.rotation_minutes)
                .await?;
        }
        self.statuses.write().remove(host_id);
        self.history.write().remove_host(host_id);
        self.term_attempts
            .lock()
            .retain(|identity, _| identity.host_id != host_id);
        Ok(())
    }

    pub async fn shutdown(&self, events: EventSink) {
        self.shutting_down.store(true, Ordering::Release);
        let pending = self.pending_watchdogs();
        let limiter = self.btop_limiter.clone();
        let _ = tokio::time::timeout(BTOP_SHUTDOWN_TIMEOUT, async {
            tokio::join!(self.stop_all_monitors(&events), async {
                stop_watchdog_contexts(pending, limiter).await;
                while !self.watchdog_operations.lock().is_empty() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            });
        })
        .await;
        self.watchdogs.write().clear();
        self.term_attempts.lock().clear();
    }

    fn reserve_watchdog_operation(
        &self,
        watchdog: OwnedWatchdog,
    ) -> AppResult<WatchdogOperationGuard> {
        let host_id = watchdog.host.id.clone();
        let mut operations = self.watchdog_operations.lock();
        if operations.contains_key(&host_id) {
            return Err(AppError::State(format!(
                "another btop operation for host {host_id} is already running"
            )));
        }
        operations.insert(host_id.clone(), watchdog);
        drop(operations);
        Ok(WatchdogOperationGuard {
            host_id,
            operations: self.watchdog_operations.clone(),
        })
    }

    fn pending_watchdogs(&self) -> HashMap<String, OwnedWatchdog> {
        let mut pending = self.watchdogs.read().clone();
        for (host_id, watchdog) in self.watchdog_operations.lock().iter() {
            pending
                .entry(host_id.clone())
                .or_insert_with(|| watchdog.clone());
        }
        pending
    }
}

async fn stop_watchdog_contexts(
    watchdogs: HashMap<String, OwnedWatchdog>,
    limiter: Arc<Semaphore>,
) {
    let mut tasks = tokio::task::JoinSet::new();
    for watchdog in watchdogs.into_values() {
        let limiter = limiter.clone();
        tasks.spawn(async move {
            if let Ok(_permit) = acquire_btop_permit(limiter).await {
                let _ = stop_remote_owned_watchdog(&watchdog).await;
            }
        });
    }
    while tasks.join_next().await.is_some() {}
}

async fn acquire_btop_permit(limiter: Arc<Semaphore>) -> AppResult<OwnedSemaphorePermit> {
    tokio::time::timeout(BTOP_COMMAND_TIMEOUT, limiter.acquire_owned())
        .await
        .map_err(|_| AppError::Timeout("waiting for a btop operation slot timed out".to_owned()))?
        .map_err(|_| AppError::State("btop operation limiter is closed".to_owned()))
}

async fn run_btop_command(
    runtime: &SshRuntime,
    host: &HostProfile,
    command: String,
) -> AppResult<CommandResult> {
    tokio::time::timeout(
        BTOP_COMMAND_TIMEOUT,
        runtime.run_command(host, command, None),
    )
    .await
    .map_err(|_| AppError::Timeout("btop remote operation exceeded 20 seconds".to_owned()))?
}

async fn inspect_btop_status(
    runtime: &SshRuntime,
    host: &HostProfile,
    default_rotation_minutes: u64,
    owner_nonce: &str,
) -> AppResult<BtopStatus> {
    let result =
        run_btop_command(runtime, host, build_btop_status_command(host, owner_nonce)).await?;
    if result.exit_code != Some(0) {
        return Err(AppError::Process(nonempty_btop_error(
            &result,
            "failed to inspect btop watchdog",
        )));
    }
    Ok(parse_btop_status(
        &host.id,
        default_rotation_minutes,
        &result.stdout,
    ))
}

async fn stop_remote_owned_watchdog(watchdog: &OwnedWatchdog) -> AppResult<()> {
    stop_remote_owned_watchdog_parts(&watchdog.runtime, &watchdog.host, &watchdog.owner_nonce).await
}

async fn stop_remote_owned_watchdog_parts(
    runtime: &SshRuntime,
    host: &HostProfile,
    owner_nonce: &str,
) -> AppResult<()> {
    let result =
        run_btop_command(runtime, host, build_btop_stop_command(host, owner_nonce)).await?;
    match result.exit_code {
        Some(0) => Ok(()),
        Some(46) => Err(AppError::State(
            "refused to stop a tmux session not marked as RemoteDeck-owned".to_owned(),
        )),
        _ => Err(AppError::Process(nonempty_btop_error(
            &result,
            "failed to stop btop watchdog",
        ))),
    }
}

fn btop_start_error(result: CommandResult) -> AppError {
    let fallback = match result.exit_code {
        Some(40) => "remote btop is not installed",
        Some(41) => "remote tmux is not installed",
        Some(42) => "remote timeout is not installed",
        Some(43) => "the deterministic tmux session is not owned by this RemoteDeck installation",
        Some(44) => "tmux could not create the RemoteDeck btop session",
        Some(45) => "tmux could not mark the btop session as RemoteDeck-owned",
        _ => "failed to start btop watchdog",
    };
    AppError::Process(nonempty_btop_error(&result, fallback))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchdogStartDisposition {
    Created,
    Adopted,
    Conflict,
}

fn watchdog_start_disposition(exit_code: Option<i32>) -> WatchdogStartDisposition {
    match exit_code {
        Some(0) => WatchdogStartDisposition::Created,
        Some(47) => WatchdogStartDisposition::Adopted,
        _ => WatchdogStartDisposition::Conflict,
    }
}

fn nonempty_btop_error(result: &CommandResult, fallback: &str) -> String {
    let error = result.stderr.trim();
    if error.is_empty() {
        fallback.to_owned()
    } else {
        error.chars().take(2_048).collect()
    }
}

fn build_btop_start_command(
    host: &HostProfile,
    rotation_minutes: u64,
    owner_nonce: &str,
) -> String {
    let session = btop_session(host);
    let seconds = rotation_minutes.saturating_mul(60);
    let owner = btop_owner_value(owner_nonce);
    format!(
        "command -v btop >/dev/null || exit 40; \
         command -v tmux >/dev/null || exit 41; \
         command -v timeout >/dev/null || exit 42; \
         if tmux has-session -t '{session}' 2>/dev/null; then owner=$(tmux show-options -v -t '{session}' {BTOP_OWNER_OPTION} 2>/dev/null || true); if [ \"$owner\" = '{owner}' ]; then exit 47; else exit 43; fi; fi; \
         tmux new-session -d -s '{session}' 'remaining=30; while [ \"$(tmux show-options -v -t {session} {BTOP_OWNER_OPTION} 2>/dev/null)\" != \"{owner}\" ]; do remaining=$((remaining - 1)); if [ \"$remaining\" -le 0 ]; then tmux kill-session -t {session} 2>/dev/null || true; exit 1; fi; sleep 1; done; count=0; while :; do timeout {seconds}s btop; count=$((count + 1)); tmux set-option -q -t {session} {BTOP_RESTART_OPTION} \"$count\"; sleep 1; done' || exit 44; \
         tmux set-option -q -t '{session}' {BTOP_ROTATION_OPTION} '{rotation_minutes}' && \
         tmux set-option -q -t '{session}' {BTOP_RESTART_OPTION} '0' && \
         tmux set-option -q -t '{session}' {BTOP_OWNER_OPTION} '{owner}' || \
         {{ tmux kill-session -t '{session}' 2>/dev/null || true; exit 45; }}"
    )
}

fn build_btop_stop_command(host: &HostProfile, owner_nonce: &str) -> String {
    let session = btop_session(host);
    let expected_owner = btop_owner_value(owner_nonce);
    format!(
        "if ! tmux has-session -t '{session}' 2>/dev/null; then exit 0; fi; \
         owner=$(tmux show-options -v -t '{session}' {BTOP_OWNER_OPTION} 2>/dev/null || true); \
         if [ \"$owner\" != '{expected_owner}' ]; then exit 46; fi; \
         tmux kill-session -t '{session}'"
    )
}

fn build_btop_status_command(host: &HostProfile, owner_nonce: &str) -> String {
    let session = btop_session(host);
    let expected_owner = btop_owner_value(owner_nonce);
    format!(
        "if command -v btop >/dev/null; then \
             version=$(btop --version 2>/dev/null | head -n 1); \
             printf '__REMOTEDECK_BTOP_VERSION__%s\\n' \"$version\"; \
         else printf '__REMOTEDECK_BTOP_MISSING__\\n'; fi; \
         if ! command -v tmux >/dev/null; then printf '__REMOTEDECK_TMUX_MISSING__\\n'; \
         elif tmux has-session -t '{session}' 2>/dev/null; then \
             owner=$(tmux show-options -v -t '{session}' {BTOP_OWNER_OPTION} 2>/dev/null || true); \
             if [ \"$owner\" = '{expected_owner}' ]; then \
                 printf '__REMOTEDECK_WATCHDOG_RUNNING__\\n'; \
                 printf '__REMOTEDECK_ROTATION__%s\\n' \"$(tmux show-options -v -t '{session}' {BTOP_ROTATION_OPTION} 2>/dev/null)\"; \
                 printf '__REMOTEDECK_RESTARTS__%s\\n' \"$(tmux show-options -v -t '{session}' {BTOP_RESTART_OPTION} 2>/dev/null)\"; \
             else printf '__REMOTEDECK_WATCHDOG_CONFLICT__\\n'; fi; \
         fi; \
         if ! command -v timeout >/dev/null; then printf '__REMOTEDECK_TIMEOUT_MISSING__\\n'; fi"
    )
}

fn btop_owner_value(owner_nonce: &str) -> String {
    format!("{BTOP_OWNER_PREFIX}{owner_nonce}")
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

fn watchdog_uses_host(watchdog: &OwnedWatchdog, profile: &HostProfile) -> bool {
    connection_uses_host(&watchdog.host, profile)
}

fn connection_uses_host(connection: &HostProfile, profile: &HostProfile) -> bool {
    connection.id == profile.id
        || connection.proxy_jump.as_deref().is_some_and(|reference| {
            reference == profile.id || reference.eq_ignore_ascii_case(&profile.alias)
        })
}

fn parse_btop_status(host_id: &str, default_rotation_minutes: u64, stdout: &str) -> BtopStatus {
    let mut installed = true;
    let mut tmux_installed = true;
    let mut timeout_installed = true;
    let mut running = false;
    let mut conflict = false;
    let mut version = None;
    let mut rotation_minutes = default_rotation_minutes;
    let mut restart_count = None;
    let mut counter_invalid = false;
    for line in stdout.lines() {
        if line == "__REMOTEDECK_BTOP_MISSING__" {
            installed = false;
        } else if line == "__REMOTEDECK_TMUX_MISSING__" {
            tmux_installed = false;
        } else if line == "__REMOTEDECK_TIMEOUT_MISSING__" {
            timeout_installed = false;
        } else if line == "__REMOTEDECK_WATCHDOG_RUNNING__" {
            running = true;
        } else if line == "__REMOTEDECK_WATCHDOG_CONFLICT__" {
            conflict = true;
        } else if let Some(value) = line.strip_prefix("__REMOTEDECK_BTOP_VERSION__") {
            let value = value.trim();
            if !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control) {
                version = Some(value.to_owned());
            }
        } else if let Some(value) = line.strip_prefix("__REMOTEDECK_ROTATION__") {
            if let Ok(value) = value.trim().parse::<u64>()
                && (1..=1440).contains(&value)
            {
                rotation_minutes = value;
            }
        } else if let Some(value) = line.strip_prefix("__REMOTEDECK_RESTARTS__") {
            match value.trim().parse::<u64>() {
                Ok(value) => restart_count = Some(value),
                Err(_) => counter_invalid = true,
            }
        }
    }
    let dependencies_available = installed && tmux_installed && timeout_installed;
    let last_error = if !installed {
        Some("remote btop is not installed".to_owned())
    } else if !tmux_installed {
        Some("remote tmux is not installed".to_owned())
    } else if !timeout_installed {
        Some("remote timeout is not installed".to_owned())
    } else if conflict {
        Some(
            "the deterministic tmux name exists without the RemoteDeck ownership marker".to_owned(),
        )
    } else if running && (counter_invalid || restart_count.is_none()) {
        Some("watchdog restart counter is unavailable".to_owned())
    } else {
        None
    };
    BtopStatus {
        host_id: host_id.to_owned(),
        installed,
        version,
        watchdog_state: if !dependencies_available {
            "unavailable"
        } else if conflict {
            "conflict"
        } else if running {
            "running"
        } else {
            "stopped"
        }
        .to_owned(),
        rotation_minutes,
        restart_count: if running { restart_count } else { None },
        last_error,
    }
}

fn btop_session(host: &HostProfile) -> String {
    let mut short = host
        .id
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .take(12)
        .collect::<String>();
    if short.len() < 12 {
        short = Sha256::digest(host.id.as_bytes())[..6]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
    }
    format!("remotedeck-btop-{short}")
}

fn at_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
}

fn bounded_text(value: &Value, path: &[&str], max_bytes: usize) -> AppResult<String> {
    at_path(value, path)
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
        })
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::Process(format!("telemetry field {} is invalid", path.join("."))))
}

fn bounded_number(value: &Value, path: &[&str], min: f64, max: f64) -> AppResult<f64> {
    let number = at_path(value, path)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| {
            AppError::Process(format!("telemetry field {} is invalid", path.join(".")))
        })?;
    if !(min..=max).contains(&number) {
        return Err(AppError::Process(format!(
            "telemetry field {} is outside the supported range",
            path.join(".")
        )));
    }
    Ok(number)
}

fn bounded_value_number(value: &Value, name: &str, min: f64, max: f64) -> AppResult<f64> {
    let number = value
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| AppError::Process(format!("telemetry field {name} is invalid")))?;
    if !(min..=max).contains(&number) {
        return Err(AppError::Process(format!(
            "telemetry field {name} is outside the supported range"
        )));
    }
    Ok(number)
}

fn optional_bounded_number(
    value: &Value,
    path: &[&str],
    min: f64,
    max: f64,
) -> AppResult<Option<f64>> {
    match at_path(value, path) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => bounded_value_number(value, &path.join("."), min, max).map(Some),
    }
}

fn unsigned(value: &Value, path: &[&str]) -> AppResult<u64> {
    at_path(value, path)
        .and_then(Value::as_u64)
        .ok_or_else(|| AppError::Process(format!("telemetry field {} is invalid", path.join("."))))
}

fn bounded_array<'a>(value: &'a Value, name: &str, max_items: usize) -> AppResult<&'a Vec<Value>> {
    let values = value
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Process(format!("telemetry field {name} is invalid")))?;
    if values.len() > max_items {
        return Err(AppError::Process(format!(
            "telemetry field {name} exceeds {max_items} items"
        )));
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    const VALID_SAMPLE: &str = r#"{"capturedAt":"2026-01-01T00:00:00Z","hostname":"lab","currentUser":"alice","cpu":{"totalPercent":12.5,"loadAverage":[1.0,2.0,3.0]},"memory":{"usedBytes":2,"totalBytes":4,"swapUsedBytes":0,"swapTotalBytes":1},"network":{"receiveBytesPerSecond":5.0,"sendBytesPerSecond":6.0},"disks":[{"mount":"/","usedBytes":2,"totalBytes":4,"availableBytes":2}],"gpus":[{"index":0,"name":"GPU","utilizationPercent":10,"memoryUsedMiB":20,"memoryTotalMiB":40,"temperatureC":50,"powerW":60}],"processes":[{"pid":7,"startTicks":1234,"user":"alice","cpuPercent":1.0,"memoryPercent":2.0,"state":"S","elapsed":"9","command":"python"}]}"#;

    #[test]
    fn parses_collector_json_into_ui_contract() {
        let sample = parse_sample("host", VALID_SAMPLE).expect("sample");
        assert_eq!(sample.cpu.load15, 3.0);
        assert_eq!(sample.processes[0].elapsed_seconds, 9);
        assert_eq!(sample.processes[0].start_ticks, 1234);
        assert_eq!(sample.gpus[0].memory_total_mi_b, 40.0);
    }

    #[test]
    fn parser_rejects_oversized_collections_and_fields() {
        let mut value: Value = serde_json::from_str(VALID_SAMPLE).expect("fixture");
        let process = value["processes"][0].clone();
        value["processes"] = Value::Array(vec![process; MAX_PROCESSES + 1]);
        let error = parse_sample("host", &value.to_string()).expect_err("too many processes");
        assert!(error.to_string().contains("exceeds 256 items"));

        let mut value: Value = serde_json::from_str(VALID_SAMPLE).expect("fixture");
        value["processes"][0]["command"] = json!("x".repeat(MAX_PROCESS_COMMAND_BYTES + 1));
        let error = parse_sample("host", &value.to_string()).expect_err("long command");
        assert!(error.to_string().contains("command"));
    }

    #[test]
    fn parser_rejects_inconsistent_or_out_of_range_values() {
        let mut value: Value = serde_json::from_str(VALID_SAMPLE).expect("fixture");
        value["memory"]["usedBytes"] = json!(5);
        let error = parse_sample("host", &value.to_string()).expect_err("invalid memory");
        assert!(error.to_string().contains("memory capacity"));

        let mut value: Value = serde_json::from_str(VALID_SAMPLE).expect("fixture");
        value["cpu"]["totalPercent"] = json!(101);
        let error = parse_sample("host", &value.to_string()).expect_err("invalid CPU");
        assert!(error.to_string().contains("outside the supported range"));

        let mut value: Value = serde_json::from_str(VALID_SAMPLE).expect("fixture");
        value["capturedAt"] = json!("not-a-timestamp");
        let error = parse_sample("host", &value.to_string()).expect_err("invalid timestamp");
        assert!(error.to_string().contains("capturedAt"));
    }

    #[test]
    fn bounded_reader_rejects_a_line_before_unbounded_growth() {
        let input = vec![b'x'; MAX_SAMPLE_LINE_BYTES + 1];
        let error = read_bounded_line(&mut Cursor::new(input)).expect_err("oversized line");
        assert!(error.to_string().contains("exceeds 524288 bytes"));

        let mut input = Cursor::new(b"{\"ok\":true}\r\nnext\n".to_vec());
        assert_eq!(
            read_bounded_line(&mut input).expect("first"),
            Some("{\"ok\":true}".to_owned())
        );
        assert_eq!(
            read_bounded_line(&mut input).expect("second"),
            Some("next".to_owned())
        );
        assert_eq!(read_bounded_line(&mut input).expect("end"), None);
    }

    #[test]
    fn history_enforces_per_host_and_global_sample_budgets() {
        let mut history = HistoryStore::default();
        for sequence in 0..5 {
            history.record(snapshot("one", sequence, 0), 3);
        }
        assert_eq!(history.samples("one").len(), 3);
        assert_eq!(history.total_samples, 3);

        let mut history = HistoryStore::default();
        for sequence in 0..=MAX_HISTORY_SAMPLES_GLOBAL {
            history.record(
                snapshot(&format!("host-{}", sequence % 3), sequence, 0),
                MAX_HISTORY_SAMPLES_PER_HOST,
            );
        }
        assert_eq!(history.total_samples, MAX_HISTORY_SAMPLES_GLOBAL);
        assert!(
            history
                .hosts
                .values()
                .all(|host| host.samples.len() <= MAX_HISTORY_SAMPLES_PER_HOST)
        );
    }

    #[test]
    fn history_enforces_accounted_byte_budget_and_removal() {
        let mut history = HistoryStore::default();
        history.record(
            snapshot("large", 0, MAX_HISTORY_BYTES_PER_HOST + 1),
            MAX_HISTORY_SAMPLES_PER_HOST,
        );
        assert!(history.samples("large").is_empty());
        assert_eq!(history.total_accounted_bytes, 0);

        history.record(snapshot("remove-me", 1, 32), 10);
        assert!(!history.samples("remove-me").is_empty());
        history.remove_host("remove-me");
        assert!(history.samples("remove-me").is_empty());
        assert_eq!(history.total_samples, 0);
    }

    #[test]
    fn btop_session_is_owned_and_shell_safe() {
        let host = test_host();
        assert_eq!(btop_session(&host), "remotedeck-btop-111111112222");

        let mut hostile = host;
        hostile.id = "'; rm -rf /".to_owned();
        let session = btop_session(&hostile);
        assert!(session.starts_with("remotedeck-btop-"));
        assert_eq!(session.len(), "remotedeck-btop-".len() + 12);
        assert!(
            session
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        );
    }

    #[test]
    fn btop_commands_require_and_verify_remotedeck_ownership() {
        let host = test_host();
        let owner_nonce = "0123456789abcdef0123456789abcdef";
        let other_nonce = "fedcba9876543210fedcba9876543210";
        let owner = btop_owner_value(owner_nonce);
        let other_owner = btop_owner_value(other_nonce);
        let start = build_btop_start_command(&host, 15, owner_nonce);
        assert!(start.contains("timeout 900s btop"));
        assert!(start.contains(BTOP_OWNER_OPTION));
        assert!(start.contains(&owner));
        assert!(!start.contains(&other_owner));
        assert!(start.contains(BTOP_RESTART_OPTION));
        assert!(start.contains("count=$((count + 1))"));
        assert!(start.contains("remaining=30"));
        assert!(start.contains("exit 47"));

        let stop = build_btop_stop_command(&host, owner_nonce);
        let owner_check = stop.find(&owner).expect("owner marker");
        let kill = stop.rfind("tmux kill-session").expect("kill");
        assert!(owner_check < kill);
        assert!(!stop.contains(&other_owner));
        assert!(stop.contains("exit 46"));

        let status = build_btop_status_command(&host, owner_nonce);
        assert!(status.contains(BTOP_OWNER_OPTION));
        assert!(status.contains(&owner));
        assert!(!status.contains(&other_owner));
        assert!(status.contains("__REMOTEDECK_RESTARTS__"));
    }

    #[test]
    fn registry_nonce_is_persistable_and_shared_by_clones() {
        let persisted: Arc<str> = Arc::from("0123456789abcdef0123456789abcdef");
        let registry = TelemetryRegistry::with_owner_nonce(persisted.clone());
        let cloned = registry.clone();
        let other = TelemetryRegistry::default();

        assert_eq!(
            registry.watchdog_owner_nonce.as_ref(),
            cloned.watchdog_owner_nonce.as_ref()
        );
        assert_eq!(registry.watchdog_owner_nonce.as_ref(), persisted.as_ref());
        assert_ne!(
            registry.watchdog_owner_nonce.as_ref(),
            other.watchdog_owner_nonce.as_ref()
        );
        assert_eq!(registry.watchdog_owner_nonce.len(), 32);
        assert!(
            registry
                .watchdog_owner_nonce
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        );
        assert_eq!(
            registry.btop_limiter.available_permits(),
            MAX_CONCURRENT_BTOP_OPERATIONS
        );
    }

    #[test]
    fn persisted_ownership_proof_recovers_only_matching_watchdogs() {
        assert_eq!(
            watchdog_start_disposition(Some(0)),
            WatchdogStartDisposition::Created
        );
        assert_eq!(
            watchdog_start_disposition(Some(47)),
            WatchdogStartDisposition::Adopted
        );
        assert_eq!(
            watchdog_start_disposition(Some(43)),
            WatchdogStartDisposition::Conflict
        );
        assert_eq!(
            watchdog_start_disposition(None),
            WatchdogStartDisposition::Conflict
        );
    }

    #[test]
    fn btop_status_reports_real_counter_and_remote_rotation() {
        let status = parse_btop_status(
            "host",
            15,
            "__REMOTEDECK_BTOP_VERSION__btop version 1.4.0\n\
             __REMOTEDECK_WATCHDOG_RUNNING__\n\
             __REMOTEDECK_ROTATION__20\n\
             __REMOTEDECK_RESTARTS__7\n",
        );
        assert!(status.installed);
        assert_eq!(status.watchdog_state, "running");
        assert_eq!(status.rotation_minutes, 20);
        assert_eq!(status.restart_count, Some(7));
        assert_eq!(status.last_error, None);
    }

    #[test]
    fn btop_status_never_fabricates_a_counter_or_ownership() {
        let stopped = parse_btop_status(
            "host",
            15,
            "__REMOTEDECK_BTOP_VERSION__btop version 1.4.0\n",
        );
        assert_eq!(stopped.watchdog_state, "stopped");
        assert_eq!(stopped.restart_count, None);

        let conflict = parse_btop_status(
            "host",
            15,
            "__REMOTEDECK_BTOP_VERSION__btop version 1.4.0\n\
             __REMOTEDECK_WATCHDOG_CONFLICT__\n",
        );
        assert_eq!(conflict.watchdog_state, "conflict");
        assert_eq!(conflict.restart_count, None);
        assert!(conflict.last_error.is_some());

        let unavailable = parse_btop_status(
            "host",
            15,
            "__REMOTEDECK_BTOP_VERSION__btop version 1.4.0\n\
             __REMOTEDECK_TMUX_MISSING__\n",
        );
        assert_eq!(unavailable.watchdog_state, "unavailable");
        assert_eq!(unavailable.restart_count, None);
    }

    fn test_host() -> HostProfile {
        HostProfile {
            schema_version: 2,
            id: "11111111-2222-4333-8444-555555555555".to_owned(),
            alias: "lab".to_owned(),
            hostname: "host".to_owned(),
            port: 22,
            username: "user".to_owned(),
            auth_method: crate::model::AuthMethod::Interactive,
            identity_file: None,
            proxy_jump: None,
            default_workspace: "~".to_owned(),
            groups: Vec::new(),
            advanced: Default::default(),
            monitor_enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn snapshot(host_id: &str, sequence: usize, command_bytes: usize) -> TelemetrySnapshot {
        TelemetrySnapshot {
            host_id: host_id.to_owned(),
            sampled_at: format!("2026-01-01T00:00:{:02}Z", sequence % 60),
            hostname: "lab".to_owned(),
            current_user: "alice".to_owned(),
            cpu: CpuSnapshot {
                percent: 1.0,
                load1: 1.0,
                load5: 1.0,
                load15: 1.0,
            },
            memory: MemorySnapshot {
                used_bytes: 1,
                total_bytes: 2,
                swap_used_bytes: 0,
                swap_total_bytes: 0,
            },
            network: NetworkSnapshot {
                receive_bytes_per_second: 0.0,
                send_bytes_per_second: 0.0,
            },
            disks: Vec::new(),
            gpus: Vec::new(),
            processes: if command_bytes == 0 {
                Vec::new()
            } else {
                vec![ProcessSnapshot {
                    pid: 1,
                    start_ticks: 10,
                    user: "alice".to_owned(),
                    cpu_percent: 0.0,
                    memory_percent: 0.0,
                    state: "S".to_owned(),
                    elapsed_seconds: 1,
                    command: "x".repeat(command_bytes),
                }]
            },
        }
    }

    #[test]
    fn signal_command_quotes_identity_and_rechecks_start_ticks() {
        let command = build_signal_command(7, 1234, "alice", "python 'job.py'", "TERM");
        assert!(command.contains("expected_start=1234"));
        assert!(command.contains("expected_command='python '\\''job.py'\\'''"));
        assert_eq!(command.matches("/proc/$pid/stat").count(), 2);
        assert!(command.ends_with("kill -s TERM -- \"$pid\""));
        assert!(validate_process_signal(7, 1234, "alice", "python 'job.py'", "TERM").is_ok());
    }

    #[test]
    fn process_signal_validation_rejects_unbound_or_injected_identity() {
        assert!(validate_process_signal(0, 1, "alice", "python", "TERM").is_err());
        assert!(validate_process_signal(7, 0, "alice", "python", "TERM").is_err());
        assert!(validate_process_signal(7, 1, "alice\nroot", "python", "TERM").is_err());
        assert!(validate_process_signal(7, 1, "alice", "python\nkill", "TERM").is_err());
        assert!(validate_process_signal(7, 1, "alice", "python", "STOP").is_err());
    }
}
