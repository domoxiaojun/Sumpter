//! 引擎行为测试:FakeTransport 脚本化上游,对照 docs/architecture.md §8 的核心条目。

#[path = "../../../../tests/contracts/gemini.rs"]
mod gemini;

#[path = "../../../../tests/contracts/pi.rs"]
mod pi;

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};
use sumpter_core::config::*;
use sumpter_core::events::{
    ClientKind, RuntimeEventOutcome, RuntimeEventPhase, RuntimeFailureKind, RuntimeFailurePhase,
    RuntimeSnapshot,
};
use sumpter_engine::PlatformNotice;
use sumpter_macos_adapter::engine::Engine;
use sumpter_macos_adapter::outbound::{
    OutboundRequest, TransportError, UpstreamResponse, UpstreamTransport,
};
use tokio::io::AsyncWriteExt;

// ---------------------------------------------------------------------------
// FakeTransport
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Outcome {
    /// 固定响应:状态 + 头 + 分块 body。
    Status {
        status: u16,
        headers: Vec<(String, String)>,
        chunks: Vec<Vec<u8>>,
    },
    /// 传输错误。
    Error(String),
    /// 确定性请求/响应构造错误，不得单独触发跨轮。
    Invalid(String),
    /// 首响应截止超时。
    Timeout,
    /// 长流:状态 200,body 由测试端经 channel 灌入。
    Gated { status: u16 },
    /// 悬挂在响应头阶段，用于验证响应超时与请求取消。
    Hang,
}

#[derive(Clone)]
struct Recorded {
    host: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

type Script = HashMap<String, VecDeque<Outcome>>;

struct FakeTransport {
    /// host → 结局队列。
    script: Mutex<Script>,
    requests: Mutex<Vec<Recorded>>,
    gate_txs: Mutex<Vec<tokio::sync::mpsc::UnboundedSender<Result<Bytes, TransportError>>>>,
    /// 每次出站的人为延迟(deferred 轮测试用来控制节奏)。
    delay: Mutex<Option<Duration>>,
}

impl FakeTransport {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(HashMap::new()),
            requests: Mutex::new(Vec::new()),
            gate_txs: Mutex::new(Vec::new()),
            delay: Mutex::new(None),
        })
    }

    fn set_delay(&self, delay: Duration) {
        *self.delay.lock().unwrap() = Some(delay);
    }

    fn push(&self, host: &str, outcome: Outcome) {
        self.script
            .lock()
            .unwrap()
            .entry(host.to_string())
            .or_default()
            .push_back(outcome);
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    /// 取走 gated 流的发送端(**移出**队列:调用方成为唯一持有者,
    /// drop 它即代表上游正常收尾)。
    fn gate_sender(&self) -> tokio::sync::mpsc::UnboundedSender<Result<Bytes, TransportError>> {
        self.gate_txs
            .lock()
            .unwrap()
            .pop()
            .expect("gated stream requested")
    }
}

#[async_trait::async_trait]
impl UpstreamTransport for FakeTransport {
    async fn send_streaming(
        &self,
        request: OutboundRequest,
        _response_timeout: Option<Duration>,
    ) -> Result<UpstreamResponse, TransportError> {
        let delay = *self.delay.lock().unwrap();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let host = reqwest::Url::parse(&request.base_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default();
        self.requests.lock().unwrap().push(Recorded {
            host: host.clone(),
            path: request.path_and_query.clone(),
            headers: request.headers.clone(),
            body: request.body.clone(),
        });
        let outcome = {
            let mut script = self.script.lock().unwrap();
            script
                .get_mut(&host)
                .and_then(|q| q.pop_front())
                .unwrap_or(Outcome::Error("no scripted outcome".into()))
        };
        match outcome {
            Outcome::Status {
                status,
                headers,
                chunks,
            } => Ok(UpstreamResponse {
                status,
                headers,
                stream: futures_util::stream::iter(
                    chunks
                        .into_iter()
                        .map(|c| Ok(Bytes::from(c)))
                        .collect::<Vec<_>>(),
                )
                .boxed(),
            }),
            Outcome::Error(message) => Err(TransportError::ConnectionFailed(message)),
            Outcome::Invalid(message) => Err(TransportError::InvalidResponse(message)),
            Outcome::Timeout => Err(TransportError::Timeout),
            Outcome::Gated { status } => {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                self.gate_txs.lock().unwrap().push(tx);
                let stream = futures_util::stream::unfold(rx, |mut rx| async move {
                    rx.recv().await.map(|item| (item, rx))
                });
                Ok(UpstreamResponse {
                    status,
                    headers: vec![],
                    stream: stream.boxed(),
                })
            }
            Outcome::Hang => {
                // 挂在响应头阶段;被掐断即随 future drop 消失。
                futures_util::future::pending::<()>().await;
                unreachable!()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 基建
// ---------------------------------------------------------------------------

fn endpoint(id: &str, host: &str, key: &str) -> Endpoint {
    Endpoint {
        api_key: key.into(),
        base_url: format!("https://{host}"),
        catalog: None,
        enabled: true,
        id: id.into(),
        keep_alive: false,
        mappings: vec![],
        name: id.into(),
        priority: 0,
        protocol: EndpointProtocolMode::Anthropic,
        sticky_group: None,
    }
}

fn two_endpoint_config() -> AppConfig {
    let mut a = endpoint("a", "a.example.com", "sk-a");
    let mut b = endpoint("b", "b.example.com", "sk-b");
    // Most legacy engine tests assert the historical single-account ordering.
    // Keep that intent explicit; empty-group distribution is covered by
    // dedicated fixtures below.
    a.sticky_group = Some("default".into());
    b.sticky_group = Some("default".into());
    for endpoint in [&mut a, &mut b] {
        endpoint.mappings = vec![ModelMapping {
            client_pattern: "claude-*".into(),
            context: ContextMode::OneMillion,
            failover_timeout_seconds: None,
            thinking: ThinkingMode::Adaptive,
            effort: None,
            upstream_model: String::new(),
            capabilities: Vec::new(),
        }];
    }
    let retry = RetryPolicy {
        max_deferred_rounds: 1,
        session_sticky_retries: 0,
        ..RetryPolicy::default()
    };
    // 绝大多数用例只验证单轮结果；跨轮语义由专门用例显式开启，避免失败剧本
    // 产品默认时长闸为 0（无限）；本用例用 max_deferred_rounds 显式限制轮数。
    AppConfig {
        feature_rules: vec![],
        listener: ListenerConfig::default(),
        endpoints: vec![a, b],
        retry,
        session_sticky_ttl_hours: sumpter_core::config::DEFAULT_SESSION_STICKY_TTL_HOURS,
        schema_version: 6,
        model_groups: None,
    }
    .normalized()
}

fn set_all_endpoint_protocols(config: &mut AppConfig, protocol: EndpointProtocolMode) {
    for endpoint in config.endpoints.iter_mut() {
        endpoint.protocol = protocol;
    }
}

fn engine_with(config: AppConfig, fake: Arc<FakeTransport>) -> Engine {
    Engine::new(config, None, fake, "test-token".into())
}

fn temp_config_dir(tag: &str) -> sumpter_core::config_store::ConfigDir {
    let root = std::env::temp_dir().join(format!(
        "sumpter-engine-{tag}-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let _ = std::fs::remove_dir_all(&root);
    sumpter_core::config_store::ConfigDir::new(root)
}

fn body() -> Bytes {
    body_for("claude-opus-5", "hello")
}

fn body_for(model: &str, user: &str) -> Bytes {
    Bytes::from(
        serde_json::to_vec(&json!({
            "model": model,
            "system": "You are Claude Code, Anthropic's official CLI for Claude.",
            "messages": [{"role": "user", "content": user}],
            "stream": true,
        }))
        .unwrap(),
    )
}

fn stable_session(label: &str) -> String {
    format!("engine-stable-{label}")
}

fn session_header(session_id: &str) -> Vec<(String, String)> {
    vec![("x-claude-code-session-id".into(), session_id.into())]
}

fn engine_with_dir(
    config: AppConfig,
    dir: sumpter_core::config_store::ConfigDir,
    fake: Arc<FakeTransport>,
) -> Engine {
    Engine::new(config, Some(dir), fake, "test-token".into())
}

#[test]
fn explicit_capture_clear_recovers_from_corrupt_snapshot() {
    let dir = temp_config_dir("capture-corrupt-clear");
    dir.ensure_exists().unwrap();
    std::fs::write(dir.root.join("diagnostic_capture.json"), b"{broken").unwrap();
    let engine = Engine::new(
        two_endpoint_config(),
        Some(dir.clone()),
        FakeTransport::new(),
        "test-token".into(),
    );
    assert!(engine.diagnostic_capture_export_file().is_err());

    engine.clear_diagnostic_capture().unwrap();

    let snapshot = dir.load_diagnostic_capture().expect("DELETE 应重建空快照");
    assert!(snapshot.records.is_empty());
    assert!(!snapshot.enabled);
    assert!(
        engine
            .diagnostic_capture_index()
            .get("records")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    );
    let _ = std::fs::remove_dir_all(dir.root);
}

#[cfg(unix)]
#[test]
fn capture_export_rejects_symbolic_link() {
    use std::os::unix::fs::symlink;

    let dir = temp_config_dir("capture-export-symlink");
    dir.ensure_exists().unwrap();
    let target = dir.root.join("unrelated.json");
    std::fs::write(
        &target,
        serde_json::to_vec(&sumpter_core::events::DiagnosticCaptureSnapshot::default()).unwrap(),
    )
    .unwrap();
    symlink(&target, dir.root.join("diagnostic_capture.json")).unwrap();
    let engine = Engine::new(
        two_endpoint_config(),
        Some(dir.clone()),
        FakeTransport::new(),
        "test-token".into(),
    );

    assert!(engine.diagnostic_capture_export_file().is_err());

    let _ = std::fs::remove_dir_all(dir.root);
}

fn loopback() -> Option<IpAddr> {
    Some(IpAddr::from([127, 0, 0, 1]))
}

async fn call(
    engine: &Engine,
    remote: Option<IpAddr>,
    path: &str,
    headers: Vec<(String, String)>,
    body: Bytes,
) -> (u16, Vec<u8>) {
    let response = engine
        .handle_request(remote, "POST", path, headers, body)
        .await;
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    (status, bytes.to_vec())
}

async fn runtime_of(engine: &Engine) -> sumpter_core::events::RuntimeSnapshot {
    engine.runtime_snapshot()
}

fn body_with_poll_flag(polled: Arc<AtomicBool>) -> Body {
    let stream = futures_util::stream::once(async move {
        polled.store(true, Ordering::SeqCst);
        Ok::<Bytes, std::io::Error>(body())
    });
    Body::from_stream(stream)
}

fn sse_ok(chunks: &[&str]) -> Outcome {
    Outcome::Status {
        status: 200,
        headers: vec![("content-type".into(), "text/event-stream".into())],
        chunks: chunks.iter().map(|c| c.as_bytes().to_vec()).collect(),
    }
}

fn codex_responses_body() -> Bytes {
    Bytes::from(
        serde_json::to_vec(&json!({
            "model": "claude-opus-5",
            "input": "ls",
            "stream": true,
        }))
        .unwrap(),
    )
}

/// 事件消息词表前缀清单(docs/architecture.md §5.1)。
/// 与 Swift 侧 `RuntimeEventPresentationTests` 的清单互钉:两边不同步会各自红。
const MESSAGE_TOKEN_PREFIXES: &[&str] = &[
    "bridge ",
    "passthrough ",
    "deferred_rounds ",
    "unmatched_no_tools",
    "timeout",
    "connection failed: ",
    "invalid response: ",
    "stream interrupted: ",
    "client_disconnected",
    "upstream_retryable_status",
    "all endpoints failed",
    "inbound_auth_required",
    "openai_tools_unsupported",
    "body is not JSON",
    "anthropic request shape invalid",
    "inbound_convert_failed: ",
    "body is not an object",
    "no Provider accepts model ",
    "pool not found: ",
    "no enabled endpoint in pool ",
    "feature rule not found: ",
    "no compatible protocol endpoint in pool ",
];

/// 断言快照里所有引擎产出的消息段都在词表内(notify 是人话,豁免)。
fn assert_messages_in_vocabulary(runtime: &sumpter_core::events::RuntimeSnapshot) {
    for event in &runtime.recent_events {
        if event.kind == "notify" {
            continue;
        }
        let Some(message) = &event.message else {
            continue;
        };
        for segment in message.split("; ") {
            assert!(
                MESSAGE_TOKEN_PREFIXES
                    .iter()
                    .any(|p| segment.starts_with(p)),
                "消息段不在词表(docs/architecture.md §5.1): {segment}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

fn anthropic_tool_sse() -> Vec<String> {
    [
        r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-up","usage":{"input_tokens":5}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"running"}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"shell","input":{}}}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":[\"ls\"]}"}}"#,
        r#"{"type":"content_block_stop","index":1}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}"#,
        r#"{"type":"message_stop"}"#,
    ]
    .map(|data| format!("data: {data}\n\n"))
    .to_vec()
}

#[tokio::test]
async fn empty_api_key_forwards_without_auth_headers() {
    // 【Rust 变更】空 key = 无鉴权上游(本地 LLM/内网中转),照常转发不再 401。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].api_key = String::new();
    let engine = engine_with(config, fake.clone());
    fake.push("a.example.com", sse_ok(&["data: {\"ok\":1}\n\n"]));

    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);

    let recorded = fake.requests();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].host, "a.example.com");
    // 不发任何鉴权头。
    assert!(
        !recorded[0]
            .headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("authorization")
                || n.eq_ignore_ascii_case("x-api-key")),
        "空 key 时不应发送鉴权头,实际 headers: {:?}",
        recorded[0].headers
    );
    // 其余指纹头照旧。
    assert!(
        recorded[0]
            .headers
            .iter()
            .any(|(n, v)| n == "user-agent" && v == "claude-cli/2.1.220 (external, cli)")
    );
}

#[tokio::test]
async fn plaintext_http_endpoint_forwards_over_http() {
    // 【Rust 变更】http:// 上游走明文(Ollama/vLLM 等本地服务)。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].base_url = "http://127.0.0.1:11434".into();
    config.endpoints[0].api_key = String::new();
    config.endpoints[1].enabled = false;
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push("127.0.0.1", sse_ok(&["data: {\"ok\":1}\n\n"]));

    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    assert_eq!(fake.requests()[0].host, "127.0.0.1");

    // 事件里的 upstreamHost 也应是明文地址的 host。
    let runtime = runtime_of(&engine).await;
    let upstream = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "upstream")
        .unwrap();
    assert_eq!(upstream.upstream_host.as_deref(), Some("127.0.0.1"));
}

#[tokio::test]
async fn no_hardcoded_request_timeout_when_config_says_none() {
    // 回应「是不是硬编码 60s 超时」:两个超时都为 None 时,引擎不施加任何截止;
    // provider/model 冷却有独立上限,不掐断进行中的请求。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.response_timeout_seconds = None;
    config.retry.stream_idle_timeout_seconds = None;
    let engine = engine_with(config, fake.clone());
    fake.push("a.example.com", Outcome::Gated { status: 200 });

    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    assert_eq!(response.status(), 200);
    let tx = fake.gate_sender();
    let mut stream = response.into_body().into_data_stream();

    // 拉长的静默间隔不触发任何取消(真实场景里分类器思考期就是这样)。
    for i in 0..3 {
        tokio::time::sleep(Duration::from_millis(120)).await;
        tx.send(Ok(Bytes::from(format!("data: chunk{i}\n\n"))))
            .unwrap();
        let chunk = stream.next().await.unwrap().unwrap();
        assert_eq!(chunk, Bytes::from(format!("data: chunk{i}\n\n")));
    }
    drop(tx); // 上游正常收尾
    assert!(stream.next().await.is_none());

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert_eq!(client.status_code, 200, "长静默流不应被超时/取消");
    assert_eq!(runtime.client_successes, 1);
}

#[tokio::test]
async fn cidr_and_inbound_auth_enforced() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.listener.allowed_cidrs = vec!["192.168.1.0/24".into()];
    config.listener.auth_token = "sk-inbound".into();
    let engine = engine_with(config, fake.clone());

    // CIDR 外 403(全路径最先)。
    let (status, _) = call(
        &engine,
        Some("8.8.8.8".parse().unwrap()),
        "/v1/messages",
        vec![],
        body(),
    )
    .await;
    assert_eq!(status, 403);

    // CIDR 内但无 token:401。
    let (status, resp) = call(
        &engine,
        Some("192.168.1.9".parse().unwrap()),
        "/v1/messages",
        vec![],
        body(),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(
        serde_json::from_slice::<Value>(&resp).unwrap()["error"],
        "inbound_auth_required"
    );

    // Bearer 正确:放行到出站。
    fake.push("a.example.com", sse_ok(&["event: ok\ndata: {}\n\n"]));
    let (status, _) = call(
        &engine,
        Some("192.168.1.9".parse().unwrap()),
        "/v1/messages",
        vec![("Authorization".into(), "Bearer sk-inbound".into())],
        body(),
    )
    .await;
    assert_eq!(status, 200);

    // 环回恒放行(x-api-key 形式)。
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![("x-api-key".into(), "sk-inbound".into())],
        body(),
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn retryable_status_fails_over_and_counts_once() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 503,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("b.example.com", sse_ok(&["data: {\"type\":\"ping\"}\n\n"]));

    let (status, resp) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    assert_eq!(
        String::from_utf8_lossy(&resp),
        "data: {\"type\":\"ping\"}\n\n"
    );

    let runtime = runtime_of(&engine).await;
    assert_eq!(runtime.upstream_attempts, 2);
    assert_eq!(runtime.upstream_failures, 1);
    assert_eq!(runtime.upstream_successes, 1);
    assert_eq!(runtime.client_requests, 1);
    assert_eq!(runtime.client_successes, 1);
    assert_eq!(runtime.failovers, 1);

    // 事件:首入口 failover=false,次入口 failover=true。
    let upstream: Vec<_> = runtime
        .recent_events
        .iter()
        .filter(|e| e.kind == "upstream")
        .collect();
    let a = upstream
        .iter()
        .find(|e| e.endpoint_id.as_deref() == Some("a"))
        .unwrap();
    let b = upstream
        .iter()
        .find(|e| e.endpoint_id.as_deref() == Some("b"))
        .unwrap();
    assert!(!a.failover);
    assert!(b.failover);
    assert_eq!(a.status_code, 503);
    assert_eq!(b.status_code, 200);
    assert!(
        a.ttfb_ms.is_some(),
        "503 已收到响应头，必须记录该次尝试 TTFB"
    );
    assert!(a.ttfb_ms.unwrap() <= a.duration_ms);
    let client = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .unwrap();
    assert!(
        upstream
            .iter()
            .all(|event| event.request_id == client.request_id),
        "同一次请求的所有上游尝试必须共享 requestID"
    );
}

#[tokio::test]
async fn transport_error_fails_over_and_bad_status_401_also_retries() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    // 401 在可重试集内(坏 key 不终结请求)。
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 401,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);

    // 传输错误同样换下家。
    fake.push("a.example.com", Outcome::Error("connection refused".into()));
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn non_retryable_status_passes_through() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 404,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![b"{\"err\":true}".to_vec()],
        },
    );
    let (status, resp) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 404); // 原样透传,不 failover
    assert_eq!(resp, b"{\"err\":true}");
    assert_eq!(fake.requests().len(), 1);

    let runtime = runtime_of(&engine).await;
    assert_eq!(runtime.client_failures, 1); // 404 按失败记
}

#[tokio::test]
async fn bad_request_400_does_not_switch_endpoints() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 400,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![b"{\"error\":\"bad request\"}".to_vec()],
        },
    );
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 400);
    assert_eq!(fake.requests().len(), 1, "请求错误不得盲目换入口");
}

#[tokio::test]
async fn deferred_rounds_retry_without_passthrough_until_duration_gate() {
    let fake = FakeTransport::new();
    fake.set_delay(Duration::from_millis(2)); // 控制节奏,防止瞬间烧穿剧本
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 0; // 不限轮
    config.retry.max_retry_duration_seconds = 0.6; // 容纳首轮 0.5s 退避，第二轮后由时长闸刹停
    let engine = engine_with(config, fake.clone());
    // 双入口持续 429/502/503:永不透传,直到时长闸。
    for _ in 0..64 {
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 429,
                headers: vec![],
                chunks: vec![],
            },
        );
        fake.push(
            "b.example.com",
            Outcome::Status {
                status: 503,
                headers: vec![],
                chunks: vec![],
            },
        );
    }
    let (status, resp) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 503); // 最后可重试状态原样回
    assert_eq!(
        serde_json::from_slice::<Value>(&resp).unwrap()["error"],
        "upstream_retryable_status"
    );
    let attempts = fake.requests().len();
    assert!(attempts > 2, "跨轮重试应发生多次尝试,实际 {attempts}");
    let runtime = runtime_of(&engine).await;
    assert_eq!(runtime.failovers, 1); // 换过入口,但每请求最多 +1
    assert_eq!(runtime.client_requests, 1);
}

#[tokio::test]
async fn deferred_round_recovers_on_second_round() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 2;
    let engine = engine_with(config, fake.clone());
    // 第一轮双 503;第二轮 a 恢复 200。
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 503,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push(
        "b.example.com",
        Outcome::Status {
            status: 503,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("a.example.com", sse_ok(&["data: {\"ok\":1}\n\n"]));
    let (status, resp) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&resp).contains("ok"));
    assert_eq!(fake.requests().len(), 3);
}

#[tokio::test]
async fn deferred_all_502_retries_next_round_and_recovers() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 2;
    let engine = engine_with(config, fake.clone());
    // 502 属于统一 retryable 集，经退避重跑下一轮，不一轮即弃。
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 502,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push(
        "b.example.com",
        Outcome::Status {
            status: 502,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("a.example.com", sse_ok(&["data: {\"ok\":1}\n\n"]));
    let (status, resp) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&resp).contains("ok"));
    assert_eq!(fake.requests().len(), 3);
}

#[tokio::test]
async fn mixed_retryable_http_and_connection_failure_recovers_next_round() {
    // 连接失败与上游 HTTP 401 都是首响应前可自愈故障，混合出现也必须跨轮。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 2;
    let engine = engine_with(config, fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 401,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("b.example.com", Outcome::Error("connection reset".into()));
    fake.push("a.example.com", sse_ok(&["data: {\"recovered\":1}\n\n"]));

    let (status, response) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&response).contains("recovered"));
    assert_eq!(
        fake.requests()
            .iter()
            .map(|request| request.host.as_str())
            .collect::<Vec<_>>(),
        vec!["a.example.com", "b.example.com", "a.example.com"],
        "混合可重试故障必须进入下一轮"
    );
}

