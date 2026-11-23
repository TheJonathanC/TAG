//! Brief bootstrap: generate a project brief and `.tag/project.toml` from a
//! repository's README, manifests, layout and recent history.

use crate::doc::{project_id, render_doc, slugify, Doc, DocKind, Frontmatter};
use crate::error::{Error, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_BRIEF: usize = 4096;
const SKIP_DIRS: &[&str] = &["target", "node_modules", "dist", "build", "vendor", "venv"];

/// A generated brief plus the commands detected while generating it.
#[derive(Debug, Clone)]
pub struct BriefDraft {
    /// Full Markdown (frontmatter + body).
    pub markdown: String,
    /// Detected test command.
    pub detected_test_command: Option<String>,
    /// Detected lint command.
    pub detected_lint_command: Option<String>,
}

/// What [`init_project`] wrote.
#[derive(Debug, Clone)]
pub struct InitReport {
    /// File the brief was written to.
    pub brief_path: PathBuf,
    /// True when `brief.md` already existed and `brief.generated.md` was written.
    pub wrote_generated_instead: bool,
    /// `.tag/project.toml` if it was created by this call.
    pub project_toml: Option<PathBuf>,
}

fn read(root: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(root.join(name)).ok()
}

fn dir_name(root: &Path) -> String {
    std::fs::canonicalize(root)
        .ok()
        .as_deref()
        .unwrap_or(root)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", s[..end].trim_end())
}

fn flush_para(cur: &mut Vec<String>, paras: &mut Vec<String>) {
    if !cur.is_empty() {
        paras.push(cur.join(" "));
        cur.clear();
    }
}

/// First meaningful README paragraphs, skipping badges, HTML and headings.
fn overview(root: &Path) -> Option<String> {
    let text = ["README.md", "README", "readme.md", "README.rst"]
        .iter()
        .find_map(|n| read(root, n))?;
    let mut paras: Vec<String> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut in_code = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            in_code = !in_code;
            flush_para(&mut cur, &mut paras);
            continue;
        }
        let skip = in_code
            || t.is_empty()
            || t.starts_with('#')
            || t.starts_with('<')
            || t.starts_with("![")
            || t.starts_with("[![")
            || t.starts_with("---")
            || t.starts_with("===")
            || t.starts_with('|')
            || t.starts_with("> [!");
        if skip {
            flush_para(&mut cur, &mut paras);
        } else {
            cur.push(t.to_string());
        }
    }
    flush_para(&mut cur, &mut paras);
    let mut out = String::new();
    for p in paras.into_iter().filter(|p| p.len() > 20).take(3) {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&p);
        if out.len() >= 1500 {
            break;
        }
    }
    (!out.is_empty()).then(|| truncate(&out, 1500))
}

#[derive(Default)]
struct Stack {
    lines: Vec<String>,
    test: Option<String>,
    lint: Option<String>,
    build: Option<String>,
    rust_members: Vec<String>,
}

fn key_deps<'a>(known: &[&'a str], deps: &[String]) -> Vec<&'a str> {
    known
        .iter()
        .copied()
        .filter(|k| deps.iter().any(|d| d == k))
        .collect()
}

fn detect_cargo(root: &Path, s: &mut Stack) {
    let Some(text) = read(root, "Cargo.toml") else {
        return;
    };
    let Ok(v) = text.parse::<toml::Table>() else {
        return;
    };
    let mut line = String::from("Rust");
    let ws = v.get("workspace").and_then(|w| w.as_table());
    let edition = v
        .get("package")
        .and_then(|p| p.get("edition"))
        .or_else(|| {
            ws.and_then(|w| w.get("package"))
                .and_then(|p| p.get("edition"))
        })
        .and_then(|e| e.as_str());
    if let Some(e) = edition {
        line.push_str(&format!(" (edition {e})"));
    }
    if let Some(members) = ws.and_then(|w| w.get("members")).and_then(|m| m.as_array()) {
        for m in members.iter().filter_map(|m| m.as_str()) {
            if let Some(prefix) = m.strip_suffix("/*") {
                if let Ok(rd) = std::fs::read_dir(root.join(prefix)) {
                    let mut names: Vec<String> = rd
                        .flatten()
                        .filter(|e| e.path().join("Cargo.toml").exists())
                        .map(|e| format!("{prefix}/{}", e.file_name().to_string_lossy()))
                        .collect();
                    names.sort();
                    s.rust_members.extend(names);
                }
            } else {
                s.rust_members.push(m.to_string());
            }
        }
        line.push_str(&format!(", workspace with {} crates", s.rust_members.len()));
    }
    let mut deps: Vec<String> = Vec::new();
    for tbl in [
        ws.and_then(|w| w.get("dependencies")),
        v.get("dependencies"),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(t) = tbl.as_table() {
            deps.extend(t.keys().cloned());
        }
    }
    let key = key_deps(
        &[
            "tokio",
            "serde",
            "clap",
            "axum",
            "actix-web",
            "rocket",
            "ratatui",
            "rusqlite",
            "sqlx",
            "diesel",
            "reqwest",
            "anyhow",
            "bevy",
            "tauri",
            "wasm-bindgen",
        ],
        &deps,
    );
    if !key.is_empty() {
        line.push_str(&format!("; key deps: {}", key.join(", ")));
    }
    s.lines.push(line);
    let multi = ws.is_some();
    let (t, l, b) = if multi {
        (
            "cargo test --workspace",
            "cargo clippy --workspace --all-targets -- -D warnings",
            "cargo build --workspace",
        )
    } else {
        (
            "cargo test",
            "cargo clippy --all-targets -- -D warnings",
            "cargo build",
        )
    };
    s.test = Some(t.into());
    s.lint = Some(l.into());
    s.build = Some(b.into());
}

