//! Cross-platform per-core CPU telemetry, process RSS/peak-RSS readers,
//! open file-descriptor / Win32 handle counting, and scoped phase CPU profiling.
//!
//! - On Linux: uses direct `/proc/stat`, `/proc/self/status`, and `/proc/self/fd` fast paths.
//! - On Windows: uses `sysinfo` for per-core CPU utilization paired with Win32 `kernel32`
//!   (`GetSystemTimes`, `K32GetProcessMemoryInfo`, `GetProcessHandleCount`) for zero-overhead
//!   process memory, peak working set, and handle telemetry.
//! - On other non-Linux platforms (e.g. macOS): uses `sysinfo` with atomic peak-RSS tracking.

#![deny(clippy::await_holding_lock)]

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::fs;
use std::io;
#[cfg(any(not(target_os = "linux"), test))]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(25);
#[cfg(target_os = "linux")]
const KIB_PER_MIB: f64 = 1024.0;
#[cfg(not(target_os = "linux"))]
const BYTES_PER_MIB: f64 = 1024.0 * 1024.0;

#[cfg(not(target_os = "linux"))]
static PEAK_RSS_BYTES: AtomicU64 = AtomicU64::new(0);

#[inline]
fn clamp_cpu_pct(val: f32) -> f32 {
    if val.is_finite() {
        val.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

#[cfg(any(not(target_os = "linux"), test))]
#[inline]
fn record_peak_bytes(peak: &AtomicU64, sample_bytes: u64) -> u64 {
    peak.fetch_max(sample_bytes, Ordering::Relaxed)
        .max(sample_bytes)
}

#[cfg(target_os = "windows")]
mod win32_telemetry {
    use std::ffi::c_void;
    use std::mem::{size_of, MaybeUninit};

    #[repr(C)]
    struct FileTime {
        dw_low_date_time: u32,
        dw_high_date_time: u32,
    }

    #[repr(C)]
    #[allow(dead_code)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn GetProcessHandleCount(h_process: *mut c_void, pdw_handle_count: *mut u32) -> i32;
        fn K32GetProcessMemoryInfo(
            process: *mut c_void,
            ppsmem_counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
        fn GetSystemTimes(
            lp_idle_time: *mut FileTime,
            lp_kernel_time: *mut FileTime,
            lp_user_time: *mut FileTime,
        ) -> i32;
    }

    #[inline]
    fn filetime_to_u64(ft: FileTime) -> u64 {
        (u64::from(ft.dw_high_date_time) << 32) | u64::from(ft.dw_low_date_time)
    }

    pub(super) fn read_handle_count() -> usize {
        let mut count: u32 = 0;
        // SAFETY: `GetCurrentProcess` returns a pseudo-handle valid for the current process,
        // and `&mut count` points to valid, aligned writable `u32` storage.
        let ok = unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) };
        if ok != 0 {
            count as usize
        } else {
            0
        }
    }

    /// Returns `(working_set_bytes, peak_working_set_bytes)` for the current process.
    pub(super) fn read_memory_counters_bytes() -> Option<(u64, u64)> {
        let mut counters = MaybeUninit::<ProcessMemoryCounters>::zeroed();
        let cb = size_of::<ProcessMemoryCounters>() as u32;
        // SAFETY: `counters` is valid aligned `ProcessMemoryCounters` storage initialized
        // with zeros and `cb` set to the exact struct byte size required by Win32.
        let ok = unsafe {
            (*counters.as_mut_ptr()).cb = cb;
            K32GetProcessMemoryInfo(GetCurrentProcess(), counters.as_mut_ptr(), cb)
        };
        if ok != 0 {
            // SAFETY: `K32GetProcessMemoryInfo` succeeded and populated `counters`.
            let c = unsafe { counters.assume_init() };
            Some((c.working_set_size as u64, c.peak_working_set_size as u64))
        } else {
            None
        }
    }

    /// Returns `(user_ticks, system_ticks, idle_ticks)` from Win32 `GetSystemTimes`.
    pub(super) fn read_system_times() -> Option<(u64, u64, u64)> {
        let mut idle = MaybeUninit::<FileTime>::zeroed();
        let mut kernel = MaybeUninit::<FileTime>::zeroed();
        let mut user = MaybeUninit::<FileTime>::zeroed();
        // SAFETY: All three pointers point to valid, aligned `FileTime` storage.
        let ok =
            unsafe { GetSystemTimes(idle.as_mut_ptr(), kernel.as_mut_ptr(), user.as_mut_ptr()) };
        if ok != 0 {
            // SAFETY: `GetSystemTimes` succeeded and initialized all three `FileTime` structs.
            let (idle_ft, kernel_ft, user_ft) =
                unsafe { (idle.assume_init(), kernel.assume_init(), user.assume_init()) };
            let idle_v = filetime_to_u64(idle_ft);
            let kernel_v = filetime_to_u64(kernel_ft);
            let user_v = filetime_to_u64(user_ft);
            // Per Win32 documentation, `lpKernelTime` includes `lpIdleTime`.
            let system_v = kernel_v.saturating_sub(idle_v);
            Some((user_v, system_v, idle_v))
        } else {
            None
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn new_cpu_system() -> sysinfo::System {
    sysinfo::System::new_with_specifics(
        sysinfo::RefreshKind::nothing()
            .with_cpu(sysinfo::CpuRefreshKind::nothing().with_cpu_usage()),
    )
}

#[cfg(not(target_os = "linux"))]
fn read_rss_pair_mib_sysinfo() -> (f64, f64) {
    let Ok(pid) = sysinfo::get_current_pid() else {
        let peak = PEAK_RSS_BYTES.load(Ordering::Relaxed);
        return (0.0, peak as f64 / BYTES_PER_MIB);
    };
    let mut sys = sysinfo::System::new();
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        false,
        sysinfo::ProcessRefreshKind::nothing().with_memory(),
    );
    let rss_bytes = sys.process(pid).map_or(0, sysinfo::Process::memory);
    let peak_bytes = record_peak_bytes(&PEAK_RSS_BYTES, rss_bytes);
    (
        rss_bytes as f64 / BYTES_PER_MIB,
        peak_bytes as f64 / BYTES_PER_MIB,
    )
}

/// Raw jiffies sample for either the aggregate `cpu` line (`core_id == None`)
/// or a specific logical core `cpuN` (`core_id == Some(N)`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoreSample {
    pub core_id: Option<usize>,
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CoreSample {
    /// Active CPU jiffies: `user + nice + system + irq + softirq + steal`.
    /// Note: Linux already accounts `guest` inside `user` and `guest_nice` inside `nice`.
    #[inline]
    pub fn busy(&self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }

    /// Total CPU jiffies: `busy + idle + iowait`.
    #[inline]
    pub fn total(&self) -> u64 {
        self.busy()
            .saturating_add(self.idle)
            .saturating_add(self.iowait)
    }

    /// Computes exact kernel-accounted utilization percentage (`0.0..=100.0`)
    /// over the interval from `prev` to `self`.
    #[inline]
    pub fn utilization_since(&self, prev: &Self) -> f32 {
        let delta_busy = self.busy().saturating_sub(prev.busy());
        let delta_total = self.total().saturating_sub(prev.total());
        if delta_total == 0 {
            return 0.0;
        }
        clamp_cpu_pct(((delta_busy as f64 / delta_total as f64) * 100.0) as f32)
    }
}

/// Parsed snapshot of `/proc/stat` containing both aggregate `cpu` and per-core `cpu0..cpuN` counters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcStatRaw {
    pub total: CoreSample,
    pub cores: Vec<CoreSample>,
}

impl ProcStatRaw {
    /// Computes `(total_pct, per_core_pct)` delta utilization from `prev` to `self`,
    /// with every value clamped to `0.0..=100.0`.
    pub fn utilization_delta(&self, prev: &Self) -> (f32, Vec<f32>) {
        let total_pct = self.total.utilization_since(&prev.total);
        let per_core_pct = self
            .cores
            .iter()
            .enumerate()
            .map(|(idx, curr_core)| {
                let prev_core = prev
                    .cores
                    .get(idx)
                    .filter(|c| c.core_id == curr_core.core_id)
                    .or_else(|| prev.cores.iter().find(|c| c.core_id == curr_core.core_id));
                match prev_core {
                    Some(p) => curr_core.utilization_since(p),
                    None => 0.0,
                }
            })
            .collect();
        (total_pct, per_core_pct)
    }
}

/// Parses Linux `/proc/stat` contents into aggregate `cpu` and sorted `cpu0..cpuN` jiffies counters.
pub fn parse_proc_stat(contents: &str) -> ProcStatRaw {
    let mut total = CoreSample::default();
    let mut cores = Vec::new();

    for line in contents.lines() {
        let mut parts = line.split_whitespace();
        let Some(label) = parts.next() else {
            continue;
        };

        if label == "cpu" {
            total = parse_cpu_fields(None, &mut parts);
        } else if let Some(suffix) = label.strip_prefix("cpu") {
            if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) {
                if let Ok(core_id) = suffix.parse::<usize>() {
                    cores.push(parse_cpu_fields(Some(core_id), &mut parts));
                }
            }
        }
    }

    cores.sort_by_key(|c| c.core_id);
    ProcStatRaw { total, cores }
}

