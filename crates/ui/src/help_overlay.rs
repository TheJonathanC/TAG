use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use tag_core::theme::Theme;
use tag_keys::Keymap;

pub fn render_help_overlay(
    frame: &mut Frame,
    total_area: Rect,
    _keymap: &Keymap,
    scroll_offset: usize,
    theme: &Theme,
) {
    let modal_width = (total_area.width * 8 / 10)
        .clamp(60, 120)
        .min(total_area.width);
    let modal_height = (total_area.height * 8 / 10)
        .clamp(18, 42)
        .min(total_area.height);
    let modal_x = (total_area.width.saturating_sub(modal_width)) / 2;
    let modal_y = (total_area.height.saturating_sub(modal_height)) / 2;

    let modal_area = Rect {
        x: modal_x,
        y: modal_y,
        width: modal_width,
        height: modal_height,
    };

    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" TAG Complete Feature Guide & Help (Up/Down Scroll, Esc to close) ")
        .borders(Borders::ALL)
        .border_style(theme.border_focused);

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let sections = vec![
        (
            "1. Interaction Model & Discoverability",
            "TAG uses Ctrl+Space as the leader key. It never conflicts with readline (Ctrl+A) or tmux (Ctrl+B).",
            vec![
                ("Ctrl+Space (wait 400ms)", "Which-Key popup", "Shows all available follower keys and actions"),
                ("Ctrl+Space p", "Command Palette", "Fuzzy-searchable list of all actions with shortcuts"),
                ("Ctrl+Space ? or F1", "Help Overlay", "This comprehensive guide and shortcut reference"),
                ("Ctrl+Space Esc", "Passthrough Mode", "Bypasses all TAG bindings to send keys directly to child app"),
                ("Command: tag setup-hooks", "Hook Setup CLI", "Prints configuration diffs to connect agents to TAG"),
            ],
        ),
        (
            "2. Navigation & Workspaces",
            "Virtual desktops let you group tasks. Panes keep running in the background when switched.",
            vec![
                ("Alt+h / j / k / l", "Focus Navigation", "Move focus left, down, up, or right across splits"),
                ("Alt+1 .. Alt+9", "Switch Workspace", "Direct jump to numbered virtual workspace"),
                ("Ctrl+Space w", "New Workspace", "Create a new virtual desktop with a fresh shell"),
                ("Ctrl+Space b", "Tab Tray", "Toggle sidebar listing all panes across all workspaces"),
                ("Ctrl+Space Space", "Quick Switcher", "Fuzzy switcher ordered by most-recently-used and alerts"),
            ],
        ),
        (
            "3. Layout Engine & Tiling",
            "Binary tree tiling organizes splits. Ratio-based sizing allows resizing without process restarts.",
            vec![
                ("Ctrl+Space |", "Split Vertical", "Split focused pane side-by-side (left/right)"),
                ("Ctrl+Space -", "Split Horizontal", "Split focused pane top-and-bottom"),
                ("Ctrl+Space x", "Close Pane", "Terminate child process and reclaim layout space"),
                ("Alt+Enter / Ctrl+Space z", "Toggle Zoom", "Fullscreen the active pane without altering tree state"),
                ("Ctrl+Space Z", "Undo Layout", "Revert accidental splits, closes, or resizes from history"),
                ("Mouse Drag Border", "Resize Border", "Click and drag pane borders to adjust layout ratios"),
            ],
        ),
        (
            "4. Tab Tray (Cross-Workspace Manager)",
            "The Tab Tray (Ctrl+Space b) shows a unified tree of all panes across every workspace.",
            vec![
                ("Type letters", "Live Filter", "Filter rows instantly by pane title, branch, or workspace"),
                ("Enter / Left Click", "Switch / Focus", "Jump to that pane, switching workspaces automatically"),
                ("s / v keys", "Open in Split", "Pull selected pane into the current view as a vertical/horizontal split"),
                ("Shift+Enter", "Swap Panes", "Swap the focused pane's position with the selected pane"),
                ("x key", "Close Pane", "Close the selected pane directly from the tray"),
                ("z key", "Zoom Pane", "Focus and zoom into the selected pane"),
            ],
        ),
        (
            "5. Autonomous Agent Supervision",
            "Monitors autonomous coding agents (Claude Code, Codex, Antigravity/agy) via layered status detection.",
            vec![
                ("Ctrl+Space a", "Attention Queue", "Ordered list of agent requests needing permission or user answers"),
                ("Enter in Queue", "Quick Jump", "Jumps straight into the alerted pane with input focused"),
                ("Ctrl+Space n", "Toggle DND", "Do Not Disturb: mute desktop notifications and audible bells"),
                ("Layer 1 (Hooks)", "Socket IPC", "cockpit-notify communicates directly with COCKPIT_SOCK"),
                ("Layer 2 (Signals)", "Terminal Signals", "Captures terminal BELL and OSC 9 / OSC 777 escape sequences"),
                ("Layer 3 (Patterns)", "Output Heuristics", "Monitors approval prompts and question markers via regex"),
            ],
        ),
        (
            "6. Token Usage & Cost Tracking",
            "Read-only parsers tail agent transcripts and record token counts into a persistent SQLite database.",
            vec![
                ("Ctrl+Space u", "Usage Dashboard", "View today's tokens, rolling 5-hour window, and estimated costs"),
                ("Status Bar Badge", "Live Summary", "Displays current session tokens and dollar estimate in status line"),
                ("pricing.toml", "Model Rates", "Configurable pricing per million input, output, and cache tokens"),
                ("Claude Code", "Project Transcripts", "Tails JSONL under ~/.claude/projects/"),
                ("Codex CLI", "Session Logs", "Tails token_count events under ~/.codex/sessions/"),
                ("Antigravity (agy)", "Brain Transcripts", "Tails step logs under ~/.gemini/antigravity-cli/brain/"),
            ],
        ),
        (
            "7. Git Worktrees & Diff Viewer",
            "Orchestrates Git worktrees so agents and developers can work on isolated branches concurrently.",
            vec![
                ("Ctrl+Space g", "Git Overlay", "Inspect worktrees, branch status, changed files, and unified diff"),
                ("Diff Navigation", "Scroll Diff", "Review changes with colored additions (+), deletions (-), and hunks"),
                ("Worktree Binding", "Workspace Binding", "Bind workspaces to worktrees so new panes spawn in correct directory"),
            ],
        ),
        (
            "8. Clipboard & Snippet Bank",
            "Structured macro bank and clipboard image bridge for rapid prompt injection.",
            vec![
                ("Ctrl+Space s", "Snippet Bank", "Searchable catalog of templated prompts and macros"),
                ("Enter on Snippet", "Bracketed Paste", "Injects multi-line text safely using bracketed paste escapes"),
                ("{{variable}}", "Templating", "Substitutes variables dynamically before injection"),
                ("Ctrl+Space i", "Image Bridge", "Extracts clipboard image, saves to .orchestrator/cache/, and pastes path"),
            ],
        ),
        (
            "9. Secrets Vault & Environment Injection",
            "Encrypted credential store at rest using Argon2id and ChaCha20-Poly1305 authenticated encryption.",
            vec![
                ("Ctrl+Space v", "Secrets Vault", "Unlock vault with passphrase, view keys, and configure env sets"),
                ("Auto Injection", "PTY Spawn Injection", "Injects API keys into agent environments without shell history leaks"),
                ("Auto-Lock", "Idle Timeout", "Zeroizes memory and locks vault after configured idle period"),
            ],
        ),
        (
            "10. Mouse & Selection Mechanics",
            "Independent 50,000-line scrollback ring buffer per pane decoupled from child processes.",
            vec![
                ("Mouse Wheel / Trackpad", "Scroll Viewport", "Scrolls history without pausing child process output"),
                ("Left Click Drag", "Text Selection", "Character-level rectangular text selection in terminal"),
                ("Double Click", "Select Word", "Selects the word under the mouse cursor"),
                ("Triple Click", "Select Line", "Selects the entire row under the mouse cursor"),
                ("Shift + Click Drag", "Terminal Bypass", "Bypasses in-app mouse tracking for native emulator selection"),
            ],
        ),
        (
            "11. Applets",
            "Built-in non-PTY panes proving the extensible Pane trait architecture.",
            vec![
                ("Peer Chat", "Team Room", "Collaborative developer chat pane"),
                ("Texas Hold'em", "Mini-Game", "Terminal poker game pane with chip counts and community cards"),
            ],
        ),
    ];

    let mut lines = Vec::new();

    for (sec_title, sec_desc, items) in sections {
        lines.push(Line::from(vec![Span::styled(
            format!("── {} ──", sec_title),
            theme.title_focused.add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(vec![Span::styled(
            format!("   {}", sec_desc),
            theme.text.add_modifier(Modifier::DIM),
        )]));
        lines.push(Line::raw(""));

        for (key, action, desc) in items {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("   {:<28}", key),
                    theme.accent.add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{:<22}", action), theme.info),
                Span::styled(desc, theme.text),
            ]));
        }
        lines.push(Line::raw(""));
    }

    let visible_height = inner.height as usize;
    let scrolled_lines: Vec<Line<'static>> = lines
        .into_iter()
        .skip(scroll_offset)
        .take(visible_height)
        .collect();

    let p = Paragraph::new(scrolled_lines);
    frame.render_widget(p, inner);
}
