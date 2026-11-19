#![deny(clippy::await_holding_lock)]
//! Dedicated Downloads & Active Installations view (`Загрузки`).
//!
//! Displays live KPI summary, active instance installations/verifications,
//! catalog/modpack job queue, pending manual downloads, real-time speed
//! sparkline, and session history. Strictly zero emojis!

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use egui::epaint::Vertex;
use egui::{
    Align, Color32, CornerRadius, FontId, Id, Layout, Mesh, Pos2, Rect, RichText, ScrollArea,
    Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};
use mq_core::{InstanceId, ModLoaderKind};
use mq_instances::InstanceManifest;
use mq_mods::{CatalogSource, ProjectKind};

use crate::animation::{animate_spring_f32, SpringConfig};
use crate::catalog_flow::{
    default_downloads_dir, format_size, is_safe_http_url, InstallJob, JobStatus, RetrySpec,
};
use crate::catalog_view::{source_color, source_name};
use crate::components::{button_primary, button_secondary, render_badge, BadgeKind, FilterChip};
use crate::install_types::InstallStage;
use crate::state::{
    safe_clamp_f32, safe_progress_clamp, safe_progress_ratio, DownloadHistoryEntry,
    DownloadHistoryStatus, UiState, UiWorkerCommand, MAX_TELEMETRY_SAMPLES,
};
use crate::theme::{blend_color, solid_card_frame_with_config, Tokens, ACCENT_COBALT, RADIUS_TAG};
use crate::theme_styles::{show_themed_card, with_alpha, ThemeStyleParams};
use crate::widgets::{
    danger_button, loader_badge, paint_vector_icon, status_pill, version_badge, VectorIcon,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadsAction {
    OpenCatalog,
    OpenInstance(mq_core::InstanceId),
}

#[derive(Default)]
pub struct DownloadsView;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum HistoryFilter {
    #[default]
    All,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
struct DownloadsSummary {
    active_count: usize,
    active_jobs_count: usize,
    manual_pending_count: usize,
    history_count: usize,
    current_mib_per_sec: f64,
    peak_mib_per_sec: f64,
    files_per_sec: f64,
    bytes_done: u64,
    bytes_total: u64,
    items_done: u64,
    items_total: u64,
    overall_progress: f32,
}

#[derive(Debug, Clone)]
struct SpeedChartStats {
    samples: Vec<f32>,
    current_mib: f32,
    peak_mib: f32,
    avg_mib: f32,
}

impl DownloadsView {
    pub fn ui(&mut self, ui: &mut Ui, state: &mut UiState) -> Option<DownloadsAction> {
        let mut action: Option<DownloadsAction> = None;
        let tokens = Tokens::from_config(&state.config.theme);
        let style = ThemeStyleParams::active(ui.ctx());
        let cr = style.card_corner_radius(state.config.theme.corner_radius);

        let summary = compute_downloads_summary(state);
        let speed_stats = compute_speed_stats(&state.download_speed_history);

        if state.jobs.active_count() > 0 || state.installing_instance.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        self.drive_manual_polling(ui.ctx(), state);

        // 1. Header row
        self.render_header(ui, state, &tokens, &summary, &mut action);
        ui.add_space(10.0);

        let has_instance_op = state.installing_instance.is_some()
            || state.installing_operation.is_some()
            || (state.active_phase.is_some() && state.jobs.active_count() == 0);
        let has_install_error = state.installing_instance.is_none()
            && (state.detailed_install_error.is_some() || state.install_error.is_some());
        let has_manual = !state.manual.pending.is_empty()
            || !state.manual.completed.is_empty()
            || state.manual.message.is_some();
        let has_jobs = !state.jobs.jobs.is_empty();
        let has_history = !state.download_history.is_empty();
        let has_speed_chart = summary.active_count > 0 || !speed_stats.samples.is_empty();
        let is_completely_idle_and_empty = summary.active_count == 0
            && !has_instance_op
            && !has_install_error
            && !has_jobs
            && !has_manual
            && !has_history;

        ScrollArea::vertical()
            .id_salt("mq_downloads_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.render_kpi_bar(ui, state, &tokens, &style, cr, &summary);
                ui.add_space(12.0);

                if has_instance_op {
                    self.render_active_instance_op(ui, state, &tokens, &style, cr, &mut action);
                    ui.add_space(10.0);
                }

                if has_install_error {
                    self.render_install_error_banner(ui, state, &tokens, &style, cr, &mut action);
                    ui.add_space(10.0);
                }

                if has_jobs {
                    self.render_jobs_section(ui, state, &tokens, &style, cr, &mut action);
                    ui.add_space(10.0);
                }

                if has_manual {
                    self.render_manual_section(ui, state, &tokens, &style, cr);
                    ui.add_space(10.0);
                }

                if has_speed_chart {
                    self.render_speed_chart(
                        ui,
                        state,
                        &tokens,
                        &style,
                        cr,
                        &speed_stats,
                        summary.current_mib_per_sec,
                    );
                    ui.add_space(10.0);
                }

                if has_history {
                    self.render_history_section(ui, state, &tokens, &style, cr, &mut action);
                    ui.add_space(10.0);
                }

                if is_completely_idle_and_empty {
                    self.render_empty_state_card(ui, state, &tokens, &style, cr, &mut action);
                }
            });

        action
    }

    fn drive_manual_polling(&self, ctx: &egui::Context, state: &mut UiState) {
        if state.manual.pending.is_empty() {
            return;
        }
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        let due = state
            .manual
            .last_poll
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(2));
        if due || !dropped.is_empty() {
            state.manual.last_poll = Some(Instant::now());
            let _ = state
                .cmd_sender
                .send(UiWorkerCommand::PollManual { dropped });
        }
        ctx.request_repaint_after(Duration::from_secs(2));
    }

    fn render_header(
        &self,
        ui: &mut Ui,
        state: &mut UiState,
        tokens: &Tokens,
        summary: &DownloadsSummary,
        action: &mut Option<DownloadsAction>,
    ) {
        let render_title_group = |ui: &mut Ui| {
            let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
            paint_vector_icon(ui.painter(), icon_rect, VectorIcon::Download, tokens.accent);

            ui.heading(
                RichText::new("Загрузки")
                    .color(tokens.text)
                    .size(20.0)
                    .strong(),
            );

            if summary.active_count > 0 {
                status_pill(
                    ui,
                    &format!("Активно: {}", summary.active_count),
                    ACCENT_COBALT,
                );
            } else {
                render_badge(ui, tokens, "Простой", BadgeKind::Neutral);
            }

            if summary.manual_pending_count > 0 {
                render_badge(
                    ui,
                    tokens,
                    &format!("Вручную: {}", summary.manual_pending_count),
                    BadgeKind::Warning,
                );
            }
        };

        let draw_catalog = |ui: &mut Ui, action: &mut Option<DownloadsAction>| {
            if button_primary(ui, tokens, Some(VectorIcon::Catalog), "Открыть каталог").clicked()
            {
                *action = Some(DownloadsAction::OpenCatalog);
            }
        };
        let draw_clear = |ui: &mut Ui, state: &mut UiState| {
            ui.add_enabled_ui(!state.download_history.is_empty(), |ui| {
                if button_secondary(ui, tokens, Some(VectorIcon::Trash), "Очистить историю")
                    .on_hover_text("Удалить записи истории загрузок за текущую сессию")
                    .clicked()
                {
                    state.download_history.clear();
                    state.status_message = Some(("История загрузок очищена".to_string(), false));
                }
            });
        };

        if ui.available_width() >= 600.0 {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                render_title_group(ui);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                    draw_catalog(ui, action);
                    draw_clear(ui, state);
                });
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                render_title_group(ui);
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                draw_clear(ui, state);
                draw_catalog(ui, action);
            });
        }
    }

    fn render_kpi_bar(
        &self,
        ui: &mut Ui,
        state: &UiState,
        tokens: &Tokens,
        style: &ThemeStyleParams,
        cr: CornerRadius,
        summary: &DownloadsSummary,
    ) {
        let avail_w = ui.available_width();
        let gap = 8.0;
        let single_row = avail_w >= 640.0;
        let card_w = if single_row {
            ((avail_w - gap * 2.0 - 2.0) / 3.0).max(180.0)
        } else {
            avail_w
        };

        let phase_sub = if let Some(label) = state.active_hud_phase_label() {
            label
        } else if summary.active_jobs_count > 0 {
            format!("Задач в очереди: {}", summary.active_jobs_count)
        } else if summary.manual_pending_count > 0 {
            format!("Ожидают скачивания: {}", summary.manual_pending_count)
        } else {
            "Очередь свободна".to_string()
        };

        let speed_val = format_speed_mib(summary.current_mib_per_sec);
        let speed_sub = if summary.peak_mib_per_sec > 0.01 {
            format!("Пик: {}", format_speed_mib(summary.peak_mib_per_sec))
        } else if summary.files_per_sec > 0.1 {
            format!("{:.0} файл./с", summary.files_per_sec)
        } else {
            "Поток неактивен".to_string()
        };

        let progress_val = if summary.bytes_total > 0 || summary.bytes_done > 0 {
            format_bytes_ratio(summary.bytes_done, summary.bytes_total)
        } else if summary.items_total > 0 || summary.items_done > 0 {
            format_files_ratio(summary.items_done, summary.items_total)
        } else {
            "—".to_string()
        };

        let progress_sub = if summary.items_total > 0 {
            format!(
                "Файлы: {} · {:.0}%",
                format_files_ratio(summary.items_done, summary.items_total),
                summary.overall_progress * 100.0
            )
        } else if summary.active_count > 0 {
            format!("Выполнено: {:.0}%", summary.overall_progress * 100.0)
        } else {
            format!("Завершено в сессии: {}", summary.history_count)
        };

        let draw_kpi = |ui: &mut Ui,
                        id_salt: &str,
                        icon: VectorIcon,
                        icon_col: Color32,
                        title: &str,
                        value: &str,
                        sub: &str,
                        width: f32| {
            let frame = solid_card_frame_with_config(tokens, &state.config.theme)
                .inner_margin(egui::Margin::symmetric(12, 10));
            show_themed_card(
                ui,
                frame,
                cr,
                ui.make_persistent_id(id_salt),
                style,
                tokens,
                |ui| {
                    ui.set_width((width - 24.0).max(120.0));
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        paint_vector_icon(ui.painter(), r, icon, icon_col);
                        ui.label(RichText::new(title).color(tokens.text_muted).small());
                    });
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(value)
                                .color(tokens.text)
                                .font(FontId::monospace(16.5))
                                .strong(),
                        )
                        .truncate(),
                    );
                    ui.add_space(2.0);
                    ui.add(
                        egui::Label::new(RichText::new(sub).color(tokens.text_muted).small())
                            .truncate(),
                    );
                },
            );
        };

        let active_str = summary.active_count.to_string();
        let active_col = if summary.active_count > 0 {
            ACCENT_COBALT
        } else {
            tokens.text_muted
        };
        let speed_col = if summary.current_mib_per_sec > 0.01 {
            tokens.accent
        } else {
            tokens.text_muted
        };
        let kpis = [
            (
                "mq_dl_kpi_active",
                VectorIcon::Activity,
                active_col,
                "Активные операции",
                active_str.as_str(),
                phase_sub.as_str(),
            ),
            (
                "mq_dl_kpi_speed",
                VectorIcon::Download,
                speed_col,
                "Текущая скорость",
                speed_val.as_str(),
                speed_sub.as_str(),
            ),
            (
                "mq_dl_kpi_progress",
                VectorIcon::Layers,
                tokens.accent,
                "Прогресс загрузки",
                progress_val.as_str(),
                progress_sub.as_str(),
            ),
        ];

        if single_row {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for &(id_salt, icon, icon_col, title, value, sub) in &kpis {
                    draw_kpi(ui, id_salt, icon, icon_col, title, value, sub, card_w);
                }
            });
        } else {
            for (idx, &(id_salt, icon, icon_col, title, value, sub)) in kpis.iter().enumerate() {
                if idx > 0 {
                    ui.add_space(6.0);
                }
                draw_kpi(ui, id_salt, icon, icon_col, title, value, sub, card_w);
            }
        }
    }

    fn render_active_instance_op(
        &self,
        ui: &mut Ui,
        state: &mut UiState,
        tokens: &Tokens,
        style: &ThemeStyleParams,
        cr: CornerRadius,
        action: &mut Option<DownloadsAction>,
    ) {
        let mut cancel_clicked = false;
        let mut inspect_id: Option<InstanceId> = None;

        let target_raw = state
            .installing_instance
            .as_ref()
            .map(|id| id.as_str().to_string())
            .or_else(|| state.installing_operation.clone());

        let (resolved_id, display_name, meta) = match target_raw.as_deref() {
            Some(raw) => resolve_instance_display(&state.instances, raw),
            None => (None, "Активная операция".to_string(), None),
        };
        let target_inst_id = state.installing_instance.clone().or(resolved_id);
        let op_label = state.installing_operation.clone();
        let phase_label = state.active_hud_phase_label().unwrap_or_else(|| {
            state
                .current_install_stage
                .full_localized_name()
                .to_string()
        });

        let (frac, speed_mib, bytes_done, bytes_total, items_done, items_total) =
            if let Some(prog) = &state.phase_progress {
                let f = if prog.total_items > 0 {
                    safe_progress_ratio(prog.completed_items, prog.total_items)
                } else {
                    safe_progress_ratio(prog.bytes_done, prog.bytes_total)
                };
                (
                    f,
                    sanitize_nonneg_f64(prog.mib_per_sec),
                    prog.bytes_done,
                    prog.bytes_total,
                    prog.completed_items,
                    prog.total_items,
                )
            } else {
                (0.0, 0.0, 0, 0, 0, 0)
            };

        let frame = solid_card_frame_with_config(tokens, &state.config.theme)
            .stroke(Stroke::new(
                1.0_f32,
                blend_color(tokens.border_strong, ACCENT_COBALT, 0.45),
            ))
            .inner_margin(egui::Margin::same(14));

        show_themed_card(
            ui,
            frame,
            cr,
            ui.make_persistent_id("mq_dl_active_instance_card"),
            style,
            tokens,
            |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                    let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                    paint_vector_icon(ui.painter(), icon_rect, VectorIcon::Download, ACCENT_COBALT);

                    ui.label(
                        RichText::new(&display_name)
                            .color(tokens.text)
                            .size(15.0)
                            .strong(),
                    );

                    if let Some((ref mc_ver, loader)) = meta {
                        version_badge(ui, mc_ver, ACCENT_COBALT);
                        loader_badge(ui, loader);
                    }

                    if let Some(ref op) = op_label {
                        if op != &display_name && op != &phase_label {
                            render_badge(ui, tokens, op, BadgeKind::Accent);
                        }
                    }

                    status_pill(ui, &phase_label, ACCENT_COBALT);
                });

                if state.installing_instance.is_some() {
                    ui.add_space(10.0);
                    render_stage_stepper(ui, tokens, state.current_install_stage);
                }

                ui.add_space(10.0);
                paint_animated_progress_bar(
                    ui,
                    Id::new("mq_dl_active_inst_prog"),
                    frac,
                    8.0,
                    tokens,
                    ACCENT_COBALT,
                );

                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                    render_badge(
                        ui,
                        tokens,
                        &format!("Прогресс: {:.1}%", safe_progress_clamp(frac) * 100.0),
                        BadgeKind::Accent,
                    );
                    if items_total > 0 || items_done > 0 {
                        render_badge(
                            ui,
                            tokens,
                            &format!("Файлы: {}", format_files_ratio(items_done, items_total)),
                            BadgeKind::Neutral,
                        );
                    }
                    if bytes_total > 0 || bytes_done > 0 {
                        render_badge(
                            ui,
                            tokens,
                            &format!("Данные: {}", format_bytes_ratio(bytes_done, bytes_total)),
                            BadgeKind::Neutral,
                        );
                    }
                    render_badge(
                        ui,
                        tokens,
                        &format!("Скорость: {}", format_speed_mib(speed_mib)),
                        BadgeKind::Neutral,
                    );
                    if let Some(eta) = state.install_eta_secs {
                        render_badge(
                            ui,
                            tokens,
                            &format!("Осталось: {}", format_eta_secs(Some(eta))),
                            BadgeKind::Neutral,
                        );
                    }
                    if state.install_retries_count > 0 {
                        render_badge(
                            ui,
                            tokens,
                            &format!("Повторов: {}", state.install_retries_count),
                            BadgeKind::Warning,
                        );
                    }
                });

                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                    if danger_button(ui, Some(VectorIcon::Cross), "Отменить").clicked() {
                        cancel_clicked = true;
                    }
                    if let Some(ref id) = target_inst_id {
                        if button_secondary(ui, tokens, Some(VectorIcon::Eye), "Открыть инстанс")
                            .clicked()
                        {
                            inspect_id = Some(id.clone());
                        }
                    }
                });
            },
        );

        if cancel_clicked {
            let _ = state.cmd_sender.send(UiWorkerCommand::CancelDownload);
        } else if let Some(id) = inspect_id {
            *action = Some(DownloadsAction::OpenInstance(id));
        }
    }

    fn render_install_error_banner(
        &self,
        ui: &mut Ui,
        state: &mut UiState,
        tokens: &Tokens,
        style: &ThemeStyleParams,
        cr: CornerRadius,
        action: &mut Option<DownloadsAction>,
    ) {
        let mut retry_id: Option<InstanceId> = None;
        let mut verify_id: Option<InstanceId> = None;
        let mut open_id: Option<InstanceId> = None;
        let mut dismiss = false;

        let failed_id = state.last_failed_install_instance.clone();
        let inst_name = failed_id
            .as_ref()
            .and_then(|id| state.instances.iter().find(|i| &i.id == id))
            .map(|i| i.name.clone())
            .or_else(|| failed_id.as_ref().map(|id| id.as_str().to_string()))
            .unwrap_or_else(|| "Инстанс".to_string());

        let (title, details) = if let Some(ref err) = state.detailed_install_error {
            (err.localized_title.clone(), err.localized_details.clone())
        } else {
            (
                "Сбой установки инстанса".to_string(),
                state.install_error.clone().unwrap_or_default(),
            )
        };

        let frame = solid_card_frame_with_config(tokens, &state.config.theme)
            .stroke(Stroke::new(1.0_f32, tokens.danger))
            .inner_margin(egui::Margin::same(12));

        show_themed_card(
            ui,
            frame,
            cr,
            ui.make_persistent_id("mq_dl_install_err_card"),
            style,
            tokens,
            |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                    paint_vector_icon(ui.painter(), r, VectorIcon::Warning, tokens.danger);
                    ui.label(
                        RichText::new(format!("{inst_name}: {title}"))
                            .color(tokens.danger)
                            .strong(),
                    );
                });

                if !details.is_empty() {
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(RichText::new(&details).color(tokens.text_muted).small())
                            .wrap(),
                    );
                }

                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                    if let Some(ref id) = failed_id {
                        if button_primary(ui, tokens, Some(VectorIcon::Refresh), "Повторить")
                            .clicked()
                        {
                            retry_id = Some(id.clone());
                        }
                        if button_secondary(ui, tokens, Some(VectorIcon::Verify), "Проверить файлы")
                            .clicked()
                        {
                            verify_id = Some(id.clone());
                        }
                        if button_secondary(ui, tokens, Some(VectorIcon::Eye), "К инстансу")
                            .clicked()
                        {
                            open_id = Some(id.clone());
                        }
                    }
                    if button_secondary(ui, tokens, Some(VectorIcon::Cross), "Скрыть").clicked()
                    {
                        dismiss = true;
                    }
                });
            },
        );

        if dismiss {
            state.install_error = None;
            state.detailed_install_error = None;
        } else if let Some(id) = retry_id {
            state.install_error = None;
            state.detailed_install_error = None;
            state.installing_instance = Some(id.clone());
            let _ = state
                .cmd_sender
                .send(UiWorkerCommand::InstallInstance { instance_id: id });
        } else if let Some(id) = verify_id {
            state.install_error = None;
            state.detailed_install_error = None;
            let _ = state
                .cmd_sender
                .send(UiWorkerCommand::VerifyAndRepairInstance { instance_id: id });
        } else if let Some(id) = open_id {
            *action = Some(DownloadsAction::OpenInstance(id));
        }
    }

    fn render_jobs_section(
        &self,
        ui: &mut Ui,
        state: &mut UiState,
        tokens: &Tokens,
        style: &ThemeStyleParams,
        cr: CornerRadius,
        action: &mut Option<DownloadsAction>,
    ) {
        let mut cancel_id: Option<u64> = None;
        let mut retry_id: Option<u64> = None;
        let mut open_inst: Option<InstanceId> = None;
        let mut clear_finished = false;

        let has_finished = state.jobs.jobs.iter().any(|j| !j.is_active());
        let total_jobs = state.jobs.jobs.len();

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
            ui.label(
                RichText::new("Очередь загрузки контента")
                    .color(tokens.text)
                    .size(15.0)
                    .strong(),
            );
            render_badge(ui, tokens, &format!("{total_jobs}"), BadgeKind::Neutral);
            if has_finished {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if button_secondary(ui, tokens, Some(VectorIcon::Trash), "Очистить завершённые")
                        .clicked()
                    {
                        clear_finished = true;
                    }
                });
            }
        });
        ui.add_space(6.0);

        let mut ordered_indices: Vec<usize> = (0..state.jobs.jobs.len())
            .filter(|&i| state.jobs.jobs[i].is_active())
            .collect();
        ordered_indices.extend(
            (0..state.jobs.jobs.len())
                .rev()
                .filter(|&i| !state.jobs.jobs[i].is_active()),
        );

        for idx in ordered_indices {
            let job = &state.jobs.jobs[idx];
            let job_inst_id = resolve_job_instance_id(job, &state.instances);
            let kind_lbl = job_kind_label(job);
            let source_opt = job_source_label(job);
            let frac = job.progress();

            let border_col = match &job.status {
                JobStatus::Running => blend_color(tokens.border, ACCENT_COBALT, 0.45),
                JobStatus::Failed(_) => blend_color(tokens.border, tokens.danger, 0.55),
                JobStatus::Cancelled => blend_color(tokens.border, tokens.warning, 0.4),
                JobStatus::Done => tokens.border,
            };

            let frame = solid_card_frame_with_config(tokens, &state.config.theme)
                .stroke(Stroke::new(1.0_f32, border_col))
                .inner_margin(egui::Margin::symmetric(12, 10));

            show_themed_card(
                ui,
                frame,
                cr,
                ui.make_persistent_id(("mq_dl_job_card", job.id)),
                style,
                tokens,
                |ui| {
                    ui.set_width(ui.available_width());

                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                        match &job.status {
                            JobStatus::Running => status_pill(ui, "Загрузка", ACCENT_COBALT),
                            JobStatus::Done => status_pill(ui, "Готово", tokens.success),
                            JobStatus::Cancelled => status_pill(ui, "Отменено", tokens.warning),
                            JobStatus::Failed(_) => status_pill(ui, "Ошибка", tokens.danger),
                        }
                        ui.label(RichText::new(&job.title).color(tokens.text).strong());
                        render_badge(ui, tokens, kind_lbl, BadgeKind::Neutral);
                        if let Some(src) = source_opt {
                            version_badge(ui, source_name(src), source_color(src));
                        }
                        if !job.target_label.is_empty() {
                            ui.label(
                                RichText::new(&job.target_label)
                                    .color(tokens.text_muted)
                                    .small(),
                            );
                        }
                    });

                    if let Some(ref stage) = job.stage_label {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!("Этап: {stage}"))
                                .color(tokens.accent)
                                .small(),
                        );
                    }

                    if let JobStatus::Failed(ref err) = job.status {
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(RichText::new(err).color(tokens.danger).small())
                                .truncate(),
                        )
                        .on_hover_text(err);
                    }

                    if job.is_active() || job.total_files > 0 || job.bytes_total > 0 {
                        ui.add_space(6.0);
                        let bar_col = match &job.status {
                            JobStatus::Running => ACCENT_COBALT,
                            JobStatus::Done => tokens.success,
                            JobStatus::Cancelled => tokens.warning,
                            JobStatus::Failed(_) => tokens.danger,
                        };
                        paint_animated_progress_bar(
                            ui,
                            Id::new(("mq_dl_job_prog", job.id)),
                            frac,
                            6.0,
                            tokens,
                            bar_col,
                        );
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                            render_badge(
                                ui,
                                tokens,
                                &format!("{:.0}%", safe_progress_clamp(frac) * 100.0),
                                if job.is_active() {
                                    BadgeKind::Accent
                                } else {
                                    BadgeKind::Neutral
                                },
                            );
                            if job.total_files > 0 || job.done_files > 0 {
                                render_badge(
                                    ui,
                                    tokens,
                                    &format!(
                                        "Файлы: {}",
                                        format_files_ratio(
                                            job.done_files as u64,
                                            job.total_files as u64
                                        )
                                    ),
                                    BadgeKind::Neutral,
                                );
                            }
                            if job.bytes_total > 0 || job.bytes_done > 0 {
                                render_badge(
                                    ui,
                                    tokens,
                                    &format!(
                                        "Объём: {}",
                                        format_bytes_ratio(job.bytes_done, job.bytes_total)
                                    ),
                                    BadgeKind::Neutral,
                                );
                            }
                            if job.is_active() {
                                let spd = sanitize_nonneg_f64(job.mib_per_sec);
                                if spd > 0.001 {
                                    render_badge(
                                        ui,
                                        tokens,
                                        &format!("Скорость: {}", format_speed_mib(spd)),
                                        BadgeKind::Neutral,
                                    );
                                }
                            }
                        });
                    }

                    let show_cancel = job.is_active();
                    let show_retry =
                        matches!(job.status, JobStatus::Failed(_) | JobStatus::Cancelled)
                            && job.retry.is_some();
                    let show_inspect = job_inst_id.is_some();

                    if show_cancel || show_retry || show_inspect {
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                            if show_cancel
                                && danger_button(ui, Some(VectorIcon::Cross), "Отмена").clicked()
                            {
                                cancel_id = Some(job.id);
                            }
                            if show_retry
                                && button_secondary(
                                    ui,
                                    tokens,
                                    Some(VectorIcon::Refresh),
                                    "Повторить",
                                )
                                .clicked()
                            {
                                retry_id = Some(job.id);
                            }
                            if let Some(ref inst_id) = job_inst_id {
                                if button_secondary(ui, tokens, Some(VectorIcon::Eye), "К инстансу")
                                    .clicked()
                                {
                                    open_inst = Some(inst_id.clone());
                                }
                            }
                        });
                    }
                },
            );
            ui.add_space(6.0);
        }

        if clear_finished {
            state.jobs.remove_finished();
        }
        if let Some(id) = cancel_id {
            state.cancel_job(id);
        }
        if let Some(id) = retry_id {
            state.retry_job(id);
        }
        if let Some(id) = open_inst {
            *action = Some(DownloadsAction::OpenInstance(id));
        }
    }

    fn render_manual_section(
        &self,
        ui: &mut Ui,
        state: &mut UiState,
        tokens: &Tokens,
        style: &ThemeStyleParams,
        cr: CornerRadius,
    ) {
        let mut recheck = false;
        let mut dismiss_completed = false;
        let mut open_url: Option<String> = None;
        let mut copy_url: Option<String> = None;
        let mut remove_idx: Option<usize> = None;

        let pending_count = state.manual.remaining();
        let (_, inst_display, _) =
            resolve_instance_display(&state.instances, &state.manual.instance_label);

        let frame = solid_card_frame_with_config(tokens, &state.config.theme)
            .stroke(Stroke::new(
                1.0_f32,
                if pending_count > 0 {
                    blend_color(tokens.border, tokens.warning, 0.55)
                } else {
                    tokens.border
                },
            ))
            .inner_margin(egui::Margin::same(12));

        show_themed_card(
            ui,
            frame,
            cr,
            ui.make_persistent_id("mq_dl_manual_section_card"),
            style,
            tokens,
            |ui| {
                ui.set_width(ui.available_width());

                let render_title = |ui: &mut Ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                    paint_vector_icon(
                        ui.painter(),
                        r,
                        if pending_count > 0 {
                            VectorIcon::Warning
                        } else {
                            VectorIcon::Check
                        },
                        if pending_count > 0 {
                            tokens.warning
                        } else {
                            tokens.success
                        },
                    );
                    ui.label(
                        RichText::new("Файлы для ручной загрузки")
                            .color(tokens.text)
                            .size(15.0)
                            .strong(),
                    );
                    render_badge(
                        ui,
                        tokens,
                        &format!("Осталось: {pending_count}"),
                        if pending_count > 0 {
                            BadgeKind::Warning
                        } else {
                            BadgeKind::Success
                        },
                    );
                };

                if ui.available_width() >= 540.0 {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                        render_title(ui);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                            if pending_count == 0
                                && button_secondary(ui, tokens, Some(VectorIcon::Cross), "Скрыть")
                                    .clicked()
                            {
                                dismiss_completed = true;
                            }
                            if button_secondary(
                                ui,
                                tokens,
                                Some(VectorIcon::Refresh),
                                "Проверить папку",
                            )
                            .clicked()
                            {
                                recheck = true;
                            }
                        });
                    });
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                        render_title(ui);
                    });
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                        if button_secondary(
                            ui,
                            tokens,
                            Some(VectorIcon::Refresh),
                            "Проверить папку",
                        )
                        .clicked()
                        {
                            recheck = true;
                        }
                        if pending_count == 0
                            && button_secondary(ui, tokens, Some(VectorIcon::Cross), "Скрыть")
                                .clicked()
                        {
                            dismiss_completed = true;
                        }
                    });
                }

                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "Автор проекта запретил автоматическую загрузку. Скачайте файлы в браузере или перетащите их в окно лаунчера.",
                        )
                        .color(tokens.text_muted)
                        .small(),
                    )
                    .wrap(),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "Папка отслеживания: {} · Инстанс: {}",
                            state.manual.watch_dir_label, inst_display
                        ))
                        .color(tokens.text_muted)
                        .small(),
                    )
                    .wrap(),
                );

                if !state.manual.pending.is_empty() {
                    ui.add_space(8.0);
                    for (idx, item) in state.manual.pending.iter().enumerate() {
                        egui::Frame::new()
                            .fill(tokens.bg_input)
                            .stroke(Stroke::new(1.0_f32, tokens.border))
                            .corner_radius(RADIUS_TAG)
                            .inner_margin(egui::Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                                    ui.label(
                                        RichText::new(&item.filename).color(tokens.text).strong(),
                                    );
                                    if !item.project_title.is_empty()
                                        && item.project_title != item.filename
                                    {
                                        render_badge(
                                            ui,
                                            tokens,
                                            &item.project_title,
                                            BadgeKind::Neutral,
                                        );
                                    }
                                    if item.expected_size > 0 {
                                        render_badge(
                                            ui,
                                            tokens,
                                            &format_size(item.expected_size),
                                            BadgeKind::Neutral,
                                        );
                                    }
                                    if !item.target_rel_path.is_empty() {
                                        ui.label(
                                            RichText::new(&item.target_rel_path)
                                                .color(tokens.text_muted)
                                                .small(),
                                        );
                                    }
                                });

                                ui.add_space(6.0);
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                                    if button_secondary(
                                        ui,
                                        tokens,
                                        Some(VectorIcon::Globe),
                                        "Открыть в браузере",
                                    )
                                    .clicked()
                                    {
                                        open_url = Some(item.webpage_url.clone());
                                    }
                                    if button_secondary(
                                        ui,
                                        tokens,
                                        Some(VectorIcon::Copy),
                                        "Копировать URL",
                                    )
                                    .clicked()
                                    {
                                        copy_url = Some(item.webpage_url.clone());
                                    }
                                    if danger_button(ui, Some(VectorIcon::Trash), "Убрать")
                                        .clicked()
                                    {
                                        remove_idx = Some(idx);
                                    }
                                });
                            });
                        ui.add_space(4.0);
                    }
                }

                if !state.manual.completed.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                        for name in &state.manual.completed {
                            render_badge(
                                ui,
                                tokens,
                                &format!("Получено: {name}"),
                                BadgeKind::Success,
                            );
                        }
                    });
                }

                if let Some(ref msg) = state.manual.message {
                    ui.add_space(4.0);
                    ui.label(RichText::new(msg).color(tokens.success).small());
                }
            },
        );

        if let Some(url) = open_url {
            if is_safe_http_url(&url) {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            } else {
                state.status_message =
                    Some(("Некорректный URL для ручной загрузки".to_string(), true));
            }
        }
        if let Some(url) = copy_url {
            ui.ctx().copy_text(url);
            state.status_message = Some((
                "Ссылка на страницу скачивания скопирована".to_string(),
                false,
            ));
        }
        if let Some(idx) = remove_idx {
            remove_pending_manual_item(state, idx);
        }
        if recheck {
            state.manual.last_poll = Some(Instant::now());
            let _ = state.cmd_sender.send(UiWorkerCommand::PollManual {
                dropped: Vec::new(),
            });
        }
        if dismiss_completed && state.manual.pending.is_empty() {
            state.manual.completed.clear();
            state.manual.message = None;
            state.manual.open = false;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_speed_chart(
        &self,
        ui: &mut Ui,
        state: &UiState,
        tokens: &Tokens,
        style: &ThemeStyleParams,
        cr: CornerRadius,
        stats: &SpeedChartStats,
        live_current_mib: f64,
    ) {
        let shown_current = if live_current_mib > 0.001 {
            live_current_mib
        } else {
            stats.current_mib as f64
        };
        let shown_peak = (stats.peak_mib as f64).max(shown_current);

        let frame = solid_card_frame_with_config(tokens, &state.config.theme)
            .inner_margin(egui::Margin::same(12));

        show_themed_card(
            ui,
            frame,
            cr,
            ui.make_persistent_id("mq_dl_speed_chart_card"),
            style,
            tokens,
            |ui| {
                ui.set_width(ui.available_width());

                let mut badges: Vec<(String, BadgeKind)> = Vec::with_capacity(3);
                badges.push((
                    format!("Текущая: {}", format_speed_mib(shown_current)),
                    BadgeKind::Accent,
                ));
                badges.push((
                    format!("Пик: {}", format_speed_mib(shown_peak)),
                    BadgeKind::Neutral,
                ));
                if stats.avg_mib > 0.01 {
                    badges.push((
                        format!("Средняя: {}", format_speed_mib(stats.avg_mib as f64)),
                        BadgeKind::Neutral,
                    ));
                }

                let render_badges = |ui: &mut Ui, rtl: bool| {
                    if rtl {
                        for (label, kind) in badges.iter().rev() {
                            render_badge(ui, tokens, label, *kind);
                        }
                    } else {
                        for (label, kind) in &badges {
                            render_badge(ui, tokens, label, *kind);
                        }
                    }
                };

                if ui.available_width() >= 620.0 {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        paint_vector_icon(ui.painter(), r, VectorIcon::Activity, tokens.accent);
                        ui.label(
                            RichText::new("Скорость загрузки в реальном времени")
                                .color(tokens.text)
                                .strong(),
                        );

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                            render_badges(ui, true);
                        });
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        paint_vector_icon(ui.painter(), r, VectorIcon::Activity, tokens.accent);
                        ui.label(
                            RichText::new("Скорость загрузки в реальном времени")
                                .color(tokens.text)
                                .strong(),
                        );
                    });
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                        render_badges(ui, false);
                    });
                }

                ui.add_space(8.0);
                paint_speed_sparkline(ui, tokens, stats);
            },
        );
    }

    fn render_history_section(
        &self,
        ui: &mut Ui,
        state: &mut UiState,
        tokens: &Tokens,
        style: &ThemeStyleParams,
        cr: CornerRadius,
        action: &mut Option<DownloadsAction>,
    ) {
        let filter_id = Id::new("mq_downloads_history_filter");
        let mut active_filter = ui
            .ctx()
            .data(|d| d.get_temp::<HistoryFilter>(filter_id))
            .unwrap_or_default();

        let (completed_cnt, failed_cnt, cancelled_cnt) =
            count_history_statuses(&state.download_history);
        let total_cnt = state.download_history.len();

        let mut clear_history = false;
        let mut jump_inst: Option<InstanceId> = None;

        let frame = solid_card_frame_with_config(tokens, &state.config.theme)
            .inner_margin(egui::Margin::same(12));

        show_themed_card(
            ui,
            frame,
            cr,
            ui.make_persistent_id("mq_dl_history_card"),
            style,
            tokens,
            |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                    ui.label(
                        RichText::new("История загрузок за сессию")
                            .color(tokens.text)
                            .size(15.0)
                            .strong(),
                    );
                    render_badge(ui, tokens, &format!("{total_cnt}"), BadgeKind::Neutral);

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if button_secondary(ui, tokens, Some(VectorIcon::Trash), "Очистить")
                            .on_hover_text("Очистить список истории загрузок")
                            .clicked()
                        {
                            clear_history = true;
                        }
                    });
                });

                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                    if FilterChip::new("Все", active_filter == HistoryFilter::All)
                        .count(total_cnt)
                        .show(ui, tokens)
                        .clicked()
                    {
                        active_filter = HistoryFilter::All;
                    }
                    if FilterChip::new("Завершено", active_filter == HistoryFilter::Completed)
                        .dot_color(tokens.success)
                        .count(completed_cnt)
                        .show(ui, tokens)
                        .clicked()
                    {
                        active_filter = HistoryFilter::Completed;
                    }
                    if FilterChip::new("Ошибки", active_filter == HistoryFilter::Failed)
                        .dot_color(tokens.danger)
                        .count(failed_cnt)
                        .show(ui, tokens)
                        .clicked()
                    {
                        active_filter = HistoryFilter::Failed;
                    }
                    if FilterChip::new("Отменено", active_filter == HistoryFilter::Cancelled)
                        .dot_color(tokens.warning)
                        .count(cancelled_cnt)
                        .show(ui, tokens)
                        .clicked()
                    {
                        active_filter = HistoryFilter::Cancelled;
                    }
                });

                ui.add_space(8.0);

                let filtered: Vec<&DownloadHistoryEntry> = state
                    .download_history
                    .iter()
                    .filter(|entry| match active_filter {
                        HistoryFilter::All => true,
                        HistoryFilter::Completed => {
                            entry.status == DownloadHistoryStatus::Completed
                        }
                        HistoryFilter::Failed => entry.status == DownloadHistoryStatus::Failed,
                        HistoryFilter::Cancelled => {
                            entry.status == DownloadHistoryStatus::Cancelled
                        }
                    })
                    .collect();

                if filtered.is_empty() {
                    ui.label(
                        RichText::new("Нет записей для выбранного фильтра")
                            .color(tokens.text_muted)
                            .small(),
                    );
                } else {
                    for (idx, entry) in filtered.iter().enumerate() {
                        if idx > 0 {
                            ui.add_space(4.0);
                        }
                        let (status_lbl, status_col) = match entry.status {
                            DownloadHistoryStatus::Completed => ("Завершено", tokens.success),
                            DownloadHistoryStatus::Failed => ("Ошибка", tokens.danger),
                            DownloadHistoryStatus::Cancelled => ("Отменено", tokens.warning),
                        };

                        egui::Frame::new()
                            .fill(tokens.bg_input)
                            .stroke(Stroke::new(1.0_f32, tokens.border))
                            .corner_radius(RADIUS_TAG)
                            .inner_margin(egui::Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                                    status_pill(ui, status_lbl, status_col);
                                    if !entry.category.is_empty() {
                                        render_badge(
                                            ui,
                                            tokens,
                                            &entry.category,
                                            BadgeKind::Neutral,
                                        );
                                    }
                                    ui.label(
                                        RichText::new(&entry.title).color(tokens.text).strong(),
                                    );
                                    if !entry.timestamp.is_empty() {
                                        ui.label(
                                            RichText::new(&entry.timestamp)
                                                .font(FontId::monospace(11.5))
                                                .color(tokens.text_muted),
                                        );
                                    }
                                });

                                if !entry.detail.is_empty() || entry.instance_id.is_some() {
                                    ui.add_space(4.0);
                                    ui.horizontal_wrapped(|ui| {
                                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                                        if !entry.detail.is_empty() {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&entry.detail)
                                                        .color(tokens.text_muted)
                                                        .small(),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(&entry.detail);
                                        }
                                        if let Some(ref inst_id) = entry.instance_id {
                                            let inst_hint = state
                                                .instances
                                                .iter()
                                                .find(|i| &i.id == inst_id)
                                                .map(|i| format!("Перейти к инстансу «{}»", i.name))
                                                .unwrap_or_else(|| {
                                                    format!(
                                                        "Перейти к инстансу {}",
                                                        inst_id.as_str()
                                                    )
                                                });
                                            if button_secondary(
                                                ui,
                                                tokens,
                                                Some(VectorIcon::Eye),
                                                "К инстансу",
                                            )
                                            .on_hover_text(inst_hint)
                                            .clicked()
                                            {
                                                jump_inst = Some(inst_id.clone());
                                            }
                                        }
                                    });
                                }
                            });
                    }
                }
            },
        );

        ui.ctx()
            .data_mut(|d| d.insert_temp(filter_id, active_filter));

        if clear_history {
            state.download_history.clear();
            state.status_message = Some(("История загрузок очищена".to_string(), false));
        } else if let Some(id) = jump_inst {
            *action = Some(DownloadsAction::OpenInstance(id));
        }
    }

    fn render_empty_state_card(
        &self,
        ui: &mut Ui,
        state: &UiState,
        tokens: &Tokens,
        style: &ThemeStyleParams,
        cr: CornerRadius,
        action: &mut Option<DownloadsAction>,
    ) {
        let frame = solid_card_frame_with_config(tokens, &state.config.theme)
            .inner_margin(egui::Margin::symmetric(20, 28));

        show_themed_card(
            ui,
            frame,
            cr,
            ui.make_persistent_id("mq_dl_empty_state_card"),
            style,
            tokens,
            |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.add_space(12.0);
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(46.0), Sense::hover());
                    paint_vector_icon(ui.painter(), rect, VectorIcon::Download, tokens.text_muted);
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("Активных загрузок нет")
                            .size(16.0)
                            .strong()
                            .color(tokens.text),
                    );
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(
                                "Выберите моды, шейдеры, ресурспаки или сборки в каталоге, либо запустите установку инстанса — здесь отобразятся скорость, этапы и журнал сессии.",
                            )
                            .size(13.0)
                            .color(tokens.text_muted),
                        )
                        .wrap(),
                    );
                    ui.add_space(16.0);
                    if button_primary(ui, tokens, Some(VectorIcon::Catalog), "Перейти в каталог")
                        .clicked()
                    {
                        *action = Some(DownloadsAction::OpenCatalog);
                    }
                    ui.add_space(8.0);
                });
            },
        );
    }
}

