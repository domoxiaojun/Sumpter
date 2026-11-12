//! 入站 HTTP 服务:axum 全路径接入引擎(路径分发在引擎内,对齐 Swift 版
//! 「server 不认路径」)。keep-alive 保留(hyper 天然),客户端断开经响应体
//! drop 传播取消(见 engine.rs)。

use std::net::{IpAddr, SocketAddr};

use axum::Router;
use axum::body::Body;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::{ConnectInfo, State, WebSocketUpgrade};
use axum::http::{HeaderMap, Method, Request, StatusCode, Uri};
use axum::response::Response;
use sumpter_core::access;
use sumpter_engine::engine::inbound::is_realtime_websocket_path;

use crate::engine::Engine;

pub fn router(engine: Engine) -> Router {
    Router::new()
        // HTTP keeps its original path/method; only Realtime/Live may upgrade.
        .fallback(dispatch)
        .layer(axum::extract::DefaultBodyLimit::disable())
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
    // Only a GET can enter the WebSocket upgrade path. POST Realtime/Live
    // requests remain ordinary HTTP bootstraps even if a proxy leaked Upgrade
    // headers onto them.
    if request.method() == Method::GET
        && is_realtime_websocket_path(uri.path())
        && let Ok(upgrade) = upgrade
    {
        let shutdown = request
            .extensions()
            .get::<tokio_util::sync::CancellationToken>()
            .cloned()
            .unwrap_or_default();
        let path_and_query = uri
            .path_and_query()
            .map(|value| value.as_str().to_string())
            .unwrap_or_else(|| "/".into());
        let authorized = engine.authorize_realtime_websocket(remote_ip, &pairs);
        if !authorized {
            let cidr_allowed = access::is_allowed(
                remote_ip.map(|ip| ip.to_string()).as_deref(),
                &engine.config().listener.allowed_cidrs,
            );
            engine.record_rejected_websocket(
                remote_ip,
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
        let prepared = match engine
            .prepare_realtime_websocket(remote_ip, &path_and_query, &pairs)
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                return sumpter_engine::engine::websocket_prepare_error_response(&error);
            }
        };
        let upgrade = select_websocket_protocol(upgrade, &pairs)
            .max_frame_size(prepared.max_frame_bytes())
            .max_message_size(prepared.max_message_bytes());
        let ws_engine = engine.clone();
        return upgrade.on_upgrade(move |socket| async move {
            ws_engine
                .handle_prepared_websocket_with_shutdown(socket, remote_ip, prepared, shutdown)
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

/// 启动监听(返回实际绑定地址与 serve future 的 JoinHandle)。
pub async fn serve(
    engine: Engine,
    address: SocketAddr,
) -> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let https = sumpter_engine::inbound_tls::prepare_https(address, &engine.config().listener)?;
    let listeners = sumpter_engine::inbound_tls::bind_proxy_listeners(address, https).await?;
    serve_bound_proxy_router(router(engine), listeners)
}

/// 通用 Router 监听(admin listener 复用)。
pub async fn serve_router(
    app: Router,
    address: SocketAddr,
) -> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listeners = sumpter_engine::inbound_tls::bind_proxy_listeners(address, None).await?;
    serve_bound_proxy_router(app, listeners)
}

fn serve_bound_proxy_router(
    app: Router,
    listeners: sumpter_engine::inbound_tls::BoundProxyListeners,
) -> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let (local, _) = listeners.addresses()?;
    // Upgraded callbacks outlive axum's listener task. Abort must also wake
    // first-message waits and active relays instead of leaving orphan sockets.
    let shutdown = tokio_util::sync::CancellationToken::new();
    let app = app.layer(axum::Extension(shutdown.clone()));
    let guard = shutdown.clone().drop_guard();
    let handle = tokio::spawn(async move {
        let _guard = guard;
        let _ = sumpter_engine::inbound_tls::serve_proxy_bound(app, listeners, shutdown).await;
    });
    Ok((local, handle))
}
