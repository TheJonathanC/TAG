//! T5.4: brain.* control methods end to end.

mod common;

use common::*;
use serde_json::json;
use std::path::PathBuf;

#[test]
fn search_get_import_note_across_projects() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("proj-a");
    let b = tmp.path().join("proj-b");
    init_repo(&a);
    init_repo(&b);
    std::fs::create_dir_all(a.join("src")).unwrap();
    std::fs::write(a.join("src/auth.rs"), "fn jwt_refresh() {}\n").unwrap();
    for args in [&["add", "-A"][..], &["commit", "-q", "-m", "auth"][..]] {
        let st = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(&a)
            .status()
            .unwrap();
        assert!(st.success());
    }
    let doc = tag_brain::pin_pattern(
        &a,
        &[PathBuf::from("src/auth.rs")],
        "Quokka middleware",
        vec!["auth".into()],
        Some("Refreshes the quokka token.".into()),
        None,
    )
    .unwrap();
    let pat_id = doc.frontmatter.id.clone();

    let mut d = Daemon::start_in(None, Some(&b), |state_dir| {
        let mut reg = tag_projects::ProjectRegistry::empty(state_dir.join("projects.json"));
        reg.touch(&a);
        reg.touch(&b);
        reg.save().unwrap();
    });
    let mut c = d.client("ctl");

    let hits = c
        .call_raw("brain.search", json!({"query": "quokka"}))
        .unwrap();
    let hits = hits.as_array().unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0]["id"], pat_id.as_str());
    assert_eq!(hits[0]["kind"], "pattern");

    // Project filter by name excludes other projects' docs.
    let hits = c
        .call_raw(
            "brain.search",
            json!({"query": "quokka", "project": "proj-b"}),
        )
        .unwrap();
    assert!(hits.as_array().unwrap().is_empty());
    let hits = c
        .call_raw(
            "brain.search",
            json!({"query": "quokka", "project": "proj-a"}),
        )
        .unwrap();
    assert_eq!(hits.as_array().unwrap().len(), 1);

    let got = c.call_raw("brain.get", json!({"id": pat_id})).unwrap();
    assert_eq!(got["frontmatter"]["source"]["paths"][0], "src/auth.rs");
    assert!(got["body"].as_str().unwrap().contains("quokka"));

    let pat = c
        .call_raw("brain.get_pattern", json!({"id": pat_id}))
        .unwrap();
    assert_eq!(pat["files"][0]["path"], "src/auth.rs");
    assert_eq!(pat["files"][0]["content"], "fn jwt_refresh() {}\n");
    let small = c
        .call_raw("brain.get_pattern", json!({"id": pat_id, "max_bytes": 5}))
        .unwrap();
    assert_eq!(small["files"][0]["content"], "fn jw");
    assert_eq!(small["files"][0]["truncated"], true);

    let err = c.call_raw("brain.get", json!({"id": "nope"})).unwrap_err();
    assert!(format!("{err:?}").contains("-32003"), "{err:?}");
    assert!(c
        .call_raw("brain.search", json!({"query": "x", "kind": "bogus"}))
        .is_err());

    // Import into a worktree of project B.
    let wt = c
        .call_raw("worktree.create", json!({"name": "w1"}))
        .unwrap();
    let wt_path = PathBuf::from(wt["path"].as_str().unwrap());
    let r = c
        .call_raw(
            "brain.import_pattern",
            json!({"id": pat_id, "worktree": "w1"}),
        )
        .unwrap();
    let dir = PathBuf::from(r["path"].as_str().unwrap());
    assert!(dir.starts_with(wt_path.join(".tag/ref")), "{dir:?}");
    assert_eq!(
        std::fs::read_to_string(dir.join("src/auth.rs")).unwrap(),
        "fn jwt_refresh() {}\n"
    );
    assert!(std::fs::read_to_string(dir.join("PATTERN.md"))
        .unwrap()
        .contains("quokka"));
    let exclude = std::fs::read_to_string(b.join(".git/info/exclude")).unwrap();
    assert!(
        exclude.lines().any(|l| l.trim() == ".tag/ref/"),
        "{exclude}"
    );

    // Notes.
    let n = c
        .call_raw(
            "brain.note",
            json!({"title": "Wombat decision", "body": "We chose wombats.", "kind": "decision",
                   "links": [pat_id], "tags": ["zoo"]}),
        )
        .unwrap();
    let nid = n["id"].as_str().unwrap().to_string();
    assert_eq!(nid, "dec_wombat-decision");
    assert!(b.join(".tag/brain/decisions/wombat-decision.md").exists());
    let hits = c
        .call_raw("brain.search", json!({"query": "wombats"}))
        .unwrap();
    assert_eq!(hits[0]["id"], nid.as_str());
    let linked = c.call_raw("brain.get", json!({"id": pat_id})).unwrap();
    assert!(linked["backlinks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x == nid.as_str()));
    let again = c
        .call_raw(
            "brain.note",
            json!({"title": "Wombat decision", "body": "x", "kind": "decision"}),
        )
        .unwrap();
    assert_eq!(again["id"], "dec_wombat-decision-2");
    assert!(c
        .call_raw(
            "brain.note",
            json!({"title": "t", "body": "b", "kind": "brief"})
        )
        .is_err());

    d.shutdown();
}
