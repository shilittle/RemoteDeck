//! Explicit HTTP operations, authenticated SSE, and PTY WebSocket transport.
use crate::{
    api::{self, AppContext},
    auth::{ApiError, Auth},
    native,
    operations::{Admission, Claim, LONG_OPERATIONS, Operations, Outcome, Requests},
};
use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Path, Query, State, WebSocketUpgrade,
        rejection::JsonRejection,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use remotedeck_core::session::TerminalAttachment;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{broadcast, watch};

#[derive(Clone, Serialize)]
pub(crate) struct Envelope {
    pub sequence: u64,
    pub event: String,
    pub payload: Value,
}
#[derive(Default)]
struct EventLog {
    sequence: u64,
    entries: VecDeque<(Arc<Envelope>, usize)>,
    bytes: usize,
}
#[derive(Clone)]
pub(crate) struct EventHub {
    log: Arc<Mutex<EventLog>>,
    sender: broadcast::Sender<Arc<Envelope>>,
}
impl Default for EventHub {
    fn default() -> Self {
        Self {
            log: Default::default(),
            sender: broadcast::channel(256).0,
        }
    }
}
impl EventHub {
    pub(crate) fn publish(&self, event: &str, payload: Value) {
        // Terminal bytes stay in the per-session transport; never global SSE or diagnostics.
        if event == "terminal-event"
            && payload
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|k| k == "output" || k == "replayTruncated")
        {
            return;
        }
        let mut log = self.log.lock();
        log.sequence += 1;
        let item = Arc::new(Envelope {
            sequence: log.sequence,
            event: event.into(),
            payload,
        });
        let bytes = serde_json::to_vec(&*item).map_or(0, |value| value.len());
        log.bytes += bytes;
        log.entries.push_back((item.clone(), bytes));
        while log.entries.len() > 512 || log.bytes > 8 * 1024 * 1024 {
            if let Some((_, bytes)) = log.entries.pop_front() {
                log.bytes = log.bytes.saturating_sub(bytes);
            }
        }
        let _ = self.sender.send(item);
    }
    fn cursor(&self) -> u64 {
        self.log.lock().sequence
    }
    fn initial(&self, since: Option<u64>) -> VecDeque<Arc<Envelope>> {
        let log = self.log.lock();
        if let Some(since) = since
            && since <= log.sequence
            && log
                .entries
                .front()
                .is_none_or(|(e, _)| since.saturating_add(1) >= e.sequence)
        {
            return log
                .entries
                .iter()
                .filter(|(e, _)| e.sequence > since)
                .map(|(e, _)| e.clone())
                .collect();
        }
        VecDeque::from([Arc::new(Envelope {
            sequence: log.sequence,
            event: "resync".into(),
            payload: Value::Null,
        })])
    }
}

