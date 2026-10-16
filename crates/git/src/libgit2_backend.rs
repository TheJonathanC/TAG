use std::path::{Path, PathBuf};
use std::process::Command;

use crate::backend::{FileChange, GitBackend, RepoStatus, WorktreeInfo};

pub struct StandardGitBackend;

impl GitBackend for StandardGitBackend {
    fn repo_status(&self, path: &Path) -> Result<RepoStatus, anyhow::Error> {
        let output = Command::new("git")
            .args(["status", "--porcelain=v1", "-uall"])
            .current_dir(path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("git status failed: {}", stderr.trim()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut changed_files = Vec::new();

        for line in stdout.lines() {
            if line.len() < 3 {
                continue;
            }
            let index_stat = line.as_bytes()[0] as char;
            let wt_stat = line.as_bytes()[1] as char;
            let path_str = line[3..].trim().to_string();

            let is_conflict = index_stat == 'U'
                || wt_stat == 'U'
                || (index_stat == 'A' && wt_stat == 'A')
                || (index_stat == 'D' && wt_stat == 'D');

            let (desc, staged) = if is_conflict {
                ("conflict", false)
            } else if index_stat == '?' && wt_stat == '?' {
                ("untracked", false)
            } else if index_stat != ' ' && index_stat != '?' {
                let d = match index_stat {
                    'M' => "modified",
                    'A' => "added",
                    'D' => "deleted",
                    'R' => "renamed",
                    _ => "staged",
                };
                (d, true)
            } else {
                let d = match wt_stat {
                    'M' => "modified",
                    'D' => "deleted",
                    _ => "modified",
                };
                (d, false)
            };

            changed_files.push(FileChange {
                path: path_str,
                status: desc,
                is_staged: staged,
            });
        }

        let branch_out = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(path)
            .output()?;
        let branch = if branch_out.status.success() {
            let b = String::from_utf8_lossy(&branch_out.stdout).trim().to_string();
            if b.is_empty() {
                "HEAD (detached)".to_string()
            } else {
                b
            }
        } else {
            "HEAD (detached)".to_string()
        };

        // Check if merge is currently in progress
        let is_merging = Command::new("git")
            .args(["rev-parse", "-q", "--verify", "MERGE_HEAD"])
            .current_dir(path)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        let final_branch = if is_merging {
            format!("{} [MERGING CONFLICT]", branch)
        } else {
            branch
        };

        // Try getting ahead/behind count from upstream tracking branch
        let mut ahead = 0;
        let mut behind = 0;
        if let Ok(count_out) = Command::new("git")
            .args(["rev-list", "--left-right", "--count", "HEAD...@{u}"])
            .current_dir(path)
            .output()
        {
            if count_out.status.success() {
                let count_str = String::from_utf8_lossy(&count_out.stdout);
                let parts: Vec<&str> = count_str
                    .split_whitespace()
                    .map(|s| s.trim())
                    .collect();
                if parts.len() == 2 {
                    ahead = parts[0].parse().unwrap_or(0);
                    behind = parts[1].parse().unwrap_or(0);
                }
            }
        }

        Ok(RepoStatus {
            branch: final_branch,
            ahead,
            behind,
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
            if stderr.contains("already exists") {
                let fallback = Command::new("git")
                    .args(["worktree", "add", wt_path.to_str().unwrap(), branch])
                    .current_dir(repo_path)
                    .output()?;
                if !fallback.status.success() {
                    let err = String::from_utf8_lossy(&fallback.stderr);
                    return Err(anyhow::anyhow!("git worktree add failed: {}", err.trim()));
                }
            } else {
                return Err(anyhow::anyhow!("git worktree add failed: {}", stderr.trim()));
            }
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
            return Err(anyhow::anyhow!("git worktree remove failed: {}", stderr.trim()));
        }

        Ok(())
    }

    fn get_diff(&self, path: &Path) -> Result<String, anyhow::Error> {
        let output = Command::new("git")
            .args(["diff", "HEAD"])
            .current_dir(path)
            .output()?;

        let mut diff_str = if output.status.success() {
            String::from_utf8_lossy(&output.stdout).to_string()
        } else {
            let out_unstaged = Command::new("git").arg("diff").current_dir(path).output()?;
            String::from_utf8_lossy(&out_unstaged.stdout).to_string()
        };

        if diff_str.is_empty() {
            if let Ok(staged_out) = Command::new("git")
                .args(["diff", "--cached"])
                .current_dir(path)
                .output()
            {
                diff_str = String::from_utf8_lossy(&staged_out.stdout).to_string();
            }
        }

        if diff_str.is_empty() {
            if let Ok(status_out) = Command::new("git")
                .args(["status", "--porcelain=v1", "-uall"])
                .current_dir(path)
                .output()
            {
                let s = String::from_utf8_lossy(&status_out.stdout);
                let mut untracked_diff = String::new();
                for line in s.lines() {
                    if line.starts_with("?? ") {
                        let fpath = line[3..].trim();
                        untracked_diff.push_str(&format!("diff --untracked a/{0} b/{0}\n", fpath));
                        untracked_diff.push_str(&format!("--- /dev/null\n+++ b/{}\n", fpath));
                        let full_p = path.join(fpath);
                        if let Ok(content) = std::fs::read_to_string(&full_p) {
                            for cline in content.lines().take(60) {
                                untracked_diff.push_str(&format!("+{}\n", cline));
                            }
                        } else {
                            untracked_diff.push_str("+[Untracked file]\n");
                        }
                    }
                }
                if !untracked_diff.is_empty() {
                    diff_str = untracked_diff;
                }
            }
        }

        Ok(diff_str)
    }

    fn merge_branch(
        &self,
        repo_path: &Path,
        branch: &str,
    ) -> Result<String, anyhow::Error> {
        let output = Command::new("git")
            .args(["merge", "--no-edit", branch])
            .current_dir(repo_path)
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

        if !output.status.success() {
            let mut msg = String::new();
            if !stdout.is_empty() {
                msg.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !msg.is_empty() {
                    msg.push('\n');
                }
                msg.push_str(&stderr);
            }
            return Err(anyhow::anyhow!("Merge failed: {}", msg));
        }

        let result = if !stdout.is_empty() { stdout } else { "Merge successful".to_string() };
        Ok(result)
    }

    fn abort_merge(&self, repo_path: &Path) -> Result<(), anyhow::Error> {
        let output = Command::new("git")
            .args(["merge", "--abort"])
            .current_dir(repo_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("git merge --abort failed: {}", stderr.trim()));
        }

        Ok(())
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
