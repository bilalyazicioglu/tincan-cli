//! Best-effort clipboard support.
//!
//! The invite code is 63 characters and its whole purpose is to reach someone
//! else, so getting it out of the terminal has to be effortless. Shelling out to
//! the platform's clipboard tool keeps this dependency-free, and every failure is
//! silent: the code is always shown as text too, so a missing tool costs the user
//! nothing.

use std::io::Write;
use std::process::{Command, Stdio};

#[cfg(target_os = "macos")]
const CANDIDATES: &[(&str, &[&str])] = &[("pbcopy", &[])];

// `clip.exe` would be the shorter answer, but it decodes stdin as the machine's
// local code page, which mangles anything outside it. PowerShell reads UTF-8 and
// is present on every Windows that can run this.
#[cfg(target_os = "windows")]
const CANDIDATES: &[(&str, &[&str])] = &[(
    "powershell",
    &[
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "$input | Set-Clipboard",
    ],
)];

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const CANDIDATES: &[(&str, &[&str])] = &[
    ("wl-copy", &[]),
    ("xclip", &["-selection", "clipboard"]),
    ("xsel", &["--clipboard", "--input"]),
];

/// Copies `text` to the system clipboard, reporting whether it worked.
pub fn copy(text: &str) -> bool {
    CANDIDATES
        .iter()
        .any(|(program, args)| try_copy(program, args, text))
}

fn try_copy(program: &str, args: &[&str], text: &str) -> bool {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };

    let Some(mut stdin) = child.stdin.take() else {
        return false;
    };
    let written = stdin.write_all(text.as_bytes()).is_ok();
    // The pipe has to close before the tool will exit.
    drop(stdin);

    written && child.wait().map(|status| status.success()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every platform has to name at least one tool, or `copy` can only ever
    /// return false and the invite code silently never reaches the clipboard —
    /// which is what Windows did before it had a branch of its own.
    #[test]
    fn this_platform_has_a_clipboard_tool() {
        assert!(!CANDIDATES.is_empty());
    }

    #[test]
    fn a_missing_tool_fails_quietly_rather_than_panicking() {
        assert!(!try_copy("tincan-no-such-clipboard-tool", &[], "hello"));
    }
}
