use super::*;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

fn w(p: &Path, s: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
}

struct Fx {
    _t: tempfile::TempDir,
    proj: std::path::PathBuf,
    glob: std::path::PathBuf,
    idx: std::path::PathBuf,
}

fn fixture() -> Fx {
    let t = tempfile::tempdir().unwrap();
    let proj = t.path().join("repo/.tag/brain");
    let glob = t.path().join("cfg/brain");
    w(
        &proj.join("brief.md"),
        "---\nproject: p1\n---\n# Project brief\nA FastAPI backend. See [[pat_jwt-middleware]].\n",
    );
    w(
        &proj.join("patterns/jwt.md"),
        "---\nid: pat_jwt-middleware\nkind: pattern\ntitle: JWT auth middleware (FastAPI)\nproject: p1\ntags: [auth, fastapi, jwt]\nsource: {paths: [\"backend/auth/jwt.py\"], commit: \"4e1f2a9\"}\ncreated: 2026-11-23\n---\nBody. Related: [[pat_refresh-tokens]], [[dec_session-storage|sessions]]\n",
    );
    w(
        &proj.join("decisions/session-storage.md"),
        "# Session storage\nWe store sessions in redis. Uses [[pat_jwt-middleware]].\n",
    );
    w(
        &glob.join("patterns/refresh-tokens.md"),
        "---\ntitle: Refresh tokens\ntags: [auth]\n---\nRotate refresh tokens on use.\n",
    );
    w(&glob.join("notes/misc.md"), "just a note about redis\n");
    let idx = t.path().join("state/brain.db");
    Fx {
        _t: t,
        proj,
        glob,
        idx,
    }
}

fn open(f: &Fx) -> Brain {
    Brain::open(
        &f.idx,
        vec![
            BrainRoot {
                path: f.proj.clone(),
                project: Some("p1".into()),
            },
            BrainRoot {
                path: f.glob.clone(),
                project: None,
            },
        ],
    )
    .unwrap()
}

#[test]
fn roundtrip() {
    let text = "---\nid: pat_x\nkind: pattern\ntitle: X\nproject: p\ntags:\n- a\nsource:\n  paths:\n  - a.rs\n  commit: abc\ncreated: 2026-11-23\nfoo: bar\n---\nbody [[a]] [[b|c]]\n\nmore\n";
    let d = parse_doc(Path::new("/x/patterns/x.md"), text).unwrap();
    assert_eq!(d.frontmatter.created.as_deref(), Some("2026-11-23"));
    assert_eq!(d.links, vec!["a", "b"]);
    assert!(d.frontmatter.extra.contains_key("foo"));
    let r = render_doc(&d);
    let d2 = parse_doc(Path::new("/x/patterns/x.md"), &r).unwrap();
    assert_eq!(d, d2);
}

#[test]
fn defaults_and_links() {
    let d = parse_doc(Path::new("/a/decisions/use-redis.md"), "# Use Redis\nx").unwrap();
    assert_eq!(d.frontmatter.id, "dec_use-redis");
    assert_eq!(d.frontmatter.kind, DocKind::Decision);
    assert_eq!(d.frontmatter.title, "Use Redis");
    let d = parse_doc(Path::new("/a/notes/n.md"), "hi").unwrap();
    assert_eq!(
        (d.frontmatter.id.as_str(), d.frontmatter.title.as_str()),
        ("note_n", "n")
    );
    assert_eq!(
        extract_links("[[a]] [[a|x]] [[ b ]] [[]] [[unclosed\n]] [[c"),
        vec!["a", "b"]
    );
    assert_eq!(
        slugify("JWT auth middleware (FastAPI)!"),
        "jwt-auth-middleware-fastapi"
    );
    assert_eq!(slugify("***"), "untitled");
}

#[test]
fn fts5_available() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE VIRTUAL TABLE t USING fts5(a);")
        .unwrap();
}

