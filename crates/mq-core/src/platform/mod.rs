//! Platform-specific abstractions for OS detection, paths, filesystem operations,
//! wrapper availability checks, and process interactions across Linux, Windows, and macOS.

pub mod sanitize;

pub use sanitize::{is_windows_reserved_name, sanitize_filename};

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(any(target_os = "windows", target_os = "macos"))]
use directories::BaseDirs;

#[cfg(any(target_os = "windows", test))]
use crate::fs_util::strip_windows_verbatim_prefix;
use crate::fs_util::{remove_file_with_retry, rename_with_retry};

/// Win32 process creation flag (`CREATE_NO_WINDOW`) suppressing transient console windows
/// when spawning helper processes from GUI applications on Windows.
pub const WINDOWS_CREATE_NO_WINDOW: u32 = 0x0800_0000;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(1);

#[cfg(target_os = "windows")]
mod win32_platform {
    use std::ffi::c_void;
    use std::io;
    use std::iter::once;

    const SW_SHOWNORMAL: i32 = 1;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: *mut c_void,
            lp_operation: *const u16,
            lp_file: *const u16,
            lp_parameters: *const u16,
            lp_directory: *const u16,
            n_show_cmd: i32,
        ) -> isize;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(
            dw_desired_access: u32,
            b_inherit_handle: i32,
            dw_process_id: u32,
        ) -> *mut c_void;
        fn GetExitCodeProcess(h_process: *mut c_void, lp_exit_code: *mut u32) -> i32;
        fn CloseHandle(h_object: *mut c_void) -> i32;
    }

    fn to_wide_null(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(once(0)).collect()
    }

    /// Opens a URL or shell target via Win32 `ShellExecuteW` without invoking `cmd.exe`,
    /// avoiding `&` command-separator truncation, shell injection, and flashing console windows.
    pub(super) fn open_shell_target(target: &str) -> bool {
        let verb = to_wide_null("open");
        let file = to_wide_null(target);
        // SAFETY: Both `verb` and `file` are NUL-terminated UTF-16 buffers that outlive the call.
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        result > 32
    }

    /// Checks whether a Win32 process with `pid` is currently alive without spawning `tasklist.exe`.
    pub(super) fn is_process_alive(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        // SAFETY: `OpenProcess` is safe to invoke for any PID; valid handles are closed below.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            // ERROR_ACCESS_DENIED (5) indicates the process exists with higher privilege.
            return io::Error::last_os_error().raw_os_error() == Some(5);
        }
        let mut exit_code: u32 = 0;
        // SAFETY: `handle` is non-null and `&mut exit_code` points to valid writable `u32`.
        let ok = unsafe { GetExitCodeProcess(handle, &mut exit_code) };
        // SAFETY: `handle` was obtained from `OpenProcess` and is closed exactly once.
        unsafe {
            let _ = CloseHandle(handle);
        }
        ok != 0 && exit_code == STILL_ACTIVE
    }
}

/// Returns the normalized lowercase OS identifier: `"windows"`, `"macos"`, or `"linux"`.
#[inline]
pub fn os_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "windows"
    }
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        "linux"
    }
}

/// Returns the JVM classpath separator character: `';'` on Windows, `':'` on Unix/macOS.
#[inline]
pub fn classpath_separator() -> char {
    #[cfg(target_os = "windows")]
    {
        ';'
    }
    #[cfg(not(target_os = "windows"))]
    {
        ':'
    }
}

/// Formats a Windows absolute path with the extended-length prefix (`\\?\` or `\\?\UNC\`)
/// when its UTF-16 length is at least 240 code units.
#[cfg(any(target_os = "windows", test))]
fn format_windows_long_path(raw: &str) -> String {
    if raw.starts_with(r"\\?\") {
        return raw.to_owned();
    }
    let clean = raw.replace('/', "\\");
    if clean.encode_utf16().count() < 240 {
        return raw.to_owned();
    }
    if let Some(unc_rest) = clean.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{unc_rest}")
    } else if clean.len() >= 3
        && clean.as_bytes()[0].is_ascii_alphabetic()
        && clean.as_bytes()[1] == b':'
        && clean.as_bytes()[2] == b'\\'
    {
        format!(r"\\?\{clean}")
    } else {
        raw.to_owned()
    }
}

