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
