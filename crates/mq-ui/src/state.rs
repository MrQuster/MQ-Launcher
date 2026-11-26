#![deny(clippy::await_holding_lock)]
//! Shared application state and background worker coordinator for `mq-ui`.
//!
//! Enforces non-blocking UI design (Lesson E1):
//! - UI thread NEVER performs network/disk I/O, hash verification, or JSON/ZIP parsing.
//! - Background tasks communicate via bounded `flume` channels.
//! - Frame times are monitored by `UiWatchdog` (>8 ms logs warning).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mq_auth::AccountStore;
use mq_core::{
    AppConfig, CpuSnapshot, Event, EventBus, GameLogEntry, GameLogLevel, InstanceId, ModLoaderKind,
    MqPaths, MqResult, PhaseProgress,
};
use mq_install::CasStore;
use mq_instances::{InstanceManager, InstanceManifest};
use mq_mods::{InstalledModEntry, ModConflict, SearchQuery};
use mq_net::NetEngine;
use mq_sched::{CancelEpoch, ComputePool, UiWatchdog};

use crate::install_types::{InstallError, InstallStage};

/// Max lines retained in memory for live game log console (10,000 lines).
pub const MAX_GAME_LOG_LINES: usize = 10000;

/// Max samples retained in rolling live charts (CPU, RAM, frame time, download speed).
pub const MAX_TELEMETRY_SAMPLES: usize = 60;

/// Max session entries kept in the downloads history list.
pub const MAX_DOWNLOAD_HISTORY: usize = 50;

/// Outcome of a completed download / installation entry recorded in session history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadHistoryStatus {
    Completed,
    Failed,
    Cancelled,
}

/// Historical download or installation item shown on the Downloads tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadHistoryEntry {
    pub title: String,
    pub category: String,
    pub detail: String,
    pub timestamp: String,
    pub status: DownloadHistoryStatus,
    pub instance_id: Option<InstanceId>,
}

/// Disk-usage breakdown across MQ Launcher's data directories.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageStats {
    pub total_bytes: u64,
    pub cas_bytes: u64,
    pub cas_files: usize,
    pub instances_bytes: u64,
    pub assets_libs_bytes: u64,
    pub java_bytes: u64,
    pub temp_logs_bytes: u64,
    pub scanned_at: String,
}

/// Background responses delivered back to UI thread without blocking.
pub enum UiWorkerResponse {
    CatalogResults(crate::catalog_state::CatalogPage),
    IconLoaded {
        url: String,
        image: Option<egui::ColorImage>,
    },
    ProjectLoaded {
        request_id: u64,
        result: Result<Box<crate::catalog_flow::ProjectPage>, String>,
    },
    PlanReady {
        request_id: u64,
        result: Result<crate::catalog_ops::PlanOutcome, String>,
    },
    ModpackPrepared {
        request_id: u64,
        result: Result<crate::catalog_flow::ModpackPreview, String>,
    },
    JobProgress {
        job_id: u64,
        done: usize,
        total: usize,
    },
    JobStage {
        job_id: u64,
        stage_label: String,
    },
    JobFinished {
        job_id: u64,
        result: Result<JobOutput, String>,
    },
    TelemetryExtra {
        peak_rss_mib: f64,
        open_fds: usize,
    },
    StorageStatsReady(StorageStats),
    TempCacheCleaned {
        removed_files: usize,
        freed_bytes: u64,
    },
    UpdatesChecked {
        request_id: u64,
        result: Result<Vec<crate::catalog_flow::UpdateCandidate>, String>,
    },
    ManualProgress {
        pending: Vec<mq_mods::BlockedDownload>,
        newly_done: Vec<String>,
    },
    ScanModsResults {
        mods: Vec<InstalledModEntry>,
        conflicts: Vec<ModConflict>,
    },
    ScanModsFailed(String),
    UnusedDepsCleaned {
        removed: usize,
        error: Option<String>,
    },
    OrphanedDepsFound {
        instance_id: InstanceId,
        deleted_file: String,
        orphans: Vec<String>,
    },
    PreflightResult {
        instance_id: InstanceId,
        report: mq_mods::PreflightReport,
    },
    PreflightFailed(String),
    VersionManifestFetched {
        latest_release: Option<String>,
        versions: Vec<(String, String)>,
    },
    InstanceInstallStarted(InstanceId),
    InstanceInstallCompleted(InstanceId),
    InstanceError {
        instance_id: InstanceId,
        error: String,
    },
    GameStarted(InstanceId),
    GamePid {
        instance_id: InstanceId,
        pid: u32,
    },
    GameExited {
        instance_id: InstanceId,
        exit_code: i32,
    },
    DryRunFinished(InstanceId),
    LaunchAborted(InstanceId),
    ImportArchivePicked(Option<std::path::PathBuf>),
    MclogsUploaded(Result<String, String>),
    ServerPingReady {
        ip: String,
        result: Result<mq_instances::ServerPingResult, String>,
    },
    JavaRuntimesDiscovered(Vec<mq_launch::JavaRuntime>),
    JavaDownloadFinished,
    UpdateCheckFinished(Result<Option<mq_update::ReleaseAssetInfo>, String>),
    UpdateDownloadProgress(f32),
    UpdateDownloadFailed(String),
    InstanceExported(Result<std::path::PathBuf, String>),
    InstanceDeleted {
        instance_id: InstanceId,
        name: String,
        error: Option<String>,
    },
    CrashAnalysisReady {
        instance_id: InstanceId,
        analysis: mq_launch::CrashAnalysis,
    },
    ElyByAuthUrlReady(String),
    ElyByLoginFinished(Result<String, String>),
    GameRssSamples(Vec<(InstanceId, f32)>),
    AutoFixDepsDone {
        instance_id: InstanceId,
        report: Result<mq_mods::AutoFixDepsReport, String>,
    },
    ModpackUpdateChecked {
        instance_id: InstanceId,
        result: Result<Option<mq_mods::ModpackUpdateInfo>, String>,
    },
    ModpackUpdateApplied {
        instance_id: InstanceId,
        result: Result<usize, String>,
    },
    ExternalInstancesDiscovered(Vec<mq_instances::DiscoveredExternalInstance>),
    ExternalInstanceImported(Box<Result<InstanceManifest, String>>),
    WorldsBackedUp {
        instance_id: InstanceId,
        result: Result<Vec<std::path::PathBuf>, String>,
    },
    MsaDeviceCodeReady {
        verification_uri: String,
        user_code: String,
    },
    MsaLoginFinished(Result<String, String>),
    DiscordRpcFinished(Option<mq_launch::DiscordRpcClient>),
    WorldBackedUp {
        instance_id: InstanceId,
        result: Result<std::path::PathBuf, String>,
    },
    WorldExported {
        instance_id: InstanceId,
        result: Result<std::path::PathBuf, String>,
    },
    WorldRestored {
        instance_id: InstanceId,
        file_name: String,
        result: Result<std::path::PathBuf, String>,
    },
    InstanceZipImported(Box<Result<InstanceManifest, String>>),
    WorldSizesReady {
        instance_id: InstanceId,
        sizes: HashMap<String, u64>,
    },
}

/// Successful result of an install job.
pub struct JobOutput {
    /// Instance the content ended up in (new instance for modpacks).
    pub instance_id: Option<InstanceId>,
    /// Files the user still has to download by hand.
    pub blocked: Vec<mq_mods::BlockedDownload>,
}

