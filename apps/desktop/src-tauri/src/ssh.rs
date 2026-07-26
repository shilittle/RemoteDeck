use crate::{
    error::{AppError, AppResult},
    model::{
        CommandResult, ConnectionTestResult, HostKeyCandidate, HostProfile, RuntimeCapabilities,
        TunnelDirection, TunnelProfile,
    },
};
use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Output,
    time::{Duration, Instant},
};
use tokio::{process::Command, time::timeout};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SshRuntime {
    ssh_path: Option<PathBuf>,
    keyscan_path: Option<PathBuf>,
    keygen_path: Option<PathBuf>,
    known_hosts_path: PathBuf,
}

impl SshRuntime {
    pub fn discover(known_hosts_path: PathBuf) -> Self {
        Self {
            ssh_path: find_executable(&["ssh.exe", "ssh"]),
            keyscan_path: find_executable(&["ssh-keyscan.exe", "ssh-keyscan"]),
            keygen_path: find_executable(&["ssh-keygen.exe", "ssh-keygen"]),
            known_hosts_path,
        }
    }

    pub fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities {
            ssh_path: path_text(self.ssh_path.as_deref()),
            keyscan_path: path_text(self.keyscan_path.as_deref()),
            keygen_path: path_text(self.keygen_path.as_deref()),
            pty: self.ssh_path.is_some(),
            local_forward: self.ssh_path.is_some(),
            remote_forward: self.ssh_path.is_some(),
        }
    }

    pub async fn scan_host_keys(&self, host: &HostProfile) -> AppResult<Vec<HostKeyCandidate>> {
        let program = self.keyscan()?;
        let seconds = host.advanced.connect_timeout_seconds.clamp(1, 300);
        let args = vec![
            OsString::from("-p"),
            OsString::from(host.port.to_string()),
            OsString::from("-T"),
            OsString::from(seconds.to_string()),
            OsString::from(&host.hostname),
        ];
        let output = run_output(program, &args, Duration::from_secs(seconds + 3)).await?;
        let expected_token = known_hosts_host_token(&host.hostname, host.port);
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 3 {
                continue;
            }
            let algorithm = fields[1];
            let key = fields[2];
            validate_host_key_fields(algorithm, key)?;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(key)
                .map_err(|error| {
                    AppError::Process(format!("ssh-keyscan returned invalid base64: {error}"))
                })?;
            let fingerprint = format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(decoded)));
            if seen.insert(format!("{algorithm}:{key}")) {
                candidates.push(HostKeyCandidate {
                    host_token: expected_token.clone(),
                    algorithm: algorithm.to_owned(),
                    public_key_base64: key.to_owned(),
                    sha256_fingerprint: fingerprint,
                    raw_line: format!("{expected_token} {algorithm} {key}"),
                });
            }
        }
        candidates.sort_by(|left, right| left.algorithm.cmp(&right.algorithm));
        if candidates.is_empty() && !output.status.success() {
            return Err(AppError::Process(nonempty_or(
                lossy_limited(&output.stderr),
                "ssh-keyscan did not return a host key",
            )));
        }
        Ok(candidates)
    }

    pub async fn accept_host_key(
        &self,
        host: &HostProfile,
        candidate: &HostKeyCandidate,
    ) -> AppResult<()> {
        validate_host_key_fields(&candidate.algorithm, &candidate.public_key_base64)?;
        let token = known_hosts_host_token(&host.hostname, host.port);
        if candidate.host_token != token {
            return Err(AppError::Validation(
                "host-key candidate does not belong to the selected host".to_owned(),
            ));
        }
        let current = self.scan_host_keys(host).await?;
        if !current.iter().any(|item| {
            item.algorithm == candidate.algorithm
                && item.public_key_base64 == candidate.public_key_base64
                && item.sha256_fingerprint == candidate.sha256_fingerprint
        }) {
            return Err(AppError::Process("host key changed between scan and acceptance; scan again and verify the new fingerprint".to_owned()));
        }
        let existing = fs::read_to_string(&self.known_hosts_path).unwrap_or_default();
        let mut lines = existing
            .lines()
            .filter(|line| !line_matches_host_token(line, &token))
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        lines.push(format!(
            "{} {} {}",
            token, candidate.algorithm, candidate.public_key_base64
        ));
        let mut content = lines.join("\n");
        content.push('\n');
        atomic_write_text(&self.known_hosts_path, &content)
    }

    pub async fn test_connection(&self, host: &HostProfile) -> AppResult<ConnectionTestResult> {
        let mut args = self.base_args(host, true)?;
        args.push(OsString::from(destination(host)));
        args.push(OsString::from(
            "printf '__REMOTEDECK_OK__\\n'; uname -srm 2>/dev/null || true",
        ));
        let start = Instant::now();
        let output = run_output(
            self.ssh()?,
            &args,
            Duration::from_secs(host.advanced.connect_timeout_seconds + 5),
        )
        .await?;
        let stdout = lossy_limited(&output.stdout);
        let stderr = lossy_limited(&output.stderr);
        let success =
            output.status.success() && stdout.lines().any(|line| line == "__REMOTEDECK_OK__");
        Ok(ConnectionTestResult {
            success,
            latency_ms: start.elapsed().as_millis(),
            server_line: stdout
                .lines()
                .find(|line| !line.trim().is_empty() && *line != "__REMOTEDECK_OK__")
                .map(ToOwned::to_owned),
            error: if success {
                None
            } else {
                Some(nonempty_or(stderr, "OpenSSH connection test failed"))
            },
        })
    }

    pub async fn run_command(
        &self,
        host: &HostProfile,
        command: String,
        working_directory: Option<String>,
    ) -> AppResult<CommandResult> {
        validate_remote_command(&command)?;
        let remote = match working_directory {
            Some(directory) if !directory.trim().is_empty() => {
                validate_remote_path(&directory)?;
                format!("cd -- {} && {command}", posix_quote(directory.trim()))
            }
            _ => command,
        };
        let mut args = self.base_args(host, true)?;
        args.push(OsString::from(destination(host)));
        args.push(OsString::from(remote));
        let start = Instant::now();
        let output = run_output(self.ssh()?, &args, Duration::from_secs(3600)).await?;
        Ok(CommandResult {
            exit_code: output.status.code(),
            stdout: lossy_limited(&output.stdout),
            stderr: lossy_limited(&output.stderr),
            duration_ms: start.elapsed().as_millis(),
        })
    }

    pub fn terminal_command(&self, host: &HostProfile) -> AppResult<(PathBuf, Vec<OsString>)> {
        let program = self.ssh()?.to_path_buf();
        let mut args = self.base_args(host, false)?;
        args.push(OsString::from("-tt"));
        args.push(OsString::from(destination(host)));
        let workspace = host.default_workspace.trim();
        if !workspace.is_empty() && workspace != "~" {
            validate_remote_path(workspace)?;
            args.push(OsString::from(format!(
                "cd -- {} && exec \"${{SHELL:-/bin/sh}}\" -l",
                posix_quote(workspace)
            )));
        }
        Ok((program, args))
    }

    pub fn tunnel_command(
        &self,
        host: &HostProfile,
        tunnel: &TunnelProfile,
    ) -> AppResult<(PathBuf, Vec<OsString>)> {
        let program = self.ssh()?.to_path_buf();
        let mut args = self.base_args(host, true)?;
        args.push(OsString::from("-N"));
        push_option(&mut args, "ExitOnForwardFailure=yes".to_owned());
        args.push(OsString::from(match tunnel.direction {
            TunnelDirection::Local => "-L",
            TunnelDirection::Remote => "-R",
        }));
        args.push(OsString::from(format!(
            "{}:{}:{}:{}",
            tunnel.bind_address, tunnel.source_port, tunnel.target_host, tunnel.target_port
        )));
        args.push(OsString::from(destination(host)));
        Ok((program, args))
    }

    fn base_args(&self, host: &HostProfile, batch_mode: bool) -> AppResult<Vec<OsString>> {
        if !self.known_hosts_path.exists() {
            return Err(AppError::State(
                "known_hosts file is unavailable".to_owned(),
            ));
        }
        let mut args = vec![OsString::from("-p"), OsString::from(host.port.to_string())];
        push_option(
            &mut args,
            format!("BatchMode={}", if batch_mode { "yes" } else { "no" }),
        );
        push_option(&mut args, "StrictHostKeyChecking=yes".to_owned());
        push_option(
            &mut args,
            format!("UserKnownHostsFile={}", self.known_hosts_path.display()),
        );
        push_option(&mut args, format!("GlobalKnownHostsFile={}", null_device()));
        push_option(
            &mut args,
            format!("ConnectTimeout={}", host.advanced.connect_timeout_seconds),
        );
        push_option(
            &mut args,
            format!(
                "ServerAliveInterval={}",
                host.advanced.server_alive_interval_seconds
            ),
        );
        push_option(
            &mut args,
            format!(
                "ServerAliveCountMax={}",
                host.advanced.server_alive_count_max
            ),
        );
        push_option(
            &mut args,
            format!("TCPKeepAlive={}", yes_no(host.advanced.tcp_keep_alive)),
        );
        push_option(
            &mut args,
            format!("Compression={}", yes_no(host.advanced.compression)),
        );
        if let Some(identity) = host.identity_file.as_ref() {
            args.push(OsString::from("-i"));
            args.push(OsString::from(identity));
            if host.advanced.identities_only {
                push_option(&mut args, "IdentitiesOnly=yes".to_owned());
            }
        }
        if let Some(jump) = host.proxy_jump.as_ref() {
            args.push(OsString::from("-J"));
            args.push(OsString::from(jump));
        }
        Ok(args)
    }

    fn ssh(&self) -> AppResult<&Path> {
        self.ssh_path
            .as_deref()
            .ok_or_else(|| AppError::MissingExecutable("OpenSSH ssh.exe".to_owned()))
    }
    fn keyscan(&self) -> AppResult<&Path> {
        self.keyscan_path
            .as_deref()
            .ok_or_else(|| AppError::MissingExecutable("OpenSSH ssh-keyscan.exe".to_owned()))
    }
}

