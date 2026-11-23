//! `tag ctl`: command-line client for the TAG control plane.

use clap::Subcommand;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;
use tag_control::discovery::{discover_socket, TOKEN_ENV};
use tag_control::{codes, ClientError, ControlClient};

/// Exit code for a `wait` timeout.
const EXIT_TIMEOUT: i32 = 2;
/// Exit code when TAG is not running.
const EXIT_NOT_RUNNING: i32 = 3;

#[derive(Subcommand, Debug)]
pub(crate) enum CtlCommands {
    /// Start an orchestration run: a conductor agent plans the goal and runs workers
    Orchestrate {
        /// What the run should achieve
        goal: String,
        /// Conductor profile (default: project's default_conductor, else claude)
        #[arg(long)]
        conductor: Option<String>,
        /// Worker profiles, comma-separated
        #[arg(long, value_delimiter = ',')]
        workers: Option<Vec<String>>,
        /// Merge policy: ask or auto
        #[arg(long)]
        merge: Option<String>,
        /// Budget in USD (0 = unlimited)
        #[arg(long)]
        budget: Option<f64>,
        /// Skip plan approval
        #[arg(long)]
        yes: bool,
    },
    /// Raw JSON-RPC call: `tag ctl call <method> [json|-]`
    Call {
        method: String,
        /// JSON params (default `{}`); `-` reads from stdin
        #[arg(value_name = "JSON")]
        params: Option<String>,
    },
    /// List panes
    Ls,
    /// Print recent output of a pane
    Read {
        pane: u32,
        #[arg(long)]
        lines: Option<usize>,
        #[arg(long)]
        max_bytes: Option<usize>,
    },
    /// Spawn a pane running a profile; prints the pane id
    Spawn {
        profile: String,
        /// Create/use a worktree with this name
        #[arg(long)]
        worktree: Option<String>,
        /// Base ref for a new worktree
        #[arg(long)]
        base: Option<String>,
        #[arg(long, conflicts_with = "prompt_file")]
        prompt: Option<String>,
        #[arg(long)]
        prompt_file: Option<PathBuf>,
        /// hidden, card or pane
        #[arg(long)]
        visibility: Option<String>,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        title: Option<String>,
        /// Vault secret to inject (repeatable)
        #[arg(long = "secret")]
        secrets: Vec<String>,
    },
    /// Send text to a pane
    Send {
        pane: u32,
        text: String,
        /// Do not press Enter after the text
        #[arg(long)]
        no_submit: bool,
    },
    /// Wait for a pane's status; exit 0 if matched, 2 on timeout
    Wait {
        pane: u32,
        /// Comma-separated statuses (default: done,awaiting_input,exited,failed)
        #[arg(long, value_delimiter = ',')]
        until: Vec<String>,
        /// Timeout in seconds (default 600)
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// Show the diff of a worktree
    Diff {
        worktree: String,
        #[arg(long)]
        stat: bool,
    },
    /// Run tests in a worktree; exit code is the test exit code
    Test {
        worktree: String,
        #[arg(long)]
        command: Option<String>,
    },
    /// Merge a worktree
    Merge {
        worktree: String,
        #[arg(long)]
        no_require_tests: bool,
    },
    /// Show the status of an agent pane
    Status { pane: u32 },
    /// List runs
    Runs,
    /// Show one run
    Run { id: String },
    /// Stream state events as JSON lines until killed
    Subscribe {
        #[arg(long, value_delimiter = ',')]
        kinds: Vec<String>,
    },
}

fn s(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Null) | None => "-".into(),
        Some(o) => o.to_string(),
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

fn table(headers: &[&str], rows: Vec<Vec<String>>) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for r in &rows {
        for (i, c) in r.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let n = cells.len();
        let mut out = String::new();
        for (i, c) in cells.iter().enumerate() {
            if i + 1 == n {
                out.push_str(c);
            } else {
                out.push_str(&format!("{:<w$}  ", c, w = widths[i]));
            }
        }
        out.trim_end().to_string()
    };
    let mut out = line(headers.to_vec());
    for r in &rows {
        out.push('\n');
        out.push_str(&line(r.iter().map(String::as_str).collect()));
    }
    out
}

fn is_not_running(e: &ClientError) -> bool {
    match e {
        ClientError::NotRunning | ClientError::Connect { .. } => true,
        ClientError::Io(io) => matches!(
            io.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
        ),
        _ => false,
    }
}

