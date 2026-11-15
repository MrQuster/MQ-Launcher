//! Solid Button and IconButton components with 6px corner radius,
//! clear visual hover/pressed/focus states, and vector icon support.

use egui::{CursorIcon, Response, Stroke, Ui, Vec2};

use crate::components::registry::{icon_tech_name, register_widget, ElementKind, ElementMeta};
use crate::theme::{Tokens, RADIUS_BUTTON};
use crate::widgets::{paint_vector_icon, VectorIcon};

fn srgb_channel_to_linear(c: u8) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

fn relative_luminance(c: egui::Color32) -> f32 {
    0.2126 * srgb_channel_to_linear(c.r())
        + 0.7152 * srgb_channel_to_linear(c.g())
        + 0.0722 * srgb_channel_to_linear(c.b())
}

fn wcag_contrast(a: egui::Color32, b: egui::Color32) -> f32 {
    let l1 = relative_luminance(a);
    let l2 = relative_luminance(b);
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// Ensures button foreground text/icon meets WCAG AA (`>= 4.5:1`) against `bg`.
pub(crate) fn contrast_fg_for_bg(preferred_fg: egui::Color32, bg: egui::Color32) -> egui::Color32 {
    if wcag_contrast(preferred_fg, bg) >= 4.5 {
        return preferred_fg;
    }
    let dark_ink = egui::Color32::from_rgb(0x0B, 0x10, 0x14);
    let light_ink = egui::Color32::WHITE;
    if wcag_contrast(dark_ink, bg) >= wcag_contrast(light_ink, bg) {
        dark_ink
    } else {
        light_ink
    }
}

/// Renders a primary action button (e.g. "Играть", "Создать", "Сохранить")
/// using the single accent color (`#4C9A82` in Matte Dark) with high contrast text.
pub fn button_primary(
    ui: &mut Ui,
    tokens: &Tokens,
    icon: Option<VectorIcon>,
    text: &str,
) -> Response {
    let base_bg = crate::theme::blend_color(tokens.bg_surface, tokens.accent, 0.26);
    let hover_bg = crate::theme::blend_color(tokens.bg_surface, tokens.accent, 0.38);
    let active_bg = crate::theme::blend_color(tokens.bg_surface, tokens.accent, 0.44);
    let fg_color = contrast_fg_for_bg(tokens.text, base_bg);
    let text_w = ui
        .painter()
        .layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(13.5),
            fg_color,
        )
        .size()
        .x;
    let width = text_w + if icon.is_some() { 46.0 } else { 24.0 };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 34.0), egui::Sense::click());

    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let bg_color = if response.is_pointer_button_down_on() {
        active_bg
    } else if response.hovered() {
        hover_bg
    } else {
        base_bg
    };
    let fg_color = contrast_fg_for_bg(tokens.text, bg_color);

    let painter = ui.painter();
    painter.rect_filled(rect, RADIUS_BUTTON, bg_color);
    painter.rect_stroke(
        rect,
        RADIUS_BUTTON,
        Stroke::new(1.5_f32, tokens.accent),
        egui::StrokeKind::Inside,
    );

    if response.has_focus() {
        painter.rect_stroke(
            rect.expand(2.0),
            RADIUS_BUTTON,
            Stroke::new(2.0_f32, tokens.accent_hover),
            egui::StrokeKind::Outside,
        );
    }

    let mut content_left = rect.left() + 12.0;
    if let Some(ic) = icon {
        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(content_left, rect.center().y - 8.0),
            Vec2::splat(16.0),
        );
        paint_vector_icon(painter, icon_rect, ic, fg_color);
        content_left += 22.0;
    }

    let text_pos = egui::pos2(content_left, rect.center().y - 7.0);
    painter.text(
        text_pos,
        egui::Align2::LEFT_TOP,
        text,
        egui::FontId::proportional(13.5),
        fg_color,
    );

    if ui.is_enabled() {
        register_widget(
            ui,
            &response,
            ElementMeta::new(ElementKind::Button)
                .label(text)
                .text(text)
                .fg(fg_color)
                .bg(bg_color),
        );
    }

    response
}

/// Renders a secondary / neutral button with surface background and 1px border.
pub fn button_secondary(
    ui: &mut Ui,
    tokens: &Tokens,
    icon: Option<VectorIcon>,
    text: &str,
) -> Response {
    let text_w = ui
        .painter()
        .layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(13.0),
            tokens.text,
        )
        .size()
        .x;
    let width = text_w + if icon.is_some() { 42.0 } else { 22.0 };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 32.0), egui::Sense::click());

    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let (bg_color, stroke_color) = if response.is_pointer_button_down_on() {
        (tokens.bg_surface_hover, tokens.accent)
    } else if response.hovered() {
        (tokens.bg_surface_hover, tokens.border_strong)
    } else {
        (tokens.bg_surface, tokens.border)
    };

    let painter = ui.painter();
    painter.rect(
        rect,
        RADIUS_BUTTON,
        bg_color,
        Stroke::new(1.0_f32, stroke_color),
        egui::StrokeKind::Inside,
    );

    if response.has_focus() {
        painter.rect_stroke(
            rect.expand(2.0),
            RADIUS_BUTTON,
            Stroke::new(2.0_f32, tokens.accent),
            egui::StrokeKind::Outside,
        );
    }

    let mut content_left = rect.left() + 10.0;
    if let Some(ic) = icon {
        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(content_left, rect.center().y - 7.5),
            Vec2::splat(15.0),
        );
        paint_vector_icon(painter, icon_rect, ic, tokens.text);
        content_left += 20.0;
    }

    let text_pos = egui::pos2(content_left, rect.center().y - 6.5);
    painter.text(
        text_pos,
        egui::Align2::LEFT_TOP,
        text,
        egui::FontId::proportional(13.0),
        tokens.text,
    );

    if ui.is_enabled() {
        register_widget(
            ui,
            &response,
            ElementMeta::new(ElementKind::Button)
                .label(text)
                .text(text)
                .fg(tokens.text)
                .bg(bg_color),
        );
    }

    response
}

/// Compact square icon button (32x32px) for toolbar actions (e.g. folder, refresh, menu).
pub fn icon_button(ui: &mut Ui, tokens: &Tokens, icon: VectorIcon, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(32.0), egui::Sense::click());

    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let (bg, border) = if response.is_pointer_button_down_on() {
        (tokens.bg_surface_hover, tokens.accent)
    } else if response.hovered() {
        (tokens.bg_surface_hover, tokens.border_strong)
    } else {
        (tokens.bg_surface, tokens.border)
    };

    let painter = ui.painter();
    painter.rect(
        rect,
        RADIUS_BUTTON,
        bg,
        Stroke::new(1.0_f32, border),
        egui::StrokeKind::Inside,
    );

    let icon_rect = rect.shrink(8.0);
    let icon_col = if response.hovered() {
        tokens.text
    } else {
        tokens.text_muted
    };
    paint_vector_icon(painter, icon_rect, icon, icon_col);

    register_widget(
        ui,
        &response,
        ElementMeta::new(ElementKind::IconButton)
            .tech_name(icon_tech_name(icon))
            .tooltip(tooltip)
            .fg(icon_col)
            .bg(bg),
    );

    if !tooltip.is_empty() {
        response.on_hover_text(tooltip)
    } else {
        response
    }
}
