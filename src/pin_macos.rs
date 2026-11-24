//! Pin to Screen on macOS: borderless, always-on-top floating reference windows for captures.
//! Same API and controls as `pin.rs` (the Windows version).
//!
//! Each pin is an AppKit window at the floating level that joins every Space, with a custom view
//! that draws the capture and handles the mouse and the keyboard. The windows don't depend on the
//! main `eframe` window, so they stay on screen while SnapKit is hidden.
//!
//! Controls on a pinned window:
//! - **Drag** with the left mouse button to move across monitors
//! - **Mouse wheel** to zoom (`25%`–`400%`), anchored at the cursor
//! - **Ctrl (or Cmd) + mouse wheel** to change opacity (`20%`–`100%`)
//! - **Cmd + C** (or Ctrl + C) to copy the image to the clipboard
//! - **Right-click** (or Ctrl + click) for the context menu (Copy, Zoom, Opacity, Close)
//! - **Double-click**, **middle-click**, or **Esc** to close
//!
//! AppKit objects only live on the main thread: windows are created there with [`run_on_main`] and
//! the list of open pins keeps them as plain addresses (`usize`) of retained windows.

use std::cell::Cell;
use std::sync::Mutex;

use dispatch2::{DispatchQueue, run_on_main};
use image::RgbaImage;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSControlStateValueOn, NSCursor, NSEvent, NSEventModifierFlags,
    NSFloatingWindowLevel, NSGraphicsContext, NSMenu, NSMenuItem, NSResponder, NSView, NSWindow,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGBitmapContextGetBytesPerRow,
    CGBitmapContextGetData, CGColorSpace, CGContext, CGDisplayBounds, CGImage, CGImageAlphaInfo,
    CGInterpolationQuality, CGMainDisplayID,
};
use objc2_foundation::NSString;

use crate::platform::{self, PxRect};

/// Border width in logical points (1x pixels); scaled by `platform::scale()` for Retina crispness.
const BORDER_PX: i32 = 2;
const MIN_PIN_SIDE: i32 = 48;
const ZOOM_MIN: f32 = 0.25;
const ZOOM_MAX: f32 = 4.0;
/// Lowest opacity (20%), as a 0–255 alpha.
const ALPHA_MIN: u8 = 51;
/// Trackpads report scrolling in points; this many count as one wheel notch.
const PRECISE_POINTS_PER_NOTCH: f64 = 30.0;

const CMD_COPY: isize = 1;
const CMD_ZOOM_50: isize = 2;
const CMD_ZOOM_100: isize = 3;
const CMD_ZOOM_200: isize = 4;
const CMD_ALPHA_100: isize = 10;
const CMD_ALPHA_75: isize = 11;
const CMD_ALPHA_50: isize = 12;
const CMD_ALPHA_30: isize = 13;
const CMD_CLOSE: isize = 99;

/// macOS virtual key code of Esc (kVK_Escape).
const KEY_ESCAPE: u16 = 0x35;

/// Open pins: addresses of windows retained with `Retained::into_raw`, released by [`close_pin`].
static ACTIVE_PINS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn active_list() -> std::sync::MutexGuard<'static, Vec<usize>> {
    ACTIVE_PINS.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------------------------
// Pure geometry (screen pixels)

fn border_px_for(scale: f64) -> i32 {
    (BORDER_PX as f64 * scale).round() as i32
}

fn border_px() -> i32 {
    border_px_for(platform::scale())
}

/// Window size, border included, for an image of `img_w`×`img_h` pixels at `zoom`.
fn window_size(img_w: u32, img_h: u32, zoom: f32, border: i32) -> (i32, i32) {
    let iw = ((img_w as f32 * zoom).round() as i32).max(MIN_PIN_SIDE);
    let ih = ((img_h as f32 * zoom).round() as i32).max(MIN_PIN_SIDE);
    (iw + border * 2, ih + border * 2)
}