fn detect_node(root: &Path, s: &mut Stack) {
    let Some(text) = read(root, "package.json") else {
        return;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let mut line = String::from("Node.js");
    if let Some(n) = v.get("name").and_then(|n| n.as_str()) {
        line.push_str(&format!(" package `{n}`"));
    }
    let mut deps: Vec<String> = Vec::new();
    for k in ["dependencies", "devDependencies"] {
        if let Some(o) = v.get(k).and_then(|o| o.as_object()) {
            deps.extend(o.keys().cloned());
        }
    }
    let key = key_deps(
        &[
            "react",
            "next",
            "vue",
            "svelte",
            "express",
            "fastify",
            "typescript",
            "vite",
            "jest",
            "vitest",
            "eslint",
            "tailwindcss",
        ],
        &deps,
    );
    if !key.is_empty() {
        line.push_str(&format!("; key deps: {}", key.join(", ")));
    }
    s.lines.push(line);
    let scripts = v.get("scripts").and_then(|o| o.as_object());
    let has = |k: &str| scripts.is_some_and(|o| o.contains_key(k));
    if has("test") {
        s.test = Some("npm test".into());
    }
    if has("lint") {
        s.lint = Some("npm run lint".into());
    }
    if has("build") {
        s.build = Some("npm run build".into());
    }
}

fn detect_python(root: &Path, s: &mut Stack) {
    let py = read(root, "pyproject.toml");
    let req = read(root, "requirements.txt");
    if py.is_none() && req.is_none() {
        return;
    }
    let hay = format!(
        "{}\n{}",
        py.as_deref().unwrap_or(""),
        req.as_deref().unwrap_or("")
    )
    .to_lowercase();
    let mut line = String::from("Python");
    if let Some(name) = py
        .as_deref()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .and_then(|t| {
            t.get("project")
                .or_else(|| t.get("tool").and_then(|x| x.get("poetry")))
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str().map(String::from))
        })
    {
        line.push_str(&format!(" package `{name}`"));
    }
    let key: Vec<&str> = [
        "fastapi",
        "django",
        "flask",
        "pydantic",
        "sqlalchemy",
        "pytest",
        "ruff",
        "numpy",
        "pandas",
        "torch",
    ]
    .into_iter()
    .filter(|k| hay.contains(k))
    .collect();
    if !key.is_empty() {
        line.push_str(&format!("; key deps: {}", key.join(", ")));
    }
    s.lines.push(line);
    s.test = Some("pytest".into());
    if hay.contains("ruff") {
        s.lint = Some("ruff check .".into());
    }
}

fn detect_go(root: &Path, s: &mut Stack) {
    let Some(text) = read(root, "go.mod") else {
        return;
    };
    let mut module = None;
    let mut ver = None;
    for l in text.lines() {
        if let Some(m) = l.strip_prefix("module ") {
            module = Some(m.trim().to_string());
        } else if let Some(g) = l.strip_prefix("go ") {
            ver = Some(g.trim().to_string());
        }
    }
    let mut line = String::from("Go");
    if let Some(v) = ver {
        line.push_str(&format!(" {v}"));
    }
    if let Some(m) = module {
        line.push_str(&format!(", module `{m}`"));
    }
    s.lines.push(line);
    s.test = Some("go test ./...".into());
    s.lint = Some("go vet ./...".into());
    s.build = Some("go build ./...".into());
}

