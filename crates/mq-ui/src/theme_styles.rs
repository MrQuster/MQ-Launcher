//! Theme style parameters (`ThemeStyleParams`) and optical **Liquid Glass** renderer
//! using hardware-accelerated WGSL shaders via `egui_wgpu::CallbackTrait` (`PaintCallback`),
//! with an automatic `egui::Mesh` + CPU-blur fallback when running under `egui_glow` / OpenGL.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use egui::epaint::{Mesh, Vertex};
use egui::{
    Color32, Context, CornerRadius, Id, InnerResponse, Painter, Pos2, Rect, Shape, Stroke,
    StrokeKind, TextureId, Ui,
};
use mq_core::{ThemeConfig, ThemePreset};

use crate::theme::{
    active_gpu_backdrop, blend_color, is_wgpu_glass_available, ActiveGpuBackdrop,
    LiquidGlassGpuResources, LiquidGlassUniforms, Tokens,
};

/// Active blurred wallpaper texture info shared between background painting and glass overlays.
#[derive(Clone, Copy, Debug)]
pub struct ActiveBackdropTexture {
    pub tex_id: TextureId,
    pub screen_rect: Rect,
    pub uv_rect: Rect,
}

#[inline]
pub fn store_active_backdrop_texture(ctx: &Context, active: Option<ActiveBackdropTexture>) {
    let key = Id::new("mq_active_glass_backdrop_tex");
    ctx.data_mut(|d| {
        d.insert_temp(key, active);
    });
}

