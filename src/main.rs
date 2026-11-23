#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
mod i18n;
mod audio;
mod autostart;
mod capture;
mod config;
mod editor;
mod export;
mod icons;
// The old name only existed on Windows.
#[cfg(windows)]
mod legacy;
mod library;
mod neural;
mod ocr;
#[cfg(target_os = "macos")]
mod ocr_vision;
#[cfg_attr(target_os = "macos", path = "pin_macos.rs")]
mod pin;
mod record;
mod render;
mod scroll;
mod steps;
mod theme;
mod tray;
mod ui;
mod update;
mod video_editor;
mod platform;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, ColorImage, CornerRadius, Margin, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use image::RgbaImage;
use raw_window_handle::HasWindowHandle;

use config::{Config, HotkeyCfg};
use icons::Icon;
use ui as kit;

const APP_NAME: &str = "SnapKit";

/// A shortcut with the system's command key, for the hints: "Ctrl C" on Windows, "⌘C" on macOS.
/// The keys themselves are read with `egui::Modifiers::COMMAND`, which is the same key.
fn cmd_key(key: &str) -> String {
    if cfg!(target_os = "macos") { format!("⌘{key}") } else { format!("Ctrl {key}") }
}

/// Where the error log is, written the way the system shows paths.
#[cfg(windows)]
const ERROR_LOG: &str = "%APPDATA%\\SnapKit\\error.log";
#[cfg(not(windows))]
const ERROR_LOG: &str = "~/Library/Application Support/SnapKit/error.log";

#[derive(Clone, Copy, PartialEq)]
enum CaptureKind {
    Region,
    FullScreen,
    /// Every monitor in a single image.
    AllScreens,
    /// Picks an area and scrolls it to the end, stitching the shots together.
    Scroll,
    /// Picks an area; the user scrolls and the program assembles the mosaic.
    Panoramic,
    /// Picks an area to record as video.
    Video,
    /// Like video, but without sound and converted to a GIF when it stops.
    Gif,
    /// Picks an area; each click in it becomes a numbered shot of a guide.
    Steps,
}

/// State shared between the interface and the capture and hotkey threads.
struct Shared {
    /// A finished capture, waiting for the interface to pick it up.
    pending: Mutex<Option<(CaptureKind, Result<capture::Shot, String>)>>,
    /// Request to open the selection: the mode and the title of the window that was in the
    /// foreground. The last value says whether the window was showing before the capture.
    select_request: Mutex<Option<(CaptureKind, String, bool)>>,
    /// True from the moment the capture is triggered until the selection ends.
    busy: AtomicBool,
    /// HWND of the main window (0 if unknown).
    hwnd: isize,
    /// Seconds to wait before taking the shot (to open menus, for example).
    delay_secs: AtomicU32,
    /// Draw the mouse cursor in the capture.
    include_cursor: AtomicBool,
    /// Active global hotkey IDs mapped to their capture mode.
    hotkey_map: Mutex<Vec<(u32, CaptureKind)>>,
    /// True while the window is cloaked (`DWMWA_CLOAK`), so it isn't mistaken for visible.
    cloaked: AtomicBool,
    /// Asked to show the main window from the tray or a second copy.
    open_requested: AtomicBool,
    /// Asked to close the program cleanly from the tray or `--quit`.
    quit_requested: AtomicBool,
}

/// Triggers a capture. Full screen takes the shot right away; the other modes open the selection,
/// which happens live, over the screen, without freezing anything.
fn start_capture(kind: CaptureKind, shared: Arc<Shared>, ctx: egui::Context) {
    if shared.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let hwnd = shared.hwnd;
        // Read before anything of ours takes the focus.
        let mut title = platform::foreground_window_title_except(hwnd);
        if matches!(kind, CaptureKind::FullScreen | CaptureKind::AllScreens) {
            let delay = shared.delay_secs.load(Ordering::Relaxed);
            let cursor = shared.include_cursor.load(Ordering::Relaxed);
            if hwnd != 0 {
                // Excluded from captures, the window doesn't show in the shot, without having to
                // leave the screen.
                platform::exclude_from_capture(hwnd, true);
                if delay > 0 && platform::is_on_screen(hwnd) {
                    // With a delay, get out of the way: the user wants to work with what's behind.
                    platform::minimize(hwnd);
                }
            }
            // A moment for the system to apply the exclusion before the shot.
            std::thread::sleep(Duration::from_millis(80));
            if delay > 0 {
                std::thread::sleep(Duration::from_secs(delay as u64));
            }
            if delay > 0 || title.is_empty() {
                let current = platform::foreground_window_title_except(hwnd);
                if !current.is_empty() {
                    title = current;
                }
            }
            let shot = if kind == CaptureKind::FullScreen {
                guarded(|| capture::capture_monitor_at_cursor(hwnd, cursor)).map(|mut s| {
                    if s.title.is_empty() || s.title == APP_NAME {
                        s.title = title;
                    }
                    s
                })
            } else {
                let all = platform::virtual_screen();
                guarded(|| capture::grab_virtual(all, cursor)).map(|image| capture::Shot {
                    image,
                    windows: Vec::new(),
                    title,
                    note: String::new(),
                })
            };
            *shared.pending.lock().unwrap() = Some((CaptureKind::FullScreen, shot));
            if hwnd != 0 {
                platform::exclude_from_capture(hwnd, false);
                platform::show(hwnd);
            }
        } else {
            let was_on_screen =
                hwnd != 0 && platform::is_on_screen(hwnd) && !shared.cloaked.load(Ordering::SeqCst);
            *shared.select_request.lock().unwrap() = Some((kind, title, was_on_screen));
            if hwnd != 0 {
                // Invisible while it changes size and loses its frame: without this the editor
                // flashes on the screen before the selection layer is ready.
                shared.cloaked.store(true, Ordering::SeqCst);
                platform::cloak(hwnd, true);
            }
            // The window becomes the selection layer; if it was in the tray or minimized, it comes
            // back.
            if hwnd != 0 && !was_on_screen {
                platform::show(hwnd);
            }
        }
        ctx.request_repaint();
    });
}

/// Selection in progress: the main window, transparent and frameless, covers every monitor.
struct Selecting {
    kind: CaptureKind,
    /// Windows visible when the selection started, in screen coordinates, front to back.
    windows: Vec<platform::PxRect>,
    /// Title of the window that was in the foreground, for searching the library.
    title: String,
    /// Where the mouse button went down. Kept here because, in the frame in which the button is
    /// released, egui no longer reports the origin of the drag.
    drag_origin: Option<Pos2>,
    /// Region already drawn, waiting for adjustment and confirmation (in interface points). `None`
    /// while the user hasn't dragged yet; when the button is released, the region stays armed
    /// instead of being captured right away.
    drawn: Option<Rect>,
    /// Adjustment in progress on the region already drawn.
    adjust: Option<Adjust>,
    /// The main window was showing before the capture (otherwise it hides again at the end).
    was_on_screen: bool,
    /// Quick annotation in progress, after the region was chosen.
    quick: Option<Quick>,
    /// When the selection started, so the layer appears even if something is delayed.
    started: Instant,
    /// Frames already drawn with the window covering every monitor.
    ready_frames: u8,
    /// Screen area the layer covers: every monitor on Windows, the one under the cursor on macOS.
    area: platform::PxRect,
    /// The layer has already been shown (the window starts invisible; see `platform::cloak`).
    revealed: bool,
}

/// Where the update check stands.
enum UpdateState {
    Idle,
    Checking,
    /// Checked: this is the latest version.
    Latest,
    /// A newer version is published.
    Available(update::Release),
    /// Downloading the installer of that version.
    Downloading(update::Release),
}

/// A result from the update work in the background.
enum UpdateMsg {
    Checked { manual: bool, result: Result<update::Release, String> },
    Downloaded { release: update::Release, result: Result<std::path::PathBuf, String> },
}

/// The main window, invisible while it returns from the selection to its normal size.
struct Settle {
    /// At the end, the window leaves the screen (tray or taskbar) instead of reappearing.
    put_away: bool,
    /// The window has already been put away; it only remains to confirm it didn't come back.
    hidden: bool,
    /// Time limit for the current stage, in case the frames don't come as expected.
    deadline: Instant,
}

/// Adjustment of the region already drawn: move the whole region, or pull one of the eight handles
/// (0 at the northwest corner, clockwise).
#[derive(Clone, Copy)]
enum Adjust {
    Move { grab: Pos2, start: Rect },
    Resize { handle: usize, start: Rect },
}

/// Smallest dimension accepted when resizing the region, in points.
const MIN_REGION: f32 = 10.0;

/// Cursor for each handle, in the same order as `handles`.
const HANDLE_CURSORS: [egui::CursorIcon; 8] = [
    egui::CursorIcon::ResizeNorthWest,
    egui::CursorIcon::ResizeNorth,
    egui::CursorIcon::ResizeNorthEast,
    egui::CursorIcon::ResizeEast,
    egui::CursorIcon::ResizeSouthEast,
    egui::CursorIcon::ResizeSouth,
    egui::CursorIcon::ResizeSouthWest,
    egui::CursorIcon::ResizeWest,
];

/// The eight handles of a region: 0 at the northwest corner, clockwise.
fn handles(area: Rect) -> [Rect; 8] {
    let grip = egui::vec2(14.0, 14.0);
    let at = |p: Pos2| Rect::from_center_size(p, grip);
    let (n, e, s, w) = (
        Pos2::new(area.center().x, area.min.y),
        Pos2::new(area.max.x, area.center().y),
        Pos2::new(area.center().x, area.max.y),
        Pos2::new(area.min.x, area.center().y),
    );
    [at(area.min), at(n), at(area.right_top()), at(e), at(area.max), at(s), at(area.left_bottom()), at(w)]
}

/// Resizes `start` by pulling the handle `handle` to `to`, without leaving `bounds`.
fn apply_handle(start: Rect, handle: usize, to: Pos2, bounds: Rect) -> Rect {
    let to = to.clamp(bounds.min, bounds.max);
    // Pulled past the minimum: locks the moving edge at the minimum distance from the fixed one.
    let west = to.x.min(start.max.x - MIN_REGION).max(bounds.min.x);
    let east = to.x.max(start.min.x + MIN_REGION).min(bounds.max.x);
    let north = to.y.min(start.max.y - MIN_REGION).max(bounds.min.y);
    let south = to.y.max(start.min.y + MIN_REGION).min(bounds.max.y);
    match handle {
        0 => Rect::from_min_max(Pos2::new(west, north), start.max),                            // northwest
        1 => Rect::from_min_max(Pos2::new(start.min.x, north), start.max),                     // north
        2 => Rect::from_min_max(Pos2::new(start.min.x, north), Pos2::new(east, start.max.y)),  // northeast
        3 => Rect::from_min_max(start.min, Pos2::new(east, start.max.y)),                      // east
        4 => Rect::from_min_max(start.min, Pos2::new(east, south)),                            // southeast
        5 => Rect::from_min_max(start.min, Pos2::new(start.max.x, south)),                     // south
        6 => Rect::from_min_max(Pos2::new(west, start.min.y), Pos2::new(start.max.x, south)),  // southwest
        _ => Rect::from_min_max(Pos2::new(west, start.min.y), start.max),                      // west
    }
}

/// Moves `start` by `delta`, keeping the whole region inside `bounds`.
fn clamp_move(start: Rect, delta: egui::Vec2, bounds: Rect) -> Rect {
    let mut r = start.translate(delta);
    if r.min.x < bounds.min.x { r = r.translate(egui::vec2(bounds.min.x - r.min.x, 0.0)); }
    if r.max.x > bounds.max.x { r = r.translate(egui::vec2(bounds.max.x - r.max.x, 0.0)); }
    if r.min.y < bounds.min.y { r = r.translate(egui::vec2(0.0, bounds.min.y - r.min.y)); }
    if r.max.y > bounds.max.y { r = r.translate(egui::vec2(0.0, bounds.max.y - r.max.y)); }
    r
}

/// A capture just taken, annotated in place, over the screen.
struct Quick {
    /// Where the capture is on the selection layer, in points.
    area: Rect,
    editor: editor::Editor,
    /// Adjustment in progress: move by the border or resize by a corner.
    adjust: Option<Adjust>,
    /// Only the annotations, drawn over the live screen while the adjustment lasts.
    overlay: Option<egui::TextureHandle>,
}

#[derive(Clone, Copy)]
enum QuickAction {
    Copy,
    Pin,
    Save,
    Editor,
    Cancel,
}

type OcrResult = Result<(String, ocr::Engine), String>;
type RedactResult = Result<Vec<(f32, f32, f32, f32)>, String>;

struct App {
    shared: Arc<Shared>,
    cfg: Config,
    selecting: Option<Selecting>,
    editor: Option<editor::Editor>,
    video_editor: Option<video_editor::VideoEditor>,
    status: String,
    hotkeys: Option<GlobalHotKeyManager>,
    /// Hotkeys currently registered with the system.
    active_hotkeys: Vec<HotKey>,
    /// Keeps the tray icon alive; `None` if the tray couldn't be created.
    tray: Option<tray::Tray>,
    show_prefs: bool,
    /// Draft of the primary shortcut in the preferences window, before Apply.
    hotkey_draft: HotkeyCfg,
    /// Draft of the per-mode shortcuts in the preferences window, before Apply.
    mode_hotkeys_draft: config::ModeHotkeysCfg,
    autostart: bool,
    library: library::Library,
    writer: library::Writer,
    /// Library file open in the editor, and its revision that has already been saved.
    current_file: Option<String>,
    saved_revision: u64,
    /// Multi-selected library files for batch operations (`Ctrl+Click` / `Shift+Click`).
    selected_files: Vec<String>,
    /// Anchor file for `Shift+Click` range selection in the library.
    selection_anchor: Option<String>,
    /// Thumbnails already loaded, by file name.
    thumbs: HashMap<String, TextureHandle>,
    show_library: bool,
    tools_expanded: bool,
    logo_tex: Option<TextureHandle>,
    search: String,
    library_filter: library::MediaFilter,
    /// Library entry currently being renamed / tagged: `(file, title_draft, tags_csv_draft)`.
    edit_meta: Option<(String, String, String)>,
    /// Panoramic capture being prepared or in progress.
    pano: Option<PanoUi>,
    /// Video recording being prepared or in progress.
    video: Option<VideoUi>,
    /// Step-by-step capture being prepared or in progress.
    steps: Option<StepsUi>,
    /// Result of a background video/GIF export or edit job, delivered by the worker thread.
    video_job_pending: Arc<Mutex<Option<Result<VideoJobDone, String>>>>,
    video_job_running: bool,
    /// Result of the OCR in progress, delivered by the recognition thread.
    ocr_pending: Arc<Mutex<Option<OcrResult>>>,
    ocr_running: bool,
    /// Result of automatic sensitive-data detection in progress.
    redact_pending: Arc<Mutex<Option<RedactResult>>>,
    redact_running: bool,
    /// Extracted text, shown in a window until the user closes it.
    ocr_text: Option<String>,
    /// Last image read, so it can be read again with the other engine.
    ocr_image: Option<RgbaImage>,
    /// Size (in points) and position (in screen pixels) of the window in normal use, to put it back
    /// in place after the panoramic panel.
    normal_size: Option<egui::Vec2>,
    normal_pos: Option<(i32, i32)>,
    /// Rectangle of the window in normal use, in screen pixels, to restore it after the selection.
    normal_rect: Option<platform::PxRect>,
    /// Frames still left reapplying `normal_rect` after leaving the selection.
    restore_frames: u8,
    /// The window is invisible, returning from the selection: at the end, it reappears or leaves
    /// the screen.
    settle: Option<Settle>,
    /// Open the save dialog as soon as the main window is back to normal.
    save_requested: bool,
    /// Frames left before hiding the window, when SnapKit starts in the tray (`--minimized`).
    hide_after_start: u8,
    /// Where the update check stands (Preferences > Updates).
    update_state: UpdateState,
    /// Last update problem, shown under the update buttons.
    update_note: String,
    /// Results of the update check and download, from the background.
    update_pending: Arc<Mutex<Option<UpdateMsg>>>,
    /// macOS: whether SnapKit's icon is in the Dock right now (see `sync_dock_icon`).
    #[cfg(target_os = "macos")]
    dock_visible: bool,
    /// `shutdown` already ran (it's also called when macOS ends the program with Quit).
    #[cfg(target_os = "macos")]
    shut_down: bool,
    /// For `on_exit`, which gets no context.
    #[cfg(target_os = "macos")]
    egui_ctx: egui::Context,
}

/// State of the small floating panel of the panoramic capture.
struct PanoUi {
    rect: platform::PxRect,
    title: String,
    session: Option<scroll::PanoramicSession>,
    /// Frame around the area, visible while the panel exists.
    _frame: Option<platform::RegionFrame>,
    /// Frames left before positioning the panel (the window is still leaving full screen).
    setup_frames: u8,
}

/// State of the floating panel of the step-by-step capture.
struct StepsUi {
    rect: platform::PxRect,
    title: String,
    /// `None` until the user presses Start.
    recorder: Option<steps::Recorder>,
    /// The keyboard can be read (macOS asks for Input Monitoring), so Enter also makes a step.
    keys_ok: bool,
    /// Frame around the area, visible while the panel exists.
    _frame: Option<platform::RegionFrame>,
    setup_frames: u8,
}

/// Result of a background video or GIF processing task.
enum VideoJobDone {
    /// New MP4 clip saved to the library.
    SavedClip { title: String, path: std::path::PathBuf },
    /// Existing MP4 in the library overwritten in-place.
    Overwritten { file: String, path: std::path::PathBuf },
    /// Animated GIF created in the library.
    SavedGif { file: String, title: String, path: std::path::PathBuf },
    /// MP4 exported to an external destination chosen by the user.
    ExportedExternal { path: std::path::PathBuf },
}

struct VideoUi {
    rect: platform::PxRect,
    title: String,
    recorder: Option<record::Session>,
    /// A recording that becomes a GIF when it stops.
    as_gif: bool,
    /// Active countdown deadline before recording starts automatically.
    countdown_until: Option<Instant>,
    /// Animated click ripple overlay while recording is active and unpaused.
    halo: Option<platform::ClickHalo>,
    /// Frame around the area, visible while the panel exists.
    _frame: Option<platform::RegionFrame>,
    setup_frames: u8,
}

const PANO_HUD_SIZE: egui::Vec2 = egui::vec2(460.0, 84.0);
const STEPS_HUD_SIZE: egui::Vec2 = egui::vec2(620.0, 84.0);
const VIDEO_HUD_SIZE: egui::Vec2 = egui::vec2(760.0, 84.0);

/// Turns the window into a small panel, always on top and excluded from captures, and places it
/// next to the area `rect`. Called every frame until `setup_frames` reaches zero: the window is
/// still leaving full screen and only accepts the position a few frames later.
fn hud_setup(ctx: &egui::Context, hwnd: isize, rect: platform::PxRect, size: egui::Vec2, setup_frames: &mut u8) {
    if *setup_frames == 0 {
        return;
    }
    if *setup_frames == 6 {
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(size));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        platform::exclude_from_capture(hwnd, true);
    }
    *setup_frames -= 1;
    if *setup_frames == 0 {
        // Right below the area; if it doesn't fit, above; as a last resort, at the bottom of the
        // monitor.
        let center = ((rect.0 + rect.2) / 2, (rect.1 + rect.3) / 2);
        let scale = platform::dpi_scale_at(center);
        let (w, h) = ((size.x * scale) as i32, (size.y * scale) as i32 + 48);
        let work = platform::work_area_at(center).unwrap_or((rect.0, rect.1, rect.2, rect.3 + h + 24));
        let x = (center.0 - w / 2).clamp(work.0, (work.2 - w).max(work.0));
        let y = if rect.3 + 12 + h <= work.3 {
            rect.3 + 12
        } else if rect.1 - 12 - h >= work.1 {
            rect.1 - 12 - h
        } else {
            work.3 - h - 12
        };
        platform::set_window_pos(hwnd, (x, y));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }
    ctx.request_repaint();
}
const NORMAL_MIN_SIZE: egui::Vec2 = egui::vec2(520.0, 320.0);
/// Width that Copy, Save, Pin, Read text, Auto-redact, Library and Preferences take up together in the bar.
const RIGHT_GROUP_WIDTH: f32 = 600.0;