fn render_stage_stepper(ui: &mut Ui, tokens: &Tokens, current_stage: InstallStage) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
        let cur_idx = current_stage.pipeline_index();
        for (i, stage) in InstallStage::ALL_DISPLAY_STAGES.iter().enumerate() {
            if i > 0 {
                ui.label(RichText::new("→").color(tokens.text_disabled).size(11.5));
            }
            let stage_idx = stage.pipeline_index();
            if stage_idx < cur_idx {
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(11.0), Sense::hover());
                paint_vector_icon(ui.painter(), icon_rect, VectorIcon::Check, tokens.accent);
                ui.label(
                    RichText::new(stage.localized_name())
                        .color(tokens.accent)
                        .size(11.5)
                        .strong(),
                );
            } else if stage_idx == cur_idx {
                let resp = ui.label(
                    RichText::new(stage.localized_name())
                        .color(tokens.accent)
                        .size(11.5)
                        .strong(),
                );
                ui.painter().line_segment(
                    [
                        Pos2::new(resp.rect.min.x, resp.rect.max.y + 1.0),
                        Pos2::new(resp.rect.max.x, resp.rect.max.y + 1.0),
                    ],
                    Stroke::new(1.5_f32, tokens.accent),
                );
            } else {
                ui.label(
                    RichText::new(stage.localized_name())
                        .color(tokens.text_disabled)
                        .size(11.5),
                );
            }
        }
    });
}

