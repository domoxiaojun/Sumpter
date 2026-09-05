//! Shared state ownership, construction and recovery of persisted engine data.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use sumpter_core::config::AppConfig;
use sumpter_core::config_store::ConfigDir;
use sumpter_core::events::RuntimeSnapshot;

use crate::outbound::UpstreamTransport;
use crate::runtime_store::RuntimeStore;

use super::Engine;
use super::capture::{
    CAPTURE_STOP_CAPACITY, DiagnosticCaptureIndexCache, DiagnosticCaptureState,
    capture_index_cache_from_capture, diagnostic_capture_size,
};
use super::dispatch::ProviderModelHealth;
use super::events::EngineNotice;
use super::sessions::{
    LiveSessionEntry, RealtimeClientSecretEntry, SessionStickyEntry, prune_session_sticky,
};
use crate::boundary::{EngineServices, PlatformBoundary};

pub(super) struct EngineState {
    pub(super) runtime: RuntimeSnapshot,
    pub(super) last_error: Option<String>,
    pub(super) stats_durability_warning: Option<String>,
    /// `(endpoint_id, routed_model)` health isolates a rejected model from
    /// other models served by the same provider.
    pub(super) provider_model_health: HashMap<(String, String), ProviderModelHealth>,
    /// affinityID → 调度组；稳定会话不超时并持久化，内容指纹仅进程内兼容。
    pub(super) session_sticky: HashMap<String, SessionStickyEntry>,
    pub(super) last_session_prune_at: f64,
}

pub struct EngineInner {
    pub(super) config: RwLock<Arc<AppConfig>>,
    pub(super) generation: RwLock<String>,
    pub(super) transport: Arc<dyn UpstreamTransport>,
    pub(super) platform: Arc<dyn PlatformBoundary>,
    pub(super) state: Mutex<EngineState>,
    pub(super) dir: Option<ConfigDir>,
    pub(super) stats_writable: AtomicBool,
    pub(super) session_affinity_flush: Mutex<()>,
    pub(super) session_affinity_dirty: AtomicBool,
    pub(super) session_affinity_writable: AtomicBool,
    pub(super) resource_bindings_flush: Mutex<()>,
    pub(super) resource_bindings_dirty: AtomicBool,
    pub(super) resource_bindings_writable: AtomicBool,
    pub(super) capture_flush: Mutex<()>,
    pub(super) capture_dirty: AtomicBool,
    pub(super) capture_writable: AtomicBool,
    pub(super) capture: Mutex<DiagnosticCaptureState>,
    pub(super) capture_index: Mutex<DiagnosticCaptureIndexCache>,
    pub(super) notices: tokio::sync::broadcast::Sender<EngineNotice>,
    pub(super) started_at: Instant,
    pub(super) runtime_store: Option<RuntimeStore>,
    pub(super) runtime_write: Mutex<()>,
    pub(super) realtime_client_secrets: Mutex<HashMap<String, RealtimeClientSecretEntry>>,
    pub(super) live_sessions: Mutex<HashMap<String, LiveSessionEntry>>,
    pub(super) video_sessions: Mutex<HashMap<String, LiveSessionEntry>>,
}

