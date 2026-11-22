//! Matte Dark solid design system tokens and theme engine.
//! Zero transparency, pure solid surfaces, 4/8px spatial rhythm, 8px card radius, 6px button radius.
//! Strictly enforces Section 2 rules: no blurs, no glassy sheens, no glow borders, contrast >= 4.5:1.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use egui::epaint::{Mesh, Vertex};
use egui::style::{Selection, WidgetVisuals, Widgets};
use egui::{
    Color32, ColorImage, Context, CornerRadius, FontId, Frame, Id, Margin, Painter, Pos2, Rect,
    Stroke, Style, TextStyle, TextureHandle, TextureOptions, Vec2, Visuals,
};
use mq_core::{BackgroundStyle, ThemeConfig, ThemePreset, UiDensity};

use crate::theme_styles::{
    paint_translucent_clean_backdrop, store_active_backdrop_texture, with_alpha,
    ActiveBackdropTexture, ThemeStyleParams,
};

/// Curated calm accent swatch preset for customization controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccentPreset {
    pub name: &'static str,
    pub primary: [u8; 3],
    pub secondary: [u8; 3],
}

/// Calm, balanced accent presets that avoid harsh neon saturation.
pub const CALM_ACCENT_PRESETS: [AccentPreset; 6] = [
    AccentPreset {
        name: "Шалфей",
        primary: [0x4C, 0x9A, 0x82],
        secondary: [0x3D, 0x83, 0x6D],
    },
    AccentPreset {
        name: "Грифельно-синий",
        primary: [0x5B, 0x8D, 0xEF],
        secondary: [0x46, 0x74, 0xD0],
    },
    AccentPreset {
        name: "Нордический льдистый",
        primary: [0x6E, 0xA8, 0xB8],
        secondary: [0x56, 0x8E, 0x9E],
    },
    AccentPreset {
        name: "Янтарь",
        primary: [0xC9, 0x8A, 0x4B],
        secondary: [0xAA, 0x71, 0x3A],
    },
    AccentPreset {
        name: "Лавандовый графит",
        primary: [0x8B, 0x86, 0xC6],
        secondary: [0x71, 0x6C, 0xAA],
    },
    AccentPreset {
        name: "Терракота",
        primary: [0xC2, 0x6D, 0x68],
        secondary: [0xA4, 0x56, 0x52],
    },
];

/// Linearly blends two opaque RGB colors (`t = 0.0` returns `a`, `t = 1.0` returns `b`).
#[inline]
pub fn blend_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    Color32::from_rgb(
        ((a.r() as f32) * inv + (b.r() as f32) * t).round() as u8,
        ((a.g() as f32) * inv + (b.g() as f32) * t).round() as u8,
        ((a.b() as f32) * inv + (b.b() as f32) * t).round() as u8,
    )
}

