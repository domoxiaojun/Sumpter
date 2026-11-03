use sumpter_core::bridge::{BridgeTerminal, OpenAiStreamBridge, ResponsesStreamBridge, SseBridge};
use sumpter_core::bridge_gemini::{GeminiClientBridge, GeminiStreamBridge};
use sumpter_core::bridge_in::{ChatClientBridge, ResponsesClientBridge};

#[test]
fn oversized_frames_fail_every_bridge_and_cannot_become_successful_later() {
    let oversized = format!("data: {}\n\n", "x".repeat(8 * 1024 * 1024 + 128));
    for stream in [false, true] {
        let bridges: Vec<Box<dyn SseBridge>> = vec![
            Box::new(OpenAiStreamBridge::new_with_stream(
                "id".into(),
                "model".into(),
                false,
                false,
                stream,
            )),
            Box::new(ResponsesStreamBridge::new_with_stream(
                "id".into(),
                "model".into(),
                false,
                stream,
            )),
            Box::new(GeminiStreamBridge::new("id".into(), "model".into(), stream)),
            Box::new(ChatClientBridge::new("id".into(), "model".into(), stream)),
            Box::new(ResponsesClientBridge::new(
                "id".into(),
                "model".into(),
                stream,
            )),
            Box::new(GeminiClientBridge::new("model".into(), stream)),
        ];
        for mut bridge in bridges {
            let mut output = Vec::new();
            for chunk in oversized.as_bytes().chunks(64 * 1024) {
                output.extend(bridge.feed(chunk));
            }
            output.extend(bridge.feed(
                b"data: [DONE]\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            ));
            output.extend(bridge.finish());
            assert!(matches!(bridge.terminal(), BridgeTerminal::Failed(_)));
            assert!(String::from_utf8_lossy(&output).contains("exceeds buffer limit"));
        }
    }
}

#[test]
fn a_large_network_chunk_of_small_frames_keeps_all_content() {
    let part = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n";
    let mut input = part.repeat(180_000);
    input.push_str("data: [DONE]\n\n");
    assert!(input.len() > 8 * 1024 * 1024);
    let mut bridge =
        OpenAiStreamBridge::new_with_stream("id".into(), "model".into(), false, false, false);
    let bytes = bridge.feed(input.as_bytes());
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["content"][0]["text"].as_str().unwrap().len(), 180_000);
    assert_eq!(bridge.terminal(), BridgeTerminal::Completed);
}