fn apply_window_theme(ctx: &egui::Context, hwnd: isize, theme: &theme::Theme) {
    theme::apply(ctx, theme);
    let pal = theme::palette(theme);
    platform::apply_titlebar_theme(
        hwnd,
        pal.dark,
        [pal.bg_chrome.r(), pal.bg_chrome.g(), pal.bg_chrome.b()],
        [pal.text_primary.r(), pal.text_primary.g(), pal.text_primary.b()],
    );
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let cfg = Config::load();
        i18n::set(cfg.language.unwrap_or_else(i18n::Lang::system));
        theme::install_fonts(&cc.egui_ctx);
        let hwnd = match cc.window_handle().map(|h| h.as_raw()) {
            Ok(handle) => platform::window_id(handle),
            _ => 0,
        };
        apply_window_theme(&cc.egui_ctx, hwnd, theme::by_name(&cfg.theme));
        let shared = Arc::new(Shared {
            pending: Mutex::new(None),
            select_request: Mutex::new(None),
            busy: AtomicBool::new(false),
            hwnd,
            delay_secs: AtomicU32::new(cfg.delay_secs),
            include_cursor: AtomicBool::new(cfg.include_cursor),
            hotkey_map: Mutex::new(Vec::new()),
            cloaked: AtomicBool::new(false),
            open_requested: AtomicBool::new(false),
            quit_requested: AtomicBool::new(false),
        });

        let hotkeys = GlobalHotKeyManager::new().ok();
        spawn_hotkey_listener(shared.clone(), cc.egui_ctx.clone());
        spawn_instance_listener(shared.clone(), cc.egui_ctx.clone());

        let tray = {
            let (shared, ctx) = (shared.clone(), cc.egui_ctx.clone());
            tray::build(
                &cfg.hotkey.label(),
                Arc::new(move |action| match action {
                    tray::TrayAction::Region => {
                        start_capture(CaptureKind::Region, shared.clone(), ctx.clone());
                    }
                    tray::TrayAction::FullScreen => {
                        start_capture(CaptureKind::FullScreen, shared.clone(), ctx.clone());
                    }
                    tray::TrayAction::Open => {
                        shared.open_requested.store(true, Ordering::SeqCst);
                        platform::show(shared.hwnd);
                        ctx.request_repaint();
                    }
                    tray::TrayAction::Quit => {
                        shared.quit_requested.store(true, Ordering::SeqCst);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        ctx.request_repaint();
                    }
                }),
            )
            .ok()
        };

        // The size it was created with, until the user moves or resizes it. Without it, a capture
        // made before the window was ever shown (started in the tray) would leave it the size of
        // every monitor.
        let normal_rect = platform::window_rect(shared.hwnd);
        let mut app = Self {
            shared,
            hotkey_draft: cfg.hotkey.clone(),
            mode_hotkeys_draft: cfg.mode_hotkeys.clone(),
            cfg,
            selecting: None,
            editor: None,
            video_editor: None,
            status: String::new(),
            hotkeys,
            active_hotkeys: Vec::new(),
            tray,
            show_prefs: false,
            autostart: autostart::is_enabled(),
            library: library::Library::open(library::Library::default_dir()),
            writer: library::Writer::spawn(),
            current_file: None,
            saved_revision: 0,
            selected_files: Vec::new(),
            selection_anchor: None,
            thumbs: HashMap::new(),
            show_library: true,
            tools_expanded: false,
            logo_tex: None,
            search: String::new(),
            library_filter: library::MediaFilter::All,
            edit_meta: None,
            pano: None,
            video: None,
            steps: None,
            video_job_pending: Arc::new(Mutex::new(None)),
            video_job_running: false,
            ocr_pending: Arc::new(Mutex::new(None)),
            ocr_running: false,
            redact_pending: Arc::new(Mutex::new(None)),
            redact_running: false,
            ocr_text: None,
            ocr_image: None,
            normal_size: None,
            normal_pos: None,
            normal_rect,
            restore_frames: 0,
            settle: None,
            save_requested: false,
            hide_after_start: 0,
            update_state: UpdateState::Idle,
            update_note: String::new(),
            update_pending: Arc::new(Mutex::new(None)),
            #[cfg(target_os = "macos")]
            dock_visible: true,
            #[cfg(target_os = "macos")]
            shut_down: false,
            #[cfg(target_os = "macos")]
            egui_ctx: cc.egui_ctx.clone(),
        };
        // `snapkit --select` opens straight into region selection (shortcuts and tests).
        if std::env::args().any(|a| a == "--select") {
            start_capture(CaptureKind::Region, app.shared.clone(), cc.egui_ctx.clone());
        }
        app.status = match app.apply_hotkeys(&app.cfg.hotkey.clone(), &app.cfg.mode_hotkeys.clone()) {
            Ok(()) => tf!("Pronto. Atalho global: {}", "Ready. Global shortcut: {}", app.cfg.hotkey.label()),
            Err(e) => tf!("Atalho {} indisponível: {e}", "Shortcut {} unavailable: {e}", app.cfg.hotkey.label()),
        };
        // `--minimized` (Start with Windows): eframe shows the window after the first frame
        // whatever was asked, so it's kept cloaked, invisible, and hidden right after that.
        if std::env::args().any(|a| a == autostart::MINIMIZED_ARG) && app.shared.hwnd != 0 {
            app.shared.cloaked.store(true, Ordering::SeqCst);
            platform::cloak(app.shared.hwnd, true);
            app.hide_after_start = 2;
            cc.egui_ctx.request_repaint();
        }
        // macOS: without the Screen Recording permission captures only show the wallpaper. The
        // first time, the system asks; on Windows there's nothing to ask.
        platform::ensure_screen_permission();
        RUNNING.store(true, Ordering::SeqCst);
        app
    }

    /// Validates and replaces all registered global hotkeys (primary Region + enabled modes).
    /// On error, restores the previous set of hotkeys.
    fn apply_hotkeys(&mut self, primary: &HotkeyCfg, modes: &config::ModeHotkeysCfg) -> Result<(), String> {
        let manager = self
            .hotkeys
            .as_ref()
            .ok_or_else(|| t!("atalhos globais indisponíveis", "global shortcuts unavailable").to_owned())?;

        let mut candidates: Vec<(HotkeyCfg, HotKey, CaptureKind)> = Vec::new();
        let mut push_candidate = |cfg: &HotkeyCfg, kind: CaptureKind| -> Result<(), String> {
            if !cfg.is_safe() {
                let [ctrl, alt, _, win] = config::MODIFIER_LONG_NAMES;
                return Err(tf!(
                    "{} atrapalharia a digitação; inclua {ctrl}, {alt} ou {win}.",
                    "{} would get in the way of typing; include {ctrl}, {alt} or {win}.",
                    cfg.label()
                ));
            }
            let hk = cfg
                .to_hotkey()
                .ok_or_else(|| tf!("tecla desconhecida ({})", "unknown key ({})", cfg.label()))?;
            if candidates.iter().any(|(prev, _, _)| prev.conflicts_with(cfg)) {
                return Err(tf!(
                    "Atalho duplicado: {}. Cada modo precisa de um atalho diferente.",
                    "Duplicate shortcut: {}. Each mode needs a distinct shortcut.",
                    cfg.label()
                ));
            }
            candidates.push((cfg.clone(), hk, kind));
            Ok(())
        };

        push_candidate(primary, CaptureKind::Region)?;
        for (mode, opt) in modes.all() {
            if let Some(cfg) = opt {
                push_candidate(cfg, capture_kind_for_mode(mode))?;
            }
        }

        for old in &self.active_hotkeys {
            let _ = manager.unregister(*old);
        }

        let mut registered: Vec<HotKey> = Vec::with_capacity(candidates.len());
        let mut map: Vec<(u32, CaptureKind)> = Vec::with_capacity(candidates.len());
        for (cfg, hk, kind) in candidates {
            if let Err(e) = manager.register(hk) {
                for r in registered {
                    let _ = manager.unregister(r);
                }
                for old in &self.active_hotkeys {
                    let _ = manager.register(*old);
                }
                return Err(tf!("Não foi possível usar {}: {e}.", "Could not use {}: {e}.", cfg.label()));
            }
            map.push((hk.id(), kind));
            registered.push(hk);
        }

        self.active_hotkeys = registered;
        if let Ok(mut guard) = self.shared.hotkey_map.lock() {
            *guard = map;
        }
        Ok(())
    }

    /// Changes the interface language right away, including the tray menu, and saves the choice.
    fn set_language(&mut self, lang: i18n::Lang) {
        i18n::set(lang);
        self.cfg.language = Some(lang);
        self.save_cfg();
        if let Some(tray) = &self.tray {
            tray.relabel(&self.cfg.hotkey.label());
        }
    }

    /// The theme chosen in the preferences.
    fn theme(&self) -> &'static theme::Theme {
        theme::by_name(&self.cfg.theme)
    }

    /// Accent color of the current theme.
    fn accent(&self) -> Color32 {
        self.theme().accent
    }

    fn sync_titlebar(&self) {
        let pal = theme::palette(self.theme());
        platform::apply_titlebar_theme(
            self.shared.hwnd,
            pal.dark,
            [pal.bg_chrome.r(), pal.bg_chrome.g(), pal.bg_chrome.b()],
            [pal.text_primary.r(), pal.text_primary.g(), pal.text_primary.b()],
        );
    }

    fn save_cfg(&mut self) {
        if let Err(e) = self.cfg.save() {
            self.status = tf!("Não foi possível salvar as preferências: {e}", "Could not save the preferences: {e}");
        }
    }

    /// Asks GitHub for the latest version, in the background. `manual`: the user clicked
    /// **Check for updates** (rather than the daily automatic check), so failures are shown.
    fn start_update_check(&mut self, ctx: &egui::Context, manual: bool) {
        if matches!(self.update_state, UpdateState::Checking | UpdateState::Downloading(_)) {
            return;
        }
        self.update_state = UpdateState::Checking;
        self.update_note.clear();
        let (pending, ctx) = (self.update_pending.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = guarded(update::latest);
            *pending.lock().unwrap() = Some(UpdateMsg::Checked { manual, result });
            ctx.request_repaint();
        });
    }

    /// Downloads and checks the installer of `release`, in the background; then it's run.
    fn start_update_download(&mut self, ctx: &egui::Context, release: update::Release) {
        if self.video.is_some() {
            self.update_note = t!("Termine a gravação antes de atualizar.", "Finish the recording before updating.").to_owned();
            return;
        }
        let Some(asset) = release.installer.clone() else {
            self.update_note = t!("Essa versão não tem instalador; use a página de download.", "That release has no installer; use the download page.").to_owned();
            return;
        };
        self.update_state = UpdateState::Downloading(release.clone());
        self.update_note.clear();
        let (pending, ctx) = (self.update_pending.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = guarded(|| update::download(&asset));
            *pending.lock().unwrap() = Some(UpdateMsg::Downloaded { release, result });
            ctx.request_repaint();
        });
    }

    /// Takes in the results of the update work, and starts the daily check when it's due.
    fn update_logic(&mut self, ctx: &egui::Context) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if self.cfg.check_updates
            && matches!(self.update_state, UpdateState::Idle | UpdateState::Latest)
            && now.saturating_sub(self.cfg.last_update_check) >= 24 * 60 * 60
        {
            // Recorded before the answer: a check that fails isn't retried on every frame.
            self.cfg.last_update_check = now;
            self.save_cfg();
            self.start_update_check(ctx, false);
        }
        let Some(msg) = self.update_pending.lock().unwrap().take() else { return };
        match msg {
            UpdateMsg::Checked { manual, result } => {
                self.cfg.last_update_check = now;
                self.save_cfg();
                match result {
                    Ok(release) if update::is_newer(&release.version, update::CURRENT) => {
                        self.status = tf!(
                            "SnapKit {} disponível. Veja em Preferências > Atualizações.",
                            "SnapKit {} is available. See Preferences > Updates.",
                            release.version
                        );
                        self.update_state = UpdateState::Available(release);
                    }
                    Ok(_) => {
                        self.update_state = UpdateState::Latest;
                    }
                    Err(e) => {
                        self.update_state = UpdateState::Idle;
                        if manual {
                            self.update_note = e;
                        } else {
                            log_error(&format!("automatic update check: {e}"));
                        }
                    }
                }
            }
            UpdateMsg::Downloaded { release, result } => match result.and_then(|path| {
                // Captures still being written finish first; then the installer takes over.
                self.shutdown(ctx);
                update::run_installer(&path)
            }) {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    self.update_note = e;
                    self.update_state = UpdateState::Available(release);
                }
            },
        }
    }

    /// Preferences > Updates: the version, the check button and the daily check.
    fn updates_ui(&mut self, ui: &mut egui::Ui) {
        let pal = theme::current_palette(ui.ctx());
        ui.label(
            egui::RichText::new(tf!("Versão instalada: {}", "Installed version: {}", update::CURRENT))
                .size(12.0)
                .color(pal.text_secondary),
        );
        ui.add_space(4.0);
        let mut check = false;
        let mut install = None;
        let mut page = None;
        ui.horizontal(|ui| {
            let busy = matches!(self.update_state, UpdateState::Checking | UpdateState::Downloading(_));
            check = ui
                .add_enabled_ui(!busy, |ui| {
                    kit::ghost_button(
                        ui,
                        Some(Icon::Refresh),
                        t!("Verificar atualização", "Check for updates"),
                        false,
                    )
                })
                .inner
                .on_hover_text(t!(
                    "Consulta a versão mais recente no GitHub. Só o nome e a versão do SnapKit são enviados.",
                    "Looks up the latest version on GitHub. Only SnapKit's name and version are sent."
                ))
                .clicked();
            match &self.update_state {
                UpdateState::Idle => {}
                UpdateState::Checking => {
                    ui.spinner();
                    ui.label(t!("Verificando…", "Checking…"));
                }
                UpdateState::Latest => {
                    ui.colored_label(pal.success, t!("Você tem a versão mais recente.", "You have the latest version."));
                }
                UpdateState::Available(release) => {
                    ui.colored_label(self.accent(), tf!("Versão {} disponível.", "Version {} is available.", release.version));
                }
                UpdateState::Downloading(release) => {
                    ui.spinner();
                    ui.label(tf!("Baixando a versão {}…", "Downloading version {}…", release.version));
                }
            }
        });
        if let UpdateState::Available(release) = &self.update_state {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if update::is_installed() {
                    if kit::primary_button(ui, Some(Icon::UpdateArrow), t!("Atualizar agora", "Update now"))
                        .on_hover_text(t!(
                            "Baixa o instalador, confere o SHA-256, instala e abre o SnapKit de novo. Preferências, estilos e biblioteca são mantidos.",
                            "Downloads the installer, checks its SHA-256, installs it and opens SnapKit again. Preferences, styles and library are kept."
                        ))
                        .clicked()
                    {
                        install = Some(release.clone());
                    }
                } else if kit::primary_button(
                    ui,
                    Some(Icon::ExternalOpen),
                    t!("Abrir a página de download", "Open the download page"),
                )
                .on_hover_text(if cfg!(target_os = "macos") {
                    t!(
                        "Baixe o .dmg novo e substitua o SnapKit em Aplicativos.",
                        "Download the new .dmg and replace SnapKit in Applications."
                    )
                } else {
                    t!(
                        "Esta cópia é a portátil (zip): baixe o zip novo e extraia por cima.",
                        "This is the portable copy (zip): download the new zip and extract it over this one."
                    )
                })
                .clicked()
                {
                    page = Some(release.page.clone());
                }
                if kit::ghost_button(ui, Some(Icon::ExternalOpen), t!("Novidades", "What's new"), false).clicked() {
                    page = Some(release.page.clone());
                }
            });
        }
        if !self.update_note.is_empty() {
            ui.colored_label(ui.visuals().warn_fg_color, &self.update_note);
        }
        ui.add_space(4.0);
        let mut auto = self.cfg.check_updates;
        ui.checkbox(&mut auto, t!("Verificar automaticamente (uma vez por dia)", "Check automatically (once a day)"))
            .on_hover_text(t!(
                "Só avisa quando houver versão nova: nada é instalado sem um clique seu. Desligado, o SnapKit não usa a internet.",
                "Only tells you when there's a new version: nothing is installed without a click from you. When off, SnapKit doesn't use the internet."
            ));
        if auto != self.cfg.check_updates {
            self.cfg.check_updates = auto;
            self.save_cfg();
        }
        let ctx = ui.ctx().clone();
        if check {
            self.start_update_check(&ctx, true);
        }
        if let Some(release) = install {
            self.start_update_download(&ctx, release);
        }
        if let Some(url) = page {
            update::open_page(&url);
        }
    }

    fn prefs_ui(&mut self, ctx: &egui::Context) {
        let mut open = self.show_prefs;
        let pal = theme::current_palette(ctx);
        egui::Window::new(t!("Preferências", "Preferences"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(500.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(560.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;

                        kit::section_card(ui, t!("Aparência", "Appearance"), |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(t!("Idioma:", "Language:")).color(pal.text_secondary));
                                let mut lang = i18n::current();
                                egui::ComboBox::from_id_salt("language")
                                    .selected_text(lang.name())
                                    .width(180.0)
                                    .show_ui(ui, |ui| {
                                        for option in i18n::Lang::ALL {
                                            ui.selectable_value(&mut lang, option, option.name());
                                        }
                                    });
                                if lang != i18n::current() {
                                    self.set_language(lang);
                                }
                            });
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(t!(
                                    "Tema da interface (muda as cores na hora):",
                                    "Interface theme (updates colors right away):"
                                ))
                                .size(12.0)
                                .color(pal.text_secondary),
                            );
                            ui.add_space(4.0);
                            let mut picked_theme: Option<String> = None;
                            egui::Grid::new("theme_swatch_grid")
                                .num_columns(3)
                                .spacing([6.0, 6.0])
                                .show(ui, |ui| {
                                    for (idx, t_item) in theme::THEMES.iter().enumerate() {
                                        let active = self.cfg.theme == t_item.name;
                                        let (rect, resp) = ui.allocate_exact_size(
                                            egui::vec2(146.0, 34.0),
                                            Sense::click(),
                                        );
                                        if resp.hovered() {
                                            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                                        }
                                        let chip_bg = if active {
                                            pal.accent.gamma_multiply(0.14)
                                        } else if resp.hovered() {
                                            pal.bg_elevated
                                        } else {
                                            pal.bg_chrome
                                        };
                                        let chip_border = if active {
                                            Stroke::new(1.5, pal.accent)
                                        } else if resp.hovered() {
                                            Stroke::new(1.0, pal.border_strong)
                                        } else {
                                            Stroke::new(1.0, pal.border_subtle)
                                        };
                                        let p = ui.painter_at(rect);
                                        p.rect_filled(rect, CornerRadius::same(7), chip_bg);
                                        p.rect_stroke(rect, CornerRadius::same(7), chip_border, StrokeKind::Inside);

                                        let sw_center = Pos2::new(rect.min.x + 16.0, rect.center().y);
                                        p.circle_filled(sw_center, 8.0, t_item.panel_fill);
                                        p.circle_stroke(sw_center, 8.0, Stroke::new(1.0, pal.border_strong));
                                        p.circle_filled(sw_center + egui::vec2(4.0, 2.0), 4.5, t_item.accent);

                                        let name = theme::display_name(t_item.name);
                                        let font = if active {
                                            egui::FontId::new(11.5, theme::semibold_family())
                                        } else {
                                            egui::FontId::proportional(11.5)
                                        };
                                        p.text(
                                            Pos2::new(rect.min.x + 30.0, rect.center().y),
                                            egui::Align2::LEFT_CENTER,
                                            name,
                                            font,
                                            if active { pal.text_primary } else { pal.text_secondary },
                                        );
                                        if active {
                                            icons::paint(
                                                &p,
                                                ctx,
                                                Icon::Check,
                                                Pos2::new(rect.max.x - 12.0, rect.center().y),
                                                12.0,
                                                pal.accent,
                                            );
                                        }
                                        if resp.clicked() && !active {
                                            picked_theme = Some(t_item.name.to_owned());
                                        }
                                        if idx % 3 == 2 {
                                            ui.end_row();
                                        }
                                    }
                                });
                            if let Some(name) = picked_theme {
                                self.cfg.theme = name;
                                apply_window_theme(ctx, self.shared.hwnd, self.theme());
                                let accent = self.accent();
                                if let Some(editor) = &mut self.editor {
                                    editor.set_accent(accent);
                                }
                                if let Some(ve) = &mut self.video_editor {
                                    ve.set_accent(accent);
                                }
                                self.save_cfg();
                            }
                        });

                        kit::section_card(ui, t!("Atalhos", "Shortcuts"), |ui| {
                            ui.label(
                                egui::RichText::new(t!("Atalho global de captura (Região)", "Global capture shortcut (Region)"))
                                    .size(12.0)
                                    .color(pal.text_secondary),
                            );
                            ui.horizontal(|ui| {
                                hotkey_editor_row(ui, "key", &mut self.hotkey_draft);
                            });
                            ui.collapsing(t!("Atalhos para outros modos", "Shortcuts for other modes"), |ui| {
                                egui::Grid::new("mode_hotkeys_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                                    for (idx, mode) in ALL_SHORTCUT_MODES.iter().copied().enumerate() {
                                        let mut enabled = self.mode_hotkeys_draft.get(mode).is_some();
                                        if ui.checkbox(&mut enabled, mode_hotkey_label(mode)).changed() {
                                            let next = if enabled { Some(HotkeyCfg::default()) } else { None };
                                            self.mode_hotkeys_draft.set(mode, next);
                                        }
                                        ui.horizontal(|ui| {
                                            if let Some(hk) = self.mode_hotkeys_draft.get_mut(mode) {
                                                hotkey_editor_row(ui, ("mode_hk", idx), hk);
                                            } else {
                                                ui.weak(t!("Desativado", "Disabled"));
                                            }
                                        });
                                        ui.end_row();
                                    }
                                });
                            });
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                let changed = self.hotkey_draft != self.cfg.hotkey || self.mode_hotkeys_draft != self.cfg.mode_hotkeys;
                                if ui
                                    .add_enabled_ui(changed, |ui| {
                                        kit::primary_button(ui, Some(Icon::Check), t!("Aplicar atalhos", "Apply shortcuts"))
                                    })
                                    .inner
                                    .clicked()
                                {
                                    let primary = self.hotkey_draft.clone();
                                    let modes = self.mode_hotkeys_draft.clone();
                                    match self.apply_hotkeys(&primary, &modes) {
                                        Ok(()) => {
                                            self.cfg.hotkey = primary;
                                            self.cfg.mode_hotkeys = modes;
                                            self.save_cfg();
                                            if let Some(tray) = &self.tray {
                                                tray.relabel(&self.cfg.hotkey.label());
                                            }
                                            self.status = tf!("Atalho global: {}", "Global shortcut: {}", self.cfg.hotkey.label());
                                        }
                                        Err(e) => {
                                            self.status = e;
                                        }
                                    }
                                }
                                ui.label(
                                    egui::RichText::new(tf!("Atual: {}", "Current: {}", self.cfg.hotkey.label()))
                                        .size(12.0)
                                        .color(pal.text_muted),
                                );
                            });
                        });

                        kit::section_card(ui, t!("Captura", "Capture"), |ui| {
                            let mut quick = self.cfg.quick_annotate;
                            ui.checkbox(
                                &mut quick,
                                t!(
                                    "Depois de escolher a região, anotar ali mesmo (Enter copia)",
                                    "After choosing the region, annotate right there (Enter copies)"
                                ),
                            )
                            .on_hover_text(t!(
                                "Desligado, a captura de região abre direto no editor completo.",
                                "When off, a region capture opens straight in the full editor."
                            ));
                            if quick != self.cfg.quick_annotate {
                                self.cfg.quick_annotate = quick;
                                self.save_cfg();
                            }
                        });

                        kit::section_card(ui, t!("Sistema", "System"), |ui| {
                            let tray_ok = self.tray.is_some();
                            let mut close_to_tray = self.cfg.close_to_tray;
                            ui.add_enabled(
                                tray_ok,
                                egui::Checkbox::new(
                                    &mut close_to_tray,
                                    if cfg!(target_os = "macos") {
                                        t!("Fechar a janela mantém o SnapKit na barra de menus", "Closing the window keeps SnapKit in the menu bar")
                                    } else {
                                        t!("Fechar a janela mantém o SnapKit na bandeja", "Closing the window keeps SnapKit in the tray")
                                    },
                                ),
                            );
                            if close_to_tray != self.cfg.close_to_tray {
                                self.cfg.close_to_tray = close_to_tray;
                                self.save_cfg();
                            }
                            let mut autostart = self.autostart;
                            let autostart_label = if cfg!(target_os = "macos") {
                                t!("Abrir ao iniciar sessão (na barra de menus)", "Start at login (in the menu bar)")
                            } else {
                                t!("Iniciar com o Windows (na bandeja)", "Start with Windows (in the tray)")
                            };
                            ui.checkbox(&mut autostart, autostart_label);
                            if autostart != self.autostart {
                                match autostart::set_enabled(autostart) {
                                    Ok(()) => self.autostart = autostart,
                                    Err(e) => self.status = tf!("Não foi possível alterar a inicialização: {e}", "Could not change the startup setting: {e}"),
                                }
                            }
                            if !tray_ok {
                                ui.colored_label(
                                    pal.warning,
                                    if cfg!(target_os = "macos") {
                                        t!(
                                            "O ícone da barra de menus não pôde ser criado nesta sessão.",
                                            "The menu bar icon could not be created in this session."
                                        )
                                    } else {
                                        t!("O ícone da bandeja não pôde ser criado nesta sessão.", "The tray icon could not be created in this session.")
                                    },
                                );
                            }
                        });

                        kit::section_card(ui, t!("Atualizações", "Updates"), |ui| {
                            self.updates_ui(ui);
                        });
                    });
            });
        self.show_prefs = open;
    }

    /// Recognizes the text in the image in the background; the result arrives through
    /// `ocr_pending`.
    fn start_ocr(&mut self, ctx: &egui::Context, image: RgbaImage, mode: ocr::Mode) {
        if self.ocr_running {
            return;
        }
        self.ocr_running = true;
        self.status = match mode {
            ocr::Mode::Printed => t!("Lendo o texto da imagem…", "Reading the text in the image…").to_owned(),
            ocr::Mode::Handwriting => t!("Lendo como letra manuscrita…", "Reading as handwriting…").to_owned(),
        };
        self.ocr_image = Some(image.clone());
        let (pending, ctx) = (self.ocr_pending.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = guarded(|| ocr::recognize(&image, mode));
            *pending.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    /// Receives the OCR result, copies the text and shows the window with it.
    fn ocr_ui(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.ocr_pending.lock().unwrap().take() {
            self.ocr_running = false;
            match result {
                Ok((text, _)) if text.trim().is_empty() => {
                    self.status = t!("Nenhum texto encontrado nessa área.", "No text found in that area.").to_owned();
                }
                Ok((text, engine)) => {
                    self.status = match copy_text(&text) {
                        Ok(()) => tf!(
                            "Texto copiado ({} linhas, lido com {}).",
                            "Text copied ({} lines, read with {}).",
                            text.lines().count(),
                            engine.name()
                        ),
                        Err(e) => tf!("Texto lido, mas não foi possível copiar: {e}", "Text read, but could not be copied: {e}"),
                    };
                    self.ocr_text = Some(text);
                }
                Err(e) => self.status = tf!("Não foi possível ler o texto: {e}", "Could not read the text: {e}"),
            }
        }
        let Some(text) = &mut self.ocr_text else { return };
        let mut open = true;
        let mut copy = false;
        let mut reread = false;
        let can_reread = !self.ocr_running && self.ocr_image.is_some();
        egui::Window::new(t!("Texto extraído", "Extracted text"))
            .open(&mut open)
            .collapsible(false)
            .default_size([420.0, 280.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    copy = kit::primary_button(ui, Some(Icon::Copy), t!("Copiar", "Copy")).clicked();
                    reread = ui
                        .add_enabled_ui(can_reread, |ui| {
                            kit::ghost_button(
                                ui,
                                Some(Icon::Edit),
                                t!("Ler como manuscrito", "Read as handwriting"),
                                false,
                            )
                        })
                        .inner
                        .on_hover_text(t!(
                            "Relê a mesma área com o motor neural. Melhor para letra manuscrita ou \
                             decorativa; pior para código (perde maiúsculas).",
                            "Reads the same area again with the neural engine. Better for handwriting or \
                             decorative lettering; worse for code (loses capital letters)."
                        ))
                        .clicked();
                    ui.weak(t!("Você pode corrigir o texto antes de copiar.", "You can fix the text before copying."));
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add(egui::TextEdit::multiline(text).desired_width(f32::INFINITY).desired_rows(10));
                });
            });
        if copy {
            self.status = match copy_text(text) {
                Ok(()) => t!("Texto copiado.", "Text copied.").to_owned(),
                Err(e) => tf!("Não foi possível copiar: {e}", "Could not copy: {e}"),
            };
        }
        if reread && let Some(image) = self.ocr_image.clone() {
            self.start_ocr(ctx, image, ocr::Mode::Handwriting);
        }
        if !open {
            self.ocr_text = None;
            self.ocr_image = None;
        }
    }

    /// Scans `image` locally with OCR to find sensitive data (emails, passwords, tokens, IDs,
    /// credit cards, phones) on a background thread.
    fn start_redact(&mut self, ctx: &egui::Context, image: RgbaImage) {
        if self.redact_running || self.ocr_running {
            return;
        }
        self.redact_running = true;
        self.status = t!(
            "Procurando dados sensíveis na imagem…",
            "Scanning image for sensitive data…"
        )
        .to_owned();
        let (pending, ctx) = (self.redact_pending.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = guarded(|| ocr::find_sensitive_regions(&image));
            *pending.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    /// Applies detected sensitive-data redactions to the active editor when the background OCR
    /// pass completes.
    fn poll_redact(&mut self) {
        let Some(result) = self.redact_pending.lock().unwrap().take() else {
            return;
        };
        self.redact_running = false;
        match result {
            Ok(boxes) if boxes.is_empty() => {
                self.status = t!(
                    "Nenhum dado sensível detectado na imagem.",
                    "No sensitive data detected in the image."
                )
                .to_owned();
            }
            Ok(boxes) => {
                if let Some(ed) = &mut self.editor {
                    let n = ed.apply_redactions(&boxes);
                    self.status = tf!(
                        "{n} dados sensíveis ocultados (use Desfazer ou ajuste as caixas).",
                        "{n} sensitive items redacted (use Undo or adjust the boxes)."
                    );
                }
            }
            Err(e) => {
                self.status = tf!(
                    "Não foi possível analisar dados sensíveis: {e}",
                    "Could not scan for sensitive data: {e}"
                );
            }
        }
    }

    /// Saves `self.editor`'s flattened image, thumbnail, and editable annotation layers (when
    /// annotations exist or previously existed) to the library writer.
    fn save_editor_to_library(&mut self, ctx: &egui::Context, file: &str, is_new: bool) {
        let Some(editor) = &self.editor else { return };
        let image = editor.image().clone();
        let thumb = library::make_thumb(&image);
        let texture = ctx.load_texture(
            "thumbnail",
            editor::texture_image(&thumb, ctx.input(|i| i.max_texture_side)),
            TextureOptions::LINEAR,
        );
        self.thumbs.insert(file.to_owned(), texture);

        let ops = editor.ops();
        let layers_path = self.library.layers_path(file);
        let layers = if !ops.is_empty() || layers_path.exists() {
            match library::Library::save_layers_json(ops) {
                Ok(ops_json) => {
                    let base_path = self.library.base_path(file);
                    let base_image = if is_new || !base_path.exists() {
                        Some(editor.base_image().clone())
                    } else {
                        None
                    };
                    Some(library::LayersJob { base_path, base_image, layers_path, ops_json })
                }
                Err(_) => None,
            }
        } else {
            None
        };
        if let Some(layers) = layers {
            self.writer
                .save_with_layers(self.library.path(file), self.library.thumb_path(file), image, Some(layers));
        } else {
            self.writer.save(&self.library, file, image, thumb);
        }
    }

    /// Saves the editor's current image to the library right away, if it changed since the last
    /// save.
    fn flush_autosave(&mut self, ctx: &egui::Context) {
        let (Some(editor), Some(file)) = (&mut self.editor, &self.current_file) else { return };
        editor.commit_pending_text();
        if editor.revision() == self.saved_revision {
            return;
        }
        let file = file.clone();
        let rev = editor.revision();
        self.library.set_size(&file, editor.image().width(), editor.image().height());
        self.save_editor_to_library(ctx, &file, false);
        self.saved_revision = rev;
    }

    /// Saves the editor's current image to the library, if it changed since the last save. Waits
    /// for the edit to settle so the PNG isn't rewritten on every stroke.
    fn autosave(&mut self, ctx: &egui::Context) {
        let (Some(editor), Some(_)) = (&self.editor, &self.current_file) else { return };
        if editor.revision() == self.saved_revision {
            return;
        }
        let settle = Duration::from_millis(900);
        let idle = editor.idle_time();
        if idle < settle {
            ctx.request_repaint_after(settle - idle);
            return;
        }
        self.flush_autosave(ctx);
    }

    /// Stops any active recording or capture session and finishes writing pending captures before
    /// the program exits.
    fn shutdown(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "macos")]
        {
            self.shut_down = true;
        }
        pin::close_all();
        if let Some(session) = self.video.take().and_then(|v| v.recorder) {
            let _ = session.stop();
        }
        self.pano = None;
        self.steps = None;
        self.selecting = None;
        self.flush_autosave(ctx);
        self.writer.wait_idle(Duration::from_secs(10));
    }

    /// Opens a capture or video kept in the library in the appropriate editor.
    fn open_from_library(&mut self, ctx: &egui::Context, file: &str) {
        self.flush_autosave(ctx);
        let is_video = std::path::Path::new(file)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("mp4"));
        if is_video {
            let path = self.library.path(file);
            if !path.is_file() {
                self.status = tf!(
                    "Não foi possível abrir {file}: arquivo não encontrado.",
                    "Could not open {file}: file not found."
                );
                return;
            }
            let initial_thumb = image::open(self.library.thumb_path(file))
                .map(|i| i.to_rgba8())
                .ok()
                .or_else(|| {
                    let frame = record::extract_frame(&path, 0.0).ok()?;
                    self.library.save_thumb(file, &frame);
                    Some(frame)
                });
            let mut ve = video_editor::VideoEditor::new(ctx, path, initial_thumb, self.accent());
            ve.set_busy(self.video_job_running);
            let info = ve.info();
            if info.width > 0 && info.height > 0 {
                self.library
                    .update_media(file, info.width, info.height, info.duration as f32);
            }
            self.editor = None;
            self.video_editor = Some(ve);
            self.current_file = Some(file.to_owned());
            self.status = tf!("Vídeo aberto: {file}", "Video opened: {file}");
            return;
        }

        let loaded = if let Some((base, ops)) = self.library.load_layers(file) {
            Ok(editor::Editor::from_parts(ctx, base, ops, self.accent()))
        } else {
            image::open(self.library.path(file)).map(|img| editor::Editor::new(ctx, img.to_rgba8(), self.accent()))
        };
        match loaded {
            Ok(editor) => {
                self.saved_revision = editor.revision();
                self.video_editor = None;
                self.editor = Some(editor);
                self.current_file = Some(file.to_owned());
                self.status = tf!("Aberto: {file}", "Opened: {file}");
            }
            Err(e) => self.status = tf!("Não foi possível abrir {file}: {e}", "Could not open {file}: {e}"),
        }
    }

    /// Sends the capture to the Windows Recycle Bin and removes it from the library.
    fn delete_from_library(&mut self, file: &str) {
        match trash::delete(self.library.path(file)) {
            Ok(()) => {
                let _ = std::fs::remove_file(self.library.thumb_path(file));
                self.library.remove_layers(file);
                self.library.forget(file);
                self.thumbs.remove(file);
                self.selected_files.retain(|f| f != file);
                if self.selection_anchor.as_deref() == Some(file) {
                    self.selection_anchor = None;
                }
                if self.edit_meta.as_ref().is_some_and(|(f, _, _)| f == file) {
                    self.edit_meta = None;
                }
                if self.current_file.as_deref() == Some(file) {
                    self.current_file = None;
                    self.editor = None;
                    self.video_editor = None;
                }
                self.status = tf!("{file} foi para a Lixeira.", "{file} was moved to the Recycle Bin.");
            }
            Err(e) => self.status = tf!("Não foi possível apagar {file}: {e}", "Could not delete {file}: {e}"),
        }
    }

    fn delete_batch_from_library(&mut self, files: &[String]) {
        let mut ok = 0usize;
        for file in files {
            if trash::delete(self.library.path(file)).is_ok() {
                let _ = std::fs::remove_file(self.library.thumb_path(file));
                self.library.remove_layers(file);
                self.library.forget(file);
                self.thumbs.remove(file);
                if self.current_file.as_deref() == Some(file.as_str()) {
                    self.current_file = None;
                    self.editor = None;
                    self.video_editor = None;
                }
                ok += 1;
            }
        }
        self.selected_files.retain(|f| !files.contains(f));
        if self
            .selection_anchor
            .as_ref()
            .is_some_and(|a| files.contains(a))
        {
            self.selection_anchor = None;
        }
        if self
            .edit_meta
            .as_ref()
            .is_some_and(|(f, _, _)| files.contains(f))
        {
            self.edit_meta = None;
        }
        if ok > 0 {
            self.status = tf!(
                "{ok} itens foram para a Lixeira.",
                "{ok} items were moved to the Recycle Bin."
            );
        }
    }

    fn export_files_to_folder(&mut self, files: &[String]) {
        if files.is_empty() {
            return;
        }
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let mut copied = 0usize;
        for f in files {
            let src = self.library.path(f);
            let dst = folder.join(f);
            if std::fs::copy(&src, &dst).is_ok() {
                copied += 1;
            }
        }
        self.status = tf!(
            "{copied} arquivos exportados para {}",
            "{copied} files exported to {}",
            folder.display()
        );
    }

    fn copy_library_files_to_clipboard(&mut self, files: &[String]) {
        let paths: Vec<std::path::PathBuf> = files.iter().map(|f| self.library.path(f)).collect();
        match platform::copy_files_to_clipboard(&paths) {
            Ok(()) => {
                self.status = if paths.len() == 1 {
                    t!(
                        "Arquivo copiado para a área de transferência.",
                        "File copied to the clipboard."
                    )
                    .to_owned()
                } else {
                    tf!(
                        "{} arquivos copiados para a área de transferência.",
                        "{} files copied to the clipboard.",
                        paths.len()
                    )
                };
            }
            Err(e) => {
                self.status = tf!(
                    "Não foi possível copiar os arquivos: {e}",
                    "Could not copy the files: {e}"
                );
            }
        }
    }

    fn combine_selected_images(
        &mut self,
        ctx: &egui::Context,
        files: &[String],
        vertical: bool,
    ) {
        let img_paths: Vec<std::path::PathBuf> = files
            .iter()
            .filter(|f| !f.to_ascii_lowercase().ends_with(".mp4"))
            .map(|f| self.library.path(f))
            .collect();
        match library::combine_images(&img_paths, vertical, 12) {
            Ok(combined) => {
                let count = img_paths.len();
                self.selected_files.clear();
                let title = tf!(
                    "{count} imagens combinadas",
                    "{count} combined images"
                );
                self.set_captured(ctx, combined, title);
            }
            Err(e) => {
                self.status = tf!(
                    "Não foi possível juntar as imagens: {e}",
                    "Could not combine the images: {e}"
                );
            }
        }
    }

    fn thumb_texture(&mut self, ctx: &egui::Context, file: &str) -> Option<TextureHandle> {
        if let Some(t) = self.thumbs.get(file) {
            return Some(t.clone());
        }
        let is_video = std::path::Path::new(file)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("mp4"));
        // Missing thumbnail (old library or deleted file): rebuilds it from the original.
        let thumb = if let Ok(img) = image::open(self.library.thumb_path(file)) {
            img.to_rgba8()
        } else if is_video {
            let frame = record::extract_frame(&self.library.path(file), 0.0).ok()?;
            self.library.save_thumb(file, &frame);
            library::make_thumb(&frame)
        } else {
            let img = image::open(self.library.path(file)).ok()?.to_rgba8();
            library::make_thumb(&img)
        };
        let texture = ctx.load_texture("thumbnail", editor::texture_image(&thumb, ctx.input(|i| i.max_texture_side)), TextureOptions::LINEAR);
        self.thumbs.insert(file.to_owned(), texture.clone());
        Some(texture)
    }

    fn library_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let pal = theme::current_palette(&ctx);
        self.selected_files.retain(|f| self.library.get(f).is_some());

        // Search input
        egui::Frame::NONE
            .fill(pal.bg_surface)
            .stroke(Stroke::new(1.0, pal.border_subtle))
            .corner_radius(CornerRadius::same(7))
            .inner_margin(Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (ic_rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 18.0), Sense::hover());
                    icons::paint(ui.painter(), &ctx, Icon::Search, ic_rect.center(), 14.0, pal.text_muted);
                    let clear_w = if self.search.is_empty() { 0.0 } else { 24.0 };
                    let edit_w = (ui.available_width() - clear_w).max(40.0);
                    ui.add_sized(
                        egui::vec2(edit_w, 20.0),
                        egui::TextEdit::singleline(&mut self.search)
                            .frame(egui::Frame::NONE)
                            .margin(Margin::symmetric(0, 2))
                            .hint_text(t!(
                                "Buscar por janela, etiqueta ou data",
                                "Search by window, tag or date"
                            )),
                    );
                    if !self.search.is_empty()
                        && kit::icon_button(ui, Icon::Close, false, 20.0)
                            .on_hover_text(t!("Limpar busca", "Clear search"))
                            .clicked()
                    {
                        self.search.clear();
                    }
                });
            });
        ui.add_space(5.0);

        // Single-row segmented filter bar
        let (total_count, img_count, vid_count) = self.library.counts();
        let fav_count = self.library.favorite_count();
        let filters = [
            (
                library::MediaFilter::All,
                None,
                t!("Todas", "All").to_owned(),
                tf!("Todas ({total_count})", "All ({total_count})"),
            ),
            (
                library::MediaFilter::Favorites,
                Some(Icon::StarFilled),
                format!("{fav_count}"),
                tf!("Favoritas ({fav_count})", "Favorites ({fav_count})"),
            ),
            (
                library::MediaFilter::Images,
                None,
                t!("Img", "Img").to_owned(),
                tf!("Imagens ({img_count})", "Images ({img_count})"),
            ),
            (
                library::MediaFilter::Videos,
                None,
                t!("Víd", "Vid").to_owned(),
                tf!("Vídeos ({vid_count})", "Videos ({vid_count})"),
            ),
        ];
        egui::Frame::NONE
            .fill(pal.bg_surface)
            .stroke(Stroke::new(1.0, pal.border_subtle))
            .corner_radius(CornerRadius::same(7))
            .inner_margin(Margin::same(2))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let seg_w = ((ui.available_width() - 6.0) / 4.0).max(28.0);
                    for (filter, icon, short_label, tip) in filters {
                        let active = self.library_filter == filter;
                        let (seg_rect, resp) =
                            ui.allocate_exact_size(egui::vec2(seg_w, 24.0), Sense::click());
                        if resp.hovered() {
                            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        let bg = if active {
                            pal.accent.gamma_multiply(0.18)
                        } else if resp.hovered() {
                            pal.bg_elevated
                        } else {
                            Color32::TRANSPARENT
                        };
                        let fg = if active {
                            pal.accent
                        } else if resp.hovered() {
                            pal.text_primary
                        } else {
                            pal.text_secondary
                        };
                        let p = ui.painter_at(seg_rect);
                        if bg.a() > 0 {
                            p.rect_filled(seg_rect, CornerRadius::same(5), bg);
                        }
                        if active {
                            p.rect_stroke(
                                seg_rect,
                                CornerRadius::same(5),
                                Stroke::new(1.0, pal.accent.gamma_multiply(0.55)),
                                StrokeKind::Inside,
                            );
                        }
                        let font = if active {
                            egui::FontId::new(11.5, theme::semibold_family())
                        } else {
                            egui::FontId::proportional(11.5)
                        };
                        if let Some(ic) = icon {
                            let g = p.layout_no_wrap(short_label.clone(), font.clone(), fg);
                            let total_w = 12.0 + 3.0 + g.size().x;
                            let start_x = seg_rect.center().x - total_w * 0.5;
                            let ic_color = if active {
                                pal.accent
                            } else {
                                Color32::from_rgb(245, 196, 66)
                            };
                            icons::paint(
                                &p,
                                &ctx,
                                ic,
                                Pos2::new(start_x + 6.0, seg_rect.center().y),
                                11.5,
                                ic_color,
                            );
                            p.galley(
                                Pos2::new(start_x + 15.0, seg_rect.center().y - g.size().y * 0.5),
                                g,
                                fg,
                            );
                        } else {
                            p.text(
                                seg_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                &short_label,
                                font,
                                fg,
                            );
                        }
                        if resp.on_hover_text(tip).clicked() {
                            self.library_filter = filter;
                        }
                    }
                });
            });

        ui.add_space(4.0);
        let entries: Vec<library::Entry> = match self.library_filter {
            library::MediaFilter::All => self.library.search(&self.search),
            filter => self.library.search_filtered(&self.search, filter),
        }
        .into_iter()
        .cloned()
        .collect();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(tf!("{} de {} itens", "{} of {} items", entries.len(), self.library.len()))
                    .size(11.5)
                    .color(pal.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if kit::small_ghost_button(ui, Some(Icon::Folder), t!("Pasta", "Folder"), false)
                    .on_hover_text(t!("Abrir pasta", "Open folder"))
                    .clicked()
                {
                    let _ = std::fs::create_dir_all(self.library.dir());
                    platform::open(self.library.dir());
                }
            });
        });

        let mut batch_delete = false;
        let mut batch_export = false;
        let mut batch_pdf = false;
        let mut batch_copy: Option<Vec<String>> = None;
        let mut batch_combine: Option<bool> = None;

        if !self.selected_files.is_empty() {
            ui.add_space(4.0);
            let selected_img_count = self
                .selected_files
                .iter()
                .filter(|f| !f.to_ascii_lowercase().ends_with(".mp4"))
                .count();
            egui::Frame::NONE
                .fill(pal.bg_surface)
                .stroke(Stroke::new(1.0, pal.accent.gamma_multiply(0.7)))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::same(8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            kit::semibold(tf!(
                                "{} selecionadas",
                                "{} selected",
                                self.selected_files.len()
                            ))
                            .size(12.0)
                            .color(pal.text_primary),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if kit::icon_button(ui, Icon::Close, false, 22.0)
                                .on_hover_text(t!("Limpar seleção", "Clear selection"))
                                .clicked()
                            {
                                self.selected_files.clear();
                            }
                        });
                    });
                    ui.add_space(3.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                        if kit::small_ghost_button(ui, Some(Icon::Trash), t!("Apagar", "Delete"), false)
                            .on_hover_text(t!(
                                "Move todas as selecionadas para a Lixeira",
                                "Moves all selected items to the Recycle Bin"
                            ))
                            .clicked()
                        {
                            batch_delete = true;
                        }
                        if kit::small_ghost_button(ui, Some(Icon::Folder), t!("Exportar…", "Export…"), false)
                            .on_hover_text(t!(
                                "Copia os arquivos selecionados para uma pasta",
                                "Copies the selected files to a folder"
                            ))
                            .clicked()
                        {
                            batch_export = true;
                        }
                        if selected_img_count >= 1
                            && kit::small_ghost_button(ui, Some(Icon::FilePdf), "PDF", false)
                                .on_hover_text(t!(
                                    "Exporta as imagens selecionadas em um único PDF",
                                    "Exports selected images into a single PDF"
                                ))
                                .clicked()
                        {
                            batch_pdf = true;
                        }
                        if kit::small_ghost_button(ui, Some(Icon::Copy), t!("Copiar", "Copy"), false)
                            .on_hover_text(t!("Copiar arquivos", "Copy files"))
                            .clicked()
                        {
                            batch_copy = Some(self.selected_files.clone());
                        }
                        if selected_img_count >= 2 {
                            if kit::small_ghost_button(ui, Some(Icon::CombineV), t!("Juntar V", "Combine V"), false)
                                .on_hover_text(t!("Juntar (Vertical)", "Combine (Vertical)"))
                                .clicked()
                            {
                                batch_combine = Some(true);
                            }
                            if kit::small_ghost_button(ui, Some(Icon::CombineH), t!("Juntar H", "Combine H"), false)
                                .on_hover_text(t!(
                                    "Juntar (Horizontal)",
                                    "Combine (Horizontal)"
                                ))
                                .clicked()
                            {
                                batch_combine = Some(false);
                            }
                        }
                    });
                });
        }

        kit::divider_h(ui);

        let mut to_open = None;
        let mut to_delete = None;
        let mut to_pin: Option<String> = None;
        let mut to_toggle_fav: Option<String> = None;
        let mut to_edit_meta: Option<(String, String, String)> = None;
        let mut to_drag: Option<Vec<std::path::PathBuf>> = None;
        let thumb_h = 98.0;
        let card_gap = 8.0;
        let row_h = thumb_h + 56.0;
        let accent = self.accent();
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, row_h, entries.len(), |ui, rows| {
            for entry in &entries[rows] {
                let selected = self.current_file.as_deref() == Some(entry.file.as_str());
                let multi_selected = self.selected_files.iter().any(|f| f == &entry.file);
                let (outer_rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), row_h),
                    Sense::click_and_drag(),
                );
                let rect = outer_rect.shrink2(egui::vec2(0.0, card_gap * 0.5));
                let hovered = response.hovered();
                let hover_t = ctx.animate_bool_responsive(response.id.with("card_h"), hovered);
                if hovered {
                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                }

                let card_fill = if hover_t > 0.01 {
                    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * hover_t).round() as u8;
                    Color32::from_rgb(
                        mix(pal.bg_surface.r(), pal.bg_elevated.r()),
                        mix(pal.bg_surface.g(), pal.bg_elevated.g()),
                        mix(pal.bg_surface.b(), pal.bg_elevated.b()),
                    )
                } else {
                    pal.bg_surface
                };
                let card_stroke = if selected || multi_selected {
                    Stroke::new(1.5, accent)
                } else if hovered {
                    Stroke::new(1.0, pal.border_strong)
                } else {
                    Stroke::new(1.0, pal.border_subtle)
                };
                ui.painter().rect_filled(rect, CornerRadius::same(8), card_fill);
                ui.painter().rect_stroke(rect, CornerRadius::same(8), card_stroke, StrokeKind::Inside);

                let inner = rect.shrink(6.0);
                let thumb_box = Rect::from_min_size(inner.min, egui::vec2(inner.width(), thumb_h));
                ui.painter().rect_filled(thumb_box, CornerRadius::same(6), pal.bg_canvas);
                let mut thumb_rect = thumb_box;
                if let Some(tex) = self.thumb_texture(&ctx, &entry.file) {
                    let size = tex.size_vec2();
                    let scale = (thumb_box.width() / size.x).min(thumb_box.height() / size.y).min(1.0);
                    let img_rect = Rect::from_center_size(thumb_box.center(), size * scale);
                    thumb_rect = img_rect;
                    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                    ui.painter().image(tex.id(), img_rect, uv, Color32::WHITE);
                }

                let painter = ui.painter_at(inner);
                // Overlay media type badge (MP4 / GIF) and duration pill over thumbnail box
                if entry.is_video() || entry.is_gif() {
                    let badge_text = if entry.is_video() { "MP4" } else { "GIF" };
                    let badge_bg = if entry.is_video() {
                        accent
                    } else {
                        Color32::from_rgb(138, 86, 226)
                    };
                    let badge_fg = if entry.is_video() {
                        theme::text_on(accent)
                    } else {
                        Color32::WHITE
                    };
                    let b_galley = painter.layout_no_wrap(
                        badge_text.to_owned(),
                        egui::FontId::new(10.0, theme::semibold_family()),
                        badge_fg,
                    );
                    let b_pos = thumb_box.min + egui::vec2(5.0, 5.0);
                    let b_rect = Rect::from_min_size(b_pos, b_galley.size() + egui::vec2(8.0, 3.0));
                    painter.rect_filled(b_rect, 4.0, badge_bg);
                    painter.galley(b_pos + egui::vec2(4.0, 1.5), b_galley, badge_fg);

                    if entry.duration_secs > 0.0 {
                        let dur_str = format_duration_short(entry.duration_secs);
                        let d_galley = painter.layout_no_wrap(
                            dur_str,
                            egui::FontId::proportional(10.0),
                            Color32::WHITE,
                        );
                        let d_size = d_galley.size() + egui::vec2(20.0, 4.0);
                        let d_pos = Pos2::new(
                            (thumb_box.max.x - d_size.x - 5.0).max(thumb_box.min.x + 4.0),
                            (thumb_box.max.y - d_size.y - 5.0).max(thumb_box.min.y + 4.0),
                        );
                        let d_rect = Rect::from_min_size(d_pos, d_size);
                        painter.rect_filled(d_rect, 4.0, Color32::from_black_alpha(185));
                        icons::paint(
                            &painter,
                            &ctx,
                            Icon::Play,
                            Pos2::new(d_rect.min.x + 8.0, d_rect.center().y),
                            9.0,
                            Color32::WHITE,
                        );
                        painter.galley(d_pos + egui::vec2(14.0, 2.0), d_galley, Color32::WHITE);
                    }
                }
                let _ = thumb_rect;

                if multi_selected {
                    let check_rect = Rect::from_min_size(
                        Pos2::new(thumb_box.min.x + 5.0, thumb_box.min.y + if entry.is_video() || entry.is_gif() { 26.0 } else { 5.0 }),
                        egui::vec2(18.0, 18.0),
                    );
                    painter.rect_filled(check_rect, 5.0, accent);
                    icons::paint(&painter, &ctx, Icon::Check, check_rect.center(), 12.0, theme::text_on(accent));
                }

                // Quick hover action pill (top-right of thumb_box)
                #[derive(Clone, Copy)]
                enum QuickCardAct {
                    Fav,
                    Pin,
                    Delete,
                }
                let mut action_slots: [(Rect, QuickCardAct); 3] = [(Rect::ZERO, QuickCardAct::Fav); 3];
                let mut action_count = 0usize;
                if hovered {
                    let mut items = Vec::with_capacity(3);
                    items.push((QuickCardAct::Fav, if entry.favorite { Icon::StarFilled } else { Icon::Star }));
                    if !entry.is_video() {
                        items.push((QuickCardAct::Pin, Icon::Pin));
                    }
                    items.push((QuickCardAct::Delete, Icon::Trash));
                    let slot_w = 22.0;
                    let slot_h = 20.0;
                    let pill_w = items.len() as f32 * slot_w + 4.0;
                    let pill_rect = Rect::from_min_size(
                        Pos2::new(thumb_box.max.x - pill_w - 5.0, thumb_box.min.y + 5.0),
                        egui::vec2(pill_w, slot_h + 2.0),
                    );
                    painter.rect_filled(pill_rect, CornerRadius::same(6), Color32::from_black_alpha(190));
                    painter.rect_stroke(
                        pill_rect,
                        CornerRadius::same(6),
                        Stroke::new(1.0, Color32::from_white_alpha(30)),
                        StrokeKind::Inside,
                    );
                    let pointer = ctx.pointer_hover_pos();
                    for (idx, (act, ic)) in items.into_iter().enumerate() {
                        let sr = Rect::from_min_size(
                            Pos2::new(pill_rect.min.x + 2.0 + idx as f32 * slot_w, pill_rect.min.y + 1.0),
                            egui::vec2(slot_w, slot_h),
                        );
                        let over = pointer.is_some_and(|p| sr.contains(p));
                        if over {
                            painter.rect_filled(sr, CornerRadius::same(4), Color32::from_white_alpha(32));
                        }
                        let tint = match act {
                            QuickCardAct::Fav if entry.favorite => Color32::from_rgb(255, 204, 51),
                            QuickCardAct::Delete if over => pal.danger,
                            _ => Color32::WHITE,
                        };
                        icons::paint(&painter, &ctx, ic, sr.center(), 12.0, tint);
                        action_slots[action_count] = (sr, act);
                        action_count += 1;
                    }
                } else if entry.favorite {
                    let fav_rect = Rect::from_min_size(
                        Pos2::new(thumb_box.max.x - 23.0, thumb_box.min.y + 5.0),
                        egui::vec2(18.0, 18.0),
                    );
                    painter.rect_filled(fav_rect, 5.0, Color32::from_black_alpha(175));
                    icons::paint(
                        &painter,
                        &ctx,
                        Icon::StarFilled,
                        fav_rect.center(),
                        11.5,
                        Color32::from_rgb(255, 204, 51),
                    );
                }

                let text_rect = Rect::from_min_max(
                    Pos2::new(inner.min.x, thumb_box.max.y + 5.0),
                    inner.max,
                );
                let text_painter = ui.painter_at(text_rect);
                let title = if entry.title.is_empty() { t!("(sem título)", "(untitled)") } else { &entry.title };
                text_painter.text(
                    text_rect.min,
                    egui::Align2::LEFT_TOP,
                    title,
                    egui::FontId::new(12.5, theme::semibold_family()),
                    pal.text_primary,
                );
                let mut detail = if entry.duration_secs > 0.0 && entry.width > 0 && entry.height > 0 {
                    format!(
                        "{}  •  {}×{}  •  {}",
                        entry.when,
                        entry.width,
                        entry.height,
                        format_duration_short(entry.duration_secs)
                    )
                } else if entry.width > 0 && entry.height > 0 {
                    format!("{}  •  {}×{}", entry.when, entry.width, entry.height)
                } else {
                    entry.when.clone()
                };
                if !entry.tags.is_empty() {
                    let tags_str = entry
                        .tags
                        .iter()
                        .map(|tag| format!("#{}", tag.trim_start_matches('#')))
                        .collect::<Vec<_>>()
                        .join(" ");
                    detail.push_str("  •  ");
                    detail.push_str(&tags_str);
                }
                text_painter.text(
                    text_rect.min + egui::vec2(0.0, 17.0),
                    egui::Align2::LEFT_TOP,
                    detail,
                    egui::FontId::proportional(11.0),
                    pal.text_muted,
                );

                if response.clicked() {
                    let click_pos = response
                        .interact_pointer_pos()
                        .or_else(|| ctx.pointer_interact_pos());
                    let quick_hit = click_pos.and_then(|p| {
                        action_slots[..action_count]
                            .iter()
                            .find(|(sr, _)| sr.contains(p))
                            .map(|(_, act)| *act)
                    });
                    if let Some(act) = quick_hit {
                        match act {
                            QuickCardAct::Fav => to_toggle_fav = Some(entry.file.clone()),
                            QuickCardAct::Pin => to_pin = Some(entry.file.clone()),
                            QuickCardAct::Delete => to_delete = Some(entry.file.clone()),
                        }
                    } else {
                        let (ctrl, shift) =
                            ctx.input(|i| (i.modifiers.command || i.modifiers.ctrl, i.modifiers.shift));
                        if shift {
                            let anchor = self
                                .selection_anchor
                                .clone()
                                .or_else(|| self.current_file.clone());
                            if let Some(anchor_file) = anchor
                                && let Some(a_idx) = entries.iter().position(|e| e.file == anchor_file)
                                && let Some(b_idx) = entries.iter().position(|e| e.file == entry.file)
                            {
                                let (lo, hi) = (a_idx.min(b_idx), a_idx.max(b_idx));
                                self.selected_files =
                                    entries[lo..=hi].iter().map(|e| e.file.clone()).collect();
                            } else {
                                self.selected_files = vec![entry.file.clone()];
                                self.selection_anchor = Some(entry.file.clone());
                            }
                        } else if ctrl {
                            if self.selected_files.is_empty()
                                && let Some(cur) = &self.current_file
                                && cur != &entry.file
                            {
                                self.selected_files.push(cur.clone());
                            }
                            if let Some(pos) = self.selected_files.iter().position(|f| f == &entry.file)
                            {
                                self.selected_files.remove(pos);
                            } else {
                                self.selected_files.push(entry.file.clone());
                            }
                            self.selection_anchor = Some(entry.file.clone());
                        } else {
                            self.selected_files.clear();
                            self.selection_anchor = Some(entry.file.clone());
                            to_open = Some(entry.file.clone());
                        }
                    }
                }
                if response.drag_started() {
                    let paths = if self.selected_files.contains(&entry.file)
                        && !self.selected_files.is_empty()
                    {
                        self.selected_files
                            .iter()
                            .map(|f| self.library.path(f))
                            .collect()
                    } else {
                        vec![self.library.path(&entry.file)]
                    };
                    to_drag = Some(paths);
                }
                response.context_menu(|ui| {
                    if ui.button(t!("Abrir", "Open")).clicked() {
                        to_open = Some(entry.file.clone());
                        ui.close();
                    }
                    if !entry.is_video()
                        && ui
                            .button(t!("Fixar na tela", "Pin to screen"))
                            .clicked()
                    {
                        to_pin = Some(entry.file.clone());
                        ui.close();
                    }
                    if entry.is_video()
                        && ui.button(t!("Abrir no player", "Open in player")).clicked()
                    {
                        platform::open(self.library.path(&entry.file));
                        ui.close();
                    }
                    let fav_label = if entry.favorite {
                        t!("Remover dos favoritos", "Remove from favorites")
                    } else {
                        t!("Favoritar", "Favorite")
                    };
                    if ui.button(fav_label).clicked() {
                        to_toggle_fav = Some(entry.file.clone());
                        ui.close();
                    }
                    if ui
                        .button(t!("Renomear / etiquetas…", "Rename / tags…"))
                        .clicked()
                    {
                        to_edit_meta = Some((
                            entry.file.clone(),
                            entry.title.clone(),
                            entry.tags.join(", "),
                        ));
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .button(t!("Copiar arquivo", "Copy file"))
                        .clicked()
                    {
                        batch_copy = Some(vec![entry.file.clone()]);
                        ui.close();
                    }
                    if ui.button(t!("Mostrar na pasta", "Show in folder")).clicked() {
                        platform::reveal(&self.library.path(&entry.file));
                        ui.close();
                    }
                    if ui.button(t!("Mover para a Lixeira", "Move to Recycle Bin")).clicked() {
                        to_delete = Some(entry.file.clone());
                        ui.close();
                    }
                    if multi_selected && self.selected_files.len() >= 2 {
                        ui.separator();
                        ui.weak(tf!(
                            "{} selecionadas",
                            "{} selected",
                            self.selected_files.len()
                        ));
                        if ui
                            .button(t!("Copiar arquivos", "Copy files"))
                            .clicked()
                        {
                            batch_copy = Some(self.selected_files.clone());
                            ui.close();
                        }
                        if ui
                            .button(t!("Exportar…", "Export…"))
                            .clicked()
                        {
                            batch_export = true;
                            ui.close();
                        }
                        let img_count = self
                            .selected_files
                            .iter()
                            .filter(|f| !f.to_ascii_lowercase().ends_with(".mp4"))
                            .count();
                        if img_count >= 1
                            && ui
                                .button(t!("Exportar PDF…", "Export PDF…"))
                                .clicked()
                        {
                            batch_pdf = true;
                            ui.close();
                        }
                        if img_count >= 2 {
                            if ui
                                .button(t!("Juntar (Vertical)", "Combine (Vertical)"))
                                .clicked()
                            {
                                batch_combine = Some(true);
                                ui.close();
                            }
                            if ui
                                .button(t!("Juntar (Horizontal)", "Combine (Horizontal)"))
                                .clicked()
                            {
                                batch_combine = Some(false);
                                ui.close();
                            }
                        }
                        if ui
                            .button(t!("Apagar selecionadas", "Delete selected"))
                            .clicked()
                        {
                            batch_delete = true;
                            ui.close();
                        }
                    }
                });
            }
        });
        if let Some(drag_paths) = to_drag {
            let _ = platform::start_file_drag(&drag_paths);
        }
        // Delete removes the selected capture, with the mouse over the library. It doesn't apply
        // while typing in the search, nor when the key is meant for an annotation selected in the
        // editor.
        let editing = self.editor.as_ref().is_some_and(|e| e.has_selection() || e.is_typing());
        let delete_key = ui.rect_contains_pointer(ui.max_rect())
            && !editing
            && !ctx.egui_wants_keyboard_input()
            && ctx.input(|i| i.key_pressed(egui::Key::Delete));
        let mut open_after = None;
        if delete_key {
            if !self.selected_files.is_empty() {
                batch_delete = true;
            } else {
                let files: Vec<&str> = entries.iter().map(|e| e.file.as_str()).collect();
                if let Some(file) = self.current_file.clone() {
                    open_after = library::neighbor(&files, &file).map(str::to_owned);
                    to_delete = Some(file);
                }
            }
        }
        if batch_delete {
            let files = self.selected_files.clone();
            self.delete_batch_from_library(&files);
        }
        if batch_export {
            let files = self.selected_files.clone();
            self.export_files_to_folder(&files);
        }
        if batch_pdf {
            let img_paths: Vec<std::path::PathBuf> = self
                .selected_files
                .iter()
                .filter(|f| !f.to_ascii_lowercase().ends_with(".mp4"))
                .map(|f| self.library.path(f))
                .collect();
            self.status = export::export_files_as_pdf_dialog(&img_paths);
        }
        if let Some(files) = batch_copy {
            self.copy_library_files_to_clipboard(&files);
        }
        if let Some(vertical) = batch_combine {
            let files = self.selected_files.clone();
            self.combine_selected_images(&ctx, &files, vertical);
        }
        if let Some(file) = to_pin {
            self.flush_autosave(&ctx);
            let acc = self.accent();
            let img_res = if self.current_file.as_deref() == Some(file.as_str())
                && let Some(ed) = &self.editor
            {
                Ok(ed.image().clone())
            } else {
                image::open(self.library.path(&file))
                    .map(|i| i.to_rgba8())
                    .map_err(|e| e.to_string())
            };
            self.status =
                match img_res.and_then(|img| pin::spawn(img, [acc.r(), acc.g(), acc.b()], None)) {
                    Ok(()) => t!(
                        "Captura fixada na tela (arraste para mover, Esc fecha).",
                        "Capture pinned to screen (drag to move, Esc closes)."
                    )
                    .to_owned(),
                    Err(e) => tf!(
                        "Não foi possível fixar {file}: {e}",
                        "Could not pin {file}: {e}"
                    ),
                };
        }
        if let Some(file) = to_toggle_fav
            && let Some(now_fav) = self.library.toggle_favorite(&file)
        {
            self.status = if now_fav {
                tf!(
                    "★ {file} adicionado aos favoritos.",
                    "★ {file} added to favorites."
                )
            } else {
                tf!(
                    "{file} removido dos favoritos.",
                    "{file} removed from favorites."
                )
            };
        }
        if let Some(meta) = to_edit_meta {
            self.edit_meta = Some(meta);
        }
        if let Some(file) = to_open {
            self.open_from_library(&ctx, &file);
        }
        if let Some(file) = to_delete {
            self.delete_from_library(&file);
            // With the key, the selection moves to the neighboring capture: you can delete several
            // in a row.
            if let Some(next) = open_after.filter(|_| self.current_file.is_none()) {
                let status = std::mem::take(&mut self.status);
                self.open_from_library(&ctx, &next);
                self.status = status;
            }
        }
    }

    fn edit_meta_ui(&mut self, ctx: &egui::Context) {
        let Some((file, mut title_draft, mut tags_draft)) = self.edit_meta.clone() else {
            return;
        };
        let mut open = true;
        let mut save = false;
        let mut cancel = false;
        egui::Window::new(t!("Renomear e etiquetas", "Rename & tags"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.weak(&file);
                ui.add_space(4.0);
                ui.label(t!("Título", "Title"));
                let title_resp = ui.add(
                    egui::TextEdit::singleline(&mut title_draft)
                        .hint_text(t!("Título da captura ou vídeo", "Capture or video title"))
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(6.0);
                ui.label(t!(
                    "Etiquetas (separadas por vírgula)",
                    "Tags (comma-separated)"
                ));
                let tags_resp = ui.add(
                    egui::TextEdit::singleline(&mut tags_draft)
                        .hint_text(t!("ex: bug, login, sprint-4", "e.g. bug, login, sprint-4"))
                        .desired_width(f32::INFINITY),
                );
                let enter_pressed = (title_resp.lost_focus() || tags_resp.lost_focus())
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    || (!ctx.egui_wants_keyboard_input()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                if enter_pressed {
                    save = true;
                }
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                    cancel = true;
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if kit::primary_button(ui, Some(Icon::Check), t!("Salvar", "Save")).clicked() {
                        save = true;
                    }
                    if kit::ghost_button(ui, None, t!("Cancelar", "Cancel"), false).clicked() {
                        cancel = true;
                    }
                });
            });

        if save {
            let parsed_tags: Vec<String> = tags_draft
                .split(',')
                .map(|s| s.trim().trim_start_matches('#').trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            let clean_title = title_draft.trim().to_owned();
            if self.library.set_metadata(&file, clean_title, parsed_tags) {
                self.status = tf!("Detalhes atualizados: {file}", "Details updated: {file}");
            }
            self.edit_meta = None;
        } else if cancel || !open {
            self.edit_meta = None;
        } else {
            self.edit_meta = Some((file, title_draft, tags_draft));
        }
    }

    fn set_captured(&mut self, ctx: &egui::Context, image: RgbaImage, title: String) {
        self.adopt_editor(ctx, editor::Editor::new(ctx, image, self.accent()), title);
    }

    /// Saves the editor's image to the library, copies it and shows it in the main editor.
    fn adopt_editor(&mut self, ctx: &egui::Context, mut editor: editor::Editor, title: String) {
        self.flush_autosave(ctx);
        editor.commit_pending_text();
        let image = editor.image().clone();
        let file = self.library.add(platform::local_time(), title, image.width(), image.height());
        self.current_file = Some(file.clone());
        // The image saved below already includes what has been annotated so far.
        self.saved_revision = editor.revision();
        self.video_editor = None;
        self.editor = Some(editor);
        self.save_editor_to_library(ctx, &file, true);
        self.status = match copy_to_clipboard(&image) {
            Ok(()) => tf!(
                "Captura de {}×{} px copiada para a área de transferência.",
                "{}×{} px capture copied to the clipboard.",
                image.width(),
                image.height()
            ),
            Err(e) => tf!("Captura feita, mas não foi possível copiar: {e}", "Captured, but could not copy: {e}"),
        };
    }

    /// Registers a newly recorded or exported MP4 inside the library folder, generates its
    /// thumbnail, and opens it directly in the integrated `VideoEditor`.
    fn adopt_video(&mut self, ctx: &egui::Context, path: std::path::PathBuf, title: String) {
        self.flush_autosave(ctx);
        let Some(file) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            return;
        };
        let info = record::probe(&path);
        let thumb = record::extract_frame(&path, 0.0).ok();
        let (width, height, duration_secs) = match (&info, &thumb) {
            (Some(i), _) => (i.width, i.height, i.duration as f32),
            (None, Some(img)) => (img.width(), img.height(), 0.0),
            (None, None) => (0, 0, 0.0),
        };
        if let Some(ref img) = thumb {
            self.library.save_thumb(&file, img);
            let small = library::make_thumb(img);
            let texture = ctx.load_texture(
                "thumbnail",
                editor::texture_image(&small, ctx.input(|i| i.max_texture_side)),
                TextureOptions::LINEAR,
            );
            self.thumbs.insert(file.clone(), texture);
        }
        self.library.register_existing_file(
            file.clone(),
            platform::local_time(),
            title,
            width,
            height,
            duration_secs,
        );
        self.current_file = Some(file.clone());
        self.editor = None;
        let mut ve = video_editor::VideoEditor::new(ctx, path, thumb, self.accent());
        ve.set_busy(self.video_job_running);
        self.video_editor = Some(ve);
        self.status = tf!(
            "Vídeo salvo na biblioteca ({file}, {:.1} s).",
            "Video saved to the library ({file}, {:.1} s).",
            duration_secs
        );
    }

    /// Registers a newly created GIF inside the library folder, saves its thumbnail, and opens it.
    fn adopt_gif(
        &mut self,
        ctx: &egui::Context,
        file: String,
        path: std::path::PathBuf,
        title: String,
    ) {
        self.flush_autosave(ctx);
        let duration_secs = record::video_duration(&path).map_or(0.0, |d| d.as_secs_f32());
        let first_frame = image::open(&path)
            .map(|i| i.to_rgba8())
            .ok()
            .or_else(|| record::extract_frame(&path, 0.0).ok());
        let (width, height) = first_frame
            .as_ref()
            .map(|img| (img.width(), img.height()))
            .unwrap_or((0, 0));
        if let Some(ref img) = first_frame {
            self.library.save_thumb(&file, img);
            let small = library::make_thumb(img);
            let texture = ctx.load_texture(
                "thumbnail",
                editor::texture_image(&small, ctx.input(|i| i.max_texture_side)),
                TextureOptions::LINEAR,
            );
            self.thumbs.insert(file.clone(), texture);
        }
        self.library.register_existing_file(
            file.clone(),
            platform::local_time(),
            title,
            width,
            height,
            duration_secs,
        );
        if let Some(img) = first_frame {
            let ed = editor::Editor::new(ctx, img, self.accent());
            self.saved_revision = ed.revision();
            self.video_editor = None;
            self.editor = Some(ed);
            self.current_file = Some(file.clone());
        }
        self.status = tf!(
            "GIF salvo na biblioteca ({file}).",
            "GIF saved to the library ({file})."
        );
    }

    /// Hides the window and scrolls the chosen area in the background; the result arrives through
    /// the same queue as the other captures.
    fn start_scroll(
        &mut self,
        ctx: &egui::Context,
        rect: platform::PxRect,
        direction: scroll::Direction,
        title: String,
    ) {
        // macOS: the scrolling is done with synthesized wheel events, which need the Accessibility
        // permission. Without it nothing would scroll and the capture would end after one shot.
        if !platform::ensure_input_permission() {
            self.settle_window(false);
            self.status = t!(
                "A captura com rolagem precisa controlar a rolagem: permita o SnapKit em Ajustes do Sistema > Privacidade e Segurança > Acessibilidade e tente de novo.",
                "The scrolling capture needs to control scrolling: allow SnapKit in System Settings > Privacy & Security > Accessibility and try again."
            )
            .to_owned();
            return;
        }
        if self.shared.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        self.status = if platform::key_permission() {
            t!("Capturando com rolagem… Esc interrompe.", "Scrolling capture… Esc stops.")
        } else {
            // macOS without Input Monitoring: Esc can't be read, the capture runs to the end.
            t!(
                "Capturando com rolagem… Para o Esc interromper, permita o SnapKit em Ajustes do Sistema > Privacidade e Segurança > Monitoramento de Entrada.",
                "Scrolling capture… For Esc to stop it, allow SnapKit in System Settings > Privacy & Security > Input Monitoring."
            )
        }
        .to_owned();
        let (shared, ctx) = (self.shared.clone(), ctx.clone());
        ctx.request_repaint();
        std::thread::spawn(move || {
            // Gives the window time to leave full screen before minimizing.
            std::thread::sleep(Duration::from_millis(250));
            if shared.hwnd != 0 {
                platform::minimize(shared.hwnd);
            }
            std::thread::sleep(Duration::from_millis(450));
            // Frame around the area while the scrolling happens.
            let frame = platform::RegionFrame::show(rect);
            let result = guarded(|| scroll::run(rect, direction));
            drop(frame);
            let result = result.map(|(image, note)| capture::Shot {
                image,
                windows: Vec::new(),
                title,
                note,
            });
            // Delivered as "full screen": the image is already finished, with no selection step.
            *shared.pending.lock().unwrap() = Some((CaptureKind::FullScreen, result));
            if shared.hwnd != 0 {
                platform::show(shared.hwnd);
            }
            ctx.request_repaint();
        });
    }

    /// Turns the window into a small, always visible panel to control the panoramic capture.
    fn start_panoramic(&mut self, rect: platform::PxRect, title: String) {
        self.shared.busy.store(true, Ordering::SeqCst);
        self.pano = Some(PanoUi {
            rect,
            title,
            session: None,
            _frame: platform::RegionFrame::show(rect),
            setup_frames: 6,
        });
    }

    /// Turns the window into a small panel to control the video recording.
    fn start_video(&mut self, rect: platform::PxRect, as_gif: bool, title: String) {
        self.shared.busy.store(true, Ordering::SeqCst);
        let frame = platform::RegionFrame::show(rect);
        self.video = Some(VideoUi {
            rect,
            title,
            recorder: None,
            as_gif,
            countdown_until: None,
            halo: None,
            _frame: frame,
            setup_frames: 6,
        });
    }

    fn video_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let hwnd = self.shared.hwnd;
        let Some(video) = &mut self.video else { return };
        ctx.request_repaint_after(Duration::from_millis(100));
        hud_setup(&ctx, hwnd, video.rect, VIDEO_HUD_SIZE, &mut video.setup_frames);

        // FFmpeg only exits on its own when something went wrong.
        let crashed = video.recorder.as_mut().is_some_and(|r| r.has_exited());
        let (mut arm_record, mut start_now, mut stop, mut cancel, mut toggle_pause) =
            (false, false, crashed, false, false);
        let (mut system_audio, mut microphone) =
            (self.cfg.record_system_audio, self.cfg.record_microphone);
        let (mut fps, mut quality, mut cursor, mut countdown) = (
            self.cfg.video_fps,
            self.cfg.video_quality,
            self.cfg.video_cursor,
            self.cfg.video_countdown,
        );
        let mut highlight_clicks = self.cfg.video_highlight_clicks;
        let as_gif = video.as_gif;

        if video.recorder.is_none()
            && let Some(deadline) = video.countdown_until
            && Instant::now() >= deadline
        {
            video.countdown_until = None;
            start_now = true;
        }
        let countdown_deadline = video.countdown_until;

        let pal = theme::current_palette(ui.ctx());
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(pal.bg_chrome).inner_margin(Margin::symmetric(10, 6)))
            .show(ui, |ui| {
            ui.horizontal_centered(|ui| match &video.recorder {
                None => {
                    if let Some(deadline) = countdown_deadline {
                        let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::hover());
                        icons::paint(ui.painter(), ui.ctx(), Icon::RecordDot, dot_rect.center(), 13.0, pal.danger);
                        let rem = deadline
                            .saturating_duration_since(Instant::now())
                            .as_secs_f32()
                            .ceil()
                            .max(1.0) as u32;
                        ui.strong(tf!(
                            "Gravando em {rem} s…",
                            "Recording in {rem} s…"
                        ));
                        kit::divider_v(ui, 18.0);
                        start_now |= kit::primary_button(ui, Some(Icon::Play), t!("Começar agora", "Start now"))
                            .clicked();
                        cancel = kit::ghost_button(ui, None, t!("Cancelar", "Cancel"), false).clicked();
                    } else {
                        arm_record = kit::primary_button(
                            ui,
                            Some(Icon::RecordDot),
                            if as_gif {
                                t!("Gravar GIF", "Record GIF")
                            } else {
                                t!("Gravar", "Record")
                            },
                        )
                        .clicked();
                        cancel = kit::ghost_button(ui, None, t!("Cancelar", "Cancel"), false).clicked();
                        kit::divider_v(ui, 18.0);

                        // FPS selector
                        egui::ComboBox::from_id_salt("hud_video_fps")
                            .width(62.0)
                            .selected_text(format!("{fps} fps"))
                            .show_ui(ui, |ui| {
                                for f in [15u32, 24, 30, 60] {
                                    ui.selectable_value(&mut fps, f, format!("{f} fps"));
                                }
                            });

                        // Quality selector
                        let q_label = |q: config::VideoQuality| match q {
                            config::VideoQuality::Standard => t!("Normal", "Standard"),
                            config::VideoQuality::High => t!("Alta", "High"),
                            config::VideoQuality::Ultra => t!("Ultra", "Ultra"),
                        };
                        egui::ComboBox::from_id_salt("hud_video_quality")
                            .width(66.0)
                            .selected_text(q_label(quality))
                            .show_ui(ui, |ui| {
                                for q in [
                                    config::VideoQuality::Standard,
                                    config::VideoQuality::High,
                                    config::VideoQuality::Ultra,
                                ] {
                                    ui.selectable_value(&mut quality, q, q_label(q));
                                }
                            });

                        // Countdown selector
                        let cd_label = |c: u32| {
                            if c == 0 {
                                t!("0s", "0s").to_owned()
                            } else {
                                format!("{c}s")
                            }
                        };
                        egui::ComboBox::from_id_salt("hud_video_countdown")
                            .width(52.0)
                            .selected_text(cd_label(countdown))
                            .show_ui(ui, |ui| {
                                for c in [0u32, 3, 5] {
                                    ui.selectable_value(
                                        &mut countdown,
                                        c,
                                        if c == 0 {
                                            t!("Sem contagem (0s)", "No countdown (0s)").to_owned()
                                        } else {
                                            tf!("Contagem {c}s", "Countdown {c}s")
                                        },
                                    );
                                }
                            });

                        ui.checkbox(&mut cursor, t!("Cursor", "Cursor"));
                        ui.checkbox(&mut highlight_clicks, t!("Cliques", "Clicks"))
                            .on_hover_text(t!(
                                "Destaca cliques do mouse na gravação",
                                "Highlights mouse clicks in the recording"
                            ));
                        if !as_gif {
                            ui.checkbox(&mut system_audio, t!("Som do sistema", "System sound"));
                            ui.checkbox(&mut microphone, t!("Microfone", "Microphone"));
                        }
                    }
                }
                Some(session) => {
                    stop |= kit::primary_button(ui, Some(Icon::Stop), t!("Parar", "Stop")).clicked();
                    let paused = session.is_paused();
                    toggle_pause = kit::ghost_button(
                        ui,
                        Some(if paused { Icon::Play } else { Icon::Pause }),
                        if paused {
                            t!("Continuar", "Resume")
                        } else {
                            t!("Pausar", "Pause")
                        },
                        paused,
                    )
                    .clicked();
                    let secs = session.elapsed().as_secs();
                    if paused {
                        ui.label(tf!(
                            "Em pausa  {:02}:{:02}",
                            "Paused  {:02}:{:02}",
                            secs / 60,
                            secs % 60
                        ));
                    } else {
                        let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::hover());
                        icons::paint(ui.painter(), ui.ctx(), Icon::RecordDot, dot_rect.center(), 13.0, pal.danger);
                        ui.label(tf!(
                            "Gravando  {:02}:{:02}",
                            "Recording  {:02}:{:02}",
                            secs / 60,
                            secs % 60
                        ));
                    }
                    if let Some(warning) = session.warnings.first() {
                        ui.colored_label(Color32::from_rgb(255, 200, 80), warning);
                    }
                }
            });
        });

        let (rect, idle) = (video.rect, video.recorder.is_none());
        let prefs_changed = (!as_gif
            && (system_audio, microphone)
                != (self.cfg.record_system_audio, self.cfg.record_microphone))
            || (fps, quality, cursor, countdown, highlight_clicks)
                != (
                    self.cfg.video_fps,
                    self.cfg.video_quality,
                    self.cfg.video_cursor,
                    self.cfg.video_countdown,
                    self.cfg.video_highlight_clicks,
                );
        if prefs_changed {
            if !as_gif {
                self.cfg.record_system_audio = system_audio;
                self.cfg.record_microphone = microphone;
            }
            self.cfg.video_fps = fps;
            self.cfg.video_quality = quality;
            self.cfg.video_cursor = cursor;
            self.cfg.video_countdown = countdown;
            self.cfg.video_highlight_clicks = highlight_clicks;
            self.save_cfg();
        }

        if arm_record {
            if countdown > 0 {
                if let Some(video) = &mut self.video {
                    video.countdown_until =
                        Some(Instant::now() + Duration::from_secs(countdown as u64));
                }
            } else {
                start_now = true;
            }
        }

        if start_now {
            let now = platform::local_time();
            let file = self.library.reserve_name(now, "mp4");
            let stem = file.strip_suffix(".mp4").unwrap_or(&file);
            let options = record::Options {
                system_audio: !as_gif && system_audio,
                microphone: !as_gif && microphone,
                fps: self.cfg.video_fps,
                quality: self.cfg.video_quality,
                draw_mouse: self.cfg.video_cursor,
                highlight_clicks: self.cfg.video_highlight_clicks,
            };
            match record::Session::start(rect, options, self.library.dir(), stem) {
                Ok(session) => {
                    let show_halo = self.cfg.video_highlight_clicks;
                    if let Some(video) = &mut self.video {
                        video.countdown_until = None;
                        video.recorder = Some(session);
                        video.halo = show_halo.then(|| platform::ClickHalo::show(rect, hwnd));
                    }
                }
                Err(e) => {
                    self.leave_pano_hud(&ctx);
                    self.status = tf!("Não foi possível gravar: {e}", "Could not record: {e}");
                }
            }
        } else if stop {
            let Some(VideoUi {
                title,
                recorder,
                as_gif,
                ..
            }) = self.video.take()
            else {
                return;
            };
            self.leave_pano_hud(&ctx);
            match recorder.map(record::Session::stop) {
                Some(Ok(path)) if as_gif => {
                    // GIF mode: converts directly into the library and removes the intermediate MP4.
                    self.start_video_job(&ctx, t!("Criando o GIF", "Creating the GIF"), move || {
                        let res = record::to_gif(&path, &record::GifOptions::default());
                        let _ = std::fs::remove_file(&path);
                        res.map(|p| {
                            let gif_file = p
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("captura.gif")
                                .to_owned();
                            VideoJobDone::SavedGif {
                                file: gif_file,
                                title,
                                path: p,
                            }
                        })
                    });
                }
                Some(Ok(path)) => {
                    self.adopt_video(&ctx, path, title);
                }
                Some(Err(e)) => {
                    self.status = tf!("A gravação falhou: {e}", "The recording failed: {e}");
                }
                None => {}
            }
        } else if toggle_pause
            && let Some(video) = &mut self.video
            && let Some(session) = &mut video.recorder
        {
            let was_paused = session.is_paused();
            let result = if was_paused {
                session.resume()
            } else {
                session.pause()
            };
            match result {
                Ok(()) => {
                    if was_paused {
                        if self.cfg.video_highlight_clicks {
                            video.halo = Some(platform::ClickHalo::show(rect, hwnd));
                        }
                    } else {
                        video.halo = None;
                    }
                }
                Err(e) => {
                    self.status = tf!(
                        "Não foi possível pausar ou continuar: {e}",
                        "Could not pause or resume: {e}"
                    );
                }
            }
        } else if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) && idle {
            self.leave_pano_hud(&ctx);
            self.status = t!("Gravação cancelada.", "Recording canceled.").to_owned();
        }
    }

    /// Runs a background video/GIF export or edit job and updates `self.video_editor` busy state.
    fn start_video_job(
        &mut self,
        ctx: &egui::Context,
        label: &str,
        job: impl FnOnce() -> Result<VideoJobDone, String> + Send + 'static,
    ) {
        if self.video_job_running {
            return;
        }
        self.video_job_running = true;
        if let Some(ve) = &mut self.video_editor {
            ve.set_busy(true);
        }
        self.status = format!("{label}…");
        let (pending, ctx) = (self.video_job_pending.clone(), ctx.clone());
        std::thread::spawn(move || {
            *pending.lock().unwrap() = Some(guarded(job));
            ctx.request_repaint();
        });
    }

    /// Polls finished background video/GIF jobs and updates the library and active editor.
    fn poll_video_jobs(&mut self, ctx: &egui::Context) {
        let Some(result) = self.video_job_pending.lock().unwrap().take() else {
            return;
        };
        self.video_job_running = false;
        if let Some(ve) = &mut self.video_editor {
            ve.set_busy(false);
        }
        match result {
            Ok(VideoJobDone::SavedClip { title, path }) => {
                self.adopt_video(ctx, path, title);
            }
            Ok(VideoJobDone::Overwritten { file, path }) => {
                let info = record::probe(&path);
                let thumb = record::extract_frame(&path, 0.0).ok();
                if let Some(ref img) = thumb {
                    self.library.save_thumb(&file, img);
                    let small = library::make_thumb(img);
                    let texture = ctx.load_texture(
                        "thumbnail",
                        editor::texture_image(&small, ctx.input(|i| i.max_texture_side)),
                        TextureOptions::LINEAR,
                    );
                    self.thumbs.insert(file.clone(), texture);
                }
                if let Some(i) = info {
                    self.library
                        .update_media(&file, i.width, i.height, i.duration as f32);
                }
                if let Some(ve) = &mut self.video_editor {
                    ve.reload_after_overwrite(ctx);
                }
                self.status = tf!(
                    "Edições aplicadas em {file}.",
                    "Edits applied to {file}."
                );
            }
            Ok(VideoJobDone::SavedGif { file, title, path }) => {
                self.adopt_gif(ctx, file, path, title);
            }
            Ok(VideoJobDone::ExportedExternal { path }) => {
                platform::reveal(&path);
                self.status = tf!(
                    "Vídeo exportado em {}",
                    "Video exported to {}",
                    path.display()
                );
            }
            Err(e) => {
                self.status = tf!(
                    "Não foi possível processar o vídeo: {e}",
                    "Could not process the video: {e}"
                );
            }
        }
    }

    /// Handles an action triggered from the integrated `VideoEditor`.
    fn handle_video_action(&mut self, ctx: &egui::Context, action: video_editor::VideoAction) {
        let base_title = self
            .current_file
            .as_deref()
            .and_then(|f| self.library.get(f))
            .map(|e| e.title.clone())
            .unwrap_or_default();

        match action {
            video_editor::VideoAction::CaptureFrame { image } => {
                let title = if base_title.is_empty() {
                    t!("Quadro de vídeo", "Video frame").to_owned()
                } else {
                    base_title
                };
                self.set_captured(ctx, image, title);
            }
            video_editor::VideoAction::SaveClipToLibrary { opts } => {
                let Some(ve) = &self.video_editor else { return };
                let src = ve.path().to_path_buf();
                let file = self.library.reserve_name(platform::local_time(), "mp4");
                let out = self.library.path(&file);
                let title = if base_title.is_empty() {
                    t!("Trecho de vídeo", "Video clip").to_owned()
                } else {
                    base_title
                };
                self.start_video_job(
                    ctx,
                    t!("Salvando trecho na biblioteca", "Saving clip to library"),
                    move || {
                        let pure_trim = (opts.speed - 1.0).abs() < 0.01
                            && !opts.mute
                            && opts.crop.is_none()
                            && opts.max_width == 0
                            && opts.end.is_some();
                        let res = if pure_trim {
                            record::trim(&src, opts.start, opts.end.unwrap_or(0.0)).and_then(
                                |clip_path| {
                                    std::fs::rename(&clip_path, &out)
                                        .map(|()| out)
                                        .map_err(|e| e.to_string())
                                },
                            )
                        } else {
                            record::edit_video(&src, &opts, &out)
                        };
                        res.map(|p| VideoJobDone::SavedClip { title, path: p })
                    },
                );
            }
            video_editor::VideoAction::OverwriteInLibrary { opts } => {
                let Some(ve) = &self.video_editor else { return };
                let src = ve.path().to_path_buf();
                let file = self
                    .current_file
                    .clone()
                    .or_else(|| {
                        src.file_name()
                            .and_then(|n| n.to_str())
                            .map(str::to_owned)
                    })
                    .unwrap_or_default();
                self.start_video_job(
                    ctx,
                    t!("Aplicando edições no vídeo", "Applying edits to video"),
                    move || {
                        record::edit_video(&src, &opts, &src)
                            .map(|p| VideoJobDone::Overwritten { file, path: p })
                    },
                );
            }
            video_editor::VideoAction::ExportGifToLibrary { opts } => {
                let Some(ve) = &self.video_editor else { return };
                let src = ve.path().to_path_buf();
                let file = self.library.reserve_name(platform::local_time(), "gif");
                let out = self.library.path(&file);
                let title = if base_title.is_empty() {
                    "GIF".to_owned()
                } else {
                    base_title
                };
                self.start_video_job(
                    ctx,
                    t!("Criando GIF na biblioteca", "Creating GIF in library"),
                    move || {
                        record::to_gif_at(&src, &opts, &out).map(|p| VideoJobDone::SavedGif {
                            file,
                            title,
                            path: p,
                        })
                    },
                );
            }
            video_editor::VideoAction::ExportMp4As { opts } => {
                let Some(ve) = &self.video_editor else { return };
                let src = ve.path().to_path_buf();
                let default_name = src
                    .file_stem()
                    .map(|s| format!("{}-editado.mp4", s.to_string_lossy()))
                    .unwrap_or_else(|| t!("video.mp4", "video.mp4").to_owned());
                let mut dialog = rfd::FileDialog::new()
                    .add_filter(t!("Vídeo MP4", "MP4 video"), &["mp4"])
                    .set_file_name(&default_name);
                let vids = record::videos_dir();
                if vids.is_dir() {
                    dialog = dialog.set_directory(vids);
                }
                let Some(dest) = dialog.save_file() else {
                    self.status = t!("Exportação cancelada.", "Export canceled.").to_owned();
                    return;
                };
                self.start_video_job(
                    ctx,
                    t!("Exportando vídeo MP4", "Exporting MP4 video"),
                    move || {
                        record::edit_video(&src, &opts, &dest)
                            .map(|p| VideoJobDone::ExportedExternal { path: p })
                    },
                );
            }
        }
    }

    /// Puts the window back at the size and place it had before the floating panel.
    fn leave_pano_hud(&mut self, ctx: &egui::Context) {
        self.pano = None;
        self.video = None;
        self.steps = None;
        self.restore_frames = 0;
        self.shared.busy.store(false, Ordering::SeqCst);
        platform::exclude_from_capture(self.shared.hwnd, false);
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(NORMAL_MIN_SIZE));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
            self.normal_size.unwrap_or(egui::vec2(900.0, 600.0)),
        ));
        if let Some(pos) = self.normal_pos {
            platform::set_window_pos(self.shared.hwnd, pos);
        }
        self.sync_titlebar();
    }

    /// Turns the window into a small panel to control the step-by-step capture.
    fn start_steps(&mut self, rect: platform::PxRect, title: String) {
        self.shared.busy.store(true, Ordering::SeqCst);
        let frame = platform::RegionFrame::show(rect);
        // Asked now, before Start: once the session runs, clicking the system prompt inside the
        // area would become a step.
        let keys_ok = platform::ensure_key_permission();
        self.steps = Some(StepsUi { rect, title, recorder: None, keys_ok, _frame: frame, setup_frames: 6 });
    }

    fn steps_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let hwnd = self.shared.hwnd;
        let Some(steps) = &mut self.steps else { return };
        ctx.request_repaint_after(Duration::from_millis(250));
        hud_setup(&ctx, hwnd, steps.rect, STEPS_HUD_SIZE, &mut steps.setup_frames);

        let (mut start, mut finish, mut cancel, mut undo) = (false, false, false, false);
        let pal = theme::current_palette(ui.ctx());
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(pal.bg_chrome).inner_margin(Margin::symmetric(10, 6)))
            .show(ui, |ui| {
            ui.horizontal_centered(|ui| match &steps.recorder {
                None => {
                    start = kit::primary_button(ui, Some(Icon::RecordDot), t!("Começar", "Start")).clicked();
                    cancel = kit::ghost_button(ui, None, t!("Cancelar", "Cancel"), false).clicked();
                    kit::divider_v(ui, 18.0);
                    if steps.keys_ok {
                        ui.label(t!(
                            "Depois de começar, cada clique na área (e cada Enter) vira um passo.",
                            "Once started, each click in the area (and each Enter) becomes a step."
                        ));
                    } else {
                        // macOS without Input Monitoring: the clicks still count, Enter doesn't.
                        ui.label(egui::RichText::new(t!(
                            "Depois de começar, cada clique na área vira um passo. Para o Enter contar também, permita o SnapKit em Ajustes do Sistema > Privacidade e Segurança > Monitoramento de Entrada.",
                            "Once started, each click in the area becomes a step. For Enter to count too, allow SnapKit in System Settings > Privacy & Security > Input Monitoring."
                        )).size(12.0));
                    }
                }
                Some(recorder) => {
                    let count = recorder.count();
                    finish = ui
                        .add_enabled_ui(count > 0, |ui| {
                            kit::primary_button(ui, Some(Icon::Stop), t!("Concluir", "Finish"))
                        })
                        .inner
                        .clicked();
                    undo = ui
                        .add_enabled_ui(count > 0, |ui| {
                            kit::ghost_button(
                                ui,
                                Some(Icon::Undo),
                                t!("Tirar o último", "Remove the last one"),
                                false,
                            )
                        })
                        .inner
                        .on_hover_text(t!(
                            "Descarta o último passo, se o clique não fazia parte da tarefa.",
                            "Discards the last step, if that click was not part of the task."
                        ))
                        .clicked();
                    cancel = kit::ghost_button(ui, None, t!("Cancelar", "Cancel"), false).clicked();
                    kit::divider_v(ui, 18.0);
                    let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::hover());
                    icons::paint(ui.painter(), ui.ctx(), Icon::RecordDot, dot_rect.center(), 13.0, pal.danger);
                    let text = match count {
                        0 => t!("Clique na área para registrar o primeiro passo", "Click in the area to record the first step").to_owned(),
                        1 => t!("1 passo", "1 step").to_owned(),
                        n if n >= steps::MAX_STEPS => tf!("{n} passos (limite)", "{n} steps (limit)"),
                        n => tf!("{n} passos", "{n} steps"),
                    };
                    ui.label(text);
                }
            });
        });

        if start {
            let repaint = ctx.clone();
            steps.recorder = Some(steps::Recorder::start(steps.rect, hwnd, move || repaint.request_repaint()));
        } else if undo && let Some(recorder) = &steps.recorder {
            recorder.undo_last();
        } else if finish {
            let Some(StepsUi { title, recorder: Some(recorder), .. }) = self.steps.take() else { return };
            let shots = recorder.finish();
            self.leave_pano_hud(&ctx);
            match steps::compose(&shots) {
                Some((guide, markers)) => {
                    let editor = editor::Editor::with_ops(&ctx, guide, markers, self.accent());
                    self.adopt_editor(&ctx, editor, title);
                    self.status = tf!(
                        "Guia com {} passos montado. Duplo clique em uma legenda reescreve o texto; os números podem ser movidos.",
                        "Guide with {} steps ready. Double-click a caption to rewrite it; the numbers can be moved.",
                        shots.len()
                    );
                }
                None => self.status = t!("Nenhum passo foi registrado.", "No step was recorded.").to_owned(),
            }
        } else if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.leave_pano_hud(&ctx);
            self.status = t!("Captura passo a passo cancelada.", "Step capture canceled.").to_owned();
        }
    }

    fn pano_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let hwnd = self.shared.hwnd;
        let Some(pano) = &mut self.pano else { return };
        ctx.request_repaint_after(Duration::from_millis(100));

        hud_setup(&ctx, hwnd, pano.rect, PANO_HUD_SIZE, &mut pano.setup_frames);

        let mut start = false;
        let mut stop = false;
        let mut cancel = false;
        let pal = theme::current_palette(ui.ctx());
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(pal.bg_chrome).inner_margin(Margin::symmetric(10, 6)))
            .show(ui, |ui| {
            ui.horizontal_centered(|ui| match &pano.session {
                None => {
                    start = kit::primary_button(ui, Some(Icon::RecordDot), t!("Iniciar", "Start")).clicked();
                    cancel = kit::ghost_button(ui, None, t!("Cancelar", "Cancel"), false).clicked();
                    ui.label(t!("Depois de iniciar, role a área devagar,\nem qualquer direção.", "Once started, scroll the area slowly,\nin any direction."));
                }
                Some(session) => {
                    stop = kit::primary_button(ui, Some(Icon::Stop), t!("Parar", "Stop")).clicked();
                    let (w, h) = session.size();
                    let lost = session.lost();
                    if lost == 0 {
                        ui.label(tf!(
                            "Capturando… {w} × {h} px\nRole devagar; clique em Parar ao terminar.",
                            "Capturing… {w} × {h} px\nScroll slowly; click Stop when done."
                        ));
                    } else {
                        ui.colored_label(
                            Color32::from_rgb(255, 200, 80),
                            tf!("Capturando… {w} × {h} px\nRápido demais: {lost} fotos não encaixaram.", "Capturing… {w} × {h} px\nToo fast: {lost} shots did not fit."),
                        );
                    }
                }
            });
        });

        if start {
            pano.session = Some(scroll::start_panoramic(pano.rect));
        } else if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) && pano.session.is_none() {
            self.leave_pano_hud(&ctx);
            self.status = t!("Captura panorâmica cancelada.", "Panoramic capture canceled.").to_owned();
        } else if stop {
            let Some(PanoUi { title, session: Some(session), .. }) = self.pano.take() else { return };
            let result = session.finish();
            self.leave_pano_hud(&ctx);
            let shot = result.map(|(image, note)| capture::Shot {
                image,
                windows: Vec::new(),
                title,
                note,
            });
            *self.shared.pending.lock().unwrap() = Some((CaptureKind::FullScreen, shot));
            ctx.request_repaint();
        }
    }

    /// Leaves the selection and puts the window back at its normal size and place.
    fn end_selection(&mut self, ctx: &egui::Context) {
        if self.selecting.take().is_some_and(|s| !s.revealed) {
            self.shared.cloaked.store(false, Ordering::SeqCst);
            platform::cloak(self.shared.hwnd, false);
        }
        self.shared.busy.store(false, Ordering::SeqCst);
        platform::exclude_from_capture(self.shared.hwnd, false);
        platform::set_tool_window(self.shared.hwnd, false);
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
        if let Some(r) = self.normal_rect {
            platform::set_window_rect(self.shared.hwnd, r);
        }
        self.sync_titlebar();
        // The frame comes back a few frames later; the rectangle is reapplied until then.
        self.restore_frames = 10;
        ctx.request_repaint();
    }

    /// Photographs `rect` after the configured delay, with the window out of the way.
    fn capture_after_delay(&mut self, ctx: &egui::Context, rect: platform::PxRect, mut title: String, delay: u32, cursor: bool) {
        if self.shared.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        self.status = tf!("Capturando em {delay} s…", "Capturing in {delay} s…");
        let (shared, ctx) = (self.shared.clone(), ctx.clone());
        std::thread::spawn(move || {
            if shared.hwnd != 0 {
                platform::exclude_from_capture(shared.hwnd, true);
                platform::minimize(shared.hwnd);
            }
            std::thread::sleep(Duration::from_secs(delay as u64));
            let current = platform::foreground_window_title_except(shared.hwnd);
            if !current.is_empty() {
                title = current;
            }
            let shot = guarded(|| capture::grab_virtual(rect, cursor)).map(|image| capture::Shot {
                image,
                windows: Vec::new(),
                title,
                note: String::new(),
            });
            *shared.pending.lock().unwrap() = Some((CaptureKind::FullScreen, shot));
            if shared.hwnd != 0 {
                platform::exclude_from_capture(shared.hwnd, false);
                platform::show(shared.hwnd);
            }
            ctx.request_repaint();
        });
    }

    /// Quick annotation: the capture stays where it came from, with the toolbar next to it.
    /// Dragging the border moves the capture and the corners resize it; the new place is
    /// photographed again, with the annotations kept. Enter copies and closes, Esc cancels.
    fn quick_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let hwnd = self.shared.hwnd;
        let rect = ui.max_rect();
        let accent = self.accent();
        let Some(Selecting { quick: Some(quick), .. }) = &mut self.selecting else { return };
        // Conversion between interface points and screen pixels, to readjust the area.
        let fallback_scale = ctx.pixels_per_point();
        let desktop = platform::virtual_screen();
        let (ox, oy, scale_x, scale_y) = match platform::client_rect_screen(hwnd) {
            Some((cl, ct, cr, cb)) if rect.width() >= 1.0 && rect.height() >= 1.0 => (
                cl,
                ct,
                (cr - cl) as f32 / rect.width(),
                (cb - ct) as f32 / rect.height(),
            ),
            _ => {
                let (ox, oy) = platform::client_origin(hwnd).unwrap_or((desktop.0, desktop.1));
                (ox, oy, fallback_scale, fallback_scale)
            }
        };
        let to_px = |p: Pos2| -> (i32, i32) {
            (
                ox + ((p.x - rect.min.x) * scale_x).round() as i32,
                oy + ((p.y - rect.min.y) * scale_y).round() as i32,
            )
        };
        let px_to_screen = |r: platform::PxRect| {
            Rect::from_min_max(
                rect.min + egui::vec2((r.0 - ox) as f32 / scale_x, (r.1 - oy) as f32 / scale_y),
                rect.min + egui::vec2((r.2 - ox) as f32 / scale_x, (r.3 - oy) as f32 / scale_y),
            )
        };
        let pointer = ctx.pointer_latest_pos();

        // Adjustment in progress: the area follows the cursor, showing the live screen; on release,
        // the new place is photographed again and the annotations remain editable. Moving takes the
        // annotations along with the frame; resizing leaves them over the same spot on the screen.
        if let Some(adj) = quick.adjust {
            if let Some(p) = pointer.filter(|_| ctx.input(|i| i.pointer.primary_down())) {
                match adj {
                    Adjust::Move { grab, start } => quick.area = clamp_move(start, p - grab, rect),
                    Adjust::Resize { handle, start } => quick.area = apply_handle(start, handle, p, rect),
                }
            } else {
                quick.adjust = None;
                quick.overlay = None;
                let (a, b) = (to_px(quick.area.min), to_px(quick.area.max));
                let (dx, dy) = match adj {
                    Adjust::Move { .. } => (0, 0),
                    Adjust::Resize { start, .. } => {
                        let s = to_px(start.min);
                        (s.0 - a.0, s.1 - a.1)
                    }
                };
                if let Ok(image) = capture::grab_virtual((a.0, a.1, b.0, b.1), false) {
                    quick.area = px_to_screen((a.0, a.1, b.0, b.1));
                    quick.editor.commit_pending_text();
                    quick.editor.rebase(image, dx as f32, dy as f32);
                }
            }
        }
        let area = quick.area;
        // Outside the area the screen stays visible, darkened. The layer receives the clicks, so
        // they don't end up in the windows behind.
        ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let dim = Color32::from_black_alpha(110);
        for r in [
            Rect::from_min_max(rect.min, Pos2::new(rect.max.x, area.min.y)),
            Rect::from_min_max(Pos2::new(rect.min.x, area.max.y), rect.max),
            Rect::from_min_max(Pos2::new(rect.min.x, area.min.y), Pos2::new(area.min.x, area.max.y)),
            Rect::from_min_max(Pos2::new(area.max.x, area.min.y), Pos2::new(rect.max.x, area.max.y)),
        ] {
            painter.rect_filled(r, 0.0, dim);
        }

        // Esc and Enter serve the text being typed first; Esc also deselects an annotation first.
        let typing_before = quick.editor.is_typing();
        let selected_before = quick.editor.has_selection();
        // Moving or resizing: the area is hollow and shows the live screen, with only the
        // annotations on top; the old photo dragged along looked like a loose image. On release,
        // the new place is photographed (above) and the image shows again.
        if let Some(adj) = quick.adjust {
            if let Some(tex) = &quick.overlay {
                let (start, origin) = match adj {
                    Adjust::Move { start, .. } => (start, area.min),
                    Adjust::Resize { start, .. } => (start, start.min),
                };
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                ui.painter_at(area).image(tex.id(), Rect::from_min_size(origin, start.size()), uv, Color32::WHITE);
            }
        } else {
            let mut canvas = ui.new_child(egui::UiBuilder::new().max_rect(area));
            quick.editor.canvas_at(&mut canvas, area);
        }
        ui.painter().rect_stroke(area, 0.0, Stroke::new(2.0, accent), StrokeKind::Outside);

        // Movable borders, over the frame; corners that resize.
        const EDGE: f32 = 6.0;
        let edges = [
            Rect::from_min_max(area.min, Pos2::new(area.max.x, area.min.y + EDGE)),
            Rect::from_min_max(Pos2::new(area.min.x, area.max.y - EDGE), area.max),
            Rect::from_min_max(Pos2::new(area.min.x, area.min.y + EDGE), Pos2::new(area.min.x + EDGE, area.max.y - EDGE)),
            Rect::from_min_max(Pos2::new(area.max.x - EDGE, area.min.y + EDGE), Pos2::new(area.max.x, area.max.y - EDGE)),
        ];
        for (i, r) in edges.into_iter().enumerate() {
            let h = ui.interact(r, egui::Id::unique(("quick_border", i as u32)), Sense::click_and_drag());
            if h.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::Move);
            }
            if h.drag_started() && let Some(p) = pointer {
                quick.editor.commit_pending_text();
                quick.adjust = Some(Adjust::Move { grab: p, start: quick.area });
            }
        }
        {
            for (handle, corner) in [(0usize, area.min), (2, area.right_top()), (4, area.max), (6, area.left_bottom())] {
                let grip = Rect::from_center_size(corner, egui::vec2(12.0, 12.0));
                painter.rect_filled(grip, 2.0, accent);
                painter.rect_stroke(grip, 2.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside);
                let h = ui.interact(grip, egui::Id::unique(("quick_grip", handle as u32)), Sense::drag());
                if h.hovered() {
                    ctx.set_cursor_icon(HANDLE_CURSORS[handle]);
                }
                if h.drag_started() {
                    quick.editor.commit_pending_text();
                    quick.adjust = Some(Adjust::Resize { handle, start: quick.area });
                }
            }
        }
        // An adjustment started: keeps the annotations aside, to draw them over the live screen.
        if quick.adjust.is_some() && quick.overlay.is_none() && !quick.editor.is_blank() {
            let image = quick.editor.annotations_image();
            quick.overlay = Some(ctx.load_texture("quick_annotations", image, egui::TextureOptions::LINEAR));
        }
        // Consistent cursor while the adjustment lasts.
        match quick.adjust {
            Some(Adjust::Move { .. }) => ctx.set_cursor_icon(egui::CursorIcon::Move),
            Some(Adjust::Resize { handle, .. }) => ctx.set_cursor_icon(HANDLE_CURSORS[handle]),
            None => {}
        }

        // The bar goes below the area; without room, above; as a last resort, inside it.
        const BAR_H: f32 = 44.0;
        let mut pos = if area.max.y + 10.0 + BAR_H <= rect.max.y {
            area.left_bottom() + egui::vec2(0.0, 10.0)
        } else if area.min.y - 10.0 - BAR_H >= rect.min.y {
            area.left_top() - egui::vec2(0.0, 10.0 + BAR_H)
        } else {
            area.left_bottom() + egui::vec2(8.0, -BAR_H - 8.0)
        };
        pos.x = pos.x.min(rect.max.x - 680.0).max(rect.min.x + 4.0);
        let mut action = None;
        let pal = theme::current_palette(&ctx);
        egui::Area::new(egui::Id::unique("quick_bar"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(&ctx, |ui| {
                kit::floating_capsule_frame(&pal).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        quick.editor.quick_toolbar_ui(ui);
                        kit::divider_v(ui, 18.0);
                        if kit::primary_button(ui, Some(Icon::Copy), t!("Copiar", "Copy"))
                            .on_hover_text(tf!("Copia e fecha (Enter ou {})", "Copies and closes (Enter or {})", cmd_key("C")))
                            .clicked()
                        {
                            action = Some(QuickAction::Copy);
                        }
                        if kit::ghost_button(ui, Some(Icon::Pin), t!("Fixar", "Pin"), false)
                            .on_hover_text(t!(
                                "Fixa na tela sempre visível (P)",
                                "Pins on top of the screen (P)"
                            ))
                            .clicked()
                        {
                            action = Some(QuickAction::Pin);
                        }
                        if kit::ghost_button(ui, Some(Icon::Save), t!("Salvar", "Save"), false)
                            .on_hover_text(cmd_key("S"))
                            .clicked()
                        {
                            action = Some(QuickAction::Save);
                        }
                        if kit::ghost_button(ui, Some(Icon::Edit), t!("Editor", "Editor"), false)
                            .on_hover_text(tf!("Continua no editor completo ({})", "Continues in the full editor ({})", cmd_key("E")))
                            .clicked()
                        {
                            action = Some(QuickAction::Editor);
                        }
                        if kit::icon_button(ui, Icon::Close, false, 26.0)
                            .on_hover_text(t!("Cancelar (Esc)", "Cancel (Esc)"))
                            .clicked()
                        {
                            action = Some(QuickAction::Cancel);
                        }
                    });
                });
            });

        if action.is_none() && !typing_before && !quick.editor.is_typing() && !ctx.egui_wants_keyboard_input() {
            action = ctx.input_mut(|i| {
                let copy = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    || i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
                    || copy
                {
                    Some(QuickAction::Copy)
                } else if i.consume_key(egui::Modifiers::NONE, egui::Key::P) {
                    Some(QuickAction::Pin)
                } else if i.consume_key(egui::Modifiers::COMMAND, egui::Key::S) {
                    Some(QuickAction::Save)
                } else if i.consume_key(egui::Modifiers::COMMAND, egui::Key::E) {
                    Some(QuickAction::Editor)
                } else if !selected_before && i.key_pressed(egui::Key::Escape) {
                    Some(QuickAction::Cancel)
                } else {
                    None
                }
            });
        }
        if let Some(action) = action {
            let (a, b) = (to_px(area.min), to_px(area.max));
            self.finish_quick(&ctx, action, (a.0, a.1, b.0, b.1));
        }
    }

    fn finish_quick(&mut self, ctx: &egui::Context, action: QuickAction, px_rect: platform::PxRect) {
        let Some(sel) = self.selecting.as_mut() else { return };
        let title = sel.title.clone();
        let Some(quick) = sel.quick.take() else { return };
        match action {
            QuickAction::Cancel => {
                self.status = t!("Captura cancelada.", "Capture canceled.").to_owned();
                self.dismiss_selection(ctx);
            }
            QuickAction::Copy => {
                self.adopt_editor(ctx, quick.editor, title);
                // Copied: the capture is already on the clipboard and in the library. The window
                // leaves the screen even if it was open before, so the editor doesn't jump to the
                // front.
                if let Some(sel) = &mut self.selecting {
                    sel.was_on_screen = false;
                }
                self.dismiss_selection(ctx);
            }
            QuickAction::Pin => {
                let mut editor = quick.editor;
                editor.commit_pending_text();
                let img = editor.image().clone();
                let acc = self.accent();
                self.adopt_editor(ctx, editor, title);
                if let Some(sel) = &mut self.selecting {
                    sel.was_on_screen = false;
                }
                self.dismiss_selection(ctx);
                if let Err(e) = pin::spawn(img, [acc.r(), acc.g(), acc.b()], Some(px_rect)) {
                    self.status = tf!(
                        "Não foi possível fixar na tela: {e}",
                        "Could not pin to screen: {e}"
                    );
                } else {
                    self.status = t!(
                        "Captura fixada na tela (arraste para mover, Esc fecha).",
                        "Capture pinned to screen (drag to move, Esc closes)."
                    )
                    .to_owned();
                }
            }
            QuickAction::Save => {
                self.adopt_editor(ctx, quick.editor, title);
                self.end_selection(ctx);
                self.settle_window(false);
                // The save dialog only opens after our window is back to its normal size.
                self.save_requested = true;
            }
            QuickAction::Editor => {
                self.adopt_editor(ctx, quick.editor, title);
                self.end_selection(ctx);
                self.settle_window(false);
            }
        }
    }

    /// Leaves the selection without opening the editor: if the window was hidden before the
    /// capture, it goes back to being hidden.
    fn dismiss_selection(&mut self, ctx: &egui::Context) {
        let was_on_screen = self.selecting.as_ref().is_none_or(|s| s.was_on_screen);
        self.end_selection(ctx);
        self.settle_window(!was_on_screen);
    }

    /// Keeps the window invisible while it returns from the selection (normal size, frame). With
    /// `put_away`, it leaves the screen at the end without ever appearing; otherwise, it reappears
    /// ready.
    fn settle_window(&mut self, put_away: bool) {
        if self.shared.hwnd == 0 {
            return;
        }
        self.shared.cloaked.store(true, Ordering::SeqCst);
        platform::cloak(self.shared.hwnd, true);
        self.settle = Some(Settle { put_away, hidden: false, deadline: Instant::now() + Duration::from_millis(600) });
    }

    /// Follows the return from the selection, every frame, until the window can reappear or leave
    /// the screen.
    fn settle_step(&mut self, ctx: &egui::Context) {
        let Some(settle) = &self.settle else { return };
        let (put_away, hidden, deadline) = (settle.put_away, settle.hidden, settle.deadline);
        let hwnd = self.shared.hwnd;
        let now = Instant::now();
        let restored = self.restore_frames == 0 || now >= deadline;
        if !put_away {
            if restored {
                self.shared.cloaked.store(false, Ordering::SeqCst);
                platform::cloak(hwnd, false);
                self.settle = None;
            }
        } else if !hidden {
            if restored {
                self.restore_frames = 0;
                self.put_away();
                // The frame restored by winit can show the window again right after the hide
                // request; it stays invisible a moment longer, to be safe.
                self.settle = Some(Settle { put_away, hidden: true, deadline: now + Duration::from_millis(300) });
            }
        } else if now >= deadline {
            self.shared.cloaked.store(false, Ordering::SeqCst);
            platform::cloak(hwnd, false);
            self.settle = None;
        } else {
            self.put_away();
        }
        ctx.request_repaint();
    }

    /// macOS: the Dock icon follows the main window. Put away in the menu bar (hidden, not
    /// minimized), SnapKit leaves the Dock and the app menu bar; when the window comes back, so does
    /// the icon. Without a menu bar icon the Dock is the only way back, so it stays. Captures in
    /// progress don't count: the window is only passing through (selection layer, minimized while
    /// scrolling, panels).
    #[cfg(target_os = "macos")]
    fn sync_dock_icon(&mut self) {
        if self.tray.is_none() || self.selecting.is_some() || self.settle.is_some() || self.shared.busy.load(Ordering::SeqCst) {
            return;
        }
        let visible = !platform::is_hidden(self.shared.hwnd);
        if visible != self.dock_visible {
            self.dock_visible = visible;
            platform::set_dock_icon(visible);
        }
    }

    /// Takes the window off the screen: to the tray, if there is one, or to the taskbar.
    fn put_away(&self) {
        let hwnd = self.shared.hwnd;
        if !platform::is_on_screen(hwnd) {
            return;
        }
        if let Some(r) = self.normal_rect {
            platform::set_window_rect(hwnd, r);
        }
        if self.tray.is_some() {
            platform::hide(hwnd);
        } else {
            platform::minimize(hwnd);
        }
    }

    fn selection_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let hwnd = self.shared.hwnd;
        // The window has to cover every monitor. It loses its frame a few frames after the request,
        // and the system can change its size in the meantime; that's why the check runs every
        // frame.
        let desktop = self.selecting.as_ref().map_or_else(platform::selection_area, |s| s.area);
        // Checked every frame: the system or winit can put the normal style back meanwhile.
        platform::set_tool_window(hwnd, true);
        let covering = platform::window_rect(hwnd) == Some(desktop);
        if !covering {
            platform::set_window_rect(hwnd, desktop);
        }
        // The window started invisible: it appears once it has been drawn a few times at the right
        // size, so the switch doesn't flicker.
        if let Some(sel) = self.selecting.as_mut().filter(|s| !s.revealed) {
            sel.ready_frames = if covering { sel.ready_frames + 1 } else { 0 };
            if sel.ready_frames >= 3 || sel.started.elapsed() > Duration::from_millis(500) {
                self.shared.cloaked.store(false, Ordering::SeqCst);
                platform::cloak(hwnd, false);
                sel.revealed = true;
            }
        }
        if self.selecting.as_ref().is_some_and(|s| s.quick.is_some()) {
            self.quick_ui(ui);
            return;
        }
        // While a button is pressed, remember where the press started.
        if let Some(sel) = &mut self.selecting
            && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
        {
            sel.drag_origin = Some(origin);
        }
        let Some(sel) = &self.selecting else { return };
        let (kind, title) = (sel.kind, sel.title.clone());
        let rect = ui.max_rect();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        // The screen underneath stays live; the magnifier follows.
        ctx.request_repaint();

        // Conversion between interface points and screen pixels.
        let fallback_scale = ctx.pixels_per_point();
        let (ox, oy, scale_x, scale_y) = match platform::client_rect_screen(hwnd) {
            Some((cl, ct, cr, cb)) if rect.width() >= 1.0 && rect.height() >= 1.0 => (
                cl,
                ct,
                (cr - cl) as f32 / rect.width(),
                (cb - ct) as f32 / rect.height(),
            ),
            _ => {
                let (ox, oy) = platform::client_origin(hwnd).unwrap_or((desktop.0, desktop.1));
                (ox, oy, fallback_scale, fallback_scale)
            }
        };
        let to_px = |p: Pos2| -> (i32, i32) {
            (
                ox + ((p.x - rect.min.x) * scale_x).round() as i32,
                oy + ((p.y - rect.min.y) * scale_y).round() as i32,
            )
        };
        let px_to_screen = |r: platform::PxRect| {
            Rect::from_min_max(
                rect.min + egui::vec2((r.0 - ox) as f32 / scale_x, (r.1 - oy) as f32 / scale_y),
                rect.min + egui::vec2((r.2 - ox) as f32 / scale_x, (r.3 - oy) as f32 / scale_y),
            )
        };

        let pointer = ctx.pointer_latest_pos();
        let press_origin = ctx.input(|i| i.pointer.press_origin());
        let mut drawn = sel.drawn;
        let mut adjust = sel.adjust;
        let mut confirm = false;
        let mut hover_cursor = None;

        // Dragging (no armed region): the region goes from the point where the button went down to
        // the cursor.
        let dragging = response.dragged() || response.drag_stopped();
        let drag_rect = match (dragging, sel.drag_origin, pointer) {
            (true, Some(a), Some(b)) if drawn.is_none() => Some(Rect::from_two_pos(a, b).intersect(rect)),
            _ => None,
        };
        // Drag released: the region stays armed for adjustment instead of being captured right
        // away. In region mode, it goes straight into quick annotation: a single step, with the
        // arrows at hand.
        let mut quick_now = None;
        if response.drag_stopped()
            && drawn.is_none()
            && let Some(r) = drag_rect.filter(|r| r.width() >= 4.0 && r.height() >= 4.0)
        {
            if kind == CaptureKind::Region
                && self.cfg.quick_annotate
                && self.shared.delay_secs.load(Ordering::Relaxed) == 0
            {
                quick_now = Some(r);
            } else {
                drawn = Some(r);
                adjust = None;
            }
        }
        // A new drag outside the armed region: back to drawing over it.
        if let Some(area) = drawn
            && response.drag_started()
            && press_origin.is_some_and(|p| !area.contains(p))
        {
            drawn = None;
            adjust = None;
        }
        // No armed region: highlights the window under the cursor.
        let hovered_window = if drawn.is_none() {
            match (drag_rect, pointer) {
                (None, Some(p)) => {
                    let (x, y) = to_px(p);
                    capture::window_at(&sel.windows, x as f32, y as f32).map(|w| px_to_screen(w).intersect(rect))
                }
                _ => None,
            }
        } else {
            None
        };

        // Automatic scrolling arrows over the highlighted window (region mode only).
        let arrows: Vec<(scroll::Direction, Rect)> = match hovered_window {
            Some(w) if kind == CaptureKind::Region && w.width() > 180.0 && w.height() > 140.0 => {
                let size = egui::vec2(40.0, 40.0);
                vec![
                    (scroll::Direction::Vertical, Rect::from_center_size(w.center_bottom() - egui::vec2(0.0, 34.0), size)),
                    (scroll::Direction::Horizontal, Rect::from_center_size(w.right_center() - egui::vec2(34.0, 0.0), size)),
                    (scroll::Direction::Both, Rect::from_center_size(w.right_bottom() - egui::vec2(34.0, 34.0), size)),
                ]
            }
            _ => Vec::new(),
        };
        let hovered_arrow = pointer.and_then(|p| arrows.iter().find(|(_, r)| r.contains(p)).map(|(d, _)| *d));

        // Armed region: dragging the middle moves it; the handles resize it; double-clicking
        // confirms.
        if let Some(area) = drawn {
            let inside = ui.interact(area, egui::Id::unique("region_adjust"), Sense::click_and_drag());
            if inside.hovered() {
                hover_cursor = Some(egui::CursorIcon::Move);
            }
            if inside.drag_started() && let Some(p) = pointer {
                adjust = Some(Adjust::Move { grab: p, start: area });
            }
            if inside.double_clicked() {
                confirm = true;
            }
            for (i, handle) in handles(area).into_iter().enumerate() {
                let h = ui.interact(handle, egui::Id::unique(("region_handle", i as u32)), Sense::drag());
                if h.drag_started() {
                    adjust = Some(Adjust::Resize { handle: i, start: area });
                }
                if h.hovered() {
                    hover_cursor = Some(HANDLE_CURSORS[i]);
                }
            }
        }
        // Adjusting: the region follows the cursor; when the button is released, the adjustment
        // ends.
        if let Some(a) = adjust {
            if let Some(p) = pointer.filter(|_| ctx.input(|i| i.pointer.primary_down())) {
                match a {
                    Adjust::Move { grab, start } => drawn = Some(clamp_move(start, p - grab, rect)),
                    Adjust::Resize { handle, start } => drawn = Some(apply_handle(start, handle, p, rect)),
                }
            } else {
                adjust = None;
            }
        }
        match adjust {
            Some(Adjust::Move { .. }) => ctx.set_cursor_icon(egui::CursorIcon::Move),
            Some(Adjust::Resize { handle, .. }) => ctx.set_cursor_icon(HANDLE_CURSORS[handle]),
            None => {
                if let Some(c) = hover_cursor {
                    ctx.set_cursor_icon(c);
                }
            }
        }

        // Arrow keys nudge an armed region by 1 px (or 10 px with Shift), clamped inside `rect`.
        if let Some(area) = drawn
            && adjust.is_none()
        {
            let delta = ctx.input_mut(|i| {
                let (mods, step) = if i.modifiers.shift {
                    (egui::Modifiers::SHIFT, 10.0)
                } else {
                    (egui::Modifiers::NONE, 1.0)
                };
                let mut dx = 0.0f32;
                let mut dy = 0.0f32;
                if i.consume_key(mods, egui::Key::ArrowLeft) {
                    dx -= step / scale_x;
                }
                if i.consume_key(mods, egui::Key::ArrowRight) {
                    dx += step / scale_x;
                }
                if i.consume_key(mods, egui::Key::ArrowUp) {
                    dy -= step / scale_y;
                }
                if i.consume_key(mods, egui::Key::ArrowDown) {
                    dy += step / scale_y;
                }
                egui::vec2(dx, dy)
            });
            if delta != egui::Vec2::ZERO {
                drawn = Some(clamp_move(area, delta, rect));
            }
        }

        let dim = Color32::from_black_alpha(110);
        match drawn.or(drag_rect).or(hovered_window) {
            Some(s) => {
                // Darkens the four strips around the highlighted area.
                for r in [
                    Rect::from_min_max(rect.min, Pos2::new(rect.max.x, s.min.y)),
                    Rect::from_min_max(Pos2::new(rect.min.x, s.max.y), rect.max),
                    Rect::from_min_max(Pos2::new(rect.min.x, s.min.y), Pos2::new(s.min.x, s.max.y)),
                    Rect::from_min_max(Pos2::new(s.max.x, s.min.y), Pos2::new(rect.max.x, s.max.y)),
                ] {
                    painter.rect_filled(r, 0.0, dim);
                }
                // Nearly invisible, but not empty: makes sure a click in the middle is ours.
                painter.rect_filled(s, 0.0, Color32::from_black_alpha(1));
                painter.rect_stroke(s, 0.0, Stroke::new(2.0, self.accent()), StrokeKind::Inside);
                let size = format!("{:.0} × {:.0}", s.width() * scale_x, s.height() * scale_y);
                let label = if drawn.is_some() {
                    tf!("{size}  •  arraste para mover, puxe as alças, {} captura", "{size}  •  drag to move, pull the handles, {} captures", cmd_key("C"))
                } else if drag_rect.is_some() {
                    size
                } else {
                    match hovered_arrow {
                        Some(scroll::Direction::Vertical) => t!("Capturar com rolagem vertical", "Capture with vertical scrolling").to_owned(),
                        Some(scroll::Direction::Horizontal) => t!("Capturar com rolagem horizontal", "Capture with horizontal scrolling").to_owned(),
                        Some(scroll::Direction::Both) => t!("Capturar com rolagem nos dois sentidos", "Capture scrolling both ways").to_owned(),
                        None if arrows.is_empty() => tf!(
                            "{size}  •  clique captura esta janela, arraste escolhe uma região",
                            "{size}  •  click captures this window, drag picks a region"
                        ),
                        None => tf!(
                            "{size}  •  clique captura a janela, setas capturam com rolagem",
                            "{size}  •  click captures the window, arrows capture with scrolling"
                        ),
                    }
                };
                for (direction, r) in &arrows {
                    let hot = hovered_arrow == Some(*direction);
                    let fill = if hot { self.accent() } else { Color32::from_black_alpha(200) };
                    painter.circle_filled(r.center(), 20.0, fill);
                    painter.circle_stroke(r.center(), 20.0, Stroke::new(1.5, Color32::WHITE));
                    let arrow = Stroke::new(2.5, Color32::WHITE);
                    let c = r.center();
                    match direction {
                        scroll::Direction::Vertical => painter.arrow(c - egui::vec2(0.0, 9.0), egui::vec2(0.0, 18.0), arrow),
                        scroll::Direction::Horizontal => painter.arrow(c - egui::vec2(9.0, 0.0), egui::vec2(18.0, 0.0), arrow),
                        scroll::Direction::Both => {
                            painter.arrow(c - egui::vec2(7.0, 7.0), egui::vec2(14.0, 14.0), arrow);
                        }
                    }
                }
                if hovered_arrow.is_some() {
                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                // The label goes above the area, or inside it when there's no room above.
                let above = s.min.y - rect.min.y >= 26.0;
                let pos = s.left_top() + egui::vec2(6.0, if above { -24.0 } else { 6.0 });
                let galley = painter.layout_no_wrap(label, egui::FontId::proportional(15.0), Color32::WHITE);
                let bg = Rect::from_min_size(pos, galley.size()).expand2(egui::vec2(6.0, 3.0));
                painter.rect_filled(bg, 4.0, Color32::from_black_alpha(190));
                painter.galley(pos, galley, Color32::WHITE);
                // Handles of the armed region: drag them to resize.
                if let Some(area) = drawn {
                    for hr in handles(area) {
                        painter.rect_filled(hr, 2.0, self.accent());
                        painter.rect_stroke(hr, 2.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside);
                    }
                }
            }
            None => {
                painter.rect_filled(rect, 0.0, dim);
                if let Some(p) = pointer {
                    let text = match kind {
                        CaptureKind::Scroll => t!(
                            "Arraste sobre a área que rola (ou clique na janela)  •  Esc cancela",
                            "Drag over the area that scrolls (or click the window)  •  Esc cancels"
                        ),
                        CaptureKind::Panoramic => t!(
                            "Arraste sobre a área que você vai rolar  •  Esc cancela",
                            "Drag over the area you are going to scroll  •  Esc cancels"
                        ),
                        CaptureKind::Steps => t!(
                            "Arraste sobre a área da tarefa (ou clique na janela)  •  Esc cancela",
                            "Drag over the area of the task (or click the window)  •  Esc cancels"
                        ),
                        CaptureKind::Video | CaptureKind::Gif => t!(
                            "Arraste sobre a área a gravar (ou clique na janela)  •  Esc cancela",
                            "Drag over the area to record (or click the window)  •  Esc cancels"
                        ),
                        _ => t!("Arraste para selecionar uma região  •  Esc cancela", "Drag to select a region  •  Esc cancels"),
                    };
                    let galley = painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(15.0), Color32::WHITE);
                    let pos = p + egui::vec2(22.0, -34.0);
                    let bg = Rect::from_min_size(pos, galley.size()).expand2(egui::vec2(6.0, 3.0));
                    painter.rect_filled(bg, 4.0, Color32::from_black_alpha(190));
                    painter.galley(pos, galley, Color32::WHITE);
                }
            }
        }

        // Pixel magnifier next to the cursor, read from the screen every frame.
        if let Some(p) = pointer.filter(|p| drawn.is_none() && rect.contains(*p) && hovered_arrow.is_none()) {
            const CELLS: i32 = 15;
            const CELL: f32 = 8.0;
            let side = CELLS as f32 * CELL;
            let (cx, cy) = to_px(p);
            // It sits below and to the right of the cursor; it switches sides when it doesn't fit
            // on the screen.
            let mut min = p + egui::vec2(22.0, 22.0);
            if min.x + side > rect.max.x - 4.0 {
                min.x = p.x - 22.0 - side;
            }
            if min.y + side + 26.0 > rect.max.y - 4.0 {
                min.y = p.y - 22.0 - side - 26.0;
            }
            let loupe = Rect::from_min_size(min, egui::Vec2::splat(side));
            painter.rect_filled(loupe.expand(2.0), 3.0, Color32::from_gray(25));
            let mut center_rgb: Option<[u8; 3]> = None;
            if let Some(pixels) = platform::grab_pixels(cx - CELLS / 2, cy - CELLS / 2, CELLS, CELLS) {
                let center_idx = ((CELLS / 2) * CELLS + (CELLS / 2)) as usize;
                center_rgb = pixels.get(center_idx).copied();
                for (i, [r, g, b]) in pixels.into_iter().enumerate() {
                    let (col, row) = ((i as i32 % CELLS) as f32, (i as i32 / CELLS) as f32);
                    let cell = Rect::from_min_size(loupe.min + egui::vec2(col, row) * CELL, egui::Vec2::splat(CELL));
                    painter.rect_filled(cell, 0.0, Color32::from_rgb(r, g, b));
                }
            }
            let mid = (CELLS / 2) as f32 * CELL;
            let center = Rect::from_min_size(loupe.min + egui::vec2(mid, mid), egui::Vec2::splat(CELL));
            painter.rect_stroke(center, 0.0, Stroke::new(1.5, self.accent()), StrokeKind::Outside);
            painter.rect_stroke(loupe, 3.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside);
            let pill_text = match center_rgb {
                Some([r, g, b]) => format!("{cx}, {cy}  •  #{r:02X}{g:02X}{b:02X}"),
                None => format!("{cx}, {cy}"),
            };
            let coords = painter.layout_no_wrap(pill_text, egui::FontId::monospace(12.5), Color32::WHITE);
            let mut pos = loupe.left_bottom() + egui::vec2(0.0, 6.0);
            if pos.x + coords.size().x + 10.0 > rect.max.x - 4.0 {
                pos.x = (rect.max.x - 4.0 - coords.size().x - 6.0).max(rect.min.x + 4.0);
            }
            let bg = Rect::from_min_size(pos, coords.size()).expand2(egui::vec2(5.0, 2.0));
            painter.rect_filled(bg, 3.0, Color32::from_black_alpha(200));
            painter.galley(pos, coords, Color32::WHITE);

            if let Some([r, g, b]) = center_rgb
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::C))
            {
                let hex = format!("#{r:02X}{g:02X}{b:02X}");
                let _ = copy_text(&hex);
                self.status = tf!(
                    "Cor copiada: #{r:02X}{g:02X}{b:02X}",
                    "Color copied: #{r:02X}{g:02X}{b:02X}"
                );
                self.dismiss_selection(&ctx);
                return;
            }
        }

        // Esc cancels the whole capture; the Redo button reopens the region drawing.
        let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape)) || platform::escape_pressed();

        // Confirmation bar of the armed region.
        let mut bar = None;
        if let Some(area) = drawn {
            const BAR_H: f32 = 40.0;
            let mut pos = if area.max.y + 10.0 + BAR_H <= rect.max.y {
                area.left_bottom() + egui::vec2(0.0, 10.0)
            } else if area.min.y - 10.0 - BAR_H >= rect.min.y {
                area.left_top() - egui::vec2(0.0, 10.0 + BAR_H)
            } else {
                area.left_bottom() + egui::vec2(8.0, -BAR_H - 8.0)
            };
            pos.x = pos.x.min(rect.max.x - 330.0).max(rect.min.x + 4.0);
            let pal = theme::current_palette(&ctx);
            egui::Area::new(egui::Id::unique("adjust_bar"))
                .order(egui::Order::Foreground)
                .fixed_pos(pos)
                .show(&ctx, |ui| {
                    kit::floating_capsule_frame(&pal).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if kit::primary_button(
                                ui,
                                Some(Icon::CaptureFrame),
                                t!("Capturar (Enter)", "Capture (Enter)"),
                            )
                            .on_hover_text(tf!(
                                "{} ou duplo clique dentro da região também captura",
                                "{} or a double-click inside the region also captures",
                                cmd_key("C")
                            ))
                            .clicked()
                            {
                                bar = Some(true);
                            }
                            if kit::ghost_button(
                                ui,
                                Some(Icon::Refresh),
                                t!("Refazer", "Redo"),
                                false,
                            )
                            .on_hover_text(t!("Desenha a região de novo", "Draws the region again"))
                            .clicked()
                            {
                                bar = Some(false);
                            }
                            if kit::icon_button(ui, Icon::Close, false, 28.0)
                                .on_hover_text(t!("Cancelar (Esc)", "Cancel (Esc)"))
                                .clicked()
                            {
                                cancel = true;
                            }
                        });
                    });
                });
        }

        if bar == Some(true) {
            confirm = true;
        } else if bar == Some(false) {
            drawn = None;
            adjust = None;
        }
        // Enter or Ctrl+C confirm the armed region as it is.
        if drawn.is_some() && ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                || i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
                || i.events.iter().any(|e| matches!(e, egui::Event::Copy))
        }) {
            confirm = true;
        }
        if cancel {
            self.status = t!("Captura cancelada.", "Capture canceled.").to_owned();
            self.dismiss_selection(&ctx);
            return;
        }

        if let Some(sel) = &mut self.selecting {
            sel.drawn = drawn;
            sel.adjust = adjust;
            if confirm {
                sel.drag_origin = None;
            }
        }

        // Print Screen region: takes the shot right away and opens the tools over the screen, in a
        // single step.
        if let Some(area) = quick_now {
            let (a, b) = (to_px(area.min), to_px(area.max));
            let cursor = self.shared.include_cursor.load(Ordering::Relaxed);
            match capture::grab_virtual((a.0, a.1, b.0, b.1), cursor) {
                Ok(image) => {
                    let editor = editor::Editor::new(&ctx, image, self.accent());
                    if let Some(sel) = &mut self.selecting {
                        sel.quick = Some(Quick { area: px_to_screen((a.0, a.1, b.0, b.1)), editor, adjust: None, overlay: None });
                        sel.drag_origin = None;
                        sel.drawn = None;
                        sel.adjust = None;
                    }
                    ctx.request_repaint();
                    return;
                }
                Err(e) => {
                    self.status = e;
                    self.dismiss_selection(&ctx);
                    return;
                }
            }
        }

        // With no armed region, a click captures the whole window, as before.
        let chosen = if confirm {
            Some(drawn)
        } else if response.clicked() && drawn.is_none() {
            Some(hovered_window)
        } else {
            None
        };
        if let Some(area) = chosen {
            // The area in screen pixels; it can span monitors.
            let screen_rect = area
                .map(|s| {
                    let (a, b) = (to_px(s.min), to_px(s.max));
                    (a.0, a.1, b.0, b.1)
                })
                .filter(|r| r.2 - r.0 >= 4 && r.3 - r.1 >= 4);
            // Click on an arrow: automatic scrolling of the highlighted window, in the direction of
            // the arrow.
            let arrow = if response.clicked() { hovered_arrow } else { None };
            let delay = self.shared.delay_secs.load(Ordering::Relaxed);
            let cursor = self.shared.include_cursor.load(Ordering::Relaxed);
            match screen_rect {
                None => {
                    self.end_selection(&ctx);
                    self.settle_window(false);
                    self.status = t!("Seleção pequena demais; captura cancelada.", "Selection too small; capture canceled.").to_owned();
                }
                Some(r) if arrow.is_some() => {
                    self.end_selection(&ctx);
                    self.start_scroll(&ctx, r, arrow.unwrap_or(scroll::Direction::Vertical), title);
                }
                Some(r) if kind == CaptureKind::Scroll => {
                    self.end_selection(&ctx);
                    self.start_scroll(&ctx, r, scroll::Direction::Vertical, title);
                }
                Some(r) if kind == CaptureKind::Panoramic => {
                    self.end_selection(&ctx);
                    self.start_panoramic(r, title);
                }
                Some(r) if kind == CaptureKind::Steps => {
                    self.end_selection(&ctx);
                    self.start_steps(r, title);
                }
                Some(r) if matches!(kind, CaptureKind::Video | CaptureKind::Gif) => {
                    self.end_selection(&ctx);
                    self.start_video(r, kind == CaptureKind::Gif, title);
                }
                Some(r) if delay == 0 => {
                    // Takes the shot right away: our layer still covers the screen, but doesn't
                    // show in captures.
                    match capture::grab_virtual(r, cursor) {
                        Ok(image) if kind == CaptureKind::Region && self.cfg.quick_annotate => {
                            // Quick annotation: the capture stays where it was, ready for arrows.
                            let editor = editor::Editor::new(&ctx, image, self.accent());
                            if let Some(s) = &mut self.selecting {
                                s.quick = Some(Quick { area: px_to_screen(r), editor, adjust: None, overlay: None });
                                s.drag_origin = None;
                                s.drawn = None;
                                s.adjust = None;
                            }
                        }
                        Ok(image) => {
                            self.end_selection(&ctx);
                            self.settle_window(false);
                            self.set_captured(&ctx, image, title);
                        }
                        Err(e) => {
                            self.end_selection(&ctx);
                            self.settle_window(false);
                            self.status = e;
                        }
                    }
                }
                Some(r) => {
                    self.end_selection(&ctx);
                    self.capture_after_delay(&ctx, r, title, delay, cursor);
                }
            }
        }
    }

    fn button_with_shortcut(&self, base: &str, mode: config::CaptureShortcutMode) -> String {
        match self.cfg.mode_hotkeys.get(mode) {
            Some(hk) => format!("{base} ({})", hk.label()),
            None => base.to_owned(),
        }
    }

    fn home_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let pal = theme::current_palette(&ctx);
        let mut styles_changed = false;
        let mut video_action = None;
        egui::Panel::top("bar")
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(pal.bg_chrome).inner_margin(Margin::ZERO))
            .show(ui, |ui| {
                egui::Frame::NONE
                    .inner_margin(Margin::symmetric(10, 7))
                    .show(ui, |ui| {
                        // Program row: on the left what captures, on the right what you do with the capture.
                        // With a narrow window the buttons move to the next row instead of overlapping.
                        ui.horizontal_wrapped(|ui| {
                            if kit::primary_button(
                                ui,
                                Some(Icon::Region),
                                &tf!("Região ({})", "Region ({})", self.cfg.hotkey.label()),
                            )
                            .clicked()
                            {
                                start_capture(CaptureKind::Region, self.shared.clone(), ctx.clone());
                            }
                            let fs_label = self.button_with_shortcut(
                                t!("Tela cheia", "Full screen"),
                                config::CaptureShortcutMode::FullScreen,
                            );
                            if kit::ghost_button(ui, Some(Icon::FullScreen), &fs_label, false)
                                .on_hover_text(t!("O monitor onde está o cursor.", "The monitor the cursor is on."))
                                .clicked()
                            {
                                start_capture(CaptureKind::FullScreen, self.shared.clone(), ctx.clone());
                            }
                            if platform::monitor_count() > 1 {
                                let all_label = self.button_with_shortcut(
                                    t!("Todas as telas", "All screens"),
                                    config::CaptureShortcutMode::AllScreens,
                                );
                                if kit::ghost_button(ui, Some(Icon::AllScreens), &all_label, false)
                                    .on_hover_text(t!("Todos os monitores em uma imagem só.", "Every monitor in a single image."))
                                    .clicked()
                                {
                                    start_capture(CaptureKind::AllScreens, self.shared.clone(), ctx.clone());
                                }
                            }
                            let scroll_label = self.button_with_shortcut(
                                t!("Rolagem", "Scrolling"),
                                config::CaptureShortcutMode::Scrolling,
                            );
                            if kit::ghost_button(ui, Some(Icon::Scrolling), &scroll_label, false)
                                .on_hover_text(t!(
                                    "Escolha a área que rola; o SnapKit desce a página e junta tudo em uma imagem.",
                                    "Pick the area that scrolls; SnapKit scrolls the page and joins it all into one image."
                                ))
                                .clicked()
                            {
                                start_capture(CaptureKind::Scroll, self.shared.clone(), ctx.clone());
                            }
                            let pano_label = self.button_with_shortcut(
                                t!("Panorâmica", "Panoramic"),
                                config::CaptureShortcutMode::Panoramic,
                            );
                            if kit::ghost_button(ui, Some(Icon::Panoramic), &pano_label, false)
                                .on_hover_text(t!(
                                    "Escolha uma área e role você mesmo, em qualquer direção; o SnapKit monta a imagem.",
                                    "Pick an area and scroll it yourself, in any direction; SnapKit builds the image."
                                ))
                                .clicked()
                            {
                                start_capture(CaptureKind::Panoramic, self.shared.clone(), ctx.clone());
                            }
                            let video_label = self.button_with_shortcut(
                                t!("Vídeo", "Video"),
                                config::CaptureShortcutMode::Video,
                            );
                            if kit::ghost_button(ui, Some(Icon::Video), &video_label, false)
                                .on_hover_text(t!(
                                    "Escolha uma área e grave a tela em MP4, com som do sistema e microfone.",
                                    "Pick an area and record the screen as MP4, with system sound and microphone."
                                ))
                                .clicked()
                            {
                                start_capture(CaptureKind::Video, self.shared.clone(), ctx.clone());
                            }
                            if kit::ghost_button(ui, Some(Icon::Gif), "GIF", false)
                                .on_hover_text(t!(
                                    "Escolha uma área e grave; ao parar, a gravação vira um GIF animado, sem som.",
                                    "Pick an area and record; when stopped, the recording becomes an animated GIF, without sound."
                                ))
                                .clicked()
                            {
                                start_capture(CaptureKind::Gif, self.shared.clone(), ctx.clone());
                            }
                            let steps_label = self.button_with_shortcut(
                                t!("Passo a passo", "Step by step"),
                                config::CaptureShortcutMode::Steps,
                            );
                            if kit::ghost_button(ui, Some(Icon::Steps), &steps_label, false)
                                .on_hover_text(t!(
                                    "Escolha uma área e faça a tarefa: cada clique vira uma foto numerada de um guia.",
                                    "Pick an area and do the task: each click becomes a numbered shot of a guide."
                                ))
                                .clicked()
                            {
                                start_capture(CaptureKind::Steps, self.shared.clone(), ctx.clone());
                            }
                            let (mut delay, mut cursor) = (self.cfg.delay_secs, self.cfg.include_cursor);
                            let summary = match (delay, cursor) {
                                (0, false) => t!("Opções", "Options").to_owned(),
                                (0, true) => t!("Opções: com cursor", "Options: with cursor").to_owned(),
                                (secs, false) => tf!("Opções: {}", "Options: {}", delay_label(secs)),
                                (secs, true) => tf!("Opções: {}, com cursor", "Options: {}, with cursor", delay_label(secs)),
                            };
                            kit::ghost_menu_button(ui, Some(Icon::Options), &summary, |ui| {
                                ui.weak(t!("Esperar antes de capturar", "Wait before capturing"));
                                for secs in [0, 3, 5, 10] {
                                    ui.radio_value(&mut delay, secs, delay_label(secs));
                                }
                                ui.separator();
                                ui.checkbox(&mut cursor, t!("Incluir o cursor do mouse", "Include the mouse cursor"));
                            });
                            if delay != self.cfg.delay_secs || cursor != self.cfg.include_cursor {
                                self.cfg.delay_secs = delay;
                                self.cfg.include_cursor = cursor;
                                self.shared.delay_secs.store(delay, Ordering::Relaxed);
                                self.shared.include_cursor.store(cursor, Ordering::Relaxed);
                                self.save_cfg();
                            }
                            // The right-hand group needs room of its own; without it, it moves as a whole to
                            // another row. (In a wrapping row, `available_width` gives the whole row; what's
                            // really left goes from the next button to the edge.)
                            if ui.max_rect().right() - ui.cursor().left() < RIGHT_GROUP_WIDTH {
                                ui.end_row();
                            }
                            // Right to left: the first item goes at the right end.
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if kit::icon_button(ui, Icon::Preferences, self.show_prefs, 28.0)
                                    .on_hover_text(t!("Preferências", "Preferences"))
                                    .clicked()
                                {
                                    self.show_prefs = !self.show_prefs;
                                    self.hotkey_draft = self.cfg.hotkey.clone();
                                    self.mode_hotkeys_draft = self.cfg.mode_hotkeys.clone();
                                }
                                if let UpdateState::Available(release) = &self.update_state {
                                    let label = tf!("Versão {}", "Version {}", release.version);
                                    if kit::pill_toggle(ui, Some(Icon::UpdateArrow), &label, true)
                                        .on_hover_text(t!("Há uma versão nova. Abre Preferências > Atualizações.", "A new version is out. Opens Preferences > Updates."))
                                        .clicked()
                                    {
                                        self.show_prefs = true;
                                    }
                                }
                                if kit::ghost_button(
                                    ui,
                                    Some(Icon::Library),
                                    t!("Biblioteca", "Library"),
                                    self.show_library,
                                )
                                .clicked()
                                {
                                    self.show_library = !self.show_library;
                                }
                                kit::divider_v(ui, 18.0);
                                if let Some(ve) = &self.video_editor
                                    && self.editor.is_none()
                                {
                                    if kit::ghost_button(
                                        ui,
                                        Some(Icon::ExternalOpen),
                                        t!("Player", "Player"),
                                        false,
                                    )
                                    .on_hover_text(if cfg!(target_os = "macos") {
                                        t!("Abre o vídeo no reprodutor padrão do macOS.", "Opens the video in the default macOS player.")
                                    } else {
                                        t!(
                                            "Abre o vídeo no reprodutor padrão do Windows.",
                                            "Opens the video in the default Windows media player."
                                        )
                                    })
                                    .clicked()
                                    {
                                        platform::open(ve.path());
                                    }
                                    let can_grab = !self.video_job_running && ve.current_frame().is_some();
                                    if ui
                                        .add_enabled_ui(can_grab, |ui| {
                                            kit::ghost_button(
                                                ui,
                                                Some(Icon::CaptureFrame),
                                                t!("Quadro → Editor", "Frame → Editor"),
                                                false,
                                            )
                                        })
                                        .inner
                                        .on_hover_text(t!(
                                            "Envia o quadro atual do vídeo direto para o editor de anotações.",
                                            "Sends the current video frame straight to the annotation editor."
                                        ))
                                        .clicked()
                                        && let Some(frame) = ve.current_frame().cloned()
                                    {
                                        video_action = Some(video_editor::VideoAction::CaptureFrame { image: frame });
                                    }
                                } else {
                                    let has_capture = self.editor.is_some();
                                    let can_ocr = has_capture && !self.ocr_running && !self.redact_running;
                                    if ui
                                        .add_enabled_ui(can_ocr, |ui| {
                                            kit::ghost_button(
                                                ui,
                                                Some(Icon::AutoRedact),
                                                t!("Ocultar dados", "Auto-redact"),
                                                false,
                                            )
                                        })
                                        .inner
                                        .on_hover_text(t!(
                                            "Detecta e desfoca automaticamente e-mails, senhas, chaves, CPFs, cartões e telefones via OCR local.",
                                            "Automatically detects and blurs emails, passwords, keys, IDs, cards, and phones via local OCR."
                                        ))
                                        .clicked()
                                        && let Some(e) = &mut self.editor
                                    {
                                        e.commit_pending_text();
                                        let image = e.image().clone();
                                        self.start_redact(&ctx, image);
                                    }
                                    if ui
                                        .add_enabled_ui(can_ocr, |ui| {
                                            kit::ghost_button(
                                                ui,
                                                Some(Icon::ReadText),
                                                t!("Ler texto", "Read text"),
                                                false,
                                            )
                                        })
                                        .inner
                                        .on_hover_text(t!(
                                            "Reconhece todo o texto da imagem e copia. Para uma área só, use a ferramenta Extrair texto.",
                                            "Recognizes all the text in the image and copies it. For a single area, use the Grab text tool."
                                        ))
                                        .clicked()
                                        && let Some(e) = &mut self.editor
                                    {
                                        e.commit_pending_text();
                                        let image = e.image().clone();
                                        self.start_ocr(&ctx, image, ocr::Mode::Printed);
                                    }
                                    let acc = self.accent();
                                    if ui
                                        .add_enabled_ui(has_capture, |ui| {
                                            kit::ghost_button(ui, Some(Icon::Pin), t!("Fixar", "Pin"), false)
                                        })
                                        .inner
                                        .on_hover_text(t!(
                                            "Fixa a captura na tela sempre visível.",
                                            "Pins the capture on top of the screen."
                                        ))
                                        .clicked()
                                        && let Some(e) = &mut self.editor
                                    {
                                        e.commit_pending_text();
                                        let img = e.image().clone();
                                        self.status = match pin::spawn(img, [acc.r(), acc.g(), acc.b()], None) {
                                            Ok(()) => t!(
                                                "Captura fixada na tela (arraste para mover, Esc fecha).",
                                                "Capture pinned to screen (drag to move, Esc closes)."
                                            )
                                            .to_owned(),
                                            Err(err) => tf!(
                                                "Não foi possível fixar na tela: {err}",
                                                "Could not pin to screen: {err}"
                                            ),
                                        };
                                    }
                                    if ui
                                        .add_enabled_ui(has_capture, |ui| {
                                            kit::ghost_button(ui, Some(Icon::Save), t!("Salvar…", "Save…"), false)
                                        })
                                        .inner
                                        .on_hover_text(tf!(
                                            "Salva em PNG, JPEG, WebP, PDF ou BMP ({})",
                                            "Saves as PNG, JPEG, WebP, PDF, or BMP ({})",
                                            cmd_key("S")
                                        ))
                                        .clicked()
                                        && let Some(e) = &mut self.editor
                                    {
                                        e.commit_pending_text();
                                        self.status = export::save_capture_dialog(e.image());
                                    }
                                    if ui
                                        .add_enabled_ui(has_capture, |ui| {
                                            kit::ghost_button(
                                                ui,
                                                Some(Icon::Copy),
                                                t!("Copiar", "Copy"),
                                                has_capture,
                                            )
                                        })
                                        .inner
                                        .on_hover_text(cmd_key("C"))
                                        .clicked()
                                        && let Some(e) = &mut self.editor
                                    {
                                        e.commit_pending_text();
                                        self.status = match copy_to_clipboard(e.image()) {
                                            Ok(()) => t!("Copiado para a área de transferência.", "Copied to the clipboard.").to_owned(),
                                            Err(e) => tf!("Não foi possível copiar: {e}", "Could not copy: {e}"),
                                        };
                                    }
                                }
                            });
                        });
                    });

                // Second row: distinct properties strip for the active image tool or video editor.
                if self.editor.is_some() || self.video_editor.is_some() {
                    let top_r = ui.max_rect();
                    ui.painter().line_segment(
                        [Pos2::new(top_r.min.x, ui.cursor().top()), Pos2::new(top_r.max.x, ui.cursor().top())],
                        Stroke::new(1.0, pal.border_subtle),
                    );
                    egui::Frame::NONE
                        .fill(pal.bg_surface)
                        .inner_margin(Margin::symmetric(10, 5))
                        .show(ui, |ui| {
                            if let Some(editor) = &mut self.editor {
                                styles_changed = editor.options_ui(ui, &mut self.cfg.styles);
                            } else if let Some(ve) = &mut self.video_editor
                                && let Some(act) = ve.options_ui(ui)
                            {
                                video_action = Some(act);
                            }
                        });
                }
                let bot_r = ui.max_rect();
                ui.painter().line_segment(
                    [bot_r.left_bottom(), bot_r.right_bottom()],
                    Stroke::new(1.0, pal.border_subtle),
                );
            });
        if styles_changed {
            self.save_cfg();
        }
        egui::Panel::bottom("status")
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(pal.bg_chrome).inner_margin(Margin::symmetric(10, 4)))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().line_segment(
                    [r.left_top(), r.right_top()],
                    Stroke::new(1.0, pal.border_subtle),
                );
                ui.horizontal(|ui| {
                    if self.ocr_running || self.redact_running || self.video_job_running {
                        ui.spinner();
                    } else if !self.status.is_empty() {
                        let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 14.0), Sense::hover());
                        ui.painter().circle_filled(dot_rect.center(), 3.0, pal.accent);
                    }
                    ui.label(
                        egui::RichText::new(&self.status)
                            .size(12.0)
                            .color(pal.text_secondary),
                    );
                    if let Some(editor) = &mut self.editor {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| editor.zoom_ui(ui));
                    } else if let Some(ve) = &self.video_editor {
                        let info = ve.info();
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(tf!(
                                    "{}×{} px  •  {:.1} s  •  Espaço reproduz, [ e ] marcam corte",
                                    "{}×{} px  •  {:.1} s  •  Space plays, [ and ] mark trim",
                                    info.width,
                                    info.height,
                                    info.duration
                                ))
                                .size(11.5)
                                .color(pal.text_muted),
                            );
                        });
                    }
                });
            });
        if let Some(editor) = &mut self.editor {
            egui::Panel::left("tools")
                .resizable(false)
                .exact_size(if self.tools_expanded { 156.0 } else { 50.0 })
                .show_separator_line(false)
                .frame(
                    egui::Frame::NONE
                        .fill(pal.bg_chrome)
                        .inner_margin(Margin::symmetric(if self.tools_expanded { 8 } else { 6 }, 6)),
                )
                .show(ui, |ui| {
                    let r = ui.max_rect();
                    ui.painter().line_segment(
                        [r.right_top(), r.right_bottom()],
                        Stroke::new(1.0, pal.border_subtle),
                    );
                    editor.tools_ui(ui, &mut self.tools_expanded);
                });
        }
        if self.show_library {
            egui::Panel::right("library")
                .resizable(true)
                .default_size(256.0)
                .size_range(210.0..=420.0)
                .show_separator_line(false)
                .frame(egui::Frame::NONE.fill(pal.bg_chrome).inner_margin(Margin::symmetric(10, 8)))
                .show(ui, |ui| {
                    let r = ui.max_rect();
                    ui.painter().line_segment(
                        [r.left_top(), r.left_bottom()],
                        Stroke::new(1.0, pal.border_subtle),
                    );
                    self.library_ui(ui);
                });
        }
        self.autosave(&ctx);
        if let Some(area) = self.editor.as_mut().and_then(|e| e.take_text_request()) {
            self.start_ocr(&ctx, area, ocr::Mode::Printed);
        }
        if self.editor.as_mut().is_some_and(|e| e.take_redact_request())
            && let Some(e) = &mut self.editor
        {
            let image = e.image().clone();
            self.start_redact(&ctx, image);
        }
        self.ocr_ui(&ctx);
        self.poll_redact();
        self.poll_video_jobs(&ctx);
        self.edit_meta_ui(&ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(pal.bg_canvas))
            .show(ui, |ui| {
            if let Some(editor) = &mut self.editor {
                editor.canvas_ui(ui);
            } else if let Some(ve) = &mut self.video_editor {
                if let Some(act) = ve.canvas_ui(ui) {
                    video_action = Some(act);
                }
            } else {
                self.empty_state_ui(ui, &ctx);
            }
        });
        if let Some(action) = video_action {
            self.handle_video_action(&ctx, action);
        }
        if self.show_prefs {
            self.prefs_ui(&ctx);
        }
        if let Some(e) = &mut self.editor
            && !e.is_typing()
            && !ctx.egui_wants_keyboard_input()
        {
            let (save_sc, copy_sc) = ctx.input_mut(|i| {
                let save = i.consume_key(egui::Modifiers::COMMAND, egui::Key::S);
                let copy_ev = i.events.iter().any(|ev| matches!(ev, egui::Event::Copy));
                let copy = i.consume_key(egui::Modifiers::COMMAND, egui::Key::C) || copy_ev;
                (save, copy)
            });
            if save_sc {
                e.commit_pending_text();
                self.status = export::save_capture_dialog(e.image());
            } else if copy_sc {
                e.commit_pending_text();
                self.status = match copy_to_clipboard(e.image()) {
                    Ok(()) => t!(
                        "Copiado para a área de transferência.",
                        "Copied to the clipboard."
                    )
                    .to_owned(),
                    Err(err) => tf!("Não foi possível copiar: {err}", "Could not copy: {err}"),
                };
            }
        } else if self.editor.is_none()
            && self.video_editor.is_none()
            && !ctx.egui_wants_keyboard_input()
        {
            let paste = ctx.input_mut(|i| {
                let paste_ev = i.events.iter().any(|ev| matches!(ev, egui::Event::Paste(_)));
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::V) || paste_ev
            });
            if paste {
                self.open_from_clipboard(&ctx);
            }
        }
    }

    /// Quick-start empty state shown when no capture or video is currently open in the workspace.
    fn empty_state_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let avail = ui.available_size();
        let pal = theme::current_palette(ctx);
        let accent = self.accent();
        let logo = self
            .logo_tex
            .get_or_insert_with(|| {
                let rgba = tray::icon_rgba(96);
                let img = ColorImage::from_rgba_unmultiplied([96, 96], &rgba);
                ctx.load_texture("snapkit_hero_logo", img, TextureOptions::LINEAR)
            })
            .clone();

        ui.vertical_centered(|ui| {
            let top_pad = ((avail.y - 340.0) * 0.42).max(20.0);
            ui.add_space(top_pad);

            let (logo_rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), Sense::hover());
            ui.painter().add(
                egui::Shadow {
                    offset: [0, 6],
                    blur: 16,
                    spread: 0,
                    color: pal.shadow_color,
                }
                .as_shape(logo_rect, 10.0),
            );
            ui.painter().image(
                logo.id(),
                logo_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            ui.add_space(12.0);

            ui.label(
                kit::semibold(t!(
                    "Pronto para capturar ou gravar",
                    "Ready to capture or record"
                ))
                .size(20.0)
                .color(pal.text_primary),
            );
            ui.add_space(5.0);

            // Subtitle with styled keycap badge for the primary shortcut
            let sub_prefix = t!(
                "Escolha um modo abaixo, use a barra superior ou pressione",
                "Pick a mode below, use the top bar, or press"
            );
            let hk_label = self.cfg.hotkey.label();
            let row_w = ui.available_width();
            let (sub_rect, _) = ui.allocate_exact_size(egui::vec2(row_w, 22.0), Sense::hover());
            let sub_painter = ui.painter_at(sub_rect);
            let sub_galley = sub_painter.layout_no_wrap(
                sub_prefix.to_owned(),
                egui::FontId::proportional(13.0),
                pal.text_secondary,
            );
            let kbd_galley = sub_painter.layout_no_wrap(
                hk_label.clone(),
                egui::FontId::proportional(10.5),
                pal.text_secondary,
            );
            let kbd_w = kbd_galley.size().x + 10.0;
            let total_w = sub_galley.size().x + 6.0 + kbd_w;
            let start_x = sub_rect.center().x - total_w * 0.5;
            sub_painter.galley(
                Pos2::new(start_x, sub_rect.center().y - sub_galley.size().y * 0.5),
                sub_galley,
                pal.text_secondary,
            );
            kit::kbd_badge(
                &sub_painter,
                Pos2::new(start_x + total_w, sub_rect.center().y),
                egui::Align2::RIGHT_CENTER,
                &hk_label,
                &pal,
            );

            ui.add_space(20.0);

            let cards: [(CaptureKind, Icon, &str, &str, Option<String>); 6] = [
                (
                    CaptureKind::Region,
                    Icon::Region,
                    t!("Região", "Region"),
                    t!(
                        "Selecione uma área ou janela e anote na hora",
                        "Select an area or window and annotate right away"
                    ),
                    Some(self.cfg.hotkey.label()),
                ),
                (
                    CaptureKind::FullScreen,
                    Icon::FullScreen,
                    t!("Tela cheia", "Full screen"),
                    t!(
                        "Captura imediata do monitor atual",
                        "Instant capture of the current monitor"
                    ),
                    self.cfg
                        .mode_hotkeys
                        .get(config::CaptureShortcutMode::FullScreen)
                        .map(|h| h.label()),
                ),
                (
                    CaptureKind::Scroll,
                    Icon::Scrolling,
                    t!("Rolagem", "Scrolling"),
                    t!(
                        "Página inteira costurada automaticamente",
                        "Full scrolling page stitched automatically"
                    ),
                    self.cfg
                        .mode_hotkeys
                        .get(config::CaptureShortcutMode::Scrolling)
                        .map(|h| h.label()),
                ),
                (
                    CaptureKind::Video,
                    Icon::Video,
                    t!("Vídeo MP4", "MP4 Video"),
                    t!(
                        "Grave com som, corte na linha do tempo e recorte",
                        "Record with sound, trim on the timeline, and crop"
                    ),
                    self.cfg
                        .mode_hotkeys
                        .get(config::CaptureShortcutMode::Video)
                        .map(|h| h.label()),
                ),
                (
                    CaptureKind::Gif,
                    Icon::Gif,
                    "GIF",
                    t!(
                        "Animação leve pronta para compartilhar",
                        "Lightweight animation ready to share"
                    ),
                    None,
                ),
                (
                    CaptureKind::Steps,
                    Icon::Steps,
                    t!("Passo a passo", "Step by step"),
                    t!(
                        "Cada clique vira um passo numerado no guia",
                        "Each click becomes a numbered step in a guide"
                    ),
                    self.cfg
                        .mode_hotkeys
                        .get(config::CaptureShortcutMode::Steps)
                        .map(|h| h.label()),
                ),
            ];

            let card_w = 204.0;
            let card_h = 84.0;
            let gap = 12.0;
            for row in cards.chunks(3) {
                let row_w = row.len() as f32 * card_w + (row.len().saturating_sub(1)) as f32 * gap;
                ui.horizontal(|ui| {
                    let left_pad = ((ui.available_width() - row_w) * 0.5).max(0.0);
                    ui.add_space(left_pad);
                    ui.spacing_mut().item_spacing.x = gap;
                    for (kind, icon, title, subtitle, shortcut) in row {
                        let (rect, resp) = ui.allocate_exact_size(
                            egui::vec2(card_w, card_h),
                            Sense::click(),
                        );
                        let hover_t = ctx.animate_bool_responsive(resp.id.with("hero_card"), resp.hovered());
                        if resp.hovered() {
                            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * hover_t).round() as u8;
                        let bg = Color32::from_rgb(
                            mix(pal.bg_surface.r(), pal.bg_elevated.r()),
                            mix(pal.bg_surface.g(), pal.bg_elevated.g()),
                            mix(pal.bg_surface.b(), pal.bg_elevated.b()),
                        );
                        let border = if hover_t > 0.05 {
                            Stroke::new(
                                1.0 + 0.5 * hover_t,
                                Color32::from_rgba_unmultiplied(
                                    mix(pal.border_subtle.r(), accent.r()),
                                    mix(pal.border_subtle.g(), accent.g()),
                                    mix(pal.border_subtle.b(), accent.b()),
                                    mix(pal.border_subtle.a(), 255),
                                ),
                            )
                        } else {
                            Stroke::new(1.0, pal.border_subtle)
                        };
                        let painter = ui.painter_at(rect);
                        painter.rect_filled(rect, CornerRadius::same(10), bg);
                        painter.rect_stroke(rect, CornerRadius::same(10), border, StrokeKind::Inside);

                        let inner = rect.shrink2(egui::vec2(12.0, 11.0));
                        let badge_rect = Rect::from_min_size(inner.left_top(), egui::vec2(32.0, 32.0));
                        painter.rect_filled(badge_rect, CornerRadius::same(8), accent.gamma_multiply(0.15));
                        icons::paint(&painter, ctx, *icon, badge_rect.center(), 17.0, accent);

                        if let Some(hk) = shortcut {
                            kit::kbd_badge(&painter, inner.right_top(), egui::Align2::RIGHT_TOP, hk, &pal);
                        }

                        let text_x = inner.left() + 42.0;
                        let text_w = (inner.right() - text_x).max(40.0);
                        painter.text(
                            Pos2::new(text_x, inner.top() + 1.0),
                            egui::Align2::LEFT_TOP,
                            *title,
                            egui::FontId::new(13.5, theme::semibold_family()),
                            pal.text_primary,
                        );
                        let sub_galley = painter.layout(
                            (*subtitle).to_owned(),
                            egui::FontId::proportional(11.5),
                            pal.text_secondary,
                            text_w,
                        );
                        painter.galley(
                            Pos2::new(text_x, inner.top() + 21.0),
                            sub_galley,
                            pal.text_secondary,
                        );

                        if resp.clicked() {
                            start_capture(*kind, self.shared.clone(), ctx.clone());
                        }
                    }
                });
                ui.add_space(gap);
            }
            ui.add_space(6.0);
            if kit::pill_toggle(
                ui,
                Some(Icon::Clipboard),
                &tf!(
                    "Abrir imagem da área de transferência ({})",
                    "Open image from clipboard ({})",
                    cmd_key("V")
                ),
                false,
            )
            .clicked()
            {
                self.open_from_clipboard(ctx);
            }
        });
    }

    fn open_from_clipboard(&mut self, ctx: &egui::Context) {
        match arboard::Clipboard::new().and_then(|mut c| c.get_image()) {
            Ok(img) => {
                if let Some(rgba) =
                    RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
                {
                    self.set_captured(
                        ctx,
                        rgba,
                        t!("Área de transferência", "Clipboard").to_owned(),
                    );
                } else {
                    self.status = t!(
                        "A imagem da área de transferência é inválida.",
                        "The clipboard image is invalid."
                    )
                    .to_owned();
                }
            }
            Err(_) => {
                self.status = t!(
                    "Nenhuma imagem encontrada na área de transferência.",
                    "No image found in the clipboard."
                )
                .to_owned();
            }
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        // During the selection the window is a transparent layer over the screen.
        if self.selecting.is_some() {
            return [0.0; 4];
        }
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.shared.quit_requested.swap(false, Ordering::SeqCst)
            || INSTANCE.get().is_some_and(|i| i.take_quit_request())
        {
            self.shutdown(ctx);
            std::process::exit(0);
        }
        if self.shared.open_requested.swap(false, Ordering::SeqCst) {
            self.hide_after_start = 0;
            self.settle = None;
            self.shared.cloaked.store(false, Ordering::SeqCst);
            platform::cloak(self.shared.hwnd, false);
        }
        if self.hide_after_start > 0 {
            self.hide_after_start -= 1;
            if self.hide_after_start == 0 {
                self.put_away();
                self.shared.cloaked.store(false, Ordering::SeqCst);
                platform::cloak(self.shared.hwnd, false);
            } else {
                ctx.request_repaint();
            }
        }
        if let Some(e) = self.writer.take_error() {
            self.status = tf!("Não foi possível salvar na biblioteca: {e}", "Could not save to the library: {e}");
        }
        self.update_logic(ctx);
        if let Some((kind, title, was_on_screen)) = self.shared.select_request.lock().unwrap().take() {
            let hwnd = self.shared.hwnd;
            let windows = platform::visible_windows(hwnd);
            self.selecting = Some(Selecting { kind, windows, title, drag_origin: None, drawn: None, adjust: None, was_on_screen, quick: None, started: Instant::now(), ready_frames: 0, area: platform::selection_area(), revealed: false });
            self.hide_after_start = 0;
            self.restore_frames = 0;
            self.settle = None;
            // Excluded from captures: the shot of the chosen area comes out without our layer on
            // top.
            platform::exclude_from_capture(hwnd, true);
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
        let Some((_, shot)) = self.shared.pending.lock().unwrap().take() else {
            return;
        };
        self.hide_after_start = 0;
        self.settle = None;
        self.shared.cloaked.store(false, Ordering::SeqCst);
        platform::cloak(self.shared.hwnd, false);
        match shot {
            Err(e) => self.status = e,
            Ok(shot) => {
                let note = shot.note;
                self.set_captured(ctx, shot.image, shot.title);
                if !note.is_empty() {
                    self.status = tf!("Captura com rolagem: {note}. {}", "Scrolling capture: {note}. {}", self.status);
                }
            }
        }
        self.shared.busy.store(false, Ordering::SeqCst);
        ctx.request_repaint();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            let ctx = ui.ctx().clone();
            if self.shared.quit_requested.load(Ordering::SeqCst) {
                self.shutdown(&ctx);
            } else if self.selecting.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.status = t!("Captura cancelada.", "Capture canceled.").to_owned();
                self.dismiss_selection(&ctx);
            } else if self.pano.is_some() || self.video.is_some() || self.steps.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                if let Some(VideoUi {
                    title,
                    recorder: Some(session),
                    as_gif,
                    ..
                }) = self.video.take()
                    && let Ok(path) = session.stop()
                {
                    if as_gif {
                        let _ = std::fs::remove_file(&path);
                    } else {
                        self.adopt_video(&ctx, path, title);
                    }
                }
                self.leave_pano_hud(&ctx);
            } else if self.cfg.close_to_tray && self.tray.is_some() && self.shared.hwnd != 0 {
                // Closing with the X only hides the window when there's a tray to bring it back.
                self.flush_autosave(&ctx);
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                platform::hide(self.shared.hwnd);
            } else {
                self.shutdown(&ctx);
            }
        }
        self.settle_step(ui.ctx());
        if self.selecting.is_some() {
            self.selection_ui(ui);
        } else if self.pano.is_some() {
            self.pano_ui(ui);
        } else if self.video.is_some() {
            self.video_ui(ui);
        } else if self.steps.is_some() {
            self.steps_ui(ui);
        } else {
            // Remember the window's size and position in normal use.
            let ctx = ui.ctx().clone();
            let fullscreen = ctx.input(|i| i.viewport().fullscreen).unwrap_or(false);
            if self.restore_frames > 0 {
                // Leaving the selection: the window is still coming back from the size of every
                // monitor.
                if platform::is_on_screen(self.shared.hwnd) {
                    if let Some(r) = self.normal_rect
                        && platform::window_rect(self.shared.hwnd) != Some(r)
                    {
                        platform::set_window_rect(self.shared.hwnd, r);
                    }
                    self.restore_frames -= 1;
                    ctx.request_repaint();
                }
            } else if !fullscreen
                && platform::is_on_screen(self.shared.hwnd)
                && let Some(size) = ctx.input(|i| i.viewport().inner_rect).map(|r| r.size())
                && size.x >= NORMAL_MIN_SIZE.x
                && size.y >= NORMAL_MIN_SIZE.y
            {
                self.normal_size = Some(size);
                self.normal_pos = platform::window_pos(self.shared.hwnd);
                self.normal_rect = platform::window_rect(self.shared.hwnd);
            }
            if self.save_requested && self.restore_frames == 0 {
                self.save_requested = false;
                if let Some(e) = &mut self.editor {
                    e.commit_pending_text();
                    self.status = export::save_capture_dialog(e.image());
                }
            }
            self.home_ui(ui);
        }
        #[cfg(target_os = "macos")]
        self.sync_dock_icon();
    }

    /// macOS: Quit in the app menu (⌘Q) ends the event loop without a close request, and the
    /// process exits right after; what closing does (finish a recording, save the open capture)
    /// still has to happen.
    #[cfg(target_os = "macos")]
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if !self.shut_down {
            let ctx = self.egui_ctx.clone();
            self.shutdown(&ctx);
        }
    }
}

