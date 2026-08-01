use super::{
    BoundedOutput, ChildCancellation, SshRuntime, captured_text, run_output_with_input,
    run_output_with_input_cancellable,
};
use crate::{
    error::{AppError, AppResult},
    model::HostProfile,
};
use chrono::{DateTime, Datelike, Local, TimeZone, Utc};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Notify, broadcast};
use uuid::Uuid;

const MAX_BATCH_BYTES: usize = 1_048_576;
const MAX_REMOTE_ENTRIES: usize = 10_000;
const MAX_RECURSION_DEPTH: usize = 64;
const MAX_TREE_PATH_BYTES: usize = 8 * 1024 * 1024;
const BASIC_OPERATION_TIMEOUT: Duration = Duration::from_secs(300);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
const TRANSFER_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_ACTIVE_TRANSFERS: usize = 8;
const MAX_RETAINED_TRANSFERS: usize = 512;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SftpEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpEntry {
    pub name: String,
    pub path: String,
    pub kind: SftpEntryKind,
    pub size: u64,
    pub modified_at: Option<String>,
    pub permissions: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpListRequest {
    pub path: String,
    #[serde(default)]
    pub show_hidden: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpListResult {
    pub host_id: String,
    pub path: String,
    pub parent_path: Option<String>,
    pub entries: Vec<SftpEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpMkdirRequest {
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpRenameRequest {
    pub source: String,
    pub destination: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpDeleteRequest {
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SftpOperationResult {
    pub success: bool,
    pub affected: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConflictPolicy {
    Ask,
    Skip,
    Overwrite,
    Rename,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadRequest {
    pub local_path: String,
    pub remote_path: String,
    pub conflict_policy: ConflictPolicy,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadRequest {
    pub remote_path: String,
    pub local_path: String,
    pub conflict_policy: ConflictPolicy,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TransferState {
    Queued,
    Running,
    Cancelling,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferJob {
    pub id: String,
    pub host_id: String,
    pub direction: TransferDirection,
    pub source: String,
    pub destination: String,
    pub conflict_policy: ConflictPolicy,
    pub recursive: bool,
    pub state: TransferState,
    pub attempts: u32,
    pub bytes_transferred: u64,
    pub total_bytes: Option<u64>,
    pub skipped: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferEvent {
    pub job: TransferJob,
}

#[derive(Debug, Clone)]
pub struct SftpService {
    runtime: SshRuntime,
    host_operations: Arc<HostOperationBarrier>,
}

#[derive(Debug, Default)]
struct HostOperationBarrier {
    hosts: Mutex<HashMap<String, HostOperationState>>,
    changed: Notify,
}

#[derive(Debug, Default)]
struct HostOperationState {
    active: usize,
    retired: bool,
}

pub struct SftpHostOperation {
    barrier: Arc<HostOperationBarrier>,
    host_id: String,
}

impl Drop for SftpHostOperation {
    fn drop(&mut self) {
        let mut hosts = self.barrier.hosts.lock();
        if let Some(state) = hosts.get_mut(&self.host_id) {
            state.active = state.active.saturating_sub(1);
            if state.active == 0 && !state.retired {
                hosts.remove(&self.host_id);
            }
        }
        drop(hosts);
        self.barrier.changed.notify_waiters();
    }
}

impl SftpService {
    pub fn new(runtime: SshRuntime) -> Self {
        Self {
            runtime,
            host_operations: Arc::default(),
        }
    }

    pub fn begin_host_operation(&self, host_id: &str) -> AppResult<SftpHostOperation> {
        let mut hosts = self.host_operations.hosts.lock();
        let state = hosts.entry(host_id.to_owned()).or_default();
        if state.retired {
            return Err(AppError::State(format!(
                "SFTP operations for host {host_id} are being retired"
            )));
        }
        state.active = state.active.saturating_add(1);
        Ok(SftpHostOperation {
            barrier: self.host_operations.clone(),
            host_id: host_id.to_owned(),
        })
    }

    fn retire_host(&self, host_id: &str) {
        self.host_operations
            .hosts
            .lock()
            .entry(host_id.to_owned())
            .or_default()
            .retired = true;
    }

    async fn wait_host_idle(&self, host_id: &str) {
        loop {
            let changed = self.host_operations.changed.notified();
            if self
                .host_operations
                .hosts
                .lock()
                .get(host_id)
                .is_none_or(|state| state.active == 0)
            {
                return;
            }
            changed.await;
        }
    }

    fn restore_host(&self, host_id: &str) {
        let mut hosts = self.host_operations.hosts.lock();
        if let Some(state) = hosts.get_mut(host_id) {
            state.retired = false;
            if state.active == 0 {
                hosts.remove(host_id);
            }
        }
        drop(hosts);
        self.host_operations.changed.notify_waiters();
    }

    pub async fn list(
        &self,
        host: &HostProfile,
        request: SftpListRequest,
    ) -> AppResult<SftpListResult> {
        validate_remote_path(&request.path)?;
        let script = batch_command("ls -lan", [&request.path])?;
        let output = self
            .run_batch(host, script, BASIC_OPERATION_TIMEOUT)
            .await?;
        let mut entries = parse_long_listing(&captured_text(&output.stdout), &request.path)?;
        entries.retain(|entry| entry.name != "." && entry.name != "..");
        if !request.show_hidden {
            entries.retain(|entry| !entry.name.starts_with('.'));
        }
        entries.sort_by(|left, right| {
            entry_kind_order(left.kind)
                .cmp(&entry_kind_order(right.kind))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(SftpListResult {
            host_id: host.id.clone(),
            parent_path: remote_parent(&request.path),
            path: request.path,
            entries,
        })
    }

    async fn list_cancellable(
        &self,
        host: &HostProfile,
        request: SftpListRequest,
        duration: Duration,
        cancellation: &ChildCancellation,
    ) -> AppResult<SftpListResult> {
        validate_remote_path(&request.path)?;
        let script = batch_command("ls -lan", [&request.path])?;
        let output = self
            .run_batch_cancellable(host, script, duration, cancellation)
            .await?;
        let mut entries = parse_long_listing(&captured_text(&output.stdout), &request.path)?;
        entries.retain(|entry| entry.name != "." && entry.name != "..");
        if !request.show_hidden {
            entries.retain(|entry| !entry.name.starts_with('.'));
        }
        entries.sort_by(|left, right| {
            entry_kind_order(left.kind)
                .cmp(&entry_kind_order(right.kind))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(SftpListResult {
            host_id: host.id.clone(),
            parent_path: remote_parent(&request.path),
            path: request.path,
            entries,
        })
    }

    pub async fn mkdir(
        &self,
        host: &HostProfile,
        request: SftpMkdirRequest,
    ) -> AppResult<SftpOperationResult> {
        validate_remote_path(&request.path)?;
        let script = batch_command("mkdir", [&request.path])?;
        self.run_batch(host, script, BASIC_OPERATION_TIMEOUT)
            .await?;
        Ok(SftpOperationResult {
            success: true,
            affected: vec![request.path],
        })
    }

    pub async fn rename(
        &self,
        host: &HostProfile,
        request: SftpRenameRequest,
    ) -> AppResult<SftpOperationResult> {
        validate_destructive_remote_path(&request.source)?;
        validate_destructive_remote_path(&request.destination)?;
        let script = batch_command("rename", [&request.source, &request.destination])?;
        self.run_batch(host, script, BASIC_OPERATION_TIMEOUT)
            .await?;
        Ok(SftpOperationResult {
            success: true,
            affected: vec![request.source, request.destination],
        })
    }

    pub async fn delete(
        &self,
        host: &HostProfile,
        request: SftpDeleteRequest,
    ) -> AppResult<SftpOperationResult> {
        validate_destructive_remote_path(&request.path)?;
        let Some(metadata) = self.metadata(host, &request.path).await? else {
            return Ok(SftpOperationResult {
                success: true,
                affected: Vec::new(),
            });
        };
        let affected = if metadata.kind == SftpEntryKind::Directory {
            if request.recursive {
                self.delete_tree(host, &request.path).await?
            } else {
                let script = batch_command("rmdir", [&request.path])?;
                self.run_batch(host, script, BASIC_OPERATION_TIMEOUT)
                    .await?;
                vec![request.path]
            }
        } else {
            let script = batch_command("rm", [&request.path])?;
            self.run_batch(host, script, BASIC_OPERATION_TIMEOUT)
                .await?;
            vec![request.path]
        };
        Ok(SftpOperationResult {
            success: true,
            affected,
        })
    }

    async fn metadata(&self, host: &HostProfile, path: &str) -> AppResult<Option<SftpEntry>> {
        self.metadata_inner(host, path, BASIC_OPERATION_TIMEOUT, None)
            .await
    }

    async fn metadata_cancellable(
        &self,
        host: &HostProfile,
        path: &str,
        duration: Duration,
        cancellation: &ChildCancellation,
    ) -> AppResult<Option<SftpEntry>> {
        self.metadata_inner(host, path, duration, Some(cancellation))
            .await
    }

    async fn metadata_inner(
        &self,
        host: &HostProfile,
        path: &str,
        duration: Duration,
        cancellation: Option<&ChildCancellation>,
    ) -> AppResult<Option<SftpEntry>> {
        validate_remote_path(path)?;
        let (query_path, target_name) = metadata_listing_target(path);
        // OpenSSH's sftp `ls` does not implement the shell `-d` flag. Querying
        // the parent and selecting the exact entry preserves symlink metadata
        // without interpreting the target name as a glob.
        let script = batch_command("ls -lan", [&query_path])?;
        let output = self
            .run_raw_batch_inner(host, script, duration, cancellation)
            .await?;
        if !output.status.success() {
            let message = process_message(&output);
            if is_missing_path_error(&message) {
                return Ok(None);
            }
            return Err(AppError::Process(message));
        }

        let Some(target_name) = target_name else {
            return Ok(Some(SftpEntry {
                name: remote_file_name(path).unwrap_or_else(|| path.to_owned()),
                path: path.to_owned(),
                kind: SftpEntryKind::Directory,
                size: 0,
                modified_at: None,
                permissions: None,
            }));
        };
        let entries = parse_long_listing(&captured_text(&output.stdout), &query_path)?;
        select_metadata_entry(entries, &target_name, path)
    }

    async fn delete_tree(&self, host: &HostProfile, root: &str) -> AppResult<Vec<String>> {
        let mut stack = vec![(root.to_owned(), 0_usize, false)];
        let mut files = Vec::new();
        let mut directories = Vec::new();
        let mut discovered = 1_usize;
        let mut path_bytes = root.len();
        while let Some((path, depth, visited)) = stack.pop() {
            if depth > MAX_RECURSION_DEPTH {
                return Err(AppError::Validation(format!(
                    "remote tree exceeds {MAX_RECURSION_DEPTH} levels"
                )));
            }
            if discovered > MAX_REMOTE_ENTRIES || path_bytes > MAX_TREE_PATH_BYTES {
                return Err(AppError::Validation(format!(
                    "remote tree exceeds the {MAX_REMOTE_ENTRIES}-entry or {MAX_TREE_PATH_BYTES}-byte path budget"
                )));
            }
            if visited {
                directories.push(path);
                continue;
            }
            stack.push((path.clone(), depth, true));
            let children = self
                .list(
                    host,
                    SftpListRequest {
                        path,
                        show_hidden: true,
                    },
                )
                .await?
                .entries;
            for child in children.into_iter().rev() {
                discovered = discovered.saturating_add(1);
                path_bytes = path_bytes.saturating_add(child.path.len());
                if discovered > MAX_REMOTE_ENTRIES || path_bytes > MAX_TREE_PATH_BYTES {
                    return Err(AppError::Validation(
                        "remote tree exceeds deletion safety limits".to_owned(),
                    ));
                }
                if child.kind == SftpEntryKind::Directory {
                    stack.push((child.path, depth + 1, false));
                } else {
                    files.push(child.path);
                }
            }
        }
        self.run_path_commands(host, "rm", &files).await?;
        self.run_path_commands(host, "rmdir", &directories).await?;
        files.extend(directories);
        Ok(files)
    }

    async fn run_path_commands(
        &self,
        host: &HostProfile,
        command: &str,
        paths: &[String],
    ) -> AppResult<()> {
        let mut script = String::new();
        for path in paths {
            let line = batch_command(command, [path])?;
            if line.len() > MAX_BATCH_BYTES {
                return Err(AppError::Validation(
                    "one SFTP batch command exceeds the safety limit".to_owned(),
                ));
            }
            if !script.is_empty() && script.len() + line.len() > MAX_BATCH_BYTES {
                self.run_batch(host, std::mem::take(&mut script), BASIC_OPERATION_TIMEOUT)
                    .await?;
            }
            script.push_str(&line);
        }
        if !script.is_empty() {
            self.run_batch(host, script, BASIC_OPERATION_TIMEOUT)
                .await?;
        }
        Ok(())
    }

    async fn run_batch(
        &self,
        host: &HostProfile,
        script: String,
        duration: Duration,
    ) -> AppResult<BoundedOutput> {
        let output = self.run_raw_batch(host, script, duration).await?;
        if !output.status.success() {
            return Err(AppError::Process(process_message(&output)));
        }
        Ok(output)
    }

    async fn run_batch_cancellable(
        &self,
        host: &HostProfile,
        script: String,
        duration: Duration,
        cancellation: &ChildCancellation,
    ) -> AppResult<BoundedOutput> {
        let output = self
            .run_raw_batch_inner(host, script, duration, Some(cancellation))
            .await?;
        if !output.status.success() {
            return Err(AppError::Process(process_message(&output)));
        }
        Ok(output)
    }

    async fn run_raw_batch(
        &self,
        host: &HostProfile,
        script: String,
        duration: Duration,
    ) -> AppResult<BoundedOutput> {
        self.run_raw_batch_inner(host, script, duration, None).await
    }

    async fn run_raw_batch_inner(
        &self,
        host: &HostProfile,
        mut script: String,
        duration: Duration,
        cancellation: Option<&ChildCancellation>,
    ) -> AppResult<BoundedOutput> {
        if script.is_empty() || script.len() > MAX_BATCH_BYTES || script.contains('\0') {
            return Err(AppError::Validation(
                "SFTP batch payload is invalid or too large".to_owned(),
            ));
        }
        if !script.ends_with('\n') {
            script.push('\n');
        }
        let (program, args) = self.runtime.sftp_batch_spec(host)?;
        let output = match cancellation {
            Some(cancellation) => {
                run_output_with_input_cancellable(
                    &program,
                    &args,
                    duration,
                    Some(script.into_bytes()),
                    Some(cancellation),
                )
                .await?
            }
            None => {
                run_output_with_input(&program, &args, duration, Some(script.into_bytes())).await?
            }
        };
        if output.stdout.truncated || output.stderr.truncated {
            return Err(AppError::Process(
                "SFTP output exceeded the 1 MiB safety limit".to_owned(),
            ));
        }
        Ok(output)
    }
}

#[derive(Clone)]
pub struct TransferRegistry {
    service: SftpService,
    operations: Arc<Mutex<()>>,
    entries: Arc<RwLock<HashMap<String, TransferEntry>>>,
    retired_hosts: Arc<RwLock<HashSet<String>>>,
    events: broadcast::Sender<TransferEvent>,
}

struct TransferEntry {
    job: TransferJob,
    host: HostProfile,
    operation: TransferOperation,
    control: Arc<TransferControl>,
}

#[derive(Clone)]
enum TransferOperation {
    Upload(UploadRequest),
    Download(DownloadRequest),
}

#[derive(Default)]
struct TransferControl {
    cancellation: ChildCancellation,
    finished: AtomicBool,
    finished_notify: Notify,
}

impl TransferControl {
    fn cancel(&self) {
        self.cancellation.cancel();
    }

    fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    fn mark_finished(&self) {
        if !self.finished.swap(true, Ordering::AcqRel) {
            self.finished_notify.notify_waiters();
            self.finished_notify.notify_one();
        }
    }

    async fn wait_finished(&self) {
        while !self.finished.load(Ordering::Acquire) {
            self.finished_notify.notified().await;
        }
    }
}

impl TransferRegistry {
    pub fn new(service: SftpService) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            service,
            operations: Arc::new(Mutex::new(())),
            entries: Arc::new(RwLock::new(HashMap::new())),
            retired_hosts: Arc::new(RwLock::new(HashSet::new())),
            events,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TransferEvent> {
        self.events.subscribe()
    }

    pub fn list(&self) -> Vec<TransferJob> {
        let mut jobs = self
            .entries
            .read()
            .values()
            .map(|entry| entry.job.clone())
            .collect::<Vec<_>>();
        jobs.sort_by_key(|job| job.created_at);
        jobs
    }

    pub fn get(&self, job_id: &str) -> AppResult<TransferJob> {
        self.entries
            .read()
            .get(job_id)
            .map(|entry| entry.job.clone())
            .ok_or_else(|| AppError::NotFound(format!("transfer job {job_id}")))
    }

    pub fn start_upload(
        &self,
        host: HostProfile,
        request: UploadRequest,
    ) -> AppResult<TransferJob> {
        let _operation = self.operations.lock();
        let host = self.service.runtime.current_host_profile(&host)?;
        validate_local_path(&request.local_path)?;
        validate_destructive_remote_path(&request.remote_path)?;
        self.start(host, TransferOperation::Upload(request))
    }

    pub fn start_download(
        &self,
        host: HostProfile,
        request: DownloadRequest,
    ) -> AppResult<TransferJob> {
        let _operation = self.operations.lock();
        let host = self.service.runtime.current_host_profile(&host)?;
        validate_remote_path(&request.remote_path)?;
        validate_local_path(&request.local_path)?;
        self.start(host, TransferOperation::Download(request))
    }

    fn start(&self, host: HostProfile, operation: TransferOperation) -> AppResult<TransferJob> {
        let retired_hosts = self.retired_hosts.read();
        if retired_hosts.contains(&host.id) {
            return Err(AppError::NotFound(format!("host {}", host.id)));
        }
        let now = Utc::now();
        let (direction, source, destination, conflict_policy, recursive) = match &operation {
            TransferOperation::Upload(request) => (
                TransferDirection::Upload,
                request.local_path.clone(),
                request.remote_path.clone(),
                request.conflict_policy,
                request.recursive,
            ),
            TransferOperation::Download(request) => (
                TransferDirection::Download,
                request.remote_path.clone(),
                request.local_path.clone(),
                request.conflict_policy,
                request.recursive,
            ),
        };
        let job = TransferJob {
            id: Uuid::new_v4().to_string(),
            host_id: host.id.clone(),
            direction,
            source,
            destination,
            conflict_policy,
            recursive,
            state: TransferState::Queued,
            attempts: 0,
            bytes_transferred: 0,
            total_bytes: None,
            skipped: false,
            created_at: now,
            updated_at: now,
            completed_at: None,
            error: None,
        };
        let mut entries = self.entries.write();
        prune_terminal_transfers(&mut entries, MAX_RETAINED_TRANSFERS);
        if entries
            .values()
            .filter(|entry| !transfer_state_is_terminal(entry.job.state))
            .count()
            >= MAX_ACTIVE_TRANSFERS
        {
            return Err(AppError::Validation(format!(
                "at most {MAX_ACTIVE_TRANSFERS} active transfers are allowed"
            )));
        }
        entries.insert(
            job.id.clone(),
            TransferEntry {
                job: job.clone(),
                host,
                operation,
                control: Arc::new(TransferControl::default()),
            },
        );
        drop(entries);
        drop(retired_hosts);
        self.emit(job.clone());
        self.spawn_attempt(job.id.clone());
        Ok(job)
    }

    pub async fn cancel(&self, job_id: &str) -> AppResult<TransferJob> {
        let (job, control) = {
            let mut entries = self.entries.write();
            let entry = entries
                .get_mut(job_id)
                .ok_or_else(|| AppError::NotFound(format!("transfer job {job_id}")))?;
            if matches!(
                entry.job.state,
                TransferState::Queued | TransferState::Running
            ) {
                entry.job.state = transition(entry.job.state, TransferState::Cancelling)?;
                entry.job.updated_at = Utc::now();
            }
            (entry.job.clone(), entry.control.clone())
        };
        if job.state == TransferState::Cancelling {
            control.cancel();
            self.emit(job.clone());
        }
        Ok(job)
    }

    pub fn retry(&self, job_id: &str, host: HostProfile) -> AppResult<TransferJob> {
        let _operation = self.operations.lock();
        let host = self.service.runtime.current_host_profile(&host)?;
        let retired_hosts = self.retired_hosts.read();
        if retired_hosts.contains(&host.id) {
            return Err(AppError::NotFound(format!("host {}", host.id)));
        }
        let job = {
            let mut entries = self.entries.write();
            if entries
                .iter()
                .filter(|(id, entry)| {
                    id.as_str() != job_id && !transfer_state_is_terminal(entry.job.state)
                })
                .count()
                >= MAX_ACTIVE_TRANSFERS
            {
                return Err(AppError::Validation(format!(
                    "at most {MAX_ACTIVE_TRANSFERS} active transfers are allowed"
                )));
            }
            let entry = entries
                .get_mut(job_id)
                .ok_or_else(|| AppError::NotFound(format!("transfer job {job_id}")))?;
            prepare_transfer_retry(entry, host)?
        };
        drop(retired_hosts);
        self.emit(job.clone());
        self.spawn_attempt(job.id.clone());
        Ok(job)
    }

    /// Serializes profile persistence with transfer registration and rejects
    /// connection-critical edits while a captured direct or ProxyJump route
    /// is still in use by queued, running, or cancelling work.
    pub fn apply_host_update<T>(
        &self,
        previous: &HostProfile,
        next: &HostProfile,
        update: impl FnOnce() -> AppResult<T>,
    ) -> AppResult<T> {
        let _operation = self.operations.lock();
        if !same_connection_profile(previous, next)
            && self.entries.read().values().any(|entry| {
                !transfer_state_is_terminal(entry.job.state)
                    && connection_uses_host(&entry.host, previous)
            })
        {
            return Err(AppError::State(
                "stop active transfers before changing these SSH connection settings".to_owned(),
            ));
        }
        update()
    }

    /// Retires a host before repository deletion, cancels its in-flight work,
    /// and removes every retained job so no stale profile can be retried.
    pub async fn remove_host(&self, host_id: &str) -> AppResult<()> {
        self.retired_hosts.write().insert(host_id.to_owned());
        self.service.retire_host(host_id);
        let matching = self
            .entries
            .read()
            .iter()
            .filter(|(_, entry)| entry.job.host_id == host_id)
            .map(|(id, entry)| (id.clone(), entry.control.clone()))
            .collect::<Vec<_>>();
        for (id, _) in &matching {
            let _ = self.cancel(id).await;
        }
        tokio::time::timeout(Duration::from_secs(30), async {
            for (_, control) in &matching {
                control.wait_finished().await;
            }
            self.service.wait_host_idle(host_id).await;
        })
        .await
        .map_err(|_| {
            AppError::Timeout(
                "timed out waiting for in-flight SFTP operations to finish safely".to_owned(),
            )
        })?;
        self.entries
            .write()
            .retain(|_, entry| entry.job.host_id != host_id);
        Ok(())
    }

    /// Restores a host only when a coordinated repository deletion aborts.
    pub fn restore_host(&self, host_id: &str) {
        self.retired_hosts.write().remove(host_id);
        self.service.restore_host(host_id);
    }

    pub async fn cancel_all(&self) {
        let active = self
            .entries
            .read()
            .iter()
            .filter(|(_, entry)| {
                matches!(
                    entry.job.state,
                    TransferState::Queued | TransferState::Running | TransferState::Cancelling
                )
            })
            .map(|(id, entry)| (id.clone(), entry.control.clone()))
            .collect::<Vec<_>>();
        for (id, _) in &active {
            let _ = self.cancel(id).await;
        }
        let _ = tokio::time::timeout(Duration::from_secs(30), async {
            for (_, control) in active {
                control.wait_finished().await;
            }
        })
        .await;
    }

    fn spawn_attempt(&self, job_id: String) {
        let registry = self.clone();
        tokio::spawn(async move {
            registry.execute(job_id).await;
        });
    }

    async fn execute(&self, job_id: String) {
        let (host, operation, control, running) = {
            let mut entries = self.entries.write();
            let Some(entry) = entries.get_mut(&job_id) else {
                return;
            };
            let control = entry.control.clone();
            if entry.control.is_cancelled() {
                entry.job.state = TransferState::Cancelled;
                entry.job.updated_at = Utc::now();
                entry.job.completed_at = Some(Utc::now());
                let job = entry.job.clone();
                drop(entries);
                self.emit(job);
                control.mark_finished();
                return;
            }
            entry.job.state = match transition(entry.job.state, TransferState::Running) {
                Ok(state) => state,
                Err(error) => {
                    entry.job.state = TransferState::Failed;
                    entry.job.error = Some(error.to_string());
                    let job = entry.job.clone();
                    drop(entries);
                    self.emit(job);
                    control.mark_finished();
                    return;
                }
            };
            entry.job.attempts = entry.job.attempts.saturating_add(1);
            entry.job.updated_at = Utc::now();
            (
                entry.host.clone(),
                entry.operation.clone(),
                control,
                entry.job.clone(),
            )
        };
        self.emit(running);
        let result = match self.service.begin_host_operation(&host.id) {
            Ok(_host_operation) => match operation {
                TransferOperation::Upload(request) => {
                    self.perform_upload(&job_id, &host, request, control.clone())
                        .await
                }
                TransferOperation::Download(request) => {
                    self.perform_download(&job_id, &host, request, control.clone())
                        .await
                }
            },
            Err(error) => Err(error),
        };
        let job = {
            let mut entries = self.entries.write();
            let Some(entry) = entries.get_mut(&job_id) else {
                control.mark_finished();
                return;
            };
            let now = Utc::now();
            match result {
                Ok(outcome) => {
                    entry.job.state = TransferState::Completed;
                    entry.job.destination = outcome.destination;
                    entry.job.total_bytes = Some(outcome.total_bytes);
                    entry.job.bytes_transferred = outcome.total_bytes;
                    entry.job.skipped = outcome.skipped;
                    entry.job.error = None;
                }
                Err(error) => {
                    if is_cancelled_error(&error) {
                        entry.job.state = TransferState::Cancelled;
                        entry.job.error = None;
                    } else {
                        entry.job.state = TransferState::Failed;
                        entry.job.error = Some(limited_error(&error.to_string()));
                    }
                }
            }
            entry.job.updated_at = now;
            entry.job.completed_at = Some(now);
            entry.job.clone()
        };
        control.mark_finished();
        self.emit(job);
    }

    async fn perform_upload(
        &self,
        job_id: &str,
        host: &HostProfile,
        request: UploadRequest,
        control: Arc<TransferControl>,
    ) -> AppResult<TransferOutcome> {
        let preflight_deadline = Instant::now() + TRANSFER_PREFLIGHT_TIMEOUT;
        let local_path = PathBuf::from(&request.local_path);
        let metadata = fs::symlink_metadata(&local_path)?;
        if metadata.file_type().is_symlink() {
            return Err(AppError::Validation(
                "symbolic-link upload sources are not followed".to_owned(),
            ));
        }
        let is_directory = metadata.is_dir();
        if is_directory && !request.recursive {
            return Err(AppError::Validation(
                "directory upload requires recursive=true".to_owned(),
            ));
        }
        let local_for_size = local_path.clone();
        let total_bytes = tokio::task::spawn_blocking(move || local_tree_size(&local_for_size))
            .await
            .map_err(|error| AppError::Process(format!("size worker failed: {error}")))??;
        ensure_transfer_preflight(&control, preflight_deadline)?;
        let resolution = self
            .resolve_remote_conflict(
                host,
                &request.remote_path,
                request.conflict_policy,
                is_directory,
                &control,
                preflight_deadline,
            )
            .await?;
        if resolution.skip {
            ensure_transfer_preflight(&control, preflight_deadline)?;
            return Ok(TransferOutcome {
                destination: resolution.destination,
                total_bytes,
                skipped: true,
            });
        }
        let temporary = owned_remote_temp(&resolution.destination, job_id)?;
        let duration = transfer_preflight_remaining(&control, preflight_deadline)?;
        if self
            .service
            .metadata_cancellable(host, &temporary, duration, &control.cancellation)
            .await?
            .is_some()
        {
            return Err(AppError::State(format!(
                "an app-owned temporary already exists at {temporary}; inspect or remove it before retrying"
            )));
        }
        let transfer_script = batch_command(
            if is_directory { "put -r" } else { "put" },
            [request.local_path.as_str(), temporary.as_str()],
        )?;
        let result = self
            .service
            .run_batch_cancellable(
                host,
                transfer_script,
                TRANSFER_TIMEOUT,
                &control.cancellation,
            )
            .await
            .map(|_| ());
        if let Err(error) = result {
            if matches!(&error, AppError::ProcessCleanup(_)) {
                return Err(AppError::ProcessCleanup(format!(
                    "{error}; an app-owned temporary may remain at {temporary}"
                )));
            }
            if let Err(cleanup_error) = self.remove_remote_temp(host, &temporary).await {
                return Err(AppError::Process(format!(
                    "transfer ended ({error}); failed to remove app-owned temporary {temporary}: {cleanup_error}"
                )));
            }
            return Err(error);
        }
        if control.is_cancelled() {
            if let Err(cleanup_error) = self.remove_remote_temp(host, &temporary).await {
                return Err(AppError::Process(format!(
                    "transfer was cancelled after SFTP exit; failed to remove app-owned temporary {temporary}: {cleanup_error}"
                )));
            }
            return Err(cancelled_error());
        }
        let existing = self.service.metadata(host, &resolution.destination).await?;
        if let Err(error) = self
            .commit_remote_transfer(
                host,
                job_id,
                &temporary,
                &resolution.destination,
                existing,
                request.conflict_policy,
            )
            .await
        {
            if let Err(cleanup_error) = self.remove_remote_temp(host, &temporary).await {
                return Err(AppError::Process(format!(
                    "transfer commit failed ({error}); failed to remove app-owned temporary {temporary}: {cleanup_error}"
                )));
            }
            return Err(error);
        }
        Ok(TransferOutcome {
            destination: resolution.destination,
            total_bytes,
            skipped: false,
        })
    }

    async fn perform_download(
        &self,
        job_id: &str,
        host: &HostProfile,
        request: DownloadRequest,
        control: Arc<TransferControl>,
    ) -> AppResult<TransferOutcome> {
        let preflight_deadline = Instant::now() + TRANSFER_PREFLIGHT_TIMEOUT;
        let metadata_duration = transfer_preflight_remaining(&control, preflight_deadline)?;
        let metadata = self
            .service
            .metadata_cancellable(
                host,
                &request.remote_path,
                metadata_duration,
                &control.cancellation,
            )
            .await?
            .ok_or_else(|| AppError::NotFound(format!("remote path {}", request.remote_path)))?;
        let is_directory = metadata.kind == SftpEntryKind::Directory;
        if is_directory && !request.recursive {
            return Err(AppError::Validation(
                "directory download requires recursive=true".to_owned(),
            ));
        }
        let total_bytes = if is_directory {
            self.remote_tree_size(host, &request.remote_path, &control, preflight_deadline)
                .await?
        } else {
            metadata.size
        };
        ensure_transfer_preflight(&control, preflight_deadline)?;
        let resolution = resolve_local_conflict(
            Path::new(&request.local_path),
            request.conflict_policy,
            is_directory,
        )?;
        if resolution.skip {
            ensure_transfer_preflight(&control, preflight_deadline)?;
            return Ok(TransferOutcome {
                destination: resolution.destination.to_string_lossy().into_owned(),
                total_bytes,
                skipped: true,
            });
        }
        let temporary = owned_local_temp(&resolution.destination, job_id)?;
        if let Some(parent) = temporary.parent() {
            fs::create_dir_all(parent)?;
        }
        remove_local_temp_if_owned(&temporary)?;
        ensure_transfer_preflight(&control, preflight_deadline)?;
        let temporary_text = temporary.to_string_lossy().into_owned();
        let transfer_script = batch_command(
            if is_directory { "get -r" } else { "get" },
            [request.remote_path.as_str(), temporary_text.as_str()],
        )?;
        let result = self
            .service
            .run_batch_cancellable(
                host,
                transfer_script,
                TRANSFER_TIMEOUT,
                &control.cancellation,
            )
            .await
            .map(|_| ());
        if let Err(error) = result {
            if matches!(&error, AppError::ProcessCleanup(_)) {
                return Err(AppError::ProcessCleanup(format!(
                    "{error}; an app-owned temporary may remain at {}",
                    temporary.display()
                )));
            }
            if let Err(cleanup_error) = remove_local_temp_if_owned(&temporary) {
                return Err(AppError::Process(format!(
                    "transfer ended ({error}); failed to remove app-owned temporary {}: {cleanup_error}",
                    temporary.display()
                )));
            }
            return Err(error);
        }
        if control.is_cancelled() {
            if let Err(cleanup_error) = remove_local_temp_if_owned(&temporary) {
                return Err(AppError::Process(format!(
                    "transfer was cancelled after SFTP exit; failed to remove app-owned temporary {}: {cleanup_error}",
                    temporary.display()
                )));
            }
            return Err(cancelled_error());
        }
        if let Err(error) = commit_local_transfer(
            job_id,
            &temporary,
            &resolution.destination,
            request.conflict_policy,
        ) {
            if let Err(cleanup_error) = remove_local_temp_if_owned(&temporary) {
                return Err(AppError::Process(format!(
                    "transfer commit failed ({error}); failed to remove app-owned temporary {}: {cleanup_error}",
                    temporary.display()
                )));
            }
            return Err(error);
        }
        Ok(TransferOutcome {
            destination: resolution.destination.to_string_lossy().into_owned(),
            total_bytes,
            skipped: false,
        })
    }

    async fn resolve_remote_conflict(
        &self,
        host: &HostProfile,
        requested: &str,
        policy: ConflictPolicy,
        is_directory: bool,
        control: &TransferControl,
        deadline: Instant,
    ) -> AppResult<RemoteResolution> {
        let duration = transfer_preflight_remaining(control, deadline)?;
        if self
            .service
            .metadata_cancellable(host, requested, duration, &control.cancellation)
            .await?
            .is_none()
        {
            return Ok(RemoteResolution {
                destination: requested.to_owned(),
                skip: false,
            });
        }
        match policy {
            ConflictPolicy::Ask => Err(conflict_requires_choice("remote")),
            ConflictPolicy::Skip => Ok(RemoteResolution {
                destination: requested.to_owned(),
                skip: true,
            }),
            ConflictPolicy::Overwrite => Ok(RemoteResolution {
                destination: requested.to_owned(),
                skip: false,
            }),
            ConflictPolicy::Rename => {
                for counter in 1..=9999 {
                    let duration = transfer_preflight_remaining(control, deadline)?;
                    let candidate = conflict_remote_path(requested, counter, is_directory)?;
                    if self
                        .service
                        .metadata_cancellable(host, &candidate, duration, &control.cancellation)
                        .await?
                        .is_none()
                    {
                        return Ok(RemoteResolution {
                            destination: candidate,
                            skip: false,
                        });
                    }
                }
                Err(AppError::Process(
                    "could not find an unused remote conflict name".to_owned(),
                ))
            }
        }
    }

    async fn remote_tree_size(
        &self,
        host: &HostProfile,
        root: &str,
        control: &TransferControl,
        deadline: Instant,
    ) -> AppResult<u64> {
        let mut total = 0_u64;
        let mut count = 0_usize;
        let mut path_bytes = root.len();
        let mut stack = vec![(root.to_owned(), 0_usize)];
        while let Some((path, depth)) = stack.pop() {
            let duration = transfer_preflight_remaining(control, deadline)?;
            if depth > MAX_RECURSION_DEPTH
                || count > MAX_REMOTE_ENTRIES
                || path_bytes > MAX_TREE_PATH_BYTES
            {
                return Err(AppError::Validation(
                    "remote tree exceeds transfer safety limits".to_owned(),
                ));
            }
            for entry in self
                .service
                .list_cancellable(
                    host,
                    SftpListRequest {
                        path,
                        show_hidden: true,
                    },
                    duration,
                    &control.cancellation,
                )
                .await?
                .entries
            {
                count += 1;
                path_bytes = path_bytes.saturating_add(entry.path.len());
                if count > MAX_REMOTE_ENTRIES || path_bytes > MAX_TREE_PATH_BYTES {
                    return Err(AppError::Validation(
                        "remote tree exceeds transfer safety limits".to_owned(),
                    ));
                }
                if entry.kind == SftpEntryKind::Directory {
                    stack.push((entry.path, depth + 1));
                } else {
                    total = total.saturating_add(entry.size);
                }
            }
        }
        Ok(total)
    }

    async fn commit_remote_transfer(
        &self,
        host: &HostProfile,
        job_id: &str,
        temporary: &str,
        destination: &str,
        existing: Option<SftpEntry>,
        policy: ConflictPolicy,
    ) -> AppResult<()> {
        let Some(existing) = existing else {
            return self
                .service
                .rename(
                    host,
                    SftpRenameRequest {
                        source: temporary.to_owned(),
                        destination: destination.to_owned(),
                    },
                )
                .await
                .map(|_| ());
        };
        if policy != ConflictPolicy::Overwrite {
            return Err(AppError::Process(
                "transfer destination appeared before final rename".to_owned(),
            ));
        }

        let backup = owned_remote_backup(destination, job_id)?;
        if !is_owned_remote_backup(&backup) {
            return Err(AppError::State(
                "generated remote backup path is not app-owned".to_owned(),
            ));
        }
        if self.service.metadata(host, &backup).await?.is_some() {
            return Err(AppError::State(format!(
                "a preserved transfer backup already exists at {backup}; recover it before retrying"
            )));
        }
        self.service
            .rename(
                host,
                SftpRenameRequest {
                    source: destination.to_owned(),
                    destination: backup.clone(),
                },
            )
            .await?;
        if let Err(replace_error) = self
            .service
            .rename(
                host,
                SftpRenameRequest {
                    source: temporary.to_owned(),
                    destination: destination.to_owned(),
                },
            )
            .await
        {
            if let Err(rollback_error) = self
                .service
                .rename(
                    host,
                    SftpRenameRequest {
                        source: backup.clone(),
                        destination: destination.to_owned(),
                    },
                )
                .await
            {
                return Err(AppError::State(format!(
                    "replacement failed ({replace_error}); rollback failed ({rollback_error}); the original remains at {backup}"
                )));
            }
            return Err(replace_error);
        }
        if let Err(error) = self
            .service
            .delete(
                host,
                SftpDeleteRequest {
                    path: backup.clone(),
                    recursive: existing.kind == SftpEntryKind::Directory,
                },
            )
            .await
        {
            return Err(AppError::State(format!(
                "replacement succeeded but the old destination backup remains at {backup}: {error}"
            )));
        }
        Ok(())
    }

    async fn remove_remote_temp(&self, host: &HostProfile, temporary: &str) -> AppResult<()> {
        if !is_owned_remote_temp(temporary) {
            return Err(AppError::State(
                "refusing to remove a transfer path that is not app-owned".to_owned(),
            ));
        }
        let Some(metadata) = self.service.metadata(host, temporary).await? else {
            return Ok(());
        };
        self.service
            .delete(
                host,
                SftpDeleteRequest {
                    path: temporary.to_owned(),
                    recursive: metadata.kind == SftpEntryKind::Directory,
                },
            )
            .await?;
        Ok(())
    }

    fn emit(&self, job: TransferJob) {
        let _ = self.events.send(TransferEvent { job });
    }
}

fn transfer_state_is_terminal(state: TransferState) -> bool {
    matches!(
        state,
        TransferState::Completed | TransferState::Cancelled | TransferState::Failed
    )
}

fn validate_transfer_operation(operation: &TransferOperation) -> AppResult<()> {
    match operation {
        TransferOperation::Upload(request) => {
            validate_local_path(&request.local_path)?;
            validate_destructive_remote_path(&request.remote_path)
        }
        TransferOperation::Download(request) => {
            validate_remote_path(&request.remote_path)?;
            validate_local_path(&request.local_path)
        }
    }
}

fn ensure_transfer_preflight(control: &TransferControl, deadline: Instant) -> AppResult<()> {
    transfer_preflight_remaining(control, deadline).map(|_| ())
}

fn transfer_preflight_remaining(
    control: &TransferControl,
    deadline: Instant,
) -> AppResult<Duration> {
    if control.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .map(|remaining| remaining.min(BASIC_OPERATION_TIMEOUT))
        .ok_or_else(|| {
            AppError::Timeout(format!(
                "transfer preflight exceeded {} seconds",
                TRANSFER_PREFLIGHT_TIMEOUT.as_secs()
            ))
        })
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

fn prepare_transfer_retry(entry: &mut TransferEntry, host: HostProfile) -> AppResult<TransferJob> {
    if entry.job.host_id != host.id {
        return Err(AppError::Validation(
            "transfer job does not belong to the selected host".to_owned(),
        ));
    }
    validate_transfer_operation(&entry.operation)?;
    entry.job.state = transition(entry.job.state, TransferState::Queued)?;
    entry.host = host;
    entry.job.updated_at = Utc::now();
    entry.job.completed_at = None;
    entry.job.error = None;
    entry.job.skipped = false;
    entry.job.bytes_transferred = 0;
    entry.control = Arc::new(TransferControl::default());
    Ok(entry.job.clone())
}

fn prune_terminal_transfers(entries: &mut HashMap<String, TransferEntry>, limit: usize) {
    while entries.len() >= limit {
        let oldest = entries
            .iter()
            .filter(|(_, entry)| transfer_state_is_terminal(entry.job.state))
            .min_by_key(|(_, entry)| entry.job.created_at)
            .map(|(id, _)| id.clone());
        let Some(oldest) = oldest else {
            break;
        };
        entries.remove(&oldest);
    }
}

struct TransferOutcome {
    destination: String,
    total_bytes: u64,
    skipped: bool,
}

struct RemoteResolution {
    destination: String,
    skip: bool,
}

struct LocalResolution {
    destination: PathBuf,
    skip: bool,
}

fn batch_quote(value: &str) -> AppResult<String> {
    if value.is_empty() || value.contains(['\0', '\r', '\n']) || value.len() > 32_767 {
        return Err(AppError::Validation(
            "SFTP batch path is invalid".to_owned(),
        ));
    }
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        if matches!(character, '\\' | '"' | '*' | '?' | '[' | ']') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    Ok(quoted)
}

fn batch_command<I, S>(command: &str, arguments: I) -> AppResult<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if command.is_empty()
        || command
            .chars()
            .any(|character| character.is_control() || character == ';')
    {
        return Err(AppError::Validation(
            "SFTP batch command is invalid".to_owned(),
        ));
    }
    let mut line = command.to_owned();
    for argument in arguments {
        line.push(' ');
        line.push_str(&batch_quote(argument.as_ref())?);
    }
    line.push('\n');
    Ok(line)
}

fn parse_long_listing(output: &str, parent: &str) -> AppResult<Vec<SftpEntry>> {
    let mut entries = Vec::new();
    let mut retained_bytes = 0_usize;
    for raw_line in output.lines() {
        let line = raw_line.trim_end_matches('\r').trim_start();
        if line.is_empty()
            || line.starts_with("sftp>")
            || !matches!(
                line.as_bytes().first(),
                Some(b'-' | b'd' | b'l' | b'b' | b'c' | b'p' | b's')
            )
        {
            continue;
        }
        let (fields, remainder) = take_fields(line, 8).ok_or_else(|| {
            AppError::Process(format!("could not parse SFTP listing line: {line}"))
        })?;
        let permissions = fields[0].to_owned();
        if permissions.len() < 10 {
            return Err(AppError::Process(format!(
                "invalid SFTP permissions field: {permissions}"
            )));
        }
        let kind = match permissions.as_bytes()[0] {
            b'-' => SftpEntryKind::File,
            b'd' => SftpEntryKind::Directory,
            b'l' => SftpEntryKind::Symlink,
            _ => SftpEntryKind::Other,
        };
        let size = fields[4]
            .parse::<u64>()
            .map_err(|_| AppError::Process(format!("invalid SFTP size: {}", fields[4])))?;
        let displayed_name = if kind == SftpEntryKind::Symlink {
            remainder
                .rsplit_once(" -> ")
                .map_or(remainder, |(name, _)| name)
        } else {
            remainder
        };
        if displayed_name.is_empty() || displayed_name.contains(['\0', '\r', '\n']) {
            return Err(AppError::Process(
                "SFTP returned an invalid file name".to_owned(),
            ));
        }
        if entries.len() >= MAX_REMOTE_ENTRIES {
            return Err(AppError::Validation(format!(
                "SFTP listing exceeds {MAX_REMOTE_ENTRIES} entries"
            )));
        }
        let name = listing_file_name(displayed_name, parent);
        let path = remote_join(parent, &name);
        retained_bytes = retained_bytes
            .saturating_add(name.len())
            .saturating_add(path.len())
            .saturating_add(permissions.len());
        if retained_bytes > MAX_TREE_PATH_BYTES {
            return Err(AppError::Validation(
                "SFTP listing exceeds the retained path budget".to_owned(),
            ));
        }
        entries.push(SftpEntry {
            path,
            name,
            kind,
            size,
            modified_at: parse_listing_modified(fields[5], fields[6], fields[7]),
            permissions: Some(permissions),
        });
    }
    Ok(entries)
}

fn take_fields(value: &str, count: usize) -> Option<(Vec<&str>, &str)> {
    let mut remaining = value.trim_start();
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        if remaining.is_empty() {
            return None;
        }
        let end = remaining
            .find(char::is_whitespace)
            .unwrap_or(remaining.len());
        fields.push(&remaining[..end]);
        remaining = remaining[end..].trim_start();
    }
    (!remaining.is_empty()).then_some((fields, remaining))
}

fn validate_remote_path(path: &str) -> AppResult<()> {
    if path.is_empty()
        || path.len() > 4096
        || path.starts_with('-')
        || path.contains(['\0', '\r', '\n'])
    {
        return Err(AppError::Validation("remote path is invalid".to_owned()));
    }
    Ok(())
}

fn validate_destructive_remote_path(path: &str) -> AppResult<()> {
    validate_remote_path(path)?;
    if remote_path_is_dangerous(path) {
        return Err(AppError::Validation(
            "refusing a destructive operation on a remote root or ambiguous path".to_owned(),
        ));
    }
    Ok(())
}

fn remote_path_is_dangerous(path: &str) -> bool {
    if path != path.trim() {
        return true;
    }
    let value = path;
    if value.is_empty() || matches!(value, "/" | "//" | "." | ".." | "~" | "~/") {
        return true;
    }
    if !value.starts_with('/') && !value.starts_with("~/") {
        return true;
    }
    let suffix = value
        .strip_prefix("~/")
        .unwrap_or(value.trim_start_matches('/'));
    let mut depth = 0_usize;
    for component in suffix.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            _ => depth += 1,
        }
    }
    depth == 0
}

fn validate_local_path(path: &str) -> AppResult<()> {
    if path.is_empty() || path.len() > 32_767 || path.contains(['\0', '\r', '\n']) {
        return Err(AppError::Validation("local path is invalid".to_owned()));
    }
    if !Path::new(path).is_absolute() {
        return Err(AppError::Validation(
            "local transfer paths must be absolute".to_owned(),
        ));
    }
    Ok(())
}

fn remote_parent(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let index = trimmed.rfind('/')?;
    if index == 0 {
        Some("/".to_owned())
    } else if index == 1 && trimmed.starts_with('~') {
        Some("~/".to_owned())
    } else {
        Some(trimmed[..index].to_owned())
    }
}

fn remote_file_name(path: &str) -> Option<String> {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn metadata_listing_target(path: &str) -> (String, Option<String>) {
    let target_name =
        remote_file_name(path).filter(|name| !matches!(name.as_str(), "." | ".." | "~"));
    let query_path = target_name
        .as_ref()
        .and_then(|_| remote_parent(path))
        .unwrap_or_else(|| {
            if target_name.is_some() {
                ".".to_owned()
            } else {
                path.to_owned()
            }
        });
    (query_path, target_name)
}

fn select_metadata_entry(
    entries: Vec<SftpEntry>,
    target_name: &str,
    path: &str,
) -> AppResult<Option<SftpEntry>> {
    let mut matching = entries
        .into_iter()
        .filter(|entry| entry.name == target_name);
    let entry = matching.next();
    if matching.next().is_some() {
        return Err(AppError::Process(
            "SFTP metadata returned duplicate records".to_owned(),
        ));
    }
    Ok(entry.map(|mut entry| {
        entry.path = path.to_owned();
        entry
    }))
}

fn remote_join(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn listing_file_name(displayed: &str, parent: &str) -> String {
    let prefix = if parent.ends_with('/') {
        parent.to_owned()
    } else {
        format!("{parent}/")
    };
    displayed
        .strip_prefix(&prefix)
        .unwrap_or(displayed)
        .rsplit('/')
        .next()
        .unwrap_or(displayed)
        .to_owned()
}

fn entry_kind_order(kind: SftpEntryKind) -> u8 {
    match kind {
        SftpEntryKind::Directory => 0,
        SftpEntryKind::File => 1,
        SftpEntryKind::Symlink => 2,
        SftpEntryKind::Other => 3,
    }
}

fn process_message(output: &BoundedOutput) -> String {
    let stderr = captured_text(&output.stderr);
    let stdout = captured_text(&output.stdout);
    let message = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    if message.trim().is_empty() {
        format!("OpenSSH sftp exited with {}", output.status)
    } else {
        limited_error(message.trim())
    }
}

fn limited_error(value: &str) -> String {
    const LIMIT: usize = 16 * 1024;
    if value.len() <= LIMIT {
        return value.to_owned();
    }
    let mut end = LIMIT;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[RemoteDeck: error truncated]", &value[..end])
}

fn cancelled_error() -> AppError {
    AppError::Cancelled
}

fn is_cancelled_error(error: &AppError) -> bool {
    matches!(error, AppError::Cancelled)
}

fn is_missing_path_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("no such file") || lower.contains("not found")
}

fn conflict_remote_path(path: &str, counter: u32, is_directory: bool) -> AppResult<String> {
    let parent = remote_parent(path)
        .ok_or_else(|| AppError::Validation("remote destination has no parent".to_owned()))?;
    let name = remote_file_name(path)
        .ok_or_else(|| AppError::Validation("remote destination has no name".to_owned()))?;
    Ok(remote_join(
        &parent,
        &conflict_name(&name, counter, is_directory),
    ))
}

fn resolve_local_conflict(
    requested: &Path,
    policy: ConflictPolicy,
    is_directory: bool,
) -> AppResult<LocalResolution> {
    if !local_path_exists(requested)? {
        return Ok(LocalResolution {
            destination: requested.to_path_buf(),
            skip: false,
        });
    }
    match policy {
        ConflictPolicy::Ask => Err(conflict_requires_choice("local")),
        ConflictPolicy::Skip => Ok(LocalResolution {
            destination: requested.to_path_buf(),
            skip: true,
        }),
        ConflictPolicy::Overwrite => Ok(LocalResolution {
            destination: requested.to_path_buf(),
            skip: false,
        }),
        ConflictPolicy::Rename => {
            let parent = requested.parent().ok_or_else(|| {
                AppError::Validation("local destination has no parent".to_owned())
            })?;
            let name = requested
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    AppError::Validation("local destination name is not valid Unicode".to_owned())
                })?;
            for counter in 1..=9999 {
                let candidate = parent.join(conflict_name(name, counter, is_directory));
                if !candidate.exists() {
                    return Ok(LocalResolution {
                        destination: candidate,
                        skip: false,
                    });
                }
            }
            Err(AppError::Process(
                "could not find an unused local conflict name".to_owned(),
            ))
        }
    }
}

fn conflict_requires_choice(location: &str) -> AppError {
    AppError::State(format!(
        "{location} transfer destination already exists; choose overwrite, skip, or rename"
    ))
}

fn parse_listing_modified(month: &str, day: &str, year_or_time: &str) -> Option<String> {
    let month = match month.to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let day = day.parse::<u32>().ok()?;
    let now = Local::now();
    let (mut year, hour, minute, infer_year) =
        if let Some((hour, minute)) = year_or_time.split_once(':') {
            (
                now.year(),
                hour.parse::<u32>().ok()?,
                minute.parse::<u32>().ok()?,
                true,
            )
        } else {
            (year_or_time.parse::<i32>().ok()?, 0, 0, false)
        };
    let mut modified = Local
        .with_ymd_and_hms(year, month, day, hour, minute, 0)
        .earliest()?;
    if infer_year && modified > now + chrono::Duration::days(1) {
        year -= 1;
        modified = Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .earliest()?;
    }
    Some(modified.to_rfc3339())
}

fn conflict_name(name: &str, counter: u32, is_directory: bool) -> String {
    if is_directory {
        return format!("{name} ({counter})");
    }
    let extension_at = name
        .rfind('.')
        .filter(|index| *index > 0 && *index + 1 < name.len());
    match extension_at {
        Some(index) => format!("{} ({counter}){}", &name[..index], &name[index..]),
        None => format!("{name} ({counter})"),
    }
}

fn owned_remote_temp(destination: &str, job_id: &str) -> AppResult<String> {
    let parent = remote_parent(destination)
        .ok_or_else(|| AppError::Validation("remote destination has no parent".to_owned()))?;
    let name = remote_file_name(destination)
        .ok_or_else(|| AppError::Validation("remote destination has no name".to_owned()))?;
    Ok(remote_join(
        &parent,
        &format!(".{name}.remotedeck-{job_id}.part"),
    ))
}

fn owned_remote_backup(destination: &str, job_id: &str) -> AppResult<String> {
    let parent = remote_parent(destination)
        .ok_or_else(|| AppError::Validation("remote destination has no parent".to_owned()))?;
    let name = remote_file_name(destination)
        .ok_or_else(|| AppError::Validation("remote destination has no name".to_owned()))?;
    Ok(remote_join(
        &parent,
        &format!(".{name}.remotedeck-{job_id}.backup"),
    ))
}

fn is_owned_remote_temp(path: &str) -> bool {
    let Some(name) = remote_file_name(path) else {
        return false;
    };
    let Some((_, suffix)) = name.rsplit_once(".remotedeck-") else {
        return false;
    };
    let Some(uuid) = suffix.strip_suffix(".part") else {
        return false;
    };
    Uuid::parse_str(uuid).is_ok()
}

fn is_owned_remote_backup(path: &str) -> bool {
    let Some(name) = remote_file_name(path) else {
        return false;
    };
    let Some((_, suffix)) = name.rsplit_once(".remotedeck-") else {
        return false;
    };
    let Some(uuid) = suffix.strip_suffix(".backup") else {
        return false;
    };
    Uuid::parse_str(uuid).is_ok()
}

fn owned_local_temp(destination: &Path, job_id: &str) -> AppResult<PathBuf> {
    let parent = destination
        .parent()
        .ok_or_else(|| AppError::Validation("local destination has no parent".to_owned()))?;
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            AppError::Validation("local destination name is not valid Unicode".to_owned())
        })?;
    Ok(parent.join(format!(".{name}.remotedeck-{job_id}.part")))
}

fn owned_local_backup(destination: &Path, job_id: &str) -> AppResult<PathBuf> {
    let parent = destination
        .parent()
        .ok_or_else(|| AppError::Validation("local destination has no parent".to_owned()))?;
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            AppError::Validation("local destination name is not valid Unicode".to_owned())
        })?;
    Ok(parent.join(format!(".{name}.remotedeck-{job_id}.backup")))
}

fn commit_local_transfer(
    job_id: &str,
    temporary: &Path,
    destination: &Path,
    policy: ConflictPolicy,
) -> AppResult<()> {
    if !local_path_exists(destination)? {
        fs::rename(temporary, destination)?;
        return Ok(());
    }
    if policy != ConflictPolicy::Overwrite {
        return Err(AppError::Process(
            "download destination appeared before final rename".to_owned(),
        ));
    }
    let backup = owned_local_backup(destination, job_id)?;
    if local_path_exists(&backup)? {
        return Err(AppError::State(format!(
            "a preserved transfer backup already exists at {}; recover it before retrying",
            backup.display()
        )));
    }
    fs::rename(destination, &backup)?;
    if let Err(replace_error) = fs::rename(temporary, destination) {
        if let Err(rollback_error) = fs::rename(&backup, destination) {
            return Err(AppError::State(format!(
                "replacement failed ({replace_error}); rollback failed ({rollback_error}); the original remains at {}",
                backup.display()
            )));
        }
        return Err(AppError::Io(replace_error));
    }
    if let Err(error) = remove_local_backup_if_owned(&backup) {
        return Err(AppError::State(format!(
            "replacement succeeded but the old destination backup remains at {}: {error}",
            backup.display()
        )));
    }
    Ok(())
}

fn local_tree_size(path: &Path) -> AppResult<u64> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(AppError::Validation(
            "symbolic links are not followed during upload".to_owned(),
        ));
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() {
        return Err(AppError::Validation(
            "upload source is not a regular file or directory".to_owned(),
        ));
    }
    let mut total = 0_u64;
    let mut count = 0_usize;
    let mut path_bytes = path.as_os_str().len();
    let mut stack = vec![(path.to_path_buf(), 0_usize)];
    while let Some((directory, depth)) = stack.pop() {
        if depth > MAX_RECURSION_DEPTH
            || count > MAX_REMOTE_ENTRIES
            || path_bytes > MAX_TREE_PATH_BYTES
        {
            return Err(AppError::Validation(
                "local tree exceeds transfer safety limits".to_owned(),
            ));
        }
        for item in fs::read_dir(directory)? {
            let item = item?;
            count += 1;
            let item_path = item.path();
            path_bytes = path_bytes.saturating_add(item_path.as_os_str().len());
            if count > MAX_REMOTE_ENTRIES || path_bytes > MAX_TREE_PATH_BYTES {
                return Err(AppError::Validation(
                    "local tree exceeds transfer safety limits".to_owned(),
                ));
            }
            let metadata = fs::symlink_metadata(&item_path)?;
            if metadata.file_type().is_symlink() {
                return Err(AppError::Validation(
                    "symbolic links are not followed during upload".to_owned(),
                ));
            }
            if metadata.is_dir() {
                stack.push((item_path, depth + 1));
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

fn remove_local_temp_if_owned(path: &Path) -> AppResult<()> {
    if !is_owned_local_temp(path) {
        return Err(AppError::State(
            "refusing to remove a transfer path that is not app-owned".to_owned(),
        ));
    }
    remove_local_path(path)
}

fn remove_local_backup_if_owned(path: &Path) -> AppResult<()> {
    if !is_owned_local_backup(path) {
        return Err(AppError::State(
            "refusing to remove a transfer backup that is not app-owned".to_owned(),
        ));
    }
    remove_local_path(path)
}

fn is_owned_local_temp(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some((_, suffix)) = name.rsplit_once(".remotedeck-") else {
        return false;
    };
    let Some(uuid) = suffix.strip_suffix(".part") else {
        return false;
    };
    Uuid::parse_str(uuid).is_ok()
}

fn is_owned_local_backup(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some((_, suffix)) = name.rsplit_once(".remotedeck-") else {
        return false;
    };
    let Some(uuid) = suffix.strip_suffix(".backup") else {
        return false;
    };
    Uuid::parse_str(uuid).is_ok()
}

fn local_path_exists(path: &Path) -> AppResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(AppError::Io(error)),
    }
}

fn remove_local_path(path: &Path) -> AppResult<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(AppError::Io(error)),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn transition(current: TransferState, target: TransferState) -> AppResult<TransferState> {
    let allowed = matches!(
        (current, target),
        (TransferState::Queued, TransferState::Running)
            | (TransferState::Queued, TransferState::Cancelling)
            | (TransferState::Running, TransferState::Cancelling)
            | (TransferState::Running, TransferState::Completed)
            | (TransferState::Running, TransferState::Failed)
            | (TransferState::Cancelling, TransferState::Cancelled)
            | (TransferState::Failed, TransferState::Queued)
            | (TransferState::Cancelled, TransferState::Queued)
    );
    if !allowed {
        return Err(AppError::State(format!(
            "invalid transfer state transition: {current:?} -> {target:?}"
        )));
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AuthMethod, SshAdvancedOptions};

    #[test]
    fn batch_quote_preserves_unicode_spaces_and_blocks_newlines() {
        assert_eq!(
            batch_quote(r#"C:\Users\name\资料 [1].txt"#).expect("quote"),
            r#""C:\\Users\\name\\资料 \[1\].txt""#
        );
        assert!(batch_quote("safe\nrm /").is_err());
        assert_eq!(
            batch_command("rename", ["/tmp/a b", "/tmp/中 文"]).expect("command"),
            "rename \"/tmp/a b\" \"/tmp/中 文\"\n"
        );
    }

    #[test]
    fn parses_long_listing_without_losing_spaces_or_unicode() {
        let listing = concat!(
            "drwxr-xr-x    2 1000     1000         4096 Aug  1 12:34 中文 目录\n",
            "-rw-r--r--    1 1000     1000           17 Jul 31  2026 report final.txt\n",
            "lrwxrwxrwx    1 1000     1000            6 Jul 30  2026 current -> target\n"
        );
        let entries = parse_long_listing(listing, "/home/test").expect("parse");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "中文 目录");
        assert_eq!(entries[0].path, "/home/test/中文 目录");
        assert_eq!(entries[0].kind, SftpEntryKind::Directory);
        assert_eq!(entries[1].name, "report final.txt");
        assert_eq!(entries[1].size, 17);
        assert!(
            entries[1]
                .modified_at
                .as_deref()
                .is_some_and(|value| value.contains("2026-07-31"))
        );
        assert_eq!(entries[1].permissions.as_deref(), Some("-rw-r--r--"));
        assert_eq!(entries[2].name, "current");
        assert_eq!(entries[2].kind, SftpEntryKind::Symlink);
    }

    #[test]
    fn metadata_uses_supported_parent_listing_and_selects_the_exact_entry() {
        let (query_path, target_name) = metadata_listing_target("/tmp/project data/");
        assert_eq!(query_path, "/tmp");
        assert_eq!(target_name.as_deref(), Some("project data"));
        let script = batch_command("ls -lan", [&query_path]).expect("metadata listing");
        assert_eq!(script, "ls -lan \"/tmp\"\n");
        assert!(!script.contains(" -d"));

        let listing = concat!(
            "-rw-r--r-- 1 1000 1000 2 Aug 1 12:34 project\n",
            "drwxr-xr-x 2 1000 1000 0 Aug 1 12:34 project data\n"
        );
        let entries = parse_long_listing(listing, &query_path).expect("parse parent listing");
        let entry = select_metadata_entry(
            entries,
            target_name.as_deref().expect("target name"),
            "/tmp/project data/",
        )
        .expect("select metadata")
        .expect("target exists");
        assert_eq!(entry.name, "project data");
        assert_eq!(entry.path, "/tmp/project data/");
        assert_eq!(entry.kind, SftpEntryKind::Directory);

        assert_eq!(metadata_listing_target("relative.txt").0, ".");
        assert_eq!(metadata_listing_target("~/").1, None);
        assert_eq!(metadata_listing_target("/").1, None);
    }

    #[test]
    fn rejects_listing_amplification_beyond_entry_budget() {
        let row = "-rw-r--r-- 1 1000 1000 1 Aug 1 12:34 file\n";
        let listing = row.repeat(MAX_REMOTE_ENTRIES + 1);
        assert!(matches!(
            parse_long_listing(&listing, "/tmp"),
            Err(AppError::Validation(message)) if message.contains("listing exceeds")
        ));
    }

    #[test]
    fn destructive_operations_reject_roots_and_ambiguous_paths() {
        for path in [
            "/",
            "//",
            "~",
            "~/",
            ".",
            "..",
            "/tmp/..",
            "~/a/..",
            "relative",
            " /tmp/project",
            "/tmp/project ",
        ] {
            assert!(
                validate_destructive_remote_path(path).is_err(),
                "{path} should be rejected"
            );
        }
        assert!(validate_destructive_remote_path("/tmp/project").is_ok());
        assert!(validate_destructive_remote_path("~/project").is_ok());
    }

    #[test]
    fn conflict_names_preserve_only_the_final_extension() {
        assert_eq!(
            conflict_name("archive.tar.gz", 2, false),
            "archive.tar (2).gz"
        );
        assert_eq!(conflict_name(".bashrc", 1, false), ".bashrc (1)");
        assert_eq!(conflict_name("folder", 3, true), "folder (3)");
        assert_eq!(
            conflict_remote_path("/tmp/archive.tar.gz", 4, false).expect("candidate"),
            "/tmp/archive.tar (4).gz"
        );
    }

    #[test]
    fn frontend_contract_uses_camel_case_and_ask_requires_a_choice() {
        assert_eq!(
            serde_json::to_value(ConflictPolicy::Ask).expect("serialize policy"),
            serde_json::json!("ask")
        );
        let listing = SftpListResult {
            host_id: "host-1".to_owned(),
            path: "/data".to_owned(),
            parent_path: Some("/".to_owned()),
            entries: Vec::new(),
        };
        assert_eq!(
            serde_json::to_value(listing).expect("serialize listing"),
            serde_json::json!({
                "hostId": "host-1",
                "path": "/data",
                "parentPath": "/",
                "entries": []
            })
        );

        let directory =
            std::env::temp_dir().join(format!("remotedeck-conflict-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create conflict directory");
        let existing = directory.join("result.dat");
        fs::write(&existing, b"existing").expect("create existing destination");
        assert!(matches!(
            resolve_local_conflict(&existing, ConflictPolicy::Ask, false),
            Err(AppError::State(message)) if message.contains("choose overwrite, skip, or rename")
        ));
        fs::remove_dir_all(directory).expect("remove conflict directory");
    }

    #[test]
    fn owned_temp_detection_requires_a_valid_uuid_suffix() {
        let id = Uuid::new_v4().to_string();
        let temporary = owned_remote_temp("/tmp/result.dat", &id).expect("temp");
        assert!(is_owned_remote_temp(&temporary));
        assert!(!is_owned_remote_temp(
            "/tmp/.result.dat.remotedeck-not-a-uuid.part"
        ));
        let backup = owned_remote_backup("/tmp/result.dat", &id).expect("backup");
        assert!(is_owned_remote_backup(&backup));
        assert!(!is_owned_remote_backup(&temporary));
    }

    #[test]
    fn local_overwrite_rollback_preserves_the_original() {
        let directory = std::env::temp_dir().join(format!("remotedeck-commit-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create directory");
        let destination = directory.join("result.dat");
        fs::write(&destination, b"original").expect("write original");
        let missing_temporary = directory.join("missing.part");
        let id = Uuid::new_v4().to_string();
        assert!(
            commit_local_transfer(
                &id,
                &missing_temporary,
                &destination,
                ConflictPolicy::Overwrite,
            )
            .is_err()
        );
        assert_eq!(
            fs::read(&destination).expect("original restored"),
            b"original"
        );
        assert!(
            !owned_local_backup(&destination, &id)
                .expect("backup")
                .exists()
        );

        let temporary = owned_local_temp(&destination, &id).expect("temporary");
        fs::write(&temporary, b"replacement").expect("write replacement");
        commit_local_transfer(&id, &temporary, &destination, ConflictPolicy::Overwrite)
            .expect("commit replacement");
        assert_eq!(fs::read(&destination).expect("replacement"), b"replacement");
        assert!(
            !owned_local_backup(&destination, &id)
                .expect("backup")
                .exists()
        );
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn cancellation_state_machine_is_explicit_and_retry_is_bounded() {
        assert_eq!(
            transition(TransferState::Queued, TransferState::Running).expect("run"),
            TransferState::Running
        );
        assert_eq!(
            transition(TransferState::Running, TransferState::Cancelling).expect("cancel"),
            TransferState::Cancelling
        );
        assert_eq!(
            transition(TransferState::Cancelling, TransferState::Cancelled).expect("cancelled"),
            TransferState::Cancelled
        );
        assert_eq!(
            transition(TransferState::Cancelled, TransferState::Queued).expect("retry"),
            TransferState::Queued
        );
        assert!(transition(TransferState::Completed, TransferState::Queued).is_err());
        assert!(transition(TransferState::Running, TransferState::Queued).is_err());
    }

    #[tokio::test]
    async fn completion_notification_cannot_be_lost_during_shutdown() {
        let control = Arc::new(TransferControl::default());
        let waiter = {
            let control = control.clone();
            tokio::spawn(async move { control.wait_finished().await })
        };
        tokio::task::yield_now().await;
        control.mark_finished();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("completion notification")
            .expect("waiter task");

        let already_finished = control.wait_finished();
        tokio::time::timeout(Duration::from_millis(50), already_finished)
            .await
            .expect("finished flag is persistent");
    }

    #[test]
    fn retry_rebinds_to_the_current_host_profile() {
        let old_host = test_host("host-1", "old.example");
        let mut current_host = old_host.clone();
        current_host.hostname = "new.example".to_owned();
        current_host.username = "new-user".to_owned();
        let mut entry = failed_download_entry(old_host);

        let retried = prepare_transfer_retry(&mut entry, current_host.clone()).expect("retry");
        assert_eq!(retried.state, TransferState::Queued);
        assert_eq!(entry.host.hostname, "new.example");
        assert_eq!(entry.host.username, "new-user");

        let different_host = test_host("host-2", "other.example");
        assert!(prepare_transfer_retry(&mut entry, different_host).is_err());
    }

    #[tokio::test]
    async fn deleting_a_host_purges_retryable_jobs_and_retires_new_work() {
        let runtime = SshRuntime::discover(
            std::env::temp_dir().join(format!("remotedeck-known-hosts-{}", Uuid::new_v4())),
        );
        let registry = TransferRegistry::new(SftpService::new(runtime));
        let host = test_host("host-1", "old.example");
        let entry = failed_download_entry(host.clone());
        entry.control.mark_finished();
        registry.entries.write().insert(entry.job.id.clone(), entry);

        registry.remove_host(&host.id).await.expect("retire host");
        assert!(registry.list().is_empty());
        assert!(registry.retired_hosts.read().contains(&host.id));
        let request = DownloadRequest {
            remote_path: "/tmp/result.dat".to_owned(),
            local_path: std::env::temp_dir()
                .join("result.dat")
                .to_string_lossy()
                .into_owned(),
            conflict_policy: ConflictPolicy::Rename,
            recursive: false,
        };
        assert!(registry.start_download(host.clone(), request).is_err());

        registry.restore_host(&host.id);
        assert!(!registry.retired_hosts.read().contains(&host.id));
    }

    #[test]
    fn active_transfer_blocks_direct_and_proxy_jump_connection_edits() {
        let runtime = SshRuntime::discover(
            std::env::temp_dir().join(format!("remotedeck-known-hosts-{}", Uuid::new_v4())),
        );
        let registry = TransferRegistry::new(SftpService::new(runtime));
        let jump = test_host("jump-1", "jump-a.example");
        let mut target = test_host("target-1", "target.example");
        target.proxy_jump = Some(jump.id.clone());
        let mut entry = failed_download_entry(target.clone());
        entry.job.state = TransferState::Queued;
        entry.job.completed_at = None;
        registry.entries.write().insert(entry.job.id.clone(), entry);

        let mut edited_jump = jump.clone();
        edited_jump.hostname = "jump-b.example".to_owned();
        let persisted = AtomicBool::new(false);
        assert!(matches!(
            registry.apply_host_update(&jump, &edited_jump, || {
                persisted.store(true, Ordering::Release);
                Ok(())
            }),
            Err(AppError::State(message)) if message.contains("active transfers")
        ));
        assert!(!persisted.load(Ordering::Acquire));

        let mut edited_target = target.clone();
        edited_target.hostname = "target-b.example".to_owned();
        assert!(
            registry
                .apply_host_update(&target, &edited_target, || Ok(()))
                .is_err()
        );

        let mut alias_only = jump.clone();
        alias_only.alias = "renamed jump".to_owned();
        registry
            .apply_host_update(&jump, &alias_only, || Ok(()))
            .expect("non-connection metadata remains editable");
    }

    fn failed_download_entry(host: HostProfile) -> TransferEntry {
        let now = Utc::now();
        let id = Uuid::new_v4().to_string();
        TransferEntry {
            job: TransferJob {
                id,
                host_id: host.id.clone(),
                direction: TransferDirection::Download,
                source: "/tmp/result.dat".to_owned(),
                destination: std::env::temp_dir()
                    .join("result.dat")
                    .to_string_lossy()
                    .into_owned(),
                conflict_policy: ConflictPolicy::Rename,
                recursive: false,
                state: TransferState::Failed,
                attempts: 1,
                bytes_transferred: 0,
                total_bytes: None,
                skipped: false,
                created_at: now,
                updated_at: now,
                completed_at: Some(now),
                error: Some("connection failed".to_owned()),
            },
            host,
            operation: TransferOperation::Download(DownloadRequest {
                remote_path: "/tmp/result.dat".to_owned(),
                local_path: std::env::temp_dir()
                    .join("result.dat")
                    .to_string_lossy()
                    .into_owned(),
                conflict_policy: ConflictPolicy::Rename,
                recursive: false,
            }),
            control: Arc::new(TransferControl::default()),
        }
    }

    fn test_host(id: &str, hostname: &str) -> HostProfile {
        let now = Utc::now();
        HostProfile {
            schema_version: 2,
            id: id.to_owned(),
            alias: hostname.to_owned(),
            hostname: hostname.to_owned(),
            port: 22,
            username: "researcher".to_owned(),
            auth_method: AuthMethod::Interactive,
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
}
