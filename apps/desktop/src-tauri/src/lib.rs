mod agent;
mod agent_service;
mod command_job;
mod diagnostics;
mod error;
mod host_operation;
mod key_service;
mod keys;
mod lifecycle;
mod migration;
mod model;
#[cfg(all(test, feature = "openssh-integration"))]
mod openssh_integration;
mod risk;
mod session;
mod ssh;
mod store;
mod telemetry;
mod tunnel;

use crate::{
    agent::{AgentAction, AgentCommandPlan, PlanMode},
    agent_service::{AgentSessionRequest, AgentSessionResult, AgentStatus},
    command_job::{
        CommandAnalysis, CommandJob, CommandJobError, CommandJobRegistry, CommandRunRequest,
        ResolvedCommand, analyze_resolved_command, resolve_command_request,
        resolve_repository_request,
    },
    error::{AppError, AppResult},
    key_service::{
        KeyDeployRequest, KeyGenerateRequest, KeyOperationResult, PrivateKeyRecord, SshImportResult,
    },
    model::{
        AppSettings, BootstrapPayload, CommandPreset, CommandPresetDraft, CommandRisk,
        ConnectionTestResult, HostDraft, HostKeyCandidate, HostProfile, SettingsPatch,
        TerminalSnapshot, TunnelDraft, TunnelProfile, TunnelSnapshot,
    },
    session::{TerminalCommandOptions, TerminalInputEndpoint, TerminalRegistry},
    ssh::sftp::{
        ConflictPolicy, DownloadRequest, SftpDeleteRequest, SftpListRequest, SftpListResult,
        SftpMkdirRequest, SftpRenameRequest, SftpService, TransferDirection, TransferJob,
        TransferRegistry, UploadRequest,
    },
    ssh::{SshRuntime, TrustedHostKey},
    store::AppRepository,
    telemetry::{
        BtopStatus, ProcessSignalRequest, TelemetryOptions, TelemetryRegistry, TelemetrySnapshot,
        TelemetryStatus,
    },
    tunnel::TunnelRegistry,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

const COMMAND_CONCURRENCY: usize = 8;
const APPLICATION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(12);

struct AppState {
    repository: AppRepository,
    ssh: SshRuntime,
    terminals: TerminalRegistry,
    sftp: SftpService,
    transfers: TransferRegistry,
    tunnels: TunnelRegistry,
    telemetry: TelemetryRegistry,
    command_jobs: CommandJobRegistry,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostKeyRecord {
    id: String,
    host_id: Option<String>,
    host_token: String,
    hostname: String,
    port: u16,
    algorithm: String,
    public_key_base64: String,
    sha256_fingerprint: String,
    accepted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TransferRequest {
    host_id: String,
    source: String,
    destination: String,
    conflict_policy: ConflictPolicy,
    recursive: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandDefinition {
    id: String,
    host_id: Option<String>,
    name: String,
    description: String,
    group: String,
    command: String,
    working_directory: Option<String>,
    risk: CommandRisk,
    requires_pty: bool,
    requires_sudo: bool,
    confirmation_text: Option<String>,
    sort_order: i32,
    builtin: bool,
}

#[tauri::command]
fn bootstrap(state: State<'_, AppState>) -> BootstrapPayload {
    let snapshot = state.repository.snapshot();
    BootstrapPayload {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        settings: snapshot.settings,
        hosts: snapshot.hosts,
        tunnels: snapshot.tunnels,
        capabilities: state.ssh.capabilities(),
    }
}

#[tauri::command]
fn update_settings(state: State<'_, AppState>, patch: SettingsPatch) -> AppResult<AppSettings> {
    let current = state.repository.snapshot().settings;
    let launch_change = launch_at_login_change(&current, &patch);
    if let Some(enabled) = launch_change {
        lifecycle::set_launch_at_login(enabled)
            .map_err(|error| AppError::State(error.to_string()))?;
    }
    match state.repository.update_settings(patch) {
        Ok(settings) => Ok(settings),
        Err(error) => {
            if let Some(enabled) = launch_change {
                let _ = lifecycle::set_launch_at_login(!enabled);
            }
            Err(error)
        }
    }
}

fn launch_at_login_change(current: &AppSettings, patch: &SettingsPatch) -> Option<bool> {
    patch
        .launch_at_login
        .filter(|enabled| *enabled != current.launch_at_login)
}

#[tauri::command]
async fn pick_local_path(directory: bool) -> Option<String> {
    let dialog = rfd::AsyncFileDialog::new().set_title(if directory {
        "Select a local folder"
    } else {
        "Select a local file"
    });
    let selection = if directory {
        dialog.pick_folder().await
    } else {
        dialog.pick_file().await
    };
    selection.map(|handle| handle.path().to_string_lossy().into_owned())
}

#[tauri::command]
async fn pick_save_path(suggested_name: String) -> AppResult<Option<String>> {
    validate_suggested_filename(&suggested_name)?;
    Ok(rfd::AsyncFileDialog::new()
        .set_title("Select a local destination")
        .set_file_name(suggested_name.trim())
        .save_file()
        .await
        .map(|handle| handle.path().to_string_lossy().into_owned()))
}

fn validate_suggested_filename(value: &str) -> AppResult<()> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 255
        || matches!(value, "." | "..")
        || value.chars().any(char::is_control)
        || Path::new(value).file_name().and_then(|name| name.to_str()) != Some(value)
    {
        return Err(AppError::Validation(
            "suggested file name must be one local file name".to_owned(),
        ));
    }
    Ok(())
}

#[tauri::command]
fn save_host(state: State<'_, AppState>, draft: HostDraft) -> AppResult<HostProfile> {
    let Some(host_id) = draft.id.as_deref() else {
        return state.repository.save_host(draft);
    };
    let previous = state.repository.host(host_id)?;
    let preview = state.repository.preview_host(draft.clone())?;
    state.transfers.apply_host_update(&previous, &preview, || {
        state.telemetry.apply_host_update(&previous, &preview, || {
            state
                .tunnels
                .apply_host_update(&previous, &preview, || state.repository.save_host(draft))
        })
    })
}

#[tauri::command]
async fn delete_host(app: AppHandle, host_id: String) -> AppResult<()> {
    let (repository, terminals, tunnels, telemetry, transfers, command_jobs) = {
        let state = app.state::<AppState>();
        (
            state.repository.clone(),
            state.terminals.clone(),
            state.tunnels.clone(),
            state.telemetry.clone(),
            state.transfers.clone(),
            state.command_jobs.clone(),
        )
    };
    repository.ensure_host_deletable(&host_id)?;
    if let Err(error) = transfers.remove_host(&host_id).await {
        transfers.restore_host(&host_id);
        return Err(error);
    }
    if let Err(error) = terminals.retire_host(&host_id).await {
        terminals.restore_host(&host_id);
        transfers.restore_host(&host_id);
        return Err(error);
    }
    if let Err(error) = command_jobs.retire_host(&host_id).await {
        command_jobs.restore_host(&host_id);
        terminals.restore_host(&host_id);
        transfers.restore_host(&host_id);
        return Err(command_error(error));
    }
    if let Err(error) = repository.begin_host_deletion(&host_id) {
        command_jobs.restore_host(&host_id);
        terminals.restore_host(&host_id);
        transfers.restore_host(&host_id);
        return Err(error);
    }
    let mut cleanup_errors = Vec::new();
    if let Err(error) = tunnels.stop_for_host(&app, &host_id) {
        cleanup_errors.push(format!("tunnels: {error}"));
    }
    if let Err(error) = telemetry.remove_host(&app, &host_id).await {
        cleanup_errors.push(format!("telemetry: {error}"));
    }
    if !cleanup_errors.is_empty() {
        repository.abort_host_deletion(&host_id);
        command_jobs.restore_host(&host_id);
        terminals.restore_host(&host_id);
        transfers.restore_host(&host_id);
        return Err(AppError::Process(format!(
            "host deletion was cancelled because runtime cleanup failed: {}",
            cleanup_errors.join("; ")
        )));
    }
    match repository.finish_host_deletion(&host_id) {
        Ok(()) => Ok(()),
        Err(error) => {
            command_jobs.restore_host(&host_id);
            terminals.restore_host(&host_id);
            transfers.restore_host(&host_id);
            Err(error)
        }
    }
}

#[tauri::command]
fn import_ssh_config(
    state: State<'_, AppState>,
    config_path: String,
) -> AppResult<SshImportResult> {
    key_service::import_ssh_config(&state.repository, &config_path)
}

#[tauri::command]
async fn scan_host_keys(app: AppHandle, host_id: String) -> AppResult<Vec<HostKeyCandidate>> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    ssh.scan_host_keys(&host).await
}

#[tauri::command]
async fn accept_host_key(
    app: AppHandle,
    host_id: String,
    candidate: HostKeyCandidate,
) -> AppResult<()> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    ssh.accept_host_key(&host, &candidate).await
}

#[tauri::command]
async fn list_host_keys(app: AppHandle) -> AppResult<Vec<HostKeyRecord>> {
    let (repository, ssh) = {
        let state = app.state::<AppState>();
        (state.repository.clone(), state.ssh.clone())
    };
    trusted_key_records(&repository, &ssh).await
}

#[tauri::command]
async fn remove_host_key(app: AppHandle, record_id: String) -> AppResult<()> {
    let (repository, ssh) = {
        let state = app.state::<AppState>();
        (state.repository.clone(), state.ssh.clone())
    };
    let record = trusted_key_records(&repository, &ssh)
        .await?
        .into_iter()
        .find(|record| record.id == record_id)
        .ok_or_else(|| AppError::NotFound(format!("trusted host key {record_id}")))?;
    if ssh
        .remove_trusted_key_record(
            &record.host_token,
            &record.algorithm,
            &record.public_key_base64,
        )
        .await?
    {
        Ok(())
    } else {
        Err(AppError::NotFound(format!(
            "trusted host key record {record_id}"
        )))
    }
}

#[tauri::command]
async fn test_connection(app: AppHandle, host_id: String) -> AppResult<ConnectionTestResult> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    ssh.test_connection(&host).await
}

