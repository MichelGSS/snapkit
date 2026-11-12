//! Migration from the program's old name.
//!
//! During development SnapKit was called "Recorte". Anyone who used a version with that name has
//! preferences, a capture library and recordings in `Recorte` folders. On the first run with the
//! new name, those folders are renamed (nothing is copied or deleted) and the "start with Windows"
//! entry is recreated.

use std::path::{Path, PathBuf};

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

const OLD_NAME: &str = "Recorte";
const NEW_NAME: &str = "SnapKit";

/// Renames `parent\Recorte` to `parent\SnapKit` if the old one exists and the new one doesn't.
/// Returns `true` if it renamed.
fn migrate_dir(parent: &Path) -> bool {
    let (old, new) = (parent.join(OLD_NAME), parent.join(NEW_NAME));
    // If both exist, nothing is touched: merging folders could overwrite files.
    old.is_dir() && !new.exists() && std::fs::rename(&old, &new).is_ok()
}

/// Must be called once, before reading preferences or opening the library.
pub fn migrate() {
    let env_dir = |var: &str| std::env::var_os(var).map(PathBuf::from);
    if let Some(appdata) = env_dir("APPDATA") {
        migrate_dir(&appdata);
    }
    if let Some(home) = env_dir("USERPROFILE") {
        migrate_dir(&home.join("Pictures"));
        migrate_dir(&home.join("Videos"));
    }

    // "Start with Windows": the old entry pointed to the old executable.
    let run = r"Software\Microsoft\Windows\CurrentVersion\Run";
    if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(run, KEY_READ | KEY_WRITE) {
        if key.get_value::<String, _>(OLD_NAME).is_ok() && key.delete_value(OLD_NAME).is_ok() {
            let _ = crate::autostart::set_enabled(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("snapkit-test-legacy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn old_folder_is_renamed_with_its_contents() {
        let parent = temp("rename");
        std::fs::create_dir_all(parent.join(OLD_NAME)).unwrap();
        std::fs::write(parent.join(OLD_NAME).join("config.json"), b"{}").unwrap();
        assert!(migrate_dir(&parent));
        assert!(!parent.join(OLD_NAME).exists());
        assert!(parent.join(NEW_NAME).join("config.json").is_file());
        assert!(!migrate_dir(&parent), "the second call has nothing to do");
    }

    #[test]
    fn existing_new_folder_is_never_overwritten() {
        let parent = temp("conflict");
        for (name, content) in [(OLD_NAME, "old"), (NEW_NAME, "new")] {
            std::fs::create_dir_all(parent.join(name)).unwrap();
            std::fs::write(parent.join(name).join("a.txt"), content).unwrap();
        }
        assert!(!migrate_dir(&parent));
        assert_eq!(std::fs::read_to_string(parent.join(NEW_NAME).join("a.txt")).unwrap(), "new");
        assert!(parent.join(OLD_NAME).join("a.txt").is_file(), "the old folder stays intact");
    }

    #[test]
    fn nothing_happens_without_an_old_folder() {
        assert!(!migrate_dir(&temp("empty")));
    }
}