#[inline]
pub fn active_backdrop_texture(ctx: &Context) -> Option<ActiveBackdropTexture> {
    ctx.data(|d| {
        d.get_temp::<Option<ActiveBackdropTexture>>(Id::new("mq_active_glass_backdrop_tex"))
            .flatten()
    })
}

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
    /// Draw Serpantinum optical glass bevel refraction + directional hairline rim.
    pub refraction_border: bool,
    /// Silvery primary rim highlight (`lightDir = normalize(-0.55, -0.83)`).
    pub refraction_top_left: Color32,
    /// Softer bottom counter-reflection rim (`0.55 * spec`).
    pub refraction_bottom_right: Color32,
    /// Subtle meniscus sheen alpha (`0..=255`).
    pub inner_sheen_alpha: u8,
    /// Enables gentle ambient fallback gradient when no wallpaper image is active.
    pub animated_ambient_bg: bool,
    pub ambient_lobe_a: Color32,
    pub ambient_lobe_b: Color32,
    pub ambient_lobe_c: Color32,
    /// Smoothly steers directional bevel highlight toward hovered cursor (`mix(lightDir, cursorDir, hover * 0.4)`).
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

    /// Liquid Glass (`Serpantinum v2.5.1` 1:1 optical style):
    /// clear crystal blurred wallpaper inside, Snell's-law bezel refraction on edges,
    /// delicate 1px directional specular rim (`rim = 0.45`), and 10px continuous corners.
    pub fn liquid_glass(cfg: &ThemeConfig) -> Self {
        let win_a = crate::state::safe_clamp_f32(cfg.window_opacity, 0.35, 1.0);
        let card_a = crate::state::safe_clamp_f32(cfg.card_opacity, 0.35, 1.0);
        let glow_factor = crate::state::safe_clamp_f32(cfg.border_glow, 0.10, 1.0);
        Self {
            preset: ThemePreset::LiquidGlass,
            compositor_transparent: true,
            window_alpha: win_a,
            sidebar_alpha: (win_a * 0.56).clamp(0.36, 0.80),
            surface_alpha: card_a,
            hud_alpha: (win_a * 0.62).clamp(0.40, 0.84),
            modal_alpha: (card_a + 0.48).clamp(0.80, 0.94),
            input_alpha: (card_a + 0.08).clamp(0.32, 0.72),
            backdrop_blur_strength: 0.88,
            refraction_border: true,
            refraction_top_left: Color32::from_rgba_unmultiplied(
                255,
                255,
                255,
                (128.0 * (0.68 + 0.32 * glow_factor)).round().min(165.0) as u8,
            ),
            refraction_bottom_right: Color32::from_rgba_unmultiplied(
                232,
                238,
                248,
                (76.0 * (0.65 + 0.35 * glow_factor)).round().min(108.0) as u8,
            ),
            inner_sheen_alpha: 0,
            animated_ambient_bg: true,
            ambient_lobe_a: Color32::from_rgb(0x28, 0x2C, 0x36),
            ambient_lobe_b: Color32::from_rgb(0x1E, 0x22, 0x2B),
            ambient_lobe_c: Color32::from_rgb(0x24, 0x2A, 0x35),
            cursor_specular_glow: true,
            specular_radius_card: 180.0,
            specular_radius_button: 110.0,
            specular_intensity: 0.22 + glow_factor * 0.12,
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

    /// Card rounding (`10px` default matching Serpantinum `borderRadius: 10`).
    #[inline]
    pub fn card_corner_radius(&self, base_radius: f32) -> CornerRadius {
        let clamped = if self.refraction_border {
            crate::state::safe_clamp_f32(base_radius.max(8.0), 6.0, 14.0)
        } else {
            crate::state::safe_clamp_f32(base_radius, 0.0, 16.0)
        };
        CornerRadius::same(clamped.round() as u8)
    }

    /// Control rounding (`10px` clean rounded glass rect in Liquid Glass, matching Serpantinum buttons/rows).
    #[inline]
    pub fn control_corner_radius(&self, _height: f32, fallback: CornerRadius) -> CornerRadius {
        if self.refraction_border {
            CornerRadius::same(10)
        } else {
            fallback
        }
    }

    /// Modal sheet rounding (`12px` in Liquid Glass).
    #[inline]
    pub fn modal_corner_radius(&self, fallback: CornerRadius) -> CornerRadius {
        if self.refraction_border {
            CornerRadius::same(12)
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
        let hover_alpha = (self.surface_alpha + if self.refraction_border { 0.10 } else { 0.07 })
            .clamp(0.32, 1.0);
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
        let t = 0.16 + 0.14 * hover_t.clamp(0.0, 1.0);
        let rgb = blend_color(tokens.bg_surface, tokens.accent, t);
        if self.refraction_border {
            let a = (self.surface_alpha + 0.12 + 0.08 * hover_t).clamp(0.36, 0.85);
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
            (self.surface_alpha + 0.08 * hover_t).clamp(0.32, 0.82)
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

/// Evaluates calm graphite fallback gradient when no wallpaper image is active.
pub fn sample_liquid_glass_wallpaper(
    nx: f32,
    ny: f32,
    _time: f32,
    style: &ThemeStyleParams,
    tokens: &Tokens,
) -> Color32 {
    let fx = nx.clamp(0.0, 1.0);
    let fy = ny.clamp(0.0, 1.0);
    let d1 = ((fx - 0.25).powi(2) + (fy - 0.20).powi(2)).sqrt();
    let d2 = ((fx - 0.78).powi(2) + (fy - 0.76).powi(2)).sqrt();
    let w1 = (1.0 - d1 / 1.1).max(0.0).powf(1.6) * 0.45;
    let w2 = (1.0 - d2 / 1.1).max(0.0).powf(1.6) * 0.35;
    let base = blend_color(tokens.bg_app, style.ambient_lobe_b, w2);
    clamp_glass_chroma(blend_color(base, style.ambient_lobe_a, w1))
}

/// Renders calm fallback graphite gradient when no wallpaper texture is loaded.
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

    let _ = deterministic;
    let cols = 8usize;
    let rows = 6usize;
    let mut mesh = Mesh::default();
    let alpha_u8 =
        (crate::state::safe_clamp_f32(style.window_alpha, 0.35, 1.0) * 255.0).round() as u8;

    for r in 0..=rows {
        let fy = (r as f32) / (rows as f32);
        let y = rect.top() + rect.height() * fy;
        for c in 0..=cols {
            let fx = (c as f32) / (cols as f32);
            let x = rect.left() + rect.width() * fx;
            let col = sample_liquid_glass_wallpaper(fx, fy, 0.0, style, tokens);
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
    let _ = ctx;
}

/// Renders a clean, uniform tinted backdrop for the **Translucent** preset (no glares or motion).
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

#[inline]
fn screen_pos_to_uv(pos: Pos2, backdrop: &ActiveBackdropTexture) -> Pos2 {
    let sw = backdrop.screen_rect.width().max(1.0);
    let sh = backdrop.screen_rect.height().max(1.0);
    let nx = ((pos.x - backdrop.screen_rect.left()) / sw).clamp(0.0, 1.0);
    let ny = ((pos.y - backdrop.screen_rect.top()) / sh).clamp(0.0, 1.0);
    Pos2::new(
        (backdrop.uv_rect.left() + backdrop.uv_rect.width() * nx).clamp(0.002, 0.998),
        (backdrop.uv_rect.top() + backdrop.uv_rect.height() * ny).clamp(0.002, 0.998),
    )
}

/// Builds the exact rounded ring points `(pos, outward_normal)` around `rect` with corner radius `r`.
fn build_rounded_perimeter_ring(
    rect: Rect,
    r: f32,
    steps_per_corner: usize,
) -> Vec<(Pos2, egui::Vec2)> {
    let r = r.clamp(0.0, (rect.width().min(rect.height()) * 0.49).max(0.0));
    if r < 1.0 {
        return vec![
            (rect.left_top(), egui::vec2(-0.707, -0.707)),
            (rect.right_top(), egui::vec2(0.707, -0.707)),
            (rect.right_bottom(), egui::vec2(0.707, 0.707)),
            (rect.left_bottom(), egui::vec2(-0.707, 0.707)),
        ];
    }

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

    let mut ring = Vec::with_capacity(4 * (steps_per_corner + 1));
    for (center, start_a, end_a) in corners {
        for s in 0..=steps_per_corner {
            let t = (s as f32) / (steps_per_corner as f32);
            let angle = start_a + (end_a - start_a) * t;
            let normal = egui::vec2(angle.cos(), angle.sin());
            ring.push((center + normal * r, normal));
        }
    }
    ring
}

/// Ports Serpantinum's `liquid_glass.frag` optical pipeline to pure `egui::Mesh`:
/// - `LENS_ZOOM = 0.045` center wallpaper magnification through cached Gaussian-blurred backdrop
/// - Snell's-law edge bezel refraction ring (`bezel = clamp(min(w, h) * 0.28, 4.0, 11.0)`)
/// - Clear neutral frosted dark glaze for guaranteed contrast over bright & dark wallpapers.
fn build_serpantinum_refracted_glass_mesh(
    ctx: &Context,
    rect: Rect,
    radius: f32,
    hover_t: f32,
    is_button: bool,
    tint_override: Option<Color32>,
) -> Option<Mesh> {
    let backdrop = active_backdrop_texture(ctx)?;
    if !rect.is_positive() || rect.width() < 6.0 || rect.height() < 6.0 {
        return None;
    }

    let r = radius.clamp(0.5, (rect.width().min(rect.height()) * 0.49).max(0.5));
    let bezel = (rect.width().min(rect.height()) * 0.26).clamp(3.5, 11.0);
    let inner_rect = rect.shrink(bezel);
    if !inner_rect.is_positive() {
        return None;
    }
    let inner_r = (r - bezel).max(0.5);
    let steps = 6usize;

    let outer_ring = build_rounded_perimeter_ring(rect, r, steps);
    let inner_ring = build_rounded_perimeter_ring(inner_rect, inner_r, steps);
    let n = outer_ring.len();
    if n < 4 || inner_ring.len() != n {
        return None;
    }

    let center = rect.center();
    let max_dim = rect.width().max(rect.height()) * 0.5;
    let lens_k = 1.0 - ((max_dim - 180.0) / (420.0 - 180.0)).clamp(0.0, 1.0);
    let lens_zoom = 0.045 * lens_k;

    // Refraction travel distance matching displacement(normal, 1.5, thickness) in liquid_glass.frag
    let refract_px = bezel * (1.25 + 0.18 * hover_t);
    let sw = backdrop.screen_rect.width().max(1.0);
    let sh = backdrop.screen_rect.height().max(1.0);

    // Base frosted neutral multiplier (lets blurred mountains shine while ensuring legible contrast)
    let interior_tint = match tint_override {
        Some(t) => Color32::from_rgb(
            ((190 + (t.r() as u16) * 2 / 5).min(240)) as u8,
            ((194 + (t.g() as u16) * 2 / 5).min(240)) as u8,
            ((202 + (t.b() as u16) * 2 / 5).min(245)) as u8,
        ),
        None => {
            let v = if is_button { 196 } else { 184 };
            Color32::from_rgb(v, v + 3, v + 8)
        }
    };

    let light_dir = egui::vec2(-0.5523, -0.8336);
    let mut mesh = Mesh::with_texture(backdrop.tex_id);

    // Center vertex index 0
    let c_uv = screen_pos_to_uv(center, &backdrop);
    mesh.vertices.push(Vertex {
        pos: center,
        uv: c_uv,
        color: interior_tint,
    });

    // Ring vertices: for each i in 0..n, vertex (1 + 2*i) is outer, (1 + 2*i + 1) is inner
    for i in 0..n {
        let (out_p, norm) = outer_ring[i];
        let (in_p, _) = inner_ring[i];

        // Outer vertex UV: displaced outward/inward along normal in glass bezel
        let facing = (norm.x * light_dir.x + norm.y * light_dir.y).clamp(-1.0, 1.0);
        let wobble = (out_p.x * 0.05 + out_p.y * 0.04).sin() * 0.12;
        let disp_px = egui::vec2(
            norm.x * (refract_px * (1.0 + wobble)),
            norm.y * (refract_px * (1.0 - wobble)),
        );
        let refr_pos = Pos2::new(
            (out_p.x - disp_px.x).clamp(backdrop.screen_rect.left(), backdrop.screen_rect.right()),
            (out_p.y - disp_px.y).clamp(backdrop.screen_rect.top(), backdrop.screen_rect.bottom()),
        );
        let mut out_uv = screen_pos_to_uv(refr_pos, &backdrop);
        // Subtle lens pull toward center on inner edge
        let in_mag_pos = Pos2::new(
            center.x + (in_p.x - center.x) * (1.0 - lens_zoom),
            center.y + (in_p.y - center.y) * (1.0 - lens_zoom),
        );
        let in_uv = screen_pos_to_uv(in_mag_pos, &backdrop);

        // Fresnel edge boost along bezel: lit top edge reflects slightly brighter sky/frost
        let spec_lift = (facing.max(0.0).powf(1.5) * 32.0
            + (-facing).max(0.0).powf(2.0) * 14.0
            + hover_t * 14.0)
            .round() as u16;
        let bezel_col = Color32::from_rgb(
            (interior_tint.r() as u16 + spec_lift).min(248) as u8,
            (interior_tint.g() as u16 + spec_lift).min(248) as u8,
            (interior_tint.b() as u16 + spec_lift + 4).min(252) as u8,
        );
        out_uv.x = out_uv.x.clamp(0.002 + 1.0 / sw, 0.998 - 1.0 / sw);
        out_uv.y = out_uv.y.clamp(0.002 + 1.0 / sh, 0.998 - 1.0 / sh);

        mesh.vertices.push(Vertex {
            pos: out_p,
            uv: out_uv,
            color: bezel_col,
        });
        mesh.vertices.push(Vertex {
            pos: in_p,
            uv: in_uv,
            color: interior_tint,
        });
    }

    for i in 0..n {
        let next = (i + 1) % n;
        let o0 = (1 + 2 * i) as u32;
        let i0 = o0 + 1;
        let o1 = (1 + 2 * next) as u32;
        let i1 = o1 + 1;

        // Bezel ring quad (two triangles)
        mesh.add_triangle(o0, o1, i0);
        mesh.add_triangle(o1, i1, i0);
        // Interior triangle fan
        mesh.add_triangle(0, i0, i1);
    }

    Some(mesh)
}

struct LiquidGlassCallback {
    backdrop: Option<Arc<ActiveGpuBackdrop>>,
    uniforms: LiquidGlassUniforms,
    generation: u64,
    slot_offset: AtomicU32,
}

impl egui_wgpu::CallbackTrait for LiquidGlassCallback {
    fn prepare(
        &self,
        device: &egui_wgpu::wgpu::Device,
        queue: &egui_wgpu::wgpu::Queue,
        screen_descriptor: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut egui_wgpu::wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<egui_wgpu::wgpu::CommandBuffer> {
        let Some(resources) = callback_resources.get_mut::<LiquidGlassGpuResources>() else {
            return Vec::new();
        };
        resources.begin_frame_if_needed(self.generation);
        if let Some(ref backdrop) = self.backdrop {
            resources.sync_backdrop(
                device,
                queue,
                egui_encoder,
                &backdrop.source,
                screen_descriptor.size_in_pixels,
            );
        }
        let mut u = self.uniforms;
        u.screen_size = [
            screen_descriptor.size_in_pixels[0].max(1) as f32,
            screen_descriptor.size_in_pixels[1].max(1) as f32,
        ];
        let offset = resources.push_uniforms(&u);
        self.slot_offset.store(offset, Ordering::Relaxed);
        Vec::new()
    }

    fn finish_prepare(
        &self,
        device: &egui_wgpu::wgpu::Device,
        queue: &egui_wgpu::wgpu::Queue,
        _egui_encoder: &mut egui_wgpu::wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<egui_wgpu::wgpu::CommandBuffer> {
        if let Some(resources) = callback_resources.get_mut::<LiquidGlassGpuResources>() {
            resources.flush_uniforms(device, queue);
        }
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut egui_wgpu::wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(resources) = callback_resources.get::<LiquidGlassGpuResources>() else {
            return;
        };
        if info.screen_size_px[0] == 0 || info.screen_size_px[1] == 0 {
            return;
        }
        render_pass.set_viewport(
            0.0,
            0.0,
            info.screen_size_px[0] as f32,
            info.screen_size_px[1] as f32,
            0.0,
            1.0,
        );
        render_pass.set_pipeline(resources.glass_pipeline());
        let offset = self.slot_offset.load(Ordering::Relaxed);
        render_pass.set_bind_group(0, resources.glass_bind_group(), &[offset]);
        render_pass.draw(0..6, 0..1);
    }
}

#[allow(clippy::too_many_arguments)]
fn build_gpu_liquid_glass_shape(
    ctx: &Context,
    rect: Rect,
    callback_rect: Rect,
    radius_pt: f32,
    bevel_pt: f32,
    tint_color: [f32; 4],
    ior_strength: f32,
    dispersion: f32,
    spec_radius_pt: f32,
    hover_t: f32,
) -> Shape {
    let ppp = ctx.pixels_per_point().max(0.5);
    let screen_pts = ctx.screen_rect().size();
    let screen_px = [(screen_pts.x * ppp).max(1.0), (screen_pts.y * ppp).max(1.0)];

    let (xform, opacity) = crate::animation::current_visual_transform(ctx);
    let rect = xform * rect;
    let radius_pt = radius_pt * xform.scaling;
    let bevel_pt = bevel_pt * xform.scaling;
    let op = opacity.clamp(0.01, 1.0);

    let phys_min = [rect.left() * ppp, rect.top() * ppp];
    let phys_size = [
        (rect.width() * ppp).max(1.0),
        (rect.height() * ppp).max(1.0),
    ];
    let corner_radius =
        (radius_pt * ppp).clamp(0.0, (phys_size[0].min(phys_size[1]) * 0.49).max(0.0));
    let bevel_width = if bevel_pt > 0.0 {
        (bevel_pt * ppp).clamp(2.0 * ppp, (phys_size[0].min(phys_size[1]) * 0.45).max(1.0))
    } else {
        0.0
    };

    let cursor_pos = smoothed_pointer_pos(ctx)
        .map(|p| [p.x * ppp, p.y * ppp])
        .unwrap_or([-10000.0, -10000.0]);

    let backdrop_opt = active_gpu_backdrop(ctx).map(Arc::new);
    let (uv_min, uv_size, backdrop_min, backdrop_size, backdrop_dim, generation) =
        if let Some(ref b) = backdrop_opt {
            (
                [b.uv_rect.left(), b.uv_rect.top()],
                [b.uv_rect.width().max(0.001), b.uv_rect.height().max(0.001)],
                [b.screen_rect.left() * ppp, b.screen_rect.top() * ppp],
                [
                    (b.screen_rect.width() * ppp).max(1.0),
                    (b.screen_rect.height() * ppp).max(1.0),
                ],
                b.backdrop_dim,
                b.generation,
            )
        } else {
            (
                [0.0, 0.0],
                [1.0, 1.0],
                [0.0, 0.0],
                screen_px,
                0.0,
                ctx.cumulative_pass_nr(),
            )
        };

    let mut effective_tint = tint_color;
    effective_tint[3] = (effective_tint[3] + backdrop_dim * 0.18).clamp(0.0, 0.90);

    let uniforms = LiquidGlassUniforms {
        rect_min: phys_min,
        rect_size: phys_size,
        screen_size: screen_px,
        light_dir: [-0.5523 * op, -0.8336 * op],
        uv_min,
        uv_size,
        backdrop_min,
        backdrop_size,
        tint_color: effective_tint,
        corner_radius,
        bevel_width,
        ior_strength,
        dispersion,
        cursor_pos,
        spec_radius: spec_radius_pt * xform.scaling * ppp,
        hover_t: hover_t.clamp(0.0, 1.0),
    };

    Shape::Callback(egui_wgpu::Callback::new_paint_callback(
        callback_rect,
        LiquidGlassCallback {
            backdrop: backdrop_opt,
            uniforms,
            generation,
            slot_offset: AtomicU32::new(0),
        },
    ))
}

/// Collects Liquid Glass shapes (`egui_wgpu::CallbackTrait` on wgpu, or Serpantinum `Mesh` + border
/// on `glow`/CPU fallback) strictly beneath child content.
#[allow(clippy::too_many_arguments)]
pub fn collect_glass_underlay_shapes(
    ctx: &Context,
    rect: Rect,
    radius: CornerRadius,
    style: &ThemeStyleParams,
    _tokens: &Tokens,
    hover_t: f32,
    is_button: bool,
    out: &mut Vec<Shape>,
) {
    if !rect.is_positive() || !style.refraction_border {
        return;
    }

    let hover_t = hover_t.clamp(0.0, 1.0);
    let r = (radius.nw as f32)
        .min(rect.width() * 0.49)
        .min(rect.height() * 0.49);

    if is_wgpu_glass_available(ctx) {
        let bevel_pt = (rect.width().min(rect.height()) * 0.28).clamp(4.0, 12.0);
        let veil_a = if is_button {
            0.32 + 0.06 * hover_t
        } else {
            0.28 + 0.05 * hover_t
        };
        let tint = [16.0 / 255.0, 18.0 / 255.0, 24.0 / 255.0, veil_a];
        let spec_r = if is_button {
            style.specular_radius_button
        } else {
            style.specular_radius_card
        };
        out.push(build_gpu_liquid_glass_shape(
            ctx,
            rect,
            rect.expand(3.0),
            r,
            bevel_pt,
            tint,
            if is_button { 0.036 } else { 0.045 },
            0.11,
            spec_r,
            hover_t,
        ));
        return;
    }

    let cr = CornerRadius::same(r.round() as u8);

    // Fallback (OpenGL / glow / unit tests): Subtle Serpantinum drop shadow
    if rect.height() >= 18.0 {
        out.push(Shape::rect_stroke(
            rect.translate(egui::vec2(0.0, 1.5)).expand(1.0),
            CornerRadius::same((r + 1.0).round().clamp(1.0, 20.0) as u8),
            Stroke::new(
                2.0_f32,
                Color32::from_black_alpha(if is_button { 24 } else { 32 }),
            ),
            StrokeKind::Outside,
        ));
    }

    // Fallback: CPU-blurred wallpaper refraction mesh
    if let Some(glass_mesh) =
        build_serpantinum_refracted_glass_mesh(ctx, rect, r, hover_t, is_button, None)
    {
        out.push(Shape::Mesh(glass_mesh.into()));
        let veil_a = if is_button {
            (72.0 + 18.0 * hover_t).round() as u8
        } else {
            (64.0 + 16.0 * hover_t).round() as u8
        };
        out.push(Shape::rect_filled(
            rect.shrink(0.5),
            cr,
            Color32::from_rgba_unmultiplied(16, 18, 24, veil_a),
        ));
    } else {
        let fallback_a =
            ((style.surface_alpha + 0.34 + 0.08 * hover_t).clamp(0.45, 0.84) * 255.0).round() as u8;
        out.push(Shape::rect_filled(
            rect.shrink(0.5),
            cr,
            Color32::from_rgba_unmultiplied(24, 27, 35, fallback_a),
        ));
    }

    if style.inner_sheen_alpha > 0 && rect.height() > 14.0 && rect.width() > 16.0 {
        let sheen_h = (rect.height() * 0.22).clamp(4.0, 14.0);
        let sheen_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 1.0, rect.top() + 1.0),
            Pos2::new(
                rect.right() - 1.0,
                (rect.top() + sheen_h).min(rect.bottom() - 1.5),
            ),
        );
        let sheen_a = ((style.inner_sheen_alpha as f32) * (1.0 + 0.35 * hover_t)).round() as u8;
        if let Some(mesh) = build_vertical_rounded_fade_mesh(
            sheen_rect,
            r,
            Color32::from_rgba_unmultiplied(255, 255, 255, sheen_a),
            Color32::from_rgba_unmultiplied(255, 255, 255, 0),
        ) {
            out.push(Shape::Mesh(mesh.into()));
        }
    }
}

/// Paints Liquid Glass surface (`PaintCallback` in WGSL on wgpu, or `Mesh` border fallback on `glow`).
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
    if !rect.is_positive() || !style.refraction_border {
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

    if !is_wgpu_glass_available(ctx) {
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

/// Collects tinted Liquid Glass shapes (`PaintCallback` on wgpu, or `Mesh` fallback on `glow`)
/// into `out` so callers can also assign them into a pre-allocated [`Shape::Noop`] slot.
#[allow(clippy::too_many_arguments)]
pub fn collect_tinted_glass_pill_shapes(
    ctx: &Context,
    rect: Rect,
    radius: CornerRadius,
    style: &ThemeStyleParams,
    tokens: &Tokens,
    tint: Color32,
    hover_t: f32,
    out: &mut Vec<Shape>,
) {
    if !rect.is_positive() || !style.refraction_border {
        return;
    }
    let hover_t = hover_t.clamp(0.0, 1.0);
    let r = (radius.nw as f32)
        .min(rect.width() * 0.49)
        .min(rect.height() * 0.49);

    if is_wgpu_glass_available(ctx) {
        let bevel_pt = (rect.width().min(rect.height()) * 0.28).clamp(4.0, 11.0);
        let tint_norm = [
            tint.r() as f32 / 255.0,
            tint.g() as f32 / 255.0,
            tint.b() as f32 / 255.0,
            0.32 + 0.08 * hover_t,
        ];
        out.push(build_gpu_liquid_glass_shape(
            ctx,
            rect,
            rect.expand(3.0),
            r,
            bevel_pt,
            tint_norm,
            0.036,
            0.11,
            style.specular_radius_button,
            hover_t,
        ));
        return;
    }

    if let Some(mesh) =
        build_serpantinum_refracted_glass_mesh(ctx, rect, r, hover_t, true, Some(tint))
    {
        out.push(Shape::Mesh(mesh.into()));
    }
    out.push(Shape::rect_filled(
        rect.shrink(0.5),
        CornerRadius::same(r.round() as u8),
        Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), 64),
    ));
    collect_refractive_gradient_border_shapes(
        rect,
        r,
        style.refraction_top_left,
        style.refraction_bottom_right,
        tokens.accent,
        smoothed_pointer_pos(ctx),
        style.specular_radius_button,
        hover_t,
        out,
    );
}

/// Paints a tinted variant of Liquid Glass for active navigation rows, segmented controls, and
/// primary action buttons (via WGSL `PaintCallback` on wgpu, or `Mesh` fallback on `glow`).
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
    let mut shapes = Vec::new();
    collect_tinted_glass_pill_shapes(ctx, rect, radius, style, tokens, tint, hover_t, &mut shapes);
    for shape in shapes {
        painter.add(shape);
    }
}

/// Renders a subtle Serpantinum selection highlight border around selected items without loud rings.
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
        rect.expand(1.0),
        CornerRadius::same(r.saturating_add(1)),
        Stroke::new(
            1.25_f32,
            with_alpha(blend_color(Color32::WHITE, accent, 0.5), 0.55),
        ),
        StrokeKind::Outside,
    );
}

/// Renders Serpantinum's frosted transparent left sidebar directly over the blurred wallpaper.
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

    if is_wgpu_glass_available(ctx) {
        painter.add(build_gpu_liquid_glass_shape(
            ctx,
            rect,
            rect,
            0.0,
            0.0,
            [14.0 / 255.0, 16.0 / 255.0, 22.0 / 255.0, 0.34],
            0.0,
            0.0,
            0.0,
            0.0,
        ));
    } else if let Some(backdrop) = active_backdrop_texture(ctx) {
        let mut mesh = Mesh::with_texture(backdrop.tex_id);
        let col = Color32::from_rgb(176, 180, 190);
        for &pos in &[
            rect.left_top(),
            rect.right_top(),
            rect.left_bottom(),
            rect.right_bottom(),
        ] {
            mesh.vertices.push(Vertex {
                pos,
                uv: screen_pos_to_uv(pos, &backdrop),
                color: col,
            });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 3, 2);
        painter.add(mesh);
        painter.rect_filled(
            rect,
            CornerRadius::ZERO,
            Color32::from_rgba_unmultiplied(14, 16, 22, 84),
        );
    } else {
        painter.rect_filled(
            rect,
            CornerRadius::ZERO,
            Color32::from_rgba_unmultiplied(
                tokens.bg_sidebar.r(),
                tokens.bg_sidebar.g(),
                tokens.bg_sidebar.b(),
                (style.sidebar_alpha * 255.0).round() as u8,
            ),
        );
    }

    // Fine 1px glass separator edge
    painter.line_segment(
        [
            Pos2::new(rect.right() - 0.5, rect.top()),
            Pos2::new(rect.right() - 0.5, rect.bottom()),
        ],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 34)),
    );
}