fn parse_cpu_fields<'a>(
    core_id: Option<usize>,
    parts: &mut impl Iterator<Item = &'a str>,
) -> CoreSample {
    let mut next_u64 = || {
        parts
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
    };
    CoreSample {
        core_id,
        user: next_u64(),
        nice: next_u64(),
        system: next_u64(),
        idle: next_u64(),
        iowait: next_u64(),
        irq: next_u64(),
        softirq: next_u64(),
        steal: next_u64(),
    }
}

/// Reads and parses `/proc/stat` on Linux, or queries Win32 `GetSystemTimes` on Windows.
pub fn read_proc_stat() -> io::Result<ProcStatRaw> {
    #[cfg(target_os = "linux")]
    {
        let contents = fs::read_to_string("/proc/stat")?;
        Ok(parse_proc_stat(&contents))
    }

    #[cfg(target_os = "windows")]
    {
        if let Some((user, system, idle)) = win32_telemetry::read_system_times() {
            Ok(ProcStatRaw {
                total: CoreSample {
                    core_id: None,
                    user,
                    system,
                    idle,
                    ..CoreSample::default()
                },
                cores: Vec::new(),
            })
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "/proc/stat is unavailable on this platform; use CpuSampler",
        ))
    }
}

/// Parses a `kB` memory counter (such as `VmRSS` or `VmHWM`) from `/proc/self/status` text.
pub fn parse_status_kb(contents: &str, key: &str) -> Option<u64> {
    let target = key.trim_end_matches(':');
    for line in contents.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim() == target {
                return v
                    .split_whitespace()
                    .next()
                    .and_then(|tok| tok.parse::<u64>().ok());
            }
        }
    }
    None
}

