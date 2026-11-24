//! Linux platform integration (Wayland + X11): cursor position, window list, single-instance
//! IPC, GTK3 main thread for tray/pins, XDG folders, and clipboard file URI copying.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use image::RgbaImage;
use raw_window_handle::RawWindowHandle;
use x11rb::connection::Connection;
use x11rb::errors::{ConnectionError, ReplyError};
use x11rb::protocol::xfixes;
use x11rb::protocol::xproto::{
    AtomEnum, ConfigureWindowAux, ConnectionExt, ImageFormat, KeyButMask, MapState, Window,
};
use x11rb::rust_connection::RustConnection;

use super::{InstanceRequest, PxRect};

static GTK_INIT: OnceLock<Result<(), String>> = OnceLock::new();

pub(crate) fn has_display() -> bool {
    std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

/// Ensures the dedicated GTK3 main loop thread is running and dispatches `f` onto it.
pub(crate) fn run_on_gtk<F: FnOnce() + Send + 'static>(f: F) -> Result<(), String> {
    let init = GTK_INIT.get_or_init(|| {
        if !has_display() {
            return Err("no display server (DISPLAY / WAYLAND_DISPLAY)".to_owned());
        }
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        std::thread::Builder::new()
            .name("snapkit-gtk".to_owned())
            .spawn(move || {
                if let Err(e) = gtk::init() {
                    let _ = tx.send(Err(format!("gtk::init failed: {e}")));
                    return;
                }
                let _ = tx.send(Ok(()));
                gtk::main();
            })
            .map_err(|e| format!("failed to spawn GTK thread: {e}"))?;
        rx.recv().map_err(|_| "GTK thread exited before init".to_owned())?
    });
    if let Err(e) = init {
        return Err(e.clone());
    }
    gtk::glib::MainContext::default().invoke(f);
    Ok(())
}

fn xdg_user_dir(kind: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("xdg-user-dir").arg(kind).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if s.is_empty() {
        return None;
    }
    let p = PathBuf::from(s);
    p.is_dir().then_some(p)
}

pub(super) fn xdg_pictures_dir() -> PathBuf {
    if let Some(dir) = xdg_user_dir("PICTURES") {
        return dir;
    }
    let home = super::home_dir();
    let pt = home.join("Imagens");
    if pt.is_dir() {
        return pt;
    }
    home.join("Pictures")
}

pub(super) fn xdg_videos_dir() -> PathBuf {
    if let Some(dir) = xdg_user_dir("VIDEOS") {
        return dir;
    }
    let home = super::home_dir();
    let pt = home.join("Vídeos");
    if pt.is_dir() {
        return pt;
    }
    home.join("Videos")
}

pub(super) fn reveal_in_file_manager(path: &Path) {
    if let Ok(abs) = std::fs::canonicalize(path) {
        let uri = path_to_file_uri(&abs);
        if let Ok(status) = std::process::Command::new("dbus-send")
            .args([
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                &format!("array:string:{uri}"),
                "string:",
            ])
            .status()
            && status.success()
        {
            return;
        }
    }
    let folder = path.parent().unwrap_or(path);
    super::open(folder);
}

/// Shows a blocking error message (stderr + kdialog/zenity best-effort).
pub fn error_box(title: &str, text: &str) {
    eprintln!("{title}: {text}");
    if !has_display() {
        return;
    }
    if std::process::Command::new("kdialog")
        .args(["--error", text, "--title", title])
        .status()
        .is_ok()
    {
        return;
    }
    let _ = std::process::Command::new("zenity")
        .args(["--error", &format!("--title={title}"), &format!("--text={text}")])
        .status();
}

pub fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    // SAFETY: `time` and `localtime_r` operate on valid stack memory.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return (1970, 1, 1, 0, 0, 0);
        }
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

pub fn window_id(handle: RawWindowHandle) -> isize {
    match handle {
        RawWindowHandle::Xlib(h) => h.window as isize,
        RawWindowHandle::Xcb(h) => h.window.get() as isize,
        _ => 0,
    }
}

enum X11CallError {
    Disconnected,
    X11,
}

impl From<ConnectionError> for X11CallError {
    fn from(_: ConnectionError) -> Self {
        Self::Disconnected
    }
}

