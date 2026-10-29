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
use std::io::{self, stdout, Write};
use std::path::PathBuf;
use std::time::Duration;
use tag_agents::{get_setup_hooks, AgentPane, AgentProfile, HookServer};
use tag_applets::{PeerChatPane, PokerPane};
use tag_clipboard::{
    copy_to_clipboard, get_clipboard_text, set_internal_clipboard, wrap_bracketed_paste,
    ImageBridge, SnippetBank, SnippetOverlayState,
};
use tag_core::action::{Action, SplitDirection};
use tag_core::command::CommandRegistry;
use tag_core::config::Config;
use tag_core::event::{AppEvent, HookEvent};
use tag_core::pane::{Pane, PaneId, PaneKind};
use tag_core::theme::Theme;
use tag_git::{GitBackend, GitOverlayState, GitPromptMode, GitPromptState, StandardGitBackend};
use tag_keys::KeyStateMachine;
use tag_layout::{LayoutManager, SavedPane, SavedSession, SavedWorkspace, Workspace};
use tag_notify::{
    render_attention_overlay, AttentionOverlayState, AttentionQueue, Notifier, SystemNotifier,
};
use tag_pty::PtyPane;
use tag_ui::{
    filter_tray_items, get_matched_commands, get_selected_command, hit_test_palette,
    hit_test_status_bar, render_command_palette, render_empty_state, render_help_overlay,
    render_rename_modal, render_status_bar, render_tab_tray, render_toasts, render_tour,
    render_which_key, tray_item_at_row, PaletteClick, PaletteState, RenameModalState,
    StatusBarClickTarget, StatusBarProps, TabTrayState, Toast, ToastLevel, TourState, TrayItem,
};
use tag_usage::{render_usage_overlay, UsageOverlayState, UsageStore};
use tag_vault::{render_vault_overlay, VaultMode, VaultOverlayState, VaultStore};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

mod update;

const TAG_SESSION_NAME: &str = "tag-session";

fn run_tmux_silent(args: &[&str]) -> bool {
    std::process::Command::new("tmux")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn is_session_running(name: &str) -> bool {
    run_tmux_silent(&["has-session", "-t", name])
}

fn is_tmux_server_running() -> bool {
    run_tmux_silent(&["has-session"])
}

fn inside_tmux() -> bool {
    std::env::var_os("TMUX").is_some_and(|v| !v.is_empty())
}

fn attach_to_session(name: &str) -> Result<(), anyhow::Error> {
    // Inside an existing tmux client, attaching would nest; switch that client instead
    let args = if inside_tmux() {
        ["switch-client", "-t", name]
    } else {
        ["attach-session", "-t", name]
    };
    let status = std::process::Command::new("tmux").args(args).status()?;
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
    if !inside_tmux() {
        println!(
            "\n[TAG detached. Run 'tag' or 'tag attach' to re-attach. Run 'tag kill' to terminate.]"
        );
    }
    res
}

fn config_path() -> PathBuf {
    dirs_or_home()
        .join(".config")
        .join("tag")
        .join("config.toml")
}

/// Load the config file. On a parse error the defaults are used and the error is
/// returned alongside so it can be shown (and the broken file is never overwritten).
fn load_app_config() -> (Config, Option<String>) {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => match Config::load_from_str(&content) {
            Ok(cfg) => (cfg, None),
            Err(e) => (
                Config::default(),
                Some(format!("{}: {}", path.display(), e)),
            ),
        },
        Err(_) => (Config::default(), None),
    }
}

fn save_app_config(config: &Config) -> Result<(), anyhow::Error> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, config.to_toml_string()?)?;
    Ok(())
}

fn refresh_tmux_commands(registry: &mut CommandRegistry) {
    // Rebuild the dynamic entries so closed sessions disappear and none are duplicated
    registry.remove_by_id_prefix("agents.attach_tmux_");
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
                let detected_agent = tag_pty::detect_agent_in_tmux(session);
                let (name, desc) = if let Some(ref agent) = detected_agent {
                    (
                        format!("Attach Tmux: {} ({})", session, agent),
                        format!(
                            "Attach to tmux session '{}' running agent '{}'",
                            session, agent
                        ),
                    )
                } else if tag_pty::detect_donut_in_tmux(session) {
                    (
                        format!("Attach Tmux: {} (donut)", session),
                        format!("Attach to tmux session '{}' running donut", session),
                    )
                } else {
                    let cmd = parts.get(1).map(|c| c.trim()).unwrap_or("");
                    let is_agent = tag_pty::is_known_agent(cmd);
                    if is_agent {
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
                    }
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
#[command(version)]
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
    /// Paste an image file or piped stdin into active TAG pane
    PasteImage {
        /// Path to image file (or stdin if omitted or '-')
        file: Option<PathBuf>,
    },
    /// Check for updates and update TAG to the latest release
    Update {
        /// Install beta / pre-release versions
        #[arg(long)]
        beta: bool,
    },
    /// Manage encrypted secrets in the TAG vault
    Vault {
        #[command(subcommand)]
        command: VaultCommands,
    },
}

#[derive(Subcommand, Debug)]
enum VaultCommands {
    /// Add or update a secret in the vault
    Set {
        /// Secret key name (e.g. ANTHROPIC_API_KEY)
        key: String,
        /// Secret value (if omitted, prompts securely or reads from stdin)
        value: Option<String>,
        /// Master passphrase (if omitted, prompts securely or uses TAG_VAULT_PASSWORD)
        #[arg(long)]
        passphrase: Option<String>,
        /// Custom vault file path (default: ~/.config/tag/vault.enc)
        #[arg(long)]
        vault_file: Option<PathBuf>,
    },
    /// List secret key names stored in the vault
    List {
        /// Master passphrase (if omitted, prompts securely or uses TAG_VAULT_PASSWORD)
        #[arg(long)]
        passphrase: Option<String>,
        /// Custom vault file path (default: ~/.config/tag/vault.enc)
        #[arg(long)]
        vault_file: Option<PathBuf>,
    },
    /// Get a secret value from the vault
    Get {
        /// Secret key name
        key: String,
        /// Master passphrase (if omitted, prompts securely or uses TAG_VAULT_PASSWORD)
        #[arg(long)]
        passphrase: Option<String>,
        /// Custom vault file path (default: ~/.config/tag/vault.enc)
        #[arg(long)]
        vault_file: Option<PathBuf>,
    },
    /// Remove a secret from the vault
    Remove {
        /// Secret key name
        key: String,
        /// Master passphrase (if omitted, prompts securely or uses TAG_VAULT_PASSWORD)
        #[arg(long)]
        passphrase: Option<String>,
        /// Custom vault file path (default: ~/.config/tag/vault.enc)
        #[arg(long)]
        vault_file: Option<PathBuf>,
    },
}

struct CachedGitData {
    worktrees: Vec<tag_git::WorktreeInfo>,
    repo_status: tag_git::RepoStatus,
    diff: String,
    loaded_at: std::time::Instant,
}

/// The version this binary was released as (stamped from the release tag).
const TAG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// How often transcripts are re-scanned for token usage in the background.
const USAGE_SCAN_INTERVAL: Duration = Duration::from_secs(30);
/// Window in which a second `q` / Ctrl+C on an empty workspace confirms quitting.
const QUIT_CONFIRM_WINDOW: Duration = Duration::from_secs(2);

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
    event_tx: UnboundedSender<AppEvent>,
    is_dirty: bool,
    needs_clear: bool,
    term_width: u16,
    term_height: u16,
    cached_usage_summary: Option<String>,
    last_usage_query: std::time::Instant,
    git_cache: Option<CachedGitData>,
    terminal_focused: bool,
    last_notification_time: Option<std::time::Instant>,
    selecting_pane: Option<(PaneId, Rect, (u16, u16))>,
    available_update: Option<String>,
    /// Error from parsing config.toml (defaults in use; file must not be overwritten)
    config_error: Option<String>,
    /// First `q`/Ctrl+C press on an empty workspace while other panes still run
    quit_confirm_at: Option<std::time::Instant>,
    /// Token usage events parsed by the background transcript scanner
    usage_rx: std::sync::mpsc::Receiver<Vec<tag_usage::UsageEvent>>,
    /// Git overlay data computed off the UI thread, tagged with a request generation
    git_tx: std::sync::mpsc::Sender<(u64, CachedGitData)>,
    git_rx: std::sync::mpsc::Receiver<(u64, CachedGitData)>,
    git_generation: u64,
    git_loading: bool,
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

fn main() -> Result<(), anyhow::Error> {
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
            run_tmux_silent(&["kill-session", "-t", TAG_SESSION_NAME]);
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
            run_tmux_silent(&["detach-client", "-s", TAG_SESSION_NAME]);
            println!("TAG background session detached.");
        } else {
            println!("No TAG background session is currently running.");
        }
        return Ok(());
    }

    if let Some(Commands::PasteImage { file }) = cli.command {
        return handle_paste_image(file);
    }

    if let Some(Commands::Update { beta }) = cli.command {
        if let Err(e) = update::run_install_script(beta) {
            eprintln!("Error updating TAG: {}", e);
            std::process::exit(1);
        }
        return Ok(());
    }

    if let Some(Commands::Vault { command }) = cli.command {
        return handle_vault_command(command);
    }

    // Guard against running TAG inside an existing TAG session
    if is_inside_tag() {
        eprintln!("Error: Cannot launch TAG from within an active TAG session.");
        eprintln!("Nested TAG instances are disabled to prevent terminal state conflicts.");
        std::process::exit(1);
    }

    let (config, config_error) = load_app_config();
    if let Some(ref err) = config_error {
        eprintln!(
            "Warning: ignoring invalid TAG config ({}). Using defaults.",
            err
        );
    }

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

    // Running in foreground: mark TAG_ACTIVE so child shells detect they are inside TAG.
    // Environment changes happen here, before the async runtime starts any threads.
    std::env::set_var("TAG_ACTIVE", "1");
    let socket_path = get_tag_socket_path();
    std::env::set_var("COCKPIT_SOCK", &socket_path);

    // Set up file logger (tracing to private file, NEVER stdout/stderr)
    let log_dir = dirs_or_home().join(".config").join("tag").join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&log_dir, std::fs::Permissions::from_mode(0o700));
    }
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

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let run_res = runtime.block_on(run_app(&mut terminal, socket_path, config_error));

    // Restore terminal cleanly
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange
    );
    let _ = terminal.show_cursor();
    let _ = io::stdout().flush();

    if let Err(e) = run_res {
        eprintln!("TAG error: {}", e);
        std::process::exit(1);
    }

    std::process::exit(0);
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

