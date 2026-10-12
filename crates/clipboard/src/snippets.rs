use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub name: String,
    pub description: String,
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
}
