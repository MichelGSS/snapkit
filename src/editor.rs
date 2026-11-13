//! Annotation editor: toolbar, drawing canvas and history.

use ab_glyph::FontArc;
use eframe::egui;
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2};
use image::RgbaImage;

use crate::render::{self, CalloutStyle, Handle, Op, Point, Rgba, Stamp, StepGlyph, StepShape};

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Tool {
    /// Picks an existing annotation to move, resize, change or delete.
    Select,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Pen,
    Highlight,
    Text,
    Pixelate,
    /// Brings back the original capture wherever it passes, erasing what was drawn there.
    Erase,
    /// Soft blur, an alternative to pixelation.
    Blur,
    /// Darkens everything except the dragged area.
    Spotlight,
    /// Ready-made symbols (check, cross, warning…), one per click.
    Stamp,
    /// Magnifier: copies the dragged region and shows the copy enlarged next to it.
    Magnify,
    /// Speech balloon: drag from the target to where the box goes, then type the text.
    Callout,
    Crop,
    /// Removes a strip from the middle of the image and joins the two parts.
    CutOut,
    /// Doesn't change the image: marks an area to have its text recognized (OCR).
    GrabText,
    /// Numbered markers in sequence (1, 2, 3…), one per click.
    Step,
}

/// Each tool with its name in Portuguese and in English.
const TOOLS: [(Tool, &str, &str); 19] = [
    (Tool::Select, "Selecionar", "Select"),
    (Tool::Erase, "Borracha", "Eraser"),
    (Tool::Arrow, "Seta", "Arrow"),
    (Tool::Line, "Linha", "Line"),
    (Tool::Rect, "Retângulo", "Rectangle"),
    (Tool::Ellipse, "Elipse", "Ellipse"),
    (Tool::Pen, "Caneta", "Pen"),
    (Tool::Highlight, "Marca-texto", "Highlighter"),
    (Tool::Text, "Texto", "Text"),
    (Tool::Callout, "Balão", "Callout"),
    (Tool::Pixelate, "Pixelizar", "Pixelate"),
    (Tool::Blur, "Desfocar", "Blur"),
    (Tool::Magnify, "Lupa", "Magnifier"),
    (Tool::Spotlight, "Holofote", "Spotlight"),
    (Tool::Stamp, "Carimbo", "Stamp"),
    (Tool::Crop, "Recortar", "Crop"),
    (Tool::CutOut, "Cortar faixa", "Cut out"),
    (Tool::GrabText, "Extrair texto", "Grab text"),
    (Tool::Step, "Passo", "Step"),
];

/// A tool and options combination kept for reuse with one click.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Style {
    pub name: String,
    tool: Tool,
    color: Rgba,
    width: f32,
    filled: bool,
    boxed: bool,
    dashed: bool,
    double: bool,
    stamp: Stamp,
    step_shape: StepShape,
    step_glyph: StepGlyph,
    callout_style: CalloutStyle,
}

/// Tools in the compact quick annotation bar, over the screen.
const QUICK_TOOLS: [Tool; 9] = [
    Tool::Select,
    Tool::Arrow,
    Tool::Rect,
    Tool::Pen,
    Tool::Highlight,
    Tool::Text,
    Tool::Step,
    Tool::Pixelate,
    Tool::Blur,
];

/// Tools in the order and groups in which they appear in the left column.
const TOOL_GROUPS: [(&str, &str, &[Tool]); 6] = [
    ("Editar", "Edit", &[Tool::Select, Tool::Erase]),
    ("Desenhar", "Draw", &[Tool::Arrow, Tool::Line, Tool::Rect, Tool::Ellipse, Tool::Pen]),
    ("Explicar", "Explain", &[Tool::Text, Tool::Callout, Tool::Step, Tool::Stamp]),
    ("Destacar", "Highlight", &[Tool::Highlight, Tool::Spotlight, Tool::Magnify]),
    ("Ocultar", "Hide", &[Tool::Pixelate, Tool::Blur]),
    ("Imagem", "Image", &[Tool::Crop, Tool::CutOut, Tool::GrabText]),
];

/// The name in Portuguese or in English, depending on the interface language.
fn pick(pt: &'static str, en: &'static str) -> &'static str {
    if crate::i18n::english() { en } else { pt }
}

fn tool_label(tool: Tool) -> &'static str {
    TOOLS.iter().find(|(t, _, _)| *t == tool).map_or("", |(_, pt, en)| pick(pt, en))
}

/// How to use the tool, in one line.
fn tool_hint(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => t!(
            "Clique em uma anotação para mover, redimensionar, mudar ou apagar",
            "Click an annotation to move, resize, change or delete it"
        ),
        Tool::Erase => t!(
            "Passe sobre uma anotação para apagar só esse trecho e mostrar a captura de novo",
            "Pass over an annotation to erase just that part and show the capture again"
        ),
        Tool::Text => t!(
            "Clique onde o texto começa  •  Enter confirma  •  Shift+Enter abre outra linha",
            "Click where the text starts  •  Enter confirms  •  Shift+Enter starts a new line"
        ),
        Tool::Callout => t!(
            "Arraste do ponto a apontar até onde o balão fica, depois digite",
            "Drag from the point to show to where the callout goes, then type"
        ),
        Tool::Step => t!("Cada clique coloca o próximo da sequência", "Each click places the next one in the sequence"),
        Tool::Stamp => t!("Cada clique coloca o símbolo escolhido", "Each click places the chosen symbol"),
        Tool::Magnify => t!(
            "Arraste sobre o trecho a ampliar; depois leve a cópia para onde quiser",
            "Drag over the part to enlarge; then move the copy wherever you like"
        ),
        Tool::Spotlight => t!("Arraste sobre a área que deve continuar clara", "Drag over the area that should stay bright"),
        Tool::Pixelate | Tool::Blur => t!("Arraste sobre o que deve ficar ilegível", "Drag over what should become unreadable"),
        Tool::Crop => t!("Arraste sobre a parte da imagem que fica", "Drag over the part of the image to keep"),
        Tool::CutOut => t!(
            "Arraste para baixo para tirar uma faixa horizontal, ou para o lado para tirar uma vertical",
            "Drag down to remove a horizontal strip, or sideways to remove a vertical one"
        ),
        Tool::GrabText => t!("Arraste sobre o texto a reconhecer e copiar", "Drag over the text to recognize and copy"),
        Tool::Arrow | Tool::Line | Tool::Rect | Tool::Ellipse | Tool::Pen | Tool::Highlight => {
            t!("Arraste sobre a imagem", "Drag over the image")
        }
    }
}

/// Tool that creates the `op` object type.
fn tool_for(op: &Op) -> Tool {
    match op {
        Op::Arrow { .. } => Tool::Arrow,
        Op::Line { .. } => Tool::Line,
        Op::Rect { .. } => Tool::Rect,
        Op::Ellipse { .. } => Tool::Ellipse,
        Op::Pen { .. } => Tool::Pen,
        Op::Erase { .. } => Tool::Erase,
        Op::Highlight { .. } => Tool::Highlight,
        Op::Text { .. } => Tool::Text,
        Op::Callout { .. } => Tool::Callout,
        Op::Step { .. } => Tool::Step,
        Op::Stamp { .. } => Tool::Stamp,
        Op::Spotlight { .. } => Tool::Spotlight,
        Op::Pixelate { .. } => Tool::Pixelate,
        Op::Blur { .. } => Tool::Blur,
        Op::Magnify { .. } => Tool::Magnify,
        Op::Crop { .. } | Op::Rotate { .. } | Op::Flip { .. } | Op::Resize { .. } | Op::Border { .. } | Op::Shadow { .. } | Op::CutOut { .. } => Tool::Crop,
    }
}

/// Which options make sense in the bar, depending on the tool or the selected object.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct Relevant {
    color: bool,
    /// Name of the slider, if it applies: stroke width, size or magnification.
    thickness: Option<&'static str>,
    filled: bool,
    dashed: bool,
    double: bool,
    boxed: bool,
    step: bool,
    callout: bool,
    stamp: bool,
}

impl Relevant {
    fn for_tool(tool: Tool) -> Self {
        let stroke = Self { color: true, thickness: Some(t!("Espessura", "Thickness")), ..Self::default() };
        let sized = Self { color: true, thickness: Some(t!("Tamanho", "Size")), ..Self::default() };
        match tool {
            Tool::Arrow => Self { dashed: true, double: true, ..stroke },
            Tool::Line => Self { dashed: true, ..stroke },
            Tool::Rect | Tool::Ellipse => Self { filled: true, dashed: true, ..stroke },
            Tool::Pen => stroke,
            Tool::Erase => Self { thickness: Some(t!("Espessura", "Thickness")), ..Self::default() },
            Tool::Text => Self { boxed: true, ..sized },
            Tool::Callout => Self { callout: true, ..sized },
            Tool::Step => Self { step: true, ..sized },
            Tool::Stamp => Self { stamp: true, ..sized },
            Tool::Magnify => Self { thickness: Some(t!("Ampliação", "Zoom")), ..Self::default() },
            Tool::Select
            | Tool::Highlight
            | Tool::Spotlight
            | Tool::Pixelate
            | Tool::Blur
            | Tool::Crop
            | Tool::CutOut
            | Tool::GrabText => Self::default(),
        }
    }

