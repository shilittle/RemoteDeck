//! Security-sensitive, bounded background execution for non-PTY command jobs.
//!
//! Repository-facing entry points deliberately resolve the host and preset for
//! every analysis and run.  A renderer-provided command can therefore never
//! override a stored preset after the preset has been selected.

use crate::{
    host_operation::{HostOperationBarrier, HostOperationLease},
    model::{CommandPreset, CommandRisk, HostProfile},
    risk::{self, ConfirmationRequirement, RiskLevel},
    ssh::SshRuntime,
    store::AppRepository,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::OsString,
    fmt,
    path::PathBuf,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::{Semaphore, broadcast, mpsc, watch},
};
use uuid::Uuid;

pub const COMMAND_OUTPUT_LIMIT: usize = 1024 * 1024;
const OUTPUT_TRUNCATED_MARKER: &str = "\n[RemoteDeck: output truncated at 1 MiB]";
const MAX_COMMAND_BYTES: usize = 32 * 1024;
const MAX_WORKING_DIRECTORY_BYTES: usize = 4096;
const MAX_CONFIRMATION_CHARS: usize = 256;
const MAX_CONCURRENT_JOBS: usize = 16;
const MAX_PENDING_JOBS: usize = 128;
const MAX_RETAINED_JOBS: usize = 512;
const MAX_RETAINED_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const EVENT_BUFFER: usize = 128;
const COMMAND_CANCEL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const COMMAND_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(7);
const HOST_RETIREMENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandRunRequest {
    pub host_id: String,
    pub command_id: Option<String>,
    pub command: Option<String>,
    pub working_directory: Option<String>,
    pub confirmation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandAnalysis {
    pub target_alias: String,
    pub display_command: String,
    pub working_directory: Option<String>,
    pub declared_risk: CommandRisk,
    pub effective_risk: CommandRisk,
    pub reasons: Vec<String>,
    pub required_confirmation: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CommandJobState {
    Queued,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}

impl CommandJobState {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandJob {
    pub id: String,
    pub command_id: Option<String>,
    pub host_id: String,
    pub name: String,
    pub command: String,
    pub risk: CommandRisk,
    pub state: CommandJobState,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandEvent {
    pub job: CommandJob,
}

/// A fully resolved terminal hand-off.  The command job registry never runs
/// this plan in batch mode; the library layer must open a normal terminal.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PtyCommandPlan {
    pub host_id: String,
    pub command_id: Option<String>,
    pub name: String,
    pub command: String,
    pub working_directory: Option<String>,
    pub risk: CommandRisk,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CommandJobError {
    InvalidRequest { message: String },
    Repository { message: String },
    RiskAnalysis { message: String },
    ConfirmationRequired { risk: CommandRisk, expected: String },
    ConfirmationMismatch { risk: CommandRisk, expected: String },
    RequiresPty { plan: Box<PtyCommandPlan> },
    RuntimeUnavailable,
    Process { message: String },
    UnknownJob { job_id: String },
}

impl fmt::Display for CommandJobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest { message } => {
                write!(formatter, "invalid command request: {message}")
            }
            Self::Repository { message } => {
                write!(formatter, "command repository error: {message}")
            }
            Self::RiskAnalysis { message } => {
                write!(formatter, "command risk analysis failed: {message}")
            }
            Self::ConfirmationRequired { risk, expected } => write!(
                formatter,
                "{risk:?} command requires explicit confirmation using '{expected}'"
            ),
            Self::ConfirmationMismatch { risk, .. } => {
                write!(formatter, "{risk:?} command confirmation does not match")
            }
            Self::RequiresPty { .. } => {
                formatter.write_str("command requires a PTY and must be opened in a terminal")
            }
            Self::RuntimeUnavailable => {
                formatter.write_str("a Tokio runtime is required to start a command job")
            }
            Self::Process { message } => write!(formatter, "command process failed: {message}"),
            Self::UnknownJob { job_id } => write!(formatter, "unknown command job: {job_id}"),
        }
    }
}

impl std::error::Error for CommandJobError {}

#[derive(Debug, Clone)]
pub struct ResolvedCommand {
    pub host: HostProfile,
    pub command_id: Option<String>,
    pub name: String,
    /// Command after applying the preset's sudo wrapper, but before `cd`.
    pub command: String,
    pub working_directory: Option<String>,
    pub declared_risk: CommandRisk,
    pub requires_pty: bool,
    pub confirmation_text: Option<String>,
}

#[derive(Debug, Clone)]
struct PreparedCommand {
    host: HostProfile,
    command_id: Option<String>,
    name: String,
    display_command: String,
    risk: CommandRisk,
}

/// Reloads the selected host and, when present, the selected command preset.
/// Callers must use this function independently for analyze and run requests.
pub fn resolve_repository_request(
    repository: &AppRepository,
    request: &CommandRunRequest,
) -> Result<ResolvedCommand, CommandJobError> {
    validate_request_shape(request)?;
    let host = repository
        .host(&request.host_id)
        .map_err(|error| CommandJobError::Repository {
            message: error.to_string(),
        })?;
    let preset = request
        .command_id
        .as_deref()
        .map(|command_id| {
            repository
                .command(command_id)
                .map_err(|error| CommandJobError::Repository {
                    message: error.to_string(),
                })
        })
        .transpose()?;
    resolve_command_request(host, preset, request)
}

/// Pure resolution helper used after repository values have been reloaded.
pub fn resolve_command_request(
    host: HostProfile,
    preset: Option<CommandPreset>,
    request: &CommandRunRequest,
) -> Result<ResolvedCommand, CommandJobError> {
    validate_request_shape(request)?;
    if host.id != request.host_id {
        return invalid("resolved host does not match hostId");
    }

    let (
        command_id,
        name,
        raw_command,
        preset_directory,
        declared_risk,
        requires_pty,
        requires_sudo,
        confirmation_text,
    ) = match request.command_id.as_deref() {
        Some(command_id) => {
            if request.command.is_some() {
                return invalid("command cannot override a selected commandId");
            }
            let preset = preset.ok_or_else(|| CommandJobError::InvalidRequest {
                message: "commandId was not resolved from the repository".to_owned(),
            })?;
            if preset.id != command_id {
                return invalid("resolved preset does not match commandId");
            }
            if preset.host_id.as_deref().is_some_and(|id| id != host.id) {
                return invalid("selected command preset belongs to another host");
            }
            let declared_risk = trusted_preset_risk(&preset)?;
            (
                Some(preset.id),
                preset.name,
                preset.command,
                preset.working_directory,
                declared_risk,
                preset.requires_pty,
                preset.requires_sudo,
                preset.confirmation_text,
            )
        }
        None => {
            if preset.is_some() {
                return invalid("a resolved preset requires commandId");
            }
            let command =
                request
                    .command
                    .clone()
                    .ok_or_else(|| CommandJobError::InvalidRequest {
                        message: "either commandId or command is required".to_owned(),
                    })?;
            (
                None,
                "Ad hoc command".to_owned(),
                command,
                None,
                CommandRisk::L1,
                false,
                false,
                None,
            )
        }
    };

    validate_command_text(&raw_command)?;
    let command = if requires_sudo {
        format!("sudo -- sh -lc {}", posix_quote(raw_command.trim()))
    } else {
        raw_command.trim().to_owned()
    };
    validate_command_text(&command)?;

    let working_directory = normalize_directory(
        request
            .working_directory
            .clone()
            .or(preset_directory)
            .or_else(|| Some(host.default_workspace.clone())),
    )?;
    let display_command = in_directory(&command, working_directory.as_deref());
    validate_command_text(&display_command)?;

    Ok(ResolvedCommand {
        host,
        command_id,
        name,
        command,
        working_directory,
        declared_risk,
        requires_pty,
        confirmation_text: normalize_confirmation(confirmation_text)?,
    })
}

/// Performs the Rust risk check on a resolved command.  The declared risk is a
/// floor, and the final sudo wrapper is included in classification.
pub fn analyze_resolved_command(
    resolved: &ResolvedCommand,
) -> Result<CommandAnalysis, CommandJobError> {
    let declared = risk_level(resolved.declared_risk);
    let fallback_confirmation = resolved.host.alias.trim();
    let l2_confirmation = resolved
        .confirmation_text
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_confirmation);
    let checked = risk::analyze_command(&resolved.command, declared, Some(l2_confirmation))
        .map_err(|error| CommandJobError::RiskAnalysis {
            message: error.to_string(),
        })?;
    let effective_risk = command_risk(checked.effective_risk);
    let required_confirmation = match checked.confirmation {
        ConfirmationRequirement::None => None,
        // The string request contract has no separate acknowledgement bit.
        // Requiring the visible host alias makes L1 acknowledgement explicit.
        ConfirmationRequirement::Acknowledge => Some(resolved.host.alias.clone()),
        ConfirmationRequirement::TypeExact { value } => Some(value),
    };
    let display_command = in_directory(&resolved.command, resolved.working_directory.as_deref());
    validate_command_text(&display_command)?;
    Ok(CommandAnalysis {
        target_alias: resolved.host.alias.clone(),
        display_command,
        working_directory: resolved.working_directory.clone(),
        declared_risk: resolved.declared_risk,
        effective_risk,
        reasons: checked.reasons,
        required_confirmation,
    })
}

