//! 入站 HTTP 服务:axum 全路径接入引擎(路径分发在引擎内,对齐 Swift 版
//! 「server 不认路径」)。keep-alive 保留(hyper 天然),客户端断开经响应体
//! drop 传播取消(见 engine.rs)。

use std::net::{IpAddr, SocketAddr};

use axum::Router;
use axum::body::Body;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::{ConnectInfo, State, WebSocketUpgrade};
use axum::http::{HeaderMap, Method, Request, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::Response;
use futures_util::StreamExt;
use sumpter_core::access;
use tokio_util::sync::CancellationToken;

use crate::engine::{Engine, MAX_BODY_BYTES};

/// 同时控制 accept loop 与已接入请求/响应体的服务句柄。
pub struct ServerHandle {
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl ServerHandle {
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// 停止接入并等待所有连接任务观察到取消；流式响应的 Body 会先被 drop，
    /// 因而 Engine 的 CompletionGuard 会在本 future 返回前完成 499 记账。
    pub async fn shutdown(mut self) {
        self.shutdown.cancel();
        let _ = (&mut self.task).await;
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.task.abort();
    }
}

pub fn router(engine: Engine) -> Router {
    Router::new()
        // One data-plane dispatcher: there are no compatibility aliases. Any
        // path/method is forwarded as received; WebSocket upgrades are
        // selected by the request headers rather than a path allow-list.
        .fallback(dispatch)
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(engine)
}

async fn dispatch(
    State(engine): State<Engine>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    request: Request<Body>,
) -> Response {
    let pairs = sumpter_engine::engine::inbound::header_pairs(&headers);
    let remote_ip = Some(remote.ip());
    // RFC 6455 upgrades are GET requests. A POST Live/Realtime bootstrap may
    // carry upgrade-like headers from an intermediary, but it must retain its
    // HTTP method and reach the engine's CPA-compatible bootstrap path.
    if request.method() == Method::GET
        && let Ok(upgrade) = upgrade
    {
        let path_and_query = uri
            .path_and_query()
            .map(|value| value.as_str().to_string())
            .unwrap_or_else(|| "/".into());
        let realtime = is_realtime_path(&path_and_query);
        let authorized = if realtime {
            engine.authorize_realtime_websocket(remote_ip, &pairs)
        } else {
            engine.authorize_websocket(remote_ip, &pairs)
        };
        if !authorized {
            let cidr_allowed = access::is_allowed(
                remote_ip.map(|ip| ip.to_string()).as_deref(),
                &engine.config().listener.allowed_cidrs,
            );
            engine.record_rejected_websocket(
                &path_and_query,
                &pairs,
                if cidr_allowed { 401 } else { 403 },
                if cidr_allowed {
                    "inbound_auth_required"
                } else {
                    "client_forbidden"
                },
            );
            return Response::builder()
                .status(if cidr_allowed {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                })
                .header("content-type", "application/json")
                .body(Body::from(if engine.config().listener.has_inbound_auth() {
                    r#"{"error":"inbound_auth_required"}"#
                } else {
                    r#"{"error":"client_forbidden"}"#
                }))
                .unwrap_or_else(|_| Response::new(Body::empty()));
        }
        let upgrade = select_websocket_protocol(upgrade, &pairs);
        if realtime {
            let prepared = match engine
                .prepare_realtime_websocket(&path_and_query, &pairs)
                .await
            {
                Ok(prepared) => prepared,
                Err(error) => {
                    return sumpter_engine::engine::websocket_prepare_error_response(&error);
                }
            };
            let ws_engine = engine.clone();
            return upgrade.on_upgrade(move |socket| async move {
                ws_engine
                    .handle_prepared_websocket(socket, remote_ip, prepared)
                    .await;
            });
        }
        let ws_engine = engine.clone();
        return upgrade.on_upgrade(move |socket| async move {
            ws_engine
                .handle_websocket(socket, remote_ip, path_and_query, pairs)
                .await;
        });
    }

    handle(State(engine), ConnectInfo(remote), request).await
}

async fn handle(
    State(engine): State<Engine>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Response {
    let method = request.method().as_str().to_string();
    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).to_string(),
            )
        })
        .collect();
    let body = request.into_body();
    engine
        .handle_request(
            Some(remote_ip(remote)),
            &method,
            &path_and_query,
            headers,
            body,
        )
        .await
}

