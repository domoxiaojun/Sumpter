//! Linux 配置目录与持久化。
//!
//! `config.json` 与 `stats.json` 都属于 daemon。配置严格要求当前 schema；
//! schema v3/v4/v5 会先保留原始备份再原子迁移到 v6。所有含密钥的写入均使用
//! 同目录临时文件、0600 权限和原子 rename。

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::config::{AppConfig, SCHEMA_VERSION};
use crate::events::{DiagnosticCaptureSnapshot, RuntimeSnapshot};

const MIGRATABLE_SCHEMA_V3: u32 = 3;
const MIGRATABLE_SCHEMA_V4: u32 = 4;
const MIGRATABLE_SCHEMA_V5: u32 = 5;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MigrationNotice {
    pub id: String,
    pub from_schema: u32,
    pub to_schema: u32,
    pub backup_file: String,
    pub endpoint_count: usize,
    pub expanded_legacy_passthrough_endpoints: usize,
    pub converted_to_auto_endpoint_ids: Vec<String>,
    pub removed_fields: Vec<String>,
}

#[derive(Debug)]
pub struct ConfigLoadResult {
    pub config: AppConfig,
    pub migration_notice: Option<MigrationNotice>,
}

#[derive(Debug)]
pub struct ConfigInitializeResult {
    pub config: AppConfig,
    pub created: bool,
    pub migration_notice: Option<MigrationNotice>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StickySessionAssignment {
    pub scheduling_group: String,
    pub updated_at: f64,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StickySessionFile {
    version: u32,
    sessions: HashMap<String, StickySessionAssignment>,
}

fn is_affinity_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Linux 默认配置目录：`$XDG_CONFIG_HOME/sumpter`，否则 `~/.config/sumpter`。
pub fn default_config_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(xdg).join("sumpter"));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/sumpter"))
}

#[derive(Debug, Clone)]
pub struct ConfigDir {
    pub root: PathBuf,
}

/// 原子写的提交结果。`Err(io::Error)` 只可能发生在 rename 提交点之前；
/// rename 成功后父目录 fsync 失败必须作为“已提交但耐久性不确定”显式返回。
#[derive(Debug)]
#[must_use]
pub enum PersistOutcome {
    Durable,
    CommittedUncertain { path: PathBuf, source: io::Error },
}

impl PersistOutcome {
    pub fn durability_warning(&self) -> Option<String> {
        match self {
            Self::Durable => None,
            Self::CommittedUncertain { path, source } => Some(format!(
                "{} 已原子提交，但父目录同步失败，断电耐久性不确定: {source}",
                path.display()
            )),
        }
    }
}