impl From<ReplyError> for X11CallError {
    fn from(err: ReplyError) -> Self {
        match err {
            ReplyError::ConnectionError(_) => Self::Disconnected,
            ReplyError::X11Error(_) => Self::X11,
        }
    }
}

struct Atoms {
    net_wm_name: u32,
    utf8_string: u32,
    net_active_window: u32,
    net_client_list_stacking: u32,
}

impl Atoms {
    fn intern(conn: &RustConnection) -> Self {
        let intern = |name: &[u8]| {
            conn.intern_atom(false, name)
                .ok()
                .and_then(|c| c.reply().ok())
                .map_or(0, |r| r.atom)
        };
        Self {
            net_wm_name: intern(b"_NET_WM_NAME"),
            utf8_string: intern(b"UTF8_STRING"),
            net_active_window: intern(b"_NET_ACTIVE_WINDOW"),
            net_client_list_stacking: intern(b"_NET_CLIENT_LIST_STACKING"),
        }
    }
}

struct Keycodes {
    escape: Vec<u8>,
    enter: Vec<u8>,
}

impl Keycodes {
    fn load(conn: &RustConnection) -> Self {
        let setup = conn.setup();
        let min = setup.min_keycode;
        let count = setup.max_keycode.saturating_sub(min).saturating_add(1);
        let Ok(mapping) = conn.get_keyboard_mapping(min, count).ok().and_then(|c| c.reply().ok()).ok_or(()) else {
            return Self { escape: Vec::new(), enter: Vec::new() };
        };
        let syms_per_code = mapping.keysyms_per_keycode as usize;
        if syms_per_code == 0 {
            return Self { escape: Vec::new(), enter: Vec::new() };
        }
        let mut escape = Vec::new();
        let mut enter = Vec::new();
        for (idx, chunk) in mapping.keysyms.chunks_exact(syms_per_code).enumerate() {
            let keycode = (min as usize + idx) as u8;
            if chunk.contains(&0xff1b) {
                escape.push(keycode);
            }
            if chunk.contains(&0xff0d) {
                enter.push(keycode);
            }
        }
        Self { escape, enter }
    }
}

struct X11State {
    conn: RustConnection,
    screen_num: usize,
    atoms: Atoms,
    keycodes: Keycodes,
    xfixes: bool,
}

static X11_STATE: Mutex<Option<X11State>> = Mutex::new(None);

fn connect_x11() -> Option<X11State> {
    if std::env::var_os("DISPLAY").is_none_or(|v| v.is_empty()) {
        return None;
    }
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let atoms = Atoms::intern(&conn);
    let keycodes = Keycodes::load(&conn);
    let xfixes = xfixes::query_version(&conn, 2, 0)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some();
    Some(X11State {
        conn,
        screen_num,
        atoms,
        keycodes,
        xfixes,
    })
}

/// Runs `f` against the cached X11 connection, reconnecting once if the socket dropped.
fn with_x11<T>(mut f: impl FnMut(&X11State) -> Result<T, X11CallError>) -> Option<T> {
    let mut guard = X11_STATE.lock().unwrap_or_else(|e| e.into_inner());
    for attempt in 0..2 {
        if guard.is_none() {
            *guard = connect_x11();
        }
        let state = guard.as_ref()?;
        match f(state) {
            Ok(v) => return Some(v),
            Err(X11CallError::Disconnected) if attempt == 0 => {
                *guard = None;
            }
            Err(_) => return None,
        }
    }
    None
}

fn x11_window_title(state: &X11State, win: Window) -> Result<String, X11CallError> {
    if state.atoms.net_wm_name != 0 && state.atoms.utf8_string != 0 {
        let reply = state
            .conn
            .get_property(false, win, state.atoms.net_wm_name, state.atoms.utf8_string, 0, 1024)?
            .reply()?;
        if !reply.value.is_empty() {
            return Ok(String::from_utf8_lossy(&reply.value).trim().to_owned());
        }
    }
    let reply = state
        .conn
        .get_property(false, win, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)?
        .reply()?;
    if !reply.value.is_empty() {
        return Ok(String::from_utf8_lossy(&reply.value).trim().to_owned());
    }
    Ok(String::new())
}

