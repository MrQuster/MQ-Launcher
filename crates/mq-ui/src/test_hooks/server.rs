//! JSON-line IPC server for the `test-hooks` QA automation harness.
//!
//! - On Unix: binds a `0600` Unix domain socket in `MQ_DATA_DIR/test-hooks.sock`
//!   (or `MQ_TEST_HOOKS_SOCKET`).
//! - On non-Unix: binds `127.0.0.1:0` and writes a `0600` metadata file with
//!   port and bearer token in `MQ_DATA_DIR/test-hooks.addr`.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use mq_core::MqPaths;

use crate::test_hooks::protocol::{HarnessResponse, RequestEnvelope};

pub struct QueuedRequest {
    pub req: RequestEnvelope,
    pub reply_tx: mpsc::SyncSender<HarnessResponse>,
}

pub struct TestHooksServer {
    endpoint_file: PathBuf,
    shutdown: Arc<AtomicBool>,
}

impl TestHooksServer {
    pub fn start(
        ctx: egui::Context,
        paths: &MqPaths,
    ) -> anyhow::Result<(Self, mpsc::Receiver<QueuedRequest>)> {
        let (tx, rx) = mpsc::channel::<QueuedRequest>();
        let shutdown = Arc::new(AtomicBool::new(false));

        #[cfg(unix)]
        let endpoint_file = Self::start_unix(ctx, paths, tx, shutdown.clone())?;

        #[cfg(not(unix))]
        let endpoint_file = Self::start_tcp(ctx, paths, tx, shutdown.clone())?;

        Ok((
            Self {
                endpoint_file,
                shutdown,
            },
            rx,
        ))
    }

    pub fn cleanup(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = std::fs::remove_file(&self.endpoint_file);
    }

    #[cfg(unix)]
    fn start_unix(
        ctx: egui::Context,
        paths: &MqPaths,
        tx: mpsc::Sender<QueuedRequest>,
        shutdown: Arc<AtomicBool>,
    ) -> anyhow::Result<PathBuf> {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::{UnixListener, UnixStream};

        let sock_path = std::env::var_os("MQ_TEST_HOOKS_SOCKET")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| paths.data_dir.join("test-hooks.sock"));

        if let Some(parent) = sock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        if sock_path.exists() {
            if UnixStream::connect(&sock_path).is_ok() {
                anyhow::bail!(
                    "Another test-hooks instance is already listening on {}",
                    sock_path.display()
                );
            }
            let _ = std::fs::remove_file(&sock_path);
        }

        let listener = UnixListener::bind(&sock_path)?;
        std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;

        std::thread::Builder::new()
            .name("mq-test-hooks-ipc".to_string())
            .spawn(move || {
                while !shutdown.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _addr)) => {
                            let _ = stream.set_nonblocking(false);
                            let Ok(writer) = stream.try_clone() else {
                                continue;
                            };
                            handle_stream(stream, writer, &ctx, &tx, None);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            })?;

        Ok(sock_path)
    }

    #[cfg(not(unix))]
    fn start_tcp(
        ctx: egui::Context,
        paths: &MqPaths,
        tx: mpsc::Sender<QueuedRequest>,
        shutdown: Arc<AtomicBool>,
    ) -> anyhow::Result<PathBuf> {
        let addr_path = paths.data_dir.join("test-hooks.addr");
        if let Some(parent) = addr_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let token = uuid::Uuid::new_v4().to_string();
        let meta = serde_json::json!({ "port": port, "token": token });
        std::fs::write(&addr_path, serde_json::to_string(&meta)?)?;
        listener.set_nonblocking(true)?;

        std::thread::Builder::new()
            .name("mq-test-hooks-ipc".to_string())
            .spawn(move || {
                while !shutdown.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let Ok(writer) = stream.try_clone() else {
                                continue;
                            };
                            handle_stream(stream, writer, &ctx, &tx, Some(token.as_str()));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            })?;

        Ok(addr_path)
    }
}

impl Drop for TestHooksServer {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn handle_stream<R: Read, W: Write>(
    reader_stream: R,
    mut writer: W,
    ctx: &egui::Context,
    tx: &mpsc::Sender<QueuedRequest>,
    required_token: Option<&str>,
) {
    let mut reader = BufReader::new(reader_stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(expected) = required_token {
            let token_ok = serde_json::from_str::<serde_json::Value>(trimmed)
                .ok()
                .and_then(|v| {
                    v.get("token")
                        .and_then(|t| t.as_str())
                        .map(|s| s == expected)
                })
                .unwrap_or(false);
            if !token_ok {
                let resp = HarnessResponse::err(None, "Unauthorized: invalid or missing token");
                let _ = write_json_line(&mut writer, &resp);
                continue;
            }
        }

        let req = match serde_json::from_str::<RequestEnvelope>(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let id = serde_json::from_str::<serde_json::Value>(trimmed)
                    .ok()
                    .and_then(|v| {
                        v.get("req_id")
                            .or_else(|| v.get("request_id"))
                            .and_then(|i| i.as_u64())
                    });
                let resp = HarnessResponse::err(id, format!("Invalid request JSON: {e}"));
                let _ = write_json_line(&mut writer, &resp);
                continue;
            }
        };

        let req_id = req.req_id;
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        if tx.send(QueuedRequest { req, reply_tx }).is_err() {
            let resp = HarnessResponse::err(req_id, "UI loop terminated");
            let _ = write_json_line(&mut writer, &resp);
            break;
        }

        ctx.request_repaint();

        let resp = match reply_rx.recv_timeout(Duration::from_secs(30)) {
            Ok(r) => r,
            Err(_) => HarnessResponse::err(req_id, "Timed out waiting for UI frame execution"),
        };

        if write_json_line(&mut writer, &resp).is_err() {
            break;
        }
    }
}

fn write_json_line<W: Write>(writer: &mut W, resp: &HarnessResponse) -> std::io::Result<()> {
    let serialized = serde_json::to_string(resp).unwrap_or_else(|e| {
        format!(
            "{{\"ok\":false,\"error\":\"Serialization failure: {}\"}}",
            e
        )
    });
    writer.write_all(serialized.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}