fn handle_paste_image(file: Option<PathBuf>) -> Result<(), anyhow::Error> {
    use std::io::{Read, Write};
    let bytes = match file {
        Some(ref p) if p.as_os_str() != "-" => {
            if !p.exists() {
                return Err(anyhow::anyhow!("File not found: {:?}", p));
            }
            std::fs::read(p)?
        }
        _ => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf)?;
            if buf.is_empty() {
                return Err(anyhow::anyhow!(
                    "No input image data provided via stdin. Provide a file path or pipe image data."
                ));
            }
            buf
        }
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let out_path = ImageBridge::save_image_bytes(&cwd, &bytes)?;

    let sock_path = if let Ok(s) = std::env::var("COCKPIT_SOCK") {
        if !s.is_empty() && std::path::Path::new(&s).exists() {
            Some(PathBuf::from(s))
        } else {
            find_latest_tag_socket()
        }
    } else {
        find_latest_tag_socket()
    };

    if let Some(sock) = sock_path {
        use std::os::unix::net::UnixStream;
        if let Ok(mut stream) = UnixStream::connect(&sock) {
            let payload = tag_core::event::HookPayload {
                event: tag_core::event::HookEvent::PasteImage,
                pane_id: None,
                agent: "image-bridge".into(),
                worktree: Some(cwd),
                message: out_path.to_string_lossy().to_string(),
                timestamp: chrono::Utc::now(),
            };
            if let Ok(json_bytes) = serde_json::to_vec(&payload) {
                let _ = stream.write_all(&json_bytes);
                let _ = stream.flush();
                println!("Pasted image into active TAG session: {:?}", out_path);
                return Ok(());
            }
        }
    }

    println!(
        "Saved image to {:?} (no running TAG socket found to auto-paste)",
        out_path
    );
    Ok(())
}

fn prompt_passphrase(
    cli_pass: Option<String>,
    vault_path: &std::path::Path,
) -> Result<String, anyhow::Error> {
    if let Some(p) = cli_pass {
        if !p.is_empty() {
            eprintln!(
                "Warning: --passphrase is visible in shell history and the process list; \
                 prefer the interactive prompt or TAG_VAULT_PASSWORD."
            );
            return Ok(p);
        }
    }
    if let Ok(p) = std::env::var("TAG_VAULT_PASSWORD") {
        if !p.is_empty() {
            return Ok(p);
        }
    }
    if vault_path.exists() {
        return read_password_terminal("Enter TAG Vault master passphrase: ");
    }
    // Creating a new vault: confirm so a typo can't lock the user out
    println!(
        "No vault found at {}; creating a new one.",
        vault_path.display()
    );
    let first = read_password_terminal("Choose a TAG Vault master passphrase: ")?;
    if first.is_empty() {
        return Err(anyhow::anyhow!("Passphrase cannot be empty"));
    }
    let second = read_password_terminal("Confirm master passphrase: ")?;
    if first != second {
        return Err(anyhow::anyhow!(
            "Passphrases do not match; vault not created"
        ));
    }
    Ok(first)
}

fn read_password_terminal(prompt: &str) -> Result<String, anyhow::Error> {
    use std::io::{stdout, Write};
    print!("{}", prompt);
    stdout().flush()?;

    crossterm::terminal::enable_raw_mode()?;
    let mut input = String::new();
    loop {
        if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
            if key.kind != crossterm::event::KeyEventKind::Press {
                continue;
            }
            match key.code {
                crossterm::event::KeyCode::Enter => break,
                crossterm::event::KeyCode::Backspace => {
                    input.pop();
                }
                crossterm::event::KeyCode::Char('c' | 'd')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    let _ = crossterm::terminal::disable_raw_mode();
                    println!();
                    return Err(anyhow::anyhow!("Operation cancelled"));
                }
                crossterm::event::KeyCode::Char(_)
                    if key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {}
                crossterm::event::KeyCode::Char(c) => {
                    input.push(c);
                }
                crossterm::event::KeyCode::Esc => {
                    let _ = crossterm::terminal::disable_raw_mode();
                    println!();
                    return Err(anyhow::anyhow!("Operation cancelled"));
                }
                _ => {}
            }
        }
    }
    let _ = crossterm::terminal::disable_raw_mode();
    println!();
    Ok(input)
}