/// Listens for requests from a second copy of SnapKit (opened from the Start menu while this one
/// is in the tray, for example) and carries them out here.
fn spawn_instance_listener(shared: Arc<Shared>, ctx: egui::Context) {
    let Some(instance) = INSTANCE.get() else { return };
    std::thread::spawn(move || {
        while let Some(request) = instance.wait() {
            match request {
                platform::InstanceRequest::Show => {
                    shared.open_requested.store(true, Ordering::SeqCst);
                    platform::show(shared.hwnd);
                    ctx.request_repaint();
                }
                platform::InstanceRequest::Select => start_capture(CaptureKind::Region, shared.clone(), ctx.clone()),
                platform::InstanceRequest::Quit => {
                    shared.quit_requested.store(true, Ordering::SeqCst);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    ctx.request_repaint();
                }
            }
        }
    });
}

/// Runs a background job. A panic becomes an error, so the job still reports back: otherwise
/// SnapKit would stay "busy" (no more captures from the shortcut, or Read text disabled) until
/// it's restarted. The panic itself is written to `error.log` by the hook set in `main`.
fn guarded<T>(job: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
        .unwrap_or_else(|_| Err(tf!("erro interno (detalhes em {ERROR_LOG})", "internal error (details in {ERROR_LOG})")))
}

