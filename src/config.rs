//! User preferences, saved to `%APPDATA%\SnapKit\config.json`.

use std::path::PathBuf;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use serde::{Deserialize, Serialize};

/// Keys accepted for the global shortcut, in the order they appear in the list.
pub const KEYS: [&str; 49] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "F1",
    "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "PrintScreen",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyCfg {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: String,
}

impl Default for HotkeyCfg {
    fn default() -> Self {
        Self { ctrl: false, alt: true, shift: true, win: false, key: "S".to_owned() }
    }
}

impl HotkeyCfg {
    /// Converts to the system hotkey. `None` if the key is unknown.
    pub fn to_hotkey(&self) -> Option<HotKey> {
        let mut mods = Modifiers::empty();
        mods.set(Modifiers::CONTROL, self.ctrl);
        mods.set(Modifiers::ALT, self.alt);
        mods.set(Modifiers::SHIFT, self.shift);
        mods.set(Modifiers::META, self.win);
        Some(HotKey::new(Some(mods), key_code(&self.key)?))
    }

    /// Text to show the user, such as "Alt+Shift+S".
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        for (on, name) in [(self.ctrl, "Ctrl"), (self.alt, "Alt"), (self.shift, "Shift"), (self.win, "Win")] {
            if on {
                parts.push(name);
            }
        }
        parts.push(&self.key);
        parts.join("+")
    }

    /// A single letter or digit as a global shortcut would steal normal typing.
    pub fn is_safe(&self) -> bool {
        let has_modifier = self.ctrl || self.alt || self.win;
        has_modifier || self.key.starts_with('F') && self.key.len() > 1 || self.key == "PrintScreen"
    }

    /// Returns `true` if `self` and `other` bind the exact same modifier + key combination.
    pub fn conflicts_with(&self, other: &HotkeyCfg) -> bool {
        self.ctrl == other.ctrl
            && self.alt == other.alt
            && self.shift == other.shift
            && self.win == other.win
            && self.key == other.key
    }
}

/// Capture modes that can have their own optional global shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CaptureShortcutMode {
    FullScreen,
    AllScreens,
    Scrolling,
    Panoramic,
    Steps,
    Video,
}

/// Optional per-mode global shortcuts, in addition to the primary `hotkey`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeHotkeysCfg {
    pub full_screen: Option<HotkeyCfg>,
    pub all_screens: Option<HotkeyCfg>,
    pub scrolling: Option<HotkeyCfg>,
    pub panoramic: Option<HotkeyCfg>,
    pub steps: Option<HotkeyCfg>,
    pub video: Option<HotkeyCfg>,
}

impl ModeHotkeysCfg {
    pub fn get(&self, mode: CaptureShortcutMode) -> Option<&HotkeyCfg> {
        match mode {
            CaptureShortcutMode::FullScreen => self.full_screen.as_ref(),
            CaptureShortcutMode::AllScreens => self.all_screens.as_ref(),
            CaptureShortcutMode::Scrolling => self.scrolling.as_ref(),
            CaptureShortcutMode::Panoramic => self.panoramic.as_ref(),
            CaptureShortcutMode::Steps => self.steps.as_ref(),
            CaptureShortcutMode::Video => self.video.as_ref(),
        }
    }

    pub fn get_mut(&mut self, mode: CaptureShortcutMode) -> &mut Option<HotkeyCfg> {
        match mode {
            CaptureShortcutMode::FullScreen => &mut self.full_screen,
            CaptureShortcutMode::AllScreens => &mut self.all_screens,
            CaptureShortcutMode::Scrolling => &mut self.scrolling,
            CaptureShortcutMode::Panoramic => &mut self.panoramic,
            CaptureShortcutMode::Steps => &mut self.steps,
            CaptureShortcutMode::Video => &mut self.video,
        }
    }

    pub fn set(&mut self, mode: CaptureShortcutMode, val: Option<HotkeyCfg>) {
        *self.get_mut(mode) = val;
    }

    pub fn all(&self) -> [(CaptureShortcutMode, Option<&HotkeyCfg>); 6] {
        [
            (CaptureShortcutMode::FullScreen, self.full_screen.as_ref()),
            (CaptureShortcutMode::AllScreens, self.all_screens.as_ref()),
            (CaptureShortcutMode::Scrolling, self.scrolling.as_ref()),
            (CaptureShortcutMode::Panoramic, self.panoramic.as_ref()),
            (CaptureShortcutMode::Steps, self.steps.as_ref()),
            (CaptureShortcutMode::Video, self.video.as_ref()),
        ]
    }
}

