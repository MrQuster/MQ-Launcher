//! Theme style parameters (`ThemeStyleParams`) and Apple iOS 26 **Liquid Glass** optical
//! renderer + lightweight **Translucent** renderer.
//!
//! All colors, opacities, full-window aurora wallpaper sampling, convex lens backdrop
//! refraction, dual-rim anisotropic Fresnel specular highlights, inner caustic rims,
//! capsule/continuous-corner geometry, and cursor specular spotlight live here so view
//! and widget code stays clean and decoupled from theme math.

use std::time::Duration;

use egui::epaint::{Mesh, Vertex};
use egui::{
    Color32, Context, CornerRadius, Id, InnerResponse, Painter, Pos2, Rect, Shape, Stroke,
    StrokeKind, Ui,
};
use mq_core::{ThemeConfig, ThemePreset};

use crate::theme::{blend_color, Tokens};

/// Complete visual effect and transparency parameters derived from [`ThemeConfig`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeStyleParams {
    pub preset: ThemePreset,
    /// Enable compositor-level window framebuffer transparency (`clear_color = transparent`).
    pub compositor_transparent: bool,
    /// Opacity factor (`0.35..=1.0`) for main window backdrop tint.
    pub window_alpha: f32,
    /// Opacity factor (`0.35..=1.0`) for left sidebar surface.
    pub sidebar_alpha: f32,
    /// Opacity factor (`0.35..=1.0`) for cards and panels.
    pub surface_alpha: f32,
    /// Opacity factor (`0.35..=1.0`) for bottom HUD bar.
    pub hud_alpha: f32,
    /// Opacity factor (`0.75..=1.0`) for modal dialogs to preserve legibility.
    pub modal_alpha: f32,
    /// Opacity factor (`0.55..=1.0`) for text inputs / recessed fields.
    pub input_alpha: f32,
    /// Simulated backdrop blur / frosted diffusion strength (`0.0..=1.0`).
    pub backdrop_blur_strength: f32,
    /// Draw Apple iOS 26 directional Fresnel refraction borders and optical lens layers.
    pub refraction_border: bool,
    /// Bright top-left grazing specular light along primary Fresnel edge.
    pub refraction_top_left: Color32,
    /// Cool bottom-right counter-refraction exit highlight along opposite Fresnel edge.
    pub refraction_bottom_right: Color32,
    /// Soft upper curved glass dome highlight alpha (`0..=255`).
    pub inner_sheen_alpha: u8,
    /// Draw animated multi-wave aurora wallpaper inside the window behind frosted glass.
    pub animated_ambient_bg: bool,
    pub ambient_lobe_a: Color32,
    pub ambient_lobe_b: Color32,
    pub ambient_lobe_c: Color32,
    /// Paint smooth cursor-following specular glow on buttons, pills, and cards.
    pub cursor_specular_glow: bool,
    pub specular_radius_card: f32,
    pub specular_radius_button: f32,
    pub specular_intensity: f32,
}

impl Default for ThemeStyleParams {
    fn default() -> Self {
        Self::opaque_solid(ThemePreset::MatteDark, 1.0, 1.0)
    }
}

impl ThemeStyleParams {
    fn opaque_solid(preset: ThemePreset, window_opacity: f32, card_opacity: f32) -> Self {
        let win_a = crate::state::safe_clamp_f32(window_opacity, 0.35, 1.0);
        let card_a = crate::state::safe_clamp_f32(card_opacity, 0.35, 1.0);
        Self {
            preset,
            compositor_transparent: win_a < 0.995,
            window_alpha: win_a,
            sidebar_alpha: (win_a + 0.06).min(1.0),
            surface_alpha: card_a,
            hud_alpha: (win_a + 0.08).min(1.0),
            modal_alpha: card_a.max(0.92),
            input_alpha: card_a.max(0.85),
            backdrop_blur_strength: 0.0,
            refraction_border: false,
            refraction_top_left: Color32::TRANSPARENT,
            refraction_bottom_right: Color32::TRANSPARENT,
            inner_sheen_alpha: 0,
            animated_ambient_bg: false,
            ambient_lobe_a: Color32::TRANSPARENT,
            ambient_lobe_b: Color32::TRANSPARENT,
            ambient_lobe_c: Color32::TRANSPARENT,
            cursor_specular_glow: false,
            specular_radius_card: 0.0,
            specular_radius_button: 0.0,
            specular_intensity: 0.0,
        }
    }

    /// Apple iOS 26 **Liquid Glass** optical parameters:
    /// - compositor-level translucency (`window_alpha`),
    /// - convex lens backdrop refraction with chromatic dispersion,
    /// - dual-rim anisotropic Fresnel specular borders + inner caustic rim,
    /// - capsule pill geometry and cursor-following fluid glow.
    pub fn liquid_glass(cfg: &ThemeConfig) -> Self {
        let win_a = crate::state::safe_clamp_f32(cfg.window_opacity, 0.35, 1.0);
        let card_a = crate::state::safe_clamp_f32(cfg.card_opacity, 0.35, 1.0);
        let glow_factor = crate::state::safe_clamp_f32(cfg.border_glow, 0.15, 1.0);
        Self {
            preset: ThemePreset::LiquidGlass,
            compositor_transparent: true,
            window_alpha: win_a,
            sidebar_alpha: (win_a * 0.76).clamp(0.42, 0.88),
            surface_alpha: card_a,
            hud_alpha: (win_a * 0.80).clamp(0.46, 0.90),
            modal_alpha: (card_a + 0.24).clamp(0.84, 0.95),
            input_alpha: (card_a + 0.10).clamp(0.56, 0.86),
            backdrop_blur_strength: 0.92,
            refraction_border: true,
            refraction_top_left: Color32::from_rgba_unmultiplied(
                248,
                252,
                255,
                (198.0 * (0.68 + 0.32 * glow_factor)).round().min(235.0) as u8,
            ),
            refraction_bottom_right: Color32::from_rgba_unmultiplied(
                156,
                214,
                252,
                (138.0 * (0.65 + 0.35 * glow_factor)).round().min(195.0) as u8,
            ),
            inner_sheen_alpha: 34,
            animated_ambient_bg: true,
            ambient_lobe_a: Color32::from_rgb(0x4E, 0x7A, 0xA6),
            ambient_lobe_b: Color32::from_rgb(0x64, 0x62, 0xA2),
            ambient_lobe_c: Color32::from_rgb(0x46, 0x86, 0x90),
            cursor_specular_glow: true,
            specular_radius_card: 210.0,
            specular_radius_button: 120.0,
            specular_intensity: 0.28 + glow_factor * 0.14,
        }
    }