/// Background task messages sent from UI to worker threads.
pub enum UiWorkerCommand {
    FetchVersionManifest,
    RefreshInstances,
    RefreshAccounts,
    SearchCatalog {
        request_id: u64,
        latest: Arc<std::sync::atomic::AtomicU64>,
        source: mq_mods::CatalogSourceFilter,
        query: SearchQuery,
    },
    FetchIcon {
        url: String,
        edge: u32,
    },
    LoadProject {
        request_id: u64,
        source: mq_mods::CatalogSource,
        project_id: String,
        kind: mq_mods::ProjectKind,
    },
    PlanInstall {
        request_id: u64,
        req: crate::catalog_ops::PlanRequest,
    },
    PrepareModpack {
        request_id: u64,
        version: mq_mods::ProjectVersion,
    },
    CheckUpdates {
        request_id: u64,
        instance_id: InstanceId,
        mc_version: String,
        loader: ModLoaderKind,
    },
    ApplyUpdate {
        job_id: u64,
        instance_id: InstanceId,
        candidate: Box<crate::catalog_flow::UpdateCandidate>,
    },
    PreviewLocalPack {
        request_id: u64,
        path: std::path::PathBuf,
    },
    StartContentJob {
        job_id: u64,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        instance_id: InstanceId,
        kind: mq_mods::ProjectKind,
        source: mq_mods::CatalogSource,
        downloads: Vec<mq_mods::ResolvedModDownload>,
        blocked: Vec<mq_mods::BlockedDownload>,
    },
    StartModpackJob {
        job_id: u64,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        instance_name: String,
        preview: crate::catalog_flow::ModpackPreview,
    },
    PollManual {
        dropped: Vec<std::path::PathBuf>,
    },
    PickImportArchive,
    InstallMod {
        instance_id: InstanceId,
        project_id_or_slug: String,
        mc_version: String,
        loader: ModLoaderKind,
    },
    InstallModpack {
        path_or_slug: String,
        instance_name: Option<String>,
    },
    ScanInstanceMods {
        instance_id: InstanceId,
    },
    CleanupUnusedDeps {
        instance_id: InstanceId,
    },
    ToggleMod {
        instance_id: InstanceId,
        file_name: String,
    },
    DeleteMod {
        instance_id: InstanceId,
        file_name: String,
    },
    CleanupOrphanFiles {
        instance_id: InstanceId,
        orphan_files: Vec<String>,
    },
    RunPreflight {
        instance_id: InstanceId,
    },
    AutoFixPreflight {
        instance_id: InstanceId,
        report: mq_mods::PreflightReport,
    },
    VerifyInstance {
        instance_id: InstanceId,
    },
    VerifyAndRepairInstance {
        instance_id: InstanceId,
    },
    ReinstallInstance {
        instance_id: InstanceId,
    },
    InstallInstance {
        instance_id: InstanceId,
    },
    CancelDownload,
    ScanStorageStats,
    CleanTempCache,
    StopGame {
        instance_id: InstanceId,
    },
    LaunchInstance {
        instance_id: InstanceId,
        dry_run: bool,
    },
    UploadMclogs {
        log_text: String,
    },
    PingServer {
        ip: String,
    },
    DiscoverJavaRuntimes,
    CheckLauncherUpdate {
        current_version: String,
    },
    DownloadLauncherUpdate {
        asset: Box<mq_update::ReleaseAssetInfo>,
    },
    ExportInstance {
        instance_id: InstanceId,
    },
    DeleteInstance {
        instance_id: InstanceId,
        name: String,
    },
    AnalyzeCrash {
        instance_id: InstanceId,
        exit_code: i32,
        lines: Vec<String>,
    },
    LaunchInstanceQuickPlay {
        instance_id: InstanceId,
        quick_play: mq_launch::QuickPlayTarget,
    },
    AutoInstallMissingDeps {
        instance_id: InstanceId,
        missing_ids: Vec<String>,
    },
    CheckModpackUpdate {
        instance_id: InstanceId,
    },
    ApplyModpackUpdate {
        instance_id: InstanceId,
        info: mq_mods::ModpackUpdateInfo,
    },
    DiscoverExternalInstances,
    ImportExternalInstance {
        item: mq_instances::DiscoveredExternalInstance,
    },
    AutoBackupWorlds {
        instance_id: InstanceId,
    },
    BackupWorld {
        instance_id: InstanceId,
        folder_name: String,
    },
    ExportWorld {
        instance_id: InstanceId,
        folder_name: String,
        export_path: std::path::PathBuf,
    },
    RestoreWorld {
        instance_id: InstanceId,
        file_name: String,
    },
    ImportInstanceZip {
        path: std::path::PathBuf,
        custom_name: Option<String>,
    },
    ComputeWorldSizes {
        instance_id: InstanceId,
    },
}

/// Central state model consumed by `eframe::App` in `mq-ui`.
pub struct UiState {
    pub paths: MqPaths,
    pub config: AppConfig,
    pub accounts: AccountStore,
    pub instances: Vec<InstanceManifest>,
    pub selected_instance_id: Option<InstanceId>,

    // Subsystems
    pub instance_mgr: InstanceManager,
    pub net: Arc<NetEngine>,
    pub cas: CasStore,
    pub compute_pool: Arc<ComputePool>,
    pub cancel_epoch: Arc<CancelEpoch>,
    pub event_bus: EventBus,
    pub watchdog: Arc<UiWatchdog>,

    // Channels
    pub cmd_sender: flume::Sender<UiWorkerCommand>,
    pub cmd_receiver: flume::Receiver<UiWorkerCommand>,
    pub resp_sender: flume::Sender<UiWorkerResponse>,
    pub resp_receiver: flume::Receiver<UiWorkerResponse>,

    // Telemetry & Status
    pub latest_cpu: CpuSnapshot,
    pub cpu_history: VecDeque<f32>,
    pub rss_history: VecDeque<f64>,
    pub peak_rss_mib: f64,
    pub open_fds: usize,
    pub frame_time_ms_history: VecDeque<f32>,
    pub last_frame_ms: f32,
    pub max_frame_ms: f32,
    pub gpu_backend_label: String,
    pub storage_stats: Option<StorageStats>,
    pub is_scanning_storage: bool,
    pub telemetry_stop: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub active_phase: Option<String>,
    pub phase_progress: Option<PhaseProgress>,
    pub download_speed_history: VecDeque<f32>,
    pub download_history: VecDeque<DownloadHistoryEntry>,
    pub status_message: Option<(String, bool)>, // (msg, is_error)

    // Game Console & Execution
    pub game_running: bool,
    pub running_instance: Option<InstanceId>,
    pub running_instances: HashMap<InstanceId, Option<u32>>,
    pub stopped_by_user: HashSet<InstanceId>,
    pub is_launching: bool,
    pub installing_instance: Option<InstanceId>,
    pub installing_operation: Option<String>,
    pub last_failed_install_instance: Option<InstanceId>,
    pub install_error: Option<String>,
    pub detailed_install_error: Option<InstallError>,
    pub current_install_stage: InstallStage,
    pub install_retries_count: u32,
    pub install_eta_secs: Option<u64>,
    pub last_game_exit_code: Option<i32>,
    pub game_logs: VecDeque<GameLogEntry>,
    pub log_filter: Option<GameLogLevel>,

    // Mods Screen
    pub installed_mods: Vec<InstalledModEntry>,
    pub mod_conflicts: Vec<ModConflict>,
    pub is_scanning_mods: bool,

    // Catalog Screen
    pub catalog: crate::catalog_state::CatalogState,
    pub project: crate::catalog_flow::ProjectViewState,
    pub install: Option<crate::catalog_flow::InstallDialog>,
    pub jobs: crate::catalog_flow::JobQueue,
    pub updates: crate::catalog_flow::UpdatesState,
    pub manual: crate::catalog_flow::ManualDownloadState,
    pub manual_watcher: Option<crate::catalog_ops::SharedWatcher>,
    pub install_seq: u64,

    // Dialogs & Versions
    pub show_create_instance_dialog: bool,
    pub create_inst_name: String,
    pub create_inst_mc_ver: String,
    pub create_inst_loader: ModLoaderKind,
    pub create_inst_ram_mb: u32,
    pub create_inst_java_path: String,
    pub latest_release: Option<String>,
    pub remote_versions: Vec<(String, String)>, // (id, release_type)
    pub is_fetching_manifest: bool,

    // Crash Diagnostics
    pub crash_analysis: Option<mq_launch::CrashAnalysis>,
    pub crash_analysis_instance: Option<InstanceId>,
    pub mclogs_url: Option<String>,
    pub is_uploading_mclogs: bool,

    // Mod Updates
    pub mod_updates: BTreeMap<String, mq_mods::ProjectVersion>,
    pub is_checking_mod_updates: bool,

    // Live Server Pings
    pub server_pings: HashMap<String, mq_instances::ServerPingResult>,
    pub pinging_servers: HashSet<String>,

    // Cached Java runtime discovery (offloaded from UI thread)
    pub discovered_java_runtimes: Option<Vec<mq_launch::JavaRuntime>>,
    pub is_discovering_java: bool,
    pub is_downloading_java: bool,

    // Pending async results consumed by Settings / Accounts views
    pub pending_update_check: Option<Result<Option<mq_update::ReleaseAssetInfo>, String>>,
    pub pending_update_progress: Option<f32>,
    pub pending_update_error: Option<String>,
    pub ely_auth_url: Option<String>,
    pub ely_login_done: Option<Result<String, String>>,
    pub msa_device_code: Option<(String, String)>,
    pub msa_login_done: Option<Result<String, String>>,

    // Cached per-instance install readiness to avoid per-frame disk I/O on the UI thread
    install_status_cache: RefCell<HashMap<InstanceId, (bool, Instant)>>,

    // Favorites
    pub favorite_instances: HashSet<String>,

    // Dialogs
    pub show_import_dialog: bool,
    pub import_archive_path: String,
    pub import_instance_name: String,
    pub show_duplicate_dialog: bool,
    pub duplicate_target_name: String,
    pub show_delete_confirm_dialog: bool,

    // Pending .mrpack import from CLI / IPC
    pub pending_mrpack_path: Option<std::path::PathBuf>,
    pub mrpack_ipc_receiver:
        Option<Arc<std::sync::Mutex<std::sync::mpsc::Receiver<std::path::PathBuf>>>>,
    pub picked_import_path: Option<std::path::PathBuf>,
    pub orphaned_deps_prompt: Option<(InstanceId, String, Vec<String>)>,
    pub preflight_report: Option<(InstanceId, mq_mods::PreflightReport)>,
    pub gpu_diagnostics: Option<mq_mods::shaders::GpuDiagnostics>,

