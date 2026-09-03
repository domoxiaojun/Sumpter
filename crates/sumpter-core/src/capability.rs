//! Model capability catalog used by intent routing.
//!
//! Explicit `mapping.capabilities` wins. When omitted, the client pattern is
//! classified from its name so existing configs keep working without a UI
//! change: Grok Imagine Video stays video, Grok Imagine Image stays image,
//! and ordinary chat models stay text.

use crate::model_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelCapability {
    Text,
    Image,
    Video,
    /// Realtime / Live 语音面。由 `has_exact_codex_live_mapping` 消费,用来确认某个
    /// 入口真的声明了 Live 模型,而不是靠名字猜。
    Live,
    /// Files 资源面。显式 `capabilities: ["files"]` 时由
    /// `RoutePlanner::plan_for_resource_capability` 选入口；未声明时优先混合
    /// 媒体/Live 入口，再回退到非 Anthropic 入口顺序。GET `/v1/models` 不走这条
    /// 路径，由数据面按 mapping 生成本地目录。
    Files,
}

impl ModelCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Video => "video",
            Self::Live => "live",
            Self::Files => "files",
        }
    }
}

/// Infer capabilities from a mapping pattern when none were declared.
pub fn inferred_capabilities(client_pattern: &str) -> Vec<ModelCapability> {
    let cleaned = model_name::clean(client_pattern).to_ascii_lowercase();
    let stem = cleaned.strip_suffix("-*").unwrap_or(&cleaned);
    if is_live_stem(stem) {
        vec![ModelCapability::Live]
    } else if is_video_stem(stem) {
        vec![ModelCapability::Video]
    } else if is_image_stem(stem) {
        vec![ModelCapability::Image]
    } else {
        vec![ModelCapability::Text]
    }
}

pub fn mapping_has_capability(
    declared: &[ModelCapability],
    client_pattern: &str,
    wanted: ModelCapability,
) -> bool {
    let caps = if declared.is_empty() {
        inferred_capabilities(client_pattern)
    } else {
        declared.to_vec()
    };
    caps.contains(&wanted)
}

pub fn canonical_model_from_pattern(client_pattern: &str) -> String {
    let cleaned = model_name::clean(client_pattern);
    cleaned.strip_suffix("-*").unwrap_or(&cleaned).to_string()
}

fn is_live_stem(stem: &str) -> bool {
    stem == "gpt-live-1-codex"
        || stem == "gpt-realtime"
        || stem.starts_with("gpt-realtime-")
        || stem.contains("realtime-preview")
        // OpenAI's first public Realtime deployments used gpt-4o (and
        // gpt-4o-mini) as the session model. Keep these names voice-capable
        // when older configs did not yet carry an explicit capability list;
        // normal text routing is unaffected because this filter is only used
        // by voice intent planning.
        || stem == "gpt-4o"
        || stem == "gpt-4o-mini"
}

fn is_video_stem(stem: &str) -> bool {
    stem.contains("imagine-video") || stem.starts_with("sora")
}

fn is_image_stem(stem: &str) -> bool {
    stem.contains("imagine-image") || stem.starts_with("gpt-image")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_and_sora_names_infer_media_intents() {
        assert_eq!(
            inferred_capabilities("grok-imagine-video"),
            vec![ModelCapability::Video]
        );
        assert_eq!(
            inferred_capabilities("grok-imagine-video-1.5-preview"),
            vec![ModelCapability::Video]
        );
        assert_eq!(
            inferred_capabilities("sora-2"),
            vec![ModelCapability::Video]
        );
        assert_eq!(
            inferred_capabilities("grok-imagine-image"),
            vec![ModelCapability::Image]
        );
        assert_eq!(
            inferred_capabilities("gpt-image-2"),
            vec![ModelCapability::Image]
        );
        assert_eq!(
            inferred_capabilities("gpt-live-1-codex"),
            vec![ModelCapability::Live]
        );
        assert_eq!(
            inferred_capabilities("claude-fable-5"),
            vec![ModelCapability::Text]
        );
        assert_eq!(
            inferred_capabilities("grok-4.6"),
            vec![ModelCapability::Text]
        );
    }

    #[test]
    fn declared_capabilities_override_name_inference() {
        assert!(!mapping_has_capability(
            &[ModelCapability::Text],
            "grok-imagine-video",
            ModelCapability::Video
        ));
        assert!(mapping_has_capability(
            &[ModelCapability::Video],
            "custom-media",
            ModelCapability::Video
        ));
        assert!(mapping_has_capability(
            &[],
            "grok-imagine-video-*",
            ModelCapability::Video
        ));
    }

    #[test]
    fn capabilities_use_stable_lowercase_wire_values() {
        let mapping = serde_json::json!({
            "clientPattern": "custom-video",
            "upstreamModel": "grok-imagine-video",
            "capabilities": ["video"]
        });
        let decoded: crate::config::ModelMapping = serde_json::from_value(mapping).unwrap();
        assert_eq!(decoded.capabilities, vec![ModelCapability::Video]);
        let wire = serde_json::to_value(decoded).unwrap();
        assert_eq!(wire["capabilities"], serde_json::json!(["video"]));
    }
}