/// Normalizes a path for Windows `explorer.exe` by stripping any `\\?\` verbatim prefix
/// and converting forward slashes `/` into backslashes `\`.
#[cfg(any(target_os = "windows", test))]
fn normalize_windows_explorer_path(path: &Path) -> PathBuf {
    let stripped = strip_windows_verbatim_prefix(path);
    PathBuf::from(stripped.to_string_lossy().replace('/', "\\"))
}

/// Ensures Windows extended-length path prefix (`\\?\` or `\\?\UNC\`) is attached to
/// absolute paths longer than 240 UTF-16 code units.
/// On non-Windows platforms, this is a no-op that returns the path unmodified.
pub fn ensure_long_path_prefix(path: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let path_str = path.to_string_lossy();
        if path.is_absolute() {
            PathBuf::from(format_windows_long_path(&path_str))
        } else {
            path.to_path_buf()
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        path.to_path_buf()
    }
}

/// Resolves the default persistent data directory for the current platform:
/// - Windows: `%LOCALAPPDATA%\MQ` (e.g. `C:\Users\<User>\AppData\Local\MQ`)
/// - Linux: `$XDG_DATA_HOME/mq` or `~/.local/share/mq`
/// - macOS: `~/Library/Application Support/MQ`
pub fn default_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let raw = if let Some(base) = BaseDirs::new() {
            base.data_local_dir().join("MQ")
        } else if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
            PathBuf::from(local).join("MQ")
        } else if let Some(profile) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
            PathBuf::from(profile)
                .join("AppData")
                .join("Local")
                .join("MQ")
        } else {
            PathBuf::from(r"C:\MQ")
        };
        strip_windows_verbatim_prefix(&raw)
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(base) = BaseDirs::new() {
            base.data_dir().join("MQ")
        } else {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join("Library/Application Support/MQ")
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("mq");
            }
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".local/share/mq")
    }
}

/// Resolves the default configuration directory for the current platform:
/// - Windows: `%APPDATA%\MQ` (e.g. `C:\Users\<User>\AppData\Roaming\MQ`)
/// - Linux: `$XDG_CONFIG_HOME/mq` or `~/.config/mq`
/// - macOS: `~/Library/Preferences/MQ`
pub fn default_config_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let raw = if let Some(base) = BaseDirs::new() {
            base.config_dir().join("MQ")
        } else if let Some(appdata) = std::env::var_os("APPDATA").filter(|v| !v.is_empty()) {
            PathBuf::from(appdata).join("MQ")
        } else if let Some(profile) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
            PathBuf::from(profile)
                .join("AppData")
                .join("Roaming")
                .join("MQ")
        } else {
            PathBuf::from(r"C:\MQ\config")
        };
        strip_windows_verbatim_prefix(&raw)
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(base) = BaseDirs::new() {
            base.preference_dir().join("MQ")
        } else {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join("Library/Preferences/MQ")
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("mq");
            }
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".config/mq")
    }
}

/// Resolves the default ephemeral cache directory for the current platform:
/// - Windows: `%LOCALAPPDATA%\MQ\cache`
/// - Linux: `$XDG_CACHE_HOME/mq` or `~/.cache/mq`
/// - macOS: `~/Library/Caches/MQ`
pub fn default_cache_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        default_data_dir().join("cache")
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(base) = BaseDirs::new() {
            base.cache_dir().join("MQ")
        } else {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join("Library/Caches/MQ")
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("mq");
            }
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".cache/mq")
    }
}

