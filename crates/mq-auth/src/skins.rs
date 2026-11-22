//! Local skin library persistence, listing, import, and deletion.

use std::fs;
use std::path::{Path, PathBuf};

use mq_core::fs_util::remove_file_with_retry;
use mq_core::{MqError, MqResult};
use serde::{Deserialize, Serialize};

pub const SKINS_MANIFEST_FILE: &str = "skins.json";

/// Entry in the local skin library.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkinLibraryEntry {
    pub id: String,
    pub name: String,
    pub file_path: PathBuf,
    pub slim: bool,
}

/// Lists all local skins registered in `skins_dir`.
pub fn list_local_skins(skins_dir: &Path) -> Vec<SkinLibraryEntry> {
    if !skins_dir.exists() {
        return Vec::new();
    }

    let manifest_path = skins_dir.join(SKINS_MANIFEST_FILE);
    if manifest_path.is_file() {
        if let Ok(content) = fs::read_to_string(&manifest_path) {
            if let Ok(entries) = serde_json::from_str::<Vec<SkinLibraryEntry>>(&content) {
                return entries
                    .into_iter()
                    .filter(|e| e.file_path.is_file())
                    .collect();
            }
        }
    }

    let mut discovered = Vec::new();
    if let Ok(entries) = fs::read_dir(skins_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("png"))
            {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("skin")
                    .to_string();
                discovered.push(SkinLibraryEntry {
                    id: stem.clone(),
                    name: stem,
                    file_path: path,
                    slim: false,
                });
            }
        }
    }
    discovered.sort_by(|a, b| a.name.cmp(&b.name));
    discovered
}

/// Imports a local `.png` file into the local skin library.
pub fn import_local_skin(
    skins_dir: &Path,
    name: &str,
    src_png: &Path,
    slim: bool,
) -> MqResult<SkinLibraryEntry> {
    if !src_png.is_file() {
        return Err(MqError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("source skin file not found: {}", src_png.display()),
        )));
    }

    fs::create_dir_all(skins_dir)?;

    let trimmed_name = name.trim();
    let display_name = if trimmed_name.is_empty() {
        src_png
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Custom Skin")
            .to_string()
    } else {
        trimmed_name.to_string()
    };

    let id = uuid::Uuid::new_v4().to_string();
    let file_name = format!("{id}.png");
    let dest_path = skins_dir.join(file_name);

    fs::copy(src_png, &dest_path)?;

    let entry = SkinLibraryEntry {
        id: id.clone(),
        name: display_name,
        file_path: dest_path,
        slim,
    };

    let mut all_skins = list_local_skins(skins_dir);
    all_skins.retain(|s| s.id != id);
    all_skins.push(entry.clone());

    let manifest_path = skins_dir.join(SKINS_MANIFEST_FILE);
    if let Ok(json) = serde_json::to_string_pretty(&all_skins) {
        let _ = fs::write(manifest_path, json);
    }

    Ok(entry)
}

/// Deletes a skin entry by `id` from the local skin library and removes its file.
pub fn delete_local_skin(skins_dir: &Path, id: &str) -> MqResult<()> {
    let mut all_skins = list_local_skins(skins_dir);
    let target = all_skins.iter().find(|s| s.id == id).cloned();

    all_skins.retain(|s| s.id != id);

    let manifest_path = skins_dir.join(SKINS_MANIFEST_FILE);
    if let Ok(json) = serde_json::to_string_pretty(&all_skins) {
        let _ = fs::write(manifest_path, json);
    }

    if let Some(entry) = target {
        if entry.file_path.is_file() {
            let _ = remove_file_with_retry(&entry.file_path);
        }
    } else {
        let file_path = skins_dir.join(format!("{id}.png"));
        if file_path.is_file() {
            let _ = remove_file_with_retry(&file_path);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_skin_library_import_list_delete() {
        let temp = tempdir().expect("tempdir");
        let skins_dir = temp.path().join("skins");
        let fake_skin = temp.path().join("steve.png");
        fs::write(&fake_skin, b"fake png data").expect("write fake skin");

        let entry =
            import_local_skin(&skins_dir, "My Skin", &fake_skin, true).expect("import skin");
        assert_eq!(entry.name, "My Skin");
        assert!(entry.slim);
        assert!(entry.file_path.is_file());

        let list = list_local_skins(&skins_dir);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, entry.id);

        delete_local_skin(&skins_dir, &entry.id).expect("delete skin");
        let after_delete = list_local_skins(&skins_dir);
        assert!(after_delete.is_empty());
        assert!(!entry.file_path.exists());
    }
}