fn read_rss_pair_mib() -> (f64, f64) {
    #[cfg(target_os = "linux")]
    {
        let Ok(contents) = fs::read_to_string("/proc/self/status") else {
            return (0.0, 0.0);
        };
        let rss_mib = parse_status_kb(&contents, "VmRSS")
            .map(|kb| kb as f64 / KIB_PER_MIB)
            .unwrap_or(0.0);
        let peak_mib = parse_status_kb(&contents, "VmHWM")
            .map(|kb| kb as f64 / KIB_PER_MIB)
            .unwrap_or(0.0);
        (rss_mib, peak_mib)
    }

    #[cfg(target_os = "windows")]
    {
        if let Some((working_set, peak_working_set)) = win32_telemetry::read_memory_counters_bytes()
        {
            let peak_bytes = record_peak_bytes(&PEAK_RSS_BYTES, working_set.max(peak_working_set));
            return (
                working_set as f64 / BYTES_PER_MIB,
                peak_bytes as f64 / BYTES_PER_MIB,
            );
        }
        read_rss_pair_mib_sysinfo()
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        read_rss_pair_mib_sysinfo()
    }
}

/// Reads current resident set size (`VmRSS` on Linux, Working Set on Windows) in MiB.
pub fn read_current_rss_mib() -> f64 {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| parse_status_kb(&s, "VmRSS"))
            .map(|kb| kb as f64 / KIB_PER_MIB)
            .unwrap_or(0.0)
    }

    #[cfg(not(target_os = "linux"))]
    {
        read_rss_pair_mib().0
    }
}

/// Reads peak high-water mark resident set size (`VmHWM` on Linux, Peak Working Set on Windows) in MiB.
pub fn read_peak_rss_mib() -> f64 {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| parse_status_kb(&s, "VmHWM"))
            .map(|kb| kb as f64 / KIB_PER_MIB)
            .unwrap_or(0.0)
    }

    #[cfg(not(target_os = "linux"))]
    {
        read_rss_pair_mib().1
    }
}

/// Counts currently open file descriptors (Linux `/proc/self/fd`, macOS `/dev/fd`) or
/// kernel handles (Windows `GetProcessHandleCount`) for this process.
pub fn count_open_fds() -> usize {
    #[cfg(target_os = "linux")]
    {
        fs::read_dir("/proc/self/fd")
            .map(|entries| entries.count())
            .unwrap_or(0)
    }

    #[cfg(target_os = "windows")]
    {
        win32_telemetry::read_handle_count()
    }

    #[cfg(target_os = "macos")]
    {
        fs::read_dir("/dev/fd")
            .map(|entries| entries.count())
            .unwrap_or(0)
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        0
    }
}

/// Point-in-time delta CPU utilization and process memory telemetry snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct CpuTelemetrySnapshot {
    pub total_pct: f32,
    pub per_core_pct: Vec<f32>,
    pub rss_mib: f64,
    pub peak_rss_mib: f64,
}

/// Stateful cross-platform CPU utilization sampler reporting total and per-core utilization
/// along with current and peak process RSS.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
pub struct CpuSampler {
    prev: ProcStatRaw,
}

/// Stateful cross-platform CPU utilization sampler reporting total and per-core utilization
/// along with current and peak process RSS.
#[cfg(not(target_os = "linux"))]
pub struct CpuSampler {
    prev: ProcStatRaw,
    system: Option<sysinfo::System>,
    peak_rss_mib: f64,
}

