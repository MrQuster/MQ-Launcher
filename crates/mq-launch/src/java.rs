//! JVM runtime discovery (`/usr/lib/jvm/*`, `$JAVA_HOME`, `$PATH`), fast-path `<home>/release`
//! metadata parsing, `java -version` fallback, and automatic Java 21 / 17 / 8 selection.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use mq_core::fs_util::{
    remove_dir_all_with_retry, rename_with_retry, strip_windows_verbatim_prefix,
};
use mq_core::{MqError, MqResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Standard system JVM installation root directories probed on Linux.
pub const SYSTEM_JVM_DIRS: [&str; 2] = ["/usr/lib/jvm", "/usr/lib64/jvm"];

#[cfg(windows)]
fn configure_hidden_std_command(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_hidden_std_command(_cmd: &mut Command) {}

/// Canonicalizes `path` and strips any Windows verbatim `\\?\` / `\\?\UNC\` prefix so paths
/// remain safe for Java (`-cp`, `-Djava.library.path`, `-javaagent`) and display.
fn clean_canonicalize(path: &Path) -> PathBuf {
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    strip_windows_verbatim_prefix(&resolved)
}

fn java_executable_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["java.exe", "javaw.exe", "java"]
    } else {
        &["java"]
    }
}

/// Finds the preferred Java executable inside a JVM `bin/` directory (`java.exe` preferred over
/// `javaw.exe` on Windows so log pipes and `-version` probes work cleanly with `CREATE_NO_WINDOW`).
fn find_preferred_java_in_dir(dir: &Path) -> Option<PathBuf> {
    for &bin_name in java_executable_names() {
        let candidate = dir.join(bin_name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Scans a JVM root directory up to `max_depth` levels deep for `<home>/bin/java[.exe]` or
/// `<home>/jre/bin/java[.exe]`, supporting standard vendor folders, Scoop (`current/bin`), and
/// Mojang's nested Minecraft Launcher `runtime/<component>/<platform>/<component>/bin` layout.
fn collect_java_bins_under_root(
    root: &Path,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<PathBuf>,
) {
    if depth > max_depth || !root.is_dir() {
        return;
    }

    // Check direct executable inside root (e.g. Oracle `javapath\java.exe`).
    if let Some(direct) = find_preferred_java_in_dir(root) {
        out.push(direct);
    }

    // Check standard `<root>/bin/java[.exe]` and `<root>/jre/bin/java[.exe]`.
    let bin_dir = root.join("bin");
    if let Some(found) = find_preferred_java_in_dir(&bin_dir) {
        out.push(found);
        // Once we found a JVM home, don't descend into its internal directories (`lib/`, `jmods/`, etc.).
        let jre_bin = root.join("jre").join("bin");
        if let Some(jre_found) = find_preferred_java_in_dir(&jre_bin) {
            out.push(jre_found);
        }
        return;
    }
    let jre_bin = root.join("jre").join("bin");
    if let Some(jre_found) = find_preferred_java_in_dir(&jre_bin) {
        out.push(jre_found);
        return;
    }

    if depth == max_depth {
        return;
    }

    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut dir_paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|entry| entry.path()))
        .filter(|p| p.is_dir())
        .collect();
    dir_paths.sort();

    for entry_path in dir_paths {
        if let Some(name) = entry_path.file_name().and_then(|n| n.to_str()) {
            let lower = name.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "bin" | "lib" | "jmods" | "include" | "legal" | "conf" | "man" | "demo" | "sample"
            ) {
                continue;
            }
        }
        collect_java_bins_under_root(&entry_path, depth + 1, max_depth, out);
    }
}

/// Discovered Java Virtual Machine installation on the local system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaRuntime {
    /// Canonical path to the `java` executable (`<home>/bin/java`).
    pub path: PathBuf,
    /// Root directory of the JVM installation (`<home>`).
    pub home_dir: PathBuf,
    /// Parsed major Java feature version (e.g. `8`, `17`, `21`).
    pub major_version: u32,
    /// Full version string (e.g. `"21.0.13"` or `"1.8.0_412"`).
    pub full_version: String,
    /// Implementor or vendor label (e.g. `"Arch Linux"`, `"Eclipse Adoptium"`, `"OpenJDK"`).
    pub vendor: String,
}

