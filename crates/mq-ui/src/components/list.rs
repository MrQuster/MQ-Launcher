//! Table and list row component with 44px fixed height, 1px divider, and hover highlight.

use egui::{CursorIcon, Response, Sense, Stroke, Ui, Vec2};

use crate::components::registry::{register_widget, ElementKind, ElementMeta};
use crate::theme::Tokens;

pub fn list_row<R>(
    ui: &mut Ui,
    tokens: &Tokens,
    height: f32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> (Response, R) {
    let desired_size = Vec2::new(ui.available_width(), height.max(40.0));
    let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click());

    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let anim = crate::animation::animate_interaction(
        ui.ctx(),
        response.id.with("list_row_anim"),
        response.hovered(),
        response.is_pointer_button_down_on(),
    );
    let bg_color = crate::theme_styles::lerp_rgba(
        tokens.bg_surface,
        tokens.bg_surface_hover,
        (anim.hover_t + 0.2 * anim.press_t).min(1.0),
    );
    let visual_rect = anim.transform_card_rect(rect);

    let painter = ui.painter();
    painter.rect_filled(visual_rect, 0.0, bg_color);
    if anim.hover_t > 0.02 {
        let bar_h = ((visual_rect.height() - 12.0).max(8.0)) * anim.hover_t;
        let bar_rect = egui::Rect::from_center_size(
            egui::pos2(visual_rect.left() + 2.0, visual_rect.center().y),
            Vec2::new(2.5, bar_h),
        );
        painter.rect_filled(bar_rect, 1.25, tokens.accent.gamma_multiply(anim.hover_t));
    }
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.bottom()),
            egui::pos2(rect.right(), rect.bottom()),
        ],
        Stroke::new(1.0_f32, tokens.border),
    );

    let content_rect = rect.shrink2(Vec2::new(12.0, 4.0));
    let mut row_ui = ui.new_child(egui::UiBuilder::new().max_rect(content_rect));
    let res = add_contents(&mut row_ui);

    register_widget(
        ui,
        &response,
        ElementMeta::new(ElementKind::ListRow)
            .fg(tokens.text)
            .bg(bg_color),
    );

    (response, res)
}
