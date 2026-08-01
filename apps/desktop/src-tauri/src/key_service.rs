use crate::{
    error::{AppError, AppResult},
    keys::{
        OpenSshHostCandidate, canonical_ed25519_public_key, ed25519_sha256_fingerprint,
        parse_authorized_key, parse_open_ssh_config,
    },
    model::{AuthMethod, HostDraft, HostProfile, SshAdvancedPatch},
    ssh::SshRuntime,
    store::AppRepository,
};
use serde::{Deserialize, Serialize};
use std::{
    env,
    ffi::OsString,
    fs,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{Arc, OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use uuid::Uuid;

#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::MoveFileW;
#[cfg(windows)]
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

const MAX_KEYGEN_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_PUBLIC_KEY_BYTES: usize = 64 * 1024;
const MAX_OPENSSH_CONFIG_BYTES: usize = 4 * 1024 * 1024;
const MAX_PRIVATE_KEYS: usize = 64;
const MAX_KEY_DIRECTORY_ENTRIES: usize = 512;
const MAX_CONCURRENT_KEY_OPERATIONS: usize = 2;
const KEYGEN_INSPECTION_TIMEOUT: Duration = Duration::from_secs(5);
const KEYGEN_GENERATION_TIMEOUT: Duration = Duration::from_secs(30);
const KEY_LIST_TIMEOUT: Duration = Duration::from_secs(30);
const KEYGEN_POLL_INTERVAL: Duration = Duration::from_millis(20);
const KEYGEN_REAP_TIMEOUT: Duration = Duration::from_secs(2);

static KEY_OPERATION_LIMIT: OnceLock<Arc<Semaphore>> = OnceLock::new();

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivateKeyRecord {
    pub path: String,
    pub algorithm: String,
    pub fingerprint: Option<String>,
    pub encrypted: bool,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyGenerateRequest {
    pub private_key_path: String,
    #[serde(default)]
    pub comment: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyDeployRequest {
    pub host_id: String,
    pub private_key_path: String,
    pub make_default: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyOperationResult {
    pub success: bool,
    pub message: String,
    pub private_key_path: Option<String>,
    pub public_key_path: Option<String>,
    pub fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshImportResult {
    pub imported: Vec<HostProfile>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
}

pub async fn list_private_keys() -> AppResult<Vec<PrivateKeyRecord>> {
    run_bounded_key_operation(list_private_keys_blocking).await
}

fn list_private_keys_blocking() -> AppResult<Vec<PrivateKeyRecord>> {
    let Some(home) = env::var_os("USERPROFILE").map(PathBuf::from) else {
        return Ok(Vec::new());
    };
    let directory = home.join(".ssh");
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let keygen = find_keygen()?;
    let mut keys = Vec::new();
    let deadline = Instant::now() + KEY_LIST_TIMEOUT;
    for entry in fs::read_dir(directory)?.take(MAX_KEY_DIRECTORY_ENTRIES) {
        if keys.len() >= MAX_PRIVATE_KEYS || Instant::now() >= deadline {
            break;
        }
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("pub")
            || !path.is_file()
        {
            continue;
        }
        let Ok(public) = read_bounded_utf8(&path, MAX_PUBLIC_KEY_BYTES) else {
            continue;
        };
        let Ok(canonical) = canonical_ed25519_public_key(&public) else {
            continue;
        };
        let private = path.with_extension("");
        if !private.is_file() {
            continue;
        }
        let parsed = parse_authorized_key(&canonical)
            .ok_or_else(|| AppError::State("validated public key disappeared".to_owned()))?;
        let mut command = Command::new(&keygen);
        command.args(["-y", "-P", "", "-f"]).arg(&private);
        let output = run_owned_child(
            &mut command,
            KEYGEN_INSPECTION_TIMEOUT.min(deadline.saturating_duration_since(Instant::now())),
            false,
        );
        keys.push(PrivateKeyRecord {
            path: private.to_string_lossy().into_owned(),
            algorithm: parsed.algorithm,
            fingerprint: ed25519_sha256_fingerprint(&canonical).ok(),
            encrypted: output.is_err() || output.is_ok_and(|output| !output.status.success()),
            comment: (!parsed.comment.is_empty()).then_some(parsed.comment),
        });
    }
    keys.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(keys)
}

pub async fn generate_key(request: KeyGenerateRequest) -> AppResult<KeyOperationResult> {
    run_bounded_key_operation(move || generate_key_blocking(request)).await
}

fn generate_key_blocking(request: KeyGenerateRequest) -> AppResult<KeyOperationResult> {
    validate_key_destination(&request.private_key_path)?;
    let private = PathBuf::from(&request.private_key_path);
    let public = public_key_path(&private);
    if path_occupied(&private) || path_occupied(&public) {
        return Err(AppError::Validation(
            "refusing to overwrite an existing key file".to_owned(),
        ));
    }
    let parent = private.parent().ok_or_else(|| {
        AppError::Validation("private key path must have an absolute parent".to_owned())
    })?;
    fs::create_dir_all(parent)?;
    let workspace = KeyGenerationWorkspace::create(parent)?;
    let comment = crate::keys::clean_key_comment(&request.comment);
    let keygen = find_keygen()?;
    let mut generation = Command::new(&keygen);
    generation
        .args(["-q", "-t", "ed25519", "-a", "64", "-N", "", "-C"])
        .arg(&comment)
        .arg("-f")
        .arg(&workspace.private);
    let generated = run_owned_child(&mut generation, KEYGEN_GENERATION_TIMEOUT, false)?;
    if !generated.status.success() {
        return Err(AppError::Process(format!(
            "ssh-keygen failed with {}",
            generated.status
        )));
    }
    let canonical =
        canonical_ed25519_public_key(&read_bounded_utf8(&workspace.public, MAX_PUBLIC_KEY_BYTES)?)
            .map_err(|error| AppError::Process(error.to_string()))?;
    let mut verification = Command::new(&keygen);
    verification
        .args(["-y", "-P", "", "-f"])
        .arg(&workspace.private);
    let derived = run_owned_child(&mut verification, KEYGEN_GENERATION_TIMEOUT, true)?;
    if derived.truncated {
        return Err(AppError::Process(
            "ssh-keygen verification output exceeded 64 KiB".to_owned(),
        ));
    }
    if !derived.status.success() {
        return Err(AppError::Process(
            "ssh-keygen could not verify the generated private key".to_owned(),
        ));
    }
    let derived = String::from_utf8(derived.stdout)
        .map_err(|_| AppError::Process("ssh-keygen returned non-UTF-8 output".to_owned()))?;
    let derived = canonical_ed25519_public_key(&derived)
        .map_err(|error| AppError::Process(error.to_string()))?;
    if !same_public_key(&canonical, &derived) {
        return Err(AppError::Process(
            "generated private and public keys do not match".to_owned(),
        ));
    }
    let fingerprint = ed25519_sha256_fingerprint(&canonical)
        .map_err(|error| AppError::Process(error.to_string()))?;
    commit_key_pair(&workspace, &private, &public)?;
    Ok(KeyOperationResult {
        success: true,
        message: "已生成 Ed25519 密钥。该自动化流程不会接收或保存口令；如需加密，请在终端中运行 ssh-keygen -p。".to_owned(),
        private_key_path: Some(private.to_string_lossy().into_owned()),
        public_key_path: Some(public.to_string_lossy().into_owned()),
        fingerprint: Some(fingerprint),
    })
}

async fn run_bounded_key_operation<T, F>(operation: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> AppResult<T> + Send + 'static,
{
    let semaphore = Arc::clone(
        KEY_OPERATION_LIMIT.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_KEY_OPERATIONS))),
    );
    let permit = semaphore
        .acquire_owned()
        .await
        .map_err(|_| AppError::State("key operation limiter is unavailable".to_owned()))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        operation()
    })
    .await
    .map_err(|error| AppError::State(format!("key operation worker failed: {error}")))?
}

struct OwnedProcessOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    truncated: bool,
}

type BoundedReaderResult = std::io::Result<(Vec<u8>, bool)>;

struct BoundedReader {
    receiver: mpsc::Receiver<BoundedReaderResult>,
    thread: Option<thread::JoinHandle<()>>,
}

fn run_owned_child(
    command: &mut Command,
    timeout: Duration,
    capture_output: bool,
) -> AppResult<OwnedProcessOutput> {
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    command.stdin(Stdio::null());
    if capture_output {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let mut child = command.spawn()?;
    let stdout_reader = if capture_output {
        child.stdout.take().map(spawn_bounded_reader).transpose()
    } else {
        Ok(None)
    };
    let stdout_reader = match stdout_reader {
        Ok(reader) => reader,
        Err(error) => {
            return match kill_and_reap(&mut child) {
                Ok(_) => Err(error),
                Err(cleanup) => Err(AppError::Process(format!(
                    "failed to start ssh-keygen output reader: {error}; cleanup also failed: {cleanup}"
                ))),
            };
        }
    };
    let stderr_reader = if capture_output {
        child.stderr.take().map(spawn_bounded_reader).transpose()
    } else {
        Ok(None)
    };
    let stderr_reader = match stderr_reader {
        Ok(reader) => reader,
        Err(error) => {
            let cleanup = kill_and_reap(&mut child);
            if let Some(reader) = stdout_reader {
                let _ = join_bounded_reader(Some(reader), KEYGEN_REAP_TIMEOUT);
            }
            return match cleanup {
                Ok(_) => Err(error),
                Err(cleanup) => Err(AppError::Process(format!(
                    "failed to start ssh-keygen error reader: {error}; cleanup also failed: {cleanup}"
                ))),
            };
        }
    };

    let status = wait_owned_child(&mut child, timeout);
    let stdout = join_bounded_reader(stdout_reader, KEYGEN_REAP_TIMEOUT);
    let stderr = join_bounded_reader(stderr_reader, KEYGEN_REAP_TIMEOUT);
    let (stdout, stdout_truncated) = stdout?;
    let (_, stderr_truncated) = stderr?;
    Ok(OwnedProcessOutput {
        status: status?,
        stdout,
        truncated: stdout_truncated || stderr_truncated,
    })
}

fn spawn_bounded_reader<R>(reader: R) -> AppResult<BoundedReader>
where
    R: Read + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    let thread = thread::Builder::new()
        .name("remotedeck-keygen-output".to_owned())
        .spawn(move || {
            let _ = sender.send(drain_bounded(reader, MAX_KEYGEN_OUTPUT_BYTES));
        })
        .map_err(AppError::Io)?;
    Ok(BoundedReader {
        receiver,
        thread: Some(thread),
    })
}

fn join_bounded_reader(
    reader: Option<BoundedReader>,
    timeout: Duration,
) -> AppResult<(Vec<u8>, bool)> {
    let Some(mut reader) = reader else {
        return Ok((Vec::new(), false));
    };
    let result = reader.receiver.recv_timeout(timeout).map_err(|error| {
        AppError::Process(match error {
            mpsc::RecvTimeoutError::Timeout => {
                "ssh-keygen output reader did not stop before the bounded cleanup deadline"
                    .to_owned()
            }
            mpsc::RecvTimeoutError::Disconnected => {
                "ssh-keygen output reader stopped without a result".to_owned()
            }
        })
    })?;
    if let Some(thread) = reader.thread.take() {
        thread
            .join()
            .map_err(|_| AppError::Process("ssh-keygen output reader failed".to_owned()))?;
    }
    result.map_err(AppError::Io)
}

fn drain_bounded<R: Read>(mut reader: R, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut retained = Vec::with_capacity(limit.min(8 * 1024));
    let mut buffer = [0_u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(retained.len());
        let keep = remaining.min(read);
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    Ok((retained, truncated))
}

fn wait_owned_child(child: &mut std::process::Child, timeout: Duration) -> AppResult<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(KEYGEN_POLL_INTERVAL),
            Ok(None) => {
                return match kill_and_reap(child) {
                    Ok(_) => Err(AppError::Timeout("ssh-keygen timed out".to_owned())),
                    Err(cleanup) => Err(AppError::Process(format!(
                        "ssh-keygen timed out and cleanup failed: {cleanup}"
                    ))),
                };
            }
            Err(error) => {
                let cleanup = kill_and_reap(child);
                return Err(match cleanup {
                    Ok(_) => AppError::Process(format!(
                        "failed to inspect ssh-keygen before it was terminated: {error}"
                    )),
                    Err(cleanup) => AppError::Process(format!(
                        "failed to inspect ssh-keygen: {error}; cleanup also failed: {cleanup}"
                    )),
                });
            }
        }
    }
}

