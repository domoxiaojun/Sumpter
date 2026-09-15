use serde_json::{Value, json};
use sumpter_core::{bridge, bridge_gemini, config::ProviderProtocol, routing::RoutingRequest};

fn request() -> RoutingRequest {
    RoutingRequest::from_value(&json!({
        "model": "gpt-6-astra",
        "system": [{"type":"text", "text":"top-level"}],
        "messages": [
            {"role":"user", "content":"question"},
            {"role":"system", "content":"late-system"},
            {"role":"developer", "content":[{"type":"text", "text":"developer-a"}, {"type":"text", "text":"developer-b"}]},
            {"role":"assistant", "content":"answer"},
            {"role":"system", "content":"second-system"},
            {"role":"user", "content":"continue"}
        ]
    })).unwrap()
}

#[test]
fn instruction_messages_keep_roles_positions_and_all_text_for_openai() {
    let request = request();
    let original = request.clone();
    let chat = bridge::try_make_openai_chat_body(&request, "up", None, false).unwrap();
    let responses = bridge::try_make_responses_body(&request, "up", None, false).unwrap();
    let roles = |items: &Value| {
        items
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["role"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        roles(&chat["messages"]),
        [
            "system",
            "user",
            "system",
            "developer",
            "assistant",
            "system",
            "user"
        ]
    );
    assert_eq!(
        roles(&responses["input"]),
        ["user", "system", "developer", "assistant", "system", "user"]
    );
    assert_eq!(chat["messages"][0]["content"], "top-level");
    assert_eq!(responses["instructions"], "top-level");
    for (index, text) in [
        (1, "late-system"),
        (2, "developer-a\ndeveloper-b"),
        (4, "second-system"),
    ] {
        assert_eq!(chat["messages"][index + 1]["content"], text);
        assert_eq!(
            responses["input"][index]["content"][0],
            json!({"type":"input_text", "text":text})
        );
    }
    assert_eq!(
        request, original,
        "bridges must not mutate routing or raw input"
    );
}

#[test]
fn gemini_collects_all_instruction_sources_and_preserves_conversation() {
    let body = bridge_gemini::try_make_gemini_body(&request(), None, None).unwrap();
    assert_eq!(
        body["systemInstruction"]["parts"][0]["text"],
        "top-level\n\nlate-system\n\ndeveloper-a\n\ndeveloper-b\n\nsecond-system"
    );
    assert_eq!(body["contents"].as_array().unwrap().len(), 3);
    assert_eq!(body["contents"][1]["role"], "model");
}

#[test]
fn instruction_validation_never_discards_unsupported_content() {
    for content in [
        json!([{"type":"image", "text":"must-not-be-flattened"}]),
        json!([{"type":"text", "text":"x", "citations":[{"id":"ref"}]}]),
        json!([{"type":"text", "text":12}]),
        json!(["not-a-block"]),
    ] {
        for target in [
            ProviderProtocol::OpenAI,
            ProviderProtocol::OpenAIResponses,
            ProviderProtocol::Gemini,
        ] {
            for role in ["system", "developer"] {
                let mut request = request();
                request.messages[1].role = role.into();
                request.messages[1].content = content.clone();
                assert!(
                    bridge::check_anthropic_translation(&request, target, false).is_err(),
                    "{target:?}: {content}"
                );
            }
            let mut request = request();
            request.system = Some(content.clone());
            assert!(
                bridge::check_anthropic_translation(&request, target, false).is_err(),
                "top-level {target:?}: {content}"
            );
        }
    }
}

#[test]
fn unknown_message_roles_are_still_rejected() {
    let mut request = request();
    request.messages[1].role = "unknown".into();
    for target in [
        ProviderProtocol::OpenAI,
        ProviderProtocol::OpenAIResponses,
        ProviderProtocol::Gemini,
    ] {
        let error = bridge::check_anthropic_translation(&request, target, false).unwrap_err();
        assert!(error.to_string().contains("messages[1].role=unknown"));
    }
}