    // v2.8.0 state additions
    pub game_rss_history:
        std::collections::HashMap<mq_core::InstanceId, std::collections::VecDeque<f32>>,
    pub mini_overlay_active: bool,
    pub active_bisect: Option<(mq_core::InstanceId, mq_mods::BisectSession)>,
    pub modpack_update_available:
        std::collections::HashMap<mq_core::InstanceId, mq_mods::ModpackUpdateInfo>,
    pub external_instances: Vec<mq_instances::DiscoveredExternalInstance>,
    pub show_external_import_modal: bool,
    pub is_auto_fixing_deps: bool,
    pub is_checking_modpack_update: bool,
    pub game_session_starts: std::collections::HashMap<mq_core::InstanceId, std::time::Instant>,
    pub running_pids:
        std::sync::Arc<parking_lot::Mutex<std::collections::HashMap<mq_core::InstanceId, u32>>>,
    pub pending_game_started: Vec<InstanceId>,
    pub pending_game_exited: Vec<(InstanceId, i32)>,
    pub launch_cancel_flags: HashMap<InstanceId, Arc<std::sync::atomic::AtomicBool>>,
    pub pending_discord_rpc: Option<mq_launch::DiscordRpcClient>,
    pub discord_rpc_busy: bool,
    pub discord_rpc_dirty: bool,
    pub worlds_cache_gen: u64,
    pub world_sizes: HashMap<(InstanceId, String), u64>,
    pub computing_world_sizes: HashSet<InstanceId>,
}

impl UiState {
    pub fn new(
        paths: MqPaths,
        config: AppConfig,
        compute_pool: Arc<ComputePool>,
    ) -> MqResult<Self> {
        let accounts = AccountStore::load_or_default(&paths)?;
        let instance_mgr = InstanceManager::new(paths.clone());
        let instances = instance_mgr.list_instances().unwrap_or_default();
        let selected_instance_id = instances.first().map(|i| i.id.clone());

        let cas = CasStore::new(paths.clone());
        let cancel_epoch = Arc::new(CancelEpoch::new());
        let event_bus = EventBus::new();
        let net = Arc::new(NetEngine::with_components(
            mq_net::NetConfig {
                max_connections: config.max_connections,
                ..Default::default()
            },
            Arc::new(mq_sched::BufferPool::new()),
            cancel_epoch.clone(),
            Some(event_bus.clone()),
        )?);

        let (cmd_sender, cmd_receiver) = flume::unbounded();
        let (resp_sender, resp_receiver) = flume::bounded(1024);

        Ok(Self {
            paths,
            config,
            accounts,
            instances,
            selected_instance_id,
            instance_mgr,
            net,
            cas,
            compute_pool,
            cancel_epoch,
            event_bus,
            watchdog: Arc::new(UiWatchdog::default()),
            cmd_sender,
            cmd_receiver,
            resp_sender,
            resp_receiver,
            latest_cpu: CpuSnapshot {
                total_pct: 0.0,
                per_core: Vec::new(),
                rss_mib: 0.0,
            },
            cpu_history: VecDeque::with_capacity(MAX_TELEMETRY_SAMPLES),
            rss_history: VecDeque::with_capacity(MAX_TELEMETRY_SAMPLES),
            peak_rss_mib: 0.0,
            open_fds: 0,
            frame_time_ms_history: VecDeque::with_capacity(MAX_TELEMETRY_SAMPLES),
            last_frame_ms: 0.0,
            max_frame_ms: 0.0,
            gpu_backend_label: "Auto".to_string(),
            storage_stats: None,
            is_scanning_storage: false,
            telemetry_stop: None,
            active_phase: None,
            phase_progress: None,
            download_speed_history: VecDeque::with_capacity(MAX_TELEMETRY_SAMPLES),
            download_history: VecDeque::with_capacity(MAX_DOWNLOAD_HISTORY),
            status_message: Some(("MQ Launcher готов к работе".to_string(), false)),
            game_running: false,
            running_instance: None,
            running_instances: HashMap::new(),
            stopped_by_user: HashSet::new(),
            is_launching: false,
            installing_instance: None,
            installing_operation: None,
            last_failed_install_instance: None,
            install_error: None,
            detailed_install_error: None,
            current_install_stage: InstallStage::Manifest,
            install_retries_count: 0,
            install_eta_secs: None,
            last_game_exit_code: None,
            game_logs: VecDeque::with_capacity(MAX_GAME_LOG_LINES),
            log_filter: None,
            installed_mods: Vec::new(),
            mod_conflicts: Vec::new(),
            is_scanning_mods: false,
            catalog: crate::catalog_state::CatalogState::default(),
            project: crate::catalog_flow::ProjectViewState::default(),
            install: None,
            jobs: crate::catalog_flow::JobQueue::default(),
            updates: crate::catalog_flow::UpdatesState::default(),
            manual: crate::catalog_flow::ManualDownloadState::default(),
            manual_watcher: None,
            install_seq: 0,
            show_create_instance_dialog: false,
            create_inst_name: String::new(),
            create_inst_mc_ver: String::new(),
            create_inst_loader: ModLoaderKind::Fabric,
            create_inst_ram_mb: 4096,
            create_inst_java_path: String::new(),
            latest_release: None,
            remote_versions: Vec::new(),
            is_fetching_manifest: false,
            crash_analysis: None,
            crash_analysis_instance: None,
            mclogs_url: None,
            is_uploading_mclogs: false,
            mod_updates: BTreeMap::new(),
            is_checking_mod_updates: false,
            server_pings: HashMap::new(),
            pinging_servers: HashSet::new(),
            discovered_java_runtimes: None,
            is_discovering_java: false,
            is_downloading_java: false,
            pending_update_check: None,
            pending_update_progress: None,
            pending_update_error: None,
            ely_auth_url: None,
            ely_login_done: None,
            msa_device_code: None,
            msa_login_done: None,
            install_status_cache: RefCell::new(HashMap::new()),
            favorite_instances: HashSet::new(),
            show_import_dialog: false,
            import_archive_path: String::new(),
            import_instance_name: String::new(),
            show_duplicate_dialog: false,
            duplicate_target_name: String::new(),
            show_delete_confirm_dialog: false,
            pending_mrpack_path: None,
            mrpack_ipc_receiver: None,
            picked_import_path: None,
            orphaned_deps_prompt: None,
            preflight_report: None,
            gpu_diagnostics: None,
            game_rss_history: HashMap::new(),
            mini_overlay_active: false,
            active_bisect: None,
            modpack_update_available: HashMap::new(),
            external_instances: Vec::new(),
            show_external_import_modal: false,
            is_auto_fixing_deps: false,
            is_checking_modpack_update: false,
            game_session_starts: HashMap::new(),
            running_pids: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            pending_game_started: Vec::new(),
            pending_game_exited: Vec::new(),
            launch_cancel_flags: HashMap::new(),
            pending_discord_rpc: None,
            discord_rpc_busy: false,
            discord_rpc_dirty: false,
            worlds_cache_gen: 0,
            world_sizes: HashMap::new(),
            computing_world_sizes: HashSet::new(),
        })
    }

    /// Number of currently active installations or download tasks (instance installs + catalog jobs).
    pub fn active_download_count(&self) -> usize {
        self.jobs.active_count()
            + usize::from(self.installing_instance.is_some())
            + usize::from(
                self.installing_instance.is_none()
                    && self.jobs.active_count() == 0
                    && self.active_phase.is_some(),
            )
    }

    /// Prepends an entry to `download_history`, bounded at [`MAX_DOWNLOAD_HISTORY`].
    pub fn record_download_history(
        &mut self,
        title: impl Into<String>,
        category: impl Into<String>,
        detail: impl Into<String>,
        status: DownloadHistoryStatus,
        instance_id: Option<InstanceId>,
    ) {
        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
        if self.download_history.len() >= MAX_DOWNLOAD_HISTORY {
            self.download_history.pop_back();
        }
        self.download_history.push_front(DownloadHistoryEntry {
            title: title.into(),
            category: category.into(),
            detail: detail.into(),
            timestamp,
            status,
            instance_id,
        });
    }

    /// Records the elapsed frame duration in milliseconds into the rolling sparkline buffer.
    pub fn record_frame_time_ms(&mut self, ms: f32) {
        if !ms.is_finite() || ms < 0.0 {
            return;
        }
        self.last_frame_ms = ms;
        if ms > self.max_frame_ms {
            self.max_frame_ms = ms;
        }
        push_rolling_sample(&mut self.frame_time_ms_history, ms);
    }

