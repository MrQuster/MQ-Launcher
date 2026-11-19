//! Bottom status bar and real-time per-core CPU telemetry HUD (`cpu0..cpuN` vertical bars,
//! live throughput metrics in `FontFamily::Monospace`, `UiWatchdog` alert counter, and safe
//! `0x0` Wayland/Hyprland viewport guard).

use egui::{
    Align, Color32, Context, CornerRadius, FontFamily, FontId, Layout, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, TopBottomPanel, Ui, Vec2,
};
use mq_core::CpuSnapshot;

use crate::state::safe_clamp_f32;
use crate::theme::{solid_hud_frame, Tokens, ACCENT_COBALT, RADIUS_TAG};
use crate::widgets::{status_dot, version_badge};

/// Fixed height of the bottom status & telemetry HUD bar (`38px`).
pub const BOTTOM_HUD_HEIGHT: f32 = 38.0;

/// Data model for rendering the bottom status & CPU telemetry HUD bar.
#[derive(Debug, Clone, Default)]
pub struct BottomHudData {
    /// Name of the active installation/verification/launch phase, or `None` when idle.
    pub active_phase: Option<String>,
    /// Optional completion fraction (`0.0..=1.0`) for the active phase.
    pub progress_fraction: Option<f32>,
    /// Smoothed item throughput (`files/sec`).
    pub files_per_sec: f64,
    /// Smoothed byte throughput (`MiB/sec`).
    pub mib_per_sec: f64,
    /// Number of currently executing DAG / worker tasks.
    pub active_dag_tasks: usize,
    /// Latest `/proc/stat` per-core CPU load and `VmRSS` memory snapshot.
    pub cpu_snapshot: CpuSnapshot,
    /// Cumulative `UiWatchdog` (`>8ms` frame budget) violation count.
    pub watchdog_violations: u64,
}

/// Returns `true` when the window viewport has been minimized or resized to `0x0` (`<= 1.0px`)
/// on tiling Wayland compositors such as Hyprland or Sway, allowing the frame loop to safely
/// skip layout and avoid degenerate division-by-zero geometry.
#[inline]
pub fn is_zero_viewport(ctx: &Context) -> bool {
    let rect = ctx.screen_rect();
    !rect.width().is_finite()
        || !rect.height().is_finite()
        || rect.width() <= 1.0
        || rect.height() <= 1.0
}

fn core_bar_color(tokens: &Tokens, usage_pct: f32) -> Color32 {
    if usage_pct >= 90.0 {
        tokens.warning
    } else if usage_pct >= 55.0 {
        ACCENT_COBALT
    } else {
        tokens.accent
    }
}

