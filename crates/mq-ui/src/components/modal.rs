//! Centered modal dialog container with animated backdrop, 8px panel, and Esc dismiss.

use egui::{Color32, Context, Frame, LayerId, Margin, Order, Stroke, Ui};

use crate::components::registry::{register_rect, ElementKind, ElementMeta};
use crate::theme::{Tokens, RADIUS_CARD};

pub fn render_modal_container<R>(
    ctx: &Context,
    tokens: &Tokens,
    title: &str,
    max_width: f32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let modal_id = egui::Id::new(("mq_modal_container", title));
    let anim = crate::animation::animate_modal_presence(ctx, modal_id, true);
    let screen_rect = ctx.screen_rect();
    let painter = ctx.layer_painter(LayerId::new(Order::Middle, modal_id.with("backdrop")));

    let backdrop_a = (anim.backdrop_alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    painter.rect_filled(screen_rect, 0.0, Color32::from_black_alpha(backdrop_a));

    let modal_width = max_width.min(screen_rect.width() - 32.0).max(360.0);
    let modal_pos = egui::pos2(
        (screen_rect.width() - modal_width) / 2.0,
        (screen_rect.height() * 0.20).max(32.0),
    );

    let style = crate::theme_styles::ThemeStyleParams::active(ctx);
    let modal_r = style.modal_corner_radius(RADIUS_CARD);
    let frame = Frame::new()
        .fill(style.modal_fill(tokens))
        .stroke(Stroke::new(1.0_f32, tokens.border_strong))
        .corner_radius(modal_r)
        .inner_margin(Margin::same(20));

    let area_id = modal_id.with("window");
    let area = egui::Area::new(area_id)
        .order(Order::Foreground)
        .fade_in(false)
        .fixed_pos(modal_pos);

    let start_idx = ctx.graphics(|g| {
        g.get(LayerId::new(Order::Foreground, area_id))
            .map_or(0, |l| l.next_idx().0)
    });

    let predicted_center = ctx.memory(|m| m.area_rect(area_id)).map_or_else(
        || egui::pos2(screen_rect.center().x, modal_pos.y + 120.0),
        |r| r.center(),
    );
    let pre_xform = egui::emath::TSTransform {
        scaling: anim.scale,
        translation: predicted_center.to_vec2() * (1.0 - anim.scale)
            + egui::vec2(0.0, anim.offset_y),
    };
    crate::animation::push_visual_transform(ctx, pre_xform, anim.alpha);

    let area_resp = area.show(ctx, |ui| {
        ui.set_width(modal_width);
        let resp = crate::theme_styles::show_themed_card(
            ui,
            frame,
            modal_r,
            egui::Id::new("mq_modal_glass_card"),
            &style,
            tokens,
            |ui| {
                ui.label(
                    egui::RichText::new(title)
                        .size(17.0)
                        .strong()
                        .color(tokens.text),
                );
                ui.add_space(16.0);
                add_contents(ui)
            },
        );
        register_rect(
            ui,
            resp.response.id,
            resp.response.rect,
            true,
            false,
            false,
            ElementMeta::new(ElementKind::Modal)
                .label(title)
                .text(title)
                .interactive(false)
                .fg(tokens.text)
                .bg(tokens.bg_surface),
        );
        resp.inner
    });

    crate::animation::pop_visual_transform(ctx);

    let layer_id = area_resp.response.layer_id;
    let end_idx = ctx.graphics(|g| g.get(layer_id).map_or(0, |l| l.next_idx().0));
    let actual_center = area_resp.response.rect.center();
    let actual_xform = egui::emath::TSTransform {
        scaling: anim.scale,
        translation: actual_center.to_vec2() * (1.0 - anim.scale) + egui::vec2(0.0, anim.offset_y),
    };
    crate::animation::adjust_paint_list_opacity(ctx, layer_id, start_idx, end_idx, anim.alpha);
    if actual_xform != egui::emath::TSTransform::IDENTITY && end_idx > start_idx {
        ctx.graphics_mut(|g| {
            g.entry(layer_id).transform_range(
                egui::layers::ShapeIdx(start_idx),
                egui::layers::ShapeIdx(end_idx),
                actual_xform,
            );
        });
    }

    Some(area_resp.inner)
}
