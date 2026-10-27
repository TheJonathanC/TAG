use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::event::UsageEvent;
use crate::providers::{AgyProvider, ClaudeCodeProvider, CodexProvider};

/// Incrementally scans known agent transcript locations, re-parsing only files whose
/// size or modification time changed since the previous scan. Events are keyed so that
/// re-inserting a re-parsed file is idempotent (`INSERT OR IGNORE`).
pub struct UsageScanner {
    home: PathBuf,
    seen: HashMap<PathBuf, (SystemTime, u64)>,
}

impl UsageScanner {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            seen: HashMap::new(),
        }
    }

    /// Returns events from transcripts that are new or changed since the last call.
    pub fn scan_changed(&mut self) -> Vec<UsageEvent> {
        let mut events = Vec::new();

        // 1. agy transcripts (~/.gemini/antigravity-cli/brain/<conv>/.system_generated/logs/transcript.jsonl)
        let brain_dir = self.home.join(".gemini/antigravity-cli/brain");
        if let Ok(entries) = std::fs::read_dir(&brain_dir) {
            for entry in entries.flatten() {
                let conv_id = entry.file_name().to_string_lossy().to_string();
                let path = entry.path().join(".system_generated/logs/transcript.jsonl");
                if self.changed(&path) {
                    events.extend(AgyProvider::parse_file(&path, &conv_id));
                }
            }
        }

        // 2. Claude Code transcripts (~/.claude/projects/<proj>/*.jsonl)
        let claude_dir = self.home.join(".claude/projects");
        if let Ok(projects) = std::fs::read_dir(&claude_dir) {
            for proj in projects.flatten() {
                if let Ok(files) = std::fs::read_dir(proj.path()) {
                    for file in files.flatten() {
                        let path = file.path();
                        if is_jsonl(&path) && self.changed(&path) {
                            events.extend(ClaudeCodeProvider::parse_file(&path));
                        }
                    }
                }
            }
        }

        // 3. Codex sessions (~/.codex/sessions/**/*.jsonl)
        let mut stack = vec![self.home.join(".codex/sessions")];
        while let Some(dir) = stack.pop() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    if is_dir {
                        stack.push(path);
                    } else if is_jsonl(&path) && self.changed(&path) {
                        events.extend(CodexProvider::parse_file(&path));
                    }
                }
            }
        }

        events
    }

    fn changed(&mut self, path: &Path) -> bool {
        let Ok(meta) = std::fs::metadata(path) else {
            return false;
        };
        let stamp = (
            meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            meta.len(),
        );
        if self.seen.get(path) == Some(&stamp) {
            return false;
        }
        self.seen.insert(path.to_path_buf(), stamp);
        true
    }
}

fn is_jsonl(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_only_changed_files_are_reparsed() {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let home =
            std::env::temp_dir().join(format!("tag_test_scan_{}_{}", std::process::id(), nanos));
        let proj = home.join(".claude/projects/demo");
        std::fs::create_dir_all(&proj).unwrap();
        let file = proj.join("s1.jsonl");
        let line = r#"{"sessionId":"s1","requestId":"r1","message":{"id":"m1","model":"claude-sonnet-4-5","usage":{"input_tokens":10,"output_tokens":5}}}"#;
        std::fs::write(&file, format!("{}\n{}\n", line, line)).unwrap();

        let mut scanner = UsageScanner::new(&home);
        let first = scanner.scan_changed();
        assert_eq!(first.len(), 1, "repeated content-block lines count once");
        assert!(
            scanner.scan_changed().is_empty(),
            "unchanged files are skipped"
        );

        let line2 = r#"{"sessionId":"s1","requestId":"r2","message":{"id":"m2","model":"claude-sonnet-4-5","usage":{"input_tokens":1,"output_tokens":1}}}"#;
        std::fs::write(&file, format!("{}\n{}\n", line, line2)).unwrap();
        assert_eq!(scanner.scan_changed().len(), 2);

        let _ = std::fs::remove_dir_all(&home);
    }
}
