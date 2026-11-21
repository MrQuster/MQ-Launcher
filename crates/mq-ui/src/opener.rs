//! Cross-platform URL and folder/file opener for MQ.
//!
//! Provides `open_that(target)` to open web URLs in the default browser or open local
//! files / directories in the platform's file manager:
//! - Windows: native Win32 `ShellExecuteW` (never invokes `cmd.exe`, preserving `&` in OAuth/query
//!   URLs, Unicode/Cyrillic paths, and avoiding CMD console flashes), with safe fallback to
//!   `rundll32.exe url.dll,FileProtocolHandler` (URLs) or `explorer.exe` (local paths).
//! - macOS: `open <target>`
//! - Linux/Unix: `xdg-open <target>`

use std::ffi::OsStr;
use std::io;
use std::process::Command;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(target_os = "windows")]
const SW_SHOWNORMAL: i32 = 1;

#[cfg(target_os = "windows")]
#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut std::ffi::c_void,
        lp_operation: *const u16,
        lp_file: *const u16,
        lp_parameters: *const u16,
        lp_directory: *const u16,
        n_show_cmd: i32,
    ) -> isize;
}

/// Strips Windows verbatim path prefixes (`\\?\C:\...` -> `C:\...`, `\\?\UNC\host\share` -> `\\host\share`).
/// Safe to call on all platforms and non-verbatim paths.
pub fn strip_windows_verbatim_prefix(raw: &str) -> String {
    if raw.len() >= 8 && raw[..8].eq_ignore_ascii_case(r"\\?\UNC\") {
        return format!(r"\\{}", &raw[8..]);
    }
    if let Some(rest) = raw.strip_prefix(r"\\?\") {
        return rest.to_owned();
    }
    raw.to_owned()
}

/// Normalizes a user-pasted filesystem path: trims whitespace, removes matching surrounding
/// quotes (added by Windows Explorer "Copy as path"), and strips Windows `\\?\` verbatim prefixes.
pub fn clean_user_path(raw: &str) -> String {
    let s = raw.trim();
    let unquoted = if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].trim()
    } else {
        s
    };
    strip_windows_verbatim_prefix(unquoted)
}

/// Returns `true` when `target` is an HTTP or HTTPS URL.
pub fn is_web_url(target: &str) -> bool {
    let trimmed = target.trim();
    trimmed.len() >= 7
        && (trimmed[..7].eq_ignore_ascii_case("http://")
            || (trimmed.len() >= 8 && trimmed[..8].eq_ignore_ascii_case("https://")))
}

/// Validates and normalizes an open target for Windows `ShellExecuteW` / native fallbacks.
/// Strips any `\\?\` verbatim prefix and rejects empty or NUL-containing inputs.
pub fn prepare_windows_open_target(raw: &OsStr) -> io::Result<String> {
    let text = raw.to_string_lossy();
    if text.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "open target contains NUL byte",
        ));
    }
    let normalized = if is_web_url(&text) {
        let trimmed = text.trim();
        if trimmed.contains('\r') || trimmed.contains('\n') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "URL contains newline control characters",
            ));
        }
        trimmed.to_owned()
    } else {
        strip_windows_verbatim_prefix(&text)
    };
    if normalized.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "open target is empty",
        ));
    }
    Ok(normalized)
}

/// Encodes a UTF-8 string as NUL-terminated UTF-16 code units for Win32 `*W` APIs.
pub fn encode_utf16_nul(s: &str) -> io::Result<Vec<u16>> {
    if s.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "string contains embedded NUL",
        ));
    }
    let mut wide: Vec<u16> = s.encode_utf16().collect();
    wide.push(0);
    Ok(wide)
}

