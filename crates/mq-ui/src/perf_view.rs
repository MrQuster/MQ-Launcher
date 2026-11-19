#![deny(clippy::await_holding_lock)]
//! Performance & Diagnostics dashboard: live CPU, memory, frame-time telemetry,
//! per-core load grid, running Minecraft instances, performance tuning, and
//! storage/cache management. Zero emojis!

use std::collections::VecDeque;

use egui::epaint::{Mesh, Vertex};
use egui::{
    Align, Align2, Color32, CornerRadius, CursorIcon, FontFamily, FontId, Layout, Pos2, Rect,
    RichText, ScrollArea, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};
use mq_core::{GuiBackend, InstanceId};
use mq_instances::InstanceManifest;

use crate::components::registry::{register_widget, ElementKind, ElementMeta};
use crate::state::{safe_clamp_f32, StorageStats, UiState, UiWorkerCommand};
use crate::theme::{blend_color, solid_card_frame_with_config, Tokens, ACCENT_COBALT, RADIUS_TAG};
use crate::theme_styles::{
    lerp_rgba, paint_glass_surface_overlay, paint_tinted_glass_pill, show_themed_card, with_alpha,
    ThemeStyleParams,
};
use crate::widgets::{
    danger_button, loader_badge, paint_vector_icon, primary_emerald_button, secondary_glass_button,
    status_pill, version_badge, VectorIcon,
};

/// 60 FPS frame budget threshold in milliseconds.
pub const TARGET_FRAME_MS_60FPS: f32 = 16.6;

/// UI watchdog frame budget threshold in milliseconds.
pub const WATCHDOG_BUDGET_MS: f32 = 8.0;

/// Navigation actions emitted by [`PerfView`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PerfAction {
    OpenInstanceConsole(mq_core::InstanceId),
}

/// Performance & Diagnostics dashboard view.
#[derive(Default)]
pub struct PerfView;

impl PerfView {
    pub fn ui(&mut self, ui: &mut Ui, state: &mut UiState) -> Option<PerfAction> {
        let tokens = Tokens::from_config(&state.config.theme);
        let style = ThemeStyleParams::active(ui.ctx());
        let mut pending_action: Option<PerfAction> = None;

        ScrollArea::vertical()
            .id_salt("perf_dashboard_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                render_header(ui, state, &tokens);
                ui.add_space(10.0);

                render_kpi_cards(ui, state, &tokens, &style);
                ui.add_space(12.0);

                render_telemetry_charts(ui, state, &tokens, &style);
                ui.add_space(12.0);

                render_per_core_section(ui, state, &tokens, &style);
                ui.add_space(12.0);

                if let Some(action) = render_running_instances_section(ui, state, &tokens, &style) {
                    pending_action = Some(action);
                }
                ui.add_space(12.0);

                render_tuning_section(ui, state, &tokens, &style);
                ui.add_space(12.0);

                render_storage_section(ui, state, &tokens, &style);
                ui.add_space(8.0);
            });

        pending_action
    }
}

/// Sanitized statistical summary for a rolling telemetry series.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesSummary {
    pub count: usize,
    pub current: f32,
    pub avg: f32,
    pub min: f32,
    pub max: f32,
    pub values: Vec<f32>,
}

/// Sanitizes an `f32` sample to a finite non-negative value (`NaN`, `Inf`, and negatives become `0.0`).
#[inline]
pub fn sanitize_nonneg_f32(val: f32) -> f32 {
    if val.is_finite() && val >= 0.0 {
        val
    } else {
        0.0
    }
}

/// Sanitizes an `f64` sample to a finite non-negative `f32` (`NaN`, `Inf`, and negatives become `0.0`).
#[inline]
pub fn sanitize_nonneg_f64_as_f32(val: f64) -> f32 {
    if val.is_finite() && val >= 0.0 {
        if val > f32::MAX as f64 {
            f32::MAX
        } else {
            val as f32
        }
    } else {
        0.0
    }
}

/// Sanitizes an `f64` value to a finite non-negative `f64`.
#[inline]
pub fn sanitize_nonneg_f64(val: f64) -> f64 {
    if val.is_finite() && val >= 0.0 {
        val
    } else {
        0.0
    }
}

fn summarize_sanitized_values(values: Vec<f32>) -> SeriesSummary {
    if values.is_empty() {
        return SeriesSummary {
            count: 0,
            current: 0.0,
            avg: 0.0,
            min: 0.0,
            max: 0.0,
            values,
        };
    }

    let mut sum = 0.0_f64;
    let mut min_v = f32::MAX;
    let mut max_v = 0.0_f32;
    for &v in &values {
        sum += v as f64;
        if v < min_v {
            min_v = v;
        }
        if v > max_v {
            max_v = v;
        }
    }

    let count = values.len();
    let current = *values.last().unwrap_or(&0.0);
    let avg = (sum / count as f64) as f32;

    SeriesSummary {
        count,
        current,
        avg: if avg.is_finite() { avg } else { 0.0 },
        min: if min_v.is_finite() { min_v } else { 0.0 },
        max: max_v,
        values,
    }
}

/// Computes sanitized summary statistics for a `VecDeque<f32>` series.
pub fn summarize_f32_series(samples: &VecDeque<f32>) -> SeriesSummary {
    let values = samples.iter().map(|&v| sanitize_nonneg_f32(v)).collect();
    summarize_sanitized_values(values)
}

/// Computes sanitized summary statistics for a `VecDeque<f64>` series.
pub fn summarize_f64_series(samples: &VecDeque<f64>) -> SeriesSummary {
    let values = samples
        .iter()
        .map(|&v| sanitize_nonneg_f64_as_f32(v))
        .collect();
    summarize_sanitized_values(values)
}

/// Computes approximate FPS from frame duration in milliseconds (`None` for 0, negative, or non-finite).
#[inline]
pub fn compute_approx_fps(frame_ms: f32) -> Option<u32> {
    if !frame_ms.is_finite() || frame_ms <= 0.0 {
        return None;
    }
    let fps = 1000.0_f64 / frame_ms as f64;
    if !fps.is_finite() {
        return None;
    }
    Some(fps.round().clamp(0.0, 9999.0) as u32)
}

/// Formats approximate FPS label safely (`"— FPS"` when unavailable).
pub fn format_fps_label(frame_ms: f32) -> String {
    match compute_approx_fps(frame_ms) {
        Some(fps) => format!("{fps} FPS"),
        None => "— FPS".to_string(),
    }
}

/// Formats frame duration in milliseconds safely (`"—"` when zero/non-finite).
pub fn format_frame_ms(frame_ms: f32) -> String {
    if !frame_ms.is_finite() || frame_ms <= 0.0 {
        "—".to_string()
    } else if frame_ms >= 100.0 {
        format!("{frame_ms:.0} мс")
    } else if frame_ms >= 10.0 {
        format!("{frame_ms:.1} мс")
    } else {
        format!("{frame_ms:.2} мс")
    }
}

/// Formats byte counts in Russian (`Б`, `КБ`, `МБ`, `ГБ`).
pub fn format_bytes_ru(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    let b = bytes as f64;
    if b >= GIB {
        format!("{:.2} ГБ", b / GIB)
    } else if b >= MIB {
        format!("{:.1} МБ", b / MIB)
    } else if b >= KIB {
        format!("{:.0} КБ", b / KIB)
    } else {
        format!("{bytes} Б")
    }
}

/// Formats memory size given in MiB safely into Russian (`МБ` or `ГБ`).
pub fn format_mib_ru(mib: f64) -> String {
    let safe = sanitize_nonneg_f64(mib);
    if safe >= 1024.0 {
        format!("{:.2} ГБ", safe / 1024.0)
    } else {
        format!("{safe:.1} МБ")
    }
}

/// Safely computes storage share fraction in `[0.0, 1.0]`.
#[inline]
pub fn storage_share_fraction(part: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    if part >= total {
        return 1.0;
    }
    safe_clamp_f32((part as f64 / total as f64) as f32, 0.0, 1.0)
}

/// Computes a finite chart ceiling at least `min_floor` with headroom.
pub fn nice_chart_ceiling(max_val: f32, min_floor: f32) -> f32 {
    let floor = if min_floor.is_finite() && min_floor > 0.0 {
        min_floor
    } else {
        1.0
    };
    if !max_val.is_finite() || max_val <= 0.0 {
        return floor;
    }
    (max_val * 1.15).max(floor)
}