pub fn enforce_command_confirmation(
    analysis: &CommandAnalysis,
    supplied: Option<&str>,
) -> Result<(), CommandJobError> {
    if analysis.effective_risk == CommandRisk::L0 {
        return Ok(());
    }
    let expected =
        analysis
            .required_confirmation
            .as_deref()
            .ok_or_else(|| CommandJobError::RiskAnalysis {
                message: "risk analysis did not provide required confirmation".to_owned(),
            })?;
    let supplied = supplied.map(str::trim).filter(|value| !value.is_empty());
    match supplied {
        None => Err(CommandJobError::ConfirmationRequired {
            risk: analysis.effective_risk,
            expected: expected.to_owned(),
        }),
        Some(value) if value != expected => Err(CommandJobError::ConfirmationMismatch {
            risk: analysis.effective_risk,
            expected: expected.to_owned(),
        }),
        Some(_) => Ok(()),
    }
}

#[derive(Clone)]
pub struct CommandJobRegistry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    entries: Mutex<HashMap<String, RegistryEntry>>,
    semaphore: Arc<Semaphore>,
    events: broadcast::Sender<CommandEvent>,
    next_sequence: Mutex<u64>,
    host_operations: HostOperationBarrier,
}

struct RegistryEntry {
    job: CommandJob,
    sequence: u64,
    cancel: watch::Sender<bool>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

#[derive(Debug, Clone, Copy)]
enum OutputStream {
    Stdout,
    Stderr,
}

enum PipeMessage {
    Data(OutputStream, Vec<u8>),
    Error(OutputStream, String),
}

impl CommandJobRegistry {
    pub fn new(max_concurrency: usize) -> Result<Self, CommandJobError> {
        if max_concurrency == 0 || max_concurrency > MAX_CONCURRENT_JOBS {
            return invalid("max concurrency must be between 1 and 16");
        }
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        Ok(Self {
            inner: Arc::new(RegistryInner {
                entries: Mutex::new(HashMap::new()),
                semaphore: Arc::new(Semaphore::new(max_concurrency)),
                events,
                next_sequence: Mutex::new(0),
                host_operations: HostOperationBarrier::default(),
            }),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CommandEvent> {
        self.inner.events.subscribe()
    }

    #[cfg(test)]
    pub fn job(&self, job_id: &str) -> Result<CommandJob, CommandJobError> {
        lock(&self.inner.entries)
            .get(job_id)
            .map(|entry| entry.job.clone())
            .ok_or_else(|| CommandJobError::UnknownJob {
                job_id: job_id.to_owned(),
            })
    }

    pub fn list(&self, host_id: Option<&str>) -> Vec<CommandJob> {
        let mut jobs = lock(&self.inner.entries)
            .values()
            .filter(|entry| host_id.is_none_or(|id| entry.job.host_id == id))
            .map(|entry| (entry.sequence, entry.job.clone()))
            .collect::<Vec<_>>();
        jobs.sort_by_key(|(sequence, _)| *sequence);
        jobs.into_iter().map(|(_, job)| job).collect()
    }

    /// Runs a command that the caller resolved from an immutable built-in or
    /// another trusted repository source. Confirmation and risk checks are
    /// still repeated immediately before process creation.
    pub fn run_resolved(
        &self,
        ssh: &SshRuntime,
        resolved: ResolvedCommand,
        confirmation: Option<&str>,
    ) -> Result<CommandJob, CommandJobError> {
        let _operation = self.begin_host_operation(&resolved.host.id)?;
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(CommandJobError::RuntimeUnavailable);
        }
        let prepared = prepare_run(resolved, confirmation)?;
        let (program, args) = ssh
            .remote_command_spec(&prepared.host, prepared.display_command.clone(), true)
            .map_err(|error| CommandJobError::Process {
                message: error.to_string(),
            })?;
        self.start_process(prepared, program, args)
    }

    /// Records a successfully opened PTY hand-off as a completed dispatch job.
    /// The interactive process itself is owned and cancelled by TerminalRegistry.
    pub fn record_pty_handoff(
        &self,
        plan: &PtyCommandPlan,
        session_id: &str,
    ) -> Result<CommandJob, CommandJobError> {
        let _operation = self.begin_host_operation(&plan.host_id)?;
        let now = Utc::now();
        let job = CommandJob {
            id: Uuid::new_v4().to_string(),
            command_id: plan.command_id.clone(),
            host_id: plan.host_id.clone(),
            name: plan.name.clone(),
            command: plan.command.clone(),
            risk: plan.risk,
            state: CommandJobState::Completed,
            stdout: format!("Opened interactive terminal session {session_id}."),
            stderr: String::new(),
            exit_code: Some(0),
            error: None,
            started_at: Some(now),
            finished_at: Some(now),
        };
        let (cancel, _) = watch::channel(false);
        let mut entries = lock(&self.inner.entries);
        prune_terminal_jobs(&mut entries, MAX_RETAINED_JOBS);
        let sequence = next_sequence(&self.inner);
        entries.insert(
            job.id.clone(),
            RegistryEntry {
                job: job.clone(),
                sequence,
                cancel,
                stdout_truncated: false,
                stderr_truncated: false,
            },
        );
        drop(entries);
        self.emit(job.clone());
        Ok(job)
    }

    /// Requests cancellation only through the channel paired with this job's
    /// owned `Child`.  No PID lookup, port scan, or system-wide kill is used.
    pub fn cancel(&self, job_id: &str) -> Result<CommandJob, CommandJobError> {
        let (job, sender) = {
            let mut entries = lock(&self.inner.entries);
            let entry = entries
                .get_mut(job_id)
                .ok_or_else(|| CommandJobError::UnknownJob {
                    job_id: job_id.to_owned(),
                })?;
            if entry.job.state.is_terminal() {
                return Ok(entry.job.clone());
            }
            if entry.job.state == CommandJobState::Cancelling {
                return Ok(entry.job.clone());
            }
            if entry.job.state == CommandJobState::Queued {
                entry.job.state = CommandJobState::Cancelled;
                entry.job.finished_at = Some(Utc::now());
            } else {
                entry.job.state = CommandJobState::Cancelling;
            }
            (entry.job.clone(), entry.cancel.clone())
        };
        let _ = sender.send(true);
        self.emit(job.clone());
        Ok(job)
    }

    pub async fn shutdown(&self) {
        let ids = self
            .list(None)
            .into_iter()
            .filter(|job| !job.state.is_terminal())
            .map(|job| job.id)
            .collect::<Vec<_>>();
        for id in ids {
            let _ = self.cancel(&id);
        }
        let _ = tokio::time::timeout(COMMAND_SHUTDOWN_TIMEOUT, async {
            loop {
                if self
                    .inner
                    .entries
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .values()
                    .all(|entry| entry.job.state.is_terminal())
                {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await;
    }

    fn begin_host_operation(&self, host_id: &str) -> Result<HostOperationLease, CommandJobError> {
        self.inner
            .host_operations
            .begin(host_id)
            .ok_or_else(|| CommandJobError::Process {
                message: format!("command operations for host {host_id} are being retired"),
            })
    }

    /// Retires a host before repository deletion. A start that already
    /// resolved the host must register (or fail) before the stable job set is
    /// cancelled and purged, so no late child can escape the cleanup scan.
    pub async fn retire_host(&self, host_id: &str) -> Result<(), CommandJobError> {
        self.inner.host_operations.retire(host_id);
        tokio::time::timeout(
            HOST_RETIREMENT_TIMEOUT,
            self.inner.host_operations.wait_idle(host_id),
        )
        .await
        .map_err(|_| CommandJobError::Process {
            message: "timed out waiting for command startup operations to settle".to_owned(),
        })?;

        let ids = self
            .list(Some(host_id))
            .into_iter()
            .filter(|job| !job.state.is_terminal())
            .map(|job| job.id)
            .collect::<Vec<_>>();
        for id in &ids {
            let _ = self.cancel(id);
        }
        tokio::time::timeout(HOST_RETIREMENT_TIMEOUT, async {
            loop {
                if lock(&self.inner.entries)
                    .values()
                    .filter(|entry| entry.job.host_id == host_id)
                    .all(|entry| entry.job.state.is_terminal())
                {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .map_err(|_| CommandJobError::Process {
            message: "timed out waiting for host command jobs to stop".to_owned(),
        })?;
        lock(&self.inner.entries).retain(|_, entry| entry.job.host_id != host_id);
        Ok(())
    }

    pub fn restore_host(&self, host_id: &str) {
        self.inner.host_operations.restore(host_id);
    }

    fn start_process(
        &self,
        prepared: PreparedCommand,
        program: PathBuf,
        args: Vec<OsString>,
    ) -> Result<CommandJob, CommandJobError> {
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(CommandJobError::RuntimeUnavailable);
        }
        let id = Uuid::new_v4().to_string();
        let job = CommandJob {
            id: id.clone(),
            command_id: prepared.command_id.clone(),
            host_id: prepared.host.id.clone(),
            name: prepared.name.clone(),
            command: prepared.display_command.clone(),
            risk: prepared.risk,
            state: CommandJobState::Queued,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            error: None,
            started_at: None,
            finished_at: None,
        };
        let (cancel, cancel_rx) = watch::channel(false);
        let mut entries = lock(&self.inner.entries);
        prune_terminal_jobs(&mut entries, MAX_RETAINED_JOBS);
        if entries
            .values()
            .filter(|entry| !entry.job.state.is_terminal())
            .count()
            >= MAX_PENDING_JOBS
        {
            return Err(CommandJobError::Process {
                message: format!(
                    "command queue is full; at most {MAX_PENDING_JOBS} active or queued jobs are allowed"
                ),
            });
        }
        let sequence = next_sequence(&self.inner);
        entries.insert(
            id.clone(),
            RegistryEntry {
                job: job.clone(),
                sequence,
                cancel,
                stdout_truncated: false,
                stderr_truncated: false,
            },
        );
        drop(entries);
        self.emit(job.clone());
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            execute_job(inner, id, program, args, cancel_rx).await;
        });
        Ok(job)
    }

    fn emit(&self, job: CommandJob) {
        let _ = self.inner.events.send(CommandEvent { job });
    }
}

fn next_sequence(inner: &RegistryInner) -> u64 {
    let mut next = lock(&inner.next_sequence);
    let value = *next;
    *next = next.saturating_add(1);
    value
}

fn prune_terminal_jobs(entries: &mut HashMap<String, RegistryEntry>, limit: usize) {
    while entries.len() >= limit || retained_output_bytes(entries) > MAX_RETAINED_OUTPUT_BYTES {
        let oldest = entries
            .iter()
            .filter(|(_, entry)| entry.job.state.is_terminal())
            .min_by_key(|(_, entry)| entry.sequence)
            .map(|(id, _)| id.clone());
        let Some(oldest) = oldest else {
            break;
        };
        entries.remove(&oldest);
    }
}

fn retained_output_bytes(entries: &HashMap<String, RegistryEntry>) -> usize {
    entries
        .values()
        .filter(|entry| entry.job.state.is_terminal())
        .fold(0_usize, |total, entry| {
            total
                .saturating_add(entry.job.stdout.len())
                .saturating_add(entry.job.stderr.len())
                .saturating_add(entry.job.error.as_ref().map_or(0, String::len))
        })
}

fn prepare_run(
    resolved: ResolvedCommand,
    supplied_confirmation: Option<&str>,
) -> Result<PreparedCommand, CommandJobError> {
    // Always perform a fresh Rust-side check for run, even if analyze was just
    // called for the same request.
    let analysis = analyze_resolved_command(&resolved)?;
    enforce_command_confirmation(&analysis, supplied_confirmation)?;
    if resolved.requires_pty {
        return Err(CommandJobError::RequiresPty {
            plan: Box::new(PtyCommandPlan {
                host_id: resolved.host.id.clone(),
                command_id: resolved.command_id.clone(),
                name: resolved.name.clone(),
                command: analysis.display_command.clone(),
                working_directory: resolved.working_directory.clone(),
                risk: analysis.effective_risk,
            }),
        });
    }
    Ok(PreparedCommand {
        host: resolved.host,
        command_id: resolved.command_id,
        name: resolved.name,
        display_command: analysis.display_command,
        risk: analysis.effective_risk,
    })
}

async fn execute_job(
    inner: Arc<RegistryInner>,
    job_id: String,
    program: PathBuf,
    args: Vec<OsString>,
    mut cancel_rx: watch::Receiver<bool>,
) {
    if *cancel_rx.borrow() {
        finish_cancelled(&inner, &job_id);
        return;
    }
    let permit = tokio::select! {
        biased;
        changed = cancel_rx.changed() => {
            if changed.is_err() || *cancel_rx.borrow() {
                finish_cancelled(&inner, &job_id);
                return;
            }
            return;
        }
        permit = Arc::clone(&inner.semaphore).acquire_owned() => match permit {
            Ok(permit) => permit,
            Err(_) => {
                finish_failed(&inner, &job_id, "command scheduler closed".to_owned(), None);
                return;
            }
        }
    };

    if !transition_running(&inner, &job_id) {
        drop(permit);
        return;
    }

    let mut process = Command::new(program);
    process
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(error) => {
            finish_failed(&inner, &job_id, error.to_string(), None);
            return;
        }
    };

    let (pipe_tx, mut pipe_rx) = mpsc::channel(32);
    if let Some(stdout) = child.stdout.take() {
        spawn_pipe_reader(stdout, OutputStream::Stdout, pipe_tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_pipe_reader(stderr, OutputStream::Stderr, pipe_tx.clone());
    }
    drop(pipe_tx);

    let mut status: Option<Result<ExitStatus, std::io::Error>> = None;
    let mut pipes_open = true;
    let mut cancellation_seen = false;
    let mut cancellation_deadline = None;
    let mut cancellation_failure: Option<String> = None;
    let mut pipe_error: Option<String> = None;

    while status.is_none() || pipes_open {
        tokio::select! {
            biased;
            changed = cancel_rx.changed(), if !cancellation_seen => {
                if changed.is_err() {
                    cancellation_seen = true;
                } else if *cancel_rx.borrow() {
                    cancellation_seen = true;
                    if let Err(error) = child.start_kill() {
                        cancellation_failure = Some(format!(
                            "failed to terminate the owned command process: {error}"
                        ));
                    }
                }
                cancellation_deadline = Some(
                    tokio::time::Instant::now() + COMMAND_CANCEL_TIMEOUT
                );
            }
            result = child.wait(), if status.is_none() => {
                status = Some(result);
            }
            message = pipe_rx.recv(), if pipes_open => {
                match message {
                    Some(PipeMessage::Data(stream, data)) => append_job_output(&inner, &job_id, stream, &data),
                    Some(PipeMessage::Error(stream, message)) => {
                        pipe_error.get_or_insert_with(|| format!("{} reader failed: {message}", stream_name(stream)));
                        if let Err(error) = child.start_kill() {
                            cancellation_failure.get_or_insert_with(|| format!(
                                "failed to terminate after a pipe error: {error}"
                            ));
                        }
                        cancellation_deadline = Some(
                            tokio::time::Instant::now() + COMMAND_CANCEL_TIMEOUT
                        );
                    }
                    None => pipes_open = false,
                }
            }
            _ = wait_for_command_cancel_deadline(cancellation_deadline), if cancellation_seen || pipe_error.is_some() => {
                if status.is_none() {
                    let _ = child.start_kill();
                }
                if cancellation_seen {
                    cancellation_failure.get_or_insert_with(|| {
                        "owned command process or output pipes did not close within 5 seconds"
                            .to_owned()
                    });
                } else {
                    pipe_error.get_or_insert_with(|| {
                        "owned command process did not exit after an output-pipe failure"
                            .to_owned()
                    });
                }
                break;
            }
        }
    }

    drop(permit);
    if cancellation_seen || job_cancellation_requested(&inner, &job_id) {
        if let Some(error) = cancellation_failure {
            finish_failed(&inner, &job_id, error, exit_code(&status));
        } else {
            finish_cancelled(&inner, &job_id);
        }
        return;
    }
    if let Some(error) = pipe_error {
        finish_failed(&inner, &job_id, error, exit_code(&status));
        return;
    }
    match status {
        Some(Ok(exit)) if exit.success() => finish_completed(&inner, &job_id, exit.code()),
        Some(Ok(exit)) => finish_failed(
            &inner,
            &job_id,
            format!(
                "remote command exited with code {}",
                exit.code()
                    .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
            ),
            exit.code(),
        ),
        Some(Err(error)) => finish_failed(&inner, &job_id, error.to_string(), None),
        None => finish_failed(
            &inner,
            &job_id,
            "command process ended without an exit status".to_owned(),
            None,
        ),
    }
}

async fn wait_for_command_cancel_deadline(deadline: Option<tokio::time::Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(deadline).await;
    } else {
        std::future::pending::<()>().await;
    }
}

fn spawn_pipe_reader<R>(mut reader: R, stream: OutputStream, sender: mpsc::Sender<PipeMessage>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => return,
                Ok(count) => {
                    if sender
                        .send(PipeMessage::Data(stream, buffer[..count].to_vec()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender
                        .send(PipeMessage::Error(stream, error.to_string()))
                        .await;
                    return;
                }
            }
        }
    });
}

fn transition_running(inner: &RegistryInner, job_id: &str) -> bool {
    let job = {
        let mut entries = lock(&inner.entries);
        let Some(entry) = entries.get_mut(job_id) else {
            return false;
        };
        if entry.job.state != CommandJobState::Queued {
            return false;
        }
        entry.job.state = CommandJobState::Running;
        entry.job.started_at = Some(Utc::now());
        entry.job.clone()
    };
    let _ = inner.events.send(CommandEvent { job });
    true
}

fn append_job_output(inner: &RegistryInner, job_id: &str, stream: OutputStream, data: &[u8]) {
    let job = {
        let mut entries = lock(&inner.entries);
        let Some(entry) = entries.get_mut(job_id) else {
            return;
        };
        if !matches!(
            entry.job.state,
            CommandJobState::Running | CommandJobState::Cancelling
        ) {
            return;
        }
        let (target, truncated) = match stream {
            OutputStream::Stdout => (&mut entry.job.stdout, &mut entry.stdout_truncated),
            OutputStream::Stderr => (&mut entry.job.stderr, &mut entry.stderr_truncated),
        };
        if !append_bounded(target, truncated, data) {
            return;
        }
        entry.job.clone()
    };
    let _ = inner.events.send(CommandEvent { job });
}

fn append_bounded(target: &mut String, truncated: &mut bool, data: &[u8]) -> bool {
    if data.is_empty() || *truncated {
        return false;
    }
    let value = String::from_utf8_lossy(data);
    if target.len().saturating_add(value.len()) <= COMMAND_OUTPUT_LIMIT {
        target.push_str(&value);
        return true;
    }

    let content_limit = COMMAND_OUTPUT_LIMIT.saturating_sub(OUTPUT_TRUNCATED_MARKER.len());
    truncate_utf8(target, content_limit);
    let remaining = content_limit.saturating_sub(target.len());
    if remaining > 0 {
        let mut end = remaining.min(value.len());
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        target.push_str(&value[..end]);
    }
    target.push_str(OUTPUT_TRUNCATED_MARKER);
    *truncated = true;
    true
}

fn truncate_utf8(value: &mut String, max_bytes: usize) {
    let mut end = value.len().min(max_bytes);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
}

fn finish_completed(inner: &RegistryInner, job_id: &str, exit_code: Option<i32>) {
    finish_job(inner, job_id, CommandJobState::Completed, exit_code, None);
}

fn finish_failed(inner: &RegistryInner, job_id: &str, error: String, exit_code: Option<i32>) {
    finish_job(
        inner,
        job_id,
        CommandJobState::Failed,
        exit_code,
        Some(bounded_error(error)),
    );
}

fn finish_cancelled(inner: &RegistryInner, job_id: &str) {
    let job = {
        let mut entries = lock(&inner.entries);
        let Some(entry) = entries.get_mut(job_id) else {
            return;
        };
        if entry.job.state != CommandJobState::Cancelled {
            entry.job.state = CommandJobState::Cancelled;
        }
        if entry.job.finished_at.is_none() {
            entry.job.finished_at = Some(Utc::now());
        }
        let job = entry.job.clone();
        prune_terminal_jobs(&mut entries, MAX_RETAINED_JOBS);
        job
    };
    let _ = inner.events.send(CommandEvent { job });
}

fn finish_job(
    inner: &RegistryInner,
    job_id: &str,
    state: CommandJobState,
    exit_code: Option<i32>,
    error: Option<String>,
) {
    let job = {
        let mut entries = lock(&inner.entries);
        let Some(entry) = entries.get_mut(job_id) else {
            return;
        };
        if !matches!(
            entry.job.state,
            CommandJobState::Running | CommandJobState::Cancelling
        ) {
            return;
        }
        entry.job.state = state;
        entry.job.exit_code = exit_code;
        entry.job.error = error;
        entry.job.finished_at = Some(Utc::now());
        let job = entry.job.clone();
        prune_terminal_jobs(&mut entries, MAX_RETAINED_JOBS);
        job
    };
    let _ = inner.events.send(CommandEvent { job });
}

fn job_cancellation_requested(inner: &RegistryInner, job_id: &str) -> bool {
    lock(&inner.entries).get(job_id).is_some_and(|entry| {
        matches!(
            entry.job.state,
            CommandJobState::Cancelling | CommandJobState::Cancelled
        )
    })
}

fn exit_code(status: &Option<Result<ExitStatus, std::io::Error>>) -> Option<i32> {
    status.as_ref().and_then(|result| match result {
        Ok(status) => status.code(),
        Err(_) => None,
    })
}

fn stream_name(stream: OutputStream) -> &'static str {
    match stream {
        OutputStream::Stdout => "stdout",
        OutputStream::Stderr => "stderr",
    }
}

fn bounded_error(mut value: String) -> String {
    truncate_utf8(&mut value, 4096);
    value
}

fn validate_request_shape(request: &CommandRunRequest) -> Result<(), CommandJobError> {
    if Uuid::parse_str(request.host_id.trim()).is_err() {
        return invalid("hostId must be a UUID");
    }
    if let Some(command_id) = request.command_id.as_deref() {
        let command_id = command_id.trim();
        let known_builtin = command_id
            .strip_prefix("builtin:")
            .is_some_and(|id| risk::builtin_preset(id).is_some());
        if Uuid::parse_str(command_id).is_err() && !known_builtin {
            return invalid("commandId must be a UUID or a known built-in id");
        }
    }
    if request.command_id.is_some() && request.command.is_some() {
        return invalid("commandId and command are mutually exclusive");
    }
    if request.command_id.is_none() && request.command.is_none() {
        return invalid("either commandId or command is required");
    }
    if let Some(confirmation) = request.confirmation.as_deref()
        && (confirmation.chars().count() > MAX_CONFIRMATION_CHARS
            || confirmation.chars().any(char::is_control))
    {
        return invalid("confirmation is invalid");
    }
    Ok(())
}

fn trusted_preset_risk(preset: &CommandPreset) -> Result<CommandRisk, CommandJobError> {
    let Some(id) = preset.id.strip_prefix("builtin:") else {
        return Ok(preset.risk.max(CommandRisk::L1));
    };
    let builtin = risk::builtin_preset(id).ok_or_else(|| CommandJobError::InvalidRequest {
        message: "unknown built-in command preset".to_owned(),
    })?;
    let expected_risk = command_risk(builtin.declared_risk);
    if preset.host_id.is_some()
        || preset.command != builtin.command
        || preset.risk != expected_risk
        || preset.requires_pty != builtin.requires_pty
        || preset.requires_sudo != builtin.requires_sudo
        || preset.working_directory.is_some()
        || preset.confirmation_text.is_some()
    {
        return invalid("built-in command preset does not match its immutable definition");
    }
    Ok(expected_risk)
}

fn validate_command_text(command: &str) -> Result<(), CommandJobError> {
    if command.trim().is_empty() || command.len() > MAX_COMMAND_BYTES || command.contains('\0') {
        return invalid("command must contain 1-32768 bytes and no NUL");
    }
    Ok(())
}

fn normalize_directory(value: Option<String>) -> Result<Option<String>, CommandJobError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > MAX_WORKING_DIRECTORY_BYTES || value.contains('\0') {
        return invalid("workingDirectory is invalid");
    }
    Ok(Some(value.to_owned()))
}

fn normalize_confirmation(value: Option<String>) -> Result<Option<String>, CommandJobError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > MAX_CONFIRMATION_CHARS || value.chars().any(char::is_control) {
        return invalid("preset confirmation text is invalid");
    }
    Ok(Some(value.to_owned()))
}

fn in_directory(command: &str, working_directory: Option<&str>) -> String {
    match working_directory.map(str::trim) {
        None | Some("") | Some("~") | Some("~/") => command.to_owned(),
        Some(directory) => format!("cd -- {} && {command}", shell_path(directory)),
    }
}

fn shell_path(value: &str) -> String {
    if let Some(relative) = value.strip_prefix("~/") {
        format!("\"$HOME\"/{}", posix_quote(relative))
    } else {
        posix_quote(value)
    }
}

fn posix_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn risk_level(value: CommandRisk) -> RiskLevel {
    match value {
        CommandRisk::L0 => RiskLevel::L0,
        CommandRisk::L1 => RiskLevel::L1,
        CommandRisk::L2 => RiskLevel::L2,
    }
}

fn command_risk(value: RiskLevel) -> CommandRisk {
    match value {
        RiskLevel::L0 => CommandRisk::L0,
        RiskLevel::L1 => CommandRisk::L1,
        RiskLevel::L2 => CommandRisk::L2,
    }
}

fn invalid<T>(message: impl Into<String>) -> Result<T, CommandJobError> {
    Err(CommandJobError::InvalidRequest {
        message: message.into(),
    })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SshAdvancedOptions;
    use std::time::Duration;

    fn host(alias: &str) -> HostProfile {
        let now = Utc::now();
        HostProfile {
            schema_version: 2,
            id: "11111111-1111-4111-8111-111111111111".to_owned(),
            alias: alias.to_owned(),
            hostname: "127.0.0.1".to_owned(),
            port: 22,
            username: "tester".to_owned(),
            auth_method: crate::model::AuthMethod::Interactive,
            identity_file: None,
            proxy_jump: None,
            default_workspace: "~".to_owned(),
            groups: Vec::new(),
            advanced: SshAdvancedOptions::default(),
            monitor_enabled: true,
            created_at: now,
            updated_at: now,
        }
    }

    fn preset(risk: CommandRisk, command: &str) -> CommandPreset {
        let now = Utc::now();
        CommandPreset {
            schema_version: 2,
            id: "22222222-2222-4222-8222-222222222222".to_owned(),
            host_id: None,
            name: "Test preset".to_owned(),
            description: String::new(),
            group: "Tests".to_owned(),
            command: command.to_owned(),
            working_directory: None,
            risk,
            requires_pty: false,
            requires_sudo: false,
            confirmation_text: None,
            sort_order: 0,
            created_at: now,
            updated_at: now,
        }
    }

    fn request(command_id: bool) -> CommandRunRequest {
        CommandRunRequest {
            host_id: "11111111-1111-4111-8111-111111111111".to_owned(),
            command_id: command_id.then(|| "22222222-2222-4222-8222-222222222222".to_owned()),
            command: (!command_id).then(|| "uptime".to_owned()),
            working_directory: None,
            confirmation: None,
        }
    }

    fn prepared(host_id: &str, command: &str) -> PreparedCommand {
        let mut host = host("test-host");
        host.id = host_id.to_owned();
        PreparedCommand {
            host,
            command_id: None,
            name: "Process test".to_owned(),
            display_command: command.to_owned(),
            risk: CommandRisk::L0,
        }
    }

    #[test]
    fn pure_resolution_rejects_renderer_override_and_scoped_mismatch() {
        let mut overridden = request(true);
        overridden.command = Some("rm -rf /".to_owned());
        assert!(matches!(
            resolve_command_request(
                host("lab"),
                Some(preset(CommandRisk::L0, "uptime")),
                &overridden
            ),
            Err(CommandJobError::InvalidRequest { .. })
        ));

        let mut scoped = preset(CommandRisk::L0, "uptime");
        scoped.host_id = Some("33333333-3333-4333-8333-333333333333".to_owned());
        assert!(matches!(
            resolve_command_request(host("lab"), Some(scoped), &request(true)),
            Err(CommandJobError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn l1_requires_explicit_host_alias_confirmation() {
        let resolved = resolve_command_request(
            host("gpu-lab"),
            Some(preset(CommandRisk::L1, "systemctl restart demo")),
            &request(true),
        )
        .expect("resolve");
        let analysis = analyze_resolved_command(&resolved).expect("analyze");
        assert_eq!(analysis.effective_risk, CommandRisk::L1);
        assert_eq!(analysis.required_confirmation.as_deref(), Some("gpu-lab"));
        assert!(matches!(
            enforce_command_confirmation(&analysis, None),
            Err(CommandJobError::ConfirmationRequired { .. })
        ));
        enforce_command_confirmation(&analysis, Some("gpu-lab")).expect("confirm");
    }

    #[test]
    fn l2_requires_exact_preset_text_and_sudo_is_rechecked() {
        let mut value = preset(CommandRisk::L0, "printf safe");
        value.requires_sudo = true;
        value.confirmation_text = Some("TYPE EXACTLY".to_owned());
        let resolved =
            resolve_command_request(host("lab"), Some(value), &request(true)).expect("resolve");
        let analysis = analyze_resolved_command(&resolved).expect("analyze");
        assert_eq!(analysis.effective_risk, CommandRisk::L2);
        assert_eq!(
            analysis.required_confirmation.as_deref(),
            Some("TYPE EXACTLY")
        );
        assert!(enforce_command_confirmation(&analysis, Some("type exactly")).is_err());
        enforce_command_confirmation(&analysis, Some("TYPE EXACTLY")).expect("confirm");
    }

    #[test]
    fn pty_presets_return_a_terminal_handoff_plan() {
        let mut value = preset(CommandRisk::L0, "btop");
        value.requires_pty = true;
        let resolved =
            resolve_command_request(host("lab"), Some(value), &request(true)).expect("resolve");
        let error = prepare_run(resolved, Some("lab")).expect_err("requires pty");
        match error {
            CommandJobError::RequiresPty { plan } => {
                assert_eq!(plan.command, "btop");
                assert_eq!(plan.host_id, request(true).host_id);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn ad_hoc_and_custom_read_only_commands_still_require_acknowledgement() {
        let ad_hoc = resolve_command_request(host("lab"), None, &request(false)).expect("ad hoc");
        let ad_hoc_analysis = analyze_resolved_command(&ad_hoc).expect("analyze ad hoc");
        assert_eq!(ad_hoc_analysis.effective_risk, CommandRisk::L1);
        assert_eq!(
            ad_hoc_analysis.required_confirmation.as_deref(),
            Some("lab")
        );

        let custom = resolve_command_request(
            host("lab"),
            Some(preset(CommandRisk::L0, "uptime")),
            &request(true),
        )
        .expect("custom");
        assert_eq!(
            analyze_resolved_command(&custom)
                .expect("analyze custom")
                .effective_risk,
            CommandRisk::L1
        );
    }

    #[test]
    fn immutable_builtin_can_remain_l0_but_spoofs_are_rejected() {
        let builtin = risk::builtin_preset("system-overview").expect("builtin");
        let now = Utc::now();
        let mut preset = CommandPreset {
            schema_version: 2,
            id: "builtin:system-overview".to_owned(),
            host_id: None,
            name: builtin.name.to_owned(),
            description: String::new(),
            group: builtin.group.to_owned(),
            command: builtin.command.to_owned(),
            working_directory: None,
            risk: command_risk(builtin.declared_risk),
            requires_pty: builtin.requires_pty,
            requires_sudo: builtin.requires_sudo,
            confirmation_text: None,
            sort_order: 0,
            created_at: now,
            updated_at: now,
        };
        let builtin_request = CommandRunRequest {
            host_id: host("lab").id,
            command_id: Some(preset.id.clone()),
            command: None,
            working_directory: None,
            confirmation: None,
        };
        let resolved = resolve_command_request(host("lab"), Some(preset.clone()), &builtin_request)
            .expect("resolve builtin");
        assert_eq!(
            analyze_resolved_command(&resolved)
                .expect("analyze builtin")
                .effective_risk,
            CommandRisk::L0
        );

        preset.command = "rm -rf /".to_owned();
        assert!(resolve_command_request(host("lab"), Some(preset), &builtin_request).is_err());
    }

    #[test]
    fn bounded_append_preserves_utf8_and_marker_within_limit() {
        let mut output = "界".repeat(COMMAND_OUTPUT_LIMIT / 3);
        let mut truncated = false;
        assert!(append_bounded(
            &mut output,
            &mut truncated,
            "更多".as_bytes()
        ));
        assert!(truncated);
        assert!(output.is_char_boundary(output.len()));
        assert!(output.len() <= COMMAND_OUTPUT_LIMIT);
        assert!(output.ends_with(OUTPUT_TRUNCATED_MARKER));
        let snapshot = output.clone();
        assert!(!append_bounded(&mut output, &mut truncated, b"ignored"));
        assert_eq!(output, snapshot);
    }

    #[tokio::test]
    async fn registry_bounds_stdout_and_stderr_independently() {
        let registry = CommandJobRegistry::new(1).expect("registry");
        let (program, args) = large_output_process();
        let job = registry
            .start_process(
                prepared("44444444-4444-4444-8444-444444444444", "large-output"),
                program,
                args,
            )
            .expect("start");
        let finished = wait_for_terminal(&registry, &job.id).await;
        assert_eq!(finished.state, CommandJobState::Completed);
        assert!(finished.stdout.len() <= COMMAND_OUTPUT_LIMIT);
        assert!(finished.stderr.len() <= COMMAND_OUTPUT_LIMIT);
        assert!(finished.stdout.ends_with(OUTPUT_TRUNCATED_MARKER));
        assert!(finished.stderr.ends_with(OUTPUT_TRUNCATED_MARKER));
        assert!(finished.started_at.is_some());
        assert!(finished.finished_at.is_some());
    }

    #[tokio::test]
    async fn event_stream_reports_state_machine_and_nonzero_exit_fails() {
        let registry = CommandJobRegistry::new(1).expect("registry");
        let mut events = registry.subscribe();
        let (program, args) = failing_process();
        let job = registry
            .start_process(
                prepared("99999999-9999-4999-8999-999999999999", "failing-command"),
                program,
                args,
            )
            .expect("start");
        let finished = wait_for_terminal(&registry, &job.id).await;
        assert_eq!(finished.state, CommandJobState::Failed);
        assert_eq!(finished.exit_code, Some(7));
        assert!(
            finished
                .error
                .as_deref()
                .is_some_and(|value| value.contains('7'))
        );

        let mut states = Vec::new();
        while let Ok(event) = events.try_recv() {
            if event.job.id == job.id {
                states.push(event.job.state);
            }
        }
        assert_eq!(states.first(), Some(&CommandJobState::Queued));
        assert!(states.contains(&CommandJobState::Running));
        assert_eq!(states.last(), Some(&CommandJobState::Failed));
    }

    #[tokio::test]
    async fn cancellation_targets_the_registered_job_and_reaches_cancelled() {
        let registry = CommandJobRegistry::new(2).expect("registry");
        let (program_a, args_a) = sleeping_process();
        let (program_b, args_b) = successful_process();
        let first = registry
            .start_process(
                prepared("55555555-5555-4555-8555-555555555555", "sleep"),
                program_a,
                args_a,
            )
            .expect("start first");
        let second = registry
            .start_process(
                prepared("66666666-6666-4666-8666-666666666666", "success"),
                program_b,
                args_b,
            )
            .expect("start second");
        wait_for_state(&registry, &first.id, CommandJobState::Running).await;
        let cancelled = registry.cancel(&first.id).expect("cancel");
        assert_eq!(cancelled.state, CommandJobState::Cancelling);
        assert!(cancelled.finished_at.is_none());
        let completed = wait_for_terminal(&registry, &second.id).await;
        assert_eq!(completed.state, CommandJobState::Completed);
        let cancelled = wait_for_finished_cancelled(&registry, &first.id).await;
        assert_eq!(cancelled.state, CommandJobState::Cancelled);
        assert!(cancelled.started_at.is_some());
        assert!(cancelled.finished_at.is_some());
    }

    #[tokio::test]
    async fn semaphore_queues_jobs_and_list_filters_by_host() {
        let registry = CommandJobRegistry::new(1).expect("registry");
        let (program_a, args_a) = sleeping_process();
        let (program_b, args_b) = sleeping_process();
        let first = registry
            .start_process(
                prepared("77777777-7777-4777-8777-777777777777", "sleep-a"),
                program_a,
                args_a,
            )
            .expect("first");
        let second = registry
            .start_process(
                prepared("88888888-8888-4888-8888-888888888888", "sleep-b"),
                program_b,
                args_b,
            )
            .expect("second");
        wait_for_state(&registry, &first.id, CommandJobState::Running).await;
        assert_eq!(
            registry.job(&second.id).expect("second").state,
            CommandJobState::Queued
        );
        assert_eq!(
            registry
                .list(Some("77777777-7777-4777-8777-777777777777"))
                .len(),
            1
        );
        assert_eq!(
            registry
                .list(Some("88888888-8888-4888-8888-888888888888"))
                .len(),
            1
        );
        registry.cancel(&first.id).expect("cancel first");
        registry.cancel(&second.id).expect("cancel second");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    async fn wait_for_state(
        registry: &CommandJobRegistry,
        job_id: &str,
        expected: CommandJobState,
    ) -> CommandJob {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let job = registry.job(job_id).expect("job");
                if job.state == expected {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("state timeout")
    }

    async fn wait_for_terminal(registry: &CommandJobRegistry, job_id: &str) -> CommandJob {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let job = registry.job(job_id).expect("job");
                if job.state.is_terminal() {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("job timeout")
    }

    async fn wait_for_finished_cancelled(
        registry: &CommandJobRegistry,
        job_id: &str,
    ) -> CommandJob {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let job = registry.job(job_id).expect("job");
                if job.state == CommandJobState::Cancelled && job.finished_at.is_some() {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancel timeout")
    }

    #[cfg(windows)]
    fn successful_process() -> (PathBuf, Vec<OsString>) {
        (
            PathBuf::from("powershell.exe"),
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "[Console]::Out.Write('ok')".into(),
            ],
        )
    }

    #[cfg(not(windows))]
    fn successful_process() -> (PathBuf, Vec<OsString>) {
        (
            PathBuf::from("/bin/sh"),
            vec!["-c".into(), "printf ok".into()],
        )
    }

    #[cfg(windows)]
    fn failing_process() -> (PathBuf, Vec<OsString>) {
        (
            PathBuf::from("powershell.exe"),
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "[Console]::Error.Write('expected failure'); exit 7".into(),
            ],
        )
    }

    #[cfg(not(windows))]
    fn failing_process() -> (PathBuf, Vec<OsString>) {
        (
            PathBuf::from("/bin/sh"),
            vec!["-c".into(), "printf 'expected failure' >&2; exit 7".into()],
        )
    }

    #[cfg(windows)]
    fn sleeping_process() -> (PathBuf, Vec<OsString>) {
        (
            PathBuf::from("powershell.exe"),
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "Start-Sleep -Seconds 30".into(),
            ],
        )
    }

    #[cfg(not(windows))]
    fn sleeping_process() -> (PathBuf, Vec<OsString>) {
        (
            PathBuf::from("/bin/sh"),
            vec!["-c".into(), "sleep 30".into()],
        )
    }

    #[cfg(windows)]
    fn large_output_process() -> (PathBuf, Vec<OsString>) {
        (
            PathBuf::from("powershell.exe"),
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "$x='x'*1100000; [Console]::Out.Write($x); [Console]::Error.Write($x)".into(),
            ],
        )
    }

    #[cfg(not(windows))]
    fn large_output_process() -> (PathBuf, Vec<OsString>) {
        (
            PathBuf::from("/bin/sh"),
            vec![
                "-c".into(),
                "head -c 1100000 /dev/zero | tr '\\0' x; head -c 1100000 /dev/zero | tr '\\0' x >&2"
                    .into(),
            ],
        )
    }
}
