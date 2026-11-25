//! Minecraft world saves scanner, 1-click ZIP backup and restore, and export.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, TimeZone, Utc};
use mq_core::fs_util::{
    remove_dir_all_with_retry, remove_file_with_retry, rename_with_retry,
    validate_windows_safe_filename,
};
use mq_core::platform::sanitize_filename;
use mq_core::{resolve_safe_path, MqError, MqResult};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::nbt::{parse_nbt_root, read_nbt_file_bounded, NbtValue};

/// Maximum decompressed byte size allowed for `level.dat` (`32 MiB`).
const MAX_LEVEL_DAT_BYTES: usize = 32 * 1024 * 1024;
/// Maximum uncompressed size of a single entry when restoring a world backup (`4 GiB`).
const MAX_RESTORE_ENTRY_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Maximum cumulative uncompressed size when restoring a world backup (`16 GiB`).
const MAX_RESTORE_TOTAL_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Streaming I/O chunk size (`64 KiB`) for world restore extraction.
const RESTORE_STREAM_CHUNK_BYTES: usize = 64 * 1024;

static WORLD_RESTORE_SEQ: AtomicU64 = AtomicU64::new(1);

/// Information about a single Minecraft world save directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldInfo {
    /// Directory name under `<instance_dir>/saves/<folder_name>`.
    pub folder_name: String,
    /// Human-readable world name from `level.dat` or folder name fallback.
    pub display_name: String,
    /// Game mode string (e.g. "Выживание", "Творчество", "Хардкор").
    pub game_mode: String,
    /// Cumulative directory size in bytes.
    pub size_bytes: u64,
    /// Last played or modified UTC timestamp.
    pub last_played: Option<DateTime<Utc>>,
}

/// Metadata describing a single world backup ZIP archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldBackupInfo {
    /// Backup file name (e.g. `MyWorld_2026-11-07_01-45-00.zip`).
    pub file_name: String,
    /// Absolute path to the ZIP backup archive.
    pub path: PathBuf,
    /// File size in bytes.
    pub size_bytes: u64,
    /// UTC timestamp of the backup.
    pub created_at: DateTime<Utc>,
}

fn next_staging_uuid_hex() -> String {
    let seq = WORLD_RESTORE_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id();
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:08x}{:04x}",
        (nanos >> 32) as u32,
        (nanos >> 16) as u16,
        (nanos & 0x0fff) as u16,
        (pid & 0x0fff) as u16,
        (seq >> 16) as u32,
        (seq & 0xffff) as u16
    )
}

fn resolve_single_child_safe_path(base: &Path, name: &str) -> MqResult<PathBuf> {
    if name.contains('/') || name.contains('\\') {
        return Err(MqError::ZipSlip {
            entry: name.to_owned(),
        });
    }
    resolve_safe_path(base, name)
}

fn strip_zip_extension(file_name: &str) -> &str {
    if file_name.len() >= 4
        && file_name
            .get(file_name.len() - 4..)
            .is_some_and(|ext| ext.eq_ignore_ascii_case(".zip"))
    {
        &file_name[..file_name.len() - 4]
    } else {
        file_name
    }
}

/// Returns `true` if `ts` matches a 4-digit `20xx` backup timestamp in either
/// `YYYY-MM-DD_HH-MM-SS` (`19` chars) or `YYYYMMDD-HHMMSS` (`15` chars) format.
fn is_backup_timestamp_suffix(ts: &str) -> bool {
    let b = ts.as_bytes();
    if b.len() == 15 {
        return b[0] == b'2'
            && b[1] == b'0'
            && b[..8].iter().all(u8::is_ascii_digit)
            && matches!(b[8], b'-' | b'_')
            && b[9..15].iter().all(u8::is_ascii_digit);
    }
    if b.len() == 19 {
        return b[0] == b'2'
            && b[1] == b'0'
            && b[..4].iter().all(u8::is_ascii_digit)
            && b[4] == b'-'
            && b[5..7].iter().all(u8::is_ascii_digit)
            && b[7] == b'-'
            && b[8..10].iter().all(u8::is_ascii_digit)
            && matches!(b[10], b'_' | b'-')
            && b[11..13].iter().all(u8::is_ascii_digit)
            && b[13] == b'-'
            && b[14..16].iter().all(u8::is_ascii_digit)
            && b[16] == b'-'
            && b[17..19].iter().all(u8::is_ascii_digit);
    }
    false
}

/// Strips a trailing `_YYYY-MM-DD_HH-MM-SS` or `-YYYYMMDD-HHMMSS` timestamp suffix from a backup
/// `.zip` stem while preserving underscores and hyphens inside the world's actual name.
pub(crate) fn strip_backup_timestamp_suffix(stem: &str) -> &str {
    for ts_len in [19usize, 15usize] {
        let suffix_total = ts_len + 1;
        if stem.len() > suffix_total {
            let split_idx = stem.len() - suffix_total;
            if stem.is_char_boundary(split_idx) && stem.is_char_boundary(split_idx + 1) {
                let sep = stem.as_bytes()[split_idx];
                let ts_part = &stem[split_idx + 1..];
                if matches!(sep, b'_' | b'-') && is_backup_timestamp_suffix(ts_part) {
                    return &stem[..split_idx];
                }
            }
        }
    }
    stem
}