/// Initial zoom and window rectangle. Over `at_rect` (the captured region) the image sits exactly
/// where it was taken; otherwise the window is centered on the cursor inside the work area, shrunk
/// to at most 85% of it.
fn initial_placement(
    img_w: u32,
    img_h: u32,
    cursor: (i32, i32),
    work: PxRect,
    at_rect: Option<PxRect>,
    border: i32,
) -> (f32, PxRect) {
    let mut zoom = 1.0_f32;
    let (mut win_w, mut win_h) = window_size(img_w, img_h, zoom, border);
    let max_w = ((work.2 - work.0) * 85 / 100).max(MIN_PIN_SIDE + border * 2);
    let max_h = ((work.3 - work.1) * 85 / 100).max(MIN_PIN_SIDE + border * 2);
    if at_rect.is_none() && (win_w > max_w || win_h > max_h) {
        zoom = (max_w as f32 / win_w as f32).min(max_h as f32 / win_h as f32).clamp(ZOOM_MIN, 1.0);
        (win_w, win_h) = window_size(img_w, img_h, zoom, border);
    }
    let (x, y) = match at_rect {
        Some((l, top, _, _)) => (l - border, top - border),
        None => {
            let cx = (cursor.0 - win_w / 2).clamp(work.0 + 8, (work.2 - win_w - 8).max(work.0 + 8));
            let cy = (cursor.1 - win_h / 2).clamp(work.1 + 8, (work.3 - win_h - 8).max(work.1 + 8));
            (cx, cy)
        }
    };
    (zoom, (x, y, x + win_w, y + win_h))
}

/// Top-left corner of the window after a resize to `new_size`: the point under `anchor` stays put,
/// or, without an anchor, the center does.
fn resized_origin(old: PxRect, new_size: (i32, i32), anchor: Option<(i32, i32)>) -> (i32, i32) {
    let (new_w, new_h) = new_size;
    match anchor {
        Some((ax, ay)) => {
            let old_w = (old.2 - old.0).max(1) as f32;
            let old_h = (old.3 - old.1).max(1) as f32;
            let rx = ((ax - old.0) as f32 / old_w).clamp(0.0, 1.0);
            let ry = ((ay - old.1) as f32 / old_h).clamp(0.0, 1.0);
            (ax - (new_w as f32 * rx).round() as i32, ay - (new_h as f32 * ry).round() as i32)
        }
        None => ((old.0 + old.2) / 2 - new_w / 2, (old.1 + old.3) / 2 - new_h / 2),
    }
}

fn clamp_zoom(zoom: f32) -> f32 {
    zoom.clamp(ZOOM_MIN, ZOOM_MAX)
}

/// Opacity after turning the wheel by `notches` (20 alpha steps per notch).
fn step_alpha(alpha: u8, notches: f32) -> u8 {
    (alpha as f32 + notches * 20.0).round().clamp(ALPHA_MIN as f32, 255.0) as u8
}

/// Wheel notches in a scroll event: mice report lines, trackpads points. Capped so one fast swipe
/// can't jump straight from one end of the range to the other.
fn wheel_notches(delta_y: f64, precise: bool) -> f32 {
    let notches = if precise { delta_y / PRECISE_POINTS_PER_NOTCH } else { delta_y };
    notches.clamp(-3.0, 3.0) as f32
}

// ---------------------------------------------------------------------------------------------
// Coordinates: screen pixels <-> AppKit frames (points, bottom-left origin of the main display)

fn flip(r: CGRect) -> CGRect {
    let main_height = CGDisplayBounds(CGMainDisplayID()).size.height;
    CGRect::new(CGPoint::new(r.origin.x, main_height - r.origin.y - r.size.height), r.size)
}

fn px_rect_to_frame(r: PxRect) -> CGRect {
    let s = platform::scale();
    flip(CGRect::new(
        CGPoint::new(r.0 as f64 / s, r.1 as f64 / s),
        CGSize::new((r.2 - r.0) as f64 / s, (r.3 - r.1) as f64 / s),
    ))
}