fn paint_animated_progress_bar(
    ui: &mut Ui,
    anim_id: Id,
    fraction: f32,
    height: f32,
    tokens: &Tokens,
    fill_color: Color32,
) {
    let clamped = safe_progress_clamp(fraction);
    let smooth =
        animate_spring_f32(ui.ctx(), anim_id, clamped, SpringConfig::smooth()).clamp(0.0, 1.0);
    let bar_h = height.max(4.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), bar_h), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(rect, RADIUS_TAG, tokens.bg_input);
    painter.rect_stroke(
        rect,
        RADIUS_TAG,
        Stroke::new(1.0_f32, tokens.border),
        StrokeKind::Inside,
    );
    if smooth > 0.001 {
        let fill_w = (rect.width() * smooth).max(4.0);
        let fill_rect = Rect::from_min_size(rect.min, Vec2::new(fill_w, bar_h));
        painter.rect_filled(fill_rect, RADIUS_TAG, fill_color);
    }
}

fn paint_speed_sparkline(ui: &mut Ui, tokens: &Tokens, stats: &SpeedChartStats) {
    let desired = Vec2::new(ui.available_width().max(120.0), 108.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter();
    painter.rect_filled(rect, RADIUS_TAG, tokens.bg_input);
    painter.rect_stroke(
        rect,
        RADIUS_TAG,
        Stroke::new(1.0_f32, tokens.border),
        StrokeKind::Inside,
    );

    let inner = rect.shrink2(Vec2::new(10.0, 14.0));
    if !inner.is_positive() {
        return;
    }

    let grid_stroke = Stroke::new(1.0_f32, tokens.border.gamma_multiply(0.55));
    for frac in [0.25_f32, 0.50_f32, 0.75_f32] {
        let y = inner.bottom() - inner.height() * frac;
        painter.line_segment(
            [Pos2::new(inner.left(), y), Pos2::new(inner.right(), y)],
            grid_stroke,
        );
    }

    if stats.samples.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Ожидание данных о скорости...",
            FontId::proportional(12.5),
            tokens.text_muted,
        );
        return;
    }

    let scale_max = (stats.peak_mib * 1.15).max(1.0);
    let n = stats.samples.len();

    painter.text(
        Pos2::new(inner.left() + 2.0, rect.top() + 3.0),
        egui::Align2::LEFT_TOP,
        format_speed_mib(scale_max as f64),
        FontId::monospace(10.0),
        tokens.text_disabled,
    );
    painter.text(
        Pos2::new(inner.left() + 2.0, rect.bottom() - 3.0),
        egui::Align2::LEFT_BOTTOM,
        "0 КБ/с",
        FontId::monospace(10.0),
        tokens.text_disabled,
    );

    let mut points = Vec::with_capacity(n);
    for (i, &val) in stats.samples.iter().enumerate() {
        let t_x = if n <= 1 {
            1.0
        } else {
            i as f32 / (n - 1) as f32
        };
        let norm_y = safe_clamp_f32(val / scale_max, 0.0, 1.0);
        let x = inner.left() + inner.width() * t_x;
        let y = inner.bottom() - inner.height() * norm_y;
        points.push(Pos2::new(x, y));
    }

    let line_col = tokens.accent;
    if points.len() >= 2 {
        let top_col = with_alpha(line_col, 0.24);
        let bot_col = with_alpha(line_col, 0.02);
        let mut mesh = Mesh::default();
        for pt in &points {
            mesh.vertices.push(Vertex {
                pos: *pt,
                uv: Pos2::ZERO,
                color: top_col,
            });
            mesh.vertices.push(Vertex {
                pos: Pos2::new(pt.x, inner.bottom()),
                uv: Pos2::ZERO,
                color: bot_col,
            });
        }
        for i in 0..(points.len() - 1) {
            let t0 = (i * 2) as u32;
            let b0 = t0 + 1;
            let t1 = t0 + 2;
            let b1 = t0 + 3;
            mesh.add_triangle(t0, t1, b0);
            mesh.add_triangle(t1, b1, b0);
        }
        painter.add(mesh);
        painter.add(Shape::line(points.clone(), Stroke::new(2.0_f32, line_col)));
    } else if let Some(&pt) = points.first() {
        painter.line_segment(
            [Pos2::new(inner.left(), pt.y), pt],
            Stroke::new(2.0_f32, line_col),
        );
    }

    if let Some(&last_pt) = points.last() {
        painter.circle_filled(last_pt, 6.0, line_col.gamma_multiply(0.26));
        painter.circle_filled(last_pt, 3.2, line_col);
    }

    if let Some(hover_pos) = response.hover_pos().filter(|_| response.hovered()) {
        let nearest = points
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                (a.x - hover_pos.x)
                    .abs()
                    .total_cmp(&(b.x - hover_pos.x).abs())
            })
            .map(|(idx, &pt)| (idx, pt));
        if let Some((idx, pt)) = nearest {
            painter.line_segment(
                [
                    Pos2::new(pt.x, inner.top()),
                    Pos2::new(pt.x, inner.bottom()),
                ],
                Stroke::new(1.0_f32, tokens.border_strong),
            );
            painter.circle_filled(pt, 4.5, tokens.text);
            let sample_val = stats.samples[idx];
            response.on_hover_text(format!("Скорость: {}", format_speed_mib(sample_val as f64)));
        }
    }
}

