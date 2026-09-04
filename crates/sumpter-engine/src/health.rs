//! 健康评估(菜单栏图标数据源)。对齐 Swift `ProxyHealthEvaluator`:
//! 取最近 20 条**已完成** client 事件、剔除 499(客户端取消不拉低成功率);
//! 无样本 = idle;0 成功 = down;成功率 ≥0.8 = healthy;否则 degraded;未运行 = stopped。

use serde::Serialize;
use sumpter_core::events::{KIND_CLIENT, RuntimeEvent};

const WINDOW: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthState {
    Stopped,
    Idle,
    Healthy,
    Degraded,
    Down,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthSummary {
    pub state: HealthState,
    /// 窗口内成功率(0..=1);无样本为 None。
    #[serde(rename = "successRate", skip_serializing_if = "Option::is_none")]
    pub success_rate: Option<f64>,
    #[serde(rename = "sampleCount")]
    pub sample_count: usize,
    /// 最近一次成功的时间戳(Apple 纪元秒);可指向窗口外。
    #[serde(rename = "lastSuccess", skip_serializing_if = "Option::is_none")]
    pub last_success: Option<f64>,
}

/// `events` 按引擎存储顺序(新在前)。
pub fn evaluate(events: &[RuntimeEvent], running: bool) -> HealthSummary {
    let last_success = events
        .iter()
        .filter(|e| e.kind == KIND_CLIENT && !e.is_in_flight() && e.is_succeeded())
        .map(|e| e.timestamp)
        .fold(None::<f64>, |acc, ts| Some(acc.map_or(ts, |a| a.max(ts))));

    if !running {
        return HealthSummary {
            state: HealthState::Stopped,
            success_rate: None,
            sample_count: 0,
            last_success,
        };
    }

    let window: Vec<&RuntimeEvent> = events
        .iter()
        .filter(|e| e.kind == KIND_CLIENT && !e.is_in_flight() && !e.is_cancelled())
        .take(WINDOW)
        .collect();

    if window.is_empty() {
        return HealthSummary {
            state: HealthState::Idle,
            success_rate: None,
            sample_count: 0,
            last_success,
        };
    }
    let successes = window.iter().filter(|e| e.is_succeeded()).count();
    let rate = successes as f64 / window.len() as f64;
    let state = if successes == 0 {
        HealthState::Down
    } else if rate >= 0.8 {
        HealthState::Healthy
    } else {
        HealthState::Degraded
    };
    HealthSummary {
        state,
        success_rate: Some(rate),
        sample_count: window.len(),
        last_success,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client_event(status: i64, ts: f64) -> RuntimeEvent {
        RuntimeEvent {
            client_kind: None,
            codex_metadata: None,
            client_declared: None,
            grok_metadata: None,
            client_model: None,
            source_format: None,
            target_format: None,
            route_mode: None,
            duration_ms: 1,
            effective_model: None,
            endpoint_id: None,
            endpoint_name: None,
            failover: false,
            feature_rule_id: None,
            failure_detail: None,
            failure_kind: None,
            failure_phase: None,
            id: format!("{status}-{ts}"),
            kind: KIND_CLIENT.into(),
            message: None,
            tool_calls: None,
            outcome: None,
            phase: None,
            pool_id: None,
            request_purpose: None,
            request_id: None,
            request_method: None,
            request_path: None,
            route_intent: None,
            session_id: None,
            status_code: status,
            timestamp: ts,
            ttfb_ms: None,
            stream_trace: None,
            timeout_ms: None,
            upstream_host: None,
            upstream_model: None,
            upstream_request_id: None,
            upstream_status_code: None,
        }
    }

    #[test]
    fn states() {
        assert_eq!(evaluate(&[], false).state, HealthState::Stopped);
        assert_eq!(evaluate(&[], true).state, HealthState::Idle);

        let healthy: Vec<RuntimeEvent> = (0..10)
            .map(|i| client_event(if i == 0 { 500 } else { 200 }, i as f64))
            .collect();
        assert_eq!(evaluate(&healthy, true).state, HealthState::Healthy); // 90%

        let degraded: Vec<RuntimeEvent> = (0..10)
            .map(|i| client_event(if i % 2 == 0 { 500 } else { 200 }, i as f64))
            .collect();
        assert_eq!(evaluate(&degraded, true).state, HealthState::Degraded); // 50%

        let down: Vec<RuntimeEvent> = (0..5).map(|i| client_event(502, i as f64)).collect();
        assert_eq!(evaluate(&down, true).state, HealthState::Down);
    }

    #[test]
    fn cancelled_excluded_and_window_limited() {
        // 全取消 = idle,不拉低成功率。
        let cancelled: Vec<RuntimeEvent> = (0..5).map(|i| client_event(499, i as f64)).collect();
        assert_eq!(evaluate(&cancelled, true).state, HealthState::Idle);

        // 窗口只看最近 20 条:20 条成功在前,老的 50 条失败在后 → healthy。
        let mut events: Vec<RuntimeEvent> = (0..20)
            .map(|i| client_event(200, 100.0 + i as f64))
            .collect();
        events.extend((0..50).map(|i| client_event(502, i as f64)));
        let summary = evaluate(&events, true);
        assert_eq!(summary.state, HealthState::Healthy);
        assert_eq!(summary.sample_count, 20);
    }

    #[test]
    fn last_success_can_point_outside_window() {
        let mut events: Vec<RuntimeEvent> = (0..25)
            .map(|i| client_event(502, 100.0 + i as f64))
            .collect();
        events.push(client_event(200, 42.0)); // 窗口外的旧成功
        let summary = evaluate(&events, true);
        assert_eq!(summary.state, HealthState::Down);
        assert_eq!(summary.last_success, Some(42.0));
    }

    #[test]
    fn failed_stream_with_http_200_is_not_counted_as_success() {
        let mut interrupted = client_event(200, 5.0);
        interrupted.outcome = Some(sumpter_core::events::RuntimeEventOutcome::Failed);
        let summary = evaluate(&[interrupted], true);
        assert_eq!(summary.state, HealthState::Down);
        assert_eq!(summary.success_rate, Some(0.0));
    }
}
