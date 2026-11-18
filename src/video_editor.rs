//! Interactive video and GIF editor: visual filmstrip timeline, trim handles, live frame scrub &
//! preview playback, interactive area crop overlay, speed/audio/resolution controls, GIF export
//! and direct frame capture to the annotation editor.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2};
use image::RgbaImage;

use crate::{editor, record, theme};

const FILMSTRIP_COUNT: usize = 8;
const FILMSTRIP_HEIGHT: u32 = 56;
const MIN_TRIM_GAP: f64 = 0.1;
const MIN_CROP_PX: u32 = 32;
const STREAM_MAX_WIDTH: u32 = 1280;

/// Action requested from the video editor toolbar or canvas, handled by the main application.
#[derive(Clone, Debug)]
pub enum VideoAction {
    /// Exports the trimmed/edited video as a new MP4 entry in the capture library.
    SaveClipToLibrary { opts: record::VideoEditOptions },
    /// Applies the edits in-place to the current library video file.
    OverwriteInLibrary { opts: record::VideoEditOptions },
    /// Converts the selected range into an animated GIF in the capture library.
    ExportGifToLibrary { opts: record::GifOptions },
    /// Prompts for a destination path and exports the edited MP4.
    ExportMp4As { opts: record::VideoEditOptions },
    /// Sends the currently displayed video frame straight to the Image Annotation Editor.
    CaptureFrame { image: RgbaImage },
}

/// Active drag state on the interactive crop rectangle over the video preview.
#[derive(Clone, Copy, Debug)]
enum CropDrag {
    /// Creating a brand new crop rectangle from `origin` (in video pixel coords).
    New { origin: (f32, f32) },
    /// Moving the entire crop rectangle; `grab` is the pointer offset from `(x, y)`.
    Move { grab: (f32, f32), start: (u32, u32, u32, u32) },
    /// Resizing from one of the 8 handles (0 = NW, clockwise to 7 = W).
    Resize { handle: usize, start: (u32, u32, u32, u32) },
}

/// Active drag state on the filmstrip timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimelineDrag {
    Playhead,
    TrimStart,
    TrimEnd,
}

/// Shared state between the UI thread and the background frame-extraction worker.
struct FrameWorkerShared {
    /// Next timestamp (in seconds) requested by the UI; coalesces rapid scrub movements so only the
    /// latest timestamp is extracted.
    requested_secs: Mutex<Option<f64>>,
    /// Most recently decoded frame `(timestamp, RgbaImage)`.
    ready_frame: Mutex<Option<(f64, RgbaImage)>>,
    /// Decoded filmstrip thumbnails once the background filmstrip job finishes.
    ready_filmstrip: Mutex<Option<Vec<RgbaImage>>>,
    /// Updated probe metadata if probing completed or reloaded asynchronously.
    ready_info: Mutex<Option<record::VideoInfo>>,
    /// True while a worker thread is actively extracting frames.
    worker_running: AtomicBool,
}

/// Main video editor state, rendered in the central panel and top options bar.
pub struct VideoEditor {
    path: PathBuf,
    info: record::VideoInfo,
    accent: Color32,
    busy: bool,

    // Playback & timeline state
    playhead: f64,
    trim_start: f64,
    trim_end: f64,
    is_playing: bool,
    stream: Option<record::FrameStream>,
    last_requested_frame: f64,
    timeline_drag: Option<TimelineDrag>,

    // Video edit options
    speed: f64,
    mute: bool,
    max_width: u32,
    crop_enabled: bool,
    crop_rect: Option<(u32, u32, u32, u32)>,
    crop_drag: Option<CropDrag>,

    // GIF export options
    gif_fps: u32,
    gif_width: u32,

    // Textures & frame cache
    current_frame: Option<RgbaImage>,
    preview_tex: Option<TextureHandle>,
    filmstrip_tex: Vec<TextureHandle>,
    filmstrip_loading: bool,

    // Background worker coordination
    worker: Arc<FrameWorkerShared>,
}

impl VideoEditor {
    /// Creates a new video editor for `path`, probing metadata and starting background tasks to
    /// extract the initial high-resolution frame and filmstrip thumbnails.
    pub fn new(
        ctx: &egui::Context,
        path: PathBuf,
        initial_thumb: Option<RgbaImage>,
        accent: Color32,
    ) -> Self {
        let info = record::probe(&path).unwrap_or_else(|| {
            let (w, h) = initial_thumb
                .as_ref()
                .map(|img| (img.width().max(2), img.height().max(2)))
                .unwrap_or((1920, 1080));
            record::VideoInfo {
                width: w,
                height: h,
                duration: 0.0,
                fps: 30.0,
                has_audio: false,
            }
        });

        let max_side = ctx.input(|i| i.max_texture_side).max(2048);
        let preview_tex = initial_thumb.as_ref().map(|img| {
            ctx.load_texture(
                "video_preview",
                editor::texture_image(img, max_side),
                TextureOptions::LINEAR,
            )
        });

        let worker = Arc::new(FrameWorkerShared {
            requested_secs: Mutex::new(None),
            ready_frame: Mutex::new(None),
            ready_filmstrip: Mutex::new(None),
            ready_info: Mutex::new(None),
            worker_running: AtomicBool::new(false),
        });

        let duration = info.duration.max(0.0);
        let mut editor = Self {
            path,
            info,
            accent,
            busy: false,
            playhead: 0.0,
            trim_start: 0.0,
            trim_end: duration,
            is_playing: false,
            stream: None,
            last_requested_frame: -1.0,
            timeline_drag: None,
            speed: 1.0,
            mute: false,
            max_width: 0,
            crop_enabled: false,
            crop_rect: None,
            crop_drag: None,
            gif_fps: 15,
            gif_width: 720,
            current_frame: initial_thumb,
            preview_tex,
            filmstrip_tex: Vec::new(),
            filmstrip_loading: true,
            worker,
        };

        editor.spawn_initial_loaders(ctx);
        editor.request_frame(ctx, 0.0);
        editor
    }

    /// Path of the video file currently open in the editor.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Probed metadata of the video (dimensions, duration, frame rate, audio presence).
    pub fn info(&self) -> record::VideoInfo {
        self.info
    }

    /// The highest-resolution frame currently shown in the preview, if available.
    pub fn current_frame(&self) -> Option<&RgbaImage> {
        self.current_frame.as_ref()
    }

    /// Updates the theme accent color used for timeline handles, crop guides, and active badges.
    pub fn set_accent(&mut self, accent: Color32) {
        self.accent = accent;
    }

