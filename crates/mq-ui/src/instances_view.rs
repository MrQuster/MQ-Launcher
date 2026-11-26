#![deny(clippy::await_holding_lock)]
//! Instances screen: instance grid, creation dialog, instance details, mods table,
//! live game console with log filtering, and JVM settings. Zero emojis!

use egui::{
    Color32, CornerRadius, CursorIcon, FontId, Pos2, Rect, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, Ui, Vec2,
};
use mq_core::{GameLogLevel, ModLoaderKind};
use mq_instances::{
    delete_world, list_backups, list_screenshots, list_worlds, load_servers_dat, save_servers_dat,
    InstanceManifest, ServerEntry,
};

use crate::components::button::{button_primary, button_secondary};
use crate::components::{
    render_badge, render_underline_tabs, segmented_switch, BadgeKind, FilterChip, SearchField,
    SegmentedOption, Select, SelectOption,
};
use crate::state::{UiState, UiWorkerCommand};
use crate::theme::{
    blend_color, Tokens, ACCENT_COBALT, ACCENT_EMERALD, BORDER_SUBTLE, CARD_SURFACE, RADIUS_CARD,
    RADIUS_TAG, STATUS_DANGER, STATUS_WARN, TEXT_MUTED, TEXT_PRIMARY,
};
use crate::theme_styles::{show_themed_card, ThemeStyleParams};
use crate::widgets::{
    danger_button, hero_action_button, loader_badge, paint_vector_icon, primary_emerald_button,
    secondary_glass_button, status_pill, version_badge, VectorIcon,
};

/// Minecraft version release category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McVersionCategory {
    Release,
    Snapshot,
    OldBeta,
    OldAlpha,
}

impl McVersionCategory {
    pub fn badge_label(&self) -> &'static str {
        match self {
            Self::Release => "Релиз",
            Self::Snapshot => "Снапшот",
            Self::OldBeta => "Beta",
            Self::OldAlpha => "Alpha",
        }
    }

    pub fn badge_color(&self) -> Color32 {
        match self {
            Self::Release => Color32::from_rgb(0x4C, 0x9A, 0x82),
            Self::Snapshot => ACCENT_COBALT,
            Self::OldBeta => Color32::from_rgb(0xD9, 0x8A, 0x2B),
            Self::OldAlpha => Color32::from_rgb(0xC7, 0x67, 0x8F),
        }
    }
}

/// All official release versions of Minecraft Java Edition in reverse chronological order.
pub const ALL_MC_RELEASES: &[&str] = &[
    "1.21.4", "1.21.3", "1.21.2", "1.21", "1.20.6", "1.20.5", "1.20.4", "1.20.3", "1.20.2",
    "1.20.1", "1.20", "1.19.4", "1.19.3", "1.19.2", "1.19.1", "1.19", "1.18.2", "1.18.1", "1.18",
    "1.17.1", "1.17", "1.16.5", "1.16.4", "1.16.3", "1.16.2", "1.16.1", "1.16", "1.15.2", "1.15.1",
    "1.15", "1.14.4", "1.14.3", "1.14.2", "1.14.1", "1.14", "1.13.2", "1.13.1", "1.13", "1.12.2",
    "1.12.1", "1.12", "1.11.2", "1.11", "1.10.2", "1.10.1", "1.10", "1.9.4", "1.9.2", "1.9",
    "1.8.9", "1.8.8", "1.8", "1.7.10", "1.7.9", "1.7.2", "1.6.4", "1.6.2", "1.5.2", "1.4.7",
    "1.3.2", "1.2.5", "1.1", "1.0.0",
];

pub const ALL_MC_VERSIONS: &[&str] = ALL_MC_RELEASES;

/// Notable and recent snapshot versions.
pub const ALL_MC_SNAPSHOTS: &[&str] = &[
    "25w09b",
    "25w09a",
    "25w08a",
    "25w07a",
    "25w06a",
    "25w05a",
    "25w04a",
    "25w03a",
    "25w02a",
    "1.21.4-rc3",
    "1.21.4-rc2",
    "1.21.4-rc1",
    "1.21.4-pre2",
    "1.21.4-pre1",
    "24w46a",
    "24w45a",
    "24w44a",
    "24w40a",
    "24w39a",
    "24w38a",
    "24w37a",
    "24w36a",
    "1.21.2-rc2",
    "1.21.2-pre5",
    "1.21.2-pre1",
    "24w35a",
    "24w34a",
    "24w33a",
    "24w14a",
    "24w13a",
    "24w12a",
    "24w11a",
    "24w10a",
    "23w51b",
    "23w46a",
    "23w45a",
    "23w44a",
    "23w43a",
    "22w46a",
    "22w42a",
    "21w44a",
    "21w37a",
    "20w51a",
    "20w45a",
    "20w14infinite",
    "19w46b",
    "19w34a",
    "18w50a",
    "18w07a",
    "17w47b",
    "16w50a",
    "15w51b",
    "14w34d",
    "13w49a",
    "12w49a",
];

/// Classic Minecraft Beta versions.
pub const ALL_MC_BETAS: &[&str] = &[
    "b1.8.1", "b1.8", "b1.7.3", "b1.7.2", "b1.7", "b1.6.6", "b1.6.5", "b1.6.4", "b1.6", "b1.5_01",
    "b1.5", "b1.4_01", "b1.4", "b1.3_01", "b1.3", "b1.2_02", "b1.2_01", "b1.2", "b1.1_02",
    "b1.1_01", "b1.1", "b1.0_01", "b1.0",
];

