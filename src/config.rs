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

/// How many messages are kept in memory for the chat pane by default.
pub const DEFAULT_HISTORY_LIMIT: usize = 5000;

/// Persistent application configuration.
///
/// Not `Eq`: the gate is a float. `PartialEq` is all the comparisons here need.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    /// Whether the microphone is cleaned up before anyone hears it. On unless
    /// turned off, which is the opposite of the rule above and for the opposite
    /// reason: the whole value of this one is that you never had to find it.
    #[serde(default = "enabled")]
    pub denoise: bool,
    /// How loud those clicks are, 0.0 to 1.0. `None` means never adjusted.
    #[serde(default)]
    pub typing_volume: Option<f32>,
    /// Maximum number of lines kept in memory for scrollback.
    #[serde(default)]
    pub history_limit: Option<usize>,
}

/// `#[serde(default)]` on a `bool` yields `false`, so a setting that should
/// arrive switched on needs one of these.
fn enabled() -> bool {
    true
}

// Derived, this would hand back `denoise: false` — and `Config::load` falls back
// to it whenever there is no file yet, which is every first run. The one setting
// that defaults to on would have been off for exactly the people it is for.
impl Default for Config {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            input_gates: HashMap::new(),
            typing_clicks: false,
            denoise: enabled(),
            typing_volume: None,
            history_limit: None,
        }
    }
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

    /// The maximum number of lines kept in memory, or the default (5000).
    pub fn history_limit(&self) -> usize {
        self.history_limit.unwrap_or(DEFAULT_HISTORY_LIMIT).max(100)
    }

    pub fn set_gate(&mut self, device: &str, level: f32) {
        self.input_gates
            .insert(device.to_string(), level.clamp(0.0, 1.0));
    }

    /// Returns the standard path to the configuration file:
    /// `~/.config/tincan/config.toml` (or `$XDG_CONFIG_HOME/tincan/config.toml`),
    /// and `%APPDATA%\\tincan\\config.toml` on Windows.
    pub fn default_path() -> Option<PathBuf> {
        Self::default_path_for(cfg!(windows), |key| {
            std::env::var(key).ok().filter(|v| !v.is_empty())
        })
    }

    /// The path chosen from a set of environment variables. Both the platform and
    /// the environment are arguments so that either branch can be tested from
    /// either kind of machine — otherwise the Windows rule would only ever be
    /// exercised by a Windows runner, which is how it came to be missing.
    fn default_path_for(windows: bool, env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
        let file = |dir: PathBuf| Some(dir.join("tincan").join("config.toml"));

        // Windows sets neither XDG_CONFIG_HOME nor HOME, so a Unix-only lookup
        // returned None there and every setting the user changed was discarded
        // on exit without a word. APPDATA is where this belongs on Windows, and
        // it is checked first so a stray HOME from a shell like Git Bash cannot
        // scatter the config somewhere the native build will never look.
        if windows {
            if let Some(appdata) = env("APPDATA") {
                return file(PathBuf::from(appdata));
            }
            if let Some(profile) = env("USERPROFILE") {
                return file(PathBuf::from(profile).join("AppData").join("Roaming"));
            }
        }

        if let Some(xdg) = env("XDG_CONFIG_HOME") {
            return file(PathBuf::from(xdg));
        }
        if let Some(home) = env("HOME") {
            return file(PathBuf::from(home).join(".config"));
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
        let content =
            toml::to_string_pretty(self).context("could not serialize configuration to TOML")?;
        fs::write(path, content)
            .with_context(|| format!("could not write config file: {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Looks up `key` in a fixed list, standing in for the process environment.
    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + use<'a> {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_string())
        }
    }

    #[test]
    fn xdg_config_home_wins_where_it_is_set() {
        let path = Config::default_path_for(
            false,
            env_of(&[("XDG_CONFIG_HOME", "/tmp/xdg"), ("HOME", "/home/alice")]),
        )
        .expect("a set XDG_CONFIG_HOME always yields a path");
        assert_eq!(path, PathBuf::from("/tmp/xdg/tincan/config.toml"));
    }

    #[test]
    fn home_is_the_fallback_on_unix() {
        let path = Config::default_path_for(false, env_of(&[("HOME", "/home/alice")]))
            .expect("HOME alone is enough");
        assert_eq!(
            path,
            PathBuf::from("/home/alice/.config/tincan/config.toml")
        );
    }

    #[test]
    fn nowhere_to_put_it_is_reported_rather_than_guessed() {
        assert_eq!(Config::default_path_for(false, env_of(&[])), None);
        assert_eq!(Config::default_path_for(true, env_of(&[])), None);
    }

    #[test]
    fn windows_settings_land_under_appdata() {
        let path = Config::default_path_for(
            true,
            env_of(&[
                ("APPDATA", "C:/Users/alice/AppData/Roaming"),
                // Git Bash sets HOME; it must not pull the config out of APPDATA.
                ("HOME", "/c/Users/alice"),
            ]),
        )
        .expect("APPDATA is set on every Windows session");
        assert_eq!(
            path,
            PathBuf::from("C:/Users/alice/AppData/Roaming/tincan/config.toml")
        );
    }

    #[test]
    fn windows_falls_back_to_the_user_profile() {
        let path = Config::default_path_for(true, env_of(&[("USERPROFILE", "C:/Users/alice")]))
            .expect("USERPROFILE is the backstop when APPDATA is missing");
        assert_eq!(
            path,
            PathBuf::from("C:/Users/alice/AppData/Roaming/tincan/config.toml")
        );
    }

    #[test]
    fn default_config_is_empty() {
        let config = Config::default();
        assert_eq!(config.input_device, None);
        assert_eq!(config.output_device, None);
    }

    #[test]
    fn an_unadjusted_microphone_gets_the_default_gate() {
        let mut config = Config::default();
        assert_eq!(
            config.gate_for(Some("MacBook Pro Microphone")),
            DEFAULT_GATE
        );
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
    fn noise_suppression_is_on_unless_it_was_turned_off() {
        assert!(
            Config::default().denoise,
            "a first run has no file, and this is the setting whose point is that nobody had to find it"
        );
    }

    #[test]
    fn a_config_written_before_denoise_existed_still_has_it_on() {
        let dir = std::env::temp_dir().join("tincan_test_denoise_default");
        let path = dir.join("config.toml");
        let _ = fs::create_dir_all(&dir);
        fs::write(&path, "typing_clicks = true\n").unwrap();

        let config = Config::load_from(&path).expect("an older config must still open");
        assert!(config.denoise, "a missing field is not an answer of 'off'");
        assert!(
            config.typing_clicks,
            "and the rest of the file still applies"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn turning_noise_suppression_off_survives_a_round_trip() {
        let dir = std::env::temp_dir().join("tincan_test_denoise_off");
        let path = dir.join("config.toml");
        let config = Config {
            denoise: false,
            ..Config::default()
        };
        config.save_to(&path).expect("saving must succeed");

        let loaded = Config::load_from(&path).expect("loading must succeed");
        assert!(
            !loaded.denoise,
            "the default must not overwrite a deliberate 'off' on the next run"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn typing_is_silent_until_it_is_asked_for() {
        let config = Config::default();
        assert!(
            !config.typing_clicks,
            "a keyboard that starts clicking on its own is a surprise"
        );
        assert_eq!(
            config.typing_loudness(),
            DEFAULT_TYPING_VOLUME,
            "but it has a sane volume waiting"
        );
    }

    #[test]
    fn a_config_written_before_gates_existed_still_loads() {
        let dir = std::env::temp_dir().join("tincan_test_old_config");
        let path = dir.join("config.toml");
        let _ = fs::create_dir_all(&dir);
        fs::write(&path, "input_device = \"MacBook Pro Microphone\"\n").unwrap();

        let config = Config::load_from(&path).expect("an older config must still open");
        assert_eq!(
            config.input_device.as_deref(),
            Some("MacBook Pro Microphone")
        );
        assert_eq!(
            config.gate_for(Some("MacBook Pro Microphone")),
            DEFAULT_GATE
        );
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
            denoise: false,
            typing_volume: Some(0.6),
            history_limit: Some(8000),
        };

        original
            .save_to(&path)
            .expect("saving config should succeed");
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
