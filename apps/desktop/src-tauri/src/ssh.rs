use crate::{
    error::{AppError, AppResult},
    model::{
        AuthMethod, CommandResult, ConnectionTestResult, HostKeyCandidate, HostProfile,
        RuntimeCapabilities, TunnelDirection, TunnelProfile,
    },
    store::AppRepository,
};
use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{Mutex, Notify, Semaphore},
    task::JoinHandle,
    time::timeout,
};
use uuid::Uuid;

#[path = "sftp.rs"]
pub(crate) mod sftp;

const OUTPUT_LIMIT: usize = 1_048_576;
const OUTPUT_TRUNCATED_MARKER: &str = "\n[RemoteDeck: output truncated at 1 MiB]";
const READER_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_KNOWN_HOSTS_BYTES: usize = 8 * 1024 * 1024;
const MAX_KNOWN_HOST_RECORDS: usize = 4_096;
const MAX_BOUNDED_OPENSSH_CHILDREN: usize = 32;
const OPENSSH_CHILD_SLOT_TIMEOUT: Duration = Duration::from_secs(30);
static BOUNDED_OPENSSH_CHILD_LIMITER: Semaphore =
    Semaphore::const_new(MAX_BOUNDED_OPENSSH_CHILDREN);

#[derive(Debug, Default)]
pub(crate) struct ChildCancellation {
    cancelled: AtomicBool,
    changed: Notify,
}

