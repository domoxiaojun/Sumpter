use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::os::unix::fs::MetadataExt;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};
use sumpter_core::access;
use sumpter_core::events::{
    ClientKind, KIND_CLIENT, KIND_NOTIFY, KIND_UPSTREAM, RuntimeEvent, RuntimeEventOutcome,
    RuntimeEventPhase, RuntimeFailureKind, unix_to_apple_epoch,
};

use sumpter_engine::boundary::{
    AccessDecision, ControlRequestMeta, EngineCapabilities, PlatformAction, PlatformBoundary,
    PlatformNotice, PlatformRequest,
};

/// macOS control policy.  The full notification/reload side effects are kept
/// behind this file-level boundary; the common data-plane engine never stores a
/// control token or assumes a notification implementation.
pub struct Platform {
    control_token: String,
    notify_dedup: Mutex<std::collections::HashMap<String, f64>>,
}

pub fn validate_opened_capture(
    path_metadata: &std::fs::Metadata,
    opened_metadata: &std::fs::Metadata,
) -> Result<(), String> {
    if path_metadata.dev() != opened_metadata.dev() || path_metadata.ino() != opened_metadata.ino()
    {
        return Err("诊断捕获快照在打开前已被替换，拒绝导出".into());
    }
    Ok(())
}

impl Platform {
    pub fn new(control_token: String) -> Self {
        Self {
            control_token,
            notify_dedup: Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn query_token_ok(&self, query: Option<&str>) -> bool {
        // An empty token means the macOS control boundary was not injected.
        // Never let `?token=` turn that missing credential into an allow.
        if self.control_token.is_empty() {
            return false;
        }
        let Some(query) = query else { return false };
        query.split('&').any(|pair| {
            pair.split_once('=')
                .is_some_and(|(key, value)| key == "token" && value == self.control_token)
        })
    }

    fn query_value<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
        query?.split('&').find_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            (name == key && !value.is_empty()).then_some(value)
        })
    }

    fn request_query(request: &PlatformRequest) -> Option<&str> {
        request
            .path_and_query
            .split_once('?')
            .map(|(_, query)| query)
    }

    fn should_emit_notify(&self, key: &str) -> bool {
        let now = now_unix();
        let mut seen = self.notify_dedup.lock().unwrap();
        seen.retain(|_, timestamp| now - *timestamp < NOTIFY_DEDUP_TTL_SECS);
        if seen.contains_key(key) {
            return false;
        }
        seen.insert(key.to_string(), now);
        true
    }

    async fn handle_notify(
        &self,
        request: PlatformRequest,
        engine: Arc<dyn EngineCapabilities>,
    ) -> Response {
        let query_owned = Self::request_query(&request).map(str::to_owned);
        let query = query_owned.as_deref();
        // Keep the authentication check ahead of `to_bytes`: a caller with a
        // bad token cannot make the sidecar buffer an arbitrarily large hook
        // payload.
        if !self.query_token_ok(query) {
            return forbidden_response();
        }
        let body = match axum::body::to_bytes(request.body, sumpter_engine::MAX_BODY_BYTES).await {
            Ok(body) => body,
            Err(error) => {
                return sumpter_engine::engine::error_response(
                    StatusCode::BAD_REQUEST,
                    &[("error", "bad_request"), ("message", &error.to_string())],
                );
            }
        };
        let payload: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let pick = |key: &str| {
            payload
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.is_empty())
        };

        let event_kind = Self::query_value(query, "event")
            .map(str::to_string)
            .filter(|value| !value.is_empty())
            .or_else(|| pick("hook_event_name"))
            .or_else(|| pick("type"))
            .unwrap_or_else(|| "notification".into());
        let event_kind = normalize_hook_event(&event_kind);
        let session_id = pick("session_id").or_else(|| pick("sessionId"));
        let cwd = pick("cwd");
        let project = cwd
            .as_deref()
            .and_then(|value| std::path::Path::new(value).file_name())
            .map(|name| name.to_string_lossy().to_string());
        let notification_type = (event_kind == "notification")
            .then(|| pick("notification_type").or_else(|| pick("notificationType")))
            .flatten();
        let (category, priority) = notify_category(&event_kind, notification_type.as_deref());
        let matched_failure = (event_kind == "stop_failure")
            .then(|| recent_claude_failure(engine.as_ref(), session_id.as_deref()))
            .flatten();
        let (default_title, default_message) = notify_presentation(
            &event_kind,
            project.as_deref(),
            notification_type.as_deref(),
            matched_failure.as_ref(),
        );
        let message = pick("message")
            .filter(|value| *value != event_kind)
            .or_else(|| pick("payload"))
            .unwrap_or(default_message);
        let message = if event_kind == "stop_failure" {
            failure_notification_message(pick("error").as_deref(), matched_failure.as_ref())
        } else {
            sanitize_notification_text(&message, 512)
        };
        let title = if event_kind == "stop_failure" {
            default_title
        } else {
            pick("title")
                .filter(|value| *value != "Claude Code")
                .map(|value| sanitize_notification_text(&value, 160))
                .unwrap_or(default_title)
        };
        let sound = pick("sound").map(|value| sanitize_notification_text(&value, 80));
        let action_id = [
            "action_id",
            "actionID",
            "request_id",
            "requestID",
            "tool_use_id",
        ]
        .into_iter()
        .find_map(pick)
        .map(|value| sanitize_notification_text(&value, 160));

        let event = RuntimeEvent {
            client_kind: None,
            codex_metadata: None,
            client_declared: None,
            client_model: None,
            source_format: None,
            target_format: None,
            route_mode: None,
            duration_ms: 0,
            effective_model: None,
            endpoint_id: None,
            endpoint_name: None,
            failover: false,
            feature_rule_id: None,
            failure_detail: None,
            failure_kind: None,
            failure_phase: None,
            id: new_event_id(),
            kind: KIND_NOTIFY.into(),
            message: Some(message.clone()),
            tool_calls: None,
            // Hook notifications are observations, not model requests; keep
            // the legacy event shape (no success/failure counter outcome).
            outcome: None,
            phase: Some(RuntimeEventPhase::Completed),
            pool_id: None,
            request_purpose: None,
            request_id: None,
            session_id: session_id.clone(),
            status_code: 200,
            timestamp: unix_to_apple_epoch(now_unix()),
            ttfb_ms: None,
            stream_trace: None,
            timeout_ms: None,
            upstream_host: None,
            upstream_model: None,
            upstream_request_id: None,
            upstream_status_code: None,
        };
        engine.record_platform_event(event);

        let notice = PlatformNotice::Notify {
            kind: event_kind.clone(),
            title: title.clone(),
            message: message.clone(),
            sound: sound.clone(),
            category: Some(category.into()),
            priority: Some(priority.into()),
            action_id: action_id.clone(),
            session_id: session_id.clone(),
            cwd: cwd.clone(),
        };
        let dedup_key = notify_dedup_key(
            category,
            session_id.as_deref(),
            action_id.as_deref(),
            &message,
        );
        if self.should_emit_notify(&dedup_key) {
            engine.publish_platform_notice(notice);
        }
        sumpter_engine::engine::json_response(StatusCode::OK, &json!({"ok": "true"}))
    }

    async fn handle_reload(
        &self,
        request: PlatformRequest,
        engine: Arc<dyn EngineCapabilities>,
    ) -> Response {
        let query_owned = Self::request_query(&request).map(str::to_owned);
        let query = query_owned.as_deref();
        if !self.query_token_ok(query) {
            return forbidden_response();
        }
        match engine.reload_config() {
            Ok(replacement) => sumpter_engine::engine::json_response(
                StatusCode::OK,
                &json!({
                    "reloading": "true",
                    "reloaded": "true",
                    "generation": replacement.generation,
                    "warnings": replacement.warnings,
                }),
            ),
            Err(error) if error == "no_reload_handler" || error == "reload_not_supported" => {
                sumpter_engine::engine::error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    &[("error", "no_reload_handler")],
                )
            }
            Err(error) => sumpter_engine::engine::error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &[("error", "reload_failed"), ("message", &error)],
            ),
        }
    }
}

