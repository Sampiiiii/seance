// Owns the built-in keybinding registry, app-level binding installation, and shortcut lookup.

use std::collections::HashMap;

use gpui::{Action, App, KeyBinding, KeyContext, Window};
use seance_config::{
    AppConfig, COMMAND_APP_HIDE, COMMAND_APP_OPEN_COMMAND_PALETTE, COMMAND_APP_OPEN_PREFERENCES,
    COMMAND_APP_QUIT, COMMAND_DEBUG_TOGGLE_PERF_HUD, COMMAND_SESSION_CLOSE_ACTIVE,
    COMMAND_SESSION_COPY_PREVIOUS_TURN, COMMAND_SESSION_NEW_LOCAL, COMMAND_SESSION_SELECT_NEXT,
    COMMAND_SESSION_SELECT_PREVIOUS, COMMAND_SESSION_SELECT_SLOT_1, COMMAND_SESSION_SELECT_SLOT_2,
    COMMAND_SESSION_SELECT_SLOT_3, COMMAND_SESSION_SELECT_SLOT_4, COMMAND_SESSION_SELECT_SLOT_5,
    COMMAND_SESSION_SELECT_SLOT_6, COMMAND_SESSION_SELECT_SLOT_7, COMMAND_SESSION_SELECT_SLOT_8,
    COMMAND_SESSION_SELECT_SLOT_9, COMMAND_SESSION_SELECT_SLOT_10, COMMAND_WINDOW_NEW,
    KeybindingContext, normalize_chord_for_platform,
};

use crate::{
    CloseActiveSession, CopyPreviousTurn, HideSeance, NewTerminal, OpenCommandPalette,
    OpenNewWindow, OpenPreferences, QuitSeance, SelectNextSession, SelectPreviousSession,
    SelectSessionSlot, TogglePerfHud,
};

const CONTEXT_WORKSPACE_TERMINAL: &str = "WorkspaceTerminal";
const CONTEXT_WORKSPACE_SETTINGS: &str = "WorkspaceSettings";
const CONTEXT_WORKSPACE_SECURE: &str = "WorkspaceSecure";
const CONTEXT_WORKSPACE_SFTP: &str = "WorkspaceSftp";
const CONTEXT_PALETTE: &str = "Palette";
const CONTEXT_VAULT_MODAL: &str = "VaultModal";
const CONTEXT_CONFIRM_DIALOG: &str = "ConfirmDialog";

