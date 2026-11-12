//! The program icon and the system tray icon, with its menu.

use std::sync::Arc;

use tiny_skia::{Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrayAction {
    Region,
    FullScreen,
    Open,
    Quit,
}

const ID_REGION: &str = "region";
const ID_FULL: &str = "full";
const ID_OPEN: &str = "open";
const ID_QUIT: &str = "quit";

/// Draws the SnapKit icon: a rounded orange square with white crop marks. Returns non-premultiplied
/// RGBA, `size`×`size`.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut pm = Pixmap::new(size, size).expect("invalid icon size");

    let mut bg = Paint::default();
    bg.set_color(Color::from_rgba8(255, 138, 76, 255));
    bg.anti_alias = true;
    let r = s * 0.22;
    let mut pb = PathBuilder::new();
    pb.move_to(r, 0.0);
    pb.line_to(s - r, 0.0);
    pb.quad_to(s, 0.0, s, r);
    pb.line_to(s, s - r);
    pb.quad_to(s, s, s - r, s);
    pb.line_to(r, s);
    pb.quad_to(0.0, s, 0.0, s - r);
    pb.line_to(0.0, r);
    pb.quad_to(0.0, 0.0, r, 0.0);
    pb.close();
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, &bg, FillRule::Winding, Transform::identity(), None);
    }

    // Two "L" corners, like the marks of a selection.
    let mut fg = Paint::default();
    fg.set_color(Color::WHITE);
    fg.anti_alias = true;
    let stroke = Stroke { width: (s * 0.11).max(1.5), line_cap: LineCap::Round, ..Default::default() };
    let (a, b, len) = (s * 0.27, s * 0.73, s * 0.26);
    let mut pb = PathBuilder::new();
    pb.move_to(a, a + len);
    pb.line_to(a, a);
    pb.line_to(a + len, a);
    pb.move_to(b, b - len);
    pb.line_to(b, b);
    pb.line_to(b - len, b);
    if let Some(path) = pb.finish() {
        pm.stroke_path(&path, &fg, &stroke, Transform::identity(), None);
    }
    // Center dot, so the icon doesn't look empty at small sizes.
    let dot = s * 0.07;
    if let Some(rect) = Rect::from_xywh(s / 2.0 - dot, s / 2.0 - dot, dot * 2.0, dot * 2.0) {
        pm.fill_rect(rect, &fg, Transform::identity(), None);
    }

    pm.pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

/// Tray icon with its menu, which can be rewritten (language, shortcut).
pub struct Tray {
    _icon: TrayIcon,
    region: MenuItem,
    full: MenuItem,
    open: MenuItem,
    quit: MenuItem,
}

impl Tray {
    /// Writes the menu items in the current language, with the capture shortcut next to it.
    pub fn relabel(&self, hotkey_label: &str) {
        self.region.set_text(tf!("Capturar região\t{hotkey_label}", "Capture region\t{hotkey_label}"));
        self.full.set_text(t!("Capturar tela cheia", "Capture full screen"));
        self.open.set_text(t!("Abrir SnapKit", "Open SnapKit"));
        self.quit.set_text(t!("Sair", "Quit"));
    }
}

/// Creates the tray icon. `on_action` is called from background threads.
///
/// Must be called on the main thread (the one running the window's event loop).
pub fn build(
    hotkey_label: &str,
    on_action: Arc<dyn Fn(TrayAction) + Send + Sync>,
) -> Result<Tray, String> {
    let menu = Menu::new();
    let region = MenuItem::with_id(ID_REGION, "", true, None);
    let full = MenuItem::with_id(ID_FULL, "", true, None);
    let open = MenuItem::with_id(ID_OPEN, "", true, None);
    let quit = MenuItem::with_id(ID_QUIT, "", true, None);
    menu.append_items(&[
        &region,
        &full,
        &PredefinedMenuItem::separator(),
        &open,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .map_err(|e| e.to_string())?;

    let icon = Icon::from_rgba(icon_rgba(32), 32, 32).map_err(|e| e.to_string())?;
    let icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("SnapKit")
        .with_icon(icon)
        .build()
        .map_err(|e| e.to_string())?;

    let on_menu = on_action.clone();
    std::thread::spawn(move || {
        while let Ok(event) = MenuEvent::receiver().recv() {
            if let Some(action) = action_for_menu_id(event.id.0.as_str()) {
                on_menu(action);
            }
        }
    });
    std::thread::spawn(move || {
        while let Ok(event) = TrayIconEvent::receiver().recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                on_action(TrayAction::Open);
            }
        }
    });
    let tray = Tray { _icon: icon, region, full, open, quit };
    tray.relabel(hotkey_label);
    Ok(tray)
}

fn action_for_menu_id(id: &str) -> Option<TrayAction> {
    match id {
        ID_REGION => Some(TrayAction::Region),
        ID_FULL => Some(TrayAction::FullScreen),
        ID_OPEN => Some(TrayAction::Open),
        ID_QUIT => Some(TrayAction::Quit),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_expected_size_and_colors() {
        let rgba = icon_rgba(32);
        assert_eq!(rgba.len(), 32 * 32 * 4);
        let px = |x: usize, y: usize| &rgba[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(px(0, 0)[3], 0, "rounded corner is transparent");
        assert_eq!(px(16, 4), [255, 138, 76, 255], "orange background");
        assert_eq!(px(16, 16), [255, 255, 255, 255], "ponto central branco");
    }

    #[test]
    fn menu_ids_map_to_actions() {
        assert_eq!(action_for_menu_id(ID_REGION), Some(TrayAction::Region));
        assert_eq!(action_for_menu_id(ID_QUIT), Some(TrayAction::Quit));
        assert_eq!(action_for_menu_id("other"), None);
    }

    /// Rewrites `assets/snapkit.ico` (used in the executable and the installer) from the icon
    /// drawing: `cargo test write_ico -- --ignored`.
    #[test]
    #[ignore]
    fn write_ico() {
        use image::codecs::ico::{IcoEncoder, IcoFrame};
        let frames: Vec<IcoFrame> = [16u32, 24, 32, 48, 64, 128, 256]
            .iter()
            .map(|&s| IcoFrame::as_png(&icon_rgba(s), s, s, image::ExtendedColorType::Rgba8).unwrap())
            .collect();
        let file = std::fs::File::create("assets/snapkit.ico").unwrap();
        IcoEncoder::new(file).encode_images(&frames).unwrap();
    }

    /// Generates `target/icon.png` for visual inspection: `cargo test icon_png -- --ignored`.
    #[test]
    #[ignore]
    fn icon_png() {
        let img = image::RgbaImage::from_raw(256, 256, icon_rgba(256)).unwrap();
        img.save("target/icon.png").unwrap();
    }
}