/// Discovers all installed Java runtimes across system directories, `~/.local/share/mq/java/*`
/// (or Windows `%LOCALAPPDATA%\MQ\java`), `$JAVA_HOME`, and `$PATH`.
///
/// Deduplicates runtimes by canonical binary path and sorts deterministically by
/// `(major_version, path)`.
pub fn discover_java_runtimes() -> Vec<JavaRuntime> {
    let mut jvm_roots: Vec<PathBuf> = Vec::new();

    #[cfg(target_os = "windows")]
    {
        let vendor_subdirs = [
            "Java",
            "Eclipse Adoptium",
            "AdoptOpenJDK",
            "Microsoft",
            "Zulu",
            "BellSoft",
            "Amazon Corretto",
            "Semeru",
            "SapMachine",
            "GraalVM",
        ];

        for env_var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Ok(dir) = env::var(env_var) {
                let base = PathBuf::from(dir);
                for sub in vendor_subdirs {
                    jvm_roots.push(base.join(sub));
                }
            }
        }

        if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
            let local_base = PathBuf::from(local_app_data);
            let programs = local_base.join("Programs");
            for sub in vendor_subdirs {
                jvm_roots.push(programs.join(sub));
            }
            // Microsoft Store / UWP Minecraft Launcher bundled runtimes
            jvm_roots.push(
                local_base
                    .join("Packages")
                    .join("Microsoft.4297127D64EC6_8wekyb3d8bbwe")
                    .join("LocalCache")
                    .join("Local")
                    .join("runtime"),
            );
        }

        // Legacy/MSI Minecraft Launcher bundled runtimes
        for pf_var in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Ok(pf) = env::var(pf_var) {
                let base = PathBuf::from(pf);
                jvm_roots.push(base.join("Minecraft Launcher").join("runtime"));
                jvm_roots.push(
                    base.join("Common Files")
                        .join("Oracle")
                        .join("Java")
                        .join("javapath"),
                );
            }
        }

        // Scoop & Chocolatey / Oracle javapath
        if let Ok(profile) = env::var("USERPROFILE") {
            jvm_roots.push(PathBuf::from(profile).join("scoop").join("apps"));
        }
        if let Ok(program_data) = env::var("ProgramData") {
            let pd = PathBuf::from(program_data);
            jvm_roots.push(pd.join("Oracle").join("Java").join("javapath"));
            jvm_roots.push(pd.join("chocolatey").join("lib"));
        }

        if let Ok(mq_data) = env::var("MQ_DATA_DIR") {
            let custom_java = PathBuf::from(mq_data).join("java");
            if custom_java.exists() {
                jvm_roots.push(custom_java);
            }
        }
        let mq_java = mq_core::platform::default_data_dir().join("java");
        if mq_java.exists() {
            jvm_roots.push(mq_java);
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        jvm_roots.extend(SYSTEM_JVM_DIRS.iter().map(PathBuf::from));
        if let Ok(mq_data) = env::var("MQ_DATA_DIR") {
            let custom_java = PathBuf::from(mq_data).join("java");
            if custom_java.exists() {
                jvm_roots.push(custom_java);
            }
        }
        let mq_java = mq_core::platform::default_data_dir().join("java");
        if mq_java.exists() {
            jvm_roots.push(mq_java);
        }
    }

    let roots_ref: Vec<&Path> = jvm_roots.iter().map(PathBuf::as_path).collect();
    let java_home = env::var_os("JAVA_HOME").map(PathBuf::from);
    let path_env = env::var_os("PATH");

    discover_java_runtimes_from_sources(&roots_ref, java_home.as_deref(), path_env.as_deref())
}

/// Pure/testable JVM discovery helper accepting explicit search roots, `JAVA_HOME`, and `PATH`.
pub fn discover_java_runtimes_from_sources(
    jvm_roots: &[&Path],
    java_home: Option<&Path>,
    path_env: Option<&OsStr>,
) -> Vec<JavaRuntime> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1. Scan JVM roots (supports direct `<root>/<jdk>/bin/java`, Scoop `<app>/current/bin/java.exe`,
    //    and Mojang's nested `runtime/<component>/<platform>/<component>/bin/java.exe`).
    for root in jvm_roots {
        collect_java_bins_under_root(root, 0, 4, &mut candidates);
    }

    // 2. Check $JAVA_HOME
    if let Some(home) = java_home.filter(|p| !p.as_os_str().is_empty()) {
        if let Some(bin_java) = find_preferred_java_in_dir(&home.join("bin")) {
            candidates.push(bin_java);
        } else if let Some(jre_java) = find_preferred_java_in_dir(&home.join("jre").join("bin")) {
            candidates.push(jre_java);
        } else if is_executable_file(home) {
            candidates.push(home.to_path_buf());
        }
    }

    // 3. Scan $PATH entries (prefer `java.exe` over `javaw.exe` within each directory)
    if let Some(paths_os) = path_env.filter(|p| !p.is_empty()) {
        for dir in env::split_paths(paths_os) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            if let Some(bin_java) = find_preferred_java_in_dir(&dir) {
                candidates.push(bin_java);
            }
        }
    }

    // Deduplicate by cleaned canonical path and probe each unique binary once.
    let mut by_canon_path: BTreeMap<PathBuf, JavaRuntime> = BTreeMap::new();
    for candidate in candidates {
        let mut canon = clean_canonicalize(&candidate);
        // On Windows, if a symlink/shortcut resolved to `javaw.exe` while sibling `java.exe` exists,
        // prefer `java.exe` so stdout/stderr pipes work cleanly with `CREATE_NO_WINDOW`.
        if cfg!(target_os = "windows")
            && canon
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("javaw.exe"))
        {
            let sibling_java = canon.with_file_name("java.exe");
            if is_executable_file(&sibling_java) {
                canon = clean_canonicalize(&sibling_java);
            }
        }
        if by_canon_path.contains_key(&canon) {
            continue;
        }
        if let Some(runtime) = probe_java_binary(&canon) {
            by_canon_path.insert(canon, runtime);
        }
    }

    let mut runtimes: Vec<JavaRuntime> = by_canon_path.into_values().collect();
    runtimes.sort_by(|a, b| {
        a.major_version
            .cmp(&b.major_version)
            .then_with(|| a.path.cmp(&b.path))
    });
    runtimes
}

