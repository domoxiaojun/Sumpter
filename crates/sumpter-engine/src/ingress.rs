//! Ingress-only resource leases. These are independent of response/stream limits.
//! A lower live limit prevents new reservations; it never invalidates a lease.

use axum::body::Body;
use bytes::Bytes;
use futures_util::StreamExt;
use std::sync::{Arc, Mutex};
use sumpter_core::request_policy::{IngressPolicy, RequestPolicy};
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub(crate) const MIB: usize = 1024 * 1024;
pub(crate) const BUSY: &str = "ingress_capacity_exhausted";

#[derive(Debug)]
pub(crate) struct Quota(Mutex<(usize, usize)>);

impl Quota {
    pub(crate) fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self(Mutex::new((limit, 0))))
    }
    pub(crate) fn set_limit(&self, limit: usize) {
        self.0.lock().unwrap().0 = limit;
    }
    pub(crate) fn acquire(self: &Arc<Self>, amount: usize) -> Result<Lease, &'static str> {
        let mut state = self.0.lock().unwrap();
        let used = state.1.checked_add(amount).ok_or(BUSY)?;
        if used > state.0 {
            return Err(BUSY);
        }
        state.1 = used;
        Ok(Lease {
            quota: self.clone(),
            amount,
        })
    }
}

#[derive(Debug)]
pub(crate) struct Lease {
    quota: Arc<Quota>,
    amount: usize,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.quota.0.lock().unwrap().1 -= self.amount;
    }
}

pub(crate) struct Resources {
    pub(crate) buffers: Arc<Quota>,
    pub(crate) preprocessing: Arc<Quota>,
    pub(crate) decompression: Arc<Quota>,
    pub(crate) websocket: Arc<Quota>,
}
impl Resources {
    pub(crate) fn new(policy: &RequestPolicy) -> Self {
        Self {
            buffers: Quota::new(policy.ingress.max_buffered_mib as usize * MIB),
            preprocessing: Quota::new(policy.ingress.max_preprocessing as usize),
            decompression: Quota::new(policy.ingress.max_decompressions as usize),
            websocket: Quota::new(policy.websocket.max_connections as usize),
        }
    }
    pub(crate) fn update(&self, policy: &RequestPolicy) {
        self.buffers
            .set_limit(policy.ingress.max_buffered_mib as usize * MIB);
        self.preprocessing
            .set_limit(policy.ingress.max_preprocessing as usize);
        self.decompression
            .set_limit(policy.ingress.max_decompressions as usize);
        self.websocket
            .set_limit(policy.websocket.max_connections as usize);
    }
}

/// The original upload policy belongs to this request, even after a config save.
pub(crate) struct RequestResources {
    pub(crate) policy: IngressPolicy,
    pub(crate) buffers: Arc<Quota>,
    pub(crate) decompression: Arc<Quota>,
    preprocessing: Arc<Quota>,
    slot: Mutex<Option<Lease>>,
    started: Mutex<Option<Instant>>,
    pub(crate) decoded: Mutex<Option<Bytes>>,
    pub(crate) cancel: CancellationToken,
}
impl RequestResources {
    pub(crate) fn new(policy: IngressPolicy, resources: &Resources) -> Arc<Self> {
        Arc::new(Self {
            policy,
            buffers: resources.buffers.clone(),
            decompression: resources.decompression.clone(),
            preprocessing: resources.preprocessing.clone(),
            slot: Mutex::new(None),
            started: Mutex::new(None),
            decoded: Mutex::new(None),
            cancel: CancellationToken::new(),
        })
    }
    pub(crate) fn finish_preprocessing(&self) {
        self.slot.lock().unwrap().take();
    }
    pub(crate) fn deadline(&self) -> Instant {
        *self
            .started
            .lock()
            .unwrap()
            .get_or_insert_with(Instant::now)
            + Duration::from_secs(self.policy.body_read_timeout_seconds.into())
    }
    pub(crate) async fn read(&self, body: Body) -> Result<Bytes, &'static str> {
        if let Some(decoded) = self.decoded.lock().unwrap().take() {
            return Ok(decoded);
        }
        {
            let mut slot = self.slot.lock().unwrap();
            if slot.is_none() {
                *slot = Some(self.preprocessing.acquire(1)?);
            }
        }
        let deadline = self.deadline();
        let idle = Duration::from_secs(self.policy.body_idle_timeout_seconds.into());
        let mut idle_deadline = Instant::now() + idle;
        let mut stream = body.into_data_stream();
        let mut output = Buffer::new(
            self.buffers.clone(),
            self.policy.max_body_mib as usize * MIB,
        );
        loop {
            if Instant::now() >= deadline.min(idle_deadline) {
                return Err("request_body_timeout");
            }
            let chunk = tokio::time::timeout_at(deadline.min(idle_deadline), stream.next())
                .await
                .map_err(|_| "request_body_timeout")?;
            let Some(chunk) = chunk else {
                break;
            };
            let chunk = chunk.map_err(|_| "request_body_read_failed")?;
            if chunk.is_empty() {
                continue;
            }
            // Count the received frame while copying it to the retained buffer.
            if output.as_ref().len().saturating_add(chunk.len())
                > self.policy.max_body_mib as usize * MIB
            {
                return Err("request_body_too_large");
            }
            let _frame = self.buffers.acquire(chunk.len())?;
            output.append(&chunk)?;
            idle_deadline = Instant::now() + idle;
        }
        Ok(output.into_bytes())
    }
}

