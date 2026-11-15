//! Lightweight UI element registry for automated visual/functional QA.
//! When the `test-hooks` feature is disabled, all registration calls compile
//! down to empty inline no-ops with zero runtime overhead and zero strings.

use egui::{Color32, Id, Rect, Response, Ui};

use crate::widgets::VectorIcon;

/// Semantic kind of a registered UI element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementKind {
    Button,
    IconButton,
    Input,
    Toggle,
    Select,
    SegmentedOption,
    Tab,
    Chip,
    NavItem,
    ListRow,
    Card,
    Modal,
    Badge,
    Progress,
    Toast,
    Label,
}

#[cfg(feature = "test-hooks")]
impl ElementKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Button => "button",
            Self::IconButton => "icon_button",
            Self::Input => "input",
            Self::Toggle => "toggle",
            Self::Select => "select",
            Self::SegmentedOption => "segmented_option",
            Self::Tab => "tab",
            Self::Chip => "chip",
            Self::NavItem => "nav_item",
            Self::ListRow => "list_row",
            Self::Card => "card",
            Self::Modal => "modal",
            Self::Badge => "badge",
            Self::Progress => "progress",
            Self::Toast => "toast",
            Self::Label => "label",
        }
    }
}

/// Metadata passed when registering a widget `Response` or painted region.
#[derive(Debug, Clone, Copy)]
pub struct ElementMeta<'a> {
    pub id_hint: Option<&'a str>,
    pub kind: ElementKind,
    pub label: &'a str,
    pub tech_name: Option<&'a str>,
    pub text: &'a str,
    pub tooltip: Option<&'a str>,
    pub fg_color: Color32,
    pub bg_color: Color32,
    pub active: bool,
    pub interactive: bool,
    pub text_clipped: bool,
}

impl<'a> ElementMeta<'a> {
    #[inline]
    pub const fn new(kind: ElementKind) -> Self {
        Self {
            id_hint: None,
            kind,
            label: "",
            tech_name: None,
            text: "",
            tooltip: None,
            fg_color: Color32::TRANSPARENT,
            bg_color: Color32::TRANSPARENT,
            active: false,
            interactive: true,
            text_clipped: false,
        }
    }

    #[inline]
    pub const fn id(mut self, id_hint: &'a str) -> Self {
        self.id_hint = Some(id_hint);
        self
    }

    #[inline]
    pub const fn label(mut self, label: &'a str) -> Self {
        self.label = label;
        self
    }

    #[inline]
    pub const fn tech_name(mut self, tech_name: &'a str) -> Self {
        self.tech_name = Some(tech_name);
        self
    }

    #[inline]
    pub const fn text(mut self, text: &'a str) -> Self {
        self.text = text;
        self
    }

    #[inline]
    pub const fn tooltip(mut self, tooltip: &'a str) -> Self {
        if !tooltip.is_empty() {
            self.tooltip = Some(tooltip);
        }
        self
    }

    #[inline]
    pub const fn tooltip_opt(mut self, tooltip: Option<&'a str>) -> Self {
        self.tooltip = tooltip;
        self
    }

    #[inline]
    pub const fn fg(mut self, fg_color: Color32) -> Self {
        self.fg_color = fg_color;
        self
    }

    #[inline]
    pub const fn bg(mut self, bg_color: Color32) -> Self {
        self.bg_color = bg_color;
        self
    }

    #[inline]
    pub const fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    #[inline]
    pub const fn interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    #[inline]
    pub const fn text_clipped(mut self, text_clipped: bool) -> Self {
        self.text_clipped = text_clipped;
        self
    }
}