fn key_code(key: &str) -> Option<Code> {
    use Code::*;
    const CODES: [Code; 49] = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO,
        KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ, Digit0, Digit1, Digit2,
        Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9, F1, F2, F3, F4, F5, F6, F7, F8, F9,
        F10, F11, F12, PrintScreen,
    ];
    KEYS.iter().position(|k| *k == key).map(|i| CODES[i])
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkey: HotkeyCfg,
    #[serde(default)]
    pub mode_hotkeys: ModeHotkeysCfg,
    pub delay_secs: u32,
    pub include_cursor: bool,
    /// Closing the window only hides it; the program stays in the tray.
    pub close_to_tray: bool,
    /// Audio sources checked in the last video recording.
    pub record_system_audio: bool,
    pub record_microphone: bool,
    /// After choosing the region, annotate right there, on the screen, instead of opening the
    /// editor.
    pub quick_annotate: bool,
    /// Styles saved in the editor (tool + options).
    pub styles: Vec<crate::editor::Style>,
    /// Name of the interface color theme; see `theme::THEMES`.
    pub theme: String,
    /// Interface language; with no choice, follows the Windows one.
    pub language: Option<crate::i18n::Lang>,
    /// Look for a new version on GitHub once a day. Off by default: SnapKit uses the internet
    /// only when the user asks.
    pub check_updates: bool,
    /// When updates were last checked (seconds since 1970), so the daily check really is daily.
    pub last_update_check: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: HotkeyCfg::default(),
            mode_hotkeys: ModeHotkeysCfg::default(),
            delay_secs: 0,
            include_cursor: false,
            close_to_tray: true,
            record_system_audio: true,
            record_microphone: false,
            quick_annotate: true,
            styles: Vec::new(),
            theme: "Padrão".to_owned(),
            language: None,
            check_updates: false,
            last_update_check: 0,
        }
    }
}

