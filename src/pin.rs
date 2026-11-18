//! Pin to Screen: borderless, always-on-top floating reference windows for captures.
//!
//! Each pinned capture runs on its own lightweight Win32 message-loop thread with a GDI DIB buffer,
//! independent of the main `eframe` window so pinned captures stay visible even when SnapKit is in
//! the system tray or taking new screenshots.
//!
//! Controls on a pinned window:
//! - **Drag** with the left mouse button to move across monitors
//! - **Mouse wheel** to zoom (`25%`–`400%`), anchored at the cursor
//! - **Ctrl + mouse wheel** to change opacity (`20%`–`100%`)
//! - **Ctrl + C** to copy the image to the clipboard
//! - **Right-click** for the context menu (Copy, Zoom, Opacity, Close)
//! - **Double-click**, **middle-click**, or **Esc** to close

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use image::RgbaImage;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, CreateSolidBrush, DIB_RGB_COLORS, DeleteObject,
    EndPaint, FillRect, HALFTONE, InvalidateRect, PAINTSTRUCT, SetBrushOrgEx, SetStretchBltMode,
    StretchDIBits, SRCCOPY,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, VK_CONTROL};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetCursorPos, GetMessageW, GetWindowLongPtrW, GetWindowRect,
    HTCAPTION, IDC_SIZEALL, LWA_ALPHA, LoadCursorW, MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG,
    PostQuitMessage, PostThreadMessageW, RegisterClassW, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW,
    SetForegroundWindow, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_CLOSE, WM_DESTROY,
    WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEWHEEL,
    WM_NCLBUTTONDOWN, WM_PAINT, WM_QUIT, WM_RBUTTONUP, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE, CS_DBLCLKS,
};

use crate::win::{self, PxRect};

const BORDER_PX: i32 = 2;
const MIN_PIN_SIDE: i32 = 48;
const ZOOM_MIN: f32 = 0.25;
const ZOOM_MAX: f32 = 4.0;

const CMD_COPY: usize = 1;
const CMD_ZOOM_50: usize = 2;
const CMD_ZOOM_100: usize = 3;
const CMD_ZOOM_200: usize = 4;
const CMD_ALPHA_100: usize = 10;
const CMD_ALPHA_75: usize = 11;
const CMD_ALPHA_50: usize = 12;
const CMD_ALPHA_30: usize = 13;
const CMD_CLOSE: usize = 99;

static ACTIVE_PINS: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();

fn active_list() -> &'static Mutex<Vec<u32>> {
    ACTIVE_PINS.get_or_init(|| Mutex::new(Vec::new()))
}

struct PinState {
    image: RgbaImage,
    /// Top-down 32-bit BGRA pixel buffer for `StretchDIBits`.
    bgra: Vec<u8>,
    accent_colorref: u32,
    zoom: f32,
    alpha: u8,
}

impl PinState {
    fn new(image: RgbaImage, accent_rgb: [u8; 3]) -> Self {
        let mut bgra = Vec::with_capacity(image.as_raw().len());
        for px in image.pixels() {
            let [r, g, b, a] = px.0;
            // Composite transparent pixels over dark slate so transparent stickers look clean.
            if a == 255 {
                bgra.extend_from_slice(&[b, g, r, 255]);
            } else {
                let alpha = a as u16;
                let inv = 255 - alpha;
                bgra.push(((b as u16 * alpha + 30 * inv) / 255) as u8);
                bgra.push(((g as u16 * alpha + 30 * inv) / 255) as u8);
                bgra.push(((r as u16 * alpha + 34 * inv) / 255) as u8);
                bgra.push(255);
            }
        }
        let accent_colorref =
            (accent_rgb[0] as u32) | ((accent_rgb[1] as u32) << 8) | ((accent_rgb[2] as u32) << 16);
        Self { image, bgra, accent_colorref, zoom: 1.0, alpha: 255 }
    }

    fn window_size(&self) -> (i32, i32) {
        let iw = ((self.image.width() as f32 * self.zoom).round() as i32).max(MIN_PIN_SIDE);
        let ih = ((self.image.height() as f32 * self.zoom).round() as i32).max(MIN_PIN_SIDE);
        (iw + BORDER_PX * 2, ih + BORDER_PX * 2)
    }
}

