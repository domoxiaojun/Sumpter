use sumpter_core::events::{ClientDeclaredMetadata, ClientKind};

#[test]
fn pi_client_identity_is_independent_of_protocol() {
    for ua in [
        "pi (darwin 25.0; arm64)",
        "pi (browser)",
        "pi/0.0.3 (darwin; node/v26; arm64)",
        "pi-coding-agent",
    ] {
        for openai in [true, false] {
            assert_eq!(ClientKind::detect(Some(ua), openai), ClientKind::Pi);
        }
    }
    assert_eq!(
        ClientKind::detect_with_originator(Some("claude-cli/1"), Some("pi"), false),
        ClientKind::Pi
    );
    for ua in [
        "api/client",
        "spice/1",
        "pi/",
        "pi/other",
        "pi-coding-agent-other",
        "raspberrypi (linux)",
    ] {
        assert_eq!(ClientKind::detect(Some(ua), true), ClientKind::OpenaiCompat);
    }
    assert_eq!(serde_json::to_string(&ClientKind::Pi).unwrap(), "\"pi\"");
    assert_eq!(
        serde_json::from_str::<ClientKind>("\"pi\"").unwrap(),
        ClientKind::Pi
    );
}

#[test]
fn pi_unicode_headers_decode_only_when_marked_and_reject_malformed_values() {
    let headers = vec![
        ("x-sumpter-attribution-encoding".into(), "uri-v1".into()),
        (
            "x-sumpter-project".into(),
            "%E4%B8%AD%E6%96%87%20project".into(),
        ),
        (
            "x-sumpter-workspace".into(),
            "%2Fwork%2F%E4%B8%AD%E6%96%87%20project".into(),
        ),
    ];
    let metadata = ClientDeclaredMetadata::from_headers(&headers).unwrap();
    assert_eq!(metadata.project.as_deref(), Some("中文 project"));
    assert_eq!(
        metadata.source_workspace.as_deref(),
        Some("/work/中文 project")
    );
    let literal = ClientDeclaredMetadata::from_headers(&[(
        "x-sumpter-project".into(),
        "project%20name".into(),
    )])
    .unwrap();
    assert_eq!(literal.project.as_deref(), Some("project%20name"));
    for bad in ["%0Asecret", "%FF", "%G0", "%", " "] {
        assert!(
            ClientDeclaredMetadata::from_headers(&[
                headers[0].clone(),
                ("x-sumpter-project".into(), bad.into())
            ])
            .is_none()
        );
    }
}
