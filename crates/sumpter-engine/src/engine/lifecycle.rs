//! Engine lifecycle, configuration replacement, and persistence scheduling.
//!
//! Listener ownership remains in the Linux/macOS adapters. This module only
//! coordinates the shared engine's background flusher and in-memory
//! configuration generation, while the individual persistence operations live
//! in their capture/runtime modules.

/// Minimal lifecycle state used by adapters and status surfaces.  Starting or
/// stopping a concrete listener remains the responsibility of the Linux/macOS
/// facade; the shared engine only exposes data-plane state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    Stopped,
    Running,
}

pub trait Lifecycle: Send + Sync {
    fn lifecycle_state(&self) -> LifecycleState;
}

use std::sync::Arc;
use std::time::Duration;

use sumpter_core::config::AppConfig;

use super::Engine;
use super::events::{EngineNotice, RuntimeEvent, RuntimeSnapshot};
use super::state::config_generation;
use crate::boundary::{ConfigReplacement, EngineCapabilities, PlatformNotice};

#[async_trait::async_trait]
impl EngineCapabilities for Engine {
    fn runtime_snapshot(&self) -> RuntimeSnapshot {
        Engine::runtime_snapshot(self)
    }

    fn replace_config(&self, config: AppConfig) -> Result<ConfigReplacement, String> {
        let (generation, warnings) = Engine::replace_config(self, config);
        Ok(ConfigReplacement {
            generation,
            warnings,
        })
    }

    fn reload_config(&self) -> Result<ConfigReplacement, String> {
        Engine::reload_config(self)
    }

    fn record_platform_event(&self, event: RuntimeEvent) {
        self.record_event(event);
    }

    fn publish_platform_notice(&self, notice: PlatformNotice) {
        let _ = self
            .inner
            .notices
            .send(EngineNotice::PlatformNotice(notice));
    }
}
impl Engine {
    /// session affinity 的后台防抖任务。SQLite 自己的专用 worker 已按
    /// 1 秒/条数/字节阈值提交，不能在 Tokio 请求线程重复 fsync。
    pub fn spawn_stats_flusher(&self) {
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let flush_engine = engine.clone();
                if let Err(error) = tokio::task::spawn_blocking(move || {
                    flush_engine.flush_session_affinity_if_dirty();
                    flush_engine.flush_resource_bindings_if_dirty();
                    flush_engine.flush_diagnostic_capture_if_dirty();
                })
                .await
                {
                    tracing::warn!("后台持久化任务异常退出: {error}");
                }
            }
        });
    }

    pub fn replace_config(&self, config: AppConfig) -> (String, Vec<String>) {
        let warnings = sumpter_core::warnings::evaluate(&config);
        let generation = config_generation(&config);
        *self.inner.config.write().unwrap() = Arc::new(config);
        *self.inner.generation.write().unwrap() = generation.clone();
        let _ = self.inner.notices.send(EngineNotice::ConfigReloaded {
            generation: generation.clone(),
        });
        (generation, warnings)
    }

    /// Reload a validated schema-v6 configuration from the configured
    /// directory.  The old in-memory configuration remains active when
    /// loading, validation, or migration verification fails.
    pub fn reload_config(&self) -> Result<ConfigReplacement, String> {
        let Some(dir) = &self.inner.dir else {
            return Err("no_reload_handler".into());
        };
        let loaded = dir
            .load_config_with_notice()
            .map_err(|error| error.to_string())?;
        let (generation, warnings) = self.replace_config(loaded.config.normalized());
        if let Some(notice) = loaded.migration_notice {
            self.publish_platform_notice(PlatformNotice::Migration(notice));
        }
        Ok(ConfigReplacement {
            generation,
            warnings,
        })
    }
}