trait OwnedChildControl {
    fn kill_owned(&mut self) -> std::io::Result<()>;
    fn try_wait_owned(&mut self) -> std::io::Result<Option<ExitStatus>>;
}

impl OwnedChildControl for std::process::Child {
    fn kill_owned(&mut self) -> std::io::Result<()> {
        self.kill()
    }

    fn try_wait_owned(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.try_wait()
    }
}

fn kill_and_reap(child: &mut std::process::Child) -> Result<ExitStatus, String> {
    kill_and_reap_bounded(child, KEYGEN_REAP_TIMEOUT, KEYGEN_POLL_INTERVAL)
}

fn kill_and_reap_bounded<C: OwnedChildControl>(
    child: &mut C,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<ExitStatus, String> {
    let kill_error = child.kill_owned().err();
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait_owned() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(poll_interval),
            Ok(None) => {
                return Err(match kill_error {
                    Some(error) => format!(
                        "failed to terminate ssh-keygen: {error}; it did not exit before the bounded reap deadline"
                    ),
                    None => "ssh-keygen accepted termination but did not exit before the bounded reap deadline".to_owned(),
                });
            }
            Err(wait_error) => {
                return Err(match kill_error {
                    Some(kill_error) => format!(
                        "failed to terminate ssh-keygen: {kill_error}; failed to poll it for reap: {wait_error}"
                    ),
                    None => format!("failed to poll ssh-keygen for reap: {wait_error}"),
                });
            }
        }
    }
}

