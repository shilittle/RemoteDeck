//! Authentication for the single-user, IPv4-loopback browser boundary.
use axum::http::{HeaderMap, StatusCode};
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

const TICKET_TTL: Duration = Duration::from_secs(30);
const SESSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_SESSIONS: usize = 64;
const MAX_TICKETS: usize = 128;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ApiError {
    #[serde(skip)]
    pub status: StatusCode,
    pub code: String,
    pub message: String,
}
impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for ApiError {}
impl ApiError {
    pub(crate) fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
        }
    }
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
}
impl From<remotedeck_core::error::AppError> for ApiError {
    fn from(error: remotedeck_core::error::AppError) -> Self {
        use remotedeck_core::error::AppError;
        let (status, code) = match &error {
            AppError::Validation(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::State(_) => (StatusCode::CONFLICT, "state_conflict"),
            AppError::MissingExecutable(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "missing_executable")
            }
            AppError::Timeout(_) => (StatusCode::GATEWAY_TIMEOUT, "timeout"),
            AppError::Cancelled => (StatusCode::CONFLICT, "cancelled"),
            AppError::Process(_) => (StatusCode::BAD_GATEWAY, "process_failed"),
            AppError::ProcessCleanup(_) => (StatusCode::CONFLICT, "cleanup_failed"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "operation_failed"),
        };
        Self::new(status, code, error.to_string())
    }
}

#[derive(Clone)]
pub(crate) struct Session {
    pub id: String,
    pub csrf: String,
    expires: Instant,
}
struct LaunchTicket {
    expires: Instant,
}
pub(crate) struct TerminalTicket {
    pub session_id: String,
    pub browser_id: String,
    pub generation: u64,
    expires: Instant,
}
#[derive(Default)]
struct Secrets {
    launch: HashMap<String, LaunchTicket>,
    sessions: HashMap<String, Session>,
    terminals: HashMap<String, TerminalTicket>,
}
#[derive(Clone)]
pub(crate) struct Auth {
    pub authority: String,
    pub origin: String,
    pub cookie_name: String,
    dev_origin: Option<String>,
    control_token: Arc<str>,
    secrets: Arc<Mutex<Secrets>>,
}

