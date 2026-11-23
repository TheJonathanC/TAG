# TAG Shortcut Reference

Default leader key: `Ctrl+Space` (mnemonic, non-conflicting).

## Navigation & Workspaces

| Shortcut | Action | Description |
|---|---|---|
| `Alt+h` / `Leader+h` | Focus Left | Move focus to left pane |
| `Alt+j` / `Leader+j` | Focus Down | Move focus to lower pane |
| `Alt+k` / `Leader+k` | Focus Up | Move focus to upper pane |
| `Alt+l` / `Leader+l` | Focus Right | Move focus to right pane |
| `Alt+1..9` | Switch Workspace | Jump directly to workspace 1..9 |
| `Leader+w` | New Workspace | Create new workspace |
| `Leader+b` | Tab Tray | Open cross-workspace pane tree and filter |
| `Leader+B` | Brain Browser | Live brain search, document preview, outgoing links and backlinks (Tab to the link list, Enter follows a link, Esc closes) |
| `Leader+Space` | Quick Switcher | Fuzzy jump to any pane or workspace |

## Layout & Tiling

| Shortcut | Action | Description |
|---|---|---|
| `Leader+\|` | Split Right | Split current pane vertically |
| `Leader+-` | Split Down | Split current pane horizontally |
| `Leader+x` | Close Pane | Close focused pane |
| `Alt+Enter` / `Leader+z` | Toggle Zoom | Fullscreen active pane |
| `Leader+Z` | Undo Layout | Revert last layout modification |
| `Alt+H/J/K/L` | Resize Pane | Adjust split borders |

## Clipboard & Selection

| Shortcut | Action | Description |
|---|---|---|
| `Mouse Drag & Release` | Select & Auto-Copy | Draws selection and immediately copies text to clipboard (OSC 52 + local) |
| `Ctrl+Shift+C` | Copy Selection | Copies highlighted selection to clipboard over SSH/Termius and local desktop |
| `Ctrl+C (with selection)` | Smart Copy | Copies selection when active; acts as standard SIGINT cancel when no selection |
| `Leader+y` | Yank Selection | Leader shortcut to copy highlighted text to clipboard |
| `Leader+v` / `Leader+p` | Paste Clipboard | Leader shortcut to paste text from clipboard into focused pane |
| `Ctrl+Shift+V` / `Shift+Ins` | Paste Clipboard | Direct shortcut to paste text from system clipboard into the focused pane |
| `Shift + Mouse Drag` | Terminal Selection | Bypasses in-app mouse capture for terminal emulator's native selection |

## Discoverability & Tools

| Shortcut | Action | Description |
|---|---|---|
| `Leader+Space` / `Leader+:` | Command Palette | Fuzzy search and run all commands |
| `Leader+?` / `F1` | Cheat-Sheet | Context-aware shortcut help |
| `Pause after Leader` | Which-Key Popup | Live prompt of available chords |
| `Leader+U` | Update TAG | Shows how to upgrade (run `tag update` in a shell) |
| `Leader+t` | Passthrough Mode | Send all keystrokes except the leader directly to child |
| `Leader+Leader` | Send Leader | Send a literal `Ctrl+Space` to the focused pane |
| `Leader+Esc` | Cancel | Cancel a pending leader chord |
| `Leader+a` | Attention Queue | Review and jump to pending agent requests |
| `Leader+u` | Token Usage & Costs | Usage dashboard, rolling windows, and rates |
| `Leader+g` | Git Worktrees | Worktree lifecycle, status, and diff viewer |
| `Leader+s` | Snippet Bank | Templated macros with placeholder filling |
| `Leader+i` | Image Bridge | Paste clipboard image and insert worktree path |
| `Leader+e` | Secrets Vault | Encrypted storage, interactive secret management ([a] Add, [d] Delete), and agent env injection |
| `Leader+n` | Do Not Disturb | Mute desktop notifications |
| `Leader+q` | Quit | Clean shutdown and terminal state restore |

## Orchestration

| Shortcut | Action | Description |
| --- | --- | --- |
| `Leader+o` | Orchestrate | Prompt for a goal and start a run: a conductor agent plans it and TAG runs the worker agents (approve the plan from the attention queue, `Leader+a`) |
| `Leader+A` | Workers table | k9s-style overlay of every agent worker (needs-input first). `↑/↓`/`j` move, `Enter` open as pane, `e` peek, `h` hide, `k` kill (`y` confirms), `r` reply, `/` filter text, `f` cycle filter (all / needs me / running / done), `Esc` close. Click selects, double-click opens |
| `Leader+P` | Project switcher | Fuzzy tree of projects → workspaces → panes (recent first, `*` marks the active project) with a live preview of the highlighted pane. Type to filter, `↑/↓` move, `→`/`Tab` expand, `←` collapse, `Enter` switches to the project / workspace / pane (processes keep running), `Ctrl+O` opens a folder as a new project (resolved to its git root; a shell is started if it is empty), `Esc` closes. `P` used to be a duplicate paste alias; paste stays on `p`, `v`, `V` |
| ``Leader+` `` | Last workspace | Jump back to the previous workspace, across projects (`Leader+Tab` already means next workspace) |
| `Leader+!` | Next agent needing input | Round-robin over every agent awaiting input in all projects: switches project and workspace and focuses it (hidden/card agents are promoted to a pane). Toasts "No agents need input" when none |
| `Leader+f` | Peek | Floating live view (~80% of the screen) of the first card, else the first agent needing input, else the first hidden worker. `i` turns input forwarding on (then `Alt+i` turns it off), `Enter` opens it as a pane, `Esc` closes |
| Card strip | Cards | Minimized/background workers show in a one-row strip above the status bar (`[● t1 agy 4m] [? t2 needs input] [✓ t3]`); click a card to promote it to a pane |
| `Leader+O` | Run View | Tasks of the active run with state glyphs, live worker output, diff stat / last test output (`Tab`). Keys: `Enter` open, `e` peek, `d` diff, `t` test, `r` reply, `m` merge, `k` kill (y), `a` approve plan, `[`/`]` switch run |
