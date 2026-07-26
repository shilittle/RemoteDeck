use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostProfile {
    pub schema_version: u8,
    pub id: String,
    pub alias: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
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
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    pub default_workspace: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    pub advanced: Option<SshAdvancedPatch>,
    pub monitor_enabled: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelDirection {
    Local,
    Remote,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub schema_version: u8,
    pub terminal_font_family: String,
    pub terminal_font_size: u8,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: 2,
            terminal_font_family: "Cascadia Mono, Consolas, monospace".to_owned(),
            terminal_font_size: 14,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub terminal_font_family: Option<String>,
    pub terminal_font_size: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedState {
    pub schema_version: u8,
    pub settings: AppSettings,
    pub hosts: Vec<HostProfile>,
    pub tunnels: Vec<TunnelProfile>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            schema_version: 2,
            settings: AppSettings::default(),
            hosts: Vec::new(),
            tunnels: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCapabilities {
    pub ssh_path: Option<String>,
    pub keyscan_path: Option<String>,
    pub keygen_path: Option<String>,
    pub pty: bool,
    pub local_forward: bool,
    pub remote_forward: bool,
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

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalState {
    Running,
    Closed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSnapshot {
    pub session_id: String,
    pub host_id: String,
    pub alias: String,
    pub state: TerminalState,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalEventKind {
    Started,
    Output,
    Exit,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalEvent {
    pub session_id: String,
    pub kind: TerminalEventKind,
    pub data: Option<String>,
    pub exit_code: Option<i32>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelRuntimeState {
    Stopped,
    Starting,
    Running,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelSnapshot {
    pub tunnel_id: String,
    pub state: TunnelRuntimeState,
    pub message: Option<String>,
}