/// Returns the technical name for a `VectorIcon` when `test-hooks` is enabled.
#[cfg(feature = "test-hooks")]
pub const fn icon_tech_name(icon: VectorIcon) -> &'static str {
    match icon {
        VectorIcon::Play => "icon:Play",
        VectorIcon::Stop => "icon:Stop",
        VectorIcon::Check => "icon:Check",
        VectorIcon::Verify => "icon:Verify",
        VectorIcon::Folder => "icon:Folder",
        VectorIcon::Plus => "icon:Plus",
        VectorIcon::Trash => "icon:Trash",
        VectorIcon::Search => "icon:Search",
        VectorIcon::Layers => "icon:Layers",
        VectorIcon::Catalog => "icon:Catalog",
        VectorIcon::User => "icon:User",
        VectorIcon::Cpu => "icon:Cpu",
        VectorIcon::Settings => "icon:Settings",
        VectorIcon::Refresh => "icon:Refresh",
        VectorIcon::Copy => "icon:Copy",
        VectorIcon::ChevronRight => "icon:ChevronRight",
        VectorIcon::ChevronDown => "icon:ChevronDown",
        VectorIcon::ChevronsLeft => "icon:ChevronsLeft",
        VectorIcon::ChevronsRight => "icon:ChevronsRight",
        VectorIcon::Activity => "icon:Activity",
        VectorIcon::Warning => "icon:Warning",
        VectorIcon::Shield => "icon:Shield",
        VectorIcon::Download => "icon:Download",
        VectorIcon::Cross => "icon:Cross",
        VectorIcon::Terminal => "icon:Terminal",
        VectorIcon::Globe => "icon:Globe",
        VectorIcon::Eye => "icon:Eye",
        VectorIcon::Edit => "icon:Edit",
        VectorIcon::Cube => "icon:Cube",
        VectorIcon::Server => "icon:Server",
        VectorIcon::Key => "icon:Key",
        VectorIcon::Info => "icon:Info",
    }
}

#[cfg(not(feature = "test-hooks"))]
#[inline(always)]
pub const fn icon_tech_name(_icon: VectorIcon) -> &'static str {
    ""
}

#[cfg(feature = "test-hooks")]
mod imp {
    use super::*;
    use serde::{Deserialize, Serialize};

    const CUR_KEY: &str = "mq_ui_reg_cur";
    const LAST_KEY: &str = "mq_ui_reg_last";

    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    pub struct UiRect {
        pub x: f32,
        pub y: f32,
        pub w: f32,
        pub h: f32,
    }

