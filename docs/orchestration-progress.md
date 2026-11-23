# Orchestration roadmap: progress log

Branch: `integration/orchestration` (all tasks merged here; one PR to `main` at the end).
Plan: `docs/orchestration-roadmap.md`. Based on `origin/main` (`6403cc2`).

| Task | Status | Commit | Notes |
|---|---|---|---|
| T0.1 split main.rs | done | f8e30fa | main.rs 194 lines; app/events.rs 1316 (handle_app_event_inner is one fn, left intact) |
| T0.3 fake agent | done | ce65743 | `tools/fake-agent`; unknown commands print `UNKNOWN_COMMAND` and continue; adds `ctl_save` + `${var}`/`${env:X}` substitution |
| T5.1 tag-brain | done | 2b2c613 | uses `serde_yaml 0.9` (deprecated upstream, no maintained alt cached); project filter is exact (global docs excluded); mtime compared in whole seconds |
| T2.4 tag-orchestrator | done | d8b3c23 | extra `Run.paused_from`; same-state transitions rejected; `attempts` increments on queued→working |