impl ConfigDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.json")
    }

    pub fn legacy_keys_path(&self) -> PathBuf {
        self.root.join("keys.json")
    }

    pub fn stats_path(&self) -> PathBuf {
        self.root.join("stats.json")
    }

    pub fn session_affinity_path(&self) -> PathBuf {
        self.root.join("session_affinity.json")
    }

    pub fn control_token_path(&self) -> PathBuf {
        self.root.join(".control_token")
    }

    pub fn log_path(&self) -> PathBuf {
        self.root.join("proxy.log")
    }

    pub fn pid_path(&self) -> PathBuf {
        self.root.join("sumpterd.pid")
    }

    pub fn ensure_exists(&self) -> io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    /// 加载当前 schema 配置；schema v3/v4/v5 会先备份原始文件并原子迁移到 v6。
    /// 不提前归一化，调用方必须先校验原始值，再执行 `normalized()`，避免负数
    /// 等非法输入在校验前被静默 clamp。
    pub fn load_config(&self) -> Result<AppConfig, ConfigLoadError> {
        self.load_config_with_notice().map(|result| result.config)
    }

    pub fn load_config_with_notice(&self) -> Result<ConfigLoadResult, ConfigLoadError> {
        let path = self.config_path();
        let data = std::fs::read(&path).map_err(|source| ConfigLoadError::Io {
            path: path.clone(),
            source,
        })?;
        let value: Value =
            serde_json::from_slice(&data).map_err(|source| ConfigLoadError::Parse {
                path: path.clone(),
                source,
            })?;
        let version = value.get("schemaVersion").and_then(Value::as_u64);
        if version == Some(u64::from(SCHEMA_VERSION)) {
            validate_current_wire(&value).map_err(|message| ConfigLoadError::Wire {
                path: path.clone(),
                message,
            })?;
            let config = serde_json::from_value::<AppConfig>(value)
                .map_err(|source| ConfigLoadError::Parse { path, source })?;
            return Ok(ConfigLoadResult {
                config,
                migration_notice: None,
            });
        }
        if !matches!(
            version,
            Some(v) if v == u64::from(MIGRATABLE_SCHEMA_V3)
                || v == u64::from(MIGRATABLE_SCHEMA_V4)
                || v == u64::from(MIGRATABLE_SCHEMA_V5)
        ) {
            return Err(ConfigLoadError::Schema {
                path,
                found: version,
                expected: SCHEMA_VERSION,
            });
        }
        self.migrate_legacy_config(value, &data, version.unwrap() as u32)
    }

    /// 首次启动时创建 bootstrap；如果只发现旧 `keys.json`，明确拒绝且不读、不迁移、
    /// 不改动旧文件。
    pub fn load_or_initialize_config(&self) -> Result<(AppConfig, bool), ConfigLoadError> {
        self.load_or_initialize_config_with_notice()
            .map(|result| (result.config, result.created))
    }

    pub fn load_or_initialize_config_with_notice(
        &self,
    ) -> Result<ConfigInitializeResult, ConfigLoadError> {
        match self.load_config_with_notice() {
            Ok(result) => Ok(ConfigInitializeResult {
                config: result.config,
                created: false,
                migration_notice: result.migration_notice,
            }),
            Err(ConfigLoadError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                let legacy_path = self.legacy_keys_path();
                match legacy_path.try_exists() {
                    Ok(true) => {
                        return Err(ConfigLoadError::LegacyOnly {
                            legacy_path,
                            config_path: self.config_path(),
                        });
                    }
                    Ok(false) => {}
                    Err(source) => {
                        return Err(ConfigLoadError::LegacyProbe {
                            path: legacy_path,
                            source,
                        });
                    }
                }
                let config = AppConfig::bootstrap().normalized();
                let outcome = self.save_config(&config).map_err(|source| {
                    ConfigLoadError::BootstrapWrite {
                        path: self.config_path(),
                        source,
                    }
                })?;
                if let Some(warning) = outcome.durability_warning() {
                    eprintln!("{warning}");
                }
                Ok(ConfigInitializeResult {
                    config,
                    created: true,
                    migration_notice: None,
                })
            }
            Err(error) => Err(error),
        }
    }

    fn migrate_legacy_config(
        &self,
        mut value: Value,
        original_data: &[u8],
        from_schema: u32,
    ) -> Result<ConfigLoadResult, ConfigLoadError> {
        let path = self.config_path();
        let stamp = migration_stamp();
        let backup_file = format!("config.before-schema-v6-{stamp}.json");
        let backup_path = self.root.join(&backup_file);
        let (endpoint_count, converted_to_auto_endpoint_ids) = match from_schema {
            MIGRATABLE_SCHEMA_V3 => migrate_v3_value(&mut value),
            MIGRATABLE_SCHEMA_V4 => migrate_v4_value(&mut value),
            MIGRATABLE_SCHEMA_V5 => Ok((0, Vec::new())),
            _ => unreachable!("unsupported migration schema {from_schema}"),
        }
        .map_err(|message| ConfigLoadError::MigrationInvalid {
            path: path.clone(),
            message,
        })?;
        let migrated_global_models =
            migrate_global_models_value(&mut value).map_err(|message| {
                ConfigLoadError::MigrationInvalid {
                    path: path.clone(),
                    message,
                }
            })?;
        let (flattened_endpoint_count, removed_pool_fields) =
            migrate_provider_pools_value(&mut value).map_err(|message| {
                ConfigLoadError::MigrationInvalid {
                    path: path.clone(),
                    message,
                }
            })?;
        let endpoint_count = if flattened_endpoint_count > 0 {
            flattened_endpoint_count
        } else {
            endpoint_count
        };
        validate_current_wire(&value).map_err(|message| ConfigLoadError::MigrationInvalid {
            path: path.clone(),
            message,
        })?;
        let migrated_config = serde_json::from_value::<AppConfig>(value).map_err(|source| {
            ConfigLoadError::Parse {
                path: path.clone(),
                source,
            }
        })?;

        write_new_owner_only(&backup_path, original_data).map_err(|source| {
            ConfigLoadError::MigrationBackup {
                path: backup_path.clone(),
                source,
            }
        })?;

        // 旧 schema 先解码为当前强类型配置，再重新序列化为 v6。这样只保留当前
        // AppConfig 明确定义的字段，所有不再属于现行 wire 的未知旧字段自然丢弃。
        let mut migrated_data = serde_json::to_vec_pretty(&migrated_config).map_err(|source| {
            ConfigLoadError::Parse {
                path: path.clone(),
                source,
            }
        })?;
        migrated_data.push(b'\n');
        let outcome = atomic_write(&path, &migrated_data).map_err(|source| {
            ConfigLoadError::MigrationWrite {
                path: path.clone(),
                source,
            }
        })?;
        if let Some(warning) = outcome.durability_warning() {
            eprintln!("{warning}");
        }

        let verified = self.load_current_config_without_migration();
        let config = match verified {
            Ok(config) => config,
            Err(verify_error) => {
                let rollback = atomic_write(&path, original_data);
                return Err(ConfigLoadError::MigrationVerify {
                    path,
                    verify_error: verify_error.to_string(),
                    rollback_error: rollback.err().map(|error| error.to_string()),
                });
            }
        };
        let mut removed_fields = match from_schema {
            MIGRATABLE_SCHEMA_V3 => vec![
                "listener.inboundDialectPassthrough".into(),
                "pools[].endpoints[].searchDialect".into(),
            ],
            MIGRATABLE_SCHEMA_V4 => Vec::new(),
            MIGRATABLE_SCHEMA_V5 => Vec::new(),
            _ => unreachable!("unsupported migration schema {from_schema}"),
        };
        removed_fields.extend(removed_pool_fields);
        if migrated_global_models {
            removed_fields.push("pools[].globalModels".into());
        }
        removed_fields.sort();
        removed_fields.dedup();
        let notice = MigrationNotice {
            id: format!("schema-v{from_schema}-to-v6-{stamp}"),
            from_schema,
            to_schema: SCHEMA_VERSION,
            backup_file,
            endpoint_count,
            expanded_legacy_passthrough_endpoints: converted_to_auto_endpoint_ids.len(),
            converted_to_auto_endpoint_ids,
            removed_fields,
        };
        Ok(ConfigLoadResult {
            config,
            migration_notice: Some(notice),
        })
    }

    fn load_current_config_without_migration(&self) -> Result<AppConfig, ConfigLoadError> {
        let path = self.config_path();
        let data = std::fs::read(&path).map_err(|source| ConfigLoadError::Io {
            path: path.clone(),
            source,
        })?;
        let value: Value =
            serde_json::from_slice(&data).map_err(|source| ConfigLoadError::Parse {
                path: path.clone(),
                source,
            })?;
        let version = value.get("schemaVersion").and_then(Value::as_u64);
        if version != Some(u64::from(SCHEMA_VERSION)) {
            return Err(ConfigLoadError::Schema {
                path,
                found: version,
                expected: SCHEMA_VERSION,
            });
        }
        validate_current_wire(&value).map_err(|message| ConfigLoadError::Wire {
            path: path.clone(),
            message,
        })?;
        serde_json::from_value(value).map_err(|source| ConfigLoadError::Parse { path, source })
    }

    pub fn save_config(&self, config: &AppConfig) -> io::Result<PersistOutcome> {
        let mut data = config
            .to_json_pretty()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        data.push('\n');
        self.ensure_exists()?;
        atomic_write(&self.config_path(), data.as_bytes())
    }

    /// 文件缺失视作空快照；损坏则返回错误，由 Engine 禁止后续覆盖原文件。
    pub fn load_stats(&self) -> Result<RuntimeSnapshot, StatsLoadError> {
        let path = self.stats_path();
        let data = match std::fs::read_to_string(&path) {
            Ok(data) => data,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Ok(RuntimeSnapshot::default());
            }
            Err(source) => return Err(StatsLoadError::Io { path, source }),
        };
        let mut snapshot = RuntimeSnapshot::from_json(&data)
            .map_err(|source| StatsLoadError::Parse { path, source })?;
        snapshot.normalize_loaded();
        Ok(snapshot)
    }

    pub fn save_stats(&self, snapshot: &RuntimeSnapshot) -> io::Result<PersistOutcome> {
        let mut data = snapshot
            .to_json_pretty()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        data.push('\n');
        self.ensure_exists()?;
        atomic_write(&self.stats_path(), data.as_bytes())
    }

    /// 只持久化稳定 Claude 会话 ID 的 SHA-256 摘要和调度组；不落原始 session ID。
    pub fn load_session_affinity(&self) -> io::Result<HashMap<String, StickySessionAssignment>> {
        let path = self.session_affinity_path();
        let data = match std::fs::read_to_string(&path) {
            Ok(data) => data,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(error) => return Err(error),
        };
        let envelope: serde_json::Value = serde_json::from_str(&data)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let version = envelope.get("version").and_then(serde_json::Value::as_u64);
        if version == Some(1) {
            // v1 使用 MD5/旧命名空间，不能安全迁移到复合键；冷启动重分配一次。
            return Ok(HashMap::new());
        }
        let file: StickySessionFile = serde_json::from_value(envelope)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if file.version != 2
            || file.sessions.iter().any(|(key, assignment)| {
                !is_affinity_id(key)
                    || assignment.scheduling_group.trim().is_empty()
                    || !assignment.updated_at.is_finite()
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "session_affinity.json v2 格式无效",
            ));
        }
        Ok(file.sessions)
    }

    pub fn save_session_affinity(
        &self,
        sessions: &HashMap<String, StickySessionAssignment>,
    ) -> io::Result<()> {
        let mut data = serde_json::to_vec_pretty(&StickySessionFile {
            version: 2,
            sessions: sessions.clone(),
        })
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        data.push(b'\n');
        self.ensure_exists()?;
        atomic_write(&self.session_affinity_path(), &data).map(|_| ())
    }

    /// Read or generate the local control token used by platform adapters.
    pub fn ensure_control_token(&self) -> io::Result<String> {
        let path = self.control_token_path();
        if let Ok(existing) = std::fs::read_to_string(&path) {
            let trimmed = existing.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
        let mut bytes = [0u8; 16];
        {
            use rand::RngCore;
            rand::rng().fill_bytes(&mut bytes);
        }
        let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        self.ensure_exists()?;
        atomic_write(&path, token.as_bytes()).map(|_| ())?;
        Ok(token)
    }

    /// 删除诊断捕获快照（只处理固定文件名，不扫描其他路径）。
    pub fn remove_diagnostic_capture(&self) -> io::Result<bool> {
        let path = self.root.join("diagnostic_capture.json");
        match std::fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn load_diagnostic_capture(&self) -> io::Result<DiagnosticCaptureSnapshot> {
        let path = self.root.join("diagnostic_capture.json");
        match std::fs::read_to_string(&path) {
            Ok(data) => serde_json::from_str(&data).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("diagnostic_capture.json 格式无效: {e}"),
                )
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Ok(DiagnosticCaptureSnapshot::default())
            }
            Err(e) => Err(e),
        }
    }

    pub fn save_diagnostic_capture(
        &self,
        snapshot: &DiagnosticCaptureSnapshot,
    ) -> io::Result<PersistOutcome> {
        let mut data = serde_json::to_vec_pretty(snapshot)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        data.push(b'\n');
        self.ensure_exists()?;
        atomic_write(&self.root.join("diagnostic_capture.json"), &data)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigLoadError {
    #[error("读取配置失败 {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("解析配置失败 {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("配置 {path} 的 schema v6 wire 无效: {message}")]
    Wire { path: PathBuf, message: String },
    #[error("配置 {path} 的 schemaVersion 必须显式为 {expected}，当前为 {found:?}")]
    Schema {
        path: PathBuf,
        found: Option<u64>,
        expected: u32,
    },
    #[error(
        "发现旧配置 {legacy_path}，但缺少 {config_path}；Linux Rust 版不读取或迁移 keys.json，请先提供 schema v3/v4 config.json"
    )]
    LegacyOnly {
        legacy_path: PathBuf,
        config_path: PathBuf,
    },
    #[error("检查旧配置失败 {path}: {source}；为避免误判为空目录，不创建 bootstrap")]
    LegacyProbe { path: PathBuf, source: io::Error },
    #[error("创建 bootstrap 配置失败 {path}: {source}")]
    BootstrapWrite { path: PathBuf, source: io::Error },
    #[error("旧 schema 配置 {path} 无法迁移: {message}")]
    MigrationInvalid { path: PathBuf, message: String },
    #[error("创建 schema 迁移备份失败 {path}: {source}")]
    MigrationBackup { path: PathBuf, source: io::Error },
    #[error("写入 schema v6 配置失败 {path}: {source}")]
    MigrationWrite { path: PathBuf, source: io::Error },
    #[error("schema v6 配置 {path} 写入后验证失败: {verify_error}; 回滚错误: {rollback_error:?}")]
    MigrationVerify {
        path: PathBuf,
        verify_error: String,
        rollback_error: Option<String>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum StatsLoadError {
    #[error("读取统计失败 {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("解析统计失败 {path}: {source}；为保护原文件，本次运行禁止覆盖 stats.json")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
}

pub fn validate_config_wire(value: &Value) -> Result<(), String> {
    validate_current_wire(value)
}

fn validate_current_wire(value: &Value) -> Result<(), String> {
    let root = value
        .as_object()
        .ok_or_else(|| "配置根节点必须是对象".to_string())?;
    if root.get("schemaVersion").and_then(Value::as_u64) != Some(u64::from(SCHEMA_VERSION)) {
        return Err(format!("schemaVersion 必须显式为 {SCHEMA_VERSION}"));
    }
    if root
        .get("listener")
        .and_then(Value::as_object)
        .is_some_and(|listener| listener.contains_key("inboundDialectPassthrough"))
    {
        return Err("schema v6 不允许 listener.inboundDialectPassthrough".into());
    }
    if root.contains_key("pools") {
        return Err("schema v6 不允许 pools；请先完成 Provider 候选迁移".into());
    }
    let endpoints = match root.get("endpoints") {
        None => &[][..],
        Some(value) => value
            .as_array()
            .ok_or_else(|| "endpoints 必须是数组".to_string())?,
    };
    for (endpoint_index, endpoint) in endpoints.iter().enumerate() {
        if endpoint.get("searchDialect").is_some() {
            return Err(format!(
                "endpoints[{endpoint_index}] 不允许遗留 searchDialect；WebSearch 能力由目标协议决定"
            ));
        }
        if endpoint.get("protocols").is_some() {
            return Err(format!(
                "endpoints[{endpoint_index}] 不允许 protocols 数组，请使用单值 protocol"
            ));
        }
        let protocol = endpoint
            .get("protocol")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("endpoints[{endpoint_index}].protocol 必须显式存在"))?;
        if !matches!(
            protocol,
            "auto" | "anthropic" | "openai" | "openai-responses"
        ) {
            return Err(format!(
                "endpoints[{endpoint_index}].protocol 无效: {protocol}"
            ));
        }
    }
    if let Some(rules) = root.get("featureRules").and_then(Value::as_array) {
        for (index, rule) in rules.iter().enumerate() {
            if rule
                .get("target")
                .and_then(Value::as_object)
                .is_some_and(|target| {
                    target.contains_key("poolID") || target.contains_key("poolId")
                })
            {
                return Err(format!(
                    "featureRules[{index}].target 不允许 poolID；请使用 endpointID"
                ));
            }
        }
    }
    Ok(())
}