#[cfg(not(target_os = "linux"))]
impl std::fmt::Debug for CpuSampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpuSampler")
            .field("prev", &self.prev)
            .field("has_system", &self.system.is_some())
            .field("peak_rss_mib", &self.peak_rss_mib)
            .finish()
    }
}

#[cfg(not(target_os = "linux"))]
impl Clone for CpuSampler {
    fn clone(&self) -> Self {
        Self {
            prev: self.prev.clone(),
            system: self.system.as_ref().map(|_| new_cpu_system()),
            peak_rss_mib: self.peak_rss_mib,
        }
    }
}

impl CpuSampler {
    /// Initializes a `CpuSampler` by capturing an initial OS CPU baseline.
    pub fn new() -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            Ok(Self {
                prev: read_proc_stat()?,
            })
        }

        #[cfg(not(target_os = "linux"))]
        {
            let prev = read_proc_stat().unwrap_or_default();
            let system = Some(new_cpu_system());
            let (rss_mib, peak_rss_mib) = read_rss_pair_mib();
            Ok(Self {
                prev,
                system,
                peak_rss_mib: peak_rss_mib.max(rss_mib),
            })
        }
    }

    /// Creates a `CpuSampler` from a synthetic or pre-parsed baseline.
    pub fn from_raw(baseline: ProcStatRaw) -> Self {
        #[cfg(target_os = "linux")]
        {
            Self { prev: baseline }
        }

        #[cfg(not(target_os = "linux"))]
        {
            Self {
                prev: baseline,
                system: None,
                peak_rss_mib: 0.0,
            }
        }
    }

    /// Samples CPU utilization and process memory since the previous call and advances
    /// the internal baseline.
    pub fn sample(&mut self) -> io::Result<CpuTelemetrySnapshot> {
        #[cfg(target_os = "linux")]
        {
            let current = read_proc_stat()?;
            let (total_pct, per_core_pct) = current.utilization_delta(&self.prev);
            self.prev = current;
            let (rss_mib, peak_rss_mib) = read_rss_pair_mib();
            Ok(CpuTelemetrySnapshot {
                total_pct,
                per_core_pct,
                rss_mib,
                peak_rss_mib: peak_rss_mib.max(rss_mib),
            })
        }

        #[cfg(not(target_os = "linux"))]
        {
            let sys = self.system.get_or_insert_with(new_cpu_system);
            sys.refresh_cpu_usage();

            let per_core_pct: Vec<f32> = sys
                .cpus()
                .iter()
                .map(|cpu| clamp_cpu_pct(cpu.cpu_usage()))
                .collect();

            let mut total_pct = clamp_cpu_pct(sys.global_cpu_usage());
            if total_pct <= 0.0 && !per_core_pct.is_empty() {
                let sum: f32 = per_core_pct.iter().copied().sum();
                if sum > 0.0 {
                    total_pct = clamp_cpu_pct(sum / per_core_pct.len() as f32);
                }
            }

            if let Ok(current_raw) = read_proc_stat() {
                let (raw_total, _) = current_raw.utilization_delta(&self.prev);
                self.prev = current_raw;
                if total_pct <= 0.0 && raw_total > 0.0 {
                    total_pct = clamp_cpu_pct(raw_total);
                }
            }

            let (rss_mib, peak_rss_mib) = read_rss_pair_mib();
            self.peak_rss_mib = self.peak_rss_mib.max(rss_mib).max(peak_rss_mib);

            Ok(CpuTelemetrySnapshot {
                total_pct,
                per_core_pct,
                rss_mib,
                peak_rss_mib: self.peak_rss_mib,
            })
        }
    }

    /// Advances the sampler using an explicit `ProcStatRaw` snapshot (useful for deterministic tests).
    pub fn sample_raw(&mut self, current: ProcStatRaw) -> (f32, Vec<f32>) {
        let delta = current.utilization_delta(&self.prev);
        self.prev = current;
        delta
    }
}

/// Aggregated CPU and memory report produced at the end of a `ScopedCpuProfiler` phase.
#[derive(Debug, Clone, PartialEq)]
pub struct PhaseCpuReport {
    pub elapsed_ms: f64,
    pub mean_total_pct: f32,
    pub per_core_mean_pct: Vec<f32>,
    pub per_core_peak_pct: Vec<f32>,
    pub peak_rss_mib: f64,
}

#[cfg(target_os = "linux")]
struct BgSampleStats {
    per_core_peak_pct: Vec<f32>,
    peak_rss_mib: f64,
}

#[cfg(not(target_os = "linux"))]
struct BgSampleStats {
    total_sum_pct: f64,
    per_core_sum_pct: Vec<f64>,
    per_core_peak_pct: Vec<f32>,
    sample_count: u64,
    peak_rss_mib: f64,
}

