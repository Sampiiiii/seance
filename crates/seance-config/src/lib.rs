use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;

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

const FONT_SIZE_RANGE: std::ops::RangeInclusive<f32> = 8.0..=32.0;
const LINE_HEIGHT_RANGE: std::ops::RangeInclusive<f32> = 10.0..=40.0;

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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PerfHudDefault {
    Off,
    Compact,
    Expanded,
}

impl Default for PerfHudDefault {
    fn default() -> Self {
        Self::Off
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
    pub debug: DebugConfig,
    #[serde(default)]
    pub keybindings: KeybindingsConfig,
}

impl AppConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !SUPPORTED_THEME_KEYS.contains(&self.appearance.theme.as_str()) {
            return Err(ConfigError::InvalidTheme {
                theme: self.appearance.theme.clone(),
            });
        }

        let font_family = self.terminal.font_family.trim();
        if font_family.is_empty() {
            return Err(ConfigError::EmptyField {
                field: "terminal.font_family",
            });
        }

        if !FONT_SIZE_RANGE.contains(&self.terminal.font_size_px) {
            return Err(ConfigError::OutOfRange {
                field: "terminal.font_size_px",
                value: self.terminal.font_size_px,
                min: *FONT_SIZE_RANGE.start(),
                max: *FONT_SIZE_RANGE.end(),
            });
        }

        if !LINE_HEIGHT_RANGE.contains(&self.terminal.line_height_px) {
            return Err(ConfigError::OutOfRange {
                field: "terminal.line_height_px",
                value: self.terminal.line_height_px,
                min: *LINE_HEIGHT_RANGE.start(),
                max: *LINE_HEIGHT_RANGE.end(),
            });
        }

        if let Some(local_shell) = self.terminal.local_shell.as_deref()
            && local_shell.trim().is_empty()
        {
            return Err(ConfigError::EmptyField {
                field: "terminal.local_shell",
            });
        }

        let mut seen = HashSet::new();
        for binding in &self.keybindings.overrides {
            let chord = binding.chord.trim();
            let action = binding.action.trim();
            if chord.is_empty() {
                return Err(ConfigError::EmptyField {
                    field: "keybindings.overrides.chord",
                });
            }
            if action.is_empty() {
                return Err(ConfigError::EmptyField {
                    field: "keybindings.overrides.action",
                });
            }
            if !SUPPORTED_KEYBINDING_ACTIONS.contains(&action) {
                return Err(ConfigError::UnsupportedKeybindingAction {
                    action: action.to_string(),
                });
            }
            if !seen.insert((chord.to_string(), action.to_string())) {
                return Err(ConfigError::DuplicateKeybindingOverride {
                    chord: chord.to_string(),
                    action: action.to_string(),
                });
            }
        }

        Ok(())
    }

    fn normalized(&self) -> Self {
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

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file")]
    Read {
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write config file")]
    Write {
        #[source]
        source: std::io::Error,
    },
    #[error("failed to serialize config")]
    Serialize {
        #[source]
        source: toml::ser::Error,
    },
    #[error("failed to parse config")]
    Parse {
        #[source]
        source: toml::de::Error,
    },
    #[error("unsupported theme key '{theme}'")]
    InvalidTheme { theme: String },
    #[error("{field} must not be empty")]
    EmptyField { field: &'static str },
    #[error("{field} must be between {min} and {max}, got {value}")]
    OutOfRange {
        field: &'static str,
        value: f32,
        min: f32,
        max: f32,
    },
    #[error("unsupported keybinding action '{action}'")]
    UnsupportedKeybindingAction { action: String },
    #[error("duplicate keybinding override for chord '{chord}' and action '{action}'")]
    DuplicateKeybindingOverride { chord: String, action: String },
}

pub struct ConfigStore {
    path: PathBuf,
    config: AppConfig,
}

impl ConfigStore {
    pub fn with_defaults(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            config: AppConfig::default(),
        }
    }

    pub fn load_or_default(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref().to_path_buf();
        if !path.exists() {
            return Ok(Self {
                path,
                config: AppConfig::default(),
            });
        }

        let contents = fs::read_to_string(&path).map_err(|source| ConfigError::Read { source })?;
        let config: AppConfig =
            toml::from_str(&contents).map_err(|source| ConfigError::Parse { source })?;
        let config = config.normalized();
        config.validate()?;
        Ok(Self { path, config })
    }

    pub fn snapshot(&self) -> AppConfig {
        self.config.clone()
    }

    pub fn replace(&mut self, config: AppConfig) -> Result<(), ConfigError> {
        let config = config.normalized();
        config.validate()?;
        persist_config(&self.path, &config)?;
        self.config = config;
        Ok(())
    }

    pub fn update(&mut self, f: impl FnOnce(&mut AppConfig)) -> Result<AppConfig, ConfigError> {
        let mut next = self.config.clone();
        f(&mut next);
        self.replace(next)?;
        Ok(self.snapshot())
    }
}

fn persist_config(path: &Path, config: &AppConfig) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Write { source })?;
    }

    let serialized =
        toml::to_string_pretty(config).map_err(|source| ConfigError::Serialize { source })?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temp = NamedTempFile::new_in(parent).map_err(|source| ConfigError::Write { source })?;
    temp.write_all(serialized.as_bytes())
        .map_err(|source| ConfigError::Write { source })?;
    temp.flush()
        .map_err(|source| ConfigError::Write { source })?;
    temp.persist(path).map_err(|error| ConfigError::Write {
        source: error.error,
    })?;
    Ok(())
}

