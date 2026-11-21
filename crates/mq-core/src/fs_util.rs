//! Windows-safe and cross-platform filesystem utilities with exponential backoff
//! retry for transient file locks (Defender, Search Indexer, antivirus scanners),
//! rollback-safe existing-file replacement, and cross-device atomic rename fallback.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use crate::platform::is_windows_reserved_name;

/// Exponential backoff delays (in milliseconds) for transient Windows file lock retries.
pub const RETRY_DELAYS_MS: [u64; 6] = [15, 30, 60, 120, 250, 500];

static FS_TMP_SEQ: AtomicU64 = AtomicU64::new(1);

/// Returns `true` if `err` represents a transient Windows file lock or access error:
/// - `ErrorKind::PermissionDenied`
/// - `ERROR_ACCESS_DENIED` (`5`)
/// - `ERROR_SHARING_VIOLATION` (`32`)
/// - `ERROR_LOCK_VIOLATION` (`33`)
#[inline]
pub fn is_transient_windows_lock_error(err: &io::Error) -> bool {
    if err.kind() == ErrorKind::PermissionDenied {
        return true;
    }
    matches!(err.raw_os_error(), Some(5 | 32 | 33))
}

/// Returns `true` if `err` indicates that the rename destination already exists:
/// - `ErrorKind::AlreadyExists`
/// - `ERROR_FILE_EXISTS` (`80` on Windows)
/// - `ERROR_ALREADY_EXISTS` (`183` on Windows)
#[inline]
fn is_already_exists_error(err: &io::Error) -> bool {
    err.kind() == ErrorKind::AlreadyExists || matches!(err.raw_os_error(), Some(80 | 183))
}

/// Returns `true` if `err` indicates a cross-device / cross-mount rename failure:
/// - `ERROR_NOT_SAME_DEVICE` (`17` on Windows)
/// - `EXDEV` (`18` on Linux / Unix)
#[inline]
pub fn is_cross_device_error(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(17 | 18))
}

/// Constructs a unique sibling temporary path in `target`'s parent directory so staging
/// and backup swaps remain on the same filesystem volume as `target`.
fn unique_sibling_path(target: &Path, tag: &str) -> PathBuf {
    let seq = FS_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let base = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    target.with_file_name(format!(".{base}.{tag}.{pid}.{seq}.tmp"))
}

/// Safely swaps `from` into `to` when `to` already exists as a regular file by first
/// moving `to` to a sibling backup and immediately restoring that backup if `rename_fn` fails.
/// Guarantees existing destination data is never lost on failure.
fn swap_existing_file_with_rollback<F>(from: &Path, to: &Path, rename_fn: &mut F) -> io::Result<()>
where
    F: FnMut(&Path, &Path) -> io::Result<()>,
{
    clear_readonly_attribute(to);
    let backup = unique_sibling_path(to, "mq_bak");
    fs::rename(to, &backup)?;
    match rename_fn(from, to) {
        Ok(()) => {
            let _ = remove_file_with_retry(&backup);
            Ok(())
        }
        Err(err) => {
            // Restore original destination immediately so data is never lost.
            let _ = fs::rename(&backup, to);
            Err(err)
        }
    }
}

/// Async counterpart of [`swap_existing_file_with_rollback`] using `tokio::fs::rename`.
async fn async_swap_existing_file_with_rollback(from: &Path, to: &Path) -> io::Result<()> {
    clear_readonly_attribute(to);
    let backup = unique_sibling_path(to, "mq_bak");
    tokio::fs::rename(to, &backup).await?;
    match tokio::fs::rename(from, to).await {
        Ok(()) => {
            let _ = async_remove_file_with_retry(&backup).await;
            Ok(())
        }
        Err(err) => {
            let _ = tokio::fs::rename(&backup, to).await;
            Err(err)
        }
    }
}