    impl From<Rect> for UiRect {
        fn from(r: Rect) -> Self {
            Self {
                x: r.left(),
                y: r.top(),
                w: r.width(),
                h: r.height(),
            }
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct UiElementRecord {
        pub id: String,
        pub egui_id: String,
        pub kind: String,
        pub label: String,
        pub tech_name: Option<String>,
        pub text: String,
        pub tooltip: Option<String>,
        pub has_tooltip: bool,
        pub rect: UiRect,
        pub clip_rect: UiRect,
        pub enabled: bool,
        pub visible: bool,
        pub focused: bool,
        pub hovered: bool,
        pub active: bool,
        pub interactive: bool,
        pub clipped: bool,
        pub fg_color: [u8; 4],
        pub bg_color: [u8; 4],
    }

    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct UiRegistrySnapshot {
        pub frame: u64,
        pub viewport_width: f32,
        pub viewport_height: f32,
        pub pixels_per_point: f32,
        pub elements: Vec<UiElementRecord>,
    }

    pub fn begin_frame(ctx: &egui::Context) {
        ctx.data_mut(|d| {
            d.insert_temp(Id::new(CUR_KEY), Vec::<UiElementRecord>::new());
        });
    }

    pub fn end_frame(ctx: &egui::Context, frame: u64) {
        let screen = ctx.screen_rect();
        let ppp = ctx.pixels_per_point();
        ctx.data_mut(|d| {
            let elements = d
                .get_temp::<Vec<UiElementRecord>>(Id::new(CUR_KEY))
                .unwrap_or_default();
            let snap = UiRegistrySnapshot {
                frame,
                viewport_width: screen.width(),
                viewport_height: screen.height(),
                pixels_per_point: ppp,
                elements,
            };
            d.insert_temp(Id::new(LAST_KEY), snap);
        });
    }

    pub fn last_snapshot(ctx: &egui::Context) -> Option<UiRegistrySnapshot> {
        ctx.data(|d| d.get_temp::<UiRegistrySnapshot>(Id::new(LAST_KEY)))
    }

    pub fn push_record(
        ui: &Ui,
        id: Id,
        rect: Rect,
        enabled: bool,
        focused: bool,
        hovered: bool,
        meta: ElementMeta<'_>,
    ) {
        let clip_rect = ui.clip_rect();
        let screen_rect = ui.ctx().screen_rect();
        let visible = rect.width() > 0.0
            && rect.height() > 0.0
            && rect.intersects(clip_rect)
            && rect.intersects(screen_rect);

        let rect_clipped = visible
            && (!clip_rect.expand(1.5).contains_rect(rect)
                || !screen_rect.expand(1.5).contains_rect(rect));
        let clipped = meta.text_clipped || rect_clipped;

        let tooltip = meta
            .tooltip
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let has_tooltip = tooltip.is_some();

        let tech_name = meta
            .tech_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let label_trimmed = meta.label.trim();
        let resolved_label = if !label_trimmed.is_empty() {
            label_trimmed.to_string()
        } else if let Some(ref tn) = tech_name {
            tn.clone()
        } else if let Some(ref tt) = tooltip {
            tt.clone()
        } else {
            format!(
                "{}_{:08x}",
                meta.kind.as_str(),
                (id.value() & 0xffff_ffff) as u32
            )
        };

        let resolved_text = if meta.text.is_empty() {
            resolved_label.clone()
        } else {
            meta.text.to_string()
        };

        let egui_id = format!("{:016x}", id.value());
        let resolved_id = match meta.id_hint {
            Some(hint) if !hint.trim().is_empty() => hint.trim().to_string(),
            _ => format!(
                "{}_{:08x}",
                meta.kind.as_str(),
                (id.value() & 0xffff_ffff) as u32
            ),
        };

        let record = UiElementRecord {
            id: resolved_id,
            egui_id,
            kind: meta.kind.as_str().to_string(),
            label: resolved_label,
            tech_name,
            text: resolved_text,
            tooltip,
            has_tooltip,
            rect: rect.into(),
            clip_rect: clip_rect.into(),
            enabled,
            visible,
            focused,
            hovered,
            active: meta.active,
            interactive: meta.interactive,
            clipped,
            fg_color: meta.fg_color.to_array(),
            bg_color: meta.bg_color.to_array(),
        };

        ui.ctx().data_mut(|d| {
            let mut list = d
                .get_temp::<Vec<UiElementRecord>>(Id::new(CUR_KEY))
                .unwrap_or_default();
            list.push(record);
            d.insert_temp(Id::new(CUR_KEY), list);
        });
    }
}

#[cfg(feature = "test-hooks")]
pub use imp::{begin_frame, end_frame, last_snapshot, UiElementRecord, UiRect, UiRegistrySnapshot};

/// Registers an interactive or non-interactive `egui::Response` into the UI element registry.
#[inline(always)]
pub fn register_widget(ui: &Ui, resp: &Response, meta: ElementMeta<'_>) {
    #[cfg(feature = "test-hooks")]
    {
        imp::push_record(
            ui,
            resp.id,
            resp.rect,
            resp.enabled(),
            resp.has_focus(),
            resp.hovered(),
            meta,
        );
    }
    #[cfg(not(feature = "test-hooks"))]
    {
        let _ = (ui, resp, meta);
    }
}

/// Registers a painted UI region that does not return a `Response`.
#[inline(always)]
pub fn register_rect(
    ui: &Ui,
    id: Id,
    rect: Rect,
    enabled: bool,
    focused: bool,
    hovered: bool,
    meta: ElementMeta<'_>,
) {
    #[cfg(feature = "test-hooks")]
    {
        imp::push_record(ui, id, rect, enabled, focused, hovered, meta);
    }
    #[cfg(not(feature = "test-hooks"))]
    {
        let _ = (ui, id, rect, enabled, focused, hovered, meta);
    }
}
