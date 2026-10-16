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
use tag_git::{
    GitBackend, GitOverlayState, GitPromptMode, GitPromptState, StandardGitBackend,
};
use tag_keys::KeyStateMachine;
use tag_layout::{LayoutManager, SavedPane, SavedSession, SavedWorkspace, Workspace};
use tag_notify::{
    render_attention_overlay, AttentionOverlayState, AttentionQueue, Notifier, SystemNotifier,
};
use tag_pty::PtyPane;
use tag_ui::{
    get_matched_commands, get_selected_command, hit_test_status_bar, render_command_palette,
    render_empty_state, render_help_overlay, render_rename_modal, render_status_bar,
    render_tab_tray, render_toasts, render_tour, render_which_key, PaletteState, RenameModalState,
    StatusBarClickTarget, StatusBarProps, TabTrayState, Toast, ToastLevel, TourState, TrayItem,
};
use tag_usage::{render_usage_overlay, AgyProvider, UsageOverlayState, UsageStore};
use tag_vault::{render_vault_overlay, VaultOverlayState, VaultStore};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

const TAG_SESSION_NAME: &str = "tag-session";

fn is_session_running(name: &str) -> bool {
    let status = std::process::Command::new("tmux")
        .args(["has-session", "-t", name])
        .output();
    match status {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}

fn attach_to_session(name: &str) -> Result<(), anyhow::Error> {
    let status = std::process::Command::new("tmux")
        .args(["attach-session", "-t", name])
        .status()?;
    if !status.success() {
        return Err(anyhow::anyhow!(
            "Failed to attach to tmux session '{}'",
            name
        ));
    }
    Ok(())
}

fn start_and_attach_background_session() -> Result<(), anyhow::Error> {
    let current_exe = std::env::current_exe()?;
    let status = std::process::Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            TAG_SESSION_NAME,
            &current_exe.to_string_lossy(),
            "--foreground",
        ])
        .status()?;
    if !status.success() {
        return Err(anyhow::anyhow!("Failed to start background TAG session."));
    }
    let res = attach_to_session(TAG_SESSION_NAME);
    println!(
        "\n[TAG detached. Run 'tag' or 'tag attach' to re-attach. Run 'tag kill' to terminate.]"
    );
    res
}

fn load_app_config() -> Config {
    let config_path = dirs_or_home()
        .join(".config")
        .join("tag")
        .join("config.toml");
    if let Ok(content) = std::fs::read_to_string(&config_path) {
        if let Ok(cfg) = Config::load_from_str(&content) {
            return cfg;
        }
    }
    Config::default()
}

fn save_app_config(config: &Config) {
    let config_path = dirs_or_home()
        .join(".config")
        .join("tag")
        .join("config.toml");
    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(toml_str) = config.to_toml_string() {
        let _ = std::fs::write(&config_path, toml_str);
    }
}

fn refresh_tmux_commands(registry: &mut CommandRegistry) {
    if let Ok(output) = std::process::Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{session_name}\t#{pane_current_command}",
        ])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut seen_sessions = std::collections::HashSet::new();
            for line in text.lines() {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.is_empty() {
                    continue;
                }
                let session = parts[0].trim();
                if session.is_empty()
                    || session == TAG_SESSION_NAME
                    || !seen_sessions.insert(session.to_string())
                {
                    continue;
                }
                let cmd = parts.get(1).map(|c| c.trim()).unwrap_or("");
                let is_agent = tag_pty::is_known_agent(cmd);
                let (name, desc) = if is_agent {
                    (
                        format!("Attach Tmux: {} ({})", session, cmd),
                        format!(
                            "Attach to tmux session '{}' running agent '{}'",
                            session, cmd
                        ),
                    )
                } else if !cmd.is_empty() && cmd != "bash" && cmd != "sh" && cmd != "zsh" {
                    (
                        format!("Attach Tmux: {} ({})", session, cmd),
                        format!("Attach to tmux session '{}' ({})", session, cmd),
                    )
                } else {
                    (
                        format!("Attach Tmux: {}", session),
                        format!("Attach to tmux session '{}'", session),
                    )
                };
                let cmd_id = format!("agents.attach_tmux_{}", session);
                let cmd_obj = tag_core::command::Command {
                    id: cmd_id,
                    name,
                    category: tag_core::command::CommandCategory::Agents,
                    description: desc,
                    default_shortcut: None,
                    action: Action::AttachTmux {
                        session: session.to_string(),
                    },
                };
                registry.register(cmd_obj);
            }
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "tag")]
#[command(author = "TAG Team")]
#[command(version = "0.1.0")]
#[command(about = "TAG: Terminal Agent Orchestrator & PTY Multiplexer")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Run directly in foreground (bypass background daemon)
    #[arg(short, long)]
    foreground: bool,

    /// Force running in background daemon
    #[arg(short = 'b', long = "bg")]
    background: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Configure agent notification hooks
    SetupHooks {
        /// Target agent (claude, codex, agy)
        agent: String,
    },
    /// Attach to running TAG background session
    Attach,
    /// Detach running TAG session
    Detach,
    /// Kill running TAG background session
    Kill,
    /// Show status of TAG background session
    Status,
}

struct CachedGitData {
    worktrees: Vec<tag_git::WorktreeInfo>,
    repo_status: tag_git::RepoStatus,
    diff: String,
    loaded_at: std::time::Instant,
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
    rename_prompt: Option<RenameModalState>,
    usage_store: UsageStore,
    snippets: SnippetBank,
    vault: VaultStore,
    git_backend: StandardGitBackend,
    notifier: SystemNotifier,
    event_tx: UnboundedSender<AppEvent>,
    is_dirty: bool,
    term_width: u16,
    term_height: u16,
    cached_usage_summary: Option<String>,
    last_usage_query: std::time::Instant,
    git_cache: Option<CachedGitData>,
}

