//! sumpterd Linux daemon。
//!
//! Admin 默认监听 loopback `127.0.0.1:57879`，可用 `--admin-host` / `--admin-port`
//! 或环境变量 `KEKULV_ADMIN_HOST` / `KEKULV_ADMIN_PORT` 覆盖（适合写进 systemd unit）。
//! Admin 默认读取 `<config-dir>/admin-password`，由 WebUI 内置登录页保护；
//! `--admin-password-file` / `KEKULV_ADMIN_PASSWORD_FILE` 可覆盖凭据文件路径。
//! 现有单行密码文件会在首次修改凭据时迁移为 Argon2 哈希 JSON；公网访问仍建议 HTTPS。
//! Proxy 数据面按 schema v5 `config.json.listener` 独立运行。
//! daemon 默认前台运行，适配 systemd user/system service，不监视 stdin EOF。

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use sumpter_core::config_store::{ConfigDir, default_config_dir};
use sumpter_linux_adapter::admin::{
    AdminAuth, AdminListen, AdminState, DEFAULT_ADMIN_HOST, DEFAULT_ADMIN_PORT, ProxySupervisor,
    SystemdScope, admin_router, validate_config,
};
use sumpter_linux_adapter::engine::Engine;
use sumpter_linux_adapter::outbound::ReqwestTransport;
use sumpter_linux_adapter::server;

const DEFAULT_ADMIN_PASSWORD_FILENAME: &str = "admin-password";

#[derive(Debug)]
struct Options {
    config_dir: PathBuf,
    web_root: Option<PathBuf>,
    systemd_scope: SystemdScope,
    admin_listen: AdminListen,
    admin_auth: AdminAuth,
}

fn main() -> ExitCode {
    let options = match parse_options() {
        Ok(Some(options)) => options,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            print_usage();
            return ExitCode::from(2);
        }
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("tokio runtime 启动失败: {error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run(options))
}

fn parse_options() -> Result<Option<Options>, String> {
    let mut config_dir = None;
    let mut web_root = None;
    let mut no_web = false;
    let mut systemd_scope = SystemdScope::User;
    let mut admin_host: Option<String> = None;
    let mut admin_port: Option<u16> = None;
    let mut admin_password_file: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config-dir" => {
                config_dir = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--config-dir 缺少路径".to_string())?,
                ));
            }
            "--web-root" => {
                web_root = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--web-root 缺少路径".to_string())?,
                ));
            }
            "--no-web" => no_web = true,
            "--systemd-scope" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--systemd-scope 缺少 user 或 system".to_string())?;
                systemd_scope = SystemdScope::parse(&value)?;
            }
            "--admin-host" => {
                admin_host = Some(
                    args.next()
                        .ok_or_else(|| "--admin-host 缺少地址".to_string())?,
                );
            }
            "--admin-port" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--admin-port 缺少端口".to_string())?;
                admin_port = Some(parse_admin_port_arg(&value)?);
            }
            "--admin-password-file" => {
                admin_password_file =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        "--admin-password-file 缺少路径".to_string()
                    })?));
            }
            // 兼容早期脚本；Linux daemon 本来就始终前台，不再有 stdin EOF sidecar 模式。
            "--foreground" => {}
            "--version" => {
                println!("sumpterd {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--help" | "-h" => {
                print_usage();
                return Ok(None);
            }
            other => return Err(format!("未知参数: {other}")),
        }
    }

    let config_dir = config_dir
        .or_else(default_config_dir)
        .ok_or_else(|| "无法确定配置目录（HOME/XDG_CONFIG_HOME 未设置）".to_string())?;
    let web_root = if no_web {
        None
    } else {
        web_root.or_else(default_web_root)
    };
    let admin_listen = resolve_admin_listen(admin_host, admin_port)?;
    let admin_auth = resolve_admin_auth(admin_password_file, &config_dir)?;
    Ok(Some(Options {
        config_dir,
        web_root,
        systemd_scope,
        admin_listen,
        admin_auth,
    }))
}

