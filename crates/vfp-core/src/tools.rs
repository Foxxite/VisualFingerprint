//! Where the FFmpeg programs are.
//!
//! By default the core runs `ffmpeg` and `ffprobe` from PATH. A host can set
//! `VFP_FFMPEG` and `VFP_FFPROBE` to full paths. The Android app does this,
//! because it ships the programs inside the APK.

use std::ffi::OsString;
use std::process::Command;

fn program(var: &str, default: &str) -> OsString {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => v,
        _ => OsString::from(default),
    }
}

/// A command that runs FFmpeg.
pub fn ffmpeg() -> Command {
    quiet(Command::new(program("VFP_FFMPEG", "ffmpeg")))
}

/// A command that runs FFprobe.
pub fn ffprobe() -> Command {
    quiet(Command::new(program("VFP_FFPROBE", "ffprobe")))
}

/// On Windows, a program started from a window app would open a console window. This stops that.
#[cfg(windows)]
fn quiet(mut c: Command) -> Command {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}
#[cfg(not(windows))]
fn quiet(c: Command) -> Command {
    c
}

/// The video encoder name. Default `libx264`. The Android build has no x264, so the app sets
/// `VFP_VIDEO_ENCODER=mpeg4`.
pub fn video_encoder() -> String {
    std::env::var("VFP_VIDEO_ENCODER").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "libx264".into())
}
