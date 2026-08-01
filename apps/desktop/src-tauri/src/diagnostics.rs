use crate::{error::AppResult, model::PersistedState, ssh::SshRuntime, store::AppRepository};
use chrono::Utc;
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const MAX_ENTRY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsExportResult {
    pub exported: bool,
    pub path: Option<String>,
    pub sha256: Option<String>,
    pub entries: usize,
    pub size_bytes: u64,
}

impl DiagnosticsExportResult {
    pub fn cancelled() -> Self {
        Self {
            exported: false,
            path: None,
            sha256: None,
            entries: 0,
            size_bytes: 0,
        }
    }
}

pub async fn export(
    repository: &AppRepository,
    ssh: &SshRuntime,
    destination: Option<&Path>,
) -> AppResult<DiagnosticsExportResult> {
    let destination = destination
        .filter(|value| !value.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(default_destination);
    validate_destination(&destination)?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = destination.with_file_name(format!(".remotedeck-{}.tmp", Uuid::new_v4()));
    let mut entries = Vec::new();
    let manifest = json!({
        "schemaVersion": 2,
        "appVersion": env!("CARGO_PKG_VERSION"),
        "createdAt": Utc::now(),
        "os": env::consts::OS,
        "arch": env::consts::ARCH,
        "capabilities": ssh.capabilities(),
        "notice": "Passwords, terminal contents, remote command output and credential files are never collected."
    });
    entries.push(("manifest.json", sanitized_json(&manifest)?));
    entries.push((
        "state-redacted.json",
        sanitized_json(&state_summary(&repository.snapshot()))?,
    ));
    let trusted = ssh.list_trusted_keys().await?;
    let trusted = trusted
        .into_iter()
        .map(|record| {
            json!({
                "algorithm": record.algorithm,
                "sha256Fingerprint": record.sha256_fingerprint
            })
        })
        .collect::<Vec<_>>();
    entries.push(("trusted-host-keys.json", sanitized_json(&trusted)?));

    let result = (|| -> AppResult<DiagnosticsExportResult> {
        write_zip(&temporary, &entries)?;
        let bytes = fs::read(&temporary)?;
        let sha256 = hex_digest(&bytes);
        replace_file(&temporary, &destination)?;
        Ok(DiagnosticsExportResult {
            exported: true,
            path: Some(destination.to_string_lossy().into_owned()),
            sha256: Some(sha256),
            entries: entries.len(),
            size_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn state_summary(state: &PersistedState) -> Value {
    let settings = &state.settings;
    json!({
        "schemaVersion": state.schema_version,
        "settings": {
            "terminalFontSize": settings.terminal_font_size,
            "telemetryIntervalSeconds": settings.telemetry_interval_seconds,
            "telemetryRetentionMinutes": settings.telemetry_retention_minutes,
            "downloadDirectoryConfigured": !settings.download_directory.is_empty(),
            "autoReconnect": settings.auto_reconnect,
            "closeToTray": settings.close_to_tray,
            "launchAtLogin": settings.launch_at_login,
            "btopWatchdogEnabled": settings.btop_watchdog_enabled,
            "btopRotationMinutes": settings.btop_rotation_minutes,
            "logLevel": settings.log_level,
            "onboardingCompleted": settings.onboarding_completed,
            "defaultAgent": settings.default_agent,
        },
        "hosts": state.hosts.iter().map(|host| json!({
            "authMethod": host.auth_method,
            "identityFileConfigured": host.identity_file.is_some(),
            "proxyJumpConfigured": host.proxy_jump.is_some(),
            "groupCount": host.groups.len(),
            "monitorEnabled": host.monitor_enabled,
            "advanced": host.advanced,
        })).collect::<Vec<_>>(),
        "tunnels": state.tunnels.iter().map(|tunnel| json!({
            "direction": tunnel.direction,
            "autoStart": tunnel.auto_start,
            "autoReconnect": tunnel.auto_reconnect,
            "healthCheck": tunnel.health_check.as_ref().map(|health| health.kind),
        })).collect::<Vec<_>>(),
        "commands": state.command_presets.iter().map(|command| json!({
            "builtin": command.id.starts_with("builtin:"),
            "hostScoped": command.host_id.is_some(),
            "risk": command.risk,
            "requiresPty": command.requires_pty,
            "requiresSudo": command.requires_sudo,
        })).collect::<Vec<_>>(),
        "importedSourceCount": state.imported_source_hashes.len(),
    })
}

pub fn suggested_filename() -> String {
    format!(
        "RemoteDeck-diagnostics-{}.zip",
        Utc::now().format("%Y%m%d-%H%M%S")
    )
}

fn default_destination() -> PathBuf {
    let base = env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|home| home.join("Downloads"))
        .filter(|path| path.is_dir())
        .unwrap_or_else(env::temp_dir);
    base.join(suggested_filename())
}

fn validate_destination(path: &Path) -> AppResult<()> {
    let text = path.to_string_lossy();
    if text.is_empty() || text.len() > 32_767 || text.contains('\0') {
        return Err(crate::error::AppError::Validation(
            "diagnostics destination is invalid".to_owned(),
        ));
    }
    if path.extension().and_then(|value| value.to_str()) != Some("zip") {
        return Err(crate::error::AppError::Validation(
            "diagnostics destination must end in .zip".to_owned(),
        ));
    }
    Ok(())
}

fn sanitized_json<T: Serialize>(value: &T) -> AppResult<Vec<u8>> {
    let mut value = serde_json::to_value(value)?;
    redact(&mut value);
    let bytes = serde_json::to_vec_pretty(&value)?;
    if bytes.len() > MAX_ENTRY_BYTES {
        return Err(crate::error::AppError::State(
            "diagnostics entry exceeds the 4 MiB safety limit".to_owned(),
        ));
    }
    let lower = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    if contains_secret_marker(&lower) {
        return Err(crate::error::AppError::State(
            "diagnostics secret scan failed closed".to_owned(),
        ));
    }
    Ok(bytes)
}

fn redact(value: &mut Value) {
    match value {
        Value::Object(object) => redact_object(object),
        Value::Array(values) => values.iter_mut().for_each(redact),
        Value::String(text) => *text = redact_string(text),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn redact_object(object: &mut Map<String, Value>) {
    for (key, value) in object {
        let normalized = key.to_ascii_lowercase();
        if [
            "password",
            "passphrase",
            "token",
            "authorization",
            "apikey",
            "api_key",
            "privatekey",
            "private_key",
            "secret",
        ]
        .iter()
        .any(|marker| normalized.contains(marker))
        {
            *value = Value::String("[REDACTED]".to_owned());
        } else {
            redact(value);
        }
    }
}

fn redact_string(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if contains_secret_marker(&lower) {
        return "[REDACTED]".to_owned();
    }
    let mut sanitized = value.to_owned();
    if let Some(home) = env::var_os("USERPROFILE") {
        let home = home.to_string_lossy();
        if !home.is_empty() {
            sanitized = sanitized.replace(home.as_ref(), "%USERPROFILE%");
        }
    }
    sanitized
}

fn contains_secret_marker(value: &str) -> bool {
    value.contains("-----begin ") && value.contains("private key-----")
        || value.contains("bearer ")
        || value.contains("sk-proj-")
        || value.contains("sk-ant-")
        || value.contains("ghp_")
        || value.contains("github_pat_")
        || value.contains("api_key=")
        || value.contains("apikey=")
}

fn write_zip(path: &Path, entries: &[(&str, Vec<u8>)]) -> AppResult<()> {
    let file = File::create(path)?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o600);
    for (name, bytes) in entries {
        archive.start_file(*name, options)?;
        archive.write_all(bytes)?;
    }
    archive.finish()?.sync_all()?;
    Ok(())
}

fn replace_file(temporary: &Path, destination: &Path) -> AppResult<()> {
    replace_file_platform(temporary, destination)
}

#[cfg(windows)]
fn replace_file_platform(temporary: &Path, destination: &Path) -> AppResult<()> {
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
        // SAFETY: both pointers reference live, NUL-terminated UTF-16 buffers.
        unsafe {
            ReplaceFileW(
                target.as_ptr(),
                source.as_ptr(),
                std::ptr::null(),
                REPLACEFILE_WRITE_THROUGH,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        }
    } else {
        // SAFETY: both pointers reference live, NUL-terminated UTF-16 buffers.
        unsafe {
            MoveFileExW(
                source.as_ptr(),
                target.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    if replaced == 0 {
        Err(crate::error::AppError::Io(std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file_platform(temporary: &Path, destination: &Path) -> AppResult<()> {
    fs::rename(temporary, destination)?;
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn recursive_redaction_removes_keys_and_value_markers() {
        let mut value = json!({
            "nested": {"password": "hello", "safe": "Bearer abc"},
            "items": ["-----BEGIN OPENSSH PRIVATE KEY-----", "ok"]
        });
        redact(&mut value);
        let text = serde_json::to_string(&value).expect("serialize");
        assert!(!text.contains("hello"));
        assert!(!text.contains("Bearer"));
        assert!(!text.contains("PRIVATE KEY"));
        assert!(text.contains("ok"));
    }

    #[test]
    fn state_summary_excludes_user_paths_and_command_text() {
        let mut state = PersistedState::default();
        state.settings.download_directory = r"C:\Users\person\TOP_SECRET_DIR".to_owned();
        state.command_presets.push(crate::model::CommandPreset {
            schema_version: 2,
            id: Uuid::new_v4().to_string(),
            host_id: None,
            name: "TOP_SECRET_NAME".to_owned(),
            description: "TOP_SECRET_DESCRIPTION".to_owned(),
            group: "TOP_SECRET_GROUP".to_owned(),
            command: "export TOKEN=TOP_SECRET_COMMAND".to_owned(),
            working_directory: Some("/srv/TOP_SECRET_WORKSPACE".to_owned()),
            risk: crate::model::CommandRisk::L2,
            requires_pty: true,
            requires_sudo: true,
            confirmation_text: Some("TOP_SECRET_CONFIRMATION".to_owned()),
            sort_order: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });
        let text = serde_json::to_string(&state_summary(&state)).expect("serialize summary");
        assert!(!text.contains("TOP_SECRET"));
        assert!(text.contains("downloadDirectoryConfigured"));
        assert!(text.contains("requiresSudo"));
    }

    #[test]
    fn zip_contains_expected_entries() {
        let path = env::temp_dir().join(format!("remotedeck-diag-{}.zip", Uuid::new_v4()));
        write_zip(
            &path,
            &[("one.json", b"{}".to_vec()), ("two.json", b"[]".to_vec())],
        )
        .expect("write");
        let mut archive = zip::ZipArchive::new(File::open(&path).expect("open")).expect("zip");
        assert_eq!(archive.len(), 2);
        let mut text = String::new();
        archive
            .by_name("one.json")
            .expect("entry")
            .read_to_string(&mut text)
            .expect("read");
        assert_eq!(text, "{}");
        let _ = fs::remove_file(path);
    }
}
