#![deny(clippy::await_holding_lock)]
//! Performance & Task telemetry view: per-core CPU meters from `/proc/stat`,
//! memory RSS monitoring, and UI Watchdog statistics. Zero emojis!

use egui::{ProgressBar, RichText, Ui, Vec2};

use crate::state::UiState;
use crate::theme::{ACCENT_EMERALD, STATUS_WARN, TEXT_MUTED, TEXT_PRIMARY};
use crate::widgets::{paint_vector_icon, VectorIcon};

#[derive(Default)]
pub struct PerfView;

impl PerfView {
    pub fn ui(&self, ui: &mut Ui, state: &UiState) {
        ui.heading(
            RichText::new("Производительность и Задачи")
                .color(TEXT_PRIMARY)
                .strong(),
        );
        ui.add_space(8.0);

        let tokens = crate::theme::Tokens::from_config(&state.config.theme);
        let style = crate::theme_styles::ThemeStyleParams::active(ui.ctx());
        let cr = style.card_corner_radius(state.config.theme.corner_radius);
        let card = crate::theme::solid_card_frame_with_config(&tokens, &state.config.theme)
            .inner_margin(egui::Margin::same(12));

        crate::theme_styles::show_themed_card(
            ui,
            card,
            cr,
            ui.make_persistent_id("perf_cpu_card_glass"),
            &style,
            &tokens,
            |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), egui::Sense::hover());
                    paint_vector_icon(ui.painter(), rect, VectorIcon::Cpu, ACCENT_EMERALD);
                    ui.label(RichText::new("Загрузка ядер процессора (/proc/stat)").strong());
                });

                ui.add_space(8.0);
                ui.label(format!(
                    "Общая загрузка CPU: {:.1}% | Память процесса: {:.1} МиБ",
                    state.latest_cpu.total_pct, state.latest_cpu.rss_mib
                ));

                ui.add_space(10.0);
                for core in &state.latest_cpu.per_core {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("Ядро {:02}:", core.core_id))
                                .color(TEXT_MUTED)
                                .monospace(),
                        );
                        let raw_frac =
                            crate::state::safe_clamp_f32(core.usage_pct / 100.0, 0.0, 1.0);
                        let frac = crate::animation::animate_spring_f32(
                            ui.ctx(),
                            egui::Id::new(("mq_perf_core_bar", core.core_id)),
                            raw_frac,
                            crate::animation::SpringConfig::smooth(),
                        )
                        .clamp(0.0, 1.0);
                        ui.add(
                            ProgressBar::new(frac)
                                .desired_width(260.0)
                                .text(format!("{:.1}%", core.usage_pct)),
                        );
                    });
                    ui.add_space(2.0);
                }

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                ui.label(RichText::new("Плавность интерфейса (> 8 мс)").strong());
                let violations = state.watchdog.violation_count();
                let col = if violations == 0 {
                    ACCENT_EMERALD
                } else {
                    STATUS_WARN
                };
                ui.label(
                    RichText::new(format!("Зафиксировано фризов кадра > 8 мс: {violations}"))
                        .color(col),
                );

                ui.add_space(6.0);
                ui.label(RichText::new(
                "MQ выполняет проверку файлов, распаковку сборок и сканирование модов в фоне, чтобы интерфейс оставался плавным."
            ).color(TEXT_MUTED).small());
            },
        );
    }
}
