#![deny(clippy::await_holding_lock)]
//! Main `eframe::App` implementation for MQ Launcher. Zero emojis!

use egui::{CentralPanel, SidePanel, TopBottomPanel};
use mq_core::GuiBackend;

use crate::accounts_view::AccountsView;
use crate::catalog_view::CatalogView;
use crate::downloads_view::{DownloadsAction, DownloadsView};
use crate::fonts::configure_system_fonts;
use crate::hud::{is_zero_viewport, render_bottom_hud, BottomHudData};
use crate::instances_view::{InstanceDetailTab, InstanceViewMode, InstancesView};
use crate::perf_view::{PerfAction, PerfView};
use crate::settings_view::SettingsView;
use crate::sidebar::{
    build_app_icon, render_sidebar_with_state, sidebar_width, NavTab, SidebarAction, SidebarState,
};
use crate::state::{
    DownloadHistoryStatus, StorageStats, UiState, UiWorkerCommand, UiWorkerResponse,
};
use crate::theme::{
    apply_theme, paint_launcher_background, solid_hud_frame_with_config,
    solid_sidebar_frame_with_config, Tokens,
};
use crate::theme_styles::ThemeStyleParams;

#[cfg(any(feature = "test-hooks", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestAction {
    Install {
        mc_version: String,
        loader: mq_core::ModLoaderKind,
    },
}

#[cfg(any(feature = "test-hooks", test))]
pub fn parse_test_action(raw: &str) -> Option<TestAction> {
    let trimmed = raw.trim();
    let rest = trimmed.strip_prefix("install:")?;
    let mut parts = rest.split(':');
    let mc_version = parts.next()?.trim();
    if mc_version.is_empty() {
        return None;
    }
    let loader = match parts.next() {
        Some(ldr_str) if !ldr_str.trim().is_empty() => {
            ldr_str.trim().parse::<mq_core::ModLoaderKind>().ok()?
        }
        _ => mq_core::ModLoaderKind::Vanilla,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(TestAction::Install {
        mc_version: mc_version.to_string(),
        loader,
    })
}

#[cfg(feature = "test-hooks")]
#[derive(Debug, Clone, Default)]
struct TestHookRuntimeState {
    initialized: bool,
    action_active: bool,
    started_at: Option<std::time::Instant>,
    cancel_after_ms: Option<u64>,
    cancel_sent: bool,
    exit_on_finish: bool,
    finish_frames: u32,
}

#[inline]
fn configured_mock_base_url() -> Option<String> {
    #[cfg(feature = "test-hooks")]
    {
        std::env::var("MQ_TEST_MOCK_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }
    #[cfg(not(feature = "test-hooks"))]
    {
        None
    }
}

pub struct MqApp {
    pub state: UiState,
    pub current_tab: NavTab,
    pub instances_view: InstancesView,
    pub catalog_view: CatalogView,
    pub downloads_view: DownloadsView,
    pub accounts_view: AccountsView,
    pub perf_view: PerfView,
    pub settings_view: SettingsView,
    pub recovery_journal: Option<mq_core::crash::InstallJournal>,
    pub frame_crash_message: Option<String>,
    pub discord_rpc: Option<mq_launch::DiscordRpcClient>,
    pub window_hidden_for_game: bool,
}

impl Drop for MqApp {
    fn drop(&mut self) {
        if let Some(ref stop) = self.state.telemetry_stop {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

fn dir_size_and_files(root: &std::path::Path) -> (u64, usize) {
    if !root.exists() {
        return (0, 0);
    }
    let mut total_bytes = 0u64;
    let mut total_files = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                total_files = total_files.saturating_add(1);
                if let Ok(meta) = entry.metadata() {
                    total_bytes = total_bytes.saturating_add(meta.len());
                }
            }
        }
    }
    (total_bytes, total_files)
}

fn scan_storage_stats(paths: &mq_core::MqPaths) -> StorageStats {
    let (cas_bytes, cas_files) = dir_size_and_files(&paths.cas_dir());
    let (instances_bytes, _) = dir_size_and_files(&paths.instances_dir());
    let assets_libs_bytes = dir_size_and_files(&paths.assets_dir())
        .0
        .saturating_add(dir_size_and_files(&paths.libraries_dir()).0)
        .saturating_add(dir_size_and_files(&paths.versions_dir()).0);
    let (java_bytes, _) = dir_size_and_files(&paths.java_dir());

    let mut temp_dirs: Vec<std::path::PathBuf> = Vec::new();
    for candidate in [
        paths.temp_dir(),
        paths.cache_dir.clone(),
        paths.data_dir.join("cache"),
        paths.logs_dir(),
    ] {
        let norm = std::fs::canonicalize(&candidate).unwrap_or(candidate);
        if !temp_dirs.contains(&norm) {
            temp_dirs.push(norm);
        }
    }
    let temp_logs_bytes = temp_dirs
        .iter()
        .map(|p| dir_size_and_files(p).0)
        .fold(0u64, u64::saturating_add);

    let total_bytes = cas_bytes
        .saturating_add(instances_bytes)
        .saturating_add(assets_libs_bytes)
        .saturating_add(java_bytes)
        .saturating_add(temp_logs_bytes);

    StorageStats {
        total_bytes,
        cas_bytes,
        cas_files,
        instances_bytes,
        assets_libs_bytes,
        java_bytes,
        temp_logs_bytes,
        scanned_at: chrono::Local::now().format("%H:%M:%S").to_string(),
    }
}

fn clean_dir_tree(
    root: &std::path::Path,
    preserve_names: &[&str],
    only_temp_artifacts: bool,
) -> (usize, u64) {
    if !root.is_dir() {
        return (0, 0);
    }
    let mut removed_files = 0usize;
    let mut freed_bytes = 0u64;
    let mut dirs_to_prune = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() {
                continue;
            }
            let path = entry.path();
            if ft.is_dir() {
                stack.push(path.clone());
                if !only_temp_artifacts {
                    dirs_to_prune.push(path);
                }
            } else if ft.is_file() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if preserve_names.iter().any(|p| *p == name_str.as_ref()) {
                    continue;
                }
                let should_remove = if only_temp_artifacts {
                    name_str.ends_with(".part")
                        || name_str.contains(".tmp.")
                        || name_str.contains(".tmp_")
                } else {
                    true
                };
                if !should_remove {
                    continue;
                }
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                if std::fs::remove_file(&path).is_ok() {
                    removed_files = removed_files.saturating_add(1);
                    freed_bytes = freed_bytes.saturating_add(size);
                }
            }
        }
    }

    for dir in dirs_to_prune.into_iter().rev() {
        let _ = std::fs::remove_dir(&dir);
    }

    (removed_files, freed_bytes)
}

fn clean_temp_cache(paths: &mq_core::MqPaths, skip_active_temp: bool) -> (usize, u64) {
    let mut removed = 0usize;
    let mut freed = 0u64;

    if !skip_active_temp {
        let (r, f) = clean_dir_tree(&paths.temp_dir(), &[], false);
        removed = removed.saturating_add(r);
        freed = freed.saturating_add(f);
    }

    let cache_norm =
        std::fs::canonicalize(&paths.cache_dir).unwrap_or_else(|_| paths.cache_dir.clone());
    let data_cache = paths.data_dir.join("cache");
    let data_cache_norm = std::fs::canonicalize(&data_cache).unwrap_or(data_cache);

    let (r, f) = clean_dir_tree(&paths.cache_dir, &["authlib-injector.jar"], false);
    removed = removed.saturating_add(r);
    freed = freed.saturating_add(f);

    if data_cache_norm != cache_norm {
        let (r, f) = clean_dir_tree(&data_cache_norm, &["authlib-injector.jar"], true);
        removed = removed.saturating_add(r);
        freed = freed.saturating_add(f);
    }

    if !skip_active_temp {
        for root in [
            paths.versions_dir(),
            paths.libraries_dir(),
            paths.assets_dir(),
        ] {
            let (r, f) = clean_dir_tree(&root, &[], true);
            removed = removed.saturating_add(r);
            freed = freed.saturating_add(f);
        }
    }

    let (r, f) = clean_dir_tree(&paths.config_dir, &[], true);
    removed = removed.saturating_add(r);
    freed = freed.saturating_add(f);

    (removed, freed)
}

/// Spawns a background worker future and monitors it for panics or crashes.
///
/// If the task panics or aborts abnormally:
/// 1. Formats and persists a crash report via [`mq_core::crash::write_crash_report_for_panic`].
/// 2. Converts the card state to [`UiWorkerResponse::InstanceError`] with localized title
///    «Установка прервана из-за внутренней ошибки».
fn spawn_monitored_task<F>(
    task_name: &'static str,
    paths: mq_core::MqPaths,
    instance_id: mq_core::InstanceId,
    resp_tx: flume::Sender<UiWorkerResponse>,
    fut: F,
) where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let inst_id = instance_id.clone();
    let handle = tokio::spawn(async move {
        fut.await;
    });

    tokio::spawn(async move {
        if let Err(join_err) = handle.await {
            let panic_msg = if join_err.is_panic() {
                let panic_obj = join_err.into_panic();
                if let Some(s) = panic_obj.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = panic_obj.downcast_ref::<String>() {
                    s.clone()
                } else {
                    format!("Паника в фоновом потоке {task_name}")
                }
            } else {
                format!("Фоновая задача {task_name} отменена: {join_err}")
            };

            let backtrace = std::backtrace::Backtrace::capture().to_string();
            let _ = mq_core::crash::write_crash_report_for_panic(
                &paths, &panic_msg, None, task_name, &backtrace,
            );

            let error_text = format!("Установка прервана из-за внутренней ошибки: {panic_msg}");
            let _ = resp_tx.send(UiWorkerResponse::InstanceError {
                instance_id: inst_id,
                error: error_text,
            });
        }
    });
}

impl MqApp {
    pub fn new(cc: &eframe::CreationContext<'_>, mut state: UiState) -> Self {
        apply_theme(&cc.egui_ctx, &state.config.theme);
        configure_system_fonts(&cc.egui_ctx);
        crate::theme::init_wgpu_liquid_glass(cc);
        #[cfg(feature = "test-hooks")]
        crate::test_hooks::init_for_context(&cc.egui_ctx, &state.paths);

        state.gpu_backend_label = if let Some(rs) = cc.wgpu_render_state.as_ref() {
            let info = rs.adapter.get_info();
            if info.name.is_empty() {
                format!("wgpu ({})", info.backend)
            } else {
                format!("wgpu ({} · {})", info.backend, info.name)
            }
        } else if cc.gl.is_some() {
            "glow (OpenGL)".to_string()
        } else {
            "Software".to_string()
        };

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        state.telemetry_stop = Some(std::sync::Arc::clone(&stop));
        let event_bus = state.event_bus.clone();
        let resp_tx = state.resp_sender.clone();
        let egui_ctx = cc.egui_ctx.clone();
        let running_pids = state.running_pids.clone();
        let _ = std::thread::Builder::new()
            .name("mq-telemetry".to_string())
            .spawn(move || {
                let mut sampler = mq_sched::telemetry::CpuSampler::new().ok();
                let mut first = true;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let sleep_ms = if first { 200 } else { 1000 };
                    first = false;
                    for _ in 0..(sleep_ms / 100) {
                        if stop.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    if stop.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }

                    let mut peak_rss_mib = mq_sched::telemetry::read_peak_rss_mib();
                    if let Some(sample) = sampler.as_mut().and_then(|s| s.sample().ok()) {
                        peak_rss_mib = peak_rss_mib.max(sample.peak_rss_mib);
                        let snap = mq_core::CpuSnapshot {
                            total_pct: sample.total_pct,
                            per_core: sample
                                .per_core_pct
                                .into_iter()
                                .enumerate()
                                .map(|(core_id, usage_pct)| mq_core::CoreCpuLoad {
                                    core_id,
                                    usage_pct,
                                })
                                .collect(),
                            rss_mib: sample.rss_mib,
                        };
                        event_bus.publish(mq_core::Event::CpuTelemetry(snap));
                    } else {
                        let rss_mib = mq_sched::telemetry::read_current_rss_mib();
                        peak_rss_mib = peak_rss_mib.max(rss_mib);
                        event_bus.publish(mq_core::Event::CpuTelemetry(mq_core::CpuSnapshot {
                            total_pct: 0.0,
                            per_core: Vec::new(),
                            rss_mib,
                        }));
                    }
                    let open_fds = mq_sched::telemetry::count_open_fds();
                    let _ = resp_tx.try_send(UiWorkerResponse::TelemetryExtra {
                        peak_rss_mib,
                        open_fds,
                    });

                    // Sample live RSS for all active game processes
                    let pids: Vec<(mq_core::InstanceId, u32)> = {
                        let map = running_pids.lock();
                        map.iter().map(|(id, &pid)| (id.clone(), pid)).collect()
                    };
                    if !pids.is_empty() {
                        let mut samples = Vec::new();
                        for (id, pid) in pids {
                            if let Some(rss) = mq_sched::read_process_rss_mib(pid) {
                                samples.push((id, rss as f32));
                            }
                        }
                        if !samples.is_empty() {
                            let _ = resp_tx.try_send(UiWorkerResponse::GameRssSamples(samples));
                        }
                    }

                    egui_ctx.request_repaint();
                }
            });