/// Probes a single `java` binary (or JVM home directory), preferring the fast-path
/// `<home>/release` file before falling back to spawning `<java_bin> -version`.
pub fn probe_java_binary(java_bin_or_home: &Path) -> Option<JavaRuntime> {
    let raw_bin = if java_bin_or_home.is_dir() {
        find_preferred_java_in_dir(&java_bin_or_home.join("bin"))
            .or_else(|| find_preferred_java_in_dir(&java_bin_or_home.join("jre").join("bin")))
            .or_else(|| find_preferred_java_in_dir(java_bin_or_home))?
    } else {
        java_bin_or_home.to_path_buf()
    };

    if !raw_bin.is_file() {
        return None;
    }

    let canon_bin = clean_canonicalize(&raw_bin);
    let home_dir = infer_jvm_home(&canon_bin);

    // 1. Fast path: check `<home_dir>/release` (or parent `<home_dir>/../release` for `jre/bin/java`).
    let mut release_candidates = vec![home_dir.join("release")];
    if let Some(parent_home) = home_dir.parent() {
        release_candidates.push(parent_home.join("release"));
    }

    for rel_path in release_candidates {
        if let Ok(content) = fs::read_to_string(&rel_path) {
            if let Some((major_version, full_version, vendor)) = parse_java_release_file(&content) {
                let effective_home = rel_path
                    .parent()
                    .map(strip_windows_verbatim_prefix)
                    .unwrap_or_else(|| home_dir.clone());
                return Some(JavaRuntime {
                    path: canon_bin,
                    home_dir: effective_home,
                    major_version,
                    full_version,
                    vendor,
                });
            }
        }
    }

    // 2. Fallback: run `<probe_bin> -version` with CREATE_NO_WINDOW on Windows.
    // On Windows, `javaw.exe -version` produces no console output, so probe via sibling `java.exe` if present.
    let probe_bin = if cfg!(target_os = "windows")
        && canon_bin
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("javaw.exe"))
    {
        let sibling = canon_bin.with_file_name("java.exe");
        if sibling.is_file() {
            sibling
        } else {
            canon_bin.clone()
        }
    } else {
        canon_bin.clone()
    };

    let mut cmd = Command::new(&probe_bin);
    cmd.arg("-version");
    configure_hidden_std_command(&mut cmd);
    if let Ok(output) = cmd.output() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = if stderr.trim().is_empty() {
            stdout.into_owned()
        } else if stdout.trim().is_empty() {
            stderr.into_owned()
        } else {
            format!("{stderr}\n{stdout}")
        };

        if let Some((major_version, full_version, vendor)) = parse_java_version_output(&combined) {
            return Some(JavaRuntime {
                path: canon_bin,
                home_dir,
                major_version,
                full_version,
                vendor,
            });
        }
    }

    None
}

/// Parses an OpenJDK `<jvm_home>/release` key-value file into `(major_version, full_version, vendor)`.
///
/// Supports both modern versions (`JAVA_VERSION="21.0.13"`, `JAVA_VERSION="17.0.21"`) and
/// legacy Java 8 versions (`JAVA_VERSION="1.8.0_412"`).
pub fn parse_java_release_file(content: &str) -> Option<(u32, String, String)> {
    let mut raw_version: Option<String> = None;
    let mut implementor: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, val)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let clean_val = val.trim().trim_matches('"').trim_matches('\'').trim();
        if clean_val.is_empty() {
            continue;
        }

        match key {
            "JAVA_VERSION" => raw_version = Some(clean_val.to_owned()),
            "IMPLEMENTOR" => implementor = Some(clean_val.to_owned()),
            "VENDOR" if implementor.is_none() => implementor = Some(clean_val.to_owned()),
            _ => {}
        }
    }

    let version_str = raw_version?;
    let (major, full) = parse_java_version_str(&version_str)?;
    let vendor = implementor.unwrap_or_else(|| "OpenJDK".to_owned());
    Some((major, full, vendor))
}