#[tauri::command]
async fn list_keys() -> AppResult<Vec<PrivateKeyRecord>> {
    key_service::list_private_keys().await
}

#[tauri::command]
async fn generate_key(request: KeyGenerateRequest) -> AppResult<KeyOperationResult> {
    key_service::generate_key(request).await
}

#[tauri::command]
async fn deploy_key(app: AppHandle, request: KeyDeployRequest) -> AppResult<KeyOperationResult> {
    let (repository, ssh) = {
        let state = app.state::<AppState>();
        (state.repository.clone(), state.ssh.clone())
    };
    key_service::deploy_key(&repository, &ssh, request).await
}

#[tauri::command]
fn list_terminals(state: State<'_, AppState>) -> Vec<TerminalSnapshot> {
    state.terminals.list()
}

#[tauri::command]
fn start_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
    host_id: String,
    rows: u16,
    cols: u16,
) -> AppResult<TerminalSnapshot> {
    let host = state.repository.host(&host_id)?;
    state.terminals.start(app, &host, &state.ssh, rows, cols)
}

#[tauri::command]
fn reconnect_terminal(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    rows: u16,
    cols: u16,
) -> AppResult<TerminalSnapshot> {
    let snapshot = state
        .terminals
        .list()
        .into_iter()
        .find(|snapshot| snapshot.session_id == session_id)
        .ok_or_else(|| AppError::NotFound(format!("terminal session {session_id}")))?;
    let host = state.repository.host(&snapshot.host_id)?;
    state
        .terminals
        .reconnect(app, &session_id, &host, &state.ssh, rows, cols)
}

