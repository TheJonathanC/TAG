# TAG: Terminal Agent Orchestrator & PTY Multiplexer

TAG is a developer multiplexer built for autonomous coding agents (Claude Code, Codex CLI, Antigravity/agy), Git worktree orchestration, independent scrollback, and terminal workflows.

TAG combines PTY process management with agent supervision, a decoupled 60 FPS render loop, mouse selection parity, and discoverability designed for rapid learning.

```
       ┌──────────────────────────────────────────────────────────┐
       │ [agent] claude [awaiting-input] │ [agent] agy [running]  │
       │ > Run test suite? (y/n)         │ Running turn 4/12...   │
       │                                 │                        │
       ├─────────────────────────────────┴────────────────────────┤
       │ [sh] shell                                               │
       │ $ git status                                             │
       └──────────────────────────────────────────────────────────┘
       [1: Main] [2: Dev] | [NORMAL] [!] 1 pending | [*] 14.2k tok
```

---

## Table of contents

1. [Installation and setup](#installation-and-setup)
2. [Interaction model and discoverability](#interaction-model-and-discoverability)
3. [Features: what was built, what it does, and how to use it](#features-what-was-built-what-it-does-and-how-to-use-it)
   - [PTY multiplexer and process engine](#1-pty-multiplexer-and-process-engine)
   - [Independent scrollback and mouse selection](#2-independent-scrollback-and-mouse-selection)
   - [Binary tree tiling and virtual workspaces](#3-binary-tree-tiling-and-virtual-workspaces)
   - [Layout undo history](#4-layout-undo-history)
   - [Tab Tray and quick switcher](#5-tab-tray-and-quick-switcher)
   - [Autonomous agent supervision](#6-autonomous-agent-supervision)
   - [Agent notifications and Attention Queue](#7-agent-notifications-and-attention-queue)
   - [Token usage tracking and cost estimation](#8-token-usage-tracking-and-cost-estimation)
   - [Git worktree orchestration and diff viewer](#9-git-worktree-orchestration-and-diff-viewer)
   - [Snippet bank and clipboard image bridge](#10-snippet-bank-and-clipboard-image-bridge)
   - [Secrets vault and environment injection](#11-secrets-vault-and-environment-injection)
   - [Non-PTY applets (Peer Chat and Texas Hold'em)](#12-non-pty-applets)
   - [Passthrough mode](#13-passthrough-mode)
   - [Theming and visual polish](#14-theming-and-visual-polish)
4. [Complete keyboard shortcut reference](#complete-keyboard-shortcut-reference)
5. [CLI reference and helper utilities](#cli-reference-and-helper-utilities)
6. [Configuration files](#configuration-files)
7. [Architecture and crate boundaries](#architecture-and-crate-boundaries)
8. [Testing and quality gates](#testing-and-quality-gates)

---

## Installation and setup

### Quick install (Linux & macOS)

Install the latest pre-compiled binaries with a single command:

```bash
# Install latest stable release
curl -fsSL https://raw.githubusercontent.com/TheJonathanC/TAG/main/scripts/install.sh | bash

# Or install latest beta release
curl -fsSL https://raw.githubusercontent.com/TheJonathanC/TAG/main/scripts/install.sh | bash -s -- --beta
```

Updating an existing installation:
```bash
tag update        # Update to latest stable
tag update --beta # Update to latest beta
```

---

### Building from source

Prerequisites: Rust 1.80+ (stable, 2021 edition).

Clone the repository and build the release binaries:

```bash
git clone https://github.com/TheJonathanC/TAG.git
cd TAG
cargo build --release
```

The release build generates three executable binaries in `target/release/`:
- `tag`: The primary orchestrator and multiplexer.
- `cockpit-notify`: Lightweight helper called by agent notification hooks.
- `tag-notify`: Desktop notification daemon helper.

Copy the binaries into your local path:

```bash
cp target/release/tag target/release/cockpit-notify target/release/tag-notify ~/.local/bin/
```

Verify the installation:

```bash
which tag
tag --version
```

---

## Interaction model and discoverability

TAG follows the rule: **discoverable by default, fast once learned.**

1. **Non-conflicting leader key.** The default leader is `Ctrl+Space`. It avoids conflicts with readline (`Ctrl+A`), tmux (`Ctrl+B`), and terminal EOF (`Ctrl+D`).
2. **Which-Key discovery popup.** Press `Ctrl+Space` and wait 400 milliseconds. A panel appears along the bottom displaying every available follower key and what it triggers.
3. **Command Palette (`Ctrl+Space p`).** A fuzzy-search modal listing every action available in the app alongside its active shortcut.
4. **Contextual Cheat-Sheet (`F1` or `Ctrl+Space ?`).** A full scrollable manual and key reference inside the terminal. Use `Up`/`Down` or `PageUp`/`PageDown` to browse.
5. **Always-on status line.** The bottom status line displays active workspaces, current input mode, pending agent count, token consumption, and context hints.
6. **First-run tour.** A step-by-step walkthrough covering splits, workspaces, and agent tools. Trigger it anytime from the palette or keymap.

---

## Features: what was built, what it does, and how to use it

### 1. PTY multiplexer and process engine

- **What it is:** Owns the host terminal in raw mode, manages child pseudo-terminals, and runs VT100 ANSI terminal emulation.
- **What it does:** Decouples PTY reads from rendering. Each child process runs a dedicated OS reader thread sending chunks into an unbounded channel. A frame loop draws up to 60 FPS only when changes occur. It prevents terminal freezing during high-throughput output such as `cat` on large files.
- **How to use it:**
  - Launch `tag`. It automatically starts with an interactive shell.
  - New shells spawn inside any split or new workspace automatically.
  - When a child process exits, the pane border displays the exit code (e.g., `[Exited: 0]`) rather than closing abruptly.

### 2. Independent scrollback and mouse selection

- **What it is:** A 50,000-line circular buffer per pane combined with an in-app text selection engine.
- **What it does:** Separates viewport navigation from process execution. Scrolling upward lets you inspect past output while the child process continues running. It renders a `[Scrolled: -N lines]` badge. Text selection works despite terminal mouse capture.
- **How to use it:**
  - **Scroll:** Use your mouse wheel or trackpad over any pane. TAG dynamically detects alternate screen applications:
    - In **htop, tmux, agy, vim, less**: wheel scrolling sends SGR mouse reporting sequences (if mouse mode is enabled) or Alternate Scroll cursor arrow keys (`Up` / `Down`), scrolling the program directly without clipping.
    - In standard shells and build logs: wheel scrolling scrolls the terminal viewport ring buffer (up to 50,000 lines).
  - **Keyboard scrollback:** Press `Shift+PageUp` / `Shift+PageDown` to scroll terminal history.
  - **Jump to bottom:** Press any key to resume live output.
  - **Select text:** Click and drag with the left mouse button to highlight a character block.
  - **Select word:** Double-click on any word.
  - **Select line:** Triple-click on any row.
  - **Copy:** Releasing the mouse automatically copies selected text to your system clipboard using `arboard`.
  - **Terminal emulator bypass:** Hold `Shift` while dragging to bypass TAG and use your native terminal's selection mechanism.

### 3. Binary tree tiling, pane management, and virtual workspaces

- **What it is:** A geometric layout engine supporting arbitrary vertical and horizontal splits plus numbered virtual workspaces.
- **What it does:** Calculates rectangular coordinates using ratio-based sizing. Moving, splitting, or zooming panes changes only display geometry; background processes are never restarted.
- **How to use it:**
  - **Split vertically:** Press `Ctrl+Space |`.
  - **Split horizontally:** Press `Ctrl+Space -`.
  - **Maximize / Restore pane:** Press `Ctrl+Space m`, `Alt+m`, `Ctrl+Space z`, or `Alt+Enter` to maximize the active pane to fullscreen. Press again to restore splits. You can also click the `[^]` (maximize) / `[-]` (restore) button on the top-right border.
  - **Splits disabled while maximized:** To preserve layout geometry and prevent accidental hidden splits, creating splits (`Ctrl+Space |`, `Ctrl+Space -`, or Tab Tray `s`/`v`) is blocked while a pane is maximized. Restore the pane first to split.
  - **Rename pane:** Press `Ctrl+Space ,` or `Ctrl+Space R` to open an interactive rename prompt. Enter the new title and press `Enter`. In the Tab Tray (`Ctrl+Space b`), press `r` on any pane to rename it. Custom titles persist across app restarts.
  - **Close focused pane:** Press `Ctrl+Space x`.
  - **Click to close:** Click the `[x]` button on the top-right corner of any pane border.
  - **Shell exit:** Type `exit` or press `Ctrl+D` inside any shell to close the pane cleanly.
  - **Tab Tray close:** Press `Ctrl+Space b`, navigate to any pane, and press `x`.
  - **Command Palette close:** Press `Ctrl+Space p` and search `Close Pane`.
  - **Move focus:** Press `Alt+h` (left), `Alt+j` (down), `Alt+k` (up), or `Alt+l` (right).
  - **Resize splits:** Drag split borders using your mouse.
  - **Switch workspaces:** Press `Alt+1` through `Alt+9`.
  - **Create workspace:** Press `Ctrl+Space w`.

### 4. Layout undo history

- **What it is:** A stack-based snapshot mechanism for workspace layout trees.
- **What it does:** Records snapshots of the layout tree before each split, close, or resize. If you close a pane or alter your layout by accident, undo restores the previous geometry.
- **How to use it:**
  - Press `Ctrl+Space Z` or choose "Undo Layout Change" from the Command Palette (`Ctrl+Space p`).

### 5. Tab Tray and quick switcher

- **What it is:** A cross-workspace sidebar navigator (`Ctrl+Space b`) and fuzzy switcher (`Ctrl+Space Space`).
- **What it does:** Displays an interactive tree of all open panes across all workspaces. Shows process type icons, titles, bound Git branches, live status badges, and attention alerts.
- **How to use it:**
  - Press `Ctrl+Space b` to toggle the Tab Tray.
  - **Filter:** Type characters directly to filter panes across all workspaces.
  - **Switch:** Press `Enter` or click a row to jump directly to that pane.
  - **Pull into split:** Press `s` (horizontal split) or `v` (vertical split) on a row to pull that pane into the current view.
  - **Swap views:** Press `Shift+Enter` to swap positions between the selected pane and your current focused pane.
  - **Close or zoom:** Press `x` to close or `z` to zoom.
  - **Quick switcher:** Press `Ctrl+Space Space` for an instant fuzzy overlay ordered by recent use and alert state.

### 6. Autonomous agent supervision

- **What it is:** A supervisor pane type (`AgentPane`) tailored for coding agents such as Claude Code, Codex CLI, and Antigravity.
- **What it does:** Monitors agent lifecycles using a three-tier status model:
  1. *Layer 1 (Hooks):* Private Unix domain socket IPC via `cockpit-notify`.
  2. *Layer 2 (Terminal signals):* Captures terminal BELL (`\x07`) and OSC 9 / OSC 777 notification sequences.
  3. *Layer 3 (Heuristics):* Regular expression patterns detecting permission requests and question prompts.
- **How to use it:**
  - Spawn an agent pane directly or configure profiles in `config.toml`.
  - View live badges on pane borders: `[running]`, `[awaiting-input]`, `[done]`, or `[error]`.
  - Attach to existing tmux sessions via `Action::AttachTmux`.

### 7. Agent notifications and Attention Queue

- **What it is:** An event routing engine that alerts you when agents need input, request permission, or finish tasks.
- **What it does:** Generates in-app corner toasts, highlights borders, increments the status bar badge, and sends desktop notifications (`notify-rust`) when the app is unfocused. Pending requests queue into the Attention Queue.
- **How to use it:**
  - Configure hooks once:
    ```bash
    tag setup-hooks claude
    tag setup-hooks codex
    tag setup-hooks agy
    ```
  - When an agent triggers an alert, press `Ctrl+Space a` to open the **Attention Queue**.
  - Highlight the alert and press `Enter` to jump directly into the pane with input focused.
  - Toggle **Do Not Disturb** with `Ctrl+Space n` to mute desktop alerts during meetings or focus blocks.

### 8. Token usage tracking and cost estimation

- **What it is:** Read-only log parsers and a local SQLite database that record token counts and estimate costs.
- **What it does:** Tails session transcripts incrementally without altering source files. Aggregates prompt tokens, completion tokens, and cache reads into persistent totals.
  - *Claude Code:* Reads JSONL transcripts under `~/.claude/projects/`.
  - *Codex CLI:* Reads session logs under `~/.codex/sessions/`.
  - *Antigravity (agy):* Reads transcript logs under `~/.gemini/antigravity-cli/brain/*/transcript.jsonl`.
- **How to use it:**
  - Press `Ctrl+Space u` to open the **Usage Dashboard**.
  - Inspect totals for today, rolling 5-hour windows, and overall events.
  - View live token summaries directly in the bottom status line.
  - Configure pricing models in `pricing.toml` to customize per-million token rates.

### 9. Git worktree orchestration and diff viewer

- **What it is:** A Git worktree manager with a built-in colorized diff inspector.
- **What it does:** Lists worktrees, checks ahead/behind status, reports staged and unstaged file modifications, and generates unified diffs using `git2` and Git CLI fallbacks.
- **How to use it:**
  - Press `Ctrl+Space g` to open the Git overlay.
  - **Left panel:** Browse existing worktrees and modified files.
  - **Right panel:** Inspect the unified diff with colored additions (green), deletions (red), and hunk headers (cyan).
  - Use `Up` and `Down` to scroll through diff hunks.

### 10. Snippet bank and clipboard image bridge

- **What it is:** A structured macro bank and an automated clipboard image caching pipeline.
- **What it does:**
  - *Snippets:* Stores reusable command prompts with `{{variable}}` placeholders. Injects content via bracketed paste escape codes (`\x1b[200~` ... `\x1b[201~`) so multiline snippets do not run prematurely.
  - *Image bridge:* Reads PNG image data directly from your OS clipboard, writes it to `.orchestrator/cache/img_<timestamp>.png`, automatically adds the cache path to `.git/info/exclude`, and injects the file path into the active terminal pane.
- **How to use it:**
  - **Insert snippet:** Press `Ctrl+Space s`, choose a template, and press `Enter`.
  - **Paste clipboard image:** Copy an image from your browser or screenshot tool, switch to TAG, and press `Ctrl+Space i`. The local file path is pasted into your prompt.
  - **Over SSH:**
    - Option 1 (X11 Forwarding): Connect with `ssh -X` or `ssh -Y` for direct clipboard access from your local machine.
    - Option 2 (`tag paste-image` CLI): Pipe or pass images directly from your terminal:
      ```bash
      # macOS local to remote server
      pbpaste | ssh myserver 'tag paste-image'

      # Linux Wayland/X11 local to remote server
      wl-paste | ssh myserver 'tag paste-image'

      # From within any remote shell
      tag paste-image /path/to/screenshot.png
      ```
    - Option 3 (Staged drop): Copy any image file to `.orchestrator/cache/drop.png` or `/tmp/tag-clipboard-drop.png` and press `Ctrl+Space i`. TAG automatically imports it, saves it as PNG, and injects the path.

### 11. Secrets vault and environment injection

- **What it is:** An encrypted credential vault protected by Argon2id key derivation and ChaCha20-Poly1305 authenticated encryption.
- **What it does:** Holds API keys (such as `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`) in memory using `secrecy` and `zeroize` types. Injects keys directly into child PTY environments at spawn time without passing secrets on command lines or saving them to shell history. Locks automatically after an idle timeout.
- **How to use it:**
  - Press `Ctrl+Space v` to open the Vault overlay.
  - Enter your master passphrase to unlock the vault.
  - Configured keys are injected into all subsequently spawned agent and shell panes.
  - See [docs/SECURITY.md](docs/SECURITY.md) for the threat model and security boundaries.

### 12. Non-PTY applets

- **What it is:** Native pane implementations that prove the `Pane` trait decouples the multiplexer from OS processes.
- **What it does:**
  - *Peer Chat:* An in-terminal messaging room.
  - *Texas Hold'em:* A terminal poker mini-game with chip tracking, community cards, and betting rounds.
- **How to use it:**
  - Launchable via the Command Palette (`Ctrl+Space p`) or programmatically via the pane registry.

### 13. Passthrough mode

- **What it is:** A raw keyboard bypass mode.
- **What it does:** Forwards all key events directly to the child process without interception, allowing you to use nested tmux, vim, nano, or htop sessions without shortcut collisions.
- **How to use it:**
  - Press `Ctrl+Space Esc` to toggle passthrough mode.
  - The status bar displays `[PASSTHROUGH]` in bold red.
  - Press `Ctrl+Space Esc` again to return to normal mode.

### 14. Theming and visual polish

- **What it is:** A truecolor (24-bit) theme engine.
- **What it does:** Renders borders, badges, titles, and text styles according to your configuration. Includes built-in dark themes and a dedicated high-contrast accessibility theme that avoids relying on color alone for status indication.
- **How to use it:**
  - Customize colors in `config.toml`.
  - Press `Ctrl+Space r` to reload theme settings without restarting.

---

## Complete keyboard shortcut reference

Default leader key: `Ctrl+Space`

| Category | Shortcut | Action | Description |
|---|---|---|---|
| **Discoverability** | `Ctrl+Space (pause)` | Which-Key | Displays available chords after 400ms pause |
| | `Ctrl+Space p` | Command Palette | Fuzzy search and run any action |
| | `F1` or `Ctrl+Space ?` | Help Overlay | Scrollable guide and shortcut reference |
| | `Ctrl+Space Esc` | Passthrough Mode | Send all keystrokes directly to child app |
| **Navigation** | `Alt+h` / `Ctrl+Space h` | Focus Left | Move focus to left pane |
| | `Alt+j` / `Ctrl+Space j` | Focus Down | Move focus to bottom pane |
| | `Alt+k` / `Ctrl+Space k` | Focus Up | Move focus to top pane |
| | `Alt+l` / `Ctrl+Space l` | Focus Right | Move focus to right pane |
| | `Alt+1 .. Alt+9` | Jump Workspace | Switch directly to workspace 1 through 9 |
| | `Ctrl+Space w` | New Workspace | Create a new virtual desktop with a shell |
| | `Ctrl+Space b` | Tab Tray | Toggle cross-workspace sidebar |
| | `Ctrl+Space Space` | Quick Switcher | Fuzzy jump to any pane across workspaces |
| **Layout & Splits** | `Ctrl+Space \|` | Split Right | Split active pane vertically |
| | `Ctrl+Space -` | Split Down | Split active pane horizontally |
| | `Ctrl+Space x` | Close Pane | Close focused pane |
| | `Alt+Enter` / `Ctrl+Space z`| Toggle Zoom | Fullscreen active pane |
| | `Ctrl+Space Z` | Undo Layout | Undo last split, close, or resize action |
| | `Mouse Drag Border` | Resize Split | Drag pane borders with the mouse |
| **Agent Supervision**| `Ctrl+Space a` | Attention Queue | Review pending agent permission/input alerts |
| | `Ctrl+Space u` | Usage Dashboard | View token counts and estimated costs |
| | `Ctrl+Space n` | Do Not Disturb | Mute desktop notifications and bells |
| **Tools & Git** | `Ctrl+Space g` | Git Worktrees | Worktree lifecycle, status, and diff viewer |
| | `Ctrl+Space s` | Snippet Bank | Insert templated macros via bracketed paste |
| | `Ctrl+Space i` | Image Bridge | Paste clipboard image and insert local path |
| | `Ctrl+Space v` | Secrets Vault | Encrypted storage and agent env injection |
| | `Ctrl+Space r` | Reload Config | Hot reload keybindings and theme |
| | `Ctrl+Space q` | Quit | Clean shutdown and terminal state restore |

---

## CLI reference and helper utilities

### `tag`
Main multiplexer binary.

```bash
# Start the orchestrator
tag

# View CLI options
tag --help

# Configure agent notification hooks
tag setup-hooks claude
tag setup-hooks codex
tag setup-hooks agy
```

### `cockpit-notify` / `tag-notify`
IPC helper binary called by agent hooks to post events to TAG's Unix domain socket.

```bash
# General syntax
cockpit-notify <EVENT> [--message "<MESSAGE>"] [--agent "<AGENT>"]

# Example: notify that an agent needs permission
cockpit-notify NeedsPermission --message "Bash tool execution requested" --agent claude

# Example: notify that a task turn has completed
cockpit-notify TaskDone --message "Refactoring complete" --agent codex
```

*Note: If `COCKPIT_SOCK` is not set in the environment, `cockpit-notify` exits immediately with exit code 0 as a silent no-op.*

---

## Configuration files

Configuration files live in `$XDG_CONFIG_HOME/agent-cockpit/` or `~/.config/tag/`.

### `config.toml`

```toml
leader = "ctrl+space"
which_key_delay_ms = 400
scrollback_lines = 50000
keymap_preset = "default"
vault_idle_timeout_mins = 15

[theme]
name = "default-dark"
border_focused = "#61afef"
border_unfocused = "#3e4451"
accent = "#98c379"
foreground = "#abb2bf"
warning = "#e5c07b"
error = "#e06c75"
success = "#98c379"
info = "#56b6c2"
nerd_fonts = true

[notifications]
enabled = true
desktop_notifications = true
sound = false
only_when_unfocused = true
debounce_ms = 1000

[agents.claude]
name = "Claude Code"
command = "claude"
args = []
status_patterns = ["(?i)approve", "(?i)\\(y/n\\)"]

[agents.codex]
name = "Codex CLI"
command = "codex"
args = []
status_patterns = ["(?i)permission", "(?i)confirm"]

[agents.agy]
name = "Antigravity"
command = "agy"
args = []
status_patterns = ["(?i)allow", "(?i)proceed\\?"]
```

---

## Architecture and crate boundaries

TAG is arranged as a modular Rust Cargo workspace:

```
crates/
  core/        # Event bus, AppState, Pane trait, Action enum, CommandRegistry
  pty/         # PTY manager, VT100 parser, ring buffer, mouse selection engine
  layout/      # Binary tree tiling, layout undo stack, hit testing, workspaces
  keys/        # Key chord parser, leader state machine, which-key timing
  ui/          # Ratatui views, overlays, Tab Tray, command palette, status bar
  git/         # Git backend (git2 + CLI), worktree manager, diff viewer
  clipboard/   # Snippet templating, bracketed paste, clipboard image bridge
  vault/       # Argon2id, ChaCha20-Poly1305, zeroize memory, env injection
  agents/      # Agent profiles, hook server, tmux attachment
  notify/      # Attention queue, desktop notifications via notify-rust
  usage/       # UsageProvider trait, Claude Code / Codex / agy parsers, SQLite store
  applets/     # Peer chat pane, Texas Hold'em poker pane
  bin/         # Binaries: tag, cockpit-notify, tag-notify
```

Documentation files:
- [docs/design.md](docs/design.md): System architecture and concurrency design.
- [docs/SHORTCUTS.md](docs/SHORTCUTS.md): Full live keybinding table.
- [docs/SECURITY.md](docs/SECURITY.md): Vault threat model and security boundaries.
- [docs/decisions/](docs/decisions/): Architectural Decision Records (ADRs).

---

## Testing and quality gates

All checks pass with zero warnings:

```bash
# Check code formatting
cargo fmt -- --check

# Run linter across all crates and targets
cargo clippy --all-targets -- -D warnings

# Execute unit and integration tests
cargo test
```

Unit and integration tests verify:
- Layout tree binary splitting, ratio adjustments, and undo history.
- Scrollback ring buffer capacity limits and multiline text extraction.
- Key chord parsing, leader state machine transitions, and conflict detection.
- Real PTY process spawning, stdout capture, and resize handling.
- Unix domain socket IPC message delivery with `cockpit-notify`.
- Usage log parsing against Claude Code, Codex, and Antigravity fixtures.
- Vault Argon2id and ChaCha20-Poly1305 cryptographic round-trips.

---

## License

MIT OR Apache-2.0
