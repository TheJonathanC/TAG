# Design Document: TAG (Agent Cockpit)

A terminal-native developer orchestrator with first-class support for autonomous coding agents, Git worktree management, structured clipboard/vault, usage tracking, notifications, and an extensible pane system.

## 1. Crate Boundaries

```
tag/
├── crates/
│   ├── core/         # Action enum, AppEvent, Pane trait, AppState, Config, CommandRegistry
│   ├── pty/          # PtyManager, VtGrid (vt100 integration), RingBuffer scrollback, Selection
│   ├── layout/       # Binary tree tiling, tabs, workspaces, resize math, hit-testing
│   ├── ui/           # Ratatui rendering, Theme engine, Titlebars, Overlays, Hints, Status bar
│   ├── keys/         # Keymap parser, Leader state machine, Which-key hints, Conflict detection
│   ├── git/          # GitBackend trait (libgit2 + CLI fallback), Worktree lifecycle, Diff viewer
│   ├── clipboard/    # Arboard wrapper, Snippet bank (templating), Image bridge (.orchestrator/cache)
│   ├── vault/        # Argon2id + ChaCha20Poly1305 encrypted secret store, zeroize, env injection
│   ├── agents/       # Agent profiles, layered status detection (hooks, OSC/BEL, regex), tmux attach
│   ├── notify/       # Notification center, Attention queue, notify-rust desktop notifier, DND
│   ├── usage/        # UsageProvider trait, Claude Code / Codex / agy parsers, SQLite store
│   ├── applets/      # Feature-gated non-PTY panes (peer chat, poker)
│   └── bin/          # Executable binaries: `tag` (alias `cockpit`) and `cockpit-notify` (and `tag-notify`)
└── docs/
    ├── SECURITY.md   # Vault threat model & security boundaries
    ├── SHORTCUTS.md  # Live keymap reference
    └── decisions/    # Architecture Decision Records (ADRs)
```

## 2. Event Model & Concurrency

```
+--------------------+        +-------------------------+
| PTY Reader Threads | -----> |                         |
+--------------------+        |                         |
                              |                         |
+--------------------+        |                         |
| Crossterm Input    | -----> |                         |
+--------------------+        |                         |
                              |  Unbounded MPSC Channel | ---> [ UI Event Loop ]
+--------------------+        |     (tokio::sync)       |      - Coalesce batches
| Unix Hook Socket   | -----> |                         |      - Update VT grids
+--------------------+        |                         |      - Layout recalculation
                              |                         |      - Conditional render (60fps)
+--------------------+        |                         |
| Async Tasks        | -----> |                         |
| (Usage/Git/Notify) |        |                         |
+--------------------+        +-------------------------+
```

### Event Hierarchy
- `AppEvent::PtyOutput { pane_id, data }`: Raw bytes from child PTY, consumed by pane VT emulator.
- `AppEvent::PtyExited { pane_id, exit_code }`: Child process termination notice.
- `AppEvent::TerminalInput(crossterm::event::Event)`: Keyboard, mouse, focus, resize events.
- `AppEvent::AgentHook(HookEvent)`: IPC payload sent by `cockpit-notify` via Unix domain socket.
- `AppEvent::UsageUpdate(UsageEvent)`: Normalized token consumption events.
- `AppEvent::GitStatus(GitRepoStatus)`: Asynchronous Git status/diff updates.
- `AppEvent::Action(Action)`: Semantic action dispatch (split, close, navigate, switch workspace).
- `AppEvent::Tick`: Timer tick for UI animations, toast expiration, and idle checks.

## 3. Keymap & Command Registry

- **Leader Key:** Default `Ctrl+Space`. Configurable in `config.toml`.
- **Leader State Machine:**
  - `Normal`: Passes input to active pane, except reserved direct shortcuts (`Alt+1..9`, `Alt+h/j/k/l`, `Alt+Enter`).
  - `LeaderPending`: Waits for chord. If idle for >400ms, renders the Which-Key popup overlay.
  - `Passthrough`: Toggled with `Leader+Esc`. Bypasses all multiplexer bindings for nested terminal applications.
- **Command Registry:** Centralized registry of all actions, documentation, categories, and keybindings. Powers:
  1. Searchable Command Palette (`Leader+p` or click).
  2. Contextual Cheat Sheet (`Leader+?` or `F1`).
  3. Dynamic Which-Key helper.
  4. Context-sensitive Bottom Hint Bar.

## 4. Main Risks & Mitigations

1. **PTY Flood & Memory Growth (`yes` or large `cat`):**
   - *Mitigation:* Bounded scrollback ring buffer with bulk chunk insertion. Coalesced UI rendering capped at ~60 FPS with frame dirty-checking. PTY channel drops or backpressures if reading faster than consumption limit.
2. **Terminal Mouse Capture & Selection:**
   - *Mitigation:* Full in-app mouse selection engine (click-drag, word double-click, line triple-click, auto-copy to clipboard), plus Shift-drag bypass for native terminal emulator selection.
3. **Agent Status Detection Fragility:**
   - *Mitigation:* Layered priority: Layer 1 (Unix domain socket hooks via `cockpit-notify`), Layer 2 (OSC 9 / OSC 777 / BEL escape codes), Layer 3 (configurable regex heuristics with debounce).
4. **Third-Party Agent Log Incompatibilities:**
   - *Mitigation:* Defensive, versioned JSONL parsers for Claude Code, Codex, and agy. Unknown fields ignored; corrupt lines skipped and recorded as diagnostics. App never crashes if a log file changes format.
