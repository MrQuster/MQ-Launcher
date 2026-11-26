#![deny(clippy::await_holding_lock)]
//! Accounts manager screen: Offline (UUID v3), Ely.by (Browser-only OAuth2 + authlib-injector),
//! and Microsoft Device Code Flow with 1-click switching. Zero emojis!

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use egui::{Color32, RichText, ScrollArea, Stroke, Ui, Vec2};
use mq_auth::{
    create_offline_account, delete_local_skin, import_local_skin, list_local_skins, AccountKind,
    AccountProfile, ElyByOAuthConfig, ElyByOAuthFlow, SkinLibraryEntry,
};
use mq_core::{AccountId, SecretString};
use uuid::Uuid;

use crate::components::registry::{register_widget, ElementKind, ElementMeta};
use crate::state::UiState;
use crate::theme::{
    ACCENT_COBALT, ACCENT_EMERALD, CARD_SURFACE, STATUS_WARN, TEXT_MUTED, TEXT_PRIMARY,
};
use crate::widgets::{
    paint_skin_head_avatar, primary_emerald_button, secondary_glass_button, version_badge,
    VectorIcon,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountTab {
    Offline,
    ElyBy,
    Microsoft,
}

impl AccountTab {
    const fn order_index(self) -> i32 {
        match self {
            Self::Offline => 0,
            Self::ElyBy => 1,
            Self::Microsoft => 2,
        }
    }
}

pub struct AccountsView {
    pub current_tab: AccountTab,
    // Offline form
    pub offline_username: String,
    // Ely.by browser OAuth state
    pub ely_auth_url: Option<String>,
    pub ely_waiting_browser: bool,
    pub ely_cancel_flag: Arc<AtomicBool>,
    // MSA form
    pub msa_device_code_prompt: Option<String>,
    pub msa_waiting_login: bool,
    pub msa_verification_uri: Option<String>,
    pub msa_user_code: Option<String>,
    pub msa_cancel_flag: Arc<AtomicBool>,
    // Skin Library state
    pub cached_skins: Option<Vec<SkinLibraryEntry>>,
    pub selected_skin_id: Option<String>,
    pub import_skin_path: String,
    pub import_skin_name: String,
    pub import_skin_slim: bool,
    pub skin_library_error: Option<String>,
}

impl Default for AccountsView {
    fn default() -> Self {
        Self {
            current_tab: AccountTab::Offline,
            offline_username: "Mr_Quster".to_string(),
            ely_auth_url: None,
            ely_waiting_browser: false,
            ely_cancel_flag: Arc::new(AtomicBool::new(false)),
            msa_device_code_prompt: None,
            msa_waiting_login: false,
            msa_verification_uri: None,
            msa_user_code: None,
            msa_cancel_flag: Arc::new(AtomicBool::new(false)),
            cached_skins: None,
            selected_skin_id: None,
            import_skin_path: String::new(),
            import_skin_name: String::new(),
            import_skin_slim: false,
            skin_library_error: None,
        }
    }
}

#[inline]
const fn tr_acc(is_en: bool, ru: &'static str, en: &'static str) -> &'static str {
    if is_en {
        en
    } else {
        ru
    }
}

