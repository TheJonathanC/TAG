use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Kind of a brain document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocKind {
    /// Project brief (`brief.md`).
    Brief,
    /// Free-form note.
    Note,
    /// Recorded decision.
    Decision,
    /// Reusable code pattern.
    Pattern,
}

impl DocKind {
    /// Lowercase string form (`"brief"`, `"note"`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            DocKind::Brief => "brief",
            DocKind::Note => "note",
            DocKind::Decision => "decision",
            DocKind::Pattern => "pattern",
        }
    }

    /// Parse the lowercase string form.
    pub fn parse(s: &str) -> Option<DocKind> {
        match s {
            "brief" => Some(DocKind::Brief),
            "note" => Some(DocKind::Note),
            "decision" => Some(DocKind::Decision),
            "pattern" => Some(DocKind::Pattern),
            _ => None,
        }
    }

    /// Id prefix used when deriving ids from file names.
    pub fn id_prefix(self) -> &'static str {
        match self {
            DocKind::Brief => "brief",
            DocKind::Note => "note",
            DocKind::Decision => "dec",
            DocKind::Pattern => "pat",
        }
    }

    /// Subdirectory (relative to a brain root) where this kind lives.
    /// Briefs live directly in the root.
    pub fn subdir(self) -> Option<&'static str> {
        match self {
            DocKind::Brief => None,
            DocKind::Note => Some("notes"),
            DocKind::Decision => Some("decisions"),
            DocKind::Pattern => Some("patterns"),
        }
    }
}

/// Source references (files and commit) a document was derived from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// Repo-relative source paths.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Commit the document was written against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

/// YAML frontmatter of a document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frontmatter {
    /// Globally unique document id.
    pub id: String,
    /// Document kind.
    pub kind: DocKind,
    /// Human-readable title.
    pub title: String,
    /// Owning project id (None for global brain docs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Free-form tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Source references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    /// Creation date (kept as written, e.g. `2026-11-23`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// Unknown frontmatter keys, preserved on round trip.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

/// A parsed brain document.
#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    /// Frontmatter (with defaults filled in).
    pub frontmatter: Frontmatter,
    /// Markdown body (everything after the frontmatter).
    pub body: String,
    /// Wikilink targets found in the body (deduplicated, in order).
    pub links: Vec<String>,
    /// File the document was read from (or will be written to).
    pub path: PathBuf,
}

#[derive(Deserialize, Default)]
struct RawFrontmatter {
    id: Option<String>,
    kind: Option<DocKind>,
    title: Option<String>,
    project: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    source: Option<Source>,
    created: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_yaml::Value>,
}

/// Split `text` into (frontmatter yaml, body). Yaml is `None` when the text
/// has no frontmatter block.
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let start = match lines.next() {
        Some(first) if first.trim_end() == "---" => first.len(),
        _ => return (None, text),
    };
    let mut pos = start;
    for line in lines {
        if line.trim_end() == "---" {
            return (Some(&text[start..pos]), &text[pos + line.len()..]);
        }
        pos += line.len();
    }
    (None, text)
}

fn infer_kind(path: &Path) -> DocKind {
    if path.file_name().and_then(|n| n.to_str()) == Some("brief.md") {
        return DocKind::Brief;
    }
    for comp in path.components().rev().skip(1) {
        match comp.as_os_str().to_str() {
            Some("notes") => return DocKind::Note,
            Some("decisions") => return DocKind::Decision,
            Some("patterns") => return DocKind::Pattern,
            _ => {}
        }
    }
    DocKind::Note
}

fn first_heading(body: &str) -> Option<String> {
    body.lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Parse a document, filling defaults for `id`, `kind` and `title`.
///
/// A missing `kind` is inferred from the directory, a missing `id` is derived
/// from the kind prefix and file stem, and a missing `title` falls back to
/// the first `# heading` or the stem.
pub fn parse_doc(path: &Path, text: &str) -> Result<Doc> {
    parse_doc_with_project(path, text, None)
}

/// Like [`parse_doc`], using `default_project` when the frontmatter has no
/// `project` (needed to derive `brief_<project>` ids).
pub(crate) fn parse_doc_with_project(
    path: &Path,
    text: &str,
    default_project: Option<&str>,
) -> Result<Doc> {
    let (yaml, body) = split_frontmatter(text);
    let raw: RawFrontmatter = match yaml {
        Some(y) if !y.trim().is_empty() => {
            serde_yaml::from_str(y).map_err(|e| Error::Frontmatter {
                path: path.to_path_buf(),
                msg: e.to_string(),
            })?
        }
        _ => RawFrontmatter::default(),
    };
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();
    let kind = raw.kind.unwrap_or_else(|| infer_kind(path));
    let project = raw.project.or_else(|| default_project.map(String::from));
    let id = match raw.id {
        Some(id) if !id.trim().is_empty() => id,
        _ if kind == DocKind::Brief => {
            format!("brief_{}", project.as_deref().unwrap_or("global"))
        }
        _ => format!("{}_{}", kind.id_prefix(), stem),
    };
    let title = raw
        .title
        .filter(|t| !t.trim().is_empty())
        .or_else(|| first_heading(body))
        .unwrap_or(stem);
    Ok(Doc {
        frontmatter: Frontmatter {
            id,
            kind,
            title,
            project,
            tags: raw.tags,
            source: raw.source,
            created: raw.created,
            extra: raw.extra,
        },
        links: extract_links(body),
        body: body.to_string(),
        path: path.to_path_buf(),
    })
}

/// Render a document back to Markdown with YAML frontmatter.
pub fn render_doc(doc: &Doc) -> String {
    // Serializing plain data cannot realistically fail; fall back to empty.
    let yaml = serde_yaml::to_string(&doc.frontmatter).unwrap_or_default();
    format!("---\n{}---\n{}", yaml, doc.body)
}

/// Extract `[[id]]` / `[[id|alias]]` link targets, deduplicated and in order
/// of first appearance.
pub fn extract_links(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let inner = &after[..end];
        if inner.contains('\n') || inner.contains("[[") {
            // Not a well-formed link; resume just past this opener.
            rest = after;
            continue;
        }
        let target = inner.split(['|', '#']).next().unwrap_or("").trim();
        if !target.is_empty() && !out.iter().any(|t| t == target) {
            out.push(target.to_string());
        }
        rest = &after[end + 2..];
    }
    out
}

/// Lowercase, hyphen-separated slug of `s` (`"untitled"` if nothing remains).
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.extend(c.to_lowercase());
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

/// Write `doc` under the brain root `root`, in the subdirectory for its kind
/// (briefs go in the root) using `slugify(title)` as the file name. Never
/// overwrites: appends `-2`, `-3`, ... on collision. Returns the path written.
pub fn write_doc(root: &Path, doc: &Doc) -> Result<PathBuf> {
    let dir = match doc.frontmatter.kind.subdir() {
        Some(sub) => root.join(sub),
        None => root.to_path_buf(),
    };
    std::fs::create_dir_all(&dir)?;
    let slug = slugify(&doc.frontmatter.title);
    let content = render_doc(doc);
    let mut n = 1u32;
    loop {
        let name = if n == 1 {
            format!("{slug}.md")
        } else {
            format!("{slug}-{n}.md")
        };
        let path = dir.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                f.write_all(content.as_bytes())?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e.into()),
        }
    }
}

/// Stable project id for a repo: first 16 hex chars of the SHA-256 of its
/// canonicalized path.
pub fn project_id(repo_root: &Path) -> String {
    let canon = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    let digest = Sha256::digest(canon.to_string_lossy().as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}
