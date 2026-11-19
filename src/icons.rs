//! Crisp monochrome vector icons rasterized once via `tiny-skia` on a 24×24 viewBox (48×48 px)
//! and cached as egui textures tinted at paint time.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect as SkiaRect, Stroke,
    StrokeDash, Transform,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    // Capture modes
    Region,
    FullScreen,
    AllScreens,
    Scrolling,
    Panoramic,
    Video,
    Gif,
    Steps,
    Options,

    // Global & editor actions
    Copy,
    Save,
    Pin,
    ReadText,
    AutoRedact,
    Library,
    Preferences,
    Undo,
    Redo,
    Styles,
    ImageOps,
    UpdateArrow,

    // 19 annotation tools
    Select,
    Eraser,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Pen,
    Highlighter,
    Text,
    Callout,
    StepTool,
    Stamp,
    Spotlight,
    Magnify,
    Pixelate,
    Blur,
    Crop,
    CutOut,
    GrabText,

    // Utility, library, video, alignment & z-order
    Eyedropper,
    Search,
    Close,
    Star,
    StarFilled,
    Trash,
    Folder,
    FilePdf,
    Duplicate,
    Play,
    Pause,
    Stop,
    RecordDot,
    VolumeOn,
    VolumeOff,
    CaptureFrame,
    ChevronDown,
    PanelLeft,
    Check,
    Refresh,
    Edit,
    Clipboard,
    ExternalOpen,
    CombineV,
    CombineH,
    StepBack,
    StepForward,
    SkipStart,
    SkipEnd,
    MarkIn,
    MarkOut,
    BringForward,
    SendBackward,
    BringToFront,
    SendToBack,
    AlignLeft,
    AlignCenterH,
    AlignRight,
    AlignTop,
    AlignCenterV,
    AlignBottom,
    DistributeH,
    DistributeV,
}

impl Icon {
    #[cfg(test)]
    pub const ALL: &'static [Icon] = &[
        Icon::Region,
        Icon::FullScreen,
        Icon::AllScreens,
        Icon::Scrolling,
        Icon::Panoramic,
        Icon::Video,
        Icon::Gif,
        Icon::Steps,
        Icon::Options,
        Icon::Copy,
        Icon::Save,
        Icon::Pin,
        Icon::ReadText,
        Icon::AutoRedact,
        Icon::Library,
        Icon::Preferences,
        Icon::Undo,
        Icon::Redo,
        Icon::Styles,
        Icon::ImageOps,
        Icon::UpdateArrow,
        Icon::Select,
        Icon::Eraser,
        Icon::Arrow,
        Icon::Line,
        Icon::Rect,
        Icon::Ellipse,
        Icon::Pen,
        Icon::Highlighter,
        Icon::Text,
        Icon::Callout,
        Icon::StepTool,
        Icon::Stamp,
        Icon::Spotlight,
        Icon::Magnify,
        Icon::Pixelate,
        Icon::Blur,
        Icon::Crop,
        Icon::CutOut,
        Icon::GrabText,
        Icon::Eyedropper,
        Icon::Search,
        Icon::Close,
        Icon::Star,
        Icon::StarFilled,
        Icon::Trash,
        Icon::Folder,
        Icon::FilePdf,
        Icon::Duplicate,
        Icon::Play,
        Icon::Pause,
        Icon::Stop,
        Icon::RecordDot,
        Icon::VolumeOn,
        Icon::VolumeOff,
        Icon::CaptureFrame,
        Icon::ChevronDown,
        Icon::PanelLeft,
        Icon::Check,
        Icon::Refresh,
        Icon::Edit,
        Icon::Clipboard,
        Icon::ExternalOpen,
        Icon::CombineV,
        Icon::CombineH,
        Icon::StepBack,
        Icon::StepForward,
        Icon::SkipStart,
        Icon::SkipEnd,
        Icon::MarkIn,
        Icon::MarkOut,
        Icon::BringForward,
        Icon::SendBackward,
        Icon::BringToFront,
        Icon::SendToBack,
        Icon::AlignLeft,
        Icon::AlignCenterH,
        Icon::AlignRight,
        Icon::AlignTop,
        Icon::AlignCenterV,
        Icon::AlignBottom,
        Icon::DistributeH,
        Icon::DistributeV,
    ];
}

