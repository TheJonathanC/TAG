//! Post-run notes: a brain note per finished run plus a managed "Recent
//! runs" block in the project brief.

use crate::agent_files::apply_marked;
use crate::doc::{parse_doc, render_doc, Doc, DocKind, Frontmatter};
use crate::error::Result;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Start marker of the managed recent-runs block.
pub const RUNS_START: &str = "<!-- tag:runs:start -->";
/// End marker of the managed recent-runs block.
pub const RUNS_END: &str = "<!-- tag:runs:end -->";

const MAX_RECENT: usize = 10;

/// One task of a finished run.
#[derive(Debug, Clone, Default)]
pub struct RunNoteTask {
    /// Task id.
    pub id: String,
    /// Task title.
    pub title: String,
    /// Final state (e.g. `merged`).
    pub state: String,
    /// Worker report summary.
    pub summary: Option<String>,
    /// Commits merged for this task.
    pub commits: Vec<String>,
}

/// Everything needed to write a run note.
#[derive(Debug, Clone, Default)]
pub struct RunNoteInput {
    /// Run id.
    pub run_id: String,
    /// The run's goal.
    pub goal: String,
    /// Final run state (`done`, `cancelled`).
    pub state: String,
    /// Tasks.
    pub tasks: Vec<RunNoteTask>,
    /// Project id for the frontmatter.
    pub project: Option<String>,
    /// Creation date as written in the frontmatter.
    pub created: Option<String>,
}

/// Brain id of the note for `run_id`.
pub fn run_note_id(run_id: &str) -> String {
    format!("note_run-{run_id}")
}

/// True if a note for `run_id` already exists in `brain_dir/notes`: a doc
/// with frontmatter `run: <id>`, or a title mentioning the run id.
pub fn run_note_exists(brain_dir: &Path, run_id: &str) -> bool {
    let dir = brain_dir.join("notes");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return false;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        if let Ok(d) = parse_doc(&p, &text) {
            let fm_run = d
                .frontmatter
                .extra
                .get("run")
                .and_then(|v| v.as_str())
                .is_some_and(|r| r == run_id);
            if fm_run || d.frontmatter.title.contains(run_id) {
                return true;
            }
        }
    }
    false
}

/// Render the note body.
pub fn render_run_note_body(i: &RunNoteInput) -> String {
    let mut b = String::new();
    b.push_str(&format!("# Run {}\n\n", i.run_id));
    b.push_str(&format!("## Goal\n\n{}\n\n", i.goal.trim()));
    b.push_str("## Tasks\n\n");
    if i.tasks.is_empty() {
        b.push_str("(no tasks)\n");
    }
    for t in &i.tasks {
        b.push_str(&format!("- **{}** {} [{}]", t.id, t.title, t.state));
        if let Some(s) = t.summary.as_deref().filter(|s| !s.trim().is_empty()) {
            b.push_str(&format!(": {}", s.trim().replace('\n', " ")));
        }
        b.push('\n');
    }
    b.push_str(&format!("\n## Outcome\n\nRun ended `{}`.\n", i.state));
    let n_ok = i.tasks.iter().filter(|t| t.state == "merged").count();
    b.push_str(&format!("{}/{} tasks merged.\n", n_ok, i.tasks.len()));
    b.push_str("\n## Merged commits\n\n");
    let commits: Vec<String> = i
        .tasks
        .iter()
        .flat_map(|t| t.commits.iter().map(move |c| format!("- `{c}` ({})", t.id)))
        .collect();
    if commits.is_empty() {
        b.push_str("(none)\n");
    } else {
        b.push_str(&commits.join("\n"));
        b.push('\n');
    }
    b
}

/// Write `notes/run-<id>.md` under `brain_dir` unless a note for this run
/// already exists. Returns the path written, or `None` if one existed.
pub fn write_run_note(brain_dir: &Path, i: &RunNoteInput) -> Result<Option<PathBuf>> {
    if run_note_exists(brain_dir, &i.run_id) {
        return Ok(None);
    }
    let dir = brain_dir.join("notes");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("run-{}.md", i.run_id));
    let mut extra = BTreeMap::new();
    extra.insert(
        "run".to_string(),
        serde_yaml::Value::String(i.run_id.clone()),
    );
    let doc = Doc {
        frontmatter: Frontmatter {
            id: run_note_id(&i.run_id),
            kind: DocKind::Note,
            title: format!("Run {}: {}", i.run_id, first_line(&i.goal)),
            project: i.project.clone(),
            tags: vec!["run".into()],
            source: None,
            created: i.created.clone(),
            extra,
        },
        body: render_run_note_body(i),
        links: Vec::new(),
        path: path.clone(),
    };
    std::fs::write(&path, render_doc(&doc))?;
    Ok(Some(path))
}

fn first_line(s: &str) -> String {
    let l = s.trim().lines().next().unwrap_or("");
    if l.chars().count() > 80 {
        let cut: String = l.chars().take(77).collect();
        format!("{cut}...")
    } else {
        l.to_string()
    }
}