/// CLI 优先，其次环境变量，最后默认 `127.0.0.1:57879`。
fn resolve_admin_listen(
    cli_host: Option<String>,
    cli_port: Option<u16>,
) -> Result<AdminListen, String> {
    let host = cli_host
        .or_else(|| std::env::var("KEKULV_ADMIN_HOST").ok())
        .unwrap_or_else(|| DEFAULT_ADMIN_HOST.to_string());
    let port = match cli_port {
        Some(port) => port,
        None => match std::env::var("KEKULV_ADMIN_PORT") {
            Ok(value) => parse_admin_port_arg(&value)
                .map_err(|error| format!("KEKULV_ADMIN_PORT: {error}"))?,
            Err(_) => DEFAULT_ADMIN_PORT,
        },
    };
    AdminListen::new(host, port)
}

fn parse_admin_port_arg(value: &str) -> Result<u16, String> {
    let port: u16 = value
        .parse()
        .map_err(|_| format!("Admin 端口无效: {value}（需要 1...65535）"))?;
    if port == 0 {
        return Err("Admin 端口必须在 1...65535".into());
    }
    Ok(port)
}

/// CLI 优先，其次环境变量，最后是 `<config-dir>/admin-password`。
/// 不接受明文密码环境变量，避免进入进程环境和 unit 文件。
fn resolve_admin_auth(
    cli_password_file: Option<PathBuf>,
    config_dir: &Path,
) -> Result<AdminAuth, String> {
    let env_password_file = std::env::var_os("KEKULV_ADMIN_PASSWORD_FILE").map(PathBuf::from);
    resolve_admin_auth_from(cli_password_file, env_password_file, config_dir)
}

fn resolve_admin_auth_from(
    cli_password_file: Option<PathBuf>,
    env_password_file: Option<PathBuf>,
    config_dir: &Path,
) -> Result<AdminAuth, String> {
    let password_file = cli_password_file
        .or(env_password_file)
        .unwrap_or_else(|| config_dir.join(DEFAULT_ADMIN_PASSWORD_FILENAME));
    AdminAuth::from_password_file(&password_file)
}

fn print_usage() {
    eprintln!(
        "用法: sumpterd [--config-dir <dir>] [--web-root <dir> | --no-web] \
[--systemd-scope user|system] [--admin-host <ip>] [--admin-port <port>] \
[--admin-password-file <path>] \
[--foreground] [--version]\n\
环境变量: KEKULV_ADMIN_HOST / KEKULV_ADMIN_PORT / KEKULV_ADMIN_PASSWORD_FILE / KEKULV_WEB_ROOT\n\
Admin 默认 {DEFAULT_ADMIN_HOST}:{DEFAULT_ADMIN_PORT}；凭据默认读取 <config-dir>/{DEFAULT_ADMIN_PASSWORD_FILENAME}"
    );
}

fn default_web_root() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("KEKULV_WEB_ROOT") {
        let path = PathBuf::from(path);
        if path.is_dir() {
            return Some(path);
        }
    }
    let executable = std::env::current_exe().ok()?;
    let prefix = executable.parent()?.parent()?;
    let installed = prefix.join("share/kekulv/web");
    if installed.is_dir() {
        return Some(installed);
    }
    let local = PathBuf::from("web");
    local.is_dir().then_some(local)
}

