//! macOS implementation of the platform API, on AppKit and CoreGraphics.
//!
//! Coordinates: CoreGraphics uses global display *points* with the origin at the top-left corner of
//! the main display, which is the layout SnapKit expects. To keep every pixel of a Retina screen,
//! points are multiplied by [`scale`], the largest backing factor among the connected displays
//! (usually 2). AppKit window frames use the bottom-left corner of the main display as origin and
//! are flipped on the way in and out.
//!
//! Window handles (`isize`) are `NSWindow` pointers. AppKit objects may only be touched on the main
//! thread, so the functions called from the capture and recording threads hop to it with
//! [`run_on_main`] (synchronously: the main thread never waits on those threads).

use std::ffi::c_void;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dispatch2::run_on_main;
use image::RgbaImage;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSCursor,
    NSNormalWindowLevel, NSPasteboard, NSPasteboardWriting, NSPopUpMenuWindowLevel,
    NSRunningApplication, NSScreen, NSStatusWindowLevel, NSView, NSWindow,
    NSWindowCollectionBehavior, NSWindowSharingType, NSWindowStyleMask, NSWorkspace,
};
use objc2_core_foundation::{CFArray, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGDirectDisplayID,
    CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayMode, CGEvent, CGEventSource,
    CGEventSourceStateID, CGEventTapLocation, CGGetActiveDisplayList, CGImage, CGImageAlphaInfo,
    CGMainDisplayID, CGMouseButton, CGScrollEventUnit, CGWarpMouseCursorPosition,
    CGWindowImageOption, CGWindowListCopyWindowInfo, CGWindowListOption,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSURL};
use raw_window_handle::RawWindowHandle;

use super::{InstanceRequest, PxRect};

// ---------------------------------------------------------------------------------------------
// Displays and coordinates

fn displays() -> Vec<CGDirectDisplayID> {
    let mut ids = [0 as CGDirectDisplayID; 32];
    let mut count = 0u32;
    // SAFETY: the buffer holds 32 ids, the number passed; `count` is a valid u32 to write to.
    unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    ids[..(count as usize).min(ids.len())].to_vec()
}

/// Pixels per point of one display (2 on Retina screens).
fn display_scale(id: CGDirectDisplayID) -> f64 {
    let width = CGDisplayBounds(id).size.width;
    let mode = CGDisplayCopyDisplayMode(id);
    let pixels = CGDisplayMode::pixel_width(mode.as_deref());
    if width > 0.0 && pixels > 0 { pixels as f64 / width } else { 1.0 }
}

/// Screen pixels per point: the largest factor among the connected displays. Cached for a second,
/// because it's read many times per frame and displays rarely change.
pub fn scale() -> f64 {
    static CACHE: Mutex<Option<(Instant, f64)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, s)) = *cache
        && at.elapsed() < Duration::from_secs(1)
    {
        return s;
    }
    let s = displays().into_iter().map(display_scale).fold(1.0, f64::max).clamp(1.0, 4.0);
    *cache = Some((Instant::now(), s));
    s
}

fn px(v: f64, s: f64) -> i32 {
    (v * s).round() as i32
}

/// Rectangle in global points (top-left origin) to screen pixels.
fn pt_rect_to_px(r: CGRect) -> PxRect {
    let s = scale();
    (
        px(r.origin.x, s),
        px(r.origin.y, s),
        px(r.origin.x + r.size.width, s),
        px(r.origin.y + r.size.height, s),
    )
}

/// Screen pixels to a rectangle in global points (top-left origin).
fn px_rect_to_pt(r: PxRect) -> CGRect {
    let s = scale();
    CGRect::new(
        CGPoint::new(r.0 as f64 / s, r.1 as f64 / s),
        CGSize::new((r.2 - r.0) as f64 / s, (r.3 - r.1) as f64 / s),
    )
}

/// Converts between AppKit frames (bottom-left origin) and global points (top-left origin). The
/// conversion is its own inverse.
fn flip(r: CGRect) -> CGRect {
    let main_height = CGDisplayBounds(CGMainDisplayID()).size.height;
    CGRect::new(CGPoint::new(r.origin.x, main_height - r.origin.y - r.size.height), r.size)
}

fn contains_pt(r: CGRect, p: CGPoint) -> bool {
    p.x >= r.origin.x && p.x < r.origin.x + r.size.width && p.y >= r.origin.y && p.y < r.origin.y + r.size.height
}

/// Rectangle that encloses every monitor together.
pub fn virtual_screen() -> PxRect {
    displays()
        .into_iter()
        .map(|d| pt_rect_to_px(CGDisplayBounds(d)))
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
        .unwrap_or((0, 0, 1440, 900))
}