#[tauri::command]
fn open_terminal_input(
    state: State<'_, AppState>,
    session_id: String,
) -> AppResult<TerminalInputEndpoint> {
    state.terminals.open_input(&session_id)
}

#[tauri::command]
fn resize_terminal(
    state: State<'_, AppState>,
    session_id: String,
    rows: u16,
    cols: u16,
) -> AppResult<()> {
    state.terminals.resize(&session_id, rows, cols)
}

#[tauri::command]
fn close_terminal(state: State<'_, AppState>, session_id: String) -> AppResult<()> {
    state.terminals.close(&session_id)
}

#[tauri::command]
async fn sftp_list(app: AppHandle, host_id: String, path: String) -> AppResult<SftpListResult> {
    let (host, service) = host_and_sftp(&app, &host_id)?;
    let _operation = service.begin_host_operation(&host.id)?;
    service
        .list(
            &host,
            SftpListRequest {
                path,
                show_hidden: true,
            },
        )
        .await
}

#[tauri::command]
async fn sftp_create_directory(app: AppHandle, host_id: String, path: String) -> AppResult<()> {
    let (host, service) = host_and_sftp(&app, &host_id)?;
    let _operation = service.begin_host_operation(&host.id)?;
    service.mkdir(&host, SftpMkdirRequest { path }).await?;
    Ok(())
}

#[tauri::command]
async fn sftp_rename(
    app: AppHandle,
    host_id: String,
    source_path: String,
    destination_path: String,
) -> AppResult<()> {
    let (host, service) = host_and_sftp(&app, &host_id)?;
    let _operation = service.begin_host_operation(&host.id)?;
    service
        .rename(
            &host,
            SftpRenameRequest {
                source: source_path,
                destination: destination_path,
            },
        )
        .await?;
    Ok(())
}

#[tauri::command]
async fn sftp_delete(
    app: AppHandle,
    host_id: String,
    path: String,
    recursive: bool,
) -> AppResult<()> {
    let (host, service) = host_and_sftp(&app, &host_id)?;
    let _operation = service.begin_host_operation(&host.id)?;
    service
        .delete(&host, SftpDeleteRequest { path, recursive })
        .await?;
    Ok(())
}

#[tauri::command]
fn transfer_list(state: State<'_, AppState>, host_id: Option<String>) -> Vec<TransferJob> {
    state
        .transfers
        .list()
        .into_iter()
        .filter(|job| {
            host_id
                .as_ref()
                .is_none_or(|host_id| job.host_id == *host_id)
        })
        .collect()
}

#[tauri::command]
async fn transfer_upload(app: AppHandle, request: TransferRequest) -> AppResult<TransferJob> {
    let (host, transfers) = {
        let state = app.state::<AppState>();
        (
            state.repository.host(&request.host_id)?,
            state.transfers.clone(),
        )
    };
    let name = Path::new(&request.source)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::Validation("upload source has no file name".to_owned()))?;
    transfers.start_upload(
        host,
        UploadRequest {
            local_path: request.source,
            remote_path: remote_join(&request.destination, &name),
            conflict_policy: request.conflict_policy,
            recursive: request.recursive,
        },
    )
}

#[tauri::command]
async fn transfer_download(app: AppHandle, request: TransferRequest) -> AppResult<TransferJob> {
    let (host, transfers) = {
        let state = app.state::<AppState>();
        (
            state.repository.host(&request.host_id)?,
            state.transfers.clone(),
        )
    };
    let name = remote_basename(&request.source)
        .ok_or_else(|| AppError::Validation("download source has no file name".to_owned()))?;
    let destination = safe_download_destination(&request.destination, name)?;
    transfers.start_download(
        host,
        DownloadRequest {
            remote_path: request.source,
            local_path: destination.to_string_lossy().into_owned(),
            conflict_policy: request.conflict_policy,
            recursive: request.recursive,
        },
    )
}

#[tauri::command]
async fn transfer_cancel(state: State<'_, AppState>, job_id: String) -> AppResult<TransferJob> {
    state.transfers.cancel(&job_id).await
}

