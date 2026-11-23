//! Linux platform integration (Wayland + X11): cursor position, window list, single-instance
//! IPC, GTK3 main thread for tray/pins, XDG folders, and clipboard file URI copying.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, mpsc};
use std::time::{Duration, Instant};

use image::RgbaImage;
use raw_window_handle::RawWindowHandle;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConfigureWindowAux, ConnectionExt, ImageFormat, KeyButMask, Window,
};

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

fn x11_conn() -> Option<(impl Connection, usize)> {
    if std::env::var_os("DISPLAY").is_none_or(|v| v.is_empty()) {
        return None;
    }
    x11rb::connect(None).ok()
}

fn x11_atom(conn: &impl Connection, name: &[u8]) -> Option<u32> {
    conn.intern_atom(false, name).ok()?.reply().ok().map(|r| r.atom)
}

fn x11_window_title(conn: &impl Connection, win: Window) -> String {
    if let Some(net_wm_name) = x11_atom(conn, b"_NET_WM_NAME")
        && let Some(utf8) = x11_atom(conn, b"UTF8_STRING")
        && let Ok(cookie) = conn.get_property(false, win, net_wm_name, utf8, 0, 1024)
        && let Ok(reply) = cookie.reply()
        && !reply.value.is_empty()
    {
        return String::from_utf8_lossy(&reply.value).trim().to_owned();
    }
    if let Ok(cookie) = conn.get_property(false, win, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)
        && let Ok(reply) = cookie.reply()
        && !reply.value.is_empty()
    {
        return String::from_utf8_lossy(&reply.value).trim().to_owned();
    }
    String::new()
}

pub fn foreground_window_hwnd() -> isize {
    let Some((conn, screen_num)) = x11_conn() else { return 0 };
    let root = conn.setup().roots[screen_num].root;
    let Some(atom) = x11_atom(&conn, b"_NET_ACTIVE_WINDOW") else { return 0 };
    let Ok(cookie) = conn.get_property(false, root, atom, AtomEnum::WINDOW, 0, 1) else {
        return 0;
    };
    let Ok(reply) = cookie.reply() else { return 0 };
    reply.value32().and_then(|mut it| it.next()).unwrap_or(0) as isize
}

pub fn foreground_window_title() -> String {
    foreground_window_title_except(0)
}

