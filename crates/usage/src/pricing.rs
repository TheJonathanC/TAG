use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::event::UsageEvent;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRates {
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cache_read_per_million: f64,
    pub cache_write_per_million: f64,
}

/// Estimated list prices (USD per million tokens). Keys are matched as substrings of the
/// normalized model id (lowercase, `.`/`_`/spaces turned into `-`); a key may require
/// several parts joined by `+` (e.g. `gemini+flash`). The longest matching key wins, so
/// specific versions override family-level fallbacks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingConfig {
    pub models: HashMap<String, ModelRates>,
}

fn rates(input: f64, output: f64, cache_read: f64, cache_write: f64) -> ModelRates {
    ModelRates {
        input_per_million: input,
        output_per_million: output,
        cache_read_per_million: cache_read,
        cache_write_per_million: cache_write,
    }
}

impl Default for PricingConfig {
    fn default() -> Self {
        let mut models = HashMap::new();

        // Anthropic (cache write = 5-minute cache write rate)
        models.insert("opus".into(), rates(5.0, 25.0, 0.50, 6.25));
        models.insert("opus-4-5".into(), rates(5.0, 25.0, 0.50, 6.25));
        models.insert("opus-4".into(), rates(15.0, 75.0, 1.50, 18.75));
        models.insert("opus-4-1".into(), rates(15.0, 75.0, 1.50, 18.75));
        models.insert("3-opus".into(), rates(15.0, 75.0, 1.50, 18.75));
        models.insert("sonnet".into(), rates(3.0, 15.0, 0.30, 3.75));
        models.insert("haiku".into(), rates(1.0, 5.0, 0.10, 1.25));
        models.insert("3-5-haiku".into(), rates(0.80, 4.0, 0.08, 1.0));
        models.insert("3-haiku".into(), rates(0.25, 1.25, 0.03, 0.30));

        // OpenAI / Codex (cached input is billed at the cache read rate)
        models.insert("gpt-5".into(), rates(1.25, 10.0, 0.125, 0.0));
        models.insert("gpt-5-mini".into(), rates(0.25, 2.0, 0.025, 0.0));
        models.insert("gpt-4o".into(), rates(2.50, 10.0, 1.25, 0.0));
        models.insert("gpt-4o-mini".into(), rates(0.15, 0.60, 0.075, 0.0));
        models.insert("codex-mini".into(), rates(1.50, 6.0, 0.375, 0.0));

        // Google Gemini
        models.insert("gemini".into(), rates(1.25, 10.0, 0.31, 0.0));
        models.insert("gemini+flash".into(), rates(0.30, 2.50, 0.075, 0.0));

        Self { models }
    }
}

pub fn normalize_model(model: &str) -> String {
    model
        .to_lowercase()
        .chars()
        .map(|c| if matches!(c, '.' | '_' | ' ') { '-' } else { c })
        .collect()
}

impl PricingConfig {
    pub fn rates_for(&self, model: &str) -> Option<&ModelRates> {
        let model_key = normalize_model(model);
        self.models
            .iter()
            .filter(|(k, _)| k.split('+').all(|part| model_key.contains(part)))
            .max_by_key(|(k, _)| k.len())
            .map(|(_, v)| v)
    }

    pub fn estimate_cost(&self, event: &UsageEvent) -> f64 {
        let per_m = |tokens: u64, rate: f64| (tokens as f64 / 1_000_000.0) * rate;
        // Conservative fallback for unknown models
        let fallback = rates(2.0, 8.0, 0.20, 2.50);
        let r = self.rates_for(&event.model).unwrap_or(&fallback);
        per_m(event.input_tokens, r.input_per_million)
            + per_m(event.output_tokens, r.output_per_million)
            + per_m(event.cache_read, r.cache_read_per_million)
            + per_m(event.cache_write, r.cache_write_per_million)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate_in(p: &PricingConfig, model: &str) -> f64 {
        p.rates_for(model)
            .map(|r| r.input_per_million)
            .unwrap_or(-1.0)
    }

    #[test]
    fn test_current_model_ids_match_families() {
        let p = PricingConfig::default();
        assert_eq!(rate_in(&p, "claude-sonnet-4-5-20250929"), 3.0);
        assert_eq!(rate_in(&p, "claude-opus-4-1-20250805"), 15.0);
        assert_eq!(rate_in(&p, "claude-opus-4-5-20251101"), 5.0);
        assert_eq!(rate_in(&p, "claude-3-5-haiku-20241022"), 0.80);
        assert_eq!(rate_in(&p, "claude-haiku-4-5"), 1.0);
        assert_eq!(rate_in(&p, "Gemini 3.8 Flash"), 0.30);
        assert_eq!(rate_in(&p, "gpt-5-codex"), 1.25);
        assert_eq!(rate_in(&p, "gpt-4o-mini"), 0.15);
    }

    #[test]
    fn test_cache_tokens_are_priced() {
        let p = PricingConfig::default();
        let ev = UsageEvent {
            agent: "claude".into(),
            session_id: "s".into(),
            step_id: None,
            project: None,
            model: "claude-sonnet-4-5".into(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read: 1_000_000,
            cache_write: 0,
            timestamp: chrono::Utc::now(),
        };
        assert!((p.estimate_cost(&ev) - 0.30).abs() < 1e-9);
    }
}