impl ChildCancellation {
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.changed.notify_waiters();
        self.changed.notify_one();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    async fn wait(&self) {
        while !self.is_cancelled() {
            let changed = self.changed.notified();
            if self.is_cancelled() {
                return;
            }
            changed.await;
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrustedHostKey {
    pub host_token: String,
    pub algorithm: String,
    pub public_key_base64: String,
    pub sha256_fingerprint: String,
}

#[derive(Debug, Clone)]
pub struct SshRuntime {
    ssh_path: Option<PathBuf>,
    sftp_path: Option<PathBuf>,
    keyscan_path: Option<PathBuf>,
    keygen_path: Option<PathBuf>,
    known_hosts_path: PathBuf,
    known_hosts_lock: Arc<Mutex<()>>,
    repository: Option<AppRepository>,
}

impl SshRuntime {
    pub fn discover(known_hosts_path: PathBuf) -> Self {
        Self {
            ssh_path: find_executable(&["ssh.exe", "ssh"]),
            sftp_path: find_executable(&["sftp.exe", "sftp"]),
            keyscan_path: find_executable(&["ssh-keyscan.exe", "ssh-keyscan"]),
            keygen_path: find_executable(&["ssh-keygen.exe", "ssh-keygen"]),
            known_hosts_path,
            known_hosts_lock: Arc::new(Mutex::new(())),
            repository: None,
        }
    }

    pub fn discover_with_repository(repository: AppRepository) -> Self {
        let mut runtime = Self::discover(repository.known_hosts_path().to_path_buf());
        runtime.repository = Some(repository);
        runtime
    }

    pub fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities {
            ssh_path: path_text(self.ssh_path.as_deref()),
            keyscan_path: path_text(self.keyscan_path.as_deref()),
            keygen_path: path_text(self.keygen_path.as_deref()),
            pty: self.ssh_path.is_some(),
            local_forward: self.ssh_path.is_some(),
            remote_forward: self.ssh_path.is_some(),
            sftp: self.sftp_path.is_some(),
            telemetry: self.ssh_path.is_some(),
            process_signals: self.ssh_path.is_some(),
            agents: self.ssh_path.is_some(),
        }
    }

    /// Resolves a captured profile by id immediately before a reconnect.
    /// Production runtimes always carry the repository; repository-free test
    /// runtimes retain the supplied immutable profile.
    pub(crate) fn current_host_profile(&self, captured: &HostProfile) -> AppResult<HostProfile> {
        self.repository.as_ref().map_or_else(
            || Ok(captured.clone()),
            |repository| repository.host(&captured.id),
        )
    }

    pub async fn scan_host_keys(&self, host: &HostProfile) -> AppResult<Vec<HostKeyCandidate>> {
        let seconds = host.advanced.connect_timeout_seconds.clamp(1, 300);
        let output = if let Some(jump) = self.resolve_jump(host)? {
            self.scan_host_keys_through_jump(host, &jump, seconds)
                .await?
        } else {
            let args = vec![
                OsString::from("-p"),
                OsString::from(host.port.to_string()),
                OsString::from("-T"),
                OsString::from(seconds.to_string()),
                OsString::from(&host.hostname),
            ];
            run_output(self.keyscan()?, &args, Duration::from_secs(seconds + 3)).await?
        };
        let expected_token = known_hosts_host_token(&host.hostname, host.port);
        let trusted_keys = self.list_trusted_keys().await?;
        let candidates = parse_scan_candidates(
            &captured_text(&output.stdout),
            &expected_token,
            &trusted_keys,
        )?;
        if candidates.is_empty() && !output.status.success() {
            return Err(AppError::Process(nonempty_or(
                captured_text(&output.stderr),
                "ssh-keyscan did not return a host key",
            )));
        }
        Ok(candidates)
    }

    async fn scan_host_keys_through_jump(
        &self,
        host: &HostProfile,
        jump: &HostProfile,
        seconds: u64,
    ) -> AppResult<BoundedOutput> {
        validate_proxy_endpoint(host)?;
        validate_proxy_endpoint(jump)?;
        let jump_token = known_hosts_host_token(&jump.hostname, jump.port);
        if !self
            .list_trusted_keys()
            .await?
            .iter()
            .any(|record| record.host_token == jump_token)
        {
            return Err(AppError::Validation(format!(
                "ProxyJump host '{}' must be scanned and explicitly trusted first",
                jump.alias
            )));
        }
        let remote_scan = format!(
            "command -v ssh-keyscan >/dev/null 2>&1 || {{ printf '%s\\n' 'ssh-keyscan is unavailable on the ProxyJump host' >&2; exit 127; }}; exec ssh-keyscan -p {} -T {} {}",
            host.port,
            seconds,
            posix_quote(&host.hostname)
        );
        let mut args = self.jump_connection_args(jump)?;
        args.push(OsString::from(destination(jump)));
        args.push(OsString::from(remote_scan));
        run_output(
            self.ssh()?,
            &args,
            Duration::from_secs(
                jump.advanced
                    .connect_timeout_seconds
                    .saturating_add(seconds)
                    .saturating_add(5),
            ),
        )
        .await
    }

    pub async fn accept_host_key(
        &self,
        host: &HostProfile,
        candidate: &HostKeyCandidate,
    ) -> AppResult<()> {
        validate_host_key_fields(&candidate.algorithm, &candidate.public_key_base64)?;
        if fingerprint_public_key(&candidate.public_key_base64)? != candidate.sha256_fingerprint {
            return Err(AppError::Validation(
                "host-key candidate fingerprint does not match its public key".to_owned(),
            ));
        }
        let token = known_hosts_host_token(&host.hostname, host.port);
        if candidate.host_token != token {
            return Err(AppError::Validation(
                "host-key candidate does not belong to the selected host".to_owned(),
            ));
        }
        {
            let _guard = self.known_hosts_lock.lock().await;
            let existing = read_known_hosts(&self.known_hosts_path)?;
            match trust_decision(&parse_trusted_keys(&existing)?, &token, candidate) {
                TrustDecision::AlreadyTrusted => return Ok(()),
                TrustDecision::Changed => return Err(changed_host_key_error()),
                TrustDecision::FirstUse => {}
            }
        }
        let current = self.scan_host_keys(host).await?;
        if !current.iter().any(|item| {
            item.algorithm == candidate.algorithm
                && item.public_key_base64 == candidate.public_key_base64
                && item.sha256_fingerprint == candidate.sha256_fingerprint
        }) {
            return Err(AppError::Process("host key changed between scan and acceptance; scan again and verify the new fingerprint".to_owned()));
        }
        let _guard = self.known_hosts_lock.lock().await;
        let existing = read_known_hosts(&self.known_hosts_path)?;
        match trust_decision(&parse_trusted_keys(&existing)?, &token, candidate) {
            TrustDecision::AlreadyTrusted => return Ok(()),
            TrustDecision::Changed => return Err(changed_host_key_error()),
            TrustDecision::FirstUse => {}
        }
        let mut lines = existing
            .lines()
            .filter(|line| !line_matches_host_algorithm(line, &token, &candidate.algorithm))
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

    pub async fn list_trusted_keys(&self) -> AppResult<Vec<TrustedHostKey>> {
        let _guard = self.known_hosts_lock.lock().await;
        let content = read_known_hosts(&self.known_hosts_path)?;
        let mut trusted = parse_trusted_keys(&content)?;
        trusted.sort_by(|left, right| {
            left.host_token
                .cmp(&right.host_token)
                .then_with(|| left.algorithm.cmp(&right.algorithm))
        });
        Ok(trusted)
    }

    pub async fn remove_trusted_key_record(
        &self,
        host_token: &str,
        algorithm: &str,
        public_key_base64: &str,
    ) -> AppResult<bool> {
        validate_host_token(host_token)?;
        validate_host_key_fields(algorithm, public_key_base64)?;
        let _guard = self.known_hosts_lock.lock().await;
        let existing = read_known_hosts(&self.known_hosts_path)?;
        let (content, removed) =
            remove_trusted_record(&existing, host_token, algorithm, public_key_base64)?;
        if !removed {
            return Ok(false);
        }
        atomic_write_text(&self.known_hosts_path, &content)?;
        Ok(true)
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
        let stdout = captured_text(&output.stdout);
        let stderr = captured_text(&output.stderr);
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
        self.run_command_with_timeout(host, command, working_directory, Duration::from_secs(3600))
            .await
    }

    pub(crate) async fn run_command_with_timeout(
        &self,
        host: &HostProfile,
        command: String,
        working_directory: Option<String>,
        duration: Duration,
    ) -> AppResult<CommandResult> {
        if duration.is_zero() || duration > Duration::from_secs(3600) {
            return Err(AppError::Validation(
                "SSH command timeout must be between 1 second and 1 hour".to_owned(),
            ));
        }
        validate_remote_command(&command)?;
        let remote = match working_directory {
            Some(directory) if !directory.trim().is_empty() => {
                validate_remote_path(&directory)?;
                format!("cd -- {} && {command}", shell_path(directory.trim()))
            }
            _ => command,
        };
        let (program, args) = self.remote_command_spec(host, remote, true)?;
        let start = Instant::now();
        let output = run_output(&program, &args, duration).await?;
        Ok(CommandResult {
            exit_code: output.status.code(),
            stdout: captured_text(&output.stdout),
            stderr: captured_text(&output.stderr),
            duration_ms: start.elapsed().as_millis(),
        })
    }

    pub fn terminal_command_with_remote(
        &self,
        host: &HostProfile,
        remote_command: Option<&str>,
    ) -> AppResult<(PathBuf, Vec<OsString>)> {
        let program = self.ssh()?.to_path_buf();
        let mut args = self.base_args(host, false)?;
        args.push(OsString::from("-tt"));
        args.push(OsString::from(destination(host)));
        if let Some(remote_command) = remote_command {
            validate_remote_command(remote_command)?;
            args.push(OsString::from(remote_command));
        } else {
            let workspace = host.default_workspace.trim();
            if !workspace.is_empty() {
                validate_remote_path(workspace)?;
                args.push(OsString::from(format!(
                    "cd -- {} && exec \"${{SHELL:-/bin/sh}}\" -l",
                    shell_path(workspace)
                )));
            }
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
            forward_host(&tunnel.bind_address),
            tunnel.source_port,
            forward_host(&tunnel.target_host),
            tunnel.target_port
        )));
        args.push(OsString::from(destination(host)));
        Ok((program, args))
    }

    pub(crate) fn remote_command_spec(
        &self,
        host: &HostProfile,
        remote_command: String,
        batch_mode: bool,
    ) -> AppResult<(PathBuf, Vec<OsString>)> {
        validate_remote_command(&remote_command)?;
        let program = self.ssh()?.to_path_buf();
        let mut args = self.base_args(host, batch_mode)?;
        args.push(OsString::from(destination(host)));
        args.push(OsString::from(remote_command));
        Ok((program, args))
    }

    pub(crate) fn sftp_batch_spec(
        &self,
        host: &HostProfile,
    ) -> AppResult<(PathBuf, Vec<OsString>)> {
        let program = self.sftp()?.to_path_buf();
        let mut args = self.connection_args(host, true, "-P")?;
        args.extend([
            OsString::from("-q"),
            OsString::from("-b"),
            OsString::from("-"),
            OsString::from(destination(host)),
        ]);
        Ok((program, args))
    }

    fn base_args(&self, host: &HostProfile, batch_mode: bool) -> AppResult<Vec<OsString>> {
        self.connection_args(host, batch_mode, "-p")
    }

    fn connection_args(
        &self,
        host: &HostProfile,
        batch_mode: bool,
        port_flag: &str,
    ) -> AppResult<Vec<OsString>> {
        let mut args = self.direct_connection_args(host, batch_mode, port_flag)?;
        if let Some(jump) = self.resolve_jump(host)? {
            validate_proxy_endpoint(host)?;
            validate_proxy_endpoint(&jump)?;
            push_option(
                &mut args,
                format!("ProxyCommand={}", self.proxy_command(&jump)?),
            );
            push_option(&mut args, "ProxyUseFdpass=no".to_owned());
        }
        Ok(args)
    }

    fn direct_connection_args(
        &self,
        host: &HostProfile,
        batch_mode: bool,
        port_flag: &str,
    ) -> AppResult<Vec<OsString>> {
        if !self.known_hosts_path.exists() {
            return Err(AppError::State(
                "known_hosts file is unavailable".to_owned(),
            ));
        }
        let mut args = vec![
            OsString::from("-F"),
            OsString::from("none"),
            OsString::from(port_flag),
            OsString::from(host.port.to_string()),
        ];
        push_option(
            &mut args,
            format!("BatchMode={}", if batch_mode { "yes" } else { "no" }),
        );
        push_strict_host_key_options(&mut args, &self.known_hosts_path);
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
        match host.auth_method {
            AuthMethod::PrivateKey => {
                let identity = host.identity_file.as_ref().ok_or_else(|| {
                    AppError::Validation(
                        "private-key authentication requires an identity file".to_owned(),
                    )
                })?;
                push_option(&mut args, "PreferredAuthentications=publickey".to_owned());
                args.push(OsString::from("-i"));
                args.push(OsString::from(identity));
                if host.advanced.identities_only {
                    push_option(&mut args, "IdentitiesOnly=yes".to_owned());
                }
            }
            AuthMethod::Agent => {
                push_option(&mut args, "PreferredAuthentications=publickey".to_owned());
            }
            AuthMethod::Interactive => {
                push_option(
                    &mut args,
                    "PreferredAuthentications=keyboard-interactive,password".to_owned(),
                );
            }
        }
        Ok(args)
    }

    fn resolve_jump(&self, host: &HostProfile) -> AppResult<Option<HostProfile>> {
        if host.proxy_jump.is_none() {
            return Ok(None);
        }
        self.repository
            .as_ref()
            .ok_or_else(|| {
                AppError::State(
                    "ProxyJump resolution requires the saved-host repository".to_owned(),
                )
            })?
            .resolve_jump_host(host)
    }

    fn proxy_command(&self, jump: &HostProfile) -> AppResult<String> {
        let mut args = self.jump_connection_args(jump)?;
        push_option(&mut args, "ClearAllForwardings=yes".to_owned());
        push_option(&mut args, "ExitOnForwardFailure=yes".to_owned());
        args.push(OsString::from("-W"));
        args.push(OsString::from("%h:%p"));
        args.push(OsString::from(destination(jump)));
        format_proxy_command(self.ssh()?, &args)
    }

    fn jump_connection_args(&self, jump: &HostProfile) -> AppResult<Vec<OsString>> {
        let mut isolated = jump.clone();
        match isolated.auth_method {
            AuthMethod::PrivateKey => isolated.advanced.identities_only = true,
            AuthMethod::Agent | AuthMethod::Interactive => {
                isolated.identity_file = None;
                isolated.advanced.identities_only = false;
            }
        }
        self.direct_connection_args(&isolated, true, "-p")
    }

    fn ssh(&self) -> AppResult<&Path> {
        self.ssh_path
            .as_deref()
            .ok_or_else(|| AppError::MissingExecutable("OpenSSH ssh.exe".to_owned()))
    }
    fn sftp(&self) -> AppResult<&Path> {
        self.sftp_path
            .as_deref()
            .ok_or_else(|| AppError::MissingExecutable("OpenSSH sftp.exe".to_owned()))
    }
    fn keyscan(&self) -> AppResult<&Path> {
        self.keyscan_path
            .as_deref()
            .ok_or_else(|| AppError::MissingExecutable("OpenSSH ssh-keyscan.exe".to_owned()))
    }
}

fn find_executable(names: &[&str]) -> Option<PathBuf> {
    executable_search_directories(
        std::env::var_os("WINDIR").or_else(|| std::env::var_os("SYSTEMROOT")),
        std::env::var_os("PATH"),
        cfg!(windows),
    )
    .into_iter()
    .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
    .find(|candidate| candidate.is_file())
}
fn executable_search_directories(
    windows_directory: Option<OsString>,
    path: Option<OsString>,
    is_windows: bool,
) -> Vec<PathBuf> {
    if is_windows {
        return windows_directory
            .map(PathBuf::from)
            .map(|windows| windows.join("System32").join("OpenSSH"))
            .into_iter()
            .collect();
    }
    let mut directories = Vec::new();
    if let Some(path) = path {
        directories.extend(std::env::split_paths(&path));
    }
    directories
}
fn path_text(path: Option<&Path>) -> Option<String> {
    path.map(|value| value.to_string_lossy().into_owned())
}
fn destination(host: &HostProfile) -> String {
    format!("{}@{}", host.username, host.hostname)
}
fn forward_host(value: &str) -> String {
    if value.contains(':') && !(value.starts_with('[') && value.ends_with(']')) {
        format!("[{value}]")
    } else {
        value.to_owned()
    }
}
fn push_option(args: &mut Vec<OsString>, value: String) {
    args.push(OsString::from("-o"));
    args.push(OsString::from(value));
}
fn push_strict_host_key_options(args: &mut Vec<OsString>, known_hosts_path: &Path) {
    push_option(args, "StrictHostKeyChecking=yes".to_owned());
    push_option(
        args,
        format!("UserKnownHostsFile={}", known_hosts_path.display()),
    );
    push_option(args, format!("GlobalKnownHostsFile={}", null_device()));
    push_option(args, "UpdateHostKeys=no".to_owned());
    push_option(args, "VerifyHostKeyDNS=no".to_owned());
    push_option(args, "CheckHostIP=no".to_owned());
}

fn validate_proxy_endpoint(host: &HostProfile) -> AppResult<()> {
    let valid_username = !host.username.is_empty()
        && host.username.len() <= 128
        && host.username.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '+')
        });
    let valid_hostname = !host.hostname.is_empty()
        && host.hostname.len() <= 512
        && !host.hostname.starts_with('-')
        && host.hostname.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '.' | '_' | '-' | ':' | '[' | ']')
        });
    if !valid_username || !valid_hostname {
        return Err(AppError::Validation(format!(
            "host '{}' contains characters that are unsafe for a ProxyJump command",
            host.alias
        )));
    }
    Ok(())
}