/// Classic Minecraft Alpha / Infdev / Classic versions.
pub const ALL_MC_ALPHAS: &[&str] = &[
    "a1.2.6",
    "a1.2.5",
    "a1.2.4_01",
    "a1.2.3_04",
    "a1.2.2",
    "a1.2.1_01",
    "a1.2.0_02",
    "a1.1.2_01",
    "a1.1.0",
    "a1.0.17_04",
    "a1.0.16",
    "a1.0.15",
    "a1.0.14",
    "a1.0.5_01",
    "a1.0.4",
    "inf-20100618",
    "inf-20100415",
    "c0.30_01c",
    "c0.0.13a_03",
    "c0.0.11a",
    "rd-161348",
    "rd-132211",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderFilter {
    All,
    Vanilla,
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

impl LoaderFilter {
    pub fn matches(self, kind: ModLoaderKind) -> bool {
        match self {
            Self::All => true,
            Self::Vanilla => kind == ModLoaderKind::Vanilla,
            Self::Fabric => kind == ModLoaderKind::Fabric,
            Self::Quilt => kind == ModLoaderKind::Quilt,
            Self::Forge => kind == ModLoaderKind::Forge,
            Self::NeoForge => kind == ModLoaderKind::NeoForge,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstanceDetailTab {
    Console,
    Mods,
    ResourcePacks,
    ShaderPacks,
    Worlds,
    Servers,
    Screenshots,
    Settings,
}

impl InstanceDetailTab {
    const fn order_index(self) -> i32 {
        match self {
            Self::Console => 0,
            Self::Mods => 1,
            Self::ResourcePacks => 2,
            Self::ShaderPacks => 3,
            Self::Worlds => 4,
            Self::Servers => 5,
            Self::Screenshots => 6,
            Self::Settings => 7,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum InstanceViewMode {
    #[default]
    List,
    Grid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InstanceSortMode {
    #[default]
    Playtime,
    Name,
    Created,
}

/// Filter status for installed mods list inside an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModStatusFilter {
    #[default]
    All,
    Enabled,
    Disabled,
}

/// Sub-mode inside an instance's content tabs (`Моды`, `Ресурспаки`, `Шейдеры`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum ContentSubMode {
    #[default]
    Installed,
    Catalog,
}

pub struct InstancesView {
    pub current_tab: InstanceDetailTab,
    pub content_sub_mode: ContentSubMode,
    pub synced_catalog_context: Option<(mq_core::InstanceId, crate::catalog_state::CatalogTab)>,
    pub local_file_input: String,
    pub catalog_view: crate::catalog_view::CatalogView,
    pub view_mode: InstanceViewMode,
    pub sort_mode: InstanceSortMode,
    pub loader_filter: LoaderFilter,
    pub search_query: String,
    pub mod_search_query: String,
    pub mod_status_filter: ModStatusFilter,
    pub pack_search_query: String,
    pub shader_search_query: String,
    pub console_autoscroll: bool,
    pub console_search: String,
    pub show_releases: bool,
    pub show_snapshots: bool,
    pub show_betas: bool,
    pub show_alphas: bool,
    pub version_search: String,
    pub show_locale_dialog: bool,
    pub locale_search_query: String,
    pub show_import_dialog: bool,
    pub import_file_path: String,
    pub import_name: String,
    pub import_error: Option<String>,
    pub show_duplicate_dialog: bool,
    pub duplicate_name: String,
    pub show_add_server_dialog: bool,
    pub new_server_name: String,
    pub new_server_ip: String,
    pub mod_update_check_done: bool,
    pub show_delete_dialog: bool,
    pub instance_to_delete: Option<(mq_core::InstanceId, String)>,
    pub mod_profile_input: String,
    pub selected_mod_profile: Option<String>,
    pub datapack_input_path: String,
    pub expanded_datapack_world: Option<String>,
    pub confirm_delete_world: Option<String>,
}

impl Default for InstancesView {
    fn default() -> Self {
        Self {
            current_tab: InstanceDetailTab::Console,
            content_sub_mode: ContentSubMode::Installed,
            synced_catalog_context: None,
            local_file_input: String::new(),
            catalog_view: crate::catalog_view::CatalogView::default(),
            view_mode: InstanceViewMode::Grid,
            sort_mode: InstanceSortMode::Playtime,
            loader_filter: LoaderFilter::All,
            search_query: String::new(),
            mod_search_query: String::new(),
            mod_status_filter: ModStatusFilter::All,
            pack_search_query: String::new(),
            shader_search_query: String::new(),
            console_autoscroll: true,
            console_search: String::new(),
            show_releases: true,
            show_snapshots: false,
            show_betas: false,
            show_alphas: false,
            version_search: String::new(),
            show_locale_dialog: false,
            locale_search_query: String::new(),
            show_import_dialog: false,
            import_file_path: String::new(),
            import_name: String::new(),
            import_error: None,
            show_duplicate_dialog: false,
            duplicate_name: String::new(),
            show_add_server_dialog: false,
            new_server_name: String::new(),
            new_server_ip: String::new(),
            mod_update_check_done: false,
            show_delete_dialog: false,
            instance_to_delete: None,
            mod_profile_input: String::new(),
            selected_mod_profile: None,
            datapack_input_path: String::new(),
            expanded_datapack_world: None,
            confirm_delete_world: None,
        }
    }
}

impl InstancesView {
    pub fn ui(&mut self, ui: &mut Ui, state: &mut UiState) {
        if state.selected_instance_id.is_none() && !state.instances.is_empty() {
            state.selected_instance_id = Some(state.instances[0].id.clone());
        }

        let tokens = Tokens::from_config(&state.config.theme);

        if !state.show_create_instance_dialog
            && !self.show_import_dialog
            && ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::CTRL, egui::Key::N)
                    || i.consume_key(egui::Modifiers::COMMAND, egui::Key::N)
            })
        {
            state.show_create_instance_dialog = true;
        }

        let in_embedded_catalog = self.view_mode == InstanceViewMode::List
            && state.selected_instance().is_some()
            && matches!(
                self.current_tab,
                InstanceDetailTab::Mods
                    | InstanceDetailTab::ResourcePacks
                    | InstanceDetailTab::ShaderPacks
            )
            && self.content_sub_mode == ContentSubMode::Catalog;

        if !state.instances.is_empty() {
            // Row 1: Header + primary action buttons (adapts cleanly at 720-960px widths)
            let avail_w = ui.available_width();
            let min_single_row_w = if in_embedded_catalog { 420.0 } else { 680.0 };
            if avail_w >= min_single_row_w {
                ui.horizontal(|ui| {
                    ui.heading(
                        RichText::new("Инстансы")
                            .color(tokens.text)
                            .size(20.0)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.render_header_action_buttons(
                            ui,
                            state,
                            &tokens,
                            in_embedded_catalog,
                            true,
                        );
                    });
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                    ui.heading(
                        RichText::new("Инстансы")
                            .color(tokens.text)
                            .size(20.0)
                            .strong(),
                    );
                    self.render_header_action_buttons(
                        ui,
                        state,
                        &tokens,
                        in_embedded_catalog,
                        false,
                    );
                });
            }

            if !in_embedded_catalog {
                ui.add_space(8.0);

                // Row 2: SearchField + loader filter + sorting dropdown + view mode segmented switch
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(10.0, 6.0);

                    SearchField::new(&mut self.search_query, "Поиск инстанса...")
                        .width(180.0)
                        .show(ui, &tokens);

                    let loader_opts = [
                        SelectOption::new(LoaderFilter::All, "Все загрузчики"),
                        SelectOption::new(LoaderFilter::Vanilla, "Vanilla"),
                        SelectOption::new(LoaderFilter::Fabric, "Fabric"),
                        SelectOption::new(LoaderFilter::Quilt, "Quilt"),
                        SelectOption::new(LoaderFilter::Forge, "Forge"),
                        SelectOption::new(LoaderFilter::NeoForge, "NeoForge"),
                    ];
                    Select::new(
                        "instance_loader_filter",
                        "Загрузчик",
                        &mut self.loader_filter,
                        &loader_opts,
                    )
                    .show(ui, &tokens);

                    let sort_opts = [
                        SelectOption::new(InstanceSortMode::Playtime, "По времени"),
                        SelectOption::new(InstanceSortMode::Name, "По имени"),
                        SelectOption::new(InstanceSortMode::Created, "По дате"),
                    ];
                    Select::new(
                        "instance_sort_select",
                        "Сортировка",
                        &mut self.sort_mode,
                        &sort_opts,
                    )
                    .show(ui, &tokens);

                    let view_opts = [
                        SegmentedOption::new(InstanceViewMode::Grid, "Сетка")
                            .with_icon(VectorIcon::Layers)
                            .with_tooltip("Отображение плитками"),
                        SegmentedOption::new(InstanceViewMode::List, "Список")
                            .with_icon(VectorIcon::Terminal)
                            .with_tooltip("Отображение списком"),
                    ];
                    segmented_switch(ui, &tokens, &mut self.view_mode, &view_opts);
                });

                ui.add_space(8.0);
            } else {
                ui.add_space(4.0);
            }
        }

        // Instance creation modal (runs even during fade-out)
        self.render_create_instance_modal(ui, state, &tokens);

        // Duplicate modal
        if self.show_duplicate_dialog && ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_duplicate_dialog = false;
        }
        let dup_payload = if self.show_duplicate_dialog {
            state.selected_instance().cloned()
        } else {
            None
        };
        if let Some((selected_inst, dup_anim)) = crate::animation::animate_modal_cached(
            ui.ctx(),
            egui::Id::new("mq_duplicate_inst_modal"),
            dup_payload,
        ) {
            let modal_win = egui::Window::new("Дублирование инстанса")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]);
            crate::animation::show_animated_modal_window(
                ui.ctx(),
                egui::Id::new("mq_duplicate_inst_modal_win"),
                &dup_anim,
                modal_win,
                |ui| {
                    let modal_w = 340.0_f32.min(ui.ctx().screen_rect().width() - 32.0);
                    ui.set_width(modal_w);
                    ui.label("Имя нового инстанса:");
                    ui.text_edit_singleline(&mut self.duplicate_name);
                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        if primary_emerald_button(ui, Some(VectorIcon::Check), "Дублировать")
                            .clicked()
                            && !self.duplicate_name.trim().is_empty()
                        {
                            match state
                                .instance_mgr
                                .clone_instance(&selected_inst.name, &self.duplicate_name)
                            {
                                Ok(cloned) => {
                                    state.instances.push(cloned.clone());
                                    state.selected_instance_id = Some(cloned.id);
                                    state.status_message = Some((
                                        format!(
                                            "Инстанс успешно продублирован как '{}'",
                                            self.duplicate_name
                                        ),
                                        false,
                                    ));
                                }
                                Err(e) => {
                                    state.status_message =
                                        Some((format!("Ошибка дублирования: {e}"), true));
                                }
                            }
                            self.show_duplicate_dialog = false;
                        }
                        if secondary_glass_button(ui, None, "Отмена").clicked() {
                            self.show_duplicate_dialog = false;
                        }
                    });
                },
            );
        }

        if let Some(picked) = state.picked_import_path.take() {
            self.import_file_path =
                crate::opener::strip_windows_verbatim_prefix(&picked.display().to_string());
            self.import_error = None;
            if !self.show_import_dialog {
                self.show_import_dialog = true;
            }
        }

        // Drag-and-drop `.mrpack` onto the Instances screen
        if !self.show_import_dialog && state.install.is_none() {
            let dropped_mrpack = ui.ctx().input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .filter_map(|f| f.path.clone())
                    .find(|p| {
                        p.extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| e.eq_ignore_ascii_case("mrpack"))
                    })
            });
            if let Some(pack_path) = dropped_mrpack {
                state.import_local_pack(pack_path);
            }
        }

        // Import modal
        if self.show_import_dialog
            && ui
                .ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.show_import_dialog = false;
            self.import_error = None;
        }
        let import_anim = crate::animation::animate_modal_presence(
            ui.ctx(),
            egui::Id::new("mq_import_inst_modal"),
            self.show_import_dialog,
        );
        if import_anim.should_render {
            let hovered_file = if !import_anim.is_closing {
                ui.ctx().input(|i| !i.raw.hovered_files.is_empty())
            } else {
                false
            };
            let dropped_file = if !import_anim.is_closing {
                ui.ctx().input(|i| {
                    i.raw
                        .dropped_files
                        .iter()
                        .filter_map(|f| f.path.clone())
                        .next()
                })
            } else {
                None
            };
            if let Some(p) = dropped_file {
                self.import_file_path =
                    crate::opener::strip_windows_verbatim_prefix(&p.display().to_string());
                self.import_error = None;
            }

            let import_win = egui::Window::new("Импорт сборки (.mrpack или .zip)")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .frame(
                    egui::Frame::window(&egui::Style::default())
                        .fill(tokens.bg_surface)
                        .stroke(Stroke::new(1.2_f32, tokens.border_strong))
                        .corner_radius(RADIUS_CARD)
                        .inner_margin(egui::Margin::same(16)),
                );

            crate::animation::show_animated_modal_window(
                ui.ctx(),
                egui::Id::new("mq_import_inst_modal_win"),
                &import_anim,
                import_win,
                |ui| {
                    let modal_w = 460.0_f32.min(ui.ctx().screen_rect().width() - 32.0);
                    ui.set_width(modal_w);

                    let drop_border = if hovered_file {
                        tokens.accent
                    } else {
                        tokens.border
                    };
                    egui::Frame::new()
                        .fill(tokens.bg_surface_hover)
                        .stroke(Stroke::new(1.2_f32, drop_border))
                        .corner_radius(RADIUS_CARD)
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new(if hovered_file {
                                        "Отпустите файл .mrpack или .zip для импорта"
                                    } else {
                                        "Перетащите файл .mrpack / .zip сюда или выберите через диалог"
                                    })
                                    .color(if hovered_file {
                                        tokens.accent
                                    } else {
                                        tokens.text_muted
                                    })
                                    .small(),
                                );
                            });
                        });

                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Путь к архиву (.mrpack или .zip):")
                            .color(tokens.text_muted)
                            .small(),
                    );
                    ui.horizontal(|ui| {
                        let input_w = (modal_w - 148.0).max(180.0);
                        let import_hint = if cfg!(target_os = "windows") {
                            r"C:\Users\...\modpack.mrpack"
                        } else {
                            "/path/to/modpack.mrpack"
                        };
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut self.import_file_path)
                                    .hint_text(import_hint)
                                    .desired_width(input_w),
                            )
                            .changed()
                        {
                            self.import_error = None;
                        }
                        if secondary_glass_button(ui, Some(VectorIcon::Folder), "Выбрать файл...")
                            .clicked()
                        {
                            let _ = state.cmd_sender.send(UiWorkerCommand::PickImportArchive);
                        }
                    });

                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Имя инстанса (опционально):")
                            .color(tokens.text_muted)
                            .small(),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.import_name)
                            .hint_text("По умолчанию — из манифеста или имени файла")
                            .desired_width(modal_w),
                    );

                    if let Some(ref err) = self.import_error {
                        ui.add_space(6.0);
                        ui.label(RichText::new(err).color(tokens.danger).small());
                    }

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if primary_emerald_button(ui, Some(VectorIcon::Check), "Импортировать")
                            .clicked()
                        {
                            let cleaned_path =
                                crate::opener::clean_user_path(&self.import_file_path);
                            let trimmed_path = cleaned_path.as_str();
                            let path = std::path::Path::new(trimmed_path);
                            if trimmed_path.is_empty() {
                                self.import_error =
                                    Some("Укажите путь к файлу .mrpack или .zip.".to_string());
                            } else if !path.is_file() {
                                self.import_error =
                                    Some(format!("Файл не найден: {}", path.display()));
                            } else {
                                let opt_name = if self.import_name.trim().is_empty() {
                                    None
                                } else {
                                    Some(self.import_name.trim().to_owned())
                                };
                                let is_mrpack = path
                                    .extension()
                                    .and_then(|e| e.to_str())
                                    .is_some_and(|e| e.eq_ignore_ascii_case("mrpack"));
                                if is_mrpack {
                                    self.show_import_dialog = false;
                                    self.import_error = None;
                                    state.import_local_pack_with_name(path.to_path_buf(), opt_name);
                                } else {
                                    let _ =
                                        state.cmd_sender.send(UiWorkerCommand::ImportInstanceZip {
                                            path: path.to_path_buf(),
                                            custom_name: opt_name,
                                        });
                                    state.status_message = Some((
                                        "Импорт инстанса из архива запущен...".to_string(),
                                        false,
                                    ));
                                    self.show_import_dialog = false;
                                    self.import_error = None;
                                }
                            }
                        }
                        if secondary_glass_button(ui, None, "Отмена").clicked() {
                            self.show_import_dialog = false;
                            self.import_error = None;
                        }
                    });
                },
            );
        }

        // External launcher instances import modal (Prism, ElyPrism, MultiMC, Modrinth App)
        if state.show_external_import_modal
            && ui
                .ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            state.show_external_import_modal = false;
        }
        let ext_anim = crate::animation::animate_modal_presence(
            ui.ctx(),
            egui::Id::new("mq_ext_import_modal"),
            state.show_external_import_modal,
        );
        if ext_anim.should_render {
            let ext_win = egui::Window::new("Импорт из сторонних лаунчеров")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .frame(
                    egui::Frame::window(&egui::Style::default())
                        .fill(tokens.bg_surface)
                        .stroke(Stroke::new(1.2_f32, tokens.border_strong))
                        .corner_radius(RADIUS_CARD)
                        .inner_margin(egui::Margin::same(16)),
                );

            crate::animation::show_animated_modal_window(
                ui.ctx(),
                egui::Id::new("mq_ext_import_modal_win"),
                &ext_anim,
                ext_win,
                |ui| {
                    let modal_w = 520.0_f32.min(ui.ctx().screen_rect().width() - 32.0);
                    ui.set_width(modal_w);

                    ui.label(
                        RichText::new(
                            "Обнаруженные инстансы (Prism / ElyPrism / MultiMC / Modrinth App):",
                        )
                        .color(tokens.text)
                        .strong(),
                    );
                    ui.add_space(8.0);

                    if state.external_instances.is_empty() {
                        ui.label(
                            RichText::new(
                                "Сторонние инстансы не обнаружены в стандартных директориях.",
                            )
                            .color(tokens.text_muted),
                        );
                        ui.add_space(8.0);
                        if secondary_glass_button(
                            ui,
                            Some(VectorIcon::Refresh),
                            "Сканировать снова",
                        )
                        .clicked()
                        {
                            let _ = state
                                .cmd_sender
                                .send(UiWorkerCommand::DiscoverExternalInstances);
                        }
                    } else {
                        ScrollArea::vertical()
                            .max_height(260.0)
                            .id_salt("ext_inst_scroll")
                            .show(ui, |ui| {
                                for item in &state.external_instances {
                                    let l_kind = match item.launcher {
                                        mq_instances::ExternalLauncherKind::Prism => "Prism",
                                        mq_instances::ExternalLauncherKind::ElyPrism => "ElyPrism",
                                        mq_instances::ExternalLauncherKind::MultiMc => "MultiMC",
                                        mq_instances::ExternalLauncherKind::ModrinthApp => {
                                            "Modrinth"
                                        }
                                    };
                                    let l_name = format!(
                                        "{l_kind} · {} ({}, {})",
                                        item.name,
                                        item.mc_version,
                                        item.loader.as_str()
                                    );
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(&l_name).color(tokens.text).strong(),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if primary_emerald_button(
                                                    ui,
                                                    Some(VectorIcon::Download),
                                                    "Импортировать",
                                                )
                                                .clicked()
                                                {
                                                    let _ = state.cmd_sender.send(
                                                        UiWorkerCommand::ImportExternalInstance {
                                                            item: item.clone(),
                                                        },
                                                    );
                                                    state.show_external_import_modal = false;
                                                }
                                            },
                                        );
                                    });
                                    ui.label(
                                        RichText::new(format!(
                                            "Модов: {}, миров: {}",
                                            item.mod_count, item.world_count
                                        ))
                                        .color(tokens.text_muted)
                                        .small(),
                                    );
                                    ui.separator();
                                }
                            });
                    }

                    ui.add_space(8.0);
                    if secondary_glass_button(ui, None, "Закрыть").clicked() {
                        state.show_external_import_modal = false;
                    }
                },
            );
        }

        // Delete confirmation modal
        if self.show_delete_dialog && ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_delete_dialog = false;
            self.instance_to_delete = None;
        }
        let del_payload = if self.show_delete_dialog {
            self.instance_to_delete.clone()
        } else {
            None
        };
        if self.show_delete_dialog && del_payload.is_none() {
            self.show_delete_dialog = false;
        }
        if let Some(((target_id, target_name), del_anim)) = crate::animation::animate_modal_cached(
            ui.ctx(),
            egui::Id::new("mq_delete_inst_modal"),
            del_payload,
        ) {
            let is_running = state.running_instances.contains_key(&target_id);
            let is_downloading = state.installing_instance.as_ref() == Some(&target_id);

            let del_win = egui::Window::new("Удаление инстанса")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]);
            crate::animation::show_animated_modal_window(
                ui.ctx(),
                egui::Id::new("mq_delete_inst_modal_win"),
                &del_anim,
                del_win,
                |ui| {
                    let screen_rect = ui.ctx().screen_rect();
                    let modal_w = 420.0_f32.min(screen_rect.width() - 32.0);
                    ui.set_width(modal_w);

                    let body_max_h = (screen_rect.height() - 110.0).max(60.0);
                    egui::ScrollArea::vertical()
                        .id_salt("delete_dialog_scroll")
                        .max_height(body_max_h)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                let (rect, _) =
                                    ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::hover());
                                paint_vector_icon(
                                    ui.painter(),
                                    rect,
                                    VectorIcon::Warning,
                                    STATUS_DANGER,
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    RichText::new(format!("Удалить «{}»?", target_name))
                                        .color(TEXT_PRIMARY)
                                        .strong()
                                        .size(15.0),
                                );
                            });

                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(
                                    "Вы уверены, что хотите удалить этот инстанс? Это действие безвозвратно удалит директорию инстанса, включая все установленные моды, миры, скриншоты и настройки.",
                                )
                                .color(TEXT_MUTED)
                                .size(12.5),
                            );

                            if is_running {
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(
                                        "Инстанс сейчас запущен! Закройте игру перед удалением.",
                                    )
                                    .color(STATUS_DANGER)
                                    .strong(),
                                );
                            } else if is_downloading {
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new("Инстанс сейчас загружается/устанавливается. Дождитесь окончания или отмените загрузку.")
                                        .color(STATUS_DANGER)
                                        .strong(),
                                );
                            }
                        });

                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        let can_delete = !is_running && !is_downloading;
                        ui.add_enabled_ui(can_delete, |ui| {
                            if danger_button(ui, Some(VectorIcon::Trash), "Удалить").clicked()
                            {
                                let id_to_remove = target_id.clone();
                                state.invalidate_install_cache(Some(&id_to_remove));
                                state.instances.retain(|i| i.id != id_to_remove);
                                state.favorite_instances.remove(target_id.as_str());
                                if state.selected_instance_id.as_ref() == Some(&id_to_remove) {
                                    state.selected_instance_id =
                                        state.instances.first().map(|i| i.id.clone());
                                }
                                state.status_message = Some((
                                    format!("Удаление инстанса «{}»...", target_name),
                                    false,
                                ));
                                let _ = state.cmd_sender.send(UiWorkerCommand::DeleteInstance {
                                    instance_id: id_to_remove,
                                    name: target_name.clone(),
                                });
                                self.show_delete_dialog = false;
                                self.instance_to_delete = None;
                            }
                        });
                        if secondary_glass_button(ui, None, "Отмена").clicked() {
                            self.show_delete_dialog = false;
                            self.instance_to_delete = None;
                        }
                    });
                },
            );
        }

        crate::catalog_dialogs::show_overlays(ui.ctx(), state);

        if !state.jobs.jobs.is_empty() {
            if let Some(prog) = &state.phase_progress {
                if state.installing_instance.is_none() && state.jobs.active_count() > 0 {
                    let frac =
                        crate::state::safe_progress_ratio(prog.completed_items, prog.total_items);
                    let speed = if prog.mib_per_sec.is_finite() && prog.mib_per_sec > 0.0 {
                        prog.mib_per_sec * 1024.0 * 1024.0
                    } else {
                        0.0
                    };
                    crate::widgets::render_detailed_install_progress_card(
                        ui,
                        &tokens,
                        state.current_install_stage,
                        frac,
                        speed,
                        prog.bytes_done,
                        prog.bytes_total,
                        prog.completed_items,
                        prog.total_items,
                        state.install_eta_secs,
                        state.install_retries_count,
                        || {
                            let _ = state.cmd_sender.send(UiWorkerCommand::CancelDownload);
                        },
                    );
                    ui.add_space(6.0);
                }
            }
            let tab_renders_jobs = self.view_mode == InstanceViewMode::List
                && state.selected_instance().is_some()
                && matches!(
                    self.current_tab,
                    InstanceDetailTab::Mods
                        | InstanceDetailTab::ResourcePacks
                        | InstanceDetailTab::ShaderPacks
                );
            if !tab_renders_jobs {
                crate::catalog_dialogs::jobs_panel(ui, state);
            }
        }

        let any_modal_open = state.show_create_instance_dialog
            || self.show_import_dialog
            || self.show_delete_dialog
            || self.show_duplicate_dialog;

        if state.instances.is_empty() {
            ui.add_enabled_ui(!any_modal_open, |ui| {
                self.render_empty_onboarding_hub(ui, state, &tokens);
            });
            return;
        }

        let view_order = if self.view_mode == InstanceViewMode::Grid {
            0
        } else {
            1
        };
        crate::animation::render_tab_transition(
            ui,
            "mq_inst_view_mode_trans",
            self.view_mode,
            view_order,
            true,
            |ui| {
                if self.view_mode == InstanceViewMode::Grid {
                    self.render_grid_view(ui, state, &tokens);
                    return;
                }

                let avail_h = ui.available_height();

                // Layout: Left column (instances list) + Right column (selected instance details)
                ui.horizontal(|ui| {
            ui.set_min_height(avail_h);

            // Left list of instances
            ui.vertical(|ui| {
                ui.set_width(220.0);
                ui.set_min_height(avail_h);
                ui.label(RichText::new("Инстансы").color(tokens.text_muted).strong());
                ui.add_space(4.0);

                ScrollArea::vertical()
                    .id_salt("instances_scroll")
                    .auto_shrink([false, false])
                    .max_height(avail_h - 40.0)
                    .show(ui, |ui| {
                        if state.instances.is_empty() {
                            ui.label(RichText::new("Нет инстансов").color(TEXT_MUTED));
                            if primary_emerald_button(ui, Some(VectorIcon::Plus), "Создать инстанс")
                                .clicked()
                            {
                                state.show_create_instance_dialog = true;
                            }
                        }

                        let mut action_delete = None;
                        let mut action_open_dir = None;
                        let mut action_duplicate = None;
                        let mut action_export = None;

                        for inst in &state.instances {
                            let is_selected = state.selected_instance_id.as_ref() == Some(&inst.id);
                            let is_installed = state.is_instance_installed(&inst.id);
                            let is_downloading =
                                state.installing_instance.as_ref() == Some(&inst.id);
                            let is_running = state.running_instances.contains_key(&inst.id);

                            let list_style = ThemeStyleParams::active(ui.ctx());
                            let list_radius = list_style.card_corner_radius(state.config.theme.corner_radius);
                            let stroke_color = if is_downloading {
                                ACCENT_COBALT
                            } else if is_running || is_selected {
                                tokens.accent
                            } else {
                                tokens.border
                            };

                            let frame = egui::Frame::new()
                                .fill(if is_selected {
                                    list_style.surface_hover_fill(&tokens)
                                } else {
                                    list_style.surface_fill(&tokens)
                                })
                                .stroke(if list_style.refraction_border {
                                    Stroke::NONE
                                } else {
                                    Stroke::new(1.0_f32, stroke_color)
                                })
                                .corner_radius(list_radius)
                                .inner_margin(egui::Margin::same(8));

                            let resp = show_themed_card(
                                ui,
                                frame,
                                list_radius,
                                ui.make_persistent_id(("inst_list_glass", inst.id.as_str())),
                                &list_style,
                                &tokens,
                                |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let (rect, _) = ui.allocate_exact_size(
                                        Vec2::splat(16.0),
                                        egui::Sense::hover(),
                                    );
                                    let (icon, icon_col) = if is_downloading {
                                        (VectorIcon::Download, ACCENT_COBALT)
                                    } else if is_running {
                                        (VectorIcon::Play, ACCENT_EMERALD)
                                    } else if is_installed {
                                        (
                                            VectorIcon::Layers,
                                            if is_selected {
                                                ACCENT_EMERALD
                                            } else {
                                                ACCENT_COBALT
                                            },
                                        )
                                    } else {
                                        (VectorIcon::Download, TEXT_MUTED)
                                    };
                                    paint_vector_icon(ui.painter(), rect, icon, icon_col);

                                    ui.vertical(|ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&inst.name)
                                                    .color(TEXT_PRIMARY)
                                                    .strong(),
                                            )
                                            .truncate(),
                                        );
                                        ui.horizontal(|ui| {
                                            version_badge(ui, &inst.mc_version, ACCENT_COBALT);
                                            loader_badge(ui, inst.loader);

                                            let (dot_col, tip) = if is_downloading {
                                                (ACCENT_COBALT, "Загрузка")
                                            } else if is_running {
                                                (ACCENT_EMERALD, "Запущен")
                                            } else if is_installed {
                                                (ACCENT_EMERALD.gamma_multiply(0.7), "Установлен")
                                            } else {
                                                (STATUS_WARN, "Не скачан")
                                            };
                                            let (d_rect, d_resp) = ui.allocate_exact_size(
                                                Vec2::splat(10.0),
                                                egui::Sense::hover(),
                                            );
                                            if ui.is_rect_visible(d_rect) {
                                                ui.painter().circle_filled(
                                                    d_rect.center(),
                                                    3.5,
                                                    dot_col,
                                                );
                                            }
                                            d_resp.on_hover_text(tip);
                                        });
                                    });
                                });
                            },
                            );
                            if list_style.refraction_border && (is_selected || is_running || is_downloading) {
                                crate::theme_styles::paint_liquid_glass_selection_halo(
                                    ui.painter(),
                                    resp.response.rect,
                                    list_radius,
                                    if is_downloading { ACCENT_COBALT } else { tokens.accent },
                                );
                            }

                            let click_resp = resp.response.interact(egui::Sense::click());
                            if click_resp.clicked() {
                                let switched =
                                    state.selected_instance_id.as_ref() != Some(&inst.id);
                                state.selected_instance_id = Some(inst.id.clone());
                                if switched {
                                    state.installed_mods.clear();
                                    state.mod_conflicts.clear();
                                    state.is_scanning_mods = true;
                                    state.mclogs_url = None;
                                    state.is_uploading_mclogs = false;
                                }
                                // Trigger scanning mods
                                let _ = state.cmd_sender.send(UiWorkerCommand::ScanInstanceMods {
                                    instance_id: inst.id.clone(),
                                });
                            }

                            click_resp.context_menu(|ui| {
                                ui.set_min_width(150.0);
                                if ui.button("Открыть папку").clicked() {
                                    action_open_dir = Some(inst.id.clone());
                                    ui.close_menu();
                                }
                                if ui.button("Дублировать").clicked() {
                                    action_duplicate = Some((inst.id.clone(), inst.name.clone()));
                                    ui.close_menu();
                                }
                                if ui.button("Экспорт (.zip)").clicked() {
                                    action_export = Some(inst.id.clone());
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui
                                    .button(RichText::new("Удалить инстанс").color(STATUS_DANGER))
                                    .clicked()
                                {
                                    action_delete = Some((inst.id.clone(), inst.name.clone()));
                                    ui.close_menu();
                                }
                            });

                            ui.add_space(4.0);
                        }

                        if let Some((id, name)) = action_delete {
                            self.show_delete_dialog = true;
                            self.instance_to_delete = Some((id, name));
                        }
                        if let Some((id, name)) = action_duplicate {
                            state.selected_instance_id = Some(id);
                            self.show_duplicate_dialog = true;
                            self.duplicate_name = format!("{} (Копия)", name);
                        }
                        if let Some(id) = action_open_dir {
                            let inst_dir = state.paths.instance_dir(id.as_str());
                            let _ = crate::opener::open_that(inst_dir);
                        }
                        if let Some(id) = action_export {
                            state.status_message =
                                Some(("Экспорт инстанса в архив...".to_string(), false));
                            let _ = state
                                .cmd_sender
                                .send(UiWorkerCommand::ExportInstance { instance_id: id });
                        }
                    });
            });

            ui.separator();

            // Right selected instance details
            ui.vertical(|ui| {
                ui.set_min_height(avail_h);
                ui.set_width(ui.available_width());
                let selected_info = state.selected_instance().map(|i| {
                    (
                        i.id.clone(),
                        i.name.clone(),
                        i.mc_version.clone(),
                        i.loader,
                        i.memory_max_mb,
                    )
                });

                if let Some((inst_id, inst_name, mc_ver, loader, memory_max_mb)) = selected_info {
                    let is_installed = state.is_instance_installed(&inst_id);
                    let is_downloading = state.installing_instance.as_ref() == Some(&inst_id);
                    let is_running = state.running_instances.contains_key(&inst_id);

                    // 1. Error Card (if any installation/verification error occurred)
                    let error_opt = if let Some(detailed) = state.detailed_install_error.clone() {
                        Some(detailed)
                    } else if let Some(err_str) = state.install_error.clone() {
                        Some(crate::install_types::InstallError::from_error_str(
                            state.current_install_stage,
                            &err_str,
                        ))
                    } else {
                        None
                    };

                    if let Some(err) = error_opt {
                        if let Some(action) = crate::widgets::render_detailed_install_error_card(
                            ui,
                            &tokens,
                            &err,
                        ) {
                            match action {
                                crate::widgets::InstallErrorCardAction::Retry => {
                                    state.install_error = None;
                                    state.detailed_install_error = None;
                                    state.installing_instance = Some(inst_id.clone());
                                    let _ = state.cmd_sender.send(UiWorkerCommand::InstallInstance {
                                        instance_id: inst_id.clone(),
                                    });
                                }
                                crate::widgets::InstallErrorCardAction::Verify => {
                                    state.install_error = None;
                                    state.detailed_install_error = None;
                                    let _ = state.cmd_sender.send(
                                        UiWorkerCommand::VerifyAndRepairInstance {
                                            instance_id: inst_id.clone(),
                                        },
                                    );
                                }
                                crate::widgets::InstallErrorCardAction::CopyDetails => {
                                    ui.ctx().copy_text(err.format_for_clipboard());
                                    state.status_message = Some((
                                        "Подробности ошибки скопированы в буфер обмена".to_string(),
                                        false,
                                    ));
                                }
                                crate::widgets::InstallErrorCardAction::OpenLog => {
                                    let logs_dir = state.paths.logs_dir();
                                    let crash_latest = logs_dir.join("crash-latest.log");
                                    let target = if crash_latest.is_file() {
                                        crash_latest
                                    } else if logs_dir.is_dir() {
                                        logs_dir
                                    } else {
                                        state.paths.instance_dir(inst_id.as_str())
                                    };
                                    let _ = crate::opener::open_that(target);
                                }
                                crate::widgets::InstallErrorCardAction::Dismiss => {
                                    state.install_error = None;
                                    state.detailed_install_error = None;
                                }
                            }
                        }
                        ui.add_space(8.0);
                    }

                    // 2. Active Download Progress Card (prominent real-time view)
                    if is_downloading {
                        let (frac, speed, comp_bytes, tot_bytes, comp_items, tot_items) =
                            if let Some(prog) = &state.phase_progress {
                                let f = crate::state::safe_progress_ratio(
                                    prog.completed_items,
                                    prog.total_items,
                                );
                                let speed = if prog.mib_per_sec.is_finite() && prog.mib_per_sec > 0.0 {
                                    prog.mib_per_sec * 1024.0 * 1024.0
                                } else {
                                    0.0
                                };
                                (
                                    f,
                                    speed,
                                    prog.bytes_done,
                                    prog.bytes_total,
                                    prog.completed_items,
                                    prog.total_items,
                                )
                            } else {
                                (0.0, 0.0, 0, 0, 0, 0)
                            };

                        crate::widgets::render_detailed_install_progress_card(
                            ui,
                            &tokens,
                            state.current_install_stage,
                            frac,
                            speed,
                            comp_bytes,
                            tot_bytes,
                            comp_items,
                            tot_items,
                            state.install_eta_secs,
                            state.install_retries_count,
                            || {
                                let _ = state.cmd_sender.send(UiWorkerCommand::CancelDownload);
                            },
                        );
                        ui.add_space(10.0);
                    }

                    // 3. Detail hero header card (compact single row when browsing the embedded catalog)
                    if in_embedded_catalog {
                        let det_style = ThemeStyleParams::active(ui.ctx());
                        let det_r = det_style.card_corner_radius(state.config.theme.corner_radius);
                        let compact_frame =
                            crate::theme::solid_card_frame_with_config(&tokens, &state.config.theme)
                                .inner_margin(egui::Margin::symmetric(10, 4));
                        show_themed_card(
                            ui,
                            compact_frame,
                            det_r,
                            ui.make_persistent_id(("inst_compact_hdr_glass", inst_id.as_str())),
                            &det_style,
                            &tokens,
                            |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                                ui.label(
                                    RichText::new(&inst_name)
                                        .color(TEXT_PRIMARY)
                                        .size(16.0)
                                        .strong(),
                                );
                                let loader_str = match loader {
                                    ModLoaderKind::Vanilla => "Vanilla",
                                    ModLoaderKind::Fabric => "Fabric",
                                    ModLoaderKind::Quilt => "Quilt",
                                    ModLoaderKind::Forge => "Forge",
                                    ModLoaderKind::NeoForge => "NeoForge",
                                };
                                ui.label(
                                    RichText::new(format!("{loader_str} · {mc_ver}"))
                                        .font(egui::FontId::proportional(12.5))
                                        .color(tokens.text_muted),
                                );
                                if is_running {
                                    status_pill(ui, "Игра запущена", ACCENT_EMERALD);
                                } else if is_downloading {
                                    status_pill(ui, "Скачивание...", ACCENT_COBALT);
                                } else if is_installed {
                                    status_pill(ui, "Готов", ACCENT_EMERALD);
                                } else {
                                    status_pill(ui, "Не установлен", STATUS_WARN);
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if is_running {
                                            if danger_button(
                                                ui,
                                                Some(VectorIcon::Stop),
                                                "Стоп",
                                            )
                                            .clicked()
                                            {
                                                let _ = state.cmd_sender.send(
                                                    UiWorkerCommand::StopGame {
                                                        instance_id: inst_id.clone(),
                                                    },
                                                );
                                            }
                                        } else if is_downloading {
                                            if secondary_glass_button(
                                                ui,
                                                Some(VectorIcon::Download),
                                                "Отмена",
                                            )
                                            .clicked()
                                            {
                                                let _ = state
                                                    .cmd_sender
                                                    .send(UiWorkerCommand::CancelDownload);
                                            }
                                        } else if is_installed {
                                            if primary_emerald_button(
                                                ui,
                                                Some(VectorIcon::Play),
                                                "Играть",
                                            )
                                            .clicked()
                                                && !state.is_launching
                                            {
                                                state.is_launching = true;
                                                let _ = state.cmd_sender.send(
                                                    UiWorkerCommand::LaunchInstance {
                                                        instance_id: inst_id.clone(),
                                                        dry_run: false,
                                                    },
                                                );
                                            }
                                        } else if primary_emerald_button(
                                            ui,
                                            Some(VectorIcon::Download),
                                            "Скачать",
                                        )
                                        .clicked()
                                            && !state.is_launching
                                        {
                                            state.is_launching = true;
                                            let _ = state.cmd_sender.send(
                                                UiWorkerCommand::LaunchInstance {
                                                    instance_id: inst_id.clone(),
                                                    dry_run: false,
                                                },
                                            );
                                        }
                                    },
                                );
                            });
                        },
                        );
                        ui.add_space(4.0);
                    } else {
                        let det_style = ThemeStyleParams::active(ui.ctx());
                        let det_r = det_style.card_corner_radius(state.config.theme.corner_radius);
                        let header_frame =
                            crate::theme::solid_card_frame_with_config(&tokens, &state.config.theme)
                                .inner_margin(egui::Margin::same(8));

                        show_themed_card(
                            ui,
                            header_frame,
                            det_r,
                            ui.make_persistent_id(("inst_full_hdr_glass", inst_id.as_str())),
                            &det_style,
                            &tokens,
                            |ui| {
                            let render_hero_button = |ui: &mut egui::Ui, state: &mut UiState| {
                                if is_running {
                                    if hero_action_button(
                                        ui,
                                        VectorIcon::Stop,
                                        "ОСТАНОВИТЬ",
                                        STATUS_DANGER,
                                    )
                                    .clicked()
                                    {
                                        let _ = state.cmd_sender.send(UiWorkerCommand::StopGame {
                                            instance_id: inst_id.clone(),
                                        });
                                    }
                                } else if is_downloading {
                                    if hero_action_button(
                                        ui,
                                        VectorIcon::Download,
                                        "СКАЧИВАНИЕ...",
                                        ACCENT_COBALT,
                                    )
                                    .clicked()
                                    {
                                        let _ = state
                                            .cmd_sender
                                            .send(UiWorkerCommand::CancelDownload);
                                    }
                                } else if is_installed {
                                    let btn_text = if state.is_launching {
                                        "Запуск..."
                                    } else {
                                        "Играть"
                                    };
                                    if hero_action_button(
                                        ui,
                                        VectorIcon::Play,
                                        btn_text,
                                        ACCENT_EMERALD,
                                    )
                                    .clicked()
                                        && !state.is_launching
                                        && !is_running
                                    {
                                        state.is_launching = true;
                                        let _ = state.cmd_sender.send(
                                            UiWorkerCommand::LaunchInstance {
                                                instance_id: inst_id.clone(),
                                                dry_run: false,
                                            },
                                        );
                                    }
                                } else {
                                    let btn_text = if state.is_launching {
                                        "Скачивание и запуск..."
                                    } else {
                                        "Скачать и играть"
                                    };
                                    if hero_action_button(
                                        ui,
                                        VectorIcon::Download,
                                        btn_text,
                                        ACCENT_EMERALD,
                                    )
                                    .clicked()
                                        && !state.is_launching
                                        && !is_running
                                    {
                                        state.is_launching = true;
                                        let _ = state.cmd_sender.send(
                                            UiWorkerCommand::LaunchInstance {
                                                instance_id: inst_id.clone(),
                                                dry_run: false,
                                            },
                                        );
                                    }
                                }
                            };

                            // Row 1: Full-width Instance Title (Play button lives inline on the left of Row 3)
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&inst_name)
                                        .color(TEXT_PRIMARY)
                                        .size(20.0)
                                        .strong(),
                                )
                                .truncate(),
                            );

                            ui.add_space(6.0);

                            // Row 2: Loader & version text + neutral RAM badge + status (B8)
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                                let loader_str = match loader {
                                    ModLoaderKind::Vanilla => "Vanilla",
                                    ModLoaderKind::Fabric => "Fabric",
                                    ModLoaderKind::Quilt => "Quilt",
                                    ModLoaderKind::Forge => "Forge",
                                    ModLoaderKind::NeoForge => "NeoForge",
                                };
                                ui.label(
                                    RichText::new(format!("{loader_str} · {mc_ver}"))
                                        .font(egui::FontId::proportional(13.0))
                                        .color(TEXT_PRIMARY),
                                );

                                let ram_str = if memory_max_mb >= 1024 {
                                    format!("{:.0} ГБ ОЗУ", memory_max_mb as f32 / 1024.0)
                                } else {
                                    format!("{memory_max_mb} МБ ОЗУ")
                                };
                                render_badge(ui, &tokens, &ram_str, BadgeKind::Neutral);

                                let playtime_secs = state
                                    .instances
                                    .iter()
                                    .find(|i| i.id == inst_id)
                                    .map(|i| i.total_playtime_secs)
                                    .unwrap_or(0);
                                let playtime_str = if playtime_secs >= 3600 {
                                    format!(
                                        "Время в игре: {} ч {} мин",
                                        playtime_secs / 3600,
                                        (playtime_secs % 3600) / 60
                                    )
                                } else if playtime_secs >= 60 {
                                    format!("Время в игре: {} мин", playtime_secs / 60)
                                } else if playtime_secs > 0 {
                                    format!("Время в игре: {playtime_secs} сек")
                                } else {
                                    "Время в игре: 0 мин".to_string()
                                };
                                render_badge(ui, &tokens, &playtime_str, BadgeKind::Neutral);

                                if is_running {
                                    status_pill(ui, "Игра запущена", ACCENT_EMERALD);
                                } else if is_downloading {
                                    status_pill(ui, "Скачивание файлов...", ACCENT_COBALT);
                                } else if is_installed {
                                    status_pill(ui, "Готов к запуску", ACCENT_EMERALD);
                                } else {
                                    status_pill(ui, "Требуется установка", STATUS_WARN);
                                }
                            });

                            ui.add_space(8.0);

                            // Row 3: Actions toolbar — Play button FIRST, directly to the LEFT of "Папка игры"
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);

                                render_hero_button(ui, state);

                                if secondary_glass_button(
                                    ui,
                                    None,
                                    if state.mini_overlay_active {
                                        "Полный режим"
                                    } else {
                                        "Мини-оверлей"
                                    },
                                )
                                .on_hover_text("Переключить компактный плавающий мини-оверлей поверх окон")
                                .clicked()
                                {
                                    state.mini_overlay_active = !state.mini_overlay_active;
                                }

                                if secondary_glass_button(
                                    ui,
                                    Some(VectorIcon::Folder),
                                    "Папка игры",
                                )
                                .on_hover_text("Открыть директорию инстанса в файловом менеджере")
                                .clicked()
                                {
                                    let inst_dir = state.paths.instance_dir(inst_id.as_str());
                                    let _ = crate::opener::open_that(inst_dir);
                                }

                                let can_maintain = !is_downloading && !is_running;
                                if is_installed {
                                    if ui
                                        .add_enabled_ui(can_maintain, |ui| {
                                            secondary_glass_button(
                                                ui,
                                                Some(VectorIcon::Verify),
                                                "Проверить",
                                            )
                                            .on_hover_text(
                                                "Проверить целостность и починить повреждённые файлы",
                                            )
                                        })
                                        .inner
                                        .clicked()
                                    {
                                        let _ = state.cmd_sender.send(
                                            UiWorkerCommand::VerifyAndRepairInstance {
                                                instance_id: inst_id.clone(),
                                            },
                                        );
                                    }
                                    if ui
                                        .add_enabled_ui(can_maintain, |ui| {
                                            secondary_glass_button(
                                                ui,
                                                Some(VectorIcon::Refresh),
                                                "Переустановить",
                                            )
                                            .on_hover_text(
                                                "Переустановить версию без потери миров (saves) и настроек (options.txt)",
                                            )
                                        })
                                        .inner
                                        .clicked()
                                    {
                                        let _ = state.cmd_sender.send(
                                            UiWorkerCommand::ReinstallInstance {
                                                instance_id: inst_id.clone(),
                                            },
                                        );
                                    }
                                }

                                // Secondary actions menu
                                let menu_id =
                                    ui.make_persistent_id(format!("inst_menu_{}", inst_id.as_str()));
                                let more_btn = secondary_glass_button(ui, None, "•••");
                                if more_btn.clicked() {
                                    ui.memory_mut(|mem| mem.toggle_popup(menu_id));
                                }

                                crate::animation::show_animated_dropdown_popup(
                                    ui,
                                    menu_id,
                                    &more_btn,
                                    170.0,
                                    false,
                                    true,
                                    |ui| {
                                        ui.set_min_width(160.0);
                                        if !is_installed
                                            && !is_downloading
                                            && ui.button("Установить без запуска").clicked()
                                        {
                                            let _ = state.cmd_sender.send(
                                                UiWorkerCommand::InstallInstance {
                                                    instance_id: inst_id.clone(),
                                                },
                                            );
                                            ui.memory_mut(|mem| mem.close_popup());
                                        }
                                        if ui.button("Папка модов").clicked() {
                                            let mods_dir = state
                                                .paths
                                                .instance_dir(inst_id.as_str())
                                                .join("mods");
                                            let _ = std::fs::create_dir_all(&mods_dir);
                                            let _ = crate::opener::open_that(mods_dir);
                                            ui.memory_mut(|mem| mem.close_popup());
                                        }
                                        if is_installed
                                            && can_maintain
                                            && ui.button("Переустановить").clicked()
                                        {
                                            let _ = state.cmd_sender.send(
                                                UiWorkerCommand::ReinstallInstance {
                                                    instance_id: inst_id.clone(),
                                                },
                                            );
                                            ui.memory_mut(|mem| mem.close_popup());
                                        }
                                        if ui.button("Экспорт").clicked() {
                                            state.status_message = Some((
                                                "Экспорт инстанса в архив...".to_string(),
                                                false,
                                            ));
                                            let _ = state.cmd_sender.send(
                                                UiWorkerCommand::ExportInstance {
                                                    instance_id: inst_id.clone(),
                                                },
                                            );
                                            ui.memory_mut(|mem| mem.close_popup());
                                        }
                                        if ui.button("Дублировать").clicked() {
                                            self.show_duplicate_dialog = true;
                                            self.duplicate_name = format!("{} (Копия)", inst_name);
                                            ui.memory_mut(|mem| mem.close_popup());
                                        }
                                        ui.separator();
                                        if ui
                                            .button(
                                                RichText::new("Удалить инстанс")
                                                    .color(STATUS_DANGER),
                                            )
                                            .clicked()
                                        {
                                            self.show_delete_dialog = true;
                                            self.instance_to_delete =
                                                Some((inst_id.clone(), inst_name.clone()));
                                            ui.memory_mut(|mem| mem.close_popup());
                                        }
                                    },
                                );

                                if danger_button(ui, Some(VectorIcon::Trash), "Удалить")
                                    .on_hover_text("Удалить этот инстанс")
                                    .clicked()
                                {
                                    self.show_delete_dialog = true;
                                    self.instance_to_delete =
                                        Some((inst_id.clone(), inst_name.clone()));
                                }
                            });
                        },
                        );

                        ui.add_space(4.0);
                    }

                    // Tab selector with underline tabs (B5)
                    let detail_tabs = [
                        (InstanceDetailTab::Console, "Консоль"),
                        (InstanceDetailTab::Mods, "Моды"),
                        (InstanceDetailTab::ResourcePacks, "Ресурспаки"),
                        (InstanceDetailTab::ShaderPacks, "Шейдеры"),
                        (InstanceDetailTab::Worlds, "Миры"),
                        (InstanceDetailTab::Servers, "Серверы"),
                        (InstanceDetailTab::Screenshots, "Скриншоты"),
                        (InstanceDetailTab::Settings, "Параметры"),
                    ];
                    let prev_tab = self.current_tab;
                    render_underline_tabs(ui, &tokens, &detail_tabs, &mut self.current_tab);
                    if self.current_tab != prev_tab {
                        self.content_sub_mode = ContentSubMode::Installed;
                        self.local_file_input.clear();
                        if self.current_tab == InstanceDetailTab::Mods {
                            let _ = state.cmd_sender.send(UiWorkerCommand::ScanInstanceMods {
                                instance_id: inst_id.clone(),
                            });
                        }
                    }

                    ui.separator();

                    // Tab contents
                    crate::animation::render_tab_transition(
                        ui,
                        ("mq_inst_detail_tab_trans", inst_id.as_str()),
                        self.current_tab,
                        self.current_tab.order_index(),
                        true,
                        |ui| match self.current_tab {
                            InstanceDetailTab::Console => {
                                self.render_console_tab(ui, state, &inst_id);
                            }
                            InstanceDetailTab::Mods => {
                                self.render_mods_tab(ui, state, &inst_id);
                            }
                            InstanceDetailTab::ResourcePacks => {
                                self.render_resourcepacks_tab(ui, state, &inst_id);
                            }
                            InstanceDetailTab::ShaderPacks => {
                                self.render_shaderpacks_tab(ui, state, &inst_id);
                            }
                            InstanceDetailTab::Worlds => {
                                self.render_worlds_tab(ui, state, &inst_id);
                            }
                            InstanceDetailTab::Servers => {
                                self.render_servers_tab(ui, state, &inst_id);
                            }
                            InstanceDetailTab::Screenshots => {
                                self.render_screenshots_tab(ui, state, &inst_id);
                            }
                            InstanceDetailTab::Settings => {
                                self.render_settings_tab(ui, state, &inst_id);
                            }
                        },
                    );
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            RichText::new("Выберите инстанс из списка слева").color(TEXT_MUTED),
                        );
                    });
                }
            });
        });
            },
        );
    }

    fn filtered_instances(&self, state: &UiState) -> Vec<InstanceManifest> {
        let q = self.search_query.trim().to_lowercase();
        let mut list: Vec<InstanceManifest> = state
            .instances
            .iter()
            .filter(|inst| self.loader_filter.matches(inst.loader))
            .filter(|inst| {
                q.is_empty()
                    || inst.name.to_lowercase().contains(&q)
                    || inst.mc_version.to_lowercase().contains(&q)
                    || inst.loader.as_str().to_lowercase().contains(&q)
            })
            .cloned()
            .collect();

        match self.sort_mode {
            InstanceSortMode::Playtime => {
                list.sort_by_key(|inst| std::cmp::Reverse(inst.playtime_secs));
            }
            InstanceSortMode::Name => {
                list.sort_by_key(|inst| inst.name.to_lowercase());
            }
            InstanceSortMode::Created => {
                list.sort_by_key(|inst| std::cmp::Reverse(inst.created_at));
            }
        }
        list
    }

    fn render_grid_view(&mut self, ui: &mut Ui, state: &mut UiState, tokens: &Tokens) {
        let instances = self.filtered_instances(state);
        if instances.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    RichText::new("Инстансы не найдены")
                        .size(15.0)
                        .color(tokens.text_muted),
                );
                ui.add_space(8.0);
                if button_secondary(ui, tokens, None, "Сбросить фильтры").clicked() {
                    self.search_query.clear();
                    self.loader_filter = LoaderFilter::All;
                }
            });
            return;
        }

        ScrollArea::vertical()
            .id_salt("instances_grid_scroll")
            .show(ui, |ui| {
                let avail_w = ui.available_width();
                let spacing = 16.0_f32;
                let min_outer_w = 350.0_f32;
                let cols = ((avail_w + spacing) / (min_outer_w + spacing))
                    .floor()
                    .max(1.0) as usize;
                // Account for card_frame inner_margin (14*2) and 1px stroke on each side.
                let outer_w = (avail_w - spacing * (cols as f32 - 1.0)) / cols as f32;
                let card_w = (outer_w - 30.0).max(280.0);

                for chunk in instances.chunks(cols) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(spacing, spacing);
                        for inst in chunk {
                            self.render_instance_grid_card(ui, state, inst, tokens, card_w);
                        }
                    });
                    ui.add_space(spacing);
                }
            });
    }

    fn render_instance_grid_card(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        inst: &InstanceManifest,
        tokens: &Tokens,
        card_w: f32,
    ) {
        let is_selected = state.selected_instance_id.as_ref() == Some(&inst.id);
        let is_installed = state.is_instance_installed(&inst.id);
        let is_downloading = state.installing_instance.as_ref() == Some(&inst.id);
        let is_running = state.running_instances.contains_key(&inst.id);

        let style = ThemeStyleParams::active(ui.ctx());
        let border_stroke = if style.refraction_border {
            Stroke::NONE
        } else if is_running || is_selected {
            Stroke::new(1.5_f32, tokens.accent)
        } else if is_downloading {
            Stroke::new(1.5_f32, ACCENT_COBALT)
        } else {
            Stroke::new(1.0_f32, tokens.border)
        };

        let bg_color = if is_selected {
            style.surface_hover_fill(tokens)
        } else {
            style.surface_fill(tokens)
        };

        let card_radius = style.card_corner_radius(state.config.theme.corner_radius);

        let card_frame = egui::Frame::new()
            .fill(bg_color)
            .stroke(border_stroke)
            .corner_radius(card_radius)
            .inner_margin(egui::Margin::same(14));

        let card_resp = show_themed_card(
            ui,
            card_frame,
            card_radius,
            ui.make_persistent_id(("inst_grid_card_glass", inst.id.as_str())),
            &style,
            tokens,
            |ui| {
                ui.set_width(card_w);
                ui.vertical(|ui| {
                    // Clickable top region (above the separator) so card clicks/double-clicks
                    // never overlay or intercept clicks on the action buttons below.
                    let header_resp = ui.vertical(|ui| {
                        ui.set_width(card_w);
                        // Top row: Vector Icon + Name & Version/Loader Badges
                        ui.horizontal(|ui| {
                            let (icon_rect, _) =
                                ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
                            let (icon, icon_col) = match inst.loader {
                                ModLoaderKind::Vanilla => (VectorIcon::Cube, tokens.text),
                                ModLoaderKind::Fabric | ModLoaderKind::Quilt => {
                                    (VectorIcon::Layers, tokens.accent)
                                }
                                ModLoaderKind::Forge | ModLoaderKind::NeoForge => {
                                    (VectorIcon::Shield, ACCENT_COBALT)
                                }
                            };
                            paint_vector_icon(ui.painter(), icon_rect, icon, icon_col);

                            ui.vertical(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&inst.name)
                                            .color(tokens.text)
                                            .size(15.0)
                                            .strong(),
                                    )
                                    .truncate(),
                                );
                                ui.add_space(2.0);
                                ui.horizontal(|ui| {
                                    version_badge(ui, &inst.mc_version, ACCENT_COBALT);
                                    loader_badge(ui, inst.loader);
                                });
                            });
                        });

                        ui.add_space(10.0);

                        // Status row: status dot + status label + playtime on right
                        let (status_text, status_col) = if is_running {
                            ("Запущен", tokens.accent)
                        } else if is_downloading {
                            ("Загрузка...", ACCENT_COBALT)
                        } else if is_installed {
                            ("Готов", tokens.accent)
                        } else {
                            ("Не скачан", tokens.warning)
                        };

                        ui.horizontal(|ui| {
                            let (dot_rect, _) =
                                ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                            ui.painter()
                                .circle_filled(dot_rect.center(), 3.5, status_col);
                            ui.label(
                                RichText::new(status_text)
                                    .color(status_col)
                                    .small()
                                    .strong(),
                            );

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let hours = (inst.playtime_secs as f32) / 3600.0;
                                    if hours >= 0.1 {
                                        ui.label(
                                            RichText::new(format!("Время: {:.1} ч", hours))
                                                .color(tokens.text_muted)
                                                .small(),
                                        );
                                    } else {
                                        ui.label(
                                            RichText::new("Новый").color(tokens.text_muted).small(),
                                        );
                                    }
                                },
                            );
                        });
                    });

                    let card_resp = header_resp.response.interact(Sense::click());
                    if card_resp.double_clicked() {
                        state.selected_instance_id = Some(inst.id.clone());
                        self.view_mode = InstanceViewMode::List;
                        self.content_sub_mode = ContentSubMode::Installed;
                        self.synced_catalog_context = None;
                        let _ = state.cmd_sender.send(UiWorkerCommand::ScanInstanceMods {
                            instance_id: inst.id.clone(),
                        });
                    } else if card_resp.clicked() {
                        state.selected_instance_id = Some(inst.id.clone());
                        let _ = state.cmd_sender.send(UiWorkerCommand::ScanInstanceMods {
                            instance_id: inst.id.clone(),
                        });
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // Bottom row: Actions
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                        if is_running {
                            if primary_emerald_button(ui, Some(VectorIcon::Stop), "Стоп").clicked()
                            {
                                let _ = state.cmd_sender.send(UiWorkerCommand::StopGame {
                                    instance_id: inst.id.clone(),
                                });
                            }
                        } else if is_downloading {
                            if primary_emerald_button(ui, Some(VectorIcon::Stop), "Стоп").clicked()
                            {
                                let _ = state.cmd_sender.send(UiWorkerCommand::CancelDownload);
                            }
                        } else if is_installed {
                            let play_label = if state.is_launching && is_selected {
                                "Запуск..."
                            } else {
                                "Играть"
                            };
                            if primary_emerald_button(ui, Some(VectorIcon::Play), play_label)
                                .clicked()
                                && !state.is_launching
                            {
                                state.selected_instance_id = Some(inst.id.clone());
                                state.is_launching = true;
                                let _ = state.cmd_sender.send(UiWorkerCommand::LaunchInstance {
                                    instance_id: inst.id.clone(),
                                    dry_run: false,
                                });
                            }
                        } else if primary_emerald_button(ui, Some(VectorIcon::Download), "Скачать")
                            .clicked()
                            && !state.is_launching
                        {
                            state.selected_instance_id = Some(inst.id.clone());
                            state.is_launching = true;
                            let _ = state.cmd_sender.send(UiWorkerCommand::LaunchInstance {
                                instance_id: inst.id.clone(),
                                dry_run: false,
                            });
                        }

                        if secondary_glass_button(ui, Some(VectorIcon::Layers), "Моды").clicked()
                        {
                            state.selected_instance_id = Some(inst.id.clone());
                            self.view_mode = InstanceViewMode::List;
                            self.current_tab = InstanceDetailTab::Mods;
                            self.content_sub_mode = ContentSubMode::Installed;
                            self.synced_catalog_context = None;
                            let _ = state.cmd_sender.send(UiWorkerCommand::ScanInstanceMods {
                                instance_id: inst.id.clone(),
                            });
                        }

                        if secondary_glass_button(ui, Some(VectorIcon::Settings), "Детали")
                            .clicked()
                        {
                            state.selected_instance_id = Some(inst.id.clone());
                            self.view_mode = InstanceViewMode::List;
                            self.current_tab = InstanceDetailTab::Console;
                            self.content_sub_mode = ContentSubMode::Installed;
                            self.synced_catalog_context = None;
                            let _ = state.cmd_sender.send(UiWorkerCommand::ScanInstanceMods {
                                instance_id: inst.id.clone(),
                            });
                        }

                        if crate::components::icon_button(
                            ui,
                            tokens,
                            VectorIcon::Folder,
                            "Открыть папку инстанса",
                        )
                        .clicked()
                        {
                            let inst_dir = state.paths.instance_dir(inst.id.as_str());
                            let _ = crate::opener::open_that(inst_dir);
                        }
                    });
                });
            },
        );
        if style.refraction_border && (is_running || is_selected || is_downloading) {
            let halo_col = if is_downloading {
                ACCENT_COBALT
            } else {
                tokens.accent
            };
            crate::theme_styles::paint_liquid_glass_selection_halo(
                ui.painter(),
                card_resp.response.rect,
                card_radius,
                halo_col,
            );
        }
    }

    fn render_create_instance_modal(&mut self, ui: &mut Ui, state: &mut UiState, tokens: &Tokens) {
        let modal_id = egui::Id::new("mq_create_instance_modal");
        if state.show_create_instance_dialog
            && ui
                .ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            state.show_create_instance_dialog = false;
        }
        let anim = crate::animation::animate_modal_presence(
            ui.ctx(),
            modal_id,
            state.show_create_instance_dialog,
        );
        if !anim.should_render {
            return;
        }

        if state.show_create_instance_dialog
            && state.remote_versions.is_empty()
            && !state.is_fetching_manifest
        {
            state.is_fetching_manifest = true;
            let _ = state.cmd_sender.send(UiWorkerCommand::FetchVersionManifest);
        }

        if state.create_inst_mc_ver.is_empty() {
            if let Some(latest) = &state.latest_release {
                state.create_inst_mc_ver = latest.clone();
            }
        }

        let effective_ver = if !state.create_inst_mc_ver.is_empty() {
            state.create_inst_mc_ver.clone()
        } else {
            state
                .latest_release
                .clone()
                .unwrap_or_else(|| ALL_MC_RELEASES[0].to_string())
        };
        let auto_name = format!("{} {}", state.create_inst_loader.as_str(), effective_ver);

        let screen = ui.ctx().screen_rect();
        let modal_frame = egui::Frame::window(ui.style())
            .fill(tokens.bg_surface)
            .stroke(Stroke::new(1.0_f32, tokens.border_strong))
            .corner_radius(RADIUS_CARD)
            .inner_margin(egui::Margin::same(20));

        let body_max_h = (screen.height() - 160.0).clamp(220.0, 520.0);

        fn modal_chip_pill(ui: &mut Ui, tokens: &Tokens, selected: bool, label: &str) -> bool {
            let fill = if selected {
                blend_color(
                    tokens.bg_surface,
                    tokens.accent,
                    if tokens.is_light() { 0.16 } else { 0.20 },
                )
            } else {
                tokens.bg_input
            };
            let stroke = if selected {
                tokens.accent
            } else {
                tokens.border
            };
            let fg = if selected {
                tokens.text
            } else {
                tokens.text_muted
            };
            let resp = ui.add(
                egui::Button::new(RichText::new(label).color(fg).size(12.5))
                    .fill(fill)
                    .stroke(Stroke::new(1.0_f32, stroke))
                    .corner_radius(RADIUS_TAG),
            );
            crate::components::registry::register_widget(
                ui,
                &resp,
                crate::components::registry::ElementMeta::new(
                    crate::components::registry::ElementKind::Tab,
                )
                .label(label)
                .active(selected)
                .fg(fg)
                .bg(fill),
            );
            resp.clicked()
        }

        let window = egui::Window::new("Создать инстанс")
            .frame(modal_frame)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]);
        crate::animation::show_animated_modal_window(
            ui.ctx(),
            modal_id.with("win"),
            &anim,
            window,
            |ui| {
                let modal_w = 480.0_f32.min(screen.width() - 32.0);
                ui.set_width(modal_w);

                ScrollArea::vertical()
                    .id_salt("create_instance_modal_body")
                    .max_height(body_max_h)
                    .show(ui, |ui| {
                        // 1. Instance Name with auto-placeholder
                        ui.label(RichText::new("Название").color(tokens.text).strong());
                        ui.add_space(4.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut state.create_inst_name)
                                .hint_text(&auto_name)
                                .desired_width(ui.available_width()),
                        );

                        ui.add_space(10.0);

                        // 2. Mod Loader Selection
                        ui.label(RichText::new("Загрузчик").color(tokens.text).strong());
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                            for &ldr in &[
                                ModLoaderKind::Vanilla,
                                ModLoaderKind::Fabric,
                                ModLoaderKind::Quilt,
                                ModLoaderKind::Forge,
                                ModLoaderKind::NeoForge,
                            ] {
                                let is_sel = state.create_inst_loader == ldr;
                                if modal_chip_pill(ui, tokens, is_sel, ldr.as_str()) {
                                    state.create_inst_loader = ldr;
                                }
                            }
                        });

                        ui.add_space(10.0);

                        // 3. Minecraft Version Selection
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Версия Minecraft")
                                    .color(tokens.text)
                                    .strong(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if state.is_fetching_manifest {
                                        ui.label(
                                            RichText::new("Загрузка...")
                                                .color(tokens.text_muted)
                                                .small(),
                                        );
                                    } else if button_secondary(
                                        ui,
                                        tokens,
                                        Some(VectorIcon::Refresh),
                                        "Обновить",
                                    )
                                    .clicked()
                                    {
                                        state.is_fetching_manifest = true;
                                        let _ = state
                                            .cmd_sender
                                            .send(UiWorkerCommand::FetchVersionManifest);
                                    }
                                },
                            );
                        });
                        ui.add_space(4.0);

                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(10.0, 4.0);
                            ui.checkbox(
                                &mut self.show_releases,
                                RichText::new("Релизы").color(tokens.text),
                            );
                            ui.checkbox(
                                &mut self.show_snapshots,
                                RichText::new("Снапшоты").color(tokens.text),
                            );
                            ui.checkbox(
                                &mut self.show_betas,
                                RichText::new("Beta").color(tokens.text),
                            );
                            ui.checkbox(
                                &mut self.show_alphas,
                                RichText::new("Alpha").color(tokens.text),
                            );
                        });

                        ui.add_space(4.0);

                        ui.horizontal(|ui| {
                            let search_w = if self.version_search.is_empty() {
                                ui.available_width()
                            } else {
                                (ui.available_width() - 68.0).max(120.0)
                            };
                            let search_resp = version_search_input(
                                ui,
                                &mut self.version_search,
                                search_w,
                                tokens,
                            );
                            crate::components::registry::register_widget(
                                ui,
                                &search_resp,
                                crate::components::registry::ElementMeta::new(
                                    crate::components::registry::ElementKind::Input,
                                )
                                .label("Поиск версии")
                                .text(self.version_search.as_str())
                                .fg(tokens.text)
                                .bg(tokens.bg_input),
                            );
                            if !self.version_search.is_empty()
                                && button_secondary(ui, tokens, None, "Сброс").clicked()
                            {
                                self.version_search.clear();
                            }
                        });

                        ui.add_space(6.0);

                        let query = self.version_search.trim().to_lowercase();
                        let mut filtered: Vec<(&str, McVersionCategory)> = Vec::new();

                        if !state.remote_versions.is_empty() {
                            for (id, rel_type) in &state.remote_versions {
                                let cat = match rel_type.as_str() {
                                    "release" => McVersionCategory::Release,
                                    "snapshot" => McVersionCategory::Snapshot,
                                    "old_beta" => McVersionCategory::OldBeta,
                                    "old_alpha" => McVersionCategory::OldAlpha,
                                    _ => continue,
                                };
                                let matches_cat = match cat {
                                    McVersionCategory::Release => self.show_releases,
                                    McVersionCategory::Snapshot => self.show_snapshots,
                                    McVersionCategory::OldBeta => self.show_betas,
                                    McVersionCategory::OldAlpha => self.show_alphas,
                                };
                                if !matches_cat {
                                    continue;
                                }
                                if !query.is_empty() && !id.to_lowercase().contains(&query) {
                                    continue;
                                }
                                filtered.push((id.as_str(), cat));
                            }
                        } else if !state.is_fetching_manifest {
                            if self.show_releases {
                                for &v in ALL_MC_RELEASES {
                                    if query.is_empty() || v.to_lowercase().contains(&query) {
                                        filtered.push((v, McVersionCategory::Release));
                                    }
                                }
                            }
                            if self.show_snapshots {
                                for &v in ALL_MC_SNAPSHOTS {
                                    if query.is_empty() || v.to_lowercase().contains(&query) {
                                        filtered.push((v, McVersionCategory::Snapshot));
                                    }
                                }
                            }
                            if self.show_betas {
                                for &v in ALL_MC_BETAS {
                                    if query.is_empty() || v.to_lowercase().contains(&query) {
                                        filtered.push((v, McVersionCategory::OldBeta));
                                    }
                                }
                            }
                            if self.show_alphas {
                                for &v in ALL_MC_ALPHAS {
                                    if query.is_empty() || v.to_lowercase().contains(&query) {
                                        filtered.push((v, McVersionCategory::OldAlpha));
                                    }
                                }
                            }
                        }

                        let picker_frame = egui::Frame::new()
                            .fill(tokens.bg_input)
                            .stroke(Stroke::new(1.0_f32, tokens.border))
                            .corner_radius(RADIUS_CARD)
                            .inner_margin(egui::Margin::same(6));

                        picker_frame.show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ScrollArea::vertical()
                                .id_salt("version_select_scroll")
                                .max_height(154.0)
                                .show(ui, |ui| {
                                    if state.is_fetching_manifest
                                        && state.remote_versions.is_empty()
                                    {
                                        for width_frac in [0.55_f32, 0.42, 0.62, 0.48] {
                                            let avail_w = ui.available_width();
                                            let (rect, _) = ui.allocate_exact_size(
                                                Vec2::new(avail_w * width_frac, 18.0),
                                                Sense::hover(),
                                            );
                                            ui.painter().rect_filled(
                                                rect,
                                                4.0,
                                                tokens.bg_surface_hover,
                                            );
                                            ui.add_space(4.0);
                                        }
                                    } else if filtered.is_empty() {
                                        ui.add_space(8.0);
                                        ui.label(
                                            RichText::new("Нет версий, соответствующих фильтрам")
                                                .color(tokens.text_muted),
                                        );
                                    } else {
                                        for (vid, cat) in filtered {
                                            let is_sel = effective_ver == vid;
                                            let is_latest = state.latest_release.as_deref()
                                                == Some(vid)
                                                || (state.remote_versions.is_empty()
                                                    && vid == ALL_MC_RELEASES[0]);

                                            let row_w = ui.available_width().max(180.0);
                                            let (row_rect, row_resp) = ui.allocate_exact_size(
                                                Vec2::new(row_w, 28.0),
                                                Sense::click(),
                                            );
                                            if row_resp.hovered() {
                                                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                                            }

                                            if ui.is_rect_visible(row_rect) {
                                                let painter = ui.painter();
                                                let row_fill = if is_sel {
                                                    blend_color(
                                                        tokens.bg_surface,
                                                        tokens.accent,
                                                        if tokens.is_light() { 0.14 } else { 0.18 },
                                                    )
                                                } else if row_resp.hovered() {
                                                    tokens.bg_surface_hover
                                                } else {
                                                    Color32::TRANSPARENT
                                                };
                                                if row_fill != Color32::TRANSPARENT {
                                                    painter.rect_filled(
                                                        row_rect, RADIUS_TAG, row_fill,
                                                    );
                                                }
                                                if is_sel {
                                                    painter.rect_stroke(
                                                        row_rect,
                                                        RADIUS_TAG,
                                                        Stroke::new(1.0_f32, tokens.accent),
                                                        StrokeKind::Inside,
                                                    );
                                                    let bar = Rect::from_min_max(
                                                        Pos2::new(
                                                            row_rect.left() + 4.0,
                                                            row_rect.top() + 5.0,
                                                        ),
                                                        Pos2::new(
                                                            row_rect.left() + 7.0,
                                                            row_rect.bottom() - 5.0,
                                                        ),
                                                    );
                                                    painter.rect_filled(
                                                        bar,
                                                        CornerRadius::same(2),
                                                        tokens.accent,
                                                    );
                                                } else if row_resp.hovered() {
                                                    painter.rect_stroke(
                                                        row_rect,
                                                        RADIUS_TAG,
                                                        Stroke::new(1.0_f32, tokens.border_strong),
                                                        StrokeKind::Inside,
                                                    );
                                                }

                                                let ver_col = if is_sel || row_resp.hovered() {
                                                    tokens.text
                                                } else {
                                                    blend_color(
                                                        tokens.text_muted,
                                                        tokens.text,
                                                        0.55,
                                                    )
                                                };
                                                let ver_galley = painter.layout_no_wrap(
                                                    vid.to_owned(),
                                                    FontId::proportional(13.5),
                                                    ver_col,
                                                );
                                                let ver_pos = Pos2::new(
                                                    row_rect.left()
                                                        + if is_sel { 14.0 } else { 10.0 },
                                                    row_rect.center().y - ver_galley.size().y * 0.5,
                                                );
                                                painter.galley(ver_pos, ver_galley, ver_col);
                                                crate::components::registry::register_widget(
                                                    ui,
                                                    &row_resp,
                                                    crate::components::registry::ElementMeta::new(
                                                        crate::components::registry::ElementKind::Button,
                                                    )
                                                    .label(vid)
                                                    .active(is_sel)
                                                    .fg(ver_col)
                                                    .bg(if row_fill == Color32::TRANSPARENT {
                                                        tokens.bg_input
                                                    } else {
                                                        row_fill
                                                    }),
                                                );

                                                let pad = Vec2::new(6.0, 2.0);
                                                let cat_col = cat.badge_color();
                                                let badge_fg = if tokens.is_light() {
                                                    blend_color(cat_col, tokens.text, 0.35)
                                                } else {
                                                    cat_col
                                                };
                                                let badge_bg = if tokens.is_light() {
                                                    blend_color(tokens.bg_surface, cat_col, 0.14)
                                                } else {
                                                    Color32::from_rgba_unmultiplied(
                                                        cat_col.r(),
                                                        cat_col.g(),
                                                        cat_col.b(),
                                                        32,
                                                    )
                                                };
                                                let badge_border = if tokens.is_light() {
                                                    blend_color(tokens.border, cat_col, 0.45)
                                                } else {
                                                    Color32::from_rgba_unmultiplied(
                                                        cat_col.r(),
                                                        cat_col.g(),
                                                        cat_col.b(),
                                                        110,
                                                    )
                                                };
                                                let cat_galley = painter.layout_no_wrap(
                                                    cat.badge_label().to_owned(),
                                                    FontId::monospace(10.5),
                                                    badge_fg,
                                                );
                                                let cat_size = cat_galley.size() + pad * 2.0;
                                                let mut cursor_right = row_rect.right() - 8.0;
                                                let cat_rect = Rect::from_min_size(
                                                    Pos2::new(
                                                        cursor_right - cat_size.x,
                                                        row_rect.center().y - cat_size.y * 0.5,
                                                    ),
                                                    cat_size,
                                                );
                                                painter.rect_filled(cat_rect, RADIUS_TAG, badge_bg);
                                                painter.rect_stroke(
                                                    cat_rect,
                                                    RADIUS_TAG,
                                                    Stroke::new(1.0_f32, badge_border),
                                                    StrokeKind::Inside,
                                                );
                                                painter.galley(
                                                    cat_rect.min + pad,
                                                    cat_galley,
                                                    badge_fg,
                                                );
                                                cursor_right = cat_rect.left() - 6.0;

                                                if is_latest && cat == McVersionCategory::Release {
                                                    let lat_fg = if tokens.is_light() {
                                                        blend_color(
                                                            tokens.accent,
                                                            tokens.text,
                                                            0.35,
                                                        )
                                                    } else {
                                                        tokens.accent
                                                    };
                                                    let lat_bg = blend_color(
                                                        tokens.bg_surface,
                                                        tokens.accent,
                                                        0.16,
                                                    );
                                                    let lat_border = blend_color(
                                                        tokens.border,
                                                        tokens.accent,
                                                        0.50,
                                                    );
                                                    let lat_galley = painter.layout_no_wrap(
                                                        "Актуальная".to_owned(),
                                                        FontId::monospace(10.0),
                                                        lat_fg,
                                                    );
                                                    let lat_size = lat_galley.size() + pad * 2.0;
                                                    let lat_rect = Rect::from_min_size(
                                                        Pos2::new(
                                                            cursor_right - lat_size.x,
                                                            row_rect.center().y - lat_size.y * 0.5,
                                                        ),
                                                        lat_size,
                                                    );
                                                    painter
                                                        .rect_filled(lat_rect, RADIUS_TAG, lat_bg);
                                                    painter.rect_stroke(
                                                        lat_rect,
                                                        RADIUS_TAG,
                                                        Stroke::new(1.0_f32, lat_border),
                                                        StrokeKind::Inside,
                                                    );
                                                    painter.galley(
                                                        lat_rect.min + pad,
                                                        lat_galley,
                                                        lat_fg,
                                                    );
                                                }
                                            }

                                            if row_resp.clicked() {
                                                state.create_inst_mc_ver = vid.to_string();
                                            }
                                            ui.add_space(2.0);
                                        }
                                    }
                                });
                        });

                        ui.add_space(10.0);

                        // 4. Collapsible Advanced Block (Memory + Java)
                        egui::CollapsingHeader::new(
                            RichText::new("Дополнительно").color(tokens.text),
                        )
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "Память: {} МБ ({:.1} ГБ)",
                                    state.create_inst_ram_mb,
                                    state.create_inst_ram_mb as f32 / 1024.0
                                ))
                                .color(tokens.text)
                                .small(),
                            );
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                                for &(mb, lbl) in &[
                                    (2048, "2 ГБ"),
                                    (4096, "4 ГБ"),
                                    (6144, "6 ГБ"),
                                    (8192, "8 ГБ"),
                                    (12288, "12 ГБ"),
                                    (16384, "16 ГБ"),
                                ] {
                                    if modal_chip_pill(
                                        ui,
                                        tokens,
                                        state.create_inst_ram_mb == mb,
                                        lbl,
                                    ) {
                                        state.create_inst_ram_mb = mb;
                                    }
                                }
                            });
                            ui.add_space(4.0);
                            ui.add(
                                egui::Slider::new(&mut state.create_inst_ram_mb, 1024..=16384)
                                    .step_by(512.0)
                                    .show_value(false),
                            );

                            ui.add_space(8.0);
                            ui.label(RichText::new("Путь к Java").color(tokens.text).small());
                            ui.add(
                                egui::TextEdit::singleline(&mut state.create_inst_java_path)
                                    .hint_text("Авто (встроенная или системная Java)")
                                    .desired_width(ui.available_width()),
                            );
                        });
                    });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if button_primary(ui, tokens, Some(VectorIcon::Check), "Создать инстанс")
                        .clicked()
                    {
                        let final_name = if state.create_inst_name.trim().is_empty() {
                            auto_name.clone()
                        } else {
                            state.create_inst_name.trim().to_string()
                        };
                        match state.instance_mgr.create_instance(
                            &final_name,
                            &effective_ver,
                            state.create_inst_loader,
                            None,
                        ) {
                            Ok(mut manifest) => {
                                manifest.memory_max_mb = state.create_inst_ram_mb;
                                let custom_java = state.create_inst_java_path.trim();
                                if !custom_java.is_empty() {
                                    manifest.java_path =
                                        Some(std::path::PathBuf::from(custom_java));
                                }
                                if let Err(e) = state.instance_mgr.save_instance(&manifest) {
                                    state.status_message =
                                        Some((format!("Ошибка сохранения инстанса: {e}"), true));
                                } else {
                                    let inst_id = manifest.id.clone();
                                    state.selected_instance_id = Some(inst_id.clone());
                                    state.instances.push(manifest.clone());

                                    state.installing_instance = Some(inst_id.clone());
                                    state.install_error = None;
                                    state.detailed_install_error = None;
                                    state.current_install_stage =
                                        crate::install_types::InstallStage::Manifest;
                                    state.install_retries_count = 0;
                                    state.install_eta_secs = None;
                                    state.status_message = Some((
                                        format!(
                                            "Инстанс '{}' создан. Запущена установка...",
                                            manifest.name
                                        ),
                                        false,
                                    ));
                                    let _ =
                                        state.cmd_sender.send(UiWorkerCommand::InstallInstance {
                                            instance_id: inst_id,
                                        });
                                }
                            }
                            Err(e) => {
                                state.status_message =
                                    Some((format!("Ошибка создания инстанса: {e}"), true));
                            }
                        }
                        state.create_inst_name.clear();
                        state.create_inst_java_path.clear();
                        state.show_create_instance_dialog = false;
                    }
                    ui.add_space(8.0);
                    if button_secondary(ui, tokens, None, "Отмена").clicked() {
                        state.show_create_instance_dialog = false;
                    }
                });
            },
        );
    }

    fn render_empty_onboarding_hub(&mut self, ui: &mut Ui, state: &mut UiState, tokens: &Tokens) {
        let avail = ui.available_size();
        let card_max_w = 420.0_f32.min((avail.x - 32.0).max(240.0));
        let top_pad = ((avail.y - 240.0) * 0.4).max(24.0);

        ui.vertical_centered(|ui| {
            ui.add_space(top_pad);

            let card = egui::Frame::new()
                .fill(tokens.bg_surface)
                .stroke(Stroke::new(1.0_f32, tokens.border))
                .corner_radius(RADIUS_CARD)
                .inner_margin(egui::Margin::same(32));

            card.show(ui, |ui| {
                ui.set_max_width(card_max_w);
                ui.vertical_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::hover());
                    paint_vector_icon(ui.painter(), rect, VectorIcon::Cube, tokens.text_muted);

                    ui.add_space(16.0);
                    ui.label(
                        RichText::new("Инстансов пока нет")
                            .color(tokens.text)
                            .size(18.0)
                            .strong(),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Создайте первый инстанс или импортируйте сборку")
                            .color(tokens.text_muted)
                            .size(13.0),
                    );

                    ui.add_space(20.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let total_btn_w = 300.0_f32.min(ui.available_width());
                        let side_pad = ((ui.available_width() - total_btn_w) * 0.5).max(0.0);
                        if side_pad > 0.0 {
                            ui.add_space(side_pad);
                        }
                        if button_primary(ui, tokens, Some(VectorIcon::Plus), "Создать инстанс")
                            .clicked()
                        {
                            state.show_create_instance_dialog = true;
                        }
                        if button_secondary(ui, tokens, Some(VectorIcon::Download), "Импорт сборки")
                            .clicked()
                        {
                            self.import_error = None;
                            self.show_import_dialog = true;
                        }
                    });
                });
            });
        });
    }

    /// Lock file of an instance, re-read at most every 2 seconds.
    fn cached_lock(
        ctx: &egui::Context,
        dir: &std::path::Path,
    ) -> std::sync::Arc<mq_mods::ModLockFile> {
        type Cache = (std::time::Instant, std::sync::Arc<mq_mods::ModLockFile>);
        let id = egui::Id::new(("mq_lock_cache", dir));
        if let Some((at, lock)) = ctx.data(|d| d.get_temp::<Cache>(id)) {
            if at.elapsed() < std::time::Duration::from_secs(2) {
                return lock;
            }
        }
        let lock = std::sync::Arc::new(mq_mods::ModLockFile::load_for_instance(dir));
        ctx.data_mut(|d| d.insert_temp::<Cache>(id, (std::time::Instant::now(), lock.clone())));
        lock
    }

    /// "Проверить обновления": newer versions at the source, one-click update with rollback.
    fn updates_panel(ui: &mut Ui, state: &mut UiState, inst_id: &mq_core::InstanceId) {
        let tokens = Tokens::from_config(&state.config.theme);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
            if secondary_glass_button(ui, Some(VectorIcon::Refresh), "Проверить обновления")
                .clicked()
                && !state.updates.checking
            {
                state.check_updates(inst_id.clone());
            }
            if state.updates.instance.as_ref() == Some(inst_id)
                && state.updates.items.len() > 1
                && primary_emerald_button(
                    ui,
                    Some(VectorIcon::Download),
                    &format!("Обновить все ({})", state.updates.items.len()),
                )
                .clicked()
            {
                while !state.updates.items.is_empty() {
                    state.apply_update(0);
                }
            }
            if state.updates.checking {
                ui.label(RichText::new("Проверка...").color(tokens.text_muted));
            }
        });
        if state.updates.instance.as_ref() != Some(inst_id) {
            return;
        }
        if let Some(e) = &state.updates.error {
            ui.label(RichText::new(e).color(tokens.danger));
        }
        if state.updates.checked && state.updates.items.is_empty() && state.updates.error.is_none()
        {
            ui.label(RichText::new("Все установленные файлы актуальны").color(tokens.text_muted));
        }
        let mut apply = None;
        for (i, c) in state.updates.items.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} -> {}", c.old_filename, c.new.filename))
                        .color(tokens.text),
                );
                version_badge(
                    ui,
                    match c.source {
                        mq_mods::CatalogSource::Modrinth => "Modrinth",
                        mq_mods::CatalogSource::CurseForge => "CurseForge",
                    },
                    ACCENT_COBALT,
                );
                if secondary_glass_button(ui, None, "Обновить").clicked() {
                    apply = Some(i);
                }
            });
        }
        if let Some(i) = apply {
            state.apply_update(i);
        }
        ui.add_space(4.0);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_content_sub_mode_bar(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        inst_id: &mq_core::InstanceId,
        catalog_tab: crate::catalog_state::CatalogTab,
        installed_count: usize,
        download_cta: &str,
        folder_label: &str,
        folder_path: &std::path::Path,
        tokens: &Tokens,
    ) {
        let style = ThemeStyleParams::active(ui.ctx());
        let indicator_slot = ui.painter().add(egui::Shape::Noop);
        let mut active_pill_rect: Option<Rect> = None;

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
            for (mode, label) in [
                (
                    ContentSubMode::Installed,
                    format!("Установленные ({installed_count})"),
                ),
                (ContentSubMode::Catalog, "Каталог".to_string()),
            ] {
                let selected = self.content_sub_mode == mode;
                let font_id = egui::FontId::proportional(13.0);
                let text_w = ui
                    .painter()
                    .layout_no_wrap(label.clone(), font_id.clone(), tokens.text)
                    .size()
                    .x;
                let width = (text_w + 16.0).max(68.0);
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click());
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
                let anim = crate::animation::animate_interaction(
                    ui.ctx(),
                    resp.id.with("submode_anim"),
                    resp.hovered(),
                    resp.is_pointer_button_down_on(),
                );
                let active_t = crate::animation::animate_spring_bool(
                    ui.ctx(),
                    resp.id.with("submode_sel"),
                    selected,
                    crate::animation::SpringConfig::snappy(),
                );
                let visual_rect = if selected {
                    rect
                } else {
                    anim.transform_rect(rect)
                };
                if !selected {
                    let idle_fill = tokens.bg_surface;
                    let fill = crate::theme_styles::lerp_rgba(
                        idle_fill,
                        tokens.bg_surface_hover,
                        anim.hover_t,
                    );
                    let stroke_col = crate::theme_styles::lerp_rgba(
                        tokens.border,
                        tokens.border_strong,
                        anim.hover_t,
                    );
                    ui.painter().rect(
                        visual_rect,
                        RADIUS_TAG,
                        fill,
                        Stroke::new(1.0_f32, stroke_col),
                        StrokeKind::Inside,
                    );
                }
                let fg = crate::theme_styles::lerp_rgba(
                    tokens.text_muted,
                    tokens.text,
                    active_t.max(anim.hover_t),
                );
                ui.painter().text(
                    visual_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    font_id,
                    fg,
                );
                if selected {
                    active_pill_rect = Some(rect);
                }
                if resp.clicked() && self.content_sub_mode != mode {
                    self.content_sub_mode = mode;
                    if mode == ContentSubMode::Catalog {
                        self.synced_catalog_context = None;
                    } else if catalog_tab == crate::catalog_state::CatalogTab::Mods {
                        let _ = state.cmd_sender.send(UiWorkerCommand::ScanInstanceMods {
                            instance_id: inst_id.clone(),
                        });
                    }
                }
            }

            match self.content_sub_mode {
                ContentSubMode::Installed => {
                    if primary_emerald_button(ui, Some(VectorIcon::Plus), download_cta).clicked() {
                        self.content_sub_mode = ContentSubMode::Catalog;
                        self.synced_catalog_context = None;
                    }
                }
                ContentSubMode::Catalog => {
                    if secondary_glass_button(
                        ui,
                        None,
                        &format!("К установленным ({installed_count})"),
                    )
                    .clicked()
                    {
                        self.content_sub_mode = ContentSubMode::Installed;
                        if catalog_tab == crate::catalog_state::CatalogTab::Mods {
                            let _ = state.cmd_sender.send(UiWorkerCommand::ScanInstanceMods {
                                instance_id: inst_id.clone(),
                            });
                        }
                    }
                }
            }

            if secondary_glass_button(ui, Some(VectorIcon::Folder), folder_label).clicked() {
                let _ = std::fs::create_dir_all(folder_path);
                let _ = crate::opener::open_that(folder_path);
            }
        });
        if let Some(target_rect) = active_pill_rect {
            let anim_rect = crate::animation::animate_spring_rect(
                ui.ctx(),
                ui.make_persistent_id(("mq_content_submode_pill", inst_id.as_str(), catalog_tab)),
                target_rect,
                crate::animation::SpringConfig::rect(),
            );
            if style.refraction_border {
                let mut shapes = Vec::new();
                crate::theme_styles::collect_tinted_glass_pill_shapes(
                    ui.ctx(),
                    anim_rect,
                    RADIUS_TAG,
                    &style,
                    tokens,
                    blend_color(tokens.bg_surface, tokens.accent, 0.24),
                    0.75,
                    &mut shapes,
                );
                if !shapes.is_empty() {
                    ui.painter().set(indicator_slot, egui::Shape::Vec(shapes));
                }
            } else {
                let fill = blend_color(tokens.bg_surface, tokens.accent, 0.18);
                ui.painter().set(
                    indicator_slot,
                    egui::Shape::Rect(egui::epaint::RectShape::new(
                        anim_rect,
                        RADIUS_TAG,
                        fill,
                        Stroke::new(1.0_f32, tokens.accent),
                        StrokeKind::Inside,
                    )),
                );
            }
        }
        ui.add_space(if self.content_sub_mode == ContentSubMode::Catalog {
            2.0
        } else {
            6.0
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn render_local_file_import_bar(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        inst_id: &mq_core::InstanceId,
        target_dir: &std::path::Path,
        allowed_exts: &[&str],
        hint_text: &str,
        scan_mods_on_success: bool,
        tokens: &Tokens,
    ) {
        let mut candidates: Vec<std::path::PathBuf> = ui.ctx().input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
            ui.label(RichText::new("Локальный файл:").color(tokens.text_muted));
            let input_w = (ui.available_width() - 250.0).clamp(180.0, 360.0);
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.local_file_input)
                    .hint_text(hint_text)
                    .desired_width(input_w),
            );
            let submitted = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let can_add = !self.local_file_input.trim().is_empty();
            if (secondary_glass_button(ui, Some(VectorIcon::Plus), "Добавить файл").clicked()
                || submitted)
                && can_add
            {
                let cleaned = crate::opener::clean_user_path(&self.local_file_input);
                if !cleaned.is_empty() {
                    candidates.push(std::path::PathBuf::from(cleaned));
                }
                self.local_file_input.clear();
            }
        });

        let mut added_names = Vec::new();
        let mut first_error = None;
        for path in candidates {
            match install_local_content_file(target_dir, &path, allowed_exts) {
                Ok(fname) => added_names.push(fname),
                Err(err) => {
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                }
            }
        }

        if !added_names.is_empty() {
            invalidate_fs_cache::<Vec<(String, bool, u64)>>(
                ui.ctx(),
                ("mq_packs_dir_cache", target_dir.to_path_buf()),
            );
            let inst_dir = state.paths.instance_dir(inst_id.as_str());
            invalidate_fs_cache::<(Vec<String>, Vec<String>)>(
                ui.ctx(),
                ("mq_resourcepacks_cache", inst_dir.clone()),
            );
            invalidate_fs_cache::<(Vec<String>, Option<String>)>(
                ui.ctx(),
                ("mq_shaderpacks_cache", inst_dir),
            );
            let msg = if added_names.len() == 1 {
                format!("Файл {} добавлен в инстанс", added_names[0])
            } else {
                format!("Добавлено файлов: {}", added_names.len())
            };
            state.status_message = Some((msg, false));
            if scan_mods_on_success {
                let _ = state.cmd_sender.send(UiWorkerCommand::ScanInstanceMods {
                    instance_id: inst_id.clone(),
                });
            }
        } else if let Some(err) = first_error {
            state.status_message = Some((err, true));
        }
    }

    fn sync_and_render_instance_catalog(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        inst_id: &mq_core::InstanceId,
        catalog_tab: crate::catalog_state::CatalogTab,
    ) {
        if self.synced_catalog_context.as_ref() != Some(&(inst_id.clone(), catalog_tab)) {
            state.selected_instance_id = Some(inst_id.clone());
            if let Some((mc_version, loader)) = state
                .instances
                .iter()
                .find(|i| &i.id == inst_id)
                .map(|i| (i.mc_version.clone(), i.loader))
            {
                state.catalog.sync_for_instance(
                    catalog_tab,
                    &mc_version,
                    loader,
                    std::time::Instant::now(),
                );
            }
            self.synced_catalog_context = Some((inst_id.clone(), catalog_tab));
        }
        self.catalog_view
            .ui_for_instance_tab(ui, state, inst_id, catalog_tab);
    }

    fn render_mods_tab(&mut self, ui: &mut Ui, state: &mut UiState, inst_id: &mq_core::InstanceId) {
        let tokens = Tokens::from_config(&state.config.theme);
        let mods_dir = state.paths.instance_dir(inst_id.as_str()).join("mods");
        let total_count = state.installed_mods.len();
        let enabled_count = state.installed_mods.iter().filter(|m| m.enabled).count();
        let disabled_count = total_count.saturating_sub(enabled_count);

        self.render_content_sub_mode_bar(
            ui,
            state,
            inst_id,
            crate::catalog_state::CatalogTab::Mods,
            total_count,
            "Скачать моды",
            "Открыть mods/",
            &mods_dir,
            &tokens,
        );

        let sub_order = if self.content_sub_mode == ContentSubMode::Installed {
            0
        } else {
            1
        };
        crate::animation::render_tab_transition(
            ui,
            ("mq_mods_submode_trans", inst_id.as_str()),
            self.content_sub_mode,
            sub_order,
            true,
            |ui| {
                if self.content_sub_mode == ContentSubMode::Catalog {
                    self.sync_and_render_instance_catalog(
                        ui,
                        state,
                        inst_id,
                        crate::catalog_state::CatalogTab::Mods,
                    );
                    return;
                }

                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                    ui.label(
                        RichText::new(format!("Модов: {total_count} (вкл. {enabled_count})"))
                            .color(tokens.text)
                            .strong(),
                    );
                    if secondary_glass_button(ui, Some(VectorIcon::Refresh), "Пересканировать")
                        .clicked()
                    {
                        let _ = state.cmd_sender.send(UiWorkerCommand::ScanInstanceMods {
                            instance_id: inst_id.clone(),
                        });
                    }
                    if secondary_glass_button(ui, Some(VectorIcon::Check), "Проверить зависимости")
                        .clicked()
                    {
                        let _ = state.cmd_sender.send(UiWorkerCommand::RunPreflight {
                            instance_id: inst_id.clone(),
                        });
                    }
                    if secondary_glass_button(ui, Some(VectorIcon::Trash), "Удалить неиспользуемые")
                        .clicked()
                    {
                        let _ = state.cmd_sender.send(UiWorkerCommand::CleanupUnusedDeps {
                            instance_id: inst_id.clone(),
                        });
                    }
                });

                // Mod Profiles strip
                ui.add_space(4.0);
                let inst_opt = state.instances.iter().find(|i| i.id == *inst_id).cloned();
                if let Some(inst) = inst_opt {
                    let inst_dir = state.paths.instance_dir(inst_id.as_str());
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                        ui.label(
                            RichText::new("Профиль модов:")
                                .color(tokens.text)
                                .strong()
                                .small(),
                        );

                        let profile_names: Vec<String> =
                            inst.mod_profiles.iter().map(|p| p.name.clone()).collect();
                        if !profile_names.is_empty() {
                            let mut selected =
                                self.selected_mod_profile.clone().unwrap_or_else(|| {
                                    inst.active_mod_profile
                                        .clone()
                                        .unwrap_or_else(|| profile_names[0].clone())
                                });
                            egui::ComboBox::from_id_salt(("mod_profile_combo", inst_id.as_str()))
                                .selected_text(&selected)
                                .show_ui(ui, |ui| {
                                    for pname in &profile_names {
                                        ui.selectable_value(&mut selected, pname.clone(), pname);
                                    }
                                });
                            self.selected_mod_profile = Some(selected.clone());

                            if secondary_glass_button(ui, Some(VectorIcon::Check), "Применить")
                                .clicked()
                            {
                                if let Some(prof) =
                                    inst.mod_profiles.iter().find(|p| p.name == selected)
                                {
                                    match mq_instances::apply_mod_profile(&inst_dir, prof) {
                                        Ok(_) => {
                                            let mut save_ok = true;
                                            if let Some(inst_mut) = state
                                                .instances
                                                .iter_mut()
                                                .find(|i| i.id == *inst_id)
                                            {
                                                inst_mut.active_mod_profile =
                                                    Some(selected.clone());
                                                if let Err(e) =
                                                    state.instance_mgr.save_instance(inst_mut)
                                                {
                                                    state.status_message = Some((
                                                        format!("Ошибка сохранения профиля: {e}"),
                                                        true,
                                                    ));
                                                    save_ok = false;
                                                }
                                            }
                                            if save_ok {
                                                state.status_message = Some((
                                                    format!("Профиль «{selected}» применен"),
                                                    false,
                                                ));
                                            }
                                            let _ = state.cmd_sender.send(
                                                UiWorkerCommand::ScanInstanceMods {
                                                    instance_id: inst_id.clone(),
                                                },
                                            );
                                        }
                                        Err(e) => {
                                            state.status_message = Some((
                                                format!("Ошибка применения профиля: {e}"),
                                                true,
                                            ));
                                        }
                                    }
                                }
                            }

                            if secondary_glass_button(ui, Some(VectorIcon::Trash), "Удалить")
                                .clicked()
                            {
                                if let Some(inst_mut) =
                                    state.instances.iter_mut().find(|i| i.id == *inst_id)
                                {
                                    inst_mut.mod_profiles.retain(|p| p.name != selected);
                                    if inst_mut.active_mod_profile.as_deref() == Some(&selected) {
                                        inst_mut.active_mod_profile = None;
                                    }
                                    if let Err(e) = state.instance_mgr.save_instance(inst_mut) {
                                        state.status_message =
                                            Some((format!("Ошибка сохранения профиля: {e}"), true));
                                    } else {
                                        state.status_message =
                                            Some((format!("Профиль «{selected}» удален"), false));
                                    }
                                }
                                self.selected_mod_profile = None;
                            }
                        }

                        ui.add(
                            egui::TextEdit::singleline(&mut self.mod_profile_input)
                                .hint_text("Имя профиля")
                                .desired_width(120.0),
                        );
                        if secondary_glass_button(ui, Some(VectorIcon::Plus), "Сохранить профиль")
                            .clicked()
                            && !self.mod_profile_input.trim().is_empty()
                        {
                            let name = self.mod_profile_input.trim().to_string();
                            let profile = mq_instances::capture_mod_profile(&name, &inst_dir);
                            if let Some(inst_mut) =
                                state.instances.iter_mut().find(|i| i.id == *inst_id)
                            {
                                inst_mut.mod_profiles.retain(|p| p.name != name);
                                inst_mut.mod_profiles.push(profile);
                                inst_mut.active_mod_profile = Some(name.clone());
                                if let Err(e) = state.instance_mgr.save_instance(inst_mut) {
                                    state.status_message =
                                        Some((format!("Ошибка сохранения профиля: {e}"), true));
                                } else {
                                    state.status_message =
                                        Some((format!("Профиль «{name}» сохранен"), false));
                                }
                            }
                            self.mod_profile_input.clear();
                            self.selected_mod_profile = Some(name);
                        }
                    });
                }

                // Modpack update button and banner
                let inst_opt2 = state.instances.iter().find(|i| i.id == *inst_id);
                if let Some(inst) = inst_opt2 {
                    if inst.modpack_source.is_some() {
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                            let chk_label = if state.is_checking_modpack_update {
                                "Проверка обновления..."
                            } else {
                                "Проверить обновление сборки"
                            };
                            if secondary_glass_button(ui, Some(VectorIcon::Refresh), chk_label)
                                .clicked()
                                && !state.is_checking_modpack_update
                            {
                                let _ =
                                    state.cmd_sender.send(UiWorkerCommand::CheckModpackUpdate {
                                        instance_id: inst_id.clone(),
                                    });
                            }
                        });

                        if let Some(info) = state.modpack_update_available.get(inst_id).cloned() {
                            ui.add_space(4.0);
                            egui::Frame::new()
                                .fill(tokens.bg_surface)
                                .stroke(Stroke::new(1.0_f32, tokens.accent))
                                .corner_radius(RADIUS_CARD)
                                .inner_margin(egui::Margin::same(10))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "Доступно обновление сборки: {}",
                                            info.latest_version_name
                                        ))
                                        .color(tokens.text)
                                        .strong(),
                                    );
                                    if !info.changelog.is_empty() {
                                        ui.label(
                                            RichText::new(&info.changelog)
                                                .color(tokens.text_muted)
                                                .small(),
                                        );
                                    }
                                    ui.add_space(4.0);
                                    if primary_emerald_button(
                                        ui,
                                        Some(VectorIcon::Download),
                                        "Обновить (сохранив миры и конфиги)",
                                    )
                                    .clicked()
                                    {
                                        let _ = state.cmd_sender.send(
                                            UiWorkerCommand::ApplyModpackUpdate {
                                                instance_id: inst_id.clone(),
                                                info,
                                            },
                                        );
                                    }
                                });
                        }
                    }
                }

                // Binary Crash Bisect panel
                ui.add_space(4.0);
                let is_bisect_active = state
                    .active_bisect
                    .as_ref()
                    .map(|(id, _)| id == inst_id)
                    .unwrap_or(false);
                let header_title = if is_bisect_active {
                    "Бинарный поиск краша (Активен)"
                } else {
                    "Бинарный поиск краша"
                };
                ui.collapsing(header_title, |ui| {
                    ui.label(RichText::new("Пошаговое деление модов пополам для поиска мода, вызывающего падение игры:").color(tokens.text_muted).small());
                    ui.add_space(4.0);

                    if is_bisect_active {
                        let mut should_clear = false;
                        let mut status_msg = None;
                        if let Some((_, ref mut session)) = state.active_bisect {
                            ui.label(RichText::new(format!(
                                "Шаг {} · Активно {} из {}",
                                session.round(),
                                session.active_half_len(),
                                session.suspects_len()
                            )).color(tokens.text).strong());

                            if let Some(culprit) = session.culprit() {
                                let culprit_str = culprit.to_string();
                                ui.add_space(4.0);
                                egui::Frame::new()
                                    .fill(tokens.bg_surface)
                                    .stroke(Stroke::new(1.0_f32, tokens.danger))
                                    .corner_radius(RADIUS_CARD)
                                    .inner_margin(egui::Margin::same(8))
                                    .show(ui, |ui| {
                                        ui.label(RichText::new(format!("Виновник падения: «{culprit_str}»")).color(tokens.danger).strong());
                                        ui.add_space(4.0);
                                        if danger_button(ui, Some(VectorIcon::Cross), "Отключить виновника и завершить").clicked() {
                                            let jar_path = mods_dir.join(&culprit_str);
                                            let disabled_path = mods_dir.join(format!("{culprit_str}.disabled"));
                                            let _ = std::fs::rename(jar_path, disabled_path);
                                            let _ = session.restore_all(&mods_dir);
                                            should_clear = true;
                                            status_msg = Some((format!("Мод «{culprit_str}» отключен"), false));
                                        }
                                    });
                            } else {
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                                    if danger_button(ui, Some(VectorIcon::Cross), "Крашнулось").clicked() {
                                        let _ = session.step_crashed(&mods_dir);
                                        let _ = state.cmd_sender.send(UiWorkerCommand::ScanInstanceMods { instance_id: inst_id.clone() });
                                    }
                                    if primary_emerald_button(ui, Some(VectorIcon::Check), "Запустилось ОК").clicked() {
                                        let _ = session.step_ok(&mods_dir);
                                        let _ = state.cmd_sender.send(UiWorkerCommand::ScanInstanceMods { instance_id: inst_id.clone() });
                                    }
                                    if secondary_glass_button(ui, None, "Вернуть все моды").clicked() {
                                        let _ = session.restore_all(&mods_dir);
                                        should_clear = true;
                                        status_msg = Some(("Все моды восстановлены".to_string(), false));
                                        let _ = state.cmd_sender.send(UiWorkerCommand::ScanInstanceMods { instance_id: inst_id.clone() });
                                    }
                                });
                            }
                        }
                        if should_clear {
                            state.active_bisect = None;
                        }
                        if let Some(msg) = status_msg {
                            state.status_message = Some(msg);
                        }
                    } else if secondary_glass_button(ui, Some(VectorIcon::Search), "Начать поиск").clicked() {
                        match mq_mods::BisectSession::start(&mods_dir) {
                            Ok(session) => {
                                state.active_bisect = Some((inst_id.clone(), session));
                                state.status_message = Some(("Поиск краша запущен".to_string(), false));
                                let _ = state.cmd_sender.send(UiWorkerCommand::ScanInstanceMods { instance_id: inst_id.clone() });
                            }
                            Err(e) => {
                                state.status_message = Some((format!("Не удалось начать поиск: {e}"), true));
                            }
                        }
                    }
                });

                if let Some((ref prompt_inst, ref deleted_file, ref orphans)) =
                    state.orphaned_deps_prompt
                {
                    if prompt_inst == inst_id && !orphans.is_empty() {
                        ui.add_space(6.0);
                        let deleted_file = deleted_file.clone();
                        let orphans_clone = orphans.clone();
                        let mut clear_prompt = false;
                        egui::Frame::new()
                    .fill(tokens.bg_surface)
                    .stroke(Stroke::new(1.0_f32, tokens.warning))
                    .corner_radius(RADIUS_CARD)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!(
                                "После удаления «{deleted_file}» остались неиспользуемые зависимости ({}):",
                                orphans_clone.len()
                            ))
                            .color(tokens.text)
                            .strong(),
                        );
                        for o in &orphans_clone {
                            ui.label(RichText::new(format!("• {o}")).color(tokens.text_muted).small());
                        }
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            if primary_emerald_button(
                                ui,
                                Some(VectorIcon::Trash),
                                "Удалить зависимости",
                            )
                            .clicked()
                            {
                                let _ = state.cmd_sender.send(UiWorkerCommand::CleanupOrphanFiles {
                                    instance_id: inst_id.clone(),
                                    orphan_files: orphans_clone.clone(),
                                });
                                clear_prompt = true;
                            }
                            if secondary_glass_button(ui, None, "Оставить").clicked() {
                                clear_prompt = true;
                            }
                        });
                    });
                        if clear_prompt {
                            state.orphaned_deps_prompt = None;
                        }
                    }
                }

                let mut search_dep_in_catalog: Option<String> = None;

                if let Some((ref report_inst, ref report)) = state.preflight_report {
                    if report_inst == inst_id && !report.issues.is_empty() {
                        ui.add_space(6.0);
                        let report_clone = report.clone();
                        let mut clear_report = false;
                        egui::Frame::new()
                    .fill(tokens.bg_surface)
                    .stroke(Stroke::new(1.0_f32, tokens.danger))
                    .corner_radius(RADIUS_CARD)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!(
                                "Проверка зависимостей: найдено проблем ({})",
                                report_clone.issues.len()
                            ))
                            .color(tokens.text)
                            .strong(),
                        );
                        for (idx, issue) in report_clone.issues.iter().enumerate() {
                            match issue {
                                mq_mods::PreflightIssue::MissingDependency {
                                    mod_id,
                                    file_name,
                                    required_dep,
                                } => {
                                    ui.push_id(("preflight_missing", idx), |ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                                            ui.label(
                                                RichText::new(format!(
                                                    "• Мод «{mod_id}» ({file_name}) требует зависимость «{required_dep}»"
                                                ))
                                                .color(tokens.text_muted)
                                                .small(),
                                            );
                                            if secondary_glass_button(
                                                ui,
                                                Some(VectorIcon::Search),
                                                "Найти в каталоге",
                                            )
                                            .clicked()
                                            {
                                                search_dep_in_catalog = Some(required_dep.clone());
                                            }
                                        });
                                    });
                                }
                                mq_mods::PreflightIssue::IncompatibleMod {
                                    mod_id,
                                    file_name,
                                    conflicting_mod,
                                } => {
                                    ui.label(
                                        RichText::new(format!(
                                            "• Несовместимость: мод «{mod_id}» ({file_name}) конфликтует с «{conflicting_mod}»"
                                        ))
                                        .color(tokens.danger)
                                        .small(),
                                    );
                                }
                                mq_mods::PreflightIssue::DuplicateModId { mod_id, files } => {
                                    ui.label(
                                        RichText::new(format!(
                                            "• Дубликат «{mod_id}»: {}",
                                            files.join(", ")
                                        ))
                                        .color(tokens.text_muted)
                                        .small(),
                                    );
                                }
                                mq_mods::PreflightIssue::WrongLoader {
                                    mod_id,
                                    mod_loader,
                                    instance_loader,
                                    ..
                                } => {
                                    ui.label(
                                        RichText::new(format!(
                                            "• Несовместимый загрузчик у «{mod_id}»: {} вместо {}",
                                            mod_loader.as_str(),
                                            instance_loader.as_str()
                                        ))
                                        .color(tokens.text_muted)
                                        .small(),
                                    );
                                }
                                mq_mods::PreflightIssue::WrongMcVersion {
                                    mod_id,
                                    required_mc,
                                    instance_mc,
                                    ..
                                } => {
                                    ui.label(
                                        RichText::new(format!(
                                            "• Неверная версия MC у «{mod_id}»: требуется {required_mc} (инстанс {instance_mc})"
                                        ))
                                        .color(tokens.text_muted)
                                        .small(),
                                    );
                                }
                            }
                        }
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            let missing_ids: Vec<String> = report_clone
                                .issues
                                .iter()
                                .filter_map(|issue| {
                                    if let mq_mods::PreflightIssue::MissingDependency {
                                        required_dep,
                                        ..
                                    } = issue
                                    {
                                        Some(required_dep.clone())
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            if !missing_ids.is_empty() {
                                let fix_lbl = if state.is_auto_fixing_deps {
                                    "Установка зависимостей..."
                                } else {
                                    "Установить все зависимости"
                                };
                                if primary_emerald_button(
                                    ui,
                                    Some(VectorIcon::Download),
                                    fix_lbl,
                                )
                                .clicked()
                                    && !state.is_auto_fixing_deps
                                {
                                    let _ = state.cmd_sender.send(
                                        UiWorkerCommand::AutoInstallMissingDeps {
                                            instance_id: inst_id.clone(),
                                            missing_ids,
                                        },
                                    );
                                }
                            }
                            if primary_emerald_button(
                                ui,
                                Some(VectorIcon::Check),
                                "Исправить автоматически",
                            )
                            .clicked()
                            {
                                let _ = state.cmd_sender.send(UiWorkerCommand::AutoFixPreflight {
                                    instance_id: inst_id.clone(),
                                    report: report_clone.clone(),
                                });
                            }
                            if secondary_glass_button(ui, None, "Скрыть").clicked() {
                                clear_report = true;
                            }
                        });
                    });
                        if clear_report {
                            state.preflight_report = None;
                        }
                    }
                }

                ui.add_space(6.0);
                self.render_local_file_import_bar(
                    ui,
                    state,
                    inst_id,
                    &mods_dir,
                    &[".jar"],
                    "Путь к .jar файлу мода или перетащите .jar в окно...",
                    true,
                    &tokens,
                );

                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                    let search_w = (ui.available_width() - 340.0).clamp(150.0, 300.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.mod_search_query)
                            .hint_text("Поиск модов по названию, ID или файлу...")
                            .desired_width(search_w),
                    );
                    if !self.mod_search_query.is_empty()
                        && secondary_glass_button(ui, None, "Сброс").clicked()
                    {
                        self.mod_search_query.clear();
                    }

                    for (filter, lbl) in [
                        (ModStatusFilter::All, format!("Все ({total_count})")),
                        (
                            ModStatusFilter::Enabled,
                            format!("Включены ({enabled_count})"),
                        ),
                        (
                            ModStatusFilter::Disabled,
                            format!("Отключены ({disabled_count})"),
                        ),
                    ] {
                        let is_sel = self.mod_status_filter == filter;
                        let fill = if is_sel {
                            blend_color(tokens.bg_surface, tokens.accent, 0.18)
                        } else {
                            tokens.bg_input
                        };
                        let stroke_col = if is_sel { tokens.accent } else { tokens.border };
                        let fg = if is_sel {
                            tokens.text
                        } else {
                            tokens.text_muted
                        };
                        if ui
                            .add(
                                egui::Button::new(RichText::new(lbl).color(fg).size(12.0))
                                    .fill(fill)
                                    .stroke(Stroke::new(1.0_f32, stroke_col))
                                    .corner_radius(RADIUS_TAG),
                            )
                            .clicked()
                        {
                            self.mod_status_filter = filter;
                        }
                    }
                });

                ui.add_space(4.0);
                crate::catalog_dialogs::jobs_panel(ui, state);
                Self::updates_panel(ui, state, inst_id);

                if !state.mod_conflicts.is_empty() {
                    let warn_frame = egui::Frame::new()
                        .fill(tokens.bg_surface)
                        .stroke(Stroke::new(1.0_f32, tokens.warning))
                        .corner_radius(RADIUS_CARD)
                        .inner_margin(egui::Margin::same(8));

                    warn_frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                    paint_vector_icon(ui.painter(), rect, VectorIcon::Warning, tokens.warning);
                    ui.label(
                        RichText::new(format!(
                            "Обнаружено конфликтов: {}",
                            state.mod_conflicts.len()
                        ))
                        .color(tokens.warning)
                        .strong(),
                    );
                });
                for (idx, c) in state.mod_conflicts.iter().enumerate() {
                    match c {
                        mq_mods::ModConflict::DuplicateModId { mod_id, files } => {
                            ui.label(
                                RichText::new(format!(
                                    "• Дубликат ID «{mod_id}» в файлах: {}",
                                    files.join(", ")
                                ))
                                .color(tokens.danger),
                            );
                        }
                        mq_mods::ModConflict::MissingDependency {
                            mod_id,
                            file_name,
                            required_dep,
                        } => {
                            ui.push_id(("mod_conflict_missing", idx), |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                                    ui.label(
                                        RichText::new(format!(
                                            "• Мод «{mod_id}» ({file_name}) требует отсутствующую зависимость «{required_dep}»"
                                        ))
                                        .color(tokens.warning),
                                    );
                                    if secondary_glass_button(
                                        ui,
                                        Some(VectorIcon::Search),
                                        "Найти в каталоге",
                                    )
                                    .clicked()
                                    {
                                        search_dep_in_catalog = Some(required_dep.clone());
                                    }
                                });
                            });
                        }
                        mq_mods::ModConflict::IncompatibleMod {
                            mod_id,
                            file_name,
                            conflicting_mod,
                        } => {
                            ui.label(
                                RichText::new(format!(
                                    "• Несовместимые моды: «{mod_id}» ({file_name}) конфликтует с «{conflicting_mod}»"
                                ))
                                .color(tokens.danger),
                            );
                        }
                    }
                }
            });
                    ui.add_space(6.0);
                }

                if let Some(dep_query) = search_dep_in_catalog {
                    let now = std::time::Instant::now();
                    state.selected_instance_id = Some(inst_id.clone());
                    state.catalog.query = dep_query;
                    state.catalog.page = 0;
                    if let Some((mc_version, loader)) = state
                        .instances
                        .iter()
                        .find(|i| &i.id == inst_id)
                        .map(|i| (i.mc_version.clone(), i.loader))
                    {
                        state.catalog.sync_for_instance(
                            crate::catalog_state::CatalogTab::Mods,
                            &mc_version,
                            loader,
                            now,
                        );
                    }
                    state.catalog.mark_page_changed(now, false);
                    self.synced_catalog_context =
                        Some((inst_id.clone(), crate::catalog_state::CatalogTab::Mods));
                    self.content_sub_mode = ContentSubMode::Catalog;
                    ui.ctx().request_repaint();
                }

                let lock = Self::cached_lock(ui.ctx(), &state.paths.instance_dir(inst_id.as_str()));
                let q = self.mod_search_query.trim().to_lowercase();
                let filtered_mods: Vec<_> = state
                    .installed_mods
                    .iter()
                    .filter(|m| match self.mod_status_filter {
                        ModStatusFilter::All => true,
                        ModStatusFilter::Enabled => m.enabled,
                        ModStatusFilter::Disabled => !m.enabled,
                    })
                    .filter(|m| {
                        q.is_empty()
                            || m.name.to_lowercase().contains(&q)
                            || m.file_name.to_lowercase().contains(&q)
                            || m.mod_id.to_lowercase().contains(&q)
                    })
                    .collect();

                let mut open_catalog = false;
                ScrollArea::vertical()
            .id_salt("mods_table_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if state.installed_mods.is_empty() {
                    ui.add_space(20.0);
                    ui.label(
                        RichText::new(
                            "В этом инстансе пока нет модов. Скачайте их из Каталога или добавьте .jar файл выше.",
                        )
                        .color(tokens.text_muted),
                    );
                    ui.add_space(8.0);
                    if primary_emerald_button(
                        ui,
                        Some(VectorIcon::Plus),
                        "Скачать моды из каталога (Modrinth / CurseForge)",
                    )
                    .clicked()
                    {
                        open_catalog = true;
                    }
                } else if filtered_mods.is_empty() {
                    ui.add_space(16.0);
                    ui.label(
                        RichText::new("По текущему фильтру моды не найдены.")
                            .color(tokens.text_muted),
                    );
                } else {
                    for m in filtered_mods {
                        let card = egui::Frame::new()
                            .fill(tokens.bg_surface)
                            .stroke(Stroke::new(1.0_f32, tokens.border))
                            .corner_radius(RADIUS_CARD)
                            .inner_margin(egui::Margin::same(8));

                        card.show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let mut enabled = m.enabled;
                                if crate::widgets::smooth_toggle(ui, &mut enabled).changed() {
                                    let _ = state.cmd_sender.send(UiWorkerCommand::ToggleMod {
                                        instance_id: inst_id.clone(),
                                        file_name: m.file_name.clone(),
                                    });
                                }

                                let right_w = 40.0;
                                let middle_w = (ui.available_width() - right_w - 8.0).max(60.0);

                                ui.allocate_ui_with_layout(
                                    Vec2::new(middle_w, 36.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                                            let name_col = if m.enabled {
                                                tokens.text
                                            } else {
                                                tokens.text_muted
                                            };
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&m.name).color(name_col).strong(),
                                                )
                                                .truncate(),
                                            );
                                            version_badge(ui, &m.version, ACCENT_COBALT);
                                            if let Some(ld) = m.loader {
                                                version_badge(ui, ld.as_str(), tokens.accent);
                                            }
                                            match lock.find_by_filename(&m.file_name) {
                                                Some((_, e)) => {
                                                    let (txt, col) = match e.source {
                                                        mq_mods::LockSource::Modrinth => {
                                                            ("Modrinth", tokens.accent)
                                                        }
                                                        mq_mods::LockSource::Manual => {
                                                            ("Вручную", tokens.text_muted)
                                                        }
                                                        mq_mods::LockSource::CurseForge => (
                                                            "CurseForge",
                                                            Color32::from_rgb(0xD9, 0x72, 0x45),
                                                        ),
                                                    };
                                                    version_badge(ui, txt, col);
                                                }
                                                None => {
                                                    version_badge(ui, "Локальный", tokens.text_muted)
                                                }
                                            }
                                            if m.size > 0 {
                                                ui.label(
                                                    RichText::new(crate::catalog_flow::format_size(
                                                        m.size,
                                                    ))
                                                    .color(tokens.text_muted)
                                                    .small(),
                                                );
                                            }
                                        });
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(format!("Файл: {}", m.file_name))
                                                    .color(tokens.text_muted)
                                                    .small(),
                                            )
                                            .truncate(),
                                        );
                                    },
                                );

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if secondary_glass_button(ui, Some(VectorIcon::Trash), "")
                                            .on_hover_text("Удалить мод")
                                            .clicked()
                                        {
                                            let _ =
                                                state.cmd_sender.send(UiWorkerCommand::DeleteMod {
                                                    instance_id: inst_id.clone(),
                                                    file_name: m.file_name.clone(),
                                                });
                                        }
                                    },
                                );
                            });
                        });
                        ui.add_space(4.0);
                    }
                }
            });
                if open_catalog {
                    self.content_sub_mode = ContentSubMode::Catalog;
                    self.synced_catalog_context = None;
                }
            },
        );
    }

    fn render_console_tab(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        inst_id: &mq_core::InstanceId,
    ) {
        let inst_key = inst_id.as_str();

        // 1. Crash Analysis Banner (scoped to the active instance when known)
        let crash_matches_instance = state
            .crash_analysis_instance
            .as_ref()
            .is_none_or(|id| id == inst_id);
        let crash_opt = if crash_matches_instance {
            state.crash_analysis.as_ref()
        } else {
            None
        };
        if let Some(analysis) = crash_opt {
            let err_frame = egui::Frame::new()
                .fill(Color32::from_rgb(0x28, 0x14, 0x16))
                .stroke(Stroke::new(1.0_f32, STATUS_DANGER))
                .corner_radius(RADIUS_CARD)
                .inner_margin(egui::Margin::same(12));

            err_frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), egui::Sense::hover());
                    paint_vector_icon(ui.painter(), r, VectorIcon::Warning, STATUS_DANGER);
                    ui.label(
                        RichText::new(&analysis.summary)
                            .color(STATUS_DANGER)
                            .strong()
                            .size(14.0),
                    );
                });
                ui.add_space(4.0);
                ui.label(RichText::new(&analysis.recommendation).color(TEXT_PRIMARY));
                if !analysis.suspected_mods.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Подозрительные моды:")
                                .color(TEXT_MUTED)
                                .small(),
                        );
                        for sm in &analysis.suspected_mods {
                            version_badge(ui, sm, STATUS_WARN);
                        }
                    });
                }
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if state.is_uploading_mclogs {
                        ui.label(RichText::new("Загрузка на mclogs...").color(ACCENT_COBALT));
                    } else if let Some(ref url) = state.mclogs_url {
                        ui.label(
                            RichText::new(format!("Ссылка на лог: {url}")).color(ACCENT_EMERALD),
                        );
                        if secondary_glass_button(ui, Some(VectorIcon::Copy), "Скопировать ссылку")
                            .clicked()
                        {
                            ui.ctx().copy_text(url.clone());
                            state.status_message = Some((
                                "Ссылка mclogs скопирована в буфер обмена".to_string(),
                                false,
                            ));
                        }
                    } else if secondary_glass_button(
                        ui,
                        Some(VectorIcon::Download),
                        "Поделиться логом на mclogs",
                    )
                    .clicked()
                    {
                        let full_log: String = state
                            .game_logs
                            .iter()
                            .filter(|l| l.instance == inst_key)
                            .map(|l| l.line.as_str())
                            .collect::<Vec<_>>()
                            .join("\n");
                        if full_log.trim().is_empty() {
                            state.status_message =
                                Some(("Лог пуст — нечего загружать".to_string(), true));
                        } else {
                            state.is_uploading_mclogs = true;
                            let _ = state
                                .cmd_sender
                                .send(UiWorkerCommand::UploadMclogs { log_text: full_log });
                        }
                    }
                });
            });
            ui.add_space(8.0);
        }

        // Single pass over logs for this instance: count levels and collect visible row indices.
        let query = self.console_search.trim().to_lowercase();
        let mut inst_total = 0usize;
        let mut info_count = 0usize;
        let mut warn_count = 0usize;
        let mut error_count = 0usize;
        let mut filtered_indices: Vec<usize> = Vec::new();
        for (idx, entry) in state.game_logs.iter().enumerate() {
            if entry.instance != inst_key {
                continue;
            }
            inst_total += 1;
            match entry.level {
                GameLogLevel::Info => info_count += 1,
                GameLogLevel::Warn => warn_count += 1,
                GameLogLevel::Error => error_count += 1,
                GameLogLevel::Debug => {}
            }
            if let Some(flt) = state.log_filter {
                if entry.level != flt {
                    continue;
                }
            }
            if !query.is_empty() && !entry.line.to_lowercase().contains(&query) {
                continue;
            }
            filtered_indices.push(idx);
        }

        // Row 1: Line counter, autoscroll checkbox, action buttons
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
            ui.label(RichText::new(format!("Строк в логе: {inst_total}")).color(TEXT_MUTED));
            ui.checkbox(&mut self.console_autoscroll, "Автоскролл");
            if secondary_glass_button(ui, Some(VectorIcon::Copy), "Копировать лог").clicked()
            {
                let full_log: String = state
                    .game_logs
                    .iter()
                    .filter(|l| l.instance == inst_key)
                    .map(|l| l.line.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                if full_log.is_empty() {
                    state.status_message = Some(("Лог пуст".to_string(), false));
                } else {
                    ui.ctx().copy_text(full_log);
                    state.status_message =
                        Some(("Лог скопирован в буфер обмена".to_string(), false));
                }
            }
            if secondary_glass_button(ui, None, "Очистить").clicked() {
                state.game_logs.retain(|l| l.instance != inst_key);
                state.mclogs_url = None;
                state.status_message = Some(("Лог очищен".to_string(), false));
            }
        });

        ui.add_space(4.0);

        // Row 2: Search input and level filter chips (B6, B7)
        let tokens = Tokens::from_config(&state.config.theme);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
            SearchField::new(&mut self.console_search, "Поиск в логе...")
                .width(150.0)
                .show(ui, &tokens);

            if FilterChip::new("INFO", state.log_filter == Some(GameLogLevel::Info))
                .dot_color(tokens.text_muted)
                .count(info_count)
                .show(ui, &tokens)
                .clicked()
            {
                state.log_filter = if state.log_filter == Some(GameLogLevel::Info) {
                    None
                } else {
                    Some(GameLogLevel::Info)
                };
            }

            if FilterChip::new("WARN", state.log_filter == Some(GameLogLevel::Warn))
                .dot_color(tokens.warning)
                .count(warn_count)
                .show(ui, &tokens)
                .clicked()
            {
                state.log_filter = if state.log_filter == Some(GameLogLevel::Warn) {
                    None
                } else {
                    Some(GameLogLevel::Warn)
                };
            }

            if FilterChip::new("ERROR", state.log_filter == Some(GameLogLevel::Error))
                .dot_color(tokens.danger)
                .count(error_count)
                .show(ui, &tokens)
                .clicked()
            {
                state.log_filter = if state.log_filter == Some(GameLogLevel::Error) {
                    None
                } else {
                    Some(GameLogLevel::Error)
                };
            }
        });

        ui.add_space(8.0);

        // B3: Virtualized console (`show_rows`) occupies full remaining height so 10k log lines
        // never stall the UI thread.
        let avail_h = (ui.available_height() - 12.0).max(240.0);
        let console_frame = egui::Frame::new()
            .fill(Color32::from_rgb(0x08, 0x0A, 0x0E))
            .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
            .corner_radius(RADIUS_CARD)
            .inner_margin(egui::Margin::same(8));

        console_frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(avail_h);
            let font_id = egui::FontId::monospace(12.0);
            let row_height = ui.text_style_height(&egui::TextStyle::Monospace).max(15.0);
            if filtered_indices.is_empty() {
                ScrollArea::vertical()
                    .id_salt("console_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let empty_msg = if inst_total == 0 {
                            "Логи игры появятся здесь при запуске Minecraft"
                        } else {
                            "Нет строк лога, подходящих под текущий фильтр"
                        };
                        ui.label(RichText::new(empty_msg).color(TEXT_MUTED).font(font_id));
                    });
            } else {
                ScrollArea::vertical()
                    .id_salt("console_scroll")
                    .auto_shrink([false, false])
                    .stick_to_bottom(self.console_autoscroll)
                    .show_rows(ui, row_height, filtered_indices.len(), |ui, row_range| {
                        for row_idx in row_range {
                            let Some(&log_idx) = filtered_indices.get(row_idx) else {
                                continue;
                            };
                            let Some(entry) = state.game_logs.get(log_idx) else {
                                continue;
                            };
                            let col = match entry.level {
                                GameLogLevel::Info => TEXT_PRIMARY,
                                GameLogLevel::Warn => STATUS_WARN,
                                GameLogLevel::Error => STATUS_DANGER,
                                GameLogLevel::Debug => TEXT_MUTED,
                            };
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&entry.line).color(col).font(font_id.clone()),
                                )
                                .truncate(),
                            );
                        }
                    });
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn render_pack_directory_tab(
        ui: &mut Ui,
        state: &mut UiState,
        tokens: &Tokens,
        packs_dir: &std::path::Path,
        packs: Vec<(String, bool, u64)>,
        title: &str,
        empty_hint: &str,
        catalog_btn_label: &str,
        search_query: &mut String,
    ) -> bool {
        let mut open_catalog = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
            ui.label(
                RichText::new(format!("{title}: {}", packs.len()))
                    .color(tokens.text)
                    .strong()
                    .size(14.0),
            );
            ui.add(
                egui::TextEdit::singleline(search_query)
                    .hint_text("Поиск по имени файла...")
                    .desired_width(220.0),
            );
            if !search_query.is_empty() && secondary_glass_button(ui, None, "Сброс").clicked()
            {
                search_query.clear();
            }
        });

        ui.add_space(6.0);
        let q = search_query.trim().to_lowercase();
        let filtered: Vec<_> = packs
            .into_iter()
            .filter(|(name, _, _)| q.is_empty() || name.to_lowercase().contains(&q))
            .collect();

        if filtered.is_empty() {
            ui.add_space(16.0);
            let msg = if q.is_empty() {
                empty_hint
            } else {
                "По вашему запросу ничего не найдено."
            };
            ui.label(RichText::new(msg).color(tokens.text_muted));
            if q.is_empty() {
                ui.add_space(8.0);
                if primary_emerald_button(ui, Some(VectorIcon::Plus), catalog_btn_label).clicked() {
                    open_catalog = true;
                }
            }
            return open_catalog;
        }

        ScrollArea::vertical()
            .id_salt(("pack_dir_scroll", title))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (name, enabled, size) in filtered {
                    let card = egui::Frame::new()
                        .fill(tokens.bg_surface)
                        .stroke(Stroke::new(1.0_f32, tokens.border))
                        .corner_radius(RADIUS_CARD)
                        .inner_margin(egui::Margin::same(8));

                    card.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let mut is_on = enabled;
                            if crate::widgets::smooth_toggle(ui, &mut is_on).changed() {
                                toggle_pack_file(packs_dir, &name);
                                invalidate_fs_cache::<Vec<(String, bool, u64)>>(
                                    ui.ctx(),
                                    ("mq_packs_dir_cache", packs_dir.to_path_buf()),
                                );
                            }
                            let name_col = if enabled {
                                tokens.text
                            } else {
                                tokens.text_muted
                            };
                            ui.add(
                                egui::Label::new(RichText::new(&name).color(name_col).strong())
                                    .truncate(),
                            );
                            if size > 0 {
                                ui.label(
                                    RichText::new(crate::catalog_flow::format_size(size))
                                        .color(tokens.text_muted)
                                        .small(),
                                );
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if secondary_glass_button(ui, Some(VectorIcon::Trash), "")
                                        .on_hover_text("Удалить файл")
                                        .clicked()
                                    {
                                        match delete_pack_file(packs_dir, &name) {
                                            Ok(()) => {
                                                invalidate_fs_cache::<Vec<(String, bool, u64)>>(
                                                    ui.ctx(),
                                                    ("mq_packs_dir_cache", packs_dir.to_path_buf()),
                                                );
                                            }
                                            Err(e) => {
                                                state.status_message = Some((
                                                    format!("Ошибка удаления файла: {e}"),
                                                    true,
                                                ));
                                            }
                                        }
                                    }
                                },
                            );
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        open_catalog
    }

    #[allow(clippy::too_many_arguments)]
    fn render_pack_tab(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        inst_id: &mq_core::InstanceId,
        subdir: &str,
        catalog_tab: crate::catalog_state::CatalogTab,
        title: &str,
        download_cta: &str,
        folder_btn_label: &str,
        empty_hint: &str,
        catalog_btn_label: &str,
        import_hint: &str,
        is_shader_tab: bool,
    ) {
        let tokens = Tokens::from_config(&state.config.theme);
        let dir = state.paths.instance_dir(inst_id.as_str()).join(subdir);
        let packs = cached_fs_value(ui.ctx(), ("mq_packs_dir_cache", dir.clone()), || {
            let _ = std::fs::create_dir_all(&dir);
            list_packs_in_dir(&dir)
        });

        self.render_content_sub_mode_bar(
            ui,
            state,
            inst_id,
            catalog_tab,
            packs.len(),
            download_cta,
            folder_btn_label,
            &dir,
            &tokens,
        );

        let sub_order = if self.content_sub_mode == ContentSubMode::Installed {
            0
        } else {
            1
        };
        crate::animation::render_tab_transition(
            ui,
            ("mq_pack_submode_trans", inst_id.as_str(), catalog_tab),
            self.content_sub_mode,
            sub_order,
            true,
            |ui| {
                if self.content_sub_mode == ContentSubMode::Catalog {
                    self.sync_and_render_instance_catalog(ui, state, inst_id, catalog_tab);
                    return;
                }

                self.render_local_file_import_bar(
                    ui,
                    state,
                    inst_id,
                    &dir,
                    &[".zip"],
                    import_hint,
                    false,
                    &tokens,
                );
                ui.add_space(6.0);
                crate::catalog_dialogs::jobs_panel(ui, state);

                let search_query = if is_shader_tab {
                    &mut self.shader_search_query
                } else {
                    &mut self.pack_search_query
                };
                let open_catalog = Self::render_pack_directory_tab(
                    ui,
                    state,
                    &tokens,
                    &dir,
                    packs,
                    title,
                    empty_hint,
                    catalog_btn_label,
                    search_query,
                );
                if open_catalog {
                    self.content_sub_mode = ContentSubMode::Catalog;
                    self.synced_catalog_context = None;
                }
            },
        );
    }

    fn render_resourcepacks_tab(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        inst_id: &mq_core::InstanceId,
    ) {
        self.render_pack_tab(
            ui,
            state,
            inst_id,
            "resourcepacks",
            crate::catalog_state::CatalogTab::ResourcePacks,
            "Ресурспаки",
            "Скачать ресурспаки",
            "Открыть resourcepacks/",
            "Ресурспаки не установлены. Скачайте их из Каталога или добавьте .zip файл выше.",
            "Скачать ресурспаки из каталога (Modrinth / CurseForge)",
            "Путь к .zip ресурспаку или перетащите .zip в окно...",
            false,
        );

        let inst_dir = state.paths.instance_dir(inst_id.as_str());
        let (installed, mut order) = cached_fs_value(
            ui.ctx(),
            ("mq_resourcepacks_cache", inst_dir.clone()),
            || {
                let installed = mq_mods::shaders::ShaderManager::list_resourcepacks(&inst_dir);
                let options_path = inst_dir.join("options.txt");
                let mut order =
                    mq_launch::options::MinecraftOptionsManager::read_resource_packs(&options_path)
                        .unwrap_or_default();
                for pack in &installed {
                    let entry = format!("file/{pack}");
                    if !order.contains(&entry) {
                        order.push(entry);
                    }
                }
                (installed, order)
            },
        );
        if !installed.is_empty() {
            let tokens = Tokens::from_config(&state.config.theme);
            let options_path = inst_dir.join("options.txt");
            ui.add_space(10.0);
            egui::Frame::new()
                .fill(tokens.bg_surface)
                .stroke(Stroke::new(1.0_f32, tokens.border))
                .corner_radius(RADIUS_CARD)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("Порядок ресурспаков (options.txt)")
                            .color(tokens.text)
                            .strong(),
                    );
                    ui.add_space(4.0);
                    let mut changed = false;
                    let len = order.len();
                    for idx in 0..len {
                        let name = order[idx].clone();
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!("{}. {name}", idx + 1)).color(tokens.text),
                            );
                            if idx > 0 && secondary_glass_button(ui, None, "Вверх").clicked() {
                                order.swap(idx, idx - 1);
                                changed = true;
                            }
                            if idx + 1 < len && secondary_glass_button(ui, None, "Вниз").clicked()
                            {
                                order.swap(idx, idx + 1);
                                changed = true;
                            }
                        });
                    }
                    if changed {
                        let _ = mq_launch::options::MinecraftOptionsManager::update_resource_packs(
                            &options_path,
                            &order,
                        );
                        invalidate_fs_cache::<(Vec<String>, Vec<String>)>(
                            ui.ctx(),
                            ("mq_resourcepacks_cache", inst_dir.clone()),
                        );
                    }
                });
        }
    }

    fn render_shaderpacks_tab(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        inst_id: &mq_core::InstanceId,
    ) {
        let tokens = Tokens::from_config(&state.config.theme);
        let inst_dir = state.paths.instance_dir(inst_id.as_str());
        let loader = state
            .instances
            .iter()
            .find(|i| &i.id == inst_id)
            .map(|i| i.loader)
            .unwrap_or(ModLoaderKind::Vanilla);
        let engine = mq_mods::shaders::ShaderEngine::for_loader(loader);
        let gpu = state
            .gpu_diagnostics
            .get_or_insert_with(mq_mods::shaders::GpuDiagnostics::probe)
            .clone();

        egui::Frame::new()
            .fill(tokens.bg_surface)
            .stroke(Stroke::new(1.0_f32, tokens.border))
            .corner_radius(RADIUS_CARD)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("Движок шейдеров: {}", engine.display_name()))
                            .color(tokens.text)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(format!(
                            "GPU: {} | OpenGL: {}",
                            gpu.renderer, gpu.opengl_version
                        ))
                        .color(if gpu.is_supported {
                            tokens.text_muted
                        } else {
                            tokens.danger
                        })
                        .small(),
                    );
                });
                let (packs, active) =
                    cached_fs_value(ui.ctx(), ("mq_shaderpacks_cache", inst_dir.clone()), || {
                        (
                            mq_mods::shaders::ShaderManager::list_shaderpacks(&inst_dir),
                            mq_mods::shaders::ShaderManager::read_active_shaderpack(&inst_dir),
                        )
                    });
                if !packs.is_empty() {
                    ui.add_space(4.0);
                    for pack in packs {
                        let mut is_active = active.as_deref() == Some(pack.as_str());
                        ui.horizontal_wrapped(|ui| {
                            if crate::components::toggle_switch(ui, &tokens, &mut is_active)
                                .changed()
                            {
                                let next = if is_active { Some(pack.as_str()) } else { None };
                                let _ = mq_mods::shaders::ShaderManager::set_active_shaderpack(
                                    &inst_dir, next,
                                );
                                invalidate_fs_cache::<(Vec<String>, Option<String>)>(
                                    ui.ctx(),
                                    ("mq_shaderpacks_cache", inst_dir.clone()),
                                );
                            }
                            ui.label(RichText::new(&pack).color(tokens.text));
                            if is_active {
                                version_badge(ui, "Используется", tokens.accent);
                            }
                        });
                    }
                }
            });
        ui.add_space(8.0);

        self.render_pack_tab(
            ui,
            state,
            inst_id,
            "shaderpacks",
            crate::catalog_state::CatalogTab::Shaders,
            "Шейдерпаки",
            "Скачать шейдеры",
            "Открыть shaderpacks/",
            "Шейдеры не установлены. Скачайте их из Каталога или добавьте .zip файл (для работы требуется Iris или OptiFine).",
            "Скачать шейдеры из каталога (Modrinth / CurseForge)",
            "Путь к .zip шейдерпаку или перетащите .zip в окно...",
            true,
        );
    }

    fn render_worlds_tab(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        inst_id: &mq_core::InstanceId,
    ) {
        let tokens = Tokens::from_config(&state.config.theme);
        let inst_dir = state.paths.instance_dir(inst_id.as_str());
        let backups_root = state.paths.backups_dir();

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
            ui.label(
                RichText::new("Сохранения миров (Worlds)")
                    .strong()
                    .size(14.0),
            );
            if secondary_glass_button(ui, Some(VectorIcon::Folder), "Открыть saves/").clicked()
            {
                let saves_dir = inst_dir.join("saves");
                let _ = std::fs::create_dir_all(&saves_dir);
                let _ = crate::opener::open_that(saves_dir);
            }
            if secondary_glass_button(ui, Some(VectorIcon::Refresh), "Автобэкап сейчас").clicked()
            {
                let _ = state.cmd_sender.send(UiWorkerCommand::AutoBackupWorlds {
                    instance_id: inst_id.clone(),
                });
            }
        });

        ui.add_space(6.0);

        let worlds = cached_fs_value(
            ui.ctx(),
            ("mq_worlds_cache", inst_dir.clone(), state.worlds_cache_gen),
            || list_worlds(&inst_dir).unwrap_or_default(),
        );
        if worlds.is_empty() {
            ui.add_space(20.0);
            ui.label(
                RichText::new("В этом инстансе пока нет сохранений. Создайте мир в игре!")
                    .color(TEXT_MUTED),
            );
        } else {
            for w in &worlds {
                let card = egui::Frame::new()
                    .fill(CARD_SURFACE)
                    .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
                    .corner_radius(RADIUS_CARD)
                    .inner_margin(egui::Margin::same(10));

                card.show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                        let (r, _) =
                            ui.allocate_exact_size(Vec2::splat(18.0), egui::Sense::hover());
                        paint_vector_icon(ui.painter(), r, VectorIcon::Cube, ACCENT_EMERALD);
                        ui.add(
                            egui::Label::new(
                                RichText::new(&w.display_name).color(TEXT_PRIMARY).strong(),
                            )
                            .truncate(),
                        );
                        version_badge(ui, &w.game_mode, ACCENT_COBALT);
                        let size_bytes = state
                            .world_sizes
                            .get(&(inst_id.clone(), w.folder_name.clone()))
                            .copied()
                            .unwrap_or(w.size_bytes);
                        if size_bytes == 0 && !state.computing_world_sizes.contains(inst_id) {
                            state.computing_world_sizes.insert(inst_id.clone());
                            let _ = state.cmd_sender.send(UiWorkerCommand::ComputeWorldSizes {
                                instance_id: inst_id.clone(),
                            });
                        }
                        let mib = (size_bytes as f64) / (1024.0 * 1024.0);
                        ui.label(
                            RichText::new(format!("{mib:.1} МБ"))
                                .color(TEXT_MUTED)
                                .small(),
                        );
                    });

                    ui.label(
                        RichText::new(format!("Папка: saves/{}", w.folder_name))
                            .color(TEXT_MUTED)
                            .small(),
                    );

                    let world_dir = inst_dir.join("saves").join(&w.folder_name);
                    let nbt_meta =
                        cached_fs_value(ui.ctx(), ("mq_world_nbt", world_dir.clone()), || {
                            mq_instances::read_world_level_dat(&world_dir)
                        });
                    if let Some(ref meta) = nbt_meta {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(8.0, 2.0);
                            if let Some(seed) = meta.seed {
                                ui.label(
                                    RichText::new(format!("Сид: {seed}"))
                                        .color(TEXT_MUTED)
                                        .small(),
                                );
                                if secondary_glass_button(ui, None, "Копировать").clicked()
                                {
                                    ui.ctx().copy_text(seed.to_string());
                                    state.status_message =
                                        Some(("Сид скопирован в буфер обмена".to_string(), false));
                                }
                            }
                            if let Some(ref ver) = meta.version_name {
                                ui.label(
                                    RichText::new(format!("Версия: {ver}"))
                                        .color(TEXT_MUTED)
                                        .small(),
                                );
                            }
                            if let Some((x, y, z)) = meta.spawn_xyz {
                                ui.label(
                                    RichText::new(format!("Спавн: {x}, {y}, {z}"))
                                        .color(TEXT_MUTED)
                                        .small(),
                                );
                            }
                            if meta.hardcore {
                                render_badge(ui, &tokens, "Хардкор", BadgeKind::Danger);
                            }
                        });
                    }

                    // Datapacks manager
                    let datapacks = cached_fs_value(
                        ui.ctx(),
                        ("mq_world_datapacks", world_dir.clone()),
                        || mq_instances::list_world_datapacks(&world_dir),
                    );
                    let dp_count = datapacks.len();
                    let is_expanded =
                        self.expanded_datapack_world.as_deref() == Some(&w.folder_name);
                    let toggle_text = if is_expanded {
                        format!("Датапаки ({dp_count}) [-]")
                    } else {
                        format!("Датапаки ({dp_count}) [+]")
                    };
                    if secondary_glass_button(ui, None, &toggle_text).clicked() {
                        if is_expanded {
                            self.expanded_datapack_world = None;
                        } else {
                            self.expanded_datapack_world = Some(w.folder_name.clone());
                        }
                    }
                    if is_expanded {
                        ui.indent(("datapacks_indent", &w.folder_name), |ui| {
                            for dp in &datapacks {
                                ui.horizontal(|ui| {
                                    let mut enabled = dp.enabled;
                                    if ui.checkbox(&mut enabled, &dp.name).changed() {
                                        let _ = mq_instances::toggle_world_datapack(
                                            &world_dir,
                                            &dp.file_name,
                                        );
                                        invalidate_fs_cache::<Vec<mq_instances::DatapackEntry>>(
                                            ui.ctx(),
                                            ("mq_world_datapacks", world_dir.clone()),
                                        );
                                    }
                                    if secondary_glass_button(
                                        ui,
                                        Some(VectorIcon::Trash),
                                        "Удалить",
                                    )
                                    .clicked()
                                    {
                                        let _ = mq_instances::delete_world_datapack(
                                            &world_dir,
                                            &dp.file_name,
                                        );
                                        invalidate_fs_cache::<Vec<mq_instances::DatapackEntry>>(
                                            ui.ctx(),
                                            ("mq_world_datapacks", world_dir.clone()),
                                        );
                                    }
                                });
                            }
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.datapack_input_path)
                                        .hint_text("Путь к .zip датапака")
                                        .desired_width(180.0),
                                );
                                if primary_emerald_button(ui, Some(VectorIcon::Plus), "Добавить")
                                    .clicked()
                                    && !self.datapack_input_path.trim().is_empty()
                                {
                                    let src =
                                        std::path::PathBuf::from(self.datapack_input_path.trim());
                                    if mq_instances::add_world_datapack(&world_dir, &src).is_ok() {
                                        invalidate_fs_cache::<Vec<mq_instances::DatapackEntry>>(
                                            ui.ctx(),
                                            ("mq_world_datapacks", world_dir.clone()),
                                        );
                                        self.datapack_input_path.clear();
                                    }
                                }
                            });
                        });
                    }

                    ui.add_space(4.0);

                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                        Self::quick_play_enter_button(
                            ui,
                            state,
                            inst_id,
                            mq_launch::QuickPlayTarget::Singleplayer(w.folder_name.clone()),
                        );
                        if secondary_glass_button(ui, Some(VectorIcon::Download), "Бэкап").clicked()
                        {
                            let _ = state.cmd_sender.send(UiWorkerCommand::BackupWorld {
                                instance_id: inst_id.clone(),
                                folder_name: w.folder_name.clone(),
                            });
                            state.status_message =
                                Some(("Создание бэкапа мира запущено...".to_string(), false));
                        }
                        if secondary_glass_button(ui, Some(VectorIcon::Download), "Экспорт")
                            .clicked()
                        {
                            let export_path = backups_root.join(format!("{}.zip", w.folder_name));
                            let _ = state.cmd_sender.send(UiWorkerCommand::ExportWorld {
                                instance_id: inst_id.clone(),
                                folder_name: w.folder_name.clone(),
                                export_path,
                            });
                            state.status_message =
                                Some(("Экспорт мира запущен...".to_string(), false));
                        }
                        if self.confirm_delete_world.as_deref() == Some(&w.folder_name) {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Точно удалить мир?")
                                        .color(STATUS_WARN)
                                        .small(),
                                );
                                if primary_emerald_button(ui, None, "Подтвердить").clicked()
                                {
                                    match delete_world(&inst_dir, &w.folder_name) {
                                        Ok(()) => {
                                            state.worlds_cache_gen =
                                                state.worlds_cache_gen.wrapping_add(1);
                                            invalidate_fs_cache::<Vec<mq_instances::WorldInfo>>(
                                                ui.ctx(),
                                                (
                                                    "mq_worlds_cache",
                                                    inst_dir.clone(),
                                                    state.worlds_cache_gen,
                                                ),
                                            );
                                            state.status_message = Some((
                                                format!("Мир '{}' удален", w.display_name),
                                                false,
                                            ));
                                        }
                                        Err(e) => {
                                            state.status_message =
                                                Some((format!("Ошибка удаления мира: {e}"), true));
                                        }
                                    }
                                    self.confirm_delete_world = None;
                                }
                                if secondary_glass_button(ui, None, "Отмена").clicked() {
                                    self.confirm_delete_world = None;
                                }
                            });
                        } else if secondary_glass_button(ui, Some(VectorIcon::Trash), "Удалить")
                            .clicked()
                        {
                            self.confirm_delete_world = Some(w.folder_name.clone());
                        }
                    });
                });
                ui.add_space(4.0);
            }
        }

        // Backups list section
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);
        ui.label(
            RichText::new("Резервные копии (Бэкапы)")
                .strong()
                .size(13.0),
        );
        let backups = cached_fs_value(
            ui.ctx(),
            ("mq_backups_cache", inst_id.as_str().to_owned()),
            || list_backups(&backups_root, inst_id.as_str()).unwrap_or_default(),
        );
        if backups.is_empty() {
            ui.label(
                RichText::new("Резервных копий пока нет")
                    .color(TEXT_MUTED)
                    .small(),
            );
        } else {
            for b in backups {
                ui.horizontal(|ui| {
                    let mib = (b.size_bytes as f64) / (1024.0 * 1024.0);
                    ui.label(RichText::new(&b.file_name).color(TEXT_PRIMARY).small());
                    ui.label(
                        RichText::new(format!("{mib:.1} МБ"))
                            .color(TEXT_MUTED)
                            .small(),
                    );
                    if ui.button("Восстановить").clicked() {
                        let _ = state.cmd_sender.send(UiWorkerCommand::RestoreWorld {
                            instance_id: inst_id.clone(),
                            file_name: b.file_name.clone(),
                        });
                        state.status_message =
                            Some(("Восстановление мира запущено...".to_string(), false));
                    }
                });
            }
        }
    }

    fn render_servers_tab(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        inst_id: &mq_core::InstanceId,
    ) {
        let inst_dir = state.paths.instance_dir(inst_id.as_str());
        let mut servers = cached_fs_value(ui.ctx(), ("mq_servers_cache", inst_dir.clone()), || {
            load_servers_dat(&inst_dir).unwrap_or_default()
        });

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
            ui.label(
                RichText::new(format!("Серверы (servers.dat): {}", servers.len()))
                    .strong()
                    .size(14.0),
            );
            if primary_emerald_button(ui, Some(VectorIcon::Plus), "Добавить сервер").clicked()
            {
                self.show_add_server_dialog = true;
            }
        });

        ui.add_space(6.0);

        if servers.is_empty() {
            ui.add_space(20.0);
            ui.label(
                RichText::new("Список серверов пуст. Добавьте сервер кнопкой выше!")
                    .color(TEXT_MUTED),
            );
        } else {
            let mut server_to_delete = None;
            for (idx, s) in servers.iter().enumerate() {
                let card = egui::Frame::new()
                    .fill(CARD_SURFACE)
                    .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
                    .corner_radius(RADIUS_CARD)
                    .inner_margin(egui::Margin::same(10));

                card.show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                        let (r, _) =
                            ui.allocate_exact_size(Vec2::splat(18.0), egui::Sense::hover());
                        paint_vector_icon(ui.painter(), r, VectorIcon::Server, ACCENT_COBALT);
                        ui.add(
                            egui::Label::new(RichText::new(&s.name).color(TEXT_PRIMARY).strong())
                                .truncate(),
                        );
                        ui.label(RichText::new(&s.ip).color(TEXT_MUTED));
                    });

                    if let Some(res) = state.server_pings.get(&s.ip) {
                        let tex_handle = if let Some(ref png_bytes) = res.favicon_png {
                            let tex_id = format!("srv_fav_{}", s.ip);
                            ui.ctx()
                                .data_mut(|d| {
                                    d.get_temp::<egui::TextureHandle>(egui::Id::new(&tex_id))
                                })
                                .or_else(|| {
                                    image::load_from_memory(png_bytes).ok().map(|dyn_img| {
                                        let rgba = dyn_img.to_rgba8();
                                        let (w, h) = rgba.dimensions();
                                        let color_img = egui::ColorImage::from_rgba_unmultiplied(
                                            [w as usize, h as usize],
                                            &rgba,
                                        );
                                        let h = ui.ctx().load_texture(
                                            &tex_id,
                                            color_img,
                                            egui::TextureOptions::LINEAR,
                                        );
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(egui::Id::new(&tex_id), h.clone())
                                        });
                                        h
                                    })
                                })
                        } else {
                            None
                        };

                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                            if let Some(ref handle) = tex_handle {
                                ui.image((handle.id(), Vec2::splat(24.0)));
                            }
                            version_badge(ui, &format!("{} ms", res.latency_ms), ACCENT_EMERALD);
                            version_badge(
                                ui,
                                &format!("{}/{}", res.players_online, res.players_max),
                                ACCENT_COBALT,
                            );
                            if !res.motd_segments.is_empty() {
                                for (code, segment) in &res.motd_segments {
                                    let seg_color = match code {
                                        Some('0') => Color32::from_rgb(0, 0, 0),
                                        Some('1') => Color32::from_rgb(0, 0, 170),
                                        Some('2') => Color32::from_rgb(0, 170, 0),
                                        Some('3') => Color32::from_rgb(0, 170, 170),
                                        Some('4') => Color32::from_rgb(170, 0, 0),
                                        Some('5') => Color32::from_rgb(170, 0, 170),
                                        Some('6') => Color32::from_rgb(255, 170, 0),
                                        Some('7') => Color32::from_rgb(170, 170, 170),
                                        Some('8') => Color32::from_rgb(85, 85, 85),
                                        Some('9') => Color32::from_rgb(85, 85, 255),
                                        Some('a') => Color32::from_rgb(85, 255, 85),
                                        Some('b') => Color32::from_rgb(85, 255, 255),
                                        Some('c') => Color32::from_rgb(255, 85, 85),
                                        Some('d') => Color32::from_rgb(255, 85, 255),
                                        Some('e') => Color32::from_rgb(255, 255, 85),
                                        Some('f') => Color32::from_rgb(255, 255, 255),
                                        _ => TEXT_PRIMARY,
                                    };
                                    ui.colored_label(seg_color, segment);
                                }
                            } else {
                                ui.label(RichText::new(&res.motd).color(TEXT_PRIMARY).small());
                            }
                        });

                        if !res.players_sample.is_empty() {
                            ui.label(
                                RichText::new(format!(
                                    "Игроки в сети: {}",
                                    res.players_sample.join(", ")
                                ))
                                .color(TEXT_MUTED)
                                .small(),
                            );
                        }
                    }

                    ui.add_space(4.0);

                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                        Self::quick_play_enter_button(
                            ui,
                            state,
                            inst_id,
                            mq_launch::QuickPlayTarget::Multiplayer(s.ip.clone()),
                        );
                        let is_pinging = state.pinging_servers.contains(&s.ip);
                        let ping_label = if is_pinging {
                            "Пинг..."
                        } else {
                            "Пинг"
                        };
                        if secondary_glass_button(ui, Some(VectorIcon::Refresh), ping_label)
                            .clicked()
                            && !is_pinging
                        {
                            let ip = s.ip.clone();
                            state.pinging_servers.insert(ip.clone());
                            let _ = state.cmd_sender.send(UiWorkerCommand::PingServer { ip });
                        }
                        if secondary_glass_button(ui, Some(VectorIcon::Trash), "Удалить").clicked()
                        {
                            server_to_delete = Some(idx);
                        }
                    });
                });
                ui.add_space(4.0);
            }

            if let Some(del_idx) = server_to_delete {
                servers.remove(del_idx);
                if let Err(e) = save_servers_dat(&inst_dir, &servers) {
                    state.status_message =
                        Some((format!("Ошибка сохранения servers.dat: {e}"), true));
                }
                invalidate_fs_cache::<Vec<ServerEntry>>(
                    ui.ctx(),
                    ("mq_servers_cache", inst_dir.clone()),
                );
            }
        }

        let srv_modal_anim = crate::animation::animate_modal_presence(
            ui.ctx(),
            egui::Id::new("mq_add_server_modal"),
            self.show_add_server_dialog,
        );
        if srv_modal_anim.should_render {
            let srv_win = egui::Window::new("Добавление сервера")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]);
            crate::animation::show_animated_modal_window(
                ui.ctx(),
                egui::Id::new("mq_add_server_modal_win"),
                &srv_modal_anim,
                srv_win,
                |ui| {
                    let modal_w = 320.0_f32.min(ui.ctx().screen_rect().width() - 32.0);
                    ui.set_width(modal_w);
                    ui.label("Название сервера:");
                    ui.text_edit_singleline(&mut self.new_server_name);
                    ui.add_space(4.0);
                    ui.label("Адрес сервера (IP / Host):");
                    ui.text_edit_singleline(&mut self.new_server_ip);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_emerald_button(ui, Some(VectorIcon::Check), "Добавить").clicked()
                            && !self.new_server_ip.trim().is_empty()
                        {
                            let name = if self.new_server_name.trim().is_empty() {
                                self.new_server_ip.trim().to_string()
                            } else {
                                self.new_server_name.trim().to_string()
                            };
                            servers.push(ServerEntry {
                                name,
                                ip: self.new_server_ip.trim().to_string(),
                                icon: None,
                                accept_textures: 1,
                            });
                            if let Err(e) = save_servers_dat(&inst_dir, &servers) {
                                state.status_message =
                                    Some((format!("Ошибка сохранения servers.dat: {e}"), true));
                            }
                            invalidate_fs_cache::<Vec<ServerEntry>>(
                                ui.ctx(),
                                ("mq_servers_cache", inst_dir.clone()),
                            );
                            self.new_server_name.clear();
                            self.new_server_ip.clear();
                            self.show_add_server_dialog = false;
                        }
                        if secondary_glass_button(ui, None, "Отмена").clicked() {
                            self.show_add_server_dialog = false;
                        }
                    });
                },
            );
        }
    }

    fn render_screenshots_tab(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        inst_id: &mq_core::InstanceId,
    ) {
        let inst_dir = state.paths.instance_dir(inst_id.as_str());
        let screenshots =
            cached_fs_value(ui.ctx(), ("mq_screenshots_cache", inst_dir.clone()), || {
                list_screenshots(&inst_dir).unwrap_or_default()
            });

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
            ui.label(
                RichText::new(format!("Скриншоты: {}", screenshots.len()))
                    .strong()
                    .size(14.0),
            );
            if secondary_glass_button(ui, Some(VectorIcon::Folder), "Открыть screenshots/")
                .clicked()
            {
                let scr_dir = inst_dir.join("screenshots");
                let _ = std::fs::create_dir_all(&scr_dir);
                let _ = crate::opener::open_that(scr_dir);
            }
        });

        ui.add_space(6.0);

        if screenshots.is_empty() {
            ui.add_space(20.0);
            ui.label(RichText::new("Скриншотов пока нет. Нажмите F2 в игре!").color(TEXT_MUTED));
        } else {
            ScrollArea::vertical()
                .id_salt("screenshots_scroll")
                .show(ui, |ui| {
                    let mut file_to_delete = None;
                    for s in screenshots {
                        let card = egui::Frame::new()
                            .fill(CARD_SURFACE)
                            .stroke(Stroke::new(1.0_f32, BORDER_SUBTLE))
                            .corner_radius(RADIUS_CARD)
                            .inner_margin(egui::Margin::same(8));

                        card.show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (r, _) =
                                    ui.allocate_exact_size(Vec2::splat(18.0), egui::Sense::hover());
                                paint_vector_icon(ui.painter(), r, VectorIcon::Eye, ACCENT_EMERALD);

                                let right_w = 120.0;
                                let middle_w = (ui.available_width() - right_w - 8.0).max(60.0);

                                ui.allocate_ui_with_layout(
                                    Vec2::new(middle_w, 38.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        let text_w = (ui.available_width() - 8.0).max(40.0);
                                        ui.add_sized(
                                            [text_w, 18.0],
                                            egui::Label::new(
                                                RichText::new(&s.file_name)
                                                    .color(TEXT_PRIMARY)
                                                    .strong(),
                                            )
                                            .truncate(),
                                        );
                                        let mib = (s.size_bytes as f64) / (1024.0 * 1024.0);
                                        ui.add_sized(
                                            [text_w, 16.0],
                                            egui::Label::new(
                                                RichText::new(format!(
                                                    "{mib:.2} МБ • {}",
                                                    s.created_at.format("%Y-%m-%d %H:%M")
                                                ))
                                                .color(TEXT_MUTED)
                                                .small(),
                                            )
                                            .truncate(),
                                        );
                                    },
                                );

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if secondary_glass_button(ui, Some(VectorIcon::Trash), "")
                                            .on_hover_text("Удалить скриншот")
                                            .clicked()
                                        {
                                            file_to_delete = Some(s.path.clone());
                                        }
                                        if secondary_glass_button(
                                            ui,
                                            Some(VectorIcon::Eye),
                                            "Открыть",
                                        )
                                        .clicked()
                                        {
                                            let _ = crate::opener::open_that(&s.path);
                                        }
                                    },
                                );
                            });
                        });
                        ui.add_space(4.0);
                    }
                    if let Some(path) = file_to_delete {
                        let _ = std::fs::remove_file(path);
                        invalidate_fs_cache::<Vec<mq_instances::ScreenshotInfo>>(
                            ui.ctx(),
                            ("mq_screenshots_cache", inst_dir.clone()),
                        );
                    }
                });
        }
    }

    fn render_settings_tab(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        inst_id: &mq_core::InstanceId,
    ) {
        if let Some(inst) = state.instances.iter_mut().find(|i| i.id == *inst_id) {
            ui.label(RichText::new("Конфигурация инстанса").strong());
            ui.add_space(6.0);

            ui.label(format!(
                "Выделенная оперативная память (RAM): {} МБ",
                inst.memory_max_mb
            ));
            ui.add(egui::Slider::new(&mut inst.memory_max_mb, 1024..=16384).step_by(512.0));

            ui.add_space(6.0);
            if primary_emerald_button(ui, Some(VectorIcon::Cpu), "Оптимизировать под мой ПК")
                .on_hover_text("Автоматически подобрать объем RAM и флаги JVM (Generational ZGC / G1GC) под конфигурацию ПК")
                .clicked()
            {
                let total_ram = mq_core::platform::total_memory_mb();
                let mod_count = state.installed_mods.len();
                let rec = mq_launch::recommend_jvm_tuning(total_ram, 21, mod_count);
                inst.memory_min_mb = rec.memory_min_mb;
                inst.memory_max_mb = rec.memory_max_mb;
                inst.jvm_args = rec.jvm_args;
                if let Err(e) = state.instance_mgr.save_instance(inst) {
                    state.status_message =
                        Some((format!("Ошибка сохранения параметров: {e}"), true));
                } else {
                    state.status_message = Some((rec.summary_ru, false));
                }
            }

            ui.add_space(8.0);
            egui::CollapsingHeader::new("Расширенные настройки запуска (для опытных)")
                .default_open(false)
                .show(ui, |ui| {
                    let wrapper_label = if cfg!(target_os = "windows") {
                        "Команда-обёртка запуска (необязательно; на Windows обычно не требуется):"
                    } else {
                        "Команда-враппер запуска (например, gamemoderun, mangohud, prime-run):"
                    };
                    ui.label(wrapper_label);
                    let mut wrapper_str = inst.wrapper_cmd.clone().unwrap_or_default();
                    if ui.text_edit_singleline(&mut wrapper_str).changed() {
                        inst.wrapper_cmd = if wrapper_str.trim().is_empty() {
                            None
                        } else {
                            Some(wrapper_str.trim().to_string())
                        };
                    }

                    ui.add_space(8.0);
                    ui.label("Дополнительные аргументы JVM (через пробел):");
                    let mut jvm_args_str = inst.jvm_args.join(" ");
                    if ui.text_edit_singleline(&mut jvm_args_str).changed() {
                        inst.jvm_args = jvm_args_str
                            .split_whitespace()
                            .map(|s| s.to_string())
                            .collect();
                    }
                });

            ui.add_space(8.0);
            ui.label("Язык Minecraft для этого инстанса (80+ языков):");
            let effective_loc = inst
                .mc_locale
                .as_deref()
                .unwrap_or(state.config.default_mc_locale.as_str());
            let current_loc_info = mq_meta::find_mc_locale(effective_loc);
            let display_name = current_loc_info
                .map(|i| i.display_label())
                .unwrap_or_else(|| effective_loc.to_string());

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                version_badge(ui, &display_name, ACCENT_EMERALD);
                if secondary_glass_button(ui, Some(VectorIcon::Globe), "Выбрать другой язык...")
                    .clicked()
                {
                    self.show_locale_dialog = true;
                }
                if inst.mc_locale.is_some()
                    && secondary_glass_button(
                        ui,
                        Some(VectorIcon::Cross),
                        "Сбросить на язык по умолчанию",
                    )
                    .clicked()
                {
                    inst.mc_locale = None;
                }
            });

            let loc_modal_anim = crate::animation::animate_modal_presence(
                ui.ctx(),
                egui::Id::new("mq_inst_locale_modal"),
                self.show_locale_dialog,
            );
            if loc_modal_anim.should_render {
                let loc_win = egui::Window::new("Выбор языка Minecraft")
                    .collapsible(false)
                    .resizable(true)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]);
                crate::animation::show_animated_modal_window(
                    ui.ctx(),
                    egui::Id::new("mq_inst_locale_modal_win"),
                    &loc_modal_anim,
                    loc_win,
                    |ui| {
                        let modal_w = 380.0_f32.min(ui.ctx().screen_rect().width() - 32.0);
                        ui.set_width(modal_w);
                        ui.horizontal(|ui| {
                            let (r, _) =
                                ui.allocate_exact_size(Vec2::splat(16.0), egui::Sense::hover());
                            paint_vector_icon(ui.painter(), r, VectorIcon::Search, ACCENT_COBALT);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.locale_search_query)
                                    .hint_text("Поиск языка..."),
                            );
                        });
                        ui.add_space(6.0);
                        ScrollArea::vertical()
                            .max_height(200.0)
                            .id_salt("inst_locale_modal_scroll")
                            .show(ui, |ui| {
                                for loc in mq_meta::search_mc_locales(&self.locale_search_query)
                                    .iter()
                                    .take(30)
                                {
                                    let is_active = inst.mc_locale.as_deref() == Some(loc.code);
                                    if ui
                                        .selectable_label(is_active, loc.display_label())
                                        .clicked()
                                    {
                                        inst.mc_locale = Some(loc.code.to_string());
                                        self.show_locale_dialog = false;
                                    }
                                }
                            });
                        ui.add_space(8.0);
                        if secondary_glass_button(ui, None, "Закрыть").clicked() {
                            self.show_locale_dialog = false;
                        }
                    },
                );
            }

            ui.add_space(14.0);
            let mut should_delete = false;
            let mut should_save = false;

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(10.0, 8.0);
                if primary_emerald_button(ui, Some(VectorIcon::Check), "Сохранить параметры")
                    .clicked()
                {
                    should_save = true;
                }

                if danger_button(ui, Some(VectorIcon::Trash), "Удалить инстанс").clicked()
                {
                    should_delete = true;
                }
            });

            if should_save {
                match state.instance_mgr.save_instance(inst) {
                    Ok(()) => {
                        state.status_message =
                            Some(("Параметры инстанса сохранены".to_string(), false));
                    }
                    Err(e) => {
                        state.status_message =
                            Some((format!("Ошибка сохранения параметров: {e}"), true));
                    }
                }
            }

            if should_delete {
                self.show_delete_dialog = true;
                self.instance_to_delete = Some((inst_id.clone(), inst.name.clone()));
            }
        }
    }

    fn render_header_action_buttons(
        &mut self,
        ui: &mut Ui,
        state: &mut UiState,
        tokens: &Tokens,
        in_embedded_catalog: bool,
        reverse_order: bool,
    ) {
        let render_create = |ui: &mut Ui, state: &mut UiState| {
            if button_primary(ui, tokens, Some(VectorIcon::Plus), "Создать инстанс").clicked()
            {
                state.show_create_instance_dialog = true;
            }
        };
        let render_mode_switch = |ui: &mut Ui, view_mode: &mut InstanceViewMode| {
            let view_opts = [
                SegmentedOption::new(InstanceViewMode::Grid, "Сетка")
                    .with_icon(VectorIcon::Layers)
                    .with_tooltip("Отображение плитками"),
                SegmentedOption::new(InstanceViewMode::List, "Список")
                    .with_icon(VectorIcon::Terminal)
                    .with_tooltip("Отображение списком"),
            ];
            segmented_switch(ui, tokens, view_mode, &view_opts);
        };
        let render_import_pack = |this: &mut Self, ui: &mut Ui| {
            if button_secondary(ui, tokens, Some(VectorIcon::Download), "Импорт сборки").clicked()
            {
                this.import_error = None;
                this.show_import_dialog = true;
            }
        };
        let render_import_ext = |ui: &mut Ui, state: &mut UiState| {
            if button_secondary(
                ui,
                tokens,
                Some(VectorIcon::Download),
                "Импорт из лаунчеров",
            )
            .clicked()
            {
                state.show_external_import_modal = true;
                let _ = state
                    .cmd_sender
                    .send(UiWorkerCommand::DiscoverExternalInstances);
            }
        };

        if in_embedded_catalog {
            if reverse_order {
                render_create(ui, state);
                ui.add_space(6.0);
                render_mode_switch(ui, &mut self.view_mode);
            } else {
                render_mode_switch(ui, &mut self.view_mode);
                render_create(ui, state);
            }
        } else if reverse_order {
            render_create(ui, state);
            ui.add_space(6.0);
            render_import_pack(self, ui);
            ui.add_space(6.0);
            render_import_ext(ui, state);
        } else {
            render_create(ui, state);
            render_import_pack(self, ui);
            render_import_ext(ui, state);
        }
    }

    fn quick_play_enter_button(
        ui: &mut Ui,
        state: &mut UiState,
        inst_id: &mq_core::InstanceId,
        target: mq_launch::QuickPlayTarget,
    ) {
        let is_running = state.running_instances.contains_key(inst_id);
        let can_quick_play = !state.is_launching && !is_running;
        let label = if state.is_launching {
            "Запуск..."
        } else {
            "Войти"
        };
        let resp = ui
            .add_enabled_ui(can_quick_play, |ui| {
                primary_emerald_button(ui, Some(VectorIcon::Play), label)
            })
            .inner
            .on_disabled_hover_text(if is_running {
                "Инстанс уже запущен"
            } else {
                "Идет запуск игры..."
            });
        if can_quick_play && resp.clicked() {
            state.is_launching = true;
            let _ = state
                .cmd_sender
                .send(UiWorkerCommand::LaunchInstanceQuickPlay {
                    instance_id: inst_id.clone(),
                    quick_play: target,
                });
        }
    }
}