fn remote_ip(addr: SocketAddr) -> IpAddr {
    addr.ip()
}

fn is_realtime_path(path_and_query: &str) -> bool {
    let path = path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _)| path);
    [
        "/v1/realtime",
        "/realtime",
        "/openai/v1/realtime",
        "/backend-api/codex/realtime",
        "/v1/live",
        "/live",
        "/openai/v1/live",
        "/backend-api/codex/live",
    ]
    .iter()
    .any(|prefix| {
        path == *prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

fn select_websocket_protocol(
    upgrade: WebSocketUpgrade,
    headers: &[(String, String)],
) -> WebSocketUpgrade {
    let protocols = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("sec-websocket-protocol"))
        .map(|(_, value)| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if protocols.is_empty() {
        upgrade
    } else {
        upgrade.protocols(protocols)
    }
}

/// 绑定 proxy listener。host 语义对齐 Swift:空/`0.0.0.0`/`::` = 全接口。
pub fn bind_address(host: &str, port: u16) -> SocketAddr {
    let trimmed = host.trim();
    let ip: IpAddr = if trimmed.is_empty() || trimmed == "0.0.0.0" {
        IpAddr::from([0, 0, 0, 0])
    } else if trimmed == "::" {
        IpAddr::from([0u16, 0, 0, 0, 0, 0, 0, 0])
    } else {
        trimmed.parse().unwrap_or(IpAddr::from([127, 0, 0, 1]))
    };
    SocketAddr::new(ip, port)
}

/// 启动监听（返回实际绑定地址与可等待关停的服务句柄）。
pub async fn serve(
    engine: Engine,
    address: SocketAddr,
) -> std::io::Result<(SocketAddr, ServerHandle)> {
    serve_router(router(engine), address).await
}

/// 通用 Router 监听(admin listener 复用)。
pub async fn serve_router(
    app: Router,
    address: SocketAddr,
) -> std::io::Result<(SocketAddr, ServerHandle)> {
    let (local, listener) = bind_listener(address).await?;
    let handle = serve_bound_router(app, listener);
    Ok((local, handle))
}

/// 先绑定但暂不接入请求。配置事务用它在替换 Engine 前证明新地址可用。
pub async fn bind_listener(
    address: SocketAddr,
) -> std::io::Result<(SocketAddr, tokio::net::TcpListener)> {
    let listener = tokio::net::TcpListener::bind(address).await?;
    let local = listener.local_addr()?;
    Ok((local, listener))
}

pub fn serve_bound_router(app: Router, listener: tokio::net::TcpListener) -> ServerHandle {
    let shutdown = CancellationToken::new();
    let request_shutdown = shutdown.clone();
    let app = app.layer(middleware::from_fn(move |request, next| {
        shutdown_aware(request_shutdown.clone(), request, next)
    }));
    let serve_shutdown = shutdown.clone();
    let task = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(serve_shutdown.cancelled_owned())
        .await;
    });
    ServerHandle { shutdown, task }
}

async fn shutdown_aware(
    shutdown: CancellationToken,
    request: Request<Body>,
    next: Next,
) -> Response {
    let body_shutdown = shutdown.clone();
    let response = tokio::select! {
        _ = shutdown.cancelled() => {
            return Response::builder()
                .status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
                .body(Body::empty())
                .unwrap_or_else(|_| Response::new(Body::empty()));
        }
        response = next.run(request) => response,
    };
    let (mut parts, body) = response.into_parts();
    // Body::from_stream 没有原 Body 的精确 size hint；删除 Content-Length，避免
    // shutdown 恰好截断响应时留下与实际帧数不一致的长度。
    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    let body = Body::from_stream(
        body.into_data_stream()
            .take_until(body_shutdown.cancelled_owned()),
    );
    Response::from_parts(parts, body)
}