async fn run(options: Options) -> ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let config_dir = ConfigDir::new(options.config_dir);
    if let Err(error) = config_dir.ensure_exists() {
        eprintln!("创建配置目录失败: {error}");
        return ExitCode::FAILURE;
    }
    let loaded = match config_dir.load_or_initialize_config_with_notice() {
        Ok(result) => result,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let config = loaded.config;
    let created = loaded.created;
    let migration_notice = loaded.migration_notice;
    if created {
        tracing::info!(path = %config_dir.config_path().display(), "已创建 schema v5 bootstrap 配置");
    }
    if let Err(error) = validate_config(&config) {
        eprintln!("config.json 校验失败: {error}");
        return ExitCode::FAILURE;
    }
    let normalized_config = config.clone().normalized();
    if normalized_config != config {
        let outcome = match config_dir.save_config(&normalized_config) {
            Ok(outcome) => outcome,
            Err(error) => {
                eprintln!("迁移统一 Provider 配置失败: {error}");
                return ExitCode::FAILURE;
            }
        };
        if let Some(warning) = outcome.durability_warning() {
            tracing::warn!("{warning}");
        }
        tracing::info!(path = %config_dir.config_path().display(), "已将旧池配置迁移为统一 Provider 池");
    }
    let config = normalized_config;
    if let Err(error) = validate_config(&config) {
        eprintln!("config.json 归一化后校验失败: {error}");
        return ExitCode::FAILURE;
    }

    let engine = Engine::new(
        config,
        Some(config_dir.clone()),
        Arc::new(ReqwestTransport::new()),
    );
    // 在开放任何 listener 前接管终止信号，避免 systemd 在启动窗口内按 Unix
    // 默认动作直接杀进程，绕过 Proxy 关停与 stats 落盘。
    let mut sighup = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()) {
        Ok(signal) => signal,
        Err(error) => {
            eprintln!("安装 SIGHUP handler 失败: {error}");
            if let Err(error) = engine.flush_stats() {
                tracing::warn!("启动失败时统计落盘失败: {error}");
            }
            if let Err(error) = engine.flush_session_affinity() {
                tracing::warn!("启动失败时会话粘性落盘失败: {error}");
            }
            return ExitCode::FAILURE;
        }
    };
    let mut sigterm = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        Ok(signal) => signal,
        Err(error) => {
            eprintln!("安装 SIGTERM handler 失败: {error}");
            if let Err(error) = engine.flush_stats() {
                tracing::warn!("启动失败时统计落盘失败: {error}");
            }
            if let Err(error) = engine.flush_session_affinity() {
                tracing::warn!("启动失败时会话粘性落盘失败: {error}");
            }
            return ExitCode::FAILURE;
        }
    };
    let mut sigint = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
    {
        Ok(signal) => signal,
        Err(error) => {
            eprintln!("安装 SIGINT handler 失败: {error}");
            if let Err(error) = engine.flush_stats() {
                tracing::warn!("启动失败时统计落盘失败: {error}");
            }
            return ExitCode::FAILURE;
        }
    };
    engine.spawn_stats_flusher();
    let proxy = ProxySupervisor::new(engine.clone());
    let admin_listen = options.admin_listen;
    let admin_auth = options.admin_auth;
    let state = AdminState::with_systemd_scope_and_auth(
        engine.clone(),
        proxy.clone(),
        config_dir.clone(),
        options.web_root,
        options.systemd_scope,
        admin_listen.clone(),
        admin_auth.clone(),
    );
    if let Some(notice) = migration_notice {
        tracing::info!(
            from_schema = notice.from_schema,
            to_schema = notice.to_schema,
            endpoint_count = notice.endpoint_count,
            expanded_legacy_passthrough_endpoints = notice.expanded_legacy_passthrough_endpoints,
            backup_file = %notice.backup_file,
            "config.json schema 迁移完成"
        );
        state.publish_migration_notice(notice);
    }
    let admin_address = admin_listen.socket_addr();
    let (admin_local, admin_task) =
        match server::serve_router(admin_router(state.clone()), admin_address).await {
            Ok(result) => result,
            Err(error) => {
                eprintln!("Admin 监听失败 {admin_address}: {error}");
                return ExitCode::FAILURE;
            }
        };

    // Admin 必须先占住管理端口。即使用户把 Proxy 配到同一端口，Admin 仍保持可用，
    // Proxy 启动错误通过状态页暴露，用户可在 WebUI 修正后重试。
    if let Err(error) = proxy.start().await {
        tracing::error!("{error}");
        engine.set_last_error(Some(error));
    }

    let pid = std::process::id();
    if let Err(error) = write_pid(&config_dir.pid_path(), pid) {
        tracing::warn!("PID 文件写入失败: {error}");
    }
    tracing::info!(
        admin = %admin_local,
        admin_host = %admin_listen.host,
        admin_port = admin_listen.port,
        admin_auth = admin_auth.mode(),
        pid,
        "sumpterd Linux daemon ready"
    );

    loop {
        tokio::select! {
            _ = sighup.recv() => {
                match state.reload_from_disk().await {
                    Ok(result) => tracing::info!(generation = %result.generation, "SIGHUP 配置重载完成"),
                    Err(error) => {
                        engine.set_last_error(Some(error.clone()));
                        tracing::error!("SIGHUP 配置重载失败: {error}");
                    }
                }
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM，开始关停");
                break;
            }
            _ = sigint.recv() => {
                tracing::info!("SIGINT，开始关停");
                break;
            }
        }
    }

    // 先停止 Admin 接入，避免关停途中又收到 start/PUT；再停止数据面并落盘。
    admin_task.shutdown().await;
    let _ = proxy.stop().await;
    if let Err(error) = engine.flush_stats() {
        tracing::warn!("关停统计落盘失败: {error}");
    }
    if let Err(error) = engine.flush_diagnostic_capture() {
        tracing::warn!("关停诊断捕获落盘失败: {error}");
    }
    if let Err(error) = engine.flush_session_affinity() {
        tracing::warn!("关停会话粘性落盘失败: {error}");
    }
    let _ = std::fs::remove_file(config_dir.pid_path());
    ExitCode::SUCCESS
}

