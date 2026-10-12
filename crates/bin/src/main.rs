use clap::{Parser, Subcommand};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event as CrossEvent, KeyCode, KeyModifiers,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction as LayoutDir, Layout, Rect};
use ratatui::Terminal;
use std::collections::HashMap;
use std::io::{self, stdout};
use std::path::PathBuf;
use std::time::Duration;
use tag_agents::{get_setup_hooks, AgentPane, AgentProfile, HookServer};
use tag_clipboard::{wrap_bracketed_paste, ImageBridge, SnippetBank, SnippetOverlayState};
use tag_core::action::{Action, Direction, SplitDirection};
use tag_core::command::CommandRegistry;
use tag_core::config::Config;
use tag_core::event::{AppEvent, HookEvent};
use tag_core::pane::{Pane, PaneId, PaneKind};
use tag_core::theme::Theme;
use tag_git::{GitBackend, GitOverlayState, StandardGitBackend};
use tag_keys::KeyStateMachine;
use tag_layout::{LayoutManager, SavedPane, SavedSession, SavedWorkspace, Workspace};
use tag_notify::{
    render_attention_overlay, AttentionOverlayState, AttentionQueue, Notifier, SystemNotifier,
};
use tag_pty::PtyPane;
use tag_ui::{
    get_selected_command, hit_test_status_bar, render_command_palette, render_empty_state,
    render_help_overlay, render_status_bar, render_tab_tray, render_toasts, render_tour,
    render_which_key, PaletteState, StatusBarClickTarget, StatusBarProps, TabTrayState, Toast,
    ToastLevel, TourState, TrayItem,
};
use tag_usage::{render_usage_overlay, AgyProvider, UsageOverlayState, UsageStore};
use tag_vault::{render_vault_overlay, VaultOverlayState, VaultStore};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(name = "tag")]
#[command(author = "TAG Team")]
#[command(version = "0.1.0")]
#[command(about = "TAG: Terminal Agent Orchestrator & PTY Multiplexer")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Configure agent notification hooks
    SetupHooks {
        /// Target agent (claude, codex, agy)
        agent: String,
    },
}

struct AppState {
    panes: HashMap<PaneId, Box<dyn Pane>>,
    next_pane_id: u32,
    layout_mgr: LayoutManager,
    key_state_machine: KeyStateMachine,
    command_registry: CommandRegistry,
    theme: Theme,
    config: Config,
    attention_queue: AttentionQueue,
    toasts: Vec<Toast>,
    dnd_active: bool,
    tab_tray: TabTrayState,
    palette: PaletteState,
    help_open: bool,
    help_scroll: usize,
    tour: TourState,
    attention_state: AttentionOverlayState,
    usage_state: UsageOverlayState,
    git_state: GitOverlayState,
    snippet_state: SnippetOverlayState,
    vault_state: VaultOverlayState,
    usage_store: UsageStore,
    snippets: SnippetBank,
    vault: VaultStore,
    git_backend: StandardGitBackend,
    notifier: SystemNotifier,
    event_tx: UnboundedSender<AppEvent>,
    is_dirty: bool,
    term_width: u16,
    term_height: u16,
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();

    if let Some(Commands::SetupHooks { agent }) = cli.command {
        let info = get_setup_hooks(&agent);
        println!("=== Setup Hooks for {} ===", info.agent);
        if let Some(file) = info.target_file {
            println!("Target Configuration: {}\n", file);
        }
        println!("Diff to apply:\n{}\n", info.diff);
        println!("Instructions:\n{}", info.instructions);
        return Ok(());
    }

    // Set up file logger (tracing to file, NEVER stdout/stderr)
    let log_dir = std::env::temp_dir();
    let file_appender = tracing_appender::rolling::daily(&log_dir, "tag.log");
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_writer(file_appender)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    info!("Starting TAG orchestrator...");

    // Setup terminal and panic restore hook
    setup_panic_hook();
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        EnableFocusChange
    )?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let run_res = run_app(&mut terminal).await;

    // Restore terminal cleanly
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange
    )?;
    terminal.show_cursor()?;

    if let Err(e) = run_res {
        eprintln!("TAG error: {}", e);
    }

    Ok(())
}

