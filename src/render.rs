//! Editing operations and how they're rasterized onto the image.
//!
//! Every coordinate here is in image pixels. An `Op` is applied directly to an `RgbaImage`; undoing
//! means replaying the list from the original capture.

use ab_glyph::{Font, FontArc, ScaleFont};
use image::RgbaImage;
use tiny_skia::{
    BlendMode, FillRule, LineCap, LineJoin, Paint, PathBuilder, PixmapMut, Rect, Stroke, StrokeDash, Transform,
};

pub type Point = (f32, f32);
pub type Rgba = [u8; 4];

/// Shape of the step marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StepShape {
    Circle,
    Square,
    Diamond,
    Hexagon,
    /// A drop pointing down, like a map pin.
    Pin,
}

impl StepShape {
    /// Value, Portuguese name and English name.
    pub const ALL: [(StepShape, &'static str, &'static str); 5] = [
        (StepShape::Circle, "Círculo", "Circle"),
        (StepShape::Square, "Quadrado", "Square"),
        (StepShape::Diamond, "Losango", "Diamond"),
        (StepShape::Hexagon, "Hexágono", "Hexagon"),
        (StepShape::Pin, "Gota", "Pin"),
    ];
}

/// What the step marker shows: the number or the matching letter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StepGlyph {
    Number,
    Upper,
    Lower,
}

impl StepGlyph {
    /// Value, Portuguese name and English name.
    pub const ALL: [(StepGlyph, &'static str, &'static str); 3] =
        [(StepGlyph::Number, "1 2 3", "1 2 3"), (StepGlyph::Upper, "A B C", "A B C"), (StepGlyph::Lower, "a b c", "a b c")];
}

/// Look of the speech callout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CalloutStyle {
    /// Filled box with rounded corners.
    Rounded,
    /// Filled box with square corners.
    Square,
    /// White box with an outline and text in the chosen color.
    Outline,
}

impl CalloutStyle {
    /// Value, Portuguese name and English name.
    pub const ALL: [(CalloutStyle, &'static str, &'static str); 3] = [
        (CalloutStyle::Rounded, "Arredondado", "Rounded"),
        (CalloutStyle::Square, "Reto", "Square"),
        (CalloutStyle::Outline, "Contorno", "Outline"),
    ];
}

/// Symbols available in the stamp tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Stamp {
    Check,
    Cross,
    Warning,
    Info,
    Question,
    Star,
    Plus,
    Minus,
    ArrowRight,
    ArrowLeft,
    ArrowUp,
    ArrowDown,
    Heart,
    Flag,
    Pin,
    Cursor,
}

impl Stamp {
    /// Value, Portuguese name and English name.
    pub const ALL: [(Stamp, &'static str, &'static str); 16] = [
        (Stamp::Check, "Certo", "Check"),
        (Stamp::Cross, "Errado", "Wrong"),
        (Stamp::Warning, "Atenção", "Warning"),
        (Stamp::Info, "Informação", "Info"),
        (Stamp::Question, "Dúvida", "Question"),
        (Stamp::Star, "Estrela", "Star"),
        (Stamp::Plus, "Mais", "Plus"),
        (Stamp::Minus, "Menos", "Minus"),
        (Stamp::ArrowRight, "Seta para a direita", "Right arrow"),
        (Stamp::ArrowLeft, "Seta para a esquerda", "Left arrow"),
        (Stamp::ArrowUp, "Seta para cima", "Up arrow"),
        (Stamp::ArrowDown, "Seta para baixo", "Down arrow"),
        (Stamp::Heart, "Coração", "Heart"),
        (Stamp::Flag, "Bandeira", "Flag"),
        (Stamp::Pin, "Alfinete", "Pushpin"),
        (Stamp::Cursor, "Cursor", "Cursor"),
    ];
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// `double`: a head on both ends. `dashed` (here and in the shapes below): dashed stroke.
    Arrow { from: Point, to: Point, color: Rgba, width: f32, dashed: bool, double: bool },
    Line { from: Point, to: Point, color: Rgba, width: f32, dashed: bool },
    /// `filled`: paints the whole shape instead of just the outline.
    Rect { a: Point, b: Point, color: Rgba, width: f32, filled: bool, dashed: bool },
    Ellipse { a: Point, b: Point, color: Rgba, width: f32, filled: bool, dashed: bool },
    Pen { points: Vec<Point>, color: Rgba, width: f32 },
    /// Eraser: wherever the stroke passes, the image goes back to the original capture, erasing
    /// what was drawn there before.
    Erase { points: Vec<Point>, width: f32 },
    /// Highlighter: a translucent rectangle that keeps the text underneath readable.
    Highlight { a: Point, b: Point, color: Rgba },
    /// `boxed`: draws a contrasting background box behind the text.
    Text { pos: Point, text: String, color: Rgba, size: f32, boxed: bool },
    /// Speech callout: a colored box with text and a tail that points to `tip`. `pos` is the
    /// top-left corner of the box, which grows with the text.
    Callout { tip: Point, pos: Point, text: String, color: Rgba, size: f32, style: CalloutStyle },
    /// Step marker: a filled circle with a number inside, for step-by-step tutorials.
    Step { center: Point, number: u32, color: Rgba, radius: f32, shape: StepShape, glyph: StepGlyph },
    /// Stamp: a ready-made symbol (check, cross, warning…) of size `radius`.
    Stamp { kind: Stamp, center: Point, radius: f32, color: Rgba },
    /// Spotlight: darkens the whole image except the area between `a` and `b`.
    Spotlight { a: Point, b: Point },
    /// Pixelates the area to hide information.
    Pixelate { a: Point, b: Point },
    /// Blurs the area: hides details without the blocks of pixelation.
    Blur { a: Point, b: Point },
    /// Magnifier: copies the region `src` of the image and shows the copy enlarged in the box
    /// `a`–`b`, somewhere else. The original stays visible, marked with an outline.
    Magnify { src: (Point, Point), a: Point, b: Point },
    /// Replaces the image with the crop; the following operations use the new coordinate system.
    Crop { a: Point, b: Point },
    /// Rotates the whole image a quarter turn.
    Rotate { clockwise: bool },
    /// Mirrors the whole image.
    Flip { horizontal: bool },
    /// Changes the size of the whole image, in pixels.
    Resize { width: u32, height: u32 },
    /// Plain-color border around the image.
    Border { width: u32, color: Rgba },
    /// White margin with a drop shadow, so the capture stands out in a document.
    Shadow { size: u32 },
    /// Removes a strip from the image and joins the two parts. `rows`: the strip is horizontal (the
    /// rows between `from` and `to` go away); otherwise it's vertical (the columns go away).
    CutOut { from: Point, to: Point, rows: bool },
}

/// Control point of a selected object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Handle {
    Start,
    End,
    /// Box corner: 0 = top-left, 1 = top-right, 2 = bottom-right, 3 = bottom-left.
    Corner(usize),
}

/// Text size for a value of the Width control.
pub fn text_size_for(width: f32) -> f32 {
    14.0 + width * 4.0
}

/// Step marker radius for a value of the Width control.
pub fn step_radius_for(width: f32) -> f32 {
    12.0 + width * 2.0
}

fn ordered(a: Point, b: Point) -> (Point, Point) {
    ((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1)))
}

fn corner((min, max): (Point, Point), i: usize) -> Point {
    match i % 4 {
        0 => min,
        1 => (max.0, min.1),
        2 => max,
        _ => (min.0, max.1),
    }
}

