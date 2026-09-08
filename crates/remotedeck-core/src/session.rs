use crate::{
    error::{AppError, AppResult},
    events::EventSink,
    host_operation::HostOperationBarrier,
    model::{HostProfile, TerminalEvent, TerminalEventKind, TerminalSnapshot, TerminalState},
    ssh::SshRuntime,
};
use parking_lot::{Mutex, RwLock};
use portable_pty::{
    Child, ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system,
};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap, VecDeque},
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::broadcast;
use uuid::Uuid;

pub const TERMINAL_EVENT: &str = "terminal-event";
const MAX_CHUNK: usize = 64 * 1024;
const MAX_ACTIVE_TERMINALS: usize = 32;
const MAX_RETAINED_TERMINALS: usize = 256;
const MAX_TERMINAL_REPLAY_BYTES: usize = 1024 * 1024;
const TERMINAL_BROADCAST_BUFFER: usize = 512;
const MAX_INPUT_LEASES: usize = 64;
const CONPTY_INITIAL_CURSOR_QUERY: &[u8] = b"\x1b[6n";
const CONPTY_INITIAL_CURSOR_RESPONSE: &[u8] = b"\x1b[1;1R";
const CHILD_TERMINATION_TIMEOUT: Duration = Duration::from_secs(2);
const CHILD_TERMINATION_POLL_INTERVAL: Duration = Duration::from_millis(10);
const HOST_RETIREMENT_TIMEOUT: Duration = Duration::from_secs(10);

/// A terminal event together with its generation-local monotonic sequence.
///
/// Sequences let a WebSocket client reconnect without replaying output it has
/// already rendered. A new PTY generation starts at sequence one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SequencedTerminalEvent {
    pub sequence: u64,
    pub generation: u64,
    pub event: TerminalEvent,
}

/// A live terminal subscription. Dropping the receiver only detaches the
/// browser transport; it never closes the underlying PTY.
pub struct TerminalAttachment {
    pub generation: u64,
    pub lease: u64,
    pub replay: Vec<SequencedTerminalEvent>,
    pub receiver: broadcast::Receiver<SequencedTerminalEvent>,
}

impl std::fmt::Debug for TerminalAttachment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalAttachment")
            .field("generation", &self.generation)
            .field("lease", &self.lease)
            .field("replay", &self.replay)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct TerminalRegistry {
    state: Arc<RwLock<TerminalRegistryState>>,
    next_generation: Arc<AtomicU64>,
    host_operations: HostOperationBarrier,
}

impl Default for TerminalRegistry {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(TerminalRegistryState::default())),
            next_generation: Arc::new(AtomicU64::new(0)),
            host_operations: HostOperationBarrier::default(),
        }
    }
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
    revision: u64,
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

#[derive(Clone)]
struct TerminalSession {
    generation: u64,
    host_id: String,
    input: Arc<Mutex<TerminalInputState>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    cleanup: SpawnCleanup,
    stream: Arc<Mutex<TerminalEventStream>>,
}

struct TerminalInputState {
    writer: Box<dyn Write + Send>,
    active_lease: Option<u64>,
    issued_leases: BTreeSet<u64>,
    next_lease: u64,
}

impl TerminalInputState {
    fn issue_lease(&mut self) -> AppResult<u64> {
        if self.issued_leases.len() >= MAX_INPUT_LEASES {
            return Err(AppError::Validation(format!(
                "at most {MAX_INPUT_LEASES} terminal input attachments are allowed"
            )));
        }
        loop {
            self.next_lease = self.next_lease.wrapping_add(1).max(1);
            if self.issued_leases.insert(self.next_lease) {
                return Ok(self.next_lease);
            }
        }
    }

    /// Reattaches the browser that held the most recently issued lease.
    ///
    /// This deliberately differs from a normal attachment: it may renew a
    /// dropped browser connection, but it must never reclaim input after a
    /// different attachment has taken control. The comparison and new lease
    /// activation stay under this same input lock.
    fn resume_lease(&mut self, previous_lease: u64) -> AppResult<u64> {
        if previous_lease == 0 || self.next_lease != previous_lease {
            return Err(AppError::State("terminal input control moved".to_owned()));
        }
        let lease = self.issue_lease()?;
        self.activate(lease)?;
        Ok(lease)
    }

    /// Changing the active lease happens while the exact PTY writer lock is
    /// held. Normal `TerminalRegistry::attach` calls this before returning,
    /// so an older websocket loses input authority immediately.
    fn activate(&mut self, lease: u64) -> AppResult<()> {
        if !self.issued_leases.contains(&lease) {
            return Err(AppError::Validation(
                "terminal input lease is unknown or has been released".to_owned(),
            ));
        }
        self.active_lease = Some(lease);
        self.issued_leases.retain(|candidate| *candidate >= lease);
        Ok(())
    }

    fn write(&mut self, lease: u64, bytes: &[u8]) -> AppResult<()> {
        if bytes.len() > MAX_CHUNK {
            return Err(AppError::Validation(format!(
                "terminal input exceeds the {MAX_CHUNK}-byte limit"
            )));
        }
        if self.active_lease != Some(lease) || !self.issued_leases.contains(&lease) {
            return Err(AppError::Validation(
                "terminal input lease has been superseded or released".to_owned(),
            ));
        }
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Answers the one cursor-position request emitted while Windows ConPTY
    /// initializes. This remains private to the PTY reader: browser input
    /// must continue to use a current lease through [`Self::write`].
    fn write_initial_conpty_cursor_response(&mut self) -> std::io::Result<()> {
        self.writer.write_all(CONPTY_INITIAL_CURSOR_RESPONSE)?;
        self.writer.flush()
    }

    fn release(&mut self, lease: u64) {
        self.issued_leases.remove(&lease);
        if self.active_lease == Some(lease) {
            self.active_lease = None;
        }
    }

    fn assert_active(&self, lease: u64) -> AppResult<()> {
        if self.active_lease == Some(lease) && self.issued_leases.contains(&lease) {
            Ok(())
        } else {
            Err(AppError::Validation(
                "terminal input lease does not own this session".to_owned(),
            ))
        }
    }
}

struct TerminalEventStream {
    sender: broadcast::Sender<SequencedTerminalEvent>,
    replay: VecDeque<SequencedTerminalEvent>,
    replay_bytes: usize,
    next_sequence: u64,
    dropped_through: Option<u64>,
}

impl TerminalEventStream {
    fn new() -> Self {
        let (sender, _) = broadcast::channel(TERMINAL_BROADCAST_BUFFER);
        Self {
            sender,
            replay: VecDeque::new(),
            replay_bytes: 0,
            next_sequence: 0,
            dropped_through: None,
        }
    }

