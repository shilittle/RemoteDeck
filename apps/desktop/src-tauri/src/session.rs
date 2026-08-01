use crate::{
    error::{AppError, AppResult},
    host_operation::HostOperationBarrier,
    model::{HostProfile, TerminalEvent, TerminalEventKind, TerminalSnapshot, TerminalState},
    ssh::SshRuntime,
};
use parking_lot::{Mutex, RwLock};
use portable_pty::{
    Child, ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};
use tungstenite::{
    Error as WebSocketError, accept_hdr_with_config,
    handshake::server::{Callback, ErrorResponse, Request, Response},
    http::StatusCode,
    protocol::{Message, WebSocketConfig},
};
use uuid::Uuid;

const TERMINAL_EVENT: &str = "terminal-event";
const MAX_CHUNK: usize = 64 * 1024;
const MAX_ACTIVE_TERMINALS: usize = 32;
const MAX_RETAINED_TERMINALS: usize = 256;
const MAX_INPUT_TICKETS: usize = MAX_ACTIVE_TERMINALS * 2;
const MAX_INPUT_TICKETS_PER_SESSION: usize = 4;
const MAX_PENDING_INPUT_CONNECTIONS: usize = MAX_ACTIVE_TERMINALS * 2;
const INPUT_TICKET_TTL: Duration = Duration::from_secs(30);
const INPUT_INVALIDATION_TTL: Duration = Duration::from_secs(60);
const INPUT_ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(25);
const INPUT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const INPUT_READ_POLL_INTERVAL: Duration = Duration::from_millis(250);
const CHILD_TERMINATION_TIMEOUT: Duration = Duration::from_secs(2);
const CHILD_TERMINATION_POLL_INTERVAL: Duration = Duration::from_millis(10);
const HOST_RETIREMENT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct TerminalRegistry {
    state: Arc<RwLock<TerminalRegistryState>>,
    next_generation: Arc<AtomicU64>,
    input: Arc<TerminalInputTransport>,
    host_operations: HostOperationBarrier,
}

impl Default for TerminalRegistry {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(TerminalRegistryState::default())),
            next_generation: Arc::new(AtomicU64::new(0)),
            input: Arc::new(TerminalInputTransport::default()),
            host_operations: HostOperationBarrier::default(),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInputEndpoint {
    url: String,
}

#[derive(Default)]
struct TerminalInputTransport {
    server: Mutex<Option<InputServer>>,
    tickets: Arc<Mutex<HashMap<String, InputTicket>>>,
    connections: Arc<Mutex<HashMap<u64, ActiveInputConnection>>>,
    invalidated: Arc<Mutex<HashMap<String, InvalidatedGeneration>>>,
    next_connection_id: Arc<AtomicU64>,
    pending_connections: Arc<AtomicUsize>,
    closed: Arc<AtomicBool>,
}

struct InputServer {
    address: SocketAddr,
    shutdown: Arc<AtomicBool>,
    listener_thread: Option<thread::JoinHandle<()>>,
}

#[derive(Clone)]
struct InputTicket {
    session_id: String,
    generation: u64,
    expires_at: Instant,
}

struct ActiveInputConnection {
    session_id: String,
    generation: u64,
    cancel: Arc<AtomicBool>,
}

struct InvalidatedGeneration {
    generation: u64,
    invalidated_at: Instant,
}

#[derive(Default)]
struct TerminalRegistryState {
    active: HashMap<String, TerminalSlot>,
    snapshots: HashMap<String, TerminalRecord>,
}

#[derive(Clone)]
struct TerminalRecord {
    generation: u64,
    snapshot: TerminalSnapshot,
}

enum TerminalSlot {
    Starting {
        generation: u64,
        host_id: String,
        close_requested: bool,
        cleanup: Option<SpawnCleanup>,
    },
    Running(TerminalSession),
}

impl TerminalSlot {
    fn generation(&self) -> u64 {
        match self {
            Self::Starting { generation, .. } => *generation,
            Self::Running(session) => session.generation,
        }
    }