fn frame_to_px_rect(frame: CGRect) -> PxRect {
    let s = platform::scale();
    let r = flip(frame);
    let px = |v: f64| (v * s).round() as i32;
    (px(r.origin.x), px(r.origin.y), px(r.origin.x + r.size.width), px(r.origin.y + r.size.height))
}

// ---------------------------------------------------------------------------------------------
// Image

/// The capture as an opaque CGImage: transparent pixels are composited over dark slate, like on
/// Windows, so transparent stickers look clean.
fn opaque_cg_image(image: &RgbaImage) -> Option<CFRetained<CGImage>> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: a null data pointer lets CoreGraphics allocate and own the pixel memory, which then
    // lives as long as the context (and is shared copy-on-write with the image made from it).
    let context = unsafe {
        CGBitmapContextCreate(
            std::ptr::null_mut(),
            w,
            h,
            8,
            w * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }?;
    let data = CGBitmapContextGetData(Some(&context)).cast::<u8>();
    let stride = CGBitmapContextGetBytesPerRow(Some(&context));
    if data.is_null() || stride < w * 4 {
        return None;
    }
    for (y, row) in image.as_raw().chunks_exact(w * 4).enumerate().take(h) {
        // SAFETY: the context's buffer holds `h` rows of `stride >= w * 4` bytes; nothing else
        // touches it while this slice is alive.
        let dst = unsafe { std::slice::from_raw_parts_mut(data.add(y * stride), w * 4) };
        for (d, s) in dst.chunks_exact_mut(4).zip(row.chunks_exact(4)) {
            let a = s[3] as u16;
            if a == 255 {
                d.copy_from_slice(&[s[0], s[1], s[2], 255]);
            } else {
                let inv = 255 - a;
                d[0] = ((s[0] as u16 * a + 34 * inv) / 255) as u8;
                d[1] = ((s[1] as u16 * a + 30 * inv) / 255) as u8;
                d[2] = ((s[2] as u16 * a + 30 * inv) / 255) as u8;
                d[3] = 255;
            }
        }
    }
    CGBitmapContextCreateImage(Some(&context))
}

fn copy_pin_image(img: &RgbaImage) {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_image(arboard::ImageData {
            width: img.width() as usize,
            height: img.height() as usize,
            bytes: std::borrow::Cow::Borrowed(img.as_raw()),
        });
    }
}

// ---------------------------------------------------------------------------------------------
// AppKit classes

struct PinIvars {
    image: RgbaImage,
    cg_image: CFRetained<CGImage>,
    accent: [f64; 3],
    zoom: Cell<f32>,
    alpha: Cell<u8>,
}

define_class!(
    // SAFETY: NSWindow has no special subclassing requirements; the class adds no ivars and
    // doesn't implement `Drop`.
    #[unsafe(super(NSWindow, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "SnapKitPinWindow"]
    struct PinWindow;

    impl PinWindow {
        /// Borderless windows don't take the keyboard by default; pins need it for Esc and Cmd+C.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        /// A pin never becomes the main window, so the editor keeps that role.
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }
    }
);

