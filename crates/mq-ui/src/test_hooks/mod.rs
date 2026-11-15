//! In-process `test-hooks` automation & telemetry harness for `mq-ui`.
//! Compiled strictly when `--features test-hooks` is enabled.

pub mod input;
pub mod protocol;
pub mod server;
pub mod stats;

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use egui::{ColorImage, Context, DroppedFile, PointerButton, RawInput, Vec2, ViewportCommand};
use mq_core::{BackgroundStyle, ModLoaderKind, MqPaths, ThemePreset};
use mq_instances::InstanceManifest;
use parking_lot::Mutex;

use crate::catalog_view::CatalogView;
use crate::components::registry::{begin_frame, end_frame, last_snapshot, UiRegistrySnapshot};
use crate::instances_view::InstancesView;
use crate::settings_view::SettingsSection;
use crate::sidebar::{resolve_compact_mode, toggle_sidebar_compact, NavTab};
use crate::state::UiState;
use crate::test_hooks::input::{
    build_click_steps, build_drag_steps, build_hover_steps, build_key_steps, build_scroll_steps,
    build_type_steps, parse_key, parse_modifiers, resolve_target, InputFrameStep,
};
use crate::test_hooks::protocol::{
    merge_target, HarnessCommand, HarnessResponse, RequestEnvelope, TargetSpec,
};
use crate::test_hooks::server::{QueuedRequest, TestHooksServer};
use crate::test_hooks::stats::FrameStatsTracker;
use crate::theme::apply_theme;

/// Mutable view into `MqApp` fields needed by `test-hooks` commands.
pub struct TestAppAccess<'a> {
    pub state: &'a mut UiState,
    pub current_tab: &'a mut NavTab,
    pub settings_section: &'a mut Option<SettingsSection>,
    pub instances_view: &'a mut InstancesView,
    pub catalog_view: &'a mut CatalogView,
}

#[derive(Debug, Clone, Default)]
pub struct TestFlags {
    pub deterministic: bool,
    pub disable_animations: bool,
    pub solid_background: bool,
}

struct PendingScreenshot {
    req_id: Option<u64>,
    path: PathBuf,
    region: Option<(f32, f32, f32, f32, bool)>,
    requested_at: Instant,
    reply_tx: mpsc::SyncSender<HarnessResponse>,
}

enum ActiveJob {
    DumpUi {
        req_id: Option<u64>,
        frames_remaining: u32,
        reply_tx: mpsc::SyncSender<HarnessResponse>,
    },
    State {
        req_id: Option<u64>,
        reply_tx: mpsc::SyncSender<HarnessResponse>,
    },
    Settle {
        req_id: Option<u64>,
        frames_remaining: u32,
        payload: serde_json::Value,
        reply_tx: mpsc::SyncSender<HarnessResponse>,
    },
    InputSequence {
        req_id: Option<u64>,
        initial_ui_hash: u64,
        meta: serde_json::Value,
        reply_tx: mpsc::SyncSender<HarnessResponse>,
    },
    Resize {
        req_id: Option<u64>,
        target: (f32, f32),
        frames_waited: u32,
        reply_tx: mpsc::SyncSender<HarnessResponse>,
    },
    WaitIdle {
        req_id: Option<u64>,
        started: Instant,
        timeout: Duration,
        required_frames: u32,
        idle_frames: u32,
        reply_tx: mpsc::SyncSender<HarnessResponse>,
    },
    WaitFor {
        req_id: Option<u64>,
        spec: TargetSpec,
        text: Option<String>,
        tab: Option<NavTab>,
        require_visible: bool,
        started: Instant,
        timeout: Duration,
        reply_tx: mpsc::SyncSender<HarnessResponse>,
    },
}

pub struct TestHooksController {
    server: Option<TestHooksServer>,
    cmd_rx: Option<mpsc::Receiver<QueuedRequest>>,
    deferred_req: Option<QueuedRequest>,
    active_job: Option<ActiveJob>,
    pending_raw_steps: VecDeque<InputFrameStep>,
    pending_screenshot: Option<PendingScreenshot>,
    stats: FrameStatsTracker,
    pub flags: TestFlags,
    frame_count: u64,
    frame_start: Option<Instant>,
    last_ui_hash: u64,
    action_seq: u64,
}

impl Default for TestHooksController {
    fn default() -> Self {
        let deterministic = std::env::var("MQ_TEST_DETERMINISTIC")
            .ok()
            .is_some_and(|v| {
                matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")
            });
        Self {
            server: None,
            cmd_rx: None,
            deferred_req: None,
            active_job: None,
            pending_raw_steps: VecDeque::new(),
            pending_screenshot: None,
            stats: FrameStatsTracker::default(),
            flags: TestFlags {
                deterministic,
                disable_animations: deterministic,
                solid_background: deterministic,
            },
            frame_count: 0,
            frame_start: None,
            last_ui_hash: 0,
            action_seq: 0,
        }
    }
}

impl TestHooksController {
    pub fn start(&mut self, ctx: &Context, paths: &MqPaths) {
        if self.server.is_some() {
            return;
        }
        let disabled = std::env::var("MQ_TEST_HOOKS")
            .ok()
            .is_some_and(|v| matches!(v.trim(), "0" | "false" | "off"));
        if disabled {
            return;
        }

        match TestHooksServer::start(ctx.clone(), paths) {
            Ok((srv, rx)) => {
                self.server = Some(srv);
                self.cmd_rx = Some(rx);
            }
            Err(e) => {
                tracing::warn!("test-hooks IPC server failed to start: {e}");
            }
        }
    }

