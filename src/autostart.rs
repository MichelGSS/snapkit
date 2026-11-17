//! "Start with Windows": an entry in `HKCU\...\Run`, for the current user only.

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "SnapKit";
/// Argument that makes the program open straight into the tray.
pub const MINIMIZED_ARG: &str = "--minimized";

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
fn command_line(exe: &str) -> String {
    format!("\"{exe}\" {MINIMIZED_ARG}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_quotes_paths_with_spaces() {
        assert_eq!(
            command_line(r"C:\Program Files\SnapKit\snapkit.exe"),
            r#""C:\Program Files\SnapKit\snapkit.exe" --minimized"#
        );
    }
}
