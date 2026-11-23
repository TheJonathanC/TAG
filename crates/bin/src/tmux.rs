#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*};
use tag_core::action::Action;
use tag_core::command::CommandRegistry;

pub(crate) const TAG_SESSION_NAME: &str = "tag-session";

pub(crate) fn run_tmux_silent(args: &[&str]) -> bool {
    std::process::Command::new("tmux")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub(crate) fn is_session_running(name: &str) -> bool {
    run_tmux_silent(&["has-session", "-t", name])
}

pub(crate) fn is_tmux_server_running() -> bool {
    run_tmux_silent(&["has-session"])
}

pub(crate) fn inside_tmux() -> bool {
    std::env::var_os("TMUX").is_some_and(|v| !v.is_empty())
}

pub(crate) fn attach_to_session(name: &str) -> Result<(), anyhow::Error> {
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

pub(crate) fn start_and_attach_background_session() -> Result<(), anyhow::Error> {
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

pub(crate) fn refresh_tmux_commands(registry: &mut CommandRegistry) {
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

/// True when this TAG instance is running inside its own background tmux session.
pub(crate) fn in_tag_background_session() -> bool {
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
