//! Binary search mod crash bisector: partitions candidate mods in halves across
//! testing rounds until isolating the single mod responsible for a startup crash.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use mq_core::fs_util::{remove_file_with_retry, rename_with_retry};
use mq_core::platform::atomic_write;
use mq_core::{MqError, MqResult};
use serde::{Deserialize, Serialize};

/// State file persisted inside `<mods_dir>` during an active bisect session.
const BISECT_STATE_FILE: &str = ".mq-bisect.json";

/// Core API / library mods that are kept enabled alongside `active_half` while bisecting
/// non-library suspects, and only bisected themselves if all non-library suspects pass.
const CORE_API_LIBRARIES: &[&str] = &[
    "fabric-api",
    "fabric-language-kotlin",
    "kotlinforforge",
    "architectury",
    "cloth-config",
    "geckolib",
    "curios",
    "trinkets",
    "balm",
    "bookshelf",
    "puzzleslib",
    "moonlight",
    "forgeconfigapiport",
    "owo-lib",
    "collective",
];

fn is_core_api_library(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    let base = lower
        .strip_suffix(".jar.disabled")
        .or_else(|| lower.strip_suffix(".jar"))
        .unwrap_or(&lower);

    CORE_API_LIBRARIES
        .iter()
        .any(|&lib| base.starts_with(lib) || base.contains(lib))
}

/// Active binary search bisect session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BisectSession {
    /// Initial snapshot of all mods and whether they were originally enabled.
    original_state: Vec<(String, bool)>,
    /// Core library mods kept enabled while bisecting regular mods.
    #[serde(default)]
    core_libraries: Vec<String>,
    /// Core library mods waiting to be bisected if non-library suspects all pass.
    #[serde(default)]
    library_suspects: Vec<String>,
    /// Current pool of suspects that might contain the crashing mod.
    suspects: Vec<String>,
    /// The active half currently left enabled for testing.
    active_half: Vec<String>,
    /// Current bisect round number (1-based).
    round: usize,
    /// Isolated culprit mod filename once identified.
    culprit: Option<String>,
}

impl BisectSession {
    fn state_path(mods_dir: &Path) -> PathBuf {
        mods_dir.join(BISECT_STATE_FILE)
    }

    fn save_state(&self, mods_dir: &Path) -> MqResult<()> {
        let path = Self::state_path(mods_dir);
        let data = serde_json::to_vec_pretty(self)
            .map_err(|e| MqError::Internal(format!("failed serializing bisect state: {e}")))?;
        atomic_write(&path, &data)?;
        Ok(())
    }

    fn remove_state(mods_dir: &Path) {
        let path = Self::state_path(mods_dir);
        if path.is_file() {
            let _ = remove_file_with_retry(&path);
        }
    }

    /// Starts a new bisect session by scanning `mods_dir` for all `.jar` mod files,
    /// or resumes an existing unfinished session from `<mods_dir>/.mq-bisect.json`.
    pub fn start(mods_dir: &Path) -> MqResult<Self> {
        let state_path = Self::state_path(mods_dir);
        if state_path.is_file() {
            if let Ok(raw) = fs::read_to_string(&state_path) {
                if let Ok(saved) = serde_json::from_str::<Self>(&raw) {
                    if !saved.original_state.is_empty() {
                        return Ok(saved);
                    }
                }
            }
        }

        let mut original_state = Vec::new();
        let mut enabled_mods = Vec::new();

        if mods_dir.is_dir() {
            for entry in fs::read_dir(mods_dir)?.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let file_name = entry.file_name().to_string_lossy().into_owned();
                let lower = file_name.to_ascii_lowercase();

                if lower.ends_with(".jar.disabled") {
                    let raw_name = file_name[..file_name.len() - ".disabled".len()].to_string();
                    original_state.push((raw_name, false));
                } else if lower.ends_with(".jar") {
                    original_state.push((file_name.clone(), true));
                    enabled_mods.push(file_name);
                }
            }
        }

        original_state.sort_by(|a, b| a.0.cmp(&b.0));
        enabled_mods.sort();

