use std::sync::Arc;
use sumpter_core::config::AppConfig;
use sumpter_engine::replay::ReplayTransport;
use sumpter_linux_adapter::{Engine, admin::ProxySupervisor};

#[path = "../../../../tests/support/tls.rs"]
mod tls;

#[tokio::test]
async fn failed_https_bind_restores_http_and_does_not_replace_config() {
    let certificates = tls::CertificateFiles::new();
    let mut config = AppConfig::bootstrap();
    config.listener.port = i64::from(tls::free_port());
    let engine = Engine::new(config.clone(), None, Arc::new(ReplayTransport::new([])));
    let supervisor = ProxySupervisor::new(engine.clone());
    supervisor.start().await.unwrap();
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut candidate = config.clone();
    candidate.listener.tls =
        Some(certificates.settings(i64::from(occupied.local_addr().unwrap().port())));
    assert!(supervisor.apply_config(candidate).await.is_err());
    assert_eq!(engine.config().listener, config.listener);
    assert!(supervisor.status().await.running);
    assert_eq!(
        certificates
            .client()
            .get(format!(
                "http://127.0.0.1:{}/v1/models",
                config.listener.port
            ))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    supervisor.stop().await;
}