/// Run a `tag ctl` command; returns the process exit code.
pub(crate) fn run_ctl(
    cmd: CtlCommands,
    json: bool,
    socket: Option<PathBuf>,
) -> anyhow::Result<i32> {
    match run_inner(cmd, json, socket) {
        Ok(code) => Ok(code),
        Err(e) => {
            if let Some(ce) = e.control() {
                eprintln!("error: {} (code {})", ce.message, ce.code);
                return Ok(if ce.code == codes::TIMEOUT {
                    EXIT_TIMEOUT
                } else {
                    1
                });
            }
            eprintln!("error: {e}");
            Ok(if is_not_running(&e) {
                EXIT_NOT_RUNNING
            } else {
                1
            })
        }
    }
}

fn connect(socket: Option<PathBuf>) -> Result<ControlClient, ClientError> {
    let path = socket
        .or_else(discover_socket)
        .ok_or(ClientError::NotRunning)?;
    let token = std::env::var(TOKEN_ENV).ok().filter(|t| !t.is_empty());
    ControlClient::connect(&path, "ctl", token)
}

fn emit(json: bool, v: &Value, human: impl FnOnce(&Value) -> String) {
    if json {
        println!("{}", pretty(v));
    } else {
        let text = human(v);
        if text.ends_with('\n') {
            print!("{text}");
        } else {
            println!("{text}");
        }
    }
    let _ = std::io::stdout().flush();
}

