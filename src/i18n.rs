//! Interface language: Portuguese or English.
//!
//! Each text carries both versions side by side, right where it appears: `t!("Copiar", "Copy")` for
//! a fixed text and `tf!("Abrir {file}", "Open {file}")` for a text built with `format!`. This way
//! the compiler checks both formats, and there's no separate table that could fall out of date.

use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    #[serde(rename = "pt")]
    Portuguese,
    #[serde(rename = "en")]
    English,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::Portuguese, Lang::English];

    /// Name of the language written in itself, for the list in Preferences.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Portuguese => "Português",
            Lang::English => "English",
        }
    }

    /// Windows language: Portuguese if the system interface is in Portuguese, English otherwise.
    pub fn system() -> Lang {
        // SAFETY: a function with no arguments that only reads the user's settings.
        let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
        // The low 10 bits are the primary language; 0x16 is Portuguese (Brazil and Portugal).
        if id & 0x3ff == 0x16 { Lang::Portuguese } else { Lang::English }
    }
}

#[cfg_attr(test, allow(dead_code))]
static ENGLISH: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    // In tests, each thread has its own language: a test in English doesn't change the texts
    // checked by another one running at the same time.
    static TEST_ENGLISH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn set(lang: Lang) {
    #[cfg(test)]
    TEST_ENGLISH.set(lang == Lang::English);
    #[cfg(not(test))]
    ENGLISH.store(lang == Lang::English, std::sync::atomic::Ordering::Relaxed);
}

pub fn current() -> Lang {
    if english() { Lang::English } else { Lang::Portuguese }
}

pub fn english() -> bool {
    #[cfg(test)]
    return TEST_ENGLISH.get();
    #[cfg(not(test))]
    ENGLISH.load(std::sync::atomic::Ordering::Relaxed)
}

/// Fixed text in the current language: `t!("português", "english")`.
macro_rules! t {
    ($pt:literal, $en:literal $(,)?) => {
        if $crate::i18n::english() { $en } else { $pt }
    };
}

/// Text built in the current language, with the same arguments as `format!`.
macro_rules! tf {
    ($pt:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::english() { format!($en $(, $arg)*) } else { format!($pt $(, $arg)*) }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macros_follow_the_current_language() {
        let n = 3;
        set(Lang::English);
        assert_eq!(t!("Copiar", "Copy"), "Copy");
        assert_eq!(tf!("{n} passos", "{n} steps"), "3 steps");
        assert_eq!(tf!("{} de {}", "{} of {}", 1, 2), "1 of 2");
        set(Lang::Portuguese);
        assert_eq!(t!("Copiar", "Copy"), "Copiar");
        assert_eq!(tf!("{n} passos", "{n} steps"), "3 passos");
        assert_eq!(current(), Lang::Portuguese);
    }

    #[test]
    fn language_is_saved_as_a_short_code() {
        assert_eq!(serde_json::to_string(&Lang::English).unwrap(), "\"en\"");
        assert_eq!(serde_json::from_str::<Lang>("\"pt\"").unwrap(), Lang::Portuguese);
    }
}