struct KeyGenerationWorkspace {
    directory: PathBuf,
    private: PathBuf,
    public: PathBuf,
}

impl KeyGenerationWorkspace {
    fn create(parent: &Path) -> AppResult<Self> {
        for _ in 0..16 {
            let directory = parent.join(format!(".remotedeck-keygen-{}", Uuid::new_v4()));
            match fs::create_dir(&directory) {
                Ok(()) => {
                    return Ok(Self {
                        private: directory.join("key"),
                        public: directory.join("key.pub"),
                        directory,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(AppError::Io(error)),
            }
        }
        Err(AppError::State(
            "could not reserve an app-owned key generation workspace".to_owned(),
        ))
    }
}

impl Drop for KeyGenerationWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.private);
        let _ = fs::remove_file(&self.public);
        let _ = fs::remove_dir(&self.directory);
    }
}

fn commit_key_pair(
    workspace: &KeyGenerationWorkspace,
    private: &Path,
    public: &Path,
) -> AppResult<()> {
    require_regular_file(&workspace.private)?;
    require_regular_file(&workspace.public)?;
    atomic_move_no_clobber(&workspace.public, public)?;
    if let Err(error) = atomic_move_no_clobber(&workspace.private, private) {
        return Err(AppError::Process(format!(
            "private-key commit failed after the public key was safely committed at {}; the public key was retained to avoid deleting a path another process could have replaced: {error}",
            public.display()
        )));
    }
    Ok(())
}