fn is_matching_world_backup_file(file_name: &str, safe_world: &str) -> bool {
    if file_name.len() < 4
        || !file_name
            .get(file_name.len() - 4..)
            .is_some_and(|ext| ext.eq_ignore_ascii_case(".zip"))
    {
        return false;
    }
    let stem = &file_name[..file_name.len() - 4];
    if !stem.starts_with(safe_world) || stem.len() <= safe_world.len() + 1 {
        return false;
    }
    let sep = stem.as_bytes()[safe_world.len()];
    if !matches!(sep, b'-' | b'_') {
        return false;
    }
    let rest = &stem[safe_world.len() + 1..];
    is_backup_timestamp_suffix(rest)
}

/// Lists all world saves inside `<instance_dir>/saves/`.
pub fn list_worlds(instance_dir: &Path) -> MqResult<Vec<WorldInfo>> {
    let saves_dir = instance_dir.join("saves");
    if !saves_dir.exists() {
        return Ok(Vec::new());
    }

    let mut worlds = Vec::new();
    let entries = fs::read_dir(&saves_dir)?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let folder_name = entry.file_name().to_string_lossy().into_owned();
        if folder_name.starts_with(".mq-") {
            continue;
        }
        let level_dat = path.join("level.dat");

        let mut display_name = folder_name.clone();
        let mut game_mode = "Выживание".to_string();
        let mut last_played = None;

        if level_dat.is_file() {
            if let Ok(meta) = fs::metadata(&level_dat) {
                if let Ok(mtime) = meta.modified() {
                    let dt: DateTime<Utc> = mtime.into();
                    last_played = Some(dt);
                }
            }
            if let Ok((name, mode, lp)) = parse_level_dat(&level_dat) {
                if !name.is_empty() {
                    display_name = name;
                }
                game_mode = mode;
                if let Some(ts) = lp {
                    last_played = Some(ts);
                }
            }
        } else if let Ok(meta) = fs::metadata(&path) {
            if let Ok(mtime) = meta.modified() {
                let dt: DateTime<Utc> = mtime.into();
                last_played = Some(dt);
            }
        }

        let size_bytes = compute_dir_size(&path);

        worlds.push(WorldInfo {
            folder_name,
            display_name,
            game_mode,
            size_bytes,
            last_played,
        });
    }

    worlds.sort_by_key(|a| std::cmp::Reverse(a.last_played));
    Ok(worlds)
}

/// Computes cumulative directory size in bytes.
pub fn compute_dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_file() {
                total = total.saturating_add(meta.len());
            } else if meta.is_dir() {
                total = total.saturating_add(compute_dir_size(&path));
            }
        }
    }
    total
}

#[derive(Debug, Default)]
struct ParsedLevelDatFields {
    level_name: String,
    game_type: Option<i32>,
    hardcore: bool,
    last_played: Option<DateTime<Utc>>,
    seed: Option<i64>,
    spawn_xyz: Option<(i32, i32, i32)>,
    version_name: Option<String>,
}

