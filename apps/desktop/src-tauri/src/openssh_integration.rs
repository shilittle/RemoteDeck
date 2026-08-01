use crate::{
    model::{
        AuthMethod, HostDraft, HostKeyCandidate, HostProfile, SshAdvancedPatch, TunnelDirection,
        TunnelProfile,
    },
    ssh::{
        SshRuntime, captured_text, run_output_with_input,
        sftp::{
            ConflictPolicy, DownloadRequest, SftpDeleteRequest, SftpListRequest, SftpMkdirRequest,
            SftpService, TransferJob, TransferRegistry, TransferState, UploadRequest,
        },
    },
    store::AppRepository,
};
use chrono::Utc;
use std::{
    env,
    ffi::OsString,
    fs,
    io::{self, Read as _, Write as _},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tokio::{process::Child, time::sleep};
use uuid::Uuid;

const COMMAND_OUTPUT_LIMIT: usize = 1_048_576;
const COMMAND_OUTPUT_MARKER: &str = "[RemoteDeck: output truncated at 1 MiB]";

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone)]
struct FixtureConfig {
    identity: PathBuf,
    direct_port: u16,
    jump_port: u16,
    remote_forward_port: u16,
}

impl FixtureConfig {
    fn from_environment() -> TestResult<Self> {
        let identity = required_path("REMOTEDECK_INTEGRATION_IDENTITY")?;
        if !identity.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "integration identity does not exist: {}",
                    identity.display()
                ),
            )
            .into());
        }
        Ok(Self {
            identity,
            direct_port: required_port("REMOTEDECK_DIRECT_PORT")?,
            jump_port: required_port("REMOTEDECK_JUMP_PORT")?,
            remote_forward_port: required_port("REMOTEDECK_REMOTE_FORWARD_PORT")?,
        })
    }
}

struct TestWorkspace {
    root: PathBuf,
    repository: AppRepository,
}