    /// Lightweight Translucent theme: clean compositor window tint without glares or motion.
    pub fn translucent(cfg: &ThemeConfig) -> Self {
        let win_a = crate::state::safe_clamp_f32(cfg.window_opacity, 0.35, 1.0);
        let card_a = crate::state::safe_clamp_f32(cfg.card_opacity, 0.35, 1.0);
        Self {
            preset: ThemePreset::Translucent,
            compositor_transparent: true,
            window_alpha: win_a,
            sidebar_alpha: (win_a + 0.04).clamp(0.45, 0.92),
            surface_alpha: card_a,
            hud_alpha: (win_a + 0.08).clamp(0.50, 0.94),
            modal_alpha: (card_a + 0.18).clamp(0.88, 0.97),
            input_alpha: (card_a + 0.08).clamp(0.70, 0.94),
            backdrop_blur_strength: 0.25,
            refraction_border: false,
            refraction_top_left: Color32::TRANSPARENT,
            refraction_bottom_right: Color32::TRANSPARENT,
            inner_sheen_alpha: 0,
            animated_ambient_bg: false,
            ambient_lobe_a: Color32::TRANSPARENT,
            ambient_lobe_b: Color32::TRANSPARENT,
            ambient_lobe_c: Color32::TRANSPARENT,
            cursor_specular_glow: false,
            specular_radius_card: 0.0,
            specular_radius_button: 0.0,
            specular_intensity: 0.0,
        }
    }

    /// Resolves style parameters from [`ThemeConfig`].
    pub fn from_config(cfg: &ThemeConfig) -> Self {
        match cfg.preset {
            ThemePreset::LiquidGlass => Self::liquid_glass(cfg),
            ThemePreset::Translucent => Self::translucent(cfg),
            other => Self::opaque_solid(other, cfg.window_opacity, cfg.card_opacity),
        }
    }

    /// Reads the active [`ThemeStyleParams`] stored in `ctx` by `apply_theme`.
    pub fn active(ctx: &Context) -> Self {
        ctx.data(|d| d.get_temp::<Self>(Id::new("mq_active_theme_style")))
            .unwrap_or_default()
    }

    /// Continuous iOS 26 card rounding (`16px` minimum in Liquid Glass).
    #[inline]
    pub fn card_corner_radius(&self, base_radius: f32) -> CornerRadius {
        let clamped = if self.refraction_border {
            crate::state::safe_clamp_f32(base_radius.max(16.0), 0.0, 20.0)
        } else {
            crate::state::safe_clamp_f32(base_radius, 0.0, 16.0)
        };
        CornerRadius::same(clamped.round() as u8)
    }

    /// Apple iOS 26 `.capsule` rounding for buttons, nav rows, inputs, and segmented controls.
    #[inline]
    pub fn control_corner_radius(&self, height: f32, fallback: CornerRadius) -> CornerRadius {
        if self.refraction_border {
            let pill = (height.max(24.0) * 0.5).round().clamp(12.0, 24.0) as u8;
            CornerRadius::same(pill)
        } else {
            fallback
        }
    }

    /// Continuous sheet/modal rounding in Liquid Glass.
    #[inline]
    pub fn modal_corner_radius(&self, fallback: CornerRadius) -> CornerRadius {
        if self.refraction_border {
            CornerRadius::same(18)
        } else {
            fallback
        }
    }

    #[inline]
    pub fn window_fill(&self, tokens: &Tokens) -> Color32 {
        with_alpha(tokens.bg_app, self.window_alpha)
    }

    #[inline]
    pub fn sidebar_fill(&self, tokens: &Tokens) -> Color32 {
        with_alpha(tokens.bg_sidebar, self.sidebar_alpha)
    }

    #[inline]
    pub fn surface_fill(&self, tokens: &Tokens) -> Color32 {
        with_alpha(tokens.bg_surface, self.surface_alpha)
    }

    #[inline]
    pub fn surface_hover_fill(&self, tokens: &Tokens) -> Color32 {
        let hover_alpha = (self.surface_alpha + if self.refraction_border { 0.14 } else { 0.07 })
            .clamp(0.35, 1.0);
        with_alpha(tokens.bg_surface_hover, hover_alpha)
    }

    #[inline]
    pub fn input_fill(&self, tokens: &Tokens) -> Color32 {
        with_alpha(tokens.bg_input, self.input_alpha)
    }

    #[inline]
    pub fn hud_fill(&self, tokens: &Tokens) -> Color32 {
        with_alpha(tokens.bg_sidebar, self.hud_alpha)
    }

    #[inline]
    pub fn modal_fill(&self, tokens: &Tokens) -> Color32 {
        with_alpha(tokens.bg_surface, self.modal_alpha)
    }

    /// Computes primary action button fill honoring translucency and hover state (`0.0..=1.0`).
    pub fn primary_button_fill(&self, tokens: &Tokens, hover_t: f32) -> Color32 {
        let t = 0.18 + 0.14 * hover_t.clamp(0.0, 1.0);
        let rgb = blend_color(tokens.bg_surface, tokens.accent, t);
        if self.refraction_border {
            let a = (self.surface_alpha + 0.16 + 0.08 * hover_t).clamp(0.58, 0.94);
            with_alpha(rgb, a)
        } else {
            with_alpha(rgb, self.surface_alpha.max(0.84))
        }
    }

    /// Computes secondary button fill honoring translucency and hover state (`0.0..=1.0`).
    pub fn secondary_button_fill(&self, tokens: &Tokens, hover_t: f32) -> Color32 {
        let rgb = blend_color(
            tokens.bg_surface,
            tokens.bg_surface_hover,
            hover_t.clamp(0.0, 1.0),
        );
        let a = if self.refraction_border {
            (self.surface_alpha + 0.08 * hover_t).clamp(0.48, 0.90)
        } else {
            self.surface_alpha
        };
        with_alpha(rgb, a)
    }
}