fn parse_level_dat_fields(path: &Path) -> MqResult<ParsedLevelDatFields> {
    let bytes = read_nbt_file_bounded(path, MAX_LEVEL_DAT_BYTES)?;
    if bytes.is_empty() {
        return Err(MqError::Metadata("level.dat is empty".to_owned()));
    }

    let (_root_name, root_val) = parse_nbt_root(&bytes)?;
    let data = root_val.get("Data").unwrap_or(&root_val);

    let level_name = data
        .get("LevelName")
        .and_then(NbtValue::as_str)
        .unwrap_or_default()
        .to_owned();

    let game_type = data.get("GameType").and_then(NbtValue::as_i32);
    let hardcore = data
        .get("hardcore")
        .and_then(NbtValue::as_bool)
        .unwrap_or(false);

    let last_played = data
        .get("LastPlayed")
        .and_then(NbtValue::as_i64)
        .filter(|&millis| millis > 0)
        .and_then(|millis| Utc.timestamp_millis_opt(millis).single());

    let seed = data
        .get("RandomSeed")
        .and_then(NbtValue::as_i64)
        .or_else(|| {
            data.get("WorldGenSettings")
                .and_then(|wgs| wgs.get("seed"))
                .and_then(NbtValue::as_i64)
        });

    let legacy_spawn = match (
        data.get("SpawnX").and_then(NbtValue::as_i32),
        data.get("SpawnY").and_then(NbtValue::as_i32),
        data.get("SpawnZ").and_then(NbtValue::as_i32),
    ) {
        (Some(x), Some(y), Some(z)) => Some((x, y, z)),
        _ => None,
    };

    let spawn_xyz = legacy_spawn.or_else(|| {
        let pos = data.get("spawn")?.get("pos")?;
        if let Some(arr) = pos.as_int_array() {
            if arr.len() >= 3 {
                return Some((arr[0], arr[1], arr[2]));
            }
        }
        if let Some(list) = pos.as_list() {
            if list.len() >= 3 {
                if let (Some(x), Some(y), Some(z)) =
                    (list[0].as_i32(), list[1].as_i32(), list[2].as_i32())
                {
                    return Some((x, y, z));
                }
            }
        }
        None
    });

    let version_name = data
        .get("Version")
        .and_then(|ver| ver.get("Name"))
        .and_then(NbtValue::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    Ok(ParsedLevelDatFields {
        level_name,
        game_type,
        hardcore,
        last_played,
        seed,
        spawn_xyz,
        version_name,
    })
}

/// Parses basic metadata from `level.dat` using bounded Gzip decompression and binary NBT parsing.
fn parse_level_dat(path: &Path) -> MqResult<(String, String, Option<DateTime<Utc>>)> {
    let fields = parse_level_dat_fields(path)?;
    let game_mode = if fields.hardcore {
        "Хардкор".to_string()
    } else {
        match fields.game_type.unwrap_or(0) {
            0 => "Выживание".to_string(),
            1 => "Творчество".to_string(),
            2 => "Приключение".to_string(),
            3 => "Наблюдатель".to_string(),
            _ => "Выживание".to_string(),
        }
    };
    Ok((fields.level_name, game_mode, fields.last_played))
}

/// Creates a 1-click ZIP backup of the specified world save directory under
/// `<backups_root>/<instance_id>/<world_folder>_<timestamp>.zip`.
pub fn backup_world(
    instance_dir: &Path,
    backups_root: &Path,
    instance_id: &str,
    world_folder: &str,
) -> MqResult<PathBuf> {
    let saves_dir = instance_dir.join("saves");
    let source_dir = resolve_single_child_safe_path(&saves_dir, world_folder)?;
    if !source_dir.is_dir() {
        return Err(MqError::Internal(format!(
            "world directory not found: {}",
            source_dir.display()
        )));
    }

    let target_dir = resolve_single_child_safe_path(backups_root, instance_id)?;
    fs::create_dir_all(&target_dir)?;

    let timestamp = Utc::now().format("%Y-%m-%d_%H-%M-%S");
    let safe_world = sanitize_filename(world_folder);
    let zip_name = format!("{safe_world}_{timestamp}.zip");
    let zip_path = resolve_single_child_safe_path(&target_dir, &zip_name)?;

    export_world_to_zip(instance_dir, world_folder, &zip_path)?;
    Ok(zip_path)
}

/// Lists all backups for a given instance.
pub fn list_backups(backups_root: &Path, instance_id: &str) -> MqResult<Vec<WorldBackupInfo>> {
    let target_dir = resolve_single_child_safe_path(backups_root, instance_id)?;
    if !target_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut backups = Vec::new();
    for entry in fs::read_dir(&target_dir)?.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
        {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let meta = entry.metadata()?;
            let size_bytes = meta.len();
            let created_at: DateTime<Utc> =
                meta.modified().unwrap_or_else(|_| SystemTime::now()).into();

            backups.push(WorldBackupInfo {
                file_name,
                path,
                size_bytes,
                created_at,
            });
        }
    }

    backups.sort_by_key(|a| std::cmp::Reverse(a.created_at));
    Ok(backups)
}

