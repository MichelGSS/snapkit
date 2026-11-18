//! Direct Windows API calls: cursor position, window list, cursor image and placement of our
//! window.
//!
//! Every coordinate is in physical pixels of the virtual desktop (the process is per-monitor DPI
//! aware because of winit).

use std::ffi::c_void;

use image::RgbaImage;
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CLOAK, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, ClientToScreen, CreateCompatibleDC, GetDC, ReleaseDC, SRCCOPY, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, SelectObject,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    SendInput,
    VK_ESCAPE, VK_LBUTTON, VK_RBUTTON, VK_RETURN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetForegroundWindow, GetSystemMetrics, SM_CMONITORS, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, GetWindowTextW, SetCursorPos, SetWindowDisplayAffinity,
    WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WindowFromPoint,
    CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, EnumWindows, GetCursorInfo, GetCursorPos,
    GetIconInfo, GetWindowRect, ICONINFO, IsIconic, IsWindowVisible, SW_HIDE, SW_MINIMIZE,
    SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow,
    SetWindowPos, ShowWindow, MB_ICONERROR, MB_OK, MessageBoxW,
};

/// Shows a blocking error message. SnapKit has no console, so this is how it reports a failure
/// to start.
pub fn error_box(title: &str, text: &str) {
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (title, text) = (wide(title), wide(text));
    // SAFETY: both strings are null-terminated and outlive the call.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR) };
}

/// Rectangle in pixels: (left, top, right, bottom).
pub type PxRect = (i32, i32, i32, i32);

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: `p` is a valid POINT to write to.
    unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

pub fn set_cursor_pos(p: (i32, i32)) {
    // SAFETY: a simple call, no pointers.
    unsafe { SetCursorPos(p.0, p.1) };
}

/// Turns the mouse wheel: positive scrolls up, negative down. The event goes to the window under
/// the cursor.
pub fn wheel(notches: i32) {
    send_wheel(notches, MOUSEEVENTF_WHEEL);
}

/// Horizontal wheel: positive scrolls right, negative left.
pub fn hwheel(notches: i32) {
    send_wheel(notches, MOUSEEVENTF_HWHEEL);
}

fn send_wheel(notches: i32, flags: u32) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                // WHEEL_DELTA (120) per wheel notch; the field is a u32 but carries a signed value.
                mouseData: (notches * 120) as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // SAFETY: a valid INPUT, with the correct size given.
    unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) };
}

/// Brings the top-level window at the given point to the front.
pub fn focus_window_at(p: (i32, i32)) {
    // SAFETY: handles from the system; null ones are ignored.
    unsafe {
        let hwnd = WindowFromPoint(POINT { x: p.0, y: p.1 });
        if !hwnd.is_null() {
            let root = GetAncestor(hwnd, GA_ROOT);
            SetForegroundWindow(if root.is_null() { hwnd } else { root });
        }
    }
}

/// Keeps the window out of screen captures (or back to normal). Used by the panoramic capture
/// panel, which sits over the captured area.
pub fn exclude_from_capture(hwnd: isize, exclude: bool) {
    // For debugging: with this variable the window still shows in captures, which lets you
    // photograph the selection layer and see what it looks like.
    if std::env::var_os("SNAPKIT_DEBUG_VISIBLE").is_some() {
        return;
    }
    let affinity = if exclude { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    // SAFETY: handle of our main window.
    unsafe { SetWindowDisplayAffinity(hwnd as HWND, affinity) };
}

/// Top-left corner of the window, in screen coordinates.
pub fn window_pos(hwnd: isize) -> Option<(i32, i32)> {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: `r` is a valid RECT to write to.
    (unsafe { GetWindowRect(hwnd as HWND, &mut r) } != 0).then_some((r.left, r.top))
}

/// Moves the window without changing its size or order.
pub fn set_window_pos(hwnd: isize, p: (i32, i32)) {
    // SAFETY: handle of our main window.
    unsafe {
        SetWindowPos(hwnd as HWND, std::ptr::null_mut(), p.0, p.1, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE)
    };
}

/// Rectangle that encloses every monitor together.
pub fn virtual_screen() -> PxRect {
    // SAFETY: read-only queries.
    unsafe {
        let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        (x, y, x + GetSystemMetrics(SM_CXVIRTUALSCREEN), y + GetSystemMetrics(SM_CYVIRTUALSCREEN))
    }
}

pub fn monitor_count() -> i32 {
    // SAFETY: read-only query.
    unsafe { GetSystemMetrics(SM_CMONITORS) }
}

/// Rectangle of the whole window, in screen coordinates.
pub fn window_rect(hwnd: isize) -> Option<PxRect> {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: `r` is a valid RECT to write to.
    (unsafe { GetWindowRect(hwnd as HWND, &mut r) } != 0).then_some((r.left, r.top, r.right, r.bottom))
}

/// Moves and resizes the window, without changing its order or focus.
pub fn set_window_rect(hwnd: isize, r: PxRect) {
    // SAFETY: handle of our main window.
    unsafe {
        SetWindowPos(hwnd as HWND, std::ptr::null_mut(), r.0, r.1, r.2 - r.0, r.3 - r.1, SWP_NOZORDER | SWP_NOACTIVATE)
    };
}

/// Where on the screen the top-left corner of the window's content area is.
pub fn client_origin(hwnd: isize) -> Option<(i32, i32)> {
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: `p` is a valid POINT to write to.
    (unsafe { ClientToScreen(hwnd as HWND, &mut p) } != 0).then_some((p.x, p.y))
}

/// Reads a small block of pixels straight from the screen (RGB, row by row). Much faster than
/// photographing a whole monitor; it serves the selection magnifier, which updates every frame.
pub fn grab_pixels(x: i32, y: i32, w: i32, h: i32) -> Option<Vec<[u8; 3]>> {
    if w <= 0 || h <= 0 || w > 256 || h > 256 {
        return None;
    }
    // SAFETY: every GDI object created here is released before returning; `bits` is valid while the
    // bitmap exists and has `w * h * 4` bytes.
    unsafe {
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // negative = rows top to bottom
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let screen = GetDC(std::ptr::null_mut());
        let dc = CreateCompatibleDC(screen);
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let mut out = None;
        if !bmp.is_null() {
            if !bits.is_null() {
                let old = SelectObject(dc, bmp);
                if BitBlt(dc, 0, 0, w, h, screen, x, y, SRCCOPY) != 0 {
                    let buf = std::slice::from_raw_parts(bits.cast::<u8>(), (w * h * 4) as usize);
                    // The screen comes in BGRA.
                    out = Some(buf.as_chunks::<4>().0.iter().map(|p| [p[2], p[1], p[0]]).collect());
                }
                SelectObject(dc, old);
            }
            DeleteObject(bmp);
        }
        DeleteDC(dc);
        ReleaseDC(std::ptr::null_mut(), screen);
        out
    }
}

/// Work area (without the taskbar) of the monitor that contains the point.
pub fn work_area_at(p: (i32, i32)) -> Option<PxRect> {
    // SAFETY: `info` has cbSize filled in before the call.
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: p.0, y: p.1 }, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        (GetMonitorInfoW(monitor, &mut info) != 0)
            .then_some((info.rcWork.left, info.rcWork.top, info.rcWork.right, info.rcWork.bottom))
    }
}