/// Internal helper that executes a synchronous I/O operation with up to 6 retry attempts
/// using [`RETRY_DELAYS_MS`] when [`is_transient_windows_lock_error`] is true.
pub fn retry_with_backoff<T, F, R>(delays_ms: &[u64], mut op: F, mut on_retry: R) -> io::Result<T>
where
    F: FnMut(usize) -> io::Result<T>,
    R: FnMut(usize, &io::Error),
{
    let mut attempt = 0;
    loop {
        match op(attempt) {
            Ok(val) => return Ok(val),
            Err(err) => {
                if is_transient_windows_lock_error(&err) && attempt < delays_ms.len() {
                    on_retry(attempt, &err);
                    let delay = delays_ms[attempt];
                    if delay > 0 {
                        thread::sleep(Duration::from_millis(delay));
                    }
                    attempt += 1;
                    continue;
                }
                return Err(err);
            }
        }
    }
}

/// Renames `from` to `to` with automatic retries on transient Windows file locks (`5`, `32`, `33`,
/// `PermissionDenied`), rollback-safe replacement when `to` already exists, and atomic staged
/// `copy + rename + remove` fallback if `from` and `to` reside on different filesystems/drives
/// (`EXDEV` / `ERROR_NOT_SAME_DEVICE`).
pub fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    rename_with_custom_op(from, to, &RETRY_DELAYS_MS, |src, dst| fs::rename(src, dst))
}

/// Testable core implementation of [`rename_with_retry`] accepting a custom rename closure and delay schedule.
pub fn rename_with_custom_op<F>(
    from: &Path,
    to: &Path,
    delays_ms: &[u64],
    mut rename_fn: F,
) -> io::Result<()>
where
    F: FnMut(&Path, &Path) -> io::Result<()>,
{
    if let Some(parent) = to.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            let _ = fs::create_dir_all(parent);
        }
    }

    let mut attempt = 0;
    loop {
        match rename_fn(from, to) {
            Ok(()) => return Ok(()),
            Err(err) if is_cross_device_error(&err) => {
                tracing::debug!(
                    from = %from.display(),
                    to = %to.display(),
                    "Cross-device rename detected ({err}); falling back to staged copy + rename + remove"
                );
                // Stage in `to`'s parent directory so a failed copy never leaves a partial file at `to`.
                let staging = unique_sibling_path(to, "xdev");
                if let Err(copy_err) = fs::copy(from, &staging) {
                    let _ = remove_file_with_retry(&staging);
                    return Err(copy_err);
                }
                if let Ok(file) = fs::File::open(&staging) {
                    let _ = file.sync_all();
                }
                if let Err(swap_err) = rename_same_dir_with_delays(&staging, to, delays_ms) {
                    let _ = remove_file_with_retry(&staging);
                    return Err(swap_err);
                }
                remove_file_with_retry(from)?;
                return Ok(());
            }
            Err(err) if is_already_exists_error(&err) && from.is_file() && to.is_file() => {
                match swap_existing_file_with_rollback(from, to, &mut rename_fn) {
                    Ok(()) => return Ok(()),
                    Err(swap_err)
                        if is_transient_windows_lock_error(&swap_err)
                            && attempt < delays_ms.len() =>
                    {
                        let delay = delays_ms[attempt];
                        if delay > 0 {
                            thread::sleep(Duration::from_millis(delay));
                        }
                        attempt += 1;
                    }
                    Err(swap_err) => return Err(swap_err),
                }
            }
            Err(err) if is_transient_windows_lock_error(&err) => {
                // Never delete `to` before replacement succeeds: only clear read-only attribute.
                if to.exists() {
                    clear_readonly_attribute(to);
                }
                if attempt < delays_ms.len() {
                    let delay = delays_ms[attempt];
                    if delay > 0 {
                        thread::sleep(Duration::from_millis(delay));
                    }
                    attempt += 1;
                } else {
                    #[cfg(target_os = "windows")]
                    {
                        if from.is_file() && to.is_file() {
                            return swap_existing_file_with_rollback(from, to, &mut rename_fn);
                        }
                    }
                    return Err(err);
                }
            }
            Err(err) => return Err(err),
        }
    }
}