fn x11_active_window(state: &X11State) -> Result<Window, X11CallError> {
    if state.atoms.net_active_window == 0 {
        return Ok(0);
    }
    let root = state.conn.setup().roots[state.screen_num].root;
    let reply = state
        .conn
        .get_property(false, root, state.atoms.net_active_window, AtomEnum::WINDOW, 0, 1)?
        .reply()?;
    Ok(reply.value32().and_then(|mut it| it.next()).unwrap_or(0))
}

pub fn foreground_window_hwnd() -> isize {
    with_x11(|state| Ok(x11_active_window(state)? as isize)).unwrap_or(0)
}

pub fn foreground_window_title() -> String {
    foreground_window_title_except(0)
}

pub fn foreground_window_title_except(skip: isize) -> String {
    with_x11(|state| {
        let win = x11_active_window(state)?;
        if win == 0 || win as isize == skip {
            return Ok(String::new());
        }
        x11_window_title(state, win)
    })
    .unwrap_or_default()
}

pub fn foreground_window_rect() -> Option<PxRect> {
    window_rect(foreground_window_hwnd())
}

pub fn is_foreground(hwnd: isize) -> bool {
    hwnd != 0 && foreground_window_hwnd() == hwnd
}

pub fn ensure_input_permission() -> bool {
    true
}

pub fn ensure_key_permission() -> bool {
    true
}

pub fn key_permission() -> bool {
    true
}

pub fn ensure_screen_permission() -> bool {
    true
}

pub fn cursor_pos() -> (i32, i32) {
    with_x11(|state| {
        let root = state.conn.setup().roots[state.screen_num].root;
        let reply = state.conn.query_pointer(root)?.reply()?;
        Ok((reply.root_x as i32, reply.root_y as i32))
    })
    .unwrap_or((0, 0))
}

pub fn set_cursor_pos(p: (i32, i32)) {
    let _ = std::process::Command::new("xdotool")
        .args(["mousemove", &p.0.to_string(), &p.1.to_string()])
        .status();
}

pub fn wheel(notches: i32) {
    if notches == 0 {
        return;
    }
    let btn = if notches > 0 { "4" } else { "5" };
    let count = notches.abs().max(1).to_string();
    let _ = std::process::Command::new("xdotool")
        .args(["click", "--repeat", &count, btn])
        .status();
}

pub fn hwheel(notches: i32) {
    if notches == 0 {
        return;
    }
    let btn = if notches > 0 { "7" } else { "6" };
    let count = notches.abs().max(1).to_string();
    let _ = std::process::Command::new("xdotool")
        .args(["click", "--repeat", &count, btn])
        .status();
}

pub fn focus_window_at(p: (i32, i32)) {
    let _ = std::process::Command::new("xdotool")
        .args(["mousemove", &p.0.to_string(), &p.1.to_string(), "click", "1"])
        .status();
}

pub fn exclude_from_capture(_hwnd: isize, _exclude: bool) {}

pub fn window_pos(hwnd: isize) -> Option<(i32, i32)> {
    window_rect(hwnd).map(|r| (r.0, r.1))
}

pub fn set_window_pos(hwnd: isize, p: (i32, i32)) {
    if hwnd == 0 {
        return;
    }
    with_x11(|state| {
        let aux = ConfigureWindowAux::new().x(p.0).y(p.1);
        state.conn.configure_window(hwnd as Window, &aux)?;
        state.conn.flush()?;
        Ok(())
    });
}

fn scale_logical_rect(x: i32, y: i32, w: i32, h: i32, scale: f32) -> Option<PxRect> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let scale = scale.max(0.5);
    let px = (x as f32 * scale).round() as i32;
    let py = (y as f32 * scale).round() as i32;
    let pw = (w as f32 * scale).round() as i32;
    let ph = (h as f32 * scale).round() as i32;
    (pw > 0 && ph > 0).then_some((px, py, px + pw, py + ph))
}

/// Converts `xcap`'s logical monitor geometry on Linux back into physical pixel coordinates.
pub(crate) fn monitor_physical_rect(m: &xcap::Monitor) -> Option<PxRect> {
    let scale = m.scale_factor().unwrap_or(1.0);
    let x = m.x().ok()?;
    let y = m.y().ok()?;
    let w = m.width().ok()? as i32;
    let h = m.height().ok()? as i32;
    scale_logical_rect(x, y, w, h, scale)
}