/// Lightweight RAII/scoped CPU profiler that polls OS CPU counters every 25ms on a background
/// thread to capture peak per-core utilization and peak RSS, while computing mean utilization
/// across the phase window.
pub struct ScopedCpuProfiler {
    start_instant: Instant,
    start_stat: ProcStatRaw,
    stop_tx: Option<Sender<()>>,
    worker: Option<JoinHandle<BgSampleStats>>,
}

impl ScopedCpuProfiler {
    /// Starts profiling with the default 25ms background polling interval.
    pub fn start() -> io::Result<Self> {
        Self::start_with_interval(DEFAULT_POLL_INTERVAL)
    }

    /// Starts profiling with a custom background polling interval.
    pub fn start_with_interval(poll_interval: Duration) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let start_stat = read_proc_stat()?;
            let start_instant = Instant::now();
            let initial_cores = start_stat.cores.len();
            let (init_rss, init_hwm) = read_rss_pair_mib();
            let initial_peak_rss = init_hwm.max(init_rss);

            let (stop_tx, stop_rx) = mpsc::channel::<()>();
            let prev_for_thread = start_stat.clone();

            let worker = thread::Builder::new()
                .name("mq-cpu-profiler".into())
                .spawn(move || {
                    let mut prev = prev_for_thread;
                    let mut per_core_peak_pct = vec![0.0_f32; initial_cores];
                    let mut peak_rss_mib = initial_peak_rss;

                    loop {
                        match stop_rx.recv_timeout(poll_interval) {
                            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                            Err(RecvTimeoutError::Timeout) => {
                                if let Ok(curr) = read_proc_stat() {
                                    let (_, core_pcts) = curr.utilization_delta(&prev);
                                    if per_core_peak_pct.len() < core_pcts.len() {
                                        per_core_peak_pct.resize(core_pcts.len(), 0.0);
                                    }
                                    for (peak, &pct) in
                                        per_core_peak_pct.iter_mut().zip(core_pcts.iter())
                                    {
                                        *peak = peak.max(pct);
                                    }
                                    prev = curr;
                                }
                                let (rss, hwm) = read_rss_pair_mib();
                                peak_rss_mib = peak_rss_mib.max(rss).max(hwm);
                            }
                        }
                    }

                    BgSampleStats {
                        per_core_peak_pct,
                        peak_rss_mib,
                    }
                })?;