fn rename_same_dir_with_delays(from: &Path, to: &Path, delays_ms: &[u64]) -> io::Result<()> {
    let mut attempt = 0;
    let mut direct = |src: &Path, dst: &Path| fs::rename(src, dst);
    loop {
        match direct(from, to) {
            Ok(()) => return Ok(()),
            Err(err) if is_already_exists_error(&err) && from.is_file() && to.is_file() => {
                return swap_existing_file_with_rollback(from, to, &mut direct);
            }
            Err(err) if is_transient_windows_lock_error(&err) => {
                if to.exists() {
                    clear_readonly_attribute(to);
                }
                if attempt < delays_ms.len() {
                    let delay = delays_ms[attempt];
                    if delay > 0 {
                        thread::sleep(Duration::from_millis(delay));
                    }
                    attempt += 1;
                } else {
                    #[cfg(target_os = "windows")]
                    {
                        if from.is_file() && to.is_file() {
                            return swap_existing_file_with_rollback(from, to, &mut direct);
                        }
                    }
                    return Err(err);
                }
            }
            Err(err) => return Err(err),
        }
    }
}

/// Removes a file at `path`, retrying up to 6 times on transient Windows locks.
/// Returns `Ok(())` if the file does not exist (`ErrorKind::NotFound`).
pub fn remove_file_with_retry(path: &Path) -> io::Result<()> {
    remove_file_with_custom_op(path, &RETRY_DELAYS_MS, |p| fs::remove_file(p))
}

/// Testable core implementation of [`remove_file_with_retry`].
pub fn remove_file_with_custom_op<F>(
    path: &Path,
    delays_ms: &[u64],
    mut remove_fn: F,
) -> io::Result<()>
where
    F: FnMut(&Path) -> io::Result<()>,
{
    let mut attempt = 0;
    loop {
        match remove_fn(path) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err) if is_transient_windows_lock_error(&err) && attempt < delays_ms.len() => {
                clear_readonly_attribute(path);
                let delay = delays_ms[attempt];
                if delay > 0 {
                    thread::sleep(Duration::from_millis(delay));
                }
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Recursively removes a directory tree at `path`, retrying up to 6 times on transient Windows locks
/// and clearing read-only file attributes inside the tree if needed.
/// Returns `Ok(())` if the directory does not exist (`ErrorKind::NotFound`).
pub fn remove_dir_all_with_retry(path: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err)
                if is_transient_windows_lock_error(&err) && attempt < RETRY_DELAYS_MS.len() =>
            {
                clear_readonly_recursive(path);
                let delay = RETRY_DELAYS_MS[attempt];
                if delay > 0 {
                    thread::sleep(Duration::from_millis(delay));
                }
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Asynchronously renames `from` to `to` using `tokio::fs` with exponential backoff on
/// transient Windows locks, rollback-safe replacement when `to` already exists, and
/// staged `copy + rename + remove` fallback on cross-device errors.
pub async fn async_rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
    }

    let mut attempt = 0;
    loop {
        match tokio::fs::rename(from, to).await {
            Ok(()) => return Ok(()),
            Err(err) if is_cross_device_error(&err) => {
                tracing::debug!(
                    from = %from.display(),
                    to = %to.display(),
                    "Async cross-device rename detected ({err}); falling back to staged copy + rename + remove"
                );
                let staging = unique_sibling_path(to, "xdev");
                if let Err(copy_err) = tokio::fs::copy(from, &staging).await {
                    let _ = async_remove_file_with_retry(&staging).await;
                    return Err(copy_err);
                }
                if let Err(swap_err) = async_rename_same_dir(&staging, to).await {
                    let _ = async_remove_file_with_retry(&staging).await;
                    return Err(swap_err);
                }
                async_remove_file_with_retry(from).await?;
                return Ok(());
            }
            Err(err) if is_already_exists_error(&err) && from.is_file() && to.is_file() => {
                match async_swap_existing_file_with_rollback(from, to).await {
                    Ok(()) => return Ok(()),
                    Err(swap_err)
                        if is_transient_windows_lock_error(&swap_err)
                            && attempt < RETRY_DELAYS_MS.len() =>
                    {
                        let delay = RETRY_DELAYS_MS[attempt];
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        attempt += 1;
                    }
                    Err(swap_err) => return Err(swap_err),
                }
            }
            Err(err) if is_transient_windows_lock_error(&err) => {
                // Never delete `to` before replacement succeeds: only clear read-only attribute.
                if to.exists() {
                    clear_readonly_attribute(to);
                }
                if attempt < RETRY_DELAYS_MS.len() {
                    let delay = RETRY_DELAYS_MS[attempt];
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    attempt += 1;
                } else {
                    #[cfg(target_os = "windows")]
                    {
                        if from.is_file() && to.is_file() {
                            return async_swap_existing_file_with_rollback(from, to).await;
                        }
                    }
                    return Err(err);
                }
            }
            Err(err) => return Err(err),
        }
    }
}

async fn async_rename_same_dir(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match tokio::fs::rename(from, to).await {
            Ok(()) => return Ok(()),
            Err(err) if is_already_exists_error(&err) && from.is_file() && to.is_file() => {
                return async_swap_existing_file_with_rollback(from, to).await;
            }
            Err(err) if is_transient_windows_lock_error(&err) => {
                if to.exists() {
                    clear_readonly_attribute(to);
                }
                if attempt < RETRY_DELAYS_MS.len() {
                    let delay = RETRY_DELAYS_MS[attempt];
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    attempt += 1;
                } else {
                    #[cfg(target_os = "windows")]
                    {
                        if from.is_file() && to.is_file() {
                            return async_swap_existing_file_with_rollback(from, to).await;
                        }
                    }
                    return Err(err);
                }
            }
            Err(err) => return Err(err),
        }
    }
}