fn require_regular_file(path: &Path) -> AppResult<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(AppError::Validation(format!(
            "generated key path is not a regular file: {}",
            path.display()
        )));
    }
    Ok(())
}

fn atomic_move_no_clobber(source: &Path, destination: &Path) -> AppResult<()> {
    if path_occupied(destination) {
        return Err(AppError::Validation(format!(
            "refusing to overwrite existing path {}",
            destination.display()
        )));
    }
    #[cfg(windows)]
    let result = {
        let source = source
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let destination = destination
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: both buffers are NUL-terminated and remain valid for the duration of the call.
        if unsafe { MoveFileW(source.as_ptr(), destination.as_ptr()) } == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    };
    #[cfg(not(windows))]
    let result = fs::hard_link(source, destination);
    match result {
        Ok(()) => Ok(()),
        Err(_) if path_occupied(destination) => Err(AppError::Validation(format!(
            "refusing to overwrite existing path {}",
            destination.display()
        ))),
        Err(error) => Err(AppError::Io(error)),
    }
}

fn public_key_path(private: &Path) -> PathBuf {
    let mut value = private.as_os_str().to_os_string();
    value.push(".pub");
    PathBuf::from(value)
}

fn path_occupied(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

pub async fn deploy_key(
    repository: &AppRepository,
    runtime: &SshRuntime,
    request: KeyDeployRequest,
) -> AppResult<KeyOperationResult> {
    validate_local_path(&request.private_key_path)?;
    let private = PathBuf::from(&request.private_key_path);
    let public = public_key_path(&private);
    if !private.is_file() || !public.is_file() {
        return Err(AppError::NotFound(
            "private key and matching .pub file are required".to_owned(),
        ));
    }
    let canonical =
        canonical_ed25519_public_key(&read_bounded_utf8(&public, MAX_PUBLIC_KEY_BYTES)?)
            .map_err(|error| AppError::Validation(error.to_string()))?;
    let parsed = parse_authorized_key(&canonical)
        .ok_or_else(|| AppError::Validation("public key is invalid".to_owned()))?;
    let material = format!("{} {}", parsed.algorithm, parsed.public_key_base64);
    let line = shell_quote(&canonical);
    let material = shell_quote(&material);
    let command = format!(
        "set -eu; umask 077; mkdir -p \"$HOME/.ssh\"; chmod 700 \"$HOME/.ssh\"; \
         file=\"$HOME/.ssh/authorized_keys\"; touch \"$file\"; chmod 600 \"$file\"; \
         if ! awk '{{print $1 \" \" $2}}' \"$file\" | grep -Fqx -- {material}; then \
           tmp=\"$HOME/.ssh/.remotedeck-authorized-keys-$$\"; \
           trap 'rm -f -- \"$tmp\"' EXIT HUP INT TERM; \
           cat \"$file\" >\"$tmp\"; printf '%s\\n' {line} >>\"$tmp\"; chmod 600 \"$tmp\"; mv -f -- \"$tmp\" \"$file\"; trap - EXIT; \
         fi"
    );
    let host = repository.host(&request.host_id)?;
    let deployed = runtime.run_command(&host, command, None).await?;
    if deployed.exit_code != Some(0) {
        return Err(AppError::Process(if deployed.stderr.trim().is_empty() {
            "authorized_keys deployment failed".to_owned()
        } else {
            deployed.stderr
        }));
    }
    let mut verification_host = host.clone();
    verification_host.identity_file = Some(private.to_string_lossy().into_owned());
    verification_host.advanced.identities_only = true;
    let verification = runtime.test_connection(&verification_host).await?;
    if !verification.success {
        return Err(AppError::Process(
            verification
                .error
                .unwrap_or_else(|| "fresh key verification failed".to_owned()),
        ));
    }
    if request.make_default {
        repository.save_host(HostDraft {
            id: Some(host.id),
            alias: host.alias,
            hostname: host.hostname,
            port: host.port,
            username: host.username,
            auth_method: Some(AuthMethod::PrivateKey),
            identity_file: verification_host.identity_file.clone(),
            proxy_jump: host.proxy_jump,
            default_workspace: Some(host.default_workspace),
            groups: host.groups,
            advanced: Some(SshAdvancedPatch {
                identities_only: Some(true),
                ..SshAdvancedPatch::default()
            }),
            monitor_enabled: Some(host.monitor_enabled),
        })?;
    }
    Ok(KeyOperationResult {
        success: true,
        message: "公钥已幂等部署并通过新密钥连接复验。".to_owned(),
        private_key_path: Some(private.to_string_lossy().into_owned()),
        public_key_path: Some(public.to_string_lossy().into_owned()),
        fingerprint: Some(
            ed25519_sha256_fingerprint(&canonical)
                .map_err(|error| AppError::Process(error.to_string()))?,
        ),
    })
}

pub fn import_ssh_config(
    repository: &AppRepository,
    config_path: &str,
) -> AppResult<SshImportResult> {
    validate_local_path(config_path)?;
    let metadata = fs::metadata(config_path)?;
    if !metadata.is_file() || metadata.len() > MAX_OPENSSH_CONFIG_BYTES as u64 {
        return Err(AppError::Validation(
            "OpenSSH config must be a file no larger than 4 MiB".to_owned(),
        ));
    }
    let candidates = parse_open_ssh_config(&read_bounded_utf8(
        Path::new(config_path),
        MAX_OPENSSH_CONFIG_BYTES,
    )?)
    .map_err(|error| AppError::Validation(error.to_string()))?;
    import_candidates(repository, candidates)
}

fn read_bounded_utf8(path: &Path, limit: usize) -> AppResult<String> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(AppError::Validation(format!(
            "{} exceeds the {limit}-byte safety limit",
            path.display()
        )));
    }
    String::from_utf8(bytes)
        .map_err(|_| AppError::Validation(format!("{} is not valid UTF-8", path.display())))
}