define_class!(
    // SAFETY: NSView has no special subclassing requirements; the ivars clean up after
    // themselves and the class doesn't implement `Drop`.
    #[unsafe(super(NSView, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "SnapKitPinView"]
    #[ivars = PinIvars]
    struct PinView;

    impl PinView {
        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        /// A click on a pin of an inactive SnapKit starts the drag right away.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            self.addCursorRect_cursor(self.bounds(), &NSCursor::openHandCursor());
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: CGRect) {
            self.draw();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if event.modifierFlags().contains(NSEventModifierFlags::Control) {
                self.show_menu(event);
            } else if event.clickCount() >= 2 {
                self.request_close();
            } else if let Some(window) = self.window() {
                window.performWindowDragWithEvent(event);
            }
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            self.show_menu(event);
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            if event.buttonNumber() == 2 {
                self.request_close();
            }
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            // The inertia after a trackpad swipe would keep zooming long after the fingers left.
            if !event.momentumPhase().is_empty() {
                return;
            }
            let raw_delta = if event.isDirectionInvertedFromDevice() {
                -event.scrollingDeltaY()
            } else {
                event.scrollingDeltaY()
            };
            let notches = wheel_notches(raw_delta, event.hasPreciseScrollingDeltas());
            if notches == 0.0 {
                return;
            }
            let mods = event.modifierFlags();
            if mods.contains(NSEventModifierFlags::Control) || mods.contains(NSEventModifierFlags::Command) {
                self.set_alpha(step_alpha(self.ivars().alpha.get(), notches));
            } else {
                let zoom = self.ivars().zoom.get() * 1.15_f32.powf(notches);
                self.set_zoom(zoom, Some(platform::cursor_pos()));
            }
        }

        /// Handle Cmd+C even when running as a menu-bar-only (`Accessory`) app without a main menu.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> objc2::runtime::Bool {
            if is_copy_shortcut(event) {
                copy_pin_image(&self.ivars().image);
                return objc2::runtime::Bool::YES;
            }
            // SAFETY: `NSView` implements `performKeyEquivalent:` taking `&NSEvent` and returning `BOOL`.
            unsafe { msg_send![super(self), performKeyEquivalent: event] }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if event.keyCode() == KEY_ESCAPE {
                self.request_close();
                return;
            }
            if is_copy_shortcut(event) {
                copy_pin_image(&self.ivars().image);
                return;
            }
            let key = event.charactersIgnoringModifiers().map(|s| s.to_string().to_lowercase()).unwrap_or_default();
            if key == "0" || key == "1" {
                self.set_zoom(1.0, None);
            }
        }

        /// Edit > Copy (Cmd+C) when the app's menu has one.
        #[unsafe(method(copy:))]
        fn copy_action(&self, _sender: Option<&AnyObject>) {
            copy_pin_image(&self.ivars().image);
        }

        #[unsafe(method(pinMenuAction:))]
        fn menu_action(&self, sender: Option<&NSMenuItem>) {
            let Some(item) = sender else { return };
            match item.tag() {
                CMD_COPY => copy_pin_image(&self.ivars().image),
                CMD_ZOOM_50 => self.set_zoom(0.5, None),
                CMD_ZOOM_100 => self.set_zoom(1.0, None),
                CMD_ZOOM_200 => self.set_zoom(2.0, None),
                CMD_ALPHA_100 => self.set_alpha(255),
                CMD_ALPHA_75 => self.set_alpha(191),
                CMD_ALPHA_50 => self.set_alpha(128),
                CMD_ALPHA_30 => self.set_alpha(77),
                CMD_CLOSE => self.request_close(),
                _ => {}
            }
        }
    }
);

fn is_copy_shortcut(event: &NSEvent) -> bool {
    let mods = event.modifierFlags();
    let key = event.charactersIgnoringModifiers().map(|s| s.to_string().to_lowercase()).unwrap_or_default();
    (mods.contains(NSEventModifierFlags::Command) || mods.contains(NSEventModifierFlags::Control)) && key == "c"
}

