//! Child JVM process runner with non-blocking asynchronous `stdout`/`stderr` line capture,
//! automatic secret token redaction (`[REDACTED]`), `GameLogLevel` classification, and
//! `EventBus` streaming.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use mq_core::fs_util::strip_windows_verbatim_prefix;
use mq_core::{redact_text, Event, EventBus, GameLogEntry, GameLogLevel, MqError, MqResult};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::command::BuiltLaunchCommand;

static ARGFILE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

struct ArgfileGuard(Option<std::path::PathBuf>);

impl Drop for ArgfileGuard {
    fn drop(&mut self) {
        if let Some(ref path) = self.0 {
            let _ = mq_core::fs_util::remove_file_with_retry(path);
        }
    }
}

fn resolve_java_major_for_command(program: &Path, args: &[String]) -> Option<u32> {
    let java_path: PathBuf = if crate::command::is_java_launcher_binary(program) {
        program.to_path_buf()
    } else {
        args.iter()
            .map(Path::new)
            .find(|p| crate::command::is_java_launcher_binary(p))?
            .to_path_buf()
    };
    let clean_java = strip_windows_verbatim_prefix(&java_path);
    crate::java::probe_java_binary(&clean_java).map(|rt| rt.major_version)
}

/// Spawns the configured Minecraft JVM (or wrapper) child process, streams `stdout` and `stderr`
/// lines asynchronously without blocking, scrubs any sensitive tokens in `secrets` using
/// [`redact_text`], classifies log severity via [`GameLogLevel::from_line`], and publishes
/// [`Event::GameLog`] entries to `event_bus`.
pub async fn spawn_and_stream_game(
    cmd: &BuiltLaunchCommand,
    instance_id: &str,
    secrets: Vec<String>,
    event_bus: Option<EventBus>,
) -> MqResult<ExitStatus> {
    spawn_and_stream_game_with_pid(cmd, instance_id, secrets, event_bus, None).await
}

