//! sessions regression tests.

use std::sync::Arc;

use sumpter_core::config::AppConfig;
use sumpter_core::config_store::{ConfigDir, ResourceBinding};

use crate::engine::Engine;
use crate::engine::events::new_event_id;

#[test]
fn inbound_tokens_realtime_secrets_inherit_identity_and_stop_when_owner_disabled() {
    use sumpter_core::config::InboundTokenConfig;
    let mut config = AppConfig::bootstrap();
    config.listener.auth_token.clear();
    config.listener.auth_tokens = vec![InboundTokenConfig {
        id: "work".into(),
        name: "工作".into(),
        enabled: true,
        token: "synthetic-work-token".into(),
    }];
    let identity = config
        .listener
        .matched_inbound_token(&[("x-api-key".into(), "synthetic-work-token".into())]);
    let engine = Engine::new(
        config.clone(),
        None,
        Arc::new(crate::outbound::ReqwestTransport::new()),
    );
    engine.register_realtime_client_secret("ek_synthetic", None, None, None, None, identity);
    let headers = vec![("authorization".into(), "Bearer ek_synthetic".into())];
    assert!(engine.realtime_client_secret_authorized(&headers));
    assert_eq!(
        engine
            .realtime_client_secret_inbound_token(&headers)
            .unwrap()
            .id,
        "work"
    );
    config.listener.auth_tokens[0].name = "新名称".into();
    let _ = engine.replace_config(config.clone());
    assert_eq!(
        engine
            .realtime_client_secret_inbound_token(&headers)
            .unwrap()
            .name,
        "新名称"
    );
    config.listener.auth_tokens[0].enabled = false;
    let _ = engine.replace_config(config);
    assert!(!engine.realtime_client_secret_authorized(&headers));
    assert!(
        engine
            .realtime_client_secret_inbound_token(&headers)
            .is_none()
    );
}

#[test]
fn resource_bindings_survive_engine_restart_and_expired_entries_are_pruned() {
    let root = std::env::temp_dir().join(format!(
        "sumpter-engine-resource-bindings-{}",
        new_event_id()
    ));
    let dir = ConfigDir::new(root.clone());
    let first = Engine::new(
        AppConfig::bootstrap(),
        Some(dir.clone()),
        Arc::new(crate::outbound::ReqwestTransport::new()),
    );
    first.register_live_session("rtc_restart", "cpa", "gpt-live-1-codex");
    first.register_video_session("video_restart", "grok", "grok-imagine-video");
    assert!(dir.load_resource_bindings().unwrap().len() == 2);
    drop(first);

    let second = Engine::new(
        AppConfig::bootstrap(),
        Some(dir.clone()),
        Arc::new(crate::outbound::ReqwestTransport::new()),
    );
    assert_eq!(
        second.live_session_endpoint("/v1/live/rtc_restart"),
        Some(Some("cpa".into()))
    );
    assert_eq!(
        second.live_session_model("/v1/live/rtc_restart"),
        Some(Some("gpt-live-1-codex".into()))
    );
    assert_eq!(
        second.video_session_endpoint("/v1/videos/video_restart/content"),
        Some(Some("grok".into()))
    );
    drop(second);

    let mut expired = std::collections::HashMap::new();
    expired.insert(
        "live:expired".into(),
        ResourceBinding {
            endpoint_id: "cpa".into(),
            model: "gpt-live-1-codex".into(),
            expires_at: 1.0,
        },
    );
    let _ = dir.save_resource_bindings(&expired).unwrap();
    let third = Engine::new(
        AppConfig::bootstrap(),
        Some(dir.clone()),
        Arc::new(crate::outbound::ReqwestTransport::new()),
    );
    assert_eq!(third.live_session_endpoint("/v1/live/expired"), Some(None));
    assert!(dir.load_resource_bindings().unwrap().is_empty());
    drop(third);
    let _ = std::fs::remove_dir_all(root);
}