/// Parses `java -version` stderr/stdout output into `(major_version, full_version, vendor)`.
pub fn parse_java_version_output(output: &str) -> Option<(u32, String, String)> {
    let mut version_tuple: Option<(u32, String)> = None;
    let mut vendor = "OpenJDK".to_owned();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if version_tuple.is_none() {
            // Standard format: `openjdk version "21.0.5" 2024-10-15` or `java version "1.8.0_412"`
            if let Some(first_quote) = trimmed.find('"') {
                let rest = &trimmed[first_quote + 1..];
                if let Some(end_quote) = rest.find('"') {
                    let quoted = &rest[..end_quote];
                    version_tuple = parse_java_version_str(quoted);
                }
            }
            if version_tuple.is_none() {
                for token in trimmed.split_whitespace() {
                    if token.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                        if let Some(parsed) = parse_java_version_str(token) {
                            version_tuple = Some(parsed);
                            break;
                        }
                    }
                }
            }
        }

        let lower = trimmed.to_ascii_lowercase();
        if lower.contains("temurin") || lower.contains("adoptium") {
            vendor = "Eclipse Adoptium".to_owned();
        } else if lower.contains("zulu") {
            vendor = "Azul Zulu".to_owned();
        } else if lower.contains("corretto") {
            vendor = "Amazon Corretto".to_owned();
        } else if lower.contains("microsoft") {
            vendor = "Microsoft".to_owned();
        } else if lower.contains("liberica") || lower.contains("bellsoft") {
            vendor = "BellSoft Liberica".to_owned();
        } else if lower.contains("semeru") || lower.contains("openj9") {
            vendor = "IBM Semeru".to_owned();
        } else if lower.contains("sapmachine") {
            vendor = "SapMachine".to_owned();
        } else if lower.contains("graalvm") {
            vendor = "GraalVM".to_owned();
        } else if lower.starts_with("java(tm)") {
            vendor = "Oracle".to_owned();
        }
    }

    let (major, full) = version_tuple?;
    Some((major, full, vendor))
}

/// Parses a Java version string (such as `"21.0.13"`, `"17.0.21"`, `"1.8.0_412"`, `"21-ea"`)
/// into `(major_version, cleaned_version_string)`.
pub fn parse_java_version_str(raw: &str) -> Option<(u32, String)> {
    let cleaned = raw.trim().trim_matches('"').trim_matches('\'').trim();
    if cleaned.is_empty() {
        return None;
    }

    if let Some(after_one_dot) = cleaned.strip_prefix("1.") {
        let major_digits: String = after_one_dot
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let major = major_digits.parse::<u32>().ok()?;
        if major > 0 {
            return Some((major, cleaned.to_owned()));
        }
    }

    let major_digits: String = cleaned.chars().take_while(|c| c.is_ascii_digit()).collect();
    let major = major_digits.parse::<u32>().ok()?;
    if major == 0 {
        return None;
    }
    Some((major, cleaned.to_owned()))
}

/// Returns the required Java major version for a given Minecraft version.
///
/// Priority 1 rule: If `spec_hint` is provided (from `version.json`'s `javaVersion.majorVersion`),
/// it is returned strictly without modification.
/// If `spec_hint` is `None`, falls back to inferring the Java version based on the Minecraft version:
/// - `25` for Minecraft `>= 26.x`
/// - `21` for Minecraft `>= 1.20.5`
/// - `17` for Minecraft `1.18..=1.20.4`
/// - `8` for Minecraft `<= 1.17.x`
pub fn required_java_major_for_mc(mc_version: &str, spec_hint: Option<u32>) -> u32 {
    if let Some(hint) = spec_hint {
        return hint;
    }
    infer_java_major_from_mc_version(mc_version)
}

/// Infers the expected Java major version from a Minecraft version string when `version.json` lacks
/// explicit `javaVersion` metadata.
pub fn infer_java_major_from_mc_version(mc_version: &str) -> u32 {
    let core = mc_version
        .trim()
        .split(['-', '+', ' '])
        .next()
        .unwrap_or("");
    let parts: Vec<u32> = core
        .split('.')
        .map_while(|seg| seg.parse::<u32>().ok())
        .collect();

    match parts.as_slice() {
        [major, ..] if *major >= 26 => 25,
        [1, minor, patch, ..] => {
            if *minor > 20 || (*minor == 20 && *patch >= 5) {
                21
            } else if *minor >= 18 {
                17
            } else {
                8
            }
        }
        [1, minor] => {
            if *minor > 20 {
                21
            } else if *minor >= 18 {
                17
            } else {
                8
            }
        }
        _ => 21,
    }
}

