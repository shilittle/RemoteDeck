use crate::{lifecycle, native, transport::EventHub};
use chrono::{DateTime, Utc};
use remotedeck_core::events::EventSink;
use remotedeck_core::{agent, agent_service, diagnostics, key_service, migration, risk};
use remotedeck_core::{
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
    session::{TerminalCommandOptions, TerminalRegistry},
    ssh::sftp::{
        ConflictPolicy, DownloadRequest, SftpDeleteRequest, SftpListRequest, SftpListResult,
        SftpMkdirRequest, SftpRenameRequest, SftpService, TransferDirection, TransferJob,
        TransferRegistry, UploadRequest,
    },
    ssh::{SshRuntime, TrustedHostKey, default_user_known_hosts_path},
    store::AppRepository,
    telemetry::{
        BtopStatus, ProcessSignalRequest, TelemetryOptions, TelemetryRegistry, TelemetrySnapshot,
        TelemetryStatus,
    },
    tunnel::TunnelRegistry,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const COMMAND_CONCURRENCY: usize = 8;

pub(crate) struct AppState {
    configuration: tokio::sync::Mutex<()>,
    config_revision: std::sync::atomic::AtomicU64,
    local_known_hosts_paths: Vec<PathBuf>,
    local_trust_warnings: std::sync::Mutex<Vec<String>>,
    pub(crate) repository: AppRepository,
    pub(crate) ssh: SshRuntime,
    pub(crate) terminals: TerminalRegistry,
    pub(crate) sftp: SftpService,
    pub(crate) transfers: TransferRegistry,
    pub(crate) tunnels: TunnelRegistry,
    pub(crate) telemetry: TelemetryRegistry,
    pub(crate) command_jobs: CommandJobRegistry,
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

fn bootstrap(state: &AppState) -> BootstrapPayload {
    let snapshot = state.repository.snapshot();
    BootstrapPayload {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        settings: snapshot.settings,
        hosts: snapshot.hosts,
        tunnels: snapshot.tunnels,
        capabilities: state.ssh.capabilities(),
    }
}

fn update_settings(state: &AppState, patch: SettingsPatch) -> AppResult<AppSettings> {
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

fn save_host(state: &AppState, draft: HostDraft) -> AppResult<HostProfile> {
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

async fn delete_host(app: AppContext, host_id: String) -> AppResult<()> {
    let (repository, terminals, tunnels, telemetry, transfers, command_jobs) = {
        let state = app.state.as_ref();
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
    if let Err(error) = tunnels.stop_for_host(app.events.clone(), &host_id) {
        cleanup_errors.push(format!("tunnels: {error}"));
    }
    if let Err(error) = telemetry.remove_host(app.events.clone(), &host_id).await {
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

async fn import_ssh_config(app: &AppContext, config_path: String) -> AppResult<SshImportResult> {
    let mut result = key_service::import_ssh_config(&app.state.repository, &config_path)?;
    // Reimport also repairs profiles imported by older versions, without requiring
    // the user to delete/recreate their already-saved hosts.
    let hosts = app.state.repository.snapshot().hosts;
    result.warnings.extend(app.seed_local_trust(&hosts).await);
    Ok(result)
}

async fn scan_host_keys(app: AppContext, host_id: String) -> AppResult<Vec<HostKeyCandidate>> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    ssh.scan_host_keys(&host).await
}

async fn accept_host_key(
    app: AppContext,
    host_id: String,
    candidate: HostKeyCandidate,
) -> AppResult<()> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    ssh.accept_host_key(&host, &candidate).await
}

async fn list_host_keys(app: AppContext) -> AppResult<Vec<HostKeyRecord>> {
    let (repository, ssh) = {
        let state = app.state.as_ref();
        (state.repository.clone(), state.ssh.clone())
    };
    trusted_key_records(&repository, &ssh).await
}

async fn remove_host_key(app: AppContext, record_id: String) -> AppResult<()> {
    let (repository, ssh) = {
        let state = app.state.as_ref();
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

async fn test_connection(app: AppContext, host_id: String) -> AppResult<ConnectionTestResult> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    ssh.test_connection(&host).await
}

async fn list_keys() -> AppResult<Vec<PrivateKeyRecord>> {
    key_service::list_private_keys().await
}

async fn generate_key(request: KeyGenerateRequest) -> AppResult<KeyOperationResult> {
    key_service::generate_key(request).await
}

async fn deploy_key(app: AppContext, request: KeyDeployRequest) -> AppResult<KeyOperationResult> {
    let (repository, ssh) = {
        let state = app.state.as_ref();
        (state.repository.clone(), state.ssh.clone())
    };
    key_service::deploy_key(&repository, &ssh, request).await
}

fn list_terminals(state: &AppState) -> Vec<TerminalSnapshot> {
    state.terminals.list()
}

fn start_terminal(
    app: AppContext,
    state: &AppState,
    host_id: String,
    rows: u16,
    cols: u16,
) -> AppResult<TerminalSnapshot> {
    let host = state.repository.host(&host_id)?;
    state
        .terminals
        .start(app.events.clone(), &host, &state.ssh, rows, cols)
}

fn reconnect_terminal(
    app: AppContext,
    state: &AppState,
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
    state.terminals.reconnect(
        app.events.clone(),
        &session_id,
        &host,
        &state.ssh,
        rows,
        cols,
    )
}

fn resize_terminal(
    state: &AppState,
    session_id: String,
    rows: u16,
    cols: u16,
    generation: u64,
    lease: u64,
) -> AppResult<()> {
    state
        .terminals
        .resize_input(&session_id, generation, lease, rows, cols)
}

fn close_terminal(state: &AppState, session_id: String) -> AppResult<()> {
    state.terminals.close(&session_id)
}

async fn sftp_list(app: AppContext, host_id: String, path: String) -> AppResult<SftpListResult> {
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

async fn sftp_create_directory(app: AppContext, host_id: String, path: String) -> AppResult<()> {
    let (host, service) = host_and_sftp(&app, &host_id)?;
    let _operation = service.begin_host_operation(&host.id)?;
    service.mkdir(&host, SftpMkdirRequest { path }).await?;
    Ok(())
}

async fn sftp_rename(
    app: AppContext,
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

async fn sftp_delete(
    app: AppContext,
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

fn transfer_list(state: &AppState, host_id: Option<String>) -> Vec<TransferJob> {
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

async fn transfer_upload(app: AppContext, request: TransferRequest) -> AppResult<TransferJob> {
    let (host, transfers) = {
        let state = app.state.as_ref();
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

async fn transfer_download(app: AppContext, request: TransferRequest) -> AppResult<TransferJob> {
    let (host, transfers) = {
        let state = app.state.as_ref();
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

async fn transfer_cancel(state: &AppState, job_id: String) -> AppResult<TransferJob> {
    state.transfers.cancel(&job_id).await
}

fn transfer_retry(state: &AppState, job_id: String) -> AppResult<TransferJob> {
    let job = state.transfers.get(&job_id)?;
    let host = state.repository.host(&job.host_id)?;
    state.transfers.retry(&job_id, host)
}

fn transfer_show_in_folder(state: &AppState, job_id: String) -> AppResult<()> {
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
        remotedeck_core::process::command("explorer.exe")
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

fn list_tunnels(state: &AppState, host_id: Option<String>) -> Vec<TunnelSnapshot> {
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

fn save_tunnel(state: &AppState, draft: TunnelDraft) -> AppResult<TunnelProfile> {
    state.repository.save_tunnel(draft)
}

fn delete_tunnel(app: AppContext, state: &AppState, tunnel_id: String) -> AppResult<()> {
    state.tunnels.remove(app.events.clone(), &tunnel_id)?;
    state.repository.delete_tunnel(&tunnel_id)
}

fn start_tunnel(app: AppContext, state: &AppState, tunnel_id: String) -> AppResult<TunnelSnapshot> {
    let tunnel = state.repository.tunnel(&tunnel_id)?;
    let host = state.repository.host(&tunnel.host_id)?;
    state
        .tunnels
        .start(app.events.clone(), &host, &tunnel, &state.ssh)
}

fn stop_tunnel(app: AppContext, state: &AppState, tunnel_id: String) -> AppResult<TunnelSnapshot> {
    state.tunnels.stop(app.events.clone(), &tunnel_id)?;
    let tunnel = state.repository.tunnel(&tunnel_id)?;
    Ok(state.tunnels.snapshot_for(&tunnel))
}

fn restart_tunnel(
    app: AppContext,
    state: &AppState,
    tunnel_id: String,
) -> AppResult<TunnelSnapshot> {
    let tunnel = state.repository.tunnel(&tunnel_id)?;
    let host = state.repository.host(&tunnel.host_id)?;
    state
        .tunnels
        .restart(app.events.clone(), &host, &tunnel, &state.ssh)
}

fn telemetry_list(state: &AppState) -> Vec<TelemetryStatus> {
    state.telemetry.list()
}

fn telemetry_history(state: &AppState, host_id: String) -> Vec<TelemetrySnapshot> {
    state.telemetry.history(&host_id)
}

fn telemetry_start(
    app: AppContext,
    state: &AppState,
    host_id: String,
) -> AppResult<TelemetryStatus> {
    let host = state.repository.host(&host_id)?;
    let settings = state.repository.snapshot().settings;
    state.telemetry.start(
        app.events.clone(),
        host,
        state.ssh.clone(),
        TelemetryOptions {
            interval_seconds: settings.telemetry_interval_seconds,
            retention_minutes: settings.telemetry_retention_minutes,
            auto_reconnect: settings.auto_reconnect,
        },
    )
}

async fn telemetry_stop(
    app: AppContext,
    state: &AppState,
    host_id: String,
) -> AppResult<TelemetryStatus> {
    state.repository.host(&host_id)?;
    state.telemetry.stop(app.events.clone(), &host_id).await
}

async fn signal_process(
    app: AppContext,
    host_id: String,
    pid: u32,
    expected_start_ticks: u64,
    expected_user: String,
    expected_command: String,
    signal: String,
) -> AppResult<()> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    let state = app.state.as_ref();
    if signal == "KILL"
        && !native::confirm_process_kill(&host.alias, pid, &expected_user, &expected_command).await
    {
        return Err(AppError::Cancelled);
    }
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

async fn btop_probe(app: AppContext, host_id: String) -> AppResult<BtopStatus> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    let state = app.state.as_ref();
    let rotation_minutes = state.repository.snapshot().settings.btop_rotation_minutes;
    state
        .telemetry
        .probe_btop(&ssh, &host, rotation_minutes)
        .await
}

async fn btop_start_watchdog(
    app: AppContext,
    host_id: String,
    rotation_minutes: u64,
) -> AppResult<BtopStatus> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    let state = app.state.as_ref();
    state
        .telemetry
        .start_btop_watchdog(&ssh, &host, rotation_minutes)
        .await
}

async fn btop_stop_watchdog(app: AppContext, host_id: String) -> AppResult<BtopStatus> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    let state = app.state.as_ref();
    let rotation_minutes = state.repository.snapshot().settings.btop_rotation_minutes;
    state
        .telemetry
        .stop_btop_watchdog(&ssh, &host, rotation_minutes)
        .await
}

fn list_commands(state: &AppState, host_id: Option<String>) -> Vec<CommandDefinition> {
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

fn save_command(state: &AppState, draft: CommandPresetDraft) -> AppResult<CommandDefinition> {
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

fn delete_command(state: &AppState, command_id: String, host_id: String) -> AppResult<()> {
    if command_id.starts_with("builtin:") {
        return Err(AppError::Validation(
            "built-in command presets are immutable".to_owned(),
        ));
    }
    state
        .repository
        .delete_command_for_host(&command_id, &host_id)
}

fn analyze_command(state: &AppState, request: CommandRunRequest) -> AppResult<CommandAnalysis> {
    let resolved = resolve_command(&state.repository, &request)?;
    analyze_resolved_command(&resolved).map_err(command_error)
}

async fn run_command(
    app: AppContext,
    state: &AppState,
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
                app.events.clone(),
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

fn list_command_jobs(state: &AppState, host_id: Option<String>) -> Vec<CommandJob> {
    state.command_jobs.list(host_id.as_deref())
}

fn cancel_command(state: &AppState, job_id: String) -> AppResult<CommandJob> {
    state.command_jobs.cancel(&job_id).map_err(command_error)
}

async fn probe_agent(
    app: AppContext,
    host_id: String,
    agent: agent::AgentProvider,
) -> AppResult<AgentStatus> {
    let (host, ssh) = host_and_ssh(&app, &host_id)?;
    agent_service::probe_agent(&ssh, &host, agent).await
}

fn agent_session_plan(
    state: &AppState,
    request: AgentSessionRequest,
) -> AppResult<AgentCommandPlan> {
    let host = state.repository.host(&request.host_id)?;
    agent_service::session_plan(&host, &request)
}

fn start_agent_session(
    app: AppContext,
    state: &AppState,
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
        app.events.clone(),
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

fn legacy_preview(state: &AppState, source_path: String) -> AppResult<migration::LegacyPreview> {
    migration::preview(&state.repository, &source_path)
}

fn legacy_apply(
    state: &AppState,
    request: migration::LegacyApplyRequest,
) -> AppResult<migration::LegacyApplyResult> {
    migration::apply(&state.repository, request)
}

async fn export_diagnostics(state: &AppState) -> AppResult<diagnostics::DiagnosticsExportResult> {
    let destination = native::pick_diagnostics_path(diagnostics::suggested_filename()).await;
    let Some(destination) = destination else {
        return Ok(diagnostics::DiagnosticsExportResult::cancelled());
    };
    diagnostics::export(&state.repository, &state.ssh, Some(&destination)).await
}

fn host_and_ssh(app: &AppContext, host_id: &str) -> AppResult<(HostProfile, SshRuntime)> {
    let state = app.state.as_ref();
    Ok((state.repository.host(host_id)?, state.ssh.clone()))
}

fn host_and_sftp(app: &AppContext, host_id: &str) -> AppResult<(HostProfile, SftpService)> {
    let state = app.state.as_ref();
    Ok((state.repository.host(host_id)?, state.sftp.clone()))
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
        assert!(native::validate_suggested_filename("id_ed25519").is_ok());
        assert!(native::validate_suggested_filename("../id_ed25519").is_err());
        assert!(native::validate_suggested_filename(r"C:\temp\key").is_err());
        assert!(native::validate_suggested_filename("..\nkey").is_err());
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

#[derive(Clone)]
pub(crate) struct AppContext {
    pub(crate) state: Arc<AppState>,
    pub(crate) events: EventSink,
}

impl AppContext {
    pub(crate) fn open(directory: PathBuf, hub: EventHub) -> AppResult<Self> {
        Self::open_with_local_trust_paths(
            directory,
            hub,
            default_user_known_hosts_path().into_iter().collect(),
        )
    }

    pub(crate) fn open_with_local_trust_paths(
        directory: PathBuf,
        hub: EventHub,
        local_known_hosts_paths: Vec<PathBuf>,
    ) -> AppResult<Self> {
        let repository = AppRepository::open(directory)?;
        let ssh = SshRuntime::discover_with_repository(repository.clone());
        let sftp = SftpService::new(ssh.clone());
        let transfers = TransferRegistry::new(sftp.clone());
        let command_jobs = CommandJobRegistry::new(COMMAND_CONCURRENCY).map_err(command_error)?;
        let telemetry = TelemetryRegistry::with_owner_nonce(repository.btop_owner_nonce());
        let events = EventSink::new(move |name, payload| {
            hub.publish(name, payload);
        });
        let context = Self {
            state: Arc::new(AppState {
                configuration: tokio::sync::Mutex::new(()),
                config_revision: std::sync::atomic::AtomicU64::new(0),
                local_known_hosts_paths,
                local_trust_warnings: std::sync::Mutex::new(Vec::new()),
                repository,
                ssh,
                sftp,
                transfers,
                command_jobs,
                telemetry,
                terminals: TerminalRegistry::default(),
                tunnels: TunnelRegistry::default(),
            }),
            events,
        };
        let mut transfers = context.state.transfers.subscribe();
        let transfer_sink = context.events.clone();
        tokio::spawn(async move {
            loop {
                match transfers.recv().await {
                    Ok(event) => {
                        let _ = transfer_sink.emit("transfer-event", event);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let _ = transfer_sink.emit("resync", ());
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        let mut commands = context.state.command_jobs.subscribe();
        let command_sink = context.events.clone();
        tokio::spawn(async move {
            loop {
                match commands.recv().await {
                    Ok(event) => {
                        let _ = command_sink.emit("command-event", event);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let _ = command_sink.emit("resync", ());
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(context)
    }

    async fn seed_local_trust(&self, hosts: &[HostProfile]) -> Vec<String> {
        if hosts.is_empty() {
            return Vec::new();
        }
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            self.state.ssh.seed_trusted_keys_from_local_known_hosts(
                hosts,
                &self.state.local_known_hosts_paths,
            ),
        )
        .await;
        let warnings = match result {
            Ok(Ok(result)) => {
                let mut warnings = result.source_warnings;
                for host in result.hosts {
                    warnings.extend(host.warnings);
                }
                warnings
            }
            Ok(Err(error)) => vec![format!(
                "本机 SSH 信任记录未能复用：{error}。仍执行严格主机密钥检查。"
            )],
            Err(_) => vec![
                "读取本机 SSH 信任记录超过 15 秒；未完成的主机可重新保存后重试，或手动核验指纹。"
                    .to_owned(),
            ],
        };
        let mut retained = self
            .state
            .local_trust_warnings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for warning in &warnings {
            if retained.len() < 128 && !retained.contains(warning) {
                retained.push(warning.clone());
            }
        }
        warnings
    }

    pub(crate) async fn initialize_local_trust(&self) {
        self.seed_local_trust(&self.state.repository.snapshot().hosts)
            .await;
    }
    pub(crate) fn autostart(&self) {
        let initial = self.state.repository.snapshot();
        for profile in initial.tunnels.iter().filter(|t| t.auto_start) {
            if let Ok(host) = self.state.repository.host(&profile.host_id) {
                let _ =
                    self.state
                        .tunnels
                        .start(self.events.clone(), &host, profile, &self.state.ssh);
            }
        }
        for host in initial.hosts.iter().filter(|h| h.monitor_enabled) {
            let options = TelemetryOptions {
                interval_seconds: initial.settings.telemetry_interval_seconds,
                retention_minutes: initial.settings.telemetry_retention_minutes,
                auto_reconnect: initial.settings.auto_reconnect,
            };
            if self
                .state
                .telemetry
                .start(
                    self.events.clone(),
                    host.clone(),
                    self.state.ssh.clone(),
                    options,
                )
                .is_ok()
                && initial.settings.btop_watchdog_enabled
            {
                let context = self.clone();
                let host = host.clone();
                let rotation = initial.settings.btop_rotation_minutes;
                tokio::spawn(async move {
                    let _ = context
                        .state
                        .telemetry
                        .start_btop_watchdog(&context.state.ssh, &host, rotation)
                        .await;
                });
            }
        }
    }
    pub(crate) async fn shutdown(&self) -> Vec<String> {
        let terminals = self.state.terminals.clone();
        let tunnels = self.state.tunnels.clone();
        let events = self.events.clone();
        let work = async {
            let (terminal, tunnel, _, _, _) = tokio::join!(
                tokio::task::spawn_blocking(move || terminals.stop_all()),
                tokio::task::spawn_blocking(move || tunnels.stop_all(events)),
                self.state.telemetry.shutdown(self.events.clone()),
                self.state.command_jobs.shutdown(),
                self.state.transfers.cancel_all()
            );
            let mut errors = Vec::new();
            if let Err(error) = terminal {
                errors.push(format!("terminal cleanup: {error}"));
            }
            match tunnel {
                Ok(Ok(())) => (),
                Ok(Err(e)) => errors.push(format!("tunnel cleanup: {e}")),
                Err(e) => errors.push(format!("tunnel cleanup: {e}")),
            }
            errors
        };
        match tokio::time::timeout(std::time::Duration::from_secs(12), work).await {
            Ok(errors) => errors,
            Err(_) => vec!["Resource cleanup reached the 12-second limit; remote watchdog cleanup may remain incomplete.".to_owned()]
        }
    }
}

pub(crate) const COMMANDS: &[&str] = &[
    "bootstrap",
    "update_settings",
    "pick_local_path",
    "pick_save_path",
    "save_host",
    "delete_host",
    "import_ssh_config",
    "scan_host_keys",
    "accept_host_key",
    "list_host_keys",
    "remove_host_key",
    "test_connection",
    "list_keys",
    "generate_key",
    "deploy_key",
    "list_terminals",
    "start_terminal",
    "reconnect_terminal",
    "resize_terminal",
    "close_terminal",
    "sftp_list",
    "sftp_create_directory",
    "sftp_rename",
    "sftp_delete",
    "transfer_list",
    "transfer_upload",
    "transfer_download",
    "transfer_cancel",
    "transfer_retry",
    "transfer_show_in_folder",
    "list_tunnels",
    "save_tunnel",
    "delete_tunnel",
    "start_tunnel",
    "stop_tunnel",
    "restart_tunnel",
    "telemetry_list",
    "telemetry_history",
    "telemetry_start",
    "telemetry_stop",
    "signal_process",
    "btop_probe",
    "btop_start_watchdog",
    "btop_stop_watchdog",
    "list_commands",
    "save_command",
    "delete_command",
    "analyze_command",
    "run_command",
    "list_command_jobs",
    "cancel_command",
    "probe_agent",
    "agent_session_plan",
    "start_agent_session",
    "legacy_preview",
    "legacy_apply",
    "export_diagnostics",
];

fn argument<T: serde::de::DeserializeOwned>(args: &serde_json::Value, name: &str) -> AppResult<T> {
    serde_json::from_value(args.get(name).cloned().unwrap_or(serde_json::Value::Null))
        .map_err(|error| AppError::Validation(format!("invalid {name}: {error}")))
}
pub(crate) async fn dispatch(
    app: AppContext,
    command: &str,
    args: serde_json::Value,
) -> AppResult<serde_json::Value> {
    let state = app.state.as_ref();
    let changes_configuration = matches!(
        command,
        "save_host"
            | "delete_host"
            | "save_tunnel"
            | "delete_tunnel"
            | "save_command"
            | "delete_command"
            | "update_settings"
            | "import_ssh_config"
            | "legacy_apply"
            | "deploy_key"
            | "accept_host_key"
            | "remove_host_key"
    );
    let _configuration = if changes_configuration || command == "bootstrap" {
        Some(state.configuration.lock().await)
    } else {
        None
    };
    let object = args
        .as_object()
        .ok_or_else(|| AppError::Validation("request must be a JSON object".to_owned()))?;
    let allowed: &[&str] = match command {
        "bootstrap" => &[],
        "update_settings" => &["patch"],
        "pick_local_path" => &["directory"],
        "pick_save_path" => &["suggestedName"],
        "save_host" => &["draft"],
        "delete_host" => &["hostId"],
        "import_ssh_config" => &["configPath"],
        "scan_host_keys" => &["hostId"],
        "accept_host_key" => &["hostId", "candidate"],
        "list_host_keys" => &[],
        "remove_host_key" => &["recordId"],
        "test_connection" => &["hostId"],
        "list_keys" => &[],
        "generate_key" => &["request"],
        "deploy_key" => &["request"],
        "list_terminals" => &[],
        "start_terminal" => &["hostId", "rows", "cols"],
        "reconnect_terminal" => &["sessionId", "rows", "cols"],
        "resize_terminal" => &["sessionId", "rows", "cols", "generation", "lease"],
        "close_terminal" => &["sessionId"],
        "sftp_list" => &["hostId", "path"],
        "sftp_create_directory" => &["hostId", "path"],
        "sftp_rename" => &["hostId", "sourcePath", "destinationPath"],
        "sftp_delete" => &["hostId", "path", "recursive"],
        "transfer_list" => &["hostId"],
        "transfer_upload" => &["request"],
        "transfer_download" => &["request"],
        "transfer_cancel" => &["jobId"],
        "transfer_retry" => &["jobId"],
        "transfer_show_in_folder" => &["jobId"],
        "list_tunnels" => &["hostId"],
        "save_tunnel" => &["draft"],
        "delete_tunnel" => &["tunnelId"],
        "start_tunnel" => &["tunnelId"],
        "stop_tunnel" => &["tunnelId"],
        "restart_tunnel" => &["tunnelId"],
        "telemetry_list" => &[],
        "telemetry_history" => &["hostId"],
        "telemetry_start" => &["hostId"],
        "telemetry_stop" => &["hostId"],
        "signal_process" => &[
            "hostId",
            "pid",
            "expectedStartTicks",
            "expectedUser",
            "expectedCommand",
            "signal",
        ],
        "btop_probe" => &["hostId"],
        "btop_start_watchdog" => &["hostId", "rotationMinutes"],
        "btop_stop_watchdog" => &["hostId"],
        "list_commands" => &["hostId"],
        "save_command" => &["draft"],
        "delete_command" => &["commandId", "hostId"],
        "analyze_command" => &["request"],
        "run_command" => &["request"],
        "list_command_jobs" => &["hostId"],
        "cancel_command" => &["jobId"],
        "probe_agent" => &["hostId", "agent"],
        "agent_session_plan" => &["request"],
        "start_agent_session" => &["request"],
        "legacy_preview" => &["sourcePath"],
        "legacy_apply" => &["request"],
        "export_diagnostics" => &[],
        _ => return Err(AppError::NotFound("unknown operation".to_owned())),
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(AppError::Validation("unexpected request field".to_owned()));
    }
    let mut result = match command {
        "bootstrap" => serde_json::to_value(bootstrap(state))?,
        "update_settings" => {
            serde_json::to_value(update_settings(state, argument(&args, "patch")?)?)?
        }
        "pick_local_path" => {
            serde_json::to_value(native::pick_local_path(argument(&args, "directory")?).await)?
        }
        "pick_save_path" => {
            serde_json::to_value(native::pick_save_path(argument(&args, "suggestedName")?).await?)?
        }
        "save_host" => {
            let host = save_host(state, argument(&args, "draft")?)?;
            app.seed_local_trust(std::slice::from_ref(&host)).await;
            serde_json::to_value(host)?
        }
        "delete_host" => {
            serde_json::to_value(delete_host(app.clone(), argument(&args, "hostId")?).await?)?
        }
        "import_ssh_config" => {
            serde_json::to_value(import_ssh_config(&app, argument(&args, "configPath")?).await?)?
        }
        "scan_host_keys" => {
            serde_json::to_value(scan_host_keys(app.clone(), argument(&args, "hostId")?).await?)?
        }
        "accept_host_key" => serde_json::to_value(
            accept_host_key(
                app.clone(),
                argument(&args, "hostId")?,
                argument(&args, "candidate")?,
            )
            .await?,
        )?,
        "list_host_keys" => serde_json::to_value(list_host_keys(app.clone()).await?)?,
        "remove_host_key" => {
            serde_json::to_value(remove_host_key(app.clone(), argument(&args, "recordId")?).await?)?
        }
        "test_connection" => {
            serde_json::to_value(test_connection(app.clone(), argument(&args, "hostId")?).await?)?
        }
        "list_keys" => serde_json::to_value(list_keys().await?)?,
        "generate_key" => serde_json::to_value(generate_key(argument(&args, "request")?).await?)?,
        "deploy_key" => {
            serde_json::to_value(deploy_key(app.clone(), argument(&args, "request")?).await?)?
        }
        "list_terminals" => serde_json::to_value(list_terminals(state))?,
        "start_terminal" => serde_json::to_value(start_terminal(
            app.clone(),
            state,
            argument(&args, "hostId")?,
            argument(&args, "rows")?,
            argument(&args, "cols")?,
        )?)?,
        "reconnect_terminal" => serde_json::to_value(reconnect_terminal(
            app.clone(),
            state,
            argument(&args, "sessionId")?,
            argument(&args, "rows")?,
            argument(&args, "cols")?,
        )?)?,
        "resize_terminal" => serde_json::to_value(resize_terminal(
            state,
            argument(&args, "sessionId")?,
            argument(&args, "rows")?,
            argument(&args, "cols")?,
            argument(&args, "generation")?,
            argument(&args, "lease")?,
        )?)?,
        "close_terminal" => {
            serde_json::to_value(close_terminal(state, argument(&args, "sessionId")?)?)?
        }
        "sftp_list" => serde_json::to_value(
            sftp_list(
                app.clone(),
                argument(&args, "hostId")?,
                argument(&args, "path")?,
            )
            .await?,
        )?,
        "sftp_create_directory" => serde_json::to_value(
            sftp_create_directory(
                app.clone(),
                argument(&args, "hostId")?,
                argument(&args, "path")?,
            )
            .await?,
        )?,
        "sftp_rename" => serde_json::to_value(
            sftp_rename(
                app.clone(),
                argument(&args, "hostId")?,
                argument(&args, "sourcePath")?,
                argument(&args, "destinationPath")?,
            )
            .await?,
        )?,
        "sftp_delete" => serde_json::to_value(
            sftp_delete(
                app.clone(),
                argument(&args, "hostId")?,
                argument(&args, "path")?,
                argument(&args, "recursive")?,
            )
            .await?,
        )?,
        "transfer_list" => serde_json::to_value(transfer_list(state, argument(&args, "hostId")?))?,
        "transfer_upload" => {
            serde_json::to_value(transfer_upload(app.clone(), argument(&args, "request")?).await?)?
        }
        "transfer_download" => serde_json::to_value(
            transfer_download(app.clone(), argument(&args, "request")?).await?,
        )?,
        "transfer_cancel" => {
            serde_json::to_value(transfer_cancel(state, argument(&args, "jobId")?).await?)?
        }
        "transfer_retry" => {
            serde_json::to_value(transfer_retry(state, argument(&args, "jobId")?)?)?
        }
        "transfer_show_in_folder" => {
            serde_json::to_value(transfer_show_in_folder(state, argument(&args, "jobId")?)?)?
        }
        "list_tunnels" => serde_json::to_value(list_tunnels(state, argument(&args, "hostId")?))?,
        "save_tunnel" => serde_json::to_value(save_tunnel(state, argument(&args, "draft")?)?)?,
        "delete_tunnel" => serde_json::to_value(delete_tunnel(
            app.clone(),
            state,
            argument(&args, "tunnelId")?,
        )?)?,
        "start_tunnel" => serde_json::to_value(start_tunnel(
            app.clone(),
            state,
            argument(&args, "tunnelId")?,
        )?)?,
        "stop_tunnel" => serde_json::to_value(stop_tunnel(
            app.clone(),
            state,
            argument(&args, "tunnelId")?,
        )?)?,
        "restart_tunnel" => serde_json::to_value(restart_tunnel(
            app.clone(),
            state,
            argument(&args, "tunnelId")?,
        )?)?,
        "telemetry_list" => serde_json::to_value(telemetry_list(state))?,
        "telemetry_history" => {
            serde_json::to_value(telemetry_history(state, argument(&args, "hostId")?))?
        }
        "telemetry_start" => serde_json::to_value(telemetry_start(
            app.clone(),
            state,
            argument(&args, "hostId")?,
        )?)?,
        "telemetry_stop" => serde_json::to_value(
            telemetry_stop(app.clone(), state, argument(&args, "hostId")?).await?,
        )?,
        "signal_process" => serde_json::to_value(
            signal_process(
                app.clone(),
                argument(&args, "hostId")?,
                argument(&args, "pid")?,
                argument(&args, "expectedStartTicks")?,
                argument(&args, "expectedUser")?,
                argument(&args, "expectedCommand")?,
                argument(&args, "signal")?,
            )
            .await?,
        )?,
        "btop_probe" => {
            serde_json::to_value(btop_probe(app.clone(), argument(&args, "hostId")?).await?)?
        }
        "btop_start_watchdog" => serde_json::to_value(
            btop_start_watchdog(
                app.clone(),
                argument(&args, "hostId")?,
                argument(&args, "rotationMinutes")?,
            )
            .await?,
        )?,
        "btop_stop_watchdog" => serde_json::to_value(
            btop_stop_watchdog(app.clone(), argument(&args, "hostId")?).await?,
        )?,
        "list_commands" => serde_json::to_value(list_commands(state, argument(&args, "hostId")?))?,
        "save_command" => serde_json::to_value(save_command(state, argument(&args, "draft")?)?)?,
        "delete_command" => serde_json::to_value(delete_command(
            state,
            argument(&args, "commandId")?,
            argument(&args, "hostId")?,
        )?)?,
        "analyze_command" => {
            serde_json::to_value(analyze_command(state, argument(&args, "request")?)?)?
        }
        "run_command" => serde_json::to_value(
            run_command(app.clone(), state, argument(&args, "request")?).await?,
        )?,
        "list_command_jobs" => {
            serde_json::to_value(list_command_jobs(state, argument(&args, "hostId")?))?
        }
        "cancel_command" => {
            serde_json::to_value(cancel_command(state, argument(&args, "jobId")?)?)?
        }
        "probe_agent" => serde_json::to_value(
            probe_agent(
                app.clone(),
                argument(&args, "hostId")?,
                argument(&args, "agent")?,
            )
            .await?,
        )?,
        "agent_session_plan" => {
            serde_json::to_value(agent_session_plan(state, argument(&args, "request")?)?)?
        }
        "start_agent_session" => serde_json::to_value(start_agent_session(
            app.clone(),
            state,
            argument(&args, "request")?,
        )?)?,
        "legacy_preview" => {
            serde_json::to_value(legacy_preview(state, argument(&args, "sourcePath")?)?)?
        }
        "legacy_apply" => serde_json::to_value(legacy_apply(state, argument(&args, "request")?)?)?,
        "export_diagnostics" => serde_json::to_value(export_diagnostics(state).await?)?,
        _ => return Err(AppError::NotFound("unknown operation".to_owned())),
    };
    if changes_configuration {
        let revision = state
            .config_revision
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let _ = app
            .events
            .emit("resync", serde_json::json!({"configRevision":revision}));
    }
    if command == "bootstrap" {
        result["sshTrustWarnings"] = serde_json::to_value(
            &*state
                .local_trust_warnings
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        )?;
        result["configRevision"] = state
            .config_revision
            .load(std::sync::atomic::Ordering::SeqCst)
            .into();
    }
    if command == "close_terminal" {
        let _ = app.events.emit("resync", serde_json::Value::Null);
    }
    Ok(result)
}
