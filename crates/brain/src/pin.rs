//! Pinning code patterns into a brain, and locating/removing documents.

use crate::doc::{parse_doc, project_id, slugify, write_doc, Doc, DocKind, Frontmatter, Source};
use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(msg.into()))
}

/// Today's date (UTC) as `YYYY-MM-DD`.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let z = secs.div_euclid(86400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn git_head(repo: &Path) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !out.status.success() {
        return invalid(format!(
            "{} is not a git repository with at least one commit",
            repo.display()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn md_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            md_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

fn ids_in(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    md_files(dir, &mut files);
    files
        .iter()
        .filter_map(|p| {
            let text = std::fs::read_to_string(p).ok()?;
            parse_doc(p, &text).ok().map(|d| d.frontmatter.id)
        })
        .collect()
}

/// Pin `paths` (inside `repo_root`) as a pattern document.
///
/// Writes to `<repo_root>/.tag/brain/patterns/`, or `<global_root>/patterns/`
/// when `global_root` is given. Returns the written document (its `path` set).
pub fn pin_pattern(
    repo_root: &Path,
    paths: &[PathBuf],
    title: &str,
    tags: Vec<String>,
    desc: Option<String>,
    global_root: Option<&Path>,
) -> Result<Doc> {
    if paths.is_empty() {
        return invalid("no paths given");
    }
    if title.trim().is_empty() {
        return invalid("title must not be empty");
    }
    let repo = std::fs::canonicalize(repo_root)?;
    let mut rel = Vec::new();
    for p in paths {
        let abs = if p.is_absolute() {
            p.clone()
        } else {
            repo.join(p)
        };
        let abs = std::fs::canonicalize(&abs)
            .map_err(|_| Error::Invalid(format!("path does not exist: {}", p.display())))?;
        let r = abs
            .strip_prefix(&repo)
            .map_err(|_| Error::Invalid(format!("path is outside the repo: {}", p.display())))?;
        if r.as_os_str().is_empty() {
            return invalid("cannot pin the repository root itself");
        }
        rel.push(r.to_string_lossy().replace('\\', "/"));
    }
    let commit = git_head(&repo)?;

    let root = match global_root {
        Some(g) => g.to_path_buf(),
        None => repo.join(".tag").join("brain"),
    };
    // Ids are unique across the whole brain: check the project brain and, when
    // given, the global one, whichever we write to.
    let mut existing = ids_in(&repo.join(".tag").join("brain"));
    existing.extend(ids_in(&root));
    let base = format!("pat_{}", slugify(title));
    let mut id = base.clone();
    let mut n = 2;
    while existing.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }

    let body = desc
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| "## What it does\n\n\n## How to adapt\n\n\n## Gotchas\n\n".to_string());
    let mut doc = Doc {
        frontmatter: Frontmatter {
            id,
            kind: DocKind::Pattern,
            title: title.to_string(),
            project: Some(project_id(&repo)),
            tags,
            source: Some(Source {
                paths: rel,
                commit: Some(commit),
            }),
            created: Some(today()),
            extra: Default::default(),
        },
        body: if body.ends_with('\n') {
            body
        } else {
            format!("{body}\n")
        },
        links: Vec::new(),
        path: PathBuf::new(),
    };
    doc.links = crate::doc::extract_links(&doc.body);
    doc.path = write_doc(&root, &doc)?;
    Ok(doc)
}

/// Find the Markdown file whose document id is `id` under any of `roots`.
pub fn find_doc_file(roots: &[PathBuf], id: &str) -> Option<PathBuf> {
    for root in roots {
        let mut files = Vec::new();
        md_files(root, &mut files);
        files.sort();
        for p in files {
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            if parse_doc(&p, &text).is_ok_and(|d| d.frontmatter.id == id) {
                return Some(p);
            }
        }
    }
    None
}

/// Delete the file of document `id`; returns the removed path.
pub fn remove_doc(roots: &[PathBuf], id: &str) -> Result<PathBuf> {
    let Some(p) = find_doc_file(roots, id) else {
        return invalid(format!("no document with id `{id}`"));
    };
    std::fs::remove_file(&p)?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git(dir: &Path, args: &[&str]) {
        let st = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap();
        assert!(st.success());
    }

    fn repo() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        git(t.path(), &["init", "-q"]);
        fs::create_dir_all(t.path().join("src")).unwrap();
        fs::write(t.path().join("src/a.rs"), "fn a() {}\n").unwrap();
        git(t.path(), &["add", "."]);
        git(t.path(), &["commit", "-q", "-m", "init"]);
        t
    }

    #[test]
    fn pin_writes_frontmatter_and_unique_ids() {
        let t = repo();
        let d = pin_pattern(
            t.path(),
            &[PathBuf::from("src/a.rs")],
            "My Pattern",
            vec!["x".into()],
            None,
            None,
        )
        .unwrap();
        assert_eq!(d.frontmatter.id, "pat_my-pattern");
        let src = d.frontmatter.source.as_ref().unwrap();
        assert_eq!(src.paths, vec!["src/a.rs"]);
        assert_eq!(src.commit.as_ref().unwrap().len(), 40);
        assert!(d.path.starts_with(t.path().canonicalize().unwrap()));
        assert!(d.path.ends_with(".tag/brain/patterns/my-pattern.md"));
        assert!(d.body.contains("## Gotchas"));
        assert_eq!(d.frontmatter.created.as_ref().unwrap().len(), 10);
        let parsed = parse_doc(&d.path, &fs::read_to_string(&d.path).unwrap()).unwrap();
        assert_eq!(parsed.frontmatter.id, d.frontmatter.id);
        assert_eq!(parsed.frontmatter.tags, vec!["x"]);

        let d2 = pin_pattern(
            t.path(),
            &[t.path().join("src/a.rs")],
            "My Pattern",
            vec![],
            Some("desc".into()),
            None,
        )
        .unwrap();
        assert_eq!(d2.frontmatter.id, "pat_my-pattern-2");
        assert_ne!(d.path, d2.path);
    }

    #[test]
    fn pin_rejects_bad_input_and_supports_global_and_rm() {
        let t = repo();
        assert!(pin_pattern(t.path(), &[PathBuf::from("nope")], "T", vec![], None, None).is_err());
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("f"), "x").unwrap();
        assert!(pin_pattern(
            t.path(),
            &[outside.path().join("f")],
            "T",
            vec![],
            None,
            None
        )
        .is_err());
        let nogit = tempfile::tempdir().unwrap();
        fs::write(nogit.path().join("f"), "x").unwrap();
        assert!(pin_pattern(nogit.path(), &[PathBuf::from("f")], "T", vec![], None, None).is_err());

        let g = tempfile::tempdir().unwrap();
        let proj = pin_pattern(
            t.path(),
            &[PathBuf::from("src/a.rs")],
            "Glob",
            vec![],
            None,
            None,
        )
        .unwrap();
        assert_eq!(proj.frontmatter.id, "pat_glob");
        let d = pin_pattern(
            t.path(),
            &[PathBuf::from("src/a.rs")],
            "Glob",
            vec![],
            None,
            Some(g.path()),
        )
        .unwrap();
        assert!(d.path.starts_with(g.path().join("patterns")));
        let roots = vec![g.path().to_path_buf()];
        assert_eq!(find_doc_file(&roots, "pat_glob-2"), Some(d.path.clone()));
        assert_eq!(remove_doc(&roots, "pat_glob-2").unwrap(), d.path);
        assert!(!d.path.exists());
        assert!(remove_doc(&roots, "pat_glob-2").is_err());
    }
}