pub fn monitor_count() -> i32 {
    displays().len() as i32
}

/// The monitor that contains the point (or the main one).
pub fn monitor_rect_at(p: (i32, i32)) -> PxRect {
    let s = scale();
    let pt = CGPoint::new(p.0 as f64 / s, p.1 as f64 / s);
    let id = displays().into_iter().find(|&d| contains_pt(CGDisplayBounds(d), pt)).unwrap_or(CGMainDisplayID());
    pt_rect_to_px(CGDisplayBounds(id))
}

/// Area the selection layer covers. On macOS each display has its own Space, and a window that
/// spans two displays only shows on one of them, so the layer covers the monitor under the cursor.
pub fn selection_area() -> PxRect {
    monitor_rect_at(cursor_pos())
}

/// Nothing to do: macOS reports coordinates the same way to every process.
pub fn enable_per_monitor_dpi() {}

/// Screen pixels per interface point, the same everywhere with SnapKit's coordinate model.
pub fn dpi_scale_at(_p: (i32, i32)) -> f32 {
    scale() as f32
}

/// Work area (without the menu bar and the Dock) of the monitor that contains the point.
pub fn work_area_at(p: (i32, i32)) -> Option<PxRect> {
    let s = scale();
    let pt = CGPoint::new(p.0 as f64 / s, p.1 as f64 / s);
    let visible = run_on_main(move |mtm| {
        let screens = NSScreen::screens(mtm);
        let screen = screens.iter().find(|sc| contains_pt(flip(sc.frame()), pt)).or_else(|| screens.firstObject())?;
        Some(flip(screen.visibleFrame()))
    })?;
    Some(pt_rect_to_px(visible))
}

// ---------------------------------------------------------------------------------------------
// Mouse and keyboard

pub fn cursor_pos() -> (i32, i32) {
    let event = CGEvent::new(None);
    let p = CGEvent::location(event.as_deref());
    let s = scale();
    (px(p.x, s), px(p.y, s))
}

pub fn set_cursor_pos(p: (i32, i32)) {
    let s = scale();
    let _ = CGWarpMouseCursorPosition(CGPoint::new(p.0 as f64 / s, p.1 as f64 / s));
}

/// Lines scrolled per wheel notch, like the default on Windows.
const LINES_PER_NOTCH: i32 = 3;

/// Turns the mouse wheel: positive scrolls up, negative down. The event goes to the window under
/// the cursor. Needs the Accessibility permission (see [`ensure_input_permission`]).
pub fn wheel(notches: i32) {
    post_scroll(notches * LINES_PER_NOTCH, 0);
}

/// Horizontal wheel: positive scrolls right, negative left.
pub fn hwheel(notches: i32) {
    // For CoreGraphics a positive horizontal delta scrolls left.
    post_scroll(0, -notches * LINES_PER_NOTCH);
}

fn post_scroll(vertical: i32, horizontal: i32) {
    let event = CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Line, 2, vertical, horizontal, 0);
    if event.is_some() {
        CGEvent::post(CGEventTapLocation::HIDEventTap, event.as_deref());
    }
}

fn key_down(code: u16) -> bool {
    CGEventSource::key_state(CGEventSourceStateID::CombinedSessionState, code)
}

/// Is the Esc key pressed right now?
pub fn escape_pressed() -> bool {
    key_down(0x35) // kVK_Escape
}

/// Is the left mouse button down right now, in any program?
pub fn left_button_down() -> bool {
    CGEventSource::button_state(CGEventSourceStateID::CombinedSessionState, CGMouseButton::Left)
}

pub fn right_button_down() -> bool {
    CGEventSource::button_state(CGEventSourceStateID::CombinedSessionState, CGMouseButton::Right)
}

/// Is the Return (or keypad Enter) key down right now, in any program?
pub fn enter_down() -> bool {
    key_down(0x24) || key_down(0x4C) // kVK_Return, kVK_ANSI_KeypadEnter
}

/// Asks for the Accessibility permission that synthesized scrolling needs (the scrolling capture).
/// Returns whether it's granted; the first time, macOS shows its own prompt.
pub fn ensure_input_permission() -> bool {
    objc2_core_graphics::CGPreflightPostEventAccess() || objc2_core_graphics::CGRequestPostEventAccess()
}

