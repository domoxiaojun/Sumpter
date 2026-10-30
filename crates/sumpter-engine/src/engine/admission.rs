//! Request-scoped upstream slots. Waiting never changes provider health.
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use sumpter_core::request_policy::{RequestLimits, RequestPolicy};
use tokio::sync::Notify;
use tokio::time::Instant;

#[derive(Default)]
struct Lane {
    active: u32,
    sent: VecDeque<Instant>,
}
#[derive(Default)]
struct State {
    active: u32,
    sent: VecDeque<Instant>,
    lanes: HashMap<String, Lane>,
    queue: VecDeque<(u64, String)>,
    next: u64,
}
#[derive(Default)]
pub(super) struct Admission {
    state: Mutex<State>,
    changed: Notify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdmissionError {
    QueueFull,
    QueueTimeout,
}

struct Waiting {
    owner: Arc<Admission>,
    ticket: u64,
}
impl Drop for Waiting {
    fn drop(&mut self) {
        self.owner
            .state
            .lock()
            .unwrap()
            .queue
            .retain(|(id, _)| *id != self.ticket);
        self.owner.changed.notify_waiters();
    }
}

pub(super) struct Permit {
    owner: Arc<Admission>,
    endpoint: String,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap();
        state.active = state.active.saturating_sub(1);
        if let Some(lane) = state.lanes.get_mut(&self.endpoint) {
            lane.active = lane.active.saturating_sub(1);
        }
        drop(state);
        self.owner.changed.notify_waiters();
    }
}

fn prune(times: &mut VecDeque<Instant>, now: Instant) {
    while times
        .front()
        .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(60))
    {
        times.pop_front();
    }
}
fn ready(active: u32, times: &VecDeque<Instant>, limits: &RequestLimits) -> bool {
    (limits.max_concurrent == 0 || active < limits.max_concurrent)
        && (limits.requests_per_minute == 0 || times.len() < limits.requests_per_minute as usize)
}

impl Admission {
    pub(super) fn available(&self, endpoint: &str, policy: &RequestPolicy) -> bool {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        prune(&mut state.sent, now);
        if !ready(state.active, &state.sent, &policy.global) {
            return false;
        }
        if state.queue.iter().any(|(_, id)| {
            policy.global.max_concurrent > 0
                || policy.global.requests_per_minute > 0
                || id == endpoint
        }) {
            return false;
        }
        let fallback = RequestLimits::default();
        let limits = policy
            .endpoints
            .get(endpoint)
            .map(|p| &p.limits)
            .unwrap_or(&fallback);
        state.lanes.get_mut(endpoint).is_none_or(|lane| {
            prune(&mut lane.sent, now);
            ready(lane.active, &lane.sent, limits)
        })
    }

    pub(super) async fn acquire(
        self: &Arc<Self>,
        endpoint: &str,
        policy: &RequestPolicy,
    ) -> Result<Permit, AdmissionError> {
        let fallback = RequestLimits::default();
        let local = policy
            .endpoints
            .get(endpoint)
            .map(|p| &p.limits)
            .unwrap_or(&fallback);
        let deadline = Instant::now()
            + Duration::from_secs(
                policy
                    .global
                    .queue_timeout_seconds
                    .min(local.queue_timeout_seconds)
                    .clamp(1, 300) as u64,
            );
        let mut waiting: Option<Waiting> = None;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let wake = {
                let now = Instant::now();
                let mut state = self.state.lock().unwrap();
                prune(&mut state.sent, now);
                state.lanes.retain(|_, lane| {
                    prune(&mut lane.sent, now);
                    lane.active > 0 || !lane.sent.is_empty()
                });
                if !state.lanes.contains_key(endpoint) && state.lanes.len() >= 4096 {
                    return Err(AdmissionError::QueueFull);
                }
                let first = state.queue.iter().find(|(_, id)| {
                    policy.global.max_concurrent > 0
                        || policy.global.requests_per_minute > 0
                        || id == endpoint
                });
                let turn = first.is_none()
                    || waiting
                        .as_ref()
                        .is_some_and(|w| first.is_some_and(|(id, _)| *id == w.ticket));
                let global_ready = ready(state.active, &state.sent, &policy.global);
                let lane = state.lanes.entry(endpoint.to_owned()).or_default();
                if turn && global_ready && ready(lane.active, &lane.sent, local) {
                    lane.active += 1;
                    if local.requests_per_minute > 0 {
                        lane.sent.push_back(now);
                    }
                    state.active += 1;
                    if policy.global.requests_per_minute > 0 {
                        state.sent.push_back(now);
                    }
                    if let Some(w) = &waiting {
                        state.queue.retain(|(id, _)| *id != w.ticket);
                    }
                    drop(state);
                    drop(waiting);
                    return Ok(Permit {
                        owner: self.clone(),
                        endpoint: endpoint.into(),
                    });
                }
                if waiting.is_none() {
                    let local_queued = state.queue.iter().filter(|(_, id)| id == endpoint).count();
                    if state.queue.len() >= policy.global.max_queued as usize
                        || local_queued >= local.max_queued as usize
                    {
                        return Err(AdmissionError::QueueFull);
                    }
                    state.next = state.next.wrapping_add(1);
                    let ticket = state.next;
                    state.queue.push_back((ticket, endpoint.into()));
                    waiting = Some(Waiting {
                        owner: self.clone(),
                        ticket,
                    });
                }
                let rate_wake = state
                    .sent
                    .front()
                    .into_iter()
                    .chain(state.lanes.get(endpoint).and_then(|l| l.sent.front()))
                    .map(|at| *at + Duration::from_secs(60))
                    .min()
                    .unwrap_or(deadline);
                rate_wake.min(deadline)
            };
            tokio::select! { _ = &mut changed => {}, _ = tokio::time::sleep_until(wake) => {} }
            if Instant::now() >= deadline {
                return Err(AdmissionError::QueueTimeout);
            }
        }
    }
}