/// Computes W3C relative luminance (`0.0..=1.0`) for an sRGB color.
#[inline]
pub fn relative_luminance(c: Color32) -> f32 {
    fn channel(v: u8) -> f32 {
        let s = (v as f32) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// Picks a high-contrast foreground color (dark charcoal or crisp off-white) on `bg`.
#[inline]
pub fn contrast_text_for(bg: Color32) -> Color32 {
    if relative_luminance(bg) > 0.24 {
        Color32::from_rgb(0x08, 0x13, 0x10)
    } else {
        Color32::from_rgb(0xF6, 0xF8, 0xFA)
    }
}

/// Comprehensive opaque design tokens for MQ v2.1.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tokens {
    /// Window background (#0F1115)
    pub bg_app: Color32,
    /// Sidebar & top navigation surface (#14171D)
    pub bg_sidebar: Color32,
    /// Cards, panels, elevated surfaces (#1A1E26)
    pub bg_surface: Color32,
    /// Surface hover state (#212631)
    pub bg_surface_hover: Color32,
    /// Text input and log viewer backgrounds (#12151B)
    pub bg_input: Color32,
    /// Structural 1px border (#272C37)
    pub border: Color32,
    /// Hover / focus 1px border (#363D4C)
    pub border_strong: Color32,
    /// Primary high-contrast text (#E6E8EE)
    pub text: Color32,
    /// Secondary muted text (#9AA1B2)
    pub text_muted: Color32,
    /// Disabled control text (#5B6274)
    pub text_disabled: Color32,
    /// Primary singular accent (#4C9A82)
    pub accent: Color32,
    /// Accent hover state (#3D836D)
    pub accent_hover: Color32,
    /// Text on accent backgrounds (#081310)
    pub accent_text: Color32,
    /// Danger / error status (#E5484D)
    pub danger: Color32,
    /// Warning status (#F5A524)
    pub warning: Color32,
    /// Success status (#4C9A82)
    pub success: Color32,
}

impl Default for Tokens {
    fn default() -> Self {
        Self::matte_dark()
    }
}

impl Tokens {
    /// Matte Dark (Default) — deep obsidian graphite with calm sage teal accent.
    pub const fn matte_dark() -> Self {
        Self {
            bg_app: Color32::from_rgb(0x0F, 0x11, 0x15),
            bg_sidebar: Color32::from_rgb(0x14, 0x17, 0x1D),
            bg_surface: Color32::from_rgb(0x1A, 0x1E, 0x26),
            bg_surface_hover: Color32::from_rgb(0x21, 0x26, 0x31),
            bg_input: Color32::from_rgb(0x12, 0x15, 0x1B),
            border: Color32::from_rgb(0x27, 0x2C, 0x37),
            border_strong: Color32::from_rgb(0x36, 0x3D, 0x4C),
            text: Color32::from_rgb(0xE6, 0xE8, 0xEE),
            text_muted: Color32::from_rgb(0x9A, 0xA1, 0xB2),
            text_disabled: Color32::from_rgb(0x5B, 0x62, 0x74),
            accent: Color32::from_rgb(0x4C, 0x9A, 0x82),
            accent_hover: Color32::from_rgb(0x3D, 0x83, 0x6D),
            accent_text: Color32::from_rgb(0x08, 0x13, 0x10),
            danger: Color32::from_rgb(0xE5, 0x48, 0x4D),
            warning: Color32::from_rgb(0xF5, 0xA5, 0x24),
            success: Color32::from_rgb(0x4C, 0x9A, 0x82),
        }
    }

    /// Prism Classic Dark — dark neutral slate with classic blue accent.
    pub const fn prism_classic_dark() -> Self {
        Self {
            bg_app: Color32::from_rgb(0x19, 0x19, 0x19),
            bg_sidebar: Color32::from_rgb(0x1F, 0x1F, 0x1F),
            bg_surface: Color32::from_rgb(0x24, 0x24, 0x24),
            bg_surface_hover: Color32::from_rgb(0x2E, 0x2E, 0x2E),
            bg_input: Color32::from_rgb(0x14, 0x14, 0x14),
            border: Color32::from_rgb(0x33, 0x33, 0x33),
            border_strong: Color32::from_rgb(0x44, 0x44, 0x44),
            text: Color32::from_rgb(0xEA, 0xEA, 0xEA),
            text_muted: Color32::from_rgb(0xA0, 0xA0, 0xA0),
            text_disabled: Color32::from_rgb(0x60, 0x60, 0x60),
            accent: Color32::from_rgb(0x5B, 0x8D, 0xEF),
            accent_hover: Color32::from_rgb(0x4A, 0x7C, 0xD8),
            accent_text: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            danger: Color32::from_rgb(0xE5, 0x48, 0x4D),
            warning: Color32::from_rgb(0xF5, 0xA5, 0x24),
            success: Color32::from_rgb(0x4C, 0x9A, 0x82),
        }
    }

    /// Modrinth Emerald — dark carbon with calm botanical emerald accent.
    pub const fn modrinth_emerald() -> Self {
        Self {
            bg_app: Color32::from_rgb(0x11, 0x12, 0x15),
            bg_sidebar: Color32::from_rgb(0x16, 0x18, 0x1D),
            bg_surface: Color32::from_rgb(0x1C, 0x1F, 0x26),
            bg_surface_hover: Color32::from_rgb(0x23, 0x27, 0x31),
            bg_input: Color32::from_rgb(0x0D, 0x0E, 0x11),
            border: Color32::from_rgb(0x2A, 0x2E, 0x39),
            border_strong: Color32::from_rgb(0x39, 0x3F, 0x4E),
            text: Color32::from_rgb(0xED, 0xED, 0xED),
            text_muted: Color32::from_rgb(0x9D, 0xA3, 0xAF),
            text_disabled: Color32::from_rgb(0x5E, 0x64, 0x72),
            accent: Color32::from_rgb(0x2E, 0x9E, 0x6B),
            accent_hover: Color32::from_rgb(0x24, 0x83, 0x57),
            accent_text: Color32::from_rgb(0x07, 0x16, 0x0F),
            danger: Color32::from_rgb(0xE5, 0x48, 0x4D),
            warning: Color32::from_rgb(0xF5, 0xA5, 0x24),
            success: Color32::from_rgb(0x2E, 0x9E, 0x6B),
        }
    }

    /// Catppuccin Mocha — cozy pastel dark palette.
    pub const fn catppuccin_mocha() -> Self {
        Self {
            bg_app: Color32::from_rgb(0x1E, 0x1E, 0x2E),
            bg_sidebar: Color32::from_rgb(0x18, 0x18, 0x25),
            bg_surface: Color32::from_rgb(0x31, 0x32, 0x44),
            bg_surface_hover: Color32::from_rgb(0x45, 0x47, 0x5A),
            bg_input: Color32::from_rgb(0x11, 0x11, 0x1B),
            border: Color32::from_rgb(0x45, 0x47, 0x5A),
            border_strong: Color32::from_rgb(0x58, 0x5B, 0x70),
            text: Color32::from_rgb(0xCD, 0xD6, 0xF4),
            text_muted: Color32::from_rgb(0xA6, 0xAD, 0xC8),
            text_disabled: Color32::from_rgb(0x6C, 0x70, 0x86),
            accent: Color32::from_rgb(0x89, 0xB4, 0xFA),
            accent_hover: Color32::from_rgb(0x74, 0xC7, 0xEC),
            accent_text: Color32::from_rgb(0x11, 0x11, 0x1B),
            danger: Color32::from_rgb(0xF3, 0x8B, 0xA8),
            warning: Color32::from_rgb(0xFA, 0xB3, 0x87),
            success: Color32::from_rgb(0xA6, 0xE3, 0xA1),
        }
    }

    /// Nord — arctic cool north palette.
    pub const fn nord() -> Self {
        Self {
            bg_app: Color32::from_rgb(0x24, 0x29, 0x33),
            bg_sidebar: Color32::from_rgb(0x2E, 0x34, 0x40),
            bg_surface: Color32::from_rgb(0x3B, 0x42, 0x52),
            bg_surface_hover: Color32::from_rgb(0x43, 0x4C, 0x5E),
            bg_input: Color32::from_rgb(0x1D, 0x21, 0x2A),
            border: Color32::from_rgb(0x4C, 0x56, 0x6A),
            border_strong: Color32::from_rgb(0x5E, 0x81, 0xAC),
            text: Color32::from_rgb(0xEC, 0xEF, 0xF4),
            text_muted: Color32::from_rgb(0xD8, 0xDE, 0xE9),
            text_disabled: Color32::from_rgb(0x7B, 0x88, 0xA1),
            accent: Color32::from_rgb(0x88, 0xC0, 0xD0),
            accent_hover: Color32::from_rgb(0x81, 0xA1, 0xC1),
            accent_text: Color32::from_rgb(0x2E, 0x34, 0x40),
            danger: Color32::from_rgb(0xBF, 0x61, 0x6A),
            warning: Color32::from_rgb(0xEB, 0xCB, 0x8B),
            success: Color32::from_rgb(0xA3, 0xBE, 0x8C),
        }
    }

    /// Liquid Glass (`Serpantinum v2.5.1` style) — neutral charcoal base with frosted crystal clarity.
    pub const fn liquid_glass() -> Self {
        Self {
            bg_app: Color32::from_rgb(0x10, 0x10, 0x14),
            bg_sidebar: Color32::from_rgb(0x16, 0x17, 0x1C),
            bg_surface: Color32::from_rgb(0x1A, 0x1C, 0x24),
            bg_surface_hover: Color32::from_rgb(0x25, 0x28, 0x34),
            bg_input: Color32::from_rgb(0x14, 0x16, 0x1C),
            border: Color32::from_rgb(0x38, 0x3D, 0x4A),
            border_strong: Color32::from_rgb(0x58, 0x60, 0x73),
            text: Color32::from_rgb(0xF2, 0xF5, 0xFA),
            text_muted: Color32::from_rgb(0xB2, 0xBC, 0xCB),
            text_disabled: Color32::from_rgb(0x6D, 0x75, 0x85),
            accent: Color32::from_rgb(0xA6, 0xC6, 0xE8),
            accent_hover: Color32::from_rgb(0x82, 0xA8, 0xCE),
            accent_text: Color32::from_rgb(0x0D, 0x10, 0x16),
            danger: Color32::from_rgb(0xEB, 0x5A, 0x65),
            warning: Color32::from_rgb(0xF5, 0xAD, 0x36),
            success: Color32::from_rgb(0x52, 0xC4, 0x8E),
        }
    }

    /// Translucent — neutral dark tint for clean compositor-level window transparency without glares.
    pub const fn translucent() -> Self {
        Self {
            bg_app: Color32::from_rgb(0x10, 0x13, 0x1A),
            bg_sidebar: Color32::from_rgb(0x14, 0x18, 0x21),
            bg_surface: Color32::from_rgb(0x1A, 0x20, 0x2B),
            bg_surface_hover: Color32::from_rgb(0x23, 0x2A, 0x38),
            bg_input: Color32::from_rgb(0x12, 0x16, 0x1E),
            border: Color32::from_rgb(0x2E, 0x36, 0x46),
            border_strong: Color32::from_rgb(0x45, 0x50, 0x66),
            text: Color32::from_rgb(0xF0, 0xF3, 0xF8),
            text_muted: Color32::from_rgb(0xA8, 0xB3, 0xC5),
            text_disabled: Color32::from_rgb(0x66, 0x70, 0x84),
            accent: Color32::from_rgb(0x56, 0xA3, 0x96),
            accent_hover: Color32::from_rgb(0x42, 0x88, 0x7C),
            accent_text: Color32::from_rgb(0x07, 0x14, 0x12),
            danger: Color32::from_rgb(0xE5, 0x48, 0x4D),
            warning: Color32::from_rgb(0xF5, 0xA5, 0x24),
            success: Color32::from_rgb(0x4C, 0x9A, 0x82),
        }
    }

    /// Light Clean — high-contrast clean daylight theme.
    pub const fn light_clean() -> Self {
        Self {
            bg_app: Color32::from_rgb(0xF4, 0xF5, 0xF7),
            bg_sidebar: Color32::from_rgb(0xEB, 0xED, 0xF0),
            bg_surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            bg_surface_hover: Color32::from_rgb(0xF0, 0xF2, 0xF5),
            bg_input: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            border: Color32::from_rgb(0xD1, 0xD5, 0xDB),
            border_strong: Color32::from_rgb(0x9C, 0xA3, 0xAF),
            text: Color32::from_rgb(0x11, 0x18, 0x27),
            text_muted: Color32::from_rgb(0x4B, 0x55, 0x63),
            text_disabled: Color32::from_rgb(0x9C, 0xA3, 0xAF),
            accent: Color32::from_rgb(0x05, 0x96, 0x69),
            accent_hover: Color32::from_rgb(0x04, 0x78, 0x57),
            accent_text: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            danger: Color32::from_rgb(0xDC, 0x26, 0x26),
            warning: Color32::from_rgb(0xD9, 0x77, 0x06),
            success: Color32::from_rgb(0x05, 0x96, 0x69),
        }
    }

    /// Resolves `Tokens` for a given preset.
    pub fn from_preset(preset: ThemePreset) -> Self {
        match preset {
            ThemePreset::MatteDark => Self::matte_dark(),
            ThemePreset::LiquidGlass => Self::liquid_glass(),
            ThemePreset::Translucent => Self::translucent(),
            ThemePreset::PrismClassicDark => Self::prism_classic_dark(),
            ThemePreset::ModrinthEmerald => Self::modrinth_emerald(),
            ThemePreset::CatppuccinMocha => Self::catppuccin_mocha(),
            ThemePreset::Nord => Self::nord(),
            ThemePreset::LightClean => Self::light_clean(),
            ThemePreset::Custom => Self::matte_dark(),
        }
    }

    /// Resolves `Tokens` from an active [`ThemeConfig`], honoring custom accent and surface colors.
    pub fn from_config(cfg: &ThemeConfig) -> Self {
        let bg_app = rgb_to_c32(cfg.bg_window);
        let is_light_bg = relative_luminance(bg_app) > 0.5;
        let mut tokens = match cfg.preset {
            ThemePreset::Custom => {
                if is_light_bg {
                    Self::light_clean()
                } else {
                    Self::matte_dark()
                }
            }
            other => Self::from_preset(other),
        };

        let accent = rgb_to_c32(cfg.accent_primary);
        let accent_hover = if cfg.accent_secondary == cfg.accent_primary {
            let mix_target = if is_light_bg {
                Color32::BLACK
            } else {
                Color32::from_rgb(0x0A, 0x10, 0x14)
            };
            blend_color(accent, mix_target, 0.16)
        } else {
            rgb_to_c32(cfg.accent_secondary)
        };
        tokens.accent = accent;
        tokens.accent_hover = accent_hover;
        tokens.accent_text = contrast_text_for(accent);
        tokens.success = rgb_to_c32(cfg.status_success);

        if cfg.preset == ThemePreset::Custom {
            let bg_surface = rgb_to_c32(cfg.bg_card);
            let text = rgb_to_c32(cfg.text_primary);
            let text_muted = rgb_to_c32(cfg.text_muted);
            tokens.bg_app = bg_app;
            tokens.bg_surface = bg_surface;
            tokens.bg_sidebar = blend_color(bg_app, bg_surface, 0.45);
            tokens.bg_surface_hover = blend_color(
                bg_surface,
                if is_light_bg {
                    Color32::BLACK
                } else {
                    Color32::WHITE
                },
                0.06,
            );
            tokens.bg_input = blend_color(
                bg_app,
                Color32::BLACK,
                if is_light_bg { 0.02 } else { 0.12 },
            );
            tokens.border = blend_color(bg_surface, text_muted, 0.22);
            tokens.border_strong = blend_color(bg_surface, text_muted, 0.42);
            tokens.text = text;
            tokens.text_muted = text_muted;
            tokens.text_disabled = blend_color(bg_surface, text_muted, 0.55);
            tokens.danger = rgb_to_c32(cfg.status_error);
        }

        tokens
    }

    /// Reads the active [`Tokens`] stored by [`apply_theme`], falling back to dark/light defaults.
    pub fn active(ctx: &Context) -> Self {
        ctx.data(|d| d.get_temp::<Self>(Id::new("mq_active_tokens")))
            .unwrap_or_else(|| {
                if ctx.style().visuals.dark_mode {
                    Self::matte_dark()
                } else {
                    Self::light_clean()
                }
            })
    }

    /// Returns `true` when the active background is light.
    #[inline]
    pub fn is_light(&self) -> bool {
        relative_luminance(self.bg_app) > 0.5
    }
}

// Global tokens constants for Matte Dark (backward-compatible and direct token access)
pub const BG_APP: Color32 = Color32::from_rgb(0x0F, 0x11, 0x15);
pub const BG_SIDEBAR: Color32 = Color32::from_rgb(0x14, 0x17, 0x1D);
pub const BG_SURFACE: Color32 = Color32::from_rgb(0x1A, 0x1E, 0x26);
pub const BG_SURFACE_HOVER: Color32 = Color32::from_rgb(0x21, 0x26, 0x31);
pub const BG_INPUT: Color32 = Color32::from_rgb(0x12, 0x15, 0x1B);
pub const BORDER: Color32 = Color32::from_rgb(0x27, 0x2C, 0x37);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x36, 0x3D, 0x4C);
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xE6, 0xE8, 0xEE);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x9A, 0xA1, 0xB2);
pub const TEXT_DISABLED: Color32 = Color32::from_rgb(0x5B, 0x62, 0x74);
pub const ACCENT: Color32 = Color32::from_rgb(0x4C, 0x9A, 0x82);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x3D, 0x83, 0x6D);
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0x08, 0x13, 0x10);
pub const STATUS_DANGER: Color32 = Color32::from_rgb(0xE5, 0x48, 0x4D);
pub const STATUS_WARN: Color32 = Color32::from_rgb(0xF5, 0xA5, 0x24);
pub const STATUS_SUCCESS: Color32 = Color32::from_rgb(0x4C, 0x9A, 0x82);