fn write_pid(path: &Path, pid: u32) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    writeln!(file, "{pid}")?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_password_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "kekulv-admin-password-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn password_file_accepts_one_trailing_crlf() {
        let path = temp_password_path("crlf");
        std::fs::write(&path, b"correct horse battery staple\r\n").unwrap();
        let loaded = AdminAuth::from_password_file(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(
            loaded,
            AdminAuth::password(b"correct horse battery staple").unwrap()
        );
    }

    #[test]
    fn password_file_rejects_empty_multiline_and_oversized_values() {
        for (label, value) in [
            ("empty", Vec::new()),
            ("multiline", b"first\nsecond\n".to_vec()),
            ("oversized", vec![b'x'; 16 * 1024 + 1]),
        ] {
            let path = temp_password_path(label);
            std::fs::write(&path, value).unwrap();
            assert!(AdminAuth::from_password_file(&path).is_err(), "{label}");
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn admin_password_path_priority_and_default_are_deterministic() {
        let config_dir = temp_password_path("config-dir");
        std::fs::create_dir(&config_dir).unwrap();
        let default_path = config_dir.join(DEFAULT_ADMIN_PASSWORD_FILENAME);
        let env_path = temp_password_path("env");
        let cli_path = temp_password_path("cli");
        std::fs::write(&default_path, b"default-password\n").unwrap();
        std::fs::write(&env_path, b"env-password\n").unwrap();
        std::fs::write(&cli_path, b"cli-password\n").unwrap();

        assert_eq!(
            resolve_admin_auth_from(Some(cli_path.clone()), Some(env_path.clone()), &config_dir)
                .unwrap(),
            AdminAuth::password(b"cli-password").unwrap()
        );
        assert_eq!(
            resolve_admin_auth_from(None, Some(env_path.clone()), &config_dir).unwrap(),
            AdminAuth::password(b"env-password").unwrap()
        );
        assert_eq!(
            resolve_admin_auth_from(None, None, &config_dir).unwrap(),
            AdminAuth::password(b"default-password").unwrap()
        );

        std::fs::remove_file(default_path).unwrap();
        std::fs::remove_file(env_path).unwrap();
        std::fs::remove_file(cli_path).unwrap();
        std::fs::remove_dir(config_dir).unwrap();
    }

    #[test]
    fn missing_default_admin_password_file_is_an_error() {
        let config_dir = temp_password_path("missing-config-dir");
        std::fs::create_dir(&config_dir).unwrap();
        let error = resolve_admin_auth_from(None, None, &config_dir).unwrap_err();
        assert!(error.contains("admin-password"));
        std::fs::remove_dir(config_dir).unwrap();
    }
}
