#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use crossterm::event::Event as CrossEvent;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::collections::HashMap;
use std::io::{self};
use std::path::PathBuf;
use std::time::Duration;
use tag_agents::HookServer;
use tag_core::action::Action;
use tag_core::event::AppEvent;
use tag_core::pane::{PaneId, PaneKind};
use tag_layout::{LayoutManager, SavedSession, Workspace};
use tag_pty::PtyPane;
use tag_ui::ToastLevel;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

pub(crate) async fn run_app(
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

    spawn_tick(event_tx.clone());
    spawn_signal_listener(event_tx.clone());

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
    let paths = AppPaths::user_default();
    let (config, _) = load_app_config(&paths.config_path);
    let usage_rx = spawn_usage_scanner(dirs_or_home());
    let mut state = AppState::new(config, config_error, paths, event_tx.clone(), usage_rx)?;
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
    if let Some(e) = state.start_control_server().await {
        push_toast(
            &mut state,
            "Control Socket Unavailable",
            e,
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

    restore_session(&mut state, term_size.width, term_size.height);

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
            if let Some(newer) = crate::update::check_for_github_update(TAG_VERSION) {
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

/// Restore the saved session (panes and workspaces) from `state.paths.session_path`.
/// `width`/`height` are the terminal size used to size respawned PTYs.
pub(crate) fn restore_session(state: &mut AppState, width: u16, height: u16) {
    // Restore saved session if present, otherwise start with empty workspace showing ASCII art
    let session_path = state.paths.session_path.clone();
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
                        width.saturating_sub(2),
                        height.saturating_sub(4),
                        state.config.scrollback_lines,
                        state.event_tx.clone(),
                    ) {
                        state.panes.insert(pid, Box::new(pane));
                    }
                }
                if !state.panes.is_empty() {
                    let mut workspaces: Vec<Workspace> =
                        saved.workspaces.into_iter().map(Into::into).collect();
                    for ws in &mut workspaces {
                        for pid in ws.all_panes() {
                            if !state.panes.contains_key(&pid) {
                                ws.remove_pane(pid);
                            }
                        }
                        ws.minimized_panes.retain(|p| state.panes.contains_key(p));
                    }
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
}

/// Start the background transcript scanner: re-parses only changed files and streams new events.
pub(crate) fn spawn_usage_scanner(
    home: PathBuf,
) -> std::sync::mpsc::Receiver<Vec<tag_usage::UsageEvent>> {
    let (usage_tx, usage_rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("usage-scanner".into())
        .spawn(move || {
            let mut scanner = tag_usage::UsageScanner::new(home);
            loop {
                let events = scanner.scan_changed();
                if !events.is_empty() && usage_tx.send(events).is_err() {
                    break;
                }
                std::thread::sleep(USAGE_SCAN_INTERVAL);
            }
        });
    usage_rx
}

/// Spawn the 100ms tick used for animations, toast expiry and background result draining.
pub(crate) fn spawn_tick(event_tx: UnboundedSender<AppEvent>) {
    let tick_tx = event_tx;
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        loop {
            interval.tick().await;
            if tick_tx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });
}

/// Quit gracefully (saving the session) on SIGHUP, SIGTERM or SIGINT.
pub(crate) fn spawn_signal_listener(event_tx: UnboundedSender<AppEvent>) {
    let sig_tx = event_tx;
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate()).ok();
            let mut sighup = signal(SignalKind::hangup()).ok();
            let mut sigint = signal(SignalKind::interrupt()).ok();

            tokio::select! {
                _ = async {
                    if let Some(ref mut s) = sigterm { s.recv().await } else { std::future::pending().await }
                } => {},
                _ = async {
                    if let Some(ref mut s) = sighup { s.recv().await } else { std::future::pending().await }
                } => {},
                _ = async {
                    if let Some(ref mut s) = sigint { s.recv().await } else { std::future::pending().await }
                } => {},
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }

        let _ = sig_tx.send(AppEvent::Action(Action::Quit));
    });
}