#[tokio::test]
async fn http_502_mixed_with_connection_error_retries_and_recovers() {
    // 连接失败与上游 HTTP 502 都是首响应前可自愈故障，混合出现也必须跨轮。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 2;
    let engine = engine_with(config, fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 502,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("b.example.com", Outcome::Error("connection reset".into()));
    fake.push("a.example.com", sse_ok(&["data: {\"recovered\":1}\n\n"]));

    let (status, response) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&response).contains("recovered"));
    assert_eq!(
        fake.requests()
            .iter()
            .map(|request| request.host.as_str())
            .collect::<Vec<_>>(),
        vec!["a.example.com", "b.example.com", "a.example.com"],
        "混合可重试故障必须进入下一轮"
    );
}

#[tokio::test]
async fn timeout_and_connection_failure_recover_on_next_round() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 2;
    let engine = engine_with(config, fake.clone());
    fake.push("a.example.com", Outcome::Timeout);
    fake.push("b.example.com", Outcome::Error("connection reset".into()));
    fake.push("a.example.com", sse_ok(&["data: {\"recovered\":1}\n\n"]));

    let (status, response) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&response).contains("recovered"));
    assert_eq!(fake.requests().len(), 3);
}

#[tokio::test]
async fn invalid_response_does_not_loop_even_in_unlimited_mode() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 0;
    config.retry.max_retry_duration_seconds = 0.0;
    let engine = engine_with(config, fake.clone());
    fake.push("a.example.com", Outcome::Invalid("bad URL".into()));
    fake.push("b.example.com", Outcome::Invalid("bad header".into()));

    let (status, response) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 502);
    assert_eq!(fake.requests().len(), 2);
    let json: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(json["failureKind"], "invalid_response");
}

#[tokio::test]
async fn unlimited_retry_is_backed_off_and_request_cancellation_stops_it() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 0;
    config.retry.max_retry_duration_seconds = 0.0;
    for _ in 0..8 {
        for host in ["a.example.com", "b.example.com"] {
            fake.push(
                host,
                Outcome::Status {
                    status: 503,
                    headers: vec![],
                    chunks: vec![],
                },
            );
        }
    }
    let engine = engine_with(config, fake.clone());
    let request_engine = engine.clone();
    let request = tokio::spawn(async move {
        request_engine
            .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
            .await
    });

    for _ in 0..50 {
        if fake.requests().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(fake.requests().len(), 2, "第一轮应遍历两个入口");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        fake.requests().len(),
        2,
        "0/0 无限重试也必须退避，不能 busy loop"
    );

    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    tokio::time::sleep(Duration::from_millis(550)).await;
    assert_eq!(
        fake.requests().len(),
        2,
        "请求 future 取消后不得留下后台重试"
    );

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .expect("取消请求应保留 client 事件");
    assert_eq!(client.status_code, 499);
    assert_eq!(client.outcome, Some(RuntimeEventOutcome::Cancelled));
}

#[tokio::test]
async fn tcp_client_disconnect_during_retry_backoff_records_499_and_stops() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 0;
    config.retry.max_retry_duration_seconds = 0.0;
    for _ in 0..4 {
        for host in ["a.example.com", "b.example.com"] {
            fake.push(
                host,
                Outcome::Status {
                    status: 503,
                    headers: vec![],
                    chunks: vec![],
                },
            );
        }
    }
    let engine = engine_with(config, fake.clone());
    let (address, server) =
        sumpter_macos_adapter::server::serve(engine.clone(), SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();

    let payload = body();
    let mut request = format!(
        "POST /v1/messages HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
        payload.len()
    )
    .into_bytes();
    request.extend_from_slice(&payload);
    let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
    client.write_all(&request).await.unwrap();
    client.flush().await.unwrap();

    for _ in 0..100 {
        if fake.requests().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(fake.requests().len(), 2, "首轮应遍历两个入口并进入退避");
    drop(client);

    let mut cancelled = false;
    for _ in 0..100 {
        let runtime = runtime_of(&engine).await;
        cancelled = runtime.recent_events.iter().any(|event| {
            event.kind == "client"
                && event.status_code == 499
                && event.outcome == Some(RuntimeEventOutcome::Cancelled)
        });
        if cancelled {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(cancelled, "真实 TCP 客户端断开应取消首响应前的重试 future");
    tokio::time::sleep(Duration::from_millis(550)).await;
    assert_eq!(fake.requests().len(), 2, "客户端断开后不得继续下一轮");

    server.abort();
}

#[tokio::test]
async fn all_hard_failures_return_502() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    fake.push("a.example.com", Outcome::Error("dns fail".into()));
    fake.push("b.example.com", Outcome::Error("reset".into()));
    let (status, resp) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 502);
    let json: Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(json["error"], "upstream_unavailable");
    assert_eq!(json["failureKind"], "connection_failed");
    assert_eq!(json["failurePhase"], "before_response");
    assert!(json["requestID"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(json.get("upstreamStatusCode").is_none());

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .unwrap();
    assert_eq!(client.outcome, Some(RuntimeEventOutcome::Failed));
    assert_eq!(
        client.failure_kind,
        Some(RuntimeFailureKind::ConnectionFailed)
    );
    assert_eq!(
        client.failure_phase,
        Some(RuntimeFailurePhase::BeforeResponse)
    );
    assert_eq!(
        client.endpoint_id.as_deref(),
        Some("b"),
        "响应头前全部失败时应保留最后实际尝试入口"
    );
    assert_eq!(client.endpoint_name.as_deref(), Some("b"));
    assert_eq!(client.upstream_host.as_deref(), Some("b.example.com"));
    assert_eq!(client.upstream_model.as_deref(), Some("claude-opus-5"));
    assert_eq!(client.source_format, Some(ProviderProtocol::Anthropic));
    assert_eq!(client.target_format, Some(ProviderProtocol::Anthropic));
    assert_eq!(
        client.route_mode,
        Some(sumpter_core::routing::RouteMode::Native)
    );
    assert_eq!(client.upstream_status_code, None);
    assert_eq!(client.request_id.as_deref(), json["requestID"].as_str());
}

#[tokio::test]
async fn response_timeout_reports_effective_200_second_deadline() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[1].enabled = false;
    config.retry.response_timeout_seconds = Some(200.0);
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push("a.example.com", Outcome::Timeout);

    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    assert_eq!(response.status(), 502);
    let response_request_id = response
        .headers()
        .get("x-sumpter-request-id")
        .and_then(|value| value.to_str().ok())
        .expect("失败响应应返回关联 ID")
        .to_string();
    let response = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(json["failureKind"], "response_timeout");
    assert_eq!(json["timeoutMS"], 200_000);
    assert_eq!(json["requestID"], response_request_id);
    assert!(json.get("upstreamStatusCode").is_none());

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .unwrap();
    assert_eq!(client.timeout_ms, Some(200_000));
    assert_eq!(
        client.failure_kind,
        Some(RuntimeFailureKind::ResponseTimeout)
    );
}

#[tokio::test]
async fn upstream_http_502_keeps_status_and_request_id_distinct_from_transport_502() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[1].enabled = false;
    config.retry.max_deferred_rounds = 1;
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 502,
            headers: vec![("x-request-id".into(), "upstream-trace-502".into())],
            chunks: vec![],
        },
    );

    let (status, response) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 502);
    let json: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(json["error"], "upstream_retryable_status");
    assert_eq!(json["failureKind"], "upstream_http_status");
    assert_eq!(json["upstreamStatusCode"], 502);
    assert_eq!(json["upstreamRequestID"], "upstream-trace-502");

    let runtime = runtime_of(&engine).await;
    let upstream = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "upstream")
        .unwrap();
    assert_eq!(upstream.upstream_status_code, Some(502));
    assert_eq!(
        upstream.failure_kind,
        Some(RuntimeFailureKind::UpstreamHttpStatus)
    );
    assert_eq!(
        upstream.upstream_request_id.as_deref(),
        Some("upstream-trace-502")
    );
}

#[tokio::test]
async fn upstream_http_500_retries_same_entry_then_moves_on() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 1;
    config.retry.max_500_retries = 1;
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 500,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("a.example.com", sse_ok(&["data: {\"recovered\":1}\n\n"]));

    let (status, response) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&response).contains("recovered"));
    assert_eq!(fake.requests().len(), 2);
}

#[tokio::test]
async fn upstream_http_500_zero_retries_switches_to_next_entry() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 1;
    config.retry.max_500_retries = 0;
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 500,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("b.example.com", sse_ok(&["data: {\"switched\":1}\n\n"]));

    let (status, response) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&response).contains("switched"));
    assert_eq!(fake.requests().len(), 2);
}

#[tokio::test]
async fn upstream_http_500_can_stop_without_failover() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 1;
    config.retry.max_500_retries = 0;
    config.retry.failover_on_500 = false;
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 500,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push(
        "b.example.com",
        sse_ok(&["data: {\"should_not_switch\":1}\n\n"]),
    );

    let (status, response) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 500);
    assert!(!String::from_utf8_lossy(&response).contains("should_not_switch"));
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn final_upstream_500_includes_custom_retry_delay() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[1].enabled = false;
    config.retry.max_deferred_rounds = 1;
    config.retry.retry_delay_seconds = Some(3.5);
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 500,
            headers: vec![],
            chunks: vec![],
        },
    );

    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    assert_eq!(response.status(), 500);
    assert_eq!(response.headers().get("retry-after").unwrap(), "4");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["upstreamStatusCode"], 500);
    assert_eq!(json["retry_delay"], 3.5);
}

#[tokio::test]
async fn final_retry_delay_can_be_hidden_from_client() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[1].enabled = false;
    config.retry.max_deferred_rounds = 1;
    config.retry.retry_delay_seconds = Some(3.5);
    config.retry.pass_through_retry_delay = false;
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 500,
            headers: vec![],
            chunks: vec![],
        },
    );

    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    assert_eq!(response.status(), 500);
    assert!(response.headers().get("retry-after").is_none());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json.get("retry_delay").is_none());
}

#[tokio::test]
async fn upstream_http_500_does_not_enter_unbounded_cross_round_retry() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 0;
    config.retry.max_retry_duration_seconds = 0.0;
    config.retry.max_500_retries = 0;
    let engine = engine_with(config.normalized(), fake.clone());
    for host in ["a.example.com", "b.example.com"] {
        fake.push(
            host,
            Outcome::Status {
                status: 500,
                headers: vec![],
                chunks: vec![],
            },
        );
    }

    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    assert_eq!(response.status(), 500);
    assert_eq!(fake.requests().len(), 2);
}

#[tokio::test]
async fn streaming_relay_chunked_sse_and_events_upserted() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    fake.push(
        "a.example.com",
        sse_ok(&[
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"cache_read_input_tokens\":4}}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":7},\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ]),
    );

    let mut request: serde_json::Value = serde_json::from_slice(&body()).unwrap();
    request["metadata"] =
        json!({"user_id": json!({"session_id":"native-claude-session"}).to_string()});
    let raw_body = Bytes::from(serde_json::to_vec(&request).unwrap());
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/messages",
            vec![(
                "user-agent".into(),
                "claude-cli/2.1.220 (external, cli)".into(),
            )],
            raw_body.clone(),
        )
        .await;
    assert_eq!(response.status(), 200);
    let response_request_id = response
        .headers()
        .get("x-sumpter-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .expect("成功响应应返回关联 ID");
    let mut stream = response.into_body().into_data_stream();
    let mut collected = Vec::new();
    while let Some(chunk) = stream.next().await {
        collected.extend_from_slice(&chunk.unwrap());
    }
    let text = String::from_utf8_lossy(&collected);
    assert!(text.contains("message_start"));
    assert!(text.contains("message_stop"));

    let runtime = runtime_of(&engine).await;
    // 一条 client + 一条 upstream,均 completed 且计数一次。
    assert_eq!(runtime.client_requests, 1);
    assert_eq!(runtime.upstream_attempts, 1);
    for event in &runtime.recent_events {
        assert_eq!(event.session_id.as_deref(), Some("native-claude-session"));
        assert_eq!(event.session_source.as_deref(), Some("claude_metadata"));
    }
    let forwarded: serde_json::Value = serde_json::from_slice(&fake.requests()[0].body).unwrap();
    assert_eq!(
        forwarded["metadata"]["user_id"],
        request["metadata"]["user_id"]
    );
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert_eq!(client.status_code, 200);
    assert_eq!(
        client.request_id.as_deref(),
        Some(response_request_id.as_str())
    );
    assert!(!client.is_in_flight());
    assert_eq!(client.client_model.as_deref(), Some("claude-opus-5"));
    let trace = client.stream_trace.as_ref().expect("完成流应带脱敏 trace");
    assert!(trace.chunk_count.unwrap_or_default() >= 1);
    assert!(trace.bytes_received.unwrap_or_default() > 0);
    assert_eq!(trace.terminal_event.as_deref(), Some("completed"));
    assert_eq!(
        client.cache_read.as_ref().unwrap().state,
        sumpter_core::cache_read::CacheReadState::Hit
    );
    assert_eq!(client.cache_read.as_ref().unwrap().read_tokens, Some(4));
    assert_eq!(
        trace.usage.as_ref().and_then(|usage| usage.input_tokens),
        Some(12)
    );
    assert_eq!(
        trace.usage.as_ref().and_then(|usage| usage.output_tokens),
        Some(7)
    );
    assert_eq!(
        trace
            .usage
            .as_ref()
            .and_then(|usage| usage.cache_read_input_tokens),
        Some(4)
    );
}

#[tokio::test]
async fn accepted_backfills_client_in_flight_endpoint_attribution() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    fake.push("a.example.com", Outcome::Gated { status: 200 });

    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    assert_eq!(response.status(), 200);

    // 响应头一 accepted,client in-flight 事件就应带上入口归属(此前一直是空,
    // 流式期间只能切「上游」筛选对时间戳)。
    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert!(client.is_in_flight());
    assert_eq!(client.phase, Some(RuntimeEventPhase::InFlight));
    assert_eq!(client.status_code, 200);
    assert_eq!(client.outcome, None);
    assert_eq!(client.upstream_status_code, Some(200));
    assert!(client.duration_ms >= client.ttfb_ms.unwrap_or_default());
    assert_eq!(client.endpoint_id.as_deref(), Some("a"));
    assert_eq!(client.endpoint_name.as_deref(), Some("a"));
    assert_eq!(client.upstream_host.as_deref(), Some("a.example.com"));
    assert_eq!(client.client_model.as_deref(), Some("claude-opus-5"));
    // in-flight upsert 不计数。
    assert_eq!(runtime.client_requests, 0);

    // 流尽后照常完成:归属保留、原地转完成态并计数一次。
    let tx = fake.gate_sender();
    tx.send(Ok(Bytes::from_static(b"data: {\"ok\":1}\n\n")))
        .unwrap();
    drop(tx);
    let mut stream = response.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        chunk.unwrap();
    }
    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert!(!client.is_in_flight());
    assert_eq!(client.phase, Some(RuntimeEventPhase::Completed));
    assert_eq!(client.status_code, 200);
    assert_eq!(client.endpoint_id.as_deref(), Some("a"));
    assert_eq!(runtime.client_requests, 1);
}

/// TTFB 存在的全部理由:流式请求的 durationMS 是「吐完最后一个字」,
/// 「上游卡住 80s」和「正常长输出 80s」在它眼里一模一样。ttfbMS 把两者分开。
#[tokio::test]
async fn initial_client_in_flight_records_preferred_translated_protocol_route() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[1].enabled = false;
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push("a.example.com", Outcome::Hang);

    let request_engine = engine.clone();
    let request = tokio::spawn(async move {
        request_engine
            .handle_request(
                loopback(),
                "POST",
                "/v1/responses",
                vec![],
                Bytes::from_static(br#"{"model":"claude-opus-5","input":"hello","stream":true}"#),
            )
            .await
    });
    for _ in 0..100 {
        if !fake.requests().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        fake.requests().len(),
        1,
        "request should reach preferred endpoint"
    );

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .expect("initial in-flight client event");
    assert_eq!(client.phase, Some(RuntimeEventPhase::InFlight));
    assert_eq!(client.status_code, 0);
    assert_eq!(
        client.source_format,
        Some(ProviderProtocol::OpenAIResponses)
    );
    assert_eq!(client.target_format, Some(ProviderProtocol::Anthropic));
    assert_eq!(
        client.route_mode,
        Some(sumpter_core::routing::RouteMode::Translated)
    );

    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn ttfb_separates_slow_first_byte_from_long_stream() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.response_timeout_seconds = None;
    config.retry.stream_idle_timeout_seconds = None;
    let engine = engine_with(config, fake.clone());
    fake.push("a.example.com", Outcome::Gated { status: 200 });

    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    assert_eq!(response.status(), 200);

    // 响应头已到:in-flight 事件此刻就该带 TTFB,不必等流结束。
    let runtime = runtime_of(&engine).await;
    let in_flight = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert!(in_flight.is_in_flight());
    let in_flight_ttfb = in_flight.ttfb_ms.expect("accepted 后必有 TTFB");
    assert_eq!(in_flight.status_code, 200);
    assert_eq!(in_flight.outcome, None);
    assert_eq!(in_flight.upstream_status_code, Some(200));
    assert!(
        in_flight.duration_ms >= in_flight_ttfb,
        "进行中总时长不能小于 TTFB: duration={}ms ttfb={}ms",
        in_flight.duration_ms,
        in_flight_ttfb
    );

    // 首字节之后拖很久才吐完:这是「正常长输出」,不是故障。
    let tx = fake.gate_sender();
    tokio::time::sleep(Duration::from_millis(150)).await;
    tx.send(Ok(Bytes::from_static(b"data: {\"ok\":1}\n\n")))
        .unwrap();
    drop(tx);
    let mut stream = response.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        chunk.unwrap();
    }

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    let ttfb = client.ttfb_ms.expect("完成事件保留 TTFB");
    assert_eq!(
        ttfb, in_flight_ttfb,
        "TTFB 钉在响应头时刻,不该被后续流式时间污染"
    );
    assert!(
        client.duration_ms >= 150,
        "总时长含流式输出: {}ms",
        client.duration_ms
    );
    assert!(
        ttfb < client.duration_ms,
        "首字节必早于流结束: ttfb={ttfb}ms duration={}ms",
        client.duration_ms
    );

    // upstream 事件同样带 TTFB(口径是该次尝试自身)。
    let upstream = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "upstream")
        .unwrap();
    assert!(upstream.ttfb_ms.is_some());
    assert!(upstream.ttfb_ms.unwrap() <= upstream.duration_ms);
}

/// client 与 upstream 的 ttfbMS 口径不同:前者是客户端视角的总等待(含 failover
/// 的全部尝试),后者只是该次尝试自身。看错口径会把「换了一次入口」误读成「入口很慢」。
#[tokio::test]
async fn client_ttfb_spans_failover_while_upstream_ttfb_is_per_attempt() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    // 每次出站都等 40ms 才给响应头:失败那次也要走完这段。
    fake.set_delay(Duration::from_millis(40));
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 503,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("b.example.com", sse_ok(&["data: {\"type\":\"ping\"}\n\n"]));

    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    let winner = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "upstream" && e.endpoint_id.as_deref() == Some("b"))
        .unwrap();

    let client_ttfb = client.ttfb_ms.expect("accepted 过就有 TTFB");
    let winner_ttfb = winner.ttfb_ms.expect("胜出尝试有自己的 TTFB");
    assert!(client_ttfb >= 80, "客户端等了两轮 40ms: {client_ttfb}ms");
    assert!(
        winner_ttfb >= 40 && winner_ttfb < client_ttfb,
        "胜出入口只该算自己那 40ms: winner={winner_ttfb}ms client={client_ttfb}ms"
    );

    // 失败的那次已收到 503 响应头，TTFB 仍应按该次尝试记录。
    let loser = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "upstream" && e.endpoint_id.as_deref() == Some("a"))
        .unwrap();
    assert_eq!(loser.status_code, 503);
    assert!(loser.ttfb_ms.is_some(), "503 已收到响应头，必须记录 TTFB");
    assert!(loser.ttfb_ms.unwrap() <= loser.duration_ms);
}

/// 请求在规划/鉴权阶段就被拒 → 从未 accepted → 没有 TTFB。
#[tokio::test]
async fn rejected_request_has_no_ttfb() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.listener.auth_token = "sk-inbound".into();
    let engine = engine_with(config, fake.clone());

    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 401);

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert_eq!(client.ttfb_ms, None);
}

/// 指纹失配的可见信号:CC 换了内部请求的 system 措辞后,用途会静默退化成「普通请求」。
/// `unmatched_no_tools` 是唯一能让人察觉的提示,所以它必须真的进到事件 message 里。
#[tokio::test]
async fn drifted_internal_request_gets_unmatched_token() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    fake.push("a.example.com", sse_ok(&["data: {\"type\":\"ping\"}\n\n"]));

    // 带专用 system 的单轮无工具请求,但谁的指纹都没命中。
    let drifted = Bytes::from(
        serde_json::to_vec(&json!({
            "model": "claude-opus-5",
            "stream": true,
            "system": "Generate a short title for this conversation.",
            "messages": [{"role": "user", "content": "<session>rust refactor</session>"}],
        }))
        .unwrap(),
    );
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![(
            "user-agent".into(),
            "claude-cli/2.1.220 (external, cli)".into(),
        )],
        drifted,
    )
    .await;
    assert_eq!(status, 200);

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert_eq!(
        client.request_purpose,
        Some(sumpter_core::RequestPurpose::Standard),
        "指纹没命中,用途只能是 standard"
    );
    assert_eq!(client.message.as_deref(), Some("unmatched_no_tools"));

    // 该 token 描述请求本身,与入口无关 → 不该污染 upstream 事件。
    let upstream = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "upstream")
        .unwrap();
    assert_eq!(upstream.message, None);
}

/// 正常主对话(带 tools + CC 身份 system)不该挂提示,否则消息列会常年是噪音。
#[tokio::test]
async fn main_chat_has_no_unmatched_token() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    fake.push("a.example.com", sse_ok(&["data: {\"type\":\"ping\"}\n\n"]));

    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert_eq!(client.message, None, "一次打通的主对话:消息列应保持空");
}

