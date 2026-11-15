#![deny(clippy::await_holding_lock)]
//! Catalog screen: browse mods, resource packs, shaders and modpacks from Modrinth and
//! CurseForge. Opaque matte style, theme tokens only, 4/8 spacing grid, no emoji.

use std::time::Instant;

use egui::{Color32, RichText, ScrollArea, Sense, Stroke, TextureOptions, Ui, Vec2};
use mq_core::ModLoaderKind;
use mq_mods::{CatalogHit, CatalogSource, CatalogSourceFilter, ProjectKind, SearchSort};

use crate::catalog_state::{
    CatalogErrorKind, CatalogState, CatalogTab, IconSlot, CATALOG_PAGE_SIZE,
};
use crate::components::registry::{register_widget, ElementKind, ElementMeta};
use crate::state::{UiState, UiWorkerCommand};
use crate::theme::{
    blend_color, solid_card_frame_with_config, Tokens, ACCENT_COBALT, ACCENT_EMERALD,
    RADIUS_BUTTON, RADIUS_CARD, RADIUS_TAG,
};
use crate::widgets::{
    paint_vector_icon, primary_emerald_button, secondary_glass_button, version_badge, VectorIcon,
};

/// Page where CurseForge API keys are issued.
pub const CURSEFORGE_KEYS_URL: &str = "https://console.curseforge.com/";
const ICON_SIZE: f32 = 48.0;
const SKELETON_ROWS: usize = 6;

const LOADERS: [ModLoaderKind; 4] = [
    ModLoaderKind::Fabric,
    ModLoaderKind::Quilt,
    ModLoaderKind::Forge,
    ModLoaderKind::NeoForge,
];

const SORTS: [(SearchSort, &str); 5] = [
    (SearchSort::Downloads, "По загрузкам"),
    (SearchSort::Relevance, "По релевантности"),
    (SearchSort::Follows, "По популярности"),
    (SearchSort::Newest, "Новые"),
    (SearchSort::Updated, "Обновлённые"),
];

/// Category slugs offered as filter chips per tab.
pub const fn categories_for(tab: CatalogTab) -> &'static [(&'static str, &'static str)] {
    match tab {
        CatalogTab::Mods => &[
            ("optimization", "Оптимизация"),
            ("technology", "Технологии"),
            ("magic", "Магия"),
            ("adventure", "Приключения"),
            ("utility", "Утилиты"),
            ("library", "Библиотеки"),
        ],
        CatalogTab::ResourcePacks => &[
            ("realistic", "Реализм"),
            ("simplistic", "Минимализм"),
            ("vanilla-like", "Ванильные"),
        ],
        CatalogTab::Shaders => &[
            ("realistic", "Реализм"),
            ("fantasy", "Фэнтези"),
            ("vanilla-like", "Ванильные"),
        ],
        CatalogTab::Modpacks => &[
            ("optimization", "Оптимизация"),
            ("technology", "Технологии"),
            ("magic", "Магия"),
            ("adventure", "Приключения"),
        ],
    }
}

/// Human-readable text for a source error.
pub fn source_error_text(source: CatalogSource, kind: CatalogErrorKind) -> String {
    let name = source_name(source);
    match kind {
        CatalogErrorKind::Network => {
            format!("{name}: нет связи с сервером. Проверьте подключение к сети и повторите.")
        }
        CatalogErrorKind::RateLimited => {
            format!("{name}: слишком много запросов (429). Подождите немного и повторите.")
        }
        CatalogErrorKind::MissingKey => {
            format!("{name}: нужен действующий API-ключ.")
        }
        CatalogErrorKind::Other => format!("{name}: не удалось получить результаты."),
    }
}

pub const fn source_name(source: CatalogSource) -> &'static str {
    match source {
        CatalogSource::Modrinth => "Modrinth",
        CatalogSource::CurseForge => "CurseForge",
    }
}

pub const fn source_color(source: CatalogSource) -> Color32 {
    match source {
        CatalogSource::Modrinth => ACCENT_EMERALD,
        CatalogSource::CurseForge => Color32::from_rgb(0xF1, 0x64, 0x36),
    }
}

/// Formats a download counter compactly (`1.2 млн`, `34 тыс.`).
pub fn format_downloads(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1} млн", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{} тыс.", n / 1_000)
    } else {
        n.to_string()
    }
}

#[derive(Default)]
pub struct CatalogView {
    pub curseforge_api_key_input: String,
    import_path: String,
    textures: std::collections::HashMap<String, egui::TextureHandle>,
}

impl CatalogView {
    pub fn ui(&mut self, ui: &mut Ui, state: &mut UiState) {
        let now = Instant::now();
        let tokens = Tokens::from_config(&state.config.theme);
        let mut changed = false;

        ui.horizontal(|ui| {
            ui.heading(RichText::new("Каталог").color(tokens.text).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.instance_target_selector(ui, state, &tokens);
            });
        });
        ui.add_space(8.0);

        changed |= self.tabs_row(ui, &mut state.catalog, &tokens);
        ui.add_space(8.0);
        changed |= self.source_row(ui, &mut state.catalog, &tokens);
        ui.add_space(8.0);
        self.import_row(ui, state, &tokens);

