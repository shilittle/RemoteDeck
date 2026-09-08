use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SshAdvancedOptions {
    pub connect_timeout_seconds: u64,
    pub server_alive_interval_seconds: u64,
    pub server_alive_count_max: u32,
    pub tcp_keep_alive: bool,
    pub compression: bool,
    pub identities_only: bool,
}

impl Default for SshAdvancedOptions {
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshAdvancedPatch {
    pub connect_timeout_seconds: Option<u64>,
    pub server_alive_interval_seconds: Option<u64>,
    pub server_alive_count_max: Option<u32>,
    pub tcp_keep_alive: Option<bool>,
    pub compression: Option<bool>,
    pub identities_only: Option<bool>,
}

impl SshAdvancedPatch {
    pub fn apply_to(self, mut value: SshAdvancedOptions) -> SshAdvancedOptions {
        if let Some(next) = self.connect_timeout_seconds {
            value.connect_timeout_seconds = next;
        }
        if let Some(next) = self.server_alive_interval_seconds {
            value.server_alive_interval_seconds = next;
        }
        if let Some(next) = self.server_alive_count_max {
            value.server_alive_count_max = next;
        }
        if let Some(next) = self.tcp_keep_alive {
            value.tcp_keep_alive = next;
        }
        if let Some(next) = self.compression {
            value.compression = next;
        }
        if let Some(next) = self.identities_only {
            value.identities_only = next;
        }
        value
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    #[default]
    Interactive,
    PrivateKey,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostProfile {
    pub schema_version: u8,
    pub id: String,
    pub alias: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub auth_method: AuthMethod,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    pub default_workspace: String,
    pub groups: Vec<String>,
    pub advanced: SshAdvancedOptions,
    pub monitor_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostDraft {
    pub id: Option<String>,
    pub alias: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    pub auth_method: Option<AuthMethod>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    pub default_workspace: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    pub advanced: Option<SshAdvancedPatch>,
    pub monitor_enabled: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TunnelDirection {
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TunnelHealthCheckKind {
    #[default]
    None,
    Tcp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct TunnelHealthCheck {
    pub kind: TunnelHealthCheckKind,
    pub interval_seconds: u64,
    pub timeout_seconds: u64,
}

impl Default for TunnelHealthCheck {
    fn default() -> Self {
        Self {
            kind: TunnelHealthCheckKind::None,
            interval_seconds: 30,
            timeout_seconds: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TunnelProfile {
    pub schema_version: u8,
    pub id: String,
    pub host_id: String,
    pub name: String,
    pub direction: TunnelDirection,
    pub bind_address: String,
    pub source_port: u16,
    pub target_host: String,
    pub target_port: u16,
    pub auto_start: bool,
    #[serde(default)]
    pub auto_reconnect: bool,
    #[serde(default)]
    pub health_check: Option<TunnelHealthCheck>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelDraft {
    pub id: Option<String>,
    pub host_id: String,
    pub name: String,
    pub direction: TunnelDirection,
    pub bind_address: String,
    pub source_port: u16,
    pub target_host: String,
    pub target_port: u16,
    pub auto_start: Option<bool>,
    pub auto_reconnect: Option<bool>,
    pub health_check: Option<TunnelHealthCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub schema_version: u8,
    pub terminal_font_family: String,
    pub terminal_font_size: u8,
    pub telemetry_interval_seconds: u64,
    pub telemetry_retention_minutes: u64,
    pub download_directory: String,
    pub auto_reconnect: bool,
    pub close_to_tray: bool,
    pub launch_at_login: bool,
    pub btop_watchdog_enabled: bool,
    pub btop_rotation_minutes: u64,
    pub log_level: String,
    pub onboarding_completed: bool,
    pub default_agent: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: 2,
            terminal_font_family: "Cascadia Mono, Consolas, monospace".to_owned(),
            terminal_font_size: 14,
            telemetry_interval_seconds: 3,
            telemetry_retention_minutes: 30,
            download_directory: String::new(),
            auto_reconnect: true,
            close_to_tray: true,
            launch_at_login: false,
            btop_watchdog_enabled: false,
            btop_rotation_minutes: 15,
            log_level: "info".to_owned(),
            onboarding_completed: false,
            default_agent: "codex".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub terminal_font_family: Option<String>,
    pub terminal_font_size: Option<u8>,
    pub telemetry_interval_seconds: Option<u64>,
    pub telemetry_retention_minutes: Option<u64>,
    pub download_directory: Option<String>,
    pub auto_reconnect: Option<bool>,
    pub close_to_tray: Option<bool>,
    pub launch_at_login: Option<bool>,
    pub btop_watchdog_enabled: Option<bool>,
    pub btop_rotation_minutes: Option<u64>,
    pub log_level: Option<String>,
    pub onboarding_completed: Option<bool>,
    pub default_agent: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "UPPERCASE")]
pub enum CommandRisk {
    L0,
    L1,
    L2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandPreset {
    pub schema_version: u8,
    pub id: String,
    pub host_id: Option<String>,
    pub name: String,
    pub description: String,
    pub group: String,
    pub command: String,
    pub working_directory: Option<String>,
    pub risk: CommandRisk,
    pub requires_pty: bool,
    pub requires_sudo: bool,
    pub confirmation_text: Option<String>,
    pub sort_order: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandPresetDraft {
    pub id: Option<String>,
    pub host_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub group: String,
    pub command: String,
    pub working_directory: Option<String>,
    pub risk: CommandRisk,
    #[serde(default)]
    pub requires_pty: bool,
    #[serde(default)]
    pub requires_sudo: bool,
    pub confirmation_text: Option<String>,
    #[serde(default)]
    pub sort_order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedState {
    pub schema_version: u8,
    pub settings: AppSettings,
    pub hosts: Vec<HostProfile>,
    pub tunnels: Vec<TunnelProfile>,
    #[serde(default)]
    pub command_presets: Vec<CommandPreset>,
    #[serde(default)]
    pub imported_source_hashes: Vec<String>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            schema_version: 2,
            settings: AppSettings::default(),
            hosts: Vec::new(),
            tunnels: Vec::new(),
            command_presets: Vec::new(),
            imported_source_hashes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct RuntimeCapabilities {
    pub ssh_path: Option<String>,
    pub keyscan_path: Option<String>,
    pub keygen_path: Option<String>,
    pub pty: bool,
    pub local_forward: bool,
    pub remote_forward: bool,
    pub sftp: bool,
    pub telemetry: bool,
    pub process_signals: bool,
    pub agents: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapPayload {
    pub app_version: String,
    pub settings: AppSettings,
    pub hosts: Vec<HostProfile>,
    pub tunnels: Vec<TunnelProfile>,
    pub capabilities: RuntimeCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyCandidate {
    pub host_token: String,
    pub algorithm: String,
    pub public_key_base64: String,
    pub sha256_fingerprint: String,
    pub raw_line: String,
    #[serde(default)]
    pub trusted: bool,
    #[serde(default)]
    pub mismatch: bool,
    #[serde(default)]
    pub previous_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTestResult {
    pub success: bool,
    pub latency_ms: u128,
    pub server_line: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TerminalState {
    Starting,
    Running,
    Offline,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSnapshot {
    pub session_id: String,
    /// PTY incarnation. A reconnect gets a new generation so delayed terminal
    /// events from the previous process cannot overwrite its snapshot.
    #[serde(default)]
    pub generation: u64,
    /// Monotonic snapshot revision within this terminal resource. Consumers
    /// compare generation first, then revision, when merging list and event
    /// payloads received out of order.
    #[serde(default)]
    pub revision: u64,
    pub host_id: String,
    pub alias: String,
    pub cwd: String,
    pub title: String,
    pub state: TerminalState,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TerminalEventKind {
    Started,
    Output,
    #[serde(rename = "replayTruncated")]
    ReplayTruncated,
    Exit,
    Error,
    State,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalEvent {
    pub session_id: String,
    pub kind: TerminalEventKind,
    pub data: Option<String>,
    pub exit_code: Option<i32>,
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<TerminalSnapshot>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TunnelRuntimeState {
    Stopped,
    Starting,
    Running,
    Waiting,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TunnelHealthState {
    Unknown,
    Healthy,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TunnelLogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TunnelLogEntry {
    pub at: DateTime<Utc>,
    pub level: TunnelLogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TunnelSnapshot {
    pub tunnel_id: String,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<TunnelProfile>,
    pub state: TunnelRuntimeState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<TunnelHealthState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconnect_count: Option<u32>,
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logs: Option<Vec<TunnelLogEntry>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const AT: &str = "2026-08-01T00:00:00Z";

    #[test]
    fn old_state_v2_defaults_new_host_and_tunnel_fields() {
        let old = json!({
            "schemaVersion": 2,
            "settings": {},
            "hosts": [{
                "schemaVersion": 2,
                "id": "11111111-1111-4111-8111-111111111111",
                "alias": "lab",
                "hostname": "lab.example",
                "port": 22,
                "username": "researcher",
                "identityFile": null,
                "proxyJump": null,
                "defaultWorkspace": "~",
                "groups": [],
                "advanced": {
                    "connectTimeoutSeconds": 15,
                    "serverAliveIntervalSeconds": 30,
                    "serverAliveCountMax": 3,
                    "tcpKeepAlive": true,
                    "compression": false,
                    "identitiesOnly": false
                },
                "monitorEnabled": true,
                "createdAt": AT,
                "updatedAt": AT
            }],
            "tunnels": [{
                "schemaVersion": 2,
                "id": "22222222-2222-4222-8222-222222222222",
                "hostId": "11111111-1111-4111-8111-111111111111",
                "name": "notebook",
                "direction": "local",
                "bindAddress": "127.0.0.1",
                "sourcePort": 8888,
                "targetHost": "127.0.0.1",
                "targetPort": 8888,
                "autoStart": true,
                "createdAt": AT,
                "updatedAt": AT
            }]
        });
        let state: PersistedState = serde_json::from_value(old).expect("old state");
        assert_eq!(state.settings.schema_version, 2);
        assert_eq!(state.hosts[0].auth_method, AuthMethod::Interactive);
        assert!(!state.tunnels[0].auto_reconnect);
        assert_eq!(state.tunnels[0].health_check, None);
        assert!(state.command_presets.is_empty());
        assert!(state.imported_source_hashes.is_empty());
    }

    #[test]
    fn persisted_additions_use_frontend_field_and_enum_names() {
        let mut state = PersistedState::default();
        let now = "2026-08-01T00:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("timestamp");
        state.hosts.push(HostProfile {
            schema_version: 2,
            id: "11111111-1111-4111-8111-111111111111".to_owned(),
            alias: "lab".to_owned(),
            hostname: "lab.example".to_owned(),
            port: 22,
            username: "researcher".to_owned(),
            auth_method: AuthMethod::PrivateKey,
            identity_file: Some("C:/Users/test/.ssh/id_ed25519".to_owned()),
            proxy_jump: None,
            default_workspace: "~/work".to_owned(),
            groups: vec!["gpu".to_owned()],
            advanced: SshAdvancedOptions::default(),
            monitor_enabled: true,
            created_at: now,
            updated_at: now,
        });
        state.tunnels.push(TunnelProfile {
            schema_version: 2,
            id: "22222222-2222-4222-8222-222222222222".to_owned(),
            host_id: state.hosts[0].id.clone(),
            name: "notebook".to_owned(),
            direction: TunnelDirection::Local,
            bind_address: "127.0.0.1".to_owned(),
            source_port: 8888,
            target_host: "127.0.0.1".to_owned(),
            target_port: 8888,
            auto_start: true,
            auto_reconnect: true,
            health_check: Some(TunnelHealthCheck {
                kind: TunnelHealthCheckKind::Tcp,
                interval_seconds: 15,
                timeout_seconds: 3,
            }),
            created_at: now,
            updated_at: now,
        });
        let value = serde_json::to_value(&state).expect("serialize");
        assert_eq!(value["hosts"][0]["authMethod"], "private_key");
        assert_eq!(value["tunnels"][0]["autoReconnect"], true);
        assert_eq!(value["tunnels"][0]["healthCheck"]["kind"], "tcp");
        assert_eq!(value["tunnels"][0]["healthCheck"]["intervalSeconds"], 15);
    }

    #[test]
    fn runtime_and_host_key_capabilities_match_typescript_shape() {
        let capabilities = RuntimeCapabilities {
            ssh_path: Some("ssh".to_owned()),
            keyscan_path: Some("ssh-keyscan".to_owned()),
            keygen_path: Some("ssh-keygen".to_owned()),
            pty: true,
            local_forward: true,
            remote_forward: true,
            sftp: true,
            telemetry: true,
            process_signals: true,
            agents: true,
        };
        let value = serde_json::to_value(capabilities).expect("capabilities");
        for key in [
            "sshPath",
            "keyscanPath",
            "keygenPath",
            "pty",
            "localForward",
            "remoteForward",
            "sftp",
            "telemetry",
            "processSignals",
            "agents",
        ] {
            assert!(value.get(key).is_some(), "missing {key}");
        }

        let old_candidate = json!({
            "hostToken": "lab.example",
            "algorithm": "ssh-ed25519",
            "publicKeyBase64": "AAAA",
            "sha256Fingerprint": "SHA256:test",
            "rawLine": "lab.example ssh-ed25519 AAAA"
        });
        let candidate: HostKeyCandidate =
            serde_json::from_value(old_candidate).expect("old candidate");
        assert!(!candidate.trusted);
        assert!(!candidate.mismatch);
        assert_eq!(candidate.previous_fingerprint, None);
    }

    #[test]
    fn terminal_and_tunnel_runtime_payloads_match_frontend_contract() {
        let terminal = TerminalSnapshot {
            session_id: "terminal-1".to_owned(),
            generation: 4,
            revision: 9,
            host_id: "host-1".to_owned(),
            alias: "lab".to_owned(),
            cwd: "~/work".to_owned(),
            title: "lab · ~/work".to_owned(),
            state: TerminalState::Offline,
            exit_code: None,
            error: Some("connection lost".to_owned()),
        };
        let event = TerminalEvent {
            session_id: terminal.session_id.clone(),
            kind: TerminalEventKind::State,
            data: None,
            exit_code: None,
            message: Some("reconnecting".to_owned()),
            snapshot: Some(terminal),
        };
        let terminal_value = serde_json::to_value(event).expect("terminal event");
        assert_eq!(terminal_value["kind"], "state");
        assert_eq!(terminal_value["snapshot"]["state"], "offline");
        assert_eq!(terminal_value["snapshot"]["cwd"], "~/work");
        assert_eq!(terminal_value["snapshot"]["generation"], 4);
        assert_eq!(terminal_value["snapshot"]["revision"], 9);

        let legacy_terminal = serde_json::json!({
            "sessionId": "terminal-legacy",
            "hostId": "host-1",
            "alias": "lab",
            "cwd": "~",
            "title": "lab · ~",
            "state": "offline"
        });
        let legacy_terminal: TerminalSnapshot =
            serde_json::from_value(legacy_terminal).expect("legacy terminal snapshot");
        assert_eq!(legacy_terminal.generation, 0);
        assert_eq!(legacy_terminal.revision, 0);

        let tunnel = TunnelSnapshot {
            tunnel_id: "tunnel-1".to_owned(),
            revision: 3,
            profile: None,
            state: TunnelRuntimeState::Waiting,
            health: Some(TunnelHealthState::Degraded),
            uptime_seconds: Some(120),
            reconnect_count: Some(2),
            message: None,
            logs: Some(vec![TunnelLogEntry {
                at: AT.parse().expect("timestamp"),
                level: TunnelLogLevel::Warn,
                message: "retry scheduled".to_owned(),
            }]),
        };
        let tunnel_value = serde_json::to_value(tunnel).expect("tunnel snapshot");
        assert_eq!(tunnel_value["state"], "waiting");
        assert_eq!(tunnel_value["revision"], 3);
        assert_eq!(tunnel_value["health"], "degraded");
        assert_eq!(tunnel_value["uptimeSeconds"], 120);
        assert_eq!(tunnel_value["reconnectCount"], 2);
        assert_eq!(tunnel_value["logs"][0]["level"], "warn");
        assert_eq!(tunnel_value.get("profile"), None);
        assert_eq!(tunnel_value["message"], Value::Null);
    }
}