fn import_candidates(
    repository: &AppRepository,
    candidates: Vec<OpenSshHostCandidate>,
) -> AppResult<SshImportResult> {
    let existing = repository.snapshot().hosts;
    let mut drafts = Vec::new();
    let mut skipped = Vec::new();
    let mut warnings = Vec::new();
    for candidate in candidates {
        if existing
            .iter()
            .any(|host| host.alias.eq_ignore_ascii_case(&candidate.alias))
            || drafts
                .iter()
                .any(|draft: &HostDraft| draft.alias.eq_ignore_ascii_case(&candidate.alias))
        {
            skipped.push(candidate.alias);
            continue;
        }
        if candidate.unsupported_count > 0 {
            warnings.push(format!(
                "{}: {} 条不支持的指令仅保留在原配置中。",
                candidate.alias, candidate.unsupported_count
            ));
        }
        if candidate.local_forward_count > 0 || candidate.remote_forward_count > 0 {
            warnings.push(format!(
                "{}: 转发规则需在隧道面板逐条确认后创建。",
                candidate.alias
            ));
        }
        let username = candidate
            .username
            .or_else(|| env::var("USERNAME").ok())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                AppError::Validation(format!("{} has no SSH User directive", candidate.alias))
            })?;
        let advanced = SshAdvancedPatch {
            connect_timeout_seconds: candidate.connect_timeout_seconds,
            server_alive_interval_seconds: candidate.server_alive_interval_seconds,
            server_alive_count_max: candidate.server_alive_count_max,
            tcp_keep_alive: candidate.tcp_keep_alive,
            compression: candidate.compression,
            identities_only: candidate.identities_only,
        };
        let auth_method = if candidate.identity_file.is_some() {
            AuthMethod::PrivateKey
        } else {
            AuthMethod::Interactive
        };
        drafts.push(HostDraft {
            id: None,
            alias: candidate.alias,
            hostname: candidate.hostname,
            port: candidate.port,
            username,
            auth_method: Some(auth_method),
            identity_file: candidate.identity_file,
            proxy_jump: candidate.proxy_jump,
            default_workspace: Some("~".to_owned()),
            groups: vec!["openssh-import".to_owned()],
            advanced: Some(advanced),
            monitor_enabled: Some(true),
        });
    }
    let imported = repository.import_hosts(drafts)?;
    Ok(SshImportResult {
        imported,
        skipped,
        warnings,
    })
}

