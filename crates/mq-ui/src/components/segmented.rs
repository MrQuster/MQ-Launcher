//! Segmented switch component for toggling view modes (grid/list) and display options.

use egui::{CursorIcon, Response, Sense, Stroke, Ui, Vec2};

use crate::components::registry::{icon_tech_name, register_widget, ElementKind, ElementMeta};
use crate::theme::{Tokens, RADIUS_BUTTON};
use crate::widgets::{paint_vector_icon, VectorIcon};

pub struct SegmentedOption<T: PartialEq + Clone> {
    pub value: T,
    pub label: String,
    pub icon: Option<VectorIcon>,
    pub tooltip: Option<String>,
}

impl<T: PartialEq + Clone> SegmentedOption<T> {
    pub fn new(value: T, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
            icon: None,
            tooltip: None,
        }
    }

    pub fn with_icon(mut self, icon: VectorIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn with_tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }
}

pub fn segmented_switch<T: PartialEq + Clone>(
    ui: &mut Ui,
    tokens: &Tokens,
    selected: &mut T,
    options: &[SegmentedOption<T>],
) -> Response {
    let height = 32.0;
    let mut total_width = 0.0;
    let mut widths = Vec::with_capacity(options.len());

    for opt in options {
        let mut w = 16.0;
        if opt.icon.is_some() {
            w += 18.0;
        }
        if !opt.label.is_empty() {
            let text_w = ui
                .painter()
                .layout_no_wrap(
                    opt.label.to_owned(),
                    egui::FontId::proportional(12.5),
                    tokens.text,
                )
                .size()
                .x;
            w += text_w + 16.0;
        }
        w = w.max(34.0);
        widths.push(w);
        total_width += w;
    }

    let (outer_rect, mut main_resp) =
        ui.allocate_exact_size(Vec2::new(total_width, height), Sense::hover());
    let style = crate::theme_styles::ThemeStyleParams::active(ui.ctx());
    let outer_radius = style.control_corner_radius(height, RADIUS_BUTTON);
    let inner_radius = style.control_corner_radius(height - 6.0, RADIUS_BUTTON - 1_u8);
    let hover_radius = style.control_corner_radius(height - 6.0, RADIUS_BUTTON - 2_u8);
    let painter = ui.painter();

    if style.refraction_border {
        crate::theme_styles::paint_glass_surface_overlay(
            ui.ctx(),
            painter,
            outer_rect,
            outer_radius,
            &style,
            tokens,
            0.18,
            true,
        );
    } else {
        painter.rect(
            outer_rect,
            outer_radius,
            tokens.bg_input,
            Stroke::new(1.0_f32, tokens.border),
            egui::StrokeKind::Inside,
        );
    }

    let active_pill_slot = painter.add(egui::Shape::Noop);
    let mut selected_seg_rect: Option<egui::Rect> = None;

    let mut current_x = outer_rect.left();
    for (i, opt) in options.iter().enumerate() {
        let seg_w = widths[i];
        let seg_rect = egui::Rect::from_min_size(
            egui::pos2(current_x, outer_rect.top()),
            Vec2::new(seg_w, height),
        );
        let is_selected = opt.value == *selected;
        let item_id = ui.make_persistent_id(format!("seg_{i}_{}", opt.label));
        let item_resp = ui.interact(seg_rect, item_id, Sense::click());

        if item_resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if item_resp.clicked() {
            *selected = opt.value.clone();
            main_resp.mark_changed();
        }

        if let Some(ref tt) = opt.tooltip {
            item_resp.clone().on_hover_text(tt);
        }

        let anim = crate::animation::animate_interaction(
            ui.ctx(),
            item_id.with("seg_anim"),
            item_resp.hovered(),
            item_resp.is_pointer_button_down_on(),
        );
        let active_t = crate::animation::animate_spring_bool(
            ui.ctx(),
            item_id.with("seg_active"),
            is_selected,
            crate::animation::SpringConfig::snappy(),
        );

        if is_selected {
            selected_seg_rect = Some(seg_rect);
        } else if anim.hover_t > 0.02 {
            if style.refraction_border {
                crate::theme_styles::paint_glass_surface_overlay(
                    ui.ctx(),
                    painter,
                    seg_rect.shrink(2.0),
                    hover_radius,
                    &style,
                    tokens,
                    0.55 * anim.hover_t,
                    true,
                );
            } else {
                painter.rect(
                    seg_rect.shrink(2.0),
                    hover_radius,
                    tokens.bg_surface.gamma_multiply(0.55 * anim.hover_t),
                    Stroke::NONE,
                    egui::StrokeKind::Inside,
                );
            }
        }

        let fg = crate::theme_styles::lerp_rgba(
            tokens.text_muted,
            tokens.text,
            active_t.max(anim.hover_t),
        );

        if let Some(icon) = opt.icon {
            let icon_pos = if opt.label.is_empty() {
                seg_rect.center() - Vec2::splat(8.0)
            } else {
                egui::pos2(seg_rect.left() + 8.0, seg_rect.center().y - 8.0)
            };
            let icon_rect = egui::Rect::from_min_size(icon_pos, Vec2::splat(16.0));
            paint_vector_icon(painter, icon_rect, icon, fg);
        }

        if !opt.label.is_empty() {
            let text_pos = if opt.icon.is_some() {
                egui::pos2(seg_rect.left() + 28.0, seg_rect.center().y)
            } else {
                seg_rect.center()
            };
            painter.text(
                text_pos,
                if opt.icon.is_some() {
                    egui::Align2::LEFT_CENTER
                } else {
                    egui::Align2::CENTER_CENTER
                },
                &opt.label,
                egui::FontId::proportional(12.5),
                fg,
            );
        }

        let seg_bg = if is_selected {
            tokens.bg_surface_hover
        } else {
            tokens.bg_input
        };
        let mut meta = ElementMeta::new(ElementKind::SegmentedOption)
            .label(&opt.label)
            .text(&opt.label)
            .tooltip_opt(opt.tooltip.as_deref())
            .active(is_selected)
            .fg(fg)
            .bg(seg_bg);
        if let Some(icon) = opt.icon {
            meta = meta.tech_name(icon_tech_name(icon));
        }
        register_widget(ui, &item_resp, meta);

        current_x += seg_w;
    }

    if let Some(target_seg) = selected_seg_rect {
        let target_inner = if style.refraction_border {
            target_seg.shrink(3.0)
        } else {
            target_seg.shrink(1.5)
        };
        let anim_rect = crate::animation::animate_spring_rect(
            ui.ctx(),
            ui.id().with("mq_segmented_pill"),
            target_inner,
            crate::animation::SpringConfig::rect(),
        );
        if style.refraction_border {
            let mut shapes = Vec::new();
            crate::theme_styles::collect_tinted_glass_pill_shapes(
                ui.ctx(),
                anim_rect,
                inner_radius,
                &style,
                tokens,
                crate::theme::blend_color(tokens.bg_surface_hover, tokens.accent, 0.22),
                0.75,
                &mut shapes,
            );
            if !shapes.is_empty() {
                ui.painter().set(active_pill_slot, egui::Shape::Vec(shapes));
            }
        } else {
            let shape = egui::Shape::Rect(egui::epaint::RectShape::new(
                anim_rect,
                inner_radius,
                tokens.bg_surface_hover,
                Stroke::new(1.0_f32, tokens.border_strong),
                egui::StrokeKind::Inside,
            ));
            ui.painter().set(active_pill_slot, shape);
        }
    }

    main_resp
}