#[tokio::test]
async fn client_disconnect_mid_stream_records_499_and_cancels() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    fake.push("a.example.com", Outcome::Gated { status: 200 });

    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    assert_eq!(response.status(), 200);
    let tx = fake.gate_sender();
    let mut stream = response.into_body().into_data_stream();
    tx.send(Ok(Bytes::from_static(b"data: chunk1\n\n")))
        .unwrap();
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(&first[..], b"data: chunk1\n\n");

    // 客户端断开:drop 响应体流 → guard 记 499,上游流随之撕掉。
    drop(stream);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert_eq!(client.status_code, 499);
    assert_eq!(runtime.client_requests, 1);
    assert_eq!(runtime.client_successes, 0);
    assert_eq!(runtime.client_failures, 0); // 499 不计成败
    assert_eq!(client.outcome, Some(RuntimeEventOutcome::Cancelled));
    assert_eq!(client.phase, Some(RuntimeEventPhase::Completed));
    assert_eq!(
        client.failure_kind,
        Some(RuntimeFailureKind::ClientCancelled)
    );
    let trace = client
        .stream_trace
        .as_ref()
        .expect("取消也应保留已读流 trace");
    assert_eq!(trace.chunk_count, Some(1));
    assert!(trace.bytes_received.unwrap_or_default() > 0);
    assert_eq!(trace.terminal_event, None);
    let upstream = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "upstream")
        .unwrap();
    assert_eq!(
        upstream.status_code, 200,
        "取消不能改写已收到的上游 HTTP 状态"
    );
    assert_eq!(upstream.outcome, Some(RuntimeEventOutcome::Cancelled));
    assert_eq!(upstream.phase, Some(RuntimeEventPhase::Completed));
    assert_eq!(runtime.upstream_successes, 0);
    assert_eq!(runtime.upstream_failures, 0);
    assert!(
        tx.send(Ok(Bytes::from_static(b"late"))).is_err() || {
            // 消费端已 drop;发送可能尚未察觉,但流不再被拉取。
            true
        }
    );
}

#[tokio::test]
async fn interrupted_stream_keeps_http_200_but_counts_final_failure() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    fake.push("a.example.com", Outcome::Gated { status: 200 });

    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    assert_eq!(response.status(), 200);
    let tx = fake.gate_sender();
    tx.send(Ok(Bytes::from_static(b"data: partial\n\n")))
        .unwrap();
    tx.send(Err(TransportError::ConnectionFailed("peer reset".into())))
        .unwrap();
    let _ = axum::body::to_bytes(response.into_body(), usize::MAX).await;

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .unwrap();
    let upstream = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "upstream")
        .unwrap();
    assert_eq!(client.status_code, 200, "响应头已发出后不能再伪造 HTTP 502");
    assert_eq!(client.outcome, Some(RuntimeEventOutcome::Failed));
    assert_eq!(client.phase, Some(RuntimeEventPhase::Completed));
    assert_eq!(
        client.failure_kind,
        Some(RuntimeFailureKind::StreamInterrupted)
    );
    assert_eq!(
        client.failure_phase,
        Some(RuntimeFailurePhase::ResponseStream)
    );
    assert_eq!(upstream.status_code, 200);
    assert_eq!(upstream.outcome, Some(RuntimeEventOutcome::Failed));
    assert_eq!(upstream.phase, Some(RuntimeEventPhase::Completed));
    assert_eq!(runtime.client_successes, 0);
    assert_eq!(runtime.client_failures, 1);
    assert_eq!(runtime.upstream_successes, 0);
    assert_eq!(runtime.upstream_failures, 1);
}

#[tokio::test]
async fn stream_idle_timeout_reports_its_own_post_header_deadline() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.stream_idle_timeout_seconds = Some(0.01);
    let engine = engine_with(config, fake.clone());
    fake.push("a.example.com", Outcome::Gated { status: 200 });

    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    assert_eq!(response.status(), 200);
    let _upstream_still_open = fake.gate_sender();
    let _ = axum::body::to_bytes(response.into_body(), usize::MAX).await;

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .unwrap();
    assert_eq!(client.status_code, 200);
    assert_eq!(client.outcome, Some(RuntimeEventOutcome::Failed));
    assert_eq!(
        client.failure_kind,
        Some(RuntimeFailureKind::StreamIdleTimeout)
    );
    assert_eq!(client.timeout_ms, Some(10));
    assert_eq!(
        client.failure_phase,
        Some(RuntimeFailurePhase::ResponseStream)
    );
}

#[tokio::test]
async fn openai_endpoint_bridged_to_anthropic_sse() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::OpenAI;
    // 迁移后入口已持有原池级 `claude-*` 显式映射；本用例要验证精确别名，
    // 因而用精确映射替换 fixture，避免被先出现的通配映射遮蔽。
    config.endpoints[0].mappings = vec![ModelMapping {
        client_pattern: "claude-opus-5".into(),
        context: ContextMode::Standard,
        failover_timeout_seconds: None,
        thinking: ThinkingMode::Disabled,
        effort: None,
        upstream_model: "gpt-up".into(),
        capabilities: Vec::new(),
    }];
    // b 停用,只走 a。
    config.endpoints[1].enabled = false;
    let engine = engine_with(config.normalized(), fake.clone());

    fake.push(
        "a.example.com",
        sse_ok(&[
            "data: {\"model\":\"gpt-up\",\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ]),
    );
    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream; charset=utf-8"
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("message_start"));
    assert!(text.contains("\"text\":\"Hi\""));
    assert!(text.contains("message_stop"));

    // 出站请求:路径 /v1/chat/completions、模型改写、CC UA。
    let recorded = fake.requests();
    assert_eq!(recorded[0].path, "/v1/chat/completions");
    let sent: Value = serde_json::from_slice(&recorded[0].body).unwrap();
    assert_eq!(sent["model"], "gpt-up");
    let ua = recorded[0]
        .headers
        .iter()
        .find(|(n, _)| n == "user-agent")
        .map(|(_, v)| v.clone())
        .unwrap();
    assert_eq!(ua, "claude-cli/2.1.220 (external, cli)");
}

#[tokio::test]
async fn native_endpoint_discards_translated_tool_candidate_before_forwarding() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::OpenAI;
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));

    let request_body = Bytes::from(
        serde_json::to_vec(&json!({
            "model": "claude-opus-5",
            "messages": [{"role": "user", "content": "x"}],
            "tools": [{"name": "Bash", "input_schema": {}}],
        }))
        .unwrap(),
    );
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], request_body).await;
    assert_eq!(status, 200);
    // 原生 Anthropic 候选 b 存在时，固定 Chat 的桥接候选 a 在规划阶段直接丢弃。
    let hosts: Vec<String> = fake.requests().iter().map(|r| r.host.clone()).collect();
    assert_eq!(hosts, vec!["b.example.com"]);
    let runtime = runtime_of(&engine).await;
    assert!(
        !runtime
            .recent_events
            .iter()
            .any(|e| e.kind == "upstream" && e.endpoint_id.as_deref() == Some("a"))
    );
}

#[tokio::test]
async fn control_endpoints_require_token_and_status_masks_auth() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.listener.auth_token = "secret-inbound".into();
    let engine = engine_with(config, fake.clone());

    // 旧统计控制端点永久移除，不再受 token 兼容路径影响。
    for path in [
        "/__runtime",
        "/__reset-stats",
        "/__reset-stats?token=test-token",
    ] {
        let (status, _) = call(&engine, loopback(), path, vec![], Bytes::new()).await;
        assert_eq!(status, 404, "{path} 应保持移除");
    }
    // notify/reload 仍是旧 hook 兼容端点，无 token 时拒绝。
    for path in ["/__notify", "/__reload"] {
        let (status, _) = call(&engine, loopback(), path, vec![], Bytes::new()).await;
        assert_eq!(status, 403, "{path} 应拒绝无 token");
    }
    // 带 token:reload 无配置目录 → 503 no_reload_handler。
    let (status, resp) = call(
        &engine,
        loopback(),
        "/__reload?token=test-token",
        vec![],
        Bytes::new(),
    )
    .await;
    assert_eq!(status, 503);
    assert_eq!(
        serde_json::from_slice::<Value>(&resp).unwrap()["error"],
        "no_reload_handler"
    );

    // notify:记 notify 事件。
    let notify_body =
        Bytes::from(serde_json::to_vec(&json!({"type": "Stop", "message": "done"})).unwrap());
    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token",
        vec![],
        notify_body,
    )
    .await;
    assert_eq!(status, 200);
    let runtime = runtime_of(&engine).await;
    assert!(
        runtime
            .recent_events
            .iter()
            .any(|e| e.kind == "notify" && e.message.as_deref() == Some("done"))
    );
    let notify = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "notify")
        .expect("notify event");
    assert_eq!(notify.phase, Some(RuntimeEventPhase::Completed));

    // status:环回放行,authToken 脱敏。
    let (status, resp) = call(&engine, loopback(), "/__status", vec![], Bytes::new()).await;
    assert_eq!(status, 200);
    let json: Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(json["running"], json!(true));
    assert_eq!(json["endpoints"], json!(2));
    assert_eq!(json["listener"]["authToken"], "***");

    // 非环回无 token:status 403。
    let (status, _) = call(
        &engine,
        Some("10.0.0.9".parse().unwrap()),
        "/__status",
        vec![],
        Bytes::new(),
    )
    .await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn ingress_preflight_never_polls_rejected_or_bodyless_request_bodies() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.listener.auth_token = "secret-inbound".into();
    config.listener.allowed_cidrs = vec!["10.0.0.0/8".into()];
    let engine = engine_with(config, fake);
    let external = Some(IpAddr::from([192, 0, 2, 10]));
    let auth = vec![("x-api-key".to_string(), "secret-inbound".to_string())];

    // 与 Linux 同一契约,但期望值按 macOS 数据面语义:CIDR 校验在路径分发之前,
    // 且 /__notify 在 macOS 是真实控制端点(Linux 恒 404)。
    for (remote, path, headers, expected) in [
        (loopback(), "/v1/messages", vec![], 401),
        (external, "/v1/messages", auth, 403),
        (loopback(), "/unknown", vec![], 404),
        (external, "/__status", vec![], 403),
        (external, "/__notify", vec![], 403),
        (loopback(), "/__status", vec![], 200),
    ] {
        let polled = Arc::new(AtomicBool::new(false));
        let response = engine
            .handle_request(
                remote,
                "POST",
                path,
                headers,
                body_with_poll_flag(polled.clone()),
            )
            .await;
        assert_eq!(response.status().as_u16(), expected, "{path}");
        assert!(!polled.load(Ordering::SeqCst), "{path} 不应读取请求体");
    }
}

#[tokio::test]
async fn rejected_requests_record_client_events() {
    // 【Rust 修正回归】鉴权/解析/规划失败必须在事件表可见(Swift 语义)。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.listener.auth_token = "sk-inbound".into();
    let engine = engine_with(config, fake.clone());

    // 入站鉴权失败 → 401 client 事件。
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 401);

    // 请求体不是 JSON → 400 client 事件。
    let auth = vec![("x-api-key".to_string(), "sk-inbound".to_string())];
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/messages",
        auth.clone(),
        Bytes::from_static(b"not json"),
    )
    .await;
    assert_eq!(status, 400);

    // 规划失败(没有池承接该模型)→ 400 client 事件,带 clientModel。
    let bad_model = Bytes::from(
        serde_json::to_vec(&json!({
            "model": "gpt-9",
            "messages": [{"role": "user", "content": "x"}],
        }))
        .unwrap(),
    );
    let (status, _) = call(&engine, loopback(), "/v1/messages", auth, bad_model).await;
    assert_eq!(status, 400);

    let runtime = runtime_of(&engine).await;
    let messages: Vec<String> = runtime
        .recent_events
        .iter()
        .filter(|e| e.kind == "client")
        .filter_map(|e| e.message.clone())
        .collect();
    assert!(messages.iter().any(|m| m == "inbound_auth_required"));
    assert!(messages.iter().any(|m| m == "body is not JSON"));
    assert!(
        messages
            .iter()
            .any(|m| m.starts_with("no Provider accepts model"))
    );
    assert_eq!(runtime.client_requests, 3);
    assert_eq!(runtime.client_failures, 3);
    assert!(
        runtime
            .recent_events
            .iter()
            .filter(|event| event.kind == "client")
            .all(|event| {
                event.phase == Some(RuntimeEventPhase::Completed)
                    && event.failure_kind == Some(RuntimeFailureKind::ClientRequestRejected)
                    && event.failure_phase == Some(RuntimeFailurePhase::BeforeResponse)
                    && event.failure_detail.as_deref() == event.message.as_deref()
            }),
        "引擎新产出的拒绝事件必须结构化标记拒绝原因并保留原详情"
    );
    let plan_event = runtime
        .recent_events
        .iter()
        .find(|e| {
            e.message
                .as_deref()
                .is_some_and(|m| m.starts_with("no Provider accepts model"))
        })
        .unwrap();
    assert_eq!(plan_event.client_model.as_deref(), Some("gpt-9"));
    assert_messages_in_vocabulary(&runtime);
}

#[tokio::test]
async fn client_declared_project_headers_reach_both_forwarded_and_rejected_events() {
    let fake = FakeTransport::new();
    let declared = vec![
        (
            "X-Sumpter-Project".to_string(),
            "automode-proxy".to_string(),
        ),
        (
            "x-sumpter-workspace".to_string(),
            "/Users/kkl/.claude/automode-proxy".to_string(),
        ),
        (
            "X-Sumpter-Git-Remote".to_string(),
            "https://user:tok@github.com/domoxiaojun/sumpter.git".to_string(),
        ),
    ];

    // 1) 正常转发路径:声明经 ClientMeta 落到完成态 client 事件。
    let engine = engine_with(two_endpoint_config().normalized(), fake.clone());
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            declared.clone(),
            body()
        )
        .await
        .0,
        200
    );
    let runtime = engine.runtime_snapshot();
    let event = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client" && event.status_code == 200)
        .expect("forwarded client event");
    let got = event.client_declared.as_ref().expect("client declared");
    assert_eq!(got.project.as_deref(), Some("automode-proxy"));
    // 出站要剥离,但事件里保留的是脱敏后的尾两段,不是完整本机路径。
    assert_eq!(got.workspace.as_deref(), Some(".../.claude/automode-proxy"));
    assert_eq!(
        got.git_remote.as_deref(),
        Some("https://github.com/domoxiaojun/sumpter.git"),
        "凭据必须在落事件前就被剥掉"
    );
    // 上游没收到这三个 header。
    let upstream = fake.requests();
    let sent = &upstream.first().expect("upstream request").headers;
    assert!(
        !sent.iter().any(|(name, _)| name.starts_with("x-sumpter-")),
        "私有归因 header 泄到上游: {sent:?}"
    );

    // 2) reject 路径:请求还没进转发就被拒,归因同样要在事件里。
    let mut rejecting = two_endpoint_config();
    rejecting.listener.auth_token = "sk-inbound".into();
    let engine = engine_with(rejecting, FakeTransport::new());
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/messages",
        declared,
        Bytes::from_static(b"not-json"),
    )
    .await;
    assert_eq!(status, 401);
    let runtime = engine.runtime_snapshot();
    let event = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client" && event.status_code == 401)
        .expect("rejected client event");
    assert_eq!(
        event
            .client_declared
            .as_ref()
            .and_then(|declared| declared.project.as_deref()),
        Some("automode-proxy")
    );
}

#[tokio::test]
async fn grok_sampling_headers_reach_event_details() {
    let fake = FakeTransport::new();
    let headers = vec![
        ("User-Agent".into(), "grok-shell/0.2.119".into()),
        ("x-grok-session-id".into(), "sess-1".into()),
        ("x-grok-conv-id".into(), "conv-1".into()),
        ("x-grok-req-id".into(), "req-1".into()),
        ("x-grok-agent-id".into(), "agent-1".into()),
        ("x-grok-turn-idx".into(), "3".into()),
        ("x-grok-model-override".into(), "grok-4.6".into()),
        ("x-grok-client-identifier".into(), "grok-shell".into()),
        ("x-grok-client-version".into(), "0.2.119".into()),
        ("x-grok-client-mode".into(), "interactive".into()),
        ("x-compactions-remaining".into(), "1".into()),
        (
            "traceparent".into(),
            "00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-01".into(),
        ),
    ];

    let engine = engine_with(two_endpoint_config().normalized(), fake.clone());
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(&engine, loopback(), "/v1/messages", headers.clone(), body())
            .await
            .0,
        200
    );
    let runtime = engine.runtime_snapshot();
    let event = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client" && event.status_code == 200)
        .expect("forwarded client event");
    assert_eq!(event.client_kind, Some(ClientKind::GrokBuild));
    assert_eq!(event.session_id.as_deref(), Some("sess-1"));
    let grok = event.grok_metadata.as_ref().expect("grok metadata");
    assert_eq!(grok.session_id.as_deref(), Some("sess-1"));
    assert_eq!(grok.conv_id.as_deref(), Some("conv-1"));
    assert_eq!(grok.request_id.as_deref(), Some("req-1"));
    assert_eq!(grok.agent_id.as_deref(), Some("agent-1"));
    assert_eq!(grok.turn_index.as_deref(), Some("3"));
    assert_eq!(grok.model_override.as_deref(), Some("grok-4.6"));
    assert_eq!(grok.client_identifier.as_deref(), Some("grok-shell"));
    assert_eq!(grok.client_version.as_deref(), Some("0.2.119"));
    assert_eq!(grok.client_mode.as_deref(), Some("interactive"));
    assert_eq!(grok.compactions_remaining.as_deref(), Some("1"));
    assert_eq!(grok.user_agent.as_deref(), Some("grok-shell/0.2.119"));
    assert!(
        event.codex_metadata.is_none(),
        "Grok 只有 OTel traceparent 时不应落空 Codex 元数据: {:?}",
        event.codex_metadata
    );
    let upstream = fake.requests();
    let sent = &upstream.first().expect("upstream request").headers;
    assert!(
        sent.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("x-grok-session-id") && value == "sess-1"
        }),
        "x-grok-* 是官方采样 header，出站应保留: {sent:?}"
    );

    let mut rejecting = two_endpoint_config();
    rejecting.listener.auth_token = "sk-inbound".into();
    let engine = engine_with(rejecting, FakeTransport::new());
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/messages",
        headers,
        Bytes::from_static(b"not-json"),
    )
    .await;
    assert_eq!(status, 401);
    let runtime = engine.runtime_snapshot();
    let event = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client" && event.status_code == 401)
        .expect("rejected client event");
    assert_eq!(
        event
            .grok_metadata
            .as_ref()
            .and_then(|metadata| metadata.session_id.as_deref()),
        Some("sess-1")
    );
    assert_eq!(event.session_id.as_deref(), Some("sess-1"));
}

#[tokio::test]
async fn rejected_guardian_preserves_header_identity_and_internal_scope() {
    let engine = engine_with(two_endpoint_config(), FakeTransport::new());
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/responses",
        vec![
            ("originator".into(), "Codex Desktop".into()),
            ("x-openai-subagent".into(), "guardian".into()),
            ("session-id".into(), "guardian-session".into()),
        ],
        Bytes::from_static(b"not json"),
    )
    .await;
    assert_eq!(status, 400);
    let snapshot = runtime_of(&engine).await;
    let event = snapshot
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert_eq!(event.message.as_deref(), Some("body is not JSON"));
    assert_eq!(
        event.client_kind,
        Some(sumpter_core::events::ClientKind::Codex)
    );
    let metadata = event.codex_metadata.as_ref().unwrap();
    assert_eq!(metadata.session_id.as_deref(), Some("guardian-session"));
    assert_eq!(
        sumpter_core::events::codex_thread_class(Some(metadata)).as_str(),
        "guardian_review"
    );
    assert_eq!(
        sumpter_core::events::codex_attribution_scope(Some(metadata), None).as_str(),
        "internal_feature"
    );
    assert!(metadata.workspaces.is_empty());
}

#[tokio::test]
async fn rejected_requests_preserve_codex_metadata() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.listener.auth_token = "sk-inbound".into();
    let engine = engine_with(config, fake);
    let headers = vec![(
        "x-codex-turn-metadata".to_string(),
        r#"{"thread_id":"thread-rejected","turn_id":"turn-rejected","subagent_kind":"thread_spawn"}"#.to_string(),
    )];

    // The body is intentionally invalid: header-only metadata must still be
    // attached to the rejected client event.
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/responses",
        headers,
        Bytes::from_static(b"not json"),
    )
    .await;
    assert_eq!(status, 401);

    let runtime = runtime_of(&engine).await;
    let event = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client" && event.status_code == 401)
        .expect("rejected client event");
    let metadata = event.codex_metadata.as_ref().expect("Codex metadata");
    assert_eq!(metadata.thread_id.as_deref(), Some("thread-rejected"));
    assert_eq!(metadata.turn_id.as_deref(), Some("turn-rejected"));
    assert_eq!(metadata.subagent_kind.as_deref(), Some("thread_spawn"));
}

#[tokio::test]
async fn bridge_and_deferred_round_tokens() {
    // 桥接成功:client/upstream 消息都带 `bridge openai`。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::OpenAI;
    config.endpoints[0].mappings.push(ModelMapping {
        client_pattern: "claude-opus-5".into(),
        context: ContextMode::Standard,
        failover_timeout_seconds: None,
        thinking: ThinkingMode::Disabled,
        effort: None,
        upstream_model: "gpt-up".into(),
        capabilities: Vec::new(),
    });
    config.endpoints[1].enabled = false;
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push(
        "a.example.com",
        sse_ok(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ]),
    );
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert_eq!(client.message.as_deref(), Some("bridge openai"));
    let upstream = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "upstream")
        .unwrap();
    assert_eq!(upstream.message.as_deref(), Some("bridge openai"));
    assert_messages_in_vocabulary(&runtime);

    // 跨轮第二轮恢复:兼容 token 仍为 `deferred_rounds 2`。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 2;
    let engine = engine_with(config, fake.clone());
    for host in ["a.example.com", "b.example.com"] {
        fake.push(
            host,
            Outcome::Status {
                status: 503,
                headers: vec![],
                chunks: vec![],
            },
        );
    }
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert_eq!(client.message.as_deref(), Some("deferred_rounds 2"));
    assert_messages_in_vocabulary(&runtime);
}