/// Is the Esc key pressed right now?
pub fn escape_pressed() -> bool {
    // SAFETY: read-only query. The high bit means the key is pressed.
    unsafe { GetAsyncKeyState(VK_ESCAPE as i32) < 0 }
}

/// Is the left mouse button down right now, in any program?
pub fn left_button_down() -> bool {
    // SAFETY: read-only query. The high bit means the button is down.
    unsafe { GetAsyncKeyState(VK_LBUTTON as i32) < 0 }
}

pub fn right_button_down() -> bool {
    // SAFETY: read-only query.
    unsafe { GetAsyncKeyState(VK_RBUTTON as i32) < 0 }
}

/// Is the Enter key down right now, in any program?
pub fn enter_down() -> bool {
    // SAFETY: read-only query.
    unsafe { GetAsyncKeyState(VK_RETURN as i32) < 0 }
}

/// Local date and time: (year, month, day, hour, minute, second).
pub fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    // SAFETY: SYSTEMTIME is just numbers, so all zeros is valid; GetLocalTime fills it in.
    let t = unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// Title of the foreground window (empty if there's none or it has no title).
pub fn foreground_window_title() -> String {
    foreground_window_title_except(0)
}

/// Title of the foreground window, ignoring `skip` (our own window).
pub fn foreground_window_title_except(skip: isize) -> String {
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_null() || (skip != 0 && fg as isize == skip) {
        return String::new();
    }
    let mut buf = [0u16; 256];
    // SAFETY: the buffer has the size given; a null handle just returns 0.
    let len = unsafe { GetWindowTextW(fg, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

/// Makes the window invisible without hiding it from the system (or shows it again): it stays
/// "visible", drawing and receiving commands, but the compositor doesn't put it on the screen. Used
/// to change size and frame without the intermediate stages showing.
pub fn cloak(hwnd: isize, on: bool) {
    let value: i32 = on.into();
    // SAFETY: handle of our main window; the value is a 4-byte BOOL, as the API expects.
    unsafe {
        DwmSetWindowAttribute(hwnd as HWND, DWMWA_CLOAK as u32, (&raw const value).cast(), 4);
    }
}

/// Is the window showing on the screen (neither hidden in the tray nor minimized)?
pub fn is_on_screen(hwnd: isize) -> bool {
    // SAFETY: read-only queries; an invalid handle just returns 0.
    unsafe { IsWindowVisible(hwnd as HWND) != 0 && IsIconic(hwnd as HWND) == 0 }
}

pub fn minimize(hwnd: isize) {
    // SAFETY: handle of our main window.
    unsafe { ShowWindow(hwnd as HWND, SW_MINIMIZE) };
}

/// Hides the window completely (it also leaves the taskbar).
pub fn hide(hwnd: isize) {
    // SAFETY: handle of our main window.
    unsafe { ShowWindow(hwnd as HWND, SW_HIDE) };
}

/// Brings the window back, whether it's hidden or minimized, and puts it in the foreground.
pub fn show(hwnd: isize) {
    let h = hwnd as HWND;
    // SAFETY: handle of our main window.
    unsafe {
        ShowWindow(h, SW_SHOW);
        if IsIconic(h) != 0 {
            ShowWindow(h, SW_RESTORE);
        }
        SetForegroundWindow(h);
    }
}

/// Marks the window as a tool window (or back to a normal one). During the selection the window
/// covers every monitor; as a normal window, browsers take it for one that hides them and stop
/// drawing the page, so videos pause. Tool windows don't count, and they also stay out of the
/// taskbar and Alt+Tab while the layer is up.
pub fn set_tool_window(hwnd: isize, on: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SWP_FRAMECHANGED, SWP_NOMOVE, SetWindowLongPtrW, WS_EX_APPWINDOW,
        WS_EX_TOOLWINDOW,
    };
    let h = hwnd as HWND;
    // SAFETY: handle of our main window; only its extended style bits change.
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let (tool, app) = (WS_EX_TOOLWINDOW as isize, WS_EX_APPWINDOW as isize);
        let want = if on { (ex | tool) & !app } else { (ex & !tool) | app };
        if want != ex {
            SetWindowLongPtrW(h, GWL_EXSTYLE, want);
            SetWindowPos(h, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
        }
    }
}

/// Makes `child` end together with SnapKit. Windows doesn't do this by itself: if SnapKit
/// closes during a recording (Quit in the tray, or a crash), FFmpeg would go on recording the
/// screen in the background. The child joins a job that is closed, and its processes ended,
/// when SnapKit's process ends.
pub fn end_with_us(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    // One job for the whole run; its handle is never closed, so it closes when SnapKit exits.
    static JOB: OnceLock<usize> = OnceLock::new();
    // SAFETY: `info` is fully initialized (zeros plus the flag) and outlives the call.
    let job = *JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return 0;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const info).cast(), size);
        job as usize
    });
    if job != 0 {
        // SAFETY: both handles are valid; the child's lives as long as `child`.
        unsafe { AssignProcessToJobObject(job as _, child.as_raw_handle() as _) };
    }
}