/// Restores a world save from a backup ZIP archive into `<instance_dir>/saves/<world_folder>`.
///
/// Extracts into a sibling staging directory `saves/.mq-restore-<uuid>` first with bounded stream
/// sizes (`4 GiB` per entry, `16 GiB` total), then atomically swaps any existing destination via
/// `saves/.mq-backup-prev-<uuid>` with automatic rollback on failure so world data is never lost.
pub fn restore_world(
    backups_root: &Path,
    instance_id: &str,
    backup_file_name: &str,
    instance_dir: &Path,
    target_folder_name: Option<&str>,
) -> MqResult<PathBuf> {
    let inst_backup_dir = resolve_single_child_safe_path(backups_root, instance_id)?;
    let backup_path = resolve_safe_path(&inst_backup_dir, backup_file_name)?;
    if !backup_path.is_file() {
        return Err(MqError::Internal(format!(
            "backup archive not found: {}",
            backup_path.display()
        )));
    }

    let saves_dir = instance_dir.join("saves");
    fs::create_dir_all(&saves_dir)?;

    let resolved_folder = match target_folder_name {
        Some(raw_target) => {
            let _ = resolve_single_child_safe_path(&saves_dir, raw_target)?;
            raw_target.to_owned()
        }
        None => {
            let base_file = Path::new(backup_file_name)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(backup_file_name);
            let stem = strip_zip_extension(base_file);
            let stripped = strip_backup_timestamp_suffix(stem).trim();
            if stripped.is_empty() {
                "RestoredWorld".to_owned()
            } else {
                stripped.to_owned()
            }
        }
    };

    let safe_folder = if validate_windows_safe_filename(&resolved_folder) {
        resolved_folder
    } else {
        sanitize_filename(&resolved_folder)
    };
    let dest_dir = resolve_single_child_safe_path(&saves_dir, &safe_folder)?;

    let file = File::open(&backup_path)?;
    let mut archive = ZipArchive::new(file)
        .map_err(|e| MqError::Archive(format!("failed to open backup ZIP archive: {e}")))?;

    let uid = next_staging_uuid_hex();
    let staging_dir = saves_dir.join(format!(".mq-restore-{uid}"));
    let prev_backup_dir = saves_dir.join(format!(".mq-backup-prev-{uid}"));

    // Pre-validate every entry against Zip-Slip, Windows drive prefixes, NUL bytes, symlinks,
    // and uncompressed size limits before creating or modifying any directory.
    let mut declared_total = 0u64;
    for i in 0..archive.len() {
        let entry = archive
            .by_index_raw(i)
            .map_err(|e| MqError::Archive(format!("corrupted entry in backup ZIP: {e}")))?;
        let raw_name = entry.name();
        if entry.is_symlink()
            || entry
                .unix_mode()
                .is_some_and(|m| (m & 0o170000) == 0o120000)
        {
            return Err(MqError::ZipSlip {
                entry: raw_name.to_owned(),
            });
        }
        let has_windows_drive = raw_name.len() >= 2
            && raw_name.as_bytes()[0].is_ascii_alphabetic()
            && raw_name.as_bytes()[1] == b':';
        if raw_name.contains('\0')
            || has_windows_drive
            || raw_name.starts_with('/')
            || raw_name.starts_with('\\')
        {
            return Err(MqError::ZipSlip {
                entry: raw_name.to_owned(),
            });
        }
        let normalized = raw_name.replace('\\', "/");
        if normalized.split('/').any(|seg| seg == ".." || seg == ".") {
            return Err(MqError::ZipSlip {
                entry: raw_name.to_owned(),
            });
        }
        let uncomp = entry.size();
        if uncomp > MAX_RESTORE_ENTRY_BYTES {
            return Err(MqError::Archive(format!(
                "backup entry '{raw_name}' uncompressed size {uncomp} exceeds 4 GiB limit"
            )));
        }
        declared_total = declared_total.saturating_add(uncomp);
        if declared_total > MAX_RESTORE_TOTAL_BYTES {
            return Err(MqError::Archive(format!(
                "backup archive total uncompressed size {declared_total} exceeds 16 GiB limit"
            )));
        }
        let trimmed = normalized.trim_end_matches('/');
        if !trimmed.is_empty() {
            let _ = resolve_safe_path(&dest_dir, trimmed)?;
        }
    }

    if staging_dir.exists() {
        let _ = remove_dir_all_with_retry(&staging_dir);
    }
    fs::create_dir_all(&staging_dir)?;

    let extract_res = extract_archive_to_staging_bounded(
        &mut archive,
        &staging_dir,
        MAX_RESTORE_ENTRY_BYTES,
        MAX_RESTORE_TOTAL_BYTES,
    );
    if let Err(err) = extract_res {
        let _ = remove_dir_all_with_retry(&staging_dir);
        return Err(err);
    }

    atomic_swap_restored_world_dir(&staging_dir, &dest_dir, &prev_backup_dir, |src, dst| {
        rename_with_retry(src, dst)
    })?;

    Ok(dest_dir)
}

fn extract_archive_to_staging_bounded<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    staging_dir: &Path,
    max_entry_bytes: u64,
    max_total_bytes: u64,
) -> MqResult<()> {
    let mut buf = vec![0u8; RESTORE_STREAM_CHUNK_BYTES];
    let mut total_written = 0u64;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| MqError::Archive(format!("corrupted entry in backup ZIP: {e}")))?;

        if entry.is_symlink()
            || entry
                .unix_mode()
                .is_some_and(|m| (m & 0o170000) == 0o120000)
        {
            return Err(MqError::ZipSlip {
                entry: entry.name().to_owned(),
            });
        }

        let raw_name = entry.name().to_owned();
        let normalized = raw_name.replace('\\', "/");
        let trimmed = normalized.trim_end_matches('/');
        if trimmed.is_empty() {
            continue;
        }
        let out_path = resolve_safe_path(staging_dir, trimmed)?;
        if entry.is_dir() || raw_name.ends_with('/') || raw_name.ends_with('\\') {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out_file = File::create(&out_path)?;
            let mut entry_written = 0u64;
            loop {
                let n = entry
                    .read(&mut buf)
                    .map_err(|e| MqError::Archive(format!("failed reading '{raw_name}': {e}")))?;
                if n == 0 {
                    break;
                }
                entry_written = entry_written.saturating_add(n as u64);
                total_written = total_written.saturating_add(n as u64);
                if entry_written > max_entry_bytes {
                    return Err(MqError::Archive(format!(
                        "backup entry '{raw_name}' stream exceeded {max_entry_bytes} bytes"
                    )));
                }
                if total_written > max_total_bytes {
                    return Err(MqError::Archive(format!(
                        "backup archive stream exceeded {max_total_bytes} total bytes"
                    )));
                }
                out_file.write_all(&buf[..n])?;
            }
            out_file.flush()?;
        }
    }

    Ok(())
}

