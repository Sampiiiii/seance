use serde::{Deserialize, Deserializer, Serialize};

pub const COMMAND_APP_OPEN_PREFERENCES: &str = "app.open_preferences";
pub const COMMAND_APP_OPEN_COMMAND_PALETTE: &str = "app.open_command_palette";
pub const COMMAND_SESSION_NEW_LOCAL: &str = "session.new_local";
pub const COMMAND_SESSION_CLOSE_ACTIVE: &str = "session.close_active";
pub const COMMAND_DEBUG_TOGGLE_PERF_HUD: &str = "debug.toggle_perf_hud";
pub const COMMAND_WINDOW_NEW: &str = "window.new";
pub const COMMAND_APP_QUIT: &str = "app.quit";
pub const COMMAND_APP_HIDE: &str = "app.hide";
pub const COMMAND_SESSION_SELECT_PREVIOUS: &str = "session.select_previous";
pub const COMMAND_SESSION_SELECT_NEXT: &str = "session.select_next";
pub const COMMAND_SESSION_SELECT_SLOT_1: &str = "session.select_slot_1";
pub const COMMAND_SESSION_SELECT_SLOT_2: &str = "session.select_slot_2";
pub const COMMAND_SESSION_SELECT_SLOT_3: &str = "session.select_slot_3";
pub const COMMAND_SESSION_SELECT_SLOT_4: &str = "session.select_slot_4";
pub const COMMAND_SESSION_SELECT_SLOT_5: &str = "session.select_slot_5";
pub const COMMAND_SESSION_SELECT_SLOT_6: &str = "session.select_slot_6";
pub const COMMAND_SESSION_SELECT_SLOT_7: &str = "session.select_slot_7";
pub const COMMAND_SESSION_SELECT_SLOT_8: &str = "session.select_slot_8";
pub const COMMAND_SESSION_SELECT_SLOT_9: &str = "session.select_slot_9";
pub const COMMAND_SESSION_SELECT_SLOT_10: &str = "session.select_slot_10";

pub const BUILTIN_KEYBINDING_IDS: &[&str] = &[
    COMMAND_APP_OPEN_PREFERENCES,
    COMMAND_APP_OPEN_COMMAND_PALETTE,
    COMMAND_SESSION_NEW_LOCAL,
    COMMAND_SESSION_CLOSE_ACTIVE,
    COMMAND_DEBUG_TOGGLE_PERF_HUD,
    COMMAND_WINDOW_NEW,
    COMMAND_APP_QUIT,
    COMMAND_APP_HIDE,
    COMMAND_SESSION_SELECT_PREVIOUS,
    COMMAND_SESSION_SELECT_NEXT,
    COMMAND_SESSION_SELECT_SLOT_1,
    COMMAND_SESSION_SELECT_SLOT_2,
    COMMAND_SESSION_SELECT_SLOT_3,
    COMMAND_SESSION_SELECT_SLOT_4,
    COMMAND_SESSION_SELECT_SLOT_5,
    COMMAND_SESSION_SELECT_SLOT_6,
    COMMAND_SESSION_SELECT_SLOT_7,
    COMMAND_SESSION_SELECT_SLOT_8,
    COMMAND_SESSION_SELECT_SLOT_9,
    COMMAND_SESSION_SELECT_SLOT_10,
];

pub const SUPPORTED_KEYBINDING_COMMANDS: &[&str] = BUILTIN_KEYBINDING_IDS;

const LEGACY_ACTION_APP_OPEN_PREFERENCES: &str = "seance_ui_app::OpenPreferences";
const LEGACY_ACTION_APP_OPEN_COMMAND_PALETTE: &str = "seance_ui_app::OpenCommandPalette";
const LEGACY_ACTION_SESSION_NEW_LOCAL: &str = "seance_ui_app::NewTerminal";
const LEGACY_ACTION_SESSION_CLOSE_ACTIVE: &str = "seance_ui_app::CloseActiveSession";
const LEGACY_ACTION_DEBUG_TOGGLE_PERF_HUD: &str = "seance_ui_app::TogglePerfHud";
const LEGACY_ACTION_WINDOW_NEW: &str = "seance_ui_app::OpenNewWindow";
const LEGACY_ACTION_APP_QUIT: &str = "seance_ui_app::QuitSeance";
const LEGACY_ACTION_APP_HIDE: &str = "seance_ui_app::HideSeance";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum KeybindingContext {
    #[default]
    AppGlobal,
    WorkspaceTerminal,
    WorkspaceSettings,
    WorkspaceSecure,
    WorkspaceSftp,
    Palette,
    VaultModal,
    ConfirmDialog,
}

impl KeybindingContext {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::AppGlobal => "App Global",
            Self::WorkspaceTerminal => "Terminal Workspace",
            Self::WorkspaceSettings => "Settings",
            Self::WorkspaceSecure => "Secure Workspace",
            Self::WorkspaceSftp => "SFTP",
            Self::Palette => "Command Palette",
            Self::VaultModal => "Vault Modal",
            Self::ConfirmDialog => "Confirm Dialog",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeybindingOverride {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chord: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomKeybinding {
    pub chord: String,
    pub command: String,
    #[serde(default)]
    pub context: KeybindingContext,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq, Default)]
pub struct KeybindingsConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<KeybindingOverride>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom: Vec<CustomKeybinding>,
}

