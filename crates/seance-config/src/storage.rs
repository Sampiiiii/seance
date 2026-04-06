use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use tempfile::NamedTempFile;

use crate::{AppConfig, ConfigError};

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
    temp.flush().map_err(|source| ConfigError::Write { source })?;
    temp.persist(path)
        .map_err(|error| ConfigError::Write { source: error.error })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::{AppConfig, ConfigStore, DEFAULT_THEME_KEY, PerfHudDefault};

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
                config.debug.perf_hud_default = PerfHudDefault::Expanded;
            })
            .unwrap();

        let reloaded = ConfigStore::load_or_default(&path).unwrap();
        assert_eq!(reloaded.snapshot(), saved);
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