impl PinView {
    fn new(mtm: MainThreadMarker, frame: CGRect, ivars: PinIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: `initWithFrame:` is NSView's designated initializer and takes a rectangle.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Accent border over the whole view, then the capture scaled into the area inside it.
    fn draw(&self) {
        let Some(graphics) = NSGraphicsContext::currentContext() else { return };
        let cg = graphics.CGContext();
        let ivars = self.ivars();
        let bounds = self.bounds();
        let [r, g, b] = ivars.accent;
        CGContext::set_rgb_fill_color(Some(&cg), r, g, b, 1.0);
        CGContext::fill_rect(Some(&cg), bounds);
        let border = border_px() as f64 / platform::scale();
        let inner = CGRect::new(
            CGPoint::new(bounds.origin.x + border, bounds.origin.y + border),
            CGSize::new((bounds.size.width - 2.0 * border).max(1.0), (bounds.size.height - 2.0 * border).max(1.0)),
        );
        CGContext::set_interpolation_quality(Some(&cg), CGInterpolationQuality::High);
        CGContext::draw_image(Some(&cg), inner, Some(&ivars.cg_image));
    }

    /// Resizes the window for `zoom`, keeping the point under `anchor` (screen pixels) in place, or
    /// the center without one.
    fn set_zoom(&self, zoom: f32, anchor: Option<(i32, i32)>) {
        let ivars = self.ivars();
        let zoom = clamp_zoom(zoom);
        if (zoom - ivars.zoom.get()).abs() < 1e-4 {
            return;
        }
        let Some(window) = self.window() else { return };
        ivars.zoom.set(zoom);
        let old = frame_to_px_rect(window.frame());
        let size = window_size(ivars.image.width(), ivars.image.height(), zoom, border_px());
        let (x, y) = resized_origin(old, size, anchor);
        window.setFrame_display(px_rect_to_frame((x, y, x + size.0, y + size.1)), true);
        self.setNeedsDisplay(true);
    }

    fn set_alpha(&self, alpha: u8) {
        let alpha = alpha.max(ALPHA_MIN);
        self.ivars().alpha.set(alpha);
        if let Some(window) = self.window() {
            window.setAlphaValue(alpha as f64 / 255.0);
        }
    }

    fn show_menu(&self, event: &NSEvent) {
        let mtm = self.mtm();
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let zoom = self.ivars().zoom.get();
        let alpha = self.ivars().alpha.get();
        let add = |tag: isize, title: &str, checked: bool| {
            // SAFETY: `pinMenuAction:` is implemented by this class and takes the menu item; the
            // item's target is this view, which outlives the menu (it's dropped after tracking).
            let item = unsafe {
                let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(title),
                    Some(sel!(pinMenuAction:)),
                    &NSString::from_str(""),
                );
                item.setTarget(Some(self.as_ref()));
                item
            };
            item.setTag(tag);
            if checked {
                item.setState(NSControlStateValueOn);
            }
            menu.addItem(&item);
        };
        let separator = || menu.addItem(&NSMenuItem::separatorItem(mtm));

        add(CMD_COPY, t!("Copiar imagem (⌘C)", "Copy image (⌘C)"), false);
        separator();
        let near_zoom = |target: f32| (zoom - target).abs() < 0.05;
        add(CMD_ZOOM_50, "Zoom 50%", near_zoom(0.5));
        add(CMD_ZOOM_100, t!("Tamanho real (100%)", "Actual size (100%)"), near_zoom(1.0));
        add(CMD_ZOOM_200, "Zoom 200%", near_zoom(2.0));
        separator();
        let near_alpha = |target: u8| alpha.abs_diff(target) <= 8;
        add(CMD_ALPHA_100, t!("Opacidade 100%", "Opacity 100%"), near_alpha(255));
        add(CMD_ALPHA_75, t!("Opacidade 75%", "Opacity 75%"), near_alpha(191));
        add(CMD_ALPHA_50, t!("Opacidade 50%", "Opacity 50%"), near_alpha(128));
        add(CMD_ALPHA_30, t!("Opacidade 30%", "Opacity 30%"), near_alpha(77));
        separator();
        add(CMD_CLOSE, t!("Fechar (Esc)", "Close (Esc)"), false);

        NSMenu::popUpContextMenu_withEvent_forView(&menu, event, self);
    }

    /// Closes this pin once the current event is handled (closing the window from inside its own
    /// event handler would release it while AppKit is still using it).
    fn request_close(&self) {
        if let Some(window) = self.window() {
            let ptr = Retained::as_ptr(&window) as usize;
            DispatchQueue::main().exec_async(move || close_pin(ptr));
        }
    }
}

/// Closes and releases the pin window at `ptr`, if it's still open. Main thread only.
fn close_pin(ptr: usize) {
    let open = {
        let mut list = active_list();
        let index = list.iter().position(|&p| p == ptr);
        index.map(|i| list.remove(i)).is_some()
    };
    if !open {
        return;
    }
    // SAFETY: the address came from `Retained::into_raw` in `create_pin` and was just taken out of
    // the list, so it's released exactly once; we are on the main thread.
    if let Some(window) = unsafe { Retained::from_raw(ptr as *mut PinWindow) } {
        window.orderOut(None);
        window.close();
    }
}