// Legacy aliases mapped to solid tokens
pub const BG_DEEP_GRAPHITE: Color32 = BG_APP;
pub const PANEL_DARK_GLASS: Color32 = BG_SIDEBAR;
pub const CARD_SURFACE: Color32 = BG_SURFACE;
pub const CARD_SURFACE_HOVER: Color32 = BG_SURFACE_HOVER;
pub const BG_RECESSED: Color32 = BG_INPUT;
pub const BORDER_SUBTLE: Color32 = BORDER;
pub const BORDER_HOVER: Color32 = BORDER_STRONG;
pub const ACCENT_EMERALD: Color32 = ACCENT;
pub const ACCENT_EMERALD_HOVER: Color32 = ACCENT_HOVER;
pub const ACCENT_COBALT: Color32 = Color32::from_rgb(0x5B, 0x8D, 0xEF);

// 4/8 px Spacing and radius rules (Section 2.2)
pub const RADIUS_CARD: CornerRadius = CornerRadius::same(8);
pub const RADIUS_BUTTON: CornerRadius = CornerRadius::same(6);
pub const RADIUS_TAG: CornerRadius = CornerRadius::same(4);

pub const ITEM_SPACING: Vec2 = egui::vec2(8.0, 8.0);
pub const WINDOW_MARGIN: Margin = Margin::same(16);
pub const BUTTON_PADDING: Vec2 = egui::vec2(12.0, 6.0);

/// Builds dynamic [`Visuals`] for the active theme configuration.
pub fn build_visuals_from_config(cfg: &ThemeConfig) -> Visuals {
    let tokens = Tokens::from_config(cfg);
    let style = ThemeStyleParams::from_config(cfg);
    let is_light = tokens.is_light();
    let mut visuals = if is_light {
        Visuals::light()
    } else {
        Visuals::dark()
    };

    let card_r_u8 = crate::state::safe_clamp_f32(cfg.corner_radius, 0.0, 16.0).round() as u8;
    let btn_r_u8 =
        (crate::state::safe_clamp_f32(cfg.corner_radius, 0.0, 16.0) * 0.75).round() as u8;
    let card_radius = style.card_corner_radius(card_r_u8 as f32);
    let btn_radius = style.control_corner_radius(32.0, CornerRadius::same(btn_r_u8.min(12)));

    let win_fill = style.window_fill(&tokens);
    let surf_fill = style.surface_fill(&tokens);
    let surf_hover = style.surface_hover_fill(&tokens);
    let inp_fill = style.input_fill(&tokens);
    let border_col = if style.refraction_border {
        blend_color(tokens.border, Color32::WHITE, 0.22)
    } else {
        tokens.border
    };

    visuals.dark_mode = !is_light;
    visuals.override_text_color = Some(tokens.text);
    visuals.panel_fill = win_fill;
    visuals.window_fill = style.modal_fill(&tokens);
    visuals.faint_bg_color = surf_fill;
    visuals.extreme_bg_color = inp_fill;
    visuals.code_bg_color = inp_fill;

    visuals.window_corner_radius = card_radius;
    visuals.menu_corner_radius = card_radius;
    visuals.window_stroke = Stroke::new(1.0_f32, border_col);

    visuals.hyperlink_color = tokens.accent;
    visuals.warn_fg_color = tokens.warning;
    visuals.error_fg_color = tokens.danger;

    let sel_bg = blend_color(tokens.bg_surface_hover, tokens.accent, 0.25);
    visuals.selection = Selection {
        bg_fill: sel_bg,
        stroke: Stroke::new(1.2_f32, tokens.text),
    };

    visuals.widgets = Widgets {
        noninteractive: WidgetVisuals {
            weak_bg_fill: surf_fill,
            bg_fill: surf_fill,
            bg_stroke: Stroke::new(1.0_f32, border_col),
            fg_stroke: Stroke::new(1.0_f32, tokens.text),
            corner_radius: card_radius,
            expansion: 0.0,
        },
        inactive: WidgetVisuals {
            weak_bg_fill: surf_fill,
            bg_fill: crate::theme_styles::with_alpha(
                blend_color(
                    tokens.bg_input,
                    tokens.border_strong,
                    if is_light { 0.50 } else { 0.40 },
                ),
                style.surface_alpha.max(0.68),
            ),
            bg_stroke: Stroke::new(1.0_f32, border_col),
            fg_stroke: Stroke::new(1.0_f32, tokens.text),
            corner_radius: btn_radius,
            expansion: 0.0,
        },
        hovered: WidgetVisuals {
            weak_bg_fill: surf_hover,
            bg_fill: surf_hover,
            bg_stroke: Stroke::new(1.0_f32, tokens.border_strong),
            fg_stroke: Stroke::new(1.5_f32, tokens.text),
            corner_radius: btn_radius,
            expansion: 0.0,
        },
        active: WidgetVisuals {
            weak_bg_fill: surf_hover,
            bg_fill: surf_hover,
            bg_stroke: Stroke::new(1.5_f32, tokens.accent),
            fg_stroke: Stroke::new(1.5_f32, tokens.text),
            corner_radius: btn_radius,
            expansion: 0.0,
        },
        open: WidgetVisuals {
            weak_bg_fill: surf_hover,
            bg_fill: surf_hover,
            bg_stroke: Stroke::new(1.0_f32, tokens.border_strong),
            fg_stroke: Stroke::new(1.0_f32, tokens.text_muted),
            corner_radius: btn_radius,
            expansion: 0.0,
        },
    };

    visuals
}

/// Builds dynamic [`Style`] for the active theme configuration.
pub fn build_style_from_config(cfg: &ThemeConfig) -> Style {
    let mut style = Style {
        visuals: build_visuals_from_config(cfg),
        ..Style::default()
    };

    let (item_sp, btn_pad) = match cfg.density {
        UiDensity::Compact => (Vec2::new(6.0, 6.0), Vec2::new(10.0, 4.0)),
        UiDensity::Comfortable => (Vec2::new(8.0, 8.0), Vec2::new(12.0, 6.0)),
        UiDensity::Spacious => (Vec2::new(12.0, 12.0), Vec2::new(16.0, 8.0)),
    };

    style.spacing.item_spacing = item_sp;
    style.spacing.window_margin = WINDOW_MARGIN;
    style.spacing.button_padding = btn_pad;
    style.spacing.interact_size = egui::vec2(36.0, 32.0);
    style.spacing.indent = 16.0;

    let base_font = crate::state::safe_clamp_f32(cfg.font_size, 11.0, 20.0);
    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(base_font));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::proportional(base_font));
    style.text_styles.insert(
        TextStyle::Small,
        FontId::proportional((base_font - 2.5).max(10.0)),
    );
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(base_font + 5.0));
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::monospace((base_font - 1.0).max(10.0)),
    );

    style
}

/// Applies dynamic theme configuration to [`egui::Context`].
pub fn apply_theme(ctx: &Context, cfg: &ThemeConfig) {
    let tokens = Tokens::from_config(cfg);
    let style_params = ThemeStyleParams::from_config(cfg);
    let style = build_style_from_config(cfg);
    ctx.set_visuals(style.visuals.clone());
    ctx.set_style(style);
    ctx.set_zoom_factor(crate::state::safe_clamp_f32(cfg.ui_scale, 0.8, 1.5));
    ctx.data_mut(|d| {
        d.insert_temp(Id::new("mq_active_tokens"), tokens);
        d.insert_temp(Id::new("mq_active_theme_style"), style_params);
    });
}

/// Solid opaque card frame (8px radius, 1px border, surface background).
pub fn solid_card_frame(tokens: &Tokens) -> Frame {
    Frame::new()
        .fill(tokens.bg_surface)
        .stroke(Stroke::new(1.0_f32, tokens.border))
        .corner_radius(RADIUS_CARD)
        .inner_margin(Margin::same(12))
}