fn default_true() -> bool {
    true
}

fn default_theme() -> String {
    DEFAULT_THEME_KEY.to_string()
}

fn default_terminal_font_family() -> String {
    "Menlo".to_string()
}

fn default_terminal_font_size_px() -> f32 {
    13.0
}

fn default_terminal_line_height_px() -> f32 {
    19.0
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        AppConfig, ConfigError, ConfigStore, DEFAULT_THEME_KEY, KeybindingOverride,
        SUPPORTED_KEYBINDING_ACTIONS,
    };

    #[test]
    fn missing_file_loads_defaults() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let store = ConfigStore::load_or_default(&path).unwrap();

        assert_eq!(store.snapshot(), AppConfig::default());
        assert!(!path.exists());
    }

    #[test]
    fn valid_toml_round_trips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut store = ConfigStore::with_defaults(&path);

        let saved = store
            .update(|config| {
                config.appearance.theme = "bone".into();
                config.terminal.font_family = "JetBrains Mono".into();
                config.debug.perf_hud_default = super::PerfHudDefault::Expanded;
            })
            .unwrap();

        let reloaded = ConfigStore::load_or_default(&path).unwrap();
        assert_eq!(reloaded.snapshot(), saved);
    }

    #[test]
    fn invalid_theme_is_rejected() {
        let mut config = AppConfig::default();
        config.appearance.theme = "unknown-theme".into();

        let err = config.validate().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidTheme { .. }));
    }

    #[test]
    fn empty_font_family_is_rejected() {
        let mut config = AppConfig::default();
        config.terminal.font_family = "   ".into();

        let err = config.validate().unwrap_err();
        assert!(matches!(
            err,
            ConfigError::EmptyField {
                field: "terminal.font_family"
            }
        ));
    }

    #[test]
    fn font_metrics_bounds_are_enforced() {
        let mut config = AppConfig::default();
        config.terminal.font_size_px = 7.0;
        let font_size_err = config.validate().unwrap_err();
        assert!(matches!(
            font_size_err,
            ConfigError::OutOfRange {
                field: "terminal.font_size_px",
                ..
            }
        ));

        config.terminal.font_size_px = 13.0;
        config.terminal.line_height_px = 41.0;
        let line_height_err = config.validate().unwrap_err();
        assert!(matches!(
            line_height_err,
            ConfigError::OutOfRange {
                field: "terminal.line_height_px",
                ..
            }
        ));
    }

    #[test]
    fn atomic_save_replaces_previous_contents_cleanly() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "junk\n").unwrap();

        let mut store = ConfigStore::with_defaults(&path);
        store
            .update(|config| {
                config.appearance.theme = "bone".into();
            })
            .unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("theme = \"bone\""));
        assert!(!contents.contains("junk"));
    }

    #[test]
    fn duplicate_and_invalid_keybinding_overrides_are_rejected() {
        let mut config = AppConfig::default();
        let action = SUPPORTED_KEYBINDING_ACTIONS[0].to_string();
        config.keybindings.overrides = vec![
            KeybindingOverride {
                chord: "cmd-k".into(),
                action: action.clone(),
            },
            KeybindingOverride {
                chord: "cmd-k".into(),
                action,
            },
        ];
        let duplicate_err = config.validate().unwrap_err();
        assert!(matches!(
            duplicate_err,
            ConfigError::DuplicateKeybindingOverride { .. }
        ));

        config.keybindings.overrides = vec![KeybindingOverride {
            chord: "cmd-k".into(),
            action: "seance_ui_app::Unsupported".into(),
        }];
        let invalid_err = config.validate().unwrap_err();
        assert!(matches!(
            invalid_err,
            ConfigError::UnsupportedKeybindingAction { .. }
        ));
    }

    #[test]
    fn update_normalizes_trimmed_values() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut store = ConfigStore::with_defaults(&path);

        let config = store
            .update(|config| {
                config.appearance.theme = format!(" {DEFAULT_THEME_KEY} ");
                config.terminal.font_family = " Menlo ".into();
                config.terminal.local_shell = Some(" /bin/zsh ".into());
            })
            .unwrap();

        assert_eq!(config.appearance.theme, DEFAULT_THEME_KEY);
        assert_eq!(config.terminal.font_family, "Menlo");
        assert_eq!(config.terminal.local_shell.as_deref(), Some("/bin/zsh"));
    }
}