pub fn foreground_window_title_except(skip: isize) -> String {
    let Some((conn, screen_num)) = x11_conn() else {
        return String::new();
    };
    let root = conn.setup().roots[screen_num].root;
    let Some(atom) = x11_atom(&conn, b"_NET_ACTIVE_WINDOW") else {
        return String::new();
    };
    let Ok(cookie) = conn.get_property(false, root, atom, AtomEnum::WINDOW, 0, 1) else {
        return String::new();
    };
    let Ok(reply) = cookie.reply() else {
        return String::new();
    };
    let Some(win) = reply.value32().and_then(|mut it| it.next()) else {
        return String::new();
    };
    if win == 0 || win as isize == skip {
        return String::new();
    }
    x11_window_title(&conn, win)
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
    let Some((conn, screen_num)) = x11_conn() else {
        return (0, 0);
    };
    let root = conn.setup().roots[screen_num].root;
    let Ok(cookie) = conn.query_pointer(root) else {
        return (0, 0);
    };
    let Ok(reply) = cookie.reply() else {
        return (0, 0);
    };
    (reply.root_x as i32, reply.root_y as i32)
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
    let Some((conn, _)) = x11_conn() else { return };
    let aux = ConfigureWindowAux::new().x(p.0).y(p.1);
    let _ = conn.configure_window(hwnd as Window, &aux);
    let _ = conn.flush();
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

pub fn selection_area() -> PxRect {
    virtual_screen()
}

pub fn monitor_count() -> i32 {
    xcap::Monitor::all().map(|m| m.len().max(1) as i32).unwrap_or(1)
}

pub fn window_rect(hwnd: isize) -> Option<PxRect> {
    if hwnd == 0 {
        return None;
    }
    let (conn, screen_num) = x11_conn()?;
    let root = conn.setup().roots[screen_num].root;
    let geom = conn.get_geometry(hwnd as Window).ok()?.reply().ok()?;
    let pos = conn.translate_coordinates(hwnd as Window, root, 0, 0).ok()?.reply().ok()?;
    let (x, y) = (pos.dst_x as i32, pos.dst_y as i32);
    Some((x, y, x + geom.width as i32, y + geom.height as i32))
}

pub fn set_window_rect(hwnd: isize, r: PxRect) {
    if hwnd == 0 {
        return;
    }
    let Some((conn, _)) = x11_conn() else { return };
    let w = (r.2 - r.0).max(1) as u32;
    let h = (r.3 - r.1).max(1) as u32;
    let aux = ConfigureWindowAux::new().x(r.0).y(r.1).width(w).height(h);
    let _ = conn.configure_window(hwnd as Window, &aux);
    let _ = conn.flush();
}

pub fn client_origin(hwnd: isize) -> Option<(i32, i32)> {
    window_rect(hwnd).map(|r| (r.0, r.1))
}

pub fn client_rect_screen(hwnd: isize) -> Option<PxRect> {
    window_rect(hwnd)
}

pub fn enable_per_monitor_dpi() {}

pub fn dpi_scale_at(p: (i32, i32)) -> f32 {
    if let Ok(m) = xcap::Monitor::from_point(p.0, p.1)
        && let Ok(s) = m.scale_factor()
        && s > 0.0
    {
        return s;
    }
    if let Ok(monitors) = xcap::Monitor::all()
        && let Some(m) = monitors.first()
        && let Ok(s) = m.scale_factor()
        && s > 0.0
    {
        return s;
    }
    1.0
}

pub fn grab_pixels(x: i32, y: i32, w: i32, h: i32) -> Option<Vec<[u8; 3]>> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let (conn, screen_num) = x11_conn()?;
    let root = conn.setup().roots[screen_num].root;
    let reply = conn
        .get_image(ImageFormat::Z_PIXMAP, root, x as i16, y as i16, w as u16, h as u16, u32::MAX)
        .ok()?
        .reply()
        .ok()?;
    let expected = (w as usize) * (h as usize) * 4;
    if reply.data.len() < expected {
        return None;
    }
    let mut pixels = Vec::with_capacity((w as usize) * (h as usize));
    for chunk in reply.data[..expected].chunks_exact(4) {
        pixels.push([chunk[2], chunk[1], chunk[0]]);
    }
    Some(pixels)
}

pub fn work_area_at(p: (i32, i32)) -> Option<PxRect> {
    let m = xcap::Monitor::from_point(p.0, p.1)
        .ok()
        .or_else(|| xcap::Monitor::all().ok()?.into_iter().next())?;
    monitor_physical_rect(&m)
}

fn x11_key_pressed(keysym: u32) -> bool {
    let Some((conn, _)) = x11_conn() else { return false };
    let setup = conn.setup();
    let min = setup.min_keycode;
    let count = setup.max_keycode.saturating_sub(min).saturating_add(1);
    let Ok(mapping) = conn.get_keyboard_mapping(min, count).ok().and_then(|c| c.reply().ok()).ok_or(()) else {
        return false;
    };
    let Ok(keymap) = conn.query_keymap().ok().and_then(|c| c.reply().ok()).ok_or(()) else {
        return false;
    };
    let syms_per_code = mapping.keysyms_per_keycode as usize;
    if syms_per_code == 0 {
        return false;
    }
    for (idx, chunk) in mapping.keysyms.chunks_exact(syms_per_code).enumerate() {
        if chunk.contains(&keysym) {
            let keycode = min as usize + idx;
            let byte = keycode / 8;
            let bit = keycode % 8;
            if byte < keymap.keys.len() && (keymap.keys[byte] & (1 << bit)) != 0 {
                return true;
            }
        }
    }
    false
}

pub fn escape_pressed() -> bool {
    x11_key_pressed(0xff1b)
}

pub fn left_button_down() -> bool {
    let Some((conn, screen_num)) = x11_conn() else { return false };
    let root = conn.setup().roots[screen_num].root;
    conn.query_pointer(root)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some_and(|r| r.mask.contains(KeyButMask::BUTTON1))
}

pub fn right_button_down() -> bool {
    let Some((conn, screen_num)) = x11_conn() else { return false };
    let root = conn.setup().roots[screen_num].root;
    conn.query_pointer(root)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some_and(|r| r.mask.contains(KeyButMask::BUTTON3))
}

