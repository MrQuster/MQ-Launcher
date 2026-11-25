//! Automated game crash analysis and 1-click `mclogs` API integration.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mq_core::{MqError, MqResult};
use serde::{Deserialize, Serialize};

/// Detailed result of a game crash diagnosis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrashAnalysis {
    /// Non-zero process exit code, if known.
    pub exit_code: Option<i32>,
    /// Concise human-readable cause in Russian.
    pub summary: String,
    /// Actionable recommendation in Russian.
    pub recommendation: String,
    /// Names or identifiers of mods suspected of causing the crash.
    pub suspected_mods: Vec<String>,
    /// Relevant error snippet or stacktrace.
    pub raw_snippet: String,
    /// Absolute path to the detected `crash-reports/*.txt` file, if present.
    pub crash_report_file: Option<PathBuf>,
}

/// Analyzes crash reports and trailing console log lines to diagnose game failure.
pub fn analyze_crash(
    instance_dir: &Path,
    exit_code: Option<i32>,
    log_lines: &[String],
) -> CrashAnalysis {
    // 1. Check for recent crash-reports/*.txt
    let crash_dir = instance_dir.join("crash-reports");
    let mut latest_crash_file: Option<(PathBuf, SystemTime)> = None;

    if let Ok(entries) = fs::read_dir(&crash_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("txt") {
                if let Ok(meta) = path.metadata() {
                    if let Ok(mtime) = meta.modified() {
                        if latest_crash_file
                            .as_ref()
                            .map(|(_, t)| mtime > *t)
                            .unwrap_or(true)
                        {
                            latest_crash_file = Some((path, mtime));
                        }
                    }
                }
            }
        }
    }

    let is_stale = latest_crash_file
        .as_ref()
        .and_then(|(_, mtime)| SystemTime::now().duration_since(*mtime).ok())
        .map(|dur| dur.as_secs() > 120)
        .unwrap_or(false);

    let mut crash_text = String::new();
    let mut report_path = None;

    // Do not let stale historical crash reports (>120s) override fresh non-empty console logs
    let use_report = if is_stale && !log_lines.is_empty() {
        false
    } else {
        latest_crash_file.is_some()
    };

    if use_report {
        if let Some((path, _)) = latest_crash_file {
            if let Ok(content) = fs::read_to_string(&path) {
                crash_text = content;
                report_path = Some(path);
            }
        }
    }

    if crash_text.is_empty() {
        // Fallback: analyze trailing 500 lines of console output
        let trailing = if log_lines.len() > 500 {
            &log_lines[log_lines.len() - 500..]
        } else {
            log_lines
        };
        crash_text = trailing.join("\n");
    }

    diagnose_text(&crash_text, exit_code, report_path)
}