pub(super) fn now_unix() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn config_generation(config: &AppConfig) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(config.to_json_pretty().unwrap_or_default().as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
impl Engine {
    pub fn new(
        config: AppConfig,
        dir: Option<ConfigDir>,
        transport: Arc<dyn UpstreamTransport>,
    ) -> Self {
        Self::new_with_services(
            config,
            dir,
            EngineServices {
                transport,
                platform: Arc::new(crate::boundary::NoopPlatform),
            },
        )
    }

    /// Construct the non-generic shared engine with explicit boundary
    /// services.  The legacy `new` constructor above remains available for
    /// callers that only provide a transport; platform facades use this
    /// method to inject their control policy.
    pub fn new_with_services(
        config: AppConfig,
        dir: Option<ConfigDir>,
        services: EngineServices,
    ) -> Self {
        let generation = config_generation(&config);
        let (runtime_store, runtime, stats_writable, stats_error) = match dir.as_ref() {
            Some(dir) => match RuntimeStore::new(dir.root.join("runtime.sqlite3")) {
                Ok((store, runtime)) => (Some(store), runtime, true, None),
                Err(error) => (None, RuntimeSnapshot::default(), false, Some(error)),
            },
            None => (None, RuntimeSnapshot::default(), true, None),
        };
        let now = now_unix();
        let (session_sticky, session_affinity_writable, session_affinity_pruned) =
            match dir.as_ref().map(ConfigDir::load_session_affinity) {
                Some(Ok(assignments)) => {
                    let mut loaded: HashMap<String, SessionStickyEntry> = assignments
                        .into_iter()
                        .map(|(key, assignment)| {
                            (
                                key,
                                SessionStickyEntry {
                                    label: assignment.scheduling_group,
                                    at: assignment.updated_at,
                                    persistent: true,
                                },
                            )
                        })
                        .collect();
                    let pruned = prune_session_sticky(&mut loaded, now);
                    (loaded, true, pruned)
                }
                Some(Err(error)) => {
                    tracing::warn!("session_affinity.json 加载失败，本次运行拒绝覆盖: {error}");
                    (HashMap::new(), false, false)
                }
                None => (HashMap::new(), true, false),
            };
        let (live_sessions, video_sessions, resource_bindings_writable, resource_bindings_pruned) =
            match dir.as_ref().map(ConfigDir::load_resource_bindings) {
                Some(Ok(bindings)) => {
                    let mut live = HashMap::new();
                    let mut video = HashMap::new();
                    let mut pruned = false;
                    for (key, binding) in bindings {
                        if binding.expires_at <= now {
                            pruned = true;
                            continue;
                        }
                        let target = if let Some(id) = key.strip_prefix("live:") {
                            Some((&mut live, id))
                        } else if let Some(id) = key.strip_prefix("video:") {
                            Some((&mut video, id))
                        } else {
                            pruned = true;
                            None
                        };
                        if let Some((store, id)) = target {
                            store.insert(
                                id.to_string(),
                                LiveSessionEntry {
                                    expires_at: binding.expires_at,
                                    endpoint_id: binding.endpoint_id,
                                    model: binding.model,
                                },
                            );
                        }
                    }
                    (live, video, true, pruned)
                }
                Some(Err(error)) => {
                    tracing::warn!("resource_bindings.json 加载失败，本次运行拒绝覆盖: {error}");
                    (HashMap::new(), HashMap::new(), false, false)
                }
                None => (HashMap::new(), HashMap::new(), true, false),
            };
        let (capture, capture_writable) = match dir.as_ref().map(ConfigDir::load_diagnostic_capture)
        {
            Some(Ok(snapshot)) => {
                let mut capture = DiagnosticCaptureState {
                    enabled: false,
                    started_at: snapshot.started_at,
                    max_bytes: snapshot.max_bytes.max(1),
                    captured_bytes: 0,
                    limit_reached: snapshot.limit_reached,
                    stop_reason: snapshot.stop_reason,
                    records: snapshot.records,
                    attempt_started: HashMap::new(),
                };
                capture.captured_bytes = diagnostic_capture_size(&capture.records);
                if capture.captured_bytes >= capture.max_bytes {
                    capture.limit_reached = true;
                    capture.stop_reason = Some(CAPTURE_STOP_CAPACITY.into());
                }
                (capture, true)
            }
            Some(Err(error)) => {
                tracing::warn!("diagnostic_capture.json 加载失败，本次运行拒绝覆盖: {error}");
                (DiagnosticCaptureState::default(), false)
            }
            None => (DiagnosticCaptureState::default(), true),
        };
        let (notices, _) = tokio::sync::broadcast::channel(512);
        let engine = Self {
            inner: Arc::new(EngineInner {
                config: RwLock::new(Arc::new(config)),
                generation: RwLock::new(generation),
                transport: services.transport,
                platform: services.platform,
                state: Mutex::new(EngineState {
                    runtime,
                    last_error: stats_error,
                    stats_durability_warning: None,
                    provider_model_health: HashMap::new(),
                    session_sticky,
                    last_session_prune_at: now,
                }),
                dir,
                stats_writable: AtomicBool::new(stats_writable),
                session_affinity_flush: Mutex::new(()),
                session_affinity_dirty: AtomicBool::new(session_affinity_pruned),
                session_affinity_writable: AtomicBool::new(session_affinity_writable),
                resource_bindings_flush: Mutex::new(()),
                resource_bindings_dirty: AtomicBool::new(resource_bindings_pruned),
                resource_bindings_writable: AtomicBool::new(resource_bindings_writable),
                capture_flush: Mutex::new(()),
                capture_dirty: AtomicBool::new(false),
                capture_writable: AtomicBool::new(capture_writable),
                notices,
                started_at: Instant::now(),
                capture_index: Mutex::new(capture_index_cache_from_capture(&capture)),
                capture: Mutex::new(capture),
                runtime_store,
                runtime_write: Mutex::new(()),
                realtime_client_secrets: Mutex::new(HashMap::new()),
                live_sessions: Mutex::new(live_sessions),
                video_sessions: Mutex::new(video_sessions),
            }),
        };
        // Loading is authoritative at construction time. If stale entries
        // were pruned, persist the cleaned snapshot immediately rather than
        // leaving expired bindings on disk until a background task starts or
        // an unrelated request happens to flush state.
        engine.flush_resource_bindings_if_dirty();
        engine
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EngineNotice> {
        self.inner.notices.subscribe()
    }

    pub fn publish_proxy_state(&self, running: bool, host: Option<String>, port: Option<u16>) {
        let _ = self.inner.notices.send(EngineNotice::ProxyState {
            running,
            host,
            port,
        });
    }

    pub fn config(&self) -> Arc<AppConfig> {
        self.inner.config.read().unwrap().clone()
    }

    pub fn generation(&self) -> String {
        self.inner.generation.read().unwrap().clone()
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.inner.started_at.elapsed().as_secs()
    }
}