    /// Marks whether a background export/overwrite job is currently running, disabling action buttons.
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        if busy {
            self.is_playing = false;
            self.stream = None;
        }
    }

    /// Re-probes the video file and reloads the preview frame and filmstrip after an in-place overwrite.
    pub fn reload_after_overwrite(&mut self, ctx: &egui::Context) {
        self.is_playing = false;
        self.stream = None;
        if let Some(info) = record::probe(&self.path) {
            self.info = info;
        }
        let duration = self.info.duration.max(0.0);
        self.playhead = 0.0;
        self.trim_start = 0.0;
        self.trim_end = duration;
        self.speed = 1.0;
        self.mute = false;
        self.crop_enabled = false;
        self.crop_rect = None;
        self.crop_drag = None;
        self.filmstrip_tex.clear();
        self.filmstrip_loading = true;
        self.last_requested_frame = -1.0;

        self.spawn_initial_loaders(ctx);
        self.request_frame(ctx, 0.0);
    }

    /// Spawns background threads to load the filmstrip thumbnails and re-verify probe metadata.
    fn spawn_initial_loaders(&self, ctx: &egui::Context) {
        let path = self.path.clone();
        let worker = self.worker.clone();
        let ctx_clone = ctx.clone();
        std::thread::spawn(move || {
            if let Some(probed) = record::probe(&path) {
                *worker.ready_info.lock().unwrap() = Some(probed);
                ctx_clone.request_repaint();
            }
            let thumbs = record::extract_filmstrip(&path, FILMSTRIP_COUNT, FILMSTRIP_HEIGHT);
            *worker.ready_filmstrip.lock().unwrap() = Some(thumbs);
            ctx_clone.request_repaint();
        });
    }

    /// Requests asynchronous extraction of the video frame at `at_secs`.
    /// Rapid calls coalesce into the latest requested timestamp so at most one FFmpeg extraction
    /// runs at any given moment.
    fn request_frame(&mut self, ctx: &egui::Context, at_secs: f64) {
        let clamped = if self.info.duration > 0.0 {
            at_secs.clamp(0.0, self.info.duration)
        } else {
            at_secs.max(0.0)
        };
        if (clamped - self.last_requested_frame).abs() < 0.005 {
            return;
        }
        self.last_requested_frame = clamped;
        *self.worker.requested_secs.lock().unwrap() = Some(clamped);

        if !self.worker.worker_running.swap(true, Ordering::SeqCst) {
            let path = self.path.clone();
            let worker = self.worker.clone();
            let ctx_clone = ctx.clone();
            let (init_w, init_h) = (self.info.width, self.info.height);
            std::thread::spawn(move || {
                loop {
                    let next = worker.requested_secs.lock().unwrap().take();
                    let Some(ts) = next else {
                        worker.worker_running.store(false, Ordering::SeqCst);
                        // Double-check if a request slipped in right before storing false.
                        if worker.requested_secs.lock().unwrap().is_some()
                            && !worker.worker_running.swap(true, Ordering::SeqCst)
                        {
                            continue;
                        }
                        break;
                    };
                    let (w, h) = worker
                        .ready_info
                        .lock()
                        .unwrap()
                        .map(|i| (i.width, i.height))
                        .unwrap_or((init_w, init_h));
                    if let Ok(frame) = record::extract_frame_fast(&path, ts, w, h) {
                        *worker.ready_frame.lock().unwrap() = Some((ts, frame));
                        ctx_clone.request_repaint();
                    }
                }
            });
        }
    }

    fn update_preview_texture(&mut self, ctx: &egui::Context, frame: &RgbaImage, max_side: usize) {
        let color_img = editor::texture_image(frame, max_side);
        if let Some(tex) = &mut self.preview_tex
            && tex.size() == color_img.size
        {
            tex.set(color_img, TextureOptions::LINEAR);
        } else {
            self.preview_tex = Some(ctx.load_texture(
                "video_preview",
                color_img,
                TextureOptions::LINEAR,
            ));
        }
    }

    fn playback_end_limit(&self) -> f64 {
        if self.trim_end > self.trim_start + MIN_TRIM_GAP {
            self.trim_end
        } else {
            self.info.duration.max(0.1)
        }
    }

    fn start_playback(&mut self, ctx: &egui::Context) {
        self.stream = None;
        let end_limit = self.playback_end_limit();
        if self.playhead >= end_limit - 0.05 || self.playhead < self.trim_start {
            self.playhead = self.trim_start;
        }
        *self.worker.requested_secs.lock().unwrap() = None;
        let ctx_clone = ctx.clone();
        match record::FrameStream::start(
            &self.path,
            self.playhead,
            Some(end_limit),
            self.info,
            self.speed,
            STREAM_MAX_WIDTH,
            move || ctx_clone.request_repaint(),
        ) {
            Ok(stream) => {
                self.stream = Some(stream);
                self.is_playing = true;
            }
            Err(_) => {
                self.is_playing = false;
            }
        }
    }

    fn pause_playback(&mut self, ctx: &egui::Context) {
        let was_playing = self.is_playing || self.stream.is_some();
        self.is_playing = false;
        self.stream = None;
        if was_playing {
            self.last_requested_frame = -1.0;
            self.request_frame(ctx, self.playhead);
        }
    }

    /// Polls results from background threads and advances preview playback if active.
    fn poll_background(&mut self, ctx: &egui::Context) {
        let max_side = ctx.input(|i| i.max_texture_side).max(2048);

        if let Some(probed) = self.worker.ready_info.lock().unwrap().take() {
            let old_dur = self.info.duration;
            self.info = probed;
            if (old_dur <= 0.0 || (self.trim_end - old_dur).abs() < 0.05) && self.info.duration > 0.0 {
                self.trim_end = self.info.duration;
            }
        }

        let ready_frame = self.worker.ready_frame.lock().unwrap().take();
        if let Some((_ts, frame)) = ready_frame
            && !self.is_playing
        {
            // If probe had fallback dimensions, refine from actual frame.
            if self.info.width == 0 || self.info.height == 0 {
                self.info.width = frame.width().max(2);
                self.info.height = frame.height().max(2);
            }
            self.update_preview_texture(ctx, &frame, max_side);
            self.current_frame = Some(frame);
        }

        if let Some(thumbs) = self.worker.ready_filmstrip.lock().unwrap().take() {
            self.filmstrip_tex = thumbs
                .iter()
                .enumerate()
                .map(|(i, img)| {
                    ctx.load_texture(
                        format!("filmstrip_{i}"),
                        editor::texture_image(img, max_side),
                        TextureOptions::LINEAR,
                    )
                })
                .collect();
            self.filmstrip_loading = false;
        }

        // Pull streamed frames when playing preview
        if self.stream.is_some() {
            let end_limit = self.playback_end_limit();
            let mut stream_done = false;
            let mut next_frame = None;
            if let Some(stream) = &self.stream {
                next_frame = stream.take_frame();
                if stream.is_finished() {
                    stream_done = true;
                }
            }
            if let Some((ts, frame)) = next_frame {
                self.playhead = ts.min(end_limit);
                self.update_preview_texture(ctx, &frame, max_side);
                self.current_frame = Some(frame);
                if self.playhead >= end_limit {
                    stream_done = true;
                }
            }
            if stream_done {
                self.stream = None;
                self.is_playing = false;
                self.playhead = self.trim_start;
                self.last_requested_frame = -1.0;
                self.request_frame(ctx, self.playhead);
            }
        }
    }

    /// Builds the `VideoEditOptions` reflecting the current UI controls.
    fn current_video_options(&self) -> record::VideoEditOptions {
        let end = if self.info.duration > 0.0 && (self.trim_end + 0.01) < self.info.duration {
            Some(self.trim_end)
        } else {
            None
        };
        let crop = if self.crop_enabled {
            self.crop_rect.filter(|&(_, _, w, h)| {
                w >= MIN_CROP_PX
                    && h >= MIN_CROP_PX
                    && (w < self.info.width || h < self.info.height)
            })
        } else {
            None
        };
        record::VideoEditOptions {
            start: self.trim_start.max(0.0),
            end,
            speed: self.speed,
            mute: self.mute || !self.info.has_audio,
            crop,
            max_width: self.max_width,
        }
    }

    /// Builds the `GifOptions` reflecting the current trim range and GIF settings.
    fn current_gif_options(&self) -> record::GifOptions {
        let end = if self.info.duration > 0.0 && (self.trim_end + 0.01) < self.info.duration {
            Some(self.trim_end)
        } else {
            None
        };
        record::GifOptions {
            fps: self.gif_fps,
            width: self.gif_width,
            start: self.trim_start.max(0.0),
            end,
        }
    }

    /// Returns whether any edit parameter differs from the untouched original video.
    fn has_edits(&self) -> bool {
        let trimmed = self.trim_start > 0.02
            || (self.info.duration > 0.0 && (self.info.duration - self.trim_end) > 0.02);
        let speed_changed = (self.speed - 1.0).abs() > 0.01;
        let audio_muted = self.info.has_audio && self.mute;
        let scaled = self.max_width > 0 && self.max_width < self.info.width;
        let cropped = self.crop_enabled
            && self.crop_rect.is_some_and(|(_, _, w, h)| {
                w >= MIN_CROP_PX
                    && h >= MIN_CROP_PX
                    && (w < self.info.width || h < self.info.height)
            });
        trimmed || speed_changed || audio_muted || scaled || cropped
    }

    /// Top options bar for the video editor: speed, audio mute, resolution, crop toggle, GIF settings,
    /// and export/save actions.
    pub fn options_ui(&mut self, ui: &mut egui::Ui) -> Option<VideoAction> {
        let mut action = None;
        let busy = self.busy;

        ui.horizontal_wrapped(|ui| {
            // Speed selector
            ui.label(t!("Velocidade:", "Speed:"));
            let prev_speed = self.speed;
            let speed_label = format!("{}×", self.speed);
            egui::ComboBox::from_id_salt("video_speed_combo")
                .width(64.0)
                .selected_text(speed_label)
                .show_ui(ui, |ui| {
                    for (spd, label) in [
                        (0.5, "0.5×"),
                        (0.75, "0.75×"),
                        (1.0, "1.0×"),
                        (1.25, "1.25×"),
                        (1.5, "1.5×"),
                        (2.0, "2.0×"),
                    ] {
                        ui.selectable_value(&mut self.speed, spd, label);
                    }
                });
            if (self.speed - prev_speed).abs() > 1e-4 && self.is_playing {
                let ctx = ui.ctx().clone();
                self.start_playback(&ctx);
            }

            ui.separator();

            // Audio mute toggle
            let audio_label = if !self.info.has_audio || self.mute {
                t!("🔇 Sem som", "🔇 Muted")
            } else {
                t!("🔊 Som", "🔊 Sound")
            };
            let audio_btn = ui
                .add_enabled(
                    self.info.has_audio && !busy,
                    egui::Button::new(audio_label).selected(self.info.has_audio && !self.mute),
                )
                .on_hover_text(if self.info.has_audio {
                    t!(
                        "Ativa ou remove a faixa de áudio na exportação.",
                        "Keeps or removes the audio track when exporting."
                    )
                } else {
                    t!(
                        "Este vídeo foi gravado sem faixa de áudio.",
                        "This video was recorded without an audio track."
                    )
                });
            if audio_btn.clicked() {
                self.mute = !self.mute;
            }

            ui.separator();

            // Resolution scale selector
            ui.label(t!("Resolução:", "Resolution:"));
            let res_label = |w: u32| match w {
                0 => t!("Original", "Original").to_owned(),
                1920 => "1080p (1920 px)".to_owned(),
                1280 => "720p (1280 px)".to_owned(),
                854 => "480p (854 px)".to_owned(),
                other => format!("{other} px"),
            };
            let short_res_label = |w: u32| match w {
                0 => t!("Original", "Original").to_owned(),
                1920 => "1080p".to_owned(),
                1280 => "720p".to_owned(),
                854 => "480p".to_owned(),
                other => format!("{other}p"),
            };
            egui::ComboBox::from_id_salt("video_res_combo")
                .width(82.0)
                .selected_text(short_res_label(self.max_width))
                .show_ui(ui, |ui| {
                    for w in [0, 1920, 1280, 854] {
                        ui.selectable_value(&mut self.max_width, w, res_label(w));
                    }
                });

            ui.separator();

            // Interactive Crop toggle
            let crop_btn = ui
                .add_enabled(
                    !busy,
                    egui::Button::new(t!("✂ Recortar área", "✂ Crop area"))
                        .selected(self.crop_enabled),
                )
                .on_hover_text(t!(
                    "Arraste o quadro sobre o vídeo para recortar uma região específica.",
                    "Drag the frame over the video to crop a specific region."
                ));
            if crop_btn.clicked() {
                self.crop_enabled = !self.crop_enabled;
                if self.crop_enabled && self.crop_rect.is_none() {
                    let w = self.info.width.max(64);
                    let h = self.info.height.max(64);
                    let cw = (w * 3 / 4).max(MIN_CROP_PX).min(w);
                    let ch = (h * 3 / 4).max(MIN_CROP_PX).min(h);
                    self.crop_rect = Some(((w - cw) / 2, (h - ch) / 2, cw, ch));
                }
            }
            if self.crop_enabled {
                if let Some(( _, _, cw, ch)) = self.crop_rect {
                    ui.weak(format!("{cw}×{ch}"));
                }
                if ui
                    .small_button(t!("Limpar recorte", "Clear crop"))
                    .on_hover_text(t!(
                        "Desativa o recorte e mantém o quadro inteiro.",
                        "Disables cropping and keeps the full frame."
                    ))
                    .clicked()
                {
                    self.crop_enabled = false;
                    self.crop_rect = None;
                    self.crop_drag = None;
                }
            }

            ui.separator();

            // GIF settings + action
            ui.menu_button(
                format!(
                    "🎞 GIF ({} fps, {})",
                    self.gif_fps,
                    if self.gif_width == 0 {
                        t!("Orig.", "Orig.").to_owned()
                    } else {
                        format!("{}px", self.gif_width)
                    }
                ),
                |ui| {
                    ui.strong(t!("Opções do GIF animado", "Animated GIF options"));
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(t!("Quadros/s (FPS):", "Frames/s (FPS):"));
                        egui::ComboBox::from_id_salt("ve_gif_fps")
                            .width(65.0)
                            .selected_text(format!("{} fps", self.gif_fps))
                            .show_ui(ui, |ui| {
                                for fps in [8, 12, 15, 24, 30] {
                                    ui.selectable_value(&mut self.gif_fps, fps, format!("{fps} fps"));
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label(t!("Largura máxima:", "Max width:"));
                        let w_label = |w: u32| {
                            if w == 0 {
                                t!("Original", "Original").to_owned()
                            } else {
                                format!("{w} px")
                            }
                        };
                        egui::ComboBox::from_id_salt("ve_gif_width")
                            .width(90.0)
                            .selected_text(w_label(self.gif_width))
                            .show_ui(ui, |ui| {
                                for w in [320, 480, 720, 960, 0] {
                                    ui.selectable_value(&mut self.gif_width, w, w_label(w));
                                }
                            });
                    });
                    ui.add_space(4.0);
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(t!(
                                "🎞 Criar GIF na biblioteca",
                                "🎞 Create GIF in library"
                            )),
                        )
                        .clicked()
                    {
                        action = Some(VideoAction::ExportGifToLibrary {
                            opts: self.current_gif_options(),
                        });
                        ui.close();
                    }
                },
            )
            .response
            .on_hover_text(t!(
                "Configura FPS e largura para exportar o trecho selecionado como GIF animado.",
                "Configure FPS and width to export the selected clip as an animated GIF."
            ));

            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(t!("🎞 Criar GIF na biblioteca", "🎞 Create GIF in library")),
                )
                .on_hover_text(t!(
                    "Converte o trecho selecionado em um GIF otimizado com paleta sob medida e adiciona à biblioteca.",
                    "Converts the selected clip into a palette-optimized GIF and adds it to the library."
                ))
                .clicked()
            {
                action = Some(VideoAction::ExportGifToLibrary {
                    opts: self.current_gif_options(),
                });
            }

            ui.separator();

            // Save clip to library
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(t!(
                        "✂ Salvar trecho na biblioteca",
                        "✂ Save clip to library"
                    )),
                )
                .on_hover_text(t!(
                    "Salva um novo vídeo MP4 na biblioteca com o corte, recorte, velocidade e áudio escolhidos.",
                    "Saves a new MP4 video in the library with the chosen trim, crop, speed, and audio settings."
                ))
                .clicked()
            {
                action = Some(VideoAction::SaveClipToLibrary {
                    opts: self.current_video_options(),
                });
            }

            // Overwrite in library
            let can_overwrite = !busy && self.has_edits();
            if ui
                .add_enabled(
                    can_overwrite,
                    egui::Button::new(t!("💾 Aplicar neste vídeo", "💾 Apply to this video")),
                )
                .on_hover_text(t!(
                    "Aplica as edições diretamente neste vídeo da biblioteca.",
                    "Applies the edits directly to this video in the library."
                ))
                .clicked()
            {
                action = Some(VideoAction::OverwriteInLibrary {
                    opts: self.current_video_options(),
                });
            }

            // Export MP4 As...
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(t!("📂 Exportar MP4…", "📂 Export MP4…")),
                )
                .on_hover_text(t!(
                    "Escolhe onde salvar uma cópia MP4 editada.",
                    "Choose where to save an edited MP4 copy."
                ))
                .clicked()
            {
                action = Some(VideoAction::ExportMp4As {
                    opts: self.current_video_options(),
                });
            }

            // Capture current frame into Image Annotation Editor
            let has_frame = self.current_frame.is_some();
            if ui
                .add_enabled(
                    !busy && has_frame,
                    egui::Button::new(t!("📸 Quadro → Editor", "📸 Frame → Editor")),
                )
                .on_hover_text(t!(
                    "Captura o quadro exibido agora e abre no editor de anotações (setas, textos, destaque, OCR).",
                    "Captures the currently displayed frame and opens it in the annotation editor (arrows, text, highlight, OCR)."
                ))
                .clicked()
            {
                if self.is_playing {
                    let ctx = ui.ctx().clone();
                    self.pause_playback(&ctx);
                }
                let mut full_frame = self.current_frame.clone();
                if let Some(ref f) = full_frame
                    && self.info.width > 0
                    && self.info.height > 0
                    && (f.width() != self.info.width || f.height() != self.info.height)
                    && let Ok(hi_res) = record::extract_frame_fast(
                        &self.path,
                        self.playhead,
                        self.info.width,
                        self.info.height,
                    )
                {
                    self.current_frame = Some(hi_res.clone());
                    full_frame = Some(hi_res);
                }
                if let Some(mut frame) = full_frame {
                    if self.crop_enabled
                        && let Some((cx, cy, cw, ch)) = self.crop_rect
                        && cw >= MIN_CROP_PX
                        && ch >= MIN_CROP_PX
                        && cx + cw <= frame.width()
                        && cy + ch <= frame.height()
                    {
                        frame = image::imageops::crop_imm(&frame, cx, cy, cw, ch).to_image();
                    }
                    action = Some(VideoAction::CaptureFrame { image: frame });
                }
            }

            if busy {
                ui.spinner();
                ui.colored_label(
                    self.accent,
                    t!("Processando vídeo…", "Processing video…"),
                );
            }
        });

        action
    }

    /// Renders the central video preview canvas (with interactive crop overlay) and the bottom
    /// visual filmstrip timeline & transport bar.
    pub fn canvas_ui(&mut self, ui: &mut egui::Ui) -> Option<VideoAction> {
        let ctx = ui.ctx().clone();
        self.poll_background(&ctx);
        self.handle_keyboard_shortcuts(&ctx);

        let mut action = None;

        // Bottom timeline + transport panel (~126px tall)
        egui::Panel::bottom("video_timeline_panel")
            .resizable(false)
            .exact_size(126.0)
            .show(ui, |ui| {
                if let Some(act) = self.timeline_panel_ui(ui) {
                    action = Some(act);
                }
            });

        // Main preview canvas
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(ui.visuals().extreme_bg_color))
            .show(ui, |ui| {
                self.preview_canvas_ui(ui);
            });

        action
    }

    /// Keyboard shortcuts when typing is not active: Space (play/pause), Left/Right (±0.1s),
    /// Home/End (jump to trim start/end), `[` / `]` (set trim start/end at playhead).
    fn handle_keyboard_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.busy {
            return;
        }
        let duration = self.info.duration.max(0.0);
        let mut toggle = false;
        let mut seek_to = None;

        ctx.input(|i| {
            if i.key_pressed(egui::Key::Space) {
                toggle = true;
            }
            if i.key_pressed(egui::Key::ArrowLeft) {
                let step = if i.modifiers.shift { 1.0 } else { 0.1 };
                seek_to = Some((self.playhead - step).max(0.0));
            }
            if i.key_pressed(egui::Key::ArrowRight) && duration > 0.0 {
                let step = if i.modifiers.shift { 1.0 } else { 0.1 };
                seek_to = Some((self.playhead + step).min(duration));
            }
            if i.key_pressed(egui::Key::Home) {
                seek_to = Some(self.trim_start);
            }
            if i.key_pressed(egui::Key::End) && duration > 0.0 {
                seek_to = Some(self.trim_end);
            }
            if i.key_pressed(egui::Key::OpenBracket) && duration > MIN_TRIM_GAP {
                self.trim_start = self.playhead.clamp(0.0, (self.trim_end - MIN_TRIM_GAP).max(0.0));
            }
            if i.key_pressed(egui::Key::CloseBracket) && duration > MIN_TRIM_GAP {
                self.trim_end = self
                    .playhead
                    .clamp((self.trim_start + MIN_TRIM_GAP).min(duration), duration);
            }
        });

        if toggle {
            self.toggle_play(ctx);
        }
        if let Some(ts) = seek_to {
            self.is_playing = false;
            self.stream = None;
            self.playhead = ts;
            self.last_requested_frame = -1.0;
            self.request_frame(ctx, ts);
        }
    }

    fn toggle_play(&mut self, ctx: &egui::Context) {
        if self.is_playing {
            self.pause_playback(ctx);
        } else {
            self.start_playback(ctx);
        }
    }

    /// Draws the video preview centered in the canvas, plus the interactive crop overlay if enabled.
    fn preview_canvas_ui(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_rect_before_wrap();
        let sense = if self.crop_enabled {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        let (_, response) = ui.allocate_exact_size(avail.size(), sense);
        let painter = ui.painter_at(avail);

        // Subtle backdrop
        painter.rect_filled(avail, 0.0, Color32::from_rgb(14, 15, 19));

        let vid_w = self.info.width.max(1) as f32;
        let vid_h = self.info.height.max(1) as f32;
        let pad = 18.0;
        let inner = avail.shrink(pad);
        if inner.width() <= 10.0 || inner.height() <= 10.0 {
            return;
        }

        let scale = (inner.width() / vid_w).min(inner.height() / vid_h);
        let draw_size = Vec2::new(vid_w * scale, vid_h * scale);
        let video_rect = Rect::from_center_size(inner.center(), draw_size);

        // Drop shadow & frame border around the video canvas
        painter.rect_filled(
            video_rect.translate(Vec2::new(0.0, 4.0)).expand(2.0),
            4.0,
            Color32::from_black_alpha(110),
        );

        if let Some(tex) = &self.preview_tex {
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            painter.image(tex.id(), video_rect, uv, Color32::WHITE);
        } else {
            painter.rect_filled(video_rect, 2.0, Color32::from_rgb(26, 28, 34));
            painter.text(
                video_rect.center(),
                egui::Align2::CENTER_CENTER,
                t!("Carregando quadro do vídeo…", "Loading video frame…"),
                egui::FontId::proportional(14.0),
                Color32::from_gray(170),
            );
        }

        painter.rect_stroke(
            video_rect,
            2.0,
            Stroke::new(1.0, Color32::from_white_alpha(35)),
            StrokeKind::Outside,
        );

        // Top-left info pill badge (resolution, FPS, audio, speed)
        let info_badge = format!(
            "{}×{}  •  {:.0} fps  •  {}",
            self.info.width,
            self.info.height,
            self.info.fps.max(1.0),
            if !self.info.has_audio {
                t!("Sem áudio", "No audio")
            } else if self.mute {
                t!("Áudio mutado", "Audio muted")
            } else {
                t!("Com áudio", "With audio")
            }
        );
        let badge_pos = video_rect.min + Vec2::new(10.0, 10.0);
        let galley = painter.layout_no_wrap(
            info_badge,
            egui::FontId::proportional(11.5),
            Color32::from_gray(230),
        );
        let pill_rect = Rect::from_min_size(badge_pos, galley.size() + Vec2::new(14.0, 6.0));
        painter.rect_filled(pill_rect, 5.0, Color32::from_black_alpha(165));
        painter.galley(badge_pos + Vec2::new(7.0, 3.0), galley, Color32::WHITE);

        // Interactive Crop Overlay
        if self.crop_enabled {
            self.crop_overlay_ui(ui, &painter, &response, video_rect, scale);
        } else if response.clicked() && !self.busy {
            // Clicking on the video frame toggles play/pause when crop mode is off
            let ctx = ui.ctx().clone();
            self.toggle_play(&ctx);
        }
    }

    /// Handles pointer interaction and drawing for the interactive crop rectangle on the preview canvas.
    fn crop_overlay_ui(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        video_rect: Rect,
        scale: f32,
    ) {
        let vid_w = self.info.width.max(MIN_CROP_PX);
        let vid_h = self.info.height.max(MIN_CROP_PX);

        let to_screen = |(x, y): (u32, u32)| -> Pos2 {
            Pos2::new(
                video_rect.min.x + x as f32 * scale,
                video_rect.min.y + y as f32 * scale,
            )
        };
        let to_video_f32 = |p: Pos2| -> (f32, f32) {
            let vx = ((p.x - video_rect.min.x) / scale).clamp(0.0, vid_w as f32);
            let vy = ((p.y - video_rect.min.y) / scale).clamp(0.0, vid_h as f32);
            (vx, vy)
        };

        let crop_screen_rect = self.crop_rect.map(|(x, y, w, h)| {
            Rect::from_min_max(to_screen((x, y)), to_screen((x + w, y + h)))
        });

        // Handle pointer drag start / update / end
        if response.drag_started()
            && let Some(pos) = response.interact_pointer_pos()
        {
            if let (Some(c_rect), Some(crop)) = (crop_screen_rect, self.crop_rect) {
                let handles = crop_handle_rects(c_rect);
                if let Some(idx) = handles.iter().position(|r| r.expand(4.0).contains(pos)) {
                    self.crop_drag = Some(CropDrag::Resize {
                        handle: idx,
                        start: crop,
                    });
                } else if c_rect.contains(pos) {
                    let (vx, vy) = to_video_f32(pos);
                    self.crop_drag = Some(CropDrag::Move {
                        grab: (vx - crop.0 as f32, vy - crop.1 as f32),
                        start: crop,
                    });
                } else if video_rect.contains(pos) {
                    self.crop_drag = Some(CropDrag::New {
                        origin: to_video_f32(pos),
                    });
                }
            } else if video_rect.contains(pos) {
                self.crop_drag = Some(CropDrag::New {
                    origin: to_video_f32(pos),
                });
            }
        }

        if response.dragged()
            && let (Some(drag), Some(pos)) = (self.crop_drag, response.interact_pointer_pos())
        {
            let (vx, vy) = to_video_f32(pos);
            match drag {
                CropDrag::New { origin } => {
                    let x0 = origin.0.min(vx).round() as u32;
                    let y0 = origin.1.min(vy).round() as u32;
                    let x1 = origin.0.max(vx).round() as u32;
                    let y1 = origin.1.max(vy).round() as u32;
                    let w = (x1.saturating_sub(x0)).max(MIN_CROP_PX).min(vid_w) & !1;
                    let h = (y1.saturating_sub(y0)).max(MIN_CROP_PX).min(vid_h) & !1;
                    let x = x0.min(vid_w.saturating_sub(w)) & !1;
                    let y = y0.min(vid_h.saturating_sub(h)) & !1;
                    self.crop_rect = Some((x, y, w.max(2), h.max(2)));
                }
                CropDrag::Move { grab, start } => {
                    let (_, _, w, h) = start;
                    let max_x = vid_w.saturating_sub(w) as f32;
                    let max_y = vid_h.saturating_sub(h) as f32;
                    let nx = (vx - grab.0).clamp(0.0, max_x).round() as u32 & !1;
                    let ny = (vy - grab.1).clamp(0.0, max_y).round() as u32 & !1;
                    self.crop_rect = Some((nx, ny, w, h));
                }
                CropDrag::Resize { handle, start } => {
                    self.crop_rect = Some(resize_crop_rect(start, handle, (vx, vy), (vid_w, vid_h)));
                }
            }
        }

        if response.drag_stopped() {
            self.crop_drag = None;
        }

        // Update hover cursor for handles / interior
        if let Some(pos) = response.hover_pos()
            && let Some(c_rect) = crop_screen_rect
        {
            let handles = crop_handle_rects(c_rect);
            let cursors = [
                egui::CursorIcon::ResizeNorthWest,
                egui::CursorIcon::ResizeNorth,
                egui::CursorIcon::ResizeNorthEast,
                egui::CursorIcon::ResizeEast,
                egui::CursorIcon::ResizeSouthEast,
                egui::CursorIcon::ResizeSouth,
                egui::CursorIcon::ResizeSouthWest,
                egui::CursorIcon::ResizeWest,
            ];
            if let Some(idx) = handles.iter().position(|r| r.expand(3.0).contains(pos)) {
                ui.ctx().set_cursor_icon(cursors[idx]);
            } else if c_rect.contains(pos) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            } else if video_rect.contains(pos) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
        }

        // Draw darkened mask outside crop area, rule-of-thirds grid, and 8 resize handles
        if let Some((cx, cy, cw, ch)) = self.crop_rect {
            let c_rect = Rect::from_min_max(to_screen((cx, cy)), to_screen((cx + cw, cy + ch)));
            let mask = Color32::from_black_alpha(150);

            // Top, bottom, left, right dimmed strips around `c_rect`
            if c_rect.min.y > video_rect.min.y {
                painter.rect_filled(
                    Rect::from_min_max(video_rect.min, Pos2::new(video_rect.max.x, c_rect.min.y)),
                    0.0,
                    mask,
                );
            }
            if c_rect.max.y < video_rect.max.y {
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(video_rect.min.x, c_rect.max.y), video_rect.max),
                    0.0,
                    mask,
                );
            }
            if c_rect.min.x > video_rect.min.x {
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(video_rect.min.x, c_rect.min.y),
                        Pos2::new(c_rect.min.x, c_rect.max.y),
                    ),
                    0.0,
                    mask,
                );
            }
            if c_rect.max.x < video_rect.max.x {
                painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(c_rect.max.x, c_rect.min.y),
                        Pos2::new(video_rect.max.x, c_rect.max.y),
                    ),
                    0.0,
                    mask,
                );
            }

            // Rule of thirds lines
            let grid_stroke = Stroke::new(1.0, Color32::from_white_alpha(70));
            for i in 1..=2 {
                let t = i as f32 / 3.0;
                let gx = egui::lerp(c_rect.min.x..=c_rect.max.x, t);
                let gy = egui::lerp(c_rect.min.y..=c_rect.max.y, t);
                painter.line_segment(
                    [Pos2::new(gx, c_rect.min.y), Pos2::new(gx, c_rect.max.y)],
                    grid_stroke,
                );
                painter.line_segment(
                    [Pos2::new(c_rect.min.x, gy), Pos2::new(c_rect.max.x, gy)],
                    grid_stroke,
                );
            }

            // Accent border around crop rect
            painter.rect_stroke(
                c_rect,
                0.0,
                Stroke::new(2.0, self.accent),
                StrokeKind::Inside,
            );

            // Handles (4 corners + 4 midpoints)
            for h_rect in crop_handle_rects(c_rect) {
                painter.rect_filled(h_rect, 2.0, self.accent);
                painter.rect_stroke(
                    h_rect,
                    2.0,
                    Stroke::new(1.0, theme::text_on(self.accent)),
                    StrokeKind::Outside,
                );
            }

            // Dimensions pill label
            let dim_label = format!("{cw} × {ch} px");
            let label_pos = Pos2::new(c_rect.center().x, (c_rect.max.y + 14.0).min(video_rect.max.y - 10.0));
            let galley = painter.layout_no_wrap(
                dim_label,
                egui::FontId::proportional(11.5),
                theme::text_on(self.accent),
            );
            let badge = Rect::from_center_size(label_pos, galley.size() + Vec2::new(12.0, 5.0));
            painter.rect_filled(badge, 4.0, self.accent);
            painter.galley(
                badge.min + Vec2::new(6.0, 2.5),
                galley,
                theme::text_on(self.accent),
            );
        }
    }

    /// Renders the transport controls row and the interactive visual filmstrip bar at the bottom.
    fn timeline_panel_ui(&mut self, ui: &mut egui::Ui) -> Option<VideoAction> {
        let ctx = ui.ctx().clone();
        let duration = self.info.duration.max(0.0);
        let busy = self.busy;

        ui.add_space(4.0);

        // Row 1: Transport controls + Timecode readout + Open in system player
        ui.horizontal(|ui| {
            // Jump to Trim Start
            if ui
                .add_enabled(!busy && duration > 0.0, egui::Button::new(t!("⏮ Início", "⏮ Start")))
                .on_hover_text(t!(
                    "Pula para o início do trecho selecionado (Home).",
                    "Jump to the start of the selected clip (Home)."
                ))
                .clicked()
            {
                self.is_playing = false;
                self.stream = None;
                self.playhead = self.trim_start;
                self.last_requested_frame = -1.0;
                self.request_frame(&ctx, self.playhead);
            }

            // Step -0.1s
            if ui
                .add_enabled(!busy && duration > 0.0, egui::Button::new("◀ -0.1s"))
                .on_hover_text(t!(
                    "Recua 0,1 segundo (Seta esquerda).",
                    "Step back 0.1 seconds (Left arrow)."
                ))
                .clicked()
            {
                self.is_playing = false;
                self.stream = None;
                self.playhead = (self.playhead - 0.1).max(0.0);
                self.last_requested_frame = -1.0;
                self.request_frame(&ctx, self.playhead);
            }

            // Play / Pause preview
            let play_label = if self.is_playing {
                t!("⏸ Pausar", "⏸ Pause")
            } else {
                t!("▶ Reproduzir", "▶ Play")
            };
            if ui
                .add_enabled(
                    !busy && duration > 0.0,
                    egui::Button::new(play_label).selected(self.is_playing),
                )
                .on_hover_text(t!(
                    "Reproduz ou pausa a prévia visual do trecho (Espaço).",
                    "Play or pause the visual clip preview (Space)."
                ))
                .clicked()
            {
                self.toggle_play(&ctx);
            }

            // Step +0.1s
            if ui
                .add_enabled(!busy && duration > 0.0, egui::Button::new("▶ +0.1s"))
                .on_hover_text(t!(
                    "Avança 0,1 segundo (Seta direita).",
                    "Step forward 0.1 seconds (Right arrow)."
                ))
                .clicked()
            {
                self.is_playing = false;
                self.stream = None;
                self.playhead = (self.playhead + 0.1).min(duration);
                self.last_requested_frame = -1.0;
                self.request_frame(&ctx, self.playhead);
            }

            // Jump to Trim End
            if ui
                .add_enabled(!busy && duration > 0.0, egui::Button::new(t!("⏭ Fim", "⏭ End")))
                .on_hover_text(t!(
                    "Pula para o fim do trecho selecionado (End).",
                    "Jump to the end of the selected clip (End)."
                ))
                .clicked()
            {
                self.is_playing = false;
                self.stream = None;
                self.playhead = self.trim_end;
                self.last_requested_frame = -1.0;
                self.request_frame(&ctx, self.playhead);
            }

            ui.separator();

            // Set In `[` and Set Out `]` at playhead buttons
            if ui
                .add_enabled(
                    !busy && duration > MIN_TRIM_GAP,
                    egui::Button::new(t!("[ Marcar início", "[ Mark In")),
                )
                .on_hover_text(t!(
                    "Define o início do corte na posição atual da agulha (tecla [).",
                    "Sets trim start at the current playhead position ([ key)."
                ))
                .clicked()
            {
                self.trim_start = self
                    .playhead
                    .clamp(0.0, (self.trim_end - MIN_TRIM_GAP).max(0.0));
            }
            if ui
                .add_enabled(
                    !busy && duration > MIN_TRIM_GAP,
                    egui::Button::new(t!("Marcar fim ]", "Mark Out ]")),
                )
                .on_hover_text(t!(
                    "Define o fim do corte na posição atual da agulha (tecla ]).
",
                    "Sets trim end at the current playhead position (] key)."
                ))
                .clicked()
            {
                self.trim_end = self
                    .playhead
                    .clamp((self.trim_start + MIN_TRIM_GAP).min(duration), duration);
            }

            if (self.trim_start > 0.01 || (duration > 0.0 && (duration - self.trim_end) > 0.01))
                && ui
                    .small_button(t!("Vídeo inteiro", "Full video"))
                    .on_hover_text(t!(
                        "Restaura o trecho para a duração completa do vídeo.",
                        "Resets the trim handles to the full video duration."
                    ))
                    .clicked()
            {
                self.trim_start = 0.0;
                self.trim_end = duration;
            }

            ui.separator();

            // Timecode readout
            let clip_dur = (self.trim_end - self.trim_start).max(0.0) / self.speed.max(0.1);
            ui.strong(format!(
                "{} / {}",
                format_timecode(self.playhead),
                format_timecode(duration)
            ));
            ui.colored_label(
                self.accent,
                tf!(
                    "Trecho: {} → {} ({:.1}s)",
                    "Clip: {} → {} ({:.1}s)",
                    format_timecode(self.trim_start),
                    format_timecode(self.trim_end),
                    clip_dur
                ),
            );

            // Right-aligned system player button
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(t!("🔊 Abrir no player", "🔊 Open in player"))
                    .on_hover_text(t!(
                        "Abre o vídeo no reprodutor padrão do Windows com som e taxa máxima de quadros.",
                        "Opens the video in the default Windows media player with full audio and frame rate."
                    ))
                    .clicked()
                {
                    let _ = std::process::Command::new("explorer").arg(&self.path).spawn();
                }
            });
        });

        ui.add_space(6.0);

        // Row 2: Visual Filmstrip Timeline bar (56px tall + 12px top/bottom handle margin)
        self.filmstrip_bar_ui(ui, &ctx);

        None
    }

    /// Draws the 56px interactive filmstrip bar with trim handles `[` and `]` and the scrub playhead.
    fn filmstrip_bar_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let duration = self.info.duration.max(0.1);
        let avail_w = ui.available_width().max(120.0);
        let total_h = FILMSTRIP_HEIGHT as f32 + 16.0;
        let (outer_rect, response) =
            ui.allocate_exact_size(Vec2::new(avail_w, total_h), Sense::click_and_drag());

        let bar_rect = Rect::from_min_max(
            Pos2::new(outer_rect.min.x + 10.0, outer_rect.min.y + 10.0),
            Pos2::new(outer_rect.max.x - 10.0, outer_rect.max.y - 6.0),
        );
        let bar_w = bar_rect.width().max(1.0);

        let time_to_x = |t: f64| -> f32 {
            bar_rect.min.x + ((t / duration).clamp(0.0, 1.0) as f32) * bar_w
        };
        let x_to_time = |x: f32| -> f64 {
            (((x - bar_rect.min.x) / bar_w).clamp(0.0, 1.0) as f64) * duration
        };

        let start_x = time_to_x(self.trim_start);
        let end_x = time_to_x(self.trim_end);

        let trim_start_hit = Rect::from_center_size(
            Pos2::new(start_x, bar_rect.center().y),
            Vec2::new(16.0, bar_rect.height() + 8.0),
        );
        let trim_end_hit = Rect::from_center_size(
            Pos2::new(end_x, bar_rect.center().y),
            Vec2::new(16.0, bar_rect.height() + 8.0),
        );

        // Pointer interaction on filmstrip timeline
        if !self.busy {
            if response.drag_started()
                && let Some(pos) = response.interact_pointer_pos()
            {
                self.is_playing = false;
                self.stream = None;
                if trim_start_hit.contains(pos) && (pos.x - start_x).abs() <= (pos.x - end_x).abs() {
                    self.timeline_drag = Some(TimelineDrag::TrimStart);
                } else if trim_end_hit.contains(pos) {
                    self.timeline_drag = Some(TimelineDrag::TrimEnd);
                } else {
                    self.timeline_drag = Some(TimelineDrag::Playhead);
                    let t = x_to_time(pos.x);
                    self.playhead = t;
                    self.last_requested_frame = -1.0;
                    self.request_frame(ctx, t);
                }
            }

            if (response.dragged() || response.clicked())
                && let Some(pos) = response.interact_pointer_pos()
            {
                self.is_playing = false;
                self.stream = None;
                let t = x_to_time(pos.x);
                match self.timeline_drag.unwrap_or(TimelineDrag::Playhead) {
                    TimelineDrag::TrimStart => {
                        self.trim_start = t.clamp(0.0, (self.trim_end - MIN_TRIM_GAP).max(0.0));
                        self.playhead = self.trim_start;
                        self.request_frame(ctx, self.playhead);
                    }
                    TimelineDrag::TrimEnd => {
                        self.trim_end =
                            t.clamp((self.trim_start + MIN_TRIM_GAP).min(duration), duration);
                        self.playhead = self.trim_end;
                        self.request_frame(ctx, self.playhead);
                    }
                    TimelineDrag::Playhead => {
                        self.playhead = t;
                        self.request_frame(ctx, t);
                    }
                }
            }

            if response.drag_stopped() {
                self.timeline_drag = None;
                self.last_requested_frame = -1.0;
                self.request_frame(ctx, self.playhead);
            }

            if let Some(pos) = response.hover_pos() {
                if trim_start_hit.contains(pos) || trim_end_hit.contains(pos) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                } else if bar_rect.expand(4.0).contains(pos) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
        }

        let painter = ui.painter_at(outer_rect);

        // Background track
        painter.rect_filled(bar_rect, 6.0, Color32::from_rgb(20, 22, 28));

        // Draw filmstrip thumbnails across the bar
        if !self.filmstrip_tex.is_empty() {
            let n = self.filmstrip_tex.len();
            let seg_w = bar_w / n as f32;
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            for (i, tex) in self.filmstrip_tex.iter().enumerate() {
                let x0 = bar_rect.min.x + i as f32 * seg_w;
                let cell = Rect::from_min_max(
                    Pos2::new(x0, bar_rect.min.y),
                    Pos2::new(x0 + seg_w, bar_rect.max.y),
                );
                painter.image(tex.id(), cell, uv, Color32::WHITE);
            }
        } else {
            let msg = if self.filmstrip_loading {
                t!("Gerando tira de quadros…", "Generating filmstrip…")
            } else {
                t!("Linha do tempo", "Timeline")
            };
            painter.text(
                bar_rect.center(),
                egui::Align2::CENTER_CENTER,
                msg,
                egui::FontId::proportional(11.5),
                Color32::from_gray(150),
            );
        }

        // Darken trimmed-out regions outside [trim_start, trim_end]
        let start_x = time_to_x(self.trim_start);
        let end_x = time_to_x(self.trim_end);
        let playhead_x = time_to_x(self.playhead);
        let dim_color = Color32::from_black_alpha(175);

        if start_x > bar_rect.min.x {
            painter.rect_filled(
                Rect::from_min_max(bar_rect.min, Pos2::new(start_x, bar_rect.max.y)),
                0.0,
                dim_color,
            );
        }
        if end_x < bar_rect.max.x {
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(end_x, bar_rect.min.y), bar_rect.max),
                0.0,
                dim_color,
            );
        }

        // Active clip bracket frame + handles `[` and `]`
        let active_rect = Rect::from_min_max(
            Pos2::new(start_x, bar_rect.min.y),
            Pos2::new(end_x, bar_rect.max.y),
        );
        painter.rect_stroke(
            active_rect,
            3.0,
            Stroke::new(2.0, self.accent),
            StrokeKind::Inside,
        );

        let grip_color = theme::text_on(self.accent);
        for hx in [start_x, end_x] {
            let handle_rect = Rect::from_center_size(
                Pos2::new(hx, bar_rect.center().y),
                Vec2::new(8.0, bar_rect.height() + 4.0),
            );
            painter.rect_filled(handle_rect, 3.0, self.accent);
            // Vertical grip notch inside handle
            painter.line_segment(
                [
                    Pos2::new(hx, bar_rect.center().y - 9.0),
                    Pos2::new(hx, bar_rect.center().y + 9.0),
                ],
                Stroke::new(1.5, grip_color),
            );
        }

        // Playhead vertical line + top knob
        let playhead_color = Color32::WHITE;
        painter.line_segment(
            [
                Pos2::new(playhead_x, bar_rect.min.y - 6.0),
                Pos2::new(playhead_x, bar_rect.max.y + 3.0),
            ],
            Stroke::new(2.5, playhead_color),
        );
        let knob_rect = Rect::from_center_size(
            Pos2::new(playhead_x, bar_rect.min.y - 4.0),
            Vec2::new(12.0, 9.0),
        );
        painter.rect_filled(knob_rect, 3.0, playhead_color);
        painter.rect_stroke(
            knob_rect,
            3.0,
            Stroke::new(1.0, Color32::from_black_alpha(180)),
            StrokeKind::Outside,
        );
    }
}