#[inline]
pub fn with_alpha(color: Color32, alpha_norm: f32) -> Color32 {
    let a = (crate::state::safe_clamp_f32(alpha_norm, 0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

/// Keeps frosted aurora wallpaper and optical glass tones richly luminous while staying safely
/// within restrained HSV chroma (`sat <= 0.42` for medium/bright regions), matching Apple's
/// frosted skylight aesthetic and preserving visual contrast clarity.
#[inline]
pub fn clamp_glass_chroma(c: Color32) -> Color32 {
    let r = c.r() as f32 / 255.0;
    let g = c.g() as f32 / 255.0;
    let b = c.b() as f32 / 255.0;
    let cmax = r.max(g).max(b);
    let cmin = r.min(g).min(b);
    let delta = cmax - cmin;
    if cmax <= 0.24 || delta <= 1e-4 {
        return c;
    }
    let sat = delta / cmax;
    let max_sat = 0.41_f32;
    if sat <= max_sat {
        return c;
    }
    let scale = max_sat / sat;
    let nr = cmax - (cmax - r) * scale;
    let ng = cmax - (cmax - g) * scale;
    let nb = cmax - (cmax - b) * scale;
    Color32::from_rgba_unmultiplied(
        (nr * 255.0).round().clamp(0.0, 255.0) as u8,
        (ng * 255.0).round().clamp(0.0, 255.0) as u8,
        (nb * 255.0).round().clamp(0.0, 255.0) as u8,
        c.a(),
    )
}

/// Evaluates the continuous multi-wave Apple iOS 26 dark-mode aurora wallpaper at normalized window
/// coordinates `(nx, ny)` in `[0.0, 1.0]` and animation time `time`.
pub fn sample_liquid_glass_wallpaper(
    nx: f32,
    ny: f32,
    time: f32,
    style: &ThemeStyleParams,
    tokens: &Tokens,
) -> Color32 {
    let fx = nx.clamp(0.0, 1.0);
    let fy = ny.clamp(0.0, 1.0);

    // Three slowly drifting organic aurora centers
    let lobe_a = Pos2::new(
        0.22 + 0.14 * (time * 0.26).sin(),
        0.26 + 0.12 * (time * 0.20).cos(),
    );
    let lobe_b = Pos2::new(
        0.78 + 0.12 * (time * 0.21 + 1.4).cos(),
        0.70 + 0.13 * (time * 0.18 + 0.8).sin(),
    );
    let lobe_c = Pos2::new(
        0.50 + 0.17 * (time * 0.16 + 2.5).sin(),
        0.48 + 0.14 * (time * 0.23 + 2.0).cos(),
    );

    let da = ((fx - lobe_a.x).powi(2) + (fy - lobe_a.y).powi(2)).sqrt();
    let db = ((fx - lobe_b.x).powi(2) + (fy - lobe_b.y).powi(2)).sqrt();
    let dc = ((fx - lobe_c.x).powi(2) + (fy - lobe_c.y).powi(2)).sqrt();

    // Sweeping diagonal iOS 26 liquid silk wave across the viewport
    let wave_phase = fx * 3.1 - fy * 2.2 + 0.35 * (time * 0.30).sin();
    let ribbon_a = (0.5 + 0.5 * (wave_phase).sin()).powf(2.0) * 0.26;
    let ribbon_b = (0.5 + 0.5 * (fx * 2.4 + fy * 3.0 + 1.2 - time * 0.22).cos()).powf(2.4) * 0.22;

    let wa = (((1.0 - da / 0.88).max(0.0)).powf(1.6) * 0.44 + ribbon_a).min(0.62);
    let wb = (((1.0 - db / 0.90).max(0.0)).powf(1.6) * 0.44 + ribbon_b).min(0.60);
    let wc = ((1.0 - dc / 0.78).max(0.0)).powf(1.8) * 0.34;

    let deep_base = blend_color(tokens.bg_app, Color32::from_rgb(0x09, 0x10, 0x1D), 0.35);
    let s1 = blend_color(deep_base, style.ambient_lobe_a, wa);
    let s2 = blend_color(s1, style.ambient_lobe_b, wb);
    let s3 = blend_color(s2, style.ambient_lobe_c, wc);

    clamp_glass_chroma(s3)
}

#[inline]
fn frame_wallpaper_time(ctx: &Context, style: &ThemeStyleParams) -> (f32, bool) {
    let deterministic = std::env::var("MQ_TEST_DETERMINISTIC")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        || {
            #[cfg(feature = "test-hooks")]
            {
                crate::test_hooks::is_solid_background(ctx)
            }
            #[cfg(not(feature = "test-hooks"))]
            {
                false
            }
        };
    let t = if deterministic || !style.animated_ambient_bg {
        0.0_f32
    } else {
        ctx.input(|i| i.time) as f32
    };
    (t, deterministic)
}

/// Renders the animated multi-wave aurora glass wallpaper for **Liquid Glass**.
/// When `deterministic` is `true`, freezes animation at `t = 0.0` without scheduling repaints.
pub fn paint_liquid_glass_ambient_backdrop(
    ctx: &Context,
    painter: &Painter,
    rect: Rect,
    style: &ThemeStyleParams,
    tokens: &Tokens,
    deterministic: bool,
) {
    if !rect.is_positive() {
        return;
    }

    let time = if deterministic || !style.animated_ambient_bg {
        0.0_f32
    } else {
        ctx.input(|i| i.time) as f32
    };

    let cols = 16usize;
    let rows = 10usize;
    let mut mesh = Mesh::default();
    let alpha_u8 =
        (crate::state::safe_clamp_f32(style.window_alpha, 0.35, 1.0) * 255.0).round() as u8;

    for r in 0..=rows {
        let fy = (r as f32) / (rows as f32);
        let y = rect.top() + rect.height() * fy;
        for c in 0..=cols {
            let fx = (c as f32) / (cols as f32);
            let x = rect.left() + rect.width() * fx;

            let col = sample_liquid_glass_wallpaper(fx, fy, time, style, tokens);
            mesh.vertices.push(Vertex {
                pos: Pos2::new(x, y),
                uv: Pos2::ZERO,
                color: Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha_u8),
            });
        }
    }

    let stride = (cols + 1) as u32;
    for r in 0..(rows as u32) {
        for c in 0..(cols as u32) {
            let i0 = r * stride + c;
            let i1 = i0 + 1;
            let i2 = i0 + stride;
            let i3 = i2 + 1;
            mesh.add_triangle(i0, i1, i2);
            mesh.add_triangle(i1, i3, i2);
        }
    }
    painter.add(mesh);

    // Soft frosted bokeh diffusion layers over the aurora centers.
    let lobe_a_center = Pos2::new(
        0.22 + 0.14 * (time * 0.26).sin(),
        0.26 + 0.12 * (time * 0.20).cos(),
    );
    let lobe_b_center = Pos2::new(
        0.78 + 0.12 * (time * 0.21 + 1.4).cos(),
        0.70 + 0.13 * (time * 0.18 + 0.8).sin(),
    );
    let lobe_c_center = Pos2::new(
        0.50 + 0.17 * (time * 0.16 + 2.5).sin(),
        0.48 + 0.14 * (time * 0.23 + 2.0).cos(),
    );

    let min_dim = rect.width().min(rect.height()).max(1.0);
    let draw_lobe = |norm: Pos2, raw_col: Color32, radius_frac: f32, max_alpha: f32| {
        let col = clamp_glass_chroma(raw_col);
        let center = Pos2::new(
            rect.left() + rect.width() * norm.x,
            rect.top() + rect.height() * norm.y,
        );
        let base_r = min_dim * radius_frac;
        for &(r_mul, a_mul) in &[(1.0_f32, 0.28_f32), (0.68, 0.42), (0.38, 0.26)] {
            let a = (max_alpha * a_mul * style.backdrop_blur_strength * 255.0).round() as u8;
            if a > 0 {
                painter.circle_filled(
                    center,
                    base_r * r_mul,
                    Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), a),
                );
            }
        }
    };

    draw_lobe(lobe_a_center, style.ambient_lobe_a, 0.34, 0.15);
    draw_lobe(lobe_b_center, style.ambient_lobe_b, 0.38, 0.14);
    draw_lobe(lobe_c_center, style.ambient_lobe_c, 0.28, 0.12);

    if !deterministic && style.animated_ambient_bg {
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

/// Renders a clean, uniform tinted backdrop for the **Translucent** preset (no harsh glares or motion).
pub fn paint_translucent_clean_backdrop(
    painter: &Painter,
    rect: Rect,
    style: &ThemeStyleParams,
    tokens: &Tokens,
) {
    if !rect.is_positive() {
        return;
    }

    let top_col = blend_color(tokens.bg_app, tokens.bg_surface, 0.18);
    let bot_col = blend_color(tokens.bg_app, Color32::BLACK, 0.08);
    let a = (crate::state::safe_clamp_f32(style.window_alpha, 0.35, 1.0) * 255.0).round() as u8;

    let top_rgba = Color32::from_rgba_unmultiplied(top_col.r(), top_col.g(), top_col.b(), a);
    let bot_rgba = Color32::from_rgba_unmultiplied(bot_col.r(), bot_col.g(), bot_col.b(), a);

    let mut mesh = Mesh::default();
    mesh.vertices.push(Vertex {
        pos: rect.left_top(),
        uv: Pos2::ZERO,
        color: top_rgba,
    });
    mesh.vertices.push(Vertex {
        pos: rect.right_top(),
        uv: Pos2::ZERO,
        color: top_rgba,
    });
    mesh.vertices.push(Vertex {
        pos: rect.left_bottom(),
        uv: Pos2::ZERO,
        color: bot_rgba,
    });
    mesh.vertices.push(Vertex {
        pos: rect.right_bottom(),
        uv: Pos2::ZERO,
        color: bot_rgba,
    });
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(mesh);
}

/// Builds a convex-refracted glass surface body mesh for `rect`, sampling the full-window aurora
/// wallpaper at warped coordinates so light appears physically refracted inside each iOS 26 widget.
#[allow(clippy::too_many_arguments)]
fn build_refracted_glass_body_mesh(
    ctx: &Context,
    rect: Rect,
    radius: f32,
    style: &ThemeStyleParams,
    tokens: &Tokens,
    hover_t: f32,
    is_button: bool,
    tint_override: Option<Color32>,
) -> Option<Mesh> {
    if !rect.is_positive() || rect.width() < 6.0 || rect.height() < 6.0 {
        return None;
    }

    let screen = ctx.screen_rect();
    let sw = screen.width().max(640.0);
    let sh = screen.height().max(480.0);
    let (time, _) = frame_wallpaper_time(ctx, style);

    let cols = if is_button { 4usize } else { 6usize };
    let rows = if is_button { 3usize } else { 5usize };
    let mut mesh = Mesh::default();

    let base_alpha = if is_button {
        (style.surface_alpha + 0.14 + 0.08 * hover_t).clamp(0.56, 0.90)
    } else {
        (style.surface_alpha + 0.06 + 0.08 * hover_t).clamp(0.50, 0.88)
    };

    let warp_strength = if is_button { 0.085 } else { 0.055 };
    let frost_veil = Color32::from_rgb(0xC8, 0xDF, 0xFA);
    let deep_glass_glaze = tokens.bg_surface;
    let tint_mix = tint_override.unwrap_or(tokens.bg_surface_hover);

    for r in 0..=rows {
        let fy = (r as f32) / (rows as f32);
        let y = rect.top() + rect.height() * fy;
        for c in 0..=cols {
            let fx = (c as f32) / (cols as f32);
            let x = rect.left() + rect.width() * fx;

            let sample_pos = clamp_to_rounded_rect(Pos2::new(x, y), rect, radius);

            // Map widget coordinate to normalized viewport coordinate + convex lens UV refraction
            let sx = ((sample_pos.x - screen.left()) / sw).clamp(0.0, 1.0);
            let sy = ((sample_pos.y - screen.top()) / sh).clamp(0.0, 1.0);
            let cx = fx * 2.0 - 1.0;
            let cy = fy * 2.0 - 1.0;
            let edge_weight = (cx * cx + cy * cy).sqrt().min(1.4) * 0.72;
            let refr_x = (sx + cx * warp_strength * (0.45 + edge_weight) + 0.025).clamp(0.0, 1.0);
            let refr_y = (sy + cy * warp_strength * (0.45 + edge_weight) - 0.032).clamp(0.0, 1.0);

            let refracted_wall = sample_liquid_glass_wallpaper(refr_x, refr_y, time, style, tokens);

            // Smoked dark obsidian-sapphire glass composition:
            // maintains >= 5.5:1 muted text and >= 8:1 primary text contrast while keeping
            // unmistakable refracted aurora gradients inside every card and button.
            let tint_strength = if is_button {
                if tint_override.is_some() {
                    0.34
                } else {
                    0.30
                }
            } else {
                0.42
            };
            let wall_weighted = blend_color(deep_glass_glaze, refracted_wall, 0.58);
            let tinted = blend_color(wall_weighted, tint_mix, tint_strength + 0.14 * hover_t);

            // Subtle upper dome luminance & corner chromatic dispersion
            let top_dome = (1.0 - fy).powf(2.2) * (0.09 + 0.06 * hover_t);
            let tl_dispersion = ((1.0 - fx) * (1.0 - fy)).powf(1.5) * 0.07;
            let br_caustic = (fx * fy).powf(1.5) * 0.07;

            let with_dome = blend_color(tinted, frost_veil, top_dome + tl_dispersion);
            let cauldron = blend_color(
                with_dome,
                Color32::from_rgb(0x52, 0x8C, 0xBA),
                br_caustic + if is_button { 0.03 } else { 0.0 },
            );
            let final_rgb = clamp_glass_chroma(cauldron);

            // Slightly thicker optical frost at borders (Fresnel thickness cues)
            let rim_alpha_boost = edge_weight * 0.06;
            let alpha = ((base_alpha + rim_alpha_boost).clamp(0.45, 0.94) * 255.0).round() as u8;

            mesh.vertices.push(Vertex {
                pos: sample_pos,
                uv: Pos2::ZERO,
                color: Color32::from_rgba_unmultiplied(
                    final_rgb.r(),
                    final_rgb.g(),
                    final_rgb.b(),
                    alpha,
                ),
            });
        }
    }

    let stride = (cols + 1) as u32;
    for r in 0..(rows as u32) {
        for c in 0..(cols as u32) {
            let i0 = r * stride + c;
            let i1 = i0 + 1;
            let i2 = i0 + stride;
            let i3 = i2 + 1;
            mesh.add_triangle(i0, i1, i2);
            mesh.add_triangle(i1, i3, i2);
        }
    }
    Some(mesh)
}

/// Collects Apple iOS 26 Liquid Glass floating elevation skirt, convex lens refraction mesh,
/// curved dome highlight, lower internal caustic bounce, and cursor specular spotlight so callers
/// can place them strictly beneath child text/icons.
#[allow(clippy::too_many_arguments)]
pub fn collect_glass_underlay_shapes(
    ctx: &Context,
    rect: Rect,
    radius: CornerRadius,
    style: &ThemeStyleParams,
    tokens: &Tokens,
    hover_t: f32,
    is_button: bool,
    out: &mut Vec<Shape>,
) {
    if !rect.is_positive() || (!style.refraction_border && !style.cursor_specular_glow) {
        return;
    }

    let hover_t = hover_t.clamp(0.0, 1.0);
    let r = (radius.nw as f32)
        .min(rect.width() * 0.49)
        .min(rect.height() * 0.49);

    // 1. Apple Hierarchy Elevation Halo (subtle floating shadow separates glass from backdrop)
    if style.refraction_border && rect.height() >= 18.0 {
        let shadow_r = CornerRadius::same((r + 2.0).round().clamp(1.0, 28.0) as u8);
        out.push(Shape::rect_stroke(
            rect.expand(1.5),
            shadow_r,
            Stroke::new(
                2.0_f32,
                Color32::from_black_alpha(if is_button { 28 } else { 36 }),
            ),
            StrokeKind::Outside,
        ));
        if !is_button {
            out.push(Shape::rect_stroke(
                rect.expand(3.5),
                CornerRadius::same((r + 4.0).round().clamp(2.0, 30.0) as u8),
                Stroke::new(3.0_f32, Color32::from_black_alpha(16)),
                StrokeKind::Outside,
            ));
        }
    }

    // 2. Convex optical lens refraction sampling the wallpaper behind the glass widget
    if style.refraction_border {
        if let Some(body_mesh) = build_refracted_glass_body_mesh(
            ctx,
            rect.shrink(0.5),
            (r - 0.5).max(0.0),
            style,
            tokens,
            hover_t,
            is_button,
            None,
        ) {
            out.push(Shape::Mesh(body_mesh.into()));
        }
    }

    // 3. Curved upper dome catchlight (38% top arc) + lower internal caustic bounce (20% bottom)
    if style.inner_sheen_alpha > 0 && rect.height() > 14.0 && rect.width() > 16.0 {
        let sheen_h = (rect.height() * 0.38).clamp(7.0, 32.0);
        let sheen_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 1.5, rect.top() + 1.5),
            Pos2::new(
                rect.right() - 1.5,
                (rect.top() + sheen_h).min(rect.bottom() - 2.0),
            ),
        );
        let base_a = style.inner_sheen_alpha as f32 + if is_button { 8.0 } else { 0.0 };
        let sheen_a = (base_a * (1.0 + 0.42 * hover_t)).round().clamp(0.0, 72.0) as u8;
        let top_sheen = Color32::from_rgba_unmultiplied(248, 252, 255, sheen_a);
        let mid_sheen = Color32::from_rgba_unmultiplied(245, 250, 255, 0);
        if let Some(mesh) = build_vertical_rounded_fade_mesh(sheen_rect, r, top_sheen, mid_sheen) {
            out.push(Shape::Mesh(mesh.into()));
        }

        // Bottom internal caustic exit reflection
        let bot_h = (rect.height() * 0.22).clamp(4.0, 18.0);
        let bot_rect = Rect::from_min_max(
            Pos2::new(
                rect.left() + 1.5,
                (rect.bottom() - bot_h).max(rect.top() + 2.0),
            ),
            Pos2::new(rect.right() - 1.5, rect.bottom() - 1.5),
        );
        let bot_a = ((14.0 + 10.0 * hover_t).round()).clamp(0.0, 34.0) as u8;
        let top_caustic = Color32::from_rgba_unmultiplied(165, 215, 255, 0);
        let bot_caustic = Color32::from_rgba_unmultiplied(165, 215, 255, bot_a);
        if let Some(mesh) = build_vertical_rounded_fade_mesh(bot_rect, r, top_caustic, bot_caustic)
        {
            out.push(Shape::Mesh(mesh.into()));
        }
    }

    // 4. Smooth interactive cursor-following specular spotlight inside the rounded surface
    if style.cursor_specular_glow {
        if let Some(cursor) = smoothed_pointer_pos(ctx) {
            let spec_radius = if is_button {
                style.specular_radius_button
            } else {
                style.specular_radius_card
            };
            let dist_to_rect = distance_to_rect(cursor, rect);
            if dist_to_rect < spec_radius * 0.90 {
                let proximity = (1.0 - dist_to_rect / (spec_radius * 0.90)).clamp(0.0, 1.0);
                let base_strength = style.specular_intensity * (0.45 + 0.55 * hover_t) * proximity;
                let glow_rgb = blend_color(Color32::WHITE, tokens.accent, 0.22);
                if let Some(mesh) = build_clipped_radial_specular_mesh(
                    rect.shrink(1.0),
                    (r - 1.0).max(0.0),
                    cursor,
                    spec_radius,
                    glow_rgb,
                    base_strength,
                ) {
                    out.push(Shape::Mesh(mesh.into()));
                }
            }
        }
    }
}