pub fn distance(a: Point, b: Point) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn segment_distance(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-6 {
        return distance(p, a);
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    distance(p, (a.0 + dx * t, a.1 + dy * t))
}

fn in_box(p: Point, (min, max): (Point, Point), margin: f32) -> bool {
    p.0 >= min.0 - margin && p.0 <= max.0 + margin && p.1 >= min.1 - margin && p.1 <= max.1 + margin
}

/// Width and height taken up by a text, with the same measurements as [`draw_text`].
pub fn text_extent(text: &str, size: f32, font: &FontArc) -> (f32, f32) {
    let scaled = font.as_scaled(size);
    let line_height = scaled.height() + scaled.line_gap();
    let width = text.lines().map(|l| text_width(l, size, font)).fold(0.0, f32::max);
    (width, line_height * text.lines().count().max(1) as f32)
}

/// Object geometry: this is what allows selecting, moving and resizing an annotation after it's
/// done.
impl Op {
    /// Crop, rotate, resize and the like change the whole image; they aren't selectable objects.
    pub fn is_object(&self) -> bool {
        !matches!(self, Op::Crop { .. } | Op::Rotate { .. } | Op::Flip { .. } | Op::Resize { .. } | Op::Border { .. } | Op::Shadow { .. } | Op::CutOut { .. })
    }

    /// Object box, not counting the stroke width.
    fn frame(&self, font: &FontArc) -> Option<(Point, Point)> {
        match self {
            Op::Arrow { from, to, .. } | Op::Line { from, to, .. } => Some(ordered(*from, *to)),
            Op::Rect { a, b, .. } | Op::Ellipse { a, b, .. } | Op::Highlight { a, b, .. } | Op::Pixelate { a, b }
            | Op::Blur { a, b }
            | Op::Spotlight { a, b } => {
                Some(ordered(*a, *b))
            }
            Op::Pen { points, .. } | Op::Erase { points, .. } => {
                let first = *points.first()?;
                Some(points.iter().fold((first, first), |(min, max), p| {
                    ((min.0.min(p.0), min.1.min(p.1)), (max.0.max(p.0), max.1.max(p.1)))
                }))
            }
            Op::Text { pos, text, size, .. } => {
                let (w, h) = text_extent(text, *size, font);
                Some((*pos, (pos.0 + w, pos.1 + h)))
            }
            Op::Step { center, radius, .. } | Op::Stamp { center, radius, .. } => {
                Some(((center.0 - radius, center.1 - radius), (center.0 + radius, center.1 + radius)))
            }
            Op::Callout { pos, text, size, .. } => Some(callout_box(*pos, text, *size, font)),
            Op::Magnify { a, b, .. } => Some(ordered(*a, *b)),
            Op::Crop { .. } | Op::Rotate { .. } | Op::Flip { .. } | Op::Resize { .. } | Op::Border { .. } | Op::Shadow { .. } | Op::CutOut { .. } => None,
        }
    }

    /// Box that encloses everything the object paints.
    pub fn bounds(&self, font: &FontArc) -> Option<(Point, Point)> {
        let pad = match self {
            Op::Arrow { width, .. } => (width / 2.0).max(arrow_head_len(*width) * 0.45),
            Op::Line { width, .. } | Op::Rect { width, .. } | Op::Ellipse { width, .. } | Op::Pen { width, .. } | Op::Erase { width, .. } => {
                width / 2.0
            }
            Op::Text { size, boxed: true, .. } => text_box_pad(*size),
            Op::Magnify { .. } => 2.5,
            _ => 0.0,
        };
        let (mut min, mut max) = self.frame(font)?;
        if let Op::Callout { tip, .. } = self {
            min = (min.0.min(tip.0), min.1.min(tip.1));
            max = (max.0.max(tip.0), max.1.max(tip.1));
        }
        Some(((min.0 - pad, min.1 - pad), (max.0 + pad, max.1 + pad)))
    }

    /// Does the point `p` touch the object? `tol` is the slack in image pixels. Rectangles and
    /// ellipses only count on the outline, so what's inside them can still be picked.
    pub fn hit(&self, p: Point, tol: f32, font: &FontArc) -> bool {
        match self {
            Op::Arrow { from, to, width, .. } | Op::Line { from, to, width, .. } => {
                segment_distance(p, *from, *to) <= width / 2.0 + tol
            }
            Op::Pen { points, width, .. } | Op::Erase { points, width } => {
                points.windows(2).any(|w| segment_distance(p, w[0], w[1]) <= width / 2.0 + tol)
            }
            Op::Rect { a, b, width, filled, .. } => {
                let frame = ordered(*a, *b);
                let reach = width / 2.0 + tol;
                in_box(p, frame, reach) && (*filled || !in_box(p, frame, -reach))
            }
            Op::Ellipse { a, b, width, filled, .. } => {
                let (min, max) = ordered(*a, *b);
                let (rx, ry) = ((max.0 - min.0) / 2.0, (max.1 - min.1) / 2.0);
                if rx < 1.0 || ry < 1.0 {
                    return false;
                }
                let d = (((p.0 - min.0 - rx) / rx).powi(2) + ((p.1 - min.1 - ry) / ry).powi(2)).sqrt();
                (*filled && d <= 1.0) || (d - 1.0).abs() * rx.min(ry) <= width / 2.0 + tol
            }
            _ => self.frame(font).is_some_and(|f| in_box(p, f, tol)),
        }
    }

    /// Is `p` inside a rectangle or ellipse? Used when nothing was touched on the outline.
    pub fn encloses(&self, p: Point) -> bool {
        match self {
            Op::Rect { a, b, .. } | Op::Ellipse { a, b, .. } => in_box(p, ordered(*a, *b), 0.0),
            _ => false,
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        let shift = |p: &mut Point| {
            p.0 += dx;
            p.1 += dy;
        };
        match self {
            Op::Arrow { from, to, .. } | Op::Line { from, to, .. } => {
                shift(from);
                shift(to);
            }
            Op::Rect { a, b, .. }
            | Op::Ellipse { a, b, .. }
            | Op::Highlight { a, b, .. }
            | Op::Pixelate { a, b }
            | Op::Blur { a, b }
            | Op::Spotlight { a, b }
            | Op::Magnify { a, b, .. }
            | Op::Crop { a, b } => {
                shift(a);
                shift(b);
            }
            Op::Pen { points, .. } | Op::Erase { points, .. } => points.iter_mut().for_each(shift),
            Op::Text { pos, .. } => shift(pos),
            Op::Callout { tip, pos, .. } => {
                shift(tip);
                shift(pos);
            }
            Op::Step { center, .. } | Op::Stamp { center, .. } => shift(center),
            Op::Rotate { .. }
            | Op::Flip { .. }
            | Op::Resize { .. }
            | Op::Border { .. }
            | Op::Shadow { .. }
            | Op::CutOut { .. } => {}
        }
    }

    /// The object's control points, in image pixels.
    pub fn handles(&self, font: &FontArc) -> Vec<(Handle, Point)> {
        match self {
            Op::Arrow { from, to, .. } | Op::Line { from, to, .. } => {
                vec![(Handle::Start, *from), (Handle::End, *to)]
            }
            Op::Rect { .. } | Op::Ellipse { .. } | Op::Highlight { .. } | Op::Pixelate { .. } | Op::Blur { .. } | Op::Spotlight { .. } | Op::Pen { .. } | Op::Erase { .. } => {
                match self.frame(font) {
                    Some(f) => (0..4).map(|i| (Handle::Corner(i), corner(f, i))).collect(),
                    None => Vec::new(),
                }
            }
            Op::Text { .. } => match self.frame(font) {
                Some(f) => vec![(Handle::Corner(2), f.1)],
                None => Vec::new(),
            },
            // Corners of the copy, plus two points on the source region: the middle moves, the
            // corner resizes.
            Op::Magnify { src, a, b } => {
                let (s_min, s_max) = ordered(src.0, src.1);
                let dest = ordered(*a, *b);
                let mut all: Vec<_> = (0..4).map(|i| (Handle::Corner(i), corner(dest, i))).collect();
                all.push((Handle::Start, ((s_min.0 + s_max.0) / 2.0, (s_min.1 + s_max.1) / 2.0)));
                all.push((Handle::End, s_max));
                all
            }
            Op::Callout { tip, .. } => match self.frame(font) {
                Some(f) => vec![(Handle::Start, *tip), (Handle::Corner(2), f.1)],
                None => Vec::new(),
            },
            Op::Step { center, radius, .. } | Op::Stamp { center, radius, .. } => vec![(Handle::End, (center.0 + radius, center.1))],
            Op::Crop { .. } | Op::Rotate { .. } | Op::Flip { .. } | Op::Resize { .. } | Op::Border { .. } | Op::Shadow { .. } | Op::CutOut { .. } => Vec::new(),
        }
    }

    /// The object as it looks with the control point `handle` dragged to `to`.
    pub fn with_handle(&self, handle: Handle, to: Point, font: &FontArc) -> Op {
        let mut op = self.clone();
        let frame = self.frame(font);
        match (&mut op, handle) {
            (Op::Arrow { from, .. } | Op::Line { from, .. }, Handle::Start) => *from = to,
            (Op::Arrow { to: end, .. } | Op::Line { to: end, .. }, Handle::End) => *end = to,
            (
                Op::Rect { a, b, .. } | Op::Ellipse { a, b, .. } | Op::Highlight { a, b, .. }
                | Op::Pixelate { a, b }
                | Op::Blur { a, b }
                | Op::Spotlight { a, b },
                Handle::Corner(i),
            ) => {
                if let Some(f) = frame {
                    *a = corner(f, i + 2);
                    *b = to;
                }
            }
            (Op::Pen { points, .. } | Op::Erase { points, .. }, Handle::Corner(i)) => {
                if let Some(f) = frame {
                    let (fixed, grabbed) = (corner(f, i + 2), corner(f, i));
                    // A nearly straight stroke can't be stretched in the direction in which it's
                    // thin.
                    let ratio = |to: f32, fixed: f32, grabbed: f32| {
                        if (grabbed - fixed).abs() < 1.0 { 1.0 } else { (to - fixed) / (grabbed - fixed) }
                    };
                    let (sx, sy) = (ratio(to.0, fixed.0, grabbed.0), ratio(to.1, fixed.1, grabbed.1));
                    for p in points.iter_mut() {
                        *p = (fixed.0 + (p.0 - fixed.0) * sx, fixed.1 + (p.1 - fixed.1) * sy);
                    }
                }
            }
            (Op::Magnify { src, a, b }, Handle::Corner(i)) => {
                if let Some(f) = frame {
                    // The width follows the mouse; the height follows the aspect ratio of the
                    // source region.
                    let (s_min, s_max) = ordered(src.0, src.1);
                    let aspect = (s_max.1 - s_min.1) / (s_max.0 - s_min.0).max(1.0);
                    let fixed = corner(f, i + 2);
                    let w = (to.0 - fixed.0).abs().max(8.0);
                    let (dir_x, dir_y) = (if to.0 >= fixed.0 { 1.0 } else { -1.0 }, if to.1 >= fixed.1 { 1.0 } else { -1.0 });
                    *a = fixed;
                    *b = (fixed.0 + dir_x * w, fixed.1 + dir_y * w * aspect);
                }
            }
            (Op::Magnify { src, .. }, Handle::Start) => {
                let (s_min, s_max) = ordered(src.0, src.1);
                let (dx, dy) = (to.0 - (s_min.0 + s_max.0) / 2.0, to.1 - (s_min.1 + s_max.1) / 2.0);
                *src = ((s_min.0 + dx, s_min.1 + dy), (s_max.0 + dx, s_max.1 + dy));
            }
            (Op::Magnify { src, a, b }, Handle::End) => {
                // Changing the source region keeps the copy's width and adjusts its height.
                let (s_min, _) = ordered(src.0, src.1);
                let s_max = (to.0.max(s_min.0 + 4.0), to.1.max(s_min.1 + 4.0));
                *src = (s_min, s_max);
                let (min, max) = ordered(*a, *b);
                *a = min;
                *b = (max.0, min.1 + (max.0 - min.0) * (s_max.1 - s_min.1) / (s_max.0 - s_min.0));
            }
            (Op::Callout { tip, .. }, Handle::Start) => *tip = to,
            (Op::Text { pos, size, .. } | Op::Callout { pos, size, .. }, Handle::Corner(_)) => {
                if let Some((_, max)) = frame {
                    let height = (max.1 - pos.1).max(1.0);
                    *size = (*size * (to.1 - pos.1) / height).clamp(8.0, 400.0);
                }
            }
            (Op::Step { center, radius, .. } | Op::Stamp { center, radius, .. }, _) => *radius = distance(*center, to).clamp(6.0, 300.0),
            _ => {}
        }
        op
    }

    pub fn color(&self) -> Option<Rgba> {
        match self {
            Op::Arrow { color, .. }
            | Op::Line { color, .. }
            | Op::Rect { color, .. }
            | Op::Ellipse { color, .. }
            | Op::Pen { color, .. }
            | Op::Highlight { color, .. }
            | Op::Text { color, .. }
            | Op::Callout { color, .. }
            | Op::Stamp { color, .. }
            | Op::Step { color, .. } => Some(*color),
            Op::Pixelate { .. } | Op::Blur { .. } | Op::Spotlight { .. } | Op::Magnify { .. } | Op::Erase { .. } | Op::Crop { .. } | Op::Rotate { .. } | Op::Flip { .. } | Op::Resize { .. } | Op::Border { .. } | Op::Shadow { .. } | Op::CutOut { .. } => None,
        }
    }

    pub fn set_color(&mut self, new: Rgba) {
        match self {
            Op::Arrow { color, .. }
            | Op::Line { color, .. }
            | Op::Rect { color, .. }
            | Op::Ellipse { color, .. }
            | Op::Pen { color, .. }
            | Op::Highlight { color, .. }
            | Op::Text { color, .. }
            | Op::Callout { color, .. }
            | Op::Stamp { color, .. }
            | Op::Step { color, .. } => *color = new,
            Op::Pixelate { .. } | Op::Blur { .. } | Op::Spotlight { .. } | Op::Magnify { .. } | Op::Erase { .. } | Op::Crop { .. } | Op::Rotate { .. } | Op::Flip { .. } | Op::Resize { .. } | Op::Border { .. } | Op::Shadow { .. } | Op::CutOut { .. } => {}
        }
    }

    /// Value of the Width control that matches the object (stroke, text size or marker size).
    pub fn thickness(&self) -> Option<f32> {
        match self {
            Op::Arrow { width, .. }
            | Op::Line { width, .. }
            | Op::Rect { width, .. }
            | Op::Ellipse { width, .. }
            | Op::Pen { width, .. }
            | Op::Erase { width, .. } => Some(*width),
            Op::Text { size, .. } | Op::Callout { size, .. } => Some((size - 14.0) / 4.0),
            Op::Step { radius, .. } | Op::Stamp { radius, .. } => Some((radius - 12.0) / 2.0),
            Op::Magnify { src, a, b } => {
                let zoom = (b.0 - a.0).abs() / (src.1.0 - src.0.0).abs().max(1.0);
                Some((zoom - 1.5) * 8.0)
            }
            _ => None,
        }
    }

    pub fn set_thickness(&mut self, value: f32) {
        match self {
            Op::Arrow { width, .. }
            | Op::Line { width, .. }
            | Op::Rect { width, .. }
            | Op::Ellipse { width, .. }
            | Op::Pen { width, .. }
            | Op::Erase { width, .. } => *width = value,
            Op::Text { size, .. } | Op::Callout { size, .. } => *size = text_size_for(value),
            Op::Step { radius, .. } | Op::Stamp { radius, .. } => *radius = step_radius_for(value),
            Op::Magnify { src, a, b } => {
                let zoom = magnify_zoom_for(value);
                let (cx, cy) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                let half = ((src.1.0 - src.0.0).abs() * zoom / 2.0, (src.1.1 - src.0.1).abs() * zoom / 2.0);
                *a = (cx - half.0, cy - half.1);
                *b = (cx + half.0, cy + half.1);
            }
            _ => {}
        }
    }
}

/// Gap between the text and the edge of the background box.
fn text_box_pad(size: f32) -> f32 {
    size * 0.3
}

/// Gap between the text and the edge of the callout.
pub fn callout_pad(size: f32) -> f32 {
    size * 0.5
}

/// Callout box whose top-left corner is `pos`.
pub fn callout_box(pos: Point, text: &str, size: f32, font: &FontArc) -> (Point, Point) {
    let (w, h) = text_extent(text, size, font);
    let pad = callout_pad(size);
    (pos, (pos.0 + w + pad * 2.0, pos.1 + h + pad * 2.0))
}

/// Magnifier zoom for a value of the Width control.
pub fn magnify_zoom_for(width: f32) -> f32 {
    1.5 + width / 8.0
}

/// Where to put the enlarged copy of a newly marked region: next to it, inside the image.
pub fn magnify_dest(a: Point, b: Point, zoom: f32, (img_w, img_h): (f32, f32)) -> (Point, Point) {
    const GAP: f32 = 24.0;
    let (min, max) = ordered(a, b);
    let (w, h) = ((max.0 - min.0) * zoom, (max.1 - min.1) * zoom);
    let (cx, cy) = ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0);
    // To the right; if it doesn't fit, to the left; if not, below or above.
    let (mut x, mut y) = (max.0 + GAP, cy - h / 2.0);
    if x + w > img_w {
        if min.0 - GAP - w >= 0.0 {
            x = min.0 - GAP - w;
        } else {
            x = cx - w / 2.0;
            y = if max.1 + GAP + h <= img_h { max.1 + GAP } else { min.1 - GAP - h };
        }
    }
    let x = x.clamp(0.0, (img_w - w).max(0.0));
    let y = y.clamp(0.0, (img_h - h).max(0.0));
    ((x, y), (x + w, y + h))
}

/// Light frame with a dark rule: shows on light or dark backgrounds.
fn outline(img: &mut RgbaImage, a: Point, b: Point, width: f32) {
    if let Some(r) = skia_rect(a, b) {
        let path = PathBuilder::from_rect(r);
        stroke_path(img, &path, [20, 20, 20, 255], width + 2.0);
        stroke_path(img, &path, [255, 255, 255, 255], width);
    }
}

fn magnify(img: &mut RgbaImage, src: (Point, Point), a: Point, b: Point) {
    let Some((x, y, w, h)) = pixel_rect(img, src.0, src.1) else { return };
    let (min, max) = ordered(a, b);
    let (out_w, out_h) = ((max.0 - min.0).round() as u32, (max.1 - min.1).round() as u32);
    if out_w == 0 || out_h == 0 || out_w > 16_000 || out_h > 16_000 {
        return;
    }
    // Copies the region before drawing any frame, so the frame doesn't appear in the enlargement.
    let region = image::imageops::crop_imm(img, x, y, w, h).to_image();
    let big = image::imageops::resize(&region, out_w, out_h, image::imageops::FilterType::CatmullRom);
    outline(img, src.0, src.1, 1.5);
    image::imageops::replace(img, &big, min.0.round() as i64, min.1.round() as i64);
    outline(img, min, max, 3.0);
}

/// Largest side accepted when resizing.
const MAX_SIDE: u32 = 20_000;

/// Puts the image on a white margin with a soft shadow below and to the right.
fn with_shadow(img: &RgbaImage, size: u32) -> RgbaImage {
    let offset = size / 3;
    let (w, h) = (img.width() + 2 * size, img.height() + 2 * size);
    let mut out = RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
    // The shadow is the image's silhouette, darkened, shifted and then blurred.
    for y in 0..img.height() {
        for x in 0..img.width() {
            out.put_pixel(x + size + offset, y + size + offset, image::Rgba([70, 70, 70, 255]));
        }
    }
    let mut out = image::imageops::fast_blur(&out, size as f32 / 3.0);
    image::imageops::replace(&mut out, img, size as i64, size as i64);
    out
}

/// Removes the rows (or columns) between `from` and `to` and joins what remains.
fn cut_out(img: &mut RgbaImage, from: Point, to: Point, rows: bool) {
    let (w, h) = (img.width(), img.height());
    let limit = if rows { h } else { w } as f32;
    let (a, b) = if rows { (from.1, to.1) } else { (from.0, to.0) };
    let start = a.min(b).clamp(0.0, limit).round() as u32;
    let end = a.max(b).clamp(0.0, limit).round() as u32;
    let removed = end - start;
    // Some image has to remain.
    if removed == 0 || removed >= limit as u32 {
        return;
    }
    let out = if rows {
        let mut out = RgbaImage::new(w, h - removed);
        image::imageops::replace(&mut out, &image::imageops::crop_imm(img, 0, 0, w, start).to_image(), 0, 0);
        image::imageops::replace(&mut out, &image::imageops::crop_imm(img, 0, end, w, h - end).to_image(), 0, start as i64);
        out
    } else {
        let mut out = RgbaImage::new(w - removed, h);
        image::imageops::replace(&mut out, &image::imageops::crop_imm(img, 0, 0, start, h).to_image(), 0, 0);
        image::imageops::replace(&mut out, &image::imageops::crop_imm(img, end, 0, w - end, h).to_image(), start as i64, 0);
        out
    };
    *img = out;
}

/// Blur strength: enough to make screen text unreadable.
const BLUR_SIGMA: f32 = 7.0;

/// Pixelation block size, in pixels.
const PIXEL_BLOCK: u32 = 12;

/// Applies an operation to the image.
pub fn apply(img: &mut RgbaImage, op: &Op, font: &FontArc) {
    match op {
        Op::Arrow { from, to, color, width, dashed, double } => {
            arrow(img, *from, *to, *color, *width, *dashed, *double);
        }
        Op::Line { from, to, color, width, dashed } => {
            stroke_points(img, &[*from, *to], *color, *width, *dashed);
        }
        Op::Rect { a, b, color, width, filled, dashed } => {
            if let Some(r) = skia_rect(*a, *b) {
                shape(img, &PathBuilder::from_rect(r), *color, *width, *filled, *dashed);
            }
        }
        Op::Ellipse { a, b, color, width, filled, dashed } => {
            if let Some(path) = skia_rect(*a, *b).and_then(PathBuilder::from_oval) {
                shape(img, &path, *color, *width, *filled, *dashed);
            }
        }
        Op::Pen { points, color, width } => stroke_points(img, points, *color, *width, false),
        // The eraser needs the original capture; only [`render_all`] has access to it.
        Op::Erase { .. } => {}
        Op::Highlight { a, b, color } => {
            if let Some(r) = skia_rect(*a, *b) {
                let mut paint = paint(*color);
                paint.blend_mode = BlendMode::Multiply;
                with_pixmap(img, |pm| pm.fill_rect(r, &paint, Transform::identity(), None));
            }
        }
        Op::Text { pos, text, color, size, boxed } => {
            if *boxed {
                let (w, h) = text_extent(text, *size, font);
                let pad = text_box_pad(*size);
                let frame = ((pos.0 - pad, pos.1 - pad), (pos.0 + w + pad, pos.1 + h + pad));
                fill_rounded(img, frame, pad, step_text_color(*color));
            }
            draw_text(img, *pos, text, *color, *size, font);
        }
        Op::Callout { tip, pos, text, color, size, style } => {
            callout(img, *tip, *pos, text, *color, *size, *style, font);
        }
        Op::Step { center, number, color, radius, shape, glyph } => {
            step(img, *center, *number, *color, *radius, *shape, *glyph, font);
        }
        Op::Stamp { kind, center, radius, color } => stamp(img, *kind, *center, *radius, *color, font),
        Op::Spotlight { a, b } => {
            let (min, max) = ordered(*a, *b);
            for (x, y, pixel) in img.enumerate_pixels_mut() {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                if px < min.0 || px > max.0 || py < min.1 || py > max.1 {
                    for c in 0..3 {
                        pixel.0[c] = (pixel.0[c] as u16 * 2 / 5) as u8;
                    }
                }
            }
        }
        Op::Pixelate { a, b } => {
            if let Some((x, y, w, h)) = pixel_rect(img, *a, *b) {
                pixelate(img, x, y, w, h);
            }
        }
        Op::Blur { a, b } => {
            if let Some((x, y, w, h)) = pixel_rect(img, *a, *b) {
                let region = image::imageops::crop_imm(img, x, y, w, h).to_image();
                let blurred = image::imageops::fast_blur(&region, BLUR_SIGMA);
                image::imageops::replace(img, &blurred, x as i64, y as i64);
            }
        }
        Op::Magnify { src, a, b } => magnify(img, *src, *a, *b),
        Op::Rotate { clockwise } => {
            *img = if *clockwise { image::imageops::rotate90(img) } else { image::imageops::rotate270(img) };
        }
        Op::Flip { horizontal } => {
            *img = if *horizontal { image::imageops::flip_horizontal(img) } else { image::imageops::flip_vertical(img) };
        }
        Op::Resize { width, height } => {
            let (w, h) = ((*width).clamp(1, MAX_SIDE), (*height).clamp(1, MAX_SIDE));
            *img = image::imageops::resize(img, w, h, image::imageops::FilterType::CatmullRom);
        }
        Op::Border { width, color } => {
            let b = (*width).min(2000);
            let mut out = RgbaImage::from_pixel(img.width() + 2 * b, img.height() + 2 * b, image::Rgba(*color));
            image::imageops::replace(&mut out, img, b as i64, b as i64);
            *img = out;
        }
        Op::Shadow { size } => *img = with_shadow(img, (*size).clamp(4, 200)),
        Op::CutOut { from, to, rows } => cut_out(img, *from, *to, *rows),
        Op::Crop { a, b } => {
            if let Some((x, y, w, h)) = pixel_rect(img, *a, *b) {
                *img = image::imageops::crop_imm(img, x, y, w, h).to_image();
            }
        }
    }
}

/// Rebuilds the image by applying every operation to the original capture.
pub fn render_all(original: &RgbaImage, ops: &[Op], font: &FontArc) -> RgbaImage {
    let mut img = original.clone();
    // Copy without annotations, which follows the crops: this is where the eraser takes its pixels
    // from.
    let mut clean = ops.iter().any(|op| matches!(op, Op::Erase { .. })).then(|| original.clone());
    for op in ops {
        match (op, &mut clean) {
            (Op::Erase { points, width }, Some(clean)) => erase(&mut img, clean, points, *width),
            (op, Some(clean)) if !op.is_object() => {
                apply(&mut img, op, font);
                apply(clean, op, font);
            }
            _ => apply(&mut img, op, font),
        }
    }
    img
}

/// Copies from `clean` to `img` the pixels covered by the stroke.
fn erase(img: &mut RgbaImage, clean: &RgbaImage, points: &[Point], width: f32) {
    let Some((first, rest)) = points.split_first() else { return };
    let (w, h) = (img.width().min(clean.width()), img.height().min(clean.height()));
    let Some(mut mask) = tiny_skia::Pixmap::new(w, h) else { return };
    let mut pb = PathBuilder::new();
    pb.move_to(first.0, first.1);
    for p in rest {
        pb.line_to(p.0, p.1);
    }
    let Some(path) = pb.finish() else { return };
    let stroke = Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
    mask.stroke_path(&path, &paint([255, 255, 255, 255]), &stroke, Transform::identity(), None);
    for (i, covered) in mask.pixels().iter().enumerate() {
        let alpha = covered.alpha();
        if alpha == 0 {
            continue;
        }
        let (x, y) = (i as u32 % w, i as u32 / w);
        let src = clean.get_pixel(x, y).0;
        let dst = img.get_pixel_mut(x, y);
        let t = alpha as f32 / 255.0;
        for c in 0..4 {
            dst.0[c] = (src[c] as f32 * t + dst.0[c] as f32 * (1.0 - t)).round() as u8;
        }
    }
}

/// Arrowhead size for a given stroke width.
pub fn arrow_head_len(width: f32) -> f32 {
    (width * 4.5).max(14.0)
}

fn paint(color: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(color[0], color[1], color[2], color[3]);
    p.anti_alias = true;
    p
}

fn with_pixmap(img: &mut RgbaImage, f: impl FnOnce(&mut PixmapMut<'_>)) {
    let (w, h) = (img.width(), img.height());
    // Screenshots are opaque, so straight and premultiplied RGBA are the same.
    if let Some(mut pm) = PixmapMut::from_bytes(img.as_mut(), w, h) {
        f(&mut pm);
    }
}

fn skia_rect(a: Point, b: Point) -> Option<Rect> {
    Rect::from_ltrb(a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

fn stroke_path(img: &mut RgbaImage, path: &tiny_skia::Path, color: Rgba, width: f32) {
    stroke_styled(img, path, color, width, false);
}

/// Solid or dashed stroke. Strokes have butt caps, so the gaps stay sharp.
fn stroke_styled(img: &mut RgbaImage, path: &tiny_skia::Path, color: Rgba, width: f32, dashed: bool) {
    let dash_len = (width * 2.5).max(4.0);
    let stroke = Stroke {
        width,
        line_cap: if dashed { LineCap::Butt } else { LineCap::Round },
        line_join: LineJoin::Round,
        dash: if dashed { StrokeDash::new(vec![dash_len, dash_len], 0.0) } else { None },
        ..Default::default()
    };
    let paint = paint(color);
    with_pixmap(img, |pm| pm.stroke_path(path, &paint, &stroke, Transform::identity(), None));
}

/// Outline (solid or dashed) or filled shape, depending on `filled`.
fn shape(img: &mut RgbaImage, path: &tiny_skia::Path, color: Rgba, width: f32, filled: bool, dashed: bool) {
    if filled {
        fill(img, path, color);
    } else {
        stroke_styled(img, path, color, width, dashed);
    }
}

fn fill(img: &mut RgbaImage, path: &tiny_skia::Path, color: Rgba) {
    let paint = paint(color);
    with_pixmap(img, |pm| pm.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None));
}

fn polygon(points: &[Point]) -> Option<tiny_skia::Path> {
    let (first, rest) = points.split_first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.0, first.1);
    for p in rest {
        pb.line_to(p.0, p.1);
    }
    pb.close();
    pb.finish()
}

/// Text centered on `center`, by cap height (digits and signs have that height).
fn centered_text(img: &mut RgbaImage, center: Point, text: &str, size: f32, color: Rgba, font: &FontArc) {
    let scaled = font.as_scaled(size);
    let cap_height = size * 0.68;
    let pos = (center.0 - text_width(text, size, font) / 2.0, center.1 + cap_height / 2.0 - scaled.ascent());
    draw_text(img, pos, text, color, size, font);
}

/// Stamp drawings: simple shapes, made right here.
fn stamp(img: &mut RgbaImage, kind: Stamp, c: Point, r: f32, color: Rgba, font: &FontArc) {
    let at = |x: f32, y: f32| (c.0 + x * r, c.1 + y * r);
    let ink = step_text_color(color);
    let disc = |img: &mut RgbaImage| {
        if let Some(circle) = PathBuilder::from_circle(c.0, c.1, r) {
            fill(img, &circle, color);
            stroke_path(img, &circle, [255, 255, 255, 255], (r * 0.1).max(1.5));
        }
    };
    match kind {
        Stamp::Check => {
            disc(img);
            stroke_points(img, &[at(-0.45, 0.02), at(-0.12, 0.35), at(0.48, -0.32)], ink, r * 0.22, false);
        }
        Stamp::Cross => {
            disc(img);
            stroke_points(img, &[at(-0.36, -0.36), at(0.36, 0.36)], ink, r * 0.22, false);
            stroke_points(img, &[at(0.36, -0.36), at(-0.36, 0.36)], ink, r * 0.22, false);
        }
        Stamp::Info => {
            disc(img);
            centered_text(img, c, "i", r * 1.4, ink, font);
        }
        Stamp::Question => {
            disc(img);
            centered_text(img, c, "?", r * 1.3, ink, font);
        }
        Stamp::Warning => {
            if let Some(triangle) = polygon(&[at(0.0, -0.85), at(0.9, 0.7), at(-0.9, 0.7)]) {
                fill(img, &triangle, color);
                // A stroke of the same color rounds the triangle's corners.
                stroke_path(img, &triangle, color, r * 0.2);
            }
            centered_text(img, at(0.0, 0.15), "!", r * 1.2, ink, font);
        }
        Stamp::Star => {
            let points: Vec<Point> = (0..10)
                .map(|i| {
                    let angle = std::f32::consts::PI * (i as f32 / 5.0 - 0.5);
                    let len = if i % 2 == 0 { 1.0 } else { 0.45 };
                    at(angle.cos() * len, angle.sin() * len)
                })
                .collect();
            if let Some(star) = polygon(&points) {
                fill(img, &star, color);
                stroke_path(img, &star, [255, 255, 255, 255], (r * 0.08).max(1.2));
            }
        }
        Stamp::Plus => {
            disc(img);
            stroke_points(img, &[at(-0.45, 0.0), at(0.45, 0.0)], ink, r * 0.22, false);
            stroke_points(img, &[at(0.0, -0.45), at(0.0, 0.45)], ink, r * 0.22, false);
        }
        Stamp::Minus => {
            disc(img);
            stroke_points(img, &[at(-0.45, 0.0), at(0.45, 0.0)], ink, r * 0.22, false);
        }
        Stamp::ArrowRight | Stamp::ArrowLeft | Stamp::ArrowUp | Stamp::ArrowDown => {
            disc(img);
            let (dx, dy) = match kind {
                Stamp::ArrowRight => (1.0, 0.0),
                Stamp::ArrowLeft => (-1.0, 0.0),
                Stamp::ArrowUp => (0.0, -1.0),
                _ => (0.0, 1.0),
            };
            let tip = at(dx * 0.5, dy * 0.5);
            let wing = |side: f32| at(dx * 0.12 - dy * 0.36 * side, dy * 0.12 + dx * 0.36 * side);
            stroke_points(img, &[at(-dx * 0.5, -dy * 0.5), tip], ink, r * 0.2, false);
            stroke_points(img, &[wing(1.0), tip, wing(-1.0)], ink, r * 0.2, false);
        }
        Stamp::Heart => {
            let points: Vec<Point> = (0..48)
                .map(|i| {
                    let t = std::f32::consts::TAU * i as f32 / 48.0;
                    let x = 16.0 * t.sin().powi(3);
                    let y = 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos();
                    at(x / 17.5, -y / 17.5 - 0.1)
                })
                .collect();
            if let Some(heart) = polygon(&points) {
                fill(img, &heart, color);
                stroke_path(img, &heart, [255, 255, 255, 255], (r * 0.08).max(1.2));
            }
        }
        Stamp::Flag => {
            if let Some(pennant) = polygon(&[at(-0.5, -0.88), at(0.8, -0.45), at(-0.5, -0.02)]) {
                fill(img, &pennant, color);
                stroke_path(img, &pennant, color, r * 0.1);
            }
            stroke_points(img, &[at(-0.5, -0.9), at(-0.5, 0.92)], [20, 20, 20, 255], r * 0.16, false);
        }
        Stamp::Pin => {
            let head = at(0.0, -0.3);
            if let Some(pin) = polygon(&drop_points(head, r * 0.62, at(0.0, 0.95))) {
                fill(img, &pin, color);
                stroke_path(img, &pin, [255, 255, 255, 255], (r * 0.08).max(1.2));
            }
            if let Some(hole) = PathBuilder::from_circle(head.0, head.1, r * 0.22) {
                fill(img, &hole, [255, 255, 255, 255]);
            }
        }
        Stamp::Cursor => {
            // Mouse pointer: always white with a dark outline, to look like a real cursor.
            let s = 0.1;
            let shape = [(0.0, 0.0), (0.0, 17.0), (4.0, 13.2), (7.0, 20.0), (9.6, 18.8), (6.7, 12.2), (12.0, 12.0)];
            let points: Vec<Point> = shape.iter().map(|(x, y)| at((x - 6.0) * s, (y - 10.0) * s)).collect();
            if let Some(pointer) = polygon(&points) {
                fill(img, &pointer, [255, 255, 255, 255]);
                stroke_path(img, &pointer, [20, 20, 20, 255], (r * 0.1).max(1.2));
            }
        }
    }
}

fn fill_rounded(img: &mut RgbaImage, (min, max): (Point, Point), radius: f32, color: Rgba) {
    let r = radius.min((max.0 - min.0) / 2.0).min((max.1 - min.1) / 2.0).max(0.0);
    let mut pb = PathBuilder::new();
    pb.move_to(min.0 + r, min.1);
    pb.line_to(max.0 - r, min.1);
    pb.quad_to(max.0, min.1, max.0, min.1 + r);
    pb.line_to(max.0, max.1 - r);
    pb.quad_to(max.0, max.1, max.0 - r, max.1);
    pb.line_to(min.0 + r, max.1);
    pb.quad_to(min.0, max.1, min.0, max.1 - r);
    pb.line_to(min.0, min.1 + r);
    pb.quad_to(min.0, min.1, min.0 + r, min.1);
    pb.close();
    if let Some(path) = pb.finish() {
        let fill = paint(color);
        with_pixmap(img, |pm| pm.fill_path(&path, &fill, FillRule::Winding, Transform::identity(), None));
    }
}

fn callout(
    img: &mut RgbaImage,
    tip: Point,
    pos: Point,
    text: &str,
    color: Rgba,
    size: f32,
    style: CalloutStyle,
    font: &FontArc,
) {
    let frame @ (min, max) = callout_box(pos, text, size, font);
    let pad = callout_pad(size);
    let radius = if style == CalloutStyle::Square { 0.0 } else { pad * 0.8 };
    // The tail is a triangle from the target to the middle of the box; the box covers its base.
    let tail = (!in_box(tip, frame, 0.0))
        .then(|| {
            let center = ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0);
            let (dx, dy) = (tip.0 - center.0, tip.1 - center.1);
            let len = (dx * dx + dy * dy).sqrt().max(1.0);
            let half = ((max.0 - min.0).min(max.1 - min.1) * 0.3).min(size * 0.9);
            let (nx, ny) = (-dy / len * half, dx / len * half);
            polygon(&[tip, (center.0 + nx, center.1 + ny), (center.0 - nx, center.1 - ny)])
        })
        .flatten();
    let white = [255, 255, 255, 255];
    let (body, ink) = if style == CalloutStyle::Outline {
        // First the whole silhouette in the color, slightly larger; the white on top leaves only
        // the border.
        let border = (size * 0.12).max(2.0);
        if let Some(tail) = &tail {
            fill(img, tail, color);
            stroke_path(img, tail, color, border * 2.0);
        }
        let outer = ((min.0 - border, min.1 - border), (max.0 + border, max.1 + border));
        fill_rounded(img, outer, radius + border, color);
        (white, color)
    } else {
        (color, step_text_color(color))
    };
    if let Some(tail) = &tail {
        fill(img, tail, body);
    }
    fill_rounded(img, frame, radius, body);
    draw_text(img, (pos.0 + pad, pos.1 + pad), text, ink, size, font);
}

fn stroke_points(img: &mut RgbaImage, points: &[Point], color: Rgba, width: f32, dashed: bool) {
    let Some((first, rest)) = points.split_first() else { return };
    let mut pb = PathBuilder::new();
    pb.move_to(first.0, first.1);
    for p in rest {
        pb.line_to(p.0, p.1);
    }
    if let Some(path) = pb.finish() {
        stroke_styled(img, &path, color, width, dashed);
    }
}

fn arrow(img: &mut RgbaImage, from: Point, to: Point, color: Rgba, width: f32, dashed: bool, double: bool) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let head = arrow_head_len(width).min(if double { len / 2.0 } else { len });
    let half = head * 0.45;
    // The shaft ends at the base of each head, so the heads stay sharp.
    let end = (to.0 - ux * head, to.1 - uy * head);
    let start = if double { (from.0 + ux * head, from.1 + uy * head) } else { from };
    stroke_points(img, &[start, end], color, width, dashed);

    let mut heads = vec![(to, end)];
    if double {
        heads.push((from, start));
    }
    for (tip, base) in heads {
        let wings = [tip, (base.0 - uy * half, base.1 + ux * half), (base.0 + uy * half, base.1 - ux * half)];
        if let Some(path) = polygon(&wings) {
            fill(img, &path, color);
        }
    }
}

