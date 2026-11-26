//! Discord Rich Presence IPC v1 client using pure std network/filesystem primitives.
//!
//! Connects to Discord desktop client via local domain socket (Unix) or named pipe (Windows),
//! sending JSON frames prefixed with opcode and length headers. Fails silently if Discord is absent.

use std::io::{Read, Write};

#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::time::Duration;

#[cfg(windows)]
use std::fs::{File, OpenOptions};

const DISCORD_APP_ID: &str = "1575415195591778364";

fn get_discord_app_id() -> String {
    if let Ok(val) = std::env::var("MQ_DISCORD_APP_ID") {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Some(val) = option_env!("MQ_DISCORD_APP_ID") {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    DISCORD_APP_ID.to_string()
}

#[cfg(windows)]
fn wait_for_named_pipe_data(
    file: &std::fs::File,
    required_bytes: usize,
    timeout: std::time::Duration,
) -> bool {
    use std::os::windows::io::AsRawHandle;
    type Handle = *mut std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn PeekNamedPipe(
            hNamedPipe: Handle,
            lpBuffer: *mut std::ffi::c_void,
            nBufferSize: u32,
            lpBytesRead: *mut u32,
            lpTotalBytesAvail: *mut u32,
            lpBytesLeftThisMessage: *mut u32,
        ) -> i32;
    }

    let raw_handle = file.as_raw_handle() as Handle;
    let start = std::time::Instant::now();
    let poll_interval = std::time::Duration::from_millis(10);

    while start.elapsed() <= timeout {
        let mut total_bytes_avail: u32 = 0;
        let res = unsafe {
            PeekNamedPipe(
                raw_handle,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut total_bytes_avail,
                std::ptr::null_mut(),
            )
        };
        if res == 0 {
            return false;
        }
        if (total_bytes_avail as usize) >= required_bytes {
            return true;
        }
        std::thread::sleep(poll_interval);
    }
    false
}

enum StreamKind {
    #[cfg(unix)]
    Unix(UnixStream),
    #[cfg(windows)]
    Windows(File),
}

impl StreamKind {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            #[cfg(unix)]
            Self::Unix(s) => s.write_all(buf),
            #[cfg(windows)]
            Self::Windows(f) => f.write_all(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            #[cfg(unix)]
            Self::Unix(s) => s.flush(),
            #[cfg(windows)]
            Self::Windows(f) => f.flush(),
        }
    }

    fn read_exact_with_timeout(
        &mut self,
        buf: &mut [u8],
        timeout: std::time::Duration,
    ) -> std::io::Result<()> {
        match self {
            #[cfg(unix)]
            Self::Unix(s) => {
                s.set_read_timeout(Some(timeout))?;
                s.read_exact(buf)
            }
            #[cfg(windows)]
            Self::Windows(f) => {
                if !wait_for_named_pipe_data(f, buf.len(), timeout) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "named pipe read timed out",
                    ));
                }
                f.read_exact(buf)
            }
        }
    }
}

#[cfg(unix)]
fn unix_ipc_candidate_paths(runtime_dir: Option<&str>, slot: u8) -> Vec<String> {
    let mut paths = Vec::with_capacity(4);
    if let Some(dir) = runtime_dir.map(str::trim).filter(|d| !d.is_empty()) {
        let d = dir.trim_end_matches('/');
        paths.push(format!("{d}/discord-ipc-{slot}"));
        paths.push(format!("{d}/app/com.discordapp.Discord/discord-ipc-{slot}"));
        paths.push(format!("{d}/snap.discord/discord-ipc-{slot}"));
    }
    paths.push(format!("/tmp/discord-ipc-{slot}"));
    paths
}

/// Lightweight Discord Rich Presence IPC v1 client.
pub struct DiscordRpcClient {
    stream: StreamKind,
    connected: bool,
}

impl DiscordRpcClient {
    /// Attempts to connect to an active Discord client via standard IPC endpoints (0..10).
    pub fn connect_default() -> Option<Self> {
        #[cfg(unix)]
        {
            let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
                .ok()
                .filter(|d| !d.trim().is_empty());
            for i in 0..10u8 {
                for path in unix_ipc_candidate_paths(runtime_dir.as_deref(), i) {
                    if let Ok(stream) = UnixStream::connect(&path) {
                        if let Some(client) = Self::connect_unix_stream(stream) {
                            return Some(client);
                        }
                    }
                }
            }
        }

        #[cfg(windows)]
        {
            for i in 0..10 {
                let pipe_path = format!(r"\\.\pipe\discord-ipc-{i}");
                if let Ok(file) = OpenOptions::new().read(true).write(true).open(&pipe_path) {
                    let mut client = Self {
                        stream: StreamKind::Windows(file),
                        connected: true,
                    };
                    if client.send_handshake() {
                        return Some(client);
                    }
                    client.connected = false;
                }
            }
        }

        None
    }

    #[cfg(unix)]
    fn connect_unix_stream(stream: UnixStream) -> Option<Self> {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
        let mut client = Self {
            stream: StreamKind::Unix(stream),
            connected: true,
        };
        if client.send_handshake() {
            Some(client)
        } else {
            client.connected = false;
            None
        }
    }

