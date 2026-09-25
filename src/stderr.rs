//! Keeps what C libraries print from landing on top of the interface.
//!
//! alsa-lib, and the JACK and PulseAudio plugins it loads, report their troubles by
//! writing straight to file descriptor 2. Nothing in Rust sees those lines, so the
//! log file that catches `tracing` never catches them: on Linux, opening the audio
//! settings used to paint a screenful of `ALSA lib pcm_route.c:…` over the room.
//!
//! While the interface is up, descriptor 2 points at this run's log file instead,
//! so the lines are kept rather than drawn. Anything that is not a terminal is left
//! alone, for the same reason the log is: `2>tincan.log` means what it says.

use std::fs::File;
use std::sync::OnceLock;

/// The log file, shared with `tracing`. It must be opened for appending, or the two
/// writers would each keep their own offset and write over one another.
static SINK: OnceLock<File> = OnceLock::new();

/// Names the file that stray output goes to while the interface is up.
pub fn sink_into(file: File) {
    let _ = SINK.set(file);
}

/// Points descriptor 2 at the log until the guard is dropped. Nothing happens when
/// there is no log file, which is also the case whenever stderr is not a terminal.
pub fn divert() -> Option<Diverted> {
    divert_to(SINK.get()?)
}

/// Puts descriptor 2 back if it is still diverted. Safe to call at any time, and
/// called from the panic hook, so a panic's message reaches the screen.
pub fn restore() {
    imp::restore();
}

/// Descriptor 2 is diverted for as long as this lives.
#[must_use]
pub struct Diverted(());

impl Drop for Diverted {
    fn drop(&mut self) {
        restore();
    }
}

fn divert_to(file: &File) -> Option<Diverted> {
    // Not `then_some`: a guard built and thrown away would restore on the spot.
    imp::divert(file).then(|| Diverted(()))
}

#[cfg(unix)]
mod imp {
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::sync::atomic::{AtomicI32, Ordering};

    /// A copy of the real stderr, or -1 while nothing is diverted.
    static SAVED: AtomicI32 = AtomicI32::new(-1);

    pub fn divert(file: &File) -> bool {
        if SAVED.load(Ordering::SeqCst) >= 0 {
            return false;
        }
        // SAFETY: plain descriptor calls; every result is checked before it is used.
        unsafe {
            let saved = libc::dup(libc::STDERR_FILENO);
            if saved < 0 {
                return false;
            }
            if libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) < 0 {
                libc::close(saved);
                return false;
            }
            SAVED.store(saved, Ordering::SeqCst);
        }
        true
    }

    pub fn restore() {
        let saved = SAVED.swap(-1, Ordering::SeqCst);
        if saved < 0 {
            return;
        }
        // SAFETY: `saved` came from `dup` and nothing else holds it.
        unsafe {
            libc::dup2(saved, libc::STDERR_FILENO);
            libc::close(saved);
        }
    }
}

// The audio stacks elsewhere report through their own APIs, not by printing.
#[cfg(not(unix))]
mod imp {
    use std::fs::File;

    pub fn divert(_file: &File) -> bool {
        false
    }

    pub fn restore() {}
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt as _;

    /// What descriptor 2 currently points at.
    fn stderr_identity() -> (u64, u64) {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::fstat(libc::STDERR_FILENO, &mut stat) }, 0);
        (stat.st_dev as u64, stat.st_ino as u64)
    }

    /// Writes the way alsa-lib does: straight to the descriptor, past Rust entirely.
    fn write_like_c(text: &str) {
        unsafe { libc::write(libc::STDERR_FILENO, text.as_ptr().cast(), text.len()) };
    }

    #[test]
    fn diverted_output_lands_in_the_file_and_stderr_comes_back() {
        let path = std::env::temp_dir().join(format!("tincan-stderr-{}.log", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .read(true)
            .open(&path)
            .unwrap();
        let before = stderr_identity();

        let guard = divert_to(&file).expect("diverts");
        assert!(divert_to(&file).is_none(), "a second divert would lose the real stderr");
        write_like_c("ALSA lib pcm_route.c:886: no matching channel map\n");
        let during = stderr_identity();
        drop(guard);

        let meta = file.metadata().unwrap();
        assert_eq!(during, (meta.dev(), meta.ino()));
        assert_eq!(stderr_identity(), before);
        let log = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(log, "ALSA lib pcm_route.c:886: no matching channel map\n");
    }
}
