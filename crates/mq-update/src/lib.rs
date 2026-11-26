//! Self-update engine for MQ Launcher.
//! Implements GitHub Releases polling, minisign signature verification, SHA-256 integrity,
//! atomic `<exe>.new` substitution, downgrade protection, smoke-testing, rollback,
//! and package-manager safety across Linux and Windows.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use mq_core::fs_util::{
    is_transient_windows_lock_error, remove_dir_all_with_retry, remove_file_with_retry,
    rename_with_retry, strip_windows_verbatim_prefix, RETRY_DELAYS_MS,
};
#[cfg(target_os = "windows")]
use mq_core::WINDOWS_CREATE_NO_WINDOW;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::info;

/// Embedded minisign public key for validating release checksums and binaries.
pub const MQ_RELEASE_PUBLIC_KEY: &str = "RWRlxrUOJgeLq7AIndJRG9ZhsdT5QiHy9usykXvxt1C49a1h/km3h9lt";

/// Appends `suffix` (such as `".old"` or `".failed"`) to `exe` without replacing an existing `.exe` extension.
///
/// - Linux: `/home/user/.local/bin/mq` + `".old"` -> `/home/user/.local/bin/mq.old`
/// - Windows: `C:\MQ\mq.exe` + `".old"` -> `C:\MQ\mq.exe.old`
pub fn binary_sidecar_path(exe: &Path, suffix: &str) -> PathBuf {
    let mut os = exe.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

/// Computes the staged new-binary path adjacent to `exe`.
///
/// On Windows binaries ending with `.exe`, preserves the `.exe` extension (`mq.exe` -> `mq.new.exe`)
/// so Windows `CreateProcess`, AppLocker, and antivirus scanners recognize it as a valid executable
/// during the `--version` smoke-test. On Unix, appends `.new` (`mq` -> `mq.new`).
pub fn staged_new_binary_path(exe: &Path) -> PathBuf {
    if let Some(name) = exe.file_name().and_then(|n| n.to_str()) {
        let bytes = name.as_bytes();
        if bytes.len() > 4 && bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".exe") {
            let split = bytes.len() - 4;
            let stem = &name[..split];
            let ext = &name[split..];
            return exe.with_file_name(format!("{stem}.new{ext}"));
        }
    }
    binary_sidecar_path(exe, ".new")
}

/// If `exe` ends with a `.old` backup suffix (for example after renaming the running binary
/// to `<exe>.old` before restarting), returns the path with `.old` stripped.
pub fn strip_old_backup_suffix(exe: &Path) -> Option<PathBuf> {
    let clean = strip_windows_verbatim_prefix(exe);
    let name = clean.file_name()?.to_str()?;
    let stripped = name.strip_suffix(".old")?;
    if stripped.is_empty() {
        return None;
    }
    Some(clean.with_file_name(stripped))
}

