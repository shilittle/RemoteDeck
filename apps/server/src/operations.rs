//! In-memory long-running operations and request deduplication.
//! Browser disconnects never cancel an accepted operation.
use crate::{
    api::{self, AppContext},
    auth::ApiError,
    transport::EventHub,
};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, watch};
use uuid::Uuid;

pub(crate) const LONG_OPERATIONS: &[&str] = &[
    "scan_host_keys",
    "accept_host_key",
    "list_host_keys",
    "remove_host_key",
    "test_connection",
    "list_keys",
    "generate_key",
    "deploy_key",
    "delete_host",
    "sftp_list",
    "sftp_create_directory",
    "sftp_rename",
    "sftp_delete",
    "probe_agent",
    "btop_probe",
    "btop_start_watchdog",
    "btop_stop_watchdog",
    "legacy_preview",
    "legacy_apply",
    "export_diagnostics",
    "pick_local_path",
    "pick_save_path",
    "telemetry_stop",
    "signal_process",
];

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OperationSummary {
    pub id: String,
    pub command: String,
    pub host_id: Option<String>,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error: Option<ApiError>,
}
struct Record {
    summary: OperationSummary,
    result: Option<Value>,
    result_bytes: usize,
    task: Option<tokio::task::JoinHandle<()>>,
}
#[derive(Clone)]
pub(crate) struct Operations {
    records: Arc<Mutex<HashMap<String, Record>>>,
    permits: Arc<Semaphore>,
    hub: EventHub,
}
impl Operations {
    pub(crate) fn new(hub: EventHub) -> Self {
        Self {
            records: Default::default(),
            permits: Arc::new(Semaphore::new(8)),
            hub,
        }
    }
    pub(crate) fn start(
        &self,
        context: AppContext,
        command: &'static str,
        args: Value,
    ) -> Result<String, ApiError> {
        let mut records = self.records.lock();
        if records
            .values()
            .filter(|r| r.summary.completed_at.is_none())
            .count()
            >= 32
        {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "operation_limit",
                "Wait for current operations to complete.",
            ));
        }
        while records.len() >= 256
            || records.values().map(|r| r.result_bytes).sum::<usize>() > 32 * 1024 * 1024
        {
            let oldest = records
                .values()
                .filter(|r| r.summary.completed_at.is_some())
                .min_by_key(|r| r.summary.completed_at)
                .map(|r| r.summary.id.clone());
            if let Some(id) = oldest {
                records.remove(&id);
            } else {
                break;
            }
        }
        let id = Uuid::new_v4().to_string();
        let summary = OperationSummary {
            id: id.clone(),
            command: command.into(),
            host_id: args
                .get("hostId")
                .or_else(|| args.get("request").and_then(|r| r.get("hostId")))
                .and_then(Value::as_str)
                .map(str::to_owned),
            state: "queued".into(),
            created_at: Utc::now(),
            completed_at: None,
            error: None,
        };
        records.insert(
            id.clone(),
            Record {
                summary: summary.clone(),
                result: None,
                result_bytes: 0,
                task: None,
            },
        );
        self.hub.publish(
            "operation-event",
            serde_json::to_value(summary).unwrap_or(Value::Null),
        );
        let operations = self.clone();
        let task_id = id.clone();
        let task = tokio::spawn(async move {
            let Ok(_permit) = operations.permits.clone().acquire_owned().await else {
                return;
            };
            operations.update(&task_id, |record| record.summary.state = "running".into());
            use futures_util::FutureExt;
            let result = std::panic::AssertUnwindSafe(api::dispatch(
                context.clone(),
                command,
                args,
            ))
            .catch_unwind()
            .await
            .map_err(|_| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "operation_interrupted",
                    "The operation failed unexpectedly; verify its effects before retrying.",
                )
            })
            .and_then(|result| result.map_err(ApiError::from));
            let result = result.and_then(|value| {
                let size = serde_json::to_vec(&value).map_or(usize::MAX, |v| v.len());
                if size > 2 * 1024 * 1024 { Err(ApiError::new(StatusCode::INSUFFICIENT_STORAGE, "result_limit", "Operation completed, but its response exceeded the 2 MiB in-memory result limit. Narrow the requested listing.")) }
                else { Ok((value, size)) }
            });
            operations.update(&task_id, |record| {
                record.summary.completed_at = Some(Utc::now());
                record.task = None;
                match result {
                    Ok((value, size)) => {
                        record.summary.state = "completed".into();
                        record.result = Some(value);
                        record.result_bytes = size;
                    }
                    Err(error) => {
                        record.summary.state = "failed".into();
                        record.summary.error = Some(error);
                    }
                }
            });
        });
        if let Some(record) = records.get_mut(&id) {
            record.task = Some(task);
        }
        Ok(id)
    }
    fn update(&self, id: &str, change: impl FnOnce(&mut Record)) {
        if let Some(record) = self.records.lock().get_mut(id) {
            change(record);
            self.hub.publish(
                "operation-event",
                serde_json::to_value(&record.summary).unwrap_or(Value::Null),
            );
        }
    }
    pub(crate) fn list(&self) -> Vec<OperationSummary> {
        let mut list: Vec<_> = self
            .records
            .lock()
            .values()
            .map(|r| r.summary.clone())
            .collect();
        list.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        list
    }
    pub(crate) fn get(&self, id: &str) -> Result<Value, ApiError> {
        let records = self.records.lock();
        let record = records.get(id).ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "operation_not_found",
                "This operation is no longer in the bounded in-memory history.",
            )
        })?;
        let mut result =
            serde_json::to_value(&record.summary).map_err(|e| ApiError::invalid(e.to_string()))?;
        result["result"] = record.result.clone().unwrap_or(Value::Null);
        Ok(result)
    }
    pub(crate) async fn abort_pending(&self) -> usize {
        let mut tasks = Vec::new();
        let mut count = 0;
        {
            let mut records = self.records.lock();
            for record in records
                .values_mut()
                .filter(|r| r.summary.completed_at.is_none())
            {
                if let Some(task) = record.task.take() {
                    task.abort();
                    tasks.push(task);
                }
                record.summary.state = "failed".into();
                record.summary.completed_at = Some(Utc::now());
                record.summary.error = Some(ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "service_stopping",
                    "Service shutdown interrupted this operation; verify its result before retrying.",
                ));
                count += 1;
            }
        }
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            futures_util::future::join_all(tasks),
        )
        .await;
        count
    }
}