pub fn virtual_screen() -> PxRect {
    if let Ok(monitors) = xcap::Monitor::all()
        && !monitors.is_empty()
    {
        let mut left = i32::MAX;
        let mut top = i32::MAX;
        let mut right = i32::MIN;
        let mut bottom = i32::MIN;
        for m in monitors {
            if let Some((x, y, r, b)) = monitor_physical_rect(&m) {
                left = left.min(x);
                top = top.min(y);
                right = right.max(r);
                bottom = bottom.max(b);
            }
        }
        if left < right && top < bottom {
            return (left, top, right, bottom);
        }
    }
    (0, 0, 1920, 1080)
}

/// Finds the monitor whose physical pixel rect contains `p` (avoiding xcap's logical `from_point`).
pub(crate) fn monitor_at_physical_point(p: (i32, i32)) -> Option<xcap::Monitor> {
    let monitors = xcap::Monitor::all().ok()?;
    for m in &monitors {
        if let Some((l, t, r, b)) = monitor_physical_rect(m)
            && p.0 >= l
            && p.0 < r
            && p.1 >= t
            && p.1 < b
        {
            return Some(m.clone());
        }
    }
    monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())
        .cloned()
}

pub fn selection_area() -> PxRect {
    if let Some(m) = monitor_at_physical_point(cursor_pos())
        && let Some(rect) = monitor_physical_rect(&m)
    {
        return rect;
    }
    virtual_screen()
}

pub fn monitor_count() -> i32 {
    xcap::Monitor::all().map(|m| m.len().max(1) as i32).unwrap_or(1)
}

fn x11_window_rect(state: &X11State, win: Window) -> Result<PxRect, X11CallError> {
    let root = state.conn.setup().roots[state.screen_num].root;
    let geom = state.conn.get_geometry(win)?.reply()?;
    let pos = state.conn.translate_coordinates(win, root, 0, 0)?.reply()?;
    let (x, y) = (pos.dst_x as i32, pos.dst_y as i32);
    Ok((x, y, x + geom.width as i32, y + geom.height as i32))
}

pub fn window_rect(hwnd: isize) -> Option<PxRect> {
    if hwnd == 0 {
        return None;
    }
    with_x11(|state| x11_window_rect(state, hwnd as Window))
}

pub fn set_window_rect(hwnd: isize, r: PxRect) {
    if hwnd == 0 {
        return;
    }
    with_x11(|state| {
        let w = (r.2 - r.0).max(1) as u32;
        let h = (r.3 - r.1).max(1) as u32;
        let aux = ConfigureWindowAux::new().x(r.0).y(r.1).width(w).height(h);
        state.conn.configure_window(hwnd as Window, &aux)?;
        state.conn.flush()?;
        Ok(())
    });
}

pub fn client_origin(hwnd: isize) -> Option<(i32, i32)> {
    window_rect(hwnd).map(|r| (r.0, r.1))
}

pub fn client_rect_screen(hwnd: isize) -> Option<PxRect> {
    window_rect(hwnd)
}

pub fn enable_per_monitor_dpi() {}

pub fn dpi_scale_at(p: (i32, i32)) -> f32 {
    if let Some(m) = monitor_at_physical_point(p)
        && let Ok(s) = m.scale_factor()
        && s > 0.0
    {
        return s;
    }
    1.0
}