/// Converts a slice of values into clamped screen coordinates inside `plot_rect`.
pub fn normalize_chart_points(
    values: &[f32],
    plot_rect: Rect,
    y_min: f32,
    y_max: f32,
) -> Vec<Pos2> {
    if values.is_empty()
        || !plot_rect.is_positive()
        || !plot_rect.width().is_finite()
        || !plot_rect.height().is_finite()
        || plot_rect.width() <= 1.0
        || plot_rect.height() <= 1.0
    {
        return Vec::new();
    }

    let safe_min = if y_min.is_finite() { y_min } else { 0.0 };
    let safe_max = if y_max.is_finite() && y_max > safe_min {
        y_max
    } else {
        safe_min + 1.0
    };
    let span = (safe_max - safe_min).max(f32::EPSILON);

    let n = values.len();
    let mut pts = Vec::with_capacity(n);
    for (idx, &raw) in values.iter().enumerate() {
        let v = sanitize_nonneg_f32(raw);
        let norm_y = safe_clamp_f32((v - safe_min) / span, 0.0, 1.0);
        let norm_x = if n == 1 {
            1.0_f32
        } else {
            idx as f32 / (n - 1) as f32
        };
        let x = plot_rect.left() + norm_x * plot_rect.width();
        let y = plot_rect.bottom() - norm_y * plot_rect.height();
        pts.push(Pos2::new(x, y));
    }
    pts
}

/// Computes responsive column count (`1..=4`) for the per-core CPU meter grid.
pub fn per_core_column_count(avail_width: f32, core_count: usize) -> usize {
    if core_count == 0 {
        return 1;
    }
    let w = if avail_width.is_finite() && avail_width > 0.0 {
        avail_width
    } else {
        200.0
    };
    let by_width = (w / 205.0).floor().clamp(1.0, 4.0) as usize;
    by_width.clamp(1, core_count)
}

fn resolve_core_count(state: &UiState) -> usize {
    if !state.latest_cpu.per_core.is_empty() {
        state.latest_cpu.per_core.len()
    } else {
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
    }
}

fn load_color(tokens: &Tokens, pct: f32) -> Color32 {
    if pct >= 90.0 {
        tokens.danger
    } else if pct >= 70.0 {
        tokens.warning
    } else if pct >= 45.0 {
        ACCENT_COBALT
    } else {
        tokens.accent
    }
}

fn paint_header_title(ui: &mut Ui, tokens: &Tokens) {
    let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
    paint_vector_icon(ui.painter(), icon_rect, VectorIcon::Activity, tokens.accent);
    ui.heading(
        RichText::new("Производительность и диагностика")
            .color(tokens.text)
            .strong(),
    );
}

fn render_header(ui: &mut Ui, state: &UiState, tokens: &Tokens) {
    let violations = state.watchdog.violation_count();
    let last_ms = sanitize_nonneg_f32(state.last_frame_ms);
    let backend = if state.gpu_backend_label.trim().is_empty() {
        "Auto"
    } else {
        state.gpu_backend_label.trim()
    };

    let (wd_label, wd_color) = if violations == 0 && last_ms <= TARGET_FRAME_MS_60FPS {
        ("Интерфейс плавный".to_string(), tokens.success)
    } else if violations == 0 {
        ("Высокая нагрузка кадра".to_string(), tokens.warning)
    } else {
        (format!("Фризы >8 мс: {violations}"), tokens.warning)
    };

    if ui.available_width() >= 600.0 {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            paint_header_title(ui, tokens);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                status_pill(ui, &wd_label, wd_color);
                version_badge(ui, backend, ACCENT_COBALT);
            });
        });
    } else {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            paint_header_title(ui, tokens);
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
            version_badge(ui, backend, ACCENT_COBALT);
            status_pill(ui, &wd_label, wd_color);
        });
    }

    ui.add_space(2.0);
    ui.label(
        RichText::new(
            "Телеметрия процессора (/proc/stat), памяти, времени кадра, активных экземпляров игры и дискового пространства.",
        )
        .color(tokens.text_muted)
        .small(),
    );
}

fn render_kpi_cards(ui: &mut Ui, state: &UiState, tokens: &Tokens, style: &ThemeStyleParams) {
    let cpu_summary = summarize_f32_series(&state.cpu_history);
    let rss_summary = summarize_f64_series(&state.rss_history);
    let core_count = resolve_core_count(state);
    let cpu_pct = sanitize_nonneg_f32(state.latest_cpu.total_pct);
    let current_rss = sanitize_nonneg_f64(state.latest_cpu.rss_mib);
    let peak_rss = sanitize_nonneg_f64(state.peak_rss_mib).max(current_rss);
    let last_ms = sanitize_nonneg_f32(state.last_frame_ms);
    let max_ms = sanitize_nonneg_f32(state.max_frame_ms).max(last_ms);
    let violations = state.watchdog.violation_count();
    let backend_label = if state.gpu_backend_label.trim().is_empty() {
        "Auto"
    } else {
        state.gpu_backend_label.trim()
    };

    let cr = style.card_corner_radius(state.config.theme.corner_radius);
    let card_frame = solid_card_frame_with_config(tokens, &state.config.theme)
        .inner_margin(egui::Margin::same(14));

    let draw_cpu_card = |ui: &mut Ui| {
        show_themed_card(
            ui,
            card_frame,
            cr,
            ui.make_persistent_id("perf_kpi_cpu_card"),
            style,
            tokens,
            |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(104.0);

                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                    paint_vector_icon(ui.painter(), r, VectorIcon::Cpu, tokens.accent);
                    ui.label(RichText::new("Процессор").color(tokens.text).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        version_badge(ui, &format!("{core_count} ядер"), ACCENT_COBALT);
                    });
                });

                ui.add_space(6.0);
                let col = load_color(tokens, cpu_pct);
                ui.label(
                    RichText::new(format!("{cpu_pct:.1}%"))
                        .font(FontId::new(22.0, FontFamily::Monospace))
                        .color(col)
                        .strong(),
                );

                ui.add_space(6.0);
                let frac = safe_clamp_f32(cpu_pct / 100.0, 0.0, 1.0);
                let smooth = crate::animation::animate_spring_f32(
                    ui.ctx(),
                    egui::Id::new("mq_perf_kpi_cpu_bar"),
                    frac,
                    crate::animation::SpringConfig::smooth(),
                )
                .clamp(0.0, 1.0);
                paint_mini_meter(ui, smooth, col, tokens, 6.0);

                ui.add_space(6.0);
                let sub = if cpu_summary.count > 0 {
                    format!(
                        "Средняя: {:.1}% · Пик: {:.1}%",
                        cpu_summary.avg, cpu_summary.max
                    )
                } else {
                    "Ожидание снимка /proc/stat".to_string()
                };
                ui.label(RichText::new(sub).color(tokens.text_muted).small());
            },
        );
    };

    let draw_ram_card = |ui: &mut Ui| {
        show_themed_card(
            ui,
            card_frame,
            cr,
            ui.make_persistent_id("perf_kpi_ram_card"),
            style,
            tokens,
            |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(104.0);

                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                    paint_vector_icon(ui.painter(), r, VectorIcon::Server, ACCENT_COBALT);
                    ui.label(RichText::new("Память лаунчера").color(tokens.text).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        version_badge(ui, "Процесс", tokens.text_muted);
                    });
                });

                ui.add_space(6.0);
                ui.label(
                    RichText::new(format_mib_ru(current_rss))
                        .font(FontId::new(22.0, FontFamily::Monospace))
                        .color(tokens.text)
                        .strong(),
                );

                ui.add_space(6.0);
                let rel_frac = if peak_rss > 0.0 {
                    safe_clamp_f32((current_rss / peak_rss) as f32, 0.0, 1.0)
                } else {
                    0.0
                };
                let smooth = crate::animation::animate_spring_f32(
                    ui.ctx(),
                    egui::Id::new("mq_perf_kpi_ram_bar"),
                    rel_frac,
                    crate::animation::SpringConfig::smooth(),
                )
                .clamp(0.0, 1.0);
                paint_mini_meter(ui, smooth, ACCENT_COBALT, tokens, 6.0);

                ui.add_space(6.0);
                let avg_txt = if rss_summary.count > 0 {
                    format_mib_ru(rss_summary.avg as f64)
                } else {
                    "—".to_string()
                };
                ui.label(
                    RichText::new(format!(
                        "Пик: {} · Средняя: {avg_txt}",
                        format_mib_ru(peak_rss)
                    ))
                    .color(tokens.text_muted)
                    .small(),
                );
            },
        );
    };

    let draw_render_card = |ui: &mut Ui| {
        show_themed_card(
            ui,
            card_frame,
            cr,
            ui.make_persistent_id("perf_kpi_render_card"),
            style,
            tokens,
            |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(104.0);

                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                    paint_vector_icon(ui.painter(), r, VectorIcon::Activity, tokens.accent);
                    ui.label(
                        RichText::new("Рендеринг и процесс")
                            .color(tokens.text)
                            .strong(),
                    );
                });

                ui.add_space(6.0);
                let frame_col = if last_ms > TARGET_FRAME_MS_60FPS {
                    tokens.warning
                } else {
                    tokens.text
                };
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        format_frame_ms(last_ms),
                        format_fps_label(last_ms)
                    ))
                    .font(FontId::new(20.0, FontFamily::Monospace))
                    .color(frame_col)
                    .strong(),
                );

                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "Макс. кадр: {} · Фризы >8 мс: {violations}",
                        format_frame_ms(max_ms)
                    ))
                    .color(tokens.text_muted)
                    .small(),
                );
                ui.label(
                    RichText::new(format!(
                        "Бэкенд: {backend_label} · Дескрипторы: {}",
                        state.open_fds
                    ))
                    .color(tokens.text_muted)
                    .small(),
                );
            },
        );
    };

    if ui.available_width() >= 700.0 {
        ui.columns(3, |cols| {
            draw_cpu_card(&mut cols[0]);
            draw_ram_card(&mut cols[1]);
            draw_render_card(&mut cols[2]);
        });
    } else {
        draw_cpu_card(ui);
        ui.add_space(8.0);
        draw_ram_card(ui);
        ui.add_space(8.0);
        draw_render_card(ui);
    }
}