/// Spawns the configured Minecraft JVM (or wrapper) child process, optionally notifies `pid_tx`
/// with the child OS PID immediately after spawn, streams `stdout` and `stderr` asynchronously,
/// scrubs sensitive tokens, and publishes [`Event::GameLog`] entries to `event_bus`.
pub async fn spawn_and_stream_game_with_pid(
    cmd: &BuiltLaunchCommand,
    instance_id: &str,
    secrets: Vec<String>,
    event_bus: Option<EventBus>,
    pid_tx: Option<tokio::sync::oneshot::Sender<u32>>,
) -> MqResult<ExitStatus> {
    let clean_program = strip_windows_verbatim_prefix(&cmd.program);
    let clean_working_dir = strip_windows_verbatim_prefix(&cmd.working_dir);

    if !clean_working_dir.as_os_str().is_empty() {
        fs::create_dir_all(&clean_working_dir)?;
    }

    #[cfg(target_os = "windows")]
    let (effective_args, _argfile_guard) = {
        use std::sync::atomic::Ordering;
        let argfile_base = if !clean_working_dir.as_os_str().is_empty() {
            clean_working_dir.clone()
        } else {
            std::env::temp_dir()
        };
        let seq = ARGFILE_SEQ.fetch_add(1, Ordering::Relaxed);
        let argfile_path =
            argfile_base.join(format!(".mq-args-{}-{seq}.argfile", std::process::id()));
        let java_major = resolve_java_major_for_command(&clean_program, &cmd.args);
        let (rewritten, created) = crate::command::rewrite_args_for_windows_limit(
            &clean_program,
            &cmd.args,
            &clean_working_dir,
            java_major,
            &argfile_path,
        )?;
        (rewritten, ArgfileGuard(created))
    };

    #[cfg(not(target_os = "windows"))]
    let (effective_args, _argfile_guard) = {
        let total_chars: usize = clean_program.to_string_lossy().len()
            + cmd.args.iter().map(|a| a.len() + 1).sum::<usize>();
        if total_chars > 100_000 {
            use std::sync::atomic::Ordering;
            let argfile_base = if !clean_working_dir.as_os_str().is_empty() {
                clean_working_dir.clone()
            } else {
                std::env::temp_dir()
            };
            let seq = ARGFILE_SEQ.fetch_add(1, Ordering::Relaxed);
            let argfile_path =
                argfile_base.join(format!(".mq-args-{}-{seq}.argfile", std::process::id()));
            let java_major = resolve_java_major_for_command(&clean_program, &cmd.args);
            let (rewritten, created) = crate::command::rewrite_args_for_windows_limit(
                &clean_program,
                &cmd.args,
                &clean_working_dir,
                java_major.or(Some(21)),
                &argfile_path,
            )?;
            (rewritten, ArgfileGuard(created))
        } else {
            (cmd.args.clone(), ArgfileGuard(None))
        }
    };

    let mut command = Command::new(&clean_program);
    command
        .args(&effective_args)
        .envs(&cmd.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for env_var in ["_JAVA_OPTIONS", "JAVA_TOOL_OPTIONS", "JDK_JAVA_OPTIONS"] {
        if !cmd.env.contains_key(env_var) {
            command.env_remove(env_var);
        }
    }

    if !clean_working_dir.as_os_str().is_empty() {
        command.current_dir(&clean_working_dir);
    }

    #[cfg(target_os = "windows")]
    {
        // 0x08000000 = CREATE_NO_WINDOW: prevent console window popup for Minecraft java.exe
        command.creation_flags(0x0800_0000);
    }

    let mut child = command.spawn().map_err(|err| {
        MqError::Process(format!(
            "failed to spawn game process '{}': {err}",
            clean_program.display()
        ))
    })?;

    if let (Some(tx), Some(pid)) = (pid_tx, child.id()) {
        let _ = tx.send(pid);
    }

    let shared_secrets = Arc::new(secrets);
    let instance_slug = instance_id.to_owned();

    let stdout_task = child.stdout.take().map(|stdout| {
        let reader = BufReader::new(stdout);
        let inst = instance_slug.clone();
        let sec = Arc::clone(&shared_secrets);
        let bus = event_bus.clone();
        tokio::spawn(async move {
            stream_log_lines(reader, inst, sec, bus).await;
        })
    });

    let stderr_task = child.stderr.take().map(|stderr| {
        let reader = BufReader::new(stderr);
        let inst = instance_slug;
        let sec = Arc::clone(&shared_secrets);
        let bus = event_bus;
        tokio::spawn(async move {
            stream_log_lines(reader, inst, sec, bus).await;
        })
    });

    let status = child.wait().await.map_err(|err| {
        MqError::Process(format!(
            "error waiting for game process '{}': {err}",
            clean_program.display()
        ))
    })?;

    // Bound log drain after child exit so inherited pipe handles on Windows cannot hang indefinitely.
    if let Some(handle) = stdout_task {
        if tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .is_err()
        {
            tracing::debug!("stdout log drain timed out after game exit");
        }
    }
    if let Some(handle) = stderr_task {
        if tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .is_err()
        {
            tracing::debug!("stderr log drain timed out after game exit");
        }
    }

    Ok(status)
}

async fn stream_log_lines<R>(
    mut reader: R,
    instance_id: String,
    secrets: Arc<Vec<String>>,
    event_bus: Option<EventBus>,
) where
    R: AsyncBufRead + Unpin,
{
    let secret_refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
    let mut buf = Vec::with_capacity(256);

    // Use `read_until(b'\n')` + lossy UTF-8 decoding instead of `lines()` so non-UTF-8 bytes
    // (e.g. Windows CP1251/CP866 native/JVM output) never abort log streaming or stall pipes.
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => break,
            Ok(_) => {
                while matches!(buf.last(), Some(b'\n' | b'\r')) {
                    buf.pop();
                }
                let raw_line = String::from_utf8_lossy(&buf);
                let redacted = redact_text(&raw_line, &secret_refs);
                let level = GameLogLevel::from_line(&redacted);
                let mut entry = GameLogEntry::from_raw_line(instance_id.clone(), redacted);
                entry.level = level;

                if let Some(ref bus) = event_bus {
                    bus.publish(Event::GameLog(entry));
                }
            }
            Err(_) => break,
        }
    }
}

/// Extracts a process exit code, preserving Unix signal termination as `128 + signal`.
pub fn exit_status_code(status: &ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    -1
}

/// Terminates a game process and all of its descendant child processes.
/// On Windows, executes `taskkill /F /T /PID <pid>` with `CREATE_NO_WINDOW` and falls back to
/// Win32 `TerminateProcess` if `taskkill` is unavailable or fails.
/// On Unix, collects descendant PIDs from `/proc` and terminates the tree via SIGKILL.
pub fn kill_process_tree(pid: u32) -> Result<(), std::io::Error> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let taskkill_ok = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !taskkill_ok {
            terminate_windows_pid_fallback(pid);
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        let mut pids = collect_descendant_pids(pid);
        pids.push(pid);
        for target in pids {
            let _ = std::process::Command::new("kill")
                .args(["-9", &target.to_string()])
                .output();
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn terminate_windows_pid_fallback(pid: u32) {
    type Handle = *mut std::ffi::c_void;
    const PROCESS_TERMINATE: u32 = 0x0001;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn TerminateProcess(process: Handle, exit_code: u32) -> i32;
        fn CloseHandle(object: Handle) -> i32;
    }

    // SAFETY: Win32 kernel32 APIs called with valid POD arguments and checked non-null handle.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !handle.is_null() {
            let _ = TerminateProcess(handle, 1);
            let _ = CloseHandle(handle);
        }
    }
}

