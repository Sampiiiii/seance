use std::collections::HashSet;

use thiserror::Error;

use crate::{
    AppConfig, KeybindingOverride, SUPPORTED_THEME_KEYS, is_builtin_keybinding_id,
    is_supported_keybinding_command, validate_chord_syntax,
};

const FONT_SIZE_RANGE: std::ops::RangeInclusive<f32> = 8.0..=32.0;
const LINE_HEIGHT_RANGE: std::ops::RangeInclusive<f32> = 10.0..=40.0;
const TRANSCRIPT_RETENTION_DAYS_RANGE: std::ops::RangeInclusive<u16> = 1..=365;
const TRANSCRIPT_MAX_BYTES_RANGE: std::ops::RangeInclusive<u64> = 1024..=(1024 * 1024 * 1024);

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
    #[error("{field} must be between {min} and {max}, got {value}")]
    OutOfRangeInt {
        field: &'static str,
        value: u64,
        min: u64,
        max: u64,
    },
    #[error("unknown keybinding id '{id}'")]
    UnknownKeybindingId { id: String },
    #[error("unknown keybinding command '{command}'")]
    UnknownKeybindingCommand { command: String },
    #[error("invalid keybinding chord '{chord}'")]
    InvalidKeybindingChord { chord: String },
    #[error("invalid keybinding override '{id}'")]
    InvalidKeybindingOverride { id: String },
    #[error("duplicate keybinding override id '{id}'")]
    DuplicateKeybindingOverrideId { id: String },
    #[error(
        "duplicate custom keybinding for chord '{chord}', command '{command}', and context '{context}'"
    )]
    DuplicateCustomKeybinding {
        chord: String,
        command: String,
        context: String,
    },
    #[error("duplicate vault id '{vault_id}'")]
    DuplicateVaultId { vault_id: String },
    #[error("duplicate vault name '{name}'")]
    DuplicateVaultName { name: String },
    #[error("duplicate vault db_file '{db_file}'")]
    DuplicateVaultDbFile { db_file: String },
    #[error("open_vault_ids references unknown vault id '{vault_id}'")]
    UnknownOpenVaultId { vault_id: String },
    #[error("default_target_vault_id references unknown vault id '{vault_id}'")]
    UnknownDefaultVaultId { vault_id: String },
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

        if !TRANSCRIPT_RETENTION_DAYS_RANGE
            .contains(&self.logging.session_transcript_retention_days)
        {
            return Err(ConfigError::OutOfRangeInt {
                field: "logging.session_transcript_retention_days",
                value: u64::from(self.logging.session_transcript_retention_days),
                min: u64::from(*TRANSCRIPT_RETENTION_DAYS_RANGE.start()),
                max: u64::from(*TRANSCRIPT_RETENTION_DAYS_RANGE.end()),
            });
        }

        if !TRANSCRIPT_MAX_BYTES_RANGE
            .contains(&self.logging.session_transcript_max_bytes_per_session)
        {
            return Err(ConfigError::OutOfRangeInt {
                field: "logging.session_transcript_max_bytes_per_session",
                value: self.logging.session_transcript_max_bytes_per_session,
                min: *TRANSCRIPT_MAX_BYTES_RANGE.start(),
                max: *TRANSCRIPT_MAX_BYTES_RANGE.end(),
            });
        }

        let mut seen_override_ids = HashSet::new();
        for binding in &self.keybindings.overrides {
            let id = binding.id.trim();
            if id.is_empty() {
                return Err(ConfigError::EmptyField {
                    field: "keybindings.overrides.id",
                });
            }
            if !is_builtin_keybinding_id(id) {
                return Err(ConfigError::UnknownKeybindingId { id: id.to_string() });
            }
            validate_override(binding)?;
            if !seen_override_ids.insert(id.to_string()) {
                return Err(ConfigError::DuplicateKeybindingOverrideId { id: id.to_string() });
            }
        }

        let mut seen_custom = HashSet::new();
        for binding in &self.keybindings.custom {
            let chord = binding.chord.trim();
            let command = binding.command.trim();
            if chord.is_empty() {
                return Err(ConfigError::EmptyField {
                    field: "keybindings.custom.chord",
                });
            }
            if command.is_empty() {
                return Err(ConfigError::EmptyField {
                    field: "keybindings.custom.command",
                });
            }
            if !validate_chord_syntax(chord) {
                return Err(ConfigError::InvalidKeybindingChord {
                    chord: chord.to_string(),
                });
            }
            if !is_supported_keybinding_command(command) {
                return Err(ConfigError::UnknownKeybindingCommand {
                    command: command.to_string(),
                });
            }
            let key = (
                chord.to_string(),
                command.to_string(),
                format!("{:?}", binding.context),
            );
            if !seen_custom.insert(key.clone()) {
                return Err(ConfigError::DuplicateCustomKeybinding {
                    chord: key.0,
                    command: key.1,
                    context: key.2,
                });
            }
        }

        let mut seen_vault_ids = HashSet::new();
        let mut seen_vault_names = HashSet::new();
        let mut seen_db_files = HashSet::new();
        let mut known_vault_ids = HashSet::new();
        for entry in &self.vaults.entries {
            if entry.id.is_empty() {
                return Err(ConfigError::EmptyField {
                    field: "vaults.entries.id",
                });
            }
            if entry.name.is_empty() {
                return Err(ConfigError::EmptyField {
                    field: "vaults.entries.name",
                });
            }
            if entry.db_file.is_empty() {
                return Err(ConfigError::EmptyField {
                    field: "vaults.entries.db_file",
                });
            }
            if !seen_vault_ids.insert(entry.id.clone()) {
                return Err(ConfigError::DuplicateVaultId {
                    vault_id: entry.id.clone(),
                });
            }
            let normalized_name = entry.name.to_lowercase();
            if !seen_vault_names.insert(normalized_name) {
                return Err(ConfigError::DuplicateVaultName {
                    name: entry.name.clone(),
                });
            }
            if !seen_db_files.insert(entry.db_file.clone()) {
                return Err(ConfigError::DuplicateVaultDbFile {
                    db_file: entry.db_file.clone(),
                });
            }
            known_vault_ids.insert(entry.id.clone());
        }

        for vault_id in &self.vaults.open_vault_ids {
            if !known_vault_ids.contains(vault_id) {
                return Err(ConfigError::UnknownOpenVaultId {
                    vault_id: vault_id.clone(),
                });
            }
        }

        if let Some(vault_id) = self.vaults.default_target_vault_id.as_ref()
            && !known_vault_ids.contains(vault_id)
        {
            return Err(ConfigError::UnknownDefaultVaultId {
                vault_id: vault_id.clone(),
            });
        }

        Ok(())
    }
}