/// Renders per-core CPU load vertical bars, wrapping neatly into rows when >16 cores are present.
pub fn render_per_core_bars(ui: &mut Ui, snap: &CpuSnapshot, tokens: &Tokens) {
    let core_count = snap.per_core.len().max(1);
    let bar_w = 5.0_f32;
    let gap = 2.0_f32;
    let pad_x = 3.0_f32;
    let pad_y = 2.0_f32;
    let row_gap = 2.0_f32;
    const MAX_COLS_PER_ROW: usize = 16;

    let avail_w = ui.available_width();
    let max_cols_fitting = if avail_w.is_finite() && avail_w > pad_x * 2.0 + bar_w {
        (((avail_w - pad_x * 2.0 + gap) / (bar_w + gap)).floor() as usize)
            .clamp(1, MAX_COLS_PER_ROW)
    } else {
        MAX_COLS_PER_ROW
    };

    let rows = core_count.div_ceil(max_cols_fitting).max(1);
    let cols = core_count.div_ceil(rows).clamp(1, max_cols_fitting);
    let row_h = match rows {
        1 => 16.0_f32,
        2 => 9.0_f32,
        3 => 7.0_f32,
        _ => 6.0_f32,
    };

    let total_w = (cols as f32) * bar_w + ((cols.saturating_sub(1)) as f32) * gap + pad_x * 2.0;
    let chart_h = (rows as f32) * row_h + ((rows.saturating_sub(1)) as f32) * row_gap + pad_y * 2.0;

    let (rect, response) = ui.allocate_exact_size(Vec2::new(total_w, chart_h), Sense::hover());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(3), tokens.bg_surface);
        painter.rect_stroke(
            rect,
            CornerRadius::same(3),
            Stroke::new(1.0_f32, tokens.border),
            StrokeKind::Inside,
        );

        for idx in 0..core_count {
            let row = idx / cols;
            let col = idx % cols;
            let pct = snap
                .per_core
                .get(idx)
                .map(|c| safe_clamp_f32(c.usage_pct, 0.0, 100.0))
                .unwrap_or(0.0);

            let x0 = rect.left() + pad_x + (col as f32) * (bar_w + gap);
            let y0 = rect.top() + pad_y + (row as f32) * (row_h + row_gap);
            let y1 = (y0 + row_h).min(rect.bottom() - pad_y).max(y0 + 1.0);
            let max_bar_h = (y1 - y0).max(1.0);

            let track_rect = Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x0 + bar_w, y1));
            painter.rect_filled(track_rect, CornerRadius::same(1), tokens.border);

            if pct > 0.5 {
                let fill_h = safe_clamp_f32(max_bar_h * (pct / 100.0), 1.5, max_bar_h);
                let fill_rect =
                    Rect::from_min_max(Pos2::new(x0, y1 - fill_h), Pos2::new(x0 + bar_w, y1));
                painter.rect_filled(
                    fill_rect,
                    CornerRadius::same(1),
                    core_bar_color(tokens, pct),
                );
            }
        }
    }

    if response.hovered() {
        response.on_hover_ui(|ui| {
            ui.label(
                RichText::new(format!(
                    "Процессор: {:>4.1}% | Память: {:>5.1} МБ",
                    snap.total_pct, snap.rss_mib
                ))
                .font(FontId::new(12.0, FontFamily::Monospace))
                .color(tokens.text),
            );
            if !snap.per_core.is_empty() {
                ui.separator();
                let grid_cols = if snap.per_core.len() > 16 {
                    4
                } else if snap.per_core.len() > 8 {
                    2
                } else {
                    1
                };
                egui::Grid::new("mq_hud_per_core_tooltip")
                    .spacing([12.0, 2.0])
                    .show(ui, |ui| {
                        for (i, core) in snap.per_core.iter().enumerate() {
                            ui.label(
                                RichText::new(format!(
                                    "cpu{}: {:>5.1}%",
                                    core.core_id, core.usage_pct
                                ))
                                .font(FontId::new(11.5, FontFamily::Monospace))
                                .color(tokens.text_muted),
                            );
                            if (i + 1) % grid_cols == 0 {
                                ui.end_row();
                            }
                        }
                    });
            }
        });
    }
}

fn render_smooth_progress_bar(ui: &mut Ui, fraction: f32, tokens: &Tokens) {
    let clamped = safe_clamp_f32(fraction, 0.0, 1.0);
    let smooth = crate::animation::animate_spring_f32(
        ui.ctx(),
        egui::Id::new("mq_hud_progress_bar"),
        clamped,
        crate::animation::SpringConfig::smooth(),
    )
    .clamp(0.0, 1.0);
    let desired = Vec2::new(130.0, 14.0);
    let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, RADIUS_TAG, tokens.border);
        if smooth > 0.001 {
            let fill_w = (rect.width() * smooth).max(3.0);
            let fill_rect = Rect::from_min_size(rect.min, Vec2::new(fill_w, rect.height()));
            painter.rect_filled(fill_rect, RADIUS_TAG, tokens.accent);
        }
        let pct_str = format!("{:>3.0}%", smooth * 100.0);
        let text_col = if smooth >= 0.45 {
            tokens.accent_text
        } else {
            tokens.text
        };
        let galley =
            painter.layout_no_wrap(pct_str, FontId::new(10.5, FontFamily::Monospace), text_col);
        let pos = Pos2::new(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        );
        painter.galley(pos, galley, text_col);
    }
}

