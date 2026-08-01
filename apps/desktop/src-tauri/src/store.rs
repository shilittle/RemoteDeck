use crate::{
    error::{AppError, AppResult},
    model::{
        AppSettings, AuthMethod, CommandPreset, CommandPresetDraft, HostDraft, HostProfile,
        PersistedState, SettingsPatch, SshAdvancedOptions, TunnelDraft, TunnelHealthCheckKind,
        TunnelProfile,
    },
};
use chrono::Utc;
use parking_lot::RwLock;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

const MAX_STATE_BYTES: usize = 16 * 1024 * 1024;
const MAX_HOSTS: usize = 512;
const MAX_TUNNELS: usize = 2_048;
const MAX_COMMAND_PRESETS: usize = 4_096;
const MAX_IMPORTED_SOURCE_HASHES: usize = 4_096;

#[derive(Debug, Clone)]
pub struct AppRepository {
    state_path: PathBuf,
    known_hosts_path: PathBuf,
    state: Arc<RwLock<PersistedState>>,
    deleting_hosts: Arc<RwLock<HashSet<String>>>,
    btop_owner_nonce: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportHostReference {
    Imported(String),
    Existing(String),
}

#[derive(Debug, Clone)]
pub struct ImportBatchHost {
    pub source_id: String,
    pub draft: HostDraft,
}

#[derive(Debug, Clone)]
pub struct ImportBatchTunnel {
    pub host: ImportHostReference,
    pub draft: TunnelDraft,
}

#[derive(Debug, Clone)]
pub struct ImportBatchCommand {
    pub host: Option<ImportHostReference>,
    pub draft: CommandPresetDraft,
}

#[derive(Debug, Clone)]
pub struct ImportBatch {
    pub source_hash: String,
    pub hosts: Vec<ImportBatchHost>,
    pub tunnels: Vec<ImportBatchTunnel>,
    pub commands: Vec<ImportBatchCommand>,
    pub settings: Option<SettingsPatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportBatchResult {
    pub duplicate: bool,
    pub imported_hosts: usize,
    pub imported_tunnels: usize,
    pub imported_commands: usize,
    pub settings_imported: bool,
}

impl AppRepository {
    pub fn open(directory: PathBuf) -> AppResult<Self> {
        fs::create_dir_all(&directory)?;
        let btop_owner_nonce = load_or_create_btop_owner_nonce(&directory)?;
        let state_path = directory.join("state-v2.json");
        let known_hosts_path = directory.join("known_hosts");
        if !known_hosts_path.exists() {
            File::create(&known_hosts_path)?.sync_all()?;
        }
        let mut state = if state_path.exists() {
            match read_valid_state(&state_path) {
                Ok(parsed) => parsed,
                Err(primary_error) => {
                    let backup = state_path.with_extension("json.bak");
                    let recovered = read_valid_state(&backup).map_err(|backup_error| {
                        AppError::State(format!(
                            "state and backup are both invalid; state: {primary_error}; backup: {backup_error}"
                        ))
                    })?;
                    write_state(&state_path, &recovered)?;
                    recovered
                }
            }
        } else {
            let initial = PersistedState::default();
            write_state(&state_path, &initial)?;
            initial
        };
        if clear_inactive_identity_files(&mut state.hosts) {
            write_state(&state_path, &state)?;
        }
        Ok(Self {
            state_path,
            known_hosts_path,
            state: Arc::new(RwLock::new(state)),
            deleting_hosts: Arc::new(RwLock::new(HashSet::new())),
            btop_owner_nonce,
        })
    }
    pub fn known_hosts_path(&self) -> &Path {
        &self.known_hosts_path
    }
    pub fn btop_owner_nonce(&self) -> Arc<str> {
        self.btop_owner_nonce.clone()
    }
    pub fn snapshot(&self) -> PersistedState {
        self.state.read().clone()
    }

    pub fn host(&self, host_id: &str) -> AppResult<HostProfile> {
        let deleting_hosts = self.deleting_hosts.read();
        if deleting_hosts.contains(host_id) {
            return Err(AppError::State(format!("host {host_id} is being deleted")));
        }
        self.state
            .read()
            .hosts
            .iter()
            .find(|host| host.id == host_id)
            .cloned()
            .ok_or_else(|| AppError::NotFound(format!("host {host_id}")))
    }

    pub fn resolve_jump_host(&self, host: &HostProfile) -> AppResult<Option<HostProfile>> {
        let deleting_hosts = self.deleting_hosts.read();
        // A captured target profile may finish app-owned cleanup after its id
        // is marked deleting. New work must first call `host()`, which rejects
        // that id; only its already-validated ProxyJump lookup remains usable.
        let hosts = &self.state.read().hosts;
        let jump = resolve_proxy_jump(hosts, host)?;
        if jump
            .as_ref()
            .is_some_and(|jump| deleting_hosts.contains(&jump.id))
        {
            return Err(AppError::State(
                "ProxyJump host is being deleted".to_owned(),
            ));
        }
        Ok(jump)
    }

    pub fn save_host(&self, draft: HostDraft) -> AppResult<HostProfile> {
        let deleting_hosts = self.deleting_hosts.read();
        if draft
            .id
            .as_ref()
            .is_some_and(|host_id| deleting_hosts.contains(host_id))
        {
            return Err(AppError::State(
                "cannot update a host while it is being deleted".to_owned(),
            ));
        }
        let mut guard = self.state.write();
        let mut state = guard.clone();
        let profile = apply_host_update(&mut state, draft)?;
        if resolve_proxy_jump(&state.hosts, &profile)?
            .as_ref()
            .is_some_and(|jump| deleting_hosts.contains(&jump.id))
        {
            return Err(AppError::State(
                "cannot use a ProxyJump host while it is being deleted".to_owned(),
            ));
        }
        persist_locked(&self.state_path, &state)?;
        *guard = state;
        Ok(profile)
    }

    pub fn preview_host(&self, draft: HostDraft) -> AppResult<HostProfile> {
        let deleting_hosts = self.deleting_hosts.read();
        if draft
            .id
            .as_ref()
            .is_some_and(|host_id| deleting_hosts.contains(host_id))
        {
            return Err(AppError::State(
                "cannot update a host while it is being deleted".to_owned(),
            ));
        }
        let mut state = self.state.read().clone();
        let profile = apply_host_update(&mut state, draft)?;
        if resolve_proxy_jump(&state.hosts, &profile)?
            .as_ref()
            .is_some_and(|jump| deleting_hosts.contains(&jump.id))
        {
            return Err(AppError::State(
                "cannot use a ProxyJump host while it is being deleted".to_owned(),
            ));
        }
        Ok(profile)
    }

    pub fn import_hosts(&self, drafts: Vec<HostDraft>) -> AppResult<Vec<HostProfile>> {
        if drafts.is_empty() {
            return Ok(Vec::new());
        }
        let deleting_hosts = self.deleting_hosts.read();
        if !deleting_hosts.is_empty() {
            return Err(AppError::State(
                "cannot import hosts while a host is being deleted".to_owned(),
            ));
        }
        let mut guard = self.state.write();
        let mut state = guard.clone();
        let mut imported_ids = Vec::with_capacity(drafts.len());
        for draft in drafts {
            if draft.id.is_some() {
                return Err(AppError::Validation(
                    "imported hosts must not specify destination ids".to_owned(),
                ));
            }
            imported_ids.push(apply_host_draft(&mut state, draft)?.id);
        }
        for host_id in &imported_ids {
            normalize_proxy_jump_for(&mut state.hosts, host_id)?;
        }
        for host_id in &imported_ids {
            validate_proxy_jump_role(&state.hosts, host_id)?;
        }
        let imported = imported_ids
            .iter()
            .map(|host_id| {
                state
                    .hosts
                    .iter()
                    .find(|host| host.id == *host_id)
                    .cloned()
                    .expect("freshly imported host remains present")
            })
            .collect();
        persist_locked(&self.state_path, &state)?;
        *guard = state;
        Ok(imported)
    }