pub(crate) fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn constant_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}
impl Auth {
    pub(crate) fn browser_origin(&self) -> &str {
        self.dev_origin.as_deref().unwrap_or(&self.origin)
    }
    pub(crate) fn new(port: u16, control_token: String, dev_origin: Option<String>) -> Self {
        Self {
            authority: format!("127.0.0.1:{port}"),
            origin: format!("http://127.0.0.1:{port}"),
            cookie_name: format!("remotedeck_{port}"),
            dev_origin,
            control_token: control_token.into(),
            secrets: Default::default(),
        }
    }
    pub(crate) fn check_host(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        if headers.get("host").and_then(|h| h.to_str().ok()) != Some(self.authority.as_str()) {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "invalid_host",
                "Only the bound loopback host is accepted.",
            ));
        }
        Ok(())
    }
    fn check_origin(&self, headers: &HeaderMap, required: bool) -> Result<(), ApiError> {
        match headers.get("origin").and_then(|h| h.to_str().ok()) {
            Some(origin) if origin == self.origin || self.dev_origin.as_deref() == Some(origin) => {
                Ok(())
            }
            None if !required => Ok(()),
            _ => Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "invalid_origin",
                "This request did not originate from RemoteDeck.",
            )),
        }
    }
    pub(crate) fn launch_url(&self) -> Result<String, ApiError> {
        let mut secrets = self.secrets.lock();
        secrets
            .launch
            .retain(|_, item| item.expires > Instant::now());
        if secrets.launch.len() >= MAX_TICKETS {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "ticket_limit",
                "Too many pending browser launches.",
            ));
        }
        let ticket = random_token();
        secrets.launch.insert(
            ticket.clone(),
            LaunchTicket {
                expires: Instant::now() + TICKET_TTL,
            },
        );
        let origin = self.dev_origin.as_deref().unwrap_or(&self.origin);
        Ok(format!("{origin}/#ticket={ticket}"))
    }
    pub(crate) fn exchange(
        &self,
        headers: &HeaderMap,
        ticket: &str,
    ) -> Result<(Session, String), ApiError> {
        self.check_host(headers)?;
        self.check_origin(headers, true)?;
        let mut secrets = self.secrets.lock();
        let valid = secrets
            .launch
            .remove(ticket)
            .is_some_and(|t| t.expires > Instant::now());
        if !valid {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_ticket",
                "The launch link expired or was already used. Open RemoteDeck again.",
            ));
        }
        secrets
            .sessions
            .retain(|_, item| item.expires > Instant::now());
        // Opening a second launch link in the same browser must not rotate the
        // shared cookie and invalidate every existing tab's CSRF/WS ownership.
        if let Some(id) = self.cookie_id(headers)
            && let Some(existing) = secrets.sessions.get_mut(id)
        {
            existing.expires = Instant::now() + SESSION_TTL;
            let session = existing.clone();
            let cookie = self.session_cookie(&session);
            return Ok((session, cookie));
        }
        if secrets.sessions.len() >= MAX_SESSIONS {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "session_limit",
                "Too many browser sessions.",
            ));
        }
        let session = Session {
            id: random_token(),
            csrf: random_token(),
            expires: Instant::now() + SESSION_TTL,
        };
        let cookie = self.session_cookie(&session);
        secrets.sessions.insert(session.id.clone(), session.clone());
        Ok((session, cookie))
    }
    fn session_cookie(&self, session: &Session) -> String {
        format!(
            "{}={}; HttpOnly; SameSite=Strict; Path=/",
            self.cookie_name, session.id
        )
    }
    fn cookie_id<'a>(&self, headers: &'a HeaderMap) -> Option<&'a str> {
        headers
            .get("cookie")
            .and_then(|h| h.to_str().ok())
            .and_then(|cookies| {
                cookies
                    .split(';')
                    .filter_map(|part| part.trim().split_once('='))
                    .find(|(name, _)| *name == self.cookie_name)
                    .map(|(_, value)| value)
            })
    }
    pub(crate) fn require(&self, headers: &HeaderMap, mutation: bool) -> Result<Session, ApiError> {
        self.check_host(headers)?;
        self.check_origin(headers, mutation)?;
        let id = self.cookie_id(headers).ok_or_else(|| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "session_required",
                "Open RemoteDeck from its shortcut to authorize this browser.",
            )
        })?;
        let mut secrets = self.secrets.lock();
        let session = secrets
            .sessions
            .get_mut(id)
            .filter(|s| s.expires > Instant::now())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::UNAUTHORIZED,
                    "session_expired",
                    "The browser session expired. Open RemoteDeck again.",
                )
            })?;
        if mutation
            && !headers
                .get("x-remotedeck-csrf")
                .and_then(|h| h.to_str().ok())
                .is_some_and(|csrf| constant_eq(csrf, &session.csrf))
        {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "invalid_csrf",
                "The request is missing its session-bound CSRF token.",
            ));
        }
        session.expires = Instant::now() + SESSION_TTL;
        Ok(session.clone())
    }
    pub(crate) fn live(&self, session: &str) -> bool {
        self.secrets
            .lock()
            .sessions
            .get(session)
            .is_some_and(|s| s.expires > Instant::now())
    }
    pub(crate) fn issue_terminal(
        &self,
        browser: &Session,
        session_id: String,
        generation: u64,
    ) -> Result<String, ApiError> {
        let mut secrets = self.secrets.lock();
        secrets.terminals.retain(|_, t| t.expires > Instant::now());
        if secrets.terminals.len() >= MAX_TICKETS {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "ticket_limit",
                "Too many pending terminal attachments.",
            ));
        }
        let ticket = random_token();
        secrets.terminals.insert(
            ticket.clone(),
            TerminalTicket {
                session_id,
                browser_id: browser.id.clone(),
                generation,
                expires: Instant::now() + TICKET_TTL,
            },
        );
        Ok(ticket)
    }
    pub(crate) fn claim_terminal(
        &self,
        headers: &HeaderMap,
        ticket: &str,
    ) -> Result<TerminalTicket, ApiError> {
        self.check_origin(headers, true)?;
        let session = self.require(headers, false)?;
        let mut secrets = self.secrets.lock();
        let ticket = secrets
            .terminals
            .remove(ticket)
            .filter(|t| t.expires > Instant::now() && t.browser_id == session.id)
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::UNAUTHORIZED,
                    "invalid_terminal_ticket",
                    "The terminal ticket expired or was already used.",
                )
            })?;
        Ok(ticket)
    }
    pub(crate) fn internal(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        self.check_host(headers)?;
        // The local launcher channel is deliberately unavailable to browser origins.
        if headers.contains_key("origin") || headers.contains_key("cookie") {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "invalid_control_request",
                "Browser requests cannot use the launcher channel.",
            ));
        }
        if !headers
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|v| constant_eq(v, &self.control_token))
        {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_control_token",
                "Invalid launcher credential.",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn headers(auth: &Auth) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("host", auth.authority.parse().unwrap());
        headers.insert("origin", auth.origin.parse().unwrap());
        headers
    }
    #[test]
    fn launch_is_one_use_and_csrf_is_bound_to_cookie() {
        let auth = Auth::new(43123, random_token(), None);
        let mut headers = headers(&auth);
        let url = auth.launch_url().unwrap();
        let ticket = url.split("#ticket=").nth(1).unwrap();
        let (session, cookie) = auth.exchange(&headers, ticket).unwrap();
        assert!(cookie.contains("HttpOnly; SameSite=Strict"));
        assert!(auth.exchange(&headers, ticket).is_err());
        headers.insert("cookie", cookie.split(';').next().unwrap().parse().unwrap());
        assert!(auth.require(&headers, true).is_err());
        headers.insert("x-remotedeck-csrf", session.csrf.parse().unwrap());
        assert!(auth.require(&headers, true).is_ok());
        headers.insert("origin", "http://evil.example".parse().unwrap());
        assert!(auth.require(&headers, true).is_err());
        headers.insert("origin", auth.origin.parse().unwrap());
        headers.insert("host", "evil.example:43123".parse().unwrap());
        assert!(auth.require(&headers, false).is_err());
    }
    #[test]
    fn another_launch_keeps_existing_tabs_authenticated() {
        let auth = Auth::new(43123, random_token(), None);
        let mut headers = headers(&auth);
        let launch = auth.launch_url().unwrap();
        let (first, cookie) = auth
            .exchange(&headers, launch.split("#ticket=").nth(1).unwrap())
            .unwrap();
        headers.insert("cookie", cookie.split(';').next().unwrap().parse().unwrap());
        headers.insert("x-remotedeck-csrf", first.csrf.clone().parse().unwrap());
        let launch = auth.launch_url().unwrap();
        let (second, second_cookie) = auth
            .exchange(&headers, launch.split("#ticket=").nth(1).unwrap())
            .unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(first.csrf, second.csrf);
        assert_eq!(cookie, second_cookie);
        assert!(auth.require(&headers, true).is_ok());
    }

    #[test]
    fn expired_and_wrong_browser_terminal_tickets_fail_closed() {
        let auth = Auth::new(43123, random_token(), None);
        let mut headers = headers(&auth);
        let url = auth.launch_url().unwrap();
        let (browser, cookie) = auth
            .exchange(&headers, url.split("#ticket=").nth(1).unwrap())
            .unwrap();
        headers.insert("cookie", cookie.split(';').next().unwrap().parse().unwrap());
        let ticket = auth.issue_terminal(&browser, "terminal".into(), 2).unwrap();
        auth.secrets
            .lock()
            .terminals
            .get_mut(&ticket)
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
        assert!(auth.claim_terminal(&headers, &ticket).is_err());
        let ticket = auth.issue_terminal(&browser, "terminal".into(), 2).unwrap();
        assert_eq!(
            auth.claim_terminal(&headers, &ticket).unwrap().generation,
            2
        );
        assert!(auth.claim_terminal(&headers, &ticket).is_err());
    }

    #[test]
    fn expired_launch_and_session_cannot_be_refreshed_by_requests() {
        let auth = Auth::new(43123, random_token(), None);
        let mut headers = headers(&auth);
        let launch = auth.launch_url().unwrap();
        let ticket = launch.split("#ticket=").nth(1).unwrap();
        auth.secrets.lock().launch.get_mut(ticket).unwrap().expires =
            Instant::now() - Duration::from_secs(1);
        assert_eq!(
            auth.exchange(&headers, ticket).err().unwrap().code,
            "invalid_ticket"
        );
        let launch = auth.launch_url().unwrap();
        let (session, cookie) = auth
            .exchange(&headers, launch.split("#ticket=").nth(1).unwrap())
            .unwrap();
        headers.insert("cookie", cookie.split(';').next().unwrap().parse().unwrap());
        headers.insert("x-remotedeck-csrf", session.csrf.parse().unwrap());
        auth.secrets
            .lock()
            .sessions
            .get_mut(&session.id)
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
        assert!(!auth.live(&session.id));
        assert_eq!(
            auth.require(&headers, true).err().unwrap().code,
            "session_expired"
        );
        assert!(!auth.live(&session.id));
    }

    #[test]
    fn websocket_ticket_requires_the_original_browser_and_origin() {
        let auth = Auth::new(43123, random_token(), None);
        let mut first_headers = headers(&auth);
        let first_launch = auth.launch_url().unwrap();
        let (first, cookie) = auth
            .exchange(
                &first_headers,
                first_launch.split("#ticket=").nth(1).unwrap(),
            )
            .unwrap();
        first_headers.insert("cookie", cookie.split(';').next().unwrap().parse().unwrap());
        let mut second_headers = headers(&auth);
        let second_launch = auth.launch_url().unwrap();
        let (_, cookie) = auth
            .exchange(
                &second_headers,
                second_launch.split("#ticket=").nth(1).unwrap(),
            )
            .unwrap();
        second_headers.insert("cookie", cookie.split(';').next().unwrap().parse().unwrap());
        let ticket = auth.issue_terminal(&first, "pty".into(), 1).unwrap();
        assert_eq!(
            auth.claim_terminal(&second_headers, &ticket)
                .err()
                .unwrap()
                .code,
            "invalid_terminal_ticket"
        );
        let ticket = auth.issue_terminal(&first, "pty".into(), 1).unwrap();
        first_headers.insert("origin", "http://evil.example".parse().unwrap());
        assert_eq!(
            auth.claim_terminal(&first_headers, &ticket)
                .err()
                .unwrap()
                .code,
            "invalid_origin"
        );
        first_headers.remove("origin");
        assert_eq!(
            auth.claim_terminal(&first_headers, &ticket)
                .err()
                .unwrap()
                .code,
            "invalid_origin"
        );
    }
}