    fn host_id(&self) -> &str {
        match self {
            Self::Starting { host_id, .. } => host_id,
            Self::Running(session) => &session.host_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TerminalCommandOptions {
    pub rows: u16,
    pub cols: u16,
    pub remote_command: String,
    pub title: Option<String>,
}

struct TerminalSpawnOptions {
    session_id: String,
    generation: u64,
    rows: u16,
    cols: u16,
    remote_command: Option<String>,
    title: Option<String>,
}

#[derive(Clone)]
struct SpawnCleanup {
    killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
    child: Arc<Mutex<Option<Box<dyn Child + Send + Sync>>>>,
    terminated: Arc<AtomicBool>,
}

struct TerminalSession {
    generation: u64,
    host_id: String,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    cleanup: SpawnCleanup,
}

impl TerminalInputTransport {
    fn open_endpoint(
        &self,
        state: &Arc<RwLock<TerminalRegistryState>>,
        session_id: &str,
    ) -> AppResult<TerminalInputEndpoint> {
        if self.closed.load(Ordering::Acquire) {
            return Err(AppError::State(
                "terminal input transport is shutting down".to_owned(),
            ));
        }
        let generation = running_generation(state, session_id)
            .ok_or_else(|| AppError::NotFound(format!("running terminal session {session_id}")))?;
        let address = self.ensure_server(Arc::clone(state))?;
        let now = Instant::now();
        let token = new_input_token();
        {
            let mut tickets = self.tickets.lock();
            tickets.retain(|_, ticket| ticket.expires_at > now);
            while tickets
                .values()
                .filter(|ticket| ticket.session_id == session_id && ticket.generation == generation)
                .count()
                >= MAX_INPUT_TICKETS_PER_SESSION
            {
                let oldest = tickets
                    .iter()
                    .filter(|(_, ticket)| {
                        ticket.session_id == session_id && ticket.generation == generation
                    })
                    .min_by_key(|(_, ticket)| ticket.expires_at)
                    .map(|(token, _)| token.clone());
                let Some(oldest) = oldest else {
                    break;
                };
                tickets.remove(&oldest);
            }
            if tickets.len() >= MAX_INPUT_TICKETS {
                return Err(AppError::Validation(format!(
                    "at most {MAX_INPUT_TICKETS} terminal input tickets are allowed"
                )));
            }
            tickets.insert(
                token.clone(),
                InputTicket {
                    session_id: session_id.to_owned(),
                    generation,
                    expires_at: now + INPUT_TICKET_TTL,
                },
            );
        }
        Ok(TerminalInputEndpoint {
            url: format!("ws://{address}/terminal-input?token={token}"),
        })
    }

    fn ensure_server(
        &self,
        terminal_state: Arc<RwLock<TerminalRegistryState>>,
    ) -> AppResult<SocketAddr> {
        let mut server_slot = self.server.lock();
        if let Some(server) = server_slot.as_ref() {
            return Ok(server.address);
        }
        if self.closed.load(Ordering::Acquire) {
            return Err(AppError::State(
                "terminal input transport is shutting down".to_owned(),
            ));
        }

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|error| {
            AppError::Process(format!(
                "failed to bind the terminal input transport: {error}"
            ))
        })?;
        listener.set_nonblocking(true).map_err(|error| {
            AppError::Process(format!(
                "failed to configure the terminal input transport: {error}"
            ))
        })?;
        let address = listener.local_addr().map_err(|error| {
            AppError::Process(format!(
                "failed to inspect the terminal input transport: {error}"
            ))
        })?;
        if address.ip() != Ipv4Addr::LOCALHOST {
            return Err(AppError::State(
                "terminal input transport did not bind to IPv4 loopback".to_owned(),
            ));
        }

        let shutdown = Arc::new(AtomicBool::new(false));
        let context = InputServerContext {
            address,
            terminal_state,
            tickets: Arc::clone(&self.tickets),
            connections: Arc::clone(&self.connections),
            invalidated: Arc::clone(&self.invalidated),
            next_connection_id: Arc::clone(&self.next_connection_id),
            pending_connections: Arc::clone(&self.pending_connections),
            shutdown: Arc::clone(&shutdown),
        };
        let listener_thread = thread::Builder::new()
            .name("remotedeck-terminal-input-listener".to_owned())
            .spawn(move || run_input_listener(listener, context))
            .map_err(|error| {
                AppError::Process(format!(
                    "failed to start the terminal input transport: {error}"
                ))
            })?;
        *server_slot = Some(InputServer {
            address,
            shutdown,
            listener_thread: Some(listener_thread),
        });
        Ok(address)
    }

    fn invalidate_session(&self, session_id: &str, generation: u64) {
        let now = Instant::now();
        {
            let mut invalidated = self.invalidated.lock();
            prune_invalidations(&mut invalidated, now);
            invalidated
                .entry(session_id.to_owned())
                .and_modify(|entry| {
                    if generation >= entry.generation {
                        entry.generation = generation;
                        entry.invalidated_at = now;
                    }
                })
                .or_insert(InvalidatedGeneration {
                    generation,
                    invalidated_at: now,
                });
        }
        self.tickets
            .lock()
            .retain(|_, ticket| ticket.session_id != session_id || ticket.generation != generation);
        for connection in self.connections.lock().values() {
            if connection.session_id == session_id && connection.generation == generation {
                connection.cancel.store(true, Ordering::Release);
            }
        }
    }

    fn shutdown(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.tickets.lock().clear();
        for connection in self.connections.lock().values() {
            connection.cancel.store(true, Ordering::Release);
        }
        let mut server = self.server.lock().take();
        if let Some(server) = server.as_mut() {
            server.shutdown.store(true, Ordering::Release);
            if let Some(listener_thread) = server.listener_thread.take() {
                let _ = listener_thread.join();
            }
        }
        let deadline = Instant::now() + INPUT_HANDSHAKE_TIMEOUT + INPUT_ACCEPT_POLL_INTERVAL;
        while (!self.connections.lock().is_empty()
            || self.pending_connections.load(Ordering::Acquire) > 0)
            && Instant::now() < deadline
        {
            thread::sleep(INPUT_ACCEPT_POLL_INTERVAL);
        }
        self.connections.lock().clear();
        self.invalidated.lock().clear();
    }
}

#[derive(Clone)]
struct InputServerContext {
    address: SocketAddr,
    terminal_state: Arc<RwLock<TerminalRegistryState>>,
    tickets: Arc<Mutex<HashMap<String, InputTicket>>>,
    connections: Arc<Mutex<HashMap<u64, ActiveInputConnection>>>,
    invalidated: Arc<Mutex<HashMap<String, InvalidatedGeneration>>>,
    next_connection_id: Arc<AtomicU64>,
    pending_connections: Arc<AtomicUsize>,
    shutdown: Arc<AtomicBool>,
}

fn run_input_listener(listener: TcpListener, context: InputServerContext) {
    while !context.shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, address)) => {
                if !address.ip().is_loopback() {
                    continue;
                }
                let previous = context.pending_connections.fetch_add(1, Ordering::AcqRel);
                if previous >= MAX_PENDING_INPUT_CONNECTIONS {
                    context.pending_connections.fetch_sub(1, Ordering::AcqRel);
                    continue;
                }
                let connection_context = context.clone();
                if thread::Builder::new()
                    .name("remotedeck-terminal-input".to_owned())
                    .spawn(move || handle_input_connection(stream, connection_context))
                    .is_err()
                {
                    context.pending_connections.fetch_sub(1, Ordering::AcqRel);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(INPUT_ACCEPT_POLL_INTERVAL);
            }
            Err(_) => thread::sleep(INPUT_ACCEPT_POLL_INTERVAL),
        }
    }
}

struct PendingInputGuard(Arc<AtomicUsize>);

impl Drop for PendingInputGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct InputHandshakeCallback {
    context: InputServerContext,
    claimed: Arc<Mutex<Option<InputTicket>>>,
}

impl Callback for InputHandshakeCallback {
    #[allow(clippy::result_large_err)]
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        match claim_input_ticket(request, &self.context) {
            Ok(ticket) => {
                *self.claimed.lock() = Some(ticket);
                Ok(response)
            }
            Err(status) => Err(input_rejection(status)),
        }
    }
}