struct ChartSpec<'a> {
    card_id: &'a str,
    title: &'a str,
    unit_suffix: &'a str,
    summary: &'a SeriesSummary,
    y_min: f32,
    y_max: f32,
    line_color: Color32,
    reference_line: Option<(f32, &'a str, Color32)>,
    decimals: usize,
}

fn format_metric_value(val: f32, decimals: usize, unit_suffix: &str) -> String {
    let v = sanitize_nonneg_f32(val);
    if unit_suffix.is_empty() {
        format!("{v:.decimals$}")
    } else {
        format!("{v:.decimals$} {unit_suffix}")
    }
}

fn render_telemetry_charts(
    ui: &mut Ui,
    state: &UiState,
    tokens: &Tokens,
    style: &ThemeStyleParams,
) {
    let cpu_summary = summarize_f32_series(&state.cpu_history);
    let rss_summary = summarize_f64_series(&state.rss_history);
    let frame_summary = summarize_f32_series(&state.frame_time_ms_history);

    let cpu_y_max = cpu_summary.max.max(100.0);
    let ram_y_max = nice_chart_ceiling(rss_summary.max, 64.0);
    let frame_y_max = nice_chart_ceiling(frame_summary.max.max(TARGET_FRAME_MS_60FPS), 22.0);
    let frame_color = if frame_summary.current > TARGET_FRAME_MS_60FPS {
        tokens.warning
    } else {
        tokens.success
    };

    let cpu_spec = ChartSpec {
        card_id: "perf_chart_cpu_card",
        title: "История загрузки CPU (%)",
        unit_suffix: "%",
        summary: &cpu_summary,
        y_min: 0.0,
        y_max: cpu_y_max,
        line_color: tokens.accent,
        reference_line: None,
        decimals: 1,
    };

    let ram_spec = ChartSpec {
        card_id: "perf_chart_ram_card",
        title: "Потребление RAM (МБ)",
        unit_suffix: "МБ",
        summary: &rss_summary,
        y_min: 0.0,
        y_max: ram_y_max,
        line_color: ACCENT_COBALT,
        reference_line: None,
        decimals: 1,
    };

    let frame_spec = ChartSpec {
        card_id: "perf_chart_frame_card",
        title: "Время кадра (мс)",
        unit_suffix: "мс",
        summary: &frame_summary,
        y_min: 0.0,
        y_max: frame_y_max,
        line_color: frame_color,
        reference_line: Some((TARGET_FRAME_MS_60FPS, "16.6 мс (60 FPS)", tokens.warning)),
        decimals: 2,
    };

    let cr = style.card_corner_radius(state.config.theme.corner_radius);
    let card_frame = solid_card_frame_with_config(tokens, &state.config.theme)
        .inner_margin(egui::Margin::same(12));

    if ui.available_width() >= 760.0 {
        ui.columns(3, |cols| {
            render_chart_card(&mut cols[0], card_frame, cr, style, tokens, &cpu_spec);
            render_chart_card(&mut cols[1], card_frame, cr, style, tokens, &ram_spec);
            render_chart_card(&mut cols[2], card_frame, cr, style, tokens, &frame_spec);
        });
    } else {
        render_chart_card(ui, card_frame, cr, style, tokens, &cpu_spec);
        ui.add_space(8.0);
        render_chart_card(ui, card_frame, cr, style, tokens, &ram_spec);
        ui.add_space(8.0);
        render_chart_card(ui, card_frame, cr, style, tokens, &frame_spec);
    }
}

fn render_chart_card(
    ui: &mut Ui,
    card_frame: egui::Frame,
    cr: CornerRadius,
    style: &ThemeStyleParams,
    tokens: &Tokens,
    spec: &ChartSpec<'_>,
) {
    show_themed_card(
        ui,
        card_frame,
        cr,
        ui.make_persistent_id(spec.card_id),
        style,
        tokens,
        |ui| {
            ui.set_width(ui.available_width());

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(spec.title)
                        .color(tokens.text)
                        .strong()
                        .size(13.0),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let cur_txt = if spec.summary.count > 0 {
                        format_metric_value(spec.summary.current, spec.decimals, spec.unit_suffix)
                    } else {
                        "—".to_string()
                    };
                    ui.label(
                        RichText::new(cur_txt)
                            .font(FontId::new(12.5, FontFamily::Monospace))
                            .color(spec.line_color)
                            .strong(),
                    );
                });
            });

            ui.add_space(6.0);
            render_sparkline_canvas(ui, tokens, style, spec);
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                let summary_text = if spec.summary.count > 0 {
                    format!(
                        "Средн: {} · Пик: {}",
                        format_metric_value(spec.summary.avg, spec.decimals, spec.unit_suffix),
                        format_metric_value(spec.summary.max, spec.decimals, spec.unit_suffix)
                    )
                } else {
                    "Нет накопленных отсчётов".to_string()
                };
                ui.label(RichText::new(summary_text).color(tokens.text_muted).small());

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} т.", spec.summary.count))
                            .font(FontId::new(11.0, FontFamily::Monospace))
                            .color(tokens.text_muted),
                    );
                });
            });
        },
    );
}