/// Asks for the Input Monitoring permission, which reading the keyboard state of the whole system
/// needs ([`escape_pressed`], [`enter_down`]): Esc stopping the scrolling capture, Enter as a
/// step. The mouse buttons don't need it. Returns whether it's granted; the first time, macOS
/// shows its own prompt.
pub fn ensure_key_permission() -> bool {
    key_permission() || objc2_core_graphics::CGRequestListenEventAccess()
}

/// Is the Input Monitoring permission granted? Unlike [`ensure_key_permission`], never shows the
/// system prompt (which would land in the middle of a capture).
pub fn key_permission() -> bool {
    objc2_core_graphics::CGPreflightListenEventAccess()
}

/// Asks for the Screen Recording permission. Without it, captures only show the wallpaper and
/// the menu bar. The first time, macOS shows its own prompt (it applies after SnapKit restarts).
pub fn ensure_screen_permission() -> bool {
    objc2_core_graphics::CGPreflightScreenCaptureAccess() || objc2_core_graphics::CGRequestScreenCaptureAccess()
}

// ---------------------------------------------------------------------------------------------
// Other programs' windows

struct WindowInfo {
    pid: i32,
    /// Global points, top-left origin.
    bounds: CGRect,
    title: String,
    owner: String,
}

fn number(dict: &NSDictionary<NSString, AnyObject>, key: &str) -> Option<f64> {
    let value = dict.objectForKey(&NSString::from_str(key))?;
    value.downcast::<NSNumber>().ok().map(|n| n.doubleValue())
}