/// Converts two corners into a pixel rectangle inside the image. Returns `None` if the area ends up
/// empty.
fn pixel_rect(img: &RgbaImage, a: Point, b: Point) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let left = a.0.min(b.0).clamp(0.0, w).round() as u32;
    let top = a.1.min(b.1).clamp(0.0, h).round() as u32;
    let right = a.0.max(b.0).clamp(0.0, w).round() as u32;
    let bottom = a.1.max(b.1).clamp(0.0, h).round() as u32;
    (right > left && bottom > top).then(|| (left, top, right - left, bottom - top))
}

fn pixelate(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32) {
    for by in (y..y + h).step_by(PIXEL_BLOCK as usize) {
        for bx in (x..x + w).step_by(PIXEL_BLOCK as usize) {
            let (ex, ey) = ((bx + PIXEL_BLOCK).min(x + w), (by + PIXEL_BLOCK).min(y + h));
            let mut sum = [0u32; 3];
            for py in by..ey {
                for px in bx..ex {
                    let p = img.get_pixel(px, py).0;
                    for c in 0..3 {
                        sum[c] += p[c] as u32;
                    }
                }
            }
            let n = (ex - bx) * (ey - by);
            let avg = image::Rgba([
                (sum[0] / n) as u8,
                (sum[1] / n) as u8,
                (sum[2] / n) as u8,
                255,
            ]);
            for py in by..ey {
                for px in bx..ex {
                    img.put_pixel(px, py, avg);
                }
            }
        }
    }
}