    pub fn begin_host_deletion(&self, host_id: &str) -> AppResult<()> {
        let mut deleting_hosts = self.deleting_hosts.write();
        if deleting_hosts.contains(host_id) {
            return Err(AppError::State(format!(
                "host {host_id} is already being deleted"
            )));
        }
        validate_host_deletion_state(&self.state.read(), host_id)?;
        deleting_hosts.insert(host_id.to_owned());
        Ok(())
    }

    pub fn finish_host_deletion(&self, host_id: &str) -> AppResult<()> {
        let mut deleting_hosts = self.deleting_hosts.write();
        if !deleting_hosts.contains(host_id) {
            return Err(AppError::State(format!(
                "host {host_id} deletion was not prepared"
            )));
        }
        let result = self.delete_host_locked(host_id);
        deleting_hosts.remove(host_id);
        result
    }

    pub fn abort_host_deletion(&self, host_id: &str) {
        self.deleting_hosts.write().remove(host_id);
    }

    pub fn ensure_host_deletable(&self, host_id: &str) -> AppResult<()> {
        let deleting_hosts = self.deleting_hosts.read();
        if deleting_hosts.contains(host_id) {
            return Err(AppError::State(format!(
                "host {host_id} is already being deleted"
            )));
        }
        validate_host_deletion_state(&self.state.read(), host_id)
    }

    #[cfg(test)]
    pub fn delete_host(&self, host_id: &str) -> AppResult<()> {
        self.begin_host_deletion(host_id)?;
        self.finish_host_deletion(host_id)
    }

    fn delete_host_locked(&self, host_id: &str) -> AppResult<()> {
        let mut guard = self.state.write();
        let mut state = guard.clone();
        validate_host_deletion_state(&state, host_id)?;
        let before = state.hosts.len();
        state.hosts.retain(|host| host.id != host_id);
        if before == state.hosts.len() {
            return Err(AppError::NotFound(format!("host {host_id}")));
        }
        state.tunnels.retain(|tunnel| tunnel.host_id != host_id);
        state
            .command_presets
            .retain(|command| command.host_id.as_deref() != Some(host_id));
        persist_locked(&self.state_path, &state)?;
        *guard = state;
        Ok(())
    }

    pub fn tunnel(&self, tunnel_id: &str) -> AppResult<TunnelProfile> {
        self.state
            .read()
            .tunnels
            .iter()
            .find(|tunnel| tunnel.id == tunnel_id)
            .cloned()
            .ok_or_else(|| AppError::NotFound(format!("tunnel {tunnel_id}")))
    }

    pub fn save_tunnel(&self, draft: TunnelDraft) -> AppResult<TunnelProfile> {
        let deleting_hosts = self.deleting_hosts.read();
        if deleting_hosts.contains(&draft.host_id) {
            return Err(AppError::State(
                "cannot save a tunnel for a host being deleted".to_owned(),
            ));
        }
        let mut guard = self.state.write();
        let mut state = guard.clone();
        let profile = apply_tunnel_draft(&mut state, draft)?;
        persist_locked(&self.state_path, &state)?;
        *guard = state;
        Ok(profile)
    }

    pub fn delete_tunnel(&self, tunnel_id: &str) -> AppResult<()> {
        let mut guard = self.state.write();
        let mut state = guard.clone();
        let before = state.tunnels.len();
        state.tunnels.retain(|tunnel| tunnel.id != tunnel_id);
        if before == state.tunnels.len() {
            return Err(AppError::NotFound(format!("tunnel {tunnel_id}")));
        }
        persist_locked(&self.state_path, &state)?;
        *guard = state;
        Ok(())
    }

    pub fn list_commands(&self, host_id: Option<&str>) -> Vec<CommandPreset> {
        let mut commands = self
            .state
            .read()
            .command_presets
            .iter()
            .filter(|command| command.host_id.is_none() || command.host_id.as_deref() == host_id)
            .cloned()
            .collect::<Vec<_>>();
        commands.sort_by(|left, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then_with(|| left.name.cmp(&right.name))
        });
        commands
    }

    pub fn command(&self, command_id: &str) -> AppResult<CommandPreset> {
        self.state
            .read()
            .command_presets
            .iter()
            .find(|command| command.id == command_id)
            .cloned()
            .ok_or_else(|| AppError::NotFound(format!("command preset {command_id}")))
    }

    pub fn save_command(&self, draft: CommandPresetDraft) -> AppResult<CommandPreset> {
        let deleting_hosts = self.deleting_hosts.read();
        if draft
            .host_id
            .as_ref()
            .is_some_and(|host_id| deleting_hosts.contains(host_id))
        {
            return Err(AppError::State(
                "cannot save a command for a host being deleted".to_owned(),
            ));
        }
        let mut guard = self.state.write();
        let mut state = guard.clone();
        let command = apply_command_draft(&mut state, draft)?;
        persist_locked(&self.state_path, &state)?;
        *guard = state;
        Ok(command)
    }

    pub fn delete_command_for_host(&self, command_id: &str, host_id: &str) -> AppResult<()> {
        let deleting_hosts = self.deleting_hosts.read();
        if deleting_hosts.contains(host_id) {
            return Err(AppError::State(
                "cannot change commands while their host is being deleted".to_owned(),
            ));
        }
        let mut guard = self.state.write();
        let mut state = guard.clone();
        if !state.hosts.iter().any(|host| host.id == host_id) {
            return Err(AppError::NotFound(format!("host {host_id}")));
        }
        let command = state
            .command_presets
            .iter()
            .find(|command| command.id == command_id)
            .ok_or_else(|| AppError::NotFound(format!("command preset {command_id}")))?;
        if command
            .host_id
            .as_deref()
            .is_some_and(|owner| owner != host_id)
        {
            return Err(AppError::State(
                "command preset belongs to a different selected host".to_owned(),
            ));
        }
        state
            .command_presets
            .retain(|command| command.id != command_id);
        persist_locked(&self.state_path, &state)?;
        *guard = state;
        Ok(())
    }

    pub fn has_import(&self, source_hash: &str) -> bool {
        self.state
            .read()
            .imported_source_hashes
            .iter()
            .any(|hash| hash.eq_ignore_ascii_case(source_hash))
    }