struct ActiveRequest {
    task: tokio::task::JoinHandle<()>,
    sender: watch::Sender<Option<Outcome>>,
}
#[derive(Default)]
struct AdmissionState {
    closed: bool,
    next: u64,
    tasks: HashMap<u64, ActiveRequest>,
}
#[derive(Clone, Default)]
pub(crate) struct Admission(Arc<Mutex<AdmissionState>>);
impl Admission {
    pub(crate) fn spawn(
        &self,
        sender: watch::Sender<Option<Outcome>>,
        work: impl std::future::Future<Output = Outcome> + Send + 'static,
    ) -> Result<(), ApiError> {
        use futures_util::FutureExt;
        let mut state = self.0.lock();
        if state.closed || state.tasks.len() >= 128 {
            let error = ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "admission_closed",
                "The service is stopping or its operation queue is full.",
            );
            sender.send_replace(Some(error.clone().into()));
            return Err(error);
        }
        state.next += 1;
        let id = state.next;
        let gate = self.clone();
        let completion = sender.clone();
        let task = tokio::spawn(async move {
            let outcome = std::panic::AssertUnwindSafe(work)
                .catch_unwind()
                .await
                .unwrap_or_else(|_| {
                    ApiError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "operation_interrupted",
                        "The operation failed unexpectedly; inspect tasks before retrying.",
                    )
                    .into()
                });
            completion.send_replace(Some(outcome));
            gate.0.lock().tasks.remove(&id);
        });
        state.tasks.insert(id, ActiveRequest { task, sender });
        Ok(())
    }
    pub(crate) fn close(&self) {
        self.0.lock().closed = true;
    }
    pub(crate) async fn cancel_and_wait(&self) -> bool {
        let tasks = {
            let mut state = self.0.lock();
            state.closed = true;
            state.tasks.drain().map(|(_, record)| {
                record.sender.send_replace(Some(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "service_stopping", "Service shutdown interrupted the request; verify its result before retrying.").into()));
                record.task.abort();
                record.task
            }).collect::<Vec<_>>()
        };
        tokio::time::timeout(
            Duration::from_secs(2),
            futures_util::future::join_all(tasks),
        )
        .await
        .is_ok()
    }
}