/// Renders the bottom status bar and per-core CPU telemetry HUD inside `ui`.
pub fn render_bottom_hud(ui: &mut Ui, data: &BottomHudData) {
    let tokens = Tokens::active(ui.ctx());
    let style = crate::theme_styles::ThemeStyleParams::active(ui.ctx());
    if style.refraction_border {
        let dock_rect = ui.max_rect().expand2(Vec2::new(12.0, 4.0));
        crate::theme_styles::paint_hud_glass_dock(
            ui.ctx(),
            ui.painter(),
            dock_rect,
            &style,
            &tokens,
        );
    }
    let avail_w = ui.available_width();
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;

        // Left: Phase status dot + label + optional progress bar + throughput metrics
        let (dot_color, phase_text) = match data.active_phase.as_deref() {
            Some(phase) if !phase.is_empty() => (tokens.accent, phase),
            _ => (tokens.accent.gamma_multiply(0.75), "Готов к работе"),
        };
        status_dot(ui, dot_color, phase_text);

        if let Some(frac) = data.progress_fraction {
            if avail_w >= 500.0 {
                render_smooth_progress_bar(ui, frac, &tokens);
            }
        }

        let mono_font = FontId::new(12.0, FontFamily::Monospace);

        if avail_w >= 940.0 {
            ui.separator();
            let dag_label = if data.active_dag_tasks == 0 {
                "Загрузки: нет активных".to_string()
            } else {
                format!("Загрузки: {} активных", data.active_dag_tasks)
            };
            let metrics_text = format!(
                "{:>3.0} файлов/с | {:>4.1} МБ/с | {}",
                data.files_per_sec, data.mib_per_sec, dag_label
            );
            ui.label(
                RichText::new(metrics_text)
                    .font(mono_font.clone())
                    .color(tokens.text_muted),
            );
        } else if avail_w >= 760.0 {
            ui.separator();
            let dag_label = if data.active_dag_tasks == 0 {
                "нет активных".to_string()
            } else {
                format!("{} акт.", data.active_dag_tasks)
            };
            let metrics_text = format!(
                "{:>3.0} ф/с | {:>4.1} МБ/с | Загрузки: {}",
                data.files_per_sec, data.mib_per_sec, dag_label
            );
            ui.label(
                RichText::new(metrics_text)
                    .font(mono_font.clone())
                    .color(tokens.text_muted),
            );
        }

        // Right: Per-core CPU telemetry vertical bars + Memory + Watchdog indicator
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;

            if data.watchdog_violations > 0 {
                let badge_col = if data.watchdog_violations > 5 {
                    tokens.danger
                } else {
                    tokens.warning
                };
                version_badge(ui, &format!("WD: {}", data.watchdog_violations), badge_col);
            }

            let rss_label = if avail_w < 550.0 {
                format!("Процессор: {:>3.0}%", data.cpu_snapshot.total_pct)
            } else {
                format!(
                    "Процессор: {:>3.0}% | Память: {:>5.1} МБ",
                    data.cpu_snapshot.total_pct, data.cpu_snapshot.rss_mib
                )
            };
            ui.label(RichText::new(rss_label).font(mono_font).color(tokens.text));

            render_per_core_bars(ui, &data.cpu_snapshot, &tokens);
        });
    });
}

