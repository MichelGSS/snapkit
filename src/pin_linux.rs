//! Pin to Screen on Linux: borderless, always-on-top floating reference windows via GTK3.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use gtk::gdk;
use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
use gtk::prelude::*;
use image::RgbaImage;

use crate::platform::{self, PxRect};

const BORDER_PX: i32 = 2;
const MIN_PIN_SIDE: i32 = 48;
const ZOOM_MIN: f32 = 0.25;
const ZOOM_MAX: f32 = 4.0;

static ACTIVE_COUNT: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static OPEN_PINS: RefCell<Vec<gtk::Window>> = const { RefCell::new(Vec::new()) };
}

struct PinInner {
    image: RgbaImage,
    pixbuf: Pixbuf,
    accent_rgb: [u8; 3],
    zoom: f32,
    alpha: f64,
    scale: f32,
    active_menu: Option<gtk::Menu>,
}

impl PinInner {
    fn new(image: RgbaImage, accent_rgb: [u8; 3], scale: f32) -> Self {
        let mut rgba = Vec::with_capacity(image.as_raw().len());
        for px in image.pixels() {
            let [r, g, b, a] = px.0;
            if a == 255 {
                rgba.extend_from_slice(&[r, g, b, 255]);
            } else {
                let alpha = a as u16;
                let inv = 255 - alpha;
                rgba.push(((r as u16 * alpha + 34 * inv) / 255) as u8);
                rgba.push(((g as u16 * alpha + 30 * inv) / 255) as u8);
                rgba.push(((b as u16 * alpha + 30 * inv) / 255) as u8);
                rgba.push(255);
            }
        }
        let w = image.width() as i32;
        let h = image.height() as i32;
        let bytes = gtk::glib::Bytes::from_owned(rgba);
        let pixbuf = Pixbuf::from_bytes(&bytes, Colorspace::Rgb, true, 8, w, h, w * 4);
        Self {
            image,
            pixbuf,
            accent_rgb,
            zoom: 1.0,
            alpha: 1.0,
            scale: scale.max(0.5),
            active_menu: None,
        }
    }

    fn window_size(&self) -> (i32, i32) {
        let iw = ((self.image.width() as f32 * self.zoom / self.scale).round() as i32).max(MIN_PIN_SIDE);
        let ih = ((self.image.height() as f32 * self.zoom / self.scale).round() as i32).max(MIN_PIN_SIDE);
        (iw + BORDER_PX * 2, ih + BORDER_PX * 2)
    }
}

fn copy_pin_image(image: &RgbaImage) {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_image(arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: std::borrow::Cow::Borrowed(image.as_raw()),
        });
    }
}

fn apply_zoom(window: &gtk::Window, area: &gtk::DrawingArea, state: &Rc<RefCell<PinInner>>, new_zoom: f32) {
    let clamped = new_zoom.clamp(ZOOM_MIN, ZOOM_MAX);
    let (ww, wh) = {
        let mut s = state.borrow_mut();
        if (clamped - s.zoom).abs() < 1e-4 {
            return;
        }
        s.zoom = clamped;
        s.window_size()
    };
    window.resize(ww, wh);
    area.queue_draw();
}

fn apply_alpha(window: &gtk::Window, state: &Rc<RefCell<PinInner>>, next_alpha: f64) {
    let clamped = next_alpha.clamp(0.20, 1.0);
    state.borrow_mut().alpha = clamped;
    window.set_opacity(clamped);
}