fn render_sparkline_canvas(
    ui: &mut Ui,
    tokens: &Tokens,
    style: &ThemeStyleParams,
    spec: &ChartSpec<'_>,
) {
    let desired_w = ui.available_width().max(80.0);
    let desired_h = 108.0;
    let (canvas_rect, resp) =
        ui.allocate_exact_size(Vec2::new(desired_w, desired_h), Sense::hover());

    if !ui.is_rect_visible(canvas_rect)
        || !canvas_rect.is_positive()
        || canvas_rect.width() < 24.0
        || canvas_rect.height() < 24.0
    {
        return;
    }

    let plot_rect = canvas_rect.shrink2(Vec2::new(8.0, 12.0));
    let pts = normalize_chart_points(&spec.summary.values, plot_rect, spec.y_min, spec.y_max);
    let mut hover_msg: Option<String> = None;

    {
        let painter = ui.painter();
        let bg_fill = if style.refraction_border {
            with_alpha(tokens.bg_input, 0.72)
        } else {
            tokens.bg_input
        };
        painter.rect_filled(canvas_rect, RADIUS_TAG, bg_fill);
        painter.rect_stroke(
            canvas_rect,
            RADIUS_TAG,
            Stroke::new(1.0_f32, tokens.border),
            StrokeKind::Inside,
        );

        // Horizontal grid lines at 25%, 50%, 75%
        let grid_stroke = Stroke::new(1.0_f32, tokens.border.gamma_multiply(0.55));
        for frac in [0.25_f32, 0.50_f32, 0.75_f32] {
            let y = plot_rect.bottom() - frac * plot_rect.height();
            painter.line_segment(
                [
                    Pos2::new(plot_rect.left(), y),
                    Pos2::new(plot_rect.right(), y),
                ],
                grid_stroke,
            );
        }

        // Top / bottom axis scale labels
        let scale_font = FontId::new(10.0, FontFamily::Monospace);
        let scale_col = tokens.text_muted.gamma_multiply(0.85);
        painter.text(
            Pos2::new(plot_rect.right(), canvas_rect.top() + 2.0),
            Align2::RIGHT_TOP,
            format_metric_value(spec.y_max, 0, spec.unit_suffix),
            scale_font.clone(),
            scale_col,
        );
        painter.text(
            Pos2::new(plot_rect.right(), canvas_rect.bottom() - 2.0),
            Align2::RIGHT_BOTTOM,
            format_metric_value(spec.y_min, 0, spec.unit_suffix),
            scale_font,
            scale_col,
        );

        // Optional reference threshold line (e.g. 16.6 ms = 60 FPS)
        if let Some((ref_val, ref_label, ref_col)) = spec.reference_line {
            let span = (spec.y_max - spec.y_min).max(f32::EPSILON);
            let norm = safe_clamp_f32((ref_val - spec.y_min) / span, 0.0, 1.0);
            let ref_y = plot_rect.bottom() - norm * plot_rect.height();
            let dash_stroke = Stroke::new(1.2_f32, ref_col.gamma_multiply(0.85));
            let mut x = plot_rect.left();
            while x < plot_rect.right() {
                let x_end = (x + 6.0).min(plot_rect.right());
                painter.line_segment([Pos2::new(x, ref_y), Pos2::new(x_end, ref_y)], dash_stroke);
                x += 10.0;
            }
            let (label_y, label_align) = if ref_y - plot_rect.top() < 14.0 {
                (ref_y + 2.0, Align2::LEFT_TOP)
            } else {
                (ref_y - 2.0, Align2::LEFT_BOTTOM)
            };
            painter.text(
                Pos2::new(plot_rect.left() + 2.0, label_y),
                label_align,
                ref_label,
                FontId::proportional(10.5),
                ref_col,
            );
        }

        if pts.is_empty() {
            painter.text(
                canvas_rect.center(),
                Align2::CENTER_CENTER,
                "Ожидание данных...",
                FontId::proportional(11.5),
                tokens.text_muted,
            );
        } else if pts.len() == 1 {
            let p = pts[0];
            painter.line_segment(
                [
                    Pos2::new(plot_rect.left(), p.y),
                    Pos2::new(plot_rect.right(), p.y),
                ],
                Stroke::new(1.5_f32, spec.line_color.gamma_multiply(0.7)),
            );
            painter.circle_filled(p, 4.5, spec.line_color.gamma_multiply(0.28));
            painter.circle_filled(p, 2.5, spec.line_color);
        } else {
            // Trapezoid strip mesh avoids concave polygon artifacts and keeps fill calm.
            let mut mesh = Mesh::default();
            let top_col = Color32::from_rgba_unmultiplied(
                spec.line_color.r(),
                spec.line_color.g(),
                spec.line_color.b(),
                52,
            );
            let bot_col = Color32::from_rgba_unmultiplied(
                spec.line_color.r(),
                spec.line_color.g(),
                spec.line_color.b(),
                4,
            );
            let baseline_y = plot_rect.bottom();

            for &pt in &pts {
                mesh.vertices.push(Vertex {
                    pos: pt,
                    uv: Pos2::ZERO,
                    color: top_col,
                });
                mesh.vertices.push(Vertex {
                    pos: Pos2::new(pt.x, baseline_y),
                    uv: Pos2::ZERO,
                    color: bot_col,
                });
            }
            for i in 0..(pts.len() - 1) {
                let t0 = (i * 2) as u32;
                let b0 = t0 + 1;
                let t1 = t0 + 2;
                let b1 = t0 + 3;
                mesh.add_triangle(t0, t1, b0);
                mesh.add_triangle(t1, b1, b0);
            }
            painter.add(mesh);

            painter.add(Shape::line(
                pts.clone(),
                Stroke::new(1.75_f32, spec.line_color),
            ));

            if let Some(&last_pt) = pts.last() {
                painter.circle_filled(last_pt, 4.5, spec.line_color.gamma_multiply(0.28));
                painter.circle_filled(last_pt, 2.5, spec.line_color);
            }
        }

        if resp.hovered() && !pts.is_empty() {
            if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                if canvas_rect.contains(pointer_pos) {
                    let idx = if pts.len() == 1 {
                        0
                    } else {
                        let t = ((pointer_pos.x - plot_rect.left()) / plot_rect.width().max(1.0))
                            .clamp(0.0, 1.0);
                        (t * (pts.len() - 1) as f32).round() as usize
                    };
                    let idx = idx.min(pts.len() - 1);
                    let pt = pts[idx];
                    painter.line_segment(
                        [
                            Pos2::new(pt.x, plot_rect.top()),
                            Pos2::new(pt.x, plot_rect.bottom()),
                        ],
                        Stroke::new(1.0_f32, tokens.text_muted.gamma_multiply(0.65)),
                    );
                    painter.circle_filled(pt, 3.5, spec.line_color);
                    painter.circle_stroke(pt, 5.0, Stroke::new(1.0_f32, tokens.text));

                    let val = spec.summary.values.get(idx).copied().unwrap_or(0.0);
                    let extra = if spec.reference_line.is_some() {
                        format!(" ({})", format_fps_label(val))
                    } else {
                        String::new()
                    };
                    hover_msg = Some(format!(
                        "{}\nОтсчёт #{} из {}: {}{}",
                        spec.title,
                        idx + 1,
                        spec.summary.count,
                        format_metric_value(val, spec.decimals, spec.unit_suffix),
                        extra
                    ));
                }
            }
        }
    }

    if let Some(msg) = hover_msg {
        let _ = resp.on_hover_text(msg);
    }
}

fn paint_mini_meter(ui: &mut Ui, frac: f32, fill_color: Color32, tokens: &Tokens, height: f32) {
    let clamped = safe_clamp_f32(frac, 0.0, 1.0);
    let desired = Vec2::new(ui.available_width().max(24.0), height.max(3.0));
    let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());

    if ui.is_rect_visible(rect) && rect.is_positive() {
        let painter = ui.painter();
        let cr = CornerRadius::same((height * 0.5).round().clamp(1.0, 6.0) as u8);
        painter.rect_filled(rect, cr, tokens.bg_input);
        painter.rect_stroke(
            rect,
            cr,
            Stroke::new(1.0_f32, tokens.border.gamma_multiply(0.6)),
            StrokeKind::Inside,
        );
        if clamped > 0.005 {
            let fill_w = (rect.width() * clamped).max(3.0);
            let fill_rect = Rect::from_min_size(rect.min, Vec2::new(fill_w, rect.height()));
            painter.rect_filled(fill_rect, cr, fill_color);
        }
    }
}

fn render_per_core_section(
    ui: &mut Ui,
    state: &UiState,
    tokens: &Tokens,
    style: &ThemeStyleParams,
) {
    let cr = style.card_corner_radius(state.config.theme.corner_radius);
    let card_frame = solid_card_frame_with_config(tokens, &state.config.theme)
        .inner_margin(egui::Margin::same(14));

    let cores = &state.latest_cpu.per_core;
    let busiest = cores
        .iter()
        .max_by(|a, b| {
            sanitize_nonneg_f32(a.usage_pct)
                .partial_cmp(&sanitize_nonneg_f32(b.usage_pct))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied();

    show_themed_card(
        ui,
        card_frame,
        cr,
        ui.make_persistent_id("perf_cores_card"),
        style,
        tokens,
        |ui| {
            ui.set_width(ui.available_width());

            if ui.available_width() >= 560.0 {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                    paint_vector_icon(ui.painter(), r, VectorIcon::Cpu, tokens.accent);
                    ui.label(
                        RichText::new("Загрузка логических ядер (/proc/stat)")
                            .color(tokens.text)
                            .strong()
                            .size(14.0),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if let Some(peak_core) = busiest {
                            let peak_pct = sanitize_nonneg_f32(peak_core.usage_pct);
                            version_badge(
                                ui,
                                &format!("Пик ядра #{}: {:.1}%", peak_core.core_id, peak_pct),
                                load_color(tokens, peak_pct),
                            );
                        }
                        version_badge(
                            ui,
                            &format!("Логических ядер: {}", resolve_core_count(state)),
                            ACCENT_COBALT,
                        );
                    });
                });
            } else {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                    paint_vector_icon(ui.painter(), r, VectorIcon::Cpu, tokens.accent);
                    ui.label(
                        RichText::new("Загрузка логических ядер (/proc/stat)")
                            .color(tokens.text)
                            .strong()
                            .size(14.0),
                    );
                });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                    version_badge(
                        ui,
                        &format!("Логических ядер: {}", resolve_core_count(state)),
                        ACCENT_COBALT,
                    );
                    if let Some(peak_core) = busiest {
                        let peak_pct = sanitize_nonneg_f32(peak_core.usage_pct);
                        version_badge(
                            ui,
                            &format!("Пик ядра #{}: {:.1}%", peak_core.core_id, peak_pct),
                            load_color(tokens, peak_pct),
                        );
                    }
                });
            }

            ui.add_space(8.0);

            if cores.is_empty() {
                ui.label(
                    RichText::new("Ожидание данных по отдельным ядрам от фонового сэмплера...")
                        .color(tokens.text_muted)
                        .small(),
                );
                return;
            }

            let cols = per_core_column_count(ui.available_width(), cores.len());
            for (row_idx, chunk) in cores.chunks(cols).enumerate() {
                if cols == 1 {
                    for (offset, core) in chunk.iter().enumerate() {
                        render_single_core_meter(ui, row_idx * cols + offset, core, tokens);
                    }
                } else {
                    ui.columns(cols, |col_uis| {
                        for (col_idx, core) in chunk.iter().enumerate() {
                            render_single_core_meter(
                                &mut col_uis[col_idx],
                                row_idx * cols + col_idx,
                                core,
                                tokens,
                            );
                        }
                    });
                }
                ui.add_space(4.0);
            }
        },
    );
}