impl AppState {
    fn update_usage_summary(&mut self) {
        let today_usage = self.usage_store.query_today().ok();
        self.cached_usage_summary = today_usage.map(|u| {
            let total_tokens = u.input_tokens + u.output_tokens;
            let tok_str = if total_tokens >= 1_000_000 {
                let m = total_tokens as f64 / 1_000_000.0;
                if m >= 10.0 {
                    format!("{:.1}M tok", m)
                } else {
                    format!("{:.2}M tok", m)
                }
            } else if total_tokens >= 1_000 {
                let k = total_tokens as f64 / 1_000.0;
                if k >= 10.0 {
                    format!("{:.0}k tok", k)
                } else {
                    format!("{:.1}k tok", k)
                }
            } else {
                format!("{} tok", total_tokens)
            };
            format!("{} (${:.2})", tok_str, u.estimated_cost_usd)
        });
        self.last_usage_query = std::time::Instant::now();
    }
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

    if let Some(Commands::Kill) = cli.command {
        if is_session_running(TAG_SESSION_NAME) {
            let _ = std::process::Command::new("tmux")
                .args(["kill-session", "-t", TAG_SESSION_NAME])
                .status();
            println!("TAG background session terminated.");
        } else {
            println!("No TAG background session is currently running.");
        }
        return Ok(());
    }

    if let Some(Commands::Status) = cli.command {
        if is_session_running(TAG_SESSION_NAME) {
            println!("TAG background session is active. Run 'tag' or 'tag attach' to connect.");
        } else {
            println!("No TAG background session is currently running.");
        }
        return Ok(());
    }

    if let Some(Commands::Detach) = cli.command {
        if is_session_running(TAG_SESSION_NAME) {
            let _ = std::process::Command::new("tmux")
                .args(["detach-client", "-s", TAG_SESSION_NAME])
                .status();
            println!("TAG background session detached.");
        } else {
            println!("No TAG background session is currently running.");
        }
        return Ok(());
    }

    // Guard against running TAG inside an existing TAG session
    if is_inside_tag() {
        eprintln!("Error: Cannot launch TAG from within an active TAG session.");
        eprintln!("Nested TAG instances are disabled to prevent terminal state conflicts.");
        std::process::exit(1);
    }

    let config = load_app_config();

    if let Some(Commands::Attach) = cli.command {
        if is_session_running(TAG_SESSION_NAME) {
            return attach_to_session(TAG_SESSION_NAME);
        } else {
            println!("No TAG background session found. Starting a new session...");
            return start_and_attach_background_session();
        }
    }

    // Default tag launch: check background mode
    if !cli.foreground && (cli.background || config.run_in_background) {
        if is_session_running(TAG_SESSION_NAME) {
            return attach_to_session(TAG_SESSION_NAME);
        } else {
            return start_and_attach_background_session();
        }
    }

    // Running in foreground: mark TAG_ACTIVE so child shells detect they are inside TAG
    std::env::set_var("TAG_ACTIVE", "1");

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

fn is_inside_tag() -> bool {
    // 1. Check environment variables injected into child PTYs
    if std::env::var("TAG_ACTIVE").is_ok()
        || std::env::var("COCKPIT_PANE_ID").is_ok()
        || std::env::var("TAG_PANE_ID").is_ok()
    {
        return true;
    }

    // 2. Check process ancestor tree on Linux
    #[cfg(target_os = "linux")]
    {
        let mut current_pid = std::process::id();
        for _ in 0..15 {
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", current_pid)) {
                if let Some(rparen) = stat.rfind(')') {
                    let rest = &stat[rparen + 1..];
                    let fields: Vec<&str> = rest.split_whitespace().collect();
                    if fields.len() >= 2 {
                        if let Ok(ppid) = fields[1].parse::<u32>() {
                            if ppid <= 1 {
                                break;
                            }
                            if let Ok(comm) =
                                std::fs::read_to_string(format!("/proc/{}/comm", ppid))
                            {
                                if comm.trim() == "tag" {
                                    return true;
                                }
                            }
                            current_pid = ppid;
                            continue;
                        }
                    }
                }
            }
            break;
        }
    }

    false
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

