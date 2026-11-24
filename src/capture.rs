//! Screen capture: photographs the monitor under the cursor, in physical pixels.

use image::RgbaImage;
#[cfg(any(windows, target_os = "linux"))]
use xcap::Monitor;

use crate::platform::{self, PxRect};

/// A photo of one monitor, with what's needed to pick a window in it.
pub struct Shot {
    pub image: RgbaImage,
    /// Rectangles of the visible windows, in image pixels, front to back.
    pub windows: Vec<PxRect>,
    /// Title of the window that was in the foreground, for searching the library.
    pub title: String,
    /// Note to show the user (used by the scrolling capture).
    pub note: String,
}

/// Captures the monitor under the cursor (or the primary one, if that fails).
///
/// `own_hwnd` is our window, left out of the window list.
pub fn capture_monitor_at_cursor(own_hwnd: isize, include_cursor: bool) -> Result<Shot, String> {
    let (cx, cy) = platform::cursor_pos();
    let title = platform::foreground_window_title();
    // The cursor is read before the photo, to stay as close as possible to what was on screen.
    let cursor = if include_cursor { platform::cursor_image() } else { None };
    let (mut image, (ox, oy)) = photograph_monitor_at((cx, cy))?;
    if let Some((cursor_img, (x, y))) = cursor {
        image::imageops::overlay(&mut image, &cursor_img, (x - ox) as i64, (y - oy) as i64);
    }
    let (w, h) = (image.width() as i32, image.height() as i32);
    let windows = platform::visible_windows(own_hwnd)
        .into_iter()
        .filter_map(|r| clip_to_image((r.0 - ox, r.1 - oy, r.2 - ox, r.3 - oy), w, h))
        .collect();
    Ok(Shot { image, windows, title, note: String::new() })
}

/// Photo of the monitor that contains the point, and the screen position of its top-left corner.
#[cfg(windows)]
fn photograph_monitor_at(p: (i32, i32)) -> Result<(RgbaImage, (i32, i32)), String> {
    let monitor = Monitor::from_point(p.0, p.1)
        .or_else(|_| primary_monitor())
        .map_err(|e| tf!("nenhum monitor encontrado: {e}", "no monitor found: {e}"))?;
    let origin = (monitor.x().unwrap_or(0), monitor.y().unwrap_or(0));
    let image = monitor
        .capture_image()
        .map_err(|e| tf!("falha ao capturar a tela: {e}", "could not capture the screen: {e}"))?;
    Ok((image, origin))
}

#[cfg(target_os = "macos")]
fn photograph_monitor_at(p: (i32, i32)) -> Result<(RgbaImage, (i32, i32)), String> {
    let rect = platform::monitor_rect_at(p);
    let image = platform::capture_px(rect).map_err(|e| tf!("falha ao capturar a tela: {e}", "could not capture the screen: {e}"))?;
    Ok((image, (rect.0, rect.1)))
}

#[cfg(target_os = "linux")]
fn capture_linux_fallback(include_cursor: bool) -> Result<RgbaImage, String> {
    static NEXT_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("snapkit-cap-{}-{id}.png", std::process::id()));
    let _ = std::fs::remove_file(&tmp);

    let mut spectacle = std::process::Command::new("spectacle");
    spectacle.args(["-b", "-n", "-f"]);
    if include_cursor {
        spectacle.arg("-p");
    }
    spectacle.arg("-o").arg(&tmp);

    let mut grim = std::process::Command::new("grim");
    if include_cursor {
        grim.arg("-c");
    }
    grim.arg(&tmp);

    let ok = spectacle.status().is_ok_and(|s| s.success())
        || grim.status().is_ok_and(|s| s.success());

    if ok && tmp.is_file() {
        let img = image::open(&tmp).map(|i| i.to_rgba8()).map_err(|e| e.to_string());
        let _ = std::fs::remove_file(&tmp);
        return img;
    }
    let _ = std::fs::remove_file(&tmp);
    Err(t!("falha ao capturar a tela", "could not capture the screen").to_owned())
}

#[cfg(target_os = "linux")]
fn linux_monitor_rect(m: &Monitor) -> Option<PxRect> {
    platform::monitor_physical_rect(m)
}