/// Spawns a new floating always-on-top window showing `image`.
///
/// If `at_rect` is `Some((l, t, r, b))`, the window opens right over that screen region; otherwise
/// it opens near the cursor on the active monitor.
pub fn spawn(image: RgbaImage, accent_rgb: [u8; 3], at_rect: Option<PxRect>) -> Result<(), String> {
    if image.width() == 0 || image.height() == 0 {
        return Err(t!("imagem vazia", "empty image").to_owned());
    }
    let (tx, rx) = std::sync::mpsc::channel::<Result<u32, String>>();
    std::thread::spawn(move || {
        let mut state = Box::new(PinState::new(image, accent_rgb));
        let (mut win_w, mut win_h) = state.window_size();
        let cursor = win::cursor_pos();
        let work = win::work_area_at(cursor).unwrap_or((0, 0, 1920, 1080));
        let max_w = ((work.2 - work.0) * 85 / 100).max(MIN_PIN_SIDE + BORDER_PX * 2);
        let max_h = ((work.3 - work.1) * 85 / 100).max(MIN_PIN_SIDE + BORDER_PX * 2);
        if at_rect.is_none() && (win_w > max_w || win_h > max_h) {
            let fit = (max_w as f32 / win_w as f32).min(max_h as f32 / win_h as f32).clamp(ZOOM_MIN, 1.0);
            state.zoom = fit;
            (win_w, win_h) = state.window_size();
        }
        let (x, y) = match at_rect {
            Some((l, top, _, _)) => (l - BORDER_PX, top - BORDER_PX),
            None => {
                let cx = (cursor.0 - win_w / 2).clamp(work.0 + 8, (work.2 - win_w - 8).max(work.0 + 8));
                let cy = (cursor.1 - win_h / 2).clamp(work.1 + 8, (work.3 - win_h - 8).max(work.1 + 8));
                (cx, cy)
            }
        };

        static CLASS: OnceLock<()> = OnceLock::new();
        let class_name: Vec<u16> = "SnapKitPinWindow\0".encode_utf16().collect();
        let title: Vec<u16> = "SnapKit Pin\0".encode_utf16().collect();

        // SAFETY: Win32 window registration, creation, and message loop all stay on this thread;
        // `state` lives until after `DestroyWindow` finishes.
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            CLASS.get_or_init(|| {
                let mut wc: WNDCLASSW = std::mem::zeroed();
                wc.style = CS_DBLCLKS;
                wc.lpfnWndProc = Some(pin_wnd_proc);
                wc.hInstance = instance;
                wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_SIZEALL);
                wc.lpszClassName = class_name.as_ptr();
                RegisterClassW(&wc);
            });

            let raw_state = Box::into_raw(state);
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_POPUP | WS_VISIBLE,
                x,
                y,
                win_w,
                win_h,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                drop(Box::from_raw(raw_state));
                let _ = tx.send(Err(t!(
                    "não foi possível criar a janela flutuante",
                    "could not create the floating window"
                )
                .to_owned()));
                return;
            }

            SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw_state as isize);
            SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
            InvalidateRect(hwnd, std::ptr::null(), 1);

            let tid = GetCurrentThreadId();
            if let Ok(mut list) = active_list().lock() {
                list.push(tid);
            }
            let _ = tx.send(Ok(tid));

            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            if let Ok(mut list) = active_list().lock() {
                list.retain(|&id| id != tid);
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DestroyWindow(hwnd);
            drop(Box::from_raw(raw_state));
        }
    });

    rx.recv_timeout(std::time::Duration::from_secs(2))
        .map_err(|_| t!("tempo esgotado ao fixar captura", "timed out pinning capture").to_owned())?
        .map(|_| ())
}

/// Closes every active pinned capture window cleanly (called on application shutdown).
pub fn close_all() {
    let tids: Vec<u32> = active_list()
        .lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default();
    for tid in tids {
        // SAFETY: posting `WM_QUIT` to a thread ID is safe even if the thread has already exited.
        unsafe {
            PostThreadMessageW(tid, WM_QUIT, 0, 0);
        }
    }
}

/// Number of currently open pinned windows.
#[cfg(test)]
pub fn active_count() -> usize {
    active_list().lock().map(|g| g.len()).unwrap_or(0)
}