fn render_single_core_meter(
    ui: &mut Ui,
    slot_idx: usize,
    core: &mq_core::CoreCpuLoad,
    tokens: &Tokens,
) {
    let pct = safe_clamp_f32(sanitize_nonneg_f32(core.usage_pct), 0.0, 100.0);
    let fill_col = load_color(tokens, pct);
    let smooth = crate::animation::animate_spring_f32(
        ui.ctx(),
        egui::Id::new(("mq_perf_core_meter", slot_idx, core.core_id)),
        pct / 100.0,
        crate::animation::SpringConfig::smooth(),
    )
    .clamp(0.0, 1.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.add_sized(
            [48.0, 18.0],
            egui::Label::new(
                RichText::new(format!("cpu{:02}", core.core_id))
                    .font(FontId::new(11.5, FontFamily::Monospace))
                    .color(tokens.text_muted),
            ),
        );

        let bar_w = (ui.available_width() - 56.0).max(32.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(bar_w, 8.0), Sense::hover());
        if ui.is_rect_visible(rect) && rect.is_positive() {
            let painter = ui.painter();
            let cr = CornerRadius::same(3);
            painter.rect_filled(rect, cr, tokens.bg_input);
            painter.rect_stroke(
                rect,
                cr,
                Stroke::new(1.0_f32, tokens.border.gamma_multiply(0.6)),
                StrokeKind::Inside,
            );
            if smooth > 0.005 {
                let fill_w = (rect.width() * smooth).max(3.0);
                let fill_rect = Rect::from_min_size(rect.min, Vec2::new(fill_w, rect.height()));
                painter.rect_filled(fill_rect, cr, fill_col);
            }
        }

        ui.add_sized(
            [48.0, 18.0],
            egui::Label::new(
                RichText::new(format!("{pct:>5.1}%"))
                    .font(FontId::new(11.5, FontFamily::Monospace))
                    .color(tokens.text),
            ),
        );
    });
}

fn render_running_instances_section(
    ui: &mut Ui,
    state: &UiState,
    tokens: &Tokens,
    style: &ThemeStyleParams,
) -> Option<PerfAction> {
    let cr = style.card_corner_radius(state.config.theme.corner_radius);
    let card_frame = solid_card_frame_with_config(tokens, &state.config.theme)
        .inner_margin(egui::Margin::same(14));

    let mut running_rows: Vec<(InstanceId, Option<u32>, Option<&InstanceManifest>)> = state
        .running_instances
        .iter()
        .map(|(id, pid)| {
            let manifest = state.instances.iter().find(|inst| inst.id == *id);
            (id.clone(), *pid, manifest)
        })
        .collect();
    running_rows.sort_by(|a, b| {
        let name_a = a.2.map_or(a.0.as_str(), |m| m.name.as_str());
        let name_b = b.2.map_or(b.0.as_str(), |m| m.name.as_str());
        name_a
            .cmp(name_b)
            .then_with(|| a.0.as_str().cmp(b.0.as_str()))
    });

    let mut action: Option<PerfAction> = None;

    show_themed_card(
        ui,
        card_frame,
        cr,
        ui.make_persistent_id("perf_running_games_card"),
        style,
        tokens,
        |ui| {
            ui.set_width(ui.available_width());

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                paint_vector_icon(ui.painter(), r, VectorIcon::Play, tokens.accent);
                ui.label(
                    RichText::new("Запущенные игры и процессы")
                        .color(tokens.text)
                        .strong()
                        .size(14.0),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let badge_col = if running_rows.is_empty() {
                        tokens.text_muted
                    } else {
                        tokens.success
                    };
                    version_badge(ui, &format!("Активно: {}", running_rows.len()), badge_col);
                });
            });

            ui.add_space(8.0);

            if running_rows.is_empty() {
                egui::Frame::new()
                    .fill(if style.refraction_border {
                        with_alpha(tokens.bg_input, 0.65)
                    } else {
                        tokens.bg_input
                    })
                    .stroke(Stroke::new(1.0_f32, tokens.border))
                    .corner_radius(RADIUS_TAG)
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(10.0, 4.0);
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                            paint_vector_icon(
                                ui.painter(),
                                r,
                                VectorIcon::Terminal,
                                tokens.text_muted,
                            );
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("Сейчас нет запущенных экземпляров игры")
                                        .color(tokens.text)
                                        .strong(),
                                );
                                ui.label(
                                    RichText::new(
                                        "После запуска Minecraft здесь появятся активные процессы, выделенная память и быстрый переход в консоль.",
                                    )
                                    .color(tokens.text_muted)
                                    .small(),
                                );
                            });
                        });
                    });
                return;
            }

            for (idx, (id, pid_opt, manifest_opt)) in running_rows.iter().enumerate() {
                if idx > 0 {
                    ui.add_space(6.0);
                }

                egui::Frame::new()
                    .fill(if style.refraction_border {
                        with_alpha(tokens.bg_input, 0.72)
                    } else {
                        tokens.bg_input
                    })
                    .stroke(Stroke::new(1.0_f32, tokens.border))
                    .corner_radius(RADIUS_TAG)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;

                            let (dot_rect, _) =
                                ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                            if ui.is_rect_visible(dot_rect) {
                                ui.painter()
                                    .circle_filled(dot_rect.center(), 4.0, tokens.success);
                            }

                            let display_name =
                                manifest_opt.map_or(id.as_str(), |m| m.name.as_str());
                            ui.label(
                                RichText::new(display_name)
                                    .color(tokens.text)
                                    .strong()
                                    .size(13.5),
                            );

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                if danger_button(ui, Some(VectorIcon::Stop), "Остановить").clicked()
                                {
                                    let _ = state.cmd_sender.send(UiWorkerCommand::StopGame {
                                        instance_id: id.clone(),
                                    });
                                }
                                if secondary_glass_button(
                                    ui,
                                    Some(VectorIcon::Terminal),
                                    "Открыть консоль",
                                )
                                .clicked()
                                {
                                    action = Some(PerfAction::OpenInstanceConsole(id.clone()));
                                }
                            });
                        });

                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                            if let Some(inst) = manifest_opt {
                                loader_badge(ui, inst.loader);
                                version_badge(ui, &inst.mc_version, ACCENT_COBALT);
                                version_badge(
                                    ui,
                                    &format!(
                                        "RAM: {}-{} МБ",
                                        inst.memory_min_mb, inst.memory_max_mb
                                    ),
                                    tokens.text_muted,
                                );
                                if let Some(wrapper) = inst
                                    .wrapper_cmd
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                {
                                    version_badge(ui, wrapper, tokens.text_muted);
                                }
                            }

                            let pid_label = match pid_opt {
                                Some(pid) => format!("PID: {pid}"),
                                None => "Запуск процесса".to_string(),
                            };
                            version_badge(ui, &pid_label, tokens.text_muted);
                        });
                    });
            }
        },
    );

    action
}

