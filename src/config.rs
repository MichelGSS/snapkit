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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: HotkeyCfg::default(),
            delay_secs: 0,
            include_cursor: false,
            close_to_tray: true,
            record_system_audio: true,
            record_microphone: false,
            quick_annotate: true,
            styles: Vec::new(),
            theme: "Padrão".to_owned(),
            language: None,
        }
    }
}

impl Config {
    fn path() -> Option<PathBuf> {
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join("SnapKit").join("config.json"))
    }

    /// Loads the preferences; a missing or invalid file becomes the default.
    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or(t!("pasta APPDATA não encontrada", "APPDATA folder not found"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())
    }
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
    fn config_round_trips_through_json() {
        let mut c = Config::default();
        c.hotkey.key = "PrintScreen".into();
        c.include_cursor = true;
        let text = serde_json::to_string(&c).unwrap();
        assert_eq!(Config::parse(&text), c);
    }
}
