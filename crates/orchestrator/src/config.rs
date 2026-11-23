//! Project configuration (`<repo>/.tag/project.toml`) and resolved settings.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::{MergePolicy, Visibility};
use crate::Result;

/// Raw, all-optional project (or global `[orchestration]`) configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Project name.
    #[serde(default)]
    pub name: Option<String>,
    /// Command that runs the tests.
    #[serde(default)]
    pub test_command: Option<String>,
    /// Command that runs the linter.
    #[serde(default)]
    pub lint_command: Option<String>,
    /// Default conductor profile.
    #[serde(default)]
    pub default_conductor: Option<String>,
    /// Default worker profiles.
    #[serde(default)]
    pub default_workers: Option<Vec<String>>,
    /// Merge policy.
    #[serde(default)]
    pub merge_policy: Option<MergePolicy>,
    /// Whether plans need approval.
    #[serde(default)]
    pub require_plan_approval: Option<bool>,
    /// Max concurrent workers.
    #[serde(default)]
    pub max_workers: Option<usize>,
    /// Worker visibility.
    #[serde(default)]
    pub worker_visibility: Option<Visibility>,
    /// Per-run budget in USD (0 = unlimited).
    #[serde(default)]
    pub budget_usd: Option<f64>,
    /// Kill running workers (not just stop new spawns) when the budget is reached.
    pub hard_budget: Option<bool>,
    /// Paths whose changes always need human approval.
    #[serde(default)]
    pub protected_paths: Option<Vec<String>>,
}

impl ProjectConfig {
    /// Loads `<repo_root>/.tag/project.toml`; `Ok(None)` if absent.
    pub fn load(repo_root: &Path) -> Result<Option<ProjectConfig>> {
        let path = repo_root.join(".tag").join("project.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(toml::from_str(&text)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Fully resolved orchestration settings.
#[derive(Debug, Clone, PartialEq)]
pub struct OrchestrationSettings {
    /// Project name, if configured.
    pub name: Option<String>,
    /// Test command, if configured.
    pub test_command: Option<String>,
    /// Lint command, if configured.
    pub lint_command: Option<String>,
    /// Default conductor profile, if configured.
    pub default_conductor: Option<String>,
    /// Default worker profiles.
    pub default_workers: Vec<String>,
    /// Merge policy.
    pub merge_policy: MergePolicy,
    /// Whether plans need approval.
    pub require_plan_approval: bool,
    /// Max concurrent workers.
    pub max_workers: usize,
    /// Worker visibility.
    pub worker_visibility: Visibility,
    /// Per-run budget in USD (0 = unlimited).
    pub budget_usd: f64,
    /// Kill running workers when the budget is reached.
    pub hard_budget: bool,
    /// Protected path prefixes.
    pub protected_paths: Vec<String>,
}

impl Default for OrchestrationSettings {
    fn default() -> Self {
        Self::builtin()
    }
}

impl OrchestrationSettings {
    /// Built-in defaults: ask, plan approval on, 4 workers, card
    /// visibility, no budget, no protected paths.
    pub fn builtin() -> Self {
        OrchestrationSettings {
            name: None,
            test_command: None,
            lint_command: None,
            default_conductor: None,
            default_workers: Vec::new(),
            merge_policy: MergePolicy::Ask,
            require_plan_approval: true,
            max_workers: 4,
            worker_visibility: Visibility::Card,
            budget_usd: 0.0,
            hard_budget: false,
            protected_paths: Vec::new(),
        }
    }

    /// Resolves with precedence `overrides > project > global > builtin`.
    pub fn resolve(
        builtin: &OrchestrationSettings,
        global: &ProjectConfig,
        project: &ProjectConfig,
        overrides: &ProjectConfig,
    ) -> OrchestrationSettings {
        let layers = [overrides, project, global];
        macro_rules! pick {
            ($f:ident) => {
                layers.iter().find_map(|l| l.$f.clone())
            };
        }
        OrchestrationSettings {
            name: pick!(name).or_else(|| builtin.name.clone()),
            test_command: pick!(test_command).or_else(|| builtin.test_command.clone()),
            lint_command: pick!(lint_command).or_else(|| builtin.lint_command.clone()),
            default_conductor: pick!(default_conductor)
                .or_else(|| builtin.default_conductor.clone()),
            default_workers: pick!(default_workers)
                .unwrap_or_else(|| builtin.default_workers.clone()),
            merge_policy: pick!(merge_policy).unwrap_or(builtin.merge_policy),
            require_plan_approval: pick!(require_plan_approval)
                .unwrap_or(builtin.require_plan_approval),
            max_workers: pick!(max_workers).unwrap_or(builtin.max_workers),
            worker_visibility: pick!(worker_visibility).unwrap_or(builtin.worker_visibility),
            budget_usd: pick!(budget_usd).unwrap_or(builtin.budget_usd),
            hard_budget: pick!(hard_budget).unwrap_or(builtin.hard_budget),
            protected_paths: pick!(protected_paths)
                .unwrap_or_else(|| builtin.protected_paths.clone()),
        }
    }
}

/// Returns the entries of `paths` that start with any `protected` prefix.
pub fn touches_protected(paths: &[String], protected: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|p| {
            let p = p.trim_start_matches("./");
            protected
                .iter()
                .any(|pr| !pr.is_empty() && p.starts_with(pr.trim_start_matches("./")))
        })
        .cloned()
        .collect()
}