fn run_inner(cmd: CtlCommands, json: bool, socket: Option<PathBuf>) -> Result<i32, ClientError> {
    let mut c = connect(socket)?;
    match cmd {
        CtlCommands::Orchestrate {
            goal,
            conductor,
            workers,
            merge,
            budget,
            yes,
        } => {
            let mut params = json!({
                "goal": goal,
                "conductor_profile": conductor,
                "worker_profiles": workers,
                "budget": budget,
                "spawn_conductor": true,
            });
            if let Some(m) = merge {
                params["merge_policy"] = json!(m);
            }
            if yes {
                params["require_plan_approval"] = json!(false);
            }
            let v = c.call_raw("run.create", params)?;
            emit(json, &v, |v| {
                let run = &v["run"];
                format!(
                    "Started run {} (conductor pane {}, merge policy {}).\n{}Follow it with `tag ctl run {}`.\n",
                    s(run, "id"),
                    run["conductor"]["pane"],
                    s(run, "merge_policy"),
                    if run["require_plan_approval"] == true {
                        format!(
                            "The plan needs your approval: `tag ctl call run.approve_plan '{{\"run\":\"{}\"}}'` (or the attention queue, ^Space a).\n",
                            s(run, "id")
                        )
                    } else {
                        String::new()
                    },
                    s(run, "id")
                )
            });
        }
        CtlCommands::Call { method, params: p } => {
            let raw = match p.as_deref() {
                None => "{}".to_string(),
                Some("-") => {
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf)?;
                    buf
                }
                Some(x) => x.to_string(),
            };
            let params: Value = if raw.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&raw)?
            };
            let v = c.call_raw(&method, params)?;
            println!("{}", pretty(&v));
        }
        CtlCommands::Ls => {
            let v = c.call_raw("panes.list", json!({}))?;
            emit(json, &v, |v| {
                let rows = v
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|p| {
                                vec![
                                    s(p, "id"),
                                    s(p, "kind"),
                                    s(p, "profile"),
                                    s(p, "status"),
                                    s(p, "visibility"),
                                    s(p, "title"),
                                    s(p, "cwd"),
                                ]
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                table(
                    &[
                        "ID",
                        "KIND",
                        "PROFILE",
                        "STATUS",
                        "VISIBILITY",
                        "TITLE",
                        "CWD",
                    ],
                    rows,
                )
            });
        }
        CtlCommands::Read {
            pane,
            lines,
            max_bytes,
        } => {
            let mut p = json!({ "pane": pane });
            if let Some(l) = lines {
                p["lines"] = json!(l);
            }
            if let Some(m) = max_bytes {
                p["max_bytes"] = json!(m);
            }
            let v = c.call_raw("pane.read", p)?;
            emit(json, &v, |v| {
                v.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            });
        }
        CtlCommands::Spawn {
            profile,
            worktree,
            base,
            prompt,
            prompt_file,
            visibility,
            cwd,
            title,
            secrets,
        } => {
            let prompt = match prompt_file {
                Some(f) => Some(std::fs::read_to_string(f)?),
                None => prompt,
            };
            let mut p = json!({ "profile": profile });
            if let Some(name) = worktree {
                p["worktree"] = json!({ "create": true, "name": name, "base": base });
            }
            if let Some(x) = prompt {
                p["prompt"] = json!(x);
            }
            if let Some(x) = visibility {
                p["visibility"] = json!(x);
            }
            if let Some(x) = cwd {
                p["cwd"] = json!(x);
            }
            if let Some(x) = title {
                p["title"] = json!(x);
            }
            if !secrets.is_empty() {
                p["secrets"] = json!(secrets);
            }
            let v = c.call_raw("pane.spawn", p)?;
            emit(json, &v, |v| s(v, "pane"));
        }
        CtlCommands::Send {
            pane,
            text,
            no_submit,
        } => {
            let v = c.call_raw(
                "pane.send",
                json!({ "pane": pane, "text": text, "submit": !no_submit }),
            )?;
            if json {
                println!("{}", pretty(&v));
            }
        }
        CtlCommands::Wait {
            pane,
            until,
            timeout,
        } => {
            let mut p = json!({ "pane": pane });
            if !until.is_empty() {
                p["until"] = json!(until);
            }
            if let Some(t) = timeout {
                p["timeout_s"] = json!(t);
            }
            let v = c.call_raw("agent.wait", p)?;
            emit(json, &v, |v| s(v, "status"));
        }
        CtlCommands::Diff { worktree, stat } => {
            let v = c.call_raw(
                "worktree.diff",
                json!({ "worktree": worktree, "stat_only": stat }),
            )?;
            emit(json, &v, |v| {
                let key = if stat { "stat" } else { "diff" };
                let mut out = v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
                if v.get("truncated").and_then(Value::as_bool) == Some(true) {
                    out.push_str("\n[truncated]");
                }
                out
            });
        }
        CtlCommands::Test { worktree, command } => {
            let mut p = json!({ "worktree": worktree });
            if let Some(cmd) = command {
                p["command"] = json!(cmd);
            }
            let v = c.call_raw("tests.run", p)?;
            emit(json, &v, |v| {
                format!(
                    "{}\nexit code {} in {:.1}s (commit {})",
                    s(v, "tail").trim_end(),
                    s(v, "exit_code"),
                    v.get("duration_s").and_then(Value::as_f64).unwrap_or(0.0),
                    s(v, "commit")
                )
            });
            let code = v.get("exit_code").and_then(Value::as_i64).unwrap_or(1);
            return Ok(code.clamp(0, 255) as i32);
        }
        CtlCommands::Merge {
            worktree,
            no_require_tests,
        } => {
            let v = c.call_raw(
                "worktree.merge",
                json!({ "worktree": worktree, "require_tests": !no_require_tests }),
            )?;
            emit(json, &v, |v| {
                if v.get("queued").and_then(Value::as_bool) == Some(true) {
                    "queued for approval".to_string()
                } else {
                    format!("merged {}", s(v, "merged_commit"))
                }
            });
        }
        CtlCommands::Status { pane } => {
            let v = c.call_raw("agent.status", json!({ "pane": pane }))?;
            emit(json, &v, |v| {
                format!(
                    "status: {}\nsince: {}\nlast_hook: {}\ntokens: {}",
                    s(v, "status"),
                    s(v, "since"),
                    s(v, "last_hook"),
                    s(v, "tokens")
                )
            });
        }
        CtlCommands::Runs => {
            let v = c.call_raw("run.list", json!({}))?;
            emit(json, &v, |v| {
                let rows = v
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|r| vec![s(r, "id"), s(r, "state"), s(r, "goal")])
                            .collect()
                    })
                    .unwrap_or_default();
                table(&["ID", "STATE", "GOAL"], rows)
            });
        }
        CtlCommands::Run { id } => {
            let v = c.call_raw("run.get", json!({ "run": id }))?;
            println!("{}", pretty(&v));
        }
        CtlCommands::Subscribe { kinds } => {
            let p = if kinds.is_empty() {
                json!({})
            } else {
                json!({ "kinds": kinds })
            };
            c.call_raw("events.subscribe", p)?;
            loop {
                if let Some(e) = c.next_event(Some(Duration::from_secs(3600)))? {
                    println!("{}", serde_json::to_string(&e)?);
                    let _ = std::io::stdout().flush();
                }
            }
        }
    }
    Ok(0)
}