#[cfg(target_os = "linux")]
fn photograph_monitor_at(p: (i32, i32)) -> Result<(RgbaImage, (i32, i32)), String> {
    let Some(monitor) = platform::monitor_at_physical_point(p) else {
        let full = capture_linux_fallback(false)?;
        return Ok((full, (0, 0)));
    };
    let rect = linux_monitor_rect(&monitor);
    let (ox, oy) = rect
        .map(|r| (r.0, r.1))
        .unwrap_or_else(|| (monitor.x().unwrap_or(0), monitor.y().unwrap_or(0)));
    let image = match monitor.capture_image() {
        Ok(img) => img,
        Err(e) => {
            if let Ok(full) = capture_linux_fallback(false) {
                let mw = rect.map(|r| r.2 - r.0).unwrap_or_else(|| full.width() as i32);
                let mh = rect.map(|r| r.3 - r.1).unwrap_or_else(|| full.height() as i32);
                let desktop = platform::virtual_screen();
                let rx = (ox - desktop.0).clamp(0, full.width() as i32);
                let ry = (oy - desktop.1).clamp(0, full.height() as i32);
                let rw = mw.min(full.width() as i32 - rx).max(1) as u32;
                let rh = mh.min(full.height() as i32 - ry).max(1) as u32;
                image::imageops::crop_imm(&full, rx as u32, ry as u32, rw, rh).to_image()
            } else {
                return Err(tf!("falha ao capturar a tela: {e}", "could not capture the screen: {e}"));
            }
        }
    };
    Ok((image, (ox, oy)))
}

/// Photographs only the rectangle `rect` (screen coordinates), without the cursor.
pub fn grab_rect(rect: PxRect) -> Result<RgbaImage, String> {
    grab_virtual(rect, false)
}

/// Part of `rect` that falls inside `monitor` (both in screen coordinates).
fn overlap(rect: PxRect, monitor: PxRect) -> Option<PxRect> {
    let r = (rect.0.max(monitor.0), rect.1.max(monitor.1), rect.2.min(monitor.2), rect.3.min(monitor.3));
    (r.2 > r.0 && r.3 > r.1).then_some(r)
}

/// Photographs the rectangle `rect` of the desktop, joining the monitors it crosses. Whatever falls
/// outside every monitor (monitors of different heights) comes out black.
pub fn grab_virtual(rect: PxRect, include_cursor: bool) -> Result<RgbaImage, String> {
    let (w, h) = (rect.2 - rect.0, rect.3 - rect.1);
    if w < 4 || h < 4 {
        return Err(t!("área pequena demais", "area too small").to_owned());
    }
    // The cursor is read before the photo, to stay as close as possible to what was on screen.
    let cursor = if include_cursor { platform::cursor_image() } else { None };
    let mut out = photograph_area(rect)?;
    if let Some((cursor_img, (x, y))) = cursor {
        image::imageops::overlay(&mut out, &cursor_img, (x - rect.0) as i64, (y - rect.1) as i64);
    }
    Ok(out)
}

/// Photo of exactly `rect`, joining the monitors it crosses.
#[cfg(windows)]
fn photograph_area(rect: PxRect) -> Result<RgbaImage, String> {
    let (w, h) = (rect.2 - rect.0, rect.3 - rect.1);
    let mut out = RgbaImage::from_pixel(w as u32, h as u32, image::Rgba([0, 0, 0, 255]));
    let mut found = false;
    for monitor in Monitor::all().map_err(|e| tf!("nenhum monitor encontrado: {e}", "no monitor found: {e}"))? {
        let (ox, oy) = (monitor.x().unwrap_or(0), monitor.y().unwrap_or(0));
        // Skip photographing monitors the area doesn't even touch.
        let size = (monitor.width().unwrap_or(0) as i32, monitor.height().unwrap_or(0) as i32);
        if size.0 > 0 && size.1 > 0 && overlap(rect, (ox, oy, ox + size.0, oy + size.1)).is_none() {
            continue;
        }
        let image = monitor.capture_image().map_err(|e| tf!("falha ao capturar a tela: {e}", "could not capture the screen: {e}"))?;
        let bounds = (ox, oy, ox + image.width() as i32, oy + image.height() as i32);
        let Some(part) = overlap(rect, bounds) else { continue };
        let piece = image::imageops::crop_imm(
            &image,
            (part.0 - ox) as u32,
            (part.1 - oy) as u32,
            (part.2 - part.0) as u32,
            (part.3 - part.1) as u32,
        )
        .to_image();
        image::imageops::replace(&mut out, &piece, (part.0 - rect.0) as i64, (part.1 - rect.1) as i64);
        found = true;
    }
    if !found {
        return Err(t!("nenhum monitor na área escolhida", "no monitor in the chosen area").to_owned());
    }
    Ok(out)
}