#[cfg(unix)]
fn collect_descendant_pids(root_pid: u32) -> Vec<u32> {
    let mut children_map: std::collections::HashMap<u32, Vec<u32>> =
        std::collections::HashMap::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid_str) = name.to_str() else {
                continue;
            };
            let Ok(child_pid) = pid_str.parse::<u32>() else {
                continue;
            };
            let stat_path = entry.path().join("stat");
            if let Ok(stat) = fs::read_to_string(stat_path) {
                if let Some(rparen) = stat.rfind(')') {
                    let rest = &stat[rparen + 1..];
                    let mut fields = rest.split_whitespace();
                    let _state = fields.next();
                    if let Some(ppid_str) = fields.next() {
                        if let Ok(ppid) = ppid_str.parse::<u32>() {
                            children_map.entry(ppid).or_default().push(child_pid);
                        }
                    }
                }
            }
        }
    }

    let mut result = Vec::new();
    let mut stack = vec![root_pid];
    while let Some(curr) = stack.pop() {
        if let Some(kids) = children_map.get(&curr) {
            for &kid in kids {
                result.push(kid);
                stack.push(kid);
            }
        }
    }
    result.reverse();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stream_log_lines_handles_windows_crlf_and_non_utf8_cp1251_without_stopping() {
        let bus = EventBus::with_capacity(16);
        // Line 1: valid UTF-8 with CRLF
        // Line 2: raw CP1251 bytes ("Ошибка" in CP1251) + secret token with CRLF
        // Line 3: valid UTF-8 line after invalid UTF-8 without trailing newline
        let mut payload = Vec::new();
        payload.extend_from_slice(b"[INFO] First line\r\n");
        payload.extend_from_slice(&[0xCE, 0xF8, 0xE8, 0xE1, 0xEA, 0xE0]);
        payload.extend_from_slice(b" token=secret_win_tok\r\n");
        payload.extend_from_slice(b"[WARN] Third line survives");

        let reader = BufReader::new(std::io::Cursor::new(payload));
        stream_log_lines(
            reader,
            "win-inst".to_owned(),
            Arc::new(vec!["secret_win_tok".to_owned()]),
            Some(bus.clone()),
        )
        .await;

        let events = bus.drain_all();
        assert_eq!(
            events.len(),
            3,
            "stream_log_lines must not stop on non-UTF-8 Windows console bytes"
        );
        let lines: Vec<String> = events
            .into_iter()
            .filter_map(|e| match e {
                Event::GameLog(entry) => Some(entry.line),
                _ => None,
            })
            .collect();
        assert_eq!(lines[0], "[INFO] First line");
        assert!(!lines[1].contains("secret_win_tok"));
        assert!(lines[1].contains("[REDACTED]"));
        assert!(!lines[1].ends_with('\r'));
        assert_eq!(lines[2], "[WARN] Third line survives");
    }

    #[test]
    fn resolve_java_major_for_wrapped_java8_prevents_argfile() {
        let tmp = tempfile::tempdir().unwrap();
        let java_home = tmp.path().join("jre8");
        let bin_dir = java_home.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let java_bin = bin_dir.join(if cfg!(windows) { "java.exe" } else { "java" });
        fs::write(&java_bin, b"dummy").unwrap();
        fs::write(java_home.join("release"), "JAVA_VERSION=\"1.8.0_412\"\n").unwrap();

        let wrapper = Path::new("gamemoderun");
        let huge_cp = (0..450)
            .map(|i| format!("/opt/mq/libraries/org/example/modlib-{i:03}/1.0/modlib-{i:03}.jar"))
            .collect::<Vec<_>>()
            .join(":");
        let args = vec![
            java_bin.to_string_lossy().into_owned(),
            "-cp".to_owned(),
            huge_cp,
            "net.minecraft.launchwrapper.Launch".to_owned(),
        ];

        let major = resolve_java_major_for_command(wrapper, &args);
        assert_eq!(major, Some(8));

        let argfile_path = tmp.path().join(".mq-args-wrapped-java8.argfile");
        let (rewritten, created) = crate::command::rewrite_args_for_windows_limit(
            wrapper,
            &args,
            tmp.path(),
            major,
            &argfile_path,
        )
        .unwrap();
        assert!(created.is_none());
        assert!(!argfile_path.exists());
        assert!(!rewritten.iter().any(|a| a.starts_with('@')));
    }
}