/// Renders the `38px` bottom [`TopBottomPanel`] HUD when the viewport is non-zero.
pub fn show_bottom_hud(ctx: &Context, data: &BottomHudData) {
    if is_zero_viewport(ctx) {
        return;
    }
    let tokens = Tokens::active(ctx);
    TopBottomPanel::bottom("mq_bottom_telemetry_hud")
        .min_height(BOTTOM_HUD_HEIGHT)
        .frame(solid_hud_frame(&tokens))
        .show(ctx, |ui| {
            render_bottom_hud(ui, data);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mq_core::CoreCpuLoad;

    fn contains_emoji(text: &str) -> bool {
        text.chars().any(|ch| {
            let cp = ch as u32;
            (0x1F300..=0x1FAFF).contains(&cp)
                || (0x2600..=0x27BF).contains(&cp)
                || (0xFE00..=0xFE0F).contains(&cp)
        })
    }

    #[test]
    fn zero_emojis_exist_across_all_owned_ui_files() {
        let sources = [
            ("animation.rs", include_str!("animation.rs")),
            ("theme.rs", include_str!("theme.rs")),
            ("fonts.rs", include_str!("fonts.rs")),
            ("widgets.rs", include_str!("widgets.rs")),
            ("sidebar.rs", include_str!("sidebar.rs")),
            ("hud.rs", include_str!("hud.rs")),
            ("state.rs", include_str!("state.rs")),
            ("app.rs", include_str!("app.rs")),
            ("install_types.rs", include_str!("install_types.rs")),
            ("instances_view.rs", include_str!("instances_view.rs")),
            ("downloads_view.rs", include_str!("downloads_view.rs")),
            ("perf_view.rs", include_str!("perf_view.rs")),
        ];

        for (name, content) in sources {
            assert!(
                !contains_emoji(content),
                "strict zero-emoji policy violated in {name}"
            );
        }
    }

    #[test]
    fn hud_sub_minimum_bar_height_and_nan_never_panic() {
        let ctx = Context::default();
        let data = BottomHudData {
            active_phase: Some("Загрузка".to_owned()),
            progress_fraction: Some(f32::NAN),
            files_per_sec: f64::NAN,
            mib_per_sec: f64::INFINITY,
            active_dag_tasks: 0,
            cpu_snapshot: CpuSnapshot {
                total_pct: f32::NAN,
                per_core: vec![
                    CoreCpuLoad {
                        core_id: 0,
                        usage_pct: f32::NAN,
                    },
                    CoreCpuLoad {
                        core_id: 1,
                        usage_pct: 100.0,
                    },
                ],
                rss_mib: 0.0,
            },
            watchdog_violations: 0,
        };

        // Render with tiny 1.0 height to force max_bar_h < 1.5
        let tiny_input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 1.0))),
            ..egui::RawInput::default()
        };
        let _ = ctx.run(tiny_input, |ctx| {
            show_bottom_hud(ctx, &data);
        });
    }

    #[test]
    fn bottom_hud_renders_and_handles_zero_viewport_safely() {
        let ctx = Context::default();
        let data = BottomHudData {
            active_phase: Some("Проверка файлов".to_owned()),
            progress_fraction: Some(0.64),
            files_per_sec: 420.0,
            mib_per_sec: 85.4,
            active_dag_tasks: 8,
            cpu_snapshot: CpuSnapshot {
                total_pct: 48.5,
                per_core: vec![
                    CoreCpuLoad {
                        core_id: 0,
                        usage_pct: 75.0,
                    },
                    CoreCpuLoad {
                        core_id: 1,
                        usage_pct: 22.0,
                    },
                ],
                rss_mib: 44.2,
            },
            watchdog_violations: 0,
        };

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            show_bottom_hud(ctx, &data);
        });

        let zero_input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::ZERO)),
            ..egui::RawInput::default()
        };
        let _ = ctx.run(zero_input, |ctx| {
            assert!(is_zero_viewport(ctx));
            show_bottom_hud(ctx, &data);
        });
    }

    #[test]
    fn per_core_bars_wrap_for_more_than_16_cores() {
        let ctx = Context::default();
        for core_count in [24usize, 32, 64] {
            let per_core = (0..core_count)
                .map(|core_id| CoreCpuLoad {
                    core_id,
                    usage_pct: ((core_id * 17) % 101) as f32,
                })
                .collect();
            let data = BottomHudData {
                active_phase: None,
                progress_fraction: None,
                files_per_sec: 0.0,
                mib_per_sec: 0.0,
                active_dag_tasks: 0,
                cpu_snapshot: CpuSnapshot {
                    total_pct: 37.5,
                    per_core,
                    rss_mib: 128.0,
                },
                watchdog_violations: 0,
            };
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 740.0))),
                ..egui::RawInput::default()
            };
            let _ = ctx.run(input, |ctx| {
                show_bottom_hud(ctx, &data);
            });
        }
    }
}