        self.render_catalog_body(ui, state, &tokens, now, changed, false);
        crate::catalog_dialogs::show_overlays(ui.ctx(), state);
    }

    /// Renders the catalog browser embedded directly inside an instance's content tab
    /// (`Mods`, `ResourcePacks`, or `Shaders`), locked to that target instance.
    pub fn ui_for_instance_tab(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        inst_id: &mq_core::InstanceId,
        tab: CatalogTab,
    ) {
        let now = Instant::now();
        let tokens = Tokens::from_config(&state.config.theme);
        let mut changed = false;

        state.selected_instance_id = Some(inst_id.clone());
        let inst_info = state
            .instances
            .iter()
            .find(|i| &i.id == inst_id)
            .map(|i| (i.name.clone(), i.mc_version.clone(), i.loader));

        if state.catalog.tab != tab {
            state.catalog.tab = tab;
            state.catalog.filters.categories.clear();
            changed = true;
        }

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
            if let Some((inst_name, inst_mc, inst_loader)) = inst_info {
                ui.label(
                    RichText::new(format!(
                        "{} ({} · {})",
                        inst_name,
                        inst_mc,
                        inst_loader.as_str()
                    ))
                    .color(tokens.text_muted)
                    .small(),
                );
                if secondary_glass_button(ui, Some(VectorIcon::Refresh), "Фильтр под инстанс")
                    .clicked()
                {
                    state
                        .catalog
                        .sync_for_instance(tab, &inst_mc, inst_loader, now);
                }
                ui.separator();
            }
            changed |= self.source_items(ui, &mut state.catalog, &tokens);
        });

        self.render_catalog_body(ui, state, &tokens, now, changed, true);
    }

    fn render_catalog_body(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        tokens: &Tokens,
        now: Instant,
        mut changed: bool,
        compact: bool,
    ) {
        let gap = if compact { 4.0 } else { 8.0 };
        let has_key = state.config.effective_cf_api_key().is_some();
        let cf_selected = state.catalog.source == CatalogSourceFilter::CurseForge;
        if !has_key && cf_selected {
            ui.add_space(gap);
            self.key_panel(ui, state, tokens);
        }

        ui.add_space(gap);
        changed |= self.search_row(ui, &mut state.catalog, tokens);
        ui.add_space(gap);
        changed |= self.filters_row(ui, &mut state.catalog, tokens, compact);
        ui.add_space(gap);

        self.batch_queue_bar(ui, state, tokens);

        if changed {
            state.catalog.mark_dirty(now);
        }
        self.drive_search(ui, state, now);

        if state.catalog.source == CatalogSourceFilter::All && !has_key {
            self.inline_notice(
                ui,
                state,
                tokens,
                "CurseForge отключён: не задан API-ключ. Показаны результаты Modrinth.",
            );
        }
        for err in state.catalog.errors.clone() {
            if err.kind == CatalogErrorKind::MissingKey
                && err.source == CatalogSource::CurseForge
                && !has_key
            {
                continue;
            }
            let mut text = source_error_text(err.source, err.kind);
            if err.kind == CatalogErrorKind::Other && !err.message.is_empty() {
                text.push(' ');
                text.push_str(&err.message);
            }
            self.inline_notice(ui, state, tokens, &text);
        }

        crate::catalog_dialogs::jobs_panel(ui, state);
        ui.separator();
        self.results(ui, state, tokens);
    }

    fn instance_target_selector(&self, ui: &mut Ui, state: &mut UiState, tokens: &Tokens) {
        if state.instances.is_empty() {
            ui.label(
                RichText::new("Нет инстансов для установки")
                    .color(tokens.text_muted)
                    .small(),
            );
            return;
        }
        let current_id = state.selected_instance_id.clone();
        let current_label = state
            .selected_instance()
            .map(|i| format!("{} ({} · {})", i.name, i.mc_version, i.loader.as_str()))
            .unwrap_or_else(|| "Выберите инстанс".to_string());
        let mut next_id = None;
        egui::ComboBox::from_id_salt("catalog_target_instance")
            .selected_text(current_label)
            .show_ui(ui, |ui| {
                for inst in &state.instances {
                    let selected = current_id.as_ref() == Some(&inst.id);
                    let label = format!(
                        "{} ({} · {})",
                        inst.name,
                        inst.mc_version,
                        inst.loader.as_str()
                    );
                    if ui.selectable_label(selected, label).clicked() && !selected {
                        next_id = Some(inst.id.clone());
                    }
                }
            });
        ui.label(RichText::new("Инстанс:").color(tokens.text_muted).small());
        if let Some(id) = next_id {
            state.selected_instance_id = Some(id.clone());
            state.installed_mods.clear();
            let _ = state
                .cmd_sender
                .send(UiWorkerCommand::ScanInstanceMods { instance_id: id });
        }
    }

    fn batch_queue_bar(&self, ui: &mut Ui, state: &mut UiState, tokens: &Tokens) {
        if state.catalog.batch_queue.is_empty() {
            return;
        }
        let count = state.catalog.batch_queue.len();
        let mut remove_item: Option<(CatalogSource, String)> = None;
        let mut open_batch = false;
        let mut clear_all = false;

        solid_card_frame_with_config(tokens, &state.config.theme)
            .stroke(Stroke::new(
                1.0_f32,
                blend_color(tokens.border_strong, tokens.accent, 0.38),
            ))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Очередь пакетной установки: {count}"))
                            .color(tokens.text)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                        if primary_emerald_button(
                            ui,
                            Some(VectorIcon::Plus),
                            &format!("Установить выбранные ({count}) с зависимостями"),
                        )
                        .clicked()
                        {
                            open_batch = true;
                        }
                        if secondary_glass_button(ui, None, "Очистить").clicked() {
                            clear_all = true;
                        }
                    });
                });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                    for entry in &state.catalog.batch_queue {
                        let chip_bg = blend_color(tokens.bg_surface_hover, tokens.accent, 0.14);
                        let chip_btn = egui::Button::new(
                            RichText::new(format!("{}  [x]", entry.title))
                                .color(tokens.text)
                                .small(),
                        )
                        .fill(chip_bg)
                        .stroke(Stroke::new(1.0_f32, tokens.border))
                        .corner_radius(RADIUS_TAG);
                        if ui
                            .add(chip_btn)
                            .on_hover_text("Нажмите, чтобы убрать из очереди")
                            .clicked()
                        {
                            remove_item = Some((entry.source, entry.project_id.clone()));
                        }
                    }
                });
            });
        ui.add_space(8.0);

        if let Some((src, pid)) = remove_item {
            state.catalog.remove_queued([(src, pid.as_str())]);
        }
        if clear_all {
            state.catalog.clear_queue();
        }
        if open_batch {
            state.open_batch_install();
        }
    }

    fn tabs_row(&self, ui: &mut Ui, cat: &mut CatalogState, tokens: &Tokens) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            for tab in CatalogTab::ALL {
                let selected = cat.tab == tab;
                let text = RichText::new(tab.label()).color(if selected {
                    tokens.text
                } else {
                    tokens.text_muted
                });
                let btn = egui::Button::new(if selected { text.strong() } else { text })
                    .fill(if selected {
                        tokens.bg_surface_hover
                    } else {
                        tokens.bg_surface
                    })
                    .stroke(Stroke::new(
                        1.0_f32,
                        if selected {
                            tokens.accent
                        } else {
                            tokens.border
                        },
                    ))
                    .corner_radius(RADIUS_BUTTON)
                    .min_size(Vec2::new(96.0, 32.0));
                let resp = ui.add(btn);
                register_widget(
                    &resp,
                    ElementMeta::new(ElementKind::Tab, tab.label()).interactive(true),
                );
                if resp.clicked() && !selected {
                    cat.tab = tab;
                    cat.filters.categories.clear();
                    changed = true;
                }
            }
        });
        changed
    }

    /// Local import: path field plus drag-and-drop of `.mrpack` / `.zip` onto the window.
    fn import_row(&mut self, ui: &mut Ui, state: &mut UiState, tokens: &Tokens) {
        let dropped: Vec<std::path::PathBuf> = ui.ctx().input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if let Some(p) = dropped.into_iter().next() {
            state.import_local_pack(p);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            ui.label(RichText::new("Импорт").color(tokens.text_muted));
            ui.add(
                egui::TextEdit::singleline(&mut self.import_path)
                    .hint_text("Путь к .mrpack / .zip или перетащите файл в окно")
                    .desired_width(360.0),
            );
            let can = !self.import_path.trim().is_empty();
            if secondary_glass_button(ui, None, "Импортировать").clicked() && can {
                let p = std::path::PathBuf::from(self.import_path.trim());
                self.import_path.clear();
                state.import_local_pack(p);
            }
        });
    }

    fn source_items(&self, ui: &mut Ui, cat: &mut CatalogState, tokens: &Tokens) -> bool {
        let mut changed = false;
        ui.label(RichText::new("Источник").color(tokens.text_muted));
        for (filter, label) in [
            (CatalogSourceFilter::All, "Все"),
            (CatalogSourceFilter::Modrinth, "Modrinth"),
            (CatalogSourceFilter::CurseForge, "CurseForge"),
        ] {
            let resp = ui.selectable_label(cat.source == filter, label);
            register_widget(
                &resp,
                ElementMeta::new(ElementKind::Button, label).interactive(true),
            );
            if resp.clicked() && cat.source != filter {
                cat.source = filter;
                changed = true;
            }
        }
        changed
    }

    fn source_row(&self, ui: &mut Ui, cat: &mut CatalogState, tokens: &Tokens) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            changed |= self.source_items(ui, cat, tokens);
        });
        changed
    }

    fn search_row(&self, ui: &mut Ui, cat: &mut CatalogState, tokens: &Tokens) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
            paint_vector_icon(ui.painter(), rect, VectorIcon::Search, ACCENT_COBALT);
            let resp = ui.add(
                egui::TextEdit::singleline(&mut cat.query)
                    .hint_text("Поиск по названию")
                    .desired_width((ui.available_width() - 8.0).max(160.0)),
            );
            register_widget(
                ui,
                &resp,
                ElementMeta::new(ElementKind::Input)
                    .label("Поиск по названию")
                    .text(cat.query.as_str())
                    .fg(tokens.text)
                    .bg(tokens.bg_input),
            );
            changed |= resp.changed();
        });
        changed
    }

    fn filters_row(
        &self,
        ui: &mut Ui,
        cat: &mut CatalogState,
        tokens: &Tokens,
        compact: bool,
    ) -> bool {
        let mut changed = false;
        let row_spacing = if compact {
            Vec2::new(6.0, 4.0)
        } else {
            Vec2::new(8.0, 8.0)
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = row_spacing;
            ui.label(RichText::new("Версия MC").color(tokens.text_muted));
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut cat.filters.mc_version)
                        .hint_text("любая")
                        .desired_width(72.0),
                )
                .changed();

            if matches!(cat.tab, CatalogTab::Mods | CatalogTab::Modpacks) {
                ui.label(RichText::new("Загрузчик").color(tokens.text_muted));
                egui::ComboBox::from_id_salt("cat_loader_combo")
                    .selected_text(cat.filters.loader.map_or("Любой", |l| l.as_str()))
                    .show_ui(ui, |ui| {
                        changed |= ui
                            .selectable_value(&mut cat.filters.loader, None, "Любой")
                            .changed();
                        for l in LOADERS {
                            changed |= ui
                                .selectable_value(&mut cat.filters.loader, Some(l), l.as_str())
                                .changed();
                        }
                    });
            }

            ui.label(RichText::new("Сортировка").color(tokens.text_muted));
            let current = SORTS
                .iter()
                .find(|(s, _)| *s == cat.filters.sort)
                .map_or("По загрузкам", |(_, l)| *l);
            egui::ComboBox::from_id_salt("cat_sort_combo")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (sort, label) in SORTS {
                        changed |= ui
                            .selectable_value(&mut cat.filters.sort, sort, label)
                            .changed();
                    }
                });
        });

        ui.add_space(if compact { 2.0 } else { 4.0 });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = row_spacing;
            ui.label(RichText::new("Категории").color(tokens.text_muted));
            for (slug, label) in categories_for(cat.tab) {
                let on = cat.filters.categories.iter().any(|c| c == slug);
                if ui.selectable_label(on, *label).clicked() {
                    if on {
                        cat.filters.categories.retain(|c| c != slug);
                    } else {
                        cat.filters.categories.push((*slug).to_string());
                    }
                    changed = true;
                }
            }
        });
        changed
    }

    /// Fires the debounced search and schedules the repaint for the remaining wait.
    fn drive_search(&self, ui: &Ui, state: &mut UiState, now: Instant) {
        if state.catalog.is_due(now) {
            let (request_id, query) = state.catalog.begin_request();
            let _ = state.cmd_sender.send(UiWorkerCommand::SearchCatalog {
                request_id,
                latest: state.catalog.latest_request.clone(),
                source: state.catalog.source,
                query,
            });
        } else if let Some(left) = state.catalog.debounce_remaining(now) {
            ui.ctx().request_repaint_after(left);
        } else if !state.catalog.searched && !state.catalog.loading {
            // First open of the screen: show popular projects without waiting for typing.
            state.catalog.mark_page_changed(now, false);
            ui.ctx().request_repaint();
        }
    }

    fn inline_notice(&self, ui: &mut Ui, state: &UiState, tokens: &Tokens, text: &str) {
        solid_card_frame_with_config(tokens, &state.config.theme)
            .stroke(Stroke::new(1.0_f32, tokens.warning))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(text).color(tokens.warning));
            });
        ui.add_space(4.0);
    }

    /// Panel shown when CurseForge is selected without a key: instruction, link and input.
    fn key_panel(&mut self, ui: &mut Ui, state: &mut UiState, tokens: &Tokens) {
        solid_card_frame_with_config(tokens, &state.config.theme)
            .stroke(Stroke::new(1.0_f32, tokens.warning))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("Для CurseForge нужен API-ключ")
                        .color(tokens.text)
                        .strong(),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "1. Откройте страницу ключей и войдите в аккаунт.\n\
                         2. Создайте ключ и вставьте его ниже.\n\
                         Ключ можно также задать переменной окружения MQ_CURSEFORGE_API_KEY.\n\
                         Modrinth работает без ключа.",
                    )
                    .color(tokens.text_muted),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if secondary_glass_button(ui, None, "Открыть страницу ключей").clicked()
                    {
                        let _ = crate::opener::open_that(CURSEFORGE_KEYS_URL);
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.curseforge_api_key_input)
                            .password(true)
                            .hint_text("API-ключ CurseForge")
                            .desired_width(280.0),
                    );
                    let can_save = !self.curseforge_api_key_input.trim().is_empty();
                    if primary_emerald_button(ui, None, "Сохранить ключ").clicked() && can_save
                    {
                        state.config.curseforge_api_key = Some(mq_core::SecretString::new(
                            self.curseforge_api_key_input.trim(),
                        ));
                        let _ = state.config.save(&state.paths);
                        self.curseforge_api_key_input.clear();
                        state.catalog.mark_page_changed(Instant::now(), false);
                    }
                });
            });
    }

    fn results(&mut self, ui: &mut Ui, state: &mut UiState, tokens: &Tokens) {
        let has_key = state.config.effective_cf_api_key().is_some();
        let mut go_prev = false;
        let mut go_next = false;

        ScrollArea::vertical()
            .id_salt("catalog_results_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let cat = &state.catalog;
                if cat.loading && cat.hits.is_empty() {
                    for _ in 0..SKELETON_ROWS {
                        skeleton_card(ui);
                        ui.add_space(8.0);
                    }
                    return;
                }
                let key_missing = cat.source == CatalogSourceFilter::CurseForge && !has_key;
                if cat.hits.is_empty() && key_missing {
                    // The key panel above already explains what to do.
                } else if cat.hits.is_empty() {
                    if cat.searched {
                        if cat.all_sources_failed() {
                            empty_state(
                                ui,
                                tokens,
                                "Не удалось загрузить каталог",
                                "Проверьте сообщение выше и повторите поиск.",
                            );
                        } else {
                            empty_state(
                                ui,
                                tokens,
                                "Ничего не найдено",
                                "Измените запрос или сбросьте фильтры.",
                            );
                        }
                    }
                    return;
                }

                let hits = cat.hits.clone();
                let urls = state.catalog.icons_to_fetch(hits.iter());
                for url in urls {
                    let _ = state.cmd_sender.send(UiWorkerCommand::FetchIcon {
                        url,
                        edge: crate::catalog_worker::ICON_EDGE,
                    });
                }
                for hit in &hits {
                    self.hit_card(ui, state, hit, tokens);
                    ui.add_space(8.0);
                }

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                    let page = state.catalog.page;
                    ui.add_enabled_ui(page > 0 && !state.catalog.loading, |ui| {
                        if secondary_glass_button(ui, None, "Назад").clicked() {
                            go_prev = true;
                        }
                    });
                    ui.label(
                        RichText::new(format!("Страница {}", page + 1)).color(tokens.text_muted),
                    );
                    ui.add_enabled_ui(state.catalog.has_next && !state.catalog.loading, |ui| {
                        if secondary_glass_button(ui, None, "Вперёд").clicked() {
                            go_next = true;
                        }
                    });
                    if state.catalog.loading {
                        ui.label(RichText::new("Загрузка").color(tokens.text_muted));
                    }
                    ui.label(
                        RichText::new(format!("до {CATALOG_PAGE_SIZE} на источник"))
                            .color(tokens.text_muted)
                            .small(),
                    );
                });
            });

        if go_prev && state.catalog.page > 0 {
            state.catalog.page -= 1;
            state.catalog.mark_page_changed(Instant::now(), false);
        }
        if go_next {
            state.catalog.page += 1;
            state.catalog.mark_page_changed(Instant::now(), false);
        }
    }

    /// Returns the GPU texture for an icon URL, uploading a freshly downloaded image first.
    fn icon_texture(
        &mut self,
        ui: &Ui,
        cat: &mut CatalogState,
        url: &str,
    ) -> Option<egui::TextureHandle> {
        if let Some(IconSlot::Ready(_)) = cat.icons.get(url) {
            if let Some(IconSlot::Ready(img)) =
                cat.icons.insert(url.to_string(), IconSlot::Uploaded)
            {
                let tex = ui.ctx().load_texture(
                    format!("catalog-icon:{url}"),
                    img,
                    TextureOptions::LINEAR,
                );
                self.textures.insert(url.to_string(), tex);
            }
        }
        self.textures.get(url).cloned()
    }

    fn hit_card(&mut self, ui: &mut Ui, state: &mut UiState, hit: &CatalogHit, tokens: &Tokens) {
        let installed = is_hit_installed_in_instance(state, hit);
        let queued = state.catalog.is_queued(hit.source, &hit.project_id);
        let texture = hit
            .icon_url
            .as_deref()
            .and_then(|u| self.icon_texture(ui, &mut state.catalog, u));

        let mut frame = solid_card_frame_with_config(tokens, &state.config.theme)
            .inner_margin(egui::Margin::same(12));
        if queued {
            frame = frame.stroke(Stroke::new(1.2_f32, tokens.accent));
        }
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(12.0, 4.0);
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(ICON_SIZE), Sense::hover());
                match &texture {
                    Some(tex) => {
                        egui::Image::new(tex)
                            .corner_radius(RADIUS_BUTTON)
                            .fit_to_exact_size(Vec2::splat(ICON_SIZE))
                            .paint_at(ui, rect);
                    }
                    None => {
                        ui.painter()
                            .rect_filled(rect, RADIUS_BUTTON, tokens.bg_input);
                        let icon = match hit.project_kind {
                            ProjectKind::Mod => VectorIcon::Layers,
                            ProjectKind::Modpack => VectorIcon::Catalog,
                            ProjectKind::Shader => VectorIcon::Shield,
                            ProjectKind::ResourcePack => VectorIcon::Folder,
                        };
                        paint_vector_icon(ui.painter(), rect.shrink(12.0), icon, tokens.text_muted);
                    }
                }

                let avail = ui.available_width();
                let stack_actions =
                    !installed && hit.project_kind != ProjectKind::Modpack && avail < 560.0;
                let right_w = if hit.project_kind == ProjectKind::Modpack {
                    216.0
                } else if installed {
                    224.0
                } else if stack_actions {
                    196.0
                } else {
                    336.0
                };
                let mid_w = (avail - right_w - 12.0).max(100.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(mid_w, ICON_SIZE),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&hit.title).color(tokens.text).strong(),
                                )
                                .truncate(),
                            );
                        });
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!(
                                    "{}  |  {} загрузок",
                                    hit.author,
                                    format_downloads(hit.downloads)
                                ))
                                .color(tokens.text_muted)
                                .small(),
                            )
                            .truncate(),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(&hit.description)
                                    .color(tokens.text_muted)
                                    .small(),
                            )
                            .truncate(),
                        );
                    },
                );

                if stack_actions {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                        self.install_or_hint_button(ui, state, hit, tokens);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                            self.queue_toggle_button(ui, state, hit, queued, tokens);
                            if secondary_glass_button(ui, None, "Подробнее").clicked() {
                                state.open_project(hit.clone());
                            }
                        });
                    });
                } else {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                        self.card_actions(ui, state, hit, installed, queued, tokens);
                    });
                }
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
                version_badge(ui, source_name(hit.source), source_color(hit.source));
                for l in hit.loaders.iter().take(4) {
                    version_badge(ui, l, ACCENT_COBALT);
                }
                if installed {
                    version_badge(ui, "Установлен", tokens.accent);
                }
                if queued {
                    version_badge(ui, "В очереди", tokens.accent);
                }
            });
        });
    }

    fn install_or_hint_button(
        &self,
        ui: &mut Ui,
        state: &mut UiState,
        hit: &CatalogHit,
        tokens: &Tokens,
    ) {
        if state.selected_instance().is_some() {
            if primary_emerald_button(ui, Some(VectorIcon::Plus), "Установить").clicked()
            {
                state.open_install_content(hit.clone());
            }
        } else {
            ui.label(
                RichText::new("Выберите инстанс")
                    .color(tokens.text_muted)
                    .small(),
            );
        }
    }

    fn queue_toggle_button(
        &self,
        ui: &mut Ui,
        state: &mut UiState,
        hit: &CatalogHit,
        queued: bool,
        tokens: &Tokens,
    ) {
        let queue_label = if queued {
            "В очереди"
        } else {
            "+ В очередь"
        };
        let queue_btn = egui::Button::new(
            RichText::new(queue_label)
                .color(if queued { tokens.accent } else { tokens.text })
                .small(),
        )
        .fill(if queued {
            blend_color(tokens.bg_surface_hover, tokens.accent, 0.16)
        } else {
            tokens.bg_surface
        })
        .stroke(Stroke::new(
            1.0_f32,
            if queued { tokens.accent } else { tokens.border },
        ))
        .corner_radius(RADIUS_BUTTON);
        if ui.add(queue_btn).clicked() {
            state.catalog.toggle_queue(hit.clone());
        }
    }

    fn card_actions(
        &self,
        ui: &mut Ui,
        state: &mut UiState,
        hit: &CatalogHit,
        installed: bool,
        queued: bool,
        tokens: &Tokens,
    ) {
        if hit.project_kind == ProjectKind::Modpack {
            if primary_emerald_button(ui, Some(VectorIcon::Plus), "Создать сборку").clicked()
            {
                state.open_project(hit.clone());
            }
        } else if installed {
            version_badge(ui, "Уже добавлен", tokens.accent);
        } else {
            self.install_or_hint_button(ui, state, hit, tokens);
            self.queue_toggle_button(ui, state, hit, queued, tokens);
        }
        if secondary_glass_button(ui, None, "Подробнее").clicked() {
            state.open_project(hit.clone());
        }
    }
}

