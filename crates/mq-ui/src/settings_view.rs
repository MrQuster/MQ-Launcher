#![deny(clippy::await_holding_lock)]
//! Global application settings & Customization Studio. Zero emojis!

use egui::{Color32, CornerRadius, Pos2, RichText, ScrollArea, Stroke, StrokeKind, Ui, Vec2};
use mq_core::{BackgroundStyle, GuiBackend, LaunchAction, ThemeConfig, ThemePreset, UiDensity};
use mq_meta::{find_mc_locale, search_mc_locales};

use crate::components::registry::{register_widget, ElementKind, ElementMeta};
use crate::i18n::{tr, Language};
use crate::state::UiState;
use crate::theme::{
    apply_theme, blend_color, rgb_to_c32, solid_card_frame_with_config, Tokens, ACCENT_COBALT,
    CALM_ACCENT_PRESETS, RADIUS_TAG,
};
use crate::theme_styles::{show_themed_card, ThemeStyleParams};
use crate::widgets::{
    accent_button, primary_emerald_button, secondary_glass_button, smooth_toggle, tag_pill,
    version_badge, VectorIcon,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsSection {
    Customization,
    Launch,
    Localization,
    NetworkAndApi,
    Catalog,
    Updates,
    About,
}

impl SettingsSection {
    const fn tab_index(self) -> i32 {
        match self {
            Self::Customization => 0,
            Self::Launch => 1,
            Self::Localization => 2,
            Self::NetworkAndApi => 3,
            Self::Catalog => 4,
            Self::Updates => 5,
            Self::About => 6,
        }
    }
}

#[derive(Default)]
pub struct SettingsView {
    pub active_section: Option<SettingsSection>,
    pub theme_export_path: String,
    pub catalog_key_input: String,
    pub theme_import_path: String,
    pub mc_locale_search: String,
    pub mc_locale_dropdown_open: bool,
    pub update_check_result: Option<String>,
    pub is_checking_update: bool,
    pub is_downloading_update: bool,
    pub update_progress: f32,
    pub available_update: Option<mq_update::ReleaseAssetInfo>,
    pub confirm_uninstall: bool,
    pub uninstall_purge_data: bool,
}

impl SettingsView {
    pub fn ui(&mut self, ui: &mut Ui, state: &mut UiState) {
        let tokens = Tokens::from_config(&state.config.theme);
        let style = ThemeStyleParams::active(ui.ctx());
        let current_section = self
            .active_section
            .unwrap_or(SettingsSection::Customization);
        let lang = state.config.launcher_locale.clone();

        let indicator_slot = ui.painter().add(egui::Shape::Noop);
        let mut active_tab_rect: Option<egui::Rect> = None;

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
            ui.heading(
                RichText::new(tr("tab_settings", &lang))
                    .color(tokens.text)
                    .strong(),
            );
            ui.add_space(8.0);

            // Sub-navigation tabs
            let sub_tabs = [
                (
                    SettingsSection::Customization,
                    tr("settings_customization", &lang),
                ),
                (SettingsSection::Launch, tr("settings_launch", &lang)),
                (
                    SettingsSection::Localization,
                    tr("settings_language", &lang),
                ),
                (SettingsSection::NetworkAndApi, "Сеть и API"),
                (SettingsSection::Catalog, "Каталог"),
                (SettingsSection::Updates, "Обновления"),
                (SettingsSection::About, "О программе"),
            ];
            for (sec, title) in sub_tabs {
                let is_sel = current_section == sec;
                let font_id = egui::FontId::proportional(13.0);
                let text_w = ui
                    .painter()
                    .layout_no_wrap(title.to_owned(), font_id.clone(), tokens.text)
                    .size()
                    .x;
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(text_w + 20.0, 28.0), egui::Sense::click());
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let anim = crate::animation::animate_interaction(
                    ui.ctx(),
                    resp.id.with("settings_tab_anim"),
                    resp.hovered(),
                    resp.is_pointer_button_down_on(),
                );
                let active_t = crate::animation::animate_spring_bool(
                    ui.ctx(),
                    resp.id.with("settings_tab_sel"),
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
                        tokens.bg_surface_hover.gamma_multiply(0.50 * anim.hover_t),
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
                    title,
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
                        .label(title)
                        .text(title)
                        .active(is_sel)
                        .fg(fg)
                        .bg(tokens.bg_app),
                );
                if resp.clicked() {
                    self.active_section = Some(sec);
                }
            }
        });

        if let Some(target_rect) = active_tab_rect {
            let anim_rect = crate::animation::animate_spring_rect(
                ui.ctx(),
                egui::Id::new("mq_settings_tab_pill"),
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
                        StrokeKind::Inside,
                    )),
                );
            }
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        ScrollArea::vertical()
            .id_salt("settings_scroll_area")
            .show(ui, |ui| {
                crate::animation::render_tab_transition(
                    ui,
                    "mq_settings_sec_transition",
                    current_section,
                    current_section.tab_index(),
                    true,
                    |ui| match current_section {
                        SettingsSection::Customization => {
                            self.render_customization_studio(ui, state);
                        }
                        SettingsSection::Launch => self.render_launch_settings(ui, state),
                        SettingsSection::Localization => {
                            self.render_localization_settings(ui, state);
                        }
                        SettingsSection::NetworkAndApi => {
                            self.render_network_api_settings(ui, state);
                        }
                        SettingsSection::Catalog => self.render_catalog_settings(ui, state),
                        SettingsSection::Updates => self.render_updates_settings(ui, state),
                        SettingsSection::About => self.render_about_settings(ui, state),
                    },
                );

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);

                if primary_emerald_button(ui, Some(VectorIcon::Check), tr("btn_save", &lang))
                    .clicked()
                {
                    let _ = state.config.save(&state.paths);
                    apply_theme(ui.ctx(), &state.config.theme);
                    state.status_message = Some((
                        "Настройки успешно сохранены в config.toml и theme.toml".to_string(),
                        false,
                    ));
                }
            });
    }

    fn render_customization_studio(&mut self, ui: &mut Ui, state: &mut UiState) {
        let tokens = Tokens::from_config(&state.config.theme);
        let style = ThemeStyleParams::from_config(&state.config.theme);
        let card_r = style.card_corner_radius(state.config.theme.corner_radius);
        let card = solid_card_frame_with_config(&tokens, &state.config.theme)
            .inner_margin(egui::Margin::same(16));

        fn choice_chip(ui: &mut Ui, tokens: &Tokens, selected: bool, label: &str) -> bool {
            let style = ThemeStyleParams::active(ui.ctx());
            let chip_r = style.control_corner_radius(26.0, RADIUS_TAG);
            let sel_fill = blend_color(
                tokens.bg_surface,
                tokens.accent,
                if tokens.is_light() { 0.16 } else { 0.22 },
            );
            let font_id = egui::FontId::proportional(12.5);
            let text_w = ui
                .painter()
                .layout_no_wrap(label.to_owned(), font_id.clone(), tokens.text)
                .size()
                .x;
            let desired = Vec2::new(text_w + 20.0, 26.0);
            let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let anim = crate::animation::animate_interaction(
                ui.ctx(),
                resp.id.with("choice_chip_anim"),
                resp.hovered(),
                resp.is_pointer_button_down_on(),
            );
            let sel_t = crate::animation::animate_spring_bool(
                ui.ctx(),
                resp.id.with("choice_chip_sel"),
                selected,
                crate::animation::SpringConfig::snappy(),
            );
            let visual_rect = anim.transform_rect(rect);
            let fill = crate::theme_styles::lerp_rgba(tokens.bg_input, sel_fill, sel_t);
            let stroke = crate::theme_styles::lerp_rgba(
                tokens.border,
                tokens.accent,
                sel_t.max(anim.press_t * 0.5),
            );
            let fg = crate::theme_styles::lerp_rgba(
                tokens.text_muted,
                tokens.text,
                sel_t.max(anim.hover_t),
            );
            if ui.is_rect_visible(rect) {
                let p = ui.painter();
                let galley = p.layout_no_wrap(label.to_owned(), font_id, fg);
                if style.refraction_border {
                    if selected || sel_t > 0.35 {
                        crate::theme_styles::paint_tinted_glass_pill(
                            ui.ctx(),
                            p,
                            visual_rect,
                            chip_r,
                            &style,
                            tokens,
                            fill,
                            (0.65 + 0.25 * anim.hover_t).min(1.0),
                        );
                    } else {
                        crate::theme_styles::paint_glass_surface_overlay(
                            ui.ctx(),
                            p,
                            visual_rect,
                            chip_r,
                            &style,
                            tokens,
                            0.20 + 0.52 * anim.hover_t,
                            true,
                        );
                    }
                } else {
                    let hov_fill = crate::theme_styles::lerp_rgba(
                        fill,
                        tokens.bg_surface_hover,
                        anim.hover_t * 0.5,
                    );
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

        fn accent_swatch_chip(
            ui: &mut Ui,
            tokens: &Tokens,
            name: &str,
            primary: [u8; 3],
            selected: bool,
        ) -> bool {
            let style = ThemeStyleParams::active(ui.ctx());
            let chip_r = style.control_corner_radius(28.0, RADIUS_TAG);
            let swatch = rgb_to_c32(primary);
            let font_id = egui::FontId::proportional(12.5);
            let text_w = ui
                .painter()
                .layout_no_wrap(name.to_owned(), font_id.clone(), tokens.text)
                .size()
                .x;
            let desired = Vec2::new(10.0 + 12.0 + 6.0 + text_w + 10.0, 28.0);
            let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let anim = crate::animation::animate_interaction(
                ui.ctx(),
                resp.id.with("swatch_chip_anim"),
                resp.hovered(),
                resp.is_pointer_button_down_on(),
            );
            let sel_t = crate::animation::animate_spring_bool(
                ui.ctx(),
                resp.id.with("swatch_chip_sel"),
                selected,
                crate::animation::SpringConfig::snappy(),
            );
            let visual_rect = anim.transform_rect(rect);
            let fg = crate::theme_styles::lerp_rgba(
                tokens.text_muted,
                tokens.text,
                sel_t.max(anim.hover_t),
            );
            if ui.is_rect_visible(rect) {
                let p = ui.painter();
                let galley = p.layout_no_wrap(name.to_owned(), font_id, fg);
                let base_fill = crate::theme_styles::lerp_rgba(
                    tokens.bg_input,
                    tokens.bg_surface_hover,
                    anim.hover_t,
                );
                let fill = crate::theme_styles::lerp_rgba(
                    base_fill,
                    blend_color(tokens.bg_surface, swatch, 0.18),
                    sel_t,
                );
                let base_stroke = crate::theme_styles::lerp_rgba(
                    tokens.border,
                    tokens.border_strong,
                    anim.hover_t,
                );
                let stroke_col = crate::theme_styles::lerp_rgba(base_stroke, swatch, sel_t);
                if style.refraction_border {
                    if selected || sel_t > 0.35 {
                        crate::theme_styles::paint_tinted_glass_pill(
                            ui.ctx(),
                            p,
                            visual_rect,
                            chip_r,
                            &style,
                            tokens,
                            fill,
                            (0.65 + 0.25 * anim.hover_t).min(1.0),
                        );
                    } else {
                        crate::theme_styles::paint_glass_surface_overlay(
                            ui.ctx(),
                            p,
                            visual_rect,
                            chip_r,
                            &style,
                            tokens,
                            0.20 + 0.52 * anim.hover_t,
                            true,
                        );
                    }
                } else {
                    p.rect_filled(visual_rect, chip_r, fill);
                    p.rect_stroke(
                        visual_rect,
                        chip_r,
                        Stroke::new(1.0_f32 + 0.5 * sel_t, stroke_col),
                        StrokeKind::Inside,
                    );
                }
                let dot_rect = egui::Rect::from_center_size(
                    Pos2::new(visual_rect.left() + 14.0, visual_rect.center().y),
                    Vec2::splat(12.0),
                );
                let dot_r = if style.refraction_border {
                    CornerRadius::same(6)
                } else {
                    CornerRadius::same(3)
                };
                p.rect_filled(dot_rect, dot_r, swatch);
                p.galley(
                    Pos2::new(
                        dot_rect.right() + 6.0,
                        visual_rect.center().y - galley.size().y * 0.5,
                    ),
                    galley,
                    fg,
                );
            }
            resp.clicked()
        }

        let mut style_dirty = false;
        let mut should_save = false;

        // Card 1: Theme Preset & Accent Palette
        show_themed_card(
            ui,
            card,
            card_r,
            ui.make_persistent_id("settings_custom_theme_card"),
            &style,
            &tokens,
            |ui| {
                ui.set_width(ui.available_width());

                ui.label(
                    RichText::new("1. Тема и акцентный цвет")
                        .color(tokens.text)
                        .strong()
                        .size(14.0),
                );
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Тема оформления:").color(tokens.text));
                    let combo_resp = egui::ComboBox::from_id_salt("settings_theme_preset_combo")
                        .selected_text(state.config.theme.preset.label())
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for preset in ThemePreset::ALL {
                                if ui
                                    .selectable_label(
                                        state.config.theme.preset == preset,
                                        preset.label(),
                                    )
                                    .clicked()
                                {
                                    state.config.theme.apply_preset(preset);
                                    style_dirty = true;
                                    should_save = true;
                                }
                            }
                        })
                        .response;
                    register_widget(
                        ui,
                        &combo_resp,
                        ElementMeta::new(ElementKind::Select)
                            .label("Тема оформления")
                            .text(state.config.theme.preset.label())
                            .fg(tokens.text)
                            .bg(tokens.bg_input),
                    );
                });
                ui.add_space(6.0);

                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                    for preset in ThemePreset::ALL {
                        let is_active = state.config.theme.preset == preset;
                        if choice_chip(ui, &tokens, is_active, preset.label()) {
                            state.config.theme.apply_preset(preset);
                            style_dirty = true;
                            should_save = true;
                        }
                    }
                });

                let preset_hint = match state.config.theme.preset {
                    ThemePreset::LiquidGlass => Some(
                        "Режим Liquid Glass: настоящее прозрачное стекло композитора без подложки обоев — рабочий стол и системное размытие видны сквозь матовые стеклянные карточки с оптической фаской и бликами. Своё фото-фон можно включить по желанию в разделе 2.",
                    ),
                    ThemePreset::Translucent => Some(
                        "Режим Полупрозрачная: чистая тонировка и прозрачность окна на уровне композитора без лишних эффектов.",
                    ),
                    _ => None,
                };
                if let Some(hint) = preset_hint {
                    ui.add_space(4.0);
                    ui.label(RichText::new(hint).color(tokens.text_muted).small());
                }

                ui.add_space(8.0);
                ui.label(
                    RichText::new("Спокойные акценты:")
                        .color(tokens.text_muted)
                        .small(),
                );
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                    for swatch in CALM_ACCENT_PRESETS {
                        let is_sel = state.config.theme.accent_primary == swatch.primary;
                        if accent_swatch_chip(ui, &tokens, swatch.name, swatch.primary, is_sel) {
                            state.config.theme.accent_primary = swatch.primary;
                            state.config.theme.accent_secondary = swatch.secondary;
                            state.config.theme.status_success = swatch.primary;
                            style_dirty = true;
                            should_save = true;
                        }
                    }
                });

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Свой цвет акцента:").color(tokens.text));
                    if ui
                        .color_edit_button_srgb(&mut state.config.theme.accent_primary)
                        .changed()
                    {
                        let pri = state.config.theme.accent_primary;
                        let sec = blend_color(
                            rgb_to_c32(pri),
                            Color32::BLACK,
                            if tokens.is_light() { 0.18 } else { 0.14 },
                        );
                        state.config.theme.accent_secondary = [sec.r(), sec.g(), sec.b()];
                        state.config.theme.status_success = pri;
                        style_dirty = true;
                        should_save = true;
                    }
                });
            },
        );

        ui.add_space(12.0);

        // Card 2: Background Style & Custom Backdrop
        show_themed_card(
            ui,
            card,
            card_r,
            ui.make_persistent_id("settings_custom_bg_card"),
            &style,
            &tokens,
            |ui| {
                ui.set_width(ui.available_width());

                ui.label(
                    RichText::new("2. Фон лаунчера")
                        .color(tokens.text)
                        .strong()
                        .size(14.0),
                );
                ui.add_space(6.0);

                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                    for mode in BackgroundStyle::ALL {
                        let is_sel = state.config.theme.background_style == mode;
                        if choice_chip(ui, &tokens, is_sel, mode.label()) {
                            state.config.theme.background_style = mode;
                            should_save = true;
                        }
                    }
                });

                if state.config.theme.preset == ThemePreset::LiquidGlass {
                    let bg_hint = match state.config.theme.background_style {
                        BackgroundStyle::Procedural => {
                            "Прозрачное окно композитора без фонового изображения — сквозь карточки Liquid Glass виден реальный рабочий стол."
                        }
                        BackgroundStyle::Solid => {
                            "Лёгкая полупрозрачная стеклянная вуаль поверх окна композитора без фото-обоев."
                        }
                        BackgroundStyle::CustomImage => {
                            "Необязательный пользовательский фон с оптическим преломлением и размытием на фасках карточек."
                        }
                    };
                    ui.add_space(4.0);
                    ui.label(RichText::new(bg_hint).color(tokens.text_muted).small());
                }

                if state.config.theme.background_style == BackgroundStyle::CustomImage {
                    ui.add_space(6.0);
                    let mut bg_path = state.config.theme.backdrop_path.clone().unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Изображение:").color(tokens.text));
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut bg_path)
                                    .hint_text("Путь к .png / .jpg / .webp")
                                    .desired_width((ui.available_width() - 100.0).max(160.0)),
                            )
                            .changed()
                        {
                            let cleaned = crate::opener::clean_user_path(&bg_path);
                            state.config.theme.backdrop_path = if cleaned.is_empty() {
                                None
                            } else {
                                Some(cleaned)
                            };
                            should_save = true;
                        }
                        if state.config.theme.backdrop_path.is_some()
                            && secondary_glass_button(ui, None, "Очистить").clicked()
                        {
                            state.config.theme.backdrop_path = None;
                            should_save = true;
                        }
                    });

                    let dim_resp = ui.horizontal(|ui| {
                        ui.label(RichText::new("Затемнение подложки:").color(tokens.text));
                        ui.add(
                            egui::Slider::new(&mut state.config.theme.backdrop_dim, 0.0..=0.90)
                                .step_by(0.05),
                        )
                    });
                    if dim_resp.inner.drag_stopped() {
                        should_save = true;
                    }
                }
            },
        );

        ui.add_space(12.0);

        // Card 3: Live Preview
        let live_tokens = Tokens::from_config(&state.config.theme);
        let live_style = ThemeStyleParams::from_config(&state.config.theme);
        let preview_r = live_style.card_corner_radius(state.config.theme.corner_radius);
        let preview_card = solid_card_frame_with_config(&live_tokens, &state.config.theme)
            .inner_margin(egui::Margin::same(14));

        show_themed_card(
            ui,
            preview_card,
            preview_r,
            ui.make_persistent_id("settings_live_preview_glass"),
            &live_style,
            &live_tokens,
            |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("Живое превью")
                        .color(live_tokens.text)
                        .strong()
                        .size(13.0),
                );
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    let pri_col = rgb_to_c32(state.config.theme.accent_primary);
                    let sec_col = rgb_to_c32(state.config.theme.accent_secondary);
                    let _ = accent_button(ui, Some(VectorIcon::Play), "Основная кнопка", pri_col);
                    let _ = secondary_glass_button(ui, Some(VectorIcon::Folder), "Вторичная");
                    tag_pill(ui, "Fabric", sec_col);
                    let mut sample_toggle = true;
                    let _ = smooth_toggle(ui, &mut sample_toggle);
                });
            },
        );

        ui.add_space(12.0);

        // Card 4: Surfaces, Radius, Density & Scale + Export
        show_themed_card(
            ui,
            card,
            card_r,
            ui.make_persistent_id("settings_custom_sliders_card"),
            &style,
            &tokens,
            |ui| {
                ui.set_width(ui.available_width());

                ui.label(
                    RichText::new("3. Карточки, скругления и масштаб")
                        .color(tokens.text)
                        .strong()
                        .size(14.0),
                );
                ui.add_space(6.0);

                egui::Grid::new("customization_sliders_grid")
                    .num_columns(2)
                    .spacing(Vec2::new(20.0, 8.0))
                    .show(ui, |ui| {
                        let mut slider_row = |label: &str, slider: egui::Slider<'_>| {
                            ui.label(RichText::new(label).color(tokens.text));
                            ui.scope(|ui| {
                                ui.spacing_mut().slider_width = 210.0;
                                let r = ui.add(slider);
                                if r.changed() {
                                    style_dirty = true;
                                }
                                if r.drag_stopped() {
                                    should_save = true;
                                }
                            });
                            ui.end_row();
                        };

                        slider_row(
                            "Прозрачность окна:",
                            egui::Slider::new(&mut state.config.theme.window_opacity, 0.35..=1.00)
                                .step_by(0.01),
                        );
                        slider_row(
                            "Прозрачность карточек:",
                            egui::Slider::new(&mut state.config.theme.card_opacity, 0.30..=1.00)
                                .step_by(0.01),
                        );
                        slider_row(
                            "Акцентная обводка карточек:",
                            egui::Slider::new(&mut state.config.theme.border_glow, 0.00..=1.00)
                                .step_by(0.05),
                        );
                        slider_row(
                            "Скругление углов:",
                            egui::Slider::new(&mut state.config.theme.corner_radius, 0.0..=16.0)
                                .step_by(1.0)
                                .suffix(" px"),
                        );
                        slider_row(
                            "Масштаб интерфейса:",
                            egui::Slider::new(&mut state.config.theme.ui_scale, 0.80..=1.50)
                                .step_by(0.05),
                        );
                        slider_row(
                            "Размер шрифта:",
                            egui::Slider::new(&mut state.config.theme.font_size, 12.0..=18.0)
                                .step_by(0.5)
                                .suffix(" px"),
                        );
                    });

                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                    ui.label(RichText::new("Плотность:").color(tokens.text));
                    for (density, label) in [
                        (UiDensity::Compact, "Компактная"),
                        (UiDensity::Comfortable, "Комфортная"),
                        (UiDensity::Spacious, "Просторная"),
                    ] {
                        if choice_chip(ui, &tokens, state.config.theme.density == density, label) {
                            state.config.theme.density = density;
                            style_dirty = true;
                            should_save = true;
                        }
                    }
                });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);

                egui::CollapsingHeader::new(
                    RichText::new("Тонкая настройка цветов и экспорт").color(tokens.text),
                )
                .default_open(false)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Фон окна:").color(tokens.text));
                        if ui
                            .color_edit_button_srgb(&mut state.config.theme.bg_window)
                            .changed()
                        {
                            state.config.theme.preset = ThemePreset::Custom;
                            style_dirty = true;
                            should_save = true;
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new("Фон карточек:").color(tokens.text));
                        if ui
                            .color_edit_button_srgb(&mut state.config.theme.bg_card)
                            .changed()
                        {
                            state.config.theme.preset = ThemePreset::Custom;
                            style_dirty = true;
                            should_save = true;
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new("Текст:").color(tokens.text));
                        if ui
                            .color_edit_button_srgb(&mut state.config.theme.text_primary)
                            .changed()
                        {
                            state.config.theme.preset = ThemePreset::Custom;
                            style_dirty = true;
                            should_save = true;
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new("Приглушённый:").color(tokens.text));
                        if ui
                            .color_edit_button_srgb(&mut state.config.theme.text_muted)
                            .changed()
                        {
                            state.config.theme.preset = ThemePreset::Custom;
                            style_dirty = true;
                            should_save = true;
                        }
                    });

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Файл темы:").color(tokens.text));
                        ui.text_edit_singleline(&mut self.theme_export_path);
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if secondary_glass_button(
                            ui,
                            Some(VectorIcon::Download),
                            "Экспортировать тему в файл",
                        )
                        .clicked()
                        {
                            let cleaned = crate::opener::clean_user_path(&self.theme_export_path);
                            let target_path = if cleaned.is_empty() {
                                state.paths.theme_file()
                            } else {
                                std::path::PathBuf::from(cleaned)
                            };
                            if state.config.theme.save_to_file(&target_path).is_ok() {
                                state.status_message = Some((
                                    format!(
                                        "Тема успешно экспортирована в {}",
                                        target_path.display()
                                    ),
                                    false,
                                ));
                            }
                        }

                        if secondary_glass_button(
                            ui,
                            Some(VectorIcon::Folder),
                            "Импортировать тему из файла",
                        )
                        .clicked()
                        {
                            let cleaned = crate::opener::clean_user_path(&self.theme_export_path);
                            let target_path = if cleaned.is_empty() {
                                state.paths.theme_file()
                            } else {
                                std::path::PathBuf::from(cleaned)
                            };
                            if let Ok(loaded) = ThemeConfig::load_from_file(&target_path) {
                                state.config.theme = loaded;
                                apply_theme(ui.ctx(), &state.config.theme);
                                let _ = state.config.save(&state.paths);
                                state.status_message = Some((
                                    format!("Тема успешно загружена из {}", target_path.display()),
                                    false,
                                ));
                            } else {
                                state.status_message = Some((
                                    format!(
                                        "Не удалось прочитать файл темы {}",
                                        target_path.display()
                                    ),
                                    true,
                                ));
                            }
                        }
                    });
                });
            },
        );

        if style_dirty {
            apply_theme(ui.ctx(), &state.config.theme);
        }
        if should_save {
            let _ = state.config.save(&state.paths);
        }
    }

    fn render_launch_settings(&mut self, ui: &mut Ui, state: &mut UiState) {
        let tokens = Tokens::from_config(&state.config.theme);
        let style = ThemeStyleParams::from_config(&state.config.theme);
        let card_r = style.card_corner_radius(state.config.theme.corner_radius);
        let card = solid_card_frame_with_config(&tokens, &state.config.theme)
            .inner_margin(egui::Margin::same(14));

        show_themed_card(
            ui,
            card,
            card_r,
            ui.make_persistent_id("settings_launch_glass"),
            &style,
            &tokens,
            |ui| {
                ui.set_width(ui.available_width());

                // 1. RAM Allocation
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Выделение оперативной памяти (JVM RAM)")
                            .strong()
                            .size(14.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if secondary_glass_button(
                            ui,
                            Some(VectorIcon::Cpu),
                            "Оптимизировать под мой ПК",
                        )
                        .clicked()
                        {
                            let total_ram = mq_core::total_memory_mb();
                            let rec = mq_launch::recommend_jvm_tuning(total_ram, 21, 0);
                            state.config.default_memory_min_mb = rec.memory_min_mb;
                            state.config.default_memory_max_mb = rec.memory_max_mb;
                            state.config.default_jvm_args = rec.jvm_args;
                            state.status_message = Some((rec.summary_ru.clone(), false));
                            let _ = state.config.save(&state.paths);
                        }
                    });
                });
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.label("Минимум RAM (-Xms):");
                    ui.add(
                        egui::Slider::new(&mut state.config.default_memory_min_mb, 512..=8192)
                            .suffix(" MB")
                            .step_by(256.0),
                    );
                });

                ui.horizontal(|ui| {
                    ui.label("Максимум RAM (-Xmx):");
                    ui.add(
                        egui::Slider::new(&mut state.config.default_memory_max_mb, 1024..=32768)
                            .suffix(" MB")
                            .step_by(512.0),
                    );
                });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                // 2. Quick JVM Flag Presets
                ui.label(
                    RichText::new("Быстрые пресеты аргументов JVM")
                        .strong()
                        .size(14.0),
                );
                ui.add_space(4.0);

                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                    if secondary_glass_button(
                        ui,
                        Some(VectorIcon::Cpu),
                        "Aikar's Flags (High Perf)",
                    )
                    .clicked()
                    {
                        state.config.default_jvm_args = vec![
                            "-XX:+UseG1GC".to_string(),
                            "-XX:+ParallelRefProcEnabled".to_string(),
                            "-XX:MaxGCPauseMillis=200".to_string(),
                            "-XX:+UnlockExperimentalVMOptions".to_string(),
                            "-XX:+DisableExplicitGC".to_string(),
                            "-XX:+AlwaysPreTouch".to_string(),
                            "-XX:G1NewSizePercent=30".to_string(),
                            "-XX:G1MaxNewSizePercent=40".to_string(),
                            "-XX:G1ReservePercent=20".to_string(),
                            "-XX:G1HeapWastePercent=5".to_string(),
                            "-XX:G1MixedGCCountTarget=4".to_string(),
                            "-XX:InitiatingHeapOccupancyPercent=15".to_string(),
                            "-XX:G1MixedGCLiveThresholdPercent=90".to_string(),
                            "-XX:G1RSetUpdatingPauseTimePercent=5".to_string(),
                            "-XX:SurvivorRatio=32".to_string(),
                            "-XX:+PerfDisableSharedMem".to_string(),
                            "-XX:MaxTenuringThreshold=1".to_string(),
                        ];
                    }

                    if secondary_glass_button(ui, Some(VectorIcon::Cpu), "Java 21 Generational ZGC")
                        .clicked()
                    {
                        state.config.default_jvm_args = vec![
                            "-XX:+UnlockExperimentalVMOptions".to_string(),
                            "-XX:+UseZGC".to_string(),
                            "-XX:+ZGenerational".to_string(),
                        ];
                    }

                    if secondary_glass_button(ui, Some(VectorIcon::Cpu), "Low-End PC (Low Memory)")
                        .clicked()
                    {
                        state.config.default_jvm_args = vec![
                            "-XX:+UseG1GC".to_string(),
                            "-XX:+ParallelRefProcEnabled".to_string(),
                            "-XX:MaxGCPauseMillis=100".to_string(),
                            "-XX:+UnlockExperimentalVMOptions".to_string(),
                            "-XX:G1NewSizePercent=20".to_string(),
                        ];
                    }
                });

                ui.add_space(6.0);
                ui.label("Текущие аргументы JVM:");
                let mut jvm_args_str = state.config.default_jvm_args.join(" ");
                if ui.text_edit_singleline(&mut jvm_args_str).changed() {
                    state.config.default_jvm_args = jvm_args_str
                        .split_whitespace()
                        .map(|s| s.to_string())
                        .collect();
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                // 3. Wrapper Command
                ui.label(
                    RichText::new("Команда-обёртка запуска игры")
                        .strong()
                        .size(14.0),
                );
                ui.add_space(4.0);

                ui.horizontal_wrapped(|ui| {
                    if ui
                        .selectable_label(state.config.default_wrapper.is_none(), "Без обёртки")
                        .clicked()
                    {
                        state.config.default_wrapper = None;
                    }
                    if cfg!(target_os = "windows") {
                        ui.label(
                            RichText::new(
                                "Linux-пресеты (gamemoderun, mangohud, prime-run) скрыты на Windows.",
                            )
                            .color(tokens.text_muted)
                            .small(),
                        );
                    } else {
                        if ui
                            .selectable_label(
                                state.config.default_wrapper.as_deref() == Some("gamemoderun"),
                                "gamemoderun",
                            )
                            .clicked()
                        {
                            state.config.default_wrapper = Some("gamemoderun".to_string());
                        }
                        if ui
                            .selectable_label(
                                state.config.default_wrapper.as_deref() == Some("mangohud"),
                                "mangohud",
                            )
                            .clicked()
                        {
                            state.config.default_wrapper = Some("mangohud".to_string());
                        }
                        if ui
                            .selectable_label(
                                state.config.default_wrapper.as_deref() == Some("prime-run"),
                                "prime-run",
                            )
                            .clicked()
                        {
                            state.config.default_wrapper = Some("prime-run".to_string());
                        }
                    }
                });

                let mut custom_wrapper = state.config.default_wrapper.clone().unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label("Или произвольная команда:");
                    if ui.text_edit_singleline(&mut custom_wrapper).changed() {
                        state.config.default_wrapper = if custom_wrapper.trim().is_empty() {
                            None
                        } else {
                            Some(custom_wrapper.trim().to_string())
                        };
                    }
                });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                // 4. Action upon Launch
                ui.label(
                    RichText::new("Поведение лаунчера при запуске Minecraft")
                        .strong()
                        .size(14.0),
                );
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut state.config.theme.launch_action,
                        LaunchAction::KeepOpen,
                        "Оставить лаунчер открытым",
                    );
                    ui.selectable_value(
                        &mut state.config.theme.launch_action,
                        LaunchAction::Minimize,
                        "Свернуть",
                    );
                    ui.selectable_value(
                        &mut state.config.theme.launch_action,
                        LaunchAction::HideUntilExit,
                        "Скрыть до выхода из игры",
                    );
                });

                ui.add_space(8.0);

                // Discord Rich Presence
                let mut d_rpc = state.config.discord_rpc_enabled;
                if ui
                    .checkbox(&mut d_rpc, "Discord Rich Presence (отображать статус игры)")
                    .changed()
                {
                    state.config.discord_rpc_enabled = d_rpc;
                    let _ = state.config.save(&state.paths);
                }

                // Автобэкап миров перед запуском
                let mut auto_bk = state.config.auto_backup_worlds;
                ui.horizontal(|ui| {
                    if ui
                        .checkbox(&mut auto_bk, "Автобэкап миров перед запуском")
                        .changed()
                    {
                        state.config.auto_backup_worlds = auto_bk;
                        let _ = state.config.save(&state.paths);
                    }
                    if state.config.auto_backup_worlds {
                        ui.add_space(12.0);
                        ui.label("Хранить копий:");
                        let mut max_bk = state.config.max_world_backups.clamp(1, 10);
                        if ui.add(egui::Slider::new(&mut max_bk, 1..=10)).changed() {
                            state.config.max_world_backups = max_bk;
                            let _ = state.config.save(&state.paths);
                        }
                    }
                });

                // Скрывать лаунчер при запуске игры
                let mut hide_on = state.config.hide_on_launch;
                if ui
                    .checkbox(&mut hide_on, "Скрывать лаунчер при запуске игры")
                    .changed()
                {
                    state.config.hide_on_launch = hide_on;
                    let _ = state.config.save(&state.paths);
                }

                // Компактный мини-оверлей при запуске
                ui.horizontal(|ui| {
                    let mut mini_ov = state.config.mini_overlay_on_launch;
                    if ui
                        .checkbox(&mut mini_ov, "Компактный мини-оверлей при запуске")
                        .changed()
                    {
                        state.config.mini_overlay_on_launch = mini_ov;
                        let _ = state.config.save(&state.paths);
                    }
                    ui.add_space(8.0);
                    let toggle_label = if state.mini_overlay_active {
                        "Отключить мини-оверлей"
                    } else {
                        "Переключить мини-оверлей"
                    };
                    if secondary_glass_button(ui, None, toggle_label).clicked() {
                        state.mini_overlay_active = !state.mini_overlay_active;
                    }
                });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(10.0);

                // 5. Java Runtimes & 1-Click Adoptium Downloader
                ui.label(
                    RichText::new("Управление Java (JRE / JDK)")
                        .strong()
                        .size(14.0),
                );
                ui.add_space(4.0);

                let tokens = Tokens::from_config(&state.config.theme);
                if state.discovered_java_runtimes.is_none() && !state.is_discovering_java {
                    state.is_discovering_java = true;
                    let _ = state
                        .cmd_sender
                        .send(crate::state::UiWorkerCommand::DiscoverJavaRuntimes);
                }

                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                    if state.is_discovering_java && state.discovered_java_runtimes.is_none() {
                        ui.label(
                            RichText::new("Поиск установленных версий Java...")
                                .color(tokens.text_muted)
                                .small(),
                        );
                    } else if let Some(ref runtimes) = state.discovered_java_runtimes {
                        if runtimes.is_empty() {
                            ui.label(
                                RichText::new("Установленные версии Java не найдены в системе")
                                    .color(tokens.text_muted),
                            );
                        } else {
                            ui.label(
                                RichText::new(format!("Найдено JVM в системе: {}", runtimes.len()))
                                    .color(tokens.text_muted)
                                    .small(),
                            );
                        }
                    }
                    if !state.is_discovering_java
                        && secondary_glass_button(ui, Some(VectorIcon::Refresh), "Обновить")
                            .clicked()
                    {
                        state.is_discovering_java = true;
                        let _ = state
                            .cmd_sender
                            .send(crate::state::UiWorkerCommand::DiscoverJavaRuntimes);
                    }
                });

                if let Some(ref runtimes) = state.discovered_java_runtimes {
                    for rt in runtimes.iter().take(5) {
                        ui.horizontal(|ui| {
                            version_badge(
                                ui,
                                &format!("Java {}", rt.major_version),
                                crate::theme::ACCENT_COBALT,
                            );
                            ui.label(RichText::new(&rt.vendor).color(tokens.text).strong());
                            ui.label(
                                RichText::new(rt.path.display().to_string())
                                    .color(tokens.text_muted)
                                    .small(),
                            );
                        });
                    }
                }

                ui.add_space(6.0);
                ui.label("Скачать проверенную Eclipse Temurin JRE напрямую из Adoptium:");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                    for &ver in &[8u32, 17u32, 21u32] {
                        let btn_label = match ver {
                            8 => "Скачать Java 8 (<= 1.16.5)",
                            17 => "Скачать Java 17 (1.17 - 1.20.4)",
                            21 => "Скачать Java 21 (1.20.5+)",
                            _ => "Скачать Java",
                        };
                        if secondary_glass_button(ui, Some(VectorIcon::Download), btn_label)
                            .clicked()
                        {
                            let java_dir = state.paths.java_dir();
                            let bus = state.event_bus.clone();
                            let cmd_tx = state.cmd_sender.clone();
                            tokio::spawn(async move {
                                bus.publish(mq_core::Event::PhaseStarted {
                                    phase: format!("Скачивание Eclipse Temurin Java {ver}"),
                                    total_items: 1,
                                });
                                let client = reqwest::Client::builder()
                                    .timeout(std::time::Duration::from_secs(120))
                                    .build()
                                    .unwrap_or_default();
                                match mq_launch::download_adoptium_java(&client, ver, &java_dir)
                                    .await
                                {
                                    Ok(rt) => {
                                        let _ = cmd_tx.send(
                                            crate::state::UiWorkerCommand::DiscoverJavaRuntimes,
                                        );
                                        bus.publish(mq_core::Event::PhaseCompleted {
                                            phase: format!(
                                                "Java {ver} успешно установлена: {}",
                                                rt.full_version
                                            ),
                                            elapsed_ms: 0,
                                        });
                                    }
                                    Err(e) => {
                                        bus.publish(mq_core::Event::TaskError {
                                            context: format!("Загрузка Java {ver}"),
                                            message: e.to_string(),
                                        });
                                    }
                                }
                            });
                        }
                    }
                });
            },
        );
    }

    fn render_localization_settings(&mut self, ui: &mut Ui, state: &mut UiState) {
        let tokens = Tokens::from_config(&state.config.theme);
        let style = ThemeStyleParams::from_config(&state.config.theme);
        let card_r = style.card_corner_radius(state.config.theme.corner_radius);
        let card = solid_card_frame_with_config(&tokens, &state.config.theme)
            .inner_margin(egui::Margin::same(14));

        show_themed_card(
            ui,
            card,
            card_r,
            ui.make_persistent_id("settings_loc_glass"),
            &style,
            &tokens,
            |ui| {
                ui.set_width(ui.available_width());

                // 1. Launcher Interface Language
                ui.label(
                    RichText::new("1. Язык интерфейса лаунчера (11 языков)")
                        .color(tokens.text)
                        .strong()
                        .size(14.0),
                );
                ui.add_space(4.0);

                ui.horizontal_wrapped(|ui| {
                    for lang in Language::ALL {
                        let is_active = state.config.launcher_locale == lang.code();
                        if ui.selectable_label(is_active, lang.native_name()).clicked() {
                            state.config.launcher_locale = lang.code().to_string();
                            // Automatically harmonize default Minecraft locale if not set
                            state.config.default_mc_locale =
                                mq_meta::launcher_locale_to_mc_locale(lang.code()).to_string();
                        }
                    }
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(12.0);

                // 2. Default Minecraft Game Locale (80+ locales)
                ui.label(
                    RichText::new("2. Язык Minecraft по умолчанию (80+ языков)")
                        .color(tokens.text)
                        .strong()
                        .size(14.0),
                );
                ui.label(RichText::new("Применяется автоматически ко всем новым инстансам и синхронизируется в options.txt перед запуском.").color(tokens.text_muted).small());
                ui.add_space(6.0);

                let current_info = find_mc_locale(&state.config.default_mc_locale);
                let current_title = current_info
                    .map(|i| i.display_label())
                    .unwrap_or_else(|| state.config.default_mc_locale.clone());

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Выбранная локаль Minecraft:").color(tokens.text));
                    version_badge(ui, &current_title, tokens.accent);
                });

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), egui::Sense::hover());
                crate::widgets::paint_vector_icon(ui.painter(), rect, VectorIcon::Search, Color32::from_rgb(0x3B, 0x82, 0xF6));
                let frame_resp = egui::Frame::new()
                    .fill(tokens.bg_input)
                    .stroke(Stroke::new(1.0_f32, tokens.border))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(8, 5))
                    .show(ui, |ui| {
                        ui.set_min_size(Vec2::new(324.0, 18.0));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.mc_locale_search)
                                .frame(false)
                                .hint_text("Поиск по всем 80+ языкам Minecraft (напр. українська, русский, polski)...")
                                .desired_width(324.0)
                                .vertical_align(egui::Align::Center),
                        );
                    });
                register_widget(
                    ui,
                    &frame_resp.response,
                    ElementMeta::new(ElementKind::Input)
                        .id("mc_locale_search")
                        .label("Поиск языка Minecraft")
                        .text(self.mc_locale_search.as_str())
                        .fg(tokens.text)
                        .bg(tokens.bg_input),
                );
            });

                ui.add_space(6.0);
                let search_results = search_mc_locales(&self.mc_locale_search);
                ScrollArea::vertical()
                    .max_height(160.0)
                    .id_salt("mc_locales_scroll")
                    .show(ui, |ui| {
                        for loc in search_results.iter().take(25) {
                            let is_active = state.config.default_mc_locale == loc.code;
                            if ui
                                .selectable_label(is_active, loc.display_label())
                                .clicked()
                            {
                                state.config.default_mc_locale = loc.code.to_string();
                            }
                        }
                    });
            },
        );
    }

    fn render_catalog_settings(&mut self, ui: &mut Ui, state: &mut UiState) {
        let tokens = Tokens::from_config(&state.config.theme);
        ui.label(RichText::new("Ключ CurseForge").color(tokens.text).strong());
        ui.add_space(4.0);
        let from_env = std::env::var("MQ_CURSEFORGE_API_KEY").is_ok_and(|v| !v.trim().is_empty());
        let saved = state.config.curseforge_api_key.is_some();
        let status = if saved {
            "Ключ сохранён (файл с правами 0600)."
        } else if from_env {
            "Используется ключ из переменной MQ_CURSEFORGE_API_KEY."
        } else if state.config.effective_cf_api_key().is_some() {
            "Используется встроенный ключ CurseForge (каталог работает сразу)."
        } else {
            "Ключ не задан: CurseForge отключён, Modrinth работает полностью."
        };
        ui.label(RichText::new(status).color(tokens.text_muted));
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Каталог CurseForge работает из коробки со встроенным ключом. При желании вы можете \
                 указать свой личный ключ из консоли CurseForge ниже.",
            )
            .color(tokens.text_muted),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if secondary_glass_button(ui, None, "Открыть страницу ключей").clicked()
            {
                let _ = crate::opener::open_that(crate::catalog_view::CURSEFORGE_KEYS_URL);
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.catalog_key_input)
                    .password(true)
                    .hint_text("API-ключ CurseForge")
                    .desired_width(320.0),
            );
            let can_save = !self.catalog_key_input.trim().is_empty();
            if primary_emerald_button(ui, Some(VectorIcon::Check), "Сохранить ключ").clicked()
                && can_save
            {
                let key = mq_core::SecretString::new(self.catalog_key_input.trim());
                state.config.curseforge_api_key = Some(key);
                let _ = state.config.save(&state.paths);
                self.catalog_key_input.clear();
                state
                    .catalog
                    .mark_page_changed(std::time::Instant::now(), false);
            }
            if saved && secondary_glass_button(ui, None, "Удалить ключ").clicked() {
                state.config.curseforge_api_key = None;
                let _ = state.config.save(&state.paths);
                let _ = mq_core::AppConfig::save_cf_api_key_secret_file(&state.paths, None);
            }
        });
    }

    fn render_network_api_settings(&mut self, ui: &mut Ui, state: &mut UiState) {
        let tokens = Tokens::from_config(&state.config.theme);
        let style = ThemeStyleParams::from_config(&state.config.theme);
        let card_r = style.card_corner_radius(state.config.theme.corner_radius);
        let card = solid_card_frame_with_config(&tokens, &state.config.theme)
            .inner_margin(egui::Margin::same(14));

        show_themed_card(
            ui,
            card,
            card_r,
            ui.make_persistent_id("settings_net_glass"),
            &style,
            &tokens,
            |ui| {
                ui.set_width(ui.available_width());

                ui.label(RichText::new("Потоки и соединения").strong());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Максимум параллельных HTTP-соединений:");
                    ui.add(egui::Slider::new(
                        &mut state.config.max_connections,
                        4..=128,
                    ));
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                ui.label(RichText::new("Интеграции и ключи API").strong());
                ui.add_space(4.0);
                ui.label("CurseForge Core API Key:");
                let mut cf_key = state
                    .config
                    .curseforge_api_key
                    .as_ref()
                    .map(|s| s.expose_secret().to_string())
                    .unwrap_or_default();
                if ui
                    .add(egui::TextEdit::singleline(&mut cf_key).password(true))
                    .changed()
                {
                    state.config.curseforge_api_key = if cf_key.trim().is_empty() {
                        None
                    } else {
                        Some(mq_core::SecretString::new(cf_key.trim()))
                    };
                }

                ui.add_space(6.0);
                ui.label("Microsoft OAuth Client ID:");
                let mut ms_id = state.config.ms_client_id.clone().unwrap_or_default();
                if ui.text_edit_singleline(&mut ms_id).changed() {
                    state.config.ms_client_id = if ms_id.trim().is_empty() {
                        None
                    } else {
                        Some(ms_id.trim().to_string())
                    };
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                ui.label(RichText::new("Графический бэкенд (GUI)").strong());
                ui.add_space(4.0);
                let (auto_label, wgpu_label) = if cfg!(target_os = "windows") {
                    ("Авто (DX12 / Vulkan -> OpenGL)", "DX12 / Vulkan (WGPU)")
                } else {
                    ("Авто (Vulkan -> OpenGL)", "Vulkan (WGPU)")
                };
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut state.config.ui_backend, GuiBackend::Auto, auto_label);
                    ui.selectable_value(
                        &mut state.config.ui_backend,
                        GuiBackend::Vulkan,
                        wgpu_label,
                    );
                    ui.selectable_value(
                        &mut state.config.ui_backend,
                        GuiBackend::Gl,
                        "OpenGL (Glow)",
                    );
                });
            },
        );
    }

    fn render_updates_settings(&mut self, ui: &mut Ui, state: &mut UiState) {
        if let Some(res) = state.pending_update_check.take() {
            self.is_checking_update = false;
            match res {
                Ok(Some(info)) => {
                    self.update_check_result =
                        Some(format!("Найдена новая версия: v{}", info.version));
                    self.available_update = Some(info);
                }
                Ok(None) => {
                    self.available_update = None;
                    self.update_check_result =
                        Some("У вас установлена самая актуальная версия лаунчера.".to_string());
                }
                Err(e) => {
                    self.update_check_result = Some(format!("Ошибка проверки обновлений: {e}"));
                }
            }
        }
        if let Some(err) = state.pending_update_error.take() {
            self.is_downloading_update = false;
            self.update_check_result = Some(format!("Ошибка установки обновления: {err}"));
        }

        let tokens = Tokens::from_config(&state.config.theme);
        let style = ThemeStyleParams::from_config(&state.config.theme);
        let card_r = style.card_corner_radius(state.config.theme.corner_radius);
        let max_w = ui.available_width().min(640.0);
        ui.allocate_ui_with_layout(
            Vec2::new(max_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                let card = solid_card_frame_with_config(&tokens, &state.config.theme)
                    .inner_margin(egui::Margin::same(14));

                show_themed_card(
                    ui,
                    card,
                    card_r,
                    ui.make_persistent_id("settings_updates_glass"),
                    &style,
                    &tokens,
                    |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Обновления лаунчера").color(tokens.text).strong().size(15.0));
                    ui.add_space(8.0);

                    let is_packaged = mq_update::UpdateManager::is_packaged_install();
                    if is_packaged {
                        let pkg_notice = if cfg!(target_os = "windows") {
                            "Лаунчер установлен через системный каталог / менеджер пакетов (например, winget или Program Files). Для обновления используйте менеджер пакетов или установщик."
                        } else {
                            "Лаунчер установлен через системный пакетный менеджер (/usr/). Для обновления используйте пакетный менеджер вашей системы."
                        };
                        ui.label(RichText::new(pkg_notice).color(tokens.warning));
                        ui.add_space(8.0);
                        if secondary_glass_button(ui, Some(VectorIcon::Globe), "Открыть страницу релизов на GitHub").clicked() {
                            let _ = crate::opener::open_that("https://github.com/MrQuster/mq/releases");
                        }
                    } else {
                        ui.label(RichText::new(format!("Текущая версия: v{}", env!("CARGO_PKG_VERSION"))).color(tokens.text_muted));
                        ui.add_space(6.0);

                        if let Some(ref status) = self.update_check_result {
                            ui.label(RichText::new(status).color(tokens.text));
                            ui.add_space(6.0);
                        }

                        if self.is_checking_update {
                            ui.label(RichText::new("Проверка наличия обновлений...").color(ACCENT_COBALT));
                        } else if let Some(ref asset) = self.available_update.clone() {
                            ui.label(RichText::new(format!("Доступна новая версия: v{}", asset.version)).color(tokens.accent).strong());
                            if !asset.release_notes.is_empty() {
                                ui.add_space(4.0);
                                ui.label(RichText::new("Список изменений:").small().color(tokens.text_muted));
                                egui::Frame::new()
                                    .fill(tokens.bg_input)
                                    .stroke(Stroke::new(1.0_f32, tokens.border))
                                    .corner_radius(crate::theme::RADIUS_CARD)
                                    .inner_margin(egui::Margin::same(8))
                                    .show(ui, |ui| {
                                        ui.label(RichText::new(&asset.release_notes).color(tokens.text).small());
                                    });
                            }
                            ui.add_space(8.0);

                            if self.is_downloading_update {
                                ui.label(RichText::new("Загрузка и установка обновления...").color(ACCENT_COBALT));
                                crate::components::render_progress_bar(ui, &tokens, self.update_progress, 6.0_f32);
                            } else if primary_emerald_button(ui, Some(VectorIcon::Download), "Обновить лаунчер сейчас").clicked() {
                                self.is_downloading_update = true;
                                let _ = state.cmd_sender.send(
                                    crate::state::UiWorkerCommand::DownloadLauncherUpdate {
                                        asset: Box::new(asset.clone()),
                                    },
                                );
                            }
                        } else if secondary_glass_button(ui, Some(VectorIcon::Refresh), "Проверить сейчас").clicked() {
                            self.is_checking_update = true;
                            self.update_check_result = None;
                            let _ = state.cmd_sender.send(
                                crate::state::UiWorkerCommand::CheckLauncherUpdate {
                                    current_version: env!("CARGO_PKG_VERSION").to_string(),
                                },
                            );
                        }

                        // Check if rollback is available (`<exe>.old` on Windows/Linux or `<stem>.old`)
                        if let Ok(exe) = std::env::current_exe() {
                            if rollback_backup_exists(&exe) {
                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(8.0);
                                ui.label(RichText::new("Доступна предыдущая сохранённая версия.").color(tokens.text_muted).small());
                                if secondary_glass_button(ui, Some(VectorIcon::Refresh), "Откатить до предыдущей версии").clicked() {
                                    if let Ok(()) = mq_update::UpdateManager::rollback() {
                                        state.status_message = Some(("Успешный откат к предыдущей версии. Перезапустите лаунчер.".to_string(), false));
                                    } else {
                                        state.status_message = Some(("Ошибка при откате к предыдущей версии".to_string(), true));
                                    }
                                }
                            }
                        }
                    }
                    },
                );

                ui.add_space(12.0);
                self.render_uninstall_card(ui, state, &tokens);
            },
        );
    }

    fn render_uninstall_card(&mut self, ui: &mut Ui, state: &mut UiState, tokens: &Tokens) {
        let style = ThemeStyleParams::from_config(&state.config.theme);
        let card_r = style.card_corner_radius(state.config.theme.corner_radius);
        let card = solid_card_frame_with_config(tokens, &state.config.theme)
            .inner_margin(egui::Margin::same(14));

        show_themed_card(
            ui,
            card,
            card_r,
            ui.make_persistent_id("settings_uninstall_glass"),
            &style,
            tokens,
            |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("Удаление MQ Launcher")
                        .color(tokens.text)
                        .strong()
                        .size(15.0),
                );
                ui.add_space(6.0);
                ui.label(
                RichText::new(
                    "Удаляет бинарник лаунчера, ярлык и иконку в системе (Linux и Windows). Инстансы Minecraft, моды, миры и аккаунты по умолчанию сохраняются.",
                )
                .color(tokens.text_muted)
                .small(),
            );
                ui.add_space(8.0);

                ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                if secondary_glass_button(
                    ui,
                    Some(VectorIcon::Copy),
                    "Скопировать команду удаления (Терминал / PowerShell)",
                )
                .clicked()
                {
                    let cmd = if cfg!(target_os = "windows") {
                        "$env:MQ_UNINSTALL=1; irm https://github.com/MrQuster/mq/releases/latest/download/install.ps1 | iex"
                    } else {
                        "curl -fsSL https://github.com/MrQuster/mq/releases/latest/download/install.sh | sh -s -- --uninstall"
                    };
                    ui.ctx().copy_text(cmd.to_string());
                    state.status_message =
                        Some(("Команда удаления скопирована в буфер обмена".to_string(), false));
                }

                if !self.confirm_uninstall
                    && secondary_glass_button(
                        ui,
                        Some(VectorIcon::Trash),
                        "Удалить лаунчер...",
                    )
                    .clicked()
                {
                    self.confirm_uninstall = true;
                }
            });

                if self.confirm_uninstall {
                    ui.add_space(8.0);
                    ui.checkbox(
                        &mut self.uninstall_purge_data,
                        "Также удалить все инстансы, моды, сохранения миров и настройки",
                    );
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                        if secondary_glass_button(
                            ui,
                            Some(VectorIcon::Trash),
                            "Подтвердить удаление",
                        )
                        .clicked()
                        {
                            match mq_update::UpdateManager::uninstall(
                                &state.paths.data_dir,
                                &state.paths.config_dir,
                                &state.paths.cache_dir,
                                self.uninstall_purge_data,
                                false,
                            ) {
                                Ok(_) => std::process::exit(0),
                                Err(e) => {
                                    state.status_message =
                                        Some((format!("Ошибка удаления: {e}"), true));
                                    self.confirm_uninstall = false;
                                }
                            }
                        }
                        if secondary_glass_button(ui, None, "Отмена").clicked() {
                            self.confirm_uninstall = false;
                            self.uninstall_purge_data = false;
                        }
                    });
                }
            },
        );
    }

    fn render_about_settings(&mut self, ui: &mut Ui, state: &mut UiState) {
        let tokens = Tokens::from_config(&state.config.theme);
        let style = ThemeStyleParams::from_config(&state.config.theme);
        let card_r = style.card_corner_radius(state.config.theme.corner_radius);
        let max_w = ui.available_width().min(780.0);
        ui.allocate_ui_with_layout(
            Vec2::new(max_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                // Centered Serpantinum-style emblem + title + version header directly over the blurred canvas
                ui.vertical_centered(|ui| {
                    ui.add_space(6.0);
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::hover());
                    crate::sidebar::paint_mq_emblem(ui.painter(), rect, &tokens);
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("MQ Launcher")
                            .strong()
                            .size(21.0)
                            .color(tokens.text),
                    );
                    ui.label(
                        RichText::new(format!("v{}  •  от MrQuster", env!("CARGO_PKG_VERSION")))
                            .color(tokens.text_muted)
                            .size(12.5),
                    );
                });

                ui.add_space(14.0);

                // Two-column compact Liquid Glass info tiles (Serpantinum AboutTab style)
                let backend_display = if state.gpu_backend_label.trim().is_empty() {
                    format!("{:?}", state.config.ui_backend)
                } else {
                    state.gpu_backend_label.trim().to_string()
                };
                let data_dir_display = crate::opener::strip_windows_verbatim_prefix(
                    &state.paths.data_dir.display().to_string(),
                );
                let tiles: [(&str, String); 6] = [
                    ("Версия", format!("v{}", env!("CARGO_PKG_VERSION"))),
                    ("Графический бэкенд", backend_display.clone()),
                    (
                        "ОС / Архитектура",
                        format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH),
                    ),
                    ("Автор", "MrQuster".to_string()),
                    ("Лицензия", "MIT License".to_string()),
                    ("Каталог данных", data_dir_display),
                ];

                let render_tile = |ui: &mut Ui, idx: usize, label: &str, value: &str| {
                    let tile_frame = solid_card_frame_with_config(&tokens, &state.config.theme)
                        .inner_margin(egui::Margin::symmetric(12, 9));
                    show_themed_card(
                        ui,
                        tile_frame,
                        card_r,
                        ui.make_persistent_id(("settings_about_tile", idx)),
                        &style,
                        &tokens,
                        |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(label)
                                        .color(tokens.text_muted)
                                        .size(12.5),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(value)
                                                    .color(tokens.text)
                                                    .strong()
                                                    .size(12.5),
                                            )
                                            .truncate(),
                                        );
                                    },
                                );
                            });
                        },
                    );
                };

                for row_idx in (0..tiles.len()).step_by(2) {
                    ui.columns(2, |cols| {
                        let (l0, v0) = &tiles[row_idx];
                        render_tile(&mut cols[0], row_idx, l0, v0);
                        if let Some((l1, v1)) = tiles.get(row_idx + 1) {
                            render_tile(&mut cols[1], row_idx + 1, l1, v1);
                        }
                    });
                    ui.add_space(8.0);
                }

                ui.add_space(6.0);

                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);

                    if secondary_glass_button(ui, Some(VectorIcon::User), "MrQuster (GitHub)")
                        .clicked()
                    {
                        let _ = crate::opener::open_that("https://github.com/MrQuster");
                    }
                    if secondary_glass_button(ui, Some(VectorIcon::Globe), "MrQuster/mq").clicked()
                    {
                        let _ = crate::opener::open_that("https://github.com/MrQuster/mq");
                    }
                    if secondary_glass_button(ui, Some(VectorIcon::Download), "Релизы").clicked()
                    {
                        let _ = crate::opener::open_that("https://github.com/MrQuster/mq/releases");
                    }
                    if secondary_glass_button(ui, Some(VectorIcon::Warning), "Сообщить об ошибке")
                        .clicked()
                    {
                        let _ = crate::opener::open_that("https://github.com/MrQuster/mq/issues");
                    }
                    if secondary_glass_button(
                        ui,
                        Some(VectorIcon::Copy),
                        "Скопировать диагностику",
                    )
                    .clicked()
                    {
                        let diag = format!(
                            "MQ Launcher v{}\nOS: {} ({})\nGUI Backend: {} (config: {:?})\nPaths: Data={:?}, Instances={:?}",
                            env!("CARGO_PKG_VERSION"),
                            std::env::consts::OS,
                            std::env::consts::ARCH,
                            backend_display,
                            state.config.ui_backend,
                            state.paths.data_dir,
                            state.paths.instances_dir(),
                        );
                        ui.ctx().copy_text(diag);
                        state.status_message = Some((
                            "Диагностическая информация скопирована в буфер обмена".to_string(),
                            false,
                        ));
                    }
                });

                ui.add_space(12.0);
                self.render_uninstall_card(ui, state, &tokens);
            },
        );
    }
}

fn rollback_backup_exists(exe: &std::path::Path) -> bool {
    if exe.with_extension("old").exists() {
        return true;
    }
    exe.file_name().is_some_and(|name| {
        let mut appended = name.to_os_string();
        appended.push(".old");
        exe.with_file_name(appended).exists()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_backup_detects_both_windows_exe_old_and_unix_old() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let win_exe = tmp.path().join("mq.exe");
        assert!(!rollback_backup_exists(&win_exe));

        let win_backup = tmp.path().join("mq.exe.old");
        std::fs::write(&win_backup, b"old").expect("write mq.exe.old");
        assert!(rollback_backup_exists(&win_exe));

        std::fs::remove_file(&win_backup).expect("remove mq.exe.old");
        let legacy_backup = tmp.path().join("mq.old");
        std::fs::write(&legacy_backup, b"old").expect("write mq.old");
        assert!(rollback_backup_exists(&win_exe));
    }
}