/// Selects the best matching [`JavaRuntime`] for `required_major`:
/// 1. If `override_path` is provided, uses that binary (probing its metadata if possible, or
///    wrapping it directly if it exists on disk).
/// 2. Picks an exact match where `major_version == required_major`.
/// 3. Otherwise picks the closest compatible runtime with `major_version > required_major`
///    (smallest `major_version >= required_major`).
/// 4. Otherwise falls back to the highest available `major_version < required_major` with a
///    diagnostic warning.
/// 5. Returns [`MqError::JavaNotFound`] if no Java runtime is available.
pub fn select_java_runtime(
    runtimes: &[JavaRuntime],
    required_major: u32,
    override_path: Option<&Path>,
) -> MqResult<JavaRuntime> {
    if let Some(custom) = override_path.filter(|p| !p.as_os_str().is_empty()) {
        let canon_custom = clean_canonicalize(custom);
        if let Some(found) = runtimes
            .iter()
            .find(|r| r.path == custom || r.path == canon_custom || r.home_dir == canon_custom)
        {
            return Ok(found.clone());
        }

        if let Some(probed) = probe_java_binary(custom) {
            return Ok(probed);
        }

        if custom.is_file() {
            return Ok(JavaRuntime {
                path: canon_custom.clone(),
                home_dir: infer_jvm_home(&canon_custom),
                major_version: required_major,
                full_version: format!("{required_major}.0.0"),
                vendor: "Custom".to_owned(),
            });
        }

        return Err(MqError::JavaNotFound {
            required_major,
            hint: format!(
                "configured java_path override '{}' does not exist or is not a valid executable",
                custom.display()
            ),
        });
    }

    if runtimes.is_empty() {
        let hint = if cfg!(target_os = "windows") {
            format!(
                "no Java runtime found in %ProgramFiles%\\Java, %JAVA_HOME%, or %PATH%; install Temurin/OpenJDK {required_major}"
            )
        } else {
            format!(
                "no Java runtime found in /usr/lib/jvm, $JAVA_HOME, or $PATH; install jdk{required_major}-openjdk"
            )
        };
        return Err(MqError::JavaNotFound {
            required_major,
            hint,
        });
    }

    // 1. Exact major_version match (prefer the last/newest path among identical majors).
    if let Some(exact) = runtimes
        .iter()
        .rev()
        .find(|r| r.major_version == required_major)
    {
        return Ok(exact.clone());
    }

    // 2. Closest installed major_version > required_major.
    if let Some(compatible_higher) = runtimes
        .iter()
        .filter(|r| r.major_version > required_major)
        .min_by(|a, b| {
            a.major_version
                .cmp(&b.major_version)
                .then_with(|| a.path.cmp(&b.path))
        })
    {
        tracing::info!(
            required_major,
            selected_major = compatible_higher.major_version,
            path = %compatible_higher.path.display(),
            "exact Java major version not installed; selected closest newer compatible JVM"
        );
        return Ok(compatible_higher.clone());
    }

    // 3. Fallback to highest installed major_version < required_major.
    if let Some(fallback_lower) = runtimes.iter().max_by(|a, b| {
        a.major_version
            .cmp(&b.major_version)
            .then_with(|| a.path.cmp(&b.path))
    }) {
        tracing::warn!(
            required_major,
            selected_major = fallback_lower.major_version,
            path = %fallback_lower.path.display(),
            "Java {}+ not found; falling back to closest installed JVM (Java {})",
            required_major,
            fallback_lower.major_version
        );
        return Ok(fallback_lower.clone());
    }

    Err(MqError::JavaNotFound {
        required_major,
        hint: if cfg!(target_os = "windows") {
            format!("install Temurin/OpenJDK {required_major} on Windows")
        } else {
            format!("install jdk{required_major}-openjdk under /usr/lib/jvm")
        },
    })
}

fn infer_jvm_home(java_bin: &Path) -> PathBuf {
    let clean_bin = strip_windows_verbatim_prefix(java_bin);
    if let Some(bin_dir) = clean_bin.parent() {
        if bin_dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("bin"))
        {
            if let Some(home) = bin_dir.parent() {
                return home.to_path_buf();
            }
        }
        return bin_dir.to_path_buf();
    }
    clean_bin
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Verifies that `<java_bin> -version` runs successfully and parses its output into a [`JavaRuntime`].
pub fn verify_java_executable(java_bin: &Path) -> MqResult<JavaRuntime> {
    if !java_bin.is_file() {
        return Err(MqError::JavaNotFound {
            required_major: 0,
            hint: format!("Java binary '{}' does not exist", java_bin.display()),
        });
    }

    let canon_bin = clean_canonicalize(java_bin);
    let probe_bin = if cfg!(target_os = "windows")
        && canon_bin
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("javaw.exe"))
    {
        let sibling = canon_bin.with_file_name("java.exe");
        if sibling.is_file() {
            sibling
        } else {
            canon_bin.clone()
        }
    } else {
        canon_bin.clone()
    };

    let mut cmd = Command::new(&probe_bin);
    cmd.arg("-version");
    configure_hidden_std_command(&mut cmd);
    let output = cmd.output().map_err(|e| {
        MqError::Internal(format!(
            "failed to execute '{} -version': {e}",
            probe_bin.display()
        ))
    })?;

    if !output.status.success() {
        return Err(MqError::Internal(format!(
            "executing '{} -version' exited with code {:?}",
            canon_bin.display(),
            output.status.code()
        )));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = if stderr.trim().is_empty() {
        stdout.into_owned()
    } else if stdout.trim().is_empty() {
        stderr.into_owned()
    } else {
        format!("{stderr}\n{stdout}")
    };

    if let Some((major_version, full_version, vendor)) = parse_java_version_output(&combined) {
        let home_dir = infer_jvm_home(&canon_bin);
        Ok(JavaRuntime {
            path: canon_bin,
            home_dir,
            major_version,
            full_version,
            vendor,
        })
    } else {
        Err(MqError::Internal(format!(
            "could not parse java -version output from '{}':\n{combined}",
            canon_bin.display()
        )))
    }
}

/// Origin of a resolved or installed Java runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaSource {
    /// System-installed runtime from `/usr/lib/jvm`, `$JAVA_HOME`, or `$PATH`.
    System,
    /// Official Mojang JRE package (`mojang-<component>`).
    Mojang,
    /// Eclipse Adoptium Temurin JRE.
    Adoptium,
    /// Azul Zulu JRE.
    Zulu,
    /// Custom path explicitly provided by user.
    Custom,
}

