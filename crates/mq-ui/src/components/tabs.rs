//! Underline tab bar for detail screens (Console, Mods, Resourcepacks, Shaders, Worlds, etc.)
//! Conforms to specification B5: active tab = text + accent 2px underline, hover = subtle bg,
//! non-wrapping horizontal scroll area for narrow windows.

use egui::{CursorIcon, Response, Sense, Ui, Vec2};

use crate::components::registry::{icon_tech_name, register_widget, ElementKind, ElementMeta};
use crate::theme::{Tokens, RADIUS_BUTTON};
use crate::widgets::{paint_vector_icon, VectorIcon};

pub fn render_underline_tabs<T: Copy + PartialEq>(
    ui: &mut Ui,
    tokens: &Tokens,
    tabs: &[(T, &str)],
    current: &mut T,
) {
    let converted: Vec<(T, &str, Option<VectorIcon>)> =
        tabs.iter().map(|&(t, label)| (t, label, None)).collect();
    underline_tabs(ui, tokens, current, &converted);
}

pub fn underline_tabs<T: Copy + PartialEq>(
    ui: &mut Ui,
    tokens: &Tokens,
    current: &mut T,
    tabs: &[(T, &str, Option<VectorIcon>)],
) -> Response {
    let mut tab_changed = false;
    let bar_id = ui.id().with("mq_underline_tabs_indicator");
    let indicator_bg_slot = ui.painter().add(egui::Shape::Noop);
    let mut selected_rect: Option<egui::Rect> = None;

    let mut main_resp = ui
        .horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(12.0, 4.0);
            for &(tab, label, opt_icon) in tabs {
                let is_selected = *current == tab;

                let text_w = ui
                    .painter()
                    .layout_no_wrap(
                        label.to_owned(),
                        egui::FontId::proportional(13.5),
                        tokens.text,
                    )
                    .size()
                    .x;
                let mut width = text_w + 20.0;
                if opt_icon.is_some() {
                    width += 20.0;
                }
                width = width.max(48.0);

                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(width, 34.0), Sense::click());

                if response.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }

                if response.clicked() {
                    *current = tab;
                    tab_changed = true;
                }

                let anim = crate::animation::animate_interaction(
                    ui.ctx(),
                    response.id.with("utab_anim"),
                    response.hovered(),
                    response.is_pointer_button_down_on(),
                );
                let active_t = crate::animation::animate_spring_bool(
                    ui.ctx(),
                    response.id.with("utab_sel"),
                    is_selected,
                    crate::animation::SpringConfig::snappy(),
                );
                let text_color = crate::theme_styles::lerp_rgba(
                    tokens.text_muted,
                    tokens.text,
                    active_t.max(anim.hover_t),
                );
                let visual_rect = anim.transform_rect(rect);

                let painter = ui.painter();

                if anim.hover_t > 0.02 && !is_selected {
                    painter.rect_filled(
                        visual_rect.shrink2(Vec2::new(2.0, 4.0)),
                        RADIUS_BUTTON,
                        tokens.bg_surface_hover.gamma_multiply(0.45 * anim.hover_t),
                    );
                }

                let mut text_x = visual_rect.center().x;
                if let Some(icon) = opt_icon {
                    let icon_rect = egui::Rect::from_center_size(
                        egui::pos2(visual_rect.left() + 14.0, visual_rect.center().y - 1.0),
                        Vec2::splat(14.0),
                    );
                    paint_vector_icon(painter, icon_rect, icon, text_color);
                    text_x = visual_rect.left() + 26.0;
                }

                let text_align = if opt_icon.is_some() {
                    egui::Align2::LEFT_CENTER
                } else {
                    egui::Align2::CENTER_CENTER
                };

                painter.text(
                    egui::pos2(text_x, visual_rect.center().y - 2.0),
                    text_align,
                    label,
                    egui::FontId::proportional(13.5),
                    text_color,
                );

                if is_selected {
                    selected_rect = Some(rect);
                }

                let mut meta = ElementMeta::new(ElementKind::Tab)
                    .label(label)
                    .text(label)
                    .active(is_selected)
                    .fg(text_color)
                    .bg(tokens.bg_app);
                if let Some(icon) = opt_icon {
                    meta = meta.tech_name(icon_tech_name(icon));
                }
                register_widget(ui, &response, meta);
            }
        })
        .response;
    if tab_changed {
        main_resp.mark_changed();
    }

    if let Some(target_rect) = selected_rect {
        let anim_rect = crate::animation::animate_spring_rect(
            ui.ctx(),
            bar_id,
            target_rect,
            crate::animation::SpringConfig::rect(),
        );
        let pill_shape = egui::Shape::rect_filled(
            anim_rect.shrink2(Vec2::new(2.0, 4.0)),
            RADIUS_BUTTON,
            tokens.bg_surface_hover.gamma_multiply(0.32),
        );
        ui.painter().set(indicator_bg_slot, pill_shape);
        let line_rect = egui::Rect::from_min_max(
            egui::pos2(anim_rect.left() + 4.0, anim_rect.bottom() - 2.5),
            egui::pos2(anim_rect.right() - 4.0, anim_rect.bottom()),
        );
        ui.painter()
            .rect_filled(line_rect, egui::CornerRadius::same(1), tokens.accent);
    }

    main_resp
}