fn create_pin(mtm: MainThreadMarker, image: RgbaImage, accent: [f64; 3], zoom: f32, frame: CGRect) -> Result<(), String> {
    let cg_image = opaque_cg_image(&image)
        .ok_or_else(|| t!("não foi possível preparar a imagem", "could not prepare the image").to_owned())?;
    let ivars = PinIvars { image, cg_image, accent, zoom: Cell::new(zoom), alpha: Cell::new(255) };

    let this = PinWindow::alloc(mtm).set_ivars(());
    // SAFETY: NSWindow's designated initializer, with a borderless style and buffered backing.
    let window: Retained<PinWindow> = unsafe {
        msg_send![
            super(this),
            initWithContentRect: frame,
            styleMask: NSWindowStyleMask::Borderless,
            backing: NSBackingStoreType::Buffered,
            defer: false,
        ]
    };
    // SAFETY: the window is owned by `ACTIVE_PINS` until `close_pin`, so AppKit must not release
    // it on close.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setLevel(NSFloatingWindowLevel);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    window.setOpaque(true);
    window.setBackgroundColor(Some(&NSColor::blackColor()));
    window.setHasShadow(true);
    window.setHidesOnDeactivate(false);

    let content = CGRect::new(CGPoint::new(0.0, 0.0), frame.size);
    let view = PinView::new(mtm, content, ivars);
    window.setContentView(Some(&view));
    window.makeFirstResponder(Some(&view));
    window.makeKeyAndOrderFront(None);
    window.orderFrontRegardless();

    active_list().push(Retained::into_raw(window) as usize);
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Public API

/// Spawns a new floating always-on-top window showing `image`.
///
/// If `at_rect` is `Some((l, t, r, b))`, the window opens right over that screen region; otherwise
/// it opens near the cursor on the active monitor.
pub fn spawn(image: RgbaImage, accent_rgb: [u8; 3], at_rect: Option<PxRect>) -> Result<(), String> {
    if image.width() == 0 || image.height() == 0 {
        return Err(t!("imagem vazia", "empty image").to_owned());
    }
    let cursor = platform::cursor_pos();
    let work = platform::work_area_at(cursor).unwrap_or_else(|| platform::monitor_rect_at(cursor));
    let (zoom, rect) = initial_placement(image.width(), image.height(), cursor, work, at_rect, border_px());
    let frame = px_rect_to_frame(rect);
    let accent = accent_rgb.map(|c| c as f64 / 255.0);
    run_on_main(move |mtm| create_pin(mtm, image, accent, zoom, frame))
}

/// Closes every active pinned capture window cleanly (called on application shutdown).
pub fn close_all() {
    let open: Vec<usize> = active_list().clone();
    if open.is_empty() {
        return;
    }
    run_on_main(move |_| {
        for ptr in open {
            close_pin(ptr);
        }
    });
}

/// Number of currently open pinned windows.
#[allow(dead_code)]
pub fn active_count() -> usize {
    active_list().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_size_adds_the_border_and_a_minimum_side() {
        assert_eq!(window_size(120, 80, 1.0, BORDER_PX), (124, 84));
        assert_eq!(window_size(120, 80, 2.0, BORDER_PX), (244, 164));
        assert_eq!(window_size(10, 10, 1.0, BORDER_PX), (MIN_PIN_SIDE + 4, MIN_PIN_SIDE + 4));
        assert_eq!(border_px_for(1.0), 2);
        assert_eq!(border_px_for(2.0), 4);
    }

    #[test]
    fn placement_over_the_captured_region_keeps_the_image_in_place() {
        let (zoom, rect) = initial_placement(120, 80, (0, 0), (0, 0, 1000, 800), Some((40, 40, 160, 120)), BORDER_PX);
        assert_eq!(zoom, 1.0);
        assert_eq!(rect, (38, 38, 162, 122));
    }

    #[test]
    fn placement_near_the_cursor_stays_inside_the_work_area() {
        let work = (0, 50, 1000, 800);
        let (zoom, rect) = initial_placement(200, 100, (500, 400), work, None, BORDER_PX);
        assert_eq!(zoom, 1.0);
        assert_eq!(rect, (398, 348, 602, 452));
        // Near a corner it's pushed back inside, 8 px from the edges.
        let (_, rect) = initial_placement(200, 100, (5, 60), work, None, BORDER_PX);
        assert_eq!((rect.0, rect.1), (8, 58));
        // Too big for the work area: shrunk to fit 85% of it.
        let (zoom, rect) = initial_placement(2000, 500, (500, 400), work, None, BORDER_PX);
        assert!(zoom < 1.0 && zoom >= ZOOM_MIN);
        assert!(rect.2 - rect.0 <= 850 + BORDER_PX * 2);
        // Never below the minimum zoom, even if it then overflows.
        let (zoom, _) = initial_placement(8000, 1000, (500, 400), work, None, BORDER_PX);
        assert_eq!(zoom, ZOOM_MIN);
    }

    #[test]
    fn zoom_and_opacity_are_clamped() {
        assert_eq!(clamp_zoom(0.01), ZOOM_MIN);
        assert_eq!(clamp_zoom(10.0), ZOOM_MAX);
        assert_eq!(clamp_zoom(1.5), 1.5);
        assert_eq!(step_alpha(255, 1.0), 255);
        assert_eq!(step_alpha(255, -1.0), 235);
        assert_eq!(step_alpha(60, -3.0), ALPHA_MIN);
        assert_eq!(wheel_notches(1.0, false), 1.0);
        assert_eq!(wheel_notches(15.0, true), 0.5);
        assert_eq!(wheel_notches(-500.0, true), -3.0);
    }

    #[test]
    fn resizing_keeps_the_anchor_or_the_center_in_place() {
        let old = (100, 100, 300, 200);
        // The cursor at the window's center stays at the center of the new size.
        assert_eq!(resized_origin(old, (400, 200), Some((200, 150))), (0, 50));
        // The cursor at the top-left corner keeps that corner fixed.
        assert_eq!(resized_origin(old, (400, 200), Some((100, 100))), (100, 100));
        assert_eq!(resized_origin(old, (100, 50), None), (150, 125));
    }

    #[test]
    fn px_rect_survives_round_trip_through_frame() {
        let s = platform::scale();
        let px = |v: f64| (v * s).round() as i32;
        let r = (px(10.0), px(20.0), px(110.0), px(220.0));
        assert_eq!(frame_to_px_rect(px_rect_to_frame(r)), r);
    }

    #[test]
    fn opaque_cg_image_composites_alpha_and_preserves_dimensions() {
        let mut img = RgbaImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        img.put_pixel(1, 0, image::Rgba([100, 150, 200, 255]));
        let cg = opaque_cg_image(&img).expect("opaque cg image");
        assert_eq!(CGImage::width(Some(&cg)), 2);
        assert_eq!(CGImage::height(Some(&cg)), 1);

        let space = CGColorSpace::new_device_rgb().expect("device rgb");
        // SAFETY: null data lets CoreGraphics allocate a 2x1 RGBA buffer owned by `ctx`.
        let ctx = unsafe {
            CGBitmapContextCreate(
                std::ptr::null_mut(),
                2,
                1,
                8,
                8,
                Some(&space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .expect("bitmap context");
        CGContext::draw_image(Some(&ctx), CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(2.0, 1.0)), Some(&cg));
        let ptr = CGBitmapContextGetData(Some(&ctx)).cast::<u8>();
        assert!(!ptr.is_null());
        // SAFETY: `ctx` is a 2x1 RGBA context (8 bytes), alive for the duration of this slice.
        let bytes = unsafe { std::slice::from_raw_parts(ptr, 8) };
        assert_eq!(&bytes[0..4], &[34, 30, 30, 255]);
        assert_eq!(&bytes[4..8], &[100, 150, 200, 255]);
    }
}
