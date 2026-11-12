use sumpter_core::config::{ListenerConfig, ListenerTlsConfig};

#[test]
fn optional_tls_preserves_old_wire_and_has_an_independent_port() {
    let old: ListenerConfig = serde_json::from_str(r#"{"port":57878}"#).unwrap();
    assert!(old.https_port().is_none());
    assert!(serde_json::to_value(&old).unwrap().get("tls").is_none());
    let mut listener = old;
    listener.tls = Some(ListenerTlsConfig {
        enabled: true,
        certificate_path: "/tmp/cert.pem".into(),
        private_key_path: "/tmp/key.pem".into(),
        ..Default::default()
    });
    listener.validate_tls().unwrap();
    assert_eq!(listener.https_port(), Some(57880));
    assert_eq!(
        serde_json::from_value::<ListenerConfig>(serde_json::to_value(&listener).unwrap()).unwrap(),
        listener
    );
    listener.tls.as_mut().unwrap().port = listener.port;
    assert!(listener.validate_tls().is_err());
    listener.tls.as_mut().unwrap().enabled = false;
    listener.validate_tls().unwrap();
}
