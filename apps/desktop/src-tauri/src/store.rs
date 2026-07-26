use std::{fs::{self, File, OpenOptions}, io::Write, path::{Path, PathBuf}, sync::Arc};
use chrono::Utc;
use parking_lot::RwLock;
use uuid::Uuid;
use crate::{
    error::{AppError, AppResult},
    model::{AppSettings, HostDraft, HostProfile, PersistedState, SettingsPatch, SshAdvancedOptions, TunnelDraft, TunnelProfile},
};

#[derive(Clone)]
pub struct AppRepository {
    directory: PathBuf,
    state_path: PathBuf,
    known_hosts_path: PathBuf,
    state: Arc<RwLock<PersistedState>>,
}

impl AppRepository {
    pub fn open(directory: PathBuf) -> AppResult<Self> {
        fs::create_dir_all(&directory)?;
        let state_path = directory.join("state-v2.json");
        let known_hosts_path = directory.join("known_hosts");
        if !known_hosts_path.exists() { File::create(&known_hosts_path)?.sync_all()?; }
        let state = if state_path.exists() {
            let parsed: PersistedState = serde_json::from_slice(&fs::read(&state_path)?)?;
            if parsed.schema_version != 2 {
                return Err(AppError::State(format!("unsupported state schema version {}", parsed.schema_version)));
            }
            parsed
        } else {
            let initial = PersistedState::default();
            write_state(&state_path, &initial)?;
            initial
        };
        Ok(Self { directory, state_path, known_hosts_path, state: Arc::new(RwLock::new(state)) })
    }

    pub fn app_data_directory(&self) -> &Path { &self.directory }
    pub fn known_hosts_path(&self) -> &Path { &self.known_hosts_path }
    pub fn snapshot(&self) -> PersistedState { self.state.read().clone() }

    pub fn host(&self, host_id: &str) -> AppResult<HostProfile> {
        self.state.read().hosts.iter().find(|host| host.id == host_id).cloned()
            .ok_or_else(|| AppError::NotFound(format!("host {host_id}")))
    }

    pub fn save_host(&self, draft: HostDraft) -> AppResult<HostProfile> {
        validate_host_draft(&draft)?;
        let mut state = self.state.write();
        if state.hosts.iter().any(|host| host.alias.eq_ignore_ascii_case(draft.alias.trim()) && draft.id.as_deref() != Some(host.id.as_str())) {
            return Err(AppError::Validation(format!("host alias '{}' already exists", draft.alias.trim())));
        }
        let now = Utc::now();
        let existing = draft.id.as_deref().and_then(|id| state.hosts.iter().find(|host| host.id == id).cloned());
        if draft.id.is_some() && existing.is_none() { return Err(AppError::NotFound("host being edited no longer exists".to_owned())); }
        let advanced = draft.advanced.unwrap_or_default().apply_to(existing.as_ref().map_or_else(SshAdvancedOptions::default, |host| host.advanced.clone()));
        validate_advanced(&advanced)?;
        let profile = HostProfile {
            schema_version: 2,
            id: existing.as_ref().map_or_else(|| Uuid::new_v4().to_string(), |host| host.id.clone()),
            alias: draft.alias.trim().to_owned(),
            hostname: draft.hostname.trim().to_owned(),
            port: draft.port,
            username: draft.username.trim().to_owned(),
            identity_file: normalize_optional(draft.identity_file),
            proxy_jump: normalize_optional(draft.proxy_jump),
            default_workspace: draft.default_workspace.map(|value| value.trim().to_owned()).filter(|value| !value.is_empty()).unwrap_or_else(|| "~".to_owned()),
            groups: normalize_groups(draft.groups),
            advanced,
            monitor_enabled: draft.monitor_enabled.unwrap_or(true),
            created_at: existing.as_ref().map_or_else(|| now.to_owned(), |host| host.created_at.to_owned()),
            updated_at: now,
        };
        if let Some(index) = state.hosts.iter().position(|host| host.id == profile.id) { state.hosts[index] = profile.clone(); } else { state.hosts.push(profile.clone()); }
        persist_locked(&self.state_path, &state)?;
        Ok(profile)
    }

    pub fn delete_host(&self, host_id: &str) -> AppResult<()> {
        let mut state = self.state.write();
        let before = state.hosts.len();
        state.hosts.retain(|host| host.id != host_id);
        if before == state.hosts.len() { return Err(AppError::NotFound(format!("host {host_id}"))); }
        state.tunnels.retain(|tunnel| tunnel.host_id != host_id);
        persist_locked(&self.state_path, &state)
    }