const ALL_SHORTCUT_MODES: [config::CaptureShortcutMode; 6] = [
    config::CaptureShortcutMode::FullScreen,
    config::CaptureShortcutMode::AllScreens,
    config::CaptureShortcutMode::Scrolling,
    config::CaptureShortcutMode::Panoramic,
    config::CaptureShortcutMode::Steps,
    config::CaptureShortcutMode::Video,
];

fn capture_kind_for_mode(mode: config::CaptureShortcutMode) -> CaptureKind {
    match mode {
        config::CaptureShortcutMode::FullScreen => CaptureKind::FullScreen,
        config::CaptureShortcutMode::AllScreens => CaptureKind::AllScreens,
        config::CaptureShortcutMode::Scrolling => CaptureKind::Scroll,
        config::CaptureShortcutMode::Panoramic => CaptureKind::Panoramic,
        config::CaptureShortcutMode::Steps => CaptureKind::Steps,
        config::CaptureShortcutMode::Video => CaptureKind::Video,
    }
}

fn mode_hotkey_label(mode: config::CaptureShortcutMode) -> &'static str {
    match mode {
        config::CaptureShortcutMode::FullScreen => t!("Tela inteira", "Full screen"),
        config::CaptureShortcutMode::AllScreens => t!("Todas as telas", "All screens"),
        config::CaptureShortcutMode::Scrolling => t!("Rolagem", "Scrolling"),
        config::CaptureShortcutMode::Panoramic => t!("Panorâmica", "Panoramic"),
        config::CaptureShortcutMode::Steps => t!("Passo a passo", "Step by step"),
        config::CaptureShortcutMode::Video => t!("Vídeo", "Video"),
    }
}