    pub fn is_solid_background(&self) -> bool {
        self.flags.solid_background || self.flags.deterministic
    }

    pub fn raw_input_hook(&mut self, ctx: &Context, raw_input: &mut RawInput) {
        if let Some(pending) = self.pending_screenshot.take() {
            let mut captured: Option<std::sync::Arc<ColorImage>> = None;
            for ev in &raw_input.events {
                if let egui::Event::Screenshot { image, .. } = ev {
                    captured = Some(image.clone());
                    break;
                }
            }
            if let Some(img) = captured {
                let res = save_screenshot_png(
                    &img,
                    &pending.path,
                    pending.region,
                    ctx.pixels_per_point(),
                );
                let resp = match res {
                    Ok((w, h)) => HarnessResponse::ok(
                        pending.req_id,
                        serde_json::json!({
                            "path": pending.path,
                            "width": w,
                            "height": h,
                        }),
                    ),
                    Err(e) => {
                        HarnessResponse::err(pending.req_id, format!("Screenshot save failed: {e}"))
                    }
                };
                let _ = pending.reply_tx.send(resp);
            } else {
                self.pending_screenshot = Some(pending);
            }
        }

        if let Some(step) = self.pending_raw_steps.pop_front() {
            raw_input.events.extend(step.events);
            raw_input.dropped_files.extend(step.dropped_files);
            ctx.request_repaint();
        }
    }

    pub fn pre_frame(&mut self, ctx: &Context, app: &mut TestAppAccess<'_>) {
        self.frame_start = Some(Instant::now());
        self.frame_count += 1;
        begin_frame(ctx);

        if self.flags.disable_animations || self.flags.deterministic {
            ctx.style_mut(|s| {
                s.animation_time = 0.0;
            });
        }

        if self.active_job.is_some()
            || self.pending_screenshot.is_some()
            || !self.pending_raw_steps.is_empty()
        {
            return;
        }

        let queued = self
            .deferred_req
            .take()
            .or_else(|| self.cmd_rx.as_ref().and_then(|rx| rx.try_recv().ok()));

        if let Some(q) = queued {
            self.dispatch_request(ctx, app, q);
        }
    }

