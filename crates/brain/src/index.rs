use crate::doc::{parse_doc_with_project, Doc, DocKind};
use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// A directory tree of brain documents.
#[derive(Debug, Clone)]
pub struct BrainRoot {
    /// Directory containing the Markdown files.
    pub path: PathBuf,
    /// Project id for project brains; `None` for the global brain.
    pub project: Option<String>,
}

/// Counts from a [`Brain::reindex`] run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReindexStats {
    /// New documents indexed.
    pub added: usize,
    /// Existing documents re-indexed because the file changed.
    pub updated: usize,
    /// Documents dropped because the file vanished (or became unparseable).
    pub removed: usize,
    /// Documents skipped because their mtime was unchanged.
    pub unchanged: usize,
    /// `(id, path)` of files skipped because their id was already taken.
    pub duplicates: Vec<(String, PathBuf)>,
    /// `(path, message)` of files that failed to read or parse.
    pub errors: Vec<(PathBuf, String)>,
}

/// A full-text search result.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// Document id.
    pub id: String,
    /// Document kind.
    pub kind: DocKind,
    /// Document title.
    pub title: String,
    /// Owning project, if any.
    pub project: Option<String>,
    /// Body snippet with matches wrapped in `[` `]`.
    pub snippet: String,
    /// Relevance (higher is better).
    pub score: f64,
}

/// Index metadata for one document.
#[derive(Debug, Clone, PartialEq)]
pub struct DocMeta {
    /// Document id.
    pub id: String,
    /// Document kind.
    pub kind: DocKind,
    /// Document title.
    pub title: String,
    /// Owning project, if any.
    pub project: Option<String>,
    /// File path.
    pub path: PathBuf,
    /// Tags.
    pub tags: Vec<String>,
    /// File mtime (seconds since epoch) at index time.
    pub updated: i64,
}

