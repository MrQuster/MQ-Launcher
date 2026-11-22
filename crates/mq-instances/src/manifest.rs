//! Instance manifest (`instance.toml`) domain model and directory layout helpers.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use mq_core::fs_util::{remove_file_with_retry, rename_with_retry, validate_windows_safe_filename};
use mq_core::platform::atomic_write;
use mq_core::{InstanceId, ModLoaderKind, MqError, MqPaths, MqResult};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

static RESOLVED_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Produces an [`InstanceId`] directory slug that is guaranteed safe on BOTH Unix and Windows:
/// - Strips/replaces invalid Windows characters (`< > : " / \ | ? *`, control chars)
/// - Strips trailing dots/spaces
/// - Never collides with Windows reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1..9`,
///   `LPT1..9`, with or without dot extensions like `con.1`).
pub fn windows_safe_instance_id(display_name: &str) -> InstanceId {
    let base = InstanceId::sanitize_slug(display_name);
    let trimmed = base.as_str().trim_matches(|c: char| c == '.' || c == ' ');
    let candidate = if trimmed.is_empty() {
        "instance".to_owned()
    } else {
        trimmed.to_owned()
    };

    if validate_windows_safe_filename(&candidate) {
        if let Ok(id) = InstanceId::new(&candidate) {
            return id;
        }
    }

    // Rewrite reserved device names (e.g. `con`, `nul`, `com1`, `prn.1`) so the stem before
    // any `.` is no longer reserved on Windows.
    let safe_stem = candidate.replace('.', "-");
    let suffixed = format!("{safe_stem}-instance");
    InstanceId::new(&suffixed).unwrap_or_else(|_| InstanceId::sanitize_slug("instance"))
}

/// Default minimum JVM heap allocation in MiB (`-Xms1024M`).
pub const DEFAULT_MEMORY_MIN_MB: u32 = 1024;

/// Default maximum JVM heap allocation in MiB (`-Xmx4096M`).
pub const DEFAULT_MEMORY_MAX_MB: u32 = 4096;

/// Canonical filename for the per-instance TOML manifest (`instance.toml`).
pub const INSTANCE_MANIFEST_FILE: &str = "instance.toml";

/// Canonical filename for the cached resolved version metadata (`resolved_version.json`).
pub const RESOLVED_VERSION_FILE: &str = "resolved_version.json";

/// Subdirectories created automatically inside every isolated instance directory.
pub const INSTANCE_SUBDIRS: [&str; 5] =
    ["mods", "config", "resourcepacks", "shaderpacks", "natives"];

const fn default_memory_min_mb() -> u32 {
    DEFAULT_MEMORY_MIN_MB
}

const fn default_memory_max_mb() -> u32 {
    DEFAULT_MEMORY_MAX_MB
}

/// Saved mod profile representing an active set of mods and shaders.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModProfile {
    pub name: String,
    pub enabled_mods: Vec<String>,
    pub enabled_shaders: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

/// Metadata identifying the modpack origin of an instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModpackSourceMeta {
    pub source: String,
    pub project_id: String,
    pub version_id: String,
    pub version_name: String,
}

