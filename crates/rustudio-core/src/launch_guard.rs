//! Launch guard: a launch whose window never drew makes the next one use OpenGL.
//!
//! Some graphics drivers crash the whole process as the window's Vulkan or
//! DirectX 12 device is created (an access violation inside an old Intel HD /
//! UHD driver, for instance): the app shows for a second and disappears, with
//! no message and no log line, because a crash inside a driver can't be
//! caught. OpenGL is what those drivers run.
//!
//! So each launch leaves a marker in the config folder until its window has
//! drawn a few frames. A launch that finds the previous one's marker switches
//! Settings > rustudio > Performance > Graphics API to OpenGL, saves it, and
//! says so in Output. The first launch after such a crash still disappears; the
//! guard helps from the second launch on.
//!
//! The marker is a file the launch holds an exclusive lock on, so:
//! - a crashed or killed launch releases the lock but leaves the marker's
//!   text behind (that is what the next launch looks for);
//! - a second copy started while the first is still opening finds the lock
//!   taken, which is not a crash, and leaves the first's marker alone.
//!
//! `WGPU_BACKEND` and `RUSTUDIO_PERF_GRAPHICS` (the option's own variable) win:
//! with either set the guard does nothing. macOS has no OpenGL backend, so it
//! never switches there.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Left in the config folder from a launch until its window has drawn.
const MARKER: &str = "launch-pending";

/// Frames the window must draw before a launch counts as having made it: the
/// first frame's present happens after its UI pass, and a driver can take a
/// frame or two to fail.
const FRAMES_TO_DRAW: u32 = 3;

/// What `begin_in` found.
#[derive(Debug)]
pub struct Begun {
    /// The previous launch left its marker and is gone: it never drew.
    pub previous_never_drew: bool,
    /// This launch's marker, held until `Marker::cleared`. `None` when
    /// another launch is still opening or the folder can't be written.
    pub marker: Option<Marker>,
}

/// This launch's marker. Dropping it without `cleared` (a crash, a panic, an
/// exit before drawing) leaves the marker's text for the next launch.
#[derive(Debug)]
pub struct Marker {
    file: File,
    path: PathBuf,
}

impl Marker {
    /// The window drew: the launch made it.
    pub fn cleared(self) {
        let Marker { file, path } = self;
        // (emptied first, so a marker that can't be deleted is still no marker)
        let _ = file.set_len(0);
        drop(file);
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!(target: "rustudio::gpu", "launch marker: {e}");
            }
        }
    }
}

/// Leave this launch's marker in `dir` and report whether the previous launch
/// left one that nobody holds.
pub fn begin_in(dir: &Path) -> Begun {
    let none = Begun { previous_never_drew: false, marker: None };
    if let Err(e) = std::fs::create_dir_all(dir) {
        log::warn!(target: "rustudio::gpu", "launch marker: {e}");
        return none;
    }
    let path = dir.join(MARKER);
    let mut file = match OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!(target: "rustudio::gpu", "launch marker: {e}");
            return none;
        }
    };
    match file.try_lock() {
        Ok(()) => {}
        // Another launch is still opening its window: not a crash.
        Err(TryLockError::WouldBlock) => return none,
        Err(TryLockError::Error(e)) => {
            log::warn!(target: "rustudio::gpu", "launch marker: {e}");
            return none;
        }
    }
    let previous_never_drew = file.metadata().is_ok_and(|m| m.len() > 0);
    if file.set_len(0).and_then(|()| file.write_all(b"pending\n")).and_then(|()| file.flush()).is_err() {
        return Begun { previous_never_drew, marker: None };
    }
    Begun { previous_never_drew, marker: Some(Marker { file, path }) }
}

struct State {
    marker: Option<Marker>,
    notice: Option<String>,
    frames: u32,
}

static STATE: Mutex<State> = Mutex::new(State { marker: None, notice: None, frames: 0 });

/// Startup (lib.rs `run`, after `rustudio_settings::init`): for runs a person
/// drives. Switches Graphics API to OpenGL when the last launch never drew.
pub fn begin(persist: bool) {
    // (tests, harness, bench, headless and tool runs never leave a marker)
    if !persist || std::env::var_os("WGPU_BACKEND").is_some() || std::env::var_os("RUSTUDIO_PERF_GRAPHICS").is_some() {
        return;
    }
    let begun = begin_in(&crate::paths::config().join("rustudio"));
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.marker = begun.marker;
    if begun.previous_never_drew && cfg!(not(target_os = "macos")) && crate::rustudio_settings::performance_value("graphics_api") == "auto" {
        crate::rustudio_settings::set_choice("graphics_api", "gl", "auto");
        crate::rustudio_settings::flush();
        log::warn!(target: "rustudio::gpu", "the last launch never drew its window: switching Graphics API to OpenGL");
        st.notice = Some(
            "rustudio's last launch stopped before its window opened, so it now draws with OpenGL \
             (Settings > rustudio > Performance > Graphics API). Updating the graphics driver may let it use \
             Automatic again."
                .to_string(),
        );
    }
}

/// A frame of the window was drawn (every frame; the marker goes after a few).
pub fn frame_drawn() {
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if st.marker.is_none() {
        return;
    }
    st.frames += 1;
    if st.frames >= FRAMES_TO_DRAW {
        if let Some(m) = st.marker.take() {
            m.cleared();
        }
    }
}

/// This launch's window hasn't drawn yet (its marker is still held).
pub fn pending() -> bool {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).marker.is_some()
}

/// The message for a launch that was switched to OpenGL, once.
pub fn take_notice() -> Option<String> {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).notice.take()
}

/// The window couldn't open and said so cleanly (no usable adapter or device):
/// when it was on a native API the next launch will try OpenGL.
pub fn will_retry_with_opengl() -> bool {
    cfg!(not(target_os = "macos")) && crate::rustudio_settings::performance_value("graphics_api") == "auto"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rustudio-launch-guard-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A launch that draws leaves nothing behind; one that never draws makes the
    /// next report it, once.
    #[test]
    fn a_launch_that_never_drew_is_seen_by_the_next() {
        let dir = temp("never-drew");
        let first = begin_in(&dir);
        assert!(!first.previous_never_drew, "a first launch has no predecessor");
        let marker = first.marker.expect("marker");
        assert!(dir.join(MARKER).exists());
        marker.cleared();
        assert!(!dir.join(MARKER).exists());

        // A launch that crashes: its marker is dropped without `cleared`.
        drop(begin_in(&dir).marker);
        let next = begin_in(&dir);
        assert!(next.previous_never_drew);
        // That launch draws, so the one after sees nothing.
        next.marker.expect("marker").cleared();
        let after = begin_in(&dir);
        assert!(!after.previous_never_drew);
        after.marker.expect("marker").cleared();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A second copy opening while the first still holds its marker is not a
    /// crash, and must not take the first's marker.
    #[test]
    fn a_launch_still_opening_is_not_a_crash() {
        let dir = temp("second-copy");
        let first = begin_in(&dir).marker.expect("marker");
        let second = begin_in(&dir);
        assert!(!second.previous_never_drew);
        assert!(second.marker.is_none());
        // The first still counts: if it crashes, the next launch sees it.
        drop(first);
        assert!(begin_in(&dir).previous_never_drew);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A marker that can't be deleted (emptied first) is no marker.
    #[test]
    fn an_emptied_marker_is_not_a_crash() {
        let dir = temp("emptied");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MARKER), "").unwrap();
        assert!(!begin_in(&dir).previous_never_drew);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