    /// Human-readable localized phase label for the bottom HUD and Downloads summary.
    pub fn active_hud_phase_label(&self) -> Option<String> {
        let is_en = self.config.launcher_locale.starts_with("en");
        if let Some(phase) = self.active_phase.as_deref() {
            let trimmed = phase.trim();
            if !trimmed.is_empty() {
                let localized = match (trimmed, is_en) {
                    ("install_mods", false) => "Скачивание модов",
                    ("install_mods", true) => "Downloading mods",
                    ("install_cfpack" | "install_mrpack", false) => "Установка сборки",
                    ("install_cfpack" | "install_mrpack", true) => "Installing modpack",
                    ("verify", false) => "Проверка файлов",
                    ("verify", true) => "Verifying files",
                    ("verify_and_repair", false) => "Проверка и починка",
                    ("verify_and_repair", true) => "Verifying & repairing",
                    ("install_game", false) => "Установка Minecraft",
                    ("install_game", true) => "Installing Minecraft",
                    ("reinstall", false) => "Переустановка инстанса",
                    ("reinstall", true) => "Reinstalling instance",
                    ("manifest", false) => "Манифест версии",
                    ("manifest", true) => "Version manifest",
                    ("assets", false) => "Загрузка ассетов",
                    ("assets", true) => "Downloading assets",
                    ("libraries", false) => "Загрузка библиотек",
                    ("libraries", true) => "Downloading libraries",
                    ("natives", false) => "Нативные библиотеки",
                    ("natives", true) => "Native libraries",
                    ("java", false) => "Установка Java",
                    ("java", true) => "Installing Java",
                    ("client", false) => "Загрузка клиента",
                    ("client", true) => "Downloading client",
                    _ => return Some(self.current_install_stage.full_localized_name().to_string()),
                };
                return Some(localized.to_string());
            }
        }
        if let Some(ref inst_id) = self.installing_instance {
            let name = self
                .instances
                .iter()
                .find(|i| &i.id == inst_id)
                .map_or_else(|| inst_id.as_str(), |i| i.name.as_str());
            return Some(if is_en {
                format!("Installing: {name}")
            } else {
                format!("Установка: {name}")
            });
        }
        if let Some(job) = self.jobs.jobs.iter().find(|j| j.is_active()) {
            return Some(if is_en {
                format!("Downloading: {}", job.title)
            } else {
                format!("Загрузка: {}", job.title)
            });
        }
        None
    }

    /// Completes a queue job and, if needed, opens the manual-download dialog.
    pub fn finish_job(&mut self, job_id: u64, result: Result<JobOutput, String>) {
        let meta = self.jobs.jobs.iter().find(|j| j.id == job_id).map(|j| {
            (
                j.title.clone(),
                j.target_label.clone(),
                j.total_files,
                j.is_cancel_requested(),
                matches!(
                    j.retry,
                    Some(crate::catalog_flow::RetrySpec::Modpack { .. })
                ),
            )
        });

        match result {
            Ok(out) => {
                self.invalidate_install_cache(out.instance_id.as_ref());
                self.jobs.finish(job_id, Ok(()));
                let _ = self.cmd_sender.send(UiWorkerCommand::RefreshInstances);
                let target_id = out
                    .instance_id
                    .clone()
                    .or_else(|| self.selected_instance_id.clone());
                if let Some(ref id) = target_id {
                    let _ = self.cmd_sender.send(UiWorkerCommand::ScanInstanceMods {
                        instance_id: id.clone(),
                    });
                }
                self.status_message = Some(("Установка контента завершена".to_string(), false));
                if let Some((title, target, total, _, is_modpack)) = meta {
                    let cat = if is_modpack {
                        "Сборка"
                    } else {
                        "Каталог"
                    };
                    let detail = format!("{target} ({total} ф.)");
                    self.record_download_history(
                        title,
                        cat,
                        detail,
                        DownloadHistoryStatus::Completed,
                        target_id.clone(),
                    );
                }
                if let Some(id) = out.instance_id {
                    self.selected_instance_id = Some(id.clone());
                    if !out.blocked.is_empty() {
                        self.open_manual_dialog(&id, out.blocked);
                    }
                }
            }
            Err(e) => {
                self.jobs.finish(job_id, Err(e.clone()));
                self.status_message = Some((format!("Ошибка установки: {e}"), true));
                if let Some((title, target, _, was_cancelled, is_modpack)) = meta {
                    let cat = if is_modpack {
                        "Сборка"
                    } else {
                        "Каталог"
                    };
                    let st = if was_cancelled {
                        DownloadHistoryStatus::Cancelled
                    } else {
                        DownloadHistoryStatus::Failed
                    };
                    self.record_download_history(title, cat, format!("{target}: {e}"), st, None);
                }
            }
        }
        if self.jobs.active_count() == 0 && self.installing_instance.is_none() {
            self.active_phase = None;
            self.phase_progress = None;
            self.install_eta_secs = None;
        }
    }

    /// Opens the manual-download dialog for `blocked` files of `instance_id`.
    pub fn open_manual_dialog(
        &mut self,
        instance_id: &InstanceId,
        blocked: Vec<mq_mods::BlockedDownload>,
    ) {
        let instance_dir = self.paths.instance_dir(instance_id.as_str());
        let watch_dir =
            crate::catalog_flow::default_downloads_dir().unwrap_or_else(|| self.paths.temp_dir());
        self.manual = crate::catalog_flow::ManualDownloadState {
            open: true,
            instance_label: instance_id.to_string(),
            pending: blocked.clone(),
            watch_dir_label: crate::opener::strip_windows_verbatim_prefix(
                &watch_dir.display().to_string(),
            ),
            ..Default::default()
        };
        self.manual_watcher = Some(Arc::new(parking_lot::Mutex::new(
            mq_mods::BlockedDownloadsWatcher::new(instance_dir, watch_dir, blocked),
        )));
    }