fn atomic_swap_restored_world_dir<F>(
    staging_dir: &Path,
    dest_dir: &Path,
    prev_backup_dir: &Path,
    mut rename_fn: F,
) -> MqResult<()>
where
    F: FnMut(&Path, &Path) -> std::io::Result<()>,
{
    if dest_dir.exists() {
        if prev_backup_dir.exists() {
            let _ = remove_dir_all_with_retry(prev_backup_dir);
        }
        if let Err(move_err) = rename_fn(dest_dir, prev_backup_dir) {
            let _ = remove_dir_all_with_retry(staging_dir);
            return Err(MqError::Io(move_err));
        }
        match rename_fn(staging_dir, dest_dir) {
            Ok(()) => {
                let _ = remove_dir_all_with_retry(prev_backup_dir);
                Ok(())
            }
            Err(swap_err) => {
                let _ = rename_with_retry(prev_backup_dir, dest_dir);
                let _ = remove_dir_all_with_retry(staging_dir);
                Err(MqError::Io(swap_err))
            }
        }
    } else {
        if let Err(swap_err) = rename_fn(staging_dir, dest_dir) {
            let _ = remove_dir_all_with_retry(staging_dir);
            return Err(MqError::Io(swap_err));
        }
        Ok(())
    }
}

/// Exports a world directory to a standalone ZIP archive.
pub fn export_world_to_zip(
    instance_dir: &Path,
    world_folder: &str,
    dest_zip_path: &Path,
) -> MqResult<()> {
    let saves_dir = instance_dir.join("saves");
    let src_dir = resolve_single_child_safe_path(&saves_dir, world_folder)?;
    if !src_dir.is_dir() {
        return Err(MqError::Internal(format!(
            "source world directory not found: {}",
            src_dir.display()
        )));
    }

    if let Some(p) = dest_zip_path.parent() {
        fs::create_dir_all(p)?;
    }

    let file = File::create(dest_zip_path)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    fn add_dir_to_zip<W: Write + std::io::Seek>(
        zip: &mut ZipWriter<W>,
        base_dir: &Path,
        current_dir: &Path,
        options: SimpleFileOptions,
    ) -> MqResult<()> {
        for entry in fs::read_dir(current_dir)?.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            let relative = path
                .strip_prefix(base_dir)
                .map_err(|e| MqError::Internal(format!("strip prefix failed: {e}")))?;
            let name_str = relative.to_string_lossy().replace('\\', "/");

            if meta.is_dir() {
                zip.add_directory(&name_str, options)
                    .map_err(|e| MqError::Internal(format!("failed adding dir to ZIP: {e}")))?;
                add_dir_to_zip(zip, base_dir, &path, options)?;
            } else if meta.is_file() {
                zip.start_file(&name_str, options)
                    .map_err(|e| MqError::Internal(format!("failed starting file in ZIP: {e}")))?;
                let mut f = File::open(&path)?;
                std::io::copy(&mut f, zip)?;
            }
        }
        Ok(())
    }

    add_dir_to_zip(&mut zip, &src_dir, &src_dir, options)?;
    zip.finish()
        .map_err(|e| MqError::Internal(format!("failed finalizing ZIP archive: {e}")))?;

    Ok(())
}

/// Deletes a world save folder from `<instance_dir>/saves/<world_folder>`.
pub fn delete_world(instance_dir: &Path, world_folder: &str) -> MqResult<()> {
    let saves_dir = instance_dir.join("saves");
    let world_dir = resolve_single_child_safe_path(&saves_dir, world_folder)?;
    if world_dir.exists() {
        remove_dir_all_with_retry(&world_dir)?;
    }
    Ok(())
}

/// Metadata parsed from Minecraft `level.dat`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorldNbtMeta {
    pub level_name: String,
    pub seed: Option<i64>,
    pub game_type: Option<i32>,
    pub hardcore: bool,
    pub spawn_xyz: Option<(i32, i32, i32)>,
    pub version_name: Option<String>,
}

/// Reads `level.dat` metadata from `world_dir/level.dat` using bounded Gzip decompression
/// (`<= 32 MiB`) and recursive binary NBT parsing.
pub fn read_world_level_dat(world_dir: &Path) -> Option<WorldNbtMeta> {
    let level_dat = world_dir.join("level.dat");
    if !level_dat.is_file() {
        return None;
    }

    let fields = parse_level_dat_fields(&level_dat).ok()?;
    let level_name = if fields.level_name.is_empty() {
        world_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("World")
            .to_owned()
    } else {
        fields.level_name
    };

    Some(WorldNbtMeta {
        level_name,
        seed: fields.seed,
        game_type: fields.game_type,
        hardcore: fields.hardcore,
        spawn_xyz: fields.spawn_xyz,
        version_name: fields.version_name,
    })
}

/// Entry representing a world datapack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatapackEntry {
    pub name: String,
    pub file_name: String,
    pub enabled: bool,
    pub is_dir: bool,
}

/// Lists all datapacks in `world_dir/datapacks`.
pub fn list_world_datapacks(world_dir: &Path) -> Vec<DatapackEntry> {
    let dp_dir = world_dir.join("datapacks");
    if !dp_dir.is_dir() {
        return Vec::new();
    }

    let mut list = Vec::new();
    if let Ok(entries) = fs::read_dir(&dp_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_dir = path.is_dir();
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let lower = file_name.to_ascii_lowercase();

            if is_dir || lower.ends_with(".zip") || lower.ends_with(".disabled") {
                let enabled = !lower.ends_with(".disabled");
                let base = if let Some(stripped) = file_name.strip_suffix(".disabled") {
                    stripped
                } else {
                    &file_name
                };
                let name = base.strip_suffix(".zip").unwrap_or(base).to_string();
                list.push(DatapackEntry {
                    name,
                    file_name,
                    enabled,
                    is_dir,
                });
            }
        }
    }
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