    fn send_frame(&mut self, opcode: u32, payload: &str) -> bool {
        if !self.connected {
            return false;
        }
        let len = payload.len() as u32;
        let mut msg = Vec::with_capacity(8 + payload.len());
        msg.extend_from_slice(&opcode.to_le_bytes());
        msg.extend_from_slice(&len.to_le_bytes());
        msg.extend_from_slice(payload.as_bytes());

        if self.stream.write_all(&msg).is_err() || self.stream.flush().is_err() {
            self.connected = false;
            return false;
        }
        true
    }

    fn send_handshake(&mut self) -> bool {
        let app_id = get_discord_app_id();
        let handshake_val = serde_json::json!({
            "v": 1,
            "client_id": app_id,
        });
        let handshake = handshake_val.to_string();
        if !self.send_frame(0, &handshake) {
            return false;
        }

        let mut header = [0u8; 8];
        let timeout = std::time::Duration::from_millis(200);
        if self
            .stream
            .read_exact_with_timeout(&mut header, timeout)
            .is_err()
        {
            self.connected = false;
            return false;
        }

        let opcode = u32::from_le_bytes(header[0..4].try_into().unwrap());
        // In Discord IPC protocol, response opcode must be FRAME (1)
        if opcode != 1 {
            self.connected = false;
            return false;
        }

        let body_len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        // Cap payload read at 64 KiB
        if body_len > 64 * 1024 {
            self.connected = false;
            return false;
        }

        let mut body = vec![0u8; body_len];
        let ok = self
            .stream
            .read_exact_with_timeout(&mut body, timeout)
            .is_ok();
        if !ok {
            self.connected = false;
        }
        ok
    }

    /// Sets playing activity status in Discord.
    pub fn set_playing_activity(
        &mut self,
        instance_name: &str,
        details_line: &str,
        start_unix_secs: u64,
    ) {
        let pid = std::process::id();
        let payload_val = serde_json::json!({
            "cmd": "SET_ACTIVITY",
            "args": {
                "pid": pid,
                "activity": {
                    "state": instance_name,
                    "details": details_line,
                    "timestamps": {
                        "start": start_unix_secs,
                    },
                    "assets": {
                        "large_image": "mq",
                        "large_text": "MQ Launcher",
                    }
                }
            },
            "nonce": start_unix_secs.to_string(),
        });
        let payload = payload_val.to_string();
        let _ = self.send_frame(1, &payload);
    }

    /// Clears any currently shown Discord activity.
    pub fn clear_activity(&mut self) {
        let pid = std::process::id();
        let payload_val = serde_json::json!({
            "cmd": "SET_ACTIVITY",
            "args": {
                "pid": pid,
                "activity": serde_json::Value::Null,
            },
            "nonce": "1",
        });
        let payload = payload_val.to_string();
        let _ = self.send_frame(1, &payload);
    }
}

impl Drop for DiscordRpcClient {
    fn drop(&mut self) {
        self.clear_activity();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_default_fails_gracefully_when_no_discord() {
        let client = DiscordRpcClient::connect_default();
        // On headless test machines without Discord running, must return None cleanly without panic.
        if let Some(mut c) = client {
            c.set_playing_activity("Test Instance", "Playing Minecraft", 1000);
            c.clear_activity();
        }
    }

    #[test]
    fn test_discord_app_id_override() {
        std::env::remove_var("MQ_DISCORD_APP_ID");
        let expected = option_env!("MQ_DISCORD_APP_ID")
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(DISCORD_APP_ID);
        assert_eq!(get_discord_app_id(), expected);

        std::env::set_var("MQ_DISCORD_APP_ID", "999888777666555444");
        assert_eq!(get_discord_app_id(), "999888777666555444");
        std::env::remove_var("MQ_DISCORD_APP_ID");
    }

    #[cfg(unix)]
    #[test]
    fn unix_ipc_paths_include_flatpak_and_snap_and_failed_handshake_clears_connected() {
        use std::os::unix::net::UnixListener;

        let paths = unix_ipc_candidate_paths(Some("/run/user/1000"), 3);
        assert_eq!(
            paths,
            vec![
                "/run/user/1000/discord-ipc-3",
                "/run/user/1000/app/com.discordapp.Discord/discord-ipc-3",
                "/run/user/1000/snap.discord/discord-ipc-3",
                "/tmp/discord-ipc-3",
            ]
        );

        let tmp = tempfile::tempdir().unwrap();
        let sock_path = tmp.path().join("discord-ipc-0");
        let listener = UnixListener::bind(&sock_path).unwrap();
        let srv = std::thread::spawn(move || {
            if let Ok((mut conn, _)) = listener.accept() {
                // Reply with invalid opcode (0 instead of 1) to force handshake failure.
                let _ = conn.write_all(&[0, 0, 0, 0, 0, 0, 0, 0]);
            }
        });

        let stream = UnixStream::connect(&sock_path).unwrap();
        assert!(DiscordRpcClient::connect_unix_stream(stream).is_none());
        let _ = srv.join();
    }
}