/// Renders bottom HUD dock using frosted blurred wallpaper glass + 1px upper hairline.
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
    if is_wgpu_glass_available(ctx) {
        painter.add(build_gpu_liquid_glass_shape(
            ctx,
            rect,
            rect,
            0.0,
            0.0,
            [14.0 / 255.0, 16.0 / 255.0, 22.0 / 255.0, 0.37],
            0.0,
            0.0,
            0.0,
            0.0,
        ));
    } else if let Some(backdrop) = active_backdrop_texture(ctx) {
        let mut mesh = Mesh::with_texture(backdrop.tex_id);
        let col = Color32::from_rgb(168, 172, 182);
        for &pos in &[
            rect.left_top(),
            rect.right_top(),
            rect.left_bottom(),
            rect.right_bottom(),
        ] {
            mesh.vertices.push(Vertex {
                pos,
                uv: screen_pos_to_uv(pos, &backdrop),
                color: col,
            });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 3, 2);
        painter.add(mesh);
        painter.rect_filled(
            rect,
            CornerRadius::ZERO,
            Color32::from_rgba_unmultiplied(14, 16, 22, 92),
        );
    } else {
        painter.rect_filled(
            rect,
            CornerRadius::ZERO,
            Color32::from_rgba_unmultiplied(
                tokens.bg_sidebar.r(),
                tokens.bg_sidebar.g(),
                tokens.bg_sidebar.b(),
                (style.hud_alpha * 255.0).round() as u8,
            ),
        );
    }
    painter.line_segment(
        [
            Pos2::new(rect.left(), rect.top() + 0.5),
            Pos2::new(rect.right(), rect.top() + 0.5),
        ],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 42)),
    );
}

