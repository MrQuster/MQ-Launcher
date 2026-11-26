//! Forge installer download, profile extraction, and post-download processor execution.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use mq_core::fs_util::strip_windows_verbatim_prefix;
use mq_core::{ModLoaderKind, MqError, MqPaths, MqResult};
use mq_meta::{
    fetch_forge_recommended_version, fetch_neoforge_loader_versions, forge_installer_url,
    maven_coord_to_artifact, merge_inherits_from, neoforge_installer_url, normalize_forge_version,
    parse_forge_install_profile, parse_loader_profile, ForgeInstallProfile, ResolvedLibrary,
    ResolvedVersion, RuleContext,
};

/// Prepared Forge installation artifacts extracted from the Forge installer JAR.
pub struct PreparedForge {
    pub installer_jar_path: PathBuf,
    pub resolved_loader_version: String,
    pub install_profile: ForgeInstallProfile,
    pub installer_libraries: Vec<ResolvedLibrary>,
    pub extracted_data_files: HashMap<String, PathBuf>,
}

/// Validates that `raw` is a safe relative archive path (no `..`, root, NUL, or Windows drive prefix).
fn validate_safe_rel_path(raw: &str) -> MqResult<PathBuf> {
    if raw.is_empty() || raw.contains('\0') || raw.starts_with('/') || raw.starts_with('\\') {
        return Err(MqError::ZipSlip {
            entry: raw.to_owned(),
        });
    }
    if raw.len() >= 2 && raw.as_bytes()[0].is_ascii_alphabetic() && raw.as_bytes()[1] == b':' {
        return Err(MqError::ZipSlip {
            entry: raw.to_owned(),
        });
    }
    let normalized = raw.replace('\\', "/");
    let mut out = PathBuf::new();
    for comp in Path::new(&normalized).components() {
        match comp {
            Component::Normal(seg) => out.push(seg),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(MqError::ZipSlip {
                    entry: raw.to_owned(),
                });
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(MqError::ZipSlip {
            entry: raw.to_owned(),
        });
    }
    Ok(out)
}

/// Reads an archive entry into a UTF-8 string.
fn read_zip_entry_string(
    archive: &mut zip::ZipArchive<fs::File>,
    entry_name: &str,
) -> MqResult<String> {
    let mut file = archive.by_name(entry_name).map_err(|e| {
        MqError::Archive(format!(
            "missing '{entry_name}' in Forge installer JAR: {e}"
        ))
    })?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).map_err(|e| {
        MqError::Archive(format!(
            "failed reading '{entry_name}' from Forge installer JAR: {e}"
        ))
    })?;
    Ok(buf)
}

/// Reads an archive entry into raw bytes.
fn read_zip_entry_bytes(
    archive: &mut zip::ZipArchive<fs::File>,
    entry_name: &str,
) -> MqResult<Vec<u8>> {
    let mut file = archive.by_name(entry_name).map_err(|e| {
        MqError::Archive(format!(
            "missing '{entry_name}' in Forge installer JAR: {e}"
        ))
    })?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).map_err(|e| {
        MqError::Archive(format!(
            "failed reading '{entry_name}' from Forge installer JAR: {e}"
        ))
    })?;
    Ok(buf)
}

/// Downloads the Forge installer JAR, extracts `version.json` and `install_profile.json`,
/// merges the Forge profile into `resolved_ver`, extracts bundled `maven/` artifacts and
/// `data` files, and returns the prepared processor plan.
pub async fn prepare_forge_installation(
    client: &reqwest::Client,
    paths: &MqPaths,
    installing_dir: &Path,
    mc_version: &str,
    requested_loader_version: Option<&str>,
    mock_base_url: Option<&str>,
    resolved_ver: &mut ResolvedVersion,
) -> MqResult<PreparedForge> {
    prepare_installer_loader(
        ModLoaderKind::Forge,
        client,
        paths,
        installing_dir,
        mc_version,
        requested_loader_version,
        mock_base_url,
        resolved_ver,
    )
    .await
}