/// Paints Apple iOS 26 Liquid Glass optical refraction, inner dome/caustic highlights,
/// dual-rim anisotropic Fresnel specular borders, and cursor-following specular glow over `rect`.
#[allow(clippy::too_many_arguments)]
pub fn paint_glass_surface_overlay(
    ctx: &Context,
    painter: &Painter,
    rect: Rect,
    radius: CornerRadius,
    style: &ThemeStyleParams,
    tokens: &Tokens,
    hover_t: f32,
    is_button: bool,
) {
    if !rect.is_positive() || (!style.refraction_border && !style.cursor_specular_glow) {
        return;
    }

    let hover_t = hover_t.clamp(0.0, 1.0);
    let mut shapes = Vec::new();
    collect_glass_underlay_shapes(
        ctx,
        rect,
        radius,
        style,
        tokens,
        hover_t,
        is_button,
        &mut shapes,
    );
    for shape in shapes {
        painter.add(shape);
    }

    if style.refraction_border {
        let r = (radius.nw as f32)
            .min(rect.width() * 0.49)
            .min(rect.height() * 0.49);
        paint_refractive_gradient_border(
            painter,
            rect,
            r,
            style.refraction_top_left,
            style.refraction_bottom_right,
            tokens.accent,
            smoothed_pointer_pos(ctx),
            if is_button {
                style.specular_radius_button
            } else {
                style.specular_radius_card
            },
            hover_t,
        );
    }
}

