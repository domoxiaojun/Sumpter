use serde_json::{Value, json};
use sumpter_core::client_attribution::{CLIENTS, ObservedRequestKind, observe};
use sumpter_core::events::{ClientKind, RuntimeEvent};

fn headers(values: &[(&str, &str)]) -> Vec<(String, String)> {
    values
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn gemini_paths_do_not_claim_an_openai_or_gemini_cli_product() {
    use sumpter_core::client_attribution::uses_openai_client_fallback;
    for path in [
        "/v1beta/models/gemini-test:generateContent",
        "/v1alpha/models/gemini-test:streamGenerateContent?alt=sse",
        "/v1/models/gemini-test:countTokens",
        "/v1beta/interactions",
        "/upload/v1beta/files",
        "/v1/messages",
        "/vendor/opaque",
    ] {
        assert!(!uses_openai_client_fallback(path), "{path}");
        for ua in [
            None,
            Some("google-genai-sdk/1.0"),
            Some("OpenAI/Python 1.0"),
        ] {
            let h = ua
                .map(|ua| headers(&[("user-agent", ua)]))
                .unwrap_or_default();
            assert_eq!(
                observe("POST", path, &h, None).client_kind,
                ClientKind::Unknown,
                "{path}/{ua:?}"
            );
        }
        assert_eq!(
            observe(
                "POST",
                path,
                &headers(&[("user-agent", "GeminiCLI/0.1.0")]),
                None
            )
            .client_kind,
            ClientKind::GeminiCli
        );
    }
    for path in [
        "/v1/chat/completions",
        "/chat/completions",
        "/v1/responses?x=1",
        "/responses/compact",
        "/backend-api/codex/responses",
        "/v1/models",
        "/v1/realtime/calls/id",
    ] {
        assert!(uses_openai_client_fallback(path), "{path}");
        assert_eq!(
            observe("POST", path, &[], None).client_kind,
            ClientKind::OpenaiCompat
        );
    }
}

#[test]
fn all_21_clients_have_stable_identity_positive_negative_and_declaration_contracts() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/client_attribution_v1.json"
    ))
    .unwrap();
    let clients = fixture["clients"].as_array().unwrap();
    assert_eq!(clients.len(), CLIENTS.len());
    for case in clients {
        let name = case["client"].as_str().unwrap();
        let ua = case["ua"].as_str().unwrap();
        for value in [ua.to_string(), ua.to_uppercase()] {
            let result = observe(
                "POST",
                "/v1/chat/completions",
                &headers(&[("User-Agent", &value)]),
                None,
            );
            assert_eq!(result.client_kind.as_str(), name, "{ua}");
            assert_eq!(result.client_variant, case["variant"].as_str().unwrap());
            assert_eq!(result.agent_role, "unknown");
        }
        let false_ua = format!("not{ua}");
        assert_eq!(
            observe(
                "POST",
                "/v1/messages",
                &headers(&[("user-agent", &false_ua)]),
                None
            )
            .client_kind,
            ClientKind::Unknown
        );
        let declared = observe(
            "POST",
            "/v1/responses",
            &headers(&[
                ("X-Sumpter-Client", &name.to_uppercase()),
                ("User-Agent", "OpenAI/Python 1.0"),
            ]),
            None,
        );
        assert_eq!(declared.client_kind.as_str(), name);
        assert_eq!(declared.client_variant, "unknown");
    }
}

#[test]
fn identity_does_not_guess_from_sdk_model_or_product_substrings() {
    for ua in [
        "OpenAI/Python 1.0",
        "Mozilla/5.0",
        "curl/8",
        "continue-proxy/1",
        "cline-unrelated/1",
        "opencode-proxy/1",
        "notcodex/1",
        "codex-other/1",
        "proxy/1 (grok-shell/1)",
    ] {
        assert_eq!(
            observe(
                "POST",
                "/v1/responses",
                &headers(&[("user-agent", ua)]),
                Some(&json!({"model":"codex-claude"}))
            )
            .client_kind,
            ClientKind::OpenaiCompat,
            "{ua}"
        );
    }
    let result = observe(
        "POST",
        "/v1/responses",
        &headers(&[
            ("X-Sumpter-Client", "cline"),
            ("x-sumpter-client", "cursor"),
        ]),
        None,
    );
    assert_eq!(result.client_kind, ClientKind::Unknown);
    assert_eq!(result.attribution.conflicts, ["clientKind"]);
    let result = observe(
        "POST",
        "/v1/responses",
        &headers(&[("x-sumpter-client", "pi"), ("originator", "codex_cli_rs")]),
        None,
    );
    assert_eq!(result.client_kind, ClientKind::Pi);
    assert_eq!(result.agent_role, "unknown");
    assert!(result.attribution.conflicts.contains(&"clientKind".into()));
}

