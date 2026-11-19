//! Reusable immediate-mode UI kit for SnapKit: consistent buttons, pills, tool rail items,
//! circular color swatches, dividers, section cards, and keycap badges.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Margin, Painter, Popup, Pos2,
    Rect, Response, Sense, Stroke, StrokeKind, UiKind, UiStackInfo, Vec2,
    containers::menu::{MenuConfig, menu_style},
};

use crate::icons::{self, Icon};
use crate::theme::{self, Palette};

pub fn semibold(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).family(theme::semibold_family())
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

#[derive(Clone, Copy)]
struct ButtonSpec {
    height: f32,
    pad_x: f32,
    icon_size: f32,
    font_size: f32,
    radius: u8,
    semibold_text: bool,
    fill_rest: Color32,
    fill_hover: Color32,
    stroke_rest: Stroke,
    stroke_hover: Stroke,
    fg_rest: Color32,
    fg_hover: Color32,
    trailing_chevron: bool,
}

fn render_button_spec(
    ui: &mut egui::Ui,
    icon: Option<Icon>,
    text: &str,
    spec: ButtonSpec,
) -> Response {
    let ctx = ui.ctx().clone();
    let enabled = ui.is_enabled();
    let font = if spec.semibold_text {
        FontId::new(spec.font_size, theme::semibold_family())
    } else {
        FontId::proportional(spec.font_size)
    };
    let galley = if text.is_empty() {
        None
    } else {
        Some(ui.painter().layout_no_wrap(text.to_owned(), font, spec.fg_rest))
    };

    let mut content_w = 0.0;
    if icon.is_some() {
        content_w += spec.icon_size;
    }
    if let Some(g) = &galley {
        if icon.is_some() {
            content_w += 6.0;
        }
        content_w += g.size().x;
    }
    let chevron_size = 11.0;
    if spec.trailing_chevron {
        if content_w > 0.0 {
            content_w += 4.0;
        }
        content_w += chevron_size;
    }

    let total_w = (content_w + spec.pad_x * 2.0).max(spec.height);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(total_w, spec.height), Sense::click());

    let hover_t = if enabled {
        ctx.animate_bool_responsive(resp.id.with("hover"), resp.hovered())
    } else {
        0.0
    };
    if enabled && resp.hovered() {
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }

    let mut fill = lerp_color(spec.fill_rest, spec.fill_hover, hover_t);
    let stroke_color = lerp_color(spec.stroke_rest.color, spec.stroke_hover.color, hover_t);
    let stroke_w = spec.stroke_rest.width + (spec.stroke_hover.width - spec.stroke_rest.width) * hover_t;
    let mut fg = lerp_color(spec.fg_rest, spec.fg_hover, hover_t);

    if !enabled {
        fill = fill.gamma_multiply(0.42);
        fg = fg.gamma_multiply(0.38);
    } else if resp.is_pointer_button_down_on() {
        fill = lerp_color(fill, Color32::BLACK, 0.10);
    }

    let painter = ui.painter();
    let cr = CornerRadius::same(spec.radius);
    if fill.a() > 0 {
        painter.rect_filled(rect, cr, fill);
    }
    if stroke_w > 0.0 && stroke_color.a() > 0 {
        let sc = if enabled { stroke_color } else { stroke_color.gamma_multiply(0.4) };
        painter.rect_stroke(rect, cr, Stroke::new(stroke_w, sc), StrokeKind::Inside);
    }

    let mut x = rect.center().x - content_w * 0.5;
    let cy = rect.center().y;
    if let Some(ic) = icon {
        icons::paint(painter, &ctx, ic, Pos2::new(x + spec.icon_size * 0.5, cy), spec.icon_size, fg);
        x += spec.icon_size + if galley.is_some() { 6.0 } else { 0.0 };
    }
    if let Some(g) = galley {
        let pos = Pos2::new(x, cy - g.size().y * 0.5);
        x += g.size().x;
        painter.galley(pos, g, fg);
    }
    if spec.trailing_chevron {
        let cx = x + 4.0 + chevron_size * 0.5;
        icons::paint(painter, &ctx, Icon::ChevronDown, Pos2::new(cx, cy), chevron_size, fg.gamma_multiply(0.8));
    }

    resp
}

pub fn primary_button(ui: &mut egui::Ui, icon: Option<Icon>, text: &str) -> Response {
    let pal = theme::current_palette(ui.ctx());
    let hover_fill = if pal.dark {
        lerp_color(pal.accent, Color32::WHITE, 0.12)
    } else {
        lerp_color(pal.accent, Color32::BLACK, 0.10)
    };
    render_button_spec(
        ui,
        icon,
        text,
        ButtonSpec {
            height: 28.0,
            pad_x: 10.0,
            icon_size: 14.5,
            font_size: 12.5,
            radius: 7,
            semibold_text: true,
            fill_rest: pal.accent,
            fill_hover: hover_fill,
            stroke_rest: Stroke::NONE,
            stroke_hover: Stroke::NONE,
            fg_rest: pal.on_accent,
            fg_hover: pal.on_accent,
            trailing_chevron: false,
        },
    )
}