fn tuning_choice_chip(
    ui: &mut Ui,
    tokens: &Tokens,
    style: &ThemeStyleParams,
    selected: bool,
    label: &str,
) -> bool {
    let chip_r = style.control_corner_radius(28.0, RADIUS_TAG);
    let sel_fill = blend_color(
        tokens.bg_surface,
        tokens.accent,
        if tokens.is_light() { 0.16 } else { 0.22 },
    );
    let font_id = FontId::proportional(12.5);
    let text_w = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font_id.clone(), tokens.text)
        .size()
        .x;
    let desired = Vec2::new(text_w + 20.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let anim = crate::animation::animate_interaction(
        ui.ctx(),
        resp.id.with("perf_chip_anim"),
        resp.hovered(),
        resp.is_pointer_button_down_on(),
    );
    let sel_t = crate::animation::animate_spring_bool(
        ui.ctx(),
        resp.id.with("perf_chip_sel"),
        selected,
        crate::animation::SpringConfig::snappy(),
    );
    let visual_rect = anim.transform_rect(rect);
    let fill = lerp_rgba(tokens.bg_input, sel_fill, sel_t);
    let stroke = lerp_rgba(tokens.border, tokens.accent, sel_t.max(anim.press_t * 0.5));
    let fg = lerp_rgba(tokens.text_muted, tokens.text, sel_t.max(anim.hover_t));

    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let galley = p.layout_no_wrap(label.to_owned(), font_id, fg);
        if style.refraction_border {
            if selected || sel_t > 0.35 {
                paint_tinted_glass_pill(
                    ui.ctx(),
                    p,
                    visual_rect,
                    chip_r,
                    style,
                    tokens,
                    fill,
                    (0.65 + 0.25 * anim.hover_t).min(1.0),
                );
            } else {
                paint_glass_surface_overlay(
                    ui.ctx(),
                    p,
                    visual_rect,
                    chip_r,
                    style,
                    tokens,
                    0.20 + 0.52 * anim.hover_t,
                    true,
                );
            }
        } else {
            let hov_fill = lerp_rgba(fill, tokens.bg_surface_hover, anim.hover_t * 0.5);
            p.rect_filled(visual_rect, chip_r, hov_fill);
            p.rect_stroke(
                visual_rect,
                chip_r,
                Stroke::new(1.0_f32, stroke),
                StrokeKind::Inside,
            );
        }
        p.galley(
            Pos2::new(
                visual_rect.center().x - galley.size().x * 0.5,
                visual_rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            fg,
        );
    }

    register_widget(
        ui,
        &resp,
        ElementMeta::new(ElementKind::Chip)
            .label(label)
            .text(label)
            .active(selected)
            .fg(fg)
            .bg(fill),
    );

    resp.clicked()
}

fn save_perf_config(state: &mut UiState, show_toast: bool) {
    match state.config.save(&state.paths) {
        Ok(()) => {
            if show_toast {
                state.status_message = Some((
                    "Параметры производительности сохранены в config.toml".to_string(),
                    false,
                ));
            }
        }
        Err(err) => {
            state.status_message = Some((format!("Не удалось сохранить параметры: {err}"), true));
        }
    }
}

fn render_tuning_section(
    ui: &mut Ui,
    state: &mut UiState,
    tokens: &Tokens,
    style: &ThemeStyleParams,
) {
    let cr = style.card_corner_radius(state.config.theme.corner_radius);
    let card_frame = solid_card_frame_with_config(tokens, &state.config.theme)
        .inner_margin(egui::Margin::same(14));

    let logical_cores = resolve_core_count(state);
    let max_threads = logical_cores.clamp(32, 256);
    let mut auto_save = false;
    let mut manual_save = false;

    show_themed_card(
        ui,
        card_frame,
        cr,
        ui.make_persistent_id("perf_tuning_card"),
        style,
        tokens,
        |ui| {
            ui.set_width(ui.available_width());

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                paint_vector_icon(ui.painter(), r, VectorIcon::Settings, tokens.accent);
                ui.label(
                    RichText::new("Тюнинг производительности и загрузок")
                        .color(tokens.text)
                        .strong()
                        .size(14.0),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if primary_emerald_button(ui, Some(VectorIcon::Check), "Сохранить").clicked()
                    {
                        manual_save = true;
                    }
                });
            });

            ui.add_space(10.0);

            // 1. Worker threads (state.config.threads)
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                ui.label(RichText::new("Рабочие потоки обработки:").color(tokens.text));
                if tuning_choice_chip(
                    ui,
                    tokens,
                    style,
                    state.config.threads.is_none(),
                    &format!("Авто ({logical_cores})"),
                ) {
                    state.config.threads = None;
                    auto_save = true;
                }
                if tuning_choice_chip(
                    ui,
                    tokens,
                    style,
                    state.config.threads.is_some(),
                    "Фиксированно",
                ) && state.config.threads.is_none()
                {
                    state.config.threads = Some(logical_cores.clamp(1, max_threads));
                    auto_save = true;
                }
                if let Some(ref mut thread_count) = state.config.threads {
                    let mut clamped = (*thread_count).clamp(1, max_threads);
                    ui.scope(|ui| {
                        ui.spacing_mut().slider_width = 180.0;
                        let r = ui
                            .add(egui::Slider::new(&mut clamped, 1..=max_threads).suffix(" пот."));
                        if r.changed() {
                            *thread_count = clamped;
                        }
                        if r.drag_stopped() {
                            auto_save = true;
                        }
                    });
                }
            });

            ui.add_space(8.0);

            // 2. Parallel HTTP download connections (state.config.max_connections)
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                ui.label(RichText::new("Параллельные потоки загрузки:").color(tokens.text));
                ui.scope(|ui| {
                    ui.spacing_mut().slider_width = 190.0;
                    let r = ui.add(
                        egui::Slider::new(&mut state.config.max_connections, 4..=128)
                            .suffix(" соед."),
                    );
                    if r.drag_stopped() {
                        auto_save = true;
                    }
                });
                for (val, label) in [
                    (8usize, "8 (Эконом)"),
                    (32usize, "32 (Стандарт)"),
                    (64usize, "64 (Быстро)"),
                ] {
                    if tuning_choice_chip(
                        ui,
                        tokens,
                        style,
                        state.config.max_connections == val,
                        label,
                    ) {
                        state.config.max_connections = val;
                        auto_save = true;
                    }
                }
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // 3. Default JVM Memory (state.config.default_memory_min_mb / max_mb)
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(12.0, 6.0);
                ui.label(RichText::new("Память JVM по умолчанию:").color(tokens.text));

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Минимум (-Xms):").color(tokens.text_muted));
                    ui.scope(|ui| {
                        ui.spacing_mut().slider_width = 150.0;
                        let r = ui.add(
                            egui::Slider::new(&mut state.config.default_memory_min_mb, 512..=8192)
                                .step_by(256.0)
                                .suffix(" МБ"),
                        );
                        if r.changed()
                            && state.config.default_memory_min_mb
                                > state.config.default_memory_max_mb
                        {
                            state.config.default_memory_max_mb = state.config.default_memory_min_mb;
                        }
                        if r.drag_stopped() {
                            auto_save = true;
                        }
                    });
                });

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Максимум (-Xmx):").color(tokens.text_muted));
                    ui.scope(|ui| {
                        ui.spacing_mut().slider_width = 160.0;
                        let r = ui.add(
                            egui::Slider::new(
                                &mut state.config.default_memory_max_mb,
                                1024..=32768,
                            )
                            .step_by(512.0)
                            .suffix(" МБ"),
                        );
                        if r.changed()
                            && state.config.default_memory_max_mb
                                < state.config.default_memory_min_mb
                        {
                            state.config.default_memory_min_mb =
                                state.config.default_memory_max_mb.min(8192);
                        }
                        if r.drag_stopped() {
                            auto_save = true;
                        }
                    });
                });
            });

            ui.add_space(8.0);

            // 4. Default wrapper command (state.config.default_wrapper)
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                ui.label(RichText::new("Системная обёртка запуска:").color(tokens.text));
                if tuning_choice_chip(
                    ui,
                    tokens,
                    style,
                    state.config.default_wrapper.is_none(),
                    "Без обёртки",
                ) {
                    state.config.default_wrapper = None;
                    auto_save = true;
                }
                for cmd in ["gamemoderun", "mangohud", "prime-run"] {
                    let active = state.config.default_wrapper.as_deref() == Some(cmd);
                    if tuning_choice_chip(ui, tokens, style, active, cmd) {
                        state.config.default_wrapper = Some(cmd.to_string());
                        auto_save = true;
                    }
                }
            });

            ui.add_space(8.0);

            // 5. Graphics backend (state.config.ui_backend)
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                ui.label(RichText::new("Графический бэкенд:").color(tokens.text));
                for (backend, label) in [
                    (GuiBackend::Auto, "Авто (Vulkan -> OpenGL)"),
                    (GuiBackend::Vulkan, "Vulkan (WGPU)"),
                    (GuiBackend::Gl, "OpenGL (Glow)"),
                ] {
                    if tuning_choice_chip(
                        ui,
                        tokens,
                        style,
                        state.config.ui_backend == backend,
                        label,
                    ) {
                        state.config.ui_backend = backend;
                        auto_save = true;
                    }
                }
            });

            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Изменения памяти и обёртки применяются к новым запускам; число потоков и графический бэкенд вступают в силу при следующем старте лаунчера.",
                )
                .color(tokens.text_muted)
                .small(),
            );
        },
    );

    if manual_save {
        save_perf_config(state, true);
    } else if auto_save {
        save_perf_config(state, false);
    }
}

