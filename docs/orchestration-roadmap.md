# TAG Orchestration Roadmap: Implementation Plan for Coding Agents

Status: draft v1 (2026-11-23). Owner: Jonathan.
Scope: turn TAG from a multiplexer that *shows* agents into an orchestrator that *runs* them. That means a control plane, an MCP server, multi-agent runs, per-project sessions, a multi-agent UI, and a project "brain".

Progress against this plan is tracked in `docs/orchestration-progress.md`.

---

## 0. How to use this document (read first)

This plan is written to be executed by coding agents (Claude Code, Codex, agy) one task at a time.

1. **Pick one task** from §9 whose dependencies are all merged. Do not start a task whose `depends` are still open.
2. **Read before you write:** `AGENTS.md` (release rules), `docs/design.md`, this file's §2–§8 (decisions, contracts, data models), and the files listed in the task.
3. **One task = one branch = one PR.** Branch name: `feat/<task-id>-<slug>` (e.g. `feat/T1.1-control-protocol`).
4. **Stay in scope.** Don't refactor or "improve" code outside the task. If you find a bug elsewhere, note it in the PR description.
5. **Contracts in §4–§6 are binding.** If a contract turns out to be wrong or impossible, don't silently change it. Make the smallest workable choice, and add a `## Spec deviation` section to the PR explaining it.
6. **Quality gate before opening the PR:**
   `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
7. **Never** bump to or tag a stable version, and never publish to crates.io (see `AGENTS.md` §1).
8. **Docs move with code.** If you add a key binding, CLI command, config key or control method, update `README.md`, `docs/SHORTCUTS.md`, and the reference tables in this file in the same PR.
9. **Verify external CLI flags; don't guess them.** Anything about `claude`, `codex` or `agy` flags marked **[verify]** must be checked against the installed CLI's `--help`. Record what you found in the PR.

---

## 1. Goals and non-goals

### Goals
- **G1. Orchestration.** A *conductor* agent plans work, spawns *worker* agents in isolated git worktrees, monitors them, reviews and tests their output, and merges it.
- **G2. Model-agnostic.** Any profile can be the conductor or a worker: Claude conducting agy (Gemini 4 Neon) workers, agy conducting agy, Codex conducting Claude, and so on. TAG contains **no model-specific orchestration logic**.
- **G3. Control plane.** Everything a human can do to panes, agents, worktrees and runs is available through a local API, a CLI (`tag ctl`) and an MCP server (`tag mcp`). External assistants, including Claude through the desktop app, can drive TAG and read its state.
- **G4. Multi-agent UX.** Many agents stay readable: hidden/card/pane visibility, a Workers table, a Run view, peek overlays, and better attention handling.
- **G5. Projects.** A project layer above workspaces, with per-project sessions, agent conversation resume, and fast project/workspace switching.
- **G6. Brain.** Per-project context (brief, conventions, decisions) plus a cross-project pattern library that agents can search and reuse, with a graph view.

### Non-goals (for this roadmap)
- No hosted/cloud service and no multi-user collaboration.
- No embedding of model APIs inside TAG. Agents are always external CLIs.
- No Windows support work (current targets: Linux and macOS).
- No remote HTTP MCP transport by default (§9 P6 makes it optional and feature-gated).

---

## 2. Fixed architectural decisions

These are decided. Don't relitigate them inside a task PR.

| # | Decision | Rationale |
|---|---|---|
| D1 | **Every agent runs in a PTY.** "Headless" is a *visibility* state, not a different process type. | `agy -p` is known to print nothing when stdout isn't a TTY. TAG's PTY engine sidesteps that, and keeps status detection, transcripts and usage uniform. Any hidden worker can be promoted to a pane at any time. |
| D2 | **TAG supplies the tools; the model does the orchestrating.** The conductor is an ordinary agent pane given control tools plus a prompt template. | Model-agnostic (G2). Prompts are editable files, not Rust code. |
| D3 | **New control socket, separate from the hook socket.** `control.sock` lives next to the existing hook socket and speaks newline-delimited JSON-RPC 2.0 (request/response plus server notifications). The hook socket and `cockpit-notify` stay unchanged. | The hook socket is fire-and-forget (one payload, then close), so it can't carry responses. Keeping it untouched avoids breaking installed hooks. |
| D4 | **One control protocol, three front-ends:** the socket (native), `tag ctl` (CLI), and `tag mcp` (stdio MCP server). The CLI and MCP are thin clients of the socket. | One implementation of semantics and permissions. |
| D5 | **Permission tiers per client:** `read`, `act`, `dangerous`. Each client identity has an allowed tier set. Every `act`/`dangerous` call is audit-logged. | Lets an external assistant read and act but keeps merges with the human. Workers can't spawn workers. |
| D6 | **Scoped tokens for agents TAG launches.** The conductor and workers get `TAG_CONTROL_TOKEN` in their environment, bound to a role and a run ID. | A worker can only touch its own pane, worktree and task. A conductor can only manage its own run. |
| D7 | **Vault secrets never leave over the control plane.** At most, `pane.spawn` can name secrets to *inject into the child env*. Values are never returned. | The security boundary from `docs/SECURITY.md`. |
| D8 | **A headless daemon mode exists** (`tag daemon`): same event loop and state, no terminal rendering. The UI can attach to it. | Needed for CI end-to-end tests, for MCP use while no UI is open, and for clean background mode. |
| D9 | **State lives outside the repo; config and brain live inside it.** `.tag/project.toml` and `.tag/brain/` live in the repo and are committable. Sessions, runs and indexes live in the state dir. | Avoids repo pollution while letting teams share config and brain notes. |
| D10 | **The brain is Markdown + a SQLite index.** Notes are `.md` files with YAML frontmatter and `[[wikilinks]]`. SQLite (FTS5) is a rebuildable cache. | Human-readable and editable, works with Obsidian's graph view for free, and the index can always be regenerated. |
| D11 | **Compact outputs over the control plane.** Pane reads return ANSI-stripped tails with byte caps. Diffs are capped, with a `truncated` flag. | Protects the context windows of the agents calling TAG. |

### Paths

| What | Path |
|---|---|
| Global config | `~/.config/tag/config.toml` (existing) |
| Prompt templates | `~/.config/tag/prompts/*.md` (installed from `assets/prompts/` on first run, never overwritten) |
| Global brain | `~/.config/tag/brain/` |
| State dir | `$XDG_STATE_HOME/tag` or `~/.local/state/tag` (macOS: `~/Library/Application Support/tag/state`) |
| Project registry | `<state>/projects.json` |
| Per-project session | `<state>/projects/<project_id>/session.json` |
| Runs | `<state>/projects/<project_id>/runs/<run_id>/{run.json,events.jsonl,logs/}` |
| Brain index | `<state>/brain.sqlite` |
| Control audit log | `<state>/control-audit.jsonl` |
| In-repo project config | `<repo>/.tag/project.toml` |
| In-repo brain | `<repo>/.tag/brain/` |
| Worker reference imports | `<worktree>/.tag/ref/<pattern-slug>/` (git-excluded automatically) |

`project_id` is the first 16 hex characters of SHA-256 of the canonical repo root path.

---

## 3. Current codebase facts (as of `6403cc2`; re-verified on `origin/main` at the start of the run)

Agents should rely on these and re-check them if a task touches them.

- `crates/bin/src/main.rs` is ~4,350 lines and holds `AppState`, `run_app`, the event dispatch (`handle_app_event_inner`), `execute_action`, rendering, key encoding, git overlay handling, session save/load, socket path helpers and the CLI (`clap`). It has to be split before major work (T0.1).
- `AppEvent` (`crates/core/src/event.rs`) is `#[derive(Debug, Clone)]`. Control requests need a reply channel, and `oneshot::Sender` isn't `Clone`, so wrap it (see §4.4).
- `HookServer` (`crates/agents/src/hook_server.rs`) binds a Unix socket with mode `0600`, checks the peer UID, reads one JSON `HookPayload` (1 MB cap, 5 s timeout) and forwards it as `AppEvent::AgentHook`. Reuse this hardening pattern for the control socket.
- `AgentStatus` = `Idle | Running | AwaitingInput | Done | Exited`.
- `AgentProfileConfig` (`crates/core/src/config.rs`) has `name, command, args, env, default_cwd, hook_type, status_patterns, prompt_snippets`. Config is TOML at `~/.config/tag/config.toml`, and parse errors are shown and never overwritten.
- `GitBackend` trait (`crates/git/src/backend.rs`) already has `repo_status, list_worktrees, create_worktree, delete_worktree, get_diff, merge_branch, abort_merge, stage_file, commit`. Worktrees are created in `<repo>/.worktrees/`.
- `SavedSession` (`crates/layout/src/session.rs`) saves workspaces (layout tree, focus, zoom, minimized panes, worktree binding) and panes (`title, kind, cwd, command, args`) to a **single global** `session.json`.
- `PaneKind` = `Shell | Agent | Tmux | Applet | Donut`. Applets (`crates/applets`) are non-PTY panes, which is the model for new UI panes like the Run view.
- The usage crate already parses Claude Code, Codex and agy transcripts into SQLite (`rusqlite`, bundled).
- agy facts: transcripts are under `~/.gemini/antigravity-cli/brain/`. Status comes from OSC 9 / BEL plus transcripts, and there's no native hook yet. Headless mode is `agy -p "<prompt>"` with `--output-format text|json|stream-json`. `--model <slug>` selects the model (`agy models` lists the slugs). **[verify] the exact slug for Gemini 4 Neon, and agy's MCP config mechanism.**

---

## 4. Control plane contract

### 4.1 Transport
- Unix domain socket at `<socket_dir>/control.sock` (same dir logic as the hook socket: `preferred_socket_dir()` / short fallback). Mode `0600`, peer-UID check.
- Framing: one JSON object per line (`\n`-terminated), JSON-RPC 2.0. Max request line 1 MB. Idle connections close after 10 minutes. Long-lived connections are allowed (for subscriptions).
- Discovery: clients use `$TAG_CONTROL_SOCKET` if set; otherwise the default path; otherwise the newest live socket (mirror `find_latest_tag_socket`).

### 4.2 Handshake
The first call on every connection must be `tag.hello`:

```json
{"jsonrpc":"2.0","id":1,"method":"tag.hello","params":{"client":"claude-desktop","token":null,"protocol":1}}
```
Result:
```json
{"protocol":1,"tag_version":"0.2.0-beta.1","client":"claude-desktop","role":"external","tiers":["read","act"],"run_scope":null}
```
- With a `token` (from `TAG_CONTROL_TOKEN`), the role and scope come from the token. Without one, the role is `external` and tiers come from `[control.clients.<client>]` in config. An unknown client gets `["read"]`.
- Any other first call returns error `-32001 NotAuthenticated`.

### 4.3 Errors
Standard JSON-RPC codes, plus:

| Code | Name | Meaning |
|---|---|---|
| -32001 | NotAuthenticated | missing/invalid hello |
| -32002 | Forbidden | tier or scope not allowed |
| -32003 | NotFound | pane/run/task/worktree/project unknown |
| -32004 | Conflict | e.g. merge conflict; `data.files` lists them |
| -32005 | Busy | concurrency/budget limit reached |
| -32006 | Timeout | wait exceeded |
| -32007 | PolicyBlocked | e.g. merge requires approval or passing tests |

### 4.4 Server internals (binding)
- The control server runs on tokio tasks and must never block the UI loop.
- Requests go to the loop as `AppEvent::Control(ControlEnvelope)`, where `ControlEnvelope { client: ClientCtx, request: ControlRequest, reply: ReplyHandle }` and `ReplyHandle(Arc<Mutex<Option<oneshot::Sender<ControlResult>>>>)`, implementing `Clone` and a no-op `Debug`.
- The loop handles each request with a short critical section and replies. Anything slow (diffs, tests, merges) is spawned as background work that replies when done. The existing `drain_background_results` pattern is the model.
- The loop publishes state changes on a `tokio::sync::broadcast::Sender<StateEvent>` (`PaneStatusChanged`, `PaneExited`, `TaskStateChanged`, `RunStateChanged`, `AttentionAdded`). `agent.wait` and `events.subscribe` are implemented **in the server task** on top of this broadcast, not in the loop.

### 4.5 Methods

Tier markers: **R** = read, **A** = act, **D** = dangerous. "Scope" says what a scoped token may target: `worker` = own pane/worktree/task only, `conductor` = anything in its run.

| Method | Tier | Params → Result (abridged) |
|---|---|---|
| `tag.hello` | — | see §4.2 |
| `projects.list` | R | → `[{id,name,root,active,workspaces:n,agents:n}]` |
| `workspaces.list` | R | `{project?}` → `[{id,name,panes:[pane_id],active}]` |
| `panes.list` | R | `{project?,workspace?,kind?}` → `[{id,title,kind,profile?,status?,visibility,cwd,worktree?,run?,task?}]` |
| `pane.read` | R | `{pane, lines=80, max_bytes=16384, strip_ansi=true}` → `{text,truncated,status}` |
| `agent.status` | R | `{pane}` → `{status,since,last_hook?,tokens?}` |
| `usage.summary` | R | `{project?,run?,since?}` → `{input,output,cached,cost_usd,by_profile:{}}` |
| `worktree.list` | R | `{project?}` → `[{name,path,branch,is_main,task?}]` |
| `worktree.diff` | R | `{worktree, max_bytes=65536, stat_only=false}` → `{stat,diff,truncated}` |
| `run.list` / `run.get` | R | → run objects (§5) |
| `brain.search` | R | `{query, project?, kind?, limit=10}` → `[{id,kind,title,project,snippet,score}]` |
| `brain.get` | R | `{id}` or `{project, kind:"brief"}` → `{frontmatter,body,links}` |
| `brain.get_pattern` | R | `{id, include_files=true, max_bytes=65536}` → `{pattern, files:[{path,content}]}` |
| `events.subscribe` | R | `{kinds?:[...], run?}` → server sends `{"method":"event","params":{...}}` notifications |
| `pane.spawn` | A | `{profile, project?, workspace?, cwd?, worktree?:{create:true,name,base?}, prompt?, visibility="card", secrets?:[name], title?, run?, task?}` → `{pane, worktree?}` |
| `pane.send` | A | `{pane, text, submit=true}`: bracketed paste, then Enter if `submit` → `{}` |
| `pane.set_visibility` | A | `{pane, visibility:"hidden"\|"card"\|"pane"}` |
| `agent.wait` | A | `{pane, until:["done","awaiting_input","exited","failed"], timeout_s=600}` → `{status, reason}` |
| `worktree.create` | A | `{project?, name, base?}` → `{path, branch}` |
| `tests.run` | A | `{worktree, command?}` (default from `project.toml`) → `{exit_code, duration_s, tail, commit}` |
| `run.create` | A | `{goal, conductor_profile?, worker_profiles?, merge_policy?, budget?}` → `{run}` |
| `task.add` / `task.update` | A | conductor only; see §5 |
| `brain.note` | A | `{project, title, body, kind="note", links?}` → `{id}` |
| `brain.import_pattern` | A | `{id, worktree}` → `{path}` (writes into `.tag/ref/<slug>/`) |
| `run.approve_plan` | D | `{run}` |
| `worktree.merge` | D | `{worktree \| task, into="main", require_tests=true}` → `{merged_commit}` or `Conflict` / `PolicyBlocked` |
| `worktree.remove` | D | `{worktree, force=false}` |
| `pane.kill` | D | `{pane}` |
| `run.cancel` | D | `{run, kill_workers=true}` |

Default scoped-token rights:
- **conductor:** all R; A within its run (spawning only for its own tasks, up to `max_workers`); `worktree.merge` for its run's tasks **only if** the run's `merge_policy = "auto"`; otherwise merges become attention items for the human.
- **worker:** `pane.read`/`agent.status` on itself; `brain.search/get/get_pattern`; `tests.run` and `worktree.diff` on its own worktree; `task.report` (A, worker-only: `{summary, status:"done"|"blocked", notes?}`).
- **external (no token):** from config; default `["read"]`.

### 4.6 Output limits (D11)
- `pane.read`: strip ANSI and collapse carriage-return progress lines. Apply the byte cap from the end, and set `truncated` if anything was cut.
- `worktree.diff`: if it's over `max_bytes`, return the full `--stat` plus whole-file hunks until the cap, and `truncated:true`.
- MCP tool results are text. Pretty-print JSON results with a 2-space indent and apply the same caps.

---

## 5. Data models

### 5.1 Profiles (config extension, backward compatible)

```toml
[agents.claude]
name = "Claude Code"
command = "claude"
hook_type = "claude-code"
roles = ["conductor", "worker", "reviewer"]
prompt_delivery = "type"            # "type" | "arg" | "stdin"
prompt_args = []                     # used when prompt_delivery="arg", "{prompt}" is substituted
resume_args = ["--resume", "{session_id}"]   # [verify]
mcp_config_args = ["--mcp-config", "{mcp_config_path}"]   # [verify]
done_signal = ["hook"]               # any of "hook" | "osc" | "idle" | "regex" | "exit"
idle_done_secs = 0                   # 0 = disabled
max_concurrent = 4

[agents.agy-neon]
name = "agy · Gemini 4 Neon"
command = "agy"
args = ["--model", "<neon-slug>"]   # [verify] via `agy models`
roles = ["conductor", "worker"]
prompt_delivery = "arg"
prompt_args = ["-i", "{prompt}"]    # interactive with opening prompt; [verify]
done_signal = ["osc", "idle"]
idle_done_secs = 45
max_concurrent = 6
```

New `AgentProfileConfig` fields (all `#[serde(default)]`): `model: Option<String>` (sugar that appends the profile's model flag), `roles: Vec<Role>`, `prompt_delivery`, `prompt_args`, `resume_args`, `mcp_config_args`, `done_signal: Vec<DoneSignal>`, `idle_done_secs: u64`, `max_concurrent: usize`.

For `prompt_delivery = "type"`, wait until the agent is ready (first `Idle`/`AwaitingInput` status, or 3 s with no output), then use bracketed paste plus Enter.

### 5.2 Project config (`<repo>/.tag/project.toml`)

```toml
name = "tag"
test_command = "cargo test --workspace"
lint_command = "cargo clippy --workspace --all-targets -- -D warnings"
default_conductor = "claude"
default_workers = ["agy-neon"]
merge_policy = "ask"        # "ask" | "auto"; overrides global
require_plan_approval = true
max_workers = 4
worker_visibility = "card"  # "hidden" | "card" | "pane"
budget_usd = 5.0            # per run; 0 = unlimited
protected_paths = [".github/", "AGENTS.md"]   # workers' diffs touching these always need human approval
```

Global defaults live under `[orchestration]` in `config.toml` with the same keys. Precedence: run params > project.toml > global > built-in.

### 5.3 Run and Task (`run.json`)

```json
{
  "id": "r_20261123_0915_ab12",
  "project": "a1b2c3d4e5f60718",
  "goal": "Add OAuth login",
  "state": "planning",
  "conductor": {"profile": "claude", "pane": 12},
  "merge_policy": "ask",
  "budget_usd": 5.0,
  "created_at": "2026-11-23T09:15:00Z",
  "tasks": [
    {
      "id": "t1",
      "title": "API routes",
      "prompt": "…full worker instructions…",
      "acceptance": ["routes return 200 for valid tokens", "cargo test passes"],
      "depends_on": [],
      "profile": "agy-neon",
      "state": "working",
      "worktree": ".worktrees/r_ab12-t1",
      "branch": "tag/r_ab12/t1",
      "pane": 14,
      "attempts": 1,
      "last_test": {"commit": "abc123", "exit_code": 0, "at": "…"},
      "report": null
    }
  ]
}
```

**Run states:** `planning → awaiting_approval → running → reviewing → merging → done`, plus `paused` (budget hit or user pause), `failed`, `cancelled`.

**Task states:** `planned → queued → working → (needs_input) → review → changes_requested → working … → approved → merged`, plus `failed`, `skipped`.

Valid transitions are enforced in `tag-orchestrator`. Every transition appends a line to `events.jsonl` and is published as a `StateEvent`.

Scheduler rule: a task goes `queued → working` only when all `depends_on` tasks are `merged` (or `approved` when the conductor marks them `parallel_ok`), the run is `running`, and the concurrency and budget limits allow it.

### 5.4 Visibility

`Visibility = Hidden | Card | Pane`.
- **Hidden:** the PTY runs and isn't in any layout tree. It's sized `120x40` until shown.
- **Card:** it appears in the workspace's card strip (T4.4) and in the Workers table.
- **Pane:** a normal tiled pane.

Changing visibility never restarts the process. The PTY is resized when it enters a layout.

### 5.5 Projects

`Project { id, name, root, last_opened, pinned }` is stored in `projects.json`. The active project is resolved from the focused pane's cwd git root, falling back to the last active project. The legacy global `session.json` migrates once into a project named `default`, keyed by `$HOME`.

### 5.6 Brain files

```markdown
---
id: pat_jwt-middleware
kind: pattern            # brief | note | decision | pattern
title: JWT auth middleware (FastAPI)
project: 9f8e7d6c5b4a3210
tags: [auth, fastapi, jwt]
source: {paths: ["backend/auth/jwt.py", "backend/deps.py"], commit: "4e1f2a9"}
created: 2026-11-23
---
Short description of what it does, how to adapt it, and gotchas.
Related: [[pat_refresh-tokens]], [[dec_session-storage]]
```

Layout: `<repo>/.tag/brain/{brief.md, notes/, decisions/, patterns/}` and `~/.config/tag/brain/{patterns/, notes/}` for global entries. IDs are unique across the whole brain, and `[[id]]` links resolve globally.

---

## 6. Prompt templates (`assets/prompts/`)

Plain Markdown with `{{var}}` placeholders. They're installed to `~/.config/tag/prompts/` on first run and never overwritten (add new files, keep user edits). Variables: `{{goal}}`, `{{run_id}}`, `{{project_name}}`, `{{brief}}`, `{{workers}}`, `{{merge_policy}}`, `{{max_workers}}`, `{{test_command}}`, `{{task}}`, `{{acceptance}}`, `{{worktree}}`.

See `assets/prompts/conductor.md`, `worker.md` and `reviewer.md` for the shipped templates.

---

## 7. UX specification

### 7.1 Key bindings (proposed; check conflicts with `keys` crate conflict detection)

| Keys | Action |
|---|---|
| `^Space P` | Project switcher (tree: projects → workspaces → panes, with live preview) |
| `^Space Tab` | Last workspace |
| `^Space !` | Jump to the next agent needing input, across all projects |
| `^Space W` | Workers table |
| `^Space R` | Run view for the active run (picker if there are several) |
| `^Space o` | Orchestrate: prompt for a goal, start a run with project defaults |
| `^Space e` | Peek the selected card/worker (floating) |

In the Workers table and Run view: `Enter` open as pane · `e` peek · `d` diff · `t` run tests · `r` reply (send text) · `h` hide · `k` kill (confirm) · `m` merge (confirm, policy-aware) · `a` approve plan · `/` filter · `f` cycle filter (all / needs me / running / done).

### 7.2 Workers table (k9s-style overlay)
Columns: `●` status color, name, profile·model, project, run/task, state, elapsed, tokens, last output line (truncated). It refreshes from `StateEvent` and doesn't poll. It's sorted with needs-input first, then running, then the rest.

### 7.3 Run view (lazygit-style, a non-PTY pane: `PaneKind::RunView`)
Left: task tree with state glyphs (`○ planned`, `◔ queued`, `● working`, `?` needs input, `◐ review`, `✓ merged`, `✗ failed`). Right: live tail of the selected task's pane. The bottom panel toggles between diff stat and last test output with `Tab`.

### 7.4 Card strip
A one-row strip above the status bar in a workspace with card workers: `[● t1 agy·neon 4m] [? t2 needs input] [✓ t3]`. Click or `Enter` promotes a card to a pane, and `e` peeks it.

### 7.5 Peek
A centered floating overlay (~80% of the screen) that renders the worker's `VtGrid` live. `i` toggles input forwarding (off by default), `Esc` closes, `Enter` promotes it to a pane.

### 7.6 Attention queue
Group items by run. Quick actions inline: approve plan, approve merge, reply. Each item shows project and task.

---

## 8. Testing strategy

- **Fake agent (T0.3) is mandatory infrastructure.** `tools/fake-agent` is a small Rust binary, a workspace member with `publish = false`. Every orchestration feature gets an end-to-end test using it, so there are no real models in CI.
- **Daemon mode (T1.0)** lets tests start TAG without a terminal: `tag daemon --socket-dir <tmp> --state-dir <tmp>`.
- **Protocol tests:** golden JSON request/response fixtures per method in `crates/control/tests/fixtures/`.
- **Permission tests:** for each tier and role, a table-driven test that forbidden calls return `-32002`.
- **Git tests:** temp repos.
- **UI:** unit-test state reducers and layout math. Render tests with ratatui's `TestBackend`.
- **CI:** existing workflow. E2E tests must finish in under 60 s total and must not need network access.

---

## 9. Task breakdown

| Phase | Tasks |
|---|---|
| P0 Foundations | T0.1 split `main.rs` · T0.2 StateEvent broadcast · T0.3 fake agent |
| P1 Control plane | T1.0 daemon · T1.1 `tag-control` crate · T1.2 server/auth/tiers · T1.3 read methods · T1.4 act methods · T1.5 dangerous methods · T1.6 `tag ctl` · T1.7 `tag mcp` · T1.8 ADR 0005 + docs |
| P2 Orchestration | T2.1 profile extensions · T2.2 visibility · T2.3 completion detection · T2.4 `tag-orchestrator` · T2.5 run methods + wiring · T2.6 prompts + `tag orchestrate` · T2.7 budgets |
| P3 Projects | T3.1 registry · T3.2 per-project sessions · T3.3 resume · T3.4 project switcher · T3.5 navigation keys |
| P4 Multi-agent UI | T4.1 workers table · T4.2 run view · T4.3 peek · T4.4 card strip · T4.5 attention grouping · T4.6 run auto-layout |
| P5 Brain | T5.1 `tag-brain` · T5.2 `tag brain init` · T5.3 `tag brain pin/ls/show/rm/search` · T5.4 brain methods · T5.5 context injection · T5.6 brain views + graph · T5.7 post-run notes |
| P6 Hardening | T6.1 HTTP MCP · T6.2 security review · T6.3 ADRs + beta bump |

Full task text (acceptance criteria, dependencies, waves, risks, definition of done) is kept in the original plan handed to the implementing agents; `docs/orchestration-progress.md` records status and deviations per task.