#[async_trait]
impl PlatformBoundary for Platform {
    fn authorize_status(&self, request: &ControlRequestMeta<'_>) -> AccessDecision {
        if access::is_loopback(request.remote_ip.map(|ip| ip.to_string()).as_deref())
            || self.query_token_ok(request.query)
        {
            AccessDecision::Allow
        } else {
            AccessDecision::Deny
        }
    }

    fn platform_action(&self, method: &str, path: &str) -> Option<PlatformAction> {
        if method != "POST" {
            return None;
        }
        match path {
            "/__notify" => Some(PlatformAction::Notify),
            "/__reload" => Some(PlatformAction::Reload),
            _ => None,
        }
    }

    fn validate_opened_capture(
        &self,
        path_metadata: &std::fs::Metadata,
        opened_metadata: &std::fs::Metadata,
    ) -> Result<(), String> {
        validate_opened_capture(path_metadata, opened_metadata)
    }

    async fn handle_platform_action(
        &self,
        action: PlatformAction,
        request: PlatformRequest,
        engine: Arc<dyn EngineCapabilities>,
    ) -> Response {
        match action {
            PlatformAction::Notify => self.handle_notify(request, engine).await,
            PlatformAction::Reload => self.handle_reload(request, engine).await,
        }
    }
}

const NOTIFY_DEDUP_TTL_SECS: f64 = 5.0;
const NOTIFY_RUNTIME_MATCH_WINDOW_SECS: f64 = 5.0 * 60.0;

