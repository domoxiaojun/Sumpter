//! Sessions implementation for the shared engine.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use serde_json::Value;

use sumpter_core::config_store::{ResourceBinding, StickySessionAssignment};
use sumpter_core::routing::sticky;
use sumpter_core::scheduler;

use super::Engine;
use super::protocol::{json_timestamp, live_call_id_from_target, video_id_from_path};
use super::state::now_unix;
pub(super) const SESSION_STICKY_MAX_ENTRIES: usize = 2000;
pub(super) const SESSION_STICKY_PRUNE_INTERVAL_SECS: f64 = 60.0;
#[derive(Clone)]
pub(super) struct SessionStickyEntry {
    pub(super) label: String,
    pub(super) at: f64,
    pub(super) persistent: bool,
}

/// 按 TTL + 上限清理会话归属，返回是否删除了需要同步回磁盘的持久条目。
/// `ttl_secs <= 0` 表示不按 TTL 淘汰(仍受条目数上限约束),见 scheduler。
pub(super) fn prune_session_sticky(
    sticky: &mut HashMap<String, SessionStickyEntry>,
    now: f64,
    ttl_secs: f64,
) -> bool {
    let entries: Vec<(String, f64, bool)> = sticky
        .iter()
        .map(|(key, entry)| (key.clone(), entry.at, entry.persistent))
        .collect();
    let mut removed_persistent = false;
    for key in
        scheduler::session_sticky_evictions(&entries, now, ttl_secs, SESSION_STICKY_MAX_ENTRIES)
    {
        removed_persistent |= sticky.remove(&key).is_some_and(|entry| entry.persistent);
    }
    removed_persistent
}
/// Realtime ephemeral keys are short-lived upstream credentials. Keep only a
/// bounded, in-memory allow-list so a listener token can remain enabled while
/// a client uses the `ek_…` returned by `/v1/realtime/client_secrets`.
pub(super) const MAX_REALTIME_CLIENT_SECRETS: usize = 256;
pub(super) const DEFAULT_REALTIME_CLIENT_SECRET_TTL_SECS: f64 = 300.0;
pub(super) const MAX_LIVE_SESSIONS: usize = 256;
pub(super) const DEFAULT_LIVE_SESSION_TTL_SECS: f64 = 900.0;
/// Video objects remain addressable while an asynchronous render is queued;
/// keep their binding for a day, independently from the short Live call TTL.
pub(super) const DEFAULT_VIDEO_SESSION_TTL_SECS: f64 = 24.0 * 3600.0;
pub(super) const MAX_RESOURCE_BINDING_ID_BYTES: usize = 128;
pub(super) const MAX_RESOURCE_BINDING_TEXT_BYTES: usize = 256;

#[derive(Clone)]
pub(super) struct RealtimeClientSecretEntry {
    pub(super) expires_at: f64,
    pub(super) endpoint_id: Option<String>,
    pub(super) model: Option<String>,
    /// The session configuration returned by the upstream client-secret
    /// endpoint.  CPA replays this configuration with `session.update` when
    /// the ephemeral key opens a WebSocket, so voice/instructions survive the
    /// two-step browser flow.
    pub(super) session: Option<Value>,
}

#[derive(Clone)]
pub(super) struct LiveSessionEntry {
    pub(super) expires_at: f64,
    pub(super) endpoint_id: String,
    /// Logical model selected during the bootstrap. Sideband requests often
    /// omit `model`; retaining it prevents them from being re-routed through
    /// the standard Realtime default.
    pub(super) model: String,
}

pub(super) fn valid_resource_binding_text(value: &str) -> bool {
    let trimmed = value.trim();
    value == trimmed
        && !trimmed.is_empty()
        && trimmed.len() <= MAX_RESOURCE_BINDING_TEXT_BYTES
        && !trimmed.chars().any(char::is_control)
}