    fn for_op(op: &Op) -> Self {
        // A finished highlight can change color, although the tool always starts in yellow.
        Self { color: op.color().is_some(), ..Self::for_tool(tool_for(op)) }
    }
}

const PALETTE: [Color32; 6] = [
    Color32::from_rgb(235, 64, 52),
    Color32::from_rgb(255, 138, 76),
    Color32::from_rgb(255, 214, 10),
    Color32::from_rgb(52, 199, 89),
    Color32::from_rgb(10, 132, 255),
    Color32::from_rgb(20, 20, 20),
];

const HIGHLIGHT_COLOR: Rgba = [255, 235, 60, 255];

/// egui's default font, reused to draw text on the image.
pub fn default_font() -> FontArc {
    let defs = egui::FontDefinitions::default();
    let name = defs
        .families
        .get(&egui::FontFamily::Proportional)
        .and_then(|names| names.first())
        .expect("egui without a default proportional font");
    let data = &defs.font_data[name];
    FontArc::try_from_vec((*data.font).as_ref().to_vec()).expect("invalid egui default font")
}

/// Editing history, kept apart from the interface so it can be tested.
///
/// Annotations are stored as a list of objects, and the image is redrawn from the original capture.
/// That's why an object can be changed or deleted after it's done. Undo takes the whole list back
/// to an earlier state.
pub struct History {
    original: RgbaImage,
    ops: Vec<Op>,
    undo: Vec<Vec<Op>>,
    redo: Vec<Vec<Op>>,
    current: RgbaImage,
    font: FontArc,
}

impl History {
    pub fn new(original: RgbaImage, font: FontArc) -> Self {
        Self { current: original.clone(), original, ops: Vec::new(), undo: Vec::new(), redo: Vec::new(), font }
    }

    pub fn image(&self) -> &RgbaImage {
        &self.current
    }

    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    pub fn font(&self) -> &FontArc {
        &self.font
    }

    /// Index of the first object that can still be selected. Whatever came before the last crop is
    /// in another coordinate system and stays fixed in the image.
    pub fn first_object(&self) -> usize {
        self.ops.iter().rposition(|op| !op.is_object()).map_or(0, |i| i + 1)
    }

    /// Saves the current state as a point that Undo goes back to.
    pub fn checkpoint(&mut self) {
        self.undo.push(self.ops.clone());
        self.redo.clear();
    }

    pub fn push(&mut self, op: Op) {
        self.checkpoint();
        // The eraser depends on the original capture, so the whole image is rebuilt.
        if matches!(op, Op::Erase { .. }) {
            self.ops.push(op);
            self.rerender();
            return;
        }
        render::apply(&mut self.current, &op, &self.font);
        self.ops.push(op);
    }

    /// Replaces an object without creating an undo point: the caller uses [`Self::checkpoint`] once
    /// at the start of the gesture, so a whole drag is undone at once.
    pub fn set(&mut self, index: usize, op: Op) {
        if let Some(slot) = self.ops.get_mut(index) {
            *slot = op;
            self.rerender();
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.ops.len() {
            self.checkpoint();
            self.ops.remove(index);
            self.rerender();
        }
    }

    fn rerender(&mut self) {
        self.current = render::render_all(&self.original, &self.ops, &self.font);
    }

    /// Replaces the original capture and shifts every object (including the ones kept for undo and
    /// redo) by `(dx, dy)` pixels.
    pub fn rebase(&mut self, original: RgbaImage, dx: f32, dy: f32) {
        self.original = original;
        if dx != 0.0 || dy != 0.0 {
            let saved = self.undo.iter_mut().chain(self.redo.iter_mut()).flatten();
            for op in self.ops.iter_mut().chain(saved) {
                op.translate(dx, dy);
            }
        }
        self.rerender();
    }

    /// Number of the next step marker: one more than the largest in the image. Undoing a marker
    /// gives its number back.
    pub fn next_step(&self) -> u32 {
        let highest = self.ops.iter().filter_map(|op| match op {
            Op::Step { number, .. } => Some(*number),
            _ => None,
        });
        highest.max().unwrap_or(0) + 1
    }

    /// Forgets the undo and redo steps; the current state becomes the starting point.
    pub fn forget_undo(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.ops, previous));
        self.rerender();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.ops, next));
        self.rerender();
        true
    }
}

struct PendingText {
    pos: Point,
    text: String,
    focused: bool,
    /// Index of the text being rewritten; `None` for a new text.
    edit: Option<usize>,
    /// Text of a new callout: where the tail points. `pos` is then the center of the box.
    callout_tip: Option<Point>,
}

/// Dragging a selected object: the whole object or one of its control points.
struct ObjDrag {
    index: usize,
    handle: Option<Handle>,
    start: Point,
    orig: Op,
    current: Op,
}

/// Above this size, the image is redrawn only when the mouse is released; during the drag only the
/// outline shows. Redrawing a huge scrolling capture every frame would stall.
const LIVE_PIXELS: u64 = 12_000_000;

pub struct Editor {
    history: History,
    texture: TextureHandle,
    /// Largest texture side the graphics card accepts (see [`texture_image`]).
    max_texture: usize,
    /// Accent color of the current theme (outlines and control points).
    accent: Color32,
    tool: Tool,
    color: Color32,
    width: f32,
    /// Rectangles and ellipses come out filled instead of just outlined.
    filled: bool,
    /// Texts come out with a background box.
    boxed: bool,
    /// Arrows, lines and outlines come out dashed.
    dashed: bool,
    /// Arrows come out with a head on both ends.
    double: bool,
    /// Symbol that the Stamp tool places.
    stamp: Stamp,
    step_shape: StepShape,
    step_glyph: StepGlyph,
    callout_style: CalloutStyle,
    /// Name typed in the Styles menu for the next style to save.
    style_name: String,
    /// Fixed rectangle in which to draw the image this frame (quick annotation); `None` in the
    /// regular editor.
    fixed_rect: Option<Rect>,
    /// Image menu: the image size when the fields were filled in, and the requested size.
    resize_from: (u32, u32),
    resize_to: (u32, u32),
    resize_lock: bool,
    /// Border width in the Image menu, in pixels.
    border_px: u32,
    drag_start: Option<Point>,
    pen_points: Vec<Point>,
    pending_text: Option<PendingText>,
    /// Area marked with the Grab text tool, waiting for whoever runs the OCR.
    text_request: Option<(Point, Point)>,
    /// Grows with every change to the image; whoever saves compares it with the last saved value.
    revision: u64,
    changed_at: std::time::Instant,
    /// Index, in the history list, of the object selected with the Select tool.
    selected: Option<usize>,
    drag: Option<ObjDrag>,
    /// There's already an undo point for the gesture in progress.
    gesture_open: bool,
    /// Zoom chosen by the user; `None` fits the whole image in the window.
    zoom: Option<f32>,
    /// Offset of the image center from the center of the drawing area, in screen points.
    pan: Vec2,
    /// Scale used in the last frame (screen points per image pixel).
    view_scale: f32,
}

const ZOOM_MIN: f32 = 0.05;
const ZOOM_MAX: f32 = 16.0;
const ZOOM_STEP: f32 = 1.25;

impl Editor {
    pub fn new(ctx: &egui::Context, image: RgbaImage, accent: Color32) -> Self {
        let max_texture = ctx.input(|i| i.max_texture_side);
        let texture = ctx.load_texture("editing", texture_image(&image, max_texture), TextureOptions::LINEAR);
        Self {
            history: History::new(image, default_font()),
            texture,
            max_texture,
            accent,
            tool: Tool::Arrow,
            color: PALETTE[0],
            width: 4.0,
            filled: false,
            boxed: false,
            dashed: false,
            double: false,
            stamp: Stamp::Check,
            step_shape: StepShape::Circle,
            step_glyph: StepGlyph::Number,
            callout_style: CalloutStyle::Rounded,
            style_name: String::new(),
            fixed_rect: None,
            resize_from: (0, 0),
            resize_to: (0, 0),
            resize_lock: true,
            border_px: 4,
            drag_start: None,
            pen_points: Vec::new(),
            pending_text: None,
            text_request: None,
            revision: 0,
            changed_at: std::time::Instant::now(),
            selected: None,
            drag: None,
            gesture_open: false,
            zoom: None,
            pan: Vec2::ZERO,
            view_scale: 1.0,
        }
    }

    /// Changes the accent color (outlines and control points) when the theme changes.
    pub fn set_accent(&mut self, accent: Color32) {
        self.accent = accent;
    }

    /// Nothing has been annotated yet.
    pub fn is_blank(&self) -> bool {
        self.history.ops().is_empty()
    }

    /// Only the annotations, on a transparent background: what follows the cursor while the quick
    /// annotation area is moved, with the live screen underneath.
    pub fn annotations_image(&self) -> egui::ColorImage {
        let (w, h) = self.history.image().dimensions();
        let clear = RgbaImage::new(w, h);
        to_color_image(&render::render_all(&clear, self.history.ops(), self.history.font()))
    }

    /// Replaces the photo underneath (the quick annotation area was moved or resized) and keeps the
    /// annotations, shifted by `(dx, dy)` pixels, along with undo and redo.
    pub fn rebase(&mut self, image: RgbaImage, dx: f32, dy: f32) {
        self.history.rebase(image, dx, dy);
        self.drag_start = None;
        self.pen_points.clear();
        self.pending_text = None;
        self.text_request = None;
        self.drag = None;
        self.gesture_open = false;
        self.zoom = None;
        self.pan = Vec2::ZERO;
        self.refresh_texture();
    }

