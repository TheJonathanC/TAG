# ADR 0006: Model-Agnostic Orchestration Runs

## Context
TAG should not only display coding agents but run them: plan a goal, split it into tasks, run each
task in its own agent and git worktree, review, test and merge. Users mix models (Claude, Codex, agy
with Gemini 4 Neon) and want any of them to conduct or work. Embedding model APIs or model-specific
logic in TAG would tie it to vendors and make prompts code.

## Decision
1. **TAG supplies tools; a model orchestrates (D2).** A *conductor* is an ordinary agent pane started
   with a scoped control token and a prompt rendered from `~/.config/tag/prompts/conductor.md`
   (MCP-capable profiles, via a per-run `mcp.json`) or `conductor_cli.md` (agents without a
   per-session MCP flag, such as agy, which use `tag ctl`). Workers are agent panes started from
   `worker.md`. Templates are plain Markdown with `{{var}}` placeholders, installed on first run and
   never overwritten.
2. **Every agent runs in a PTY (D1).** "Headless" is a visibility (`hidden`/`card`/`pane`), not a
   process type; any worker can be peeked at or promoted without a restart.
3. **State machine in a pure crate.** `tag-orchestrator` holds runs, tasks, transition validation,
   atomic persistence (`run.json` via temp file + rename, append-only `events.jsonl`) and a pure
   scheduler `next_actions(run, limits)`. The app loop carries out the actions on every Tick.
4. **TAG drives bookkeeping; the conductor decides.** TAG creates worktrees
   (`.worktrees/<run>-<task>`, branch `tag/<run>/<task>`), mints worker tokens, spawns workers when
   dependencies are merged and concurrency/budget allow, maps worker status and `task.report` to
   task states, and types `[TAG] …` lines into the conductor's pane (plan approved, task ready for
   review or blocked, merged, run done). This works with any model, with or without event
   subscriptions.
5. **Human gates by default.** `require_plan_approval = true` and `merge_policy = "ask"` are the
   built-in defaults; merges of protected paths always need the human. Approvals surface as
   attention items.
6. **Budgets** are tracked per run from usage events whose session id TAG chose at launch; a run
   pauses at `budget_usd` (`hard_budget` also stops its workers).

## Consequences
- Adding a model is a config change (`[agents.<name>]` with `prompt_delivery`, `prompt_args`,
  `resume_args`, `session_id_args`, `mcp_config_args`, `done_signal`); no Rust changes.
- Completion detection for agents without hooks relies on OSC 9/BEL and idle timeouts, so
  `idle_done_secs` needs tuning per agent.
- Runs survive a TAG restart as records, but worker processes don't: active tasks become `failed`
  with reason `lost_on_restart` and can be retried with `task.update {"state": "queued"}`.
- End-to-end tests use `tools/fake-agent` as both conductor and worker, so CI covers the full flow
  without any model.