pub(super) fn valid_resource_binding_id(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= MAX_RESOURCE_BINDING_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub(super) fn realtime_ephemeral_token(headers: &[(String, String)]) -> Option<String> {
    headers.iter().rev().find_map(|(name, value)| {
        let candidate = if name.eq_ignore_ascii_case("authorization") {
            let value = value.trim();
            value
                .get(..7)
                .filter(|prefix| prefix.eq_ignore_ascii_case("bearer "))
                .map(|_| value[7..].trim())
        } else if name.eq_ignore_ascii_case("x-api-key") {
            Some(value.trim())
        } else {
            None
        }?;
        (candidate.starts_with("ek_")
            && candidate.len() <= 512
            && !candidate.chars().any(char::is_control))
        .then(|| candidate.to_string())
    })
}
impl Engine {
    /// Remember an upstream Realtime ephemeral key without persisting or
    /// exposing it through runtime events. The expiration is bounded so a
    /// malformed/overly long upstream lifetime cannot turn it into a durable
    /// listener credential.
    pub(super) fn register_realtime_client_secret(
        &self,
        token: &str,
        expires_at: Option<f64>,
        endpoint_id: Option<&str>,
        model: Option<&str>,
        session: Option<Value>,
    ) {
        let token = token.trim();
        if !token.starts_with("ek_") || token.len() > 512 || token.chars().any(char::is_control) {
            return;
        }
        let now = now_unix();
        let requested_expiry = expires_at
            .filter(|value| value.is_finite() && *value > now)
            .unwrap_or(now + DEFAULT_REALTIME_CLIENT_SECRET_TTL_SECS);
        let expiry = requested_expiry.min(now + DEFAULT_REALTIME_CLIENT_SECRET_TTL_SECS);
        let mut secrets = self.inner.realtime_client_secrets.lock().unwrap();
        secrets.retain(|_, value| value.expires_at > now);
        if secrets.len() >= MAX_REALTIME_CLIENT_SECRETS
            && !secrets.contains_key(token)
            && let Some(oldest) = secrets
                .iter()
                .min_by(|(_, left), (_, right)| left.expires_at.total_cmp(&right.expires_at))
                .map(|(key, _)| key.clone())
        {
            secrets.remove(&oldest);
        }
        secrets.insert(
            token.to_string(),
            RealtimeClientSecretEntry {
                expires_at: expiry,
                endpoint_id: endpoint_id
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                model: model
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                session,
            },
        );
    }

    pub(super) fn realtime_client_secret_authorized(&self, headers: &[(String, String)]) -> bool {
        let Some(token) = realtime_ephemeral_token(headers) else {
            return false;
        };
        let now = now_unix();
        let mut secrets = self.inner.realtime_client_secrets.lock().unwrap();
        secrets.retain(|_, value| value.expires_at > now);
        secrets
            .get(&token)
            .is_some_and(|entry| entry.expires_at > now)
    }

    pub(super) fn realtime_client_secret_endpoint(
        &self,
        headers: &[(String, String)],
    ) -> Option<String> {
        let token = realtime_ephemeral_token(headers)?;
        let now = now_unix();
        let mut secrets = self.inner.realtime_client_secrets.lock().unwrap();
        secrets.retain(|_, value| value.expires_at > now);
        secrets
            .get(&token)
            .and_then(|entry| entry.endpoint_id.clone())
    }

    pub(super) fn realtime_client_secret_model(
        &self,
        headers: &[(String, String)],
    ) -> Option<String> {
        let token = realtime_ephemeral_token(headers)?;
        let now = now_unix();
        let mut secrets = self.inner.realtime_client_secrets.lock().unwrap();
        secrets.retain(|_, value| value.expires_at > now);
        secrets.get(&token).and_then(|entry| entry.model.clone())
    }

    pub(super) fn realtime_client_secret_session(
        &self,
        headers: &[(String, String)],
    ) -> Option<Value> {
        let token = realtime_ephemeral_token(headers)?;
        let now = now_unix();
        let mut secrets = self.inner.realtime_client_secrets.lock().unwrap();
        secrets.retain(|_, value| value.expires_at > now);
        secrets.get(&token).and_then(|entry| entry.session.clone())
    }

    pub(super) fn register_realtime_client_secret_from_body(
        &self,
        body: &[u8],
        endpoint_id: Option<&str>,
        model: Option<&str>,
        fallback_session: Option<Value>,
    ) {
        let Ok(value) = serde_json::from_slice::<Value>(body) else {
            return;
        };
        let root_expiry = value.get("expires_at").and_then(json_timestamp);
        let candidate = value
            .get("value")
            .and_then(Value::as_str)
            .map(|token| (token, root_expiry))
            .or_else(|| {
                let secret = value.get("client_secret")?;
                if let Some(token) = secret.as_str() {
                    return Some((token, root_expiry));
                }
                let object = secret.as_object()?;
                let token = object.get("value").and_then(Value::as_str)?;
                Some((
                    token,
                    object
                        .get("expires_at")
                        .and_then(json_timestamp)
                        .or(root_expiry),
                ))
            });
        let session = value
            .get("session")
            .and_then(Value::as_object)
            .map(|session| {
                let mut session = session.clone();
                // The public client-secret response adds bookkeeping fields that
                // are not part of the session configuration CPA replays.
                for field in ["id", "object", "expires_at", "client_secret"] {
                    session.remove(field);
                }
                Value::Object(session)
            })
            .or(fallback_session);
        if let Some((token, expires_at)) = candidate {
            self.register_realtime_client_secret(token, expires_at, endpoint_id, model, session);
        }
    }

    pub(super) fn register_live_session(&self, call_id: &str, endpoint_id: &str, model: &str) {
        let call_id = call_id.trim();
        let now = now_unix();
        if !valid_resource_binding_id(call_id)
            || !valid_resource_binding_text(endpoint_id)
            || !valid_resource_binding_text(model)
            || !DEFAULT_LIVE_SESSION_TTL_SECS.is_finite()
            || DEFAULT_LIVE_SESSION_TTL_SECS <= 0.0
            || !now.is_finite()
        {
            return;
        }
        if !(now + DEFAULT_LIVE_SESSION_TTL_SECS).is_finite() {
            return;
        }
        {
            let mut sessions = self.inner.live_sessions.lock().unwrap();
            sessions.retain(|_, entry| entry.expires_at > now);
            if sessions.len() >= MAX_LIVE_SESSIONS
                && !sessions.contains_key(call_id)
                && let Some(oldest) = sessions
                    .iter()
                    .min_by(|(_, left), (_, right)| left.expires_at.total_cmp(&right.expires_at))
                    .map(|(key, _)| key.clone())
            {
                sessions.remove(&oldest);
            }
            sessions.insert(
                call_id.to_string(),
                LiveSessionEntry {
                    expires_at: now + DEFAULT_LIVE_SESSION_TTL_SECS,
                    endpoint_id: endpoint_id.trim().to_string(),
                    model: model.trim().to_string(),
                },
            );
        }
        self.inner
            .resource_bindings_dirty
            .store(true, Ordering::Release);
        self.flush_resource_bindings_if_dirty();
    }

    /// Return the originating endpoint for a sideband target. The outer
    /// `Option` indicates whether the target carried a call id; the inner
    /// value is `None` when that id is unknown or expired. Keeping those
    /// states distinct is important: an unknown Live id must not silently
    /// fall back to ordinary provider selection.
    pub(super) fn live_session_endpoint(&self, path_and_query: &str) -> Option<Option<String>> {
        let call_id = live_call_id_from_target(path_and_query)?;
        let now = now_unix();
        let mut sessions = self.inner.live_sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|_, entry| entry.expires_at > now);
        let pruned = sessions.len() != before;
        if pruned {
            self.inner
                .resource_bindings_dirty
                .store(true, Ordering::Release);
        }
        let result = Some(
            sessions
                .get(&call_id)
                .map(|entry| entry.endpoint_id.clone()),
        );
        drop(sessions);
        if pruned {
            self.flush_resource_bindings_if_dirty();
        }
        result
    }

    pub(super) fn live_session_model(&self, path_and_query: &str) -> Option<Option<String>> {
        let call_id = live_call_id_from_target(path_and_query)?;
        let now = now_unix();
        let mut sessions = self.inner.live_sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|_, entry| entry.expires_at > now);
        let pruned = sessions.len() != before;
        if pruned {
            self.inner
                .resource_bindings_dirty
                .store(true, Ordering::Release);
        }
        let result = Some(sessions.get(&call_id).map(|entry| entry.model.clone()));
        drop(sessions);
        if pruned {
            self.flush_resource_bindings_if_dirty();
        }
        result
    }

    pub(super) fn register_video_session(&self, video_id: &str, endpoint_id: &str, model: &str) {
        self.register_live_session_into(
            &self.inner.video_sessions,
            video_id,
            endpoint_id,
            model,
            DEFAULT_VIDEO_SESSION_TTL_SECS,
        );
    }

    pub(super) fn register_live_session_into(
        &self,
        store: &Mutex<HashMap<String, LiveSessionEntry>>,
        session_id: &str,
        endpoint_id: &str,
        model: &str,
        ttl_secs: f64,
    ) {
        let session_id = session_id.trim();
        if !valid_resource_binding_id(session_id)
            || !valid_resource_binding_text(endpoint_id)
            || !valid_resource_binding_text(model)
            || !ttl_secs.is_finite()
            || ttl_secs <= 0.0
        {
            return;
        }
        let now = now_unix();
        if !now.is_finite() || !(now + ttl_secs).is_finite() {
            return;
        }
        {
            let mut sessions = store.lock().unwrap();
            sessions.retain(|_, entry| entry.expires_at > now);
            if sessions.len() >= MAX_LIVE_SESSIONS
                && !sessions.contains_key(session_id)
                && let Some(oldest) = sessions
                    .iter()
                    .min_by(|(_, left), (_, right)| left.expires_at.total_cmp(&right.expires_at))
                    .map(|(key, _)| key.clone())
            {
                sessions.remove(&oldest);
            }
            sessions.insert(
                session_id.to_string(),
                LiveSessionEntry {
                    expires_at: now + ttl_secs,
                    endpoint_id: endpoint_id.trim().to_string(),
                    model: model.trim().to_string(),
                },
            );
        }
        self.inner
            .resource_bindings_dirty
            .store(true, Ordering::Release);
        self.flush_resource_bindings_if_dirty();
    }

    pub(super) fn video_session_endpoint(&self, path: &str) -> Option<Option<String>> {
        let video_id = video_id_from_path(path)?;
        let now = now_unix();
        let mut sessions = self.inner.video_sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|_, entry| entry.expires_at > now);
        let pruned = sessions.len() != before;
        if pruned {
            self.inner
                .resource_bindings_dirty
                .store(true, Ordering::Release);
        }
        let result = Some(
            sessions
                .get(video_id)
                .map(|entry| entry.endpoint_id.clone()),
        );
        drop(sessions);
        if pruned {
            self.flush_resource_bindings_if_dirty();
        }
        result
    }

    pub(super) fn video_session_model(&self, path: &str) -> Option<Option<String>> {
        let video_id = video_id_from_path(path)?;
        let now = now_unix();
        let mut sessions = self.inner.video_sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|_, entry| entry.expires_at > now);
        let pruned = sessions.len() != before;
        if pruned {
            self.inner
                .resource_bindings_dirty
                .store(true, Ordering::Release);
        }
        let result = Some(sessions.get(video_id).map(|entry| entry.model.clone()));
        drop(sessions);
        if pruned {
            self.flush_resource_bindings_if_dirty();
        }
        result
    }

    pub fn flush_session_affinity_if_dirty(&self) {
        if !self.inner.session_affinity_writable.load(Ordering::Acquire)
            || !self
                .inner
                .session_affinity_dirty
                .swap(false, Ordering::AcqRel)
        {
            return;
        }
        if let Err(error) = self.flush_session_affinity() {
            self.inner
                .session_affinity_dirty
                .store(true, Ordering::Release);
            tracing::warn!("session_affinity.json 落盘失败: {error}");
        }
    }

    pub fn flush_session_affinity(&self) -> Result<(), String> {
        if !self.inner.session_affinity_writable.load(Ordering::Acquire) {
            return Err("session_affinity.json 加载失败，本次运行拒绝覆盖".into());
        }
        let Some(dir) = &self.inner.dir else {
            return Ok(());
        };
        let _flush = self.inner.session_affinity_flush.lock().unwrap();
        let assignments = self
            .inner
            .state
            .lock()
            .unwrap()
            .session_sticky
            .iter()
            .filter(|(_, entry)| entry.persistent)
            .map(|(key, entry)| {
                (
                    key.clone(),
                    StickySessionAssignment {
                        scheduling_group: entry.label.clone(),
                        updated_at: entry.at,
                    },
                )
            })
            .collect();
        dir.save_session_affinity(&assignments)
            .map_err(|error| error.to_string())
    }

    pub fn flush_resource_bindings_if_dirty(&self) {
        if !self
            .inner
            .resource_bindings_writable
            .load(Ordering::Acquire)
            || !self
                .inner
                .resource_bindings_dirty
                .swap(false, Ordering::AcqRel)
        {
            return;
        }
        // Snapshotting and writing happen without holding the session maps.
        // A registration can race with the write; consume the dirty bit only
        // after a successful snapshot and immediately flush again when a
        // mutation was observed during the write.  On failure restore the bit
        // so the background flusher can retry instead of silently losing the
        // binding across a restart.
        loop {
            if let Err(error) = self.flush_resource_bindings() {
                self.inner
                    .resource_bindings_dirty
                    .store(true, Ordering::Release);
                tracing::warn!("resource_bindings.json 落盘失败: {error}");
                return;
            }
            if !self
                .inner
                .resource_bindings_dirty
                .swap(false, Ordering::AcqRel)
            {
                return;
            }
        }
    }

    pub fn flush_resource_bindings(&self) -> Result<(), String> {
        if !self
            .inner
            .resource_bindings_writable
            .load(Ordering::Acquire)
        {
            return Err("resource_bindings.json 加载失败，本次运行拒绝覆盖".into());
        }
        let Some(dir) = &self.inner.dir else {
            return Ok(());
        };
        let _flush = self.inner.resource_bindings_flush.lock().unwrap();
        let now = now_unix();
        let mut bindings = HashMap::new();
        {
            let sessions = self.inner.live_sessions.lock().unwrap();
            for (id, entry) in sessions.iter().filter(|(_, entry)| entry.expires_at > now) {
                bindings.insert(
                    format!("live:{id}"),
                    ResourceBinding {
                        endpoint_id: entry.endpoint_id.clone(),
                        model: entry.model.clone(),
                        expires_at: entry.expires_at,
                    },
                );
            }
        }
        {
            let sessions = self.inner.video_sessions.lock().unwrap();
            for (id, entry) in sessions.iter().filter(|(_, entry)| entry.expires_at > now) {
                bindings.insert(
                    format!("video:{id}"),
                    ResourceBinding {
                        endpoint_id: entry.endpoint_id.clone(),
                        model: entry.model.clone(),
                        expires_at: entry.expires_at,
                    },
                );
            }
        }
        match dir.save_resource_bindings(&bindings) {
            Ok(outcome) => outcome.durability_warning().map_or(Ok(()), Err),
            Err(error) => Err(error.to_string()),
        }
    }

    pub(super) fn touch_session_success(
        &self,
        group: &str,
        session_key: &sticky::SessionKey,
        initial_group: &str,
        eligible_groups: &[String],
        sticky_enabled: bool,
        now: f64,
    ) {
        let mut state = self.inner.state.lock().unwrap();
        // 统一 Provider 的其它调度组一旦成功立即改绑。
        let replace = sticky_enabled
            && scheduler::should_replace_sticky_assignment(
                state
                    .session_sticky
                    .get(&session_key.value)
                    .map(|entry| entry.label.as_str()),
                group,
                initial_group,
                eligible_groups,
            );
        if replace {
            state.session_sticky.insert(
                session_key.value.clone(),
                SessionStickyEntry {
                    label: group.to_string(),
                    at: now,
                    persistent: session_key.persistent,
                },
            );
            if session_key.persistent {
                self.inner
                    .session_affinity_dirty
                    .store(true, Ordering::Release);
            }
        }
        let needs_prune = now - state.last_session_prune_at >= SESSION_STICKY_PRUNE_INTERVAL_SECS
            || state.session_sticky.len() > SESSION_STICKY_MAX_ENTRIES;
        if needs_prune {
            let ttl_secs = state.session_sticky_ttl_secs;
            let removed_persistent = prune_session_sticky(&mut state.session_sticky, now, ttl_secs);
            state.last_session_prune_at = now;
            if removed_persistent {
                self.inner
                    .session_affinity_dirty
                    .store(true, Ordering::Release);
            }
        }
        drop(state);
        if session_key.persistent
            && replace
            && let Err(error) = self.flush_session_affinity()
        {
            self.inner
                .session_affinity_dirty
                .store(true, Ordering::Release);
            tracing::warn!("session_affinity.json 即时落盘失败: {error}");
        }
    }

    /// 稳定 Claude 会话在首次出站前就建立固定归属，避免请求尚未成功时进程退出导致漂移。
    /// 配置删除/改名使旧组失效时，允许把归属更新为当前计划的首选组。
    pub(super) fn ensure_session_assignment(
        &self,
        session_key: &sticky::SessionKey,
        initial_group: &str,
        eligible_groups: &[String],
        now: f64,
    ) {
        if !session_key.persistent || initial_group.is_empty() {
            return;
        }
        let mut state = self.inner.state.lock().unwrap();
        let replace = scheduler::should_replace_sticky_assignment(
            state
                .session_sticky
                .get(&session_key.value)
                .map(|entry| entry.label.as_str()),
            initial_group,
            initial_group,
            eligible_groups,
        );
        if !replace {
            return;
        }
        state.session_sticky.insert(
            session_key.value.clone(),
            SessionStickyEntry {
                label: initial_group.to_string(),
                at: now,
                persistent: true,
            },
        );
        self.inner
            .session_affinity_dirty
            .store(true, Ordering::Release);
        drop(state);
        if let Err(error) = self.flush_session_affinity() {
            tracing::warn!("session_affinity.json 首次归属落盘失败: {error}");
        }
    }

    /// 按粘性键清除会话归属（运维出口：项目维度的「清除会话粘性」）。
    /// 键来自 runtime 事件里的 affinity 哈希，不触碰其它会话；返回实际删除数。
    /// 返回成功前同步落盘；失败保留 dirty 位并向调用方报告，不伪报持久成功。
    pub fn clear_session_sticky(&self, keys: &[String]) -> Result<usize, String> {
        if keys.is_empty() {
            return Ok(0);
        }
        let mut removed_persistent = false;
        let removed = {
            let mut state = self.inner.state.lock().unwrap();
            let before = state.session_sticky.len();
            for key in keys {
                removed_persistent |= state
                    .session_sticky
                    .remove(key)
                    .is_some_and(|entry| entry.persistent);
            }
            before - state.session_sticky.len()
        };
        // 重复清除也必须重试之前失败的落盘，不能因内存已空就返回成功。
        if (removed_persistent || self.inner.session_affinity_dirty.load(Ordering::Acquire))
            && let Err(error) = self.flush_session_affinity()
        {
            self.inner
                .session_affinity_dirty
                .store(true, Ordering::Release);
            return Err(format!(
                "内存归属已清除，但 session_affinity.json 同步失败：{error}"
            ));
        }
        Ok(removed)
    }
}
