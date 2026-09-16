use super::{
    DiagnosticCaptureExportQuery, diagnostic_capture_export, diagnostic_test_state,
    redact_body_text, redact_capture_value, redact_url_query,
};
use serde_json::json;

#[tokio::test]
async fn every_export_scope_applies_privacy_and_keeps_raw_source() {
    use axum::extract::{Query, State};
    use sumpter_core::config_store::ConfigDir;
    use sumpter_core::events::DiagnosticCaptureSnapshot;

    let root = std::env::temp_dir().join(format!(
        "sumpter-redaction-contract-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let dir = ConfigDir::new(root.clone());
    let record = serde_json::from_value(json!({
        "requestID": "synthetic-request",
        "timestamp": 1.0,
        "method": "POST",
        "path": "/v1/messages?%6bey=synthetic-inbound&trace=keep",
        "inboundHeaders": [],
        "inboundBody": "{\"key\":\"ordinary-key\"}",
        "inboundBodyBytes": 22,
        "clientKind": "claude_code",
        "requestPurpose": "standard",
        "clientModel": "demo",
        "effectiveModel": "demo",
        "attempts": [{
            "id": "attempt-1", "endpointID": "endpoint-1", "endpointName": "synthetic",
            "protocol": "anthropic", "startedAtMS": 1, "outboundMethod": "POST",
            "outboundURL": "https://upstream.invalid/v1/messages?KEY=synthetic-outbound",
            "outboundHeaders": [], "outboundBody": "{}", "outboundBodyBytes": 2,
            "responseHeaders": [], "upstreamChunks": []
        }],
        "clientChunks": []
    }))
    .unwrap();
    let persisted = dir
        .save_diagnostic_capture(&DiagnosticCaptureSnapshot {
            records: vec![record],
            ..DiagnosticCaptureSnapshot::default()
        })
        .unwrap();
    assert!(persisted.durability_warning().is_none());
    let state = diagnostic_test_state(dir.clone());
    for scope in ["current", "selected", "all"] {
        for format in ["json", "jsonl"] {
            for privacy in ["redacted", "raw"] {
                let response = diagnostic_capture_export(
                    State(state.clone()),
                    Query(DiagnosticCaptureExportQuery {
                        scope: Some(scope.into()),
                        format: Some(format.into()),
                        privacy: Some(privacy.into()),
                        confirm_raw: privacy == "raw",
                        request_id: (scope == "selected").then(|| "synthetic-request".into()),
                    }),
                )
                .await;
                assert_eq!(response.status(), axum::http::StatusCode::OK);
                let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                let text = std::str::from_utf8(&bytes).unwrap();
                assert_eq!(
                    text.contains("synthetic-inbound"),
                    privacy == "raw",
                    "{scope}/{format}/{privacy}"
                );
                if scope != "current" {
                    assert_eq!(text.contains("synthetic-outbound"), privacy == "raw");
                    assert!(text.contains("ordinary-key"));
                }
                assert!(text.contains("trace=keep"));
            }
        }
    }
    drop(state);
    assert!(
        dir.load_diagnostic_capture().unwrap().records[0]
            .path
            .contains("synthetic-inbound")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn url_credentials_cover_gemini_case_and_percent_encoded_keys() {
    for key in [
        "key",
        "KEY",
        "%6bey",
        "k%65y",
        "%4b%45%59",
        "api_key",
        "API-KEY",
        "api%5Fkey",
        "%74oken",
    ] {
        let raw = format!(
            "/v1beta/models/demo:generateContent?{key}=synthetic-secret&trace=a%2Fb#section"
        );
        assert_eq!(
            redact_url_query(&raw),
            format!("/v1beta/models/demo:generateContent?{key}=[REDACTED]&trace=a%2Fb#section")
        );
    }
    assert_eq!(
        redact_url_query("/v1/messages?monkey=keep&trace=%bad"),
        "/v1/messages?monkey=keep&trace=%bad"
    );
}

#[test]
fn capture_detail_all_and_index_paths_are_redacted_without_mutating_source() {
    let record = json!({
        "requestID": "synthetic-request",
        "path": "/v1/messages?api_key=synthetic-inbound&trace=keep",
        "attempts": [{"outboundURL": "https://upstream.invalid/v1beta/models/demo:generateContent?%6bey=synthetic-outbound"}],
        "inboundBody": r#"{"key":"ordinary-key","messages":[{"content":"keyboard key=value"}]}"#
    });
    for mut exported in [record.clone(), json!({"records": [record.clone()]})] {
        redact_capture_value(&mut exported);
        let text = exported.to_string();
        assert!(!text.contains("synthetic-inbound"));
        assert!(!text.contains("synthetic-outbound"));
        assert!(text.contains("trace=keep"));
        assert!(text.contains("ordinary-key"));
    }
    let mut index =
        json!({"records": [{"requestID": "synthetic-request", "path": record["path"]}]});
    redact_capture_value(&mut index);
    assert_eq!(
        index["records"][0]["path"],
        "/v1/messages?api_key=[REDACTED]&trace=keep"
    );
    assert!(record.to_string().contains("synthetic-inbound"));
}

#[test]
fn gemini_query_key_rule_does_not_redact_ordinary_body_keys() {
    let body = r#"{"key":"ordinary-key","content":"key=value is an example"}"#;
    let decoded: serde_json::Value = serde_json::from_str(&redact_body_text(body)).unwrap();
    assert_eq!(
        decoded,
        serde_json::from_str::<serde_json::Value>(body).unwrap()
    );
}
