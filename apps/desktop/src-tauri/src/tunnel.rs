use crate::{
    error::{AppError, AppResult},
    model::{HostProfile, TunnelProfile, TunnelRuntimeState, TunnelSnapshot},
    ssh::SshRuntime,
};
use parking_lot::{Mutex, RwLock};
use std::{
    collections::HashMap,
    ffi::OsString,
    io::Read,
    path::Path,
    process::{Child, Stdio},
    sync::Arc,
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter};

const TUNNEL_EVENT: &str = "tunnel-event";
#[derive(Clone, Default)]
pub struct TunnelRegistry {
    processes: Arc<RwLock<HashMap<String, TunnelProcess>>>,
}
struct TunnelProcess {
    host_id: String,
    child: Arc<Mutex<Child>>,
}

impl TunnelRegistry {
    pub fn start(
        &self,
        app: AppHandle,
        host: &HostProfile,
        tunnel: &TunnelProfile,
        runtime: &SshRuntime,
    ) -> AppResult<TunnelSnapshot> {
        if self.processes.read().contains_key(&tunnel.id) {
            return Err(AppError::Validation(format!(
                "tunnel '{}' is already running",
                tunnel.name
            )));
        }
        let (program, args) = runtime.tunnel_command(host, tunnel)?;
        let mut child = spawn(&program, &args)?;
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(child));
        self.processes.write().insert(
            tunnel.id.clone(),
            TunnelProcess {
                host_id: host.id.clone(),
                child: child.clone(),
            },
        );
        let stderr_buffer = Arc::new(Mutex::new(String::new()));
        if let Some(stderr) = stderr {
            let buffer = stderr_buffer.clone();
            thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = stderr.take(64 * 1024).read_to_end(&mut bytes);
                *buffer.lock() = String::from_utf8_lossy(&bytes).trim().to_owned();
            });
        }
        emit(
            &app,
            TunnelSnapshot {
                tunnel_id: tunnel.id.clone(),
                state: TunnelRuntimeState::Starting,
                message: None,
            },
        );
        let id = tunnel.id.clone();
        let processes = self.processes.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(350));
            let mut announced = false;
            loop {
                if !processes.read().contains_key(&id) {
                    return;
                }
                match child.lock().try_wait() {
                    Ok(Some(status)) => {
                        processes.write().remove(&id);
                        thread::sleep(Duration::from_millis(20));
                        let stderr = stderr_buffer.lock().clone();
                        emit(
                            &app,
                            TunnelSnapshot {
                                tunnel_id: id.clone(),
                                state: if status.success() {
                                    TunnelRuntimeState::Stopped
                                } else {
                                    TunnelRuntimeState::Failed
                                },
                                message: Some(if stderr.is_empty() {
                                    format!("ssh exited with {status}")
                                } else {
                                    stderr
                                }),
                            },
                        );
                        return;
                    }
                    Ok(None) => {
                        if !announced {
                            announced = true;
                            emit(
                                &app,
                                TunnelSnapshot {
                                    tunnel_id: id.clone(),
                                    state: TunnelRuntimeState::Running,
                                    message: None,
                                },
                            );
                        }
                    }
                    Err(error) => {
                        processes.write().remove(&id);
                        emit(
                            &app,
                            TunnelSnapshot {
                                tunnel_id: id.clone(),
                                state: TunnelRuntimeState::Failed,
                                message: Some(format!("failed to poll ssh tunnel: {error}")),
                            },
                        );
                        return;
                    }
                }
                thread::sleep(Duration::from_millis(250));
            }
        });
        Ok(TunnelSnapshot {
            tunnel_id: tunnel.id.clone(),
            state: TunnelRuntimeState::Starting,
            message: None,
        })
    }
    pub fn stop(&self, app: &AppHandle, tunnel_id: &str) -> AppResult<()> {
        let process = self
            .processes
            .write()
            .remove(tunnel_id)
            .ok_or_else(|| AppError::NotFound(format!("running tunnel {tunnel_id}")))?;
        let mut child = process.child.lock();
        if child.try_wait()?.is_none() {
            child.kill()?;
            let _ = child.wait();
        }
        emit(
            app,
            TunnelSnapshot {
                tunnel_id: tunnel_id.to_owned(),
                state: TunnelRuntimeState::Stopped,
                message: None,
            },
        );
        Ok(())
    }
    pub fn stop_for_host(&self, app: &AppHandle, host_id: &str) {
        let ids = self
            .processes
            .read()
            .iter()
            .filter(|(_, process)| process.host_id == host_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            let _ = self.stop(app, &id);
        }
    }
    pub fn stop_all(&self, app: &AppHandle) {
        let ids = self.processes.read().keys().cloned().collect::<Vec<_>>();
        for id in ids {
            let _ = self.stop(app, &id);
        }
    }
}
fn spawn(program: &Path, args: &[OsString]) -> AppResult<Child> {
    std::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(AppError::from)
}
fn emit(app: &AppHandle, event: TunnelSnapshot) {
    let _ = app.emit(TUNNEL_EVENT, event);
}
