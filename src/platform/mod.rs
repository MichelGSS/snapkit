//! Everything that talks to the operating system: cursor and keyboard state, the window list,
//! placement of our window, the single-instance signals, and where files live.
//!
//! The rest of SnapKit sees one API, implemented in `windows.rs` and `macos.rs`. Every coordinate
//! is in "screen pixels" with the origin at the top-left corner of the main monitor:
//! - on Windows, the physical pixels of the virtual desktop (the process is per-monitor DPI aware);
//! - on macOS, the global display points multiplied by the largest Retina factor among the
//!   connected displays (see `macos::scale`), so a capture of a Retina screen keeps every pixel.

use std::path::PathBuf;
use std::process::Command;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

/// Rectangle in pixels: (left, top, right, bottom).
pub type PxRect = (i32, i32, i32, i32);

/// What a second copy of SnapKit asks the one already running to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InstanceRequest {
    /// Show its window.
    Show,
    /// Start a region capture.
    Select,
    /// Close cleanly (used by the installer and uninstaller).
    Quit,
}

/// The user's home folder.
pub fn home_dir() -> PathBuf {
    #[cfg(windows)]
    let var = "USERPROFILE";
    #[cfg(not(windows))]
    let var = "HOME";
    std::env::var_os(var).map(PathBuf::from).unwrap_or_default()
}

/// Folder of the per-user application data: `%APPDATA%` on Windows,
/// `~/Library/Application Support` on macOS.
pub fn app_data_root() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    }
}

/// SnapKit's own data folder (preferences, error log).
pub fn data_dir() -> Option<PathBuf> {
    app_data_root().map(|d| d.join("SnapKit"))
}

/// The user's pictures folder (the default library lives in `SnapKit` inside it).
pub fn pictures_dir() -> PathBuf {
    home_dir().join("Pictures")
}

/// The user's videos folder: `Videos` on Windows, `Movies` on macOS.
pub fn movies_dir() -> PathBuf {
    #[cfg(windows)]
    let name = "Videos";
    #[cfg(not(windows))]
    let name = "Movies";
    home_dir().join(name)
}

/// File name of a bundled helper program (`ffmpeg` becomes `ffmpeg.exe` on Windows).
pub fn exe_name(base: &str) -> String {
    format!("{base}{}", std::env::consts::EXE_SUFFIX)
}

/// A command for a helper program that runs in the background, without a console window.
pub fn background_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Opens a file, a folder or a web page with its default program.
pub fn open(target: impl AsRef<std::ffi::OsStr>) {
    #[cfg(windows)]
    let opener = "explorer";
    #[cfg(not(windows))]
    let opener = "open";
    let _ = Command::new(opener).arg(target).spawn();
}

/// Shows the file selected in its folder (Explorer, Finder).
pub fn reveal(path: &std::path::Path) {
    #[cfg(windows)]
    let _ = Command::new("explorer").arg("/select,").arg(path).spawn();
    #[cfg(not(windows))]
    let _ = Command::new("open").arg("-R").arg(path).spawn();
}

/// Text that identifies the preferences in use, for the single-instance key: `%APPDATA%` on
/// Windows, the home folder on macOS.
pub fn profile_id() -> String {
    app_data_root().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_names_and_folders() {
        assert!(exe_name("ffmpeg").starts_with("ffmpeg"));
        assert_eq!(cfg!(windows), exe_name("ffmpeg").ends_with(".exe"));
        assert!(movies_dir().starts_with(home_dir()));
        assert!(data_dir().is_none_or(|d| d.ends_with("SnapKit")));
    }
}