/// Paints a tinted variant of [`paint_glass_surface_overlay`] for active navigation pills,
/// segmented controls, and accent capsule buttons in Liquid Glass mode.
#[allow(clippy::too_many_arguments)]
pub fn paint_tinted_glass_pill(
    ctx: &Context,
    painter: &Painter,
    rect: Rect,
    radius: CornerRadius,
    style: &ThemeStyleParams,
    tokens: &Tokens,
    tint: Color32,
    hover_t: f32,
) {
    if !rect.is_positive() || !style.refraction_border {
        return;
    }
    let r = (radius.nw as f32)
        .min(rect.width() * 0.49)
        .min(rect.height() * 0.49);
    if let Some(body) = build_refracted_glass_body_mesh(
        ctx,
        rect.shrink(0.5),
        (r - 0.5).max(0.0),
        style,
        tokens,
        hover_t.clamp(0.0, 1.0),
        true,
        Some(tint),
    ) {
        painter.add(Shape::Mesh(body.into()));
    }
    paint_glass_surface_overlay(ctx, painter, rect, radius, style, tokens, hover_t, true);
}

/// Renders an outer luminous frosted halo behind selected cards/tiles in **Liquid Glass**
/// so selection stands out while preserving the full dual-rim Fresnel glass border.
pub fn paint_liquid_glass_selection_halo(
    painter: &Painter,
    rect: Rect,
    radius: CornerRadius,
    accent: Color32,
) {
    if !rect.is_positive() {
        return;
    }
    let r = radius.nw;
    painter.rect_stroke(
        rect.expand(1.5),
        CornerRadius::same(r.saturating_add(2)),
        Stroke::new(2.0_f32, with_alpha(accent, 0.55)),
        StrokeKind::Outside,
    );
    painter.rect_stroke(
        rect.expand(3.5),
        CornerRadius::same(r.saturating_add(4)),
        Stroke::new(2.5_f32, with_alpha(accent, 0.22)),
        StrokeKind::Outside,
    );
}

