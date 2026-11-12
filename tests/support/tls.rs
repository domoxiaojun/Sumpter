use std::path::PathBuf;
use std::process::{Command, Stdio};

use sumpter_core::config::ListenerTlsConfig;

pub struct CertificateFiles {
    root: PathBuf,
    pub certificate: PathBuf,
    pub key: PathBuf,
}

impl CertificateFiles {
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(format!("sumpter-tls-test-{}", rand::random::<u64>()));
        std::fs::create_dir(&root).unwrap();
        let files = Self {
            certificate: root.join("cert.pem"),
            key: root.join("key.pem"),
            root,
        };
        let config = files.root.join("openssl.cnf");
        std::fs::write(&config, "[req]\nprompt=no\ndistinguished_name=dn\nx509_extensions=ext\n[dn]\nCN=localhost\n[ext]\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\n").unwrap();
        assert!(
            Command::new("openssl")
                .args([
                    "req", "-x509", "-nodes", "-newkey", "rsa:2048", "-days", "1", "-config"
                ])
                .arg(config)
                .arg("-keyout")
                .arg(&files.key)
                .arg("-out")
                .arg(&files.certificate)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
        files
    }

    pub fn settings(&self, port: i64) -> ListenerTlsConfig {
        ListenerTlsConfig {
            enabled: true,
            port,
            certificate_path: self.certificate.to_string_lossy().into_owned(),
            private_key_path: self.key.to_string_lossy().into_owned(),
        }
    }

    pub fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .add_root_certificate(
                reqwest::Certificate::from_pem(&std::fs::read(&self.certificate).unwrap()).unwrap(),
            )
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .unwrap()
    }
}

impl Drop for CertificateFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