        Self::from_state(state)
    }

    pub fn from_state(mut state: UiState) -> Self {
        let recovery_journal = mq_core::crash::InstallJournal::load_unclosed(&state.paths);
        let mut current_tab = NavTab::Instances;
        if let Some(path) = state.pending_mrpack_path.take() {
            state.import_local_pack(path);
            current_tab = NavTab::Catalog;
        }
        Self {
            state,
            current_tab,
            instances_view: InstancesView::default(),
            catalog_view: CatalogView::default(),
            downloads_view: DownloadsView,
            accounts_view: AccountsView::default(),
            perf_view: PerfView,
            settings_view: SettingsView::default(),
            recovery_journal,
            frame_crash_message: None,
            discord_rpc: None,
            window_hidden_for_game: false,
        }
    }

    fn launch_instance_core(
        &mut self,
        instance_id: mq_core::InstanceId,
        dry_run: bool,
        quick_play: Option<mq_launch::QuickPlayTarget>,
    ) {
        let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
        let paths = self.state.paths.clone();
        let bus = self.state.event_bus.clone();
        let net = self.state.net.clone();
        let cas = self.state.cas.clone();
        let pool = self.state.compute_pool.clone();
        let guard = self.state.cancel_epoch.snapshot();
        let resp_tx = self.state.resp_sender.clone();
        let default_mc_loc = self.state.config.default_mc_locale.clone();

        if let Some(inst) = self
            .state
            .instances
            .iter()
            .find(|i| i.id == instance_id)
            .cloned()
        {
            if self.state.config.auto_backup_worlds {
                let _ = mq_instances::auto_backup_instance_worlds(
                    &inst_dir,
                    self.state.config.max_world_backups,
                );
            }

            self.state
                .game_session_starts
                .insert(instance_id.clone(), std::time::Instant::now());

            if self.state.config.discord_rpc_enabled {
                let inst_name = inst.name.clone();
                let inst_mc = inst.mc_version.clone();
                let rpc_resp_tx = self.state.resp_sender.clone();
                std::thread::spawn(move || {
                    if let Some(mut rpc) = mq_launch::DiscordRpcClient::connect_default() {
                        rpc.set_playing_activity(
                            &inst_name,
                            &format!("Minecraft {inst_mc}"),
                            chrono::Utc::now().timestamp() as u64,
                        );
                        let _ = rpc_resp_tx.send(UiWorkerResponse::DiscordRpcConnected(rpc));
                    }
                });
            }

            if self.state.config.mini_overlay_on_launch {
                self.state.mini_overlay_active = true;
            }

            let active_acc = self
                .state
                .accounts
                .active_account()
                .cloned()
                .unwrap_or_else(|| mq_auth::create_offline_account("Player").unwrap());
            let account_secrets: Vec<String> = self
                .state
                .accounts
                .collect_secret_strings()
                .into_iter()
                .map(String::from)
                .collect();

            spawn_monitored_task(
                "launch_instance",
                paths.clone(),
                instance_id.clone(),
                resp_tx.clone(),
                async move {
                    // 1. Check if installation is needed
                    let resolved_path = inst_dir.join("resolved_version.json");
                    let client_jar_path = inst_dir.join("client.jar");
                    let is_installed = resolved_path.exists() && client_jar_path.exists();

                    if !is_installed {
                        let _ = resp_tx.send(UiWorkerResponse::InstanceInstallStarted(
                            instance_id.clone(),
                        ));
                        let req = mq_install::InstallRequest {
                            mc_version: inst.mc_version.clone(),
                            loader: inst.loader,
                            loader_version: inst.loader_version.clone(),
                            instance_dir: inst_dir.clone(),
                            mock_base_url: configured_mock_base_url(),
                        };

                        match mq_install::install_game(
                            &req,
                            &paths,
                            &net,
                            &cas,
                            &pool,
                            &guard,
                            Some(&bus),
                        )
                        .await
                        {
                            Ok(summary) => {
                                bus.publish(mq_core::Event::PhaseCompleted {
                                    phase: "install_game".to_string(),
                                    elapsed_ms: summary.elapsed_ms,
                                });
                                let _ = resp_tx.send(UiWorkerResponse::InstanceInstallCompleted(
                                    instance_id.clone(),
                                ));
                            }
                            Err(e) => {
                                if guard.is_cancelled() {
                                    return;
                                }
                                let err_msg =
                                    format!("Ошибка скачивания инстанса {}: {e}", inst.name);
                                bus.publish(mq_core::Event::TaskError {
                                    context: format!("Установка {}", inst.name),
                                    message: e.to_string(),
                                });
                                let _ = resp_tx.send(UiWorkerResponse::InstanceError {
                                    instance_id,
                                    error: err_msg,
                                });
                                return;
                            }
                        }
                    }

                    // 2. Read resolved version metadata
                    let bytes = match std::fs::read(&resolved_path) {
                        Ok(b) => b,
                        Err(e) => {
                            let err_msg =
                                format!("Не удалось прочитать resolved_version.json: {e}");
                            let _ = resp_tx.send(UiWorkerResponse::InstanceError {
                                instance_id,
                                error: err_msg,
                            });
                            return;
                        }
                    };

                    let ver = match serde_json::from_slice::<mq_meta::ResolvedVersion>(&bytes) {
                        Ok(v) => v,
                        Err(e) => {
                            let err_msg = format!("Ошибка структуры resolved_version.json: {e}");
                            let _ = resp_tx.send(UiWorkerResponse::InstanceError {
                                instance_id,
                                error: err_msg,
                            });
                            return;
                        }
                    };

                    // 3. Ensure Ely.by authlib injector if Ely.by account is active
                    let authlib_path = if active_acc.kind == mq_auth::AccountKind::ElyBy {
                        let client = mq_auth::ElyByClient::new();
                        match client.ensure_authlib_injector(&paths).await {
                            Ok(p) => Some(p),
                            Err(e) => {
                                bus.publish(mq_core::Event::TaskError {
                                    context: "Ely.by Authlib".to_string(),
                                    message: format!("Не удалось загрузить скин-инжектор: {e}"),
                                });
                                None
                            }
                        }
                    } else {
                        None
                    };

                    // 4. Discover and select compatible Java Virtual Machine
                    let java_runtimes = mq_launch::discover_java_runtimes();
                    let req_major = mq_launch::required_java_major_for_mc(
                        &ver.mc_version,
                        Some(ver.java_version.major_version),
                    );

                    let selected_java = match mq_launch::select_java_runtime(
                        &java_runtimes,
                        req_major,
                        inst.java_path.as_deref(),
                    ) {
                        Ok(j) => j,
                        Err(e) => {
                            let err_msg = format!(
                            "Подходящая версия Java не найдена (требуется Java {req_major}): {e}"
                        );
                            bus.publish(mq_core::Event::TaskError {
                                context: "Поиск Java".to_string(),
                                message: err_msg.clone(),
                            });
                            let _ = resp_tx.send(UiWorkerResponse::InstanceError {
                                instance_id,
                                error: err_msg,
                            });
                            return;
                        }
                    };

                    // 5. Construct launch context and arguments
                    let launch_ctx = mq_launch::LaunchContext {
                        paths,
                        instance: inst.clone(),
                        version: ver,
                        account: active_acc,
                        java_bin: selected_java.path.clone(),
                        authlib_injector_jar: authlib_path,
                        extra_secrets_to_redact: Vec::new(),
                    };

                    let built_cmd = match mq_launch::build_launch_command_with_quick_play(
                        &launch_ctx,
                        quick_play.as_ref(),
                    ) {
                        Ok(cmd) => cmd,
                        Err(e) => {
                            let err_msg = format!("Ошибка построения аргументов JVM: {e}");
                            bus.publish(mq_core::Event::TaskError {
                                context: "Запуск JVM".to_string(),
                                message: err_msg.clone(),
                            });
                            let _ = resp_tx.send(UiWorkerResponse::InstanceError {
                                instance_id,
                                error: err_msg,
                            });
                            return;
                        }
                    };

                    let mut all_secrets: Vec<String> = launch_ctx.collect_secrets();
                    for s in account_secrets {
                        if !s.is_empty() && !all_secrets.contains(&s) {
                            all_secrets.push(s);
                        }
                    }
                    let secret_refs: Vec<&str> = all_secrets.iter().map(String::as_str).collect();

                    if dry_run {
                        bus.publish(mq_core::Event::GameLog(mq_core::GameLogEntry {
                            instance: instance_id.to_string(),
                            timestamp_ms: chrono::Utc::now().timestamp_millis(),
                            level: mq_core::GameLogLevel::Info,
                            line: format!(
                                "DRY RUN COMMAND:\n{}",
                                built_cmd.to_redacted_command_line(&secret_refs)
                            ),
                        }));
                    } else {
                        let _ = resp_tx.send(UiWorkerResponse::GameStarted(instance_id.clone()));

                        // Ensure valid assets/indexes/<id>.json and lang assets before game starts
                        let _ = mq_launch::ensure_prelaunch_assets(
                            &launch_ctx.paths,
                            &inst_dir,
                            &launch_ctx.version,
                        )
                        .await;

                        // Auto-synchronize options.txt language
                        let target_mc_lang =
                            inst.mc_locale.as_deref().unwrap_or(default_mc_loc.as_str());
                        let _ = mq_launch::sync_minecraft_options_lang(&inst_dir, target_mc_lang);

                        bus.publish(mq_core::Event::GameLog(mq_core::GameLogEntry {
                            instance: instance_id.to_string(),
                            timestamp_ms: chrono::Utc::now().timestamp_millis(),
                            level: mq_core::GameLogLevel::Info,
                            line: format!(
                                "[MQ] Запуск игры '{}' (MC {}, Java {}, Язык: {})...",
                                inst.name,
                                inst.mc_version,
                                selected_java.full_version,
                                target_mc_lang
                            ),
                        }));

                        let (pid_tx, pid_rx) = tokio::sync::oneshot::channel::<u32>();
                        let pid_resp_tx = resp_tx.clone();
                        let pid_inst = instance_id.clone();
                        tokio::spawn(async move {
                            if let Ok(pid) = pid_rx.await {
                                let _ = pid_resp_tx.send(UiWorkerResponse::GamePid {
                                    instance_id: pid_inst,
                                    pid,
                                });
                            }
                        });

                        let exit_code = match mq_launch::spawn_and_stream_game_with_pid(
                            &built_cmd,
                            instance_id.as_str(),
                            all_secrets,
                            Some(bus.clone()),
                            Some(pid_tx),
                        )
                        .await
                        {
                            Ok(status) => mq_launch::exit_status_code(&status),
                            Err(e) => {
                                bus.publish(mq_core::Event::TaskError {
                                    context: "Процесс игры".to_string(),
                                    message: format!("Ошибка процесса: {e}"),
                                });
                                -1
                            }
                        };

                        let _ = resp_tx.send(UiWorkerResponse::GameExited {
                            instance_id,
                            exit_code,
                        });
                    }
                },
            );
        }
    }

    fn handle_worker_commands(&mut self) {
        while let Ok(cmd) = self.state.cmd_receiver.try_recv() {
            match cmd {
                UiWorkerCommand::FetchVersionManifest => {
                    let resp_tx = self.state.resp_sender.clone();
                    let bus = self.state.event_bus.clone();
                    tokio::spawn(async move {
                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(12))
                            .build()
                            .unwrap_or_default();
                        match mq_meta::fetch_version_manifest(&client, None).await {
                            Ok(mut manifest) => {
                                manifest.sort_by_release_time_desc();
                                let latest_release = Some(manifest.latest.release);
                                let versions = manifest
                                    .versions
                                    .into_iter()
                                    .map(|v| (v.id, v.release_type))
                                    .collect();
                                let _ = resp_tx.send(UiWorkerResponse::VersionManifestFetched {
                                    latest_release,
                                    versions,
                                });
                                bus.publish(mq_core::Event::PhaseCompleted {
                                    phase: "manifest_fetch".to_string(),
                                    elapsed_ms: 0,
                                });
                            }
                            Err(_) => {
                                let _ = resp_tx.send(UiWorkerResponse::VersionManifestFetched {
                                    latest_release: None,
                                    versions: Vec::new(),
                                });
                            }
                        }
                    });
                }
                UiWorkerCommand::RefreshInstances => {
                    if let Ok(instances) = self.state.instance_mgr.list_instances() {
                        self.state.instances = instances;
                    }
                }
                UiWorkerCommand::RefreshAccounts => {
                    if let Ok(accs) = mq_auth::AccountStore::load_or_default(&self.state.paths) {
                        self.state.accounts = accs;
                    }
                }
                UiWorkerCommand::ScanInstanceMods { instance_id } => {
                    let mods_dir = self
                        .state
                        .paths
                        .instance_dir(instance_id.as_str())
                        .join("mods");
                    let pool = self.state.compute_pool.clone();
                    let guard = self.state.cancel_epoch.snapshot();
                    let bus = self.state.event_bus.clone();
                    let resp_tx = self.state.resp_sender.clone();

                    tokio::spawn(async move {
                        if let Ok(rep) =
                            mq_mods::scan_mods_directory(&mods_dir, &pool, &guard).await
                        {
                            let _ = resp_tx.send(UiWorkerResponse::ScanModsResults {
                                mods: rep.mods,
                                conflicts: rep.conflicts,
                            });
                            bus.publish(mq_core::Event::PhaseCompleted {
                                phase: "mods_scan".to_string(),
                                elapsed_ms: rep.scan_duration_ms,
                            });
                        }
                    });
                }
                UiWorkerCommand::CleanupUnusedDeps { instance_id } => {
                    let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
                    let mods_dir = inst_dir.join("mods");
                    let pool = self.state.compute_pool.clone();
                    let guard = self.state.cancel_epoch.snapshot();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let result = tokio::task::spawn_blocking({
                            let inst_dir = inst_dir.clone();
                            let mods_dir = mods_dir.clone();
                            move || -> Result<usize, String> {
                                let mut lock = mq_mods::ModLockFile::load_for_instance(&inst_dir);
                                let mut removed = 0usize;
                                for key in lock.find_unused_dependencies() {
                                    let Some(filename) =
                                        lock.entries.get(&key).map(|e| e.filename.clone())
                                    else {
                                        continue;
                                    };
                                    for path in [
                                        mods_dir.join(&filename),
                                        mods_dir.join(format!("{filename}.disabled")),
                                    ] {
                                        if path.exists() {
                                            let _ = std::fs::remove_file(&path);
                                        }
                                    }
                                    if lock.remove_entry(&key).is_some() {
                                        removed += 1;
                                    }
                                }
                                if removed > 0 {
                                    lock.save_for_instance(&inst_dir)
                                        .map_err(|e| e.to_string())?;
                                }
                                Ok(removed)
                            }
                        })
                        .await;
                        let (removed, error) = match result {
                            Ok(Ok(n)) => (n, None),
                            Ok(Err(e)) => (0, Some(e)),
                            Err(e) => (0, Some(e.to_string())),
                        };

                        if error.is_none() {
                            if let Ok(rep) =
                                mq_mods::scan_mods_directory(&mods_dir, &pool, &guard).await
                            {
                                let _ = resp_tx.send(UiWorkerResponse::ScanModsResults {
                                    mods: rep.mods,
                                    conflicts: rep.conflicts,
                                });
                            }
                        }
                        let _ =
                            resp_tx.send(UiWorkerResponse::UnusedDepsCleaned { removed, error });
                    });
                }
                UiWorkerCommand::ToggleMod {
                    instance_id,
                    file_name,
                } => {
                    let mods_dir = self
                        .state
                        .paths
                        .instance_dir(instance_id.as_str())
                        .join("mods");
                    let pool = self.state.compute_pool.clone();
                    let guard = self.state.cancel_epoch.snapshot();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let _ = mq_mods::toggle_mod(&mods_dir, &file_name);
                        if let Ok(rep) =
                            mq_mods::scan_mods_directory(&mods_dir, &pool, &guard).await
                        {
                            let _ = resp_tx.send(UiWorkerResponse::ScanModsResults {
                                mods: rep.mods,
                                conflicts: rep.conflicts,
                            });
                        }
                    });
                }
                UiWorkerCommand::DeleteMod {
                    instance_id,
                    file_name,
                } => {
                    let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
                    let mods_dir = inst_dir.join("mods");
                    let pool = self.state.compute_pool.clone();
                    let guard = self.state.cancel_epoch.snapshot();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        if let Ok(orphans) =
                            mq_mods::delete_mod_and_find_orphans(&inst_dir, &file_name)
                        {
                            if !orphans.is_empty() {
                                let _ = resp_tx.send(UiWorkerResponse::OrphanedDepsFound {
                                    instance_id: instance_id.clone(),
                                    deleted_file: file_name.clone(),
                                    orphans,
                                });
                            }
                        }
                        if let Ok(rep) =
                            mq_mods::scan_mods_directory(&mods_dir, &pool, &guard).await
                        {
                            let _ = resp_tx.send(UiWorkerResponse::ScanModsResults {
                                mods: rep.mods,
                                conflicts: rep.conflicts,
                            });
                        }
                    });
                }
                UiWorkerCommand::CleanupOrphanFiles {
                    instance_id,
                    orphan_files,
                } => {
                    let mods_dir = self
                        .state
                        .paths
                        .instance_dir(instance_id.as_str())
                        .join("mods");
                    let pool = self.state.compute_pool.clone();
                    let guard = self.state.cancel_epoch.snapshot();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let mut removed = 0usize;
                        for f in &orphan_files {
                            if mq_mods::delete_mod(&mods_dir, f).is_ok() {
                                removed += 1;
                            }
                        }
                        let _ = resp_tx.send(UiWorkerResponse::UnusedDepsCleaned {
                            removed,
                            error: None,
                        });
                        if let Ok(rep) =
                            mq_mods::scan_mods_directory(&mods_dir, &pool, &guard).await
                        {
                            let _ = resp_tx.send(UiWorkerResponse::ScanModsResults {
                                mods: rep.mods,
                                conflicts: rep.conflicts,
                            });
                        }
                    });
                }
                UiWorkerCommand::RunPreflight { instance_id } => {
                    let mods_dir = self
                        .state
                        .paths
                        .instance_dir(instance_id.as_str())
                        .join("mods");
                    let inst_opt = self
                        .state
                        .instances
                        .iter()
                        .find(|i| i.id == instance_id)
                        .cloned();
                    let pool = self.state.compute_pool.clone();
                    let guard = self.state.cancel_epoch.snapshot();
                    let resp_tx = self.state.resp_sender.clone();
                    if let Some(inst) = inst_opt {
                        tokio::spawn(async move {
                            if let Ok(report) = mq_mods::preflight_check_mods(
                                &mods_dir,
                                inst.loader,
                                &inst.mc_version,
                                &pool,
                                &guard,
                            )
                            .await
                            {
                                let _ = resp_tx.send(UiWorkerResponse::PreflightResult {
                                    instance_id,
                                    report,
                                });
                            }
                        });
                    }
                }
                UiWorkerCommand::AutoFixPreflight {
                    instance_id,
                    report,
                } => {
                    let mods_dir = self
                        .state
                        .paths
                        .instance_dir(instance_id.as_str())
                        .join("mods");
                    let pool = self.state.compute_pool.clone();
                    let guard = self.state.cancel_epoch.snapshot();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let _ = mq_mods::auto_fix_preflight(&mods_dir, &report);
                        if let Ok(rep) =
                            mq_mods::scan_mods_directory(&mods_dir, &pool, &guard).await
                        {
                            let _ = resp_tx.send(UiWorkerResponse::ScanModsResults {
                                mods: rep.mods,
                                conflicts: rep.conflicts,
                            });
                        }
                        let _ = resp_tx.send(UiWorkerResponse::PreflightResult {
                            instance_id,
                            report: mq_mods::PreflightReport { issues: Vec::new() },
                        });
                    });
                }
                UiWorkerCommand::VerifyInstance { instance_id } => {
                    self.state.installing_operation = Some("Проверка файлов".to_string());
                    self.spawn_verify_instance(instance_id);
                }
                UiWorkerCommand::VerifyAndRepairInstance { instance_id } => {
                    self.state.installing_operation = Some("Проверка и починка".to_string());
                    self.spawn_verify_instance(instance_id);
                }
                UiWorkerCommand::ReinstallInstance { instance_id } => {
                    let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
                    let paths = self.state.paths.clone();
                    let bus = self.state.event_bus.clone();
                    let net = self.state.net.clone();
                    let cas = self.state.cas.clone();
                    let pool = self.state.compute_pool.clone();
                    let guard = self.state.cancel_epoch.snapshot();
                    let resp_tx = self.state.resp_sender.clone();

                    if let Some(inst) = self
                        .state
                        .instances
                        .iter()
                        .find(|i| i.id == instance_id)
                        .cloned()
                    {
                        self.state.installing_operation =
                            Some("Переустановка инстанса".to_string());
                        let _ = resp_tx.send(UiWorkerResponse::InstanceInstallStarted(
                            instance_id.clone(),
                        ));

                        spawn_monitored_task(
                            "reinstall_instance",
                            paths.clone(),
                            instance_id.clone(),
                            resp_tx.clone(),
                            async move {
                                // Safely remove game binaries and temporary directories off the UI thread
                                // while preserving saves/, options.txt, resourcepacks/, shaderpacks/, servers.dat.
                                let clean_dir = inst_dir.clone();
                                let _ = tokio::task::spawn_blocking(move || {
                                    let _ = std::fs::remove_file(clean_dir.join("client.jar"));
                                    let _ = std::fs::remove_file(
                                        clean_dir.join("resolved_version.json"),
                                    );
                                    let _ =
                                        std::fs::remove_file(clean_dir.join("install_state.json"));
                                    let _ = std::fs::remove_dir_all(clean_dir.join("natives"));
                                    let _ = std::fs::remove_dir_all(clean_dir.join(".installing"));
                                })
                                .await;

                                let req = mq_install::InstallRequest {
                                    mc_version: inst.mc_version.clone(),
                                    loader: inst.loader,
                                    loader_version: inst.loader_version.clone(),
                                    instance_dir: inst_dir.clone(),
                                    mock_base_url: configured_mock_base_url(),
                                };

                                match mq_install::install_game(
                                    &req,
                                    &paths,
                                    &net,
                                    &cas,
                                    &pool,
                                    &guard,
                                    Some(&bus),
                                )
                                .await
                                {
                                    Ok(summary) => {
                                        bus.publish(mq_core::Event::PhaseCompleted {
                                            phase: "reinstall".to_string(),
                                            elapsed_ms: summary.elapsed_ms,
                                        });
                                        let _ = resp_tx.send(
                                            UiWorkerResponse::InstanceInstallCompleted(instance_id),
                                        );
                                    }
                                    Err(e) => {
                                        if guard.is_cancelled() {
                                            return;
                                        }
                                        let err_msg =
                                            format!("Ошибка переустановки {}: {e}", inst.name);
                                        bus.publish(mq_core::Event::TaskError {
                                            context: format!("Переустановка {}", inst.name),
                                            message: e.to_string(),
                                        });
                                        let _ = resp_tx.send(UiWorkerResponse::InstanceError {
                                            instance_id,
                                            error: err_msg,
                                        });
                                    }
                                }
                            },
                        );
                    }
                }
                UiWorkerCommand::StopGame { instance_id } => {
                    self.state.stopped_by_user.insert(instance_id.clone());
                    if let Some(Some(pid)) = self.state.running_instances.get(&instance_id).copied()
                    {
                        tokio::spawn(async move {
                            let _ = mq_launch::kill_process_tree(pid);
                        });
                    }
                    self.state.status_message = Some(("Остановка игры...".to_string(), false));
                }
                UiWorkerCommand::CancelDownload => {
                    self.state.cancel_epoch.cancel_and_advance();
                    for job in &self.state.jobs.jobs {
                        if job.is_active() {
                            job.request_cancel();
                        }
                    }
                    let cancelled_inst = self.state.installing_instance.take();
                    let op = self
                        .state
                        .installing_operation
                        .take()
                        .unwrap_or_else(|| "Установка инстанса".to_string());
                    if let Some(inst_id) = cancelled_inst {
                        let inst_title = self
                            .state
                            .instances
                            .iter()
                            .find(|i| i.id == inst_id)
                            .map_or_else(|| inst_id.as_str().to_string(), |i| i.name.clone());
                        self.state.record_download_history(
                            inst_title,
                            "Инстанс",
                            format!("{op} · отменено"),
                            DownloadHistoryStatus::Cancelled,
                            Some(inst_id),
                        );
                        self.state.is_launching = false;
                    }
                    self.state.active_phase = None;
                    self.state.phase_progress = None;
                    self.state.install_eta_secs = None;
                    self.state.status_message =
                        Some(("Загрузка отменена пользователем".to_string(), false));
                }
                UiWorkerCommand::InstallInstance { instance_id } => {
                    let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
                    let paths = self.state.paths.clone();
                    let bus = self.state.event_bus.clone();
                    let net = self.state.net.clone();
                    let cas = self.state.cas.clone();
                    let pool = self.state.compute_pool.clone();
                    let guard = self.state.cancel_epoch.snapshot();
                    let resp_tx = self.state.resp_sender.clone();

                    if let Some(inst) = self
                        .state
                        .instances
                        .iter()
                        .find(|i| i.id == instance_id)
                        .cloned()
                    {
                        self.state.installing_operation = Some("Установка инстанса".to_string());
                        let _ = resp_tx.send(UiWorkerResponse::InstanceInstallStarted(
                            instance_id.clone(),
                        ));
                        spawn_monitored_task(
                            "install_instance",
                            paths.clone(),
                            instance_id.clone(),
                            resp_tx.clone(),
                            async move {
                                let req = mq_install::InstallRequest {
                                    mc_version: inst.mc_version.clone(),
                                    loader: inst.loader,
                                    loader_version: inst.loader_version.clone(),
                                    instance_dir: inst_dir.clone(),
                                    mock_base_url: configured_mock_base_url(),
                                };

                                match mq_install::install_game(
                                    &req,
                                    &paths,
                                    &net,
                                    &cas,
                                    &pool,
                                    &guard,
                                    Some(&bus),
                                )
                                .await
                                {
                                    Ok(summary) => {
                                        bus.publish(mq_core::Event::PhaseCompleted {
                                            phase: "install_game".to_string(),
                                            elapsed_ms: summary.elapsed_ms,
                                        });
                                        let _ = resp_tx.send(
                                            UiWorkerResponse::InstanceInstallCompleted(instance_id),
                                        );
                                    }
                                    Err(e) => {
                                        if guard.is_cancelled() {
                                            return;
                                        }
                                        let err_msg =
                                            format!("Ошибка установки {}: {e}", inst.name);
                                        bus.publish(mq_core::Event::TaskError {
                                            context: format!("Установка {}", inst.name),
                                            message: e.to_string(),
                                        });
                                        let _ = resp_tx.send(UiWorkerResponse::InstanceError {
                                            instance_id,
                                            error: err_msg,
                                        });
                                    }
                                }
                            },
                        );
                    }
                }
                UiWorkerCommand::LaunchInstance {
                    instance_id,
                    dry_run,
                } => {
                    self.launch_instance_core(instance_id, dry_run, None);
                }
                UiWorkerCommand::LaunchInstanceQuickPlay {
                    instance_id,
                    quick_play,
                } => {
                    self.launch_instance_core(instance_id, false, Some(quick_play));
                }
                UiWorkerCommand::AutoInstallMissingDeps {
                    instance_id,
                    missing_ids,
                } => {
                    let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
                    let mods_dir = inst_dir.join("mods");
                    let resp_tx = self.state.resp_sender.clone();
                    let cf_api_key = self.state.config.curseforge_api_key.clone();
                    let inst = self
                        .state
                        .instances
                        .iter()
                        .find(|i| i.id == instance_id)
                        .cloned();
                    self.state.is_auto_fixing_deps = true;
                    tokio::spawn(async move {
                        if let Some(inst) = inst {
                            let report = mq_mods::auto_install_missing_dependencies(
                                &mods_dir,
                                &missing_ids,
                                &inst.mc_version,
                                inst.loader,
                                cf_api_key.as_ref().map(|k| k.expose_secret()),
                            )
                            .await
                            .map_err(|e| e.to_string());
                            let _ = resp_tx.send(UiWorkerResponse::AutoFixDepsDone {
                                instance_id,
                                report,
                            });
                        }
                    });
                }
                UiWorkerCommand::CheckModpackUpdate { instance_id } => {
                    let resp_tx = self.state.resp_sender.clone();
                    let cf_api_key = self.state.config.curseforge_api_key.clone();
                    let inst = self
                        .state
                        .instances
                        .iter()
                        .find(|i| i.id == instance_id)
                        .cloned();
                    self.state.is_checking_modpack_update = true;
                    tokio::spawn(async move {
                        if let Some(inst) = inst {
                            if let Some(meta) = inst.modpack_source {
                                let result = mq_mods::check_modpack_update(
                                    &meta.source,
                                    &meta.project_id,
                                    &meta.version_id,
                                    cf_api_key.as_ref().map(|k| k.expose_secret()),
                                )
                                .await
                                .map_err(|e| e.to_string());
                                let _ = resp_tx.send(UiWorkerResponse::ModpackUpdateChecked {
                                    instance_id,
                                    result,
                                });
                                return;
                            }
                        }
                        let _ = resp_tx.send(UiWorkerResponse::ModpackUpdateChecked {
                            instance_id,
                            result: Ok(None),
                        });
                    });
                }
                UiWorkerCommand::ApplyModpackUpdate { instance_id, info } => {
                    let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
                    let resp_tx = self.state.resp_sender.clone();
                    let cf_api_key = self.state.config.curseforge_api_key.clone();
                    tokio::spawn(async move {
                        let result = mq_mods::apply_modpack_update(
                            &inst_dir,
                            &info,
                            cf_api_key.as_ref().map(|k| k.expose_secret()),
                        )
                        .await
                        .map_err(|e| e.to_string());
                        let _ = resp_tx.send(UiWorkerResponse::ModpackUpdateApplied {
                            instance_id,
                            result,
                        });
                    });
                }
                UiWorkerCommand::DiscoverExternalInstances => {
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::task::spawn_blocking(move || {
                        let list = mq_instances::discover_external_instances();
                        let _ = resp_tx.send(UiWorkerResponse::ExternalInstancesDiscovered(list));
                    });
                }
                UiWorkerCommand::ImportExternalInstance { item } => {
                    let mgr = self.state.instance_mgr.clone();
                    let paths = self.state.paths.clone();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::task::spawn_blocking(move || {
                        let result = mq_instances::import_external_instance(&mgr, &paths, &item)
                            .map_err(|e| e.to_string());
                        let _ = resp_tx
                            .send(UiWorkerResponse::ExternalInstanceImported(Box::new(result)));
                    });
                }
                UiWorkerCommand::AutoBackupWorlds { instance_id } => {
                    let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
                    let max_keep = self.state.config.max_world_backups;
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::task::spawn_blocking(move || {
                        let result = mq_instances::auto_backup_instance_worlds(&inst_dir, max_keep)
                            .map_err(|e| e.to_string());
                        let _ = resp_tx.send(UiWorkerResponse::WorldsBackedUp {
                            instance_id,
                            result,
                        });
                    });
                }
                UiWorkerCommand::SearchCatalog {
                    request_id,
                    latest,
                    source,
                    query,
                } => {
                    let bus = self.state.event_bus.clone();
                    let config = self.state.config.clone();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let started = std::time::Instant::now();
                        let page = crate::catalog_worker::run_search(
                            &config, request_id, &latest, source, &query,
                        )
                        .await;
                        // Cancelled by a newer request: drop silently.
                        let Some(page) = page else { return };
                        let _ = resp_tx.send(UiWorkerResponse::CatalogResults(page));
                        bus.publish(mq_core::Event::PhaseCompleted {
                            phase: "catalog_search".to_string(),
                            elapsed_ms: started.elapsed().as_millis() as u64,
                        });
                    });
                }
                UiWorkerCommand::FetchIcon { url, edge } => {
                    let resp_tx = self.state.resp_sender.clone();
                    let cache_dir = self.state.paths.cache_dir.join("catalog-icons");
                    tokio::spawn(async move {
                        let image = crate::catalog_worker::load_icon(&cache_dir, &url, edge).await;
                        let _ = resp_tx.send(UiWorkerResponse::IconLoaded { url, image });
                    });
                }
                UiWorkerCommand::LoadProject {
                    request_id,
                    source,
                    project_id,
                    kind,
                } => {
                    let config = self.state.config.clone();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let result =
                            crate::catalog_ops::load_project(&config, source, &project_id, kind)
                                .await;
                        let _ = resp_tx.send(UiWorkerResponse::ProjectLoaded {
                            request_id,
                            result: result.map(Box::new),
                        });
                    });
                }
                UiWorkerCommand::PlanInstall { request_id, req } => {
                    let config = self.state.config.clone();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let result = crate::catalog_ops::plan_content(&config, &req).await;
                        let _ = resp_tx.send(UiWorkerResponse::PlanReady { request_id, result });
                    });
                }
                UiWorkerCommand::PrepareModpack {
                    request_id,
                    version,
                } => {
                    let ctx = self.ops_ctx();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let result = crate::catalog_ops::prepare_modpack(&ctx, &version).await;
                        let _ =
                            resp_tx.send(UiWorkerResponse::ModpackPrepared { request_id, result });
                    });
                }
                UiWorkerCommand::CheckUpdates {
                    request_id,
                    instance_id,
                    mc_version,
                    loader,
                } => {
                    let config = self.state.config.clone();
                    let dir = self.state.paths.instance_dir(instance_id.as_str());
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let result =
                            crate::catalog_ops::check_updates(&config, &dir, &mc_version, loader)
                                .await;
                        let _ =
                            resp_tx.send(UiWorkerResponse::UpdatesChecked { request_id, result });
                    });
                }
                UiWorkerCommand::ApplyUpdate {
                    job_id,
                    instance_id,
                    candidate,
                } => {
                    let ctx = self.ops_ctx();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let result =
                            crate::catalog_ops::apply_update(&ctx, &instance_id, &candidate)
                                .await
                                .map(|()| crate::state::JobOutput {
                                    instance_id: Some(instance_id.clone()),
                                    blocked: Vec::new(),
                                });
                        let _ = resp_tx.send(UiWorkerResponse::JobFinished { job_id, result });
                    });
                }
                UiWorkerCommand::PreviewLocalPack { request_id, path } => {
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let result = tokio::task::spawn_blocking(move || {
                            crate::catalog_ops::preview_archive(&path)
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r);
                        let _ =
                            resp_tx.send(UiWorkerResponse::ModpackPrepared { request_id, result });
                    });
                }
                UiWorkerCommand::StartContentJob {
                    job_id,
                    cancel,
                    instance_id,
                    kind,
                    source,
                    downloads,
                    blocked,
                } => {
                    let ctx = self.ops_ctx();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let tx = resp_tx.clone();
                        let result = crate::catalog_ops::run_content_job(
                            &ctx,
                            &cancel,
                            &instance_id,
                            kind,
                            source,
                            &downloads,
                            move |done, total| {
                                let _ = tx.send(UiWorkerResponse::JobProgress {
                                    job_id,
                                    done,
                                    total,
                                });
                            },
                        )
                        .await
                        .map(|()| crate::state::JobOutput {
                            instance_id: Some(instance_id.clone()),
                            blocked,
                        });
                        let _ = resp_tx.send(UiWorkerResponse::JobFinished { job_id, result });
                    });
                }
                UiWorkerCommand::StartModpackJob {
                    job_id,
                    cancel,
                    instance_name,
                    preview,
                } => {
                    let ctx = self.ops_ctx();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let tx = resp_tx.clone();
                        let result = crate::catalog_ops::run_modpack_job(
                            &ctx,
                            &cancel,
                            &instance_name,
                            &preview,
                            move |stage| {
                                let _ = tx.send(UiWorkerResponse::JobStage {
                                    job_id,
                                    stage_label: stage.to_string(),
                                });
                            },
                        )
                        .await
                        .map(|(id, blocked)| crate::state::JobOutput {
                            instance_id: Some(id),
                            blocked,
                        });
                        let _ = resp_tx.send(UiWorkerResponse::JobFinished { job_id, result });
                    });
                }
                UiWorkerCommand::PollManual { dropped } => {
                    if let Some(watcher) = self.state.manual_watcher.clone() {
                        let resp_tx = self.state.resp_sender.clone();
                        tokio::spawn(async move {
                            let joined = tokio::task::spawn_blocking(move || {
                                crate::catalog_ops::poll_watcher(&watcher, &dropped)
                            })
                            .await;
                            if let Ok((pending, newly_done)) = joined {
                                let _ = resp_tx.send(UiWorkerResponse::ManualProgress {
                                    pending,
                                    newly_done,
                                });
                            }
                        });
                    }
                }
                UiWorkerCommand::PickImportArchive => {
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let picked =
                            tokio::task::spawn_blocking(crate::opener::pick_modpack_archive_file)
                                .await
                                .ok()
                                .flatten();
                        let _ = resp_tx.send(UiWorkerResponse::ImportArchivePicked(picked));
                    });
                }
                UiWorkerCommand::ScanStorageStats => {
                    self.state.is_scanning_storage = true;
                    let paths = self.state.paths.clone();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let stats = tokio::task::spawn_blocking(move || scan_storage_stats(&paths))
                            .await
                            .unwrap_or_default();
                        let _ = resp_tx.send(UiWorkerResponse::StorageStatsReady(stats));
                    });
                }
                UiWorkerCommand::CleanTempCache => {
                    let paths = self.state.paths.clone();
                    let resp_tx = self.state.resp_sender.clone();
                    let skip_active_temp = self.state.active_download_count() > 0;
                    tokio::spawn(async move {
                        let (removed_files, freed_bytes) = tokio::task::spawn_blocking(move || {
                            clean_temp_cache(&paths, skip_active_temp)
                        })
                        .await
                        .unwrap_or((0, 0));
                        let _ = resp_tx.send(UiWorkerResponse::TempCacheCleaned {
                            removed_files,
                            freed_bytes,
                        });
                    });
                }
                UiWorkerCommand::UploadMclogs { log_text } => {
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(15))
                            .build()
                            .unwrap_or_default();
                        let res = mq_launch::upload_to_mclogs(&client, &log_text)
                            .await
                            .map_err(|e| e.to_string());
                        let _ = resp_tx.send(UiWorkerResponse::MclogsUploaded(res));
                    });
                }
                UiWorkerCommand::PingServer { ip } => {
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let ip_clone = ip.clone();
                        let result = tokio::task::spawn_blocking(move || {
                            mq_instances::ping_server(&ip_clone, 3000).map_err(|e| e.to_string())
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("Внутренняя ошибка пинга: {e}")));
                        let _ = resp_tx.send(UiWorkerResponse::ServerPingReady { ip, result });
                    });
                }
                UiWorkerCommand::DiscoverJavaRuntimes => {
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let runtimes =
                            tokio::task::spawn_blocking(mq_launch::discover_java_runtimes)
                                .await
                                .unwrap_or_default();
                        let _ = resp_tx.send(UiWorkerResponse::JavaRuntimesDiscovered(runtimes));
                    });
                }
                UiWorkerCommand::CheckLauncherUpdate { current_version } => {
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let mgr = mq_update::UpdateManager::new("MrQuster", "MQ-Launcher");
                        let res = mgr
                            .check_latest(&current_version, false)
                            .await
                            .map_err(|e| e.to_string());
                        let _ = resp_tx.send(UiWorkerResponse::UpdateCheckFinished(res));
                    });
                }
                UiWorkerCommand::DownloadLauncherUpdate { asset } => {
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let mgr = mq_update::UpdateManager::new("MrQuster", "MQ-Launcher");
                        match mgr.download_and_install(&asset, |_d, _t| {}).await {
                            Ok(_path) => {
                                if let Err(e) = mq_update::UpdateManager::restart_process(&[]) {
                                    let _ = resp_tx.send(UiWorkerResponse::UpdateDownloadFailed(
                                        format!(
                                            "Обновление установлено, но перезапуск не удался: {e}"
                                        ),
                                    ));
                                }
                            }
                            Err(e) => {
                                tracing::error!("Update failed: {e}");
                                let _ = resp_tx
                                    .send(UiWorkerResponse::UpdateDownloadFailed(e.to_string()));
                            }
                        }
                    });
                }
                UiWorkerCommand::ExportInstance { instance_id } => {
                    let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
                    let out_path = self
                        .state
                        .paths
                        .backups_dir()
                        .join(format!("{}.zip", instance_id.as_str()));
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let res = tokio::task::spawn_blocking(move || {
                            mq_instances::export_instance_to_zip(&inst_dir, &out_path)
                                .map(|()| out_path)
                                .map_err(|e| e.to_string())
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("Внутренняя ошибка экспорта: {e}")));
                        let _ = resp_tx.send(UiWorkerResponse::InstanceExported(res));
                    });
                }
                UiWorkerCommand::DeleteInstance { instance_id, name } => {
                    let paths = self.state.paths.clone();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let id_clone = instance_id.clone();
                        let error = tokio::task::spawn_blocking(move || {
                            let mgr = mq_instances::InstanceManager::new(paths);
                            match mgr.delete_instance(id_clone.as_str()) {
                                Ok(()) | Err(mq_core::MqError::InstanceNotFound(_)) => None,
                                Err(e) => Some(e.to_string()),
                            }
                        })
                        .await
                        .unwrap_or_else(|e| Some(format!("Внутренняя ошибка удаления: {e}")));
                        let _ = resp_tx.send(UiWorkerResponse::InstanceDeleted {
                            instance_id,
                            name,
                            error,
                        });
                    });
                }
                UiWorkerCommand::AnalyzeCrash {
                    instance_id,
                    exit_code,
                    lines,
                } => {
                    let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        if let Ok(analysis) = tokio::task::spawn_blocking(move || {
                            mq_launch::analyze_crash(&inst_dir, Some(exit_code), &lines)
                        })
                        .await
                        {
                            let _ = resp_tx.send(UiWorkerResponse::CrashAnalysisReady {
                                instance_id,
                                analysis,
                            });
                        }
                    });
                }
                UiWorkerCommand::BackupWorld {
                    instance_id,
                    folder_name,
                } => {
                    let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
                    let backups_root = self.state.paths.backups_dir();
                    let resp_tx = self.state.resp_sender.clone();
                    let inst_id_str = instance_id.as_str().to_string();
                    tokio::spawn(async move {
                        let res = tokio::task::spawn_blocking(move || {
                            mq_instances::backup_world(
                                &inst_dir,
                                &backups_root,
                                &inst_id_str,
                                &folder_name,
                            )
                            .map_err(|e| e.to_string())
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("Внутренняя ошибка бэкапа: {e}")));
                        let _ = resp_tx
                            .send_async(UiWorkerResponse::WorldBackedUp {
                                instance_id,
                                result: res,
                            })
                            .await;
                    });
                }
                UiWorkerCommand::ExportWorld {
                    instance_id,
                    folder_name,
                    export_path,
                } => {
                    let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
                    let resp_tx = self.state.resp_sender.clone();
                    let export_target = export_path.clone();
                    tokio::spawn(async move {
                        let res = tokio::task::spawn_blocking(move || {
                            mq_instances::export_world_to_zip(
                                &inst_dir,
                                &folder_name,
                                &export_target,
                            )
                            .map(|()| export_target)
                            .map_err(|e| e.to_string())
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("Внутренняя ошибка экспорта: {e}")));
                        let _ = resp_tx
                            .send_async(UiWorkerResponse::WorldExported {
                                instance_id,
                                result: res,
                            })
                            .await;
                    });
                }
                UiWorkerCommand::RestoreWorld {
                    instance_id,
                    file_name,
                } => {
                    let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
                    let backups_root = self.state.paths.backups_dir();
                    let resp_tx = self.state.resp_sender.clone();
                    let inst_id_str = instance_id.as_str().to_string();
                    let fname = file_name.clone();
                    tokio::spawn(async move {
                        let res = tokio::task::spawn_blocking(move || {
                            mq_instances::restore_world(
                                &backups_root,
                                &inst_id_str,
                                &fname,
                                &inst_dir,
                                None,
                            )
                            .map_err(|e| e.to_string())
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("Внутренняя ошибка восстановления: {e}")));
                        let _ = resp_tx
                            .send_async(UiWorkerResponse::WorldRestored {
                                instance_id,
                                file_name,
                                result: res,
                            })
                            .await;
                    });
                }
                UiWorkerCommand::ImportInstanceZip { path, custom_name } => {
                    let instances_dir = self.state.paths.instances_dir();
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let res = tokio::task::spawn_blocking(move || {
                            mq_instances::import_instance_from_zip(
                                &path,
                                &instances_dir,
                                custom_name.as_deref(),
                            )
                            .map_err(|e| e.to_string())
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("Внутренняя ошибка импорта: {e}")));
                        let _ = resp_tx
                            .send_async(UiWorkerResponse::InstanceZipImported(Box::new(res)))
                            .await;
                    });
                }
                UiWorkerCommand::ComputeWorldSizes { instance_id } => {
                    let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
                    let resp_tx = self.state.resp_sender.clone();
                    tokio::spawn(async move {
                        let sizes = tokio::task::spawn_blocking(move || {
                            let saves_dir = inst_dir.join("saves");
                            let mut map = std::collections::HashMap::new();
                            if let Ok(entries) = std::fs::read_dir(saves_dir) {
                                for entry in entries.flatten() {
                                    if entry.path().is_dir() {
                                        let name = entry.file_name().to_string_lossy().to_string();
                                        let sz = mq_instances::compute_dir_size(&entry.path());
                                        map.insert(name, sz);
                                    }
                                }
                            }
                            map
                        })
                        .await
                        .unwrap_or_default();
                        let _ = resp_tx
                            .send_async(UiWorkerResponse::WorldSizesReady { instance_id, sizes })
                            .await;
                    });
                }
                _ => {}
            }
        }
    }

    fn spawn_verify_instance(&mut self, instance_id: mq_core::InstanceId) {
        let inst_dir = self.state.paths.instance_dir(instance_id.as_str());
        let resolved_path = inst_dir.join("resolved_version.json");
        let paths = self.state.paths.clone();
        let net = self.state.net.clone();
        let cas = self.state.cas.clone();
        let pool = self.state.compute_pool.clone();
        let guard = self.state.cancel_epoch.snapshot();
        let bus = self.state.event_bus.clone();
        let resp_tx = self.state.resp_sender.clone();
        let inst_opt = self
            .state
            .instances
            .iter()
            .find(|i| i.id == instance_id)
            .cloned();

        let _ = resp_tx.send(UiWorkerResponse::InstanceInstallStarted(
            instance_id.clone(),
        ));

        spawn_monitored_task(
            "verify_instance",
            paths.clone(),
            instance_id.clone(),
            resp_tx.clone(),
            async move {
                bus.publish(mq_core::Event::PhaseStarted {
                    phase: "verify".to_string(),
                    total_items: 100,
                });

                let mut verified_ok = false;
                if let Ok(bytes) = std::fs::read(&resolved_path) {
                    if let Ok(ver) = serde_json::from_slice::<mq_meta::ResolvedVersion>(&bytes) {
                        let asset_index_path = paths
                            .assets_dir()
                            .join("indexes")
                            .join(format!("{}.json", ver.asset_index.id));
                        if let Ok(idx_bytes) = std::fs::read(&asset_index_path) {
                            if let Ok(mut idx) =
                                serde_json::from_slice::<mq_meta::AssetIndex>(&idx_bytes)
                            {
                                if idx.id.is_empty() {
                                    idx.id = ver.asset_index.id.clone();
                                }
                                let targets = mq_install::collect_instance_verify_targets(
                                    &paths, &inst_dir, &ver, &idx,
                                );
                                match mq_install::verify_files_parallel(
                                    &targets,
                                    &pool,
                                    &guard,
                                    Some(&bus),
                                )
                                .await
                                {
                                    Ok(report) => {
                                        if report.is_clean() {
                                            bus.publish(mq_core::Event::PhaseCompleted {
                                                phase: "verify".to_string(),
                                                elapsed_ms: report.elapsed_ms,
                                            });
                                            let _ = resp_tx.send(
                                                UiWorkerResponse::InstanceInstallCompleted(
                                                    instance_id.clone(),
                                                ),
                                            );
                                            verified_ok = true;
                                        } else {
                                            for corrupt in &report.corrupted_files {
                                                let _ = std::fs::remove_file(corrupt);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        if guard.is_cancelled() {
                                            return;
                                        }
                                        bus.publish(mq_core::Event::TaskError {
                                            context: "Проверка файлов".to_string(),
                                            message: e.to_string(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }

                if guard.is_cancelled() {
                    return;
                }

                if !verified_ok {
                    if let Some(inst) = inst_opt {
                        let req = mq_install::InstallRequest {
                            mc_version: inst.mc_version.clone(),
                            loader: inst.loader,
                            loader_version: inst.loader_version.clone(),
                            instance_dir: inst_dir.clone(),
                            mock_base_url: configured_mock_base_url(),
                        };

                        match mq_install::install_game(
                            &req,
                            &paths,
                            &net,
                            &cas,
                            &pool,
                            &guard,
                            Some(&bus),
                        )
                        .await
                        {
                            Ok(summary) => {
                                bus.publish(mq_core::Event::PhaseCompleted {
                                    phase: "verify_and_repair".to_string(),
                                    elapsed_ms: summary.elapsed_ms,
                                });
                                let _ = resp_tx.send(UiWorkerResponse::InstanceInstallCompleted(
                                    instance_id.clone(),
                                ));
                            }
                            Err(e) => {
                                if guard.is_cancelled() {
                                    return;
                                }
                                let err_msg = format!("Ошибка починки {}: {e}", inst.name);
                                bus.publish(mq_core::Event::TaskError {
                                    context: format!("Починка {}", inst.name),
                                    message: e.to_string(),
                                });
                                let _ = resp_tx.send(UiWorkerResponse::InstanceError {
                                    instance_id,
                                    error: err_msg,
                                });
                            }
                        }
                    } else {
                        let _ =
                            resp_tx.send(UiWorkerResponse::InstanceInstallCompleted(instance_id));
                    }
                }
            },
        );
    }

    fn build_sidebar_state(&self) -> SidebarState {
        SidebarState {
            active_downloads: self.state.active_download_count(),
            update_available: self.settings_view.available_update.is_some(),
        }
    }

    fn build_hud_data(&self) -> BottomHudData {
        let active_jobs: Vec<_> = self
            .state
            .jobs
            .jobs
            .iter()
            .filter(|j| j.is_active())
            .collect();
        let job_mib_per_sec: f64 = active_jobs
            .iter()
            .map(|j| j.mib_per_sec)
            .filter(|v| v.is_finite() && *v > 0.0)
            .sum();

        let (progress_fraction, files_per_sec, mib_per_sec) =
            if let Some(p) = &self.state.phase_progress {
                let frac = if p.total_items > 0 {
                    Some(crate::state::safe_progress_ratio(
                        p.completed_items,
                        p.total_items,
                    ))
                } else if p.bytes_total > 0 {
                    Some(crate::state::safe_progress_ratio(
                        p.bytes_done,
                        p.bytes_total,
                    ))
                } else {
                    active_jobs.first().map(|j| j.progress())
                };
                let fps = if p.files_per_sec.is_finite() && p.files_per_sec >= 0.0 {
                    p.files_per_sec
                } else {
                    0.0
                };
                let mps = if p.mib_per_sec.is_finite() && p.mib_per_sec > 0.0 {
                    p.mib_per_sec
                } else {
                    job_mib_per_sec
                };
                (frac, fps, mps)
            } else if !active_jobs.is_empty() {
                let done: usize = active_jobs.iter().map(|j| j.done_files).sum();
                let total: usize = active_jobs.iter().map(|j| j.total_files).sum();
                let frac = if total > 0 {
                    Some(crate::state::safe_progress_ratio(done as u64, total as u64))
                } else {
                    active_jobs.first().map(|j| j.progress())
                };
                (frac, 0.0, job_mib_per_sec)
            } else {
                (None, 0.0, 0.0)
            };

        let phase_tasks = self
            .state
            .phase_progress
            .as_ref()
            .map(|p| p.active_tasks)
            .unwrap_or(0);
        let active_dag_tasks = phase_tasks.max(self.state.active_download_count());

        BottomHudData {
            active_phase: self.state.active_hud_phase_label(),
            progress_fraction,
            files_per_sec,
            mib_per_sec,
            active_dag_tasks,
            cpu_snapshot: self.state.latest_cpu.clone(),
            watchdog_violations: self.state.watchdog.violation_count(),
        }
    }

    fn render_active_tab(&mut self, ui: &mut egui::Ui) {
        match self.current_tab {
            NavTab::Instances => self.instances_view.ui(ui, &mut self.state),
            NavTab::Catalog => self.catalog_view.ui(ui, &mut self.state),
            NavTab::Accounts => self.accounts_view.ui(ui, &mut self.state),
            NavTab::Downloads => {
                if let Some(action) = self.downloads_view.ui(ui, &mut self.state) {
                    match action {
                        DownloadsAction::OpenCatalog => {
                            self.current_tab = NavTab::Catalog;
                        }
                        DownloadsAction::OpenInstance(inst_id) => {
                            self.state.selected_instance_id = Some(inst_id);
                            self.instances_view.view_mode = InstanceViewMode::List;
                            self.current_tab = NavTab::Instances;
                        }
                    }
                }
            }
            NavTab::Performance => {
                if let Some(action) = self.perf_view.ui(ui, &mut self.state) {
                    match action {
                        PerfAction::OpenInstanceConsole(inst_id) => {
                            self.state.selected_instance_id = Some(inst_id);
                            self.instances_view.view_mode = InstanceViewMode::List;
                            self.instances_view.current_tab = InstanceDetailTab::Console;
                            self.current_tab = NavTab::Instances;
                        }
                    }
                }
            }
            NavTab::Settings => self.settings_view.ui(ui, &mut self.state),
        }
    }

    /// Handles shared with background catalog operations.
    fn ops_ctx(&self) -> crate::catalog_ops::OpsCtx {
        crate::catalog_ops::OpsCtx {
            config: self.state.config.clone(),
            paths: self.state.paths.clone(),
            net: self.state.net.clone(),
            cas: self.state.cas.clone(),
            epoch: self.state.cancel_epoch.clone(),
            pool: self.state.compute_pool.clone(),
            bus: self.state.event_bus.clone(),
            mock_base_url: configured_mock_base_url(),
        }
    }

    /// Paints seamless full-window blurred wallpaper / aurora backdrop behind sidebar, HUD,
    /// and central area when running in `LiquidGlass` or `Translucent`.
    fn paint_full_window_glass_backdrop(
        ctx: &egui::Context,
        effective_theme: &mq_core::ThemeConfig,
        tokens: &Tokens,
    ) -> bool {
        let full_window = matches!(
            effective_theme.background_style,
            mq_core::BackgroundStyle::Procedural
                | mq_core::BackgroundStyle::Solid
                | mq_core::BackgroundStyle::CustomImage
        ) && matches!(
            effective_theme.preset,
            mq_core::ThemePreset::LiquidGlass | mq_core::ThemePreset::Translucent
        );
        if full_window {
            let bg_painter = ctx.layer_painter(egui::LayerId::background());
            paint_launcher_background(ctx, &bg_painter, ctx.screen_rect(), effective_theme, tokens);
        }
        full_window
    }

    /// Headless/offscreen frame rendering for visual QA and snapshots.
    pub fn render_frame_for_snapshot(&mut self, ctx: &egui::Context) {
        if is_zero_viewport(ctx) {
            return;
        }
        crate::animation::begin_animation_frame(ctx);
        let accounts_list = self.state.accounts.list_accounts().to_vec();
        let active_id = self
            .state
            .accounts
            .active_account()
            .map(|a| a.id.to_string());

        let launcher_locale = self.state.config.launcher_locale.clone();
        let tokens = Tokens::from_config(&self.state.config.theme);

        #[allow(unused_mut)]
        let mut effective_theme = self.state.config.theme.clone();
        #[cfg(feature = "test-hooks")]
        if crate::test_hooks::is_solid_background(ctx)
            && effective_theme.preset != mq_core::ThemePreset::LiquidGlass
        {
            effective_theme.background_style = mq_core::BackgroundStyle::Solid;
        }

        let full_window_glass =
            Self::paint_full_window_glass_backdrop(ctx, &effective_theme, &tokens);

        let sidebar_state = self.build_sidebar_state();

        SidePanel::left("mq_sidebar")
            .resizable(false)
            .exact_width(sidebar_width(ctx))
            .frame(solid_sidebar_frame_with_config(
                &tokens,
                &self.state.config.theme,
            ))
            .show(ctx, |ui| {
                let _ = render_sidebar_with_state(
                    ui,
                    self.current_tab,
                    &accounts_list,
                    active_id.as_deref(),
                    &launcher_locale,
                    &tokens,
                    sidebar_state,
                );
            });

        let hud_data = self.build_hud_data();

        TopBottomPanel::bottom("mq_hud")
            .frame(solid_hud_frame_with_config(
                &tokens,
                &self.state.config.theme,
            ))
            .show(ctx, |ui| {
                render_bottom_hud(ui, &hud_data);
            });

        let panel_frame = egui::Frame::new()
            .fill(egui::Color32::TRANSPARENT)
            .inner_margin(egui::Margin::same(16));
        CentralPanel::default().frame(panel_frame).show(ctx, |ui| {
            if !full_window_glass {
                let bg_rect = ui.max_rect().expand(16.0);
                let bg_painter = ui.painter().with_clip_rect(bg_rect);
                paint_launcher_background(ctx, &bg_painter, bg_rect, &effective_theme, &tokens);
            }
            self.render_active_tab(ui);
        });
    }
    #[cfg(feature = "test-hooks")]
    pub fn step_test_hooks(&mut self, ctx: &egui::Context) {
        crate::test_hooks::on_pre_frame(
            ctx,
            &mut crate::test_hooks::TestAppAccess {
                state: &mut self.state,
                current_tab: &mut self.current_tab,
                settings_section: &mut self.settings_view.active_section,
                instances_view: &mut self.instances_view,
                catalog_view: &mut self.catalog_view,
            },
        );

        let hook_id = egui::Id::new("mq_test_hook_state");
        let mut hook = ctx.data_mut(|d| {
            d.get_temp::<TestHookRuntimeState>(hook_id)
                .unwrap_or_default()
        });

        if !hook.initialized {
            hook.initialized = true;
            hook.cancel_after_ms = std::env::var("MQ_TEST_CANCEL_AFTER_MS")
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok());
            hook.exit_on_finish = std::env::var("MQ_TEST_EXIT_ON_FINISH")
                .ok()
                .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
                .unwrap_or(false);

            if let Ok(raw_action) = std::env::var("MQ_TEST_ACTION") {
                if let Some(TestAction::Install { mc_version, loader }) =
                    parse_test_action(&raw_action)
                {
                    let existing_id = self
                        .state
                        .instances
                        .iter()
                        .find(|i| i.mc_version == mc_version && i.loader == loader)
                        .map(|i| i.id.clone());

                    let inst_id_opt = if let Some(id) = existing_id {
                        Some(id)
                    } else {
                        let inst_name = format!("{} {}", mc_version, loader.as_str());
                        match self.state.instance_mgr.create_instance(
                            &inst_name,
                            &mc_version,
                            loader,
                            None,
                        ) {
                            Ok(mut manifest) => {
                                manifest.memory_max_mb = self.state.create_inst_ram_mb;
                                let _ = self.state.instance_mgr.save_instance(&manifest);
                                let id = manifest.id.clone();
                                self.state.instances.push(manifest);
                                Some(id)
                            }
                            Err(e) => {
                                tracing::error!("MQ_TEST_ACTION failed to create instance: {e}");
                                self.state.status_message = Some((
                                    format!("Ошибка создания инстанса (MQ_TEST_ACTION): {e}"),
                                    true,
                                ));
                                None
                            }
                        }
                    };

                    if let Some(inst_id) = inst_id_opt {
                        self.state.selected_instance_id = Some(inst_id.clone());
                        self.state.installing_instance = Some(inst_id.clone());
                        self.state.install_error = None;
                        self.state.detailed_install_error = None;
                        self.state.current_install_stage =
                            crate::install_types::InstallStage::Manifest;
                        self.state.install_retries_count = 0;
                        self.state.install_eta_secs = None;
                        self.state.status_message = Some((
                            format!(
                                "Инстанс '{}' выбран. Запущена автоматическая установка...",
                                inst_id
                            ),
                            false,
                        ));
                        let _ = self
                            .state
                            .cmd_sender
                            .send(UiWorkerCommand::InstallInstance {
                                instance_id: inst_id,
                            });
                        hook.action_active = true;
                        hook.started_at = Some(std::time::Instant::now());
                    }
                }
            }
        }

        if hook.action_active {
            if let (Some(cancel_ms), Some(started_at)) = (hook.cancel_after_ms, hook.started_at) {
                if !hook.cancel_sent
                    && started_at.elapsed() >= std::time::Duration::from_millis(cancel_ms)
                {
                    hook.cancel_sent = true;
                    let _ = self.state.cmd_sender.send(UiWorkerCommand::CancelDownload);
                }
            }
        }

        ctx.data_mut(|d| d.insert_temp(hook_id, hook));
    }

    #[cfg(feature = "test-hooks")]
    fn post_frame_test_hooks(&mut self, ctx: &egui::Context) {
        let hook_id = egui::Id::new("mq_test_hook_state");
        let mut hook = ctx.data_mut(|d| {
            d.get_temp::<TestHookRuntimeState>(hook_id)
                .unwrap_or_default()
        });

        if hook.action_active {
            ctx.request_repaint();
            let waiting_for_cancel = hook.cancel_after_ms.is_some() && !hook.cancel_sent;
            if self.state.installing_instance.is_none() && !waiting_for_cancel {
                hook.finish_frames = hook.finish_frames.saturating_add(1);
                if hook.exit_on_finish && hook.finish_frames >= 3 {
                    hook.action_active = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        ctx.data_mut(|d| d.insert_temp(hook_id, hook));

        crate::test_hooks::on_post_frame(
            ctx,
            &mut crate::test_hooks::TestAppAccess {
                state: &mut self.state,
                current_tab: &mut self.current_tab,
                settings_section: &mut self.settings_view.active_section,
                instances_view: &mut self.instances_view,
                catalog_view: &mut self.catalog_view,
            },
        );
    }

    /// Renders recovery dialog if previous launcher run terminated unexpectedly with an active install journal.
    pub fn render_recovery_dialog(&mut self, ctx: &egui::Context) {
        let Some((journal, anim)) = crate::animation::animate_modal_cached(
            ctx,
            egui::Id::new("mq_recovery_modal"),
            self.recovery_journal.clone(),
        ) else {
            return;
        };

        let tokens = Tokens::from_config(&self.state.config.theme);
        let mut close_dialog = false;
        let mut action_continue = false;
        let mut action_clean_reinstall = false;
        let mut action_copy_diag = false;

        let version_str = journal.mc_version.clone();
        let stage_str = journal.stage.clone();
        let inst_id_str = journal.instance_id.clone();
        let diag_text = journal.format_diagnostic_summary(&self.state.paths);

        let window = egui::Window::new("Восстановление после аварийного завершения")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .frame(
                egui::Frame::new()
                    .fill(tokens.bg_surface)
                    .corner_radius(egui::CornerRadius::same(8))
                    .stroke(egui::Stroke::new(1.0_f32, tokens.border))
                    .inner_margin(egui::Margin::same(20)),
            );

        crate::animation::show_animated_modal_window(
            ctx,
            egui::Id::new("mq_recovery_modal_win"),
            &anim,
            window,
            |ui| {
                ui.set_max_width(540.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("Обнаружено прерывание предыдущей установки")
                            .font(egui::FontId::proportional(16.0))
                            .color(tokens.warning),
                    );
                    ui.add_space(8.0);
                    let prompt_text = format!(
                        "В прошлый раз лаунчер завершился аварийно при установке {}, этап {}",
                        version_str, stage_str
                    );
                    ui.label(
                        egui::RichText::new(prompt_text)
                            .font(egui::FontId::proportional(13.5))
                            .color(tokens.text),
                    );
                    ui.add_space(18.0);

                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::Vec2::new(10.0, 8.0);
                        if crate::components::button_primary(ui, &tokens, None, "Продолжить")
                            .on_hover_text("Возобновить процесс установки")
                            .clicked()
                        {
                            action_continue = true;
                            close_dialog = true;
                        }

                        if crate::components::button_secondary(
                            ui,
                            &tokens,
                            None,
                            "Очистить и начать заново",
                        )
                        .on_hover_text(
                            "Удалить повреждённые временные файлы и начать чистую установку",
                        )
                        .clicked()
                        {
                            action_clean_reinstall = true;
                            close_dialog = true;
                        }

                        if crate::components::button_secondary(
                            ui,
                            &tokens,
                            None,
                            "Скопировать диагностику",
                        )
                        .on_hover_text("Скопировать отчёт об аварии в буфер обмена")
                        .clicked()
                        {
                            action_copy_diag = true;
                            close_dialog = true;
                        }
                    });
                });
            },
        );

        if action_copy_diag {
            ctx.copy_text(diag_text);
            self.state.status_message = Some((
                "Диагностический отчёт скопирован в буфер обмена".to_string(),
                false,
            ));
        }

        if action_continue {
            if let Ok(inst_id) = mq_core::InstanceId::new(&inst_id_str) {
                self.state.installing_instance = Some(inst_id.clone());
                let _ = self
                    .state
                    .cmd_sender
                    .send(UiWorkerCommand::InstallInstance {
                        instance_id: inst_id,
                    });
            }
        }

        if action_clean_reinstall {
            if let Ok(inst_id) = mq_core::InstanceId::new(&inst_id_str) {
                self.state.installing_instance = Some(inst_id.clone());
                let inst_dir = self.state.paths.instance_dir(inst_id.as_str());
                let _ = std::fs::remove_dir_all(inst_dir.join(".installing"));
                let _ = std::fs::remove_dir_all(inst_dir.join("natives"));
                let _ = std::fs::remove_file(inst_dir.join("client.jar"));
                let _ = std::fs::remove_file(inst_dir.join("resolved_version.json"));
                let _ = std::fs::remove_file(inst_dir.join("install_state.json"));
                let _ = self
                    .state
                    .cmd_sender
                    .send(UiWorkerCommand::ReinstallInstance {
                        instance_id: inst_id,
                    });
            }
        }

        if close_dialog {
            let _ = mq_core::crash::InstallJournal::mark_closed_at(&self.state.paths);
            self.recovery_journal = None;
        }
    }

    /// Drains async worker events, pending UI commands, and single-instance `.mrpack` IPC.
    /// Runs even when the window is minimized (`0x0` viewport on Windows) so downloads,
    /// launches, and external `.mrpack` open requests never stall while minimized.
    fn drain_background_and_ipc(&mut self, ctx: &egui::Context) {
        let had_events = !self.state.resp_receiver.is_empty() || !self.state.event_bus.is_empty();
        self.state.drain_events();
        self.handle_worker_commands();
        if had_events {
            ctx.request_repaint();
        }

        if let Some(rx) = self.state.mrpack_ipc_receiver.clone() {
            let mut received = Vec::new();
            if let Ok(guard) = rx.lock() {
                while let Ok(path) = guard.try_recv() {
                    received.push(path);
                }
            }
            if !received.is_empty() {
                for path in received {
                    self.state.import_local_pack(path);
                }
                self.current_tab = NavTab::Catalog;
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }

        if let Some(rpc) = self.state.pending_discord_rpc.take() {
            self.discord_rpc = Some(rpc);
        }

        for _inst_id in self.state.pending_game_started.drain(..) {
            if self.state.config.hide_on_launch {
                self.window_hidden_for_game = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        }

        for (_inst_id, _code) in self.state.pending_game_exited.drain(..) {
            if self.window_hidden_for_game {
                self.window_hidden_for_game = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            if !self.state.game_running {
                if let Some(mut rpc) = self.discord_rpc.take() {
                    rpc.clear_activity();
                }
            }
        }
    }

    fn render_mini_overlay(&mut self, ctx: &egui::Context, tokens: &Tokens) {
        CentralPanel::default()
            .frame(solid_hud_frame_with_config(
                tokens,
                &self.state.config.theme,
            ))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::Vec2::new(10.0, 4.0);
                    let active_name = if let Some(ref rid) = self.state.running_instance {
                        self.state
                            .instances
                            .iter()
                            .find(|i| i.id == *rid)
                            .map_or("Игра запущена", |i| i.name.as_str())
                    } else if let Some(ref sid) = self.state.selected_instance_id {
                        self.state
                            .instances
                            .iter()
                            .find(|i| i.id == *sid)
                            .map_or("Нет инстанса", |i| i.name.as_str())
                    } else {
                        "MQ Launcher"
                    };

                    ui.label(
                        egui::RichText::new(active_name)
                            .color(tokens.text)
                            .strong()
                            .size(13.0),
                    );

                    if self.state.game_running {
                        if let Some(ref rid) = self.state.running_instance.clone() {
                            if ui
                                .button(egui::RichText::new("Остановить").color(tokens.danger))
                                .clicked()
                            {
                                let _ = self.state.cmd_sender.send(UiWorkerCommand::StopGame {
                                    instance_id: rid.clone(),
                                });
                            }
                        }
                    } else if let Some(ref sid) = self.state.selected_instance_id.clone() {
                        if ui
                            .button(egui::RichText::new("Запуск").color(tokens.accent))
                            .clicked()
                        {
                            let _ = self.state.cmd_sender.send(UiWorkerCommand::LaunchInstance {
                                instance_id: sid.clone(),
                                dry_run: false,
                            });
                        }
                    }

                    ui.separator();

                    let cpu_pct = self.state.latest_cpu.total_pct;
                    let ram_mib = self.state.latest_cpu.rss_mib;
                    ui.label(
                        egui::RichText::new(format!("CPU: {cpu_pct:.1}% | RAM: {ram_mib:.0} МБ"))
                            .color(tokens.text_muted)
                            .small(),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Развернуть").clicked() {
                            self.state.mini_overlay_active = false;
                            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                                egui::WindowLevel::Normal,
                            ));
                        }
                    });
                });
            });

        if self.state.game_running || self.state.is_launching {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }

    /// Internal per-frame rendering logic executed under `catch_unwind`.
    fn render_frame_inner(&mut self, ctx: &egui::Context) {
        crate::animation::begin_animation_frame(ctx);
        #[cfg(feature = "test-hooks")]
        self.step_test_hooks(ctx);

        // Drain background worker events and IPC
        self.drain_background_and_ipc(ctx);

        let tokens = Tokens::from_config(&self.state.config.theme);
        if self.state.mini_overlay_active {
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::AlwaysOnTop,
            ));
            self.render_mini_overlay(ctx, &tokens);
            return;
        }

        // 1. Sidebar (Left)
        let accounts_list = self.state.accounts.list_accounts().to_vec();
        let active_id = self
            .state
            .accounts
            .active_account()
            .map(|a| a.id.to_string());

        let mut sidebar_action = None;
        let launcher_locale = self.state.config.launcher_locale.clone();
        let tokens = Tokens::from_config(&self.state.config.theme);

        #[allow(unused_mut)]
        let mut effective_theme = self.state.config.theme.clone();
        #[cfg(feature = "test-hooks")]
        if crate::test_hooks::is_solid_background(ctx)
            && effective_theme.preset != mq_core::ThemePreset::LiquidGlass
        {
            effective_theme.background_style = mq_core::BackgroundStyle::Solid;
        }

        let full_window_glass =
            Self::paint_full_window_glass_backdrop(ctx, &effective_theme, &tokens);

        let sidebar_state = self.build_sidebar_state();
        SidePanel::left("mq_sidebar")
            .resizable(false)
            .exact_width(sidebar_width(ctx))
            .frame(solid_sidebar_frame_with_config(
                &tokens,
                &self.state.config.theme,
            ))
            .show(ctx, |ui| {
                sidebar_action = render_sidebar_with_state(
                    ui,
                    self.current_tab,
                    &accounts_list,
                    active_id.as_deref(),
                    &launcher_locale,
                    &tokens,
                    sidebar_state,
                );
            });

        if let Some(act) = sidebar_action {
            match act {
                SidebarAction::SelectTab(t) => self.current_tab = t,
                SidebarAction::SwitchAccount(id) => {
                    let _ = self.state.accounts.select_account(&id);
                    let _ = self.state.accounts.save(&self.state.paths);
                }
                SidebarAction::OpenAccountsTab => self.current_tab = NavTab::Accounts,
                SidebarAction::ChangeLanguage(code) => {
                    self.state.config.launcher_locale = code;
                    let _ = self.state.config.save(&self.state.paths);
                }
            }
        }

        // 2. Bottom HUD (Bottom)
        let hud_data = self.build_hud_data();

        TopBottomPanel::bottom("mq_hud")
            .frame(solid_hud_frame_with_config(
                &tokens,
                &self.state.config.theme,
            ))
            .show(ctx, |ui| {
                render_bottom_hud(ui, &hud_data);
            });

        // 3. Central Panel (Active view)
        let panel_frame = egui::Frame::new()
            .fill(egui::Color32::TRANSPARENT)
            .inner_margin(egui::Margin::same(16));
        CentralPanel::default().frame(panel_frame).show(ctx, |ui| {
            if !full_window_glass {
                let bg_rect = ui.max_rect().expand(16.0);
                let bg_painter = ui.painter().with_clip_rect(bg_rect);
                paint_launcher_background(ctx, &bg_painter, bg_rect, &effective_theme, &tokens);
            }
            crate::animation::render_tab_transition(
                ui,
                "mq_main_nav_transition",
                self.current_tab,
                self.current_tab.shortcut_index() as i32,
                false,
                |ui| self.render_active_tab(ui),
            );
        });

        // 4. Modal Recovery Dialog (if previous installation terminated unexpectedly)
        self.render_recovery_dialog(ctx);

        // 5. Ephemeral Toast Notifications
        crate::components::ToastManager::sync_and_render_in_context(
            ctx,
            &tokens,
            &self.state.status_message,
        );

        // Request smooth immediate repaints when active, or 1 Hz idle refresh for live telemetry
        if self.state.active_phase.is_some()
            || self.state.game_running
            || self.state.installing_instance.is_some()
            || self.state.jobs.active_count() > 0
            || self.state.is_launching
            || self.state.is_scanning_storage
        {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(1000));
        }

        #[cfg(feature = "test-hooks")]
        self.post_frame_test_hooks(ctx);
    }

    /// Primary entry point for rendering a complete frame with catch_unwind and fallback screen.
    pub fn render_frame(&mut self, ctx: &egui::Context) {
        let frame_start = std::time::Instant::now();
        let watchdog = self.state.watchdog.clone();
        let _guard = watchdog.guard("mq_frame_update");

        // Safe 0x0 handling when minimized on Windows or collapsed on tiling WMs like Hyprland:
        // keep draining background events/IPC so downloads and `.mrpack` requests continue,
        // but skip egui layout and GPU rendering completely until the viewport is non-zero.
        if is_zero_viewport(ctx) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.drain_background_and_ipc(ctx);
            }));
            return;
        }

        // If a previous frame panic occurred, display the fallback recovery screen
        if let Some(err_msg) = &self.frame_crash_message {
            let tokens = Tokens::from_config(&self.state.config.theme);
            let mut reset_screen = false;
            let mut copy_report = false;
            let err_text = err_msg.clone();

            CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.label(
                        egui::RichText::new("Сбой при отображении интерфейса")
                            .font(egui::FontId::proportional(22.0))
                            .color(tokens.danger),
                    );
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new("Произошла ошибка при отрисовке кадра:")
                            .font(egui::FontId::proportional(14.0))
                            .color(tokens.text),
                    );
                    ui.add_space(12.0);

                    let err_frame = egui::Frame::new()
                        .fill(tokens.bg_surface)
                        .corner_radius(egui::CornerRadius::same(6))
                        .stroke(egui::Stroke::new(1.0_f32, tokens.border))
                        .inner_margin(egui::Margin::same(16));
                    err_frame.show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(&err_text)
                                .font(egui::FontId::monospace(12.0))
                                .color(tokens.text_muted),
                        );
                    });
                    ui.add_space(24.0);

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = egui::Vec2::new(12.0, 0.0);
                        if crate::components::button_primary(ui, &tokens, None, "Сбросить экран")
                            .on_hover_text("Вернуться к стандартному экрану инстансов")
                            .clicked()
                        {
                            reset_screen = true;
                        }
                        if crate::components::button_secondary(
                            ui,
                            &tokens,
                            None,
                            "Скопировать отчёт",
                        )
                        .on_hover_text("Скопировать сведения об аварии в буфер обмена")
                        .clicked()
                        {
                            copy_report = true;
                        }
                    });
                });
            });

            if reset_screen {
                self.frame_crash_message = None;
                self.current_tab = NavTab::Instances;
            }
            if copy_report {
                let report = format!(
                    "=== Отчёт об аварии интерфейса MQ Launcher v{} ===\n{}\n",
                    env!("CARGO_PKG_VERSION"),
                    err_text
                );
                ctx.copy_text(report);
                self.state.status_message =
                    Some(("Отчёт скопирован в буфер обмена".to_string(), false));
            }
            return;
        }

        // Catch any panics during main frame rendering
        let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.render_frame_inner(ctx);
        }));

        if let Err(panic_payload) = panic_result {
            let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "Неизвестная ошибка рендеринга кадра".to_string()
            };

            let backtrace = std::backtrace::Backtrace::capture().to_string();
            let _ = mq_core::crash::write_crash_report_for_panic(
                &self.state.paths,
                &panic_msg,
                None,
                "ui-render-thread",
                &backtrace,
            );

            self.frame_crash_message = Some(panic_msg);
            ctx.request_repaint();
        }

        let elapsed_ms = frame_start.elapsed().as_secs_f32() * 1000.0;
        self.state.record_frame_time_ms(elapsed_ms);
    }
}