    fn publish(&mut self, generation: u64, event: TerminalEvent) -> SequencedTerminalEvent {
        self.next_sequence = self.next_sequence.wrapping_add(1).max(1);
        let sequenced = SequencedTerminalEvent {
            sequence: self.next_sequence,
            generation,
            event,
        };
        self.replay_bytes = self
            .replay_bytes
            .saturating_add(replay_event_bytes(&sequenced.event));
        self.replay.push_back(sequenced.clone());
        while self.replay_bytes > MAX_TERMINAL_REPLAY_BYTES {
            let Some(removed) = self.replay.pop_front() else {
                self.replay_bytes = 0;
                break;
            };
            self.replay_bytes = self
                .replay_bytes
                .saturating_sub(replay_event_bytes(&removed.event));
            self.dropped_through = Some(removed.sequence);
        }
        let _ = self.sender.send(sequenced.clone());
        sequenced
    }

    fn attach(
        &mut self,
        session_id: &str,
        generation: u64,
        since: Option<u64>,
    ) -> (
        Vec<SequencedTerminalEvent>,
        broadcast::Receiver<SequencedTerminalEvent>,
    ) {
        let receiver = self.sender.subscribe();
        let replay_is_truncated = self.dropped_through.is_some_and(|dropped| match since {
            Some(sequence) => sequence < dropped,
            None => true,
        });
        let after = since.unwrap_or(0);
        let mut replay = Vec::with_capacity(self.replay.len().saturating_add(1));
        if replay_is_truncated {
            replay.push(SequencedTerminalEvent {
                sequence: self.dropped_through.unwrap_or(0),
                generation,
                event: TerminalEvent {
                    session_id: session_id.to_owned(),
                    kind: TerminalEventKind::ReplayTruncated,
                    data: None,
                    exit_code: None,
                    message: Some(format!(
                        "terminal replay is limited to the most recent {} bytes",
                        MAX_TERMINAL_REPLAY_BYTES
                    )),
                    snapshot: None,
                },
            });
        }
        replay.extend(
            self.replay
                .iter()
                .filter(|event| event.sequence > after)
                .cloned(),
        );
        (replay, receiver)
    }
}

fn running_session_in_state(
    state: &TerminalRegistryState,
    session_id: &str,
    expected_generation: Option<u64>,
) -> AppResult<TerminalSession> {
    state
        .active
        .get(session_id)
        .and_then(|slot| match slot {
            TerminalSlot::Running(session)
                if expected_generation
                    .is_none_or(|generation| generation == session.generation) =>
            {
                Some(session.clone())
            }
            TerminalSlot::Running(_) | TerminalSlot::Starting { .. } => None,
        })
        .ok_or_else(|| {
            let description = expected_generation.map_or_else(
                || format!("running terminal session {session_id}"),
                |generation| format!("terminal session {session_id} generation {generation}"),
            );
            AppError::NotFound(description)
        })
}

fn replay_event_bytes(event: &TerminalEvent) -> usize {
    event.data.as_ref().map_or(0, |data| data.len())
}

impl TerminalRegistry {
    pub fn start(
        &self,
        events: EventSink,
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
            events,
            host,
            runtime,
            TerminalSpawnOptions {
                session_id,
                generation,
                revision: 1,
                rows,
                cols,
                remote_command: None,
                title: None,
            },
        )
    }

    pub fn start_command(
        &self,
        events: EventSink,
        host: &HostProfile,
        runtime: &SshRuntime,
        options: TerminalCommandOptions,
    ) -> AppResult<TerminalSnapshot> {
        let _operation = self.begin_host_operation(&host.id)?;
        validate_size(options.rows, options.cols)?;
        let session_id = Uuid::new_v4().to_string();
        let generation = self.reserve_new(&session_id, &host.id)?;
        self.spawn_reserved(
            events,
            host,
            runtime,
            TerminalSpawnOptions {
                session_id,
                generation,
                revision: 1,
                rows: options.rows,
                cols: options.cols,
                remote_command: Some(options.remote_command),
                title: options.title,
            },
        )
    }

