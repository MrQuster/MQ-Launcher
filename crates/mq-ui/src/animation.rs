//! Unified animation engine with spring physics, easing curves, smart repaint
//! scheduling, tab transitions, micro-interactions, and delayed-close modal helpers.
//!
//! Strictly follows MQ Launcher's zero-emoji policy.

use std::hash::{Hash, Hasher};

use egui::emath::TSTransform;
use egui::{
    Color32, Context, CornerRadius, Frame, Id, Key, LayerId, Order, Pos2, Rect, Response, Sense,
    Shape, Stroke, Ui, Vec2,
};

const TRACKER_ID: &str = "mq_anim_frame_tracker";
const TRANSFORM_STACK_ID: &str = "mq_active_visual_transform";

/// Evaluates a cubic ease-out curve (`1 - (1 - t)^3`) on `t` clamped to `[0.0, 1.0]`.
#[inline]
pub fn ease_out_cubic(t: f32) -> f32 {
    let x = t.clamp(0.0, 1.0);
    1.0 - (1.0 - x).powi(3)
}

/// Evaluates an exponential ease-out curve (`1 - 2^(-10 * t)`) on `t` clamped to `[0.0, 1.0]`.
#[inline]
pub fn ease_out_expo(t: f32) -> f32 {
    let x = t.clamp(0.0, 1.0);
    if x >= 1.0 {
        1.0
    } else if x <= 0.0 {
        0.0
    } else {
        1.0 - 2.0_f32.powf(-10.0 * x)
    }
}

/// Evaluates a symmetric cubic ease-in-out curve on `t` clamped to `[0.0, 1.0]`.
#[inline]
pub fn ease_in_out_cubic(t: f32) -> f32 {
    let x = t.clamp(0.0, 1.0);
    if x < 0.5 {
        4.0 * x * x * x
    } else {
        1.0 - (-2.0 * x + 2.0).powi(3) * 0.5
    }
}

/// Evaluates a quadratic ease-out curve (`1 - (1 - t)^2`) on `t` clamped to `[0.0, 1.0]`.
#[inline]
pub fn ease_out_quad(t: f32) -> f32 {
    let x = t.clamp(0.0, 1.0);
    1.0 - (1.0 - x) * (1.0 - x)
}

#[derive(Clone, Copy, Default)]
struct FrameAnimTracker {
    pass_nr: u64,
    active_count: usize,
}

/// Returns `true` when animations are disabled via `style.animation_time <= 0.0001`
/// or `MQ_TEST_DETERMINISTIC`.
#[inline]
pub fn animations_disabled(ctx: &Context) -> bool {
    if ctx.style().animation_time <= 0.0001 {
        return true;
    }
    std::env::var("MQ_TEST_DETERMINISTIC")
        .is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

/// Resets per-frame active animation tracking for the current pass.
pub fn begin_animation_frame(ctx: &Context) {
    let pass_nr = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        d.insert_temp(
            Id::new(TRACKER_ID),
            FrameAnimTracker {
                pass_nr,
                active_count: 0,
            },
        );
    });
}

/// Marks at least one animation as still running during the current pass and
/// schedules an immediate repaint so frames are requested only while moving.
pub fn mark_animation_active(ctx: &Context) {
    if animations_disabled(ctx) {
        return;
    }
    let pass_nr = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let mut tracker = d
            .get_temp::<FrameAnimTracker>(Id::new(TRACKER_ID))
            .unwrap_or_default();
        if tracker.pass_nr != pass_nr {
            tracker.pass_nr = pass_nr;
            tracker.active_count = 0;
        }
        tracker.active_count = tracker.active_count.saturating_add(1);
        d.insert_temp(Id::new(TRACKER_ID), tracker);
    });
    ctx.request_repaint();
}

/// Number of active animations recorded during the current pass.
pub fn active_animation_count(ctx: &Context) -> usize {
    let pass_nr = ctx.cumulative_pass_nr();
    ctx.data(|d| {
        d.get_temp::<FrameAnimTracker>(Id::new(TRACKER_ID))
            .filter(|t| t.pass_nr == pass_nr)
            .map_or(0, |t| t.active_count)
    })
}

/// Returns `true` if any animation has requested a repaint in the current pass.
#[inline]
pub fn is_any_animation_active(ctx: &Context) -> bool {
    active_animation_count(ctx) > 0
}

/// Returns a clamped delta-time safe for reactive mode (waking from idle sleep).
#[inline]
pub fn frame_dt(ctx: &Context) -> f32 {
    let raw = ctx.input(|i| i.stable_dt);
    if raw.is_finite() {
        raw.clamp(1.0 / 240.0, 0.05)
    } else {
        1.0 / 60.0
    }
}

/// Damped spring parameters controlling stiffness, damping, and resting threshold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringConfig {
    pub stiffness: f32,
    pub damping: f32,
    pub epsilon: f32,
}

impl Default for SpringConfig {
    fn default() -> Self {
        Self::snappy()
    }
}

impl SpringConfig {
    /// Crisp, responsive spring for tabs, buttons, toggles, and filter chips.
    #[inline]
    pub const fn snappy() -> Self {
        Self {
            stiffness: 420.0,
            damping: 32.0,
            epsilon: 0.0015,
        }
    }

    /// Smooth, gentle spring for progress bars, cards, list rows, and toast stacking.
    #[inline]
    pub const fn smooth() -> Self {
        Self {
            stiffness: 260.0,
            damping: 26.0,
            epsilon: 0.0015,
        }
    }

    /// Balanced spring for modal dialogs and dropdown menus.
    #[inline]
    pub const fn modal() -> Self {
        Self {
            stiffness: 340.0,
            damping: 28.0,
            epsilon: 0.002,
        }
    }

    /// Pixel-space spring tuned for sliding tab indicator `Rect` geometry.
    #[inline]
    pub const fn rect() -> Self {
        Self {
            stiffness: 380.0,
            damping: 30.0,
            epsilon: 0.25,
        }
    }

