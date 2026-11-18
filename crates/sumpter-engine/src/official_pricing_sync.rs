//! Official model pricing synchronization from open-source public feeds (`models.dev` & `litellm`).

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::Value;
use sumpter_core::config_store::ConfigDir;
use sumpter_runtime::pricing_matcher::{
    MAX_OFFICIAL_PRICE_MODELS, ModelPriceTier, OfficialModelPriceInput, builtin_official_snapshot,
    normalize_official_prices, validate_price_tiers,
};

pub const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";
pub const LITELLM_PRICES_PRIMARY_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
pub const LITELLM_PRICES_MIRROR_URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/refs/heads/main/model_prices_and_context_window.json";

const OFFICIAL_PRICING_CACHE_FILE: &str = "official_pricing.json";
const MAX_PRICING_SOURCE_BYTES: usize = 12 * 1024 * 1024;
const MAX_SANE_RATE_MICROS: i64 = 10_000_000_000; // $10,000 / 1M tokens sanity cap

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedOfficialPricing {
    pub source: String,
    pub models: Vec<OfficialModelPriceInput>,
}

pub fn load_cached_official_pricing(dir: &ConfigDir) -> Option<CachedOfficialPricing> {
    let bytes = dir.load_model_catalog(OFFICIAL_PRICING_CACHE_FILE).ok()?;
    if bytes.len() > MAX_PRICING_SOURCE_BYTES {
        return None;
    }
    let mut cached: CachedOfficialPricing = serde_json::from_slice(&bytes).ok()?;
    cached.models = normalize_official_prices(&cached.models).ok()?;
    if cached.models.is_empty() || cached.source.trim().is_empty() {
        return None;
    }
    Some(cached)
}