    /// Non-blocking drain of background worker events (up to 256 events per frame).
    pub fn drain_events(&mut self) {
        while let Ok(resp) = self.resp_receiver.try_recv() {
            match resp {
                UiWorkerResponse::CatalogResults(page) => {
                    self.catalog.apply_page(page);
                }
                UiWorkerResponse::ProjectLoaded { request_id, result } => {
                    self.project.apply(request_id, result.map(|p| *p));
                }
                UiWorkerResponse::PlanReady { request_id, result } => {
                    if let Some(d) = self.install.as_mut() {
                        if d.request_id == request_id {
                            d.loading = false;
                            match result {
                                Ok(o) => {
                                    d.error = None;
                                    d.titles.extend(o.titles);
                                    d.sources.extend(o.sources);
                                    d.plan = Some(o.plan);
                                }
                                Err(e) => {
                                    d.plan = None;
                                    d.error = Some(e);
                                }
                            }
                        }
                    }
                }
                UiWorkerResponse::ModpackPrepared { request_id, result } => {
                    if let Some(d) = self.install.as_mut() {
                        if d.request_id == request_id {
                            d.loading = false;
                            match result {
                                Ok(p) => d.preview = Some(p),
                                Err(e) => d.error = Some(e),
                            }
                        }
                    }
                }
                UiWorkerResponse::JobProgress {
                    job_id,
                    done,
                    total,
                } => self.jobs.progress(job_id, done, total),
                UiWorkerResponse::JobStage {
                    job_id,
                    stage_label,
                } => self.jobs.set_stage(job_id, stage_label),
                UiWorkerResponse::UpdatesChecked { request_id, result } => {
                    if request_id == self.updates.request_id {
                        self.updates.checking = false;
                        self.updates.checked = true;
                        match result {
                            Ok(items) => self.updates.items = items,
                            Err(e) => self.updates.error = Some(e),
                        }
                    }
                }
                UiWorkerResponse::JobFinished { job_id, result } => {
                    self.finish_job(job_id, result);
                }
                UiWorkerResponse::TelemetryExtra {
                    peak_rss_mib,
                    open_fds,
                } => {
                    if peak_rss_mib.is_finite() && peak_rss_mib > self.peak_rss_mib {
                        self.peak_rss_mib = peak_rss_mib;
                    }
                    self.open_fds = open_fds;
                }
                UiWorkerResponse::StorageStatsReady(stats) => {
                    self.storage_stats = Some(stats);
                    self.is_scanning_storage = false;
                }
                UiWorkerResponse::TempCacheCleaned {
                    removed_files,
                    freed_bytes,
                } => {
                    let freed_str = crate::catalog_flow::format_size(freed_bytes);
                    self.status_message = Some((
                        format!("Очищено временных файлов: {removed_files} ({freed_str})"),
                        false,
                    ));
                    if !self.is_scanning_storage {
                        self.is_scanning_storage = true;
                        let _ = self.cmd_sender.send(UiWorkerCommand::ScanStorageStats);
                    }
                }
                UiWorkerResponse::ImportArchivePicked(path) => {
                    self.picked_import_path = path;
                }
                UiWorkerResponse::ManualProgress {
                    pending,
                    newly_done,
                } => {
                    self.manual.apply_progress(pending, newly_done);
                }
                UiWorkerResponse::IconLoaded { url, image } => {
                    self.catalog.icons.insert(
                        url,
                        match image {
                            Some(img) => crate::catalog_state::IconSlot::Ready(img),
                            None => crate::catalog_state::IconSlot::Failed,
                        },
                    );
                }
                UiWorkerResponse::ScanModsResults { mods, conflicts } => {
                    self.installed_mods = mods;
                    self.mod_conflicts = conflicts;
                    self.is_scanning_mods = false;
                }
                UiWorkerResponse::ScanModsFailed(err) => {
                    self.is_scanning_mods = false;
                    self.status_message = Some((format!("Ошибка сканирования модов: {err}"), true));
                }
                UiWorkerResponse::UnusedDepsCleaned { removed, error } => {
                    self.status_message = Some(match error {
                        Some(e) => (format!("Не удалось очистить зависимости: {e}"), true),
                        None if removed == 0 => {
                            ("Неиспользуемых зависимостей не найдено".to_string(), false)
                        }
                        None => (
                            format!("Удалено неиспользуемых зависимостей: {removed}"),
                            false,
                        ),
                    });
                }
                UiWorkerResponse::OrphanedDepsFound {
                    instance_id,
                    deleted_file,
                    orphans,
                } => {
                    if !orphans.is_empty() {
                        self.orphaned_deps_prompt = Some((instance_id, deleted_file, orphans));
                    }
                }
                UiWorkerResponse::PreflightResult {
                    instance_id,
                    report,
                } => {
                    if report.issues.is_empty() {
                        self.preflight_report = None;
                        self.status_message = Some((
                            "Проверка зависимостей: проблем не найдено".to_string(),
                            false,
                        ));
                    } else {
                        self.preflight_report = Some((instance_id, report));
                    }
                }
                UiWorkerResponse::PreflightFailed(err) => {
                    self.status_message =
                        Some((format!("Ошибка проверки зависимостей: {err}"), true));
                }
                UiWorkerResponse::VersionManifestFetched {
                    latest_release,
                    versions,
                } => {
                    if let Some(rel) = latest_release {
                        self.latest_release = Some(rel.clone());
                        if self.create_inst_mc_ver.is_empty() {
                            self.create_inst_mc_ver = rel;
                        }
                    }
                    if !versions.is_empty() {
                        self.remote_versions = versions;
                    }
                    self.is_fetching_manifest = false;
                }
                UiWorkerResponse::InstanceInstallStarted(id) => {
                    self.invalidate_install_cache(Some(&id));
                    self.installing_instance = Some(id);
                    self.last_failed_install_instance = None;
                    self.install_error = None;
                    self.detailed_install_error = None;
                    self.install_retries_count = 0;
                    self.install_eta_secs = None;
                    self.download_speed_history.clear();
                    self.current_install_stage = InstallStage::Manifest;
                }
                UiWorkerResponse::InstanceInstallCompleted(id) => {
                    self.invalidate_install_cache(Some(&id));
                    if self.installing_instance.as_ref() == Some(&id) {
                        self.installing_instance = None;
                    }
                    let op = self
                        .installing_operation
                        .take()
                        .unwrap_or_else(|| "Установка инстанса".to_string());
                    let (inst_title, inst_detail) =
                        self.instances.iter().find(|i| i.id == id).map_or_else(
                            || (id.as_str().to_string(), op.clone()),
                            |i| {
                                (
                                    i.name.clone(),
                                    format!("{op} · {} ({})", i.mc_version, i.loader.as_str()),
                                )
                            },
                        );
                    self.record_download_history(
                        inst_title,
                        "Инстанс",
                        inst_detail,
                        DownloadHistoryStatus::Completed,
                        Some(id),
                    );
                    self.install_eta_secs = None;
                    self.active_phase = None;
                    self.phase_progress = None;
                    self.status_message = Some(("Инстанс успешно установлен".to_string(), false));
                }
                UiWorkerResponse::InstanceError { instance_id, error } => {
                    self.invalidate_install_cache(Some(&instance_id));
                    self.installing_instance = None;
                    self.last_failed_install_instance = Some(instance_id.clone());
                    let op = self
                        .installing_operation
                        .take()
                        .unwrap_or_else(|| "Установка инстанса".to_string());
                    let inst_title = self
                        .instances
                        .iter()
                        .find(|i| i.id == instance_id)
                        .map_or_else(|| instance_id.as_str().to_string(), |i| i.name.clone());
                    self.record_download_history(
                        inst_title,
                        "Инстанс",
                        format!("{op}: {error}"),
                        DownloadHistoryStatus::Failed,
                        Some(instance_id),
                    );
                    self.is_launching = false;
                    self.install_error = Some(error.clone());
                    let stage = self.current_install_stage;
                    self.detailed_install_error = Some(InstallError::from_error_str(stage, &error));
                    self.status_message = Some((error, true));
                }
                UiWorkerResponse::GameStarted(id) => {
                    self.is_launching = false;
                    if self
                        .launch_cancel_flags
                        .get(&id)
                        .is_some_and(|f| f.load(std::sync::atomic::Ordering::SeqCst))
                    {
                        continue;
                    }
                    self.game_running = true;
                    self.running_instances.entry(id.clone()).or_insert(None);
                    self.running_instance = Some(id.clone());
                    self.pending_game_started.push(id.clone());
                    if self.crash_analysis_instance.as_ref() == Some(&id) {
                        self.crash_analysis = None;
                        self.crash_analysis_instance = None;
                        self.mclogs_url = None;
                        self.is_uploading_mclogs = false;
                    }
                }
                UiWorkerResponse::GamePid { instance_id, pid } => {
                    if self
                        .launch_cancel_flags
                        .get(&instance_id)
                        .is_some_and(|f| f.load(std::sync::atomic::Ordering::SeqCst))
                    {
                        std::thread::spawn(move || {
                            let _ = mq_launch::kill_process_tree(pid);
                        });
                        continue;
                    }
                    self.running_instances
                        .insert(instance_id.clone(), Some(pid));
                    self.running_pids.lock().insert(instance_id, pid);
                }
                UiWorkerResponse::GameExited {
                    instance_id,
                    exit_code,
                } => {
                    self.launch_cancel_flags.remove(&instance_id);
                    self.is_launching = false;
                    self.pending_game_exited
                        .push((instance_id.clone(), exit_code));
                    self.running_instances.remove(&instance_id);
                    self.running_pids.lock().remove(&instance_id);
                    if let Some(start_time) = self.game_session_starts.remove(&instance_id) {
                        let dur_secs = start_time.elapsed().as_secs();
                        if let Some(inst) = self.instances.iter_mut().find(|i| i.id == instance_id)
                        {
                            inst.total_playtime_secs =
                                inst.total_playtime_secs.saturating_add(dur_secs);
                            inst.last_played = Some(chrono::Utc::now());
                            let _ = self.instance_mgr.save_instance(inst);
                        }
                    }
                    self.game_running = !self.running_instances.is_empty();
                    self.running_instance = self.running_instances.keys().next().cloned();
                    self.last_game_exit_code = Some(exit_code);
                    let was_stopped = self.stopped_by_user.remove(&instance_id);
                    if was_stopped {
                        self.status_message =
                            Some(("Игра остановлена пользователем".to_string(), false));
                        if self.crash_analysis_instance.as_ref() == Some(&instance_id) {
                            self.crash_analysis = None;
                            self.crash_analysis_instance = None;
                        }
                    } else {
                        self.status_message = Some((
                            format!("Игра завершилась с кодом выхода {}", exit_code),
                            exit_code != 0,
                        ));
                        if exit_code != 0 {
                            let lines: Vec<String> = self
                                .game_logs
                                .iter()
                                .filter(|l| l.instance == instance_id.as_str())
                                .map(|l| l.line.clone())
                                .collect();
                            let _ = self.cmd_sender.send(UiWorkerCommand::AnalyzeCrash {
                                instance_id,
                                exit_code,
                                lines,
                            });
                        } else if self.crash_analysis_instance.as_ref() == Some(&instance_id) {
                            self.crash_analysis = None;
                            self.crash_analysis_instance = None;
                        }
                    }
                }
                UiWorkerResponse::DryRunFinished(_) => {
                    self.is_launching = false;
                }
                UiWorkerResponse::LaunchAborted(instance_id) => {
                    self.launch_cancel_flags.remove(&instance_id);
                    self.is_launching = false;
                    self.running_instances.remove(&instance_id);
                    self.running_pids.lock().remove(&instance_id);
                    self.game_session_starts.remove(&instance_id);
                    self.stopped_by_user.remove(&instance_id);
                    self.game_running = !self.running_instances.is_empty();
                    self.running_instance = self.running_instances.keys().next().cloned();
                    self.pending_game_exited.push((instance_id, 0));
                    self.status_message =
                        Some(("Запуск игры отменён пользователем".to_string(), false));
                }
                UiWorkerResponse::MclogsUploaded(res) => {
                    self.is_uploading_mclogs = false;
                    match res {
                        Ok(url) => {
                            self.mclogs_url = Some(url);
                            self.status_message =
                                Some(("Лог загружен на mclogs".to_string(), false));
                        }
                        Err(e) => {
                            self.status_message =
                                Some((format!("Не удалось загрузить лог на mclogs: {e}"), true));
                        }
                    }
                }
                UiWorkerResponse::ServerPingReady { ip, result } => {
                    self.pinging_servers.remove(&ip);
                    match result {
                        Ok(ping) => {
                            self.server_pings.insert(ip, ping);
                        }
                        Err(e) => {
                            self.status_message =
                                Some((format!("Сервер {ip} не отвечает: {e}"), true));
                        }
                    }
                }
                UiWorkerResponse::JavaRuntimesDiscovered(runtimes) => {
                    self.discovered_java_runtimes = Some(runtimes);
                    self.is_discovering_java = false;
                }
                UiWorkerResponse::JavaDownloadFinished => {
                    self.is_downloading_java = false;
                }
                UiWorkerResponse::UpdateCheckFinished(result) => {
                    self.pending_update_check = Some(result);
                }
                UiWorkerResponse::UpdateDownloadProgress(frac) => {
                    self.pending_update_progress = Some(safe_progress_clamp(frac));
                }
                UiWorkerResponse::UpdateDownloadFailed(err) => {
                    self.pending_update_error = Some(err.clone());
                    self.status_message =
                        Some((format!("Ошибка обновления лаунчера: {err}"), true));
                }
                UiWorkerResponse::InstanceExported(result) => match result {
                    Ok(path) => {
                        self.status_message =
                            Some((format!("Экспортировано в {}", path.display()), false));
                    }
                    Err(e) => {
                        self.status_message = Some((format!("Ошибка экспорта: {e}"), true));
                    }
                },
                UiWorkerResponse::InstanceDeleted {
                    instance_id,
                    name,
                    error,
                } => {
                    self.invalidate_install_cache(Some(&instance_id));
                    if let Some(err) = error {
                        self.status_message =
                            Some((format!("Ошибка при удалении «{name}»: {err}"), true));
                    } else {
                        self.status_message =
                            Some((format!("Инстанс «{name}» успешно удален"), false));
                    }
                    let _ = self.cmd_sender.send(UiWorkerCommand::RefreshInstances);
                }
                UiWorkerResponse::CrashAnalysisReady {
                    instance_id,
                    analysis,
                } => {
                    self.crash_analysis = Some(analysis);
                    self.crash_analysis_instance = Some(instance_id);
                }
                UiWorkerResponse::ElyByAuthUrlReady(url) => {
                    self.ely_auth_url = Some(url);
                }
                UiWorkerResponse::ElyByLoginFinished(result) => {
                    match &result {
                        Ok(username) => {
                            let _ = self.cmd_sender.send(UiWorkerCommand::RefreshAccounts);
                            self.status_message =
                                Some((format!("Вход Ely.by выполнен: {username}"), false));
                        }
                        Err(e) => {
                            self.status_message = Some((format!("Ошибка входа Ely.by: {e}"), true));
                        }
                    }
                    self.ely_login_done = Some(result);
                }
                UiWorkerResponse::GameRssSamples(samples) => {
                    for (id, val) in samples {
                        let hist = self
                            .game_rss_history
                            .entry(id)
                            .or_insert_with(|| std::collections::VecDeque::with_capacity(60));
                        if hist.len() >= 60 {
                            hist.pop_front();
                        }
                        hist.push_back(val);
                    }
                }
                UiWorkerResponse::AutoFixDepsDone {
                    instance_id,
                    report,
                } => {
                    self.is_auto_fixing_deps = false;
                    match report {
                        Ok(rep) => {
                            self.status_message = Some((
                                format!(
                                    "Установлено зависимостей: {} (не найдено: {})",
                                    rep.installed.len(),
                                    rep.not_found.len()
                                ),
                                false,
                            ));
                            let _ = self
                                .cmd_sender
                                .send(UiWorkerCommand::ScanInstanceMods { instance_id });
                        }
                        Err(e) => {
                            self.status_message =
                                Some((format!("Ошибка установки зависимостей: {e}"), true));
                        }
                    }
                }
                UiWorkerResponse::ModpackUpdateChecked {
                    instance_id,
                    result,
                } => {
                    self.is_checking_modpack_update = false;
                    match result {
                        Ok(Some(info)) => {
                            self.modpack_update_available.insert(instance_id, info);
                            self.status_message =
                                Some(("Доступно обновление сборки".to_string(), false));
                        }
                        Ok(None) => {
                            self.modpack_update_available.remove(&instance_id);
                            self.status_message =
                                Some(("Установлена актуальная версия сборки".to_string(), false));
                        }
                        Err(e) => {
                            self.status_message =
                                Some((format!("Ошибка проверки обновления: {e}"), true));
                        }
                    }
                }
                UiWorkerResponse::ModpackUpdateApplied {
                    instance_id,
                    result,
                } => match result {
                    Ok(count) => {
                        self.modpack_update_available.remove(&instance_id);
                        self.status_message = Some((
                            format!("Сборка обновлена (обработано файлов: {count})"),
                            false,
                        ));
                        let _ = self
                            .cmd_sender
                            .send(UiWorkerCommand::ScanInstanceMods { instance_id });
                    }
                    Err(e) => {
                        self.status_message =
                            Some((format!("Ошибка применения обновления: {e}"), true));
                    }
                },
                UiWorkerResponse::ExternalInstancesDiscovered(instances) => {
                    self.external_instances = instances;
                }
                UiWorkerResponse::ExternalInstanceImported(result) => match *result {
                    Ok(manifest) => {
                        self.instances.push(manifest.clone());
                        self.selected_instance_id = Some(manifest.id);
                        self.status_message =
                            Some(("Инстанс успешно импортирован".to_string(), false));
                        let _ = self.cmd_sender.send(UiWorkerCommand::RefreshInstances);
                    }
                    Err(e) => {
                        self.status_message =
                            Some((format!("Ошибка импорта стороннего инстанса: {e}"), true));
                    }
                },
                UiWorkerResponse::WorldsBackedUp {
                    instance_id: _,
                    result,
                } => match result {
                    Ok(files) => {
                        self.status_message = Some((
                            format!("Резервных копий миров создано: {}", files.len()),
                            false,
                        ));
                    }
                    Err(e) => {
                        self.status_message = Some((format!("Ошибка автобэкапа миров: {e}"), true));
                    }
                },
                UiWorkerResponse::MsaDeviceCodeReady {
                    verification_uri,
                    user_code,
                } => {
                    self.msa_device_code = Some((verification_uri, user_code));
                }
                UiWorkerResponse::MsaLoginFinished(result) => {
                    match &result {
                        Ok(name) => {
                            let _ = self.cmd_sender.send(UiWorkerCommand::RefreshAccounts);
                            self.status_message =
                                Some((format!("Вход Microsoft выполнен: {name}"), false));
                        }
                        Err(e) => {
                            self.status_message =
                                Some((format!("Ошибка входа Microsoft: {e}"), true));
                        }
                    }
                    self.msa_login_done = Some(result);
                }
                UiWorkerResponse::DiscordRpcFinished(rpc_opt) => {
                    self.discord_rpc_busy = false;
                    self.pending_discord_rpc = rpc_opt;
                }
                UiWorkerResponse::WorldBackedUp {
                    instance_id: _,
                    result,
                } => match result {
                    Ok(p) => {
                        self.status_message =
                            Some((format!("Создан бэкап: {}", p.display()), false));
                    }
                    Err(e) => {
                        self.status_message = Some((format!("Ошибка создания бэкапа: {e}"), true));
                    }
                },
                UiWorkerResponse::WorldExported {
                    instance_id: _,
                    result,
                } => match result {
                    Ok(p) => {
                        self.status_message =
                            Some((format!("Мир экспортирован: {}", p.display()), false));
                    }
                    Err(e) => {
                        self.status_message = Some((format!("Ошибка экспорта мира: {e}"), true));
                    }
                },
                UiWorkerResponse::WorldRestored {
                    instance_id: _,
                    file_name,
                    result,
                } => match result {
                    Ok(p) => {
                        self.worlds_cache_gen = self.worlds_cache_gen.wrapping_add(1);
                        self.status_message = Some((
                            format!("Мир восстановлен из {file_name}: {}", p.display()),
                            false,
                        ));
                    }
                    Err(e) => {
                        self.status_message =
                            Some((format!("Ошибка восстановления мира: {e}"), true));
                    }
                },
                UiWorkerResponse::InstanceZipImported(res) => match *res {
                    Ok(manifest) => {
                        self.instances.push(manifest.clone());
                        self.selected_instance_id = Some(manifest.id);
                        self.status_message =
                            Some(("Инстанс успешно импортирован из архива".to_string(), false));
                        let _ = self.cmd_sender.send(UiWorkerCommand::RefreshInstances);
                    }
                    Err(e) => {
                        self.status_message = Some((format!("Ошибка импорта архива: {e}"), true));
                    }
                },
                UiWorkerResponse::WorldSizesReady { instance_id, sizes } => {
                    self.computing_world_sizes.remove(&instance_id);
                    for (folder, sz) in sizes {
                        self.world_sizes.insert((instance_id.clone(), folder), sz);
                    }
                }
            }
        }

        for event in self.event_bus.try_drain(256) {
            match event {
                Event::PhaseStarted { phase, .. } => {
                    self.active_phase = Some(phase.clone());
                    self.current_install_stage = InstallStage::from_phase_str(&phase);
                }
                Event::Progress(prog) => {
                    self.active_phase = Some(prog.phase.clone());
                    self.current_install_stage = InstallStage::from_phase_str(&prog.phase);
                    self.install_eta_secs =
                        calculate_eta_secs(prog.bytes_done, prog.bytes_total, prog.mib_per_sec);
                    let speed = if prog.mib_per_sec.is_finite() && prog.mib_per_sec >= 0.0 {
                        prog.mib_per_sec as f32
                    } else {
                        0.0
                    };
                    push_rolling_sample(&mut self.download_speed_history, speed);
                    self.jobs.update_active_throughput(&prog.phase, &prog);
                    self.phase_progress = Some(prog);
                }
                Event::PhaseCompleted { phase, elapsed_ms } => {
                    if self.installing_instance.is_none() && self.jobs.active_count() == 0 {
                        self.active_phase = None;
                        self.phase_progress = None;
                        self.install_eta_secs = None;
                    }
                    if let Some(url) = phase.strip_prefix("mclogs:") {
                        self.is_uploading_mclogs = false;
                        self.mclogs_url = Some(url.to_string());
                        self.status_message = Some(("Лог загружен на mclogs".to_string(), false));
                    } else if let Some(msg) = format_phase_completed_toast(&phase, elapsed_ms) {
                        self.status_message = Some((msg, false));
                    }
                }
                Event::TaskError { context, message } => {
                    if self.installing_instance.is_some() {
                        self.install_retries_count = self.install_retries_count.saturating_add(1);
                    }
                    self.status_message = Some((format!("Ошибка [{context}]: {message}"), true));
                }
                Event::CpuTelemetry(snap) => {
                    let pct = safe_clamp_f32(snap.total_pct, 0.0, 100.0);
                    let rss = if snap.rss_mib.is_finite() && snap.rss_mib >= 0.0 {
                        snap.rss_mib
                    } else {
                        0.0
                    };
                    push_rolling_sample(&mut self.cpu_history, pct);
                    push_rolling_sample(&mut self.rss_history, rss);
                    if rss > self.peak_rss_mib {
                        self.peak_rss_mib = rss;
                    }
                    self.latest_cpu = snap;
                }
                Event::GameLog(log) => {
                    if self.game_logs.len() >= MAX_GAME_LOG_LINES {
                        self.game_logs.pop_front();
                    }
                    self.game_logs.push_back(log);
                }
                Event::WatchdogWarning {
                    operation,
                    elapsed_ms,
                } => {
                    tracing::warn!(
                        operation,
                        elapsed_ms,
                        "UI Watchdog warning received in state"
                    );
                }
                _ => {}
            }
        }
    }