fn remove_pending_manual_item(state: &mut UiState, idx: usize) {
    if idx < state.manual.pending.len() {
        state.manual.pending.remove(idx);
    }
    if state.manual.pending.is_empty() {
        state.manual.open = false;
        state.manual_watcher = None;
        return;
    }
    if state.manual_watcher.is_some() {
        let inst_slug = state
            .instances
            .iter()
            .find(|i| {
                i.id.as_str() == state.manual.instance_label
                    || i.name == state.manual.instance_label
            })
            .map(|i| i.id.as_str().to_string())
            .unwrap_or_else(|| state.manual.instance_label.clone());
        if InstanceId::is_valid_slug(&inst_slug) {
            let instance_dir = state.paths.instance_dir(&inst_slug);
            let watch_dir = default_downloads_dir().unwrap_or_else(|| state.paths.temp_dir());
            state.manual_watcher = Some(std::sync::Arc::new(parking_lot::Mutex::new(
                mq_mods::BlockedDownloadsWatcher::new(
                    instance_dir,
                    watch_dir,
                    state.manual.pending.clone(),
                ),
            )));
        } else {
            state.manual_watcher = None;
        }
    }
}

fn compute_downloads_summary(state: &UiState) -> DownloadsSummary {
    let active_count = state.active_download_count();
    let active_jobs_count = state.jobs.active_count();
    let manual_pending_count = state.manual.remaining();
    let history_count = state.download_history.len();
    let speed_stats = compute_speed_stats(&state.download_speed_history);

    let mut job_bytes_done: u64 = 0;
    let mut job_bytes_total: u64 = 0;
    let mut job_items_done: u64 = 0;
    let mut job_items_total: u64 = 0;
    let mut job_speed_sum: f64 = 0.0;

    for job in state.jobs.jobs.iter().filter(|j| j.is_active()) {
        job_bytes_done = job_bytes_done.saturating_add(job.bytes_done);
        job_bytes_total = job_bytes_total.saturating_add(job.bytes_total);
        job_items_done = job_items_done.saturating_add(job.done_files as u64);
        job_items_total = job_items_total.saturating_add(job.total_files as u64);
        job_speed_sum += sanitize_nonneg_f64(job.mib_per_sec);
    }

    let (bytes_done, bytes_total, items_done, items_total, files_per_sec) =
        if state.installing_instance.is_some()
            || (active_jobs_count == 0 && state.active_phase.is_some())
        {
            if let Some(ref p) = state.phase_progress {
                (
                    p.bytes_done.saturating_add(job_bytes_done),
                    p.bytes_total.saturating_add(job_bytes_total),
                    p.completed_items.saturating_add(job_items_done),
                    p.total_items.saturating_add(job_items_total),
                    sanitize_nonneg_f64(p.files_per_sec),
                )
            } else {
                (
                    job_bytes_done,
                    job_bytes_total,
                    job_items_done,
                    job_items_total,
                    0.0,
                )
            }
        } else if active_jobs_count > 0 {
            let (bd, bt) = if job_bytes_total == 0 && job_bytes_done == 0 {
                state
                    .phase_progress
                    .as_ref()
                    .map_or((0, 0), |p| (p.bytes_done, p.bytes_total))
            } else {
                (job_bytes_done, job_bytes_total)
            };
            let (id, it) = if job_items_total == 0 && job_items_done == 0 {
                state
                    .phase_progress
                    .as_ref()
                    .map_or((0, 0), |p| (p.completed_items, p.total_items))
            } else {
                (job_items_done, job_items_total)
            };
            let fps = state
                .phase_progress
                .as_ref()
                .map_or(0.0, |p| sanitize_nonneg_f64(p.files_per_sec));
            (bd, bt, id, it, fps)
        } else {
            (0, 0, 0, 0, 0.0)
        };

    let current_mib_per_sec = if active_count == 0 {
        0.0
    } else if let Some(p_spd) = state
        .phase_progress
        .as_ref()
        .map(|p| sanitize_nonneg_f64(p.mib_per_sec))
        .filter(|&s| s > 0.001)
    {
        p_spd
    } else if job_speed_sum > 0.001 {
        job_speed_sum
    } else {
        speed_stats.current_mib as f64
    };

    let peak_mib_per_sec = (speed_stats.peak_mib as f64).max(current_mib_per_sec);

    let overall_progress = if items_total > 0 {
        let ratio = safe_progress_ratio(items_done, items_total);
        if ratio == 0.0 && bytes_total > 0 && bytes_done > 0 {
            safe_progress_ratio(bytes_done, bytes_total).min(0.99)
        } else {
            ratio
        }
    } else if bytes_total > 0 {
        safe_progress_ratio(bytes_done, bytes_total)
    } else {
        0.0
    };

    DownloadsSummary {
        active_count,
        active_jobs_count,
        manual_pending_count,
        history_count,
        current_mib_per_sec,
        peak_mib_per_sec,
        files_per_sec,
        bytes_done,
        bytes_total,
        items_done,
        items_total,
        overall_progress,
    }
}