/// What a second copy of SnapKit asks the one already running to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InstanceRequest {
    /// Show its window.
    Show,
    /// Start a region capture.
    Select,
    /// Close cleanly (used by the installer and uninstaller).
    Quit,
}

/// The right to be the only SnapKit running with these preferences, plus the signals that a
/// second copy uses to talk to it.
pub struct Instance {
    show: usize,
    select: usize,
    quit: usize,
    quit_global: usize,
    quit_requested: std::sync::atomic::AtomicBool,
}

fn instance_names(key: &str) -> [Vec<u16>; 5] {
    let wide = |s: String| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    [
        wide(format!("Local\\SnapKit-{key}")),
        wide(format!("Local\\SnapKit-{key}-show")),
        wide(format!("Local\\SnapKit-{key}-select")),
        wide(format!("Local\\SnapKit-{key}-quit")),
        wide("Local\\SnapKit.SingleInstance.Quit.v1".to_owned()),
    ]
}

impl Instance {
    /// Becomes the running copy, or returns `None` if another one already is.
    pub fn claim(key: &str) -> Option<Self> {
        use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
        use windows_sys::Win32::System::Threading::{CreateEventW, CreateMutexW};
        let [mutex, show, select, quit, quit_global] = instance_names(key);
        // SAFETY: the names are null-terminated; the handles are kept for the whole run, on
        // purpose (the mutex marks this copy as the running one until it exits).
        unsafe {
            let m = CreateMutexW(std::ptr::null(), 0, mutex.as_ptr());
            if m.is_null() {
                return None;
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(m);
                return None;
            }
            let show = CreateEventW(std::ptr::null(), 0, 0, show.as_ptr()) as usize;
            let select = CreateEventW(std::ptr::null(), 0, 0, select.as_ptr()) as usize;
            let quit = CreateEventW(std::ptr::null(), 0, 0, quit.as_ptr()) as usize;
            let quit_global = CreateEventW(std::ptr::null(), 0, 0, quit_global.as_ptr()) as usize;
            if show == 0 || select == 0 || quit == 0 {
                for h in [show, select, quit, quit_global] {
                    if h != 0 {
                        CloseHandle(h as _);
                    }
                }
                CloseHandle(m);
                return None;
            }
            Some(Self {
                show,
                select,
                quit,
                quit_global,
                quit_requested: std::sync::atomic::AtomicBool::new(false),
            })
        }
    }

    /// Waits for the next request from a second copy.
    pub fn wait(&self) -> Option<InstanceRequest> {
        use windows_sys::Win32::System::Threading::{INFINITE, WaitForMultipleObjects};
        let handles = [self.show as _, self.select as _, self.quit as _, self.quit_global as _];
        let count = if self.quit_global != 0 { 4 } else { 3 };
        // SAFETY: valid event handles, owned by `self`.
        match unsafe { WaitForMultipleObjects(count, handles.as_ptr(), 0, INFINITE) } {
            0 => Some(InstanceRequest::Show),
            1 => Some(InstanceRequest::Select),
            2 | 3 => {
                self.quit_requested.store(true, std::sync::atomic::Ordering::SeqCst);
                Some(InstanceRequest::Quit)
            }
            _ => None,
        }
    }

    /// True once if another copy asked this one to quit.
    pub fn take_quit_request(&self) -> bool {
        self.quit_requested.swap(false, std::sync::atomic::Ordering::SeqCst)
    }