fn format_proxy_command(program: &Path, args: &[OsString]) -> AppResult<String> {
    let mut fields = Vec::with_capacity(args.len() + 1);
    fields.push(proxy_command_argument(program.to_str().ok_or_else(
        || AppError::Validation("ssh executable path is not Unicode".to_owned()),
    )?)?);
    for argument in args {
        let argument = argument.to_str().ok_or_else(|| {
            AppError::Validation("ProxyJump argument contains non-Unicode data".to_owned())
        })?;
        if argument == "%h:%p" {
            fields.push(if cfg!(windows) {
                "\"%h:%p\"".to_owned()
            } else {
                posix_quote(argument)
            });
        } else {
            fields.push(proxy_command_argument(argument)?);
        }
    }
    Ok(fields.join(" "))
}

fn proxy_command_argument(value: &str) -> AppResult<String> {
    if value.contains('\0') || value.contains(['\r', '\n']) {
        return Err(AppError::Validation(
            "ProxyJump command argument contains a control character".to_owned(),
        ));
    }
    if !cfg!(windows) {
        return Ok(posix_quote(value));
    }
    if value.chars().any(|character| {
        matches!(
            character,
            '"' | '%' | '!' | '^' | '&' | '|' | '<' | '>' | '$' | '`'
        )
    }) {
        return Err(AppError::Validation(
            "ProxyJump command argument contains a Windows shell metacharacter".to_owned(),
        ));
    }
    let trailing_backslashes = value
        .chars()
        .rev()
        .take_while(|character| *character == '\\')
        .count();
    Ok(format!("\"{value}{}\"", "\\".repeat(trailing_backslashes)))
}

