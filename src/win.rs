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
    SetWindowPos, ShowWindow,
};

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
        if !bmp.is_null() && !bits.is_null() {
            let old = SelectObject(dc, bmp);
            if BitBlt(dc, 0, 0, w, h, screen, x, y, SRCCOPY) != 0 {
                let buf = std::slice::from_raw_parts(bits.cast::<u8>(), (w * h * 4) as usize);
                // The screen comes in BGRA.
                out = Some(buf.chunks_exact(4).map(|p| [p[2], p[1], p[0]]).collect());
            }
            SelectObject(dc, old);
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
    let mut buf = [0u16; 256];
    // SAFETY: the buffer has the size given; a null handle just returns 0.
    let len = unsafe { GetWindowTextW(GetForegroundWindow(), buf.as_mut_ptr(), buf.len() as i32) };
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
                let mut wc: WNDCLASSW = std::mem::zeroed();
                wc.lpfnWndProc = Some(DefWindowProcW);
                wc.hInstance = instance;
                wc.hbrBackground = CreateSolidBrush(COLOR);
                wc.lpszClassName = class.as_ptr();
                // Registering again for a second frame fails harmlessly: the class already exists.
                RegisterClassW(&wc);

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
                let _ = tx.send(GetCurrentThreadId());
                let mut msg: MSG = std::mem::zeroed();
                // GetMessageW returns 0 on WM_QUIT (sent by `Drop`) and -1 on error.
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    DispatchMessageW(&msg);
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