fn text(dict: &NSDictionary<NSString, AnyObject>, key: &str) -> String {
    dict.objectForKey(&NSString::from_str(key))
        .and_then(|v| v.downcast::<NSString>().ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// The normal windows on screen (layer 0), from the frontmost to the backmost.
fn window_list() -> Vec<WindowInfo> {
    let options = CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(options, 0) else {
        return Vec::new();
    };
    // SAFETY: CFArray and NSArray are toll-free bridged; each element is a dictionary of
    // string keys, as documented for CGWindowListCopyWindowInfo.
    let list: &NSArray<NSDictionary<NSString, AnyObject>> = unsafe { &*(&*list as *const CFArray).cast() };
    let mut out = Vec::new();
    for dict in list.iter() {
        if number(&dict, "kCGWindowLayer") != Some(0.0) || number(&dict, "kCGWindowAlpha").is_some_and(|a| a <= 0.0) {
            continue;
        }
        let Some(bounds) = dict
            .objectForKey(&NSString::from_str("kCGWindowBounds"))
            .and_then(|b| b.downcast::<NSDictionary>().ok())
        else {
            continue;
        };
        // SAFETY: the bounds dictionary has string keys (X, Y, Width, Height).
        let bounds: &NSDictionary<NSString, AnyObject> = unsafe { &*(&*bounds as *const NSDictionary).cast() };
        let get = |k: &str| number(bounds, k).unwrap_or(0.0);
        out.push(WindowInfo {
            pid: number(&dict, "kCGWindowOwnerPID").unwrap_or(0.0) as i32,
            bounds: CGRect::new(CGPoint::new(get("X"), get("Y")), CGSize::new(get("Width"), get("Height"))),
            title: text(&dict, "kCGWindowName"),
            owner: text(&dict, "kCGWindowOwnerName"),
        });
    }
    out
}

fn own_pid() -> i32 {
    std::process::id() as i32
}

fn frontmost_pid() -> Option<i32> {
    NSWorkspace::sharedWorkspace().frontmostApplication().map(|app| app.processIdentifier())
}

/// Visible top-level windows of other programs, from the frontmost to the backmost.
pub fn visible_windows(_skip: isize) -> Vec<PxRect> {
    let me = own_pid();
    window_list()
        .into_iter()
        .filter(|w| w.pid != me)
        .map(|w| pt_rect_to_px(w.bounds))
        .filter(|r| r.2 - r.0 >= 8 && r.3 - r.1 >= 8)
        .collect()
}

/// Title of the foreground window (empty if there's none or it has no title).
pub fn foreground_window_title() -> String {
    foreground_window_title_except(0)
}

/// Title of the foreground window, ignoring `skip` (our own window). Without the Screen Recording
/// permission macOS hides window titles; the program name is used then.
pub fn foreground_window_title_except(skip: isize) -> String {
    let Some(pid) = frontmost_pid() else { return String::new() };
    if skip != 0 && pid == own_pid() {
        return String::new();
    }
    window_list()
        .into_iter()
        .find(|w| w.pid == pid)
        .map(|w| if w.title.is_empty() { w.owner } else { w.title })
        .unwrap_or_default()
}

/// Rectangle of the foreground window of another program, if there is one.
pub fn foreground_window_rect() -> Option<PxRect> {
    let pid = frontmost_pid().filter(|&p| p != own_pid())?;
    window_list().into_iter().find(|w| w.pid == pid).map(|w| pt_rect_to_px(w.bounds))
}

/// Is our window `hwnd` the one receiving the keyboard?
pub fn is_foreground(hwnd: isize) -> bool {
    with_window(hwnd, |w, mtm| NSApplication::sharedApplication(mtm).isActive() && w.isKeyWindow())
}

/// Brings the program whose window is at the given point to the front.
pub fn focus_window_at(p: (i32, i32)) {
    let s = scale();
    let pt = CGPoint::new(p.0 as f64 / s, p.1 as f64 / s);
    let me = own_pid();
    if let Some(w) = window_list().into_iter().find(|w| w.pid != me && contains_pt(w.bounds, pt))
        && let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(w.pid)
    {
        #[allow(deprecated)]
        app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    }
}

// ---------------------------------------------------------------------------------------------
// Screen pixels

/// Photographs the rectangle `rect` of the screen, in screen pixels, across monitors. Our windows
/// marked with [`exclude_from_capture`] are left out.
pub fn capture_px(rect: PxRect) -> Result<RgbaImage, String> {
    let (w, h) = (rect.2 - rect.0, rect.3 - rect.1);
    if w <= 0 || h <= 0 {
        return Err(t!("área vazia", "empty area").to_owned());
    }
    #[allow(deprecated)]
    let image = objc2_core_graphics::CGWindowListCreateImage(
        px_rect_to_pt(rect),
        CGWindowListOption::OptionOnScreenOnly,
        0,
        CGWindowImageOption::BestResolution,
    )
    .ok_or_else(|| {
        t!(
            "o macOS não entregou a imagem da tela (permita o SnapKit em Ajustes do Sistema > Privacidade e Segurança > Gravação de Tela)",
            "macOS didn't provide the screen image (allow SnapKit in System Settings > Privacy & Security > Screen Recording)"
        )
        .to_owned()
    })?;
    let img = cg_image_to_rgba(&image).ok_or_else(|| t!("imagem da tela ilegível", "unreadable screen image").to_owned())?;
    Ok(if (img.width() as i32, img.height() as i32) == (w, h) {
        img
    } else {
        // A display with fewer pixels per point than the largest one: scaled up to the common grid.
        image::imageops::resize(&img, w as u32, h as u32, image::imageops::FilterType::CatmullRom)
    })
}

/// Reads any CGImage as straight (not premultiplied) RGBA, by drawing it into a known format.
fn cg_image_to_rgba(image: &CGImage) -> Option<RgbaImage> {
    let (w, h) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    if w == 0 || h == 0 {
        return None;
    }
    let mut buf = draw_rgba(image, w, h)?;
    unpremultiply(&mut buf);
    RgbaImage::from_raw(w as u32, h as u32, buf)
}

/// Draws `image` into a `w`×`h` premultiplied RGBA buffer.
fn draw_rgba(image: &CGImage, w: usize, h: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; w * h * 4];
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: `buf` holds `h` rows of `w * 4` bytes and outlives the context, which is dropped at
    // the end of this function.
    let context = unsafe {
        CGBitmapContextCreate(
            buf.as_mut_ptr().cast::<c_void>(),
            w,
            h,
            8,
            w * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }?;
    CGContext::draw_image(Some(&context), CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64)), Some(image));
    drop(context);
    Some(buf)
}