    /// From a second copy: passes the request on to the running one. Returns `false` if it
    /// couldn't be reached.
    pub fn send(key: &str, request: InstanceRequest) -> bool {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
        let [_, show, select, quit, quit_global] = instance_names(key);
        if request == InstanceRequest::Quit {
            // SAFETY: null-terminated names; handles closed right after use.
            unsafe {
                let mut signaled = false;
                for name in [&quit, &quit_global] {
                    let event = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
                    if !event.is_null() {
                        signaled |= SetEvent(event) != 0;
                        CloseHandle(event);
                    }
                }
                if signaled {
                    // Wait up to 5 s for the running instance to flush writes and exit.
                    for _ in 0..50 {
                        let e = OpenEventW(EVENT_MODIFY_STATE, 0, quit.as_ptr());
                        if e.is_null() {
                            break;
                        }
                        CloseHandle(e);
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
                return signaled;
            }
        }
        let name = if request == InstanceRequest::Show { show } else { select };
        // The running copy may still be starting: it creates the signals right after the mutex.
        for _ in 0..20 {
            // SAFETY: the name is null-terminated; the handle is closed right after use.
            unsafe {
                let event = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
                if !event.is_null() {
                    let ok = SetEvent(event) != 0;
                    CloseHandle(event);
                    return ok;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    }
}

/// Visible top-level windows, from the frontmost to the backmost.
pub fn visible_windows(skip: isize) -> Vec<PxRect> {
    struct State {
        skip: isize,
        out: Vec<PxRect>,
    }
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: `lparam` is the pointer to `State` passed below, alive during EnumWindows.
        let state = unsafe { &mut *(lparam as *mut State) };
        if hwnd as isize == state.skip {
            return 1;
        }
        // SAFETY: `hwnd` comes from EnumWindows itself; the buffers have the size given.
        unsafe {
            if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
                return 1;
            }
            // "Cloaked" windows exist but don't show (other desktops, suspended apps).
            let mut cloaked: u32 = 0;
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED as u32,
                (&mut cloaked as *mut u32).cast::<c_void>(),
                size_of::<u32>() as u32,
            );
            if cloaked != 0 {
                return 1;
            }
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            // The DWM bounds exclude the invisible resize border.
            let hr = DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                (&mut r as *mut RECT).cast::<c_void>(),
                size_of::<RECT>() as u32,
            );
            if hr != 0 && GetWindowRect(hwnd, &mut r) == 0 {
                return 1;
            }
            if r.right - r.left >= 8 && r.bottom - r.top >= 8 {
                state.out.push((r.left, r.top, r.right, r.bottom));
            }
        }
        1
    }
    let mut state = State { skip, out: Vec::new() };
    // SAFETY: the callback only uses `state` during this call.
    unsafe { EnumWindows(Some(each), &mut state as *mut State as LPARAM) };
    state.out
}

/// Image of the current cursor and the top-left corner where it should be drawn. Returns `None` if
/// the cursor is hidden.
pub fn cursor_image() -> Option<(RgbaImage, (i32, i32))> {
    const SIZE: i32 = 128;
    // SAFETY: every GDI object created here is released before returning; the bit pointers are
    // valid while the bitmaps exist.
    unsafe {
        let mut ci: CURSORINFO = std::mem::zeroed();
        ci.cbSize = size_of::<CURSORINFO>() as u32;
        if GetCursorInfo(&mut ci) == 0 || ci.flags & CURSOR_SHOWING == 0 || ci.hCursor.is_null() {
            return None;
        }
        let mut ii: ICONINFO = std::mem::zeroed();
        if GetIconInfo(ci.hCursor, &mut ii) == 0 {
            return None;
        }
        let hotspot = (ii.xHotspot as i32, ii.yHotspot as i32);
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor);
        }

        // Draws the cursor on black and on white; the difference reveals the transparency.
        let draw_on = |fill: u8| -> Option<Vec<u8>> {
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIZE,
                biHeight: -SIZE, // negative = rows top to bottom
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if bmp.is_null() || bits.is_null() {
                if !bmp.is_null() {
                    DeleteObject(bmp);
                }
                DeleteDC(dc);
                return None;
            }
            let len = (SIZE * SIZE * 4) as usize;
            let buf = std::slice::from_raw_parts_mut(bits.cast::<u8>(), len);
            buf.fill(fill);
            let old = SelectObject(dc, bmp);
            DrawIconEx(dc, 0, 0, ci.hCursor, 0, 0, 0, std::ptr::null_mut(), DI_NORMAL);
            let out = buf.to_vec();
            SelectObject(dc, old);
            DeleteObject(bmp);
            DeleteDC(dc);
            Some(out)
        };
        let on_black = draw_on(0)?;
        let on_white = draw_on(255)?;
        let img = cursor_from_two_backgrounds(&on_black, &on_white, SIZE as u32);
        Some((img, (ci.ptScreenPos.x - hotspot.0, ci.ptScreenPos.y - hotspot.1)))
    }
}

/// Recovers color and alpha from the same drawing made on black and on white (BGRA).
fn cursor_from_two_backgrounds(on_black: &[u8], on_white: &[u8], size: u32) -> RgbaImage {
    let mut img = RgbaImage::new(size, size);
    for (i, px) in img.pixels_mut().enumerate() {
        let (b, w) = (&on_black[i * 4..i * 4 + 3], &on_white[i * 4..i * 4 + 3]);
        // In BGRA the middle channel is green; it serves as the reference for alpha.
        let diff = w[1] as i32 - b[1] as i32;
        if diff < 0 {
            // A pixel that inverts the background (e.g. the text cursor): becomes opaque dark gray.
            px.0 = [60, 60, 60, 255];
            continue;
        }
        let alpha = 255 - diff;
        if alpha == 0 {
            continue;
        }
        let un = |c: u8| ((c as i32 * 255) / alpha).min(255) as u8;
        px.0 = [un(b[2]), un(b[1]), un(b[0]), alpha as u8];
    }
    img
}

/// Colored frame around an area of the screen, to show what's being captured.
///
/// It's four thin windows, always on top, that don't receive clicks and don't show in screen
/// captures. They sit outside the area. It disappears when the value is dropped.
pub struct RegionFrame {
    thread_id: u32,
}

impl RegionFrame {
    pub fn show(rect: PxRect) -> Option<Self> {
        use std::sync::mpsc::channel;
        use windows_sys::Win32::Graphics::Gdi::CreateSolidBrush;
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows_sys::Win32::System::Threading::GetCurrentThreadId;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, MSG,
            RegisterClassW, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE,
        };

        const THICKNESS: i32 = 4;
        // COLORREF is 0x00BBGGRR: SnapKit's orange (255, 138, 76).
        const COLOR: u32 = 0x004C_8AFF;
        static CLASS: std::sync::OnceLock<()> = std::sync::OnceLock::new();