#[derive(Clone)]
struct CachedFsValue<T> {
    value: T,
    refreshed_at: std::time::Instant,
}

fn cached_fs_value<T, K, F>(ctx: &egui::Context, key: K, compute: F) -> T
where
    T: Clone + Send + Sync + 'static,
    K: std::hash::Hash,
    F: FnOnce() -> T,
{
    const FS_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(2);
    let id = egui::Id::new(key);
    if let Some(entry) = ctx.data(|d| d.get_temp::<CachedFsValue<T>>(id)) {
        if entry.refreshed_at.elapsed() < FS_CACHE_TTL {
            return entry.value;
        }
    }
    let value = compute();
    ctx.data_mut(|d| {
        d.insert_temp(
            id,
            CachedFsValue {
                value: value.clone(),
                refreshed_at: std::time::Instant::now(),
            },
        );
    });
    value
}

fn invalidate_fs_cache<T: 'static>(ctx: &egui::Context, key: impl std::hash::Hash) {
    let id = egui::Id::new(key);
    ctx.data_mut(|d| {
        d.remove::<CachedFsValue<T>>(id);
    });
}

fn strip_disabled_suffix(filename: &str) -> Option<&str> {
    let (stem, ext) = filename.rsplit_once('.')?;
    if !stem.is_empty() && ext.eq_ignore_ascii_case("disabled") {
        Some(stem)
    } else {
        None
    }
}

