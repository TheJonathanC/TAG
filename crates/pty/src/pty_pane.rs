use portable_pty::{Child, CommandBuilder, MasterPty, PtyPair, PtySize};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line as RatLine, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::collections::{HashMap, HashSet};
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

type ForegroundProcCache = Option<(
    Option<String>,
    Vec<String>,
    Option<PathBuf>,
    std::time::Instant,
)>;

pub struct PtyPane {
    id: PaneId,
    title: String,
    kind: PaneKind,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Option<Box<dyn Child + Send + Sync>>>>,
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
    is_zoomed: bool,
    cached_process_kind: Mutex<Option<(PaneKind, std::time::Instant)>>,
    cached_tmux_agent: Mutex<Option<(String, Option<String>, std::time::Instant)>>,
    cached_tmux_donut: Mutex<Option<(String, bool, std::time::Instant)>>,
    cached_fg_proc: Mutex<ForegroundProcCache>,
}

pub fn clean_tmux_session_name(name: &str) -> String {
    let s = name.split([' ', '(']).next().unwrap_or(name).trim();
    if let Some(idx) = s.rfind("-tag-") {
        let suffix = &s[idx + 5..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            return s[..idx].to_string();
        }
    }
    s.to_string()
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
        cmd.env("TAG_ACTIVE", "1");
        cmd.env("TAG_PANE_ID", id.0.to_string());
        if let Ok(sock) = std::env::var("COCKPIT_SOCK") {
            cmd.env("COCKPIT_SOCK", sock);
        }

        let child = pair.slave.spawn_command(cmd)?;
        let child_pid = child.process_id();
        let child_arc = Arc::new(Mutex::new(Some(child)));
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
                let mut buf = [0u8; 32768];
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
            child: child_arc,
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
            is_zoomed: false,
            cached_process_kind: Mutex::new(None),
            cached_tmux_agent: Mutex::new(None),
            cached_tmux_donut: Mutex::new(None),
            cached_fg_proc: Mutex::new(None),
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

    pub fn kill_child(&mut self) {
        self.is_alive.store(false, Ordering::SeqCst);
        if let Ok(mut lock) = self.child.lock() {
            if let Some(mut child) = lock.take() {
                let _ = child.kill();
            }
        }
    }

    pub fn tmux_session_name(&self) -> Option<String> {
        if let Some(idx) = self.args.iter().position(|a| a == "-s" || a == "-t") {
            if let Some(sess) = self.args.get(idx + 1) {
                let clean = clean_tmux_session_name(sess);
                if !clean.is_empty() {
                    return Some(clean);
                }
            }
        }
        if self.title.starts_with("tmux: ") {
            let rest = &self.title["tmux: ".len()..];
            let clean = clean_tmux_session_name(rest);
            if !clean.is_empty() {
                return Some(clean);
            }
        }
        #[cfg(target_os = "linux")]
        if let Some(pid) = self.child_pid {
            let tty = std::fs::read_link(format!("/proc/{}/fd/0", pid))
                .or_else(|_| std::fs::read_link(format!("/proc/{}/fd/1", pid)))
                .ok()
                .map(|p| p.to_string_lossy().to_string());
            let deepest = get_deepest_child_pid(pid).unwrap_or(pid);
            if let Some(sess) = get_tmux_session_for_client_or_tty(deepest, tty.as_deref()) {
                let clean = clean_tmux_session_name(&sess);
                if !clean.is_empty() {
                    return Some(clean);
                }
            }

            if let Some((cmd, args, _)) = get_foreground_process(pid) {
                let cmd_name = std::path::Path::new(&cmd)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&cmd);
                if cmd_name == "tmux" {
                    if let Some(idx) = args.iter().position(|a| a == "-s" || a == "-t") {
                        if let Some(sess) = args.get(idx + 1) {
                            let clean = clean_tmux_session_name(sess);
                            if !clean.is_empty() {
                                return Some(clean);
                            }
                        }
                    }
                    // If only 1 tmux session exists, this tmux client must be attached to it
                    if let Ok(output) = std::process::Command::new("tmux")
                        .args(["list-sessions", "-F", "#{session_name}"])
                        .output()
                    {
                        if output.status.success() {
                            let text = String::from_utf8_lossy(&output.stdout);
                            let sessions: Vec<&str> = text
                                .lines()
                                .map(|l| l.trim())
                                .filter(|l| !l.is_empty())
                                .collect();
                            if sessions.len() == 1 {
                                let clean = clean_tmux_session_name(sessions[0]);
                                return Some(clean);
                            }
                        }
                    }
                }
            }
        }
        None
    }
    pub fn cached_detect_tmux_agent(&self, session: &str) -> Option<String> {
        if let Ok(mut cache) = self.cached_tmux_agent.lock() {
            if let Some((ref sess, ref agent, ts)) = *cache {
                if sess == session && ts.elapsed() < std::time::Duration::from_millis(1500) {
                    return agent.clone();
                }
            }
            let agent = detect_agent_in_tmux(session);
            *cache = Some((
                session.to_string(),
                agent.clone(),
                std::time::Instant::now(),
            ));
            agent
        } else {
            detect_agent_in_tmux(session)
        }
    }

    pub fn cached_detect_tmux_donut(&self, session: &str) -> bool {
        if let Ok(mut cache) = self.cached_tmux_donut.lock() {
            if let Some((ref sess, donut, ts)) = *cache {
                if sess == session && ts.elapsed() < std::time::Duration::from_millis(1500) {
                    return donut;
                }
            }
            let donut = detect_donut_in_tmux(session);
            *cache = Some((session.to_string(), donut, std::time::Instant::now()));
            donut
        } else {
            detect_donut_in_tmux(session)
        }
    }

    fn compute_kind(&self) -> PaneKind {
        if let Some(session) = self.tmux_session_name() {
            if self.cached_detect_tmux_donut(&session) {
                return PaneKind::Donut;
            }
            if self.cached_detect_tmux_agent(&session).is_some() {
                return PaneKind::Agent;
            }
            return PaneKind::Tmux;
        }

        #[cfg(target_os = "linux")]
        if let Some(pid) = self.child_pid {
            if let Some((cmd, args, _cwd)) = get_foreground_process(pid) {
                let cmd_name = std::path::Path::new(&cmd)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&cmd);
                if cmd_name == "tmux" {
                    if let Some(session) = self.tmux_session_name() {
                        if self.cached_detect_tmux_donut(&session) {
                            return PaneKind::Donut;
                        }
                        if self.cached_detect_tmux_agent(&session).is_some() {
                            return PaneKind::Agent;
                        }
                    }
                    return PaneKind::Tmux;
                } else if is_donut_process(cmd_name, &args) {
                    return PaneKind::Donut;
                } else if is_known_agent(cmd_name) {
                    return PaneKind::Agent;
                }
            }
        }
        if self.kind == PaneKind::Tmux {
            if let Some(session) = self.tmux_session_name() {
                if self.cached_detect_tmux_donut(&session) {
                    return PaneKind::Donut;
                }
                if self.cached_detect_tmux_agent(&session).is_some() {
                    return PaneKind::Agent;
                }
            }
        }
        if let Some(ref cmd) = self.command {
            if is_donut_process(cmd, &self.args) {
                return PaneKind::Donut;
            }
        }
        self.kind
    }
}