/// Placeholder card (matte rectangles, no spinner) shown while the first page loads.
pub fn skeleton_card(ui: &mut Ui) {
    let tokens = Tokens::active(ui.ctx());
    egui::Frame::new()
        .fill(tokens.bg_surface)
        .stroke(Stroke::new(1.0_f32, tokens.border))
        .corner_radius(RADIUS_CARD)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), ICON_SIZE), Sense::hover());
            let p = ui.painter();
            let icon = egui::Rect::from_min_size(rect.min, Vec2::splat(ICON_SIZE));
            p.rect_filled(icon, RADIUS_BUTTON, tokens.bg_input);
            let x = icon.max.x + 12.0;
            let w = (rect.max.x - x).max(40.0);
            for (dy, frac, h) in [(4.0, 0.35, 12.0), (22.0, 0.55, 8.0), (36.0, 0.8, 8.0)] {
                let r = egui::Rect::from_min_size(
                    egui::pos2(x, rect.min.y + dy),
                    Vec2::new(w * frac, h),
                );
                p.rect_filled(r, 4.0, tokens.bg_input);
            }
        });
}

fn norm_alnum(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn strip_content_extensions(file_name: &str) -> String {
    let mut s = file_name.trim().to_ascii_lowercase();
    for ext in [".disabled", ".jar", ".zip"] {
        if let Some(stripped) = s.strip_suffix(ext) {
            s = stripped.to_owned();
        }
    }
    s
}

fn tokenize_ident(s: &str) -> Vec<&str> {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect()
}

fn is_loader_tag(tok: &str) -> bool {
    matches!(
        tok,
        "fabric"
            | "quilt"
            | "forge"
            | "neoforge"
            | "universal"
            | "common"
            | "client"
            | "server"
            | "mod"
            | "all"
    )
}

fn is_version_or_build_token(tok: &str) -> bool {
    let bytes = tok.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    if bytes[0].is_ascii_digit() {
        return true;
    }
    if (bytes[0] == b'v' || bytes[0] == b'r') && bytes.get(1).is_some_and(u8::is_ascii_digit) {
        return true;
    }
    if tok.starts_with("mc") && bytes.get(2).is_some_and(u8::is_ascii_digit) {
        return true;
    }
    tok == "mc"
}

fn rest_starts_version_or_loader(rest: &[&str]) -> bool {
    if rest.is_empty() {
        return true;
    }
    let first = rest[0];
    if is_version_or_build_token(first) {
        return true;
    }
    if is_loader_tag(first) {
        return rest.len() == 1 || rest_starts_version_or_loader(&rest[1..]);
    }
    false
}

/// Checks whether a mod/pack/shader file name matches a catalog project's `slug` or `title`
/// without false-positive substring collisions (e.g. `"fabric"` or `"map"` inside `"journeymap-..."`).
pub(crate) fn filename_matches_project(file_name: &str, slug: &str, title: &str) -> bool {
    let stem = strip_content_extensions(file_name);
    if stem.is_empty() {
        return false;
    }
    let stem_tokens = tokenize_ident(&stem);
    let norm_stem = norm_alnum(&stem);

    for raw in [slug, title] {
        let cand = raw.trim().to_ascii_lowercase();
        if cand.is_empty() {
            continue;
        }
        if stem == cand {
            return true;
        }
        let cand_tokens = tokenize_ident(&cand);
        if !cand_tokens.is_empty()
            && stem_tokens.len() >= cand_tokens.len()
            && stem_tokens[..cand_tokens.len()] == cand_tokens[..]
            && rest_starts_version_or_loader(&stem_tokens[cand_tokens.len()..])
        {
            return true;
        }
        let norm_cand = norm_alnum(&cand);
        if norm_cand.len() >= 3 && norm_stem == norm_cand {
            return true;
        }
        if norm_cand.len() >= 5 {
            if let Some(suffix) = norm_stem.strip_prefix(&norm_cand) {
                if is_version_or_build_token(suffix) {
                    return true;
                }
            }
        }
    }
    false
}

fn mod_entry_matches_project(m: &mq_mods::InstalledModEntry, slug: &str, title: &str) -> bool {
    let norm_slug = norm_alnum(slug);
    let norm_title = norm_alnum(title);
    let mod_id = m.mod_id.trim();
    let mod_name = m.name.trim();

    if !slug.is_empty() && mod_id.eq_ignore_ascii_case(slug) {
        return true;
    }
    let norm_id = norm_alnum(mod_id);
    if !norm_id.is_empty()
        && ((!norm_slug.is_empty() && norm_id == norm_slug)
            || (!norm_title.is_empty() && norm_id == norm_title))
    {
        return true;
    }
    if (!title.is_empty() && mod_name.eq_ignore_ascii_case(title))
        || (!slug.is_empty() && mod_name.eq_ignore_ascii_case(slug))
    {
        return true;
    }
    filename_matches_project(&m.file_name, slug, title)
}

fn dir_has_matching_pack(dir: &std::path::Path, slug: &str, title: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name_lossy = name.to_string_lossy();
        filename_matches_project(&name_lossy, slug, title)
    })
}