    pub fn reconnect(
        &self,
        events: EventSink,
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
            events,
            host,
            runtime,
            TerminalSpawnOptions {
                session_id: session_id.to_owned(),
                generation,
                revision: next_terminal_snapshot_revision(&previous),
                rows,
                cols,
                remote_command: None,
                title: Some(previous.title),
            },
        )
    }

    /// Attaches a browser transport to an existing running PTY. The returned
    /// lease becomes the only input authority before this method returns.
    pub fn attach(&self, session_id: &str, since: Option<u64>) -> AppResult<TerminalAttachment> {
        self.attach_with_expected_generation(session_id, None, since, None)
    }

    /// Attaches only if the running PTY still has `expected_generation`.
    ///
    /// The generation test and input-lease activation share the registry read
    /// lock. A stale transport ticket therefore cannot activate a lease on a
    /// replacement PTY and revoke that PTY's current writer.
    pub fn attach_if_generation(
        &self,
        session_id: &str,
        expected_generation: u64,
        since: Option<u64>,
    ) -> AppResult<TerminalAttachment> {
        self.attach_with_expected_generation(session_id, Some(expected_generation), since, None)
    }

    /// Reattaches an interrupted browser transport only if no other input
    /// lease has been issued since `previous_lease`.
    ///
    /// Unlike [`Self::attach_if_generation`], this is not a manual takeover.
    /// It is intended for automatic browser recovery after an abnormal socket
    /// close. A later attachment makes this return `AppError::State` without
    /// changing the current writer.
    pub fn attach_resume_if_generation(
        &self,
        session_id: &str,
        expected_generation: u64,
        since: Option<u64>,
        previous_lease: u64,
    ) -> AppResult<TerminalAttachment> {
        self.attach_with_expected_generation(
            session_id,
            Some(expected_generation),
            since,
            Some(previous_lease),
        )
    }

    fn attach_with_expected_generation(
        &self,
        session_id: &str,
        expected_generation: Option<u64>,
        since: Option<u64>,
        resume_from_lease: Option<u64>,
    ) -> AppResult<TerminalAttachment> {
        // Keep the state read lock while activating the exact PTY writer. A
        // reconnect/close needs the write lock, so it cannot swap generations
        // between the precondition check and the ownership change.
        let (session, lease) = {
            let state = self.state.read();
            let session = running_session_in_state(&state, session_id, expected_generation)?;
            let lease = {
                let mut input = session.input.lock();
                if let Some(previous_lease) = resume_from_lease {
                    input.resume_lease(previous_lease)?
                } else {
                    let lease = input.issue_lease()?;
                    input.activate(lease)?;
                    lease
                }
            };
            (session, lease)
        };
        let mut stream = session.stream.lock();
        let (replay, receiver) = stream.attach(session_id, session.generation, since);
        Ok(TerminalAttachment {
            generation: session.generation,
            lease,
            replay,
            receiver,
        })
    }

    /// Returns the generation of a currently running PTY without granting an
    /// input lease. The HTTP server binds short-lived WebSocket tickets to this
    /// value and must pass it to [`Self::attach_if_generation`] when the
    /// upgrade succeeds.
    pub fn generation(&self, session_id: &str) -> AppResult<u64> {
        Ok(self.running_session(session_id, None)?.generation)
    }

    /// Writes bytes from the current browser attachment. The input mutex
    /// validates the lease immediately before writing to the PTY.
    pub fn write_input(
        &self,
        session_id: &str,
        generation: u64,
        lease: u64,
        bytes: &[u8],
    ) -> AppResult<()> {
        let session = self.running_session(session_id, Some(generation))?;
        session.input.lock().write(lease, bytes)
    }

    /// Reports whether a browser attachment still owns terminal input. This is
    /// deliberately side-effect free for WebSocket heartbeats; an older socket
    /// must use [`Self::write_input`] to discover it has been superseded.
    pub fn input_is_current(&self, session_id: &str, generation: u64, lease: u64) -> bool {
        self.running_session(session_id, Some(generation))
            .is_ok_and(|session| session.input.lock().assert_active(lease).is_ok())
    }

    /// Releases one browser input lease without changing PTY lifetime.
    pub fn release_input(&self, session_id: &str, generation: u64, lease: u64) -> AppResult<()> {
        let session = self.running_session(session_id, Some(generation))?;
        session.input.lock().release(lease);
        Ok(())
    }

    /// Legacy resize entry point. Server transports should use
    /// [`Self::resize_input`] so a stale browser cannot resize an
    /// attachment it no longer controls.
    pub fn resize(&self, session_id: &str, rows: u16, cols: u16) -> AppResult<()> {
        let session = self.running_session(session_id, None)?;
        self.resize_session(&session, rows, cols)
    }

    pub fn resize_input(
        &self,
        session_id: &str,
        generation: u64,
        lease: u64,
        rows: u16,
        cols: u16,
    ) -> AppResult<()> {
        let session = self.running_session(session_id, Some(generation))?;
        let input = session.input.lock();
        input.assert_active(lease)?;
        self.resize_session(&session, rows, cols)
    }

    /// Compatibility spelling for callers that already adopted the first core
    /// API draft. New transports should use [`Self::resize_input`].
    pub fn resize_with_lease(
        &self,
        session_id: &str,
        generation: u64,
        lease: u64,
        rows: u16,
        cols: u16,
    ) -> AppResult<()> {
        self.resize_input(session_id, generation, lease, rows, cols)
    }

    fn resize_session(&self, session: &TerminalSession, rows: u16, cols: u16) -> AppResult<()> {
        validate_size(rows, cols)?;
        session
            .master
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
            return Ok(());
        };
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

    fn running_session(
        &self,
        session_id: &str,
        expected_generation: Option<u64>,
    ) -> AppResult<TerminalSession> {
        let state = self.state.read();
        running_session_in_state(&state, session_id, expected_generation)
    }

    fn reserve_new(&self, session_id: &str, host_id: &str) -> AppResult<u64> {
        let generation = self
            .next_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
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
        let generation = self
            .next_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
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
        events: EventSink,
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
        // This is the deliberate process-construction exception in the core.
        // portable-pty creates the Windows child with STARTUPINFOEX and binds
        // it to a ConPTY.  Routing it through process::command (or applying
        // CREATE_NO_WINDOW here) would bypass that pseudo-console setup and
        // break interactive terminal input.  It does not open an independent
        // console window: the PTY is rendered by the browser terminal.
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
            generation: options.generation,
            revision: options.revision,
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
        let stream = Arc::new(Mutex::new(TerminalEventStream::new()));
        let input = Arc::new(Mutex::new(TerminalInputState {
            writer,
            active_lease: None,
            issued_leases: BTreeSet::new(),
            next_lease: 0,
        }));
        let session = TerminalSession {
            generation: options.generation,
            host_id: host.id.clone(),
            input: Arc::clone(&input),
            master: Arc::new(Mutex::new(pair.master)),
            cleanup: cleanup.clone(),
            stream: Arc::clone(&stream),
        };

        let (reader_start_tx, reader_start_rx) = mpsc::sync_channel(0);
        let (wait_start_tx, wait_start_rx) = mpsc::sync_channel(0);
        let (reader_done_tx, reader_done_rx) = mpsc::sync_channel(1);

        let read_id = options.session_id.clone();
        let read_generation = options.generation;
        let read_state = Arc::clone(&self.state);
        let read_stream = Arc::clone(&stream);
        let read_events = events.clone();
        let read_input = Arc::clone(&input);
        let reader_thread = thread::Builder::new()
            .name("remotedeck-terminal-reader".to_owned())
            .spawn(move || {
                if reader_start_rx.recv().is_err() {
                    let _ = reader_done_tx.send(());
                    return;
                }
                let mut buffer = vec![0_u8; MAX_CHUNK];
                let mut decoder = Utf8StreamDecoder::default();
                let mut initial_conpty_handshake = InitialConPtyCursorHandshake::default();
                loop {
                    if !is_running_generation(&read_state, &read_id, read_generation) {
                        break;
                    }
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            let initial = initial_conpty_handshake.consume(&buffer[..read]);
                            if initial.answer_cursor_query {
                                if !is_running_generation(&read_state, &read_id, read_generation) {
                                    break;
                                }
                                if let Err(error) =
                                    read_input.lock().write_initial_conpty_cursor_response()
                                {
                                    if let Some(event) = record_terminal_error(
                                        &read_state,
                                        &read_id,
                                        read_generation,
                                        format!(
                                            "failed to answer initial ConPTY cursor query: {error}"
                                        ),
                                    ) {
                                        publish_terminal_event(
                                            &read_stream,
                                            &read_events,
                                            read_generation,
                                            event,
                                        );
                                    }
                                    break;
                                }
                            }
                            for data in decoder.push(initial.output.as_ref()) {
                                if !is_running_generation(&read_state, &read_id, read_generation) {
                                    break;
                                }
                                publish_terminal_event(
                                    &read_stream,
                                    &read_events,
                                    read_generation,
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
                            if let Some(event) = record_terminal_error(
                                &read_state,
                                &read_id,
                                read_generation,
                                format!("PTY read failed: {error}"),
                            ) {
                                publish_terminal_event(
                                    &read_stream,
                                    &read_events,
                                    read_generation,
                                    event,
                                );
                            }
                            break;
                        }
                    }
                }
                let initial_tail = initial_conpty_handshake.finish();
                if !initial_tail.is_empty()
                    && is_running_generation(&read_state, &read_id, read_generation)
                {
                    for data in decoder.push(&initial_tail) {
                        if !is_running_generation(&read_state, &read_id, read_generation) {
                            break;
                        }
                        publish_terminal_event(
                            &read_stream,
                            &read_events,
                            read_generation,
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
                if let Some(data) = decoder.finish()
                    && is_running_generation(&read_state, &read_id, read_generation)
                {
                    publish_terminal_event(
                        &read_stream,
                        &read_events,
                        read_generation,
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

        let wait_id = options.session_id.clone();
        let wait_generation = options.generation;
        let wait_state = Arc::clone(&self.state);
        let wait_stream = Arc::clone(&stream);
        let wait_events = events.clone();
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
                        if let Some(event) =
                            finalize_exit(&wait_state, &wait_id, wait_generation, status)
                        {
                            publish_terminal_event(
                                &wait_stream,
                                &wait_events,
                                wait_generation,
                                event,
                            );
                        }
                    }
                    Err(error) => {
                        *wait_child.lock() = Some(child);
                        if let Some(event) =
                            record_wait_error(&wait_state, &wait_id, wait_generation, error)
                        {
                            publish_terminal_event(
                                &wait_stream,
                                &wait_events,
                                wait_generation,
                                event,
                            );
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

        publish_terminal_event(
            &stream,
            &events,
            options.generation,
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

fn publish_terminal_event(
    stream: &Arc<Mutex<TerminalEventStream>>,
    events: &EventSink,
    generation: u64,
    event: TerminalEvent,
) {
    stream.lock().publish(generation, event.clone());
    let _ = events.emit(TERMINAL_EVENT, event);
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

fn next_terminal_snapshot_revision(previous: &TerminalSnapshot) -> u64 {
    previous.revision.saturating_add(1).max(1)
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
    let kill_error = {
        let mut killer = cleanup.killer.lock();
        if cleanup.terminated.load(Ordering::Acquire) {
            return Ok(());
        }
        killer.kill().err()
    };

    // Windows portable-pty 0.9.0 can report a stale OS error from its
    // split-out ChildKiller after successfully calling TerminateProcess. Do
    // not decide whether a close succeeded from that call alone. We instead
    // require an observed child exit (either directly here or by the waiter
    // thread which currently owns the Child handle). This also leaves a
    // running Child in its slot whenever termination or reaping really fails.
    match wait_for_cleanup_exit(cleanup, Instant::now() + CHILD_TERMINATION_TIMEOUT) {
        Ok(()) => Ok(()),
        Err(wait_error) => {
            if let Some(kill_error) = kill_error {
                Err(AppError::Process(format!(
                    "failed to close terminal: {kill_error}; {wait_error}"
                )))
            } else {
                Err(wait_error)
            }
        }
    }
}

fn wait_for_cleanup_exit(cleanup: &SpawnCleanup, deadline: Instant) -> AppResult<()> {
    loop {
        if cleanup.terminated.load(Ordering::Acquire) {
            return Ok(());
        }

        enum ChildPoll {
            Exited,
            Pending,
            ReapError(std::io::Error),
        }

        let poll = {
            let mut child_slot = cleanup.child.lock();
            match child_slot.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(_)) => {
                        // Drop our handle only after we have positively
                        // observed that this app-owned child exited.
                        child_slot.take();
                        ChildPoll::Exited
                    }
                    Ok(None) => ChildPoll::Pending,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        ChildPoll::Pending
                    }
                    Err(error) => ChildPoll::ReapError(error),
                },
                // The waiter thread may be blocked in Child::wait. Its
                // `terminated` flag above is the corresponding positive exit
                // observation, so wait briefly without fabricating success.
                None => ChildPoll::Pending,
            }
        };

        match poll {
            ChildPoll::Exited => {
                cleanup.terminated.store(true, Ordering::Release);
                return Ok(());
            }
            ChildPoll::ReapError(error) => {
                return Err(AppError::Process(format!(
                    "failed to reap the owned terminal child: {error}"
                )));
            }
            ChildPoll::Pending => {
                if cleanup.terminated.load(Ordering::Acquire) {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(AppError::Process(
                        "timed out waiting for the owned terminal child to exit".to_owned(),
                    ));
                }
                thread::sleep(CHILD_TERMINATION_POLL_INTERVAL);
            }
        }
    }
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
    let terminal_state = if exit_code == Some(0) {
        TerminalState::Closed
    } else {
        TerminalState::Offline
    };
    let error = status.signal().map(ToOwned::to_owned);
    let changed = record.snapshot.state != terminal_state
        || record.snapshot.exit_code != exit_code
        || record.snapshot.error != error;
    record.snapshot.state = terminal_state;
    record.snapshot.exit_code = exit_code;
    record.snapshot.error = error;
    if changed {
        revise_terminal_snapshot(&mut record.snapshot);
    }
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
    record_terminal_error(
        state,
        session_id,
        generation,
        format!("failed to wait for PTY child: {error}"),
    )
}

/// Records an error only if the exact PTY generation remains current while
/// the registry state is locked. The returned lifecycle event always carries
/// the revised snapshot, so consumers can reject an old generation after a
/// reconnect rather than applying a bare error by session id.
fn record_terminal_error(
    state: &Arc<RwLock<TerminalRegistryState>>,
    session_id: &str,
    generation: u64,
    message: String,
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
    if record.snapshot.state == TerminalState::Failed
        && record.snapshot.error.as_deref() == Some(message.as_str())
    {
        return None;
    }
    record.snapshot.state = TerminalState::Failed;
    record.snapshot.error = Some(message.clone());
    revise_terminal_snapshot(&mut record.snapshot);
    Some(TerminalEvent {
        session_id: session_id.to_owned(),
        kind: TerminalEventKind::Error,
        data: None,
        exit_code: None,
        message: Some(message),
        snapshot: Some(record.snapshot.clone()),
    })
}

fn revise_terminal_snapshot(snapshot: &mut TerminalSnapshot) {
    snapshot.revision = snapshot.revision.saturating_add(1);
}

/// Handles the one cursor-position query emitted by the Windows ConPTY
/// startup path. `portable-pty` enables `PSUEDOCONSOLE_INHERIT_CURSOR`, which
/// can write `ESC[6n` before the SSH process produces normal terminal output.
///
/// The matcher is enabled only on Windows, consumes only an exact query at the
/// beginning of a newly created PTY, and becomes inert after either a match or
/// a mismatch. It therefore never answers a later remote DSR query.
struct InitialConPtyCursorHandshake {
    awaiting_query: bool,
    matched: usize,
}

struct InitialConPtyRead<'a> {
    answer_cursor_query: bool,
    output: Cow<'a, [u8]>,
}

impl Default for InitialConPtyCursorHandshake {
    fn default() -> Self {
        Self::new(cfg!(windows))
    }
}

impl InitialConPtyCursorHandshake {
    fn new(enabled: bool) -> Self {
        Self {
            awaiting_query: enabled,
            matched: 0,
        }
    }

    /// Consumes an initial query prefix across arbitrary PTY read boundaries.
    /// A mismatch returns every buffered byte unchanged, while a full match
    /// removes the query and asks the caller to write the cursor response.
    fn consume<'a>(&mut self, bytes: &'a [u8]) -> InitialConPtyRead<'a> {
        if !self.awaiting_query {
            return InitialConPtyRead {
                answer_cursor_query: false,
                output: Cow::Borrowed(bytes),
            };
        }

        let mut consumed = 0;
        while consumed < bytes.len() && self.matched < CONPTY_INITIAL_CURSOR_QUERY.len() {
            if bytes[consumed] != CONPTY_INITIAL_CURSOR_QUERY[self.matched] {
                let mut output = Vec::with_capacity(self.matched + bytes.len() - consumed);
                output.extend_from_slice(&CONPTY_INITIAL_CURSOR_QUERY[..self.matched]);
                output.extend_from_slice(&bytes[consumed..]);
                self.awaiting_query = false;
                self.matched = 0;
                return InitialConPtyRead {
                    answer_cursor_query: false,
                    output: Cow::Owned(output),
                };
            }
            self.matched += 1;
            consumed += 1;
        }

        if self.matched < CONPTY_INITIAL_CURSOR_QUERY.len() {
            return InitialConPtyRead {
                answer_cursor_query: false,
                output: Cow::Borrowed(&[]),
            };
        }

        self.awaiting_query = false;
        self.matched = 0;
        InitialConPtyRead {
            answer_cursor_query: true,
            output: Cow::Borrowed(&bytes[consumed..]),
        }
    }

    /// Flushes a partial first prefix if the PTY closes before it can form a
    /// complete ConPTY query; output bytes are never silently discarded.
    fn finish(&mut self) -> Vec<u8> {
        if !self.awaiting_query {
            return Vec::new();
        }
        self.awaiting_query = false;
        let prefix = CONPTY_INITIAL_CURSOR_QUERY[..self.matched].to_vec();
        self.matched = 0;
        prefix
    }
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
        (!self.pending.is_empty())
            .then(|| String::from_utf8_lossy(&std::mem::take(&mut self.pending)).into_owned())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io, sync::Barrier};

    #[derive(Default)]
    struct RecordingWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for RecordingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
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

    #[derive(Debug)]
    struct ErrorKiller;

    impl ChildKiller for ErrorKiller {
        fn kill(&mut self) -> io::Result<()> {
            Err(io::Error::other("synthetic kill result"))
        }

        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(Self)
        }
    }

    #[derive(Debug)]
    struct PendingThenExitedChild {
        pending_polls: usize,
        polls: Arc<Mutex<usize>>,
    }

    impl ChildKiller for PendingThenExitedChild {
        fn kill(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(NoopKiller)
        }
    }

    impl Child for PendingThenExitedChild {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            *self.polls.lock() += 1;
            if self.pending_polls > 0 {
                self.pending_polls -= 1;
                Ok(None)
            } else {
                Ok(Some(ExitStatus::with_exit_code(1)))
            }
        }

        fn wait(&mut self) -> io::Result<ExitStatus> {
            Ok(ExitStatus::with_exit_code(1))
        }

        fn process_id(&self) -> Option<u32> {
            None
        }

        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
        }
    }

    #[derive(Debug)]
    struct ReapFailingChild;

    impl ChildKiller for ReapFailingChild {
        fn kill(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(NoopKiller)
        }
    }

    impl Child for ReapFailingChild {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            Err(io::Error::other("synthetic reap failure"))
        }

        fn wait(&mut self) -> io::Result<ExitStatus> {
            Err(io::Error::other("synthetic reap failure"))
        }

        fn process_id(&self) -> Option<u32> {
            None
        }

        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
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
        let snapshot = TerminalSnapshot {
            session_id: "session".to_owned(),
            generation: 7,
            revision: 1,
            host_id: "host".to_owned(),
            alias: "host".to_owned(),
            cwd: "~".to_owned(),
            title: "host · ~".to_owned(),
            state: TerminalState::Running,
            exit_code: None,
            error: None,
        };
        let mut state = registry.state.write();
        state.active.insert(
            "session".to_owned(),
            TerminalSlot::Running(TerminalSession {
                generation: 7,
                host_id: "host".to_owned(),
                input: Arc::new(Mutex::new(TerminalInputState {
                    writer,
                    active_lease: None,
                    issued_leases: BTreeSet::new(),
                    next_lease: 0,
                })),
                master: Arc::new(Mutex::new(pair.master)),
                cleanup: SpawnCleanup {
                    killer: Arc::new(Mutex::new(Box::new(NoopKiller))),
                    child: Arc::new(Mutex::new(None)),
                    terminated: Arc::new(AtomicBool::new(true)),
                },
                stream: Arc::new(Mutex::new(TerminalEventStream::new())),
            }),
        );
        state.snapshots.insert(
            "session".to_owned(),
            TerminalRecord {
                generation: 7,
                snapshot,
            },
        );
        drop(state);
        (registry, pair.slave)
    }

    fn event(session_id: &str, data: &str) -> TerminalEvent {
        TerminalEvent {
            session_id: session_id.to_owned(),
            kind: TerminalEventKind::Output,
            data: Some(data.to_owned()),
            exit_code: None,
            message: None,
            snapshot: None,
        }
    }

    #[test]
    fn cleanup_confirms_delayed_exit_after_a_killer_error() {
        let polls = Arc::new(Mutex::new(0));
        let cleanup = SpawnCleanup {
            killer: Arc::new(Mutex::new(Box::new(ErrorKiller))),
            child: Arc::new(Mutex::new(Some(Box::new(PendingThenExitedChild {
                pending_polls: 2,
                polls: Arc::clone(&polls),
            })))),
            terminated: Arc::new(AtomicBool::new(false)),
        };

        // A split-out Windows ChildKiller can return a stale OS error even
        // though termination was submitted. Pending polls must therefore be
        // retried until the owned child actually exits.
        terminate_cleanup(&cleanup).expect("observed child exit accepts stale killer error");

        assert_eq!(*polls.lock(), 3);
        assert!(cleanup.terminated.load(Ordering::Acquire));
        assert!(cleanup.child.lock().is_none());
    }

    #[test]
    fn cleanup_preserves_real_kill_and_reap_failures() {
        let cleanup = SpawnCleanup {
            killer: Arc::new(Mutex::new(Box::new(ErrorKiller))),
            child: Arc::new(Mutex::new(Some(Box::new(ReapFailingChild)))),
            terminated: Arc::new(AtomicBool::new(false)),
        };

        let error = terminate_cleanup(&cleanup).expect_err("a failed reap must not be hidden");
        let message = error.to_string();
        assert!(message.contains("synthetic kill result"), "{message}");
        assert!(
            message.contains("failed to reap the owned terminal child"),
            "{message}"
        );
        assert!(cleanup.child.lock().is_some());
        assert!(!cleanup.terminated.load(Ordering::Acquire));
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
    fn initial_conpty_cursor_query_is_consumed_once_across_split_reads() {
        let mut handshake = InitialConPtyCursorHandshake::new(true);

        let prefix = handshake.consume(b"\x1b[");
        assert!(!prefix.answer_cursor_query);
        assert_eq!(prefix.output.as_ref(), b"");

        let query = handshake.consume(b"6nSSH banner");
        assert!(query.answer_cursor_query);
        assert_eq!(query.output.as_ref(), b"SSH banner");

        // Once startup negotiation completes, a genuine remote DSR remains
        // terminal output; it is never answered or filtered by this shim.
        let later_remote_query = handshake.consume(b"\x1b[6n");
        assert!(!later_remote_query.answer_cursor_query);
        assert_eq!(later_remote_query.output.as_ref(), b"\x1b[6n");
    }

    #[test]
    fn initial_conpty_cursor_handshake_preserves_nonmatching_and_partial_prefixes() {
        let mut mismatched = InitialConPtyCursorHandshake::new(true);
        assert!(!mismatched.consume(b"\x1b[").answer_cursor_query);
        let output = mismatched.consume(b"5ntext");
        assert!(!output.answer_cursor_query);
        assert_eq!(output.output.as_ref(), b"\x1b[5ntext");

        let mut partial = InitialConPtyCursorHandshake::new(true);
        assert!(!partial.consume(b"\x1b[").answer_cursor_query);
        assert_eq!(partial.finish(), b"\x1b[");
        let after_eof = partial.consume(b"6n");
        assert!(!after_eof.answer_cursor_query);
        assert_eq!(after_eof.output.as_ref(), b"6n");
    }

    #[test]
    fn conpty_cursor_reply_uses_the_owned_writer_without_a_browser_lease() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let mut input = TerminalInputState {
            writer: Box::new(RecordingWriter(Arc::clone(&written))),
            active_lease: None,
            issued_leases: BTreeSet::new(),
            next_lease: 0,
        };

        input
            .write_initial_conpty_cursor_response()
            .expect("initial cursor response");
        assert_eq!(&*written.lock(), CONPTY_INITIAL_CURSOR_RESPONSE);
        assert!(input.active_lease.is_none());
    }

    #[test]
    fn output_replay_resumes_after_the_requested_sequence() {
        let mut stream = TerminalEventStream::new();
        let first = stream.publish(9, event("session", "one"));
        let second = stream.publish(9, event("session", "two"));
        let (replay, _receiver) = stream.attach("session", 9, Some(first.sequence));
        assert_eq!(replay, vec![second]);
    }

    #[test]
    fn replay_announces_the_precise_truncation_event() {
        let mut stream = TerminalEventStream::new();
        for _ in 0..17 {
            stream.publish(4, event("session", &"x".repeat(65_536)));
        }
        let (replay, _receiver) = stream.attach("session", 4, None);
        assert_eq!(
            replay.first().map(|item| item.event.kind),
            Some(TerminalEventKind::ReplayTruncated)
        );
        assert_eq!(
            serde_json::to_value(TerminalEventKind::ReplayTruncated).expect("event kind"),
            serde_json::json!("replayTruncated")
        );
        assert!(replay.iter().skip(1).all(|item| item.event.data.is_some()));
        let retained = replay
            .iter()
            .skip(1)
            .map(|item| replay_event_bytes(&item.event))
            .sum::<usize>();
        assert!(retained <= MAX_TERMINAL_REPLAY_BYTES);
    }

    #[test]
    fn terminal_snapshot_revision_tracks_state_updates_and_generation() {
        let (registry, _slave) = registry_with_running_pty();
        let initial = registry.list().pop().expect("running snapshot");
        assert_eq!(initial.generation, 7);
        assert_eq!(initial.revision, 1);

        let event = record_wait_error(
            &registry.state,
            "session",
            7,
            io::Error::other("wait failed"),
        )
        .expect("state update event");
        let snapshot = event.snapshot.expect("snapshot event");
        assert_eq!(snapshot.generation, 7);
        assert_eq!(snapshot.revision, 2);
        assert_eq!(snapshot.state, TerminalState::Failed);

        assert!(
            record_wait_error(
                &registry.state,
                "session",
                7,
                io::Error::other("wait failed"),
            )
            .is_none()
        );
        assert_eq!(registry.list()[0].revision, 2);
        registry.stop_all();
    }

    #[test]
    fn reader_errors_carry_the_current_snapshot_and_ignore_stale_generations() {
        let (registry, _slave) = registry_with_running_pty();
        assert!(
            record_terminal_error(
                &registry.state,
                "session",
                6,
                "PTY read failed: stale".to_owned(),
            )
            .is_none()
        );

        let event = record_terminal_error(
            &registry.state,
            "session",
            7,
            "PTY read failed: current".to_owned(),
        )
        .expect("current reader error is a lifecycle event");
        assert_eq!(event.kind, TerminalEventKind::Error);
        let snapshot = event.snapshot.expect("reader error snapshot");
        assert_eq!(snapshot.generation, 7);
        assert_eq!(snapshot.revision, 2);
        assert_eq!(snapshot.state, TerminalState::Failed);

        // Model a replacement PTY installed after the old reader was blocked
        // before it could acquire the registry lock. Its error must not alter
        // the replacement's snapshot.
        {
            let mut state = registry.state.write();
            let Some(TerminalSlot::Running(session)) = state.active.get_mut("session") else {
                panic!("test session must be running");
            };
            session.generation = 8;
            let record = state.snapshots.get_mut("session").expect("test snapshot");
            record.generation = 8;
            record.snapshot.generation = 8;
            record.snapshot.revision = 1;
            record.snapshot.state = TerminalState::Running;
            record.snapshot.error = None;
        }
        assert!(
            record_terminal_error(
                &registry.state,
                "session",
                7,
                "PTY read failed: old generation".to_owned(),
            )
            .is_none()
        );
        assert_eq!(registry.list()[0].generation, 8);
        assert_eq!(registry.list()[0].state, TerminalState::Running);
        registry.stop_all();
    }

    #[test]
    fn reconnect_reserves_a_new_generation_and_snapshot_revision() {
        let registry = TerminalRegistry::default();
        registry.next_generation.store(7, Ordering::Relaxed);
        registry.state.write().snapshots.insert(
            "session".to_owned(),
            TerminalRecord {
                generation: 7,
                snapshot: TerminalSnapshot {
                    session_id: "session".to_owned(),
                    generation: 7,
                    revision: 5,
                    host_id: "host".to_owned(),
                    alias: "host".to_owned(),
                    cwd: "~".to_owned(),
                    title: "host · ~".to_owned(),
                    state: TerminalState::Offline,
                    exit_code: Some(255),
                    error: Some("lost connection".to_owned()),
                },
            },
        );

        let (previous, generation) = registry
            .reserve_reconnect("session", "host")
            .expect("reserve reconnect");
        assert_eq!(generation, 8);
        assert_eq!(previous.generation, 7);
        assert_eq!(next_terminal_snapshot_revision(&previous), 6);
        assert!(matches!(
            registry.state.read().active.get("session"),
            Some(TerminalSlot::Starting { generation: 8, .. })
        ));
        registry.stop_all();
    }

    #[test]
    fn new_writer_lease_takes_over_inside_the_write_lock() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let mut input = TerminalInputState {
            writer: Box::new(RecordingWriter(Arc::clone(&written))),
            active_lease: None,
            issued_leases: BTreeSet::new(),
            next_lease: 0,
        };
        let first = input.issue_lease().expect("first lease");
        input.activate(first).expect("first attachment");
        let second = input.issue_lease().expect("second lease");
        input.write(first, b"first").expect("first write");
        input.activate(second).expect("takeover attachment");
        input.write(second, b"second").expect("takeover write");
        assert!(input.assert_active(first).is_err());
        assert!(input.write(first, b"stale").is_err());
        assert_eq!(&*written.lock(), b"firstsecond");
    }

    #[test]
    fn concurrent_leases_have_one_final_writer_and_no_interleaved_bytes() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let input = Arc::new(Mutex::new(TerminalInputState {
            writer: Box::new(RecordingWriter(Arc::clone(&written))),
            active_lease: None,
            issued_leases: BTreeSet::new(),
            next_lease: 0,
        }));
        let lease = {
            let mut guard = input.lock();
            let lease = guard.issue_lease().expect("lease");
            guard.activate(lease).expect("attachment");
            lease
        };
        let barrier = Arc::new(Barrier::new(3));
        let writes = (*b"ab")
            .into_iter()
            .map(|byte| {
                let input = Arc::clone(&input);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    input.lock().write(lease, &[byte; 1024])
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        for write in writes {
            write.join().expect("writer thread").expect("write");
        }
        let bytes = written.lock();
        assert_eq!(bytes.len(), 2048);
        assert!(bytes[..1024].iter().all(|byte| *byte == bytes[0]));
        assert!(bytes[1024..].iter().all(|byte| *byte == bytes[1024]));
        assert_ne!(bytes[0], bytes[1024]);
    }

    #[test]
    fn second_attach_immediately_revokes_the_first_writer() {
        let (registry, _slave) = registry_with_running_pty();
        let first = registry.attach("session", None).expect("first attachment");
        assert!(registry.input_is_current("session", first.generation, first.lease));

        let second = registry.attach("session", None).expect("second attachment");
        assert!(!registry.input_is_current("session", first.generation, first.lease));
        assert!(registry.input_is_current("session", second.generation, second.lease));
        assert!(
            registry
                .write_input("session", first.generation, first.lease, b"stale")
                .is_err()
        );
        registry.stop_all();
    }

    #[test]
    fn automatic_resume_can_renew_its_released_last_lease() {
        let (registry, _slave) = registry_with_running_pty();
        let first = registry
            .attach_if_generation("session", 7, None)
            .expect("first attachment");
        registry
            .release_input("session", first.generation, first.lease)
            .expect("release first attachment");

        let resumed = registry
            .attach_resume_if_generation("session", 7, None, first.lease)
            .expect("resume the last released lease");
        assert!(resumed.lease > first.lease);
        assert!(registry.input_is_current("session", resumed.generation, resumed.lease));
        registry.stop_all();
    }

    #[test]
    fn stale_automatic_resume_cannot_revoke_a_newer_writer() {
        let (registry, _slave) = registry_with_running_pty();
        let first = registry
            .attach_if_generation("session", 7, None)
            .expect("first attachment");
        registry
            .release_input("session", first.generation, first.lease)
            .expect("release disconnected attachment");
        let newer = registry
            .attach_if_generation("session", 7, None)
            .expect("manual takeover attachment");

        let error = registry
            .attach_resume_if_generation("session", 7, None, first.lease)
            .expect_err("a stale automatic resume must not take control back");
        assert!(
            matches!(error, AppError::State(message) if message == "terminal input control moved")
        );
        assert!(registry.input_is_current("session", newer.generation, newer.lease));
        assert!(!registry.input_is_current("session", first.generation, first.lease));
        registry.stop_all();
    }

    #[test]
    fn stale_generation_attach_cannot_revoke_the_replacement_writer() {
        let (registry, _slave) = registry_with_running_pty();
        let replacement_writer = registry
            .attach("session", None)
            .expect("current attachment");
        let replacement_generation = replacement_writer.generation + 1;

        // Model the state after a reconnect has installed the replacement PTY
        // and an active browser writer. A stale WebSocket ticket still carries
        // generation 7 and must fail before touching that writer's lease.
        {
            let mut state = registry.state.write();
            let Some(TerminalSlot::Running(session)) = state.active.get_mut("session") else {
                panic!("test session must be running");
            };
            session.generation = replacement_generation;
        }

        assert!(
            registry
                .attach_if_generation("session", replacement_writer.generation, None)
                .is_err()
        );
        assert!(registry.input_is_current(
            "session",
            replacement_generation,
            replacement_writer.lease
        ));
        registry
            .write_input(
                "session",
                replacement_generation,
                replacement_writer.lease,
                b"still-current",
            )
            .expect("stale attach must not revoke the replacement writer");

        let current = registry
            .attach_if_generation("session", replacement_generation, None)
            .expect("current generation attachment");
        assert!(!registry.input_is_current(
            "session",
            replacement_generation,
            replacement_writer.lease
        ));
        assert!(registry.input_is_current("session", current.generation, current.lease));
        registry.stop_all();
    }

    #[test]
    fn dimensions_are_bounded() {
        assert!(validate_size(24, 80).is_ok());
        assert!(validate_size(1, 80).is_err());
        assert!(validate_size(24, 1001).is_err());
    }
}