pub fn grab_pixels(x: i32, y: i32, w: i32, h: i32) -> Option<Vec<[u8; 3]>> {
    if w <= 0 || h <= 0 || w > 256 || h > 256 {
        return None;
    }
    with_x11(|state| {
        let screen = &state.conn.setup().roots[state.screen_num];
        let root = screen.root;
        let root_w = screen.width_in_pixels as i32;
        let root_h = screen.height_in_pixels as i32;
        let mut pixels = vec![[0u8, 0, 0]; (w as usize) * (h as usize)];

        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(root_w);
        let y1 = (y + h).min(root_h);
        if x0 >= x1 || y0 >= y1 {
            return Ok(pixels);
        }

        let gw = (x1 - x0) as u16;
        let gh = (y1 - y0) as u16;
        let reply = state
            .conn
            .get_image(ImageFormat::Z_PIXMAP, root, x0 as i16, y0 as i16, gw, gh, u32::MAX)?
            .reply()?;
        let gw_usize = gw as usize;
        let gh_usize = gh as usize;
        let expected = gw_usize * gh_usize * 4;
        if reply.data.len() < expected {
            return Err(X11CallError::X11);
        }
        let dst_x0 = (x0 - x) as usize;
        let dst_y0 = (y0 - y) as usize;
        for row in 0..gh_usize {
            let src_row = &reply.data[row * gw_usize * 4..(row + 1) * gw_usize * 4];
            let dst_row_start = (dst_y0 + row) * (w as usize) + dst_x0;
            for (col, chunk) in src_row.chunks_exact(4).enumerate() {
                pixels[dst_row_start + col] = [chunk[2], chunk[1], chunk[0]];
            }
        }
        Ok(pixels)
    })
}

pub fn work_area_at(p: (i32, i32)) -> Option<PxRect> {
    let m = monitor_at_physical_point(p)?;
    monitor_physical_rect(&m)
}

fn check_keycodes(keymap: &[u8], codes: &[u8]) -> bool {
    codes.iter().any(|&kc| {
        let byte = kc as usize / 8;
        let bit = kc as usize % 8;
        byte < keymap.len() && (keymap[byte] & (1 << bit)) != 0
    })
}

pub fn escape_pressed() -> bool {
    with_x11(|state| {
        if state.keycodes.escape.is_empty() {
            return Ok(false);
        }
        let keymap = state.conn.query_keymap()?.reply()?;
        Ok(check_keycodes(&keymap.keys, &state.keycodes.escape))
    })
    .unwrap_or(false)
}

pub fn left_button_down() -> bool {
    with_x11(|state| {
        let root = state.conn.setup().roots[state.screen_num].root;
        let reply = state.conn.query_pointer(root)?.reply()?;
        Ok(reply.mask.contains(KeyButMask::BUTTON1))
    })
    .unwrap_or(false)
}

pub fn right_button_down() -> bool {
    with_x11(|state| {
        let root = state.conn.setup().roots[state.screen_num].root;
        let reply = state.conn.query_pointer(root)?.reply()?;
        Ok(reply.mask.contains(KeyButMask::BUTTON3))
    })
    .unwrap_or(false)
}

pub fn enter_down() -> bool {
    with_x11(|state| {
        if state.keycodes.enter.is_empty() {
            return Ok(false);
        }
        let keymap = state.conn.query_keymap()?.reply()?;
        Ok(check_keycodes(&keymap.keys, &state.keycodes.enter))
    })
    .unwrap_or(false)
}

pub fn cloak(_hwnd: isize, _on: bool) {}

pub fn apply_titlebar_theme(_hwnd: isize, _dark: bool, _bg: [u8; 3], _fg: [u8; 3]) {}

pub fn is_on_screen(hwnd: isize) -> bool {
    if hwnd == 0 {
        return true;
    }
    with_x11(|state| {
        let attrs = state.conn.get_window_attributes(hwnd as Window)?.reply()?;
        Ok(attrs.map_state == MapState::VIEWABLE)
    })
    .unwrap_or(true)
}

pub fn minimize(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    with_x11(|state| {
        state.conn.unmap_window(hwnd as Window)?;
        state.conn.flush()?;
        Ok(())
    });
}

pub fn hide(hwnd: isize) {
    minimize(hwnd);
}

pub fn show(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    with_x11(|state| {
        state.conn.map_window(hwnd as Window)?;
        state.conn.flush()?;
        Ok(())
    });
}

pub fn set_tool_window(_hwnd: isize, _on: bool) {}

static CHILDREN: Mutex<Vec<i32>> = Mutex::new(Vec::new());

