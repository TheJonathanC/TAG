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
