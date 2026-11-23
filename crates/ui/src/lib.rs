pub mod card_strip;
pub mod empty_state;
pub mod first_run_tour;
pub mod help_overlay;
pub mod palette;
pub mod peek;
pub mod project_switcher;
pub mod rename_modal;
pub mod status_bar;
pub mod tab_tray;
pub mod toast;
pub mod which_key;
pub mod workers_table;

pub use card_strip::{
    card_at, card_text, format_elapsed, render_card_strip, status_glyph, CardItem,
};
pub use empty_state::render_empty_state;
pub use first_run_tour::{render_tour, TourState, TOUR_STEPS};
pub use help_overlay::render_help_overlay;
pub use palette::{
    get_matched_commands, get_selected_command, hit_test_palette, render_command_palette,
    PaletteClick, PaletteState,
};
pub use peek::{peek_area, peek_title, render_peek_frame};
pub use project_switcher::{
    preview_area, render_project_switcher, switcher_rows, ProjectSwitcherState, SwitcherPane,
    SwitcherProject, SwitcherRow, SwitcherTarget, SwitcherWorkspace,
};
pub use rename_modal::{render_prompt_modal, render_rename_modal, RenameModalState};
pub use status_bar::{
    hit_test_status_bar, render_status_bar, StatusBarClickTarget, StatusBarProps,
};
pub use tab_tray::{filter_tray_items, render_tab_tray, tray_item_at_row, TabTrayState, TrayItem};
pub use toast::{render_toasts, Toast, ToastLevel};
pub use which_key::render_which_key;
pub use workers_table::{
    inside_workers, render_workers_table, visible_rows, worker_row_at, WorkerRow, WorkersFilter,
    WorkersTableState,
};
