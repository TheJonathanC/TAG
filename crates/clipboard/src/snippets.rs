use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SnippetBank {
    pub snippets: Vec<Snippet>,
}

impl SnippetBank {
    pub fn new() -> Self {
        let mut bank = Self {
            snippets: Vec::new(),
        };
        bank.add_defaults();
        bank
    }

    pub fn load_from_file_or_default(path: &std::path::Path) -> Self {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(bank) = toml::from_str::<SnippetBank>(&content) {
                    if !bank.snippets.is_empty() {
                        return bank;
                    }
                }
            }
        } else {
            // Write default snippets.toml so users have an editable template ready
            let bank = Self::new();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(toml_str) = toml::to_string_pretty(&bank) {
                let _ = std::fs::write(path, toml_str);
            }
            return bank;
        }
        Self::new()
    }

    pub fn add_defaults(&mut self) {
        self.snippets.push(Snippet {
            name: "git-commit-wip".into(),
            description: "Quick work-in-progress commit".into(),
            tags: vec!["git".into()],
            template: "git add -A && git commit -m 'wip: {{message}}'".into(),
        });

        self.snippets.push(Snippet {
            name: "cargo-test-watch".into(),
            description: "Run cargo test with filter".into(),
            tags: vec!["rust".into(), "test".into()],
            template: "cargo test {{test_name}} -- --nocapture".into(),
        });

        self.snippets.push(Snippet {
            name: "agent-review-diff".into(),
            description: "Ask agent to review staged changes".into(),
            tags: vec!["agent".into()],
            template:
                "Please review the current diff against main branch and check for edge cases."
                    .into(),
        });
    }

    pub fn extract_placeholders(template: &str) -> Vec<String> {
        let mut vars = Vec::new();
        let mut rest = template;

        while let Some(start) = rest.find("{{") {
            let after_start = &rest[start + 2..];
            if let Some(end) = after_start.find("}}") {
                let var_name = after_start[..end].trim().to_string();
                if !vars.contains(&var_name) {
                    vars.push(var_name);
                }
                rest = &after_start[end + 2..];
            } else {
                break;
            }
        }

        vars
    }

    pub fn render(template: &str, values: &HashMap<String, String>) -> String {
        let mut rendered = template.to_string();
        for (k, v) in values {
            let placeholder = format!("{{{{{}}}}}", k);
            rendered = rendered.replace(&placeholder, v);
        }
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snippet_placeholders() {
        let vars = SnippetBank::extract_placeholders("echo {{name}} and {{age}}");
        assert_eq!(vars, vec!["name", "age"]);

        let mut values = HashMap::new();
        values.insert("name".into(), "Alice".into());
        values.insert("age".into(), "30".into());

        let out = SnippetBank::render("echo {{name}} and {{age}}", &values);
        assert_eq!(out, "echo Alice and 30");
    }

    #[test]
    fn test_load_from_file_or_default_creates_and_loads() {
        let dir = std::env::temp_dir().join(format!("tag_test_snippets_{}", std::process::id()));
        let file_path = dir.join("snippets.toml");
        if file_path.exists() {
            let _ = std::fs::remove_file(&file_path);
        }

        // First call should create the file with defaults
        let bank = SnippetBank::load_from_file_or_default(&file_path);
        assert_eq!(bank.snippets.len(), 3);
        assert!(file_path.exists());

        // Custom snippet append
        let custom_toml = r#"
[[snippets]]
name = "my-custom"
template = "echo hello"
"#;
        std::fs::write(&file_path, custom_toml).unwrap();

        let reloaded = SnippetBank::load_from_file_or_default(&file_path);
        assert_eq!(reloaded.snippets.len(), 1);
        assert_eq!(reloaded.snippets[0].name, "my-custom");
        assert_eq!(reloaded.snippets[0].template, "echo hello");
        assert!(reloaded.snippets[0].tags.is_empty());

        let _ = std::fs::remove_file(&file_path);
        let _ = std::fs::remove_dir(&dir);
    }
}
