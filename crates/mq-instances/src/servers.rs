//! Minecraft `servers.dat` parser, editor, and live Server List Ping client.

use std::io::{Read, Write};
use std::net::ToSocketAddrs;
use std::path::Path;
use std::time::{Duration, Instant};

use mq_core::{MqError, MqResult};
use serde::{Deserialize, Serialize};

use crate::nbt::{parse_nbt_root, read_nbt_file_bounded};

/// Maximum decompressed size accepted for `servers.dat` (16 MiB).
const MAX_SERVERS_DAT_BYTES: usize = 16 * 1024 * 1024;

/// Stored multiplayer server entry in `servers.dat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerEntry {
    /// Human-readable display name.
    pub name: String,
    /// Hostname or IP address with optional port (e.g. `mc.hypixel.net` or `127.0.0.1:25565`).
    pub ip: String,
    /// Optional base64-encoded PNG favicon data URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Whether custom server resource packs are accepted (0 = prompt, 1 = enabled, 2 = disabled).
    #[serde(default = "default_accept_textures")]
    pub accept_textures: u8,
}

fn default_accept_textures() -> u8 {
    1
}

/// Live status result returned from Minecraft Server List Ping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerPingResult {
    /// Round-trip ping latency in milliseconds.
    pub latency_ms: u64,
    /// Server version string (e.g. `"1.21.1"` or `"Paper 1.20.4"`).
    pub version_name: String,
    /// Current number of online players.
    pub players_online: u32,
    /// Maximum player capacity.
    pub players_max: u32,
    /// Message of the day (MOTD) plain text.
    pub motd: String,
    /// Optional base64 favicon data URL.
    pub favicon: Option<String>,
    /// Decoded raw PNG bytes from base64 favicon data URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favicon_png: Option<Vec<u8>>,
    /// Sample of online player names.
    #[serde(default)]
    pub players_sample: Vec<String>,
    /// Segmented MOTD with optional color formatting code (`§`).
    #[serde(default)]
    pub motd_segments: Vec<(Option<char>, String)>,
}

/// Parses a Minecraft MOTD string into segments of `(Option<color_code>, text)`.
/// Preserves active color code (`0-9`, `a-f`) when style formatting codes (`k-o`) are encountered.
pub fn parse_motd_segments(motd: &str) -> Vec<(Option<char>, String)> {
    let mut segments = Vec::new();
    let mut current_code = None;
    let mut current_text = String::new();

    let mut chars = motd.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '§' {
            if let Some(&code) = chars.peek() {
                chars.next();
                let lower_code = code.to_ascii_lowercase();
                if lower_code == 'r' {
                    if !current_text.is_empty() {
                        segments.push((current_code, std::mem::take(&mut current_text)));
                    }
                    current_code = None;
                } else if matches!(lower_code, '0'..='9' | 'a'..='f')
                    || (matches!(lower_code, 'k'..='o') && current_code.is_none())
                {
                    if !current_text.is_empty() {
                        segments.push((current_code, std::mem::take(&mut current_text)));
                    }
                    current_code = Some(lower_code);
                }
            }
        } else {
            current_text.push(c);
        }
    }

    if !current_text.is_empty() || segments.is_empty() {
        segments.push((current_code, current_text));
    }

    segments
}

/// Loads all multiplayer servers declared in `<instance_dir>/servers.dat`.
pub fn load_servers_dat(instance_dir: &Path) -> MqResult<Vec<ServerEntry>> {
    let dat_path = instance_dir.join("servers.dat");
    if !dat_path.exists() {
        return Ok(Vec::new());
    }

    let bytes = read_nbt_file_bounded(&dat_path, MAX_SERVERS_DAT_BYTES)?;
    if bytes.is_empty() {
        return Ok(Vec::new());
    }

    parse_servers_nbt(&bytes)
}

/// Saves the list of servers to `<instance_dir>/servers.dat` in standard uncompressed NBT format.
pub fn save_servers_dat(instance_dir: &Path, servers: &[ServerEntry]) -> MqResult<()> {
    let dat_path = instance_dir.join("servers.dat");
    let mut buf = Vec::new();

    // TAG_Compound (0x0A) + empty name (0x00, 0x00)
    buf.push(0x0A);
    buf.extend_from_slice(&[0x00, 0x00]);

    // TAG_List (0x09) + name "servers"
    buf.push(0x09);
    write_nbt_string_checked(&mut buf, "servers")?;
    // List item type: TAG_Compound (0x0A)
    buf.push(0x0A);
    let list_len: i32 = i32::try_from(servers.len())
        .map_err(|_| MqError::Metadata("too many servers for servers.dat".to_string()))?;
    buf.extend_from_slice(&list_len.to_be_bytes());

    for s in servers {
        // TAG_String "name"
        buf.push(0x08);
        write_nbt_string_checked(&mut buf, "name")?;
        write_nbt_string_checked(&mut buf, &s.name)?;

        // TAG_String "ip"
        buf.push(0x08);
        write_nbt_string_checked(&mut buf, "ip")?;
        write_nbt_string_checked(&mut buf, &s.ip)?;

        // TAG_Byte "acceptTextures"
        buf.push(0x01);
        write_nbt_string_checked(&mut buf, "acceptTextures")?;
        buf.push(s.accept_textures);

        if let Some(ref icon) = s.icon {
            // Guard string lengths against > u16::MAX: skip oversized icons
            if u16::try_from(icon.len()).is_ok() {
                buf.push(0x08);
                write_nbt_string_checked(&mut buf, "icon")?;
                write_nbt_string_checked(&mut buf, icon)?;
            } else {
                tracing::warn!("skipping oversized server icon exceeding u16::MAX bytes");
            }
        }

        // TAG_End (0x00)
        buf.push(0x00);
    }

    // TAG_End for root compound
    buf.push(0x00);

    mq_core::platform::atomic_write(&dat_path, &buf)?;
    Ok(())
}