fn run_command_output_with_av_retry(
    mut build_cmd: impl FnMut() -> Command,
) -> std::io::Result<Output> {
    let mut attempt = 0;
    loop {
        let mut cmd = build_cmd();
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(WINDOWS_CREATE_NO_WINDOW);
        }
        match cmd.output() {
            Ok(out) => return Ok(out),
            Err(err)
                if is_transient_windows_lock_error(&err) && attempt < RETRY_DELAYS_MS.len() =>
            {
                std::thread::sleep(Duration::from_millis(RETRY_DELAYS_MS[attempt]));
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

fn spawn_command_with_av_retry(
    mut build_cmd: impl FnMut() -> Command,
) -> std::io::Result<std::process::Child> {
    let mut attempt = 0;
    loop {
        let mut cmd = build_cmd();
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(err)
                if is_transient_windows_lock_error(&err) && attempt < RETRY_DELAYS_MS.len() =>
            {
                std::thread::sleep(Duration::from_millis(RETRY_DELAYS_MS[attempt]));
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Performs the atomic binary swap `current_exe -> old_exe`, `new_exe -> current_exe`,
/// with transient Windows Defender retry and immediate rollback if moving `new_exe` fails.
pub fn swap_binary_files(current_exe: &Path, new_exe: &Path, old_exe: &Path) -> Result<()> {
    if old_exe.exists() {
        let _ = remove_file_with_retry(old_exe);
    }

    if let Err(e) = rename_with_retry(current_exe, old_exe) {
        let _ = remove_file_with_retry(new_exe);
        bail!("Failed to rename current binary to old: {e}");
    }

    if let Err(e) = rename_with_retry(new_exe, current_exe) {
        // Rollback immediately so the launcher binary is never left missing.
        let _ = rename_with_retry(old_exe, current_exe);
        let _ = remove_file_with_retry(new_exe);
        bail!("Failed to install new binary: {e}. Restored old binary.");
    }

    Ok(())
}

/// Minisign signature verifier for release manifests and checksums.
pub struct MinisignVerifier;

impl MinisignVerifier {
    /// Verifies the given `checksums_content` using the embedded official public key.
    pub fn verify_checksums(checksums_content: &[u8], signature_box_str: &str) -> Result<()> {
        Self::verify(checksums_content, signature_box_str, MQ_RELEASE_PUBLIC_KEY)
    }

    /// Verifies arbitrary content and minisign signature against a base64 public key string.
    pub fn verify(content: &[u8], signature_box_str: &str, public_key_base64: &str) -> Result<()> {
        let pk = minisign_verify::PublicKey::from_base64(public_key_base64)
            .context("Неверный формат публичного ключа Minisign")?;
        let sig = minisign_verify::Signature::decode(signature_box_str)
            .context("Неверный формат подписи Minisign")?;
        pk.verify(content, &sig, false)
            .context("Верификация цифровой подписи Minisign не прошла")?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsUpdateStrategy {
    /// Installed via Inno Setup (`unins000.exe` present, no `portable.flag`): download and run `MQ-Setup-<version>-x64.exe` silently.
    InnoSetupInstaller,
    /// Portable mode (`portable.flag` present or no `unins000.exe`): atomic binary swap.
    PortableAtomicSwap,
}

/// Selects the Windows self-update strategy based on marker files next to `mq.exe`.
pub fn select_windows_update_strategy(exe_dir: &Path) -> WindowsUpdateStrategy {
    if exe_dir.join("portable.flag").is_file() {
        return WindowsUpdateStrategy::PortableAtomicSwap;
    }
    if exe_dir.join("unins000.exe").is_file() {
        return WindowsUpdateStrategy::InnoSetupInstaller;
    }
    WindowsUpdateStrategy::PortableAtomicSwap
}

/// Returns the canonical Inno Setup installer asset filename for `version`.
pub fn expected_inno_installer_name(version: &str) -> String {
    format!("MQ-Setup-{}-x64.exe", version.trim_start_matches('v'))
}

/// Returns the silent Inno Setup command-line arguments.
pub fn inno_silent_args() -> [&'static str; 3] {
    ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]
}

fn is_loopback_host(host_lc: &str) -> bool {
    matches!(host_lc, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

fn validate_release_url(url: &reqwest::Url) -> Result<()> {
    if !url.username().is_empty() || url.password().is_some() {
        bail!("Release URL must not contain credentials");
    }
    let Some(host) = url.host_str() else {
        bail!("Release URL is missing host: {url}");
    };
    let host_lc = host.to_ascii_lowercase();
    let is_loopback = is_loopback_host(&host_lc);
    let is_allowed_github = matches!(
        host_lc.as_str(),
        "github.com"
            | "api.github.com"
            | "objects.githubusercontent.com"
            | "release-assets.githubusercontent.com"
            | "githubusercontent.com"
    ) || host_lc.ends_with(".githubusercontent.com");

    match url.scheme() {
        "https" if is_allowed_github || is_loopback => Ok(()),
        "https" => bail!("Untrusted release download host '{host}' in URL: {url}"),
        "http" if is_loopback => Ok(()),
        "http" => bail!("Insecure HTTP release download URL rejected: {url}"),
        other => bail!("Unsupported URL scheme '{other}' in release download URL: {url}"),
    }
}

/// Validates that a release download URL uses `https://` on an official GitHub release host
/// (or `http://127.0.0.1` / `localhost` for local mock-server unit tests).
pub fn validate_release_download_url(raw_url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(raw_url.trim())
        .with_context(|| format!("Invalid release download URL: {raw_url}"))?;
    validate_release_url(&parsed)
}

fn build_release_http_client() -> reqwest::Client {
    let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("Too many HTTP redirects while fetching release asset");
        }
        if attempt
            .previous()
            .iter()
            .any(|prev| prev.scheme() == "https")
            && attempt.url().scheme() != "https"
        {
            return attempt.error("Blocked HTTPS to HTTP scheme downgrade redirect");
        }
        let prev_was_remote = attempt.previous().iter().any(|prev| {
            prev.host_str()
                .is_some_and(|h| !is_loopback_host(&h.to_ascii_lowercase()))
        });
        let next_is_loopback = attempt
            .url()
            .host_str()
            .is_some_and(|h| is_loopback_host(&h.to_ascii_lowercase()));
        if prev_was_remote && next_is_loopback {
            return attempt.error("Blocked redirect from external release host to loopback");
        }
        if let Err(err) = validate_release_url(attempt.url()) {
            return attempt.error(err.to_string());
        }
        attempt.follow()
    });

    reqwest::Client::builder()
        .user_agent(concat!("mq-updater/", env!("CARGO_PKG_VERSION")))
        .redirect(redirect_policy)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_default()
}

#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn encode_powershell_utf16le_base64(script: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn create_unique_installer_temp_dir() -> Result<PathBuf> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let base = strip_windows_verbatim_prefix(&std::env::temp_dir());
    let pid = std::process::id();
    for attempt in 0..32u32 {
        let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let ts = Utc::now().timestamp_nanos_opt().unwrap_or(0).unsigned_abs();
        let candidate = base.join(format!("mq-inno-update-{pid}-{ts:x}-{seq}-{attempt}"));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(e).context("Failed to create unique temporary installer directory")
            }
        }
    }
    bail!("Failed to allocate unique temporary directory for Windows installer");
}

/// Extracts the SHA-256 hex digest for the highest-priority matching filename in `SHA256SUMS`.
///
/// Handles UTF-8 BOM (`\u{FEFF}`), CRLF (`\r\n`) and LF (`\n`) line endings, binary-mode `*`
/// markers (`<hash> *file`), relative prefixes (`./file`, `.\file`), and filenames with spaces.
pub fn extract_sha256_for_targets(
    checksums_text: &str,
    target_filenames: &[&str],
) -> Option<String> {
    let mut best_match: Option<(usize, String)> = None;

    for raw_line in checksums_text.trim_start_matches('\u{FEFF}').lines() {
        let line = raw_line.trim_start_matches('\u{FEFF}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(ws_idx) = line.find(char::is_whitespace) else {
            continue;
        };
        let hash = &line[..ws_idx];
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let rest = line[ws_idx..].trim_start();
        let filename = rest
            .strip_prefix('*')
            .unwrap_or(rest)
            .trim_start_matches("./")
            .trim_start_matches(r".\");

        if let Some(priority) = target_filenames.iter().position(|&t| t == filename) {
            let normalized = hash.to_ascii_lowercase();
            if priority == 0 {
                return Some(normalized);
            }
            if best_match
                .as_ref()
                .is_none_or(|(best_prio, _)| priority < *best_prio)
            {
                best_match = Some((priority, normalized));
            }
        }
    }

    best_match.map(|(_, hash)| hash)
}

/// Verifies `SHA256SUMS` against a Minisign signature using `public_key_base64` and extracts the SHA-256 for `target_filenames`.
pub fn verify_checksums_and_extract_sha256(
    checksums_content: &[u8],
    signature_box_str: &str,
    target_filenames: &[&str],
    public_key_base64: &str,
) -> Result<String> {
    MinisignVerifier::verify(checksums_content, signature_box_str, public_key_base64)?;
    let chk_text = String::from_utf8_lossy(checksums_content);
    extract_sha256_for_targets(&chk_text, target_filenames)
        .with_context(|| format!("SHA256SUMS does not contain any of {:?}", target_filenames))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseAssetInfo {
    pub version: String,
    pub release_notes: String,
    pub binary_url: String,
    pub binary_sha256: String,
    pub tarball_url: String,
    #[serde(default)]
    pub installer_url: Option<String>,
    #[serde(default)]
    pub installer_sha256: Option<String>,
    #[serde(default)]
    pub installer_filename: Option<String>,
    pub published_at: DateTime<Utc>,
    pub is_prerelease: bool,
}

#[derive(Debug, Clone)]
pub enum UpdateState {
    Idle,
    Checking,
    UpdateAvailable(ReleaseAssetInfo),
    Downloading {
        progress: f32,
        bytes: u64,
        total: Option<u64>,
    },
    Verifying,
    ReadyToRestart {
        exe_path: PathBuf,
        old_path: PathBuf,
    },
    PackageManagerManaged {
        message: String,
        release_url: String,
    },
    Error(String),
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct GitHubRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    published_at: Option<DateTime<Utc>>,
    prerelease: bool,
    draft: bool,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

pub struct UpdateManager {
    repo_owner: String,
    repo_name: String,
    client: reqwest::Client,
    api_base_url: Option<String>,
    relaunch_on_inno_update: bool,
}

impl UpdateManager {
    pub fn new(repo_owner: &str, repo_name: &str) -> Self {
        Self {
            repo_owner: repo_owner.to_string(),
            repo_name: repo_name.to_string(),
            client: build_release_http_client(),
            api_base_url: None,
            relaunch_on_inno_update: true,
        }
    }

    pub fn with_api_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.api_base_url = Some(base_url.into());
        self
    }

    /// Controls whether silent Windows Inno Setup self-updates pass `/RELAUNCH=1`
    /// so the installer automatically restarts MQ after updating.
    pub fn with_relaunch_on_update(mut self, relaunch: bool) -> Self {
        self.relaunch_on_inno_update = relaunch;
        self
    }

    /// Check if the binary is located under `/usr/` or an unwriteable system directory.
    pub fn is_packaged_install() -> bool {
        if let Ok(raw_exe) = std::env::current_exe() {
            let exe_path = strip_windows_verbatim_prefix(&raw_exe);

            #[cfg(unix)]
            {
                let path_str = exe_path.to_string_lossy();
                if path_str.starts_with("/usr/") || path_str.starts_with("/var/") {
                    return true;
                }
                if let Some(parent) = exe_path.parent() {
                    if let Ok(metadata) = fs::metadata(parent) {
                        if metadata.permissions().readonly() {
                            return true;
                        }
                    }
                }
            }

            #[cfg(target_os = "windows")]
            {
                // Inno Setup installations manage their own updates via MQ-Setup-*-x64.exe.
                if Self::is_windows_inno_setup() {
                    return false;
                }
                // On Windows, directory `FILE_ATTRIBUTE_READONLY` is used by Explorer for
                // desktop.ini / OneDrive customization and does NOT indicate ACL write-protection.
                // Probe actual write access in the binary's parent directory instead.
                if let Some(parent) = exe_path.parent() {
                    let probe = parent.join(format!(".mq-write-probe.{}", std::process::id()));
                    match fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&probe)
                    {
                        Ok(file) => {
                            drop(file);
                            let _ = remove_file_with_retry(&probe);
                            return false;
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                            let _ = remove_file_with_retry(&probe);
                            return false;
                        }
                        Err(_) => return true,
                    }
                }
            }
        }
        false
    }

    /// Checks if running as an Inno Setup installed application on Windows (`unins000.exe` present and no `portable.flag`).
    pub fn is_windows_inno_setup() -> bool {
        if cfg!(target_os = "windows") {
            if let Ok(exe) = std::env::current_exe() {
                let clean_exe = strip_windows_verbatim_prefix(&exe);
                if let Some(dir) = clean_exe.parent() {
                    return select_windows_update_strategy(dir)
                        == WindowsUpdateStrategy::InnoSetupInstaller;
                }
            }
        }
        false
    }

    /// Removes `.old`, `.failed`, and stale `.new` binaries left over from prior atomic swaps,
    /// plus stale `mq-inno-update-*` temporary installer directories.
    /// Uses retry backoff on Windows so an exiting parent `mq.exe.old` finishes releasing its handle.
    pub fn clean_old_binary_backups() {
        if let Ok(raw_exe) = std::env::current_exe() {
            let current_exe = strip_windows_verbatim_prefix(&raw_exe);
            let candidates = [
                binary_sidecar_path(&current_exe, ".old"),
                current_exe.with_extension("old"),
                binary_sidecar_path(&current_exe, ".failed"),
                current_exe.with_extension("failed"),
                staged_new_binary_path(&current_exe),
                binary_sidecar_path(&current_exe, ".new"),
            ];
            for candidate in candidates {
                if candidate != current_exe && candidate.exists() {
                    let _ = remove_file_with_retry(&candidate);
                }
            }
        }

        let temp_base = strip_windows_verbatim_prefix(&std::env::temp_dir());
        if let Ok(entries) = fs::read_dir(temp_base) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.starts_with("mq-inno-update-") {
                        if let Ok(meta) = entry.metadata() {
                            let is_stale = meta
                                .modified()
                                .ok()
                                .and_then(|m| m.elapsed().ok())
                                .is_some_and(|age| age > Duration::from_secs(600));
                            if is_stale {
                                let _ = remove_dir_all_with_retry(&entry.path());
                            }
                        }
                    }
                }
            }
        }
    }

    /// Query GitHub Releases for updates, enforcing downgrade protection and signature checking.
    pub async fn check_latest(
        &self,
        current_version: &str,
        include_prereleases: bool,
    ) -> Result<Option<ReleaseAssetInfo>> {
        let base = self
            .api_base_url
            .clone()
            .unwrap_or_else(|| "https://api.github.com".to_string());

        let url = if include_prereleases {
            format!(
                "{base}/repos/{}/{}/releases",
                self.repo_owner, self.repo_name
            )
        } else {
            format!(
                "{base}/repos/{}/{}/releases/latest",
                self.repo_owner, self.repo_name
            )
        };

        let response = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .await
            .context("Failed to query GitHub Releases API")?;

        if !response.status().is_success() {
            bail!("GitHub API returned status: {}", response.status());
        }

        let release: GitHubRelease = if include_prereleases {
            let releases: Vec<GitHubRelease> = response.json().await?;
            let valid = releases
                .into_iter()
                .find(|r| !r.draft && (include_prereleases || !r.prerelease));
            match valid {
                Some(r) => r,
                None => return Ok(None),
            }
        } else {
            response.json().await?
        };

        let remote_ver_str = release.tag_name.trim_start_matches('v');
        // Downgrade protection: reject equal or older versions
        if !is_version_newer(current_version, remote_ver_str) {
            return Ok(None);
        }

        let target_binary_candidates: &[&str] = if cfg!(target_os = "windows") {
            &["mq-windows-x86_64.exe", "mq.exe"]
        } else {
            &["mq-linux-x86_64", "mq"]
        };
        let preferred_archive_ext = if cfg!(target_os = "windows") {
            ".zip"
        } else {
            ".tar.gz"
        };
        let expected_installer = expected_inno_installer_name(remote_ver_str);

        let mut binary_url = String::new();
        let mut selected_binary_name: Option<String> = None;
        let mut binary_priority = usize::MAX;
        let mut tarball_url = String::new();
        let mut has_preferred_archive = false;
        let mut binary_sha256 = String::new();
        let mut installer_url = None;
        let mut installer_sha256 = None;
        let mut installer_filename = None;

        let mut checksums_asset = None;
        let mut signature_asset = None;

        for asset in &release.assets {
            if let Some(prio) = target_binary_candidates
                .iter()
                .position(|&name| asset.name == name)
            {
                if prio < binary_priority {
                    binary_priority = prio;
                    binary_url = asset.browser_download_url.clone();
                    selected_binary_name = Some(asset.name.clone());
                }
            } else if asset.name == expected_installer {
                installer_url = Some(asset.browser_download_url.clone());
                installer_filename = Some(asset.name.clone());
            } else if asset.name.ends_with(".tar.gz") || asset.name.ends_with(".zip") {
                let is_preferred = asset.name.ends_with(preferred_archive_ext);
                if is_preferred || !has_preferred_archive {
                    tarball_url = asset.browser_download_url.clone();
                    if is_preferred {
                        has_preferred_archive = true;
                    }
                }
            } else if asset.name == "SHA256SUMS" {
                checksums_asset = Some(asset);
            } else if asset.name == "SHA256SUMS.minisig" {
                signature_asset = Some(asset);
            }
        }

        let inno_mode = Self::is_windows_inno_setup();
        if inno_mode {
            if installer_url.is_none() {
                tracing::warn!(
                    "Release v{} is missing expected Windows Inno Setup installer '{}'; skipping update report",
                    remote_ver_str,
                    expected_installer
                );
                return Ok(None);
            }
        } else if binary_url.is_empty() {
            return Ok(None);
        }

        if !binary_url.is_empty() {
            validate_release_download_url(&binary_url)?;
        }
        if let Some(ref inst_url) = installer_url {
            validate_release_download_url(inst_url)?;
        }
        if !tarball_url.is_empty() {
            validate_release_download_url(&tarball_url)?;
        }

        // Always require SHA256SUMS to be present on the release
        let chk_asset =
            checksums_asset.context("Release is missing required SHA256SUMS checksum manifest")?;
        validate_release_download_url(&chk_asset.browser_download_url)?;

        let chk_resp = self
            .client
            .get(&chk_asset.browser_download_url)
            .send()
            .await
            .context("Failed to download SHA256SUMS")?;
        if !chk_resp.status().is_success() {
            bail!("Failed to download SHA256SUMS: {}", chk_resp.status());
        }
        let chk_bytes = chk_resp.bytes().await?;

        if let Some(sig_asset) = signature_asset {
            validate_release_download_url(&sig_asset.browser_download_url)?;
            let sig_resp = self
                .client
                .get(&sig_asset.browser_download_url)
                .send()
                .await
                .context("Failed to download SHA256SUMS.minisig")?;
            if !sig_resp.status().is_success() {
                bail!(
                    "Failed to download SHA256SUMS.minisig: {}",
                    sig_resp.status()
                );
            }
            let sig_text = sig_resp.text().await?;
            MinisignVerifier::verify_checksums(&chk_bytes, &sig_text)
                .context("Minisign release signature verification failed")?;
            info!("Release SHA256SUMS verified with Minisign Ed25519 signature!");
        } else {
            tracing::warn!(
                "Release v{} is missing SHA256SUMS.minisig; proceeding with HTTPS + SHA256SUMS verification",
                remote_ver_str
            );
        }

        let chk_text = String::from_utf8_lossy(&chk_bytes);
        if let Some(ref bin_name) = selected_binary_name {
            binary_sha256 = extract_sha256_for_targets(&chk_text, &[bin_name.as_str()])
                .with_context(|| {
                    format!(
                        "SHA256SUMS is missing checksum entry matching selected asset '{bin_name}'"
                    )
                })?;
        }
        if let Some(ref inst_name) = installer_filename {
            installer_sha256 = extract_sha256_for_targets(&chk_text, &[inst_name.as_str()]);
            if inno_mode && installer_sha256.is_none() {
                bail!(
                    "SHA256SUMS is missing checksum entry matching Windows installer '{inst_name}'"
                );
            }
        }
        if binary_sha256.is_empty() {
            if let Some(ref inst_hash) = installer_sha256 {
                if inno_mode {
                    binary_sha256 = inst_hash.clone();
                }
            }
        }
        if binary_sha256.is_empty() {
            bail!("Release SHA256SUMS did not yield a valid non-empty binary SHA-256 checksum");
        }

        Ok(Some(ReleaseAssetInfo {
            version: remote_ver_str.to_string(),
            release_notes: release.body.unwrap_or_default(),
            binary_url,
            binary_sha256,
            tarball_url,
            installer_url,
            installer_sha256,
            installer_filename,
            published_at: release.published_at.unwrap_or_else(Utc::now),
            is_prerelease: release.prerelease,
        }))
    }

    /// Download and apply update: on Windows Inno Setup (`unins000.exe` without `portable.flag`),
    /// downloads `MQ-Setup-<version>-x64.exe`, verifies SHA-256, and launches it silently;
    /// otherwise performs an atomic `<exe>.new` binary swap.
    pub async fn download_and_install<F>(
        &self,
        asset: &ReleaseAssetInfo,
        on_progress: F,
    ) -> Result<PathBuf>
    where
        F: Fn(u64, Option<u64>) + Send + 'static,
    {
        if Self::is_windows_inno_setup() {
            return self
                .download_and_run_inno_installer(asset, on_progress)
                .await;
        }

        validate_release_download_url(&asset.binary_url)?;
        let expected_sha256 = asset.binary_sha256.trim();
        if expected_sha256.len() != 64 || !expected_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("Refusing to install update without a valid 64-character SHA-256 checksum");
        }

        let raw_exe = strip_windows_verbatim_prefix(
            &std::env::current_exe().context("Failed to determine current exe path")?,
        );
        let current_exe = strip_old_backup_suffix(&raw_exe).unwrap_or(raw_exe);
        let new_exe = staged_new_binary_path(&current_exe);
        let old_exe = binary_sidecar_path(&current_exe, ".old");

        info!(
            "Downloading new binary from {} to {:?}",
            asset.binary_url, new_exe
        );

        let stage_res: Result<()> = async {
            let resp = self
                .client
                .get(&asset.binary_url)
                .send()
                .await
                .context("Failed to download update binary")?;

            if !resp.status().is_success() {
                bail!("Download failed with status: {}", resp.status());
            }

            let total_size = resp.content_length();
            let mut downloaded: u64 = 0;
            let mut hasher = Sha256::new();

            let mut file = tokio::fs::File::create(&new_exe)
                .await
                .context("Failed to create temporary update binary")?;

            let mut stream = resp.bytes_stream();

            while let Some(chunk_res) = stream.next().await {
                let chunk = chunk_res.context("Error reading binary stream chunk")?;
                hasher.update(&chunk);
                downloaded += chunk.len() as u64;
                tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
                    .await
                    .context("Failed to write to update file")?;
                on_progress(downloaded, total_size);
            }

            tokio::io::AsyncWriteExt::flush(&mut file).await?;
            file.sync_all().await?;
            drop(file);

            let computed_hash = hex::encode(hasher.finalize());
            if !computed_hash.eq_ignore_ascii_case(expected_sha256) {
                bail!(
                    "SHA-256 checksum mismatch! Expected: {}, Computed: {}",
                    expected_sha256,
                    computed_hash
                );
            }
            info!("SHA-256 checksum verified successfully: {}", computed_hash);

            // Set executable permissions (0755 on Unix)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&new_exe)?.permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&new_exe, perms)?;
            }

            // Smoke-test `<new_exe> --version` (hidden console & AV-scan retry on Windows)
            info!("Running smoke-test on {:?}", new_exe);
            let smoke_output = run_command_output_with_av_retry(|| {
                let mut cmd = Command::new(&new_exe);
                cmd.arg("--version");
                cmd
            })
            .context("Failed to execute smoke-test on downloaded binary")?;

            if !smoke_output.status.success() {
                bail!(
                    "Smoke test failed on downloaded binary with code: {:?}",
                    smoke_output.status.code()
                );
            }

            info!(
                "Smoke-test succeeded: {}",
                String::from_utf8_lossy(&smoke_output.stdout).trim()
            );
            Ok(())
        }
        .await;

        if let Err(err) = stage_res {
            let _ = remove_file_with_retry(&new_exe);
            return Err(err);
        }

        // Atomic swap with rollback on failure
        swap_binary_files(&current_exe, &new_exe, &old_exe)?;

        info!("Update successfully installed to {:?}", current_exe);
        Ok(current_exe)
    }

    /// Downloads `MQ-Setup-<version>-x64.exe` into a unique temp directory, verifies SHA-256,
    /// and launches it with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART` (plus `/RELAUNCH=1` when enabled).
    pub async fn download_and_run_inno_installer<F>(
        &self,
        asset: &ReleaseAssetInfo,
        on_progress: F,
    ) -> Result<PathBuf>
    where
        F: Fn(u64, Option<u64>) + Send + 'static,
    {
        let installer_url = asset
            .installer_url
            .as_deref()
            .filter(|u| !u.is_empty())
            .context("Release is missing Windows Inno Setup installer asset")?;
        validate_release_download_url(installer_url)?;

        let expected_sha256 = asset
            .installer_sha256
            .as_deref()
            .map(str::trim)
            .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .context("Missing verified SHA-256 checksum for Windows Inno Setup installer")?;
        let raw_filename = asset
            .installer_filename
            .clone()
            .unwrap_or_else(|| expected_inno_installer_name(&asset.version));
        let safe_filename = Path::new(&raw_filename)
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty() && *n != "." && *n != "..")
            .context("Invalid installer filename")?;

        let temp_subdir = create_unique_installer_temp_dir()?;
        let installer_path = temp_subdir.join(safe_filename);
        info!(
            "Downloading Inno Setup installer from {} to {:?}",
            installer_url, installer_path
        );

        let download_res: Result<()> = async {
            let resp = self
                .client
                .get(installer_url)
                .send()
                .await
                .context("Failed to download Inno Setup installer")?;
            if !resp.status().is_success() {
                bail!("Installer download failed with status: {}", resp.status());
            }

            let total_size = resp.content_length();
            let mut downloaded: u64 = 0;
            let mut hasher = Sha256::new();

            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&installer_path)
                .await
                .context("Failed to create temporary installer file")?;
            let mut stream = resp.bytes_stream();

            while let Some(chunk_res) = stream.next().await {
                let chunk = chunk_res.context("Error reading installer stream chunk")?;
                hasher.update(&chunk);
                downloaded += chunk.len() as u64;
                tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
                    .await
                    .context("Failed to write installer file")?;
                on_progress(downloaded, total_size);
            }

            tokio::io::AsyncWriteExt::flush(&mut file).await?;
            file.sync_all().await?;
            drop(file);

            let computed_hash = hex::encode(hasher.finalize());
            if !computed_hash.eq_ignore_ascii_case(expected_sha256) {
                bail!(
                    "Inno Setup installer SHA-256 mismatch! Expected: {}, Computed: {}",
                    expected_sha256,
                    computed_hash
                );
            }
            info!("Inno Setup installer SHA-256 verified: {}", computed_hash);
            Ok(())
        }
        .await;

        if let Err(err) = download_res {
            let _ = remove_file_with_retry(&installer_path);
            let _ = remove_dir_all_with_retry(&temp_subdir);
            return Err(err);
        }

        let relaunch = self.relaunch_on_inno_update;
        if let Err(err) = spawn_command_with_av_retry(|| {
            let mut cmd = Command::new(&installer_path);
            cmd.args(inno_silent_args());
            if relaunch {
                cmd.arg("/RELAUNCH=1");
            }
            cmd
        })
        .context("Failed to launch Inno Setup installer")
        {
            let _ = remove_file_with_retry(&installer_path);
            let _ = remove_dir_all_with_retry(&temp_subdir);
            return Err(err);
        }

        std::process::exit(0);
    }

    /// Roll back to `<exe>.old` if available.
    pub fn rollback() -> Result<()> {
        let raw_exe = strip_windows_verbatim_prefix(
            &std::env::current_exe().context("Failed to determine current exe")?,
        );
        let current_exe = strip_old_backup_suffix(&raw_exe).unwrap_or(raw_exe);
        let primary_old = binary_sidecar_path(&current_exe, ".old");
        let legacy_old = current_exe.with_extension("old");
        let old_exe = if primary_old.exists() {
            primary_old
        } else if legacy_old.exists() {
            legacy_old
        } else {
            bail!("No previous version backup found at {:?}", primary_old);
        };

        let backup_of_failed = binary_sidecar_path(&current_exe, ".failed");
        if backup_of_failed.exists() {
            let _ = remove_file_with_retry(&backup_of_failed);
        }

        rename_with_retry(&current_exe, &backup_of_failed)?;
        if let Err(e) = rename_with_retry(&old_exe, &current_exe) {
            let _ = rename_with_retry(&backup_of_failed, &current_exe);
            bail!("Failed to restore old binary: {e}");
        }
        let _ = remove_file_with_retry(&backup_of_failed);
        info!("Successfully rolled back to previous version");
        Ok(())
    }

    /// Restart launcher process cleanly across platforms.
    pub fn restart_process(args: &[String]) -> Result<()> {
        let raw_exe = strip_windows_verbatim_prefix(
            &std::env::current_exe().context("Failed to determine current exe")?,
        );
        // After an in-place swap (`mq` -> `mq.old`, `mq.new` -> `mq`), `/proc/self/exe` on Linux
        // resolves to `mq.old`; target the restored binary path when it exists.
        let target_exe = match strip_old_backup_suffix(&raw_exe) {
            Some(restored) if restored.is_file() => restored,
            _ => raw_exe,
        };
        info!("Restarting process: {:?} {:?}", target_exe, args);

        let mut cmd = Command::new(target_exe);
        cmd.args(args);
        cmd.arg("--after-update");

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    rustix::process::setsid()
                        .map_err(|e| std::io::Error::from_raw_os_error(e.raw_os_error()))?;
                    Ok(())
                });
            }
        }

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            // DETACHED_PROCESS (0x0000_0008) | CREATE_NEW_PROCESS_GROUP (0x0000_0200):
            // detaches cleanly without inheriting or flashing a console window.
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }

        cmd.spawn().context("Failed to spawn updated process")?;
        std::process::exit(0);
    }

    /// Uninstalls user-level MQ Launcher binaries, desktop entries, and icons.
    /// When `purge_data` is true, also removes user data, config, and cache directories.
    pub fn uninstall(
        data_dir: &Path,
        config_dir: &Path,
        cache_dir: &Path,
        purge_data: bool,
        dry_run: bool,
    ) -> Result<UninstallOutcome> {
        let home_dir = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .context("Failed to determine user home directory")?;
        let current_exe = std::env::current_exe()
            .ok()
            .map(|p| strip_windows_verbatim_prefix(&p));
        execute_uninstall_with_roots(
            &home_dir,
            current_exe.as_deref(),
            data_dir,
            config_dir,
            cache_dir,
            purge_data,
            dry_run,
        )
    }
}

