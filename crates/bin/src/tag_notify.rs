use clap::Parser;
use tag_notify::{Notifier, SystemNotifier};

/// Desktop notification helper: shows a native desktop notification via notify-rust.
#[derive(Parser, Debug)]
#[command(name = "tag-notify")]
#[command(version)]
#[command(about = "Show a desktop notification (used by TAG and agent hook scripts)")]
struct Cli {
    /// Notification title
    summary: String,

    /// Notification body text
    #[arg(default_value = "")]
    body: String,
}

fn main() {
    let cli = Cli::parse();
    SystemNotifier.notify(&cli.summary, &cli.body);
}
