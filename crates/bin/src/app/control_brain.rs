//! Control-plane brain methods: search, get, patterns, notes (T5.4).

#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tag_brain::{Brain, BrainRoot, Doc, DocKind, Frontmatter};
use tag_control::methods::*;
use tag_control::{ClientCtx, ControlError, ControlResult, ReplyHandle};

fn respond<T: serde::Serialize>(v: T) -> ControlResult {
    serde_json::to_value(v).map_err(|e| ControlError::internal(e.to_string()))
}

fn brain_err(e: tag_brain::Error) -> ControlError {
    ControlError::internal(format!("brain: {e}"))
}

fn io_err(e: std::io::Error) -> ControlError {
    ControlError::internal(e.to_string())
}

/// Everything a background brain call needs, detached from `AppState`.
#[derive(Clone)]
struct BrainCtx {
    index: PathBuf,
    roots: Vec<BrainRoot>,
    /// `(id, name, root)` of known projects.
    projects: Vec<(String, String, PathBuf)>,
}

impl BrainCtx {
    fn build(state: &mut AppState) -> BrainCtx {
        let mut projects: Vec<(String, String, PathBuf)> = state
            .projects
            .projects
            .iter()
            .map(|p| (p.id.clone(), p.name.clone(), p.root.clone()))
            .collect();
        if let Ok(cwd_repo) = repo_for(state, None) {
            let id = tag_brain::project_id(&cwd_repo);
            if !projects.iter().any(|p| p.0 == id) {
                let name = cwd_repo
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                projects.push((id, name, cwd_repo));
            }
        }
        let mut roots: Vec<BrainRoot> = projects
            .iter()
            .map(|(id, _, root)| BrainRoot {
                path: root.join(".tag").join("brain"),
                project: Some(id.clone()),
            })
            .collect();
        roots.push(BrainRoot {
            path: state.paths.config_dir.join("brain"),
            project: None,
        });
        BrainCtx {
            index: state.paths.state_dir.join("brain.sqlite"),
            roots,
            projects,
        }
    }

    fn open(&self) -> Result<Brain, ControlError> {
        let mut b = Brain::open(&self.index, self.roots.clone()).map_err(brain_err)?;
        b.reindex().map_err(brain_err)?;
        Ok(b)
    }

    /// Resolve a project id or name to its id.
    fn project_id(&self, p: &str) -> Result<String, ControlError> {
        self.projects
            .iter()
            .find(|x| x.0 == p)
            .or_else(|| self.projects.iter().find(|x| x.1 == p))
            .map(|x| x.0.clone())
            .ok_or_else(|| ControlError::not_found(format!("no project {p}")))
    }

    fn project_root(&self, id: &str) -> Result<&Path, ControlError> {
        self.projects
            .iter()
            .find(|x| x.0 == id)
            .map(|x| x.2.as_path())
            .ok_or_else(|| ControlError::not_found(format!("no project {id}")))
    }
}

fn parse_kind(k: &str) -> Result<DocKind, ControlError> {
    DocKind::parse(k).ok_or_else(|| {
        ControlError::invalid_params(format!(
            "unknown kind `{k}` (brief, note, decision, pattern)"
        ))
    })
}