fn handle_input_connection(stream: TcpStream, context: InputServerContext) {
    let _pending = PendingInputGuard(Arc::clone(&context.pending_connections));
    let _ = stream.set_read_timeout(Some(INPUT_HANDSHAKE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(INPUT_HANDSHAKE_TIMEOUT));

    let claimed = Arc::new(Mutex::new(None::<InputTicket>));
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_CHUNK);
    config.max_frame_size = Some(MAX_CHUNK);
    let handshake = accept_hdr_with_config(
        stream,
        InputHandshakeCallback {
            context: context.clone(),
            claimed: Arc::clone(&claimed),
        },
        Some(config),
    );
    let Ok(mut websocket) = handshake else {
        return;
    };
    let Some(ticket) = claimed.lock().take() else {
        let _ = websocket.close(None);
        return;
    };

    let connection_id = context.next_connection_id.fetch_add(1, Ordering::Relaxed);
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut connections = context.connections.lock();
        for connection in connections.values() {
            if connection.session_id == ticket.session_id
                && connection.generation == ticket.generation
            {
                connection.cancel.store(true, Ordering::Release);
            }
        }
        connections.insert(
            connection_id,
            ActiveInputConnection {
                session_id: ticket.session_id.clone(),
                generation: ticket.generation,
                cancel: Arc::clone(&cancel),
            },
        );
    }
    if is_input_generation_invalidated(&context.invalidated, &ticket.session_id, ticket.generation)
        || running_generation(&context.terminal_state, &ticket.session_id)
            != Some(ticket.generation)
    {
        cancel.store(true, Ordering::Release);
    }

    let _ = websocket
        .get_mut()
        .set_read_timeout(Some(INPUT_READ_POLL_INTERVAL));
    let _ = websocket
        .get_mut()
        .set_write_timeout(Some(INPUT_READ_POLL_INTERVAL));
    while !cancel.load(Ordering::Acquire) && !context.shutdown.load(Ordering::Acquire) {
        match websocket.read() {
            Ok(Message::Text(data)) => {
                if write_terminal_bytes(
                    &context.terminal_state,
                    &ticket.session_id,
                    ticket.generation,
                    data.as_bytes(),
                )
                .is_err()
                {
                    break;
                }
            }
            Ok(Message::Binary(data)) => {
                if write_terminal_bytes(
                    &context.terminal_state,
                    &ticket.session_id,
                    ticket.generation,
                    &data,
                )
                .is_err()
                {
                    break;
                }
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {
                let _ = websocket.flush();
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Frame(_)) => {}
            Err(WebSocketError::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    let _ = websocket.close(None);
    context.connections.lock().remove(&connection_id);
}

fn claim_input_ticket(
    request: &Request,
    context: &InputServerContext,
) -> Result<InputTicket, StatusCode> {
    if request.uri().path() != "/terminal-input" {
        return Err(StatusCode::NOT_FOUND);
    }
    let expected_host = context.address.to_string();
    let host = request
        .headers()
        .get("host")
        .and_then(|value| value.to_str().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    if host != expected_host {
        return Err(StatusCode::BAD_REQUEST);
    }
    let origin = request
        .headers()
        .get("origin")
        .and_then(|value| value.to_str().ok())
        .ok_or(StatusCode::FORBIDDEN)?;
    if !is_allowed_input_origin(origin) {
        return Err(StatusCode::FORBIDDEN);
    }
    let token = input_token_from_query(request.uri().query()).ok_or(StatusCode::UNAUTHORIZED)?;
    let ticket = context
        .tickets
        .lock()
        .remove(token)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if ticket.expires_at <= Instant::now()
        || is_input_generation_invalidated(
            &context.invalidated,
            &ticket.session_id,
            ticket.generation,
        )
        || running_generation(&context.terminal_state, &ticket.session_id)
            != Some(ticket.generation)
    {
        return Err(StatusCode::GONE);
    }
    Ok(ticket)
}

fn input_rejection(status: StatusCode) -> ErrorResponse {
    let mut response = ErrorResponse::new(Some("terminal input handshake rejected".to_owned()));
    *response.status_mut() = status;
    response
}

fn input_token_from_query(query: Option<&str>) -> Option<&str> {
    let token = query?.strip_prefix("token=")?;
    (token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(token)
}

fn is_allowed_input_origin(origin: &str) -> bool {
    matches!(
        origin,
        "http://tauri.localhost"
            | "https://tauri.localhost"
            | "tauri://localhost"
            | "http://127.0.0.1:1420"
            | "http://localhost:1420"
    )
}

fn new_input_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn is_input_generation_invalidated(
    invalidated: &Mutex<HashMap<String, InvalidatedGeneration>>,
    session_id: &str,
    generation: u64,
) -> bool {
    invalidated
        .lock()
        .get(session_id)
        .is_some_and(|entry| entry.generation >= generation)
}

fn prune_invalidations(invalidated: &mut HashMap<String, InvalidatedGeneration>, now: Instant) {
    invalidated.retain(|_, entry| {
        now.saturating_duration_since(entry.invalidated_at) < INPUT_INVALIDATION_TTL
    });
    while invalidated.len() >= MAX_RETAINED_TERMINALS {
        let Some(oldest) = invalidated
            .iter()
            .min_by_key(|(_, entry)| entry.invalidated_at)
            .map(|(session_id, _)| session_id.clone())
        else {
            break;
        };
        invalidated.remove(&oldest);
    }
}

fn running_generation(state: &Arc<RwLock<TerminalRegistryState>>, session_id: &str) -> Option<u64> {
    state
        .read()
        .active
        .get(session_id)
        .and_then(|slot| match slot {
            TerminalSlot::Running(session) => Some(session.generation),
            TerminalSlot::Starting { .. } => None,
        })
}

fn write_terminal_bytes(
    state: &Arc<RwLock<TerminalRegistryState>>,
    session_id: &str,
    generation: u64,
    data: &[u8],
) -> AppResult<()> {
    if data.len() > MAX_CHUNK {
        return Err(AppError::Validation(
            "terminal input exceeds 64 KiB".to_owned(),
        ));
    }
    let writer = state
        .read()
        .active
        .get(session_id)
        .and_then(|slot| match slot {
            TerminalSlot::Running(session) if session.generation == generation => {
                Some(Arc::clone(&session.writer))
            }
            TerminalSlot::Running(_) | TerminalSlot::Starting { .. } => None,
        })
        .ok_or_else(|| AppError::NotFound(format!("running terminal session {session_id}")))?;
    let mut writer = writer.lock();
    writer.write_all(data)?;
    writer.flush()?;
    Ok(())
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
        let _operation = self.begin_host_operation(&host.id)?;
        validate_size(rows, cols)?;
        let session_id = Uuid::new_v4().to_string();
        let generation = self.reserve_new(&session_id, &host.id)?;
        self.spawn_reserved(
            app,
            host,
            runtime,
            TerminalSpawnOptions {
                session_id,
                generation,
                rows,
                cols,
                remote_command: None,
                title: None,
            },
        )
    }

    pub fn start_command(
        &self,
        app: AppHandle,
        host: &HostProfile,
        runtime: &SshRuntime,
        options: TerminalCommandOptions,
    ) -> AppResult<TerminalSnapshot> {
        let _operation = self.begin_host_operation(&host.id)?;
        validate_size(options.rows, options.cols)?;
        let session_id = Uuid::new_v4().to_string();
        let generation = self.reserve_new(&session_id, &host.id)?;
        self.spawn_reserved(
            app,
            host,
            runtime,
            TerminalSpawnOptions {
                session_id,
                generation,
                rows: options.rows,
                cols: options.cols,
                remote_command: Some(options.remote_command),
                title: options.title,
            },
        )
    }

    pub fn reconnect(
        &self,
        app: AppHandle,
        session_id: &str,
        host: &HostProfile,
        runtime: &SshRuntime,
        rows: u16,
        cols: u16,
    ) -> AppResult<TerminalSnapshot> {
        let _operation = self.begin_host_operation(&host.id)?;
        validate_size(rows, cols)?;
        let (previous, generation) = self.reserve_reconnect(session_id, &host.id)?;
        self.spawn_reserved(
            app,
            host,
            runtime,
            TerminalSpawnOptions {
                session_id: session_id.to_owned(),
                generation,
                rows,
                cols,
                remote_command: None,
                title: Some(previous.title),
            },
        )
    }

    fn reserve_new(&self, session_id: &str, host_id: &str) -> AppResult<u64> {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let mut state = self.state.write();
        if state.active.contains_key(session_id) || state.snapshots.contains_key(session_id) {
            return Err(AppError::State(
                "terminal session identifier collision".to_owned(),
            ));
        }
        reserve_slot(&mut state, session_id, host_id, generation)?;
        Ok(generation)
    }

    fn reserve_reconnect(
        &self,
        session_id: &str,
        host_id: &str,
    ) -> AppResult<(TerminalSnapshot, u64)> {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let mut state = self.state.write();
        let previous = state
            .snapshots
            .get(session_id)
            .map(|record| record.snapshot.clone())
            .ok_or_else(|| AppError::NotFound(format!("terminal session {session_id}")))?;
        if previous.host_id != host_id {
            return Err(AppError::State(
                "terminal session host no longer matches its profile".to_owned(),
            ));
        }
        if state.active.contains_key(session_id) {
            return Err(AppError::Validation(
                "terminal session is already running or starting".to_owned(),
            ));
        }
        reserve_slot(&mut state, session_id, host_id, generation)?;
        Ok((previous, generation))
    }

    fn spawn_reserved(
        &self,
        app: AppHandle,
        host: &HostProfile,
        runtime: &SshRuntime,
        options: TerminalSpawnOptions,
    ) -> AppResult<TerminalSnapshot> {
        let (program, args) =
            match runtime.terminal_command_with_remote(host, options.remote_command.as_deref()) {
                Ok(command) => command,
                Err(error) => {
                    self.release_reservation(&options.session_id, options.generation);
                    return Err(error);
                }
            };
        let pair = match native_pty_system().openpty(PtySize {
            rows: options.rows,
            cols: options.cols,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(pair) => pair,
            Err(error) => {
                self.release_reservation(&options.session_id, options.generation);
                return Err(AppError::Process(format!("failed to open PTY: {error}")));
            }
        };
        let mut command = CommandBuilder::new(&program);
        command.args(args);
        let child = match pair.slave.spawn_command(command) {
            Ok(child) => child,
            Err(error) => {
                self.release_reservation(&options.session_id, options.generation);
                return Err(AppError::Process(format!(
                    "failed to start ssh in PTY: {error}"
                )));
            }
        };
        let cleanup = SpawnCleanup {
            killer: Arc::new(Mutex::new(child.clone_killer())),
            child: Arc::new(Mutex::new(Some(child))),
            terminated: Arc::new(AtomicBool::new(false)),
        };
        drop(pair.slave);

        if !self.register_spawn_cleanup(&options.session_id, options.generation, cleanup.clone()) {
            return Err(self.abort_spawn(
                &options.session_id,
                options.generation,
                &cleanup,
                false,
                AppError::State("terminal session was closed while starting".to_owned()),
            ));
        }

        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                return Err(self.abort_spawn(
                    &options.session_id,
                    options.generation,
                    &cleanup,
                    false,
                    AppError::Process(format!("failed to clone PTY reader: {error}")),
                ));
            }
        };
        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(error) => {
                return Err(self.abort_spawn(
                    &options.session_id,
                    options.generation,
                    &cleanup,
                    false,
                    AppError::Process(format!("failed to take PTY writer: {error}")),
                ));
            }
        };

        let snapshot = TerminalSnapshot {
            session_id: options.session_id.clone(),
            host_id: host.id.clone(),
            alias: host.alias.clone(),
            cwd: host.default_workspace.clone(),
            title: options
                .title
                .unwrap_or_else(|| format!("{} · {}", host.alias, host.default_workspace)),
            state: TerminalState::Running,
            exit_code: None,
            error: None,
        };
        let session = TerminalSession {
            generation: options.generation,
            host_id: host.id.clone(),
            writer: Arc::new(Mutex::new(writer)),
            master: Arc::new(Mutex::new(pair.master)),
            cleanup: cleanup.clone(),
        };

        let (reader_start_tx, reader_start_rx) = mpsc::sync_channel(0);
        let (wait_start_tx, wait_start_rx) = mpsc::sync_channel(0);
        let (reader_done_tx, reader_done_rx) = mpsc::sync_channel(1);

        let read_app = app.clone();
        let read_id = options.session_id.clone();
        let read_generation = options.generation;
        let read_state = Arc::clone(&self.state);
        let reader_thread = thread::Builder::new()
            .name("remotedeck-terminal-reader".to_owned())
            .spawn(move || {
                if reader_start_rx.recv().is_err() {
                    let _ = reader_done_tx.send(());
                    return;
                }
                let mut buffer = vec![0_u8; MAX_CHUNK];
                let mut decoder = Utf8StreamDecoder::default();
                loop {
                    if !is_running_generation(&read_state, &read_id, read_generation) {
                        break;
                    }
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            for data in decoder.push(&buffer[..read]) {
                                if !is_running_generation(&read_state, &read_id, read_generation) {
                                    break;
                                }
                                emit(
                                    &read_app,
                                    TerminalEvent {
                                        session_id: read_id.clone(),
                                        kind: TerminalEventKind::Output,
                                        data: Some(data),
                                        exit_code: None,
                                        message: None,
                                        snapshot: None,
                                    },
                                );
                            }
                        }
                        Err(error) => {
                            if is_running_generation(&read_state, &read_id, read_generation) {
                                emit(
                                    &read_app,
                                    TerminalEvent {
                                        session_id: read_id.clone(),
                                        kind: TerminalEventKind::Error,
                                        data: None,
                                        exit_code: None,
                                        message: Some(format!("PTY read failed: {error}")),
                                        snapshot: None,
                                    },
                                );
                            }
                            break;
                        }
                    }
                }
                if let Some(data) = decoder.finish()
                    && is_running_generation(&read_state, &read_id, read_generation)
                {
                    emit(
                        &read_app,
                        TerminalEvent {
                            session_id: read_id,
                            kind: TerminalEventKind::Output,
                            data: Some(data),
                            exit_code: None,
                            message: None,
                            snapshot: None,
                        },
                    );
                }
                let _ = reader_done_tx.send(());
            });
        let _reader_thread = match reader_thread {
            Ok(handle) => handle,
            Err(error) => {
                return Err(self.abort_spawn(
                    &options.session_id,
                    options.generation,
                    &cleanup,
                    false,
                    AppError::Process(format!("failed to start PTY reader thread: {error}")),
                ));
            }
        };

        let wait_app = app.clone();
        let wait_id = options.session_id.clone();
        let wait_generation = options.generation;
        let wait_state = Arc::clone(&self.state);
        let wait_input = Arc::clone(&self.input);
        let wait_cleanup = cleanup.clone();
        let wait_child = Arc::clone(&cleanup.child);
        let waiter_thread = thread::Builder::new()
            .name("remotedeck-terminal-waiter".to_owned())
            .spawn(move || {
                if wait_start_rx.recv().is_err() {
                    return;
                }
                let Some(mut child) = wait_child.lock().take() else {
                    return;
                };
                match child.wait() {
                    Ok(status) => {
                        wait_cleanup.terminated.store(true, Ordering::Release);
                        let _ = reader_done_rx.recv_timeout(Duration::from_secs(2));
                        wait_input.invalidate_session(&wait_id, wait_generation);
                        if let Some(event) =
                            finalize_exit(&wait_state, &wait_id, wait_generation, status)
                        {
                            emit(&wait_app, event);
                        }
                    }
                    Err(error) => {
                        *wait_child.lock() = Some(child);
                        if let Some(event) =
                            record_wait_error(&wait_state, &wait_id, wait_generation, error)
                        {
                            emit(&wait_app, event);
                        }
                    }
                }
            });
        let _waiter_thread = match waiter_thread {
            Ok(handle) => handle,
            Err(error) => {
                drop(reader_start_tx);
                return Err(self.abort_spawn(
                    &options.session_id,
                    options.generation,
                    &cleanup,
                    false,
                    AppError::Process(format!("failed to start PTY waiter thread: {error}")),
                ));
            }
        };

        if !self.commit_running(
            &options.session_id,
            options.generation,
            session,
            snapshot.clone(),
        ) {
            drop(reader_start_tx);
            drop(wait_start_tx);
            return Err(self.abort_spawn(
                &options.session_id,
                options.generation,
                &cleanup,
                false,
                AppError::State("terminal session was closed while starting".to_owned()),
            ));
        }

        emit(
            &app,
            TerminalEvent {
                session_id: options.session_id.clone(),
                kind: TerminalEventKind::Started,
                data: None,
                exit_code: None,
                message: None,
                snapshot: Some(snapshot.clone()),
            },
        );
        if reader_start_tx.send(()).is_err() {
            drop(wait_start_tx);
            return Err(self.abort_spawn(
                &options.session_id,
                options.generation,
                &cleanup,
                true,
                AppError::Process("PTY reader thread stopped before startup".to_owned()),
            ));
        }
        if wait_start_tx.send(()).is_err() {
            return Err(self.abort_spawn(
                &options.session_id,
                options.generation,
                &cleanup,
                true,
                AppError::Process("PTY waiter thread stopped before startup".to_owned()),
            ));
        }
        Ok(snapshot)
    }

    fn register_spawn_cleanup(
        &self,
        session_id: &str,
        generation: u64,
        cleanup: SpawnCleanup,
    ) -> bool {
        let mut state = self.state.write();
        let Some(TerminalSlot::Starting {
            generation: current,
            close_requested,
            cleanup: registered,
            ..
        }) = state.active.get_mut(session_id)
        else {
            return false;
        };
        if *current != generation {
            return false;
        }
        *registered = Some(cleanup);
        !*close_requested
    }

    fn commit_running(
        &self,
        session_id: &str,
        generation: u64,
        session: TerminalSession,
        snapshot: TerminalSnapshot,
    ) -> bool {
        let mut state = self.state.write();
        let can_commit = matches!(
            state.active.get(session_id),
            Some(TerminalSlot::Starting {
                generation: current,
                close_requested: false,
                ..
            }) if *current == generation
        );
        if !can_commit {
            return false;
        }
        state
            .active
            .insert(session_id.to_owned(), TerminalSlot::Running(session));
        state.snapshots.insert(
            session_id.to_owned(),
            TerminalRecord {
                generation,
                snapshot,
            },
        );
        prune_terminal_records(&mut state, MAX_RETAINED_TERMINALS);
        true
    }

    fn release_reservation(&self, session_id: &str, generation: u64) {
        let mut state = self.state.write();
        let close_requested = match state.active.get(session_id) {
            Some(TerminalSlot::Starting {
                generation: current,
                close_requested,
                ..
            }) if *current == generation => *close_requested,
            _ => return,
        };
        state.active.remove(session_id);
        if close_requested {
            state.snapshots.remove(session_id);
        }
    }

    fn abort_spawn(
        &self,
        session_id: &str,
        generation: u64,
        cleanup: &SpawnCleanup,
        remove_snapshot: bool,
        cause: AppError,
    ) -> AppError {
        if !is_current_generation(&self.state, session_id, generation) {
            return cause;
        }
        self.input.invalidate_session(session_id, generation);
        if let Err(cleanup_error) = terminate_cleanup(cleanup) {
            return AppError::Process(format!(
                "{cause}; failed to clean up the owned PTY child: {cleanup_error}"
            ));
        }
        let mut state = self.state.write();
        let close_requested = match state.active.get(session_id) {
            Some(TerminalSlot::Starting {
                generation: current,
                close_requested,
                ..
            }) if *current == generation => *close_requested,
            Some(slot) if slot.generation() == generation => false,
            _ => return cause,
        };
        state.active.remove(session_id);
        if remove_snapshot || close_requested {
            state.snapshots.remove(session_id);
        }
        cause
    }

    pub fn open_input(&self, session_id: &str) -> AppResult<TerminalInputEndpoint> {
        self.input.open_endpoint(&self.state, session_id)
    }

    pub fn resize(&self, session_id: &str, rows: u16, cols: u16) -> AppResult<()> {
        validate_size(rows, cols)?;
        let master = self
            .state
            .read()
            .active
            .get(session_id)
            .and_then(|slot| match slot {
                TerminalSlot::Running(session) => Some(Arc::clone(&session.master)),
                TerminalSlot::Starting { .. } => None,
            })
            .ok_or_else(|| AppError::NotFound(format!("running terminal session {session_id}")))?;
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
        let (generation, cleanup) = {
            let mut state = self.state.write();
            match state.active.get_mut(session_id) {
                Some(TerminalSlot::Starting {
                    generation,
                    close_requested,
                    cleanup,
                    ..
                }) => {
                    *close_requested = true;
                    (*generation, cleanup.clone())
                }
                Some(TerminalSlot::Running(session)) => {
                    (session.generation, Some(session.cleanup.clone()))
                }
                None => {
                    if state.snapshots.remove(session_id).is_some() {
                        return Ok(());
                    }
                    return Err(AppError::NotFound(format!("terminal session {session_id}")));
                }
            }
        };

        let Some(cleanup) = cleanup else {
            self.input.invalidate_session(session_id, generation);
            return Ok(());
        };
        self.input.invalidate_session(session_id, generation);
        terminate_cleanup(&cleanup)?;

        let mut state = self.state.write();
        if state
            .active
            .get(session_id)
            .is_some_and(|slot| slot.generation() == generation)
        {
            state.active.remove(session_id);
            state.snapshots.remove(session_id);
        }
        Ok(())
    }

    pub fn list(&self) -> Vec<TerminalSnapshot> {
        let mut snapshots = self
            .state
            .read()
            .snapshots
            .values()
            .map(|record| record.snapshot.clone())
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| {
            left.title
                .cmp(&right.title)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        snapshots
    }

    fn begin_host_operation(
        &self,
        host_id: &str,
    ) -> AppResult<crate::host_operation::HostOperationLease> {
        self.host_operations.begin(host_id).ok_or_else(|| {
            AppError::State(format!(
                "terminal operations for host {host_id} are being retired"
            ))
        })
    }

    /// Closes the resolve-to-registration race before repository deletion.
    /// Every start that crossed the barrier must finish registration first;
    /// the stable registry can then be drained without a late PTY escaping.
    pub async fn retire_host(&self, host_id: &str) -> AppResult<()> {
        self.host_operations.retire(host_id);
        tokio::time::timeout(
            HOST_RETIREMENT_TIMEOUT,
            self.host_operations.wait_idle(host_id),
        )
        .await
        .map_err(|_| {
            AppError::Timeout(
                "timed out waiting for terminal startup operations to settle".to_owned(),
            )
        })?;
        self.stop_for_host(host_id)
    }

    pub fn restore_host(&self, host_id: &str) {
        self.host_operations.restore(host_id);
    }

    fn stop_for_host(&self, host_id: &str) -> AppResult<()> {
        let mut ids = {
            let state = self.state.read();
            let mut ids = state
                .active
                .iter()
                .filter(|(_, slot)| slot.host_id() == host_id)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            ids.extend(
                state
                    .snapshots
                    .iter()
                    .filter(|(_, record)| record.snapshot.host_id == host_id)
                    .map(|(id, _)| id.clone()),
            );
            ids
        };
        ids.sort();
        ids.dedup();
        let mut errors = Vec::new();
        for id in ids {
            if let Err(error) = self.close(&id) {
                errors.push(format!("{id}: {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(AppError::Process(format!(
                "failed to close terminal sessions: {}",
                errors.join("; ")
            )))
        }
    }

    pub fn stop_all(&self) {
        let cleanups = {
            let mut state = self.state.write();
            let cleanups = state
                .active
                .values()
                .filter_map(|slot| match slot {
                    TerminalSlot::Starting { cleanup, .. } => cleanup.clone(),
                    TerminalSlot::Running(session) => Some(session.cleanup.clone()),
                })
                .collect::<Vec<_>>();
            state.active.clear();
            state.snapshots.clear();
            cleanups
        };
        self.input.shutdown();
        thread::scope(|scope| {
            let handles = cleanups
                .iter()
                .map(|cleanup| scope.spawn(move || terminate_cleanup(cleanup)))
                .collect::<Vec<_>>();
            for handle in handles {
                let _ = handle.join();
            }
        });
    }
}

fn reserve_slot(
    state: &mut TerminalRegistryState,
    session_id: &str,
    host_id: &str,
    generation: u64,
) -> AppResult<()> {
    if state.active.len() >= MAX_ACTIVE_TERMINALS {
        return Err(AppError::Validation(format!(
            "at most {MAX_ACTIVE_TERMINALS} active terminal sessions are allowed"
        )));
    }
    state.active.insert(
        session_id.to_owned(),
        TerminalSlot::Starting {
            generation,
            host_id: host_id.to_owned(),
            close_requested: false,
            cleanup: None,
        },
    );
    Ok(())
}

fn prune_terminal_records(state: &mut TerminalRegistryState, limit: usize) {
    while state.snapshots.len() > limit {
        let oldest_terminal = state
            .snapshots
            .iter()
            .filter(|(id, _)| !state.active.contains_key(id.as_str()))
            .min_by_key(|(_, record)| record.generation)
            .map(|(id, _)| id.clone());
        let Some(id) = oldest_terminal else {
            break;
        };
        state.snapshots.remove(&id);
    }
}

fn is_current_generation(
    state: &Arc<RwLock<TerminalRegistryState>>,
    session_id: &str,
    generation: u64,
) -> bool {
    state
        .read()
        .active
        .get(session_id)
        .is_some_and(|slot| slot.generation() == generation)
}

fn is_running_generation(
    state: &Arc<RwLock<TerminalRegistryState>>,
    session_id: &str,
    generation: u64,
) -> bool {
    matches!(
        state.read().active.get(session_id),
        Some(TerminalSlot::Running(session)) if session.generation == generation
    )
}

fn terminate_cleanup(cleanup: &SpawnCleanup) -> AppResult<()> {
    if cleanup.terminated.load(Ordering::Acquire) {
        return Ok(());
    }

    let mut killer = cleanup.killer.lock();
    if cleanup.terminated.load(Ordering::Acquire) {
        return Ok(());
    }
    let kill_error = killer.kill().err();
    let mut child_slot = cleanup.child.lock();
    if let Some(mut child) = child_slot.take() {
        if let Some(error) = kill_error {
            match child.try_wait() {
                Ok(Some(_)) => {}
                _ => {
                    *child_slot = Some(child);
                    return Err(AppError::Process(format!(
                        "failed to close terminal: {error}"
                    )));
                }
            }
        } else {
            let deadline = Instant::now() + CHILD_TERMINATION_TIMEOUT;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() >= deadline => {
                        *child_slot = Some(child);
                        return Err(AppError::Process(
                            "timed out waiting for the owned terminal child to exit".to_owned(),
                        ));
                    }
                    Ok(None) => thread::sleep(CHILD_TERMINATION_POLL_INTERVAL),
                    Err(error) => {
                        *child_slot = Some(child);
                        return Err(AppError::Process(format!(
                            "failed to reap the owned terminal child: {error}"
                        )));
                    }
                }
            }
        }
    } else if let Some(error) = kill_error {
        return Err(AppError::Process(format!(
            "failed to close terminal: {error}"
        )));
    }

    cleanup.terminated.store(true, Ordering::Release);
    Ok(())
}

