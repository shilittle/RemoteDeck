#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod api;
mod assets;
mod auth;
#[cfg(test)]
mod http_tests;
mod lifecycle;
mod native;
mod operations;
mod transport;

use auth::Auth;
use lifecycle::{AcquireResult, RuntimeDescriptor, RuntimeGuard};
use std::{
    error::Error,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use transport::{EventHub, Server};

type ServeTask = tokio::task::JoinHandle<std::io::Result<()>>;

#[derive(Default)]
struct Options {
    directory: Option<PathBuf>,
    port: u16,
    launch_file: Option<PathBuf>,
    dev_origin: Option<String>,
    stop: bool,
    no_open: bool,
}
impl Options {
    fn parse() -> Result<Self, Box<dyn Error>> {
        let mut options = Self::default();
        let mut args = std::env::args_os().skip(1);
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--data-dir") => {
                    options.directory =
                        Some(PathBuf::from(args.next().ok_or("--data-dir needs a path")?))
                }
                Some("--port") => {
                    options.port = args
                        .next()
                        .ok_or("--port needs a number")?
                        .to_str()
                        .ok_or("invalid port")?
                        .parse()?
                }
                Some("--launch-file") => {
                    options.launch_file = Some(PathBuf::from(
                        args.next().ok_or("--launch-file needs a path")?,
                    ))
                }
                Some("--dev-origin") => {
                    let value = args
                        .next()
                        .ok_or("--dev-origin needs an origin")?
                        .into_string()
                        .map_err(|_| "invalid origin")?;
                    let suffix = value
                        .strip_prefix("http://127.0.0.1:")
                        .ok_or("development origin must use IPv4 loopback")?;
                    let port: u16 = suffix.parse()?;
                    if port == 0 {
                        return Err("development origin requires a concrete port".into());
                    }
                    options.dev_origin = Some(value);
                }
                Some("--stop") => options.stop = true,
                Some("--no-open" | "--hidden") => options.no_open = true,
                Some("--help" | "-h") => {
                    println!(
                        "RemoteDeck [--no-open] [--stop] [--data-dir PATH] [--port PORT]\nDevelopment: --dev-origin http://127.0.0.1:PORT --launch-file PATH (requires --data-dir)"
                    );
                    std::process::exit(0);
                }
                _ => return Err("unrecognized RemoteDeck argument".into()),
            }
        }
        if options.launch_file.is_some() && options.directory.is_none() {
            return Err("--launch-file requires an explicit isolated --data-dir".into());
        }
        Ok(options)
    }
    fn directory(&self) -> Result<PathBuf, Box<dyn Error>> {
        if let Some(directory) = &self.directory {
            return Ok(std::path::absolute(directory)?);
        }
        #[cfg(windows)]
        {
            Ok(
                PathBuf::from(std::env::var_os("APPDATA").ok_or("Windows APPDATA is unavailable")?)
                    .join("io.github.shilittle.remotedeck"),
            )
        }
        #[cfg(not(windows))]
        {
            Ok(
                PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap_or_else(|| {
                    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                        .join(".local/share")
                        .into_os_string()
                }))
                .join("io.github.shilittle.remotedeck"),
            )
        }
    }
}

fn write_launch(
    path: &std::path::Path,
    base_url: &str,
    browser_url: &str,
) -> Result<(), Box<dyn Error>> {
    let contents = serde_json::to_vec(&serde_json::json!({
        "baseUrl": base_url,
        "browserUrl": browser_url,
    }))?;
    write_private_atomic(path, &contents)?;
    Ok(())
}