    pub fn import_batch(&self, batch: ImportBatch) -> AppResult<ImportBatchResult> {
        validate_import_hash(&batch.source_hash)?;
        let deleting_hosts = self.deleting_hosts.read();
        if !deleting_hosts.is_empty() {
            return Err(AppError::State(
                "cannot import while a host is being deleted".to_owned(),
            ));
        }
        let source_hash = batch.source_hash.to_ascii_lowercase();
        let mut guard = self.state.write();
        if guard
            .imported_source_hashes
            .iter()
            .any(|hash| hash.eq_ignore_ascii_case(&source_hash))
        {
            return Ok(ImportBatchResult {
                duplicate: true,
                imported_hosts: 0,
                imported_tunnels: 0,
                imported_commands: 0,
                settings_imported: false,
            });
        }
        if batch.hosts.is_empty()
            && batch.tunnels.is_empty()
            && batch.commands.is_empty()
            && batch.settings.is_none()
        {
            return Err(AppError::Validation(
                "import batch contains no selected records".to_owned(),
            ));
        }

        let mut state = guard.clone();
        let existing_host_ids = state
            .hosts
            .iter()
            .map(|host| host.id.clone())
            .collect::<HashSet<_>>();
        let mut imported_host_ids = HashMap::with_capacity(batch.hosts.len());
        let imported_hosts = batch.hosts.len();
        let imported_tunnels = batch.tunnels.len();
        let imported_commands = batch.commands.len();
        let settings_imported = batch.settings.is_some();

        for planned in batch.hosts {
            validate_import_source_id(&planned.source_id)?;
            if imported_host_ids.contains_key(&planned.source_id) {
                return Err(AppError::Validation(format!(
                    "import source host id '{}' is duplicated",
                    planned.source_id
                )));
            }
            if planned.draft.id.is_some() {
                return Err(AppError::Validation(
                    "imported hosts must not specify destination ids".to_owned(),
                ));
            }
            let profile = apply_host_draft(&mut state, planned.draft)?;
            imported_host_ids.insert(planned.source_id, profile.id);
        }
        let imported_destination_ids = imported_host_ids.values().cloned().collect::<Vec<_>>();
        for host_id in &imported_destination_ids {
            normalize_proxy_jump_for(&mut state.hosts, host_id)?;
        }
        for host_id in &imported_destination_ids {
            validate_proxy_jump_role(&state.hosts, host_id)?;
        }
        for planned in batch.tunnels {
            if planned.draft.id.is_some() {
                return Err(AppError::Validation(
                    "imported tunnels must not specify destination ids".to_owned(),
                ));
            }
            let mut draft = planned.draft;
            draft.host_id =
                resolve_import_host(&planned.host, &imported_host_ids, &existing_host_ids)?;
            apply_tunnel_draft(&mut state, draft)?;
        }
        for planned in batch.commands {
            if planned.draft.id.is_some() {
                return Err(AppError::Validation(
                    "imported command presets must not specify destination ids".to_owned(),
                ));
            }
            let mut draft = planned.draft;
            draft.host_id = planned
                .host
                .as_ref()
                .map(|host| resolve_import_host(host, &imported_host_ids, &existing_host_ids))
                .transpose()?;
            apply_command_draft(&mut state, draft)?;
        }
        if let Some(patch) = batch.settings {
            apply_settings_patch(&mut state.settings, patch)?;
        }
        state.imported_source_hashes.push(source_hash);

        persist_locked(&self.state_path, &state)?;
        *guard = state;
        Ok(ImportBatchResult {
            duplicate: false,
            imported_hosts,
            imported_tunnels,
            imported_commands,
            settings_imported,
        })
    }

    pub fn update_settings(&self, patch: SettingsPatch) -> AppResult<AppSettings> {
        let mut guard = self.state.write();
        let mut state = guard.clone();
        apply_settings_patch(&mut state.settings, patch)?;
        let settings = state.settings.clone();
        persist_locked(&self.state_path, &state)?;
        *guard = state;
        Ok(settings)
    }
}

fn apply_host_update(state: &mut PersistedState, draft: HostDraft) -> AppResult<HostProfile> {
    let previous_alias = draft.id.as_deref().and_then(|id| {
        state
            .hosts
            .iter()
            .find(|host| host.id == id)
            .map(|host| host.alias.clone())
    });
    let profile = apply_host_draft(state, draft)?;
    if let Some(previous_alias) = previous_alias
        && !previous_alias.eq_ignore_ascii_case(&profile.alias)
    {
        for dependent in &mut state.hosts {
            if dependent
                .proxy_jump
                .as_deref()
                .is_some_and(|reference| reference.eq_ignore_ascii_case(&previous_alias))
            {
                dependent.proxy_jump = Some(profile.id.clone());
            }
        }
    }
    normalize_proxy_jump_for(&mut state.hosts, &profile.id)?;
    validate_proxy_jump_role(&state.hosts, &profile.id)?;
    Ok(state
        .hosts
        .iter()
        .find(|host| host.id == profile.id)
        .cloned()
        .expect("freshly applied host remains present"))
}

fn apply_host_draft(state: &mut PersistedState, draft: HostDraft) -> AppResult<HostProfile> {
    validate_host_draft(&draft)?;
    if state.hosts.iter().any(|host| {
        host.alias.eq_ignore_ascii_case(draft.alias.trim())
            && draft.id.as_deref() != Some(host.id.as_str())
    }) {
        return Err(AppError::Validation(format!(
            "host alias '{}' already exists",
            draft.alias.trim()
        )));
    }
    let now = Utc::now();
    let existing = draft
        .id
        .as_deref()
        .and_then(|id| state.hosts.iter().find(|host| host.id == id).cloned());
    if draft.id.is_some() && existing.is_none() {
        return Err(AppError::NotFound(
            "host being edited no longer exists".to_owned(),
        ));
    }
    let advanced = draft.advanced.unwrap_or_default().apply_to(
        existing
            .as_ref()
            .map_or_else(SshAdvancedOptions::default, |host| host.advanced.clone()),
    );
    validate_advanced(&advanced)?;
    let requested_identity_file = normalize_optional(draft.identity_file);
    let auth_method = draft
        .auth_method
        .or_else(|| existing.as_ref().map(|host| host.auth_method))
        .unwrap_or_else(|| {
            if requested_identity_file.is_some() {
                AuthMethod::PrivateKey
            } else {
                AuthMethod::Interactive
            }
        });
    if auth_method == AuthMethod::PrivateKey && requested_identity_file.is_none() {
        return Err(AppError::Validation(
            "private-key authentication requires an identity file".to_owned(),
        ));
    }
    let identity_file = match auth_method {
        AuthMethod::PrivateKey => requested_identity_file,
        AuthMethod::Agent | AuthMethod::Interactive => None,
    };
    let profile = HostProfile {
        schema_version: 2,
        id: existing
            .as_ref()
            .map_or_else(|| Uuid::new_v4().to_string(), |host| host.id.clone()),
        alias: draft.alias.trim().to_owned(),
        hostname: draft.hostname.trim().to_owned(),
        port: draft.port,
        username: draft.username.trim().to_owned(),
        auth_method,
        identity_file,
        proxy_jump: normalize_optional(draft.proxy_jump),
        default_workspace: draft
            .default_workspace
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "~".to_owned()),
        groups: normalize_groups(draft.groups),
        advanced,
        monitor_enabled: draft.monitor_enabled.unwrap_or(true),
        created_at: existing
            .as_ref()
            .map_or_else(|| now.to_owned(), |host| host.created_at.to_owned()),
        updated_at: now,
    };
    if let Some(index) = state.hosts.iter().position(|host| host.id == profile.id) {
        state.hosts[index] = profile.clone();
    } else {
        state.hosts.push(profile.clone());
    }
    Ok(profile)
}

fn validate_host_deletion_state(state: &PersistedState, host_id: &str) -> AppResult<()> {
    let removed = state
        .hosts
        .iter()
        .find(|host| host.id == host_id)
        .ok_or_else(|| AppError::NotFound(format!("host {host_id}")))?;
    if state.hosts.iter().any(|host| {
        host.id != host_id && proxy_reference_matches_host(host.proxy_jump.as_deref(), removed)
    }) {
        return Err(AppError::Validation(format!(
            "host '{}' is used as a ProxyJump; remove those references first",
            removed.alias
        )));
    }
    Ok(())
}

fn normalize_proxy_jump_for(hosts: &mut [HostProfile], host_id: &str) -> AppResult<()> {
    let index = hosts
        .iter()
        .position(|host| host.id == host_id)
        .ok_or_else(|| AppError::NotFound(format!("host {host_id}")))?;
    let Some(reference) = hosts[index].proxy_jump.clone() else {
        return Ok(());
    };
    let jump_id = resolve_proxy_reference(hosts, Some(&reference))?
        .ok_or_else(|| {
            AppError::Validation(format!(
                "ProxyJump '{}' must resolve to a saved host profile",
                reference
            ))
        })?
        .id
        .clone();
    if jump_id == host_id {
        return Err(AppError::Validation(
            "a host cannot use itself as ProxyJump".to_owned(),
        ));
    }
    let jump = hosts
        .iter()
        .find(|host| host.id == jump_id)
        .expect("resolved jump host remains present");
    if jump.proxy_jump.is_some() {
        return Err(AppError::Validation(format!(
            "nested ProxyJump chains are not supported; '{}' must connect directly",
            jump.alias
        )));
    }
    hosts[index].proxy_jump = Some(jump_id);
    Ok(())
}