pub fn for_tool(tool: crate::editor::Tool) -> Icon {
    use crate::editor::Tool;
    match tool {
        Tool::Select => Icon::Select,
        Tool::Erase => Icon::Eraser,
        Tool::Arrow => Icon::Arrow,
        Tool::Line => Icon::Line,
        Tool::Rect => Icon::Rect,
        Tool::Ellipse => Icon::Ellipse,
        Tool::Pen => Icon::Pen,
        Tool::Highlight => Icon::Highlighter,
        Tool::Text => Icon::Text,
        Tool::Callout => Icon::Callout,
        Tool::Step => Icon::StepTool,
        Tool::Stamp => Icon::Stamp,
        Tool::Spotlight => Icon::Spotlight,
        Tool::Magnify => Icon::Magnify,
        Tool::Pixelate => Icon::Pixelate,
        Tool::Blur => Icon::Blur,
        Tool::Crop => Icon::Crop,
        Tool::CutOut => Icon::CutOut,
        Tool::GrabText => Icon::GrabText,
    }
}

/// Returns a cached GPU texture for `icon` (48×48 anti-aliased white alpha mask).
pub fn texture(ctx: &egui::Context, icon: Icon) -> TextureHandle {
    let cache_id = egui::Id::unique("snapkit_icons");
    let cache: Arc<Mutex<HashMap<Icon, TextureHandle>>> = ctx.data_mut(|d| {
        d.get_temp::<Arc<Mutex<HashMap<Icon, TextureHandle>>>>(cache_id)
            .unwrap_or_else(|| {
                let map = Arc::new(Mutex::new(HashMap::new()));
                d.insert_temp(cache_id, map.clone());
                map
            })
    });
    if let Some(handle) = cache.lock().unwrap().get(&icon) {
        return handle.clone();
    }
    let rgba = rasterize(icon);
    let img = ColorImage::from_rgba_unmultiplied([48, 48], &rgba);
    let handle = ctx.load_texture(format!("icon_{icon:?}"), img, TextureOptions::LINEAR);
    cache.lock().unwrap().insert(icon, handle.clone());
    handle
}

/// Draws `icon` centered at `center` with `size` points and `tint` color.
pub fn paint(
    painter: &egui::Painter,
    ctx: &egui::Context,
    icon: Icon,
    center: Pos2,
    size: f32,
    tint: Color32,
) {
    let tex = texture(ctx, icon);
    let rect = Rect::from_center_size(center, Vec2::splat(size));
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    painter.image(tex.id(), rect, uv, tint);
}

fn white_paint() -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::WHITE);
    p.anti_alias = true;
    p
}

fn default_stroke() -> Stroke {
    Stroke {
        width: 1.85,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    }
}

struct Canvas {
    pm: Pixmap,
    xform: Transform,
    paint: Paint<'static>,
    stroke: Stroke,
}

impl Canvas {
    fn new() -> Self {
        Self {
            pm: Pixmap::new(48, 48).expect("48x48 icon pixmap"),
            xform: Transform::from_scale(2.0, 2.0),
            paint: white_paint(),
            stroke: default_stroke(),
        }
    }

    fn stroke_path(&mut self, path: &Path) {
        self.pm.stroke_path(path, &self.paint, &self.stroke, self.xform, None);
    }

    fn stroke_thin(&mut self, path: &Path, width: f32) {
        let mut s = self.stroke.clone();
        s.width = width;
        self.pm.stroke_path(path, &self.paint, &s, self.xform, None);
    }

    fn stroke_dashed(&mut self, path: &Path, on: f32, off: f32) {
        let mut s = self.stroke.clone();
        s.dash = StrokeDash::new(vec![on, off], 0.0);
        self.pm.stroke_path(path, &self.paint, &s, self.xform, None);
    }

    fn fill_path(&mut self, path: &Path) {
        self.pm.fill_path(path, &self.paint, FillRule::Winding, self.xform, None);
    }

    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32) {
        let mut pb = PathBuilder::new();
        pb.move_to(x1, y1);
        pb.line_to(x2, y2);
        if let Some(p) = pb.finish() {
            self.stroke_path(&p);
        }
    }

    fn poly(&mut self, pts: &[(f32, f32)], closed: bool) {
        let Some((&first, rest)) = pts.split_first() else { return };
        let mut pb = PathBuilder::new();
        pb.move_to(first.0, first.1);
        for &(x, y) in rest {
            pb.line_to(x, y);
        }
        if closed {
            pb.close();
        }
        if let Some(p) = pb.finish() {
            self.stroke_path(&p);
        }
    }

    fn fill_poly(&mut self, pts: &[(f32, f32)]) {
        let Some((&first, rest)) = pts.split_first() else { return };
        let mut pb = PathBuilder::new();
        pb.move_to(first.0, first.1);
        for &(x, y) in rest {
            pb.line_to(x, y);
        }
        pb.close();
        if let Some(p) = pb.finish() {
            self.fill_path(&p);
        }
    }

    fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        if let Some(p) = round_rect_path(x, y, w, h, r) {
            self.stroke_path(&p);
        }
    }

    fn fill_round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        if let Some(p) = round_rect_path(x, y, w, h, r) {
            self.fill_path(&p);
        }
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32) {
        if let Some(p) = PathBuilder::from_circle(cx, cy, r) {
            self.stroke_path(&p);
        }
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, r: f32) {
        if let Some(p) = PathBuilder::from_circle(cx, cy, r) {
            self.fill_path(&p);
        }
    }

    fn finish(self) -> Vec<u8> {
        self.pm
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect()
    }
}