impl TestWorkspace {
    fn new(label: &str) -> TestResult<Self> {
        let root = env::temp_dir().join(format!("remotedeck-openssh-{label}-{}", Uuid::new_v4()));
        let repository = AppRepository::open(root.clone())?;
        Ok(Self { root, repository })
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the Windows system OpenSSH client"]
async fn generated_proxy_command_is_accepted_by_system_ssh_g() -> TestResult {
    let workspace = TestWorkspace::new("ssh-g")?;
    let mut jump_draft = host_draft(
        "ssh-g-jump",
        "127.0.0.1",
        22222,
        Path::new("unused-in-ssh-g"),
        None,
    );
    jump_draft.auth_method = Some(AuthMethod::Agent);
    jump_draft.identity_file = None;
    let jump = workspace.repository.save_host(jump_draft)?;
    let mut target_draft = host_draft(
        "ssh-g-target",
        "target",
        22,
        Path::new("unused-in-ssh-g"),
        Some(jump.alias),
    );
    target_draft.auth_method = Some(AuthMethod::Agent);
    target_draft.identity_file = None;
    let target = workspace.repository.save_host(target_draft)?;
    let runtime = SshRuntime::discover_with_repository(workspace.repository.clone());

    let (program, mut arguments) =
        runtime.remote_command_spec(&target, "printf ssh-g-ok".to_owned(), true)?;
    let remote_command = arguments.pop();
    assert_eq!(
        remote_command.as_deref(),
        Some(OsString::from("printf ssh-g-ok").as_os_str())
    );
    arguments.insert(0, OsString::from("-G"));
    let output = run_output_with_input(&program, &arguments, Duration::from_secs(10), None).await?;
    let stdout = captured_text(&output.stdout);
    let stderr = captured_text(&output.stderr);
    assert!(
        output.status.success(),
        "ssh -G rejected ProxyCommand: {stderr}"
    );
    let normalized = stdout.to_ascii_lowercase();
    assert!(normalized.contains("stricthostkeychecking true"));
    assert!(normalized.contains("globalknownhostsfile nul"));
    assert!(normalized.contains("userknownhostsfile"));
    let proxy_line = normalized
        .lines()
        .find(|line| line.starts_with("proxycommand "))
        .ok_or_else(|| io::Error::other("ssh -G did not preserve ProxyCommand"))?;
    assert!(proxy_line.contains("-f"));
    assert!(proxy_line.contains("none"));
    assert!(proxy_line.contains("stricthostkeychecking=yes"));
    assert!(proxy_line.contains("userknownhostsfile="));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the disposable Docker OpenSSH fixture"]
async fn first_use_acceptance_strict_command_and_changed_key_refusal() -> TestResult {
    let config = FixtureConfig::from_environment()?;
    let workspace = TestWorkspace::new("trust")?;
    let direct = workspace.repository.save_host(host_draft(
        "direct",
        "127.0.0.1",
        config.direct_port,
        &config.identity,
        None,
    ))?;
    let other = workspace.repository.save_host(host_draft(
        "other-real-key",
        "127.0.0.1",
        config.jump_port,
        &config.identity,
        None,
    ))?;
    let runtime = SshRuntime::discover_with_repository(workspace.repository.clone());

    let before_acceptance = runtime.test_connection(&direct).await?;
    assert!(
        !before_acceptance.success,
        "StrictHostKeyChecking must reject a first-use host before acceptance"
    );
    assert!(runtime.list_trusted_keys().await?.is_empty());

    let candidate = scan_ed25519(&runtime, &direct).await?;
    assert!(!candidate.trusted);
    assert!(!candidate.mismatch);
    runtime.accept_host_key(&direct, &candidate).await?;

    let trusted = runtime.list_trusted_keys().await?;
    assert_eq!(trusted.len(), 1);
    assert_eq!(trusted[0].host_token, candidate.host_token);
    assert_eq!(trusted[0].sha256_fingerprint, candidate.sha256_fingerprint);
    let known_hosts = fs::read_to_string(workspace.repository.known_hosts_path())?;
    assert_eq!(
        known_hosts.lines().filter(|line| !line.is_empty()).count(),
        1
    );

    let after_acceptance = runtime.test_connection(&direct).await?;
    assert!(after_acceptance.success, "{after_acceptance:?}");
    let command = runtime
        .run_command(
            &direct,
            "printf 'strict-command-ok\\n'".to_owned(),
            Some("/tmp".to_owned()),
        )
        .await?;
    assert_eq!(command.exit_code, Some(0));
    assert_eq!(command.stdout, "strict-command-ok\n");

    let bounded = runtime
        .run_command(
            &direct,
            "python3 -c 'import sys; sys.stdout.write(\"x\" * 1200000)'".to_owned(),
            None,
        )
        .await?;
    assert_eq!(bounded.exit_code, Some(0));
    assert!(bounded.stdout.len() <= COMMAND_OUTPUT_LIMIT);
    assert!(bounded.stdout.ends_with(COMMAND_OUTPUT_MARKER));

    let other_candidate = scan_ed25519(&runtime, &other).await?;
    assert_ne!(
        candidate.public_key_base64, other_candidate.public_key_base64,
        "fixture endpoints must have independent host keys"
    );
    let mut changed = other_candidate;
    changed.host_token = candidate.host_token.clone();
    changed.raw_line = format!(
        "{} {} {}",
        changed.host_token, changed.algorithm, changed.public_key_base64
    );
    let error = runtime
        .accept_host_key(&direct, &changed)
        .await
        .expect_err("a replacement key must require explicit removal of the old record");
    assert!(error.to_string().to_ascii_lowercase().contains("changed"));

    let rescan = scan_ed25519(&runtime, &direct).await?;
    assert!(rescan.trusted);
    assert!(!rescan.mismatch);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the disposable Docker OpenSSH fixture"]
async fn saved_proxy_jump_scans_and_connects_to_an_unpublished_target() -> TestResult {
    let config = FixtureConfig::from_environment()?;
    let workspace = TestWorkspace::new("proxy")?;
    let jump = workspace.repository.save_host(host_draft(
        "jump",
        "127.0.0.1",
        config.jump_port,
        &config.identity,
        None,
    ))?;
    let target = workspace.repository.save_host(host_draft(
        "target",
        "target",
        22,
        &config.identity,
        Some(jump.alias.clone()),
    ))?;
    assert_eq!(target.proxy_jump.as_deref(), Some(jump.id.as_str()));

    let runtime = SshRuntime::discover_with_repository(workspace.repository.clone());
    let error = runtime
        .scan_host_keys(&target)
        .await
        .expect_err("an untrusted jump host must block target scanning");
    assert!(error.to_string().contains("explicitly trusted first"));

    let jump_candidate = scan_ed25519(&runtime, &jump).await?;
    runtime.accept_host_key(&jump, &jump_candidate).await?;
    let target_candidate = scan_ed25519(&runtime, &target).await?;
    assert_eq!(target_candidate.host_token, "target");
    assert!(!target_candidate.trusted);

    let before_target_acceptance = runtime.test_connection(&target).await?;
    assert!(
        !before_target_acceptance.success,
        "the outer target key must also be explicitly accepted"
    );
    runtime.accept_host_key(&target, &target_candidate).await?;

    assert_proxy_command_is_isolated(&runtime, &target, workspace.repository.known_hosts_path())?;
    let connected = runtime.test_connection(&target).await?;
    assert!(connected.success, "{connected:?}");
    let command = runtime
        .run_command(
            &target,
            "printf 'proxy-command-ok\\n'; hostname".to_owned(),
            None,
        )
        .await?;
    assert_eq!(command.exit_code, Some(0));
    assert!(command.stdout.starts_with("proxy-command-ok\n"));
    assert!(command.stdout.lines().any(|line| line == "target"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the disposable Docker OpenSSH fixture"]
async fn sftp_round_trips_unicode_file_names_and_content() -> TestResult {
    let config = FixtureConfig::from_environment()?;
    let workspace = TestWorkspace::new("sftp")?;
    let host = workspace.repository.save_host(host_draft(
        "sftp-direct",
        "127.0.0.1",
        config.direct_port,
        &config.identity,
        None,
    ))?;
    let runtime = SshRuntime::discover_with_repository(workspace.repository.clone());
    trust_ed25519(&runtime, &host).await?;

    let service = SftpService::new(runtime);
    let registry = TransferRegistry::new(service.clone());
    let remote_directory = format!("/tmp/remotedeck-集成-{}", Uuid::new_v4());
    service
        .mkdir(
            &host,
            SftpMkdirRequest {
                path: remote_directory.clone(),
            },
        )
        .await?;
    let remote_file = format!("{remote_directory}/上传-α-数据.txt");
    let local_source = workspace.root.join("本地-源-β.txt");
    let local_download = workspace.root.join("下载-副本-γ.txt");
    let content = "RemoteDeck UTF-8 往返 ✅\n第二行 αβγ\n".as_bytes();
    fs::write(&local_source, content)?;

    let upload = registry.start_upload(
        host.clone(),
        UploadRequest {
            local_path: path_text(&local_source)?,
            remote_path: remote_file.clone(),
            conflict_policy: ConflictPolicy::Overwrite,
            recursive: false,
        },
    )?;
    let upload = wait_for_transfer(&registry, &upload.id).await?;
    assert_completed(&upload)?;

    let listing = service
        .list(
            &host,
            SftpListRequest {
                path: remote_directory.clone(),
                show_hidden: true,
            },
        )
        .await?;
    assert!(
        listing
            .entries
            .iter()
            .any(|entry| { entry.name == "上传-α-数据.txt" && entry.path == remote_file })
    );

    let download = registry.start_download(
        host.clone(),
        DownloadRequest {
            remote_path: remote_file,
            local_path: path_text(&local_download)?,
            conflict_policy: ConflictPolicy::Overwrite,
            recursive: false,
        },
    )?;
    let download = wait_for_transfer(&registry, &download.id).await?;
    assert_completed(&download)?;
    assert_eq!(fs::read(&local_download)?, content);

    service
        .delete(
            &host,
            SftpDeleteRequest {
                path: remote_directory,
                recursive: true,
            },
        )
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the disposable Docker OpenSSH fixture"]
async fn local_and_remote_forwarding_move_real_tcp_traffic() -> TestResult {
    let config = FixtureConfig::from_environment()?;
    let workspace = TestWorkspace::new("forward")?;
    let host = workspace.repository.save_host(host_draft(
        "forward-direct",
        "127.0.0.1",
        config.direct_port,
        &config.identity,
        None,
    ))?;
    let runtime = SshRuntime::discover_with_repository(workspace.repository.clone());
    trust_ed25519(&runtime, &host).await?;

    let local_port = unused_local_port()?;
    let local_tunnel = tunnel_profile(
        &host,
        TunnelDirection::Local,
        local_port,
        "127.0.0.1",
        18080,
    );
    let mut local_child = spawn_tunnel(&runtime, &host, &local_tunnel)?;
    let local_result = wait_for_http_forward(&mut local_child, local_port).await;
    stop_child(&mut local_child).await?;
    let response = local_result?;
    assert!(response.contains("RemoteDeck OpenSSH fixture: direct"));

    let marker_server = MarkerServer::start(b"remote-forward-ok\n")?;
    let remote_tunnel = tunnel_profile(
        &host,
        TunnelDirection::Remote,
        config.remote_forward_port,
        "127.0.0.1",
        marker_server.port(),
    );
    let mut remote_child = spawn_tunnel(&runtime, &host, &remote_tunnel)?;
    let remote_result = wait_for_remote_forward(
        &runtime,
        &host,
        &mut remote_child,
        config.remote_forward_port,
    )
    .await;
    stop_child(&mut remote_child).await?;
    marker_server.stop()?;
    assert!(remote_result?.contains("remote-forward-ok"));
    Ok(())
}

fn required_path(name: &str) -> TestResult<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other(format!("{name} is required")).into())
}

fn required_port(name: &str) -> TestResult<u16> {
    let value = env::var(name).map_err(|_| io::Error::other(format!("{name} is required")))?;
    value
        .parse::<u16>()
        .map_err(|error| io::Error::other(format!("{name} is invalid: {error}")).into())
}

fn path_text(path: &Path) -> TestResult<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| io::Error::other("fixture path is not Unicode").into())
}

fn host_draft(
    alias: &str,
    hostname: &str,
    port: u16,
    identity: &Path,
    proxy_jump: Option<String>,
) -> HostDraft {
    HostDraft {
        id: None,
        alias: alias.to_owned(),
        hostname: hostname.to_owned(),
        port,
        username: "remotedeck".to_owned(),
        auth_method: Some(AuthMethod::PrivateKey),
        identity_file: Some(identity.to_string_lossy().into_owned()),
        proxy_jump,
        default_workspace: Some("/tmp".to_owned()),
        groups: vec!["integration".to_owned()],
        advanced: Some(SshAdvancedPatch {
            connect_timeout_seconds: Some(5),
            server_alive_interval_seconds: Some(2),
            server_alive_count_max: Some(2),
            identities_only: Some(true),
            ..SshAdvancedPatch::default()
        }),
        monitor_enabled: Some(false),
    }
}

async fn scan_ed25519(runtime: &SshRuntime, host: &HostProfile) -> TestResult<HostKeyCandidate> {
    runtime
        .scan_host_keys(host)
        .await?
        .into_iter()
        .find(|candidate| candidate.algorithm == "ssh-ed25519")
        .ok_or_else(|| io::Error::other("fixture did not publish an Ed25519 host key").into())
}

async fn trust_ed25519(runtime: &SshRuntime, host: &HostProfile) -> TestResult {
    let candidate = scan_ed25519(runtime, host).await?;
    runtime.accept_host_key(host, &candidate).await?;
    Ok(())
}

fn assert_proxy_command_is_isolated(
    runtime: &SshRuntime,
    target: &HostProfile,
    known_hosts: &Path,
) -> TestResult {
    let (_, arguments) =
        runtime.remote_command_spec(target, "printf proxy-spec-ok".to_owned(), true)?;
    let arguments = arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(arguments.windows(2).any(|pair| pair == ["-F", "none"]));
    let known_hosts_option = format!("UserKnownHostsFile={}", known_hosts.display());
    assert!(arguments.iter().any(|value| value == &known_hosts_option));
    assert!(
        arguments
            .iter()
            .any(|value| value == "GlobalKnownHostsFile=/dev/null")
    );
    assert!(
        arguments
            .iter()
            .any(|value| value == "StrictHostKeyChecking=yes")
    );

    let proxy = arguments
        .iter()
        .find_map(|argument| argument.strip_prefix("ProxyCommand="))
        .ok_or_else(|| io::Error::other("ProxyCommand option is missing"))?;
    assert!(proxy.contains("'-F' 'none'"));
    assert!(proxy.contains(&known_hosts_option));
    assert!(proxy.contains("GlobalKnownHostsFile=/dev/null"));
    assert!(proxy.contains("StrictHostKeyChecking=yes"));
    assert!(proxy.contains("'-W' '%h:%p'"));
    Ok(())
}

async fn wait_for_transfer(registry: &TransferRegistry, job_id: &str) -> TestResult<TransferJob> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let job = registry.get(job_id)?;
        if matches!(
            job.state,
            TransferState::Completed | TransferState::Cancelled | TransferState::Failed
        ) {
            return Ok(job);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("transfer {job_id} did not finish; state={:?}", job.state),
            )
            .into());
        }
        sleep(Duration::from_millis(50)).await;
    }
}

