use crate::model::{
    MouseTrackingScrollPolicy, MouseTrackingSelectionPolicy, TerminalRightClickPolicy,
};

pub const DEFAULT_THEME_KEY: &str = "obsidian-smoke";
pub const SUPPORTED_THEME_KEYS: &[&str] = &[
    "obsidian-smoke",
    "midnight-frost",
    "bone",
    "phosphor",
    "tokyo-night",
    "catppuccin-mocha",
    "rose-pine",
    "dracula",
    "nord",
    "solarized-dark",
];

pub(crate) fn default_true() -> bool {
    true
}

pub(crate) fn default_theme() -> String {
    DEFAULT_THEME_KEY.to_string()
}

pub(crate) fn default_terminal_font_family() -> String {
    "Menlo".to_string()
}

pub(crate) fn default_terminal_font_size_px() -> f32 {
    13.0
}

pub(crate) fn default_terminal_line_height_px() -> f32 {
    19.0
}

pub(crate) fn default_mouse_tracking_scroll_policy() -> MouseTrackingScrollPolicy {
    MouseTrackingScrollPolicy::HybridShiftWheelLocal
}

pub(crate) fn default_mouse_tracking_selection_policy() -> MouseTrackingSelectionPolicy {
    MouseTrackingSelectionPolicy::ShiftDragLocal
}

pub(crate) fn default_terminal_right_click_policy() -> TerminalRightClickPolicy {
    TerminalRightClickPolicy::CopySelectionOrPaste
}

pub(crate) fn default_logging_retention_days() -> u16 {
    7
}

pub(crate) fn default_logging_max_bytes_per_session() -> u64 {
    64 * 1024 * 1024
}