    #[inline]
    pub const fn with_stiffness(mut self, stiffness: f32) -> Self {
        self.stiffness = stiffness;
        self
    }

    #[inline]
    pub const fn with_damping(mut self, damping: f32) -> Self {
        self.damping = damping;
        self
    }

    #[inline]
    pub const fn with_epsilon(mut self, epsilon: f32) -> Self {
        self.epsilon = epsilon;
        self
    }
}

/// Advances a 1D damped harmonic oscillator over `dt` seconds with 4 sub-steps.
/// Returns `true` while motion remains above `cfg.epsilon`, or `false` once snapped at rest.
pub fn step_spring_1d(
    value: &mut f32,
    velocity: &mut f32,
    target: f32,
    dt: f32,
    cfg: SpringConfig,
) -> bool {
    if !value.is_finite() || !velocity.is_finite() || !target.is_finite() {
        *value = if target.is_finite() { target } else { 0.0 };
        *velocity = 0.0;
        return false;
    }

    let eps = cfg.epsilon.max(1e-5);
    if (*value - target).abs() <= eps && velocity.abs() <= eps * 5.0 {
        *value = target;
        *velocity = 0.0;
        return false;
    }

    let sub_dt = dt.clamp(1.0 / 480.0, 0.05) * 0.25;
    for _ in 0..4 {
        let accel = -cfg.stiffness * (*value - target) - cfg.damping * *velocity;
        *velocity += accel * sub_dt;
        *value += *velocity * sub_dt;
    }

    if !value.is_finite()
        || !velocity.is_finite()
        || ((*value - target).abs() <= eps && velocity.abs() <= eps * 5.0)
    {
        *value = target;
        *velocity = 0.0;
        false
    } else {
        true
    }
}

#[derive(Clone, Copy)]
struct SpringF32State {
    value: f32,
    velocity: f32,
    last_pass: u64,
}

/// Smoothly animates a scalar `f32` toward `target` using damped spring physics stored in [`egui::Memory`].
/// Initializes cleanly at rest on first insertion and requests repaints only while moving.
pub fn animate_spring_f32(ctx: &Context, id: Id, target: f32, cfg: SpringConfig) -> f32 {
    let target = if target.is_finite() { target } else { 0.0 };
    let pass_nr = ctx.cumulative_pass_nr();
    if animations_disabled(ctx) {
        ctx.data_mut(|d| {
            d.insert_temp(
                id,
                SpringF32State {
                    value: target,
                    velocity: 0.0,
                    last_pass: pass_nr,
                },
            );
        });
        return target;
    }

    let dt = frame_dt(ctx);
    let mut still_moving = false;
    let out = ctx.data_mut(|d| {
        if let Some(mut st) = d.get_temp::<SpringF32State>(id) {
            if st.last_pass != pass_nr {
                st.last_pass = pass_nr;
                still_moving = step_spring_1d(&mut st.value, &mut st.velocity, target, dt, cfg);
                d.insert_temp(id, st);
            } else {
                still_moving = (st.value - target).abs() > cfg.epsilon.max(1e-5)
                    || st.velocity.abs() > cfg.epsilon.max(1e-5) * 5.0;
            }
            st.value
        } else {
            d.insert_temp(
                id,
                SpringF32State {
                    value: target,
                    velocity: 0.0,
                    last_pass: pass_nr,
                },
            );
            target
        }
    });

    if still_moving {
        mark_animation_active(ctx);
    }
    out
}

/// Convenience wrapper around [`animate_spring_f32`] for boolean targets (`0.0..=1.0`).
#[inline]
pub fn animate_spring_bool(ctx: &Context, id: Id, target: bool, cfg: SpringConfig) -> f32 {
    animate_spring_f32(ctx, id, if target { 1.0 } else { 0.0 }, cfg).clamp(0.0, 1.0)
}

#[derive(Clone, Copy)]
struct SpringVec2State {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    last_pass: u64,
}

/// Smoothly animates a [`Vec2`] toward `target` using independent X/Y springs in [`egui::Memory`].
pub fn animate_spring_vec2(ctx: &Context, id: Id, target: Vec2, cfg: SpringConfig) -> Vec2 {
    let tx = if target.x.is_finite() { target.x } else { 0.0 };
    let ty = if target.y.is_finite() { target.y } else { 0.0 };
    let pass_nr = ctx.cumulative_pass_nr();
    if animations_disabled(ctx) {
        ctx.data_mut(|d| {
            d.insert_temp(
                id,
                SpringVec2State {
                    x: tx,
                    y: ty,
                    vx: 0.0,
                    vy: 0.0,
                    last_pass: pass_nr,
                },
            );
        });
        return Vec2::new(tx, ty);
    }

    let dt = frame_dt(ctx);
    let mut still_moving = false;
    let out = ctx.data_mut(|d| {
        if let Some(mut st) = d.get_temp::<SpringVec2State>(id) {
            if st.last_pass != pass_nr {
                st.last_pass = pass_nr;
                let mx = step_spring_1d(&mut st.x, &mut st.vx, tx, dt, cfg);
                let my = step_spring_1d(&mut st.y, &mut st.vy, ty, dt, cfg);
                still_moving = mx || my;
                d.insert_temp(id, st);
            } else {
                let eps = cfg.epsilon.max(1e-5);
                still_moving = (st.x - tx).abs() > eps || (st.y - ty).abs() > eps;
            }
            Vec2::new(st.x, st.y)
        } else {
            d.insert_temp(
                id,
                SpringVec2State {
                    x: tx,
                    y: ty,
                    vx: 0.0,
                    vy: 0.0,
                    last_pass: pass_nr,
                },
            );
            Vec2::new(tx, ty)
        }
    });

    if still_moving {
        mark_animation_active(ctx);
    }
    out
}

#[derive(Clone, Copy)]
struct SpringRectState {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    v_min_x: f32,
    v_min_y: f32,
    v_max_x: f32,
    v_max_y: f32,
    last_pass: u64,
}

