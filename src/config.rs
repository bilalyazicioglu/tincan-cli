//! Persistent user configuration.
//!
//! Stores user preferences such as preferred audio input/output devices
//! across sessions in `~/.config/tincan/config.toml`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Where the microphone gate sits by default, as a position on the level meter.
///
/// This is where the detector's original hard-coded 0.01 RMS actually lands, so an
/// installation that never touches the setting behaves exactly as it did before.
pub const DEFAULT_GATE: f32 = 0.23;

/// How loud key clicks are when they have never been adjusted. Under half, because a
/// keyboard should sit beneath what you are writing rather than over it.
pub const DEFAULT_TYPING_VOLUME: f32 = 0.4;

/// Persistent application configuration.
///
/// Not `Eq`: the gate is a float. `PartialEq` is all the comparisons here need.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Config {
    /// Preferred microphone name (partial matching supported).
    pub input_device: Option<String>,
    /// Preferred speaker / headphone name (partial matching supported).
    pub output_device: Option<String>,
    /// The gate below which each microphone's audio is treated as room noise, keyed by
    /// device name. Kept per device because a laptop microphone and a headset do not
    /// share a noise floor, and one value for both is wrong for at least one of them.
    #[serde(default)]
    pub input_gates: HashMap<String, f32>,
    /// Whether typing makes a sound. Off until asked for: a keyboard that starts
    /// clicking on its own is a surprise, not a feature.
    #[serde(default)]
    pub typing_clicks: bool,
    /// How loud those clicks are, 0.0 to 1.0. `None` means never adjusted.
    #[serde(default)]
    pub typing_volume: Option<f32>,
}

impl Config {
    /// The gate for a microphone, or the default for one never adjusted.
    pub fn gate_for(&self, device: Option<&str>) -> f32 {
        device
            .and_then(|name| self.input_gates.get(name))
            .copied()
            .unwrap_or(DEFAULT_GATE)
            .clamp(0.0, 1.0)
    }

    /// How loud key clicks should be, or the default for a setting never touched.
    pub fn typing_loudness(&self) -> f32 {
        self.typing_volume
            .unwrap_or(DEFAULT_TYPING_VOLUME)
            .clamp(0.0, 1.0)
    }

    pub fn set_gate(&mut self, device: &str, level: f32) {
        self.input_gates
            .insert(device.to_string(), level.clamp(0.0, 1.0));
    }

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
    fn an_unadjusted_microphone_gets_the_default_gate() {
        let mut config = Config::default();
        assert_eq!(config.gate_for(Some("MacBook Pro Microphone")), DEFAULT_GATE);
        assert_eq!(config.gate_for(None), DEFAULT_GATE);

        config.set_gate("AirPods", 0.4);
        assert_eq!(config.gate_for(Some("AirPods")), 0.4);
        assert_eq!(
            config.gate_for(Some("MacBook Pro Microphone")),
            DEFAULT_GATE,
            "one microphone's noise floor says nothing about another's"
        );
    }

    #[test]
    fn typing_is_silent_until_it_is_asked_for() {
        let config = Config::default();
        assert!(!config.typing_clicks, "a keyboard that starts clicking on its own is a surprise");
        assert_eq!(config.typing_loudness(), DEFAULT_TYPING_VOLUME, "but it has a sane volume waiting");
    }

    #[test]
    fn a_config_written_before_gates_existed_still_loads() {
        let dir = std::env::temp_dir().join("tincan_test_old_config");
        let path = dir.join("config.toml");
        let _ = fs::create_dir_all(&dir);
        fs::write(&path, "input_device = \"MacBook Pro Microphone\"\n").unwrap();

        let config = Config::load_from(&path).expect("an older config must still open");
        assert_eq!(config.input_device.as_deref(), Some("MacBook Pro Microphone"));
        assert_eq!(config.gate_for(Some("MacBook Pro Microphone")), DEFAULT_GATE);
        assert!(!config.typing_clicks);
        assert_eq!(config.typing_loudness(), DEFAULT_TYPING_VOLUME);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn roundtrip_save_and_load() {
        let temp_dir = std::env::temp_dir().join("tincan_test_config");
        let path = temp_dir.join("test_config.toml");

        let original = Config {
            input_device: Some("MacBook Pro Microphone".into()),
            output_device: Some("External Headphones".into()),
            input_gates: HashMap::from([("MacBook Pro Microphone".to_string(), 0.31)]),
            typing_clicks: true,
            typing_volume: Some(0.6),
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
