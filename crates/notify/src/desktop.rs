use notify_rust::Notification;
use tracing::warn;

pub trait Notifier: Send + Sync {
    fn notify(&self, summary: &str, body: &str);
}

pub struct SystemNotifier;

impl Notifier for SystemNotifier {
    fn notify(&self, summary: &str, body: &str) {
        let res = Notification::new()
            .appname("TAG")
            .summary(summary)
            .body(body)
            .show();

        if let Err(e) = res {
            warn!("Desktop notification failed (ignoring gracefully): {}", e);
        }
    }
}