/// Photo of exactly `rect`: CoreGraphics composes the monitors it crosses by itself.
#[cfg(target_os = "macos")]
fn photograph_area(rect: PxRect) -> Result<RgbaImage, String> {
    let all = platform::virtual_screen();
    if overlap(rect, all).is_none() {
        return Err(t!("nenhum monitor na área escolhida", "no monitor in the chosen area").to_owned());
    }
    platform::capture_px(rect).map_err(|e| tf!("falha ao capturar a tela: {e}", "could not capture the screen: {e}"))
}

#[cfg(target_os = "linux")]
fn photograph_area(rect: PxRect) -> Result<RgbaImage, String> {
    let (w, h) = (rect.2 - rect.0, rect.3 - rect.1);
    let mut out = RgbaImage::from_pixel(w as u32, h as u32, image::Rgba([0, 0, 0, 255]));
    let mut found = false;
    let Ok(monitors) = Monitor::all() else {
        let full = capture_linux_fallback(false)?;
        let bounds = (0, 0, full.width() as i32, full.height() as i32);
        let part = overlap(rect, bounds)
            .ok_or_else(|| t!("nenhum monitor na área escolhida", "no monitor in the chosen area").to_owned())?;
        let piece = image::imageops::crop_imm(
            &full,
            part.0 as u32,
            part.1 as u32,
            (part.2 - part.0) as u32,
            (part.3 - part.1) as u32,
        )
        .to_image();
        image::imageops::replace(&mut out, &piece, (part.0 - rect.0) as i64, (part.1 - rect.1) as i64);
        return Ok(out);
    };
    for monitor in monitors {
        let mrect = linux_monitor_rect(&monitor);
        let (ox, oy) = mrect
            .map(|r| (r.0, r.1))
            .unwrap_or_else(|| (monitor.x().unwrap_or(0), monitor.y().unwrap_or(0)));
        if let Some(mr) = mrect
            && overlap(rect, mr).is_none()
        {
            continue;
        }
        let image = match monitor.capture_image() {
            Ok(img) => img,
            Err(e) => {
                if let Ok(full) = capture_linux_fallback(false) {
                    let desktop = platform::virtual_screen();
                    let dw = full.width() as i32;
                    let dh = full.height() as i32;
                    let bounds = (desktop.0, desktop.1, desktop.0 + dw, desktop.1 + dh);
                    if let Some(part) = overlap(rect, bounds) {
                        let piece = image::imageops::crop_imm(
                            &full,
                            (part.0 - desktop.0) as u32,
                            (part.1 - desktop.1) as u32,
                            (part.2 - part.0) as u32,
                            (part.3 - part.1) as u32,
                        )
                        .to_image();
                        image::imageops::replace(&mut out, &piece, (part.0 - rect.0) as i64, (part.1 - rect.1) as i64);
                        found = true;
                        break;
                    }
                }
                return Err(tf!("falha ao capturar a tela: {e}", "could not capture the screen: {e}"));
            }
        };
        let bounds = (ox, oy, ox + image.width() as i32, oy + image.height() as i32);
        let Some(part) = overlap(rect, bounds) else { continue };
        let piece = image::imageops::crop_imm(
            &image,
            (part.0 - ox) as u32,
            (part.1 - oy) as u32,
            (part.2 - part.0) as u32,
            (part.3 - part.1) as u32,
        )
        .to_image();
        image::imageops::replace(&mut out, &piece, (part.0 - rect.0) as i64, (part.1 - rect.1) as i64);
        found = true;
    }
    if !found {
        return Err(t!("nenhum monitor na área escolhida", "no monitor in the chosen area").to_owned());
    }
    Ok(out)
}

/// Captures the active monitor backdrop along with its exact physical pixel bounds (used on Linux
/// where selection overlays cannot be excluded from screen capture).
#[cfg(target_os = "linux")]
pub fn grab_desktop_backdrop(include_cursor: bool) -> Result<(RgbaImage, PxRect), String> {
    if Monitor::all().map(|m| m.is_empty()).unwrap_or(true) {
        let full = capture_linux_fallback(include_cursor)?;
        let bounds = (0, 0, full.width() as i32, full.height() as i32);
        return Ok((full, bounds));
    }
    let area = platform::selection_area();
    let image = grab_virtual(area, include_cursor)?;
    Ok((image, area))
}

