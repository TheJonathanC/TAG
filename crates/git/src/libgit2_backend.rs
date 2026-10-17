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
            let b = String::from_utf8_lossy(&branch_out.stdout)
                .trim()
                .to_string();
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
                let parts: Vec<&str> = count_str.split_whitespace().map(|s| s.trim()).collect();
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
        if name.is_empty()
            || name.starts_with('-')
            || name.contains("..")
            || name.contains('/')
            || name.contains('\\')
        {
            return Err(anyhow::anyhow!("Invalid worktree name: {}", name));
        }
        if branch.is_empty() || branch.starts_with('-') || branch.contains("..") {
            return Err(anyhow::anyhow!("Invalid branch name: {}", branch));
        }

        let main_repo = get_main_repo_path(repo_path);
        let worktrees_dir = main_repo.join(".worktrees");
        if !worktrees_dir.exists() {
            std::fs::create_dir_all(&worktrees_dir)?;
        }

        ensure_worktree_exclude(&main_repo);

        let wt_path = worktrees_dir.join(name);
        let wt_str = wt_path.to_string_lossy();

        let output = Command::new("git")
            .args(["worktree", "add", "-b", branch, "--", &wt_str])
            .current_dir(&main_repo)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("already exists") {
                let fallback = Command::new("git")
                    .args(["worktree", "add", "--", &wt_str, branch])
                    .current_dir(&main_repo)
                    .output()?;
                if !fallback.status.success() {
                    let err = String::from_utf8_lossy(&fallback.stderr);
                    return Err(anyhow::anyhow!("git worktree add failed: {}", err.trim()));
                }
            } else {
                return Err(anyhow::anyhow!(
                    "git worktree add failed: {}",
                    stderr.trim()
                ));
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
        if name.is_empty() || name.starts_with('-') {
            return Err(anyhow::anyhow!("Invalid worktree name: {}", name));
        }
        let main_repo = get_main_repo_path(repo_path);
        let mut cmd = Command::new("git");
        cmd.args(["worktree", "remove"]);
        if force {
            cmd.arg("--force");
        }
        cmd.args(["--", name]);
        cmd.current_dir(&main_repo);

        let output = cmd.output()?;
        if !output.status.success() {
            let wt_rel = format!(".worktrees/{}", name);
            let mut cmd2 = Command::new("git");
            cmd2.args(["worktree", "remove"]);
            if force {
                cmd2.arg("--force");
            }
            cmd2.args(["--", &wt_rel]);
            cmd2.current_dir(&main_repo);
            let out2 = cmd2.output()?;

            if !out2.status.success() {
                let wt_abs = main_repo.join(".worktrees").join(name);
                let wt_abs_str = wt_abs.to_string_lossy();
                let mut cmd3 = Command::new("git");
                cmd3.args(["worktree", "remove"]);
                if force {
                    cmd3.arg("--force");
                }
                cmd3.args(["--", &wt_abs_str]);
                cmd3.current_dir(&main_repo);
                let out3 = cmd3.output()?;

                if !out3.status.success() {
                    let _ = Command::new("git")
                        .args(["worktree", "prune"])
                        .current_dir(&main_repo)
                        .output();

                    let stderr = String::from_utf8_lossy(&out3.stderr);
                    return Err(anyhow::anyhow!(
                        "git worktree remove failed: {}",
                        stderr.trim()
                    ));
                }
            }
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
                    if let Some(fpath_raw) = line.strip_prefix("?? ") {
                        let fpath = fpath_raw.trim();
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

    fn merge_branch(&self, repo_path: &Path, branch: &str) -> Result<String, anyhow::Error> {
        if branch.is_empty() || branch.starts_with('-') {
            return Err(anyhow::anyhow!("Invalid branch name: {}", branch));
        }
        let output = Command::new("git")
            .args(["merge", "--no-edit", "--", branch])
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

        let result = if !stdout.is_empty() {
            stdout
        } else {
            "Merge successful".to_string()
        };
        Ok(result)
    }

    fn abort_merge(&self, repo_path: &Path) -> Result<(), anyhow::Error> {
        let output = Command::new("git")
            .args(["merge", "--abort"])
            .current_dir(repo_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!(
                "git merge --abort failed: {}",
                stderr.trim()
            ));
        }

        Ok(())
    }

    fn stage_file(&self, path: &Path, file: &str) -> Result<(), anyhow::Error> {
        let output = Command::new("git")
            .args(["add", "--", file])
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

fn get_main_repo_path(repo_path: &Path) -> PathBuf {
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(repo_path)
        .output()
    {
        if output.status.success() {
            let common_git_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let p = PathBuf::from(common_git_dir);
            if let Some(parent) = p.parent() {
                if parent.exists() {
                    return parent.to_path_buf();
                }
            }
        }
    }

    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(repo_path)
        .output()
    {
        if output.status.success() {
            let toplevel = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !toplevel.is_empty() {
                return PathBuf::from(toplevel);
            }
        }
    }

    repo_path.to_path_buf()
}

fn ensure_worktree_exclude(main_repo: &Path) {
    let exclude_file = main_repo.join(".git").join("info").join("exclude");
    if let Some(parent) = exclude_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if let Ok(content) = std::fs::read_to_string(&exclude_file) {
        if content
            .lines()
            .any(|l| l.trim() == ".worktrees" || l.trim() == ".worktrees/")
        {
            return;
        }
    }

    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude_file)
    {
        use std::io::Write;
        let _ = writeln!(f, ".worktrees/");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_worktree_in_project_worktrees_folder() {
        let temp_dir = std::env::temp_dir().join(format!("tag_test_repo_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let temp_dir = std::fs::canonicalize(&temp_dir).unwrap_or(temp_dir);

        let init_out = Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&temp_dir)
            .output()
            .unwrap();
        assert!(init_out.status.success());

        let _ = Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&temp_dir)
            .output();
        let _ = Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(&temp_dir)
            .output();

        std::fs::write(temp_dir.join("README.md"), "hello").unwrap();
        let _ = Command::new("git")
            .args(["add", "README.md"])
            .current_dir(&temp_dir)
            .output();
        let _ = Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&temp_dir)
            .output();

        let backend = StandardGitBackend;
        let wt_path = backend
            .create_worktree(&temp_dir, "feature-x", "feature-x")
            .unwrap();

        // Must be inside the project folder under .worktrees, NOT in temp_dir.parent()!
        let expected_wt = temp_dir.join(".worktrees").join("feature-x");
        let wt_canon = std::fs::canonicalize(&wt_path).unwrap_or_else(|_| wt_path.clone());
        let expected_canon =
            std::fs::canonicalize(&expected_wt).unwrap_or_else(|_| expected_wt.clone());
        assert_eq!(wt_canon, expected_canon);
        assert!(wt_path.exists());

        // Verify .worktrees/ is excluded so it doesn't show in git status
        let exclude_content = std::fs::read_to_string(temp_dir.join(".git/info/exclude")).unwrap();
        assert!(exclude_content.contains(".worktrees/"));

        // Verify git status is clean
        let status = backend.repo_status(&temp_dir).unwrap();
        assert!(status.changed_files.is_empty());

        // Verify list_worktrees sees both
        let worktrees = backend.list_worktrees(&temp_dir).unwrap();
        assert_eq!(worktrees.len(), 2);
        assert!(worktrees.iter().any(|w| w.name == "feature-x"));

        // Verify delete_worktree removes it cleanly
        backend
            .delete_worktree(&temp_dir, "feature-x", true)
            .unwrap();
        assert!(!wt_path.exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