pub fn ghost_button(ui: &mut egui::Ui, icon: Option<Icon>, text: &str, active: bool) -> Response {
    ghost_button_sized(ui, icon, text, active, 28.0, 9.0, 14.5, 12.5, false)
}

pub fn small_ghost_button(ui: &mut egui::Ui, icon: Option<Icon>, text: &str, active: bool) -> Response {
    ghost_button_sized(ui, icon, text, active, 24.0, 7.0, 13.0, 11.5, false)
}

#[allow(clippy::too_many_arguments)]
fn ghost_button_sized(
    ui: &mut egui::Ui,
    icon: Option<Icon>,
    text: &str,
    active: bool,
    height: f32,
    pad_x: f32,
    icon_size: f32,
    font_size: f32,
    trailing_chevron: bool,
) -> Response {
    let pal = theme::current_palette(ui.ctx());
    let (fill_rest, fill_hover, stroke_rest, stroke_hover, fg_rest, fg_hover) = if active {
        (
            pal.accent.gamma_multiply(0.16),
            pal.accent.gamma_multiply(0.24),
            Stroke::new(1.0, pal.accent.gamma_multiply(0.55)),
            Stroke::new(1.0, pal.accent.gamma_multiply(0.85)),
            pal.accent,
            pal.accent,
        )
    } else {
        (
            Color32::TRANSPARENT,
            pal.bg_elevated,
            Stroke::NONE,
            Stroke::new(1.0, pal.border_subtle),
            pal.text_secondary,
            pal.text_primary,
        )
    };
    render_button_spec(
        ui,
        icon,
        text,
        ButtonSpec {
            height,
            pad_x,
            icon_size,
            font_size,
            radius: 6,
            semibold_text: active,
            fill_rest,
            fill_hover,
            stroke_rest,
            stroke_hover,
            fg_rest,
            fg_hover,
            trailing_chevron,
        },
    )
}

pub fn icon_button(ui: &mut egui::Ui, icon: Icon, active: bool, size: f32) -> Response {
    let pal = theme::current_palette(ui.ctx());
    let icon_size = (size * 0.56).clamp(13.0, 18.0);
    let (fill_rest, fill_hover, stroke_rest, stroke_hover, fg_rest, fg_hover) = if active {
        (
            pal.accent.gamma_multiply(0.18),
            pal.accent.gamma_multiply(0.26),
            Stroke::new(1.0, pal.accent.gamma_multiply(0.6)),
            Stroke::new(1.0, pal.accent),
            pal.accent,
            pal.accent,
        )
    } else {
        (
            Color32::TRANSPARENT,
            pal.bg_elevated,
            Stroke::NONE,
            Stroke::new(1.0, pal.border_subtle),
            pal.text_secondary,
            pal.text_primary,
        )
    };
    render_button_spec(
        ui,
        Some(icon),
        "",
        ButtonSpec {
            height: size,
            pad_x: 0.0,
            icon_size,
            font_size: 12.0,
            radius: 6,
            semibold_text: false,
            fill_rest,
            fill_hover,
            stroke_rest,
            stroke_hover,
            fg_rest,
            fg_hover,
            trailing_chevron: false,
        },
    )
}

pub fn pill_toggle(ui: &mut egui::Ui, icon: Option<Icon>, text: &str, selected: bool) -> Response {
    let pal = theme::current_palette(ui.ctx());
    let (fill_rest, fill_hover, stroke_rest, stroke_hover, fg_rest, fg_hover) = if selected {
        (
            pal.accent.gamma_multiply(0.18),
            pal.accent.gamma_multiply(0.25),
            Stroke::new(1.0, pal.accent.gamma_multiply(0.75)),
            Stroke::new(1.0, pal.accent),
            pal.accent,
            pal.accent,
        )
    } else {
        (
            pal.bg_surface,
            pal.bg_elevated,
            Stroke::new(1.0, pal.border_subtle),
            Stroke::new(1.0, pal.border_strong),
            pal.text_secondary,
            pal.text_primary,
        )
    };
    render_button_spec(
        ui,
        icon,
        text,
        ButtonSpec {
            height: 25.0,
            pad_x: 9.0,
            icon_size: 13.5,
            font_size: 12.0,
            radius: 6,
            semibold_text: selected,
            fill_rest,
            fill_hover,
            stroke_rest,
            stroke_hover,
            fg_rest,
            fg_hover,
            trailing_chevron: false,
        },
    )
}

