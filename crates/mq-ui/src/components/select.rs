//! Dropdown select component for options and sort menus.

use egui::{CursorIcon, Response, Sense, Stroke, Ui, Vec2};

use crate::components::registry::{register_widget, ElementKind, ElementMeta};
use crate::theme::{Tokens, RADIUS_BUTTON};
use crate::widgets::paint_animated_chevron;

pub struct SelectOption<T: Clone + PartialEq> {
    pub value: T,
    pub label: String,
}

impl<T: Clone + PartialEq> SelectOption<T> {
    pub fn new(value: T, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
        }
    }
}

pub struct Select<'a, T: Clone + PartialEq> {
    pub id_source: &'a str,
    pub label_prefix: &'a str,
    pub selected: &'a mut T,
    pub options: &'a [SelectOption<T>],
}

impl<'a, T: Clone + PartialEq> Select<'a, T> {
    pub fn new(
        id_source: &'a str,
        label_prefix: &'a str,
        selected: &'a mut T,
        options: &'a [SelectOption<T>],
    ) -> Self {
        Self {
            id_source,
            label_prefix,
            selected,
            options,
        }
    }

    pub fn show(self, ui: &mut Ui, tokens: &Tokens) -> Response {
        let current_label = self
            .options
            .iter()
            .find(|opt| &opt.value == self.selected)
            .map(|opt| opt.label.as_str())
            .unwrap_or("");

        let display_text = if self.label_prefix.is_empty() {
            current_label.to_string()
        } else {
            format!("{}: {}", self.label_prefix, current_label)
        };

        let text_w = ui
            .painter()
            .layout_no_wrap(
                display_text.clone(),
                egui::FontId::proportional(12.5),
                tokens.text,
            )
            .size()
            .x;
        let width = (text_w + 36.0).max(120.0);
        let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(width, 32.0), Sense::click());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }

        let popup_id = ui.make_persistent_id(self.id_source);
        let is_open = ui.memory(|mem| mem.is_popup_open(popup_id));
        let open_t = crate::animation::animate_spring_bool(
            ui.ctx(),
            popup_id.with("select_open"),
            is_open,
            crate::animation::SpringConfig::snappy(),
        );
        let anim = crate::animation::animate_interaction(
            ui.ctx(),
            resp.id.with("select_hover"),
            resp.hovered() || is_open,
            resp.is_pointer_button_down_on(),
        );
        let visual_rect = anim.transform_rect(rect);

        let style = crate::theme_styles::ThemeStyleParams::active(ui.ctx());
        let pill_r = style.control_corner_radius(32.0, RADIUS_BUTTON);
        let painter = ui.painter();
        let bg = crate::theme_styles::lerp_rgba(
            tokens.bg_surface,
            tokens.bg_surface_hover,
            anim.hover_t,
        );
        let border = crate::theme_styles::lerp_rgba(
            crate::theme_styles::lerp_rgba(tokens.border, tokens.border_strong, anim.hover_t),
            tokens.accent,
            open_t * 0.7,
        );

        if style.refraction_border {
            crate::theme_styles::paint_glass_surface_overlay(
                ui.ctx(),
                painter,
                visual_rect,
                pill_r,
                &style,
                tokens,
                0.22 + 0.63 * anim.hover_t,
                true,
            );
        } else {
            painter.rect(
                visual_rect,
                pill_r,
                bg,
                Stroke::new(1.0_f32, border),
                egui::StrokeKind::Inside,
            );
        }

        // Draw label
        let text_pos = egui::pos2(visual_rect.left() + 10.0, visual_rect.center().y - 7.0);
        painter.text(
            text_pos,
            egui::Align2::LEFT_TOP,
            &display_text,
            egui::FontId::proportional(12.5),
            tokens.text,
        );

        // Draw animated chevron
        let chev_rect = egui::Rect::from_center_size(
            egui::pos2(visual_rect.right() - 14.0, visual_rect.center().y),
            Vec2::splat(12.0),
        );
        paint_animated_chevron(
            painter,
            chev_rect,
            open_t,
            crate::theme_styles::lerp_rgba(
                tokens.text_muted,
                tokens.text,
                open_t.max(anim.hover_t),
            ),
        );

        // Popup combo menu on click
        if resp.clicked() {
            ui.memory_mut(|mem| mem.toggle_popup(popup_id));
        }

        let mut changed = false;
        let selected_val = self.selected;
        let options = self.options;

        crate::animation::show_animated_dropdown_popup(
            ui,
            popup_id,
            &resp,
            width,
            false,
            false,
            |ui| {
                ui.set_min_width(width);
                for opt in options {
                    let is_sel = &opt.value == selected_val;
                    let opt_text = egui::RichText::new(&opt.label)
                        .color(if is_sel { tokens.accent } else { tokens.text })
                        .size(12.5);
                    if ui.selectable_label(is_sel, opt_text).clicked() {
                        *selected_val = opt.value.clone();
                        changed = true;
                        ui.memory_mut(|mem| mem.close_popup());
                    }
                }
            },
        );

        if changed {
            resp.mark_changed();
        }

        register_widget(
            ui,
            &resp,
            ElementMeta::new(ElementKind::Select)
                .id(self.id_source)
                .label(if self.label_prefix.is_empty() {
                    current_label
                } else {
                    self.label_prefix
                })
                .text(display_text.as_str())
                .fg(tokens.text)
                .bg(bg),
        );

        resp
    }
}