fn write_nbt_string_checked(buf: &mut Vec<u8>, s: &str) -> MqResult<()> {
    let bytes = s.as_bytes();
    let len = u16::try_from(bytes.len()).map_err(|_| {
        MqError::Metadata(format!(
            "string exceeds max NBT length of 65535: len={}",
            bytes.len()
        ))
    })?;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(bytes);
    Ok(())
}

/// Robust binary NBT parser for `servers.dat` structures.
fn parse_servers_nbt(bytes: &[u8]) -> MqResult<Vec<ServerEntry>> {
    let (_root_name, root_val) = parse_nbt_root(bytes)?;
    let servers_list = match root_val.get("servers").and_then(|v| v.as_list()) {
        Some(list) => list,
        None => return Ok(Vec::new()),
    };

    let mut servers = Vec::with_capacity(servers_list.len());
    for item in servers_list {
        let ip = match item.get("ip").and_then(|v| v.as_str()) {
            Some(ip_str) if !ip_str.is_empty() => ip_str.to_string(),
            _ => continue,
        };

        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&ip)
            .to_string();

        let icon = item
            .get("icon")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);

        let accept_textures = item
            .get("acceptTextures")
            .and_then(|v| v.as_u8())
            .unwrap_or(1);

        servers.push(ServerEntry {
            name,
            ip,
            icon,
            accept_textures,
        });
    }

    Ok(servers)
}

/// Parses server address into `(host, port)` supporting `[ipv6]:port`, raw IPv6, and `host:port`.
fn parse_server_addr(server_addr: &str) -> (&str, u16) {
    let s = server_addr.trim();
    if s.starts_with('[') {
        if let Some(close_bracket) = s.find(']') {
            let host = &s[1..close_bracket];
            let rest = &s[close_bracket + 1..];
            let port = if let Some(p) = rest.strip_prefix(':') {
                p.trim().parse::<u16>().unwrap_or(25565)
            } else {
                25565
            };
            (host.trim(), port)
        } else {
            (s, 25565)
        }
    } else if let Some((h, p)) = s.rsplit_once(':') {
        if h.contains(':') {
            // Raw IPv6 address without port
            (s, 25565)
        } else {
            let port = p.trim().parse::<u16>().unwrap_or(25565);
            (h.trim(), port)
        }
    } else {
        (s, 25565)
    }
}

/// Recursively extracts plain text from a Minecraft JSON text component.
fn extract_component_text(val: &serde_json::Value, out: &mut String) {
    match val {
        serde_json::Value::String(s) => out.push_str(s),
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(text)) = map.get("text") {
                out.push_str(text);
            }
            if let Some(extra) = map.get("extra") {
                extract_component_text(extra, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                extract_component_text(item, out);
            }
        }
        _ => {}
    }
}