fn finalize_exit(
    state: &Arc<RwLock<TerminalRegistryState>>,
    session_id: &str,
    generation: u64,
    status: ExitStatus,
) -> Option<TerminalEvent> {
    let mut state = state.write();
    let current = matches!(
        state.active.get(session_id),
        Some(TerminalSlot::Running(session)) if session.generation == generation
    ) && state
        .snapshots
        .get(session_id)
        .is_some_and(|record| record.generation == generation);
    if !current {
        return None;
    }
    state.active.remove(session_id);
    let record = state.snapshots.get_mut(session_id)?;
    let exit_code = i32::try_from(status.exit_code()).ok();
    record.snapshot.state = if exit_code == Some(0) {
        TerminalState::Closed
    } else {
        TerminalState::Offline
    };
    record.snapshot.exit_code = exit_code;
    record.snapshot.error = status.signal().map(ToOwned::to_owned);
    Some(TerminalEvent {
        session_id: session_id.to_owned(),
        kind: TerminalEventKind::Exit,
        data: None,
        exit_code,
        message: record.snapshot.error.clone(),
        snapshot: Some(record.snapshot.clone()),
    })
}

fn record_wait_error(
    state: &Arc<RwLock<TerminalRegistryState>>,
    session_id: &str,
    generation: u64,
    error: std::io::Error,
) -> Option<TerminalEvent> {
    let mut state = state.write();
    let current = matches!(
        state.active.get(session_id),
        Some(TerminalSlot::Running(session)) if session.generation == generation
    );
    if !current {
        return None;
    }
    let record = state.snapshots.get_mut(session_id)?;
    if record.generation != generation {
        return None;
    }
    let message = format!("failed to wait for PTY child: {error}");
    record.snapshot.state = TerminalState::Failed;
    record.snapshot.error = Some(message.clone());
    Some(TerminalEvent {
        session_id: session_id.to_owned(),
        kind: TerminalEventKind::Error,
        data: None,
        exit_code: None,
        message: Some(message),
        snapshot: Some(record.snapshot.clone()),
    })
}