/// Downloads the NeoForge installer JAR, extracts `version.json` and `install_profile.json`,
/// merges the NeoForge profile into `resolved_ver`, extracts bundled `maven/` artifacts and
/// `data` files, and returns the prepared processor plan.
pub async fn prepare_neoforge_installation(
    client: &reqwest::Client,
    paths: &MqPaths,
    installing_dir: &Path,
    mc_version: &str,
    requested_loader_version: Option<&str>,
    mock_base_url: Option<&str>,
    resolved_ver: &mut ResolvedVersion,
) -> MqResult<PreparedForge> {
    prepare_installer_loader(
        ModLoaderKind::NeoForge,
        client,
        paths,
        installing_dir,
        mc_version,
        requested_loader_version,
        mock_base_url,
        resolved_ver,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn prepare_installer_loader(
    loader: ModLoaderKind,
    client: &reqwest::Client,
    paths: &MqPaths,
    installing_dir: &Path,
    mc_version: &str,
    requested_loader_version: Option<&str>,
    mock_base_url: Option<&str>,
    resolved_ver: &mut ResolvedVersion,
) -> MqResult<PreparedForge> {
    let (short_ver, installer_url, installer_filename) = match loader {
        ModLoaderKind::Forge => {
            let raw_ver = match requested_loader_version {
                Some(v) if !v.trim().is_empty() => v.trim().to_owned(),
                _ => fetch_forge_recommended_version(client, mock_base_url, mc_version).await?,
            };
            let (short_ver, _full_ver) = normalize_forge_version(mc_version, &raw_ver)?;
            let url = forge_installer_url(mock_base_url, mc_version, &short_ver)?;
            (short_ver, url, "forge-installer.jar")
        }
        ModLoaderKind::NeoForge => {
            let ver = match requested_loader_version {
                Some(v) if !v.trim().is_empty() => v.trim().to_owned(),
                _ => {
                    let versions =
                        fetch_neoforge_loader_versions(client, mock_base_url, mc_version).await?;
                    versions.into_iter().next().ok_or_else(|| {
                        MqError::Metadata(format!(
                            "no NeoForge loader version found for {mc_version}"
                        ))
                    })?
                }
            };
            let url = neoforge_installer_url(mock_base_url, &ver)?;
            (ver, url, "neoforge-installer.jar")
        }
        other => {
            return Err(MqError::Metadata(format!(
                "unsupported installer-based loader: {other:?}"
            )));
        }
    };

    tracing::info!(loader = ?loader, url = %installer_url, version = %short_ver, "downloading loader installer");

    let resp =
        client.get(&installer_url).send().await.map_err(|e| {
            MqError::Network(format!("failed to download {loader:?} installer: {e}"))
        })?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(MqError::HttpStatus {
            status: status.as_u16(),
            url: installer_url,
            body: body.chars().take(512).collect(),
        });
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| MqError::Network(format!("failed to read {loader:?} installer body: {e}")))?;

    let installer_jar_path = installing_dir.join(installer_filename);
    fs::write(&installer_jar_path, &bytes)?;

    let file = fs::File::open(&installer_jar_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| MqError::Archive(format!("invalid {loader:?} installer JAR: {e}")))?;

    let version_json = read_zip_entry_string(&mut archive, "version.json")?;
    let profile = parse_loader_profile(&version_json, loader, Some(short_ver.clone()))?;
    *resolved_ver = merge_inherits_from(resolved_ver, &profile)?;
    resolved_ver.loader = loader;
    resolved_ver.loader_version = Some(short_ver.clone());

    let install_profile_json = read_zip_entry_string(&mut archive, "install_profile.json")?;
    let install_profile = parse_forge_install_profile(&install_profile_json)?;

    // Extract any bundled `maven/...` files into `libraries/`.
    let libs_dir = paths.libraries_dir();
    let entry_names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_owned()))
        .collect();

    for name in entry_names {
        let normalized = name.replace('\\', "/");
        if normalized.ends_with('/') {
            continue;
        }
        if let Some(rel_after_maven) = normalized.strip_prefix("maven/") {
            if rel_after_maven.is_empty() {
                continue;
            }
            let _safe_rel = validate_safe_rel_path(rel_after_maven)?;
            let dest = mq_core::resolve_safe_path(&libs_dir, rel_after_maven)?;
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let data = read_zip_entry_bytes(&mut archive, &name)?;
            mq_core::platform::atomic_write(&dest, &data)?;
        }
    }

    // Extract any data entries starting with `/` (e.g., `/data/client.lzma`) into `.installing/forge-data/`.
    let forge_data_dir = installing_dir.join("forge-data");
    let mut extracted_data_files = HashMap::new();
    for entry in install_profile.data.values() {
        let val = entry.client.trim();
        if let Some(archive_rel) = val.strip_prefix('/') {
            if extracted_data_files.contains_key(val) {
                continue;
            }
            let _safe_rel = validate_safe_rel_path(archive_rel)?;
            let dest = mq_core::resolve_safe_path(&forge_data_dir, archive_rel)?;
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let data = read_zip_entry_bytes(&mut archive, archive_rel)?;
            fs::write(&dest, &data)?;
            extracted_data_files.insert(val.to_owned(), strip_windows_verbatim_prefix(&dest));
        }
    }

    let rule_ctx = RuleContext::current();
    let mut installer_libraries = Vec::with_capacity(install_profile.libraries.len());
    for raw_lib in &install_profile.libraries {
        installer_libraries.push(raw_lib.to_resolved_library_for_context(loader, &rule_ctx)?);
    }

    Ok(PreparedForge {
        installer_jar_path,
        resolved_loader_version: short_ver,
        install_profile,
        installer_libraries,
        extracted_data_files,
    })
}

