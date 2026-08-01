//! Read-only preview and explicit import of legacy LabPulse and RemoteDeck v1
//! data.  Source material is reparsed and rehashed immediately before apply.

use crate::{
    error::{AppError, AppResult},
    keys::parse_open_ssh_config,
    model::{
        AuthMethod, CommandPresetDraft, CommandRisk, HostDraft, PersistedState, SettingsPatch,
        SshAdvancedPatch, TunnelDirection, TunnelDraft, TunnelHealthCheck, TunnelHealthCheckKind,
    },
    store::{
        AppRepository, ImportBatch, ImportBatchCommand, ImportBatchHost, ImportBatchTunnel,
        ImportHostReference,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
};

const MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_HOSTS: usize = 512;
const MAX_TUNNELS: usize = 1024;
const MAX_COMMANDS: usize = 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LegacyHostPreview {
    pub alias: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LegacyPreview {
    pub source_path: String,
    pub source_hash: String,
    pub app_name: String,
    pub duplicate: bool,
    pub hosts: Vec<LegacyHostPreview>,
    pub tunnel_count: usize,
    pub command_count: usize,
    pub settings_included: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyApplyRequest {
    pub source_path: String,
    pub source_hash: String,
    pub include_hosts: bool,
    pub include_tunnels: bool,
    pub include_commands: bool,
    pub include_settings: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LegacyApplyResult {
    pub imported_hosts: usize,
    pub imported_tunnels: usize,
    pub imported_commands: usize,
    pub settings_imported: bool,
    pub message: String,
}

#[derive(Debug)]
struct LoadedMigration {
    source_path: String,
    source_hash: String,
    plan: MigrationPlan,
}

#[derive(Debug, Default)]
struct MigrationPlan {
    app_name: String,
    hosts: Vec<PlannedHost>,
    tunnels: Vec<PlannedTunnel>,
    commands: Vec<PlannedCommand>,
    settings: Option<SettingsPatch>,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct PlannedHost {
    source_id: String,
    draft: HostDraft,
}

#[derive(Debug)]
struct PlannedTunnel {
    source_host_id: String,
    draft: TunnelDraft,
}

#[derive(Debug)]
struct PlannedCommand {
    source_host_id: Option<String>,
    draft: CommandPresetDraft,
}

#[derive(Debug)]
struct SourcePart {
    role: &'static str,
    bytes: Vec<u8>,
}

pub fn preview(repository: &AppRepository, source_path: &str) -> AppResult<LegacyPreview> {
    let loaded = load_migration(source_path)?;
    Ok(preview_loaded(repository, loaded))
}

fn preview_loaded(repository: &AppRepository, loaded: LoadedMigration) -> LegacyPreview {
    LegacyPreview {
        source_path: loaded.source_path,
        source_hash: loaded.source_hash.clone(),
        app_name: loaded.plan.app_name,
        duplicate: repository.has_import(&loaded.source_hash),
        hosts: loaded
            .plan
            .hosts
            .iter()
            .map(|host| LegacyHostPreview {
                alias: host.draft.alias.clone(),
                hostname: host.draft.hostname.clone(),
                port: host.draft.port,
                username: host.draft.username.clone(),
            })
            .collect(),
        tunnel_count: loaded.plan.tunnels.len(),
        command_count: loaded.plan.commands.len(),
        settings_included: loaded.plan.settings.is_some(),
        warnings: loaded.plan.warnings,
    }
}

pub fn apply(
    repository: &AppRepository,
    request: LegacyApplyRequest,
) -> AppResult<LegacyApplyResult> {
    validate_hash(&request.source_hash)?;
    let loaded = load_migration(&request.source_path)?;
    if loaded.source_hash != request.source_hash.to_ascii_lowercase() {
        return Err(AppError::State(
            "legacy source changed after preview; preview it again before importing".to_owned(),
        ));
    }
    if repository.has_import(&loaded.source_hash) {
        return Ok(LegacyApplyResult {
            imported_hosts: 0,
            imported_tunnels: 0,
            imported_commands: 0,
            settings_imported: false,
            message: "This source bundle was already imported; no duplicate items were created."
                .to_owned(),
        });
    }

    let batch = build_import_batch(
        &loaded.plan,
        &repository.snapshot(),
        &request,
        loaded.source_hash,
    )?;
    let imported = repository.import_batch(batch)?;
    if imported.duplicate {
        return Ok(LegacyApplyResult {
            imported_hosts: 0,
            imported_tunnels: 0,
            imported_commands: 0,
            settings_imported: false,
            message: "This source bundle was already imported; no duplicate items were created."
                .to_owned(),
        });
    }

    Ok(LegacyApplyResult {
        imported_hosts: imported.imported_hosts,
        imported_tunnels: imported.imported_tunnels,
        imported_commands: imported.imported_commands,
        settings_imported: imported.settings_imported,
        message: format!(
            "Imported {} host(s), {} tunnel(s), and {} command preset(s).",
            imported.imported_hosts, imported.imported_tunnels, imported.imported_commands
        ),
    })
}

fn load_migration(source_path: &str) -> AppResult<LoadedMigration> {
    validate_source_path(source_path)?;
    let input = fs::canonicalize(source_path)?;
    let metadata = fs::metadata(&input)?;
    let (kind, primary, settings) = if metadata.is_dir() {
        let profiles = input.join("profiles.json");
        let config = input.join("config.json");
        if profiles.is_file() {
            (
                SourceKind::RemoteDeckV1,
                profiles,
                optional_file(input.join("settings.json")),
            )
        } else if config.is_file() {
            (SourceKind::LabPulse, config, None)
        } else {
            return Err(AppError::Validation(
                "migration directory must contain profiles.json or config.json".to_owned(),
            ));
        }
    } else if metadata.is_file() {
        let name = input
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match name.as_str() {
            "config.json" => (SourceKind::LabPulse, input.clone(), None),
            "profiles.json" => (
                SourceKind::RemoteDeckV1,
                input.clone(),
                input
                    .parent()
                    .and_then(|parent| optional_file(parent.join("settings.json"))),
            ),
            "settings.json" => {
                let profiles = input
                    .parent()
                    .map(|parent| parent.join("profiles.json"))
                    .filter(|path| path.is_file())
                    .ok_or_else(|| {
                        AppError::Validation(
                            "settings.json must have a sibling profiles.json".to_owned(),
                        )
                    })?;
                (SourceKind::RemoteDeckV1, profiles, Some(input.clone()))
            }
            _ => {
                return Err(AppError::Validation(
                    "migration source must be config.json, profiles.json, settings.json, or their directory"
                        .to_owned(),
                ));
            }
        }
    } else {
        return Err(AppError::Validation(
            "migration source must be a file or directory".to_owned(),
        ));
    };

    match kind {
        SourceKind::LabPulse => load_labpulse(input, primary),
        SourceKind::RemoteDeckV1 => load_v1(input, primary, settings),
    }
}

#[derive(Debug, Clone, Copy)]
enum SourceKind {
    LabPulse,
    RemoteDeckV1,
}

fn load_labpulse(input: PathBuf, config_path: PathBuf) -> AppResult<LoadedMigration> {
    let mut total = 0_u64;
    let config_bytes = read_required_part(&config_path, &mut total)?;
    let config: LabPulseConfig = serde_json::from_slice(&config_bytes)?;
    let mut parts = vec![SourcePart {
        role: "labpulse-config",
        bytes: config_bytes,
    }];
    let mut warnings = Vec::new();
    let ssh_config = discover_user_ssh_config(&mut total, &mut warnings).inspect(|bytes| {
        parts.push(SourcePart {
            role: "user-openssh-config",
            bytes: bytes.clone(),
        });
    });
    let mut plan = map_labpulse(&config, ssh_config.as_deref(), warnings)?;
    validate_plan(&mut plan)?;
    Ok(LoadedMigration {
        source_path: display_path(&input),
        source_hash: hash_parts(&mut parts),
        plan,
    })
}

fn load_v1(
    input: PathBuf,
    profiles_path: PathBuf,
    settings_path: Option<PathBuf>,
) -> AppResult<LoadedMigration> {
    let mut total = 0_u64;
    let profiles_bytes = read_required_part(&profiles_path, &mut total)?;
    let database: V1Database = serde_json::from_slice(&profiles_bytes)?;
    let mut parts = vec![SourcePart {
        role: "remotedeck-v1-profiles",
        bytes: profiles_bytes,
    }];
    let settings = settings_path
        .map(|path| {
            let bytes = read_required_part(&path, &mut total)?;
            let parsed = serde_json::from_slice::<V1Settings>(&bytes)?;
            parts.push(SourcePart {
                role: "remotedeck-v1-settings",
                bytes,
            });
            Ok::<_, AppError>(parsed)
        })
        .transpose()?;
    let mut plan = map_v1(database, settings)?;
    validate_plan(&mut plan)?;
    Ok(LoadedMigration {
        source_path: display_path(&input),
        source_hash: hash_parts(&mut parts),
        plan,
    })
}

fn build_import_batch(
    plan: &MigrationPlan,
    snapshot: &PersistedState,
    request: &LegacyApplyRequest,
    source_hash: String,
) -> AppResult<ImportBatch> {
    let actionable = request.include_hosts && !plan.hosts.is_empty()
        || request.include_tunnels && !plan.tunnels.is_empty()
        || request.include_commands && !plan.commands.is_empty()
        || request.include_settings && plan.settings.is_some();
    if !actionable {
        return Err(AppError::Validation(
            "select at least one available migration category".to_owned(),
        ));
    }

    let hosts = if request.include_hosts {
        plan.hosts
            .iter()
            .map(|planned| ImportBatchHost {
                source_id: planned.source_id.clone(),
                draft: planned.draft.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let tunnels = if request.include_tunnels {
        plan.tunnels
            .iter()
            .map(|planned| {
                Ok(ImportBatchTunnel {
                    host: destination_host_reference(
                        plan,
                        snapshot,
                        &planned.source_host_id,
                        request.include_hosts,
                    )?,
                    draft: planned.draft.clone(),
                })
            })
            .collect::<AppResult<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let commands = if request.include_commands {
        plan.commands
            .iter()
            .map(|planned| {
                Ok(ImportBatchCommand {
                    host: planned
                        .source_host_id
                        .as_deref()
                        .map(|source_id| {
                            destination_host_reference(
                                plan,
                                snapshot,
                                source_id,
                                request.include_hosts,
                            )
                        })
                        .transpose()?,
                    draft: planned.draft.clone(),
                })
            })
            .collect::<AppResult<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let settings = if request.include_settings {
        plan.settings.clone()
    } else {
        None
    };
    Ok(ImportBatch {
        source_hash,
        hosts,
        tunnels,
        commands,
        settings,
    })
}

fn destination_host_reference(
    plan: &MigrationPlan,
    snapshot: &PersistedState,
    source_id: &str,
    importing_hosts: bool,
) -> AppResult<ImportHostReference> {
    if importing_hosts {
        return Ok(ImportHostReference::Imported(source_id.to_owned()));
    }
    let source_host = plan
        .hosts
        .iter()
        .find(|host| host.source_id == source_id)
        .ok_or_else(|| {
            AppError::Validation(format!(
                "source host {source_id} is missing from import plan"
            ))
        })?;
    snapshot
        .hosts
        .iter()
        .find(|host| host.alias.eq_ignore_ascii_case(&source_host.draft.alias))
        .map(|host| ImportHostReference::Existing(host.id.clone()))
        .ok_or_else(|| {
            AppError::Validation(format!(
                "source host {source_id} is not being imported and has no matching destination alias"
            ))
        })
}

fn validate_plan(plan: &mut MigrationPlan) -> AppResult<()> {
    if plan.app_name.trim().is_empty()
        || plan.app_name.len() > 512
        || plan.app_name.chars().any(char::is_control)
    {
        return Err(AppError::Validation(
            "legacy application name is invalid".to_owned(),
        ));
    }
    if plan.hosts.len() > MAX_HOSTS
        || plan.tunnels.len() > MAX_TUNNELS
        || plan.commands.len() > MAX_COMMANDS
        || plan.warnings.len() > 128
    {
        return Err(AppError::Validation(
            "legacy source contains too many records".to_owned(),
        ));
    }
    let mut source_ids = HashSet::new();
    let mut aliases = HashSet::new();
    for host in &plan.hosts {
        if host.source_id.is_empty() || !source_ids.insert(host.source_id.clone()) {
            return Err(AppError::Validation(
                "legacy hosts contain a missing or duplicate id".to_owned(),
            ));
        }
        validate_host(&host.draft)?;
        if !aliases.insert(host.draft.alias.to_ascii_lowercase()) {
            return Err(AppError::Validation(format!(
                "legacy host alias '{}' is duplicated",
                host.draft.alias
            )));
        }
    }
    let mut tunnels = HashSet::new();
    for tunnel in &plan.tunnels {
        if !source_ids.contains(&tunnel.source_host_id) {
            return Err(AppError::Validation(format!(
                "legacy tunnel '{}' references a missing host",
                tunnel.draft.name
            )));
        }
        validate_tunnel(&tunnel.draft)?;
        if !tunnels.insert((
            tunnel.source_host_id.clone(),
            tunnel.draft.name.to_ascii_lowercase(),
        )) {
            return Err(AppError::Validation(format!(
                "legacy tunnel '{}' is duplicated",
                tunnel.draft.name
            )));
        }
    }
    let mut commands = HashSet::new();
    for command in &plan.commands {
        if command
            .source_host_id
            .as_ref()
            .is_some_and(|id| !source_ids.contains(id))
        {
            return Err(AppError::Validation(format!(
                "legacy command '{}' references a missing host",
                command.draft.name
            )));
        }
        validate_command(&command.draft)?;
        if !commands.insert((
            command.source_host_id.clone(),
            command.draft.name.to_ascii_lowercase(),
        )) {
            return Err(AppError::Validation(format!(
                "legacy command '{}' is duplicated in its scope",
                command.draft.name
            )));
        }
    }
    if let Some(settings) = plan.settings.as_ref() {
        validate_settings(settings)?;
    }
    plan.warnings.sort();
    plan.warnings.dedup();
    Ok(())
}

fn validate_host(draft: &HostDraft) -> AppResult<()> {
    if !valid_alias(&draft.alias) {
        return Err(AppError::Validation(format!(
            "legacy host alias '{}' is invalid",
            draft.alias
        )));
    }
    validate_ssh_atom("hostname", &draft.hostname, 512)?;
    validate_ssh_atom("username", &draft.username, 128)?;
    if draft.port == 0 {
        return Err(AppError::Validation(
            "legacy host port is invalid".to_owned(),
        ));
    }
    if draft.auth_method == Some(AuthMethod::PrivateKey)
        && draft
            .identity_file
            .as_deref()
            .is_none_or(|path| path.trim().is_empty())
    {
        return Err(AppError::Validation(format!(
            "legacy private-key host '{}' has no identity file",
            draft.alias
        )));
    }
    if draft
        .identity_file
        .as_deref()
        .is_some_and(|path| path.len() > 32_767 || path.contains('\0'))
        || draft
            .default_workspace
            .as_deref()
            .is_some_and(|path| path.len() > 4096 || path.contains('\0'))
    {
        return Err(AppError::Validation(format!(
            "legacy host '{}' contains an invalid path",
            draft.alias
        )));
    }
    if let Some(proxy) = draft.proxy_jump.as_deref() {
        validate_ssh_atom("ProxyJump", proxy, 1024)?;
    }
    if draft.groups.len() > 32 {
        return Err(AppError::Validation(
            "legacy host has too many groups".to_owned(),
        ));
    }
    if let Some(advanced) = draft.advanced.as_ref()
        && (advanced
            .connect_timeout_seconds
            .is_some_and(|value| !(1..=300).contains(&value))
            || advanced
                .server_alive_interval_seconds
                .is_some_and(|value| value > 3600)
            || advanced
                .server_alive_count_max
                .is_some_and(|value| !(1..=100).contains(&value)))
    {
        return Err(AppError::Validation(format!(
            "legacy host '{}' has invalid SSH options",
            draft.alias
        )));
    }
    Ok(())
}

fn validate_tunnel(draft: &TunnelDraft) -> AppResult<()> {
    if draft.name.trim().is_empty()
        || draft.name.len() > 128
        || draft.name.chars().any(char::is_control)
        || draft.source_port == 0
        || draft.target_port == 0
    {
        return Err(AppError::Validation(
            "legacy tunnel metadata is invalid".to_owned(),
        ));
    }
    validate_forward_atom("bind address", &draft.bind_address)?;
    validate_forward_atom("target host", &draft.target_host)?;
    if let Some(health) = draft.health_check.as_ref()
        && health.kind == TunnelHealthCheckKind::Tcp
        && (!(2..=3600).contains(&health.interval_seconds)
            || !(1..=60).contains(&health.timeout_seconds)
            || health.timeout_seconds >= health.interval_seconds)
    {
        return Err(AppError::Validation(
            "legacy tunnel health check is invalid".to_owned(),
        ));
    }
    Ok(())
}

fn validate_command(draft: &CommandPresetDraft) -> AppResult<()> {
    if draft.name.trim().is_empty()
        || draft.name.len() > 512
        || draft.name.chars().any(char::is_control)
        || draft.description.len() > 2048
        || draft.description.contains('\0')
        || draft.group.len() > 128
        || draft.group.chars().any(char::is_control)
        || draft.command.trim().is_empty()
        || draft.command.len() > 32_768
        || draft.command.contains('\0')
        || draft
            .working_directory
            .as_deref()
            .is_some_and(|path| path.len() > 4096 || path.contains('\0'))
        || draft
            .confirmation_text
            .as_deref()
            .is_some_and(|text| text.len() > 256 || text.chars().any(char::is_control))
    {
        return Err(AppError::Validation(format!(
            "legacy command preset '{}' is invalid",
            draft.name
        )));
    }
    Ok(())
}

fn validate_settings(patch: &SettingsPatch) -> AppResult<()> {
    if patch.terminal_font_family.as_deref().is_some_and(|value| {
        value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control)
    }) || patch
        .terminal_font_size
        .is_some_and(|value| !(9..=32).contains(&value))
        || patch
            .telemetry_interval_seconds
            .is_some_and(|value| !(1..=60).contains(&value))
        || patch
            .telemetry_retention_minutes
            .is_some_and(|value| !(1..=1440).contains(&value))
        || patch
            .download_directory
            .as_deref()
            .is_some_and(|value| value.len() > 32_767 || value.contains('\0'))
        || patch
            .btop_rotation_minutes
            .is_some_and(|value| !(1..=1440).contains(&value))
        || patch.log_level.as_deref().is_some_and(|value| {
            !matches!(
                value.to_ascii_lowercase().as_str(),
                "debug" | "info" | "warn" | "error"
            )
        })
    {
        return Err(AppError::Validation(
            "legacy settings are invalid".to_owned(),
        ));
    }
    Ok(())
}

fn validate_ssh_atom(label: &str, value: &str, max: usize) -> AppResult<()> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > max
        || value.starts_with('-')
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
    {
        return Err(AppError::Validation(format!("legacy {label} is invalid")));
    }
    Ok(())
}

fn validate_forward_atom(label: &str, value: &str) -> AppResult<()> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 512
        || value.starts_with('-')
        || value.contains(',')
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
    {
        return Err(AppError::Validation(format!(
            "legacy tunnel {label} is invalid"
        )));
    }
    Ok(())
}

fn valid_alias(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
}

fn validate_hash(value: &str) -> AppResult<()> {
    if value.len() != 64 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(AppError::Validation(
            "sourceHash must be a 64-character SHA-256 value".to_owned(),
        ));
    }
    Ok(())
}

fn validate_source_path(value: &str) -> AppResult<()> {
    if value.trim().is_empty() || value.len() > 32_767 || value.contains('\0') {
        return Err(AppError::Validation(
            "legacy source path is invalid".to_owned(),
        ));
    }
    Ok(())
}

fn read_required_part(path: &Path, total: &mut u64) -> AppResult<Vec<u8>> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES.saturating_sub(*total) {
        return Err(AppError::Validation(
            "legacy source bundle exceeds the 4 MiB limit".to_owned(),
        ));
    }
    let bytes = fs::read(path)?;
    *total = total.saturating_add(bytes.len() as u64);
    Ok(bytes)
}

fn optional_file(path: PathBuf) -> Option<PathBuf> {
    path.is_file().then_some(path)
}

fn discover_user_ssh_config(total: &mut u64, warnings: &mut Vec<String>) -> Option<Vec<u8>> {
    let home = env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from);
    let Some(path) = home.map(|home| home.join(".ssh").join("config")) else {
        warnings.push(
            "Could not locate the user OpenSSH config; the LabPulse alias was imported with safe fallback values."
                .to_owned(),
        );
        return None;
    };
    if !path.is_file() {
        warnings.push(format!(
            "User OpenSSH config '{}' was not found; the LabPulse alias was imported with safe fallback values.",
            display_path(&path)
        ));
        return None;
    }
    match read_required_part(&path, total) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            warnings.push(format!(
                "Could not read user OpenSSH config '{}': {error}; safe fallback values were used.",
                display_path(&path)
            ));
            None
        }
    }
}

fn hash_parts(parts: &mut [SourcePart]) -> String {
    parts.sort_by_key(|part| part.role);
    let mut digest = Sha256::new();
    digest.update(b"RemoteDeck migration bundle v2\0");
    for part in parts {
        digest.update((part.role.len() as u64).to_be_bytes());
        digest.update(part.role.as_bytes());
        digest.update((part.bytes.len() as u64).to_be_bytes());
        digest.update(&part.bytes);
    }
    format!("{:x}", digest.finalize())
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct LabPulseConfig {
    app_name: String,
    ssh_host: String,
    forward_host: Option<String>,
    telemetry_interval_seconds: u64,
    btop: LabPulseBtop,
    forward: Option<LabPulseForward>,
    dangerous_command_patterns: Vec<String>,
    presets: Vec<LabPulsePreset>,
}

impl Default for LabPulseConfig {
    fn default() -> Self {
        Self {
            app_name: "LabPulse SSH".to_owned(),
            ssh_host: String::new(),
            forward_host: None,
            telemetry_interval_seconds: 3,
            btop: LabPulseBtop::default(),
            forward: None,
            dangerous_command_patterns: Vec::new(),
            presets: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct LabPulseBtop {
    enabled: bool,
    auto_restart: bool,
    command: String,
    restart_cycle_seconds: u64,
}

impl Default for LabPulseBtop {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_restart: false,
            command: "btop".to_owned(),
            restart_cycle_seconds: 900,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct LabPulseForward {
    auto_start: bool,
    auto_restart: bool,
    remote_bind_address: String,
    remote_port: u16,
    local_target_address: String,
    local_target_port: u16,
    watchdog_seconds: u64,
    release_command: Option<String>,
}

impl Default for LabPulseForward {
    fn default() -> Self {
        Self {
            auto_start: false,
            auto_restart: false,
            remote_bind_address: "127.0.0.1".to_owned(),
            remote_port: 0,
            local_target_address: "127.0.0.1".to_owned(),
            local_target_port: 0,
            watchdog_seconds: 5,
            release_command: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct LabPulsePreset {
    name: String,
    description: String,
    risk: String,
    confirm_token: Option<String>,
    command: String,
}

impl Default for LabPulsePreset {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            risk: "danger".to_owned(),
            confirm_token: None,
            command: String::new(),
        }
    }
}

fn map_labpulse(
    config: &LabPulseConfig,
    ssh_config: Option<&[u8]>,
    mut warnings: Vec<String>,
) -> AppResult<MigrationPlan> {
    validate_labpulse_config(config)?;
    if !valid_alias(config.ssh_host.trim()) {
        return Err(AppError::Validation(
            "LabPulse sshHost alias is invalid".to_owned(),
        ));
    }
    let alias = config.ssh_host.trim().to_owned();
    let fallback_user = fallback_username();
    let mut host = HostDraft {
        id: None,
        alias: alias.clone(),
        hostname: alias.clone(),
        port: 22,
        username: fallback_user,
        auth_method: Some(AuthMethod::Interactive),
        identity_file: None,
        proxy_jump: None,
        default_workspace: Some("~".to_owned()),
        groups: vec!["legacy-import".to_owned(), "labpulse".to_owned()],
        advanced: None,
        monitor_enabled: Some(true),
    };
    let mut matched = false;
    if let Some(bytes) = ssh_config {
        match std::str::from_utf8(bytes)
            .map_err(|_| "OpenSSH config is not valid UTF-8".to_owned())
            .and_then(|text| parse_open_ssh_config(text).map_err(|error| error.to_string()))
        {
            Ok(candidates) => {
                if let Some(candidate) = candidates
                    .iter()
                    .find(|candidate| candidate.alias.eq_ignore_ascii_case(&alias))
                {
                    matched = true;
                    host.hostname = candidate.hostname.clone();
                    host.port = candidate.port;
                    if let Some(username) = candidate.username.as_ref() {
                        host.username = username.clone();
                    } else {
                        warnings.push(format!(
                            "OpenSSH Host {alias} has no User directive; '{}' was used and must be reviewed.",
                            host.username
                        ));
                    }
                    host.identity_file = candidate.identity_file.clone();
                    host.auth_method = Some(if host.identity_file.is_some() {
                        AuthMethod::PrivateKey
                    } else {
                        AuthMethod::Interactive
                    });
                    host.proxy_jump = candidate.proxy_jump.clone();
                    host.advanced = Some(SshAdvancedPatch {
                        connect_timeout_seconds: candidate.connect_timeout_seconds,
                        server_alive_interval_seconds: candidate.server_alive_interval_seconds,
                        server_alive_count_max: candidate.server_alive_count_max,
                        tcp_keep_alive: candidate.tcp_keep_alive,
                        compression: candidate.compression,
                        identities_only: candidate.identities_only,
                    });
                    if candidate.unsupported_count > 0 {
                        warnings.push(format!(
                            "OpenSSH Host {alias} contains {} unsupported directive(s); review the imported host.",
                            candidate.unsupported_count
                        ));
                    }
                }
            }
            Err(error) => warnings.push(format!(
                "Could not parse the user OpenSSH config ({error}); safe fallback values were used."
            )),
        }
    }
    if !matched {
        warnings.push(format!(
            "OpenSSH alias {alias} could not be resolved; hostname '{alias}', port 22, and user '{}' were used as safe reviewable fallbacks.",
            host.username
        ));
    }
    warnings.push(
        "Legacy SSH host trust is never inherited; verify the server fingerprint on first connection."
            .to_owned(),
    );
    if config
        .forward
        .as_ref()
        .and_then(|forward| forward.release_command.as_ref())
        .is_some()
    {
        warnings.push(
            "The LabPulse cleanup command was not imported and will never run automatically."
                .to_owned(),
        );
    }
    if config.btop.command.trim() != "btop" {
        warnings.push(
            "Custom LabPulse btop arguments were not imported; RemoteDeck uses its own fixed btop flow."
                .to_owned(),
        );
    }
    if !config.dangerous_command_patterns.is_empty() {
        warnings.push(
            "Legacy regular-expression risk rules were not imported; every command is reclassified by the Rust risk engine."
                .to_owned(),
        );
    }

    let source_id = format!("labpulse:{alias}");
    let tunnels = config
        .forward
        .as_ref()
        .map(|forward| PlannedTunnel {
            source_host_id: source_id.clone(),
            draft: TunnelDraft {
                id: None,
                host_id: source_id.clone(),
                name: format!(
                    "Legacy {}",
                    config.forward_host.as_deref().unwrap_or("RemoteForward")
                ),
                direction: TunnelDirection::Remote,
                bind_address: forward.remote_bind_address.clone(),
                source_port: forward.remote_port,
                target_host: forward.local_target_address.clone(),
                target_port: forward.local_target_port,
                auto_start: Some(forward.auto_start),
                auto_reconnect: Some(forward.auto_restart),
                health_check: Some(tcp_health(forward.watchdog_seconds.clamp(2, 3600), 1)),
            },
        })
        .into_iter()
        .collect();
    let commands = config
        .presets
        .iter()
        .enumerate()
        .map(|(index, preset)| {
            let risk = match preset.risk.to_ascii_lowercase().as_str() {
                "safe" => CommandRisk::L0,
                "danger" => CommandRisk::L2,
                other => {
                    return Err(AppError::Validation(format!(
                        "unknown LabPulse command risk '{other}'"
                    )));
                }
            };
            Ok(PlannedCommand {
                source_host_id: Some(source_id.clone()),
                draft: CommandPresetDraft {
                    id: None,
                    host_id: Some(source_id.clone()),
                    name: preset.name.clone(),
                    description: preset.description.clone(),
                    group: "Legacy migration".to_owned(),
                    command: preset.command.clone(),
                    working_directory: None,
                    risk,
                    requires_pty: risk == CommandRisk::L2,
                    requires_sudo: false,
                    confirmation_text: preset
                        .confirm_token
                        .clone()
                        .or_else(|| (risk == CommandRisk::L2).then(|| alias.clone())),
                    sort_order: i32::try_from(index).unwrap_or(i32::MAX),
                },
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let rotation = config
        .btop
        .restart_cycle_seconds
        .div_ceil(60)
        .clamp(1, 1440);
    Ok(MigrationPlan {
        app_name: config.app_name.clone(),
        hosts: vec![PlannedHost {
            source_id,
            draft: host,
        }],
        tunnels,
        commands,
        settings: Some(SettingsPatch {
            telemetry_interval_seconds: Some(config.telemetry_interval_seconds),
            btop_watchdog_enabled: Some(config.btop.enabled && config.btop.auto_restart),
            btop_rotation_minutes: Some(rotation),
            ..SettingsPatch::default()
        }),
        warnings,
    })
}

fn validate_labpulse_config(config: &LabPulseConfig) -> AppResult<()> {
    if config.app_name.trim().is_empty()
        || config.app_name.len() > 512
        || config.app_name.chars().any(char::is_control)
        || !(1..=60).contains(&config.telemetry_interval_seconds)
        || config.btop.command.len() > 32_768
        || !(60..=86_400).contains(&config.btop.restart_cycle_seconds)
        || config.dangerous_command_patterns.len() > 128
        || config
            .dangerous_command_patterns
            .iter()
            .any(|pattern| pattern.is_empty() || pattern.len() > 1024 || pattern.contains('\0'))
        || config.presets.len() > 256
    {
        return Err(AppError::Validation(
            "LabPulse configuration is invalid".to_owned(),
        ));
    }
    if let Some(forward) = config.forward.as_ref()
        && (forward.watchdog_seconds == 0
            || forward.watchdog_seconds > 3600
            || forward.release_command.as_deref().is_some_and(|command| {
                command.trim().is_empty() || command.len() > 16_384 || command.contains('\0')
            }))
    {
        return Err(AppError::Validation(
            "LabPulse forward configuration is invalid".to_owned(),
        ));
    }
    Ok(())
}

fn fallback_username() -> String {
    env::var("USERNAME")
        .or_else(|_| env::var("USER"))
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && !value.starts_with('-')
                && !value.chars().any(char::is_whitespace)
                && !value.chars().any(char::is_control)
        })
        .unwrap_or_else(|| "user".to_owned())
}

fn tcp_health(interval_seconds: u64, timeout_seconds: u64) -> TunnelHealthCheck {
    let interval_seconds = interval_seconds.clamp(2, 3600);
    TunnelHealthCheck {
        kind: TunnelHealthCheckKind::Tcp,
        interval_seconds,
        timeout_seconds: timeout_seconds.clamp(1, 60).min(interval_seconds - 1),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct V1Database {
    schema_version: u8,
    hosts: Vec<V1Host>,
    auth_profiles: Vec<V1Auth>,
    workspaces: Vec<V1Workspace>,
    host_keys: Vec<serde_json::Value>,
    tunnels: Vec<V1Tunnel>,
    commands: Vec<V1Command>,
    private_keys: Vec<serde_json::Value>,
    imports: Vec<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct V1Host {
    id: String,
    alias: String,
    hostname: String,
    port: u16,
    username: String,
    auth_profile_id: String,
    jump_host_id: Option<String>,
    default_workspace_id: Option<String>,
    groups: Vec<String>,
    advanced: V1Advanced,
    monitor_enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct V1Advanced {
    connect_timeout_seconds: u64,
    server_alive_interval_seconds: u64,
    server_alive_count_max: u32,
    tcp_keep_alive: bool,
    compression: bool,
    identities_only: bool,
}

impl Default for V1Advanced {
    fn default() -> Self {
        Self {
            connect_timeout_seconds: 15,
            server_alive_interval_seconds: 30,
            server_alive_count_max: 3,
            tcp_keep_alive: true,
            compression: false,
            identities_only: false,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct V1Auth {
    id: String,
    method: String,
    identity_file: Option<String>,
    agent: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct V1Workspace {
    id: String,
    host_id: String,
    remote_path: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct V1Tunnel {
    host_id: String,
    name: String,
    direction: Option<TunnelDirection>,
    bind_address: String,
    source_port: u16,
    target_host: String,
    target_port: u16,
    auto_start: bool,
    health_check: Option<V1HealthCheck>,
    legacy_cleanup_hook: Option<V1CleanupHook>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum V1HealthCheck {
    Tcp {
        #[serde(rename = "intervalSeconds")]
        interval_seconds: u64,
        #[serde(rename = "timeoutMs")]
        timeout_ms: u64,
    },
    Http {
        #[serde(rename = "intervalSeconds")]
        interval_seconds: u64,
        #[serde(rename = "timeoutMs")]
        timeout_ms: u64,
        path: String,
        #[serde(rename = "expectedStatus")]
        expected_status: u16,
    },
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct V1CleanupHook {
    command: String,
    authorized: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct V1Command {
    host_id: Option<String>,
    name: String,
    description: String,
    group: String,
    command: String,
    working_directory: Option<String>,
    risk: Option<CommandRisk>,
    requires_pty: bool,
    requires_sudo: bool,
    confirmation_text: Option<String>,
    sort_order: i32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct V1Settings {
    schema_version: u8,
    terminal_font_family: Option<String>,
    terminal_font_size: Option<u8>,
    telemetry_interval_seconds: Option<u64>,
    telemetry_retention_minutes: Option<u64>,
    ssh_config_path: Option<String>,
    download_directory: Option<String>,
    auto_reconnect: Option<bool>,
    close_to_tray: Option<bool>,
    launch_at_login: Option<bool>,
    btop_watchdog_enabled: Option<bool>,
    btop_rotation_minutes: Option<u64>,
    log_level: Option<String>,
    onboarding_completed: Option<bool>,
}

fn map_v1(database: V1Database, settings: Option<V1Settings>) -> AppResult<MigrationPlan> {
    if database.schema_version != 1 {
        return Err(AppError::Validation(format!(
            "unsupported RemoteDeck profiles schema version {}",
            database.schema_version
        )));
    }
    if settings
        .as_ref()
        .is_some_and(|settings| settings.schema_version != 1)
    {
        return Err(AppError::Validation(
            "unsupported RemoteDeck settings schema version".to_owned(),
        ));
    }
    let auth = unique_by_id(database.auth_profiles, |value| &value.id, "auth profile")?;
    let workspaces = unique_by_id(database.workspaces, |value| &value.id, "workspace")?;
    let source_hosts = unique_by_id(database.hosts, |value| &value.id, "host")?;
    let mut warnings = Vec::new();
    warnings.push(format!(
        "RemoteDeck v1 host trust records are deliberately not imported ({} record(s)); verify every server fingerprint again.",
        database.host_keys.len()
    ));
    if !database.private_keys.is_empty() {
        warnings.push(format!(
            "RemoteDeck v1 private-key metadata was not copied ({} record(s)); keys remain at their existing filesystem paths and can be rediscovered.",
            database.private_keys.len()
        ));
    }
    if !database.imports.is_empty() {
        warnings.push(
            "RemoteDeck v1 import history was not inherited; only this reviewed source bundle is recorded."
                .to_owned(),
        );
    }
    if settings
        .as_ref()
        .and_then(|settings| settings.ssh_config_path.as_deref())
        .is_some_and(|path| !path.trim().is_empty())
    {
        warnings.push(
            "The v1 sshConfigPath setting is machine-specific and was not imported.".to_owned(),
        );
    }

    let mut hosts = Vec::with_capacity(source_hosts.len());
    let mut host_order = source_hosts.keys().cloned().collect::<Vec<_>>();
    host_order.sort_by(|left, right| {
        source_hosts[left]
            .alias
            .cmp(&source_hosts[right].alias)
            .then_with(|| left.cmp(right))
    });
    for source_host_id in host_order {
        let host = &source_hosts[&source_host_id];
        let auth = auth.get(&host.auth_profile_id).ok_or_else(|| {
            AppError::Validation(format!(
                "RemoteDeck v1 host '{}' references a missing auth profile",
                host.alias
            ))
        })?;
        let (auth_method, identity_file) = match auth.method.as_str() {
            "password" | "keyboard_interactive" => {
                warnings.push(format!(
                    "Credentials for v1 host '{}' are not imported; authenticate interactively after fingerprint verification.",
                    host.alias
                ));
                (AuthMethod::Interactive, None)
            }
            "private_key" => (AuthMethod::PrivateKey, auth.identity_file.clone()),
            "agent" => {
                match auth.agent.as_deref() {
                    Some("windows_openssh") => {}
                    Some("pageant") => warnings.push(format!(
                        "v1 host '{}' used Pageant; select a supported OpenSSH agent if the imported profile cannot authenticate.",
                        host.alias
                    )),
                    _ => {
                        return Err(AppError::Validation(format!(
                            "RemoteDeck v1 host '{}' has invalid agent authentication metadata",
                            host.alias
                        )));
                    }
                }
                (AuthMethod::Agent, None)
            }
            other => {
                return Err(AppError::Validation(format!(
                    "RemoteDeck v1 host '{}' uses unknown auth method '{other}'",
                    host.alias
                )));
            }
        };
        let workspace = host
            .default_workspace_id
            .as_deref()
            .map(|id| {
                workspaces.get(id).ok_or_else(|| {
                    AppError::Validation(format!(
                        "RemoteDeck v1 host '{}' references a missing workspace",
                        host.alias
                    ))
                })
            })
            .transpose()?;
        if workspace.is_some_and(|workspace| workspace.host_id != host.id) {
            return Err(AppError::Validation(format!(
                "RemoteDeck v1 workspace ownership is invalid for host '{}'",
                host.alias
            )));
        }
        let proxy_jump = host
            .jump_host_id
            .as_deref()
            .map(|id| {
                let jump = source_hosts.get(id).ok_or_else(|| {
                    AppError::Validation(format!(
                        "RemoteDeck v1 host '{}' references a missing jump host",
                        host.alias
                    ))
                })?;
                if jump.jump_host_id.is_some() {
                    return Err(AppError::Validation(
                        "nested ProxyJump chains are not supported".to_owned(),
                    ));
                }
                Ok(proxy_destination(jump))
            })
            .transpose()?;
        hosts.push(PlannedHost {
            source_id: host.id.clone(),
            draft: HostDraft {
                id: None,
                alias: host.alias.clone(),
                hostname: host.hostname.clone(),
                port: host.port,
                username: host.username.clone(),
                auth_method: Some(auth_method),
                identity_file,
                proxy_jump,
                default_workspace: Some(
                    workspace
                        .map(|workspace| workspace.remote_path.clone())
                        .unwrap_or_else(|| "~".to_owned()),
                ),
                groups: host.groups.clone(),
                advanced: Some(SshAdvancedPatch {
                    connect_timeout_seconds: Some(host.advanced.connect_timeout_seconds),
                    server_alive_interval_seconds: Some(
                        host.advanced.server_alive_interval_seconds,
                    ),
                    server_alive_count_max: Some(host.advanced.server_alive_count_max),
                    tcp_keep_alive: Some(host.advanced.tcp_keep_alive),
                    compression: Some(host.advanced.compression),
                    identities_only: Some(host.advanced.identities_only),
                }),
                monitor_enabled: Some(host.monitor_enabled),
            },
        });
    }

    let auto_reconnect = settings
        .as_ref()
        .and_then(|settings| settings.auto_reconnect)
        .unwrap_or(false);
    let mut tunnels = Vec::with_capacity(database.tunnels.len());
    for tunnel in database.tunnels {
        let health_check = match tunnel.health_check {
            Some(V1HealthCheck::Tcp {
                interval_seconds,
                timeout_ms,
            }) => {
                if !(2..=3600).contains(&interval_seconds) || !(100..=60_000).contains(&timeout_ms)
                {
                    return Err(AppError::Validation(format!(
                        "TCP health check on v1 tunnel '{}' is invalid",
                        tunnel.name
                    )));
                }
                Some(tcp_health(interval_seconds, timeout_ms.div_ceil(1000)))
            }
            Some(V1HealthCheck::Http {
                interval_seconds,
                timeout_ms,
                path,
                expected_status,
            }) => {
                if !(2..=3600).contains(&interval_seconds)
                    || !(100..=60_000).contains(&timeout_ms)
                    || !path.starts_with('/')
                    || path.len() > 2048
                    || !(100..=599).contains(&expected_status)
                {
                    return Err(AppError::Validation(format!(
                        "HTTP health check on v1 tunnel '{}' is invalid",
                        tunnel.name
                    )));
                }
                warnings.push(format!(
                    "HTTP health check on v1 tunnel '{}' ({path}, expected {expected_status}, {interval_seconds}s/{timeout_ms}ms) is unsupported and was disabled.",
                    tunnel.name
                ));
                None
            }
            None => None,
        };
        if let Some(cleanup) = tunnel.legacy_cleanup_hook {
            if cleanup.command.trim().is_empty()
                || cleanup.command.len() > 16_384
                || cleanup.command.contains('\0')
            {
                return Err(AppError::Validation(format!(
                    "cleanup hook on v1 tunnel '{}' is invalid",
                    tunnel.name
                )));
            }
            warnings.push(format!(
                "Cleanup hook on v1 tunnel '{}' was not imported{} and will never run automatically.",
                tunnel.name,
                if cleanup.authorized { " (it was previously authorized)" } else { "" }
            ));
        }
        tunnels.push(PlannedTunnel {
            source_host_id: tunnel.host_id.clone(),
            draft: TunnelDraft {
                id: None,
                host_id: tunnel.host_id,
                name: tunnel.name,
                direction: tunnel.direction.ok_or_else(|| {
                    AppError::Validation("RemoteDeck v1 tunnel direction is missing".to_owned())
                })?,
                bind_address: tunnel.bind_address,
                source_port: tunnel.source_port,
                target_host: tunnel.target_host,
                target_port: tunnel.target_port,
                auto_start: Some(tunnel.auto_start),
                auto_reconnect: Some(auto_reconnect),
                health_check,
            },
        });
    }
    let commands = database
        .commands
        .into_iter()
        .map(|command| {
            let source_host_id = command.host_id.clone();
            Ok(PlannedCommand {
                source_host_id,
                draft: CommandPresetDraft {
                    id: None,
                    host_id: command.host_id,
                    name: command.name,
                    description: command.description,
                    group: command.group,
                    command: command.command,
                    working_directory: command.working_directory,
                    risk: command.risk.ok_or_else(|| {
                        AppError::Validation("RemoteDeck v1 command risk is missing".to_owned())
                    })?,
                    requires_pty: command.requires_pty,
                    requires_sudo: command.requires_sudo,
                    confirmation_text: command.confirmation_text,
                    sort_order: command.sort_order,
                },
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let settings_patch = settings.map(|settings| SettingsPatch {
        terminal_font_family: settings.terminal_font_family,
        terminal_font_size: settings.terminal_font_size,
        telemetry_interval_seconds: settings.telemetry_interval_seconds,
        telemetry_retention_minutes: settings.telemetry_retention_minutes,
        download_directory: settings.download_directory,
        auto_reconnect: settings.auto_reconnect,
        close_to_tray: settings.close_to_tray,
        launch_at_login: settings.launch_at_login,
        btop_watchdog_enabled: settings.btop_watchdog_enabled,
        btop_rotation_minutes: settings.btop_rotation_minutes,
        log_level: settings.log_level,
        onboarding_completed: settings.onboarding_completed,
        default_agent: None,
    });
    if settings_patch.is_none() {
        warnings.push(
            "No sibling settings.json was found; RemoteDeck v1 settings are not included."
                .to_owned(),
        );
    }
    Ok(MigrationPlan {
        app_name: "RemoteDeck v1".to_owned(),
        hosts,
        tunnels,
        commands,
        settings: settings_patch,
        warnings,
    })
}

fn unique_by_id<T>(
    values: Vec<T>,
    id: impl Fn(&T) -> &String,
    label: &str,
) -> AppResult<HashMap<String, T>> {
    let mut result = HashMap::new();
    for value in values {
        let key = id(&value).clone();
        if key.is_empty() || result.insert(key, value).is_some() {
            return Err(AppError::Validation(format!(
                "RemoteDeck v1 {label} ids are missing or duplicated"
            )));
        }
    }
    Ok(result)
}

fn proxy_destination(host: &V1Host) -> String {
    let hostname = if host.hostname.contains(':')
        && !(host.hostname.starts_with('[') && host.hostname.ends_with(']'))
    {
        format!("[{}]", host.hostname)
    } else {
        host.hostname.clone()
    };
    format!("{}@{}:{}", host.username, hostname, host.port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temporary_directory(label: &str) -> PathBuf {
        env::temp_dir().join(format!("remotedeck-migration-{label}-{}", Uuid::new_v4()))
    }

    fn all(source_path: String, source_hash: String) -> LegacyApplyRequest {
        LegacyApplyRequest {
            source_path,
            source_hash,
            include_hosts: true,
            include_tunnels: true,
            include_commands: true,
            include_settings: true,
        }
    }

    #[test]
    fn labpulse_maps_openssh_alias_and_falls_back_safely() {
        let config: LabPulseConfig = serde_json::from_value(serde_json::json!({
            "sshHost": "lab",
            "forward": { "remotePort": 17890, "localTargetPort": 7890 },
            "presets": [{ "name": "overview", "risk": "safe", "command": "uptime" }]
        }))
        .expect("config");
        let ssh = b"Host lab\n  HostName gpu.example.test\n  User alice\n  Port 2222\n  IdentityFile ~/.ssh/id_ed25519\n";
        let mut mapped = map_labpulse(&config, Some(ssh), Vec::new()).expect("mapped");
        validate_plan(&mut mapped).expect("valid");
        assert_eq!(mapped.hosts[0].draft.hostname, "gpu.example.test");
        assert_eq!(mapped.hosts[0].draft.username, "alice");
        assert_eq!(mapped.hosts[0].draft.port, 2222);
        assert_eq!(
            mapped.hosts[0].draft.auth_method,
            Some(AuthMethod::PrivateKey)
        );
        assert_eq!(mapped.tunnels.len(), 1);

        let fallback = map_labpulse(&config, None, Vec::new()).expect("fallback");
        assert_eq!(fallback.hosts[0].draft.hostname, "lab");
        assert!(
            fallback
                .warnings
                .iter()
                .any(|warning| warning.contains("fallback"))
        );
        assert!(
            fallback
                .warnings
                .iter()
                .any(|warning| warning.contains("fingerprint"))
        );
    }

    #[test]
    fn repository_labpulse_fixture_remains_preview_compatible() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../legacy/labpulse-v0.1.0/config.json");
        assert!(fixture.is_file(), "historical LabPulse fixture is missing");
        let directory = temporary_directory("labpulse-repository-fixture");
        let repository = AppRepository::open(directory.clone()).expect("repository");
        let inspected = preview(&repository, &display_path(&fixture)).expect("preview fixture");
        assert_eq!(inspected.app_name, "LabPulse SSH");
        assert_eq!(inspected.hosts.len(), 1);
        assert_eq!(inspected.command_count, 8);
        assert_eq!(inspected.tunnel_count, 1);
        assert!(
            inspected
                .warnings
                .iter()
                .any(|warning| warning.contains("cleanup"))
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn changed_labpulse_source_is_rejected_before_any_write() {
        let directory = temporary_directory("tamper");
        fs::create_dir_all(&directory).expect("directory");
        let source = directory.join("config.json");
        fs::write(
            &source,
            br#"{"sshHost":"migrationtamper","presets":[{"name":"one","risk":"safe","command":"uptime"}]}"#,
        )
        .expect("source");
        let repository = AppRepository::open(directory.join("state")).expect("repository");
        let first = preview(&repository, &display_path(&source)).expect("preview");
        fs::write(
            &source,
            br#"{"sshHost":"migrationtamper","presets":[{"name":"two","risk":"safe","command":"uname -a"}]}"#,
        )
        .expect("tamper");
        let error = apply(&repository, all(display_path(&source), first.source_hash))
            .expect_err("changed source");
        assert!(error.to_string().contains("changed after preview"));
        assert!(repository.snapshot().hosts.is_empty());
        assert!(repository.snapshot().command_presets.is_empty());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn source_bundle_over_four_mib_is_rejected_without_reading_it() {
        let directory = temporary_directory("limit");
        fs::create_dir_all(&directory).expect("directory");
        let source = directory.join("config.json");
        let file = fs::File::create(&source).expect("source");
        file.set_len(MAX_SOURCE_BYTES + 1).expect("sparse length");
        drop(file);
        let repository = AppRepository::open(directory.join("state")).expect("repository");
        let error = preview(&repository, &display_path(&source)).expect_err("oversized");
        assert!(error.to_string().contains("4 MiB"));
        assert!(repository.snapshot().hosts.is_empty());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn v1_bundle_maps_all_categories_and_is_idempotent() {
        let directory = temporary_directory("v1");
        fs::create_dir_all(&directory).expect("directory");
        let profiles = directory.join("profiles.json");
        let settings = directory.join("settings.json");
        fs::write(
            &profiles,
            serde_json::to_vec_pretty(&v1_fixture()).expect("profiles json"),
        )
        .expect("profiles");
        fs::write(
            &settings,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schemaVersion": 1,
                "terminalFontFamily": "Cascadia Mono",
                "terminalFontSize": 15,
                "telemetryIntervalSeconds": 7,
                "telemetryRetentionMinutes": 45,
                "downloadDirectory": "C:/Downloads",
                "autoReconnect": true,
                "closeToTray": false,
                "launchAtLogin": true,
                "btopWatchdogEnabled": true,
                "btopRotationMinutes": 20,
                "logLevel": "warn",
                "onboardingCompleted": true
            }))
            .expect("settings json"),
        )
        .expect("settings");
        let repository = AppRepository::open(directory.join("state")).expect("repository");

        let inspected = preview(&repository, &display_path(&directory)).expect("preview");
        assert_eq!(inspected.app_name, "RemoteDeck v1");
        assert_eq!(inspected.hosts.len(), 2);
        assert_eq!(inspected.tunnel_count, 1);
        assert_eq!(inspected.command_count, 1);
        assert!(inspected.settings_included);
        assert!(
            inspected
                .warnings
                .iter()
                .any(|warning| warning.contains("host trust"))
        );

        let request = all(inspected.source_path.clone(), inspected.source_hash.clone());
        let imported = apply(&repository, request.clone()).expect("apply");
        assert_eq!(imported.imported_hosts, 2);
        assert_eq!(imported.imported_tunnels, 1);
        assert_eq!(imported.imported_commands, 1);
        assert!(imported.settings_imported);
        let state = repository.snapshot();
        let target = state
            .hosts
            .iter()
            .find(|host| host.alias == "target")
            .expect("target");
        assert_eq!(target.auth_method, AuthMethod::PrivateKey);
        assert_eq!(target.default_workspace, "/srv/work");
        let jump = state
            .hosts
            .iter()
            .find(|host| host.alias == "jump")
            .expect("jump");
        assert_eq!(target.proxy_jump.as_deref(), Some(jump.id.as_str()));
        assert!(state.tunnels[0].auto_reconnect);
        assert_eq!(state.settings.telemetry_interval_seconds, 7);

        let repeated = apply(&repository, request).expect("duplicate");
        assert_eq!(repeated.imported_hosts, 0);
        assert_eq!(repeated.imported_tunnels, 0);
        assert_eq!(repeated.imported_commands, 0);
        assert_eq!(repository.snapshot().hosts.len(), 2);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn bundle_hash_covers_settings_and_profiles_and_accepts_settings_path() {
        let directory = temporary_directory("hash");
        fs::create_dir_all(&directory).expect("directory");
        let profiles = directory.join("profiles.json");
        let settings = directory.join("settings.json");
        fs::write(
            &profiles,
            serde_json::to_vec(&v1_fixture()).expect("profiles json"),
        )
        .expect("profiles");
        fs::write(
            &settings,
            br#"{"schemaVersion":1,"telemetryIntervalSeconds":3}"#,
        )
        .expect("settings");
        let repository = AppRepository::open(directory.join("state")).expect("repository");
        let first = preview(&repository, &display_path(&settings)).expect("settings path");
        fs::write(
            &settings,
            br#"{"schemaVersion":1,"telemetryIntervalSeconds":4}"#,
        )
        .expect("settings changed");
        let second = preview(&repository, &display_path(&profiles)).expect("profiles path");
        assert_ne!(first.source_hash, second.source_hash);
        fs::write(
            &profiles,
            serde_json::to_vec_pretty(&v1_fixture()).expect("profiles changed"),
        )
        .expect("profiles changed");
        let third = preview(&repository, &display_path(&directory)).expect("directory path");
        assert_ne!(second.source_hash, third.source_hash);
        let _ = fs::remove_dir_all(directory);
    }

    fn v1_fixture() -> serde_json::Value {
        serde_json::json!({
            "schemaVersion": 1,
            "hosts": [
                {
                    "schemaVersion": 1,
                    "id": "11111111-1111-4111-8111-111111111111",
                    "alias": "jump",
                    "hostname": "gateway.example",
                    "port": 22,
                    "username": "jump",
                    "authProfileId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
                    "groups": ["gateway"],
                    "advanced": {},
                    "monitorEnabled": true
                },
                {
                    "schemaVersion": 1,
                    "id": "22222222-2222-4222-8222-222222222222",
                    "alias": "target",
                    "hostname": "target.example",
                    "port": 2222,
                    "username": "alice",
                    "authProfileId": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
                    "jumpHostId": "11111111-1111-4111-8111-111111111111",
                    "defaultWorkspaceId": "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
                    "groups": ["gpu"],
                    "advanced": { "compression": true },
                    "monitorEnabled": true
                }
            ],
            "authProfiles": [
                { "schemaVersion": 1, "id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", "method": "password" },
                { "schemaVersion": 1, "id": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb", "method": "private_key", "identityFile": "~/.ssh/id_ed25519" }
            ],
            "workspaces": [
                { "schemaVersion": 1, "id": "cccccccc-cccc-4ccc-8ccc-cccccccccccc", "hostId": "22222222-2222-4222-8222-222222222222", "remotePath": "/srv/work" }
            ],
            "hostKeys": [
                { "id": "dddddddd-dddd-4ddd-8ddd-dddddddddddd", "host": "target.example" }
            ],
            "tunnels": [
                {
                    "hostId": "22222222-2222-4222-8222-222222222222",
                    "name": "notebook",
                    "direction": "local",
                    "bindAddress": "127.0.0.1",
                    "sourcePort": 8888,
                    "targetHost": "127.0.0.1",
                    "targetPort": 8888,
                    "autoStart": true,
                    "healthCheck": { "type": "tcp", "intervalSeconds": 5, "timeoutMs": 2000 }
                }
            ],
            "commands": [
                {
                    "hostId": "22222222-2222-4222-8222-222222222222",
                    "name": "status",
                    "description": "status",
                    "group": "v1",
                    "command": "uptime",
                    "risk": "L0",
                    "requiresPty": false,
                    "requiresSudo": false,
                    "sortOrder": 1
                }
            ]
        })
    }
}
