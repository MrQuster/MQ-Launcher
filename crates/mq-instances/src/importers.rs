//! External launcher instance discoverer and importer for Prism Launcher, ElyPrism, MultiMC, and Modrinth App.

use std::fs;
use std::path::{Path, PathBuf};

use mq_core::{ModLoaderKind, MqPaths, MqResult};

use crate::manager::InstanceManager;
use crate::manifest::InstanceManifest;

/// Recognized external launcher origins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalLauncherKind {
    Prism,
    ElyPrism,
    MultiMc,
    ModrinthApp,
}

impl ExternalLauncherKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Prism => "Prism Launcher",
            Self::ElyPrism => "ElyPrism Launcher",
            Self::MultiMc => "MultiMC",
            Self::ModrinthApp => "Modrinth App",
        }
    }
}

/// Metadata describing an external launcher instance found on disk.
#[derive(Debug, Clone)]
pub struct DiscoveredExternalInstance {
    pub launcher: ExternalLauncherKind,
    pub name: String,
    pub path: PathBuf,
    pub mc_version: String,
    pub loader: ModLoaderKind,
    pub loader_version: Option<String>,
    pub mod_count: usize,
    pub world_count: usize,
}

/// Scans standard system directories for Prism, ElyPrism, MultiMC, and Modrinth App instances.
pub fn discover_external_instances() -> Vec<DiscoveredExternalInstance> {
    let mut discovered = Vec::new();

    #[cfg(target_os = "linux")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let home_p = Path::new(&home);
            let local_share = home_p.join(".local/share");
            scan_instances_dir(
                &local_share.join("PrismLauncher/instances"),
                ExternalLauncherKind::Prism,
                &mut discovered,
            );
            scan_instances_dir(
                &local_share.join("ElyPrismLauncher/instances"),
                ExternalLauncherKind::ElyPrism,
                &mut discovered,
            );
            scan_instances_dir(
                &local_share.join("MultiMC/instances"),
                ExternalLauncherKind::MultiMc,
                &mut discovered,
            );
            scan_instances_dir(
                &home_p
                    .join(".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher/instances"),
                ExternalLauncherKind::Prism,
                &mut discovered,
            );

            let config_dir = home_p.join(".config");
            scan_modrinth_profiles_dir(
                &config_dir.join("com.modrinth.theseus/profiles"),
                &mut discovered,
            );
            scan_modrinth_profiles_dir(&local_share.join("ModrinthApp/profiles"), &mut discovered);
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let appdata_p = Path::new(&appdata);
            scan_instances_dir(
                &appdata_p.join("PrismLauncher/instances"),
                ExternalLauncherKind::Prism,
                &mut discovered,
            );
            scan_instances_dir(
                &appdata_p.join("ElyPrismLauncher/instances"),
                ExternalLauncherKind::ElyPrism,
                &mut discovered,
            );
            scan_instances_dir(
                &appdata_p.join("MultiMC/instances"),
                ExternalLauncherKind::MultiMc,
                &mut discovered,
            );
            scan_modrinth_profiles_dir(&appdata_p.join("ModrinthApp/profiles"), &mut discovered);
        }
    }

    discovered.sort_by(|a, b| a.name.cmp(&b.name));
    discovered
}

