use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_main: bool,
}

#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: String,
    pub status: &'static str, // modified, added, deleted, untracked
    pub is_staged: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RepoStatus {
    pub branch: String,
    pub ahead: usize,
    pub behind: usize,
    pub changed_files: Vec<FileChange>,
}

pub trait GitBackend: Send + Sync {
    fn repo_status(&self, path: &std::path::Path) -> Result<RepoStatus, anyhow::Error>;
    fn list_worktrees(&self, path: &std::path::Path) -> Result<Vec<WorktreeInfo>, anyhow::Error>;
    fn create_worktree(
        &self,
        repo_path: &std::path::Path,
        name: &str,
        branch: &str,
    ) -> Result<PathBuf, anyhow::Error>;
    fn delete_worktree(
        &self,
        repo_path: &std::path::Path,
        name: &str,
        force: bool,
    ) -> Result<(), anyhow::Error>;
    fn get_diff(&self, path: &std::path::Path) -> Result<String, anyhow::Error>;
    fn merge_branch(
        &self,
        repo_path: &std::path::Path,
        branch: &str,
    ) -> Result<String, anyhow::Error>;
    fn abort_merge(&self, repo_path: &std::path::Path) -> Result<(), anyhow::Error>;
    fn stage_file(&self, path: &std::path::Path, file: &str) -> Result<(), anyhow::Error>;
    fn commit(&self, path: &std::path::Path, message: &str) -> Result<(), anyhow::Error>;
}