/// Smoothly animates a [`Rect`] (position and size) toward `target` using spring physics.
/// Useful for sliding navigation pills, underline indicators, and segmented switches.
pub fn animate_spring_rect(ctx: &Context, id: Id, target: Rect, cfg: SpringConfig) -> Rect {
    if !target.is_positive() {
        return target;
    }
    let pass_nr = ctx.cumulative_pass_nr();
    if animations_disabled(ctx) {
        ctx.data_mut(|d| {
            d.insert_temp(
                id,
                SpringRectState {
                    min_x: target.min.x,
                    min_y: target.min.y,
                    max_x: target.max.x,
                    max_y: target.max.y,
                    v_min_x: 0.0,
                    v_min_y: 0.0,
                    v_max_x: 0.0,
                    v_max_y: 0.0,
                    last_pass: pass_nr,
                },
            );
        });
        return target;
    }

    let dt = frame_dt(ctx);
    let mut still_moving = false;
    let out = ctx.data_mut(|d| {
        if let Some(mut st) = d.get_temp::<SpringRectState>(id) {
            // If window resized or layout teleported far away (> 900px), treat as resize jump if needed,
            // otherwise glide smoothly between items.
            let jump_dist = (st.min_x - target.min.x)
                .abs()
                .max((st.min_y - target.min.y).abs());
            if jump_dist > 1400.0 {
                st = SpringRectState {
                    min_x: target.min.x,
                    min_y: target.min.y,
                    max_x: target.max.x,
                    max_y: target.max.y,
                    v_min_x: 0.0,
                    v_min_y: 0.0,
                    v_max_x: 0.0,
                    v_max_y: 0.0,
                    last_pass: pass_nr,
                };
                d.insert_temp(id, st);
                return target;
            }

            if st.last_pass != pass_nr {
                st.last_pass = pass_nr;
                let m0 = step_spring_1d(&mut st.min_x, &mut st.v_min_x, target.min.x, dt, cfg);
                let m1 = step_spring_1d(&mut st.min_y, &mut st.v_min_y, target.min.y, dt, cfg);
                let m2 = step_spring_1d(&mut st.max_x, &mut st.v_max_x, target.max.x, dt, cfg);
                let m3 = step_spring_1d(&mut st.max_y, &mut st.v_max_y, target.max.y, dt, cfg);
                still_moving = m0 || m1 || m2 || m3;
                d.insert_temp(id, st);
            } else {
                let eps = cfg.epsilon.max(1e-4);
                still_moving = (st.min_x - target.min.x).abs() > eps
                    || (st.min_y - target.min.y).abs() > eps
                    || (st.max_x - target.max.x).abs() > eps
                    || (st.max_y - target.max.y).abs() > eps;
            }
            Rect::from_min_max(
                Pos2::new(st.min_x, st.min_y),
                Pos2::new(st.max_x.max(st.min_x + 1.0), st.max_y.max(st.min_y + 1.0)),
            )
        } else {
            d.insert_temp(
                id,
                SpringRectState {
                    min_x: target.min.x,
                    min_y: target.min.y,
                    max_x: target.max.x,
                    max_y: target.max.y,
                    v_min_x: 0.0,
                    v_min_y: 0.0,
                    v_max_x: 0.0,
                    v_max_y: 0.0,
                    last_pass: pass_nr,
                },
            );
            target
        }
    });

    if still_moving {
        mark_animation_active(ctx);
    }
    out
}

#[derive(Clone, Copy)]
struct EasedF32State {
    from: f32,
    to: f32,
    current: f32,
    progress: f32,
    last_pass: u64,
}

/// Smoothly interpolates an `f32` toward `target` over `duration_secs` using `easing`.
pub fn animate_eased_f32(
    ctx: &Context,
    id: Id,
    target: f32,
    duration_secs: f32,
    easing: fn(f32) -> f32,
) -> f32 {
    let target = if target.is_finite() { target } else { 0.0 };
    let pass_nr = ctx.cumulative_pass_nr();
    if animations_disabled(ctx) || duration_secs <= 0.001 {
        ctx.data_mut(|d| {
            d.insert_temp(
                id,
                EasedF32State {
                    from: target,
                    to: target,
                    current: target,
                    progress: 1.0,
                    last_pass: pass_nr,
                },
            );
        });
        return target;
    }

    let dt = frame_dt(ctx);
    let mut still_moving = false;
    let out = ctx.data_mut(|d| {
        if let Some(mut st) = d.get_temp::<EasedF32State>(id) {
            if (st.to - target).abs() > 1e-4 {
                st.from = st.current;
                st.to = target;
                st.progress = 0.0;
            }
            if st.last_pass != pass_nr && st.progress < 1.0 {
                st.last_pass = pass_nr;
                st.progress = (st.progress + dt / duration_secs.max(0.01)).min(1.0);
                let k = easing(st.progress).clamp(0.0, 1.0);
                st.current = st.from + (st.to - st.from) * k;
                if (st.progress - 1.0).abs() <= 1e-4 {
                    st.progress = 1.0;
                    st.current = st.to;
                }
                d.insert_temp(id, st);
            } else if st.last_pass != pass_nr {
                st.last_pass = pass_nr;
                d.insert_temp(id, st);
            }
            still_moving = st.progress < 1.0 - 1e-4;
            st.current
        } else {
            d.insert_temp(
                id,
                EasedF32State {
                    from: target,
                    to: target,
                    current: target,
                    progress: 1.0,
                    last_pass: pass_nr,
                },
            );
            target
        }
    });

    if still_moving {
        mark_animation_active(ctx);
    }
    out
}

/// Convenience wrapper around [`animate_eased_f32`] for boolean targets (`0.0..=1.0`).
#[inline]
pub fn animate_eased_bool(
    ctx: &Context,
    id: Id,
    target: bool,
    duration_secs: f32,
    easing: fn(f32) -> f32,
) -> f32 {
    animate_eased_f32(
        ctx,
        id,
        if target { 1.0 } else { 0.0 },
        duration_secs,
        easing,
    )
    .clamp(0.0, 1.0)
}

