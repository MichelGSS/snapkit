//! Interface color themes, including a few famous ones from code editors.
//!
//! Each theme picks the panel background and the accent color (selection, hyperlinks, the frame of
//! the captured area and the control points in the editor). The chosen name is saved in the
//! preferences, in `%APPDATA%\SnapKit\config.json`.

use std::sync::Arc;

use eframe::egui;
use egui::{Color32, FontFamily, FontId, Margin, Stroke, TextStyle};

pub const SEMIBOLD_FAMILY: &str = "semibold";

const INTER_REGULAR_TTF: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
const INTER_SEMIBOLD_TTF: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");

pub fn semibold_family() -> FontFamily {
    FontFamily::Name(SEMIBOLD_FAMILY.into())
}

/// Installs the embedded Inter Regular and SemiBold fonts while keeping egui's monospace and
/// symbol/emoji fallbacks intact.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter_regular".to_owned(),
        Arc::new(egui::FontData::from_static(INTER_REGULAR_TTF)),
    );
    fonts.font_data.insert(
        "inter_semibold".to_owned(),
        Arc::new(egui::FontData::from_static(INTER_SEMIBOLD_TTF)),
    );

    let prop_fallbacks = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();

    let mut regular_chain = Vec::with_capacity(1 + prop_fallbacks.len());
    regular_chain.push("inter_regular".to_owned());
    regular_chain.extend(prop_fallbacks.iter().cloned());
    fonts.families.insert(FontFamily::Proportional, regular_chain);

    let mut semibold_chain = Vec::with_capacity(2 + prop_fallbacks.len());
    semibold_chain.push("inter_semibold".to_owned());
    semibold_chain.push("inter_regular".to_owned());
    semibold_chain.extend(prop_fallbacks);
    fonts.families.insert(semibold_family(), semibold_chain);

    ctx.set_fonts(fonts);
}

#[derive(Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub dark: bool,
    /// Background color of the panels and the window.
    pub panel_fill: Color32,
    /// Accent color: selection, hyperlinks, capture frame.
    pub accent: Color32,
}

/// Derived 4-surface design palette for the active theme.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub accent: Color32,
    pub on_accent: Color32,
    pub bg_canvas: Color32,
    pub bg_chrome: Color32,
    pub bg_surface: Color32,
    pub bg_elevated: Color32,
    pub border_subtle: Color32,
    pub border_strong: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub shadow_color: Color32,
}

fn shift_rgb(c: Color32, delta: i16) -> Color32 {
    let clamp = |v: u8| (v as i16 + delta).clamp(0, 255) as u8;
    Color32::from_rgb(clamp(c.r()), clamp(c.g()), clamp(c.b()))
}

/// Computes the 4-layer surface palette for `theme`.
pub fn palette(theme: &Theme) -> Palette {
    let on_accent = text_on(theme.accent);
    if theme.dark {
        Palette {
            dark: true,
            accent: theme.accent,
            on_accent,
            bg_canvas: shift_rgb(theme.panel_fill, -8),
            bg_chrome: theme.panel_fill,
            bg_surface: shift_rgb(theme.panel_fill, 9),
            bg_elevated: shift_rgb(theme.panel_fill, 18),
            border_subtle: Color32::from_white_alpha(18),
            border_strong: Color32::from_white_alpha(38),
            text_primary: Color32::from_rgb(236, 239, 244),
            text_secondary: Color32::from_rgb(168, 176, 190),
            text_muted: Color32::from_rgb(118, 126, 140),
            danger: Color32::from_rgb(235, 87, 87),
            success: Color32::from_rgb(63, 185, 80),
            warning: Color32::from_rgb(235, 179, 54),
            shadow_color: Color32::from_black_alpha(120),
        }
    } else {
        Palette {
            dark: false,
            accent: theme.accent,
            on_accent,
            bg_canvas: shift_rgb(theme.panel_fill, -14),
            bg_chrome: theme.panel_fill,
            bg_surface: shift_rgb(theme.panel_fill, -7),
            bg_elevated: shift_rgb(theme.panel_fill, -16),
            border_subtle: Color32::from_black_alpha(26),
            border_strong: Color32::from_black_alpha(52),
            text_primary: Color32::from_rgb(28, 34, 42),
            text_secondary: Color32::from_rgb(86, 98, 108),
            text_muted: Color32::from_rgb(128, 138, 146),
            danger: Color32::from_rgb(214, 65, 65),
            success: Color32::from_rgb(38, 150, 62),
            warning: Color32::from_rgb(196, 134, 18),
            shadow_color: Color32::from_black_alpha(45),
        }
    }
}