    pub fn post_frame(&mut self, ctx: &Context, app: &mut TestAppAccess<'_>) {
        end_frame(ctx, self.frame_count);

        if let Some(start) = self.frame_start.take() {
            self.stats.record(start.elapsed());
        }

        let snap = last_snapshot(ctx);
        let new_hash = compute_ui_hash(app, snap.as_ref());
        if new_hash != self.last_ui_hash {
            self.last_ui_hash = new_hash;
            self.action_seq += 1;
        }

        if let Some(ref pending) = self.pending_screenshot {
            if pending.requested_at.elapsed() > Duration::from_secs(10) {
                let p = self.pending_screenshot.take().unwrap();
                let _ = p.reply_tx.send(HarnessResponse::err(
                    p.req_id,
                    "Screenshot capture timed out",
                ));
            } else {
                ctx.request_repaint();
            }
        }

        let Some(job) = self.active_job.take() else {
            return;
        };

        match job {
            ActiveJob::DumpUi {
                req_id,
                frames_remaining,
                reply_tx,
            } => {
                if frames_remaining > 0 {
                    self.active_job = Some(ActiveJob::DumpUi {
                        req_id,
                        frames_remaining: frames_remaining - 1,
                        reply_tx,
                    });
                    ctx.request_repaint();
                } else {
                    let s = snap.unwrap_or_default();
                    let resp = HarnessResponse::ok(
                        req_id,
                        serde_json::json!({
                            "frame": s.frame,
                            "viewport": {
                                "width": s.viewport_width,
                                "height": s.viewport_height,
                                "pixels_per_point": s.pixels_per_point,
                            },
                            "tab": format!("{:?}", *app.current_tab),
                            "theme": format!("{:?}", app.state.config.theme.preset),
                            "elements": s.elements,
                        }),
                    );
                    let _ = reply_tx.send(resp);
                }
            }
            ActiveJob::State { req_id, reply_tx } => {
                let payload = build_state_json(ctx, app, self);
                let _ = reply_tx.send(HarnessResponse::ok(req_id, payload));
            }
            ActiveJob::Settle {
                req_id,
                frames_remaining,
                payload,
                reply_tx,
            } => {
                if frames_remaining > 0 {
                    self.active_job = Some(ActiveJob::Settle {
                        req_id,
                        frames_remaining: frames_remaining - 1,
                        payload,
                        reply_tx,
                    });
                    ctx.request_repaint();
                } else {
                    let _ = reply_tx.send(HarnessResponse::ok(req_id, payload));
                }
            }
            ActiveJob::InputSequence {
                req_id,
                initial_ui_hash,
                mut meta,
                reply_tx,
            } => {
                if !self.pending_raw_steps.is_empty() {
                    self.active_job = Some(ActiveJob::InputSequence {
                        req_id,
                        initial_ui_hash,
                        meta,
                        reply_tx,
                    });
                    ctx.request_repaint();
                } else {
                    if let Some(obj) = meta.as_object_mut() {
                        obj.insert(
                            "ui_changed".to_string(),
                            serde_json::Value::Bool(self.last_ui_hash != initial_ui_hash),
                        );
                        obj.insert(
                            "tab".to_string(),
                            serde_json::Value::String(format!("{:?}", *app.current_tab)),
                        );
                    }
                    let _ = reply_tx.send(HarnessResponse::ok(req_id, meta));
                }
            }
            ActiveJob::Resize {
                req_id,
                target,
                frames_waited,
                reply_tx,
            } => {
                let rect = ctx.screen_rect();
                let close_enough = (rect.width() - target.0).abs() <= 2.0
                    && (rect.height() - target.1).abs() <= 2.0;
                if !close_enough && frames_waited < 6 {
                    self.active_job = Some(ActiveJob::Resize {
                        req_id,
                        target,
                        frames_waited: frames_waited + 1,
                        reply_tx,
                    });
                    ctx.request_repaint();
                } else {
                    let _ = reply_tx.send(HarnessResponse::ok(
                        req_id,
                        serde_json::json!({
                            "width": rect.width(),
                            "height": rect.height(),
                        }),
                    ));
                }
            }
            ActiveJob::WaitIdle {
                req_id,
                started,
                timeout,
                required_frames,
                mut idle_frames,
                reply_tx,
            } => {
                let busy = app.state.active_phase.is_some()
                    || app.state.installing_instance.is_some()
                    || app.state.catalog.loading;
                if !busy {
                    idle_frames += 1;
                } else {
                    idle_frames = 0;
                }

                if idle_frames >= required_frames {
                    let _ = reply_tx.send(HarnessResponse::ok(
                        req_id,
                        serde_json::json!({
                            "waited_ms": started.elapsed().as_millis() as u64,
                            "state": build_state_json(ctx, app, self),
                        }),
                    ));
                } else if started.elapsed() >= timeout {
                    let _ = reply_tx.send(HarnessResponse::err(
                        req_id,
                        format!("wait_idle timed out after {} ms", timeout.as_millis()),
                    ));
                } else {
                    self.active_job = Some(ActiveJob::WaitIdle {
                        req_id,
                        started,
                        timeout,
                        required_frames,
                        idle_frames,
                        reply_tx,
                    });
                    ctx.request_repaint_after(Duration::from_millis(16));
                }
            }
            ActiveJob::WaitFor {
                req_id,
                spec,
                text,
                tab,
                require_visible,
                started,
                timeout,
                reply_tx,
            } => {
                let tab_ok = tab.is_none_or(|t| *app.current_tab == t);
                let elem_ok = if spec.id.is_some() || spec.label.is_some() || text.is_some() {
                    snap.as_ref().is_some_and(|s| {
                        s.elements.iter().any(|e| {
                            if require_visible && !e.visible {
                                return false;
                            }
                            let id_match = spec
                                .id
                                .as_ref()
                                .is_none_or(|id| &e.id == id || &e.egui_id == id);
                            let label_match = spec.label.as_ref().is_none_or(|l| {
                                e.label == *l || e.label.to_lowercase().contains(&l.to_lowercase())
                            });
                            let text_match = text.as_ref().is_none_or(|t| {
                                e.text.to_lowercase().contains(&t.to_lowercase())
                                    || e.label.to_lowercase().contains(&t.to_lowercase())
                            });
                            id_match && label_match && text_match
                        })
                    })
                } else {
                    true
                };

                if tab_ok && elem_ok {
                    let _ = reply_tx.send(HarnessResponse::ok(
                        req_id,
                        serde_json::json!({
                            "waited_ms": started.elapsed().as_millis() as u64,
                            "tab": format!("{:?}", *app.current_tab),
                        }),
                    ));
                } else if started.elapsed() >= timeout {
                    let _ = reply_tx.send(HarnessResponse::err(
                        req_id,
                        format!("wait_for timed out after {} ms", timeout.as_millis()),
                    ));
                } else {
                    self.active_job = Some(ActiveJob::WaitFor {
                        req_id,
                        spec,
                        text,
                        tab,
                        require_visible,
                        started,
                        timeout,
                        reply_tx,
                    });
                    ctx.request_repaint_after(Duration::from_millis(16));
                }
            }
        }
    }

