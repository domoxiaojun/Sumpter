use serde_json::json;
use sumpter_core::client_attribution::{ObservedRequestKind, observe_configured};
use sumpter_core::client_identity::{ClientBinding, ClientIdentitySettings};
use sumpter_core::config::{InboundTokenConfig, InboundTokenIdentity, ListenerConfig};
use sumpter_core::events::{ClientKind, CodexMetadata, codex_request_kind};
use sumpter_core::responses_observation::ResponsesRequestObserver;

fn headers(values: &[(&str, &str)]) -> Vec<(String, String)> {
    values
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect()
}
fn settings(client: &str) -> ClientIdentitySettings {
    let mut settings = ClientIdentitySettings::default();
    settings.token_bindings.insert(
        "work".into(),
        ClientBinding {
            client: client.into(),
            ..Default::default()
        },
    );
    settings
}
fn token() -> InboundTokenIdentity {
    InboundTokenIdentity {
        id: "work".into(),
        name: "任意名称".into(),
    }
}

#[test]
fn binding_requires_an_enabled_authenticated_token_and_preserves_raw_sdk() {
    let mut listener = ListenerConfig::default();
    listener.auth_tokens.push(InboundTokenConfig {
        id: "work".into(),
        name: "Codex label is not evidence".into(),
        enabled: true,
        token: "synthetic-test-key".into(),
    });
    for auth in ["", "Bearer wrong", "Bearer codex"] {
        let h = headers(&[("authorization", auth), ("user-agent", "OpenAI/Python 1.0")]);
        let matched = listener.matched_inbound_token(&h);
        assert!(matched.is_none());
        assert_eq!(
            observe_configured(
                "POST",
                "/v1/responses",
                &h,
                None,
                &settings("codex"),
                matched.as_ref()
            )
            .client_kind,
            ClientKind::OpenaiCompat
        );
    }
    let h = headers(&[
        ("authorization", "Bearer synthetic-test-key"),
        ("user-agent", "OpenAI/Python 1.0"),
    ]);
    let matched = listener.matched_inbound_token(&h).unwrap();
    let observation = observe_configured(
        "POST",
        "/v1/responses",
        &h,
        None,
        &settings("codex"),
        Some(&matched),
    );
    assert_eq!(observation.client_kind, ClientKind::Codex);
    assert_eq!(observation.agent_role, "unknown");
    let identity = observation.attribution.identity.unwrap();
    assert_eq!(identity.product.as_deref(), Some("codex"));
    assert_eq!(identity.raw_product.as_deref(), Some("openai"));
    assert!(identity.session_id.is_none());
    listener.auth_tokens[0].enabled = false;
    assert!(listener.matched_inbound_token(&h).is_none());
}

#[test]
fn native_identity_and_ambiguous_identity_cannot_be_overridden() {
    let s = settings("codex");
    let cursor = observe_configured(
        "POST",
        "/v1/responses",
        &headers(&[("user-agent", "Cursor/1")]),
        None,
        &s,
        Some(&token()),
    );
    assert_eq!(cursor.client_kind, ClientKind::Cursor);
    assert!(cursor.attribution.conflicts.contains(&"clientKind".into()));
    let conflict = observe_configured(
        "POST",
        "/v1/responses",
        &headers(&[("x-sumpter-client", "pi"), ("x-sumpter-client", "codex")]),
        None,
        &s,
        Some(&token()),
    );
    assert_eq!(conflict.client_kind, ClientKind::Unknown);
    assert!(
        conflict
            .attribution
            .conflicts
            .contains(&"clientKind".into())
    );
}

#[test]
fn aliases_are_exact_and_host_engine_and_subagents_remain_independent() {
    let mut s = ClientIdentitySettings::default();
    s.ua_aliases.insert("custom-cli".into(), "open_code".into());
    let exact = observe_configured(
        "POST",
        "/v1/responses",
        &headers(&[("user-agent", "custom-cli/1")]),
        None,
        &s,
        None,
    );
    assert_eq!(exact.client_kind, ClientKind::OpenCode);
    let negative = observe_configured(
        "POST",
        "/v1/responses",
        &headers(&[("user-agent", "custom-cli-extra/1")]),
        None,
        &s,
        None,
    );
    assert_eq!(negative.client_kind, ClientKind::OpenaiCompat);
    let hosted = observe_configured(
        "POST",
        "/v1/responses",
        &headers(&[("user-agent", "vscode_agent_host/1")]),
        None,
        &s,
        None,
    );
    assert_eq!(hosted.client_kind, ClientKind::Codex);
    let identity = hosted.attribution.identity.unwrap();
    assert_eq!(identity.host.as_deref(), Some("vscode"));
    assert_eq!(identity.engine.as_deref(), Some("codex"));
    let claude = observe_configured(
        "POST",
        "/v1/messages",
        &headers(&[
            ("user-agent", "claude-cli/1"),
            ("x-claude-code-agent-id", "child"),
            ("x-claude-code-parent-agent-id", "parent"),
        ]),
        None,
        &s,
        None,
    );
    assert_eq!(claude.agent_role, "subagent");
    let identity = claude.attribution.identity.unwrap();
    assert_eq!(identity.agent_id.as_deref(), Some("child"));
    assert_eq!(identity.parent_agent_id.as_deref(), Some("parent"));
    assert_eq!(
        observe_configured(
            "POST",
            "/v1/messages",
            &headers(&[("user-agent", "claude-cli/1")]),
            None,
            &s,
            None
        )
        .agent_role,
        "unknown"
    );
}