fn scan_instances_dir(
    root: &Path,
    launcher: ExternalLauncherKind,
    out: &mut Vec<DiscoveredExternalInstance>,
) {
    if !root.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let cfg_path = path.join("instance.cfg");
        let mmc_pack_path = path.join("mmc-pack.json");
        if !cfg_path.is_file() && !mmc_pack_path.is_file() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let mut display_name = dir_name;
        let mut mc_version = "1.20.1".to_string();
        let mut loader = ModLoaderKind::Vanilla;
        let mut loader_version = None;

        if let Ok(cfg_content) = fs::read_to_string(&cfg_path) {
            for line in cfg_content.lines() {
                if let Some(val) = line.strip_prefix("name=") {
                    let trimmed = val.trim();
                    if !trimmed.is_empty() {
                        display_name = trimmed.to_string();
                    }
                } else if let Some(val) = line.strip_prefix("IntendedVersion=") {
                    let trimmed = val.trim();
                    if !trimmed.is_empty() {
                        mc_version = trimmed.to_string();
                    }
                }
            }
        }

        if let Ok(pack_content) = fs::read_to_string(&mmc_pack_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&pack_content) {
                if let Some(components) = json.get("components").and_then(|c| c.as_array()) {
                    for comp in components {
                        let uid = comp.get("uid").and_then(|u| u.as_str()).unwrap_or_default();
                        let version = comp
                            .get("version")
                            .or_else(|| comp.get("cachedVersion"))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        match uid {
                            "net.minecraft" => {
                                if !version.is_empty() {
                                    mc_version = version.to_string();
                                }
                            }
                            "net.fabricmc.fabric-loader" => {
                                loader = ModLoaderKind::Fabric;
                                if !version.is_empty() {
                                    loader_version = Some(version.to_string());
                                }
                            }
                            "org.quiltmc.quilt-loader" => {
                                loader = ModLoaderKind::Quilt;
                                if !version.is_empty() {
                                    loader_version = Some(version.to_string());
                                }
                            }
                            "net.neoforged" | "net.neoforged.neoforge" => {
                                loader = ModLoaderKind::NeoForge;
                                if !version.is_empty() {
                                    loader_version = Some(version.to_string());
                                }
                            }
                            "net.minecraftforge" => {
                                loader = ModLoaderKind::Forge;
                                if !version.is_empty() {
                                    loader_version = Some(version.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        let mc_dir = resolve_external_game_dir(&path);

        let mod_count = count_files(&mc_dir.join("mods"), "jar");
        let world_count = count_subdirs(&mc_dir.join("saves"));

        out.push(DiscoveredExternalInstance {
            launcher,
            name: display_name,
            path,
            mc_version,
            loader,
            loader_version,
            mod_count,
            world_count,
        });
    }
}

fn scan_modrinth_profiles_dir(root: &Path, out: &mut Vec<DiscoveredExternalInstance>) {
    if !root.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let profile_json_path = path.join("profile.json");
        if !profile_json_path.is_file() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let mut display_name = dir_name;
        let mut mc_version = "1.20.1".to_string();
        let mut loader = ModLoaderKind::Vanilla;
        let mut loader_version = None;

        if let Ok(raw) = fs::read_to_string(&profile_json_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(name) = json.get("name").and_then(|n| n.as_str()) {
                    if !name.trim().is_empty() {
                        display_name = name.trim().to_string();
                    }
                }
                if let Some(gv) = json.get("game_version").and_then(|v| v.as_str()) {
                    if !gv.trim().is_empty() {
                        mc_version = gv.trim().to_string();
                    }
                }
                if let Some(l) = json.get("loader").and_then(|v| v.as_str()) {
                    match l.to_ascii_lowercase().as_str() {
                        "fabric" => loader = ModLoaderKind::Fabric,
                        "quilt" => loader = ModLoaderKind::Quilt,
                        "neoforge" => loader = ModLoaderKind::NeoForge,
                        "forge" => loader = ModLoaderKind::Forge,
                        _ => {}
                    }
                }
                if let Some(lv) = json.get("loader_version").and_then(|v| v.as_str()) {
                    if !lv.trim().is_empty() {
                        loader_version = Some(lv.trim().to_string());
                    }
                }
            }
        }

        let mod_count = count_files(&path.join("mods"), "jar");
        let world_count = count_subdirs(&path.join("saves"));

        out.push(DiscoveredExternalInstance {
            launcher: ExternalLauncherKind::ModrinthApp,
            name: display_name,
            path,
            mc_version,
            loader,
            loader_version,
            mod_count,
            world_count,
        });
    }
}

fn count_files(dir: &Path, ext: &str) -> usize {
    if !dir.is_dir() {
        return 0;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            e.path().is_file()
                && e.path()
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|e_str| e_str.eq_ignore_ascii_case(ext))
        })
        .count()
}

fn count_subdirs(dir: &Path) -> usize {
    if !dir.is_dir() {
        return 0;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries.flatten().filter(|e| e.path().is_dir()).count()
}

fn resolve_external_game_dir(path: &Path) -> PathBuf {
    if path.join(".minecraft").is_dir() {
        path.join(".minecraft")
    } else if path.join("minecraft").is_dir() {
        path.join("minecraft")
    } else {
        path.to_path_buf()
    }
}

/// Imports a discovered external instance into the launcher.
pub fn import_external_instance(
    manager: &InstanceManager,
    _paths: &MqPaths,
    item: &DiscoveredExternalInstance,
) -> MqResult<InstanceManifest> {
    let mut manifest = manager.create_instance(
        &item.name,
        &item.mc_version,
        item.loader,
        item.loader_version.clone(),
    )?;

    let target_instance_dir = manifest.instance_dir(manager.paths());
    let source_game_dir = resolve_external_game_dir(&item.path);

    for subdir in &["mods", "saves", "config", "resourcepacks", "shaderpacks"] {
        let src = source_game_dir.join(subdir);
        let dst = target_instance_dir.join(subdir);
        if src.is_dir() {
            copy_dir_all(&src, &dst)?;
        }
    }

    for file in &["options.txt", "servers.dat"] {
        let src = source_game_dir.join(file);
        let dst = target_instance_dir.join(file);
        if src.is_file() {
            fs::copy(&src, &dst)?;
        }
    }

    manifest.modpack_source = None;

    manager.save_instance(&manifest)?;
    Ok(manifest)
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let meta = match fs::symlink_metadata(&from) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };

        // Skip dangling symlinks safely
        if meta.file_type().is_symlink() && !from.exists() {
            continue;
        }

        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_import_external_instance() {
        let temp = tempdir().expect("tempdir");
        let external_inst = temp.path().join("PrismInst");
        let ext_mods = external_inst.join("mods");
        fs::create_dir_all(&ext_mods).expect("create ext mods");
        fs::write(ext_mods.join("test_mod.jar"), b"jar content").unwrap();

        let mq_paths = MqPaths::with_root(temp.path().join("mq_data"));
        let manager = InstanceManager::new(mq_paths.clone());

        let discovered = DiscoveredExternalInstance {
            launcher: ExternalLauncherKind::Prism,
            name: "Imported Prism".to_string(),
            path: external_inst,
            mc_version: "1.21.1".to_string(),
            loader: ModLoaderKind::Fabric,
            loader_version: Some("0.16.9".to_string()),
            mod_count: 1,
            world_count: 0,
        };

        let manifest = import_external_instance(&manager, &mq_paths, &discovered).expect("import");
        assert_eq!(manifest.name, "Imported Prism");
        assert_eq!(manifest.mc_version, "1.21.1");
        assert_eq!(manifest.loader, ModLoaderKind::Fabric);

        let imported_dir = manifest.instance_dir(&mq_paths);
        assert!(imported_dir.join("mods").join("test_mod.jar").is_file());
    }
}