/// Returns `true` if the given Linux/Unix launch wrapper command (`gamemoderun`, `mangohud`,
/// `prime-run`, etc.) is available on the current platform.
///
/// Linux-only wrappers (`gamemode`, `gamemoderun`, `mangohud`, `prime-run`) unconditionally
/// report unavailable (`false`) on Windows.
pub fn is_wrapper_available(wrapper: &str) -> bool {
    let trimmed = wrapper.trim();
    if trimmed.is_empty() {
        return false;
    }

    #[cfg(target_os = "windows")]
    {
        let lower = trimmed.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "gamemode" | "gamemoderun" | "mangohud" | "prime-run"
        ) {
            return false;
        }
        let candidate = Path::new(trimmed);
        if candidate.is_file() {
            return true;
        }
        if let Some(path_var) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_var) {
                let direct = dir.join(trimmed);
                if direct.is_file() {
                    return true;
                }
                if !lower.ends_with(".exe") && dir.join(format!("{trimmed}.exe")).is_file() {
                    return true;
                }
            }
        }
        false
    }

    #[cfg(not(target_os = "windows"))]
    {
        let candidate = Path::new(trimmed);
        if candidate.components().count() > 1 {
            return is_executable_unix_file(candidate);
        }
        if let Some(path_var) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_var) {
                if is_executable_unix_file(&dir.join(trimmed)) {
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(not(target_os = "windows"))]
fn is_executable_unix_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Checks whether a process with OS identifier `pid` is currently alive.
///
/// On Windows, uses direct Win32 `OpenProcess` + `GetExitCodeProcess` FFI without
/// spawning any console subprocess (`tasklist.exe` / `cmd.exe`).
pub fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    #[cfg(target_os = "windows")]
    {
        win32_platform::is_process_alive(pid)
    }

    #[cfg(target_os = "linux")]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|s| s.success())
    }
}

/// Atomically writes `content` to `path` via a temporary file and atomic replace.
/// Guarantees that readers never observe partially written contents.
pub fn atomic_write(path: &Path, content: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("atomic_file");
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!("{file_name}.tmp_{}_{seq}", std::process::id());
    let tmp_path = path.with_file_name(tmp_name);

    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
    }

    #[cfg(unix)]
    {
        // Enforce 0600 permissions on sensitive files if in config dir
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600));
    }

    if let Err(err) = rename_with_retry(&tmp_path, path) {
        let _ = remove_file_with_retry(&tmp_path);
        return Err(err);
    }

    Ok(())
}

/// Links `src` to `dst` via filesystem hardlink, or falls back seamlessly to an atomic copy
/// when hardlinking fails (e.g. cross-filesystem boundary, FAT/exFAT, or Windows permissions).
/// Stages via a sibling temp path + [`rename_with_retry`] so existing `dst` is never lost or
/// left partially written on failure.
pub fn link_or_copy(src: &Path, dst: &Path) -> io::Result<()> {
    if src == dst {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let file_name = dst
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("linked_file");
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = dst.with_file_name(format!(
        ".{file_name}.link_{}_{seq}.tmp",
        std::process::id()
    ));

    if let Err(link_err) = fs::hard_link(src, &tmp_path) {
        tracing::debug!(
            "Hardlink failed from {:?} to {:?} ({link_err}); falling back to staged copy",
            src,
            dst
        );
        if let Err(copy_err) = fs::copy(src, &tmp_path) {
            let _ = remove_file_with_retry(&tmp_path);
            return Err(copy_err);
        }
    }

    if let Err(rename_err) = rename_with_retry(&tmp_path, dst) {
        let _ = remove_file_with_retry(&tmp_path);
        return Err(rename_err);
    }

    Ok(())
}

/// Opens a local file or directory using the platform's default file explorer.
pub fn open_folder(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        let normalized = normalize_windows_explorer_path(path);
        Command::new("explorer.exe")
            .arg(normalized)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(path).spawn().map(|_| ())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Command::new("xdg-open").arg(path).spawn().map(|_| ())
    }
}