#[test]
fn authenticated_relay_is_source_scoped_bounded_and_preserves_native_precedence() {
    let mut s = settings("sumpter");
    s.token_bindings.get_mut("work").unwrap().relay_source = Some("office".into());
    let h = headers(&[
        ("x-sumpter-relay-source", "office"),
        ("x-sumpter-forwarded-client", "codex"),
        ("x-sumpter-forwarded-session", "s1"),
    ]);
    let spoof = observe_configured("POST", "/v1/responses", &h, None, &s, None);
    assert_eq!(spoof.client_kind, ClientKind::OpenaiCompat);
    assert!(spoof.attribution.issues.contains(&"untrustedRelay".into()));
    let trusted = observe_configured("POST", "/v1/responses", &h, None, &s, Some(&token()));
    assert_eq!(trusted.client_kind, ClientKind::Codex);
    assert_eq!(
        trusted.attribution.identity.unwrap().session_id.as_deref(),
        Some("relay:office:s1")
    );
    let mut wrong = h.clone();
    wrong[0].1 = "elsewhere".into();
    assert!(
        observe_configured("POST", "/v1/responses", &wrong, None, &s, Some(&token()))
            .attribution
            .issues
            .contains(&"relaySourceMismatch".into())
    );
    let mut repeated = h.clone();
    repeated.push(h[1].clone());
    assert_ne!(
        observe_configured("POST", "/v1/responses", &repeated, None, &s, Some(&token()))
            .client_kind,
        ClientKind::Codex
    );
    let mut native = h.clone();
    native.push(("user-agent".into(), "Cursor/1".into()));
    assert_eq!(
        observe_configured("POST", "/v1/responses", &native, None, &s, Some(&token())).client_kind,
        ClientKind::Cursor
    );
    s.token_bindings.get_mut("work").unwrap().relay_source = Some("home".into());
    let mut other = h;
    other[0].1 = "home".into();
    assert_eq!(
        observe_configured("POST", "/v1/responses", &other, None, &s, Some(&token()))
            .attribution
            .identity
            .unwrap()
            .session_id
            .as_deref(),
        Some("relay:home:s1")
    );
}

#[test]
fn native_task_aliases_apply_per_create_without_sticking_to_the_socket() {
    let mut observer = ResponsesRequestObserver::default();
    for (alias, expected) in [
        ("thread_title", "session_title"),
        ("memgen", "memory"),
        ("guardian_review", "review"),
    ] {
        let h = headers(&[
            ("originator", "codex_cli_rs"),
            (
                "x-codex-turn-metadata",
                &json!({"request_kind":alias}).to_string(),
            ),
        ]);
        let metadata = CodexMetadata::from_request(&h, None);
        assert_eq!(codex_request_kind(metadata.as_ref()), expected);
        let observed = observe_configured(
            "POST",
            "/v1/responses",
            &h,
            None,
            &ClientIdentitySettings::default(),
            None,
        );
        assert_eq!(observed.request_kind.as_str(), expected);
        observer.client(&json!({"type":"response.create","generate":true}), metadata);
    }
    observer.client(&json!({"type":"response.create","generate":true}), None);
    assert_eq!(observer.summary().recent.last().unwrap().kind, "unknown");
    let h = headers(&[
        ("originator", "codex_cli_rs"),
        (
            "x-codex-turn-metadata",
            r#"{"request_kind":"turn","thread_source":"memgen"}"#,
        ),
    ]);
    assert_eq!(
        observe_configured(
            "POST",
            "/v1/responses",
            &h,
            None,
            &ClientIdentitySettings::default(),
            None
        )
        .request_kind,
        ObservedRequestKind::Turn
    );
}

#[test]
fn configuration_is_optional_and_validates_limits_without_secret_parsing() {
    let listener = ListenerConfig::default();
    assert!(
        serde_json::to_value(&listener)
            .unwrap()
            .get("clientIdentity")
            .is_none()
    );
    let mut s = settings("my-agent");
    let b = s.token_bindings.remove("work").unwrap();
    s.token_bindings
        .insert(format!("token-{}", "a".repeat(64)), b);
    assert!(s.validate().is_ok());
    s.ua_aliases.insert("openai".into(), "invented-kind".into());
    assert!(s.validate().is_err());
    assert!(sumpter_core::client_registry::product(&"a".repeat(65)).is_none());
    assert!(sumpter_core::client_registry::product("/etc/private").is_none());
}
