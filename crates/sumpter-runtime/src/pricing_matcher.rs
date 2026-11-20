//! Model key normalization, best-match pricing lookup, and long-context tier calculation.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_PRICE_TIERS_PER_MODEL: usize = 8;
pub const MAX_OFFICIAL_PRICE_MODELS: usize = 10_000;
/// Bump whenever candidate key normalization, matching precedence, or cost
/// accounting completeness semantics change so existing SQLite rollups are
/// rebuilt automatically on startup.
pub const PRICING_MATCHER_VERSION: i64 = 3;

const BUILTIN_OFFICIAL_SNAPSHOT_JSON: &str = include_str!("official_pricing_snapshot.json");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPriceTier {
    /// Applies when total request input tokens strictly exceed this threshold (e.g. 200_000).
    pub above_input_tokens: i64,
    pub input_per_million_micros: Option<i64>,
    pub output_per_million_micros: Option<i64>,
    pub cache_read_per_million_micros: Option<i64>,
    pub cache_creation_per_million_micros: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialModelPriceInput {
    pub model_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub input_per_million_micros: Option<i64>,
    pub output_per_million_micros: Option<i64>,
    pub cache_read_per_million_micros: Option<i64>,
    pub cache_creation_per_million_micros: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<ModelPriceTier>,
}

#[derive(Debug, Clone)]
pub struct OfficialPricingSyncUpdate {
    pub source: String,
    pub prices: Vec<OfficialModelPriceInput>,
    pub sync_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialPricingSyncMutation {
    pub revision: i64,
    pub changed: bool,
    pub source: String,
    pub model_count: usize,
    pub tiered_model_count: usize,
    pub updated_at: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialPricingCatalogStatus {
    pub enabled: bool,
    pub source: String,
    pub model_count: usize,
    pub tiered_model_count: usize,
    pub updated_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinOfficialSnapshot {
    snapshot_version: i64,
    #[allow(dead_code)]
    currency: String,
    models: Vec<OfficialModelPriceInput>,
}

pub fn builtin_official_snapshot() -> &'static (i64, Vec<OfficialModelPriceInput>, String) {
    static SNAPSHOT: OnceLock<(i64, Vec<OfficialModelPriceInput>, String)> = OnceLock::new();
    SNAPSHOT.get_or_init(|| {
        let parsed: BuiltinOfficialSnapshot = serde_json::from_str(BUILTIN_OFFICIAL_SNAPSHOT_JSON)
            .expect("valid builtin official_pricing_snapshot.json");
        let normalized =
            normalize_official_prices(&parsed.models).expect("valid builtin official model prices");
        let digest = official_prices_digest(&normalized);
        (parsed.snapshot_version, normalized, digest)
    })
}

pub fn validate_price_tiers(tiers: &[ModelPriceTier]) -> Result<Vec<ModelPriceTier>, String> {
    if tiers.len() > MAX_PRICE_TIERS_PER_MODEL {
        return Err(format!(
            "单模型阶梯价格不能超过 {MAX_PRICE_TIERS_PER_MODEL} 档"
        ));
    }
    let mut normalized = Vec::with_capacity(tiers.len());
    for tier in tiers {
        if tier.above_input_tokens <= 0 {
            return Err("阶梯价格阈值 aboveInputTokens 必须大于 0".into());
        }
        let rates = [
            tier.input_per_million_micros,
            tier.output_per_million_micros,
            tier.cache_read_per_million_micros,
            tier.cache_creation_per_million_micros,
        ];
        if rates.iter().any(|rate| rate.is_some_and(|value| value < 0)) {
            return Err("阶梯价格不能为负数".into());
        }
        if rates.iter().all(Option::is_none) {
            return Err("阶梯价格至少需要配置一项单价".into());
        }
        normalized.push(tier.clone());
    }
    normalized.sort_by_key(|tier| tier.above_input_tokens);
    for pair in normalized.windows(2) {
        if pair[0].above_input_tokens == pair[1].above_input_tokens {
            return Err("同一模型的阶梯价格阈值 aboveInputTokens 不能重复".into());
        }
    }
    Ok(normalized)
}

pub fn serialize_tiers_json(tiers: &[ModelPriceTier]) -> Result<Option<String>, String> {
    let normalized = validate_price_tiers(tiers)?;
    if normalized.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(&normalized)
        .map(Some)
        .map_err(|error| error.to_string())
}

pub fn parse_tiers_json(raw: Option<&str>) -> Vec<ModelPriceTier> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<ModelPriceTier>>(raw)
        .ok()
        .and_then(|tiers| validate_price_tiers(&tiers).ok())
        .unwrap_or_default()
}

pub fn normalize_official_prices(
    prices: &[OfficialModelPriceInput],
) -> Result<Vec<OfficialModelPriceInput>, String> {
    if prices.len() > MAX_OFFICIAL_PRICE_MODELS {
        return Err(format!(
            "官方模型价格数量不能超过 {MAX_OFFICIAL_PRICE_MODELS}"
        ));
    }
    let mut by_key = std::collections::BTreeMap::<String, OfficialModelPriceInput>::new();
    for item in prices {
        let key = item.model_key.trim().to_ascii_lowercase();
        if key.is_empty() || key.len() > 512 || key.contains('\u{1f}') {
            return Err("官方模型 key 无效".into());
        }
        for rate in [
            item.input_per_million_micros,
            item.output_per_million_micros,
            item.cache_read_per_million_micros,
            item.cache_creation_per_million_micros,
        ] {
            if rate.is_some_and(|value| value < 0) {
                return Err(format!("{key} 的价格不能为负数"));
            }
        }
        if item.input_per_million_micros.is_none() && item.output_per_million_micros.is_none() {
            continue;
        }
        let tiers = validate_price_tiers(&item.tiers)?;
        let provider = item
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        by_key.insert(
            key.clone(),
            OfficialModelPriceInput {
                model_key: key,
                provider,
                input_per_million_micros: item.input_per_million_micros,
                output_per_million_micros: item.output_per_million_micros,
                cache_read_per_million_micros: item.cache_read_per_million_micros,
                cache_creation_per_million_micros: item.cache_creation_per_million_micros,
                tiers,
            },
        );
    }
    Ok(by_key.into_values().collect())
}

pub fn official_prices_digest(prices: &[OfficialModelPriceInput]) -> String {
    let mut hasher = Sha256::new();
    if let Ok(bytes) = serde_json::to_vec(prices) {
        hasher.update(&bytes);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

/// Normalize version digit separators so `claude-sonnet-4.6` and `claude-sonnet-4-6`
/// map to the same canonical lookup key while preserving non-digit separators.
pub(crate) fn canonical_digit_separators(model: &str) -> String {
    let bytes = model.as_bytes();
    let mut out = String::with_capacity(model.len());
    for (index, ch) in model.char_indices() {
        if ch == '.'
            && index > 0
            && index + 1 < bytes.len()
            && bytes[index - 1].is_ascii_digit()
            && bytes[index + 1].is_ascii_digit()
        {
            out.push('-');
        } else {
            out.push(ch);
        }
    }
    out
}

/// Generate candidate model strings ordered from most specific to stripped forms:
/// 1) exact trimmed lowercase model
/// 2) progressively stripped `/` prefixes (`models/`, `openai/`, `openrouter/anthropic/`, ...)
/// 3) stripped routing tags (`:default`, `:latest`, `@default`) and `@YYYYMMDD` normalized to `-YYYYMMDD`
/// 4) progressively stripped trailing snapshot/date/preview suffixes (never tier words like `mini`/`lite`/`pro`).
pub fn candidate_lookup_keys(raw_model: &str) -> Vec<String> {
    let trimmed = raw_model.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let lowered = trimmed.to_ascii_lowercase();
    let mut slash_variants = Vec::with_capacity(4);
    slash_variants.push(lowered.clone());
    let mut cursor = lowered.as_str();
    while let Some((_prefix, rest)) = cursor.split_once('/') {
        let rest = rest.trim();
        if !rest.is_empty() && !slash_variants.iter().any(|existing| existing == rest) {
            slash_variants.push(rest.to_owned());
        }
        cursor = rest;
    }

    let mut out = Vec::with_capacity(12);
    let mut push_with_digit_canonical = |candidate: &str| {
        if candidate.is_empty() {
            return;
        }
        if !out.iter().any(|existing| existing == candidate) {
            out.push(candidate.to_owned());
        }
        let digit_norm = canonical_digit_separators(candidate);
        if digit_norm != candidate && !out.iter().any(|existing| existing == &digit_norm) {
            out.push(digit_norm);
        }
    };

    for variant in &slash_variants {
        push_with_digit_canonical(variant);
        let cleaned = normalize_tag_and_at_suffix(variant);
        if cleaned != *variant {
            push_with_digit_canonical(&cleaned);
        }
        let mut base = cleaned;
        while let Some(stripped) = strip_one_non_tier_suffix(&base) {
            push_with_digit_canonical(&stripped);
            base = stripped;
        }
    }
    out
}

fn normalize_tag_and_at_suffix(model: &str) -> String {
    let mut value = model.trim().to_owned();
    for _ in 0..8 {
        let before = value.clone();

        // 1. Strip one trailing `[...]` annotation (e.g. `[1m]`, `[200k]`) without nested `]`.
        if value.ends_with(']')
            && let Some(open) = value.rfind('[')
        {
            let inner = &value[open + 1..value.len() - 1];
            let head = value[..open].trim();
            if !inner.contains(']') && !head.is_empty() {
                value = head.to_owned();
            }
        }

        // 2. Strip trailing `(effort)` suffix when the parenthesized token is a known effort level.
        if value.ends_with(')')
            && let Some(open) = value.rfind('(')
        {
            let inner = value[open + 1..value.len() - 1].trim();
            let head = value[..open].trim();
            if !head.is_empty() && is_reasoning_effort_token(inner) {
                value = head.to_owned();
            }
        }

        // 3. Strip Bedrock `:0` revision or channel/effort/thinking/context tags like `:latest`,
        //    `:xhigh`, `:thinking`, `:1m`. Never strip billing-altering tags like `:free` or `:online`.
        if let Some((head, tail)) = value.rsplit_once(':') {
            let head = head.trim();
            let tail = tail.trim();
            if !head.is_empty()
                && !tail.is_empty()
                && (tail.chars().all(|ch| ch.is_ascii_digit())
                    || matches!(
                        tail,
                        "latest"
                            | "default"
                            | "beta"
                            | "preview"
                            | "stable"
                            | "exacto"
                            | "none"
                            | "auto"
                            | "minimal"
                            | "low"
                            | "medium"
                            | "high"
                            | "xhigh"
                            | "max"
                            | "ultra"
                            | "thinking"
                            | "nothinking"
                            | "nonthinking"
                            | "reasoning"
                            | "1m"
                            | "200k"
                            | "256k"
                            | "512k"
                    ))
            {
                value = head.to_owned();
            }
        }

        // 4. Convert Vertex AI `@20250929` or `@default` into hyphenated or bare form.
        if let Some((head, tail)) = value.rsplit_once('@') {
            let head = head.trim();
            let tail = tail.trim();
            if !head.is_empty() && !tail.is_empty() {
                if tail == "default" || tail == "latest" {
                    value = head.to_owned();
                } else {
                    value = format!("{head}-{tail}");
                }
            }
        }

        if value == before {
            break;
        }
    }
    value
}

fn is_reasoning_effort_token(token: &str) -> bool {
    matches!(
        token,
        "none" | "auto" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
    )
}

fn strip_one_non_tier_suffix(model: &str) -> Option<String> {
    // 1. Check trailing `-YYYY-MM-DD` (3 segments).
    let parts: Vec<&str> = model.split('-').collect();
    if parts.len() >= 4 {
        let n = parts.len();
        if is_year_4(parts[n - 3]) && is_month_2(parts[n - 2]) && is_day_2(parts[n - 1]) {
            return Some(parts[..n - 3].join("-"));
        }
    }
    // 2. Check trailing `-preview-MM-DD` or `-exp-MM-DD` (2 trailing 2-digit date segments after channel marker).
    if parts.len() >= 4 {
        let n = parts.len();
        if matches!(parts[n - 3], "preview" | "exp" | "experimental" | "beta")
            && is_month_2(parts[n - 2])
            && is_day_2(parts[n - 1])
        {
            return Some(parts[..n - 2].join("-"));
        }
    }
    // 3. Check 2-segment thinking/reasoning/effort suffixes (e.g. `-no-thinking`, `-thinking-high`, `-thinking-16k`).
    if parts.len() >= 3 {
        let n = parts.len();
        let p2 = parts[n - 2];
        let p1 = parts[n - 1];
        let is_two_seg_suffix = (matches!(p2, "no" | "non") && p1 == "thinking")
            || (matches!(p2, "thinking" | "reasoning" | "effort") && is_reasoning_effort_token(p1))
            || (matches!(p2, "thinking" | "reasoning") && is_k_budget_token(p1));
        if is_two_seg_suffix {
            let head = parts[..n - 2].join("-");
            if !head.is_empty() {
                return Some(head);
            }
        }
    }
    // 4. Check single trailing hyphen segment.
    let (head, tail) = model.rsplit_once('-')?;
    if head.is_empty() {
        return None;
    }
    if is_safe_single_version_token(tail) || can_strip_effort_max_or_ultra(head, tail) {
        return Some(head.to_owned());
    }
    None
}

fn is_k_budget_token(token: &str) -> bool {
    token
        .strip_suffix('k')
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit()))
}

fn can_strip_effort_max_or_ultra(head: &str, tail: &str) -> bool {
    if !matches!(tail, "max" | "ultra") {
        return false;
    }
    let model_head = head.rsplit('/').next().unwrap_or(head);
    let mut segment_count = 0usize;
    let mut has_known_subfamily = false;
    for seg in model_head.split('-') {
        if seg.is_empty() {
            continue;
        }
        segment_count += 1;
        if matches!(
            seg,
            "opus"
                | "sonnet"
                | "haiku"
                | "fable"
                | "pro"
                | "flash"
                | "lite"
                | "mini"
                | "nano"
                | "hydrogen"
                | "neon"
                | "carbon"
                | "argon"
                | "sol"
                | "terra"
                | "luna"
                | "astra"
                | "codex"
                | "chat"
                | "reasoner"
        ) {
            has_known_subfamily = true;
        }
    }
    segment_count >= 2 && has_known_subfamily
}

fn is_year_4(token: &str) -> bool {
    token.len() == 4
        && token
            .parse::<u16>()
            .is_ok_and(|year| (2020..=2099).contains(&year))
}

fn is_month_2(token: &str) -> bool {
    token.len() == 2
        && token
            .parse::<u8>()
            .is_ok_and(|month| (1..=12).contains(&month))
}

fn is_day_2(token: &str) -> bool {
    token.len() == 2 && token.parse::<u8>().is_ok_and(|day| (1..=31).contains(&day))
}

fn is_safe_single_version_token(token: &str) -> bool {
    if matches!(
        token,
        "latest"
            | "preview"
            | "exp"
            | "experimental"
            | "beta"
            | "alpha"
            | "rc"
            | "snapshot"
            | "stable"
            | "default"
            | "legacy"
            | "customtools"
            | "xhigh"
            | "high"
            | "medium"
            | "low"
            | "minimal"
            | "none"
            | "auto"
            | "thinking"
            | "nothinking"
            | "nonthinking"
            | "reasoning"
            | "1m"
            | "200k"
            | "256k"
            | "512k"
    ) {
        return true;
    }
    // `v1`, `v2` revision markers (only strip when model already has base family, e.g. `claude-3-5-sonnet-v2`,
    // NOT when `head` is a bare vendor name like `deepseek` in `deepseek-v3`!).
    if let Some(rest) = token.strip_prefix('v')
        && !rest.is_empty()
        && rest.len() <= 2
        && rest.chars().all(|ch| ch.is_ascii_digit())
    {
        return false; // Handled carefully below only if needed; keep `deepseek-v3` intact!
    }
    // 8-digit `YYYYMMDD` snapshot date, e.g. `20250929`.
    if token.len() == 8
        && is_year_4(&token[0..4])
        && is_month_2(&token[4..6])
        && is_day_2(&token[6..8])
    {
        return true;
    }
    // 4-digit `MMDD` / `YYMM` snapshot code, e.g. `0324`, `2507`, `0528`.
    if token.len() == 4 && token.chars().all(|ch| ch.is_ascii_digit()) {
        let first_two = token[0..2].parse::<u8>().unwrap_or(0);
        let last_two = token[2..4].parse::<u8>().unwrap_or(0);
        if ((1..=12).contains(&first_two) && (1..=31).contains(&last_two))
            || ((23..=35).contains(&first_two) && (1..=12).contains(&last_two))
        {
            return true;
        }
    }
    // 3-digit zero-padded build revision, e.g. `001`, `002`.
    if token.len() == 3 && token.starts_with('0') && token.chars().all(|ch| ch.is_ascii_digit()) {
        return true;
    }
    false
}

/// Select effective per-million-micros rates given base rates, sorted tiers, and total input tokens.
pub fn effective_rates_for_input(
    base_input: Option<i64>,
    base_output: Option<i64>,
    base_cache_read: Option<i64>,
    base_cache_creation: Option<i64>,
    tiers: &[ModelPriceTier],
    total_input_tokens: i64,
) -> (Option<i64>, Option<i64>, Option<i64>, Option<i64>) {
    let Some(active_tier) = tiers
        .iter()
        .rfind(|tier| tier.above_input_tokens > 0 && total_input_tokens > tier.above_input_tokens)
    else {
        return (
            base_input,
            base_output,
            base_cache_read,
            base_cache_creation,
        );
    };
    let next_input = active_tier.input_per_million_micros.or(base_input);
    let next_output = active_tier.output_per_million_micros.or(base_output);
    let next_cache_read = active_tier
        .cache_read_per_million_micros
        .or_else(|| {
            scale_rate(
                base_cache_read,
                base_input,
                active_tier.input_per_million_micros,
            )
        })
        .or(base_cache_read);
    let next_cache_creation = active_tier
        .cache_creation_per_million_micros
        .or_else(|| {
            scale_rate(
                base_cache_creation,
                base_input,
                active_tier.input_per_million_micros,
            )
        })
        .or(base_cache_creation);
    (
        next_input,
        next_output,
        next_cache_read,
        next_cache_creation,
    )
}

fn scale_rate(
    base_sub_rate: Option<i64>,
    base_input: Option<i64>,
    tier_input: Option<i64>,
) -> Option<i64> {
    let sub = base_sub_rate?;
    if sub == 0 {
        return Some(0);
    }
    let base_in = base_input.filter(|value| *value > 0)?;
    let tier_in = tier_input.filter(|value| *value >= 0)?;
    let scaled =
        ((sub as i128).saturating_mul(tier_in as i128) + (base_in as i128 / 2)) / (base_in as i128);
    Some(scaled.min(i64::MAX as i128) as i64)
}

/// Compute `tokens * rate` numerator for a single request. Returns `None` if any
/// non-zero token component lacks a configured rate.
impl ResolvedRateEntry {
    pub(crate) fn compute_numerator(
        &self,
        uncached_input: i64,
        cache_read: i64,
        cache_creation: i64,
        output: i64,
        processed_input: Option<i64>,
    ) -> Option<i128> {
        let total_input = processed_input
            .map(|value| value.max(0))
            .unwrap_or_else(|| {
                uncached_input
                    .saturating_add(cache_read)
                    .saturating_add(cache_creation)
            });
        let (input_rate, output_rate, cache_read_rate, cache_creation_rate) =
            effective_rates_for_input(
                self.input_per_million_micros,
                self.output_per_million_micros,
                self.cache_read_per_million_micros,
                self.cache_creation_per_million_micros,
                &self.tiers,
                total_input,
            );
        let components = [
            (uncached_input, input_rate),
            (cache_read, cache_read_rate),
            (cache_creation, cache_creation_rate),
            (output, output_rate),
        ];
        let mut request_numerator = 0_i128;
        for (tokens, rate) in components {
            if tokens == 0 {
                continue;
            }
            let rate = rate?;
            request_numerator =
                request_numerator.saturating_add((tokens as i128).saturating_mul(rate as i128));
        }
        Some(request_numerator)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedRateEntry {
    pub endpoint_id: Option<String>,
    pub effective_from: f64,
    pub effective_to: Option<f64>,
    pub input_per_million_micros: Option<i64>,
    pub output_per_million_micros: Option<i64>,
    pub cache_read_per_million_micros: Option<i64>,
    pub cache_creation_per_million_micros: Option<i64>,
    pub tiers: Vec<ModelPriceTier>,
}

/// Fast indexed resolver shared by rollup rebuilds and analytics queries.
/// Priority order:
/// 1. Manual endpoint-scoped exact rule (`endpoint_id == Some(ep)`) matching `effective_model` (then `upstream_model`)
/// 2. Manual endpoint-scoped wildcard rule (`prefix-*`) matching longest prefix first
/// 3. Manual global exact rule (`endpoint_id == None`) matching `effective_model` (then `upstream_model`)
/// 4. Manual global wildcard rule (`prefix-*`) matching longest prefix first
/// 5. Official synced/builtin catalog matching `upstream_model` first (actual billed upstream model),
///    falling back to `effective_model` when `upstream_model` is absent or an opaque alias.
#[derive(Default)]
pub(crate) struct PriceBook {
    manual_scoped: HashMap<String, Vec<ResolvedRateEntry>>,
    manual_scoped_wildcards: HashMap<String, Vec<(String, ResolvedRateEntry)>>,
    manual_global: HashMap<String, Vec<ResolvedRateEntry>>,
    manual_global_wildcards: Vec<(String, ResolvedRateEntry)>,
    official: HashMap<String, ResolvedRateEntry>,
}

impl PriceBook {
    pub(crate) fn insert_manual(&mut self, model_key: &str, entry: ResolvedRateEntry) {
        let trimmed = model_key.trim();
        if let Some(raw_prefix) = trimmed.strip_suffix('*') {
            if raw_prefix.trim().is_empty() {
                return;
            }
            for prefix in index_wildcard_prefixes(raw_prefix) {
                if let Some(endpoint) = entry.endpoint_id.as_deref().filter(|id| !id.is_empty()) {
                    self.manual_scoped_wildcards
                        .entry(endpoint.to_owned())
                        .or_default()
                        .push((prefix, entry.clone()));
                } else {
                    self.manual_global_wildcards.push((prefix, entry.clone()));
                }
            }
            return;
        }
        for alias in index_aliases(trimmed) {
            if let Some(endpoint) = entry.endpoint_id.as_deref().filter(|id| !id.is_empty()) {
                let key = format!("{endpoint}\u{1f}{alias}");
                self.manual_scoped
                    .entry(key)
                    .or_default()
                    .push(entry.clone());
            } else {
                self.manual_global
                    .entry(alias)
                    .or_default()
                    .push(entry.clone());
            }
        }
    }

    pub(crate) fn insert_official(&mut self, model_key: &str, entry: ResolvedRateEntry) {
        for alias in index_aliases(model_key) {
            self.official.entry(alias).or_insert_with(|| entry.clone());
        }
    }

    pub(crate) fn resolve(
        &self,
        endpoint_id: Option<&str>,
        effective_model: Option<&str>,
        upstream_model: Option<&str>,
        timestamp: f64,
    ) -> Option<&ResolvedRateEntry> {
        let manual_models = [effective_model, upstream_model];
        if let Some(endpoint) = endpoint_id.filter(|id| !id.is_empty()) {
            for candidate_model in manual_models.into_iter().flatten() {
                for key in candidate_lookup_keys(candidate_model) {
                    let scoped_key = format!("{endpoint}\u{1f}{key}");
                    if let Some(entries) = self.manual_scoped.get(&scoped_key)
                        && let Some(hit) = entries
                            .iter()
                            .find(|entry| matches_effective_window(entry, timestamp))
                    {
                        return Some(hit);
                    }
                }
            }
            if let Some(wildcards) = self.manual_scoped_wildcards.get(endpoint)
                && let Some(hit) = resolve_wildcard(wildcards, manual_models, timestamp)
            {
                return Some(hit);
            }
        }
        for candidate_model in manual_models.into_iter().flatten() {
            for key in candidate_lookup_keys(candidate_model) {
                if let Some(entries) = self.manual_global.get(&key)
                    && let Some(hit) = entries
                        .iter()
                        .find(|entry| matches_effective_window(entry, timestamp))
                {
                    return Some(hit);
                }
            }
        }
        if let Some(hit) = resolve_wildcard(&self.manual_global_wildcards, manual_models, timestamp)
        {
            return Some(hit);
        }
        // Official baseline catalog: prefer actual upstream billed model when known,
        // falling back to effective_model when upstream_model is an opaque relay alias.
        let official_models = [upstream_model, effective_model];
        for candidate_model in official_models.into_iter().flatten() {
            for key in candidate_lookup_keys(candidate_model) {
                if let Some(hit) = self.official.get(&key) {
                    return Some(hit);
                }
            }
        }
        None
    }
}

fn resolve_wildcard<'a>(
    wildcards: &'a [(String, ResolvedRateEntry)],
    models: [Option<&str>; 2],
    timestamp: f64,
) -> Option<&'a ResolvedRateEntry> {
    if wildcards.is_empty() {
        return None;
    }
    for candidate_model in models.into_iter().flatten() {
        let keys = candidate_lookup_keys(candidate_model);
        let mut best: Option<((usize, usize), &'a ResolvedRateEntry)> = None;
        for (prefix, entry) in wildcards {
            if !matches_effective_window(entry, timestamp) {
                continue;
            }
            if keys.iter().any(|key| key.starts_with(prefix.as_str())) {
                let leaf_len = prefix.rsplit('/').next().unwrap_or(prefix).len();
                let score = (leaf_len, prefix.len());
                if best.is_none_or(|(best_score, _)| score > best_score) {
                    best = Some((score, entry));
                }
            }
        }
        if let Some((_, entry)) = best {
            return Some(entry);
        }
    }
    None
}

fn matches_effective_window(entry: &ResolvedRateEntry, timestamp: f64) -> bool {
    timestamp >= entry.effective_from
        && entry
            .effective_to
            .is_none_or(|effective_to| timestamp < effective_to)
}

fn index_aliases(model_key: &str) -> Vec<String> {
    let mut aliases = Vec::with_capacity(4);
    let lowered = model_key.trim().to_ascii_lowercase();
    if lowered.is_empty() {
        return aliases;
    }
    aliases.push(lowered.clone());
    let digit_norm = canonical_digit_separators(&lowered);
    if digit_norm != lowered {
        aliases.push(digit_norm);
    }
    if let Some((_prefix, bare)) = lowered.rsplit_once('/')
        && !bare.is_empty()
    {
        if !aliases.iter().any(|existing| existing == bare) {
            aliases.push(bare.to_owned());
        }
        let bare_norm = canonical_digit_separators(bare);
        if !aliases.iter().any(|existing| existing == &bare_norm) {
            aliases.push(bare_norm);
        }
    }
    aliases
}

/// Preserve trailing hyphens/slashes on wildcard prefixes while normalizing case and digit dots.
fn index_wildcard_prefixes(raw_prefix: &str) -> Vec<String> {
    let lowered = raw_prefix.trim_start().to_ascii_lowercase();
    if lowered.trim().is_empty() {
        return Vec::new();
    }
    let mut prefixes = Vec::with_capacity(4);
    prefixes.push(lowered.clone());
    let digit_norm = canonical_digit_separators(&lowered);
    if digit_norm != lowered {
        prefixes.push(digit_norm);
    }
    if let Some((_vendor, bare)) = lowered.rsplit_once('/')
        && !bare.trim().is_empty()
    {
        if !prefixes.iter().any(|existing| existing == bare) {
            prefixes.push(bare.to_owned());
        }
        let bare_norm = canonical_digit_separators(bare);
        if !prefixes.iter().any(|existing| existing == &bare_norm) {
            prefixes.push(bare_norm);
        }
    }
    prefixes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_keys_strip_prefixes_and_snapshots_without_crossing_model_tiers() {
        let gemini = candidate_lookup_keys("models/gemini-2.5-pro-preview-06-05");
        assert!(gemini.contains(&"gemini-2.5-pro-preview-06-05".to_string()));
        assert!(gemini.contains(&"gemini-2.5-pro-preview".to_string()));
        assert!(gemini.contains(&"gemini-2.5-pro".to_string()));

        let claude = candidate_lookup_keys("openrouter/anthropic/claude-sonnet-4.6-20260514");
        assert!(claude.contains(&"claude-sonnet-4-6".to_string()));

        let openai = candidate_lookup_keys("openai/gpt-5.4-2026-03-05");
        assert!(openai.contains(&"gpt-5.4".to_string()));

        // Must NEVER strip tier words like `mini`, `nano`, `lite`, or `flash`!
        let mini = candidate_lookup_keys("openai/gpt-4o-mini");
        assert!(mini.contains(&"gpt-4o-mini".to_string()));
        assert!(!mini.contains(&"gpt-4o".to_string()));

        let flash_lite = candidate_lookup_keys("models/gemini-2.5-flash-lite-preview-06-17");
        assert!(flash_lite.contains(&"gemini-2.5-flash-lite".to_string()));
        assert!(!flash_lite.contains(&"gemini-2.5-flash".to_string()));

        // Must NEVER strip single major/minor generation numbers!
        let next_gen = candidate_lookup_keys("claude-sonnet-4-9");
        assert!(!next_gen.contains(&"claude-sonnet-4".to_string()));

        // Must preserve `deepseek-v3` intact!
        let deepseek = candidate_lookup_keys("deepseek/deepseek-v3-0324");
        assert!(deepseek.contains(&"deepseek-v3".to_string()));
        assert!(!deepseek.contains(&"deepseek".to_string()));
    }

    #[test]
    fn long_context_tier_switches_rates_above_threshold() {
        let tiers = vec![ModelPriceTier {
            above_input_tokens: 200_000,
            input_per_million_micros: Some(6_000_000),
            output_per_million_micros: Some(22_500_000),
            cache_read_per_million_micros: Some(600_000),
            cache_creation_per_million_micros: Some(7_500_000),
        }];
        let entry = ResolvedRateEntry {
            endpoint_id: None,
            effective_from: 0.0,
            effective_to: None,
            input_per_million_micros: Some(3_000_000),
            output_per_million_micros: Some(15_000_000),
            cache_read_per_million_micros: Some(300_000),
            cache_creation_per_million_micros: Some(3_750_000),
            tiers,
        };
        // Exactly 200,000 stays on base rate.
        let standard = entry
            .compute_numerator(200_000, 0, 0, 10_000, Some(200_000))
            .unwrap();
        assert_eq!(standard, 200_000 * 3_000_000 + 10_000 * 15_000_000);

        // 200,001 switches to long-context tier rate.
        let elevated = entry
            .compute_numerator(100_001, 100_000, 0, 10_000, Some(200_001))
            .unwrap();
        assert_eq!(
            elevated,
            100_001 * 6_000_000 + 100_000 * 600_000 + 10_000 * 22_500_000
        );
    }

    #[test]
    fn candidate_keys_strip_effort_thinking_and_context_without_corrupting_tiers() {
        for (input, expected_base) in [
            ("gemini-4-hydrogen-xhigh", "gemini-4-hydrogen"),
            ("models/gemini-4-hydrogen-xhigh", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-high", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-medium", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-low", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-minimal", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-none", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-auto", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-thinking", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-nothinking", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-nonthinking", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-reasoning", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-no-thinking", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-non-thinking", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-thinking-high", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-effort-xhigh", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-thinking-16k", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-1m", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen(xhigh)[1m]", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen (XHIGH) [200k]", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen:xhigh", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen:thinking[1m]", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen@20261112:thinking", "gemini-4-hydrogen"),
            ("gemini-4-hydrogen-max", "gemini-4-hydrogen"),
            ("claude-opus-4-6-max", "claude-opus-4-6"),
            ("gpt-5.6-sol-ultra", "gpt-5.6-sol"),
        ] {
            let keys = candidate_lookup_keys(input);
            assert!(
                keys.contains(&expected_base.to_string()),
                "expected `{input}` candidates {keys:?} to contain `{expected_base}`"
            );
        }

        // Protected `-max` / `-ultra` / `:free` / tier suffixes must NOT strip!
        for (input, forbidden) in [
            ("qwen-max", "qwen"),
            ("qwen3-max", "qwen3"),
            ("qwen-2.5-max", "qwen-2.5"),
            ("gemini-1.0-ultra", "gemini-1.0"),
            ("gemini-ultra", "gemini"),
            ("nova-ultra", "nova"),
            ("sonar-ultra", "sonar"),
            ("gemini-4-argon:free", "gemini-4-argon"),
            ("gemini-4-argon:free[1m]", "gemini-4-argon"),
            ("gpt-3.5-turbo-16k", "gpt-3.5-turbo"),
            ("gemini-4-hydrogen", "gemini-4"),
            ("gemini-4-neon", "gemini-4"),
            ("gpt-6-astra", "gpt-6"),
        ] {
            let keys = candidate_lookup_keys(input);
            assert!(
                !keys.contains(&forbidden.to_string()),
                "candidates for `{input}` unexpectedly stripped to `{forbidden}`: {keys:?}"
            );
        }
    }

    #[test]
    fn manual_wildcard_rules_respect_precedence_longest_prefix_and_time_window() {
        let rate = |input_micros: i64, from: f64, to: Option<f64>| ResolvedRateEntry {
            endpoint_id: None,
            effective_from: from,
            effective_to: to,
            input_per_million_micros: Some(input_micros),
            output_per_million_micros: Some(input_micros * 2),
            cache_read_per_million_micros: Some(0),
            cache_creation_per_million_micros: Some(0),
            tiers: Vec::new(),
        };

        let mut book = PriceBook::default();
        book.insert_official(
            "gemini-4-hydrogen",
            rate(1_500_000, f64::NEG_INFINITY, None),
        );
        book.insert_manual("my-relay-*", rate(5_000_000, 0.0, None));
        book.insert_manual("my-relay-gemini-*", rate(3_000_000, 0.0, Some(200.0)));
        book.insert_manual("my-relay-gemini-*", rate(3_500_000, 200.0, None));
        book.insert_manual("anthropic/claude-*", rate(9_000_000, 0.0, None));
        book.insert_manual("claude-sonnet-*", rate(4_000_000, 0.0, None));

        let mut ep_short = rate(2_000_000, 0.0, None);
        ep_short.endpoint_id = Some("ep-scoped".into());
        book.insert_manual("my-relay-gemini-*", ep_short);

        let mut ep_exact = rate(1_000_000, 0.0, None);
        ep_exact.endpoint_id = Some("ep-scoped".into());
        book.insert_manual("my-relay-gemini-exact", ep_exact);

        // 1. Exact scoped rule wins over scoped wildcard.
        assert_eq!(
            book.resolve(
                Some("ep-scoped"),
                Some("my-relay-gemini-exact-xhigh"),
                None,
                100.0,
            )
            .unwrap()
            .input_per_million_micros,
            Some(1_000_000)
        );
        // 2. Scoped wildcard wins over global wildcard.
        assert_eq!(
            book.resolve(
                Some("ep-scoped"),
                Some("my-relay-gemini-exp-xhigh"),
                None,
                100.0,
            )
            .unwrap()
            .input_per_million_micros,
            Some(2_000_000)
        );
        // 3. Longest global wildcard wins over shorter wildcard, respecting time windows.
        assert_eq!(
            book.resolve(
                Some("ep-other"),
                Some("my-relay-gemini-exp-xhigh"),
                None,
                100.0,
            )
            .unwrap()
            .input_per_million_micros,
            Some(3_000_000)
        );
        assert_eq!(
            book.resolve(
                Some("ep-other"),
                Some("my-relay-gemini-exp-xhigh"),
                None,
                250.0,
            )
            .unwrap()
            .input_per_million_micros,
            Some(3_500_000)
        );
        // 4. More specific leaf prefix beats shorter leaf prefix with vendor namespace.
        assert_eq!(
            book.resolve(
                Some("ep-other"),
                Some("anthropic/claude-sonnet-4-6-xhigh"),
                None,
                100.0,
            )
            .unwrap()
            .input_per_million_micros,
            Some(4_000_000)
        );
        // 5. Official catalog still resolves when no manual rule matches.
        assert_eq!(
            book.resolve(
                Some("ep-other"),
                Some("gemini-4-hydrogen-xhigh"),
                None,
                100.0
            )
            .unwrap()
            .input_per_million_micros,
            Some(1_500_000)
        );
    }

    #[test]
    fn builtin_snapshot_parses_and_resolves_prefixed_models() {
        let (version, models, digest) = builtin_official_snapshot();
        assert_eq!(*version, 2);
        assert!(!models.is_empty());
        assert_eq!(digest.len(), 64);

        let mut book = PriceBook::default();
        for item in models {
            book.insert_official(
                &item.model_key,
                ResolvedRateEntry {
                    endpoint_id: None,
                    effective_from: 0.0,
                    effective_to: None,
                    input_per_million_micros: item.input_per_million_micros,
                    output_per_million_micros: item.output_per_million_micros,
                    cache_read_per_million_micros: item.cache_read_per_million_micros,
                    cache_creation_per_million_micros: item.cache_creation_per_million_micros,
                    tiers: item.tiers.clone(),
                },
            );
        }
        let hit = book
            .resolve(
                Some("ep-1"),
                Some("models/gemini-2.5-pro-preview-05-06"),
                Some("opaque-upstream"),
                100.0,
            )
            .expect("resolve prefixed gemini model");
        assert_eq!(hit.input_per_million_micros, Some(1_250_000));
        assert_eq!(hit.tiers.len(), 1);

        for (model, expected_input) in [
            ("gemini-4-hydrogen-xhigh", 1_500_000),
            ("models/gemini-4-hydrogen-xhigh", 1_500_000),
            ("gemini-4-hydrogen(xhigh)[1m]", 1_500_000),
            ("claude-opus-4-6-thinking-high", 5_000_000),
            ("gpt-5.4-xhigh", 2_500_000),
            ("gpt-6.1-sol-xhigh", 2_000_000),
            ("gpt-5.1-codex-max", 1_250_000),
        ] {
            let resolved = book
                .resolve(Some("ep-1"), Some(model), None, 100.0)
                .unwrap_or_else(|| panic!("expected builtin snapshot to resolve `{model}`"));
            assert_eq!(
                resolved.input_per_million_micros,
                Some(expected_input),
                "unexpected input rate for `{model}`"
            );
        }
    }
}