            Ok(Self {
                start_instant,
                start_stat,
                stop_tx: Some(stop_tx),
                worker: Some(worker),
            })
        }

        #[cfg(not(target_os = "linux"))]
        {
            let start_stat = read_proc_stat().unwrap_or_default();
            let mut sampler = CpuSampler::new()?;
            let start_instant = Instant::now();
            let (init_rss, init_hwm) = read_rss_pair_mib();
            let initial_peak_rss = init_hwm.max(init_rss);

            let (stop_tx, stop_rx) = mpsc::channel::<()>();

            let worker = thread::Builder::new()
                .name("mq-cpu-profiler".into())
                .spawn(move || {
                    let mut total_sum_pct = 0.0_f64;
                    let mut per_core_sum_pct: Vec<f64> = Vec::new();
                    let mut per_core_peak_pct: Vec<f32> = Vec::new();
                    let mut sample_count = 0u64;
                    let mut peak_rss_mib = initial_peak_rss;

                    let mut record = |snap: CpuTelemetrySnapshot| {
                        if per_core_peak_pct.len() < snap.per_core_pct.len() {
                            per_core_peak_pct.resize(snap.per_core_pct.len(), 0.0);
                        }
                        if per_core_sum_pct.len() < snap.per_core_pct.len() {
                            per_core_sum_pct.resize(snap.per_core_pct.len(), 0.0);
                        }
                        for (peak, &pct) in
                            per_core_peak_pct.iter_mut().zip(snap.per_core_pct.iter())
                        {
                            *peak = peak.max(pct);
                        }
                        for (sum, &pct) in per_core_sum_pct.iter_mut().zip(snap.per_core_pct.iter())
                        {
                            *sum += f64::from(pct);
                        }
                        total_sum_pct += f64::from(snap.total_pct);
                        sample_count = sample_count.saturating_add(1);
                        peak_rss_mib = peak_rss_mib.max(snap.rss_mib).max(snap.peak_rss_mib);
                    };

                    loop {
                        match stop_rx.recv_timeout(poll_interval) {
                            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                                if let Ok(snap) = sampler.sample() {
                                    record(snap);
                                }
                                break;
                            }
                            Err(RecvTimeoutError::Timeout) => {
                                if let Ok(snap) = sampler.sample() {
                                    record(snap);
                                }
                            }
                        }
                    }

                    BgSampleStats {
                        total_sum_pct,
                        per_core_sum_pct,
                        per_core_peak_pct,
                        sample_count,
                        peak_rss_mib,
                    }
                })?;

            Ok(Self {
                start_instant,
                start_stat,
                stop_tx: Some(stop_tx),
                worker: Some(worker),
            })
        }
    }

    /// Stops the background sampler and computes the `PhaseCpuReport` over the phase window.
    pub fn finish(mut self) -> PhaseCpuReport {
        self.stop_and_collect()
    }

    /// Alias for [`ScopedCpuProfiler::finish`].
    pub fn stop(self) -> PhaseCpuReport {
        self.finish()
    }

    fn stop_and_collect(&mut self) -> PhaseCpuReport {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }

        #[cfg(target_os = "linux")]
        {
            let bg = self
                .worker
                .take()
                .and_then(|h| h.join().ok())
                .unwrap_or_else(|| BgSampleStats {
                    per_core_peak_pct: vec![0.0; self.start_stat.cores.len()],
                    peak_rss_mib: 0.0,
                });

            let elapsed_ms = self.start_instant.elapsed().as_secs_f64() * 1000.0;
            let end_stat = read_proc_stat().unwrap_or_else(|_| self.start_stat.clone());
            let (mean_total_pct, per_core_mean_pct) = end_stat.utilization_delta(&self.start_stat);

            let mut per_core_peak_pct = bg.per_core_peak_pct;
            if per_core_peak_pct.len() < per_core_mean_pct.len() {
                per_core_peak_pct.resize(per_core_mean_pct.len(), 0.0);
            }
            for (peak, &mean) in per_core_peak_pct.iter_mut().zip(per_core_mean_pct.iter()) {
                *peak = clamp_cpu_pct(peak.max(mean));
            }

            let (end_rss, end_hwm) = read_rss_pair_mib();
            let peak_rss_mib = bg.peak_rss_mib.max(end_rss).max(end_hwm);

            PhaseCpuReport {
                elapsed_ms,
                mean_total_pct,
                per_core_mean_pct,
                per_core_peak_pct,
                peak_rss_mib,
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            let bg = self
                .worker
                .take()
                .and_then(|h| h.join().ok())
                .unwrap_or(BgSampleStats {
                    total_sum_pct: 0.0,
                    per_core_sum_pct: Vec::new(),
                    per_core_peak_pct: Vec::new(),
                    sample_count: 0,
                    peak_rss_mib: 0.0,
                });

            let elapsed_ms = self.start_instant.elapsed().as_secs_f64() * 1000.0;
            let denom = bg.sample_count.max(1) as f64;
            let per_core_mean_pct: Vec<f32> = if bg.sample_count > 0 {
                bg.per_core_sum_pct
                    .iter()
                    .map(|&sum| clamp_cpu_pct((sum / denom) as f32))
                    .collect()
            } else {
                Vec::new()
            };

            let raw_total = read_proc_stat()
                .ok()
                .map(|end| end.utilization_delta(&self.start_stat).0)
                .unwrap_or(0.0);
            let mean_total_pct = if raw_total > 0.0 {
                clamp_cpu_pct(raw_total)
            } else if bg.sample_count > 0 {
                clamp_cpu_pct((bg.total_sum_pct / denom) as f32)
            } else {
                0.0
            };

            let mut per_core_peak_pct = bg.per_core_peak_pct;
            if per_core_peak_pct.len() < per_core_mean_pct.len() {
                per_core_peak_pct.resize(per_core_mean_pct.len(), 0.0);
            }
            for (peak, &mean) in per_core_peak_pct.iter_mut().zip(per_core_mean_pct.iter()) {
                *peak = clamp_cpu_pct(peak.max(mean));
            }

            let (end_rss, end_hwm) = read_rss_pair_mib();
            let peak_rss_mib = bg.peak_rss_mib.max(end_rss).max(end_hwm);

            PhaseCpuReport {
                elapsed_ms,
                mean_total_pct,
                per_core_mean_pct,
                per_core_peak_pct,
                peak_rss_mib,
            }
        }
    }
}

impl Drop for ScopedCpuProfiler {
    fn drop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use std::fs::File;

    const SAMPLE_STAT_T0: &str = "\
cpu  1000 100 300 8000 500 50 50 0 20 10
cpu0 500 50 150 4000 250 25 25 0 10 5
cpu1 500 50 150 4000 250 25 25 0 10 5
ctxt 123456
btime 1700000000
processes 4200
";

    const SAMPLE_STAT_T1: &str = "\
cpu  1120 110 340 8050 530 60 60 10 25 10
cpu1 520 50 160 4050 270 25 25 0 10 5
cpu0 600 60 180 4000 260 35 35 10 15 5
ctxt 124000
";