#[test]
fn index_search_and_links() {
    let f = fixture();
    let mut b = open(&f);
    let s = b.reindex().unwrap();
    assert_eq!((s.added, s.unchanged), (5, 0), "{s:?}");
    let ids: Vec<_> = b.all_docs().unwrap().into_iter().map(|d| d.id).collect();
    assert!(ids.contains(&"brief_p1".to_string()));
    assert!(ids.contains(&"pat_refresh-tokens".to_string()));

    let hits = b.search("jwt", None, None, 10).unwrap();
    assert_eq!(hits[0].id, "pat_jwt-middleware");
    assert!(hits[0].score > 0.0);
    assert!(b
        .search("redis", Some("p1"), None, 10)
        .unwrap()
        .iter()
        .all(|h| h.project.as_deref() == Some("p1")));
    assert_eq!(
        b.search("redis", None, Some(DocKind::Note), 10)
            .unwrap()
            .len(),
        1
    );
    assert!(b
        .search("auth", None, Some(DocKind::Decision), 10)
        .unwrap()
        .is_empty());
    for q in [
        "", "   ", "\"", "a AND (b", "foo*:bar", "NEAR(", "-x ^y", "...",
    ] {
        b.search(q, None, None, 5).unwrap();
    }

    assert_eq!(
        b.backlinks("pat_jwt-middleware").unwrap(),
        vec!["brief_p1", "dec_session-storage"]
    );
    // cross-root: project doc links to global doc
    assert_eq!(
        b.backlinks("pat_refresh-tokens").unwrap(),
        vec!["pat_jwt-middleware"]
    );
    assert_eq!(
        b.outlinks("pat_jwt-middleware").unwrap(),
        vec!["dec_session-storage", "pat_refresh-tokens"]
    );
    assert_eq!(
        b.get_brief("p1").unwrap().unwrap().frontmatter.id,
        "brief_p1"
    );
    assert!(b.get("nope").unwrap().is_none());
    assert_eq!(
        b.get("pat_refresh-tokens")
            .unwrap()
            .unwrap()
            .frontmatter
            .title,
        "Refresh tokens"
    );
    assert_eq!(b.links().unwrap().len(), 4);
}

#[test]
fn reindex_incremental() {
    let f = fixture();
    let mut b = open(&f);
    b.reindex().unwrap();
    let s = b.reindex().unwrap();
    assert_eq!((s.added, s.updated, s.removed, s.unchanged), (0, 0, 0, 5));

    let p = f.glob.join("notes/misc.md");
    fs::write(&p, "now about postgres\n").unwrap();
    let fh = fs::File::options().write(true).open(&p).unwrap();
    fh.set_modified(SystemTime::now() + Duration::from_secs(100))
        .unwrap();
    let s = b.reindex().unwrap();
    assert_eq!((s.updated, s.unchanged), (1, 4));
    assert_eq!(b.search("postgres", None, None, 5).unwrap().len(), 1);
    assert!(b
        .search("redis", None, Some(DocKind::Note), 5)
        .unwrap()
        .is_empty());

    fs::remove_file(f.proj.join("decisions/session-storage.md")).unwrap();
    let s = b.reindex().unwrap();
    assert_eq!(s.removed, 1);
    assert_eq!(b.backlinks("pat_jwt-middleware").unwrap(), vec!["brief_p1"]);
    assert!(b.get("dec_session-storage").unwrap().is_none());
    assert!(b.outlinks("dec_session-storage").unwrap().is_empty());
}

#[test]
fn duplicate_ids() {
    let f = fixture();
    w(
        &f.glob.join("patterns/dup.md"),
        "---\nid: pat_jwt-middleware\n---\nx\n",
    );
    let mut b = open(&f);
    let s = b.reindex().unwrap();
    assert_eq!(s.duplicates.len(), 1);
    assert!(s.duplicates[0].1.ends_with("dup.md"));
    assert_eq!(s.duplicates[0].0, "pat_jwt-middleware");
}

#[test]
fn write_never_overwrites() {
    let t = tempfile::tempdir().unwrap();
    let d = parse_doc(
        Path::new("x/patterns/a.md"),
        "---\ntitle: My Pat\n---\nhi\n",
    )
    .unwrap();
    let p1 = write_doc(t.path(), &d).unwrap();
    let p2 = write_doc(t.path(), &d).unwrap();
    let p3 = write_doc(t.path(), &d).unwrap();
    assert!(p1.ends_with("patterns/my-pat.md"));
    assert!(p2.ends_with("patterns/my-pat-2.md"));
    assert!(p3.ends_with("patterns/my-pat-3.md"));
    let back = parse_doc(&p1, &fs::read_to_string(&p1).unwrap()).unwrap();
    assert_eq!(back.frontmatter, d.frontmatter);
}

#[test]
fn project_id_stable() {
    let t = tempfile::tempdir().unwrap();
    let a = project_id(t.path());
    assert_eq!(a.len(), 16);
    assert_eq!(a, project_id(t.path()));
}