impl Pane for PtyPane {
    fn id(&self) -> PaneId {
        self.id
    }

    fn title(&self) -> String {
        if let Some(session) = self.tmux_session_name() {
            if self.cached_detect_tmux_donut(&session) {
                return format!("tmux: {} (donut)", session);
            }
            if let Some(agent) = self.cached_detect_tmux_agent(&session) {
                return format!("tmux: {} ({})", session, agent);
            }
            return format!("tmux: {}", session);
        } else if self.kind == PaneKind::Tmux || self.title.starts_with("tmux:") {
            return self.title.clone();
        }
        self.title.clone()
    }

    fn set_title(&mut self, title: String) {
        self.title = title;
    }

    fn set_zoomed(&mut self, zoomed: bool) {
        self.is_zoomed = zoomed;
    }

    fn kind(&self) -> PaneKind {
        if let Ok(mut cache) = self.cached_process_kind.lock() {
            if let Some((k, ts)) = *cache {
                if ts.elapsed() < std::time::Duration::from_millis(1500) {
                    return k;
                }
            }
            let computed = self.compute_kind();
            *cache = Some((computed, std::time::Instant::now()));
            computed
        } else {
            self.compute_kind()
        }
    }

    fn is_active(&self) -> bool {
        self.is_alive.load(Ordering::SeqCst)
    }