#[derive(Clone, Copy)]
struct ActiveVisualTransform {
    transform: TSTransform,
    opacity: f32,
}

/// Pushes an active layer-level visual transform + opacity onto the context stack so
/// custom `wgpu` Liquid Glass shaders can align cleanly with animated containers.
pub fn push_visual_transform(ctx: &Context, transform: TSTransform, opacity: f32) {
    ctx.data_mut(|d| {
        let mut stack = d
            .get_temp::<Vec<ActiveVisualTransform>>(Id::new(TRANSFORM_STACK_ID))
            .unwrap_or_default();
        let (prev_xform, prev_op) = stack
            .last()
            .copied()
            .map_or((TSTransform::IDENTITY, 1.0), |a| (a.transform, a.opacity));
        stack.push(ActiveVisualTransform {
            transform: prev_xform * transform,
            opacity: (prev_op * opacity).clamp(0.0, 1.0),
        });
        d.insert_temp(Id::new(TRANSFORM_STACK_ID), stack);
    });
}

/// Pops the most recent visual transform pushed by [`push_visual_transform`].
pub fn pop_visual_transform(ctx: &Context) {
    ctx.data_mut(|d| {
        if let Some(mut stack) =
            d.get_temp::<Vec<ActiveVisualTransform>>(Id::new(TRANSFORM_STACK_ID))
        {
            stack.pop();
            d.insert_temp(Id::new(TRANSFORM_STACK_ID), stack);
        }
    });
}

/// Returns the combined active `(TSTransform, opacity)` for the current UI scope.
pub fn current_visual_transform(ctx: &Context) -> (TSTransform, f32) {
    ctx.data(|d| {
        d.get_temp::<Vec<ActiveVisualTransform>>(Id::new(TRANSFORM_STACK_ID))
            .and_then(|s| s.last().copied())
            .map_or((TSTransform::IDENTITY, 1.0), |a| (a.transform, a.opacity))
    })
}

fn compute_key_hash<K: Hash>(key: &K) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone, Copy)]
struct TabTransitionState {
    last_key_hash: u64,
    last_order: i32,
    progress: f32,
    direction: f32,
    last_pass: u64,
}

/// Animates tab or sub-screen transitions smoothly on key change:
/// fade-in (`0.20 -> 1.0`) combined with a `14 px` directional slide and subtle `0.985 -> 1.0` scale,
/// without mutating layout geometry (zero layout jitter).
pub fn render_tab_transition<K: Hash + Eq + Copy, R>(
    ui: &mut Ui,
    id_source: impl Hash,
    current_key: K,
    order_index: i32,
    horizontal: bool,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    let id = Id::new(("mq_tab_trans", id_source));
    let key_hash = compute_key_hash(&current_key);
    let pass_nr = ui.ctx().cumulative_pass_nr();
    let disabled = animations_disabled(ui.ctx());
    let dt = frame_dt(ui.ctx());

    let mut progress = 1.0_f32;
    let mut direction = 1.0_f32;
    let mut animating = false;

    ui.ctx().data_mut(|d| {
        if let Some(mut st) = d.get_temp::<TabTransitionState>(id) {
            if key_hash != st.last_key_hash {
                st.direction = if order_index > st.last_order {
                    1.0
                } else if order_index < st.last_order {
                    -1.0
                } else {
                    1.0
                };
                st.last_key_hash = key_hash;
                st.last_order = order_index;
                st.progress = if disabled { 1.0 } else { 0.08 };
                st.last_pass = pass_nr;
            } else if st.last_pass != pass_nr && st.progress < 1.0 - 0.001 {
                st.last_pass = pass_nr;
                if disabled {
                    st.progress = 1.0;
                } else {
                    st.progress = (st.progress + dt / 0.18).min(1.0);
                }
            }
            if st.progress >= 1.0 - 0.001 {
                st.progress = 1.0;
            } else {
                animating = true;
            }
            progress = st.progress;
            direction = st.direction;
            d.insert_temp(id, st);
        } else {
            d.insert_temp(
                id,
                TabTransitionState {
                    last_key_hash: key_hash,
                    last_order: order_index,
                    progress: 1.0,
                    direction: 1.0,
                    last_pass: pass_nr,
                },
            );
        }
    });

    if animating {
        mark_animation_active(ui.ctx());
    }

    if progress >= 0.999 || disabled {
        return add_contents(ui);
    }

    let t_cubic = ease_out_cubic(progress);
    let t_expo = ease_out_expo(progress);
    let alpha = (0.20 + 0.80 * t_cubic).clamp(0.0, 1.0);
    let offset_px = (1.0 - t_expo) * 14.0 * direction;
    let scale = 0.985 + 0.015 * t_expo;

    let center = ui.available_rect_before_wrap().center();
    let delta = if horizontal {
        Vec2::new(offset_px, 0.0)
    } else {
        Vec2::new(0.0, offset_px)
    };
    let transform = TSTransform {
        scaling: scale,
        translation: center.to_vec2() * (1.0 - scale) + delta,
    };

    let layer_id = ui.layer_id();
    let start_idx = ui.painter().add(Shape::Noop);
    push_visual_transform(ui.ctx(), transform, alpha);
    let res = ui.scope(|ui| {
        ui.multiply_opacity(alpha);
        add_contents(ui)
    });
    pop_visual_transform(ui.ctx());
    let end_idx = ui.painter().add(Shape::Noop);

    ui.ctx().graphics_mut(|g| {
        g.entry(layer_id)
            .transform_range(start_idx, end_idx, transform);
    });

    res.inner
}

/// Interactive hover/press micro-animation outputs without altering layout boxes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InteractionAnim {
    pub hover_t: f32,
    pub press_t: f32,
    pub lift_y: f32,
    pub scale: f32,
}