fn handle_vault_command(cmd: VaultCommands) -> Result<(), anyhow::Error> {
    use std::io::IsTerminal;

    let default_vault_path = dirs_or_home().join(".config").join("tag").join("vault.enc");

    match cmd {
        VaultCommands::Set {
            key,
            value,
            passphrase,
            vault_file,
        } => {
            let path = vault_file.unwrap_or(default_vault_path);
            let mut vault = VaultStore::new(path.clone(), 60);
            let pass = prompt_passphrase(passphrase, &path)?;
            vault.unlock(&pass)?;

            let val = match value {
                Some(v) => v,
                None => {
                    if !std::io::stdin().is_terminal() {
                        use std::io::Read;
                        let mut buf = String::new();
                        std::io::stdin().read_to_string(&mut buf)?;
                        buf.trim_end_matches(&['\r', '\n'][..]).to_string()
                    } else {
                        read_password_terminal(&format!("Enter secret value for '{}': ", key))?
                    }
                }
            };

            vault.set_secret(key.clone(), val);
            vault.save_current()?;
            println!(
                "Secret '{}' saved and encrypted in TAG vault ({}).",
                key,
                path.display()
            );
        }
        VaultCommands::List {
            passphrase,
            vault_file,
        } => {
            let path = vault_file.unwrap_or(default_vault_path);
            if !path.exists() {
                return Err(anyhow::anyhow!(
                    "No TAG vault found at {}. Run 'tag vault set <KEY>' to create one.",
                    path.display()
                ));
            }
            let mut vault = VaultStore::new(path.clone(), 60);
            let pass = prompt_passphrase(passphrase, &path)?;
            vault.unlock(&pass)?;
            let keys = vault.list_keys();
            if keys.is_empty() {
                println!("TAG Secrets Vault is empty ({}).", path.display());
                println!(
                    "Run 'tag vault set <KEY>' or press Ctrl+Space e inside TAG to add secrets."
                );
            } else {
                println!(
                    "TAG Secrets Vault ({} secret{} stored in {}):",
                    keys.len(),
                    if keys.len() == 1 { "" } else { "s" },
                    path.display()
                );
                for k in keys {
                    println!("  • {:<28} [••••••••••••] (Auto-injected into agents)", k);
                }
            }
        }
        VaultCommands::Get {
            key,
            passphrase,
            vault_file,
        } => {
            let path = vault_file.unwrap_or(default_vault_path);
            if !path.exists() {
                return Err(anyhow::anyhow!(
                    "No TAG vault found at {}. Run 'tag vault set <KEY>' to create one.",
                    path.display()
                ));
            }
            let mut vault = VaultStore::new(path.clone(), 60);
            let pass = prompt_passphrase(passphrase, &path)?;
            vault.unlock(&pass)?;
            if let Some(sec) = vault.get_secret_value(&key) {
                println!("{}", sec);
            } else {
                eprintln!("Error: Secret '{}' not found in vault.", key);
                std::process::exit(1);
            }
        }
        VaultCommands::Remove {
            key,
            passphrase,
            vault_file,
        } => {
            let path = vault_file.unwrap_or(default_vault_path);
            if !path.exists() {
                return Err(anyhow::anyhow!(
                    "No TAG vault found at {}. Run 'tag vault set <KEY>' to create one.",
                    path.display()
                ));
            }
            let mut vault = VaultStore::new(path.clone(), 60);
            let pass = prompt_passphrase(passphrase, &path)?;
            vault.unlock(&pass)?;
            if vault.delete_secret(&key) {
                vault.save_current()?;
                println!(
                    "Secret '{}' removed from TAG vault ({}).",
                    key,
                    path.display()
                );
            } else {
                eprintln!("Error: Secret '{}' not found in vault.", key);
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// Unix socket paths are limited to ~104-108 bytes; keep a safety margin.
const MAX_SOCKET_PATH_LEN: usize = 100;

fn ensure_private_dir(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
}

fn preferred_socket_dir() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("tag")
    } else {
        dirs_or_home().join(".config").join("tag").join("run")
    }
}

/// Short per-user fallback under /tmp, used only when the preferred path is too long
/// for a Unix socket. Rejected unless it is a real directory owned by us and private.
fn short_socket_dir() -> Option<PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let dir = PathBuf::from(format!("/tmp/tag-{}", uid));
    let _ = std::fs::create_dir(&dir);
    let meta = std::fs::symlink_metadata(&dir).ok()?;
    if !meta.is_dir() || meta.uid() != uid {
        return None;
    }
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    Some(dir)
}

fn socket_file_name() -> String {
    format!("tag-sock-{}.sock", std::process::id())
}

fn get_tag_socket_path() -> PathBuf {
    let dir = preferred_socket_dir();
    let path = dir.join(socket_file_name());
    if path.as_os_str().len() <= MAX_SOCKET_PATH_LEN {
        ensure_private_dir(&dir);
        return path;
    }
    match short_socket_dir() {
        Some(short) => short.join(socket_file_name()),
        None => {
            ensure_private_dir(&dir);
            path
        }
    }
}

fn find_latest_tag_socket() -> Option<PathBuf> {
    let mut best_sock = None;
    let mut best_time = std::time::SystemTime::UNIX_EPOCH;

    // Only private, per-user locations (never scan the shared temp dir)
    let mut candidate_dirs = vec![
        preferred_socket_dir(),
        dirs_or_home().join(".config").join("tag").join("run"),
    ];
    // SAFETY: getuid has no preconditions and cannot fail.
    let short = PathBuf::from(format!("/tmp/tag-{}", unsafe { libc::getuid() }));
    if short.exists() {
        candidate_dirs.extend(short_socket_dir());
    }

    for dir in candidate_dirs {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("tag-sock-") && name_str.ends_with(".sock") {
                    if let Ok(metadata) = entry.metadata() {
                        if let Ok(mtime) = metadata.modified() {
                            if mtime > best_time {
                                best_time = mtime;
                                best_sock = Some(entry.path());
                            }
                        }
                    }
                }
            }
        }
    }
    best_sock
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
    socket_path: PathBuf,
    config_error: Option<String>,
) -> Result<(), anyhow::Error> {
    let (event_tx, mut event_rx) = unbounded_channel::<AppEvent>();

    // Start private hook socket listener (COCKPIT_SOCK was exported before the runtime started)
    let hook_server = HookServer::new(socket_path.clone());
    let hook_error = hook_server.start(event_tx.clone()).await.err();

    // Spawn crossterm event poller on dedicated thread with 50ms poll timeout & cancellation flag
    let is_running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let poller_running = is_running.clone();
    let input_tx = event_tx.clone();
    let input_thread = std::thread::Builder::new()
        .name("crossterm-input".into())
        .spawn(move || {
            while poller_running.load(std::sync::atomic::Ordering::Relaxed) {
                match crossterm::event::poll(Duration::from_millis(50)) {
                    Ok(true) => {
                        if let Ok(ev) = crossterm::event::read() {
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
                    }
                    Ok(false) => {}
                    Err(_) => break,
                }
            }
        })?;

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

    // Ensure tmux has mouse mode and responsive window sizing globally enabled if a tmux server is running (asynchronously)
    tokio::task::spawn_blocking(|| {
        if is_tmux_server_running() {
            run_tmux_silent(&["set-option", "-g", "mouse", "on"]);
            run_tmux_silent(&["set-option", "-g", "set-clipboard", "on"]);
            run_tmux_silent(&["set-option", "-g", "allow-passthrough", "on"]);
            run_tmux_silent(&["set-option", "-g", "window-size", "latest"]);
            run_tmux_silent(&["set-option", "-g", "aggressive-resize", "on"]);
        }
    });

    // Initialize state
    let (config, _) = load_app_config();
    let theme = Theme::from_config(config.theme.clone());
    let command_registry = CommandRegistry::new();
    let key_state_machine = KeyStateMachine::from_config(&config, &command_registry);
    let layout_mgr = LayoutManager::new();
    let usage_store = UsageStore::open_in_memory()?;
    let snippets_path = dirs_or_home()
        .join(".config")
        .join("tag")
        .join("snippets.toml");
    let snippets = SnippetBank::load_from_file_or_default(&snippets_path);
    let vault_path = dirs_or_home().join(".config").join("tag").join("vault.enc");
    let vault = VaultStore::new(vault_path, config.vault_idle_timeout_mins);

    // Background transcript scanner: re-parses only changed files and streams new events
    let (usage_tx, usage_rx) = std::sync::mpsc::channel();
    let scan_home = dirs_or_home();
    let _ = std::thread::Builder::new()
        .name("usage-scanner".into())
        .spawn(move || {
            let mut scanner = tag_usage::UsageScanner::new(scan_home);
            loop {
                let events = scanner.scan_changed();
                if !events.is_empty() && usage_tx.send(events).is_err() {
                    break;
                }
                std::thread::sleep(USAGE_SCAN_INTERVAL);
            }
        });
    let (git_tx, git_rx) = std::sync::mpsc::channel();

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
        event_tx: event_tx.clone(),
        is_dirty: true,
        needs_clear: false,
        term_width: 80,
        term_height: 24,
        cached_usage_summary: None,
        last_usage_query: std::time::Instant::now(),
        git_cache: None,
        terminal_focused: true,
        last_notification_time: None,
        selecting_pane: None,
        available_update: None,
        config_error,
        quit_confirm_at: None,
        usage_rx,
        git_tx,
        git_rx,
        git_generation: 0,
        git_loading: false,
    };
    state.update_usage_summary();
    if let Some(err) = state.config_error.clone() {
        push_toast(
            &mut state,
            "Config Error",
            format!("Using defaults; fix config.toml: {}", err),
            ToastLevel::Error,
            10,
        );
    }
    if let Some(e) = hook_error {
        push_toast(
            &mut state,
            "Agent Hooks Unavailable",
            format!("Could not listen on {}: {}", socket_path.display(), e),
            ToastLevel::Error,
            10,
        );
    }

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
                    if sp.kind == PaneKind::Applet {
                        let name = sp.args.first().map(String::as_str).unwrap_or("");
                        if let Some(mut applet) = make_applet(name, pid) {
                            applet.set_title(sp.title);
                            state.panes.insert(pid, applet);
                        }
                        continue;
                    }

                    let mut cmd = sp.command;
                    let mut args = sp.args;
                    let mut title = sp.title;
                    let mut kind = sp.kind;

                    // Sanitize tmux session on restore
                    if cmd.as_deref() == Some("tmux")
                        || kind == PaneKind::Tmux
                        || title.starts_with("tmux:")
                    {
                        let sess_opt =
                            if let Some(idx) = args.iter().position(|a| a == "-s" || a == "-t") {
                                args.get(idx + 1)
                                    .map(|s| tag_pty::clean_tmux_session_name(s))
                            } else {
                                title
                                    .strip_prefix("tmux: ")
                                    .map(tag_pty::clean_tmux_session_name)
                            };

                        let target_session = match sess_opt {
                            Some(s) if !s.is_empty() => s,
                            _ => {
                                // Fallback: find existing active tmux session if available
                                if let Ok(output) = std::process::Command::new("tmux")
                                    .args(["list-sessions", "-F", "#{session_name}"])
                                    .output()
                                {
                                    if output.status.success() {
                                        let text = String::from_utf8_lossy(&output.stdout);
                                        text.lines()
                                            .map(|l| tag_pty::clean_tmux_session_name(l.trim()))
                                            .find(|l| !l.is_empty())
                                            .unwrap_or_else(|| "0".to_string())
                                    } else {
                                        "0".to_string()
                                    }
                                } else {
                                    "0".to_string()
                                }
                            }
                        };

                        cmd = Some("tmux".to_string());
                        args = vec![
                            "new-session".to_string(),
                            "-A".to_string(),
                            "-s".to_string(),
                            target_session.clone(),
                        ];
                        title = format!("tmux: {}", target_session);
                        if kind != PaneKind::Agent && kind != PaneKind::Donut {
                            kind = PaneKind::Tmux;
                        }
                    }

                    if let Ok(pane) = PtyPane::spawn(
                        pid,
                        title,
                        kind,
                        cmd,
                        &args,
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
                    // Drop layout slots for panes that failed to respawn
                    let live: std::collections::HashSet<PaneId> =
                        state.panes.keys().copied().collect();
                    state.layout_mgr.retain_panes(&|p| live.contains(&p));
                    // Panes not referenced by any workspace would be unreachable
                    let placed: std::collections::HashSet<PaneId> = state
                        .layout_mgr
                        .workspaces()
                        .iter()
                        .flat_map(|w| w.all_panes().into_iter().chain(w.minimized_panes.clone()))
                        .collect();
                    state.panes.retain(|pid, _| placed.contains(pid));
                    state.next_pane_id = max_pid + 1;
                }
            }
        }
    }

    // Ensure clean screen and draw initial frame immediately before scanning historical logs
    let _ = terminal.clear();
    terminal.draw(|f| {
        render_frame(f, &mut state);
    })?;
    state.is_dirty = false;

    state.is_dirty = true;

    // Background update check after app startup
    let update_tx = event_tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        tokio::task::spawn_blocking(move || {
            if let Some(newer) = update::check_for_github_update(TAG_VERSION) {
                let _ = update_tx.send(AppEvent::UpdateAvailable(newer));
            }
        });
    });

    // Main event loop
    loop {
        if state.needs_clear {
            let _ = terminal.clear();
            state.needs_clear = false;
        }

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

        // Batch-drain all pending events in queue before redrawing
        while !should_quit {
            match event_rx.try_recv() {
                Ok(next_ev) => should_quit = handle_app_event(&mut state, next_ev),
                Err(_) => break,
            }
        }
        if should_quit {
            break;
        }
    }

    // Persist the session exactly once on the way out
    save_session_to_disk(&state);

    // Cleanly kill child processes in all panes
    for pane in state.panes.values_mut() {
        pane.kill();
    }

    // Stop input poller and wait for it to cleanly finish
    is_running.store(false, std::sync::atomic::Ordering::Relaxed);
    let _ = input_thread.join();

    // Clean up socket file
    let _ = std::fs::remove_file(socket_path);

    Ok(())
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
        render_empty_state(frame, content_area, &state.theme, min_count, TAG_VERSION);
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
        render_tab_tray(
            frame,
            t_area,
            &tray_items,
            &mut state.tab_tray,
            &state.theme,
        );
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
            version: TAG_VERSION,
            available_update: state.available_update.as_deref(),
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
            &mut state.help_scroll,
            TAG_VERSION,
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

fn is_any_overlay_open(state: &AppState) -> bool {
    state.palette.is_open
        || state.help_open
        || state.tour.is_active
        || state.attention_state.is_open
        || state.usage_state.is_open
        || state.git_state.is_open
        || state.snippet_state.is_open
        || state.vault_state.is_open
        || state.rename_prompt.is_some()
        || state.tab_tray.is_open
}

fn handle_app_event(state: &mut AppState, event: AppEvent) -> bool {
    let was_overlay_open = is_any_overlay_open(state);
    let should_quit = handle_app_event_inner(state, event);
    let is_overlay_open = is_any_overlay_open(state);
    if was_overlay_open != is_overlay_open {
        state.needs_clear = true;
        state.is_dirty = true;
    }
    should_quit
}