    fn send_input(&mut self, data: &[u8]) {
        self.write_bytes(data);
    }

    fn scroll(&mut self, delta: i32) {
        let (cursor_col, cursor_row) = self.vt.cursor_position();
        let col = if cursor_col > 0 && cursor_col <= self.width {
            cursor_col + 1
        } else {
            (self.width / 2).max(1)
        };
        let row = if cursor_row > 0 && cursor_row < self.height.saturating_sub(1) {
            cursor_row + 1
        } else {
            (self.height / 2).max(1)
        };
        self.scroll_at(delta, col, row);
    }

    fn scroll_at(&mut self, delta: i32, col: u16, row: u16) {
        if self.vt.is_alternate() {
            let mouse_mode = self.vt.mouse_protocol_mode();
            let encoding = self.vt.mouse_protocol_encoding();
            let is_tmux = self.kind() == PaneKind::Tmux
                || self.command.as_deref() == Some("tmux")
                || self.title.starts_with("tmux");

            if mouse_mode != vt100::MouseProtocolMode::None || is_tmux {
                let code = if delta > 0 { 64 } else { 65 };
                let count = if delta.abs() <= 3 {
                    1
                } else {
                    (delta.abs() / 3).max(1) as usize
                };

                for _ in 0..count {
                    match encoding {
                        vt100::MouseProtocolEncoding::Sgr => {
                            let seq = format!("\x1b[<{};{};{}M", code, col, row);
                            self.write_bytes(seq.as_bytes());
                        }
                        vt100::MouseProtocolEncoding::Default => {
                            if is_tmux && mouse_mode == vt100::MouseProtocolMode::None {
                                let seq = format!("\x1b[<{};{};{}M", code, col, row);
                                self.write_bytes(seq.as_bytes());
                            } else {
                                let cb = (code + 32) as u8;
                                let cx = (col.min(223) + 32) as u8;
                                let cy = (row.min(223) + 32) as u8;
                                self.write_bytes(&[0x1b, b'[', b'M', cb, cx, cy]);
                            }
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

    fn start_selection(&mut self, col: u16, row: u16) {
        self.selection = Some(Selection::new(Coord { col, row }, SelectionMode::Normal));
    }

    fn update_selection(&mut self, col: u16, row: u16) {
        if let Some(sel) = &mut self.selection {
            sel.update(Coord { col, row });
        }
    }

    fn clear_selection(&mut self) {
        self.selection = None;
    }

    fn has_selection(&self) -> bool {
        self.selection.is_some()
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
        let title_str = self.title();
        let mut title_spans = vec![
            Span::styled(format!(" {} ", self.kind().icon()), theme.text),
            Span::styled(
                format!("{} ", title_str),
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

        if let Some(status) = self.agent_status() {
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

        if self.is_zoomed {
            title_spans.push(Span::styled(
                " [MAXIMIZED] ",
                theme.accent.add_modifier(Modifier::BOLD),
            ));
        }

        let min_btn = Span::styled(
            "[_] ",
            if is_focused {
                theme.warning
            } else {
                theme.text.add_modifier(Modifier::DIM)
            },
        );

        let zoom_btn = if self.is_zoomed {
            Span::styled("[-] ", theme.warning.add_modifier(Modifier::BOLD))
        } else {
            Span::styled("[^] ", theme.text.add_modifier(Modifier::DIM))
        };

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
            .title(RatLine::from(vec![min_btn, zoom_btn, close_btn]).alignment(Alignment::Right));

        let inner_area = block.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);

        // Render viewport text lines
        let rendered_lines =
            self.vt
                .render_viewport(self.scroll_offset, &self.ring, self.selection.as_ref());
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
        if let Some(session) = self.tmux_session_name() {
            if self.cached_detect_tmux_agent(&session).is_some() {
                return Some("running");
            }
        }
        if let Some(status) = self.agent_status_text {
            return Some(status);
        }
        None
    }

    fn running_process(&self) -> (Option<String>, Vec<String>, Option<PathBuf>) {
        if let Ok(mut cache) = self.cached_fg_proc.lock() {
            if let Some((ref cmd, ref args, ref cwd, ts)) = *cache {
                if ts.elapsed() < std::time::Duration::from_millis(1500) {
                    return (cmd.clone(), args.clone(), cwd.clone());
                }
            }
            #[cfg(target_os = "linux")]
            if let Some(pid) = self.child_pid {
                if let Some((cmd, args, cwd)) = get_foreground_process(pid) {
                    let cmd_opt = if cmd.is_empty() { None } else { Some(cmd) };
                    let res = (cmd_opt, args, Some(cwd));
                    *cache = Some((
                        res.0.clone(),
                        res.1.clone(),
                        res.2.clone(),
                        std::time::Instant::now(),
                    ));
                    return res;
                }
            }
            let res = (self.command.clone(), self.args.clone(), self.cwd.clone());
            *cache = Some((
                res.0.clone(),
                res.1.clone(),
                res.2.clone(),
                std::time::Instant::now(),
            ));
            return res;
        }

        (self.command.clone(), self.args.clone(), self.cwd.clone())
    }

    fn kill(&mut self) {
        self.kill_child();
    }

    fn tmux_session_name(&self) -> Option<String> {
        self.tmux_session_name()
    }
}

impl Drop for PtyPane {
    fn drop(&mut self) {
        self.kill_child();
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

    if cmd_name == "tmux" {
        let session = args.windows(2).find(|w| w[0] == "-t").map(|w| &w[1]);
        let mut tmux_cmd = std::process::Command::new("tmux");
        tmux_cmd.arg("display-message");
        if let Some(s) = session {
            tmux_cmd.args(["-t", s]);
        }
        tmux_cmd.args(["-p", "#{pane_current_path}"]);
        if let Ok(output) = tmux_cmd.output() {
            if output.status.success() {
                let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path_str.is_empty() {
                    let p = PathBuf::from(path_str);
                    if p.exists() {
                        return Some((cmd, args, p));
                    }
                }
            }
        }
    }

    let is_shell = matches!(cmd_name, "bash" | "zsh" | "fish" | "sh" | "dash");
    if current_pid == shell_pid && is_shell {
        return Some((String::new(), Vec::new(), cwd));
    }

    Some((cmd, args, cwd))
}

#[cfg(target_os = "linux")]
pub fn get_deepest_child_pid(shell_pid: u32) -> Option<u32> {
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
    if current_pid != shell_pid {
        Some(current_pid)
    } else {
        None
    }
}

type ClientCache = Mutex<
    Option<(
        HashMap<u32, String>,
        HashMap<String, String>,
        std::time::Instant,
    )>,
>;

pub fn get_tmux_session_for_client_or_tty(client_pid: u32, tty: Option<&str>) -> Option<String> {
    static CLIENT_CACHE: std::sync::LazyLock<ClientCache> =
        std::sync::LazyLock::new(|| Mutex::new(None));

    if let Ok(cache) = CLIENT_CACHE.lock() {
        if let Some((ref pid_map, ref tty_map, ts)) = *cache {
            if ts.elapsed() < std::time::Duration::from_millis(1500) {
                if let Some(sess) = pid_map.get(&client_pid) {
                    return Some(sess.clone());
                }
                if let Some(t) = tty {
                    if let Some(sess) = tty_map.get(t) {
                        return Some(sess.clone());
                    }
                }
            }
        }
    }

    let mut pid_map = HashMap::new();
    let mut tty_map = HashMap::new();
    if let Ok(output) = std::process::Command::new("tmux")
        .args([
            "list-clients",
            "-F",
            "#{client_pid}\t#{client_tty}\t#{client_session}",
        ])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 3 {
                    if let Ok(pid) = parts[0].trim().parse::<u32>() {
                        pid_map.insert(pid, parts[2].trim().to_string());
                    }
                    let t = parts[1].trim();
                    if !t.is_empty() {
                        tty_map.insert(t.to_string(), parts[2].trim().to_string());
                    }
                } else if parts.len() >= 2 {
                    if let Ok(pid) = parts[0].trim().parse::<u32>() {
                        pid_map.insert(pid, parts[1].trim().to_string());
                    }
                }
            }
        }
    }

    let res = pid_map
        .get(&client_pid)
        .cloned()
        .or_else(|| tty.and_then(|t| tty_map.get(t).cloned()));
    if let Ok(mut cache) = CLIENT_CACHE.lock() {
        *cache = Some((pid_map, tty_map, std::time::Instant::now()));
    }
    res
}

pub fn get_tmux_session_for_client(client_pid: u32) -> Option<String> {
    get_tmux_session_for_client_or_tty(client_pid, None)
}

type TmuxAgentCache = Mutex<HashMap<String, (Option<String>, std::time::Instant)>>;

pub fn detect_agent_in_tmux(session_name: &str) -> Option<String> {
    static TMUX_CACHE: std::sync::LazyLock<TmuxAgentCache> =
        std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

    if let Ok(cache) = TMUX_CACHE.lock() {
        if let Some((ref agent, ts)) = cache.get(session_name) {
            if ts.elapsed() < std::time::Duration::from_millis(1500) {
                return agent.clone();
            }
        }
    }

    let detected = scan_tmux_session_for_agent(session_name);

    if let Ok(mut cache) = TMUX_CACHE.lock() {
        cache.insert(
            session_name.to_string(),
            (detected.clone(), std::time::Instant::now()),
        );
    }

    detected
}

type TmuxDonutCache = Mutex<HashMap<String, (bool, std::time::Instant)>>;

pub fn detect_donut_in_tmux(session_name: &str) -> bool {
    static TMUX_DONUT_CACHE: std::sync::LazyLock<TmuxDonutCache> =
        std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

    if let Ok(cache) = TMUX_DONUT_CACHE.lock() {
        if let Some(&(detected, ts)) = cache.get(session_name) {
            if ts.elapsed() < std::time::Duration::from_millis(1500) {
                return detected;
            }
        }
    }

    let detected = scan_tmux_session_for_donut(session_name);

    if let Ok(mut cache) = TMUX_DONUT_CACHE.lock() {
        cache.insert(
            session_name.to_string(),
            (detected, std::time::Instant::now()),
        );
    }

    detected
}

fn scan_tmux_session_for_donut(session_name: &str) -> bool {
    let output = match std::process::Command::new("tmux")
        .args([
            "list-panes",
            "-s",
            "-t",
            session_name,
            "-F",
            "#{pane_pid}\t#{pane_current_command}\t#{pane_title}",
        ])
        .output()
    {
        Ok(out) => out,
        Err(_) => return false,
    };

    if !output.status.success() {
        return false;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.is_empty() {
            continue;
        }

        let pane_pid: Option<u32> = parts.first().and_then(|p| p.trim().parse().ok());
        let current_cmd = parts.get(1).map(|c| c.trim()).unwrap_or("");
        let pane_title = parts.get(2).map(|t| t.trim()).unwrap_or("");

        // 1. Direct check of pane_current_command
        if is_donut_process(current_cmd, &[]) {
            return true;
        }

        // 2. Check process tree under pane_pid
        #[cfg(target_os = "linux")]
        if let Some(pid) = pane_pid {
            if inspect_proc_tree_for_donut(pid) {
                return true;
            }
        }

        // 3. Check pane title
        if pane_title.to_lowercase().contains("donut") {
            return true;
        }
    }

    false
}

#[cfg(target_os = "linux")]
fn inspect_proc_tree_for_donut(root_pid: u32) -> bool {
    let mut pids_to_check = vec![root_pid];
    let mut visited = HashSet::new();

    while let Some(current_pid) = pids_to_check.pop() {
        if !visited.insert(current_pid) {
            continue;
        }

        // Check cmdline of current_pid
        if let Ok(cmdline_bytes) = std::fs::read(format!("/proc/{}/cmdline", current_pid)) {
            let parts: Vec<String> = cmdline_bytes
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).to_string())
                .collect();

            if !parts.is_empty() && is_donut_process(&parts[0], &parts[1..]) {
                return true;
            }
        }

        // Add children of current_pid
        let children_path = format!("/proc/{}/task/{}/children", current_pid, current_pid);
        if let Ok(children_str) = std::fs::read_to_string(&children_path) {
            for child_str in children_str.split_whitespace() {
                if let Ok(child_pid) = child_str.parse::<u32>() {
                    pids_to_check.push(child_pid);
                }
            }
        }
    }

    false
}

fn scan_tmux_session_for_agent(session_name: &str) -> Option<String> {
    let output = std::process::Command::new("tmux")
        .args([
            "list-panes",
            "-s",
            "-t",
            session_name,
            "-F",
            "#{pane_pid}\t#{pane_current_command}\t#{pane_title}",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.is_empty() {
            continue;
        }

        let pane_pid: Option<u32> = parts.first().and_then(|p| p.trim().parse().ok());
        let current_cmd = parts.get(1).map(|c| c.trim()).unwrap_or("");
        let pane_title = parts.get(2).map(|t| t.trim()).unwrap_or("");

        // 1. Direct check of pane_current_command
        if is_known_agent(current_cmd) {
            return Some(normalize_agent_name(current_cmd));
        }

        // 2. Check process tree under pane_pid (for node, python, wrapper scripts, etc.)
        #[cfg(target_os = "linux")]
        if let Some(pid) = pane_pid {
            if let Some(agent) = inspect_proc_tree_for_agent(pid) {
                return Some(agent);
            }
        }

        // 3. Check pane title
        if let Some(agent) = detect_agent_from_text(pane_title) {
            return Some(agent);
        }
    }

    None
}

#[cfg(target_os = "linux")]
fn inspect_proc_tree_for_agent(root_pid: u32) -> Option<String> {
    let mut pids_to_check = vec![root_pid];
    let mut visited = HashSet::new();

    while let Some(current_pid) = pids_to_check.pop() {
        if !visited.insert(current_pid) {
            continue;
        }

        // Check cmdline of current_pid
        if let Ok(cmdline_bytes) = std::fs::read(format!("/proc/{}/cmdline", current_pid)) {
            let parts: Vec<String> = cmdline_bytes
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).to_string())
                .collect();

            if !parts.is_empty() {
                if let Some(agent) = detect_agent_from_cmd_and_args(&parts[0], &parts[1..]) {
                    return Some(agent);
                }
            }
        }

        // Add children of current_pid
        let children_path = format!("/proc/{}/task/{}/children", current_pid, current_pid);
        if let Ok(children_str) = std::fs::read_to_string(&children_path) {
            for child_str in children_str.split_whitespace() {
                if let Ok(child_pid) = child_str.parse::<u32>() {
                    pids_to_check.push(child_pid);
                }
            }
        }
    }

    None
}

pub fn detect_agent_from_cmd_and_args(cmd: &str, args: &[String]) -> Option<String> {
    if is_known_agent(cmd) {
        return Some(normalize_agent_name(cmd));
    }
    for arg in args {
        if arg.starts_with('-') {
            continue;
        }
        let base = std::path::Path::new(arg)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(arg);
        if is_known_agent(base) {
            return Some(normalize_agent_name(base));
        }
        let lower = arg.to_lowercase();
        if lower.contains("claude-code") || lower.contains("@anthropic-ai/claude") {
            return Some("claude".to_string());
        }
        if lower.contains("antigravity-cli") {
            return Some("agy".to_string());
        }
    }
    None
}

pub fn detect_agent_from_text(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let agents = [
        ("claude", "claude"),
        ("antigravity", "agy"),
        ("agy", "agy"),
        ("codex", "codex"),
        ("aider", "aider"),
        ("goose", "goose"),
        ("cursor", "cursor"),
        ("gemini", "gemini"),
        ("copilot", "copilot"),
        ("cline", "cline"),
        ("roo", "roo"),
        ("amp", "amp"),
        ("windsurf", "windsurf"),
        ("devin", "devin"),
    ];
    for (needle, name) in agents {
        if lower.contains(needle) {
            return Some(name.to_string());
        }
    }
    None
}

pub fn normalize_agent_name(cmd: &str) -> String {
    let base = std::path::Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd)
        .to_lowercase();
    base
}

pub fn is_known_agent(cmd: &str) -> bool {
    let base = std::path::Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd)
        .to_lowercase();
    matches!(
        base.as_str(),
        "agy"
            | "claude"
            | "codex"
            | "aider"
            | "goose"
            | "cursor"
            | "gemini"
            | "copilot"
            | "cline"
            | "roo"
            | "amp"
            | "windsurf"
            | "chatgpt"
            | "ollama"
            | "open-interpreter"
            | "devin"
            | "agent"
    )
}

pub fn is_donut_process(cmd: &str, args: &[String]) -> bool {
    let base = std::path::Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd)
        .to_lowercase();
    if base == "donut" || base.starts_with("donut-") || base.starts_with("donut.") {
        return true;
    }
    for arg in args {
        let trimmed = arg.trim();
        if trimmed.starts_with('-') && trimmed != "-donut" {
            continue;
        }
        let arg_base = std::path::Path::new(trimmed)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(trimmed)
            .to_lowercase();
        if arg_base == "donut" || arg_base.starts_with("donut.") || arg_base.starts_with("donut-") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_known_agent() {
        assert!(is_known_agent("agy"));
        assert!(is_known_agent("/home/user/.local/bin/agy"));
        assert!(is_known_agent("claude"));
        assert!(is_known_agent("aider"));
        assert!(is_known_agent("goose"));
        assert!(!is_known_agent("bash"));
        assert!(!is_known_agent("zsh"));
        assert!(!is_known_agent("tmux"));
        assert!(!is_known_agent("vim"));
    }

    #[test]
    fn test_is_donut_process() {
        assert!(is_donut_process("donut", &[]));
        assert!(is_donut_process("/home/jon/.local/bin/donut", &[]));
        assert!(is_donut_process("/home/jon/.cargo/bin/donut", &[]));
        assert!(is_donut_process("python3", &["donut.py".into()]));
        assert!(is_donut_process("python", &["/path/to/donut.py".into()]));
        assert!(is_donut_process(
            "cargo",
            &["run".into(), "--bin".into(), "donut".into()]
        ));
        assert!(!is_donut_process("bash", &[]));
        assert!(!is_donut_process("agy", &[]));
        assert!(!is_donut_process("vim", &["document.txt".into()]));
    }

    #[test]
    fn test_pane_kind_donut_icon() {
        assert_eq!(PaneKind::Donut.icon(), "[donut]");
        assert_eq!(PaneKind::Shell.icon(), "[sh]");
        assert_eq!(PaneKind::Agent.icon(), "[agent]");
    }

    #[test]
    fn test_detect_agent_from_text() {
        assert_eq!(
            detect_agent_from_text("running agy v1.0"),
            Some("agy".into())
        );
        assert_eq!(
            detect_agent_from_text("Claude Code v0.2"),
            Some("claude".into())
        );
        assert_eq!(
            detect_agent_from_text("Antigravity Agent"),
            Some("agy".into())
        );
        assert_eq!(detect_agent_from_text("just random bash prompt"), None);
    }

    #[test]
    fn test_detect_agent_in_tmux_nonexistent() {
        assert_eq!(detect_agent_in_tmux("nonexistent_session_999999"), None);
    }

    #[test]
    fn test_clean_tmux_session_name() {
        assert_eq!(clean_tmux_session_name("0"), "0");
        assert_eq!(clean_tmux_session_name("a"), "a");
        assert_eq!(clean_tmux_session_name("0-tag-5"), "0");
        assert_eq!(clean_tmux_session_name("my-sess-tag-12"), "my-sess");
        assert_eq!(clean_tmux_session_name("dev (agent: agy)"), "dev");
        assert_eq!(clean_tmux_session_name("tag-session"), "tag-session");
        assert_eq!(clean_tmux_session_name("my-tag-custom"), "my-tag-custom");
    }
}
