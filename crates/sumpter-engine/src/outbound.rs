//! 出站传输层:pinned IP + SNI 分离的 HTTPS 客户端。对齐 Swift `PinnedHTTPSClient`
//! 的行为面(specs/spec-engine.md §2),实现换成 reqwest + rustls:
//!
//! - **TCP 连 IP、SNI/证书校验名/Host 头用域名**:`resolve(host, ip)` 只覆盖 DNS,
//!   URL 域名不变,SNI/校验/Host 自动都是域名。
//! - **恒 TLS**:http baseURL 也强制按 https 连(Swift 同款)。
//! - **连接不复用**(`pool_max_idle_per_host(0)`)、**http1 only**(Swift 是手写 h1 栈,
//!   锁 h1 保行为一致)、**不跟随重定向**(3xx 原样透传)、**不走系统代理**、**不自动解压**。
//! - 超时归属:响应头截止由调用方传入(此处 timeout 包 connect+写+读头);
//!   流式块间空闲超时在引擎层逐 chunk 包。
//! - 取消:future drop 即撕连接(tokio 语义),对应 Swift 的 cancel → connectionFailed。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;

/// 传输错误分类 —— failover 判定依据,语义对齐 Swift `TransportError`。
#[derive(Debug, Clone, thiserror::Error)]
pub enum TransportError {
    /// 响应头截止超时(连接/写/读头整段)。
    #[error("timeout")]
    Timeout,
    /// 连接建立/传输失败(含对端重置、取消)。
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    /// 无法构造请求或响应形状非法。
    #[error("invalid response: {0}")]
    InvalidResponse(String),
}

impl TransportError {
    /// 只把可能随网络/上游恢复而自愈的首响应前失败带入下一整轮。
    /// InvalidResponse 来自非法 URL/method/header 等确定性本地配置错误，重放无意义。
    pub fn is_retryable_before_response(&self) -> bool {
        matches!(self, Self::Timeout | Self::ConnectionFailed(_))
    }
}

#[derive(Debug, Clone)]
pub struct OutboundRequest {
    pub method: String,
    /// endpoint.baseURL(scheme/host/port/根路径来源)。
    pub base_url: String,
    /// 拼接好的路径+query(base path 与入站路径已去重斜杠)。
    pub path_and_query: String,
    /// 已构造完毕的出站 header(黑名单/强制项由引擎处理)。
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// TCP 直连的固定 IP;None = 正常 DNS。
    pub pinned_ip: Option<String>,
    /// 【实验】连接复用(Endpoint.keepAlive):false = 每请求新建连接。
    pub keep_alive: bool,
}

pub struct UpstreamResponse {
    pub status: u16,
    /// 响应头,key 已小写。
    pub headers: Vec<(String, String)>,
    pub stream: BoxStream<'static, Result<Bytes, TransportError>>,
}

#[async_trait::async_trait]
pub trait UpstreamTransport: Send + Sync {
    /// 发出请求并在拿到响应头后返回;body 以流暴露。
    /// `response_timeout` = 响应头截止(None 不限)。
    async fn send_streaming(
        &self,
        request: OutboundRequest,
        response_timeout: Option<Duration>,
    ) -> Result<UpstreamResponse, TransportError>;
}

/// 目标解析:恒 https;端口 = baseURL 端口 ?? 443。
pub struct ResolvedTarget {
    pub host: String,
    pub port: u16,
    pub url: reqwest::Url,
}

pub fn resolve_target(
    base_url: &str,
    path_and_query: &str,
) -> Result<ResolvedTarget, TransportError> {
    let parsed = reqwest::Url::parse(base_url)
        .map_err(|e| TransportError::InvalidResponse(format!("invalid base URL: {e}")))?;
    // 按 baseURL scheme 决定明文/TLS:http = 明文(本地 LLM/内网上游),https = TLS。
    // 【Rust 变更】Swift 老版恒 TLS;明文支持是用户点名的功能增强。
    let scheme = match parsed.scheme() {
        "http" => "http",
        "https" => "https",
        other => {
            return Err(TransportError::InvalidResponse(format!(
                "unsupported scheme: {other}"
            )));
        }
    };
    let host = parsed
        .host_str()
        .ok_or_else(|| TransportError::InvalidResponse("base URL has no host".into()))?
        .to_string();
    let port = parsed
        .port()
        .unwrap_or(if scheme == "http" { 80 } else { 443 });
    let url = reqwest::Url::parse(&format!("{scheme}://{host}:{port}{path_and_query}"))
        .map_err(|e| TransportError::InvalidResponse(format!("invalid outbound URL: {e}")))?;
    Ok(ResolvedTarget { host, port, url })
}