#[tokio::test]
async fn notify_enriches_hook_payloads() {
    use sumpter_engine::PlatformNotice;
    use sumpter_macos_adapter::engine::EngineNotice;

    async fn next_notify(
        rx: &mut tokio::sync::broadcast::Receiver<EngineNotice>,
    ) -> (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        loop {
            match rx.recv().await.unwrap() {
                EngineNotice::PlatformNotice(PlatformNotice::Notify {
                    title,
                    message,
                    kind,
                    session_id,
                    cwd,
                    ..
                }) => return (title, message, Some(kind), session_id, cwd),
                _ => continue,
            }
        }
    }

    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake);
    let mut rx = engine.subscribe();

    // Notification:CC 原话 message + session/cwd 透传(hook 脚本 v2 转发 stdin)。
    let payload = json!({
        "session_id": "sess-1",
        "transcript_path": "/tmp/t.jsonl",
        "cwd": "/Users/kkl/projects/sumpter",
        "hook_event_name": "Notification",
        "message": "Claude needs your permission to use Bash",
    });
    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&event=notification",
        vec![],
        Bytes::from(serde_json::to_vec(&payload).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    let (title, message, kind, session_id, cwd) = next_notify(&mut rx).await;
    assert_eq!(title, "Claude Code · 等待确认");
    assert_eq!(message, "Claude needs your permission to use Bash");
    assert_eq!(kind.as_deref(), Some("notification"));
    assert_eq!(session_id.as_deref(), Some("sess-1"));
    assert_eq!(cwd.as_deref(), Some("/Users/kkl/projects/sumpter"));

    // Stop:无 message → cwd 项目名兜底;transcript_path 不再被当成消息弹出。
    let payload = json!({
        "session_id": "sess-2",
        "transcript_path": "/tmp/t2.jsonl",
        "cwd": "/work/automode-proxy",
        "hook_event_name": "Stop",
        "stop_hook_active": false,
    });
    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&event=stop",
        vec![],
        Bytes::from(serde_json::to_vec(&payload).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    let (title, message, kind, _, _) = next_notify(&mut rx).await;
    assert_eq!(title, "Claude Code · 回合结束");
    assert_eq!(message, "「automode-proxy」的回合已结束");
    assert_eq!(kind.as_deref(), Some("stop"));

    // 老脚本兼容:message=事件名视同缺失走富化;通用标题 "Claude Code" 让位。
    let payload = json!({"type": "stop", "title": "Claude Code", "message": "stop"});
    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token",
        vec![],
        Bytes::from(serde_json::to_vec(&payload).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    let (title, message, kind, session_id, _) = next_notify(&mut rx).await;
    assert_eq!(title, "Claude Code · 回合结束");
    assert_eq!(message, "对话回合已结束");
    assert_eq!(kind.as_deref(), Some("stop"));
    assert_eq!(session_id, None);

    // runtime notify 事件的 message 记富化后的人话(事件表可读)。
    let runtime = runtime_of(&engine).await;
    assert!(runtime.recent_events.iter().any(|e| e.kind == "notify"
        && e.hook_event.as_deref() == Some("Stop")
        && e.message.as_deref() == Some("「automode-proxy」的回合已结束")));
    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token",
        vec![],
        Bytes::from(serde_json::to_vec(&json!({"session_id":"hook-not-provided"})).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    let runtime = runtime_of(&engine).await;
    let untyped = runtime
        .recent_events
        .iter()
        .find(|event| event.session_id.as_deref() == Some("hook-not-provided"))
        .unwrap();
    assert_eq!(untyped.hook_event, None);
}

#[tokio::test]
async fn codex_stop_notify_is_source_tagged_and_payload_safe() {
    use sumpter_core::events::ClientKind;
    use sumpter_engine::PlatformNotice;
    use sumpter_macos_adapter::engine::EngineNotice;
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake);
    let mut rx = engine.subscribe();
    let payload = json!({
        "session_id": "codex-session",
        "cwd": "/Users/kkl/projects/private-work",
        "hook_event_name": "Stop",
        "last_assistant_message": "PRIVATE TRANSCRIPT MUST NOT ESCAPE",
        "message": "PRIVATE PROMPT MUST NOT ESCAPE",
        "error": "raw-error-details",
    });
    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&event=stop&clientKind=codex",
        vec![],
        Bytes::from(serde_json::to_vec(&payload).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    let notice = loop {
        match rx.recv().await.unwrap() {
            EngineNotice::PlatformNotice(PlatformNotice::Notify {
                client_kind,
                title,
                message,
                kind,
                cwd,
                ..
            }) => break (client_kind, title, message, kind, cwd),
            _ => continue,
        }
    };
    assert_eq!(notice.0, ClientKind::Codex);
    assert_eq!(notice.1, "Codex CLI · 回合完成");
    assert_eq!(notice.2, "Codex CLI 主回合已完成");
    assert_eq!(notice.3, "stop");
    assert_eq!(notice.4, None);
    assert!(!notice.2.contains("PRIVATE"));

    let runtime = runtime_of(&engine).await;
    let event = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "notify")
        .expect("Codex notify runtime event");
    assert_eq!(event.client_kind, Some(ClientKind::Codex));
    assert_eq!(event.message.as_deref(), Some("Codex CLI 主回合已完成"));

    async fn next_platform_notice(rx: &mut tokio::sync::broadcast::Receiver<EngineNotice>) {
        loop {
            if matches!(
                rx.recv().await.unwrap(),
                EngineNotice::PlatformNotice(PlatformNotice::Notify { .. })
            ) {
                return;
            }
        }
    }

    let round = json!({
        "session_id": "codex-session",
        "turn_id": "turn-1",
        "hook_event_name": "Stop",
    });
    let (_, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&event=stop&clientKind=codex",
        vec![],
        Bytes::from(serde_json::to_vec(&round).unwrap()),
    )
    .await;
    next_platform_notice(&mut rx).await;
    let (_, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&event=stop&clientKind=codex",
        vec![],
        Bytes::from(serde_json::to_vec(&round).unwrap()),
    )
    .await;
    let duplicate = loop {
        match rx.try_recv() {
            Ok(EngineNotice::PlatformNotice(PlatformNotice::Notify { .. })) => break true,
            Ok(_) => continue,
            Err(_) => break false,
        }
    };
    assert!(!duplicate);
    let next_round = json!({
        "session_id": "codex-session",
        "turn_id": "turn-2",
        "hook_event_name": "Stop",
    });
    let (_, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&event=stop&clientKind=codex",
        vec![],
        Bytes::from(serde_json::to_vec(&next_round).unwrap()),
    )
    .await;
    next_platform_notice(&mut rx).await;
}

#[tokio::test]
async fn codex_notification_events_share_categories_and_stay_payload_safe() {
    use sumpter_engine::PlatformNotice;
    use sumpter_macos_adapter::engine::EngineNotice;

    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake);
    let mut rx = engine.subscribe();

    let cases = [
        (
            "PermissionRequest",
            "action_required",
            "Codex CLI · 需要你处理",
            "Codex CLI 正在等待你的授权决定",
        ),
        (
            "SubagentStop",
            "subtask_completed",
            "Codex CLI · 子任务结束",
            "Codex CLI 子任务已完成",
        ),
        (
            "Interrupt",
            "turn_failed",
            "Codex CLI · 回合中断",
            "Codex CLI 回合被中断，未完成",
        ),
    ];
    for (index, (event, category, title, message)) in cases.into_iter().enumerate() {
        let payload = json!({
            "session_id": format!("codex-session-{index}"),
            "hook_event_name": event,
            "tool_name": "Bash",
            "tool_input": {"description": "PRIVATE INPUT MUST NOT ESCAPE"},
            "last_assistant_message": "PRIVATE TRANSCRIPT MUST NOT ESCAPE",
            "error": "PRIVATE ERROR MUST NOT ESCAPE",
        });
        let (status, _) = call(
            &engine,
            loopback(),
            "/__notify?token=test-token&clientKind=codex",
            vec![],
            Bytes::from(serde_json::to_vec(&payload).unwrap()),
        )
        .await;
        assert_eq!(status, 200);
        let notice = loop {
            match rx.recv().await.unwrap() {
                EngineNotice::PlatformNotice(PlatformNotice::Notify {
                    title: actual_title,
                    message: actual_message,
                    category: Some(actual_category),
                    cwd,
                    ..
                }) => break (actual_title, actual_message, actual_category, cwd),
                _ => continue,
            }
        };
        assert_eq!(notice.0, title);
        assert_eq!(notice.1, message);
        assert_eq!(notice.2, category);
        assert_eq!(notice.3, None);
        assert!(!notice.1.contains("PRIVATE"));
    }

    // An unsupported event is acknowledged but never recorded or displayed.
    let notify_count_before = runtime_of(&engine)
        .await
        .recent_events
        .iter()
        .filter(|event| event.kind == "notify")
        .count();
    let payload = json!({"hook_event_name": "SessionStart"});
    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&clientKind=codex",
        vec![],
        Bytes::from(serde_json::to_vec(&payload).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    assert!(rx.try_recv().is_err());
    let runtime = runtime_of(&engine).await;
    assert_eq!(
        runtime
            .recent_events
            .iter()
            .filter(|event| event.kind == "notify")
            .count(),
        notify_count_before
    );
}

#[tokio::test]
async fn grok_notification_events_share_categories_and_stay_payload_safe() {
    use sumpter_core::events::ClientKind;
    use sumpter_engine::PlatformNotice;
    use sumpter_macos_adapter::engine::EngineNotice;

    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake);
    let mut rx = engine.subscribe();

    async fn next_notify(
        rx: &mut tokio::sync::broadcast::Receiver<EngineNotice>,
    ) -> (
        ClientKind,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
    ) {
        loop {
            match rx.recv().await.unwrap() {
                EngineNotice::PlatformNotice(PlatformNotice::Notify {
                    client_kind,
                    title,
                    message,
                    kind,
                    category,
                    cwd,
                    ..
                }) => break (client_kind, title, message, kind, category, cwd),
                _ => continue,
            }
        }
    }

    let cases = [
        (
            json!({
                "hookEventName": "Stop",
                "sessionId": "grok-session-stop",
                "promptId": "prompt-1",
                "reason": "end_turn",
                "cwd": "/Users/kkl/projects/private-work",
                "lastAssistantMessage": "PRIVATE TRANSCRIPT MUST NOT ESCAPE",
                "message": "PRIVATE PROMPT MUST NOT ESCAPE",
            }),
            "stop",
            "turn_completed",
            "Grok Build · 回合完成",
            "Grok Build 主回合已完成",
        ),
        (
            json!({
                "hookEventName": "notification",
                "sessionId": "grok-session-permission",
                "notificationType": "permission_prompt",
                "message": "PRIVATE PERMISSION TEXT MUST NOT ESCAPE",
                "toolInput": {"command": "PRIVATE"},
            }),
            "notification",
            "action_required",
            "Grok Build · 需要你处理",
            "Grok Build 正在等待你的授权或确认",
        ),
        (
            json!({
                "hookEventName": "Notification",
                "sessionId": "grok-session-idle",
                "notificationType": "idle_prompt",
                "message": "PRIVATE IDLE TEXT MUST NOT ESCAPE",
            }),
            "notification",
            "action_required",
            "Grok Build · 等待继续",
            "Grok Build 正在等待你继续输入",
        ),
        (
            json!({
                "hookEventName": "notification",
                "sessionId": "grok-session-task",
                "notificationType": "task_complete",
                "message": "PRIVATE TASK TEXT MUST NOT ESCAPE",
            }),
            "notification",
            "subtask_completed",
            "Grok Build · 任务完成",
            "Grok Build 后台任务已完成",
        ),
        (
            json!({
                "hookEventName": "StopCancelled",
                "sessionId": "grok-session-cancel",
                "reason": "user_interrupt",
                "lastAssistantMessage": "PRIVATE TRANSCRIPT MUST NOT ESCAPE",
            }),
            "stop_cancelled",
            "turn_failed",
            "Grok Build · 回合中断",
            "Grok Build 回合被中断，未完成",
        ),
        (
            json!({
                "hookEventName": "SubagentStop",
                "sessionId": "grok-session-sub",
                "subagentType": "explore",
                "lastAssistantMessage": "PRIVATE TRANSCRIPT MUST NOT ESCAPE",
            }),
            "subagent_stop",
            "subtask_completed",
            "Grok Build · 子任务结束",
            "Grok Build 子任务已完成",
        ),
        (
            json!({
                "hookEventName": "StopFailure",
                "sessionId": "grok-session-fail",
                "error": "rate_limit",
                "errorDetails": "PRIVATE ERROR DETAILS MUST NOT ESCAPE",
            }),
            "stop_failure",
            "turn_failed",
            "Grok Build · 回合异常",
            "上游限流，回合未完成",
        ),
    ];
    for (payload, kind, category, title, message) in cases {
        let (status, _) = call(
            &engine,
            loopback(),
            "/__notify?token=test-token&clientKind=grok_build",
            vec![],
            Bytes::from(serde_json::to_vec(&payload).unwrap()),
        )
        .await;
        assert_eq!(status, 200);
        let notice = next_notify(&mut rx).await;
        assert_eq!(notice.0, ClientKind::GrokBuild);
        assert_eq!(notice.3, kind);
        assert_eq!(notice.4.as_deref(), Some(category));
        assert_eq!(notice.1, title);
        assert_eq!(notice.2, message);
        assert!(!notice.2.contains("PRIVATE"));
        if kind == "stop" {
            assert_eq!(
                notice.5.as_deref(),
                Some("/Users/kkl/projects/private-work")
            );
        }
    }

    let notify_count_before = runtime_of(&engine)
        .await
        .recent_events
        .iter()
        .filter(|event| event.kind == "notify")
        .count();

    // Session-end Stop is observe-only and must not look like a finished turn.
    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&clientKind=grok_build",
        vec![],
        Bytes::from(
            serde_json::to_vec(&json!({
                "hookEventName": "stop",
                "sessionId": "grok-session-end",
                "reason": "channel_closed",
            }))
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert!(rx.try_recv().is_err());

    // Tool/session hooks stay out of the notification stream.
    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&clientKind=grok_build",
        vec![],
        Bytes::from(serde_json::to_vec(&json!({"hookEventName": "SessionStart"})).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    assert!(rx.try_recv().is_err());

    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&clientKind=grok_build",
        vec![],
        Bytes::from(
            serde_json::to_vec(&json!({
                "hookEventName": "notification",
                "notificationType": "auth_success",
                "message": "PRIVATE",
            }))
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, 200);
    assert!(rx.try_recv().is_err());

    let runtime = runtime_of(&engine).await;
    assert_eq!(
        runtime
            .recent_events
            .iter()
            .filter(|event| event.kind == "notify")
            .count(),
        notify_count_before
    );

    // Duplicate Stop for the same prompt is collapsed; a new prompt still fires.
    let round = json!({
        "sessionId": "grok-session-dedup",
        "promptId": "prompt-repeat",
        "hookEventName": "Stop",
        "reason": "end_turn",
    });
    let (_, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&clientKind=grok_build",
        vec![],
        Bytes::from(serde_json::to_vec(&round).unwrap()),
    )
    .await;
    let _ = next_notify(&mut rx).await;
    let (_, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&clientKind=grok_build",
        vec![],
        Bytes::from(serde_json::to_vec(&round).unwrap()),
    )
    .await;
    let duplicate = loop {
        match rx.try_recv() {
            Ok(EngineNotice::PlatformNotice(PlatformNotice::Notify { .. })) => break true,
            Ok(_) => continue,
            Err(_) => break false,
        }
    };
    assert!(!duplicate);
}

#[tokio::test]
async fn notify_classifies_client_actions_and_suppresses_duplicate_hooks() {
    use sumpter_macos_adapter::engine::EngineNotice;

    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake);
    let mut rx = engine.subscribe();

    let action = json!({
        "session_id": "sess-action",
        "cwd": "/work/client-notifications",
        "hook_event_name": "Notification",
        "notification_type": "permission_prompt",
        "action_id": "permission-1",
        "message": "Claude needs permission",
    });
    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&event=notification",
        vec![],
        Bytes::from(serde_json::to_vec(&action).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    let first = loop {
        match rx.recv().await.unwrap() {
            EngineNotice::PlatformNotice(PlatformNotice::Notify {
                category,
                priority,
                action_id,
                message,
                ..
            }) => break (category, priority, action_id, message),
            _ => continue,
        }
    };
    assert_eq!(first.0.as_deref(), Some("action_required"));
    assert_eq!(first.1.as_deref(), Some("high"));
    assert_eq!(first.2.as_deref(), Some("permission-1"));
    assert_eq!(first.3, "Claude needs permission");

    // RuntimeEvent 仍会记录第二次 hook，但系统通知不应重复发出。
    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&event=notification",
        vec![],
        Bytes::from(serde_json::to_vec(&action).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    let mut duplicate_notify = false;
    while let Ok(notice) = rx.try_recv() {
        duplicate_notify |= matches!(
            notice,
            EngineNotice::PlatformNotice(PlatformNotice::Notify { .. })
        );
    }
    assert!(!duplicate_notify);

    let status_payload = json!({
        "session_id": "sess-action",
        "hook_event_name": "Notification",
        "notification_type": "auth_success",
    });
    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&event=notification",
        vec![],
        Bytes::from(serde_json::to_vec(&status_payload).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    let status_notice = loop {
        match rx.recv().await.unwrap() {
            EngineNotice::PlatformNotice(PlatformNotice::Notify {
                category, priority, ..
            }) => break (category, priority),
            _ => continue,
        }
    };
    assert_eq!(status_notice.0.as_deref(), Some("status"));
    assert_eq!(status_notice.1.as_deref(), Some("low"));
}

#[tokio::test]
async fn stop_failure_uses_safe_error_summary_without_inventing_http_status() {
    use sumpter_macos_adapter::engine::EngineNotice;

    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake);
    let mut rx = engine.subscribe();
    let payload = json!({
        "session_id": "sess-failure",
        "cwd": "/work/client-notifications",
        "hook_event_name": "StopFailure",
        "error": "server_error",
        "title": "raw title must not escape classification",
        "error_details": "raw upstream detail must not be displayed",
        "last_assistant_message": "private response text",
    });
    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&event=StopFailure",
        vec![],
        Bytes::from(serde_json::to_vec(&payload).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    let notice = loop {
        match rx.recv().await.unwrap() {
            EngineNotice::PlatformNotice(PlatformNotice::Notify {
                category,
                priority,
                title,
                message,
                ..
            }) => break (category, priority, title, message),
            _ => continue,
        }
    };
    assert_eq!(notice.0.as_deref(), Some("turn_failed"));
    assert_eq!(notice.1.as_deref(), Some("high"));
    assert_eq!(notice.2, "Claude Code · 回合异常");
    assert_eq!(notice.3, "上游服务异常，回合未完成");
    assert!(!notice.3.contains("HTTP"));
    assert!(!notice.3.contains("raw upstream"));
    assert!(!notice.3.contains("private response"));
}

#[tokio::test]
async fn stop_failure_adds_http_status_only_from_matching_claude_runtime_event() {
    use sumpter_macos_adapter::engine::EngineNotice;

    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[1].enabled = false;
    config.retry.max_deferred_rounds = 1;
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 502,
            headers: vec![],
            chunks: vec![],
        },
    );
    let headers = vec![
        ("user-agent".to_string(), "claude-cli/2.1.220".to_string()),
        (
            "x-claude-code-session-id".to_string(),
            "sess-http".to_string(),
        ),
    ];
    let (status, _) = call(&engine, loopback(), "/v1/messages", headers, body()).await;
    assert_eq!(status, 502);

    let mut rx = engine.subscribe();
    let payload = json!({
        "session_id": "sess-http",
        "hook_event_name": "StopFailure",
        "error": "server_error",
    });
    let (status, _) = call(
        &engine,
        loopback(),
        "/__notify?token=test-token&event=StopFailure",
        vec![],
        Bytes::from(serde_json::to_vec(&payload).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    let message = loop {
        match rx.recv().await.unwrap() {
            EngineNotice::PlatformNotice(PlatformNotice::Notify { message, .. }) => break message,
            _ => continue,
        }
    };
    assert_eq!(message, "上游返回错误状态，回合未完成（上游返回 HTTP 502）");
}

// ---------------------------------------------------------------------------
// 分类器 / WebFetch / WebSearch 指纹 × 协议覆盖(feature rule target.protocol)
// ---------------------------------------------------------------------------

/// CC 2.1.x 安全分类器指纹(XML 两阶段 stage-1:无 tools + stop_sequences)。
fn classifier_body(model: &str) -> Bytes {
    Bytes::from(
        serde_json::to_vec(&json!({
            "model": model,
            "max_tokens": 512,
            "stream": true,
            "system": "You are a security monitor for autonomous AI coding agents. Analyze the transcript and assess the pending command.",
            "messages": [{"role": "user", "content": "<transcript>\nAssistant wants to run: ls -la /tmp\n</transcript>\nAssess the pending command."}],
            "stop_sequences": ["</severity>"],
        }))
        .unwrap(),
    )
}

/// WebFetch 指纹:无 tools(所以可以过 openai 系桥)。
fn webfetch_body(model: &str) -> Bytes {
    Bytes::from(
        serde_json::to_vec(&json!({
            "model": model,
            "max_tokens": 512,
            "stream": true,
            "system": "You are Claude Code, Anthropic's official CLI for Claude.",
            "messages": [{"role": "user", "content": "Web page content:\n---\nSome docs body\n---\nProvide a concise response based on the content above. Include relevant details, code examples, and documentation excerpts as needed."}],
        }))
        .unwrap(),
    )
}

/// WebSearch 指纹：必带 web_search 工具和强制 tool_choice；最终按 TargetFormat 适配。
fn websearch_body(model: &str) -> Bytes {
    Bytes::from(
        serde_json::to_vec(&json!({
            "model": model,
            "max_tokens": 512,
            "stream": true,
            "system": "You are an assistant for performing a web search tool use",
            "messages": [{"role": "user", "content": "Perform a web search for the query: rust tokio"}],
            "tools": [{"name": "web_search", "type": "web_search_20250305", "max_uses": 8}],
            "tool_choice": {"type": "tool", "name": "web_search"},
        }))
        .unwrap(),
    )
}

fn rule(
    id: &str,
    kind: RequestKind,
    model: &str,
    protocol: Option<ProviderProtocol>,
) -> FeatureRule {
    FeatureRule {
        enabled: true,
        id: id.into(),
        match_: FeatureRuleMatch {
            request_kind: Some(kind),
            ..Default::default()
        },
        name: id.into(),
        target: FeatureRuleTarget {
            endpoint_id: Some("a".into()),
            effort: None,
            model: model.into(),
            protocol_override: protocol,
        },
    }
}

#[tokio::test]
async fn classifier_rule_bridges_via_openai_protocol_override() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    config.endpoints[1].enabled = false;
    config.feature_rules = vec![rule(
        "classifier",
        RequestKind::Classifier,
        "qwen-cls",
        Some(ProviderProtocol::OpenAI),
    )];
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push(
        "a.example.com",
        sse_ok(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"<block>false</block>\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ]),
    );

    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/messages",
            vec![],
            classifier_body("claude-haiku-4-5-20251001"),
        )
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream; charset=utf-8"
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("message_start"));
    assert!(text.contains("<block>false</block>"));

    // 出站:openai 桥 → /v1/chat/completions,模型被规则改写,stop_sequences 透传为 stop。
    let recorded = fake.requests();
    assert_eq!(recorded[0].path, "/v1/chat/completions");
    let sent: Value = serde_json::from_slice(&recorded[0].body).unwrap();
    assert_eq!(sent["model"], "qwen-cls");
    assert_eq!(sent["stop"], json!(["</severity>"]));
    // 声明过 stop:finish_reason=stop 回映射 stop_sequence。
    assert!(text.contains("\"stop_reason\":\"stop_sequence\""));

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert_eq!(
        client.request_purpose,
        Some(sumpter_core::routing::RequestPurpose::Classifier)
    );
    assert_eq!(client.feature_rule_id.as_deref(), Some("classifier"));
    assert_eq!(client.message.as_deref(), Some("bridge openai"));
    let upstream = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "upstream")
        .expect("upstream 事件");
    assert_eq!(
        upstream.client_model.as_deref(),
        Some("claude-haiku-4-5-20251001")
    );
    assert_eq!(upstream.effective_model.as_deref(), Some("qwen-cls"));
    assert_eq!(upstream.feature_rule_id.as_deref(), Some("classifier"));
    assert_messages_in_vocabulary(&runtime);
}

#[tokio::test]
async fn classifier_rule_rejects_responses_when_stop_sequences_cannot_translate() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    config.endpoints[1].enabled = false;
    config.feature_rules = vec![rule(
        "classifier",
        RequestKind::Classifier,
        "qwen-cls",
        Some(ProviderProtocol::OpenAIResponses),
    )];
    let engine = engine_with(config.normalized(), fake.clone());
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/messages",
            vec![],
            classifier_body("claude-haiku-4-5-20251001"),
        )
        .await;
    assert_eq!(response.status(), 400);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["error"], "no_compatible_protocol");
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn webfetch_and_websearch_rules_bridge_via_openai() {
    // WebFetch 指纹无 tools → openai 桥可用。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    config.endpoints[1].enabled = false;
    config.feature_rules = vec![rule(
        "webfetch",
        RequestKind::WebFetch,
        "qwen-wf",
        Some(ProviderProtocol::OpenAI),
    )];
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push(
        "a.example.com",
        sse_ok(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"summary\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ]),
    );
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![],
        webfetch_body("claude-opus-5"),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(fake.requests()[0].path, "/v1/chat/completions");
    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert_eq!(
        client.request_purpose,
        Some(sumpter_core::routing::RequestPurpose::WebFetch)
    );
    assert_eq!(client.feature_rule_id.as_deref(), Some("webfetch"));

    // WebSearch 按最终 TargetFormat 自动注入 OpenAI Chat 的服务端搜索参数。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    config.endpoints[1].enabled = false;
    config.feature_rules = vec![rule(
        "websearch",
        RequestKind::WebSearch,
        "qwen-ws",
        Some(ProviderProtocol::OpenAI),
    )];
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push(
        "a.example.com",
        sse_ok(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"search result\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ]),
    );
    let (status, resp) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![],
        websearch_body("claude-opus-5"),
    )
    .await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&resp).contains("search result"));
    let requests = fake.requests();
    assert_eq!(requests[0].path, "/v1/chat/completions");
    let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(sent["web_search_options"], json!({}));
    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert_eq!(
        client.request_purpose,
        Some(sumpter_core::routing::RequestPurpose::WebSearch)
    );
    assert_messages_in_vocabulary(&runtime);
}