    /// Editor that opens with ready-made annotations on the image, still movable and editable.
    pub fn with_ops(ctx: &egui::Context, image: RgbaImage, ops: Vec<Op>, accent: Color32) -> Self {
        let mut editor = Self::new(ctx, image, accent);
        for op in ops {
            editor.history.push(op);
        }
        // Opening doesn't count as an edit: Undo must not remove what came ready-made.
        editor.history.forget_undo();
        editor.refresh_texture();
        editor.tool = Tool::Select;
        editor
    }

    /// Final image, with every annotation applied.
    pub fn image(&self) -> &RgbaImage {
        self.history.image()
    }

    /// The part of the image marked with Grab text, if there's a pending request.
    pub fn take_text_request(&mut self) -> Option<RgbaImage> {
        let (a, b) = self.text_request.take()?;
        crate::capture::crop(self.history.image(), a.0, a.1, b.0, b.1)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Time since the last change to the image.
    pub fn idle_time(&self) -> std::time::Duration {
        self.changed_at.elapsed()
    }

    /// Every change to the image goes through here.
    fn refresh_texture(&mut self) {
        self.texture.set(texture_image(self.history.image(), self.max_texture), TextureOptions::LINEAR);
        self.revision += 1;
        self.changed_at = std::time::Instant::now();
    }

    fn commit(&mut self, op: Op) {
        self.history.push(op);
        self.refresh_texture();
    }

    fn undo(&mut self) {
        self.selected = None;
        self.drag = None;
        if self.history.undo() {
            self.refresh_texture();
        }
    }

    fn redo(&mut self) {
        self.selected = None;
        self.drag = None;
        if self.history.redo() {
            self.refresh_texture();
        }
    }

    /// Changes the zoom while keeping in place whatever is at the center of the drawing area.
    fn set_zoom(&mut self, zoom: f32) {
        let zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        self.pan *= zoom / self.view_scale;
        self.zoom = Some(zoom);
    }

    /// Shows the whole image in the window again.
    fn zoom_to_fit(&mut self) {
        self.zoom = None;
        self.pan = Vec2::ZERO;
    }

    fn rgba(&self) -> Rgba {
        self.color.to_array()
    }

    /// Radius of the step marker, in image pixels.
    fn step_radius(&self) -> f32 {
        render::step_radius_for(self.width)
    }

    fn text_size(&self) -> f32 {
        render::text_size_for(self.width)
    }

    /// Tool column, grouped by what the tools do.
    pub fn tools_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                for (pt, en, tools) in TOOL_GROUPS {
                    ui.add_space(6.0);
                    ui.weak(pick(pt, en));
                    for tool in tools {
                        let clicked =
                            ui.selectable_label(self.tool == *tool, tool_label(*tool)).on_hover_text(tool_hint(*tool)).clicked();
                        if clicked {
                            self.tool = *tool;
                            self.pending_text = None;
                            self.selected = None;
                            self.drag = None;
                        }
                    }
                }
            });
        });
    }

    /// Zoom controls, for the bottom bar. The caller uses a right-to-left layout.
    pub fn zoom_ui(&mut self, ui: &mut egui::Ui) {
        if ui.button("100%").on_hover_text(t!("Tamanho real (Ctrl 1)", "Actual size (Ctrl 1)")).clicked() {
            self.set_zoom(1.0);
        }
        if ui.button(t!("Ajustar", "Fit")).on_hover_text(t!("Imagem inteira na janela (Ctrl 0)", "Whole image in the window (Ctrl 0)")).clicked() {
            self.zoom_to_fit();
        }
        if ui.button("+").on_hover_text(t!("Aumentar (Ctrl + ou Ctrl e a roda do mouse)", "Zoom in (Ctrl + or Ctrl and the mouse wheel)")).clicked() {
            self.set_zoom(self.view_scale * ZOOM_STEP);
        }
        ui.label(format!("{:.0}%", self.view_scale * 100.0));
        if ui.button("-").on_hover_text(t!("Diminuir (Ctrl -)", "Zoom out (Ctrl -)")).clicked() {
            self.set_zoom(self.view_scale / ZOOM_STEP);
        }
        ui.separator();
        let img = self.history.image();
        ui.weak(format!("{} × {} px", img.width(), img.height()));
    }

    /// Current tool and options as a style. With an object selected, its type applies.
    fn current_style(&self, name: String) -> Style {
        let tool = self.selection().map_or(self.tool, |i| tool_for(&self.history.ops()[i]));
        Style {
            name,
            tool,
            color: self.rgba(),
            width: self.width,
            filled: self.filled,
            boxed: self.boxed,
            dashed: self.dashed,
            double: self.double,
            stamp: self.stamp,
            step_shape: self.step_shape,
            step_glyph: self.step_glyph,
            callout_style: self.callout_style,
        }
    }

    /// Switches to the style's tool, with its options already set.
    fn apply_style(&mut self, style: &Style) {
        let [r, g, b, a] = style.color;
        self.tool = style.tool;
        self.color = Color32::from_rgba_unmultiplied(r, g, b, a);
        self.width = style.width.clamp(1.0, 24.0);
        (self.filled, self.boxed, self.dashed, self.double) = (style.filled, style.boxed, style.dashed, style.double);
        (self.stamp, self.step_shape, self.step_glyph) = (style.stamp, style.step_shape, style.step_glyph);
        self.callout_style = style.callout_style;
        self.pending_text = None;
        self.selected = None;
        self.drag = None;
    }

    /// Styles menu: apply, save the current one or delete. Returns `true` if the list changed.
    fn styles_menu(&mut self, ui: &mut egui::Ui, styles: &mut Vec<Style>) -> bool {
        let mut changed = false;
        ui.menu_button(t!("Estilos", "Styles"), |ui| {
            ui.set_min_width(240.0);
            if styles.is_empty() {
                ui.weak(t!("Nenhum estilo salvo ainda.", "No saved style yet."));
            }
            let (mut apply, mut remove) = (None, None);
            for (i, style) in styles.iter().enumerate() {
                ui.horizontal(|ui| {
                    let [r, g, b, a] = style.color;
                    let (swatch, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                    ui.painter().rect_filled(swatch, 3.0, Color32::from_rgba_unmultiplied(r, g, b, a));
                    if ui.button(&style.name).on_hover_text(tool_label(style.tool)).clicked() {
                        apply = Some(i);
                    }
                    if ui.small_button("×").on_hover_text(t!("Apagar este estilo", "Delete this style")).clicked() {
                        remove = Some(i);
                    }
                });
            }
            ui.separator();
            ui.weak(t!("Guardar a ferramenta e as opções de agora", "Save the current tool and options"));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.style_name).hint_text(t!("Nome (opcional)", "Name (optional)")).desired_width(130.0));
                if ui.button(t!("Salvar", "Save")).clicked() {
                    let typed = self.style_name.trim().to_owned();
                    let mut style = self.current_style(typed);
                    if style.name.is_empty() {
                        style.name = format!("{} {}", tool_label(style.tool), styles.len() + 1);
                    }
                    styles.push(style);
                    self.style_name.clear();
                    changed = true;
                }
            });
            if let Some(i) = apply {
                let style = styles[i].clone();
                self.apply_style(&style);
                ui.close();
            }
            if let Some(i) = remove {
                styles.remove(i);
                changed = true;
            }
        });
        changed
    }

    /// Image menu: changes that apply to the whole image.
    fn image_menu(&mut self, ui: &mut egui::Ui) {
        let (iw, ih) = (self.history.image().width(), self.history.image().height());
        // The size fields follow the image whenever it changes.
        if self.resize_from != (iw, ih) {
            self.resize_from = (iw, ih);
            self.resize_to = (iw, ih);
        }
        let mut op = None;
        ui.menu_button(t!("Imagem", "Image"), |ui| {
            ui.set_min_width(240.0);
            if ui.button(t!("Girar para a esquerda", "Rotate left")).clicked() {
                op = Some(Op::Rotate { clockwise: false });
            }
            if ui.button(t!("Girar para a direita", "Rotate right")).clicked() {
                op = Some(Op::Rotate { clockwise: true });
            }
            if ui.button(t!("Espelhar na horizontal", "Flip horizontally")).clicked() {
                op = Some(Op::Flip { horizontal: true });
            }
            if ui.button(t!("Espelhar na vertical", "Flip vertically")).clicked() {
                op = Some(Op::Flip { horizontal: false });
            }
            ui.separator();
            ui.weak(t!("Tamanho", "Size"));
            ui.horizontal(|ui| {
                let (mut w, mut h) = self.resize_to;
                let w_changed = ui.add(egui::DragValue::new(&mut w).range(1..=20000).suffix(" px")).changed();
                ui.label("×");
                let h_changed = ui.add(egui::DragValue::new(&mut h).range(1..=20000).suffix(" px")).changed();
                if self.resize_lock {
                    if w_changed {
                        h = scaled_side(w, iw, ih);
                    } else if h_changed {
                        w = scaled_side(h, ih, iw);
                    }
                }
                self.resize_to = (w, h);
            });
            ui.checkbox(&mut self.resize_lock, t!("Manter proporção", "Keep proportions"));
            let (width, height) = self.resize_to;
            if ui.add_enabled((width, height) != (iw, ih), egui::Button::new(t!("Aplicar tamanho", "Apply size"))).clicked() {
                op = Some(Op::Resize { width, height });
            }
            ui.separator();
            ui.weak(t!("Moldura", "Frame"));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.border_px).range(1..=200).suffix(" px"));
                if ui.button(t!("Borda na cor atual", "Border in the current color")).clicked() {
                    op = Some(Op::Border { width: self.border_px, color: self.rgba() });
                }
            });
            if ui
                .button(t!("Sombra", "Shadow"))
                .on_hover_text(t!("Margem branca com sombra, para colar em documentos", "White margin with a shadow, for pasting into documents"))
                .clicked()
            {
                op = Some(Op::Shadow { size: 24 });
            }
        });
        if let Some(op) = op {
            self.pending_text = None;
            self.selected = None;
            self.drag = None;
            self.commit(op);
        }
    }

    /// Options bar: shows only what applies to the active tool or the selected object. `styles` is
    /// the list of saved styles; the return value says whether it changed and needs to be saved.
    pub fn options_ui(&mut self, ui: &mut egui::Ui, styles: &mut Vec<Style>) -> bool {
        let mut styles_changed = false;
        let before = (self.color, self.width, self.filled, self.boxed, self.dashed, self.double, self.stamp);
        let before_style = (self.step_shape, self.step_glyph, self.callout_style);
        let selection = self.selection();
        let selected_op = selection.map(|i| self.history.ops()[i].clone());
        let show = match &selected_op {
            Some(op) => Relevant::for_op(op),
            None => Relevant::for_tool(self.tool),
        };
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(self.history.can_undo(), egui::Button::new(t!("↶ Desfazer", "↶ Undo"))).on_hover_text("Ctrl Z").clicked() {
                self.undo();
            }
            if ui.add_enabled(self.history.can_redo(), egui::Button::new(t!("↷ Refazer", "↷ Redo"))).on_hover_text("Ctrl Y").clicked() {
                self.redo();
            }
            styles_changed = self.styles_menu(ui, styles);
            self.image_menu(ui);
            ui.separator();
            match &selected_op {
                Some(op) => ui.strong(tf!("Selecionado: {}", "Selected: {}", tool_label(tool_for(op)))),
                None => ui.strong(tool_label(self.tool)),
            };
            if show.color {
                ui.separator();
                for c in PALETTE {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                    ui.painter().rect_filled(rect, 4.0, c);
                    if self.color == c {
                        ui.painter().rect_stroke(rect, 4.0, Stroke::new(2.0, ui.visuals().strong_text_color()), StrokeKind::Outside);
                    }
                    if resp.clicked() {
                        self.color = c;
                    }
                }
                ui.color_edit_button_srgba(&mut self.color).on_hover_text(t!("Outra cor", "Other color"));
            }
            if let Some(label) = show.thickness {
                ui.separator();
                ui.label(label);
                ui.add(egui::Slider::new(&mut self.width, 1.0..=24.0).fixed_decimals(0));
            }
            if show.filled {
                ui.checkbox(&mut self.filled, t!("Preencher", "Fill"))
                    .on_hover_text(t!("Forma cheia em vez de só o contorno", "Solid shape instead of just the outline"));
            }
            if show.dashed {
                ui.checkbox(&mut self.dashed, t!("Tracejado", "Dashed"));
            }
            if show.double {
                ui.checkbox(&mut self.double, t!("Duas pontas", "Two heads"));
            }
            if show.boxed {
                ui.checkbox(&mut self.boxed, t!("Fundo", "Background")).on_hover_text(t!("Caixa de fundo atrás do texto", "Box behind the text"));
            }
            if show.step {
                ui.label(t!("Formato", "Shape"));
                combo(ui, "step_shape", &mut self.step_shape, &StepShape::ALL);
                combo(ui, "step_label", &mut self.step_glyph, &StepGlyph::ALL);
            }
            if show.callout {
                ui.label(t!("Estilo", "Style"));
                combo(ui, "callout_style", &mut self.callout_style, &CalloutStyle::ALL);
            }
            if show.stamp {
                ui.label(t!("Símbolo", "Symbol"));
                combo(ui, "stamp", &mut self.stamp, &Stamp::ALL);
            }
            ui.separator();
            match selection {
                Some(i) => {
                    if let Op::Step { number, .. } = self.history.ops()[i] {
                        let mut n = number;
                        ui.label(t!("Nº", "No."));
                        if ui.add(egui::DragValue::new(&mut n).range(1..=999)).changed() {
                            let mut op = self.history.ops()[i].clone();
                            if let Op::Step { number, .. } = &mut op {
                                *number = n;
                            }
                            self.change(i, op);
                        }
                    }
                    if ui.button(t!("Duplicar", "Duplicate")).on_hover_text("Ctrl D").clicked() {
                        self.duplicate(i);
                    }
                    if ui.button(t!("Apagar", "Delete")).on_hover_text("Delete").clicked() {
                        self.delete(i);
                    }
                }
                None => {
                    ui.weak(tool_hint(self.tool));
                }
            }
        });
        // Color and width changed while an object is selected apply to it.
        if let Some(i) = self.selection() {
            let mut op = self.history.ops()[i].clone();
            if self.color != before.0 {
                op.set_color(self.rgba());
            }
            if self.width != before.1 {
                op.set_thickness(self.width);
            }
            if self.filled != before.2 {
                if let Op::Rect { filled, .. } | Op::Ellipse { filled, .. } = &mut op {
                    *filled = self.filled;
                }
            }
            if self.boxed != before.3 {
                if let Op::Text { boxed, .. } = &mut op {
                    *boxed = self.boxed;
                }
            }
            if self.dashed != before.4 {
                if let Op::Arrow { dashed, .. }
                | Op::Line { dashed, .. }
                | Op::Rect { dashed, .. }
                | Op::Ellipse { dashed, .. } = &mut op
                {
                    *dashed = self.dashed;
                }
            }
            if self.double != before.5 {
                if let Op::Arrow { double, .. } = &mut op {
                    *double = self.double;
                }
            }
            if self.stamp != before.6 {
                if let Op::Stamp { kind, .. } = &mut op {
                    *kind = self.stamp;
                }
            }
            if (self.step_shape, self.step_glyph, self.callout_style) != before_style {
                match &mut op {
                    Op::Step { shape, glyph, .. } => (*shape, *glyph) = (self.step_shape, self.step_glyph),
                    Op::Callout { style, .. } => *style = self.callout_style,
                    _ => {}
                }
            }
            self.change(i, op);
        }
        styles_changed
    }

    /// Draws the image exactly in `rect`, without zoom or offset. Used in quick annotation, where
    /// the capture stays in the same place on the screen it came from.
    pub fn canvas_at(&mut self, ui: &mut egui::Ui, rect: Rect) {
        self.fixed_rect = Some(rect);
        self.canvas_ui(ui);
        self.fixed_rect = None;
    }

    /// A text is being typed over the image.
    pub fn is_typing(&self) -> bool {
        self.pending_text.is_some()
    }

    /// An annotation is selected with the Select tool.
    pub fn has_selection(&self) -> bool {
        self.selection().is_some()
    }

    /// Compact quick annotation bar: the most used tools, colors and width.
    pub fn quick_toolbar_ui(&mut self, ui: &mut egui::Ui) {
        let before = (self.color, self.width);
        for tool in QUICK_TOOLS {
            let clicked = ui.selectable_label(self.tool == tool, tool_label(tool)).on_hover_text(tool_hint(tool)).clicked();
            if clicked {
                self.tool = tool;
                self.pending_text = None;
                self.selected = None;
                self.drag = None;
            }
        }
        ui.separator();
        for c in PALETTE {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
            ui.painter().rect_filled(rect, 3.0, c);
            if self.color == c {
                ui.painter().rect_stroke(rect, 3.0, Stroke::new(2.0, ui.visuals().strong_text_color()), StrokeKind::Outside);
            }
            if resp.clicked() {
                self.color = c;
            }
        }
        ui.spacing_mut().slider_width = 70.0;
        ui.add(egui::Slider::new(&mut self.width, 1.0..=24.0).show_value(false)).on_hover_text(t!("Espessura ou tamanho", "Thickness or size"));
        if ui.add_enabled(self.history.can_undo(), egui::Button::new("↶")).on_hover_text(t!("Desfazer (Ctrl Z)", "Undo (Ctrl Z)")).clicked() {
            self.undo();
        }
        // Color and width changed while an object is selected apply to it.
        if let Some(i) = self.selection() {
            let mut op = self.history.ops()[i].clone();
            if self.color != before.0 {
                op.set_color(self.rgba());
            }
            if self.width != before.1 {
                op.set_thickness(self.width);
            }
            self.change(i, op);
        }
    }

    pub fn canvas_ui(&mut self, ui: &mut egui::Ui) {
        self.canvas_inner(ui);
        // With the mouse released the gesture is over: the next change creates another Undo step.
        if !ui.ctx().input(|i| i.pointer.any_down()) {
            self.gesture_open = false;
        }
    }

    fn canvas_inner(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // Shortcuts only apply when no text field is being edited.
        if self.pending_text.is_none() && !ctx.egui_wants_keyboard_input() {
            let (undo, redo) = ctx.input_mut(|i| {
                let redo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)
                    || i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z);
                (i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z), redo)
            });
            let zoom_keys = self.fixed_rect.is_none();
            let (fit, actual, zoom_in, zoom_out) = ctx.input_mut(|i| {
                if !zoom_keys {
                    return (false, false, false, false);
                }
                let cmd = egui::Modifiers::COMMAND;
                (
                    i.consume_key(cmd, egui::Key::Num0),
                    i.consume_key(cmd, egui::Key::Num1),
                    i.consume_key(cmd, egui::Key::Plus) || i.consume_key(cmd, egui::Key::Equals),
                    i.consume_key(cmd, egui::Key::Minus),
                )
            });
            if fit {
                self.zoom_to_fit();
            } else if actual {
                self.set_zoom(1.0);
            } else if zoom_in {
                self.set_zoom(self.view_scale * ZOOM_STEP);
            } else if zoom_out {
                self.set_zoom(self.view_scale / ZOOM_STEP);
            }
            if undo {
                self.undo();
            }
            if redo {
                self.redo();
            }
        }

        let img = self.history.image();
        let img_size = Vec2::new(img.width() as f32, img.height() as f32);
        let (avail, scale, img_rect) = match self.fixed_rect {
            // Quick annotation: the image fills exactly the given rectangle.
            Some(r) => (r, r.width() / img_size.x.max(1.0), r),
            None => self.layout_view(ui, &ctx, img_size),
        };
        self.view_scale = scale;
        let response = ui.allocate_rect(img_rect.intersect(avail), Sense::click_and_drag());
        let painter = ui.painter_at(avail.expand(8.0));
        self.canvas_body(&ctx, response, painter, img_rect, img_size, scale);
    }

    /// Position and scale of the image in the regular editor, with zoom and offset.
    fn layout_view(&mut self, ui: &egui::Ui, ctx: &egui::Context, img_size: Vec2) -> (Rect, f32, Rect) {
        let avail = ui.available_rect_before_wrap().shrink(8.0);
        let fit = (avail.width() / img_size.x).min(avail.height() / img_size.y).min(1.0).max(0.01);
        let mut scale = self.zoom.unwrap_or(fit);
        // Ctrl + wheel zooms at the point under the cursor; the wheel alone or the middle button
        // pans.
        if let Some(cursor) = ctx.input(|i| i.pointer.hover_pos()).filter(|p| avail.contains(*p)) {
            let (factor, scroll, middle) = ctx.input(|i| {
                let middle = if i.pointer.middle_down() { i.pointer.delta() } else { Vec2::ZERO };
                (i.zoom_delta(), i.smooth_scroll_delta, middle)
            });
            if factor != 1.0 {
                let new = (scale * factor).clamp(ZOOM_MIN, ZOOM_MAX);
                let center = avail.center() + self.pan;
                self.pan = cursor + (center - cursor) * (new / scale) - avail.center();
                scale = new;
                self.zoom = Some(new);
            }
            self.pan += scroll + middle;
        }
        // The image only pans where it doesn't fit the area; what fits stays centered.
        let slack = ((img_size * scale - avail.size()) / 2.0).max(Vec2::ZERO);
        self.pan = self.pan.clamp(-slack, slack);
        let img_rect = Rect::from_center_size(avail.center() + self.pan, img_size * scale);
        (avail, scale, img_rect)
    }

    /// Draws the image and handles the active tool.
    fn canvas_body(
        &mut self,
        ctx: &egui::Context,
        response: egui::Response,
        painter: egui::Painter,
        img_rect: Rect,
        img_size: Vec2,
        scale: f32,
    ) {
        let ctx = ctx.clone();

        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(self.texture.id(), img_rect, uv, Color32::WHITE);

        let to_img = |p: Pos2| -> Point {
            let v = (p - img_rect.min) / scale;
            (v.x.clamp(0.0, img_size.x), v.y.clamp(0.0, img_size.y))
        };
        let to_screen = |p: Point| img_rect.min + Vec2::new(p.0, p.1) * scale;

        if self.tool == Tool::Select {
            // Not clamped to the image: an object can be dragged partly outside it.
            let free = |p: Pos2| -> Point {
                let v = (p - img_rect.min) / scale;
                (v.x, v.y)
            };
            self.select_ui(&ctx, &response, &painter, free, to_screen, scale);
            self.pending_text_ui(&ctx, to_screen, scale);
            return;
        }

        if response.hovered() {
            ctx.set_cursor_icon(if self.tool == Tool::Text {
                egui::CursorIcon::Text
            } else {
                egui::CursorIcon::Crosshair
            });
        }

        if self.tool == Tool::Stamp {
            let radius = self.step_radius();
            if let Some(p) = response.hover_pos() {
                painter.circle_stroke(p, radius * scale, Stroke::new(2.0, self.color));
            }
            if response.clicked() {
                if let Some(p) = response.interact_pointer_pos() {
                    let color = self.rgba();
                    self.commit(Op::Stamp { kind: self.stamp, center: to_img(p), radius, color });
                }
            }
            return;
        }

        if self.tool == Tool::Step {
            let (number, radius) = (self.history.next_step(), self.step_radius());
            // Preview under the cursor, already showing the number that will be used.
            if let Some(p) = response.hover_pos() {
                painter.circle_filled(p, radius * scale, self.color.gamma_multiply(0.55));
                let [r, g, b, _] = render::step_text_color(self.rgba());
                painter.text(
                    p,
                    egui::Align2::CENTER_CENTER,
                    render::step_label(number, self.step_glyph),
                    egui::FontId::proportional(render::step_font_size(number, radius) * scale),
                    Color32::from_rgb(r, g, b),
                );
            }
            if response.clicked() {
                if let Some(p) = response.interact_pointer_pos() {
                    let color = self.rgba();
                    let (shape, glyph) = (self.step_shape, self.step_glyph);
                    self.commit(Op::Step { center: to_img(p), number, color, radius, shape, glyph });
                }
            }
            return;
        }

        if self.tool == Tool::Text {
            if response.clicked() {
                self.commit_pending_text();
                if let Some(p) = response.interact_pointer_pos() {
                    self.pending_text = Some(PendingText { pos: to_img(p), text: String::new(), focused: false, edit: None, callout_tip: None });
                }
            }
            self.pending_text_ui(&ctx, to_screen, scale);
            return;
        }

        let pointer = response.interact_pointer_pos().map(to_img);
        if response.drag_started() {
            self.drag_start = pointer;
            self.pen_points.clear();
        }
        if let (Some(start), Some(cur)) = (self.drag_start, pointer) {
            if matches!(self.tool, Tool::Pen | Tool::Erase) && self.pen_points.last() != Some(&cur) {
                self.pen_points.push(cur);
            }
            self.preview(&painter, start, cur, to_screen, scale, img_rect);
            if response.drag_stopped() {
                if self.tool == Tool::GrabText {
                    self.text_request = Some((start, cur));
                } else if self.tool == Tool::Callout {
                    if render::distance(start, cur) >= 3.0 {
                        self.pending_text =
                            Some(PendingText { pos: cur, text: String::new(), focused: false, edit: None, callout_tip: Some(start) });
                    }
                } else if let Some(op) = self.build_op(start, cur) {
                    self.commit(op);
                    // The magnifier comes out already selected, so the copy can be taken to the
                    // place and size you want.
                    if self.tool == Tool::Magnify {
                        self.tool = Tool::Select;
                        self.select(self.history.ops().len() - 1);
                    }
                }
            }
        }
        if !response.dragged() {
            self.drag_start = None;
        }
        if self.tool == Tool::Callout {
            self.pending_text_ui(&ctx, to_screen, scale);
        }
    }

    /// Selected object, if the index is still valid.
    fn selection(&self) -> Option<usize> {
        self.selected.filter(|&i| i >= self.history.first_object() && i < self.history.ops().len())
    }

    /// Selects the object and shows its color and width in the bar.
    fn select(&mut self, index: usize) {
        self.selected = Some(index);
        let Some(op) = self.history.ops().get(index) else { return };
        if let Some([r, g, b, a]) = op.color() {
            self.color = Color32::from_rgba_unmultiplied(r, g, b, a);
        }
        if let Some(t) = op.thickness() {
            self.width = t.clamp(1.0, 24.0);
        }
        match op {
            Op::Rect { filled, dashed, .. } | Op::Ellipse { filled, dashed, .. } => {
                self.filled = *filled;
                self.dashed = *dashed;
            }
            Op::Arrow { dashed, double, .. } => {
                self.dashed = *dashed;
                self.double = *double;
            }
            Op::Line { dashed, .. } => self.dashed = *dashed,
            Op::Stamp { kind, .. } => self.stamp = *kind,
            Op::Step { shape, glyph, .. } => (self.step_shape, self.step_glyph) = (*shape, *glyph),
            Op::Callout { style, .. } => self.callout_style = *style,
            Op::Text { boxed, .. } => self.boxed = *boxed,
            _ => {}
        }
    }

    /// Topmost object under the point `p`. If nothing was hit, the surrounding rectangle or ellipse
    /// counts.
    fn pick(&self, p: Point, tol: f32) -> Option<usize> {
        let (ops, font) = (self.history.ops(), self.history.font());
        let candidates = || (self.history.first_object()..ops.len()).rev();
        candidates().find(|&i| ops[i].hit(p, tol, font)).or_else(|| candidates().find(|&i| ops[i].encloses(p)))
    }

    /// Changes an object. The changes from one gesture (a drag, a slide of the control) become a
    /// single Undo step.
    fn change(&mut self, index: usize, op: Op) {
        if self.history.ops().get(index) == Some(&op) {
            return;
        }
        if !self.gesture_open {
            self.history.checkpoint();
            self.gesture_open = true;
        }
        self.history.set(index, op);
        self.refresh_texture();
    }

    fn delete(&mut self, index: usize) {
        self.history.remove(index);
        self.selected = None;
        self.drag = None;
        self.refresh_texture();
    }

    fn duplicate(&mut self, index: usize) {
        let Some(mut op) = self.history.ops().get(index).cloned() else { return };
        op.translate(16.0, 16.0);
        if let Op::Step { number, .. } = &mut op {
            *number = self.history.next_step();
        }
        self.commit(op);
        self.select(self.history.ops().len() - 1);
    }

    /// Select tool: clicking picks an object, dragging moves it, the small squares resize it,
    /// double-clicking a text rewrites it.
    fn select_ui(
        &mut self,
        ctx: &egui::Context,
        response: &egui::Response,
        painter: &egui::Painter,
        to_img: impl Fn(Pos2) -> Point,
        to_screen: impl Fn(Point) -> Pos2,
        scale: f32,
    ) {
        let font = self.history.font().clone();
        let tol = 6.0 / scale;
        let grip = 9.0 / scale;
        if self.selection().is_none() {
            self.selected = None;
        }
        let handle_at = |ed: &Self, p: Point| {
            let i = ed.selection()?;
            let (h, _) = ed.history.ops()[i].handles(&font).into_iter().find(|(_, hp)| render::distance(*hp, p) <= grip)?;
            Some((i, h))
        };

        if response.drag_started() {
            // A drag is only recognized after the mouse moves a little; what counts is where it was
            // pressed.
            let origin = ctx.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos()).map(&to_img);
            let grabbed = origin.and_then(|o| {
                let (index, handle) = match handle_at(self, o) {
                    Some((i, h)) => (i, Some(h)),
                    None => (self.pick(o, tol)?, None),
                };
                let orig = self.history.ops()[index].clone();
                Some(ObjDrag { index, handle, start: o, current: orig.clone(), orig })
            });
            match grabbed.as_ref().map(|d| d.index) {
                Some(i) => self.select(i),
                None => self.selected = None,
            }
            self.drag = grabbed;
        }

        if let Some(mut d) = self.drag.take() {
            if let Some(cur) = response.interact_pointer_pos().map(&to_img) {
                d.current = match d.handle {
                    Some(h) => d.orig.with_handle(h, cur, &font),
                    None => {
                        let mut op = d.orig.clone();
                        op.translate(cur.0 - d.start.0, cur.1 - d.start.1);
                        op
                    }
                };
            }
            let stop = response.drag_stopped();
            let img = self.history.image();
            let live = img.width() as u64 * img.height() as u64 <= LIVE_PIXELS;
            if live || stop {
                self.change(d.index, d.current.clone());
            }
            if response.dragged() && !stop {
                self.drag = Some(d);
            }
        }

        let click_pos = response.interact_pointer_pos().map(&to_img);
        if response.double_clicked() {
            if let Some(i) = click_pos.and_then(|p| self.pick(p, tol)) {
                let editable = match &self.history.ops()[i] {
                    Op::Text { pos, text, .. } => Some((*pos, text.clone(), None)),
                    Op::Callout { tip, pos, text, size, .. } => {
                        let pad = render::callout_pad(*size);
                        Some(((pos.0 + pad, pos.1 + pad), text.clone(), Some(*tip)))
                    }
                    _ => None,
                };
                if let Some((pos, text, callout_tip)) = editable {
                    self.pending_text = Some(PendingText { pos, text, focused: false, edit: Some(i), callout_tip });
                }
            }
        } else if response.clicked() {
            match click_pos.and_then(|p| self.pick(p, tol)) {
                Some(i) => self.select(i),
                None => self.selected = None,
            }
        }

        if self.pending_text.is_none() && !ctx.egui_wants_keyboard_input() {
            if let Some(i) = self.selection() {
                let (delete, escape, duplicate, dx, dy) = ctx.input_mut(|inp| {
                    let step = if inp.modifiers.shift { 10.0 } else { 1.0 };
                    let axis = |neg: egui::Key, pos: egui::Key| {
                        step * (inp.key_pressed(pos) as i32 - inp.key_pressed(neg) as i32) as f32
                    };
                    let (dx, dy) = (
                        axis(egui::Key::ArrowLeft, egui::Key::ArrowRight),
                        axis(egui::Key::ArrowUp, egui::Key::ArrowDown),
                    );
                    (
                        inp.key_pressed(egui::Key::Delete) || inp.key_pressed(egui::Key::Backspace),
                        inp.key_pressed(egui::Key::Escape),
                        inp.consume_key(egui::Modifiers::COMMAND, egui::Key::D),
                        dx,
                        dy,
                    )
                });
                if delete {
                    self.delete(i);
                } else if escape {
                    self.selected = None;
                } else if duplicate {
                    self.duplicate(i);
                } else if dx != 0.0 || dy != 0.0 {
                    let mut op = self.history.ops()[i].clone();
                    op.translate(dx, dy);
                    self.change(i, op);
                }
            }
        }

        if let Some(p) = response.hover_pos().map(&to_img) {
            ctx.set_cursor_icon(if handle_at(self, p).is_some() {
                egui::CursorIcon::Crosshair
            } else if self.drag.is_some() || self.pick(p, tol).is_some() {
                egui::CursorIcon::Move
            } else {
                egui::CursorIcon::Default
            });
        }

        // Outline and control points of the selected object.
        let shown = match &self.drag {
            Some(d) => Some(d.current.clone()),
            None => self.selection().map(|i| self.history.ops()[i].clone()),
        };
        if let Some(op) = shown {
            if !matches!(op, Op::Arrow { .. } | Op::Line { .. }) {
                if let Some((min, max)) = op.bounds(&font) {
                    let frame = Rect::from_min_max(to_screen(min), to_screen(max)).expand(3.0);
                    painter.rect_stroke(frame, 2.0, Stroke::new(1.5, self.accent), StrokeKind::Middle);
                }
            }
            for (_, hp) in op.handles(&font) {
                let knob = Rect::from_center_size(to_screen(hp), Vec2::splat(9.0));
                painter.rect_filled(knob, 2.0, Color32::WHITE);
                painter.rect_stroke(knob, 2.0, Stroke::new(1.5, self.accent), StrokeKind::Middle);
            }
        }
    }

    /// Builds the final operation of the drag; ignores gestures that are too small.
    fn build_op(&mut self, a: Point, b: Point) -> Option<Op> {
        let (color, width) = (self.rgba(), self.width);
        let dist = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let area_ok = (b.0 - a.0).abs() >= 3.0 && (b.1 - a.1).abs() >= 3.0;
        match self.tool {
            Tool::Arrow => {
                (dist >= 3.0).then_some(Op::Arrow { from: a, to: b, color, width, dashed: self.dashed, double: self.double })
            }
            Tool::Line => (dist >= 3.0).then_some(Op::Line { from: a, to: b, color, width, dashed: self.dashed }),
            Tool::Rect => area_ok.then_some(Op::Rect { a, b, color, width, filled: self.filled, dashed: self.dashed }),
            Tool::Ellipse => area_ok.then_some(Op::Ellipse { a, b, color, width, filled: self.filled, dashed: self.dashed }),
            Tool::Pen => {
                let points = std::mem::take(&mut self.pen_points);
                (points.len() >= 2).then_some(Op::Pen { points, color, width })
            }
            Tool::Erase => {
                let points = std::mem::take(&mut self.pen_points);
                (points.len() >= 2).then_some(Op::Erase { points, width })
            }
            Tool::Highlight => area_ok.then_some(Op::Highlight { a, b, color: HIGHLIGHT_COLOR }),
            Tool::Pixelate => area_ok.then_some(Op::Pixelate { a, b }),
            Tool::Blur => area_ok.then_some(Op::Blur { a, b }),
            Tool::Spotlight => area_ok.then_some(Op::Spotlight { a, b }),
            Tool::Magnify => area_ok.then(|| {
                let img = self.history.image();
                let size = (img.width() as f32, img.height() as f32);
                let (da, db) = render::magnify_dest(a, b, render::magnify_zoom_for(width), size);
                Op::Magnify { src: (a, b), a: da, b: db }
            }),
            Tool::Crop => area_ok.then_some(Op::Crop { a, b }),
            Tool::CutOut => {
                // The direction of the drag says which strip goes: downward, rows; sideways,
                // columns.
                let (dx, dy) = ((b.0 - a.0).abs(), (b.1 - a.1).abs());
                (dx.max(dy) >= 3.0).then_some(Op::CutOut { from: a, to: b, rows: dy >= dx })
            }
            Tool::Text | Tool::GrabText | Tool::Step | Tool::Select | Tool::Callout | Tool::Stamp => None,
        }
    }

    /// Draws the shape in progress over the image, at screen scale.
    fn preview(
        &self,
        painter: &egui::Painter,
        a: Point,
        b: Point,
        to_screen: impl Fn(Point) -> Pos2,
        scale: f32,
        img_rect: Rect,
    ) {
        let (sa, sb) = (to_screen(a), to_screen(b));
        let stroke = Stroke::new((self.width * scale).max(1.0), self.color);
        let rect = Rect::from_two_pos(sa, sb);
        match self.tool {
            Tool::Arrow => {
                let dir = sb - sa;
                let len = dir.length();
                if len > 1.0 {
                    let u = dir / len;
                    let head = (render::arrow_head_len(self.width) * scale).min(len);
                    let base = sb - u * head;
                    let n = Vec2::new(-u.y, u.x) * head * 0.45;
                    painter.line_segment([sa, base], stroke);
                    painter.add(egui::Shape::convex_polygon(
                        vec![sb, base + n, base - n],
                        self.color,
                        Stroke::NONE,
                    ));
                }
            }
            Tool::Line => {
                painter.line_segment([sa, sb], stroke);
            }
            Tool::Rect if self.filled => {
                painter.rect_filled(rect, 0.0, self.color);
            }
            Tool::Rect => {
                painter.rect_stroke(rect, 0.0, stroke, StrokeKind::Middle);
            }
            Tool::Ellipse if self.filled => {
                painter.add(egui::Shape::ellipse_filled(rect.center(), rect.size() / 2.0, self.color));
            }
            Tool::Ellipse => {
                painter.add(egui::Shape::ellipse_stroke(rect.center(), rect.size() / 2.0, stroke));
            }
            Tool::Magnify => {
                // The marked region and, next to it, where the enlarged copy will appear.
                let size = img_rect.size() / scale;
                let (min, max) = render::magnify_dest(a, b, render::magnify_zoom_for(self.width), (size.x, size.y));
                painter.rect_stroke(rect, 0.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Middle);
                let dest = Rect::from_min_max(to_screen(min), to_screen(max));
                painter.rect_filled(dest, 0.0, Color32::from_rgba_unmultiplied(self.accent.r(), self.accent.g(), self.accent.b(), 40));
                painter.rect_stroke(dest, 0.0, Stroke::new(2.0, self.accent), StrokeKind::Middle);
            }
            Tool::Callout => {
                // Line from the target to the point where the callout box will go.
                painter.line_segment([sa, sb], Stroke::new(2.0, self.color));
                painter.rect_filled(Rect::from_center_size(sb, Vec2::new(96.0, 36.0)), 8.0, self.color);
            }
            Tool::Pen => {
                let pts: Vec<Pos2> = self.pen_points.iter().map(|p| to_screen(*p)).collect();
                painter.add(egui::Shape::line(pts, stroke));
            }
            Tool::Erase => {
                let pts: Vec<Pos2> = self.pen_points.iter().map(|p| to_screen(*p)).collect();
                let trail = Stroke::new((self.width * scale).max(1.0), Color32::from_white_alpha(150));
                painter.add(egui::Shape::line(pts, trail));
            }
            Tool::Highlight => {
                painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(255, 235, 60, 110));
            }
            Tool::Pixelate | Tool::Blur => {
                painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(128, 128, 128, 140));
                painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Middle);
            }
            Tool::CutOut => {
                // The strip that will be removed, from one edge of the image to the other.
                let band = if rect.height() >= rect.width() {
                    Rect::from_min_max(Pos2::new(img_rect.min.x, rect.min.y), Pos2::new(img_rect.max.x, rect.max.y))
                } else {
                    Rect::from_min_max(Pos2::new(rect.min.x, img_rect.min.y), Pos2::new(rect.max.x, img_rect.max.y))
                };
                painter.rect_filled(band, 0.0, Color32::from_rgba_unmultiplied(235, 64, 52, 90));
                painter.rect_stroke(band, 0.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Middle);
            }
            Tool::Crop | Tool::Spotlight => {
                let dim = Color32::from_black_alpha(140);
                for r in [
                    Rect::from_min_max(img_rect.min, Pos2::new(img_rect.max.x, rect.min.y)),
                    Rect::from_min_max(Pos2::new(img_rect.min.x, rect.max.y), img_rect.max),
                    Rect::from_min_max(Pos2::new(img_rect.min.x, rect.min.y), Pos2::new(rect.min.x, rect.max.y)),
                    Rect::from_min_max(Pos2::new(rect.max.x, rect.min.y), Pos2::new(img_rect.max.x, rect.max.y)),
                ] {
                    painter.rect_filled(r, 0.0, dim);
                }
                painter.rect_stroke(rect, 0.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Middle);
            }
            Tool::GrabText => {
                painter.rect_filled(rect, 0.0, self.accent.gamma_multiply(0.16));
                painter.rect_stroke(rect, 0.0, Stroke::new(1.5, self.accent), StrokeKind::Middle);
            }
            Tool::Text | Tool::Step | Tool::Select | Tool::Stamp => {}
        }
    }

    fn commit_pending_text(&mut self) {
        let Some(p) = self.pending_text.take() else { return };
        match p.edit {
            Some(i) => {
                let Some(mut op) = self.history.ops().get(i).cloned() else { return };
                if p.text.trim().is_empty() {
                    self.delete(i);
                } else if let Op::Text { text, .. } | Op::Callout { text, .. } = &mut op {
                    *text = p.text;
                    self.change(i, op);
                }
            }
            None if p.text.trim().is_empty() => {}
            None => {
                let (color, size) = (self.rgba(), self.text_size());
                let op = match p.callout_tip {
                    Some(tip) => {
                        // The callout is centered where the drag ended.
                        let (_, (w, h)) = render::callout_box((0.0, 0.0), &p.text, size, self.history.font());
                        Op::Callout { tip, pos: (p.pos.0 - w / 2.0, p.pos.1 - h / 2.0), text: p.text, color, size, style: self.callout_style }
                    }
                    None => Op::Text { pos: p.pos, text: p.text, color, size, boxed: self.boxed },
                };
                self.commit(op);
            }
        }
    }

    /// Floating text field over the image. Enter confirms, Shift+Enter starts a new line, Esc
    /// cancels.
    fn pending_text_ui(&mut self, ctx: &egui::Context, to_screen: impl Fn(Point) -> Pos2, scale: f32) {
        let Some(p) = &self.pending_text else { return };
        // A text being rewritten keeps the size and color it already had. In a callout the color is
        // the box's, so the field uses the interface's normal text color.
        let (size, color) = match p.edit.and_then(|i| self.history.ops().get(i)) {
            Some(Op::Text { size, color: [r, g, b, a], .. }) => {
                (*size, Some(Color32::from_rgba_unmultiplied(*r, *g, *b, *a)))
            }
            Some(Op::Callout { size, .. }) => (*size, None),
            _ if p.callout_tip.is_some() => (self.text_size(), None),
            _ => (self.text_size(), Some(self.color)),
        };
        let pivot = if p.callout_tip.is_some() && p.edit.is_none() {
            egui::Align2::CENTER_CENTER
        } else {
            egui::Align2::LEFT_TOP
        };
        let font_px = (size * scale).max(8.0);
        let enter = ctx.input_mut(|i| !i.modifiers.shift && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let Some(pending) = &mut self.pending_text else { return };
        let mut done = false;
        let mut cancel = false;
        egui::Area::new(egui::Id::unique("pending_text"))
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(to_screen(pending.pos))
            .show(ctx, |ui| {
                let mut edit = egui::TextEdit::multiline(&mut pending.text)
                    .font(egui::FontId::proportional(font_px))
                    .desired_rows(1)
                    .desired_width(font_px * 10.0)
                    .hint_text(t!("Digite e Enter", "Type, then Enter"));
                if let Some(c) = color {
                    edit = edit.text_color(c);
                }
                let resp = ui.add(edit);
                if !pending.focused {
                    resp.request_focus();
                    pending.focused = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                } else if enter || resp.lost_focus() {
                    done = true;
                }
            });
        if cancel {
            self.pending_text = None;
        } else if done {
            self.commit_pending_text();
        }
    }
}