impl Config {
    fn path() -> Option<PathBuf> {
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join("SnapKit").join("config.json"))
    }

    /// Loads the preferences; a missing file becomes the default. A file that can't be read is
    /// moved aside to `config.json.bad` first, so the next save doesn't destroy the styles in it.
    pub fn load() -> Self {
        let Some(path) = Self::path() else { return Self::default() };
        Self::load_from(&path)
    }

    fn load_from(path: &std::path::Path) -> Self {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                let _ = std::fs::rename(path, path.with_extension("json.bad"));
                crate::log_error(&format!("config.json could not be read ({e}); it was kept as config.json.bad"));
                return Self::default();
            }
        };
        match std::str::from_utf8(&bytes)
            .map_err(|e| e.to_string())
            .and_then(|text| Self::try_parse(text).map_err(|e| e.to_string()))
        {
            Ok(cfg) => cfg,
            Err(e) => {
                let _ = std::fs::rename(path, path.with_extension("json.bad"));
                crate::log_error(&format!("config.json could not be read ({e}); it was kept as config.json.bad"));
                Self::default()
            }
        }
    }

    fn try_parse(text: &str) -> Result<Self, serde_json::Error> {
        // Notepad and PowerShell may save with a byte order mark, which JSON doesn't allow.
        serde_json::from_str(text.trim_start_matches('\u{feff}'))
    }

    #[cfg(test)]
    fn parse(text: &str) -> Self {
        Self::try_parse(text).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or(t!("pasta APPDATA não encontrada", "APPDATA folder not found"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        write_atomic(&path, text.as_bytes()).map_err(|e| e.to_string())
    }
}

/// Writes the whole file or nothing: first to a temporary file beside it, flushed to disk, then
/// renamed over the old one. A write cut short (SnapKit closed or the computer turned off in the
/// middle) leaves the old file intact instead of half a file.
pub fn write_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let mut file = std::fs::File::create(&tmp)?;
    if let Err(e) = file.write_all(data).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    drop(file);
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_key_maps_to_a_code() {
        for key in KEYS {
            assert!(key_code(key).is_some(), "{key}");
        }
        assert!(key_code("Ç").is_none());
    }

    #[test]
    fn label_lists_modifiers_in_order() {
        let h = HotkeyCfg { ctrl: true, alt: false, shift: true, win: true, key: "F5".into() };
        assert_eq!(h.label(), "Ctrl+Shift+Win+F5");
        assert_eq!(HotkeyCfg::default().label(), "Alt+Shift+S");
    }

    #[test]
    fn bare_letters_are_rejected_as_unsafe() {
        let mut h = HotkeyCfg { ctrl: false, alt: false, shift: false, win: false, key: "F".into() };
        assert!(!h.is_safe(), "letra F sozinha");
        h.shift = true;
        assert!(!h.is_safe(), "Shift+F is normal typing");
        h.key = "F9".into();
        assert!(h.is_safe());
        h.key = "PrintScreen".into();
        h.shift = false;
        assert!(h.is_safe());
    }

    #[test]
    fn parse_tolerates_garbage_and_missing_fields() {
        assert_eq!(Config::parse("this is not json"), Config::default());
        let partial = Config::parse(r#"{"delay_secs": 5, "hotkey": {"key": "X"}}"#);
        assert_eq!(partial.delay_secs, 5);
        assert_eq!(partial.hotkey.key, "X");
        assert!(partial.hotkey.alt, "missing fields use the default");
        assert!(partial.close_to_tray);
    }

    #[test]
    fn the_update_check_is_off_unless_turned_on() {
        assert!(!Config::default().check_updates);
        assert!(!Config::parse("{}").check_updates, "an older config.json leaves it off");
    }

    #[test]
    fn a_byte_order_mark_doesnt_reset_the_preferences() {
        let c = Config { include_cursor: true, ..Config::default() };
        let text = format!("\u{feff}{}", serde_json::to_string(&c).unwrap());
        assert_eq!(Config::try_parse(&text).unwrap(), c);
    }

    #[test]
    fn atomic_write_replaces_the_file_and_leaves_no_temporary() {
        let dir = std::env::temp_dir().join(format!("snapkit-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.json");
        write_atomic(&file, b"one").unwrap();
        write_atomic(&file, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two");
        assert!(!dir.join("a.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_utf8_config_is_backed_up_to_bad() {
        let dir = std::env::temp_dir().join(format!("snapkit-cfg-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        std::fs::write(&file, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(Config::load_from(&file), Config::default());
        assert!(dir.join("config.json.bad").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_round_trips_through_json() {
        let c = Config {
            hotkey: HotkeyCfg { key: "PrintScreen".into(), ..HotkeyCfg::default() },
            include_cursor: true,
            ..Config::default()
        };
        let text = serde_json::to_string(&c).unwrap();
        assert_eq!(Config::parse(&text), c);
    }

    #[test]
    fn parse_without_mode_hotkeys_defaults_to_none() {
        let cfg = Config::parse(r#"{"delay_secs": 3, "hotkey": {"key": "A"}}"#);
        assert_eq!(cfg.delay_secs, 3);
        assert_eq!(cfg.mode_hotkeys, ModeHotkeysCfg::default());
        for (_, hk) in cfg.mode_hotkeys.all() {
            assert!(hk.is_none());
        }
    }

    #[test]
    fn mode_hotkeys_round_trip_and_helpers() {
        let mut modes = ModeHotkeysCfg::default();
        let fs = HotkeyCfg { ctrl: true, alt: true, shift: false, win: false, key: "F1".into() };
        let vid = HotkeyCfg { ctrl: true, alt: false, shift: true, win: false, key: "R".into() };
        modes.set(CaptureShortcutMode::FullScreen, Some(fs.clone()));
        *modes.get_mut(CaptureShortcutMode::Video) = Some(vid.clone());

        assert_eq!(modes.get(CaptureShortcutMode::FullScreen), Some(&fs));
        assert_eq!(modes.get(CaptureShortcutMode::Video), Some(&vid));
        assert_eq!(modes.get(CaptureShortcutMode::Scrolling), None);

        let cfg = Config { mode_hotkeys: modes, ..Config::default() };
        let text = serde_json::to_string(&cfg).unwrap();
        let parsed = Config::try_parse(&text).unwrap();
        assert_eq!(parsed, cfg);
        assert_eq!(parsed.mode_hotkeys.all().len(), 6);
    }

    #[test]
    fn hotkey_conflicts_with_checks_all_fields() {
        let base = HotkeyCfg { ctrl: true, alt: true, shift: false, win: false, key: "S".into() };
        let same = HotkeyCfg { ctrl: true, alt: true, shift: false, win: false, key: "S".into() };
        let diff_key = HotkeyCfg { key: "X".into(), ..base.clone() };
        let diff_shift = HotkeyCfg { shift: true, ..base.clone() };
        let diff_win = HotkeyCfg { win: true, ..base.clone() };
        assert!(base.conflicts_with(&same));
        assert!(!base.conflicts_with(&diff_key));
        assert!(!base.conflicts_with(&diff_shift));
        assert!(!base.conflicts_with(&diff_win));
    }
}