/// Persistent metadata describing an isolated Minecraft game instance, serialized
/// at `<data_dir>/instances/<id>/instance.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceManifest {
    /// Validated filesystem-safe directory slug (`[a-z0-9._-]+`).
    pub id: InstanceId,

    /// Human-readable display name.
    pub name: String,

    /// Target Minecraft version identifier (e.g. `"1.21.1"`).
    pub mc_version: String,

    /// Mod loader family (defaults to [`ModLoaderKind::Vanilla`]).
    #[serde(default)]
    pub loader: ModLoaderKind,

    /// Specific mod loader version (e.g. `Some("0.16.9")` for Fabric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader_version: Option<String>,

    /// Minimum JVM heap size in MiB (`-Xms`, defaults to `1024`).
    #[serde(default = "default_memory_min_mb")]
    pub memory_min_mb: u32,

    /// Maximum JVM heap size in MiB (`-Xmx`, defaults to `4096`).
    #[serde(default = "default_memory_max_mb")]
    pub memory_max_mb: u32,

    /// Optional custom path to a `java` binary overriding automatic JVM selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java_path: Option<PathBuf>,

    /// Additional user-configured JVM arguments appended after `-Xms`/`-Xmx`.
    #[serde(default)]
    pub jvm_args: Vec<String>,

    /// Optional process wrapper command prefix (e.g. `"gamemoderun"`, `"mangohud"`, `"prime-run"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapper_cmd: Option<String>,

    /// UTC timestamp when the instance was created.
    pub created_at: DateTime<Utc>,

    /// UTC timestamp when the instance was last launched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_played: Option<DateTime<Utc>>,

    /// Cumulative recorded playtime in seconds.
    #[serde(default)]
    pub playtime_secs: u64,

    /// Total recorded playtime in seconds.
    #[serde(default)]
    pub total_playtime_secs: u64,

    /// Saved mod & shader profiles for rapid switching.
    #[serde(default)]
    pub mod_profiles: Vec<ModProfile>,

    /// Currently active mod profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_mod_profile: Option<String>,

    /// Optional origin identifier if imported from a `.mrpack` or CurseForge `.zip` modpack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modpack_source: Option<ModpackSourceMeta>,

    /// Optional Minecraft game locale override (e.g. `Some("uk_ua")` or `Some("ru_ru")`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mc_locale: Option<String>,

    /// Whether the language was already initialized in options.txt, so launcher won't overwrite user changes.
    #[serde(default)]
    pub lang_initialized: bool,

    /// Optional instance icon (name of built-in vector icon or relative path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,

    /// Optional category group for collapsible organization in UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