/// Opens a web URL in the system's default internet browser.
pub fn open_url(url: &str) -> io::Result<()> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot open an empty URL",
        ));
    }

    #[cfg(target_os = "windows")]
    {
        if win32_platform::open_shell_target(trimmed) {
            return Ok(());
        }
        use std::os::windows::process::CommandExt;
        Command::new("rundll32.exe")
            .creation_flags(WINDOWS_CREATE_NO_WINDOW)
            .args(["url.dll,FileProtocolHandler", trimmed])
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(trimmed).spawn().map(|_| ())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Command::new("xdg-open").arg(trimmed).spawn().map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_os_name_valid() {
        let name = os_name();
        assert!(matches!(name, "linux" | "windows" | "macos"));
    }

    #[test]
    fn test_classpath_separator() {
        let sep = classpath_separator();
        if cfg!(target_os = "windows") {
            assert_eq!(sep, ';');
        } else {
            assert_eq!(sep, ':');
        }
    }

    #[test]
    fn test_atomic_write_and_cyrillic_paths() {
        let dir = tempdir().expect("tempdir");
        let cyrillic_dir = dir.path().join("Иван Петров/Папка MQ");
        fs::create_dir_all(&cyrillic_dir).expect("create cyrillic dir");

        let target_file = cyrillic_dir.join("инстанс_тест.json");
        let data = b"{\"name\": \"\xD0\xA2\xD0\xB5\xD1\x81\xD1\x82\"}";

        atomic_write(&target_file, data).expect("atomic_write to cyrillic path");
        let read = fs::read(&target_file).expect("read file");
        assert_eq!(read, data);

        // Overwrite existing file cleanly via atomic_write.
        let data_v2 = b"{\"name\": \"\xD0\x9E\xD0\xB1\xD0\xBD\xD0\xBE\xD0\xB2\xD0\xBB\xD0\xB5\xD0\xBD\xD0\xBE\"}";
        atomic_write(&target_file, data_v2).expect("atomic_write overwrite");
        assert_eq!(fs::read(&target_file).expect("read overwritten"), data_v2);
    }

    #[test]
    fn test_link_or_copy_fallback_and_overwrite() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("source.blob");
        let dst = dir.path().join("dest/linked.blob");

        fs::write(&src, b"CAS BLOB CONTENT").expect("write src");
        link_or_copy(&src, &dst).expect("link_or_copy");

        let read = fs::read(&dst).expect("read dst");
        assert_eq!(read, b"CAS BLOB CONTENT");

        // Overwriting existing destination and self-link are both safe.
        let src2 = dir.path().join("source2.blob");
        fs::write(&src2, b"CAS BLOB V2").expect("write src2");
        link_or_copy(&src2, &dst).expect("link_or_copy overwrite");
        assert_eq!(
            fs::read(&dst).expect("read overwritten dst"),
            b"CAS BLOB V2"
        );

        link_or_copy(&dst, &dst).expect("same-path link_or_copy is no-op");
        assert_eq!(fs::read(&dst).expect("still intact"), b"CAS BLOB V2");
    }

    #[test]
    fn test_ensure_long_path_prefix_and_windows_path_helpers() {
        let short = Path::new(r"C:\MQ\instances\test");
        assert_eq!(ensure_long_path_prefix(short), short.to_path_buf());
        assert_eq!(
            format_windows_long_path(r"C:\MQ\instances\test"),
            r"C:\MQ\instances\test"
        );

        let long_drive = format!(r"C:\Users\Player\{}\client.jar", "a".repeat(250));
        assert_eq!(
            format_windows_long_path(&long_drive),
            format!(r"\\?\{long_drive}")
        );

        let long_unc = format!(r"\\fileserver\share\{}\client.jar", "b".repeat(250));
        assert_eq!(
            format_windows_long_path(&long_unc),
            format!(r"\\?\UNC\fileserver\share\{}\client.jar", "b".repeat(250))
        );

        assert_eq!(
            normalize_windows_explorer_path(Path::new(
                r"\\?\C:\Users\Иван Петров/AppData/Local/MQ"
            )),
            PathBuf::from(r"C:\Users\Иван Петров\AppData\Local\MQ")
        );
    }

    #[test]
    fn test_process_liveness_and_wrapper_availability() {
        assert!(is_process_alive(std::process::id()));
        assert!(!is_process_alive(0));
        assert!(!is_wrapper_available(""));
        assert!(!is_wrapper_available("nonexistent_mq_wrapper_xyz_987654"));

        #[cfg(target_os = "windows")]
        {
            assert!(!is_wrapper_available("gamemoderun"));
            assert!(!is_wrapper_available("gamemode"));
            assert!(!is_wrapper_available("mangohud"));
            assert!(!is_wrapper_available("prime-run"));
        }
    }
}