fn compute_speed_stats(history: &VecDeque<f32>) -> SpeedChartStats {
    let samples: Vec<f32> = history
        .iter()
        .copied()
        .take(MAX_TELEMETRY_SAMPLES)
        .map(sanitize_speed_sample)
        .collect();
    if samples.is_empty() {
        return SpeedChartStats {
            samples,
            current_mib: 0.0,
            peak_mib: 0.0,
            avg_mib: 0.0,
        };
    }
    let current_mib = *samples.last().unwrap_or(&0.0);
    let mut peak_mib = 0.0_f32;
    let mut sum = 0.0_f64;
    for &v in &samples {
        if v > peak_mib {
            peak_mib = v;
        }
        sum += v as f64;
    }
    let avg_mib = (sum / samples.len() as f64) as f32;
    SpeedChartStats {
        samples,
        current_mib,
        peak_mib,
        avg_mib,
    }
}

fn count_history_statuses(history: &VecDeque<DownloadHistoryEntry>) -> (usize, usize, usize) {
    let mut completed = 0usize;
    let mut failed = 0usize;
    let mut cancelled = 0usize;
    for item in history {
        match item.status {
            DownloadHistoryStatus::Completed => completed += 1,
            DownloadHistoryStatus::Failed => failed += 1,
            DownloadHistoryStatus::Cancelled => cancelled += 1,
        }
    }
    (completed, failed, cancelled)
}