/// Asynchronously removes a file at `path` using `tokio::fs` with exponential backoff on
/// transient Windows locks. Returns `Ok(())` if the file is already missing (`ErrorKind::NotFound`).
pub async fn async_remove_file_with_retry(path: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match tokio::fs::remove_file(path).await {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err)
                if is_transient_windows_lock_error(&err) && attempt < RETRY_DELAYS_MS.len() =>
            {
                clear_readonly_attribute(path);
                let delay = RETRY_DELAYS_MS[attempt];
                tokio::time::sleep(Duration::from_millis(delay)).await;
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Clears the read-only attribute on `path` if set (frequently needed on Windows before unlinking).
pub(crate) fn clear_readonly_attribute(path: &Path) {
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = fs::set_permissions(path, perms);
        }
    }
}

fn clear_readonly_recursive(path: &Path) {
    clear_readonly_attribute(path);
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            clear_readonly_recursive(&entry_path);
        } else {
            clear_readonly_attribute(&entry_path);
        }
    }
}

/// Validates whether a single filename component is safe on Windows (and Unix):
/// - Non-empty, not `.` or `..`
/// - Does not end with a space `' '` or a period `'.'`
/// - Contains no forbidden characters (`< > : " / \ | ? *` or ASCII control characters `0x00..=0x1F`)
/// - Stem (before first `.`) is not a Windows reserved device name (`CON`, `PRN`, `AUX`, `NUL`,
///   `COM1..9`, `LPT1..9`, case-insensitively).
pub fn validate_windows_safe_filename(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return false;
    }

    if name.ends_with(' ') || name.ends_with('.') {
        return false;
    }

    if name.chars().any(|ch| {
        matches!(
            ch,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\0'..='\x1f'
        )
    }) {
        return false;
    }

    let stem = match name.split_once('.') {
        Some((s, _)) => s,
        None => name,
    };

    if stem.is_empty() && name.starts_with('.') {
        // Dotfiles like `.gitignore` are valid when they don't violate the rules above
        return true;
    }

    !is_windows_reserved_name(stem)
}

/// Strips the Windows extended-length verbatim prefix (`\\?\` or `\\?\UNC\`) from `path`
/// so external tools (such as `java.exe` or legacy Win32 APIs) receive standard paths.
pub fn strip_windows_verbatim_prefix(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(unc_rest) = raw.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc_rest}"))
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

