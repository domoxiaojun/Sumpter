use serde_json::json;
use sumpter_core::events::{CodexMetadata, CodexThreadClass, codex_agent_role, codex_thread_class};
use sumpter_core::responses_observation::{MAX_OBSERVED_REQUESTS, ResponsesRequestObserver};

#[test]
fn internal_roles_and_safe_metadata_follow_current_codex_wire() {
    for (source, header, role) in [
        ("memory_consolidation", "memory_consolidation", "memory"),
        ("guardian_review", "guardian", "guardian"),
        ("subagent", "collab_spawn", "subagent"),
    ] {
        let canonical = json!({"request_kind":"turn","thread_source":source,"history_initialization":"cold_resume","analytics_enabled":false});
        let body = json!({"client_metadata": {
            "x-codex-turn-metadata":canonical.to_string(),
            "mcp_attribution": json!({"status":"attribution_error","error_reason":"payload_too_large","sources":[{"server_name":"PRIVATE","tool_name":"SECRET"}]}).to_string()
        }});
        let metadata = CodexMetadata::from_request(
            &[("x-openai-subagent".into(), header.into())],
            Some(&body),
        )
        .unwrap();
        assert_eq!(codex_agent_role(&metadata), role);
        assert_eq!(metadata.is_subagent, role == "subagent");
        assert_eq!(
            metadata.history_initialization.as_deref(),
            Some("cold_resume")
        );
        assert_eq!(metadata.analytics_enabled, Some(false));
        assert_eq!(metadata.mcp_attribution.as_ref().unwrap().source_count, 1);
        assert!(
            !serde_json::to_string(&metadata)
                .unwrap()
                .contains("PRIVATE")
        );
        assert!(!serde_json::to_string(&metadata).unwrap().contains("SECRET"));
    }
    let metadata =
        CodexMetadata::from_request(&[("x-openai-memgen-request".into(), "true".into())], None)
            .unwrap();
    assert_eq!(
        codex_thread_class(Some(&metadata)),
        CodexThreadClass::MemoryConsolidation
    );
    assert_eq!(codex_agent_role(&metadata), "memory");
    let invalid = json!({"client_metadata":{"x-codex-turn-metadata":json!({"analytics_enabled":"false"}).to_string(), "mcp_attribution":"x".repeat(17000)}});
    let metadata = CodexMetadata::from_request(&[], Some(&invalid)).unwrap();
    assert!(metadata.malformed && metadata.truncated);
    assert_eq!(metadata.analytics_enabled, None);
    assert_eq!(metadata.mcp_attribution, None);
}

#[test]
fn prewarm_turn_and_controls_keep_separate_identity_and_terminal() {
    let mut observer = ResponsesRequestObserver::default();
    for (i, kind) in ["prewarm", "turn", "compaction", "memory"]
        .iter()
        .enumerate()
    {
        let metadata = CodexMetadata {
            request_kind: Some((*kind).into()),
            turn_id: Some(format!("turn-{i}")),
            ..Default::default()
        };
        observer.client(
            &json!({"type":"response.create","generate":i!=0}),
            Some(metadata),
        );
        observer
            .upstream(&json!({"type":"response.created","response":{"id":format!("resp-{i}")}}));
        observer.client(&json!({"type":"response.interrupt"}), None);
        observer.upstream(&json!({"type":if i==1 {"response.interrupted"} else {"response.completed"},"response":{"id":format!("resp-{i}")}}));
    }
    let summary = observer.summary();
    assert_eq!(summary.counts.values().sum::<u64>(), 4);
    assert_eq!(summary.controls["response.interrupt"], 4);
    assert_eq!(
        summary.recent[1].terminal.as_deref(),
        Some("response.interrupted")
    );
    assert_eq!(
        observer.latest_metadata.unwrap().turn_id.as_deref(),
        Some("turn-3")
    );
}

#[test]
fn ambiguous_late_and_oversize_observations_never_guess_or_grow_unbounded() {
    let mut observer = ResponsesRequestObserver::default();
    observer.client(&json!({"type":"response.create"}), None);
    observer.upstream(&json!({"type":"response.completed","response":{"id":"late-unknown"}}));
    assert_eq!(observer.summary().recent[0].terminal, None);
    observer.client(&json!({"type":"response.create"}), None);
    observer.upstream(&json!({"type":"response.created","response":{"id":"ambiguous"}}));
    assert!(
        observer
            .summary()
            .recent
            .iter()
            .all(|r| r.response_id.is_none())
    );
    for _ in 0..100 {
        observer.client(&json!({"type":"response.create"}), None);
    }
    observer.unobserved();
    let summary = observer.summary();
    assert_eq!(summary.counts["unknown"], 102);
    assert_eq!(summary.recent.len(), MAX_OBSERVED_REQUESTS);
    assert_eq!(summary.dropped, 102 - MAX_OBSERVED_REQUESTS as u64);
    assert_eq!(summary.unobserved_frames, 1);
    assert_eq!(summary.unassociated_events, 2);
}

#[test]
fn idless_error_can_end_only_a_single_known_active_request() {
    let mut observer = ResponsesRequestObserver::default();
    observer.client(&json!({"type":"response.create"}), None);
    observer.upstream(&json!({"type":"response.created","response":{"id":"one"}}));
    observer.upstream(&json!({"type":"error"}));
    assert_eq!(
        observer.summary().recent[0].terminal.as_deref(),
        Some("error")
    );
}