fn hotkey_editor_row(ui: &mut egui::Ui, id_salt: impl std::hash::Hash + std::fmt::Debug, hk: &mut HotkeyCfg) {
    // Windows: Ctrl, Alt, Shift, Win. macOS: Control, Option, Shift, Command.
    let [ctrl, alt, shift, win] = config::MODIFIER_LONG_NAMES;
    ui.checkbox(&mut hk.ctrl, ctrl);
    ui.checkbox(&mut hk.alt, alt);
    ui.checkbox(&mut hk.shift, shift);
    ui.checkbox(&mut hk.win, win);
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(hk.key.clone())
        .width(100.0)
        .show_ui(ui, |ui| {
            for key in config::keys() {
                ui.selectable_value(&mut hk.key, key.to_owned(), key);
            }
        });
}

/// Listens for global hotkeys in the background and triggers the corresponding capture mode.
fn spawn_hotkey_listener(shared: Arc<Shared>, ctx: egui::Context) {
    std::thread::spawn(move || {
        while let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
            if event.state != HotKeyState::Pressed {
                continue;
            }
            let kind = shared
                .hotkey_map
                .lock()
                .ok()
                .and_then(|map| map.iter().find(|(id, _)| *id == event.id).map(|(_, kind)| *kind));
            if let Some(kind) = kind {
                start_capture(kind, shared.clone(), ctx.clone());
            }
        }
    });
}