/// Extracts `Main-Class` from `META-INF/MANIFEST.MF` inside a processor JAR.
fn read_jar_main_class(jar_path: &Path) -> MqResult<String> {
    let file = fs::File::open(jar_path).map_err(|e| {
        MqError::Archive(format!(
            "failed to open Forge processor JAR {}: {e}",
            jar_path.display()
        ))
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        MqError::Archive(format!(
            "invalid Forge processor JAR {}: {e}",
            jar_path.display()
        ))
    })?;
    let mut manifest = archive.by_name("META-INF/MANIFEST.MF").map_err(|e| {
        MqError::Archive(format!(
            "missing META-INF/MANIFEST.MF in {}: {e}",
            jar_path.display()
        ))
    })?;
    let mut text = String::new();
    manifest.read_to_string(&mut text)?;

    // Unfold RFC822/JAR manifest continuation lines (`\r\n ` or `\n `).
    let unfolded = text.replace("\r\n ", "").replace("\n ", "");
    for line in unfolded.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("Main-Class:") {
            let mc = val.trim();
            if !mc.is_empty() {
                return Ok(mc.to_owned());
            }
        }
    }

    Err(MqError::Archive(format!(
        "Main-Class not found in MANIFEST.MF of {}",
        jar_path.display()
    )))
}

/// Resolves a `[group:artifact:version[:classifier][@ext]]` bracketed Maven coordinate
/// into an absolute path inside `libs_dir` using native OS separators and without Windows `\\?\`.
fn resolve_maven_bracketed_path(coord: &str, libs_dir: &Path) -> MqResult<PathBuf> {
    let art = maven_coord_to_artifact(coord, None, None, 0)?;
    let mut path = strip_windows_verbatim_prefix(libs_dir);
    for seg in art.path.split('/') {
        if !seg.is_empty() {
            path.push(seg);
        }
    }
    Ok(path)
}

/// Resolves a single Forge processor argument template.
fn resolve_processor_arg(
    raw_arg: &str,
    mc_version: &str,
    instance_dir: &Path,
    minecraft_jar: &Path,
    installer_jar: &Path,
    libs_dir: &Path,
    forge: &PreparedForge,
) -> MqResult<String> {
    let mut s = raw_arg.to_owned();
    let clean_mc_jar = strip_windows_verbatim_prefix(minecraft_jar);
    let clean_instance_dir = strip_windows_verbatim_prefix(instance_dir);
    let clean_installer_jar = strip_windows_verbatim_prefix(installer_jar);
    let clean_libs_dir = strip_windows_verbatim_prefix(libs_dir);

    // Built-in Forge processor variables
    let builtins: [(&str, String); 6] = [
        ("{SIDE}", "client".to_owned()),
        (
            "{MINECRAFT_JAR}",
            clean_mc_jar.to_string_lossy().into_owned(),
        ),
        ("{MINECRAFT_VERSION}", mc_version.to_owned()),
        ("{ROOT}", clean_instance_dir.to_string_lossy().into_owned()),
        (
            "{INSTALLER}",
            clean_installer_jar.to_string_lossy().into_owned(),
        ),
        (
            "{LIBRARY_DIR}",
            clean_libs_dir.to_string_lossy().into_owned(),
        ),
    ];
    for (token, replacement) in builtins {
        if s.contains(token) {
            s = s.replace(token, &replacement);
        }
    }

    // Custom `data` map placeholders
    for (key, entry) in &forge.install_profile.data {
        let token = format!("{{{key}}}");
        if !s.contains(&token) {
            continue;
        }
        let raw_val = entry.client.trim();
        let resolved_val =
            if raw_val.starts_with('[') && raw_val.ends_with(']') && raw_val.len() > 2 {
                let coord = &raw_val[1..raw_val.len() - 1];
                resolve_maven_bracketed_path(coord, &clean_libs_dir)?
                    .to_string_lossy()
                    .into_owned()
            } else if raw_val.starts_with('/') {
                let extracted = forge.extracted_data_files.get(raw_val).ok_or_else(|| {
                    MqError::Archive(format!(
                        "missing extracted Forge data file '{raw_val}' for key '{key}'"
                    ))
                })?;
                strip_windows_verbatim_prefix(extracted)
                    .to_string_lossy()
                    .into_owned()
            } else if raw_val.starts_with('\'') && raw_val.ends_with('\'') && raw_val.len() >= 2 {
                raw_val[1..raw_val.len() - 1].to_owned()
            } else {
                raw_val.to_owned()
            };
        s = s.replace(&token, &resolved_val);
    }

    // Direct `[group:artifact:version...]` argument
    let trimmed = s.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() > 2 {
        let coord = &trimmed[1..trimmed.len() - 1];
        return Ok(resolve_maven_bracketed_path(coord, &clean_libs_dir)?
            .to_string_lossy()
            .into_owned());
    }

    Ok(s)
}