fn parse_scan_candidates(
    stdout: &str,
    expected_token: &str,
    trusted_keys: &[TrustedHostKey],
) -> AppResult<Vec<HostKeyCandidate>> {
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for line in stdout.lines() {
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
        let fingerprint = fingerprint_public_key(key)?;
        if seen.insert(format!("{algorithm}:{key}")) {
            let (trusted, mismatch, previous_fingerprint) =
                host_key_candidate_status(trusted_keys, expected_token, algorithm, key);
            candidates.push(HostKeyCandidate {
                host_token: expected_token.to_owned(),
                algorithm: algorithm.to_owned(),
                public_key_base64: key.to_owned(),
                sha256_fingerprint: fingerprint,
                raw_line: format!("{expected_token} {algorithm} {key}"),
                trusted,
                mismatch,
                previous_fingerprint,
            });
        }
    }
    candidates.sort_by(|left, right| left.algorithm.cmp(&right.algorithm));
    Ok(candidates)
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
async fn run_output(
    program: &Path,
    args: &[OsString],
    duration: Duration,
) -> AppResult<BoundedOutput> {
    run_output_with_input(program, args, duration, None).await
}

pub(crate) async fn run_output_with_input(
    program: &Path,
    args: &[OsString],
    duration: Duration,
    input: Option<Vec<u8>>,
) -> AppResult<BoundedOutput> {
    run_output_with_input_cancellable(program, args, duration, input, None).await
}

pub(crate) async fn run_output_with_input_cancellable(
    program: &Path,
    args: &[OsString],
    duration: Duration,
    input: Option<Vec<u8>>,
    cancellation: Option<&ChildCancellation>,
) -> AppResult<BoundedOutput> {
    if cancellation.is_some_and(ChildCancellation::is_cancelled) {
        return Err(AppError::Cancelled);
    }
    let _permit = tokio::select! {
        result = timeout(
            OPENSSH_CHILD_SLOT_TIMEOUT,
            BOUNDED_OPENSSH_CHILD_LIMITER.acquire(),
        ) => result
            .map_err(|_| AppError::Timeout("waiting for an OpenSSH child slot timed out".to_owned()))?
            .map_err(|_| AppError::State("OpenSSH child limiter is closed".to_owned()))?,
        _ = wait_for_child_cancellation(cancellation) => return Err(AppError::Cancelled),
    };
    if cancellation.is_some_and(ChildCancellation::is_cancelled) {
        return Err(AppError::Cancelled);
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let mut stdin_task = match input {
        Some(input) => {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| AppError::State("child stdin pipe is unavailable".to_owned()))?;
            Some(tokio::spawn(async move {
                stdin.write_all(&input).await?;
                stdin.shutdown().await
            }))
        }
        None => None,
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::State("child stdout pipe is unavailable".to_owned()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::State("child stderr pipe is unavailable".to_owned()))?;
    let mut stdout_task = tokio::spawn(read_bounded(stdout));
    let mut stderr_task = tokio::spawn(read_bounded(stderr));
    enum WaitOutcome {
        Exited(io::Result<ExitStatus>),
        TimedOut,
        Cancelled,
    }
    let outcome = tokio::select! {
        result = child.wait() => WaitOutcome::Exited(result),
        _ = tokio::time::sleep(duration) => WaitOutcome::TimedOut,
        _ = wait_for_child_cancellation(cancellation) => WaitOutcome::Cancelled,
    };
    let status = match outcome {
        WaitOutcome::Exited(result) => result?,
        reason @ (WaitOutcome::TimedOut | WaitOutcome::Cancelled) => {
            let kill_error = child.start_kill().err();
            let reap_result = timeout(READER_DRAIN_TIMEOUT, child.wait()).await;
            if let Some(task) = stdin_task.take() {
                task.abort();
                let _ = task.await;
            }
            let reaped = match reap_result {
                Ok(Ok(_)) => true,
                Ok(Err(error)) => {
                    abort_reader_tasks(&mut stdout_task, &mut stderr_task).await;
                    return Err(AppError::ProcessCleanup(format!(
                        "{} could not be reaped after cancellation: {error}",
                        program.display()
                    )));
                }
                Err(_) => false,
            };
            if !reaped {
                abort_reader_tasks(&mut stdout_task, &mut stderr_task).await;
                return Err(AppError::ProcessCleanup(kill_error.map_or_else(
                    || {
                        format!(
                            "{} did not exit before the child cleanup deadline",
                            program.display()
                        )
                    },
                    |error| {
                        format!(
                            "{} termination failed ({error}) and the child did not exit before the cleanup deadline",
                            program.display()
                        )
                    },
                )));
            }
            drain_or_abort_reader_tasks(&mut stdout_task, &mut stderr_task).await;
            return Err(match reason {
                WaitOutcome::TimedOut => AppError::Timeout(format!(
                    "{} exceeded {} seconds",
                    program.display(),
                    duration.as_secs()
                )),
                WaitOutcome::Cancelled => AppError::Cancelled,
                WaitOutcome::Exited(_) => unreachable!(),
            });
        }
    };
    let input_result = if let Some(task) = stdin_task.take() {
        match task.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Ok(Err(error)) => Err(AppError::Io(error)),
            Err(error) => Err(AppError::Process(format!(
                "input writer task failed: {error}"
            ))),
        }
    } else {
        Ok(())
    };
    let captures = timeout(READER_DRAIN_TIMEOUT, async {
        tokio::try_join!(join_reader(&mut stdout_task), join_reader(&mut stderr_task))
    })
    .await;
    let (stdout, stderr) = match captures {
        Ok(result) => result?,
        Err(_) => {
            stdout_task.abort();
            stderr_task.abort();
            return Err(AppError::Timeout(
                "child output pipes did not close after process exit".to_owned(),
            ));
        }
    };
    input_result?;
    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
    })
}

async fn wait_for_child_cancellation(cancellation: Option<&ChildCancellation>) {
    match cancellation {
        Some(cancellation) => cancellation.wait().await,
        None => std::future::pending::<()>().await,
    }
}

async fn abort_reader_tasks(
    stdout_task: &mut JoinHandle<io::Result<BoundedCapture>>,
    stderr_task: &mut JoinHandle<io::Result<BoundedCapture>>,
) {
    stdout_task.abort();
    stderr_task.abort();
    let _ = stdout_task.await;
    let _ = stderr_task.await;
}

async fn drain_or_abort_reader_tasks(
    stdout_task: &mut JoinHandle<io::Result<BoundedCapture>>,
    stderr_task: &mut JoinHandle<io::Result<BoundedCapture>>,
) {
    if timeout(READER_DRAIN_TIMEOUT, async {
        let _ = tokio::join!(&mut *stdout_task, &mut *stderr_task);
    })
    .await
    .is_err()
    {
        abort_reader_tasks(stdout_task, stderr_task).await;
    }
}

#[derive(Debug)]
pub(crate) struct BoundedOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: BoundedCapture,
    pub(crate) stderr: BoundedCapture,
}

#[derive(Debug, Default)]
pub(crate) struct BoundedCapture {
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
}

impl BoundedCapture {
    fn push(&mut self, chunk: &[u8]) {
        let remaining = OUTPUT_LIMIT.saturating_sub(self.bytes.len());
        let take = remaining.min(chunk.len());
        self.bytes.extend_from_slice(&chunk[..take]);
        self.truncated |= take < chunk.len();
    }
}