#[cfg(windows)]
fn primary_monitor() -> xcap::XCapResult<Monitor> {
    let monitors = Monitor::all()?;
    monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())
        .cloned()
        .ok_or_else(|| xcap::XCapError::new("empty monitor list"))
}

/// Clamps a rectangle to the image; returns `None` if fewer than a few pixels remain.
fn clip_to_image(r: PxRect, w: i32, h: i32) -> Option<PxRect> {
    let c = (r.0.max(0), r.1.max(0), r.2.min(w), r.3.min(h));
    (c.2 - c.0 >= 8 && c.3 - c.1 >= 8).then_some(c)
}

/// The frontmost window that contains the point (in image pixels).
pub fn window_at(windows: &[PxRect], x: f32, y: f32) -> Option<PxRect> {
    let (x, y) = (x as i32, y as i32);
    windows
        .iter()
        .copied()
        .find(|r| x >= r.0 && x < r.2 && y >= r.1 && y < r.3)
}

/// Crops `img` to the rectangle given in pixels, clamped to the image edges. Returns `None` if the
/// area is too small to be an intentional capture.
pub fn crop(img: &RgbaImage, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<RgbaImage> {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let left = x0.min(x1).clamp(0.0, w).round() as u32;
    let top = y0.min(y1).clamp(0.0, h).round() as u32;
    let right = x0.max(x1).clamp(0.0, w).round() as u32;
    let bottom = y0.max(y1).clamp(0.0, h).round() as u32;
    let (cw, ch) = (right.saturating_sub(left), bottom.saturating_sub(top));
    if cw < 4 || ch < 4 {
        return None;
    }
    Some(image::imageops::crop_imm(img, left, top, cw, ch).to_image())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_normalizes_and_clamps() {
        let img = RgbaImage::new(100, 50);
        // Dragging "backwards" and past the edge of the image.
        let out = crop(&img, 80.0, 40.0, -10.0, 10.0).unwrap();
        assert_eq!((out.width(), out.height()), (80, 30));
    }

    #[test]
    fn overlap_splits_an_area_between_two_monitors() {
        let (left, right) = ((-1920, 0, 0, 1080), (0, 0, 2560, 1440));
        let area = (-100, 50, 300, 250);
        assert_eq!(overlap(area, left), Some((-100, 50, 0, 250)));
        assert_eq!(overlap(area, right), Some((0, 50, 300, 250)));
        assert_eq!(overlap((10, 10, 50, 50), left), None, "area only on the right monitor");
    }

    #[test]
    fn grab_virtual_returns_exactly_the_requested_area() {
        let all = platform::virtual_screen();
        assert!(grab_virtual((all.0, all.1, all.0 + 2, all.1 + 2), false).is_err(), "area too small");
        if crate::in_ci() {
            return;
        }
        // The whole desktop: with two monitors, the image is as wide as both.
        let Ok(image) = grab_virtual(all, false) else {
            return;
        };
        assert_eq!((image.width() as i32, image.height() as i32), (all.2 - all.0, all.3 - all.1));
        // An area entirely off screen has nothing to photograph.
        assert!(grab_virtual((all.2 + 5000, all.3 + 5000, all.2 + 5100, all.3 + 5100), false).is_err());
    }

    #[test]
    fn crop_rejects_tiny_area() {
        let img = RgbaImage::new(100, 50);
        assert!(crop(&img, 10.0, 10.0, 12.0, 30.0).is_none());
    }

    #[test]
    fn window_at_prefers_the_frontmost() {
        let windows = [(10, 10, 50, 50), (0, 0, 100, 100)];
        assert_eq!(window_at(&windows, 20.0, 20.0), Some((10, 10, 50, 50)));
        assert_eq!(window_at(&windows, 80.0, 80.0), Some((0, 0, 100, 100)));
        assert_eq!(window_at(&windows, 150.0, 20.0), None);
    }

    #[test]
    fn clip_drops_windows_outside_the_monitor() {
        assert_eq!(clip_to_image((-20, -20, 60, 60), 100, 100), Some((0, 0, 60, 60)));
        assert_eq!(clip_to_image((-500, 0, -100, 80), 100, 100), None);
    }
}