        if enabled_mods.is_empty() {
            return Err(MqError::Internal(
                "cannot start bisect session: no active .jar mods found in directory".to_string(),
            ));
        }

        let (libraries, non_libraries): (Vec<String>, Vec<String>) = enabled_mods
            .into_iter()
            .partition(|m| is_core_api_library(m));

        let (suspects, core_libraries, library_suspects) = if !non_libraries.is_empty() {
            (non_libraries, libraries.clone(), libraries)
        } else {
            (libraries, Vec::new(), Vec::new())
        };

        let mut session = Self {
            original_state,
            core_libraries,
            library_suspects,
            suspects,
            active_half: Vec::new(),
            round: 1,
            culprit: None,
        };

        session.partition_and_apply(mods_dir)?;
        session.save_state(mods_dir)?;
        Ok(session)
    }

    /// Number of the current test round.
    pub fn round(&self) -> usize {
        self.round
    }

    /// Number of mods currently enabled in this round's test half.
    pub fn active_half_len(&self) -> usize {
        self.active_half.len()
    }

    /// Total number of remaining suspect mods.
    pub fn suspects_len(&self) -> usize {
        self.suspects.len()
    }

    /// The isolated culprit mod file name, if found.
    pub fn culprit(&self) -> Option<&str> {
        self.culprit.as_deref()
    }

    fn partition_and_apply(&mut self, mods_dir: &Path) -> MqResult<()> {
        if self.suspects.len() <= 1 {
            self.culprit = self.suspects.first().cloned();
            self.active_half = self.suspects.clone();
            return Ok(());
        }

        let mid = self.suspects.len().div_ceil(2);
        self.active_half = self.suspects[..mid].to_vec();

        self.apply_active_set(mods_dir)?;
        Ok(())
    }

    fn apply_active_set(&self, mods_dir: &Path) -> MqResult<()> {
        let mut active_set: HashSet<&str> = self.active_half.iter().map(String::as_str).collect();
        for lib in &self.core_libraries {
            active_set.insert(lib.as_str());
        }

        if mods_dir.is_dir() {
            for entry in fs::read_dir(mods_dir)?.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let file_name = entry.file_name().to_string_lossy().into_owned();
                let lower = file_name.to_ascii_lowercase();

                if lower.ends_with(".jar.disabled") {
                    let base = &file_name[..file_name.len() - ".disabled".len()];
                    if active_set.contains(base) {
                        let target = mods_dir.join(base);
                        let _ = rename_with_retry(&path, &target);
                    }
                } else if lower.ends_with(".jar") && !active_set.contains(file_name.as_str()) {
                    let target = mods_dir.join(format!("{file_name}.disabled"));
                    let _ = rename_with_retry(&path, &target);
                }
            }
        }
        Ok(())
    }

    /// Called when the game crashed in this round.
    /// The crash reproduced with `active_half`, meaning the culprit is within `active_half`.
    pub fn step_crashed(&mut self, mods_dir: &Path) -> MqResult<Option<String>> {
        if self.culprit.is_some() {
            return Ok(self.culprit.clone());
        }

        self.suspects = std::mem::take(&mut self.active_half);
        if self.suspects.len() <= 1 {
            let culprit = self.suspects.first().cloned();
            self.culprit = culprit.clone();
            self.save_state(mods_dir)?;
            return Ok(culprit);
        }

        self.round += 1;
        self.partition_and_apply(mods_dir)?;
        self.save_state(mods_dir)?;
        Ok(self.culprit.clone())
    }

    /// Called when the game launched successfully in this round.
    /// The `active_half` was clean, meaning the culprit is in the other half of suspects.
    pub fn step_ok(&mut self, mods_dir: &Path) -> MqResult<Option<String>> {
        if self.culprit.is_some() {
            return Ok(self.culprit.clone());
        }

        let active_set: HashSet<String> = self.active_half.drain(..).collect();
        self.suspects.retain(|s| !active_set.contains(s));

        if self.suspects.is_empty() {
            if !self.library_suspects.is_empty() {
                // All non-library mods passed; now bisect libraries themselves.
                self.suspects = std::mem::take(&mut self.library_suspects);
                self.core_libraries.clear();
                self.round += 1;
                self.partition_and_apply(mods_dir)?;
                self.save_state(mods_dir)?;
                return Ok(self.culprit.clone());
            }
            self.culprit = None;
            self.save_state(mods_dir)?;
            return Ok(None);
        }

        if self.suspects.len() <= 1 {
            let culprit = self.suspects.first().cloned();
            self.culprit = culprit.clone();
            self.save_state(mods_dir)?;
            return Ok(culprit);
        }

        self.round += 1;
        self.partition_and_apply(mods_dir)?;
        self.save_state(mods_dir)?;
        Ok(self.culprit.clone())
    }

    /// Restores all mods in `mods_dir` to their state prior to starting the bisect session.
    pub fn restore_all(&mut self, mods_dir: &Path) -> MqResult<()> {
        for (name, was_enabled) in &self.original_state {
            let enabled_path = mods_dir.join(name);
            let disabled_path = mods_dir.join(format!("{name}.disabled"));

            if *was_enabled {
                if disabled_path.is_file() {
                    let _ = rename_with_retry(&disabled_path, &enabled_path);
                }
            } else if enabled_path.is_file() {
                let _ = rename_with_retry(&enabled_path, &disabled_path);
            }
        }
        self.culprit = None;
        Self::remove_state(mods_dir);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_bisect_finds_culprit() {
        let temp = tempdir().expect("tempdir");
        let mods_dir = temp.path().join("mods");
        fs::create_dir_all(&mods_dir).expect("create mods dir");

        for i in 1..=4 {
            fs::write(mods_dir.join(format!("mod{i}.jar")), b"jar data").expect("write mod");
        }

        let mut session = BisectSession::start(&mods_dir).expect("start bisect");
        assert_eq!(session.suspects_len(), 4);
        assert_eq!(session.round(), 1);

        // Assume mod3 is culprit.
        // Active half in round 1 will be mod1, mod2 (since 4 suspects, (4+1)/2 = 2 => [mod1, mod2]).
        // Testing active half (mod1, mod2) does not crash -> step_ok!
        let outcome = session.step_ok(&mods_dir).expect("step ok");
        assert!(outcome.is_none());
        assert_eq!(session.round(), 2);
        assert_eq!(session.suspects_len(), 2); // remaining: mod3, mod4

        // Round 2 active half will be mod3.
        // Testing active half (mod3) crashes -> step_crashed!
        let culprit = session.step_crashed(&mods_dir).expect("step crashed");
        assert_eq!(culprit.as_deref(), Some("mod3.jar"));
        assert_eq!(session.culprit(), Some("mod3.jar"));

        // Restore all mods to original state
        session.restore_all(&mods_dir).expect("restore");
        for i in 1..=4 {
            assert!(mods_dir.join(format!("mod{i}.jar")).is_file());
            assert!(!mods_dir.join(format!("mod{i}.jar.disabled")).exists());
        }
        assert!(!mods_dir.join(BISECT_STATE_FILE).exists());
    }

    #[test]
    fn test_bisect_keeps_core_libraries_enabled() {
        let temp = tempdir().expect("tempdir");
        let mods_dir = temp.path().join("mods");
        fs::create_dir_all(&mods_dir).expect("create mods dir");

        fs::write(mods_dir.join("fabric-api-0.90.0.jar"), b"api").unwrap();
        fs::write(mods_dir.join("modA.jar"), b"a").unwrap();
        fs::write(mods_dir.join("modB.jar"), b"b").unwrap();

        let session = BisectSession::start(&mods_dir).expect("start");
        // Suspects should only be non-libraries: modA and modB
        assert_eq!(session.suspects_len(), 2);
        // fabric-api must still be enabled (.jar, not .jar.disabled)
        assert!(mods_dir.join("fabric-api-0.90.0.jar").is_file());
    }
}
