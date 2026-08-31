//! 入站 HTTP 服务:axum 全路径接入引擎(路径分发在引擎内,对齐 Swift 版
//! 「server 不认路径」)。keep-alive 保留(hyper 天然),客户端断开经响应体
//! drop 传播取消(见 engine.rs)。

use std::net::{IpAddr, SocketAddr};

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::Request;
use axum::response::Response;

use crate::engine::{Engine, MAX_BODY_BYTES};

pub fn router(engine: Engine) -> Router {
    Router::new()
        .fallback(handle)
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(engine)
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
    serve_router(router(engine), address).await
}

/// 通用 Router 监听(admin listener 复用)。
pub async fn serve_router(
    app: Router,
    address: SocketAddr,
) -> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(address).await?;
    let local = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    Ok((local, handle))
}