/// 将 schema v5 及更早版本的池壳一次性展平为 Provider 候选序列。
/// 迁移只改变结构和旧池优先级，不按名称、地址或 ID 重新排序入口。
fn migrate_provider_pools_value(value: &mut Value) -> Result<(usize, Vec<String>), String> {
    let root = value
        .as_object_mut()
        .ok_or_else(|| "配置根节点必须是对象".to_string())?;
    let Some(pools_value) = root.remove("pools") else {
        if !root.contains_key("endpoints") {
            root.insert("endpoints".into(), Value::Array(Vec::new()));
        }
        root.insert(
            "schemaVersion".into(),
            Value::Number(serde_json::Number::from(SCHEMA_VERSION)),
        );
        return Ok((0, Vec::new()));
    };
    let pools = pools_value
        .as_array()
        .ok_or_else(|| "pools 必须是数组".to_string())?;
    let mut order: Vec<usize> = (0..pools.len()).collect();
    if let Some(primary) = pools.iter().position(|pool| {
        pool.get("role").and_then(Value::as_str) == Some("primary")
            || pool.get("id").and_then(Value::as_str) == Some("primary")
    }) {
        order.swap(0, primary);
    }
    let mut flattened = Vec::new();
    let mut base_max_priority: i64 = 0;
    let mut first_pool = true;
    let mut removed = vec![
        "pools".to_string(),
        "featureRules[].target.poolID".to_string(),
    ];
    for pool_index in order {
        let pool = pools
            .get(pool_index)
            .and_then(Value::as_object)
            .ok_or_else(|| format!("pools[{pool_index}] 必须是对象"))?;
        let endpoints = pool
            .get("endpoints")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("pools[{pool_index}].endpoints 必须是数组"))?;
        let mut next_priority = base_max_priority.saturating_add(10);
        for endpoint in endpoints {
            let mut endpoint = endpoint.clone();
            let object = endpoint
                .as_object_mut()
                .ok_or_else(|| format!("pools[{pool_index}].endpoints 必须是对象数组"))?;
            if !first_pool {
                object.insert(
                    "priority".into(),
                    Value::Number(serde_json::Number::from(next_priority)),
                );
                next_priority = next_priority.saturating_add(1);
            }
            flattened.push(endpoint);
        }
        if first_pool {
            base_max_priority = flattened
                .iter()
                .filter_map(|endpoint| endpoint.get("priority").and_then(Value::as_i64))
                .max()
                .unwrap_or(0);
            first_pool = false;
        } else if next_priority > base_max_priority {
            base_max_priority = next_priority;
        }
    }
    if let Some(rules) = root.get_mut("featureRules").and_then(Value::as_array_mut) {
        for rule in rules {
            if let Some(target) = rule.get_mut("target").and_then(Value::as_object_mut)
                && (target.remove("poolID").is_some() || target.remove("poolId").is_some())
            {
                removed.push("featureRules[].target.poolID".to_string());
            }
        }
    }
    root.insert("endpoints".into(), Value::Array(flattened.clone()));
    root.insert(
        "schemaVersion".into(),
        Value::Number(serde_json::Number::from(SCHEMA_VERSION)),
    );
    Ok((flattened.len(), removed))
}