#[derive(Clone)]
pub(crate) struct Server {
    pub context: AppContext,
    pub auth: Auth,
    pub hub: EventHub,
    pub operations: Operations,
    pub requests: Requests,
    pub admission: Admission,
    pub stopping: Arc<AtomicBool>,
    pub stop: watch::Sender<bool>,
}
impl Server {
    pub(crate) fn new(context: AppContext, auth: Auth, hub: EventHub) -> Self {
        let operations = Operations::new(hub.clone());
        Self {
            context,
            auth,
            hub,
            operations,
            requests: Default::default(),
            admission: Default::default(),
            stopping: Default::default(),
            stop: watch::channel(false).0,
        }
    }
    pub(crate) fn request_stop(&self) {
        self.admission.close();
        self.stopping.store(true, Ordering::SeqCst);
        self.stop.send_replace(true);
    }
    pub(crate) fn open_browser(&self) -> Result<(), ApiError> {
        let url = self.auth.launch_url()?;
        native::open_url(&url).map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "browser_failed",
                error.to_string(),
            )
        })
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self)).into_response()
    }
}
impl IntoResponse for Outcome {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exchange {
    ticket: String,
}
async fn exchange(
    State(server): State<Server>,
    headers: HeaderMap,
    payload: Result<Json<Exchange>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(payload) = payload.map_err(|error| ApiError::invalid(error.body_text()))?;
    let (session, cookie) = server.auth.exchange(&headers, &payload.ticket)?;
    let mut response = Json(json!({"csrfToken":session.csrf})).into_response();
    response.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_str(&cookie).map_err(|_| ApiError::invalid("Invalid session cookie"))?,
    );
    Ok(response)
}
async fn auth_session(
    State(server): State<Server>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let session = server.auth.require(&headers, false)?;
    Ok(Json(json!({"csrfToken":session.csrf})))
}
fn required_key(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::invalid("Idempotency-Key is required for this operation."))
}
fn needs_key(command: &str) -> bool {
    LONG_OPERATIONS.contains(&command)
        || !matches!(
            command,
            "bootstrap"
                | "list_terminals"
                | "list_tunnels"
                | "transfer_list"
                | "telemetry_list"
                | "telemetry_history"
                | "list_commands"
                | "analyze_command"
                | "list_command_jobs"
                | "agent_session_plan"
        )
}
fn contains_credentials(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.iter().any(|(key, value)| {
            matches!(
                key.to_ascii_lowercase().replace('_', "").as_str(),
                "password" | "passphrase" | "privatekey" | "privatekeycontent"
            ) || contains_credentials(value)
        }),
        Value::Array(values) => values.iter().any(contains_credentials),
        _ => false,
    }
}
async fn business(
    server: Server,
    headers: HeaderMap,
    command: &'static str,
    payload: Result<Json<Value>, JsonRejection>,
) -> Response {
    let result = async {
        let session = server.auth.require(&headers, true)?;
        if server.stopping.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "service_stopping",
                "RemoteDeck is stopping; new operations are closed.",
            ));
        }
        let Json(args) = payload.map_err(|error| ApiError::invalid(error.body_text()))?;
        if !args.is_object() {
            return Err(ApiError::invalid("Request must be a JSON object."));
        }
        if contains_credentials(&args) {
            return Err(ApiError::invalid(
                "Credentials must be entered only in the interactive terminal.",
            ));
        }
        if !needs_key(command) {
            return Ok(execute(&server, command, args).await);
        }
        let claim = server
            .requests
            .claim(&session.id, required_key(&headers)?, command, &args)?;
        let mut receiver = match claim {
            Claim::Existing(receiver) => receiver,
            Claim::New(sender) => {
                let receiver = sender.subscribe();
                // The task, rather than the HTTP socket, owns accepted side effects.
                server
                    .admission
                    .clone()
                    .spawn(sender, async move { execute(&server, command, args).await })?;
                receiver
            }
        };
        loop {
            if let Some(outcome) = receiver.borrow().clone() {
                return Ok(outcome);
            }
            receiver.changed().await.map_err(|_| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "request_interrupted",
                    "The operation could not report its result; refresh tasks before retrying.",
                )
            })?;
        }
    }
    .await;
    match result {
        Ok(outcome) => outcome.into_response(),
        Err(error) => error.into_response(),
    }
}
async fn execute(server: &Server, command: &'static str, args: Value) -> Outcome {
    if LONG_OPERATIONS.contains(&command) {
        return match server
            .operations
            .start(server.context.clone(), command, args)
        {
            Ok(id) => Outcome {
                status: StatusCode::ACCEPTED,
                body: json!({"operationId":id}),
            },
            Err(error) => error.into(),
        };
    }
    match api::dispatch(server.context.clone(), command, args).await {
        Ok(body) => Outcome {
            status: StatusCode::OK,
            body,
        },
        Err(error) => ApiError::from(error).into(),
    }
}
async fn list_operations(
    State(server): State<Server>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    server.auth.require(&headers, false)?;
    Ok(Json(
        serde_json::to_value(server.operations.list())
            .map_err(|e| ApiError::invalid(e.to_string()))?,
    ))
}
async fn operation(
    State(server): State<Server>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    server.auth.require(&headers, false)?;
    Ok(Json(server.operations.get(&id)?))
}

