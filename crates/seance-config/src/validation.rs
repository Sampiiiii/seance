use std::collections::HashSet;

use thiserror::Error;

use crate::{AppConfig, SUPPORTED_KEYBINDING_ACTIONS, SUPPORTED_THEME_KEYS};

const FONT_SIZE_RANGE: std::ops::RangeInclusive<f32> = 8.0..=32.0;
const LINE_HEIGHT_RANGE: std::ops::RangeInclusive<f32> = 10.0..=40.0;

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
}

#[cfg(test)]
mod tests {
    use crate::{AppConfig, ConfigError, KeybindingOverride, SUPPORTED_KEYBINDING_ACTIONS};

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
}