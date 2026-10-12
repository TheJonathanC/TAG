use portable_pty::{CommandBuilder, MasterPty, PtyPair, PtySize};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line as RatLine, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info};

use tag_core::event::{AppEvent, EventResult};
use tag_core::pane::{Pane, PaneId, PaneKind};
use tag_core::theme::Theme;

use crate::ring_buffer::ScrollbackRing;
use crate::selection::{Coord, Selection, SelectionMode};
use crate::vt_grid::VtGrid;

pub struct PtyPane {
    id: PaneId,
    title: String,
    kind: PaneKind,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    vt: VtGrid,
    ring: ScrollbackRing,
    scroll_offset: usize,
    selection: Option<Selection>,
    is_alive: Arc<AtomicBool>,
    exit_code: Option<i32>,
    cwd: Option<PathBuf>,
    branch: Option<String>,
    width: u16,
    height: u16,
    needs_attention: bool,
    agent_status_text: Option<&'static str>,
    child_pid: Option<u32>,
    command: Option<String>,
    args: Vec<String>,
}

impl PtyPane {
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        id: PaneId,
        title: String,
        kind: PaneKind,
        command: Option<String>,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: Option<PathBuf>,
        width: u16,
        height: u16,
        scrollback_capacity: usize,
        event_tx: UnboundedSender<AppEvent>,
    ) -> Result<Self, anyhow::Error> {
        let pty_system = portable_pty::native_pty_system();
        let pair: PtyPair = pty_system.openpty(PtySize {
            rows: height.max(1),
            cols: width.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let shell = command
            .clone()
            .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()));

        let mut cmd = CommandBuilder::new(&shell);
        cmd.args(args);
        if let Some(ref dir) = cwd {
            cmd.cwd(dir);
        }

        // Inject standard cockpit environment variables
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("COCKPIT_PANE_ID", id.0.to_string());

        let child = pair.slave.spawn_command(cmd)?;
        let child_pid = child.process_id();
        drop(pair.slave); // Close slave handle in master process

        let master: Box<dyn MasterPty + Send> = pair.master;
        let mut reader = master.try_clone_reader()?;
        let writer: Box<dyn Write + Send> = master.take_writer()?;

        let master_arc = Arc::new(Mutex::new(master));
        let writer_arc = Arc::new(Mutex::new(writer));
        let is_alive = Arc::new(AtomicBool::new(true));

        // Spawn dedicated reader thread
        let tx = event_tx.clone();
        let pane_id = id;
        let alive_flag = is_alive.clone();

        thread::Builder::new()
            .name(format!("pty-reader-{}", id.0))
            .spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            info!("PTY EOF for pane {}", pane_id.0);
                            break;
                        }
                        Ok(n) => {
                            let data = buf[..n].to_vec();
                            if tx.send(AppEvent::PtyOutput { pane_id, data }).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            error!("PTY read error for pane {}: {}", pane_id.0, e);
                            break;
                        }
                    }
                }
                alive_flag.store(false, Ordering::SeqCst);
                let _ = tx.send(AppEvent::PtyExited {
                    pane_id,
                    exit_code: None,
                });
            })?;

        let vt = VtGrid::new(width.max(1), height.max(1), scrollback_capacity);
        let ring = ScrollbackRing::new(scrollback_capacity);

        Ok(Self {
            id,
            title,
            kind,
            master: master_arc,
            writer: writer_arc,
            vt,
            ring,
            scroll_offset: 0,
            selection: None,
            is_alive,
            exit_code: None,
            cwd,
            branch: None,
            width: width.max(1),
            height: height.max(1),
            needs_attention: false,
            agent_status_text: None,
            child_pid,
            command,
            args: args.to_vec(),
        })
    }

    pub fn write_bytes(&self, data: &[u8]) {
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.write_all(data);
            let _ = writer.flush();
        }
    }

    pub fn set_agent_status(&mut self, status: Option<&'static str>) {
        self.agent_status_text = status;
    }

    pub fn set_needs_attention(&mut self, attention: bool) {
        self.needs_attention = attention;
    }

    pub fn needs_attention(&self) -> bool {
        self.needs_attention
    }

    pub fn set_branch(&mut self, branch: Option<String>) {
        self.branch = branch;
    }

    pub fn start_selection(&mut self, col: u16, row: u16, mode: SelectionMode) {
        self.selection = Some(Selection::new(Coord { col, row }, mode));
    }

    pub fn update_selection(&mut self, col: u16, row: u16) {
        if let Some(sel) = &mut self.selection {
            sel.update(Coord { col, row });
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }
}