fn validate_proxy_jump_role(hosts: &[HostProfile], host_id: &str) -> AppResult<()> {
    let host = hosts
        .iter()
        .find(|host| host.id == host_id)
        .ok_or_else(|| AppError::NotFound(format!("host {host_id}")))?;
    let _ = resolve_proxy_jump(hosts, host)?;
    if host.proxy_jump.is_none() {
        return Ok(());
    }
    for dependent in hosts.iter().filter(|candidate| candidate.id != host.id) {
        if proxy_reference_matches_host(dependent.proxy_jump.as_deref(), host) {
            return Err(AppError::Validation(format!(
                "host '{}' is already used as a ProxyJump and must connect directly",
                host.alias
            )));
        }
    }
    Ok(())
}

fn resolve_proxy_jump(hosts: &[HostProfile], host: &HostProfile) -> AppResult<Option<HostProfile>> {
    let Some(jump) = resolve_proxy_reference(hosts, host.proxy_jump.as_deref())? else {
        return Ok(None);
    };
    if jump.id == host.id {
        return Err(AppError::Validation(
            "a host cannot use itself as ProxyJump".to_owned(),
        ));
    }
    if jump.proxy_jump.is_some() {
        return Err(AppError::Validation(format!(
            "nested ProxyJump chains are not supported; '{}' must connect directly",
            jump.alias
        )));
    }
    Ok(Some(jump.clone()))
}

fn resolve_proxy_reference<'a>(
    hosts: &'a [HostProfile],
    reference: Option<&str>,
) -> AppResult<Option<&'a HostProfile>> {
    let Some(reference) = reference.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if let Some(host) = hosts.iter().find(|host| host.id == reference) {
        return Ok(Some(host));
    }
    if let Some(host) = hosts
        .iter()
        .find(|host| host.alias.eq_ignore_ascii_case(reference))
    {
        return Ok(Some(host));
    }
    let matches = hosts
        .iter()
        .filter(|host| {
            proxy_endpoint(host) == reference || proxy_endpoint_with_port(host) == reference
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(AppError::Validation(format!(
            "ProxyJump '{}' must match a saved host alias or exact user@host[:port]",
            reference
        ))),
        [host] => Ok(Some(*host)),
        _ => Err(AppError::Validation(format!(
            "ProxyJump '{}' is ambiguous; use the saved host alias",
            reference
        ))),
    }
}

fn proxy_endpoint(host: &HostProfile) -> String {
    let hostname = if host.hostname.contains(':')
        && !(host.hostname.starts_with('[') && host.hostname.ends_with(']'))
    {
        format!("[{}]", host.hostname)
    } else {
        host.hostname.clone()
    };
    if host.port == 22 {
        format!("{}@{hostname}", host.username)
    } else {
        format!("{}@{hostname}:{}", host.username, host.port)
    }
}

fn proxy_endpoint_with_port(host: &HostProfile) -> String {
    let hostname = if host.hostname.contains(':')
        && !(host.hostname.starts_with('[') && host.hostname.ends_with(']'))
    {
        format!("[{}]", host.hostname)
    } else {
        host.hostname.clone()
    };
    format!("{}@{hostname}:{}", host.username, host.port)
}

fn proxy_reference_matches_host(reference: Option<&str>, host: &HostProfile) -> bool {
    reference.map(str::trim).is_some_and(|reference| {
        reference == host.id
            || reference.eq_ignore_ascii_case(&host.alias)
            || reference == proxy_endpoint(host)
            || reference == proxy_endpoint_with_port(host)
    })
}

fn apply_tunnel_draft(state: &mut PersistedState, draft: TunnelDraft) -> AppResult<TunnelProfile> {
    validate_tunnel_draft(&draft)?;
    if !state.hosts.iter().any(|host| host.id == draft.host_id) {
        return Err(AppError::NotFound(format!("host {}", draft.host_id)));
    }
    if state.tunnels.iter().any(|item| {
        item.host_id == draft.host_id
            && item.name.eq_ignore_ascii_case(draft.name.trim())
            && draft.id.as_deref() != Some(item.id.as_str())
    }) {
        return Err(AppError::Validation(format!(
            "tunnel name '{}' already exists for this host",
            draft.name.trim()
        )));
    }
    let now = Utc::now();
    let existing = draft
        .id
        .as_deref()
        .and_then(|id| state.tunnels.iter().find(|item| item.id == id).cloned());
    if draft.id.is_some() && existing.is_none() {
        return Err(AppError::NotFound(
            "tunnel being edited no longer exists".to_owned(),
        ));
    }
    let profile = TunnelProfile {
        schema_version: 2,
        id: existing
            .as_ref()
            .map_or_else(|| Uuid::new_v4().to_string(), |item| item.id.clone()),
        host_id: draft.host_id,
        name: draft.name.trim().to_owned(),
        direction: draft.direction,
        bind_address: draft.bind_address.trim().to_owned(),
        source_port: draft.source_port,
        target_host: draft.target_host.trim().to_owned(),
        target_port: draft.target_port,
        auto_start: draft.auto_start.unwrap_or(false),
        auto_reconnect: draft.auto_reconnect.unwrap_or(false),
        health_check: draft.health_check,
        created_at: existing
            .as_ref()
            .map_or_else(|| now.to_owned(), |item| item.created_at.to_owned()),
        updated_at: now,
    };
    if let Some(index) = state.tunnels.iter().position(|item| item.id == profile.id) {
        state.tunnels[index] = profile.clone();
    } else {
        state.tunnels.push(profile.clone());
    }
    Ok(profile)
}

fn apply_command_draft(
    state: &mut PersistedState,
    draft: CommandPresetDraft,
) -> AppResult<CommandPreset> {
    validate_command_draft(&draft)?;
    if let Some(host_id) = draft.host_id.as_deref()
        && !state.hosts.iter().any(|host| host.id == host_id)
    {
        return Err(AppError::NotFound(format!("host {host_id}")));
    }
    if state.command_presets.iter().any(|command| {
        command.host_id == draft.host_id
            && command.name.eq_ignore_ascii_case(draft.name.trim())
            && draft.id.as_deref() != Some(command.id.as_str())
    }) {
        return Err(AppError::Validation(format!(
            "command preset '{}' already exists in this scope",
            draft.name.trim()
        )));
    }
    let existing = draft.id.as_deref().and_then(|id| {
        state
            .command_presets
            .iter()
            .find(|command| command.id == id)
            .cloned()
    });
    if draft.id.is_some() && existing.is_none() {
        return Err(AppError::NotFound(
            "command preset being edited no longer exists".to_owned(),
        ));
    }
    let now = Utc::now();
    let command = CommandPreset {
        schema_version: 2,
        id: existing.as_ref().map_or_else(
            || Uuid::new_v4().to_string(),
            |existing| existing.id.clone(),
        ),
        host_id: draft.host_id,
        name: draft.name.trim().to_owned(),
        description: draft.description.trim().to_owned(),
        group: draft.group.trim().to_owned(),
        command: draft.command.trim().to_owned(),
        working_directory: normalize_optional(draft.working_directory),
        risk: draft.risk,
        requires_pty: draft.requires_pty,
        requires_sudo: draft.requires_sudo,
        confirmation_text: normalize_optional(draft.confirmation_text),
        sort_order: draft.sort_order,
        created_at: existing
            .as_ref()
            .map_or_else(|| now.to_owned(), |existing| existing.created_at.to_owned()),
        updated_at: now,
    };
    if let Some(index) = state
        .command_presets
        .iter()
        .position(|existing| existing.id == command.id)
    {
        state.command_presets[index] = command.clone();
    } else {
        state.command_presets.push(command.clone());
    }
    Ok(command)
}