#[inline]
fn sanitize_nonneg_f64(val: f64) -> f64 {
    if val.is_finite() && val >= 0.0 {
        val
    } else {
        0.0
    }
}

#[inline]
fn sanitize_speed_sample(val: f32) -> f32 {
    if val.is_finite() && val >= 0.0 {
        val
    } else {
        0.0
    }
}

/// Formats speed in MiB/s as localized `МБ/с` or `КБ/с`.
pub fn format_speed_mib(mib_per_sec: f64) -> String {
    let mib = sanitize_nonneg_f64(mib_per_sec);
    if mib >= 100.0 {
        format!("{mib:.0} МБ/с")
    } else if mib >= 1.0 {
        format!("{mib:.1} МБ/с")
    } else {
        let kib = mib * 1024.0;
        if kib < 0.5 {
            "0 КБ/с".to_string()
        } else {
            format!("{kib:.0} КБ/с")
        }
    }
}

/// Formats ETA in seconds as localized Russian duration.
pub fn format_eta_secs(eta_secs: Option<u64>) -> String {
    match eta_secs {
        None => "расчёт...".to_string(),
        Some(secs) if secs < 60 => format!("~{secs} с"),
        Some(secs) if secs < 3600 => format!("~{} мин {} с", secs / 60, secs % 60),
        Some(secs) => format!("~{} ч {} мин", secs / 3600, (secs % 3600) / 60),
    }
}

