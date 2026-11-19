//! Filter chip component for logs and tags with colored level dots and counts.

use egui::{Color32, CursorIcon, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::components::registry::{register_widget, ElementKind, ElementMeta};
use crate::theme::{Tokens, RADIUS_TAG};

/// Toggleable filter chip with optional colored level dot and count badge.
pub struct FilterChip<'a> {
    pub label: &'a str,
    pub active: bool,
    pub dot_color: Option<Color32>,
    pub count: Option<usize>,
}

impl<'a> FilterChip<'a> {
    pub fn new(label: &'a str, active: bool) -> Self {
        Self {
            label,
            active,
            dot_color: None,
            count: None,
        }
    }

    pub fn dot_color(mut self, color: Color32) -> Self {
        self.dot_color = Some(color);
        self
    }

    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    pub fn show(self, ui: &mut Ui, tokens: &Tokens) -> Response {
        let label_w = ui
            .painter()
            .layout_no_wrap(
                self.label.to_string(),
                egui::FontId::proportional(12.0),
                tokens.text,
            )
            .size()
            .x;
        let mut width = 16.0 + label_w; // Left/right padding + label
        if self.dot_color.is_some() {
            width += 12.0; // Dot + spacing
        }
        if let Some(cnt) = self.count {
            let cnt_w = ui
                .painter()
                .layout_no_wrap(
                    format!("{cnt}"),
                    egui::FontId::monospace(11.0),
                    tokens.text_muted,
                )
                .size()
                .x;
            width += cnt_w + 8.0;
        }
        width = width.max(48.0);

        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }

        let anim = crate::animation::animate_interaction(
            ui.ctx(),
            resp.id.with("chip_anim"),
            resp.hovered(),
            resp.is_pointer_button_down_on(),
        );
        let active_t = crate::animation::animate_spring_bool(
            ui.ctx(),
            resp.id.with("chip_active"),
            self.active,
            crate::animation::SpringConfig::snappy(),
        );
        let visual_rect = anim.transform_rect(rect);

        let painter = ui.painter();
        let bg_idle =
            crate::theme_styles::lerp_rgba(tokens.bg_input, tokens.bg_surface_hover, active_t);
        let bg_fill =
            crate::theme_styles::lerp_rgba(bg_idle, tokens.bg_surface_hover, anim.hover_t * 0.55);

        let idle_stroke = crate::theme_styles::lerp_rgba(
            tokens.border.gamma_multiply(0.6),
            tokens.border_strong,
            active_t,
        );
        let stroke_color =
            crate::theme_styles::lerp_rgba(idle_stroke, tokens.border_strong, anim.hover_t * 0.7);

        painter.rect(
            visual_rect,
            RADIUS_TAG,
            bg_fill,
            Stroke::new(1.0_f32, stroke_color),
            egui::StrokeKind::Inside,
        );

        let mut current_x = visual_rect.left() + 8.0;
        let center_y = visual_rect.center().y;

        // Render level dot if present
        if let Some(dot_col) = self.dot_color {
            let dot_rect =
                Rect::from_center_size(egui::pos2(current_x + 3.0, center_y), Vec2::splat(6.0));
            painter.circle_filled(dot_rect.center(), 3.0, dot_col);
            current_x += 12.0;
        }

        // Render chip label text
        let text_color = crate::theme_styles::lerp_rgba(
            tokens.text_muted,
            tokens.text,
            active_t.max(anim.hover_t),
        );

        let galley = painter.layout_no_wrap(
            self.label.to_string(),
            egui::FontId::proportional(12.0),
            text_color,
        );
        let text_w = galley.size().x;
        painter.galley(
            egui::pos2(current_x, center_y - galley.size().y * 0.5),
            galley,
            text_color,
        );
        current_x += text_w;

        // Render count badge if present
        if let Some(cnt) = self.count {
            current_x += 4.0;
            let count_str = format!("{cnt}");
            let count_galley =
                painter.layout_no_wrap(count_str, egui::FontId::monospace(10.5), tokens.text_muted);
            painter.galley(
                egui::pos2(current_x, center_y - count_galley.size().y * 0.5),
                count_galley,
                tokens.text_muted,
            );
        }

        register_widget(
            ui,
            &resp,
            ElementMeta::new(ElementKind::Chip)
                .label(self.label)
                .text(self.label)
                .active(self.active)
                .fg(text_color)
                .bg(bg_fill),
        );

        resp
    }
}