impl eframe::App for MqApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let style = ThemeStyleParams::from_config(&self.state.config.theme);
        if style.compositor_transparent {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            Tokens::from_config(&self.state.config.theme)
                .bg_app
                .to_normalized_gamma_f32()
        }
    }

    #[cfg(feature = "test-hooks")]
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        crate::test_hooks::on_raw_input(ctx, raw_input);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.render_frame(ctx);
    }
}

fn build_wgpu_configuration() -> egui_wgpu::WgpuConfiguration {
    let setup = egui_wgpu::WgpuSetupCreateNew {
        device_descriptor: std::sync::Arc::new(|adapter| {
            let adapter_limits = adapter.limits();
            let mut required_limits = if adapter.get_info().backend == wgpu::Backend::Gl {
                wgpu::Limits::downlevel_webgl2_defaults()
            } else {
                wgpu::Limits::downlevel_defaults()
            };
            required_limits.max_texture_dimension_2d =
                adapter_limits.max_texture_dimension_2d.min(8192);
            required_limits.max_texture_dimension_1d = adapter_limits
                .max_texture_dimension_1d
                .min(required_limits.max_texture_dimension_1d);
            required_limits.max_buffer_size = adapter_limits
                .max_buffer_size
                .min(required_limits.max_buffer_size);
            wgpu::DeviceDescriptor {
                label: Some("mq egui wgpu device"),
                required_features: wgpu::Features::default(),
                required_limits,
                memory_hints: wgpu::MemoryHints::default(),
            }
        }),
        ..Default::default()
    };

    egui_wgpu::WgpuConfiguration {
        wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(setup),
        on_surface_error: std::sync::Arc::new(|err| match err {
            wgpu::SurfaceError::Outdated => {
                // Normal while minimized or mid-resize on Windows; skipping avoids 0x0 reconfigure panics.
                egui_wgpu::SurfaceErrorAction::SkipFrame
            }
            wgpu::SurfaceError::Lost => {
                // Recover cleanly after Windows lockscreen (Win+L), RDP reconnect, or GPU reset.
                tracing::warn!("wgpu surface lost; recreating swap chain");
                egui_wgpu::SurfaceErrorAction::RecreateSurface
            }
            other => {
                tracing::warn!("Dropped wgpu frame with surface error: {other}");
                egui_wgpu::SurfaceErrorAction::SkipFrame
            }
        }),
        ..Default::default()
    }
}

