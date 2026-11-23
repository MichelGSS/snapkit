//! "Start with the system", for the current user only: an entry in `HKCU\...\Run` on Windows, a
//! LaunchAgent in `~/Library/LaunchAgents` on macOS.

/// Argument that makes the program open straight into the tray.
pub const MINIMIZED_ARG: &str = "--minimized";

#[cfg(windows)]
mod imp {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "SnapKit";

    pub fn is_enabled() -> bool {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, KEY_READ)
            .and_then(|k| k.get_value::<String, _>(VALUE_NAME))
            .is_ok()
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if enabled {
            let (key, _) = hkcu.create_subkey(RUN_KEY).map_err(|e| e.to_string())?;
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            key.set_value(VALUE_NAME, &command_line(&exe.to_string_lossy()))
                .map_err(|e| e.to_string())
        } else {
            let key = match hkcu.open_subkey_with_flags(RUN_KEY, KEY_WRITE) {
                Ok(key) => key,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e.to_string()),
            };
            match key.delete_value(VALUE_NAME) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            }
        }
    }

    /// Command line written to the registry; quotes protect paths with spaces.
    pub(super) fn command_line(exe: &str) -> String {
        format!("\"{exe}\" {}", super::MINIMIZED_ARG)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::PathBuf;

    /// Identifier of the LaunchAgent, the same as the app bundle's.
    pub const LABEL: &str = "io.github.michelgss.snapkit";

    fn plist_path() -> PathBuf {
        crate::platform::home_dir().join("Library").join("LaunchAgents").join(format!("{LABEL}.plist"))
    }

    pub fn is_enabled() -> bool {
        plist_path().is_file()
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let path = plist_path();
        if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, plist(&exe.to_string_lossy())).map_err(|e| e.to_string())
        } else {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            }
        }
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    }

    /// The LaunchAgent: runs SnapKit once at login, straight into the menu bar.
    pub(super) fn plist(exe: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#,
            xml_escape(exe),
            super::MINIMIZED_ARG
        )
    }
}

pub use imp::{is_enabled, set_enabled};

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn command_line_quotes_paths_with_spaces() {
        assert_eq!(
            super::imp::command_line(r"C:\Program Files\SnapKit\snapkit.exe"),
            r#""C:\Program Files\SnapKit\snapkit.exe" --minimized"#
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launch_agent_runs_the_app_minimized() {
        let plist = super::imp::plist("/Applications/Snap & Kit.app/Contents/MacOS/snapkit");
        assert!(plist.contains("<string>/Applications/Snap &amp; Kit.app/Contents/MacOS/snapkit</string>"));
        assert!(plist.contains("<string>--minimized</string>"));
        assert!(plist.contains(super::imp::LABEL));
    }
}