        let (tx, rx) = channel::<u32>();
        std::thread::spawn(move || {
            let class: Vec<u16> = "SnapKitMoldura\0".encode_utf16().collect();
            let t = THICKNESS;
            let (l, top, r, b) = rect;
            // Top, bottom, left and right bars, outside the area.
            let bars = [
                (l - t, top - t, r - l + 2 * t, t),
                (l - t, b, r - l + 2 * t, t),
                (l - t, top, t, b - top),
                (r, top, t, b - top),
            ];
            // SAFETY: the class and the windows are created and destroyed on this same thread; the
            // pointers passed (class name, MSG) live until the end of the block.
            unsafe {
                let instance = GetModuleHandleW(std::ptr::null());
                CLASS.get_or_init(|| {
                    let brush = CreateSolidBrush(COLOR);
                    let mut wc: WNDCLASSW = std::mem::zeroed();
                    wc.lpfnWndProc = Some(DefWindowProcW);
                    wc.hInstance = instance;
                    wc.hbrBackground = brush;
                    wc.lpszClassName = class.as_ptr();
                    if RegisterClassW(&wc) == 0 && !brush.is_null() {
                        DeleteObject(brush);
                    }
                });

                let mut windows = Vec::new();
                for (x, y, w, h) in bars {
                    let hwnd = CreateWindowExW(
                        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED,
                        class.as_ptr(),
                        std::ptr::null(),
                        WS_POPUP | WS_VISIBLE,
                        x,
                        y,
                        w.max(1),
                        h.max(1),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        instance,
                        std::ptr::null(),
                    );
                    if !hwnd.is_null() {
                        // A layered window only shows after it gets an opacity.
                        windows_sys::Win32::UI::WindowsAndMessaging::SetLayeredWindowAttributes(hwnd, 0, 255, 2);
                        SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
                        windows.push(hwnd);
                    }
                }
                if tx.send(GetCurrentThreadId()).is_ok() {
                    let mut msg: MSG = std::mem::zeroed();
                    // GetMessageW returns 0 on WM_QUIT (sent by `Drop`) and -1 on error.
                    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                        DispatchMessageW(&msg);
                    }
                }
                for hwnd in windows {
                    DestroyWindow(hwnd);
                }
            }
        });
        rx.recv_timeout(std::time::Duration::from_secs(2)).ok().map(|thread_id| Self { thread_id })
    }
}

impl Drop for RegionFrame {
    fn drop(&mut self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
        // SAFETY: it only posts a message; if the thread has already ended, the call fails
        // harmlessly.
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
    }
}

const CF_HDROP: u32 = 15;
const GMEM_MOVEABLE: u32 = 0x0002;
const GMEM_ZEROINIT: u32 = 0x0040;

#[repr(C)]
struct DropFilesHeader {
    p_files: u32,
    pt_x: i32,
    pt_y: i32,
    f_nc: i32,
    f_wide: i32,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const IID_IUNKNOWN: Guid = Guid {
    data1: 0x0000_0000,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const IID_IDATAOBJECT: Guid = Guid {
    data1: 0x0000_010E,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const IID_IDROPSOURCE: Guid = Guid {
    data1: 0x0000_0121,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

#[repr(C)]
#[derive(Clone, Copy)]
struct FormatEtc {
    cf_format: u16,
    ptd: *mut c_void,
    dw_aspect: u32,
    lindex: i32,
    tymed: u32,
}

#[repr(C)]
struct StgMedium {
    tymed: u32,
    hglobal: *mut c_void,
    p_unk_for_release: *mut c_void,
}

const S_OK: i32 = 0;
const S_FALSE: i32 = 1;
const E_INVALIDARG: i32 = -2147024809i32; // 0x80070057
const E_NOINTERFACE: i32 = -2147467262i32; // 0x80004002
const E_NOTIMPL: i32 = -2147467263i32; // 0x80004001
const E_OUTOFMEMORY: i32 = -2147024882i32; // 0x8007000E
const DV_E_FORMATETC: i32 = -2147221404i32; // 0x80040064
const OLE_E_ADVISENOTSUPPORTED: i32 = -2147221501i32; // 0x80040003
const DATA_S_SAMEFORMATETC: i32 = 0x0004_0130;
const DRAGDROP_S_DROP: i32 = 0x0004_0100;
const DRAGDROP_S_CANCEL: i32 = 0x0004_0101;
const DRAGDROP_S_USEDEFAULTCURSORS: i32 = 0x0004_0102;

const DVASPECT_CONTENT: u32 = 1;
const TYMED_HGLOBAL: u32 = 1;
const DATADIR_GET: u32 = 1;
const DROPEFFECT_COPY: u32 = 1;
const MK_LBUTTON: u32 = 0x0001;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalAlloc(uFlags: u32, dwBytes: usize) -> *mut c_void;
    fn GlobalLock(hMem: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(hMem: *mut c_void) -> i32;
    fn GlobalFree(hMem: *mut c_void) -> *mut c_void;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn OpenClipboard(hWndNewOwner: HWND) -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(uFormat: u32, hMem: *mut c_void) -> *mut c_void;
    fn CloseClipboard() -> i32;
    fn ReleaseCapture() -> i32;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn OleInitialize(pvReserved: *mut c_void) -> i32;
    fn DoDragDrop(
        pDataObj: *mut c_void,
        pDropSource: *mut c_void,
        dwOKEffects: u32,
        pdwEffect: *mut u32,
    ) -> i32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn SHCreateStdEnumFmtEtc(
        cfmt: u32,
        afmt: *const FormatEtc,
        ppenumFormatEtc: *mut *mut c_void,
    ) -> i32;
}

fn normalize_win_path(p: &std::path::Path) -> String {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf())
    };
    let s = abs.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned()
}

fn build_dropfiles_hglobal(paths: &[std::path::PathBuf]) -> Result<*mut c_void, String> {
    if paths.is_empty() {
        return Err("No files provided".into());
    }
    let mut wide_paths: Vec<u16> = Vec::new();
    for p in paths {
        let s = normalize_win_path(p);
        wide_paths.extend(s.encode_utf16());
        wide_paths.push(0);
    }
    wide_paths.push(0); // Double null-terminator

    let header_bytes = size_of::<DropFilesHeader>();
    let chars_bytes = wide_paths.len() * size_of::<u16>();
    let total_bytes = header_bytes + chars_bytes;

    // SAFETY: allocates movable, zero-initialized global memory and copies the DROPFILES header + UTF-16 paths.
    unsafe {
        let hglobal = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, total_bytes);
        if hglobal.is_null() {
            return Err("GlobalAlloc failed".into());
        }
        let ptr = GlobalLock(hglobal);
        if ptr.is_null() {
            GlobalFree(hglobal);
            return Err("GlobalLock failed".into());
        }
        let header = DropFilesHeader {
            p_files: header_bytes as u32,
            pt_x: 0,
            pt_y: 0,
            f_nc: 0,
            f_wide: 1,
        };
        std::ptr::copy_nonoverlapping(
            (&raw const header).cast::<u8>(),
            ptr.cast::<u8>(),
            header_bytes,
        );
        std::ptr::copy_nonoverlapping(
            wide_paths.as_ptr().cast::<u8>(),
            ptr.cast::<u8>().add(header_bytes),
            chars_bytes,
        );
        GlobalUnlock(hglobal);
        Ok(hglobal)
    }
}

/// Copies one or more files to the Windows clipboard as `CF_HDROP` so they can be pasted directly
/// into Explorer, browsers, Discord, Slack, WhatsApp, or email clients.
pub fn copy_files_to_clipboard(paths: &[std::path::PathBuf]) -> Result<(), String> {
    let hglobal = build_dropfiles_hglobal(paths)?;
    // SAFETY: standard Win32 clipboard sequence; on SetClipboardData success the system owns `hglobal`.
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            GlobalFree(hglobal);
            return Err("Could not open Windows clipboard".into());
        }
        EmptyClipboard();
        let ok = !SetClipboardData(CF_HDROP, hglobal).is_null();
        CloseClipboard();
        if !ok {
            GlobalFree(hglobal);
            return Err("SetClipboardData(CF_HDROP) failed".into());
        }
    }
    Ok(())
}