fn handle_app_event_inner(state: &mut AppState, event: AppEvent) -> bool {
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
            for ws in state.layout_mgr.workspaces_mut() {
                ws.remove_pane(pane_id);
            }
            state.panes.remove(&pane_id);
            state.attention_queue.clear_by_pane(pane_id);
            state.needs_clear = true;
            false
        }
        AppEvent::AgentHook(payload) => {
            state.is_dirty = true;
            if let HookEvent::PasteImage = payload.event {
                let target_pid = payload
                    .pane_id
                    .or(state.layout_mgr.active_workspace().focused_pane);
                if let Some(pid) = target_pid {
                    if let Some(pane) = state.panes.get_mut(&pid) {
                        let bytes = if pane.bracketed_paste() {
                            wrap_bracketed_paste(&payload.message)
                        } else {
                            payload.message.as_bytes().to_vec()
                        };
                        pane.send_input(&bytes);
                    }
                }
                let filename = std::path::Path::new(&payload.message)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&payload.message);
                state.toasts.push(Toast {
                    title: "Image Bridge".into(),
                    message: format!("Pasted {}", filename),
                    level: ToastLevel::Info,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
                return false;
            }

            info!("Agent hook event received: {:?}", payload.event);
            let _item_id = state.attention_queue.push(
                payload.pane_id,
                payload.agent.clone(),
                payload.event.clone(),
                payload.message.clone(),
                payload.worktree.clone(),
            );

            // Toast notification (when notifications enabled)
            if state.config.notifications.enabled {
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
            }

            // Out-of-app notification & bell if not DND, enabled, focus policy satisfied, and debounced
            let debounce_ok = state
                .last_notification_time
                .map(|t| {
                    t.elapsed()
                        >= std::time::Duration::from_millis(state.config.notifications.debounce_ms)
                })
                .unwrap_or(true);
            let focus_ok =
                !state.config.notifications.only_when_unfocused || !state.terminal_focused;

            if !state.dnd_active && state.config.notifications.enabled && focus_ok && debounce_ok {
                state.last_notification_time = Some(std::time::Instant::now());
                if state.config.notifications.desktop_notifications {
                    // D-Bus/notification daemons can block for a long time; never on the UI thread
                    let summary = format!("TAG - {}", payload.agent);
                    let body = payload.message.clone();
                    let _ = std::thread::Builder::new()
                        .name("desktop-notify".into())
                        .spawn(move || SystemNotifier.notify(&summary, &body));
                }
                if state.config.notifications.sound {
                    use std::io::Write;
                    let _ = std::io::stdout().write_all(b"\x07");
                    let _ = std::io::stdout().flush();
                }
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
                    _ => {
                        if let Some(c) = text_input_char(&key) {
                            rename.input.push(c);
                        }
                    }
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
                    _ => {
                        if let Some(c) = text_input_char(&key) {
                            state.palette.query.push(c);
                            state.palette.selected_index = 0;
                            state.palette.scroll_offset = 0;
                        }
                    }
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
                        let count = state.attention_queue.count();
                        if count > 0 {
                            state.attention_state.selected_index =
                                (state.attention_state.selected_index + 1).min(count - 1);
                        }
                    }
                    KeyCode::Enter => {
                        let item = state
                            .attention_queue
                            .items()
                            .get(state.attention_state.selected_index)
                            .cloned();
                        if let Some(item) = item {
                            match item.pane_id {
                                Some(pid) => {
                                    if !focus_pane_anywhere(state, pid) {
                                        push_toast(
                                            state,
                                            "Attention Queue",
                                            "That pane no longer exists.",
                                            ToastLevel::Warning,
                                            3,
                                        );
                                    }
                                    state.attention_queue.clear_by_pane(pid);
                                }
                                None => {
                                    state.attention_queue.clear_item(item.id);
                                }
                            }
                        }
                        state.attention_state.selected_index = 0;
                        state.attention_state.is_open = false;
                    }
                    KeyCode::Char('x') | KeyCode::Char('d') | KeyCode::Delete => {
                        // Dismiss the selected alert without jumping to it
                        let id = state
                            .attention_queue
                            .items()
                            .get(state.attention_state.selected_index)
                            .map(|i| i.id);
                        if let Some(id) = id {
                            state.attention_queue.clear_item(id);
                        }
                        let count = state.attention_queue.count();
                        state.attention_state.selected_index = state
                            .attention_state
                            .selected_index
                            .min(count.saturating_sub(1));
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
                handle_git_overlay_key(state, key);
                return false;
            }

            if state.snippet_state.is_open {
                if let Some(ref mut fill) = state.snippet_state.fill {
                    match key.code {
                        KeyCode::Esc => state.snippet_state.fill = None,
                        KeyCode::Backspace => {
                            fill.input.pop();
                        }
                        KeyCode::Enter => {
                            if let Some(text) = state.snippet_state.submit_fill_value() {
                                state.snippet_state.is_open = false;
                                paste_into_focused(state, &text);
                            }
                        }
                        _ => {
                            if let Some(c) = text_input_char(&key) {
                                fill.input.push(c);
                            }
                        }
                    }
                    return false;
                }
                match key.code {
                    KeyCode::Esc => state.snippet_state.is_open = false,
                    KeyCode::Up => {
                        state.snippet_state.selected_index =
                            state.snippet_state.selected_index.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        if !state.snippets.snippets.is_empty() {
                            state.snippet_state.selected_index =
                                (state.snippet_state.selected_index + 1)
                                    .min(state.snippets.snippets.len() - 1);
                        }
                    }
                    KeyCode::Enter => {
                        let template = state
                            .snippets
                            .snippets
                            .get(state.snippet_state.selected_index)
                            .map(|snip| snip.template.clone());
                        if let Some(template) = template {
                            // Templates with {{placeholders}} prompt for values first
                            if let Some(text) = state.snippet_state.begin_fill(&template) {
                                state.snippet_state.is_open = false;
                                paste_into_focused(state, &text);
                            }
                        }
                    }
                    _ => {}
                }
                return false;
            }

            if state.vault_state.is_open {
                // If not unlocked, handle passphrase input
                if !state.vault.is_unlocked() {
                    let creating = !state.vault.exists();
                    match key.code {
                        KeyCode::Esc => {
                            state.vault_state.clear_input();
                            if state.vault_state.pending_new_passphrase.is_some() {
                                // Start over choosing the new passphrase
                                state.vault_state.clear_pending_passphrase();
                                state.vault_state.status_message = None;
                            } else {
                                state.vault_state.is_open = false;
                            }
                        }
                        KeyCode::Backspace => {
                            state.vault_state.input_buffer.pop();
                        }
                        KeyCode::Enter if creating && state.vault_state.input_buffer.is_empty() => {
                            state.vault_state.status_message =
                                Some("Passphrase cannot be empty".into());
                        }
                        KeyCode::Enter
                            if creating && state.vault_state.pending_new_passphrase.is_none() =>
                        {
                            // New vault: ask for the passphrase twice before creating it
                            state.vault_state.pending_new_passphrase =
                                Some(std::mem::take(&mut state.vault_state.input_buffer));
                            state.vault_state.status_message = None;
                        }
                        KeyCode::Enter
                            if creating
                                && state.vault_state.pending_new_passphrase.as_deref()
                                    != Some(state.vault_state.input_buffer.as_str()) =>
                        {
                            state.vault_state.clear_input();
                            state.vault_state.clear_pending_passphrase();
                            state.vault_state.status_message = Some(
                                "Passphrases did not match. Choose a passphrase again.".into(),
                            );
                        }
                        KeyCode::Enter => {
                            let res = state.vault.unlock(&state.vault_state.input_buffer);
                            state.vault_state.clear_input();
                            state.vault_state.clear_pending_passphrase();
                            match res {
                                Ok(_) => {
                                    state.vault_state.mode = VaultMode::Viewing;
                                    state.vault_state.status_message =
                                        Some("Unlocked successfully".into());
                                }
                                Err(e) => {
                                    state.vault_state.status_message =
                                        Some(format!("Error: {}", e));
                                }
                            }
                        }
                        _ => {
                            if let Some(c) = text_input_char(&key) {
                                state.vault_state.input_buffer.push(c);
                            }
                        }
                    }
                    return false;
                }

                // If unlocked, handle based on vault mode
                match state.vault_state.mode {
                    VaultMode::Locked => {
                        state.vault_state.mode = VaultMode::Viewing;
                    }
                    VaultMode::Viewing => {
                        let keys = state.vault.list_keys();
                        match key.code {
                            KeyCode::Esc => {
                                state.vault_state.clear_input();
                                state.vault_state.is_open = false;
                            }
                            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                                state.vault_state.selected_index =
                                    state.vault_state.selected_index.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                                if !keys.is_empty() {
                                    state.vault_state.selected_index =
                                        (state.vault_state.selected_index + 1).min(keys.len() - 1);
                                }
                            }
                            KeyCode::Char('a') | KeyCode::Char('A') if key.modifiers.is_empty() => {
                                state.vault_state.mode = VaultMode::AddingKey;
                                state.vault_state.clear_input();
                                state.vault_state.pending_key.clear();
                                state.vault_state.status_message = None;
                            }
                            KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Delete
                                if key.modifiers.is_empty() || key.code == KeyCode::Delete =>
                            {
                                if let Some(key_to_delete) =
                                    keys.get(state.vault_state.selected_index)
                                {
                                    let key_name = key_to_delete.clone();
                                    state.vault.delete_secret(&key_name);
                                    let _ = state.vault.save_current();
                                    state.vault_state.status_message =
                                        Some(format!("Deleted secret '{}'", key_name));
                                    let remaining_keys = state.vault.list_keys();
                                    if state.vault_state.selected_index >= remaining_keys.len() {
                                        state.vault_state.selected_index =
                                            remaining_keys.len().saturating_sub(1);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    VaultMode::AddingKey => match key.code {
                        KeyCode::Esc => {
                            state.vault_state.clear_input();
                            state.vault_state.pending_key.clear();
                            state.vault_state.mode = VaultMode::Viewing;
                            state.vault_state.status_message = None;
                        }
                        KeyCode::Backspace => {
                            state.vault_state.input_buffer.pop();
                        }
                        KeyCode::Enter => {
                            let key_name = state.vault_state.input_buffer.trim().to_string();
                            if !key_name.is_empty() {
                                state.vault_state.pending_key = key_name;
                                state.vault_state.clear_input();
                                state.vault_state.mode = VaultMode::AddingValue;
                                state.vault_state.status_message = None;
                            } else {
                                state.vault_state.status_message =
                                    Some("Key name cannot be empty".into());
                            }
                        }
                        _ => {
                            if let Some(c) = text_input_char(&key) {
                                state.vault_state.input_buffer.push(c);
                            }
                        }
                    },
                    VaultMode::AddingValue => match key.code {
                        KeyCode::Esc => {
                            state.vault_state.clear_input();
                            state.vault_state.pending_key.clear();
                            state.vault_state.mode = VaultMode::Viewing;
                            state.vault_state.status_message = None;
                        }
                        KeyCode::Backspace => {
                            state.vault_state.input_buffer.pop();
                        }
                        KeyCode::Enter => {
                            let secret_val = state.vault_state.input_buffer.clone();
                            let key_name = state.vault_state.pending_key.clone();
                            state.vault.set_secret(key_name.clone(), secret_val);
                            let save_res = state.vault.save_current();
                            state.vault_state.clear_input();
                            state.vault_state.pending_key.clear();
                            state.vault_state.mode = VaultMode::Viewing;
                            match save_res {
                                Ok(_) => {
                                    state.vault_state.status_message =
                                        Some(format!("Secret '{}' saved and encrypted", key_name));
                                }
                                Err(e) => {
                                    state.vault_state.status_message =
                                        Some(format!("Failed to save secret: {}", e));
                                }
                            }
                        }
                        _ => {
                            if let Some(c) = text_input_char(&key) {
                                state.vault_state.input_buffer.push(c);
                            }
                        }
                    },
                }
                return false;
            }

            if state.tab_tray.is_open {
                let tray_items = build_tray_items(state);
                let filtered = filter_tray_items(&tray_items, &state.tab_tray.search_query);
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
                        _ => {
                            if let Some(c) = text_input_char(&key) {
                                state.tab_tray.search_query.push(c);
                                state.tab_tray.selected_index = 0;
                            }
                        }
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
                            let target_pid = item.pane_id;
                            state.tab_tray.is_open = false;
                            focus_pane_anywhere(state, target_pid);
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
                            close_pane(state, pid);
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

                let is_ctrl_c =
                    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
                let is_q = key.code == KeyCode::Char('q') && key.modifiers.is_empty();
                if is_ctrl_c || is_q {
                    return confirm_quit_from_empty_workspace(state);
                }
                if !key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT {
                    return false;
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
                    _ => {}
                }
                return false;
            }

            let was_leader = state.key_state_machine.state() == tag_keys::KeyState::LeaderPending;
            // Process through key state machine
            if let Some(action) = state.key_state_machine.process_key(key) {
                return execute_action(state, action);
            }
            if was_leader {
                // Key was entered while Leader was pending; do not forward unmapped follower to child pane!
                state.is_dirty = true;
                return false;
            }

            // Direct Ctrl+Shift+V or Shift+Insert for paste
            if (key.modifiers.contains(KeyModifiers::CONTROL)
                && key.modifiers.contains(KeyModifiers::SHIFT)
                && matches!(key.code, KeyCode::Char('v') | KeyCode::Char('V')))
                || (key.modifiers.contains(KeyModifiers::SHIFT) && key.code == KeyCode::Insert)
            {
                return execute_action(state, Action::PasteFromClipboard);
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

            // If focused pane has an active selection and Ctrl+C is pressed, copy the selection
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
            {
                let has_sel = state
                    .layout_mgr
                    .active_workspace()
                    .focused_pane
                    .and_then(|pid| state.panes.get(&pid))
                    .map(|p| p.has_selection())
                    .unwrap_or(false);

                if has_sel {
                    return execute_action(state, Action::CopySelection);
                }
            }

            // In passthrough or normal non-chord mode, forward key to active child pane
            if let Some(focused_id) = state.layout_mgr.active_workspace().focused_pane {
                if let Some(pane) = state.panes.get_mut(&focused_id) {
                    let bytes = key_event_to_bytes(key, pane.application_cursor());
                    if !bytes.is_empty() {
                        pane.send_input(&bytes);
                        pane.scroll_to_bottom();
                    }
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
                        let screen = Rect::new(0, 0, state.term_width, state.term_height);
                        match hit_test_palette(
                            screen,
                            &state.palette,
                            matched_count,
                            mouse.column,
                            mouse.row,
                        ) {
                            PaletteClick::Outside => state.palette.is_open = false,
                            PaletteClick::Inside => {}
                            PaletteClick::Item(idx) => {
                                state.palette.selected_index = idx;
                                if let Some(cmd) =
                                    get_selected_command(&state.command_registry, &state.palette)
                                {
                                    let act = cmd.action.clone();
                                    state.palette.is_open = false;
                                    return execute_action(state, act);
                                }
                            }
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

            let tray_w = tray_width(state);

            if state.tab_tray.is_open
                && mouse.column < tray_w
                && mouse.row < state.term_height.saturating_sub(1)
                && !matches!(mouse.kind, MouseEventKind::Down(_))
            {
                let tray_items = build_tray_items(state);
                let filtered = filter_tray_items(&tray_items, &state.tab_tray.search_query);
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

            let content_area = content_area(state);

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
                        let tray_area =
                            Rect::new(0, 0, tray_w, state.term_height.saturating_sub(1));
                        let tray_items = build_tray_items(state);
                        let target = tray_item_at_row(tray_area, &tray_items, &state.tab_tray, row)
                            .and_then(|idx| {
                                filter_tray_items(&tray_items, &state.tab_tray.search_query)
                                    .get(idx)
                                    .map(|item| item.pane_id)
                            });
                        if let Some(target_pid) = target {
                            state.tab_tray.is_open = false;
                            focus_pane_anywhere(state, target_pid);
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
                                        close_pane(state, pid);
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
                                // Start text selection inside the pane's content area
                                for p in state.panes.values_mut() {
                                    p.clear_selection();
                                }
                                let rel_col = col
                                    .saturating_sub(rect.x + 1)
                                    .min(rect.width.saturating_sub(3));
                                let rel_row = row
                                    .saturating_sub(rect.y + 1)
                                    .min(rect.height.saturating_sub(3));
                                if let Some(pane) = state.panes.get_mut(&pid) {
                                    pane.start_selection(rel_col, rel_row);
                                }
                                state.selecting_pane = Some((pid, rect, (rel_col, rel_row)));
                                break;
                            }
                        }
                    }
                }
                MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                    if let Some((pid, rect, _)) = state.selecting_pane {
                        let rel_col = mouse
                            .column
                            .saturating_sub(rect.x + 1)
                            .min(rect.width.saturating_sub(3));
                        let rel_row = mouse
                            .row
                            .saturating_sub(rect.y + 1)
                            .min(rect.height.saturating_sub(3));
                        if let Some(pane) = state.panes.get_mut(&pid) {
                            pane.update_selection(rel_col, rel_row);
                        }
                    }
                }
                MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
                    if let Some((pid, rect, start_pos)) = state.selecting_pane.take() {
                        let rel_col = mouse
                            .column
                            .saturating_sub(rect.x + 1)
                            .min(rect.width.saturating_sub(3));
                        let rel_row = mouse
                            .row
                            .saturating_sub(rect.y + 1)
                            .min(rect.height.saturating_sub(3));
                        if (rel_col, rel_row) != start_pos {
                            if let Some(pane) = state.panes.get_mut(&pid) {
                                pane.update_selection(rel_col, rel_row);
                                if let Some(text) = pane.copy_selection() {
                                    if !text.is_empty() {
                                        copy_to_clipboard(&text);
                                        state.toasts.push(Toast {
                                            title: "Clipboard".into(),
                                            message: "Copied selection to clipboard.".into(),
                                            level: ToastLevel::Info,
                                            created_at: chrono::Utc::now(),
                                            duration_secs: 2,
                                        });
                                    }
                                }
                            }
                        } else if let Some(pane) = state.panes.get_mut(&pid) {
                            pane.clear_selection();
                        }
                    }
                }
                MouseEventKind::Down(crossterm::event::MouseButton::Right) => {
                    return execute_action(state, Action::PasteFromClipboard);
                }
                _ => {}
            }
            false
        }
        AppEvent::TerminalInput(CrossEvent::Paste(text)) => {
            set_internal_clipboard(&text);
            route_paste(state, &text);
            false
        }
        AppEvent::FocusGained => {
            state.terminal_focused = true;
            false
        }
        AppEvent::FocusLost => {
            state.terminal_focused = false;
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
            if state.key_state_machine.should_show_which_key() {
                state.is_dirty = true;
            }
            if state.last_usage_query.elapsed() >= std::time::Duration::from_secs(3) {
                let old_summary = state.cached_usage_summary.clone();
                state.update_usage_summary();
                if old_summary != state.cached_usage_summary {
                    state.is_dirty = true;
                }
            }
            drain_background_results(state);
            if state.git_state.is_open && !state.git_loading {
                let needs_reload = match &state.git_cache {
                    Some(cache) => cache.loaded_at.elapsed() > std::time::Duration::from_secs(3),
                    None => true,
                };
                if needs_reload {
                    request_git_refresh(state);
                }
            }
            // Auto-lock wipes decrypted secrets from memory after the idle timeout
            if state.vault.enforce_idle_lock() {
                state.vault_state.mode = VaultMode::Locked;
                state.vault_state.clear_input();
                state.vault_state.pending_key.clear();
                state.vault_state.status_message = Some("Vault locked after idle timeout".into());
                state.is_dirty = true;
            }
            false
        }
        AppEvent::Resize { width, height } => {
            state.is_dirty = true;
            state.needs_clear = true;
            state.term_width = width;
            state.term_height = height;

            let rects = state.layout_mgr.compute_rects(content_area(state));
            for (pane_id, area) in rects {
                if let Some(pane) = state.panes.get_mut(&pane_id) {
                    let inner_w = area.width.saturating_sub(2);
                    let inner_h = area.height.saturating_sub(2);
                    pane.on_resize(inner_w, inner_h);
                }
            }
            false
        }
        AppEvent::UpdateAvailable(ver) => {
            state.available_update = Some(ver.clone());
            state.toasts.push(Toast {
                title: "Update Available".into(),
                message: format!(
                    "TAG v{} is available! Press Leader+U or run 'tag update' to upgrade.",
                    ver
                ),
                level: ToastLevel::Info,
                created_at: chrono::Utc::now(),
                duration_secs: 6,
            });
            state.is_dirty = true;
            false
        }
        _ => false,
    }
}

/// `q` / Ctrl+C on an empty workspace quits immediately only when no panes run anywhere;
/// otherwise the first press asks for confirmation.
fn confirm_quit_from_empty_workspace(state: &mut AppState) -> bool {
    let confirmed = state
        .quit_confirm_at
        .is_some_and(|t| t.elapsed() < QUIT_CONFIRM_WINDOW);
    if state.panes.is_empty() || confirmed {
        return execute_action(state, Action::Quit);
    }
    state.quit_confirm_at = Some(std::time::Instant::now());
    let running = state.panes.len();
    push_toast(
        state,
        "Quit TAG?",
        format!(
            "{} pane(s) are still running in other workspaces. Press again to quit and stop them.",
            running
        ),
        ToastLevel::Warning,
        QUIT_CONFIRM_WINDOW.as_secs(),
    );
    false
}

fn git_toast_result(
    state: &mut AppState,
    ok_title: &str,
    err_title: &str,
    res: Result<String, String>,
) {
    match res {
        Ok(msg) => push_toast(state, ok_title, msg, ToastLevel::Success, 4),
        Err(msg) => push_toast(state, err_title, msg, ToastLevel::Error, 6),
    }
}

/// Main worktree path from the cached overlay data, falling back to the active repo.
fn git_main_worktree_path(state: &AppState) -> PathBuf {
    state
        .git_cache
        .as_ref()
        .and_then(|c| {
            c.worktrees
                .iter()
                .find(|w| w.is_main)
                .map(|w| w.path.clone())
        })
        .unwrap_or_else(|| get_active_repo_path(state))
}

fn open_shell_in(state: &mut AppState, title: String, dir: PathBuf) -> bool {
    let new_id = state.allocate_pane_id();
    let env = vault_env(state);
    let (w, h) = initial_pane_size(state);
    match PtyPane::spawn(
        new_id,
        title,
        PaneKind::Shell,
        None,
        &[],
        &env,
        Some(dir),
        w,
        h,
        state.config.scrollback_lines,
        state.event_tx.clone(),
    ) {
        Ok(new_pane) => {
            state.panes.insert(new_id, Box::new(new_pane));
            let ws = state.layout_mgr.active_workspace_mut();
            ws.zoomed_pane = None;
            let _ = ws.split_current(new_id, SplitDirection::Vertical);
            state.needs_clear = true;
            true
        }
        Err(e) => {
            push_toast(
                state,
                "Spawn Error",
                format!("Failed to open shell: {}", e),
                ToastLevel::Error,
                4,
            );
            false
        }
    }
}

fn handle_git_overlay_key(state: &mut AppState, key: crossterm::event::KeyEvent) {
    // A modal prompt inside the git overlay takes all keys
    if let Some(prompt) = state.git_state.prompt.clone() {
        match prompt.mode {
            GitPromptMode::CreateWorktree => match key.code {
                KeyCode::Esc => state.git_state.prompt = None,
                KeyCode::Backspace => {
                    if let Some(ref mut p) = state.git_state.prompt {
                        p.input.pop();
                    }
                }
                KeyCode::Enter => {
                    let name = prompt.input.trim().to_string();
                    state.git_state.prompt = None;
                    if name.is_empty() {
                        return;
                    }
                    let active_path = get_active_repo_path(state);
                    match state
                        .git_backend
                        .create_worktree(&active_path, &name, &name)
                    {
                        Ok(wt_path) => {
                            state.git_state.is_open = false;
                            state.git_cache = None;
                            if open_shell_in(state, format!("wt: {}", name), wt_path) {
                                push_toast(
                                    state,
                                    "Worktree Created",
                                    format!("Created worktree and opened shell in {}", name),
                                    ToastLevel::Success,
                                    3,
                                );
                            }
                        }
                        Err(e) => push_toast(
                            state,
                            "Worktree Creation Failed",
                            e.to_string(),
                            ToastLevel::Error,
                            5,
                        ),
                    }
                }
                _ => {
                    if let Some(c) = text_input_char(&key) {
                        if let Some(ref mut p) = state.git_state.prompt {
                            p.input.push(c);
                        }
                    }
                }
            },
            GitPromptMode::ConfirmMerge {
                branch,
                target_branch,
                ..
            } => match key.code {
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    state.git_state.prompt = None;
                }
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    state.git_state.prompt = None;
                    let main_path = git_main_worktree_path(state);
                    let res = state
                        .git_backend
                        .merge_branch(&main_path, &branch)
                        .map(|msg| {
                            format!(
                                "Branch '{}' merged into '{}': {}",
                                branch, target_branch, msg
                            )
                        })
                        .map_err(|e| format!("{} (press A to abort the merge)", e));
                    request_git_refresh(state);
                    git_toast_result(state, "Worktree Merged", "Merge Conflict / Error", res);
                }
                _ => {}
            },
            GitPromptMode::ConfirmDelete {
                worktree_name,
                worktree_path,
            } => match key.code {
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    state.git_state.prompt = None;
                }
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    state.git_state.prompt = None;
                    let main_path = git_main_worktree_path(state);
                    let res = state
                        .git_backend
                        .delete_worktree(&main_path, &worktree_path, false)
                        .map(|_| format!("Worktree '{}' removed", worktree_name))
                        .map_err(|e| e.to_string());
                    if res.is_ok() {
                        state.git_state.selected_index = 0;
                        state.git_state.selected_worktree = 0;
                    }
                    request_git_refresh(state);
                    git_toast_result(state, "Worktree Removed", "Removal Failed", res);
                }
                _ => {}
            },
        }
        return;
    }

    let selected_worktree = state
        .git_cache
        .as_ref()
        .and_then(|c| c.worktrees.get(state.git_state.selected_worktree).cloned());

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.git_state.is_open = false;
        }
        KeyCode::Tab | KeyCode::BackTab => {
            let section = state.git_state.selected_section;
            state.git_state.selected_section = if key.code == KeyCode::Tab {
                (section + 1) % 3
            } else {
                (section + 2) % 3
            };
            // The worktree list remembers its selection; other lists start at the top
            state.git_state.selected_index = if state.git_state.selected_section == 0 {
                state.git_state.selected_worktree
            } else {
                0
            };
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if state.git_state.selected_section == 2 {
                state.git_state.diff_scroll = state.git_state.diff_scroll.saturating_sub(1);
            } else {
                state.git_state.selected_index = state.git_state.selected_index.saturating_sub(1);
                if state.git_state.selected_section == 0
                    && state.git_state.selected_worktree != state.git_state.selected_index
                {
                    state.git_state.selected_worktree = state.git_state.selected_index;
                    state.git_state.diff_scroll = 0;
                    request_git_refresh(state);
                }
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.git_state.selected_section == 2 {
                state.git_state.diff_scroll = state.git_state.diff_scroll.saturating_add(1);
            } else {
                let max_idx = match state.git_state.selected_section {
                    0 => state
                        .git_cache
                        .as_ref()
                        .map(|c| c.worktrees.len())
                        .unwrap_or(0),
                    _ => state
                        .git_cache
                        .as_ref()
                        .map(|c| c.repo_status.changed_files.len())
                        .unwrap_or(0),
                };
                if state.git_state.selected_index + 1 < max_idx {
                    state.git_state.selected_index += 1;
                    if state.git_state.selected_section == 0 {
                        state.git_state.selected_worktree = state.git_state.selected_index;
                        state.git_state.diff_scroll = 0;
                        request_git_refresh(state);
                    }
                }
            }
        }
        KeyCode::Char('c') | KeyCode::Char('n') | KeyCode::Char('a')
            if state.git_state.selected_section == 0 =>
        {
            state.git_state.prompt = Some(GitPromptState {
                mode: GitPromptMode::CreateWorktree,
                input: String::new(),
            });
        }
        KeyCode::Char('A') => {
            let main_path = git_main_worktree_path(state);
            let res = state
                .git_backend
                .abort_merge(&main_path)
                .map(|_| "Merge aborted; worktree restored".to_string())
                .map_err(|e| e.to_string());
            request_git_refresh(state);
            git_toast_result(state, "Merge Aborted", "Abort Failed", res);
        }
        KeyCode::Char('m') if state.git_state.selected_section == 0 => match selected_worktree {
            Some(wt) if !wt.is_main => {
                if let Some(branch) = wt.branch {
                    let target_branch = state
                        .git_cache
                        .as_ref()
                        .and_then(|c| c.worktrees.iter().find(|w| w.is_main))
                        .and_then(|w| w.branch.clone())
                        .unwrap_or_else(|| "the main worktree's HEAD".into());
                    state.git_state.prompt = Some(GitPromptState {
                        mode: GitPromptMode::ConfirmMerge {
                            worktree_name: wt.name,
                            branch,
                            target_branch,
                        },
                        input: String::new(),
                    });
                }
            }
            Some(_) => push_toast(
                state,
                "Cannot Merge Main",
                "Select a feature worktree to merge into the main worktree",
                ToastLevel::Warning,
                3,
            ),
            None => {}
        },
        KeyCode::Char('d') | KeyCode::Char('x') if state.git_state.selected_section == 0 => {
            match selected_worktree {
                Some(wt) if !wt.is_main => {
                    state.git_state.prompt = Some(GitPromptState {
                        mode: GitPromptMode::ConfirmDelete {
                            worktree_name: wt.name,
                            worktree_path: wt.path,
                        },
                        input: String::new(),
                    });
                }
                Some(_) => push_toast(
                    state,
                    "Cannot Delete Main",
                    "Cannot remove the primary repository worktree",
                    ToastLevel::Warning,
                    3,
                ),
                None => {}
            }
        }
        KeyCode::Enter if state.git_state.selected_section == 0 => {
            if let Some(wt) = selected_worktree {
                state.git_state.is_open = false;
                if open_shell_in(state, format!("wt: {}", wt.name), wt.path) {
                    push_toast(
                        state,
                        "Worktree Opened",
                        format!("Opened shell in {}", wt.name),
                        ToastLevel::Info,
                        3,
                    );
                }
            }
        }
        _ => {}
    }
}

