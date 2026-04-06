use serde::{Deserialize, Serialize};

use crate::defaults::{
    default_terminal_font_family, default_terminal_font_size_px, default_terminal_line_height_px,
    default_theme, default_true,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppearanceConfig {
    #[serde(default = "default_theme")]
    pub theme: String,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: default_theme(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowConfig {
    #[serde(default = "default_true")]
    pub keep_running_without_windows: bool,
    #[serde(default = "default_true")]
    pub hide_on_last_window_close: bool,
    #[serde(default = "default_true")]
    pub keep_sessions_alive_without_windows: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            keep_running_without_windows: true,
            hide_on_last_window_close: true,
            keep_sessions_alive_without_windows: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerminalConfig {
    #[serde(default)]
    pub local_shell: Option<String>,
    #[serde(default = "default_terminal_font_family")]
    pub font_family: String,
    #[serde(default = "default_terminal_font_size_px")]
    pub font_size_px: f32,
    #[serde(default = "default_terminal_line_height_px")]
    pub line_height_px: f32,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            local_shell: None,
            font_family: default_terminal_font_family(),
            font_size_px: default_terminal_font_size_px(),
            line_height_px: default_terminal_line_height_px(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PerfHudDefault {
    #[default]
    Off,
    Compact,
    Expanded,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateInstallMode {
    #[default]
    Prompted,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateReleaseChannel {
    #[default]
    Stable,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateConfig {
    #[serde(default = "default_true")]
    pub auto_check: bool,
    #[serde(default)]
    pub install_mode: UpdateInstallMode,
    #[serde(default)]
    pub channel: UpdateReleaseChannel,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            auto_check: true,
            install_mode: UpdateInstallMode::Prompted,
            channel: UpdateReleaseChannel::Stable,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DebugConfig {
    #[serde(default)]
    pub perf_hud_default: PerfHudDefault,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeybindingOverride {
    pub chord: String,
    pub action: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct KeybindingsConfig {
    #[serde(default)]
    pub overrides: Vec<KeybindingOverride>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub window: WindowConfig,
    #[serde(default)]
    pub terminal: TerminalConfig,
    #[serde(default)]
    pub updates: UpdateConfig,
    #[serde(default)]
    pub debug: DebugConfig,
    #[serde(default)]
    pub keybindings: KeybindingsConfig,
}

impl AppConfig {
    pub(crate) fn normalized(&self) -> Self {
        let mut normalized = self.clone();
        normalized.appearance.theme = normalized.appearance.theme.trim().to_string();
        normalized.terminal.font_family = normalized.terminal.font_family.trim().to_string();
        normalized.terminal.local_shell = normalized
            .terminal
            .local_shell
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        for binding in &mut normalized.keybindings.overrides {
            binding.chord = binding.chord.trim().to_string();
            binding.action = binding.action.trim().to_string();
        }
        normalized
    }
}
