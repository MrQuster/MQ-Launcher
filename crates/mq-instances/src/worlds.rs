//! Minecraft world saves scanner, 1-click ZIP backup and restore, and export.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use flate2::read::GzDecoder;
use mq_core::fs_util::{
    remove_dir_all_with_retry, remove_file_with_retry, rename_with_retry,
    validate_windows_safe_filename,
};
use mq_core::platform::sanitize_filename;
use mq_core::{resolve_safe_path, MqError, MqResult};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

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
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            } else if path.is_dir() {
                total += compute_dir_size(&path);
            }
        }
    }
    total
}

/// Parses basic metadata from `level.dat` using Gzip decompression and simple NBT scanning.
fn parse_level_dat(path: &Path) -> MqResult<(String, String, Option<DateTime<Utc>>)> {
    let file = File::open(path)?;
    let mut decoder = GzDecoder::new(file);
    let mut bytes = Vec::new();
    decoder.read_to_end(&mut bytes)?;

    let mut name = String::new();
    let mut game_mode = "Выживание".to_string();
    let mut last_played = None;

    // Scan for LevelName tag: TAG_String (8) followed by name "LevelName"
    if let Some(pos) = find_subsequence(&bytes, b"LevelName") {
        let after = pos + 9;
        if after + 2 <= bytes.len() {
            let len = u16::from_be_bytes([bytes[after], bytes[after + 1]]) as usize;
            if after + 2 + len <= bytes.len() {
                if let Ok(s) = std::str::from_utf8(&bytes[after + 2..after + 2 + len]) {
                    name = s.to_string();
                }
            }
        }
    }

    // Scan for GameType tag: TAG_Int (3) or TAG_Byte (1) with name "GameType"
    if let Some(pos) = find_subsequence(&bytes, b"GameType") {
        let after = pos + 8;
        if after + 4 <= bytes.len() {
            let mode_val = i32::from_be_bytes([
                bytes[after],
                bytes[after + 1],
                bytes[after + 2],
                bytes[after + 3],
            ]);
            game_mode = match mode_val {
                0 => "Выживание".to_string(),
                1 => "Творчество".to_string(),
                2 => "Приключение".to_string(),
                3 => "Наблюдатель".to_string(),
                _ => "Выживание".to_string(),
            };
        }
    }

    // Scan for hardcore tag: TAG_Byte (1) with name "hardcore"
    if let Some(pos) = find_subsequence(&bytes, b"hardcore") {
        let after = pos + 8;
        if after < bytes.len() && bytes[after] == 1 {
            game_mode = "Хардкор".to_string();
        }
    }

    // Scan for LastPlayed: TAG_Long (4) with name "LastPlayed"
    if let Some(pos) = find_subsequence(&bytes, b"LastPlayed") {
        let after = pos + 10;
        if after + 8 <= bytes.len() {
            let millis = i64::from_be_bytes([
                bytes[after],
                bytes[after + 1],
                bytes[after + 2],
                bytes[after + 3],
                bytes[after + 4],
                bytes[after + 5],
                bytes[after + 6],
                bytes[after + 7],
            ]);
            if millis > 0 {
                last_played = Utc.timestamp_millis_opt(millis).single();
            }
        }
    }

    Ok((name, game_mode, last_played))
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Creates a 1-click ZIP backup of the specified world save directory under
/// `<backups_root>/<instance_id>/<world_folder>_<timestamp>.zip`.
pub fn backup_world(
    instance_dir: &Path,
    backups_root: &Path,
    instance_id: &str,
    world_folder: &str,
) -> MqResult<PathBuf> {
    let source_dir = instance_dir.join("saves").join(world_folder);
    if !source_dir.is_dir() {
        return Err(MqError::Internal(format!(
            "world directory not found: {}",
            source_dir.display()
        )));
    }

    let target_dir = backups_root.join(instance_id);
    fs::create_dir_all(&target_dir)?;

    let timestamp = Utc::now().format("%Y-%m-%d_%H-%M-%S");
    let safe_world = sanitize_filename(world_folder);
    let zip_name = format!("{safe_world}_{timestamp}.zip");
    let zip_path = target_dir.join(&zip_name);

    export_world_to_zip(instance_dir, world_folder, &zip_path)?;
    Ok(zip_path)
}

/// Lists all backups for a given instance.
pub fn list_backups(backups_root: &Path, instance_id: &str) -> MqResult<Vec<WorldBackupInfo>> {
    let target_dir = backups_root.join(instance_id);
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
            let created_at: DateTime<Utc> = meta
                .modified()
                .unwrap_or_else(|_| std::time::SystemTime::now())
                .into();

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
pub fn restore_world(
    backups_root: &Path,
    instance_id: &str,
    backup_file_name: &str,
    instance_dir: &Path,
    target_folder_name: Option<&str>,
) -> MqResult<PathBuf> {
    let backup_path = backups_root.join(instance_id).join(backup_file_name);
    if !backup_path.is_file() {
        return Err(MqError::Internal(format!(
            "backup archive not found: {}",
            backup_path.display()
        )));
    }

    let resolved_folder = target_folder_name
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            backup_file_name
                .trim_end_matches(".zip")
                .trim_end_matches(".ZIP")
                .split('_')
                .next()
                .unwrap_or("RestoredWorld")
                .to_string()
        });
    let safe_folder = if validate_windows_safe_filename(&resolved_folder) {
        resolved_folder
    } else {
        sanitize_filename(&resolved_folder)
    };

    let saves_dir = instance_dir.join("saves");
    fs::create_dir_all(&saves_dir)?;
    let dest_dir = resolve_safe_path(&saves_dir, &safe_folder)?;

    let file = File::open(&backup_path)?;
    let mut archive = ZipArchive::new(file)
        .map_err(|e| MqError::Internal(format!("failed to open backup ZIP archive: {e}")))?;

    // Pre-validate every entry against Zip-Slip, Windows drive prefixes, NUL bytes, and symlinks
    // before touching any existing world save directory.
    for i in 0..archive.len() {
        let entry = archive
            .by_index_raw(i)
            .map_err(|e| MqError::Internal(format!("corrupted entry in backup ZIP: {e}")))?;
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
        if normalized.split('/').any(|seg| seg == "..") {
            return Err(MqError::ZipSlip {
                entry: raw_name.to_owned(),
            });
        }
        let trimmed = normalized.trim_end_matches('/');
        if !trimmed.is_empty() {
            let _ = resolve_safe_path(&dest_dir, trimmed)?;
        }
    }

    if dest_dir.exists() {
        remove_dir_all_with_retry(&dest_dir)?;
    }
    fs::create_dir_all(&dest_dir)?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| MqError::Internal(format!("corrupted entry in backup ZIP: {e}")))?;

        let raw_name = entry.name().to_string();
        let normalized = raw_name.replace('\\', "/");
        let trimmed = normalized.trim_end_matches('/');
        if trimmed.is_empty() {
            continue;
        }
        let out_path = resolve_safe_path(&dest_dir, trimmed)?;
        if entry.is_dir() || raw_name.ends_with('/') || raw_name.ends_with('\\') {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out_file = File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out_file)?;
        }
    }

    Ok(dest_dir)
}

