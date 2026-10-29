use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::pane::{PaneId, WorkspaceId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    // Navigation & Focus
    Focus(Direction),
    FocusPane(PaneId),
    FocusNext,
    FocusPrev,
    SwitchWorkspace(WorkspaceId),
    /// Switch to the workspace at this 0-based position (as shown in the status bar)
    SwitchWorkspaceIndex(usize),
    NextWorkspace,
    PrevWorkspace,

    // Layout Management
    Split(SplitDirection),
    ClosePane,
    MinimizePane(Option<PaneId>),
    RestorePane(PaneId),
    ToggleZoom,
    RenamePane(Option<PaneId>),
    Resize(Direction, u16),
    NewWorkspace,
    CloseWorkspace(WorkspaceId),
    CloseActiveWorkspace,
    UndoLayout,

    // Overlays & UI Panels
    ToggleTabTray,
    ToggleCommandPalette,
    ToggleHelp,
    ToggleWhichKey,
    ToggleAttentionQueue,
    ToggleUsageOverlay,
    ToggleGitOverlay,
    ToggleSnippetOverlay,
    ToggleVaultOverlay,
    ToggleQuickSwitcher,

    // Modes & Toggles
    TogglePassthrough,
    ToggleDnd,
    /// Send the leader key itself to the focused pane (Leader, Leader)
    SendLeaderKey,

    // Agents & Creation
    SpawnShell,
    SpawnAgent {
        profile: String,
        worktree: Option<PathBuf>,
    },
    AttachTmux {
        session: String,
    },
    SpawnApplet {
        name: String,
    },
    RestartPane(PaneId),

    // Clipboard & Actions
    PasteFromClipboard,
    CopySelection,
    PasteImageBridge,

    // System
    Quit,
    Detach,
    ToggleBackgroundMode,
    ReloadConfig,
    StartTour,
    Redraw,
    CheckUpdate,
    Update,
}
