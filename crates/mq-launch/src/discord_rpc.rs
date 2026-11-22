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

const DISCORD_APP_ID: &str = "1342500000000000000";

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

    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            #[cfg(unix)]
            Self::Unix(s) => s.read(buf),
            #[cfg(windows)]
            Self::Windows(f) => f.read(buf),
        }
    }
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
            let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok();
            for i in 0..10 {
                let candidate_paths = [
                    runtime_dir.as_ref().map(|d| format!("{d}/discord-ipc-{i}")),
                    Some(format!("/tmp/discord-ipc-{i}")),
                ];
                for path_opt in candidate_paths.into_iter().flatten() {
                    if let Ok(stream) = UnixStream::connect(&path_opt) {
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                        let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
                        let mut client = Self {
                            stream: StreamKind::Unix(stream),
                            connected: true,
                        };
                        if client.send_handshake() {
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
                }
            }
        }

        None
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
        let handshake = format!(r#"{{"v":1,"client_id":"{DISCORD_APP_ID}"}}"#);
        if !self.send_frame(0, &handshake) {
            return false;
        }
        let mut header = [0u8; 8];
        let Ok(n) = self.stream.read(&mut header) else {
            return false;
        };
        n >= 8
    }

    /// Sets playing activity status in Discord.
    pub fn set_playing_activity(
        &mut self,
        instance_name: &str,
        details_line: &str,
        start_unix_secs: u64,
    ) {
        let pid = std::process::id();
        let safe_name = instance_name.replace('"', "\\\"");
        let safe_details = details_line.replace('"', "\\\"");
        let payload = format!(
            r#"{{"cmd":"SET_ACTIVITY","args":{{"pid":{pid},"activity":{{"state":"{safe_name}","details":"{safe_details}","timestamps":{{"start":{start_unix_secs}}},"assets":{{"large_image":"mq","large_text":"MQ Launcher"}}}}}},"nonce":"{start_unix_secs}"}}"#
        );
        let _ = self.send_frame(1, &payload);
    }

    /// Clears any currently shown Discord activity.
    pub fn clear_activity(&mut self) {
        let pid = std::process::id();
        let payload = format!(
            r#"{{"cmd":"SET_ACTIVITY","args":{{"pid":{pid},"activity":null}},"nonce":"1"}}"#
        );
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
}
