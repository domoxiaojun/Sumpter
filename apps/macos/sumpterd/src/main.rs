//! sumpterd —— Sumpter Rust 后端 sidecar。
//!
//! 由 SwiftUI 壳 spawn:
//! - 启动完成后向 stdout 写一行握手 JSON(adminPort/proxyPort/pid/generation);
//! - **stdin EOF = 父进程退出信号**,随之优雅退出(`--foreground` 关闭该监视,供手动调试);
//! - SIGTERM/SIGINT → flush 统计 → 退出;
//! - pid 写入配置目录 `sumpterd.pid`,退出时清理。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use serde_json::json;
use sumpter_core::config::AppConfig;
use sumpter_core::config_store::{ConfigDir, ConfigLoadError, default_config_dir};
use sumpter_macos_adapter::engine::Engine;
use sumpter_macos_adapter::outbound::ReqwestTransport;
use sumpter_macos_adapter::server;

fn main() -> ExitCode {
    let mut config_dir_arg: Option<String> = None;
    let mut foreground = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config-dir" => config_dir_arg = args.next(),
            "--foreground" => foreground = true,
            "--version" => {
                println!("sumpterd {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("未知参数: {other}(支持 --config-dir <path> / --foreground / --version)");
                return ExitCode::from(2);
            }
        }
    }

    let root = config_dir_arg
        .map(PathBuf::from)
        .or_else(default_config_dir);
    let Some(root) = root else {
        eprintln!("无法确定配置目录(HOME 未设置且未传 --config-dir)");
        return ExitCode::from(2);
    };
    let dir = ConfigDir::new(root);
    if let Err(e) = dir.ensure_exists() {
        eprintln!("创建配置目录失败: {e}");
        return ExitCode::from(2);
    }

    // 配置缺失 = 全新安装,用 bootstrap 空壳起服务(Swift 壳稍后写入并 reload);
    // 配置损坏 = 报错退出,绝不覆盖原文件。
    let (config, migration_notice) = match dir.load_config_with_notice() {
        Ok(loaded) => {
            let original = loaded.config;
            let normalized = original.clone().normalized();
            if normalized != original {
                if let Err(error) = dir.save_config(&normalized) {
                    eprintln!("配置归一化迁移写盘失败: {error}");
                    return ExitCode::FAILURE;
                }
                eprintln!("已将旧的池级模型规则迁移为入口显式映射");
            }
            (normalized, loaded.migration_notice)
        }
        Err(ConfigLoadError::Io { .. }) => {
            eprintln!("config.json 不存在,以 bootstrap 空配置启动");
            (AppConfig::bootstrap().normalized(), None)
        }
        Err(e @ ConfigLoadError::Parse { .. }) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let port = config.listener.port;
    if !(0..=65535).contains(&port) {
        eprintln!("invalid port: {port}");
        return ExitCode::FAILURE;
    }

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tokio runtime 启动失败: {e}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run(dir, config, migration_notice, foreground))
}

async fn run(
    dir: ConfigDir,
    config: AppConfig,
    migration_notice: Option<sumpter_core::config_store::MigrationNotice>,
    foreground: bool,
) -> ExitCode {
    // 日志走 stderr(stdout 只发握手行);RUST_LOG 可调级别。
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let token = match dir.ensure_control_token() {
        Ok(token) => token,
        Err(e) => {
            eprintln!("control token 初始化失败: {e}");
            return ExitCode::FAILURE;
        }
    };

    let host = config.listener.host.clone();
    let port = config.listener.port as u16;
    let engine = Engine::new(
        config,
        Some(dir.clone()),
        Arc::new(ReqwestTransport::new()),
        token,
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
        engine.publish_migration_notice(notice);
    }
    engine.spawn_stats_flusher();

    let proxy_local = if let Some(issue) = engine.runtime_database_issue() {
        tracing::error!("{message}", message = issue.message);
        SocketAddr::from(([127, 0, 0, 1], 0))
    } else {
        let proxy_addr = server::bind_address(&host, port);
        match server::serve(engine.clone(), proxy_addr).await {
            Ok((local, _task)) => local,
            Err(e) => {
                eprintln!("proxy 监听失败 {proxy_addr}: {e}");
                return ExitCode::FAILURE;
            }
        }
    };

    let admin_router = sumpter_macos_adapter::admin::admin_router(engine.clone());
    let admin_addr = server::bind_address("127.0.0.1", 0);
    let (admin_local, _admin_task) = match server::serve_router(admin_router, admin_addr).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("admin 监听失败: {e}");
            return ExitCode::FAILURE;
        }
    };

    let pid = std::process::id();
    let _ = std::fs::write(dir.pid_path(), pid.to_string());

    // 握手:Swift 壳读 stdout 第一行拿 admin 端口。
    println!(
        "{}",
        json!({
            "event": "ready",
            "pid": pid,
            "proxyPort": proxy_local.port(),
            "adminPort": admin_local.port(),
            "generation": engine.generation(),
        })
    );
    use std::io::Write;
    let _ = std::io::stdout().flush();
    tracing::info!("sumpterd ready: proxy={proxy_local} admin={admin_local}");

    // 等待退出信号。
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! {
        _ = stdin_eof(), if !foreground => {
            tracing::info!("stdin EOF(父进程退出),关停");
        }
        _ = sigterm.recv() => {
            tracing::info!("SIGTERM,关停");
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("SIGINT,关停");
        }
    }

    if let Err(error) = engine.flush_stats() {
        tracing::warn!("统计落盘失败: {error}");
    }
    if let Err(error) = engine.flush_diagnostic_capture() {
        tracing::warn!("诊断捕获落盘失败: {error}");
    }
    if let Err(error) = engine.flush_session_affinity() {
        tracing::warn!("会话粘性落盘失败: {error}");
    }
    if let Err(error) = engine.flush_resource_bindings() {
        tracing::warn!("资源绑定落盘失败: {error}");
    }
    let _ = std::fs::remove_file(dir.pid_path());
    ExitCode::SUCCESS
}

/// 读 stdin 直到 EOF(父进程持有写端;其退出/崩溃即触发)。
async fn stdin_eof() {
    use tokio::io::AsyncReadExt;
    let mut stdin = tokio::io::stdin();
    let mut buffer = [0u8; 256];
    loop {
        match stdin.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(_) => continue,
        }
    }
}