/// Encodes a PowerShell script as UTF-16LE Base64 suitable for `powershell.exe -EncodedCommand`,
/// preserving Cyrillic/Unicode text regardless of Windows console code page.
pub fn encode_powershell_utf16le_base64(script: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
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

/// Opens a path or URL using the platform-specific default application.
pub fn open_that<T: AsRef<OsStr>>(target: T) -> io::Result<()> {
    let target = target.as_ref();

    #[cfg(target_os = "windows")]
    {
        let prepared = prepare_windows_open_target(target)?;
        let op_wide = encode_utf16_nul("open")?;
        let file_wide = encode_utf16_nul(&prepared)?;

        // SAFETY: `op_wide` and `file_wide` are valid NUL-terminated UTF-16 slices live for the call.
        let ret = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                op_wide.as_ptr(),
                file_wide.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        if ret > 32 {
            return Ok(());
        }

        tracing::warn!(
            code = ret,
            "ShellExecuteW failed to open target; trying direct Windows fallback"
        );
        if is_web_url(&prepared) {
            // Never pass URLs through `cmd /C start` (which splits on `&` and executes commands).
            Command::new("rundll32.exe")
                .args(["url.dll,FileProtocolHandler", &prepared])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map(|_| ())
        } else {
            Command::new("explorer.exe")
                .arg(&prepared)
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map(|_| ())
        }
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(target).spawn().map(|_| ())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Command::new("xdg-open").arg(target).spawn().map(|_| ())
    }
}

/// Opens a native OS file picker dialog for selecting a `.mrpack` or `.zip` modpack archive.
/// Must be called from a background/blocking thread so the UI thread never stalls.
pub fn pick_modpack_archive_file() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let script = concat!(
            "Add-Type -AssemblyName System.Windows.Forms; ",
            "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); ",
            "$owner = New-Object System.Windows.Forms.Form; ",
            "$owner.TopMost = $true; ",
            "$d = New-Object System.Windows.Forms.OpenFileDialog; ",
            "$d.Title = 'Выберите сборку (.mrpack или .zip)'; ",
            "$d.Filter = 'Сборки (*.mrpack;*.zip)|*.mrpack;*.zip|Все файлы (*.*)|*.*'; ",
            "try { ",
            "if ($d.ShowDialog($owner) -eq [System.Windows.Forms.DialogResult]::OK) { ",
            "Write-Output $d.FileName } ",
            "} finally { $d.Dispose(); $owner.Dispose() }"
        );
        let encoded = encode_powershell_utf16le_base64(script);
        let out = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-STA",
                "-EncodedCommand",
                &encoded,
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let raw = String::from_utf8_lossy(&out.stdout);
        let s = raw.trim_start_matches('\u{feff}').trim();
        let cleaned = strip_windows_verbatim_prefix(s);
        if cleaned.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(cleaned))
        }
    }

    #[cfg(target_os = "macos")]
    {
        let script = r#"POSIX path of (choose file with prompt "Выберите сборку (.mrpack или .zip)" of type {"mrpack", "zip"})"#;
        let out = Command::new("osascript")
            .args(["-e", script])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        (!s.is_empty()).then(|| std::path::PathBuf::from(s))
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(out) = Command::new("zenity")
            .args([
                "--file-selection",
                "--title=Выберите сборку (.mrpack или .zip)",
                "--file-filter=Сборки (*.mrpack *.zip) | *.mrpack *.zip",
                "--file-filter=Все файлы | *",
            ])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                if !s.is_empty() {
                    return Some(std::path::PathBuf::from(s));
                }
            }
            return None;
        }

        if let Ok(out) = Command::new("kdialog")
            .args([
                "--getopenfilename",
                ".",
                "*.mrpack *.zip|Сборки (.mrpack, .zip)",
            ])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                if !s.is_empty() {
                    return Some(std::path::PathBuf::from(s));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_that_signature() {
        // Validate open_that accepts &str, String, Path, PathBuf at compile time
        let check_signatures = || {
            let _ = open_that("https://github.com/MrQuster/mq");
            let _ = open_that(String::from("https://github.com/MrQuster/mq"));
            let _ = open_that(std::path::Path::new("."));
            let _ = open_that(std::path::PathBuf::from("."));
        };
        let _ = check_signatures;
    }

    #[test]
    fn windows_verbatim_prefix_and_user_path_cleaning() {
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\C:\Users\Иван\AppData\Roaming\MQ"),
            r"C:\Users\Иван\AppData\Roaming\MQ"
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\UNC\fileserver\share\modpack.mrpack"),
            r"\\fileserver\share\modpack.mrpack"
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\unc\NAS\Games"),
            r"\\NAS\Games"
        );
        assert_eq!(
            strip_windows_verbatim_prefix("/home/user/mq"),
            "/home/user/mq"
        );

        assert_eq!(
            clean_user_path(r#"  "\\?\C:\My Folder\bg image.png"  "#),
            r"C:\My Folder\bg image.png"
        );
        assert_eq!(clean_user_path(" '/tmp/theme.json' "), "/tmp/theme.json");
    }

    #[test]
    fn windows_open_target_preserves_oauth_ampersands_and_rejects_invalid_inputs() {
        let oauth = "https://account.ely.by/oauth2/v1?client_id=mq&redirect_uri=http://127.0.0.1:43210/cb&response_type=code&scope=account_info%20minecraft_server_session";
        let prepared = prepare_windows_open_target(OsStr::new(oauth)).expect("valid OAuth URL");
        assert_eq!(prepared, oauth);
        assert!(is_web_url(&prepared));

        let verbatim_dir = r"\\?\C:\Users\Тест Пользователь\Downloads";
        let prepared_dir =
            prepare_windows_open_target(OsStr::new(verbatim_dir)).expect("valid Windows dir");
        assert_eq!(prepared_dir, r"C:\Users\Тест Пользователь\Downloads");
        assert!(!is_web_url(&prepared_dir));

        assert!(prepare_windows_open_target(OsStr::new("")).is_err());
        assert!(prepare_windows_open_target(OsStr::new("https://example.com\r\ncalc")).is_err());
        assert!(prepare_windows_open_target(OsStr::new("bad\0target")).is_err());
        assert!(encode_utf16_nul("ok\0bad").is_err());

        let wide = encode_utf16_nul("Привет&test").expect("wide utf16");
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(
            String::from_utf16_lossy(&wide[..wide.len() - 1]),
            "Привет&test"
        );
    }

    #[test]
    fn powershell_utf16le_base64_matches_standard_vectors() {
        // "hi" -> UTF-16LE bytes [0x68, 0x00, 0x69, 0x00] -> Base64 "aABpAA=="
        assert_eq!(encode_powershell_utf16le_base64("hi"), "aABpAA==");
        assert_eq!(encode_powershell_utf16le_base64(""), "");
        // Cyrillic string encodes cleanly without panicking and preserves 4-char Base64 alignment
        let b64 = encode_powershell_utf16le_base64("Выберите сборку");
        assert_eq!(b64.len() % 4, 0);
        assert!(!b64.is_empty());
    }
}
