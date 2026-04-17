use serde::{Deserialize, Serialize};

use crate::defaults::{
    default_logging_max_bytes_per_session, default_logging_retention_days,
    default_terminal_font_family, default_terminal_font_size_px, default_terminal_line_height_px,
    default_theme, default_true,
};
use crate::keybindings::KeybindingsConfig;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultRegistryEntry {
    pub id: String,
    pub name: String,
    pub db_file: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VaultRegistryConfig {
    #[serde(default)]
    pub entries: Vec<VaultRegistryEntry>,
    #[serde(default)]
    pub open_vault_ids: Vec<String>,
    #[serde(default)]
    pub default_target_vault_id: Option<String>,
}

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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoggingConfig {
    #[serde(default)]
    pub session_transcript_enabled: bool,
    #[serde(default = "default_logging_retention_days")]
    pub session_transcript_retention_days: u16,
    #[serde(default = "default_logging_max_bytes_per_session")]
    pub session_transcript_max_bytes_per_session: u64,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            session_transcript_enabled: false,
            session_transcript_retention_days: default_logging_retention_days(),
            session_transcript_max_bytes_per_session: default_logging_max_bytes_per_session(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DebugConfig {
    #[serde(default)]
    pub perf_hud_default: PerfHudDefault,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub vaults: VaultRegistryConfig,
    #[serde(default)]
    pub window: WindowConfig,
    #[serde(default)]
    pub terminal: TerminalConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
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
        for entry in &mut normalized.vaults.entries {
            entry.id = entry.id.trim().to_string();
            entry.name = entry.name.trim().to_string();
            entry.db_file = entry.db_file.trim().to_string();
        }
        normalized.vaults.open_vault_ids = normalized
            .vaults
            .open_vault_ids
            .into_iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect();
        normalized.vaults.default_target_vault_id = normalized
            .vaults
            .default_target_vault_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        normalized.terminal.font_family = normalized.terminal.font_family.trim().to_string();
        normalized.terminal.local_shell = normalized
            .terminal
            .local_shell
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        for binding in &mut normalized.keybindings.overrides {
            binding.id = binding.id.trim().to_string();
            binding.chord = binding
                .chord
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
        }
        for binding in &mut normalized.keybindings.custom {
            binding.chord = binding.chord.trim().to_string();
            binding.command = binding.command.trim().to_string();
        }
        normalized
    }
}