fn now_unix() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

fn forbidden_response() -> Response {
    sumpter_engine::engine::error_response(
        StatusCode::FORBIDDEN,
        &[("error", "forbidden: bad/missing token")],
    )
}

fn new_event_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>();
    format!(
        "{}{}{}{}-{}{}-{}{}-{}{}-{}{}{}{}{}{}",
        hex[0],
        hex[1],
        hex[2],
        hex[3],
        hex[4],
        hex[5],
        hex[6],
        hex[7],
        hex[8],
        hex[9],
        hex[10],
        hex[11],
        hex[12],
        hex[13],
        hex[14],
        hex[15]
    )
}

fn normalize_hook_event(event: &str) -> String {
    match event {
        "Notification" => "notification".into(),
        "Stop" => "stop".into(),
        "StopFailure" => "stop_failure".into(),
        "SubagentStop" => "subagent_stop".into(),
        other => other.to_string(),
    }
}

fn notify_category(event: &str, notification_type: Option<&str>) -> (&'static str, &'static str) {
    match event {
        "stop_failure" => ("turn_failed", "high"),
        "subagent_stop" => ("subtask_completed", "normal"),
        "stop" => ("turn_completed", "normal"),
        "notification" => match notification_type {
            None
            | Some(
                "permission_prompt"
                | "worker_permission_prompt"
                | "elicitation_dialog"
                | "elicitation_url_dialog"
                | "idle_prompt",
            ) => ("action_required", "high"),
            _ => ("status", "low"),
        },
        _ => ("status", "low"),
    }
}

fn notify_presentation(
    event: &str,
    project: Option<&str>,
    notification_type: Option<&str>,
    matched_failure: Option<&RuntimeEvent>,
) -> (String, String) {
    match event {
        "notification" if notification_type.is_none() => (
            "Claude Code · 等待确认".into(),
            "Claude Code 正在等待你的输入或确认".into(),
        ),
        "notification" if notify_category(event, notification_type).0 == "action_required" => (
            "Claude Code · 需要你处理".into(),
            notification_type
                .map(notification_type_message)
                .unwrap_or_else(|| "Claude Code 正在等待你的输入或确认".into()),
        ),
        "notification" => (
            "Claude Code · 状态更新".into(),
            notification_type
                .map(notification_type_message)
                .unwrap_or_else(|| "Claude Code 有一条状态更新".into()),
        ),
        "stop" => (
            "Claude Code · 回合结束".into(),
            project
                .map(|value| format!("「{value}」的回合已结束"))
                .unwrap_or_else(|| "对话回合已结束".into()),
        ),
        "subagent_stop" => (
            "Claude Code · 子任务结束".into(),
            project
                .map(|value| format!("「{value}」的子任务已结束"))
                .unwrap_or_else(|| "子任务已结束".into()),
        ),
        "stop_failure" => (
            "Claude Code · 回合异常".into(),
            failure_notification_message(None, matched_failure),
        ),
        other => (format!("Claude Code · {other}"), "Claude Code 事件".into()),
    }
}

fn recent_claude_failure(
    engine: &dyn EngineCapabilities,
    session_id: Option<&str>,
) -> Option<RuntimeEvent> {
    let session_id = session_id?;
    let now = unix_to_apple_epoch(now_unix());
    let runtime = engine.runtime_snapshot();
    let client = runtime
        .recent_events
        .iter()
        .filter(|event| {
            event.kind == KIND_CLIENT
                && event.client_kind == Some(ClientKind::ClaudeCode)
                && event.session_id.as_deref() == Some(session_id)
                && event.outcome == Some(RuntimeEventOutcome::Failed)
                && now - event.timestamp <= NOTIFY_RUNTIME_MATCH_WINDOW_SECS
        })
        .max_by(|a, b| a.timestamp.total_cmp(&b.timestamp))?
        .clone();
    if client.upstream_status_code.is_some() {
        return Some(client);
    }
    let upstream_status = client.request_id.as_deref().and_then(|request_id| {
        runtime
            .recent_events
            .iter()
            .filter(|event| {
                event.kind == KIND_UPSTREAM
                    && event.request_id.as_deref() == Some(request_id)
                    && event.outcome == Some(RuntimeEventOutcome::Failed)
            })
            .max_by(|a, b| a.timestamp.total_cmp(&b.timestamp))
            .and_then(|event| event.upstream_status_code)
    });
    upstream_status
        .map(|status| {
            let mut enriched = client.clone();
            enriched.upstream_status_code = Some(status);
            enriched
        })
        .or(Some(client))
}

