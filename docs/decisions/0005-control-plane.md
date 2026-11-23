# ADR 0005: Local Control Plane (socket, `tag ctl`, `tag mcp`)

## Context
TAG shows agents; to *run* them (orchestration, external assistants driving TAG, agents
coordinating each other) everything a human can do to panes, agents, worktrees and runs must be
available programmatically. The existing hook socket is fire-and-forget (one payload, then close),
so it cannot carry responses, and installed agent hooks depend on it staying unchanged.

## Decision
1. **A second socket (D3).** Each TAG process (UI or `tag daemon`) serves
   `tag-control-<pid>.sock` next to its hook socket (`$XDG_RUNTIME_DIR/tag`, `~/.config/tag/run`, or
   the short `/tmp/tag-<uid>` fallback), mode `0600`, with a peer-UID check. Framing is
   newline-delimited JSON-RPC 2.0 with server→client `event` notifications; request lines are capped
   at 1 MB (an over-long line gets an error and the connection closes; a malformed line gets a parse
   error and the connection stays open). Idle connections close after 10 minutes unless subscribed.
   The file name carries the PID (unlike the plan's fixed `control.sock`) so several instances can
   coexist; clients use `$TAG_CONTROL_SOCKET`, else the newest live socket.
2. **One protocol, three front-ends (D4).** The socket is the implementation; `tag ctl` (CLI) and
   `tag mcp` (stdio MCP server, hand-rolled, no SDK) are thin clients of it. Method names, tiers,
   allowed roles, descriptions and JSON-Schema params live in one catalog
   (`tag_control::methods::CATALOG`), which drives server authorization and MCP `tools/list`.
3. **Handshake and tiers (D5).** The first call must be `tag.hello`. Every method has a tier:
   `read`, `act` or `dangerous`. Untokened clients get tiers from `[control.clients.<name>]`
   (built-in: `ctl` → read/act/dangerous, `mcp` and `claude-desktop` → read/act, anything else →
   read). Every act/dangerous call is appended to `<state>/control-audit.jsonl` with free-text fields
   (`prompt`, `text`, `body`, …) truncated to 200 characters.
4. **Scoped tokens (D6).** Agents TAG launches for a run get `TAG_CONTROL_TOKEN`, `TAG_RUN_ID`,
   `TAG_TASK_ID` and `TAG_CONTROL_SOCKET` in their environment. A conductor token is limited to its
   run (and can only merge when the run's `merge_policy = "auto"`, never touching protected paths;
   otherwise the merge becomes an attention item). A worker token can only read/test its own
   pane and worktree and `task.report`. Tokens live in memory and are revoked when the run ends.
5. **Server internals.** The server runs on tokio tasks and never blocks the app loop. Requests reach
   the loop as `AppEvent::Control(ControlEnvelope)` with a clonable one-shot `ReplyHandle`; handlers
   answer within a short critical section and push slow work (git, tests, merges, worktree
   creation) to background threads, which reply directly or re-enter the loop with an internal
   continuation. `agent.wait` and `events.subscribe` run in the server on a
   `broadcast::Sender<StateEvent>` the loop publishes to (pane status changes are detected by
   diffing `agent_status()` after each event).
6. **Vault boundary (D7).** `pane.spawn` may name vault secrets to inject into the child's
   environment; values are never returned over the control plane, and a locked vault is a
   `PolicyBlocked` error.
7. **Compact outputs (D11).** `pane.read` returns an ANSI-free tail (screen plus up to one screen of
   scrollback) capped from the end; `worktree.diff` returns the full `--stat` and whole-file hunks up
   to `max_bytes` with `truncated`; MCP results are pretty JSON capped at 100 KB.

## Consequences
- External assistants (Claude desktop app, Claude Code, agy) can list, read and drive agents through
  `tag mcp`; merges, kills and plan approval stay with the human unless a client is explicitly
  granted `dangerous`.
- **Tiers are a guardrail, not a sandbox.** Any process running as the same UID can connect to the
  socket and can simply omit its token (becoming an untokened client such as `ctl`). Tiers and
  scoped tokens stop confused or prompt-injected agents that use the provided tools; they do not
  stop deliberately hostile code with shell access as the same user. See `docs/SECURITY.md`.
- The control socket path differs per process; scripts should use `$TAG_CONTROL_SOCKET` (set for
  every pane TAG spawns) or `tag ctl`'s discovery.
- Codex's flags (resume, MCP config) are unverified on the development machine.
