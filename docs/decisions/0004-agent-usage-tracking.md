# ADR 0004: Agent Usage Tracking and Cost Estimation

## Context
Developers need real-time awareness of token consumption (prompt tokens, completion tokens, cache reads, cache writes) and estimated costs across agents, projects, and worktrees.

## Decision
1. **Architecture:**
   - Define a read-only `UsageProvider` trait with `detect()`, `scan()`, and `watch()` methods.
   - Separate provider implementations for:
     - `ClaudeCodeProvider`: Parses session transcript JSONL in `~/.claude/projects/`.
     - `CodexProvider`: Parses session JSONL in `~/.codex/sessions/`.
     - `AgyProvider`: Parses transcript JSONL in `~/.gemini/antigravity-cli/brain/<conversation-id>/.system_generated/logs/transcript.jsonl`.
2. **Defensive Parsing:**
   - Parsers are versioned and read-only.
   - Unknown schema fields are ignored. Malformed lines are skipped and increment a diagnostic error counter without halting execution.
   - File offsets are persisted to avoid re-reading unchanged history.
3. **Storage & Aggregation:**
   - Store normalized `UsageEvent`s in a local SQLite database (`$XDG_DATA_HOME/tag/usage.db`).
   - Deduplicate events by `(agent, session_id, step_index)` or message ID.
   - Provide aggregations for today, rolling 5-hour window, 7-day, and 30-day totals.
4. **Pricing Transparency:**
   - Costs are configurable via `pricing.toml` and always labeled as "estimated".