/// Configured card frame honoring [`ThemeConfig::card_opacity`], [`ThemeConfig::corner_radius`],
/// [`ThemeConfig::border_glow`], and active [`ThemeStyleParams`].
pub fn solid_card_frame_with_config(tokens: &Tokens, cfg: &ThemeConfig) -> Frame {
    let style = ThemeStyleParams::from_config(cfg);
    let fill = style.surface_fill(tokens);
    let glow = crate::state::safe_clamp_f32(cfg.border_glow, 0.0, 1.0);
    let border_col = if style.refraction_border {
        Color32::TRANSPARENT
    } else if glow > 0.01 {
        blend_color(tokens.border, tokens.accent, glow * 0.45)
    } else {
        tokens.border
    };
    let cr = style.card_corner_radius(cfg.corner_radius);
    let stroke = if style.refraction_border {
        Stroke::NONE
    } else {
        Stroke::new(1.0_f32, border_col)
    };
    Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(cr)
        .inner_margin(Margin::same(12))
}

#[inline]
pub fn rgb_to_c32(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// Sidebar frame honoring active [`ThemeStyleParams`] translucency and refractive tint.
pub fn solid_sidebar_frame_with_config(tokens: &Tokens, cfg: &ThemeConfig) -> Frame {
    let style = ThemeStyleParams::from_config(cfg);
    if style.refraction_border {
        return Frame::new()
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE)
            .inner_margin(Margin::same(12));
    }
    Frame::new()
        .fill(style.sidebar_fill(tokens))
        .stroke(Stroke::new(1.0_f32, tokens.border))
        .inner_margin(Margin::same(12))
}

/// Solid opaque sidebar frame (surface background, 1px border).
pub fn solid_sidebar_frame(tokens: &Tokens) -> Frame {
    Frame::new()
        .fill(tokens.bg_sidebar)
        .stroke(Stroke::new(1.0_f32, tokens.border))
        .inner_margin(Margin::same(12))
}

/// Modal dialog panel frame (8px radius, border_strong).
pub fn solid_modal_frame(tokens: &Tokens) -> Frame {
    Frame::new()
        .fill(tokens.bg_surface)
        .stroke(Stroke::new(1.0_f32, tokens.border_strong))
        .corner_radius(RADIUS_CARD)
        .inner_margin(Margin::same(20))
}

/// Bottom HUD bar frame honoring active [`ThemeStyleParams`] translucency.
pub fn solid_hud_frame_with_config(tokens: &Tokens, cfg: &ThemeConfig) -> Frame {
    let style = ThemeStyleParams::from_config(cfg);
    if style.refraction_border {
        return Frame::new()
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE)
            .inner_margin(Margin::symmetric(12, 4));
    }
    Frame::new()
        .fill(style.hud_fill(tokens))
        .stroke(Stroke::new(1.0_f32, tokens.border))
        .inner_margin(Margin::symmetric(12, 4))
}

/// HUD bottom bar frame matching the active theme tokens.
pub fn solid_hud_frame(tokens: &Tokens) -> Frame {
    Frame::new()
        .fill(tokens.bg_sidebar)
        .stroke(Stroke::new(1.0_f32, tokens.border))
        .inner_margin(Margin::symmetric(12, 4))
}