/// Toggles a world datapack between enabled and disabled (`.disabled` suffix).
pub fn toggle_world_datapack(world_dir: &Path, file_name: &str) -> MqResult<bool> {
    let dp_dir = world_dir.join("datapacks");
    let src = resolve_single_child_safe_path(&dp_dir, file_name)?;
    if !src.exists() {
        return Err(MqError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("datapack not found: {file_name}"),
        )));
    }

    if let Some(stripped) = file_name.strip_suffix(".disabled") {
        let dest = resolve_single_child_safe_path(&dp_dir, stripped)?;
        rename_with_retry(&src, &dest)?;
        Ok(true)
    } else {
        let disabled_name = format!("{file_name}.disabled");
        let dest = resolve_single_child_safe_path(&dp_dir, &disabled_name)?;
        rename_with_retry(&src, &dest)?;
        Ok(false)
    }
}

/// Adds a datapack (directory or `.zip` file) to `world_dir/datapacks`.
pub fn add_world_datapack(world_dir: &Path, src: &Path) -> MqResult<PathBuf> {
    if !src.exists() {
        return Err(MqError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("source datapack not found: {}", src.display()),
        )));
    }

    let dp_dir = world_dir.join("datapacks");
    fs::create_dir_all(&dp_dir)?;

    let file_name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| MqError::Internal("invalid source filename".into()))?;
    let dest = resolve_single_child_safe_path(&dp_dir, file_name)?;

    if src.is_dir() {
        copy_dir_recursive(src, &dest)?;
    } else {
        fs::copy(src, &dest)?;
    }

    Ok(dest)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let meta = fs::symlink_metadata(&from)?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let to = dst.join(entry.file_name());
        if meta.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if meta.is_file() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Deletes a world datapack by filename or directory name.
pub fn delete_world_datapack(world_dir: &Path, file_name: &str) -> MqResult<()> {
    let dp_dir = world_dir.join("datapacks");
    let target = resolve_single_child_safe_path(&dp_dir, file_name)?;
    if target.is_dir() {
        remove_dir_all_with_retry(&target)?;
    } else if target.is_file() {
        remove_file_with_retry(&target)?;
    }
    Ok(())
}

