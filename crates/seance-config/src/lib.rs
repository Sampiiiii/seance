//! Root config API map: models, validation, defaults, and storage stay in dedicated modules,
//! while callers keep importing the public surface from the crate root.

mod defaults;
mod model;
mod storage;
mod validation;

pub use defaults::{DEFAULT_THEME_KEY, SUPPORTED_KEYBINDING_ACTIONS, SUPPORTED_THEME_KEYS};
pub use model::{
    AppConfig, AppearanceConfig, DebugConfig, KeybindingOverride, KeybindingsConfig,
    PerfHudDefault, TerminalConfig, UpdateConfig, UpdateInstallMode, UpdateReleaseChannel,
    WindowConfig,
};
pub use storage::ConfigStore;
pub use validation::ConfigError;