/// schema v5 仍可能被旧版本写入池级 `globalModels`。只在配置文件读取时执行一次
/// 迁移；转换完成后立即删掉旧字段，运行时模型和路由不再认识它。
fn migrate_global_models_value(value: &mut Value) -> Result<bool, String> {
    let root = value
        .as_object_mut()
        .ok_or_else(|| "配置根节点必须是对象".to_string())?;
    let Some(pools_value) = root.get_mut("pools") else {
        return Ok(false);
    };
    let pools = pools_value
        .as_array_mut()
        .ok_or_else(|| "pools 必须是数组".to_string())?;
    let mut changed = false;
    for (pool_index, pool) in pools.iter_mut().enumerate() {
        let pool = pool
            .as_object_mut()
            .ok_or_else(|| format!("pools[{pool_index}] 必须是对象"))?;
        let Some(global_value) = pool.remove("globalModels") else {
            continue;
        };
        changed = true;
        let rules = global_value
            .as_array()
            .ok_or_else(|| format!("pools[{pool_index}].globalModels 必须是数组"))?;
        let mappings: Vec<Value> = rules
            .iter()
            .enumerate()
            .map(|(rule_index, rule)| {
                let rule = rule.as_object().ok_or_else(|| {
                    format!("pools[{pool_index}].globalModels[{rule_index}] 必须是对象")
                })?;
                let pattern = rule
                    .get("pattern")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        format!(
                            "pools[{pool_index}].globalModels[{rule_index}].pattern 必须是非空字符串"
                        )
                    })?;
                Ok(serde_json::json!({
                    "clientPattern": pattern,
                    "context": rule
                        .get("context")
                        .cloned()
                        .unwrap_or_else(|| Value::String("standard".into())),
                    "thinking": rule
                        .get("thinking")
                        .cloned()
                        .unwrap_or_else(|| Value::String("adaptive".into())),
                    "upstreamModel": ""
                }))
            })
            .collect::<Result<_, String>>()?;
        let Some(endpoints_value) = pool.get_mut("endpoints") else {
            continue;
        };
        let endpoints = endpoints_value
            .as_array_mut()
            .ok_or_else(|| format!("pools[{pool_index}].endpoints 必须是数组"))?;
        for (endpoint_index, endpoint) in endpoints.iter_mut().enumerate() {
            let endpoint = endpoint.as_object_mut().ok_or_else(|| {
                format!("pools[{pool_index}].endpoints[{endpoint_index}] 必须是对象")
            })?;
            let has_explicit_mappings = match endpoint.get("mappings") {
                None | Some(Value::Null) => false,
                Some(Value::Array(items)) => !items.is_empty(),
                Some(_) => {
                    return Err(format!(
                        "pools[{pool_index}].endpoints[{endpoint_index}].mappings 必须是数组"
                    ));
                }
            };
            if !has_explicit_mappings {
                endpoint.insert("mappings".into(), Value::Array(mappings.clone()));
            }
        }
    }
    Ok(changed)
}