#[tokio::test]
async fn websearch_target_protocol_bridges_and_injects_upstream_search() {
    // Chat 目标协议按 WebSearch 用途进入桥接，出站体注入标准
    // web_search_options；不再需要 Provider 额外能力声明。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    config.endpoints[1].enabled = false;
    config.feature_rules = vec![rule(
        "websearch",
        RequestKind::WebSearch,
        "gpt-search",
        Some(ProviderProtocol::OpenAI),
    )];
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push(
        "a.example.com",
        sse_ok(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"result with link\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ]),
    );
    let (status, resp) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![],
        websearch_body("claude-opus-5"),
    )
    .await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&resp).contains("result with link"));
    let sent: Value = serde_json::from_slice(&fake.requests()[0].body).unwrap();
    assert_eq!(sent["web_search_options"], json!({}));
    assert_eq!(sent["model"], "gpt-search");
    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert_eq!(
        client.request_purpose,
        Some(sumpter_core::routing::RequestPurpose::WebSearch)
    );
    assert_eq!(client.message.as_deref(), Some("bridge openai"));
    assert_messages_in_vocabulary(&runtime);

    // responses 方言:注入内建 web_search 工具,web_search_call 合成结果块。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    config.endpoints[1].enabled = false;
    config.feature_rules = vec![rule(
        "websearch",
        RequestKind::WebSearch,
        "gpt-search",
        Some(ProviderProtocol::OpenAIResponses),
    )];
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push(
        "a.example.com",
        sse_ok(&[
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"web_search_call\",\"id\":\"ws_9\",\"action\":{\"type\":\"search\",\"query\":\"rust tokio\"}}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"searched answer\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"output_tokens\":2}}}\n\n",
        ]),
    );
    let (status, resp) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![],
        websearch_body("claude-opus-5"),
    )
    .await;
    assert_eq!(status, 200);
    let text = String::from_utf8_lossy(&resp);
    assert!(text.contains("server_tool_use"));
    assert!(text.contains("web_search_tool_result"));
    assert!(text.contains("searched answer"));
    let sent: Value = serde_json::from_slice(&fake.requests()[0].body).unwrap();
    assert_eq!(sent["tools"], json!([{"type": "web_search"}]));
}

#[tokio::test]
async fn grok_websearch_rejects_non_responses_protocol_override() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    config.endpoints[1].enabled = false;
    config.feature_rules = vec![rule(
        "websearch",
        RequestKind::WebSearch,
        "grok-4.5",
        Some(ProviderProtocol::Anthropic),
    )];
    let engine = engine_with(config.normalized(), fake.clone());
    let (status, response) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![],
        websearch_body("claude-opus-5"),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(fake.requests().len(), 0);
    let json: Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(json["error"], "route_planning");
}

#[tokio::test]
async fn grok_server_retrieval_cross_protocol_engine_matrix() {
    for protocol in [ProviderProtocol::OpenAI, ProviderProtocol::OpenAIResponses] {
        let fake = FakeTransport::new();
        let mut config = two_endpoint_config();
        config.endpoints[0].protocol = EndpointProtocolMode::Auto;
        config.endpoints[1].enabled = false;
        config.feature_rules = vec![rule(
            "websearch",
            RequestKind::WebSearch,
            "grok-4.5",
            Some(protocol),
        )];
        let engine = engine_with(config.normalized(), fake.clone());
        if protocol == ProviderProtocol::OpenAIResponses {
            fake.push("a.example.com", sse_ok(&[
                "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"web_search_call\",\"id\":\"ws_1\",\"action\":{\"type\":\"search\",\"query\":\"x post\"}}}\n\n",
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"searched\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
            ]));
        }
        let (status, response) = call(
            &engine,
            loopback(),
            "/v1/messages",
            vec![],
            websearch_body("claude-opus-5"),
        )
        .await;
        match protocol {
            ProviderProtocol::OpenAI => {
                assert_eq!(status, 400);
                assert_eq!(fake.requests().len(), 0);
                let json: Value = serde_json::from_slice(&response).unwrap();
                assert_eq!(json["error"], "route_planning");
            }
            ProviderProtocol::OpenAIResponses => {
                assert_eq!(status, 200);
                assert!(String::from_utf8_lossy(&response).contains("searched"));
                let request = &fake.requests()[0];
                let sent: Value = serde_json::from_slice(&request.body).unwrap();
                assert_eq!(request.path, "/v1/responses");
                assert_eq!(sent["model"], "grok-4.5");
                assert_eq!(sent["tools"], json!([{"type": "web_search"}]));
                assert!(String::from_utf8_lossy(&response).contains("web_search_tool_result"));
            }
            ProviderProtocol::Anthropic => unreachable!(),
            ProviderProtocol::Gemini => unreachable!(),
        }
    }

    for protocol in [
        ProviderProtocol::Anthropic,
        ProviderProtocol::OpenAI,
        ProviderProtocol::OpenAIResponses,
    ] {
        let fake = FakeTransport::new();
        let mut config = two_endpoint_config();
        config.endpoints[0].protocol = EndpointProtocolMode::Auto;
        config.endpoints[1].enabled = false;
        config.feature_rules = vec![rule(
            "webfetch",
            RequestKind::WebFetch,
            "grok-4.5",
            Some(protocol),
        )];
        let engine = engine_with(config.normalized(), fake.clone());
        if protocol == ProviderProtocol::OpenAIResponses {
            fake.push("a.example.com", sse_ok(&[
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"fetched\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
            ]));
        }
        let body = Bytes::from(
            serde_json::to_vec(&json!({
                "model": "claude-opus-5",
                "max_tokens": 512,
                "stream": true,
                "system": "You are Claude Code, Anthropic's official CLI for Claude.",
                "messages": [{"role": "user", "content": "Web page content:\n---\nTarget URL: https://x.com/example/status/1\nFetch restricted.\n---\nProvide a concise response based on the content above. Include relevant details, code examples, and documentation excerpts as needed."}],
            }))
            .unwrap(),
        );
        let (status, response) = call(&engine, loopback(), "/v1/messages", vec![], body).await;
        match protocol {
            ProviderProtocol::Anthropic => {
                assert_eq!(status, 400);
                assert!(fake.requests().is_empty());
            }
            ProviderProtocol::OpenAI => {
                assert_eq!(status, 400);
                assert!(fake.requests().is_empty());
            }
            ProviderProtocol::OpenAIResponses => {
                assert_eq!(status, 200);
                let sent: Value = serde_json::from_slice(&fake.requests()[0].body).unwrap();
                assert_eq!(sent["tools"], json!([{"type": "web_search"}]));
            }
            ProviderProtocol::Gemini => unreachable!(),
        }
        if protocol != ProviderProtocol::OpenAIResponses {
            let json: Value = serde_json::from_slice(&response).unwrap();
            assert_eq!(json["error"], "route_planning");
        }
    }
}

#[tokio::test]
async fn unknown_path_404_shape() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake);
    let (status, resp) = call(&engine, loopback(), "/v1/unknown", vec![], Bytes::new()).await;
    assert_eq!(status, 404);
    let json: Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(json["error"], "not_found");
    assert_eq!(json["path"], "/v1/unknown");

    // The listener-download helper is intentionally Linux-only; macOS embeds
    // the script in the App and must not expose a remote copy endpoint.
    let response = engine
        .handle_request(
            loopback(),
            "GET",
            "/__sumpter/cc-project-attribution.sh",
            vec![],
            Bytes::new(),
        )
        .await;
    assert_eq!(response.status(), 404);
}

async fn call_get(
    engine: &Engine,
    remote: Option<IpAddr>,
    path: &str,
    headers: Vec<(String, String)>,
) -> (u16, Vec<u8>) {
    let response = engine
        .handle_request(remote, "GET", path, headers, Bytes::new())
        .await;
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    (status, bytes.to_vec())
}

#[tokio::test]
async fn models_catalog_is_local_and_does_not_hit_upstream() {
    let fake = FakeTransport::new();
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![],
            chunks: vec![br#"{"object":"list","data":[]}"#.to_vec()],
        },
    );
    let mut config = two_endpoint_config();
    // Wildcard mappings are directory patterns; only concrete models observed
    // in an endpoint catalog belong in the local `/v1/models` response.
    for endpoint in &mut config.endpoints {
        endpoint.catalog = Some(EndpointCatalog {
            models: vec!["claude-opus-5".into()],
            ..EndpointCatalog::default()
        });
    }
    let engine = engine_with(config, fake.clone());
    let (status, resp) = call_get(&engine, loopback(), "/v1/models", vec![]).await;
    assert_eq!(status, 200);
    let json: Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(json["object"], "list");
    assert!(
        json["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["id"] == "claude-opus-5")
    );
    assert!(
        fake.requests().is_empty(),
        "catalog must not relay upstream"
    );

    let (status, resp) = call_get(
        &engine,
        loopback(),
        "/v1/models?client_version=0.149.1",
        vec![],
    )
    .await;
    assert_eq!(status, 200);
    let json: Value = serde_json::from_slice(&resp).unwrap();
    assert!(json.get("data").is_none());
    assert!(
        json["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["slug"] == "claude-opus-5")
    );
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn models_catalog_requires_inbound_auth_when_configured() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.listener.auth_token = "listener-secret".into();
    let engine = engine_with(config, fake.clone());
    let (status, resp) = call_get(&engine, loopback(), "/v1/models", vec![]).await;
    assert_eq!(status, 401);
    let json: Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(json["error"], "inbound_auth_required");
    assert!(fake.requests().is_empty());

    let (status, resp) = call_get(
        &engine,
        loopback(),
        "/v1/models",
        vec![("authorization".into(), "Bearer listener-secret".into())],
    )
    .await;
    assert_eq!(status, 200);
    assert!(fake.requests().is_empty());
    let json: Value = serde_json::from_slice(&resp).unwrap();
    assert_eq!(json["object"], "list");
}

#[tokio::test]
async fn sticky_prefers_last_successful_group() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].sticky_group = Some("ga".into());
    config.endpoints[1].sticky_group = Some("gb".into());
    let engine = engine_with(config.normalized(), fake.clone());

    // 第一次:两个组都可能先被试;让两边都备好——首选组 503、另一组 200。
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 503,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    // 若哈希起点是 b:b 直接 200,a 剧本不消耗。
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    let succeeded_host = fake.requests().last().unwrap().host.clone();

    // 第二次同会话:应直接粘住上次成功的组(首个出站就是它)。
    fake.push(&succeeded_host, sse_ok(&["data: {}\n\n"]));
    let before = fake.requests().len();
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    let requests = fake.requests();
    assert_eq!(requests.len(), before + 1, "粘住后应一次命中,不再遍历");
    assert_eq!(requests.last().unwrap().host, succeeded_host);
}

#[tokio::test]
async fn sticky_group_retries_within_request_then_rebinds_immediately() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].sticky_group = Some("ga".into());
    config.endpoints[1].sticky_group = Some("gb".into());
    config.retry.session_sticky_retries = 2;
    let engine = engine_with(config.normalized(), fake.clone());
    let header = session_header(&stable_session("same-request-retries"));

    for _ in 0..3 {
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 503,
                headers: vec![],
                chunks: vec![],
            },
        );
    }
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(&engine, loopback(), "/v1/messages", header.clone(), body())
            .await
            .0,
        200
    );
    assert_eq!(
        fake.requests()
            .iter()
            .map(|request| request.host.as_str())
            .collect::<Vec<_>>(),
        vec![
            "a.example.com",
            "a.example.com",
            "a.example.com",
            "b.example.com"
        ]
    );

    // 备用入口成功后，下一次同会话直接命中 b，不再先试 a。
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    let before = fake.requests().len();
    assert_eq!(
        call(&engine, loopback(), "/v1/messages", header, body())
            .await
            .0,
        200
    );
    assert_eq!(fake.requests().len(), before + 1);
    assert_eq!(fake.requests().last().unwrap().host, "b.example.com");
}

#[tokio::test]
async fn clear_project_sticky_removes_bindings_and_persists_to_disk() {
    let dir = temp_config_dir("sticky-clear");
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].sticky_group = Some("ga".into());
    config.endpoints[1].sticky_group = Some("gb".into());
    let engine = engine_with_dir(config.normalized(), dir.clone(), fake.clone());
    let header = session_header(&stable_session("sticky-clear"));

    // 第一次请求建立粘性归属;两边都备好 200,起点组任意。
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(&engine, loopback(), "/v1/messages", header, body())
            .await
            .0,
        200
    );
    engine.flush_session_affinity_if_dirty();
    let persisted = dir.load_session_affinity().unwrap();
    assert_eq!(persisted.len(), 1, "粘性归属应已落盘");
    let sticky_key = persisted.keys().next().unwrap().clone();

    // 用目录阻止原子替换：清盘失败必须返回错误；重复调用不能因内存已空而伪报成功。
    let affinity_path = dir.session_affinity_path();
    std::fs::remove_file(&affinity_path).unwrap();
    std::fs::create_dir(&affinity_path).unwrap();
    assert!(engine.clear_project_sticky("unidentified_project").is_err());
    assert!(engine.clear_project_sticky("unidentified_project").is_err());
    std::fs::remove_dir(&affinity_path).unwrap();
    let recovered = engine.clear_project_sticky("unidentified_project").unwrap();
    assert_eq!(recovered["cleared"], 0);
    assert!(dir.load_session_affinity().unwrap().is_empty());

    // 同一会话再次请求，重新建立一条归属，继续验证正常清除链路。
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            session_header(&stable_session("sticky-clear")),
            body()
        )
        .await
        .0,
        200
    );

    // 事件带 stickyKey 入库;Claude Code 会话无 workspace 声明,
    // 项目归属是 unidentified_project。
    let result = engine.clear_project_sticky("unidentified_project").unwrap();
    assert_eq!(result["matched"], 1, "应聚合到 1 个粘性键");
    assert_eq!(result["cleared"], 1, "应清除 1 条内存归属");

    // 清除后落盘文件同步变空;重复清除 matched 不变(事件仍在)但 cleared=0。
    assert!(dir.load_session_affinity().unwrap().is_empty());
    let again = engine.clear_project_sticky("unidentified_project").unwrap();
    assert_eq!(again["cleared"], 0);
    assert_eq!(again["matched"], 1);

    // 已知键直接清除也应返回 0(幂等)。
    assert_eq!(engine.clear_session_sticky(&[sticky_key]).unwrap(), 0);

    // 无 runtime store 的引擎给出明确错误。
    let storeless = engine_with(two_endpoint_config(), fake.clone());
    assert!(
        storeless
            .clear_project_sticky("unidentified_project")
            .is_err()
    );

    let _ = std::fs::remove_dir_all(dir.root);
}

#[tokio::test]
async fn new_primary_session_starts_at_lowest_priority_in_configuration_order() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].sticky_group = Some("agent-1".into());
    config.endpoints[0].priority = 10;
    config.endpoints[1].sticky_group = Some("agent-2".into());
    config.endpoints[1].priority = 0;
    let engine = engine_with(config.normalized(), fake.clone());
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));

    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            session_header("priority-first"),
            body()
        )
        .await
        .0,
        200
    );
    assert_eq!(fake.requests()[0].host, "b.example.com");
}

#[tokio::test]
async fn priority_failover_does_not_create_account_cooldown_for_next_session() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].sticky_group = Some("agent-1".into());
    config.endpoints[0].priority = 0;
    config.endpoints[1].sticky_group = Some("agent-2".into());
    config.endpoints[1].priority = 10;
    let engine = engine_with(config.normalized(), fake.clone());

    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 503,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            session_header("priority-failover"),
            body()
        )
        .await
        .0,
        200
    );
    assert_eq!(
        fake.requests()
            .iter()
            .map(|request| request.host.as_str())
            .collect::<Vec<_>>(),
        vec!["a.example.com", "b.example.com"]
    );
    // A 503 cools the endpoint+model pair. The next session therefore starts
    // at the healthy B entry instead of immediately hammering A again.
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            session_header("priority-next"),
            body()
        )
        .await
        .0,
        200
    );
    assert_eq!(fake.requests().last().unwrap().host, "b.example.com");
}

#[tokio::test]
async fn same_session_keeps_independent_assignments_per_effective_model() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].sticky_group = Some("ga".into());
    config.endpoints[0].mappings = vec![ModelMapping {
        client_pattern: "claude-opus-5".into(),
        context: ContextMode::Standard,
        failover_timeout_seconds: None,
        thinking: ThinkingMode::Disabled,
        effort: None,
        upstream_model: String::new(),
        capabilities: Vec::new(),
    }];
    config.endpoints[1].sticky_group = Some("gb".into());
    config.endpoints[1].mappings = vec![ModelMapping {
        client_pattern: "gpt-5".into(),
        context: ContextMode::Standard,
        failover_timeout_seconds: None,
        thinking: ThinkingMode::Disabled,
        effort: None,
        upstream_model: String::new(),
        capabilities: Vec::new(),
    }];
    let dir = temp_config_dir("sticky-model-namespace");
    let engine = engine_with_dir(config.normalized(), dir.clone(), fake.clone());
    let header = session_header("multi-model-session");

    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            header.clone(),
            body_for("claude-opus-5", "opus request")
        )
        .await
        .0,
        200
    );
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            header.clone(),
            body_for("gpt-5", "gpt request")
        )
        .await
        .0,
        200
    );
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            header,
            body_for("claude-opus-5", "opus request again")
        )
        .await
        .0,
        200
    );

    assert_eq!(
        fake.requests()
            .iter()
            .map(|request| request.host.as_str())
            .collect::<Vec<_>>(),
        vec!["a.example.com", "b.example.com", "a.example.com"]
    );
    engine.flush_session_affinity().unwrap();
    let raw = std::fs::read_to_string(dir.session_affinity_path()).unwrap();
    let persisted: Value = serde_json::from_str(&raw).unwrap();
    let sessions = persisted["sessions"].as_object().unwrap();
    assert_eq!(sessions.len(), 2, "两个 effectiveModel 必须有两条独立归属");
    let mut groups = sessions
        .values()
        .filter_map(|entry| entry["schedulingGroup"].as_str())
        .collect::<Vec<_>>();
    groups.sort_unstable();
    assert_eq!(groups, vec!["ga", "gb"]);
    assert!(!raw.contains("multi-model-session"));
    let _ = std::fs::remove_dir_all(dir.root);
}

#[tokio::test]
async fn legacy_pool_ids_normalize_to_one_assignment_namespace() {
    let fake = FakeTransport::new();
    let dir = temp_config_dir("sticky-pool-namespace");
    let header = session_header("cross-pool-session");

    let mut first_config = two_endpoint_config();
    first_config.endpoints[0].sticky_group = Some("ga".into());
    first_config.endpoints.truncate(1);
    let first_engine = engine_with_dir(first_config.normalized(), dir.clone(), fake.clone());
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &first_engine,
            loopback(),
            "/v1/messages",
            header.clone(),
            body()
        )
        .await
        .0,
        200
    );
    first_engine.flush_session_affinity().unwrap();
    drop(first_engine);

    let mut second_config = two_endpoint_config();
    second_config.endpoints[1].sticky_group = Some("gb".into());
    second_config.endpoints.remove(0);
    let second_engine = engine_with_dir(second_config.normalized(), dir.clone(), fake.clone());
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(&second_engine, loopback(), "/v1/messages", header, body())
            .await
            .0,
        200
    );
    second_engine.flush_session_affinity().unwrap();

    let persisted: Value =
        serde_json::from_slice(&std::fs::read(dir.session_affinity_path()).unwrap()).unwrap();
    let sessions = persisted["sessions"].as_object().unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "旧 pool id 归一化后应共用 primary 命名空间"
    );
    assert_eq!(
        sessions
            .values()
            .next()
            .and_then(|entry| entry["schedulingGroup"].as_str()),
        Some("gb")
    );
    let _ = std::fs::remove_dir_all(dir.root);
}

