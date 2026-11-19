//! Ephemeral notification toasts in bottom-right corner with smooth slide-in,
//! spring vertical stacking, and gentle fade-out.

use egui::{Context, CornerRadius, Id, Pos2, Rect, Stroke, Vec2};
use std::time::{Duration, Instant};

use crate::animation::{
    animate_spring_f32, animations_disabled, ease_out_cubic, mark_animation_active, SpringConfig,
};
use crate::theme::{Tokens, RADIUS_CARD};
use crate::widgets::{paint_vector_icon, VectorIcon};

const TOAST_ENTER_SECS: f32 = 0.24;
const TOAST_EXIT_SECS: f32 = 0.26;
const MAX_VISIBLE_TOASTS: usize = 5;

#[derive(Debug, Clone)]
pub struct Toast {
    pub id: u64,
    pub message: String,
    pub icon: VectorIcon,
    pub created_at: Instant,
    pub duration: Duration,
    pub is_error: bool,
}

#[derive(Clone, Default)]
pub struct ToastManager {
    toasts: Vec<Toast>,
    next_id: u64,
    last_status: Option<(String, bool)>,
}

impl ToastManager {
    /// Initializes a [`ToastManager`] seeding `last_status` from initial startup message
    /// so subsequent updates trigger fresh animated toasts without re-popping boot banner.
    pub fn new_with_initial_message(initial: Option<&(String, bool)>) -> Self {
        Self {
            toasts: Vec::new(),
            next_id: 1,
            last_status: initial.cloned(),
        }
    }