fn execute_action(state: &mut AppState, action: Action) -> bool {
    match action {
        Action::Quit => {
            // The session is saved once when the main loop exits
            return true;
        }
        Action::Redraw => {
            state.needs_clear = true;
            state.is_dirty = true;
            return false;
        }
        Action::CheckUpdate => {
            state.toasts.push(Toast {
                title: "Checking for updates...".into(),
                message: "Querying GitHub Releases for new TAG versions...".into(),
                level: ToastLevel::Info,
                created_at: chrono::Utc::now(),
                duration_secs: 3,
            });
            let tx = state.event_tx.clone();
            let current_ver = TAG_VERSION.to_string();
            std::thread::spawn(move || {
                if let Some(newer) = crate::update::check_for_github_update(&current_ver) {
                    let _ = tx.send(AppEvent::UpdateAvailable(newer));
                }
            });
            return false;
        }
        Action::Update => {
            let msg = if let Some(ref ver) = state.available_update {
                format!(
                    "New TAG v{} ready! Exit TAG and run 'tag update' in your shell to upgrade.",
                    ver
                )
            } else {
                "Run 'tag update' in your shell to check and update to the latest release.".into()
            };
            state.toasts.push(Toast {
                title: "Update TAG".into(),
                message: msg,
                level: ToastLevel::Info,
                created_at: chrono::Utc::now(),
                duration_secs: 6,
            });
            return false;
        }
        Action::ToggleTabTray => {
            state.tab_tray.is_open = !state.tab_tray.is_open;
            state.needs_clear = true;
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
                request_git_refresh(state);
            } else {
                state.git_cache = None;
                state.git_state.prompt = None;
            }
        }
        Action::ToggleSnippetOverlay => {
            state.snippet_state.is_open = !state.snippet_state.is_open;
            state.snippet_state.fill = None;
            if state.snippet_state.is_open {
                let snippets_path = dirs_or_home()
                    .join(".config")
                    .join("tag")
                    .join("snippets.toml");
                state.snippets = SnippetBank::load_from_file_or_default(&snippets_path);
                if state.snippet_state.selected_index >= state.snippets.snippets.len() {
                    state.snippet_state.selected_index =
                        state.snippets.snippets.len().saturating_sub(1);
                }
            }
        }
        Action::ToggleVaultOverlay => {
            state.vault_state.is_open = !state.vault_state.is_open;
            if state.vault_state.is_open {
                if state.vault.is_unlocked() {
                    state.vault_state.mode = VaultMode::Viewing;
                } else {
                    state.vault_state.mode = VaultMode::Locked;
                }
                state.vault_state.clear_input();
                state.vault_state.clear_pending_passphrase();
                state.vault_state.pending_key.clear();
                state.vault_state.status_message = None;
            }
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
            state.needs_clear = true;
        }
        Action::UndoLayout => {
            state.layout_mgr.active_workspace_mut().undo_layout();
            state.needs_clear = true;
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
            let env = vault_env(state);
            let (w, h) = initial_pane_size(state);
            // New splits start where the focused pane currently is
            let split_cwd = focused_pane_cwd(state).or_else(|| Some(dirs_or_home()));

            if let Ok(new_pane) = PtyPane::spawn(
                new_id,
                format!("Shell-{}", new_id.0),
                PaneKind::Shell,
                None,
                &[],
                &env,
                split_cwd,
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
                state.needs_clear = true;
            }
        }
        Action::ClosePane => {
            if let Some(focused) = state.layout_mgr.active_workspace().focused_pane {
                close_pane(state, focused);
            }
        }
        Action::SwitchWorkspaceIndex(idx) => {
            if state.layout_mgr.switch_workspace_index(idx) {
                state.needs_clear = true;
            }
        }
        Action::SendLeaderKey => {
            let leader = state.key_state_machine.keymap().leader.clone();
            let ev = crossterm::event::KeyEvent::new(leader.code, leader.modifiers);
            if let Some(focused) = state.layout_mgr.active_workspace().focused_pane {
                if let Some(pane) = state.panes.get_mut(&focused) {
                    let bytes = key_event_to_bytes(ev, pane.application_cursor());
                    pane.send_input(&bytes);
                }
            }
        }
        Action::SwitchWorkspace(ws_id) => {
            state.layout_mgr.switch_workspace(ws_id);
            state.needs_clear = true;
        }
        Action::NextWorkspace => {
            state.layout_mgr.next_workspace();
            state.needs_clear = true;
        }
        Action::PrevWorkspace => {
            state.layout_mgr.prev_workspace();
            state.needs_clear = true;
        }
        Action::NewWorkspace => {
            // Capture the focused pane's directory before the new workspace becomes active
            let ws_cwd_hint = focused_pane_cwd(state);
            let _new_id = state
                .layout_mgr
                .add_workspace(format!("WS-{}", state.layout_mgr.workspaces().len() + 1));
            let ws_cwd = ws_cwd_hint.or_else(|| Some(dirs_or_home()));
            let new_pid = state.allocate_pane_id();
            let (w, h) = initial_pane_size(state);
            let env = vault_env(state);

            if let Ok(new_pane) = PtyPane::spawn(
                new_pid,
                "Shell".into(),
                PaneKind::Shell,
                None,
                &[],
                &env,
                ws_cwd,
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
                state.needs_clear = true;
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
                    if let Some(mut pane) = state.panes.remove(pid) {
                        pane.kill();
                    }
                    state.attention_queue.clear_by_pane(*pid);
                }
                state.needs_clear = true;
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
            // Move focus to the pane that is actually in that direction on screen
            let area = content_area(state);
            let ws = state.layout_mgr.active_workspace();
            if let Some(curr) = ws.focused_pane {
                if let Some(next) = ws.neighbor_in_direction(area, curr, dir) {
                    state.layout_mgr.active_workspace_mut().focused_pane = Some(next);
                }
            }
        }
        Action::FocusPane(pid) => {
            focus_pane_anywhere(state, pid);
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
                        state.needs_clear = true;
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
                    state.needs_clear = true;
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
            // Save next to the focused pane's project so the agent there can read it
            let cwd = focused_pane_cwd(state)
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            match ImageBridge::save_clipboard_image(&cwd) {
                Ok(path) => {
                    let text = path.to_string_lossy().to_string();
                    paste_into_focused(state, &text);
                    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("image");
                    state.toasts.push(Toast {
                        title: "Image Bridge".into(),
                        message: format!("Pasted {}", filename),
                        level: ToastLevel::Info,
                        created_at: chrono::Utc::now(),
                        duration_secs: 3,
                    });
                }
                Err(e) => {
                    state.toasts.push(Toast {
                        title: "Image Bridge".into(),
                        message: format!("Image paste failed: {}", e),
                        level: ToastLevel::Error,
                        created_at: chrono::Utc::now(),
                        duration_secs: 6,
                    });
                }
            }
        }
        Action::StartTour => {
            state.tour.is_active = true;
            state.tour.step = 0;
        }
        Action::ReloadConfig => {
            let (config, error) = load_app_config();
            if let Some(err) = error {
                // Keep the current settings rather than silently falling back to defaults
                state.config_error = Some(err.clone());
                push_toast(
                    state,
                    "Config Error",
                    format!("Not reloaded: {}", err),
                    ToastLevel::Error,
                    8,
                );
            } else {
                state.config_error = None;
                state.config = config;
                state.theme = Theme::from_config(state.config.theme.clone());
                state.key_state_machine =
                    KeyStateMachine::from_config(&state.config, &state.command_registry);
                push_toast(
                    state,
                    "Config",
                    "Configuration reloaded.",
                    ToastLevel::Info,
                    3,
                );
            }
        }
        Action::CopySelection => {
            if let Some(focused) = state.layout_mgr.active_workspace().focused_pane {
                if let Some(pane) = state.panes.get_mut(&focused) {
                    if let Some(text) = pane.copy_selection() {
                        if !text.is_empty() {
                            copy_to_clipboard(&text);
                            pane.clear_selection();
                            state.toasts.push(Toast {
                                title: "Clipboard".into(),
                                message: "Copied selection to clipboard.".into(),
                                level: ToastLevel::Info,
                                created_at: chrono::Utc::now(),
                                duration_secs: 2,
                            });
                        }
                    }
                }
            }
        }
        Action::PasteFromClipboard => {
            if let Some(text) = get_clipboard_text() {
                route_paste(state, &text);
            } else {
                push_toast(
                    state,
                    "Clipboard Empty",
                    "No text found in system or TAG clipboard.",
                    ToastLevel::Warning,
                    3,
                );
            }
        }
        Action::SpawnApplet { name } => {
            if state.layout_mgr.active_workspace().zoomed_pane.is_some() {
                state.toasts.push(Toast {
                    title: "Applet Disabled".into(),
                    message: "Pane is maximized. Restore before opening an applet.".into(),
                    level: ToastLevel::Warning,
                    created_at: chrono::Utc::now(),
                    duration_secs: 3,
                });
                return false;
            }
            let pid = state.allocate_pane_id();
            let Some(applet_pane) = make_applet(&name, pid) else {
                push_toast(
                    state,
                    "Unknown Applet",
                    format!("No applet named '{}' (available: chat, poker).", name),
                    ToastLevel::Error,
                    3,
                );
                return false;
            };
            state.panes.insert(pid, applet_pane);
            let _ = state
                .layout_mgr
                .active_workspace_mut()
                .split_current(pid, SplitDirection::Vertical);
            state.needs_clear = true;
        }
        Action::MinimizePane(target_pid) => {
            let pid = target_pid.or(state.layout_mgr.active_workspace().focused_pane);
            if let Some(pid) = pid {
                state.layout_mgr.active_workspace_mut().minimize_pane(pid);
                state.needs_clear = true;
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
            state.needs_clear = true;
        }
        Action::Detach => {
            // Detaching keeps TAG (and every pane) running in its background tmux session
            if in_tag_background_session() {
                save_session_to_disk(state);
                run_tmux_silent(&["detach-client"]);
            } else {
                push_toast(
                    state,
                    "Detach Unavailable",
                    "TAG is running in the foreground. Start it with 'tag --bg' (or enable background mode) to detach and re-attach later.",
                    ToastLevel::Warning,
                    5,
                );
            }
            return false;
        }
        Action::ToggleBackgroundMode => {
            if let Some(err) = state.config_error.clone() {
                push_toast(
                    state,
                    "Config Error",
                    format!("Fix config.toml before changing settings ({})", err),
                    ToastLevel::Error,
                    6,
                );
                return false;
            }
            state.config.run_in_background = !state.config.run_in_background;
            if let Err(e) = save_app_config(&state.config) {
                push_toast(
                    state,
                    "Config",
                    format!("Could not save config: {}", e),
                    ToastLevel::Error,
                    5,
                );
            }
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

fn make_applet(name: &str, pid: PaneId) -> Option<Box<dyn Pane>> {
    match name {
        "poker" => Some(Box::new(PokerPane::new(pid))),
        "chat" => Some(Box::new(PeerChatPane::new(pid))),
        _ => None,
    }
}

fn push_toast(
    state: &mut AppState,
    title: &str,
    message: impl Into<String>,
    level: ToastLevel,
    duration_secs: u64,
) {
    state.toasts.push(Toast {
        title: title.into(),
        message: message.into(),
        level,
        created_at: chrono::Utc::now(),
        duration_secs,
    });
    state.is_dirty = true;
}

impl AppState {
    fn allocate_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        id
    }
}

/// Map Ctrl+<key> to its control byte (as xterm does).
fn ctrl_byte(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        ' ' | '@' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// Encode a key event the way xterm would, so programs in panes receive
/// navigation keys, function keys and modifier combinations.
fn key_event_to_bytes(key: crossterm::event::KeyEvent, app_cursor: bool) -> Vec<u8> {
    let mods = key.modifiers;
    let shift = mods.contains(KeyModifiers::SHIFT);
    let alt = mods.contains(KeyModifiers::ALT);
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    // xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl
    let m = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;

    // CSI 1;<m><final> with modifiers, otherwise CSI/SS3 <final>
    let cursor = |fin: char| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{}{}", m, fin).into_bytes()
        } else if app_cursor {
            format!("\x1bO{}", fin).into_bytes()
        } else {
            format!("\x1b[{}", fin).into_bytes()
        }
    };
    // CSI <n>[;<m>]~
    let tilde = |n: u8| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[{};{}~", n, m).into_bytes()
        } else {
            format!("\x1b[{}~", n).into_bytes()
        }
    };
    let with_alt = |mut bytes: Vec<u8>| -> Vec<u8> {
        if alt {
            bytes.insert(0, 0x1b);
        }
        bytes
    };

    match key.code {
        KeyCode::Char(c) => {
            let bytes = if ctrl {
                ctrl_byte(c)
                    .map(|b| vec![b])
                    .unwrap_or_else(|| c.to_string().into_bytes())
            } else {
                c.to_string().into_bytes()
            };
            with_alt(bytes)
        }
        KeyCode::Enter => with_alt(vec![b'\r']),
        KeyCode::Backspace => with_alt(vec![if ctrl { 0x08 } else { 0x7f }]),
        KeyCode::Tab => with_alt(vec![b'\t']),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => with_alt(vec![0x1b]),
        KeyCode::Up => cursor('A'),
        KeyCode::Down => cursor('B'),
        KeyCode::Right => cursor('C'),
        KeyCode::Left => cursor('D'),
        KeyCode::Home => cursor('H'),
        KeyCode::End => cursor('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => {
            let fin = (b'P' + (n - 1)) as char;
            if m > 1 {
                format!("\x1b[1;{}{}", m, fin).into_bytes()
            } else {
                format!("\x1bO{}", fin).into_bytes()
            }
        }
        KeyCode::F(n @ 5..=12) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                _ => 24,
            };
            tilde(code)
        }
        _ => Vec::new(),
    }
}