impl InteractionAnim {
    /// Transforms a visual button/pill `Rect` with subtle hover lift and tactile click squeeze.
    #[inline]
    pub fn transform_rect(&self, rect: Rect) -> Rect {
        if !rect.is_positive() {
            return rect;
        }
        let s = self.scale.clamp(0.90, 1.05);
        if (s - 1.0).abs() < 1e-4 && self.lift_y.abs() < 1e-3 {
            return rect;
        }
        let center = rect.center();
        let half = rect.size() * (0.5 * s);
        Rect::from_center_size(Pos2::new(center.x, center.y + self.lift_y), half * 2.0)
    }

    /// Transforms a visual card `Rect` with softer lift and gentler press compression.
    #[inline]
    pub fn transform_card_rect(&self, rect: Rect) -> Rect {
        if !rect.is_positive() {
            return rect;
        }
        let s = (1.0 - 0.012 * self.press_t).clamp(0.95, 1.02);
        let lift = -1.5 * self.hover_t * (1.0 - self.press_t);
        if (s - 1.0).abs() < 1e-4 && lift.abs() < 1e-3 {
            return rect;
        }
        let center = rect.center();
        let half = rect.size() * (0.5 * s);
        Rect::from_center_size(Pos2::new(center.x, center.y + lift), half * 2.0)
    }
}

/// Computes spring-driven hover and tactile press micro-animation parameters for any widget.
pub fn animate_interaction(ctx: &Context, id: Id, hovered: bool, pressed: bool) -> InteractionAnim {
    let hover_t = animate_spring_bool(
        ctx,
        id.with("ihov"),
        hovered || pressed,
        SpringConfig::snappy(),
    );
    let press_t = animate_spring_bool(
        ctx,
        id.with("iprs"),
        pressed,
        SpringConfig::snappy()
            .with_stiffness(580.0)
            .with_damping(36.0),
    );
    let lift_y = -1.2 * hover_t * (1.0 - press_t) + 0.5 * press_t;
    let scale = 1.0 + 0.006 * hover_t * (1.0 - press_t) - 0.028 * press_t;
    InteractionAnim {
        hover_t,
        press_t,
        lift_y,
        scale,
    }
}

/// Current animated presence parameters of a modal dialog or dropdown popup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModalAnimState {
    pub should_render: bool,
    pub t: f32,
    pub alpha: f32,
    pub scale: f32,
    pub offset_y: f32,
    pub backdrop_alpha: f32,
    pub is_closing: bool,
}

#[derive(Clone, Copy)]
struct ModalPresenceEntry {
    value: f32,
    velocity: f32,
    was_open: bool,
    last_pass: u64,
}

/// Drives spring open/close transitions for modals and popups so closing does not vanish abruptly.
/// Keep calling every frame (even after `is_open` turns `false`) until `should_render == false`.
pub fn animate_modal_presence(ctx: &Context, id: Id, is_open: bool) -> ModalAnimState {
    let disabled = animations_disabled(ctx);
    let pass_nr = ctx.cumulative_pass_nr();
    let dt = frame_dt(ctx);

    if disabled {
        let val = if is_open { 1.0 } else { 0.0 };
        ctx.data_mut(|d| {
            d.insert_temp(
                id,
                ModalPresenceEntry {
                    value: val,
                    velocity: 0.0,
                    was_open: is_open,
                    last_pass: pass_nr,
                },
            );
        });
        return ModalAnimState {
            should_render: is_open,
            t: val,
            alpha: val,
            scale: 1.0,
            offset_y: 0.0,
            backdrop_alpha: if is_open { 0.60 } else { 0.0 },
            is_closing: false,
        };
    }

    let mut still_moving = false;
    let entry = ctx.data_mut(|d| {
        let mut st = d
            .get_temp::<ModalPresenceEntry>(id)
            .unwrap_or_else(|| ModalPresenceEntry {
                value: if is_open { 0.06 } else { 0.0 },
                velocity: 0.0,
                was_open: is_open,
                last_pass: pass_nr.saturating_sub(1),
            });

        if is_open && !st.was_open {
            st.value = st.value.max(0.06);
        }
        st.was_open = is_open;

        let target = if is_open { 1.0 } else { 0.0 };
        if st.last_pass != pass_nr {
            st.last_pass = pass_nr;
            still_moving = step_spring_1d(
                &mut st.value,
                &mut st.velocity,
                target,
                dt,
                SpringConfig::modal(),
            );
            st.value = st.value.clamp(0.0, 1.0);
            if !is_open && st.value <= 0.002 {
                st.value = 0.0;
                st.velocity = 0.0;
                still_moving = false;
            }
            d.insert_temp(id, st);
        } else {
            still_moving = (st.value - target).abs() > 0.002 || st.velocity.abs() > 0.01;
        }
        st
    });

    if still_moving {
        mark_animation_active(ctx);
    }

    let should_render = is_open || entry.value > 0.002;
    let eased = ease_out_cubic(entry.value.clamp(0.0, 1.0));
    ModalAnimState {
        should_render,
        t: entry.value,
        alpha: eased,
        scale: 0.95 + 0.05 * eased,
        offset_y: (1.0 - eased) * 8.0,
        backdrop_alpha: 0.60 * eased,
        is_closing: !is_open && should_render,
    }
}

/// Caches `Option<T>` dialog payload inside [`egui::Memory`] across the closing fade-out animation.
pub fn animate_modal_cached<T: Clone + Send + Sync + 'static>(
    ctx: &Context,
    id: Id,
    current: Option<T>,
) -> Option<(T, ModalAnimState)> {
    let cache_id = id.with("mq_modal_payload_cache");
    let is_open = current.is_some();
    if let Some(ref val) = current {
        let cloned = val.clone();
        ctx.data_mut(|d| d.insert_temp(cache_id, Some(cloned)));
    }
    let anim = animate_modal_presence(ctx, id, is_open);
    if !anim.should_render {
        ctx.data_mut(|d| d.insert_temp::<Option<T>>(cache_id, None));
        return None;
    }
    let payload = current.or_else(|| ctx.data(|d| d.get_temp::<Option<T>>(cache_id).flatten()))?;
    Some((payload, anim))
}