/// base path 与入站路径拼接,斜杠去重(对齐 Swift 路径拼接)。
pub fn join_paths(base_path: &str, inbound_path: &str) -> String {
    let base = base_path.trim_end_matches('/');
    if inbound_path.is_empty() {
        return if base.is_empty() {
            "/".into()
        } else {
            base.to_string()
        };
    }
    let inbound = if inbound_path.starts_with('/') {
        inbound_path.to_string()
    } else {
        format!("/{inbound_path}")
    };
    format!("{base}{inbound}")
}

type ClientKey = (String, u16, Option<String>, bool);

pub struct ReqwestTransport {
    clients: Mutex<HashMap<ClientKey, reqwest::Client>>,
}

impl ReqwestTransport {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
        }
    }

    fn client_for(
        &self,
        host: &str,
        port: u16,
        pinned_ip: Option<&str>,
        keep_alive: bool,
    ) -> Result<reqwest::Client, TransportError> {
        let key: ClientKey = (
            host.to_string(),
            port,
            pinned_ip.map(str::to_string),
            keep_alive,
        );
        if let Some(client) = self.clients.lock().unwrap().get(&key) {
            return Ok(client.clone());
        }
        let mut builder = reqwest::Client::builder()
            .use_rustls_tls()
            .http1_only()
            .redirect(reqwest::redirect::Policy::none())
            .tcp_nodelay(true)
            .no_proxy();
        // 【实验】keepAlive 入口:小池 + 90s 空闲回收,省去每次 TCP+TLS 握手
        // (远程中转实测 ~100ms);默认仍关池对齐手写栈行为面。
        builder = if keep_alive {
            builder
                .pool_max_idle_per_host(2)
                .pool_idle_timeout(Duration::from_secs(90))
        } else {
            builder.pool_max_idle_per_host(0)
        };
        if let Some(ip) = pinned_ip {
            let addr: SocketAddr = format!("{ip}:{port}")
                .parse()
                .or_else(|_| format!("[{ip}]:{port}").parse())
                .map_err(|_| TransportError::ConnectionFailed(format!("invalid pinned IP {ip}")))?;
            builder = builder.resolve(host, addr);
        }
        let client = builder
            .build()
            .map_err(|e| TransportError::ConnectionFailed(format!("client build: {e}")))?;
        self.clients.lock().unwrap().insert(key, client.clone());
        Ok(client)
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

const MAX_ERROR_CHAIN_CHARS: usize = 1_024;
const MAX_ERROR_CHAIN_DEPTH: usize = 8;

/// reqwest 的顶层 Display 往往只有 `error sending request for url (...)`，真正的
/// TCP/TLS/EOF 原因在 source chain 里。URL 先由 `without_url` 移除，避免 query 中的
/// 临时签名进入 stats/admin UI；错误链再做深度和长度上限。
fn bounded_error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = Vec::new();
    let mut current = Some(error);
    while let Some(item) = current.take() {
        let rendered = item
            .to_string()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !rendered.is_empty() && parts.last() != Some(&rendered) {
            parts.push(rendered);
        }
        if parts.len() >= MAX_ERROR_CHAIN_DEPTH {
            break;
        }
        current = item.source();
    }
    let joined = parts.join(": ");
    if joined.chars().count() <= MAX_ERROR_CHAIN_CHARS {
        joined
    } else {
        let mut truncated: String = joined.chars().take(MAX_ERROR_CHAIN_CHARS - 1).collect();
        truncated.push('…');
        truncated
    }
}