/// Add (or move to the top) `run_id` in the managed "Recent runs" block of
/// `brain_dir/brief.md`, keeping the last 10. Creates a minimal brief if
/// absent; text outside the markers is untouched.
pub fn update_recent_runs(brain_dir: &Path, run_id: &str, goal: &str, state: &str) -> Result<()> {
    let path = brain_dir.join("brief.md");
    let existing = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "# Project brief\n".to_string(),
        Err(e) => return Err(e.into()),
    };
    let mut lines: Vec<String> = Vec::new();
    if let (Some(s), Some(e)) = (existing.find(RUNS_START), existing.find(RUNS_END)) {
        if e > s {
            for l in existing[s + RUNS_START.len()..e].lines() {
                let l = l.trim();
                if l.starts_with("- [[") {
                    lines.push(l.to_string());
                }
            }
        }
    }
    let tag = format!("[[{}]]", run_note_id(run_id));
    lines.retain(|l| !l.contains(&tag));
    lines.insert(
        0,
        format!(
            "- {tag} — {} ({state})",
            first_line(goal).replace('\n', " ")
        ),
    );
    lines.truncate(MAX_RECENT);
    let block = format!(
        "{RUNS_START}\n## Recent runs\n\n{}\n{RUNS_END}",
        lines.join("\n")
    );
    let (new, _) = apply_marked(&existing, RUNS_START, RUNS_END, &block);
    std::fs::create_dir_all(brain_dir)?;
    std::fs::write(&path, new)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str) -> RunNoteInput {
        RunNoteInput {
            run_id: id.into(),
            goal: "Add login".into(),
            state: "done".into(),
            tasks: vec![
                RunNoteTask {
                    id: "t1".into(),
                    title: "API".into(),
                    state: "merged".into(),
                    summary: Some("did it".into()),
                    commits: vec!["abc123".into()],
                },
                RunNoteTask {
                    id: "t2".into(),
                    title: "UI".into(),
                    state: "failed".into(),
                    ..Default::default()
                },
            ],
            project: Some("p1".into()),
            created: Some("2026-11-23".into()),
        }
    }

    #[test]
    fn note_content_and_no_duplicate() {
        let t = tempfile::tempdir().unwrap();
        let p = write_run_note(t.path(), &input("r1")).unwrap().unwrap();
        assert!(p.ends_with("notes/run-r1.md"));
        let text = std::fs::read_to_string(&p).unwrap();
        let d = parse_doc(&p, &text).unwrap();
        assert_eq!(d.frontmatter.id, "note_run-r1");
        assert_eq!(d.frontmatter.kind, DocKind::Note);
        assert_eq!(d.frontmatter.extra["run"].as_str(), Some("r1"));
        assert!(d.body.contains("Add login"));
        assert!(d.body.contains("**t1** API [merged]: did it"));
        assert!(d.body.contains("`abc123` (t1)"));
        assert!(d.body.contains("1/2 tasks merged"));
        assert!(write_run_note(t.path(), &input("r1")).unwrap().is_none());
    }

    #[test]
    fn existing_note_by_frontmatter_or_title_detected() {
        let t = tempfile::tempdir().unwrap();
        let n = t.path().join("notes");
        std::fs::create_dir_all(&n).unwrap();
        std::fs::write(n.join("a.md"), "---\ntitle: x\nrun: r9\n---\nb\n").unwrap();
        std::fs::write(n.join("b.md"), "---\ntitle: Summary of run r8\n---\nb\n").unwrap();
        assert!(run_note_exists(t.path(), "r9"));
        assert!(run_note_exists(t.path(), "r8"));
        assert!(!run_note_exists(t.path(), "r7"));
    }

    #[test]
    fn recent_runs_block_managed() {
        let t = tempfile::tempdir().unwrap();
        // Absent brief: created minimal.
        update_recent_runs(t.path(), "r1", "first", "done").unwrap();
        let p = t.path().join("brief.md");
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.starts_with("# Project brief\n"));
        assert!(s.contains("- [[note_run-r1]] — first (done)"));
        // Hand-edit outside markers survives; 12 more runs keep only 10.
        let edited = format!("intro\n{s}outro");
        std::fs::write(&p, &edited).unwrap();
        for i in 2..=12 {
            update_recent_runs(t.path(), &format!("r{i}"), "g", "done").unwrap();
        }
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.starts_with("intro\n# Project brief\n"));
        assert!(s.ends_with("<!-- tag:runs:end -->\noutro"));
        assert_eq!(s.matches("- [[note_run-").count(), 10);
        assert!(s.contains("[[note_run-r12]]"));
        assert!(!s.contains("[[note_run-r1]]"));
        // Re-adding moves to top without duplicating.
        update_recent_runs(t.path(), "r5", "again", "cancelled").unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert_eq!(s.matches("[[note_run-r5]]").count(), 1);
        let first = s.lines().find(|l| l.starts_with("- [[")).unwrap();
        assert!(first.contains("r5") && first.contains("(cancelled)"));
    }
}