#[tokio::test]
async fn feature_rule_has_independent_assignment_from_same_pool_and_model() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].sticky_group = Some("ga".into());
    config.endpoints[1].sticky_group = Some("gb".into());
    config.feature_rules = vec![FeatureRule {
        enabled: true,
        id: "special-route".into(),
        match_: FeatureRuleMatch {
            messages_contain: Some("route-special".into()),
            ..Default::default()
        },
        name: "special-route".into(),
        target: FeatureRuleTarget {
            endpoint_id: Some("b".into()),
            effort: None,
            model: "claude-opus-5".into(),
            protocol_override: None,
        },
    }];
    let session_id = stable_session("feature-rule");
    let header = session_header(&session_id);
    let dir = temp_config_dir("sticky-rule-namespace");
    let engine = engine_with_dir(config.normalized(), dir.clone(), fake.clone());

    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            header.clone(),
            body_for("claude-opus-5", "ordinary request")
        )
        .await
        .0,
        200
    );
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            header.clone(),
            body_for("claude-opus-5", "route-special")
        )
        .await
        .0,
        200
    );
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            header,
            body_for("claude-opus-5", "ordinary request again")
        )
        .await
        .0,
        200
    );

    assert_eq!(
        fake.requests()
            .iter()
            .map(|request| request.host.as_str())
            .collect::<Vec<_>>(),
        vec!["a.example.com", "b.example.com", "a.example.com"]
    );
    engine.flush_session_affinity().unwrap();
    let persisted: Value =
        serde_json::from_slice(&std::fs::read(dir.session_affinity_path()).unwrap()).unwrap();
    assert_eq!(persisted["sessions"].as_object().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(dir.root);
}

#[tokio::test]
async fn empty_sticky_groups_follow_priority_order_and_survive_restart() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].sticky_group = None;
    config.endpoints[1].sticky_group = None;
    let config = config.normalized();
    let dir = temp_config_dir("sticky-empty-groups");
    let session_a = stable_session("a");
    let session_b = stable_session("b");
    let engine = engine_with_dir(config.clone(), dir.clone(), fake.clone());

    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            session_header(&session_a),
            body()
        )
        .await
        .0,
        200
    );
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            session_header(&session_b),
            body()
        )
        .await
        .0,
        200
    );
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            session_header(&session_a),
            body()
        )
        .await
        .0,
        200
    );
    engine.flush_session_affinity().unwrap();
    drop(engine);

    let restarted = engine_with_dir(config, dir.clone(), fake.clone());
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &restarted,
            loopback(),
            "/v1/messages",
            session_header(&session_b),
            body()
        )
        .await
        .0,
        200
    );
    assert_eq!(
        fake.requests()
            .iter()
            .map(|request| request.host.as_str())
            .collect::<Vec<_>>(),
        vec![
            "a.example.com",
            "a.example.com",
            "a.example.com",
            "a.example.com"
        ]
    );
    let _ = std::fs::remove_dir_all(dir.root);
}

#[tokio::test]
async fn explicit_and_empty_groups_keep_group_order() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].sticky_group = Some("shared".into());
    config.endpoints[1].sticky_group = Some("shared".into());
    config
        .endpoints
        .push(endpoint("c", "c.example.com", "sk-c"));
    let engine = engine_with(config.normalized(), fake.clone());
    let empty_session = stable_session("empty");
    let shared_session = stable_session("shared");

    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            session_header(&empty_session),
            body()
        )
        .await
        .0,
        200
    );
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 503,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            session_header(&shared_session),
            body()
        )
        .await
        .0,
        200
    );
    assert_eq!(
        fake.requests()
            .iter()
            .map(|request| request.host.as_str())
            .collect::<Vec<_>>(),
        vec!["a.example.com", "a.example.com", "b.example.com"]
    );
}

#[tokio::test]
async fn content_fingerprint_assignment_is_not_persisted() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].sticky_group = None;
    config.endpoints.truncate(1);
    let dir = temp_config_dir("sticky-temporary");
    let engine = engine_with_dir(config.normalized(), dir.clone(), fake.clone());
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));

    assert_eq!(
        call(&engine, loopback(), "/v1/messages", vec![], body())
            .await
            .0,
        200
    );
    engine.flush_session_affinity().unwrap();
    let raw = std::fs::read_to_string(dir.session_affinity_path()).unwrap();
    let persisted: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(persisted["version"], 2);
    assert!(persisted["sessions"].as_object().unwrap().is_empty());
    assert!(!raw.contains("hello"));
    let _ = std::fs::remove_dir_all(dir.root);
}

/// 旧 fallback 角色会归一化成统一 Provider，因此稳定会话也应正常写入归属。
#[tokio::test]
async fn legacy_fallback_pool_persists_unified_provider_assignment() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints.truncate(1);
    let dir = temp_config_dir("sticky-fallback-migrated");
    let engine = engine_with_dir(config.normalized(), dir.clone(), fake.clone());
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));

    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            session_header("stable-fallback-session"),
            body(),
        )
        .await
        .0,
        200
    );
    engine.flush_session_affinity().unwrap();
    let persisted: Value =
        serde_json::from_slice(&std::fs::read(dir.session_affinity_path()).unwrap()).unwrap();
    assert_eq!(persisted["version"], 2);
    let sessions = persisted["sessions"].as_object().unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "迁移后的 Provider 应留下稳定归属: {persisted}"
    );
    let _ = std::fs::remove_dir_all(dir.root);
}

/// 超过 TTL 的归属在冷启动时就该被丢掉。持久条目若不参与淘汰会只增不减,
/// 越过上限后连 SESSION_STICKY_MAX_ENTRIES 都失效(可删条目不够)。
#[tokio::test]
async fn expired_affinity_entries_are_dropped_on_load() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints.truncate(1);
    config.endpoints[0].sticky_group = Some("ga".into());
    let dir = temp_config_dir("sticky-ttl-load");
    dir.ensure_exists().unwrap();
    let stale_key = "0".repeat(64);
    let fresh_key = "1".repeat(64);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    // 31 天前 = 越过 30 天 TTL;另一条刚更新过,必须留下。
    std::fs::write(
        dir.session_affinity_path(),
        serde_json::to_vec(&json!({
            "version": 2,
            "sessions": {
                stale_key.clone(): {"schedulingGroup": "ga", "updatedAt": now - 31.0 * 24.0 * 3600.0},
                fresh_key.clone(): {"schedulingGroup": "ga", "updatedAt": now - 60.0},
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let engine = engine_with_dir(config.normalized(), dir.clone(), fake.clone());
    engine.flush_session_affinity().unwrap();
    let persisted: Value =
        serde_json::from_slice(&std::fs::read(dir.session_affinity_path()).unwrap()).unwrap();
    let sessions = persisted["sessions"].as_object().unwrap();
    assert!(!sessions.contains_key(&stale_key), "过期归属应被丢弃");
    assert!(sessions.contains_key(&fresh_key), "TTL 内的归属必须保留");
    let _ = std::fs::remove_dir_all(dir.root);
}

#[tokio::test]
async fn legacy_affinity_is_replaced_by_v2_on_first_stable_request() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints.truncate(1);
    config.endpoints[0].sticky_group = Some("ga".into());
    let dir = temp_config_dir("sticky-v1-upgrade");
    dir.ensure_exists().unwrap();
    std::fs::write(
        dir.session_affinity_path(),
        r#"{"version":1,"sessions":{"0123456789abcdef0123456789abcdef":{"schedulingGroup":"legacy","updatedAt":1}}}"#,
    )
    .unwrap();
    let engine = engine_with_dir(config.normalized(), dir.clone(), fake.clone());
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));

    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            session_header("v1-upgrade-session"),
            body()
        )
        .await
        .0,
        200
    );
    let raw = std::fs::read_to_string(dir.session_affinity_path()).unwrap();
    let persisted: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(persisted["version"], 2);
    assert_eq!(persisted["sessions"].as_object().unwrap().len(), 1);
    assert!(!raw.contains("0123456789abcdef0123456789abcdef"));
    assert!(!raw.contains("v1-upgrade-session"));
    let _ = std::fs::remove_dir_all(dir.root);
}

#[tokio::test]
async fn invalid_affinity_keeps_engine_read_only_and_preserves_original_file() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints.truncate(1);
    let dir = temp_config_dir("sticky-invalid-protection");
    dir.ensure_exists().unwrap();
    let original = r#"{"version":2,"sessions":{"bad":{"schedulingGroup":"ga","updatedAt":1}}}"#;
    std::fs::write(dir.session_affinity_path(), original).unwrap();
    let engine = engine_with_dir(config.normalized(), dir.clone(), fake.clone());
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));

    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            session_header("protected-session"),
            body()
        )
        .await
        .0,
        200
    );
    assert!(engine.flush_session_affinity().is_err());
    assert_eq!(
        std::fs::read_to_string(dir.session_affinity_path()).unwrap(),
        original
    );
    let _ = std::fs::remove_dir_all(dir.root);
}

#[tokio::test]
async fn legacy_stats_is_ignored_and_preserved_across_requests_and_reset() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints.truncate(1);
    let dir = temp_config_dir("stats-invalid-protection");
    dir.ensure_exists().unwrap();
    let original = "{broken-stats";
    let archived_stats = dir.config_path().with_file_name("stats.json");
    std::fs::write(&archived_stats, original).unwrap();
    let before = std::fs::metadata(&archived_stats)
        .unwrap()
        .modified()
        .unwrap();
    let engine = engine_with_dir(config.normalized(), dir.clone(), fake.clone());
    assert!(engine.stats_writable());
    assert_eq!(engine.runtime_snapshot(), RuntimeSnapshot::default());

    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(&engine, loopback(), "/v1/messages", vec![], body())
            .await
            .0,
        200
    );
    engine.flush_stats().unwrap();
    engine.reset_runtime().unwrap();
    assert_eq!(std::fs::read_to_string(&archived_stats).unwrap(), original);
    assert_eq!(
        std::fs::metadata(&archived_stats)
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
    let _ = std::fs::remove_dir_all(dir.root);
}

#[tokio::test]
async fn claude_session_assignment_survives_request_error_context_change_and_restart() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].sticky_group = Some("ga".into());
    config.endpoints[1].sticky_group = Some("gb".into());
    let config = config.normalized();
    let dir = temp_config_dir("sticky-restart");
    let header = vec![("X-Claude-Code-Session-Id".into(), "s0".into())];
    let first_body = body();
    let compacted_body = Bytes::from(
        serde_json::to_vec(&json!({
            "model": "claude-opus-5",
            "system": "compacted system changed",
            "messages": [{"role": "user", "content": "summary replaced the first message"}],
            "stream": true
        }))
        .unwrap(),
    );

    let engine = Engine::new(
        config.clone(),
        Some(dir.clone()),
        fake.clone(),
        "test-token".into(),
    );
    // 新会话按主池优先级/配置顺序从 a 开始；400 属于请求错误，不得迁移。
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 400,
            headers: vec![],
            chunks: vec![],
        },
    );
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            header.clone(),
            first_body.clone()
        )
        .await
        .0,
        400
    );
    engine.flush_session_affinity().unwrap();
    let persisted = std::fs::read_to_string(dir.session_affinity_path()).unwrap();
    assert!(!persisted.contains("s0"), "原始 session ID 不得落盘");

    // 即使首次没有成功，重启 Engine 并改变上下文仍应直接命中 a，不尝试 b。
    let restarted = Engine::new(config, Some(dir.clone()), fake.clone(), "test-token".into());
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    let before = fake.requests().len();
    assert_eq!(
        call(
            &restarted,
            loopback(),
            "/v1/messages",
            header,
            compacted_body
        )
        .await
        .0,
        200
    );
    let requests = fake.requests();
    assert_eq!(requests.len(), before + 1);
    assert_eq!(requests.last().unwrap().host, "a.example.com");
    let _ = std::fs::remove_dir_all(dir.root);
}

#[tokio::test]
async fn explicit_402_moves_session_once_then_sticks_to_replacement() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].sticky_group = Some("ga".into());
    config.endpoints[1].sticky_group = Some("gb".into());
    let engine = engine_with(config.normalized(), fake.clone());
    let session_id = stable_session("moves");
    let header = session_header(&session_id);

    // 选定 home=a 的稳定会话。402 明确表示当前入口额度不可用，允许迁移到 b。
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 402,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(&engine, loopback(), "/v1/messages", header.clone(), body())
            .await
            .0,
        200
    );
    assert_eq!(
        fake.requests()
            .iter()
            .map(|request| request.host.as_str())
            .collect::<Vec<_>>(),
        vec!["a.example.com", "b.example.com"]
    );

    // 后续同会话直接固定到迁移后的 b。
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    let before = fake.requests().len();
    assert_eq!(
        call(&engine, loopback(), "/v1/messages", header, body())
            .await
            .0,
        200
    );
    let requests = fake.requests();
    assert_eq!(requests.len(), before + 1);
    assert_eq!(requests.last().unwrap().host, "b.example.com");
}

#[tokio::test]
async fn responses_inbound_bridges_tools_and_passes_client_ua() {
    let fake = FakeTransport::new();
    let chunks = anthropic_tool_sse();
    fake.push(
        "a.example.com",
        sse_ok(&chunks.iter().map(String::as_str).collect::<Vec<_>>()),
    );
    let engine = engine_with(two_endpoint_config(), fake.clone());
    let body = Bytes::from(
        serde_json::to_vec(&json!({
            "model": "claude-opus-5",
            "instructions": "you are codex",
            "input": [{"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": "ls"}]}],
            "tools": [{"type": "function", "name": "shell",
                       "parameters": {"type": "object"}}],
            "stream": true,
        }))
        .unwrap(),
    );
    let headers = vec![
        ("User-Agent".to_string(), "codex_cli_rs/0.5.0".to_string()),
        ("session_id".to_string(), "codex-session-1".to_string()),
        (
            "x-codex-turn-metadata".to_string(),
            r#"{"thread_id":"thread-accepted","turn_id":"turn-accepted","subagent_kind":"thread_spawn"}"#.to_string(),
        ),
    ];
    let response = engine
        .handle_request(loopback(), "POST", "/v1/responses", headers, body)
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream; charset=utf-8"
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("event: response.created\n"));
    assert!(text.contains("event: response.output_text.delta\n"));
    assert!(text.contains("event: response.function_call_arguments.delta\n"));
    assert!(text.contains("event: response.completed\n"));
    assert!(text.contains(r#"\"cmd\":[\"ls\"]"#) || text.contains(r#"{\"cmd\":"#));

    // 出站:Anthropic 形状、路径覆写 /v1/messages、tools 保留、UA 透传 codex。
    let recorded = fake.requests();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].path, "/v1/messages");
    let sent: Value = serde_json::from_slice(&recorded[0].body).unwrap();
    assert_eq!(sent["system"], "you are codex");
    assert_eq!(sent["messages"][0]["content"][0]["text"], "ls");
    assert_eq!(sent["tools"][0]["name"], "shell");
    assert_eq!(sent["stream"], true);
    let ua = recorded[0]
        .headers
        .iter()
        .find(|(n, _)| n == "user-agent")
        .map(|(_, v)| v.clone())
        .unwrap();
    assert_eq!(ua, "codex_cli_rs/0.5.0");
    let runtime = runtime_of(&engine).await;
    for kind in ["client", "upstream"] {
        let event = runtime
            .recent_events
            .iter()
            .find(|event| event.kind == kind)
            .expect("accepted Codex event");
        let metadata = event
            .codex_metadata
            .as_ref()
            .expect("accepted event should retain Codex metadata");
        assert_eq!(metadata.thread_id.as_deref(), Some("thread-accepted"));
        assert_eq!(metadata.turn_id.as_deref(), Some("turn-accepted"));
        assert_eq!(metadata.subagent_kind.as_deref(), Some("thread_spawn"));
    }
    assert_messages_in_vocabulary(&runtime);
}

/// Codex Responses 的普通无工具请求即使带 instructions,也不能被误报为
/// Claude Code 内部请求的指纹失配。
#[tokio::test]
async fn codex_responses_without_tools_has_no_unmatched_token() {
    let fake = FakeTransport::new();
    fake.push(
        "a.example.com",
        sse_ok(&[
            "event: message_start\ndata: {}\n\n",
            "event: message_stop\ndata: {}\n\n",
        ]),
    );
    let engine = engine_with(two_endpoint_config(), fake);
    let body = Bytes::from(
        serde_json::to_vec(&json!({
            "model": "claude-opus-5",
            "instructions": "You are Codex.",
            "input": "ls",
            "stream": true,
        }))
        .unwrap(),
    );
    let (status, _) = call(
        &engine,
        loopback(),
        "/backend-api/codex/responses",
        vec![("user-agent".into(), "codex_cli_rs/0.5.0".into())],
        body,
    )
    .await;
    assert_eq!(status, 200);

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .unwrap();
    assert_eq!(client.client_kind, Some(ClientKind::Codex));
    assert_eq!(
        client.request_purpose,
        Some(sumpter_core::routing::RequestPurpose::Standard)
    );
    assert!(
        !client
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("unmatched_no_tools")
    );
}

#[tokio::test]
async fn codex_responses_retries_mixed_401_and_connection_failure_before_recovery() {
    let fake = FakeTransport::new();
    fake.set_delay(Duration::from_millis(1));
    let mut config = two_endpoint_config();
    config.retry.max_deferred_rounds = 2;
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 401,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("b.example.com", Outcome::Error("connection reset".into()));
    let chunks = anthropic_tool_sse();
    fake.push(
        "a.example.com",
        sse_ok(&chunks.iter().map(String::as_str).collect::<Vec<_>>()),
    );
    let engine = engine_with(config, fake.clone());
    let body = Bytes::from(
        serde_json::to_vec(&json!({
            "model": "claude-opus-5",
            "input": "ls",
            "stream": true,
        }))
        .unwrap(),
    );
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            vec![("session_id".into(), "codex-retry-1".into())],
            body,
        )
        .await;
    assert_eq!(response.status(), 200);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("event: response.completed"));
    let recorded = fake.requests();
    assert_eq!(recorded.len(), 3, "401 + connection failure 后应进入下一轮");
    assert!(
        recorded
            .iter()
            .all(|request| request.path == "/v1/messages")
    );
    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .unwrap();
    assert!(
        client
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("deferred_rounds 2")
    );
}

#[tokio::test]
async fn chat_inbound_non_stream_aggregates_to_single_json() {
    let fake = FakeTransport::new();
    let chunks = anthropic_tool_sse();
    fake.push(
        "a.example.com",
        sse_ok(&chunks.iter().map(String::as_str).collect::<Vec<_>>()),
    );
    let engine = engine_with(two_endpoint_config(), fake.clone());
    let body = Bytes::from(
        serde_json::to_vec(&json!({
            "model": "claude-opus-5",
            "messages": [{"role": "user", "content": "ls"}],
        }))
        .unwrap(),
    );
    let response = engine
        .handle_request(loopback(), "POST", "/chat/completions", vec![], body)
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let completion: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(completion["object"], "chat.completion");
    assert_eq!(completion["choices"][0]["message"]["content"], "running");
    assert_eq!(
        completion["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
        "shell"
    );
    assert_eq!(completion["choices"][0]["finish_reason"], "tool_calls");
    // 无 UA 入站:出站回填 CC 指纹。
    let ua = fake.requests()[0]
        .headers
        .iter()
        .find(|(n, _)| n == "user-agent")
        .map(|(_, v)| v.clone())
        .unwrap();
    assert_eq!(ua, "claude-cli/2.1.220 (external, cli)");
}

#[tokio::test]
async fn translated_openai_chat_non_stream_survives_double_bridge() {
    // OpenAI 入站 → Responses 上游需要两层桥: Responses→Anthropic→Chat。
    // 上游桥必须保持 SSE,否则客户端桥无法消费非流式聚合 JSON。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::OpenAIResponses;
    config.endpoints[1].enabled = false;
    fake.push(
        "a.example.com",
        sse_ok(&[
            "data: {\"type\":\"response.created\",\"response\":{\"model\":\"responses-up\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\n\n",
        ]),
    );
    let engine = engine_with(config.normalized(), fake.clone());
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/chat/completions",
            vec![],
            Bytes::from(
                serde_json::to_vec(&json!({
                    "model": "claude-opus-5",
                    "messages": [{"role": "user", "content": "hi"}],
                    "stream": false,
                }))
                .unwrap(),
            ),
        )
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );
    let completion: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(completion["object"], "chat.completion");
    assert_eq!(completion["choices"][0]["message"]["content"], "hello");
    assert_eq!(completion["choices"][0]["finish_reason"], "stop");
    assert_eq!(fake.requests()[0].path, "/v1/responses");
}

#[tokio::test]
async fn openai_inbound_rejects_unconvertible_body() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    let (status, bytes) = call(
        &engine,
        loopback(),
        "/v1/responses",
        vec![],
        Bytes::from(r#"{"model":"claude-opus-5"}"#),
    )
    .await;
    assert_eq!(status, 400);
    let error: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(error["error"], "invalid_request");
    assert!(fake.requests().is_empty());
    assert_messages_in_vocabulary(&runtime_of(&engine).await);
}

#[tokio::test]
async fn messages_path_rejects_non_anthropic_request_shape_without_upstream_request() {
    for body in [
        r#"{"model":"claude-opus-5","input":[]}"#,
        r#"{"model":"","messages":[]}"#,
        r#"{"model":"claude-opus-5","messages":"hello"}"#,
    ] {
        let fake = FakeTransport::new();
        let engine = engine_with(two_endpoint_config(), fake.clone());
        let (status, bytes) = call(
            &engine,
            loopback(),
            "/v1/messages",
            vec![],
            Bytes::from(body),
        )
        .await;
        assert_eq!(status, 400);
        let error: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(error["error"], "invalid_request");
        assert!(fake.requests().is_empty());
        let runtime = runtime_of(&engine).await;
        let event = runtime
            .recent_events
            .iter()
            .find(|event| event.kind == "client")
            .expect("rejected client event");
        assert_eq!(
            event.message.as_deref(),
            Some("anthropic request shape invalid")
        );
        assert_eq!(event.source_format, Some(ProviderProtocol::Anthropic));
        assert_eq!(event.target_format, None);
        assert_eq!(event.route_mode, None);
        assert_messages_in_vocabulary(&runtime);
    }
}

