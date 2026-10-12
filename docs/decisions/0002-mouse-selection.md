# ADR 0002: Mouse Selection and Terminal Passthrough

## Context
When a TUI multiplexer enables mouse capture (`EnableMouseCapture` in crossterm), mouse events (clicks, drags, scrolls) are captured by the multiplexer and are not handled natively by the parent terminal emulator. Users expect standard text selection and copy behavior.

## Decision
1. Implement in-app mouse selection across the visible grid and scrollback:
   - Left-click and drag: character-level selection box.
   - Double-click: select word under cursor.
   - Triple-click: select entire row under cursor.
   - On release or copy shortcut: copy selected text to OS clipboard via `arboard`.
2. Honor terminal modifier bypass: most modern terminal emulators (Alacritty, Kitty, iTerm2, WezTerm) bypass application mouse tracking when holding `Shift`. Document Shift-drag for native terminal selection.
3. Support trackpad and wheel scrolling without sending mouse sequences to child unless the child specifically requested terminal mouse reporting (e.g. mouse mode in vim/tmux).