impl<'de> Deserialize<'de> for KeybindingsConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize, Default)]
        struct WireKeybindingsConfig {
            #[serde(default)]
            overrides: Vec<WireOverride>,
            #[serde(default)]
            custom: Vec<CustomKeybinding>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum WireOverride {
            Modern(KeybindingOverride),
            Legacy(LegacyKeybindingOverride),
        }

        #[derive(Deserialize)]
        struct LegacyKeybindingOverride {
            chord: String,
            action: String,
        }

        let wire = WireKeybindingsConfig::deserialize(deserializer)?;
        let mut config = KeybindingsConfig {
            overrides: Vec::new(),
            custom: wire.custom,
        };

        for override_entry in wire.overrides {
            match override_entry {
                WireOverride::Modern(binding) => config.overrides.push(binding),
                WireOverride::Legacy(binding) => {
                    let Some(command) = legacy_action_to_command(&binding.action) else {
                        return Err(serde::de::Error::custom(format!(
                            "unsupported legacy keybinding action '{}'",
                            binding.action
                        )));
                    };
                    config.custom.push(CustomKeybinding {
                        chord: binding.chord,
                        command: command.to_string(),
                        context: command_default_context(command),
                    });
                }
            }
        }

        Ok(config)
    }
}

pub fn is_builtin_keybinding_id(id: &str) -> bool {
    BUILTIN_KEYBINDING_IDS.contains(&id)
}

pub fn is_supported_keybinding_command(command: &str) -> bool {
    SUPPORTED_KEYBINDING_COMMANDS.contains(&command)
}

pub fn command_default_context(command: &str) -> KeybindingContext {
    match command {
        COMMAND_SESSION_SELECT_PREVIOUS
        | COMMAND_SESSION_SELECT_NEXT
        | COMMAND_SESSION_SELECT_SLOT_1
        | COMMAND_SESSION_SELECT_SLOT_2
        | COMMAND_SESSION_SELECT_SLOT_3
        | COMMAND_SESSION_SELECT_SLOT_4
        | COMMAND_SESSION_SELECT_SLOT_5
        | COMMAND_SESSION_SELECT_SLOT_6
        | COMMAND_SESSION_SELECT_SLOT_7
        | COMMAND_SESSION_SELECT_SLOT_8
        | COMMAND_SESSION_SELECT_SLOT_9
        | COMMAND_SESSION_SELECT_SLOT_10 => KeybindingContext::WorkspaceTerminal,
        _ => KeybindingContext::AppGlobal,
    }
}

pub fn legacy_action_to_command(action: &str) -> Option<&'static str> {
    match action {
        LEGACY_ACTION_APP_OPEN_PREFERENCES => Some(COMMAND_APP_OPEN_PREFERENCES),
        LEGACY_ACTION_APP_OPEN_COMMAND_PALETTE => Some(COMMAND_APP_OPEN_COMMAND_PALETTE),
        LEGACY_ACTION_SESSION_NEW_LOCAL => Some(COMMAND_SESSION_NEW_LOCAL),
        LEGACY_ACTION_SESSION_CLOSE_ACTIVE => Some(COMMAND_SESSION_CLOSE_ACTIVE),
        LEGACY_ACTION_DEBUG_TOGGLE_PERF_HUD => Some(COMMAND_DEBUG_TOGGLE_PERF_HUD),
        LEGACY_ACTION_WINDOW_NEW => Some(COMMAND_WINDOW_NEW),
        LEGACY_ACTION_APP_QUIT => Some(COMMAND_APP_QUIT),
        LEGACY_ACTION_APP_HIDE => Some(COMMAND_APP_HIDE),
        _ => None,
    }
}

pub fn normalize_chord_for_platform(chord: &str) -> String {
    chord
        .split_whitespace()
        .map(normalize_keystroke_for_platform)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn validate_chord_syntax(chord: &str) -> bool {
    let normalized = normalize_chord_for_platform(chord);
    let strokes = normalized.split_whitespace().collect::<Vec<_>>();
    if strokes.is_empty() {
        return false;
    }

    strokes.iter().all(|stroke| validate_keystroke(stroke))
}

fn normalize_keystroke_for_platform(keystroke: &str) -> String {
    keystroke
        .split('-')
        .map(|component| {
            if component.eq_ignore_ascii_case("primary") {
                primary_modifier_name().to_string()
            } else {
                component.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

fn primary_modifier_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "super"
    }
}

fn validate_keystroke(keystroke: &str) -> bool {
    let mut components = keystroke.split('-').peekable();
    let mut saw_key = false;

    while let Some(component) = components.next() {
        if component.is_empty() {
            return false;
        }

        if components.peek().is_some() {
            if !is_modifier_component(component) {
                return false;
            }
            continue;
        }

        saw_key = is_valid_key_component(component);
    }

    saw_key
}

fn is_modifier_component(component: &str) -> bool {
    matches!(
        component.to_ascii_lowercase().as_str(),
        "ctrl" | "alt" | "shift" | "fn" | "cmd" | "super" | "win" | "secondary"
    )
}

fn is_valid_key_component(component: &str) -> bool {
    !component.trim().is_empty()
}