    fn dispatch_request(
        &mut self,
        ctx: &Context,
        app: &mut TestAppAccess<'_>,
        queued: QueuedRequest,
    ) {
        let RequestEnvelope { req_id, cmd } = queued.req;
        let reply_tx = queued.reply_tx;

        match cmd {
            HarnessCommand::Quit => {
                let _ = reply_tx.send(HarnessResponse::ok(
                    req_id,
                    serde_json::json!({ "quitting": true }),
                ));
                if let Some(srv) = self.server.take() {
                    srv.cleanup();
                }
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            HarnessCommand::State => {
                self.active_job = Some(ActiveJob::State { req_id, reply_tx });
                ctx.request_repaint();
            }
            HarnessCommand::DumpUi => {
                self.active_job = Some(ActiveJob::DumpUi {
                    req_id,
                    frames_remaining: 1,
                    reply_tx,
                });
                ctx.request_repaint();
            }
            HarnessCommand::Screenshot { path } => {
                self.pending_screenshot = Some(PendingScreenshot {
                    req_id,
                    path,
                    region: None,
                    requested_at: Instant::now(),
                    reply_tx,
                });
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
                ctx.request_repaint();
            }
            HarnessCommand::ScreenshotRegion {
                path,
                x,
                y,
                w,
                h,
                logical,
            } => {
                self.pending_screenshot = Some(PendingScreenshot {
                    req_id,
                    path,
                    region: Some((x, y, w, h, logical)),
                    requested_at: Instant::now(),
                    reply_tx,
                });
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
                ctx.request_repaint();
            }
            HarnessCommand::FrameStats { action, op, reset } => {
                let mode = action
                    .or(op)
                    .unwrap_or_else(|| if reset { "reset" } else { "get" }.to_string());
                if mode.eq_ignore_ascii_case("reset") {
                    self.stats.reset();
                    let _ = reply_tx.send(HarnessResponse::ok(
                        req_id,
                        serde_json::json!({ "reset": true }),
                    ));
                } else {
                    let _ = reply_tx.send(HarnessResponse::ok(req_id, self.stats.to_json()));
                }
            }
            HarnessCommand::Resize { width, height } => {
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(
                    width.max(320.0),
                    height.max(240.0),
                )));
                self.active_job = Some(ActiveJob::Resize {
                    req_id,
                    target: (width, height),
                    frames_waited: 0,
                    reply_tx,
                });
                ctx.request_repaint();
            }
            HarnessCommand::SetScale { scale } => {
                let clamped = scale.clamp(0.5, 4.0);
                ctx.set_pixels_per_point(clamped);
                self.active_job = Some(ActiveJob::Settle {
                    req_id,
                    frames_remaining: 2,
                    payload: serde_json::json!({ "pixels_per_point": clamped }),
                    reply_tx,
                });
                ctx.request_repaint();
            }
            HarnessCommand::SetTheme { preset } => match parse_theme_preset(&preset) {
                Some(p) => {
                    app.state.config.theme.apply_preset(p);
                    if self.is_solid_background() {
                        app.state.config.theme.background_style = BackgroundStyle::Solid;
                    }
                    apply_theme(ctx, &app.state.config.theme);
                    self.active_job = Some(ActiveJob::Settle {
                        req_id,
                        frames_remaining: 2,
                        payload: serde_json::json!({ "preset": format!("{p:?}") }),
                        reply_tx,
                    });
                    ctx.request_repaint();
                }
                None => {
                    let _ = reply_tx.send(HarnessResponse::err(
                        req_id,
                        format!("Unknown theme preset: '{preset}'"),
                    ));
                }
            },
            HarnessCommand::Goto { tab, section } => match parse_nav_tab(&tab) {
                Some(t) => {
                    *app.current_tab = t;
                    if let Some(ref sec_name) = section {
                        if let Some(sec) = parse_settings_section(sec_name) {
                            *app.settings_section = Some(sec);
                        }
                    }
                    self.active_job = Some(ActiveJob::Settle {
                        req_id,
                        frames_remaining: 2,
                        payload: serde_json::json!({ "tab": format!("{t:?}") }),
                        reply_tx,
                    });
                    ctx.request_repaint();
                }
                None => {
                    let _ = reply_tx.send(HarnessResponse::err(
                        req_id,
                        format!("Unknown navigation tab: '{tab}'"),
                    ));
                }
            },
            HarnessCommand::SetFlag { name, value } => {
                match apply_flag(ctx, app, &mut self.flags, &name, &value) {
                    Ok(summary) => {
                        self.active_job = Some(ActiveJob::Settle {
                            req_id,
                            frames_remaining: 1,
                            payload: summary,
                            reply_tx,
                        });
                        ctx.request_repaint();
                    }
                    Err(e) => {
                        let _ = reply_tx.send(HarnessResponse::err(req_id, e));
                    }
                }
            }
            HarnessCommand::WaitIdle { timeout_ms, frames } => {
                self.active_job = Some(ActiveJob::WaitIdle {
                    req_id,
                    started: Instant::now(),
                    timeout: Duration::from_millis(timeout_ms),
                    required_frames: frames.max(1),
                    idle_frames: 0,
                    reply_tx,
                });
                ctx.request_repaint();
            }
            HarnessCommand::WaitFor {
                target,
                id,
                label,
                text,
                tab,
                visible,
                timeout_ms,
            } => {
                let spec = merge_target(target, id, label, None, None);
                let parsed_tab = match tab {
                    Some(ref t) => match parse_nav_tab(t) {
                        Some(nt) => Some(nt),
                        None => {
                            let _ = reply_tx.send(HarnessResponse::err(
                                req_id,
                                format!("Unknown tab in wait_for: '{t}'"),
                            ));
                            return;
                        }
                    },
                    None => None,
                };
                self.active_job = Some(ActiveJob::WaitFor {
                    req_id,
                    spec,
                    text,
                    tab: parsed_tab,
                    require_visible: visible,
                    started: Instant::now(),
                    timeout: Duration::from_millis(timeout_ms),
                    reply_tx,
                });
                ctx.request_repaint();
            }
            HarnessCommand::Click {
                target,
                id,
                label,
                x,
                y,
            } => {
                let spec = merge_target(target, id, label, x, y);
                self.start_pointer_cmd(
                    ctx,
                    req_id,
                    spec,
                    PointerButton::Primary,
                    false,
                    false,
                    reply_tx,
                );
            }
            HarnessCommand::DoubleClick {
                target,
                id,
                label,
                x,
                y,
            } => {
                let spec = merge_target(target, id, label, x, y);
                self.start_pointer_cmd(
                    ctx,
                    req_id,
                    spec,
                    PointerButton::Primary,
                    true,
                    false,
                    reply_tx,
                );
            }
            HarnessCommand::RightClick {
                target,
                id,
                label,
                x,
                y,
            } => {
                let spec = merge_target(target, id, label, x, y);
                self.start_pointer_cmd(
                    ctx,
                    req_id,
                    spec,
                    PointerButton::Secondary,
                    false,
                    false,
                    reply_tx,
                );
            }
            HarnessCommand::Hover {
                target,
                id,
                label,
                x,
                y,
            } => {
                let spec = merge_target(target, id, label, x, y);
                self.start_pointer_cmd(
                    ctx,
                    req_id,
                    spec,
                    PointerButton::Primary,
                    false,
                    true,
                    reply_tx,
                );
            }
            HarnessCommand::Type {
                text,
                target,
                id,
                label,
                x,
                y,
                clear,
            } => {
                let spec = merge_target(target, id, label, x, y);
                let has_target = spec.id.is_some()
                    || spec.label.is_some()
                    || (spec.x.is_some() && spec.y.is_some());
                let focus_pos = if has_target {
                    let snap = last_snapshot(ctx);
                    if snap.is_none() && (spec.id.is_some() || spec.label.is_some()) {
                        self.deferred_req = Some(QueuedRequest {
                            req: RequestEnvelope {
                                req_id,
                                cmd: HarnessCommand::Type {
                                    text,
                                    target: Some(spec),
                                    id: None,
                                    label: None,
                                    x: None,
                                    y: None,
                                    clear,
                                },
                            },
                            reply_tx,
                        });
                        ctx.request_repaint();
                        return;
                    }
                    match resolve_target(&spec, snap.as_ref()) {
                        Ok(r) => Some(r.pos),
                        Err(e) => {
                            let _ = reply_tx.send(HarnessResponse::err(req_id, e));
                            return;
                        }
                    }
                } else {
                    None
                };

                self.pending_raw_steps
                    .extend(build_type_steps(focus_pos, &text, clear));
                self.active_job = Some(ActiveJob::InputSequence {
                    req_id,
                    initial_ui_hash: self.last_ui_hash,
                    meta: serde_json::json!({ "typed_len": text.chars().count() }),
                    reply_tx,
                });
                ctx.request_repaint();
            }
            HarnessCommand::Key { key, modifiers } => {
                let Some(parsed_key) = parse_key(&key) else {
                    let _ = reply_tx.send(HarnessResponse::err(
                        req_id,
                        format!("Unknown key name: '{key}'"),
                    ));
                    return;
                };
                let mods = parse_modifiers(&modifiers);
                self.pending_raw_steps
                    .extend(build_key_steps(parsed_key, mods));
                self.active_job = Some(ActiveJob::InputSequence {
                    req_id,
                    initial_ui_hash: self.last_ui_hash,
                    meta: serde_json::json!({ "key": key }),
                    reply_tx,
                });
                ctx.request_repaint();
            }
            HarnessCommand::Scroll {
                dx,
                dy,
                target,
                id,
                label,
                x,
                y,
            } => {
                let spec = merge_target(target, id, label, x, y);
                let has_target = spec.id.is_some()
                    || spec.label.is_some()
                    || (spec.x.is_some() && spec.y.is_some());
                let pos = if has_target {
                    let snap = last_snapshot(ctx);
                    match resolve_target(&spec, snap.as_ref()) {
                        Ok(r) => Some(r.pos),
                        Err(e) => {
                            let _ = reply_tx.send(HarnessResponse::err(req_id, e));
                            return;
                        }
                    }
                } else {
                    None
                };
                self.pending_raw_steps
                    .extend(build_scroll_steps(pos, dx, dy));
                self.active_job = Some(ActiveJob::InputSequence {
                    req_id,
                    initial_ui_hash: self.last_ui_hash,
                    meta: serde_json::json!({ "dx": dx, "dy": dy }),
                    reply_tx,
                });
                ctx.request_repaint();
            }
            HarnessCommand::Drag {
                from,
                to,
                from_x,
                from_y,
                to_x,
                to_y,
                steps,
            } => {
                let snap = last_snapshot(ctx);
                let from_spec = merge_target(from, None, None, from_x, from_y);
                let to_spec = merge_target(to, None, None, to_x, to_y);
                let from_pos = match resolve_target(&from_spec, snap.as_ref()) {
                    Ok(r) => r.pos,
                    Err(e) => {
                        let _ =
                            reply_tx.send(HarnessResponse::err(req_id, format!("drag from: {e}")));
                        return;
                    }
                };
                let to_pos = match resolve_target(&to_spec, snap.as_ref()) {
                    Ok(r) => r.pos,
                    Err(e) => {
                        let _ =
                            reply_tx.send(HarnessResponse::err(req_id, format!("drag to: {e}")));
                        return;
                    }
                };
                self.pending_raw_steps
                    .extend(build_drag_steps(from_pos, to_pos, steps));
                self.active_job = Some(ActiveJob::InputSequence {
                    req_id,
                    initial_ui_hash: self.last_ui_hash,
                    meta: serde_json::json!({
                        "from": [from_pos.x, from_pos.y],
                        "to": [to_pos.x, to_pos.y],
                    }),
                    reply_tx,
                });
                ctx.request_repaint();
            }
            HarnessCommand::DropFile { path } => {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("dropped")
                    .to_string();
                self.pending_raw_steps.push_back(InputFrameStep {
                    events: Vec::new(),
                    dropped_files: vec![DroppedFile {
                        path: Some(path.clone()),
                        name,
                        mime: String::new(),
                        last_modified: None,
                        bytes: None,
                    }],
                });
                self.pending_raw_steps.push_back(InputFrameStep::default());
                self.active_job = Some(ActiveJob::InputSequence {
                    req_id,
                    initial_ui_hash: self.last_ui_hash,
                    meta: serde_json::json!({ "dropped_path": path }),
                    reply_tx,
                });
                ctx.request_repaint();
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_pointer_cmd(
        &mut self,
        ctx: &Context,
        req_id: Option<u64>,
        spec: TargetSpec,
        button: PointerButton,
        double: bool,
        hover_only: bool,
        reply_tx: mpsc::SyncSender<HarnessResponse>,
    ) {
        let snap = last_snapshot(ctx);
        if snap.is_none() && (spec.id.is_some() || spec.label.is_some()) {
            let cmd = if hover_only {
                HarnessCommand::Hover {
                    target: Some(spec),
                    id: None,
                    label: None,
                    x: None,
                    y: None,
                }
            } else if double {
                HarnessCommand::DoubleClick {
                    target: Some(spec),
                    id: None,
                    label: None,
                    x: None,
                    y: None,
                }
            } else if button == PointerButton::Secondary {
                HarnessCommand::RightClick {
                    target: Some(spec),
                    id: None,
                    label: None,
                    x: None,
                    y: None,
                }
            } else {
                HarnessCommand::Click {
                    target: Some(spec),
                    id: None,
                    label: None,
                    x: None,
                    y: None,
                }
            };
            self.deferred_req = Some(QueuedRequest {
                req: RequestEnvelope { req_id, cmd },
                reply_tx,
            });
            ctx.request_repaint();
            return;
        }

        match resolve_target(&spec, snap.as_ref()) {
            Ok(resolved) => {
                let steps = if hover_only {
                    build_hover_steps(resolved.pos)
                } else {
                    build_click_steps(resolved.pos, button, double)
                };
                self.pending_raw_steps.extend(steps);
                self.active_job = Some(ActiveJob::InputSequence {
                    req_id,
                    initial_ui_hash: self.last_ui_hash,
                    meta: serde_json::json!({
                        "x": resolved.pos.x,
                        "y": resolved.pos.y,
                        "matched_id": resolved.matched_id,
                        "matched_label": resolved.matched_label,
                        "was_active": resolved.was_active,
                    }),
                    reply_tx,
                });
                ctx.request_repaint();
            }
            Err(e) => {
                let _ = reply_tx.send(HarnessResponse::err(req_id, e));
            }
        }
    }
}

fn save_screenshot_png(
    img: &ColorImage,
    path: &Path,
    region: Option<(f32, f32, f32, f32, bool)>,
    pixels_per_point: f32,
) -> anyhow::Result<(u32, u32)> {
    let img_w = img.size[0];
    let img_h = img.size[1];
    if img_w == 0 || img_h == 0 {
        anyhow::bail!("Captured screenshot image is empty");
    }

    let (x0, y0, out_w, out_h) = match region {
        Some((rx, ry, rw, rh, logical)) => {
            let scale = if logical {
                pixels_per_point.max(0.5)
            } else {
                1.0
            };
            let px = ((rx * scale).round().max(0.0) as usize).min(img_w.saturating_sub(1));
            let py = ((ry * scale).round().max(0.0) as usize).min(img_h.saturating_sub(1));
            let pw = ((rw * scale).round().max(1.0) as usize).min(img_w - px);
            let ph = ((rh * scale).round().max(1.0) as usize).min(img_h - py);
            (px, py, pw, ph)
        }
        None => (0, 0, img_w, img_h),
    };

    let mut rgba = Vec::with_capacity(out_w * out_h * 4);
    for row in y0..(y0 + out_h) {
        let row_start = row * img_w + x0;
        for px in &img.pixels[row_start..(row_start + out_w)] {
            rgba.extend_from_slice(&px.to_array());
        }
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    image::save_buffer(
        path,
        &rgba,
        out_w as u32,
        out_h as u32,
        image::ExtendedColorType::Rgba8,
    )?;

    Ok((out_w as u32, out_h as u32))
}

fn build_state_json(
    ctx: &Context,
    app: &TestAppAccess<'_>,
    ctrl: &TestHooksController,
) -> serde_json::Value {
    let screen = ctx.screen_rect();
    let instances: Vec<serde_json::Value> = app
        .state
        .instances
        .iter()
        .map(|i| {
            serde_json::json!({
                "id": i.id.as_str(),
                "name": i.name,
                "mc_version": i.mc_version,
                "loader": i.loader.as_str(),
            })
        })
        .collect();

    serde_json::json!({
        "tab": format!("{:?}", *app.current_tab),
        "settings_section": app.settings_section.map(|s| format!("{s:?}")),
        "theme_preset": format!("{:?}", app.state.config.theme.preset),
        "locale": app.state.config.launcher_locale,
        "instances_count": app.state.instances.len(),
        "instances": instances,
        "selected_instance_id": app.state.selected_instance_id.as_ref().map(|i| i.as_str()),
        "installing_instance": app.state.installing_instance.as_ref().map(|i| i.as_str()),
        "active_phase": app.state.active_phase,
        "game_running": app.state.game_running,
        "accounts_count": app.state.accounts.list_accounts().len(),
        "active_account": app.state.accounts.active_account().map(|a| a.username.as_str()),
        "show_create_instance_dialog": app.state.show_create_instance_dialog,
        "status_message": app.state.status_message.as_ref().map(|(m, is_err)| serde_json::json!({"text": m, "is_error": is_err})),
        "watchdog_violations": app.state.watchdog.violation_count(),
        "frame_count": ctrl.frame_count,
        "action_seq": ctrl.action_seq,
        "ui_hash": ctrl.last_ui_hash,
        "pixels_per_point": ctx.pixels_per_point(),
        "viewport": {
            "width": screen.width(),
            "height": screen.height(),
        },
        "deterministic": ctrl.flags.deterministic,
        "disable_animations": ctrl.flags.disable_animations,
        "solid_background": ctrl.flags.solid_background,
        "paths": {
            "data_dir": app.state.paths.data_dir,
            "config_dir": app.state.paths.config_dir,
            "cache_dir": app.state.paths.cache_dir,
        }
    })
}

fn compute_ui_hash(app: &TestAppAccess<'_>, snap: Option<&UiRegistrySnapshot>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", *app.current_tab).hash(&mut h);
    app.settings_section.map(|s| format!("{s:?}")).hash(&mut h);
    format!("{:?}", app.state.config.theme.preset).hash(&mut h);
    app.state.config.launcher_locale.hash(&mut h);
    app.state
        .selected_instance_id
        .as_ref()
        .map(|i| i.as_str().to_string())
        .hash(&mut h);
    app.state.show_create_instance_dialog.hash(&mut h);
    app.instances_view.show_import_dialog.hash(&mut h);
    app.instances_view.show_duplicate_dialog.hash(&mut h);
    app.instances_view.search_query.hash(&mut h);
    app.catalog_view.curseforge_api_key_input.hash(&mut h);
    app.state.catalog.query.hash(&mut h);
    app.state.status_message.hash(&mut h);
    app.state.instances.len().hash(&mut h);
    if let Some(s) = snap {
        s.elements.len().hash(&mut h);
        for e in &s.elements {
            e.id.hash(&mut h);
            e.label.hash(&mut h);
            e.text.hash(&mut h);
            e.active.hash(&mut h);
            e.focused.hash(&mut h);
            e.enabled.hash(&mut h);
            e.visible.hash(&mut h);
        }
    }
    h.finish()
}

fn apply_flag(
    ctx: &Context,
    app: &mut TestAppAccess<'_>,
    flags: &mut TestFlags,
    name: &str,
    value: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let as_bool = || {
        value
            .as_bool()
            .ok_or_else(|| format!("Flag '{name}' expects boolean value"))
    };
    match name.trim().to_ascii_lowercase().as_str() {
        "deterministic" => {
            let b = as_bool()?;
            flags.deterministic = b;
            flags.disable_animations = b;
            flags.solid_background = b;
            if b {
                ctx.style_mut(|s| s.animation_time = 0.0);
                app.state.config.theme.background_style = BackgroundStyle::Solid;
            }
            Ok(serde_json::json!({ "deterministic": b }))
        }
        "disable_animations" => {
            let b = as_bool()?;
            flags.disable_animations = b;
            ctx.style_mut(|s| {
                s.animation_time = if b { 0.0 } else { 1.0 / 12.0 };
            });
            Ok(serde_json::json!({ "disable_animations": b }))
        }
        "disable_background" | "solid_background" => {
            let b = as_bool()?;
            flags.solid_background = b;
            if b {
                app.state.config.theme.background_style = BackgroundStyle::Solid;
            }
            Ok(serde_json::json!({ "solid_background": b }))
        }
        "locale" => {
            let loc = value
                .as_str()
                .ok_or_else(|| "Flag 'locale' expects string value".to_string())?;
            app.state.config.launcher_locale = loc.to_string();
            Ok(serde_json::json!({ "locale": loc }))
        }
        "sidebar_compact" => {
            let b = as_bool()?;
            let current = resolve_compact_mode(ctx.screen_rect().width(), None);
            let _ = current;
            ctx.data_mut(|d| {
                d.insert_temp(
                    egui::Id::new("mq_sidebar_collapse_state"),
                    crate::sidebar::SidebarCollapseState {
                        manual_compact: Some(b),
                    },
                );
            });
            Ok(serde_json::json!({ "sidebar_compact": b }))
        }
        "toggle_sidebar" => {
            toggle_sidebar_compact(ctx);
            Ok(serde_json::json!({ "toggled": true }))
        }
        "demo_instances" => {
            let b = as_bool()?;
            if b && app.state.instances.is_empty() {
                let mut m1 =
                    InstanceManifest::new("26.1 Vanilla", "26.1", ModLoaderKind::Vanilla, None);
                if let Ok(id) = mq_core::InstanceId::new("qa-vanilla") {
                    m1.id = id;
                }
                let mut m2 = InstanceManifest::new(
                    "Fabric QA",
                    "1.21.4",
                    ModLoaderKind::Fabric,
                    Some("0.16.9".to_string()),
                );
                if let Ok(id) = mq_core::InstanceId::new("qa-fabric") {
                    m2.id = id;
                }
                app.state.selected_instance_id = Some(m1.id.clone());
                app.state.instances = vec![m1, m2];
            } else if !b {
                app.state.instances.clear();
                app.state.selected_instance_id = None;
            }
            Ok(serde_json::json!({ "instances_count": app.state.instances.len() }))
        }
        other => Err(format!("Unknown flag: '{other}'")),
    }
}

fn parse_theme_preset(raw: &str) -> Option<ThemePreset> {
    let norm = raw.trim().to_ascii_lowercase().replace(['-', '_', ' '], "");
    match norm.as_str() {
        "mattedark" | "dark" | "default" => Some(ThemePreset::MatteDark),
        "prismclassicdark" | "classicdark" | "prism" => Some(ThemePreset::PrismClassicDark),
        "modrinthemerald" | "emerald" | "modrinth" => Some(ThemePreset::ModrinthEmerald),
        "catppuccinmocha" | "catppuccin" | "mocha" => Some(ThemePreset::CatppuccinMocha),
        "nord" => Some(ThemePreset::Nord),
        "lightclean" | "light" | "lightslate" => Some(ThemePreset::LightClean),
        _ => None,
    }
}

fn parse_nav_tab(raw: &str) -> Option<NavTab> {
    let norm = raw.trim().to_ascii_lowercase().replace(['-', '_', ' '], "");
    match norm.as_str() {
        "instances" | "инстансы" => Some(NavTab::Instances),
        "catalog" | "каталог" => Some(NavTab::Catalog),
        "downloads" | "загрузки" => Some(NavTab::Downloads),
        "performance" | "perf" | "производительность" => {
            Some(NavTab::Performance)
        }
        "accounts" | "аккаунты" => Some(NavTab::Accounts),
        "settings" | "настройки" => Some(NavTab::Settings),
        _ => None,
    }
}

fn parse_settings_section(raw: &str) -> Option<SettingsSection> {
    let norm = raw.trim().to_ascii_lowercase().replace(['-', '_', ' '], "");
    match norm.as_str() {
        "customization" | "кастомизация" | "оформление" => {
            Some(SettingsSection::Customization)
        }
        "launch" | "запуск" => Some(SettingsSection::Launch),
        "localization" | "language" | "язык" | "локализация" => {
            Some(SettingsSection::Localization)
        }
        "networkandapi" | "network" | "сеть" | "сетьиapi" => {
            Some(SettingsSection::NetworkAndApi)
        }
        "catalog" | "каталог" => Some(SettingsSection::Catalog),
        "updates" | "обновления" => Some(SettingsSection::Updates),
        "about" | "опрограмме" => Some(SettingsSection::About),
        _ => None,
    }
}

const CONTROLLER_KEY: &str = "mq_test_hooks_controller";

#[derive(Clone)]
struct ControllerHandle(Arc<Mutex<TestHooksController>>);

fn get_or_create_handle(ctx: &Context) -> ControllerHandle {
    ctx.data_mut(|d| {
        if let Some(h) = d.get_temp::<ControllerHandle>(egui::Id::new(CONTROLLER_KEY)) {
            h
        } else {
            let h = ControllerHandle(Arc::new(Mutex::new(TestHooksController::default())));
            d.insert_temp(egui::Id::new(CONTROLLER_KEY), h.clone());
            h
        }
    })
}

/// Initializes the `test-hooks` IPC server for the given `egui::Context`.
pub fn init_for_context(ctx: &Context, paths: &MqPaths) {
    let handle = get_or_create_handle(ctx);
    let mut guard = handle.0.lock();
    guard.start(ctx, paths);
}

/// Injects queued synthetic input events and captures `Event::Screenshot` before `ctx.run`.
pub fn on_raw_input(ctx: &Context, raw_input: &mut RawInput) {
    let handle = get_or_create_handle(ctx);
    let mut guard = handle.0.lock();
    guard.raw_input_hook(ctx, raw_input);
}

/// Runs pre-frame `test-hooks` command dispatch and deterministic styling.
pub fn on_pre_frame(ctx: &Context, app: &mut TestAppAccess<'_>) {
    let handle = get_or_create_handle(ctx);
    let mut guard = handle.0.lock();
    guard.pre_frame(ctx, app);
}

/// Runs post-frame `test-hooks` UI snapshot completion and frame timing accounting.
pub fn on_post_frame(ctx: &Context, app: &mut TestAppAccess<'_>) {
    let handle = get_or_create_handle(ctx);
    let mut guard = handle.0.lock();
    guard.post_frame(ctx, app);
}

/// Returns `true` when deterministic solid background override is active.
pub fn is_solid_background(ctx: &Context) -> bool {
    let handle = get_or_create_handle(ctx);
    let guard = handle.0.lock();
    guard.is_solid_background()
}