pub fn spawn(image: RgbaImage, accent_rgb: [u8; 3], at_rect: Option<PxRect>) -> Result<(), String> {
    if image.width() == 0 || image.height() == 0 {
        return Err(t!("imagem vazia", "empty image").to_owned());
    }
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    platform::run_on_gtk(move || {
        let cursor = platform::cursor_pos();
        let scale = platform::dpi_scale_at(cursor).max(0.5);
        let mut inner = PinInner::new(image, accent_rgb, scale);
        let (mut win_w, mut win_h) = inner.window_size();
        let work = platform::work_area_at(cursor).unwrap_or((0, 0, 1920, 1080));
        let work_l = (
            (work.0 as f32 / scale).round() as i32,
            (work.1 as f32 / scale).round() as i32,
            (work.2 as f32 / scale).round() as i32,
            (work.3 as f32 / scale).round() as i32,
        );
        let cursor_l = (
            (cursor.0 as f32 / scale).round() as i32,
            (cursor.1 as f32 / scale).round() as i32,
        );
        let max_w = ((work_l.2 - work_l.0) * 85 / 100).max(MIN_PIN_SIDE + BORDER_PX * 2);
        let max_h = ((work_l.3 - work_l.1) * 85 / 100).max(MIN_PIN_SIDE + BORDER_PX * 2);
        if at_rect.is_none() && (win_w > max_w || win_h > max_h) {
            let fit = (max_w as f32 / win_w as f32).min(max_h as f32 / win_h as f32).clamp(ZOOM_MIN, 1.0);
            inner.zoom = fit;
            (win_w, win_h) = inner.window_size();
        }
        let (x, y) = match at_rect {
            Some((l, top, _, _)) => (
                (l as f32 / scale).round() as i32 - BORDER_PX,
                (top as f32 / scale).round() as i32 - BORDER_PX,
            ),
            None => {
                let cx = (cursor_l.0 - win_w / 2).clamp(work_l.0 + 8, (work_l.2 - win_w - 8).max(work_l.0 + 8));
                let cy = (cursor_l.1 - win_h / 2).clamp(work_l.1 + 8, (work_l.3 - win_h - 8).max(work_l.1 + 8));
                (cx, cy)
            }
        };

        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("SnapKit Pin");
        window.set_decorated(false);
        window.set_keep_above(true);
        window.set_skip_taskbar_hint(true);
        window.set_app_paintable(true);
        window.set_default_size(win_w, win_h);
        window.move_(x, y);
        window.add_events(
            gdk::EventMask::BUTTON_PRESS_MASK
                | gdk::EventMask::BUTTON_RELEASE_MASK
                | gdk::EventMask::SCROLL_MASK
                | gdk::EventMask::SMOOTH_SCROLL_MASK
                | gdk::EventMask::KEY_PRESS_MASK,
        );

        let area = gtk::DrawingArea::new();
        window.add(&area);

        let state = Rc::new(RefCell::new(inner));

        {
            let state = state.clone();
            area.connect_draw(move |widget, cr| {
                let s = state.borrow();
                let alloc = widget.allocation();
                let ww = alloc.width() as f64;
                let wh = alloc.height() as f64;

                // Border
                cr.set_source_rgb(
                    s.accent_rgb[0] as f64 / 255.0,
                    s.accent_rgb[1] as f64 / 255.0,
                    s.accent_rgb[2] as f64 / 255.0,
                );
                let _ = cr.paint();

                // Scaled image inside the 2px border
                let dst_w = (ww - (BORDER_PX * 2) as f64).max(1.0);
                let dst_h = (wh - (BORDER_PX * 2) as f64).max(1.0);
                let iw = s.image.width().max(1) as f64;
                let ih = s.image.height().max(1) as f64;
                cr.save().ok();
                cr.translate(BORDER_PX as f64, BORDER_PX as f64);
                cr.scale(dst_w / iw, dst_h / ih);
                cr.set_source_pixbuf(&s.pixbuf, 0.0, 0.0);
                let _ = cr.paint();
                cr.restore().ok();

                gtk::glib::Propagation::Stop
            });
        }

        {
            let window_weak = window.downgrade();
            let area_weak = area.downgrade();
            let state = state.clone();
            window.connect_button_press_event(move |win, ev| {
                match ev.button() {
                    1 => {
                        if ev.event_type() == gdk::EventType::DoubleButtonPress {
                            win.close();
                        } else {
                            let (rx, ry) = ev.root();
                            win.begin_move_drag(1, rx as i32, ry as i32, ev.time());
                        }
                        gtk::glib::Propagation::Stop
                    }
                    2 => {
                        win.close();
                        gtk::glib::Propagation::Stop
                    }
                    3 => {
                        let menu = gtk::Menu::new();
                        let copy_item = gtk::MenuItem::with_label(t!("Copiar imagem (Ctrl+C)", "Copy image (Ctrl+C)"));
                        {
                            let state = state.clone();
                            copy_item.connect_activate(move |_| copy_pin_image(&state.borrow().image));
                        }
                        menu.append(&copy_item);
                        menu.append(&gtk::SeparatorMenuItem::new());

                        for (label, z) in [("50%", 0.50f32), ("100%", 1.00f32), ("200%", 2.00f32)] {
                            let item = gtk::MenuItem::with_label(&tf!("Zoom {label}", "Zoom {label}"));
                            let w = window_weak.clone();
                            let a = area_weak.clone();
                            let s = state.clone();
                            item.connect_activate(move |_| {
                                if let (Some(w), Some(a)) = (w.upgrade(), a.upgrade()) {
                                    apply_zoom(&w, &a, &s, z);
                                }
                            });
                            menu.append(&item);
                        }
                        menu.append(&gtk::SeparatorMenuItem::new());

                        for (label, a_val) in [("100%", 1.0), ("75%", 0.75), ("50%", 0.50), ("30%", 0.30)] {
                            let item = gtk::MenuItem::with_label(&tf!("Opacidade {label}", "Opacity {label}"));
                            let w = window_weak.clone();
                            let s = state.clone();
                            item.connect_activate(move |_| {
                                if let Some(w) = w.upgrade() {
                                    apply_alpha(&w, &s, a_val);
                                }
                            });
                            menu.append(&item);
                        }
                        menu.append(&gtk::SeparatorMenuItem::new());

                        let close_item = gtk::MenuItem::with_label(t!("Fechar (Esc)", "Close (Esc)"));
                        let w = window_weak.clone();
                        close_item.connect_activate(move |_| {
                            if let Some(w) = w.upgrade() {
                                w.close();
                            }
                        });
                        menu.append(&close_item);
                        {
                            let state = state.clone();
                            menu.connect_deactivate(move |_| {
                                state.borrow_mut().active_menu = None;
                            });
                        }
                        menu.show_all();
                        menu.popup_at_pointer(Some(ev));
                        state.borrow_mut().active_menu = Some(menu);
                        gtk::glib::Propagation::Stop
                    }
                    _ => gtk::glib::Propagation::Proceed,
                }
            });
        }

        {
            let area_weak = area.downgrade();
            let state = state.clone();
            window.connect_scroll_event(move |win, ev| {
                let delta = match ev.direction() {
                    gdk::ScrollDirection::Up => 1.0f32,
                    gdk::ScrollDirection::Down => -1.0f32,
                    gdk::ScrollDirection::Smooth => -ev.delta().1 as f32,
                    _ => 0.0,
                };
                if delta.abs() < 1e-4 {
                    return gtk::glib::Propagation::Proceed;
                }
                let ctrl = ev.state().contains(gdk::ModifierType::CONTROL_MASK);
                if ctrl {
                    let cur = state.borrow().alpha;
                    apply_alpha(win, &state, cur + (delta as f64) * 0.08);
                } else if let Some(a) = area_weak.upgrade() {
                    let cur = state.borrow().zoom;
                    apply_zoom(win, &a, &state, cur * 1.15_f32.powf(delta));
                }
                gtk::glib::Propagation::Stop
            });
        }

        {
            let area_weak = area.downgrade();
            let state = state.clone();
            window.connect_key_press_event(move |win, ev| {
                let kv = ev.keyval();
                let ctrl = ev.state().contains(gdk::ModifierType::CONTROL_MASK);
                if kv == gdk::keys::constants::Escape {
                    win.close();
                    return gtk::glib::Propagation::Stop;
                }
                if ctrl && (kv == gdk::keys::constants::c || kv == gdk::keys::constants::C) {
                    copy_pin_image(&state.borrow().image);
                    return gtk::glib::Propagation::Stop;
                }
                if kv == gdk::keys::constants::_0 || kv == gdk::keys::constants::_1 {
                    if let Some(a) = area_weak.upgrade() {
                        apply_zoom(win, &a, &state, 1.0);
                    }
                    return gtk::glib::Propagation::Stop;
                }
                gtk::glib::Propagation::Proceed
            });
        }

        ACTIVE_COUNT.fetch_add(1, Ordering::SeqCst);
        OPEN_PINS.with(|list| list.borrow_mut().push(window.clone()));
        window.connect_destroy(|w| {
            ACTIVE_COUNT.fetch_sub(1, Ordering::SeqCst);
            OPEN_PINS.with(|list| list.borrow_mut().retain(|item| item != w));
        });

        window.show_all();
        let _ = tx.send(Ok(()));
    })?;
    rx.recv_timeout(std::time::Duration::from_secs(2))
        .map_err(|_| t!("tempo esgotado ao fixar captura", "timed out pinning capture").to_owned())?
}

pub fn close_all() {
    let _ = platform::run_on_gtk(|| {
        let windows: Vec<gtk::Window> = OPEN_PINS.with(|list| list.borrow().clone());
        for w in windows {
            w.close();
        }
    });
}

#[cfg(test)]
pub fn active_count() -> usize {
    ACTIVE_COUNT.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_image_is_rejected_and_pin_window_spawns_when_display_exists() {
        assert!(spawn(RgbaImage::new(0, 0), [0; 3], None).is_err());
        if !platform::has_display() {
            return;
        }
        let img = RgbaImage::from_pixel(120, 80, image::Rgba([255, 138, 76, 255]));
        spawn(img, [255, 138, 76], Some((40, 40, 160, 120))).unwrap();
        assert!(active_count() >= 1);
        close_all();
        for _ in 0..20 {
            if active_count() == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}