fn setup_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            std::io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableBracketedPaste,
            DisableFocusChange
        );
        default_hook(info);
    }));
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<(), anyhow::Error> {
    let (event_tx, mut event_rx) = unbounded_channel::<AppEvent>();

    // Start private hook socket listener
    let socket_path = PathBuf::from(format!("/tmp/tag-sock-{}.sock", std::process::id()));
    std::env::set_var("COCKPIT_SOCK", &socket_path);

    let hook_server = HookServer::new(socket_path.clone());
    let _ = hook_server.start(event_tx.clone()).await;

    // Spawn crossterm event poller
    let input_tx = event_tx.clone();
    tokio::task::spawn_blocking(move || {
        while let Ok(ev) = crossterm::event::read() {
            let app_event = match ev {
                CrossEvent::Resize(w, h) => AppEvent::Resize {
                    width: w,
                    height: h,
                },
                CrossEvent::FocusGained => AppEvent::FocusGained,
                CrossEvent::FocusLost => AppEvent::FocusLost,
                other => {
                    // Forward key/mouse directly
                    let _ = input_tx.send(AppEvent::TerminalInput(other));
                    continue;
                }
            };
            if input_tx.send(app_event).is_err() {
                break;
            }
        }
    });

    // Spawn tick interval (60 fps frame loop ~16ms)
    let tick_tx = event_tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(16));
        loop {
            interval.tick().await;
            if tick_tx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });

    // Initialize state
    let config = Config::default();
    let theme = Theme::from_config(config.theme.clone());
    let command_registry = CommandRegistry::new();
    let key_state_machine = KeyStateMachine::default();
    let layout_mgr = LayoutManager::new();
    let usage_store = UsageStore::open_in_memory()?;
    let snippets = SnippetBank::new();
    let vault_path = dirs_or_home().join(".config").join("tag").join("vault.enc");
    let vault = VaultStore::new(vault_path, config.vault_idle_timeout_mins);

    let mut state = AppState {
        panes: HashMap::new(),
        next_pane_id: 1,
        layout_mgr,
        key_state_machine,
        command_registry,
        theme,
        config,
        attention_queue: AttentionQueue::new(),
        toasts: Vec::new(),
        dnd_active: false,
        tab_tray: TabTrayState::default(),
        palette: PaletteState::default(),
        help_open: false,
        help_scroll: 0,
        tour: TourState::default(),
        attention_state: AttentionOverlayState::default(),
        usage_state: UsageOverlayState::default(),
        git_state: GitOverlayState::default(),
        snippet_state: SnippetOverlayState::default(),
        vault_state: VaultOverlayState::default(),
        usage_store,
        snippets,
        vault,
        git_backend: StandardGitBackend,
        notifier: SystemNotifier,
        event_tx: event_tx.clone(),
        is_dirty: true,
        term_width: 80,
        term_height: 24,
    };

    let term_size = terminal.size()?;
    state.term_width = term_size.width;
    state.term_height = term_size.height;

    // Restore saved session if present, otherwise start with empty workspace showing ASCII art
    let session_path = dirs_or_home()
        .join(".config")
        .join("tag")
        .join("session.json");
    if session_path.exists() {
        if let Ok(saved) = SavedSession::load_from_file(&session_path) {
            if !saved.panes.is_empty() {
                let mut max_pid = 0u32;
                for (pid, sp) in saved.panes {
                    max_pid = max_pid.max(pid.0);
                    let env = if state.vault.is_unlocked() {
                        state.vault.get_env_set()
                    } else {
                        HashMap::new()
                    };
                    if let Ok(pane) = PtyPane::spawn(
                        pid,
                        sp.title,
                        sp.kind,
                        None,
                        &[],
                        &env,
                        sp.cwd.or_else(|| std::env::current_dir().ok()),
                        term_size.width.saturating_sub(2),
                        term_size.height.saturating_sub(4),
                        state.config.scrollback_lines,
                        event_tx.clone(),
                    ) {
                        state.panes.insert(pid, Box::new(pane));
                    }
                }
                if !state.panes.is_empty() {
                    let workspaces: Vec<Workspace> =
                        saved.workspaces.into_iter().map(Into::into).collect();
                    state.layout_mgr =
                        LayoutManager::from_workspaces(workspaces, saved.active_workspace_id);
                    state.next_pane_id = max_pid + 1;
                }
            }
        }
    }

    // Initial log scan for active agents
    scan_known_agent_logs(&mut state);

    // Main event loop
    loop {
        // Render if state is dirty
        if state.is_dirty {
            terminal.draw(|f| {
                render_frame(f, &mut state);
            })?;
            state.is_dirty = false;
        }

        // Wait for next event
        let event = match event_rx.recv().await {
            Some(ev) => ev,
            None => break,
        };

        let should_quit = handle_app_event(&mut state, event);
        if should_quit {
            save_session_to_disk(&state);
            break;
        }
    }

    save_session_to_disk(&state);

    // Clean up socket file
    let _ = std::fs::remove_file(socket_path);

    Ok(())
}

fn scan_known_agent_logs(state: &mut AppState) {
    // Scan agy transcripts if present
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let brain_dir = PathBuf::from(&home).join(".gemini/antigravity-cli/brain");

    if brain_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&brain_dir) {
            for entry in entries.flatten() {
                let conv_id = entry.file_name().to_string_lossy().to_string();
                let transcript_path = entry.path().join(".system_generated/logs/transcript.jsonl");
                if transcript_path.exists() {
                    let events = AgyProvider::parse_file(&transcript_path, &conv_id);
                    for ev in events {
                        let _ = state.usage_store.insert_event(&ev);
                    }
                }
            }
        }
    }
}

