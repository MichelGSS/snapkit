#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
mod i18n;
mod audio;
mod autostart;
mod capture;
mod config;
mod editor;
mod legacy;
mod library;
mod neural;
mod ocr;
mod record;
mod render;
mod scroll;
mod steps;
mod theme;
mod tray;
mod win;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use image::RgbaImage;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use config::{Config, HotkeyCfg};

const APP_NAME: &str = "SnapKit";

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
    /// Identifier of the global hotkey in effect (0 = none).
    hotkey_id: AtomicU32,
}

/// Triggers a capture. Full screen takes the shot right away; the other modes open the selection,
/// which happens live, over the screen, without freezing anything.
fn start_capture(kind: CaptureKind, shared: Arc<Shared>, ctx: egui::Context) {
    if shared.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        // Read before anything of ours takes the focus.
        let title = win::foreground_window_title();
        let hwnd = shared.hwnd;
        if matches!(kind, CaptureKind::FullScreen | CaptureKind::AllScreens) {
            let delay = shared.delay_secs.load(Ordering::Relaxed);
            let cursor = shared.include_cursor.load(Ordering::Relaxed);
            if hwnd != 0 {
                // Excluded from captures, the window doesn't show in the shot, without having to
                // leave the screen.
                win::exclude_from_capture(hwnd, true);
                if delay > 0 && win::is_on_screen(hwnd) {
                    // With a delay, get out of the way: the user wants to work with what's behind.
                    win::minimize(hwnd);
                }
            }
            // A moment for the system to apply the exclusion before the shot.
            std::thread::sleep(Duration::from_millis(80));
            if delay > 0 {
                std::thread::sleep(Duration::from_secs(delay as u64));
            }
            let shot = if kind == CaptureKind::FullScreen {
                capture::capture_monitor_at_cursor(hwnd, cursor)
            } else {
                let all = win::virtual_screen();
                capture::grab_virtual(all, cursor).map(|image| capture::Shot {
                    image,
                    windows: Vec::new(),
                    title,
                    note: String::new(),
                })
            };
            *shared.pending.lock().unwrap() = Some((CaptureKind::FullScreen, shot));
            if hwnd != 0 {
                win::exclude_from_capture(hwnd, false);
                win::show(hwnd);
            }
        } else {
            let was_on_screen = hwnd != 0 && win::is_on_screen(hwnd);
            *shared.select_request.lock().unwrap() = Some((kind, title, was_on_screen));
            if hwnd != 0 {
                // Invisible while it changes size and loses its frame: without this the editor
                // flashes on the screen before the selection layer is ready.
                win::cloak(hwnd, true);
            }
            // The window becomes the selection layer; if it was in the tray or minimized, it comes
            // back.
            if hwnd != 0 && !was_on_screen {
                win::show(hwnd);
            }
        }
        ctx.request_repaint();
    });
}

/// Selection in progress: the main window, transparent and frameless, covers every monitor.
struct Selecting {
    kind: CaptureKind,
    /// Windows visible when the selection started, in screen coordinates, front to back.
    windows: Vec<win::PxRect>,
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
    /// The layer has already been shown (the window starts invisible; see `win::cloak`).
    revealed: bool,
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
    let mut r = match handle {
        0 => Rect::from_two_pos(to, start.max),                           // northwest
        1 => Rect::from_two_pos(Pos2::new(start.min.x, to.y), start.max), // north
        2 => Rect::from_two_pos(to, Pos2::new(start.min.x, start.max.y)), // northeast
        3 => Rect::from_two_pos(start.min, Pos2::new(to.x, start.max.y)), // east
        4 => Rect::from_two_pos(start.min, to),                           // southeast
        5 => Rect::from_two_pos(Pos2::new(start.max.x, to.y), start.min), // south
        6 => Rect::from_two_pos(to, Pos2::new(start.max.x, start.min.y)), // southwest
        _ => Rect::from_two_pos(start.max, Pos2::new(to.x, start.min.y)), // west
    };
    // Pulled past the minimum: locks the moving edge at the minimum distance from the fixed one.
    if r.width() < MIN_REGION {
        if matches!(handle, 0 | 6 | 7) { r.min.x = r.max.x - MIN_REGION; } else { r.max.x = r.min.x + MIN_REGION; }
    }
    if r.height() < MIN_REGION {
        if matches!(handle, 0 | 1 | 2) { r.min.y = r.max.y - MIN_REGION; } else { r.max.y = r.min.y + MIN_REGION; }
    }
    r
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
    Save,
    Editor,
    Cancel,
}

struct App {
    shared: Arc<Shared>,
    cfg: Config,
    selecting: Option<Selecting>,
    editor: Option<editor::Editor>,
    status: String,
    hotkeys: Option<GlobalHotKeyManager>,
    /// Hotkey currently registered with the system.
    active_hotkey: Option<HotKey>,
    /// Keeps the tray icon alive; `None` if the tray couldn't be created.
    tray: Option<tray::Tray>,
    show_prefs: bool,
    /// Draft of the shortcut in the preferences window, before Apply.
    hotkey_draft: HotkeyCfg,
    autostart: bool,
    library: library::Library,
    writer: library::Writer,
    /// Library file open in the editor, and its revision that has already been saved.
    current_file: Option<String>,
    saved_revision: u64,
    /// Thumbnails already loaded, by file name.
    thumbs: HashMap<String, TextureHandle>,
    show_library: bool,
    search: String,
    /// Panoramic capture being prepared or in progress.
    pano: Option<PanoUi>,
    /// Video recording being prepared or in progress.
    video: Option<VideoUi>,
    /// Step-by-step capture being prepared or in progress.
    steps: Option<StepsUi>,
    /// Last finished recording, shown in a window until the user closes it.
    video_done: Option<VideoDone>,
    /// Result of the GIF conversion, delivered by the thread that does it.
    gif_pending: Arc<Mutex<Option<Result<std::path::PathBuf, String>>>>,
    gif_running: bool,
    /// Result of the OCR in progress, delivered by the recognition thread.
    ocr_pending: Arc<Mutex<Option<Result<(String, ocr::Engine), String>>>>,
    ocr_running: bool,
    /// Extracted text, shown in a window until the user closes it.
    ocr_text: Option<String>,
    /// Last image read, so it can be read again with the other engine.
    ocr_image: Option<RgbaImage>,
    /// Size (in points) and position (in screen pixels) of the window in normal use, to put it back
    /// in place after the panoramic panel.
    normal_size: Option<egui::Vec2>,
    normal_pos: Option<(i32, i32)>,
    /// Rectangle of the window in normal use, in screen pixels, to restore it after the selection.
    normal_rect: Option<win::PxRect>,
    /// Frames still left reapplying `normal_rect` after leaving the selection.
    restore_frames: u8,
    /// The window is invisible, returning from the selection: at the end, it reappears or leaves
    /// the screen.
    settle: Option<Settle>,
    /// Open the save dialog as soon as the main window is back to normal.
    save_requested: bool,
}

/// State of the small floating panel of the panoramic capture.
struct PanoUi {
    rect: win::PxRect,
    title: String,
    session: Option<scroll::PanoramicSession>,
    /// Frame around the area, visible while the panel exists.
    _frame: Option<win::RegionFrame>,
    /// Frames left before positioning the panel (the window is still leaving full screen).
    setup_frames: u8,
}

/// State of the floating panel of the step-by-step capture.
struct StepsUi {
    rect: win::PxRect,
    title: String,
    /// `None` until the user presses Start.
    recorder: Option<steps::Recorder>,
    /// Frame around the area, visible while the panel exists.
    _frame: Option<win::RegionFrame>,
    setup_frames: u8,
}