/// Java runtime selected and verified through the cascading priority engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedJava {
    pub runtime: JavaRuntime,
    pub source: JavaSource,
    pub component: Option<String>,
}

/// Cascading Java runtime resolver with automatic download:
/// 1. Priority 1: Suitable system Java (`/usr/lib/jvm`, `$JAVA_HOME`, `$PATH`)
///    matching or compatible with `required_major`.
/// 2. Priority 2: Official Mojang runtime (`mojang-<component>`) for the specified
///    `component_hint` or `required_major`.
/// 3. Priority 3: Adoptium Temurin or Azul Zulu JRE (8, 17, 21, 25).
pub async fn ensure_java_runtime(
    client: &reqwest::Client,
    java_dir: &Path,
    required_major: u32,
    component_hint: Option<&str>,
    override_path: Option<&Path>,
) -> MqResult<SelectedJava> {
    // 0. Explicit override path
    if let Some(custom) = override_path.filter(|p| !p.as_os_str().is_empty()) {
        if let Ok(verified) = verify_java_executable(custom) {
            return Ok(SelectedJava {
                runtime: verified,
                source: JavaSource::Custom,
                component: None,
            });
        }
        if let Some(probed) = probe_java_binary(custom) {
            return Ok(SelectedJava {
                runtime: probed,
                source: JavaSource::Custom,
                component: None,
            });
        }
        return Err(MqError::JavaNotFound {
            required_major,
            hint: format!(
                "override java_path '{}' does not exist or failed verification",
                custom.display()
            ),
        });
    }

    // 1. Priority (1): System Java matching or compatible with required_major
    let system_runtimes = discover_java_runtimes();

    // Check exact match first
    if let Some(exact) = system_runtimes
        .iter()
        .find(|r| r.major_version == required_major)
    {
        if let Ok(verified) = verify_java_executable(&exact.path) {
            return Ok(SelectedJava {
                runtime: verified,
                source: JavaSource::System,
                component: None,
            });
        }
    }

    // Check compatible higher system Java (e.g. required 17, system has 21)
    if required_major >= 17 {
        if let Some(compatible) = system_runtimes
            .iter()
            .filter(|r| r.major_version > required_major)
            .min_by_key(|r| r.major_version)
        {
            if let Ok(verified) = verify_java_executable(&compatible.path) {
                tracing::info!(
                    required_major,
                    system_major = verified.major_version,
                    path = %verified.path.display(),
                    "using compatible newer system Java"
                );
                return Ok(SelectedJava {
                    runtime: verified,
                    source: JavaSource::System,
                    component: None,
                });
            }
        }
    }

    // 2. Priority (2): Official Mojang Java Runtime
    let comp = component_hint
        .unwrap_or_else(|| crate::mojang_java::component_for_major_version(required_major));

    match crate::mojang_java::ensure_mojang_java_runtime(client, java_dir, comp, None, None).await {
        Ok(mojang_rt) => {
            return Ok(SelectedJava {
                runtime: mojang_rt,
                source: JavaSource::Mojang,
                component: Some(comp.to_string()),
            });
        }
        Err(e) => {
            tracing::warn!(
                component = comp,
                error = %e,
                "Mojang java-runtime download failed; attempting Adoptium fallback"
            );
        }
    }

    // 3. Priority (3): Adoptium Temurin or Zulu JRE (8, 17, 21, 25)
    match download_adoptium_java(client, required_major, java_dir).await {
        Ok(adoptium_rt) => {
            return Ok(SelectedJava {
                runtime: adoptium_rt,
                source: JavaSource::Adoptium,
                component: None,
            });
        }
        Err(e) => {
            tracing::warn!(
                feature_version = required_major,
                error = %e,
                "Adoptium JRE download failed; attempting Azul Zulu fallback"
            );
        }
    }

    match download_zulu_java(client, required_major, java_dir).await {
        Ok(zulu_rt) => {
            return Ok(SelectedJava {
                runtime: zulu_rt,
                source: JavaSource::Zulu,
                component: None,
            });
        }
        Err(e) => {
            tracing::error!(
                feature_version = required_major,
                error = %e,
                "Azul Zulu JRE download failed"
            );
        }
    }

    Err(MqError::JavaNotFound {
        required_major,
        hint: format!(
            "failed to find or download Java {required_major} runtime from all sources (System, Mojang, Adoptium, Zulu)"
        ),
    })
}

