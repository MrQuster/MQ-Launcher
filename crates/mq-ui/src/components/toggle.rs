//! Clean 36x20px toggle switch component.

use egui::{CursorIcon, Response, Sense, Stroke, Ui, Vec2};

use crate::components::registry::{register_widget, ElementKind, ElementMeta};
use crate::theme::Tokens;

pub fn toggle_switch(ui: &mut Ui, tokens: &Tokens, on: &mut bool) -> Response {
    let desired_size = Vec2::new(40.0, 24.0);
    let (rect, mut response) = ui.allocate_exact_size(desired_size, Sense::click());
    let track_rect = egui::Rect::from_center_size(rect.center(), Vec2::new(36.0, 20.0));

    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }

    let t = crate::animation::animate_spring_bool(
        ui.ctx(),
        response.id,
        *on,
        crate::animation::SpringConfig::snappy(),
    );
    let press_t = crate::animation::animate_spring_bool(
        ui.ctx(),
        response.id.with("toggle_press"),
        response.is_pointer_button_down_on(),
        crate::animation::SpringConfig::snappy().with_stiffness(560.0),
    );

    let bg_color = crate::theme::blend_color(tokens.bg_input, tokens.accent, t);
    let border_color = crate::theme::blend_color(tokens.border, tokens.accent, t);

    let painter = ui.painter();
    painter.rect(
        track_rect,
        10.0,
        bg_color,
        Stroke::new(1.0_f32, border_color),
        egui::StrokeKind::Inside,
    );

    let circle_x = egui::lerp((track_rect.left() + 10.0)..=(track_rect.right() - 10.0), t);
    let knob_color = crate::theme_styles::lerp_rgba(tokens.text_muted, tokens.accent_text, t);
    let knob_r = 7.0 * (1.0 - 0.12 * press_t);
    painter.circle_filled(
        egui::pos2(circle_x, track_rect.center().y),
        knob_r,
        knob_color,
    );

    register_widget(
        ui,
        &response,
        ElementMeta::new(ElementKind::Toggle)
            .text(if *on { "on" } else { "off" })
            .active(*on)
            .fg(knob_color)
            .bg(bg_color),
    );

    response
}