/// Formats completed/total bytes using localized units (`Б`, `КБ`, `МБ`, `ГБ`).
pub fn format_bytes_ratio(done: u64, total: u64) -> String {
    if total > 0 {
        format!("{} / {}", format_size(done), format_size(total))
    } else if done > 0 {
        format_size(done)
    } else {
        "—".to_string()
    }
}

/// Formats completed/total files counter.
pub fn format_files_ratio(done: u64, total: u64) -> String {
    if total > 0 {
        format!("{done} / {total} ф.")
    } else if done > 0 {
        format!("{done} ф.")
    } else {
        "—".to_string()
    }
}

const fn project_kind_label(kind: ProjectKind) -> &'static str {
    match kind {
        ProjectKind::Mod => "Мод",
        ProjectKind::ResourcePack => "Ресурспак",
        ProjectKind::Shader => "Шейдер",
        ProjectKind::Modpack => "Сборка",
    }
}

fn job_kind_label(job: &InstallJob) -> &'static str {
    match &job.retry {
        Some(RetrySpec::Content { kind, .. }) => project_kind_label(*kind),
        Some(RetrySpec::Modpack { .. }) => "Сборка",
        None => {
            if job.title.starts_with("Обновление") {
                "Обновление"
            } else {
                "Контент"
            }
        }
    }
}

fn job_source_label(job: &InstallJob) -> Option<CatalogSource> {
    match &job.retry {
        Some(RetrySpec::Content { source, .. }) => Some(*source),
        _ => None,
    }
}

fn extract_target_instance_label(target_label: &str) -> Option<&str> {
    let start = target_label.find('«')?;
    let rest = &target_label[start + '«'.len_utf8()..];
    let end = rest.find('»')?;
    let inner = rest[..end].trim();
    (!inner.is_empty()).then_some(inner)
}

fn resolve_instance_display(
    instances: &[InstanceManifest],
    id_or_name: &str,
) -> (Option<InstanceId>, String, Option<(String, ModLoaderKind)>) {
    if let Some(inst) = instances
        .iter()
        .find(|i| i.id.as_str() == id_or_name || i.name == id_or_name)
    {
        (
            Some(inst.id.clone()),
            inst.name.clone(),
            Some((inst.mc_version.clone(), inst.loader)),
        )
    } else {
        (None, id_or_name.to_string(), None)
    }
}

