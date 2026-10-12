# ADR 0001: PTY and VT Emulation Strategy

## Context
A terminal multiplexer requires spawning pseudo-terminal pairs (PTY) on Unix systems and accurately rendering ANSI/VT escape sequences, including colors, cursor shapes, and alternate screen buffers (for vim, htop, and interactive agent UIs).

## Decision
1. Use `portable-pty` for PTY allocation and child process management. Reader streams run on dedicated blocking OS threads, sending chunks over `tokio::sync::mpsc` channels to the main loop.
2. Use `vt100` for ANSI/VT state tracking. It provides a clean, well-tested in-memory terminal screen grid, tracks cursor position, handles alternate screen buffers, and decodes character attributes.
3. Decouple PTY reading from ratatui frame drawing. Drain PTY chunks into the `vt100` parser on receipt, mark the pane dirty, and redraw only up to 60 FPS.