#[tauri::command]
fn transfer_retry(state: State<'_, AppState>, job_id: String) -> AppResult<TransferJob> {
    let job = state.transfers.get(&job_id)?;
    let host = state.repository.host(&job.host_id)?;
    state.transfers.retry(&job_id, host)
}

#[tauri::command]
fn transfer_show_in_folder(state: State<'_, AppState>, job_id: String) -> AppResult<()> {
    let job = state.transfers.get(&job_id)?;
    let local = match job.direction {
        TransferDirection::Upload => job.source,
        TransferDirection::Download => job.destination,
    };
    let path = PathBuf::from(local);
    if !path.exists() {
        return Err(AppError::NotFound(
            "the local transfer path no longer exists".to_owned(),
        ));
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer.exe")
            .arg(format!("/select,{}", path.to_string_lossy()))
            .spawn()?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Err(AppError::State(
            "show in folder is supported only on Windows".to_owned(),
        ))
    }
}

#[tauri::command]
fn list_tunnels(state: State<'_, AppState>, host_id: Option<String>) -> Vec<TunnelSnapshot> {
    state
        .repository
        .snapshot()
        .tunnels
        .into_iter()
        .filter(|tunnel| {
            host_id
                .as_ref()
                .is_none_or(|host_id| tunnel.host_id == *host_id)
        })
        .map(|tunnel| state.tunnels.snapshot_for(&tunnel))
        .collect()
}

#[tauri::command]
fn save_tunnel(state: State<'_, AppState>, draft: TunnelDraft) -> AppResult<TunnelProfile> {
    state.repository.save_tunnel(draft)
}

#[tauri::command]
fn delete_tunnel(app: AppHandle, state: State<'_, AppState>, tunnel_id: String) -> AppResult<()> {
    state.tunnels.remove(&app, &tunnel_id)?;
    state.repository.delete_tunnel(&tunnel_id)
}

#[tauri::command]
fn start_tunnel(
    app: AppHandle,
    state: State<'_, AppState>,
    tunnel_id: String,
) -> AppResult<TunnelSnapshot> {
    let tunnel = state.repository.tunnel(&tunnel_id)?;
    let host = state.repository.host(&tunnel.host_id)?;
    state.tunnels.start(app, &host, &tunnel, &state.ssh)
}

#[tauri::command]
fn stop_tunnel(
    app: AppHandle,
    state: State<'_, AppState>,
    tunnel_id: String,
) -> AppResult<TunnelSnapshot> {
    state.tunnels.stop(&app, &tunnel_id)?;
    let tunnel = state.repository.tunnel(&tunnel_id)?;
    Ok(state.tunnels.snapshot_for(&tunnel))
}

#[tauri::command]
fn restart_tunnel(
    app: AppHandle,
    state: State<'_, AppState>,
    tunnel_id: String,
) -> AppResult<TunnelSnapshot> {
    let tunnel = state.repository.tunnel(&tunnel_id)?;
    let host = state.repository.host(&tunnel.host_id)?;
    state.tunnels.restart(app, &host, &tunnel, &state.ssh)
}

#[tauri::command]
fn telemetry_list(state: State<'_, AppState>) -> Vec<TelemetryStatus> {
    state.telemetry.list()
}

#[tauri::command]
fn telemetry_history(state: State<'_, AppState>, host_id: String) -> Vec<TelemetrySnapshot> {
    state.telemetry.history(&host_id)
}

#[tauri::command]
fn telemetry_start(
    app: AppHandle,
    state: State<'_, AppState>,
    host_id: String,
) -> AppResult<TelemetryStatus> {
    let host = state.repository.host(&host_id)?;
    let settings = state.repository.snapshot().settings;
    state.telemetry.start(
        app,
        host,
        state.ssh.clone(),
        TelemetryOptions {
            interval_seconds: settings.telemetry_interval_seconds,
            retention_minutes: settings.telemetry_retention_minutes,
            auto_reconnect: settings.auto_reconnect,
        },
    )
}

#[tauri::command]
async fn telemetry_stop(
    app: AppHandle,
    state: State<'_, AppState>,
    host_id: String,
) -> AppResult<TelemetryStatus> {
    state.repository.host(&host_id)?;
    state.telemetry.stop(&app, &host_id).await
}

#[tauri::command]
async fn signal_process(
    app: AppHandle,
    host_id: String,
    pid: u32,
    expected_start_ticks: u64,
    expected_user: String,
    expected_command: String,
    signal: String,
) -> AppResult<()> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    let state = app.state::<AppState>();
    state
        .telemetry
        .signal_process(
            &ssh,
            &host,
            ProcessSignalRequest {
                pid,
                expected_start_ticks,
                expected_user: &expected_user,
                expected_command: &expected_command,
                signal: &signal,
            },
        )
        .await
}

#[tauri::command]
async fn btop_probe(app: AppHandle, host_id: String) -> AppResult<BtopStatus> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    let state = app.state::<AppState>();
    let rotation_minutes = state.repository.snapshot().settings.btop_rotation_minutes;
    state
        .telemetry
        .probe_btop(&ssh, &host, rotation_minutes)
        .await
}

#[tauri::command]
async fn btop_start_watchdog(
    app: AppHandle,
    host_id: String,
    rotation_minutes: u64,
) -> AppResult<BtopStatus> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    let state = app.state::<AppState>();
    state
        .telemetry
        .start_btop_watchdog(&ssh, &host, rotation_minutes)
        .await
}