/// Printable character for a text field, ignoring Ctrl/Alt chords.
fn text_input_char(key: &crossterm::event::KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            Some(c)
        }
        _ => None,
    }
}

fn vault_env(state: &mut AppState) -> HashMap<String, String> {
    if state.vault.is_unlocked() {
        state.vault.get_env_set()
    } else {
        HashMap::new()
    }
}

/// Size for a newly spawned pane before its first render resizes it.
fn initial_pane_size(state: &AppState) -> (u16, u16) {
    if state.term_width > 0 && state.term_height > 0 {
        (
            state.term_width.saturating_sub(2),
            state.term_height.saturating_sub(4),
        )
    } else {
        (80, 24)
    }
}

/// Screen area used by panes (excludes the status bar and an open tab tray).
fn content_area(state: &AppState) -> Rect {
    let height = state.term_height.saturating_sub(1);
    let tray_w = tray_width(state);
    Rect {
        x: tray_w,
        y: 0,
        width: state.term_width.saturating_sub(tray_w),
        height,
    }
}

fn tray_width(state: &AppState) -> u16 {
    if state.tab_tray.is_open {
        (state.term_width * 25 / 100)
            .clamp(24, 45)
            .min(state.term_width)
    } else {
        0
    }
}

/// Current directory of the focused pane (falls back to its spawn directory).
fn focused_pane_cwd(state: &AppState) -> Option<PathBuf> {
    let pid = state.layout_mgr.active_workspace().focused_pane?;
    let pane = state.panes.get(&pid)?;
    let (_, _, cwd) = pane.running_process();
    cwd.or_else(|| pane.cwd()).filter(|p| p.is_dir())
}

