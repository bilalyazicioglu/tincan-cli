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

#[cfg(not(target_os = "macos"))]
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