impl AccountsView {
    pub fn ui(&mut self, ui: &mut Ui, state: &mut UiState) {
        let is_en = state.config.launcher_locale == "en";

        ui.heading(
            RichText::new(tr_acc(is_en, "Управление аккаунтами", "Account Management"))
                .color(TEXT_PRIMARY)
                .strong(),
        );
        ui.add_space(8.0);

        let tokens = crate::theme::Tokens::from_config(&state.config.theme);
        let style = crate::theme_styles::ThemeStyleParams::active(ui.ctx());
        let indicator_slot = ui.painter().add(egui::Shape::Noop);
        let mut active_tab_rect: Option<egui::Rect> = None;

        // Tab buttons conforming to specification:
        // 1. Офлайн (только ник)
        // 2. Ely.by (через браузер)
        // 3. Microsoft (через браузер)
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            let tabs = [
                (
                    AccountTab::Offline,
                    tr_acc(is_en, "Офлайн (только ник)", "Offline (username only)"),
                ),
                (
                    AccountTab::ElyBy,
                    tr_acc(is_en, "Ely.by (через браузер)", "Ely.by (browser)"),
                ),
                (
                    AccountTab::Microsoft,
                    tr_acc(is_en, "Microsoft (через браузер)", "Microsoft (browser)"),
                ),
            ];
            for (tab, label) in tabs {
                let is_sel = self.current_tab == tab;
                let font_id = egui::FontId::proportional(13.0);
                let text_w = ui
                    .painter()
                    .layout_no_wrap(label.to_owned(), font_id.clone(), tokens.text)
                    .size()
                    .x;
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(text_w + 22.0, 30.0), egui::Sense::click());
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let anim = crate::animation::animate_interaction(
                    ui.ctx(),
                    resp.id.with("acc_tab_anim"),
                    resp.hovered(),
                    resp.is_pointer_button_down_on(),
                );
                let active_t = crate::animation::animate_spring_bool(
                    ui.ctx(),
                    resp.id.with("acc_tab_sel"),
                    is_sel,
                    crate::animation::SpringConfig::snappy(),
                );
                let visual_rect = if is_sel {
                    rect
                } else {
                    anim.transform_rect(rect)
                };
                if !is_sel && anim.hover_t > 0.02 {
                    let tab_r = style
                        .control_corner_radius(visual_rect.height(), crate::theme::RADIUS_BUTTON);
                    ui.painter().rect_filled(
                        visual_rect,
                        tab_r,
                        tokens.bg_surface_hover.gamma_multiply(0.55 * anim.hover_t),
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
                if is_sel {
                    active_tab_rect = Some(rect);
                }
                register_widget(
                    ui,
                    &resp,
                    ElementMeta::new(ElementKind::Tab)
                        .label(label)
                        .text(label)
                        .active(is_sel)
                        .fg(fg)
                        .bg(CARD_SURFACE),
                );
                if resp.clicked() {
                    self.current_tab = tab;
                }
            }
        });

        if let Some(target_rect) = active_tab_rect {
            let anim_rect = crate::animation::animate_spring_rect(
                ui.ctx(),
                egui::Id::new("mq_accounts_tab_pill"),
                target_rect,
                crate::animation::SpringConfig::rect(),
            );
            let pill_r =
                style.control_corner_radius(anim_rect.height(), crate::theme::RADIUS_BUTTON);
            if style.refraction_border {
                let mut shapes = Vec::new();
                crate::theme_styles::collect_tinted_glass_pill_shapes(
                    ui.ctx(),
                    anim_rect,
                    pill_r,
                    &style,
                    &tokens,
                    crate::theme::blend_color(tokens.bg_surface_hover, tokens.accent, 0.24),
                    0.75,
                    &mut shapes,
                );
                if !shapes.is_empty() {
                    ui.painter().set(indicator_slot, egui::Shape::Vec(shapes));
                }
            } else {
                let pill_fill = crate::theme::blend_color(tokens.bg_surface, tokens.accent, 0.16);
                ui.painter().set(
                    indicator_slot,
                    egui::Shape::Rect(egui::epaint::RectShape::new(
                        anim_rect,
                        pill_r,
                        pill_fill,
                        Stroke::new(1.0_f32, tokens.accent),
                        egui::StrokeKind::Inside,
                    )),
                );
            }
        }

        ui.add_space(8.0);

        // Tab form card
        let card_r = style.card_corner_radius(state.config.theme.corner_radius);
        let form_card = crate::theme::solid_card_frame_with_config(&tokens, &state.config.theme)
            .inner_margin(egui::Margin::same(12));

        crate::theme_styles::show_themed_card(
            ui,
            form_card,
            card_r,
            ui.make_persistent_id("acc_form_card_glass"),
            &style,
            &tokens,
            |ui| {
                ui.set_width(ui.available_width());
                crate::animation::render_tab_transition(
                    ui,
                    "mq_accounts_tab_transition",
                    self.current_tab,
                    self.current_tab.order_index(),
                    true,
                    |ui| match self.current_tab {
                        AccountTab::Offline => self.render_offline_tab(ui, state),
                        AccountTab::ElyBy => self.render_elyby_tab(ui, state),
                        AccountTab::Microsoft => self.render_msa_tab(ui, state),
                    },
                );
            },
        );

        ui.add_space(12.0);
        self.render_skin_library_section(ui, state);

        ui.add_space(12.0);
        ui.heading(
            RichText::new(tr_acc(is_en, "Сохранённые профили", "Saved Profiles"))
                .color(TEXT_PRIMARY)
                .strong(),
        );
        ui.add_space(6.0);

