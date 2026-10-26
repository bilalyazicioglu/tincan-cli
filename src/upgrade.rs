//! What tincan says when it meets something only a newer version understands.
//!
//! An invite in a form this version does not know, or a room that has moved on to a
//! newer protocol, ends in the same place: one message that says what needed the newer
//! version, which version, and the command that updates the copy that is running.

use std::fmt;
use std::path::Path;

/// Asked to read or reach something that a newer tincan understands and this one does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedsNewerTincan {
    /// What it was, as the sentence will name it: "this invite", "this room".
    pub what: String,
    /// The oldest version that understands it, when that is known.
    pub min_version: Option<String>,
}

impl fmt::Display for NeedsNewerTincan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.min_version {
            Some(version) => write!(f, "{} needs tincan {version} or newer", self.what)?,
            None => write!(f, "{} needs a newer tincan", self.what)?,
        }
        let hint = std::env::current_exe()
            .map(|exe| update_hint(&exe))
            .unwrap_or(ALL_METHODS);
        write!(f, ". update with: {hint}")
    }
}

impl std::error::Error for NeedsNewerTincan {}

/// Builds the error; `min_version` is left out when nobody knows which version it takes.
pub fn needs_newer(what: impl Into<String>, min_version: Option<&str>) -> NeedsNewerTincan {
    NeedsNewerTincan {
        what: what.into(),
        min_version: min_version.map(str::to_string),
    }
}

const ALL_METHODS: &str =
    "brew upgrade tincan, npm i -g tincan-cli@latest, or cargo install tincan-chat";

/// The update command for the way this binary was installed, read off where it lives.
/// A path that gives nothing away gets every method, rather than a guess.
pub fn update_hint(exe: &Path) -> &'static str {
    let path = exe.to_string_lossy();
    if path.contains("/Cellar/") || path.contains("homebrew") {
        "brew upgrade tincan"
    } else if path.contains("node_modules") {
        "npm i -g tincan-cli@latest"
    } else if path.contains("/.cargo/bin/") {
        "cargo install tincan-chat"
    } else if path.starts_with("/nix/store/") {
        "nix profile upgrade tincan"
    } else {
        ALL_METHODS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn update_hint_follows_the_install() {
        assert_eq!(
            update_hint(Path::new("/opt/homebrew/Cellar/tincan/0.4.0/bin/tincan")),
            "brew upgrade tincan"
        );
        assert_eq!(
            update_hint(Path::new(
                "/usr/local/lib/node_modules/tincan-cli/bin/tincan"
            )),
            "npm i -g tincan-cli@latest"
        );
        assert_eq!(
            update_hint(Path::new("/Users/someone/.cargo/bin/tincan")),
            "cargo install tincan-chat"
        );
        assert_eq!(
            update_hint(Path::new("/nix/store/abc-tincan-0.4.0/bin/tincan")),
            "nix profile upgrade tincan"
        );
        assert_eq!(
            update_hint(Path::new("/usr/local/bin/tincan")),
            "brew upgrade tincan, npm i -g tincan-cli@latest, or cargo install tincan-chat"
        );
    }

    #[test]
    fn the_message_names_what_and_the_version() {
        let text = needs_newer("this invite", Some("0.4.0")).to_string();
        assert!(text.contains("this invite"), "{text}");
        assert!(text.contains("0.4.0"), "{text}");
        assert!(text.contains("update with:"), "{text}");
    }
}