pub fn adjust_paint_list_opacity(
    ctx: &Context,
    layer_id: LayerId,
    start_idx: usize,
    end_idx: usize,
    alpha: f32,
) {
    if alpha >= 0.999 {
        return;
    }
    let alpha = alpha.clamp(0.0, 1.0);
    ctx.graphics_mut(|g| {
        let list = g.entry(layer_id);
        let limit = end_idx.min(list.next_idx().0);
        for i in start_idx..limit {
            list.mutate_shape(egui::layers::ShapeIdx(i), |cs| {
                egui::epaint::shape_transform::adjust_colors(&mut cs.shape, move |color| {
                    if *color != Color32::PLACEHOLDER {
                        *color = color.gamma_multiply(alpha);
                    }
                });
            });
        }
    });
}

/// Renders an [`egui::Window`] modal with synchronized dimmed backdrop, scale (`0.95 -> 1.0`),
/// vertical settle, and fade-in / delayed fade-out.
pub fn show_animated_modal_window<R>(
    ctx: &Context,
    id: Id,
    anim: &ModalAnimState,
    window: egui::Window<'_>,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    if !anim.should_render {
        return None;
    }

    let screen_rect = ctx.screen_rect();
    let backdrop_alpha_u8 = (anim.backdrop_alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    if backdrop_alpha_u8 > 0 {
        egui::Area::new(id.with("mq_modal_backdrop"))
            .fixed_pos(screen_rect.min)
            .order(Order::Middle)
            .interactable(!anim.is_closing)
            .show(ctx, |ui| {
                let sense = if anim.is_closing {
                    Sense::hover()
                } else {
                    Sense::click()
                };
                let (rect, _) = ui.allocate_exact_size(screen_rect.size(), sense);
                ui.painter()
                    .rect_filled(rect, 0.0, Color32::from_black_alpha(backdrop_alpha_u8));
            });
    }

    let order = Order::Foreground;
    let predicted_center = ctx
        .memory(|m| m.area_rect(id))
        .map_or_else(|| screen_rect.center(), |r| r.center());
    let pre_transform = TSTransform {
        scaling: anim.scale,
        translation: predicted_center.to_vec2() * (1.0 - anim.scale)
            + Vec2::new(0.0, anim.offset_y),
    };

    let start_idx = ctx.graphics(|g| g.get(LayerId::new(order, id)).map_or(0, |l| l.next_idx().0));

    push_visual_transform(ctx, pre_transform, anim.alpha);
    let win_resp = window
        .id(id)
        .order(order)
        .fade_in(false)
        .fade_out(false)
        .show(ctx, |ui| {
            if anim.is_closing {
                ui.disable();
            }
            add_contents(ui)
        });
    pop_visual_transform(ctx);

    let inner = win_resp?;

    let layer_id = inner.response.layer_id;
    let end_idx = ctx.graphics(|g| g.get(layer_id).map_or(0, |l| l.next_idx().0));
    let actual_center = inner.response.rect.center();
    let actual_transform = TSTransform {
        scaling: anim.scale,
        translation: actual_center.to_vec2() * (1.0 - anim.scale) + Vec2::new(0.0, anim.offset_y),
    };

    if anim.alpha < 0.999 {
        adjust_paint_list_opacity(ctx, layer_id, start_idx, end_idx, anim.alpha);
    }
    if actual_transform != TSTransform::IDENTITY && end_idx > start_idx {
        ctx.graphics_mut(|g| {
            g.entry(layer_id).transform_range(
                egui::layers::ShapeIdx(start_idx),
                egui::layers::ShapeIdx(end_idx),
                actual_transform,
            );
        });
    }

    inner.inner
}

