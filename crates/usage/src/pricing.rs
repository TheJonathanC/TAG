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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingConfig {
    pub models: HashMap<String, ModelRates>,
}

impl Default for PricingConfig {
    fn default() -> Self {
        let mut models = HashMap::new();

        // Claude 3.5 Sonnet
        models.insert(
            "claude-3-5-sonnet".into(),
            ModelRates {
                input_per_million: 3.0,
                output_per_million: 15.0,
                cache_read_per_million: 0.30,
                cache_write_per_million: 3.75,
            },
        );

        // GPT-4o / Codex
        models.insert(
            "gpt-4o".into(),
            ModelRates {
                input_per_million: 2.50,
                output_per_million: 10.0,
                cache_read_per_million: 1.25,
                cache_write_per_million: 2.50,
            },
        );

        // Gemini 3.8 / 1.5 Pro
        models.insert(
            "gemini-flash".into(),
            ModelRates {
                input_per_million: 0.075,
                output_per_million: 0.30,
                cache_read_per_million: 0.018,
                cache_write_per_million: 0.075,
            },
        );

        Self { models }
    }
}

impl PricingConfig {
    pub fn estimate_cost(&self, event: &UsageEvent) -> f64 {
        let model_key = event.model.to_lowercase();
        let rates = self
            .models
            .iter()
            .find(|(k, _)| model_key.contains(k.as_str()))
            .map(|(_, v)| v);

        if let Some(r) = rates {
            let in_cost = (event.input_tokens as f64 / 1_000_000.0) * r.input_per_million;
            let out_cost = (event.output_tokens as f64 / 1_000_000.0) * r.output_per_million;
            let cr_cost = (event.cache_read as f64 / 1_000_000.0) * r.cache_read_per_million;
            let cw_cost = (event.cache_write as f64 / 1_000_000.0) * r.cache_write_per_million;
            in_cost + out_cost + cr_cost + cw_cost
        } else {
            // Default conservative fallback
            (event.input_tokens as f64 / 1_000_000.0) * 2.0
                + (event.output_tokens as f64 / 1_000_000.0) * 8.0
        }
    }
}