#[tokio::test]
async fn responses_compact_is_unary_passthrough_with_model_rewrite() {
    for path in [
        "/v1/responses/compact",
        "/responses/compact",
        "/backend-api/codex/responses/compact",
    ] {
        let fake = FakeTransport::new();
        let mut config = two_endpoint_config();
        set_all_endpoint_protocols(&mut config, EndpointProtocolMode::Auto);
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                chunks: vec![br#"{"object":"response.compaction","output":[{"type":"compaction","encrypted_content":"opaque"}]}"#.to_vec()],
            },
        );
        let engine = engine_with(config.normalized(), fake.clone());
        let (status, bytes) = call(
            &engine,
            loopback(),
            path,
            vec![("User-Agent".into(), "codex_cli_rs/0.5.0".into())],
            Bytes::from(
                serde_json::to_vec(&json!({
                    "model": "claude-opus-5(high)",
                    "input": [{"type": "message", "role": "user", "content": "old"}],
                    "stream": false,
                }))
                .unwrap(),
            ),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["object"],
            "response.compaction"
        );
        let recorded = fake.requests();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].path, "/v1/responses/compact");
        assert_eq!(
            recorded[0]
                .headers
                .iter()
                .find(|(name, _)| name == "accept")
                .unwrap()
                .1,
            "application/json"
        );
        let sent: Value = serde_json::from_slice(&recorded[0].body).unwrap();
        assert_eq!(sent["model"], "claude-opus-5");
        assert_eq!(sent["input"][0]["content"], "old");
        let runtime = runtime_of(&engine).await;
        let client = runtime
            .recent_events
            .iter()
            .find(|event| event.kind == "client")
            .expect("compact client event");
        assert_eq!(client.client_kind, Some(ClientKind::Codex));
        assert_eq!(
            client.request_purpose,
            Some(sumpter_core::routing::RequestPurpose::Compact)
        );
        let upstream = runtime
            .recent_events
            .iter()
            .find(|event| event.kind == "upstream")
            .expect("compact upstream event");
        assert_eq!(
            upstream.request_purpose,
            Some(sumpter_core::routing::RequestPurpose::Compact)
        );
    }
}

#[tokio::test]
async fn responses_compact_failover_happens_before_unary_response_headers() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    set_all_endpoint_protocols(&mut config, EndpointProtocolMode::Auto);
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 503,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push(
        "b.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![br#"{"object":"response.compaction","output":[]}"#.to_vec()],
        },
    );
    let engine = engine_with(config.normalized(), fake.clone());
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/responses/compact",
        vec![],
        Bytes::from(r#"{"model":"claude-opus-5","input":[]}"#),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        fake.requests()
            .iter()
            .map(|request| request.host.as_str())
            .collect::<Vec<_>>(),
        ["a.example.com", "b.example.com"]
    );
}

#[tokio::test]
async fn responses_compact_requires_compatible_endpoint_protocol() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    let (status, bytes) = call(
        &engine,
        loopback(),
        "/v1/responses/compact",
        vec![],
        Bytes::from(r#"{"model":"claude-opus-5","input":[]}"#),
    )
    .await;
    assert_eq!(status, 400);
    let error: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(error["error"], "no_compatible_protocol");
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn auto_endpoint_sends_raw_responses_body_natively() {
    // Auto 按入站路径解析为 Responses；上游响应原样回写，工具声明一字不动。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    // Auto 不是出站协议；本次请求实际解析为 openai-responses。
    assert_eq!(config.endpoints[0].protocol, EndpointProtocolMode::Auto);
    config.endpoints[0].mappings = vec![ModelMapping {
        client_pattern: "claude-opus-*".into(),
        context: ContextMode::Standard,
        failover_timeout_seconds: None,
        thinking: ThinkingMode::Adaptive,
        effort: None,
        upstream_model: String::new(),
        capabilities: Vec::new(),
    }];
    let upstream_sse = concat!(
        "event: response.created\ndata: {\"type\":\"response.created\"}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
        "event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n",
    );
    fake.push("a.example.com", sse_ok(&[upstream_sse]));
    let engine = engine_with(config.normalized(), fake.clone());
    let inbound_body = json!({
        "model": "claude-opus-5",
        "instructions": "you are codex",
        "input": [{"type": "message", "role": "user",
                   "content": [{"type": "input_text", "text": "ls"}]}],
        "tools": [{"type": "function", "name": "shell", "parameters": {"type": "object"}}],
        "include": ["reasoning.encrypted_content"],
        "stream": true,
    });
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            vec![("User-Agent".into(), "codex_cli_rs/0.5.0".into())],
            Bytes::from(serde_json::to_vec(&inbound_body).unwrap()),
        )
        .await;
    assert_eq!(response.status(), 200);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    // 响应原样回写:上游的 Responses SSE 一字不改(未经 Anthropic 往返)。
    assert_eq!(String::from_utf8_lossy(&bytes), upstream_sse);

    let recorded = fake.requests();
    assert_eq!(recorded.len(), 1);
    // baseURL 未带 /v1 时补上(与既有 openai 出站同规)。
    assert_eq!(recorded[0].path, "/v1/responses");
    let sent: Value = serde_json::from_slice(&recorded[0].body).unwrap();
    // 原始体保真:仅 model 被换成 upstreamModel。
    assert_eq!(sent["model"], "claude-opus-5");
    assert_eq!(sent["instructions"], "you are codex");
    assert_eq!(sent["tools"], inbound_body["tools"]);
    assert_eq!(sent["include"], inbound_body["include"]);
    assert_eq!(sent["input"], inbound_body["input"]);
    assert!(sent.get("messages").is_none()); // 没被转成 Anthropic 形状
    // 事件记 passthrough 而非 bridge。
    let runtime = runtime_of(&engine).await;
    assert!(
        runtime.recent_events.iter().any(|e| e
            .message
            .as_deref()
            .is_some_and(|m| m.contains("passthrough responses"))),
        "事件应带 passthrough token: {:?}",
        runtime
            .recent_events
            .iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
    );
    assert!(
        !runtime
            .recent_events
            .iter()
            .any(|e| e.message.as_deref().is_some_and(|m| m.contains("bridge "))),
    );
    assert_messages_in_vocabulary(&runtime);
}

#[tokio::test]
async fn native_non_stream_conversation_without_json_terminal_is_stream_failure() {
    // Native non-stream Chat/Responses bodies are JSON, but still need a
    // protocol-level terminal field before a 200 request can be counted as
    // successful.  Exercise malformed, truncated and valid-but-in-progress
    // bodies for both dialects.
    let cases = [
        (
            "/v1/responses",
            json!({"model":"claude-opus-5","input":"ls","stream":false}),
            vec![
                br#"{"status":}"#.to_vec(),
                br#"{"status":"completed""#.to_vec(),
                br#"{"status":"in_progress"}"#.to_vec(),
            ],
        ),
        (
            "/v1/chat/completions",
            json!({"model":"claude-opus-5","messages":[{"role":"user","content":"ls"}],"stream":false}),
            vec![
                br#"{"choices":}"#.to_vec(),
                br#"{"choices":[{"message":{"content":"hi"}}]"#.to_vec(),
                br#"{"choices":[{"message":{"content":"hi"},"finish_reason":null}]}"#.to_vec(),
            ],
        ),
        (
            "/v1/messages",
            json!({"model":"claude-opus-5","messages":[{"role":"user","content":"ls"}],"stream":false}),
            vec![
                br#"{"type":}"#.to_vec(),
                br#"{"type":"message""#.to_vec(),
                br#"{"type":"message_start","message":{"id":"m"}}"#.to_vec(),
            ],
        ),
    ];

    for (path, request, bodies) in cases {
        for response_body in bodies {
            let fake = FakeTransport::new();
            fake.push(
                "a.example.com",
                Outcome::Status {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    chunks: {
                        let midpoint = (response_body.len() / 2).max(1);
                        vec![
                            response_body[..midpoint].to_vec(),
                            response_body[midpoint..].to_vec(),
                        ]
                    },
                },
            );
            let mut config = two_endpoint_config();
            config.endpoints[0].protocol = match path {
                "/v1/messages" => EndpointProtocolMode::Anthropic,
                "/v1/chat/completions" => EndpointProtocolMode::OpenAI,
                "/v1/responses" => EndpointProtocolMode::OpenAIResponses,
                _ => unreachable!(),
            };
            config.endpoints[1].enabled = false;
            let engine = engine_with(config.normalized(), fake);
            let (status, bytes) = call(
                &engine,
                loopback(),
                path,
                vec![("User-Agent".into(), "codex_cli_rs/0.5.0".into())],
                Bytes::from(serde_json::to_vec(&request).unwrap()),
            )
            .await;
            assert_eq!(status, 200);
            assert_eq!(bytes, response_body);

            let runtime = runtime_of(&engine).await;
            let client = runtime
                .recent_events
                .iter()
                .find(|event| event.kind == "client")
                .expect("client event");
            assert_eq!(client.status_code, 200);
            assert_eq!(
                client.outcome,
                Some(RuntimeEventOutcome::Failed),
                "path={path}, body={:?}",
                String::from_utf8_lossy(&response_body)
            );
            assert_eq!(
                client.failure_kind,
                Some(RuntimeFailureKind::StreamInterrupted)
            );
            assert!(
                client
                    .failure_detail
                    .as_deref()
                    .is_some_and(|detail| { detail.contains("before a protocol terminal event") })
            );
            assert_eq!(runtime.client_successes, 0);
            assert_eq!(runtime.client_failures, 1);
            assert_messages_in_vocabulary(&runtime);
        }
    }
}

#[tokio::test]
async fn responses_terminal_completes_while_upstream_body_stays_open() {
    // Codex 收到 response.completed 后会释放 body；上游 TCP 仍未 EOF 不能把它记成 499。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    config.endpoints[1].enabled = false;
    fake.push("a.example.com", Outcome::Gated { status: 200 });
    let engine = engine_with(config.normalized(), fake.clone());

    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            vec![("User-Agent".into(), "codex_cli_rs/0.5.0".into())],
            codex_responses_body(),
        )
        .await;
    assert_eq!(response.status(), 200);
    let tx = fake.gate_sender();
    let mut stream = response.into_body().into_data_stream();

    let chunks: [&[u8]; 4] = [
        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"response.completed\"}\n\n",
        b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"name\":\"shell_command\"}}\n\n",
        b"event: response.completed\ndata: {\"type\":\"response.comp",
        b"leted\",\"response\":{\"status\":\"completed\"}}\n\n",
    ];
    let mut forwarded = Vec::new();
    for chunk in chunks {
        tx.send(Ok(Bytes::copy_from_slice(chunk))).unwrap();
        let received = stream.next().await.unwrap().unwrap();
        forwarded.extend_from_slice(&received);
    }
    assert_eq!(forwarded, chunks.concat(), "协议观察不得改变透传字节");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), stream.next())
            .await
            .expect("终止事件后 body 应主动结束")
            .is_none(),
        "上游 sender 仍存活时也应按协议结束"
    );

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .unwrap();
    let upstream = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "upstream")
        .unwrap();
    assert_eq!(client.status_code, 200);
    assert_eq!(client.outcome, Some(RuntimeEventOutcome::Succeeded));
    assert_eq!(client.failure_kind, None);
    assert_eq!(
        client.tool_calls.as_deref(),
        Some(["shell_command".into()].as_slice())
    );
    assert_eq!(
        upstream.tool_calls.as_deref(),
        Some(["shell_command".into()].as_slice())
    );
    assert_eq!(upstream.status_code, 200);
    assert_eq!(upstream.outcome, Some(RuntimeEventOutcome::Succeeded));
    assert_eq!(runtime.client_successes, 1);
    assert_eq!(runtime.client_failures, 0);
    assert_eq!(runtime.upstream_successes, 1);
    assert_eq!(runtime.upstream_failures, 0);
    assert_messages_in_vocabulary(&runtime);

    // 保持 tx 活到断言之后：证明成功不是由上游 channel EOF 触发。
    drop(tx);
}

#[tokio::test]
async fn chat_done_and_anthropic_message_stop_complete_before_eof() {
    let cases = [
        (
            true,
            "/v1/chat/completions",
            Bytes::from(
                r#"{"model":"claude-opus-5","messages":[{"role":"user","content":"hi"}],"stream":true}"#,
            ),
            b"data: [DONE]\n\n".as_slice(),
        ),
        (
            false,
            "/v1/messages",
            body(),
            b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".as_slice(),
        ),
    ];

    for (auto_mode, path, request_body, terminal) in cases {
        let fake = FakeTransport::new();
        let mut config = two_endpoint_config();
        if auto_mode {
            config.endpoints[0].protocol = match path {
                "/v1/messages" => EndpointProtocolMode::Anthropic,
                "/v1/chat/completions" => EndpointProtocolMode::OpenAI,
                "/v1/responses" => EndpointProtocolMode::OpenAIResponses,
                _ => unreachable!(),
            };
        }
        config.endpoints[1].enabled = false;
        fake.push("a.example.com", Outcome::Gated { status: 200 });
        let engine = engine_with(config.normalized(), fake.clone());

        let response = engine
            .handle_request(loopback(), "POST", path, vec![], request_body)
            .await;
        assert_eq!(response.status(), 200);
        let tx = fake.gate_sender();
        let mut stream = response.into_body().into_data_stream();
        tx.send(Ok(Bytes::copy_from_slice(terminal))).unwrap();
        assert_eq!(stream.next().await.unwrap().unwrap().as_ref(), terminal);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), stream.next())
                .await
                .expect("协议终止后应结束 body")
                .is_none()
        );

        let runtime = runtime_of(&engine).await;
        let client = runtime
            .recent_events
            .iter()
            .find(|event| event.kind == "client")
            .unwrap();
        assert_eq!(client.status_code, 200);
        assert_eq!(client.outcome, Some(RuntimeEventOutcome::Succeeded));
        assert_eq!(runtime.client_successes, 1);
        assert_messages_in_vocabulary(&runtime);
        drop(tx);
    }
}

#[tokio::test]
async fn responses_terminal_failure_and_incomplete_are_not_client_cancellations() {
    for (payload, expected_kind) in [
        (
            "data: {\"type\":\"response.incomplete\",\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
            RuntimeFailureKind::UpstreamResponseIncomplete,
        ),
        (
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\"}}}\n\n",
            RuntimeFailureKind::UpstreamResponseFailed,
        ),
        (
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
            RuntimeFailureKind::UpstreamResponseIncomplete,
        ),
        (
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\"}}}\n\n",
            RuntimeFailureKind::UpstreamResponseFailed,
        ),
    ] {
        let fake = FakeTransport::new();
        let mut config = two_endpoint_config();
        config.endpoints[0].protocol = EndpointProtocolMode::Auto;
        config.endpoints[1].enabled = false;
        fake.push("a.example.com", sse_ok(&[payload]));
        let engine = engine_with(config.normalized(), fake);

        let (status, bytes) = call(
            &engine,
            loopback(),
            "/v1/responses",
            vec![("User-Agent".into(), "codex_cli_rs/0.5.0".into())],
            codex_responses_body(),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(bytes, payload.as_bytes());

        let runtime = runtime_of(&engine).await;
        let client = runtime
            .recent_events
            .iter()
            .find(|event| event.kind == "client")
            .unwrap();
        assert_eq!(client.status_code, 200);
        assert_eq!(client.outcome, Some(RuntimeEventOutcome::Failed));
        assert_eq!(client.failure_kind, Some(expected_kind));
        assert_eq!(
            client.failure_phase,
            Some(RuntimeFailurePhase::ResponseStream)
        );
        assert_eq!(runtime.client_successes, 0);
        assert_eq!(runtime.client_failures, 1);
        assert_messages_in_vocabulary(&runtime);
    }
}

#[tokio::test]
async fn responses_passthrough_eof_without_terminal_is_stream_failure() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    config.endpoints[1].enabled = false;
    let partial = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n";
    fake.push("a.example.com", sse_ok(&[partial]));
    let engine = engine_with(config.normalized(), fake);

    let (status, bytes) = call(
        &engine,
        loopback(),
        "/v1/responses",
        vec![("User-Agent".into(), "codex_cli_rs/0.5.0".into())],
        codex_responses_body(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(bytes, partial.as_bytes());

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .unwrap();
    assert_eq!(client.outcome, Some(RuntimeEventOutcome::Failed));
    assert_eq!(
        client.failure_kind,
        Some(RuntimeFailureKind::StreamInterrupted)
    );
    assert!(
        client
            .failure_detail
            .as_deref()
            .is_some_and(|detail| detail.contains("before a protocol terminal event"))
    );
    assert_eq!(runtime.client_successes, 0);
    assert_eq!(runtime.client_failures, 1);
    assert_messages_in_vocabulary(&runtime);
}

#[tokio::test]
async fn native_responses_reaches_fixed_responses_endpoint_with_tools() {
    // 原生 Responses 不经过 Translator 的 tools 能力守卫。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::OpenAIResponses;
    fake.push(
        "a.example.com",
        sse_ok(&["data: {\"type\":\"response.completed\"}\n\n"]),
    );
    let engine = engine_with(config.normalized(), fake.clone());
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            vec![],
            Bytes::from(
                serde_json::to_vec(&json!({
                    "model": "claude-opus-5",
                    "input": "ls",
                    "tools": [{"type": "function", "name": "shell",
                               "parameters": {"type": "object"}}],
                    "stream": true,
                }))
                .unwrap(),
            ),
        )
        .await;
    assert_eq!(response.status(), 200);
    let recorded = fake.requests();
    assert_eq!(
        recorded.len(),
        1,
        "不应被 openai_tools_unsupported 守卫跳过"
    );
    assert_eq!(recorded[0].path, "/v1/responses");
}

#[tokio::test]
async fn fixed_anthropic_endpoint_translates_responses() {
    // 固定 Anthropic 入口对 Responses 入站走双向 Translator。
    let fake = FakeTransport::new();
    let chunks = anthropic_tool_sse();
    fake.push(
        "a.example.com",
        sse_ok(&chunks.iter().map(String::as_str).collect::<Vec<_>>()),
    );
    let config = two_endpoint_config();
    assert_eq!(
        config.endpoints[0].protocol,
        EndpointProtocolMode::Anthropic
    );
    let engine = engine_with(config, fake.clone());
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            vec![],
            Bytes::from(r#"{"model":"claude-opus-5","input":"ls","stream":true}"#),
        )
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(fake.requests()[0].path, "/v1/messages");
}

#[tokio::test]
async fn events_separate_logical_model_from_upstream_alias() {
    // 分类器经分流规则打到别名上游:effectiveModel 记逻辑模型,
    // upstreamModel 只记实际出站的中转站别名 —— 两者不再互相顶掉。
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    // 迁入 Provider 的入口 b:把逻辑模型 gpt-5.6-luna 映射成中转站私有别名。
    config.endpoints.push({
        let mut ep = endpoint("relay", "relay.example.com", "sk-relay");
        ep.mappings = vec![ModelMapping {
            client_pattern: "gpt-5.6-luna".into(),
            context: ContextMode::Standard,
            failover_timeout_seconds: None,
            thinking: ThinkingMode::Passthrough,
            effort: None,
            upstream_model: "provider-luna".into(),
            capabilities: Vec::new(),
        }];
        ep
    });
    config.feature_rules = vec![FeatureRule {
        enabled: true,
        id: "classifier".into(),
        match_: FeatureRuleMatch {
            system_contains: Some(
                "You are a security monitor for autonomous AI coding agents.".into(),
            ),
            ..Default::default()
        },
        name: "Classifier".into(),
        target: FeatureRuleTarget {
            endpoint_id: Some("relay".into()),
            effort: None,
            model: "gpt-5.6-luna".into(),
            protocol_override: None,
        },
    }];
    fake.push(
        "relay.example.com",
        sse_ok(&["data: {\"type\":\"message_stop\"}\n\n"]),
    );
    let engine = engine_with(config.normalized(), fake.clone());
    let body = Bytes::from(
        serde_json::to_vec(&json!({
            "model": "claude-haiku-4-5-20251001",
            "system": "You are a security monitor for autonomous AI coding agents.",
            // classifier 指纹:最后一条 user 消息须由 <transcript> 包裹(无 tools 分支)。
            "messages": [{"role": "user", "content": "<transcript>\nls -la\n</transcript>"}],
            "stream": true,
        }))
        .unwrap(),
    );
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body).await;
    assert_eq!(status, 200);
    // 出站确实使用了入口映射后的模型名。
    assert_eq!(
        serde_json::from_slice::<Value>(&fake.requests()[0].body).unwrap()["model"],
        "provider-luna"
    );

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .expect("client 事件");
    assert_eq!(client.feature_rule_id.as_deref(), Some("classifier"));
    // 逻辑模型 = 规则 target.model;客户端模型 = CC 原始请求模型。
    assert_eq!(
        client.client_model.as_deref(),
        Some("claude-haiku-4-5-20251001")
    );
    assert_eq!(client.effective_model.as_deref(), Some("gpt-5.6-luna"));
    // 上游模型只出现在 upstreamModel,不再顶掉逻辑模型。
    assert_eq!(client.upstream_model.as_deref(), Some("provider-luna"));

    let upstream = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "upstream")
        .expect("upstream 事件");
    assert_eq!(
        upstream.client_model.as_deref(),
        Some("claude-haiku-4-5-20251001")
    );
    assert_eq!(upstream.feature_rule_id.as_deref(), Some("classifier"));
    assert_eq!(upstream.effective_model.as_deref(), Some("gpt-5.6-luna"));
    assert_eq!(upstream.upstream_model.as_deref(), Some("provider-luna"));
}

// ---------------------------------------------------------------------------
// 客户端类型(clientKind)
// ---------------------------------------------------------------------------

/// UA 判定 Claude Code,client 与 upstream 两类事件都带上。
#[tokio::test]
async fn client_kind_marks_claude_code_from_user_agent() {
    let fake = FakeTransport::new();
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    let engine = engine_with(two_endpoint_config(), fake.clone());
    let body = Bytes::from(
        serde_json::to_vec(&json!({
            "model": "claude-opus-5",
            "messages": [{"role": "user", "content": "x"}],
        }))
        .unwrap(),
    );
    let headers = vec![(
        "user-agent".to_string(),
        "claude-cli/2.1.220 (external, cli)".to_string(),
    )];
    let (status, _) = call(&engine, loopback(), "/v1/messages", headers, body).await;
    assert_eq!(status, 200);

    let runtime = runtime_of(&engine).await;
    for kind in ["client", "upstream"] {
        let event = runtime
            .recent_events
            .iter()
            .find(|e| e.kind == kind)
            .expect("事件");
        assert_eq!(
            event.client_kind,
            Some(ClientKind::ClaudeCode),
            "{kind} 事件应记 claude_code"
        );
    }
}

