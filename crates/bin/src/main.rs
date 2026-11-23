use clap::Parser;
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, stdout, Write};
use tag_agents::get_setup_hooks;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

mod app;
mod brain_cli;
mod cli;
mod ctl;
mod session_io;
mod sockets;
mod tmux;
mod update;

use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};

fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();

    if let Some(Commands::Ctl { json, socket, cmd }) = cli.command {
        std::process::exit(ctl::run_ctl(cmd, json, socket)?);
    }

    if let Some(Commands::Brain { cmd }) = cli.command {
        return brain_cli::handle_brain_command(cmd);
    }

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

    let (config, config_error) = load_app_config(&config_path());
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