    pub fn selected_instance(&self) -> Option<&InstanceManifest> {
        let sel_id = self.selected_instance_id.as_ref()?;
        self.instances.iter().find(|i| i.id == *sel_id)
    }

    /// Invalidates cached install readiness for one instance (or all instances when `None`).
    pub fn invalidate_install_cache(&self, id: Option<&InstanceId>) {
        let mut cache = self.install_status_cache.borrow_mut();
        if let Some(target) = id {
            cache.remove(target);
        } else {
            cache.clear();
        }
    }

    /// Checks whether an instance has already been installed into the local CAS and instance directory.
    /// Cached briefly (2s) so hot UI loops do not hammer the filesystem every frame.
    pub fn is_instance_installed(&self, id: &InstanceId) -> bool {
        const INSTALL_CACHE_TTL: Duration = Duration::from_secs(2);
        if let Some(&(installed, checked_at)) = self.install_status_cache.borrow().get(id) {
            if checked_at.elapsed() < INSTALL_CACHE_TTL {
                return installed;
            }
        }
        let installed = self.probe_instance_installed_on_disk(id);
        self.install_status_cache
            .borrow_mut()
            .insert(id.clone(), (installed, Instant::now()));
        installed
    }

    fn probe_instance_installed_on_disk(&self, id: &InstanceId) -> bool {
        let inst_dir = self.paths.instance_dir(id.as_str());
        let state_file = inst_dir.join("install_state.json");
        if state_file.is_file() {
            if let Ok(content) = std::fs::read_to_string(&state_file) {
                if let Ok(st) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(status) = st.get("status").and_then(|v| v.as_str()) {
                        if status == "ready" {
                            return true;
                        } else if status == "failed" || status == "installing" {
                            return false;
                        }
                    }
                }
            }
        }
        inst_dir.join("resolved_version.json").exists() && inst_dir.join("client.jar").exists()
    }
}