/// Focus a pane wherever it lives: switch to its workspace and restore it if minimized.
fn focus_pane_anywhere(state: &mut AppState, pid: PaneId) -> bool {
    let Some(ws_id) = state.layout_mgr.find_pane_workspace(pid) else {
        return false;
    };
    state.layout_mgr.switch_workspace(ws_id);
    let ws = state.layout_mgr.active_workspace_mut();
    if ws.is_minimized(pid) {
        ws.restore_pane(pid, None);
    }
    ws.focused_pane = Some(pid);
    state.needs_clear = true;
    true
}

/// Close a pane: remove it from every workspace, kill its process and drop its alerts.
fn close_pane(state: &mut AppState, pid: PaneId) {
    for ws in state.layout_mgr.workspaces_mut() {
        ws.remove_pane(pid);
    }
    if let Some(mut pane) = state.panes.remove(&pid) {
        pane.kill();
    }
    state.attention_queue.clear_by_pane(pid);
    state.needs_clear = true;
}

/// Send text to the focused pane as a paste (bracketed only if the program enabled it).
fn paste_into_focused(state: &mut AppState, text: &str) {
    if let Some(focused) = state.layout_mgr.active_workspace().focused_pane {
        if let Some(pane) = state.panes.get_mut(&focused) {
            pane.clear_selection();
            let bytes = if pane.bracketed_paste() {
                wrap_bracketed_paste(text)
            } else {
                text.as_bytes().to_vec()
            };
            pane.send_input(&bytes);
            pane.scroll_to_bottom();
            state.is_dirty = true;
        }
    }
}