/// Summary of files and directories removed during uninstall.
#[derive(Debug, Clone, Default)]
pub struct UninstallOutcome {
    pub removed_files: Vec<PathBuf>,
    pub removed_dirs: Vec<PathBuf>,
    pub spawned_external_uninstaller: bool,
}

fn is_dev_or_system_exe(exe: &Path) -> bool {
    let s = exe.to_string_lossy();
    s.contains("/target/debug")
        || s.contains("/target/release")
        || s.contains("\\target\\debug")
        || s.contains("\\target\\release")
        || s.starts_with("/usr/")
        || s.starts_with("/var/")
}

/// Collects user-installed MQ Launcher binaries, shortcuts, and icon paths.
pub fn collect_user_install_artifacts(home_dir: &Path, current_exe: Option<&Path>) -> Vec<PathBuf> {
    let mut targets = vec![
        home_dir.join(".local/bin/mq"),
        home_dir.join(".local/bin/mq.old"),
        home_dir.join(".local/share/applications/mq.desktop"),
        home_dir.join(".local/share/icons/hicolor/scalable/apps/mq.svg"),
        home_dir.join("Desktop/mq.desktop"),
        home_dir.join("Рабочий стол/mq.desktop"),
        home_dir.join("Desktop/MQ.lnk"),
        home_dir.join("Desktop/MQ Launcher.lnk"),
    ];

    if let Some(exe) = current_exe {
        let clean_exe = strip_windows_verbatim_prefix(exe);
        if !is_dev_or_system_exe(&clean_exe) {
            if !targets.contains(&clean_exe) {
                targets.push(clean_exe.clone());
            }
            let sidecar_old = binary_sidecar_path(&clean_exe, ".old");
            if !targets.contains(&sidecar_old) {
                targets.push(sidecar_old);
            }
            let legacy_old = clean_exe.with_extension("old");
            if !targets.contains(&legacy_old) {
                targets.push(legacy_old);
            }
        }
    }

    targets
}