fn render_frame(frame: &mut ratatui::Frame, state: &mut AppState) {
    let size = frame.area();
    if size.width < 10 || size.height < 5 {
        return;
    }

    // Layout: top pane area, bottom status bar (height 1)
    let chunks = Layout::default()
        .direction(LayoutDir::Vertical)
        .constraints([
            Constraint::Length(size.height.saturating_sub(1)),
            Constraint::Length(1),
        ])
        .split(size);

    let main_area = chunks[0];
    let status_area = chunks[1];

    // Determine tab tray split if open
    let (tray_area, content_area) = if state.tab_tray.is_open {
        let tray_w = (main_area.width * 25 / 100)
            .clamp(24, 45)
            .min(main_area.width);
        let split = Layout::default()
            .direction(LayoutDir::Horizontal)
            .constraints([
                Constraint::Length(tray_w),
                Constraint::Length(main_area.width.saturating_sub(tray_w)),
            ])
            .split(main_area);
        (Some(split[0]), split[1])
    } else {
        (None, main_area)
    };

    // 1. Render active workspace panes or empty state
    let active_panes = state.layout_mgr.active_workspace().all_panes();
    if active_panes.is_empty() {
        render_empty_state(frame, content_area, &state.theme);
    } else {
        let rects = state.layout_mgr.compute_rects(content_area);
        let focused_id = state.layout_mgr.active_workspace().focused_pane;

        for (pane_id, area) in rects {
            if let Some(pane) = state.panes.get_mut(&pane_id) {
                let is_focused = focused_id == Some(pane_id);
                pane.render(frame, area, is_focused, &state.theme);
            }
        }
    }

    // 2. Render Tab Tray if open
    if let Some(t_area) = tray_area {
        let tray_items = build_tray_items(state);
        render_tab_tray(frame, t_area, &tray_items, &state.tab_tray, &state.theme);
    }

    // 3. Render Status Bar
    let today_usage = state.usage_store.query_today().ok();
    let usage_summary = today_usage.map(|u| {
        format!(
            "{:.3}k tok (${:.3})",
            u.input_tokens as f64 / 1000.0,
            u.estimated_cost_usd
        )
    });

    render_status_bar(
        frame,
        status_area,
        StatusBarProps {
            layout_mgr: &state.layout_mgr,
            key_state: state.key_state_machine.state(),
            dnd_active: state.dnd_active,
            attention_count: state.attention_queue.count(),
            token_usage_summary: usage_summary.as_deref(),
            theme: &state.theme,
        },
    );

    // 4. Render Overlays
    if state.key_state_machine.should_show_which_key() {
        render_which_key(frame, size, state.key_state_machine.keymap(), &state.theme);
    }

    if state.palette.is_open {
        render_command_palette(
            frame,
            size,
            &state.command_registry,
            &state.palette,
            &state.theme,
        );
    }

    if state.help_open {
        render_help_overlay(
            frame,
            size,
            state.key_state_machine.keymap(),
            state.help_scroll,
            &state.theme,
        );
    }

    if state.attention_state.is_open {
        render_attention_overlay(
            frame,
            size,
            &state.attention_queue,
            &state.attention_state,
            &state.theme,
        );
    }

    if state.usage_state.is_open {
        render_usage_overlay(
            frame,
            size,
            &state.usage_store,
            &state.usage_state,
            &state.theme,
        );
    }

    if state.git_state.is_open {
        let worktrees = state
            .git_backend
            .list_worktrees(&PathBuf::from("."))
            .unwrap_or_default();
        let repo_status = state
            .git_backend
            .repo_status(&PathBuf::from("."))
            .unwrap_or_default();
        let diff = state
            .git_backend
            .get_diff(&PathBuf::from("."))
            .unwrap_or_default();
        tag_git::render_git_overlay(
            frame,
            size,
            &worktrees,
            &repo_status,
            &diff,
            &state.git_state,
            &state.theme,
        );
    }

    if state.snippet_state.is_open {
        tag_clipboard::render_snippet_overlay(
            frame,
            size,
            &state.snippets,
            &state.snippet_state,
            &state.theme,
        );
    }

    if state.vault_state.is_open {
        render_vault_overlay(frame, size, &state.vault, &state.vault_state, &state.theme);
    }

    if state.tour.is_active {
        render_tour(frame, size, &state.tour, &state.theme);
    }

    render_toasts(frame, size, &state.toasts, &state.theme);
}

