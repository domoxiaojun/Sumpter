//! 模型名解析:剥尾部 `[1m]` 标记与合法 `(effort)` 后缀、`prefix-*` 通配匹配。
//! 对齐 Swift `ModelName` / `ModelPattern`(Models.swift)。

use serde::{Deserialize, Serialize};

/// CPA / CLIProxyAPI 兼容的 reasoning effort 档位(模型名后缀 `model(high)`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Auto,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Ultra,
}

impl ReasoningEffort {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "none" => Some(Self::None),
            "auto" => Some(Self::Auto),
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            "max" => Some(Self::Max),
            "ultra" => Some(Self::Ultra),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Auto => "auto",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedModel {
    pub base_name: String,
    pub effort: Option<ReasoningEffort>,
}

/// 解析模型名:去掉尾部 `[...]`(一次)与合法 `(effort)` 后缀。
///
/// 与 Swift `ModelName.parse` 逐步对齐:先 trim,再剥尾部 `\[[^\]]*\]\s*$`,再 trim;
/// 然后仅当最后一对括号内(trim + lowercase 后)是已知 effort 档位才剥离,
/// 未知括号原样保留,避免误伤真实模型 id。
pub fn parse(model: &str) -> ParsedModel {
    let mut cleaned = model.trim().to_string();

    // 尾部 [..] 标记(如 `[1m]`):等价于正则 `\[[^\]]*\]\s*$`,只剥一次。
    if cleaned.ends_with(']')
        && let Some(open) = cleaned.rfind('[')
    {
        let inner = &cleaned[open + 1..cleaned.len() - 1];
        if !inner.contains(']') {
            cleaned.truncate(open);
            cleaned = cleaned.trim().to_string();
        }
    }

    if cleaned.ends_with(')')
        && let Some(open) = cleaned.rfind('(')
    {
        let raw = cleaned[open + 1..cleaned.len() - 1].trim().to_lowercase();
        if let Some(effort) = ReasoningEffort::parse(&raw) {
            let base = cleaned[..open].trim().to_string();
            return ParsedModel {
                base_name: base,
                effort: Some(effort),
            };
        }
    }
    ParsedModel {
        base_name: cleaned,
        effort: None,
    }
}

pub fn clean(model: &str) -> String {
    parse(model).base_name
}

pub fn reasoning_effort(model: &str) -> Option<ReasoningEffort> {
    parse(model).effort
}

/// 客户端模型匹配:精确名或 `claude-opus-*` 前缀通配;双方先 clean。
/// 空 pattern 恒不命中。对齐 Swift `ModelPattern.matches`。
pub fn pattern_matches(pattern: &str, model: &str) -> bool {
    let cleaned_model = clean(model);
    let cleaned_pattern = clean(pattern);
    if cleaned_pattern.is_empty() {
        return false;
    }
    if let Some(prefix) = cleaned_pattern.strip_suffix('*') {
        cleaned_model.starts_with(prefix)
    } else {
        cleaned_model == cleaned_pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_bracket_and_effort_suffix() {
        assert_eq!(clean("claude-fable-5[1m]"), "claude-fable-5");
        assert_eq!(clean("gpt-5.6-luna(high)[1m]"), "gpt-5.6-luna");
        assert_eq!(clean("gpt-5.6-luna(high)"), "gpt-5.6-luna");
        assert_eq!(clean("  claude-opus-5  "), "claude-opus-5");
        assert_eq!(reasoning_effort("m(XHIGH)"), Some(ReasoningEffort::Xhigh));
        assert_eq!(reasoning_effort("m (max)"), Some(ReasoningEffort::Max));
        assert_eq!(reasoning_effort("m(none)"), Some(ReasoningEffort::None));
    }

    #[test]
    fn keeps_unknown_parens_and_brackets() {
        assert_eq!(clean("model(custom)"), "model(custom)");
        assert_eq!(reasoning_effort("model(custom)"), None);
        assert_eq!(clean("model(ultra)"), "model");
        // `[..]` 只剥最尾部一段。
        assert_eq!(clean("a[x]b"), "a[x]b");
        assert_eq!(clean("model[beta][1m]"), "model[beta]");
    }

    #[test]
    fn pattern_matching() {
        assert!(pattern_matches("claude-opus-*", "claude-opus-4-8"));
        assert!(pattern_matches("claude-opus-*", "claude-opus-5[1m]"));
        assert!(pattern_matches("claude-fable-5", "claude-fable-5(high)"));
        assert!(!pattern_matches("claude-opus-*", "claude-sonnet-5"));
        assert!(!pattern_matches("", "anything"));
        assert!(!pattern_matches("   ", "anything"));
        assert!(pattern_matches(" claude-fable-5 ", "claude-fable-5"));
    }
}