fn diagnose_text(
    text: &str,
    exit_code: Option<i32>,
    report_file: Option<PathBuf>,
) -> CrashAnalysis {
    let lower = text.to_lowercase();
    let mut suspected_mods = Vec::new();

    // 1. Out of Memory (OOM) or JVM heap too small
    if lower.contains("outofmemoryerror")
        || lower.contains("java heap space")
        || lower.contains("too small maximum heap")
        || lower.contains("initial heap size set to a larger value than the maximum heap size")
        || lower.contains("could not reserve enough space for")
        || lower
            .contains("there is insufficient memory for the java runtime environment to continue")
        || exit_code == Some(137)
    {
        return CrashAnalysis {
            exit_code,
            summary: "Нехватка оперативной памяти (OutOfMemoryError)".to_string(),
            recommendation: "Выделенной памяти недостаточно для загрузки игры и модов. Перейдите в настройки инстанса и увеличьте максимальное выделение RAM (рекомендуется от 4096 МБ до 6144 МБ).".to_string(),
            suspected_mods: Vec::new(),
            raw_snippet: extract_snippet(text, "memory", 6),
            crash_report_file: report_file,
        };
    }

    // 2. Incompatible Java version
    if lower.contains("unsupportedclassversionerror")
        || lower.contains("has been compiled by a more recent version of the java runtime")
    {
        return CrashAnalysis {
            exit_code,
            summary: "Неподходящая версия Java (UnsupportedClassVersionError)".to_string(),
            recommendation: "Один или несколько модов скомпилированы для более новой версии Java, чем выбранная. Перейдите в настройки инстанса или воспользуйтесь автоматической загрузкой Temurin JRE 17 / 21.".to_string(),
            suspected_mods: Vec::new(),
            raw_snippet: extract_snippet(text, "UnsupportedClassVersionError", 6),
            crash_report_file: report_file,
        };
    }

    // 3. Duplicate Mods
    if lower.contains("duplicatemodsfoundexception") || lower.contains("duplicate mod id") {
        extract_mod_candidates(text, &mut suspected_mods);
        return CrashAnalysis {
            exit_code,
            summary: "Обнаружены дубликаты модов".to_string(),
            recommendation: "В папке mods установлены разные версии одного и того же мода. Удалите устаревшие копии во вкладке «Моды».".to_string(),
            suspected_mods,
            raw_snippet: extract_snippet(text, "duplicate", 8),
            crash_report_file: report_file,
        };
    }

    // 4. Missing Dependency
    if lower.contains("classnotfoundexception") || lower.contains("noclassdeffounderror") {
        extract_mod_candidates(text, &mut suspected_mods);
        return CrashAnalysis {
            exit_code,
            summary: "Отсутствует обязательная библиотека или зависимость мода".to_string(),
            recommendation: "Один из модов требует библиотеки (например, Fabric API, Cloth Config, Architectury). Проверьте список зависимостей во вкладке «Моды» или в Каталоге.".to_string(),
            suspected_mods,
            raw_snippet: extract_snippet(text, "ClassNotFoundException", 8),
            crash_report_file: report_file,
        };
    }

    // 5. Mixin Transformation Failure
    if lower.contains("mixinapplyerror") || lower.contains("org.spongepowered.asm.mixin") {
        extract_mod_candidates(text, &mut suspected_mods);
        return CrashAnalysis {
            exit_code,
            summary: "Конфликт Mixin-инъекций между модами".to_string(),
            recommendation: "Два или более мода пытаются изменить один и тот же метод в игре несовместимым способом. Попробуйте обновить конфликтующие моды до актуальных версий или отключить подозрительный мод.".to_string(),
            suspected_mods,
            raw_snippet: extract_snippet(text, "mixin", 10),
            crash_report_file: report_file,
        };
    }

    // 6. Shader / Iris / Sodium / OpenGL / GLFW errors
    if lower.contains("net.irisshaders")
        || lower.contains("me.jellysquid.mods.sodium")
        || lower.contains("glfw error")
        || lower.contains("wgl")
        || lower.contains("glcontext")
        || lower.contains("opengl error")
        || lower.contains("gl_invalid_operation")
    {
        extract_mod_candidates(text, &mut suspected_mods);
        return CrashAnalysis {
            exit_code,
            summary: "Сбой графического шейдер-движка или OpenGL/GLFW".to_string(),
            recommendation: "Произошла ошибка инициализации рендерера (Iris/Sodium/OpenGL). Убедитесь, что драйверы видеокарты обновлены (поддержка OpenGL >= 3.3) и установлена совместимая версия шейдеров и Iris/Sodium для вашей версии игры.".to_string(),
            suspected_mods,
            raw_snippet: extract_snippet(text, "gl", 8),
            crash_report_file: report_file,
        };
    }

    // 7. Signal / native exception termination (Unix SIGSEGV/SIGABRT/SIGTERM & Windows NTSTATUS)
    if matches!(
        exit_code,
        Some(139)
            | Some(134)
            | Some(143)
            | Some(-1_073_741_819) // 0xC0000005 STATUS_ACCESS_VIOLATION
            | Some(-1_073_741_571) // 0xC00000FD STATUS_STACK_OVERFLOW
            | Some(-1_073_740_791) // 0xC0000409 STATUS_STACK_BUFFER_OVERRUN
    ) {
        let (sig_name, rec) = match exit_code {
            Some(139) => (
                "SIGSEGV",
                "Произошёл сбой нативной библиотеки или JVM (Segmentation Fault). Проверьте драйверы видеокарты, версию Java и нативные библиотеки.",
            ),
            Some(-1_073_741_819) => (
                "ACCESS_VIOLATION / 0xC0000005",
                "Произошёл сбой нативной библиотеки Windows (Access Violation, 0xC0000005). Чаще всего это связано с драйверами видеокарты (OpenGL/LWJGL), несовместимой Java или оверлеями.",
            ),
            Some(-1_073_741_571) => (
                "STACK_OVERFLOW / 0xC00000FD",
                "Переполнение стека нативного потока Windows (0xC00000FD). Проверьте моды и аргументы JVM (-Xss).",
            ),
            Some(-1_073_740_791) => (
                "STACK_BUFFER_OVERRUN / 0xC0000409",
                "Аварийное завершение процесса Windows (0xC0000409). Проверьте драйверы видеокарты и целостность нативных библиотек.",
            ),
            Some(134) => (
                "SIGABRT",
                "Процесс JVM был аварийно прерван (SIGABRT). Проверьте журнал консоли и совместимость модов/драйверов.",
            ),
            _ => (
                "SIGTERM",
                "Процесс игры был завершён внешним сигналом (SIGTERM).",
            ),
        };
        return CrashAnalysis {
            exit_code,
            summary: format!("Процесс игры завершён по сигналу ({sig_name})"),
            recommendation: rec.to_string(),
            suspected_mods,
            raw_snippet: extract_tail_snippet(text, 12),
            crash_report_file: report_file,
        };
    }

    // 8. Generic Failure
    extract_mod_candidates(text, &mut suspected_mods);
    let code_str = exit_code.map(|c| format!(" (код {c})")).unwrap_or_default();
    CrashAnalysis {
        exit_code,
        summary: format!("Игра неожиданно завершила работу{code_str}"),
        recommendation: "Проверьте журнал консоли и отчёт об ошибке. Если установлены новые моды, попробуйте отключать их по очереди для локализации сбоя.".to_string(),
        suspected_mods,
        raw_snippet: extract_tail_snippet(text, 12),
        crash_report_file: report_file,
    }
}

