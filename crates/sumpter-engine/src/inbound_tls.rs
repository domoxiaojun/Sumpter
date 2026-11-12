//! Optional parallel HTTPS listener. The HTTP proxy and admin listeners stay HTTP.

use std::io::{self, Read};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::serve::{Listener, ListenerExt};
use sha2::{Digest, Sha256};
use sumpter_core::config::ListenerConfig;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use tokio_rustls::server::TlsStream;
use tokio_util::sync::CancellationToken;

const MAX_PEM_BYTES: u64 = 1024 * 1024;
const MAX_HANDSHAKES: usize = 64;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct LoadedTls {
    config: Arc<rustls::ServerConfig>,
    fingerprint: [u8; 32],
}

pub fn same_tls(left: Option<&LoadedTls>, right: Option<&LoadedTls>) -> bool {
    left.map(|tls| tls.fingerprint) == right.map(|tls| tls.fingerprint)
}

pub fn prepare_https(
    http: SocketAddr,
    listener: &ListenerConfig,
) -> io::Result<Option<(SocketAddr, LoadedTls)>> {
    let tls = load_listener_tls(listener)?;
    Ok(listener
        .https_port()
        .zip(tls)
        .map(|(port, tls)| (SocketAddr::new(http.ip(), port as u16), tls)))
}

pub struct BoundProxyListeners {
    pub http: TcpListener,
    pub https: Option<(TcpListener, LoadedTls)>,
}

impl BoundProxyListeners {
    pub fn addresses(&self) -> io::Result<(SocketAddr, Option<SocketAddr>)> {
        Ok((
            self.http.local_addr()?,
            self.https
                .as_ref()
                .map(|(tcp, _)| tcp.local_addr())
                .transpose()?,
        ))
    }
}

/// Bind all sockets before exposing either one. Dropping a failed preparation
/// releases every newly bound socket and leaves the current runtime untouched.
pub async fn bind_proxy_listeners(
    http: SocketAddr,
    https: Option<(SocketAddr, LoadedTls)>,
) -> io::Result<BoundProxyListeners> {
    let http = TcpListener::bind(http).await?;
    let https = match https {
        Some((address, tls)) => Some((TcpListener::bind(address).await?, tls)),
        None => None,
    };
    Ok(BoundProxyListeners { http, https })
}

pub async fn serve_proxy_bound(
    app: Router,
    listeners: BoundProxyListeners,
    shutdown: CancellationToken,
) -> io::Result<()> {
    if let Some((https, tls)) = listeners.https {
        let (http_result, https_result) = tokio::join!(
            async {
                let result = serve_bound(app.clone(), listeners.http, None, shutdown.clone()).await;
                shutdown.cancel();
                result
            },
            async {
                let result = serve_bound(app.clone(), https, Some(tls), shutdown.clone()).await;
                shutdown.cancel();
                result
            }
        );
        http_result.and(https_result)
    } else {
        serve_bound(app, listeners.http, None, shutdown).await
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn read_pem(path: &str, kind: &str) -> io::Result<Vec<u8>> {
    let file =
        std::fs::File::open(path).map_err(|_| invalid(&format!("无法读取 HTTPS {kind}文件")))?;
    let mut bytes = Vec::new();
    file.take(MAX_PEM_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid(&format!("无法读取 HTTPS {kind}文件")))?;
    if bytes.len() as u64 > MAX_PEM_BYTES {
        bytes.fill(0);
        return Err(invalid("HTTPS PEM 文件不能超过 1 MiB"));
    }
    Ok(bytes)
}

/// Load before binding/stopping listeners. Errors never contain PEM contents.
/// ring is selected explicitly instead of installing a process-global provider.
pub fn load_listener_tls(listener: &ListenerConfig) -> io::Result<Option<LoadedTls>> {
    listener
        .validate_tls()
        .map_err(|message| invalid(&message))?;
    let Some(tls) = listener.tls.as_ref().filter(|tls| tls.enabled) else {
        return Ok(None);
    };
    let certificates = read_pem(&tls.certificate_path, "证书链")?;
    let chain = CertificateDer::pem_slice_iter(&certificates)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid("HTTPS 证书链不是有效的 PEM"))?;
    if chain.is_empty() {
        return Err(invalid("HTTPS 证书链为空"));
    }
    let mut secret = read_pem(&tls.private_key_path, "私钥")?;
    let parsed_key = PrivateKeyDer::from_pem_slice(&secret);
    secret.fill(0);
    let key = parsed_key.map_err(|_| invalid("HTTPS 私钥必须为未加密的 PEM 私钥"))?;
    let mut fingerprint = Sha256::new();
    fingerprint.update(&certificates);
    fingerprint.update(key.secret_der());
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| invalid("HTTPS TLS 协议配置失败"))?
    .with_no_client_auth()
    .with_single_cert(chain, key)
    .map_err(|_| invalid("HTTPS 证书与私钥无效或不匹配"))?;
    // HTTP/1.1 covers streaming responses and RFC 6455 WebSocket upgrades.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Some(LoadedTls {
        config: Arc::new(config),
        fingerprint: fingerprint.finalize().into(),
    }))
}

struct TlsListener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    handshakes: JoinSet<Option<(TlsStream<TcpStream>, SocketAddr)>>,
}

impl Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            tokio::select! {
                result = self.handshakes.join_next(), if !self.handshakes.is_empty() => {
                    if let Some(Ok(Some(accepted))) = result {
                        return accepted;
                    }
                }
                accepted = self.tcp.accept(), if self.handshakes.len() < MAX_HANDSHAKES => {
                    match accepted {
                        Ok((stream, peer)) => {
                            let acceptor = self.acceptor.clone();
                            // A slow/malformed peer cannot block subsequent TCP accepts.
                            self.handshakes.spawn(async move {
                                tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream))
                                    .await.ok()?.ok().map(|stream| (stream, peer))
                            });
                        }
                        Err(error) => {
                            tracing::warn!(%error, "HTTPS TCP accept failed");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.tcp.local_addr()
    }
}

pub async fn serve_bound(
    app: Router,
    listener: TcpListener,
    tls: Option<LoadedTls>,
    shutdown: CancellationToken,
) -> io::Result<()> {
    match tls {
        Some(tls) => {
            let listener = TlsListener {
                tcp: listener,
                acceptor: TlsAcceptor::from(tls.config),
                handshakes: JoinSet::new(),
            };
            // TapIo's generic Connected implementation preserves the real TCP peer.
            axum::serve(
                listener.tap_io(|_| {}),
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
        }
        None => {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
        }
    }
}