const FORGE_PROCESSOR_ARGFILE_THRESHOLD: usize = 20_000;

struct TempArgfileGuard(PathBuf);

impl Drop for TempArgfileGuard {
    fn drop(&mut self) {
        let _ = mq_core::fs_util::remove_file_with_retry(&self.0);
    }
}

fn is_java_9_or_newer_for_forge(java_major: Option<u32>, mc_version: &str) -> bool {
    if let Some(major) = java_major {
        return major >= 9;
    }
    let core = mc_version
        .trim()
        .split(['-', '+', ' '])
        .next()
        .unwrap_or("");
    let nums: Vec<u32> = core.split('.').filter_map(|s| s.parse().ok()).collect();
    match nums.as_slice() {
        [major, ..] if *major >= 2 => true,
        [1, minor, ..] => *minor > 12,
        _ => false,
    }
}

fn prepare_forge_processor_cp_args(
    classpath: &str,
    java_major: Option<u32>,
    mc_version: &str,
    argfile_dir: &Path,
    proc_idx: usize,
) -> MqResult<(Vec<String>, Option<TempArgfileGuard>)> {
    if classpath.len() > FORGE_PROCESSOR_ARGFILE_THRESHOLD
        && is_java_9_or_newer_for_forge(java_major, mc_version)
    {
        let base_dir = if argfile_dir.is_dir() {
            strip_windows_verbatim_prefix(argfile_dir)
        } else {
            strip_windows_verbatim_prefix(&std::env::temp_dir())
        };
        let argfile_path = base_dir.join(format!(
            ".mq-forge-proc-{}-{proc_idx}.argfile",
            std::process::id()
        ));
        let content = format!(
            "{}\n{}\n",
            mq_launch::command::format_java_argfile_token("-cp"),
            mq_launch::command::format_java_argfile_token(classpath)
        );
        fs::write(&argfile_path, content.as_bytes())?;
        let argfile_flag = format!("@{}", argfile_path.display());
        Ok((vec![argfile_flag], Some(TempArgfileGuard(argfile_path))))
    } else {
        Ok((vec!["-cp".to_owned(), classpath.to_owned()], None))
    }
}