pub(super) fn hold_response(response: &mut crate::outbound::UpstreamResponse, permit: Permit) {
    use futures_util::{StreamExt, stream};
    let upstream = std::mem::replace(&mut response.stream, stream::empty().boxed());
    response.stream = stream::unfold((upstream, permit), |(mut upstream, permit)| async move {
        upstream
            .next()
            .await
            .map(|chunk| (chunk, (upstream, permit)))
    })
    .boxed();
}

#[derive(Default)]
pub(super) struct BufferBudget(Mutex<usize>);
pub(super) struct BufferLease {
    owner: Arc<BufferBudget>,
    bytes: usize,
}
impl BufferBudget {
    pub(super) fn lease(self: &Arc<Self>) -> BufferLease {
        BufferLease {
            owner: self.clone(),
            bytes: 0,
        }
    }
}
impl BufferLease {
    pub(super) fn reserve(&mut self, bytes: usize, limit: usize) -> bool {
        let mut used = self.owner.0.lock().unwrap();
        if used.saturating_add(bytes) > limit {
            return false;
        }
        *used += bytes;
        self.bytes += bytes;
        true
    }
}
impl Drop for BufferLease {
    fn drop(&mut self) {
        let mut used = self.owner.0.lock().unwrap();
        *used = used.saturating_sub(self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn slots_last_until_response_drop_and_cancelled_waiters_leave() {
        let admission = Arc::new(Admission::default());
        let mut policy = RequestPolicy::default();
        policy.global.max_concurrent = 1;
        policy.global.max_queued = 1;
        let held = admission.acquire("a", &policy).await.unwrap();
        let task = {
            let a = admission.clone();
            let p = policy.clone();
            tokio::spawn(async move { a.acquire("b", &p).await })
        };
        tokio::task::yield_now().await;
        assert!(matches!(
            admission.acquire("c", &policy).await,
            Err(AdmissionError::QueueFull)
        ));
        task.abort();
        let _ = task.await;
        let mut response = crate::outbound::UpstreamResponse {
            status: 200,
            headers: vec![],
            stream: futures_util::StreamExt::boxed(futures_util::stream::pending()),
        };
        hold_response(&mut response, held);
        assert_eq!(admission.state.lock().unwrap().active, 1);
        drop(response);
        let _next = admission.acquire("b", &policy).await.unwrap();
        assert!(admission.state.lock().unwrap().queue.is_empty());
    }
    #[test]
    fn buffer_leases_share_one_budget_and_release_on_drop() {
        let budget = Arc::new(BufferBudget::default());
        let mut first = budget.lease();
        let mut second = budget.lease();
        assert!(first.reserve(6, 10));
        assert!(!second.reserve(5, 10));
        drop(first);
        assert!(second.reserve(10, 10));
        drop(second);
        assert_eq!(*budget.0.lock().unwrap(), 0);
    }
}