/// Renders frosted sidebar glass dock shading and luminous right specular edge for Liquid Glass.
pub fn paint_sidebar_glass_dock(
    ctx: &Context,
    painter: &Painter,
    rect: Rect,
    style: &ThemeStyleParams,
    tokens: &Tokens,
) {
    if !rect.is_positive() || !style.refraction_border {
        return;
    }
    let screen = ctx.screen_rect();
    let sw = screen.width().max(640.0);
    let sh = screen.height().max(480.0);
    let (time, _) = frame_wallpaper_time(ctx, style);

    let cols = 3usize;
    let rows = 8usize;
    let mut mesh = Mesh::default();
    let base_alpha = (style.sidebar_alpha * 0.86).clamp(0.42, 0.86);
    let a_u8 = (base_alpha * 255.0).round() as u8;

    for r in 0..=rows {
        let fy = (r as f32) / (rows as f32);
        let y = rect.top() + rect.height() * fy;
        for c in 0..=cols {
            let fx = (c as f32) / (cols as f32);
            let x = rect.left() + rect.width() * fx;

            let sx = ((x - screen.left()) / sw).clamp(0.0, 1.0);
            let sy = ((y - screen.top()) / sh).clamp(0.0, 1.0);
            let refr = sample_liquid_glass_wallpaper(
                (sx + 0.03).clamp(0.0, 1.0),
                (sy - 0.02).clamp(0.0, 1.0),
                time,
                style,
                tokens,
            );
            let glass = blend_color(tokens.bg_sidebar, refr, 0.42);
            let frost = blend_color(
                glass,
                Color32::from_rgb(0xB8, 0xD2, 0xEE),
                (1.0 - fy) * 0.05,
            );
            let final_rgb = clamp_glass_chroma(frost);
            mesh.vertices.push(Vertex {
                pos: Pos2::new(x, y),
                uv: Pos2::ZERO,
                color: Color32::from_rgba_unmultiplied(
                    final_rgb.r(),
                    final_rgb.g(),
                    final_rgb.b(),
                    a_u8,
                ),
            });
        }
    }

    let stride = (cols + 1) as u32;
    for r in 0..(rows as u32) {
        for c in 0..(cols as u32) {
            let i0 = r * stride + c;
            let i1 = i0 + 1;
            let i2 = i0 + stride;
            let i3 = i2 + 1;
            mesh.add_triangle(i0, i1, i2);
            mesh.add_triangle(i1, i3, i2);
        }
    }
    painter.add(mesh);

    // Subtle icy vertical edge catchlight along right edge of glass sidebar
    painter.line_segment(
        [
            Pos2::new(rect.right() - 1.0, rect.top()),
            Pos2::new(rect.right() - 1.0, rect.bottom()),
        ],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(235, 246, 255, 86)),
    );
    painter.line_segment(
        [
            Pos2::new(rect.right() - 2.0, rect.top()),
            Pos2::new(rect.right() - 2.0, rect.bottom()),
        ],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(150, 205, 250, 24)),
    );
}

/// Renders frosted bottom HUD glass dock shading and luminous top specular lip for Liquid Glass.
pub fn paint_hud_glass_dock(
    ctx: &Context,
    painter: &Painter,
    rect: Rect,
    style: &ThemeStyleParams,
    tokens: &Tokens,
) {
    if !rect.is_positive() || !style.refraction_border {
        return;
    }
    if let Some(mesh) = build_vertical_rounded_fade_mesh(
        rect,
        0.0,
        Color32::from_rgba_unmultiplied(220, 238, 255, 18),
        with_alpha(tokens.bg_sidebar, 0.04),
    ) {
        painter.add(Shape::Mesh(mesh.into()));
    }
    let _ = ctx;
    painter.line_segment(
        [
            Pos2::new(rect.left(), rect.top() + 0.5),
            Pos2::new(rect.right(), rect.top() + 0.5),
        ],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(240, 248, 255, 118)),
    );
    painter.line_segment(
        [
            Pos2::new(rect.left(), rect.top() + 1.5),
            Pos2::new(rect.right(), rect.top() + 1.5),
        ],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(150, 210, 255, 28)),
    );
}

/// Renders an `egui::Frame` whose Liquid Glass convex lens refraction, dome catchlight, and cursor
/// specular glow sit strictly behind child widgets (preserving crisp text legibility), topped by
/// an Apple iOS 26 dual-rim anisotropic Fresnel specular border.
pub fn show_themed_card<R>(
    ui: &mut Ui,
    frame: egui::Frame,
    radius: CornerRadius,
    hover_id: Id,
    style: &ThemeStyleParams,
    tokens: &Tokens,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    if !style.refraction_border && !style.cursor_specular_glow {
        return frame.show(ui, add_contents);
    }

    let effective_radius = if style.refraction_border {
        style.card_corner_radius(radius.nw as f32)
    } else {
        radius
    };

    let mut frame = frame;
    if style.refraction_border {
        frame.stroke = Stroke::NONE;
        frame.corner_radius = effective_radius;
        // Let our refracted lens mesh render the glass fill so there is no flat opaque glaze
        frame.fill = Color32::TRANSPARENT;
    }

    let mut prepared = frame.begin(ui);
    let overlay_slot = prepared.content_ui.painter().add(Shape::Noop);
    let inner = add_contents(&mut prepared.content_ui);
    let frame_resp = prepared.end(ui);
    let hovered = ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|pos| frame_resp.rect.contains(pos));
    let hover_t = ui.ctx().animate_bool_with_time(hover_id, hovered, 0.14);

    let mut under_shapes = Vec::new();
    collect_glass_underlay_shapes(
        ui.ctx(),
        frame_resp.rect,
        effective_radius,
        style,
        tokens,
        hover_t,
        false,
        &mut under_shapes,
    );
    if !under_shapes.is_empty() {
        ui.painter().set(overlay_slot, Shape::Vec(under_shapes));
    }

    if style.refraction_border {
        paint_refractive_gradient_border(
            ui.painter(),
            frame_resp.rect,
            effective_radius.nw as f32,
            style.refraction_top_left,
            style.refraction_bottom_right,
            tokens.accent,
            smoothed_pointer_pos(ui.ctx()),
            style.specular_radius_card,
            hover_t,
        );
    }

    InnerResponse::new(inner, frame_resp)
}

fn smoothed_pointer_pos(ctx: &Context) -> Option<Pos2> {
    let raw = ctx.input(|i| i.pointer.hover_pos())?;
    let x = ctx.animate_value_with_time(Id::new("mq_glass_cursor_x"), raw.x, 0.07);
    let y = ctx.animate_value_with_time(Id::new("mq_glass_cursor_y"), raw.y, 0.07);
    Some(Pos2::new(x, y))
}

#[inline]
fn distance_to_rect(p: Pos2, rect: Rect) -> f32 {
    let dx = (rect.left() - p.x).max(0.0).max(p.x - rect.right());
    let dy = (rect.top() - p.y).max(0.0).max(p.y - rect.bottom());
    (dx * dx + dy * dy).sqrt()
}

