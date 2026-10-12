use chrono::{DateTime, Utc};
use std::path::PathBuf;

use tag_core::event::HookEvent;
use tag_core::pane::PaneId;

#[derive(Debug, Clone)]
pub struct AttentionItem {
    pub id: u64,
    pub pane_id: Option<PaneId>,
    pub agent: String,
    pub event: HookEvent,
    pub message: String,
    pub worktree: Option<PathBuf>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct AttentionQueue {
    items: Vec<AttentionItem>,
    next_id: u64,
}

impl AttentionQueue {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
        }
    }

    pub fn push(
        &mut self,
        pane_id: Option<PaneId>,
        agent: String,
        event: HookEvent,
        message: String,
        worktree: Option<PathBuf>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        self.items.push(AttentionItem {
            id,
            pane_id,
            agent,
            event,
            message,
            worktree,
            timestamp: Utc::now(),
        });
        id
    }

    pub fn items(&self) -> &[AttentionItem] {
        &self.items
    }

    pub fn count(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn clear_item(&mut self, id: u64) -> Option<AttentionItem> {
        if let Some(pos) = self.items.iter().position(|i| i.id == id) {
            Some(self.items.remove(pos))
        } else {
            None
        }
    }

    pub fn clear_by_pane(&mut self, pane_id: PaneId) {
        self.items.retain(|i| i.pane_id != Some(pane_id));
    }

    pub fn clear_all(&mut self) {
        self.items.clear();
    }
}