pub fn ghost_menu_button(
    ui: &mut egui::Ui,
    icon: Option<Icon>,
    text: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> Response {
    let is_open_id = ui.scope_id().with(("menu_btn_open", icon.map(|i| i as u16)));
    let was_open = ui.ctx().data(|d| d.get_temp::<bool>(is_open_id).unwrap_or(false));
    let resp = ghost_button_sized(ui, icon, text, was_open, 28.0, 9.0, 14.5, 12.5, true);
    let popup_id = Popup::default_response_id(&resp);
    let is_open = Popup::is_id_open(ui.ctx(), popup_id);
    ui.ctx().data_mut(|d| d.insert_temp(is_open_id, is_open));

    let config = MenuConfig::find(ui);
    Popup::menu(&resp)
        .close_behavior(config.close_behavior)
        .style(menu_style)
        .info(UiStackInfo::new(UiKind::Menu).with_tag_value(MenuConfig::MENU_CONFIG_TAG, config))
        .show(add_contents);
    resp
}

pub fn rail_tool_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    hint: &str,
    active: bool,
    expanded: bool,
) -> Response {
    let ctx = ui.ctx().clone();
    let pal = theme::current_palette(&ctx);
    let size = if expanded {
        egui::vec2(ui.available_width(), 30.0)
    } else {
        egui::vec2(38.0, 32.0)
    };
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover_t = ctx.animate_bool_responsive(resp.id.with("rail_h"), resp.hovered());
    if resp.hovered() {
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }

    let bg = if active {
        lerp_color(
            pal.accent.gamma_multiply(0.15),
            pal.accent.gamma_multiply(0.22),
            hover_t,
        )
    } else {
        lerp_color(Color32::TRANSPARENT, pal.bg_elevated, hover_t)
    };
    let fg = if active {
        pal.accent
    } else {
        lerp_color(pal.text_secondary, pal.text_primary, hover_t)
    };

    let painter = ui.painter();
    if bg.a() > 0 {
        painter.rect_filled(rect, CornerRadius::same(7), bg);
    }
    if active {
        let bar = Rect::from_min_size(
            Pos2::new(rect.min.x + 1.5, rect.center().y - 9.0),
            Vec2::new(3.0, 18.0),
        );
        painter.rect_filled(bar, CornerRadius::same(2), pal.accent);
    }

    if expanded {
        let icon_center = Pos2::new(rect.min.x + 17.0, rect.center().y);
        icons::paint(painter, &ctx, icon, icon_center, 16.0, fg);
        let font = if active {
            FontId::new(12.5, theme::semibold_family())
        } else {
            FontId::proportional(12.5)
        };
        painter.text(
            Pos2::new(rect.min.x + 31.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            if active { pal.text_primary } else { fg },
        );
        resp.on_hover_text(hint)
    } else {
        icons::paint(painter, &ctx, icon, rect.center(), 17.0, fg);
        resp.on_hover_ui(|ui| {
            ui.set_max_width(240.0);
            ui.label(semibold(label));
            ui.weak(hint);
        })
    }
}

pub fn color_dot(ui: &mut egui::Ui, color: Color32, selected: bool, diameter: f32) -> Response {
    let ctx = ui.ctx().clone();
    let pal = theme::current_palette(&ctx);
    let slot = diameter + 4.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(slot), Sense::click());
    if resp.hovered() {
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    let r = diameter * 0.5;
    let c = rect.center();
    let painter = ui.painter();
    painter.circle_filled(c, r, color);
    painter.circle_stroke(c, r, Stroke::new(1.0, pal.border_strong));
    if selected {
        painter.circle_stroke(c, r + 2.0, Stroke::new(1.75, pal.text_primary));
    } else if resp.hovered() {
        painter.circle_stroke(c, r + 1.5, Stroke::new(1.0, pal.border_strong));
    }
    resp
}

pub fn divider_v(ui: &mut egui::Ui, height: f32) {
    let pal = theme::current_palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, height), Sense::hover());
    let x = rect.center().x;
    ui.painter().line_segment(
        [Pos2::new(x, rect.min.y + 1.0), Pos2::new(x, rect.max.y - 1.0)],
        Stroke::new(1.0, pal.border_subtle),
    );
}

pub fn divider_h(ui: &mut egui::Ui) {
    let pal = theme::current_palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 9.0), Sense::hover());
    let y = rect.center().y;
    ui.painter().line_segment(
        [Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)],
        Stroke::new(1.0, pal.border_subtle),
    );
}

pub fn section_card(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    let pal = theme::current_palette(ui.ctx());
    Frame::NONE
        .fill(pal.bg_surface)
        .stroke(Stroke::new(1.0, pal.border_subtle))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(semibold(title).size(13.0).color(pal.text_primary));
            ui.add_space(6.0);
            add_contents(ui);
        });
}

pub fn kbd_badge(painter: &Painter, pos: Pos2, align: Align2, text: &str, pal: &Palette) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(10.5), pal.text_secondary);
    let size = galley.size() + Vec2::new(10.0, 4.0);
    let rect = align.anchor_size(pos, size);
    painter.rect_filled(rect, CornerRadius::same(4), pal.bg_elevated);
    painter.rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, pal.border_strong), StrokeKind::Inside);
    painter.galley(rect.min + Vec2::new(5.0, 2.0), galley, pal.text_secondary);
    rect
}

pub fn floating_capsule_frame(pal: &Palette) -> Frame {
    Frame::NONE
        .fill(pal.bg_surface)
        .stroke(Stroke::new(1.0, pal.border_strong))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(8, 6))
        .shadow(egui::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: pal.shadow_color,
        })
}
