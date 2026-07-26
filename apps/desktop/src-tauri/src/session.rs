use crate::{
    error::{AppError, AppResult},
    model::{HostProfile, TerminalEvent, TerminalEventKind, TerminalSnapshot, TerminalState},
    ssh::SshRuntime,
};
use parking_lot::{Mutex, RwLock};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, PtySystem, native_pty_system};
use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::Arc,
    thread,
};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

const TERMINAL_EVENT: &str = "terminal-event";
const MAX_CHUNK: usize = 64 * 1024;

#[derive(Clone, Default)]
pub struct TerminalRegistry {
    sessions: Arc<RwLock<HashMap<String, TerminalSession>>>,
}
struct TerminalSession {
    host_id: String,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
}

impl TerminalRegistry {
    pub fn start(
        &self,
        app: AppHandle,
        host: &HostProfile,
        runtime: &SshRuntime,
        rows: u16,
        cols: u16,
    ) -> AppResult<TerminalSnapshot> {
        validate_size(rows, cols)?;
        let (program, args) = runtime.terminal_command(host)?;
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| AppError::Process(format!("failed to open PTY: {error}")))?;
        let mut command = CommandBuilder::new(&program);
        command.args(args);
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| AppError::Process(format!("failed to start ssh in PTY: {error}")))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| AppError::Process(format!("failed to clone PTY reader: {error}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| AppError::Process(format!("failed to take PTY writer: {error}")))?;
        let killer = child.clone_killer();
        drop(pair.slave);
        let master = Arc::new(Mutex::new(pair.master));
        let session_id = Uuid::new_v4().to_string();
        self.sessions.write().insert(
            session_id.clone(),
            TerminalSession {
                host_id: host.id.clone(),
                writer: Arc::new(Mutex::new(writer)),
                master,
                killer: Arc::new(Mutex::new(killer)),
            },
        );
        emit(
            &app,
            TerminalEvent {
                session_id: session_id.clone(),
                kind: TerminalEventKind::Started,
                data: None,
                exit_code: None,
                message: None,
            },
        );

        let read_app = app.clone();
        let read_id = session_id.clone();
        thread::spawn(move || {
            let mut buffer = vec![0_u8; MAX_CHUNK];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => emit(
                        &read_app,
                        TerminalEvent {
                            session_id: read_id.clone(),
                            kind: TerminalEventKind::Output,
                            data: Some(String::from_utf8_lossy(&buffer[..read]).into_owned()),
                            exit_code: None,
                            message: None,
                        },
                    ),
                    Err(error) => {
                        emit(
                            &read_app,
                            TerminalEvent {
                                session_id: read_id.clone(),
                                kind: TerminalEventKind::Error,
                                data: None,
                                exit_code: None,
                                message: Some(format!("PTY read failed: {error}")),
                            },
                        );
                        break;
                    }
                }
            }
        });

        let wait_app = app;
        let wait_id = session_id.clone();
        let sessions = self.sessions.clone();
        thread::spawn(move || {
            let event = match child.wait() {
                Ok(status) => TerminalEvent {
                    session_id: wait_id.clone(),
                    kind: TerminalEventKind::Exit,
                    data: None,
                    exit_code: i32::try_from(status.exit_code()).ok(),
                    message: status.signal().map(ToOwned::to_owned),
                },
                Err(error) => TerminalEvent {
                    session_id: wait_id.clone(),
                    kind: TerminalEventKind::Error,
                    data: None,
                    exit_code: None,
                    message: Some(format!("failed to wait for PTY child: {error}")),
                },
            };
            sessions.write().remove(&wait_id);
            emit(&wait_app, event);
        });
        Ok(TerminalSnapshot {
            session_id,
            host_id: host.id.clone(),
            alias: host.alias.clone(),
            state: TerminalState::Running,
        })
    }

    pub fn write(&self, session_id: &str, data: &str) -> AppResult<()> {
        if data.len() > MAX_CHUNK {
            return Err(AppError::Validation(
                "terminal input exceeds 64 KiB".to_owned(),
            ));
        }
        let writer = self
            .sessions
            .read()
            .get(session_id)
            .map(|session| session.writer.clone())
            .ok_or_else(|| AppError::NotFound(format!("terminal session {session_id}")))?;
        let mut writer = writer.lock();
        writer.write_all(data.as_bytes())?;
        writer.flush()?;
        Ok(())
    }
    pub fn resize(&self, session_id: &str, rows: u16, cols: u16) -> AppResult<()> {
        validate_size(rows, cols)?;
        let master = self
            .sessions
            .read()
            .get(session_id)
            .map(|session| session.master.clone())
            .ok_or_else(|| AppError::NotFound(format!("terminal session {session_id}")))?;
        master
            .lock()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| AppError::Process(format!("failed to resize PTY: {error}")))
    }
    pub fn close(&self, session_id: &str) -> AppResult<()> {
        let session = self
            .sessions
            .write()
            .remove(session_id)
            .ok_or_else(|| AppError::NotFound(format!("terminal session {session_id}")))?;
        session
            .killer
            .lock()
            .kill()
            .map_err(|error| AppError::Process(format!("failed to close terminal: {error}")))
    }
    pub fn stop_for_host(&self, host_id: &str) {
        let ids = self
            .sessions
            .read()
            .iter()
            .filter(|(_, session)| session.host_id == host_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            let _ = self.close(&id);
        }
    }
    pub fn stop_all(&self) {
        let ids = self.sessions.read().keys().cloned().collect::<Vec<_>>();
        for id in ids {
            let _ = self.close(&id);
        }
    }
}
fn validate_size(rows: u16, cols: u16) -> AppResult<()> {
    if !(2..=1000).contains(&rows) || !(2..=1000).contains(&cols) {
        return Err(AppError::Validation(
            "terminal dimensions must be between 2 and 1000".to_owned(),
        ));
    }
    Ok(())
}
fn emit(app: &AppHandle, event: TerminalEvent) {
    let _ = app.emit(TERMINAL_EVENT, event);
}
