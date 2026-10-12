# ADR 0003: Agent Status Detection and Hooks Architecture

## Context
Coding agents (Claude Code, Codex CLI, Antigravity/agy) run asynchronously inside terminal panes. The multiplexer must detect when an agent needs user permission, asks a question, runs a turn, or finishes a task.

## Decision
Layered priority model, ordered by reliability:
1. **Layer 1: Agent Hooks & IPC Socket (`COCKPIT_SOCK`):**
   - The multiplexer opens a private Unix domain socket at `$XDG_RUNTIME_DIR/tag/cockpit.sock` with `0600` permissions.
   - PTY processes receive `COCKPIT_SOCK` and `COCKPIT_PANE_ID`.
   - The companion CLI binary `cockpit-notify` (or `tag-notify`) sends structured JSON notifications:
     - `NeedsPermission`: Agent requires approval for command/tool execution.
     - `NeedsInput`: Agent prompt waiting for user question answer.
     - `TaskDone`: Turn finished.
     - `Error`: Fatal or task error.
2. **Layer 2: Terminal Notification Sequences:**
   - Detect terminal BELL (`\x07`), OSC 9 (iTerm2 style), and OSC 777 (rxvt/kitty desktop notify) sequences emitted on stdout/stderr.
3. **Layer 3: Output Pattern Heuristics:**
   - Configurable per-agent regex rules and idle timers (e.g. matching `(y/n)` or `[Y/n]` or `approval requested`). Treated as best effort.
4. **Tool Compatibility:**
   - Claude Code: Hook configuration supported via config/hooks.
   - Codex CLI: Notify command supported.
   - Antigravity (agy): Uses Layer 2/3 heuristics and transcript event polling; marked "limited hook support" until direct hook integration is exposed.