pub(crate) async fn read_bounded<R>(mut reader: R) -> io::Result<BoundedCapture>
where
    R: AsyncRead + Unpin,
{
    let mut capture = BoundedCapture::default();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(capture);
        }
        capture.push(&buffer[..read]);
    }
}

pub(crate) async fn join_reader(
    task: &mut JoinHandle<io::Result<BoundedCapture>>,
) -> AppResult<BoundedCapture> {
    task.await
        .map_err(|error| AppError::Process(format!("output reader task failed: {error}")))?
        .map_err(AppError::from)
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
fn validate_host_token(token: &str) -> AppResult<()> {
    if token.is_empty()
        || token.len() > 1024
        || token
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(AppError::Validation(
            "known-hosts host token is invalid".to_owned(),
        ));
    }
    Ok(())
}
fn line_matches_host_algorithm(line: &str, token: &str, algorithm: &str) -> bool {
    let mut fields = line.split_whitespace();
    let Some(hosts) = fields.next() else {
        return false;
    };
    let Some(record_algorithm) = fields.next() else {
        return false;
    };
    record_algorithm == algorithm && hosts.split(',').any(|host| host == token)
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
fn shell_path(value: &str) -> String {
    if value == "~" || value == "~/" {
        return "\"$HOME\"".to_owned();
    }
    if let Some(relative) = value.strip_prefix("~/") {
        return format!("\"$HOME\"/{}", posix_quote(relative));
    }
    posix_quote(value)
}
fn null_device() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}
pub(crate) fn captured_text(capture: &BoundedCapture) -> String {
    let mut value = String::from_utf8_lossy(&capture.bytes).into_owned();
    if capture.truncated {
        truncate_utf8(
            &mut value,
            OUTPUT_LIMIT.saturating_sub(OUTPUT_TRUNCATED_MARKER.len()),
        );
        value.push_str(OUTPUT_TRUNCATED_MARKER);
    } else {
        truncate_utf8(&mut value, OUTPUT_LIMIT);
    }
    value
}
fn truncate_utf8(value: &mut String, maximum_bytes: usize) {
    if value.len() <= maximum_bytes {
        return;
    }
    let mut end = maximum_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
}
fn nonempty_or(value: String, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_owned()
    } else {
        value
    }
}
fn read_known_hosts(path: &Path) -> AppResult<String> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(
            u64::try_from(MAX_KNOWN_HOSTS_BYTES)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_KNOWN_HOSTS_BYTES {
        return Err(AppError::State(format!(
            "known_hosts exceeds the {MAX_KNOWN_HOSTS_BYTES}-byte safety limit"
        )));
    }
    String::from_utf8(bytes)
        .map_err(|_| AppError::State("known_hosts is not valid UTF-8".to_owned()))
}