fn assert_completed(job: &TransferJob) -> TestResult {
    if job.state != TransferState::Completed {
        return Err(io::Error::other(format!(
            "transfer {} ended in {:?}: {}",
            job.id,
            job.state,
            job.error.as_deref().unwrap_or("no error detail")
        ))
        .into());
    }
    Ok(())
}

fn tunnel_profile(
    host: &HostProfile,
    direction: TunnelDirection,
    source_port: u16,
    target_host: &str,
    target_port: u16,
) -> TunnelProfile {
    let now = Utc::now();
    TunnelProfile {
        schema_version: 2,
        id: Uuid::new_v4().to_string(),
        host_id: host.id.clone(),
        name: format!("integration-{direction:?}"),
        direction,
        bind_address: "127.0.0.1".to_owned(),
        source_port,
        target_host: target_host.to_owned(),
        target_port,
        auto_start: false,
        auto_reconnect: false,
        health_check: None,
        created_at: now,
        updated_at: now,
    }
}

fn spawn_tunnel(
    runtime: &SshRuntime,
    host: &HostProfile,
    tunnel: &TunnelProfile,
) -> TestResult<Child> {
    let (program, arguments) = runtime.tunnel_command(host, tunnel)?;
    Ok(tokio::process::Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?)
}

async fn stop_child(child: &mut Child) -> TestResult {
    if child.try_wait()?.is_none() {
        child.start_kill()?;
    }
    tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "tunnel child did not exit"))??;
    Ok(())
}