/// Raw wallpaper RGBA buffer prepared for GPU upload into [`LiquidGlassGpuResources`].
#[derive(Clone)]
pub struct GpuBackdropSource {
    pub key: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

/// Active screen-space backdrop geometry & image source passed to `wgpu` Liquid Glass callbacks.
#[derive(Clone)]
pub struct ActiveGpuBackdrop {
    pub source: GpuBackdropSource,
    pub screen_rect: Rect,
    pub uv_rect: Rect,
    pub backdrop_dim: f32,
    pub generation: u64,
}

#[derive(Clone)]
struct CachedBackdrop {
    path: String,
    texture: Option<TextureHandle>,
    blurred_texture: Option<TextureHandle>,
    sharp_rgba: Option<Arc<Vec<u8>>>,
    size: [usize; 2],
}

/// GPU uniform payload for the `liquid_glass.wgsl` fragment shader (128 bytes, 16-byte aligned).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LiquidGlassUniforms {
    pub rect_min: [f32; 2],
    pub rect_size: [f32; 2],
    pub screen_size: [f32; 2],
    pub light_dir: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_size: [f32; 2],
    pub backdrop_min: [f32; 2],
    pub backdrop_size: [f32; 2],
    pub tint_color: [f32; 4],
    pub corner_radius: f32,
    pub bevel_width: f32,
    pub ior_strength: f32,
    pub dispersion: f32,
    pub cursor_pos: [f32; 2],
    pub spec_radius: f32,
    pub hover_t: f32,
    /// `[has_backdrop, glass_opacity, chroma_strength, 0.0]`.
    pub mode_params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BlurUniforms {
    texel_step: [f32; 2],
    radius: f32,
    _pad: f32,
}

const WGPU_GLASS_AVAILABLE_KEY: &str = "mq_wgpu_liquid_glass_available";
const ACTIVE_GPU_BACKDROP_KEY: &str = "mq_active_gpu_glass_backdrop";

/// Returns `true` when `egui_wgpu::CallbackResources` has initialized GPU Liquid Glass pipelines.
#[inline]
pub fn is_wgpu_glass_available(ctx: &Context) -> bool {
    ctx.data(|d| {
        d.get_temp::<bool>(Id::new(WGPU_GLASS_AVAILABLE_KEY))
            .unwrap_or(false)
    })
}

/// Stores the active backdrop descriptor consumed by [`LiquidGlassGpuResources`] during `prepare()`.
#[inline]
pub fn store_active_gpu_backdrop(ctx: &Context, active: Option<ActiveGpuBackdrop>) {
    let key = Id::new(ACTIVE_GPU_BACKDROP_KEY);
    ctx.data_mut(|d| {
        d.insert_temp(key, active);
    });
}

/// Returns the active GPU backdrop descriptor for the current frame, if any.
#[inline]
pub fn active_gpu_backdrop(ctx: &Context) -> Option<ActiveGpuBackdrop> {
    ctx.data(|d| {
        d.get_temp::<Option<ActiveGpuBackdrop>>(Id::new(ACTIVE_GPU_BACKDROP_KEY))
            .flatten()
    })
}

/// Persistent `wgpu` pipelines, cached sharp & GPU-blurred wallpaper textures, and dynamic uniform
/// ring buffer stored inside [`egui_wgpu::CallbackResources`].
pub struct LiquidGlassGpuResources {
    blur_pipeline: wgpu::RenderPipeline,
    blur_bind_group_layout: wgpu::BindGroupLayout,
    blur_u_h1: wgpu::Buffer,
    blur_u_v1: wgpu::Buffer,
    blur_u_h2: wgpu::Buffer,
    blur_u_v2: wgpu::Buffer,
    glass_pipeline: wgpu::RenderPipeline,
    glass_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    sharp_texture: wgpu::Texture,
    sharp_view: wgpu::TextureView,
    blur_temp_texture: wgpu::Texture,
    blur_temp_view: wgpu::TextureView,
    blurred_texture: wgpu::Texture,
    blurred_view: wgpu::TextureView,
    glass_uniform_buffer: wgpu::Buffer,
    glass_uniform_capacity: usize,
    glass_bind_group: wgpu::BindGroup,
    uniform_stride: usize,
    staging_bytes: Vec<u8>,
    current_gen: Option<u64>,
    frame_flushed: bool,
    loaded_key: String,
    sharp_size: [u32; 2],
    blur_size: [u32; 2],
    loaded_window_px: [u32; 2],
}

impl LiquidGlassGpuResources {
    /// Compiles WGSL shaders and allocates initial sharp/blurred textures and dynamic uniform buffer.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        let shader_src = include_str!("shaders/liquid_glass.wgsl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mq_liquid_glass_wgsl"),
            source: wgpu::ShaderSource::Wgsl(shader_src.into()),
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mq_liquid_glass_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let blur_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("mq_glass_blur_bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(
                                std::mem::size_of::<BlurUniforms>() as u64,
                            ),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let blur_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mq_glass_blur_pl"),
            bind_group_layouts: &[&blur_bind_group_layout],
            push_constant_ranges: &[],
        });

        let blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mq_glass_blur_pipeline"),
            layout: Some(&blur_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_blur"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_blur"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let uniform_size = std::mem::size_of::<LiquidGlassUniforms>() as u64;
        let glass_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("mq_liquid_glass_bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: true,
                            min_binding_size: wgpu::BufferSize::new(uniform_size),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let glass_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("mq_liquid_glass_pl"),
                bind_group_layouts: &[&glass_bind_group_layout],
                push_constant_ranges: &[],
            });

        let glass_entry = if target_format.is_srgb() {
            "fs_main_linear"
        } else {
            "fs_main_gamma"
        };

        let glass_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mq_liquid_glass_pipeline"),
            layout: Some(&glass_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(glass_entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::OneMinusDstAlpha,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let make_blur_buf = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: std::mem::size_of::<BlurUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };

        let (sharp_texture, sharp_view) = Self::create_rgba_texture(device, "mq_glass_sharp", 4, 4);
        let (blur_temp_texture, blur_temp_view) =
            Self::create_rgba_texture(device, "mq_glass_blur_tmp", 4, 4);
        let (blurred_texture, blurred_view) =
            Self::create_rgba_texture(device, "mq_glass_blurred", 4, 4);

        let fallback_px = vec![24_u8; 4 * 4 * 4];
        for tex in [&sharp_texture, &blur_temp_texture, &blurred_texture] {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &fallback_px,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * 4),
                    rows_per_image: Some(4),
                },
                wgpu::Extent3d {
                    width: 4,
                    height: 4,
                    depth_or_array_layers: 1,
                },
            );
        }

        let align = device.limits().min_uniform_buffer_offset_alignment.max(1) as usize;
        let uniform_stride = (std::mem::size_of::<LiquidGlassUniforms>()).div_ceil(align) * align;
        let glass_uniform_capacity = uniform_stride * 64;
        let glass_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mq_glass_dynamic_uniforms"),
            size: glass_uniform_capacity as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let glass_bind_group = Self::build_glass_bind_group(
            device,
            &glass_bind_group_layout,
            &glass_uniform_buffer,
            &sharp_view,
            &blurred_view,
            &sampler,
        );

        Self {
            blur_pipeline,
            blur_bind_group_layout,
            blur_u_h1: make_blur_buf("mq_blur_u_h1"),
            blur_u_v1: make_blur_buf("mq_blur_u_v1"),
            blur_u_h2: make_blur_buf("mq_blur_u_h2"),
            blur_u_v2: make_blur_buf("mq_blur_u_v2"),
            glass_pipeline,
            glass_bind_group_layout,
            sampler,
            sharp_texture,
            sharp_view,
            blur_temp_texture,
            blur_temp_view,
            blurred_texture,
            blurred_view,
            glass_uniform_buffer,
            glass_uniform_capacity,
            glass_bind_group,
            uniform_stride,
            staging_bytes: Vec::with_capacity(glass_uniform_capacity),
            current_gen: None,
            frame_flushed: false,
            loaded_key: String::new(),
            sharp_size: [4, 4],
            blur_size: [4, 4],
            loaded_window_px: [0, 0],
        }
    }

    fn create_rgba_texture(
        device: &wgpu::Device,
        label: &str,
        width: u32,
        height: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let max_dim = device.limits().max_texture_dimension_2d.max(1);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: width.clamp(1, max_dim),
                height: height.clamp(1, max_dim),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    fn build_glass_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniform_buffer: &wgpu::Buffer,
        sharp_view: &wgpu::TextureView,
        blurred_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        let binding_len = std::mem::size_of::<LiquidGlassUniforms>() as u64;
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mq_liquid_glass_bg"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: uniform_buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(binding_len),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(sharp_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(blurred_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    /// Ensures per-frame dynamic uniform staging starts cleanly on frame `gen`.
    pub fn begin_frame_if_needed(&mut self, gen: u64) {
        if self.current_gen != Some(gen) {
            self.current_gen = Some(gen);
            self.frame_flushed = false;
            self.staging_bytes.clear();
        }
    }

    /// Appends one panel's `LiquidGlassUniforms` padded to `min_uniform_buffer_offset_alignment`.
    pub fn push_uniforms(&mut self, uniforms: &LiquidGlassUniforms) -> u32 {
        let aligned = self.staging_bytes.len().div_ceil(self.uniform_stride) * self.uniform_stride;
        self.staging_bytes.resize(aligned + self.uniform_stride, 0);
        let bytes = bytemuck::bytes_of(uniforms);
        self.staging_bytes[aligned..aligned + bytes.len()].copy_from_slice(bytes);
        aligned as u32
    }

    /// Uploads all staged panel uniforms for this frame in a single buffer write before `paint()`.
    pub fn flush_uniforms(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.frame_flushed || self.staging_bytes.is_empty() {
            return;
        }
        if self.staging_bytes.len() > self.glass_uniform_capacity {
            let new_cap = self
                .staging_bytes
                .len()
                .next_power_of_two()
                .max(self.uniform_stride * 64);
            self.glass_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mq_glass_dynamic_uniforms"),
                size: new_cap as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.glass_uniform_capacity = new_cap;
            self.glass_bind_group = Self::build_glass_bind_group(
                device,
                &self.glass_bind_group_layout,
                &self.glass_uniform_buffer,
                &self.sharp_view,
                &self.blurred_view,
                &self.sampler,
            );
        }
        queue.write_buffer(&self.glass_uniform_buffer, 0, &self.staging_bytes);
        self.frame_flushed = true;
    }

    /// Returns whether `offset` is an aligned in-bounds dynamic offset for `LiquidGlassUniforms`.
    #[inline]
    pub fn is_valid_uniform_offset(&self, offset: u32) -> bool {
        let off = offset as usize;
        let stride = self.uniform_stride.max(1);
        off.is_multiple_of(stride)
            && off
                .checked_add(std::mem::size_of::<LiquidGlassUniforms>())
                .is_some_and(|end| end <= self.glass_uniform_capacity)
    }

    /// Uploads wallpaper sharp pixels when changed and executes 4-pass GPU separable blur
    /// only when wallpaper changes or window size changes.
    pub fn sync_backdrop(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: &GpuBackdropSource,
        window_px: [u32; 2],
    ) {
        if window_px[0] == 0 || window_px[1] == 0 || source.width == 0 || source.height == 0 {
            return;
        }
        let max_dim = device.limits().max_texture_dimension_2d.max(256);
        let target_w = (window_px[0] / 2).clamp(160, 960).min(max_dim).max(1);
        let target_h = (window_px[1] / 2).clamp(120, 640).min(max_dim).max(1);
        let target_blur = [target_w, target_h];

        let (upload_w, upload_h, resized_rgba) =
            downscale_rgba_if_exceeds(&source.rgba, source.width, source.height, max_dim);
        if upload_w == 0 || upload_h == 0 {
            return;
        }

        let key_changed = self.loaded_key != source.key || self.sharp_size != [upload_w, upload_h];
        let size_changed = self.blur_size != target_blur || self.loaded_window_px != window_px;

        if !key_changed && !size_changed {
            return;
        }

        let mut rebuild_bg = false;

        if key_changed {
            let upload_bytes: &[u8] = resized_rgba.as_deref().unwrap_or(&source.rgba);
            let expected_len = (upload_w as usize)
                .saturating_mul(upload_h as usize)
                .saturating_mul(4);
            let bytes_per_row = upload_w.checked_mul(4);
            if let Some(bpr) = bytes_per_row {
                if expected_len > 0 && upload_bytes.len() >= expected_len {
                    if self.sharp_size != [upload_w, upload_h] {
                        let (tex, view) =
                            Self::create_rgba_texture(device, "mq_glass_sharp", upload_w, upload_h);
                        self.sharp_texture = tex;
                        self.sharp_view = view;
                        self.sharp_size = [upload_w, upload_h];
                        rebuild_bg = true;
                    }
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &self.sharp_texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &upload_bytes[..expected_len],
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(bpr),
                            rows_per_image: Some(upload_h),
                        },
                        wgpu::Extent3d {
                            width: upload_w,
                            height: upload_h,
                            depth_or_array_layers: 1,
                        },
                    );
                }
            }
            self.loaded_key = source.key.clone();
        }

        if self.blur_size != target_blur {
            let (tmp_tex, tmp_view) =
                Self::create_rgba_texture(device, "mq_glass_blur_tmp", target_w, target_h);
            let (out_tex, out_view) =
                Self::create_rgba_texture(device, "mq_glass_blurred", target_w, target_h);
            self.blur_temp_texture = tmp_tex;
            self.blur_temp_view = tmp_view;
            self.blurred_texture = out_tex;
            self.blurred_view = out_view;
            self.blur_size = target_blur;
            rebuild_bg = true;
        }

        self.loaded_window_px = window_px;

        let dx = 1.0 / (target_w as f32);
        let dy = 1.0 / (target_h as f32);
        let pass_params = [
            (&self.blur_u_h1, [dx, 0.0], 2.2_f32),
            (&self.blur_u_v1, [0.0, dy], 2.2_f32),
            (&self.blur_u_h2, [dx, 0.0], 3.4_f32),
            (&self.blur_u_v2, [0.0, dy], 3.4_f32),
        ];
        for (buf, step, rad) in pass_params {
            let u = BlurUniforms {
                texel_step: step,
                radius: rad,
                _pad: 0.0,
            };
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&u));
        }

        let passes = [
            (
                &self.blur_u_h1,
                &self.sharp_view,
                &self.blur_temp_view,
                "mq_blur_h1",
            ),
            (
                &self.blur_u_v1,
                &self.blur_temp_view,
                &self.blurred_view,
                "mq_blur_v1",
            ),
            (
                &self.blur_u_h2,
                &self.blurred_view,
                &self.blur_temp_view,
                "mq_blur_h2",
            ),
            (
                &self.blur_u_v2,
                &self.blur_temp_view,
                &self.blurred_view,
                "mq_blur_v2",
            ),
        ];

        for (ubuf, src_view, dst_view, label) in passes {
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.blur_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ubuf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(src_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dst_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rpass.set_viewport(0.0, 0.0, target_w as f32, target_h as f32, 0.0, 1.0);
            rpass.set_pipeline(&self.blur_pipeline);
            rpass.set_bind_group(0, &bg, &[]);
            rpass.draw(0..6, 0..1);
        }

        if rebuild_bg {
            self.glass_bind_group = Self::build_glass_bind_group(
                device,
                &self.glass_bind_group_layout,
                &self.glass_uniform_buffer,
                &self.sharp_view,
                &self.blurred_view,
                &self.sampler,
            );
        }
    }

    /// Returns the compiled Liquid Glass render pipeline.
    #[inline]
    pub fn glass_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.glass_pipeline
    }

    /// Returns the bind group holding dynamic panel uniforms + sharp/blurred GPU textures.
    #[inline]
    pub fn glass_bind_group(&self) -> &wgpu::BindGroup {
        &self.glass_bind_group
    }
}

