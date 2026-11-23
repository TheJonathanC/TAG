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
use tag_layout::{LayoutManager, SavedPane, SavedSession, Workspace};
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
    install_templates(&state.paths.config_dir);
    load_active_runs(&mut state);
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

/// Respawn one saved pane under id `pid` (applet or PTY). Returns whether it is now in `state.panes`.
fn spawn_saved_pane(
    state: &mut AppState,
    pid: PaneId,
    sp: SavedPane,
    width: u16,
    height: u16,
) -> bool {
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
        return state.panes.contains_key(&pid);
    }

    // Agent panes TAG launched from a profile: relaunch through the profile, resuming the
    // conversation when the profile supports it.
    if let Some(cfg) = sp
        .profile
        .as_ref()
        .and_then(|p| state.config.agents.get(p))
        .cloned()
        .filter(|_| sp.command.as_deref() != Some("tmux"))
    {
        let profile_key = sp.profile.clone().unwrap_or_default();
        let (opts, session_id, fresh) = resume_launch(&cfg, sp.resume.as_deref());
        if fresh {
            state.toasts.push(tag_ui::Toast {
                title: "Agent Restarted".into(),
                message: format!(
                    "{} can't resume conversations; started a fresh session",
                    cfg.name
                ),
                level: tag_ui::ToastLevel::Info,
                created_at: chrono::Utc::now(),
                duration_secs: 5,
            });
        }
        let (command, args) = cfg.build_argv(&opts);
        let mut prof = tag_agents::AgentProfile::from_config(&cfg);
        prof.command = command;
        prof.args = args;
        prof.env.extend(env);
        prof.default_cwd = sp.cwd.clone();
        prof.name = sp.title.clone();
        return match tag_agents::AgentPane::spawn(
            pid,
            &prof,
            None,
            width.saturating_sub(2),
            height.saturating_sub(4),
            state.config.scrollback_lines,
            state.event_tx.clone(),
        ) {
            Ok(pane) => {
                state.panes.insert(pid, Box::new(pane));
                state.pane_meta.insert(
                    pid,
                    PaneMeta {
                        profile: Some(profile_key),
                        session_id,
                        ..Default::default()
                    },
                );
                true
            }
            Err(_) => false,
        };
    }

    let mut cmd = sp.command;
    let mut args = sp.args;
    let mut title = sp.title;
    let mut kind = sp.kind;

    // Sanitize tmux session on restore
    if cmd.as_deref() == Some("tmux") || kind == PaneKind::Tmux || title.starts_with("tmux:") {
        let sess_opt = if let Some(idx) = args.iter().position(|a| a == "-s" || a == "-t") {
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

    match PtyPane::spawn(
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
        Ok(pane) => {
            state.panes.insert(pid, Box::new(pane));
            true
        }
        Err(_) => false,
    }
}

/// How to relaunch a saved agent pane: resume `saved` with the profile's `resume_args` when it
/// has them; otherwise start fresh (with a new TAG-chosen id when the profile supports one).
/// Returns (launch options, session id to record, whether a resume was wanted but impossible).
pub(crate) fn resume_launch(
    cfg: &tag_core::config::AgentProfileConfig,
    saved: Option<&str>,
) -> (tag_core::config::LaunchOpts, Option<String>, bool) {
    let mut opts = tag_core::config::LaunchOpts::default();
    match saved {
        Some(id) if !cfg.resume_args.is_empty() => {
            opts.resume_session = Some(id.to_string());
            (opts, Some(id.to_string()), false)
        }
        _ => {
            let id = (!cfg.session_id_args.is_empty()).then(new_session_uuid);
            opts.new_session_id = id.clone();
            (opts, id, saved.is_some())
        }
    }
}

/// Respawn the panes of one saved session and build its layouts. Pane ids that collide with
/// panes already in `state.panes` (another project's) get fresh ids, remapped in the layouts.
/// Panes that fail to respawn, or that no workspace references, are dropped.
/// Returns `None` when nothing was restored.
pub(crate) fn restore_project_session(
    state: &mut AppState,
    saved: SavedSession,
    width: u16,
    height: u16,
) -> Option<LayoutManager> {
    if saved.panes.is_empty() {
        return None;
    }
    let max_pid = saved.panes.keys().map(|p| p.0).max().unwrap_or(0);
    state.next_pane_id = state.next_pane_id.max(max_pid + 1);
    let mut remap: HashMap<PaneId, PaneId> = HashMap::new();
    for pid in saved.panes.keys() {
        if state.panes.contains_key(pid) {
            remap.insert(*pid, state.allocate_pane_id());
        }
    }
    let mut spawned: std::collections::HashSet<PaneId> = std::collections::HashSet::new();
    let mut entries: Vec<_> = saved.panes.into_iter().collect();
    entries.sort_by_key(|(pid, _)| pid.0);
    for (old, sp) in entries {
        let pid = remap.get(&old).copied().unwrap_or(old);
        if spawn_saved_pane(state, pid, sp, width, height) {
            spawned.insert(pid);
        }
    }
    if spawned.is_empty() {
        return None;
    }
    let mut workspaces: Vec<Workspace> = saved.workspaces.into_iter().map(Into::into).collect();
    for ws in &mut workspaces {
        ws.remap_panes(&remap);
    }
    let mut layout = LayoutManager::from_workspaces(workspaces, saved.active_workspace_id);
    // Drop layout slots for panes that failed to respawn
    layout.retain_panes(&|p| spawned.contains(&p));
    // Panes not referenced by any workspace would be unreachable
    let placed: std::collections::HashSet<PaneId> = layout
        .workspaces()
        .iter()
        .flat_map(|w| w.all_panes().into_iter().chain(w.minimized_panes.clone()))
        .collect();
    for pid in spawned.difference(&placed) {
        if let Some(mut pane) = state.panes.remove(pid) {
            pane.kill();
        }
    }
    Some(layout)
}

/// Restore every project's saved session from `<state_dir>/projects/*/session.json`; the
/// active project is the one recorded in `<state_dir>/sessions.json` (else the first).
/// With no per-project sessions, the legacy global session (`state.paths.session_path`) is
/// imported once into the `default` project and a `migrated-v1` marker is written.
/// `width`/`height` are the terminal size used to size respawned PTYs.
pub(crate) fn restore_session(state: &mut AppState, width: u16, height: u16) {
    let state_dir = state.paths.state_dir.clone();
    let mut sessions = read_project_sessions(&state_dir);
    let mut migrated = false;
    if sessions.is_empty()
        && !any_project_session_exists(&state_dir)
        && !migration_marker_path(&state_dir).exists()
        && state.paths.session_path.exists()
    {
        if let Ok(saved) = SavedSession::load_from_file(&state.paths.session_path) {
            let project = state.projects.default_project(&dirs_or_home());
            sessions.push((project.id, saved));
            migrated = true;
        }
    }

    let mut restored: Vec<(String, LayoutManager)> = Vec::new();
    for (id, saved) in sessions {
        if let Some(layout) = restore_project_session(state, saved, width, height) {
            restored.push((id, layout));
        }
    }
    if !restored.is_empty() {
        let wanted = read_active_project_index(&state_dir);
        let idx = wanted
            .and_then(|w| restored.iter().position(|(id, _)| *id == w))
            .unwrap_or(0);
        let (active_id, layout) = restored.remove(idx);
        state.layout_mgr = layout;
        for (id, layout) in restored {
            state.parked.insert(id, ParkedProject { layout });
        }
        state.projects.set_active(&active_id);
        if let Some(p) = state.projects.get(&active_id).cloned() {
            state.active_project_name = Some(p.name);
            state.active_project_root = Some(p.root);
        }
        state.active_project_id = Some(active_id);
        if state.projects_load_error.is_none() {
            let _ = state.projects.save();
        }
    }
    state.ensure_active_project();
    if migrated {
        save_session_to_disk(state);
        let _ = std::fs::write(
            migration_marker_path(&state_dir),
            "legacy session.json imported into per-project sessions\n",
        );
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

/// Headless mode (`tag daemon`): the same state and event handling as the UI, with no
/// terminal rendering or input. Serves the hook and control sockets until SIGTERM/SIGINT,
/// then saves the session and stops its panes.
pub(crate) async fn run_daemon(paths: AppPaths) -> Result<(), anyhow::Error> {
    ensure_private_dir(&paths.socket_dir);
    std::fs::create_dir_all(&paths.state_dir)?;
    let (event_tx, mut event_rx) = unbounded_channel::<AppEvent>();

    let hook_path = paths.hook_socket_path();
    std::env::set_var("COCKPIT_SOCK", &hook_path);
    let hook_server = HookServer::new(hook_path.clone());
    hook_server.start(event_tx.clone()).await?;

    let (config, config_error) = load_app_config(&paths.config_path);
    if let Some(err) = &config_error {
        eprintln!("tag daemon: config error, using defaults: {err}");
    }
    let usage_rx = spawn_usage_scanner(dirs_or_home());
    let mut state = AppState::new(config, config_error, paths, event_tx.clone(), usage_rx)?;
    install_templates(&state.paths.config_dir);
    load_active_runs(&mut state);
    if let Some(err) = state.start_control_server().await {
        let _ = std::fs::remove_file(&hook_path);
        anyhow::bail!(err);
    }
    // Hidden panes are sized 120x40 until shown (§5.4).
    state.term_width = 122;
    state.term_height = 44;
    restore_session(&mut state, 122, 44);

    spawn_tick(event_tx.clone());
    spawn_signal_listener(event_tx.clone());
    eprintln!(
        "tag daemon: control socket {}",
        state
            .control_server
            .as_ref()
            .map(|s| s.path().display().to_string())
            .unwrap_or_else(|| "disabled".into())
    );

    while let Some(event) = event_rx.recv().await {
        // Terminal-only events never arrive in daemon mode.
        if handle_app_event(&mut state, event) {
            break;
        }
    }

    save_session_to_disk(&state);
    for pane in state.panes.values_mut() {
        pane.kill();
    }
    state.control_server = None;
    let _ = std::fs::remove_file(hook_path);
    Ok(())
}

#[cfg(test)]
mod resume_tests {
    use super::*;

    #[test]
    fn restore_builds_resume_argv() {
        let cfg = tag_core::config::Config::default().agents["claude"].clone();
        let (opts, id, fresh) = resume_launch(&cfg, Some("abc-123"));
        assert!(!fresh);
        assert_eq!(id.as_deref(), Some("abc-123"));
        let (cmd, args) = cfg.build_argv(&opts);
        assert_eq!(cmd, "claude");
        let i = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[i + 1], "abc-123");
        assert!(!args.iter().any(|a| a == "--session-id"));
    }

    #[test]
    fn profile_without_resume_starts_fresh() {
        let mut cfg = tag_core::config::Config::default().agents["claude"].clone();
        cfg.resume_args.clear();
        let (opts, id, fresh) = resume_launch(&cfg, Some("abc-123"));
        assert!(fresh);
        // Still gets a new TAG-chosen id so the next restart can resume.
        let (_, args) = cfg.build_argv(&opts);
        let i = args.iter().position(|a| a == "--session-id").unwrap();
        assert_eq!(Some(&args[i + 1]), id.as_ref());
        // Nothing saved, nothing to report.
        assert!(!resume_launch(&cfg, None).2);
    }
}