fn find_executable(names: &[&str]) -> Option<PathBuf> {
    let mut directories = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    if cfg!(windows) {
        if let Some(windows) = std::env::var_os("WINDIR") {
            directories.push(PathBuf::from(windows).join("System32").join("OpenSSH"));
        }
    }
    directories
        .into_iter()
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|candidate| candidate.is_file())
}
fn path_text(path: Option<&Path>) -> Option<String> {
    path.map(|value| value.to_string_lossy().into_owned())
}
fn destination(host: &HostProfile) -> String {
    format!("{}@{}", host.username, host.hostname)
}
fn push_option(args: &mut Vec<OsString>, value: String) {
    args.push(OsString::from("-o"));
    args.push(OsString::from(value));
}
fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
async fn run_output(program: &Path, args: &[OsString], duration: Duration) -> AppResult<Output> {
    let mut command = Command::new(program);
    command.args(args).kill_on_drop(true);
    match timeout(duration, command.output()).await {
        Ok(result) => result.map_err(AppError::from),
        Err(_) => Err(AppError::Timeout(format!(
            "{} exceeded {} seconds",
            program.display(),
            duration.as_secs()
        ))),
    }
}
fn validate_remote_command(command: &str) -> AppResult<()> {
    if command.trim().is_empty() || command.len() > 32_768 || command.contains('\0') {
        return Err(AppError::Validation("remote command is invalid".to_owned()));
    }
    Ok(())
}
fn validate_remote_path(path: &str) -> AppResult<()> {
    if path.trim().is_empty() || path.len() > 4096 || path.contains('\0') {
        return Err(AppError::Validation("remote path is invalid".to_owned()));
    }
    Ok(())
}
fn validate_host_key_fields(algorithm: &str, key: &str) -> AppResult<()> {
    if algorithm.is_empty()
        || algorithm.len() > 128
        || algorithm
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        || key.len() < 16
        || key.len() > 16_384
        || key.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(AppError::Validation(
            "host-key candidate contains invalid fields".to_owned(),
        ));
    }
    Ok(())
}
fn line_matches_host_token(line: &str, token: &str) -> bool {
    line.split_whitespace()
        .next()
        .is_some_and(|hosts| hosts.split(',').any(|host| host == token))
}
fn known_hosts_host_token(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    }
}
fn posix_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn null_device() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}
fn lossy_limited(bytes: &[u8]) -> String {
    const MAX: usize = 1_048_576;
    if bytes.len() <= MAX {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        let mut value = String::from_utf8_lossy(&bytes[..MAX]).into_owned();
        value.push_str("\n[RemoteDeck: output truncated at 1 MiB]");
        value
    }
}
fn nonempty_or(value: String, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_owned()
    } else {
        value
    }
}
fn atomic_write_text(path: &Path, content: &str) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::State("known_hosts path has no parent".to_owned()))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".known-hosts-{}.tmp", Uuid::new_v4()));
    fs::write(&temporary, content.as_bytes())?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_) if path.exists() => {
            fs::remove_file(path)?;
            fs::rename(&temporary, path).map_err(AppError::from)
        }
        Err(error) => Err(AppError::Io(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quotes_posix_paths() {
        assert_eq!(posix_quote("/tmp/a'b"), "'/tmp/a'\\''b'");
    }
    #[test]
    fn formats_known_host_tokens() {
        assert_eq!(known_hosts_host_token("example.test", 22), "example.test");
        assert_eq!(
            known_hosts_host_token("example.test", 2222),
            "[example.test]:2222"
        );
    }
    #[test]
    fn matches_exact_host_tokens() {
        assert!(line_matches_host_token("host ssh-ed25519 AAAA", "host"));
        assert!(!line_matches_host_token("host2 ssh-ed25519 AAAA", "host"));
    }
}