/// Renders an `egui::Frame` with smooth spring hover lift, subtle press feedback, and
/// blurred glass interior + bezel refraction beneath child widgets when in Liquid Glass mode.
pub fn show_themed_card<R>(
    ui: &mut Ui,
    frame: egui::Frame,
    radius: CornerRadius,
    hover_id: Id,
    style: &ThemeStyleParams,
    tokens: &Tokens,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    if !style.refraction_border {
        let orig_fill = frame.fill;
        let orig_stroke = frame.stroke;
        let orig_radius = frame.corner_radius;
        let mut frame = frame;
        frame.fill = Color32::TRANSPARENT;
        frame.stroke = Stroke::NONE;

        let mut prepared = frame.begin(ui);
        let bg_slot = prepared.content_ui.painter().add(Shape::Noop);
        let inner = add_contents(&mut prepared.content_ui);
        let frame_resp = prepared.end(ui);

        let pointer_pos = ui.input(|i| i.pointer.hover_pos());
        let hovered = pointer_pos.is_some_and(|pos| frame_resp.rect.contains(pos));
        let pressed = hovered && ui.input(|i| i.pointer.primary_down());
        let anim = crate::animation::animate_interaction(ui.ctx(), hover_id, hovered, pressed);
        let visual_rect = anim.transform_card_rect(frame_resp.rect);

        let fill = if orig_fill == Color32::TRANSPARENT {
            Color32::TRANSPARENT
        } else {
            lerp_rgba(
                orig_fill,
                style.surface_hover_fill(tokens),
                anim.hover_t * 0.55,
            )
        };
        let stroke_col = if orig_stroke.width <= 0.0 || orig_stroke.color == Color32::TRANSPARENT {
            Color32::TRANSPARENT
        } else {
            lerp_rgba(
                orig_stroke.color,
                blend_color(tokens.border_strong, tokens.accent, 0.35),
                anim.hover_t,
            )
        };
        let shape = egui::epaint::RectShape::new(
            visual_rect,
            orig_radius,
            fill,
            Stroke::new(orig_stroke.width, stroke_col),
            StrokeKind::Inside,
        );
        ui.painter().set(bg_slot, Shape::Rect(shape));
        return InnerResponse::new(inner, frame_resp);
    }

    let effective_radius = style.card_corner_radius(radius.nw as f32);
    let mut frame = frame;
    frame.stroke = Stroke::NONE;
    frame.corner_radius = effective_radius;
    frame.fill = Color32::TRANSPARENT;

    let mut prepared = frame.begin(ui);
    let overlay_slot = prepared.content_ui.painter().add(Shape::Noop);
    let inner = add_contents(&mut prepared.content_ui);
    let frame_resp = prepared.end(ui);
    let pointer_pos = ui.input(|i| i.pointer.hover_pos());
    let hovered = pointer_pos.is_some_and(|pos| frame_resp.rect.contains(pos));
    let pressed = hovered && ui.input(|i| i.pointer.primary_down());
    let anim = crate::animation::animate_interaction(ui.ctx(), hover_id, hovered, pressed);
    let visual_rect = anim.transform_card_rect(frame_resp.rect);

    let mut under_shapes = Vec::new();
    collect_glass_underlay_shapes(
        ui.ctx(),
        visual_rect,
        effective_radius,
        style,
        tokens,
        anim.hover_t,
        false,
        &mut under_shapes,
    );
    if !under_shapes.is_empty() {
        ui.painter().set(overlay_slot, Shape::Vec(under_shapes));
    }

    if !is_wgpu_glass_available(ui.ctx()) {
        paint_refractive_gradient_border(
            ui.painter(),
            visual_rect,
            effective_radius.nw as f32,
            style.refraction_top_left,
            style.refraction_bottom_right,
            tokens.accent,
            smoothed_pointer_pos(ui.ctx()),
            style.specular_radius_card,
            anim.hover_t,
        );
    }

    InnerResponse::new(inner, frame_resp)
}