/// Executes launcher uninstallation against explicit root paths (safe to unit-test).
pub fn execute_uninstall_with_roots(
    home_dir: &Path,
    current_exe: Option<&Path>,
    data_dir: &Path,
    config_dir: &Path,
    cache_dir: &Path,
    purge_data: bool,
    dry_run: bool,
) -> Result<UninstallOutcome> {
    let mut outcome = UninstallOutcome::default();

    // Purge user data/config/cache BEFORE spawning Windows `unins000.exe`
    // (because `unins000.exe` runs `taskkill /f /im mq.exe` during `InitializeUninstall`).
    if purge_data {
        for dir in [data_dir, config_dir, cache_dir] {
            if dir.exists() && !outcome.removed_dirs.iter().any(|d| d == dir) {
                if !dry_run {
                    let _ = remove_dir_all_with_retry(dir);
                }
                outcome.removed_dirs.push(dir.to_path_buf());
            }
        }
    }

    for path in collect_user_install_artifacts(home_dir, current_exe) {
        if path.exists() {
            if !dry_run {
                // On Windows, a running .exe cannot be unlinked in-place; deferred cleanup handles it.
                let _ = remove_file_with_retry(&path);
            }
            outcome.removed_files.push(path);
        }
    }

    #[cfg(target_os = "windows")]
    if !dry_run {
        if let Some(app_data) = std::env::var_os("APPDATA") {
            let start_menu = PathBuf::from(app_data).join("Microsoft/Windows/Start Menu/Programs");
            for sc_name in ["MQ.lnk", "MQ Launcher.lnk"] {
                let sc = start_menu.join(sc_name);
                if sc.exists() {
                    let _ = remove_file_with_retry(&sc);
                    outcome.removed_files.push(sc);
                }
            }
        }

        let mut unins_candidates: Vec<PathBuf> = Vec::new();
        if let Some(exe) = current_exe {
            let clean_exe = strip_windows_verbatim_prefix(exe);
            if let Some(parent) = clean_exe.parent() {
                unins_candidates.push(parent.join("unins000.exe"));
            }
        }
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            unins_candidates.push(
                PathBuf::from(local_app_data)
                    .join("Programs")
                    .join("MQ")
                    .join("unins000.exe"),
            );
        }
        for unins_path in unins_candidates {
            if unins_path.is_file() {
                let mut cmd = Command::new(&unins_path);
                cmd.args(inno_silent_args());
                if purge_data {
                    cmd.arg("/PURGE=1");
                }
                if cmd.spawn().is_ok() {
                    outcome.spawned_external_uninstaller = true;
                }
                break;
            }
        }

        if !outcome.spawned_external_uninstaller {
            if let Some(exe) = current_exe {
                let clean_exe = strip_windows_verbatim_prefix(exe);
                if clean_exe.exists() && !is_dev_or_system_exe(&clean_exe) {
                    use std::os::windows::process::CommandExt;
                    let ps_script = "for ($i = 0; $i -lt 20; $i++) { \
                        Start-Sleep -Milliseconds 250; \
                        if (-not $env:MQ_SELF_DELETE_EXE -or -not (Test-Path -LiteralPath $env:MQ_SELF_DELETE_EXE)) { break }; \
                        Remove-Item -LiteralPath $env:MQ_SELF_DELETE_EXE -Force -ErrorAction SilentlyContinue \
                    }";
                    let encoded = encode_powershell_utf16le_base64(ps_script);
                    let _ = Command::new("powershell.exe")
                        .creation_flags(WINDOWS_CREATE_NO_WINDOW)
                        .env("MQ_SELF_DELETE_EXE", &clean_exe)
                        .args([
                            "-NoProfile",
                            "-NonInteractive",
                            "-WindowStyle",
                            "Hidden",
                            "-EncodedCommand",
                            &encoded,
                        ])
                        .spawn();
                }
            }
        }
    }

    Ok(outcome)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedSemver<'a> {
    core: Vec<u64>,
    prerelease: Option<Vec<&'a str>>,
}

