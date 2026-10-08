//! Live Responses connections are process-local gauges, never historical events.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;

use super::Engine;
use super::websocket_relay::WebSocketEventContext;

#[derive(Clone, Copy)]
pub(super) enum ConnectionStage {
    AwaitingFirstMessage,
    ConnectingUpstream,
    Relaying,
}

pub(super) struct ConnectionEntry {
    stage: ConnectionStage,
    guardian: bool,
    started: Instant,
}

pub(super) type ConnectionRegistry = Mutex<HashMap<String, ConnectionEntry>>;

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ResponsesWebSocketConnections {
    total: usize,
    awaiting_first_message: usize,
    guardian_awaiting_first_message: usize,
    connecting_upstream: usize,
    relaying: usize,
    #[serde(rename = "oldestFirstMessageWaitMS")]
    oldest_first_message_wait_ms: Option<i64>,
}

pub(super) fn is_explicit_guardian(context: &WebSocketEventContext) -> bool {
    context.client_kind == sumpter_core::events::ClientKind::Codex
        && context.codex_metadata.as_ref().is_some_and(|metadata| {
            !metadata.malformed
                && !metadata.truncated
                && !metadata.has_conflicts
                && metadata
                    .subagent_kind
                    .as_deref()
                    .or(metadata.subagent_header.as_deref())
                    == Some("guardian")
        })
}

/// Owns exactly one upgraded connection, including cancellation/early returns.
pub(super) struct ConnectionGuard {
    engine: Engine,
    id: String,
}

impl ConnectionGuard {
    pub(super) fn set_stage(&self, stage: ConnectionStage) {
        let mut connections = self
            .engine
            .inner
            .responses_websocket_connections
            .lock()
            .unwrap();
        if let Some(entry) = connections.get_mut(&self.id) {
            entry.stage = stage;
        }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.engine
            .inner
            .responses_websocket_connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}

impl Engine {
    pub(super) fn track_responses_connection(
        &self,
        context: &WebSocketEventContext,
        stage: ConnectionStage,
    ) -> ConnectionGuard {
        self.inner
            .responses_websocket_connections
            .lock()
            .unwrap()
            .insert(
                context.request_id.clone(),
                ConnectionEntry {
                    stage,
                    guardian: is_explicit_guardian(context),
                    started: context.started,
                },
            );
        ConnectionGuard {
            engine: self.clone(),
            id: context.request_id.clone(),
        }
    }

    pub(super) fn responses_websocket_connections(&self) -> ResponsesWebSocketConnections {
        let connections = self.inner.responses_websocket_connections.lock().unwrap();
        let now = Instant::now();
        let mut summary = ResponsesWebSocketConnections {
            total: connections.len(),
            ..Default::default()
        };
        for entry in connections.values() {
            match entry.stage {
                ConnectionStage::AwaitingFirstMessage => {
                    summary.awaiting_first_message += 1;
                    summary.guardian_awaiting_first_message += usize::from(entry.guardian);
                    let wait = now
                        .saturating_duration_since(entry.started)
                        .as_millis()
                        .min(i64::MAX as u128) as i64;
                    summary.oldest_first_message_wait_ms =
                        Some(summary.oldest_first_message_wait_ms.unwrap_or(0).max(wait));
                }
                ConnectionStage::ConnectingUpstream => summary.connecting_upstream += 1,
                ConnectionStage::Relaying => summary.relaying += 1,
            }
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::super::protocol::RealtimeRouteIntent;
    use super::super::websocket_relay::websocket_event_context;
    use super::*;
    use crate::replay::ReplayTransport;
    use std::sync::Arc;
    use std::time::Duration;
    use sumpter_core::config::AppConfig;
    use sumpter_core::config_store::ConfigDir;

    fn context() -> WebSocketEventContext {
        websocket_event_context(
            None,
            "/v1/responses",
            &[],
            "",
            RealtimeRouteIntent::StandardRealtime,
            Instant::now(),
        )
    }

    #[tokio::test]
    async fn live_summary_survives_stats_reset_but_not_guard_or_engine_lifetime() {
        for persistent in [false, true] {
            let temp = std::env::temp_dir().join(format!(
                "sumpter-live-ws-{}",
                super::super::events::new_event_id()
            ));
            std::fs::create_dir_all(&temp).unwrap();
            let engine = Engine::new(
                AppConfig::bootstrap(),
                persistent.then(|| ConfigDir::new(&temp)),
                Arc::new(ReplayTransport::new([])),
            );
            let mut context = context();
            context.started -= Duration::from_secs(2);
            let guard =
                engine.track_responses_connection(&context, ConnectionStage::AwaitingFirstMessage);
            let summary = engine.runtime_summary_value();
            let live = &summary["responsesWebSocketConnections"];
            assert_eq!(summary["apiVersion"], 1);
            assert_eq!(live["total"], 1);
            assert_eq!(live["awaitingFirstMessage"], 1);
            assert!(live["oldestFirstMessageWaitMS"].as_i64().unwrap() >= 2000);
            if persistent {
                engine.reset_runtime().unwrap();
                assert_eq!(engine.responses_websocket_connections().total, 1);
            }
            guard.set_stage(ConnectionStage::ConnectingUpstream);
            assert_eq!(
                engine.responses_websocket_connections().connecting_upstream,
                1
            );
            assert_eq!(
                engine
                    .responses_websocket_connections()
                    .oldest_first_message_wait_ms,
                None
            );
            guard.set_stage(ConnectionStage::Relaying);
            assert_eq!(engine.responses_websocket_connections().relaying, 1);
            drop(guard);
            assert_eq!(
                engine.responses_websocket_connections(),
                ResponsesWebSocketConnections::default()
            );
            drop(engine);
            let restarted = Engine::new(
                AppConfig::bootstrap(),
                persistent.then(|| ConfigDir::new(&temp)),
                Arc::new(ReplayTransport::new([])),
            );
            assert_eq!(
                restarted.responses_websocket_connections(),
                ResponsesWebSocketConnections::default()
            );
            drop(restarted);
            std::fs::remove_dir_all(temp).unwrap();
        }
    }

    #[tokio::test]
    async fn aborted_task_drops_connection_registration() {
        let engine = Engine::new(
            AppConfig::bootstrap(),
            None,
            Arc::new(ReplayTransport::new([])),
        );
        let guard =
            engine.track_responses_connection(&context(), ConnectionStage::AwaitingFirstMessage);
        let task = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            engine.responses_websocket_connections(),
            ResponsesWebSocketConnections::default()
        );
    }
}