fn unpremultiply(buf: &mut [u8]) {
    for p in buf.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

/// Reads a small block of pixels straight from the screen (RGB, row by row), for the selection
/// magnifier.
pub fn grab_pixels(x: i32, y: i32, w: i32, h: i32) -> Option<Vec<[u8; 3]>> {
    if w <= 0 || h <= 0 || w > 256 || h > 256 {
        return None;
    }
    let img = capture_px((x, y, x + w, y + h)).ok()?;
    Some(img.pixels().map(|p| [p.0[0], p.0[1], p.0[2]]).collect())
}

/// Image of the current cursor and the top-left corner where it should be drawn. Returns `None` if
/// it can't be read.
pub fn cursor_image() -> Option<(RgbaImage, (i32, i32))> {
    let s = scale();
    let (image, hotspot) = run_on_main(move |_mtm| {
        #[allow(deprecated)]
        let cursor = NSCursor::currentSystemCursor()?;
        let image = cursor.image();
        let size = image.size();
        let hot = cursor.hotSpot();
        let (w, h) = ((size.width * s).round() as usize, (size.height * s).round() as usize);
        if w == 0 || h == 0 || w > 512 || h > 512 {
            return None;
        }
        // SAFETY: a null proposed rect means the whole image; no context or hints.
        let cg = unsafe { image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }?;
        let mut buf = draw_rgba(&cg, w, h)?;
        unpremultiply(&mut buf);
        Some((RgbaImage::from_raw(w as u32, h as u32, buf)?, (hot.x * s, hot.y * s)))
    })?;
    let (cx, cy) = cursor_pos();
    Some((image, (cx - hotspot.0.round() as i32, cy - hotspot.1.round() as i32)))
}

// ---------------------------------------------------------------------------------------------
// Our window

/// Handle of our main window, from what winit reports.
pub fn window_id(handle: RawWindowHandle) -> isize {
    let RawWindowHandle::AppKit(h) = handle else { return 0 };
    // SAFETY: winit gives the pointer of the window's NSView, alive while the window is; we are on
    // the main thread (the app is being created). The NSWindow is retained for the whole run.
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    view.window().map(|w| Retained::into_raw(w) as isize).unwrap_or(0)
}

/// Runs `f` with our window on the main thread. Returns the default value if there's no window.
fn with_window<R: Send + Default>(hwnd: isize, f: impl FnOnce(&NSWindow, MainThreadMarker) -> R + Send) -> R {
    if hwnd == 0 {
        return R::default();
    }
    run_on_main(move |mtm| {
        // SAFETY: `hwnd` comes from `window_id`, a window retained for the whole run.
        let window = unsafe { &*(hwnd as *const NSWindow) };
        f(window, mtm)
    })
}

/// Keeps the window out of screen captures (or back to normal).
pub fn exclude_from_capture(hwnd: isize, exclude: bool) {
    if std::env::var_os("SNAPKIT_DEBUG_VISIBLE").is_some() {
        return;
    }
    let sharing = if exclude { NSWindowSharingType::None } else { NSWindowSharingType::ReadOnly };
    with_window(hwnd, move |w, _| w.setSharingType(sharing));
}

/// Top-left corner of the window, in screen coordinates.
pub fn window_pos(hwnd: isize) -> Option<(i32, i32)> {
    window_rect(hwnd).map(|r| (r.0, r.1))
}

/// Rectangle of the whole window, in screen coordinates.
pub fn window_rect(hwnd: isize) -> Option<PxRect> {
    with_window(hwnd, |w, _| Some(w.frame())).map(|f| pt_rect_to_px(flip(f)))
}

/// Moves the window without changing its size.
pub fn set_window_pos(hwnd: isize, p: (i32, i32)) {
    let s = scale();
    with_window(hwnd, move |w, _| {
        let size = w.frame().size;
        let top_left = CGRect::new(CGPoint::new(p.0 as f64 / s, p.1 as f64 / s), size);
        w.setFrame_display(flip(top_left), true);
    });
}

/// Moves and resizes the window.
pub fn set_window_rect(hwnd: isize, r: PxRect) {
    let frame = flip(px_rect_to_pt(r));
    with_window(hwnd, move |w, _| w.setFrame_display(frame, true));
}

/// Where on the screen the top-left corner of the window's content area is.
pub fn client_origin(hwnd: isize) -> Option<(i32, i32)> {
    client_rect_screen(hwnd).map(|r| (r.0, r.1))
}

/// Content area of the window, in screen coordinates.
pub fn client_rect_screen(hwnd: isize) -> Option<PxRect> {
    let content = with_window(hwnd, |w, _| Some(w.contentRectForFrameRect(w.frame())))?;
    let r = pt_rect_to_px(flip(content));
    (r.2 > r.0 && r.3 > r.1).then_some(r)
}

/// Makes the window invisible without hiding it (or shows it again): it keeps drawing and
/// receiving commands. Used to change size and frame without the intermediate stages showing.
pub fn cloak(hwnd: isize, on: bool) {
    with_window(hwnd, move |w, _| w.setAlphaValue(if on { 0.0 } else { 1.0 }));
}

/// Matches the window's title bar (light or dark) to the active SnapKit theme.
pub fn apply_titlebar_theme(hwnd: isize, dark: bool, _bg: [u8; 3], _fg: [u8; 3]) {
    with_window(hwnd, move |w, _| {
        // SAFETY: the appearance names are constants provided by AppKit.
        let name = unsafe { if dark { NSAppearanceNameDarkAqua } else { NSAppearanceNameAqua } };
        let appearance = NSAppearance::appearanceNamed(name);
        w.setAppearance(appearance.as_deref());
    });
}

/// Is the window showing on the screen (neither hidden nor minimized)?
pub fn is_on_screen(hwnd: isize) -> bool {
    with_window(hwnd, |w, _| w.isVisible() && !w.isMiniaturized())
}

/// Is the window put away: hidden, and not just minimized to the Dock?
pub fn is_hidden(hwnd: isize) -> bool {
    with_window(hwnd, |w, _| !w.isVisible() && !w.isMiniaturized())
}

pub fn minimize(hwnd: isize) {
    with_window(hwnd, |w, _| {
        // A borderless window (the selection layer) can't go to the Dock; it's hidden instead.
        if w.styleMask().contains(NSWindowStyleMask::Miniaturizable) {
            w.miniaturize(None);
        } else {
            w.orderOut(None);
        }
    });
}

/// Hides the window completely.
pub fn hide(hwnd: isize) {
    with_window(hwnd, |w, _| w.orderOut(None));
}

/// Brings the window back, whether it's hidden or minimized, and puts it in the foreground.
pub fn show(hwnd: isize) {
    with_window(hwnd, |w, mtm| {
        if w.isMiniaturized() {
            w.deminiaturize(None);
        }
        w.makeKeyAndOrderFront(None);
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    });
}

/// Turns the window into the selection layer (or back to a normal window): above the menu bar
/// and the Dock, on every Space, over full-screen apps, and out of the window cycle.
pub fn set_tool_window(hwnd: isize, on: bool) {
    with_window(hwnd, move |w, _| {
        let (layer, normal) = (NSPopUpMenuWindowLevel, NSNormalWindowLevel);
        let want = if on { layer } else { normal };
        if w.level() == want {
            return;
        }
        w.setLevel(want);
        w.setHasShadow(!on);
        w.setCollectionBehavior(if on {
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
        } else {
            NSWindowCollectionBehavior::Default
        });
    });
}

/// Shows or removes SnapKit's icon in the Dock (and its menu bar). Put away in the menu bar, the
/// program has no window on screen and shouldn't linger in the Dock.
pub fn set_dock_icon(visible: bool) {
    let policy = if visible { NSApplicationActivationPolicy::Regular } else { NSApplicationActivationPolicy::Accessory };
    run_on_main(move |mtm| {
        let app = NSApplication::sharedApplication(mtm);
        if app.activationPolicy() == policy {
            return;
        }
        app.setActivationPolicy(policy);
        if visible {
            // Coming back as a regular app, it has to be activated again to get its menu bar.
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
    });
}

// ---------------------------------------------------------------------------------------------
// Messages, clipboard, drag

/// Shows a blocking error message.
pub fn error_box(title: &str, text: &str) {
    if let Some(mtm) = MainThreadMarker::new() {
        let _ = NSApplication::sharedApplication(mtm);
        let alert = NSAlert::new(mtm);
        alert.setAlertStyle(NSAlertStyle::Critical);
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(text));
        alert.runModal();
        return;
    }
    // From another thread AppKit can't be used; AppleScript shows the same kind of alert.
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let script = format!("display alert {} message {} as critical", quote(title), quote(text));
    let _ = std::process::Command::new("osascript").args(["-e", &script]).status();
}

/// Puts the files on the clipboard, as Finder does when copying them.
pub fn copy_files_to_clipboard(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("No files provided".into());
    }
    let paths: Vec<String> = paths
        .iter()
        .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()).to_string_lossy().into_owned())
        .collect();
    let ok = run_on_main(move |_| {
        let board = NSPasteboard::generalPasteboard();
        board.clearContents();
        let urls: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
            .iter()
            .map(|p| ProtocolObject::from_retained(NSURL::fileURLWithPath(&NSString::from_str(p))))
            .collect();
        board.writeObjects(&NSArray::from_retained_slice(&urls))
    });
    if ok { Ok(()) } else { Err(t!("a área de transferência recusou os arquivos", "the clipboard refused the files").to_owned()) }
}