    pub fn tunnel(&self, tunnel_id: &str) -> AppResult<TunnelProfile> {
        self.state.read().tunnels.iter().find(|tunnel| tunnel.id == tunnel_id).cloned()
            .ok_or_else(|| AppError::NotFound(format!("tunnel {tunnel_id}")))
    }

    pub fn save_tunnel(&self, draft: TunnelDraft) -> AppResult<TunnelProfile> {
        validate_tunnel_draft(&draft)?;
        let mut state = self.state.write();
        if !state.hosts.iter().any(|host| host.id == draft.host_id) { return Err(AppError::NotFound(format!("host {}", draft.host_id))); }
        if state.tunnels.iter().any(|item| item.host_id == draft.host_id && item.name.eq_ignore_ascii_case(draft.name.trim()) && draft.id.as_deref() != Some(item.id.as_str())) {
            return Err(AppError::Validation(format!("tunnel name '{}' already exists for this host", draft.name.trim())));
        }
        let now = Utc::now();
        let existing = draft.id.as_deref().and_then(|id| state.tunnels.iter().find(|item| item.id == id).cloned());
        if draft.id.is_some() && existing.is_none() { return Err(AppError::NotFound("tunnel being edited no longer exists".to_owned())); }
        let profile = TunnelProfile {
            schema_version: 2,
            id: existing.as_ref().map_or_else(|| Uuid::new_v4().to_string(), |item| item.id.clone()),
            host_id: draft.host_id,
            name: draft.name.trim().to_owned(),
            direction: draft.direction,
            bind_address: draft.bind_address.trim().to_owned(),
            source_port: draft.source_port,
            target_host: draft.target_host.trim().to_owned(),
            target_port: draft.target_port,
            auto_start: draft.auto_start.unwrap_or(false),
            created_at: existing.as_ref().map_or_else(|| now.to_owned(), |item| item.created_at.to_owned()),
            updated_at: now,
        };
        if let Some(index) = state.tunnels.iter().position(|item| item.id == profile.id) { state.tunnels[index] = profile.clone(); } else { state.tunnels.push(profile.clone()); }
        persist_locked(&self.state_path, &state)?;
        Ok(profile)
    }

    pub fn delete_tunnel(&self, tunnel_id: &str) -> AppResult<()> {
        let mut state = self.state.write();
        let before = state.tunnels.len();
        state.tunnels.retain(|tunnel| tunnel.id != tunnel_id);
        if before == state.tunnels.len() { return Err(AppError::NotFound(format!("tunnel {tunnel_id}"))); }
        persist_locked(&self.state_path, &state)
    }

    pub fn update_settings(&self, patch: SettingsPatch) -> AppResult<AppSettings> {
        let mut state = self.state.write();
        if let Some(value) = patch.terminal_font_family {
            let value = value.trim();
            if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) { return Err(AppError::Validation("terminal font family is invalid".to_owned())); }
            state.settings.terminal_font_family = value.to_owned();
        }
        if let Some(value) = patch.terminal_font_size {
            if !(9..=32).contains(&value) { return Err(AppError::Validation("terminal font size must be between 9 and 32".to_owned())); }
            state.settings.terminal_font_size = value;
        }
        let settings = state.settings.clone();
        persist_locked(&self.state_path, &state)?;
        Ok(settings)
    }
}

fn validate_host_draft(draft: &HostDraft) -> AppResult<()> {
    let alias = draft.alias.trim();
    if alias.is_empty() || alias.len() > 64 || !alias.chars().all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character)) {
        return Err(AppError::Validation("alias must use 1-64 ASCII letters, digits, dot, underscore or dash".to_owned()));
    }
    validate_ssh_atom("hostname", draft.hostname.trim(), 512)?;
    validate_ssh_atom("username", draft.username.trim(), 128)?;
    if draft.port == 0 { return Err(AppError::Validation("port must be between 1 and 65535".to_owned())); }
    if let Some(path) = draft.identity_file.as_ref() {
        if path.len() > 32_767 || path.contains('\0') { return Err(AppError::Validation("identity file path is invalid".to_owned())); }
    }
    if let Some(proxy_jump) = draft.proxy_jump.as_ref() {
        let value = proxy_jump.trim();
        if !value.is_empty() { validate_ssh_atom("ProxyJump", value, 1024)?; }
    }
    if let Some(path) = draft.default_workspace.as_ref() {
        if path.len() > 4096 || path.contains('\0') { return Err(AppError::Validation("default workspace path is invalid".to_owned())); }
    }
    if draft.groups.len() > 32 { return Err(AppError::Validation("at most 32 groups are allowed".to_owned())); }
    Ok(())
}

