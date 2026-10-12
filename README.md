# TAG

A terminal multiplexer built for autonomous coding agents, Git worktree orchestration, and high-performance workflows.

TAG combines PTY multiplexing with agent supervision, independent scrollback, native mouse behavior, a decoupled render loop, and an interface discoverable in minutes.

```
       ┌──────────────────────────────────────────────────────────┐
       │ 🤖 claude [awaiting-input]  │ 🤖 agy [running]           │
       │ > Run test suite? (y/n)    │ Running turn 4/12...       │
       │                            │                            │
       ├────────────────────────────┴────────────────────────────┤
       │ ❯_ shell                                                │
       │ $ git status                                            │
       └──────────────────────────────────────────────────────────┘
       [1: Main] [2: Review] | [NORMAL] 🔔 1 pending | ⚡ 14.2k tok
```

## Features

- **Decoupled PTY engine.** Dedicated blocking reader threads feed unbounded channels. Output renders up to 60 FPS without unbounded memory growth or child pauses.
- **Independent scrollback and mouse selection.** 50,000-line ring buffer per pane. Scroll wheel, trackpad, and page keys navigate history while background commands run. In-app click-drag, word double-click, and line triple-click copy directly to system clipboard. Shift-drag modifier passthrough honors host terminal selections.
- **Binary tree tiling and virtual workspaces.** Horizontal and vertical splits, tab groups, per-pane zoom, keyboard and mouse resize, and full layout undo history (`Leader+Z`).
- **Tab Tray (`Leader+b`).** Hideable cross-workspace tree sidebar. Filter panes as you type, drag to split, swap with `Shift+Enter`, and see live attention badges.
- **Agent supervision and hooks.** Layered status detection using Unix domain socket hooks (`cockpit-notify`), terminal notification sequences (OSC 9 / OSC 777 / BEL), and pattern heuristics.
- **Attention Queue (`Leader+a`).** Aggregates agent requests when permission or input is needed. Press Enter to jump directly to the target pane.
- **Token usage tracking and cost estimates (`Leader+u`).** Tail log parsers for Claude Code, Codex CLI, and Antigravity. Persistent SQLite aggregates for today, rolling 5-hour windows, and week totals. User-configurable pricing rates.
- **Git worktree manager (`Leader+g`).** Inspect status, create and switch worktrees, view unified diffs, and safely stage changes.
- **Snippet bank (`Leader+s`) and image bridge (`Leader+i`).** Templated macro insertions with bracketed paste. Clipboard image bridge saves to `.orchestrator/cache/` and injects local paths.
- **Secrets vault (`Leader+v`).** Argon2id passphrase key derivation and ChaCha20-Poly1305 encrypted storage. Automatically injects API keys as environment variables into child PTY sessions without writing secrets to shell history.
- **Complete discoverability.** Command palette (`Leader+p`), which-key hints after a brief leader pause, cheat-sheet overlay (`Leader+?` or `F1`), always-on hint bar, and an interactive first-run tour.

## Installation and building

Prerequisites: Rust 1.80+ (2021 edition).

```bash
# Clone the repository
git clone https://github.com/TheJonathanC/TAG.git
cd TAG

# Build release binaries
cargo build --release

# Binaries generated in target/release/:
# - tag (main multiplexer)
# - cockpit-notify (agent hook helper)
# - tag-notify (alias)
```

Run tests and clippy checks:

```bash
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Usage

Start the multiplexer:

```bash
./target/release/tag
```

### Agent notification hooks

TAG injects `COCKPIT_SOCK` and `COCKPIT_PANE_ID` into every spawned child terminal session. To configure agent hooks so alerts route into the Attention Queue:

```bash
./target/release/tag setup-hooks claude
./target/release/tag setup-hooks codex
./target/release/tag setup-hooks agy
```

When running inside TAG, agent commands invoke `cockpit-notify NeedsPermission --message "Approval needed"` to alert you immediately. Outside TAG, `cockpit-notify` is a silent no-op.

## Architecture

```
crates/
  core/        # Event bus, pane trait, command registry, theme engine
  pty/         # PTY lifecycle, VT100 parser integration, ring buffer, selection
  layout/      # Binary tree tiling, workspaces, hit testing, layout undo
  keys/        # Key chord parser, leader state machine, which-key timing
  ui/          # Ratatui views, overlays, Tab Tray, command palette, status bar
  git/         # Git backend, worktree manager, status and diff viewer
  clipboard/   # Snippet templating, bracketed paste, clipboard image bridge
  vault/       # Encrypted secret store, Argon2id, ChaCha20-Poly1305, env injection
  agents/      # Agent profiles, hook server, tmux attachment
  notify/      # Attention queue, desktop notifications via notify-rust
  usage/       # UsageProvider trait, Claude Code / Codex / agy parsers, SQLite store
  applets/     # Peer chat pane, Texas Hold'em pane
  bin/         # CLI binaries (tag, cockpit-notify, tag-notify)
```

Detailed technical documentation:
- [Design Architecture](docs/design.md)
- [Shortcut Reference](docs/SHORTCUTS.md)
- [Security Model](docs/SECURITY.md)
- [Architecture Decisions](docs/decisions/)

## License

MIT OR Apache-2.0