impl InstanceManifest {
    /// Constructs a new [`InstanceManifest`] with sanitized slug ID and standard defaults.
    pub fn new(
        name: impl Into<String>,
        mc_version: impl Into<String>,
        loader: ModLoaderKind,
        loader_version: Option<String>,
    ) -> Self {
        let name_str = name.into().trim().to_owned();
        let id = windows_safe_instance_id(&name_str);
        Self {
            id,
            name: name_str,
            mc_version: mc_version.into().trim().to_owned(),
            loader,
            loader_version: loader_version
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty()),
            memory_min_mb: DEFAULT_MEMORY_MIN_MB,
            memory_max_mb: DEFAULT_MEMORY_MAX_MB,
            java_path: None,
            jvm_args: Vec::new(),
            wrapper_cmd: None,
            created_at: Utc::now(),
            last_played: None,
            playtime_secs: 0,
            total_playtime_secs: 0,
            mod_profiles: Vec::new(),
            active_mod_profile: None,
            modpack_source: None,
            mc_locale: None,
            lang_initialized: false,
            icon: None,
            group: None,
        }
    }

    /// Root directory for this instance (`<data_dir>/instances/<id>`).
    #[inline]
    pub fn instance_dir(&self, paths: &MqPaths) -> PathBuf {
        paths.instance_dir(self.id.as_str())
    }

    /// Isolated Minecraft `--gameDir` for this instance (`<data_dir>/instances/<id>`).
    #[inline]
    pub fn game_dir(&self, paths: &MqPaths) -> PathBuf {
        self.instance_dir(paths)
    }

    /// Path to `<data_dir>/instances/<id>/instance.toml`.
    #[inline]
    pub fn manifest_path(&self, paths: &MqPaths) -> PathBuf {
        self.instance_dir(paths).join(INSTANCE_MANIFEST_FILE)
    }

    /// Path to `<data_dir>/instances/<id>/mods`.
    #[inline]
    pub fn mods_dir(&self, paths: &MqPaths) -> PathBuf {
        self.instance_dir(paths).join("mods")
    }

    /// Path to `<data_dir>/instances/<id>/config`.
    #[inline]
    pub fn config_dir(&self, paths: &MqPaths) -> PathBuf {
        self.instance_dir(paths).join("config")
    }

    /// Path to `<data_dir>/instances/<id>/resourcepacks`.
    #[inline]
    pub fn resourcepacks_dir(&self, paths: &MqPaths) -> PathBuf {
        self.instance_dir(paths).join("resourcepacks")
    }

    /// Path to `<data_dir>/instances/<id>/shaderpacks`.
    #[inline]
    pub fn shaderpacks_dir(&self, paths: &MqPaths) -> PathBuf {
        self.instance_dir(paths).join("shaderpacks")
    }

    /// Path to `<data_dir>/instances/<id>/natives`.
    #[inline]
    pub fn natives_dir(&self, paths: &MqPaths) -> PathBuf {
        self.instance_dir(paths).join("natives")
    }

    /// Path to `<data_dir>/instances/<id>/resolved_version.json` for offline launch resolution.
    #[inline]
    pub fn resolved_version_json_path(&self, paths: &MqPaths) -> PathBuf {
        self.instance_dir(paths).join(RESOLVED_VERSION_FILE)
    }

    /// Atomically persists the cached resolved version metadata (`resolved_version.json`)
    /// inside this instance's directory so `mq launch` works completely offline.
    pub fn save_resolved_version<T: Serialize>(
        &self,
        paths: &MqPaths,
        version: &T,
    ) -> MqResult<PathBuf> {
        let inst_dir = self.instance_dir(paths);
        fs::create_dir_all(&inst_dir)?;
        let target_path = self.resolved_version_json_path(paths);
        let bytes = serde_json::to_vec_pretty(version)?;

        let seq = RESOLVED_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp_path = inst_dir.join(format!(
            ".{RESOLVED_VERSION_FILE}.{}.{seq}.tmp",
            std::process::id()
        ));

        let write_res = (|| -> std::io::Result<()> {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            rename_with_retry(&tmp_path, &target_path)?;
            Ok(())
        })();

        if let Err(err) = write_res {
            let _ = remove_file_with_retry(&tmp_path);
            return Err(MqError::Io(err));
        }

        Ok(target_path)
    }

    /// Loads the cached resolved version metadata (`resolved_version.json`) from this
    /// instance's directory.
    pub fn load_resolved_version<T: DeserializeOwned>(&self, paths: &MqPaths) -> MqResult<T> {
        let path = self.resolved_version_json_path(paths);
        if !path.is_file() {
            return Err(MqError::Metadata(format!(
                "cached resolved_version.json not found for instance '{}' at {}",
                self.id,
                path.display()
            )));
        }
        let raw = fs::read_to_string(&path)?;
        let parsed = serde_json::from_str(&raw)?;
        Ok(parsed)
    }

    /// Persists this manifest atomically to `<dir>/instance.toml`.
    pub fn save_to_dir(&self, dir: &std::path::Path) -> MqResult<()> {
        let manifest_path = dir.join(INSTANCE_MANIFEST_FILE);
        let toml_str = toml::to_string_pretty(self).map_err(|e| {
            MqError::Internal(format!("failed to serialize instance manifest: {e}"))
        })?;
        atomic_write(&manifest_path, toml_str.as_bytes())?;
        Ok(())
    }
}

/// Captures active mods and shaders from an instance directory into a [`ModProfile`].
pub fn capture_mod_profile(name: impl Into<String>, instance_dir: &std::path::Path) -> ModProfile {
    let mods_dir = instance_dir.join("mods");
    let mut enabled_mods = Vec::new();
    if mods_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&mods_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let file_name = entry.file_name().to_string_lossy().into_owned();
                    let lower = file_name.to_ascii_lowercase();
                    if lower.ends_with(".jar") {
                        enabled_mods.push(file_name);
                    }
                }
            }
        }
    }
    enabled_mods.sort();

    let shaderpacks_dir = instance_dir.join("shaderpacks");
    let mut enabled_shaders = Vec::new();
    if shaderpacks_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&shaderpacks_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let file_name = entry.file_name().to_string_lossy().into_owned();
                let lower = file_name.to_ascii_lowercase();
                if !lower.ends_with(".disabled") && (path.is_dir() || lower.ends_with(".zip")) {
                    enabled_shaders.push(file_name);
                }
            }
        }
    }
    enabled_shaders.sort();

    ModProfile {
        name: name.into(),
        enabled_mods,
        enabled_shaders,
        updated_at: Utc::now(),
    }
}