extern "C" fn end_children() {
    if let Ok(list) = CHILDREN.try_lock() {
        for &pid in list.iter() {
            // SAFETY: only sends SIGTERM to tracked child PIDs; FFmpeg finalizes its output on SIGTERM.
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
    }
}

/// Ensures `child` terminates with SnapKit by sending `SIGTERM` on process exit.
pub fn end_with_us(child: &std::process::Child) {
    static HOOK: std::sync::Once = std::sync::Once::new();
    // SAFETY: registers a plain `extern "C"` function to run at exit.
    HOOK.call_once(|| unsafe {
        libc::atexit(end_children);
    });
    let mut list = CHILDREN.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: signal 0 only checks whether the process still exists without delivering a signal.
    list.retain(|&pid| unsafe { libc::kill(pid, 0) } == 0);
    list.push(child.id() as i32);
}

/// Single-instance lock + IPC over a Unix domain socket.
pub struct Instance {
    _lock: std::fs::File,
    socket_path: PathBuf,
    listener: UnixListener,
    quit_requested: AtomicBool,
}

fn instance_paths(key: &str) -> (PathBuf, PathBuf) {
    // SAFETY: getuid() has no preconditions and always succeeds.
    let uid = unsafe { libc::getuid() };
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    (
        dir.join(format!("snapkit-{uid}-{key}.lock")),
        dir.join(format!("snapkit-{uid}-{key}.sock")),
    )
}

fn try_lock(path: &Path) -> Option<std::fs::File> {
    use std::os::fd::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .ok()?;
    // SAFETY: `file` is a valid open descriptor; the lock lives as long as `file`.
    (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(file)
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

impl Instance {
    pub fn claim(key: &str) -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;
        let (lock_path, socket_path) = instance_paths(key);
        let lock = try_lock(&lock_path)?;
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).ok()?;
        let _ = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600));
        Some(Self {
            _lock: lock,
            socket_path,
            listener,
            quit_requested: AtomicBool::new(false),
        })
    }

    pub fn wait(&self) -> Option<InstanceRequest> {
        loop {
            let Ok((mut stream, _)) = self.listener.accept() else {
                return None;
            };
            let mut buf = [0u8; 1];
            if stream.read_exact(&mut buf).is_err() {
                continue;
            }
            match buf[0] {
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

    pub fn take_quit_request(&self) -> bool {
        self.quit_requested.swap(false, Ordering::SeqCst)
    }

    pub fn send(key: &str, request: InstanceRequest) -> bool {
        let (lock_path, socket_path) = instance_paths(key);
        let byte = match request {
            InstanceRequest::Show => b'S',
            InstanceRequest::Select => b'R',
            InstanceRequest::Quit => b'Q',
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut sent = false;
        while Instant::now() < deadline {
            if let Ok(mut stream) = UnixStream::connect(&socket_path)
                && stream.write_all(&[byte]).is_ok()
            {
                sent = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if sent && request == InstanceRequest::Quit {
            let wait_deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < wait_deadline {
                if try_lock(&lock_path).is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        sent
    }
}

pub fn visible_windows(skip: isize) -> Vec<PxRect> {
    with_x11(|state| {
        if state.atoms.net_client_list_stacking == 0 {
            return Ok(Vec::new());
        }
        let root = state.conn.setup().roots[state.screen_num].root;
        let reply = state
            .conn
            .get_property(
                false,
                root,
                state.atoms.net_client_list_stacking,
                AtomEnum::WINDOW,
                0,
                4096,
            )?
            .reply()?;
        let Some(ids) = reply.value32() else {
            return Ok(Vec::new());
        };
        let windows: Vec<Window> = ids.collect();
        let mut rects = Vec::new();
        for win in windows.into_iter().rev() {
            if win as isize == skip {
                continue;
            }
            let Ok(attrs) = state
                .conn
                .get_window_attributes(win)
                .ok()
                .and_then(|c| c.reply().ok())
                .ok_or(())
            else {
                continue;
            };
            if attrs.map_state != MapState::VIEWABLE {
                continue;
            }
            if let Ok(r) = x11_window_rect(state, win)
                && r.2 - r.0 >= 16
                && r.3 - r.1 >= 16
            {
                rects.push(r);
            }
        }
        Ok(rects)
    })
    .unwrap_or_default()
}

pub fn cursor_image() -> Option<(RgbaImage, (i32, i32))> {
    with_x11(|state| {
        if !state.xfixes {
            return Err(X11CallError::X11);
        }
        let reply = xfixes::get_cursor_image(&state.conn)?.reply()?;
        let w = reply.width as u32;
        let h = reply.height as u32;
        if w == 0 || h == 0 || reply.cursor_image.len() < (w * h) as usize {
            return Err(X11CallError::X11);
        }
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for &px in &reply.cursor_image[..(w * h) as usize] {
            let a = ((px >> 24) & 0xff) as u8;
            let r = ((px >> 16) & 0xff) as u8;
            let g = ((px >> 8) & 0xff) as u8;
            let b = (px & 0xff) as u8;
            if a == 0 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            } else if a == 255 {
                rgba.extend_from_slice(&[r, g, b, 255]);
            } else {
                let a16 = a as u16;
                let unpre = |c: u8| -> u8 { ((c as u16 * 255 + a16 / 2) / a16).min(255) as u8 };
                rgba.extend_from_slice(&[unpre(r), unpre(g), unpre(b), a]);
            }
        }
        let img = RgbaImage::from_raw(w, h, rgba).ok_or(X11CallError::X11)?;
        let pos = (reply.x as i32 - reply.xhot as i32, reply.y as i32 - reply.yhot as i32);
        Ok((img, pos))
    })
}

static NEXT_FRAME_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static REGION_FRAMES: RefCell<Vec<(u64, Vec<gtk::Window>)>> = const { RefCell::new(Vec::new()) };
}

pub struct RegionFrame {
    id: Option<u64>,
}

impl RegionFrame {
    pub fn show(rect: PxRect) -> Option<Self> {
        if !has_display() {
            return Some(Self { id: None });
        }
        let scale = dpi_scale_at((rect.0, rect.1)).max(0.5);
        let t = (4.0 * scale).round() as i32;
        let (l, top, r, b) = rect;
        let bars = [
            (l - t, top - t, r - l + 2 * t, t),
            (l - t, b, r - l + 2 * t, t),
            (l - t, top, t, b - top),
            (r, top, t, b - top),
        ];
        let logical_bars: Vec<(i32, i32, i32, i32)> = bars
            .into_iter()
            .map(|(bx, by, bw, bh)| {
                (
                    (bx as f32 / scale).round() as i32,
                    (by as f32 / scale).round() as i32,
                    ((bw as f32 / scale).round() as i32).max(1),
                    ((bh as f32 / scale).round() as i32).max(1),
                )
            })
            .collect();
        let id = NEXT_FRAME_ID.fetch_add(1, Ordering::Relaxed);
        run_on_gtk(move || {
            let mut windows = Vec::with_capacity(4);
            for (x, y, w, h) in logical_bars {
                let win = gtk::Window::new(gtk::WindowType::Toplevel);
                win.set_decorated(false);
                win.set_keep_above(true);
                win.set_skip_taskbar_hint(true);
                win.set_skip_pager_hint(true);
                win.set_accept_focus(false);
                win.set_focus_on_map(false);
                win.set_app_paintable(true);
                win.set_default_size(w, h);
                win.move_(x, y);
                win.connect_draw(|_, cr| {
                    cr.set_source_rgb(1.0, 138.0 / 255.0, 76.0 / 255.0);
                    let _ = cr.paint();
                    gtk::glib::Propagation::Stop
                });
                win.show_all();
                win.input_shape_combine_region(Some(&gtk::cairo::Region::create()));
                windows.push(win);
            }
            REGION_FRAMES.with(|frames| frames.borrow_mut().push((id, windows)));
        })
        .ok()?;
        Some(Self { id: Some(id) })
    }
}

impl Drop for RegionFrame {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        let _ = run_on_gtk(move || {
            REGION_FRAMES.with(|frames| {
                let mut list = frames.borrow_mut();
                if let Some(pos) = list.iter().position(|(fid, _)| *fid == id) {
                    let (_, windows) = list.swap_remove(pos);
                    for win in windows {
                        win.close();
                    }
                }
            });
        });
    }
}

pub struct ClickHalo;

impl ClickHalo {
    pub fn show(_rect: PxRect, _own_hwnd: isize) -> Self {
        Self
    }
}

pub(crate) fn path_to_file_uri(path: &Path) -> String {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|d| d.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };
    let mut out = String::from("file://");
    for byte in abs.as_os_str().as_encoded_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*byte as char);
            }
            b => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

pub fn copy_files_to_clipboard(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("no files to copy".to_owned());
    }
    let uris: Vec<String> = paths.iter().map(|p| path_to_file_uri(p)).collect();
    let payload = format!("{}\r\n", uris.join("\r\n"));

    let mut child = if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()) {
        std::process::Command::new("wl-copy")
            .args(["--type", "text/uri-list"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("wl-copy failed: {e}"))?
    } else {
        match std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "text/uri-list"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::process::Command::new("xsel")
                .args(["--clipboard", "--input"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e2| format!("xsel failed: {e2}"))?,
            Err(e) => return Err(format!("xclip failed: {e}")),
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(payload.as_bytes()).map_err(|e| e.to_string())?;
    }
    // Never use wait_with_output() with piped stdout/stderr: xclip/xsel fork a daemon that keeps them open.
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("clipboard helper exited with {status}"));
    }
    Ok(())
}