    #[test]
    fn synthetic_proc_stat_parsing_and_delta_math() {
        let s0 = parse_proc_stat(SAMPLE_STAT_T0);
        let s1 = parse_proc_stat(SAMPLE_STAT_T1);

        assert_eq!(s0.cores.len(), 2);
        assert_eq!(s1.cores.len(), 2);
        // Verify cores are sorted even when cpu1 appears before cpu0 in input.
        assert_eq!(s1.cores[0].core_id, Some(0));
        assert_eq!(s1.cores[1].core_id, Some(1));

        // t0 aggregate: busy = 1000+100+300+50+50+0 = 1500, idle+iowait = 8500, total = 10000
        assert_eq!(s0.total.busy(), 1500);
        assert_eq!(s0.total.total(), 10_000);

        // t1 cpu0:
        // t0 busy = 500+50+150+25+25+0 = 750, total = 750 + 4000 + 250 = 5000
        // t1 busy = 600+60+180+35+35+10 = 920, total = 920 + 4000 + 260 = 5180
        // delta_busy = 170, delta_total = 180 => 94.44444%
        let mut sampler = CpuSampler::from_raw(s0.clone());
        let (total_pct, per_core) = sampler.sample_raw(s1.clone());

        let expected_cpu0 = (170.0_f64 / 180.0_f64 * 100.0) as f32;
        // t1 cpu1:
        // t0 busy = 750, total = 5000
        // t1 busy = 520+50+160+25+25+0 = 780, total = 780 + 4050 + 270 = 5100
        // delta_busy = 30, delta_total = 100 => 30.0%
        let expected_cpu1 = 30.0_f32;
        // aggregate: delta_busy = 200, delta_total = 280 => 71.42857%
        let expected_total = (200.0_f64 / 280.0_f64 * 100.0) as f32;

        assert!((total_pct - expected_total).abs() < 1e-4);
        assert!((per_core[0] - expected_cpu0).abs() < 1e-4);
        assert!((per_core[1] - expected_cpu1).abs() < 1e-4);

        // Zero-delta sample must return 0.0 (never NaN).
        let (zero_total, zero_cores) = sampler.sample_raw(s1);
        assert_eq!(zero_total, 0.0);
        assert_eq!(zero_cores, vec![0.0, 0.0]);
    }

    #[test]
    fn clamp_cpu_pct_and_atomic_peak_rss_helpers() {
        assert_eq!(clamp_cpu_pct(f32::NAN), 0.0);
        assert_eq!(clamp_cpu_pct(f32::INFINITY), 0.0);
        assert_eq!(clamp_cpu_pct(f32::NEG_INFINITY), 0.0);
        assert_eq!(clamp_cpu_pct(-5.0), 0.0);
        assert_eq!(clamp_cpu_pct(142.5), 100.0);
        assert!((clamp_cpu_pct(37.25) - 37.25).abs() < 1e-6);

        let peak = AtomicU64::new(0);
        assert_eq!(record_peak_bytes(&peak, 16 * 1024 * 1024), 16 * 1024 * 1024);
        assert_eq!(record_peak_bytes(&peak, 64 * 1024 * 1024), 64 * 1024 * 1024);
        // Subsequent lower RSS must preserve the high-water mark.
        assert_eq!(record_peak_bytes(&peak, 32 * 1024 * 1024), 64 * 1024 * 1024);

        // Verify GetSystemTimes-style aggregate-only ProcStatRaw delta calculation.
        let t0 = ProcStatRaw {
            total: CoreSample {
                core_id: None,
                user: 1_000,
                system: 500,
                idle: 8_500,
                ..CoreSample::default()
            },
            cores: Vec::new(),
        };
        let t1 = ProcStatRaw {
            total: CoreSample {
                core_id: None,
                user: 1_600,
                system: 900,
                idle: 9_500,
                ..CoreSample::default()
            },
            cores: Vec::new(),
        };
        let (total_pct, cores) = t1.utilization_delta(&t0);
        // delta_busy = 600 + 400 = 1000, delta_idle = 1000 => 50.0%
        assert!((total_pct - 50.0).abs() < 1e-4);
        assert!(cores.is_empty());
    }