fn classify(e: reqwest::Error) -> TransportError {
    let is_timeout = e.is_timeout();
    let sanitized = e.without_url();
    if is_timeout {
        TransportError::Timeout
    } else {
        TransportError::ConnectionFailed(bounded_error_chain(&sanitized))
    }
}

#[async_trait::async_trait]
impl UpstreamTransport for ReqwestTransport {
    async fn send_streaming(
        &self,
        request: OutboundRequest,
        response_timeout: Option<Duration>,
    ) -> Result<UpstreamResponse, TransportError> {
        let target = resolve_target(&request.base_url, &request.path_and_query)?;
        let client = self.client_for(
            &target.host,
            target.port,
            request.pinned_ip.as_deref(),
            request.keep_alive,
        )?;

        let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|_| {
            TransportError::InvalidResponse(format!("bad method {}", request.method))
        })?;
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in &request.headers {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| TransportError::InvalidResponse(format!("bad header name {name}")))?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| TransportError::InvalidResponse("bad header value".into()))?;
            headers.append(name, value);
        }

        let send = client
            .request(method, target.url)
            .headers(headers)
            .body(request.body)
            .send();

        let response = match response_timeout {
            Some(deadline) => tokio::time::timeout(deadline, send)
                .await
                .map_err(|_| TransportError::Timeout)?
                .map_err(classify)?,
            None => send.await.map_err(classify)?,
        };

        let status = response.status().as_u16();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_lowercase(),
                    String::from_utf8_lossy(v.as_bytes()).to_string(),
                )
            })
            .collect();
        let stream = response
            .bytes_stream()
            .map(|item| item.map_err(classify))
            .boxed();
        Ok(UpstreamResponse {
            status,
            headers,
            stream,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_paths_deduplicates_slashes() {
        assert_eq!(join_paths("", "/v1/messages"), "/v1/messages");
        assert_eq!(join_paths("/", "/v1/messages"), "/v1/messages");
        assert_eq!(
            join_paths("/apps/anthropic", "/v1/messages"),
            "/apps/anthropic/v1/messages"
        );
        assert_eq!(
            join_paths("/apps/anthropic/", "/v1/messages"),
            "/apps/anthropic/v1/messages"
        );
        assert_eq!(join_paths("/base", ""), "/base");
        assert_eq!(join_paths("", ""), "/");
    }

    #[test]
    fn resolve_target_honours_scheme_and_default_port() {
        // https:默认 443(reqwest::Url 规范化掉默认端口,实际仍连 443)。
        let t = resolve_target("https://example.com", "/v1/messages").unwrap();
        assert_eq!(t.host, "example.com");
        assert_eq!(t.port, 443);
        assert_eq!(t.url.as_str(), "https://example.com/v1/messages");

        // http:明文 + 默认 80(本地 LLM / 内网上游)。
        let t = resolve_target("http://127.0.0.1", "/v1/messages").unwrap();
        assert_eq!(t.port, 80);
        assert_eq!(t.url.scheme(), "http");
        assert_eq!(t.url.as_str(), "http://127.0.0.1/v1/messages");

        // http + 显式端口(Ollama/vLLM 常见 11434/8000)。
        let t = resolve_target("http://localhost:11434", "/v1/messages").unwrap();
        assert_eq!(t.port, 11434);
        assert_eq!(t.url.as_str(), "http://localhost:11434/v1/messages");

        let t = resolve_target("https://example.com:8443/base", "/base/v1/messages").unwrap();
        assert_eq!(t.port, 8443);
        assert_eq!(t.url.path(), "/base/v1/messages");

        assert!(resolve_target("not a url", "/x").is_err());
        assert!(resolve_target("ftp://example.com", "/x").is_err());
    }

    #[test]
    fn bounded_error_chain_keeps_nested_cause() {
        let nested = std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            std::io::Error::other("TLS peer closed during handshake"),
        );
        let rendered = bounded_error_chain(&nested);
        assert!(rendered.contains("TLS peer closed during handshake"));
        assert!(rendered.chars().count() <= MAX_ERROR_CHAIN_CHARS);
    }
}