/// Color of the number on a marker of color `fill`: white on dark colors, black on light ones.
pub fn step_text_color(fill: Rgba) -> Rgba {
    let luma = (fill[0] as u32 * 3 + fill[1] as u32 * 6 + fill[2] as u32) / 10;
    if luma > 165 { [20, 20, 20, 255] } else { [255, 255, 255, 255] }
}

/// Font size of the number: shrinks with more digits to fit in the circle.
pub fn step_font_size(number: u32, radius: f32) -> f32 {
    let digits = number.max(1).ilog10() + 1;
    radius * match digits {
        1 => 1.25,
        2 => 1.0,
        _ => 0.75,
    }
}

/// Width of the text in pixels, with the same advance used by [`draw_text`].
fn text_width(text: &str, size: f32, font: &FontArc) -> f32 {
    let scaled = font.as_scaled(size);
    let mut width = 0.0;
    let mut prev = None;
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(p) = prev {
            width += scaled.kern(p, id);
        }
        width += scaled.h_advance(id);
        prev = Some(id);
    }
    width
}

/// Marker text: the number, or the letter in the same position (27 becomes "AA").
pub fn step_label(number: u32, glyph: StepGlyph) -> String {
    if glyph == StepGlyph::Number {
        return number.to_string();
    }
    let base = if glyph == StepGlyph::Upper { b'A' } else { b'a' };
    let mut letters = Vec::new();
    let mut n = number.max(1);
    while n > 0 {
        n -= 1;
        letters.push((base + (n % 26) as u8) as char);
        n /= 26;
    }
    letters.iter().rev().collect()
}