fn should_enable_window_transparency(theme: &mq_core::ThemeConfig) -> bool {
    if cfg!(target_os = "windows") {
        // Keep opaque Windows frames on standard DWM composition unless a transparent theme is active.
        ThemeStyleParams::from_config(theme).compositor_transparent
    } else {
        true
    }
}

fn build_native_options(renderer: eframe::Renderer, transparent: bool) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MQ Launcher")
            .with_inner_size([1180.0, 740.0])
            .with_min_inner_size([960.0, 600.0])
            .with_transparent(transparent)
            .with_decorations(true)
            .with_icon(build_app_icon()),
        renderer,
        wgpu_options: build_wgpu_configuration(),
        ..Default::default()
    }
}

fn run_native_attempt(
    state_slot: &std::cell::RefCell<Option<UiState>>,
    renderer: eframe::Renderer,
    transparent: bool,
) -> Result<(), eframe::Error> {
    let options = build_native_options(renderer, transparent);
    eframe::run_native(
        "MQ Launcher",
        options,
        Box::new(move |cc| {
            let st = state_slot.borrow_mut().take().ok_or_else(
                || -> Box<dyn std::error::Error + Send + Sync> {
                    "UI state already consumed by an earlier backend attempt".into()
                },
            )?;
            Ok(Box::new(MqApp::new(cc, st)))
        }),
    )
}

