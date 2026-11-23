//! Keep agent instruction files (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`) in
//! sync with the project brain via a managed marker block.

use crate::doc::{parse_doc, Doc, DocKind};
use crate::error::Result;
use std::path::{Path, PathBuf};

/// Start marker of the managed block.
pub const BLOCK_START: &str = "<!-- tag:brain:start -->";
/// End marker of the managed block.
pub const BLOCK_END: &str = "<!-- tag:brain:end -->";

/// Agent instruction files kept in sync, relative to the repo root.
pub const AGENT_FILES: [&str; 3] = ["CLAUDE.md", "AGENTS.md", "GEMINI.md"];

/// What [`sync_agent_files`] did to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOutcome {
    /// The file did not exist and was created.
    Created,
    /// An existing managed block was replaced.
    Updated,
    /// No markers were present; a block was appended.
    Appended,
    /// The block was already up to date.
    Unchanged,
    /// The file does not exist and `create` was false (or is not UTF-8).
    Skipped,
}

fn read_docs(dir: &Path) -> Vec<Doc> {
    let mut out: Vec<Doc> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&p) {
            if let Ok(d) = parse_doc(&p, &text) {
                out.push(d);
            }
        }
    }
    out.sort_by(|a, b| a.frontmatter.id.cmp(&b.frontmatter.id));
    out
}

fn strip_block(text: &str, start: &str, end: &str) -> String {
    match (text.find(start), text.find(end)) {
        (Some(s), Some(e)) if e > s => format!("{}{}", &text[..s], &text[e + end.len()..]),
        _ => text.to_string(),
    }
}

fn brief_summary(brain_dir: &Path) -> String {
    let path = brain_dir.join("brief.md");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return "No brief yet; run `tag brain init` to generate `.tag/brain/brief.md`.".into();
    };
    let body = match parse_doc(&path, &text) {
        Ok(d) => d.body,
        Err(_) => text,
    };
    // Drop the managed "recent runs" block: it changes constantly.
    let body = strip_block(&body, "<!-- tag:runs:start -->", "<!-- tag:runs:end -->");
    let all: Vec<&str> = body.trim().lines().collect();
    if all.is_empty() {
        return "Brief is empty; see `.tag/brain/brief.md`.".into();
    }
    let mut s = all.iter().take(40).copied().collect::<Vec<_>>().join("\n");
    if all.len() > 40 {
        s.push_str("\n\n(truncated; full brief: `.tag/brain/brief.md`)");
    }
    s
}

/// Render the managed block content (markers included, `\n` line endings).
pub fn render_agent_block(repo_root: &Path) -> String {
    let brain = repo_root.join(".tag").join("brain");
    let mut s = String::new();
    s.push_str(BLOCK_START);
    s.push_str("\n## TAG project brain\n\n");
    s.push_str("_Managed by `tag brain sync-agent-files`; edit the brain, not this block._\n\n");
    s.push_str("### Brief\n\n");
    s.push_str(&brief_summary(&brain));
    s.push_str("\n\n");
    for (title, kind, sub) in [
        ("Decisions", DocKind::Decision, "decisions"),
        ("Patterns", DocKind::Pattern, "patterns"),
    ] {
        s.push_str(&format!("### {title}\n\n"));
        let docs: Vec<Doc> = read_docs(&brain.join(sub))
            .into_iter()
            .filter(|d| d.frontmatter.kind == kind)
            .collect();
        if docs.is_empty() {
            s.push_str("(none yet)\n");
        }
        for d in &docs {
            s.push_str(&format!(
                "- {} (`{}`)\n",
                d.frontmatter.title.replace('\n', " "),
                d.frontmatter.id
            ));
        }
        s.push('\n');
    }
    s.push_str(
        "Search the brain with `tag brain search <query>` or, from a TAG agent session, \
`tag ctl call brain.search`.\n",
    );
    s.push_str(BLOCK_END);
    s
}

/// Pure text transform: put `block` (LF endings) into `existing`, preserving
/// every byte outside the markers. Returns the new text and the outcome.
pub fn apply_block(existing: &str, block: &str) -> (String, SyncOutcome) {
    apply_marked(existing, BLOCK_START, BLOCK_END, block)
}

/// Like [`apply_block`] with caller-supplied markers. `block` must include
/// the markers themselves.
pub fn apply_marked(existing: &str, start: &str, end: &str, block: &str) -> (String, SyncOutcome) {
    let crlf = existing.contains("\r\n");
    let block_eol = if crlf {
        block.replace('\n', "\r\n")
    } else {
        block.to_string()
    };
    if let Some(s) = existing.find(start) {
        if let Some(rel) = existing[s..].find(end) {
            let e = s + rel + end.len();
            let new = format!("{}{}{}", &existing[..s], block_eol, &existing[e..]);
            let outcome = if new == existing {
                SyncOutcome::Unchanged
            } else {
                SyncOutcome::Updated
            };
            return (new, outcome);
        }
    }
    let eol = if crlf { "\r\n" } else { "\n" };
    let mut new = existing.to_string();
    if !new.is_empty() {
        if !new.ends_with('\n') {
            new.push_str(eol);
        }
        new.push_str(eol);
    }
    new.push_str(&block_eol);
    new.push_str(eol);
    (new, SyncOutcome::Appended)
}