fn atomic_write_text(path: &Path, content: &str) -> AppResult<()> {
    if content.len() > MAX_KNOWN_HOSTS_BYTES {
        return Err(AppError::State(format!(
            "known_hosts exceeds the {MAX_KNOWN_HOSTS_BYTES}-byte safety limit"
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| AppError::State("known_hosts path has no parent".to_owned()))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".known-hosts-{}.tmp", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    drop(file);
    let result = replace_known_hosts_file(&temporary, path);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn replace_known_hosts_file(temporary: &Path, destination: &Path) -> AppResult<()> {
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
        Err(AppError::Io(std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_known_hosts_file(temporary: &Path, destination: &Path) -> AppResult<()> {
    fs::rename(temporary, destination)?;
    if let Some(parent) = destination.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn fingerprint_public_key(public_key_base64: &str) -> AppResult<String> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(public_key_base64)
        .map_err(|error| AppError::Process(format!("host key contains invalid base64: {error}")))?;
    Ok(format!(
        "SHA256:{}",
        STANDARD_NO_PAD.encode(Sha256::digest(decoded))
    ))
}

fn parse_trusted_keys(content: &str) -> AppResult<Vec<TrustedHostKey>> {
    let mut trusted = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let hosts = fields.next();
        let algorithm = fields.next();
        let public_key_base64 = fields.next();
        let (Some(hosts), Some(algorithm), Some(public_key_base64)) =
            (hosts, algorithm, public_key_base64)
        else {
            return Err(AppError::State(
                "app-owned known_hosts contains an unsupported record".to_owned(),
            ));
        };
        if hosts.starts_with('@') {
            return Err(AppError::State(
                "app-owned known_hosts contains an unsupported record".to_owned(),
            ));
        }
        validate_host_key_fields(algorithm, public_key_base64)?;
        let fingerprint = fingerprint_public_key(public_key_base64)?;
        for host_token in hosts.split(',') {
            if trusted.len() >= MAX_KNOWN_HOST_RECORDS {
                return Err(AppError::State(format!(
                    "app-owned known_hosts exceeds {MAX_KNOWN_HOST_RECORDS} records"
                )));
            }
            validate_host_token(host_token)?;
            trusted.push(TrustedHostKey {
                host_token: host_token.to_owned(),
                algorithm: algorithm.to_owned(),
                public_key_base64: public_key_base64.to_owned(),
                sha256_fingerprint: fingerprint.clone(),
            });
        }
    }
    Ok(trusted)
}

fn remove_trusted_record(
    content: &str,
    host_token: &str,
    algorithm: &str,
    public_key_base64: &str,
) -> AppResult<(String, bool)> {
    parse_trusted_keys(content)?;
    let mut removed = false;
    let mut output = Vec::new();
    for line in content.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 3
            || fields[1] != algorithm
            || fields[2] != public_key_base64
            || !fields[0].split(',').any(|host| host == host_token)
        {
            output.push(line.to_owned());
            continue;
        }
        removed = true;
        let remaining_hosts = fields[0]
            .split(',')
            .filter(|host| *host != host_token)
            .collect::<Vec<_>>();
        if !remaining_hosts.is_empty() {
            output.push(format!(
                "{} {}",
                remaining_hosts.join(","),
                fields[1..].join(" ")
            ));
        }
    }
    let mut updated = output.join("\n");
    if !updated.is_empty() {
        updated.push('\n');
    }
    Ok((updated, removed))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrustDecision {
    FirstUse,
    AlreadyTrusted,
    Changed,
}

fn changed_host_key_error() -> AppError {
    AppError::Process(
        "SSH host key changed; remove the existing trusted key explicitly before accepting a replacement"
            .to_owned(),
    )
}

fn trust_decision(
    trusted: &[TrustedHostKey],
    host_token: &str,
    candidate: &HostKeyCandidate,
) -> TrustDecision {
    let records = trusted
        .iter()
        .filter(|record| record.host_token == host_token && record.algorithm == candidate.algorithm)
        .collect::<Vec<_>>();
    if records.is_empty() {
        TrustDecision::FirstUse
    } else if records.iter().any(|record| {
        record.algorithm == candidate.algorithm
            && record.public_key_base64 == candidate.public_key_base64
            && record.sha256_fingerprint == candidate.sha256_fingerprint
    }) {
        TrustDecision::AlreadyTrusted
    } else {
        TrustDecision::Changed
    }
}

fn host_key_candidate_status(
    trusted: &[TrustedHostKey],
    host_token: &str,
    algorithm: &str,
    public_key_base64: &str,
) -> (bool, bool, Option<String>) {
    let mut previous_fingerprint = None;
    for record in trusted
        .iter()
        .filter(|record| record.host_token == host_token && record.algorithm == algorithm)
    {
        if record.public_key_base64 == public_key_base64 {
            return (true, false, None);
        }
        if previous_fingerprint.is_none() {
            previous_fingerprint = Some(record.sha256_fingerprint.clone());
        }
    }
    let mismatch = previous_fingerprint.is_some();
    (false, mismatch, previous_fingerprint)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quotes_posix_paths() {
        assert_eq!(posix_quote("/tmp/a'b"), "'/tmp/a'\\''b'");
        assert_eq!(shell_path("~"), "\"$HOME\"");
        assert_eq!(shell_path("~/"), "\"$HOME\"");
        assert_eq!(shell_path("~/work tree"), "\"$HOME\"/'work tree'");
    }
    #[test]
    fn formats_known_host_tokens() {
        assert_eq!(known_hosts_host_token("example.test", 22), "example.test");
        assert_eq!(
            known_hosts_host_token("example.test", 2222),
            "[example.test]:2222"
        );
        assert_eq!(forward_host("127.0.0.1"), "127.0.0.1");
        assert_eq!(forward_host("::1"), "[::1]");
        assert_eq!(forward_host("[2001:db8::1]"), "[2001:db8::1]");
    }
    #[test]
    fn windows_uses_only_the_system_openssh_directory() {
        let windows = OsString::from(r"C:\Windows");
        let path = std::env::join_paths([PathBuf::from(r"C:\custom")]).expect("PATH");
        let directories = executable_search_directories(Some(windows), Some(path), true);
        assert_eq!(directories, [PathBuf::from(r"C:\Windows\System32\OpenSSH")]);
    }

    #[test]
    fn strict_arguments_disable_external_config_and_key_updates() {
        let directory = std::env::temp_dir().join(format!("remotedeck-ssh-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create temp directory");
        let known_hosts = directory.join("known_hosts");
        fs::write(&known_hosts, []).expect("create known_hosts");
        let runtime = SshRuntime {
            ssh_path: Some(PathBuf::from("ssh")),
            sftp_path: Some(PathBuf::from("sftp")),
            keyscan_path: Some(PathBuf::from("ssh-keyscan")),
            keygen_path: Some(PathBuf::from("ssh-keygen")),
            known_hosts_path: known_hosts.clone(),
            known_hosts_lock: Arc::new(Mutex::new(())),
            repository: None,
        };
        let args = runtime.base_args(&host_profile(), true).expect("arguments");
        let text = args
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(text.windows(2).any(|pair| pair == ["-F", "none"]));
        for option in [
            "StrictHostKeyChecking=yes".to_owned(),
            format!("UserKnownHostsFile={}", known_hosts.display()),
            format!("GlobalKnownHostsFile={}", null_device()),
            "UpdateHostKeys=no".to_owned(),
            "VerifyHostKeyDNS=no".to_owned(),
            "CheckHostIP=no".to_owned(),
        ] {
            assert!(
                text.windows(2)
                    .any(|pair| pair[0] == "-o" && pair[1] == option)
            );
        }
        fs::remove_dir_all(directory).expect("remove temp directory");
    }

    #[test]
    fn direct_arguments_honor_auth_method_even_with_a_stale_identity_path() {
        let directory = std::env::temp_dir().join(format!("remotedeck-auth-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create temp directory");
        let known_hosts = directory.join("known_hosts");
        fs::write(&known_hosts, []).expect("create known_hosts");
        let runtime = SshRuntime {
            ssh_path: Some(PathBuf::from("ssh")),
            sftp_path: Some(PathBuf::from("sftp")),
            keyscan_path: Some(PathBuf::from("ssh-keyscan")),
            keygen_path: Some(PathBuf::from("ssh-keygen")),
            known_hosts_path: known_hosts,
            known_hosts_lock: Arc::new(Mutex::new(())),
            repository: None,
        };
        let mut host = host_profile();
        host.identity_file = Some(r"C:\Keys\stale-key".to_owned());

        host.auth_method = AuthMethod::Interactive;
        let interactive = runtime.base_args(&host, false).expect("interactive args");
        assert!(!interactive.iter().any(|value| value == "-i"));
        assert!(interactive.windows(2).any(|pair| {
            pair[0] == "-o" && pair[1] == "PreferredAuthentications=keyboard-interactive,password"
        }));

        host.auth_method = AuthMethod::Agent;
        let agent = runtime.base_args(&host, true).expect("agent args");
        assert!(!agent.iter().any(|value| value == "-i"));
        assert!(
            agent
                .windows(2)
                .any(|pair| { pair[0] == "-o" && pair[1] == "PreferredAuthentications=publickey" })
        );
        fs::remove_dir_all(directory).expect("remove temp directory");
    }

    #[test]
    fn sftp_spec_reuses_typed_connection_arguments() {
        let directory = std::env::temp_dir().join(format!("remotedeck-spec-{}", Uuid::new_v4()));
        let repository = AppRepository::open(directory.clone()).expect("open repository");
        let jump = repository
            .save_host(crate::model::HostDraft {
                id: None,
                alias: "jump".to_owned(),
                hostname: "jump.example".to_owned(),
                port: 2200,
                username: "jump-user".to_owned(),
                auth_method: Some(crate::model::AuthMethod::PrivateKey),
                identity_file: Some(r"C:\Keys\jump key".to_owned()),
                proxy_jump: None,
                default_workspace: Some("~".to_owned()),
                groups: Vec::new(),
                advanced: None,
                monitor_enabled: Some(false),
            })
            .expect("save jump");
        let host = repository
            .save_host(crate::model::HostDraft {
                id: None,
                alias: "target".to_owned(),
                hostname: "example.test".to_owned(),
                port: 2222,
                username: "tester".to_owned(),
                auth_method: Some(crate::model::AuthMethod::PrivateKey),
                identity_file: Some(r"C:\Keys\research key".to_owned()),
                proxy_jump: Some(jump.id),
                default_workspace: Some("~".to_owned()),
                groups: Vec::new(),
                advanced: None,
                monitor_enabled: Some(false),
            })
            .expect("save target");
        let runtime = SshRuntime {
            ssh_path: Some(PathBuf::from("ssh")),
            sftp_path: Some(PathBuf::from("system-sftp")),
            keyscan_path: Some(PathBuf::from("ssh-keyscan")),
            keygen_path: Some(PathBuf::from("ssh-keygen")),
            known_hosts_path: repository.known_hosts_path().to_path_buf(),
            known_hosts_lock: Arc::new(Mutex::new(())),
            repository: Some(repository),
        };

        let (sftp, sftp_args) = runtime.sftp_batch_spec(&host).expect("sftp spec");
        let sftp_args = sftp_args
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(sftp, PathBuf::from("system-sftp"));
        for pair in [
            ["-F", "none"],
            ["-P", "2222"],
            ["-i", r"C:\Keys\research key"],
            ["-b", "-"],
        ] {
            assert!(sftp_args.windows(2).any(|values| values == pair));
        }
        assert!(!sftp_args.iter().any(|value| value == "-J"));
        let proxy_command = sftp_args
            .windows(2)
            .find(|values| values[0] == "-o" && values[1].starts_with("ProxyCommand="))
            .map(|values| values[1].as_str())
            .expect("explicit ProxyCommand");
        for required in [
            "-F",
            "none",
            "StrictHostKeyChecking=yes",
            "GlobalKnownHostsFile=",
            "C:\\Keys\\jump key",
            "IdentitiesOnly=yes",
            "2200",
            "%h:%p",
            "jump-user@jump.example",
        ] {
            assert!(proxy_command.contains(required), "missing {required}");
        }
        assert_eq!(
            sftp_args.last().map(String::as_str),
            Some("tester@example.test")
        );

        let (ssh, terminal_args) = runtime
            .terminal_command_with_remote(&host, Some("exec codex --version"))
            .expect("remote terminal spec");
        assert_eq!(ssh, PathBuf::from("ssh"));
        assert_eq!(
            terminal_args.last(),
            Some(&OsString::from("exec codex --version"))
        );
        assert!(terminal_args.iter().any(|value| value == "-tt"));
        assert!(
            terminal_args
                .windows(2)
                .any(|values| { values[0] == "-o" && values[1] == "BatchMode=no" })
        );
        assert!(
            runtime
                .terminal_command_with_remote(&host, Some("\0"))
                .is_err()
        );
        fs::remove_dir_all(directory).expect("remove temp directory");
    }

    #[test]
    fn proxy_command_rejects_windows_shell_metacharacters() {
        if cfg!(windows) {
            assert!(proxy_command_argument(r"C:\safe path\known_hosts").is_ok());
            for unsafe_value in [
                r"C:\Users\%USERNAME%\key",
                r"C:\key & calc.exe",
                r"C:\key!name",
                r"C:\key^name",
                r"C:\key$(calc).name",
                r"C:\key`name",
            ] {
                assert!(
                    proxy_command_argument(unsafe_value).is_err(),
                    "accepted {unsafe_value}"
                );
            }
        }
    }

    #[test]
    fn system_openssh_accepts_the_generated_proxy_configuration() {
        let directory =
            std::env::temp_dir().join(format!("remotedeck-proxy-parse-{}", Uuid::new_v4()));
        let repository = AppRepository::open(directory.clone()).expect("open repository");
        let identity = directory.join("jump-key").to_string_lossy().into_owned();
        let jump = repository
            .save_host(crate::model::HostDraft {
                id: None,
                alias: "parser-jump".to_owned(),
                hostname: "jump.example".to_owned(),
                port: 2200,
                username: "jump-user".to_owned(),
                auth_method: Some(crate::model::AuthMethod::PrivateKey),
                identity_file: Some(identity),
                proxy_jump: None,
                default_workspace: Some("~".to_owned()),
                groups: Vec::new(),
                advanced: None,
                monitor_enabled: Some(false),
            })
            .expect("save jump");
        let target = repository
            .save_host(crate::model::HostDraft {
                id: None,
                alias: "parser-target".to_owned(),
                hostname: "target.internal".to_owned(),
                port: 2222,
                username: "target-user".to_owned(),
                auth_method: Some(crate::model::AuthMethod::Agent),
                identity_file: None,
                proxy_jump: Some(jump.id),
                default_workspace: Some("~".to_owned()),
                groups: Vec::new(),
                advanced: None,
                monitor_enabled: Some(false),
            })
            .expect("save target");
        let runtime = SshRuntime::discover_with_repository(repository);
        let Some(program) = runtime.ssh_path.as_deref() else {
            fs::remove_dir_all(directory).expect("remove directory");
            return;
        };
        let mut args = runtime.base_args(&target, true).expect("proxy arguments");
        args.insert(0, OsString::from("-G"));
        args.push(OsString::from(destination(&target)));
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .expect("run system OpenSSH parser");
        assert!(
            output.status.success(),
            "OpenSSH rejected ProxyCommand: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let effective = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
        assert!(effective.contains("proxycommand"));
        assert!(effective.contains("-f"));
        assert!(effective.contains("none"));
        fs::remove_dir_all(directory).expect("remove directory");
    }

    #[test]
    fn proxy_endpoint_rejects_shell_syntax_even_if_direct_ssh_accepts_it() {
        let mut host = host_profile();
        host.hostname = "jump.example&calc.exe".to_owned();
        assert!(validate_proxy_endpoint(&host).is_err());
        host.hostname = "2001:db8::1".to_owned();
        host.username = "valid-user".to_owned();
        assert!(validate_proxy_endpoint(&host).is_ok());
    }

    #[test]
    fn scan_output_from_jump_is_rewritten_for_the_target_token() {
        let key = base64::engine::general_purpose::STANDARD
            .encode(b"target host key material for proxy scan");
        let stdout = format!(
            "# target.internal:2222 SSH-2.0-server\ntarget.internal ssh-ed25519 {key}\ntarget.internal ssh-ed25519 {key}\n"
        );
        let candidates =
            parse_scan_candidates(&stdout, "[target.internal]:2222", &[]).expect("parse scan");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].host_token, "[target.internal]:2222");
        assert_eq!(
            candidates[0].raw_line,
            format!("[target.internal]:2222 ssh-ed25519 {key}")
        );
        assert!(!candidates[0].trusted);
        assert!(!candidates[0].mismatch);
    }

    #[test]
    fn unresolved_proxy_jump_cannot_fall_back_to_external_ssh_config() {
        let directory = std::env::temp_dir().join(format!("remotedeck-proxy-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create directory");
        let known_hosts = directory.join("known_hosts");
        fs::write(&known_hosts, []).expect("known hosts");
        let runtime = SshRuntime {
            ssh_path: Some(PathBuf::from("ssh")),
            sftp_path: Some(PathBuf::from("sftp")),
            keyscan_path: Some(PathBuf::from("ssh-keyscan")),
            keygen_path: Some(PathBuf::from("ssh-keygen")),
            known_hosts_path: known_hosts,
            known_hosts_lock: Arc::new(Mutex::new(())),
            repository: None,
        };
        let mut host = host_profile();
        host.proxy_jump = Some("external-alias".to_owned());
        let error = runtime
            .base_args(&host, true)
            .expect_err("unresolved proxy");
        assert!(error.to_string().contains("saved-host repository"));
        fs::remove_dir_all(directory).expect("remove directory");
    }

    #[test]
    fn changed_key_requires_explicit_removal() {
        let first_key = base64::engine::general_purpose::STANDARD.encode(b"first trusted host key");
        let replacement_key =
            base64::engine::general_purpose::STANDARD.encode(b"replacement host key");
        let trusted = vec![TrustedHostKey {
            host_token: "example.test".to_owned(),
            algorithm: "ssh-ed25519".to_owned(),
            sha256_fingerprint: fingerprint_public_key(&first_key).expect("fingerprint"),
            public_key_base64: first_key,
        }];
        let replacement = HostKeyCandidate {
            host_token: "example.test".to_owned(),
            algorithm: "ssh-ed25519".to_owned(),
            sha256_fingerprint: fingerprint_public_key(&replacement_key).expect("fingerprint"),
            public_key_base64: replacement_key,
            raw_line: String::new(),
            trusted: false,
            mismatch: true,
            previous_fingerprint: Some(trusted[0].sha256_fingerprint.clone()),
        };
        assert_eq!(
            trust_decision(&trusted, "example.test", &replacement),
            TrustDecision::Changed
        );
        assert_eq!(
            host_key_candidate_status(
                &trusted,
                "example.test",
                "ssh-ed25519",
                &replacement.public_key_base64,
            ),
            (false, true, Some(trusted[0].sha256_fingerprint.clone()))
        );
        assert_eq!(
            host_key_candidate_status(
                &trusted,
                "example.test",
                "ssh-ed25519",
                &trusted[0].public_key_base64,
            ),
            (true, false, None)
        );

        let different_algorithm = HostKeyCandidate {
            algorithm: "ecdsa-sha2-nistp256".to_owned(),
            mismatch: false,
            previous_fingerprint: None,
            ..replacement
        };
        assert_eq!(
            trust_decision(&trusted, "example.test", &different_algorithm),
            TrustDecision::FirstUse
        );
        assert_eq!(
            host_key_candidate_status(
                &trusted,
                "example.test",
                "ecdsa-sha2-nistp256",
                &different_algorithm.public_key_base64,
            ),
            (false, false, None)
        );
    }

    #[test]
    fn known_hosts_parser_rejects_comma_token_amplification() {
        let hosts = (0..=MAX_KNOWN_HOST_RECORDS)
            .map(|index| format!("host-{index}"))
            .collect::<Vec<_>>()
            .join(",");
        let key = base64::engine::general_purpose::STANDARD.encode(b"bounded host key");
        let content = format!("{hosts} ssh-ed25519 {key}\n");
        assert!(matches!(
            parse_trusted_keys(&content),
            Err(AppError::State(message)) if message.contains("exceeds")
        ));
    }

    #[tokio::test]
    async fn trusted_keys_can_be_listed_and_removed_explicitly() {
        let directory = std::env::temp_dir().join(format!("remotedeck-trust-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create temp directory");
        let known_hosts = directory.join("known_hosts");
        let ed25519_key =
            base64::engine::general_purpose::STANDARD.encode(b"trusted ed25519 host key material");
        let ecdsa_key =
            base64::engine::general_purpose::STANDARD.encode(b"trusted ecdsa host key material");
        let rsa_key =
            base64::engine::general_purpose::STANDARD.encode(b"trusted rsa host key material");
        fs::write(
            &known_hosts,
            format!(
                "# managed records\nexample.test ssh-ed25519 {ed25519_key}\nexample.test ecdsa-sha2-nistp256 {ecdsa_key}\nexample.test,alias.test ssh-rsa {rsa_key}\n"
            ),
        )
        .expect("write known_hosts");
        let runtime = SshRuntime {
            ssh_path: None,
            sftp_path: None,
            keyscan_path: None,
            keygen_path: None,
            known_hosts_path: known_hosts.clone(),
            known_hosts_lock: Arc::new(Mutex::new(())),
            repository: None,
        };

        let trusted = runtime.list_trusted_keys().await.expect("list trusted");
        assert_eq!(trusted.len(), 4);
        assert!(trusted.iter().any(|record| {
            record.host_token == "example.test"
                && record.algorithm == "ssh-ed25519"
                && record.public_key_base64 == ed25519_key
        }));
        assert!(
            runtime
                .remove_trusted_key_record("example.test", "ssh-ed25519", &ed25519_key)
                .await
                .expect("remove trusted")
        );
        let remaining = runtime
            .list_trusted_keys()
            .await
            .expect("list after exact removal");
        assert_eq!(remaining.len(), 3);
        assert!(
            remaining
                .iter()
                .any(|record| record.host_token == "example.test"
                    && record.algorithm == "ecdsa-sha2-nistp256")
        );
        assert!(remaining.iter().any(|record| {
            record.host_token == "example.test"
                && record.algorithm == "ssh-rsa"
                && record.public_key_base64 == rsa_key
        }));
        assert!(
            runtime
                .remove_trusted_key_record("example.test", "ssh-rsa", &rsa_key)
                .await
                .expect("remove one token from combined record")
        );
        let content = fs::read_to_string(&known_hosts).expect("read exact deletion result");
        assert!(content.contains("# managed records"));
        assert!(content.contains(&format!("alias.test ssh-rsa {rsa_key}")));
        assert!(!content.contains(&format!("example.test,alias.test ssh-rsa {rsa_key}")));
        assert!(
            !runtime
                .remove_trusted_key_record("example.test", "ssh-ed25519", &ed25519_key)
                .await
                .expect("idempotent exact removal")
        );
        fs::remove_dir_all(directory).expect("remove temp directory");
    }

    #[test]
    fn bounded_capture_drains_but_keeps_only_one_mebibyte() {
        let mut capture = BoundedCapture::default();
        capture.push(&vec![b'a'; OUTPUT_LIMIT - 2]);
        capture.push(b"bcdef");
        assert_eq!(capture.bytes.len(), OUTPUT_LIMIT);
        assert!(capture.truncated);
        let text = captured_text(&capture);
        assert_eq!(text.len(), OUTPUT_LIMIT);
        assert!(text.ends_with(OUTPUT_TRUNCATED_MARKER));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cancellation_reaps_the_owned_child_before_returning() {
        let marker = std::env::temp_dir().join(format!("remotedeck-cancel-{}.txt", Uuid::new_v4()));
        let script = format!(
            "Set-Content -LiteralPath '{}' -Value $PID; Start-Sleep -Seconds 30",
            marker.display()
        );
        let program = PathBuf::from("powershell.exe");
        let args = vec![
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-Command"),
            OsString::from(script),
        ];
        let cancellation = Arc::new(ChildCancellation::default());
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            run_output_with_input_cancellable(
                &program,
                &args,
                Duration::from_secs(60),
                None,
                Some(task_cancellation.as_ref()),
            )
            .await
        });
        let marker_deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < marker_deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(marker.exists(), "test child must start before cancellation");
        cancellation.cancel();
        let result = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("cancellation cleanup deadline")
            .expect("cancellation task");
        assert!(matches!(result, Err(AppError::Cancelled)));
        fs::remove_file(marker).expect("remove cancellation marker");
    }

    fn host_profile() -> HostProfile {
        let now = chrono::Utc::now();
        HostProfile {
            schema_version: 2,
            id: Uuid::new_v4().to_string(),
            alias: "test".to_owned(),
            hostname: "example.test".to_owned(),
            port: 22,
            username: "tester".to_owned(),
            auth_method: crate::model::AuthMethod::Interactive,
            identity_file: None,
            proxy_jump: None,
            default_workspace: "~".to_owned(),
            groups: Vec::new(),
            advanced: crate::model::SshAdvancedOptions::default(),
            monitor_enabled: true,
            created_at: now,
            updated_at: now,
        }
    }
}