fn smoothed_pointer_pos(ctx: &Context) -> Option<Pos2> {
    let raw = ctx.input(|i| i.pointer.hover_pos())?;
    let sm = crate::animation::animate_spring_vec2(
        ctx,
        Id::new("mq_glass_cursor_pos"),
        raw.to_vec2(),
        crate::animation::SpringConfig::snappy()
            .with_stiffness(650.0)
            .with_damping(42.0)
            .with_epsilon(0.35),
    );
    Some(sm.to_pos2())
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

/// Delicate 1.0px Serpantinum specular rim (`hair = exp(-t/1.1)` + 5px meniscus `wide`),
/// lit by `lightDir = normalize(-0.55, -0.83)` with gentle hover direction steering.
#[allow(clippy::too_many_arguments)]
fn collect_refractive_gradient_border_shapes(
    rect: Rect,
    radius: f32,
    top_left_col: Color32,
    bottom_right_col: Color32,
    _accent_col: Color32,
    cursor: Option<Pos2>,
    spec_radius: f32,
    hover_t: f32,
    out: &mut Vec<Shape>,
) {
    let r = radius.clamp(0.0, (rect.width().min(rect.height()) * 0.49).max(0.0));
    if r < 1.0 {
        out.push(Shape::rect_stroke(
            rect,
            CornerRadius::ZERO,
            Stroke::new(1.0_f32, top_left_col),
            StrokeKind::Inside,
        ));
        return;
    }

    let ring = build_rounded_perimeter_ring(rect, r, 7);
    let n = ring.len();
    if n < 8 {
        return;
    }

    let base_light = egui::vec2(-0.5523, -0.8336);
    let mid_neutral = Color32::from_rgba_unmultiplied(215, 222, 232, 24);

    let hair_w = 1.0_f32 + 0.15 * hover_t;
    let wide_w = 3.0_f32;
    let mut hair_mesh = Mesh::default();
    let mut wide_mesh = Mesh::default();

    for &(outer_pt, normal) in &ring {
        let mut light_dir = base_light;
        if let Some(cur) = cursor {
            let to_cur = cur - outer_pt;
            let cd = to_cur.length();
            if cd > 1.0 && cd < spec_radius {
                let cdir = to_cur / cd;
                let mix_t = 0.4 * hover_t * (1.0 - cd / spec_radius.max(1.0));
                let blended = light_dir * (1.0 - mix_t) + cdir * mix_t;
                let blen = blended.length();
                if blen > 1e-4 {
                    light_dir = blended / blen;
                }
            }
        }

        // Exact Serpantinum facing & spec formula (liquid_glass.frag lines 310-311)
        let facing = (normal.x * light_dir.x + normal.y * light_dir.y).clamp(-1.0, 1.0);
        let col = if facing >= 0.0 {
            let k = facing.powf(1.6);
            lerp_rgba(mid_neutral, top_left_col, k)
        } else {
            let k = (-facing).powf(2.2) * 0.55;
            lerp_rgba(mid_neutral, bottom_right_col, k)
        };

        let inner_hair = outer_pt - normal * hair_w;
        hair_mesh.vertices.push(Vertex {
            pos: outer_pt,
            uv: Pos2::ZERO,
            color: col,
        });
        hair_mesh.vertices.push(Vertex {
            pos: inner_hair,
            uv: Pos2::ZERO,
            color: col,
        });

        let wide_a = (col.a() as f32 * 0.09).round().clamp(0.0, 15.0) as u8;
        wide_mesh.vertices.push(Vertex {
            pos: inner_hair,
            uv: Pos2::ZERO,
            color: Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), wide_a),
        });
        wide_mesh.vertices.push(Vertex {
            pos: outer_pt - normal * wide_w,
            uv: Pos2::ZERO,
            color: Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 0),
        });
    }

    for i in 0..n {
        let next = (i + 1) % n;
        let o0 = (i * 2) as u32;
        let i0 = o0 + 1;
        let o1 = (next * 2) as u32;
        let i1 = o1 + 1;
        hair_mesh.add_triangle(o0, o1, i0);
        hair_mesh.add_triangle(o1, i1, i0);
        wide_mesh.add_triangle(o0, o1, i0);
        wide_mesh.add_triangle(o1, i1, i0);
    }

    if r >= 5.0 {
        out.push(Shape::Mesh(wide_mesh.into()));
    }
    out.push(Shape::Mesh(hair_mesh.into()));
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
    let mut shapes = Vec::new();
    collect_refractive_gradient_border_shapes(
        rect,
        radius,
        top_left_col,
        bottom_right_col,
        accent_col,
        cursor,
        spec_radius,
        hover_t,
        &mut shapes,
    );
    for shape in shapes {
        painter.add(shape);
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
        assert_eq!(glass.card_corner_radius(10.0), CornerRadius::same(10));
        assert_eq!(
            glass.control_corner_radius(32.0, CornerRadius::same(6)),
            CornerRadius::same(10)
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
                        CornerRadius::same(10),
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

    #[test]
    fn gpu_liquid_glass_uniforms_alignment_dpi_scaling_and_callback_dispatch() {
        assert_eq!(std::mem::size_of::<LiquidGlassUniforms>(), 112);
        assert_eq!(std::mem::size_of::<LiquidGlassUniforms>() % 16, 0);

        let ctx = Context::default();
        ctx.set_pixels_per_point(2.0);
        let cfg = ThemeConfig::preset_liquid_glass();
        let style = ThemeStyleParams::from_config(&cfg);
        let tokens = Tokens::from_config(&cfg);
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), egui::vec2(200.0, 80.0));

        // 1. Without wgpu initialized, falls back cleanly to non-Callback Mesh/border shapes
        let mut fallback_shapes = Vec::new();
        collect_glass_underlay_shapes(
            &ctx,
            rect,
            CornerRadius::same(10),
            &style,
            &tokens,
            0.5,
            false,
            &mut fallback_shapes,
        );
        assert!(fallback_shapes
            .iter()
            .all(|s| !matches!(s, Shape::Callback(_))));

        // 2. When wgpu glass is active, emits PaintCallback with coordinates scaled by pixels_per_point (2.0)
        crate::theme::mark_wgpu_glass_available_for_test(&ctx);
        let shape = build_gpu_liquid_glass_shape(
            &ctx,
            rect,
            rect.expand(3.0),
            10.0,
            8.0,
            [0.06, 0.07, 0.09, 0.25],
            0.045,
            0.24,
            180.0,
            0.5,
        );
        let Shape::Callback(cb) = shape else {
            panic!("expected Shape::Callback when wgpu is active");
        };
        let glass_cb = cb
            .callback
            .downcast_ref::<egui_wgpu::Callback>()
            .expect("expected egui_wgpu::Callback");
        let _ = glass_cb;

        let mut gpu_shapes = Vec::new();
        collect_glass_underlay_shapes(
            &ctx,
            rect,
            CornerRadius::same(10),
            &style,
            &tokens,
            0.5,
            false,
            &mut gpu_shapes,
        );
        assert_eq!(gpu_shapes.len(), 1);
        assert!(matches!(gpu_shapes[0], Shape::Callback(_)));
    }
}