fn migrate_v3_value(value: &mut Value) -> Result<(usize, Vec<String>), String> {
    let root = value
        .as_object_mut()
        .ok_or_else(|| "配置根节点必须是对象".to_string())?;
    let legacy_passthrough = match root.get_mut("listener") {
        Some(listener) => {
            let listener = listener
                .as_object_mut()
                .ok_or_else(|| "listener 必须是对象".to_string())?;
            match listener.remove("inboundDialectPassthrough") {
                None => false,
                Some(raw) => raw
                    .as_bool()
                    .ok_or_else(|| "listener.inboundDialectPassthrough 必须是布尔值".to_string())?,
            }
        }
        None => false,
    };
    let mut endpoint_count = 0usize;
    let mut converted_to_auto_endpoint_ids = Vec::new();
    if let Some(pools_value) = root.get_mut("pools") {
        let pools = pools_value
            .as_array_mut()
            .ok_or_else(|| "schema v3 pools 必须是数组".to_string())?;
        for (pool_index, pool) in pools.iter_mut().enumerate() {
            let Some(endpoints_value) = pool.get_mut("endpoints") else {
                continue;
            };
            let endpoints = endpoints_value
                .as_array_mut()
                .ok_or_else(|| format!("pools[{pool_index}].endpoints 必须是数组"))?;
            for (endpoint_index, endpoint) in endpoints.iter_mut().enumerate() {
                let endpoint = endpoint.as_object_mut().ok_or_else(|| {
                    format!("pools[{pool_index}].endpoints[{endpoint_index}] 必须是对象")
                })?;
                endpoint.remove("searchDialect");
                if let Some(protocol) = endpoint.get("protocol") {
                    let protocol = protocol.as_str().ok_or_else(|| {
                        format!(
                            "pools[{pool_index}].endpoints[{endpoint_index}].protocol 必须是字符串"
                        )
                    })?;
                    if !matches!(protocol, "anthropic" | "openai" | "openai-responses") {
                        return Err(format!(
                            "pools[{pool_index}].endpoints[{endpoint_index}].protocol 不是有效的 schema v3 协议: {protocol}"
                        ));
                    }
                }
                endpoint_count += 1;
                if legacy_passthrough {
                    let endpoint_id = endpoint
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("pool-{pool_index}-endpoint-{endpoint_index}"));
                    endpoint.insert("protocol".into(), Value::String("auto".into()));
                    converted_to_auto_endpoint_ids.push(endpoint_id);
                } else if !endpoint.contains_key("protocol") {
                    endpoint.insert("protocol".into(), Value::String("anthropic".into()));
                }
            }
        }
    }
    root.insert(
        "schemaVersion".into(),
        Value::Number(serde_json::Number::from(SCHEMA_VERSION)),
    );
    Ok((endpoint_count, converted_to_auto_endpoint_ids))
}

fn migrate_v4_value(value: &mut Value) -> Result<(usize, Vec<String>), String> {
    let root = value
        .as_object_mut()
        .ok_or_else(|| "配置根节点必须是对象".to_string())?;
    let mut endpoint_count = 0usize;
    if let Some(pools_value) = root.get_mut("pools") {
        let pools = pools_value
            .as_array_mut()
            .ok_or_else(|| "schema v4 pools 必须是数组".to_string())?;
        for (pool_index, pool) in pools.iter_mut().enumerate() {
            let Some(endpoints_value) = pool.get_mut("endpoints") else {
                continue;
            };
            let endpoints = endpoints_value
                .as_array_mut()
                .ok_or_else(|| format!("pools[{pool_index}].endpoints 必须是数组"))?;
            for (endpoint_index, endpoint) in endpoints.iter().enumerate() {
                endpoint.as_object().ok_or_else(|| {
                    format!("pools[{pool_index}].endpoints[{endpoint_index}] 必须是对象")
                })?;
                endpoint_count += 1;
            }
        }
    }
    root.insert(
        "schemaVersion".into(),
        Value::Number(serde_json::Number::from(SCHEMA_VERSION)),
    );
    Ok((endpoint_count, Vec::new()))
}

