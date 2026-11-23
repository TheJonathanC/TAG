//! `tag brain ...`: pin patterns and browse/search the brain from the shell.

use anyhow::{anyhow, bail, Context, Result};
use clap::Subcommand;
use std::path::{Path, PathBuf};
use tag_brain::{
    find_doc_file, pin_pattern, project_id, remove_doc, render_doc, Brain, BrainRoot, DocKind,
};

#[derive(Subcommand, Debug)]
pub(crate) enum BrainCommands {
    /// Pin files from the current repo as a reusable pattern
    Pin {
        /// Files or directories to pin (inside the repo)
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Pattern title
        #[arg(long)]
        title: String,
        /// Comma-separated tags
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// Description (becomes the document body)
        #[arg(long)]
        desc: Option<String>,
        /// Store in the global brain instead of the project brain
        #[arg(long)]
        global: bool,
    },
    /// List brain documents
    Ls {
        /// Only this kind (brief, note, decision, pattern)
        #[arg(long)]
        kind: Option<String>,
        /// Only the global brain
        #[arg(long)]
        global: bool,
        /// Everything in the index
        #[arg(long)]
        all: bool,
    },
    /// Show a document with its links and backlinks
    Show { id: String },
    /// Delete a document
    Rm { id: String },
    /// Full-text search
    Search {
        query: String,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
}

fn repo_root() -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then(|| PathBuf::from(s))
}

fn global_root() -> PathBuf {
    crate::session_io::dirs_or_home()
        .join(".config")
        .join("tag")
        .join("brain")
}

fn parse_kind(k: &Option<String>) -> Result<Option<DocKind>> {
    match k {
        None => Ok(None),
        Some(s) => DocKind::parse(s)
            .map(Some)
            .ok_or_else(|| anyhow!("unknown kind `{s}` (brief, note, decision, pattern)")),
    }
}

fn open_brain(repo: Option<&Path>) -> Result<(Brain, Vec<PathBuf>)> {
    let mut roots = Vec::new();
    if let Some(r) = repo {
        roots.push(BrainRoot {
            path: r.join(".tag").join("brain"),
            project: Some(project_id(r)),
        });
    }
    roots.push(BrainRoot {
        path: global_root(),
        project: None,
    });
    let paths = roots.iter().map(|r| r.path.clone()).collect();
    let idx = crate::app::default_state_dir().join("brain.sqlite");
    let mut brain = Brain::open(&idx, roots).context("opening brain index")?;
    brain.reindex().context("reindexing brain")?;
    Ok((brain, paths))
}

pub(crate) fn handle_brain_command(cmd: BrainCommands) -> Result<()> {
    let repo = repo_root();
    match cmd {
        BrainCommands::Pin {
            paths,
            title,
            tags,
            desc,
            global,
        } => {
            let repo = repo.ok_or_else(|| anyhow!("not inside a git repository"))?;
            let g = global_root();
            let doc = pin_pattern(&repo, &paths, &title, tags, desc, global.then_some(&*g))?;
            println!("{}", doc.frontmatter.id);
            println!("{}", doc.path.display());
            let _ = open_brain(Some(&repo));
        }
        BrainCommands::Ls { kind, global, all } => {
            let kind = parse_kind(&kind)?;
            let (brain, _) = open_brain(repo.as_deref())?;
            let pid = repo.as_deref().map(project_id);
            let mut rows = 0;
            println!("{:<32} {:<9} TITLE", "ID", "KIND");
            for d in brain.all_docs()? {
                let keep = if all {
                    true
                } else if global {
                    d.project.is_none()
                } else {
                    d.project.is_none() || d.project == pid
                };
                if !keep || kind.is_some_and(|k| k != d.kind) {
                    continue;
                }
                println!("{:<32} {:<9} {}", d.id, d.kind.as_str(), d.title);
                rows += 1;
            }
            if rows == 0 {
                println!("(no documents)");
            }
        }
        BrainCommands::Show { id } => {
            let (brain, _) = open_brain(repo.as_deref())?;
            let doc = brain
                .get(&id)?
                .ok_or_else(|| anyhow!("no document with id `{id}`"))?;
            println!("# {}", doc.path.display());
            print!("{}", render_doc(&doc));
            let out = brain.outlinks(&id)?;
            let back = brain.backlinks(&id)?;
            println!("\nLinks: {}", join_or_none(&out));
            println!("Backlinks: {}", join_or_none(&back));
        }
        BrainCommands::Rm { id } => {
            let (_, roots) = open_brain(repo.as_deref())?;
            if find_doc_file(&roots, &id).is_none() {
                bail!("no document with id `{id}`");
            }
            let p = remove_doc(&roots, &id)?;
            println!("removed {id} ({})", p.display());
            open_brain(repo.as_deref())?;
        }
        BrainCommands::Search { query, kind, limit } => {
            let kind = parse_kind(&kind)?;
            let (brain, _) = open_brain(repo.as_deref())?;
            let hits = brain.search(&query, None, kind, limit)?;
            if hits.is_empty() {
                println!("(no results)");
            }
            for h in hits {
                println!("{}  [{}]  {}", h.id, h.kind.as_str(), h.title);
                println!("    {}", h.snippet.replace('\n', " "));
            }
        }
    }
    Ok(())
}

fn join_or_none(v: &[String]) -> String {
    if v.is_empty() {
        "(none)".into()
    } else {
        v.join(", ")
    }
}