#[test]
fn malformed_declarations_and_oversized_headers_are_ignored() {
    for value in ["bad\nvalue".to_string(), "x".repeat(4097), "made-up".into()] {
        let result = observe(
            "POST",
            "/v1/messages",
            &headers(&[
                ("x-sumpter-client", &value),
                ("x-sumpter-agent-role", &value),
                ("x-sumpter-request-kind", &value),
            ]),
            None,
        );
        assert_eq!(result.client_kind, ClientKind::Unknown);
        assert_eq!(result.request_kind, ObservedRequestKind::Unknown);
        assert_eq!(result.agent_role, "unknown");
    }
    let result = observe(
        "POST",
        "/v1/messages",
        &headers(&[
            ("x-sumpter-request-kind", "memory"),
            ("X-Sumpter-Request-Kind", "review"),
        ]),
        None,
    );
    assert_eq!(result.request_kind, ObservedRequestKind::Unknown);
    assert_eq!(result.attribution.conflicts, ["requestKind"]);
}

#[test]
fn duplicate_native_tasks_conflict_but_canonical_body_keeps_precedence() {
    let first = json!({"originator":"codex_cli_rs","request_kind":"turn","thread_source":"user"})
        .to_string();
    let second =
        json!({"originator":"codex_cli_rs","request_kind":"memory","thread_source":"subagent"})
            .to_string();
    let h = headers(&[
        ("x-codex-turn-metadata", &first),
        ("X-Codex-Turn-Metadata", &second),
    ]);
    let result = observe("POST", "/v1/responses", &h, None);
    assert_eq!(result.client_kind, ClientKind::Codex);
    assert_eq!(result.agent_role, "unknown");
    assert_eq!(result.request_kind, ObservedRequestKind::Unknown);
    assert!(result.attribution.conflicts.contains(&"requestKind".into()));
    let body = json!({"client_metadata":{"x-codex-turn-metadata":{"originator":"codex_cli_rs","request_kind":"compaction","thread_source":"user"}}});
    let result = observe("POST", "/v1/responses", &h, Some(&body));
    assert_eq!(result.request_kind, ObservedRequestKind::Compaction);
    assert_eq!(result.agent_role, "root");
}

#[test]
fn dedicated_operations_and_native_tasks_override_declarations_with_evidence() {
    for (path, expected) in [
        ("/v1/responses/compact", "compaction"),
        ("/v1/messages/count_tokens", "token_count"),
        ("/v1/images/generations", "image_generation"),
        ("/v1/images/edits", "image_edit"),
        ("/v1/alpha/search", "alpha_search"),
        ("/v1/realtime", "realtime"),
    ] {
        let result = observe(
            "POST",
            path,
            &headers(&[("x-sumpter-request-kind", "turn")]),
            None,
        );
        assert_eq!(result.request_kind.as_str(), expected);
        assert!(result.attribution.conflicts.contains(&"requestKind".into()));
    }
    let body = json!({"dsh_session_log":{"version":1,"session":{"origin":"subagent","parentSession":"parent"},"events":[{"data":{"secret":"not retained"}}]}});
    let result = observe(
        "POST",
        "/v1/messages",
        &headers(&[
            ("user-agent", "deepseek-harness/1"),
            ("x-deepseek-harness-compact", "1"),
        ]),
        Some(&body),
    );
    assert_eq!(result.request_kind, ObservedRequestKind::Compaction);
    assert_eq!(result.agent_role, "subagent");
    assert!(
        !serde_json::to_string(&result.attribution)
            .unwrap()
            .contains("secret")
    );
    let fork = json!({"dsh_session_log":{"version":1,"session":{"parentSession":"parent"}}});
    assert_eq!(
        observe(
            "POST",
            "/v1/messages",
            &headers(&[("user-agent", "deepseek-harness/1")]),
            Some(&fork)
        )
        .agent_role,
        "unknown"
    );
}

#[test]
fn entrypoint_variants_and_codex_internal_roles_stay_independent() {
    for (ua, expected) in [
        ("claude-cli/2 (external, claude-vscode)", "vscode"),
        ("claude-cli/2 (external, sdk-ts)", "sdk"),
        (
            "CloudCodeVSCode/1 (aidev_client; proxy_client=geminicli)",
            "vscode",
        ),
        ("grok-pager/1 grok-shell/1", "cli"),
        ("grok-desktop/1", "desktop"),
        ("Codex Desktop/0.156.1", "desktop"),
    ] {
        assert_eq!(
            observe(
                "POST",
                "/v1/responses",
                &headers(&[("user-agent", ua)]),
                None
            )
            .client_variant,
            expected
        );
    }
    for (source, expected) in [
        ("guardian_review", "guardian"),
        ("memory_consolidation", "memory"),
        ("subagent", "subagent"),
        ("user", "root"),
    ] {
        let meta =
            json!({"originator":"codex_cli_rs", "thread_source":source,"request_kind":"turn"})
                .to_string();
        let result = observe(
            "POST",
            "/v1/responses",
            &headers(&[("x-codex-turn-metadata", &meta)]),
            None,
        );
        assert_eq!(result.agent_role, expected);
        assert_eq!(result.request_kind, ObservedRequestKind::Turn);
    }
}