fn migration_stamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().to_string())
        .unwrap_or_else(|_| "0".into())
}

fn write_new_owner_only(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    set_owner_only(path)?;
    file.write_all(data)?;
    file.sync_all()?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "备份文件缺少父目录"))?;
    std::fs::File::open(parent)?.sync_all()
}

fn atomic_write(path: &Path, data: &[u8]) -> io::Result<PersistOutcome> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("sumpter-data");
    let tmp = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    let prepare_result = (|| {
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        set_owner_only(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "目标文件缺少父目录"))?;
        let directory = std::fs::File::open(parent)?;
        Ok(directory)
    })();
    let directory = match prepare_result {
        Ok(directory) => directory,
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(error);
        }
    };
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    match directory.sync_all() {
        Ok(()) => Ok(PersistOutcome::Durable),
        Err(source) => Ok(PersistOutcome::CommittedUncertain {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn set_owner_only(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ContextMode;
    use serde_json::json;

    fn temp_dir(tag: &str) -> ConfigDir {
        let root =
            std::env::temp_dir().join(format!("sumpter-linux-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        ConfigDir::new(root)
    }

    #[test]
    fn bootstrap_uses_current_schema_and_owner_only() {
        let dir = temp_dir("bootstrap");
        let (config, created) = dir.load_or_initialize_config().unwrap();
        assert!(created);
        assert_eq!(config.schema_version, SCHEMA_VERSION);
        assert_eq!(dir.load_config().unwrap(), config);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.config_path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn legacy_only_is_rejected_without_touching_it() {
        let dir = temp_dir("legacy");
        dir.ensure_exists().unwrap();
        std::fs::write(dir.legacy_keys_path(), "legacy-content").unwrap();
        assert!(matches!(
            dir.load_or_initialize_config(),
            Err(ConfigLoadError::LegacyOnly { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(dir.legacy_keys_path()).unwrap(),
            "legacy-content"
        );
        assert!(!dir.config_path().exists());
    }

    #[test]
    fn missing_or_unsupported_schema_is_rejected() {
        let dir = temp_dir("schema");
        dir.ensure_exists().unwrap();
        std::fs::write(dir.config_path(), "{}").unwrap();
        assert!(matches!(
            dir.load_config(),
            Err(ConfigLoadError::Schema { found: None, .. })
        ));
        std::fs::write(dir.config_path(), r#"{"schemaVersion":2}"#).unwrap();
        assert!(matches!(
            dir.load_config(),
            Err(ConfigLoadError::Schema { found: Some(2), .. })
        ));
    }

    #[test]
    fn v3_passthrough_true_converts_every_endpoint_to_auto() {
        let mut value = json!({
            "schemaVersion": 3,
            "listener": {"inboundDialectPassthrough": true},
            "pools": [{
                "id": "primary",
                "endpoints": [
                    {"id": "a", "protocol": "anthropic"},
                    {"id": "b", "protocol": "openai", "searchDialect": "openai"},
                    {"id": "c", "protocol": "openai-responses"}
                ]
            }]
        });

        let (endpoint_count, converted) = migrate_v3_value(&mut value).unwrap();

        assert_eq!(endpoint_count, 3);
        assert_eq!(converted, vec!["a", "b", "c"]);
        assert_eq!(value["schemaVersion"], json!(SCHEMA_VERSION));
        assert!(value["listener"].get("inboundDialectPassthrough").is_none());
        assert_eq!(value["pools"][0]["endpoints"][0]["protocol"], "auto");
        assert_eq!(value["pools"][0]["endpoints"][1]["protocol"], "auto");
        assert_eq!(value["pools"][0]["endpoints"][2]["protocol"], "auto");
        assert!(
            value["pools"][0]["endpoints"][1]
                .get("searchDialect")
                .is_none()
        );
    }

    #[test]
    fn v3_passthrough_false_or_missing_preserves_protocol_and_defaults_missing_to_anthropic() {
        for listener in [json!({"inboundDialectPassthrough": false}), json!({})] {
            let mut value = json!({
                "schemaVersion": 3,
                "listener": listener,
                "pools": [{
                    "id": "primary",
                    "endpoints": [
                        {"id": "a", "protocol": "openai"},
                        {"id": "b"}
                    ]
                }]
            });

            let (_, converted) = migrate_v3_value(&mut value).unwrap();

            assert!(converted.is_empty());
            assert_eq!(value["pools"][0]["endpoints"][0]["protocol"], "openai");
            assert_eq!(value["pools"][0]["endpoints"][1]["protocol"], "anthropic");
        }
    }

    #[test]
    fn v4_migration_writes_v5() {
        let mut value = json!({
            "schemaVersion": 4,
            "listener": {},
            "pools": [{
                "id": "primary",
                "endpoints": [{
                    "id": "a",
                    "protocol": "auto"
                }]
            }]
        });

        let (endpoint_count, converted) = migrate_v4_value(&mut value).unwrap();

        assert_eq!(endpoint_count, 1);
        assert!(converted.is_empty());
        assert_eq!(value["schemaVersion"], json!(SCHEMA_VERSION));
        migrate_provider_pools_value(&mut value).unwrap();
        assert!(validate_config_wire(&value).is_ok());
    }

    #[test]
    fn current_wire_requires_explicit_four_state_protocol_and_rejects_legacy_switch() {
        let mut value = json!({
            "schemaVersion": SCHEMA_VERSION,
            "listener": {},
            "endpoints": [{"id": "a", "protocol": "auto"}]
        });
        assert!(validate_config_wire(&value).is_ok());

        value["endpoints"][0]
            .as_object_mut()
            .unwrap()
            .remove("protocol");
        assert!(
            validate_config_wire(&value)
                .unwrap_err()
                .contains("必须显式存在")
        );

        value["endpoints"][0]["protocol"] = json!("invalid");
        assert!(
            validate_config_wire(&value)
                .unwrap_err()
                .contains("protocol 无效")
        );

        value["endpoints"][0]["protocol"] = json!("auto");
        value["endpoints"][0]["protocols"] = json!(["anthropic"]);
        assert!(
            validate_config_wire(&value)
                .unwrap_err()
                .contains("不允许 protocols 数组")
        );
        value["endpoints"][0]
            .as_object_mut()
            .unwrap()
            .remove("protocols");
        value["listener"]["inboundDialectPassthrough"] = json!(true);
        assert!(
            validate_config_wire(&value)
                .unwrap_err()
                .contains("不允许 listener.inboundDialectPassthrough")
        );
        value["listener"]
            .as_object_mut()
            .unwrap()
            .remove("inboundDialectPassthrough");
        value["endpoints"][0]["searchDialect"] = json!("openai");
        assert!(
            validate_config_wire(&value)
                .unwrap_err()
                .contains("不允许遗留 searchDialect")
        );
        value["endpoints"][0]
            .as_object_mut()
            .unwrap()
            .remove("searchDialect");
    }

    #[test]
    fn v3_file_migration_keeps_owner_only_backup_and_is_idempotent() {
        if SCHEMA_VERSION != 6 {
            return;
        }
        let dir = temp_dir("schema-v5-migration");
        dir.ensure_exists().unwrap();
        let original = json!({
            "schemaVersion": 3,
            "featureRules": [],
            "listener": {
                "allowedCIDRs": [],
                "authToken": "test-secret",
                "host": "127.0.0.1",
                "inboundDialectPassthrough": true,
                "port": 57878
            },
            "pools": [{
                "endpoints": [{
                    "apiKey": "endpoint-secret",
                    "baseURL": "https://example.invalid",
                    "enabled": true,
                    "id": "provider-a",
                    "mappings": [],
                    "name": "Provider A",
                    "obsoleteEndpointOption": true,
                    "pinnedIPExclusive": false,
                    "pinnedIPs": [],
                    "protocol": "anthropic"
                }],
                "globalModels": [],
                "id": "primary",
                "name": "Provider",
                "role": "primary"
            }],
            "retry": {}
        });
        let original_data = serde_json::to_vec_pretty(&original).unwrap();
        std::fs::write(dir.config_path(), &original_data).unwrap();

        let first = dir.load_config_with_notice().unwrap();
        let notice = first.migration_notice.unwrap();
        assert_eq!(notice.converted_to_auto_endpoint_ids, vec!["provider-a"]);
        let backup_path = dir.root.join(&notice.backup_file);
        assert_eq!(std::fs::read(&backup_path).unwrap(), original_data);
        let migrated: Value =
            serde_json::from_slice(&std::fs::read(dir.config_path()).unwrap()).unwrap();
        assert_eq!(migrated["endpoints"][0]["protocol"], "auto");
        assert!(
            migrated["listener"]
                .get("inboundDialectPassthrough")
                .is_none()
        );
        assert!(
            migrated["endpoints"][0]
                .get("obsoleteEndpointOption")
                .is_none()
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&backup_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(dir.config_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        let second = dir.load_config_with_notice().unwrap();
        assert!(second.migration_notice.is_none());
        let backups = std::fs::read_dir(&dir.root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("config.before-schema-v6-")
            })
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn v4_file_migration_keeps_owner_only_backup_and_is_idempotent() {
        if SCHEMA_VERSION != 6 {
            return;
        }
        let dir = temp_dir("schema-v4-migration");
        dir.ensure_exists().unwrap();
        let original = json!({
            "schemaVersion": 4,
            "featureRules": [],
            "listener": {
                "allowedCIDRs": [],
                "authToken": "test-secret",
                "host": "127.0.0.1",
                "port": 57878
            },
            "pools": [{
                "endpoints": [{
                    "apiKey": "endpoint-secret",
                    "baseURL": "https://example.invalid",
                    "enabled": true,
                    "id": "provider-a",
                    "mappings": [],
                    "name": "Provider A",
                    "obsoleteEndpointOption": true,
                    "pinnedIPExclusive": false,
                    "pinnedIPs": [],
                    "protocol": "auto"
                }],
                "globalModels": [],
                "id": "primary",
                "name": "Provider",
                "role": "primary"
            }],
            "retry": {}
        });
        let original_data = serde_json::to_vec_pretty(&original).unwrap();
        std::fs::write(dir.config_path(), &original_data).unwrap();

        let first = dir.load_config_with_notice().unwrap();
        let notice = first.migration_notice.unwrap();
        assert_eq!(notice.from_schema, 4);
        assert_eq!(notice.to_schema, SCHEMA_VERSION);
        assert_eq!(notice.endpoint_count, 1);
        assert_eq!(notice.expanded_legacy_passthrough_endpoints, 0);
        assert!(notice.converted_to_auto_endpoint_ids.is_empty());
        assert!(notice.removed_fields.iter().any(|field| field == "pools"));
        let backup_path = dir.root.join(&notice.backup_file);
        assert_eq!(std::fs::read(&backup_path).unwrap(), original_data);

        let migrated: Value =
            serde_json::from_slice(&std::fs::read(dir.config_path()).unwrap()).unwrap();
        assert_eq!(migrated["schemaVersion"], json!(SCHEMA_VERSION));
        assert_eq!(migrated["endpoints"][0]["protocol"], "auto");
        assert!(
            migrated["endpoints"][0]
                .get("obsoleteEndpointOption")
                .is_none()
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&backup_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(dir.config_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        let second = dir.load_config_with_notice().unwrap();
        assert!(second.migration_notice.is_none());
        let backups = std::fs::read_dir(&dir.root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("config.before-schema-v6-")
            })
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn v5_file_migrates_global_models_to_explicit_endpoint_mappings_once() {
        let dir = temp_dir("global-models-v5-migration");
        dir.ensure_exists().unwrap();
        let original = json!({
            "schemaVersion": 5,
            "listener": {},
            "pools": [{
                "id": "primary",
                "role": "primary",
                "globalModels": [{"pattern": " claude-opus-* ", "thinking": "adaptive"}],
                "endpoints": [
                    {"id": "inherit", "baseURL": "https://inherit.invalid", "protocol": "anthropic", "mappings": []},
                    {"id": "explicit", "baseURL": "https://explicit.invalid", "protocol": "anthropic", "mappings": [{
                        "clientPattern": "gpt-5.4", "upstreamModel": "gpt-5.4", "thinking": "disabled", "context": "standard"
                    }]}
                ]
            }]
        });
        std::fs::write(
            dir.config_path(),
            serde_json::to_vec_pretty(&original).unwrap(),
        )
        .unwrap();

        let first = dir.load_config_with_notice().unwrap();
        let notice = first.migration_notice.unwrap();
        assert_eq!(notice.from_schema, 5);
        assert!(
            notice
                .removed_fields
                .iter()
                .any(|field| field == "pools[].globalModels")
        );
        assert_eq!(
            first.config.endpoints[0].mappings[0].client_pattern,
            "claude-opus-*"
        );
        assert_eq!(
            first.config.endpoints[0].mappings[0].context,
            ContextMode::Standard
        );
        assert_eq!(first.config.endpoints[1].mappings.len(), 1);

        let migrated: Value =
            serde_json::from_slice(&std::fs::read(dir.config_path()).unwrap()).unwrap();
        assert!(migrated.get("pools").is_none());
        assert_eq!(
            migrated["endpoints"][0]["mappings"][0]["clientPattern"],
            "claude-opus-*"
        );
        assert!(
            dir.load_config_with_notice()
                .unwrap()
                .migration_notice
                .is_none()
        );
    }

    #[test]
    fn invalid_v3_migration_does_not_touch_original_or_create_backup() {
        let dir = temp_dir("schema-v5-invalid-migration");
        dir.ensure_exists().unwrap();
        let original = br#"{
  "schemaVersion": 3,
  "listener": {"inboundDialectPassthrough": false},
  "pools": [{"id": "primary", "endpoints": [{"id": "bad", "protocol": "unknown"}]}]
}"#;
        std::fs::write(dir.config_path(), original).unwrap();

        assert!(matches!(
            dir.load_config_with_notice(),
            Err(ConfigLoadError::MigrationInvalid { .. })
        ));
        assert_eq!(std::fs::read(dir.config_path()).unwrap(), original);
        assert!(
            std::fs::read_dir(&dir.root)
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("config.before-schema-v6-"))
        );
    }

    #[test]
    fn corrupt_stats_returns_error_and_is_not_rewritten_by_loader() {
        let dir = temp_dir("stats-corrupt");
        dir.ensure_exists().unwrap();
        std::fs::write(dir.stats_path(), "{broken").unwrap();
        assert!(matches!(
            dir.load_stats(),
            Err(StatsLoadError::Parse { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(dir.stats_path()).unwrap(),
            "{broken"
        );
    }

    #[test]
    fn atomic_replace_commits_owner_only_inode_without_leftover_temp() {
        let dir = temp_dir("atomic-permissions");
        dir.ensure_exists().unwrap();
        std::fs::write(dir.config_path(), "old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.config_path(), std::fs::Permissions::from_mode(0o666))
                .unwrap();
        }

        let config = AppConfig::bootstrap().normalized();
        let outcome = dir.save_config(&config).unwrap();
        assert!(outcome.durability_warning().is_none());
        assert_eq!(dir.load_config().unwrap(), config);
        assert!(
            !dir.root
                .join(format!(".config.json.{}.tmp", std::process::id()))
                .exists()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.config_path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn session_affinity_roundtrip_contains_only_hashed_keys() {
        let dir = temp_dir("session-affinity");
        let assignments = HashMap::from([(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            StickySessionAssignment {
                scheduling_group: "account-b".into(),
                updated_at: 123.0,
            },
        )]);
        dir.save_session_affinity(&assignments).unwrap();
        assert_eq!(dir.load_session_affinity().unwrap(), assignments);
        let raw = std::fs::read_to_string(dir.session_affinity_path()).unwrap();
        assert!(raw.contains("\"version\": 2"));
        assert!(!raw.contains("claude-session-secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.session_affinity_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn session_affinity_v1_is_discarded_and_can_be_replaced_by_v2() {
        let dir = temp_dir("session-affinity-v1");
        dir.ensure_exists().unwrap();
        std::fs::write(
            dir.session_affinity_path(),
            r#"{"version":1,"sessions":{"0123456789abcdef0123456789abcdef":{"schedulingGroup":"legacy","updatedAt":1}}}"#,
        )
        .unwrap();

        assert!(dir.load_session_affinity().unwrap().is_empty());
        let assignments = HashMap::from([(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            StickySessionAssignment {
                scheduling_group: "account-a".into(),
                updated_at: 2.0,
            },
        )]);
        dir.save_session_affinity(&assignments).unwrap();
        assert_eq!(dir.load_session_affinity().unwrap(), assignments);
        assert!(
            std::fs::read_to_string(dir.session_affinity_path())
                .unwrap()
                .contains("\"version\": 2")
        );
    }

    #[test]
    fn invalid_or_unknown_session_affinity_is_rejected_without_overwrite() {
        let dir = temp_dir("session-affinity-invalid");
        dir.ensure_exists().unwrap();
        for raw in [
            "not-json",
            r#"{"version":9,"sessions":{}}"#,
            r#"{"version":2,"sessions":{"ABC":{"schedulingGroup":"","updatedAt":1}}}"#,
        ] {
            std::fs::write(dir.session_affinity_path(), raw).unwrap();
            assert!(dir.load_session_affinity().is_err());
            assert_eq!(
                std::fs::read_to_string(dir.session_affinity_path()).unwrap(),
                raw
            );
        }
    }
}