/// Codex 经 /v1/responses 入站:UA 认出 codex,兼容层不影响判定。
#[tokio::test]
async fn client_kind_marks_codex_from_openai_inbound() {
    let fake = FakeTransport::new();
    let chunks = anthropic_tool_sse();
    fake.push(
        "a.example.com",
        sse_ok(&chunks.iter().map(String::as_str).collect::<Vec<_>>()),
    );
    let engine = engine_with(two_endpoint_config(), fake.clone());
    let body = Bytes::from(
        serde_json::to_vec(&json!({
            "model": "claude-opus-5",
            "input": [{"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": "ls"}]}],
            "stream": true,
        }))
        .unwrap(),
    );
    let headers = vec![("User-Agent".to_string(), "codex_cli_rs/0.5.0".to_string())];
    let response = engine
        .handle_request(loopback(), "POST", "/v1/responses", headers, body)
        .await;
    assert_eq!(response.status(), 200);
    let _ = axum::body::to_bytes(response.into_body(), usize::MAX).await;

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .expect("client 事件");
    assert_eq!(client.client_kind, Some(ClientKind::Codex));
}

/// Grok Build 的客户端产品名仍是 grok-shell；经 Responses 入站时应保留具体来源，
/// 不能被 OpenAI 方言兜底吞成 openai_compat。
#[tokio::test]
async fn client_kind_marks_grok_build_from_shell_user_agent() {
    let fake = FakeTransport::new();
    let chunks = anthropic_tool_sse();
    fake.push(
        "a.example.com",
        sse_ok(&chunks.iter().map(String::as_str).collect::<Vec<_>>()),
    );
    let engine = engine_with(two_endpoint_config(), fake.clone());
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            vec![(
                "User-Agent".to_string(),
                "grok-pager/0.2.119 grok-shell/0.2.119 (macos; aarch64)".to_string(),
            )],
            Bytes::from(
                serde_json::to_vec(&json!({
                    "model": "claude-opus-5",
                    "input": "hello",
                    "stream": true,
                }))
                .unwrap(),
            ),
        )
        .await;
    assert_eq!(response.status(), 200);
    let _ = axum::body::to_bytes(response.into_body(), usize::MAX).await;

    let runtime = runtime_of(&engine).await;
    for kind in ["client", "upstream"] {
        let event = runtime
            .recent_events
            .iter()
            .find(|e| e.kind == kind)
            .expect("事件");
        assert_eq!(
            event.client_kind,
            Some(ClientKind::GrokBuild),
            "{kind} 事件应记 grok_build"
        );
    }
}

/// 未知 UA:Anthropic 入站记 unknown,OpenAI 兼容层入站记 openai_compat
/// ——「走哪个兼容层」本身就是排障信息,不能一律 unknown。
#[tokio::test]
async fn client_kind_falls_back_to_inbound_dialect() {
    let fake = FakeTransport::new();
    fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
    let engine = engine_with(two_endpoint_config(), fake.clone());
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![("user-agent".to_string(), "curl/8.7.1".to_string())],
        Bytes::from(
            serde_json::to_vec(&json!({
                "model": "claude-opus-5",
                "messages": [{"role": "user", "content": "x"}],
            }))
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, 200);
    let runtime = runtime_of(&engine).await;
    assert_eq!(
        runtime
            .recent_events
            .iter()
            .find(|e| e.kind == "client")
            .unwrap()
            .client_kind,
        Some(ClientKind::Unknown),
        "Anthropic 入站 + 未知 UA = unknown"
    );

    let fake = FakeTransport::new();
    fake.push(
        "a.example.com",
        sse_ok(
            &anthropic_tool_sse()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        ),
    );
    let engine = engine_with(two_endpoint_config(), fake.clone());
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/chat/completions",
            vec![("user-agent".to_string(), "curl/8.7.1".to_string())],
            Bytes::from(
                serde_json::to_vec(&json!({
                    "model": "claude-opus-5",
                    "messages": [{"role": "user", "content": "x"}],
                }))
                .unwrap(),
            ),
        )
        .await;
    assert_eq!(response.status(), 200);
    let _ = axum::body::to_bytes(response.into_body(), usize::MAX).await;
    let runtime = runtime_of(&engine).await;
    assert_eq!(
        runtime
            .recent_events
            .iter()
            .find(|e| e.kind == "client")
            .unwrap()
            .client_kind,
        Some(ClientKind::OpenaiCompat),
        "兼容层入站 + 未知 UA = openai_compat"
    );
}

/// 规划前就被拒的请求(如模型无候选)同样带客户端类型:
/// 这类失败最需要知道是谁在发。
#[tokio::test]
async fn client_kind_survives_rejected_requests() {
    let fake = FakeTransport::new();
    let engine = engine_with(two_endpoint_config(), fake.clone());
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![(
            "user-agent".to_string(),
            "claude-cli/2.1.220 (external, cli)".to_string(),
        )],
        Bytes::from_static(b"not json"),
    )
    .await;
    assert_eq!(status, 400);

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .expect("client 事件");
    assert_eq!(client.client_kind, Some(ClientKind::ClaudeCode));
    assert_eq!(client.status_code, 400);
}

/// Native Adapter 仍执行入口模型名改写，但保留其余 Responses 字段。
#[tokio::test]
async fn native_adapter_applies_endpoint_model_alias() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    config.endpoints[0].mappings = vec![ModelMapping {
        client_pattern: "gpt-5.6-terra".into(),
        context: ContextMode::Standard,
        failover_timeout_seconds: None,
        thinking: ThinkingMode::Passthrough,
        effort: None,
        upstream_model: "provider-terra".into(),
        capabilities: Vec::new(),
    }];
    fake.push(
        "a.example.com",
        sse_ok(&["event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n"]),
    );
    let engine = engine_with(config.normalized(), fake.clone());
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            vec![("User-Agent".into(), "codex_cli_rs/0.5.0".into())],
            Bytes::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.6-terra",
                    "input": [{"type": "message", "role": "user",
                               "content": [{"type": "input_text", "text": "ls"}]}],
                    "stream": true,
                }))
                .unwrap(),
            ),
        )
        .await;
    assert_eq!(response.status(), 200);
    let _ = axum::body::to_bytes(response.into_body(), usize::MAX).await;

    let recorded = fake.requests();
    assert_eq!(recorded[0].path, "/v1/responses");
    let sent: Value = serde_json::from_slice(&recorded[0].body).unwrap();
    assert_eq!(
        sent["model"], "provider-terra",
        "Native Adapter 应只改写 model，并保留 Responses 请求形状"
    );

    // 事件也必须说实话:UI 显示的模型就是线上真正发出去的那个。
    let runtime = runtime_of(&engine).await;
    for kind in ["client", "upstream"] {
        let event = runtime
            .recent_events
            .iter()
            .find(|e| e.kind == kind)
            .expect("事件");
        assert_eq!(
            event.upstream_model.as_deref(),
            Some("provider-terra"),
            "{kind} 事件的 upstreamModel 必须与实际出站一致"
        );
    }
}

/// 分流规则先改写逻辑模型，再由入口映射解析最终上游模型。
#[tokio::test]
async fn native_adapter_applies_rule_then_endpoint_model_mapping() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    config.endpoints[0].mappings = vec![ModelMapping {
        client_pattern: "gpt-5.6-luna".into(),
        context: ContextMode::Standard,
        failover_timeout_seconds: None,
        thinking: ThinkingMode::Passthrough,
        effort: None,
        upstream_model: "provider-luna".into(),
        capabilities: Vec::new(),
    }];
    config.feature_rules = vec![FeatureRule {
        enabled: true,
        id: "classifier".into(),
        match_: FeatureRuleMatch {
            request_kind: Some(RequestKind::Classifier),
            ..FeatureRuleMatch::default()
        },
        name: "分类器".into(),
        target: FeatureRuleTarget {
            endpoint_id: None,
            effort: None,
            model: "gpt-5.6-luna".into(),
            protocol_override: None,
        },
    }];
    fake.push(
        "a.example.com",
        sse_ok(&["event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n"]),
    );
    let engine = engine_with(config.normalized(), fake.clone());
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            vec![("User-Agent".into(), "codex_cli_rs/0.5.0".into())],
            Bytes::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.6-sol",
                    "instructions": "You are a security monitor for autonomous AI coding agents.",
                    "input": [{"type": "message", "role": "user",
                               "content": [{"type": "input_text",
                                            "text": "<transcript>\nrm -rf /\n</transcript>"}]}],
                    "stream": true,
                }))
                .unwrap(),
            ),
        )
        .await;
    assert_eq!(response.status(), 200);
    let _ = axum::body::to_bytes(response.into_body(), usize::MAX).await;

    let sent: Value = serde_json::from_slice(&fake.requests()[0].body).unwrap();
    assert_eq!(
        sent["model"], "provider-luna",
        "规则逻辑模型命中入口映射后应使用最终 upstreamModel"
    );
}

/// 透传失败时也必须记 passthrough token:它描述的是「以客户端方言原样发出」这个
/// 既成事实,恰恰是失败时最该看到的线索。此前只在 2xx 记,导致上游回 400 的
/// 事件只剩一句「上游返回 HTTP 400」,只能靠复现才查得出请求是透传出去的。
#[tokio::test]
async fn dialect_passthrough_token_recorded_on_failure_too() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    config.endpoints.truncate(1);
    // 400 不可重试:不 failover、不换轮,直接回客户端。
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 400,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![
                br#"{"error":{"message":"unknown provider for model x","code":"model_not_found"}}"#
                    .to_vec(),
            ],
        },
    );
    let engine = engine_with(config.normalized(), fake.clone());
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            vec![("User-Agent".into(), "codex_cli_rs/0.5.0".into())],
            Bytes::from(
                serde_json::to_vec(&json!({
                    "model": "claude-opus-5",
                    "input": [{"type": "message", "role": "user",
                               "content": [{"type": "input_text", "text": "ls"}]}],
                    "stream": true,
                }))
                .unwrap(),
            ),
        )
        .await;
    assert_eq!(response.status(), 400);
    let _ = axum::body::to_bytes(response.into_body(), usize::MAX).await;

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .expect("client 事件");
    assert_eq!(client.status_code, 400);
    assert!(
        client
            .message
            .as_deref()
            .is_some_and(|m| m.contains("passthrough responses")),
        "400 也要记 passthrough token,实际: {:?}",
        client.message
    );
}

fn native_passthrough_config(pattern: &str) -> AppConfig {
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    // 新配置不再通过池级 globalModels 承接新规则；直接写入目标入口映射。
    config.endpoints[0].mappings.push(ModelMapping {
        client_pattern: pattern.into(),
        context: ContextMode::Standard,
        failover_timeout_seconds: None,
        thinking: ThinkingMode::Passthrough,
        effort: None,
        upstream_model: String::new(),
        capabilities: Vec::new(),
    });
    config.endpoints[1].enabled = false;
    config.normalized()
}

#[tokio::test]
async fn images_generation_alias_streams_natively_and_records_purpose() {
    let fake = FakeTransport::new();
    let upstream = concat!(
        "event: image_generation.partial_image\n",
        "data: {\"type\":\"image_generation.partial_image\",\"partial_image_index\":0}\n\n",
        "event: image_generation.completed\n",
        "data: {\"type\":\"image_generation.completed\"}\n\n",
    );
    fake.push("a.example.com", sse_ok(&[upstream]));
    let engine = engine_with(native_passthrough_config("grok-imagine-*"), fake.clone());
    let inbound = json!({
        "model": "grok-imagine-image-2.0",
        "prompt": "draw an otter",
        "aspect_ratio": "16:9",
        "resolution": "2k",
        "partial_images": 1,
        "output_format": "webp",
        "stream": true,
    });
    let (status, response) = call(
        &engine,
        loopback(),
        "/images/generations",
        vec![(
            "content-type".into(),
            "application/json; charset=utf-8".into(),
        )],
        Bytes::from(serde_json::to_vec(&inbound).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response, upstream.as_bytes());
    let recorded = fake.requests();
    assert_eq!(recorded[0].path, "/v1/images/generations");
    let sent: Value = serde_json::from_slice(&recorded[0].body).unwrap();
    assert_eq!(sent["prompt"], inbound["prompt"]);
    assert_eq!(sent["aspect_ratio"], "16:9");
    assert_eq!(sent["resolution"], "2k");
    assert_eq!(sent["partial_images"], 1);
    assert_eq!(sent["output_format"], "webp");

    let runtime = runtime_of(&engine).await;
    let client = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .unwrap();
    assert_eq!(
        client.request_purpose,
        Some(sumpter_core::routing::RequestPurpose::ImageGeneration)
    );
    assert_eq!(client.outcome, Some(RuntimeEventOutcome::Succeeded));
    assert_eq!(
        client
            .stream_trace
            .as_ref()
            .and_then(|trace| trace.terminal_event.as_deref()),
        Some("completed")
    );
}

#[tokio::test]
async fn images_json_without_model_defaults_to_gpt_image_2() {
    let fake = FakeTransport::new();
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![br#"{"created":1,"data":[]}"#.to_vec()],
        },
    );
    let engine = engine_with(native_passthrough_config("gpt-image-*"), fake.clone());
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/images/generations",
        vec![("content-type".into(), "application/json".into())],
        Bytes::from(r#"{"prompt":"draw an otter"}"#),
    )
    .await;
    assert_eq!(status, 200);
    let sent: Value = serde_json::from_slice(&fake.requests()[0].body).unwrap();
    assert_eq!(sent["model"], "gpt-image-2");
}

#[tokio::test]
async fn images_edit_multipart_preserves_body_and_content_type_byte_for_byte() {
    let fake = FakeTransport::new();
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![br#"{"created":1,"data":[]}"#.to_vec()],
        },
    );
    let engine = engine_with(native_passthrough_config("gpt-image-*"), fake.clone());
    let boundary = "sumpter-boundary";
    let mut multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngpt-image-2\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"stream\"\r\n\r\nfalse\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"input.png\"\r\n\
         Content-Type: image/png\r\n\r\n"
    )
    .into_bytes();
    multipart.extend_from_slice(&[0x89, b'P', b'N', b'G', 0, 0xff, 0x00]);
    multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let content_type = format!("multipart/form-data; boundary=\"{boundary}\"");
    let (status, _) = call(
        &engine,
        loopback(),
        "/backend-api/codex/images/edits",
        vec![("content-type".into(), content_type.clone())],
        Bytes::from(multipart.clone()),
    )
    .await;
    assert_eq!(status, 200);
    let recorded = fake.requests();
    assert_eq!(recorded[0].path, "/v1/images/edits");
    assert_eq!(recorded[0].body, multipart);
    assert_eq!(
        recorded[0]
            .headers
            .iter()
            .find(|(name, _)| name == "content-type")
            .map(|(_, value)| value.as_str()),
        Some(content_type.as_str())
    );
}

#[tokio::test]
async fn alpha_search_direct_alias_sanitizes_submission_only_fields() {
    let fake = FakeTransport::new();
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![br#"{"results":[]}"#.to_vec()],
        },
    );
    let engine = engine_with(native_passthrough_config("gpt-*"), fake.clone());
    let inbound = json!({
        "id": "search-session",
        "model": "gpt-5.6-sol",
        "query": "OpenAI",
        "prompt_cache_key": "submission-only",
        "prompt_cache_retention": "24h",
        "future_field": {"keep": true},
    });
    let (status, _) = call(
        &engine,
        loopback(),
        "/backend-api/codex/alpha/search",
        vec![("content-type".into(), "application/json".into())],
        Bytes::from(serde_json::to_vec(&inbound).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    let recorded = fake.requests();
    assert_eq!(recorded[0].path, "/v1/alpha/search");
    let sent: Value = serde_json::from_slice(&recorded[0].body).unwrap();
    assert!(sent.get("prompt_cache_key").is_none());
    assert!(sent.get("prompt_cache_retention").is_none());
    assert_eq!(sent["future_field"], inbound["future_field"]);
    assert_eq!(
        recorded[0]
            .headers
            .iter()
            .find(|(name, _)| name == "originator")
            .map(|(_, value)| value.as_str()),
        Some("codex_cli_rs")
    );
    let runtime = runtime_of(&engine).await;
    assert!(runtime.recent_events.iter().any(|event| {
        event.kind == "client"
            && event.request_purpose == Some(sumpter_core::routing::RequestPurpose::AlphaSearch)
    }));
}

#[tokio::test]
async fn legacy_completions_and_claude_count_tokens_use_native_routes() {
    let fake = FakeTransport::new();
    for body in [
        br#"{"id":"cmpl_1","choices":[{"text":"ok"}]}"#.to_vec(),
        br#"{"input_tokens":42}"#.to_vec(),
    ] {
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                chunks: vec![body],
            },
        );
    }
    let engine = engine_with(native_passthrough_config("gpt-*"), fake.clone());
    let (completion_status, _) = call(
        &engine,
        loopback(),
        "/v1/completions",
        vec![("content-type".into(), "application/json".into())],
        Bytes::from(r#"{"model":"gpt-5.6-sol","prompt":"hello","echo":true}"#),
    )
    .await;
    assert_eq!(completion_status, 200);
    let (count_status, count_body) = call(
        &engine,
        loopback(),
        "/messages/count_tokens",
        vec![("content-type".into(), "application/json".into())],
        Bytes::from(r#"{"model":"claude-opus-5","messages":[{"role":"user","content":"hello"}]}"#),
    )
    .await;
    assert_eq!(count_status, 200);
    assert_eq!(
        serde_json::from_slice::<Value>(&count_body).unwrap()["input_tokens"],
        42
    );
    assert_eq!(
        fake.requests()
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        ["/v1/completions", "/v1/messages/count_tokens"]
    );
    let runtime = runtime_of(&engine).await;
    assert!(runtime.recent_events.iter().any(|event| {
        event.kind == "client"
            && event.request_purpose == Some(sumpter_core::routing::RequestPurpose::TokenCount)
    }));
}

#[tokio::test]
async fn responses_shaped_body_on_chat_route_is_rejected() {
    let fake = FakeTransport::new();
    fake.push(
        "a.example.com",
        sse_ok(&[
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"model\":\"claude-opus-5\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ]),
    );
    let engine = engine_with(two_endpoint_config(), fake.clone());
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/chat/completions",
            vec![("content-type".into(), "application/json".into())],
            Bytes::from(
                r#"{"model":"claude-opus-5","instructions":"be concise","input":"hello","stream":true}"#,
            ),
        )
        .await;
    assert_eq!(response.status(), 400);
    let response: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(response["error"], "invalid_request");
    assert!(fake.requests().is_empty());
}

// Model groups extend candidate selection without replacing any retry policy.
#[tokio::test]
async fn endpoint_reorder_reload_routes_new_session_for_default_group_and_flat() {
    for grouped in [false, true] {
        let dir = temp_config_dir(if grouped {
            "reorder-grouped"
        } else {
            "reorder-flat"
        });
        let fake = FakeTransport::new();
        let mut config = two_endpoint_config();
        for endpoint in &mut config.endpoints {
            endpoint.priority = 0;
            endpoint.sticky_group = Some(endpoint.id.clone());
        }
        if grouped {
            config.migrate_model_groups();
        }
        config = config.normalized();
        let engine = engine_with_dir(config.clone(), dir.clone(), fake.clone());
        fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
        assert_eq!(
            call(
                &engine,
                loopback(),
                "/v1/messages",
                session_header(&stable_session("before-reorder")),
                body()
            )
            .await
            .0,
            200
        );
        assert_eq!(fake.requests().last().unwrap().host, "a.example.com");

        config.endpoints.swap(0, 1);
        // 模拟保存旧 bindings + 新 endpoints，真实 reload 负责归一化。
        let _ = dir.save_config(&config).unwrap();
        engine.reload_config().unwrap();
        fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
        assert_eq!(
            call(
                &engine,
                loopback(),
                "/v1/messages",
                session_header(&stable_session("after-reorder")),
                body()
            )
            .await
            .0,
            200
        );
        assert_eq!(fake.requests().last().unwrap().host, "b.example.com");
        // 原会话仍保留粘性，排序调整只决定新会话的初始归属。
        fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
        assert_eq!(
            call(
                &engine,
                loopback(),
                "/v1/messages",
                session_header(&stable_session("before-reorder")),
                body()
            )
            .await
            .0,
            200
        );
        assert_eq!(fake.requests().len(), 3);
        assert_eq!(fake.requests().last().unwrap().host, "a.example.com");
        drop(engine);
        let _ = std::fs::remove_dir_all(dir.root);
    }
}

fn with_model_groups(mut config: AppConfig) -> AppConfig {
    let models: Vec<String> = config.endpoints[0]
        .mappings
        .iter()
        .map(|m| m.client_pattern.clone())
        .collect();
    config.model_groups = Some(config.endpoints.iter().enumerate().map(|(i, e)| {
        serde_json::from_value(json!({"id": format!("group-{i}"), "name": format!("Group {i}"),
            "priority": i, "models": models, "bindings":[{"endpointID":e.id,"priority":0,"models":null}]})).unwrap()
    }).collect());
    config
}

#[tokio::test]
async fn model_groups_keep_500_entry_retries_then_switch_groups() {
    let fake = FakeTransport::new();
    let mut config = with_model_groups(two_endpoint_config());
    config.retry.max_500_retries = 1;
    for _ in 0..2 {
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 500,
                headers: vec![],
                chunks: vec![],
            },
        );
    }
    fake.push("b.example.com", sse_ok(&["data: {\"ok\":true}\n\n"]));
    let engine = engine_with(config, fake.clone());
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    assert_eq!(
        fake.requests()
            .iter()
            .map(|r| r.host.as_str())
            .collect::<Vec<_>>(),
        ["a.example.com", "a.example.com", "b.example.com"]
    );
    assert!(
        engine
            .runtime_snapshot()
            .recent_events
            .iter()
            .any(|e| e.model_group_id.as_deref() == Some("group-1"))
    );
}

#[tokio::test]
async fn model_groups_preserve_disabled_500_failover() {
    let fake = FakeTransport::new();
    let mut config = with_model_groups(two_endpoint_config());
    config.retry.failover_on_500 = false;
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 500,
            headers: vec![],
            chunks: vec![],
        },
    );
    let engine = engine_with(config, fake.clone());
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 500);
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn model_groups_preserve_deferred_rounds_and_same_model() {
    let fake = FakeTransport::new();
    let mut config = with_model_groups(two_endpoint_config());
    config.retry.max_deferred_rounds = 2;
    for host in ["a.example.com", "b.example.com"] {
        fake.push(
            host,
            Outcome::Status {
                status: 429,
                headers: vec![],
                chunks: vec![],
            },
        );
    }
    fake.push("a.example.com", sse_ok(&["data: {\"ok\":true}\n\n"]));
    let engine = engine_with(config, fake.clone());
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    assert_eq!(
        fake.requests()
            .iter()
            .map(|r| r.host.as_str())
            .collect::<Vec<_>>(),
        ["a.example.com", "b.example.com", "a.example.com"]
    );
    let models: Vec<_> = fake
        .requests()
        .iter()
        .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap()["model"].clone())
        .collect();
    assert!(models.windows(2).all(|p| p[0] == p[1]));
}
