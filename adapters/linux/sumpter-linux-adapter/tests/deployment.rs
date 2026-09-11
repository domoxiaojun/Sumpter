//! 部署模板契约：`platforms/linux/compose.yaml` 里 init 服务写入的 bootstrap 配置，
//! 必须能被真实 daemon 的加载与校验路径接受。
//!
//! 与 `engine.rs` 直接引用 `platforms/linux/scripts/` 的做法一致，这里直接引用部署模板，
//! 避免测试和模板各维护一份 JSON。

use sumpter_core::config::SCHEMA_VERSION;
use sumpter_core::config_store::ConfigDir;
use sumpter_linux_adapter::admin::validate_config;

const COMPOSE: &str = include_str!("../../../../platforms/linux/compose.yaml");

/// 取出 init 的 shell 里那一行 JSON 字面量（`printf '%s\n' '{...}'`）。
fn bootstrap_config_json() -> String {
    let line = COMPOSE
        .lines()
        .find(|line| line.contains("'{") && line.contains("schemaVersion"))
        .expect("compose.yaml 的 init 服务必须生成 bootstrap config.json");
    let start = line.find("'{").expect("缺少 JSON 起始引号") + 1;
    let end = line.rfind("}'").expect("缺少 JSON 结束引号") + 1;
    line[start..end].to_string()
}

#[test]
fn compose_bootstrap_config_loads_and_validates() {
    let json = bootstrap_config_json();
    let dir = std::env::temp_dir().join(format!("sumpter-deploy-bootstrap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), &json).unwrap();

    let config = ConfigDir::new(&dir)
        .load_config()
        .expect("daemon 必须能加载 init 生成的配置");
    validate_config(&config).expect("init 生成的配置必须通过 Admin 校验");
    assert_eq!(config.schema_version, SCHEMA_VERSION);
    // bridge 部署必须在容器内监听非回环地址，否则端口映射无法访问代理。
    assert_eq!(config.listener.host, "0.0.0.0");
    assert_eq!(config.listener.port, 57878);
    assert!(!config.listener.has_inbound_auth());
    assert!(config.endpoints.is_empty());

    // daemon 启动时会把配置归一化并写回：缺失的 featureRules 必须恢复为内建规则集，
    // 否则 WebUI 的分流规则会整体消失。
    let normalized = config.clone().normalized();
    assert_ne!(normalized, config, "空 featureRules 必须被归一化");
    assert!(
        !normalized.feature_rules.is_empty(),
        "归一化后必须恢复内建分流规则"
    );
    validate_config(&normalized).expect("归一化后的配置必须仍然有效");

    let _ = std::fs::remove_dir_all(&dir);
}
