//! `tag-brain`: a Markdown knowledge base ("brain") with a rebuildable SQLite
//! FTS5 index.
//!
//! Documents are Markdown files with YAML frontmatter and `[[wikilinks]]`.
//! The SQLite index is only a cache: it can always be regenerated from the
//! files with [`Brain::reindex`].

mod agent_files;
mod doc;
mod error;
mod index;
mod init;
mod pin;

pub use agent_files::{
    apply_block, render_agent_block, sync_agent_files, SyncOutcome, AGENT_FILES, BLOCK_END,
    BLOCK_START,
};
pub use doc::{
    extract_links, parse_doc, project_id, render_doc, slugify, write_doc, Doc, DocKind,
    Frontmatter, Source,
};
pub use error::{Error, Result};
pub use index::{Brain, BrainRoot, DocMeta, ReindexStats, SearchHit};
pub use init::{generate_brief, init_project, BriefDraft, InitReport};
pub use pin::{find_doc_file, pin_pattern, remove_doc};

#[cfg(test)]
mod tests;
