//! Scripted fake agent interpreter. See README.md for the script format.

use regex::Regex;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Execution environment (injectable for tests).
#[derive(Debug, Clone)]
pub struct Config {
    pub cwd: PathBuf,
    pub notify_bin: String,
    pub tag_bin: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            notify_bin: std::env::var("FAKE_AGENT_NOTIFY_BIN")
                .unwrap_or_else(|_| "cockpit-notify".into()),
            tag_bin: std::env::var("FAKE_AGENT_TAG_BIN").unwrap_or_else(|_| "tag".into()),
        }
    }
}

pub const EXIT_EOF: i32 = 3;
pub const EXIT_COMMIT_FAILED: i32 = 4;

fn subst(text: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find('}') {
            Some(j) => {
                let name = &after[..j];
                if let Some(env) = name.strip_prefix("env:") {
                    out.push_str(&std::env::var(env).unwrap_or_default());
                } else if let Some(v) = vars.get(name) {
                    out.push_str(v);
                } else {
                    out.push_str(&rest[i..i + 2 + j + 1]);
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn json_path(v: &serde_json::Value, path: &str) -> Option<String> {
    let mut cur = v;
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        cur = match cur {
            serde_json::Value::Array(a) => a.get(seg.parse::<usize>().ok()?)?,
            _ => cur.get(seg)?,
        };
    }
    Some(match cur {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

fn run_ctl(cfg: &Config, method: &str, json: &str) -> Result<String, String> {
    let o = Command::new(&cfg.tag_bin)
        .args(["ctl", "call", method, json])
        .current_dir(&cfg.cwd)
        .output()
        .map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
    }
}

fn git_ok(cfg: &Config, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(&cfg.cwd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run a script; returns the process exit code.
pub fn run<R: BufRead, W: Write>(
    script: &str,
    prompt: Option<&str>,
    cfg: &Config,
    stdin: &mut R,
    out: &mut W,
) -> i32 {
    let mut vars: HashMap<String, String> = HashMap::new();
    if let Some(p) = prompt {
        let _ = writeln!(out, "PROMPT: {p}");
        let _ = out.flush();
    }
    for raw in script.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (cmd, rest) = match line.split_once(char::is_whitespace) {
            Some((c, r)) => (c, subst(r.trim_start(), &vars)),
            None => (line, String::new()),
        };
        match cmd {
            "print" => {
                let _ = writeln!(out, "{}", rest.replace("\\e", "\x1b"));
            }
            "wait_input" => {
                let re = match Regex::new(&rest) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = writeln!(out, "BAD_REGEX: {e}");
                        return 2;
                    }
                };
                loop {
                    let mut l = String::new();
                    match stdin.read_line(&mut l) {
                        Ok(0) | Err(_) => {
                            let _ = out.flush();
                            return EXIT_EOF;
                        }
                        Ok(_) => {
                            let l = l.trim_end_matches(['\r', '\n']);
                            if re.is_match(l) {
                                let _ = writeln!(out, "INPUT: {l}");
                                break;
                            }
                        }
                    }
                }
            }
            "write" => {
                let (path, text) = rest.split_once(char::is_whitespace).unwrap_or((&rest, ""));
                let p = cfg.cwd.join(path);
                if let Some(parent) = p.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&p, text.trim_start().replace("\\n", "\n")) {
                    let _ = writeln!(out, "WRITE_FAILED: {e}");
                }
            }
            "commit" => {
                let ok = git_ok(cfg, &["add", "-A"])
                    && git_ok(
                        cfg,
                        &[
                            "-c",
                            "user.name=fake-agent",
                            "-c",
                            "user.email=fake@agent",
                            "commit",
                            "-m",
                            &rest,
                        ],
                    );
                if !ok {
                    let _ = out.flush();
                    return EXIT_COMMIT_FAILED;
                }
            }
            "bel" => {
                let _ = out.write_all(b"\x07");
            }
            "osc9" => {
                let _ = write!(out, "\x1b]9;{rest}\x07");
            }
            "notify" => {
                let ok = Command::new(&cfg.notify_bin)
                    .args(["--event", &rest])
                    .current_dir(&cfg.cwd)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if !ok {
                    let _ = writeln!(out, "NOTIFY_FAILED");
                }
            }
            "ctl" | "ctl_save" => {
                let (var, rest2) = if cmd == "ctl_save" {
                    let (v, r) = rest.split_once(char::is_whitespace).unwrap_or((&rest, ""));
                    (Some(v.to_string()), r.trim_start().to_string())
                } else {
                    (None, rest.clone())
                };
                let (method, json) = rest2
                    .split_once(char::is_whitespace)
                    .unwrap_or((&rest2, "{}"));
                match run_ctl(cfg, method, json.trim()) {
                    Ok(stdout) => match var {
                        None => {
                            for l in stdout.lines() {
                                let _ = writeln!(out, "CTL: {l}");
                            }
                        }
                        Some(v) => {
                            let (name, path) = match v.split_once('.') {
                                Some((n, p)) => (n.to_string(), Some(p.to_string())),
                                None => (v.clone(), None),
                            };
                            let val = match path {
                                None => Some(stdout.trim().to_string()),
                                Some(p) => serde_json::from_str::<serde_json::Value>(stdout.trim())
                                    .ok()
                                    .and_then(|j| json_path(&j, &p)),
                            };
                            match val {
                                Some(val) => {
                                    vars.insert(name, val);
                                }
                                None => {
                                    let _ = writeln!(out, "CTL_ERROR: cannot extract {v}");
                                }
                            }
                        }
                    },
                    Err(e) => {
                        let _ = writeln!(out, "CTL_ERROR: {e}");
                    }
                }
            }
            "sleep" => {
                let ms = rest.trim().parse::<u64>().unwrap_or(0);
                let _ = out.flush();
                std::thread::sleep(Duration::from_millis(ms));
            }
            "exit" => {
                let _ = out.flush();
                return rest.trim().parse::<i32>().unwrap_or(0);
            }
            other => {
                let _ = writeln!(out, "UNKNOWN_COMMAND: {other}");
            }
        }
        let _ = out.flush();
    }
    let _ = out.flush();
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "fake-agent-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn cfg(cwd: PathBuf) -> Config {
        Config {
            cwd,
            notify_bin: "/bin/echo".into(),
            tag_bin: "/bin/echo".into(),
        }
    }

    fn go(script: &str, prompt: Option<&str>, input: &str, cfg: &Config) -> (i32, String) {
        let mut out = Vec::new();
        let mut inp = Cursor::new(input.as_bytes().to_vec());
        let code = run(script, prompt, cfg, &mut inp, &mut out);
        (code, String::from_utf8_lossy(&out).into_owned())
    }

    fn shell_script(dir: &std::path::Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn print_prompt_comments_and_escape() {
        let c = cfg(tmp());
        let (code, out) = go("# c\n\nprint hi\\e[0m there\n", Some("do it"), "", &c);
        assert_eq!(code, 0);
        assert_eq!(out, "PROMPT: do it\nhi\x1b[0m there\n");
    }

    #[test]
    fn wait_input_matches_and_eof() {
        let c = cfg(tmp());
        let (code, out) = go("wait_input ^go\nprint done", None, "no\ngo now\n", &c);
        assert_eq!(code, 0);
        assert_eq!(out, "INPUT: go now\ndone\n");
        let (code, _) = go("wait_input ^go", None, "no\n", &c);
        assert_eq!(code, EXIT_EOF);
    }

    #[test]
    fn write_creates_parents() {
        let d = tmp();
        let c = cfg(d.clone());
        go("write a/b/f.txt line1\\nline2", None, "", &c);
        assert_eq!(
            std::fs::read_to_string(d.join("a/b/f.txt")).unwrap(),
            "line1\nline2"
        );
    }

    #[test]
    fn commit_works_and_fails() {
        let d = tmp();
        let c = cfg(d.clone());
        let ok = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&d)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let (code, _) = go("write f.txt x\ncommit hello world", None, "", &c);
        assert_eq!(code, 0);
        let log = Command::new("git")
            .args(["log", "--format=%an|%s"])
            .current_dir(&d)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&log.stdout).trim(),
            "fake-agent|hello world"
        );
        // Nothing to commit now -> failure.
        let (code, _) = go("commit again", None, "", &c);
        assert_eq!(code, EXIT_COMMIT_FAILED);
    }

    #[test]
    fn bel_and_osc9() {
        let c = cfg(tmp());
        let (_, out) = go("bel\nosc9 hello", None, "", &c);
        assert_eq!(out, "\x07\x1b]9;hello\x07");
    }

    #[test]
    fn notify_ok_and_failed() {
        let d = tmp();
        let c = cfg(d.clone());
        let (_, out) = go("notify done", None, "", &c);
        assert!(!out.contains("NOTIFY_FAILED"));
        let mut c2 = c.clone();
        c2.notify_bin = shell_script(&d, "bad.sh", "exit 1");
        let (_, out) = go("notify done", None, "", &c2);
        assert!(out.contains("NOTIFY_FAILED"));
        let marker = d.join("args.txt");
        c2.notify_bin = shell_script(&d, "rec.sh", &format!("echo \"$@\" > {}", marker.display()));
        go("notify agent-done", None, "", &c2);
        assert_eq!(
            std::fs::read_to_string(marker).unwrap().trim(),
            "--event agent-done"
        );
    }

    #[test]
    fn ctl_prints_and_errors() {
        let d = tmp();
        let c = cfg(d.clone());
        let (_, out) = go("ctl pane.list {\"a\":1}", None, "", &c);
        assert_eq!(out, "CTL: ctl call pane.list {\"a\":1}\n");
        let mut c2 = c.clone();
        c2.tag_bin = shell_script(&d, "fail.sh", "echo boom >&2; exit 1");
        let (_, out) = go("ctl x {}\nprint after", None, "", &c2);
        assert_eq!(out, "CTL_ERROR: boom\nafter\n");
    }

    #[test]
    fn ctl_save_and_substitution() {
        let d = tmp();
        let mut c = cfg(d.clone());
        c.tag_bin = shell_script(&d, "json.sh", "echo '{\"pane\":{\"id\":\"p7\"},\"n\":3}'");
        let script =
            "ctl_save whole x {}\nctl_save pane.pane.id pane.spawn {}\nctl_save n.n x {}\n\
                      print id=${pane} n=${n} unset=${env:FAKE_AGENT_TEST_UNSET}.\nprint ${whole}";
        let (_, out) = go(script, None, "", &c);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "id=p7 n=3 unset=.");
        assert!(lines[1].starts_with("{\"pane\""));
        let (_, out) = go("print ${env:PATH}", None, "", &c);
        assert_eq!(out.trim(), std::env::var("PATH").unwrap());
    }

    #[test]
    fn sleep_and_exit() {
        let c = cfg(tmp());
        let t = std::time::Instant::now();
        let (code, out) = go("sleep 30\nexit 7\nprint no", None, "", &c);
        assert!(t.elapsed() >= Duration::from_millis(30));
        assert_eq!(code, 7);
        assert_eq!(out, "");
    }
}
