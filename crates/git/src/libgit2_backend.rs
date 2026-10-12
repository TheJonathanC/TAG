use git2::{Repository, StatusOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::backend::{FileChange, GitBackend, RepoStatus, WorktreeInfo};

pub struct StandardGitBackend;

impl GitBackend for StandardGitBackend {
    fn repo_status(&self, path: &Path) -> Result<RepoStatus, anyhow::Error> {
        let repo = Repository::discover(path)?;
        let head = repo.head().ok();
        let branch = head
            .as_ref()
            .and_then(|h| h.shorthand())
            .unwrap_or("HEAD (detached)")
            .to_string();

        let mut opts = StatusOptions::new();
        opts.include_untracked(true);
        opts.recurse_untracked_dirs(true);

        let statuses = repo.statuses(Some(&mut opts))?;
        let mut changed_files = Vec::new();

        for entry in statuses.iter() {
            let status = entry.status();
            let path_str = entry.path().unwrap_or("?").to_string();

            let (desc, staged) = if status.is_index_new() {
                ("added", true)
            } else if status.is_index_modified() {
                ("modified", true)
            } else if status.is_index_deleted() {
                ("deleted", true)
            } else if status.is_wt_new() {
                ("untracked", false)
            } else if status.is_wt_modified() {
                ("modified", false)
            } else if status.is_wt_deleted() {
                ("deleted", false)
            } else {
                ("other", false)
            };

            changed_files.push(FileChange {
                path: path_str,
                status: desc,
                is_staged: staged,
            });
        }

        Ok(RepoStatus {
            branch,
            ahead: 0,
            behind: 0,
            changed_files,
        })
    }

    fn list_worktrees(&self, path: &Path) -> Result<Vec<WorktreeInfo>, anyhow::Error> {
        let output = Command::new("git")
            .arg("worktree")
            .arg("list")
            .arg("--porcelain")
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("git worktree list failed: {}", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut list = Vec::new();
        let mut current_path: Option<PathBuf> = None;
        let mut current_branch: Option<String> = None;
        let mut is_first = true;

        for line in stdout.lines() {
            if let Some(stripped) = line.strip_prefix("worktree ") {
                let p = PathBuf::from(stripped);
                current_path = Some(p);
            } else if let Some(b) = line.strip_prefix("branch ") {
                let short_b = b.strip_prefix("refs/heads/").unwrap_or(b).to_string();
                current_branch = Some(short_b);
            } else if line.is_empty() {
                if let Some(p) = current_path.take() {
                    let name = p
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "main".to_string());
                    list.push(WorktreeInfo {
                        name,
                        path: p,
                        branch: current_branch.take(),
                        is_main: is_first,
                    });
                    is_first = false;
                }
            }
        }

        if let Some(p) = current_path.take() {
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "main".to_string());
            list.push(WorktreeInfo {
                name,
                path: p,
                branch: current_branch.take(),
                is_main: is_first,
            });
        }

        Ok(list)
    }

    fn create_worktree(
        &self,
        repo_path: &Path,
        name: &str,
        branch: &str,
    ) -> Result<PathBuf, anyhow::Error> {
        let parent = repo_path.parent().unwrap_or(repo_path);
        let wt_path = parent.join(name);

        let output = Command::new("git")
            .args(["worktree", "add", "-b", branch, wt_path.to_str().unwrap()])
            .current_dir(repo_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("git worktree add failed: {}", stderr));
        }

        Ok(wt_path)
    }

    fn delete_worktree(
        &self,
        repo_path: &Path,
        name: &str,
        force: bool,
    ) -> Result<(), anyhow::Error> {
        let mut cmd = Command::new("git");
        cmd.args(["worktree", "remove"]);
        if force {
            cmd.arg("--force");
        }
        cmd.arg(name);
        cmd.current_dir(repo_path);

        let output = cmd.output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("git worktree remove failed: {}", stderr));
        }

        Ok(())
    }

    fn get_diff(&self, path: &Path) -> Result<String, anyhow::Error> {
        let output = Command::new("git")
            .args(["diff", "HEAD"])
            .current_dir(path)
            .output()?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            // Fallback to diff without HEAD for initial repos
            let out_unstaged = Command::new("git").arg("diff").current_dir(path).output()?;
            Ok(String::from_utf8_lossy(&out_unstaged.stdout).to_string())
        }
    }

    fn stage_file(&self, path: &Path, file: &str) -> Result<(), anyhow::Error> {
        let output = Command::new("git")
            .args(["add", file])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("git add failed: {}", stderr));
        }

        Ok(())
    }

    fn commit(&self, path: &Path, message: &str) -> Result<(), anyhow::Error> {
        let output = Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("git commit failed: {}", stderr));
        }

        Ok(())
    }
}