/// Registers [`LiquidGlassGpuResources`] into `renderer.callback_resources` and enables the GPU
/// `wgpu` Liquid Glass path on `ctx` (leaves OpenGL/`glow` fallback untouched when unavailable).
pub fn init_wgpu_liquid_glass_for_renderer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target_format: wgpu::TextureFormat,
    renderer: &mut egui_wgpu::Renderer,
    ctx: &Context,
) {
    let resources = LiquidGlassGpuResources::new(device, queue, target_format);
    renderer.callback_resources.insert(resources);
    ctx.data_mut(|d| {
        d.insert_temp(Id::new(WGPU_GLASS_AVAILABLE_KEY), true);
    });
}

/// Initializes GPU Liquid Glass pipelines from `eframe::CreationContext` if running under `wgpu`.
pub fn init_wgpu_liquid_glass(cc: &eframe::CreationContext<'_>) {
    if let Some(rs) = cc.wgpu_render_state.as_ref() {
        let mut renderer = rs.renderer.write();
        init_wgpu_liquid_glass_for_renderer(
            &rs.device,
            &rs.queue,
            rs.target_format,
            &mut renderer,
            &cc.egui_ctx,
        );
    }
}

#[cfg(test)]
pub(crate) fn mark_wgpu_glass_available_for_test(ctx: &Context) {
    ctx.data_mut(|d| {
        d.insert_temp(Id::new(WGPU_GLASS_AVAILABLE_KEY), true);
    });
}