pub fn start_file_drag(_paths: &[PathBuf]) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_time_returns_plausible_date() {
        let (y, m, d, hh, mm, ss) = local_time();
        assert!(y >= 2024);
        assert!((1..=12).contains(&m));
        assert!((1..=31).contains(&d));
        assert!(hh <= 23 && mm <= 59 && ss <= 59);
    }

    #[test]
    fn file_uri_encodes_spaces_and_non_ascii() {
        let uri = path_to_file_uri(Path::new("/home/user/Área de trabalho/a b.png"));
        assert_eq!(uri, "file:///home/user/%C3%81rea%20de%20trabalho/a%20b.png");
    }

    #[test]
    fn instance_claim_send_and_quit_work() {
        let key = format!("test-{}", std::process::id());
        let inst = Instance::claim(&key).expect("first claim succeeds");
        assert!(Instance::claim(&key).is_none(), "second claim must fail");
        let t = std::thread::spawn(move || {
            assert_eq!(inst.wait(), Some(InstanceRequest::Show));
            assert_eq!(inst.wait(), Some(InstanceRequest::Quit));
            assert!(inst.take_quit_request());
        });
        assert!(Instance::send(&key, InstanceRequest::Show));
        assert!(Instance::send(&key, InstanceRequest::Quit));
        t.join().unwrap();
    }

    #[test]
    fn scale_logical_rect_restores_physical_dimensions() {
        assert_eq!(scale_logical_rect(0, 0, 1536, 864, 1.25), Some((0, 0, 1920, 1080)));
        assert_eq!(scale_logical_rect(1536, 0, 1536, 864, 1.25), Some((1920, 0, 3840, 1080)));
        assert_eq!(scale_logical_rect(0, 0, 0, 864, 1.25), None);
    }

    #[test]
    fn xvfb_display_queries_work() {
        if !has_display() {
            return;
        }
        let vs = virtual_screen();
        assert!(vs.2 > vs.0 && vs.3 > vs.1);
        assert!(monitor_count() >= 1);
        let _ = visible_windows(0);
    }

    #[test]
    fn xvfb_grab_pixels_and_edge_clamping_work() {
        if !has_display() || std::env::var_os("DISPLAY").is_none_or(|v| v.is_empty()) {
            return;
        }
        let center = grab_pixels(0, 0, 16, 16).expect("16x16 grab at origin");
        assert_eq!(center.len(), 256);
        let edge = grab_pixels(-4, -4, 16, 16).expect("edge-clamped 16x16 grab");
        assert_eq!(edge.len(), 256);
        assert_eq!(edge[0], [0, 0, 0]);
        assert!(grab_pixels(0, 0, 0, 16).is_none());
        assert!(grab_pixels(0, 0, 257, 16).is_none());
    }

    #[test]
    fn xvfb_region_frame_show_and_drop() {
        if !has_display() {
            return;
        }
        let frame = RegionFrame::show((20, 20, 120, 100));
        assert!(frame.is_some());
        drop(frame);
    }

    #[test]
    fn xvfb_copy_files_to_clipboard_does_not_hang() {
        if !has_display() {
            return;
        }
        let has_xclip = std::process::Command::new("xclip")
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok();
        if !has_xclip {
            return;
        }
        let res = copy_files_to_clipboard(&[PathBuf::from("/tmp/snapkit-test-clip.png")]);
        assert!(res.is_ok(), "copy_files_to_clipboard failed: {res:?}");
    }
}