#[derive(Clone)]
pub(crate) struct Outcome {
    pub status: StatusCode,
    pub body: Value,
}
impl From<ApiError> for Outcome {
    fn from(error: ApiError) -> Self {
        Self {
            status: error.status,
            body: serde_json::to_value(error).unwrap_or(Value::Null),
        }
    }
}
struct RequestRecord {
    digest: Vec<u8>,
    started: Instant,
    result: watch::Sender<Option<Outcome>>,
}
#[derive(Clone, Default)]
pub(crate) struct Requests(Arc<Mutex<HashMap<String, RequestRecord>>>);
pub(crate) enum Claim {
    Existing(watch::Receiver<Option<Outcome>>),
    New(watch::Sender<Option<Outcome>>),
}
impl Requests {
    pub(crate) fn claim(
        &self,
        session: &str,
        key: &str,
        command: &str,
        args: &Value,
    ) -> Result<Claim, ApiError> {
        if Uuid::parse_str(key).is_err() {
            return Err(ApiError::invalid("Idempotency-Key must be a UUID."));
        }
        let key = format!("{session}:{key}");
        let mut digest = Sha256::new();
        digest.update(command.as_bytes());
        digest.update([0]);
        digest.update(serde_json::to_vec(args).map_err(|e| ApiError::invalid(e.to_string()))?);
        let digest = digest.finalize().to_vec();
        let mut requests = self.0.lock();
        requests.retain(|_, r| {
            r.started.elapsed() < Duration::from_secs(30 * 60) || r.result.borrow().is_none()
        });
        if let Some(record) = requests.get(&key) {
            if record.digest != digest {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "idempotency_conflict",
                    "This request identifier was already used with different arguments.",
                ));
            }
            return Ok(Claim::Existing(record.result.subscribe()));
        }
        if requests.len() >= 1024 {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "request_limit",
                "The request history is full; retry after older entries expire.",
            ));
        }
        let (result, _) = watch::channel(None);
        requests.insert(
            key,
            RequestRecord {
                digest,
                started: Instant::now(),
                result: result.clone(),
            },
        );
        Ok(Claim::New(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn closing_admission_cancels_registered_work_before_returning() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Running(Arc<AtomicBool>);
        impl Drop for Running {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let gate = Admission::default();
        let active = Arc::new(AtomicBool::new(false));
        let flag = active.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (sender, _) = watch::channel(None);
        gate.spawn(sender, async move {
            flag.store(true, Ordering::SeqCst);
            let _running = Running(flag);
            let _ = started.send(());
            std::future::pending::<Outcome>().await
        })
        .unwrap();
        ready.await.unwrap();
        gate.close();
        assert!(gate.cancel_and_wait().await);
        assert!(!active.load(Ordering::SeqCst));
        let (sender, _) = watch::channel(None);
        assert!(
            gate.spawn(sender, async {
                panic!("closed admission must not execute")
            })
            .is_err()
        );
    }
    #[tokio::test]
    async fn deduplication_survives_disconnected_waiters_and_rejects_changed_args() {
        let requests = Requests::default();
        let key = Uuid::new_v4().to_string();
        let Claim::New(sender) = requests
            .claim(
                "browser",
                &key,
                "start_terminal",
                &serde_json::json!({"hostId":"a"}),
            )
            .unwrap()
        else {
            panic!()
        };
        sender.send_replace(Some(Outcome {
            status: StatusCode::OK,
            body: serde_json::json!({"sessionId":"one"}),
        }));
        let Claim::Existing(receiver) = requests
            .claim(
                "browser",
                &key,
                "start_terminal",
                &serde_json::json!({"hostId":"a"}),
            )
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(receiver.borrow().as_ref().unwrap().body["sessionId"], "one");
        assert!(
            requests
                .claim(
                    "browser",
                    &key,
                    "start_terminal",
                    &serde_json::json!({"hostId":"b"})
                )
                .is_err()
        );
    }
}