/// Runs all client-side Forge `install_profile.json` processors sequentially using `java_path`.
pub async fn run_forge_processors(
    java_path: &Path,
    paths: &MqPaths,
    instance_dir: &Path,
    mc_version: &str,
    minecraft_jar: &Path,
    forge: &PreparedForge,
) -> MqResult<()> {
    let clean_java_path = strip_windows_verbatim_prefix(java_path);
    let java_major = mq_launch::probe_java_binary(&clean_java_path).map(|rt| rt.major_version);
    let libs_dir = strip_windows_verbatim_prefix(&paths.libraries_dir());
    let clean_instance_dir = strip_windows_verbatim_prefix(instance_dir);
    let clean_mc_jar = strip_windows_verbatim_prefix(minecraft_jar);
    let clean_installer_jar = strip_windows_verbatim_prefix(&forge.installer_jar_path);
    let sep = mq_core::platform::classpath_separator();

    for (idx, proc) in forge.install_profile.processors.iter().enumerate() {
        if !proc.sides.is_empty() && !proc.sides.iter().any(|s| s == "client") {
            continue;
        }

        let jar_path = resolve_maven_bracketed_path(&proc.jar, &libs_dir)?;
        if !jar_path.is_file() {
            return Err(MqError::Archive(format!(
                "Forge processor JAR missing: {}",
                jar_path.display()
            )));
        }

        let main_class = read_jar_main_class(&jar_path)?;

        let mut cp_entries = Vec::with_capacity(1 + proc.classpath.len());
        cp_entries.push(jar_path.to_string_lossy().into_owned());
        for cp_coord in &proc.classpath {
            let cp_path = resolve_maven_bracketed_path(cp_coord, &libs_dir)?;
            cp_entries.push(cp_path.to_string_lossy().into_owned());
        }
        let mut cp_sep_buf = [0u8; 4];
        let sep_str = sep.encode_utf8(&mut cp_sep_buf);
        let classpath = cp_entries.join(sep_str);

        let mut resolved_args = Vec::with_capacity(proc.args.len());
        for raw_arg in &proc.args {
            resolved_args.push(resolve_processor_arg(
                raw_arg,
                mc_version,
                &clean_instance_dir,
                &clean_mc_jar,
                &clean_installer_jar,
                &libs_dir,
                forge,
            )?);
        }

        tracing::info!(
            processor_index = idx,
            jar = %proc.jar,
            main_class = %main_class,
            "running Forge installer processor"
        );

        let (cp_args, _argfile_guard) = prepare_forge_processor_cp_args(
            &classpath,
            java_major,
            mc_version,
            &clean_instance_dir,
            idx,
        )?;

        let mut cmd = tokio::process::Command::new(&clean_java_path);
        cmd.stdin(std::process::Stdio::null());
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
            cmd.arg("-Dfile.encoding=UTF-8");
        }
        let output = cmd
            .args(&cp_args)
            .arg(&main_class)
            .args(&resolved_args)
            .output()
            .await
            .map_err(|e| {
                MqError::Archive(format!(
                    "failed to spawn Java ({}) for Forge processor {}: {e}",
                    clean_java_path.display(),
                    proc.jar
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
            let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
            let tail_err: String = stderr
                .chars()
                .rev()
                .take(800)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            let tail_out: String = stdout
                .chars()
                .rev()
                .take(400)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            return Err(MqError::Archive(format!(
                "Forge processor {} failed with status {:?}: stderr={tail_err} stdout={tail_out}",
                proc.jar,
                output.status.code()
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mq_meta::ForgeDataEntry;
    use std::io::Write;

    #[test]
    fn resolves_processor_args_and_rejects_zip_slip() {
        assert!(validate_safe_rel_path("../evil.jar").is_err());
        assert!(validate_safe_rel_path("/abs/evil.jar").is_err());
        assert!(validate_safe_rel_path(r"C:\Windows\evil.jar").is_err());
        assert!(validate_safe_rel_path(r"\evil.jar").is_err());
        assert!(validate_safe_rel_path(r"foo\..\evil.jar").is_err());
        assert_eq!(
            validate_safe_rel_path("net/minecraftforge/forge.jar").unwrap(),
            PathBuf::from("net/minecraftforge/forge.jar")
        );
        assert_eq!(
            validate_safe_rel_path(r"net\minecraftforge\forge.jar").unwrap(),
            PathBuf::from("net/minecraftforge/forge.jar")
        );

        let mut data = HashMap::new();
        data.insert(
            "BINPATCH".to_owned(),
            ForgeDataEntry {
                client: "/data/client.lzma".to_owned(),
                server: String::new(),
            },
        );
        data.insert(
            "MC_SRG".to_owned(),
            ForgeDataEntry {
                client: "[net.minecraft:client:1.20.1:srg]".to_owned(),
                server: String::new(),
            },
        );
        data.insert(
            "SIDE_LIT".to_owned(),
            ForgeDataEntry {
                client: "'client'".to_owned(),
                server: String::new(),
            },
        );

        let mut extracted = HashMap::new();
        extracted.insert(
            "/data/client.lzma".to_owned(),
            PathBuf::from("/tmp/forge-data/data/client.lzma"),
        );

        let forge = PreparedForge {
            installer_jar_path: PathBuf::from("/tmp/forge-installer.jar"),
            resolved_loader_version: "47.3.0".to_owned(),
            install_profile: ForgeInstallProfile {
                spec: Some(1),
                minecraft: "1.20.1".to_owned(),
                version: "1.20.1-forge-47.3.0".to_owned(),
                libraries: vec![],
                data,
                processors: vec![],
            },
            installer_libraries: vec![],
            extracted_data_files: extracted,
        };

        let libs = Path::new("/data/libraries");
        let mc_jar = Path::new("/data/versions/1.20.1/1.20.1.jar");
        let inst_dir = Path::new("/data/instances/forge");

        assert_eq!(
            resolve_processor_arg(
                "{MINECRAFT_JAR}",
                "1.20.1",
                inst_dir,
                mc_jar,
                &forge.installer_jar_path,
                libs,
                &forge
            )
            .unwrap(),
            "/data/versions/1.20.1/1.20.1.jar"
        );
        assert_eq!(
            resolve_processor_arg(
                "{BINPATCH}",
                "1.20.1",
                inst_dir,
                mc_jar,
                &forge.installer_jar_path,
                libs,
                &forge
            )
            .unwrap(),
            "/tmp/forge-data/data/client.lzma"
        );
        assert_eq!(
            PathBuf::from(
                resolve_processor_arg(
                    "{MC_SRG}",
                    "1.20.1",
                    inst_dir,
                    mc_jar,
                    &forge.installer_jar_path,
                    libs,
                    &forge
                )
                .unwrap()
            ),
            PathBuf::from("/data/libraries/net/minecraft/client/1.20.1/client-1.20.1-srg.jar")
        );
        assert_eq!(
            resolve_processor_arg(
                "{SIDE_LIT}",
                "1.20.1",
                inst_dir,
                mc_jar,
                &forge.installer_jar_path,
                libs,
                &forge
            )
            .unwrap(),
            "client"
        );
    }

    #[test]
    fn reads_main_class_from_jar_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let jar_path = tmp.path().join("proc.jar");
        let file = fs::File::create(&jar_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file(
            "META-INF/MANIFEST.MF",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"Manifest-Version: 1.0\r\nMain-Class: net.minecraftforge.installertools.ConsoleTool\r\n")
            .unwrap();
        zip.finish().unwrap();

        assert_eq!(
            read_jar_main_class(&jar_path).unwrap(),
            "net.minecraftforge.installertools.ConsoleTool"
        );
    }

    #[test]
    fn forge_processor_uses_argfile_for_long_classpath_on_java9_plus_and_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let long_cp = (0..400)
            .map(|i| format!(r"C:\Users\Player\MQ\libraries\org\example\lib-{i:03}\lib-{i:03}.jar"))
            .collect::<Vec<_>>()
            .join(";");
        assert!(long_cp.len() > FORGE_PROCESSOR_ARGFILE_THRESHOLD);

        // Java 17 -> uses @argfile and cleans it up on drop
        let created_path = {
            let (args, guard) =
                prepare_forge_processor_cp_args(&long_cp, Some(17), "1.20.1", tmp.path(), 0)
                    .unwrap();
            assert_eq!(args.len(), 1);
            assert!(args[0].starts_with('@'));
            let p = PathBuf::from(args[0].trim_start_matches('@'));
            assert!(p.is_file());
            let content = fs::read_to_string(&p).unwrap();
            assert!(content.contains("-cp"));
            assert!(content.contains(r"C:\\Users\\Player\\MQ\\libraries"));
            assert!(guard.is_some());
            p
        };
        assert!(
            !created_path.exists(),
            "argfile must be deleted on guard drop"
        );

        // Java 8 -> keeps inline -cp
        let (args8, guard8) =
            prepare_forge_processor_cp_args(&long_cp, Some(8), "1.12.2", tmp.path(), 1).unwrap();
        assert_eq!(args8[0], "-cp");
        assert_eq!(args8[1], long_cp);
        assert!(guard8.is_none());

        // Short classpath -> keeps inline -cp
        let (args_short, guard_short) =
            prepare_forge_processor_cp_args("a.jar;b.jar", Some(21), "1.21.1", tmp.path(), 2)
                .unwrap();
        assert_eq!(args_short, vec!["-cp", "a.jar;b.jar"]);
        assert!(guard_short.is_none());
    }
}
