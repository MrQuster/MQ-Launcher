//! Child JVM process runner with non-blocking asynchronous `stdout`/`stderr` line capture,
//! automatic secret token redaction (`[REDACTED]`), `GameLogLevel` classification, and
//! `EventBus` streaming.

use std::fs;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;

use mq_core::{redact_text, Event, EventBus, GameLogEntry, GameLogLevel, MqError, MqResult};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::command::BuiltLaunchCommand;

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
    if !cmd.working_dir.as_os_str().is_empty() {
        fs::create_dir_all(&cmd.working_dir)?;
    }

    let mut command = Command::new(&cmd.program);
    command
        .args(&cmd.args)
        .envs(&cmd.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if !cmd.working_dir.as_os_str().is_empty() {
        command.current_dir(&cmd.working_dir);
    }

    #[cfg(target_os = "windows")]
    {
        // 0x08000000 = CREATE_NO_WINDOW: prevent console window popup for Minecraft javaw/java
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }

    let mut child = command.spawn().map_err(|err| {
        MqError::Process(format!(
            "failed to spawn game process '{}': {err}",
            cmd.program.display()
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
            cmd.program.display()
        ))
    })?;

    if let Some(handle) = stdout_task {
        let _ = handle.await;
    }
    if let Some(handle) = stderr_task {
        let _ = handle.await;
    }

    Ok(status)
}

async fn stream_log_lines<R>(
    reader: R,
    instance_id: String,
    secrets: Arc<Vec<String>>,
    event_bus: Option<EventBus>,
) where
    R: AsyncBufRead + Unpin,
{
    let secret_refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
    let mut lines = reader.lines();

    while let Ok(Some(raw_line)) = lines.next_line().await {
        let redacted = redact_text(&raw_line, &secret_refs);
        let level = GameLogLevel::from_line(&redacted);
        let mut entry = GameLogEntry::from_raw_line(instance_id.clone(), redacted);
        entry.level = level;

        if let Some(ref bus) = event_bus {
            bus.publish(Event::GameLog(entry));
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
/// On Windows, executes `taskkill /F /T /PID <pid>`.
/// On Unix, collects descendant PIDs from `/proc` and terminates the tree via SIGKILL.
pub fn kill_process_tree(pid: u32) -> Result<(), std::io::Error> {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
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