/// Automatically backs up all worlds inside `<instance_dir>/saves` to `<instance_dir>/backups/auto/<world>-<ts>.zip`.
/// Prunes older automatic backups exceeding `max_keep`.
pub fn auto_backup_instance_worlds(instance_dir: &Path, max_keep: usize) -> MqResult<Vec<PathBuf>> {
    let saves_dir = instance_dir.join("saves");
    if !saves_dir.is_dir() {
        return Ok(Vec::new());
    }

    let auto_dir = instance_dir.join("backups").join("auto");
    fs::create_dir_all(&auto_dir)?;

    let mut created = Vec::new();
    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");

    for entry in fs::read_dir(&saves_dir)?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let world_name = entry.file_name().to_string_lossy().into_owned();
        if world_name.starts_with(".mq-") {
            continue;
        }
        let safe_world = sanitize_filename(&world_name);
        let zip_name = format!("{safe_world}-{timestamp}.zip");
        let Ok(zip_path) = resolve_single_child_safe_path(&auto_dir, &zip_name) else {
            continue;
        };

        if export_world_to_zip(instance_dir, &world_name, &zip_path).is_ok() {
            created.push(zip_path);
        }

        let mut world_backups = Vec::new();
        if let Ok(entries) = fs::read_dir(&auto_dir) {
            for b_entry in entries.flatten() {
                let p = b_entry.path();
                if p.is_file() {
                    let fname = b_entry.file_name().to_string_lossy().into_owned();
                    if is_matching_world_backup_file(&fname, &safe_world) {
                        if let Ok(meta) = b_entry.metadata() {
                            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                            world_backups.push((p, mtime));
                        }
                    }
                }
            }
        }

        world_backups.sort_by_key(|a| std::cmp::Reverse(a.1));
        if world_backups.len() > max_keep {
            for (stale_path, _) in world_backups.iter().skip(max_keep) {
                let _ = remove_file_with_retry(stale_path);
            }
        }
    }

    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::{
        write_nbt_string_checked, TAG_BYTE, TAG_COMPOUND, TAG_END, TAG_INT, TAG_INT_ARRAY,
        TAG_LIST, TAG_LONG, TAG_STRING,
    };
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use tempfile::tempdir;

    #[test]
    fn test_datapacks_and_traversal_guards() {
        let temp = tempdir().expect("tempdir");
        let world_dir = temp.path().join("saves").join("NewWorld");
        fs::create_dir_all(&world_dir).expect("create world dir");

        let dp_file = temp.path().join("extra_pack.zip");
        fs::write(&dp_file, b"PK extra datapack zip").unwrap();

        let added = add_world_datapack(&world_dir, &dp_file).expect("add datapack");
        assert!(added.is_file());

        let list = list_world_datapacks(&world_dir);
        assert_eq!(list.len(), 1);
        assert!(list[0].enabled);
        assert_eq!(list[0].name, "extra_pack");

        let toggled = toggle_world_datapack(&world_dir, "extra_pack.zip").expect("toggle");
        assert!(!toggled);

        let list_disabled = list_world_datapacks(&world_dir);
        assert_eq!(list_disabled.len(), 1);
        assert!(!list_disabled[0].enabled);

        delete_world_datapack(&world_dir, "extra_pack.zip.disabled").expect("delete dp");
        assert!(list_world_datapacks(&world_dir).is_empty());

        // Traversal attempts in world/datapack operations must fail with MqError::ZipSlip
        for bad in ["..", "../escape", "sub/nested", r"..\escape"] {
            assert!(matches!(
                toggle_world_datapack(&world_dir, bad),
                Err(MqError::ZipSlip { .. })
            ));
            assert!(matches!(
                delete_world_datapack(&world_dir, bad),
                Err(MqError::ZipSlip { .. })
            ));
            assert!(matches!(
                delete_world(temp.path(), bad),
                Err(MqError::ZipSlip { .. })
            ));
            assert!(matches!(
                export_world_to_zip(temp.path(), bad, &temp.path().join("out.zip")),
                Err(MqError::ZipSlip { .. })
            ));
            assert!(matches!(
                backup_world(temp.path(), &temp.path().join("backups"), "inst", bad),
                Err(MqError::ZipSlip { .. })
            ));
        }
    }

    #[test]
    fn test_restore_world_preserves_underscores_and_rolls_back_on_corrupt_zip() {
        let temp = tempdir().expect("tempdir");
        let inst_dir = temp.path().join("inst");
        let backups_root = temp.path().join("backups");
        let inst_backups = backups_root.join("inst");
        fs::create_dir_all(&inst_backups).unwrap();

        // 1. Create valid backup with underscores in world name + timestamp suffix
        let zip_name = "My_Survival_World_2026-11-24_14-00-00.zip";
        let zip_path = inst_backups.join(zip_name);
        {
            let f = File::create(&zip_path).unwrap();
            let mut zw = ZipWriter::new(f);
            zw.start_file("level.dat_new", SimpleFileOptions::default())
                .unwrap();
            zw.write_all(b"restored-payload-v2").unwrap();
            zw.finish().unwrap();
        }

        let restored_dir =
            restore_world(&backups_root, "inst", zip_name, &inst_dir, None).expect("restore");
        assert_eq!(
            restored_dir.file_name().and_then(|s| s.to_str()),
            Some("My_Survival_World")
        );
        assert_eq!(
            fs::read(restored_dir.join("level.dat_new")).unwrap(),
            b"restored-payload-v2"
        );

        // 2. Corrupt ZIP must NOT delete or overwrite existing My_Survival_World!
        fs::write(restored_dir.join("precious.dat"), b"do-not-lose-me").unwrap();
        let corrupt_zip = "My_Survival_World_2026-11-24_15-00-00.zip";
        fs::write(inst_backups.join(corrupt_zip), b"PK\x03\x04corrupted_data").unwrap();

        assert!(restore_world(&backups_root, "inst", corrupt_zip, &inst_dir, None).is_err());
        assert_eq!(
            fs::read(restored_dir.join("precious.dat")).unwrap(),
            b"do-not-lose-me"
        );

        // 3. Simulated rename failure after moving dest_dir -> .mq-backup-prev-<uuid> must restore original dest_dir!
        let staging = inst_dir.join("saves").join(".mq-restore-sim");
        let prev = inst_dir.join("saves").join(".mq-backup-prev-sim");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("incoming.dat"), b"new").unwrap();

        let swap_err = atomic_swap_restored_world_dir(&staging, &restored_dir, &prev, |s, d| {
            if s == staging {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "simulated rename failure",
                ))
            } else {
                fs::rename(s, d)
            }
        });
        assert!(swap_err.is_err());
        assert!(!staging.exists());
        assert!(!prev.exists());
        assert_eq!(
            fs::read(restored_dir.join("precious.dat")).unwrap(),
            b"do-not-lose-me"
        );
    }

    #[test]
    fn test_auto_backup_pruning_never_deletes_prefix_sibling_worlds() {
        let temp = tempdir().expect("tempdir");
        let inst_dir = temp.path().join("inst");
        let saves_dir = inst_dir.join("saves");
        let auto_dir = inst_dir.join("backups").join("auto");
        fs::create_dir_all(saves_dir.join("Survival")).unwrap();
        fs::write(saves_dir.join("Survival").join("data.bin"), b"w1").unwrap();
        fs::create_dir_all(&auto_dir).unwrap();

        // Pre-populate backups for sibling world "Survival-Hardcore" and older "Survival" backups
        let sibling_backup = auto_dir.join("Survival-Hardcore-20260101-120000.zip");
        fs::write(&sibling_backup, b"sibling-world-backup").unwrap();
        let old1 = auto_dir.join("Survival-20250101-010000.zip");
        let old2 = auto_dir.join("Survival-20250102-010000.zip");
        fs::write(&old1, b"old1").unwrap();
        fs::write(&old2, b"old2").unwrap();

        let created = auto_backup_instance_worlds(&inst_dir, 1).expect("auto backup");
        assert_eq!(created.len(), 1);
        assert!(
            sibling_backup.is_file(),
            "Survival pruning must never delete Survival-Hardcore backups"
        );
        assert!(!old1.exists() || !old2.exists());
    }

    #[test]
    fn test_binary_nbt_level_dat_parses_1_21_5_spawn_and_worldgen_seed_accurately() {
        let temp = tempdir().expect("tempdir");
        let world_dir = temp.path().join("ModernWorld");
        fs::create_dir_all(&world_dir).unwrap();

        // Build real gzipped binary NBT with DataVersion + decoy wheat_seeds + GameType(Byte)
        // + WorldGenSettings.seed + 1.21.5 spawn.pos IntArray + Version.Name
        let mut nbt = Vec::new();
        nbt.push(TAG_COMPOUND);
        write_nbt_string_checked(&mut nbt, "root", "").unwrap();

        nbt.push(TAG_COMPOUND);
        write_nbt_string_checked(&mut nbt, "Data", "Data").unwrap();

        // Decoy DataVersion TAG_Int BEFORE Version compound
        nbt.push(TAG_INT);
        write_nbt_string_checked(&mut nbt, "DataVersion", "DataVersion").unwrap();
        nbt.extend_from_slice(&4189i32.to_be_bytes());

        // Decoy inventory item containing substring "seed" BEFORE WorldGenSettings.seed
        nbt.push(TAG_LIST);
        write_nbt_string_checked(&mut nbt, "Inventory", "Inventory").unwrap();
        nbt.push(TAG_STRING);
        nbt.extend_from_slice(&1i32.to_be_bytes());
        write_nbt_string_checked(&mut nbt, "item", "minecraft:wheat_seeds").unwrap();

        // LevelName
        nbt.push(TAG_STRING);
        write_nbt_string_checked(&mut nbt, "LevelName", "LevelName").unwrap();
        write_nbt_string_checked(&mut nbt, "val", "Мир 1.21.5").unwrap();

        // GameType as TAG_Byte (1 = Creative)
        nbt.push(TAG_BYTE);
        write_nbt_string_checked(&mut nbt, "GameType", "GameType").unwrap();
        nbt.push(1);

        // hardcore = 0
        nbt.push(TAG_BYTE);
        write_nbt_string_checked(&mut nbt, "hardcore", "hardcore").unwrap();
        nbt.push(0);

        // LastPlayed
        nbt.push(TAG_LONG);
        write_nbt_string_checked(&mut nbt, "LastPlayed", "LastPlayed").unwrap();
        nbt.extend_from_slice(&1_730_000_000_000i64.to_be_bytes());

        // WorldGenSettings.seed
        nbt.push(TAG_COMPOUND);
        write_nbt_string_checked(&mut nbt, "WorldGenSettings", "WorldGenSettings").unwrap();
        nbt.push(TAG_LONG);
        write_nbt_string_checked(&mut nbt, "seed", "seed").unwrap();
        nbt.extend_from_slice(&(-987_654_321_123_456i64).to_be_bytes());
        nbt.push(TAG_END);

        // 1.21.5+ spawn.pos IntArray [120, 68, -340]
        nbt.push(TAG_COMPOUND);
        write_nbt_string_checked(&mut nbt, "spawn", "spawn").unwrap();
        nbt.push(TAG_INT_ARRAY);
        write_nbt_string_checked(&mut nbt, "pos", "pos").unwrap();
        nbt.extend_from_slice(&3i32.to_be_bytes());
        nbt.extend_from_slice(&120i32.to_be_bytes());
        nbt.extend_from_slice(&68i32.to_be_bytes());
        nbt.extend_from_slice(&(-340i32).to_be_bytes());
        nbt.push(TAG_END);

        // Version.Name
        nbt.push(TAG_COMPOUND);
        write_nbt_string_checked(&mut nbt, "Version", "Version").unwrap();
        nbt.push(TAG_STRING);
        write_nbt_string_checked(&mut nbt, "Name", "Name").unwrap();
        write_nbt_string_checked(&mut nbt, "ver", "1.21.5").unwrap();
        nbt.push(TAG_END);

        nbt.push(TAG_END); // end Data
        nbt.push(TAG_END); // end root

        let level_dat = world_dir.join("level.dat");
        let mut gz = GzEncoder::new(File::create(&level_dat).unwrap(), Compression::default());
        gz.write_all(&nbt).unwrap();
        gz.finish().unwrap();

        let meta = read_world_level_dat(&world_dir).expect("parse level.dat");
        assert_eq!(meta.level_name, "Мир 1.21.5");
        assert_eq!(meta.game_type, Some(1));
        assert!(!meta.hardcore);
        assert_eq!(meta.seed, Some(-987_654_321_123_456));
        assert_eq!(meta.spawn_xyz, Some((120, 68, -340)));
        assert_eq!(meta.version_name.as_deref(), Some("1.21.5"));

        let (disp_name, mode_label, lp) = parse_level_dat(&level_dat).expect("parse summary");
        assert_eq!(disp_name, "Мир 1.21.5");
        assert_eq!(mode_label, "Творчество");
        assert!(lp.is_some());
    }
}