fn list_packs_in_dir(dir: &std::path::Path) -> Vec<(String, bool, u64)> {
    let mut packs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_disabled = strip_disabled_suffix(&name).is_some();
            let size = if path.is_file() {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            } else {
                0
            };
            packs.push((name, !is_disabled, size));
        }
    }
    packs.sort_by(|a, b| a.0.cmp(&b.0));
    packs
}

fn toggle_pack_file(dir: &std::path::Path, filename: &str) {
    let src = dir.join(filename);
    let dst = if let Some(enabled_name) = strip_disabled_suffix(filename) {
        dir.join(enabled_name)
    } else {
        dir.join(format!("{filename}.disabled"))
    };
    if std::fs::rename(&src, &dst).is_err() && dst.is_file() {
        // On Windows, `rename` fails if the destination file already exists.
        let _ = std::fs::remove_file(&dst);
        let _ = std::fs::rename(&src, &dst);
    }
}

fn delete_pack_file(dir: &std::path::Path, filename: &str) -> std::io::Result<()> {
    let target = mq_core::resolve_safe_path(dir, filename)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string()))?;
    if target.is_dir() {
        std::fs::remove_dir_all(target)
    } else {
        std::fs::remove_file(target)
    }
}

/// Copies a local mod (`.jar`) or pack (`.zip`) file into an instance content directory.
fn install_local_content_file(
    target_dir: &std::path::Path,
    src_path: &std::path::Path,
    allowed_exts: &[&str],
) -> Result<String, String> {
    let cleaned_src = crate::opener::clean_user_path(&src_path.to_string_lossy());
    let src_path = std::path::Path::new(&cleaned_src);
    if !src_path.exists() || !src_path.is_file() {
        return Err(format!("Файл не найден: {}", src_path.display()));
    }
    let file_name = src_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .ok_or_else(|| "Некорректное имя файла".to_string())?;
    let lower = file_name.to_ascii_lowercase();
    if !allowed_exts
        .iter()
        .any(|ext| lower.ends_with(&ext.to_ascii_lowercase()))
    {
        return Err(format!(
            "Неподдерживаемый формат файла «{}» (ожидается {})",
            file_name,
            allowed_exts.join(", ")
        ));
    }
    std::fs::create_dir_all(target_dir).map_err(|e| {
        format!(
            "Не удалось создать папку {}: {e}",
            crate::opener::strip_windows_verbatim_prefix(&target_dir.display().to_string())
        )
    })?;
    let dst_path = target_dir.join(&file_name);
    let same_file = matches!(
        (
            std::fs::canonicalize(src_path),
            std::fs::canonicalize(&dst_path)
        ),
        (Ok(s), Ok(d)) if s == d
    );
    if same_file {
        return Ok(file_name);
    }
    std::fs::copy(src_path, &dst_path)
        .map_err(|e| format!("Не удалось скопировать файл {file_name}: {e}"))?;
    Ok(file_name)
}