fn failure_notification_message(error: Option<&str>, matched: Option<&RuntimeEvent>) -> String {
    let base = matched
        .and_then(|event| event.failure_kind)
        .map(failure_kind_message)
        .or_else(|| error.map(stop_failure_error_message))
        .unwrap_or_else(|| "上游返回异常，回合未完成".into());
    if let Some(status) = matched.and_then(|event| event.upstream_status_code) {
        format!("{base}（上游返回 HTTP {status}）")
    } else {
        base
    }
}

fn stop_failure_error_message(error: &str) -> String {
    match error {
        "rate_limit" => "上游限流，回合未完成".into(),
        "authentication_failed" => "上游认证失败，回合未完成".into(),
        "billing_error" => "上游账户或计费异常，回合未完成".into(),
        "invalid_request" => "请求参数被上游拒绝，回合未完成".into(),
        "server_error" => "上游服务异常，回合未完成".into(),
        "max_output_tokens" => "输出达到上限，回合未完成".into(),
        _ => "上游返回异常，回合未完成".into(),
    }
}

fn failure_kind_message(kind: RuntimeFailureKind) -> String {
    match kind {
        RuntimeFailureKind::ResponseTimeout => "上游响应超时，回合未完成".into(),
        RuntimeFailureKind::ConnectionFailed => "无法连接上游，回合未完成".into(),
        RuntimeFailureKind::InvalidResponse => "上游响应无效，回合未完成".into(),
        RuntimeFailureKind::UpstreamHttpStatus => "上游返回错误状态，回合未完成".into(),
        RuntimeFailureKind::StreamIdleTimeout => "上游响应长时间无数据，回合未完成".into(),
        RuntimeFailureKind::StreamInterrupted => "上游响应流中断，回合未完成".into(),
        RuntimeFailureKind::UpstreamResponseIncomplete => "上游响应不完整，回合未完成".into(),
        RuntimeFailureKind::UpstreamResponseFailed => "上游响应失败，回合未完成".into(),
        RuntimeFailureKind::EndpointsExhausted => "所有上游入口均失败，回合未完成".into(),
        RuntimeFailureKind::ClientCancelled => "回合由客户端中断".into(),
        RuntimeFailureKind::ClientRequestRejected => "请求未被代理接受，回合未完成".into(),
    }
}

fn notification_type_message(notification_type: &str) -> String {
    match notification_type {
        "permission_prompt" | "worker_permission_prompt" => "Claude Code 正在等待权限决定".into(),
        "elicitation_dialog" | "elicitation_url_dialog" => "Claude Code 正在等待你的选择".into(),
        "idle_prompt" => "Claude Code 正在等待你继续输入".into(),
        "auth_success" => "Claude Code 上游认证已成功".into(),
        "elicitation_complete" => "Claude Code 的选择流程已完成".into(),
        "elicitation_response" => "Claude Code 已收到你的选择".into(),
        "computer_use_enter" => "Claude Code 进入了计算机控制状态".into(),
        "computer_use_exit" => "Claude Code 退出了计算机控制状态".into(),
        other => format!(
            "Claude Code 状态更新：{}",
            sanitize_notification_text(other, 120)
        ),
    }
}

fn sanitize_notification_text(value: &str, max_chars: usize) -> String {
    let sanitized: String = value
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect();
    let mut result: String = sanitized.chars().take(max_chars).collect();
    if sanitized.chars().count() > max_chars {
        result.push('…');
    }
    result
}

fn notify_dedup_key(
    category: &str,
    session_id: Option<&str>,
    action_id: Option<&str>,
    message: &str,
) -> String {
    let mut hasher = DefaultHasher::new();
    message.hash(&mut hasher);
    format!(
        "{category}|{}|{}|{:016x}",
        session_id.unwrap_or("<no-session>"),
        action_id.unwrap_or("<no-action>"),
        hasher.finish()
    )
}