/// Live Minecraft TCP Server List Ping implementation.
pub fn ping_server(server_addr: &str, timeout_ms: u64) -> MqResult<ServerPingResult> {
    let (host, port) = parse_server_addr(server_addr);

    let start = Instant::now();
    let sock_addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| MqError::Network(format!("DNS resolution failed for {host}:{port}: {e}")))?
        .collect();

    if sock_addrs.is_empty() {
        return Err(MqError::Network(format!(
            "no IP addresses resolved for {host}:{port}"
        )));
    }

    let timeout = Duration::from_millis(timeout_ms);
    let mut stream = std::net::TcpStream::connect_timeout(&sock_addrs[0], timeout)
        .map_err(|e| MqError::Network(format!("connection timed out or refused: {e}")))?;

    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();

    // 1. Handshake Packet (ID 0x00)
    let mut handshake_data = Vec::new();
    write_varint(&mut handshake_data, 0x00); // Packet ID
    write_varint(&mut handshake_data, 765); // Protocol version (1.20.4)
    write_string(&mut handshake_data, host);
    handshake_data.extend_from_slice(&port.to_be_bytes());
    write_varint(&mut handshake_data, 1); // Next state: status (1)

    let mut packet = Vec::new();
    write_varint(&mut packet, handshake_data.len() as i32);
    packet.extend_from_slice(&handshake_data);
    stream.write_all(&packet)?;

    // 2. Status Request Packet (ID 0x00)
    let req_packet = [0x01, 0x00];
    stream.write_all(&req_packet)?;
    stream.flush()?;

    // 3. Status Response
    let _packet_len = read_varint(&mut stream)?;
    let packet_id = read_varint(&mut stream)?;
    if packet_id != 0 {
        return Err(MqError::Network(format!(
            "unexpected response packet ID: {packet_id}"
        )));
    }

    let raw_json_len = read_varint(&mut stream)?;
    if !(1..=2 * 1024 * 1024).contains(&raw_json_len) {
        return Err(MqError::Network(format!(
            "invalid or oversized ping response length: {raw_json_len}"
        )));
    }

    let json_len = raw_json_len as usize;
    let mut json_buf = vec![0u8; json_len];
    stream.read_exact(&mut json_buf)?;

    let latency_ms = start.elapsed().as_millis() as u64;
    let json_str = String::from_utf8_lossy(&json_buf);

    let val: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| MqError::Internal(format!("failed parsing server ping response JSON: {e}")))?;

    let version_name = val
        .pointer("/version/name")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let players_online = val
        .pointer("/players/online")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let players_max = val
        .pointer("/players/max")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let motd = if let Some(desc) = val.get("description") {
        let mut text_buf = String::new();
        extract_component_text(desc, &mut text_buf);
        if text_buf.is_empty() {
            "A Minecraft Server".to_string()
        } else {
            text_buf
        }
    } else {
        "A Minecraft Server".to_string()
    };

    let favicon = val
        .get("favicon")
        .and_then(|f| f.as_str())
        .map(ToOwned::to_owned);

    let favicon_png = favicon.as_deref().and_then(|fav| {
        let b64 = fav.strip_prefix("data:image/png;base64,").unwrap_or(fav);
        use base64::Engine;
        base64::prelude::BASE64_STANDARD.decode(b64.trim()).ok()
    });

    let mut players_sample = Vec::new();
    if let Some(sample_arr) = val.pointer("/players/sample").and_then(|v| v.as_array()) {
        for p in sample_arr {
            if let Some(name) = p.get("name").and_then(|n| n.as_str()) {
                players_sample.push(name.to_string());
            }
        }
    }

    let motd_segments = parse_motd_segments(&motd);

    Ok(ServerPingResult {
        latency_ms,
        version_name,
        players_online,
        players_max,
        motd,
        favicon,
        favicon_png,
        players_sample,
        motd_segments,
    })
}

fn write_varint(w: &mut Vec<u8>, mut val: i32) {
    loop {
        if (val & !0x7F) == 0 {
            w.push(val as u8);
            return;
        }
        w.push(((val & 0x7F) | 0x80) as u8);
        val = ((val as u32) >> 7) as i32;
    }
}

fn write_string(w: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    write_varint(w, bytes.len() as i32);
    w.extend_from_slice(bytes);
}

fn read_varint<R: Read>(r: &mut R) -> MqResult<i32> {
    let mut num_read = 0;
    let mut result = 0;
    let mut byte = [0u8; 1];

    loop {
        r.read_exact(&mut byte)?;
        let val = byte[0] & 0x7F;
        result |= (val as i32) << (7 * num_read);
        num_read += 1;
        if num_read > 5 {
            return Err(MqError::Internal("VarInt is too big".to_string()));
        }
        if (byte[0] & 0x80) == 0 {
            break;
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_motd_segments() {
        let motd = "§aWelcome to §6§lMQ Server§r - Have Fun!";
        let segments = parse_motd_segments(motd);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0], (Some('a'), "Welcome to ".to_string()));
        assert_eq!(segments[1], (Some('6'), "MQ Server".to_string()));
        assert_eq!(segments[2], (None, " - Have Fun!".to_string()));
    }

    #[test]
    fn test_parse_server_addr() {
        assert_eq!(parse_server_addr("127.0.0.1:25565"), ("127.0.0.1", 25565));
        assert_eq!(
            parse_server_addr("mc.hypixel.net"),
            ("mc.hypixel.net", 25565)
        );
        assert_eq!(parse_server_addr("[::1]:25565"), ("::1", 25565));
        assert_eq!(parse_server_addr("[2001:db8::1]"), ("2001:db8::1", 25565));
        assert_eq!(parse_server_addr("2001:db8::1"), ("2001:db8::1", 25565));
    }

    #[test]
    fn test_servers_dat_roundtrip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let servers = vec![
            ServerEntry {
                name: "Hypixel".to_string(),
                ip: "mc.hypixel.net".to_string(),
                icon: Some("data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==".to_string()),
                accept_textures: 1,
            },
            ServerEntry {
                name: "Local".to_string(),
                ip: "127.0.0.1:25565".to_string(),
                icon: None,
                accept_textures: 0,
            },
        ];

        save_servers_dat(temp_dir.path(), &servers).unwrap();
        let loaded = load_servers_dat(temp_dir.path()).unwrap();
        assert_eq!(loaded, servers);
    }
}