fn version_search_input(
    ui: &mut Ui,
    query: &mut String,
    width: f32,
    tokens: &Tokens,
) -> egui::Response {
    let inner_w = (width - 16.0).max(80.0);
    let frame_resp = egui::Frame::new()
        .fill(tokens.bg_input)
        .stroke(egui::Stroke::new(1.0_f32, tokens.border))
        .corner_radius(6.0_f32)
        .inner_margin(egui::Margin::symmetric(8, 7))
        .show(ui, |ui| {
            ui.set_min_width(inner_w);
            ui.set_min_height(14.0);
            ui.add(
                egui::TextEdit::singleline(query)
                    .frame(false)
                    .hint_text("Поиск версии (1.21.4, 25w09b)...")
                    .desired_width(inner_w)
                    .vertical_align(egui::Align::Center),
            )
        });
    if frame_resp.response.clicked() {
        frame_resp.inner.request_focus();
    }
    frame_resp.response.union(frame_resp.inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_catalogs_contain_expected_categories_and_versions() {
        assert!(ALL_MC_RELEASES.contains(&"1.21.4"));
        assert!(ALL_MC_RELEASES.contains(&"1.7.10"));
        assert!(ALL_MC_SNAPSHOTS.contains(&"25w09b"));
        assert!(ALL_MC_BETAS.contains(&"b1.7.3"));
        assert!(ALL_MC_ALPHAS.contains(&"a1.2.6"));
        assert!(ALL_MC_ALPHAS.contains(&"c0.30_01c"));
    }

    #[test]
    fn version_categories_have_clean_labels_and_colors() {
        assert_eq!(McVersionCategory::Release.badge_label(), "Релиз");
        assert_eq!(McVersionCategory::Snapshot.badge_label(), "Снапшот");
        assert_eq!(McVersionCategory::OldBeta.badge_label(), "Beta");
        assert_eq!(McVersionCategory::OldAlpha.badge_label(), "Alpha");
    }

    #[test]
    fn instances_view_default_configuration() {
        let view = InstancesView::default();
        assert_eq!(view.view_mode, InstanceViewMode::Grid);
        assert_eq!(view.loader_filter, LoaderFilter::All);
        assert!(view.show_releases);
        assert!(!view.show_snapshots);
        assert!(!view.show_betas);
        assert!(!view.show_alphas);
        assert!(view.version_search.is_empty());
        assert!(!view.show_delete_dialog);
        assert!(view.instance_to_delete.is_none());
    }

    fn test_ui_state() -> (tempfile::TempDir, UiState) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = mq_core::MqPaths::with_root(tmp.path().to_path_buf());
        let config = mq_core::AppConfig::default();
        let pool = std::sync::Arc::new(mq_sched::ComputePool::new(2));
        let state = UiState::new(paths, config, pool).expect("ui state");
        (tmp, state)
    }

    #[test]
    fn empty_state_renders_cleanly_without_panic() {
        let (_tmp, mut state) = test_ui_state();
        assert!(state.instances.is_empty());

        let mut view = InstancesView::default();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1024.0, 720.0),
            )),
            ..Default::default()
        };

        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                view.ui(ui, &mut state);
            });
        });
    }

    #[test]
    fn create_dialog_renders_skeleton_and_manifest_with_26x_support() {
        let (_tmp, mut state) = test_ui_state();
        state.show_create_instance_dialog = true;
        state.is_fetching_manifest = true;
        assert!(state.create_inst_name.is_empty());
        assert!(state.create_inst_mc_ver.is_empty());

        let mut view = InstancesView::default();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1024.0, 720.0),
            )),
            ..Default::default()
        };

        // 1. Loading skeleton state
        let _ = ctx.run(input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                view.ui(ui, &mut state);
            });
        });

        // 2. Manifest fetched with 26.x release
        state.latest_release = Some("26.1".to_string());
        state.remote_versions = vec![
            ("26.1".to_string(), "release".to_string()),
            ("26.1-snapshot-1".to_string(), "snapshot".to_string()),
            ("1.21.4".to_string(), "release".to_string()),
        ];
        state.is_fetching_manifest = false;

        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                view.ui(ui, &mut state);
            });
        });

        assert_eq!(state.create_inst_mc_ver, "26.1");
    }

    #[test]
    fn filtered_instances_filters_by_query_and_loader() {
        let (_tmp, mut state) = test_ui_state();
        let fab = state
            .instance_mgr
            .create_instance("My Fabric", "26.1", ModLoaderKind::Fabric, None)
            .expect("fabric");
        let van = state
            .instance_mgr
            .create_instance("Classic Vanilla", "1.21.4", ModLoaderKind::Vanilla, None)
            .expect("vanilla");
        state.instances.push(fab);
        state.instances.push(van);

        let mut view = InstancesView::default();
        assert_eq!(view.filtered_instances(&state).len(), 2);

        view.loader_filter = LoaderFilter::Fabric;
        let only_fab = view.filtered_instances(&state);
        assert_eq!(only_fab.len(), 1);
        assert_eq!(only_fab[0].name, "My Fabric");

        view.loader_filter = LoaderFilter::All;
        view.search_query = "26.1".to_string();
        let by_ver = view.filtered_instances(&state);
        assert_eq!(by_ver.len(), 1);
        assert_eq!(by_ver[0].mc_version, "26.1");
    }

    #[test]
    fn install_local_content_file_copies_matching_extension_and_rejects_others() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src_dir = tmp.path().join("src");
        let dst_dir = tmp.path().join("mods");
        std::fs::create_dir_all(&src_dir).expect("src dir");

        let jar_path = src_dir.join("sodium-fabric.jar");
        std::fs::write(&jar_path, b"jar-bytes").expect("write jar");
        let txt_path = src_dir.join("notes.txt");
        std::fs::write(&txt_path, b"text").expect("write txt");

        let installed =
            install_local_content_file(&dst_dir, &jar_path, &[".jar"]).expect("jar should install");
        assert_eq!(installed, "sodium-fabric.jar");
        assert_eq!(
            std::fs::read(dst_dir.join("sodium-fabric.jar")).expect("copied"),
            b"jar-bytes"
        );

        let err = install_local_content_file(&dst_dir, &txt_path, &[".jar"])
            .expect_err("txt should be rejected");
        assert!(err.contains(".jar"), "unexpected error: {err}");

        // Windows case-insensitive uppercase `.JAR` and `.DISABLED` suffix handling
        let upper_jar = src_dir.join("IRIS-SHADER.JAR");
        std::fs::write(&upper_jar, b"upper-jar").expect("write upper jar");
        let installed_upper =
            install_local_content_file(&dst_dir, &upper_jar, &[".JAR"]).expect("case-insensitive");
        assert_eq!(installed_upper, "IRIS-SHADER.JAR");

        toggle_pack_file(&dst_dir, "IRIS-SHADER.JAR");
        assert!(dst_dir.join("IRIS-SHADER.JAR.disabled").is_file());
        std::fs::rename(
            dst_dir.join("IRIS-SHADER.JAR.disabled"),
            dst_dir.join("IRIS-SHADER.JAR.DISABLED"),
        )
        .expect("rename to uppercase DISABLED");
        let packs = list_packs_in_dir(&dst_dir);
        let iris_entry = packs
            .iter()
            .find(|(n, _, _)| n == "IRIS-SHADER.JAR.DISABLED")
            .expect("listed uppercase disabled pack");
        assert!(!iris_entry.1, "uppercase .DISABLED must report as disabled");
        toggle_pack_file(&dst_dir, "IRIS-SHADER.JAR.DISABLED");
        assert!(dst_dir.join("IRIS-SHADER.JAR").is_file());
    }

    #[test]
    fn in_instance_catalog_mode_syncs_and_renders_for_mods_resourcepacks_and_shaders() {
        let (_tmp, mut state) = test_ui_state();
        let inst = state
            .instance_mgr
            .create_instance("Test Fabric", "1.21.4", ModLoaderKind::Fabric, None)
            .expect("fabric instance");
        let inst_id = inst.id.clone();
        state.instances.push(inst);
        state.selected_instance_id = Some(inst_id.clone());
        // Prevent auto-triggering a background search request beyond testing sync.
        state.catalog.loading = true;

        let mut view = InstancesView {
            view_mode: InstanceViewMode::List,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1180.0, 740.0),
            )),
            ..Default::default()
        };

        for (detail_tab, expected_cat_tab, expected_loader) in [
            (
                InstanceDetailTab::Mods,
                crate::catalog_state::CatalogTab::Mods,
                Some(ModLoaderKind::Fabric),
            ),
            (
                InstanceDetailTab::ResourcePacks,
                crate::catalog_state::CatalogTab::ResourcePacks,
                None,
            ),
            (
                InstanceDetailTab::ShaderPacks,
                crate::catalog_state::CatalogTab::Shaders,
                None,
            ),
        ] {
            view.current_tab = detail_tab;
            view.content_sub_mode = ContentSubMode::Catalog;
            let _ = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    view.ui(ui, &mut state);
                });
            });
            assert_eq!(state.selected_instance_id.as_ref(), Some(&inst_id));
            assert_eq!(state.catalog.tab, expected_cat_tab);
            assert_eq!(state.catalog.filters.mc_version, "1.21.4");
            assert_eq!(state.catalog.filters.loader, expected_loader);
        }
    }

    #[test]
    fn embedded_catalog_preserves_usable_results_height_with_active_job() {
        let (_tmp, mut state) = test_ui_state();
        let inst = state
            .instance_mgr
            .create_instance("quilt-26.3", "26.3", ModLoaderKind::Quilt, None)
            .expect("quilt instance");
        let inst_id = inst.id.clone();
        state.instances.push(inst);
        state.selected_instance_id = Some(inst_id);
        state.catalog.searched = true;
        state.catalog.loading = false;
        state.catalog.hits = vec![
            mq_mods::CatalogHit {
                source: mq_mods::CatalogSource::Modrinth,
                project_id: "sodium".into(),
                slug: "sodium".into(),
                project_kind: mq_mods::ProjectKind::Mod,
                title: "Sodium".into(),
                description: "Optimization mod".into(),
                author: "JellySquid".into(),
                downloads: 34_000_000,
                icon_url: None,
                loaders: vec!["fabric".into(), "quilt".into()],
                game_versions: vec!["26.3".into()],
                latest_version: None,
            },
            mq_mods::CatalogHit {
                source: mq_mods::CatalogSource::CurseForge,
                project_id: "32274".into(),
                slug: "journeymap".into(),
                project_kind: mq_mods::ProjectKind::Mod,
                title: "JourneyMap".into(),
                description: "Real-time mapping in game or in a web browser".into(),
                author: "techbrew".into(),
                downloads: 120_000_000,
                icon_url: None,
                loaders: vec!["fabric".into()],
                game_versions: vec!["26.3".into()],
                latest_version: None,
            },
        ];
        let (job_id, _) =
            state
                .jobs
                .push("JourneyMap".into(), "Инстанс «quilt-26.3»".into(), 2, None);
        state.jobs.finish(job_id, Ok(()));

        let mut view = InstancesView {
            view_mode: InstanceViewMode::List,
            current_tab: InstanceDetailTab::Mods,
            content_sub_mode: ContentSubMode::Catalog,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        // Central panel size inside a 1024x720 window (minus sidebar and status HUD).
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(810.0, 680.0),
            )),
            ..Default::default()
        };

        // Run 2 frames so ScrollArea sizes settle cleanly.
        let _ = ctx.run(input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                view.ui(ui, &mut state);
            });
        });
        let last_output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                view.ui(ui, &mut state);
            });
        });

        let has_scroll_viewport_for_both_cards = last_output
            .shapes
            .iter()
            .any(|s| s.clip_rect.height() >= 250.0 && s.clip_rect.height() < 550.0);
        assert!(
            has_scroll_viewport_for_both_cards,
            "expected embedded catalog ScrollArea viewport >= 250px tall even with active job"
        );
    }

    fn find_text_center_in_shape(shape: &egui::epaint::Shape, needle: &str) -> Option<egui::Pos2> {
        match shape {
            egui::epaint::Shape::Text(ts) => {
                (ts.galley.text() == needle).then(|| ts.pos + ts.galley.size() * 0.5)
            }
            egui::epaint::Shape::Vec(shapes) => shapes
                .iter()
                .find_map(|s| find_text_center_in_shape(s, needle)),
            _ => None,
        }
    }

    fn find_text_center(shapes: &[egui::epaint::ClippedShape], needle: &str) -> egui::Pos2 {
        shapes
            .iter()
            .find_map(|cs| find_text_center_in_shape(&cs.shape, needle))
            .unwrap_or_else(|| panic!("expected text shape '{needle}'"))
    }

    #[test]
    fn grid_card_buttons_receive_clicks_and_trigger_actions() {
        let (_tmp, mut state) = test_ui_state();
        let inst = state
            .instance_mgr
            .create_instance("quilt-26.3", "26.3", ModLoaderKind::Quilt, None)
            .expect("quilt instance");
        state.instances.push(inst);

        let mut view = InstancesView {
            view_mode: InstanceViewMode::Grid,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let make_input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1024.0, 720.0),
            )),
            events,
            ..Default::default()
        };
        let click_at = |view: &mut InstancesView, state: &mut UiState, pos: egui::Pos2| {
            for pressed in [true, false] {
                let _ = ctx.run(
                    make_input(vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]),
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            view.ui(ui, state);
                        });
                    },
                );
            }
        };

        // 1. Settle layout so card and child button rects are registered.
        let _ = ctx.run(make_input(vec![]), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                view.ui(ui, &mut state);
            });
        });
        let settled = ctx.run(make_input(vec![]), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                view.ui(ui, &mut state);
            });
        });

        // 2. Click "Моды" inside the Grid card.
        let mods_pos = find_text_center(&settled.shapes, "Моды");
        click_at(&mut view, &mut state, mods_pos);
        assert_eq!(view.view_mode, InstanceViewMode::List);
        assert_eq!(view.current_tab, InstanceDetailTab::Mods);

        // 3. Switch back to Grid and click "Детали".
        view.view_mode = InstanceViewMode::Grid;
        let settled = ctx.run(make_input(vec![]), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                view.ui(ui, &mut state);
            });
        });
        let details_pos = find_text_center(&settled.shapes, "Детали");
        click_at(&mut view, &mut state, details_pos);
        assert_eq!(view.view_mode, InstanceViewMode::List);
        assert_eq!(view.current_tab, InstanceDetailTab::Console);

        // 4. Switch back to Grid and click "Скачать".
        view.view_mode = InstanceViewMode::Grid;
        let settled = ctx.run(make_input(vec![]), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                view.ui(ui, &mut state);
            });
        });
        let download_pos = find_text_center(&settled.shapes, "Скачать");
        click_at(&mut view, &mut state, download_pos);
        assert!(
            state.is_launching,
            "expected Скачать button click to trigger launch"
        );
    }

    #[test]
    fn play_button_is_placed_left_of_game_folder_in_action_toolbar() {
        let (_tmp, mut state) = test_ui_state();
        let inst = state
            .instance_mgr
            .create_instance("Forge Dawn", "1.20.1", ModLoaderKind::Forge, None)
            .expect("forge instance");
        let inst_id = inst.id.clone();
        state.instances.push(inst);
        state.selected_instance_id = Some(inst_id.clone());

        let mut view = InstancesView {
            view_mode: InstanceViewMode::List,
            current_tab: InstanceDetailTab::Console,
            ..Default::default()
        };

        let ctx = egui::Context::default();
        for width in [1180.0_f32, 760.0_f32] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(width, 740.0),
                )),
                ..Default::default()
            };

            // 1. Uninstalled instance: "Скачать и играть" is left of "Папка игры" on the same row.
            let _ = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    view.ui(ui, &mut state);
                });
            });
            let out_uninstalled = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    view.ui(ui, &mut state);
                });
            });
            let play_pos = find_text_center(&out_uninstalled.shapes, "Скачать и играть");
            let folder_pos = find_text_center(&out_uninstalled.shapes, "Папка игры");
            assert!(
                play_pos.x < folder_pos.x,
                "at width {width}, expected 'Скачать и играть' ({}) to be left of 'Папка игры' ({})",
                play_pos.x,
                folder_pos.x
            );
            assert!(
                (play_pos.y - folder_pos.y).abs() < 4.0,
                "at width {width}, expected Play ({}) and Folder ({}) on the same toolbar row",
                play_pos.y,
                folder_pos.y
            );

            // 2. Installed instance: "Играть" is left of "Папка игры" on the same row.
            let inst_dir = state.paths.instance_dir(inst_id.as_str());
            std::fs::create_dir_all(&inst_dir).expect("inst dir");
            std::fs::write(inst_dir.join("install_state.json"), r#"{"status":"ready"}"#)
                .expect("write install_state");
            state.invalidate_install_cache(Some(&inst_id));

            let _ = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    view.ui(ui, &mut state);
                });
            });
            let out_installed = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    view.ui(ui, &mut state);
                });
            });
            let play_ready = find_text_center(&out_installed.shapes, "Играть");
            let folder_ready = find_text_center(&out_installed.shapes, "Папка игры");
            assert!(
                play_ready.x < folder_ready.x,
                "at width {width}, expected 'Играть' ({}) to be left of 'Папка игры' ({})",
                play_ready.x,
                folder_ready.x
            );
            assert!(
                (play_ready.y - folder_ready.y).abs() < 4.0,
                "at width {width}, expected 'Играть' ({}) and 'Папка игры' ({}) on the same row",
                play_ready.y,
                folder_ready.y
            );

            // Reset to uninstalled for next width iteration.
            let _ = std::fs::remove_file(inst_dir.join("install_state.json"));
            state.invalidate_install_cache(Some(&inst_id));
        }
    }

    #[test]
    fn quick_play_enter_button_blocks_while_launching_or_running_and_sets_is_launching() {
        let (_tmp, mut state) = test_ui_state();
        let inst = state
            .instance_mgr
            .create_instance("QP", "1.21.4", ModLoaderKind::Vanilla, None)
            .expect("instance");
        let inst_id = inst.id.clone();
        state.instances.push(inst);

        let ctx = egui::Context::default();
        let make_input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(600.0, 200.0),
            )),
            events,
            ..Default::default()
        };

        // 1. Enabled initially: clicking sets is_launching = true and sends LaunchInstanceQuickPlay.
        let out = ctx.run(make_input(vec![]), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                InstancesView::quick_play_enter_button(
                    ui,
                    &mut state,
                    &inst_id,
                    mq_launch::QuickPlayTarget::Singleplayer("world".into()),
                );
            });
        });
        let pos = find_text_center(&out.shapes, "Войти");
        for pressed in [true, false] {
            let _ = ctx.run(
                make_input(vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]),
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        InstancesView::quick_play_enter_button(
                            ui,
                            &mut state,
                            &inst_id,
                            mq_launch::QuickPlayTarget::Singleplayer("world".into()),
                        );
                    });
                },
            );
        }
        assert!(state.is_launching);
        assert!(matches!(
            state.cmd_receiver.try_recv().unwrap(),
            UiWorkerCommand::LaunchInstanceQuickPlay { .. }
        ));

        // 2. While is_launching = true, clicking does not send another command.
        let out_launching = ctx.run(make_input(vec![]), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                InstancesView::quick_play_enter_button(
                    ui,
                    &mut state,
                    &inst_id,
                    mq_launch::QuickPlayTarget::Multiplayer("play.example.org".into()),
                );
            });
        });
        let pos_launching = find_text_center(&out_launching.shapes, "Запуск...");
        for pressed in [true, false] {
            let _ = ctx.run(
                make_input(vec![
                    egui::Event::PointerMoved(pos_launching),
                    egui::Event::PointerButton {
                        pos: pos_launching,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]),
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        InstancesView::quick_play_enter_button(
                            ui,
                            &mut state,
                            &inst_id,
                            mq_launch::QuickPlayTarget::Multiplayer("play.example.org".into()),
                        );
                    });
                },
            );
        }
        assert!(state.cmd_receiver.try_recv().is_err());

        // 3. When instance is already running, clicking does not launch.
        state.is_launching = false;
        state.running_instances.insert(inst_id.clone(), Some(1234));
        let out_running = ctx.run(make_input(vec![]), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                InstancesView::quick_play_enter_button(
                    ui,
                    &mut state,
                    &inst_id,
                    mq_launch::QuickPlayTarget::Singleplayer("world".into()),
                );
            });
        });
        let pos_running = find_text_center(&out_running.shapes, "Войти");
        for pressed in [true, false] {
            let _ = ctx.run(
                make_input(vec![
                    egui::Event::PointerMoved(pos_running),
                    egui::Event::PointerButton {
                        pos: pos_running,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]),
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        InstancesView::quick_play_enter_button(
                            ui,
                            &mut state,
                            &inst_id,
                            mq_launch::QuickPlayTarget::Singleplayer("world".into()),
                        );
                    });
                },
            );
        }
        assert!(!state.is_launching);
        assert!(state.cmd_receiver.try_recv().is_err());
    }

    #[test]
    fn instances_header_and_version_search_meet_layout_constraints() {
        let (_tmp, mut state) = test_ui_state();
        let inst = state
            .instance_mgr
            .create_instance("Test", "1.21.4", ModLoaderKind::Fabric, None)
            .expect("instance");
        state.instances.push(inst);
        state.show_create_instance_dialog = true;

        let mut view = InstancesView::default();
        let ctx = egui::Context::default();
        // Central panel widths corresponding to 720px and 960px window widths.
        for central_w in [500.0_f32, 720.0_f32] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(central_w, 720.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    view.ui(ui, &mut state);
                });
            });
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    view.ui(ui, &mut state);
                });
            });
            let title_pos = find_text_center(&out.shapes, "Инстансы");
            let create_pos = find_text_center(&out.shapes, "Создать инстанс");
            assert!(
                title_pos.x < create_pos.x || title_pos.y < create_pos.y,
                "at width {central_w}, Инстансы ({title_pos:?}) must not overlap Создать инстанс ({create_pos:?})"
            );
        }

        // Verify the version search TextEdit respects >= 28.0px minimum height.
        let mut search_height = 0.0_f32;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(480.0, 200.0),
                )),
                ..Default::default()
            },
            |ctx| {
                let tokens = Tokens::active(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    let resp = version_search_input(ui, &mut view.version_search, 240.0, &tokens);
                    search_height = resp.rect.height();
                });
            },
        );
        assert!(
            search_height >= 28.0,
            "expected Поиск версии height >= 28.0, got {search_height}"
        );
    }
}
