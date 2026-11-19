//! Minimal solid progress bar component (3px line or standard 8px bar) using single accent.

use egui::{CornerRadius, Rect, Ui, Vec2};

use crate::components::registry::{register_widget, ElementKind, ElementMeta};
use crate::theme::Tokens;

pub fn render_progress_bar(ui: &mut Ui, tokens: &Tokens, progress: f32, height: f32) {
    let clamped = crate::state::safe_progress_clamp(progress);
    let p = crate::animation::animate_spring_f32(
        ui.ctx(),
        ui.id().with("mq_progress_bar"),
        clamped,
        crate::animation::SpringConfig::smooth(),
    )
    .clamp(0.0, 1.0);
    let desired_width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(desired_width, height.max(2.0)),
        egui::Sense::hover(),
    );

    let painter = ui.painter();
    let cr = CornerRadius::same((height / 2.0).round() as u8);
    painter.rect_filled(rect, cr, tokens.bg_input);

    if p > 0.001 {
        let fill_rect = Rect::from_min_size(rect.min, Vec2::new(rect.width() * p, rect.height()));
        painter.rect_filled(fill_rect, cr, tokens.accent);
    }

    register_widget(
        ui,
        &response,
        ElementMeta::new(ElementKind::Progress)
            .interactive(false)
            .fg(tokens.accent)
            .bg(tokens.bg_input),
    );
}