#[tauri::command]
async fn btop_stop_watchdog(app: AppHandle, host_id: String) -> AppResult<BtopStatus> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    let state = app.state::<AppState>();
    let rotation_minutes = state.repository.snapshot().settings.btop_rotation_minutes;
    state
        .telemetry
        .stop_btop_watchdog(&ssh, &host, rotation_minutes)
        .await
}

#[tauri::command]
fn list_commands(state: State<'_, AppState>, host_id: Option<String>) -> Vec<CommandDefinition> {
    let mut definitions = risk::builtin_presets()
        .iter()
        .enumerate()
        .map(|(index, preset)| command_definition_from_builtin(index, preset))
        .collect::<Vec<_>>();
    definitions.extend(
        state
            .repository
            .list_commands(host_id.as_deref())
            .into_iter()
            .map(command_definition_from_preset),
    );
    definitions.sort_by(|left, right| {
        left.sort_order
            .cmp(&right.sort_order)
            .then_with(|| left.name.cmp(&right.name))
    });
    definitions
}

#[tauri::command]
fn save_command(
    state: State<'_, AppState>,
    draft: CommandPresetDraft,
) -> AppResult<CommandDefinition> {
    if draft
        .id
        .as_deref()
        .is_some_and(|id| id.starts_with("builtin:"))
    {
        return Err(AppError::Validation(
            "built-in command presets are immutable".to_owned(),
        ));
    }
    state
        .repository
        .save_command(draft)
        .map(command_definition_from_preset)
}

#[tauri::command]
fn delete_command(
    state: State<'_, AppState>,
    command_id: String,
    host_id: String,
) -> AppResult<()> {
    if command_id.starts_with("builtin:") {
        return Err(AppError::Validation(
            "built-in command presets are immutable".to_owned(),
        ));
    }
    state
        .repository
        .delete_command_for_host(&command_id, &host_id)
}

#[tauri::command]
fn analyze_command(
    state: State<'_, AppState>,
    request: CommandRunRequest,
) -> AppResult<CommandAnalysis> {
    let resolved = resolve_command(&state.repository, &request)?;
    analyze_resolved_command(&resolved).map_err(command_error)
}

#[tauri::command]
async fn run_command(
    app: AppHandle,
    state: State<'_, AppState>,
    request: CommandRunRequest,
) -> AppResult<CommandJob> {
    let resolved = resolve_command(&state.repository, &request)?;
    match state
        .command_jobs
        .run_resolved(&state.ssh, resolved, request.confirmation.as_deref())
    {
        Ok(job) => Ok(job),
        Err(CommandJobError::RequiresPty { plan }) => {
            let host = state.repository.host(&plan.host_id)?;
            let terminal = state.terminals.start_command(
                app,
                &host,
                &state.ssh,
                TerminalCommandOptions {
                    rows: 30,
                    cols: 120,
                    remote_command: plan.command.clone(),
                    title: Some(format!("{} · {}", host.alias, plan.name)),
                },
            )?;
            match state
                .command_jobs
                .record_pty_handoff(&plan, &terminal.session_id)
            {
                Ok(job) => Ok(job),
                Err(error) => {
                    let _ = state.terminals.close(&terminal.session_id);
                    Err(command_error(error))
                }
            }
        }
        Err(error) => Err(command_error(error)),
    }
}

#[tauri::command]
fn list_command_jobs(state: State<'_, AppState>, host_id: Option<String>) -> Vec<CommandJob> {
    state.command_jobs.list(host_id.as_deref())
}

#[tauri::command]
fn cancel_command(state: State<'_, AppState>, job_id: String) -> AppResult<CommandJob> {
    state.command_jobs.cancel(&job_id).map_err(command_error)
}

#[tauri::command]
async fn probe_agent(
    app: AppHandle,
    host_id: String,
    agent: agent::AgentProvider,
) -> AppResult<AgentStatus> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    agent_service::probe_agent(&ssh, &host, agent).await
}

#[tauri::command]
fn agent_session_plan(
    state: State<'_, AppState>,
    request: AgentSessionRequest,
) -> AppResult<AgentCommandPlan> {
    let host = state.repository.host(&request.host_id)?;
    agent_service::session_plan(&host, &request)
}

#[tauri::command]
fn start_agent_session(
    app: AppHandle,
    state: State<'_, AppState>,
    request: AgentSessionRequest,
) -> AppResult<AgentSessionResult> {
    let host = state.repository.host(&request.host_id)?;
    let plan = agent_service::confirmed_session_plan(&host, &request)?;
    if matches!(request.action, AgentAction::Probe) {
        return Err(AppError::Validation(
            "agent probe must use the non-interactive probe command".to_owned(),
        ));
    }
    let command = plan
        .commands
        .iter()
        .find(|command| command.mode == PlanMode::InteractivePty)
        .ok_or_else(|| AppError::State("agent plan has no interactive command".to_owned()))?;
    let terminal = state.terminals.start_command(
        app,
        &host,
        &state.ssh,
        TerminalCommandOptions {
            rows: 30,
            cols: 120,
            remote_command: command.command.clone(),
            title: Some(format!("{} · {}", host.alias, plan.display_name)),
        },
    )?;
    Ok(AgentSessionResult {
        terminal,
        message: plan.notice,
    })
}