fn downscale_rgba_if_exceeds(
    src: &[u8],
    width: u32,
    height: u32,
    max_dim: u32,
) -> (u32, u32, Option<Vec<u8>>) {
    if width == 0 || height == 0 || max_dim == 0 {
        return (0, 0, None);
    }
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if src.len() < expected {
        return (0, 0, None);
    }
    if width <= max_dim && height <= max_dim {
        return (width, height, None);
    }
    let scale = (max_dim as f64) / (width.max(height) as f64);
    let dw = ((width as f64 * scale).floor() as u32).clamp(1, max_dim);
    let dh = ((height as f64 * scale).floor() as u32).clamp(1, max_dim);
    let mut out = vec![0_u8; (dw as usize) * (dh as usize) * 4];
    for y in 0..dh {
        let sy = (((y as u64) * (height as u64)) / (dh as u64)).min((height - 1) as u64) as usize;
        for x in 0..dw {
            let sx = (((x as u64) * (width as u64)) / (dw as u64)).min((width - 1) as u64) as usize;
            let si = (sy * (width as usize) + sx) * 4;
            let di = ((y as usize) * (dw as usize) + (x as usize)) * 4;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    (dw, dh, Some(out))
}

fn build_blurred_backdrop_image(src: &[u8], w: usize, h: usize) -> ColorImage {
    let expected = w.saturating_mul(h).saturating_mul(4);
    if w == 0 || h == 0 || src.len() < expected {
        return ColorImage::new([1, 1], Color32::from_rgb(16, 18, 24));
    }
    let scale = (480.0 / (w.max(h) as f32)).min(1.0);
    let dw = ((w as f32 * scale).round() as usize).max(16);
    let dh = ((h as f32 * scale).round() as usize).max(16);
    let mut buf = vec![0.0_f32; dw * dh * 4];
    for y in 0..dh {
        let sy = ((y as f32 + 0.5) * h as f32 / dh as f32 - 0.5)
            .clamp(0.0, (h.saturating_sub(1)) as f32) as usize;
        for x in 0..dw {
            let sx = ((x as f32 + 0.5) * w as f32 / dw as f32 - 0.5)
                .clamp(0.0, (w.saturating_sub(1)) as f32) as usize;
            let si = (sy * w + sx) * 4;
            let di = (y * dw + x) * 4;
            buf[di] = src[si] as f32;
            buf[di + 1] = src[si + 1] as f32;
            buf[di + 2] = src[si + 2] as f32;
            buf[di + 3] = 255.0;
        }
    }

    let mut tmp = vec![0.0_f32; dw * dh * 4];
    let rad = 7isize;
    for _pass in 0..2 {
        // Horizontal box blur using sliding sum
        for y in 0..dh {
            let row = y * dw * 4;
            let mut acc = [0.0_f32; 3];
            let mut cnt = 0.0_f32;
            for x in 0..=(rad as usize).min(dw - 1) {
                let idx = row + x * 4;
                acc[0] += buf[idx];
                acc[1] += buf[idx + 1];
                acc[2] += buf[idx + 2];
                cnt += 1.0;
            }
            for x in 0..dw {
                let xi = x as isize;
                if xi - rad > 0 {
                    let rem = row + ((xi - rad - 1) as usize) * 4;
                    acc[0] -= buf[rem];
                    acc[1] -= buf[rem + 1];
                    acc[2] -= buf[rem + 2];
                    cnt -= 1.0;
                }
                if x > 0 && xi + rad < dw as isize {
                    let add = row + ((xi + rad) as usize) * 4;
                    acc[0] += buf[add];
                    acc[1] += buf[add + 1];
                    acc[2] += buf[add + 2];
                    cnt += 1.0;
                }
                let di = row + x * 4;
                let inv = 1.0 / cnt.max(1.0);
                tmp[di] = acc[0] * inv;
                tmp[di + 1] = acc[1] * inv;
                tmp[di + 2] = acc[2] * inv;
                tmp[di + 3] = 255.0;
            }
        }
        // Vertical box blur using sliding sum
        for x in 0..dw {
            let mut acc = [0.0_f32; 3];
            let mut cnt = 0.0_f32;
            for y in 0..=(rad as usize).min(dh - 1) {
                let idx = (y * dw + x) * 4;
                acc[0] += tmp[idx];
                acc[1] += tmp[idx + 1];
                acc[2] += tmp[idx + 2];
                cnt += 1.0;
            }
            for y in 0..dh {
                let yi = y as isize;
                if yi - rad > 0 {
                    let rem = (((yi - rad - 1) as usize) * dw + x) * 4;
                    acc[0] -= tmp[rem];
                    acc[1] -= tmp[rem + 1];
                    acc[2] -= tmp[rem + 2];
                    cnt -= 1.0;
                }
                if y > 0 && yi + rad < dh as isize {
                    let add = (((yi + rad) as usize) * dw + x) * 4;
                    acc[0] += tmp[add];
                    acc[1] += tmp[add + 1];
                    acc[2] += tmp[add + 2];
                    cnt += 1.0;
                }
                let di = (y * dw + x) * 4;
                let inv = 1.0 / cnt.max(1.0);
                buf[di] = acc[0] * inv;
                buf[di + 1] = acc[1] * inv;
                buf[di + 2] = acc[2] * inv;
                buf[di + 3] = 255.0;
            }
        }
    }

    let out_rgba: Vec<u8> = buf
        .into_iter()
        .map(|v| v.round().clamp(0.0, 255.0) as u8)
        .collect();
    ColorImage::from_rgba_unmultiplied([dw, dh], &out_rgba)
}

struct ResolvedBackdrop {
    sharp_tex: TextureHandle,
    blurred_tex: Option<TextureHandle>,
    gpu_source: Option<GpuBackdropSource>,
    size: [usize; 2],
}

fn resolve_backdrop_texture(ctx: &Context, path: &str) -> Option<ResolvedBackdrop> {
    let cleaned = crate::opener::clean_user_path(path);
    let trimmed = cleaned.as_str();
    if trimmed.is_empty() {
        return None;
    }

    let use_wgpu = is_wgpu_glass_available(ctx);
    let cache_id = Id::new("mq_backdrop_tex_cache");
    if let Some(mut cached) = ctx.data(|d| d.get_temp::<CachedBackdrop>(cache_id)) {
        if cached.path == trimmed {
            if let Some(sharp_tex) = cached.texture.clone() {
                if !use_wgpu && cached.blurred_texture.is_none() {
                    if let Some(raw) = cached.sharp_rgba.as_ref() {
                        let blur_img =
                            build_blurred_backdrop_image(raw, cached.size[0], cached.size[1]);
                        let blur_tex = ctx.load_texture(
                            "mq_glass_blur_backdrop",
                            blur_img,
                            TextureOptions::LINEAR,
                        );
                        cached.blurred_texture = Some(blur_tex);
                        ctx.data_mut(|d| {
                            d.insert_temp(cache_id, cached.clone());
                        });
                    }
                }
                let gpu_source = cached.sharp_rgba.as_ref().map(|rgba| GpuBackdropSource {
                    key: trimmed.to_owned(),
                    width: cached.size[0] as u32,
                    height: cached.size[1] as u32,
                    rgba: Arc::clone(rgba),
                });
                return Some(ResolvedBackdrop {
                    sharp_tex,
                    blurred_tex: cached.blurred_texture,
                    gpu_source,
                    size: cached.size,
                });
            }
            return None;
        }
    }

    let loaded = (|| {
        let bytes = fs::read(Path::new(trimmed)).ok()?;
        let img = image::load_from_memory(&bytes).ok()?;
        let max_backdrop_side = 4096_u32;
        let img = if img.width() > max_backdrop_side || img.height() > max_backdrop_side {
            img.resize(
                max_backdrop_side,
                max_backdrop_side,
                image::imageops::FilterType::Triangle,
            )
        } else {
            img
        };
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        if size[0] == 0 || size[1] == 0 {
            return None;
        }
        let raw_arc = Arc::new(rgba.into_raw());
        let color_image = ColorImage::from_rgba_unmultiplied(size, &raw_arc);
        let tex = ctx.load_texture("mq_custom_backdrop", color_image, TextureOptions::LINEAR);
        let blur_tex = if use_wgpu {
            None
        } else {
            let blur_image = build_blurred_backdrop_image(&raw_arc, size[0], size[1]);
            Some(ctx.load_texture("mq_glass_blur_backdrop", blur_image, TextureOptions::LINEAR))
        };
        let gpu_source = GpuBackdropSource {
            key: trimmed.to_owned(),
            width: size[0] as u32,
            height: size[1] as u32,
            rgba: Arc::clone(&raw_arc),
        };
        Some((tex, blur_tex, raw_arc, gpu_source, size))
    })();

    ctx.data_mut(|d| {
        d.insert_temp(
            cache_id,
            CachedBackdrop {
                path: trimmed.to_owned(),
                texture: loaded.as_ref().map(|(t, _, _, _, _)| t.clone()),
                blurred_texture: loaded.as_ref().and_then(|(_, b, _, _, _)| b.clone()),
                sharp_rgba: loaded.as_ref().map(|(_, _, r, _, _)| Arc::clone(r)),
                size: loaded.as_ref().map(|(_, _, _, _, s)| *s).unwrap_or([1, 1]),
            },
        );
    });

    loaded.map(
        |(sharp_tex, blurred_tex, _, gpu_source, size)| ResolvedBackdrop {
            sharp_tex,
            blurred_tex,
            gpu_source: Some(gpu_source),
            size,
        },
    )
}

fn paint_procedural_backdrop(painter: &Painter, rect: Rect, tokens: &Tokens) {
    let is_light = tokens.is_light();
    let cool_secondary = if is_light {
        Color32::from_rgb(0x4B, 0x6B, 0x99)
    } else {
        Color32::from_rgb(0x4A, 0x6B, 0x9C)
    };

    // 4x3 layered gradient mesh: subtle ambient corner glow over flat base.
    let cols = 4usize;
    let rows = 3usize;
    let mut mesh = Mesh::default();
    for r in 0..=rows {
        let fy = (r as f32) / (rows as f32);
        let y = rect.top() + rect.height() * fy;
        for c in 0..=cols {
            let fx = (c as f32) / (cols as f32);
            let x = rect.left() + rect.width() * fx;

            let tl_dist = ((fx * fx) + (fy * fy)).sqrt().clamp(0.0, 1.42);
            let br_dist = (((1.0 - fx).powi(2)) + ((1.0 - fy).powi(2)))
                .sqrt()
                .clamp(0.0, 1.42);

            let tl_weight =
                ((1.0 - tl_dist / 1.1).max(0.0)).powf(1.8) * if is_light { 0.08 } else { 0.11 };
            let br_weight =
                ((1.0 - br_dist / 1.1).max(0.0)).powf(1.8) * if is_light { 0.06 } else { 0.09 };

            let mid = blend_color(tokens.bg_app, tokens.accent, tl_weight);
            let color = blend_color(mid, cool_secondary, br_weight);
            mesh.vertices.push(Vertex {
                pos: Pos2::new(x, y),
                uv: Pos2::ZERO,
                color,
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

    // Architectural 32px micro-grid & 128px registration crosshairs.
    let grid_step = 32.0_f32;
    let grid_alpha = if is_light { 12 } else { 14 };
    let grid_color = Color32::from_rgba_unmultiplied(
        tokens.border_strong.r(),
        tokens.border_strong.g(),
        tokens.border_strong.b(),
        grid_alpha,
    );
    let cross_color = Color32::from_rgba_unmultiplied(
        tokens.accent.r(),
        tokens.accent.g(),
        tokens.accent.b(),
        if is_light { 34 } else { 38 },
    );

    let max_span = 4096.0_f32;
    let right_limit = rect.left() + rect.width().min(max_span);
    let bottom_limit = rect.top() + rect.height().min(max_span);

    let mut x = (rect.left() / grid_step).ceil() * grid_step;
    while x < right_limit {
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, bottom_limit)],
            Stroke::new(1.0_f32, grid_color),
        );
        x += grid_step;
    }

    let mut y = (rect.top() / grid_step).ceil() * grid_step;
    while y < bottom_limit {
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(right_limit, y)],
            Stroke::new(1.0_f32, grid_color),
        );
        y += grid_step;
    }

    let major_step = 128.0_f32;
    let mut cx = (rect.left() / major_step).ceil() * major_step;
    while cx < right_limit {
        let mut cy = (rect.top() / major_step).ceil() * major_step;
        while cy < bottom_limit {
            painter.line_segment(
                [Pos2::new(cx - 3.5, cy), Pos2::new(cx + 3.5, cy)],
                Stroke::new(1.0_f32, cross_color),
            );
            painter.line_segment(
                [Pos2::new(cx, cy - 3.5), Pos2::new(cx, cy + 3.5)],
                Stroke::new(1.0_f32, cross_color),
            );
            cy += major_step;
        }
        cx += major_step;
    }
}

fn draw_aspect_fill_wallpaper(
    painter: &Painter,
    rect: Rect,
    tex: &TextureHandle,
    size: [usize; 2],
    window_alpha: f32,
) -> Rect {
    if !rect.is_positive() || size[0] == 0 || size[1] == 0 {
        return Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    }
    let tex_aspect = (size[0] as f32) / (size[1].max(1) as f32);
    let rect_aspect = rect.width() / rect.height().max(1.0);
    let uv = if rect_aspect > tex_aspect {
        let h = tex_aspect / rect_aspect;
        let pad = (1.0 - h) * 0.5;
        Rect::from_min_max(Pos2::new(0.0, pad), Pos2::new(1.0, 1.0 - pad))
    } else {
        let w = rect_aspect / tex_aspect;
        let pad = (1.0 - w) * 0.5;
        Rect::from_min_max(Pos2::new(pad, 0.0), Pos2::new(1.0 - pad, 1.0))
    };
    let tint = Color32::from_rgba_unmultiplied(
        255,
        255,
        255,
        (crate::state::safe_clamp_f32(window_alpha, 0.35, 1.0) * 255.0).round() as u8,
    );
    painter.image(tex.id(), rect, uv, tint);
    uv
}

/// Paints the configured launcher backdrop (`Procedural`, `Solid`, or `CustomImage`) inside `rect`.
///
/// Under [`ThemePreset::LiquidGlass`]:
/// - `Procedural`: leaves the window framebuffer genuinely transparent (`clear_color = [0, 0, 0, 0]`)
///   so the OS compositor blur & real desktop wallpaper shine directly through with zero fake wallpaper.
/// - `Solid`: paints only a subtle translucent atmospheric glass tint (never an opaque wallpaper).
/// - `CustomImage`: loads and refracts ONLY an explicitly configured `cfg.backdrop_path`; if unset or
///   unreadable, stays cleanly transparent.
pub fn paint_launcher_background(
    ctx: &Context,
    painter: &Painter,
    rect: Rect,
    cfg: &ThemeConfig,
    tokens: &Tokens,
) {
    if !rect.is_positive() {
        return;
    }
    let style = ThemeStyleParams::from_config(cfg);
    let generation = ctx.cumulative_pass_nr();
    let mut active_glass_tex: Option<ActiveBackdropTexture> = None;
    let mut active_gpu: Option<ActiveGpuBackdrop> = None;

    match cfg.background_style {
        BackgroundStyle::Solid => {
            if cfg.preset == ThemePreset::LiquidGlass {
                // Keep window genuinely translucent over the compositor desktop; never paint an opaque wall.
                let win_norm = crate::state::safe_clamp_f32(cfg.window_opacity, 0.0, 1.0);
                let tint_alpha = if win_norm <= 0.02 {
                    0.0
                } else {
                    (win_norm * 0.42).clamp(0.05, 0.44)
                };
                if tint_alpha > 0.005 {
                    painter.rect_filled(
                        rect,
                        CornerRadius::ZERO,
                        with_alpha(tokens.bg_app, tint_alpha),
                    );
                }
            } else {
                painter.rect_filled(rect, CornerRadius::ZERO, style.window_fill(tokens));
            }
        }
        BackgroundStyle::Procedural => match cfg.preset {
            ThemePreset::LiquidGlass => {
                // True transparent compositor Liquid Glass: no wallpaper photo, no opaque fill,
                // no fake ambient mesh. Cards, sidebar, HUD, and controls float as frosted glass
                // directly over the transparent window framebuffer.
            }
            ThemePreset::Translucent => {
                paint_translucent_clean_backdrop(painter, rect, &style, tokens);
            }
            _ => {
                painter.rect_filled(rect, CornerRadius::ZERO, style.window_fill(tokens));
                paint_procedural_backdrop(painter, rect, tokens);
            }
        },
        BackgroundStyle::CustomImage => {
            let resolved = cfg
                .backdrop_path
                .as_deref()
                .and_then(|p| resolve_backdrop_texture(ctx, p));
            if let Some(resolved) = resolved {
                painter.rect_filled(rect, CornerRadius::ZERO, style.window_fill(tokens));
                let uv = draw_aspect_fill_wallpaper(
                    painter,
                    rect,
                    &resolved.sharp_tex,
                    resolved.size,
                    style.window_alpha,
                );

                let dim = crate::state::safe_clamp_f32(cfg.backdrop_dim, 0.0, 0.92);
                if dim > 0.01 {
                    let scrim = Color32::from_rgba_unmultiplied(
                        tokens.bg_app.r(),
                        tokens.bg_app.g(),
                        tokens.bg_app.b(),
                        (dim * 255.0).round() as u8,
                    );
                    painter.rect_filled(rect, CornerRadius::ZERO, scrim);
                }
                if cfg.preset == ThemePreset::LiquidGlass {
                    if let Some(blur_tex) = resolved.blurred_tex {
                        active_glass_tex = Some(ActiveBackdropTexture {
                            tex_id: blur_tex.id(),
                            screen_rect: rect,
                            uv_rect: uv,
                        });
                    }
                    if let Some(gpu_source) = resolved.gpu_source {
                        active_gpu = Some(ActiveGpuBackdrop {
                            source: gpu_source,
                            screen_rect: rect,
                            uv_rect: uv,
                            backdrop_dim: dim,
                            generation,
                        });
                    }
                }
            } else {
                match cfg.preset {
                    ThemePreset::LiquidGlass => {
                        // Without an explicit readable custom image, keep the window genuinely transparent.
                    }
                    ThemePreset::Translucent => {
                        paint_translucent_clean_backdrop(painter, rect, &style, tokens);
                    }
                    _ => {
                        painter.rect_filled(rect, CornerRadius::ZERO, style.window_fill(tokens));
                        paint_procedural_backdrop(painter, rect, tokens);
                    }
                }
            }
        }
    }

    store_active_backdrop_texture(ctx, active_glass_tex);
    store_active_gpu_backdrop(ctx, active_gpu);
}

// Aliases for legacy call sites
#[inline]
pub fn custom_glass_card_frame(cfg: &ThemeConfig) -> Frame {
    solid_card_frame_with_config(&Tokens::from_config(cfg), cfg)
}

#[inline]
pub fn custom_sidebar_frame(cfg: &ThemeConfig) -> Frame {
    solid_sidebar_frame_with_config(&Tokens::from_config(cfg), cfg)
}

#[inline]
pub fn sidebar_frame() -> Frame {
    Frame::new()
        .fill(BG_SIDEBAR)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .inner_margin(Margin::same(12))
}

#[inline]
pub fn hud_frame() -> Frame {
    Frame::new()
        .fill(BG_SIDEBAR)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .inner_margin(Margin::symmetric(12, 4))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_tokens_match_matte_dark_specification() {
        let t = Tokens::matte_dark();
        assert_eq!(t.bg_app, Color32::from_rgb(0x0F, 0x11, 0x15));
        assert_eq!(t.bg_sidebar, Color32::from_rgb(0x14, 0x17, 0x1D));
        assert_eq!(t.bg_surface, Color32::from_rgb(0x1A, 0x1E, 0x26));
        assert_eq!(t.bg_surface_hover, Color32::from_rgb(0x21, 0x26, 0x31));
        assert_eq!(t.bg_input, Color32::from_rgb(0x12, 0x15, 0x1B));
        assert_eq!(t.border, Color32::from_rgb(0x27, 0x2C, 0x37));
        assert_eq!(t.border_strong, Color32::from_rgb(0x36, 0x3D, 0x4C));
        assert_eq!(t.text, Color32::from_rgb(0xE6, 0xE8, 0xEE));
        assert_eq!(t.text_muted, Color32::from_rgb(0x9A, 0xA1, 0xB2));
        assert_eq!(t.text_disabled, Color32::from_rgb(0x5B, 0x62, 0x74));
        assert_eq!(t.accent, Color32::from_rgb(0x4C, 0x9A, 0x82));
        assert_eq!(t.accent_hover, Color32::from_rgb(0x3D, 0x83, 0x6D));
        assert_eq!(t.accent_text, Color32::from_rgb(0x08, 0x13, 0x10));
        assert_eq!(t.danger, Color32::from_rgb(0xE5, 0x48, 0x4D));
        assert_eq!(t.warning, Color32::from_rgb(0xF5, 0xA5, 0x24));
        assert_eq!(t.success, Color32::from_rgb(0x4C, 0x9A, 0x82));

        // All colors strictly opaque (alpha == 255)
        assert_eq!(t.bg_app.a(), 255);
        assert_eq!(t.bg_sidebar.a(), 255);
        assert_eq!(t.bg_surface.a(), 255);
        assert_eq!(t.border.a(), 255);
        assert_eq!(t.accent.a(), 255);
    }

    #[test]
    fn six_opaque_presets_exist_and_resolve() {
        for preset in ThemePreset::ALL {
            let tokens = Tokens::from_preset(preset);
            assert_eq!(tokens.bg_app.a(), 255);
            assert_eq!(tokens.bg_surface.a(), 255);
            assert_eq!(tokens.border.a(), 255);
            assert_eq!(tokens.accent.a(), 255);
        }
    }

    #[test]
    fn from_config_honors_custom_accent_and_selection_contrast() {
        let mut cfg = ThemeConfig::preset_matte_dark();
        cfg.accent_primary = [0x5B, 0x8D, 0xEF];
        cfg.accent_secondary = [0x46, 0x74, 0xD0];
        let tokens = Tokens::from_config(&cfg);
        assert_eq!(tokens.accent, Color32::from_rgb(0x5B, 0x8D, 0xEF));
        assert_eq!(tokens.accent_hover, Color32::from_rgb(0x46, 0x74, 0xD0));

        let visuals = build_visuals_from_config(&cfg);
        assert_ne!(visuals.selection.bg_fill, visuals.selection.stroke.color);
        assert_eq!(visuals.selection.stroke.color, tokens.text);
    }

    #[test]
    fn backdrop_helpers_handle_zero_and_oversized_dimensions_safely() {
        let empty = build_blurred_backdrop_image(&[], 0, 0);
        assert_eq!(empty.size, [1, 1]);

        let short = build_blurred_backdrop_image(&[1, 2, 3], 4, 4);
        assert_eq!(short.size, [1, 1]);

        assert_eq!(downscale_rgba_if_exceeds(&[], 0, 10, 4096), (0, 0, None));
        assert_eq!(downscale_rgba_if_exceeds(&[0; 8], 4, 4, 4096), (0, 0, None));

        let big = vec![128_u8; 32 * 16 * 4];
        let (dw, dh, resized) = downscale_rgba_if_exceeds(&big, 32, 16, 8);
        assert_eq!((dw, dh), (8, 4));
        assert_eq!(resized.expect("should downscale").len(), (8 * 4 * 4));
    }

    #[test]
    fn liquid_glass_procedural_and_solid_never_load_wallpaper_fallback() {
        assert_eq!(std::mem::size_of::<LiquidGlassUniforms>(), 128);
        assert_eq!(std::mem::size_of::<LiquidGlassUniforms>() % 16, 0);

        let ctx = Context::default();
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0));
        let input = egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        };

        // Procedural Liquid Glass must emit ZERO backdrop shapes and ZERO wallpaper textures.
        let mut cfg_proc = ThemeConfig::preset_liquid_glass();
        cfg_proc.background_style = BackgroundStyle::Procedural;
        cfg_proc.backdrop_path = None;
        let tokens = Tokens::from_config(&cfg_proc);
        let out_proc = ctx.run(input.clone(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            paint_launcher_background(ctx, &painter, rect, &cfg_proc, &tokens);
        });
        assert!(
            active_gpu_backdrop(&ctx).is_none(),
            "LiquidGlass Procedural must not populate GPU wallpaper backdrop"
        );
        assert!(
            crate::theme_styles::active_backdrop_texture(&ctx).is_none(),
            "LiquidGlass Procedural must not populate CPU wallpaper texture"
        );
        assert!(
            out_proc.shapes.is_empty(),
            "LiquidGlass Procedural must leave window canvas genuinely transparent"
        );

        // Solid Liquid Glass also never loads wallpaper or paints opaque fill.
        let mut cfg_solid = ThemeConfig::preset_liquid_glass();
        cfg_solid.background_style = BackgroundStyle::Solid;
        let _ = ctx.run(input.clone(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            paint_launcher_background(ctx, &painter, rect, &cfg_solid, &tokens);
        });
        assert!(active_gpu_backdrop(&ctx).is_none());
        assert!(crate::theme_styles::active_backdrop_texture(&ctx).is_none());

        // CustomImage without backdrop_path stays transparent (no serpantinum fallback).
        let mut cfg_custom_empty = ThemeConfig::preset_liquid_glass();
        cfg_custom_empty.background_style = BackgroundStyle::CustomImage;
        cfg_custom_empty.backdrop_path = None;
        let out_empty = ctx.run(input.clone(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            paint_launcher_background(ctx, &painter, rect, &cfg_custom_empty, &tokens);
        });
        assert!(active_gpu_backdrop(&ctx).is_none());
        assert!(out_empty.shapes.is_empty());

        // CustomImage WITH an explicit valid image DOES load backdrop for refraction.
        let tmp = tempfile::tempdir().expect("tempdir");
        let img_path = tmp.path().join("custom_bg.png");
        let img = image::RgbaImage::from_pixel(16, 16, image::Rgba([40, 80, 120, 255]));
        image::DynamicImage::ImageRgba8(img)
            .save(&img_path)
            .expect("save temp png");
        let mut cfg_custom = ThemeConfig::preset_liquid_glass();
        cfg_custom.background_style = BackgroundStyle::CustomImage;
        cfg_custom.backdrop_path = Some(img_path.to_string_lossy().into_owned());
        let _ = ctx.run(input, |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            paint_launcher_background(ctx, &painter, rect, &cfg_custom, &tokens);
        });
        assert!(
            active_gpu_backdrop(&ctx).is_some(),
            "explicit CustomImage must populate GPU backdrop for refraction"
        );
    }
}