#[repr(C)]
struct IDataObjectVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_data: unsafe extern "system" fn(*mut c_void, *const FormatEtc, *mut StgMedium) -> i32,
    get_data_here: unsafe extern "system" fn(*mut c_void, *const FormatEtc, *mut StgMedium) -> i32,
    query_get_data: unsafe extern "system" fn(*mut c_void, *const FormatEtc) -> i32,
    get_canonical_format_etc: unsafe extern "system" fn(*mut c_void, *const FormatEtc, *mut FormatEtc) -> i32,
    set_data: unsafe extern "system" fn(*mut c_void, *const FormatEtc, *const StgMedium, i32) -> i32,
    enum_format_etc: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32,
    d_advise: unsafe extern "system" fn(*mut c_void, *const FormatEtc, u32, *mut c_void, *mut u32) -> i32,
    d_unadvise: unsafe extern "system" fn(*mut c_void, u32) -> i32,
    enum_d_advise: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32,
}

#[repr(C)]
struct FileDataObject {
    vtbl: *const IDataObjectVtbl,
    ref_count: std::sync::atomic::AtomicU32,
    paths: Vec<std::path::PathBuf>,
}

fn matches_hdrop_format(fmt: &FormatEtc) -> bool {
    fmt.cf_format == CF_HDROP as u16
        && (fmt.dw_aspect & DVASPECT_CONTENT) != 0
        && (fmt.tymed & TYMED_HGLOBAL) != 0
}