/// Launches the desktop GUI window, automatically falling back from `wgpu` to OpenGL (`glow`)
/// in `GuiBackend::Auto` when DX12/Vulkan initialization fails (e.g., Windows VMs, RDP, legacy GPUs).
pub fn run_gui(state: UiState, backend: GuiBackend) -> Result<(), eframe::Error> {
    let want_transparent = should_enable_window_transparency(&state.config.theme);
    let state_slot = std::cell::RefCell::new(Some(state));

    match backend {
        GuiBackend::Vulkan => {
            run_native_attempt(&state_slot, eframe::Renderer::Wgpu, want_transparent)
        }
        GuiBackend::Gl => {
            match run_native_attempt(&state_slot, eframe::Renderer::Glow, want_transparent) {
                Ok(()) => Ok(()),
                Err(err) if want_transparent && state_slot.borrow().is_some() => {
                    tracing::warn!(
                        "OpenGL (glow) failed with transparent surface ({err}); retrying opaque surface"
                    );
                    run_native_attempt(&state_slot, eframe::Renderer::Glow, false)
                }
                Err(err) => Err(err),
            }
        }
        GuiBackend::Auto => {
            match run_native_attempt(&state_slot, eframe::Renderer::Wgpu, want_transparent) {
                Ok(()) => Ok(()),
                Err(wgpu_err) if state_slot.borrow().is_some() => {
                    tracing::warn!(
                        "WGPU backend initialization failed ({wgpu_err}); falling back to OpenGL (glow)"
                    );
                    match run_native_attempt(&state_slot, eframe::Renderer::Glow, false) {
                        Ok(()) => Ok(()),
                        Err(glow_err) => {
                            tracing::error!(
                                "Both WGPU ({wgpu_err}) and OpenGL fallback ({glow_err}) failed"
                            );
                            Err(glow_err)
                        }
                    }
                }
                Err(wgpu_err) => Err(wgpu_err),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mq_core::ModLoaderKind;

    #[test]
    fn parse_test_action_supports_version_and_loader_variants() {
        assert_eq!(
            parse_test_action("install:26.3"),
            Some(TestAction::Install {
                mc_version: "26.3".to_string(),
                loader: ModLoaderKind::Vanilla,
            })
        );
        assert_eq!(
            parse_test_action("install:26.3:vanilla"),
            Some(TestAction::Install {
                mc_version: "26.3".to_string(),
                loader: ModLoaderKind::Vanilla,
            })
        );
        assert_eq!(
            parse_test_action("  install:1.20.1:fabric  "),
            Some(TestAction::Install {
                mc_version: "1.20.1".to_string(),
                loader: ModLoaderKind::Fabric,
            })
        );
        assert_eq!(
            parse_test_action("install:1.21.4:neoforge"),
            Some(TestAction::Install {
                mc_version: "1.21.4".to_string(),
                loader: ModLoaderKind::NeoForge,
            })
        );
        assert_eq!(parse_test_action(""), None);
        assert_eq!(parse_test_action("install:"), None);
        assert_eq!(parse_test_action("install:1.20.1:unknown"), None);
        assert_eq!(parse_test_action("install:1.20.1:fabric:extra"), None);
        assert_eq!(parse_test_action("launch:1.20.1"), None);
    }

    #[cfg(feature = "test-hooks")]
    #[test]
    fn step_test_hooks_creates_instance_and_queues_install_once() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = mq_core::MqPaths::with_root(tmp.path().to_path_buf());
        let config = mq_core::AppConfig::default();
        let pool = std::sync::Arc::new(mq_sched::ComputePool::new(2));
        let state = UiState::new(paths, config, pool).expect("ui state");

        let mut app = MqApp::from_state(state);

        std::env::set_var("MQ_TEST_ACTION", "install:26.3:vanilla");
        let ctx = egui::Context::default();

        app.step_test_hooks(&ctx);
        assert_eq!(app.state.instances.len(), 1);
        assert_eq!(app.state.instances[0].mc_version, "26.3");
        assert_eq!(app.state.instances[0].loader, ModLoaderKind::Vanilla);
        let created_id = app.state.instances[0].id.clone();
        assert_eq!(app.state.selected_instance_id, Some(created_id.clone()));
        assert_eq!(app.state.installing_instance, Some(created_id.clone()));

        match app.state.cmd_receiver.try_recv() {
            Ok(UiWorkerCommand::InstallInstance { instance_id }) => {
                assert_eq!(instance_id, created_id);
            }
            other => panic!("expected InstallInstance command, got {:?}", other.is_ok()),
        }

        // Calling step_test_hooks a second time on the same Context must not duplicate the command
        app.step_test_hooks(&ctx);
        assert!(app.state.cmd_receiver.try_recv().is_err());

        std::env::remove_var("MQ_TEST_ACTION");
    }

    #[test]
    fn test_recovery_dialog_detects_unclosed_journal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = mq_core::MqPaths::with_root(tmp.path().to_path_buf());

        // Create an unclosed journal as if a previous launch panicked during client_jar stage
        let journal = mq_core::crash::InstallJournal::start(
            &paths,
            "inst-1",
            "1.21.4",
            "fabric",
            "client_jar",
        )
        .expect("start journal");
        assert!(journal.active);

        let config = mq_core::AppConfig::default();
        let pool = std::sync::Arc::new(mq_sched::ComputePool::new(2));
        let state = UiState::new(paths.clone(), config, pool).expect("ui state");

        let mut app = MqApp::from_state(state);
        assert!(app.recovery_journal.is_some());
        let rec = app.recovery_journal.as_ref().unwrap();
        assert_eq!(rec.mc_version, "1.21.4");
        assert_eq!(rec.stage, "client_jar");

        // Verify diagnostic summary formatting
        let diag = rec.format_diagnostic_summary(&paths);
        assert!(diag.contains("1.21.4"));
        assert!(diag.contains("client_jar"));

        // Render frame with recovery dialog active
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.render_recovery_dialog(ctx);
        });

        // Closing the journal manually closes it on disk
        mq_core::crash::InstallJournal::mark_closed_at(&paths).expect("mark closed");
        assert!(mq_core::crash::InstallJournal::load_unclosed(&paths).is_none());
    }

    #[test]
    fn test_frame_crash_fallback_screen_renders_safely() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = mq_core::MqPaths::with_root(tmp.path().to_path_buf());
        let config = mq_core::AppConfig::default();
        let pool = std::sync::Arc::new(mq_sched::ComputePool::new(2));
        let state = UiState::new(paths, config, pool).expect("ui state");
        let mut app = MqApp::from_state(state);

        app.frame_crash_message = Some("Паника рендеринга кадра: division by zero".to_string());
        let ctx = egui::Context::default();

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.render_frame(ctx);
        });

        // Resetting the crash screen
        app.frame_crash_message = None;
        assert!(app.frame_crash_message.is_none());
    }

    #[tokio::test]
    async fn test_spawn_monitored_task_catches_panic_and_reports_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = mq_core::MqPaths::with_root(tmp.path().to_path_buf());
        let (resp_tx, resp_rx) = flume::bounded(16);
        let inst_id = mq_core::InstanceId::new("panicking-instance").expect("valid id");

        spawn_monitored_task(
            "test_panicking_task",
            paths.clone(),
            inst_id.clone(),
            resp_tx,
            async move {
                panic!("Умышленная ошибка внутри фоновой задачи");
            },
        );

        // Await the worker error response
        let resp = tokio::time::timeout(std::time::Duration::from_secs(3), resp_rx.recv_async())
            .await
            .expect("timeout waiting for monitored task response")
            .expect("channel recv error");

        match resp {
            UiWorkerResponse::InstanceError { instance_id, error } => {
                assert_eq!(instance_id, inst_id);
                assert!(
                    error.contains("Установка прервана из-за внутренней ошибки"),
                    "unexpected error format: {error}"
                );
                assert!(
                    error.contains("Умышленная ошибка внутри фоновой задачи"),
                    "panic message not retained: {error}"
                );
            }
            _other => panic!("expected InstanceError, got response"),
        }

        // Verify crash log was written
        let crash_latest = paths.logs_dir().join("crash-latest.log");
        assert!(
            crash_latest.is_file(),
            "crash-latest.log should be created on panic"
        );
        let crash_text = std::fs::read_to_string(&crash_latest).expect("read crash log");
        assert!(crash_text.contains("Умышленная ошибка внутри фоновой задачи"));
    }

    #[test]
    fn scan_and_clean_storage_stats_preserve_core_data() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = mq_core::MqPaths::with_root(tmp.path().to_path_buf());
        paths.ensure_dirs().expect("ensure dirs");

        let cas_blob = paths.cas_sha1_path("0123456789abcdef0123456789abcdef01234567");
        std::fs::create_dir_all(cas_blob.parent().unwrap()).unwrap();
        std::fs::write(&cas_blob, vec![1u8; 100]).unwrap();

        let inst_file = paths.instance_dir("demo").join("client.jar");
        std::fs::create_dir_all(inst_file.parent().unwrap()).unwrap();
        std::fs::write(&inst_file, vec![2u8; 200]).unwrap();

        let lib_file = paths.libraries_dir().join("lib.jar");
        std::fs::write(&lib_file, vec![3u8; 50]).unwrap();

        let java_file = paths.java_dir().join("bin");
        std::fs::write(&java_file, vec![4u8; 25]).unwrap();

        let temp_file = paths.temp_dir().join("staged.mrpack");
        std::fs::write(&temp_file, vec![5u8; 40]).unwrap();

        let icon_file = paths.cache_dir.join("catalog-icons").join("a.png");
        std::fs::create_dir_all(icon_file.parent().unwrap()).unwrap();
        std::fs::write(&icon_file, vec![6u8; 30]).unwrap();

        let authlib = paths.authlib_injector_jar();
        std::fs::create_dir_all(authlib.parent().unwrap()).unwrap();
        std::fs::write(&authlib, vec![7u8; 15]).unwrap();

        let stats = scan_storage_stats(&paths);
        assert_eq!(stats.cas_bytes, 100);
        assert_eq!(stats.cas_files, 1);
        assert_eq!(stats.instances_bytes, 200);
        assert_eq!(stats.assets_libs_bytes, 50);
        assert_eq!(stats.java_bytes, 25);
        assert_eq!(stats.temp_logs_bytes, 40 + 30 + 15);
        assert_eq!(stats.total_bytes, 100 + 200 + 50 + 25 + 85);

        let (removed, freed) = clean_temp_cache(&paths, false);
        assert_eq!(removed, 2);
        assert_eq!(freed, 70);
        assert!(!temp_file.exists());
        assert!(!icon_file.exists());
        assert!(authlib.is_file(), "authlib-injector.jar must be preserved");
        assert!(cas_blob.is_file(), "CAS blobs must be preserved");
        assert!(inst_file.is_file(), "Instance files must be preserved");
    }

    #[test]
    fn cancel_download_records_history_and_clears_operation() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = mq_core::MqPaths::with_root(tmp.path().to_path_buf());
        let config = mq_core::AppConfig::default();
        let pool = std::sync::Arc::new(mq_sched::ComputePool::new(2));
        let mut state = UiState::new(paths, config, pool).expect("ui state");

        let inst_id = mq_core::InstanceId::new("demo-inst").expect("valid id");
        state.installing_instance = Some(inst_id.clone());
        state.installing_operation = Some("Проверка файлов".to_string());
        let _ = state.cmd_sender.send(UiWorkerCommand::CancelDownload);

        let mut app = MqApp::from_state(state);
        app.handle_worker_commands();

        assert!(app.state.installing_instance.is_none());
        assert!(app.state.installing_operation.is_none());
        assert_eq!(app.state.download_history.len(), 1);
        assert_eq!(
            app.state.download_history[0].status,
            DownloadHistoryStatus::Cancelled
        );
        assert!(app.state.download_history[0]
            .detail
            .contains("Проверка файлов"));
    }

    #[test]
    fn minimized_zero_viewport_drains_commands_without_panicking() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = mq_core::MqPaths::with_root(tmp.path().to_path_buf());
        let config = mq_core::AppConfig::default();
        let pool = std::sync::Arc::new(mq_sched::ComputePool::new(2));
        let mut state = UiState::new(paths, config, pool).expect("ui state");

        let inst_id = mq_core::InstanceId::new("min-test").expect("valid id");
        state.installing_instance = Some(inst_id);
        let _ = state.cmd_sender.send(UiWorkerCommand::CancelDownload);

        let mut app = MqApp::from_state(state);
        let ctx = egui::Context::default();
        let zero_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::ZERO,
            )),
            ..Default::default()
        };
        let _ = ctx.run(zero_input, |ctx| {
            app.render_frame(ctx);
            app.render_frame_for_snapshot(ctx);
        });

        assert!(app.frame_crash_message.is_none());
        assert!(
            app.state.installing_instance.is_none(),
            "background commands must still drain while minimized at 0x0"
        );
    }

    #[test]
    fn native_options_include_icon_and_wgpu_config() {
        let opts = build_native_options(eframe::Renderer::Wgpu, false);
        assert!(opts.viewport.icon.is_some());
        assert_eq!(opts.viewport.transparent, Some(false));
        assert!(should_enable_window_transparency(
            &mq_core::ThemeConfig::preset_liquid_glass()
        ));
        assert_eq!(
            should_enable_window_transparency(&mq_core::ThemeConfig::preset_matte_dark()),
            !cfg!(target_os = "windows")
        );
    }
}
