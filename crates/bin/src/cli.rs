#[allow(unused_imports)]
use crate::{app::*, session_io::*, sockets::*, tmux::*};
use clap::{Parser, Subcommand};
use crossterm::event::KeyModifiers;
use std::path::PathBuf;
use tag_vault::VaultStore;

#[derive(Parser, Debug)]
#[command(name = "tag")]
#[command(author = "TAG Team")]
#[command(version)]
#[command(about = "TAG: Terminal Agent Orchestrator & PTY Multiplexer")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,

    /// Run directly in foreground (bypass background daemon)
    #[arg(short, long)]
    pub(crate) foreground: bool,

    /// Force running in background daemon
    #[arg(short = 'b', long = "bg")]
    pub(crate) background: bool,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// Run an MCP server over stdio exposing TAG control methods as tools
    Mcp {
        /// Client name sent to TAG (decides default permission tiers)
        #[arg(long, default_value = "mcp")]
        client: String,
        /// Control socket path (default: auto-discover)
        #[arg(long)]
        socket: Option<PathBuf>,
    },
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
    /// Pin patterns and search the project/global brain
    Brain {
        #[command(subcommand)]
        cmd: crate::brain_cli::BrainCommands,
    },
    /// Manage encrypted secrets in the TAG vault
    Vault {
        #[command(subcommand)]
        command: VaultCommands,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum VaultCommands {
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

pub(crate) fn prompt_passphrase(
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

pub(crate) fn read_password_terminal(prompt: &str) -> Result<String, anyhow::Error> {
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

pub(crate) fn handle_vault_command(cmd: VaultCommands) -> Result<(), anyhow::Error> {
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