#[derive(Clone, Copy)]
pub(crate) struct BuiltinKeybinding {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) command: &'static str,
    pub(crate) context: KeybindingContext,
    pub(crate) default_chord: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EffectiveKeybindingEntry {
    pub(crate) id: Option<String>,
    pub(crate) label: String,
    pub(crate) command: String,
    pub(crate) context: KeybindingContext,
    pub(crate) default_chord: Option<String>,
    pub(crate) default_display: Option<String>,
    pub(crate) effective_chord: Option<String>,
    pub(crate) effective_display: Option<String>,
    pub(crate) disabled: bool,
    pub(crate) customized: bool,
    pub(crate) is_custom: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeybindingConflict {
    pub(crate) chord: String,
    pub(crate) context: KeybindingContext,
    pub(crate) commands: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct EffectiveKeymap {
    pub(crate) bindings: Vec<KeyBinding>,
    pub(crate) entries: Vec<EffectiveKeybindingEntry>,
    pub(crate) conflicts: Vec<KeybindingConflict>,
}

const BUILTIN_KEYBINDINGS: &[BuiltinKeybinding] = &[
    BuiltinKeybinding {
        id: COMMAND_APP_OPEN_PREFERENCES,
        label: "Open Preferences",
        command: COMMAND_APP_OPEN_PREFERENCES,
        context: KeybindingContext::AppGlobal,
        default_chord: "primary-,",
    },
    BuiltinKeybinding {
        id: COMMAND_APP_OPEN_COMMAND_PALETTE,
        label: "Open Command Palette",
        command: COMMAND_APP_OPEN_COMMAND_PALETTE,
        context: KeybindingContext::AppGlobal,
        default_chord: "primary-k",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_NEW_LOCAL,
        label: "New Local Terminal",
        command: COMMAND_SESSION_NEW_LOCAL,
        context: KeybindingContext::AppGlobal,
        default_chord: "primary-t",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_CLOSE_ACTIVE,
        label: "Close Active Session",
        command: COMMAND_SESSION_CLOSE_ACTIVE,
        context: KeybindingContext::AppGlobal,
        default_chord: "primary-w",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_COPY_PREVIOUS_TURN,
        label: "Copy Previous Turn",
        command: COMMAND_SESSION_COPY_PREVIOUS_TURN,
        context: KeybindingContext::WorkspaceTerminal,
        default_chord: if cfg!(target_os = "macos") {
            "cmd-shift-c"
        } else {
            "ctrl-shift-alt-c"
        },
    },
    BuiltinKeybinding {
        id: COMMAND_DEBUG_TOGGLE_PERF_HUD,
        label: "Toggle Performance HUD",
        command: COMMAND_DEBUG_TOGGLE_PERF_HUD,
        context: KeybindingContext::AppGlobal,
        default_chord: "primary-shift-.",
    },
    BuiltinKeybinding {
        id: COMMAND_WINDOW_NEW,
        label: "Open New Window",
        command: COMMAND_WINDOW_NEW,
        context: KeybindingContext::AppGlobal,
        default_chord: "primary-n",
    },
    BuiltinKeybinding {
        id: COMMAND_APP_QUIT,
        label: "Quit Séance",
        command: COMMAND_APP_QUIT,
        context: KeybindingContext::AppGlobal,
        default_chord: "primary-q",
    },
    BuiltinKeybinding {
        id: COMMAND_APP_HIDE,
        label: "Hide Séance",
        command: COMMAND_APP_HIDE,
        context: KeybindingContext::AppGlobal,
        default_chord: "primary-h",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_SELECT_PREVIOUS,
        label: "Select Previous Session",
        command: COMMAND_SESSION_SELECT_PREVIOUS,
        context: KeybindingContext::WorkspaceTerminal,
        default_chord: "primary-left",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_SELECT_NEXT,
        label: "Select Next Session",
        command: COMMAND_SESSION_SELECT_NEXT,
        context: KeybindingContext::WorkspaceTerminal,
        default_chord: "primary-right",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_SELECT_SLOT_1,
        label: "Select Session Slot 1",
        command: COMMAND_SESSION_SELECT_SLOT_1,
        context: KeybindingContext::WorkspaceTerminal,
        default_chord: "primary-1",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_SELECT_SLOT_2,
        label: "Select Session Slot 2",
        command: COMMAND_SESSION_SELECT_SLOT_2,
        context: KeybindingContext::WorkspaceTerminal,
        default_chord: "primary-2",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_SELECT_SLOT_3,
        label: "Select Session Slot 3",
        command: COMMAND_SESSION_SELECT_SLOT_3,
        context: KeybindingContext::WorkspaceTerminal,
        default_chord: "primary-3",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_SELECT_SLOT_4,
        label: "Select Session Slot 4",
        command: COMMAND_SESSION_SELECT_SLOT_4,
        context: KeybindingContext::WorkspaceTerminal,
        default_chord: "primary-4",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_SELECT_SLOT_5,
        label: "Select Session Slot 5",
        command: COMMAND_SESSION_SELECT_SLOT_5,
        context: KeybindingContext::WorkspaceTerminal,
        default_chord: "primary-5",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_SELECT_SLOT_6,
        label: "Select Session Slot 6",
        command: COMMAND_SESSION_SELECT_SLOT_6,
        context: KeybindingContext::WorkspaceTerminal,
        default_chord: "primary-6",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_SELECT_SLOT_7,
        label: "Select Session Slot 7",
        command: COMMAND_SESSION_SELECT_SLOT_7,
        context: KeybindingContext::WorkspaceTerminal,
        default_chord: "primary-7",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_SELECT_SLOT_8,
        label: "Select Session Slot 8",
        command: COMMAND_SESSION_SELECT_SLOT_8,
        context: KeybindingContext::WorkspaceTerminal,
        default_chord: "primary-8",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_SELECT_SLOT_9,
        label: "Select Session Slot 9",
        command: COMMAND_SESSION_SELECT_SLOT_9,
        context: KeybindingContext::WorkspaceTerminal,
        default_chord: "primary-9",
    },
    BuiltinKeybinding {
        id: COMMAND_SESSION_SELECT_SLOT_10,
        label: "Select Session Slot 10",
        command: COMMAND_SESSION_SELECT_SLOT_10,
        context: KeybindingContext::WorkspaceTerminal,
        default_chord: "primary-0",
    },
];

pub(crate) fn install_app_keybindings(cx: &mut App, config: &AppConfig) -> EffectiveKeymap {
    let effective = resolve_effective_keymap(config);
    cx.bind_keys(effective.bindings.iter().cloned());
    effective
}

pub(crate) fn rebuild_app_keybindings(cx: &mut App, config: &AppConfig) -> EffectiveKeymap {
    cx.clear_key_bindings();
    install_app_keybindings(cx, config)
}

pub(crate) fn resolve_effective_keymap(config: &AppConfig) -> EffectiveKeymap {
    let mut bindings = Vec::new();
    let mut entries = Vec::new();
    let mut collisions: HashMap<(String, KeybindingContext), Vec<String>> = HashMap::new();
    let override_by_id = config
        .keybindings
        .overrides
        .iter()
        .map(|binding| (binding.id.as_str(), binding))
        .collect::<HashMap<_, _>>();

    for builtin in BUILTIN_KEYBINDINGS {
        let binding_override = override_by_id.get(builtin.id).copied();
        let default_display =
            display_chord_for_command(builtin.default_chord, builtin.command, builtin.context);
        let disabled = binding_override.is_some_and(|binding| binding.disabled);
        let effective_chord = binding_override
            .and_then(|binding| binding.chord.clone())
            .or_else(|| (!disabled).then(|| builtin.default_chord.to_string()));
        let effective_display = effective_chord
            .as_deref()
            .and_then(|chord| display_chord_for_command(chord, builtin.command, builtin.context));

        if let Some(chord) = effective_chord.as_deref()
            && let Some(binding) =
                build_binding_for_command(chord, builtin.command, builtin.context)
        {
            bindings.push(binding);
            register_collision(&mut collisions, chord, builtin.context, builtin.command);
        }

        entries.push(EffectiveKeybindingEntry {
            id: Some(builtin.id.to_string()),
            label: builtin.label.to_string(),
            command: builtin.command.to_string(),
            context: builtin.context,
            default_chord: Some(builtin.default_chord.to_string()),
            default_display,
            effective_chord,
            effective_display,
            disabled,
            customized: binding_override.is_some(),
            is_custom: false,
        });
    }

    for custom in &config.keybindings.custom {
        let effective_display =
            display_chord_for_command(&custom.chord, &custom.command, custom.context);
        if let Some(binding) =
            build_binding_for_command(&custom.chord, &custom.command, custom.context)
        {
            bindings.push(binding);
            register_collision(
                &mut collisions,
                &custom.chord,
                custom.context,
                &custom.command,
            );
        }

        entries.push(EffectiveKeybindingEntry {
            id: None,
            label: format!("Custom: {}", command_label(&custom.command)),
            command: custom.command.clone(),
            context: custom.context,
            default_chord: None,
            default_display: None,
            effective_chord: Some(custom.chord.clone()),
            effective_display,
            disabled: false,
            customized: true,
            is_custom: true,
        });
    }

    let conflicts = collisions
        .into_iter()
        .filter_map(|((chord, context), commands)| {
            (commands.len() > 1).then_some(KeybindingConflict {
                chord,
                context,
                commands,
            })
        })
        .collect();

    EffectiveKeymap {
        bindings,
        entries,
        conflicts,
    }
}

pub(crate) fn builtin_keybinding_entries(config: &AppConfig) -> EffectiveKeymap {
    resolve_effective_keymap(config)
}

pub(crate) fn command_label(command: &str) -> &'static str {
    BUILTIN_KEYBINDINGS
        .iter()
        .find(|binding| binding.command == command)
        .map(|binding| binding.label)
        .unwrap_or("Custom Command")
}

pub(crate) fn command_context(command: &str) -> KeybindingContext {
    BUILTIN_KEYBINDINGS
        .iter()
        .find(|binding| binding.command == command)
        .map(|binding| binding.context)
        .unwrap_or(KeybindingContext::AppGlobal)
}

pub(crate) fn command_shortcut(window: &Window, command: &str) -> Option<String> {
    let action = action_for_command(command)?;
    let context = command_context(command);
    binding_for_action(window, action.as_ref(), context).map(|binding| display_binding(&binding))
}

pub(crate) fn binding_for_action(
    window: &Window,
    action: &dyn Action,
    context: KeybindingContext,
) -> Option<KeyBinding> {
    match context_key(context) {
        Some(context_name) => {
            let parsed = KeyContext::parse(context_name).ok()?;
            window.highest_precedence_binding_for_action_in_context(action, parsed)
        }
        None => window.highest_precedence_binding_for_action(action),
    }
}

fn register_collision(
    collisions: &mut HashMap<(String, KeybindingContext), Vec<String>>,
    chord: &str,
    context: KeybindingContext,
    command: &str,
) {
    collisions
        .entry((normalize_chord_for_platform(chord), context))
        .or_default()
        .push(command.to_string());
}

fn display_chord_for_command(
    chord: &str,
    command: &str,
    context: KeybindingContext,
) -> Option<String> {
    build_binding_for_command(chord, command, context).map(|binding| display_binding(&binding))
}

fn display_binding(binding: &KeyBinding) -> String {
    binding
        .keystrokes()
        .iter()
        .map(|keystroke| format!("{keystroke}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn context_key(context: KeybindingContext) -> Option<&'static str> {
    match context {
        KeybindingContext::AppGlobal => None,
        KeybindingContext::WorkspaceTerminal => Some(CONTEXT_WORKSPACE_TERMINAL),
        KeybindingContext::WorkspaceSettings => Some(CONTEXT_WORKSPACE_SETTINGS),
        KeybindingContext::WorkspaceSecure => Some(CONTEXT_WORKSPACE_SECURE),
        KeybindingContext::WorkspaceSftp => Some(CONTEXT_WORKSPACE_SFTP),
        KeybindingContext::Palette => Some(CONTEXT_PALETTE),
        KeybindingContext::VaultModal => Some(CONTEXT_VAULT_MODAL),
        KeybindingContext::ConfirmDialog => Some(CONTEXT_CONFIRM_DIALOG),
    }
}

fn build_binding_for_command(
    chord: &str,
    command: &str,
    context: KeybindingContext,
) -> Option<KeyBinding> {
    let normalized = normalize_chord_for_platform(chord);
    let context_name = context_key(context);

    match command {
        COMMAND_APP_OPEN_PREFERENCES => {
            Some(KeyBinding::new(&normalized, OpenPreferences, context_name))
        }
        COMMAND_APP_OPEN_COMMAND_PALETTE => Some(KeyBinding::new(
            &normalized,
            OpenCommandPalette,
            context_name,
        )),
        COMMAND_SESSION_NEW_LOCAL => Some(KeyBinding::new(&normalized, NewTerminal, context_name)),
        COMMAND_SESSION_CLOSE_ACTIVE => Some(KeyBinding::new(
            &normalized,
            CloseActiveSession,
            context_name,
        )),
        COMMAND_SESSION_COPY_PREVIOUS_TURN => {
            Some(KeyBinding::new(&normalized, CopyPreviousTurn, context_name))
        }
        COMMAND_DEBUG_TOGGLE_PERF_HUD => {
            Some(KeyBinding::new(&normalized, TogglePerfHud, context_name))
        }
        COMMAND_WINDOW_NEW => Some(KeyBinding::new(&normalized, OpenNewWindow, context_name)),
        COMMAND_APP_QUIT => Some(KeyBinding::new(&normalized, QuitSeance, context_name)),
        COMMAND_APP_HIDE => Some(KeyBinding::new(&normalized, HideSeance, context_name)),
        COMMAND_SESSION_SELECT_PREVIOUS => Some(KeyBinding::new(
            &normalized,
            SelectPreviousSession,
            context_name,
        )),
        COMMAND_SESSION_SELECT_NEXT => Some(KeyBinding::new(
            &normalized,
            SelectNextSession,
            context_name,
        )),
        COMMAND_SESSION_SELECT_SLOT_1 => Some(KeyBinding::new(
            &normalized,
            SelectSessionSlot { slot: 1 },
            context_name,
        )),
        COMMAND_SESSION_SELECT_SLOT_2 => Some(KeyBinding::new(
            &normalized,
            SelectSessionSlot { slot: 2 },
            context_name,
        )),
        COMMAND_SESSION_SELECT_SLOT_3 => Some(KeyBinding::new(
            &normalized,
            SelectSessionSlot { slot: 3 },
            context_name,
        )),
        COMMAND_SESSION_SELECT_SLOT_4 => Some(KeyBinding::new(
            &normalized,
            SelectSessionSlot { slot: 4 },
            context_name,
        )),
        COMMAND_SESSION_SELECT_SLOT_5 => Some(KeyBinding::new(
            &normalized,
            SelectSessionSlot { slot: 5 },
            context_name,
        )),
        COMMAND_SESSION_SELECT_SLOT_6 => Some(KeyBinding::new(
            &normalized,
            SelectSessionSlot { slot: 6 },
            context_name,
        )),
        COMMAND_SESSION_SELECT_SLOT_7 => Some(KeyBinding::new(
            &normalized,
            SelectSessionSlot { slot: 7 },
            context_name,
        )),
        COMMAND_SESSION_SELECT_SLOT_8 => Some(KeyBinding::new(
            &normalized,
            SelectSessionSlot { slot: 8 },
            context_name,
        )),
        COMMAND_SESSION_SELECT_SLOT_9 => Some(KeyBinding::new(
            &normalized,
            SelectSessionSlot { slot: 9 },
            context_name,
        )),
        COMMAND_SESSION_SELECT_SLOT_10 => Some(KeyBinding::new(
            &normalized,
            SelectSessionSlot { slot: 10 },
            context_name,
        )),
        _ => None,
    }
}

fn action_for_command(command: &str) -> Option<Box<dyn Action>> {
    match command {
        COMMAND_APP_OPEN_PREFERENCES => Some(Box::new(OpenPreferences)),
        COMMAND_APP_OPEN_COMMAND_PALETTE => Some(Box::new(OpenCommandPalette)),
        COMMAND_SESSION_NEW_LOCAL => Some(Box::new(NewTerminal)),
        COMMAND_SESSION_CLOSE_ACTIVE => Some(Box::new(CloseActiveSession)),
        COMMAND_SESSION_COPY_PREVIOUS_TURN => Some(Box::new(CopyPreviousTurn)),
        COMMAND_DEBUG_TOGGLE_PERF_HUD => Some(Box::new(TogglePerfHud)),
        COMMAND_WINDOW_NEW => Some(Box::new(OpenNewWindow)),
        COMMAND_APP_QUIT => Some(Box::new(QuitSeance)),
        COMMAND_APP_HIDE => Some(Box::new(HideSeance)),
        COMMAND_SESSION_SELECT_PREVIOUS => Some(Box::new(SelectPreviousSession)),
        COMMAND_SESSION_SELECT_NEXT => Some(Box::new(SelectNextSession)),
        COMMAND_SESSION_SELECT_SLOT_1 => Some(Box::new(SelectSessionSlot { slot: 1 })),
        COMMAND_SESSION_SELECT_SLOT_2 => Some(Box::new(SelectSessionSlot { slot: 2 })),
        COMMAND_SESSION_SELECT_SLOT_3 => Some(Box::new(SelectSessionSlot { slot: 3 })),
        COMMAND_SESSION_SELECT_SLOT_4 => Some(Box::new(SelectSessionSlot { slot: 4 })),
        COMMAND_SESSION_SELECT_SLOT_5 => Some(Box::new(SelectSessionSlot { slot: 5 })),
        COMMAND_SESSION_SELECT_SLOT_6 => Some(Box::new(SelectSessionSlot { slot: 6 })),
        COMMAND_SESSION_SELECT_SLOT_7 => Some(Box::new(SelectSessionSlot { slot: 7 })),
        COMMAND_SESSION_SELECT_SLOT_8 => Some(Box::new(SelectSessionSlot { slot: 8 })),
        COMMAND_SESSION_SELECT_SLOT_9 => Some(Box::new(SelectSessionSlot { slot: 9 })),
        COMMAND_SESSION_SELECT_SLOT_10 => Some(Box::new(SelectSessionSlot { slot: 10 })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use seance_config::{
        COMMAND_APP_OPEN_COMMAND_PALETTE, COMMAND_SESSION_SELECT_SLOT_10, CustomKeybinding,
        KeybindingOverride,
    };

    use super::*;

    #[test]
    fn primary_alias_normalizes_for_builtin_display() {
        let display = display_chord_for_command(
            "primary-k",
            COMMAND_APP_OPEN_COMMAND_PALETTE,
            KeybindingContext::AppGlobal,
        )
        .expect("display");
        assert!(!display.is_empty());
    }

    #[test]
    fn duplicate_context_chords_are_reported_as_conflicts() {
        let mut config = AppConfig::default();
        config.keybindings.custom.push(CustomKeybinding {
            chord: "primary-0".into(),
            command: COMMAND_APP_OPEN_COMMAND_PALETTE.into(),
            context: KeybindingContext::WorkspaceTerminal,
        });

        let effective = resolve_effective_keymap(&config);
        assert!(effective.conflicts.iter().any(|conflict| {
            conflict.chord == normalize_chord_for_platform("primary-0")
                && conflict.context == KeybindingContext::WorkspaceTerminal
                && conflict
                    .commands
                    .iter()
                    .any(|command| command == COMMAND_SESSION_SELECT_SLOT_10)
        }));
    }

    #[test]
    fn overrides_replace_builtin_bindings() {
        let mut config = AppConfig::default();
        config.keybindings.overrides.push(KeybindingOverride {
            id: COMMAND_APP_OPEN_COMMAND_PALETTE.into(),
            chord: Some("primary-p".into()),
            disabled: false,
        });

        let effective = resolve_effective_keymap(&config);
        let palette_entry = effective
            .entries
            .iter()
            .find(|entry| entry.command == COMMAND_APP_OPEN_COMMAND_PALETTE)
            .expect("palette entry");

        assert_eq!(palette_entry.effective_chord.as_deref(), Some("primary-p"));
        assert!(
            palette_entry
                .effective_display
                .as_deref()
                .is_some_and(|display| display.to_ascii_lowercase().contains('p'))
        );
    }

    #[test]
    fn disabled_overrides_remove_builtin_bindings() {
        let mut config = AppConfig::default();
        config.keybindings.overrides.push(KeybindingOverride {
            id: COMMAND_APP_OPEN_COMMAND_PALETTE.into(),
            chord: None,
            disabled: true,
        });

        let effective = resolve_effective_keymap(&config);
        let palette_entry = effective
            .entries
            .iter()
            .find(|entry| entry.command == COMMAND_APP_OPEN_COMMAND_PALETTE)
            .expect("palette entry");

        assert!(palette_entry.disabled);
        assert!(palette_entry.effective_chord.is_none());
    }
}