/// Side that keeps the aspect ratio: `side` is to `from` as the result is to `other`.
fn scaled_side(side: u32, from: u32, other: u32) -> u32 {
    ((side as f64 * other as f64 / from.max(1) as f64).round() as u32).max(1)
}

/// Drop-down list to pick one of the options in `all`. Each value in `all` comes with its name in
/// Portuguese and in English.
fn combo<T: Copy + PartialEq>(ui: &mut egui::Ui, id: &str, current: &mut T, all: &[(T, &'static str, &'static str)]) {
    let label = all.iter().find(|(value, _, _)| value == current).map_or("", |(_, pt, en)| pick(pt, en));
    egui::ComboBox::from_id_salt(id).selected_text(label).show_ui(ui, |ui| {
        for (value, pt, en) in all {
            ui.selectable_value(current, *value, pick(pt, en));
        }
    });
}

fn to_color_image(img: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

/// The image as a texture for the screen. Graphics cards accept textures only up to a size per
/// side (16 384 or 32 768 px, depending on the card), and a larger one stops the program, which
/// a long scrolling capture can reach. A larger image is shown reduced; the image itself, and
/// everything copied or saved from it, keeps its full size.
pub fn texture_image(img: &RgbaImage, max_side: usize) -> egui::ColorImage {
    let max = max_side.clamp(1, u32::MAX as usize) as u32;
    let (w, h) = img.dimensions();
    if w <= max && h <= max {
        return to_color_image(img);
    }
    let scale = max as f64 / w.max(h) as f64;
    let fit = |side: u32| ((side as f64 * scale).floor() as u32).clamp(1, max);
    to_color_image(&image::imageops::resize(img, fit(w), fit(h), image::imageops::FilterType::Triangle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textures_never_exceed_the_cards_limit() {
        let tall = RgbaImage::new(400, 33_000);
        let tex = texture_image(&tall, 16_384);
        assert!(tex.size[0] <= 16_384 && tex.size[1] <= 16_384, "{:?}", tex.size);
        assert_eq!(tex.size[1], 16_384, "the long side fills the limit");
        assert_eq!(tex.size[0], 198, "the proportions are kept");
        let small = RgbaImage::new(800, 600);
        assert_eq!(texture_image(&small, 16_384).size, [800, 600], "small images are untouched");
    }

    const RED: Rgba = [255, 0, 0, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];

    fn history() -> History {
        let img = RgbaImage::from_pixel(100, 100, image::Rgba(WHITE));
        History::new(img, default_font())
    }

    fn line() -> Op {
        Op::Line { from: (10.0, 50.0), to: (90.0, 50.0), color: RED, width: 6.0, dashed: false }
    }

    #[test]
    fn undo_and_redo_restore_pixels() {
        let mut h = history();
        h.push(line());
        assert_eq!(h.image().get_pixel(50, 50).0, RED);
        assert!(h.undo());
        assert_eq!(h.image().get_pixel(50, 50).0, WHITE);
        assert!(h.redo());
        assert_eq!(h.image().get_pixel(50, 50).0, RED);
    }

    #[test]
    fn rebase_swaps_the_photo_and_shifts_every_object() {
        let mut h = history();
        h.push(line());
        h.push(Op::Pixelate { a: (0.0, 0.0), b: (20.0, 20.0) });
        let gray = [90, 90, 90, 255];
        h.rebase(RgbaImage::from_pixel(120, 120, image::Rgba(gray)), 0.0, 10.0);
        assert_eq!((h.image().width(), h.image().height()), (120, 120), "a foto nova vale");
        assert_eq!(h.image().get_pixel(50, 60).0, RED, "the line moved down 10 pixels");
        assert_eq!(h.image().get_pixel(50, 50).0, gray);
        assert!(h.undo(), "the undo is still there");
        assert!(h.undo());
        assert!(h.redo());
        assert_eq!(h.image().get_pixel(50, 60).0, RED, "what was stored also moved");
    }

    #[test]
    fn undo_restores_size_after_crop() {
        let mut h = history();
        h.push(Op::Crop { a: (0.0, 0.0), b: (40.0, 30.0) });
        assert_eq!((h.image().width(), h.image().height()), (40, 30));
        h.undo();
        assert_eq!((h.image().width(), h.image().height()), (100, 100));
    }

    #[test]
    fn new_edit_discards_redo() {
        let mut h = history();
        h.push(line());
        h.undo();
        h.push(Op::Pixelate { a: (0.0, 0.0), b: (20.0, 20.0) });
        assert!(!h.can_redo());
        assert!(!h.redo());
    }

    #[test]
    fn step_numbers_follow_the_markers_on_the_image() {
        let mut h = history();
        let step = |n| Op::Step { center: (20.0 * n as f32, 20.0), number: n, color: RED, radius: 8.0, shape: StepShape::Circle, glyph: StepGlyph::Number };
        assert_eq!(h.next_step(), 1);
        h.push(step(1));
        h.push(line()); // other annotations don't count
        h.push(step(2));
        assert_eq!(h.next_step(), 3);
        h.undo();
        assert_eq!(h.next_step(), 2, "undo gives the number back");
        h.redo();
        assert_eq!(h.next_step(), 3);
    }

    #[test]
    fn moving_an_object_is_one_undo_step() {
        let mut h = history();
        h.push(line());
        // One drag: one undo point, several intermediate positions.
        h.checkpoint();
        for dy in [5.0, 12.0, 30.0] {
            let mut op = line();
            op.translate(0.0, dy);
            h.set(0, op);
        }
        assert_eq!(h.image().get_pixel(50, 80).0, RED);
        assert_eq!(h.image().get_pixel(50, 50).0, WHITE, "the old position is gone");
        assert!(h.undo());
        assert_eq!(h.image().get_pixel(50, 50).0, RED, "goes back to where it was before the drag");
        assert!(h.redo());
        assert_eq!(h.image().get_pixel(50, 80).0, RED);
    }

    #[test]
    fn removing_an_object_can_be_undone() {
        let mut h = history();
        h.push(line());
        h.push(Op::Pixelate { a: (0.0, 0.0), b: (20.0, 20.0) });
        h.remove(0);
        assert_eq!(h.ops().len(), 1);
        assert_eq!(h.image().get_pixel(50, 50).0, WHITE);
        h.undo();
        assert_eq!(h.ops().len(), 2);
        assert_eq!(h.image().get_pixel(50, 50).0, RED);
    }

    #[test]
    fn objects_before_a_crop_are_not_selectable() {
        let mut h = history();
        h.push(line());
        assert_eq!(h.first_object(), 0);
        h.push(Op::Crop { a: (0.0, 0.0), b: (60.0, 60.0) });
        assert_eq!(h.first_object(), 2);
        h.push(line());
        assert_eq!(h.first_object(), 2);
    }

    #[test]
    fn next_step_skips_past_the_highest_number() {
        let mut h = history();
        let step = |n| Op::Step { center: (20.0, 20.0), number: n, color: RED, radius: 8.0, shape: StepShape::Circle, glyph: StepGlyph::Number };
        h.push(step(1));
        h.push(step(2));
        h.push(step(3));
        h.remove(1);
        assert_eq!(h.next_step(), 4, "deleting 2 doesn't make the next one repeat 3");
    }

    #[test]
    fn every_tool_is_in_exactly_one_group_and_has_a_name() {
        let grouped: Vec<Tool> = TOOL_GROUPS.iter().flat_map(|(_, _, tools)| tools.iter().copied()).collect();
        assert_eq!(grouped.len(), TOOLS.len());
        for (tool, label, en) in TOOLS {
            assert_eq!(grouped.iter().filter(|t| **t == tool).count(), 1, "{label}");
            assert!(!tool_label(tool).is_empty() && !tool_hint(tool).is_empty());
            assert!(!en.is_empty(), "{label} has no English name");
        }
        crate::i18n::set(crate::i18n::Lang::English);
        assert_eq!(tool_label(Tool::Highlight), "Highlighter");
        assert_eq!(tool_hint(Tool::Crop), "Drag over the part of the image to keep");
        crate::i18n::set(crate::i18n::Lang::Portuguese);
    }

    #[test]
    fn options_bar_only_offers_what_applies() {
        let arrow = Relevant::for_tool(Tool::Arrow);
        assert!(arrow.color && arrow.dashed && arrow.double && !arrow.filled && !arrow.boxed);
        assert_eq!(arrow.thickness, Some("Espessura"));
        assert_eq!(Relevant::for_tool(Tool::Pixelate), Relevant::default());
        assert_eq!(Relevant::for_tool(Tool::Magnify).thickness, Some("Ampliação"));
        assert!(Relevant::for_tool(Tool::Text).boxed && !Relevant::for_tool(Tool::Callout).boxed);
        // A selected object shows the options of the tool that created it.
        assert_eq!(Relevant::for_op(&line()), Relevant::for_tool(Tool::Line));
        let highlight = Op::Highlight { a: (0.0, 0.0), b: (9.0, 9.0), color: RED };
        assert!(Relevant::for_op(&highlight).color && !Relevant::for_tool(Tool::Highlight).color);
    }

    #[test]
    fn eraser_stroke_can_be_undone() {
        let mut h = history();
        h.push(line());
        h.push(Op::Erase { points: vec![(50.0, 30.0), (50.0, 70.0)], width: 12.0 });
        assert_eq!(h.image().get_pixel(50, 50).0, WHITE, "the eraser shows the original capture");
        assert_eq!(h.image().get_pixel(20, 50).0, RED);
        h.undo();
        assert_eq!(h.image().get_pixel(50, 50).0, RED);
    }

    #[test]
    fn styles_survive_being_saved_and_loaded() {
        let style = Style {
            name: "Warning arrow".into(),
            tool: Tool::Arrow,
            color: RED,
            width: 6.0,
            filled: false,
            boxed: false,
            dashed: true,
            double: true,
            stamp: Stamp::Warning,
            step_shape: StepShape::Pin,
            step_glyph: StepGlyph::Upper,
            callout_style: CalloutStyle::Outline,
        };
        let text = serde_json::to_string(&style).unwrap();
        assert_eq!(serde_json::from_str::<Style>(&text).unwrap(), style);
    }

    #[test]
    fn turning_the_image_can_be_undone_and_fixes_earlier_objects() {
        let mut h = history();
        h.push(line());
        h.push(Op::Resize { width: 50, height: 25 });
        assert_eq!((h.image().width(), h.image().height()), (50, 25));
        assert_eq!(h.first_object(), 2, "what came before stays fixed in the image");
        h.undo();
        assert_eq!((h.image().width(), h.image().height()), (100, 100));
        assert_eq!(h.first_object(), 0);
        assert_eq!(scaled_side(50, 100, 60), 30);
        assert_eq!(scaled_side(1, 1000, 3), 1, "nunca chega a zero");
    }

    #[test]
    fn undo_on_empty_history_is_noop() {
        let mut h = history();
        assert!(!h.can_undo());
        assert!(!h.undo());
    }
}