/// Downloads and installs an Eclipse Temurin JRE binary from the Adoptium API v3.
/// Supported feature versions: 8, 11, 16, 17, 21, 25.
/// Target directory: `<target_parent_dir>/temurin-<version>-<arch>`
pub async fn download_adoptium_java(
    client: &reqwest::Client,
    feature_version: u32,
    target_parent_dir: &Path,
) -> MqResult<JavaRuntime> {
    if !matches!(feature_version, 8 | 11 | 16 | 17 | 21 | 25) {
        return Err(MqError::Internal(format!(
            "unsupported Java feature version: {feature_version} (supported: 8, 11, 16, 17, 21, 25)"
        )));
    }

    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        other => {
            return Err(MqError::Internal(format!(
                "unsupported CPU architecture for Temurin JRE: {other}"
            )))
        }
    };

    let target_os = if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    let bin_name = if cfg!(target_os = "windows") {
        "java.exe"
    } else {
        "java"
    };
    let dest_dir_name = format!("temurin-{feature_version}-{arch}");
    let dest_dir = target_parent_dir.join(&dest_dir_name);
    let bin_java = dest_dir.join("bin").join(bin_name);

    // If already installed and probeable, return existing
    if bin_java.is_file() {
        if let Ok(rt) = verify_java_executable(&bin_java) {
            return Ok(rt);
        }
        if let Some(rt) = probe_java_binary(&bin_java) {
            return Ok(rt);
        }
    }

    fs::create_dir_all(target_parent_dir)?;

    // 1. Fetch checksum if available
    let checksum_url = format!(
        "https://api.adoptium.net/v3/assets/latest/{feature_version}/hotspot?architecture={arch}&image_type=jre&os={target_os}"
    );
    let mut expected_checksum = None;
    if let Ok(resp) = client.get(&checksum_url).send().await {
        if resp.status().is_success() {
            if let Ok(val) = resp.json::<serde_json::Value>().await {
                if let Some(arr) = val.as_array() {
                    for item in arr {
                        if let Some(cs) = item
                            .pointer("/binary/package/checksum")
                            .and_then(|v| v.as_str())
                        {
                            expected_checksum = Some(cs.trim().to_lowercase());
                            break;
                        }
                    }
                }
            }
        }
    }

    // 2. Download binary (zip for Windows, tar.gz for Linux)
    let binary_url = format!(
        "https://api.adoptium.net/v3/binary/latest/{feature_version}/ga/{target_os}/{arch}/jre/hotspot/normal/eclipse"
    );
    let resp = client
        .get(&binary_url)
        .send()
        .await
        .map_err(|e| MqError::Network(format!("failed to initiate Adoptium download: {e}")))?;

    if !resp.status().is_success() {
        return Err(MqError::Network(format!(
            "Adoptium returned HTTP status {} for Java {feature_version}",
            resp.status()
        )));
    }

    let archive_bytes = resp
        .bytes()
        .await
        .map_err(|e| MqError::Network(format!("failed to download Adoptium archive: {e}")))?;

    // 3. Verify SHA-256 if expected checksum is found
    if let Some(ref expected) = expected_checksum {
        let mut hasher = Sha256::new();
        hasher.update(&archive_bytes);
        let actual = hex::encode(hasher.finalize()).to_lowercase();
        if actual != *expected {
            return Err(MqError::Internal(format!(
                "Adoptium Java {feature_version} SHA-256 mismatch: expected {expected}, got {actual}"
            )));
        }
    }

    // 4. Safely extract archive
    let temp_extract = target_parent_dir.join(format!(
        ".tmp_extract_{feature_version}_{}",
        std::process::id()
    ));
    if temp_extract.exists() {
        let _ = remove_dir_all_with_retry(&temp_extract);
    }
    fs::create_dir_all(&temp_extract)?;

    if target_os == "windows" {
        let cursor = std::io::Cursor::new(&archive_bytes[..]);
        let mut zip_archive = zip::ZipArchive::new(cursor).map_err(|e| {
            let _ = remove_dir_all_with_retry(&temp_extract);
            MqError::Internal(format!("failed to parse Adoptium zip: {e}"))
        })?;
        zip_archive.extract(&temp_extract).map_err(|e| {
            let _ = remove_dir_all_with_retry(&temp_extract);
            MqError::Internal(format!("failed to unpack Adoptium zip archive: {e}"))
        })?;
    } else {
        let gz_decoder = flate2::read::GzDecoder::new(&archive_bytes[..]);
        let mut archive = tar::Archive::new(gz_decoder);
        archive.unpack(&temp_extract).map_err(|e| {
            let _ = remove_dir_all_with_retry(&temp_extract);
            MqError::Internal(format!("failed to unpack Adoptium archive: {e}"))
        })?;
    }

    // Adoptium tarballs extract into a single root folder (e.g. `jdk-21.0.5+11-jre`)
    let mut root_extracted = temp_extract.clone();
    if let Ok(entries) = fs::read_dir(&temp_extract) {
        let valid_entries: Vec<PathBuf> =
            entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        if valid_entries.len() == 1 && valid_entries[0].is_dir() {
            root_extracted = valid_entries[0].clone();
        }
    }

    if dest_dir.exists() {
        let _ = remove_dir_all_with_retry(&dest_dir);
    }
    rename_with_retry(&root_extracted, &dest_dir)?;
    let _ = remove_dir_all_with_retry(&temp_extract);

    // Ensure permissions
    let final_bin = dest_dir.join("bin").join(bin_name);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if final_bin.exists() {
            let _ = fs::set_permissions(&final_bin, fs::Permissions::from_mode(0o755));
        }
    }

    // Verify or probe the runtime
    verify_java_executable(&final_bin).or_else(|_| {
        probe_java_binary(&final_bin).ok_or_else(|| {
            MqError::Internal(format!(
                "installed Adoptium Java {feature_version} could not be probed at {}",
                final_bin.display()
            ))
        })
    })
}

