//! The program icon and the system tray icon, with its menu.

use std::sync::Arc;

use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, Path, PathBuilder,
    Pixmap, Point, RadialGradient, SpreadMode, Stroke, Transform,
};
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

fn squircle_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let k = r * 0.552_28;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn brackets_path() -> Option<Path> {
    let mut pb = PathBuilder::new();
    // Top-left
    pb.move_to(21.0, 38.0);
    pb.line_to(21.0, 28.5);
    pb.quad_to(21.0, 21.0, 28.5, 21.0);
    pb.line_to(38.0, 21.0);
    // Top-right
    pb.move_to(62.0, 21.0);
    pb.line_to(71.5, 21.0);
    pb.quad_to(79.0, 21.0, 79.0, 28.5);
    pb.line_to(79.0, 38.0);
    // Bottom-right
    pb.move_to(79.0, 62.0);
    pb.line_to(79.0, 71.5);
    pb.quad_to(79.0, 79.0, 71.5, 79.0);
    pb.line_to(62.0, 79.0);
    // Bottom-left
    pb.move_to(38.0, 79.0);
    pb.line_to(28.5, 79.0);
    pb.quad_to(21.0, 79.0, 21.0, 71.5);
    pb.line_to(21.0, 62.0);
    pb.finish()
}

/// Draws the SnapKit icon: a warm gradient squircle framing precision viewfinder corners and an
/// optical camera lens. Returns non-premultiplied RGBA, `size`×`size`.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let xform = Transform::from_scale(s / 100.0, s / 100.0);
    let mut pm = Pixmap::new(size, size).expect("invalid icon size");

    // Layer 1: Warm sunset gradient squircle + specular bloom + crisp inner rim highlight
    if let Some(bg_path) = squircle_path(0.0, 0.0, 100.0, 100.0, 24.0) {
        let mut bg = Paint { anti_alias: true, ..Default::default() };
        if let Some(shader) = LinearGradient::new(
            Point::from_xy(10.0, 6.0),
            Point::from_xy(92.0, 96.0),
            vec![
                GradientStop::new(0.0, Color::from_rgba8(255, 180, 67, 255)),
                GradientStop::new(0.48, Color::from_rgba8(255, 110, 58, 255)),
                GradientStop::new(1.0, Color::from_rgba8(222, 46, 91, 255)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            bg.shader = shader;
        }
        pm.fill_path(&bg_path, &bg, FillRule::Winding, xform, None);

        let mut bloom = Paint { anti_alias: true, ..Default::default() };
        if let Some(shader) = RadialGradient::new(
            Point::from_xy(28.0, 18.0),
            0.0,
            Point::from_xy(28.0, 18.0),
            62.0,
            vec![
                GradientStop::new(0.0, Color::from_rgba8(255, 255, 255, 72)),
                GradientStop::new(0.6, Color::from_rgba8(255, 255, 255, 13)),
                GradientStop::new(1.0, Color::from_rgba8(255, 255, 255, 0)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            bloom.shader = shader;
        }
        pm.fill_path(&bg_path, &bloom, FillRule::Winding, xform, None);
    }

    if let Some(rim_path) = squircle_path(1.5, 1.5, 97.0, 97.0, 22.8) {
        let mut rim = Paint { anti_alias: true, ..Default::default() };
        if let Some(shader) = LinearGradient::new(
            Point::from_xy(50.0, 1.5),
            Point::from_xy(50.0, 98.5),
            vec![
                GradientStop::new(0.0, Color::from_rgba8(255, 255, 255, 102)),
                GradientStop::new(0.45, Color::from_rgba8(255, 255, 255, 20)),
                GradientStop::new(1.0, Color::from_rgba8(0, 0, 0, 40)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            rim.shader = shader;
        }
        let rim_stroke = Stroke { width: 1.8, ..Default::default() };
        pm.stroke_path(&rim_path, &rim, &rim_stroke, xform, None);
    }

    let bracket_w = if size <= 24 { 8.8 } else { 7.4 };
    let bracket_stroke = Stroke {
        width: bracket_w,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };

    // Layer 2: Soft contact shadow under foreground marks
    if let (Some(b_path), Some(bezel_path)) = (
        brackets_path(),
        PathBuilder::from_circle(50.0, 50.0, 18.5),
    ) {
        for (dy, alpha, extra_w) in [(4.0f32, 36u8, 2.0f32), (3.0, 72, 0.0)] {
            let mut sh_paint = Paint { anti_alias: true, ..Default::default() };
            sh_paint.set_color(Color::from_rgba8(74, 12, 22, alpha));
            let sh_xform = xform.post_translate(0.0, dy * (s / 100.0));
            let mut sh_stroke = bracket_stroke.clone();
            sh_stroke.width += extra_w;
            pm.stroke_path(&b_path, &sh_paint, &sh_stroke, sh_xform, None);
            pm.fill_path(&bezel_path, &sh_paint, FillRule::Winding, sh_xform, None);
        }
    }

    // Layer 3: Four pure white viewfinder corner brackets
    let mut white = Paint { anti_alias: true, ..Default::default() };
    white.set_color(Color::WHITE);
    if let Some(b_path) = brackets_path() {
        pm.stroke_path(&b_path, &white, &bracket_stroke, xform, None);
    }

    // Layer 4: Precision optical camera lens
    if let Some(bezel) = PathBuilder::from_circle(50.0, 50.0, 18.5) {
        pm.fill_path(&bezel, &white, FillRule::Winding, xform, None);
    }
    if let Some(glass) = PathBuilder::from_circle(50.0, 50.0, 13.2) {
        let mut glass_paint = Paint { anti_alias: true, ..Default::default() };
        if let Some(shader) = LinearGradient::new(
            Point::from_xy(38.0, 38.0),
            Point::from_xy(62.0, 62.0),
            vec![
                GradientStop::new(0.0, Color::from_rgba8(43, 50, 72, 255)),
                GradientStop::new(0.55, Color::from_rgba8(22, 25, 37, 255)),
                GradientStop::new(1.0, Color::from_rgba8(13, 15, 24, 255)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            glass_paint.shader = shader;
        }
        pm.fill_path(&glass, &glass_paint, FillRule::Winding, xform, None);
    }
    if let Some(ar_ring) = PathBuilder::from_circle(50.0, 50.0, 10.4) {
        let mut ar_paint = Paint { anti_alias: true, ..Default::default() };
        if let Some(shader) = LinearGradient::new(
            Point::from_xy(40.0, 60.0),
            Point::from_xy(60.0, 40.0),
            vec![
                GradientStop::new(0.0, Color::from_rgba8(255, 148, 77, 140)),
                GradientStop::new(1.0, Color::from_rgba8(110, 175, 255, 115)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            ar_paint.shader = shader;
        }
        let ar_stroke = Stroke { width: 1.6, ..Default::default() };
        pm.stroke_path(&ar_ring, &ar_paint, &ar_stroke, xform, None);
    }
    if let Some(pupil) = PathBuilder::from_circle(50.0, 50.0, 5.4) {
        let mut p = Paint { anti_alias: true, ..Default::default() };
        p.set_color(Color::from_rgba8(9, 11, 18, 255));
        pm.fill_path(&pupil, &p, FillRule::Winding, xform, None);
    }
    if let Some(tint) = PathBuilder::from_circle(50.0, 50.0, 3.2) {
        let mut p = Paint { anti_alias: true, ..Default::default() };
        p.set_color(Color::from_rgba8(70, 92, 148, 122));
        pm.fill_path(&tint, &p, FillRule::Winding, xform, None);
    }
    if let Some(glint) = PathBuilder::from_circle(44.8, 44.8, 3.2) {
        pm.fill_path(&glint, &white, FillRule::Winding, xform, None);
    }
    if let Some(glint2) = PathBuilder::from_circle(54.8, 54.6, 1.3) {
        let mut p = Paint { anti_alias: true, ..Default::default() };
        p.set_color(Color::from_rgba8(255, 215, 175, 217));
        pm.fill_path(&glint2, &p, FillRule::Winding, xform, None);
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
#[cfg(not(target_os = "linux"))]
pub struct Tray {
    _icon: TrayIcon,
    region: MenuItem,
    full: MenuItem,
    open: MenuItem,
    quit: MenuItem,
}

#[cfg(target_os = "linux")]
pub struct Tray {}

#[cfg(target_os = "linux")]
impl Drop for Tray {
    fn drop(&mut self) {
        let _ = crate::platform::run_on_gtk(|| {
            GTK_TRAY.with(|cell| {
                *cell.borrow_mut() = None;
            });
        });
    }
}

#[cfg(target_os = "linux")]
struct GtkTrayState {
    _icon: TrayIcon,
    region: MenuItem,
    full: MenuItem,
    open: MenuItem,
    quit: MenuItem,
}

#[cfg(target_os = "linux")]
std::thread_local! {
    static GTK_TRAY: std::cell::RefCell<Option<GtkTrayState>> = const { std::cell::RefCell::new(None) };
}

impl Tray {
    /// Writes the menu items in the current language, with the capture shortcut next to it.
    pub fn relabel(&self, hotkey_label: &str) {
        // Windows menus align what follows a tab to the right, like a shortcut. macOS and GTK3
        // menus don't, so the shortcut goes in parentheses there.
        let region_text = if cfg!(any(target_os = "macos", target_os = "linux")) {
            tf!("Capturar região ({hotkey_label})", "Capture region ({hotkey_label})")
        } else {
            tf!("Capturar região\t{hotkey_label}", "Capture region\t{hotkey_label}")
        };
        let full_text = t!("Capturar tela cheia", "Capture full screen").to_owned();
        let open_text = t!("Abrir SnapKit", "Open SnapKit").to_owned();
        let quit_text = t!("Sair", "Quit").to_owned();
        #[cfg(not(target_os = "linux"))]
        {
            self.region.set_text(region_text);
            self.full.set_text(full_text);
            self.open.set_text(open_text);
            self.quit.set_text(quit_text);
        }
        #[cfg(target_os = "linux")]
        {
            let _ = crate::platform::run_on_gtk(move || {
                GTK_TRAY.with(|cell| {
                    if let Some(state) = cell.borrow().as_ref() {
                        state.region.set_text(region_text);
                        state.full.set_text(full_text);
                        state.open.set_text(open_text);
                        state.quit.set_text(quit_text);
                    }
                });
            });
        }
    }
}

/// Creates the tray icon. `on_action` is called from background threads.
///
/// Must be called on the main thread (the one running the window's event loop).
pub fn build(
    hotkey_label: &str,
    on_action: Arc<dyn Fn(TrayAction) + Send + Sync>,
) -> Result<Tray, String> {
    #[cfg(not(target_os = "linux"))]
    let tray = {
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
            .with_menu_on_left_click(cfg!(target_os = "macos"))
            .with_tooltip("SnapKit")
            .with_icon(icon)
            .build()
            .map_err(|e| e.to_string())?;
        Tray { _icon: icon, region, full, open, quit }
    };

    #[cfg(target_os = "linux")]
    let tray = {
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        crate::platform::run_on_gtk(move || {
            let res = (|| -> Result<GtkTrayState, String> {
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
                Ok(GtkTrayState { _icon: icon, region, full, open, quit })
            })();
            match res {
                Ok(state) => {
                    GTK_TRAY.with(|cell| *cell.borrow_mut() = Some(state));
                    let _ = tx.send(Ok(()));
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        })?;
        rx.recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "tray init timed out".to_owned())??;
        Tray {}
    };

    let on_menu = on_action.clone();
    std::thread::spawn(move || {
        while let Ok(event) = MenuEvent::receiver().recv() {
            if let Some(action) = action_for_menu_id(event.id.0.as_str()) {
                on_menu(action);
            }
        }
    });
    if !cfg!(target_os = "macos") {
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
    }
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
        let bg = px(16, 3);
        assert_eq!(bg[3], 255);
        assert!(bg[0] > 200 && bg[0] > bg[2], "warm gradient background: {bg:?}");
        let lens = px(16, 16);
        assert_eq!(lens[3], 255);
        assert!(lens[0] < 85 && lens[1] < 95 && lens[2] < 130, "dark lens interior: {lens:?}");
        let bezel = px(16, 11);
        assert_eq!(bezel[3], 255);
        assert!(bezel[0] > 190 && bezel[1] > 190 && bezel[2] > 190, "white lens bezel: {bezel:?}");
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
