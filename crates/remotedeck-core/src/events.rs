//! A narrow, renderer-agnostic event boundary for the native business core.
//!
//! The server owns delivery policy (SSE, WebSocket, persistence, filtering).
//! Core registries only publish named, serializable state changes through this
//! sink and never depend on a UI runtime.

use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

type EventCallback = dyn Fn(&str, Value) + Send + Sync + 'static;

#[derive(Clone)]
pub struct EventSink {
    callback: Arc<EventCallback>,
}

impl EventSink {
    pub fn new(callback: impl Fn(&str, Value) + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }

    pub fn emit<T: Serialize>(&self, event: &str, payload: T) -> Result<(), serde_json::Error> {
        let payload = serde_json::to_value(payload)?;
        (self.callback)(event, payload);
        Ok(())
    }
}

impl Default for EventSink {
    fn default() -> Self {
        Self::new(|_, _| {})
    }
}

impl std::fmt::Debug for EventSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EventSink(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    #[test]
    fn default_sink_discards_serializable_events() {
        EventSink::default()
            .emit("ignored", serde_json::json!({ "value": true }))
            .expect("serialize event");
    }

    #[test]
    fn sink_serializes_once_before_delivery() {
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let target = Arc::clone(&delivered);
        let sink = EventSink::new(move |event, payload| {
            target.lock().push((event.to_owned(), payload));
        });
        sink.emit("terminal-event", ("session", 7_u64))
            .expect("serialize event");
        assert_eq!(
            &*delivered.lock(),
            &[(
                "terminal-event".to_owned(),
                serde_json::json!(["session", 7])
            )]
        );
    }
}