fn is_hit_installed_in_instance(state: &UiState, hit: &CatalogHit) -> bool {
    let slug = hit.slug.trim().to_lowercase();
    let title = hit.title.trim().to_lowercase();
    if slug.is_empty() && title.is_empty() {
        return false;
    }

    match hit.project_kind {
        ProjectKind::Mod => state
            .installed_mods
            .iter()
            .any(|m| mod_entry_matches_project(m, &slug, &title)),
        ProjectKind::ResourcePack => state.selected_instance_id.as_ref().is_some_and(|id| {
            let dir = state.paths.instance_dir(id.as_str()).join("resourcepacks");
            dir_has_matching_pack(&dir, &slug, &title)
        }),
        ProjectKind::Shader => state.selected_instance_id.as_ref().is_some_and(|id| {
            let dir = state.paths.instance_dir(id.as_str()).join("shaderpacks");
            dir_has_matching_pack(&dir, &slug, &title)
        }),
        ProjectKind::Modpack => false,
    }
}

fn empty_state(ui: &mut Ui, tokens: &Tokens, title: &str, hint: &str) {
    ui.add_space(32.0);
    ui.vertical_centered(|ui| {
        egui::Frame::new()
            .fill(tokens.bg_surface)
            .stroke(Stroke::new(1.0_f32, tokens.border_strong))
            .corner_radius(RADIUS_CARD)
            .inner_margin(egui::Margin::same(24))
            .show(ui, |ui| {
                ui.label(RichText::new(title).color(tokens.text).strong());
                ui.add_space(4.0);
                ui.label(RichText::new(hint).color(tokens.text_muted));
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_are_formatted_compactly() {
        assert_eq!(format_downloads(999), "999");
        assert_eq!(format_downloads(34_820), "34 тыс.");
        assert_eq!(format_downloads(34_820_000), "34.8 млн");
    }

    #[test]
    fn error_texts_cover_all_kinds_without_emoji() {
        for kind in [
            CatalogErrorKind::Network,
            CatalogErrorKind::RateLimited,
            CatalogErrorKind::MissingKey,
            CatalogErrorKind::Other,
        ] {
            let t = source_error_text(CatalogSource::CurseForge, kind);
            assert!(t.starts_with("CurseForge:"));
            assert!(t.chars().all(|c| (c as u32) < 0x1F000), "no emoji: {t}");
        }
        assert!(
            source_error_text(CatalogSource::Modrinth, CatalogErrorKind::RateLimited)
                .contains("429")
        );
    }

    #[test]
    fn every_tab_has_categories_and_maps_back() {
        for tab in CatalogTab::ALL {
            assert!(!categories_for(tab).is_empty());
            assert_eq!(CatalogTab::from_kind(tab.kind()), tab);
        }
    }

    #[test]
    fn keys_page_is_official_console() {
        assert_eq!(CURSEFORGE_KEYS_URL, "https://console.curseforge.com/");
    }

    #[test]
    fn is_hit_installed_checks_mods_resourcepacks_and_shaders() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = mq_core::MqPaths::with_root(tmp.path());
        let inst_id = mq_core::InstanceId::new("qa-inst").unwrap();
        let inst_dir = paths.instance_dir(inst_id.as_str());
        std::fs::create_dir_all(inst_dir.join("resourcepacks")).unwrap();
        std::fs::create_dir_all(inst_dir.join("shaderpacks")).unwrap();
        std::fs::write(inst_dir.join("resourcepacks/Faithful-32x.zip"), b"zip").unwrap();
        std::fs::write(
            inst_dir.join("shaderpacks/BSL_v8.2.09.zip.disabled"),
            b"zip",
        )
        .unwrap();

        let pool = std::sync::Arc::new(mq_sched::ComputePool::new(1));
        let mut state = UiState::new(paths, mq_core::AppConfig::default(), pool).unwrap();
        state.selected_instance_id = Some(inst_id);
        state.installed_mods.push(mq_mods::InstalledModEntry {
            file_name: "sodium-fabric-0.5.11.jar".into(),
            path: std::path::PathBuf::from("mods/sodium-fabric-0.5.11.jar"),
            enabled: true,
            mod_id: "sodium".into(),
            name: "Sodium".into(),
            version: "0.5.11".into(),
            authors: vec![],
            description: None,
            loader: Some(ModLoaderKind::Fabric),
            dependencies: vec![],
            sha1: mq_core::Sha1Digest::from_bytes([0u8; 20]),
            sha512: mq_core::Sha512Digest::from_bytes([0u8; 64]),
            size: 100,
        });

        let make_hit = |kind: ProjectKind, slug: &str, title: &str| CatalogHit {
            source: CatalogSource::Modrinth,
            project_id: slug.into(),
            slug: slug.into(),
            project_kind: kind,
            title: title.into(),
            description: String::new(),
            author: String::new(),
            downloads: 0,
            icon_url: None,
            loaders: vec![],
            game_versions: vec![],
            latest_version: None,
        };

        assert!(is_hit_installed_in_instance(
            &state,
            &make_hit(ProjectKind::Mod, "sodium", "Sodium")
        ));
        assert!(is_hit_installed_in_instance(
            &state,
            &make_hit(ProjectKind::ResourcePack, "faithful-32x", "Faithful 32x")
        ));
        assert!(!is_hit_installed_in_instance(
            &state,
            &make_hit(ProjectKind::Shader, "sodium", "Sodium")
        ));
        assert!(is_hit_installed_in_instance(
            &state,
            &make_hit(ProjectKind::Shader, "bsl-shaders", "BSL")
        ));
        assert!(!is_hit_installed_in_instance(
            &state,
            &make_hit(ProjectKind::Modpack, "sodium", "Sodium")
        ));

        state.installed_mods.push(mq_mods::InstalledModEntry {
            file_name: "journeymap-fabric-26.3-6.0.10.jar".into(),
            path: std::path::PathBuf::from("mods/journeymap-fabric-26.3-6.0.10.jar"),
            enabled: true,
            mod_id: "journeymap".into(),
            name: "JourneyMap".into(),
            version: "6.0.10".into(),
            authors: vec![],
            description: None,
            loader: Some(ModLoaderKind::Fabric),
            dependencies: vec![],
            sha1: mq_core::Sha1Digest::from_bytes([0u8; 20]),
            sha512: mq_core::Sha512Digest::from_bytes([0u8; 64]),
            size: 100,
        });
        state.installed_mods.push(mq_mods::InstalledModEntry {
            file_name: "fabric-api-0.166.2-26.3.jar".into(),
            path: std::path::PathBuf::from("mods/fabric-api-0.166.2-26.3.jar"),
            enabled: true,
            mod_id: "fabric-api".into(),
            name: "Fabric API".into(),
            version: "0.166.2".into(),
            authors: vec![],
            description: None,
            loader: Some(ModLoaderKind::Fabric),
            dependencies: vec![],
            sha1: mq_core::Sha1Digest::from_bytes([0u8; 20]),
            sha512: mq_core::Sha512Digest::from_bytes([0u8; 64]),
            size: 100,
        });

        assert!(is_hit_installed_in_instance(
            &state,
            &make_hit(ProjectKind::Mod, "journeymap", "JourneyMap")
        ));
        assert!(is_hit_installed_in_instance(
            &state,
            &make_hit(ProjectKind::Mod, "fabric-api", "Fabric API")
        ));
        for unrelated in [
            ("fabric", "Fabric"),
            ("api", "API"),
            ("map", "Map"),
            ("journey", "Journey"),
            ("fabric-language-kotlin", "Fabric Language Kotlin"),
            ("sodium-extra", "Sodium Extra"),
        ] {
            assert!(
                !is_hit_installed_in_instance(
                    &state,
                    &make_hit(ProjectKind::Mod, unrelated.0, unrelated.1)
                ),
                "unexpected false positive for {} / {}",
                unrelated.0,
                unrelated.1
            );
        }
    }
}