/// Applies a [`ModProfile`] to an instance directory, toggling mods and shaders to match the profile.
/// Returns the number of files renamed/toggled.
pub fn apply_mod_profile(instance_dir: &std::path::Path, profile: &ModProfile) -> MqResult<usize> {
    let mut toggled = 0;
    let mods_dir = instance_dir.join("mods");
    let enabled_mods_set: std::collections::HashSet<&str> =
        profile.enabled_mods.iter().map(String::as_str).collect();

    if mods_dir.is_dir() {
        for entry in fs::read_dir(&mods_dir)?.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let lower = file_name.to_ascii_lowercase();

            if lower.ends_with(".jar.disabled") {
                let base = &file_name[..file_name.len() - ".disabled".len()];
                if enabled_mods_set.contains(base) {
                    let target = mods_dir.join(base);
                    rename_with_retry(&path, &target)?;
                    toggled += 1;
                }
            } else if lower.ends_with(".jar") && !enabled_mods_set.contains(file_name.as_str()) {
                let target = mods_dir.join(format!("{file_name}.disabled"));
                rename_with_retry(&path, &target)?;
                toggled += 1;
            }
        }
    }

    let shaderpacks_dir = instance_dir.join("shaderpacks");
    let enabled_shaders_set: std::collections::HashSet<&str> =
        profile.enabled_shaders.iter().map(String::as_str).collect();

    if shaderpacks_dir.is_dir() {
        for entry in fs::read_dir(&shaderpacks_dir)?.flatten() {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let lower = file_name.to_ascii_lowercase();

            if lower.ends_with(".disabled") {
                let base = &file_name[..file_name.len() - ".disabled".len()];
                if enabled_shaders_set.contains(base) {
                    let target = shaderpacks_dir.join(base);
                    rename_with_retry(&path, &target)?;
                    toggled += 1;
                }
            } else if (path.is_dir() || lower.ends_with(".zip"))
                && !enabled_shaders_set.contains(file_name.as_str())
            {
                let target = shaderpacks_dir.join(format!("{file_name}.disabled"));
                rename_with_retry(&path, &target)?;
                toggled += 1;
            }
        }
    }

    Ok(toggled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_capture_and_apply_mod_profile() {
        let temp = tempdir().expect("tempdir");
        let inst_dir = temp.path();
        let mods_dir = inst_dir.join("mods");
        fs::create_dir_all(&mods_dir).expect("create mods dir");

        fs::write(mods_dir.join("mod_a.jar"), b"a").unwrap();
        fs::write(mods_dir.join("mod_b.jar"), b"b").unwrap();
        fs::write(mods_dir.join("mod_c.jar.disabled"), b"c").unwrap();

        let profile = capture_mod_profile("Profile 1", inst_dir);
        assert_eq!(profile.name, "Profile 1");
        assert_eq!(profile.enabled_mods, vec!["mod_a.jar", "mod_b.jar"]);

        let mut switched_profile = profile.clone();
        switched_profile.enabled_mods = vec!["mod_c.jar".into()];

        let toggled = apply_mod_profile(inst_dir, &switched_profile).expect("apply profile");
        assert_eq!(toggled, 3); // mod_a disabled, mod_b disabled, mod_c enabled
        assert!(mods_dir.join("mod_a.jar.disabled").is_file());
        assert!(mods_dir.join("mod_b.jar.disabled").is_file());
        assert!(mods_dir.join("mod_c.jar").is_file());
    }
}