fn handle_app_event(state: &mut AppState, event: AppEvent) -> bool {
    state.is_dirty = true;

    match event {
        AppEvent::Action(action) => execute_action(state, action),
        AppEvent::PtyOutput { pane_id, data } => {
            if let Some(pane) = state.panes.get_mut(&pane_id) {
                let _ = pane.handle_event(&AppEvent::PtyOutput { pane_id, data });
            }
            false
        }
        AppEvent::PtyExited { pane_id, exit_code } => {
            if let Some(pane) = state.panes.get_mut(&pane_id) {
                let _ = pane.handle_event(&AppEvent::PtyExited { pane_id, exit_code });
            }
            false
        }
        AppEvent::AgentHook(payload) => {
            info!("Agent hook event received: {:?}", payload.event);
            let _item_id = state.attention_queue.push(
                payload.pane_id,
                payload.agent.clone(),
                payload.event.clone(),
                payload.message.clone(),
                payload.worktree.clone(),
            );

            // Toast notification
            state.toasts.push(Toast {
                title: format!("{} Alert", payload.agent),
                message: payload.message.clone(),
                level: match payload.event {
                    HookEvent::Error => ToastLevel::Error,
                    HookEvent::TaskDone => ToastLevel::Success,
                    _ => ToastLevel::Warning,
                },
                created_at: chrono::Utc::now(),
                duration_secs: 5,
            });

            // Out-of-app notification if not DND
            if !state.dnd_active && state.config.notifications.desktop_notifications {
                state
                    .notifier
                    .notify(&format!("TAG - {}", payload.agent), &payload.message);
            }

            // Route to target pane
            if let Some(pid) = payload.pane_id {
                if let Some(pane) = state.panes.get_mut(&pid) {
                    let _ = pane.handle_event(&AppEvent::AgentHook(payload));
                }
            }
            false
        }
        AppEvent::TerminalInput(CrossEvent::Key(key)) => {
            // Check modal overlays first
            if state.palette.is_open {
                match key.code {
                    KeyCode::Esc => {
                        state.palette.is_open = false;
                    }
                    KeyCode::Up => {
                        state.palette.selected_index =
                            state.palette.selected_index.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        state.palette.selected_index += 1;
                    }
                    KeyCode::Enter => {
                        if let Some(cmd) =
                            get_selected_command(&state.command_registry, &state.palette)
                        {
                            let act = cmd.action.clone();
                            state.palette.is_open = false;
                            return execute_action(state, act);
                        }
                    }
                    KeyCode::Backspace => {
                        state.palette.query.pop();
                        state.palette.selected_index = 0;
                    }
                    KeyCode::Char(c) => {
                        state.palette.query.push(c);
                        state.palette.selected_index = 0;
                    }
                    _ => {}
                }
                return false;
            }

            if state.help_open {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        state.help_open = false;
                        state.help_scroll = 0;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        state.help_scroll = state.help_scroll.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        state.help_scroll += 1;
                    }
                    KeyCode::PageUp => {
                        state.help_scroll = state.help_scroll.saturating_sub(10);
                    }
                    KeyCode::PageDown => {
                        state.help_scroll += 10;
                    }
                    _ => {}
                }
                return false;
            }

            if state.tour.is_active {
                match key.code {
                    KeyCode::Esc => {
                        state.tour.is_active = false;
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        state.tour.step += 1;
                        if state.tour.step >= tag_ui::TOUR_STEPS.len() {
                            state.tour.is_active = false;
                        }
                    }
                    _ => {}
                }
                return false;
            }

            if state.attention_state.is_open {
                match key.code {
                    KeyCode::Esc => state.attention_state.is_open = false,
                    KeyCode::Up => {
                        state.attention_state.selected_index =
                            state.attention_state.selected_index.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        state.attention_state.selected_index += 1;
                    }
                    KeyCode::Enter => {
                        if let Some(item) = state
                            .attention_queue
                            .items()
                            .get(state.attention_state.selected_index)
                        {
                            if let Some(pid) = item.pane_id {
                                state.layout_mgr.active_workspace_mut().focused_pane = Some(pid);
                                state.attention_queue.clear_by_pane(pid);
                            }
                        }
                        state.attention_state.is_open = false;
                    }
                    _ => {}
                }
                return false;
            }

            if state.usage_state.is_open {
                if key.code == KeyCode::Esc || key.code == KeyCode::Char('q') {
                    state.usage_state.is_open = false;
                }
                return false;
            }

            if state.git_state.is_open {
                if key.code == KeyCode::Esc || key.code == KeyCode::Char('q') {
                    state.git_state.is_open = false;
                }
                return false;
            }

            if state.snippet_state.is_open {
                match key.code {
                    KeyCode::Esc => state.snippet_state.is_open = false,
                    KeyCode::Up => {
                        state.snippet_state.selected_index =
                            state.snippet_state.selected_index.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        state.snippet_state.selected_index += 1;
                    }
                    KeyCode::Enter => {
                        if let Some(snip) = state
                            .snippets
                            .snippets
                            .get(state.snippet_state.selected_index)
                        {
                            let text = snip.template.clone();
                            state.snippet_state.is_open = false;
                            // Inject into focused pane via bracketed paste
                            if let Some(focused) = state.layout_mgr.active_workspace().focused_pane
                            {
                                if let Some(pane) = state.panes.get_mut(&focused) {
                                    let wrapped = wrap_bracketed_paste(&text);
                                    pane.send_input(&wrapped);
                                }
                            }
                        }
                    }
                    _ => {}
                }
                return false;
            }

            if state.vault_state.is_open {
                match key.code {
                    KeyCode::Esc => state.vault_state.is_open = false,
                    KeyCode::Backspace => {
                        state.vault_state.input_buffer.pop();
                    }
                    KeyCode::Char(c) => {
                        state.vault_state.input_buffer.push(c);
                    }
                    KeyCode::Enter => {
                        let pass = state.vault_state.input_buffer.clone();
                        state.vault_state.input_buffer.clear();
                        match state.vault.unlock(&pass) {
                            Ok(_) => {
                                state.vault_state.status_message =
                                    Some("Unlocked successfully".into());
                            }
                            Err(e) => {
                                state.vault_state.status_message = Some(format!("Error: {}", e));
                            }
                        }
                    }
                    _ => {}
                }
                return false;
            }

            if state.tab_tray.is_open {
                let tray_items = build_tray_items(state);
                let filtered = get_filtered_tray_items(&tray_items, &state.tab_tray.search_query);
                let count = filtered.len();

                if state.tab_tray.is_searching {
                    match key.code {
                        KeyCode::Esc => {
                            state.tab_tray.is_searching = false;
                            state.tab_tray.search_query.clear();
                            state.tab_tray.selected_index = 0;
                        }
                        KeyCode::Enter => {
                            state.tab_tray.is_searching = false;
                        }
                        KeyCode::Up => {
                            state.tab_tray.selected_index =
                                state.tab_tray.selected_index.saturating_sub(1);
                        }
                        KeyCode::Down => {
                            if count > 0 {
                                state.tab_tray.selected_index =
                                    (state.tab_tray.selected_index + 1).min(count - 1);
                            }
                        }
                        KeyCode::Backspace => {
                            state.tab_tray.search_query.pop();
                            state.tab_tray.selected_index = 0;
                        }
                        KeyCode::Char(c) => {
                            state.tab_tray.search_query.push(c);
                            state.tab_tray.selected_index = 0;
                        }
                        _ => {}
                    }
                    return false;
                }

                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        state.tab_tray.is_open = false;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        state.tab_tray.selected_index =
                            state.tab_tray.selected_index.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if count > 0 {
                            state.tab_tray.selected_index =
                                (state.tab_tray.selected_index + 1).min(count - 1);
                        }
                    }
                    KeyCode::Char('/') => {
                        state.tab_tray.is_searching = true;
                    }
                    KeyCode::Enter => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let target_ws = item.workspace_id;
                            let target_pid = item.pane_id;
                            state.tab_tray.is_open = false;
                            state.layout_mgr.switch_workspace(target_ws);
                            state.layout_mgr.active_workspace_mut().focused_pane = Some(target_pid);
                        }
                    }
                    KeyCode::Char('s') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let target_ws = item.workspace_id;
                            let target_pid = item.pane_id;
                            state.tab_tray.is_open = false;
                            state.layout_mgr.switch_workspace(target_ws);
                            state.layout_mgr.active_workspace_mut().focused_pane = Some(target_pid);
                            return execute_action(
                                state,
                                Action::Split(SplitDirection::Horizontal),
                            );
                        }
                    }
                    KeyCode::Char('v') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let target_ws = item.workspace_id;
                            let target_pid = item.pane_id;
                            state.tab_tray.is_open = false;
                            state.layout_mgr.switch_workspace(target_ws);
                            state.layout_mgr.active_workspace_mut().focused_pane = Some(target_pid);
                            return execute_action(state, Action::Split(SplitDirection::Vertical));
                        }
                    }
                    KeyCode::Char('x') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let pid = item.pane_id;
                            let ws_id = item.workspace_id;
                            if let Some(ws) = state
                                .layout_mgr
                                .workspaces_mut()
                                .iter_mut()
                                .find(|w| w.id == ws_id)
                            {
                                ws.remove_pane(pid);
                            }
                            state.panes.remove(&pid);
                            if state.tab_tray.selected_index > 0
                                && state.tab_tray.selected_index >= count.saturating_sub(1)
                            {
                                state.tab_tray.selected_index =
                                    state.tab_tray.selected_index.saturating_sub(1);
                            }
                        }
                    }
                    KeyCode::Char('z') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let target_ws = item.workspace_id;
                            let target_pid = item.pane_id;
                            state.layout_mgr.switch_workspace(target_ws);
                            state.layout_mgr.active_workspace_mut().focused_pane = Some(target_pid);
                            state.layout_mgr.active_workspace_mut().toggle_zoom();
                        }
                    }
                    _ => {}
                }
                return false;
            }

            // If active workspace is empty, check empty state actions and leader keys
            if state.layout_mgr.active_workspace().all_panes().is_empty() {
                if let Some(action) = state.key_state_machine.process_key(key) {
                    return execute_action(state, action);
                }

                match key.code {
                    KeyCode::Char('c') | KeyCode::Enter => {
                        return execute_action(state, Action::SpawnShell);
                    }
                    KeyCode::Char('w') => {
                        return execute_action(state, Action::NewWorkspace);
                    }
                    KeyCode::Char('p') => {
                        return execute_action(state, Action::ToggleCommandPalette);
                    }
                    KeyCode::Char('b') => {
                        return execute_action(state, Action::ToggleTabTray);
                    }
                    KeyCode::Char('?') => {
                        return execute_action(state, Action::ToggleHelp);
                    }
                    KeyCode::Char('q') => {
                        return execute_action(state, Action::Quit);
                    }
                    _ => {}
                }
                return false;
            }

            // Process through key state machine
            if let Some(action) = state.key_state_machine.process_key(key) {
                return execute_action(state, action);
            }

            // In passthrough or normal non-chord mode, forward key to active child pane
            if let Some(focused_id) = state.layout_mgr.active_workspace().focused_pane {
                if let Some(pane) = state.panes.get_mut(&focused_id) {
                    let bytes = key_event_to_bytes(key);
                    pane.send_input(&bytes);
                    pane.scroll_to_bottom();
                }
            }
            false
        }
        AppEvent::TerminalInput(CrossEvent::Mouse(mouse)) => {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    if let Some(focused_id) = state.layout_mgr.active_workspace().focused_pane {
                        if let Some(pane) = state.panes.get_mut(&focused_id) {
                            pane.scroll(3);
                        }
                    }
                }
                MouseEventKind::ScrollDown => {
                    if let Some(focused_id) = state.layout_mgr.active_workspace().focused_pane {
                        if let Some(pane) = state.panes.get_mut(&focused_id) {
                            pane.scroll(-3);
                        }
                    }
                }
                MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                    let col = mouse.column;
                    let row = mouse.row;

                    // 1. Check if clicked on status bar
                    if state.term_height > 0 && row >= state.term_height.saturating_sub(1) {
                        if let Some(target) = hit_test_status_bar(
                            state.layout_mgr.workspaces(),
                            state.layout_mgr.active_workspace_id(),
                            col,
                        ) {
                            match target {
                                StatusBarClickTarget::Workspace(idx) => {
                                    state.layout_mgr.switch_workspace_index(idx);
                                }
                                StatusBarClickTarget::AddWorkspace => {
                                    return execute_action(state, Action::NewWorkspace);
                                }
                            }
                        }
                        return false;
                    }

                    // 2. Check if clicked in Tab Tray
                    let tray_w = (state.term_width * 25 / 100)
                        .clamp(24, 45)
                        .min(state.term_width);
                    if state.tab_tray.is_open && col < tray_w {
                        let list_start_y = if state.tab_tray.is_searching
                            || !state.tab_tray.search_query.is_empty()
                        {
                            2
                        } else {
                            1
                        };
                        if row >= list_start_y {
                            let clicked_idx = (row - list_start_y) as usize;
                            let tray_items = build_tray_items(state);
                            let filtered =
                                get_filtered_tray_items(&tray_items, &state.tab_tray.search_query);
                            if let Some(item) = filtered.get(clicked_idx) {
                                let target_ws = item.workspace_id;
                                let target_pid = item.pane_id;
                                state.tab_tray.is_open = false;
                                state.layout_mgr.switch_workspace(target_ws);
                                state.layout_mgr.active_workspace_mut().focused_pane =
                                    Some(target_pid);
                            }
                        }
                        return false;
                    }

                    // 3. Content area
                    let content_x = if state.tab_tray.is_open { tray_w } else { 0 };
                    let content_w = state.term_width.saturating_sub(content_x);
                    let content_h = state.term_height.saturating_sub(1);
                    let content_area = Rect {
                        x: content_x,
                        y: 0,
                        width: content_w,
                        height: content_h,
                    };

                    if state.layout_mgr.active_workspace().all_panes().is_empty() {
                        return execute_action(state, Action::SpawnShell);
                    } else if let Some(hit) =
                        state
                            .layout_mgr
                            .active_workspace()
                            .hit_test(content_area, col, row)
                    {
                        state.layout_mgr.active_workspace_mut().focused_pane = Some(hit);
                    }
                }
                _ => {}
            }
            false
        }
        AppEvent::Tick => {
            // Expire toasts older than duration
            let now = chrono::Utc::now();
            state.toasts.retain(|t| {
                now.signed_duration_since(t.created_at).num_seconds() < t.duration_secs as i64
            });
            false
        }
        AppEvent::Resize { width, height } => {
            state.term_width = width;
            state.term_height = height;
            for pane in state.panes.values_mut() {
                pane.on_resize(width, height);
            }
            false
        }
        _ => false,
    }
}