fn render_storage_section(
    ui: &mut Ui,
    state: &mut UiState,
    tokens: &Tokens,
    style: &ThemeStyleParams,
) {
    let cr = style.card_corner_radius(state.config.theme.corner_radius);
    let card_frame = solid_card_frame_with_config(tokens, &state.config.theme)
        .inner_margin(egui::Margin::same(14));

    let mut trigger_scan = false;
    let mut trigger_clean = false;

    show_themed_card(
        ui,
        card_frame,
        cr,
        ui.make_persistent_id("perf_storage_card"),
        style,
        tokens,
        |ui| {
            ui.set_width(ui.available_width());

            let scan_label = if state.is_scanning_storage {
                "Сканирование..."
            } else {
                "Сканировать"
            };
            if ui.available_width() >= 620.0 {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                    paint_vector_icon(ui.painter(), r, VectorIcon::Folder, tokens.accent);
                    ui.label(
                        RichText::new("Диск и кэш")
                            .color(tokens.text)
                            .strong()
                            .size(14.0),
                    );
                    if let Some(ref stats) = state.storage_stats {
                        version_badge(
                            ui,
                            &format!("Всего: {}", format_bytes_ru(stats.total_bytes)),
                            ACCENT_COBALT,
                        );
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if secondary_glass_button(
                            ui,
                            Some(VectorIcon::Trash),
                            "Очистить временный кэш",
                        )
                        .clicked()
                        {
                            trigger_clean = true;
                        }
                        ui.add_enabled_ui(!state.is_scanning_storage, |ui| {
                            if secondary_glass_button(ui, Some(VectorIcon::Refresh), scan_label)
                                .clicked()
                            {
                                trigger_scan = true;
                            }
                        });
                    });
                });
            } else {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                    paint_vector_icon(ui.painter(), r, VectorIcon::Folder, tokens.accent);
                    ui.label(
                        RichText::new("Диск и кэш")
                            .color(tokens.text)
                            .strong()
                            .size(14.0),
                    );
                    if let Some(ref stats) = state.storage_stats {
                        version_badge(
                            ui,
                            &format!("Всего: {}", format_bytes_ru(stats.total_bytes)),
                            ACCENT_COBALT,
                        );
                    }
                });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                    ui.add_enabled_ui(!state.is_scanning_storage, |ui| {
                        if secondary_glass_button(ui, Some(VectorIcon::Refresh), scan_label)
                            .clicked()
                        {
                            trigger_scan = true;
                        }
                    });
                    if secondary_glass_button(ui, Some(VectorIcon::Trash), "Очистить временный кэш")
                        .clicked()
                    {
                        trigger_clean = true;
                    }
                });
            }

            ui.add_space(8.0);

            match &state.storage_stats {
                Some(stats) => {
                    render_storage_breakdown(ui, stats, state.is_scanning_storage, tokens);
                }
                None => {
                    let msg = if state.is_scanning_storage {
                        "Идёт подсчёт размеров директорий инстансов, библиотек, Java и кэша..."
                    } else {
                        "Нажмите «Сканировать», чтобы проанализировать использование диска по категориям."
                    };
                    ui.label(RichText::new(msg).color(tokens.text_muted).small());
                }
            }
        },
    );

    if trigger_scan && !state.is_scanning_storage {
        state.is_scanning_storage = true;
        let _ = state.cmd_sender.send(UiWorkerCommand::ScanStorageStats);
    }
    if trigger_clean {
        let _ = state.cmd_sender.send(UiWorkerCommand::CleanTempCache);
    }
}