#[inline]
fn clamp_to_rounded_rect(p: Pos2, rect: Rect, radius: f32) -> Pos2 {
    let r = radius.clamp(0.0, (rect.width().min(rect.height()) * 0.5).max(0.0));
    if r <= 0.5 {
        return Pos2::new(
            p.x.clamp(rect.left(), rect.right()),
            p.y.clamp(rect.top(), rect.bottom()),
        );
    }
    let inner = rect.shrink(r);
    let clamped_inner = Pos2::new(
        p.x.clamp(inner.left(), inner.right()),
        p.y.clamp(inner.top(), inner.bottom()),
    );
    let diff = p - clamped_inner;
    let len = diff.length();
    if len <= r || len < 1e-4 {
        p
    } else {
        clamped_inner + diff * (r / len)
    }
}

fn build_vertical_rounded_fade_mesh(
    rect: Rect,
    top_radius: f32,
    top_col: Color32,
    bot_col: Color32,
) -> Option<Mesh> {
    if !rect.is_positive() {
        return None;
    }
    let inset = (top_radius * 0.35).min(rect.width() * 0.25);
    let mut mesh = Mesh::default();
    mesh.vertices.push(Vertex {
        pos: Pos2::new(rect.left() + inset, rect.top()),
        uv: Pos2::ZERO,
        color: top_col,
    });
    mesh.vertices.push(Vertex {
        pos: Pos2::new(rect.right() - inset, rect.top()),
        uv: Pos2::ZERO,
        color: top_col,
    });
    mesh.vertices.push(Vertex {
        pos: Pos2::new(rect.left() + inset * 0.25, rect.bottom()),
        uv: Pos2::ZERO,
        color: bot_col,
    });
    mesh.vertices.push(Vertex {
        pos: Pos2::new(rect.right() - inset * 0.25, rect.bottom()),
        uv: Pos2::ZERO,
        color: bot_col,
    });
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    Some(mesh)
}

#[allow(clippy::too_many_arguments)]
fn build_clipped_radial_specular_mesh(
    rect: Rect,
    radius: f32,
    cursor: Pos2,
    spec_radius: f32,
    glow_rgb: Color32,
    intensity: f32,
) -> Option<Mesh> {
    if !rect.is_positive() || intensity <= 0.005 {
        return None;
    }
    let clamped_center = Pos2::new(
        cursor.x.clamp(rect.left(), rect.right()),
        cursor.y.clamp(rect.top(), rect.bottom()),
    );
    let cols = 6usize;
    let rows = 4usize;
    let mut mesh = Mesh::default();

    for r in 0..=rows {
        let fy = (r as f32) / (rows as f32);
        let y = rect.top() + rect.height() * fy;
        for c in 0..=cols {
            let fx = (c as f32) / (cols as f32);
            let x = rect.left() + rect.width() * fx;
            let sample = clamp_to_rounded_rect(Pos2::new(x, y), rect, radius);
            let dist = (sample - clamped_center).length();
            let falloff = (1.0 - dist / spec_radius.max(1.0))
                .clamp(0.0, 1.0)
                .powf(2.0);
            let a = (falloff * intensity * 255.0).round().clamp(0.0, 104.0) as u8;
            mesh.vertices.push(Vertex {
                pos: sample,
                uv: Pos2::ZERO,
                color: Color32::from_rgba_unmultiplied(glow_rgb.r(), glow_rgb.g(), glow_rgb.b(), a),
            });
        }
    }

    let stride = (cols + 1) as u32;
    for r in 0..(rows as u32) {
        for c in 0..(cols as u32) {
            let i0 = r * stride + c;
            let i1 = i0 + 1;
            let i2 = i0 + stride;
            let i3 = i2 + 1;
            mesh.add_triangle(i0, i1, i2);
            mesh.add_triangle(i1, i3, i2);
        }
    }
    Some(mesh)
}

#[allow(clippy::too_many_arguments)]
fn paint_refractive_gradient_border(
    painter: &Painter,
    rect: Rect,
    radius: f32,
    top_left_col: Color32,
    bottom_right_col: Color32,
    accent_col: Color32,
    cursor: Option<Pos2>,
    spec_radius: f32,
    hover_t: f32,
) {
    let r = radius.clamp(0.0, (rect.width().min(rect.height()) * 0.5).max(0.0));
    if r < 1.0 {
        painter.rect_stroke(
            rect,
            CornerRadius::ZERO,
            Stroke::new(1.0_f32, top_left_col),
            StrokeKind::Inside,
        );
        return;
    }

    // 8 steps per corner yields silky-smooth iOS continuous-corner arcs.
    let steps_per_corner = 8usize;
    let corners = [
        (
            Pos2::new(rect.left() + r, rect.top() + r),
            std::f32::consts::PI,
            std::f32::consts::FRAC_PI_2 * 3.0,
        ),
        (
            Pos2::new(rect.right() - r, rect.top() + r),
            std::f32::consts::FRAC_PI_2 * 3.0,
            std::f32::consts::TAU,
        ),
        (
            Pos2::new(rect.right() - r, rect.bottom() - r),
            0.0_f32,
            std::f32::consts::FRAC_PI_2,
        ),
        (
            Pos2::new(rect.left() + r, rect.bottom() - r),
            std::f32::consts::FRAC_PI_2,
            std::f32::consts::PI,
        ),
    ];

    let mut ring: Vec<(Pos2, egui::Vec2)> = Vec::with_capacity(4 * (steps_per_corner + 1));
    for (center, start_a, end_a) in corners {
        for s in 0..=steps_per_corner {
            let t = (s as f32) / (steps_per_corner as f32);
            let angle = start_a + (end_a - start_a) * t;
            let normal = egui::vec2(angle.cos(), angle.sin());
            let pt = center + normal * r;
            ring.push((pt, normal));
        }
    }

    let n = ring.len();
    if n < 8 {
        return;
    }

    // Anisotropic Apple iOS 26 Fresnel light response along the glass chamfer:
    // peak specular at top-left (~135°), soft dim neutral at top-right / bottom-left slopes,
    // and luminous secondary counter-refraction exit rim at bottom-right (~-45°).
    let light_dir = egui::vec2(
        -std::f32::consts::FRAC_1_SQRT_2,
        -std::f32::consts::FRAC_1_SQRT_2,
    );
    let mid_neutral = Color32::from_rgba_unmultiplied(195, 220, 245, 52);

    let outer_w = 1.25_f32 + 0.35 * hover_t;
    let mut outer_mesh = Mesh::default();
    let mut inner_mesh = Mesh::default();
    let w = rect.width().max(1.0);
    let h = rect.height().max(1.0);

    for &(outer_pt, normal) in &ring {
        let inner_pt = outer_pt - normal * outer_w;
        let nx = ((outer_pt.x - rect.left()) / w).clamp(0.0, 1.0);
        let ny = ((outer_pt.y - rect.top()) / h).clamp(0.0, 1.0);

        let grazing = (normal.x * light_dir.x + normal.y * light_dir.y).clamp(-1.0, 1.0);
        let pos_bias = ((1.0 - 0.5 * (nx + ny)) * 2.0 - 1.0).clamp(-1.0, 1.0);
        let anisotropy = (grazing * 0.72 + pos_bias * 0.28).clamp(-1.0, 1.0);

        let mut col = if anisotropy >= 0.0 {
            let k = anisotropy.powf(1.25);
            lerp_rgba(mid_neutral, top_left_col, k)
        } else {
            let k = (-anisotropy).powf(1.45);
            lerp_rgba(mid_neutral, bottom_right_col, k)
        };

        // Cursor-following Fresnel glint near the active pointer
        if let Some(cur) = cursor {
            let dist = (outer_pt - cur).length();
            if dist < spec_radius {
                let prox = (1.0 - dist / spec_radius.max(1.0))
                    .clamp(0.0, 1.0)
                    .powf(1.5);
                let glint_rgb = blend_color(Color32::WHITE, accent_col, 0.18);
                let glint = Color32::from_rgba_unmultiplied(
                    glint_rgb.r(),
                    glint_rgb.g(),
                    glint_rgb.b(),
                    238,
                );
                col = lerp_rgba(col, glint, prox * (0.58 + 0.34 * hover_t));
            }
        }

        outer_mesh.vertices.push(Vertex {
            pos: outer_pt,
            uv: Pos2::ZERO,
            color: col,
        });
        outer_mesh.vertices.push(Vertex {
            pos: inner_pt,
            uv: Pos2::ZERO,
            color: col,
        });

        // Second inner 0.9px caustic rim inset slightly inside the primary specular border
        let ca_outer = outer_pt - normal * (outer_w + 0.35);
        let ca_inner = outer_pt - normal * (outer_w + 1.25);
        let caustic_col = if anisotropy >= 0.0 {
            Color32::from_rgba_unmultiplied(
                245,
                250,
                255,
                (40.0 * (0.35 + 0.65 * anisotropy)) as u8,
            )
        } else {
            Color32::from_rgba_unmultiplied(
                150,
                210,
                255,
                (24.0 * (0.25 + 0.75 * -anisotropy)) as u8,
            )
        };
        inner_mesh.vertices.push(Vertex {
            pos: ca_outer,
            uv: Pos2::ZERO,
            color: caustic_col,
        });
        inner_mesh.vertices.push(Vertex {
            pos: ca_inner,
            uv: Pos2::ZERO,
            color: caustic_col,
        });
    }

    for i in 0..n {
        let next = (i + 1) % n;
        let o0 = (i * 2) as u32;
        let i0 = o0 + 1;
        let o1 = (next * 2) as u32;
        let i1 = o1 + 1;
        outer_mesh.add_triangle(o0, o1, i0);
        outer_mesh.add_triangle(o1, i1, i0);
        inner_mesh.add_triangle(o0, o1, i0);
        inner_mesh.add_triangle(o1, i1, i0);
    }

    painter.add(Shape::Mesh(outer_mesh.into()));
    if r >= 6.0 {
        painter.add(Shape::Mesh(inner_mesh.into()));
    }
}

