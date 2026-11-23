# Orchestration roadmap: progress log

Branch: `integration/orchestration` (all tasks merged here; one PR to `main` at the end).
Plan: `docs/orchestration-roadmap.md`. Based on `origin/main` (`6403cc2`).

| Task | Status | Commit | Notes |
|---|---|---|---|
| T0.1 split main.rs | done | f8e30fa | main.rs 194 lines; app/events.rs 1316 (handle_app_event_inner is one fn, left intact) |
| T0.3 fake agent | done | ce65743 | `tools/fake-agent`; unknown commands print `UNKNOWN_COMMAND` and continue; adds `ctl_save` + `${var}`/`${env:X}` substitution |
| T5.1 tag-brain | done | 2b2c613 | uses `serde_yaml 0.9` (deprecated upstream, no maintained alt cached); project filter is exact (global docs excluded); mtime compared in whole seconds |
| T2.4 tag-orchestrator | done | d8b3c23 | extra `Run.paused_from`; same-state transitions rejected; `attempts` increments on queued→working |
| T0.2 StateEvent broadcast | done | a8a7ed8 | status transitions detected by diffing `agent_status()` after each event (single emission point); also adds `AppState::new` + `AppPaths` |
| T1.1 tag-control | done | 7428847 | envelope types in `tag_core::control` (avoids crate cycle); control socket is `tag-control-<pid>.sock` beside the hook socket (not a fixed `control.sock`, so several TAG instances coexist); schemars for param schemas |

## Verified CLI facts (this machine, 2026-11-23)
- `claude` 2.1.314: opening prompt as positional arg (interactive), `--resume <id>`, `--session-id <uuid>` (TAG can pick the id up front), `--mcp-config <file>`, `--strict-mcp-config`, `claude mcp add <name> -- <cmd> [args]`.
- `agy` 1.4.5: `-i/--prompt-interactive <prompt>`, `--conversation <id>` (resume), `-c/--continue`, `--model <slug>`; Gemini 4 Neon slugs `gemini-4-neon-{low,medium,high,xhigh}`; MCP only via global `agy mcp add [--env K=V] <name> <cmd> [args]` (no per-session config flag → agy conductors use the `tag ctl` prompt variant).
- `codex`: not installed here; its flags remain **[verify]**.
| T1.2 control server | done | e1600a6 | default client tiers: `ctl` read+act+dangerous, `mcp`/`claude-desktop` read+act, unknown read; over-1MB line → error then close; tiers are a guardrail, not a sandbox (same-UID peers can omit their token) |
| T2.1 profile extensions | done | 4f3bfaf | `build_argv`; built-in `agy-neon` uses verified `gemini-4-neon-high`; claude/agy prompt via args (claude positional, agy `-i`) |
| T3.1 project registry | done | f24ea71 | status bar shows `◆ project`, refreshed every 3 s on Tick |
| T5.3 brain pin/ls/show/rm/search | done | a46a13a | project pin after a global pin of the same title can reuse the id (library doesn't see global root in that call) |
| T1.0 tag daemon | done | 339f7ec | UI `--attach` deferred (per plan); daemon session lives in the state dir, separate from the UI's `~/.config/tag/session.json`; E2E helpers in `crates/bin/tests/common` scrub TAG/tmux env |
| T1.3 read methods | done | b22fa59 | `pane.read` returns screen + ≤1 screen-height of scrollback (vt100 0.15 overflows when the scrollback offset exceeds rows — the UI's own scroll may hit this too, worth checking); `usage.summary.by_profile` empty until T2.7; brain.* in T5.4 |
| T1.4 act methods | done | 81a6ebc | **Spec deviation:** `tests.run` runs as a captured background process (not a hidden shell pane) for a reliable exit code/timeout; results recorded per worktree+HEAD for the merge gate. Worktree creation goes off-loop and re-enters via an internal continuation. Hidden panes aren't restored from the session yet (T2.2) |
| T1.5 dangerous methods | done | 5e16691 | merge refuses a dirty main worktree and tests run on a dirty tree; conductor merges are blocked until run policy is wired (T2.5) |
| T1.6 tag ctl | done | 4aa2366 | positional JSON arg named `params` (a `json` positional clashed with global `--json`); exit codes 1 error / 2 timeout / 3 not running |
| T5.2 brain init | done | 295a877 | regenerated brief goes to `brief.generated.md` with id `brief_gen_<slug>` |
| T2.5 run methods + wiring | done | 56ed1f8 | E2E: fake conductor + 2 fake workers, auto & ask policies (M3). TAG types `[TAG] …` lines into the conductor pane (plan approved / task ready / blocked / merged / run done). Runs stay `running` through review (scheduler's EnterReviewing ignored) and finish when all tasks settle. **Spec additions:** `tests.run` accepts `{run, task}`; `run.create` has `spawn_conductor`. Conductors can't approve their own plan when approval is required. Worker spawn creates its worktree synchronously in the loop (git, ~50 ms) |
