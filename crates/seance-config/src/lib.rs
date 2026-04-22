//! Root config API map: models, validation, defaults, and storage stay in dedicated modules,
//! while callers keep importing the public surface from the crate root.

mod defaults;
mod keybindings;
mod model;
mod storage;
mod validation;

pub use defaults::{DEFAULT_THEME_KEY, SUPPORTED_THEME_KEYS};
pub use keybindings::{
    BUILTIN_KEYBINDING_IDS, COMMAND_APP_HIDE, COMMAND_APP_OPEN_COMMAND_PALETTE,
    COMMAND_APP_OPEN_PREFERENCES, COMMAND_APP_QUIT, COMMAND_DEBUG_TOGGLE_PERF_HUD,
    COMMAND_SESSION_CLOSE_ACTIVE, COMMAND_SESSION_COPY_PREVIOUS_TURN, COMMAND_SESSION_NEW_LOCAL,
    COMMAND_SESSION_SELECT_NEXT, COMMAND_SESSION_SELECT_PREVIOUS, COMMAND_SESSION_SELECT_SLOT_1,
    COMMAND_SESSION_SELECT_SLOT_2, COMMAND_SESSION_SELECT_SLOT_3, COMMAND_SESSION_SELECT_SLOT_4,
    COMMAND_SESSION_SELECT_SLOT_5, COMMAND_SESSION_SELECT_SLOT_6, COMMAND_SESSION_SELECT_SLOT_7,
    COMMAND_SESSION_SELECT_SLOT_8, COMMAND_SESSION_SELECT_SLOT_9, COMMAND_SESSION_SELECT_SLOT_10,
    COMMAND_WINDOW_NEW, CustomKeybinding, KeybindingContext, KeybindingOverride, KeybindingsConfig,
    SUPPORTED_KEYBINDING_COMMANDS, command_default_context, is_builtin_keybinding_id,
    is_supported_keybinding_command, legacy_action_to_command, normalize_chord_for_platform,
    validate_chord_syntax,
};
pub use model::{
    AppConfig, AppearanceConfig, DebugConfig, LoggingConfig, MouseTrackingScrollPolicy,
    MouseTrackingSelectionPolicy, PerfHudDefault, TerminalConfig, TerminalInteractionConfig,
    TerminalRightClickPolicy, UpdateConfig, UpdateInstallMode, UpdateReleaseChannel,
    VaultRegistryConfig, VaultRegistryEntry, WindowConfig,
};
pub use storage::ConfigStore;
pub use validation::ConfigError;