#[inline]
pub fn lerp_rgba(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    Color32::from_rgba_unmultiplied(
        ((a.r() as f32) * inv + (b.r() as f32) * t).round() as u8,
        ((a.g() as f32) * inv + (b.g() as f32) * t).round() as u8,
        ((a.b() as f32) * inv + (b.b() as f32) * t).round() as u8,
        ((a.a() as f32) * inv + (b.a() as f32) * t).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_params_resolve_for_liquid_glass_translucent_and_solid() {
        let solid = ThemeStyleParams::from_config(&ThemeConfig::preset_matte_dark());
        assert!(!solid.compositor_transparent);
        assert!(!solid.refraction_border);
        assert!(!solid.animated_ambient_bg);
        assert!(!solid.cursor_specular_glow);
        assert_eq!(solid.window_alpha, 1.0);

        let glass = ThemeStyleParams::from_config(&ThemeConfig::preset_liquid_glass());
        assert!(glass.compositor_transparent);
        assert!(glass.refraction_border);
        assert!(glass.animated_ambient_bg);
        assert!(glass.cursor_specular_glow);
        assert!(glass.backdrop_blur_strength > 0.5);
        assert!(glass.window_alpha < 1.0);
        assert!(glass.surface_alpha < 1.0);
        assert_eq!(glass.card_corner_radius(12.0), CornerRadius::same(16));
        assert_eq!(
            glass.control_corner_radius(32.0, CornerRadius::same(6)),
            CornerRadius::same(16)
        );

        let trans = ThemeStyleParams::from_config(&ThemeConfig::preset_translucent());
        assert!(trans.compositor_transparent);
        assert!(!trans.refraction_border);
        assert!(!trans.animated_ambient_bg);
        assert!(!trans.cursor_specular_glow);
        assert!(trans.window_alpha < 1.0);
        assert!(trans.surface_alpha < 1.0);
    }

    #[test]
    fn liquid_glass_and_translucent_render_without_panics() {
        let ctx = Context::default();
        for cfg in [
            ThemeConfig::preset_liquid_glass(),
            ThemeConfig::preset_translucent(),
        ] {
            let tokens = Tokens::from_config(&cfg);
            let style = ThemeStyleParams::from_config(&cfg);
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1180.0, 740.0))),
                events: vec![egui::Event::PointerMoved(Pos2::new(240.0, 180.0))],
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let rect = ui.max_rect();
                    if cfg.preset == ThemePreset::LiquidGlass {
                        paint_liquid_glass_ambient_backdrop(
                            ctx,
                            ui.painter(),
                            rect,
                            &style,
                            &tokens,
                            true,
                        );
                    } else {
                        paint_translucent_clean_backdrop(ui.painter(), rect, &style, &tokens);
                    }
                    let frame = crate::theme::solid_card_frame_with_config(&tokens, &cfg);
                    show_themed_card(
                        ui,
                        frame,
                        CornerRadius::same(12),
                        Id::new("test_card"),
                        &style,
                        &tokens,
                        |ui| {
                            ui.label("Glass label");
                        },
                    );
                });
            });
        }
    }

    #[test]
    fn glass_wallpaper_samples_stay_within_frosted_chroma_budget() {
        let cfg = ThemeConfig::preset_liquid_glass();
        let style = ThemeStyleParams::from_config(&cfg);
        let tokens = Tokens::from_config(&cfg);
        for iy in 0..=8 {
            for ix in 0..=12 {
                let c = sample_liquid_glass_wallpaper(
                    ix as f32 / 12.0,
                    iy as f32 / 8.0,
                    0.5,
                    &style,
                    &tokens,
                );
                let r = c.r() as f32 / 255.0;
                let g = c.g() as f32 / 255.0;
                let b = c.b() as f32 / 255.0;
                let cmax = r.max(g).max(b);
                let cmin = r.min(g).min(b);
                let delta = cmax - cmin;
                let sat = if cmax > 0.0 { delta / cmax } else { 0.0 };
                assert!(
                    !(sat > 0.45 && delta > 0.22 && cmax > 0.25),
                    "wallpaper sample {:?} exceeds frosted budget (sat={sat:.3}, delta={delta:.3})",
                    c
                );
            }
        }
    }
}