/// Font size for a marker label: shrinks with more characters.
pub fn step_label_size(label: &str, radius: f32) -> f32 {
    let len = label.chars().count().clamp(1, 3) as u32;
    step_font_size(10u32.pow(len - 1), radius)
}

/// Drop outline: a circle with a point at `tip`.
fn drop_points(c: Point, r: f32, tip: Point) -> Vec<Point> {
    let mut points = vec![tip];
    for degrees in (130..=410).step_by(10) {
        let angle = (degrees as f32).to_radians();
        points.push((c.0 + angle.cos() * r, c.1 + angle.sin() * r));
    }
    points
}

#[allow(clippy::too_many_arguments)]
fn step(
    img: &mut RgbaImage,
    center: Point,
    number: u32,
    color: Rgba,
    radius: f32,
    shape: StepShape,
    glyph: StepGlyph,
    font: &FontArc,
) {
    let at = |x: f32, y: f32| (center.0 + x * radius, center.1 + y * radius);
    let path = match shape {
        StepShape::Circle => PathBuilder::from_circle(center.0, center.1, radius),
        StepShape::Square => polygon(&[at(-0.9, -0.9), at(0.9, -0.9), at(0.9, 0.9), at(-0.9, 0.9)]),
        StepShape::Diamond => polygon(&[at(0.0, -1.15), at(1.15, 0.0), at(0.0, 1.15), at(-1.15, 0.0)]),
        StepShape::Hexagon => {
            let points: Vec<Point> = (0..6)
                .map(|i| {
                    let angle = std::f32::consts::PI * (i as f32 / 3.0);
                    at(angle.cos() * 1.1, angle.sin() * 1.1)
                })
                .collect();
            polygon(&points)
        }
        StepShape::Pin => polygon(&drop_points(center, radius, at(0.0, 1.55))),
    };
    let Some(path) = path else { return };
    fill(img, &path, color);
    // A thin white ring sets the marker apart from any background.
    stroke_path(img, &path, [255, 255, 255, 255], (radius * 0.12).max(1.5));
    let label = step_label(number, glyph);
    centered_text(img, center, &label, step_label_size(&label, radius), step_text_color(color), font);
}

