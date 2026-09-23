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
use crate::model_catalog::{ModelCatalogScheduler, ModelCatalogStatus, ProviderCatalogSnapshot};

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
    pub fn spawn_model_catalog_scheduler(&self) -> tokio::task::JoinHandle<()> {
        ModelCatalogScheduler::new(self.clone()).start()
    }

    pub async fn model_catalog_status(&self) -> ModelCatalogStatus {
        ModelCatalogScheduler::new(self.clone()).status().await
    }

    pub async fn refresh_model_catalog_now(&self) {
        ModelCatalogScheduler::new(self.clone())
            .refresh_once()
            .await;
    }

    pub fn apply_provider_catalog_snapshot(
        &self,
        snapshot: &ProviderCatalogSnapshot,
    ) -> Result<bool, String> {
        for _attempt in 0..2 {
            let _guard = self.inner.config_persistence.lock().unwrap();
            let expected_generation = self.generation();
            let current = self.config();
            let Some(endpoint) = current.endpoint(&snapshot.endpoint_id) else {
                return Ok(false);
            };
            let mut next = current.as_ref().clone();
            let target = next
                .endpoint_mut(&snapshot.endpoint_id)
                .ok_or_else(|| "模型目录入口已不存在".to_string())?;
            let mut catalog = target.catalog.clone().unwrap_or_default();
            catalog.attempted_at = snapshot.attempted_at.clone();
            if let Some(error) = &snapshot.error {
                catalog.status = "获取失败".into();
                catalog.error = error.clone();
            } else {
                catalog.models = snapshot.models.clone();
                catalog.source = snapshot.source.clone();
                catalog.status = "已获取".into();
                catalog.error.clear();
                catalog.updated_at = snapshot.updated_at.clone();
            }
            if endpoint.catalog.as_ref() == Some(&catalog) {
                return Ok(false);
            }
            if self.generation() != expected_generation {
                continue;
            }
            target.catalog = Some(catalog);
            next = next.normalized();
            if let Some(dir) = &self.inner.dir {
                let _ = dir
                    .save_config(&next)
                    .map_err(|error| format!("模型目录配置写盘失败: {error}"))?;
            }
            let (generation, _) = self.replace_config(next);
            let model_count = self
                .config()
                .endpoint(&snapshot.endpoint_id)
                .and_then(|endpoint| endpoint.catalog.as_ref())
                .map_or(0, |catalog| catalog.models.len());
            let _ = self.inner.notices.send(EngineNotice::ModelCatalogUpdated {
                endpoint_id: snapshot.endpoint_id.clone(),
                model_count,
                generation,
            });
            return Ok(true);
        }
        Err("配置在目录刷新期间连续变化，本次目录结果未写入".into())
    }

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
        // TTL 等调度参数跟随配置替换即时生效；粘性归属本身不动，
        // 只影响后续的周期淘汰。
        let session_sticky_ttl_secs = config.session_sticky_ttl_secs();
        *self.inner.config.write().unwrap() = Arc::new(config);
        {
            let config = self.config();
            let mut status = self.inner.model_catalog_status.write().unwrap();
            status.enabled = config.model_catalog.auto_refresh;
            status.refresh_on_startup = config.model_catalog.refresh_on_startup;
            status.interval_minutes = config.model_catalog.refresh_interval_minutes;
            status.remote_metadata.enabled = config.model_catalog.remote_metadata_enabled;
        }
        {
            let mut state = self.inner.state.lock().unwrap();
            state.session_sticky_ttl_secs = session_sticky_ttl_secs;
            state.round_robin_cursors.clear();
        }
        *self.inner.generation.write().unwrap() = generation.clone();
        let _ = self.inner.notices.send(EngineNotice::ConfigReloaded {
            generation: generation.clone(),
        });
        (generation, warnings)
    }

    /// Reload a validated schema-v7 configuration from the configured
    /// directory.  The old in-memory configuration remains active when
    /// loading, validation, or migration verification fails.
    pub fn reload_config(&self) -> Result<ConfigReplacement, String> {
        let Some(dir) = &self.inner.dir else {
            return Err("no_reload_handler".into());
        };
        let loaded = dir
            .load_config_with_notice()
            .map_err(|error| error.to_string())?;
        loaded.config.validate_resolve_ips()?;
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