/// Exports a world directory to a standalone ZIP archive.
pub fn export_world_to_zip(
    instance_dir: &Path,
    world_folder: &str,
    dest_zip_path: &Path,
) -> MqResult<()> {
    let src_dir = instance_dir.join("saves").join(world_folder);
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
            let relative = path
                .strip_prefix(base_dir)
                .map_err(|e| MqError::Internal(format!("strip prefix failed: {e}")))?;
            let name_str = relative.to_string_lossy().replace('\\', "/");

            if path.is_dir() {
                zip.add_directory(&name_str, options)
                    .map_err(|e| MqError::Internal(format!("failed adding dir to ZIP: {e}")))?;
                add_dir_to_zip(zip, base_dir, &path, options)?;
            } else if path.is_file() {
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
    let world_dir = instance_dir.join("saves").join(world_folder);
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

/// Reads level.dat metadata from `world_dir/level.dat` using Gzip decompression and NBT tag scanning.
pub fn read_world_level_dat(world_dir: &Path) -> Option<WorldNbtMeta> {
    let level_dat = world_dir.join("level.dat");
    if !level_dat.is_file() {
        return None;
    }
    let file = File::open(&level_dat).ok()?;
    let mut decoder = GzDecoder::new(file);
    let mut bytes = Vec::new();
    if decoder.read_to_end(&mut bytes).is_err() {
        if let Ok(raw) = fs::read(&level_dat) {
            bytes = raw;
        } else {
            return None;
        }
    }

    let mut meta = WorldNbtMeta::default();

    if let Some(pos) = find_subsequence(&bytes, b"LevelName") {
        let after = pos + 9;
        if after + 2 <= bytes.len() {
            let len = u16::from_be_bytes([bytes[after], bytes[after + 1]]) as usize;
            if after + 2 + len <= bytes.len() {
                if let Ok(s) = std::str::from_utf8(&bytes[after + 2..after + 2 + len]) {
                    meta.level_name = s.to_string();
                }
            }
        }
    }
    if meta.level_name.is_empty() {
        meta.level_name = world_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("World")
            .to_string();
    }

    if let Some(pos) = find_subsequence(&bytes, b"GameType") {
        let after = pos + 8;
        if after + 4 <= bytes.len() {
            let mode = i32::from_be_bytes([
                bytes[after],
                bytes[after + 1],
                bytes[after + 2],
                bytes[after + 3],
            ]);
            meta.game_type = Some(mode);
        }
    }

    if let Some(pos) = find_subsequence(&bytes, b"hardcore") {
        let after = pos + 8;
        if after < bytes.len() {
            meta.hardcore = bytes[after] == 1;
        }
    }

    if let Some(pos) = find_subsequence(&bytes, b"RandomSeed") {
        let after = pos + 10;
        if after + 8 <= bytes.len() {
            meta.seed = Some(i64::from_be_bytes([
                bytes[after],
                bytes[after + 1],
                bytes[after + 2],
                bytes[after + 3],
                bytes[after + 4],
                bytes[after + 5],
                bytes[after + 6],
                bytes[after + 7],
            ]));
        }
    } else if let Some(pos) = find_subsequence(&bytes, b"seed") {
        let after = pos + 4;
        if after + 8 <= bytes.len() {
            meta.seed = Some(i64::from_be_bytes([
                bytes[after],
                bytes[after + 1],
                bytes[after + 2],
                bytes[after + 3],
                bytes[after + 4],
                bytes[after + 5],
                bytes[after + 6],
                bytes[after + 7],
            ]));
        }
    }

    let sx = find_subsequence(&bytes, b"SpawnX").and_then(|pos| {
        let after = pos + 6;
        (after + 4 <= bytes.len()).then(|| {
            i32::from_be_bytes([
                bytes[after],
                bytes[after + 1],
                bytes[after + 2],
                bytes[after + 3],
            ])
        })
    });
    let sy = find_subsequence(&bytes, b"SpawnY").and_then(|pos| {
        let after = pos + 6;
        (after + 4 <= bytes.len()).then(|| {
            i32::from_be_bytes([
                bytes[after],
                bytes[after + 1],
                bytes[after + 2],
                bytes[after + 3],
            ])
        })
    });
    let sz = find_subsequence(&bytes, b"SpawnZ").and_then(|pos| {
        let after = pos + 6;
        (after + 4 <= bytes.len()).then(|| {
            i32::from_be_bytes([
                bytes[after],
                bytes[after + 1],
                bytes[after + 2],
                bytes[after + 3],
            ])
        })
    });
    if let (Some(x), Some(y), Some(z)) = (sx, sy, sz) {
        meta.spawn_xyz = Some((x, y, z));
    }

    if let Some(pos) = find_subsequence(&bytes, b"Version") {
        if let Some(name_pos) = find_subsequence(&bytes[pos..], b"Name") {
            let abs_name_pos = pos + name_pos;
            let after = abs_name_pos + 4;
            if after + 2 <= bytes.len() {
                let len = u16::from_be_bytes([bytes[after], bytes[after + 1]]) as usize;
                if after + 2 + len <= bytes.len() {
                    if let Ok(s) = std::str::from_utf8(&bytes[after + 2..after + 2 + len]) {
                        if !s.is_empty() {
                            meta.version_name = Some(s.to_string());
                        }
                    }
                }
            }
        }
    }

    Some(meta)
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
    let src = dp_dir.join(file_name);
    if !src.exists() {
        return Err(MqError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("datapack not found: {file_name}"),
        )));
    }

    if let Some(stripped) = file_name.strip_suffix(".disabled") {
        let dest = dp_dir.join(stripped);
        rename_with_retry(&src, &dest)?;
        Ok(true)
    } else {
        let dest = dp_dir.join(format!("{file_name}.disabled"));
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
        .ok_or_else(|| MqError::Internal("invalid source filename".into()))?;
    let dest = dp_dir.join(file_name);

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
        let entry_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Deletes a world datapack by filename or directory name.
pub fn delete_world_datapack(world_dir: &Path, file_name: &str) -> MqResult<()> {
    let dp_dir = world_dir.join("datapacks");
    let target = dp_dir.join(file_name);
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
        let zip_name = format!("{world_name}-{timestamp}.zip");
        let zip_path = auto_dir.join(&zip_name);

        if export_world_to_zip(instance_dir, &world_name, &zip_path).is_ok() {
            created.push(zip_path);
        }

        let mut world_backups = Vec::new();
        if let Ok(entries) = fs::read_dir(&auto_dir) {
            let prefix = format!("{world_name}-");
            for b_entry in entries.flatten() {
                let p = b_entry.path();
                if p.is_file() {
                    let fname = b_entry.file_name().to_string_lossy().into_owned();
                    if fname.starts_with(&prefix) && fname.ends_with(".zip") {
                        if let Ok(meta) = b_entry.metadata() {
                            let mtime =
                                meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
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
    use tempfile::tempdir;

    #[test]
    fn test_datapacks_and_level_dat() {
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
        assert!(!toggled); // now disabled

        let list_disabled = list_world_datapacks(&world_dir);
        assert_eq!(list_disabled.len(), 1);
        assert!(!list_disabled[0].enabled);

        delete_world_datapack(&world_dir, "extra_pack.zip.disabled").expect("delete dp");
        assert!(list_world_datapacks(&world_dir).is_empty());
    }
}