/// Converts a completed phase identifier into an optional user-facing Russian toast message.
///
/// Internal background phases (`catalog_search`, `manifest_fetch`, `mods_scan`, download sub-stages,
/// and unrecognized internal ASCII identifiers) return `None` so they never spam raw internal names.
pub fn format_phase_completed_toast(phase: &str, elapsed_ms: u64) -> Option<String> {
    let trimmed = phase.trim();
    if trimmed.is_empty() {
        return None;
    }
    let _ = elapsed_ms;
    match trimmed {
        // Silent internal/background phases and sub-phases already covered by dedicated completion events
        "catalog_search" | "manifest_fetch" | "mods_scan" | "manifest" | "assets" | "libraries"
        | "natives" | "client" | "java" | "install_game" | "install_mods" | "install_cfpack"
        | "install_mrpack" => None,
        // User-initiated verification / repair / reinstall operations
        "verify" => Some("Проверка целостности файлов завершена".to_string()),
        "verify_and_repair" => Some("Проверка и восстановление файлов завершены".to_string()),
        "reinstall" => Some("Переустановка инстанса завершена".to_string()),
        // Already-localized Russian status strings (e.g. Adoptium Java installation)
        other
            if other
                .chars()
                .any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)) =>
        {
            Some(other.to_string())
        }
        // Suppress any other raw internal ASCII phase name
        _ => None,
    }
}

/// Appends `val` to a rolling telemetry buffer capped at [`MAX_TELEMETRY_SAMPLES`].
#[inline]
pub fn push_rolling_sample<T>(deque: &mut VecDeque<T>, val: T) {
    while deque.len() >= MAX_TELEMETRY_SAMPLES {
        deque.pop_front();
    }
    deque.push_back(val);
}

/// Clamps `val` between `min` and `max` without panicking under any circumstances:
/// - Inverted bounds (`min > max`) are normalized safely without asserting.
/// - `NaN` values for `val`, `min`, or `max` are safely replaced without panicking.
/// - `+Inf` and `-Inf` are clamped to finite bounds when bounds are finite.
#[inline]
pub fn safe_clamp_f32(val: f32, min: f32, max: f32) -> f32 {
    let min = if min.is_nan() { 0.0 } else { min };
    let max = if max.is_nan() { min } else { max };
    let (effective_min, effective_max) = if min > max { (max, min) } else { (min, max) };
    if val.is_nan() {
        return effective_min;
    }
    if val < effective_min {
        effective_min
    } else if val > effective_max {
        effective_max
    } else {
        val
    }
}

/// Clamps any floating point progress value to the safe range `[0.0, 1.0]`.
/// Returns `0.0` for `NaN` and negative values, and `1.0` for values >= 1.0 or `+Inf`.
#[inline]
pub fn safe_progress_clamp(val: f32) -> f32 {
    safe_clamp_f32(val, 0.0, 1.0)
}

/// Safely computes the completion ratio `done / total` as an `f32` in `[0.0, 1.0]`.
///
/// - If `total == 0`, returns `0.0`.
/// - If `done >= total`, returns `1.0`.
/// - Uses `u64` bounds, checked divisions, and avoids any floating-point overflow or NaN.
#[inline]
pub fn safe_progress_ratio(done: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    if done >= total {
        return 1.0;
    }
    let frac = (done as f64) / (total as f64);
    if !frac.is_finite() || frac <= 0.0 {
        0.0
    } else if frac >= 1.0 {
        1.0
    } else {
        frac as f32
    }
}