/// Markdown brain plus its SQLite FTS5 index.
pub struct Brain {
    conn: Connection,
    roots: Vec<BrainRoot>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS docs(
    id TEXT PRIMARY KEY, kind TEXT NOT NULL, title TEXT NOT NULL,
    project TEXT, path TEXT NOT NULL, tags TEXT NOT NULL DEFAULT '',
    updated INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS docs_path ON docs(path);
CREATE VIRTUAL TABLE IF NOT EXISTS docs_fts USING fts5(id UNINDEXED, title, tags, body);
CREATE TABLE IF NOT EXISTS links(src TEXT NOT NULL, dst TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS links_src ON links(src);
CREATE INDEX IF NOT EXISTS links_dst ON links(dst);
";

fn collect_md(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let hidden = e.file_name().to_string_lossy().starts_with('.');
        match e.file_type() {
            Ok(t) if t.is_dir() && !hidden => collect_md(&p, out),
            Ok(t) if t.is_file() && p.extension().is_some_and(|x| x == "md") => out.push(p),
            _ => {}
        }
    }
}

fn mtime_secs(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// Quote each whitespace-separated term so FTS5 syntax characters are inert.
fn fts_query(q: &str) -> Option<String> {
    let terms: Vec<String> = q
        .split_whitespace()
        .map(|t| t.replace('"', ""))
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .map(|t| format!("\"{t}\""))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
}

fn kind_of(s: String) -> DocKind {
    DocKind::parse(&s).unwrap_or(DocKind::Note)
}

fn delete_doc(tx: &rusqlite::Transaction, id: &str) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM docs WHERE id=?1", [id])?;
    tx.execute("DELETE FROM docs_fts WHERE id=?1", [id])?;
    tx.execute("DELETE FROM links WHERE src=?1", [id])?;
    Ok(())
}

impl Brain {
    /// Open (creating if needed) the index at `index_path` for `roots`.
    /// Call [`Brain::reindex`] to populate it.
    pub fn open(index_path: &Path, roots: Vec<BrainRoot>) -> Result<Brain> {
        if let Some(parent) = index_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(index_path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Brain { conn, roots })
    }

    /// Incrementally sync the index with the files on disk (by mtime).
    ///
    /// Files with duplicate ids are skipped (first wins) and reported in
    /// [`ReindexStats::duplicates`]; unparseable files are reported in
    /// [`ReindexStats::errors`].
    pub fn reindex(&mut self) -> Result<ReindexStats> {
        let mut stats = ReindexStats::default();
        // path -> (id, updated)
        let mut existing: HashMap<String, (String, i64)> = HashMap::new();
        {
            let mut st = self.conn.prepare("SELECT path, id, updated FROM docs")?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    (r.get::<_, String>(1)?, r.get::<_, i64>(2)?),
                ))
            })?;
            for r in rows {
                let (p, v) = r?;
                existing.insert(p, v);
            }
        }
        let mut files: Vec<(PathBuf, Option<String>, i64)> = Vec::new();
        for root in &self.roots {
            let mut found = Vec::new();
            collect_md(&root.path, &mut found);
            for p in found {
                let m = mtime_secs(&p);
                files.push((p, root.project.clone(), m));
            }
        }
        let on_disk: HashSet<String> = files
            .iter()
            .map(|(p, _, _)| p.to_string_lossy().into_owned())
            .collect();

        let mut claimed: HashMap<String, PathBuf> = HashMap::new();
        let mut changed = Vec::new();
        for (p, proj, m) in &files {
            let key = p.to_string_lossy().into_owned();
            match existing.get(&key) {
                Some((id, old)) if old == m && !claimed.contains_key(id) => {
                    claimed.insert(id.clone(), p.clone());
                    stats.unchanged += 1;
                }
                _ => changed.push((p.clone(), proj.clone(), *m)),
            }
        }
        let changed_paths: HashSet<String> = changed
            .iter()
            .map(|(p, _, _)| p.to_string_lossy().into_owned())
            .collect();

        let tx = self.conn.transaction()?;
        // Drop vanished files and files about to be re-indexed.
        for (path, (id, _)) in &existing {
            if !on_disk.contains(path) {
                delete_doc(&tx, id)?;
                stats.removed += 1;
            } else if changed_paths.contains(path) {
                delete_doc(&tx, id)?;
            }
        }
        for (path, proj, m) in changed {
            let key = path.to_string_lossy().into_owned();
            let was_indexed = existing.contains_key(&key);
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| {
                    parse_doc_with_project(&path, &t, proj.as_deref()).map_err(|e| e.to_string())
                });
            let doc = match parsed {
                Ok(d) => d,
                Err(e) => {
                    stats.errors.push((path, e));
                    if was_indexed {
                        stats.removed += 1;
                    }
                    continue;
                }
            };
            let fm = &doc.frontmatter;
            if claimed.contains_key(&fm.id) {
                stats.duplicates.push((fm.id.clone(), path));
                if was_indexed {
                    stats.removed += 1;
                }
                continue;
            }
            claimed.insert(fm.id.clone(), path.clone());
            let tags = fm.tags.join(" ");
            tx.execute(
                "INSERT INTO docs(id,kind,title,project,path,tags,updated) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![fm.id, fm.kind.as_str(), fm.title, fm.project, key, tags, m],
            )?;
            tx.execute(
                "INSERT INTO docs_fts(id,title,tags,body) VALUES(?1,?2,?3,?4)",
                params![fm.id, fm.title, tags, doc.body],
            )?;
            for dst in &doc.links {
                tx.execute(
                    "INSERT INTO links(src,dst) VALUES(?1,?2)",
                    params![fm.id, dst],
                )?;
            }
            if was_indexed {
                stats.updated += 1;
            } else {
                stats.added += 1;
            }
        }
        tx.commit()?;
        Ok(stats)
    }

    /// Full-text search ranked by bm25. `project` and `kind` are exact
    /// filters (a `project` filter excludes global docs). Each query term is
    /// quoted, so punctuation cannot break FTS syntax; terms are ANDed.
    pub fn search(
        &self,
        query: &str,
        project: Option<&str>,
        kind: Option<DocKind>,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let Some(q) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let mut st = self.conn.prepare(
            "SELECT d.id, d.kind, d.title, d.project,
                    snippet(docs_fts, 3, '[', ']', '...', 16),
                    bm25(docs_fts, 0.0, 5.0, 3.0, 1.0) AS rank
             FROM docs_fts JOIN docs d ON d.id = docs_fts.id
             WHERE docs_fts MATCH ?1
               AND (?2 IS NULL OR d.project = ?2)
               AND (?3 IS NULL OR d.kind = ?3)
             ORDER BY rank LIMIT ?4",
        )?;
        let rows = st.query_map(
            params![q, project, kind.map(DocKind::as_str), limit as i64],
            |r| {
                Ok(SearchHit {
                    id: r.get(0)?,
                    kind: kind_of(r.get(1)?),
                    title: r.get(2)?,
                    project: r.get(3)?,
                    snippet: r.get(4)?,
                    score: -r.get::<_, f64>(5)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn load(&self, filter: &str, arg: &str) -> Result<Option<Doc>> {
        let sql = format!("SELECT path, project FROM docs WHERE {filter}");
        let row: Option<(String, Option<String>)> = self
            .conn
            .query_row(&sql, [arg], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((path, project)) = row else {
            return Ok(None);
        };
        let path = PathBuf::from(path);
        let text = std::fs::read_to_string(&path)?;
        Ok(Some(parse_doc_with_project(
            &path,
            &text,
            project.as_deref(),
        )?))
    }

    /// Load a document by id, reading it from disk.
    pub fn get(&self, id: &str) -> Result<Option<Doc>> {
        self.load("id = ?1", id)
    }

    /// Load the brief of `project`.
    pub fn get_brief(&self, project: &str) -> Result<Option<Doc>> {
        self.load("kind = 'brief' AND project = ?1", project)
    }

    /// Ids of documents that link to `id`, sorted.
    pub fn backlinks(&self, id: &str) -> Result<Vec<String>> {
        self.strings(
            "SELECT DISTINCT src FROM links WHERE dst = ?1 ORDER BY src",
            id,
        )
    }

    /// Link targets referenced by `id`, sorted.
    pub fn outlinks(&self, id: &str) -> Result<Vec<String>> {
        self.strings(
            "SELECT DISTINCT dst FROM links WHERE src = ?1 ORDER BY dst",
            id,
        )
    }

    fn strings(&self, sql: &str, arg: &str) -> Result<Vec<String>> {
        let mut st = self.conn.prepare(sql)?;
        let rows = st.query_map([arg], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Metadata for every indexed document, ordered by id.
    pub fn all_docs(&self) -> Result<Vec<DocMeta>> {
        let mut st = self.conn.prepare(
            "SELECT id, kind, title, project, path, tags, updated FROM docs ORDER BY id",
        )?;
        let rows = st.query_map([], |r| {
            let tags: String = r.get(5)?;
            let path: String = r.get(4)?;
            Ok(DocMeta {
                id: r.get(0)?,
                kind: kind_of(r.get(1)?),
                title: r.get(2)?,
                project: r.get(3)?,
                path: PathBuf::from(path),
                tags: tags.split_whitespace().map(String::from).collect(),
                updated: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// All `(src, dst)` links, including links whose target does not exist.
    pub fn links(&self) -> Result<Vec<(String, String)>> {
        let mut st = self
            .conn
            .prepare("SELECT src, dst FROM links ORDER BY src, dst")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