#[path = "macos_drag.rs"]
mod drag;
pub use drag::start_file_drag;

// ---------------------------------------------------------------------------------------------
// Time and child processes

/// Local date and time: (year, month, day, hour, minute, second).
pub fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    // SAFETY: `time` with a null pointer only returns the time; `tm` is plain numbers, so all zeros
    // is valid, and `localtime_r` fills it in.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        (
            (tm.tm_year + 1900) as u16,
            (tm.tm_mon + 1) as u16,
            tm.tm_mday as u16,
            tm.tm_hour as u16,
            tm.tm_min as u16,
            tm.tm_sec as u16,
        )
    }
}

static CHILDREN: Mutex<Vec<i32>> = Mutex::new(Vec::new());

extern "C" fn end_children() {
    if let Ok(list) = CHILDREN.try_lock() {
        for &pid in list.iter() {
            // SAFETY: only sends a signal; FFmpeg finishes the file it's writing on SIGTERM.
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
    }
}

/// Makes `child` end together with SnapKit: if SnapKit quits during a recording, FFmpeg would go
/// on recording the screen in the background. The children still running when SnapKit exits get
/// SIGTERM.
pub fn end_with_us(child: &std::process::Child) {
    static HOOK: std::sync::Once = std::sync::Once::new();
    // SAFETY: registers a plain function to run at exit.
    HOOK.call_once(|| unsafe {
        libc::atexit(end_children);
    });
    let mut list = CHILDREN.lock().unwrap_or_else(|e| e.into_inner());
    // Forget the ones that have already ended (signal 0 only checks that the process exists).
    // SAFETY: signal 0 is never delivered.
    list.retain(|&pid| unsafe { libc::kill(pid, 0) } == 0);
    list.push(child.id() as i32);
}

// ---------------------------------------------------------------------------------------------
// Single instance

/// The right to be the only SnapKit running with these preferences, plus the socket that a second
/// copy uses to talk to it. The lock is a `flock` on a file, released by the system even if
/// SnapKit crashes.
pub struct Instance {
    _lock: std::fs::File,
    listener: UnixListener,
    quit_requested: AtomicBool,
}

fn instance_paths(key: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir();
    (dir.join(format!("SnapKit-{key}.lock")), dir.join(format!("SnapKit-{key}.sock")))
}

fn try_lock(path: &std::path::Path) -> Option<std::fs::File> {
    use std::os::fd::AsRawFd;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(path).ok()?;
    // SAFETY: a valid open descriptor; the lock lives as long as the file.
    (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(file)
}

impl Instance {
    /// Becomes the running copy, or returns `None` if another one already is.
    pub fn claim(key: &str) -> Option<Self> {
        let (lock_path, socket) = instance_paths(key);
        let lock = try_lock(&lock_path)?;
        // Whoever held the lock is gone; its socket file is stale.
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).ok()?;
        Some(Self { _lock: lock, listener, quit_requested: AtomicBool::new(false) })
    }

    /// Waits for the next request from a second copy.
    pub fn wait(&self) -> Option<InstanceRequest> {
        loop {
            let (mut stream, _) = self.listener.accept().ok()?;
            let mut byte = [0u8; 1];
            if stream.read_exact(&mut byte).is_err() {
                continue;
            }
            match byte[0] {
                b'S' => return Some(InstanceRequest::Show),
                b'R' => return Some(InstanceRequest::Select),
                b'Q' => {
                    self.quit_requested.store(true, Ordering::SeqCst);
                    return Some(InstanceRequest::Quit);
                }
                _ => continue,
            }
        }
    }

    /// True once if another copy asked this one to quit.
    pub fn take_quit_request(&self) -> bool {
        self.quit_requested.swap(false, Ordering::SeqCst)
    }

    /// From a second copy: passes the request on to the running one. Returns `false` if it
    /// couldn't be reached.
    pub fn send(key: &str, request: InstanceRequest) -> bool {
        let (lock_path, socket) = instance_paths(key);
        let byte = match request {
            InstanceRequest::Show => b'S',
            InstanceRequest::Select => b'R',
            InstanceRequest::Quit => b'Q',
        };
        // The running copy may still be starting: it opens the socket right after the lock.
        let mut sent = false;
        for _ in 0..20 {
            if let Ok(mut stream) = UnixStream::connect(&socket)
                && stream.write_all(&[byte]).is_ok()
            {
                sent = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if sent && request == InstanceRequest::Quit {
            // Wait up to 5 s for the running instance to flush writes and exit.
            for _ in 0..50 {
                if try_lock(&lock_path).is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        sent
    }
}

// ---------------------------------------------------------------------------------------------
// Overlays

fn overlay_behavior() -> NSWindowCollectionBehavior {
    NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::Stationary
        | NSWindowCollectionBehavior::IgnoresCycle
        | NSWindowCollectionBehavior::FullScreenAuxiliary
}

/// A borderless window that clicks go through, above normal windows.
fn overlay_window(mtm: MainThreadMarker, frame_pt: CGRect, color: &NSColor) -> Retained<NSWindow> {
    // SAFETY: a plain borderless window created on the main thread; it's kept alive by the
    // returned reference (not released when closed).
    let window = unsafe {
        let w = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            flip(frame_pt),
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        );
        w.setReleasedWhenClosed(false);
        w
    };
    window.setBackgroundColor(Some(color));
    window.setOpaque(true);
    window.setHasShadow(false);
    window.setIgnoresMouseEvents(true);
    window.setLevel(NSStatusWindowLevel);
    window.setCollectionBehavior(overlay_behavior());
    window
}

/// Colored frame around an area of the screen, to show what's being captured.
///
/// It's four thin windows, always on top, that don't receive clicks and don't show in screen
/// captures. They sit outside the area. It disappears when the value is dropped.
pub struct RegionFrame {
    /// The windows, as retained pointers (AppKit objects can't cross threads).
    windows: Vec<usize>,
}

impl RegionFrame {
    pub fn show(rect: PxRect) -> Option<Self> {
        const THICKNESS: i32 = 4;
        let t = (THICKNESS as f64 * scale()).round() as i32;
        let (l, top, r, b) = rect;
        let bars = [
            (l - t, top - t, r + t, top),
            (l - t, b, r + t, b + t),
            (l - t, top, l, b),
            (r, top, r + t, b),
        ];
        let frames: Vec<CGRect> = bars.iter().map(|&bar| px_rect_to_pt(bar)).collect();
        let windows = run_on_main(move |mtm| {
            // SnapKit's orange (255, 138, 76).
            let color = NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 138.0 / 255.0, 76.0 / 255.0, 1.0);
            frames
                .into_iter()
                .map(|f| {
                    let w = overlay_window(mtm, f, &color);
                    w.setSharingType(NSWindowSharingType::None);
                    w.orderFrontRegardless();
                    Retained::into_raw(w) as usize
                })
                .collect::<Vec<_>>()
        });
        Some(Self { windows })
    }
}

impl Drop for RegionFrame {
    fn drop(&mut self) {
        let windows = std::mem::take(&mut self.windows);
        run_on_main(move |_| {
            for ptr in windows {
                // SAFETY: each pointer came from `Retained::into_raw` in `show` and is used once.
                if let Some(w) = unsafe { Retained::from_raw(ptr as *mut NSWindow) } {
                    w.orderOut(None);
                    w.close();
                }
            }
        });
    }
}

/// Ring shown around mouse clicks during a recording. On macOS FFmpeg draws the clicks itself
/// (`-capture_mouse_clicks`), so this only stands in for the Windows window.
pub struct ClickHalo;

impl ClickHalo {
    pub fn show(_rect: PxRect, _skip_hwnd: isize) -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_survive_the_round_trip_through_points() {
        let s = scale();
        let r = (px(10.0, s), px(20.0, s), px(110.0, s), px(220.0, s));
        assert_eq!(pt_rect_to_px(px_rect_to_pt(r)), r);
        let f = CGRect::new(CGPoint::new(5.0, 7.0), CGSize::new(30.0, 40.0));
        let back = flip(flip(f));
        assert!((back.origin.y - f.origin.y).abs() < 1e-9 && back.size.height == f.size.height);
    }

    #[test]
    fn virtual_screen_covers_at_least_one_monitor() {
        let (left, top, right, bottom) = virtual_screen();
        assert!(right > left && bottom > top);
        assert!(monitor_count() >= 1 || crate::in_ci());
        assert!(grab_pixels(0, 0, 0, 5).is_none());
        let _ = cursor_pos();
        let _ = visible_windows(0);
        let _ = local_time();
    }

    #[test]
    fn a_second_instance_reaches_the_first() {
        let key = format!("test-{}", std::process::id());
        let first = Instance::claim(&key).expect("first claim");
        assert!(Instance::claim(&key).is_none(), "only one copy may hold the lock");
        let sender = std::thread::spawn({
            let key = key.clone();
            move || Instance::send(&key, InstanceRequest::Select)
        });
        assert_eq!(first.wait(), Some(InstanceRequest::Select));
        assert!(sender.join().unwrap());
        drop(first);
        assert!(Instance::claim(&key).is_some(), "the lock is released with the instance");
    }

    #[test]
    fn unpremultiply_restores_color() {
        let mut px = [64, 0, 0, 128, 10, 20, 30, 255, 0, 0, 0, 0];
        unpremultiply(&mut px);
        assert_eq!(&px[..4], &[128, 0, 0, 128]);
        assert_eq!(&px[4..8], &[10, 20, 30, 255]);
    }
}