#[derive(Default)]
struct Utf8StreamDecoder {
    pending: Vec<u8>,
}

impl Utf8StreamDecoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(bytes);
        let mut output = Vec::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(value) => {
                    if !value.is_empty() {
                        output.push(value.to_owned());
                    }
                    self.pending.clear();
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    let invalid = error.error_len();
                    if valid > 0 {
                        output.push(String::from_utf8_lossy(&self.pending[..valid]).into_owned());
                        self.pending.drain(..valid);
                    }
                    match invalid {
                        Some(invalid) => {
                            output.push(
                                String::from_utf8_lossy(&self.pending[..invalid]).into_owned(),
                            );
                            self.pending.drain(..invalid);
                        }
                        None => break,
                    }
                }
            }
        }
        output
    }

    fn finish(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            None
        } else {
            let bytes = std::mem::take(&mut self.pending);
            Some(String::from_utf8_lossy(&bytes).into_owned())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io, sync::Barrier};
    use tungstenite::{client, client::IntoClientRequest, http::HeaderValue};

    fn snapshot(session_id: &str, state: TerminalState) -> TerminalSnapshot {
        TerminalSnapshot {
            session_id: session_id.to_owned(),
            host_id: "host".to_owned(),
            alias: "lab".to_owned(),
            cwd: "~".to_owned(),
            title: session_id.to_owned(),
            state,
            exit_code: None,
            error: None,
        }
    }

    #[derive(Debug)]
    struct FailingKiller;

    impl ChildKiller for FailingKiller {
        fn kill(&mut self) -> io::Result<()> {
            Err(io::Error::other("synthetic kill failure"))
        }

        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(Self)
        }
    }

    #[derive(Debug)]
    struct NoopKiller;

    impl ChildKiller for NoopKiller {
        fn kill(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(Self)
        }
    }

    fn registry_with_running_pty() -> (TerminalRegistry, Box<dyn portable_pty::SlavePty>) {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("test PTY");
        let writer = pair.master.take_writer().expect("test PTY writer");
        let registry = TerminalRegistry::default();
        registry.state.write().active.insert(
            "session".to_owned(),
            TerminalSlot::Running(TerminalSession {
                generation: 7,
                host_id: "host".to_owned(),
                writer: Arc::new(Mutex::new(writer)),
                master: Arc::new(Mutex::new(pair.master)),
                cleanup: SpawnCleanup {
                    killer: Arc::new(Mutex::new(Box::new(NoopKiller))),
                    child: Arc::new(Mutex::new(None)),
                    terminated: Arc::new(AtomicBool::new(true)),
                },
            }),
        );
        (registry, pair.slave)
    }

    #[test]
    fn utf8_decoder_preserves_characters_split_across_reads() {
        let text = "中文🙂".as_bytes();
        let mut decoder = Utf8StreamDecoder::default();
        let mut output = Vec::new();
        for byte in text {
            output.extend(decoder.push(&[*byte]));
        }
        if let Some(tail) = decoder.finish() {
            output.push(tail);
        }
        assert_eq!(output.concat(), "中文🙂");
    }

    #[test]
    fn utf8_decoder_replaces_only_truly_invalid_bytes() {
        let mut decoder = Utf8StreamDecoder::default();
        let output = decoder.push(&[b'a', 0xff, b'b']).concat();
        assert_eq!(output, "a�b");
        assert!(decoder.finish().is_none());
    }

    #[test]
    fn dimensions_are_bounded() {
        assert!(validate_size(24, 80).is_ok());
        assert!(validate_size(1, 80).is_err());
        assert!(validate_size(24, 1001).is_err());
    }

    #[test]
    fn input_origin_and_token_parsing_are_strict() {
        assert!(is_allowed_input_origin("http://tauri.localhost"));
        assert!(is_allowed_input_origin("https://tauri.localhost"));
        assert!(is_allowed_input_origin("tauri://localhost"));
        assert!(is_allowed_input_origin("http://127.0.0.1:1420"));
        assert!(!is_allowed_input_origin("https://attacker.example"));
        assert!(!is_allowed_input_origin("http://127.0.0.1:1421"));

        let token = new_input_token();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(
            input_token_from_query(Some(&format!("token={token}"))),
            Some(token.as_str())
        );
        assert!(input_token_from_query(Some(&format!("token={token}&extra=1"))).is_none());
        assert!(input_token_from_query(Some("token=short")).is_none());
    }

    #[test]
    fn input_endpoint_is_loopback_single_use_and_origin_bound() {
        let (registry, _slave) = registry_with_running_pty();
        let rejected_endpoint = registry.open_input("session").expect("input endpoint");
        let address = registry
            .input
            .server
            .lock()
            .as_ref()
            .expect("input server")
            .address;
        assert_eq!(address.ip(), Ipv4Addr::LOCALHOST);

        let mut rejected_request = rejected_endpoint
            .url
            .clone()
            .into_client_request()
            .expect("client request");
        rejected_request.headers_mut().insert(
            "origin",
            HeaderValue::from_static("https://attacker.example"),
        );
        let rejected_stream = TcpStream::connect(address).expect("loopback input server");
        assert!(client(rejected_request, rejected_stream).is_err());

        let mut accepted_request = rejected_endpoint
            .url
            .clone()
            .into_client_request()
            .expect("client request");
        accepted_request
            .headers_mut()
            .insert("origin", HeaderValue::from_static("http://tauri.localhost"));
        let accepted_stream = TcpStream::connect(address).expect("loopback input server");
        let (mut websocket, _) =
            client(accepted_request.clone(), accepted_stream).expect("authenticated WebSocket");
        websocket
            .send(Message::Binary(Vec::from("password\r").into()))
            .expect("bounded terminal input frame");
        websocket.close(None).expect("close input WebSocket");

        let replay_stream = TcpStream::connect(address).expect("loopback input server");
        assert!(client(accepted_request, replay_stream).is_err());
        registry.stop_all();
    }

    #[test]
    fn active_session_reservations_are_bounded() {
        let registry = TerminalRegistry::default();
        for index in 0..MAX_ACTIVE_TERMINALS {
            assert!(
                registry
                    .reserve_new(&format!("session-{index}"), "host")
                    .is_ok()
            );
        }
        assert!(registry.reserve_new("one-too-many", "host").is_err());
        assert_eq!(registry.state.read().active.len(), MAX_ACTIVE_TERMINALS);
    }

    #[test]
    fn concurrent_reconnects_share_one_atomic_reservation() {
        let registry = TerminalRegistry::default();
        registry.state.write().snapshots.insert(
            "session".to_owned(),
            TerminalRecord {
                generation: 1,
                snapshot: snapshot("session", TerminalState::Offline),
            },
        );
        let barrier = Arc::new(Barrier::new(3));
        let attempts = (0..2)
            .map(|_| {
                let registry = registry.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    registry.reserve_reconnect("session", "host").is_ok()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let successes = attempts
            .into_iter()
            .map(|attempt| attempt.join().expect("reservation thread"))
            .filter(|succeeded| *succeeded)
            .count();
        assert_eq!(successes, 1);
        assert_eq!(registry.state.read().active.len(), 1);
    }

    #[test]
    fn stale_waiter_cannot_remove_or_overwrite_new_generation() {
        let state = Arc::new(RwLock::new(TerminalRegistryState::default()));
        {
            let mut registry = state.write();
            registry.active.insert(
                "session".to_owned(),
                TerminalSlot::Starting {
                    generation: 2,
                    host_id: "host".to_owned(),
                    close_requested: false,
                    cleanup: None,
                },
            );
            registry.snapshots.insert(
                "session".to_owned(),
                TerminalRecord {
                    generation: 2,
                    snapshot: snapshot("session", TerminalState::Running),
                },
            );
        }
        assert!(finalize_exit(&state, "session", 1, ExitStatus::with_exit_code(0)).is_none());
        let registry = state.read();
        assert_eq!(registry.active["session"].generation(), 2);
        assert_eq!(
            registry.snapshots["session"].snapshot.state,
            TerminalState::Running
        );
    }

    #[test]
    fn history_pruning_keeps_running_and_newest_terminal_records() {
        let mut state = TerminalRegistryState::default();
        for generation in 0..5 {
            let id = format!("closed-{generation}");
            state.snapshots.insert(
                id.clone(),
                TerminalRecord {
                    generation,
                    snapshot: snapshot(&id, TerminalState::Closed),
                },
            );
        }
        state.snapshots.insert(
            "active-failed".to_owned(),
            TerminalRecord {
                generation: 100,
                snapshot: snapshot("active-failed", TerminalState::Failed),
            },
        );
        state.active.insert(
            "active-failed".to_owned(),
            TerminalSlot::Starting {
                generation: 100,
                host_id: "host".to_owned(),
                close_requested: false,
                cleanup: None,
            },
        );
        prune_terminal_records(&mut state, 3);
        assert_eq!(state.snapshots.len(), 3);
        assert!(state.snapshots.contains_key("active-failed"));
        assert!(state.snapshots.contains_key("closed-4"));
        assert!(state.snapshots.contains_key("closed-3"));
    }

    #[test]
    fn close_keeps_tracking_when_owned_child_kill_fails() {
        let registry = TerminalRegistry::default();
        let cleanup = SpawnCleanup {
            killer: Arc::new(Mutex::new(Box::new(FailingKiller))),
            child: Arc::new(Mutex::new(None)),
            terminated: Arc::new(AtomicBool::new(false)),
        };
        {
            let mut state = registry.state.write();
            state.active.insert(
                "session".to_owned(),
                TerminalSlot::Starting {
                    generation: 7,
                    host_id: "host".to_owned(),
                    close_requested: false,
                    cleanup: Some(cleanup),
                },
            );
            state.snapshots.insert(
                "session".to_owned(),
                TerminalRecord {
                    generation: 6,
                    snapshot: snapshot("session", TerminalState::Offline),
                },
            );
        }
        assert!(registry.close("session").is_err());
        let state = registry.state.read();
        assert!(state.active.contains_key("session"));
        assert!(state.snapshots.contains_key("session"));
    }
}