fn find_keygen() -> AppResult<PathBuf> {
    keygen_candidates(system_directory(), env::var_os("PATH"), cfg!(windows))
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| AppError::MissingExecutable("OpenSSH ssh-keygen.exe".to_owned()))
}

fn keygen_candidates(
    system_directory: Option<PathBuf>,
    path: Option<OsString>,
    is_windows: bool,
) -> Vec<PathBuf> {
    if is_windows {
        return system_directory
            .map(|directory| directory.join("OpenSSH").join("ssh-keygen.exe"))
            .into_iter()
            .collect();
    }
    path.map(|path| {
        env::split_paths(&path)
            .map(|directory| directory.join("ssh-keygen"))
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(windows)]
fn system_directory() -> Option<PathBuf> {
    let mut buffer = vec![0_u16; 32_768];
    // SAFETY: `buffer` is writable for its declared length and remains alive for the call.
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if length == 0 || length >= buffer.len() {
        return None;
    }
    buffer.truncate(length);
    Some(PathBuf::from(OsString::from_wide(&buffer)))
}

#[cfg(not(windows))]
fn system_directory() -> Option<PathBuf> {
    None
}

fn validate_local_path(value: &str) -> AppResult<()> {
    if value.trim().is_empty() || value.len() > 32_767 || value.contains('\0') {
        return Err(AppError::Validation("local path is invalid".to_owned()));
    }
    Ok(())
}

fn validate_key_destination(value: &str) -> AppResult<()> {
    validate_local_path(value)?;
    let path = Path::new(value);
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .parent()
            .is_none_or(|parent| parent.as_os_str().is_empty())
    {
        return Err(AppError::Validation(
            "private key destination must be an absolute file path".to_owned(),
        ));
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn same_public_key(left: &str, right: &str) -> bool {
    let Some(left) = parse_authorized_key(left) else {
        return false;
    };
    let Some(right) = parse_authorized_key(right) else {
        return false;
    };
    left.algorithm == right.algorithm && left.public_key_base64 == right.public_key_base64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
    };

    struct KillFailingChild {
        polls: usize,
    }

    struct SlowReader;

    impl Read for SlowReader {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            thread::sleep(Duration::from_millis(200));
            Ok(0)
        }
    }

    impl OwnedChildControl for KillFailingChild {
        fn kill_owned(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "synthetic kill failure",
            ))
        }

        fn try_wait_owned(&mut self) -> std::io::Result<Option<ExitStatus>> {
            self.polls += 1;
            Ok(None)
        }
    }

    #[test]
    fn config_import_skips_duplicate_aliases() {
        let directory = env::temp_dir().join(format!("remotedeck-keys-{}", Uuid::new_v4()));
        let repository = AppRepository::open(directory.clone()).expect("repository");
        repository
            .save_host(HostDraft {
                id: None,
                alias: "lab".to_owned(),
                hostname: "old".to_owned(),
                port: 22,
                username: "user".to_owned(),
                auth_method: Some(AuthMethod::Interactive),
                identity_file: None,
                proxy_jump: None,
                default_workspace: None,
                groups: Vec::new(),
                advanced: None,
                monitor_enabled: None,
            })
            .expect("host");
        let result = import_candidates(
            &repository,
            parse_open_ssh_config("Host lab new\n  HostName example.test\n  User alice\n")
                .expect("parse"),
        )
        .expect("import");
        assert_eq!(result.skipped, vec!["lab"]);
        assert_eq!(result.imported[0].alias, "new");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn config_import_resolves_a_later_proxy_jump_atomically() {
        let directory = env::temp_dir().join(format!("remotedeck-proxy-import-{}", Uuid::new_v4()));
        let repository = AppRepository::open(directory.clone()).expect("repository");
        let candidates = parse_open_ssh_config(
            "Host target\n  HostName target.internal\n  User target-user\n  ProxyJump jump\n\nHost jump\n  HostName jump.example\n  User jump-user\n  Port 2200\n",
        )
        .expect("parse");
        let result = import_candidates(&repository, candidates).expect("import");
        let target = result
            .imported
            .iter()
            .find(|host| host.alias == "target")
            .expect("target");
        let jump = result
            .imported
            .iter()
            .find(|host| host.alias == "jump")
            .expect("jump");
        assert_eq!(target.proxy_jump.as_deref(), Some(jump.id.as_str()));
        assert_eq!(repository.snapshot().hosts.len(), 2);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn shell_quote_handles_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn public_key_comparison_ignores_comments_but_not_material() {
        use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};

        fn line(seed: u8, comment: &str) -> String {
            let mut blob = Vec::new();
            blob.extend_from_slice(&11_u32.to_be_bytes());
            blob.extend_from_slice(b"ssh-ed25519");
            blob.extend_from_slice(&32_u32.to_be_bytes());
            blob.extend_from_slice(&[seed; 32]);
            format!("ssh-ed25519 {} {comment}", STANDARD_NO_PAD.encode(blob))
        }

        assert!(same_public_key(&line(7, "first"), &line(7, "second")));
        assert!(!same_public_key(&line(7, "same"), &line(8, "same")));
    }

    #[test]
    fn windows_keygen_selection_ignores_path_candidates() {
        let system = PathBuf::from(r"C:\Windows\System32");
        let path = env::join_paths([
            PathBuf::from(r"C:\attacker"),
            PathBuf::from(r"D:\portable-openssh"),
        ])
        .expect("PATH");
        assert_eq!(
            keygen_candidates(Some(system), Some(path), true),
            [PathBuf::from(r"C:\Windows\System32\OpenSSH\ssh-keygen.exe")]
        );
    }

    #[test]
    fn no_clobber_commit_preserves_existing_destination_and_owned_source() {
        let directory = env::temp_dir().join(format!("remotedeck-key-commit-{}", Uuid::new_v4()));
        fs::create_dir(&directory).expect("test directory");
        let source = directory.join("source");
        let destination = directory.join("destination");
        fs::write(&source, b"generated").expect("source");
        fs::write(&destination, b"existing").expect("destination");

        assert!(atomic_move_no_clobber(&source, &destination).is_err());
        assert_eq!(fs::read(&destination).expect("destination"), b"existing");
        assert_eq!(fs::read(&source).expect("source"), b"generated");
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn keygen_output_drain_retains_only_the_configured_budget() {
        let input = vec![b'x'; MAX_KEYGEN_OUTPUT_BYTES + 1024];
        let (output, truncated) =
            drain_bounded(std::io::Cursor::new(input), MAX_KEYGEN_OUTPUT_BYTES).expect("drain");
        assert_eq!(output.len(), MAX_KEYGEN_OUTPUT_BYTES);
        assert!(truncated);
    }

    #[test]
    fn kill_failure_uses_a_bounded_reap_deadline() {
        let mut child = KillFailingChild { polls: 0 };
        let started = Instant::now();
        let error = kill_and_reap_bounded(
            &mut child,
            Duration::from_millis(25),
            Duration::from_millis(1),
        )
        .expect_err("cleanup must fail explicitly");

        assert!(error.contains("synthetic kill failure"));
        assert!(error.contains("bounded reap deadline"));
        assert!(child.polls > 1);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn stalled_output_reader_has_a_bounded_join_deadline() {
        let reader = spawn_bounded_reader(SlowReader).expect("reader thread");
        let started = Instant::now();
        let error = join_bounded_reader(Some(reader), Duration::from_millis(25))
            .expect_err("reader must time out");

        assert!(error.to_string().contains("bounded cleanup deadline"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cleanup_failures_release_the_shared_operation_permit() {
        let tasks = (0..(MAX_CONCURRENT_KEY_OPERATIONS + 2))
            .map(|_| {
                tokio::spawn(async {
                    run_bounded_key_operation(|| {
                        let mut child = KillFailingChild { polls: 0 };
                        let error = kill_and_reap_bounded(
                            &mut child,
                            Duration::from_millis(10),
                            Duration::from_millis(1),
                        )
                        .expect_err("cleanup failure");
                        Err::<(), _>(AppError::Process(error))
                    })
                    .await
                })
            })
            .collect::<Vec<_>>();
        tokio::time::timeout(Duration::from_secs(2), async {
            for task in tasks {
                assert!(task.await.expect("task").is_err());
            }
        })
        .await
        .expect("all queued operations must release their permits");
    }

    #[test]
    fn concurrent_key_pair_commits_select_exactly_one_owner() {
        let directory = env::temp_dir().join(format!("remotedeck-key-race-{}", Uuid::new_v4()));
        fs::create_dir(&directory).expect("test directory");
        let private = directory.join("id_ed25519");
        let public = public_key_path(&private);
        let barrier = Arc::new(Barrier::new(3));
        let attempts = b"ab"
            .iter()
            .copied()
            .map(|marker| {
                let workspace = KeyGenerationWorkspace::create(&directory).expect("workspace");
                fs::write(&workspace.private, [marker]).expect("private");
                fs::write(&workspace.public, [marker]).expect("public");
                let private = private.clone();
                let public = public.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    commit_key_pair(&workspace, &private, &public).is_ok()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let successes = attempts
            .into_iter()
            .map(|attempt| attempt.join().expect("commit thread"))
            .filter(|success| *success)
            .count();
        assert_eq!(successes, 1);
        assert_eq!(
            fs::read(&private).expect("private"),
            fs::read(&public).expect("public")
        );
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn workspace_cleanup_never_removes_unowned_extra_files() {
        let directory = env::temp_dir().join(format!("remotedeck-key-cleanup-{}", Uuid::new_v4()));
        fs::create_dir(&directory).expect("test directory");
        let workspace = KeyGenerationWorkspace::create(&directory).expect("workspace");
        fs::write(&workspace.private, b"private").expect("private");
        fs::write(&workspace.public, b"public").expect("public");
        let workspace_directory = workspace.directory.clone();
        let foreign = workspace_directory.join("foreign");
        fs::write(&foreign, b"not-owned-by-cleanup").expect("foreign");
        drop(workspace);

        assert_eq!(
            fs::read(&foreign).expect("foreign retained"),
            b"not-owned-by-cleanup"
        );
        fs::remove_file(foreign).expect("remove foreign");
        fs::remove_dir(workspace_directory).expect("remove workspace");
        fs::remove_dir(directory).expect("remove test directory");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn blocking_key_operations_share_a_concurrency_limit() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let tasks = (0..8)
            .map(|_| {
                let active = Arc::clone(&active);
                let maximum = Arc::clone(&maximum);
                tokio::spawn(async move {
                    run_bounded_key_operation(move || {
                        let current = active.fetch_add(1, Ordering::AcqRel) + 1;
                        maximum.fetch_max(current, Ordering::AcqRel);
                        thread::sleep(Duration::from_millis(50));
                        active.fetch_sub(1, Ordering::AcqRel);
                        Ok(())
                    })
                    .await
                })
            })
            .collect::<Vec<_>>();
        for task in tasks {
            task.await.expect("task").expect("operation");
        }
        assert!(maximum.load(Ordering::Acquire) <= MAX_CONCURRENT_KEY_OPERATIONS);
    }

    #[cfg(windows)]
    #[test]
    fn owned_child_timeout_kills_and_reaps_before_returning() {
        let powershell = system_directory()
            .expect("System32")
            .join(r"WindowsPowerShell\v1.0\powershell.exe");
        let mut command = Command::new(powershell);
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 30",
        ]);
        let started = Instant::now();
        let result = run_owned_child(&mut command, Duration::from_millis(100), false);
        assert!(matches!(result, Err(AppError::Timeout(_))));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn real_system_keygen_commits_a_verified_pair_without_clobbering() {
        let directory = env::temp_dir().join(format!("remotedeck-keygen-e2e-{}", Uuid::new_v4()));
        fs::create_dir(&directory).expect("test directory");
        let private = directory.join("id_ed25519");
        let request = KeyGenerateRequest {
            private_key_path: private.to_string_lossy().into_owned(),
            comment: "RemoteDeck test".to_owned(),
        };
        let result = generate_key(request.clone()).await.expect("generate key");
        assert!(result.success);
        let public = public_key_path(&private);
        let original_private = fs::read(&private).expect("private key");
        let original_public = fs::read(&public).expect("public key");

        assert!(generate_key(request).await.is_err());
        assert_eq!(fs::read(&private).expect("private key"), original_private);
        assert_eq!(fs::read(&public).expect("public key"), original_public);
        fs::remove_dir_all(directory).expect("cleanup");
    }
}