fn resolve_job_instance_id(job: &InstallJob, instances: &[InstanceManifest]) -> Option<InstanceId> {
    match &job.retry {
        Some(RetrySpec::Content { instance_id, .. }) => {
            if instances.is_empty() || instances.iter().any(|i| i.id == *instance_id) {
                Some(instance_id.clone())
            } else {
                None
            }
        }
        Some(RetrySpec::Modpack { instance_name, .. }) => instances
            .iter()
            .find(|i| i.name == *instance_name || i.id.as_str() == *instance_name)
            .map(|i| i.id.clone()),
        None => {
            let label = extract_target_instance_label(&job.target_label)?;
            instances
                .iter()
                .find(|i| i.id.as_str() == label || i.name == label)
                .map(|i| i.id.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog_flow::{JobQueue, ModpackPreview};
    use mq_core::{AppConfig, MqPaths, PhaseProgress};
    use mq_mods::BlockedDownload;
    use mq_sched::ComputePool;
    use std::sync::Arc;

    fn make_test_state() -> (UiState, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = MqPaths::with_root(tmp.path().to_path_buf());
        let state = UiState::new(paths, AppConfig::default(), Arc::new(ComputePool::new(1)))
            .expect("UiState::new");
        (state, tmp)
    }

    #[test]
    fn format_speed_mib_handles_units_and_invalid_values() {
        assert_eq!(format_speed_mib(0.0), "0 КБ/с");
        assert_eq!(format_speed_mib(-3.5), "0 КБ/с");
        assert_eq!(format_speed_mib(f64::NAN), "0 КБ/с");
        assert_eq!(format_speed_mib(f64::INFINITY), "0 КБ/с");
        assert_eq!(format_speed_mib(0.5), "512 КБ/с");
        assert_eq!(format_speed_mib(1.0), "1.0 МБ/с");
        assert_eq!(format_speed_mib(12.34), "12.3 МБ/с");
        assert_eq!(format_speed_mib(145.6), "146 МБ/с");
    }

    #[test]
    fn format_bytes_files_and_eta_edge_cases() {
        assert_eq!(format_bytes_ratio(0, 0), "—");
        assert_eq!(format_bytes_ratio(2048, 0), "2 КБ");
        assert_eq!(
            format_bytes_ratio(5 * 1024 * 1024, 10 * 1024 * 1024),
            "5.0 МБ / 10.0 МБ"
        );
        assert_eq!(format_files_ratio(0, 0), "—");
        assert_eq!(format_files_ratio(3, 0), "3 ф.");
        assert_eq!(format_files_ratio(3, 12), "3 / 12 ф.");

        assert_eq!(format_eta_secs(None), "расчёт...");
        assert_eq!(format_eta_secs(Some(0)), "~0 с");
        assert_eq!(format_eta_secs(Some(42)), "~42 с");
        assert_eq!(format_eta_secs(Some(125)), "~2 мин 5 с");
        assert_eq!(format_eta_secs(Some(3665)), "~1 ч 1 мин");
    }

    #[test]
    fn speed_stats_sanitize_nan_and_compute_peak_current_avg() {
        let empty = VecDeque::new();
        let s0 = compute_speed_stats(&empty);
        assert!(s0.samples.is_empty());
        assert_eq!(s0.current_mib, 0.0);
        assert_eq!(s0.peak_mib, 0.0);
        assert_eq!(s0.avg_mib, 0.0);

        let mut buf = VecDeque::from(vec![f32::NAN, -4.0, 2.0, f32::INFINITY, 6.0, 4.0]);
        crate::state::push_rolling_sample(&mut buf, 8.0);
        let stats = compute_speed_stats(&buf);
        assert_eq!(stats.samples, vec![0.0, 0.0, 2.0, 0.0, 6.0, 4.0, 8.0]);
        assert!((stats.current_mib - 8.0).abs() < 1e-5);
        assert!((stats.peak_mib - 8.0).abs() < 1e-5);
        assert!((stats.avg_mib - (20.0 / 7.0)).abs() < 1e-4);
    }

    #[test]
    fn push_rolling_sample_caps_at_max_telemetry_samples() {
        let mut deque = VecDeque::new();
        for i in 0..(MAX_TELEMETRY_SAMPLES + 15) {
            crate::state::push_rolling_sample(&mut deque, i);
        }
        assert_eq!(deque.len(), MAX_TELEMETRY_SAMPLES);
        assert_eq!(deque.front().copied(), Some(15));
        assert_eq!(deque.back().copied(), Some(MAX_TELEMETRY_SAMPLES + 14));
    }

    #[test]
    fn job_metadata_and_instance_resolution() {
        let (mut state, _tmp) = make_test_state();
        let inst = state
            .instance_mgr
            .create_instance("My Fabric Pack", "1.21.4", ModLoaderKind::Fabric, None)
            .expect("create instance");
        state.instances.push(inst.clone());

        let mut q = JobQueue::default();
        let (j_content, _) = q.push(
            "Sodium".into(),
            "Инстанс «My Fabric Pack»".into(),
            2,
            Some(RetrySpec::Content {
                instance_id: inst.id.clone(),
                kind: ProjectKind::Shader,
                source: CatalogSource::Modrinth,
                downloads: Vec::new(),
                blocked: Vec::new(),
            }),
        );
        let (j_pack, _) = q.push(
            "Pack".into(),
            "Новый инстанс «My Fabric Pack»".into(),
            10,
            Some(RetrySpec::Modpack {
                instance_name: "My Fabric Pack".into(),
                preview: ModpackPreview {
                    mc_version: "1.21.4".into(),
                    loader: ModLoaderKind::Fabric,
                    loader_version: None,
                    file_count: 10,
                    total_size: 1000,
                    blocked: Vec::new(),
                    archive_path: "pack.mrpack".into(),
                },
            }),
        );
        let (j_update, _) = q.push(
            "Обновление: lithium.jar".into(),
            format!("Инстанс «{}»", inst.id.as_str()),
            1,
            None,
        );

        let jc = q.jobs.iter().find(|j| j.id == j_content).unwrap();
        assert_eq!(job_kind_label(jc), "Шейдер");
        assert_eq!(job_source_label(jc), Some(CatalogSource::Modrinth));
        assert_eq!(
            resolve_job_instance_id(jc, &state.instances),
            Some(inst.id.clone())
        );

        let jp = q.jobs.iter().find(|j| j.id == j_pack).unwrap();
        assert_eq!(job_kind_label(jp), "Сборка");
        assert_eq!(job_source_label(jp), None);
        assert_eq!(
            resolve_job_instance_id(jp, &state.instances),
            Some(inst.id.clone())
        );

        let ju = q.jobs.iter().find(|j| j.id == j_update).unwrap();
        assert_eq!(job_kind_label(ju), "Обновление");
        assert_eq!(resolve_job_instance_id(ju, &state.instances), Some(inst.id));
        assert_eq!(
            extract_target_instance_label("Инстанс «demo-pack»"),
            Some("demo-pack")
        );
    }

    #[test]
    fn downloads_view_renders_empty_and_populated_states_without_panicking() {
        let (mut state, _tmp) = make_test_state();
        let mut view = DownloadsView;
        let ctx = egui::Context::default();

        // 1. Empty state render
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let act = view.ui(ui, &mut state);
                assert_eq!(act, None);
            });
        });

        // 2. Populated state (active instance install + jobs + manual + speed + history)
        let inst = state
            .instance_mgr
            .create_instance("Survival 1.21.4", "1.21.4", ModLoaderKind::Fabric, None)
            .expect("create instance");
        state.instances.push(inst.clone());
        state.installing_instance = Some(inst.id.clone());
        state.installing_operation = Some("Установка игры".into());
        state.active_phase = Some("assets".into());
        state.install_eta_secs = Some(42);
        state.phase_progress = Some(PhaseProgress {
            phase: "assets".into(),
            completed_items: 25,
            total_items: 100,
            bytes_done: 12_500_000,
            bytes_total: 50_000_000,
            files_per_sec: 12.0,
            mib_per_sec: 8.4,
            active_tasks: 4,
        });
        state.download_speed_history = VecDeque::from(vec![0.0, 4.2, f32::NAN, 8.4, 7.1]);

        let (j_run, _) = state.jobs.push(
            "Sodium".into(),
            format!("Инстанс «{}»", inst.name),
            3,
            Some(RetrySpec::Content {
                instance_id: inst.id.clone(),
                kind: ProjectKind::Mod,
                source: CatalogSource::Modrinth,
                downloads: Vec::new(),
                blocked: Vec::new(),
            }),
        );
        state.jobs.progress(j_run, 1, 3);
        state.jobs.set_stage(j_run, "Скачивание зависимостей");

        let (j_fail, _) =
            state
                .jobs
                .push("Iris".into(), "Инстанс «Survival 1.21.4»".into(), 1, None);
        state.jobs.finish(j_fail, Err("HTTP 503".into()));

        state.open_manual_dialog(
            &inst.id,
            vec![BlockedDownload {
                project_id: "123".into(),
                file_id: "456".into(),
                project_title: "Restricted Mod".into(),
                filename: "restricted-1.0.jar".into(),
                webpage_url: "https://www.curseforge.com/minecraft/mc-mods/test".into(),
                expected_size: 524_288,
                expected_sha1: None,
                expected_fingerprint: None,
                target_rel_path: "mods/restricted-1.0.jar".into(),
            }],
        );
        state.manual.open = false;

        state.record_download_history(
            "Fabric Loader",
            "Инстанс",
            "1.21.4 (fabric)",
            DownloadHistoryStatus::Completed,
            Some(inst.id.clone()),
        );
        state.record_download_history(
            "Broken Pack",
            "Сборка",
            "Сетевой таймаут",
            DownloadHistoryStatus::Failed,
            None,
        );
        state.record_download_history(
            "Canceled Mod",
            "Каталог",
            "Отменено пользователем",
            DownloadHistoryStatus::Cancelled,
            Some(inst.id),
        );

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let act = view.ui(ui, &mut state);
                assert_eq!(act, None);
            });
        });

        // 3. Remove manual item cleanly clears/updates watcher
        remove_pending_manual_item(&mut state, 0);
        assert!(state.manual.pending.is_empty());
        assert!(state.manual_watcher.is_none());
    }

    #[test]
    fn zero_emojis_in_downloads_view() {
        let src = include_str!("downloads_view.rs");
        let has_emoji = src.chars().any(|ch| {
            let cp = ch as u32;
            (0x1F300..=0x1FAFF).contains(&cp)
                || (0x2600..=0x27BF).contains(&cp)
                || (0xFE00..=0xFE0F).contains(&cp)
        });
        assert!(
            !has_emoji,
            "strict zero-emoji policy violated in downloads_view.rs"
        );
    }
}