fn unused_local_port() -> TestResult<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(listener.local_addr()?.port())
}

async fn wait_for_http_forward(child: &mut Child, port: u16) -> TestResult<String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "local-forward process exited early with {status}"
            ))
            .into());
        }
        let probe = tokio::task::spawn_blocking(move || http_probe(port)).await??;
        if let Some(response) = probe {
            return Ok(response);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "local forward did not become ready",
            )
            .into());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

fn http_probe(port: u16) -> io::Result<Option<String>> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = match TcpStream::connect_timeout(&address, Duration::from_millis(250)) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::TimedOut
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"GET / HTTP/1.0\r\nHost: fixture\r\n\r\n")?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    Ok(Some(String::from_utf8_lossy(&response).into_owned()))
}

async fn wait_for_remote_forward(
    runtime: &SshRuntime,
    host: &HostProfile,
    child: &mut Child,
    source_port: u16,
) -> TestResult<String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let command = format!(
        "python3 -c 'import socket; s=socket.create_connection((\"127.0.0.1\", {source_port}), 2); print(s.recv(64).decode(), end=\"\")'"
    );
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "remote-forward process exited early with {status}"
            ))
            .into());
        }
        if let Ok(result) = runtime.run_command(host, command.clone(), None).await
            && result.exit_code == Some(0)
            && result.stdout.contains("remote-forward-ok")
        {
            return Ok(result.stdout);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "remote forward did not become ready",
            )
            .into());
        }
        sleep(Duration::from_millis(150)).await;
    }
}

struct MarkerServer {
    port: u16,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<io::Result<()>>>,
}

impl MarkerServer {
    fn start(marker: &'static [u8]) -> TestResult<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while !worker_stop.load(Ordering::Acquire) && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                        stream.write_all(marker)?;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        });
        Ok(Self {
            port,
            stop,
            worker: Some(worker),
        })
    }

    fn port(&self) -> u16 {
        self.port
    }

    fn stop(mut self) -> TestResult {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| io::Error::other("marker server thread panicked"))??;
        }
        Ok(())
    }
}

impl Drop for MarkerServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
