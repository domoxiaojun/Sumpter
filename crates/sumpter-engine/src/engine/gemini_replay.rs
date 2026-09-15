//! Gemini `thoughtSignature` 的会话回放状态。
//!
//! Gemini 要求把上一轮签发的不透明签名在下一轮原样带回。代理是链路上唯一见过它的
//! 一方(客户端拿到的是转换后的 Anthropic/OpenAI 方言),所以必须在会话内保存。
//!
//! 状态刻意只留在进程内,并且:
//! - 键包含入口、真实上游模型与会话标识:不同入口或模型之间绝不共享;
//! - 有 TTL、条目数与单条字节上限,过期或超限即失效 —— 宁可让这一轮没有签名,
//!   也不能为了保留它无限增长,更不能截断 parts 后当作完整回放;
//! - 只有流正常结束的尝试才写入:失败、取消与 failover 的未完成尝试若写进去,
//!   下一轮就会拿一份并不存在的历史去回放。

use std::collections::HashMap;

use serde_json::Value;
use sumpter_core::bridge_gemini::GeminiReplay;

/// 单条会话最多保留的 parts 字节数。
const MAX_ENTRY_BYTES: usize = 256 * 1024;
/// 全局最多保留的会话数。
const MAX_ENTRIES: usize = 512;
/// 条目存活秒数:超过就当作过期(签名是不是还有效由上游决定,这里只是不让状态无限增长)。
const TTL_SECS: f64 = 30.0 * 60.0;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct GeminiReplayKey {
    pub(super) endpoint_id: String,
    pub(super) upstream_model: String,
    pub(super) session: String,
}

struct GeminiReplayEntry {
    replay: GeminiReplay,
    updated_at: f64,
}

#[derive(Default)]
pub(super) struct GeminiReplayStore {
    entries: HashMap<GeminiReplayKey, GeminiReplayEntry>,
}

impl GeminiReplayStore {
    pub(super) fn record(&mut self, key: GeminiReplayKey, parts: Vec<Value>, now: f64) {
        if parts.is_empty() {
            return;
        }
        let bytes = serde_json::to_vec(&parts)
            .map(|raw| raw.len())
            .unwrap_or(usize::MAX);
        if bytes > MAX_ENTRY_BYTES {
            // 超限就不存:宁可这一轮没有签名可用,也不要截断 parts 后当成完整的
            // 历史回放出去。
            self.entries.remove(&key);
            return;
        }
        self.evict(now);
        self.entries.insert(
            key,
            GeminiReplayEntry {
                replay: GeminiReplay { parts },
                updated_at: now,
            },
        );
        self.enforce_capacity();
    }

    pub(super) fn lookup(&mut self, key: &GeminiReplayKey, now: f64) -> Option<GeminiReplay> {
        self.evict(now);
        self.entries.get(key).map(|entry| entry.replay.clone())
    }

    fn evict(&mut self, now: f64) {
        self.entries
            .retain(|_, entry| now - entry.updated_at <= TTL_SECS);
    }

    fn enforce_capacity(&mut self) {
        if self.entries.len() <= MAX_ENTRIES {
            return;
        }
        let mut ages: Vec<(f64, GeminiReplayKey)> = self
            .entries
            .iter()
            .map(|(key, entry)| (entry.updated_at, key.clone()))
            .collect();
        ages.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        for (_, key) in ages.into_iter().take(self.entries.len() - MAX_ENTRIES) {
            self.entries.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(session: &str) -> GeminiReplayKey {
        GeminiReplayKey {
            endpoint_id: "ep".into(),
            upstream_model: "gemini-upstream".into(),
            session: session.into(),
        }
    }

    fn parts(signature: &str) -> Vec<Value> {
        vec![
            json!({"functionCall": {"name": "read_file", "args": {}}, "thoughtSignature": signature}),
        ]
    }

    #[test]
    fn round_trips_within_the_same_session_only() {
        let mut store = GeminiReplayStore::default();
        store.record(key("s1"), parts("sig"), 100.0);
        assert!(store.lookup(&key("s1"), 100.0).is_some());
        // 别的会话拿不到:签名不能跨会话或跨入口搬运。
        assert!(store.lookup(&key("s2"), 100.0).is_none());
        assert!(
            store
                .lookup(
                    &GeminiReplayKey {
                        upstream_model: "other".into(),
                        ..key("s1")
                    },
                    100.0
                )
                .is_none()
        );
    }

    #[test]
    fn entries_expire_and_capacity_is_bounded() {
        let mut store = GeminiReplayStore::default();
        store.record(key("s1"), parts("sig"), 100.0);
        assert!(store.lookup(&key("s1"), 100.0 + TTL_SECS + 1.0).is_none());

        for index in 0..(MAX_ENTRIES + 32) {
            store.record(key(&format!("s{index}")), parts("sig"), 200.0);
        }
        assert!(store.entries.len() <= MAX_ENTRIES);
    }

    #[test]
    fn oversized_entries_are_dropped_rather_than_truncated() {
        let mut store = GeminiReplayStore::default();
        store.record(key("s1"), parts("sig"), 100.0);
        let huge = vec![json!({"text": "x".repeat(MAX_ENTRY_BYTES)})];
        store.record(key("s1"), huge, 100.0);
        // 超限时清掉旧值,而不是留一份被截断的历史。
        assert!(store.lookup(&key("s1"), 100.0).is_none());
    }

    #[test]
    fn empty_parts_are_not_recorded() {
        let mut store = GeminiReplayStore::default();
        store.record(key("s1"), parts("sig"), 100.0);
        store.record(key("s1"), Vec::new(), 100.0);
        assert!(store.lookup(&key("s1"), 100.0).is_some());
    }
}