    // Spawn tick interval (100ms for animations and toast expiry)
    let tick_tx = event_tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        loop {
            interval.tick().await;
            if tick_tx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });

    // Ensure tmux has mouse mode globally enabled if tmux is installed (asynchronously)
    tokio::task::spawn_blocking(|| {
        let _ = std::process::Command::new("tmux")
            .args(["start-server"])
            .status();
        let _ = std::process::Command::new("tmux")
            .args(["set-option", "-g", "mouse", "on"])
            .status();
    });

    // Initialize state
    let config = load_app_config();
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
        rename_prompt: None,
        usage_store,
        snippets,
        vault,
        git_backend: StandardGitBackend,
        notifier: SystemNotifier,
        event_tx: event_tx.clone(),
        is_dirty: true,
        term_width: 80,
        term_height: 24,
        cached_usage_summary: None,
        last_usage_query: std::time::Instant::now(),
        git_cache: None,
    };
    state.update_usage_summary();

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
                        sp.command,
                        &sp.args,
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

        let mut should_quit = handle_app_event(&mut state, event);
        if should_quit {
            save_session_to_disk(&state);
            break;
        }

        // Batch-drain all pending events in queue before redrawing
        while let Ok(next_ev) = event_rx.try_recv() {
            should_quit = handle_app_event(&mut state, next_ev);
            if should_quit {
                break;
            }
        }
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
        let min_count = state.layout_mgr.active_workspace().minimized_panes.len();
        render_empty_state(frame, content_area, &state.theme, min_count);
    } else {
        let rects = state.layout_mgr.compute_rects(content_area);
        let focused_id = state.layout_mgr.active_workspace().focused_pane;
        let is_zoomed = state.layout_mgr.active_workspace().zoomed_pane.is_some();

        for (pane_id, area) in rects {
            if let Some(pane) = state.panes.get_mut(&pane_id) {
                let is_focused = focused_id == Some(pane_id);
                pane.set_zoomed(is_zoomed && is_focused);
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
    render_status_bar(
        frame,
        status_area,
        StatusBarProps {
            layout_mgr: &state.layout_mgr,
            key_state: state.key_state_machine.state(),
            dnd_active: state.dnd_active,
            attention_count: state.attention_queue.count(),
            token_usage_summary: state.cached_usage_summary.as_deref(),
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
            &mut state.palette,
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
        let active_repo_path = get_active_repo_path(state);
        let needs_reload = match &state.git_cache {
            Some(cache) => cache.loaded_at.elapsed() > std::time::Duration::from_secs(3),
            None => true,
        };
        if needs_reload {
            let worktrees = state
                .git_backend
                .list_worktrees(&active_repo_path)
                .unwrap_or_default();

            let target_path = if let Some(wt) = worktrees.get(state.git_state.selected_index) {
                wt.path.clone()
            } else {
                active_repo_path.clone()
            };

            let repo_status = state
                .git_backend
                .repo_status(&target_path)
                .unwrap_or_default();
            let diff = state
                .git_backend
                .get_diff(&target_path)
                .unwrap_or_default();
            state.git_cache = Some(CachedGitData {
                worktrees,
                repo_status,
                diff,
                loaded_at: std::time::Instant::now(),
            });
        }
        if let Some(ref cache) = state.git_cache {
            tag_git::render_git_overlay(
                frame,
                size,
                &cache.worktrees,
                &cache.repo_status,
                &cache.diff,
                &state.git_state,
                &state.theme,
            );
        }
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

    if let Some(ref rename_state) = state.rename_prompt {
        render_rename_modal(frame, size, rename_state, &state.theme);
    }

    render_toasts(frame, size, &state.toasts, &state.theme);
}

fn handle_app_event(state: &mut AppState, event: AppEvent) -> bool {
    match event {
        AppEvent::Action(action) => {
            state.is_dirty = true;
            execute_action(state, action)
        }
        AppEvent::PtyOutput { pane_id, data } => {
            state.is_dirty = true;
            if let Some(pane) = state.panes.get_mut(&pane_id) {
                let _ = pane.handle_event(&AppEvent::PtyOutput { pane_id, data });
            }
            false
        }
        AppEvent::PtyExited { pane_id, exit_code } => {
            state.is_dirty = true;
            if let Some(pane) = state.panes.get_mut(&pane_id) {
                let _ = pane.handle_event(&AppEvent::PtyExited { pane_id, exit_code });
            }
            if exit_code.is_none() || exit_code == Some(0) {
                for ws in state.layout_mgr.workspaces_mut() {
                    ws.remove_pane(pane_id);
                }
                state.panes.remove(&pane_id);
            }
            false
        }
        AppEvent::AgentHook(payload) => {
            state.is_dirty = true;
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
            state.update_usage_summary();
            false
        }
        AppEvent::TerminalInput(CrossEvent::Key(key)) => {
            state.is_dirty = true;
            // Check rename modal prompt first
            if let Some(ref mut rename) = state.rename_prompt {
                match key.code {
                    KeyCode::Enter => {
                        let new_title = rename.input.trim().to_string();
                        let pid = rename.pane_id;
                        if !new_title.is_empty() {
                            if let Some(pane) = state.panes.get_mut(&pid) {
                                pane.set_title(new_title.clone());
                                state.toasts.push(Toast {
                                    title: "Pane Renamed".into(),
                                    message: format!("Title updated to '{}'", new_title),
                                    level: ToastLevel::Success,
                                    created_at: chrono::Utc::now(),
                                    duration_secs: 2,
                                });
                            }
                        }
                        state.rename_prompt = None;
                    }
                    KeyCode::Esc => {
                        state.rename_prompt = None;
                    }
                    KeyCode::Backspace => {
                        rename.input.pop();
                    }
                    KeyCode::Char(c) => {
                        rename.input.push(c);
                    }
                    _ => {}
                }
                return false;
            }

            // Check modal overlays first
            if state.palette.is_open {
                let matched_count =
                    get_matched_commands(&state.command_registry, &state.palette.query).len();
                match key.code {
                    KeyCode::Esc => {
                        state.palette.is_open = false;
                    }
                    KeyCode::Up => {
                        state.palette.selected_index =
                            state.palette.selected_index.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        if matched_count > 0 && state.palette.selected_index + 1 < matched_count {
                            state.palette.selected_index += 1;
                        }
                    }
                    KeyCode::PageUp => {
                        state.palette.selected_index =
                            state.palette.selected_index.saturating_sub(8);
                    }
                    KeyCode::PageDown => {
                        if matched_count > 0 {
                            state.palette.selected_index =
                                (state.palette.selected_index + 8).min(matched_count - 1);
                        }
                    }
                    KeyCode::Home => {
                        state.palette.selected_index = 0;
                    }
                    KeyCode::End => {
                        if matched_count > 0 {
                            state.palette.selected_index = matched_count - 1;
                        }
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
                        state.palette.scroll_offset = 0;
                    }
                    KeyCode::Char(c) => {
                        state.palette.query.push(c);
                        state.palette.selected_index = 0;
                        state.palette.scroll_offset = 0;
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
                // If a modal prompt is open within the git overlay
                if let Some(ref mut prompt) = state.git_state.prompt {
                    match prompt.mode.clone() {
                        GitPromptMode::CreateWorktree => {
                            match key.code {
                                KeyCode::Esc => {
                                    state.git_state.prompt = None;
                                }
                                KeyCode::Backspace => {
                                    prompt.input.pop();
                                }
                                KeyCode::Char(c) => {
                                    prompt.input.push(c);
                                }
                                KeyCode::Enter => {
                                    let name = prompt.input.trim().to_string();
                                    state.git_state.prompt = None;
                                    if !name.is_empty() {
                                        let active_path = get_active_repo_path(state);
                                        match state.git_backend.create_worktree(&active_path, &name, &name) {
                                            Ok(wt_path) => {
                                                state.git_state.is_open = false;
                                                state.git_cache = None;
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
                                                    format!("wt: {}", name),
                                                    PaneKind::Shell,
                                                    None,
                                                    &[],
                                                    &env,
                                                    Some(wt_path),
                                                    w,
                                                    h,
                                                    state.config.scrollback_lines,
                                                    state.event_tx.clone(),
                                                ) {
                                                    state.panes.insert(new_id, Box::new(new_pane));
                                                    let _ = state
                                                        .layout_mgr
                                                        .active_workspace_mut()
                                                        .split_current(new_id, SplitDirection::Vertical);
                                                    state.toasts.push(Toast {
                                                        title: "Worktree Created".into(),
                                                        message: format!("Created worktree and opened shell in {}", name),
                                                        level: ToastLevel::Success,
                                                        created_at: chrono::Utc::now(),
                                                        duration_secs: 3,
                                                    });
                                                }
                                            }
                                            Err(e) => {
                                                state.toasts.push(Toast {
                                                    title: "Worktree Creation Failed".into(),
                                                    message: format!("{}", e),
                                                    level: ToastLevel::Error,
                                                    created_at: chrono::Utc::now(),
                                                    duration_secs: 5,
                                                });
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                            return false;
                        }
                        GitPromptMode::ConfirmMerge { branch, .. } => {
                            match key.code {
                                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                                    state.git_state.prompt = None;
                                }
                                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                                    state.git_state.prompt = None;
                                    let active_path = get_active_repo_path(state);
                                    let main_path = state.git_cache.as_ref()
                                        .and_then(|c| c.worktrees.iter().find(|w| w.is_main).map(|w| w.path.clone()))
                                        .unwrap_or(active_path);

                                    match state.git_backend.merge_branch(&main_path, &branch) {
                                        Ok(msg) => {
                                            state.git_cache = None;
                                            state.toasts.push(Toast {
                                                title: "Worktree Merged".into(),
                                                message: format!("Branch '{}' merged into main: {}", branch, msg),
                                                level: ToastLevel::Success,
                                                created_at: chrono::Utc::now(),
                                                duration_secs: 4,
                                            });
                                        }
                                        Err(e) => {
                                            state.git_cache = None;
                                            state.toasts.push(Toast {
                                                title: "Merge Conflict / Error".into(),
                                                message: format!("{}", e),
                                                level: ToastLevel::Error,
                                                created_at: chrono::Utc::now(),
                                                duration_secs: 6,
                                            });
                                        }
                                    }
                                }
                                _ => {}
                            }
                            return false;
                        }
                        GitPromptMode::ConfirmDelete { worktree_name } => {
                            match key.code {
                                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                                    state.git_state.prompt = None;
                                }
                                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                                    state.git_state.prompt = None;
                                    let active_path = get_active_repo_path(state);
                                    let main_path = state.git_cache.as_ref()
                                        .and_then(|c| c.worktrees.iter().find(|w| w.is_main).map(|w| w.path.clone()))
                                        .unwrap_or(active_path);

                                    match state.git_backend.delete_worktree(&main_path, &worktree_name, false) {
                                        Ok(_) => {
                                            state.git_cache = None;
                                            state.git_state.selected_index = 0;
                                            state.toasts.push(Toast {
                                                title: "Worktree Removed".into(),
                                                message: format!("Worktree '{}' removed", worktree_name),
                                                level: ToastLevel::Success,
                                                created_at: chrono::Utc::now(),
                                                duration_secs: 3,
                                            });
                                        }
                                        Err(e) => {
                                            state.toasts.push(Toast {
                                                title: "Removal Failed".into(),
                                                message: format!("{}", e),
                                                level: ToastLevel::Error,
                                                created_at: chrono::Utc::now(),
                                                duration_secs: 5,
                                            });
                                        }
                                    }
                                }
                                _ => {}
                            }
                            return false;
                        }
                    }
                }

                // Normal git overlay navigation
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        state.git_state.is_open = false;
                    }
                    KeyCode::Tab => {
                        state.git_state.selected_section =
                            (state.git_state.selected_section + 1) % 3;
                        state.git_state.selected_index = 0;
                    }
                    KeyCode::BackTab => {
                        state.git_state.selected_section =
                            if state.git_state.selected_section == 0 {
                                2
                            } else {
                                state.git_state.selected_section - 1
                            };
                        state.git_state.selected_index = 0;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if state.git_state.selected_section == 2 {
                            state.git_state.diff_scroll =
                                state.git_state.diff_scroll.saturating_sub(1);
                        } else {
                            let old_idx = state.git_state.selected_index;
                            state.git_state.selected_index = old_idx.saturating_sub(1);
                            if state.git_state.selected_section == 0 && state.git_state.selected_index != old_idx {
                                state.git_cache = None;
                            }
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if state.git_state.selected_section == 2 {
                            state.git_state.diff_scroll =
                                state.git_state.diff_scroll.saturating_add(1);
                        } else {
                            let max_idx = match state.git_state.selected_section {
                                0 => state
                                    .git_cache
                                    .as_ref()
                                    .map(|c| c.worktrees.len())
                                    .unwrap_or(0),
                                1 => state
                                    .git_cache
                                    .as_ref()
                                    .map(|c| c.repo_status.changed_files.len())
                                    .unwrap_or(0),
                                _ => 0,
                            };
                            if max_idx > 0 && state.git_state.selected_index + 1 < max_idx {
                                state.git_state.selected_index += 1;
                                if state.git_state.selected_section == 0 {
                                    state.git_cache = None;
                                }
                            }
                        }
                    }
                    KeyCode::Char('c') | KeyCode::Char('n') | KeyCode::Char('a') => {
                        if state.git_state.selected_section == 0 {
                            state.git_state.prompt = Some(GitPromptState {
                                mode: GitPromptMode::CreateWorktree,
                                input: String::new(),
                            });
                        }
                    }
                    KeyCode::Char('m') => {
                        if state.git_state.selected_section == 0 {
                            if let Some(ref cache) = state.git_cache {
                                if let Some(wt) = cache.worktrees.get(state.git_state.selected_index) {
                                    if !wt.is_main {
                                        if let Some(ref branch) = wt.branch {
                                            state.git_state.prompt = Some(GitPromptState {
                                                mode: GitPromptMode::ConfirmMerge {
                                                    worktree_name: wt.name.clone(),
                                                    branch: branch.clone(),
                                                },
                                                input: String::new(),
                                            });
                                        }
                                    } else {
                                        state.toasts.push(Toast {
                                            title: "Cannot Merge Main".into(),
                                            message: "Select a feature worktree to merge into main".into(),
                                            level: ToastLevel::Warning,
                                            created_at: chrono::Utc::now(),
                                            duration_secs: 3,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    KeyCode::Char('d') | KeyCode::Char('x') => {
                        if state.git_state.selected_section == 0 {
                            if let Some(ref cache) = state.git_cache {
                                if let Some(wt) = cache.worktrees.get(state.git_state.selected_index) {
                                    if !wt.is_main {
                                        state.git_state.prompt = Some(GitPromptState {
                                            mode: GitPromptMode::ConfirmDelete {
                                                worktree_name: wt.name.clone(),
                                            },
                                            input: String::new(),
                                        });
                                    } else {
                                        state.toasts.push(Toast {
                                            title: "Cannot Delete Main".into(),
                                            message: "Cannot remove the primary repository worktree".into(),
                                            level: ToastLevel::Warning,
                                            created_at: chrono::Utc::now(),
                                            duration_secs: 3,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    KeyCode::Enter => {
                        if state.git_state.selected_section == 0 {
                            if let Some(ref cache) = state.git_cache {
                                if let Some(wt) =
                                    cache.worktrees.get(state.git_state.selected_index)
                                {
                                    let wt_path = wt.path.clone();
                                    let wt_name = wt.name.clone();
                                    state.git_state.is_open = false;

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
                                        format!("wt: {}", wt_name),
                                        PaneKind::Shell,
                                        None,
                                        &[],
                                        &env,
                                        Some(wt_path),
                                        w,
                                        h,
                                        state.config.scrollback_lines,
                                        state.event_tx.clone(),
                                    ) {
                                        state.panes.insert(new_id, Box::new(new_pane));
                                        let _ = state
                                            .layout_mgr
                                            .active_workspace_mut()
                                            .split_current(new_id, SplitDirection::Vertical);
                                        state.toasts.push(Toast {
                                            title: "Worktree Opened".into(),
                                            message: format!("Opened shell in {}", wt_name),
                                            level: ToastLevel::Info,
                                            created_at: chrono::Utc::now(),
                                            duration_secs: 3,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
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
                            let is_min = item.is_minimized;
                            state.tab_tray.is_open = false;
                            state.layout_mgr.switch_workspace(target_ws);
                            if is_min {
                                if let Some(ws) = state.layout_mgr.find_workspace_mut(target_ws) {
                                    ws.restore_pane(target_pid, None);
                                }
                            }
                            state.layout_mgr.active_workspace_mut().focused_pane = Some(target_pid);
                        }
                    }
                    KeyCode::Char('s') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let target_ws = item.workspace_id;
                            let target_pid = item.pane_id;
                            let is_min = item.is_minimized;
                            state.tab_tray.is_open = false;
                            state.layout_mgr.switch_workspace(target_ws);
                            if is_min {
                                if let Some(ws) = state.layout_mgr.find_workspace_mut(target_ws) {
                                    ws.restore_pane(target_pid, Some(SplitDirection::Horizontal));
                                }
                                state.layout_mgr.active_workspace_mut().focused_pane =
                                    Some(target_pid);
                            } else {
                                state.layout_mgr.active_workspace_mut().focused_pane =
                                    Some(target_pid);
                                return execute_action(
                                    state,
                                    Action::Split(SplitDirection::Horizontal),
                                );
                            }
                        }
                    }
                    KeyCode::Char('v') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let target_ws = item.workspace_id;
                            let target_pid = item.pane_id;
                            let is_min = item.is_minimized;
                            state.tab_tray.is_open = false;
                            state.layout_mgr.switch_workspace(target_ws);
                            if is_min {
                                if let Some(ws) = state.layout_mgr.find_workspace_mut(target_ws) {
                                    ws.restore_pane(target_pid, Some(SplitDirection::Vertical));
                                }
                                state.layout_mgr.active_workspace_mut().focused_pane =
                                    Some(target_pid);
                            } else {
                                state.layout_mgr.active_workspace_mut().focused_pane =
                                    Some(target_pid);
                                return execute_action(
                                    state,
                                    Action::Split(SplitDirection::Vertical),
                                );
                            }
                        }
                    }
                    KeyCode::Char('x') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let pid = item.pane_id;
                            let ws_id = item.workspace_id;
                            if let Some(ws) = state.layout_mgr.find_workspace_mut(ws_id) {
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
                    KeyCode::Char('r') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let pid = item.pane_id;
                            state.tab_tray.is_open = false;
                            return execute_action(state, Action::RenamePane(Some(pid)));
                        }
                    }
                    KeyCode::Char('m') => {
                        if let Some(item) = filtered.get(state.tab_tray.selected_index) {
                            let target_ws = item.workspace_id;
                            let target_pid = item.pane_id;
                            let is_min = item.is_minimized;
                            if let Some(ws) = state.layout_mgr.find_workspace_mut(target_ws) {
                                if is_min {
                                    ws.restore_pane(target_pid, None);
                                } else {
                                    ws.minimize_pane(target_pid);
                                }
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
                        let min_count = state.layout_mgr.active_workspace().minimized_panes.len();
                        if min_count > 0 && key.code == KeyCode::Enter {
                            if let Some(first_min) = state
                                .layout_mgr
                                .active_workspace()
                                .minimized_panes
                                .first()
                                .copied()
                            {
                                state
                                    .layout_mgr
                                    .active_workspace_mut()
                                    .restore_pane(first_min, None);
                                state.layout_mgr.active_workspace_mut().focused_pane =
                                    Some(first_min);
                                return false;
                            }
                        }
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

            // Check Shift+PageUp / Shift+PageDown for viewport scrollback
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                match key.code {
                    KeyCode::PageUp => {
                        if let Some(focused_id) = state.layout_mgr.active_workspace().focused_pane {
                            if let Some(pane) = state.panes.get_mut(&focused_id) {
                                pane.scroll(15);
                            }
                        }
                        return false;
                    }
                    KeyCode::PageDown => {
                        if let Some(focused_id) = state.layout_mgr.active_workspace().focused_pane {
                            if let Some(pane) = state.panes.get_mut(&focused_id) {
                                pane.scroll(-15);
                            }
                        }
                        return false;
                    }
                    _ => {}
                }
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
            state.is_dirty = true;
            if state.palette.is_open {
                let matched_count =
                    get_matched_commands(&state.command_registry, &state.palette.query).len();
                match mouse.kind {
                    MouseEventKind::ScrollDown => {
                        if matched_count > 0 && state.palette.selected_index + 1 < matched_count {
                            state.palette.selected_index += 1;
                        }
                    }
                    MouseEventKind::ScrollUp => {
                        state.palette.selected_index =
                            state.palette.selected_index.saturating_sub(1);
                    }
                    MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                        if let Some(cmd) =
                            get_selected_command(&state.command_registry, &state.palette)
                        {
                            let act = cmd.action.clone();
                            state.palette.is_open = false;
                            return execute_action(state, act);
                        }
                    }
                    _ => {}
                }
                return false;
            }

            if state.help_open {
                match mouse.kind {
                    MouseEventKind::ScrollDown => {
                        state.help_scroll += 3;
                    }
                    MouseEventKind::ScrollUp => {
                        state.help_scroll = state.help_scroll.saturating_sub(3);
                    }
                    _ => {}
                }
                return false;
            }

            let tray_w = if state.tab_tray.is_open {
                (state.term_width * 25 / 100)
                    .clamp(24, 45)
                    .min(state.term_width)
            } else {
                0
            };

            if state.tab_tray.is_open && mouse.column < tray_w {
                let tray_items = build_tray_items(state);
                let filtered = get_filtered_tray_items(&tray_items, &state.tab_tray.search_query);
                let count = filtered.len();
                match mouse.kind {
                    MouseEventKind::ScrollDown => {
                        if count > 0 {
                            state.tab_tray.selected_index =
                                (state.tab_tray.selected_index + 1).min(count - 1);
                        }
                    }
                    MouseEventKind::ScrollUp => {
                        state.tab_tray.selected_index =
                            state.tab_tray.selected_index.saturating_sub(1);
                    }
                    _ => {}
                }
                return false;
            }

            let content_w = state.term_width.saturating_sub(tray_w);
            let content_h = state.term_height.saturating_sub(1);
            let content_area = Rect {
                x: tray_w,
                y: 0,
                width: content_w,
                height: content_h,
            };

            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    let target_id = state
                        .layout_mgr
                        .active_workspace()
                        .hit_test(content_area, mouse.column, mouse.row)
                        .or(state.layout_mgr.active_workspace().focused_pane);
                    if let Some(pid) = target_id {
                        let pane_rects = state
                            .layout_mgr
                            .active_workspace()
                            .compute_rects(content_area);
                        let (col, row) = if let Some(rect) = pane_rects.get(&pid) {
                            let inner_w = rect.width.saturating_sub(2).max(1);
                            let inner_h = rect.height.saturating_sub(2).max(1);
                            let inner_x = rect.x + 1;
                            let inner_y = rect.y + 1;
                            let c = mouse.column.saturating_sub(inner_x) + 1;
                            let r = mouse.row.saturating_sub(inner_y) + 1;
                            let max_r = if inner_h > 1 {
                                inner_h.saturating_sub(1)
                            } else {
                                1
                            };
                            (c.clamp(1, inner_w), r.clamp(1, max_r))
                        } else {
                            (1, 1)
                        };
                        if let Some(pane) = state.panes.get_mut(&pid) {
                            pane.scroll_at(3, col, row);
                        }
                    }
                }
                MouseEventKind::ScrollDown => {
                    let target_id = state
                        .layout_mgr
                        .active_workspace()
                        .hit_test(content_area, mouse.column, mouse.row)
                        .or(state.layout_mgr.active_workspace().focused_pane);
                    if let Some(pid) = target_id {
                        let pane_rects = state
                            .layout_mgr
                            .active_workspace()
                            .compute_rects(content_area);
                        let (col, row) = if let Some(rect) = pane_rects.get(&pid) {
                            let inner_w = rect.width.saturating_sub(2).max(1);
                            let inner_h = rect.height.saturating_sub(2).max(1);
                            let inner_x = rect.x + 1;
                            let inner_y = rect.y + 1;
                            let c = mouse.column.saturating_sub(inner_x) + 1;
                            let r = mouse.row.saturating_sub(inner_y) + 1;
                            let max_r = if inner_h > 1 {
                                inner_h.saturating_sub(1)
                            } else {
                                1
                            };
                            (c.clamp(1, inner_w), r.clamp(1, max_r))
                        } else {
                            (1, 1)
                        };
                        if let Some(pane) = state.panes.get_mut(&pid) {
                            pane.scroll_at(-3, col, row);
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
                                StatusBarClickTarget::CloseWorkspace(idx) => {
                                    if let Some(ws) = state.layout_mgr.workspaces().get(idx) {
                                        let ws_id = ws.id;
                                        return execute_action(
                                            state,
                                            Action::CloseWorkspace(ws_id),
                                        );
                                    }
                                }
                                StatusBarClickTarget::AddWorkspace => {
                                    return execute_action(state, Action::NewWorkspace);
                                }
                            }
                        }
                        return false;
                    }

                    // 2. Check if clicked in Tab Tray
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
                                let is_min = item.is_minimized;
                                state.tab_tray.is_open = false;
                                state.layout_mgr.switch_workspace(target_ws);
                                if is_min {
                                    if let Some(ws) = state.layout_mgr.find_workspace_mut(target_ws)
                                    {
                                        ws.restore_pane(target_pid, None);
                                    }
                                }
                                state.layout_mgr.active_workspace_mut().focused_pane =
                                    Some(target_pid);
                            }
                        }
                        return false;
                    }

                    // 3. Content area
                    if state.layout_mgr.active_workspace().all_panes().is_empty() {
                        let min_count = state.layout_mgr.active_workspace().minimized_panes.len();
                        if min_count > 0 {
                            if let Some(first_min) = state
                                .layout_mgr
                                .active_workspace()
                                .minimized_panes
                                .first()
                                .copied()
                            {
                                state
                                    .layout_mgr
                                    .active_workspace_mut()
                                    .restore_pane(first_min, None);
                                state.layout_mgr.active_workspace_mut().focused_pane =
                                    Some(first_min);
                                return false;
                            }
                        }
                        return execute_action(state, Action::SpawnShell);
                    } else {
                        let pane_rects = state
                            .layout_mgr
                            .active_workspace()
                            .compute_rects(content_area);
                        for (&pid, &rect) in &pane_rects {
                            if col >= rect.x
                                && col < rect.x + rect.width
                                && row >= rect.y
                                && row < rect.y + rect.height
                            {
                                // Clicked on top border
                                if row == rect.y {
                                    // 1. Clicked on top-right [x] close button
                                    if col >= rect.x + rect.width.saturating_sub(4) {
                                        state.layout_mgr.active_workspace_mut().remove_pane(pid);
                                        state.panes.remove(&pid);
                                        return false;
                                    }
                                    // 2. Clicked on maximize/restore button [^] or [-]
                                    if col >= rect.x + rect.width.saturating_sub(8) {
                                        state.layout_mgr.active_workspace_mut().focused_pane =
                                            Some(pid);
                                        state.layout_mgr.active_workspace_mut().toggle_zoom();
                                        return false;
                                    }
                                    // 3. Clicked on minimize button [_]
                                    if col >= rect.x + rect.width.saturating_sub(12) {
                                        state.layout_mgr.active_workspace_mut().minimize_pane(pid);
                                        return false;
                                    }
                                }
                                state.layout_mgr.active_workspace_mut().focused_pane = Some(pid);
                                break;
                            }
                        }
                    }
                }
                _ => {}
            }
            false
        }
        AppEvent::Tick => {
            let now = chrono::Utc::now();
            let before = state.toasts.len();
            state.toasts.retain(|t| {
                now.signed_duration_since(t.created_at).num_seconds() < t.duration_secs as i64
            });
            if before != state.toasts.len() || state.tour.is_active {
                state.is_dirty = true;
            }
            if state.last_usage_query.elapsed() >= std::time::Duration::from_secs(3) {
                let old_summary = state.cached_usage_summary.clone();
                state.update_usage_summary();
                if old_summary != state.cached_usage_summary {
                    state.is_dirty = true;
                }
            }
            false
        }
        AppEvent::Resize { width, height } => {
            state.is_dirty = true;
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
            if state.palette.is_open {
                refresh_tmux_commands(&mut state.command_registry);
            }
            state.palette.query.clear();
            state.palette.selected_index = 0;
            state.palette.scroll_offset = 0;
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
            if state.git_state.is_open {
                state.git_cache = None;
            }
        }
        Action::ToggleSnippetOverlay => {
            state.snippet_state.is_open = !state.snippet_state.is_open;
        }
        Action::ToggleVaultOverlay => {
            state.vault_state.is_open = !state.vault_state.is_open;
        }
        Action::ToggleQuickSwitcher => {
            state.palette.is_open = true;
            refresh_tmux_commands(&mut state.command_registry);
            state.palette.query.clear();
            state.palette.selected_index = 0;
            state.palette.scroll_offset = 0;
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
        Action::RenamePane(target_pid) => {
            let pid = target_pid.or(state.layout_mgr.active_workspace().focused_pane);
            if let Some(pid) = pid {
                let current_title = state.panes.get(&pid).map(|p| p.title()).unwrap_or_default();
                state.rename_prompt = Some(RenameModalState::new(pid, current_title));
            }
        }
        Action::Split(direction) => {
            if state.layout_mgr.active_workspace().zoomed_pane.is_some() {
                state.toasts.push(Toast {
                    title: "Split Disabled".into(),
                    message: "Pane is maximized. Restore (Alt+Enter, Leader+m, or click [-]) before splitting.".into(),
                    level: ToastLevel::Warning,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
                return false;
            }
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
        Action::CloseWorkspace(ws_id) => {
            if state.layout_mgr.workspaces().len() <= 1 {
                state.toasts.push(Toast {
                    title: "Workspace".into(),
                    message: "Cannot close the only remaining workspace.".into(),
                    level: ToastLevel::Warning,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
                return false;
            }
            let (closed, pids) = state.layout_mgr.close_workspace(ws_id);
            if closed {
                for pid in &pids {
                    state.panes.remove(pid);
                }
                state.toasts.push(Toast {
                    title: "Workspace Closed".into(),
                    message: format!("Closed workspace and terminated {} pane(s).", pids.len()),
                    level: ToastLevel::Info,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
            }
        }
        Action::CloseActiveWorkspace => {
            let active_id = state.layout_mgr.active_workspace_id();
            return execute_action(state, Action::CloseWorkspace(active_id));
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
            if state.layout_mgr.active_workspace().zoomed_pane.is_some() {
                state.toasts.push(Toast {
                    title: "Spawn Disabled".into(),
                    message: "Pane is maximized. Restore (Alt+Enter, Leader+m, or click [-]) before spawning a new pane.".into(),
                    level: ToastLevel::Warning,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
                return false;
            }
            let pid = state.allocate_pane_id();
            if let Some(prof_cfg) = state.config.agents.get(&profile).cloned() {
                let prof = AgentProfile::from_config(&prof_cfg);
                let (w, h) = if state.term_width > 0 && state.term_height > 0 {
                    (
                        state.term_width.saturating_sub(2),
                        state.term_height.saturating_sub(4),
                    )
                } else {
                    (80, 24)
                };
                match AgentPane::spawn(
                    pid,
                    &prof,
                    worktree,
                    w,
                    h,
                    state.config.scrollback_lines,
                    state.event_tx.clone(),
                ) {
                    Ok(agent_pane) => {
                        state.panes.insert(pid, Box::new(agent_pane));
                        let _ = state
                            .layout_mgr
                            .active_workspace_mut()
                            .split_current(pid, SplitDirection::Vertical);
                    }
                    Err(e) => {
                        state.toasts.push(Toast {
                            title: "Spawn Error".into(),
                            message: format!("Failed to spawn agent '{}': {}", prof.command, e),
                            level: ToastLevel::Error,
                            created_at: chrono::Utc::now(),
                            duration_secs: 4,
                        });
                    }
                }
            } else {
                state.toasts.push(Toast {
                    title: "Unknown Agent".into(),
                    message: format!("Agent profile '{}' not found in config.", profile),
                    level: ToastLevel::Error,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
            }
        }
        Action::AttachTmux { session } => {
            if state.layout_mgr.active_workspace().zoomed_pane.is_some() {
                state.toasts.push(Toast {
                    title: "Attach Disabled".into(),
                    message: "Pane is maximized. Restore (Alt+Enter, Leader+m, or click [-]) before attaching tmux.".into(),
                    level: ToastLevel::Warning,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
                return false;
            }
            let pid = state.allocate_pane_id();
            let (w, h) = if state.term_width > 0 && state.term_height > 0 {
                (
                    state.term_width.saturating_sub(2),
                    state.term_height.saturating_sub(4),
                )
            } else {
                (80, 24)
            };
            match AgentPane::spawn_tmux(
                pid,
                &session,
                w,
                h,
                state.config.scrollback_lines,
                state.event_tx.clone(),
            ) {
                Ok(tmux_pane) => {
                    state.panes.insert(pid, Box::new(tmux_pane));
                    let _ = state
                        .layout_mgr
                        .active_workspace_mut()
                        .split_current(pid, SplitDirection::Vertical);
                }
                Err(e) => {
                    state.toasts.push(Toast {
                        title: "Tmux Error".into(),
                        message: format!("Failed to attach tmux session '{}': {}", session, e),
                        level: ToastLevel::Error,
                        created_at: chrono::Utc::now(),
                        duration_secs: 4,
                    });
                }
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
        Action::MinimizePane(target_pid) => {
            let pid = target_pid.or(state.layout_mgr.active_workspace().focused_pane);
            if let Some(pid) = pid {
                state.layout_mgr.active_workspace_mut().minimize_pane(pid);
                state.toasts.push(Toast {
                    title: "Pane Minimized".into(),
                    message: "Pane minimized. Press Ctrl+Space b or open Tab Tray to restore."
                        .into(),
                    level: ToastLevel::Info,
                    created_at: chrono::Utc::now(),
                    duration_secs: 2,
                });
            }
        }
        Action::RestorePane(target_pid) => {
            state
                .layout_mgr
                .active_workspace_mut()
                .restore_pane(target_pid, None);
            state.layout_mgr.active_workspace_mut().focused_pane = Some(target_pid);
        }
        Action::Detach => {
            save_session_to_disk(state);
            let _ = std::process::Command::new("tmux")
                .args(["detach-client"])
                .status();
            return true;
        }
        Action::ToggleBackgroundMode => {
            state.config.run_in_background = !state.config.run_in_background;
            save_app_config(&state.config);
            let status = if state.config.run_in_background {
                "enabled (TAG will persist in background)"
            } else {
                "disabled (TAG will run in foreground)"
            };
            state.toasts.push(Toast {
                title: "Background Mode".into(),
                message: format!("Background mode is now {}", status),
                level: ToastLevel::Success,
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
        let (command, args, cwd) = pane.running_process();
        saved.panes.insert(
            pid,
            SavedPane {
                id: pid,
                title: pane.title(),
                kind: pane.kind(),
                cwd: cwd.or_else(|| pane.cwd()),
                command,
                args,
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
                    is_minimized: false,
                });
            }
        }
        for &pid in &ws.minimized_panes {
            if let Some(p) = state.panes.get(&pid) {
                tray_items.push(TrayItem {
                    pane_id: pid,
                    workspace_id: ws.id,
                    workspace_name: ws.name.clone(),
                    title: p.title(),
                    icon: p.kind().icon(),
                    branch: p.branch(),
                    status: p.agent_status().unwrap_or("running"),
                    is_visible: false,
                    needs_attention: false,
                    is_minimized: true,
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

fn get_active_repo_path(state: &AppState) -> PathBuf {
    let mut candidate_path = None;

    if let Some(focused_pid) = state.layout_mgr.active_workspace().focused_pane {
        if let Some(pane) = state.panes.get(&focused_pid) {
            let (_, _, cwd) = pane.running_process();
            if let Some(p) = cwd.or_else(|| pane.cwd()) {
                if p.exists() {
                    candidate_path = Some(p);
                }
            }
        }
    }

    if candidate_path.is_none() {
        for pane in state.panes.values() {
            let (_, _, cwd) = pane.running_process();
            if let Some(p) = cwd.or_else(|| pane.cwd()) {
                if p.exists() {
                    candidate_path = Some(p);
                    break;
                }
            }
        }
    }

    let search_path = candidate_path.unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    });

    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&search_path)
        .output()
    {
        if output.status.success() {
            let toplevel = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !toplevel.is_empty() {
                return PathBuf::from(toplevel);
            }
        }
    }

    search_path
}