/// Safely calculates remaining ETA in seconds given bytes completed, total bytes, and transfer speed in MiB/s.
/// Uses `saturating_sub` and validates against NaN/Inf/zero speeds.
#[inline]
pub fn calculate_eta_secs(bytes_done: u64, bytes_total: u64, mib_per_sec: f64) -> Option<u64> {
    if !mib_per_sec.is_finite() || mib_per_sec <= 0.001 {
        return None;
    }
    let remaining = bytes_total.saturating_sub(bytes_done);
    if remaining == 0 {
        return None;
    }
    let speed_bytes = mib_per_sec * 1024.0 * 1024.0;
    if !speed_bytes.is_finite() || speed_bytes <= 0.0 {
        return None;
    }
    let secs = remaining as f64 / speed_bytes;
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    if secs > (u64::MAX as f64) {
        Some(u64::MAX)
    } else {
        Some(secs as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_clamp_f32_edges() {
        // Normal in-bounds
        assert_eq!(safe_clamp_f32(0.5, 0.0, 1.0), 0.5);
        // Exceeds max
        assert_eq!(safe_clamp_f32(1.5, 0.0, 1.0), 1.0);
        // Below min
        assert_eq!(safe_clamp_f32(-0.5, 0.0, 1.0), 0.0);
        // Inverted min > max (e.g. hud.rs:99 case max_bar_h < 1.5)
        assert_eq!(safe_clamp_f32(0.8, 1.5, 1.0), 1.0);
        assert_eq!(safe_clamp_f32(2.0, 1.5, 1.0), 1.5);
        assert_eq!(safe_clamp_f32(1.2, 1.5, 1.0), 1.2);
        // NaNs
        assert_eq!(safe_clamp_f32(f32::NAN, 0.0, 1.0), 0.0);
        assert_eq!(safe_clamp_f32(0.5, f32::NAN, 1.0), 0.5);
        assert_eq!(safe_clamp_f32(0.5, 0.0, f32::NAN), 0.0);
        assert_eq!(safe_clamp_f32(f32::NAN, f32::NAN, f32::NAN), 0.0);
        // Infinities
        assert_eq!(safe_clamp_f32(f32::INFINITY, 0.0, 1.0), 1.0);
        assert_eq!(safe_clamp_f32(f32::NEG_INFINITY, 0.0, 1.0), 0.0);
    }

    #[test]
    fn test_safe_progress_ratio_edges() {
        assert_eq!(safe_progress_ratio(0, 0), 0.0);
        assert_eq!(safe_progress_ratio(50, 0), 0.0);
        assert_eq!(safe_progress_ratio(0, 100), 0.0);
        assert_eq!(safe_progress_ratio(50, 100), 0.5);
        assert_eq!(safe_progress_ratio(100, 100), 1.0);
        assert_eq!(safe_progress_ratio(150, 100), 1.0);
        assert_eq!(safe_progress_ratio(u64::MAX, u64::MAX), 1.0);
        assert_eq!(safe_progress_ratio(u64::MAX, 10), 1.0);
        assert_eq!(safe_progress_ratio(0, u64::MAX), 0.0);
    }

    #[test]
    fn test_calculate_eta_secs_edges() {
        assert_eq!(calculate_eta_secs(0, 0, 10.0), None);
        assert_eq!(calculate_eta_secs(100, 50, 10.0), None);
        assert_eq!(calculate_eta_secs(50, 100, 0.0), None);
        assert_eq!(calculate_eta_secs(50, 100, -5.0), None);
        assert_eq!(calculate_eta_secs(50, 100, f64::NAN), None);
        assert_eq!(calculate_eta_secs(50, 100, f64::INFINITY), None);

        // 1 MiB remaining at 1 MiB/s = 1 second
        let one_mib = 1024 * 1024;
        assert_eq!(calculate_eta_secs(0, one_mib, 1.0), Some(1));
        // Saturating subtraction when done > total
        assert_eq!(calculate_eta_secs(u64::MAX, 100, 10.0), None);
    }

    #[test]
    fn test_progress_ten_thousand_extreme_states_never_panic() {
        let extreme_u64 = [
            0,
            1,
            2,
            100,
            1000,
            u32::MAX as u64,
            (u32::MAX as u64) + 1,
            u64::MAX / 2,
            u64::MAX - 1,
            u64::MAX,
        ];

        let extreme_f32 = [
            0.0,
            -0.0,
            1.0,
            -1.0,
            0.5,
            -0.5,
            1.5,
            100.0,
            -100.0,
            f32::MIN_POSITIVE,
            f32::EPSILON,
            f32::MAX,
            f32::MIN,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ];

        let extreme_f64 = [
            0.0,
            -0.0,
            0.0001,
            0.001,
            1.0,
            -1.0,
            50.0,
            f64::MIN_POSITIVE,
            f64::MAX,
            f64::MIN,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ];

        let string_samples = [
            "",
            "   ",
            "manifest",
            "assets",
            "natives",
            "libraries",
            "java",
            "client",
            "verify",
            "unknown_phase_xyz",
            "404 Not Found",
            "Connection reset",
            "No space left on device",
            "Permission denied",
            "SHA1 checksum mismatch",
            "паника в потоке",
            "Установка прервана из-за внутренней ошибки",
            "\0\r\n\t",
            "https://piston-meta.mojang.com/v1/packages/test.json",
        ];

        let mut seed: u64 = 0xDEADBEEF_CAFE1234;
        let mut next_rand = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            seed
        };

        for i in 0..10_000 {
            let done = if i < extreme_u64.len() {
                extreme_u64[i]
            } else {
                let r = next_rand();
                match r % 4 {
                    0 => extreme_u64[(r as usize) % extreme_u64.len()],
                    1 => r,
                    2 => r % 10_000,
                    _ => 0,
                }
            };

            let total = if i < extreme_u64.len() {
                extreme_u64[extreme_u64.len() - 1 - i]
            } else {
                let r = next_rand();
                match r % 4 {
                    0 => extreme_u64[(r as usize) % extreme_u64.len()],
                    1 => r,
                    2 => r % 10_000,
                    _ => 0,
                }
            };

            let ratio = safe_progress_ratio(done, total);
            assert!(
                (0.0..=1.0).contains(&ratio),
                "ratio out of bounds: {ratio} for done={done}, total={total}"
            );
            assert!(
                ratio.is_finite(),
                "ratio is not finite for done={done}, total={total}"
            );

            let val = extreme_f32[(next_rand() as usize) % extreme_f32.len()];
            let min = extreme_f32[(next_rand() as usize) % extreme_f32.len()];
            let max = extreme_f32[(next_rand() as usize) % extreme_f32.len()];
            let clamped = safe_clamp_f32(val, min, max);
            assert!(
                !clamped.is_nan(),
                "clamped returned NaN for val={val}, min={min}, max={max}"
            );

            let unit_clamped = safe_progress_clamp(val);
            assert!(
                (0.0..=1.0).contains(&unit_clamped),
                "unit clamped out of range: {unit_clamped} for val={val}"
            );

            let speed = extreme_f64[(next_rand() as usize) % extreme_f64.len()];
            let eta = calculate_eta_secs(done, total, speed);
            if let Some(secs) = eta {
                let _ = secs;
            }

            let phase_str = string_samples[(next_rand() as usize) % string_samples.len()];
            let stage = InstallStage::from_phase_str(phase_str);
            let _ = stage.localized_name();
            let _ = stage.pipeline_index();

            let err_str = string_samples[(next_rand() as usize) % string_samples.len()];
            let err = InstallError::from_error_str(stage, err_str);
            assert!(!err.localized_title.is_empty());
            assert!(!err.localized_details.is_empty());
            let clip = err.format_for_clipboard();
            assert!(!clip.is_empty());
        }
    }

    #[test]
    fn phase_completed_toast_suppresses_catalog_search_and_localizes_user_phases() {
        for silent in [
            "catalog_search",
            "manifest_fetch",
            "mods_scan",
            "manifest",
            "assets",
            "libraries",
            "natives",
            "client",
            "java",
            "install_game",
            "install_mods",
            "unknown_internal_phase",
            "",
        ] {
            assert_eq!(
                format_phase_completed_toast(silent, 10009),
                None,
                "internal phase '{silent}' must never produce a user toast"
            );
        }

        let verify_msg =
            format_phase_completed_toast("verify", 420).expect("verify should produce toast");
        assert!(verify_msg.contains("Проверка"));
        assert!(!verify_msg.contains("Фаза '"));

        let repair_msg = format_phase_completed_toast("verify_and_repair", 1200)
            .expect("verify_and_repair should produce toast");
        assert!(repair_msg.contains("восстановление"));

        let reinstall_msg = format_phase_completed_toast("reinstall", 3100)
            .expect("reinstall should produce toast");
        assert!(reinstall_msg.contains("Переустановка"));

        let java_msg = format_phase_completed_toast("Java 21 успешно установлена: 21.0.6", 0)
            .expect("already localized Russian phase should pass through");
        assert_eq!(java_msg, "Java 21 успешно установлена: 21.0.6");
    }

    #[test]
    fn drain_events_ignores_catalog_search_phase_and_handles_mclogs_response() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = MqPaths::with_root(tmp.path().to_path_buf());
        let cfg = AppConfig::default();
        let pool = Arc::new(ComputePool::new(1));
        let mut state = UiState::new(paths, cfg, pool).expect("state");

        state.status_message = None;
        state.event_bus.publish(Event::PhaseCompleted {
            phase: "catalog_search".to_string(),
            elapsed_ms: 10009,
        });
        state.drain_events();
        assert_eq!(
            state.status_message, None,
            "catalog_search PhaseCompleted must never set status_message toast"
        );

        state.is_uploading_mclogs = true;
        state
            .resp_sender
            .send(UiWorkerResponse::MclogsUploaded(Ok(
                "https://mclo.gs/test123".to_string(),
            )))
            .expect("send ok");
        state.drain_events();
        assert!(!state.is_uploading_mclogs);
        assert_eq!(state.mclogs_url.as_deref(), Some("https://mclo.gs/test123"));
        assert!(state
            .status_message
            .as_ref()
            .is_some_and(|(msg, is_err)| !is_err && msg.contains("mclogs")));

        state.is_uploading_mclogs = true;
        state
            .resp_sender
            .send(UiWorkerResponse::MclogsUploaded(Err("timeout".to_string())))
            .expect("send err");
        state.drain_events();
        assert!(!state.is_uploading_mclogs);
        assert!(state
            .status_message
            .as_ref()
            .is_some_and(|(msg, is_err)| *is_err && msg.contains("timeout")));
    }
}