fn execute_action(state: &mut AppState, action: Action) -> bool {
    match action {
        Action::Quit => {
            save_session_to_disk(state);
            return true;
        }
        Action::ToggleTabTray => {
            state.tab_tray.is_open = !state.tab_tray.is_open;
        }
        Action::ToggleCommandPalette => {
            state.palette.is_open = !state.palette.is_open;
            state.palette.query.clear();
            state.palette.selected_index = 0;
        }
        Action::ToggleHelp => {
            state.help_open = !state.help_open;
            state.help_scroll = 0;
        }
        Action::ToggleWhichKey => {}
        Action::ToggleAttentionQueue => {
            state.attention_state.is_open = !state.attention_state.is_open;
        }
        Action::ToggleUsageOverlay => {
            state.usage_state.is_open = !state.usage_state.is_open;
        }
        Action::ToggleGitOverlay => {
            state.git_state.is_open = !state.git_state.is_open;
        }
        Action::ToggleSnippetOverlay => {
            state.snippet_state.is_open = !state.snippet_state.is_open;
        }
        Action::ToggleVaultOverlay => {
            state.vault_state.is_open = !state.vault_state.is_open;
        }
        Action::ToggleQuickSwitcher => {
            state.palette.is_open = true;
            state.palette.query.clear();
        }
        Action::TogglePassthrough => {
            state.key_state_machine.toggle_passthrough();
        }
        Action::ToggleDnd => {
            state.dnd_active = !state.dnd_active;
        }
        Action::ToggleZoom => {
            state.layout_mgr.active_workspace_mut().toggle_zoom();
        }
        Action::UndoLayout => {
            state.layout_mgr.active_workspace_mut().undo_layout();
        }
        Action::Split(direction) => {
            let new_id = state.allocate_pane_id();
            let env = if state.vault.is_unlocked() {
                state.vault.get_env_set()
            } else {
                HashMap::new()
            };

            let (w, h) = if state.term_width > 0 && state.term_height > 0 {
                (
                    state.term_width.saturating_sub(2),
                    state.term_height.saturating_sub(4),
                )
            } else {
                (80, 24)
            };

            if let Ok(new_pane) = PtyPane::spawn(
                new_id,
                format!("Shell-{}", new_id.0),
                PaneKind::Shell,
                None,
                &[],
                &env,
                std::env::current_dir().ok(),
                w,
                h,
                state.config.scrollback_lines,
                state.event_tx.clone(),
            ) {
                state.panes.insert(new_id, Box::new(new_pane));
                let _ = state
                    .layout_mgr
                    .active_workspace_mut()
                    .split_current(new_id, direction);
            }
        }
        Action::ClosePane => {
            if let Some(focused) = state.layout_mgr.active_workspace().focused_pane {
                state.layout_mgr.active_workspace_mut().remove_pane(focused);
                state.panes.remove(&focused);
            }
        }
        Action::SwitchWorkspace(ws_id) => {
            state.layout_mgr.switch_workspace(ws_id);
        }
        Action::NextWorkspace => {
            state.layout_mgr.next_workspace();
        }
        Action::PrevWorkspace => {
            state.layout_mgr.prev_workspace();
        }
        Action::NewWorkspace => {
            let _new_id = state
                .layout_mgr
                .add_workspace(format!("WS-{}", state.layout_mgr.workspaces().len() + 1));
            let new_pid = state.allocate_pane_id();
            let (w, h) = if state.term_width > 0 && state.term_height > 0 {
                (
                    state.term_width.saturating_sub(2),
                    state.term_height.saturating_sub(4),
                )
            } else {
                (80, 24)
            };
            if let Ok(new_pane) = PtyPane::spawn(
                new_pid,
                "Shell".into(),
                PaneKind::Shell,
                None,
                &[],
                &HashMap::new(),
                std::env::current_dir().ok(),
                w,
                h,
                state.config.scrollback_lines,
                state.event_tx.clone(),
            ) {
                state.panes.insert(new_pid, Box::new(new_pane));
                state
                    .layout_mgr
                    .active_workspace_mut()
                    .add_initial_pane(new_pid);
            }
        }
        Action::Focus(dir) => {
            // Move focus between panes
            let panes = state.layout_mgr.active_workspace().all_panes();
            if let Some(curr) = state.layout_mgr.active_workspace().focused_pane {
                if let Some(pos) = panes.iter().position(|&p| p == curr) {
                    let next_pos = match dir {
                        Direction::Right | Direction::Down => (pos + 1) % panes.len(),
                        Direction::Left | Direction::Up => {
                            if pos == 0 {
                                panes.len() - 1
                            } else {
                                pos - 1
                            }
                        }
                    };
                    state.layout_mgr.active_workspace_mut().focused_pane = Some(panes[next_pos]);
                }
            }
        }
        Action::FocusPane(pid) => {
            state.layout_mgr.active_workspace_mut().focused_pane = Some(pid);
        }
        Action::SpawnShell => {
            execute_action(state, Action::Split(SplitDirection::Vertical));
        }
        Action::SpawnAgent { profile, worktree } => {
            let pid = state.allocate_pane_id();
            if let Some(prof_cfg) = state.config.agents.get(&profile) {
                let prof = AgentProfile::from_config(prof_cfg);
                if let Ok(agent_pane) = AgentPane::spawn(
                    pid,
                    &prof,
                    worktree,
                    80,
                    24,
                    state.config.scrollback_lines,
                    state.event_tx.clone(),
                ) {
                    state.panes.insert(pid, Box::new(agent_pane));
                    let _ = state
                        .layout_mgr
                        .active_workspace_mut()
                        .split_current(pid, SplitDirection::Vertical);
                }
            }
        }
        Action::AttachTmux { session } => {
            let pid = state.allocate_pane_id();
            if let Ok(tmux_pane) = AgentPane::spawn_tmux(
                pid,
                &session,
                80,
                24,
                state.config.scrollback_lines,
                state.event_tx.clone(),
            ) {
                state.panes.insert(pid, Box::new(tmux_pane));
                let _ = state
                    .layout_mgr
                    .active_workspace_mut()
                    .split_current(pid, SplitDirection::Vertical);
            }
        }
        Action::PasteImageBridge => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            match ImageBridge::save_clipboard_image(&cwd) {
                Ok(path) => {
                    if let Some(focused) = state.layout_mgr.active_workspace().focused_pane {
                        if let Some(pane) = state.panes.get_mut(&focused) {
                            let text = path.to_string_lossy().to_string();
                            let wrapped = wrap_bracketed_paste(&text);
                            pane.send_input(&wrapped);
                        }
                    }
                }
                Err(e) => {
                    state.toasts.push(Toast {
                        title: "Image Bridge".into(),
                        message: format!("Image paste failed: {}", e),
                        level: ToastLevel::Error,
                        created_at: chrono::Utc::now(),
                        duration_secs: 4,
                    });
                }
            }
        }
        Action::StartTour => {
            state.tour.is_active = true;
            state.tour.step = 0;
        }
        Action::ReloadConfig => {
            state.toasts.push(Toast {
                title: "Config".into(),
                message: "Configuration reloaded.".into(),
                level: ToastLevel::Info,
                created_at: chrono::Utc::now(),
                duration_secs: 3,
            });
        }
        _ => {}
    }
    false
}