    #[test]
    fn parse_proc_status_kb_extracts_rss_and_hwm() {
        let status = "\
Name:\tmq-sched
State:\tR (running)
VmPeak:\t   65536 kB
VmHWM:\t   32768 kB
VmRSS:\t   16384 kB
Threads:\t4
";
        assert_eq!(parse_status_kb(status, "VmRSS"), Some(16384));
        assert_eq!(parse_status_kb(status, "VmHWM:"), Some(32768));
        assert_eq!(parse_status_kb(status, "NonExistent"), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn live_linux_proc_stat_rss_and_fd_count() {
        let mut sampler = CpuSampler::new().expect("read /proc/stat baseline");
        let snap = sampler.sample().expect("sample /proc/stat");

        assert!(!snap.per_core_pct.is_empty());
        assert!((0.0..=100.0).contains(&snap.total_pct));
        for &pct in &snap.per_core_pct {
            assert!((0.0..=100.0).contains(&pct));
        }

        let rss = read_current_rss_mib();
        let hwm = read_peak_rss_mib();
        assert!(rss > 0.0, "expected positive VmRSS MiB, got {rss}");
        assert!(hwm >= rss * 0.5, "expected positive VmHWM MiB, got {hwm}");

        let fds_before = count_open_fds();
        assert!(
            fds_before >= 3,
            "expected at least stdin/stdout/stderr open"
        );
        {
            let _extra = File::open("/proc/stat").expect("open /proc/stat");
            let fds_during = count_open_fds();
            assert_eq!(fds_during, fds_before + 1);
        }
        let fds_after = count_open_fds();
        assert_eq!(fds_after, fds_before);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn live_windows_telemetry_sampler_rss_and_handle_count() {
        let mut sampler = CpuSampler::new().expect("initialize Windows CpuSampler");
        thread::sleep(Duration::from_millis(50));
        let snap = sampler.sample().expect("sample Windows CPU telemetry");

        assert!(!snap.per_core_pct.is_empty());
        assert!((0.0..=100.0).contains(&snap.total_pct));
        for &pct in &snap.per_core_pct {
            assert!((0.0..=100.0).contains(&pct));
        }
        assert!(snap.rss_mib > 0.0, "expected positive RSS MiB on Windows");
        assert!(snap.peak_rss_mib >= snap.rss_mib);

        let rss = read_current_rss_mib();
        let peak = read_peak_rss_mib();
        assert!(rss > 0.0);
        assert!(peak >= rss * 0.5);

        let handles = count_open_fds();
        assert!(handles > 0, "expected positive Win32 process handle count");

        let raw = read_proc_stat().expect("GetSystemTimes should succeed on Windows");
        assert!(raw.total.total() > 0);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn scoped_cpu_profiler_tracks_multithreaded_busy_loop() {
        let profiler = ScopedCpuProfiler::start().expect("start profiler");

        let workers: Vec<_> = (0..2)
            .map(|_| {
                thread::spawn(|| {
                    let deadline = Instant::now() + Duration::from_millis(90);
                    let mut acc = 1u64;
                    while Instant::now() < deadline {
                        for i in 1..10_000u64 {
                            acc = std::hint::black_box(
                                acc.wrapping_mul(6364136223846793005).wrapping_add(i),
                            );
                        }
                    }
                    acc
                })
            })
            .collect();

        for w in workers {
            let _ = w.join().unwrap();
        }

        let report = profiler.finish();
        assert!(report.elapsed_ms >= 80.0);
        assert!(
            report.mean_total_pct > 0.0,
            "expected non-zero CPU utilization, got {:?}",
            report
        );
        assert!((0.0..=100.0).contains(&report.mean_total_pct));
        assert!(!report.per_core_mean_pct.is_empty());
        assert_eq!(
            report.per_core_mean_pct.len(),
            report.per_core_peak_pct.len()
        );
        assert!(report.per_core_mean_pct.iter().any(|&p| p > 0.0));
        for (&mean, &peak) in report
            .per_core_mean_pct
            .iter()
            .zip(report.per_core_peak_pct.iter())
        {
            assert!((0.0..=100.0).contains(&mean));
            assert!((0.0..=100.0).contains(&peak));
            assert!(peak >= mean);
        }
        assert!(report.peak_rss_mib > 0.0);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn scoped_cpu_profiler_works_on_windows() {
        let profiler = ScopedCpuProfiler::start().expect("start Windows profiler");
        let deadline = Instant::now() + Duration::from_millis(80);
        let mut acc = 1u64;
        while Instant::now() < deadline {
            for i in 1..10_000u64 {
                acc = std::hint::black_box(acc.wrapping_mul(6364136223846793005).wrapping_add(i));
            }
        }
        let _ = std::hint::black_box(acc);
        let report = profiler.finish();
        assert!(report.elapsed_ms >= 60.0);
        assert!((0.0..=100.0).contains(&report.mean_total_pct));
        assert!(!report.per_core_mean_pct.is_empty());
        assert_eq!(
            report.per_core_mean_pct.len(),
            report.per_core_peak_pct.len()
        );
        for (&mean, &peak) in report
            .per_core_mean_pct
            .iter()
            .zip(report.per_core_peak_pct.iter())
        {
            assert!((0.0..=100.0).contains(&mean));
            assert!((0.0..=100.0).contains(&peak));
            assert!(peak >= mean);
        }
        assert!(report.peak_rss_mib > 0.0);
    }
}