#[test]
fn compaction_fingerprints_only_read_current_direct_instructions_even_with_large_history() {
    for (client, system) in [
        (
            "pi",
            "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.",
        ),
        (
            "gemini_cli",
            "You are a specialized system component responsible for distilling chat history into a structured XML <state_snapshot>.",
        ),
    ] {
        let body =
            json!({"system":system,"messages":[{"role":"user","content":"history".repeat(20000)}]});
        let h = headers(&[("x-sumpter-client", client)]);
        assert_eq!(
            observe("POST", "/v1/messages", &h, Some(&body)).request_kind,
            ObservedRequestKind::Compaction
        );
        let quoted = json!({"system":format!("You are a coding assistant.\nHere is a quoted prompt:\n{system}"),"messages":[{"role":"user","content":"summarize this file"}]});
        assert_eq!(
            observe("POST", "/v1/messages", &h, Some(&quoted)).request_kind,
            ObservedRequestKind::Unknown
        );
        let tool_result = json!({"system":system,"messages":[{"role":"user","content":[{"type":"tool_result","content":system}]}]});
        assert_eq!(
            observe("POST", "/v1/messages", &h, Some(&tool_result)).request_kind,
            ObservedRequestKind::Unknown
        );
        assert_eq!(
            observe(
                "POST",
                "/v1/messages",
                &headers(&[("x-sumpter-client", "chatbox")]),
                Some(&body)
            )
            .request_kind,
            ObservedRequestKind::Unknown
        );
    }
}

#[test]
fn claude_auxiliary_fingerprints_require_identity_structure_and_current_position() {
    let h = headers(&[("user-agent", "claude-cli/2.1.220 (external, cli)")]);
    let cases = [
        (
            json!({"system":"Write the title in English. Keep technical terms and code identifiers in their original form.","messages":[{"role":"user","content":"<session>example</session>"}]}),
            "session_title",
        ),
        (
            json!({"system":"You are a security monitor for autonomous AI coding agents.","messages":[{"role":"user","content":"<transcript>\nexample\n</transcript>"}]}),
            "classifier",
        ),
        (
            json!({"system":"You are an assistant for performing a web search tool use","messages":[{"role":"user","content":"Perform a web search for the query: example"}],"tools":[{"type":"web_search_20250305","name":"web_search"}],"tool_choice":{"type":"tool","name":"web_search"}}),
            "websearch",
        ),
        (
            json!({"system":"You are Claude Code, Anthropic's official CLI for Claude.","messages":[{"role":"user","content":"Web page content:\n---\nexample\n---\nProvide a concise response based on the content above. Include relevant details, code examples, and documentation excerpts as needed."}]}),
            "webfetch",
        ),
    ];
    for (body, expected) in cases {
        assert_eq!(
            observe("POST", "/v1/messages", &h, Some(&body))
                .request_kind
                .as_str(),
            expected
        );
        assert_eq!(
            observe("POST", "/v1/messages", &[], Some(&body)).request_kind,
            ObservedRequestKind::Unknown
        );
        let mut history = body.clone();
        history["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"user","content":"please continue coding"}));
        assert_eq!(
            observe("POST", "/v1/messages", &h, Some(&history)).request_kind,
            ObservedRequestKind::Unknown
        );
    }
}

#[test]
fn new_observations_survive_normalization_without_reclassifying_legacy_events() {
    let mut event: RuntimeEvent = serde_json::from_value(json!({"id":"old","kind":"client","timestamp":1,"statusCode":200,"durationMS":1,"failover":false,"clientKind":"cursor","clientVariant":"cli"})).unwrap();
    event.refresh_cache_read();
    assert_eq!(event.client_variant.as_deref(), Some("cli"));
    assert!(event.request_kind.is_none());
    assert!(event.client_attribution.is_none());
    observe(
        "POST",
        "/v1/messages",
        &headers(&[
            ("x-sumpter-client", "cursor"),
            ("x-sumpter-agent-role", "subagent"),
        ]),
        None,
    )
    .apply(&mut event);
    event.refresh_cache_read();
    assert_eq!(event.client_variant.as_deref(), Some("unknown"));
    assert_eq!(event.agent_role.as_deref(), Some("subagent"));
    assert_eq!(
        serde_json::from_value::<RuntimeEvent>(serde_json::to_value(&event).unwrap()).unwrap(),
        event
    );
}