pub fn save_cached_official_pricing(
    dir: &ConfigDir,
    source: &str,
    models: &[OfficialModelPriceInput],
) -> Result<(), String> {
    let payload = CachedOfficialPricing {
        source: source.to_owned(),
        models: models.to_vec(),
    };
    let bytes = serde_json::to_vec(&payload).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_PRICING_SOURCE_BYTES {
        return Err("官方价格缓存体积超限".into());
    }
    dir.save_model_catalog(OFFICIAL_PRICING_CACHE_FILE, &bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub async fn fetch_official_pricing_catalog()
-> Result<(String, Vec<OfficialModelPriceInput>), String> {
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .http1_only()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;

    let (models_dev_res, litellm_res) = tokio::join!(
        fetch_json(&client, &[MODELS_DEV_API_URL]),
        fetch_json(
            &client,
            &[LITELLM_PRICES_PRIMARY_URL, LITELLM_PRICES_MIRROR_URL]
        ),
    );

    let mut errors = Vec::new();
    let models_dev_list = match models_dev_res {
        Ok((payload, _)) => match parse_models_dev_catalog(&payload) {
            Ok(list) if !list.is_empty() => Some(list),
            Ok(_) => {
                errors.push("models.dev: 未提取到官方模型价格".to_string());
                None
            }
            Err(error) => {
                errors.push(format!("models.dev: {error}"));
                None
            }
        },
        Err(error) => {
            errors.push(format!("models.dev: {error}"));
            None
        }
    };

    let litellm_list = match litellm_res {
        Ok((payload, _)) => match parse_litellm_prices(&payload) {
            Ok(list) if !list.is_empty() => Some(list),
            Ok(_) => {
                errors.push("litellm: 未提取到官方模型价格".to_string());
                None
            }
            Err(error) => {
                errors.push(format!("litellm: {error}"));
                None
            }
        },
        Err(error) => {
            errors.push(format!("litellm: {error}"));
            None
        }
    };

    match (models_dev_list, litellm_list) {
        (None, None) => Err(errors.join("；")),
        (models_dev, litellm) => {
            let source = match (&models_dev, &litellm) {
                (Some(_), Some(_)) => "models.dev+litellm",
                (Some(_), None) => "models.dev",
                (None, Some(_)) => "litellm",
                (None, None) => unreachable!(),
            };
            let merged = merge_official_catalogs(
                &builtin_official_snapshot().1,
                litellm.as_deref().unwrap_or(&[]),
                models_dev.as_deref().unwrap_or(&[]),
            )?;
            Ok((source.to_string(), merged))
        }
    }
}

async fn fetch_json(client: &reqwest::Client, urls: &[&str]) -> Result<(Value, String), String> {
    let mut errors = Vec::new();
    for url in urls {
        match client.get(*url).send().await {
            Ok(response) if response.status() == StatusCode::OK => {
                match read_bounded_bytes(response, MAX_PRICING_SOURCE_BYTES).await {
                    Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                        Ok(value) => return Ok((value, (*url).to_string())),
                        Err(error) => errors.push(format!("{url}: JSON {error}")),
                    },
                    Err(error) => errors.push(format!("{url}: {error}")),
                }
            }
            Ok(response) => errors.push(format!("{url}: HTTP {}", response.status())),
            Err(error) => errors.push(format!("{url}: {error}")),
        }
    }
    Err(errors.join("；"))
}

async fn read_bounded_bytes(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("读取响应失败: {error}"))?;
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("响应过大(>{} MiB)", max_bytes / (1024 * 1024)));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Merge baseline snapshot + secondary supplement (LiteLLM) + primary catalog (`models.dev`).
/// Higher layers win on base rates, while preserving compatible long-context tiers when a higher
/// layer omits tier metadata for the exact same model and base input rate.
pub fn merge_official_catalogs(
    builtin: &[OfficialModelPriceInput],
    supplement: &[OfficialModelPriceInput],
    primary: &[OfficialModelPriceInput],
) -> Result<Vec<OfficialModelPriceInput>, String> {
    let mut merged = BTreeMap::<String, OfficialModelPriceInput>::new();
    for layer in [builtin, supplement, primary] {
        for item in layer {
            if merged.len() >= MAX_OFFICIAL_PRICE_MODELS && !merged.contains_key(&item.model_key) {
                continue;
            }
            let key = item.model_key.trim().to_ascii_lowercase();
            if key.is_empty() {
                continue;
            }
            let mut next = item.clone();
            next.model_key = key.clone();
            if next.tiers.is_empty()
                && let Some(previous) = merged.get(&key)
                && !previous.tiers.is_empty()
                && previous.input_per_million_micros == next.input_per_million_micros
            {
                next.tiers = previous.tiers.clone();
            }
            merged.insert(key, next);
        }
    }
    normalize_official_prices(&merged.into_values().collect::<Vec<_>>())
}