unsafe extern "system" fn data_obj_query_interface(
    this: *mut c_void,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> i32 {
    if ppv.is_null() || riid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `riid` and `ppv` are non-null and `this` is a valid `FileDataObject`.
    unsafe {
        let iid = *riid;
        if iid == IID_IUNKNOWN || iid == IID_IDATAOBJECT {
            *ppv = this;
            data_obj_add_ref(this);
            S_OK
        } else {
            *ppv = std::ptr::null_mut();
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn data_obj_add_ref(this: *mut c_void) -> u32 {
    // SAFETY: `this` points to a valid `FileDataObject`.
    let obj = unsafe { &*(this as *const FileDataObject) };
    obj.ref_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

unsafe extern "system" fn data_obj_release(this: *mut c_void) -> u32 {
    // SAFETY: `this` points to a heap-allocated `FileDataObject`.
    let obj = unsafe { &*(this as *const FileDataObject) };
    let prev = obj.ref_count.fetch_sub(1, std::sync::atomic::Ordering::Release);
    if prev == 1 {
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
        unsafe { drop(Box::from_raw(this as *mut FileDataObject)) };
    }
    prev - 1
}

unsafe extern "system" fn data_obj_get_data(
    this: *mut c_void,
    pformatetc_in: *const FormatEtc,
    pmedium: *mut StgMedium,
) -> i32 {
    if pformatetc_in.is_null() || pmedium.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: pointers validated above.
    unsafe {
        if !matches_hdrop_format(&*pformatetc_in) {
            return DV_E_FORMATETC;
        }
        let obj = &*(this as *const FileDataObject);
        match build_dropfiles_hglobal(&obj.paths) {
            Ok(hglobal) => {
                *pmedium = StgMedium {
                    tymed: TYMED_HGLOBAL,
                    hglobal,
                    p_unk_for_release: std::ptr::null_mut(),
                };
                S_OK
            }
            Err(_) => E_OUTOFMEMORY,
        }
    }
}

unsafe extern "system" fn data_obj_get_data_here(
    _this: *mut c_void,
    _pformatetc: *const FormatEtc,
    _pmedium: *mut StgMedium,
) -> i32 {
    E_NOTIMPL
}

unsafe extern "system" fn data_obj_query_get_data(
    _this: *mut c_void,
    pformatetc: *const FormatEtc,
) -> i32 {
    if pformatetc.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `pformatetc` is non-null.
    if unsafe { matches_hdrop_format(&*pformatetc) } {
        S_OK
    } else {
        DV_E_FORMATETC
    }
}

unsafe extern "system" fn data_obj_get_canonical_format_etc(
    _this: *mut c_void,
    _pformatect_in: *const FormatEtc,
    pformatetc_out: *mut FormatEtc,
) -> i32 {
    if !pformatetc_out.is_null() {
        // SAFETY: `pformatetc_out` is non-null.
        unsafe { (*pformatetc_out).ptd = std::ptr::null_mut() };
    }
    DATA_S_SAMEFORMATETC
}

unsafe extern "system" fn data_obj_set_data(
    _this: *mut c_void,
    _pformatetc: *const FormatEtc,
    _pmedium: *const StgMedium,
    _f_release: i32,
) -> i32 {
    E_NOTIMPL
}

unsafe extern "system" fn data_obj_enum_format_etc(
    _this: *mut c_void,
    dw_direction: u32,
    ppenum_format_etc: *mut *mut c_void,
) -> i32 {
    if ppenum_format_etc.is_null() {
        return E_INVALIDARG;
    }
    if dw_direction == DATADIR_GET {
        let fmt = FormatEtc {
            cf_format: CF_HDROP as u16,
            ptd: std::ptr::null_mut(),
            dw_aspect: DVASPECT_CONTENT,
            lindex: -1,
            tymed: TYMED_HGLOBAL,
        };
        // SAFETY: `SHCreateStdEnumFmtEtc` creates a standard COM enumerator for `fmt`.
        unsafe { SHCreateStdEnumFmtEtc(1, &fmt, ppenum_format_etc) }
    } else {
        // SAFETY: `ppenum_format_etc` is non-null.
        unsafe { *ppenum_format_etc = std::ptr::null_mut() };
        E_NOTIMPL
    }
}

unsafe extern "system" fn data_obj_d_advise(
    _this: *mut c_void,
    _pformatetc: *const FormatEtc,
    _advf: u32,
    _padv_sink: *mut c_void,
    _pdw_connection: *mut u32,
) -> i32 {
    OLE_E_ADVISENOTSUPPORTED
}

unsafe extern "system" fn data_obj_d_unadvise(_this: *mut c_void, _dw_connection: u32) -> i32 {
    OLE_E_ADVISENOTSUPPORTED
}

unsafe extern "system" fn data_obj_enum_d_advise(
    _this: *mut c_void,
    _ppenum_advise: *mut *mut c_void,
) -> i32 {
    OLE_E_ADVISENOTSUPPORTED
}

static DATA_OBJECT_VTBL: IDataObjectVtbl = IDataObjectVtbl {
    query_interface: data_obj_query_interface,
    add_ref: data_obj_add_ref,
    release: data_obj_release,
    get_data: data_obj_get_data,
    get_data_here: data_obj_get_data_here,
    query_get_data: data_obj_query_get_data,
    get_canonical_format_etc: data_obj_get_canonical_format_etc,
    set_data: data_obj_set_data,
    enum_format_etc: data_obj_enum_format_etc,
    d_advise: data_obj_d_advise,
    d_unadvise: data_obj_d_unadvise,
    enum_d_advise: data_obj_enum_d_advise,
};

#[repr(C)]
struct IDropSourceVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    query_continue_drag: unsafe extern "system" fn(*mut c_void, i32, u32) -> i32,
    give_feedback: unsafe extern "system" fn(*mut c_void, u32) -> i32,
}

#[repr(C)]
struct FileDropSource {
    vtbl: *const IDropSourceVtbl,
    ref_count: std::sync::atomic::AtomicU32,
}

unsafe extern "system" fn drop_src_query_interface(
    this: *mut c_void,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> i32 {
    if ppv.is_null() || riid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: `riid` and `ppv` are non-null and `this` is a valid `FileDropSource`.
    unsafe {
        let iid = *riid;
        if iid == IID_IUNKNOWN || iid == IID_IDROPSOURCE {
            *ppv = this;
            drop_src_add_ref(this);
            S_OK
        } else {
            *ppv = std::ptr::null_mut();
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn drop_src_add_ref(this: *mut c_void) -> u32 {
    // SAFETY: `this` points to a valid `FileDropSource`.
    let obj = unsafe { &*(this as *const FileDropSource) };
    obj.ref_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

unsafe extern "system" fn drop_src_release(this: *mut c_void) -> u32 {
    // SAFETY: `this` points to a heap-allocated `FileDropSource`.
    let obj = unsafe { &*(this as *const FileDropSource) };
    let prev = obj.ref_count.fetch_sub(1, std::sync::atomic::Ordering::Release);
    if prev == 1 {
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
        unsafe { drop(Box::from_raw(this as *mut FileDropSource)) };
    }
    prev - 1
}

unsafe extern "system" fn drop_src_query_continue_drag(
    _this: *mut c_void,
    f_escape_pressed: i32,
    grf_key_state: u32,
) -> i32 {
    if f_escape_pressed != 0 {
        DRAGDROP_S_CANCEL
    } else if (grf_key_state & MK_LBUTTON) == 0 {
        DRAGDROP_S_DROP
    } else {
        S_OK
    }
}

unsafe extern "system" fn drop_src_give_feedback(_this: *mut c_void, _dw_effect: u32) -> i32 {
    DRAGDROP_S_USEDEFAULTCURSORS
}

static DROP_SOURCE_VTBL: IDropSourceVtbl = IDropSourceVtbl {
    query_interface: drop_src_query_interface,
    add_ref: drop_src_add_ref,
    release: drop_src_release,
    query_continue_drag: drop_src_query_continue_drag,
    give_feedback: drop_src_give_feedback,
};

/// Starts a native Windows OLE file drag-and-drop operation (`CF_HDROP`) for the given paths.
/// Returns `true` if the user dropped the files onto a target that accepted them.
pub fn start_file_drag(paths: &[std::path::PathBuf]) -> bool {
    if paths.is_empty() {
        return false;
    }
    let data_obj = Box::into_raw(Box::new(FileDataObject {
        vtbl: &DATA_OBJECT_VTBL,
        ref_count: std::sync::atomic::AtomicU32::new(1),
        paths: paths.to_vec(),
    }));
    let drop_src = Box::into_raw(Box::new(FileDropSource {
        vtbl: &DROP_SOURCE_VTBL,
        ref_count: std::sync::atomic::AtomicU32::new(1),
    }));

    let mut effect: u32 = 0;
    // SAFETY: initializes OLE on this thread (harmless if already initialized), releases winit mouse
    // capture so the Shell modal drag loop receives mouse input, runs `DoDragDrop`, and releases our COM refs.
    let hr = unsafe {
        let _ = OleInitialize(std::ptr::null_mut());
        ReleaseCapture();
        let hr = DoDragDrop(
            data_obj.cast::<c_void>(),
            drop_src.cast::<c_void>(),
            DROPEFFECT_COPY,
            &mut effect,
        );
        data_obj_release(data_obj.cast::<c_void>());
        drop_src_release(drop_src.cast::<c_void>());
        hr
    };
    (hr == DRAGDROP_S_DROP || hr == S_OK || hr == S_FALSE) && effect != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_background_trick_recovers_alpha_and_color() {
        // 4 BGRA pixels: transparent, opaque red, 50% red, inverting.
        let on_black = [0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 128, 0, 255, 255, 255, 0];
        let on_white = [255, 255, 255, 0, 0, 0, 255, 0, 127, 127, 255, 0, 0, 0, 0, 0];
        let img = cursor_from_two_backgrounds(&on_black, &on_white, 2);
        assert_eq!(img.get_pixel(0, 0).0[3], 0);
        assert_eq!(img.get_pixel(1, 0).0, [255, 0, 0, 255]);
        let half = img.get_pixel(0, 1).0;
        assert_eq!((half[0], half[1], half[2], half[3]), (255, 0, 0, 128));
        assert_eq!(img.get_pixel(1, 1).0, [60, 60, 60, 255]);
    }

    #[test]
    fn virtual_screen_covers_at_least_one_monitor() {
        let (left, top, right, bottom) = virtual_screen();
        assert!(right - left >= 640 && bottom - top >= 480);
        assert!(monitor_count() >= 1);
        // The magnifier reads a small block of the screen; on servers without a screen the read can
        // fail, but must not hang.
        if let Some(pixels) = grab_pixels(left, top, 15, 15) {
            assert_eq!(pixels.len(), 225);
        }
        assert!(grab_pixels(0, 0, 0, 5).is_none());
    }

    #[test]
    fn window_list_and_cursor_do_not_crash() {
        let _ = visible_windows(0);
        let _ = cursor_pos();
        let _ = cursor_image();
    }

    #[test]
    fn region_frame_can_be_created_and_dropped() {
        // Outside every monitor, so it doesn't flash on the screen during the tests.
        let frame = RegionFrame::show((-30000, -30000, -29900, -29900));
        assert!(frame.is_some());
        drop(frame);
    }

    #[test]
    fn dropfiles_hglobal_and_com_data_object_work() {
        assert!(build_dropfiles_hglobal(&[]).is_err());
        let paths = vec![std::path::PathBuf::from(r"C:\Temp\a.png"), std::path::PathBuf::from(r"C:\Temp\b.mp4")];
        let hglobal = build_dropfiles_hglobal(&paths).unwrap();
        unsafe {
            let ptr = GlobalLock(hglobal).cast::<DropFilesHeader>();
            assert!(!ptr.is_null());
            assert_eq!((*ptr).p_files, 20);
            assert_eq!((*ptr).f_wide, 1);
            GlobalUnlock(hglobal);
            GlobalFree(hglobal);

            let data_obj = Box::into_raw(Box::new(FileDataObject {
                vtbl: &DATA_OBJECT_VTBL,
                ref_count: std::sync::atomic::AtomicU32::new(1),
                paths,
            }));
            let fmt = FormatEtc {
                cf_format: CF_HDROP as u16,
                ptd: std::ptr::null_mut(),
                dw_aspect: DVASPECT_CONTENT,
                lindex: -1,
                tymed: TYMED_HGLOBAL,
            };
            assert_eq!(data_obj_query_get_data(data_obj.cast(), &fmt), S_OK);
            let mut medium: StgMedium = std::mem::zeroed();
            assert_eq!(data_obj_get_data(data_obj.cast(), &fmt, &mut medium), S_OK);
            assert_eq!(medium.tymed, TYMED_HGLOBAL);
            assert!(!medium.hglobal.is_null());
            GlobalFree(medium.hglobal);
            assert_eq!(data_obj_release(data_obj.cast()), 0);

            assert_eq!(drop_src_query_continue_drag(std::ptr::null_mut(), 1, MK_LBUTTON), DRAGDROP_S_CANCEL);
            assert_eq!(drop_src_query_continue_drag(std::ptr::null_mut(), 0, 0), DRAGDROP_S_DROP);
            assert_eq!(drop_src_query_continue_drag(std::ptr::null_mut(), 0, MK_LBUTTON), S_OK);
        }
    }

    /// Manual diagnosis: `cargo test cursor_report -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn cursor_report() {
        match cursor_image() {
            None => println!("cursor: None"),
            Some((img, pos)) => {
                let visible = img.pixels().filter(|p| p.0[3] > 0).count();
                println!("cursor: pos={pos:?} visible pixels={visible}");
                img.save("target/cursor.png").unwrap();
            }
        }
    }
}
