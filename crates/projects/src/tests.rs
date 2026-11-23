use super::*;
use std::process::Command;

fn mk(p: &Path) {
    fs::create_dir_all(p).unwrap();
}

#[test]
fn project_id_known_value() {
    // Missing path: no canonicalization, so the value is just sha256(path)[..8].
    let d = Sha256::digest(b"/nonexistent-tag-x");
    let e: String = d.iter().take(8).map(|b| format!("{b:02x}")).collect();
    assert_eq!(project_id(Path::new("/nonexistent-tag-x")), e);
    assert_eq!(e.len(), 16);

    let t = tempfile::tempdir().unwrap();
    let canon = fs::canonicalize(t.path()).unwrap();
    let d = Sha256::digest(canon.to_string_lossy().as_bytes());
    let expect: String = d.iter().take(8).map(|b| format!("{b:02x}")).collect();
    assert_eq!(project_id(t.path()), expect);
}

#[test]
fn finds_plain_repo_root_from_subdir() {
    let t = tempfile::tempdir().unwrap();
    let repo = fs::canonicalize(t.path()).unwrap().join("r");
    mk(&repo.join(".git"));
    mk(&repo.join("a/b"));
    assert_eq!(find_repo_root(&repo.join("a/b")), Some(repo));
}

#[test]
fn worktree_resolves_to_main_root_synthetic() {
    let t = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(t.path()).unwrap();
    let repo = base.join("repo");
    mk(&repo.join(".git/worktrees/x"));
    let wt = repo.join(".worktrees/x");
    mk(&wt.join("src"));
    fs::write(
        wt.join(".git"),
        format!("gitdir: {}\n", repo.join(".git/worktrees/x").display()),
    )
    .unwrap();
    assert_eq!(find_repo_root(&wt.join("src")), Some(repo.clone()));

    fs::write(repo.join(".git/worktrees/x/commondir"), "../..\n").unwrap();
    assert_eq!(find_repo_root(&wt), Some(repo));
}

#[test]
fn worktree_relative_gitdir() {
    let t = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(t.path()).unwrap();
    let repo = base.join("repo");
    mk(&repo.join(".git/worktrees/y"));
    let wt = base.join("other");
    mk(&wt);
    fs::write(wt.join(".git"), "gitdir: ../repo/.git/worktrees/y\n").unwrap();
    assert_eq!(find_repo_root(&wt), Some(repo));
}

#[test]
fn nested_repo_innermost_wins() {
    let t = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(t.path()).unwrap();
    let outer = base.join("outer");
    let inner = outer.join("vendor/inner");
    mk(&outer.join(".git"));
    mk(&inner.join(".git"));
    mk(&inner.join("src"));
    assert_eq!(find_repo_root(&inner.join("src")), Some(inner));
}

#[test]
fn submodule_git_file_is_own_project() {
    let t = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(t.path()).unwrap();
    let outer = base.join("outer");
    mk(&outer.join(".git/modules/sub"));
    let sub = outer.join("sub");
    mk(&sub);
    fs::write(sub.join(".git"), "gitdir: ../.git/modules/sub\n").unwrap();
    assert_eq!(find_repo_root(&sub), Some(sub));
}

#[test]
fn real_git_worktree() {
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(t.path()).unwrap();
    let repo = base.join("repo");
    mk(&repo);
    let git = |args: &[&str]| {
        Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
            .arg(&repo)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if !git(&["init", "-q"]) || !git(&["commit", "-q", "--allow-empty", "-m", "i"]) {
        return;
    }
    if !git(&["worktree", "add", "-q", ".worktrees/x", "-b", "xb"]) {
        return;
    }
    assert_eq!(find_repo_root(&repo.join(".worktrees/x")), Some(repo));
}

#[test]
fn registry_round_trip_and_missing() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("sub/projects.json");
    let mut r = ProjectRegistry::load(&path).unwrap();
    assert!(r.projects.is_empty());
    let root = t.path().join("proj");
    mk(&root);
    let p = r.touch(&root).clone();
    assert_eq!(p.name, "proj");
    assert!(r.set_active(&p.id));
    r.save().unwrap();
    let r2 = ProjectRegistry::load(&path).unwrap();
    assert_eq!(r2.projects, r.projects);
    assert_eq!(r2.last_active.as_deref(), Some(p.id.as_str()));
    assert_eq!(r2.get("proj").unwrap().id, p.id);
    assert_eq!(r2.get(&p.id).unwrap().name, "proj");
}

#[test]
fn parse_error_does_not_overwrite() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("projects.json");
    fs::write(&path, "{ not json").unwrap();
    assert!(ProjectRegistry::load(&path).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
}

#[test]
fn recent_ordering() {
    let t = tempfile::tempdir().unwrap();
    let mut r = ProjectRegistry::empty(t.path().join("p.json"));
    for n in ["a", "b", "c"] {
        mk(&t.path().join(n));
        r.touch(&t.path().join(n));
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let names: Vec<_> = r.list_recent().iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, ["c", "b", "a"]);
    r.projects
        .iter_mut()
        .find(|p| p.name == "a")
        .unwrap()
        .pinned = true;
    r.touch(&t.path().join("b"));
    let names: Vec<_> = r.list_recent().iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, ["a", "b", "c"]);
}

#[test]
fn resolve_and_fallback() {
    let t = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(t.path()).unwrap();
    let mut r = ProjectRegistry::empty(base.join("p.json"));
    assert!(r.resolve(None).is_none());
    let repo = base.join("repo");
    mk(&repo.join(".git"));
    mk(&repo.join("src"));
    let p = r.resolve(Some(&repo.join("src"))).unwrap();
    assert_eq!(p.root, repo);
    r.set_active(&p.id);
    assert_eq!(r.resolve(None).unwrap().id, p.id);
    let nogit = base.join("plain");
    mk(&nogit);
    if find_repo_root(&nogit).is_none() {
        assert_eq!(r.resolve(Some(&nogit)).unwrap().id, p.id);
    }
}

#[test]
fn default_project_keyed_by_home() {
    let t = tempfile::tempdir().unwrap();
    let mut r = ProjectRegistry::empty(t.path().join("p.json"));
    let d = r.default_project(t.path());
    assert_eq!(d.name, "default");
    assert_eq!(d.id, project_id(t.path()));
    let d2 = r.default_project(t.path());
    assert_eq!(d.id, d2.id);
    assert_eq!(r.projects.len(), 1);
}