async fn events(State(server): State<Server>, headers: HeaderMap) -> Result<Response, ApiError> {
    let session = server.auth.require(&headers, false)?;
    let receiver = server.hub.sender.subscribe();
    let since = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let pending = server.hub.initial(since);
    let stream = futures_util::stream::unfold(
        (server, session, receiver, pending, since.unwrap_or(0)),
        |(server, session, mut receiver, mut pending, mut last)| async move {
            loop {
                if server.stopping.load(Ordering::SeqCst) || !server.auth.live(&session.id) {
                    return None;
                }
                let item = if let Some(item) = pending.pop_front() {
                    item
                } else {
                    let received = tokio::select! {
                        received = tokio::time::timeout(Duration::from_secs(15), receiver.recv()) => received,
                        _ = async {
                            let mut stop = server.stop.subscribe();
                            if !*stop.borrow_and_update() { let _ = stop.changed().await; }
                        } => return None,
                    };
                    match received {
                        Ok(Ok(item)) => item,
                        Ok(Err(broadcast::error::RecvError::Closed)) => return None,
                        Ok(Err(broadcast::error::RecvError::Lagged(_))) => Arc::new(Envelope {
                            sequence: server.hub.cursor(),
                            event: "resync".into(),
                            payload: Value::Null,
                        }),
                        Err(_) => {
                            return Some((
                                Ok::<Event, Infallible>(Event::default().comment("keepalive")),
                                (server, session, receiver, pending, last),
                            ));
                        }
                    }
                };
                if item.sequence <= last && item.event != "resync" {
                    continue;
                }
                last = item.sequence;
                let event = Event::default()
                    .id(item.sequence.to_string())
                    .json_data(&*item)
                    .unwrap_or_else(|_| Event::default().data("{\"event\":\"resync\"}"));
                return Some((
                    Ok::<Event, Infallible>(event),
                    (server, session, receiver, pending, last),
                ));
            }
        },
    );
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TerminalRequest {
    session_id: String,
}
async fn open_terminal_input(
    State(server): State<Server>,
    headers: HeaderMap,
    payload: Result<Json<TerminalRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let session = server.auth.require(&headers, true)?;
    if server.stopping.load(Ordering::SeqCst) {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_stopping",
            "RemoteDeck is stopping.",
        ));
    }
    let Json(request) = payload.map_err(|e| ApiError::invalid(e.body_text()))?;
    let generation = server
        .context
        .state
        .terminals
        .generation(&request.session_id)?;
    let ticket = server
        .auth
        .issue_terminal(&session, request.session_id.clone(), generation)?;
    let origin = server.auth.browser_origin().replacen("http:", "ws:", 1);
    Ok(Json(
        json!({"url":format!("{origin}/api/v1/terminals/{}/stream?ticket={ticket}",request.session_id),"generation":generation,"maxFrameBytes":65536,"maxBufferedBytes":262144}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StreamQuery {
    ticket: String,
    since: Option<u64>,
    resume_lease: Option<u64>,
}
async fn terminal_upgrade(
    State(server): State<Server>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<StreamQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let ticket = server.auth.claim_terminal(&headers, &query.ticket)?;
    if server.stopping.load(Ordering::SeqCst)
        || ticket.session_id != id
        || server.context.state.terminals.generation(&id)? != ticket.generation
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "stale_terminal",
            "The terminal was replaced or stopped. Attach again.",
        ));
    }
    Ok(ws.max_frame_size(65536).max_message_size(65536).on_upgrade(move |mut socket| async move {
        if server.stopping.load(Ordering::SeqCst) || !server.auth.live(&ticket.browser_id) { return; }
        // Claim control only after the HTTP upgrade succeeds, and only for the ticket's generation.
        let attachment = match query.resume_lease {
            Some(lease) => server.context.state.terminals.attach_resume_if_generation(&id, ticket.generation, query.since, lease),
            None => server.context.state.terminals.attach_if_generation(&id, ticket.generation, query.since),
        };
        match attachment {
            Ok(attachment) => terminal_socket(server, ticket.browser_id, id, attachment, socket).await,
            Err(remotedeck_core::error::AppError::State(message)) if message == "terminal input control moved" => {
                let _ = socket.send(terminal_close(4001, "Input control moved to another attachment")).await;
            }
            Err(_) => { let _ = socket.send(Message::Text(json!({"type":"error","message":"The terminal changed before attachment. Attach again."}).to_string().into())).await; }
        }
    }).into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResizeFrame {
    r#type: String,
    rows: u16,
    cols: u16,
}
async fn terminal_socket(
    server: Server,
    browser: String,
    id: String,
    mut attachment: TerminalAttachment,
    socket: WebSocket,
) {
    let (mut output, mut input) = socket.split();
    let generation = attachment.generation;
    let lease = attachment.lease;
    let attached = json!({"type":"attached","sessionId":id,"generation":generation,"lease":lease});
    let work = async {
        output
            .send(Message::Text(attached.to_string().into()))
            .await
            .ok()?;
        let mut sequence = 0;
        for event in attachment.replay {
            sequence = event.sequence;
            let data = serde_json::to_string(&event).ok()?;
            tokio::time::timeout(
                Duration::from_secs(5),
                output.send(Message::Text(data.into())),
            )
            .await
            .ok()?
            .ok()?;
        }
        output
            .send(Message::Text(
                json!({"type":"replay_complete"}).to_string().into(),
            ))
            .await
            .ok()?;
        let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    let closing = if server.stopping.load(Ordering::SeqCst) { Some((4000, "Service stopping")) }
                        else if !server.auth.live(&browser) { Some((4003, "Browser session expired")) }
                        else if !server.context.state.terminals.input_is_current(&id, generation, lease) { Some((4001, "Input control moved to another attachment")) }
                        else { None };
                    if let Some((code, reason)) = closing {
                        let _ = tokio::time::timeout(Duration::from_secs(2), output.send(terminal_close(code, reason))).await;
                        break;
                    }
                    tokio::time::timeout(Duration::from_secs(5), output.send(Message::Ping(Vec::new().into()))).await.ok()?.ok()?;
                }
                event = attachment.receiver.recv() => match event {
                    Ok(event) => {
                        if event.sequence <= sequence { continue; }
                        sequence = event.sequence;
                        let data = serde_json::to_string(&event).ok()?;
                        tokio::time::timeout(Duration::from_secs(5), output.send(Message::Text(data.into()))).await.ok()?.ok()?;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let _ = output.send(Message::Text(json!({"type":"resync","message":"Terminal output exceeded the live buffer. Attach again for recent output."}).to_string().into())).await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                message = input.next() => {
                    let Some(Ok(message)) = message else { break; };
                    if !server.auth.live(&browser) { break; }
                    if !server.context.state.terminals.input_is_current(&id, generation, lease) {
                        let _ = tokio::time::timeout(Duration::from_secs(2), output.send(terminal_close(4001, "Input control moved to another attachment"))).await;
                        break;
                    }
                    match message {
                        Message::Binary(bytes) => { if server.context.state.terminals.write_input(&id, generation, lease, &bytes).is_err() { break; } }
                        Message::Text(text) => {
                            let Ok(frame) = serde_json::from_str::<ResizeFrame>(&text) else { break; };
                            if frame.r#type != "resize" || server.context.state.terminals.resize_input(&id, generation, lease, frame.rows, frame.cols).is_err() { break; }
                        }
                        Message::Close(_) => break,
                        Message::Ping(bytes) => { output.send(Message::Pong(bytes)).await.ok()?; }
                        Message::Pong(_) => (),
                    }
                }
            }
        }
        Some(())
    };
    let _ = work.await;
    let _ = server
        .context
        .state
        .terminals
        .release_input(&id, generation, lease);
    let _ = tokio::time::timeout(Duration::from_secs(2), output.close()).await;
}

fn terminal_close(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

async fn shutdown(
    State(server): State<Server>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    server.auth.require(&headers, true)?;
    server.request_stop();
    Ok(Json(json!({"stopping":true})))
}
async fn internal_open(
    State(server): State<Server>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    server.auth.internal(&headers)?;
    server.open_browser()?;
    Ok(Json(json!({"opened":true})))
}
async fn internal_stop(
    State(server): State<Server>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    server.auth.internal(&headers)?;
    server.request_stop();
    Ok(Json(json!({"stopping":true})))
}
async fn internal_launch(
    State(server): State<Server>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    server.auth.internal(&headers)?;
    Ok(Json(json!({"browserUrl":server.auth.launch_url()?})))
}

pub(crate) fn router(server: Server) -> Router {
    let mut router = Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ready"})) }))
        .route("/api/v1/auth/exchange", post(exchange))
        .route("/api/v1/auth/session", get(auth_session))
        .route("/api/v1/events", get(events))
        .route("/api/v1/operations", get(list_operations))
        .route("/api/v1/operations/{id}", get(operation))
        .route("/api/v1/open_terminal_input", post(open_terminal_input))
        .route("/api/v1/terminals/{id}/stream", get(terminal_upgrade))
        .route("/api/v1/shutdown", post(shutdown))
        .route("/internal/open", post(internal_open))
        .route("/internal/launch", post(internal_launch))
        .route("/internal/stop", post(internal_stop));
    for &command in api::COMMANDS {
        router = router.route(
            &format!("/api/v1/{command}"),
            post(
                move |State(server): State<Server>,
                      headers: HeaderMap,
                      payload: Result<Json<Value>, JsonRejection>| {
                    business(server, headers, command, payload)
                },
            ),
        );
    }
    router
        .fallback(crate::assets::serve)
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(axum::middleware::from_fn_with_state(
            server.clone(),
            security_headers,
        ))
        .with_state(server)
}
async fn security_headers(
    State(server): State<Server>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if let Err(error) = server.auth.check_host(request.headers()) {
        return error.into_response();
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    headers.insert("content-security-policy", HeaderValue::from_static("default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; font-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_replay_is_ordered_and_terminal_bytes_are_excluded() {
        let hub = EventHub::default();
        hub.publish(
            "terminal-event",
            json!({"kind":"output","data":"private terminal text"}),
        );
        assert_eq!(hub.cursor(), 0);
        hub.publish("transfer-event", json!({"state":"running"}));
        hub.publish("transfer-event", json!({"state":"completed"}));
        let replay = hub.initial(Some(1));
        assert_eq!(replay.len(), 1);
        assert_eq!(replay.front().unwrap().sequence, 2);
        for _ in 0..600 {
            hub.publish("test", Value::Null);
        }
        assert_eq!(hub.initial(Some(1)).front().unwrap().event, "resync");
    }
}