fn round_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w * 0.5).min(h * 0.5);
    if r <= 0.1 {
        return SkiaRect::from_xywh(x, y, w, h).map(PathBuilder::from_rect);
    }
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Rasterizes `icon` into 48×48 non-premultiplied RGBA bytes (white shapes on transparent).
pub fn rasterize(icon: Icon) -> Vec<u8> {
    let mut c = Canvas::new();
    match icon {
        Icon::Region => {
            // Dashed selection corners + central crosshair
            c.poly(&[(4.0, 8.5), (4.0, 4.0), (8.5, 4.0)], false);
            c.poly(&[(15.5, 4.0), (20.0, 4.0), (20.0, 8.5)], false);
            c.poly(&[(20.0, 15.5), (20.0, 20.0), (15.5, 20.0)], false);
            c.poly(&[(8.5, 20.0), (4.0, 20.0), (4.0, 15.5)], false);
            c.line(12.0, 8.5, 12.0, 15.5);
            c.line(8.5, 12.0, 15.5, 12.0);
        }
        Icon::FullScreen => {
            c.round_rect(3.0, 4.5, 18.0, 13.0, 2.0);
            c.line(9.0, 20.0, 15.0, 20.0);
            c.line(12.0, 17.5, 12.0, 20.0);
            c.poly(&[(6.5, 10.0), (6.5, 7.5), (9.0, 7.5)], false);
            c.poly(&[(15.0, 14.5), (17.5, 14.5), (17.5, 12.0)], false);
        }
        Icon::AllScreens => {
            c.round_rect(2.5, 5.5, 11.5, 9.5, 1.5);
            c.round_rect(10.0, 9.0, 11.5, 9.5, 1.5);
        }
        Icon::Scrolling => {
            c.round_rect(5.5, 3.5, 13.0, 17.0, 2.5);
            c.line(12.0, 7.5, 12.0, 16.5);
            c.poly(&[(9.5, 10.0), (12.0, 7.5), (14.5, 10.0)], false);
            c.poly(&[(9.5, 14.0), (12.0, 16.5), (14.5, 14.0)], false);
        }
        Icon::Panoramic => {
            c.round_rect(2.5, 6.0, 19.0, 12.0, 2.0);
            c.line(5.5, 12.0, 18.5, 12.0);
            c.poly(&[(8.0, 9.5), (5.5, 12.0), (8.0, 14.5)], false);
            c.poly(&[(16.0, 9.5), (18.5, 12.0), (16.0, 14.5)], false);
        }
        Icon::Video => {
            c.round_rect(2.5, 6.0, 13.5, 12.0, 2.2);
            c.poly(&[(16.0, 9.5), (21.5, 6.8), (21.5, 17.2), (16.0, 14.5)], true);
        }
        Icon::Gif => {
            c.round_rect(2.5, 4.5, 19.0, 15.0, 2.2);
            c.line(7.0, 4.5, 7.0, 19.5);
            c.line(17.0, 4.5, 17.0, 19.5);
            c.line(2.5, 9.5, 7.0, 9.5);
            c.line(2.5, 14.5, 7.0, 14.5);
            c.line(17.0, 9.5, 21.5, 9.5);
            c.line(17.0, 14.5, 21.5, 14.5);
            c.fill_poly(&[(10.3, 9.2), (14.8, 12.0), (10.3, 14.8)]);
        }
        Icon::Steps => {
            c.circle(6.5, 6.5, 2.5);
            c.line(11.5, 6.5, 20.0, 6.5);
            c.circle(6.5, 12.5, 2.5);
            c.line(11.5, 12.5, 20.0, 12.5);
            c.circle(6.5, 18.5, 2.5);
            c.line(11.5, 18.5, 20.0, 18.5);
        }
        Icon::Options => {
            c.line(4.0, 7.0, 20.0, 7.0);
            c.line(4.0, 12.0, 20.0, 12.0);
            c.line(4.0, 17.0, 20.0, 17.0);
            c.fill_circle(9.0, 7.0, 2.2);
            c.fill_circle(15.5, 12.0, 2.2);
            c.fill_circle(9.5, 17.0, 2.2);
        }
        Icon::Copy => {
            c.round_rect(8.5, 8.5, 11.5, 12.0, 2.0);
            c.poly(&[(5.5, 15.5), (4.0, 15.5), (4.0, 4.0), (15.5, 4.0), (15.5, 5.5)], false);
        }
        Icon::Save => {
            c.poly(&[(4.5, 4.0), (16.5, 4.0), (20.0, 7.5), (20.0, 19.5), (4.5, 19.5)], true);
            c.round_rect(7.5, 13.0, 9.0, 6.5, 0.8);
            c.round_rect(8.0, 4.0, 6.5, 4.5, 0.5);
        }
        Icon::Pin => {
            c.line(12.0, 15.5, 12.0, 21.0);
            c.line(7.0, 15.5, 17.0, 15.5);
            c.poly(&[(8.5, 4.5), (15.5, 4.5), (14.5, 10.5), (16.8, 15.5), (7.2, 15.5), (9.5, 10.5)], true);
        }
        Icon::ReadText => {
            c.poly(&[(4.0, 7.5), (4.0, 4.0), (7.5, 4.0)], false);
            c.poly(&[(16.5, 4.0), (20.0, 4.0), (20.0, 7.5)], false);
            c.poly(&[(20.0, 16.5), (20.0, 20.0), (16.5, 20.0)], false);
            c.poly(&[(7.5, 20.0), (4.0, 20.0), (4.0, 16.5)], false);
            c.line(7.5, 9.5, 16.5, 9.5);
            c.line(7.5, 12.5, 14.5, 12.5);
            c.line(7.5, 15.5, 12.5, 15.5);
        }
        Icon::AutoRedact => {
            c.poly(&[(12.0, 3.0), (19.0, 5.8), (19.0, 11.5), (12.0, 20.5), (5.0, 11.5), (5.0, 5.8)], true);
            c.round_rect(9.0, 10.5, 6.0, 4.5, 1.0);
            c.poly(&[(10.2, 10.5), (10.2, 8.8), (12.0, 7.8), (13.8, 8.8), (13.8, 10.5)], false);
        }
        Icon::Library => {
            c.round_rect(3.5, 4.5, 7.0, 6.5, 1.2);
            c.round_rect(13.5, 4.5, 7.0, 6.5, 1.2);
            c.round_rect(3.5, 13.5, 7.0, 6.5, 1.2);
            c.round_rect(13.5, 13.5, 7.0, 6.5, 1.2);
        }
        Icon::Preferences => {
            c.circle(12.0, 12.0, 3.0);
            for i in 0..8 {
                let a = (i as f32) * std::f32::consts::FRAC_PI_4;
                c.line(12.0 + a.cos() * 6.2, 12.0 + a.sin() * 6.2, 12.0 + a.cos() * 8.6, 12.0 + a.sin() * 8.6);
            }
            c.circle(12.0, 12.0, 6.2);
        }
        Icon::Undo => {
            c.poly(&[(8.5, 5.5), (4.5, 9.5), (8.5, 13.5)], false);
            let mut pb = PathBuilder::new();
            pb.move_to(5.0, 9.5);
            pb.line_to(14.5, 9.5);
            pb.quad_to(19.5, 9.5, 19.5, 14.0);
            pb.quad_to(19.5, 18.5, 14.5, 18.5);
            pb.line_to(10.0, 18.5);
            if let Some(p) = pb.finish() {
                c.stroke_path(&p);
            }
        }
        Icon::Redo => {
            c.poly(&[(15.5, 5.5), (19.5, 9.5), (15.5, 13.5)], false);
            let mut pb = PathBuilder::new();
            pb.move_to(19.0, 9.5);
            pb.line_to(9.5, 9.5);
            pb.quad_to(4.5, 9.5, 4.5, 14.0);
            pb.quad_to(4.5, 18.5, 9.5, 18.5);
            pb.line_to(14.0, 18.5);
            if let Some(p) = pb.finish() {
                c.stroke_path(&p);
            }
        }
        Icon::Styles => {
            c.circle(12.0, 12.0, 8.5);
            c.fill_circle(8.8, 9.5, 1.3);
            c.fill_circle(13.2, 8.2, 1.3);
            c.fill_circle(16.0, 11.8, 1.3);
            c.fill_circle(10.0, 14.8, 1.8);
        }
        Icon::ImageOps => {
            c.round_rect(3.5, 4.5, 17.0, 15.0, 2.2);
            c.fill_circle(8.5, 9.5, 1.6);
            c.poly(&[(4.0, 17.0), (9.5, 12.0), (13.0, 15.0), (16.0, 12.2), (20.0, 16.5)], false);
        }
        Icon::UpdateArrow => {
            c.line(12.0, 19.0, 12.0, 5.0);
            c.poly(&[(6.5, 10.5), (12.0, 5.0), (17.5, 10.5)], false);
        }
        Icon::Select => {
            c.poly(&[(5.5, 3.8), (5.5, 18.0), (9.8, 14.2), (13.0, 20.2), (15.2, 19.0), (12.0, 13.2), (17.2, 13.2)], true);
        }
        Icon::Eraser => {
            c.poly(&[(14.2, 4.0), (20.0, 9.8), (11.2, 18.6), (5.2, 18.6), (3.8, 17.2), (3.8, 14.4)], true);
            c.line(8.2, 10.0, 14.0, 15.8);
            c.line(11.0, 18.6, 20.0, 18.6);
        }
        Icon::Arrow => {
            c.line(5.0, 19.0, 19.0, 5.0);
            c.poly(&[(11.0, 5.0), (19.0, 5.0), (19.0, 13.0)], false);
        }
        Icon::Line => {
            c.line(5.0, 19.0, 19.0, 5.0);
            c.fill_circle(5.0, 19.0, 1.8);
            c.fill_circle(19.0, 5.0, 1.8);
        }
        Icon::Rect => {
            c.round_rect(4.0, 5.5, 16.0, 13.0, 2.0);
        }
        Icon::Ellipse => {
            if let Some(r) = SkiaRect::from_xywh(3.5, 6.0, 17.0, 12.0)
                && let Some(p) = PathBuilder::from_oval(r)
            {
                c.stroke_path(&p);
            }
        }
        Icon::Pen => {
            c.poly(&[(16.2, 3.8), (20.2, 7.8), (8.2, 19.8), (4.0, 20.2), (4.4, 16.0)], true);
            c.line(13.8, 6.2, 17.8, 10.2);
        }
        Icon::Highlighter => {
            c.poly(&[(9.0, 15.0), (17.0, 7.0), (20.0, 10.0), (12.0, 18.0)], true);
            c.poly(&[(9.0, 15.0), (5.0, 17.5), (5.0, 20.0), (9.0, 20.0), (12.0, 18.0)], false);
            c.line(4.0, 20.0, 16.0, 20.0);
        }
        Icon::Text => {
            c.line(5.0, 5.0, 19.0, 5.0);
            c.line(12.0, 5.0, 12.0, 19.0);
            c.line(8.5, 19.0, 15.5, 19.0);
            c.line(5.0, 5.0, 5.0, 7.5);
            c.line(19.0, 5.0, 19.0, 7.5);
        }
        Icon::Callout => {
            c.poly(&[(3.5, 4.5), (20.5, 4.5), (20.5, 16.0), (10.5, 16.0), (6.0, 20.0), (6.0, 16.0), (3.5, 16.0)], true);
            c.line(7.5, 9.0, 16.5, 9.0);
            c.line(7.5, 12.2, 13.5, 12.2);
        }
        Icon::StepTool => {
            c.circle(12.0, 12.0, 8.5);
            c.poly(&[(10.4, 9.4), (12.4, 7.8), (12.4, 16.2)], false);
            c.line(10.0, 16.2, 14.8, 16.2);
        }
        Icon::Stamp => {
            c.circle(12.0, 8.0, 3.2);
            c.line(12.0, 11.2, 12.0, 14.2);
            c.round_rect(5.0, 14.2, 14.0, 3.8, 1.0);
            c.line(4.0, 20.0, 20.0, 20.0);
        }
        Icon::Spotlight => {
            c.circle(12.0, 12.0, 4.5);
            c.line(12.0, 3.2, 12.0, 5.2);
            c.line(12.0, 18.8, 12.0, 20.8);
            c.line(3.2, 12.0, 5.2, 12.0);
            c.line(18.8, 12.0, 20.8, 12.0);
            c.line(5.8, 5.8, 7.2, 7.2);
            c.line(16.8, 16.8, 18.2, 18.2);
            c.line(18.2, 5.8, 16.8, 7.2);
            c.line(7.2, 16.8, 5.8, 18.2);
        }
        Icon::Magnify | Icon::Search => {
            c.circle(10.5, 10.5, 6.2);
            c.line(15.2, 15.2, 20.0, 20.0);
            if icon == Icon::Magnify {
                c.line(8.2, 10.5, 12.8, 10.5);
                c.line(10.5, 8.2, 10.5, 12.8);
            }
        }
        Icon::Pixelate => {
            for r in 0..3 {
                for col in 0..3 {
                    if (r + col) % 2 == 0 {
                        c.fill_round_rect(4.2 + col as f32 * 5.6, 4.2 + r as f32 * 5.6, 4.4, 4.4, 0.8);
                    } else {
                        c.round_rect(4.5 + col as f32 * 5.6, 4.5 + r as f32 * 5.6, 3.8, 3.8, 0.8);
                    }
                }
            }
        }
        Icon::Blur => {
            c.circle(12.0, 12.0, 8.2);
            if let Some(p) = PathBuilder::from_circle(12.0, 12.0, 4.8) {
                c.stroke_dashed(&p, 1.8, 2.0);
            }
            c.fill_circle(12.0, 12.0, 1.8);
        }
        Icon::Crop => {
            c.poly(&[(7.0, 3.5), (7.0, 17.0), (20.5, 17.0)], false);
            c.poly(&[(3.5, 7.0), (17.0, 7.0), (17.0, 20.5)], false);
        }
        Icon::CutOut => {
            c.round_rect(4.0, 4.0, 16.0, 5.0, 1.0);
            c.round_rect(4.0, 15.0, 16.0, 5.0, 1.0);
            let mut pb = PathBuilder::new();
            pb.move_to(3.0, 12.0);
            pb.line_to(21.0, 12.0);
            if let Some(p) = pb.finish() {
                c.stroke_dashed(&p, 2.2, 2.2);
            }
        }
        Icon::GrabText => {
            c.round_rect(4.0, 4.0, 16.0, 16.0, 2.0);
            c.line(7.5, 8.5, 16.5, 8.5);
            c.line(7.5, 12.0, 16.5, 12.0);
            c.line(7.5, 15.5, 13.0, 15.5);
        }
        Icon::Eyedropper => {
            c.line(14.5, 4.5, 19.5, 9.5);
            c.poly(&[(15.5, 5.5), (18.5, 8.5), (9.5, 17.5), (5.0, 19.0), (6.5, 14.5)], true);
            c.line(12.0, 7.0, 17.0, 12.0);
        }
        Icon::Close => {
            c.line(6.0, 6.0, 18.0, 18.0);
            c.line(18.0, 6.0, 6.0, 18.0);
        }
        Icon::Star | Icon::StarFilled => {
            let mut pts = Vec::with_capacity(10);
            for i in 0..10 {
                let r = if i % 2 == 0 { 8.6 } else { 3.9 };
                let a = -std::f32::consts::FRAC_PI_2 + (i as f32) * std::f32::consts::PI / 5.0;
                pts.push((12.0 + r * a.cos(), 12.3 + r * a.sin()));
            }
            if icon == Icon::StarFilled {
                c.fill_poly(&pts);
            } else {
                c.poly(&pts, true);
            }
        }
        Icon::Trash => {
            c.line(4.5, 6.5, 19.5, 6.5);
            c.poly(&[(9.5, 6.5), (9.5, 4.0), (14.5, 4.0), (14.5, 6.5)], false);
            c.poly(&[(6.5, 6.5), (7.5, 19.5), (16.5, 19.5), (17.5, 6.5)], false);
            c.line(10.0, 10.0, 10.0, 16.0);
            c.line(14.0, 10.0, 14.0, 16.0);
        }
        Icon::Folder => {
            c.poly(&[(3.5, 6.0), (9.5, 6.0), (11.5, 8.5), (20.5, 8.5), (20.5, 18.5), (3.5, 18.5)], true);
        }
        Icon::FilePdf => {
            c.poly(&[(5.5, 3.5), (14.5, 3.5), (18.5, 7.5), (18.5, 20.5), (5.5, 20.5)], true);
            c.poly(&[(14.0, 3.5), (14.0, 8.0), (18.5, 8.0)], false);
            c.line(8.5, 12.5, 15.5, 12.5);
            c.line(8.5, 16.0, 13.5, 16.0);
        }
        Icon::Duplicate => {
            c.round_rect(8.5, 8.5, 11.5, 11.5, 1.8);
            c.poly(&[(6.0, 15.0), (4.0, 15.0), (4.0, 4.0), (15.0, 4.0), (15.0, 6.0)], false);
            c.line(14.2, 11.5, 14.2, 17.0);
            c.line(11.5, 14.2, 17.0, 14.2);
        }
        Icon::Play => {
            c.fill_poly(&[(7.0, 5.0), (19.0, 12.0), (7.0, 19.0)]);
        }
        Icon::Pause => {
            c.fill_round_rect(6.5, 5.5, 3.8, 13.0, 1.0);
            c.fill_round_rect(13.7, 5.5, 3.8, 13.0, 1.0);
        }
        Icon::Stop => {
            c.fill_round_rect(6.0, 6.0, 12.0, 12.0, 2.0);
        }
        Icon::RecordDot => {
            c.circle(12.0, 12.0, 7.5);
            c.fill_circle(12.0, 12.0, 3.8);
        }
        Icon::VolumeOn | Icon::VolumeOff => {
            c.poly(&[(4.0, 9.5), (8.0, 9.5), (12.5, 5.5), (12.5, 18.5), (8.0, 14.5), (4.0, 14.5)], true);
            if icon == Icon::VolumeOn {
                let mut pb = PathBuilder::new();
                pb.move_to(16.0, 9.2);
                pb.quad_to(18.2, 12.0, 16.0, 14.8);
                if let Some(p) = pb.finish() {
                    c.stroke_path(&p);
                }
                let mut pb2 = PathBuilder::new();
                pb2.move_to(18.5, 7.0);
                pb2.quad_to(21.5, 12.0, 18.5, 17.0);
                if let Some(p) = pb2.finish() {
                    c.stroke_path(&p);
                }
            } else {
                c.line(16.0, 9.5, 20.5, 14.5);
                c.line(20.5, 9.5, 16.0, 14.5);
            }
        }
        Icon::CaptureFrame => {
            c.round_rect(3.5, 6.0, 17.0, 13.0, 2.2);
            c.circle(12.0, 12.5, 3.5);
            c.poly(&[(8.5, 6.0), (9.8, 4.0), (14.2, 4.0), (15.5, 6.0)], false);
        }
        Icon::ChevronDown => {
            c.poly(&[(6.5, 9.5), (12.0, 15.0), (17.5, 9.5)], false);
        }
        Icon::PanelLeft => {
            c.round_rect(3.5, 4.5, 17.0, 15.0, 2.2);
            c.line(9.5, 4.5, 9.5, 19.5);
        }
        Icon::Check => {
            c.poly(&[(5.0, 12.5), (9.8, 17.2), (19.0, 7.2)], false);
        }
        Icon::Refresh => {
            let mut pb = PathBuilder::new();
            pb.move_to(19.0, 10.5);
            pb.quad_to(17.5, 4.8, 11.5, 5.0);
            pb.quad_to(5.5, 5.2, 5.0, 11.5);
            pb.quad_to(5.2, 18.0, 11.8, 18.8);
            pb.quad_to(16.5, 19.2, 18.5, 15.5);
            if let Some(p) = pb.finish() {
                c.stroke_path(&p);
            }
            c.poly(&[(15.5, 10.5), (19.5, 10.8), (19.2, 6.8)], false);
        }
        Icon::Edit => {
            c.round_rect(4.0, 5.5, 14.5, 14.5, 2.0);
            c.line(12.0, 12.0, 20.0, 4.0);
            c.poly(&[(16.5, 4.0), (20.0, 4.0), (20.0, 7.5)], false);
        }
        Icon::Clipboard => {
            c.round_rect(5.5, 5.5, 13.0, 15.0, 2.0);
            c.round_rect(8.5, 3.5, 7.0, 3.5, 1.0);
            c.line(8.5, 11.5, 15.5, 11.5);
            c.line(8.5, 15.5, 13.5, 15.5);
        }
        Icon::ExternalOpen => {
            c.poly(&[(14.0, 4.5), (19.5, 4.5), (19.5, 10.0)], false);
            c.line(11.0, 13.0, 19.5, 4.5);
            c.poly(&[(11.0, 6.0), (5.0, 6.0), (5.0, 19.0), (18.0, 19.0), (18.0, 13.0)], false);
        }
        Icon::CombineV => {
            c.round_rect(5.5, 4.0, 13.0, 6.0, 1.2);
            c.round_rect(5.5, 14.0, 13.0, 6.0, 1.2);
            c.line(12.0, 10.0, 12.0, 14.0);
        }
        Icon::CombineH => {
            c.round_rect(4.0, 5.5, 6.0, 13.0, 1.2);
            c.round_rect(14.0, 5.5, 6.0, 13.0, 1.2);
            c.line(10.0, 12.0, 14.0, 12.0);
        }
        Icon::StepBack => {
            c.line(7.0, 6.0, 7.0, 18.0);
            c.fill_poly(&[(17.5, 6.0), (9.5, 12.0), (17.5, 18.0)]);
        }
        Icon::StepForward => {
            c.fill_poly(&[(6.5, 6.0), (14.5, 12.0), (6.5, 18.0)]);
            c.line(17.0, 6.0, 17.0, 18.0);
        }
        Icon::SkipStart => {
            c.line(6.0, 6.0, 6.0, 18.0);
            c.fill_poly(&[(13.5, 6.5), (7.5, 12.0), (13.5, 17.5)]);
            c.fill_poly(&[(19.5, 6.5), (13.5, 12.0), (19.5, 17.5)]);
        }
        Icon::SkipEnd => {
            c.fill_poly(&[(4.5, 6.5), (10.5, 12.0), (4.5, 17.5)]);
            c.fill_poly(&[(10.5, 6.5), (16.5, 12.0), (10.5, 17.5)]);
            c.line(18.0, 6.0, 18.0, 18.0);
        }
        Icon::MarkIn => {
            c.poly(&[(12.5, 5.0), (7.5, 5.0), (7.5, 19.0), (12.5, 19.0)], false);
            c.line(17.5, 12.0, 11.5, 12.0);
            c.poly(&[(14.0, 9.2), (11.2, 12.0), (14.0, 14.8)], false);
        }
        Icon::MarkOut => {
            c.poly(&[(11.5, 5.0), (16.5, 5.0), (16.5, 19.0), (11.5, 19.0)], false);
            c.line(6.5, 12.0, 12.5, 12.0);
            c.poly(&[(10.0, 9.2), (12.8, 12.0), (10.0, 14.8)], false);
        }
        Icon::BringForward => {
            c.line(12.0, 19.0, 12.0, 5.5);
            c.poly(&[(7.0, 10.5), (12.0, 5.5), (17.0, 10.5)], false);
        }
        Icon::SendBackward => {
            c.line(12.0, 5.0, 12.0, 18.5);
            c.poly(&[(7.0, 13.5), (12.0, 18.5), (17.0, 13.5)], false);
        }
        Icon::BringToFront => {
            c.line(5.5, 5.0, 18.5, 5.0);
            c.line(12.0, 19.5, 12.0, 8.5);
            c.poly(&[(7.5, 13.0), (12.0, 8.5), (16.5, 13.0)], false);
        }
        Icon::SendToBack => {
            c.line(5.5, 19.0, 18.5, 19.0);
            c.line(12.0, 4.5, 12.0, 15.5);
            c.poly(&[(7.5, 11.0), (12.0, 15.5), (16.5, 11.0)], false);
        }
        Icon::AlignLeft => {
            c.line(5.0, 4.5, 5.0, 19.5);
            c.fill_round_rect(8.0, 6.5, 11.5, 4.2, 1.0);
            c.fill_round_rect(8.0, 13.3, 7.5, 4.2, 1.0);
        }
        Icon::AlignCenterH => {
            c.line(12.0, 4.5, 12.0, 19.5);
            c.fill_round_rect(5.5, 6.5, 13.0, 4.2, 1.0);
            c.fill_round_rect(7.5, 13.3, 9.0, 4.2, 1.0);
        }
        Icon::AlignRight => {
            c.line(19.0, 4.5, 19.0, 19.5);
            c.fill_round_rect(4.5, 6.5, 11.5, 4.2, 1.0);
            c.fill_round_rect(8.5, 13.3, 7.5, 4.2, 1.0);
        }
        Icon::AlignTop => {
            c.line(4.5, 5.0, 19.5, 5.0);
            c.fill_round_rect(6.5, 8.0, 4.2, 11.5, 1.0);
            c.fill_round_rect(13.3, 8.0, 4.2, 7.5, 1.0);
        }
        Icon::AlignCenterV => {
            c.line(4.5, 12.0, 19.5, 12.0);
            c.fill_round_rect(6.5, 5.5, 4.2, 13.0, 1.0);
            c.fill_round_rect(13.3, 7.5, 4.2, 9.0, 1.0);
        }
        Icon::AlignBottom => {
            c.line(4.5, 19.0, 19.5, 19.0);
            c.fill_round_rect(6.5, 4.5, 4.2, 11.5, 1.0);
            c.fill_round_rect(13.3, 8.5, 4.2, 7.5, 1.0);
        }
        Icon::DistributeH => {
            c.line(4.5, 5.0, 4.5, 19.0);
            c.line(19.5, 5.0, 19.5, 19.0);
            c.fill_round_rect(9.8, 7.0, 4.4, 10.0, 1.0);
        }
        Icon::DistributeV => {
            c.line(5.0, 4.5, 19.0, 4.5);
            c.line(5.0, 19.5, 19.0, 19.5);
            c.fill_round_rect(7.0, 9.8, 10.0, 4.4, 1.0);
        }
    }
    let _ = Canvas::stroke_thin; // keep helper available if needed
    c.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_icons_rasterize_cleanly() {
        for &icon in Icon::ALL {
            let rgba = rasterize(icon);
            assert_eq!(rgba.len(), 48 * 48 * 4, "{icon:?}");
            let lit = rgba.chunks_exact(4).filter(|p| p[3] > 128).count();
            assert!(lit >= 20, "{icon:?} has too few opaque pixels ({lit})");
        }
    }
}