        // List of stored accounts
        ScrollArea::vertical()
            .id_salt("stored_accounts_scroll")
            .show(ui, |ui| {
                let accounts = state.accounts.list_accounts().to_vec();
                if accounts.is_empty() {
                    ui.label(
                        RichText::new(tr_acc(
                            is_en,
                            "Нет сохранённых аккаунтов. Добавьте первый выше!",
                            "No saved accounts yet. Add one above!",
                        ))
                        .color(TEXT_MUTED),
                    );
                } else {
                    for acc in &accounts {
                        let is_active =
                            state.accounts.active_account().map(|a| a.id) == Some(acc.id);

                        let card = egui::Frame::new()
                            .fill(if is_active {
                                style.surface_hover_fill(&tokens)
                            } else {
                                style.surface_fill(&tokens)
                            })
                            .stroke(if style.refraction_border {
                                Stroke::NONE
                            } else {
                                Stroke::new(
                                    1.0_f32,
                                    if is_active {
                                        tokens.accent
                                    } else {
                                        tokens.border
                                    },
                                )
                            })
                            .corner_radius(card_r)
                            .inner_margin(egui::Margin::same(10));

                        let acc_resp = crate::theme_styles::show_themed_card(
                            ui,
                            card,
                            card_r,
                            ui.make_persistent_id(("acc_card_glass", acc.id.to_string())),
                            &style,
                            &tokens,
                            |ui| {
                                ui.horizontal(|ui| {
                                    paint_skin_head_avatar(
                                        ui,
                                        36.0,
                                        &acc.username,
                                        acc.avatar_head.as_ref(),
                                    );

                                    let right_w = if !is_active { 150.0 } else { 0.0 };
                                    let middle_w =
                                        (ui.available_width() - right_w - 12.0).max(60.0);

                                    ui.allocate_ui_with_layout(
                                        Vec2::new(middle_w, 40.0),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
                                                let name_w =
                                                    (ui.available_width() - 110.0).max(50.0);
                                                ui.add_sized(
                                                    [name_w.min(160.0), 18.0],
                                                    egui::Label::new(
                                                        RichText::new(&acc.username)
                                                            .color(TEXT_PRIMARY)
                                                            .strong(),
                                                    )
                                                    .truncate(),
                                                );
                                                let badge_col = match acc.kind {
                                                    AccountKind::Offline => ACCENT_COBALT,
                                                    AccountKind::ElyBy => ACCENT_EMERALD,
                                                    AccountKind::Microsoft => {
                                                        Color32::from_rgb(0xF5, 0x9E, 0x0B)
                                                    }
                                                };
                                                version_badge(
                                                    ui,
                                                    acc.kind.badge_label(),
                                                    badge_col,
                                                );
                                                if is_active {
                                                    version_badge(
                                                        ui,
                                                        tr_acc(is_en, "АКТИВЕН", "ACTIVE"),
                                                        ACCENT_EMERALD,
                                                    );
                                                }
                                            });
                                            let uuid_w = (ui.available_width() - 8.0).max(40.0);
                                            ui.add_sized(
                                                [uuid_w, 16.0],
                                                egui::Label::new(
                                                    RichText::new(format!(
                                                        "UUID: {}",
                                                        acc.uuid_hyphenated()
                                                    ))
                                                    .color(TEXT_MUTED)
                                                    .small(),
                                                )
                                                .truncate(),
                                            );
                                        },
                                    );

                                    if !is_active {
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if secondary_glass_button(
                                                    ui,
                                                    Some(VectorIcon::Trash),
                                                    "",
                                                )
                                                .clicked()
                                                {
                                                    let _ = state
                                                        .accounts
                                                        .remove_account(&acc.id.to_string());
                                                    let _ = state.accounts.save(&state.paths);
                                                }
                                                if primary_emerald_button(
                                                    ui,
                                                    Some(VectorIcon::Check),
                                                    tr_acc(is_en, "Выбрать", "Select"),
                                                )
                                                .clicked()
                                                {
                                                    let _ = state
                                                        .accounts
                                                        .select_account(&acc.id.to_string());
                                                    let _ = state.accounts.save(&state.paths);
                                                }
                                            },
                                        );
                                    }
                                });
                            },
                        );
                        if style.refraction_border && is_active {
                            crate::theme_styles::paint_liquid_glass_selection_halo(
                                ui.painter(),
                                acc_resp.response.rect,
                                card_r,
                                tokens.accent,
                            );
                        }
                        ui.add_space(4.0);
                    }
                }
            });
    }

    fn render_skin_library_section(&mut self, ui: &mut Ui, state: &mut UiState) {
        let is_en = state.config.launcher_locale == "en";
        let tokens = crate::theme::Tokens::from_config(&state.config.theme);
        let style = crate::theme_styles::ThemeStyleParams::active(ui.ctx());
        let card_r = style.card_corner_radius(state.config.theme.corner_radius);
        let skins_dir = state.paths.data_dir.join("skins");

        // Cache local skins list in view (avoid reading disk every frame)
        if self.cached_skins.is_none() {
            let _ = std::fs::create_dir_all(&skins_dir);
            self.cached_skins = Some(list_local_skins(&skins_dir));
        }

        let form_card = crate::theme::solid_card_frame_with_config(&tokens, &state.config.theme)
            .inner_margin(egui::Margin::same(12));

        crate::theme_styles::show_themed_card(
            ui,
            form_card,
            card_r,
            ui.make_persistent_id("skin_library_card_glass"),
            &style,
            &tokens,
            |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(tr_acc(is_en, "Библиотека скинов", "Skin Library"))
                        .color(TEXT_PRIMARY)
                        .strong()
                        .size(15.0),
                );
                ui.add_space(8.0);

                self.render_skin_list_and_import(ui, state, &skins_dir, &tokens, is_en);
            },
        );
    }

    fn render_skin_list_and_import(
        &mut self,
        ui: &mut Ui,
        _state: &mut UiState,
        skins_dir: &Path,
        _tokens: &crate::theme::Tokens,
        is_en: bool,
    ) {
        ui.label(
            RichText::new(tr_acc(is_en, "Локальные скины:", "Local Skins:"))
                .color(TEXT_PRIMARY)
                .strong(),
        );
        ui.add_space(4.0);

        let skins = self.cached_skins.clone().unwrap_or_default();
        ScrollArea::vertical()
            .id_salt("skins_library_scroll")
            .max_height(130.0)
            .show(ui, |ui| {
                if skins.is_empty() {
                    ui.label(
                        RichText::new(tr_acc(
                            is_en,
                            "Нет сохранённых скинов. Импортируйте PNG файл ниже.",
                            "No saved skins. Import a PNG file below.",
                        ))
                        .color(TEXT_MUTED)
                        .small(),
                    );
                } else {
                    let mut delete_id: Option<String> = None;
                    for skin in &skins {
                        let is_sel = self.selected_skin_id.as_deref() == Some(&skin.id);
                        ui.horizontal(|ui| {
                            let label_text = format!(
                                "{} ({})",
                                skin.name,
                                if skin.slim { "Slim 3px" } else { "Classic 4px" }
                            );
                            if ui.selectable_label(is_sel, label_text).clicked() {
                                self.selected_skin_id = Some(skin.id.clone());
                            }
                            if secondary_glass_button(ui, Some(VectorIcon::Trash), "").clicked() {
                                delete_id = Some(skin.id.clone());
                            }
                        });
                    }
                    if let Some(del_id) = delete_id {
                        let _ = delete_local_skin(skins_dir, &del_id);
                        if self.selected_skin_id.as_deref() == Some(&del_id) {
                            self.selected_skin_id = None;
                        }
                        self.cached_skins = None;
                    }
                }
            });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(
            RichText::new(tr_acc(is_en, "Импорт скина (.png):", "Import Skin (.png):"))
                .color(TEXT_PRIMARY)
                .strong(),
        );
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label(tr_acc(is_en, "Файл:", "File:"));
            ui.add(
                egui::TextEdit::singleline(&mut self.import_skin_path)
                    .hint_text("/path/to/skin.png")
                    .desired_width(180.0),
            );
        });

        ui.horizontal(|ui| {
            ui.label(tr_acc(is_en, "Имя:", "Name:"));
            ui.add(
                egui::TextEdit::singleline(&mut self.import_skin_name)
                    .hint_text(tr_acc(is_en, "Название", "Name"))
                    .desired_width(120.0),
            );
            ui.checkbox(&mut self.import_skin_slim, "Slim (3px)");
        });

        if let Some(ref err) = self.skin_library_error {
            ui.label(RichText::new(err).color(STATUS_WARN).small());
        }

        ui.add_space(4.0);
        if primary_emerald_button(
            ui,
            Some(VectorIcon::Download),
            tr_acc(is_en, "Добавить в библиотеку", "Add to Library"),
        )
        .clicked()
        {
            let trimmed_path = self.import_skin_path.trim();
            let p = Path::new(trimmed_path);
            if trimmed_path.is_empty() || !p.exists() || !p.is_file() {
                self.skin_library_error = Some(
                    tr_acc(
                        is_en,
                        "Указанный файл .png не найден",
                        "Specified .png file not found",
                    )
                    .to_string(),
                );
            } else {
                let name = if self.import_skin_name.trim().is_empty() {
                    p.file_stem().and_then(|s| s.to_str()).unwrap_or("skin")
                } else {
                    self.import_skin_name.trim()
                };
                match import_local_skin(skins_dir, name, p, self.import_skin_slim) {
                    Ok(entry) => {
                        self.selected_skin_id = Some(entry.id);
                        self.cached_skins = None;
                        self.import_skin_path.clear();
                        self.import_skin_name.clear();
                        self.skin_library_error = None;
                    }
                    Err(e) => {
                        self.skin_library_error = Some(format!(
                            "{}: {e}",
                            tr_acc(is_en, "Ошибка импорта", "Import error")
                        ));
                    }
                }
            }
        }
    }

    fn render_offline_tab(&mut self, ui: &mut Ui, state: &mut UiState) {
        let is_en = state.config.launcher_locale == "en";
        ui.label(
            RichText::new(tr_acc(
                is_en,
                "Вход без пароля для одиночной игры и оффлайн-серверов",
                "Password-free login for singleplayer and offline servers",
            ))
            .color(TEXT_MUTED),
        );
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label(tr_acc(is_en, "Никнейм игрока:", "Player username:"));
            let tokens = crate::theme::Tokens::active(ui.ctx());
            let frame_resp = egui::Frame::new()
                .fill(tokens.bg_input)
                .stroke(Stroke::new(1.0_f32, tokens.border))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.set_min_size(Vec2::new(204.0, 18.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.offline_username)
                            .frame(false)
                            .desired_width(204.0)
                            .vertical_align(egui::Align::Center),
                    );
                });
            crate::components::registry::register_widget(
                ui,
                &frame_resp.response,
                crate::components::registry::ElementMeta::new(
                    crate::components::registry::ElementKind::Input,
                )
                .id("offline_nick_input")
                .label(tr_acc(is_en, "Никнейм игрока", "Player username"))
                .text(self.offline_username.as_str())
                .fg(tokens.text)
                .bg(tokens.bg_input),
            );
        });

        // Instant preview of computed Offline UUID v3
        let clean_nick = self.offline_username.trim();
        if !clean_nick.is_empty() {
            let computed_uuid = mq_auth::compute_offline_uuid(clean_nick);
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "{}: {}",
                    tr_acc(
                        is_en,
                        "Вычисленный Offline UUID v3",
                        "Computed Offline UUID v3"
                    ),
                    computed_uuid.hyphenated()
                ))
                .color(ACCENT_COBALT)
                .monospace()
                .small(),
            );
        }

        ui.add_space(8.0);
        if primary_emerald_button(
            ui,
            Some(VectorIcon::Plus),
            tr_acc(is_en, "Добавить оффлайн-профиль", "Add offline profile"),
        )
        .clicked()
        {
            match create_offline_account(clean_nick) {
                Ok(profile) => {
                    state.accounts.upsert_account(profile, true);
                    let _ = state.accounts.save(&state.paths);
                    state.status_message = Some((
                        if is_en {
                            format!("Offline profile '{clean_nick}' added and selected")
                        } else {
                            format!("Оффлайн-профиль '{clean_nick}' добавлен и выбран")
                        },
                        false,
                    ));
                }
                Err(e) => {
                    state.status_message = Some((
                        if is_en {
                            format!("Failed to create profile: {e}")
                        } else {
                            format!("Ошибка создания профиля: {e}")
                        },
                        true,
                    ));
                }
            }
        }
    }

    fn render_elyby_tab(&mut self, ui: &mut Ui, state: &mut UiState) {
        if state.ely_login_done.take().is_some() {
            self.ely_waiting_browser = false;
            self.ely_auth_url = None;
        }
        let is_en = state.config.launcher_locale == "en";
        ui.label(
            RichText::new(tr_acc(
                is_en,
                "Безопасный вход через официальный портал Ely.by в браузере (OAuth2)",
                "Secure login via the official Ely.by portal in your browser (OAuth2)",
            ))
            .color(TEXT_MUTED),
        );
        ui.add_space(6.0);

        let oauth_cfg = ElyByOAuthConfig::default();

        if self.ely_waiting_browser {
            ui.label(
                RichText::new(tr_acc(
                    is_en,
                    "Ожидание подтверждения входа в браузере...",
                    "Waiting for browser login confirmation...",
                ))
                .color(ACCENT_COBALT)
                .strong(),
            );
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                if secondary_glass_button(
                    ui,
                    Some(VectorIcon::Globe),
                    tr_acc(is_en, "Открыть ссылку ещё раз", "Open link again"),
                )
                .clicked()
                {
                    if let Some(ref url) = self.ely_auth_url {
                        let _ = crate::opener::open_that(url);
                    }
                }
                if secondary_glass_button(
                    ui,
                    Some(VectorIcon::Copy),
                    tr_acc(is_en, "Скопировать ссылку", "Copy link"),
                )
                .clicked()
                {
                    if let Some(ref url) = self.ely_auth_url {
                        ui.ctx().copy_text(url.clone());
                        state.status_message = Some((
                            tr_acc(
                                is_en,
                                "Ссылка для авторизации скопирована в буфер обмена",
                                "Authorization link copied to clipboard",
                            )
                            .to_string(),
                            false,
                        ));
                    }
                }
                if secondary_glass_button(
                    ui,
                    Some(VectorIcon::Cross),
                    tr_acc(is_en, "Отмена", "Cancel"),
                )
                .clicked()
                {
                    self.ely_waiting_browser = false;
                    self.ely_cancel_flag.store(true, Ordering::SeqCst);
                    state.status_message = Some((
                        tr_acc(is_en, "Вход через Ely.by отменён", "Ely.by login cancelled")
                            .to_string(),
                        false,
                    ));
                }
            });
            return;
        }

        let client_id = oauth_cfg.client_id.as_deref().unwrap_or("mq-launcher");

        ui.horizontal_wrapped(|ui| {
            if primary_emerald_button(
                ui,
                Some(VectorIcon::Globe),
                tr_acc(
                    is_en,
                    "Войти через Ely.by (в браузере)",
                    "Sign in with Ely.by (browser)",
                ),
            )
            .clicked()
            {
                self.ely_waiting_browser = true;
                let state_str = Uuid::new_v4().to_string();
                let port = oauth_cfg.redirect_port;
                let pkce = mq_auth::elyby::PkcePair::generate();
                let auth_url = ElyByOAuthFlow::generate_auth_url_pkce(
                    client_id,
                    port,
                    &state_str,
                    &pkce.challenge,
                );
                self.ely_auth_url = Some(auth_url.clone());
                let _ = crate::opener::open_that(&auth_url);

                let cancel_flag = Arc::new(AtomicBool::new(false));
                self.ely_cancel_flag = cancel_flag.clone();
                let client_id_owned = client_id.to_string();
                let client_secret_owned = oauth_cfg.client_secret.clone();
                let verifier_owned = pkce.verifier;
                let paths_clone = state.paths.clone();
                let resp_tx = state.resp_sender.clone();

                // Spawn background task to receive loopback callback and exchange token
                let handle = tokio::runtime::Handle::current();
                std::thread::spawn(move || {
                    handle.block_on(async move {
                        let res = ElyByOAuthFlow::start_loopback_listener(port, &state_str).await;
                        if cancel_flag.load(Ordering::SeqCst) {
                            return;
                        }
                        let outcome: Result<String, String> = match res {
                            Ok(code) => {
                                let http = reqwest::Client::builder()
                                    .timeout(std::time::Duration::from_secs(20))
                                    .build()
                                    .unwrap_or_default();
                                let redirect_uri = format!("http://127.0.0.1:{port}/callback");
                                match ElyByOAuthFlow::exchange_code_pkce(
                                    &http,
                                    &client_id_owned,
                                    &code,
                                    Some(&verifier_owned),
                                    &redirect_uri,
                                    client_secret_owned.as_deref(),
                                )
                                .await
                                {
                                    Ok(token_resp) => {
                                        match ElyByOAuthFlow::fetch_account_info(
                                            &http,
                                            &token_resp.access_token,
                                        )
                                        .await
                                        {
                                            Ok(info) => {
                                                let username = info.username.clone();
                                                let profile = AccountProfile {
                                                    id: AccountId::new_v4(),
                                                    username: info.username,
                                                    uuid: Uuid::parse_str(&info.uuid)
                                                        .unwrap_or_else(|_| Uuid::new_v4()),
                                                    kind: AccountKind::ElyBy,
                                                    access_token: SecretString::new(
                                                        token_resp.access_token,
                                                    ),
                                                    refresh_token: token_resp
                                                        .refresh_token
                                                        .map(SecretString::new),
                                                    client_token: None,
                                                    expires_at: token_resp.expires_in.map(|secs| {
                                                        chrono::Utc::now()
                                                            + chrono::Duration::seconds(secs as i64)
                                                    }),
                                                    skin_url: None,
                                                    avatar_head: None,
                                                };
                                                match mq_auth::AccountStore::load_or_default(
                                                    &paths_clone,
                                                ) {
                                                    Ok(mut store) => {
                                                        store.upsert_account(profile, true);
                                                        match store.save(&paths_clone) {
                                                            Ok(()) => Ok(username),
                                                            Err(e) => Err(format!(
                                                                "Не удалось сохранить аккаунт: {e}"
                                                            )),
                                                        }
                                                    }
                                                    Err(e) => Err(format!(
                                                        "Не удалось загрузить хранилище аккаунтов: {e}"
                                                    )),
                                                }
                                            }
                                            Err(e) => {
                                                tracing::error!(
                                                    "Failed to fetch Ely.by account info: {e}"
                                                );
                                                Err(format!(
                                                    "Не удалось получить профиль Ely.by: {e}"
                                                ))
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "Failed to exchange Ely.by code for token: {e}"
                                        );
                                        Err(format!("Не удалось обменять код на токен: {e}"))
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("Ely.by loopback callback failed: {e}");
                                Err(format!("Ошибка ожидания ответа браузера: {e}"))
                            }
                        };
                        if !cancel_flag.load(Ordering::SeqCst) {
                            let _ = resp_tx.send(
                                crate::state::UiWorkerResponse::ElyByLoginFinished(outcome),
                            );
                        }
                    });
                });
            }

            if secondary_glass_button(
                ui,
                Some(VectorIcon::Globe),
                tr_acc(is_en, "Создать аккаунт на Ely.by", "Create Ely.by account"),
            )
            .clicked()
            {
                let _ = crate::opener::open_that("https://ely.by/registration");
            }
        });
    }

    fn render_msa_tab(&mut self, ui: &mut Ui, state: &mut UiState) {
        let is_en = state.config.launcher_locale == "en";
        ui.label(
            RichText::new(tr_acc(
                is_en,
                "Официальная лицензия Microsoft (Device Code Flow)",
                "Official Microsoft Account (Device Code Flow)",
            ))
            .color(TEXT_MUTED),
        );
        ui.add_space(6.0);

        if let Some((uri, code)) = state.msa_device_code.take() {
            self.msa_verification_uri = Some(uri);
            self.msa_user_code = Some(code);
        }
        if state.msa_login_done.is_some() {
            self.msa_waiting_login = false;
        }

        let has_client_id = state.config.effective_ms_client_id().is_some();
        if !has_client_id {
            ui.label(
                RichText::new(tr_acc(
                    is_en,
                    "Вход через лицензию Microsoft временно недоступен — скоро заработает после подтверждения приложения в Microsoft.",
                    "Microsoft account sign-in will be enabled once Microsoft app approval is complete.",
                ))
                .color(STATUS_WARN)
                .small(),
            );
            ui.add_space(6.0);
        }

        if self.msa_waiting_login {
            ui.label(
                RichText::new(tr_acc(
                    is_en,
                    "Ожидание авторизации Microsoft...",
                    "Waiting for Microsoft authorization...",
                ))
                .color(ACCENT_COBALT)
                .strong(),
            );
            ui.add_space(4.0);

            if let Some(user_code) = self.msa_user_code.clone() {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{}: {}", tr_acc(is_en, "Код", "Code"), user_code))
                            .color(ACCENT_EMERALD)
                            .heading()
                            .strong(),
                    );
                });
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    if let Some(uri) = &self.msa_verification_uri {
                        if primary_emerald_button(
                            ui,
                            Some(VectorIcon::Globe),
                            tr_acc(is_en, "Открыть в браузере", "Open in browser"),
                        )
                        .clicked()
                        {
                            let _ = crate::opener::open_that(uri);
                        }
                    }
                    if secondary_glass_button(
                        ui,
                        Some(VectorIcon::Refresh),
                        tr_acc(is_en, "Скопировать код", "Copy code"),
                    )
                    .clicked()
                    {
                        ui.ctx().copy_text(user_code.clone());
                        state.status_message = Some((
                            tr_acc(
                                is_en,
                                "Код скопирован в буфер обмена",
                                "Code copied to clipboard",
                            )
                            .to_string(),
                            false,
                        ));
                    }
                    if secondary_glass_button(ui, None, tr_acc(is_en, "Отмена", "Cancel")).clicked()
                    {
                        self.msa_cancel_flag.store(true, Ordering::SeqCst);
                        self.msa_waiting_login = false;
                        self.msa_user_code = None;
                        self.msa_verification_uri = None;
                        state.status_message = Some((
                            tr_acc(
                                is_en,
                                "Вход Microsoft отменен",
                                "Microsoft sign-in cancelled",
                            )
                            .to_string(),
                            false,
                        ));
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new(tr_acc(
                            is_en,
                            "Получение кода устройства...",
                            "Requesting device code...",
                        ))
                        .color(TEXT_MUTED),
                    );
                    if secondary_glass_button(ui, None, tr_acc(is_en, "Отмена", "Cancel")).clicked()
                    {
                        self.msa_cancel_flag.store(true, Ordering::SeqCst);
                        self.msa_waiting_login = false;
                    }
                });
            }
        } else {
            let btn = ui.add_enabled_ui(has_client_id, |ui| {
                primary_emerald_button(
                    ui,
                    Some(VectorIcon::User),
                    tr_acc(is_en, "Войти через Microsoft", "Sign in with Microsoft"),
                )
            });
            if has_client_id && btn.inner.clicked() {
                if let Some(client_id) = state.config.effective_ms_client_id() {
                    self.msa_waiting_login = true;
                    self.msa_user_code = None;
                    self.msa_verification_uri = None;
                    let cancel_flag = Arc::new(AtomicBool::new(false));
                    self.msa_cancel_flag = cancel_flag.clone();
                    let client_id_owned = client_id;
                    let paths_clone = state.paths.clone();
                    let resp_tx = state.resp_sender.clone();

                    let handle = tokio::runtime::Handle::current();
                    std::thread::spawn(move || {
                        handle.block_on(async move {
                            let client = match mq_auth::MsaClient::new() {
                                Ok(c) => c,
                                Err(e) => {
                                    let _ = resp_tx.send(
                                        crate::state::UiWorkerResponse::MsaLoginFinished(Err(
                                            format!("Ошибка инициализации MSA: {e}"),
                                        )),
                                    );
                                    return;
                                }
                            };
                            let prompt = match client.start_device_code_flow(&client_id_owned).await
                            {
                                Ok(p) => p,
                                Err(e) => {
                                    let _ = resp_tx.send(
                                        crate::state::UiWorkerResponse::MsaLoginFinished(Err(
                                            format!("Ошибка Device Code: {e}"),
                                        )),
                                    );
                                    return;
                                }
                            };
                            if cancel_flag.load(Ordering::SeqCst) {
                                return;
                            }
                            let _ =
                                resp_tx.send(crate::state::UiWorkerResponse::MsaDeviceCodeReady {
                                    verification_uri: prompt.verification_uri.clone(),
                                    user_code: prompt.user_code.clone(),
                                });

                            let outcome: Result<String, String> = match client
                                .poll_and_complete_login(&client_id_owned, &prompt)
                                .await
                            {
                                Ok(acc) => {
                                    if cancel_flag.load(Ordering::SeqCst) {
                                        return;
                                    }
                                    let username = acc.username.clone();
                                    match mq_auth::AccountStore::load_or_default(&paths_clone) {
                                        Ok(mut store) => {
                                            store.upsert_account(acc, true);
                                            match store.save(&paths_clone) {
                                                Ok(()) => Ok(username),
                                                Err(e) => Err(format!(
                                                    "Не удалось сохранить аккаунт: {e}"
                                                )),
                                            }
                                        }
                                        Err(e) => Err(format!(
                                            "Не удалось загрузить хранилище аккаунтов: {e}"
                                        )),
                                    }
                                }
                                Err(e) => {
                                    if cancel_flag.load(Ordering::SeqCst) {
                                        return;
                                    }
                                    Err(format!("Ошибка авторизации Microsoft: {e}"))
                                }
                            };
                            if !cancel_flag.load(Ordering::SeqCst) {
                                let _ = resp_tx.send(
                                    crate::state::UiWorkerResponse::MsaLoginFinished(outcome),
                                );
                            }
                        });
                    });
                }
            }
        }
    }
}