fn apply_settings_patch(settings: &mut AppSettings, patch: SettingsPatch) -> AppResult<()> {
    if let Some(value) = patch.terminal_font_family {
        let value = value.trim();
        if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err(AppError::Validation(
                "terminal font family is invalid".to_owned(),
            ));
        }
        settings.terminal_font_family = value.to_owned();
    }
    if let Some(value) = patch.terminal_font_size {
        if !(9..=32).contains(&value) {
            return Err(AppError::Validation(
                "terminal font size must be between 9 and 32".to_owned(),
            ));
        }
        settings.terminal_font_size = value;
    }
    if let Some(value) = patch.telemetry_interval_seconds {
        if !(1..=60).contains(&value) {
            return Err(AppError::Validation(
                "telemetry interval must be between 1 and 60 seconds".to_owned(),
            ));
        }
        settings.telemetry_interval_seconds = value;
    }
    if let Some(value) = patch.telemetry_retention_minutes {
        if !(1..=1440).contains(&value) {
            return Err(AppError::Validation(
                "telemetry retention must be between 1 and 1440 minutes".to_owned(),
            ));
        }
        settings.telemetry_retention_minutes = value;
    }
    if let Some(value) = patch.download_directory {
        let value = value.trim();
        if value.len() > 32_767 || value.contains('\0') {
            return Err(AppError::Validation(
                "download directory is invalid".to_owned(),
            ));
        }
        settings.download_directory = value.to_owned();
    }
    if let Some(value) = patch.auto_reconnect {
        settings.auto_reconnect = value;
    }
    if let Some(value) = patch.close_to_tray {
        settings.close_to_tray = value;
    }
    if let Some(value) = patch.launch_at_login {
        settings.launch_at_login = value;
    }
    if let Some(value) = patch.btop_watchdog_enabled {
        settings.btop_watchdog_enabled = value;
    }
    if let Some(value) = patch.btop_rotation_minutes {
        if !(1..=1440).contains(&value) {
            return Err(AppError::Validation(
                "btop rotation must be between 1 and 1440 minutes".to_owned(),
            ));
        }
        settings.btop_rotation_minutes = value;
    }
    if let Some(value) = patch.log_level {
        let value = value.trim().to_ascii_lowercase();
        if !matches!(value.as_str(), "debug" | "info" | "warn" | "error") {
            return Err(AppError::Validation("log level is invalid".to_owned()));
        }
        settings.log_level = value;
    }
    if let Some(value) = patch.onboarding_completed {
        settings.onboarding_completed = value;
    }
    if let Some(value) = patch.default_agent {
        let value = value.trim().to_ascii_lowercase();
        if !matches!(value.as_str(), "codex" | "claude" | "gemini" | "opencode") {
            return Err(AppError::Validation("default agent is invalid".to_owned()));
        }
        settings.default_agent = value;
    }
    Ok(())
}

fn resolve_import_host(
    reference: &ImportHostReference,
    imported_host_ids: &HashMap<String, String>,
    existing_host_ids: &HashSet<String>,
) -> AppResult<String> {
    match reference {
        ImportHostReference::Imported(source_id) => {
            imported_host_ids.get(source_id).cloned().ok_or_else(|| {
                AppError::Validation(format!("import references missing source host {source_id}"))
            })
        }
        ImportHostReference::Existing(host_id) if existing_host_ids.contains(host_id) => {
            Ok(host_id.clone())
        }
        ImportHostReference::Existing(host_id) => Err(AppError::Validation(format!(
            "import references missing destination host {host_id}"
        ))),
    }
}

fn validate_import_source_id(source_id: &str) -> AppResult<()> {
    if source_id.is_empty() || source_id.len() > 512 || source_id.chars().any(char::is_control) {
        return Err(AppError::Validation(
            "import source host id is invalid".to_owned(),
        ));
    }
    Ok(())
}

fn validate_import_hash(source_hash: &str) -> AppResult<()> {
    if source_hash.len() != 64
        || !source_hash
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(AppError::Validation("source hash is invalid".to_owned()));
    }
    Ok(())
}

fn validate_host_draft(draft: &HostDraft) -> AppResult<()> {
    let alias = draft.alias.trim();
    if alias.is_empty()
        || alias.len() > 64
        || !alias
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
    {
        return Err(AppError::Validation(
            "alias must use 1-64 ASCII letters, digits, dot, underscore or dash".to_owned(),
        ));
    }
    validate_ssh_atom("hostname", draft.hostname.trim(), 512)?;
    validate_ssh_atom("username", draft.username.trim(), 128)?;
    if draft.port == 0 {
        return Err(AppError::Validation(
            "port must be between 1 and 65535".to_owned(),
        ));
    }
    if let Some(path) = draft.identity_file.as_ref()
        && (path.len() > 32_767 || path.contains('\0'))
    {
        return Err(AppError::Validation(
            "identity file path is invalid".to_owned(),
        ));
    }
    if draft.auth_method == Some(AuthMethod::PrivateKey)
        && draft
            .identity_file
            .as_deref()
            .is_none_or(|path| path.trim().is_empty())
    {
        return Err(AppError::Validation(
            "private-key authentication requires an identity file".to_owned(),
        ));
    }
    if let Some(proxy_jump) = draft.proxy_jump.as_ref() {
        let value = proxy_jump.trim();
        if !value.is_empty() {
            validate_ssh_atom("ProxyJump", value, 1024)?;
        }
    }
    if let Some(path) = draft.default_workspace.as_ref()
        && (path.len() > 4096 || path.contains('\0'))
    {
        return Err(AppError::Validation(
            "default workspace path is invalid".to_owned(),
        ));
    }
    if draft.groups.len() > 32 {
        return Err(AppError::Validation(
            "at most 32 groups are allowed".to_owned(),
        ));
    }
    Ok(())
}

fn validate_advanced(options: &SshAdvancedOptions) -> AppResult<()> {
    if !(1..=300).contains(&options.connect_timeout_seconds) {
        return Err(AppError::Validation(
            "connect timeout must be between 1 and 300 seconds".to_owned(),
        ));
    }
    if options.server_alive_interval_seconds > 3600 {
        return Err(AppError::Validation(
            "server alive interval must be at most 3600 seconds".to_owned(),
        ));
    }
    if !(1..=100).contains(&options.server_alive_count_max) {
        return Err(AppError::Validation(
            "server alive count must be between 1 and 100".to_owned(),
        ));
    }
    Ok(())
}

fn validate_tunnel_draft(draft: &TunnelDraft) -> AppResult<()> {
    let name = draft.name.trim();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(AppError::Validation("tunnel name is invalid".to_owned()));
    }
    if draft.source_port == 0 || draft.target_port == 0 {
        return Err(AppError::Validation(
            "tunnel ports must be between 1 and 65535".to_owned(),
        ));
    }
    validate_forward_atom("bind address", draft.bind_address.trim())?;
    validate_forward_atom("target host", draft.target_host.trim())?;
    if let Some(health) = draft.health_check.as_ref()
        && health.kind == TunnelHealthCheckKind::Tcp
        && (!(2..=3600).contains(&health.interval_seconds)
            || !(1..=60).contains(&health.timeout_seconds)
            || health.timeout_seconds >= health.interval_seconds)
    {
        return Err(AppError::Validation(
            "tunnel health interval/timeout is invalid".to_owned(),
        ));
    }
    Ok(())
}