/// Growth reserves the new allocation while the old allocation still exists.
/// Bytes clones/slices share this owner, so queues and retries cannot return its
/// quota early. No giant buffer is allocated when a listener starts.
pub(crate) struct Buffer {
    data: Vec<u8>,
    lease: Option<Lease>,
    quota: Arc<Quota>,
    max: usize,
}
impl Buffer {
    pub(crate) fn new(quota: Arc<Quota>, max: usize) -> Self {
        Self {
            data: Vec::new(),
            lease: None,
            quota,
            max,
        }
    }
    pub(crate) fn append(&mut self, chunk: &[u8]) -> Result<(), &'static str> {
        let size = self
            .data
            .len()
            .checked_add(chunk.len())
            .ok_or("request_body_too_large")?;
        if size > self.max {
            return Err("request_body_too_large");
        }
        if size > self.data.capacity() {
            let capacity = size
                .max(self.data.capacity().saturating_mul(2))
                .min(self.max);
            let lease = self.quota.acquire(capacity)?;
            let mut data = Vec::new();
            data.try_reserve_exact(capacity).map_err(|_| BUSY)?;
            // Allocators may round up: account the capacity exposed by Vec.
            let extra = if data.capacity() > capacity {
                Some(self.quota.acquire(data.capacity() - capacity)?)
            } else {
                None
            };
            data.extend_from_slice(&self.data);
            self.data = data;
            self.lease = Some(lease);
            if let Some(mut extra) = extra {
                self.lease.as_mut().unwrap().amount += extra.amount;
                extra.amount = 0;
            }
        }
        self.data.extend_from_slice(chunk);
        Ok(())
    }
    pub(crate) fn into_bytes(self) -> Bytes {
        Bytes::from_owner(self)
    }
}
impl AsRef<[u8]> for Buffer {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl std::io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.append(bytes).map_err(std::io::Error::other)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct BuildBudget {
    quota: Arc<Quota>,
    leases: Vec<Lease>,
    failed: bool,
}
thread_local! {
    // Synchronous serialization only: never keep this scope across an await.
    static BUILD_BUDGET: std::cell::RefCell<Option<BuildBudget>> = const { std::cell::RefCell::new(None) };
}

/// Reserve serialized buffers as they grow, before they are allocated. Legacy
/// pure builders keep their public API, while this boundary propagates any
/// allocation failure before a request can enter a queue or reach a transport.
pub(crate) fn build_bytes<T>(
    quota: Arc<Quota>,
    build: impl FnOnce() -> T,
    body: impl FnOnce(&mut T) -> Option<&mut Bytes>,
) -> Result<T, &'static str> {
    struct Restore(Option<BuildBudget>);
    impl Drop for Restore {
        fn drop(&mut self) {
            BUILD_BUDGET.with(|cell| *cell.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(BUILD_BUDGET.with(|cell| {
        cell.replace(Some(BuildBudget {
            quota,
            leases: Vec::new(),
            failed: false,
        }))
    }));
    let mut result = build();
    let state = BUILD_BUDGET.with(|cell| cell.borrow_mut().take()).unwrap();
    if state.failed {
        return Err(BUSY);
    }
    if !state.leases.is_empty() {
        struct Owned {
            bytes: Bytes,
            _leases: Vec<Lease>,
        }
        impl AsRef<[u8]> for Owned {
            fn as_ref(&self) -> &[u8] {
                &self.bytes
            }
        }
        if let Some(target) = body(&mut result) {
            *target = Bytes::from_owner(Owned {
                bytes: std::mem::take(target),
                _leases: state.leases,
            });
        }
    }
    Ok(result)
}

pub(crate) fn to_vec<T: serde::Serialize + ?Sized>(
    value: &T,
) -> Result<Vec<u8>, serde_json::Error> {
    let quota = BUILD_BUDGET.with(|cell| cell.borrow().as_ref().map(|state| state.quota.clone()));
    let Some(quota) = quota else {
        return serde_json::to_vec(value);
    };
    let mut buffer = Buffer::new(quota, usize::MAX);
    if let Err(error) = serde_json::to_writer(&mut buffer, value) {
        BUILD_BUDGET.with(|cell| cell.borrow_mut().as_mut().unwrap().failed = true);
        return Err(error);
    }
    if let Some(lease) = buffer.lease.take() {
        BUILD_BUDGET.with(|cell| cell.borrow_mut().as_mut().unwrap().leases.push(lease));
    }
    Ok(buffer.data)
}

pub(crate) fn copy_parts<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> Bytes {
    let quota = BUILD_BUDGET.with(|cell| cell.borrow().as_ref().map(|state| state.quota.clone()));
    if let Some(quota) = quota {
        let mut buffer = Buffer::new(quota, usize::MAX);
        for part in parts {
            if buffer.append(part).is_err() {
                BUILD_BUDGET.with(|cell| cell.borrow_mut().as_mut().unwrap().failed = true);
                return Bytes::new();
            }
        }
        buffer.into_bytes()
    } else {
        let mut buffer = Vec::new();
        for part in parts {
            buffer.extend_from_slice(part);
        }
        Bytes::from(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serialized_copies_keep_their_leases_and_budget_failure_cannot_send_an_empty_body() {
        let quota = Quota::new(256);
        let bytes = build_bytes(
            quota.clone(),
            || {
                Bytes::from(
                    to_vec(&serde_json::json!({"text":"a bounded copy"})).unwrap_or_default(),
                )
            },
            |body| Some(body),
        )
        .unwrap();
        let retry = bytes.clone();
        drop(bytes);
        assert!(quota.acquire(256).is_err());
        drop(retry);
        assert!(quota.acquire(256).is_ok());
        let result = build_bytes(
            Quota::new(8),
            || Bytes::from(to_vec(&serde_json::json!({"text":"too large"})).unwrap_or_default()),
            |body| Some(body),
        );
        assert_eq!(result, Err(BUSY));
    }
    #[test]
    fn clones_slices_and_lowered_limits_retain_actual_owners() {
        let quota = Quota::new(16);
        let mut buffer = Buffer::new(quota.clone(), 16);
        buffer.append(b"12345678").unwrap();
        let bytes = buffer.into_bytes();
        let queued = bytes.slice(2..4);
        drop(bytes);
        quota.set_limit(4);
        assert!(quota.acquire(1).is_err());
        drop(queued);
        assert!(quota.acquire(4).is_ok());
    }
    #[test]
    fn growth_and_failures_release_reservations() {
        let quota = Quota::new(12);
        let mut buffer = Buffer::new(quota.clone(), 16);
        buffer.append(b"12345678").unwrap();
        assert_eq!(buffer.append(b"9"), Err(BUSY));
        drop(buffer);
        assert!(quota.acquire(12).is_ok());
    }
    #[tokio::test(start_paused = true)]
    async fn idle_timeout_and_preprocessing_are_bounded() {
        let policy = RequestPolicy::default();
        let resources = Resources::new(&policy);
        let request = RequestResources::new(policy.ingress.clone(), &resources);
        let body =
            Body::from_stream(futures_util::stream::pending::<Result<Bytes, std::io::Error>>());
        assert_eq!(request.read(body).await, Err("request_body_timeout"));
        drop(request);
        assert!(resources.preprocessing.acquire(4).is_ok());
    }
}