fn validate_advanced(options: &SshAdvancedOptions) -> AppResult<()> {
    if !(1..=300).contains(&options.connect_timeout_seconds) { return Err(AppError::Validation("connect timeout must be between 1 and 300 seconds".to_owned())); }
    if options.server_alive_interval_seconds > 3600 { return Err(AppError::Validation("server alive interval must be at most 3600 seconds".to_owned())); }
    if !(1..=100).contains(&options.server_alive_count_max) { return Err(AppError::Validation("server alive count must be between 1 and 100".to_owned())); }
    Ok(())
}

fn validate_tunnel_draft(draft: &TunnelDraft) -> AppResult<()> {
    let name = draft.name.trim();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) { return Err(AppError::Validation("tunnel name is invalid".to_owned())); }
    if draft.source_port == 0 || draft.target_port == 0 { return Err(AppError::Validation("tunnel ports must be between 1 and 65535".to_owned())); }
    validate_forward_atom("bind address", draft.bind_address.trim())?;
    validate_forward_atom("target host", draft.target_host.trim())?;
    Ok(())
}

fn validate_ssh_atom(label: &str, value: &str, max_len: usize) -> AppResult<()> {
    if value.is_empty() || value.len() > max_len || value.starts_with('-') || value.chars().any(char::is_whitespace) || value.chars().any(char::is_control) {
        return Err(AppError::Validation(format!("{label} is invalid")));
    }
    Ok(())
}

fn validate_forward_atom(label: &str, value: &str) -> AppResult<()> {
    if value.is_empty() || value.len() > 512 || value.starts_with('-') || value.contains(',') || value.chars().any(char::is_whitespace) || value.chars().any(char::is_control) {
        return Err(AppError::Validation(format!("{label} is invalid")));
    }
    Ok(())
}

fn normalize_optional(value: Option<String>) -> Option<String> { value.map(|item| item.trim().to_owned()).filter(|item| !item.is_empty()) }
fn normalize_groups(groups: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for group in groups {
        let group = group.trim();
        if group.is_empty() || group.len() > 128 || group.chars().any(char::is_control) { continue; }
        if !normalized.iter().any(|existing: &String| existing.eq_ignore_ascii_case(group)) { normalized.push(group.to_owned()); }
    }
    normalized
}
fn persist_locked(path: &Path, state: &PersistedState) -> AppResult<()> { write_state(path, state) }
fn write_state(path: &Path, state: &PersistedState) -> AppResult<()> {
    let parent = path.parent().ok_or_else(|| AppError::State("state path has no parent".to_owned()))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".state-{}.tmp", Uuid::new_v4()));
    let mut file = OpenOptions::new().create_new(true).write(true).open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(state)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    if path.exists() { let _ = fs::copy(path, path.with_extension("json.bak")); }
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_) if path.exists() => { fs::remove_file(path)?; fs::rename(&temporary, path).map_err(AppError::from) }
        Err(error) => Err(AppError::Io(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temporary_directory() -> PathBuf { std::env::temp_dir().join(format!("remotedeck-test-{}", Uuid::new_v4())) }
    fn draft(alias: &str) -> HostDraft { HostDraft { id: None, alias: alias.to_owned(), hostname: "127.0.0.1".to_owned(), port: 22, username: "tester".to_owned(), identity_file: None, proxy_jump: None, default_workspace: None, groups: vec![" gpu ".to_owned()], advanced: None, monitor_enabled: None } }
    #[test]
    fn host_round_trip_persists_without_secrets() {
        let directory = temporary_directory();
        let repository = AppRepository::open(directory.clone()).expect("open");
        let saved = repository.save_host(draft("lab")).expect("save");
        assert_eq!(saved.groups, vec!["gpu"]);
        assert_eq!(AppRepository::open(directory.clone()).expect("reopen").snapshot().hosts.len(), 1);
        assert!(!fs::read_to_string(directory.join("state-v2.json")).expect("read").to_ascii_lowercase().contains("password"));
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
    fn option_injection_is_rejected() {
        let mut value = draft("lab"); value.hostname = "-oProxyCommand=bad".to_owned();
        assert!(validate_host_draft(&value).is_err());
    }
}
