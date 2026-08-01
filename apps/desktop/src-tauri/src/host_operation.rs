use parking_lot::Mutex;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Notify;

/// Coordinates the narrow race between resolving a stored host profile and
/// registering the runtime work that uses it. A deletion retires the host,
/// waits for every operation that already crossed the barrier, and only then
/// scans/cancels the registry. New operations remain rejected unless the
/// coordinated deletion is explicitly rolled back.
#[derive(Clone, Debug, Default)]
pub struct HostOperationBarrier {
    inner: Arc<BarrierInner>,
}

#[derive(Debug, Default)]
struct BarrierInner {
    hosts: Mutex<HashMap<String, HostOperationState>>,
    changed: Notify,
}

#[derive(Debug, Default)]
struct HostOperationState {
    active: usize,
    retired: bool,
}

#[derive(Debug)]
pub struct HostOperationLease {
    inner: Arc<BarrierInner>,
    host_id: String,
}

impl Drop for HostOperationLease {
    fn drop(&mut self) {
        let mut hosts = self.inner.hosts.lock();
        if let Some(state) = hosts.get_mut(&self.host_id) {
            state.active = state.active.saturating_sub(1);
            if state.active == 0 && !state.retired {
                hosts.remove(&self.host_id);
            }
        }
        drop(hosts);
        self.inner.changed.notify_waiters();
    }
}

impl HostOperationBarrier {
    pub fn begin(&self, host_id: &str) -> Option<HostOperationLease> {
        let mut hosts = self.inner.hosts.lock();
        let state = hosts.entry(host_id.to_owned()).or_default();
        if state.retired {
            return None;
        }
        state.active = state.active.saturating_add(1);
        Some(HostOperationLease {
            inner: Arc::clone(&self.inner),
            host_id: host_id.to_owned(),
        })
    }

    pub fn retire(&self, host_id: &str) {
        self.inner
            .hosts
            .lock()
            .entry(host_id.to_owned())
            .or_default()
            .retired = true;
    }

    pub async fn wait_idle(&self, host_id: &str) {
        loop {
            // Register the waiter before inspecting state so the final lease
            // cannot notify in the gap between the check and the await.
            let changed = self.inner.changed.notified();
            if self
                .inner
                .hosts
                .lock()
                .get(host_id)
                .is_none_or(|state| state.active == 0)
            {
                return;
            }
            changed.await;
        }
    }

    pub fn restore(&self, host_id: &str) {
        let mut hosts = self.inner.hosts.lock();
        if let Some(state) = hosts.get_mut(host_id) {
            state.retired = false;
            if state.active == 0 {
                hosts.remove(host_id);
            }
        }
        drop(hosts);
        self.inner.changed.notify_waiters();
    }

    #[cfg(test)]
    pub fn is_retired(&self, host_id: &str) -> bool {
        self.inner
            .hosts
            .lock()
            .get(host_id)
            .is_some_and(|state| state.retired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn retirement_rejects_new_work_and_waits_for_existing_lease() {
        let barrier = HostOperationBarrier::default();
        let lease = barrier.begin("host").expect("initial lease");
        barrier.retire("host");
        assert!(barrier.begin("host").is_none());

        let waiting = {
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait_idle("host").await;
            })
        };
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        drop(lease);
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("idle wait must complete")
            .expect("wait task");
    }

    #[tokio::test]
    async fn restoration_reopens_an_aborted_deletion() {
        let barrier = HostOperationBarrier::default();
        barrier.retire("host");
        assert!(barrier.is_retired("host"));
        barrier.restore("host");
        assert!(barrier.begin("host").is_some());
    }
}