/// Formats seconds as `MM:SS.d` (e.g., `00:02.4`).
fn format_timecode(secs: f64) -> String {
    let s = secs.max(0.0);
    let mins = (s / 60.0).floor() as u32;
    let rem = s - (mins as f64 * 60.0);
    format!("{mins:02}:{rem:04.1}")
}

/// Returns the 8 resize handle rectangles (NW, N, NE, E, SE, S, SW, W) around `rect`.
fn crop_handle_rects(rect: Rect) -> [Rect; 8] {
    let size = Vec2::new(10.0, 10.0);
    let at = |p: Pos2| Rect::from_center_size(p, size);
    let c = rect.center();
    [
        at(rect.left_top()),
        at(Pos2::new(c.x, rect.min.y)),
        at(rect.right_top()),
        at(Pos2::new(rect.max.x, c.y)),
        at(rect.right_bottom()),
        at(Pos2::new(c.x, rect.max.y)),
        at(rect.left_bottom()),
        at(Pos2::new(rect.min.x, c.y)),
    ]
}

/// Resizes `(x, y, w, h)` in video pixel coordinates by dragging `handle` to `(vx, vy)`,
/// enforcing `MIN_CROP_PX` and even dimensions for H.264 compatibility.
fn resize_crop_rect(
    start: (u32, u32, u32, u32),
    handle: usize,
    to: (f32, f32),
    bounds: (u32, u32),
) -> (u32, u32, u32, u32) {
    let (sx, sy, sw, sh) = start;
    let (left0, top0, right0, bottom0) = (
        sx as f32,
        sy as f32,
        (sx + sw) as f32,
        (sy + sh) as f32,
    );
    let min_s = MIN_CROP_PX as f32;
    let max_w = bounds.0 as f32;
    let max_h = bounds.1 as f32;

    let west = to.0.clamp(0.0, (right0 - min_s).max(0.0));
    let east = to.0.clamp((left0 + min_s).min(max_w), max_w);
    let north = to.1.clamp(0.0, (bottom0 - min_s).max(0.0));
    let south = to.1.clamp((top0 + min_s).min(max_h), max_h);

    let (l, t, r, b) = match handle {
        0 => (west, north, right0, bottom0),
        1 => (left0, north, right0, bottom0),
        2 => (left0, north, east, bottom0),
        3 => (left0, top0, east, bottom0),
        4 => (left0, top0, east, south),
        5 => (left0, top0, right0, south),
        6 => (west, top0, right0, south),
        _ => (west, top0, right0, bottom0),
    };

    let x = (l.round() as u32).min(bounds.0.saturating_sub(MIN_CROP_PX)) & !1;
    let y = (t.round() as u32).min(bounds.1.saturating_sub(MIN_CROP_PX)) & !1;
    let w = ((r - l).round() as u32)
        .max(MIN_CROP_PX)
        .min(bounds.0.saturating_sub(x))
        & !1;
    let h = ((b - t).round() as u32)
        .max(MIN_CROP_PX)
        .min(bounds.1.saturating_sub(y))
        & !1;
    (x, y, w.max(2), h.max(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timecode_formats_minutes_and_tenths() {
        assert_eq!(format_timecode(0.0), "00:00.0");
        assert_eq!(format_timecode(2.44), "00:02.4");
        assert_eq!(format_timecode(65.2), "01:05.2");
    }

    #[test]
    fn crop_resize_respects_bounds_and_even_dimensions() {
        let start = (100, 100, 400, 300);
        // Dragging SE handle (4) to (601, 451) snaps to even width/height within 1920x1080
        let (x, y, w, h) = resize_crop_rect(start, 4, (601.0, 451.0), (1920, 1080));
        assert_eq!((x, y), (100, 100));
        assert_eq!(w % 2, 0);
        assert_eq!(h % 2, 0);
        assert!(w >= MIN_CROP_PX && h >= MIN_CROP_PX);

        // Dragging NW handle (0) past SE corner clamps to MIN_CROP_PX
        let (x2, y2, w2, h2) = resize_crop_rect(start, 0, (900.0, 900.0), (1920, 1080));
        assert_eq!(x2 + w2, 500);
        assert_eq!(y2 + h2, 400);
        assert_eq!(w2, MIN_CROP_PX);
        assert_eq!(h2, MIN_CROP_PX);
    }
}