/// Renders an animated dropdown menu anchored above or below `widget_response`,
/// keeping it mounted smoothly through fade-in (`0.95 -> 1.0`) and delayed fade-out.
/// Returns the current open progress (`0.0..=1.0`) for driving chevron animation.
pub fn show_animated_dropdown_popup(
    ui: &mut Ui,
    popup_id: Id,
    widget_response: &Response,
    min_width: f32,
    above: bool,
    close_on_click_inside: bool,
    add_contents: impl FnOnce(&mut Ui),
) -> f32 {
    let ctx = ui.ctx().clone();
    let is_open = ctx.memory(|m| m.is_popup_open(popup_id));
    let anim = animate_modal_presence(&ctx, popup_id.with("mq_dropdown_anim"), is_open);
    if !anim.should_render {
        return 0.0;
    }

    let tokens = crate::theme::Tokens::active(&ctx);
    let style = crate::theme_styles::ThemeStyleParams::active(&ctx);
    let corner_r = style.control_corner_radius(32.0, CornerRadius::same(8));
    let screen = ctx.screen_rect();
    let width = min_width
        .max(widget_response.rect.width())
        .clamp(120.0, (screen.width() - 16.0).max(120.0));

    let area_id = popup_id.with("mq_dropdown_area");
    let prev_rect = ctx.memory(|m| m.area_rect(area_id));
    let est_h = prev_rect.map_or(140.0, |r| r.height().max(32.0));

    let x = widget_response.rect.left().clamp(
        screen.left() + 8.0,
        (screen.right() - width - 8.0).max(screen.left() + 8.0),
    );
    let y = if above {
        (widget_response.rect.top() - est_h - 4.0).max(screen.top() + 8.0)
    } else {
        let below_y = widget_response.rect.bottom() + 4.0;
        if below_y + est_h > screen.bottom() - 8.0
            && widget_response.rect.top() - est_h - 4.0 >= screen.top() + 8.0
        {
            widget_response.rect.top() - est_h - 4.0
        } else {
            below_y
        }
    };

    let pivot = Pos2::new(x + width * 0.5, if above { y + est_h } else { y });
    let slide_y = (1.0 - anim.alpha) * if above { 6.0 } else { -6.0 };
    let pre_transform = TSTransform {
        scaling: anim.scale,
        translation: pivot.to_vec2() * (1.0 - anim.scale) + Vec2::new(0.0, slide_y),
    };

    let order = Order::Foreground;
    let start_idx = ctx.graphics(|g| {
        g.get(LayerId::new(order, area_id))
            .map_or(0, |l| l.next_idx().0)
    });

    let popup_fill = if style.refraction_border {
        crate::theme_styles::with_alpha(tokens.bg_surface, style.modal_alpha.max(0.88))
    } else {
        tokens.bg_surface
    };

    push_visual_transform(&ctx, pre_transform, anim.alpha);
    let area_resp = egui::Area::new(area_id)
        .order(order)
        .fixed_pos(Pos2::new(x, y))
        .interactable(!anim.is_closing)
        .fade_in(false)
        .show(&ctx, |ui| {
            if anim.is_closing {
                ui.disable();
            }
            let frame = Frame::popup(ui.style())
                .fill(popup_fill)
                .stroke(Stroke::new(1.0_f32, tokens.border_strong))
                .corner_radius(corner_r)
                .inner_margin(egui::Margin::same(8));
            frame.show(ui, |ui| {
                ui.set_min_width((min_width - 16.0).max(100.0));
                ui.set_max_height((screen.height() * 0.55).max(160.0));
                add_contents(ui);
            });
        });
    pop_visual_transform(&ctx);

    let layer_id = area_resp.response.layer_id;
    let end_idx = ctx.graphics(|g| g.get(layer_id).map_or(0, |l| l.next_idx().0));
    let actual_rect = area_resp.response.rect;
    let actual_pivot = Pos2::new(
        actual_rect.center().x,
        if above {
            actual_rect.bottom()
        } else {
            actual_rect.top()
        },
    );
    let actual_transform = TSTransform {
        scaling: anim.scale,
        translation: actual_pivot.to_vec2() * (1.0 - anim.scale) + Vec2::new(0.0, slide_y),
    };

    if anim.alpha < 0.999 {
        adjust_paint_list_opacity(&ctx, layer_id, start_idx, end_idx, anim.alpha);
    }
    if actual_transform != TSTransform::IDENTITY && end_idx > start_idx {
        ctx.graphics_mut(|g| {
            g.entry(layer_id).transform_range(
                egui::layers::ShapeIdx(start_idx),
                egui::layers::ShapeIdx(end_idx),
                actual_transform,
            );
        });
    }

    if is_open && !anim.is_closing {
        let esc_pressed = ctx.input(|i| i.key_pressed(Key::Escape));
        let any_click_elsewhere = ctx.input(|i| i.pointer.any_pressed())
            && !widget_response.clicked()
            && !widget_response.contains_pointer()
            && !area_resp.response.contains_pointer();
        let clicked_inside = close_on_click_inside
            && ctx.input(|i| i.pointer.any_released())
            && area_resp.response.contains_pointer();
        if esc_pressed || any_click_elsewhere || clicked_inside {
            ctx.memory_mut(|m| m.close_popup());
        }
    }

    anim.alpha
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{RawInput, Vec2};

    fn make_input(time: f64) -> RawInput {
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1024.0, 720.0))),
            time: Some(time),
            predicted_dt: 1.0 / 60.0,
            ..Default::default()
        }
    }

    #[test]
    fn easing_curves_hit_endpoints_and_are_monotonic() {
        for easing in [
            ease_out_cubic,
            ease_out_expo,
            ease_in_out_cubic,
            ease_out_quad,
        ] {
            assert!((easing(-0.5) - 0.0).abs() < 1e-5);
            assert!((easing(0.0) - 0.0).abs() < 1e-5);
            assert!((easing(1.0) - 1.0).abs() < 1e-5);
            assert!((easing(1.5) - 1.0).abs() < 1e-5);
            let mut prev = 0.0;
            for i in 1..=20 {
                let v = easing(i as f32 / 20.0);
                assert!(v >= prev - 1e-5, "non-monotonic at step {i}: {prev} -> {v}");
                prev = v;
            }
        }
    }

    #[test]
    fn spring_converges_and_stops_requesting_repaints_when_settled() {
        let ctx = Context::default();
        let anim_id = Id::new("test_spring_idle");
        let mut target = 0.0_f32;
        let mut last_val = 0.0_f32;

        // Frame 0: initial insertion starts settled at target = 0.0 -> zero active animations
        let _ = ctx.run(make_input(0.0), |ctx| {
            begin_animation_frame(ctx);
            last_val = animate_spring_f32(ctx, anim_id, target, SpringConfig::snappy());
            assert_eq!(last_val, 0.0);
            assert!(!is_any_animation_active(ctx));
        });

        // Switch target to 1.0 -> activates spring and repaints
        target = 1.0;
        let mut t = 1.0 / 60.0;
        let mut active_frames = 0;
        for _ in 0..90 {
            let _ = ctx.run(make_input(t), |ctx| {
                begin_animation_frame(ctx);
                last_val = animate_spring_f32(ctx, anim_id, target, SpringConfig::snappy());
                if is_any_animation_active(ctx) {
                    active_frames += 1;
                }
            });
            t += 1.0 / 60.0;
        }

        assert!(
            active_frames > 0,
            "spring should animate after target change"
        );
        assert!(
            (last_val - 1.0).abs() < 1e-4,
            "spring should converge to 1.0, got {last_val}"
        );

        // Next idle frame: must have 0 active animations (no idle CPU/GPU wakeups)
        let _ = ctx.run(make_input(t), |ctx| {
            begin_animation_frame(ctx);
            let v = animate_spring_f32(ctx, anim_id, target, SpringConfig::snappy());
            assert_eq!(v, 1.0);
            assert!(!is_any_animation_active(ctx));
        });
    }

    #[test]
    fn spring_rect_interpolates_indicator_position_and_width() {
        let ctx = Context::default();
        let id = Id::new("test_rect_indicator");
        let r1 = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(80.0, 32.0));
        let r2 = Rect::from_min_size(Pos2::new(140.0, 20.0), Vec2::new(120.0, 32.0));
        let mut current = r1;

        let _ = ctx.run(make_input(0.0), |ctx| {
            begin_animation_frame(ctx);
            current = animate_spring_rect(ctx, id, r1, SpringConfig::rect());
            assert_eq!(current, r1);
            assert!(!is_any_animation_active(ctx));
        });

        let _ = ctx.run(make_input(1.0 / 60.0), |ctx| {
            begin_animation_frame(ctx);
            current = animate_spring_rect(ctx, id, r2, SpringConfig::rect());
            assert!(current.left() > r1.left() && current.left() < r2.left());
            assert!(current.width() > r1.width() && current.width() < r2.width());
            assert!(is_any_animation_active(ctx));
        });

        let mut t = 2.0 / 60.0;
        for _ in 0..90 {
            let _ = ctx.run(make_input(t), |ctx| {
                begin_animation_frame(ctx);
                current = animate_spring_rect(ctx, id, r2, SpringConfig::rect());
            });
            t += 1.0 / 60.0;
        }
        assert!((current.left() - r2.left()).abs() < 0.5);
        assert!((current.width() - r2.width()).abs() < 0.5);
    }

    #[test]
    fn modal_presence_delays_unmount_until_close_animation_completes() {
        let ctx = Context::default();
        let modal_id = Id::new("test_modal_presence");
        let mut t = 0.0;

        // Open for several frames until settled
        for _ in 0..45 {
            let _ = ctx.run(make_input(t), |ctx| {
                begin_animation_frame(ctx);
                let anim = animate_modal_presence(ctx, modal_id, true);
                assert!(anim.should_render);
                assert!(!anim.is_closing);
                assert!(anim.scale >= 0.95 && anim.scale <= 1.001);
            });
            t += 1.0 / 60.0;
        }

        // First frame after closing: still mounted cleanly while fading out
        let mut closing_frames = 0;
        let mut vanished = false;
        for _ in 0..90 {
            let _ = ctx.run(make_input(t), |ctx| {
                begin_animation_frame(ctx);
                let anim = animate_modal_presence(ctx, modal_id, false);
                if anim.should_render {
                    assert!(anim.is_closing);
                    assert!(anim.alpha > 0.0);
                    closing_frames += 1;
                } else {
                    vanished = true;
                }
            });
            if vanished {
                break;
            }
            t += 1.0 / 60.0;
        }

        assert!(
            closing_frames > 0,
            "modal should remain rendered during fade-out"
        );
        assert!(vanished, "modal must eventually finish closing and unmount");
    }

    #[test]
    fn tab_transition_tracks_direction_and_settles_to_idle() {
        let ctx = Context::default();
        let mut tab_idx = 0_i32;
        let mut t = 0.0;

        // Initial render at tab 0 is already settled
        let _ = ctx.run(make_input(t), |ctx| {
            begin_animation_frame(ctx);
            egui::CentralPanel::default().show(ctx, |ui| {
                render_tab_transition(ui, "test_tabs", tab_idx, tab_idx, true, |ui| {
                    ui.label("Tab 0");
                });
            });
            assert!(!is_any_animation_active(ctx));
        });

        // Switch to tab 2 -> triggers transition
        tab_idx = 2;
        t += 1.0 / 60.0;
        let _ = ctx.run(make_input(t), |ctx| {
            begin_animation_frame(ctx);
            egui::CentralPanel::default().show(ctx, |ui| {
                render_tab_transition(ui, "test_tabs", tab_idx, tab_idx, true, |ui| {
                    ui.label("Tab 2");
                });
            });
            assert!(is_any_animation_active(ctx));
        });

        // Advance past 0.18s duration -> must settle cleanly
        for _ in 0..20 {
            t += 1.0 / 60.0;
            let _ = ctx.run(make_input(t), |ctx| {
                begin_animation_frame(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    render_tab_transition(ui, "test_tabs", tab_idx, tab_idx, true, |ui| {
                        ui.label("Tab 2");
                    });
                });
            });
        }

        t += 1.0 / 60.0;
        let _ = ctx.run(make_input(t), |ctx| {
            begin_animation_frame(ctx);
            egui::CentralPanel::default().show(ctx, |ui| {
                render_tab_transition(ui, "test_tabs", tab_idx, tab_idx, true, |ui| {
                    ui.label("Tab 2");
                });
            });
            assert!(!is_any_animation_active(ctx));
        });
    }

    #[test]
    fn disabled_animations_never_deadlock_or_panic() {
        let ctx = Context::default();
        ctx.style_mut(|s| s.animation_time = 0.0);

        let rect = Rect::from_min_size(Pos2::new(12.0, 24.0), Vec2::new(120.0, 36.0));
        let vec = Vec2::new(32.0, 48.0);
        let _ = ctx.run(make_input(0.0), |ctx| {
            begin_animation_frame(ctx);
            let f = animate_spring_f32(ctx, Id::new("disabled_f32"), 0.75, SpringConfig::snappy());
            let v = animate_spring_vec2(ctx, Id::new("disabled_vec2"), vec, SpringConfig::snappy());
            let r = animate_spring_rect(ctx, Id::new("disabled_rect"), rect, SpringConfig::rect());
            let e = animate_eased_f32(ctx, Id::new("disabled_eased"), 0.5, 0.25, ease_out_cubic);
            let e_zero =
                animate_eased_f32(ctx, Id::new("disabled_eased_zero"), 0.9, 0.0, ease_out_quad);
            assert!((f - 0.75).abs() < 1e-5);
            assert_eq!(v, vec);
            assert_eq!(r, rect);
            assert!((e - 0.5).abs() < 1e-5);
            assert!((e_zero - 0.9).abs() < 1e-5);
            assert!(!is_any_animation_active(ctx));
        });
    }
}