fn validate_override(binding: &KeybindingOverride) -> Result<(), ConfigError> {
    match (binding.disabled, binding.chord.as_deref()) {
        (true, Some(_)) => Err(ConfigError::InvalidKeybindingOverride {
            id: binding.id.clone(),
        }),
        (true, None) => Ok(()),
        (false, None) => Err(ConfigError::InvalidKeybindingOverride {
            id: binding.id.clone(),
        }),
        (false, Some(chord)) => {
            if !validate_chord_syntax(chord) {
                Err(ConfigError::InvalidKeybindingChord {
                    chord: chord.to_string(),
                })
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        AppConfig, COMMAND_APP_OPEN_COMMAND_PALETTE, COMMAND_APP_OPEN_PREFERENCES, ConfigError,
        CustomKeybinding, KeybindingContext, KeybindingOverride, UpdateInstallMode,
        UpdateReleaseChannel, VaultRegistryEntry,
    };

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
    fn transcript_logging_bounds_are_enforced() {
        let mut config = AppConfig::default();
        config.logging.session_transcript_retention_days = 0;
        let retention_err = config.validate().unwrap_err();
        assert!(matches!(
            retention_err,
            ConfigError::OutOfRangeInt {
                field: "logging.session_transcript_retention_days",
                ..
            }
        ));

        config.logging.session_transcript_retention_days = 7;
        config.logging.session_transcript_max_bytes_per_session = 0;
        let size_err = config.validate().unwrap_err();
        assert!(matches!(
            size_err,
            ConfigError::OutOfRangeInt {
                field: "logging.session_transcript_max_bytes_per_session",
                ..
            }
        ));
    }

    #[test]
    fn invalid_keybinding_overrides_and_custom_entries_are_rejected() {
        let mut config = AppConfig::default();
        config.keybindings.overrides = vec![
            KeybindingOverride {
                id: COMMAND_APP_OPEN_PREFERENCES.into(),
                chord: Some("primary-,".into()),
                disabled: false,
            },
            KeybindingOverride {
                id: COMMAND_APP_OPEN_PREFERENCES.into(),
                chord: Some("cmd-,".into()),
                disabled: false,
            },
        ];
        let duplicate_err = config.validate().unwrap_err();
        assert!(matches!(
            duplicate_err,
            ConfigError::DuplicateKeybindingOverrideId { .. }
        ));

        config.keybindings.overrides = vec![KeybindingOverride {
            id: "unknown.binding".into(),
            chord: Some("cmd-k".into()),
            disabled: false,
        }];
        let invalid_err = config.validate().unwrap_err();
        assert!(matches!(
            invalid_err,
            ConfigError::UnknownKeybindingId { .. }
        ));

        config.keybindings.overrides = vec![KeybindingOverride {
            id: COMMAND_APP_OPEN_COMMAND_PALETTE.into(),
            chord: None,
            disabled: false,
        }];
        let state_err = config.validate().unwrap_err();
        assert!(matches!(
            state_err,
            ConfigError::InvalidKeybindingOverride { .. }
        ));

        config.keybindings.overrides.clear();
        config.keybindings.custom = vec![
            CustomKeybinding {
                chord: "primary-k".into(),
                command: COMMAND_APP_OPEN_COMMAND_PALETTE.into(),
                context: KeybindingContext::AppGlobal,
            },
            CustomKeybinding {
                chord: "primary-k".into(),
                command: COMMAND_APP_OPEN_COMMAND_PALETTE.into(),
                context: KeybindingContext::AppGlobal,
            },
        ];
        let custom_duplicate_err = config.validate().unwrap_err();
        assert!(matches!(
            custom_duplicate_err,
            ConfigError::DuplicateCustomKeybinding { .. }
        ));
    }

    #[test]
    fn update_defaults_use_prompted_stable_channel() {
        let config = AppConfig::default();
        assert!(config.updates.auto_check);
        assert_eq!(config.updates.install_mode, UpdateInstallMode::Prompted);
        assert_eq!(config.updates.channel, UpdateReleaseChannel::Stable);
    }

    #[test]
    fn duplicate_vault_name_and_unknown_refs_are_rejected() {
        let mut config = AppConfig::default();
        config.vaults.entries = vec![
            VaultRegistryEntry {
                id: "vault-a".into(),
                name: "Personal".into(),
                db_file: "vault-a.sqlite".into(),
                created_at: 1,
                updated_at: 1,
            },
            VaultRegistryEntry {
                id: "vault-b".into(),
                name: "personal".into(),
                db_file: "vault-b.sqlite".into(),
                created_at: 1,
                updated_at: 1,
            },
        ];
        let duplicate_err = config.validate().unwrap_err();
        assert!(matches!(
            duplicate_err,
            ConfigError::DuplicateVaultName { .. }
        ));

        config.vaults.entries.pop();
        config.vaults.open_vault_ids = vec!["missing".into()];
        let open_ref_err = config.validate().unwrap_err();
        assert!(matches!(
            open_ref_err,
            ConfigError::UnknownOpenVaultId { .. }
        ));

        config.vaults.open_vault_ids.clear();
        config.vaults.default_target_vault_id = Some("missing".into());
        let default_ref_err = config.validate().unwrap_err();
        assert!(matches!(
            default_ref_err,
            ConfigError::UnknownDefaultVaultId { .. }
        ));
    }
}