/// Returns the active `Palette` stored in `ctx`, falling back to the default theme's palette.
pub fn current_palette(ctx: &egui::Context) -> Palette {
    ctx.data(|d| d.get_temp::<Palette>(egui::Id::unique("snapkit_palette")))
        .unwrap_or_else(|| palette(&THEMES[0]))
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
    let pal = palette(theme);
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("snapkit_palette"), pal));

    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Heading, FontId::new(18.0, semibold_family())),
        (TextStyle::Body, FontId::proportional(13.0)),
        (TextStyle::Button, FontId::proportional(12.5)),
        (TextStyle::Small, FontId::proportional(11.0)),
        (TextStyle::Monospace, FontId::monospace(12.0)),
    ]
    .into();

    style.spacing.button_padding = egui::vec2(9.0, 5.0);
    style.spacing.item_spacing = egui::vec2(6.0, 5.0);
    style.spacing.window_margin = Margin::same(14);
    style.spacing.menu_margin = Margin::same(6);
    style.spacing.slider_width = 112.0;
    style.spacing.interact_size = egui::vec2(30.0, 26.0);

    let mut visuals = if theme.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    visuals.panel_fill = pal.bg_chrome;
    visuals.window_fill = pal.bg_surface;
    visuals.faint_bg_color = pal.bg_surface;
    visuals.extreme_bg_color = pal.bg_canvas;
    visuals.code_bg_color = pal.bg_canvas;
    visuals.window_stroke = Stroke::new(1.0, pal.border_subtle);
    visuals.window_corner_radius = egui::CornerRadius::same(10);
    visuals.menu_corner_radius = egui::CornerRadius::same(8);
    visuals.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: pal.shadow_color,
    };
    visuals.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: pal.shadow_color,
    };
    visuals.selection.bg_fill = pal.accent;
    // The selected item's text sits on the accent color: dark on light accents.
    visuals.selection.stroke = Stroke::new(1.0, pal.on_accent);
    visuals.hyperlink_color = pal.accent;

    let radius = egui::CornerRadius::same(6);
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.corner_radius = radius;
        w.expansion = 0.0;
    }

    visuals.widgets.noninteractive.bg_fill = pal.bg_surface;
    visuals.widgets.noninteractive.weak_bg_fill = pal.bg_surface;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, pal.border_subtle);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, pal.text_primary);

    visuals.widgets.inactive.bg_fill = pal.bg_surface;
    visuals.widgets.inactive.weak_bg_fill = pal.bg_surface;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, pal.border_subtle);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, pal.text_primary);

    visuals.widgets.hovered.bg_fill = pal.bg_elevated;
    visuals.widgets.hovered.weak_bg_fill = pal.bg_elevated;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, pal.border_strong);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, pal.text_primary);

    visuals.widgets.active.bg_fill = pal.accent.gamma_multiply(0.24);
    visuals.widgets.active.weak_bg_fill = pal.accent.gamma_multiply(0.24);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, pal.accent);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, pal.text_primary);

    visuals.widgets.open.bg_fill = pal.bg_elevated;
    visuals.widgets.open.weak_bg_fill = pal.bg_elevated;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, pal.accent.gamma_multiply(0.8));
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, pal.text_primary);

    style.visuals = visuals;
    ctx.set_global_style(style);
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

    #[test]
    fn embedded_inter_fonts_are_valid_ttf() {
        assert!(ab_glyph::FontArc::try_from_slice(INTER_REGULAR_TTF).is_ok());
        assert!(ab_glyph::FontArc::try_from_slice(INTER_SEMIBOLD_TTF).is_ok());
        let ctx = egui::Context::default();
        install_fonts(&ctx);
    }

    #[test]
    fn palette_surfaces_distinct_for_every_theme() {
        for theme in &THEMES {
            let pal = palette(theme);
            assert_ne!(pal.bg_canvas, pal.bg_chrome, "{}", theme.name);
            assert_ne!(pal.bg_surface, pal.bg_chrome, "{}", theme.name);
        }
    }
}