/// Sync the managed brain block into the agent files at `repo_root`.
/// Missing files are created only when `create` is true.
pub fn sync_agent_files(repo_root: &Path, create: bool) -> Result<Vec<(PathBuf, SyncOutcome)>> {
    let block = render_agent_block(repo_root);
    let mut out = Vec::new();
    for name in AGENT_FILES {
        let path = repo_root.join(name);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let Ok(existing) = String::from_utf8(bytes) else {
                    // Never rewrite non-UTF-8 files lossily.
                    out.push((path, SyncOutcome::Skipped));
                    continue;
                };
                let (new, outcome) = apply_block(&existing, &block);
                if outcome != SyncOutcome::Unchanged {
                    std::fs::write(&path, new)?;
                }
                out.push((path, outcome));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if create {
                    let (new, _) = apply_block("", &block);
                    std::fs::write(&path, new)?;
                    out.push((path, SyncOutcome::Created));
                } else {
                    out.push((path, SyncOutcome::Skipped));
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let b = t.path().join(".tag/brain");
        std::fs::create_dir_all(b.join("decisions")).unwrap();
        std::fs::create_dir_all(b.join("patterns")).unwrap();
        std::fs::write(b.join("brief.md"), "# Brief\nA thing.\n").unwrap();
        std::fs::write(
            b.join("decisions/a.md"),
            "---\nid: dec_a\ntitle: Use redis\n---\nbody\n",
        )
        .unwrap();
        std::fs::write(
            b.join("patterns/p.md"),
            "---\nid: pat_p\ntitle: JWT\n---\nbody\n",
        )
        .unwrap();
        t
    }

    #[test]
    fn missing_without_create_untouched() {
        let t = repo();
        let r = sync_agent_files(t.path(), false).unwrap();
        assert!(r.iter().all(|(_, o)| *o == SyncOutcome::Skipped));
        assert!(!t.path().join("CLAUDE.md").exists());
    }

    #[test]
    fn create_and_idempotent() {
        let t = repo();
        let r = sync_agent_files(t.path(), true).unwrap();
        assert!(r.iter().all(|(_, o)| *o == SyncOutcome::Created));
        let txt = std::fs::read_to_string(t.path().join("AGENTS.md")).unwrap();
        assert!(txt.contains("Use redis (`dec_a`)"));
        assert!(txt.contains("JWT (`pat_p`)"));
        assert!(txt.contains("tag ctl call brain.search"));
        let r2 = sync_agent_files(t.path(), true).unwrap();
        assert!(r2.iter().all(|(_, o)| *o == SyncOutcome::Unchanged));
        assert_eq!(
            std::fs::read_to_string(t.path().join("AGENTS.md")).unwrap(),
            txt
        );
    }

    #[test]
    fn preserves_outside_bytes_no_trailing_newline() {
        let t = repo();
        let p = t.path().join("CLAUDE.md");
        std::fs::write(&p, "# Mine\nkeep me").unwrap();
        sync_agent_files(t.path(), false).unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.starts_with("# Mine\nkeep me\n\n<!-- tag:brain:start -->"));
        let with_tail = format!("{after}tail without newline");
        std::fs::write(&p, &with_tail).unwrap();
        std::fs::write(
            t.path().join(".tag/brain/decisions/b.md"),
            "---\nid: dec_b\ntitle: New\n---\nx\n",
        )
        .unwrap();
        let r = sync_agent_files(t.path(), false).unwrap();
        assert_eq!(r[0].1, SyncOutcome::Updated);
        let updated = std::fs::read_to_string(&p).unwrap();
        assert!(updated.starts_with("# Mine\nkeep me\n\n"));
        assert!(updated.ends_with("<!-- tag:brain:end -->\ntail without newline"));
        assert!(updated.contains("dec_b"));
    }

    #[test]
    fn crlf_preserved() {
        let t = repo();
        let p = t.path().join("GEMINI.md");
        std::fs::write(&p, "line1\r\nline2\r\n").unwrap();
        sync_agent_files(t.path(), false).unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.starts_with("line1\r\nline2\r\n\r\n<!-- tag:brain:start -->\r\n"));
        assert_eq!(s.matches('\n').count(), s.matches("\r\n").count());
        let r = sync_agent_files(t.path(), false).unwrap();
        assert_eq!(r[2].1, SyncOutcome::Unchanged);
    }

    #[test]
    fn crlf_prefix_and_suffix_bytes_identical() {
        let before =
            "head\r\n<!-- tag:brain:start -->\r\nold\r\n<!-- tag:brain:end -->\r\ntail\r\n";
        let (new, o) = apply_block(before, &format!("{BLOCK_START}\nx\n{BLOCK_END}"));
        assert_eq!(o, SyncOutcome::Updated);
        assert!(new.starts_with("head\r\n"));
        assert!(new.ends_with("\r\ntail\r\n"));
    }
}