/// Downloads and installs an Azul Zulu JRE binary.
/// Supported feature versions: 8, 11, 17, 21, 25.
/// Target directory: `<target_parent_dir>/zulu-<version>-<arch>`
pub async fn download_zulu_java(
    client: &reqwest::Client,
    feature_version: u32,
    target_parent_dir: &Path,
) -> MqResult<JavaRuntime> {
    if !matches!(feature_version, 8 | 11 | 17 | 21 | 25) {
        return Err(MqError::Internal(format!(
            "unsupported Java feature version for Zulu: {feature_version}"
        )));
    }

    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        other => {
            return Err(MqError::Internal(format!(
                "unsupported CPU architecture for Zulu JRE: {other}"
            )))
        }
    };

    let (target_os, ext, bin_name) = if cfg!(target_os = "windows") {
        ("windows", "zip", "java.exe")
    } else {
        ("linux", "tar.gz", "java")
    };

    let dest_dir = target_parent_dir.join(format!("zulu-{feature_version}-{arch}"));
    let bin_java = dest_dir.join("bin").join(bin_name);

    if bin_java.is_file() {
        if let Ok(rt) = verify_java_executable(&bin_java) {
            return Ok(rt);
        }
        if let Some(rt) = probe_java_binary(&bin_java) {
            return Ok(rt);
        }
    }

    // Query Azul Zulu packages metadata API
    let api_url = format!(
        "https://api.azul.com/metadata/v1/zulu/packages/?java_version={feature_version}&os={target_os}&arch={arch}&archive_type={ext}&java_package_type=jre"
    );

    let resp = client
        .get(&api_url)
        .send()
        .await
        .map_err(|e| MqError::Network(format!("failed to query Zulu API: {e}")))?;

    if !resp.status().is_success() {
        return Err(MqError::Network(format!(
            "Zulu API returned HTTP status {}",
            resp.status()
        )));
    }

    let val = resp
        .json::<serde_json::Value>()
        .await
        .map_err(|e| MqError::Network(format!("failed to parse Zulu JSON response: {e}")))?;

    let download_url = val
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("download_url"))
        .and_then(|u| u.as_str())
        .ok_or_else(|| {
            MqError::Network(format!(
                "no download URL found in Zulu API response for Java {feature_version}"
            ))
        })?;

    let dl_resp = client
        .get(download_url)
        .send()
        .await
        .map_err(|e| MqError::Network(format!("failed to download Zulu archive: {e}")))?;

    if !dl_resp.status().is_success() {
        return Err(MqError::Network(format!(
            "Zulu download returned HTTP status {}",
            dl_resp.status()
        )));
    }

    let archive_bytes = dl_resp
        .bytes()
        .await
        .map_err(|e| MqError::Network(format!("failed to read Zulu archive bytes: {e}")))?;

    fs::create_dir_all(target_parent_dir)?;
    let temp_extract = target_parent_dir.join(format!(
        ".tmp_extract_zulu_{feature_version}_{}",
        std::process::id()
    ));
    if temp_extract.exists() {
        let _ = remove_dir_all_with_retry(&temp_extract);
    }
    fs::create_dir_all(&temp_extract)?;

    if target_os == "windows" {
        let cursor = std::io::Cursor::new(&archive_bytes[..]);
        let mut zip_archive = zip::ZipArchive::new(cursor).map_err(|e| {
            let _ = remove_dir_all_with_retry(&temp_extract);
            MqError::Internal(format!("failed to parse Zulu zip: {e}"))
        })?;
        zip_archive.extract(&temp_extract).map_err(|e| {
            let _ = remove_dir_all_with_retry(&temp_extract);
            MqError::Internal(format!("failed to unpack Zulu zip archive: {e}"))
        })?;
    } else {
        let gz_decoder = flate2::read::GzDecoder::new(&archive_bytes[..]);
        let mut archive = tar::Archive::new(gz_decoder);
        archive.unpack(&temp_extract).map_err(|e| {
            let _ = remove_dir_all_with_retry(&temp_extract);
            MqError::Internal(format!("failed to unpack Zulu archive: {e}"))
        })?;
    }

    let mut root_extracted = temp_extract.clone();
    if let Ok(entries) = fs::read_dir(&temp_extract) {
        let valid_entries: Vec<PathBuf> =
            entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        if valid_entries.len() == 1 && valid_entries[0].is_dir() {
            root_extracted = valid_entries[0].clone();
        }
    }

    if dest_dir.exists() {
        let _ = remove_dir_all_with_retry(&dest_dir);
    }
    rename_with_retry(&root_extracted, &dest_dir)?;
    let _ = remove_dir_all_with_retry(&temp_extract);

    let final_bin = dest_dir.join("bin").join(bin_name);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if final_bin.exists() {
            let _ = fs::set_permissions(&final_bin, fs::Permissions::from_mode(0o755));
        }
    }

    verify_java_executable(&final_bin).or_else(|_| {
        probe_java_binary(&final_bin).ok_or_else(|| {
            MqError::Internal(format!(
                "installed Zulu Java {feature_version} could not be probed at {}",
                final_bin.display()
            ))
        })
    })
}