impl Pane for PtyPane {
    fn id(&self) -> PaneId {
        self.id
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn kind(&self) -> PaneKind {
        self.kind
    }

    fn is_active(&self) -> bool {
        self.is_alive.load(Ordering::SeqCst)
    }

    fn send_input(&mut self, data: &[u8]) {
        self.write_bytes(data);
    }

    fn scroll(&mut self, delta: i32) {
        if self.vt.is_alternate() {
            let mouse_mode = self.vt.mouse_protocol_mode();
            if mouse_mode != vt100::MouseProtocolMode::None {
                let encoding = self.vt.mouse_protocol_encoding();
                let (cursor_col, cursor_row) = self.vt.cursor_position();
                let col = (cursor_col + 1).max(1);
                let row = (cursor_row + 1).max(1);
                let code = if delta > 0 { 64 } else { 65 };
                let count = delta.unsigned_abs() as usize;

                for _ in 0..count {
                    match encoding {
                        vt100::MouseProtocolEncoding::Sgr => {
                            let seq = format!("\x1b[<{};{};{}M", code, col, row);
                            self.write_bytes(seq.as_bytes());
                        }
                        vt100::MouseProtocolEncoding::Default => {
                            let cb = (code + 32) as u8;
                            let cx = (col.min(223) + 32) as u8;
                            let cy = (row.min(223) + 32) as u8;
                            self.write_bytes(&[0x1b, b'[', b'M', cb, cx, cy]);
                        }
                        _ => {
                            let seq = format!("\x1b[<{};{};{}M", code, col, row);
                            self.write_bytes(seq.as_bytes());
                        }
                    }
                }
            } else {
                let arrow = if delta > 0 { b"\x1b[A" } else { b"\x1b[B" };
                let count = delta.unsigned_abs() as usize;
                for _ in 0..count {
                    self.write_bytes(arrow);
                }
            }
        } else {
            if delta < 0 {
                self.scroll_offset = self.scroll_offset.saturating_sub((-delta) as usize);
            } else {
                self.scroll_offset = self.scroll_offset.saturating_add(delta as usize);
            }
            let actual = self.vt.set_scrollback(self.scroll_offset);
            self.scroll_offset = actual;
        }
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.vt.set_scrollback(0);
    }

    fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    fn copy_selection(&mut self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let mut lines = Vec::new();
        for r in 0..self.height {
            lines.push(self.vt.get_row_text(r));
        }
        let text = sel.extract_text(&lines);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    fn on_resize(&mut self, width: u16, height: u16) {
        if width == 0 || height == 0 {
            return;
        }
        self.width = width;
        self.height = height;
        self.vt.resize(width, height);

        if let Ok(master) = self.master.lock() {
            let _ = master.resize(PtySize {
                rows: height,
                cols: width,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }

    fn handle_event(&mut self, event: &AppEvent) -> EventResult {
        match event {
            AppEvent::PtyOutput { pane_id, data } if *pane_id == self.id => {
                self.vt.process(data, &mut self.ring);
                EventResult::Consumed
            }
            AppEvent::PtyExited { pane_id, exit_code } if *pane_id == self.id => {
                self.is_alive.store(false, Ordering::SeqCst);
                self.exit_code = *exit_code;
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, is_focused: bool, theme: &Theme) {
        if area.width < 2 || area.height < 2 {
            return;
        }

        let inner_width = area.width.saturating_sub(2);
        let inner_height = area.height.saturating_sub(2);

        if inner_width != self.width || inner_height != self.height {
            self.on_resize(inner_width, inner_height);
        }

        let border_style = if self.needs_attention {
            theme.warning.add_modifier(Modifier::BOLD)
        } else if is_focused {
            theme.border_focused
        } else {
            theme.border_unfocused
        };

        // Construct title bar with metadata and badges
        let mut title_spans = vec![
            Span::styled(format!(" {} ", self.kind.icon()), theme.text),
            Span::styled(
                format!("{} ", self.title),
                if is_focused {
                    theme.title_focused
                } else {
                    theme.title_unfocused
                },
            ),
        ];

        if let Some(branch) = &self.branch {
            title_spans.push(Span::styled(
                format!("git: {} ", branch),
                theme.info.add_modifier(Modifier::DIM),
            ));
        }

        if let Some(status) = self.agent_status_text {
            title_spans.push(Span::styled(
                format!(" [{}] ", status),
                theme.accent.add_modifier(Modifier::BOLD),
            ));
        }

        if !self.is_active() {
            title_spans.push(Span::styled(
                format!(
                    " [Exited: {}] ",
                    self.exit_code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "EOF".to_string())
                ),
                theme.error,
            ));
        }

        if self.scroll_offset > 0 {
            title_spans.push(Span::styled(
                format!(" [Scrolled: -{} lines] ", self.scroll_offset),
                theme.warning,
            ));
        }

        let close_btn = Span::styled(
            "[x] ",
            if is_focused {
                theme.error
            } else {
                theme.text.add_modifier(Modifier::DIM)
            },
        );

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(RatLine::from(title_spans))
            .title(RatLine::from(vec![close_btn]).alignment(Alignment::Right));

        let inner_area = block.inner(area);
        frame.render_widget(block, area);

        // Render viewport text lines
        let rendered_lines = self.vt.render_viewport(self.scroll_offset, &self.ring);
        let paragraph = Paragraph::new(rendered_lines);
        frame.render_widget(paragraph, inner_area);

        // Render cursor if focused, active, and not scrolled
        if is_focused && self.is_active() && self.scroll_offset == 0 {
            let (col, row) = self.vt.cursor_position();
            if col < inner_area.width && row < inner_area.height {
                frame.set_cursor_position((inner_area.x + col, inner_area.y + row));
            }
        }
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.cwd.clone()
    }

    fn branch(&self) -> Option<String> {
        self.branch.clone()
    }

    fn agent_status(&self) -> Option<&'static str> {
        self.agent_status_text
    }

    fn running_process(&self) -> (Option<String>, Vec<String>, Option<PathBuf>) {
        #[cfg(target_os = "linux")]
        if let Some(pid) = self.child_pid {
            if let Some((cmd, args, cwd)) = get_foreground_process(pid) {
                let cmd_opt = if cmd.is_empty() { None } else { Some(cmd) };
                return (cmd_opt, args, Some(cwd));
            }
        }

        (self.command.clone(), self.args.clone(), self.cwd.clone())
    }
}

#[cfg(target_os = "linux")]
fn get_foreground_process(shell_pid: u32) -> Option<(String, Vec<String>, PathBuf)> {
    let mut current_pid = shell_pid;
    for _ in 0..10 {
        let children_path = format!("/proc/{}/task/{}/children", current_pid, current_pid);
        if let Ok(children_str) = std::fs::read_to_string(&children_path) {
            let pids: Vec<u32> = children_str
                .split_whitespace()
                .filter_map(|s| s.parse::<u32>().ok())
                .collect();
            if let Some(&last_child) = pids.last() {
                current_pid = last_child;
                continue;
            }
        }
        break;
    }

    let cwd = std::fs::read_link(format!("/proc/{}/cwd", current_pid))
        .or_else(|_| std::fs::read_link(format!("/proc/{}/cwd", shell_pid)))
        .ok()?;

    let cmdline_bytes = std::fs::read(format!("/proc/{}/cmdline", current_pid)).ok()?;
    if cmdline_bytes.is_empty() {
        return None;
    }

    let parts: Vec<String> = cmdline_bytes
        .split(|&b| b == 0)
        .filter(|slice| !slice.is_empty())
        .map(|slice| String::from_utf8_lossy(slice).to_string())
        .collect();

    if parts.is_empty() {
        return None;
    }

    let cmd = parts[0].clone();
    let args = parts[1..].to_vec();

    let cmd_name = std::path::Path::new(&cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&cmd);

    let is_shell = matches!(cmd_name, "bash" | "zsh" | "fish" | "sh" | "dash");
    if current_pid == shell_pid && is_shell {
        return Some((String::new(), Vec::new(), cwd));
    }

    Some((cmd, args, cwd))
}
