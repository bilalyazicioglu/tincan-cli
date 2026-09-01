//! Persistent user configuration.
//!
//! Stores user preferences such as preferred audio input/output devices
//! across sessions in `~/.config/tincan/config.toml`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Persistent application configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Config {
    /// Preferred microphone name (partial matching supported).
    pub input_device: Option<String>,
    /// Preferred speaker / headphone name (partial matching supported).
    pub output_device: Option<String>,
}

impl Config {
    /// Returns the standard path to the configuration file:
    /// `~/.config/tincan/config.toml` (or `$XDG_CONFIG_HOME/tincan/config.toml`).
    pub fn default_path() -> Option<PathBuf> {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
            && !xdg.is_empty()
        {
            return Some(PathBuf::from(xdg).join("tincan").join("config.toml"));
        }
        if let Ok(home) = std::env::var("HOME")
            && !home.is_empty()
        {
            return Some(PathBuf::from(home).join(".config").join("tincan").join("config.toml"));
        }
        None
    }

    /// Loads the configuration from the standard path.
    /// Returns default settings if the file does not exist or fails to parse.
    pub fn load() -> Self {
        match Self::default_path() {
            Some(path) => Self::load_from(&path).unwrap_or_default(),
            None => Self::default(),
        }
    }

    /// Loads configuration from an explicit file path.
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("could not read config file: {}", path.display()))?;
        let config: Config = toml::from_str(&content)
            .with_context(|| format!("invalid config format in: {}", path.display()))?;
        Ok(config)
    }

    /// Saves the current configuration to the standard path.
    pub fn save(&self) -> Result<()> {
        let path = Self::default_path().context("could not determine user config directory")?;
        self.save_to(&path)
    }

    /// Saves configuration to an explicit file path.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("could not create directory: {}", parent.display()))?;
        }
        let content = toml::to_string_pretty(self)
            .context("could not serialize configuration to TOML")?;
        fs::write(path, content)
            .with_context(|| format!("could not write config file: {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_empty() {
        let config = Config::default();
        assert_eq!(config.input_device, None);
        assert_eq!(config.output_device, None);
    }

    #[test]
    fn roundtrip_save_and_load() {
        let temp_dir = std::env::temp_dir().join("tincan_test_config");
        let path = temp_dir.join("test_config.toml");

        let original = Config {
            input_device: Some("MacBook Pro Microphone".into()),
            output_device: Some("External Headphones".into()),
        };

        original.save_to(&path).expect("saving config should succeed");
        let loaded = Config::load_from(&path).expect("loading config should succeed");
        assert_eq!(original, loaded);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn load_nonexistent_returns_default() {
        let path = Path::new("/tmp/nonexistent_tincan_config_12345.toml");
        let config = Config::load_from(path).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn invalid_toml_returns_error() {
        let temp_dir = std::env::temp_dir().join("tincan_test_invalid_config");
        let path = temp_dir.join("invalid.toml");
        let _ = fs::create_dir_all(&temp_dir);
        fs::write(&path, "invalid [[[ toml = ").unwrap();

        assert!(Config::load_from(&path).is_err());
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