unsafe fn set_pin_zoom(hwnd: HWND, state: &mut PinState, new_zoom: f32, anchor: Option<POINT>) {
    let clamped = new_zoom.clamp(ZOOM_MIN, ZOOM_MAX);
    if (clamped - state.zoom).abs() < 1e-4 {
        return;
    }
    let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: `wr` is valid stack storage.
    unsafe { GetWindowRect(hwnd, &mut wr) };
    let old_w = (wr.right - wr.left).max(1) as f32;
    let old_h = (wr.bottom - wr.top).max(1) as f32;
    state.zoom = clamped;
    let (new_w, new_h) = state.window_size();

    let (nx, ny) = if let Some(pt) = anchor {
        let rx = ((pt.x - wr.left) as f32 / old_w).clamp(0.0, 1.0);
        let ry = ((pt.y - wr.top) as f32 / old_h).clamp(0.0, 1.0);
        (pt.x - (new_w as f32 * rx).round() as i32, pt.y - (new_h as f32 * ry).round() as i32)
    } else {
        let cx = (wr.left + wr.right) / 2;
        let cy = (wr.top + wr.bottom) / 2;
        (cx - new_w / 2, cy - new_h / 2)
    };
    // SAFETY: valid `hwnd`.
    unsafe {
        SetWindowPos(hwnd, std::ptr::null_mut(), nx, ny, new_w, new_h, SWP_NOZORDER | SWP_NOACTIVATE);
        InvalidateRect(hwnd, std::ptr::null(), 1);
    }
}

unsafe fn set_pin_alpha(hwnd: HWND, state: &mut PinState, alpha: u8) {
    state.alpha = alpha.clamp(51, 255);
    // SAFETY: valid layered `hwnd`.
    unsafe {
        SetLayeredWindowAttributes(hwnd, 0, state.alpha, LWA_ALPHA);
    }
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

unsafe fn show_context_menu(hwnd: HWND, state: &mut PinState) {
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    // SAFETY: menu handles are created, used synchronously, and destroyed before returning.
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        let add = |flags: u32, id: usize, label: &str| {
            let w = wide(label);
            AppendMenuW(menu, flags, id, w.as_ptr());
        };
        add(MF_STRING, CMD_COPY, t!("Copiar imagem (Ctrl+C)", "Copy image (Ctrl+C)"));
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());

        let z_flag = |target: f32| {
            if (state.zoom - target).abs() < 0.05 { MF_STRING | MF_CHECKED } else { MF_STRING }
        };
        add(z_flag(0.5), CMD_ZOOM_50, "Zoom 50%");
        add(z_flag(1.0), CMD_ZOOM_100, t!("Tamanho real (100%)", "Actual size (100%)"));
        add(z_flag(2.0), CMD_ZOOM_200, "Zoom 200%");
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());

        let a_flag = |target: u8| {
            if state.alpha.abs_diff(target) <= 8 { MF_STRING | MF_CHECKED } else { MF_STRING }
        };
        add(a_flag(255), CMD_ALPHA_100, t!("Opacidade 100%", "Opacity 100%"));
        add(a_flag(191), CMD_ALPHA_75, t!("Opacidade 75%", "Opacity 75%"));
        add(a_flag(128), CMD_ALPHA_50, t!("Opacidade 50%", "Opacity 50%"));
        add(a_flag(77), CMD_ALPHA_30, t!("Opacidade 30%", "Opacity 30%"));
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());

        add(MF_STRING, CMD_CLOSE, t!("Fechar (Esc)", "Close (Esc)"));

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            0,
            hwnd,
            std::ptr::null(),
        ) as usize;
        DestroyMenu(menu);

        match cmd {
            CMD_COPY => copy_pin_image(&state.image),
            CMD_ZOOM_50 => set_pin_zoom(hwnd, state, 0.5, None),
            CMD_ZOOM_100 => set_pin_zoom(hwnd, state, 1.0, None),
            CMD_ZOOM_200 => set_pin_zoom(hwnd, state, 2.0, None),
            CMD_ALPHA_100 => set_pin_alpha(hwnd, state, 255),
            CMD_ALPHA_75 => set_pin_alpha(hwnd, state, 191),
            CMD_ALPHA_50 => set_pin_alpha(hwnd, state, 128),
            CMD_ALPHA_30 => set_pin_alpha(hwnd, state, 77),
            CMD_CLOSE => {
                PostQuitMessage(0);
            }
            _ => {}
        }
    }
}