fn guess_dir(name: &str) -> &'static str {
    match name {
        "src" => "source",
        "tests" | "test" | "spec" | "__tests__" => "tests",
        "docs" | "doc" => "documentation",
        "crates" => "Rust workspace crates",
        "scripts" | "bin" => "scripts",
        "examples" => "examples",
        "benches" => "benchmarks",
        "lib" => "library code",
        "app" | "apps" => "application code",
        "packages" => "workspace packages",
        "public" | "static" | "assets" => "static assets",
        "config" | "configs" => "configuration",
        "migrations" => "database migrations",
        "tools" => "tooling",
        _ => "project directory",
    }
}

fn layout(root: &Path, members: &[String]) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<String> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && !SKIP_DIRS.contains(&n.as_str()))
        .collect();
    dirs.sort();
    let mut out: Vec<String> = dirs
        .iter()
        .map(|d| format!("- `{d}/`: {}", guess_dir(d)))
        .collect();
    if !members.is_empty() {
        let names: Vec<&str> = members
            .iter()
            .map(|m| m.rsplit('/').next().unwrap_or(m))
            .collect();
        out.push(format!("- Rust crates: {}", names.join(", ")));
    }
    out
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn recent_work(root: &Path) -> Option<String> {
    let log = git(root, &["log", "--pretty=%s", "-n", "50"])?;
    let subjects: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
    if subjects.is_empty() {
        return None;
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for s in &subjects {
        if let Some((head, _)) = s.split_once(':') {
            let key = head.trim_end_matches('!');
            if !key.is_empty() && key.len() <= 30 && !key.contains(' ') {
                *counts.entry(key.to_string()).or_default() += 1;
            }
        }
    }
    let mut themes: Vec<(String, usize)> = counts.into_iter().collect();
    themes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut out = String::new();
    if !themes.is_empty() {
        let t: Vec<String> = themes
            .iter()
            .take(8)
            .map(|(k, n)| format!("{k}: {n}"))
            .collect();
        out.push_str(&format!(
            "Themes (last {} commits): {}\n\n",
            subjects.len(),
            t.join(", ")
        ));
    }
    out.push_str("Latest:\n");
    for s in subjects.iter().take(5) {
        out.push_str(&format!("- {}\n", truncate(s, 100)));
    }
    Some(out)
}

fn render_body(
    root: &Path,
    stack: &Stack,
    overview: Option<&str>,
    recent: Option<&str>,
    layout_cap: usize,
) -> String {
    let mut b = String::from("## Overview\n\n");
    b.push_str(overview.unwrap_or("(no README found; describe the project here)"));
    b.push_str("\n\n## Stack\n\n");
    if stack.lines.is_empty() {
        b.push_str("(no recognised manifest)\n");
    }
    for l in &stack.lines {
        b.push_str(&format!("- {l}\n"));
    }
    b.push_str("\n## Layout\n\n");
    let lay = layout(root, &stack.rust_members);
    if lay.is_empty() {
        b.push_str("(no top-level directories)\n");
    }
    for l in lay.iter().take(layout_cap) {
        b.push_str(l);
        b.push('\n');
    }
    b.push_str("\n## Commands\n\n");
    let mut any = false;
    for (k, v) in [
        ("Test", &stack.test),
        ("Lint", &stack.lint),
        ("Build", &stack.build),
    ] {
        if let Some(v) = v {
            b.push_str(&format!("- {k}: `{v}`\n"));
            any = true;
        }
    }
    if !any {
        b.push_str("(none detected)\n");
    }
    if let Some(r) = recent {
        b.push_str("\n## Recent work\n\n");
        b.push_str(r);
    }
    b
}

fn build(repo_root: &Path, id: &str) -> Result<BriefDraft> {
    if !repo_root.is_dir() {
        return Err(Error::Invalid(format!(
            "not a directory: {}",
            repo_root.display()
        )));
    }
    let name = dir_name(repo_root);
    let mut stack = Stack::default();
    detect_cargo(repo_root, &mut stack);
    detect_node(repo_root, &mut stack);
    detect_python(repo_root, &mut stack);
    detect_go(repo_root, &mut stack);

    let make = |body: &str| -> String {
        let body = if body.ends_with('\n') {
            body.to_string()
        } else {
            format!("{body}\n")
        };
        render_doc(&Doc {
            frontmatter: Frontmatter {
                id: id.to_string(),
                kind: DocKind::Brief,
                title: format!("{name} brief"),
                project: Some(project_id(repo_root)),
                tags: Vec::new(),
                source: None,
                created: None,
                extra: Default::default(),
            },
            body,
            links: Vec::new(),
            path: PathBuf::new(),
        })
    };

    let recent = recent_work(repo_root);
    let ov = overview(repo_root);
    let mut md = make(&render_body(
        repo_root,
        &stack,
        ov.as_deref(),
        recent.as_deref(),
        40,
    ));
    // Keep the brief under the size cap by shrinking the overview first.
    let mut cap = 1500;
    while md.len() > MAX_BRIEF && cap > 200 {
        cap = cap * 2 / 3;
        let ov2 = ov.as_deref().map(|o| truncate(o, cap));
        md = make(&render_body(
            repo_root,
            &stack,
            ov2.as_deref(),
            recent.as_deref(),
            25,
        ));
    }
    if md.len() > MAX_BRIEF {
        let ov2 = ov.as_deref().map(|o| truncate(o, 200));
        md = make(&render_body(repo_root, &stack, ov2.as_deref(), None, 15));
    }
    Ok(BriefDraft {
        markdown: md,
        detected_test_command: stack.test,
        detected_lint_command: stack.lint,
    })
}

/// Generate a brief for the repository at `repo_root`.
pub fn generate_brief(repo_root: &Path) -> Result<BriefDraft> {
    let name = dir_name(repo_root);
    build(repo_root, &format!("brief_{}", slugify(&name)))
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn project_toml_text(name: &str, test: Option<&str>, lint: Option<&str>) -> String {
    let mut t = format!("name = {}\n", toml_str(name));
    match test {
        Some(c) => t.push_str(&format!("test_command = {}\n", toml_str(c))),
        None => t.push_str("# test_command = \"\"\n"),
    }
    match lint {
        Some(c) => t.push_str(&format!("lint_command = {}\n", toml_str(c))),
        None => t.push_str("# lint_command = \"\"\n"),
    }
    t.push_str(
        "# default_conductor = \"claude\"\n\
         # default_workers = [\"agy-neon\"]\n\
         # merge_policy = \"ask\"        # \"ask\" | \"auto\"\n\
         # require_plan_approval = true\n\
         # max_workers = 4\n\
         # worker_visibility = \"card\"  # \"hidden\" | \"card\" | \"pane\"\n\
         # budget_usd = 5.0            # per run; 0 = unlimited\n\
         # protected_paths = [\".github/\", \"AGENTS.md\"]\n",
    );
    t
}

/// Write `.tag/brain/brief.md` (or `brief.generated.md` if it exists), the
/// brain subdirectories and, if missing, `.tag/project.toml`.
pub fn init_project(repo_root: &Path, write_project_toml: bool) -> Result<InitReport> {
    let draft = generate_brief(repo_root)?;
    let brain = repo_root.join(".tag").join("brain");
    for d in ["notes", "decisions", "patterns"] {
        std::fs::create_dir_all(brain.join(d))?;
    }
    let brief = brain.join("brief.md");
    let wrote_generated_instead = brief.exists();
    let (brief_path, content) = if wrote_generated_instead {
        // Distinct id so both files can be indexed together.
        let md = draft.markdown.replacen("id: brief_", "id: brief_gen_", 1);
        (brain.join("brief.generated.md"), md)
    } else {
        (brief, draft.markdown.clone())
    };
    std::fs::write(&brief_path, content)?;

    let mut project_toml = None;
    let pt = repo_root.join(".tag").join("project.toml");
    if write_project_toml && !pt.exists() {
        let text = project_toml_text(
            &dir_name(repo_root),
            draft.detected_test_command.as_deref(),
            draft.detected_lint_command.as_deref(),
        );
        std::fs::write(&pt, text)?;
        project_toml = Some(pt);
    }
    Ok(InitReport {
        brief_path,
        wrote_generated_instead,
        project_toml,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git_init(p: &Path, msgs: &[&str]) {
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(p)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q"]);
        for m in msgs {
            run(&["commit", "-q", "--allow-empty", "-m", m]);
        }
    }

    #[test]
    fn rust_workspace() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        fs::write(
            p.join("Cargo.toml"),
            "[workspace]\nmembers=[\"crates/*\"]\n[workspace.package]\nedition=\"2021\"\n[workspace.dependencies]\ntokio=\"1\"\nserde=\"1\"\n",
        )
        .unwrap();
        for c in ["alpha", "beta"] {
            fs::create_dir_all(p.join("crates").join(c)).unwrap();
            fs::write(
                p.join("crates").join(c).join("Cargo.toml"),
                "[package]\nname=\"x\"\n",
            )
            .unwrap();
        }
        fs::create_dir_all(p.join("docs")).unwrap();
        fs::create_dir_all(p.join("target")).unwrap();
        fs::write(
            p.join("README.md"),
            "# Proj\n\n[![CI](x)](y)\n\n<p align=\"center\">hi</p>\n\nThis is a terminal tool that does many useful things for people.\n",
        )
        .unwrap();
        git_init(p, &["feat(git): a", "feat(git): b", "fix(tmux): c"]);

        let d1 = generate_brief(p).unwrap();
        let m = &d1.markdown;
        assert!(m.contains("kind: brief"));
        assert!(m.contains("terminal tool"));
        assert!(!m.contains("[![CI"));
        assert!(m.contains("edition 2021"));
        assert!(m.contains("tokio"));
        assert!(m.contains("`crates/`: Rust workspace crates"));
        assert!(m.contains("`docs/`: documentation"));
        assert!(!m.contains("`target/`"));
        assert!(m.contains("alpha, beta"));
        assert!(m.contains("`cargo test --workspace`"));
        assert!(m.contains("feat(git): 2"));
        assert!(m.contains("Latest:"));
        assert!(m.len() < MAX_BRIEF);
        assert_eq!(
            d1.detected_test_command.as_deref(),
            Some("cargo test --workspace")
        );
        assert!(d1.detected_lint_command.unwrap().contains("clippy"));

        let r1 = init_project(p, true).unwrap();
        assert!(!r1.wrote_generated_instead);
        assert!(p.join(".tag/brain/patterns").is_dir());
        let ptoml = r1.project_toml.unwrap();
        let parsed: toml::Table = fs::read_to_string(&ptoml).unwrap().parse().unwrap();
        assert_eq!(
            parsed["test_command"].as_str(),
            Some("cargo test --workspace")
        );
        let before = fs::read(&r1.brief_path).unwrap();
        let toml_before = fs::read(&ptoml).unwrap();

        let r2 = init_project(p, true).unwrap();
        assert!(r2.wrote_generated_instead);
        assert!(r2.brief_path.ends_with("brief.generated.md"));
        assert!(r2.project_toml.is_none());
        assert_eq!(fs::read(&r1.brief_path).unwrap(), before);
        assert_eq!(fs::read(&ptoml).unwrap(), toml_before);
    }

    #[test]
    fn node_project() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        fs::write(
            p.join("package.json"),
            r#"{"name":"web","scripts":{"test":"jest","lint":"eslint .","build":"vite build"},"dependencies":{"react":"18"},"devDependencies":{"typescript":"5"}}"#,
        )
        .unwrap();
        fs::write(
            p.join("README.md"),
            "# Web\n\nA small web frontend built for demonstrating things.\n",
        )
        .unwrap();
        fs::create_dir_all(p.join("src")).unwrap();
        fs::create_dir_all(p.join("node_modules")).unwrap();
        let r = init_project(p, false).unwrap();
        assert!(r.project_toml.is_none());
        let m = fs::read_to_string(&r.brief_path).unwrap();
        assert!(m.contains("Node.js package `web`"));
        assert!(m.contains("react"));
        assert!(m.contains("`npm test`"));
        assert!(m.contains("`npm run lint`"));
        assert!(m.contains("`src/`: source"));
        assert!(!m.contains("node_modules"));
        assert!(!m.contains("Recent work"));
    }

    #[test]
    fn python_project() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        fs::write(
            p.join("pyproject.toml"),
            "[project]\nname=\"svc\"\ndependencies=[\"fastapi\",\"pydantic\"]\n",
        )
        .unwrap();
        fs::create_dir_all(p.join("tests")).unwrap();
        let r = init_project(p, true).unwrap();
        let m = fs::read_to_string(&r.brief_path).unwrap();
        assert!(m.contains("Python package `svc`"));
        assert!(m.contains("fastapi"));
        assert!(m.contains("`pytest`"));
        assert!(m.contains("`tests/`: tests"));
        let t = fs::read_to_string(r.project_toml.unwrap()).unwrap();
        assert!(t.contains("test_command = \"pytest\""));
        assert!(t.contains("# lint_command"));
    }
}
