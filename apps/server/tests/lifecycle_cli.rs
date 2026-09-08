//! Windows-only black-box coverage for the service process lifecycle.
//!
//! This never starts the historical Tauri executable and never uses the real
//! application-data directory. It exercises the final Rust service binary via
//! a fresh, isolated data directory instead.

#[cfg(not(windows))]
#[test]
#[ignore = "the real lifecycle process test requires Windows mutex and DPAPI semantics"]
fn lifecycle_cli_requires_windows() {}

#[cfg(windows)]
mod windows {
    use serde::Deserialize;
    use std::{
        error::Error,
        fs::{self, File},
        io,
        path::{Path, PathBuf},
        process::{Child, ExitStatus, Stdio},
        time::{Duration, Instant},
    };
    use tempfile::TempDir;
    use tokio::time::sleep;

    const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
    const EXIT_TIMEOUT: Duration = Duration::from_secs(15);

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RuntimeFile {
        pid: u32,
        base_url: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LaunchFile {
        base_url: String,
        browser_url: String,
    }

    struct OwnedChild {
        child: Option<Child>,
        stderr_path: PathBuf,
    }

    impl OwnedChild {
        fn pid(&self) -> u32 {
            self.child.as_ref().expect("child is retained").id()
        }

        fn is_running(&mut self) -> io::Result<bool> {
            Ok(self
                .child
                .as_mut()
                .expect("child is retained")
                .try_wait()?
                .is_none())
        }

        async fn wait_for_exit(&mut self, timeout: Duration) -> Result<ExitStatus, Box<dyn Error>> {
            let deadline = Instant::now() + timeout;
            loop {
                if let Some(status) = self.child.as_mut().expect("child is retained").try_wait()? {
                    self.child.take();
                    return Ok(status);
                }
                if Instant::now() >= deadline {
                    return Err(format!(
                        "RemoteDeck child did not exit in {timeout:?}; stderr: {}",
                        stderr_excerpt(&self.stderr_path)
                    )
                    .into());
                }
                sleep(Duration::from_millis(50)).await;
            }
        }
    }

    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if let Some(child) = self.child.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    fn binary() -> PathBuf {
        PathBuf::from(env!("CARGO_BIN_EXE_RemoteDeck"))
    }

    #[test]
    fn service_executable_uses_gui_subsystem_even_in_debug_builds() -> Result<(), Box<dyn Error>> {
        // A hidden parent alone does not protect a user who double-clicks a debug
        // binary. Verify the built PE, rather than just the source attribute.
        let image = fs::read(binary())?;
        assert_eq!(&image[..2], b"MZ");
        let pe = u32::from_le_bytes(image[0x3c..0x40].try_into()?) as usize;
        assert_eq!(&image[pe..pe + 4], b"PE\0\0");
        let subsystem_offset = pe + 24 + 68;
        let subsystem =
            u16::from_le_bytes(image[subsystem_offset..subsystem_offset + 2].try_into()?);
        assert_eq!(
            subsystem, 2,
            "RemoteDeck must never allocate a console window"
        );
        Ok(())
    }

    fn spawn(
        binary: &Path,
        args: &[&Path],
        stderr_path: PathBuf,
    ) -> Result<OwnedChild, Box<dyn Error>> {
        let stderr = File::create(&stderr_path)?;
        let mut command = remotedeck_core::process::command(binary);
        command
            .arg("--no-open")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr));
        Ok(OwnedChild {
            child: Some(command.spawn()?),
            stderr_path,
        })
    }

    fn server_args<'a>(data_dir: &'a Path, launch_file: &'a Path) -> [&'a Path; 4] {
        [
            Path::new("--data-dir"),
            data_dir,
            Path::new("--launch-file"),
            launch_file,
        ]
    }

    fn stop_args(data_dir: &Path) -> [&Path; 3] {
        [Path::new("--stop"), Path::new("--data-dir"), data_dir]
    }

    fn stderr_excerpt(path: &Path) -> String {
        const MAX_BYTES: usize = 2048;
        match fs::read(path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_BYTES)])
                .trim()
                .to_owned(),
            Err(error) => format!("unavailable ({error})"),
        }
    }

    fn parse_runtime(data_dir: &Path) -> Option<RuntimeFile> {
        let contents = fs::read(data_dir.join("runtime.json")).ok()?;
        serde_json::from_slice(&contents).ok()
    }

    fn parse_launch(path: &Path) -> Option<LaunchFile> {
        let contents = fs::read(path).ok()?;
        serde_json::from_slice(&contents).ok()
    }

    async fn wait_for_ready(
        data_dir: &Path,
        launch_file: &Path,
        stderr_path: &Path,
    ) -> Result<(RuntimeFile, LaunchFile), Box<dyn Error>> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(1))
            .build()?;
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if let (Some(runtime), Some(launch)) =
                (parse_runtime(data_dir), parse_launch(launch_file))
            {
                let healthy = client
                    .get(format!("{}/health", runtime.base_url))
                    .send()
                    .await
                    .ok()
                    .is_some_and(|response| response.status().is_success());
                if healthy && launch.base_url == runtime.base_url {
                    return Ok((runtime, launch));
                }
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "RemoteDeck did not publish a healthy isolated service in {STARTUP_TIMEOUT:?}; stderr: {}",
                    stderr_excerpt(stderr_path)
                )
                .into());
            }
            sleep(Duration::from_millis(50)).await;
        }
    }

    fn ticket(launch: &LaunchFile) -> Result<&str, Box<dyn Error>> {
        let prefix = format!("{}/#ticket=", launch.base_url.trim_end_matches('/'));
        let ticket = launch
            .browser_url
            .strip_prefix(&prefix)
            .ok_or("launch file did not contain the service's loopback ticket URL")?;
        if ticket.len() != 64 || !ticket.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("launch file did not contain a 64-character one-time ticket".into());
        }
        Ok(ticket)
    }

    async fn exchange_once(
        base_url: &str,
        ticket: &str,
    ) -> Result<reqwest::Response, Box<dyn Error>> {
        let client = reqwest::Client::builder().no_proxy().build()?;
        let endpoint = format!("{}/api/v1/auth/exchange", base_url.trim_end_matches('/'));
        let response = client
            .post(&endpoint)
            .header("origin", base_url)
            .json(&serde_json::json!({ "ticket": ticket }))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err("a newly issued browser ticket was rejected".into());
        }
        let cookie = response.headers()["set-cookie"]
            .to_str()?
            .split(';')
            .next()
            .ok_or("missing session cookie")?
            .to_owned();
        let replay = client
            .post(endpoint)
            .header("origin", base_url)
            .json(&serde_json::json!({ "ticket": ticket }))
            .send()
            .await?;
        if replay.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Err("a consumed browser ticket was accepted again".into());
        }
        let events = client
            .get(format!("{base_url}/api/v1/events"))
            .header("cookie", cookie)
            .header("origin", base_url)
            .send()
            .await?;
        if !events.status().is_success() {
            return Err("authenticated event stream was rejected".into());
        }
        Ok(events)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn duplicate_cli_reuses_one_background_service_and_stop_cleans_it()
    -> Result<(), Box<dyn Error>> {
        let workspace = TempDir::new()?;
        let data_dir = workspace.path().join("isolated-data");
        let first_launch = workspace.path().join("first-launch.json");
        let second_launch = workspace.path().join("second-launch.json");
        let first_stderr = workspace.path().join("first.stderr.log");
        let second_stderr = workspace.path().join("second.stderr.log");
        let stop_stderr = workspace.path().join("stop.stderr.log");
        let binary = binary();

        let mut primary = spawn(
            &binary,
            &server_args(&data_dir, &first_launch),
            first_stderr.clone(),
        )?;
        let (first_runtime, first_launch_data) =
            wait_for_ready(&data_dir, &first_launch, &first_stderr).await?;
        if first_runtime.pid != primary.pid() {
            return Err("runtime descriptor pid does not identify the owned service child".into());
        }
        if !primary.is_running()? {
            return Err(format!(
                "initial RemoteDeck service exited before duplicate launch; stderr: {}",
                stderr_excerpt(&first_stderr)
            )
            .into());
        }
        let first_ticket = ticket(&first_launch_data)?;

        let mut duplicate = spawn(
            &binary,
            &server_args(&data_dir, &second_launch),
            second_stderr.clone(),
        )?;
        let duplicate_status = duplicate.wait_for_exit(EXIT_TIMEOUT).await?;
        if !duplicate_status.success() {
            return Err(format!(
                "duplicate RemoteDeck CLI exited unsuccessfully; stderr: {}",
                stderr_excerpt(&second_stderr)
            )
            .into());
        }

        let second_launch_data = parse_launch(&second_launch)
            .ok_or("duplicate CLI did not write its requested launch file")?;
        let second_runtime = parse_runtime(&data_dir)
            .ok_or("duplicate CLI removed the active runtime descriptor")?;
        if second_runtime.pid != first_runtime.pid || second_runtime.pid != primary.pid() {
            return Err(
                "two CLI invocations did not retain exactly one background service pid".into(),
            );
        }
        if !primary.is_running()? {
            return Err("duplicate CLI stopped the existing background service".into());
        }
        let second_ticket = ticket(&second_launch_data)?;
        if first_ticket == second_ticket
            || first_launch_data.browser_url == second_launch_data.browser_url
        {
            return Err("duplicate CLI did not receive a distinct one-time browser URL".into());
        }
        let open_event_stream = exchange_once(&second_launch_data.base_url, second_ticket).await?;

        let mut stopper = spawn(&binary, &stop_args(&data_dir), stop_stderr.clone())?;
        let stop_status = stopper.wait_for_exit(EXIT_TIMEOUT).await?;
        if !stop_status.success() {
            return Err(format!(
                "stop CLI exited unsuccessfully; stderr: {}",
                stderr_excerpt(&stop_stderr)
            )
            .into());
        }
        let primary_status = primary.wait_for_exit(EXIT_TIMEOUT).await?;
        if !primary_status.success() {
            return Err(format!(
                "owned service did not stop cleanly; stderr: {}",
                stderr_excerpt(&first_stderr)
            )
            .into());
        }
        if data_dir.join("runtime.json").exists() {
            return Err("owned service left runtime.json after a clean stop".into());
        }
        let shutdown: serde_json::Value =
            serde_json::from_slice(&fs::read(data_dir.join("last-shutdown.json"))?)?;
        if shutdown.get("clean").and_then(serde_json::Value::as_bool) != Some(true) {
            return Err("owned service did not persist a clean shutdown report".into());
        }
        drop(open_event_stream);
        Ok(())
    }
}