/// True on continuous integration servers, where there's no `vendor\` and no sound: the tests that
/// use the real Tesseract, neural models or FFmpeg skip themselves.
#[cfg(test)]
pub fn in_ci() -> bool {
    std::env::var_os("CI").is_some()
}

fn delay_label(secs: u32) -> String {
    if secs == 0 { t!("Sem atraso", "No delay").to_owned() } else { tf!("Atraso {secs} s", "Delay {secs} s") }
}

fn format_duration_short(secs: f32) -> String {
    let total = secs.max(0.0).round() as u32;
    format!("{:02}:{:02}", total / 60, total % 60)
}

fn copy_to_clipboard(img: &RgbaImage) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard
        .set_image(arboard::ImageData {
            width: img.width() as usize,
            height: img.height() as usize,
            bytes: std::borrow::Cow::Borrowed(img.as_raw()),
        })
        .map_err(|e| e.to_string())
}

fn copy_text(text: &str) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(text.to_owned()))
        .map_err(|e| e.to_string())
}

fn main() -> eframe::Result {
    platform::enable_per_monitor_dpi();
    // Self-test mode: `snapkit --test-capture output.png` captures the screen and exits.
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--test-capture" {
        match capture::capture_monitor_at_cursor(0, true) {
            Ok(shot) => {
                let img = shot.image;
                println!("windows: {}", shot.windows.len());
                img.save(&args[2]).expect("failed to save the test PNG");
                println!("ok {}x{}", img.width(), img.height());
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // Without a console, a crash at startup would otherwise just look like "it doesn't open".
    std::panic::set_hook(Box::new(|info| {
        let detail = info.to_string();
        log_error(&detail);
        if std::thread::current().name() == Some("main") {
            fatal_box(&detail);
        }
    }));

    let start_hidden = args.iter().any(|a| a == autostart::MINIMIZED_ARG);
    let key = instance_key();
    if args.iter().any(|a| a == "--quit") {
        platform::Instance::send(&key, platform::InstanceRequest::Quit);
        return Ok(());
    }
    // Only one SnapKit per set of preferences. A second copy (from the Start menu while this one
    // is in the tray, say) would fight over the shortcut and the library index; it passes its
    // request on and leaves.
    match platform::Instance::claim(&key) {
        Some(instance) => {
            let _ = INSTANCE.set(instance);
        }
        None => {
            let request = if args.iter().any(|a| a == "--select") {
                Some(platform::InstanceRequest::Select)
            } else if start_hidden {
                None
            } else {
                Some(platform::InstanceRequest::Show)
            };
            if let Some(request) = request {
                platform::Instance::send(&key, request);
            }
            return Ok(());
        }
    }

    #[cfg(windows)]
    legacy::migrate();
    let icon = egui::IconData { rgba: tray::icon_rgba(64), width: 64, height: 64 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_icon(icon)
            .with_visible(!start_hidden)
            // Region selection uses this same window as a transparent layer over the screen.
            .with_transparent(true)
            .with_inner_size([900.0, 600.0])
            .with_min_inner_size(NORMAL_MIN_SIZE),
        // With the default renderer (wgpu) the transparent window comes out black on Windows.
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let result = eframe::run_native(APP_NAME, options, Box::new(|cc| Ok(Box::new(App::new(cc)))));
    if let Err(e) = &result {
        // Usually the graphics driver: the window needs OpenGL 2 or later.
        let detail = format!("{e}");
        log_error(&detail);
        fatal_box(&detail);
    }
    result
}

/// This copy's claim to be the only SnapKit running (see `main`).
static INSTANCE: OnceLock<platform::Instance> = OnceLock::new();

/// Set once the interface is up: from then on, a fatal error isn't a failure to start.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Identifies the preferences in use (`%APPDATA%`), so that a separate profile, such as the one
/// used for the documentation screenshots, can run alongside. A fixed hash (FNV-1a), the same in
/// every version, so an old and a new SnapKit still recognize each other.
fn instance_key() -> String {
    let dir = platform::profile_id().to_lowercase();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in dir.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Appends a line to `%APPDATA%\SnapKit\error.log`, so a failure can be reported later.
fn log_error(detail: &str) {
    use std::io::Write;
    let Some(dir) = platform::data_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("error.log")) {
        let (y, mo, d, h, mi, s) = platform::local_time();
        let _ = writeln!(f, "{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} SnapKit {}: {detail}", env!("CARGO_PKG_VERSION"));
    }
}

/// Tells the user SnapKit couldn't start, or has to close. In both languages, because the
/// preference may not be loaded yet.
fn fatal_box(detail: &str) {
    let text = if RUNNING.load(Ordering::SeqCst) {
        format!(
            "SnapKit ran into an error and has to close.\n\
             Your captures are in the library. If this keeps happening, please report it with the \
             file below.\n\n\
             O SnapKit encontrou um erro e precisa fechar.\n\
             Suas capturas estão na biblioteca. Se isso se repetir, informe o problema junto com o \
             arquivo abaixo.\n\n\
             {detail}\n\n\
             Details / Detalhes: {ERROR_LOG}"
        )
    } else if cfg!(target_os = "macos") {
        format!(
            "SnapKit could not start.\n\
             If this keeps happening, please report it with the file below.\n\n\
             O SnapKit não conseguiu iniciar.\n\
             Se isso se repetir, informe o problema junto com o arquivo abaixo.\n\n\
             {detail}\n\n\
             Details / Detalhes: {ERROR_LOG}"
        )
    } else {
        format!(
            "SnapKit could not start.\n\
             If this keeps happening, update your graphics driver and install the latest \
             Microsoft Visual C++ Redistributable (x64).\n\n\
             O SnapKit não conseguiu iniciar.\n\
             Se isso se repetir, atualize o driver de vídeo e instale o Microsoft Visual C++ \
             Redistributable (x64) mais recente.\n\n\
             {detail}\n\n\
             Details / Detalhes: {ERROR_LOG}"
        )
    };
    platform::error_box(APP_NAME, &text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_handle_locks_at_minimum_without_moving_fixed_edge() {
        let bounds = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(500.0, 500.0));
        let start = Rect::from_min_max(Pos2::new(100.0, 100.0), Pos2::new(200.0, 200.0));
        // Dragging the West handle (7) past the East edge (200.0) keeps max.x fixed at 200.0.
        let r = apply_handle(start, 7, Pos2::new(250.0, 150.0), bounds);
        assert_eq!(r.max.x, 200.0);
        assert_eq!(r.min.x, 200.0 - MIN_REGION);
        // Dragging the East handle (3) past the West edge (100.0) keeps min.x fixed at 100.0.
        let r = apply_handle(start, 3, Pos2::new(50.0, 150.0), bounds);
        assert_eq!(r.min.x, 100.0);
        assert_eq!(r.max.x, 100.0 + MIN_REGION);
        // Dragging the Northwest handle (0) past the Southeast corner keeps max fixed at (200, 200).
        let r = apply_handle(start, 0, Pos2::new(205.0, 205.0), bounds);
        assert_eq!(r.max, Pos2::new(200.0, 200.0));
        assert_eq!(r.min, Pos2::new(200.0 - MIN_REGION, 200.0 - MIN_REGION));
    }
}
