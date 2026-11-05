use sumpter_core::config::{InboundTokenConfig, ListenerConfig};

fn listener() -> ListenerConfig {
    ListenerConfig {
        auth_token: "old-short-token".into(),
        auth_tokens: vec![InboundTokenConfig {
            id: "work".into(),
            name: "工作电脑".into(),
            enabled: true,
            token: "synthetic-work-token".into(),
        }],
        ..Default::default()
    }
}

#[test]
fn default_inbound_token_can_be_disabled_without_erasing_it_or_opening_access() {
    let mut config = listener();
    let headers = vec![("authorization".into(), "Bearer old-short-token".into())];
    assert_eq!(config.matched_inbound_token(&headers).unwrap().id, "legacy");
    config.auth_token_enabled = false;
    config.auth_tokens.clear();
    assert!(config.has_inbound_auth());
    assert!(config.matched_inbound_token(&headers).is_none());
    assert_eq!(config.auth_token, "old-short-token");
    config.auth_token_enabled = true;
    assert_eq!(config.matched_inbound_token(&headers).unwrap().id, "legacy");
}

#[test]
fn inbound_tokens_match_legacy_and_named_headers_and_reject_conflicts() {
    let mut config = listener();
    config.validate_inbound_tokens(true).unwrap();
    for header in ["Authorization", "X-API-Key", "x-goog-api-key"] {
        let value = if header == "Authorization" {
            "bEaReR synthetic-work-token"
        } else {
            "synthetic-work-token"
        };
        let matched = config
            .matched_inbound_token(&[(header.into(), value.into())])
            .unwrap();
        assert_eq!(matched.id, "work");
        assert_eq!(matched.name, "工作电脑");
    }
    let headers = vec![("authorization".into(), "Bearer old-short-token".into())];
    assert_eq!(config.matched_inbound_token(&headers).unwrap().id, "legacy");
    let mut conflict = headers;
    conflict.push(("x-api-key".into(), "synthetic-work-token".into()));
    assert!(config.matched_inbound_token(&conflict).is_none());
    config.auth_token.clear();
    config.auth_tokens[0].enabled = false;
    assert!(config.has_inbound_auth());
    assert!(config.matched_inbound_token(&conflict).is_none());
    assert!(config.matched_inbound_token(&[]).is_none());
}

#[test]
fn inbound_tokens_validate_identity_secrets_and_redacted_config() {
    let mut config = listener();
    config.auth_tokens[0].token.clear();
    assert!(config.validate_inbound_tokens(false).is_ok());
    assert!(config.validate_inbound_tokens(true).is_err());
    for id in ["", "legacy", "unrecorded", "with space"] {
        let mut config = listener();
        config.auth_tokens[0].id = id.into();
        assert!(config.validate_inbound_tokens(true).is_err());
    }
    let mut config = listener();
    config.auth_token = config.auth_tokens[0].token.clone();
    assert!(config.validate_inbound_tokens(true).is_err());
    config.auth_token.clear();
    config.auth_tokens.push(config.auth_tokens[0].clone());
    assert!(config.validate_inbound_tokens(true).is_err());
    let legacy: ListenerConfig = serde_json::from_str(r#"{"authToken":"legacy"}"#).unwrap();
    assert!(legacy.auth_tokens.is_empty());
    assert_eq!(legacy.auth_token_name, "默认 Token");
}
