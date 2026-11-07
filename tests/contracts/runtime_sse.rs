//! Both Admin adapters must publish TPS in the first completed SSE frame.

use std::convert::Infallible;

use crate::runtime_store::RuntimeEventListItem;
use axum::body::to_bytes;
use axum::response::{IntoResponse, Sse};
use serde_json::{Value, json};
use sumpter_core::events::RuntimeEvent;

async fn frame(name: &str, data: Value) -> (String, Value) {
    let event = super::admin_sse_event(name, data);
    let stream = futures_util::stream::iter([Ok::<_, Infallible>(event)]);
    let body = Sse::new(stream).into_response().into_body();
    let text = String::from_utf8(to_bytes(body, usize::MAX).await.unwrap().to_vec()).unwrap();
    let data = text
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("SSE data frame");
    let value = serde_json::from_str(data).unwrap();
    (text, value)
}

fn completed_event() -> Value {
    json!({
        "id": "first-request", "timestamp": 1.0, "kind": "client",
        "phase": "completed", "outcome": "succeeded", "statusCode": 200,
        "durationMS": 9100, "ttfbMS": 5100,
        "upstreamDurationMS": 2100, "upstreamTTFBMS": 100,
        "streamTrace": {"outputSpanMS": 2000, "usage": {"inputTokens": 1000, "outputTokens": 100}},
        "failover": false
    })
}

#[tokio::test]
async fn first_completed_sse_frame_has_the_same_tps_as_history() {
    let event = completed_event();
    let runtime_event: RuntimeEvent = serde_json::from_value(event.clone()).unwrap();
    let listed = RuntimeEventListItem::from_change(1, 2, runtime_event);
    let (text, value) = frame(
        "runtime-change",
        json!({"seq": 1, "changeSeq": 2, "event": event}),
    )
    .await;

    assert!(text.lines().any(|line| line == "event: runtime-change"));
    assert!(text.lines().any(|line| line == "id: 2"));
    assert_eq!(value["seq"], 1);
    assert_eq!(value["changeSeq"], 2);
    assert_eq!(value["event"]["outputTokensPerSecond"], 50.0);
    assert_eq!(
        value["event"]["outputTokensPerSecond"].as_f64(),
        listed.output_tokens_per_second
    );
    let mut expected = event;
    expected["outputTokensPerSecond"] = json!(50.0);
    assert_eq!(value["event"], expected);
}

#[tokio::test]
async fn sse_speed_preserves_eligibility_and_unrelated_notices() {
    for (key, replacement) in [
        ("phase", json!("inFlight")),
        ("outcome", json!("failed")),
        ("outcome", json!("cancelled")),
        ("kind", json!("upstream")),
        ("requestPurpose", json!("token_count")),
        ("streamTrace", Value::Null),
        ("streamTrace", json!({"usage":{"outputTokens":490}})),
        (
            "streamTrace",
            json!({"outputSpanMS":12,"usage":{"outputTokens":490}}),
        ),
    ] {
        let mut event = completed_event();
        event[key] = replacement;
        let payload = json!({"seq": 1, "changeSeq": 2, "event": event});
        let (_, value) = frame("runtime-change", payload.clone()).await;
        let mut expected = payload;
        expected["event"]["outputTokensPerSecond"] = Value::Null;
        assert_eq!(value, expected, "ineligible event: {key}");
    }

    let payload = json!({"generation": "unchanged"});
    let (text, value) = frame("config-reloaded", payload.clone()).await;
    assert_eq!(value, payload);
    assert!(!text.lines().any(|line| line.starts_with("id:")));
}