/// A video just recorded, with the choices from the Recording saved window.
struct VideoDone {
    path: std::path::PathBuf,
    /// Length in seconds (0 if unknown).
    length: f64,
    /// Chosen clip, in seconds.
    start: f64,
    end: f64,
    /// GIF options.
    fps: u32,
    width: u32,
}

impl VideoDone {
    fn new(path: std::path::PathBuf) -> Self {
        let length = record::video_duration(&path).map_or(0.0, |d| d.as_secs_f64());
        Self { path, length, start: 0.0, end: length, fps: 12, width: 720 }
    }
}

struct VideoUi {
    rect: win::PxRect,
    recorder: Option<record::Session>,
    /// A recording that becomes a GIF when it stops.
    as_gif: bool,
    /// Frame around the area, visible while the panel exists.
    _frame: Option<win::RegionFrame>,
    setup_frames: u8,
}

const PANO_HUD_SIZE: egui::Vec2 = egui::vec2(460.0, 84.0);
const STEPS_HUD_SIZE: egui::Vec2 = egui::vec2(620.0, 84.0);
const VIDEO_HUD_SIZE: egui::Vec2 = egui::vec2(560.0, 84.0);

/// Turns the window into a small panel, always on top and excluded from captures, and places it
/// next to the area `rect`. Called every frame until `setup_frames` reaches zero: the window is
/// still leaving full screen and only accepts the position a few frames later.
fn hud_setup(ctx: &egui::Context, hwnd: isize, rect: win::PxRect, size: egui::Vec2, setup_frames: &mut u8) {
    if *setup_frames == 0 {
        return;
    }
    if *setup_frames == 6 {
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(size));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        win::exclude_from_capture(hwnd, true);
    }
    *setup_frames -= 1;
    if *setup_frames == 0 {
        // Right below the area; if it doesn't fit, above; as a last resort, at the bottom of the
        // monitor.
        let scale = ctx.pixels_per_point();
        let (w, h) = ((size.x * scale) as i32, (size.y * scale) as i32 + 48);
        let center = ((rect.0 + rect.2) / 2, (rect.1 + rect.3) / 2);
        let work = win::work_area_at(center).unwrap_or((rect.0, rect.1, rect.2, rect.3 + h + 24));
        let x = (center.0 - w / 2).clamp(work.0, (work.2 - w).max(work.0));
        let y = if rect.3 + 12 + h <= work.3 {
            rect.3 + 12
        } else if rect.1 - 12 - h >= work.1 {
            rect.1 - 12 - h
        } else {
            work.3 - h - 12
        };
        win::set_window_pos(hwnd, (x, y));
    }
    ctx.request_repaint();
}
const NORMAL_MIN_SIZE: egui::Vec2 = egui::vec2(520.0, 320.0);
/// Width that Copy, Save, Read text, Library and Preferences take up together in the bar.
const RIGHT_GROUP_WIDTH: f32 = 500.0;

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let cfg = Config::load();
        i18n::set(cfg.language.unwrap_or_else(i18n::Lang::system));
        theme::apply(&cc.egui_ctx, theme::by_name(&cfg.theme));
        let hwnd = match cc.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
            _ => 0,
        };
        let shared = Arc::new(Shared {
            pending: Mutex::new(None),
            select_request: Mutex::new(None),
            busy: AtomicBool::new(false),
            hwnd,
            delay_secs: AtomicU32::new(cfg.delay_secs),
            include_cursor: AtomicBool::new(cfg.include_cursor),
            hotkey_id: AtomicU32::new(0),
        });

        let hotkeys = GlobalHotKeyManager::new().ok();
        spawn_hotkey_listener(shared.clone(), cc.egui_ctx.clone());

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
                        win::show(shared.hwnd);
                        ctx.request_repaint();
                    }
                    tray::TrayAction::Quit => std::process::exit(0),
                }),
            )
            .ok()
        };

        let mut app = Self {
            shared,
            hotkey_draft: cfg.hotkey.clone(),
            cfg,
            selecting: None,
            editor: None,
            status: String::new(),
            hotkeys,
            active_hotkey: None,
            tray,
            show_prefs: false,
            autostart: autostart::is_enabled(),
            library: library::Library::open(library::Library::default_dir()),
            writer: library::Writer::spawn(),
            current_file: None,
            saved_revision: 0,
            thumbs: HashMap::new(),
            show_library: true,
            search: String::new(),
            pano: None,
            video: None,
            steps: None,
            video_done: None,
            gif_pending: Arc::new(Mutex::new(None)),
            gif_running: false,
            ocr_pending: Arc::new(Mutex::new(None)),
            ocr_running: false,
            ocr_text: None,
            ocr_image: None,
            normal_size: None,
            normal_pos: None,
            normal_rect: None,
            restore_frames: 0,
            settle: None,
            save_requested: false,
        };
        // `snapkit --select` opens straight into region selection (shortcuts and tests).
        if std::env::args().any(|a| a == "--select") {
            start_capture(CaptureKind::Region, app.shared.clone(), cc.egui_ctx.clone());
        }
        app.status = match app.apply_hotkey(&app.cfg.hotkey.clone()) {
            Ok(()) => tf!("Pronto. Atalho global: {}", "Ready. Global shortcut: {}", app.cfg.hotkey.label()),
            Err(e) => tf!("Atalho {} indisponível: {e}", "Shortcut {} unavailable: {e}", app.cfg.hotkey.label()),
        };
        app
    }

    /// Replaces the global hotkey registered with the system. On error, the previous one is kept.
    fn apply_hotkey(&mut self, cfg: &HotkeyCfg) -> Result<(), String> {
        let manager = self.hotkeys.as_ref().ok_or(t!("atalhos globais indisponíveis", "global shortcuts unavailable"))?;
        let new = cfg.to_hotkey().ok_or(t!("tecla desconhecida", "unknown key"))?;
        if self.active_hotkey == Some(new) {
            return Ok(());
        }
        manager.register(new).map_err(|e| e.to_string())?;
        if let Some(old) = self.active_hotkey.replace(new) {
            let _ = manager.unregister(old);
        }
        self.shared.hotkey_id.store(new.id(), Ordering::Relaxed);
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

    fn save_cfg(&mut self) {
        if let Err(e) = self.cfg.save() {
            self.status = tf!("Não foi possível salvar as preferências: {e}", "Could not save the preferences: {e}");
        }
    }

    fn prefs_ui(&mut self, ctx: &egui::Context) {
        let mut open = self.show_prefs;
        egui::Window::new(t!("Preferências", "Preferences"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.strong(t!("Atalho global de captura", "Global capture shortcut"));
                ui.horizontal(|ui| {
                    let d = &mut self.hotkey_draft;
                    ui.checkbox(&mut d.ctrl, "Ctrl");
                    ui.checkbox(&mut d.alt, "Alt");
                    ui.checkbox(&mut d.shift, "Shift");
                    ui.checkbox(&mut d.win, "Win");
                    egui::ComboBox::from_id_salt("key")
                        .selected_text(d.key.clone())
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            for key in config::KEYS {
                                ui.selectable_value(&mut d.key, key.to_owned(), key);
                            }
                        });
                });
                ui.horizontal(|ui| {
                    let changed = self.hotkey_draft != self.cfg.hotkey;
                    if ui.add_enabled(changed, egui::Button::new(t!("Aplicar atalho", "Apply shortcut"))).clicked() {
                        let draft = self.hotkey_draft.clone();
                        if !draft.is_safe() {
                            self.status = tf!(
                                "{} atrapalharia a digitação; inclua Ctrl, Alt ou Win.",
                                "{} would get in the way of typing; include Ctrl, Alt or Win.",
                                draft.label()
                            );
                        } else {
                            match self.apply_hotkey(&draft) {
                                Ok(()) => {
                                    self.status = tf!("Atalho global: {}", "Global shortcut: {}", draft.label());
                                    self.cfg.hotkey = draft;
                                    self.save_cfg();
                                    if let Some(tray) = &self.tray {
                                        tray.relabel(&self.cfg.hotkey.label());
                                    }
                                }
                                Err(e) => {
                                    self.status = tf!(
                                        "Não foi possível usar {}: {e}. Mantido {}.",
                                        "Could not use {}: {e}. Kept {}.",
                                        draft.label(),
                                        self.cfg.hotkey.label()
                                    );
                                }
                            }
                        }
                    }
                    ui.label(tf!("Atual: {}", "Current: {}", self.cfg.hotkey.label()));
                });
                ui.separator();
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
                ui.separator();
                ui.strong(t!("Idioma", "Language"));
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
                ui.separator();
                ui.strong(t!("Tema da interface", "Interface theme"));
                let mut name = self.cfg.theme.clone();
                let theme_combo = egui::ComboBox::from_id_salt("theme")
                    .selected_text(theme::display_name(&name))
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for theme in theme::THEMES.iter() {
                            ui.selectable_value(&mut name, theme.name.to_owned(), theme::display_name(theme.name));
                        }
                    });
                theme_combo.response.on_hover_text(t!(
                    "Muda as cores da interface e do editor, na hora.",
                    "Changes the colors of the interface and the editor right away."
                ));
                if name != self.cfg.theme {
                    self.cfg.theme = name;
                    theme::apply(ctx, self.theme());
                    let accent = self.accent();
                    if let Some(editor) = &mut self.editor {
                        editor.set_accent(accent);
                    }
                    self.save_cfg();
                }
                ui.separator();
                let tray_ok = self.tray.is_some();
                let mut close_to_tray = self.cfg.close_to_tray;
                ui.add_enabled(
                    tray_ok,
                    egui::Checkbox::new(
                        &mut close_to_tray,
                        t!("Fechar a janela mantém o SnapKit na bandeja", "Closing the window keeps SnapKit in the tray"),
                    ),
                );
                if close_to_tray != self.cfg.close_to_tray {
                    self.cfg.close_to_tray = close_to_tray;
                    self.save_cfg();
                }
                let mut autostart = self.autostart;
                ui.checkbox(&mut autostart, t!("Iniciar com o Windows (na bandeja)", "Start with Windows (in the tray)"));
                if autostart != self.autostart {
                    match autostart::set_enabled(autostart) {
                        Ok(()) => self.autostart = autostart,
                        Err(e) => self.status = tf!("Não foi possível alterar a inicialização: {e}", "Could not change the startup setting: {e}"),
                    }
                }
                if !tray_ok {
                    ui.label(t!("O ícone da bandeja não pôde ser criado nesta sessão.", "The tray icon could not be created in this session."));
                }
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
            let result = ocr::recognize(&image, mode);
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
                    copy = ui.button(t!("📋 Copiar", "📋 Copy")).clicked();
                    reread = ui
                        .add_enabled(can_reread, egui::Button::new(t!("✍ Ler como manuscrito", "✍ Read as handwriting")))
                        .on_hover_text(t!(
                            "Relê a mesma área com o motor neural. Melhor para letra manuscrita ou \
                             decorativa; pior para código (perde maiúsculas).",
                            "Reads the same area again with the neural engine. Better for handwriting or \
                             decorative lettering; worse for code (loses capital letters)."
                        ))
                        .clicked();
                    ui.label(t!("Você pode corrigir o texto antes de copiar.", "You can fix the text before copying."));
                });
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
        if reread {
            if let Some(image) = self.ocr_image.clone() {
                self.start_ocr(ctx, image, ocr::Mode::Handwriting);
            }
        }
        if !open {
            self.ocr_text = None;
            self.ocr_image = None;
        }
    }

    /// Saves the editor's current image to the library, if it changed since the last save. Waits
    /// for the edit to settle so the PNG isn't rewritten on every stroke.
    fn autosave(&mut self, ctx: &egui::Context) {
        let (Some(editor), Some(file)) = (&self.editor, &self.current_file) else { return };
        if editor.revision() == self.saved_revision {
            return;
        }
        let settle = Duration::from_millis(900);
        let idle = editor.idle_time();
        if idle < settle {
            ctx.request_repaint_after(settle - idle);
            return;
        }
        let image = editor.image().clone();
        let thumb = library::make_thumb(&image);
        let texture = ctx.load_texture("thumbnail", to_color_image(&thumb), TextureOptions::LINEAR);
        self.thumbs.insert(file.clone(), texture);
        self.library.set_size(file, image.width(), image.height());
        self.writer.save(&self.library, file, image, thumb);
        self.saved_revision = editor.revision();
    }

    /// Opens a capture kept in the library in the editor.
    fn open_from_library(&mut self, ctx: &egui::Context, file: &str) {
        match image::open(self.library.path(file)) {
            Ok(img) => {
                let editor = editor::Editor::new(ctx, img.to_rgba8(), self.accent());
                self.saved_revision = editor.revision();
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
                self.library.forget(file);
                self.thumbs.remove(file);
                if self.current_file.as_deref() == Some(file) {
                    self.current_file = None;
                    self.editor = None;
                }
                self.status = tf!("{file} foi para a Lixeira.", "{file} was moved to the Recycle Bin.");
            }
            Err(e) => self.status = tf!("Não foi possível apagar {file}: {e}", "Could not delete {file}: {e}"),
        }
    }

    fn thumb_texture(&mut self, ctx: &egui::Context, file: &str) -> Option<TextureHandle> {
        if let Some(t) = self.thumbs.get(file) {
            return Some(t.clone());
        }
        // Missing thumbnail (old library or deleted file): rebuilds it from the original.
        let thumb = image::open(self.library.thumb_path(file))
            .map(|i| i.to_rgba8())
            .or_else(|_| image::open(self.library.path(file)).map(|i| library::make_thumb(&i.to_rgba8())))
            .ok()?;
        let texture = ctx.load_texture("thumbnail", to_color_image(&thumb), TextureOptions::LINEAR);
        self.thumbs.insert(file.to_owned(), texture.clone());
        Some(texture)
    }

    fn library_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.add_space(6.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text(t!("Buscar por janela ou data", "Search by window or date"))
                .desired_width(f32::INFINITY),
        );
        let entries: Vec<library::Entry> = self.library.search(&self.search).into_iter().cloned().collect();
        ui.horizontal(|ui| {
            ui.label(tf!("{} de {} capturas", "{} of {} captures", entries.len(), self.library.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button(t!("Abrir pasta", "Open folder")).clicked() {
                    let _ = std::fs::create_dir_all(self.library.dir());
                    let _ = std::process::Command::new("explorer").arg(self.library.dir()).spawn();
                }
            });
        });
        ui.separator();

        let mut to_open = None;
        let mut to_delete = None;
        let thumb_h = 96.0;
        let row_h = thumb_h + 44.0;
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, row_h, entries.len(), |ui, rows| {
            for entry in &entries[rows] {
                let selected = self.current_file.as_deref() == Some(entry.file.as_str());
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), Sense::click());
                if selected || response.hovered() {
                    let fill = if selected { self.accent().gamma_multiply(0.25) } else { ui.visuals().widgets.hovered.weak_bg_fill };
                    ui.painter().rect_filled(rect, 6.0, fill);
                }
                let inner = rect.shrink(6.0);
                if let Some(tex) = self.thumb_texture(&ctx, &entry.file) {
                    let size = tex.size_vec2();
                    let scale = (inner.width() / size.x).min(thumb_h / size.y).min(1.0);
                    let img_rect = Rect::from_min_size(inner.min, size * scale);
                    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                    ui.painter().image(tex.id(), img_rect, uv, Color32::WHITE);
                }
                let text_top = inner.min + egui::vec2(0.0, thumb_h + 2.0);
                let title = if entry.title.is_empty() { t!("(sem título)", "(untitled)") } else { &entry.title };
                let painter = ui.painter_at(inner);
                // Theme colors: a fixed white disappeared on the light theme's background.
                let (title_color, detail_color) = (ui.visuals().strong_text_color(), ui.visuals().weak_text_color());
                painter.text(text_top, egui::Align2::LEFT_TOP, title, egui::FontId::proportional(12.5), title_color);
                painter.text(
                    text_top + egui::vec2(0.0, 16.0),
                    egui::Align2::LEFT_TOP,
                    format!("{}  •  {}×{}", entry.when, entry.width, entry.height),
                    egui::FontId::proportional(11.0),
                    detail_color,
                );
                if response.clicked() {
                    to_open = Some(entry.file.clone());
                }
                response.context_menu(|ui| {
                    if ui.button(t!("Abrir", "Open")).clicked() {
                        to_open = Some(entry.file.clone());
                        ui.close();
                    }
                    if ui.button(t!("Mostrar na pasta", "Show in folder")).clicked() {
                        let _ = std::process::Command::new("explorer")
                            .arg("/select,")
                            .arg(self.library.path(&entry.file))
                            .spawn();
                        ui.close();
                    }
                    if ui.button(t!("Mover para a Lixeira", "Move to Recycle Bin")).clicked() {
                        to_delete = Some(entry.file.clone());
                        ui.close();
                    }
                });
            }
        });
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
            let files: Vec<&str> = entries.iter().map(|e| e.file.as_str()).collect();
            if let Some(file) = self.current_file.clone() {
                open_after = library::neighbor(&files, &file).map(str::to_owned);
                to_delete = Some(file);
            }
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

    fn set_captured(&mut self, ctx: &egui::Context, image: RgbaImage, title: String) {
        self.adopt_editor(ctx, editor::Editor::new(ctx, image, self.accent()), title);
    }

    /// Saves the editor's image to the library, copies it and shows it in the main editor.
    fn adopt_editor(&mut self, ctx: &egui::Context, editor: editor::Editor, title: String) {
        let image = editor.image().clone();
        let file = self.library.add(win::local_time(), title, image.width(), image.height());
        let thumb = library::make_thumb(&image);
        let texture = ctx.load_texture("thumbnail", to_color_image(&thumb), TextureOptions::LINEAR);
        self.thumbs.insert(file.clone(), texture);
        self.writer.save(&self.library, &file, image.clone(), thumb);
        self.current_file = Some(file);
        // The image saved above already includes what has been annotated so far.
        self.saved_revision = editor.revision();
        self.status = match copy_to_clipboard(&image) {
            Ok(()) => tf!(
                "Captura de {}×{} px copiada para a área de transferência.",
                "{}×{} px capture copied to the clipboard.",
                image.width(),
                image.height()
            ),
            Err(e) => tf!("Captura feita, mas não foi possível copiar: {e}", "Captured, but could not copy: {e}"),
        };
        self.editor = Some(editor);
    }

    /// Hides the window and scrolls the chosen area in the background; the result arrives through
    /// the same queue as the other captures.
    fn start_scroll(
        &mut self,
        ctx: &egui::Context,
        rect: win::PxRect,
        direction: scroll::Direction,
        title: String,
    ) {
        if self.shared.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        self.status = t!("Capturando com rolagem… Esc interrompe.", "Scrolling capture… Esc stops.").to_owned();
        let (shared, ctx) = (self.shared.clone(), ctx.clone());
        ctx.request_repaint();
        std::thread::spawn(move || {
            // Gives the window time to leave full screen before minimizing.
            std::thread::sleep(Duration::from_millis(250));
            if shared.hwnd != 0 {
                win::minimize(shared.hwnd);
            }
            std::thread::sleep(Duration::from_millis(450));
            // Frame around the area while the scrolling happens.
            let frame = win::RegionFrame::show(rect);
            let result = scroll::run(rect, direction);
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
                win::show(shared.hwnd);
            }
            ctx.request_repaint();
        });
    }

    /// Turns the window into a small, always visible panel to control the panoramic capture.
    fn start_panoramic(&mut self, rect: win::PxRect, title: String) {
        self.shared.busy.store(true, Ordering::SeqCst);
        self.pano = Some(PanoUi {
            rect,
            title,
            session: None,
            _frame: win::RegionFrame::show(rect),
            setup_frames: 6,
        });
    }

    /// Turns the window into a small panel to control the video recording.
    fn start_video(&mut self, rect: win::PxRect, as_gif: bool) {
        self.shared.busy.store(true, Ordering::SeqCst);
        let frame = win::RegionFrame::show(rect);
        self.video = Some(VideoUi { rect, recorder: None, as_gif, _frame: frame, setup_frames: 6 });
    }

    fn video_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let hwnd = self.shared.hwnd;
        let Some(video) = &mut self.video else { return };
        ctx.request_repaint_after(Duration::from_millis(200));
        hud_setup(&ctx, hwnd, video.rect, VIDEO_HUD_SIZE, &mut video.setup_frames);

        // FFmpeg only exits on its own when something went wrong.
        let crashed = video.recorder.as_mut().is_some_and(|r| r.has_exited());
        let (mut start, mut stop, mut cancel, mut toggle_pause) = (false, crashed, false, false);
        let (mut system_audio, mut microphone) = (self.cfg.record_system_audio, self.cfg.record_microphone);
        let as_gif = video.as_gif;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal_centered(|ui| match &video.recorder {
                None => {
                    start = ui.button(if as_gif { t!("● Gravar GIF", "● Record GIF") } else { t!("● Gravar", "● Record") }).clicked();
                    cancel = ui.button(t!("Cancelar", "Cancel")).clicked();
                    ui.separator();
                    if as_gif {
                        ui.weak(t!("Sem som; vira GIF ao parar.", "No sound; becomes a GIF when stopped."));
                    } else {
                        ui.checkbox(&mut system_audio, t!("Som do sistema", "System sound"));
                        ui.checkbox(&mut microphone, t!("Microfone", "Microphone"));
                    }
                }
                Some(session) => {
                    stop |= ui.button(t!("■ Parar", "■ Stop")).clicked();
                    let paused = session.is_paused();
                    toggle_pause = ui.button(if paused { t!("Continuar", "Resume") } else { t!("Pausar", "Pause") }).clicked();
                    let secs = session.elapsed().as_secs();
                    if paused {
                        ui.label(tf!("Em pausa  {:02}:{:02}", "Paused  {:02}:{:02}", secs / 60, secs % 60));
                    } else {
                        ui.colored_label(Color32::from_rgb(235, 64, 52), "●");
                        ui.label(tf!("Gravando  {:02}:{:02}", "Recording  {:02}:{:02}", secs / 60, secs % 60));
                    }
                    if let Some(warning) = session.warnings.first() {
                        ui.colored_label(Color32::from_rgb(255, 200, 80), warning);
                    }
                }
            });
        });
        let (rect, idle) = (video.rect, video.recorder.is_none());
        if !as_gif && (system_audio, microphone) != (self.cfg.record_system_audio, self.cfg.record_microphone) {
            self.cfg.record_system_audio = system_audio;
            self.cfg.record_microphone = microphone;
            self.save_cfg();
        }

        if start {
            let (y, mo, d, h, mi, s) = win::local_time();
            let name = format!("{y:04}-{mo:02}-{d:02}_{h:02}-{mi:02}-{s:02}");
            let options = if as_gif {
                record::Options { system_audio: false, microphone: false }
            } else {
                record::Options { system_audio, microphone }
            };
            match record::Session::start(rect, options, &record::videos_dir(), &name) {
                Ok(session) => {
                    if let Some(video) = &mut self.video {
                        video.recorder = Some(session);
                    }
                }
                Err(e) => {
                    self.leave_pano_hud(&ctx);
                    self.status = tf!("Não foi possível gravar: {e}", "Could not record: {e}");
                }
            }
        } else if stop {
            let session = self.video.take().and_then(|v| v.recorder);
            self.leave_pano_hud(&ctx);
            match session.map(record::Session::stop) {
                Some(Ok(path)) if as_gif => {
                    // GIF mode: converts directly and doesn't leave the intermediate video behind.
                    self.start_export(&ctx, t!("Criando o GIF", "Creating the GIF"), move || {
                        let gif = record::to_gif(&path, &record::GifOptions::default());
                        if gif.is_ok() {
                            let _ = std::fs::remove_file(&path);
                        }
                        gif
                    });
                }
                Some(Ok(path)) => {
                    self.status = tf!("Vídeo salvo em {}", "Video saved to {}", path.display());
                    self.video_done = Some(VideoDone::new(path));
                }
                Some(Err(e)) => self.status = tf!("A gravação falhou: {e}", "The recording failed: {e}"),
                None => {}
            }
        } else if toggle_pause {
            if let Some(session) = self.video.as_mut().and_then(|v| v.recorder.as_mut()) {
                let result = if session.is_paused() { session.resume() } else { session.pause() };
                if let Err(e) = result {
                    self.status = tf!("Não foi possível pausar ou continuar: {e}", "Could not pause or resume: {e}");
                }
            }
        } else if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) && idle {
            self.leave_pano_hud(&ctx);
            self.status = t!("Gravação cancelada.", "Recording canceled.").to_owned();
        }
    }

    /// Runs a job that produces a file (GIF or video clip) in the background.
    fn start_export(
        &mut self,
        ctx: &egui::Context,
        label: &str,
        job: impl FnOnce() -> Result<std::path::PathBuf, String> + Send + 'static,
    ) {
        self.gif_running = true;
        self.status = format!("{label}…");
        let (pending, ctx) = (self.gif_pending.clone(), ctx.clone());
        std::thread::spawn(move || {
            *pending.lock().unwrap() = Some(job());
            ctx.request_repaint();
        });
    }

    /// Window with the video just recorded: open it, trim a clip or create a GIF.
    fn video_done_ui(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.gif_pending.lock().unwrap().take() {
            self.gif_running = false;
            self.status = match result {
                Ok(file) => {
                    let _ = std::process::Command::new("explorer").arg("/select,").arg(&file).spawn();
                    tf!("Arquivo salvo em {}", "File saved to {}", file.display())
                }
                Err(e) => tf!("Não foi possível gerar o arquivo: {e}", "Could not create the file: {e}"),
            };
        }
        let busy = self.gif_running;
        let Some(done) = &mut self.video_done else { return };
        let (mut open, mut make_gif, mut save_cut) = (true, false, false);
        egui::Window::new(t!("Gravação salva", "Recording saved")).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            let name = done.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            ui.label(format!("{name}  •  {:.1} s", done.length));
            ui.horizontal(|ui| {
                if ui.button(t!("▶ Abrir", "▶ Open")).clicked() {
                    let _ = std::process::Command::new("explorer").arg(&done.path).spawn();
                }
                if ui.button(t!("Mostrar na pasta", "Show in folder")).clicked() {
                    let _ = std::process::Command::new("explorer").arg("/select,").arg(&done.path).spawn();
                }
            });
            // Without a known length there's no way to choose a clip.
            if done.length > 0.2 {
                ui.separator();
                ui.strong(t!("Trecho", "Clip"));
                let (length, gap) = (done.length, 0.1);
                ui.horizontal(|ui| {
                    ui.label(t!("Início", "Start"));
                    ui.add(egui::Slider::new(&mut done.start, 0.0..=length - gap).fixed_decimals(1).suffix(" s"));
                });
                ui.horizontal(|ui| {
                    ui.label(t!("Fim    ", "End    "));
                    ui.add(egui::Slider::new(&mut done.end, gap..=length).fixed_decimals(1).suffix(" s"));
                });
                done.end = done.end.max(done.start + gap).min(length);
                done.start = done.start.min(done.end - gap).max(0.0);
                let whole = done.start <= 0.0 && done.end >= length;
                save_cut = ui
                    .add_enabled(!busy && !whole, egui::Button::new(t!("Salvar só esse trecho", "Save only this clip")))
                    .on_hover_text(t!(
                        "Cria um vídeo novo ao lado do original, que não é alterado.",
                        "Creates a new video next to the original, which is left unchanged."
                    ))
                    .clicked();
            }
            ui.separator();
            ui.strong("GIF");
            ui.horizontal(|ui| {
                ui.label(t!("Quadros por segundo", "Frames per second"));
                egui::ComboBox::from_id_salt("gif_fps").width(60.0).selected_text(done.fps.to_string()).show_ui(ui, |ui| {
                    for fps in [8, 12, 15, 24] {
                        ui.selectable_value(&mut done.fps, fps, fps.to_string());
                    }
                });
                ui.label(t!("Largura", "Width"));
                let label = |w: u32| if w == 0 { t!("Original", "Original").to_owned() } else { format!("{w} px") };
                egui::ComboBox::from_id_salt("gif_width").width(90.0).selected_text(label(done.width)).show_ui(ui, |ui| {
                    for width in [320, 480, 720, 960, 0] {
                        ui.selectable_value(&mut done.width, width, label(width));
                    }
                });
            });
            let label = if busy { t!("Trabalhando…", "Working…") } else { t!("Criar GIF", "Create GIF") };
            make_gif = ui
                .add_enabled(!busy, egui::Button::new(label))
                .on_hover_text(t!(
                    "Usa o trecho escolhido acima. Mais quadros e mais largura deixam o arquivo maior.",
                    "Uses the clip chosen above. More frames and more width make the file bigger."
                ))
                .clicked();
        });
        let (path, start, end, length) = (done.path.clone(), done.start, done.end, done.length);
        let options = record::GifOptions {
            fps: done.fps,
            width: done.width,
            start,
            end: (length > 0.2 && end < length).then_some(end),
        };
        if !open {
            self.video_done = None;
        }
        if make_gif {
            self.start_export(ctx, t!("Criando o GIF", "Creating the GIF"), move || record::to_gif(&path, &options));
        } else if save_cut {
            self.start_export(ctx, t!("Salvando o trecho", "Saving the clip"), move || record::trim(&path, start, end));
        }
    }

    /// Puts the window back at the size and place it had before the floating panel.
    fn leave_pano_hud(&mut self, ctx: &egui::Context) {
        self.pano = None;
        self.video = None;
        self.steps = None;
        self.shared.busy.store(false, Ordering::SeqCst);
        win::exclude_from_capture(self.shared.hwnd, false);
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(NORMAL_MIN_SIZE));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
            self.normal_size.unwrap_or(egui::vec2(900.0, 600.0)),
        ));
        if let Some(pos) = self.normal_pos {
            win::set_window_pos(self.shared.hwnd, pos);
        }
    }

    /// Turns the window into a small panel to control the step-by-step capture.
    fn start_steps(&mut self, rect: win::PxRect, title: String) {
        self.shared.busy.store(true, Ordering::SeqCst);
        let frame = win::RegionFrame::show(rect);
        self.steps = Some(StepsUi { rect, title, recorder: None, _frame: frame, setup_frames: 6 });
    }

    fn steps_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let hwnd = self.shared.hwnd;
        let Some(steps) = &mut self.steps else { return };
        ctx.request_repaint_after(Duration::from_millis(250));
        hud_setup(&ctx, hwnd, steps.rect, STEPS_HUD_SIZE, &mut steps.setup_frames);

        let (mut start, mut finish, mut cancel, mut undo) = (false, false, false, false);
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal_centered(|ui| match &steps.recorder {
                None => {
                    start = ui.button(t!("● Começar", "● Start")).clicked();
                    cancel = ui.button(t!("Cancelar", "Cancel")).clicked();
                    ui.separator();
                    ui.label(t!(
                        "Depois de começar, cada clique na área (e cada Enter) vira um passo.",
                        "Once started, each click in the area (and each Enter) becomes a step."
                    ));
                }
                Some(recorder) => {
                    let count = recorder.count();
                    finish = ui.add_enabled(count > 0, egui::Button::new(t!("■ Concluir", "■ Finish"))).clicked();
                    undo = ui
                        .add_enabled(count > 0, egui::Button::new(t!("Tirar o último", "Remove the last one")))
                        .on_hover_text(t!(
                            "Descarta o último passo, se o clique não fazia parte da tarefa.",
                            "Discards the last step, if that click was not part of the task."
                        ))
                        .clicked();
                    cancel = ui.button(t!("Cancelar", "Cancel")).clicked();
                    ui.separator();
                    ui.colored_label(Color32::from_rgb(235, 64, 52), "●");
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
        } else if undo {
            if let Some(recorder) = &steps.recorder {
                recorder.undo_last();
            }
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
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal_centered(|ui| match &pano.session {
                None => {
                    start = ui.button(t!("● Iniciar", "● Start")).clicked();
                    cancel = ui.button(t!("Cancelar", "Cancel")).clicked();
                    ui.label(t!("Depois de iniciar, role a área devagar,\nem qualquer direção.", "Once started, scroll the area slowly,\nin any direction."));
                }
                Some(session) => {
                    stop = ui.button(t!("■ Parar", "■ Stop")).clicked();
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
            win::cloak(self.shared.hwnd, false);
        }
        self.shared.busy.store(false, Ordering::SeqCst);
        win::exclude_from_capture(self.shared.hwnd, false);
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
        // The frame comes back a few frames later; the rectangle is reapplied until then.
        self.restore_frames = 10;
        ctx.request_repaint();
    }

    /// Photographs `rect` after the configured delay, with the window out of the way.
    fn capture_after_delay(&mut self, ctx: &egui::Context, rect: win::PxRect, title: String, delay: u32, cursor: bool) {
        if self.shared.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        self.status = tf!("Capturando em {delay} s…", "Capturing in {delay} s…");
        let (shared, ctx) = (self.shared.clone(), ctx.clone());
        std::thread::spawn(move || {
            if shared.hwnd != 0 {
                win::exclude_from_capture(shared.hwnd, true);
                win::minimize(shared.hwnd);
            }
            std::thread::sleep(Duration::from_secs(delay as u64));
            let shot = capture::grab_virtual(rect, cursor).map(|image| capture::Shot {
                image,
                windows: Vec::new(),
                title,
                note: String::new(),
            });
            *shared.pending.lock().unwrap() = Some((CaptureKind::FullScreen, shot));
            if shared.hwnd != 0 {
                win::exclude_from_capture(shared.hwnd, false);
                win::show(shared.hwnd);
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
        let scale = ctx.pixels_per_point();
        let desktop = win::virtual_screen();
        let (ox, oy) = win::client_origin(hwnd).unwrap_or((desktop.0, desktop.1));
        let to_px = |p: Pos2| -> (i32, i32) {
            (ox + ((p.x - rect.min.x) * scale).round() as i32, oy + ((p.y - rect.min.y) * scale).round() as i32)
        };
        let pointer = ctx.pointer_latest_pos();

        // Adjustment in progress: the area follows the cursor, showing the live screen; on release,
        // the new place is photographed again and the annotations remain editable. Moving takes the
        // annotations along with the frame; resizing leaves them over the same spot on the screen.
        if let Some(adj) = quick.adjust {
            if pointer.is_some() && ctx.input(|i| i.pointer.primary_down()) {
                match adj {
                    Adjust::Move { grab, start } => quick.area = clamp_move(start, pointer.unwrap() - grab, rect),
                    Adjust::Resize { handle, start } => quick.area = apply_handle(start, handle, pointer.unwrap(), rect),
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
                let cursor = self.shared.include_cursor.load(Ordering::Relaxed);
                if let Ok(image) = capture::grab_virtual((a.0, a.1, b.0, b.1), cursor) {
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

        // Esc and Enter serve the text being typed or the selected annotation first.
        let busy_before = quick.editor.is_typing() || quick.editor.has_selection();
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
            if h.drag_started() {
                if let Some(p) = pointer {
                    quick.adjust = Some(Adjust::Move { grab: p, start: quick.area });
                }
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
        pos.x = pos.x.min(rect.max.x - 900.0).max(rect.min.x + 4.0);
        let mut action = None;
        egui::Area::new(egui::Id::unique("quick_bar"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        quick.editor.quick_toolbar_ui(ui);
                        ui.separator();
                        if ui.button(t!("📋 Copiar", "📋 Copy")).on_hover_text(t!("Copia e fecha (Enter ou Ctrl C)", "Copies and closes (Enter or Ctrl C)")).clicked() {
                            action = Some(QuickAction::Copy);
                        }
                        if ui.button(t!("💾 Salvar", "💾 Save")).on_hover_text("Ctrl S").clicked() {
                            action = Some(QuickAction::Save);
                        }
                        if ui.button(t!("✎ Editor", "✎ Editor")).on_hover_text(t!("Continua no editor completo (Ctrl E)", "Continues in the full editor (Ctrl E)")).clicked() {
                            action = Some(QuickAction::Editor);
                        }
                        if ui.button("✕").on_hover_text(t!("Cancelar (Esc)", "Cancel (Esc)")).clicked() {
                            action = Some(QuickAction::Cancel);
                        }
                    });
                });
            });

        if action.is_none() && !busy_before && !quick.editor.is_typing() && !ctx.egui_wants_keyboard_input() {
            action = ctx.input_mut(|i| {
                let copy = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    || i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
                    || copy
                {
                    Some(QuickAction::Copy)
                } else if i.consume_key(egui::Modifiers::COMMAND, egui::Key::S) {
                    Some(QuickAction::Save)
                } else if i.consume_key(egui::Modifiers::COMMAND, egui::Key::E) {
                    Some(QuickAction::Editor)
                } else if i.key_pressed(egui::Key::Escape) {
                    Some(QuickAction::Cancel)
                } else {
                    None
                }
            });
        }
        if let Some(action) = action {
            self.finish_quick(&ctx, action);
        }
    }

    fn finish_quick(&mut self, ctx: &egui::Context, action: QuickAction) {
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
        win::cloak(self.shared.hwnd, true);
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
                win::cloak(hwnd, false);
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
            win::cloak(hwnd, false);
            self.settle = None;
        } else {
            self.put_away();
        }
        ctx.request_repaint();
    }

    /// Takes the window off the screen: to the tray, if there is one, or to the taskbar.
    fn put_away(&self) {
        let hwnd = self.shared.hwnd;
        if !win::is_on_screen(hwnd) {
            return;
        }
        if let Some(r) = self.normal_rect {
            win::set_window_rect(hwnd, r);
        }
        if self.tray.is_some() {
            win::hide(hwnd);
        } else {
            win::minimize(hwnd);
        }
    }

    fn selection_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let hwnd = self.shared.hwnd;
        // The window has to cover every monitor. It loses its frame a few frames after the request,
        // and the system can change its size in the meantime; that's why the check runs every
        // frame.
        let desktop = win::virtual_screen();
        let covering = win::window_rect(hwnd) == Some(desktop);
        if !covering {
            win::set_window_rect(hwnd, desktop);
        }
        // The window started invisible: it appears once it has been drawn a few times at the right
        // size, so the switch doesn't flicker.
        if let Some(sel) = self.selecting.as_mut().filter(|s| !s.revealed) {
            sel.ready_frames = if covering { sel.ready_frames + 1 } else { 0 };
            if sel.ready_frames >= 3 || sel.started.elapsed() > Duration::from_millis(500) {
                win::cloak(hwnd, false);
                sel.revealed = true;
            }
        }
        if self.selecting.as_ref().is_some_and(|s| s.quick.is_some()) {
            self.quick_ui(ui);
            return;
        }
        // While a button is pressed, remember where the press started.
        if let Some(sel) = &mut self.selecting {
            if let Some(origin) = ctx.input(|i| i.pointer.press_origin()) {
                sel.drag_origin = Some(origin);
            }
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
        let scale = ctx.pixels_per_point();
        let (ox, oy) = win::client_origin(hwnd).unwrap_or((desktop.0, desktop.1));
        let to_px = |p: Pos2| -> (i32, i32) {
            (ox + ((p.x - rect.min.x) * scale).round() as i32, oy + ((p.y - rect.min.y) * scale).round() as i32)
        };
        let px_to_screen = |r: win::PxRect| {
            Rect::from_min_max(
                rect.min + egui::vec2((r.0 - ox) as f32 / scale, (r.1 - oy) as f32 / scale),
                rect.min + egui::vec2((r.2 - ox) as f32 / scale, (r.3 - oy) as f32 / scale),
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
        if response.drag_stopped() && drawn.is_none() {
            if let Some(r) = drag_rect.filter(|r| r.width() >= 4.0 && r.height() >= 4.0) {
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
        }
        // A new drag outside the armed region: back to drawing over it.
        if let Some(area) = drawn {
            if response.drag_started() && press_origin.is_some_and(|p| !area.contains(p)) {
                drawn = None;
                adjust = None;
            }
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
            if inside.drag_started() {
                if let Some(p) = pointer {
                    adjust = Some(Adjust::Move { grab: p, start: area });
                }
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
            if pointer.is_some() && ctx.input(|i| i.pointer.primary_down()) {
                match a {
                    Adjust::Move { grab, start } => drawn = Some(clamp_move(start, pointer.unwrap() - grab, rect)),
                    Adjust::Resize { handle, start } => drawn = Some(apply_handle(start, handle, pointer.unwrap(), rect)),
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
                let size = format!("{:.0} × {:.0}", s.width() * scale, s.height() * scale);
                let label = if drawn.is_some() {
                    tf!("{size}  •  arraste para mover, puxe as alças, Ctrl C captura", "{size}  •  drag to move, pull the handles, Ctrl C captures")
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
            if let Some(pixels) = win::grab_pixels(cx - CELLS / 2, cy - CELLS / 2, CELLS, CELLS) {
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
            let coords = painter.layout_no_wrap(format!("{cx}, {cy}"), egui::FontId::monospace(13.0), Color32::WHITE);
            let pos = loupe.left_bottom() + egui::vec2(4.0, 6.0);
            let bg = Rect::from_min_size(pos, coords.size()).expand2(egui::vec2(5.0, 2.0));
            painter.rect_filled(bg, 3.0, Color32::from_black_alpha(200));
            painter.galley(pos, coords, Color32::WHITE);
        }

        // Esc cancels the whole capture; the Redo button reopens the region drawing.
        let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape)) || win::escape_pressed();

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
            egui::Area::new(egui::Id::unique("adjust_bar"))
                .order(egui::Order::Foreground)
                .fixed_pos(pos)
                .show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .button(t!("📸 Capturar (Enter)", "📸 Capture (Enter)"))
                                .on_hover_text(t!(
                                    "Ctrl C ou duplo clique dentro da região também captura",
                                    "Ctrl C or a double-click inside the region also captures"
                                ))
                                .clicked()
                            {
                                bar = Some(true);
                            }
                            if ui.button(t!("↺ Refazer", "↺ Redo")).on_hover_text(t!("Desenha a região de novo", "Draws the region again")).clicked() {
                                bar = Some(false);
                            }
                            if ui.button("✕").on_hover_text(t!("Cancelar (Esc)", "Cancel (Esc)")).clicked() {
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
                    self.start_video(r, kind == CaptureKind::Gif);
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

    fn home_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut styles_changed = false;
        egui::Panel::top("bar").show(ui, |ui| {
            ui.add_space(6.0);
            // Program row: on the left what captures, on the right what you do with the capture.
            // With a narrow window the buttons move to the next row instead of overlapping.
            ui.horizontal_wrapped(|ui| {
                if ui.button(tf!("✂ Região ({})", "✂ Region ({})", self.cfg.hotkey.label())).clicked() {
                    start_capture(CaptureKind::Region, self.shared.clone(), ctx.clone());
                }
                if ui
                    .button(t!("⛶ Tela cheia", "⛶ Full screen"))
                    .on_hover_text(t!("O monitor onde está o cursor.", "The monitor the cursor is on."))
                    .clicked()
                {
                    start_capture(CaptureKind::FullScreen, self.shared.clone(), ctx.clone());
                }
                if win::monitor_count() > 1
                    && ui
                        .button(t!("▣ Todas as telas", "▣ All screens"))
                        .on_hover_text(t!("Todos os monitores em uma imagem só.", "Every monitor in a single image."))
                        .clicked()
                {
                    start_capture(CaptureKind::AllScreens, self.shared.clone(), ctx.clone());
                }
                if ui
                    .button(t!("⇵ Rolagem", "⇵ Scrolling"))
                    .on_hover_text(t!(
                        "Escolha a área que rola; o SnapKit desce a página e junta tudo em uma imagem.",
                        "Pick the area that scrolls; SnapKit scrolls the page and joins it all into one image."
                    ))
                    .clicked()
                {
                    start_capture(CaptureKind::Scroll, self.shared.clone(), ctx.clone());
                }
                if ui
                    .button(t!("▭ Panorâmica", "▭ Panoramic"))
                    .on_hover_text(t!(
                        "Escolha uma área e role você mesmo, em qualquer direção; o SnapKit monta a imagem.",
                        "Pick an area and scroll it yourself, in any direction; SnapKit builds the image."
                    ))
                    .clicked()
                {
                    start_capture(CaptureKind::Panoramic, self.shared.clone(), ctx.clone());
                }
                if ui
                    .button(t!("● Vídeo", "● Video"))
                    .on_hover_text(t!(
                        "Escolha uma área e grave a tela em MP4, com som do sistema e microfone.",
                        "Pick an area and record the screen as MP4, with system sound and microphone."
                    ))
                    .clicked()
                {
                    start_capture(CaptureKind::Video, self.shared.clone(), ctx.clone());
                }
                if ui
                    .button("GIF")
                    .on_hover_text(t!(
                        "Escolha uma área e grave; ao parar, a gravação vira um GIF animado, sem som.",
                        "Pick an area and record; when stopped, the recording becomes an animated GIF, without sound."
                    ))
                    .clicked()
                {
                    start_capture(CaptureKind::Gif, self.shared.clone(), ctx.clone());
                }
                if ui
                    .button(t!("Passo a passo", "Step by step"))
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
                ui.menu_button(summary, |ui| {
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
                    if ui.button(t!("⚙ Preferências", "⚙ Preferences")).clicked() {
                        self.show_prefs = !self.show_prefs;
                        self.hotkey_draft = self.cfg.hotkey.clone();
                    }
                    ui.toggle_value(&mut self.show_library, t!("Biblioteca", "Library"));
                    ui.separator();
                    let has_capture = self.editor.is_some();
                    let can_read = has_capture && !self.ocr_running;
                    if ui
                        .add_enabled(can_read, egui::Button::new(t!("🔤 Ler texto", "🔤 Read text")))
                        .on_hover_text(t!(
                            "Reconhece todo o texto da imagem e copia. Para uma área só, use a ferramenta Extrair texto.",
                            "Recognizes all the text in the image and copies it. For a single area, use the Grab text tool."
                        ))
                        .clicked()
                    {
                        if let Some(image) = self.editor.as_ref().map(|e| e.image().clone()) {
                            self.start_ocr(&ctx, image, ocr::Mode::Printed);
                        }
                    }
                    if ui.add_enabled(has_capture, egui::Button::new(t!("💾 Salvar PNG", "💾 Save PNG"))).clicked() {
                        if let Some(e) = &self.editor {
                            self.status = save_png(e.image());
                        }
                    }
                    if ui.add_enabled(has_capture, egui::Button::new(t!("📋 Copiar", "📋 Copy"))).clicked() {
                        if let Some(e) = &self.editor {
                            self.status = match copy_to_clipboard(e.image()) {
                                Ok(()) => t!("Copiado para a área de transferência.", "Copied to the clipboard.").to_owned(),
                                Err(e) => tf!("Não foi possível copiar: {e}", "Could not copy: {e}"),
                            };
                        }
                    }
                });
            });
            ui.add_space(4.0);
            // Editor row: options of the active tool or the selected object.
            if let Some(editor) = &mut self.editor {
                ui.separator();
                styles_changed = editor.options_ui(ui, &mut self.cfg.styles);
                ui.add_space(6.0);
            }
        });
        if styles_changed {
            self.save_cfg();
        }
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                if let Some(editor) = &mut self.editor {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| editor.zoom_ui(ui));
                }
            });
        });
        if let Some(editor) = &mut self.editor {
            egui::Panel::left("tools").resizable(false).default_size(132.0).show(ui, |ui| editor.tools_ui(ui));
        }
        if self.show_library {
            egui::Panel::right("library")
                .resizable(true)
                .default_size(250.0)
                .size_range(200.0..=420.0)
                .show(ui, |ui| self.library_ui(ui));
        }
        self.autosave(&ctx);
        if let Some(area) = self.editor.as_mut().and_then(|e| e.take_text_request()) {
            self.start_ocr(&ctx, area, ocr::Mode::Printed);
        }
        self.ocr_ui(&ctx);
        self.video_done_ui(&ctx);
        egui::CentralPanel::default().show(ui, |ui| match &mut self.editor {
            Some(editor) => editor.canvas_ui(ui),
            None => {
                ui.centered_and_justified(|ui| {
                    ui.label(tf!(
                        "Nenhuma captura ainda.\nUse os botões acima ou {}.",
                        "No capture yet.\nUse the buttons above or {}.",
                        self.cfg.hotkey.label()
                    ));
                });
            }
        });
        if self.show_prefs {
            self.prefs_ui(&ctx);
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
        if let Some((kind, title, was_on_screen)) = self.shared.select_request.lock().unwrap().take() {
            let hwnd = self.shared.hwnd;
            let windows = win::visible_windows(hwnd);
            self.selecting = Some(Selecting { kind, windows, title, drag_origin: None, drawn: None, adjust: None, was_on_screen, quick: None, started: Instant::now(), ready_frames: 0, revealed: false });
            self.restore_frames = 0;
            self.settle = None;
            // Excluded from captures: the shot of the chosen area comes out without our layer on
            // top.
            win::exclude_from_capture(hwnd, true);
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
        let Some((_, shot)) = self.shared.pending.lock().unwrap().take() else {
            return;
        };
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
        // Closing with the X only hides the window when there's a tray to bring it back.
        if ui.ctx().input(|i| i.viewport().close_requested())
            && self.cfg.close_to_tray
            && self.tray.is_some()
            && self.shared.hwnd != 0
        {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
            win::hide(self.shared.hwnd);
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
                if win::is_on_screen(self.shared.hwnd) {
                    if let Some(r) = self.normal_rect {
                        if win::window_rect(self.shared.hwnd) != Some(r) {
                            win::set_window_rect(self.shared.hwnd, r);
                        }
                    }
                    self.restore_frames -= 1;
                }
                ctx.request_repaint();
            } else if !fullscreen && win::is_on_screen(self.shared.hwnd) {
                if let Some(size) = ctx.input(|i| i.viewport().inner_rect).map(|r| r.size()) {
                    if size.x >= NORMAL_MIN_SIZE.x && size.y >= NORMAL_MIN_SIZE.y {
                        self.normal_size = Some(size);
                        self.normal_pos = win::window_pos(self.shared.hwnd);
                        self.normal_rect = win::window_rect(self.shared.hwnd);
                    }
                }
            }
            if self.save_requested && self.restore_frames == 0 {
                self.save_requested = false;
                if let Some(e) = &self.editor {
                    self.status = save_png(e.image());
                }
            }
            self.home_ui(ui);
        }
    }
}

/// Listens for the global hotkey in the background and triggers the region capture.
fn spawn_hotkey_listener(shared: Arc<Shared>, ctx: egui::Context) {
    std::thread::spawn(move || {
        while let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
            let active = shared.hotkey_id.load(Ordering::Relaxed);
            if active != 0 && event.id == active && event.state == HotKeyState::Pressed {
                start_capture(CaptureKind::Region, shared.clone(), ctx.clone());
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

fn to_color_image(img: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    )
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

fn save_png(img: &RgbaImage) -> String {
    let Some(path) = rfd::FileDialog::new()
        .add_filter(t!("Imagem PNG", "PNG image"), &["png"])
        .set_file_name(t!("captura.png", "capture.png"))
        .save_file()
    else {
        return t!("Salvamento cancelado.", "Save canceled.").to_owned();
    };
    match img.save(&path) {
        Ok(()) => tf!("Salvo em {}", "Saved to {}", path.display()),
        Err(e) => tf!("Não foi possível salvar: {e}", "Could not save: {e}"),
    }
}

fn main() -> eframe::Result {
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

    legacy::migrate();
    let start_hidden = args.iter().any(|a| a == autostart::MINIMIZED_ARG);
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
    eframe::run_native(APP_NAME, options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