fn validate_ssh_atom(label: &str, value: &str, max_len: usize) -> AppResult<()> {
    if value.is_empty()
        || value.len() > max_len
        || value.starts_with('-')
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
    {
        return Err(AppError::Validation(format!("{label} is invalid")));
    }
    Ok(())
}

fn validate_forward_atom(label: &str, value: &str) -> AppResult<()> {
    if value.is_empty()
        || value.len() > 512
        || value.starts_with('-')
        || value.contains(',')
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
    {
        return Err(AppError::Validation(format!("{label} is invalid")));
    }
    Ok(())
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_owned())
        .filter(|item| !item.is_empty())
}

fn clear_inactive_identity_files(hosts: &mut [HostProfile]) -> bool {
    let mut changed = false;
    for host in hosts {
        if host.auth_method != AuthMethod::PrivateKey && host.identity_file.take().is_some() {
            changed = true;
        }
    }
    changed
}
fn normalize_groups(groups: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for group in groups {
        let group = group.trim();
        if group.is_empty() || group.len() > 128 || group.chars().any(char::is_control) {
            continue;
        }
        if !normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(group))
        {
            normalized.push(group.to_owned());
        }
    }
    normalized
}
fn persist_locked(path: &Path, state: &PersistedState) -> AppResult<()> {
    write_state(path, state)
}

fn validate_command_draft(draft: &CommandPresetDraft) -> AppResult<()> {
    let name = draft.name.trim();
    if name.is_empty() || name.len() > 512 || name.chars().any(char::is_control) {
        return Err(AppError::Validation(
            "command preset name is invalid".to_owned(),
        ));
    }
    if draft.description.len() > 2048 || draft.description.contains('\0') {
        return Err(AppError::Validation(
            "command preset description is invalid".to_owned(),
        ));
    }
    if draft.group.len() > 128 || draft.group.chars().any(char::is_control) {
        return Err(AppError::Validation(
            "command preset group is invalid".to_owned(),
        ));
    }
    if draft.command.trim().is_empty()
        || draft.command.len() > 32_768
        || draft.command.contains('\0')
    {
        return Err(AppError::Validation(
            "command preset command is invalid".to_owned(),
        ));
    }
    if let Some(path) = draft.working_directory.as_ref()
        && (path.len() > 4096 || path.contains('\0'))
    {
        return Err(AppError::Validation(
            "command working directory is invalid".to_owned(),
        ));
    }
    if let Some(text) = draft.confirmation_text.as_ref()
        && (text.len() > 256 || text.chars().any(char::is_control))
    {
        return Err(AppError::Validation(
            "command confirmation text is invalid".to_owned(),
        ));
    }
    Ok(())
}