pub fn enter_down() -> bool {
    x11_key_pressed(0xff0d)
}

pub fn cloak(_hwnd: isize, _on: bool) {}

pub fn apply_titlebar_theme(_hwnd: isize, _dark: bool, _bg: [u8; 3], _fg: [u8; 3]) {}

pub fn is_on_screen(hwnd: isize) -> bool {
    if hwnd == 0 {
        return true;
    }
    let Some((conn, _)) = x11_conn() else { return true };
    conn.get_window_attributes(hwnd as Window)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some_and(|a| a.map_state == x11rb::protocol::xproto::MapState::VIEWABLE)
}

pub fn minimize(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let Some((conn, _)) = x11_conn() else { return };
    let _ = conn.unmap_window(hwnd as Window);
    let _ = conn.flush();
}

pub fn hide(hwnd: isize) {
    minimize(hwnd);
}

pub fn show(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let Some((conn, _)) = x11_conn() else { return };
    let _ = conn.map_window(hwnd as Window);
    let _ = conn.flush();
}

pub fn set_tool_window(_hwnd: isize, _on: bool) {}

pub fn end_with_us(_child: &std::process::Child) {}

/// Single-instance lock + IPC over a Unix domain socket.
pub struct Instance {
    socket_path: PathBuf,
    listener: UnixListener,
    quit_requested: AtomicBool,
}

fn instance_socket_path(key: &str) -> PathBuf {
    // SAFETY: getuid() has no preconditions and always succeeds.
    let uid = unsafe { libc::getuid() };
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("snapkit-{uid}-{key}.sock"))
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

impl Instance {
    pub fn claim(key: &str) -> Option<Self> {
        let socket_path = instance_socket_path(key);
        if UnixStream::connect(&socket_path).is_ok() {
            return None;
        }
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).ok()?;
        Some(Self {
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
        let socket_path = instance_socket_path(key);
        let byte = match request {
            InstanceRequest::Show => b'S',
            InstanceRequest::Select => b'R',
            InstanceRequest::Quit => b'Q',
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut sent = false;
        while Instant::now() < deadline {
            if let Ok(mut stream) = UnixStream::connect(&socket_path) {
                if stream.write_all(&[byte]).is_ok() {
                    sent = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if sent && request == InstanceRequest::Quit {
            let wait_deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < wait_deadline {
                if UnixStream::connect(&socket_path).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        sent
    }
}

pub fn visible_windows(skip: isize) -> Vec<PxRect> {
    let Some((conn, screen_num)) = x11_conn() else {
        return Vec::new();
    };
    let root = conn.setup().roots[screen_num].root;
    let Some(list_atom) = x11_atom(&conn, b"_NET_CLIENT_LIST_STACKING") else {
        return Vec::new();
    };
    let Ok(cookie) = conn.get_property(false, root, list_atom, AtomEnum::WINDOW, 0, 4096) else {
        return Vec::new();
    };
    let Ok(reply) = cookie.reply() else {
        return Vec::new();
    };
    let Some(ids) = reply.value32() else {
        return Vec::new();
    };
    let windows: Vec<Window> = ids.collect();
    let mut rects = Vec::new();
    for win in windows.into_iter().rev() {
        if win as isize == skip {
            continue;
        }
        let Ok(attrs) = conn.get_window_attributes(win).ok().and_then(|c| c.reply().ok()).ok_or(()) else {
            continue;
        };
        if attrs.map_state != x11rb::protocol::xproto::MapState::VIEWABLE {
            continue;
        }
        if let Some(r) = window_rect(win as isize)
            && r.2 - r.0 >= 16
            && r.3 - r.1 >= 16
        {
            rects.push(r);
        }
    }
    rects
}

pub fn cursor_image() -> Option<(RgbaImage, (i32, i32))> {
    None
}

pub struct RegionFrame;

impl RegionFrame {
    pub fn show(_rect: PxRect) -> Option<Self> {
        Some(Self)
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

    let mut cmd = if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()) {
        let mut c = std::process::Command::new("wl-copy");
        c.args(["--type", "text/uri-list"]);
        c
    } else {
        let mut c = std::process::Command::new("xclip");
        c.args(["-selection", "clipboard", "-t", "text/uri-list"]);
        c
    };
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("clipboard helper failed: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(payload.as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
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
}
