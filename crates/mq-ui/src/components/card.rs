//! Solid Card container with 8px corner radius and 1px border.

use egui::{Frame, Margin, Stroke, Ui};

use crate::theme::{Tokens, RADIUS_CARD};

pub fn card_panel<R>(
    ui: &mut Ui,
    tokens: &Tokens,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    let style = crate::theme_styles::ThemeStyleParams::active(ui.ctx());
    let cr = style.card_corner_radius(RADIUS_CARD.nw as f32);
    let frame = Frame::new()
        .fill(style.surface_fill(tokens))
        .stroke(if style.refraction_border {
            Stroke::NONE
        } else {
            Stroke::new(1.0_f32, tokens.border)
        })
        .corner_radius(cr)
        .inner_margin(Margin::same(12));

    crate::theme_styles::show_themed_card(
        ui,
        frame,
        cr,
        ui.id().with("mq_card_panel"),
        &style,
        tokens,
        add_contents,
    )
}