/// Deliver pasted text to whichever input currently has focus: a text field in an
/// open overlay, or the focused pane when no overlay is open. Overlays without a
/// text field swallow the paste instead of sending it to the pane behind them.
fn route_paste(state: &mut AppState, text: &str) {
    state.is_dirty = true;
    if let Some(ref mut prompt) = state.rename_prompt {
        prompt.input.push_str(text);
    } else if state.palette.is_open {
        state.palette.query.push_str(text);
        state.palette.selected_index = 0;
    } else if state.help_open
        || state.tour.is_active
        || state.attention_state.is_open
        || state.usage_state.is_open
    {
        // no text input
    } else if state.git_state.is_open {
        if let Some(ref mut prompt) = state.git_state.prompt {
            if prompt.mode == GitPromptMode::CreateWorktree {
                prompt.input.push_str(text);
            }
        }
    } else if state.snippet_state.is_open {
        if let Some(ref mut fill) = state.snippet_state.fill {
            fill.input.push_str(text);
        }
    } else if state.vault_state.is_open {
        let accepts_text = !state.vault.is_unlocked()
            || matches!(
                state.vault_state.mode,
                VaultMode::AddingKey | VaultMode::AddingValue
            );
        if accepts_text {
            state.vault_state.input_buffer.push_str(text);
        }
    } else if state.tab_tray.is_open {
        state.tab_tray.search_query.push_str(text);
        state.tab_tray.is_searching = true;
        state.tab_tray.selected_index = 0;
    } else {
        paste_into_focused(state, text);
    }
}

/// True when this TAG instance is running inside its own background tmux session.
fn in_tag_background_session() -> bool {
    if !inside_tmux() {
        return false;
    }
    std::process::Command::new("tmux")
        .args(["display-message", "-p", "#{session_name}"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == TAG_SESSION_NAME)
        .unwrap_or(false)
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
        let (_, _, cwd) = pane.running_process();
        let tmux_session = pane.tmux_session_name().or_else(|| {
            pane.title()
                .strip_prefix("tmux: ")
                .and_then(|s| s.split([' ', '(']).next())
                .filter(|s| !s.is_empty())
                .map(tag_pty::clean_tmux_session_name)
        });

        let (command, args, kind, title) = if let Some(name) = pane.applet_name() {
            (None, vec![name.to_string()], PaneKind::Applet, pane.title())
        } else if let Some(session) = tmux_session {
            (
                Some("tmux".to_string()),
                vec![
                    "new-session".to_string(),
                    "-A".to_string(),
                    "-s".to_string(),
                    session.clone(),
                ],
                if pane.kind() == PaneKind::Agent {
                    PaneKind::Agent
                } else if pane.kind() == PaneKind::Donut {
                    PaneKind::Donut
                } else {
                    PaneKind::Tmux
                },
                format!("tmux: {}", session),
            )
        } else {
            let (cmd, args) = match pane.kind() {
                PaneKind::Shell => (None, Vec::new()),
                _ => {
                    let (cmd, args, _) = pane.running_process();
                    (cmd, args)
                }
            };
            (cmd, args, pane.kind(), pane.title())
        };
        saved.panes.insert(
            pid,
            SavedPane {
                id: pid,
                title,
                kind,
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

fn candidate_repo_dir(state: &AppState) -> PathBuf {
    if let Some(p) = focused_pane_cwd(state) {
        return p;
    }
    if let Some(ref binding) = state.layout_mgr.active_workspace().worktree_binding {
        if binding.exists() {
            return binding.clone();
        }
    }
    for pane in state.panes.values() {
        let (_, _, cwd) = pane.running_process();
        if let Some(p) = cwd.or_else(|| pane.cwd()) {
            if p.exists() {
                return p;
            }
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Main repository root for `search_path` (shared by all of its worktrees).
fn resolve_repo_root(search_path: &std::path::Path) -> PathBuf {
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(search_path)
        .output()
    {
        if output.status.success() {
            let common_git_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let p = PathBuf::from(common_git_dir);
            if let Some(parent) = p.parent() {
                if parent.exists() {
                    return parent.to_path_buf();
                }
            }
        }
    }

    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(search_path)
        .output()
    {
        if output.status.success() {
            let toplevel = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !toplevel.is_empty() {
                return PathBuf::from(toplevel);
            }
        }
    }

    search_path.to_path_buf()
}

fn get_active_repo_path(state: &AppState) -> PathBuf {
    resolve_repo_root(&candidate_repo_dir(state))
}

/// Recompute the git overlay data on a background thread. Results are applied on the
/// next tick; responses to superseded requests are discarded by generation.
fn request_git_refresh(state: &mut AppState) {
    state.git_generation += 1;
    state.git_loading = true;
    let generation = state.git_generation;
    let search_path = candidate_repo_dir(state);
    let selected = state.git_state.selected_worktree;
    let tx = state.git_tx.clone();
    let _ = std::thread::Builder::new()
        .name("git-refresh".into())
        .spawn(move || {
            let backend = StandardGitBackend;
            let repo = resolve_repo_root(&search_path);
            let worktrees = backend.list_worktrees(&repo).unwrap_or_default();
            let target = worktrees
                .get(selected)
                .map(|w| w.path.clone())
                .unwrap_or_else(|| repo.clone());
            let repo_status = backend.repo_status(&target).unwrap_or_default();
            let diff = backend.get_diff(&target).unwrap_or_default();
            let _ = tx.send((
                generation,
                CachedGitData {
                    worktrees,
                    repo_status,
                    diff,
                    loaded_at: std::time::Instant::now(),
                },
            ));
        });
}

/// Apply finished background work (usage scans, git refreshes) to the UI state.
fn drain_background_results(state: &mut AppState) {
    let mut usage_changed = false;
    while let Ok(events) = state.usage_rx.try_recv() {
        if state.usage_store.insert_events_batch(&events).unwrap_or(0) > 0 {
            usage_changed = true;
        }
    }
    if usage_changed {
        state.update_usage_summary();
        state.is_dirty = true;
    }

    while let Ok((generation, data)) = state.git_rx.try_recv() {
        if generation != state.git_generation {
            continue;
        }
        state.git_loading = false;
        if state.git_state.is_open {
            let count = data.worktrees.len();
            if state.git_state.selected_worktree >= count {
                state.git_state.selected_worktree = count.saturating_sub(1);
                if state.git_state.selected_section == 0 {
                    state.git_state.selected_index = state.git_state.selected_worktree;
                }
            }
            state.git_cache = Some(data);
            state.is_dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn test_navigation_and_function_keys_are_encoded() {
        let none = KeyModifiers::empty();
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Delete, none), false),
            b"\x1b[3~"
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Home, none), false),
            b"\x1b[H"
        );
        assert_eq!(key_event_to_bytes(key(KeyCode::End, none), true), b"\x1bOF");
        assert_eq!(
            key_event_to_bytes(key(KeyCode::PageDown, none), false),
            b"\x1b[6~"
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::BackTab, KeyModifiers::SHIFT), false),
            b"\x1b[Z"
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::F(1), none), false),
            b"\x1bOP"
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::F(5), none), false),
            b"\x1b[15~"
        );
        assert_eq!(key_event_to_bytes(key(KeyCode::Up, none), true), b"\x1bOA");
        assert_eq!(key_event_to_bytes(key(KeyCode::Up, none), false), b"\x1b[A");
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Right, KeyModifiers::CONTROL), false),
            b"\x1b[1;5C"
        );
    }

    #[test]
    fn test_modifier_chars_are_encoded() {
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Backspace, KeyModifiers::empty()), false),
            vec![0x7f]
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Char('x'), KeyModifiers::ALT), false),
            b"\x1bx"
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            vec![3]
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Char('\\'), KeyModifiers::CONTROL), false),
            vec![0x1c]
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Char(']'), KeyModifiers::CONTROL), false),
            vec![0x1d]
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Char(' '), KeyModifiers::CONTROL), false),
            vec![0]
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Char('é'), KeyModifiers::empty()), false),
            "é".as_bytes()
        );
    }
}