/// Parse `https://models.dev/api.json`.
/// Prioritizes first-party official providers so third-party resellers/routers never override
/// official vendor list prices.
pub fn parse_models_dev_catalog(value: &Value) -> Result<Vec<OfficialModelPriceInput>, String> {
    let root = value
        .as_object()
        .ok_or_else(|| "顶层必须是对象".to_string())?;

    let mut first_party = BTreeMap::<String, OfficialModelPriceInput>::new();
    let mut fallback_party = BTreeMap::<String, OfficialModelPriceInput>::new();

    for (provider_id, provider_val) in root {
        let provider_key = provider_id.trim().to_ascii_lowercase();
        let tier = classify_models_dev_provider(&provider_key);
        if tier == ProviderPriority::Excluded {
            continue;
        }
        let Some(models_obj) = provider_val.get("models").and_then(Value::as_object) else {
            continue;
        };
        for (model_id, model_val) in models_obj {
            let Some(model_obj) = model_val.as_object() else {
                continue;
            };
            let Some(cost_obj) = model_obj.get("cost").and_then(Value::as_object) else {
                continue;
            };
            let raw_key = model_obj
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or(model_id)
                .trim();
            let bare_key = strip_vendor_prefix(raw_key);
            if bare_key.is_empty() || bare_key.len() > 256 {
                continue;
            }
            let Some(entry) = parse_models_dev_cost(&bare_key, &provider_key, cost_obj) else {
                continue;
            };
            match tier {
                ProviderPriority::FirstParty => {
                    first_party.insert(bare_key, entry);
                }
                ProviderPriority::Secondary => {
                    fallback_party.entry(bare_key).or_insert(entry);
                }
                ProviderPriority::Excluded => {}
            }
        }
    }

    for (key, entry) in fallback_party {
        if first_party.len() >= MAX_OFFICIAL_PRICE_MODELS {
            break;
        }
        first_party.entry(key).or_insert(entry);
    }
    normalize_official_prices(&first_party.into_values().collect::<Vec<_>>())
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ProviderPriority {
    FirstParty,
    Secondary,
    Excluded,
}

fn classify_models_dev_provider(provider: &str) -> ProviderPriority {
    match provider {
        "openai" | "anthropic" | "google" | "deepseek" | "xai" | "mistral" | "cohere"
        | "moonshotai" | "moonshot" | "alibaba" | "qwen" | "zai" | "zhipu" | "zhipuai"
        | "minimax" | "perplexity" => ProviderPriority::FirstParty,
        // Exclude aggregators, cloud resellers, and subscription wrappers that alter list prices.
        "openrouter"
        | "togetherai"
        | "fireworks-ai"
        | "amazon-bedrock"
        | "google-vertex"
        | "google-vertex-anthropic"
        | "azure"
        | "github-models"
        | "github-copilot"
        | "cloudflare-workers-ai"
        | "vercel"
        | "helicone"
        | "requesty"
        | "poe" => ProviderPriority::Excluded,
        _ => ProviderPriority::Secondary,
    }
}

fn parse_models_dev_cost(
    model_key: &str,
    provider: &str,
    cost: &serde_json::Map<String, Value>,
) -> Option<OfficialModelPriceInput> {
    let input = usd_per_million_to_micros(cost.get("input"))?;
    let output = usd_per_million_to_micros(cost.get("output"))?;
    if input == 0 && output == 0 {
        return None;
    }
    let cache_read =
        usd_per_million_to_micros(cost.get("cache_read").or_else(|| cost.get("cacheRead")))
            .or(Some(input));
    let default_cache_write = if provider == "anthropic" {
        // Standard 5-minute Anthropic prompt cache write rate is 1.25x base input when omitted.
        Some(((input as i128 * 125 + 50) / 100) as i64)
    } else {
        Some(0)
    };
    let cache_creation =
        usd_per_million_to_micros(cost.get("cache_write").or_else(|| cost.get("cacheWrite")))
            .or(default_cache_write);

    let mut tiers_by_threshold = BTreeMap::<i64, ModelPriceTier>::new();
    for over_200k_key in ["context_over_200k", "experimentalOver200K", "over_200k"] {
        if let Some(sub) = cost.get(over_200k_key).and_then(Value::as_object)
            && let Some(tier) = parse_models_dev_tier_object(200_000, sub, provider)
        {
            tiers_by_threshold.insert(200_000, tier);
        }
    }
    if let Some(tiers_arr) = cost.get("tiers").and_then(Value::as_array) {
        for item in tiers_arr {
            let Some(item_obj) = item.as_object() else {
                continue;
            };
            if let Some(tier_kind) = item.pointer("/tier/type").and_then(Value::as_str)
                && tier_kind != "context"
            {
                continue;
            }
            let threshold = item
                .pointer("/tier/size")
                .or_else(|| item_obj.get("above_tokens"))
                .or_else(|| item_obj.get("aboveInputTokens"))
                .or_else(|| item_obj.get("min_tokens"))
                .or_else(|| item_obj.get("context"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if threshold <= 0 {
                continue;
            }
            if let Some(tier) = parse_models_dev_tier_object(threshold, item_obj, provider) {
                tiers_by_threshold.insert(threshold, tier);
            }
        }
    }
    let tiers = validate_price_tiers(&tiers_by_threshold.into_values().collect::<Vec<_>>())
        .unwrap_or_default();

    Some(OfficialModelPriceInput {
        model_key: model_key.to_owned(),
        provider: Some(provider.to_owned()),
        input_per_million_micros: Some(input),
        output_per_million_micros: Some(output),
        cache_read_per_million_micros: cache_read,
        cache_creation_per_million_micros: cache_creation,
        tiers,
    })
}

fn parse_models_dev_tier_object(
    threshold: i64,
    obj: &serde_json::Map<String, Value>,
    provider: &str,
) -> Option<ModelPriceTier> {
    let input = usd_per_million_to_micros(obj.get("input"));
    let output = usd_per_million_to_micros(obj.get("output"));
    if input.is_none() && output.is_none() {
        return None;
    }
    let cache_read =
        usd_per_million_to_micros(obj.get("cache_read").or_else(|| obj.get("cacheRead")));
    let cache_creation =
        usd_per_million_to_micros(obj.get("cache_write").or_else(|| obj.get("cacheWrite")))
            .or_else(|| {
                if provider == "anthropic" {
                    input.map(|inp| ((inp as i128 * 125 + 50) / 100) as i64)
                } else {
                    Some(0)
                }
            });
    Some(ModelPriceTier {
        above_input_tokens: threshold,
        input_per_million_micros: input,
        output_per_million_micros: output,
        cache_read_per_million_micros: cache_read,
        cache_creation_per_million_micros: cache_creation,
    })
}

/// Parse LiteLLM `model_prices_and_context_window.json` (rates are USD per single token).
pub fn parse_litellm_prices(value: &Value) -> Result<Vec<OfficialModelPriceInput>, String> {
    let root = value
        .as_object()
        .ok_or_else(|| "顶层必须是对象".to_string())?;
    let mut entries = BTreeMap::<String, OfficialModelPriceInput>::new();

    for (raw_key, raw_val) in root {
        if raw_key == "sample_spec" {
            continue;
        }
        let Some(obj) = raw_val.as_object() else {
            continue;
        };
        let provider = obj
            .get("litellm_provider")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !is_first_party_litellm_provider(&provider) {
            continue;
        }
        if let Some(mode) = obj.get("mode").and_then(Value::as_str)
            && !matches!(mode, "chat" | "responses" | "completion")
        {
            continue;
        }
        let bare_key = strip_vendor_prefix(raw_key);
        if bare_key.is_empty() || bare_key.len() > 256 || bare_key.contains(':') {
            continue;
        }
        let Some(input) = usd_per_token_to_million_micros(obj.get("input_cost_per_token")) else {
            continue;
        };
        let Some(output) = usd_per_token_to_million_micros(obj.get("output_cost_per_token")) else {
            continue;
        };
        if input == 0 && output == 0 {
            continue;
        }
        let normalized_provider = canonical_provider_name(&provider);
        let cache_read =
            usd_per_token_to_million_micros(obj.get("cache_read_input_token_cost")).or(Some(input));
        let default_cache_creation = if normalized_provider == "anthropic" {
            Some(((input as i128 * 125 + 50) / 100) as i64)
        } else {
            Some(0)
        };
        let cache_creation =
            usd_per_token_to_million_micros(obj.get("cache_creation_input_token_cost"))
                .or(default_cache_creation);

        let mut tiers = Vec::new();
        for (threshold, suffix) in [(128_000_i64, "128k"), (200_000, "200k"), (272_000, "272k")] {
            let tier_in = usd_per_token_to_million_micros(
                obj.get(&format!("input_cost_per_token_above_{suffix}_tokens")),
            );
            let tier_out = usd_per_token_to_million_micros(
                obj.get(&format!("output_cost_per_token_above_{suffix}_tokens")),
            );
            if tier_in.is_none() && tier_out.is_none() {
                continue;
            }
            let tier_cr = usd_per_token_to_million_micros(obj.get(&format!(
                "cache_read_input_token_cost_above_{suffix}_tokens"
            )));
            let tier_cc = usd_per_token_to_million_micros(obj.get(&format!(
                "cache_creation_input_token_cost_above_{suffix}_tokens"
            )))
            .or_else(|| {
                if normalized_provider == "anthropic" {
                    tier_in.map(|inp| ((inp as i128 * 125 + 50) / 100) as i64)
                } else {
                    Some(0)
                }
            });
            tiers.push(ModelPriceTier {
                above_input_tokens: threshold,
                input_per_million_micros: tier_in,
                output_per_million_micros: tier_out,
                cache_read_per_million_micros: tier_cr,
                cache_creation_per_million_micros: tier_cc,
            });
        }
        let tiers = validate_price_tiers(&tiers).unwrap_or_default();

        // Prefer bare unprefixed entry when both `gpt-5` and `openai/gpt-5` exist in LiteLLM.
        let is_unprefixed = !raw_key.contains('/');
        if (is_unprefixed || !entries.contains_key(&bare_key))
            && (entries.len() < MAX_OFFICIAL_PRICE_MODELS || entries.contains_key(&bare_key))
        {
            entries.insert(
                bare_key.clone(),
                OfficialModelPriceInput {
                    model_key: bare_key,
                    provider: Some(normalized_provider.to_owned()),
                    input_per_million_micros: Some(input),
                    output_per_million_micros: Some(output),
                    cache_read_per_million_micros: cache_read,
                    cache_creation_per_million_micros: cache_creation,
                    tiers,
                },
            );
        }
    }

    normalize_official_prices(&entries.into_values().collect::<Vec<_>>())
}

fn is_first_party_litellm_provider(provider: &str) -> bool {
    matches!(
        provider,
        "openai"
            | "text-completion-openai"
            | "anthropic"
            | "gemini"
            | "deepseek"
            | "xai"
            | "mistral"
            | "codestral"
            | "cohere"
            | "cohere_chat"
            | "moonshot"
            | "dashscope"
            | "zai"
            | "minimax"
    )
}

fn canonical_provider_name(provider: &str) -> &'static str {
    match provider {
        "openai" | "text-completion-openai" => "openai",
        "anthropic" => "anthropic",
        "gemini" => "google",
        "deepseek" => "deepseek",
        "xai" => "xai",
        "mistral" | "codestral" => "mistral",
        "cohere" | "cohere_chat" => "cohere",
        "moonshot" => "moonshot",
        "dashscope" => "alibaba",
        "zai" => "zai",
        "minimax" => "minimax",
        _ => "other",
    }
}

fn strip_vendor_prefix(raw_key: &str) -> String {
    let lowered = raw_key.trim().to_ascii_lowercase();
    lowered
        .rsplit_once('/')
        .map(|(_prefix, bare)| bare.trim().to_owned())
        .unwrap_or(lowered)
}

fn usd_per_million_to_micros(value: Option<&Value>) -> Option<i64> {
    let number = value?.as_f64()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let micros = (number * 1_000_000.0).round();
    if !micros.is_finite() || micros > MAX_SANE_RATE_MICROS as f64 {
        return None;
    }
    Some(micros as i64)
}

fn usd_per_token_to_million_micros(value: Option<&Value>) -> Option<i64> {
    let number = value?.as_f64()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let micros = (number * 1_000_000_000_000.0).round();
    if !micros.is_finite() || micros > MAX_SANE_RATE_MICROS as f64 {
        return None;
    }
    Some(micros as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_models_dev_first_party_and_long_context_tiers_while_ignoring_resellers() {
        let payload = json!({
            "anthropic": {
                "models": {
                    "claude-sonnet-4-6": {
                        "id": "claude-sonnet-4-6",
                        "cost": {
                            "input": 3.0,
                            "output": 15.0,
                            "cache_read": 0.3,
                            "cache_write": 3.75,
                            "context_over_200k": {
                                "input": 6.0,
                                "output": 22.5,
                                "cache_read": 0.6,
                                "cache_write": 7.5
                            }
                        }
                    }
                }
            },
            "openai": {
                "models": {
                    "gpt-5.4": {
                        "id": "gpt-5.4",
                        "cost": {
                            "input": 2.5,
                            "output": 15.0,
                            "cache_read": 0.25,
                            "tiers": [
                                {
                                    "tier": { "type": "context", "size": 272000 },
                                    "input": 5.0,
                                    "output": 22.5,
                                    "cache_read": 0.5
                                }
                            ]
                        }
                    }
                }
            },
            "openrouter": {
                "models": {
                    "anthropic/claude-sonnet-4-6": {
                        "id": "anthropic/claude-sonnet-4-6",
                        "cost": {
                            "input": 99.0,
                            "output": 99.0
                        }
                    }
                }
            }
        });
        let parsed = parse_models_dev_catalog(&payload).unwrap();
        assert_eq!(parsed.len(), 2);

        let claude = parsed
            .iter()
            .find(|item| item.model_key == "claude-sonnet-4-6")
            .unwrap();
        assert_eq!(claude.input_per_million_micros, Some(3_000_000));
        assert_eq!(claude.cache_creation_per_million_micros, Some(3_750_000));
        assert_eq!(claude.tiers.len(), 1);
        assert_eq!(claude.tiers[0].above_input_tokens, 200_000);
        assert_eq!(claude.tiers[0].input_per_million_micros, Some(6_000_000));

        let gpt = parsed
            .iter()
            .find(|item| item.model_key == "gpt-5.4")
            .unwrap();
        assert_eq!(gpt.input_per_million_micros, Some(2_500_000));
        assert_eq!(gpt.cache_creation_per_million_micros, Some(0));
        assert_eq!(gpt.tiers.len(), 1);
        assert_eq!(gpt.tiers[0].above_input_tokens, 272_000);
        assert_eq!(gpt.tiers[0].input_per_million_micros, Some(5_000_000));
    }

    #[test]
    fn parses_litellm_per_token_rates_and_above_200k_fields() {
        let payload = json!({
            "sample_spec": {
                "litellm_provider": "openai",
                "input_cost_per_token": 0.01,
                "output_cost_per_token": 0.02
            },
            "gemini/gemini-2.5-pro": {
                "litellm_provider": "gemini",
                "mode": "chat",
                "input_cost_per_token": 0.00000125,
                "output_cost_per_token": 0.00001,
                "cache_read_input_token_cost": 0.000000125,
                "input_cost_per_token_above_200k_tokens": 0.0000025,
                "output_cost_per_token_above_200k_tokens": 0.000015,
                "cache_read_input_token_cost_above_200k_tokens": 0.00000025
            },
            "bedrock/anthropic.claude-sonnet": {
                "litellm_provider": "bedrock",
                "mode": "chat",
                "input_cost_per_token": 0.00009,
                "output_cost_per_token": 0.00009
            }
        });
        let parsed = parse_litellm_prices(&payload).unwrap();
        assert_eq!(parsed.len(), 1);
        let gemini = &parsed[0];
        assert_eq!(gemini.model_key, "gemini-2.5-pro");
        assert_eq!(gemini.input_per_million_micros, Some(1_250_000));
        assert_eq!(gemini.output_per_million_micros, Some(10_000_000));
        assert_eq!(gemini.cache_read_per_million_micros, Some(125_000));
        assert_eq!(gemini.tiers.len(), 1);
        assert_eq!(gemini.tiers[0].above_input_tokens, 200_000);
        assert_eq!(gemini.tiers[0].input_per_million_micros, Some(2_500_000));
    }
}