fn extract_snippet(text: &str, keyword: &str, line_count: usize) -> String {
    let lower_kw = keyword.to_lowercase();
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.to_lowercase().contains(&lower_kw) {
            let start = i.saturating_sub(2);
            let end = (start + line_count).min(lines.len());
            return lines[start..end].join("\n");
        }
    }
    extract_tail_snippet(text, line_count)
}

fn extract_tail_snippet(text: &str, line_count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= line_count {
        return text.to_string();
    }
    lines[lines.len() - line_count..].join("\n")
}

fn extract_mod_candidates(text: &str, out: &mut Vec<String>) {
    for line in text.lines() {
        if line.contains(".jar") {
            for token in line.split_whitespace() {
                if token.ends_with(".jar") || token.contains(".jar:") || token.contains(".jar!") {
                    let jar_part = token
                        .split_once(".jar")
                        .map(|(head, _)| format!("{head}.jar"))
                        .unwrap_or_else(|| token.to_owned());
                    let file_name = jar_part.rsplit(['/', '\\']).next().unwrap_or(&jar_part);
                    let clean = file_name.trim_matches(|c: char| {
                        !c.is_alphanumeric() && c != '.' && c != '-' && c != '_'
                    });
                    if !clean.is_empty() && !out.contains(&clean.to_string()) && out.len() < 5 {
                        out.push(clean.to_string());
                    }
                }
            }
        }
    }
}

/// Uploads a crash log or console log to the public `mclogs` API (`https://api.mclogs.com/1/log`).
///
/// Returns the shareable URL (e.g. `https://mclogs.com/xxxxxxx`).
pub async fn upload_to_mclogs(client: &reqwest::Client, log_content: &str) -> MqResult<String> {
    if log_content.trim().is_empty() {
        return Err(MqError::Internal("log content is empty".to_string()));
    }

    let url = "https://api.mclogs.com/1/log";
    let params = [("content", log_content)];

    let resp = client
        .post(url)
        .form(&params)
        .send()
        .await
        .map_err(|e| MqError::Network(format!("failed to connect to mclogs API: {e}")))?;

    if !resp.status().is_success() {
        return Err(MqError::Network(format!(
            "mclogs API returned HTTP status {}",
            resp.status()
        )));
    }

    let json_val: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| MqError::Network(format!("failed parsing mclogs API response: {e}")))?;

    if json_val.get("success").and_then(|v| v.as_bool()) == Some(true) {
        if let Some(log_url) = json_val.get("url").and_then(|v| v.as_str()) {
            return Ok(log_url.to_string());
        }
    }

    let err_msg = json_val
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error from mclogs API");

    Err(MqError::Network(format!("mclogs upload error: {err_msg}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_crash_picks_up_logs_when_no_recent_crash_report() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let fresh_logs = vec![
            "[Render thread/ERROR]: Incompatible mod set!".to_string(),
            "[Render thread/ERROR]: Mod 'foo' requires 'bar' 1.0+".to_string(),
        ];

        let result = analyze_crash(tmp.path(), Some(1), &fresh_logs);
        assert!(result.crash_report_file.is_none());
        assert!(!result.summary.is_empty());
    }

    #[test]
    fn analyze_crash_picks_up_fresh_crash_report() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let crash_dir = tmp.path().join("crash-reports");
        fs::create_dir_all(&crash_dir).expect("mkdir");

        let fresh_report = crash_dir.join("crash-fresh.txt");
        fs::write(&fresh_report, "OutOfMemoryError: Java heap space").expect("write");

        let logs = vec!["some normal log".to_string()];
        let result = analyze_crash(tmp.path(), Some(1), &logs);
        assert_eq!(
            result.summary,
            "Нехватка оперативной памяти (OutOfMemoryError)"
        );
        assert_eq!(result.crash_report_file, Some(fresh_report));
    }
}
