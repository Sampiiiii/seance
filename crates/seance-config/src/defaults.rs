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

pub const SUPPORTED_KEYBINDING_ACTIONS: &[&str] = &[
    "seance_ui_app::NewTerminal",
    "seance_ui_app::OpenCommandPalette",
    "seance_ui_app::OpenPreferences",
    "seance_ui_app::CloseActiveSession",
    "seance_ui_app::OpenNewWindow",
    "seance_ui_app::TogglePerfHud",
    "seance_ui_app::QuitSeance",
    "seance_ui_app::HideSeance",
    "seance_ui_app::HideOtherApps",
    "seance_ui_app::ShowAllApps",
    "seance_ui::SwitchTheme",
    "seance_ui::ConnectHost",
    "seance_ui::SelectSession",
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