fn parse_semver(raw: &str) -> Option<ParsedSemver<'_>> {
    let trimmed = raw.trim();
    let without_v = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    if without_v.is_empty() {
        return None;
    }

    // SemVer 2.0 §10: build metadata (`+...`) MUST be ignored when determining version precedence.
    let without_build = without_v.split('+').next().unwrap_or(without_v);
    let (core_str, prerelease_str) = match without_build.split_once('-') {
        Some((c, pre)) => (c, Some(pre)),
        None => (without_build, None),
    };

    if core_str.is_empty() {
        return None;
    }
    let mut core = Vec::with_capacity(3);
    for seg in core_str.split('.') {
        if seg.is_empty() || !seg.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        core.push(seg.parse::<u64>().ok()?);
    }

    let prerelease = match prerelease_str {
        Some(pre) => {
            if pre.is_empty() {
                return None;
            }
            let ids: Vec<&str> = pre.split('.').collect();
            if ids.iter().any(|id| id.is_empty()) {
                return None;
            }
            Some(ids)
        }
        None => None,
    };

    Some(ParsedSemver { core, prerelease })
}

fn compare_prerelease_identifiers(cur_ids: &[&str], cand_ids: &[&str]) -> std::cmp::Ordering {
    for (cur_id, cand_id) in cur_ids.iter().zip(cand_ids.iter()) {
        if cur_id == cand_id {
            continue;
        }
        let cur_num = if cur_id.bytes().all(|b| b.is_ascii_digit()) {
            cur_id.parse::<u128>().ok()
        } else {
            None
        };
        let cand_num = if cand_id.bytes().all(|b| b.is_ascii_digit()) {
            cand_id.parse::<u128>().ok()
        } else {
            None
        };
        let ord = match (cand_num, cur_num) {
            // SemVer 2.0 §11.4.1: Identifiers consisting of only digits are compared numerically.
            (Some(cn), Some(nn)) => cn.cmp(&nn),
            // SemVer 2.0 §11.4.3: Numeric identifiers always have lower precedence than non-numeric.
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            // SemVer 2.0 §11.4.2: Alphanumeric identifiers are compared lexically in ASCII sort order.
            (None, None) => cand_id.cmp(cur_id),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    // SemVer 2.0 §11.4.4: Larger set of pre-release fields has higher precedence if prefix matches.
    cand_ids.len().cmp(&cur_ids.len())
}

/// SemVer 2.0 comparison helper: returns `true` iff `candidate` has strictly higher precedence than `current`.
pub fn is_version_newer(current: &str, candidate: &str) -> bool {
    let Some(cand) = parse_semver(candidate) else {
        return false;
    };
    let Some(cur) = parse_semver(current) else {
        return true;
    };

    let max_core = cand.core.len().max(cur.core.len()).max(3);
    for idx in 0..max_core {
        let c = cand.core.get(idx).copied().unwrap_or(0);
        let n = cur.core.get(idx).copied().unwrap_or(0);
        if c > n {
            return true;
        }
        if c < n {
            return false;
        }
    }

    match (&cur.prerelease, &cand.prerelease) {
        (None, None) => false,
        // Pre-release has lower precedence than the associated normal version (`2.8.1-rc.1 -> 2.8.1` is true).
        (Some(_), None) => true,
        // Normal version must never downgrade to a pre-release of the same core (`2.8.1 -> 2.8.1-rc.1` is false).
        (None, Some(_)) => false,
        (Some(cur_pre), Some(cand_pre)) => {
            compare_prerelease_identifiers(cur_pre, cand_pre) == std::cmp::Ordering::Greater
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_newer_logic() {
        assert!(is_version_newer("2.0.0", "2.1.0"));
        assert!(is_version_newer("2.0.99", "2.1.0"));
        assert!(!is_version_newer("2.1.0", "2.1.0"));
        assert!(!is_version_newer("2.1.0", "2.0.0"));
        assert!(is_version_newer("1.20.1", "1.21.1"));
        assert!(!is_version_newer("2.2.0", "2.1.0"));

        // SemVer 2.0 pre-release vs stable rules
        assert!(is_version_newer("2.8.1-rc.1", "2.8.1"));
        assert!(is_version_newer("2.8.1-beta.2", "2.8.1"));
        assert!(!is_version_newer("2.8.1", "2.8.1-rc.1"));
        assert!(!is_version_newer("2.8.1", "2.8.1-beta.2"));

        // Pre-release identifier precedence
        assert!(is_version_newer("2.8.1-alpha.1", "2.8.1-beta.1"));
        assert!(is_version_newer("2.8.1-beta.2", "2.8.1-rc.1"));
        assert!(!is_version_newer("2.8.1-rc.1", "2.8.1-beta.2"));
        assert!(is_version_newer("2.8.1-rc.2", "2.8.1-rc.10"));
        assert!(!is_version_newer("2.8.1-rc.10", "2.8.1-rc.2"));
        assert!(is_version_newer("2.8.1-alpha", "2.8.1-alpha.1"));

        // Build metadata (`+build`) ignored for precedence
        assert!(!is_version_newer("2.8.1+build1", "2.8.0"));
        assert!(!is_version_newer("2.8.1+build1", "2.8.1+build2"));
        assert!(is_version_newer("2.8.0+build9", "2.8.1+build1"));
        assert!(is_version_newer("v2.8.1-rc.1+sha.abc", "v2.8.1+sha.def"));
    }

    #[test]
    fn test_validate_release_download_url_enforces_https_and_allowed_hosts() {
        assert!(validate_release_download_url(
            "https://github.com/MrQuster/MQ-Launcher/releases/download/v2.8.1/mq-linux-x86_64"
        )
        .is_ok());
        assert!(validate_release_download_url(
            "https://objects.githubusercontent.com/github-production-release-asset-2e65be/mq"
        )
        .is_ok());
        assert!(validate_release_download_url(
            "https://release-assets.githubusercontent.com/asset"
        )
        .is_ok());
        assert!(validate_release_download_url("http://127.0.0.1:8080/mock/mq").is_ok());
        assert!(validate_release_download_url("http://localhost:9000/mock/mq").is_ok());

        // Reject plain HTTP on non-loopback hosts, non-GitHub hosts, credentials, and file://
        assert!(validate_release_download_url(
            "http://github.com/MrQuster/MQ-Launcher/releases/download/v2.8.1/mq"
        )
        .is_err());
        assert!(validate_release_download_url("https://evil-github.com/mq").is_err());
        assert!(validate_release_download_url("https://mrquster.github.io/mq").is_err());
        assert!(validate_release_download_url("https://user:pass@github.com/mq").is_err());
        assert!(validate_release_download_url("file:///tmp/mq").is_err());
    }

    #[test]
    fn test_powershell_utf16le_base64_encoding() {
        // "AB" in UTF-16LE is [0x41, 0x00, 0x42, 0x00] -> Base64 "QQBCAA=="
        assert_eq!(encode_powershell_utf16le_base64("AB"), "QQBCAA==");
    }

    #[test]
    fn test_windows_and_unix_sidecar_staging_and_restart_paths() {
        let win_exe = Path::new(r"C:\Users\Иван Петров\AppData\Local\Programs\MQ\mq.exe");
        assert_eq!(
            binary_sidecar_path(win_exe, ".old"),
            PathBuf::from(r"C:\Users\Иван Петров\AppData\Local\Programs\MQ\mq.exe.old")
        );
        assert_eq!(
            staged_new_binary_path(win_exe),
            PathBuf::from(r"C:\Users\Иван Петров\AppData\Local\Programs\MQ\mq.new.exe")
        );
        assert_eq!(
            strip_old_backup_suffix(Path::new(r"\\?\C:\MQ\mq.exe.old")),
            Some(PathBuf::from(r"C:\MQ\mq.exe"))
        );

        let unix_exe = Path::new("/home/player/.local/bin/mq");
        assert_eq!(
            binary_sidecar_path(unix_exe, ".old"),
            PathBuf::from("/home/player/.local/bin/mq.old")
        );
        assert_eq!(
            staged_new_binary_path(unix_exe),
            PathBuf::from("/home/player/.local/bin/mq.new")
        );
        assert_eq!(
            strip_old_backup_suffix(Path::new("/home/player/.local/bin/mq.old")),
            Some(PathBuf::from("/home/player/.local/bin/mq"))
        );
        assert_eq!(strip_old_backup_suffix(unix_exe), None);
    }

    #[test]
    fn test_extract_sha256_handles_crlf_bom_binary_asterisk_and_priority() {
        let h_win = "1111111111111111111111111111111111111111111111111111111111111111";
        let h_fallback = "2222222222222222222222222222222222222222222222222222222222222222";
        let h_setup = "ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789";

        let sums = format!(
            "\u{FEFF}# generated on Windows\r\n\
             not-a-valid-hash  mq-windows-x86_64.exe\r\n\
             {h_fallback}  mq.exe\r\n\
             {h_win} *./mq-windows-x86_64.exe\r\n\
             {h_setup}  .\\MQ-Setup-2.7.3-x64.exe\r\n"
        );

        // Priority order must choose mq-windows-x86_64.exe over earlier mq.exe fallback.
        assert_eq!(
            extract_sha256_for_targets(&sums, &["mq-windows-x86_64.exe", "mq.exe"]).as_deref(),
            Some(h_win)
        );
        assert_eq!(
            extract_sha256_for_targets(&sums, &["MQ-Setup-2.7.3-x64.exe"]).as_deref(),
            Some(h_setup.to_ascii_lowercase().as_str())
        );
    }

    #[test]
    fn test_swap_binary_files_succeeds_and_rolls_back_on_error() {
        let tmp = tempfile::tempdir().unwrap();
        let current = tmp.path().join("mq.exe");
        let new_bin = tmp.path().join("mq.new.exe");
        let old_bin = binary_sidecar_path(&current, ".old");

        fs::write(&current, b"v1-old-binary").unwrap();
        fs::write(&new_bin, b"v2-new-binary").unwrap();

        swap_binary_files(&current, &new_bin, &old_bin).expect("binary swap should succeed");
        assert_eq!(fs::read(&current).unwrap(), b"v2-new-binary");
        assert_eq!(fs::read(&old_bin).unwrap(), b"v1-old-binary");
        assert!(!new_bin.exists());

        // Rollback when `new_exe` is missing must restore original `current`.
        let missing_new = tmp.path().join("does-not-exist.new.exe");
        let err = swap_binary_files(&current, &missing_new, &old_bin).unwrap_err();
        assert!(err.to_string().contains("Restored old binary"));
        assert_eq!(fs::read(&current).unwrap(), b"v2-new-binary");
    }

    #[test]
    fn test_minisign_signature_verification_success_and_tampering() {
        let keypair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pk_box = keypair.pk.to_box().unwrap().to_string();
        let pk_str = pk_box.lines().nth(1).unwrap().trim();

        let data = b"4a4f89d34e62  mq-linux-x86_64\n3b2e7c1092a4  mq-windows-x86_64.exe\n";
        let sig_box = minisign::sign(None, &keypair.sk, data.as_slice(), None, None)
            .unwrap()
            .to_string();

        // 1. Success validation
        assert!(MinisignVerifier::verify(data, &sig_box, pk_str).is_ok());

        // 2. Tampered content rejected
        let tampered_data = b"tampered content";
        assert!(MinisignVerifier::verify(tampered_data, &sig_box, pk_str).is_err());

        // 3. Wrong public key rejected
        let other_keypair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let other_pk_box = other_keypair.pk.to_box().unwrap().to_string();
        let other_pk_str = other_pk_box.lines().nth(1).unwrap().trim();
        assert!(MinisignVerifier::verify(data, &sig_box, other_pk_str).is_err());
    }

    #[test]
    fn test_official_embedded_public_key_format() {
        let pk = minisign_verify::PublicKey::from_base64(MQ_RELEASE_PUBLIC_KEY);
        assert!(
            pk.is_ok(),
            "Embedded public key must be valid base64 Minisign public key"
        );
    }

    #[test]
    fn test_select_windows_update_strategy_inno_vs_portable() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            select_windows_update_strategy(tmp.path()),
            WindowsUpdateStrategy::PortableAtomicSwap
        );

        fs::write(tmp.path().join("unins000.exe"), b"").unwrap();
        assert_eq!(
            select_windows_update_strategy(tmp.path()),
            WindowsUpdateStrategy::InnoSetupInstaller
        );

        // portable.flag overrides unins000.exe so portable installs stay portable
        fs::write(tmp.path().join("portable.flag"), b"").unwrap();
        assert_eq!(
            select_windows_update_strategy(tmp.path()),
            WindowsUpdateStrategy::PortableAtomicSwap
        );
    }

    #[test]
    fn test_inno_installer_name_and_silent_args() {
        assert_eq!(
            expected_inno_installer_name("2.3.0"),
            "MQ-Setup-2.3.0-x64.exe"
        );
        assert_eq!(
            expected_inno_installer_name("v2.3.0"),
            "MQ-Setup-2.3.0-x64.exe"
        );
        assert_eq!(
            inno_silent_args(),
            ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]
        );
    }

    #[test]
    fn test_verify_checksums_and_extract_sha256_for_inno_installer() {
        let keypair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pk_box = keypair.pk.to_box().unwrap().to_string();
        let pk_str = pk_box.lines().nth(1).unwrap().trim();

        let installer = expected_inno_installer_name("2.3.0");
        let data = format!(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  mq-windows-x86_64.exe\n\
             bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  {installer}\n"
        );
        let sig_box = minisign::sign(None, &keypair.sk, data.as_bytes(), None, None)
            .unwrap()
            .to_string();

        let hash = verify_checksums_and_extract_sha256(
            data.as_bytes(),
            &sig_box,
            &[installer.as_str()],
            pk_str,
        )
        .expect("valid signature and present installer hash");
        assert_eq!(
            hash,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );

        // Tampered checksums rejected
        assert!(verify_checksums_and_extract_sha256(
            b"tampered",
            &sig_box,
            &[installer.as_str()],
            pk_str,
        )
        .is_err());

        // Missing target rejected
        assert!(verify_checksums_and_extract_sha256(
            data.as_bytes(),
            &sig_box,
            &["MQ-Setup-9.9.9-x64.exe"],
            pk_str,
        )
        .is_err());
    }

    #[test]
    fn test_execute_uninstall_with_roots_preserves_or_purges_data() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let bin_dir = home.join(".local/bin");
        let app_dir = home.join(".local/share/applications");
        let icon_dir = home.join(".local/share/icons/hicolor/scalable/apps");
        let data_dir = home.join(".local/share/mq");
        let config_dir = home.join(".config/mq");
        let cache_dir = home.join(".cache/mq");

        for dir in [
            &bin_dir,
            &app_dir,
            &icon_dir,
            &data_dir,
            &config_dir,
            &cache_dir,
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        let bin_path = bin_dir.join("mq");
        let desktop_path = app_dir.join("mq.desktop");
        let icon_path = icon_dir.join("mq.svg");
        let instance_marker = data_dir.join("instances/inst.json");
        fs::create_dir_all(instance_marker.parent().unwrap()).unwrap();
        fs::write(&bin_path, b"bin").unwrap();
        fs::write(&desktop_path, b"[Desktop Entry]").unwrap();
        fs::write(&icon_path, b"<svg/>").unwrap();
        fs::write(&instance_marker, b"{}").unwrap();

        // Dry run keeps everything intact
        let dry = execute_uninstall_with_roots(
            &home,
            Some(&bin_path),
            &data_dir,
            &config_dir,
            &cache_dir,
            false,
            true,
        )
        .unwrap();
        assert_eq!(dry.removed_files.len(), 3);
        assert!(bin_path.exists());
        assert!(instance_marker.exists());

        // Uninstall without purge removes launcher files but preserves user data
        let out = execute_uninstall_with_roots(
            &home,
            Some(&bin_path),
            &data_dir,
            &config_dir,
            &cache_dir,
            false,
            false,
        )
        .unwrap();
        assert_eq!(out.removed_files.len(), 3);
        assert!(out.removed_dirs.is_empty());
        assert!(!bin_path.exists());
        assert!(!desktop_path.exists());
        assert!(!icon_path.exists());
        assert!(instance_marker.exists());

        // Purge removes data/config/cache directories too
        let purged = execute_uninstall_with_roots(
            &home,
            None,
            &data_dir,
            &config_dir,
            &cache_dir,
            true,
            false,
        )
        .unwrap();
        assert_eq!(purged.removed_dirs.len(), 3);
        assert!(!data_dir.exists());
        assert!(!config_dir.exists());
        assert!(!cache_dir.exists());
    }
}
