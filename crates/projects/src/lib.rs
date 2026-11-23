//! Project registry and repo/worktree resolution for TAG.
//!
//! A project is a git repository root (linked worktrees resolve to their main
//! repository). The registry is persisted as JSON.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error in {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("serialize error: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub last_opened: DateTime<Utc>,
    #[serde(default)]
    pub pinned: bool,
}

/// Stable project id: first 16 hex chars of SHA-256 of the canonicalized path.
/// Identical to `tag_brain::project_id`.
pub fn project_id(root: &Path) -> String {
    let canon = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let digest = Sha256::digest(canon.to_string_lossy().as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Walk up from `path` to the directory containing `.git`. Linked worktrees
/// resolve to the main repository root. The innermost `.git` wins.
pub fn find_repo_root(path: &Path) -> Option<PathBuf> {
    let start = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut cur: Option<&Path> = Some(&start);
    while let Some(dir) = cur {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            return Some(dir.to_path_buf());
        }
        if dot_git.is_file() {
            return Some(resolve_git_file(dir, &dot_git).unwrap_or_else(|| dir.to_path_buf()));
        }
        cur = dir.parent();
    }
    None
}

/// Resolve a `.git` file (`gitdir: <path>`) to a main repo root when it is a
/// linked worktree; `None` for submodules / unknown layouts.
fn resolve_git_file(dir: &Path, dot_git: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(dot_git).ok()?;
    let line = content.lines().find_map(|l| l.strip_prefix("gitdir:"))?;
    let mut gitdir = PathBuf::from(line.trim());
    if gitdir.is_relative() {
        gitdir = dir.join(gitdir);
    }
    let gitdir = fs::canonicalize(&gitdir).unwrap_or(gitdir);

    let common = match fs::read_to_string(gitdir.join("commondir")) {
        Ok(c) => {
            let p = PathBuf::from(c.trim());
            let p = if p.is_relative() { gitdir.join(p) } else { p };
            fs::canonicalize(&p).unwrap_or(p)
        }
        Err(_) => {
            // <common>/worktrees/<name>
            let parent = gitdir.parent()?;
            if parent.file_name()? != "worktrees" {
                return None;
            }
            parent.parent()?.to_path_buf()
        }
    };
    if common.file_name()? != ".git" {
        return None;
    }
    common.parent().map(Path::to_path_buf)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    projects: Vec<Project>,
    #[serde(default)]
    last_active: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectRegistry {
    pub path: PathBuf,
    pub projects: Vec<Project>,
    pub last_active: Option<String>,
}

impl ProjectRegistry {
    /// An empty registry bound to `path` (nothing is read or written).
    pub fn empty(path: PathBuf) -> Self {
        Self {
            path,
            projects: Vec::new(),
            last_active: None,
        }
    }

    /// Load from `path`. A missing file yields an empty registry; a parse
    /// error is returned and the file is left untouched.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::empty(path.to_path_buf()))
            }
            Err(e) => return Err(e.into()),
        };
        if text.trim().is_empty() {
            return Ok(Self::empty(path.to_path_buf()));
        }
        let f: RegistryFile = serde_json::from_str(&text).map_err(|source| Error::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            projects: f.projects,
            last_active: f.last_active,
        })
    }

    /// Atomic save (temp file + rename).
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let f = RegistryFile {
            projects: self.projects.clone(),
            last_active: self.last_active.clone(),
        };
        let json = serde_json::to_string_pretty(&f)?;
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Insert or update the project at `root`, bumping `last_opened`.
    pub fn touch(&mut self, root: &Path) -> &Project {
        self.touch_named(root, None)
    }

    fn touch_named(&mut self, root: &Path, name: Option<&str>) -> &Project {
        let canon = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let id = project_id(&canon);
        let now = Utc::now();
        let idx = match self.projects.iter().position(|p| p.id == id) {
            Some(i) => {
                self.projects[i].last_opened = now;
                i
            }
            None => {
                let name = name
                    .map(str::to_string)
                    .or_else(|| canon.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| canon.to_string_lossy().into_owned());
                self.projects.push(Project {
                    id,
                    name,
                    root: canon,
                    last_opened: now,
                    pinned: false,
                });
                self.projects.len() - 1
            }
        };
        &self.projects[idx]
    }

    /// Look up by exact id, else by exact name.
    pub fn get(&self, id_or_name: &str) -> Option<&Project> {
        self.projects
            .iter()
            .find(|p| p.id == id_or_name)
            .or_else(|| self.projects.iter().find(|p| p.name == id_or_name))
    }

    /// Pinned first, then most recently opened first.
    pub fn list_recent(&self) -> Vec<&Project> {
        let mut v: Vec<&Project> = self.projects.iter().collect();
        v.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.last_opened.cmp(&a.last_opened))
        });
        v
    }

    /// Mark `id` as the active project. Returns false if unknown.
    pub fn set_active(&mut self, id: &str) -> bool {
        if self.projects.iter().any(|p| p.id == id) {
            self.last_active = Some(id.to_string());
            true
        } else {
            false
        }
    }

    /// cwd -> repo root -> touch; fall back to `last_active`; else `None`.
    pub fn resolve(&mut self, cwd: Option<&Path>) -> Option<Project> {
        if let Some(root) = cwd.and_then(find_repo_root) {
            return Some(self.touch(&root).clone());
        }
        let id = self.last_active.as_deref()?;
        self.projects.iter().find(|p| p.id == id).cloned()
    }

    /// The legacy `default` project keyed by `$HOME`.
    pub fn default_project(&mut self, home: &Path) -> Project {
        self.touch_named(home, Some("default")).clone()
    }
}

#[cfg(test)]
mod tests;
