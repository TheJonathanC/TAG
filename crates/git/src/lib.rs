pub mod backend;
pub mod diff;
pub mod libgit2_backend;
pub mod overlay;

pub use backend::{FileChange, GitBackend, RepoStatus, WorktreeInfo};
pub use diff::format_diff_lines;
pub use libgit2_backend::StandardGitBackend;
pub use overlay::{render_git_overlay, GitOverlayState, GitPromptMode, GitPromptState};