/// Dispatch brain methods. Returns `None` if `method` isn't one of them.
pub(crate) fn handle_brain(
    state: &mut AppState,
    client: &ClientCtx,
    method: &str,
    params: Value,
    reply: ReplyHandle,
) -> Option<()> {
    if !matches!(
        method,
        "brain.search" | "brain.get" | "brain.get_pattern" | "brain.import_pattern" | "brain.note"
    ) {
        return None;
    }
    let ctx = BrainCtx::build(state);
    let r = reply.clone();
    let result: Result<(), ControlError> = (|| {
        match method {
            "brain.search" => {
                let p = parse_params::<BrainSearchParams>(params)?;
                reply_in_background(r, move || respond(search(&ctx, p)?));
            }
            "brain.get" => {
                let mut p = parse_params::<BrainGetParams>(params)?;
                if p.id.is_none() && p.project.is_none() {
                    let repo = repo_for(state, None)?;
                    p.project = Some(tag_brain::project_id(&repo));
                }
                reply_in_background(r, move || respond(get(&ctx, p)?));
            }
            "brain.get_pattern" => {
                let p = parse_params::<BrainGetPatternParams>(params)?;
                reply_in_background(r, move || respond(get_pattern(&ctx, p)?));
            }
            "brain.import_pattern" => {
                let p = parse_params::<BrainImportParams>(params)?;
                let own = own_worktree(state, client);
                let repo = repo_for(state, None).ok();
                reply_in_background(r, move || respond(import_pattern(&ctx, p, own, repo)?));
            }
            _ => {
                let mut p = parse_params::<BrainNoteParams>(params)?;
                if p.project.is_none() {
                    p.project = Some(tag_brain::project_id(&repo_for(state, None)?));
                }
                reply_in_background(r, move || respond(note(&ctx, p)?));
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        reply.send(Err(e));
    }
    Some(())
}

fn search(ctx: &BrainCtx, p: BrainSearchParams) -> Result<Vec<BrainHit>, ControlError> {
    let kind = p.kind.as_deref().map(parse_kind).transpose()?;
    let pid = p
        .project
        .as_deref()
        .map(|x| ctx.project_id(x))
        .transpose()?;
    let brain = ctx.open()?;
    // The library's project filter is exact (it would drop global docs), so when a
    // project is given, search everything and filter to project + global here.
    let fetch = if pid.is_some() { 1000 } else { p.limit };
    let hits = brain
        .search(&p.query, None, kind, fetch)
        .map_err(brain_err)?;
    Ok(hits
        .into_iter()
        .filter(|h| match &pid {
            Some(id) => h.project.is_none() || h.project.as_deref() == Some(id.as_str()),
            None => true,
        })
        .take(p.limit)
        .map(|h| BrainHit {
            id: h.id,
            kind: h.kind.as_str().into(),
            title: h.title,
            project: h.project,
            snippet: h.snippet,
            score: h.score,
        })
        .collect())
}

fn to_brain_doc(brain: &Brain, doc: &Doc) -> Result<BrainDoc, ControlError> {
    let fm = &doc.frontmatter;
    Ok(BrainDoc {
        id: fm.id.clone(),
        kind: fm.kind.as_str().into(),
        title: fm.title.clone(),
        frontmatter: serde_json::to_value(fm).map_err(|e| ControlError::internal(e.to_string()))?,
        body: doc.body.clone(),
        links: doc.links.clone(),
        backlinks: brain.backlinks(&fm.id).map_err(brain_err)?,
    })
}

fn get_doc(brain: &Brain, id: &str) -> Result<Doc, ControlError> {
    brain
        .get(id)
        .map_err(brain_err)?
        .ok_or_else(|| ControlError::not_found(format!("no brain document {id}")))
}

fn get(ctx: &BrainCtx, p: BrainGetParams) -> Result<BrainDoc, ControlError> {
    let brain = ctx.open()?;
    let doc = if let Some(id) = &p.id {
        get_doc(&brain, id)?
    } else {
        if let Some(k) = &p.kind {
            if parse_kind(k)? != DocKind::Brief {
                return Err(ControlError::invalid_params(
                    "without `id`, only kind \"brief\" can be fetched",
                ));
            }
        }
        let proj = p
            .project
            .as_deref()
            .ok_or_else(|| ControlError::invalid_params("give `id` or `project`"))?;
        let pid = ctx.project_id(proj)?;
        brain
            .get_brief(&pid)
            .map_err(brain_err)?
            .ok_or_else(|| ControlError::not_found(format!("no brief for project {proj}")))?
    };
    to_brain_doc(&brain, &doc)
}

/// Content of `path` at `commit`, run in `repo`.
fn git_show(repo: &Path, commit: &str, path: &str) -> Result<Vec<u8>, ControlError> {
    let out = std::process::Command::new("git")
        .args(["show", &format!("{commit}:{path}")])
        .current_dir(repo)
        .output()
        .map_err(|e| ControlError::internal(format!("git: {e}")))?;
    if !out.status.success() {
        return Err(ControlError::not_found(format!(
            "git show {commit}:{path}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// Load a pattern doc plus the root of the project its sources live in.
fn load_pattern(ctx: &BrainCtx, brain: &Brain, id: &str) -> Result<(Doc, PathBuf), ControlError> {
    let doc = get_doc(brain, id)?;
    if doc.frontmatter.kind != DocKind::Pattern {
        return Err(ControlError::invalid_params(format!(
            "{id} is a {}, not a pattern",
            doc.frontmatter.kind.as_str()
        )));
    }
    let proj = doc
        .frontmatter
        .project
        .as_deref()
        .ok_or_else(|| ControlError::not_found(format!("pattern {id} has no project")))?;
    let root = ctx.project_root(proj)?.to_path_buf();
    Ok((doc, root))
}

fn pattern_sources(doc: &Doc) -> (Vec<String>, String) {
    match &doc.frontmatter.source {
        Some(s) => (
            s.paths.clone(),
            s.commit.clone().unwrap_or_else(|| "HEAD".into()),
        ),
        None => (Vec::new(), "HEAD".into()),
    }
}

fn get_pattern(ctx: &BrainCtx, p: BrainGetPatternParams) -> Result<PatternResult, ControlError> {
    let brain = ctx.open()?;
    let (doc, root) = load_pattern(ctx, &brain, &p.id)?;
    let mut files = Vec::new();
    if p.include_files {
        let (paths, commit) = pattern_sources(&doc);
        let mut remaining = p.max_bytes;
        for path in paths {
            let bytes = git_show(&root, &commit, &path)?;
            let mut take = bytes.len().min(remaining);
            let truncated = take < bytes.len();
            if truncated {
                // Back off to a UTF-8 boundary (at most 3 bytes) when the file is text.
                let mut t = take;
                while t > 0 && take - t < 4 && std::str::from_utf8(&bytes[..t]).is_err() {
                    t -= 1;
                }
                if std::str::from_utf8(&bytes[..t]).is_ok() {
                    take = t;
                }
            }
            remaining -= take;
            files.push(PatternFile {
                path,
                content: String::from_utf8_lossy(&bytes[..take]).into_owned(),
                truncated,
            });
        }
    }
    Ok(PatternResult {
        pattern: to_brain_doc(&brain, &doc)?,
        files,
    })
}

fn import_pattern(
    ctx: &BrainCtx,
    p: BrainImportParams,
    own: Result<Option<PathBuf>, ControlError>,
    cwd_repo: Option<PathBuf>,
) -> Result<PathResult, ControlError> {
    let brain = ctx.open()?;
    let (doc, root) = load_pattern(ctx, &brain, &p.id)?;
    // Find the worktree in the current repo first, then in any known project.
    let mut candidates: Vec<PathBuf> = cwd_repo.into_iter().collect();
    candidates.extend(ctx.projects.iter().map(|x| x.2.clone()));
    let wt = candidates
        .iter()
        .find_map(|r| find_worktree(r, &p.worktree).ok())
        .ok_or_else(|| ControlError::not_found(format!("no worktree {}", p.worktree)))?;
    check_worktree_scope(&own, &wt.path)?;

    let (paths, commit) = pattern_sources(&doc);
    // Fetch everything before writing anything.
    let mut contents = Vec::new();
    for path in &paths {
        let rel = Path::new(path);
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(ControlError::invalid_params(format!(
                "unsafe pattern source path {path}"
            )));
        }
        contents.push((rel.to_path_buf(), git_show(&root, &commit, path)?));
    }
    let dir = wt
        .path
        .join(".tag")
        .join("ref")
        .join(tag_brain::slugify(&doc.frontmatter.id));
    std::fs::create_dir_all(&dir).map_err(io_err)?;
    for (rel, bytes) in contents {
        let dest = dir.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io_err)?;
        }
        std::fs::write(&dest, bytes).map_err(io_err)?;
    }
    std::fs::write(dir.join("PATTERN.md"), tag_brain::render_doc(&doc)).map_err(io_err)?;
    exclude_in_repo(&wt.path, ".tag/ref/");
    Ok(PathResult {
        path: dir.display().to_string(),
    })
}

fn note(ctx: &BrainCtx, p: BrainNoteParams) -> Result<IdResult, ControlError> {
    let kind = parse_kind(&p.kind)?;
    if kind == DocKind::Brief {
        return Err(ControlError::invalid_params(
            "kind must be note, decision or pattern",
        ));
    }
    if p.title.trim().is_empty() {
        return Err(ControlError::invalid_params("title must not be empty"));
    }
    let proj = p.project.as_deref().unwrap_or_default();
    let pid = ctx.project_id(proj)?;
    let root = ctx.project_root(&pid)?.join(".tag").join("brain");
    let brain = ctx.open()?;
    let base = format!("{}_{}", kind.id_prefix(), tag_brain::slugify(&p.title));
    let mut id = base.clone();
    let mut n = 2;
    while brain.get(&id).map_err(brain_err)?.is_some() {
        id = format!("{base}-{n}");
        n += 1;
    }
    let mut body = p.body.trim_end().to_string();
    if !p.links.is_empty() {
        let links: Vec<String> = p.links.iter().map(|l| format!("[[{l}]]")).collect();
        body.push_str(&format!("\n\nRelated: {}", links.join(", ")));
    }
    body.push('\n');
    let doc = Doc {
        frontmatter: Frontmatter {
            id: id.clone(),
            kind,
            title: p.title.clone(),
            project: Some(pid),
            tags: p.tags,
            source: None,
            created: Some(chrono::Utc::now().format("%Y-%m-%d").to_string()),
            extra: Default::default(),
        },
        links: tag_brain::extract_links(&body),
        body,
        path: PathBuf::new(),
    };
    tag_brain::write_doc(&root, &doc).map_err(brain_err)?;
    Ok(IdResult { id })
}