unsafe extern "system" fn pin_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: `GWLP_USERDATA` is either 0 (during initial `CreateWindowExW`) or a valid `*mut PinState`
    // owned exclusively by this window's thread.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut PinState;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let state = unsafe { &mut *ptr };

    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            unsafe {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(hwnd, &mut ps);
                if !dc.is_null() {
                    let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    GetWindowRect(hwnd, &mut wr);
                    let ww = (wr.right - wr.left).max(1);
                    let wh = (wr.bottom - wr.top).max(1);

                    // Draw crisp accent border around the floating capture
                    let border_brush = CreateSolidBrush(state.accent_colorref);
                    if !border_brush.is_null() {
                        let top_r = RECT { left: 0, top: 0, right: ww, bottom: BORDER_PX };
                        let bot_r = RECT { left: 0, top: wh - BORDER_PX, right: ww, bottom: wh };
                        let left_r = RECT { left: 0, top: BORDER_PX, right: BORDER_PX, bottom: wh - BORDER_PX };
                        let right_r = RECT { left: ww - BORDER_PX, top: BORDER_PX, right: ww, bottom: wh - BORDER_PX };
                        FillRect(dc, &top_r, border_brush);
                        FillRect(dc, &bot_r, border_brush);
                        FillRect(dc, &left_r, border_brush);
                        FillRect(dc, &right_r, border_brush);
                        DeleteObject(border_brush);
                    }

                    let dst_w = (ww - BORDER_PX * 2).max(1);
                    let dst_h = (wh - BORDER_PX * 2).max(1);
                    let iw = state.image.width() as i32;
                    let ih = state.image.height() as i32;
                    let mut bmi: BITMAPINFO = std::mem::zeroed();
                    bmi.bmiHeader = BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: iw,
                        biHeight: -ih, // top-down
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..std::mem::zeroed()
                    };
                    SetStretchBltMode(dc, HALFTONE as i32);
                    SetBrushOrgEx(dc, 0, 0, std::ptr::null_mut());
                    StretchDIBits(
                        dc,
                        BORDER_PX,
                        BORDER_PX,
                        dst_w,
                        dst_h,
                        0,
                        0,
                        iw,
                        ih,
                        state.bgra.as_ptr().cast::<c_void>(),
                        &bmi,
                        DIB_RGB_COLORS,
                        SRCCOPY,
                    );
                }
                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_LBUTTONDOWN => {
            unsafe {
                ReleaseCapture();
                SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
            }
            0
        }
        WM_LBUTTONDBLCLK | WM_MBUTTONUP | WM_CLOSE | WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        WM_RBUTTONUP => {
            unsafe { show_context_menu(hwnd, state) };
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam >> 16) as i16) as f32 / 120.0;
            let ctrl = unsafe { (GetAsyncKeyState(VK_CONTROL as i32) as u16) & 0x8000 != 0 };
            if ctrl {
                let next_alpha = (state.alpha as f32 + delta * 20.0).round().clamp(51.0, 255.0) as u8;
                unsafe { set_pin_alpha(hwnd, state, next_alpha) };
            } else {
                let mut pt = POINT { x: 0, y: 0 };
                unsafe {
                    GetCursorPos(&mut pt);
                    let factor = 1.15_f32.powf(delta);
                    set_pin_zoom(hwnd, state, state.zoom * factor, Some(pt));
                }
            }
            0
        }
        WM_KEYDOWN => {
            let vk = wparam as u32;
            let ctrl = unsafe { (GetAsyncKeyState(VK_CONTROL as i32) as u16) & 0x8000 != 0 };
            if vk == 0x1B {
                // VK_ESCAPE
                unsafe { PostQuitMessage(0) };
                return 0;
            }
            if ctrl && vk == b'C' as u32 {
                copy_pin_image(&state.image);
                return 0;
            }
            if vk == b'0' as u32 || vk == b'1' as u32 {
                unsafe { set_pin_zoom(hwnd, state, 1.0, None) };
                return 0;
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_window_can_spawn_and_close_cleanly() {
        let img = RgbaImage::from_pixel(120, 80, image::Rgba([255, 138, 76, 255]));
        spawn(img, [255, 138, 76], Some((40, 40, 160, 120))).unwrap();
        assert!(active_count() >= 1);
        close_all();
        // Give the thread a brief moment to process WM_QUIT.
        for _ in 0..20 {
            if active_count() == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(active_count(), 0);
    }
}