impl AppState {
    fn allocate_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        id
    }
}

fn key_event_to_bytes(key: crossterm::event::KeyEvent) -> Vec<u8> {
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let code = (c.to_ascii_lowercase() as u8).saturating_sub(b'a' - 1);
                vec![code]
            } else {
                c.to_string().into_bytes()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![8],
        KeyCode::Tab => vec![9],
        KeyCode::Esc => vec![27],
        KeyCode::Up => vec![27, b'[', b'A'],
        KeyCode::Down => vec![27, b'[', b'B'],
        KeyCode::Right => vec![27, b'[', b'C'],
        KeyCode::Left => vec![27, b'[', b'D'],
        _ => Vec::new(),
    }
}

fn dirs_or_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn save_session_to_disk(state: &AppState) {
    let session_path = dirs_or_home()
        .join(".config")
        .join("tag")
        .join("session.json");
    let mut saved = SavedSession::new(state.layout_mgr.active_workspace_id());
    for ws in state.layout_mgr.workspaces() {
        saved.workspaces.push(SavedWorkspace::from(ws));
    }
    for (&pid, pane) in &state.panes {
        saved.panes.insert(
            pid,
            SavedPane {
                id: pid,
                title: pane.title(),
                kind: pane.kind(),
                cwd: pane.cwd(),
            },
        );
    }
    let _ = saved.save_to_file(&session_path);
}

fn build_tray_items(state: &AppState) -> Vec<TrayItem> {
    let mut tray_items = Vec::new();
    for ws in state.layout_mgr.workspaces() {
        for pid in ws.all_panes() {
            if let Some(p) = state.panes.get(&pid) {
                tray_items.push(TrayItem {
                    pane_id: pid,
                    workspace_id: ws.id,
                    workspace_name: ws.name.clone(),
                    title: p.title(),
                    icon: p.kind().icon(),
                    branch: p.branch(),
                    status: p.agent_status().unwrap_or("running"),
                    is_visible: ws.id == state.layout_mgr.active_workspace_id(),
                    needs_attention: false,
                });
            }
        }
    }
    tray_items
}

fn get_filtered_tray_items<'a>(items: &'a [TrayItem], query: &str) -> Vec<&'a TrayItem> {
    items
        .iter()
        .filter(|item| {
            if query.is_empty() {
                true
            } else {
                let q = query.to_lowercase();
                item.title.to_lowercase().contains(&q)
                    || item.workspace_name.to_lowercase().contains(&q)
                    || item
                        .branch
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&q)
            }
        })
        .collect()
}