fn load_or_create_btop_owner_nonce(directory: &Path) -> AppResult<Arc<str>> {
    let path = directory.join("btop-owner-v2");
    if path.exists() {
        return read_btop_owner_nonce(&path);
    }

    let nonce = Uuid::new_v4().simple().to_string();
    let temporary = directory.join(format!(".btop-owner-{}.tmp", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(nonce.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    let linked = fs::hard_link(&temporary, &path);
    let cleanup = fs::remove_file(&temporary);
    match linked {
        Ok(()) => {
            cleanup?;
            Ok(Arc::from(nonce))
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            cleanup?;
            read_btop_owner_nonce(&path)
        }
        Err(error) => {
            let _ = cleanup;
            Err(AppError::Io(error))
        }
    }
}

fn read_btop_owner_nonce(path: &Path) -> AppResult<Arc<str>> {
    let mut value = String::new();
    File::open(path)?.take(65).read_to_string(&mut value)?;
    let value = value.trim();
    if value.len() != 32 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(AppError::State(
            "the persisted btop ownership proof is invalid".to_owned(),
        ));
    }
    Ok(Arc::from(value.to_ascii_lowercase()))
}

fn read_valid_state(path: &Path) -> AppResult<PersistedState> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(
            u64::try_from(MAX_STATE_BYTES)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err(AppError::State(format!(
            "persisted state exceeds the {MAX_STATE_BYTES}-byte safety limit"
        )));
    }
    let parsed: PersistedState = serde_json::from_slice(&bytes)?;
    validate_persisted_state(&parsed)?;
    Ok(parsed)
}

fn validate_persisted_state(state: &PersistedState) -> AppResult<()> {
    if state.schema_version != 2 {
        return Err(AppError::State(format!(
            "unsupported state schema version {}",
            state.schema_version
        )));
    }
    if state.hosts.len() > MAX_HOSTS
        || state.tunnels.len() > MAX_TUNNELS
        || state.command_presets.len() > MAX_COMMAND_PRESETS
        || state.imported_source_hashes.len() > MAX_IMPORTED_SOURCE_HASHES
    {
        return Err(AppError::State(
            "persisted state exceeds an item-count safety limit".to_owned(),
        ));
    }
    for (index, host) in state.hosts.iter().enumerate() {
        let draft = HostDraft {
            id: Some(host.id.clone()),
            alias: host.alias.clone(),
            hostname: host.hostname.clone(),
            port: host.port,
            username: host.username.clone(),
            auth_method: Some(host.auth_method),
            identity_file: host.identity_file.clone(),
            proxy_jump: host.proxy_jump.clone(),
            default_workspace: Some(host.default_workspace.clone()),
            groups: host.groups.clone(),
            advanced: None,
            monitor_enabled: Some(host.monitor_enabled),
        };
        validate_host_draft(&draft).map_err(|error| {
            AppError::State(format!(
                "persisted host at index {index} is invalid: {error}"
            ))
        })?;
        validate_advanced(&host.advanced).map_err(|error| {
            AppError::State(format!(
                "persisted host at index {index} is invalid: {error}"
            ))
        })?;
        if state.hosts[..index]
            .iter()
            .any(|other| other.id == host.id || other.alias.eq_ignore_ascii_case(&host.alias))
        {
            return Err(AppError::State(format!(
                "persisted host '{}' duplicates an id or alias",
                host.alias
            )));
        }
    }
    for (index, tunnel) in state.tunnels.iter().enumerate() {
        let draft = TunnelDraft {
            id: Some(tunnel.id.clone()),
            host_id: tunnel.host_id.clone(),
            name: tunnel.name.clone(),
            direction: tunnel.direction,
            bind_address: tunnel.bind_address.clone(),
            source_port: tunnel.source_port,
            target_host: tunnel.target_host.clone(),
            target_port: tunnel.target_port,
            auto_start: Some(tunnel.auto_start),
            auto_reconnect: Some(tunnel.auto_reconnect),
            health_check: tunnel.health_check.clone(),
        };
        validate_tunnel_draft(&draft).map_err(|error| {
            AppError::State(format!(
                "persisted tunnel at index {index} is invalid: {error}"
            ))
        })?;
        if !state.hosts.iter().any(|host| host.id == tunnel.host_id) {
            return Err(AppError::State(format!(
                "persisted tunnel '{}' refers to a missing host",
                tunnel.name
            )));
        }
        if state.tunnels[..index].iter().any(|other| {
            other.id == tunnel.id
                || (other.host_id == tunnel.host_id
                    && other.name.eq_ignore_ascii_case(&tunnel.name))
        }) {
            return Err(AppError::State(format!(
                "persisted tunnel '{}' duplicates an id or name",
                tunnel.name
            )));
        }
    }
    for (index, command) in state.command_presets.iter().enumerate() {
        let draft = CommandPresetDraft {
            id: Some(command.id.clone()),
            host_id: command.host_id.clone(),
            name: command.name.clone(),
            description: command.description.clone(),
            group: command.group.clone(),
            command: command.command.clone(),
            working_directory: command.working_directory.clone(),
            risk: command.risk,
            requires_pty: command.requires_pty,
            requires_sudo: command.requires_sudo,
            confirmation_text: command.confirmation_text.clone(),
            sort_order: command.sort_order,
        };
        validate_command_draft(&draft).map_err(|error| {
            AppError::State(format!(
                "persisted command at index {index} is invalid: {error}"
            ))
        })?;
        if let Some(host_id) = command.host_id.as_deref()
            && !state.hosts.iter().any(|host| host.id == host_id)
        {
            return Err(AppError::State(format!(
                "persisted command '{}' refers to a missing host",
                command.name
            )));
        }
        if state.command_presets[..index].iter().any(|other| {
            other.id == command.id
                || (other.host_id == command.host_id
                    && other.name.eq_ignore_ascii_case(&command.name))
        }) {
            return Err(AppError::State(format!(
                "persisted command '{}' duplicates an id or name",
                command.name
            )));
        }
    }
    if state.imported_source_hashes.iter().any(|hash| {
        hash.len() != 64 || !hash.chars().all(|character| character.is_ascii_hexdigit())
    }) {
        return Err(AppError::State(
            "persisted import history contains an invalid hash".to_owned(),
        ));
    }
    Ok(())
}
fn write_state(path: &Path, state: &PersistedState) -> AppResult<()> {
    validate_persisted_state(state)?;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::State("state path has no parent".to_owned()))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".state-{}.tmp", Uuid::new_v4()));
    let mut bytes = serde_json::to_vec_pretty(state)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_STATE_BYTES {
        return Err(AppError::State(format!(
            "persisted state exceeds the {MAX_STATE_BYTES}-byte safety limit"
        )));
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    let backup = path.with_extension("json.bak");
    let preserve_current_as_backup = path.is_file() && read_valid_state(path).is_ok();
    let result = atomic_replace(
        &temporary,
        path,
        preserve_current_as_backup.then_some(backup.as_path()),
    );
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn atomic_replace(temporary: &Path, destination: &Path, backup: Option<&Path>) -> AppResult<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, REPLACEFILE_WRITE_THROUGH,
        ReplaceFileW,
    };

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let source = wide(temporary);
    let target = wide(destination);
    let replaced = if destination.is_file() {
        if let Some(backup) = backup {
            let backup = wide(backup);
            // SAFETY: all pointers reference NUL-terminated UTF-16 buffers that remain alive
            // for the duration of the synchronous Win32 call.
            unsafe {
                ReplaceFileW(
                    target.as_ptr(),
                    source.as_ptr(),
                    backup.as_ptr(),
                    REPLACEFILE_WRITE_THROUGH,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            }
        } else {
            // The existing primary is invalid. Replace it without touching the known-good backup.
            unsafe {
                MoveFileExW(
                    source.as_ptr(),
                    target.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            }
        }
    } else {
        unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) }
    };
    if replaced == 0 {
        Err(AppError::Io(std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace(temporary: &Path, destination: &Path, backup: Option<&Path>) -> AppResult<()> {
    if let Some(backup) = backup {
        let parent = destination
            .parent()
            .ok_or_else(|| AppError::State("state path has no parent".to_owned()))?;
        let backup_temporary = parent.join(format!(".state-backup-{}.tmp", Uuid::new_v4()));
        fs::copy(destination, &backup_temporary)?;
        File::open(&backup_temporary)?.sync_all()?;
        fs::rename(&backup_temporary, backup)?;
    }
    fs::rename(temporary, destination)?;
    if let Some(parent) = destination.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CommandRisk, TunnelDirection};

    fn temporary_directory() -> PathBuf {
        std::env::temp_dir().join(format!("remotedeck-test-{}", Uuid::new_v4()))
    }
    fn draft(alias: &str) -> HostDraft {
        HostDraft {
            id: None,
            alias: alias.to_owned(),
            hostname: "127.0.0.1".to_owned(),
            port: 22,
            username: "tester".to_owned(),
            auth_method: Some(AuthMethod::Interactive),
            identity_file: None,
            proxy_jump: None,
            default_workspace: None,
            groups: vec![" gpu ".to_owned()],
            advanced: None,
            monitor_enabled: None,
        }
    }

    fn tunnel_draft(name: &str) -> TunnelDraft {
        TunnelDraft {
            id: None,
            host_id: "source-placeholder".to_owned(),
            name: name.to_owned(),
            direction: TunnelDirection::Local,
            bind_address: "127.0.0.1".to_owned(),
            source_port: 8888,
            target_host: "127.0.0.1".to_owned(),
            target_port: 8888,
            auto_start: Some(true),
            auto_reconnect: Some(true),
            health_check: None,
        }
    }

    fn command_draft(name: &str) -> CommandPresetDraft {
        CommandPresetDraft {
            id: None,
            host_id: Some("source-placeholder".to_owned()),
            name: name.to_owned(),
            description: String::new(),
            group: "imported".to_owned(),
            command: "uptime".to_owned(),
            working_directory: None,
            risk: CommandRisk::L0,
            requires_pty: false,
            requires_sudo: false,
            confirmation_text: None,
            sort_order: 0,
        }
    }

    fn complete_batch(source_hash: &str) -> ImportBatch {
        let source_id = "legacy-lab".to_owned();
        ImportBatch {
            source_hash: source_hash.to_owned(),
            hosts: vec![ImportBatchHost {
                source_id: source_id.clone(),
                draft: draft("lab"),
            }],
            tunnels: vec![ImportBatchTunnel {
                host: ImportHostReference::Imported(source_id.clone()),
                draft: tunnel_draft("notebook"),
            }],
            commands: vec![ImportBatchCommand {
                host: Some(ImportHostReference::Imported(source_id)),
                draft: command_draft("status"),
            }],
            settings: Some(SettingsPatch {
                terminal_font_size: Some(16),
                ..SettingsPatch::default()
            }),
        }
    }

    #[test]
    fn host_round_trip_persists_without_secrets() {
        let directory = temporary_directory();
        let repository = AppRepository::open(directory.clone()).expect("open");
        let saved = repository.save_host(draft("lab")).expect("save");
        assert_eq!(saved.groups, vec!["gpu"]);
        assert_eq!(
            AppRepository::open(directory.clone())
                .expect("reopen")
                .snapshot()
                .hosts
                .len(),
            1
        );
        assert!(
            !fs::read_to_string(directory.join("state-v2.json"))
                .expect("read")
                .to_ascii_lowercase()
                .contains("password")
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn switching_away_from_private_key_clears_the_hidden_identity() {
        let directory = temporary_directory();
        let repository = AppRepository::open(directory.clone()).expect("open");
        let mut private = draft("lab-key");
        private.auth_method = Some(AuthMethod::PrivateKey);
        private.identity_file = Some(r"C:\Keys\lab key".to_owned());
        let saved = repository
            .save_host(private)
            .expect("save private-key host");

        let mut interactive = draft("lab-key");
        interactive.id = Some(saved.id);
        interactive.auth_method = Some(AuthMethod::Interactive);
        interactive.identity_file = Some(r"C:\Keys\lab key".to_owned());
        let switched = repository
            .save_host(interactive)
            .expect("switch authentication");
        assert_eq!(switched.auth_method, AuthMethod::Interactive);
        assert_eq!(switched.identity_file, None);
        let _ = fs::remove_dir_all(directory);
    }
    #[test]
    fn duplicate_alias_is_rejected_case_insensitively() {
        let directory = temporary_directory();
        let repository = AppRepository::open(directory.clone()).expect("open");
        repository.save_host(draft("lab")).expect("save");
        assert!(repository.save_host(draft("LAB")).is_err());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn proxy_jump_is_resolved_to_a_saved_host_id() {
        let directory = temporary_directory();
        let repository = AppRepository::open(directory.clone()).expect("open");
        let mut jump_draft = draft("jump");
        jump_draft.hostname = "jump.example".to_owned();
        jump_draft.port = 2200;
        let jump = repository.save_host(jump_draft).expect("save jump");

        let mut target_draft = draft("target");
        target_draft.hostname = "target.internal".to_owned();
        target_draft.proxy_jump = Some("tester@jump.example:2200".to_owned());
        let target = repository.save_host(target_draft).expect("save target");

        assert_eq!(target.proxy_jump.as_deref(), Some(jump.id.as_str()));
        assert_eq!(
            repository
                .resolve_jump_host(&target)
                .expect("resolve jump")
                .expect("jump")
                .id,
            jump.id
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn proxy_jump_rejects_missing_self_and_nested_profiles() {
        let directory = temporary_directory();
        let repository = AppRepository::open(directory.clone()).expect("open");
        let mut missing = draft("missing-target");
        missing.proxy_jump = Some("not-saved".to_owned());
        assert!(repository.save_host(missing).is_err());

        let direct = repository.save_host(draft("direct")).expect("direct");
        let mut self_reference = draft("direct");
        self_reference.id = Some(direct.id.clone());
        self_reference.proxy_jump = Some(direct.alias.clone());
        assert!(repository.save_host(self_reference).is_err());

        let jump = repository.save_host(draft("jump")).expect("jump");
        let mut target = draft("target");
        target.proxy_jump = Some(jump.alias.clone());
        repository.save_host(target).expect("target through jump");
        let mut make_jump_nested = draft("jump");
        make_jump_nested.id = Some(jump.id);
        make_jump_nested.proxy_jump = Some(direct.id);
        assert!(repository.save_host(make_jump_nested).is_err());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn referenced_proxy_jump_cannot_be_deleted() {
        let directory = temporary_directory();
        let repository = AppRepository::open(directory.clone()).expect("open");
        let jump = repository.save_host(draft("jump")).expect("jump");
        let mut target = draft("target");
        target.proxy_jump = Some(jump.id.clone());
        repository.save_host(target).expect("target");
        let error = repository
            .delete_host(&jump.id)
            .expect_err("referenced jump is protected");
        assert!(error.to_string().contains("ProxyJump"));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn atomic_host_import_resolves_target_before_jump() {
        let directory = temporary_directory();
        let repository = AppRepository::open(directory.clone()).expect("open");
        let mut target = draft("target");
        target.proxy_jump = Some("jump".to_owned());
        let imported = repository
            .import_hosts(vec![target, draft("jump")])
            .expect("atomic import");
        let target = imported
            .iter()
            .find(|host| host.alias == "target")
            .expect("target");
        let jump = imported
            .iter()
            .find(|host| host.alias == "jump")
            .expect("jump");
        assert_eq!(target.proxy_jump.as_deref(), Some(jump.id.as_str()));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn corrupt_primary_recovers_without_overwriting_valid_backup() {
        let directory = temporary_directory();
        let repository = AppRepository::open(directory.clone()).expect("open");
        repository.save_host(draft("first")).expect("first save");
        repository.save_host(draft("second")).expect("second save");
        let primary = directory.join("state-v2.json");
        let backup = directory.join("state-v2.json.bak");
        let backup_before = fs::read(&backup).expect("valid backup");
        fs::write(&primary, b"{ definitely not json").expect("corrupt primary");

        let recovered = AppRepository::open(directory.clone()).expect("recover from backup");
        assert_eq!(recovered.snapshot().hosts.len(), 1);
        assert_eq!(recovered.snapshot().hosts[0].alias, "first");
        assert_eq!(fs::read(&backup).expect("backup remains"), backup_before);
        assert!(read_valid_state(&primary).is_ok());
        assert!(read_valid_state(&backup).is_ok());
        let _ = fs::remove_dir_all(directory);
    }
    #[test]
    fn option_injection_is_rejected() {
        let mut value = draft("lab");
        value.hostname = "-oProxyCommand=bad".to_owned();
        assert!(validate_host_draft(&value).is_err());
    }

    #[test]
    fn command_delete_is_atomically_bound_to_the_selected_host() {
        let directory = temporary_directory();
        let repository = AppRepository::open(directory.clone()).expect("open");
        let first = repository.save_host(draft("first")).expect("first host");
        let second = repository.save_host(draft("second")).expect("second host");
        let mut scoped = command_draft("scoped");
        scoped.host_id = Some(first.id.clone());
        let saved = repository.save_command(scoped).expect("save command");

        assert!(
            repository
                .delete_command_for_host(&saved.id, &second.id)
                .is_err()
        );
        assert!(repository.command(&saved.id).is_ok());
        repository
            .delete_command_for_host(&saved.id, &first.id)
            .expect("delete from owning host");
        assert!(repository.command(&saved.id).is_err());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn import_batch_is_complete_and_idempotent() {
        let directory = temporary_directory();
        let repository = AppRepository::open(directory.clone()).expect("open");
        let batch = complete_batch(&"a".repeat(64));

        let imported = repository.import_batch(batch.clone()).expect("import");
        assert!(!imported.duplicate);
        assert_eq!(imported.imported_hosts, 1);
        assert_eq!(imported.imported_tunnels, 1);
        assert_eq!(imported.imported_commands, 1);
        assert!(imported.settings_imported);
        let state = repository.snapshot();
        assert_eq!(state.hosts.len(), 1);
        assert_eq!(state.tunnels[0].host_id, state.hosts[0].id);
        assert_eq!(
            state.command_presets[0].host_id.as_deref(),
            Some(state.hosts[0].id.as_str())
        );
        assert_eq!(state.settings.terminal_font_size, 16);

        let duplicate = repository.import_batch(batch).expect("duplicate");
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.imported_hosts, 0);
        assert_eq!(repository.snapshot().hosts.len(), 1);
        assert_eq!(repository.snapshot().imported_source_hashes.len(), 1);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn import_batch_conflict_does_not_apply_earlier_records() {
        let directory = temporary_directory();
        let repository = AppRepository::open(directory.clone()).expect("open");
        repository.save_host(draft("occupied")).expect("seed host");
        let before = serde_json::to_value(repository.snapshot()).expect("serialize before");
        let batch = ImportBatch {
            source_hash: "b".repeat(64),
            hosts: vec![
                ImportBatchHost {
                    source_id: "first".to_owned(),
                    draft: draft("fresh"),
                },
                ImportBatchHost {
                    source_id: "second".to_owned(),
                    draft: draft("OCCUPIED"),
                },
            ],
            tunnels: Vec::new(),
            commands: Vec::new(),
            settings: None,
        };

        let error = repository.import_batch(batch).expect_err("alias conflict");
        assert!(error.to_string().contains("already exists"));
        assert_eq!(
            serde_json::to_value(repository.snapshot()).expect("serialize after"),
            before
        );
        assert!(repository.snapshot().imported_source_hashes.is_empty());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn import_batch_persist_failure_does_not_mutate_memory() {
        let directory = temporary_directory();
        let repository = AppRepository::open(directory.clone()).expect("open");
        let before = serde_json::to_value(repository.snapshot()).expect("serialize before");
        fs::remove_file(&repository.state_path).expect("remove state file");
        fs::create_dir(&repository.state_path).expect("replace state file with directory");

        repository
            .import_batch(complete_batch(&"c".repeat(64)))
            .expect_err("persist failure");
        assert_eq!(
            serde_json::to_value(repository.snapshot()).expect("serialize after"),
            before
        );
        let _ = fs::remove_dir_all(directory);
    }
}