/// Canonicalizes `path` and strips any Windows verbatim `\\?\` prefix so the resulting path
/// is safe to store in configuration, display in UI, or pass to `java.exe`.
pub fn canonicalize_clean(path: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(path).map(|p| strip_windows_verbatim_prefix(&p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    #[test]
    fn detects_transient_windows_lock_errors_5_32_33_and_permission_denied() {
        for code in [5, 32, 33] {
            let err = io::Error::from_raw_os_error(code);
            assert!(
                is_transient_windows_lock_error(&err),
                "expected OS error {code} to be recognized as transient lock error"
            );
        }

        let perm = io::Error::new(ErrorKind::PermissionDenied, "permission denied");
        assert!(is_transient_windows_lock_error(&perm));

        let not_found = io::Error::new(ErrorKind::NotFound, "missing");
        assert!(!is_transient_windows_lock_error(&not_found));
    }

    #[test]
    fn detects_cross_device_errors_17_and_18() {
        assert!(is_cross_device_error(&io::Error::from_raw_os_error(17)));
        assert!(is_cross_device_error(&io::Error::from_raw_os_error(18)));
        assert!(!is_cross_device_error(&io::Error::from_raw_os_error(2)));
    }

    #[test]
    fn rename_with_retry_recovers_from_simulated_sharing_violations() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("download.part");
        let dst = dir.path().join("final.jar");
        fs::write(&src, b"jar-payload-bytes").expect("write src");

        let attempts = AtomicUsize::new(0);
        // Simulate ERROR_SHARING_VIOLATION (32) on first 3 attempts, then succeed on 4th
        let res = rename_with_custom_op(&src, &dst, &[1, 1, 1, 1, 1, 1], |s, d| {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            match n {
                0 => Err(io::Error::from_raw_os_error(32)),
                1 => Err(io::Error::from_raw_os_error(33)),
                2 => Err(io::Error::from_raw_os_error(5)),
                _ => fs::rename(s, d),
            }
        });

        assert!(res.is_ok(), "rename should succeed after transient retries");
        assert_eq!(attempts.load(Ordering::SeqCst), 4);
        assert!(!src.exists());
        assert_eq!(fs::read(&dst).expect("read dst"), b"jar-payload-bytes");
    }

    #[test]
    fn rename_with_retry_replaces_existing_file_on_already_exists_and_preserves_on_failure() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("new.tmp");
        let dst = dir.path().join("config.toml");
        fs::write(&src, b"new-config").expect("write src");
        fs::write(&dst, b"original-config").expect("write dst");

        // 1. Simulated Windows AlreadyExists when destination exists triggers safe backup-swap.
        let calls = AtomicUsize::new(0);
        let res = rename_with_custom_op(&src, &dst, &[1, 1], |s, d| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 && d.exists() {
                Err(io::Error::new(
                    ErrorKind::AlreadyExists,
                    "destination exists",
                ))
            } else {
                fs::rename(s, d)
            }
        });
        assert!(res.is_ok(), "should replace existing file via safe swap");
        assert!(!src.exists());
        assert_eq!(fs::read(&dst).expect("read replaced dst"), b"new-config");

        // 2. If replacement fails even during backup-swap, original destination MUST be restored!
        let src2 = dir.path().join("broken.tmp");
        fs::write(&src2, b"incoming-data").expect("write src2");
        let err = rename_with_custom_op(&src2, &dst, &[1], |_s, d| {
            if d.exists() {
                Err(io::Error::new(
                    ErrorKind::AlreadyExists,
                    "destination exists",
                ))
            } else {
                Err(io::Error::new(ErrorKind::PermissionDenied, "source locked"))
            }
        });
        assert!(err.is_err());
        assert_eq!(
            fs::read(&dst).expect("original dst must remain intact after failed swap"),
            b"new-config"
        );
    }

    #[test]
    fn rename_with_retry_falls_back_to_copy_and_remove_on_cross_device_error() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("temp_drive_c.part");
        let dst = dir.path().join("sub/drive_d.jar");
        fs::write(&src, b"cross-drive-content-12345").expect("write src");

        // Simulate Windows ERROR_NOT_SAME_DEVICE (17)
        let res = rename_with_custom_op(&src, &dst, &[1, 1], |_s, _d| {
            Err(io::Error::from_raw_os_error(17))
        });

        assert!(res.is_ok(), "cross-device fallback should succeed");
        assert!(!src.exists(), "source file must be removed after copy");
        assert_eq!(
            fs::read(&dst).expect("read dst"),
            b"cross-drive-content-12345"
        );
    }

    #[test]
    fn remove_file_with_retry_handles_not_found_and_transient_locks() {
        let dir = tempdir().expect("tempdir");
        let missing = dir.path().join("does_not_exist.tmp");
        assert!(remove_file_with_retry(&missing).is_ok());

        let file = dir.path().join("locked.tmp");
        fs::write(&file, b"temp").expect("write temp");

        let attempts = AtomicUsize::new(0);
        let res = remove_file_with_custom_op(&file, &[1, 1, 1], |p| {
            if attempts.fetch_add(1, Ordering::SeqCst) < 2 {
                Err(io::Error::from_raw_os_error(32))
            } else {
                fs::remove_file(p)
            }
        });

        assert!(res.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn async_rename_and_remove_work_cleanly() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("async.part");
        let dst = dir.path().join("nested/async.jar");
        tokio::fs::write(&src, b"async-bytes")
            .await
            .expect("write src");

        async_rename_with_retry(&src, &dst)
            .await
            .expect("async rename");
        assert!(!src.exists());
        assert_eq!(
            tokio::fs::read(&dst).await.expect("read dst"),
            b"async-bytes"
        );

        // Overwrite existing file cleanly via async_rename_with_retry
        let src2 = dir.path().join("async2.part");
        tokio::fs::write(&src2, b"async-bytes-v2")
            .await
            .expect("write src2");
        async_rename_with_retry(&src2, &dst)
            .await
            .expect("async overwrite");
        assert_eq!(
            tokio::fs::read(&dst).await.expect("read overwritten dst"),
            b"async-bytes-v2"
        );

        async_remove_file_with_retry(&dst)
            .await
            .expect("async remove");
        assert!(!dst.exists());
        // Removing again should be Ok(()) on NotFound
        async_remove_file_with_retry(&dst)
            .await
            .expect("async remove idempotent");
    }

    #[test]
    fn validate_windows_safe_filename_rejects_reserved_and_illegal_names() {
        for bad in [
            "",
            ".",
            "..",
            "CON",
            "con",
            "PRN.txt",
            "aux.jar",
            "NUL",
            "COM1",
            "com9.log",
            "LPT1",
            "lpt9.zip",
            "bad:name.jar",
            "bad<name>.jar",
            "bad|name.jar",
            "bad?name.jar",
            "bad*name.jar",
            "slash/name.jar",
            "back\\name.jar",
            "trailing_space ",
            "trailing_dot.",
            "null\0byte.jar",
        ] {
            assert!(
                !validate_windows_safe_filename(bad),
                "expected {bad:?} to be rejected as unsafe Windows filename"
            );
        }

        for good in [
            "client.jar",
            "26.3-vanilla",
            "1.20.1-fabric",
            "COM10.txt",
            "con_mod.jar",
            "Сборка Выживание 1.21.1.mrpack",
            ".gitignore",
        ] {
            assert!(
                validate_windows_safe_filename(good),
                "expected {good:?} to be accepted as valid Windows filename"
            );
        }
    }

    #[test]
    fn strip_windows_verbatim_prefix_normalizes_paths() {
        assert_eq!(
            strip_windows_verbatim_prefix(Path::new(r"\\?\C:\Users\Player\AppData\Local\MQ")),
            PathBuf::from(r"C:\Users\Player\AppData\Local\MQ")
        );
        assert_eq!(
            strip_windows_verbatim_prefix(Path::new(r"\\?\UNC\server\share\mq")),
            PathBuf::from(r"\\server\share\mq")
        );
        assert_eq!(
            strip_windows_verbatim_prefix(Path::new("/home/player/.local/share/mq")),
            PathBuf::from("/home/player/.local/share/mq")
        );

        let dir = tempdir().expect("tempdir");
        let clean = canonicalize_clean(dir.path()).expect("canonicalize_clean");
        assert!(!clean.to_string_lossy().starts_with(r"\\?\"));
    }
}