#[tauri::command]
fn legacy_preview(
    state: State<'_, AppState>,
    source_path: String,
) -> AppResult<migration::LegacyPreview> {
    migration::preview(&state.repository, &source_path)
}

#[tauri::command]
fn legacy_apply(
    state: State<'_, AppState>,
    request: migration::LegacyApplyRequest,
) -> AppResult<migration::LegacyApplyResult> {
    migration::apply(&state.repository, request)
}

#[tauri::command]
async fn export_diagnostics(
    state: State<'_, AppState>,
) -> AppResult<diagnostics::DiagnosticsExportResult> {
    let destination = rfd::AsyncFileDialog::new()
        .set_title("Export RemoteDeck diagnostics")
        .set_file_name(diagnostics::suggested_filename())
        .add_filter("ZIP archive", &["zip"])
        .save_file()
        .await;
    let Some(destination) = destination else {
        return Ok(diagnostics::DiagnosticsExportResult::cancelled());
    };
    diagnostics::export(&state.repository, &state.ssh, Some(destination.path())).await
}

pub fn run() {
    let application = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            let _ = lifecycle::show_main_window(app);
        }))
        .setup(|app| {
            app.manage(lifecycle::LifecycleState::default());
            let repository = AppRepository::open(app.path().app_data_dir()?)?;
            let ssh = SshRuntime::discover_with_repository(repository.clone());
            let sftp = SftpService::new(ssh.clone());
            let transfers = TransferRegistry::new(sftp.clone());
            let command_jobs = CommandJobRegistry::new(COMMAND_CONCURRENCY)?;
            let tunnels = TunnelRegistry::default();
            let telemetry = TelemetryRegistry::with_owner_nonce(repository.btop_owner_nonce());
            let initial = repository.snapshot();
            app.manage(AppState {
                repository: repository.clone(),
                ssh: ssh.clone(),
                terminals: TerminalRegistry::default(),
                sftp,
                transfers: transfers.clone(),
                tunnels: tunnels.clone(),
                telemetry: telemetry.clone(),
                command_jobs: command_jobs.clone(),
            });
            forward_transfer_events(app.handle(), &transfers);
            forward_command_events(app.handle(), &command_jobs);
            let _ = lifecycle::install_tray(app.handle())?;
            if initial.settings.launch_at_login
                && let Err(error) = lifecycle::set_launch_at_login(true)
            {
                eprintln!("RemoteDeck could not restore launch-at-login: {error}");
            }
            if lifecycle::current_launch_is_hidden() {
                let _ = lifecycle::hide_main_window(app.handle())?;
            }
            for tunnel_profile in initial.tunnels.iter().filter(|tunnel| tunnel.auto_start) {
                if let Ok(host) = repository.host(&tunnel_profile.host_id) {
                    let _ = tunnels.start(app.handle().clone(), &host, tunnel_profile, &ssh);
                }
            }
            let telemetry_options = TelemetryOptions {
                interval_seconds: initial.settings.telemetry_interval_seconds,
                retention_minutes: initial.settings.telemetry_retention_minutes,
                auto_reconnect: initial.settings.auto_reconnect,
            };
            for host in initial.hosts.iter().filter(|host| host.monitor_enabled) {
                if telemetry
                    .start(
                        app.handle().clone(),
                        host.clone(),
                        ssh.clone(),
                        telemetry_options,
                    )
                    .is_ok()
                    && initial.settings.btop_watchdog_enabled
                {
                    let telemetry = telemetry.clone();
                    let ssh = ssh.clone();
                    let host = host.clone();
                    let rotation_minutes = initial.settings.btop_rotation_minutes;
                    tauri::async_runtime::spawn(async move {
                        let _ = telemetry
                            .start_btop_watchdog(&ssh, &host, rotation_minutes)
                            .await;
                    });
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != lifecycle::MAIN_WINDOW_LABEL {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let close_to_tray = app
                    .try_state::<AppState>()
                    .is_some_and(|state| state.repository.snapshot().settings.close_to_tray);
                if let Some(lifecycle) = app.try_state::<lifecycle::LifecycleState>() {
                    let _ = lifecycle::handle_close_requested(app, api, close_to_tray, &lifecycle);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            update_settings,
            pick_local_path,
            pick_save_path,
            save_host,
            delete_host,
            import_ssh_config,
            scan_host_keys,
            accept_host_key,
            list_host_keys,
            remove_host_key,
            test_connection,
            list_keys,
            generate_key,
            deploy_key,
            list_terminals,
            start_terminal,
            reconnect_terminal,
            open_terminal_input,
            resize_terminal,
            close_terminal,
            sftp_list,
            sftp_create_directory,
            sftp_rename,
            sftp_delete,
            transfer_list,
            transfer_upload,
            transfer_download,
            transfer_cancel,
            transfer_retry,
            transfer_show_in_folder,
            list_tunnels,
            save_tunnel,
            delete_tunnel,
            start_tunnel,
            stop_tunnel,
            restart_tunnel,
            telemetry_list,
            telemetry_history,
            telemetry_start,
            telemetry_stop,
            signal_process,
            btop_probe,
            btop_start_watchdog,
            btop_stop_watchdog,
            list_commands,
            save_command,
            delete_command,
            analyze_command,
            run_command,
            list_command_jobs,
            cancel_command,
            probe_agent,
            agent_session_plan,
            start_agent_session,
            legacy_preview,
            legacy_apply,
            export_diagnostics,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build RemoteDeck");

    application.run(|app, event| {
        if matches!(event, RunEvent::Exit) {
            let state = app.state::<AppState>();
            let terminals = state.terminals.clone();
            let tunnels = state.tunnels.clone();
            let telemetry = state.telemetry.clone();
            let command_jobs = state.command_jobs.clone();
            let transfers = state.transfers.clone();
            let tunnel_app = app.clone();
            let telemetry_app = app.clone();
            let completed = tauri::async_runtime::block_on(tokio::time::timeout(
                APPLICATION_SHUTDOWN_TIMEOUT,
                async move {
                    let terminal_cleanup =
                        tauri::async_runtime::spawn_blocking(move || terminals.stop_all());
                    let tunnel_cleanup =
                        tauri::async_runtime::spawn_blocking(move || tunnels.stop_all(&tunnel_app));
                    let (_, tunnel_result, _, _, _) = tokio::join!(
                        terminal_cleanup,
                        tunnel_cleanup,
                        telemetry.shutdown(&telemetry_app),
                        command_jobs.shutdown(),
                        transfers.cancel_all(),
                    );
                    if let Ok(Err(error)) = tunnel_result {
                        eprintln!("RemoteDeck could not stop all tunnels cleanly: {error}");
                    }
                },
            ));
            if completed.is_err() {
                eprintln!("RemoteDeck cleanup reached the 12-second application shutdown limit");
            }
        }
    });
}

fn host_and_ssh(app: &AppHandle, host_id: &str) -> AppResult<(HostProfile, SshRuntime)> {
    let state = app.state::<AppState>();
    Ok((state.repository.host(host_id)?, state.ssh.clone()))
}

fn host_and_sftp(app: &AppHandle, host_id: &str) -> AppResult<(HostProfile, SftpService)> {
    let state = app.state::<AppState>();
    Ok((state.repository.host(host_id)?, state.sftp.clone()))
}

fn forward_transfer_events(app: &AppHandle, registry: &TransferRegistry) {
    let mut receiver = registry.subscribe();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let _ = app.emit("transfer-event", event);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn forward_command_events(app: &AppHandle, registry: &CommandJobRegistry) {
    let mut receiver = registry.subscribe();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let _ = app.emit("command-event", event);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

async fn trusted_key_records(
    repository: &AppRepository,
    ssh: &SshRuntime,
) -> AppResult<Vec<HostKeyRecord>> {
    let trusted = ssh.list_trusted_keys().await?;
    let hosts = repository.snapshot().hosts;
    let accepted_at = std::fs::metadata(repository.known_hosts_path())
        .and_then(|metadata| metadata.modified())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|_| Utc::now());
    Ok(trusted
        .into_iter()
        .map(|trusted| trusted_key_record(&hosts, trusted, accepted_at))
        .collect())
}

fn trusted_key_record(
    hosts: &[HostProfile],
    trusted: TrustedHostKey,
    accepted_at: DateTime<Utc>,
) -> HostKeyRecord {
    let (hostname, port) = parse_host_token(&trusted.host_token);
    let host_id = hosts
        .iter()
        .find(|host| host.port == port && host.hostname.eq_ignore_ascii_case(&hostname))
        .map(|host| host.id.clone());
    let mut digest = Sha256::new();
    digest.update(trusted.host_token.as_bytes());
    digest.update([0]);
    digest.update(trusted.algorithm.as_bytes());
    digest.update([0]);
    digest.update(trusted.public_key_base64.as_bytes());
    let id = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    HostKeyRecord {
        id,
        host_id,
        host_token: trusted.host_token,
        hostname,
        port,
        algorithm: trusted.algorithm,
        public_key_base64: trusted.public_key_base64,
        sha256_fingerprint: trusted.sha256_fingerprint,
        accepted_at,
    }
}

fn parse_host_token(token: &str) -> (String, u16) {
    if let Some(value) = token.strip_prefix('[')
        && let Some((host, port)) = value.rsplit_once("]:")
        && let Ok(port) = port.parse::<u16>()
    {
        return (host.to_owned(), port);
    }
    (token.to_owned(), 22)
}

fn remote_join(directory: &str, name: &str) -> String {
    let directory = directory.trim_end_matches('/');
    if directory.is_empty() {
        format!("/{name}")
    } else {
        format!("{directory}/{name}")
    }
}

fn remote_basename(path: &str) -> Option<&str> {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
}

fn safe_download_destination(directory: &str, remote_name: &str) -> AppResult<PathBuf> {
    if remote_name.is_empty()
        || remote_name == "."
        || remote_name == ".."
        || remote_name.contains(['/', '\\', ':', '\0', '\r', '\n'])
        || remote_name.ends_with([' ', '.'])
        || !matches!(
            Path::new(remote_name)
                .components()
                .collect::<Vec<_>>()
                .as_slice(),
            [std::path::Component::Normal(_)]
        )
    {
        return Err(AppError::Validation(
            "remote file name is unsafe on Windows".to_owned(),
        ));
    }
    let base = PathBuf::from(directory);
    if !base.is_absolute() {
        return Err(AppError::Validation(
            "download directory must be absolute".to_owned(),
        ));
    }
    let destination = base.join(remote_name);
    if !destination.starts_with(&base) {
        return Err(AppError::Validation(
            "download destination escapes its selected directory".to_owned(),
        ));
    }
    Ok(destination)
}

fn command_definition_from_preset(preset: CommandPreset) -> CommandDefinition {
    CommandDefinition {
        id: preset.id,
        host_id: preset.host_id,
        name: preset.name,
        description: preset.description,
        group: preset.group,
        command: preset.command,
        working_directory: preset.working_directory,
        risk: preset.risk,
        requires_pty: preset.requires_pty,
        requires_sudo: preset.requires_sudo,
        confirmation_text: preset.confirmation_text,
        sort_order: preset.sort_order,
        builtin: false,
    }
}

fn command_definition_from_builtin(
    index: usize,
    preset: &risk::BuiltinPreset,
) -> CommandDefinition {
    CommandDefinition {
        id: format!("builtin:{}", preset.id),
        host_id: None,
        name: preset.name.to_owned(),
        description: preset
            .dependency
            .map_or_else(String::new, |dependency| format!("Requires {dependency}.")),
        group: preset.group.to_owned(),
        command: preset.command.to_owned(),
        working_directory: None,
        risk: command_risk(preset.declared_risk),
        requires_pty: preset.requires_pty,
        requires_sudo: preset.requires_sudo,
        confirmation_text: None,
        sort_order: i32::try_from(index).unwrap_or(i32::MAX),
        builtin: true,
    }
}

fn resolve_command(
    repository: &AppRepository,
    request: &CommandRunRequest,
) -> AppResult<ResolvedCommand> {
    if let Some(id) = request
        .command_id
        .as_deref()
        .and_then(|id| id.strip_prefix("builtin:"))
    {
        let builtin = risk::builtin_preset(id)
            .ok_or_else(|| AppError::NotFound(format!("built-in command {id}")))?;
        let now = Utc::now();
        let preset = CommandPreset {
            schema_version: 2,
            id: format!("builtin:{id}"),
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
        let host = repository.host(&request.host_id)?;
        resolve_command_request(host, Some(preset), request).map_err(command_error)
    } else {
        resolve_repository_request(repository, request).map_err(command_error)
    }
}

fn command_risk(risk: risk::RiskLevel) -> CommandRisk {
    match risk {
        risk::RiskLevel::L0 => CommandRisk::L0,
        risk::RiskLevel::L1 => CommandRisk::L1,
        risk::RiskLevel::L2 => CommandRisk::L2,
    }
}

fn command_error(error: CommandJobError) -> AppError {
    match error {
        CommandJobError::InvalidRequest { .. }
        | CommandJobError::RiskAnalysis { .. }
        | CommandJobError::ConfirmationRequired { .. }
        | CommandJobError::ConfirmationMismatch { .. }
        | CommandJobError::RequiresPty { .. } => AppError::Validation(error.to_string()),
        CommandJobError::Repository { .. } | CommandJobError::UnknownJob { .. } => {
            AppError::NotFound(error.to_string())
        }
        CommandJobError::RuntimeUnavailable | CommandJobError::Process { .. } => {
            AppError::Process(error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_destination_joins_without_shell_interpolation() {
        assert_eq!(remote_join("/srv/data", "a b.txt"), "/srv/data/a b.txt");
        assert_eq!(remote_join("/", "file"), "/file");
        assert_eq!(remote_join("~", "file"), "~/file");
    }

    #[test]
    fn launch_at_login_registry_changes_only_when_the_setting_changes() {
        let current = AppSettings::default();
        assert_eq!(
            launch_at_login_change(
                &current,
                &SettingsPatch {
                    launch_at_login: Some(false),
                    ..SettingsPatch::default()
                }
            ),
            None
        );
        assert_eq!(
            launch_at_login_change(
                &current,
                &SettingsPatch {
                    launch_at_login: Some(true),
                    ..SettingsPatch::default()
                }
            ),
            Some(true)
        );
    }

    #[test]
    fn native_save_picker_rejects_path_injection_in_suggested_name() {
        assert!(validate_suggested_filename("id_ed25519").is_ok());
        assert!(validate_suggested_filename("../id_ed25519").is_err());
        assert!(validate_suggested_filename(r"C:\temp\key").is_err());
        assert!(validate_suggested_filename("..\nkey").is_err());
    }

    #[test]
    fn parses_standard_and_nonstandard_known_host_tokens() {
        assert_eq!(
            parse_host_token("example.test"),
            ("example.test".to_owned(), 22)
        );
        assert_eq!(
            parse_host_token("[example.test]:2222"),
            ("example.test".to_owned(), 2222)
        );
    }

    #[test]
    fn remote_basename_rejects_roots() {
        assert_eq!(remote_basename("/srv/a.txt"), Some("a.txt"));
        assert_eq!(remote_basename("/"), None);
        assert_eq!(remote_basename(".."), None);
    }

    #[test]
    fn download_destination_rejects_windows_escape_and_ads_names() {
        let base = if cfg!(windows) {
            r"C:\Downloads"
        } else {
            "/tmp/downloads"
        };
        assert!(safe_download_destination(base, "result.txt").is_ok());
        for name in [
            r"..\escape.txt",
            r"C:\escape.txt",
            "result.txt:secret",
            "trail. ",
        ] {
            assert!(safe_download_destination(base, name).is_err(), "{name}");
        }
    }
}
