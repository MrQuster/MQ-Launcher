//! Forge installer download, profile extraction, and post-download processor execution.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use mq_core::{ModLoaderKind, MqError, MqPaths, MqResult};
use mq_meta::{
    fetch_forge_recommended_version, forge_installer_url, maven_coord_to_artifact,
    merge_inherits_from, normalize_forge_version, parse_forge_install_profile,
    parse_loader_profile, ForgeInstallProfile, ResolvedLibrary, ResolvedVersion,
};

/// Prepared Forge installation artifacts extracted from the Forge installer JAR.
pub struct PreparedForge {
    pub installer_jar_path: PathBuf,
    pub resolved_loader_version: String,
    pub install_profile: ForgeInstallProfile,
    pub installer_libraries: Vec<ResolvedLibrary>,
    pub extracted_data_files: HashMap<String, PathBuf>,
}

/// Validates that `rel` is a safe relative archive path (no `..`, root, or drive prefix).
fn validate_safe_rel_path(raw: &str) -> MqResult<PathBuf> {
    let mut out = PathBuf::new();
    for comp in Path::new(raw).components() {
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
    let raw_ver = match requested_loader_version {
        Some(v) if !v.trim().is_empty() => v.trim().to_owned(),
        _ => fetch_forge_recommended_version(client, mock_base_url, mc_version).await?,
    };
    let (short_ver, _full_ver) = normalize_forge_version(mc_version, &raw_ver)?;

    let installer_url = forge_installer_url(mock_base_url, mc_version, &short_ver)?;
    tracing::info!(url = %installer_url, version = %short_ver, "downloading Forge installer");

    let resp = client
        .get(&installer_url)
        .send()
        .await
        .map_err(|e| MqError::Network(format!("failed to download Forge installer: {e}")))?;
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
        .map_err(|e| MqError::Network(format!("failed to read Forge installer body: {e}")))?;

    let installer_jar_path = installing_dir.join("forge-installer.jar");
    fs::write(&installer_jar_path, &bytes)?;

    let file = fs::File::open(&installer_jar_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| MqError::Archive(format!("invalid Forge installer JAR: {e}")))?;

    let version_json = read_zip_entry_string(&mut archive, "version.json")?;
    let profile =
        parse_loader_profile(&version_json, ModLoaderKind::Forge, Some(short_ver.clone()))?;
    *resolved_ver = merge_inherits_from(resolved_ver, &profile)?;
    resolved_ver.loader = ModLoaderKind::Forge;
    resolved_ver.loader_version = Some(short_ver.clone());

    let install_profile_json = read_zip_entry_string(&mut archive, "install_profile.json")?;
    let install_profile = parse_forge_install_profile(&install_profile_json)?;

    // Extract any bundled `maven/...` files into `libraries/`.
    let libs_dir = paths.libraries_dir();
    let entry_names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_owned()))
        .collect();

    for name in entry_names {
        if name.ends_with('/') {
            continue;
        }
        if let Some(rel_after_maven) = name.strip_prefix("maven/") {
            if rel_after_maven.is_empty() {
                continue;
            }
            let safe_rel = validate_safe_rel_path(rel_after_maven)?;
            let dest = libs_dir.join(safe_rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let data = read_zip_entry_bytes(&mut archive, &name)?;
            fs::write(&dest, &data)?;
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
            let safe_rel = validate_safe_rel_path(archive_rel)?;
            let dest = forge_data_dir.join(safe_rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let data = read_zip_entry_bytes(&mut archive, archive_rel)?;
            fs::write(&dest, &data)?;
            extracted_data_files.insert(val.to_owned(), dest);
        }
    }

    let mut installer_libraries = Vec::with_capacity(install_profile.libraries.len());
    for raw_lib in &install_profile.libraries {
        installer_libraries.push(raw_lib.to_resolved_library_for_loader(ModLoaderKind::Forge)?);
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
/// into an absolute path inside `libs_dir` using native OS separators.
fn resolve_maven_bracketed_path(coord: &str, libs_dir: &Path) -> MqResult<PathBuf> {
    let art = maven_coord_to_artifact(coord, None, None, 0)?;
    let mut path = libs_dir.to_path_buf();
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

    // Built-in Forge processor variables
    let builtins: [(&str, String); 6] = [
        ("{SIDE}", "client".to_owned()),
        (
            "{MINECRAFT_JAR}",
            minecraft_jar.to_string_lossy().into_owned(),
        ),
        ("{MINECRAFT_VERSION}", mc_version.to_owned()),
        ("{ROOT}", instance_dir.to_string_lossy().into_owned()),
        ("{INSTALLER}", installer_jar.to_string_lossy().into_owned()),
        ("{LIBRARY_DIR}", libs_dir.to_string_lossy().into_owned()),
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
                resolve_maven_bracketed_path(coord, libs_dir)?
                    .to_string_lossy()
                    .into_owned()
            } else if raw_val.starts_with('/') {
                forge
                    .extracted_data_files
                    .get(raw_val)
                    .ok_or_else(|| {
                        MqError::Archive(format!(
                            "missing extracted Forge data file '{raw_val}' for key '{key}'"
                        ))
                    })?
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
        return Ok(resolve_maven_bracketed_path(coord, libs_dir)?
            .to_string_lossy()
            .into_owned());
    }

    Ok(s)
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
    let libs_dir = paths.libraries_dir();
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
                instance_dir,
                minecraft_jar,
                &forge.installer_jar_path,
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

        let output = tokio::process::Command::new(java_path)
            .arg("-cp")
            .arg(&classpath)
            .arg(&main_class)
            .args(&resolved_args)
            .output()
            .await
            .map_err(|e| {
                MqError::Archive(format!(
                    "failed to spawn Java ({}) for Forge processor {}: {e}",
                    java_path.display(),
                    proc.jar
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
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
        assert_eq!(
            validate_safe_rel_path("net/minecraftforge/forge.jar").unwrap(),
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
}