fn draw_text(img: &mut RgbaImage, pos: Point, text: &str, color: Rgba, size: f32, font: &FontArc) {
    let scaled = font.as_scaled(size);
    let line_height = scaled.height() + scaled.line_gap();
    let (w, h) = (img.width() as i32, img.height() as i32);
    for (line_no, line) in text.lines().enumerate() {
        let baseline = pos.1 + scaled.ascent() + line_no as f32 * line_height;
        let mut x = pos.0;
        let mut prev = None;
        for ch in line.chars() {
            let id = scaled.glyph_id(ch);
            if let Some(p) = prev {
                x += scaled.kern(p, id);
            }
            let glyph = id.with_scale_and_position(size, ab_glyph::point(x, baseline));
            x += scaled.h_advance(id);
            prev = Some(id);
            let Some(outline) = font.outline_glyph(glyph) else { continue };
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let (px, py) = (bounds.min.x as i32 + gx as i32, bounds.min.y as i32 + gy as i32);
                if px < 0 || py < 0 || px >= w || py >= h {
                    return;
                }
                let alpha = coverage.clamp(0.0, 1.0) * (color[3] as f32 / 255.0);
                let dst = img.get_pixel_mut(px as u32, py as u32);
                for c in 0..3 {
                    dst.0[c] = (color[c] as f32 * alpha + dst.0[c] as f32 * (1.0 - alpha)).round() as u8;
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba = [255, 0, 0, 255];

    fn white(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]))
    }

    fn test_font() -> FontArc {
        crate::editor::default_font()
    }

    #[test]
    fn line_paints_its_color_on_the_path() {
        let mut img = white(100, 100);
        let op = Op::Line { from: (10.0, 50.0), to: (90.0, 50.0), color: RED, width: 6.0, dashed: false };
        apply(&mut img, &op, &test_font());
        assert_eq!(img.get_pixel(50, 50).0, RED);
        assert_eq!(img.get_pixel(50, 10).0, [255, 255, 255, 255]);
    }

    #[test]
    fn arrow_tip_and_shaft_are_painted() {
        let mut img = white(200, 100);
        let op = Op::Arrow { from: (10.0, 50.0), to: (190.0, 50.0), color: RED, width: 4.0, dashed: false, double: false };
        apply(&mut img, &op, &test_font());
        assert_eq!(img.get_pixel(100, 50).0, RED, "shaft");
        assert_eq!(img.get_pixel(180, 50).0, RED, "tip");
        // The head is wider than the shaft.
        assert_eq!(img.get_pixel(175, 55).0, RED);
        assert_eq!(img.get_pixel(100, 55).0, [255, 255, 255, 255]);
    }

    #[test]
    fn highlight_keeps_dark_text_readable() {
        let mut img = white(50, 50);
        img.put_pixel(10, 10, image::Rgba([0, 0, 0, 255]));
        let op = Op::Highlight { a: (0.0, 0.0), b: (50.0, 50.0), color: [255, 235, 60, 255] };
        apply(&mut img, &op, &test_font());
        assert_eq!(img.get_pixel(10, 10).0, [0, 0, 0, 255], "preto continua preto");
        assert_eq!(img.get_pixel(30, 30).0, [255, 235, 60, 255], "white becomes the color");
    }

    #[test]
    fn pixelate_flattens_each_block() {
        let mut img = white(24, 24);
        img.put_pixel(0, 0, image::Rgba([0, 0, 0, 255]));
        apply(&mut img, &Op::Pixelate { a: (0.0, 0.0), b: (24.0, 24.0) }, &test_font());
        let first = img.get_pixel(0, 0).0;
        assert_ne!(first, [0, 0, 0, 255]);
        assert_eq!(img.get_pixel(11, 11).0, first, "same block, same color");
        assert_eq!(img.get_pixel(12, 12).0, [255, 255, 255, 255], "bloco vizinho intacto");
    }

    #[test]
    fn crop_changes_size_and_later_ops_use_new_coordinates() {
        let ops = [
            Op::Crop { a: (50.0, 50.0), b: (90.0, 80.0) },
            Op::Line { from: (0.0, 5.0), to: (40.0, 5.0), color: RED, width: 4.0, dashed: false },
        ];
        let out = render_all(&white(100, 100), &ops, &test_font());
        assert_eq!((out.width(), out.height()), (40, 30));
        assert_eq!(out.get_pixel(20, 5).0, RED);
    }

    #[test]
    fn text_draws_pixels_and_stays_in_bounds() {
        let mut img = white(120, 40);
        let op = Op::Text { pos: (100.0, 30.0), text: "Olá, ção".into(), color: RED, size: 24.0, boxed: false };
        apply(&mut img, &op, &test_font());
        assert!(img.pixels().any(|p| p.0 != [255, 255, 255, 255]));
    }

    #[test]
    fn step_marker_is_a_filled_circle_with_a_centered_number() {
        let mut img = white(100, 100);
        let blue = [10, 60, 200, 255];
        apply(&mut img, &Op::Step { center: (50.0, 50.0), number: 7, color: blue, radius: 20.0, shape: StepShape::Circle, glyph: StepGlyph::Number }, &test_font());
        assert_eq!(img.get_pixel(50, 34).0, blue, "inside the circle, above the number");
        assert_eq!(img.get_pixel(50, 5).0, [255, 255, 255, 255], "outside the circle");
        // The number is white and sits in the middle: there are white pixels on both sides of the
        // center.
        let white_in = |x0: u32, x1: u32| (x0..x1).any(|x| (40..60).any(|y| img.get_pixel(x, y).0 == [255, 255, 255, 255]));
        assert!(white_in(42, 50) && white_in(50, 58));
    }

    #[test]
    fn step_number_adapts_to_digits_and_background() {
        assert!(step_font_size(9, 20.0) > step_font_size(10, 20.0));
        assert!(step_font_size(99, 20.0) > step_font_size(100, 20.0));
        assert_eq!(step_text_color([255, 214, 10, 255]), [20, 20, 20, 255], "yellow calls for a dark number");
        assert_eq!(step_text_color([235, 64, 52, 255]), [255, 255, 255, 255], "red calls for a white number");
    }

    #[test]
    fn objects_are_hit_on_what_they_paint() {
        let f = test_font();
        let line = Op::Line { from: (10.0, 10.0), to: (90.0, 10.0), color: RED, width: 4.0, dashed: false };
        assert!(line.hit((50.0, 12.0), 2.0, &f));
        assert!(!line.hit((50.0, 30.0), 2.0, &f));

        let rect = Op::Rect { a: (10.0, 10.0), b: (90.0, 90.0), color: RED, width: 4.0, filled: false, dashed: false };
        assert!(rect.hit((10.0, 50.0), 2.0, &f), "outline");
        assert!(!rect.hit((50.0, 50.0), 2.0, &f), "the inside doesn't count as outline");
        assert!(rect.encloses((50.0, 50.0)));

        let ellipse = Op::Ellipse { a: (10.0, 10.0), b: (90.0, 50.0), color: RED, width: 4.0, filled: false, dashed: false };
        assert!(ellipse.hit((50.0, 10.0), 2.0, &f));
        assert!(!ellipse.hit((12.0, 12.0), 2.0, &f), "box corner falls outside the ellipse");

        let step = Op::Step { center: (50.0, 50.0), number: 1, color: RED, radius: 10.0, shape: StepShape::Circle, glyph: StepGlyph::Number };
        assert!(step.hit((55.0, 55.0), 0.0, &f));
        assert!(!Op::Crop { a: (0.0, 0.0), b: (90.0, 90.0) }.hit((5.0, 5.0), 2.0, &f));
    }

    #[test]
    fn moving_an_object_moves_its_pixels() {
        let f = test_font();
        let mut op = Op::Line { from: (10.0, 20.0), to: (90.0, 20.0), color: RED, width: 6.0, dashed: false };
        op.translate(0.0, 40.0);
        let mut img = white(100, 100);
        apply(&mut img, &op, &f);
        assert_eq!(img.get_pixel(50, 60).0, RED);
        assert_eq!(img.get_pixel(50, 20).0, [255, 255, 255, 255]);
    }

    #[test]
    fn handles_resize_each_kind_of_object() {
        let f = test_font();
        let arrow = Op::Arrow { from: (10.0, 10.0), to: (50.0, 50.0), color: RED, width: 4.0, dashed: false, double: false };
        assert_eq!(
            arrow.with_handle(Handle::End, (80.0, 20.0), &f),
            Op::Arrow { from: (10.0, 10.0), to: (80.0, 20.0), color: RED, width: 4.0, dashed: false, double: false }
        );

        // Dragging the top-left corner keeps the bottom-right one in place.
        let rect = Op::Rect { a: (60.0, 60.0), b: (20.0, 20.0), color: RED, width: 4.0, filled: false, dashed: false };
        let resized = rect.with_handle(Handle::Corner(0), (5.0, 10.0), &f);
        assert_eq!(resized.bounds(&f), Some(((3.0, 8.0), (62.0, 62.0))));

        let pen = Op::Pen { points: vec![(0.0, 0.0), (10.0, 20.0)], color: RED, width: 2.0 };
        let Op::Pen { points, .. } = pen.with_handle(Handle::Corner(2), (20.0, 60.0), &f) else { panic!() };
        assert_eq!(points, vec![(0.0, 0.0), (20.0, 60.0)]);

        let step = Op::Step { center: (50.0, 50.0), number: 1, color: RED, radius: 10.0, shape: StepShape::Circle, glyph: StepGlyph::Number };
        let Op::Step { radius, .. } = step.with_handle(Handle::End, (80.0, 50.0), &f) else { panic!() };
        assert_eq!(radius, 30.0);

        let text = Op::Text { pos: (10.0, 10.0), text: "abc".into(), color: RED, size: 20.0, boxed: false };
        let (_, corner) = text.handles(&f)[0];
        let Op::Text { size, .. } = text.with_handle(Handle::Corner(2), (corner.0, 10.0 + (corner.1 - 10.0) * 2.0), &f)
        else {
            panic!()
        };
        assert!((size - 40.0).abs() < 0.01, "dobrar a altura dobra a fonte: {size}");
    }

    #[test]
    fn thickness_round_trips_through_the_slider_value() {
        let mut text = Op::Text { pos: (0.0, 0.0), text: "a".into(), color: RED, size: 10.0, boxed: false };
        text.set_thickness(4.0);
        assert_eq!(text.thickness(), Some(4.0));
        let mut step = Op::Step { center: (0.0, 0.0), number: 1, color: RED, radius: 1.0, shape: StepShape::Circle, glyph: StepGlyph::Number };
        step.set_thickness(3.0);
        assert_eq!(step.thickness(), Some(3.0));
        assert_eq!(Op::Pixelate { a: (0.0, 0.0), b: (1.0, 1.0) }.thickness(), None);
    }

    #[test]
    fn filled_shapes_paint_their_inside() {
        let f = test_font();
        let mut img = white(100, 100);
        let rect = Op::Rect { a: (10.0, 10.0), b: (90.0, 90.0), color: RED, width: 2.0, filled: true, dashed: false };
        apply(&mut img, &rect, &f);
        assert_eq!(img.get_pixel(50, 50).0, RED);
        assert!(rect.hit((50.0, 50.0), 0.0, &f), "a filled shape is picked by its inside");

        let mut img = white(100, 100);
        apply(&mut img, &Op::Ellipse { a: (10.0, 10.0), b: (90.0, 90.0), color: RED, width: 2.0, filled: true, dashed: false }, &f);
        assert_eq!(img.get_pixel(50, 50).0, RED);
        assert_eq!(img.get_pixel(12, 12).0, [255, 255, 255, 255], "corner outside the ellipse");
    }

    #[test]
    fn blur_softens_edges_only_inside_its_area() {
        let mut img = white(120, 60);
        for y in 0..60 {
            for x in 0..60 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        apply(&mut img, &Op::Blur { a: (40.0, 0.0), b: (80.0, 30.0) }, &test_font());
        let edge = img.get_pixel(59, 15).0[0];
        assert!(edge > 20 && edge < 235, "the black/white edge turns gray: {edge}");
        assert_eq!(img.get_pixel(59, 45).0, [0, 0, 0, 255], "nothing changes outside the area");
        assert_eq!(img.get_pixel(60, 45).0, [255, 255, 255, 255]);
    }

    #[test]
    fn callout_paints_a_box_with_a_tail_and_grows_with_the_text() {
        let f = test_font();
        let blue = [10, 60, 200, 255];
        let op = |text: &str| Op::Callout { tip: (20.0, 180.0), pos: (100.0, 40.0), text: text.into(), color: blue, size: 20.0, style: CalloutStyle::Rounded };
        let mut img = white(300, 200);
        apply(&mut img, &op("Click here"), &f);
        assert_eq!(img.get_pixel(104, 60).0, blue, "box");
        assert_eq!(img.get_pixel(90, 121).0, blue, "tail, on the way to the target");
        assert_eq!(img.get_pixel(280, 180).0, [255, 255, 255, 255]);
        assert!(img.pixels().any(|p| p.0 == [255, 255, 255, 255]), "white text on blue");

        let (_, one) = callout_box((0.0, 0.0), "a", 20.0, &f);
        let (_, two) = callout_box((0.0, 0.0), "a\nbcdef", 20.0, &f);
        assert!(two.0 > one.0 && two.1 > one.1);
        // The selection frame includes the tail.
        let (min, max) = op("Click here").bounds(&f).unwrap();
        assert!(min.0 <= 20.0 && max.1 >= 180.0);
    }

    #[test]
    fn boxed_text_gets_a_contrasting_background() {
        let mut img = RgbaImage::from_pixel(200, 60, image::Rgba([120, 120, 120, 255]));
        let op = Op::Text { pos: (20.0, 15.0), text: "abc".into(), color: [255, 255, 255, 255], size: 24.0, boxed: true };
        apply(&mut img, &op, &test_font());
        assert_eq!(img.get_pixel(17, 30).0, [20, 20, 20, 255], "dark background for light text");
        assert_eq!(img.get_pixel(190, 30).0, [120, 120, 120, 255]);
    }

    #[test]
    fn magnify_shows_an_enlarged_copy_somewhere_else() {
        let f = test_font();
        let mut img = white(300, 200);
        apply(&mut img, &Op::Rect { a: (40.0, 40.0), b: (60.0, 60.0), color: RED, width: 1.0, filled: true, dashed: false }, &f);
        // The 40×40 region around the square appears 2.5 times larger on the right.
        let op = Op::Magnify { src: ((30.0, 30.0), (70.0, 70.0)), a: (150.0, 50.0), b: (250.0, 150.0) };
        apply(&mut img, &op, &f);
        assert_eq!(img.get_pixel(50, 50).0, RED, "the original stays in place");
        assert_eq!(img.get_pixel(200, 100).0, RED, "enlarged copy");
        assert_eq!(img.get_pixel(180, 100).0, RED, "the 20 px square became 50");
        assert_eq!(img.get_pixel(160, 100).0, [255, 255, 255, 255]);
        assert_eq!(img.get_pixel(280, 100).0, [255, 255, 255, 255], "outside the copy");

        // Moving takes only the copy; the source stays.
        let mut moved = op.clone();
        moved.translate(10.0, 0.0);
        assert_eq!(moved, Op::Magnify { src: ((30.0, 30.0), (70.0, 70.0)), a: (160.0, 50.0), b: (260.0, 150.0) });

        // The copy's size is free, but it keeps the region's aspect ratio.
        let Op::Magnify { a, b, .. } = op.with_handle(Handle::Corner(2), (350.0, 999.0), &f) else { panic!() };
        assert_eq!((a, b), ((150.0, 50.0), (350.0, 250.0)));

        // The width control picks the magnification, around the center of the copy.
        let mut sized = op.clone();
        sized.set_thickness(4.0);
        assert_eq!(sized, Op::Magnify { src: ((30.0, 30.0), (70.0, 70.0)), a: (160.0, 60.0), b: (240.0, 140.0) });
        assert_eq!(sized.thickness(), Some(4.0));

        // A region against the right edge gets its copy on the left side.
        let (da, db) = magnify_dest((250.0, 80.0), (290.0, 120.0), 2.0, (300.0, 200.0));
        assert_eq!((da, db), ((146.0, 60.0), (226.0, 140.0)));
    }

    #[test]
    fn dashed_lines_leave_gaps() {
        let f = test_font();
        let row = |dashed| {
            let mut img = white(200, 20);
            apply(&mut img, &Op::Line { from: (0.0, 10.0), to: (200.0, 10.0), color: RED, width: 4.0, dashed }, &f);
            (0..200).filter(|&x| img.get_pixel(x, 10).0 == RED).count()
        };
        assert_eq!(row(false), 200);
        let painted = row(true);
        assert!((80..=120).contains(&painted), "half stroke, half gap: {painted}");
    }

    #[test]
    fn double_arrow_has_a_head_on_each_end() {
        let f = test_font();
        let tip_width = |double| {
            let mut img = white(200, 60);
            let op = Op::Arrow { from: (10.0, 30.0), to: (190.0, 30.0), color: RED, width: 4.0, dashed: false, double };
            apply(&mut img, &op, &f);
            (0..60).filter(|&y| img.get_pixel(24, y).0 == RED).count()
        };
        assert!(tip_width(true) > tip_width(false) + 4, "the start head is wider than the shaft");
    }

    #[test]
    fn spotlight_darkens_everything_but_its_area() {
        let mut img = white(100, 100);
        apply(&mut img, &Op::Spotlight { a: (30.0, 30.0), b: (70.0, 70.0) }, &test_font());
        assert_eq!(img.get_pixel(50, 50).0, [255, 255, 255, 255]);
        assert_eq!(img.get_pixel(10, 50).0, [102, 102, 102, 255]);
        assert_eq!(img.get_pixel(50, 90).0, [102, 102, 102, 255]);
    }

    #[test]
    fn every_stamp_paints_something_and_stays_near_its_center() {
        let f = test_font();
        for (kind, name, _) in Stamp::ALL {
            let mut img = white(100, 100);
            let op = Op::Stamp { kind, center: (50.0, 50.0), radius: 20.0, color: RED };
            apply(&mut img, &op, &f);
            let painted = img.enumerate_pixels().filter(|(_, _, p)| p.0 != [255, 255, 255, 255]);
            let far = painted.clone().filter(|(x, y, _)| distance((*x as f32, *y as f32), (50.0, 50.0)) > 26.0).count();
            assert!(painted.count() > 100, "{name} painted almost nothing");
            assert_eq!(far, 0, "{name} pintou longe do centro");
            assert!(op.hit((50.0, 50.0), 0.0, &f));
        }
        // Check is a disc in the chosen color with the light mark on top.
        let mut img = white(100, 100);
        apply(&mut img, &Op::Stamp { kind: Stamp::Check, center: (50.0, 50.0), radius: 20.0, color: RED }, &f);
        assert_eq!(img.get_pixel(50, 36).0, RED);
        assert_eq!(img.get_pixel(48, 57).0, [255, 255, 255, 255]);
    }

    #[test]
    fn step_shapes_and_letters() {
        let f = test_font();
        for (shape, name, _) in StepShape::ALL {
            let mut img = white(100, 100);
            let op = Op::Step { center: (50.0, 50.0), number: 7, color: RED, radius: 20.0, shape, glyph: StepGlyph::Number };
            apply(&mut img, &op, &f);
            assert_eq!(img.get_pixel(50, 35).0, RED, "{name}: miolo na cor");
            assert_eq!(img.get_pixel(3, 3).0, [255, 255, 255, 255], "{name}: canto livre");
        }
        // The drop has a point below the circle; the circle doesn't.
        let below = |shape| {
            let mut img = white(100, 100);
            apply(&mut img, &Op::Step { center: (50.0, 50.0), number: 1, color: RED, radius: 20.0, shape, glyph: StepGlyph::Number }, &f);
            img.get_pixel(50, 76).0
        };
        assert_eq!(below(StepShape::Pin), RED);
        assert_eq!(below(StepShape::Circle), [255, 255, 255, 255]);

        assert_eq!(step_label(3, StepGlyph::Number), "3");
        assert_eq!(step_label(1, StepGlyph::Upper), "A");
        assert_eq!(step_label(26, StepGlyph::Upper), "Z");
        assert_eq!(step_label(27, StepGlyph::Upper), "AA");
        assert_eq!(step_label(2, StepGlyph::Lower), "b");
    }

    #[test]
    fn outline_callout_is_white_with_a_colored_border() {
        let blue = [10, 60, 200, 255];
        let mut img = RgbaImage::from_pixel(300, 200, image::Rgba([200, 200, 200, 255]));
        let op = Op::Callout {
            tip: (20.0, 180.0),
            pos: (100.0, 40.0),
            text: "Click here".into(),
            color: blue,
            size: 20.0,
            style: CalloutStyle::Outline,
        };
        apply(&mut img, &op, &test_font());
        assert_eq!(img.get_pixel(104, 62).0, [255, 255, 255, 255], "miolo branco");
        assert_eq!(img.get_pixel(98, 62).0, blue, "border in the color");
        assert_eq!(img.get_pixel(90, 121).0, [255, 255, 255, 255], "ponta branca por dentro");
        assert!(img.pixels().any(|p| p.0 == blue), "text in the color");
        assert_eq!(img.get_pixel(280, 180).0, [200, 200, 200, 255]);
    }

    #[test]
    fn eraser_brings_back_the_original_capture() {
        let f = test_font();
        let gray = [200, 200, 200, 255];
        let original = RgbaImage::from_pixel(100, 100, image::Rgba(gray));
        let ops = vec![
            Op::Line { from: (10.0, 50.0), to: (90.0, 50.0), color: RED, width: 8.0, dashed: false },
            Op::Erase { points: vec![(50.0, 30.0), (50.0, 70.0)], width: 10.0 },
        ];
        let out = render_all(&original, &ops, &f);
        assert_eq!(out.get_pixel(50, 50).0, gray, "the part of the line under the eraser goes away");
        assert_eq!(out.get_pixel(20, 50).0, RED, "the rest of the line stays");
        assert_eq!(out.get_pixel(50, 10).0, gray);

        // What's drawn after the eraser isn't affected by it.
        let mut later = ops.clone();
        later.push(Op::Line { from: (40.0, 60.0), to: (60.0, 60.0), color: RED, width: 4.0, dashed: false });
        assert_eq!(render_all(&original, &later, &f).get_pixel(50, 60).0, RED);
    }

    #[test]
    fn eraser_after_a_crop_uses_the_cropped_capture() {
        let f = test_font();
        let mut original = white(100, 100);
        // Black right half: after the crop it starts at x = 10.
        for y in 0..100 {
            for x in 50..100 {
                original.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let ops = vec![
            Op::Crop { a: (40.0, 0.0), b: (100.0, 60.0) },
            Op::Rect { a: (0.0, 0.0), b: (60.0, 60.0), color: RED, width: 1.0, filled: true, dashed: false },
            Op::Erase { points: vec![(0.0, 30.0), (60.0, 30.0)], width: 10.0 },
        ];
        let out = render_all(&original, &ops, &f);
        assert_eq!((out.width(), out.height()), (60, 60));
        assert_eq!(out.get_pixel(5, 30).0, [255, 255, 255, 255]);
        assert_eq!(out.get_pixel(30, 30).0, [0, 0, 0, 255]);
        assert_eq!(out.get_pixel(30, 10).0, RED);
    }

    #[test]
    fn whole_image_edits_change_size_and_content() {
        let f = test_font();
        let mut base = white(40, 20);
        base.put_pixel(0, 0, image::Rgba(RED));
        let run = |op: Op| {
            let mut img = base.clone();
            apply(&mut img, &op, &f);
            img
        };

        let turned = run(Op::Rotate { clockwise: true });
        assert_eq!((turned.width(), turned.height()), (20, 40));
        assert_eq!(turned.get_pixel(19, 0).0, RED, "the top-left corner goes to the top-right");
        assert_eq!(run(Op::Rotate { clockwise: false }).get_pixel(0, 39).0, RED);

        assert_eq!(run(Op::Flip { horizontal: true }).get_pixel(39, 0).0, RED);
        assert_eq!(run(Op::Flip { horizontal: false }).get_pixel(0, 19).0, RED);

        let small = run(Op::Resize { width: 20, height: 10 });
        assert_eq!((small.width(), small.height()), (20, 10));

        let framed = run(Op::Border { width: 5, color: [0, 0, 255, 255] });
        assert_eq!((framed.width(), framed.height()), (50, 30));
        assert_eq!(framed.get_pixel(2, 2).0, [0, 0, 255, 255]);
        assert_eq!(framed.get_pixel(5, 5).0, RED, "the image is offset by the border");

        let shaded = run(Op::Shadow { size: 12 });
        assert_eq!((shaded.width(), shaded.height()), (64, 44));
        assert_eq!(shaded.get_pixel(12, 12).0, RED);
        assert_eq!(shaded.get_pixel(1, 1).0, [255, 255, 255, 255], "margem branca");
        assert!(shaded.get_pixel(54, 34).0[0] < 250, "shadow below and to the right");
    }

    #[test]
    fn cut_out_removes_a_band_and_joins_the_rest() {
        let f = test_font();
        let mut img = white(30, 30);
        for x in 0..30 {
            img.put_pixel(x, 5, image::Rgba(RED)); // above the strip
            img.put_pixel(x, 25, image::Rgba([0, 0, 255, 255])); // below the strip
        }
        apply(&mut img, &Op::CutOut { from: (3.0, 10.0), to: (9.0, 20.0), rows: true }, &f);
        assert_eq!((img.width(), img.height()), (30, 20));
        assert_eq!(img.get_pixel(0, 5).0, RED);
        assert_eq!(img.get_pixel(0, 15).0, [0, 0, 255, 255], "what was below moves up 10 rows");

        let mut img = white(30, 30);
        apply(&mut img, &Op::CutOut { from: (10.0, 0.0), to: (25.0, 3.0), rows: false }, &f);
        assert_eq!((img.width(), img.height()), (15, 30));
        // A strip the size of the whole image isn't accepted.
        apply(&mut img, &Op::CutOut { from: (0.0, 0.0), to: (15.0, 0.0), rows: false }, &f);
        assert_eq!((img.width(), img.height()), (15, 30));
    }

    #[test]
    fn eraser_follows_the_image_after_it_is_turned() {
        let f = test_font();
        let mut original = white(40, 20);
        original.put_pixel(0, 0, image::Rgba([0, 0, 255, 255]));
        let ops = vec![
            Op::Rotate { clockwise: true },
            Op::Rect { a: (0.0, 0.0), b: (20.0, 40.0), color: RED, width: 1.0, filled: true, dashed: false },
            Op::Erase { points: vec![(15.0, 0.0), (19.5, 0.0)], width: 2.0 },
        ];
        let out = render_all(&original, &ops, &f);
        assert_eq!(out.get_pixel(19, 0).0, [0, 0, 255, 255], "the eraser shows the already rotated capture");
        assert_eq!(out.get_pixel(5, 20).0, RED);
        assert!(!Op::Rotate { clockwise: true }.is_object() && !Op::Shadow { size: 8 }.is_object());
    }

    #[test]
    fn degenerate_shapes_do_not_panic() {
        let mut img = white(20, 20);
        let f = test_font();
        let p = (5.0, 5.0);
        for op in [
            Op::Arrow { from: p, to: p, color: RED, width: 3.0, dashed: false, double: false },
            Op::Rect { a: p, b: p, color: RED, width: 3.0, filled: false, dashed: false },
            Op::Ellipse { a: p, b: p, color: RED, width: 3.0, filled: false, dashed: false },
            Op::Pen { points: vec![], color: RED, width: 3.0 },
            Op::Pixelate { a: p, b: p },
            Op::Crop { a: p, b: p },
            Op::Crop { a: (-50.0, -50.0), b: (-10.0, -10.0) },
        ] {
            apply(&mut img, &op, &f);
        }
        assert_eq!((img.width(), img.height()), (20, 20));
    }

    /// Generates `target/demo.png` for visual inspection: `cargo test demo_png -- --ignored`.
    #[test]
    #[ignore]
    fn demo_png() {
        let mut img = white(640, 360);
        let f = test_font();
        let blue = [10, 132, 255, 255];
        apply(&mut img, &Op::Text { pos: (30.0, 30.0), text: "Password: 1234-5678 (pixelated next to it)".into(), color: [20, 20, 20, 255], size: 22.0, boxed: false }, &f);
        apply(&mut img, &Op::Text { pos: (30.0, 70.0), text: "Highlighted text: ação, coração".into(), color: [20, 20, 20, 255], size: 22.0, boxed: false }, &f);
        for op in [
            Op::Pixelate { a: (105.0, 28.0), b: (270.0, 58.0) },
            Op::Highlight { a: (25.0, 68.0), b: (420.0, 100.0), color: [255, 235, 60, 255] },
            Op::Arrow { from: (60.0, 300.0), to: (250.0, 170.0), color: RED, width: 5.0, dashed: false, double: false },
            Op::Rect { a: (300.0, 140.0), b: (450.0, 230.0), color: blue, width: 4.0, filled: false, dashed: false },
            Op::Ellipse { a: (470.0, 140.0), b: (610.0, 230.0), color: [52, 199, 89, 255], width: 4.0, filled: false, dashed: false },
            Op::Line { from: (300.0, 260.0), to: (610.0, 260.0), color: [255, 138, 76, 255], width: 8.0, dashed: false },
            Op::Pen { points: (0..60).map(|i| (300.0 + i as f32 * 5.0, 310.0 + (i as f32 * 0.5).sin() * 18.0)).collect(), color: blue, width: 3.0 },
            Op::Text { pos: (60.0, 310.0), text: "Hello!".into(), color: RED, size: 30.0, boxed: false },
            Op::Rect { a: (20.0, 120.0), b: (90.0, 160.0), color: blue, width: 4.0, filled: true, dashed: false },
            Op::Ellipse { a: (100.0, 120.0), b: (170.0, 160.0), color: [52, 199, 89, 255], width: 4.0, filled: true, dashed: false },
            Op::Blur { a: (25.0, 25.0), b: (100.0, 60.0) },
            Op::Text { pos: (30.0, 190.0), text: "Text with a background\nand two lines".into(), color: [255, 255, 255, 255], size: 18.0, boxed: true },
            Op::Callout { tip: (375.0, 185.0), pos: (180.0, 225.0), text: "A callout that\npoints".into(), color: [255, 138, 76, 255], size: 18.0, style: CalloutStyle::Rounded },
            Op::Magnify { src: ((55.0, 305.0), (115.0, 340.0)), a: (500.0, 280.0), b: (620.0, 350.0) },
            Op::Arrow { from: (200.0, 300.0), to: (290.0, 340.0), color: blue, width: 4.0, dashed: true, double: true },
            Op::Rect { a: (455.0, 125.0), b: (625.0, 245.0), color: RED, width: 3.0, filled: false, dashed: true },
            Op::Stamp { kind: Stamp::Check, center: (30.0, 255.0), radius: 14.0, color: [52, 199, 89, 255] },
            Op::Stamp { kind: Stamp::Cross, center: (64.0, 255.0), radius: 14.0, color: RED },
            Op::Stamp { kind: Stamp::Warning, center: (98.0, 255.0), radius: 14.0, color: [255, 214, 10, 255] },
            Op::Stamp { kind: Stamp::Info, center: (132.0, 255.0), radius: 14.0, color: blue },
            Op::Stamp { kind: Stamp::Question, center: (166.0, 255.0), radius: 14.0, color: [20, 20, 20, 255] },
            Op::Stamp { kind: Stamp::Star, center: (30.0, 290.0), radius: 14.0, color: [255, 138, 76, 255] },
            Op::Stamp { kind: Stamp::Cursor, center: (150.0, 300.0), radius: 14.0, color: RED },
            Op::Stamp { kind: Stamp::Plus, center: (320.0, 30.0), radius: 12.0, color: [52, 199, 89, 255] },
            Op::Stamp { kind: Stamp::Minus, center: (350.0, 30.0), radius: 12.0, color: RED },
            Op::Stamp { kind: Stamp::ArrowRight, center: (380.0, 30.0), radius: 12.0, color: blue },
            Op::Stamp { kind: Stamp::ArrowUp, center: (410.0, 30.0), radius: 12.0, color: blue },
            Op::Stamp { kind: Stamp::Heart, center: (320.0, 62.0), radius: 12.0, color: RED },
            Op::Stamp { kind: Stamp::Flag, center: (350.0, 62.0), radius: 12.0, color: RED },
            Op::Stamp { kind: Stamp::Pin, center: (380.0, 62.0), radius: 12.0, color: RED },
            Op::Step { center: (440.0, 110.0), number: 1, color: RED, radius: 13.0, shape: StepShape::Square, glyph: StepGlyph::Number },
            Op::Step { center: (475.0, 110.0), number: 2, color: blue, radius: 13.0, shape: StepShape::Diamond, glyph: StepGlyph::Number },
            Op::Step { center: (510.0, 110.0), number: 3, color: [52, 199, 89, 255], radius: 13.0, shape: StepShape::Hexagon, glyph: StepGlyph::Upper },
            Op::Step { center: (545.0, 105.0), number: 4, color: [255, 138, 76, 255], radius: 13.0, shape: StepShape::Pin, glyph: StepGlyph::Lower },
            Op::Callout { tip: (300.0, 110.0), pos: (330.0, 85.0), text: "Contorno".into(), color: blue, size: 15.0, style: CalloutStyle::Outline },
            Op::Step { center: (470.0, 60.0), number: 1, color: RED, radius: 20.0, shape: StepShape::Circle, glyph: StepGlyph::Number },
            Op::Step { center: (525.0, 60.0), number: 12, color: blue, radius: 20.0, shape: StepShape::Circle, glyph: StepGlyph::Number },
            Op::Step { center: (580.0, 60.0), number: 3, color: [255, 214, 10, 255], radius: 20.0, shape: StepShape::Circle, glyph: StepGlyph::Number },
            Op::Step { center: (615.0, 100.0), number: 128, color: [20, 20, 20, 255], radius: 14.0, shape: StepShape::Circle, glyph: StepGlyph::Number },
        ] {
            apply(&mut img, &op, &f);
        }
        img.save("target/demo.png").unwrap();
    }
}
