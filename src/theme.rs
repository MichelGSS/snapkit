//! Interface color themes, including a few famous ones from code editors.
//!
//! Each theme picks the panel background and the accent color (selection, hyperlinks, the frame of
//! the captured area and the control points in the editor). The chosen name is saved in the
//! preferences, in `%APPDATA%\SnapKit\config.json`.

use eframe::egui;
use egui::Color32;

#[derive(Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub dark: bool,
    /// Background color of the panels and the window.
    pub panel_fill: Color32,
    /// Accent color: selection, hyperlinks, capture frame.
    pub accent: Color32,
}

/// The accent color of the original theme, also the default.
pub const ACCENT: Color32 = Color32::from_rgb(255, 138, 76);

/// Available themes, in the order they appear in the preferences.
pub const THEMES: [Theme; 9] = [
    Theme { name: "Padrão", dark: true, panel_fill: Color32::from_rgb(22, 24, 29), accent: ACCENT },
    // Purple from the Dracula palette.
    Theme { name: "Dracula", dark: true, panel_fill: Color32::from_rgb(40, 42, 54), accent: Color32::from_rgb(189, 147, 249) },
    // Ice blue from Nord ("frost").
    Theme { name: "Nord", dark: true, panel_fill: Color32::from_rgb(46, 52, 64), accent: Color32::from_rgb(136, 192, 208) },
    Theme { name: "One Dark", dark: true, panel_fill: Color32::from_rgb(40, 44, 52), accent: Color32::from_rgb(97, 175, 239) },
    // Pink from the Monokai keywords.
    Theme { name: "Monokai", dark: true, panel_fill: Color32::from_rgb(39, 40, 34), accent: Color32::from_rgb(249, 38, 114) },
    // Orange from the Gruvbox accents.
    Theme { name: "Gruvbox Escuro", dark: true, panel_fill: Color32::from_rgb(40, 40, 40), accent: Color32::from_rgb(254, 128, 25) },
    // Blue from Tokyo Night.
    Theme { name: "Tokyo Noite", dark: true, panel_fill: Color32::from_rgb(26, 27, 38), accent: Color32::from_rgb(122, 162, 247) },
    // Mauve from Catppuccin Mocha.
    Theme { name: "Catppuccin Mocha", dark: true, panel_fill: Color32::from_rgb(30, 30, 46), accent: Color32::from_rgb(203, 166, 247) },
    Theme { name: "Solarized Claro", dark: false, panel_fill: Color32::from_rgb(253, 246, 227), accent: Color32::from_rgb(38, 139, 210) },
];

/// The theme with that name; an unknown name falls back to the default ("Padrão").
pub fn by_name(name: &str) -> &'static Theme {
    THEMES.iter().find(|t| t.name == name).unwrap_or(&THEMES[0])
}

/// Theme name to display, in the interface language. The name saved in the preferences is always
/// the Portuguese one, so the choice holds in both languages.
pub fn display_name(name: &str) -> &str {
    match name {
        "Padrão" => t!("Padrão", "Default"),
        "Gruvbox Escuro" => t!("Gruvbox Escuro", "Gruvbox Dark"),
        "Tokyo Noite" => t!("Tokyo Noite", "Tokyo Night"),
        "Solarized Claro" => t!("Solarized Claro", "Solarized Light"),
        other => other,
    }
}

/// Readable text color on a `fill` background: near black on light backgrounds, white on dark ones.
pub fn text_on(fill: Color32) -> Color32 {
    let luma = (fill.r() as u32 * 3 + fill.g() as u32 * 6 + fill.b() as u32) / 10;
    if luma > 140 { Color32::from_rgb(20, 20, 24) } else { Color32::WHITE }
}

/// Applies the theme to the egui context.
pub fn apply(ctx: &egui::Context, theme: &Theme) {
    let mut visuals = if theme.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    visuals.panel_fill = theme.panel_fill;
    visuals.selection.bg_fill = theme.accent;
    // The selected item's text sits on the accent color: dark on light accents.
    visuals.selection.stroke = egui::Stroke::new(1.0, text_on(theme.accent));
    visuals.hyperlink_color = theme.accent;
    ctx.set_visuals(visuals);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = THEMES.iter().map(|t| t.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), THEMES.len());
    }

    #[test]
    fn unknown_name_falls_back_to_default() {
        assert_eq!(by_name("Padrão").name, "Padrão");
        assert_eq!(by_name("this does not exist"), by_name("Padrão"));
    }

    #[test]
    fn selected_text_contrasts_with_every_accent() {
        // Minimum brightness difference between the selected item's text and the accent color.
        let luma = |c: Color32| (c.r() as i32 * 3 + c.g() as i32 * 6 + c.b() as i32) / 10;
        for theme in &THEMES {
            let gap = (luma(text_on(theme.accent)) - luma(theme.accent)).abs();
            assert!(gap >= 100, "{}: texto pouco legível sobre o destaque ({gap})", theme.name);
        }
        assert_eq!(text_on(Color32::from_rgb(136, 192, 208)), Color32::from_rgb(20, 20, 24), "ice blue calls for dark text");
        assert_eq!(text_on(Color32::from_rgb(38, 139, 210)), Color32::WHITE);
    }

    #[test]
    fn every_theme_applies_without_panicking() {
        let ctx = egui::Context::default();
        for theme in &THEMES {
            apply(&ctx, theme);
        }
    }
}