    pub fn push_info(&mut self, message: impl Into<String>) {
        let msg = message.into();
        if msg.trim().is_empty() {
            return;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.toasts.push(Toast {
            id,
            message: msg,
            icon: VectorIcon::Check,
            created_at: Instant::now(),
            duration: Duration::from_secs(4),
            is_error: false,
        });
        if self.toasts.len() > MAX_VISIBLE_TOASTS {
            self.toasts.remove(0);
        }
    }

    pub fn push_error(&mut self, message: impl Into<String>) {
        let msg = message.into();
        if msg.trim().is_empty() {
            return;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.toasts.push(Toast {
            id,
            message: msg,
            icon: VectorIcon::Warning,
            created_at: Instant::now(),
            duration: Duration::from_secs(6),
            is_error: true,
        });
        if self.toasts.len() > MAX_VISIBLE_TOASTS {
            self.toasts.remove(0);
        }
    }

    /// Watches `status_message` changes and pushes a corresponding info/error toast automatically.
    pub fn sync_status_message(&mut self, status: &Option<(String, bool)>) {
        if *status == self.last_status {
            return;
        }
        self.last_status = status.clone();
        if let Some((msg, is_err)) = status {
            if *is_err {
                self.push_error(msg.clone());
            } else {
                self.push_info(msg.clone());
            }
        }
    }

    /// Reads the persistent [`ToastManager`] from [`egui::Memory`], synchronizes it with `status`,
    /// renders visible toasts, and writes the updated manager back to memory.
    pub fn sync_and_render_in_context(
        ctx: &Context,
        tokens: &Tokens,
        status: &Option<(String, bool)>,
    ) {
        let id = Id::new("mq_app_toast_manager");
        let mut mgr = ctx.data_mut(|d| {
            d.get_temp::<ToastManager>(id)
                .unwrap_or_else(|| ToastManager::new_with_initial_message(status.as_ref()))
        });
        mgr.sync_status_message(status);
        mgr.render(ctx, tokens);
        ctx.data_mut(|d| d.insert_temp(id, mgr));
    }

    pub fn len(&self) -> usize {
        self.toasts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }

    pub fn render(&mut self, ctx: &Context, tokens: &Tokens) {
        let now = Instant::now();
        let total_extra = Duration::from_secs_f32(TOAST_EXIT_SECS);
        self.toasts
            .retain(|t| now.duration_since(t.created_at) < t.duration + total_extra);

        if self.toasts.is_empty() {
            return;
        }

        let disabled = animations_disabled(ctx);
        let screen_rect = ctx.screen_rect();
        let mut y_offset = screen_rect.bottom() - 48.0;
        let mut animating_now = false;
        let mut next_wake: Option<Duration> = None;

        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("toasts_layer"),
        ));

        for toast in self.toasts.iter().rev() {
            let elapsed = now.duration_since(toast.created_at).as_secs_f32();
            let hold_secs = toast.duration.as_secs_f32();

            let enter_t = if disabled {
                1.0
            } else {
                ease_out_cubic((elapsed / TOAST_ENTER_SECS).clamp(0.0, 1.0))
            };
            let exit_t = if disabled || elapsed <= hold_secs {
                0.0
            } else {
                ease_out_cubic(((elapsed - hold_secs) / TOAST_EXIT_SECS).clamp(0.0, 1.0))
            };

            let alpha = (enter_t * (1.0 - exit_t)).clamp(0.0, 1.0);
            if alpha <= 0.005 {
                y_offset -= 48.0;
                continue;
            }

            if !disabled && (elapsed < TOAST_ENTER_SECS || elapsed >= hold_secs) {
                animating_now = true;
            } else if !disabled {
                let rem = Duration::from_secs_f32((hold_secs - elapsed).max(0.016));
                next_wake = Some(next_wake.map_or(rem, |prev| prev.min(rem)));
            }

            let width = 320.0_f32.min((screen_rect.width() - 32.0).max(200.0));
            let height = 40.0;
            let target_y = y_offset - height;
            y_offset -= height + 8.0;

            let anim_y = animate_spring_f32(
                ctx,
                Id::new(("mq_toast_y", toast.id)),
                target_y,
                SpringConfig::smooth(),
            );
            let slide_x = (1.0 - enter_t) * 32.0 + exit_t * 24.0;
            let scale = 0.96 + 0.04 * enter_t - 0.02 * exit_t;

            let base_pos = Pos2::new(screen_rect.right() - width - 16.0 + slide_x, anim_y);
            let base_rect = Rect::from_min_size(base_pos, Vec2::new(width, height));
            let rect = Rect::from_center_size(base_rect.center(), base_rect.size() * scale);

            let accent_col = if toast.is_error {
                tokens.danger
            } else {
                tokens.accent
            }
            .gamma_multiply(alpha);
            let border_col = if toast.is_error {
                tokens.danger
            } else {
                tokens.border_strong
            }
            .gamma_multiply(alpha);
            let fill_col = tokens.bg_surface.gamma_multiply(alpha * 0.96);

            painter.rect_filled(
                rect.translate(Vec2::new(0.0, 2.0)),
                RADIUS_CARD,
                egui::Color32::from_black_alpha((34.0 * alpha).round() as u8),
            );
            painter.rect(
                rect,
                RADIUS_CARD,
                fill_col,
                Stroke::new(1.0_f32, border_col),
                egui::StrokeKind::Inside,
            );

            let stripe = Rect::from_min_size(
                Pos2::new(rect.left() + 3.0, rect.top() + 7.0),
                Vec2::new(3.0, (rect.height() - 14.0).max(8.0)),
            );
            painter.rect_filled(stripe, CornerRadius::same(2), accent_col);

            let icon_rect = Rect::from_center_size(
                Pos2::new(rect.left() + 22.0, rect.center().y),
                Vec2::splat(16.0),
            );
            paint_vector_icon(&painter, icon_rect, toast.icon, accent_col);

            let max_text_w = (rect.right() - (rect.left() + 40.0) - 12.0).max(40.0);
            let text_col = tokens.text.gamma_multiply(alpha);
            let galley = painter.layout(
                toast.message.clone(),
                egui::FontId::proportional(12.5),
                text_col,
                max_text_w,
            );
            let text_y = rect.center().y - (galley.size().y.min(rect.height() - 6.0)) * 0.5;
            let text_painter = painter.with_clip_rect(rect.shrink2(Vec2::new(8.0, 3.0)));
            text_painter.galley(Pos2::new(rect.left() + 38.0, text_y), galley, text_col);
        }

        if animating_now {
            mark_animation_active(ctx);
        } else if let Some(wake) = next_wake {
            ctx.request_repaint_after(wake);
        }
    }
}