/// Write a complete private file in the destination directory, then replace the
/// old file in one same-filesystem rename. `std::fs::rename` replaces an
/// existing file on supported Windows versions, so a reader sees either the
/// former complete JSON document or the new one, never a truncated document.
fn write_private_atomic(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    if path.file_name().is_none_or(|name| name.is_empty()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "missing file name",
        ));
    }
    std::fs::create_dir_all(parent)?;

    let temporary = parent.join(format!(
        ".remotedeck-{}-{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = options.open(&temporary).and_then(|mut file| {
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

async fn shutdown_server(
    server: &Server,
    serve: Option<&mut ServeTask>,
    directory: &std::path::Path,
) -> Vec<String> {
    server.request_stop();
    let requests_drained = server.admission.cancel_and_wait().await;
    let interrupted = server.operations.abort_pending().await;
    let mut errors = server.context.shutdown().await;
    if !requests_drained {
        errors
            .push("Some admitted requests did not finish cancellation within two seconds.".into());
    }
    if interrupted > 0 {
        errors.push(format!(
            "{interrupted} in-flight operations were interrupted; verify their remote effects before retrying."
        ));
    }
    if let Some(serve) = serve {
        match tokio::time::timeout(Duration::from_secs(3), &mut *serve).await {
            Ok(Ok(Ok(()))) => (),
            Ok(Ok(Err(error))) => errors.push(format!("HTTP service shutdown: {error}")),
            Ok(Err(error)) => errors.push(format!("HTTP service task shutdown: {error}")),
            Err(_) => {
                serve.abort();
                let _ = tokio::time::timeout(Duration::from_secs(1), &mut *serve).await;
                errors
                    .push("HTTP service did not stop within three seconds and was aborted.".into());
            }
        }
    }
    let report = serde_json::json!({
        "at": chrono::Utc::now(),
        "clean": errors.is_empty(),
        "issues": &errors,
    });
    match serde_json::to_vec_pretty(&report)
        .map_err(std::io::Error::other)
        .and_then(|contents| write_private_atomic(&directory.join("last-shutdown.json"), &contents))
    {
        Ok(()) => (),
        Err(error) => errors.push(format!("Could not persist the shutdown report: {error}")),
    }
    errors
}

fn finish(
    primary: Result<(), Box<dyn Error>>,
    cleanup_errors: Vec<String>,
) -> Result<(), Box<dyn Error>> {
    if cleanup_errors.is_empty() {
        return primary;
    }
    let cleanup = cleanup_errors.join(" ");
    match primary {
        Ok(()) => Err(format!("RemoteDeck stopped with cleanup issues: {cleanup}").into()),
        Err(error) => Err(format!("{error}; cleanup issues: {cleanup}").into()),
    }
}
async fn existing(descriptor: RuntimeDescriptor, options: &Options) -> Result<(), Box<dyn Error>> {
    // Never inherit a proxy for the authenticated local launcher channel.
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()?;
    let action = if options.stop {
        "stop"
    } else if options.launch_file.is_some() {
        "launch"
    } else if options.no_open {
        return Ok(());
    } else {
        "open"
    };
    let response = client
        .post(format!(
            "{}/internal/{action}",
            descriptor.base_url.trim_end_matches('/')
        ))
        .bearer_auth(descriptor.control_token)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(format!(
            "Existing RemoteDeck service refused {action}: {}",
            response.status()
        )
        .into());
    }
    if let Some(path) = &options.launch_file
        && action == "launch"
    {
        let body: serde_json::Value = response.json().await?;
        let url = body
            .get("browserUrl")
            .and_then(serde_json::Value::as_str)
            .ok_or("service returned no browser launch URL")?;
        write_launch(path, &descriptor.base_url, url)?;
    }
    Ok(())
}

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("create RemoteDeck runtime");
    let result = runtime.block_on(run());
    // A stuck OS/PTY blocking call must not turn a bounded application cleanup
    // into an unbounded Tokio runtime drop. Resource issues are recorded above.
    runtime.shutdown_timeout(Duration::from_secs(2));
    if let Err(error) = result {
        #[cfg(debug_assertions)]
        eprintln!("RemoteDeck: {error}");
        #[cfg(not(debug_assertions))]
        {
            rfd::MessageDialog::new()
                .set_title("RemoteDeck")
                .set_description(error.to_string())
                .set_level(rfd::MessageLevel::Error)
                .show();
        }
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn Error>> {
    let options = Options::parse()?;
    let directory = options.directory()?;
    std::fs::create_dir_all(&directory)?;
    let callback_server = Arc::new(Mutex::new(None::<Server>));
    let callback = callback_server.clone();
    let acquired = RuntimeGuard::acquire(
        &directory,
        Arc::new(move || {
            if let Some(server) = callback.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                let _ = server.open_browser();
            }
        }),
    )?;
    let guard = match acquired {
        AcquireResult::Existing(descriptor) => return existing(descriptor, &options).await,
        AcquireResult::Primary(guard) => guard,
    };
    if options.stop {
        return Ok(());
    }
    let listener =
        tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, options.port)).await?;
    let port = listener.local_addr()?.port();
    let control_token = auth::random_token();
    let auth = Auth::new(port, control_token.clone(), options.dev_origin.clone());
    let hub = EventHub::default();
    let context = api::AppContext::open(directory.clone(), hub.clone())?;
    // Reuse trust already established by local OpenSSH before any monitor or
    // forwarding child can start. This never scans or accepts a network key.
    context.initialize_local_trust().await;
    let server = Server::new(context, auth, hub);
    let mut primary: Result<(), Box<dyn Error>> = guard
        .publish(RuntimeDescriptor {
            pid: std::process::id(),
            base_url: server.auth.origin.clone(),
            control_token,
        })
        .map_err(Into::into);
    let mut serve = None;
    if primary.is_ok() {
        *callback_server.lock().unwrap_or_else(|e| e.into_inner()) = Some(server.clone());
        let app = transport::router(server.clone());
        let mut shutdown = server.stop.subscribe();
        serve = Some(tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    loop {
                        if *shutdown.borrow_and_update() {
                            break;
                        }
                        if shutdown.changed().await.is_err() {
                            break;
                        }
                    }
                })
                .await
        }));
        server.context.autostart();
        if let Some(path) = &options.launch_file {
            primary = match server.auth.launch_url() {
                Ok(url) => write_launch(path, &server.auth.origin, &url),
                Err(error) => Err(error.into()),
            };
        }
        if primary.is_ok() && !options.no_open && options.launch_file.is_none() {
            primary = server.open_browser().map_err(Into::into);
        }
    }

    let mut serve_finished = false;
    if primary.is_ok() {
        let mut requested = server.stop.subscribe();
        let serve = serve.as_mut().expect("service task starts with the server");
        primary = tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                server.request_stop();
                Ok(())
            }
            _ = async { loop {
                if *requested.borrow_and_update() { break; }
                if requested.changed().await.is_err() { break; }
            } } => Ok(()),
            result = serve => {
                serve_finished = true;
                match result {
                    Ok(Ok(())) if server.stopping.load(std::sync::atomic::Ordering::SeqCst) => Ok(()),
                    Ok(Ok(())) => Err("The HTTP service stopped unexpectedly.".into()),
                    Ok(Err(error)) => Err(format!("The HTTP service stopped unexpectedly: {error}").into()),
                    Err(error) => Err(format!("The HTTP service task failed: {error}").into()),
                }
            }
        }
    }

    let pending_serve = if serve_finished { None } else { serve.as_mut() };
    let cleanup_errors = shutdown_server(&server, pending_serve, &directory).await;
    *callback_server.lock().unwrap_or_else(|e| e.into_inner()) = None;
    drop(guard);
    finish(primary, cleanup_errors)
}

#[cfg(test)]
mod tests {
    use super::{write_launch, write_private_atomic};
    use std::fs;

    #[test]
    fn launch_file_replaces_an_existing_complete_document() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let launch = directory.path().join("launch.json");
        fs::write(&launch, b"{\"obsolete\":true}").expect("seed old launch document");

        write_launch(
            &launch,
            "http://127.0.0.1:4567",
            "http://127.0.0.1:4567/#ticket=abcd",
        )
        .expect("replace launch document");

        let actual: serde_json::Value =
            serde_json::from_slice(&fs::read(&launch).expect("read launch document"))
                .expect("valid launch JSON");
        assert_eq!(actual["baseUrl"], "http://127.0.0.1:4567");
        assert_eq!(actual["browserUrl"], "http://127.0.0.1:4567/#ticket=abcd");
    }

    #[test]
    fn failed_replacement_removes_the_private_temporary_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let destination = directory.path().join("destination-directory");
        fs::create_dir(&destination).expect("create replacement blocker");

        assert!(write_private_atomic(&destination, b"new document").is_err());
        let remnants = fs::read_dir(directory.path())
            .expect("read temporary directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".remotedeck-")
            })
            .count();
        assert_eq!(remnants, 0, "failed replacement left a temporary file");
    }
}