fn render_storage_breakdown(ui: &mut Ui, stats: &StorageStats, is_scanning: bool, tokens: &Tokens) {
    let bucket_sum = stats
        .instances_bytes
        .saturating_add(stats.assets_libs_bytes)
        .saturating_add(stats.cas_bytes)
        .saturating_add(stats.java_bytes)
        .saturating_add(stats.temp_logs_bytes);
    let denom = stats.total_bytes.max(bucket_sum);

    let cas_col = Color32::from_rgb(0x8B, 0x86, 0xC6);
    let segments = [
        (
            "Инстансы и миры",
            stats.instances_bytes,
            tokens.accent,
            String::new(),
        ),
        (
            "Ассеты и библиотеки",
            stats.assets_libs_bytes,
            ACCENT_COBALT,
            String::new(),
        ),
        (
            "Хранилище артефактов",
            stats.cas_bytes,
            cas_col,
            format!("{} файлов", stats.cas_files),
        ),
        (
            "Рантаймы Java",
            stats.java_bytes,
            tokens.warning,
            String::new(),
        ),
        (
            "Временный кэш и логи",
            stats.temp_logs_bytes,
            tokens.text_muted,
            String::new(),
        ),
    ];

    // Stacked distribution bar
    let bar_w = ui.available_width().max(40.0);
    let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(bar_w, 10.0), Sense::hover());
    if ui.is_rect_visible(bar_rect) && bar_rect.is_positive() {
        let painter = ui.painter();
        let cr = CornerRadius::same(4);
        painter.rect_filled(bar_rect, cr, tokens.bg_input);
        painter.rect_stroke(
            bar_rect,
            cr,
            Stroke::new(1.0_f32, tokens.border),
            StrokeKind::Inside,
        );

        let mut cursor_x = bar_rect.left();
        for (_, bytes, col, _) in &segments {
            if *bytes == 0 || denom == 0 {
                continue;
            }
            let frac = storage_share_fraction(*bytes, denom);
            let seg_w = (bar_rect.width() * frac).max(2.0);
            let max_w = (bar_rect.right() - cursor_x).max(0.0);
            let w = seg_w.min(max_w);
            if w > 0.5 {
                let seg_rect =
                    Rect::from_min_size(Pos2::new(cursor_x, bar_rect.top()), Vec2::new(w, 10.0));
                painter.rect_filled(seg_rect, CornerRadius::same(2), *col);
                cursor_x += w;
            }
        }
    }

    ui.add_space(10.0);

    for (label, bytes, col, detail) in segments {
        let frac = storage_share_fraction(bytes, denom);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;

            let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            if ui.is_rect_visible(dot_rect) {
                ui.painter().circle_filled(dot_rect.center(), 4.0, col);
            }

            ui.label(RichText::new(label).color(tokens.text));
            if !detail.is_empty() {
                ui.label(
                    RichText::new(format!("({detail})"))
                        .color(tokens.text_muted)
                        .small(),
                );
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.add_sized(
                    [54.0, 18.0],
                    egui::Label::new(
                        RichText::new(format!("{:.1}%", frac * 100.0))
                            .font(FontId::new(11.5, FontFamily::Monospace))
                            .color(tokens.text_muted),
                    ),
                );
                ui.add_sized(
                    [82.0, 18.0],
                    egui::Label::new(
                        RichText::new(format_bytes_ru(bytes))
                            .font(FontId::new(12.0, FontFamily::Monospace))
                            .color(tokens.text)
                            .strong(),
                    ),
                );
            });
        });
        ui.add_space(3.0);
    }

    ui.add_space(4.0);
    let mut meta_parts = Vec::new();
    if !stats.scanned_at.trim().is_empty() {
        meta_parts.push(format!(
            "Последнее сканирование: {}",
            stats.scanned_at.trim()
        ));
    }
    if is_scanning {
        meta_parts.push("Обновление статистики...".to_string());
    }
    if !meta_parts.is_empty() {
        ui.label(
            RichText::new(meta_parts.join(" · "))
                .color(tokens.text_muted)
                .small(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mq_core::{AppConfig, CoreCpuLoad, CpuSnapshot, ModLoaderKind, MqPaths, ThemePreset};
    use mq_sched::ComputePool;
    use std::sync::Arc;

    fn contains_emoji(text: &str) -> bool {
        text.chars().any(|ch| {
            let cp = ch as u32;
            (0x1F300..=0x1FAFF).contains(&cp)
                || (0x2600..=0x27BF).contains(&cp)
                || (0xFE00..=0xFE0F).contains(&cp)
        })
    }

    #[test]
    fn zero_emojis_in_perf_view_source() {
        assert!(!contains_emoji(include_str!("perf_view.rs")));
    }

    #[test]
    fn bytes_and_mib_formatting_edges() {
        assert_eq!(format_bytes_ru(0), "0 Б");
        assert_eq!(format_bytes_ru(512), "512 Б");
        assert_eq!(format_bytes_ru(2048), "2 КБ");
        assert_eq!(format_bytes_ru(5 * 1024 * 1024), "5.0 МБ");
        assert_eq!(format_bytes_ru(2 * 1024 * 1024 * 1024), "2.00 ГБ");

        assert_eq!(format_mib_ru(0.0), "0.0 МБ");
        assert_eq!(format_mib_ru(-42.5), "0.0 МБ");
        assert_eq!(format_mib_ru(f64::NAN), "0.0 МБ");
        assert_eq!(format_mib_ru(f64::INFINITY), "0.0 МБ");
        assert_eq!(format_mib_ru(f64::NEG_INFINITY), "0.0 МБ");
        assert_eq!(format_mib_ru(256.4), "256.4 МБ");
        assert_eq!(format_mib_ru(2048.0), "2.00 ГБ");
    }

    #[test]
    fn frame_time_and_fps_edge_cases() {
        assert_eq!(compute_approx_fps(0.0), None);
        assert_eq!(compute_approx_fps(-1.0), None);
        assert_eq!(compute_approx_fps(f32::NAN), None);
        assert_eq!(compute_approx_fps(f32::INFINITY), None);
        assert_eq!(compute_approx_fps(f32::NEG_INFINITY), None);
        assert_eq!(compute_approx_fps(16.6667), Some(60));
        assert_eq!(compute_approx_fps(8.0), Some(125));
        assert_eq!(compute_approx_fps(0.01), Some(9999));

        assert_eq!(format_fps_label(0.0), "— FPS");
        assert_eq!(format_fps_label(f32::NAN), "— FPS");
        assert_eq!(format_fps_label(16.6667), "60 FPS");

        assert_eq!(format_frame_ms(0.0), "—");
        assert_eq!(format_frame_ms(-3.0), "—");
        assert_eq!(format_frame_ms(f32::NAN), "—");
        assert_eq!(format_frame_ms(f32::INFINITY), "—");
        assert_eq!(format_frame_ms(2.345), "2.35 мс");
        assert_eq!(format_frame_ms(16.6), "16.6 мс");
        assert_eq!(format_frame_ms(125.0), "125 мс");
    }

    #[test]
    fn series_summary_handles_empty_single_and_non_finite() {
        let empty_f32 = VecDeque::new();
        let s_empty = summarize_f32_series(&empty_f32);
        assert_eq!(s_empty.count, 0);
        assert!(s_empty.values.is_empty());
        assert!(s_empty.current.abs() < 1e-6);
        assert!(s_empty.avg.abs() < 1e-6);
        assert!(s_empty.max.abs() < 1e-6);

        let empty_f64 = VecDeque::new();
        let s_empty_64 = summarize_f64_series(&empty_f64);
        assert_eq!(s_empty_64.count, 0);
        assert!(s_empty_64.values.is_empty());

        let single = VecDeque::from([42.5_f32]);
        let s_single = summarize_f32_series(&single);
        assert_eq!(s_single.count, 1);
        assert!((s_single.current - 42.5).abs() < 1e-4);
        assert!((s_single.avg - 42.5).abs() < 1e-4);
        assert!((s_single.min - 42.5).abs() < 1e-4);
        assert!((s_single.max - 42.5).abs() < 1e-4);

        let dirty = VecDeque::from([f32::NAN, -5.0, 20.0, f32::INFINITY, 40.0, f32::NEG_INFINITY]);
        let s_dirty = summarize_f32_series(&dirty);
        assert_eq!(s_dirty.count, 6);
        assert!(s_dirty.values.iter().all(|v| v.is_finite() && *v >= 0.0));
        assert!((s_dirty.avg - 10.0).abs() < 1e-4);
        assert!((s_dirty.max - 40.0).abs() < 1e-4);
        assert!(s_dirty.min.abs() < 1e-6);
        assert!(s_dirty.current.abs() < 1e-6);

        let dirty_f64 = VecDeque::from([f64::NAN, -10.0, 128.0, f64::INFINITY]);
        let s_dirty_64 = summarize_f64_series(&dirty_f64);
        assert_eq!(s_dirty_64.count, 4);
        assert!((s_dirty_64.avg - 32.0).abs() < 1e-4);
        assert!((s_dirty_64.max - 128.0).abs() < 1e-4);
    }

    #[test]
    fn chart_normalization_and_storage_share_edges() {
        let rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(110.0, 120.0));
        assert!(normalize_chart_points(&[], rect, 0.0, 100.0).is_empty());

        let zero_rect = Rect::from_min_size(Pos2::ZERO, Vec2::ZERO);
        assert!(normalize_chart_points(&[10.0, 20.0], zero_rect, 0.0, 100.0).is_empty());

        let single_pt = normalize_chart_points(&[50.0], rect, 0.0, 100.0);
        assert_eq!(single_pt.len(), 1);
        assert!((single_pt[0].x - 110.0).abs() < 1e-3);
        assert!((single_pt[0].y - 70.0).abs() < 1e-3);

        let multi =
            normalize_chart_points(&[0.0, f32::NAN, 100.0, f32::INFINITY], rect, 0.0, 100.0);
        assert_eq!(multi.len(), 4);
        for p in &multi {
            assert!(p.x.is_finite() && p.y.is_finite());
            assert!(p.x >= rect.left() && p.x <= rect.right());
            assert!(p.y >= rect.top() && p.y <= rect.bottom());
        }
        assert!((multi[0].y - rect.bottom()).abs() < 1e-3);
        assert!((multi[2].y - rect.top()).abs() < 1e-3);

        // Inverted / zero span y bounds never produce NaN
        let bad_bounds = normalize_chart_points(&[10.0, 20.0], rect, 100.0, 100.0);
        assert_eq!(bad_bounds.len(), 2);
        assert!(bad_bounds
            .iter()
            .all(|p| p.x.is_finite() && p.y.is_finite()));

        assert!(storage_share_fraction(100, 0).abs() < 1e-6);
        assert!((storage_share_fraction(200, 100) - 1.0).abs() < 1e-6);
        assert!((storage_share_fraction(25, 100) - 0.25).abs() < 1e-6);

        assert_eq!(per_core_column_count(0.0, 16), 1);
        assert_eq!(per_core_column_count(f32::NAN, 8), 1);
        assert_eq!(per_core_column_count(900.0, 2), 2);
        assert_eq!(per_core_column_count(900.0, 16), 4);

        assert!((nice_chart_ceiling(0.0, 22.0) - 22.0).abs() < 1e-4);
        assert!((nice_chart_ceiling(f32::NAN, 64.0) - 64.0).abs() < 1e-4);
        assert!((nice_chart_ceiling(-10.0, 22.0) - 22.0).abs() < 1e-4);
        assert!((nice_chart_ceiling(100.0, 64.0) - 115.0).abs() < 1e-3);
    }

    #[test]
    fn perf_view_renders_empty_and_populated_states_without_panicking() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = MqPaths::with_root(tmp.path().to_path_buf());
        let pool = Arc::new(ComputePool::new(2));
        let mut state = UiState::new(paths, AppConfig::default(), pool).expect("ui state");

        let ctx = egui::Context::default();
        let mut view = PerfView;

        // 1. Render completely empty initial state (0.0 frame ms, empty histories, no storage stats)
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert_eq!(view.ui(ui, &mut state), None);
            });
        });

        // 2. Render zero-bytes StorageStats edge case
        state.storage_stats = Some(StorageStats::default());
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert_eq!(view.ui(ui, &mut state), None);
            });
        });

        // 3. Render populated state including non-finite samples, 16 cores, running game, storage stats, LiquidGlass
        state.config.theme.preset = ThemePreset::LiquidGlass;
        crate::theme::apply_theme(&ctx, &state.config.theme);
        state.gpu_backend_label = "wgpu (Vulkan)".to_string();
        state.last_frame_ms = 4.25;
        state.max_frame_ms = 18.4;
        state.peak_rss_mib = 142.8;
        state.open_fds = 37;
        state.cpu_history = VecDeque::from([12.0, f32::NAN, 35.5, 48.0]);
        state.rss_history = VecDeque::from([98.0, 112.4, f64::INFINITY, 128.0]);
        state.frame_time_ms_history = VecDeque::from([2.1, 4.5, 19.2, 4.25]);
        state.latest_cpu = CpuSnapshot {
            total_pct: 48.0,
            rss_mib: 128.0,
            per_core: (0..16)
                .map(|core_id| CoreCpuLoad {
                    core_id,
                    usage_pct: (core_id as f32 * 6.0).min(100.0),
                })
                .collect(),
        };
        let inst = InstanceManifest::new(
            "Fabric Test",
            "1.21.4",
            ModLoaderKind::Fabric,
            Some("0.16.9".into()),
        );
        let inst_id = inst.id.clone();
        state.instances.push(inst);
        state.running_instances.insert(inst_id.clone(), Some(4242));
        let expected_action = PerfAction::OpenInstanceConsole(inst_id);
        assert_eq!(expected_action, expected_action.clone());

        state.storage_stats = Some(StorageStats {
            total_bytes: 2_500_000_000,
            cas_bytes: 800_000_000,
            cas_files: 1240,
            instances_bytes: 1_100_000_000,
            assets_libs_bytes: 400_000_000,
            java_bytes: 180_000_000,
            temp_logs_bytes: 20_000_000,
            scanned_at: "12:00:00".to_string(),
        });

        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1180.0, 740.0))),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = view.ui(ui, &mut state);
            });
        });
    }
}
