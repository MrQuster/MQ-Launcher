//! Deterministic `inheritsFrom` modloader profile merging and JVM `-cp` classpath assembly
//! (Lesson E8: zero reliance on thread completion or non-deterministic hash iteration order).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use mq_core::{resolve_safe_path, ModLoaderKind, MqError, MqResult};

use crate::loaders::{maven_coord_to_artifact, LoaderProfile};
use crate::models::{
    ArtifactDownload, LoggingClientConfig, LoggingConfig, ResolvedLibrary, ResolvedVersion,
};

/// Deterministically merges a child modloader [`LoaderProfile`] onto a parent vanilla
/// [`ResolvedVersion`].
///
/// Guarantees (Lesson E8):
/// - Validates `child.inherits_from` (when present) matches `parent.mc_version` or `parent.id`.
/// - `child.main_class` overrides `parent.main_class` when non-empty.
/// - Libraries are keyed by `(group, artifact, classifier)` via [`ResolvedLibrary::module_key`]:
///   child libraries appear first in exact profile order, overriding any conflicting version in
///   `parent.libraries` (e.g. ASM / Gson / Guava) while preserving exact relative manifest order
///   for all remaining parent libraries and deduplicating exact coordinates.
/// - `jvm_arguments` and `game_arguments` concatenate `parent` followed by `child`, deduplicating
///   adjacent identical flags while preserving exact order.
pub fn merge_inherits_from(
    parent: &ResolvedVersion,
    child: &LoaderProfile,
) -> MqResult<ResolvedVersion> {
    if let Some(target) = child
        .inherits_from
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if target != parent.mc_version && target != parent.id {
            return Err(MqError::Metadata(format!(
                "loader profile '{}' inheritsFrom '{}' does not match parent version id '{}' or mc_version '{}'",
                child.id, target, parent.id, parent.mc_version
            )));
        }
    }

    let id = if !child.id.trim().is_empty() {
        child.id.clone()
    } else {
        parent.id.clone()
    };

    let loader = if child.loader != ModLoaderKind::Vanilla {
        child.loader
    } else {
        parent.loader
    };

    let loader_version = child
        .loader_version
        .clone()
        .or_else(|| parent.loader_version.clone());

    let inherits_from = child
        .inherits_from
        .clone()
        .or_else(|| parent.inherits_from.clone());

    let main_class = if !child.main_class.trim().is_empty() {
        child.main_class.clone()
    } else {
        parent.main_class.clone()
    };

    let libraries = merge_libraries_deterministically(&parent.libraries, &child.libraries);
    let jvm_arguments =
        merge_arguments_deterministically(&parent.jvm_arguments, &child.jvm_arguments);
    let game_arguments =
        merge_game_arguments_deterministically(&parent.game_arguments, &child.game_arguments);

    let asset_index = child
        .asset_index
        .as_ref()
        .filter(|ai| !ai.id.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| parent.asset_index.clone());

    let client_jar = child
        .client_jar
        .as_ref()
        .filter(|cj| !cj.path.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| parent.client_jar.clone());

    let java_version = child
        .java_version
        .clone()
        .unwrap_or_else(|| parent.java_version.clone());
    let logging = child
        .logging
        .as_ref()
        .map(|c| {
            let path = if c.file.id.trim().is_empty() {
                String::new()
            } else {
                format!("log_configs/{}", c.file.id.trim())
            };
            LoggingConfig {
                client: Some(LoggingClientConfig {
                    argument: c.argument.clone(),
                    file: ArtifactDownload {
                        path,
                        url: c.file.url.clone(),
                        sha1: c.file.sha1,
                        size: c.file.size,
                    },
                    logging_type: c.logging_type.clone(),
                }),
            }
        })
        .or_else(|| parent.logging.clone());

    Ok(ResolvedVersion {
        id,
        mc_version: parent.mc_version.clone(),
        loader,
        loader_version,
        inherits_from,
        main_class,
        java_version,
        asset_index,
        client_jar,
        libraries,
        jvm_arguments,
        game_arguments,
        logging,
    })
}

/// Deterministically merges a child [`ResolvedVersion`] (which has `inheritsFrom`)
/// onto a parent [`ResolvedVersion`].
///
/// Inherits properties from parent if not defined on child:
/// - assetIndex
/// - assets / client_jar downloads
/// - javaVersion
/// - logging
/// - mainClass
/// - libraries (child overrides duplicate modules, child libs prepended)
/// - arguments (jvm and game arguments: parent followed by child)
pub fn merge_resolved_versions(
    parent: &ResolvedVersion,
    child: &ResolvedVersion,
) -> MqResult<ResolvedVersion> {
    if let Some(target) = child
        .inherits_from
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if target != parent.mc_version && target != parent.id {
            return Err(MqError::Metadata(format!(
                "version '{}' inheritsFrom '{}' does not match parent version id '{}' or mc_version '{}'",
                child.id, target, parent.id, parent.mc_version
            )));
        }
    }

    let id = if !child.id.trim().is_empty() {
        child.id.clone()
    } else {
        parent.id.clone()
    };

    let loader = if child.loader != ModLoaderKind::Vanilla {
        child.loader
    } else {
        parent.loader
    };

    let loader_version = child
        .loader_version
        .clone()
        .or_else(|| parent.loader_version.clone());

    let inherits_from = child
        .inherits_from
        .clone()
        .or_else(|| parent.inherits_from.clone());

    let main_class = if !child.main_class.trim().is_empty() {
        child.main_class.clone()
    } else {
        parent.main_class.clone()
    };

    let java_version = if child.java_version.major_version > 0 {
        child.java_version.clone()
    } else {
        parent.java_version.clone()
    };

    let asset_index = if !child.asset_index.id.trim().is_empty() {
        child.asset_index.clone()
    } else {
        parent.asset_index.clone()
    };

    let client_jar = if (!child.client_jar.path.trim().is_empty()
        && child.client_jar.path != format!("versions/{}/{}.jar", child.id, child.id))
        || !child.client_jar.url.trim().is_empty()
    {
        child.client_jar.clone()
    } else {
        parent.client_jar.clone()
    };

    let logging = child.logging.clone().or_else(|| parent.logging.clone());

    let libraries = merge_libraries_deterministically(&parent.libraries, &child.libraries);
    let jvm_arguments =
        merge_arguments_deterministically(&parent.jvm_arguments, &child.jvm_arguments);
    let game_arguments =
        merge_game_arguments_deterministically(&parent.game_arguments, &child.game_arguments);

    Ok(ResolvedVersion {
        id,
        mc_version: parent.mc_version.clone(),
        loader,
        loader_version,
        inherits_from,
        main_class,
        java_version,
        asset_index,
        client_jar,
        libraries,
        jvm_arguments,
        game_arguments,
        logging,
    })
}

fn merge_libraries_deterministically(
    parent_libs: &[ResolvedLibrary],
    child_libs: &[ResolvedLibrary],
) -> Vec<ResolvedLibrary> {
    let total_cap = parent_libs.len() + child_libs.len();
    let mut merged = Vec::with_capacity(total_cap);
    let mut seen_artifact_modules: HashSet<(String, String, Option<String>)> =
        HashSet::with_capacity(total_cap);
    let mut seen_native_modules: HashSet<(String, String, Option<String>)> =
        HashSet::with_capacity(total_cap);
    let mut seen_fallback_coords: HashSet<String> = HashSet::new();

    let mut child_claimed_native_groups: HashSet<(String, String)> = HashSet::new();
    for lib in child_libs {
        let (group, artifact, classifier) = lib.module_key();
        if !group.is_empty()
            && !artifact.is_empty()
            && (classifier.is_none()
                || classifier
                    .as_deref()
                    .is_some_and(|c| c.starts_with("natives-")))
        {
            child_claimed_native_groups.insert((group, artifact));
        }
    }

    for (is_parent, lib) in child_libs
        .iter()
        .map(|l| (false, l))
        .chain(parent_libs.iter().map(|l| (true, l)))
    {
        let key = lib.module_key();
        let has_valid_module_key = !key.0.is_empty() && !key.1.is_empty();

        if !has_valid_module_key {
            if seen_fallback_coords.insert(lib.name.clone()) {
                merged.push(lib.clone());
            }
            continue;
        }

        let keep_artifact = lib.artifact.is_some() && seen_artifact_modules.insert(key.clone());

        let allow_native = !(is_parent
            && key.2.is_none()
            && child_claimed_native_groups.contains(&(key.0.clone(), key.1.clone())));

        let keep_native =
            allow_native && lib.native_linux.is_some() && seen_native_modules.insert(key);

        if !keep_artifact && !keep_native {
            continue;
        }

        let mut entry = lib.clone();
        if !keep_artifact {
            entry.artifact = None;
            entry.include_in_classpath = false;
        }
        if !keep_native {
            entry.native_linux = None;
        }
        merged.push(entry);
    }

    merged
}

fn merge_arguments_deterministically(parent_args: &[String], child_args: &[String]) -> Vec<String> {
    let mut merged = Vec::with_capacity(parent_args.len() + child_args.len());
    for arg in parent_args.iter().chain(child_args.iter()) {
        if merged.last() != Some(arg) {
            merged.push(arg.clone());
        }
    }
    merged
}

fn merge_game_arguments_deterministically(
    parent_args: &[String],
    child_args: &[String],
) -> Vec<String> {
    // Legacy `minecraftArguments` child profiles (e.g. Forge 1.12.2) provide a complete
    // replacement argument list containing `--username` rather than incremental `arguments.game`.
    if child_args.iter().any(|arg| arg == "--username") {
        return child_args.to_vec();
    }
    merge_arguments_deterministically(parent_args, child_args)
}

/// Returns `true` when `main_class` is a JPMS modular bootstrap entry point used by modern
/// Forge (1.17–1.21+) or NeoForge (1.20.1–1.21+), such as
/// `cpw.mods.bootstraplauncher.BootstrapLauncher` or `net.minecraftforge.bootstrap.ForgeBootstrap`.
pub fn is_modular_bootstrap_main_class(main_class: &str) -> bool {
    let trimmed = main_class.trim();
    trimmed.starts_with("cpw.mods.bootstraplauncher.")
        || trimmed.starts_with("net.minecraftforge.bootstrap.")
}

/// Returns `true` when `version` launches through a modern Forge/NeoForge modular bootstrap that
/// resolves its own patched SRG/extra Minecraft client JARs via FML (`-DlibraryDirectory`,
/// `--launchTarget`, `--fml.*`, or `-DignoreList`).
///
/// Including the vanilla obfuscated `client.jar` on `-cp` for these profiles creates a colliding
/// automatic JPMS module `client` alongside Forge's `minecraft` module.
pub fn uses_fml_managed_client_jar(version: &ResolvedVersion) -> bool {
    if !is_modular_bootstrap_main_class(&version.main_class) {
        return false;
    }
    let has_bootstrap_jvm_marker = version
        .jvm_arguments
        .iter()
        .any(|arg| arg.starts_with("-DignoreList=") || arg.starts_with("-DlibraryDirectory="));
    let has_fml_game_marker = version
        .game_arguments
        .iter()
        .any(|arg| arg == "--launchTarget" || arg.starts_with("--fml."));
    has_bootstrap_jvm_marker || has_fml_game_marker
}

/// Builds the ordered, deduplicated list of JAR paths for the JVM `-cp` classpath, placing
/// `version.libraries` (where `include_in_classpath` is `true`) in exact manifest order
/// followed by `client_jar_path` (omitted for modern Forge/NeoForge FML-managed modular
/// bootstrap profiles where `client-*-srg.jar` / `client-*-extra.jar` are loaded via FML).
pub fn build_deterministic_classpath(
    version: &ResolvedVersion,
    libraries_dir: &Path,
    client_jar_path: &Path,
) -> Vec<PathBuf> {
    let mut classpath = Vec::with_capacity(version.libraries.len() + 1);
    let mut seen = HashSet::with_capacity(version.libraries.len() + 1);

    for lib in &version.libraries {
        if !lib.include_in_classpath {
            continue;
        }

        let rel_path = lib
            .artifact
            .as_ref()
            .map(|art| art.path.as_str())
            .filter(|p| !p.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| {
                maven_coord_to_artifact(&lib.name, None, None, 0)
                    .ok()
                    .map(|art| art.path)
            });

        if let Some(rel) = rel_path {
            if let Ok(full_path) = resolve_safe_path(libraries_dir, &rel) {
                if seen.insert(full_path.clone()) {
                    classpath.push(full_path);
                }
            }
        }
    }

    if !client_jar_path.as_os_str().is_empty() && !uses_fml_managed_client_jar(version) {
        let client_buf = client_jar_path.to_path_buf();
        if seen.insert(client_buf.clone()) {
            classpath.push(client_buf);
        }
    }

    classpath
}

/// Joins ordered classpath entries using the platform-specific JVM separator (`;` on Windows, `:` on Unix).
pub fn format_classpath_string<P: AsRef<Path>>(entries: &[P]) -> String {
    format_classpath_string_separator(entries, mq_core::platform::classpath_separator())
}

/// Joins ordered classpath entries with an explicit separator character,
/// stripping any Windows `\\?\` verbatim prefixes that break Java `-cp`.
pub fn format_classpath_string_separator<P: AsRef<Path>>(entries: &[P], sep: char) -> String {
    let mut sep_buf = [0u8; 4];
    let sep_str = sep.encode_utf8(&mut sep_buf);
    entries
        .iter()
        .map(|entry| {
            mq_core::fs_util::strip_windows_verbatim_prefix(entry.as_ref())
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>()
        .join(sep_str)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use mq_core::ModLoaderKind;

    use super::*;
    use crate::loaders::{maven_coord_to_artifact, parse_loader_profile, LoaderProfile};
    use crate::models::{
        ArtifactDownload, AssetIndexPointer, JavaVersionSpec, ResolvedLibrary, ResolvedVersion,
    };

    fn make_lib(coord: &str, base_url: Option<&str>, include_in_cp: bool) -> ResolvedLibrary {
        let artifact = maven_coord_to_artifact(coord, base_url, None, 1024).expect("valid coord");
        ResolvedLibrary {
            name: coord.to_owned(),
            artifact: Some(artifact),
            native_linux: None,
            extract_exclude: Vec::new(),
            include_in_classpath: include_in_cp,
        }
    }

    fn sample_vanilla_parent() -> ResolvedVersion {
        ResolvedVersion {
            id: "1.21.1".to_owned(),
            mc_version: "1.21.1".to_owned(),
            loader: ModLoaderKind::Vanilla,
            loader_version: None,
            inherits_from: None,
            main_class: "net.minecraft.client.main.Main".to_owned(),
            java_version: JavaVersionSpec {
                component: "java-runtime-delta".to_owned(),
                major_version: 21,
            },
            asset_index: AssetIndexPointer {
                id: "17".to_owned(),
                sha1: None,
                size: 4096,
                total_size: 1_048_576,
                url: "https://piston-meta.mojang.com/v1/packages/hash/17.json".to_owned(),
            },
            client_jar: ArtifactDownload {
                path: "versions/1.21.1/1.21.1.jar".to_owned(),
                url: "https://piston-data.mojang.com/v1/objects/hash/client.jar".to_owned(),
                sha1: None,
                size: 25_000_000,
            },
            libraries: vec![
                make_lib("com.google.code.gson:gson:2.10.1", None, true),
                make_lib("org.ow2.asm:asm:9.3", None, true),
                make_lib("org.lwjgl:lwjgl:3.3.3", None, true),
                make_lib("org.lwjgl:lwjgl:3.3.3:natives-linux", None, false),
                make_lib("org.slf4j:slf4j-api:2.0.9", None, true),
            ],
            jvm_arguments: vec![
                "-Djava.library.path=${natives_directory}".to_owned(),
                "-cp".to_owned(),
                "${classpath}".to_owned(),
                "-XX:+UseG1GC".to_owned(),
            ],
            game_arguments: vec![
                "--username".to_owned(),
                "${auth_player_name}".to_owned(),
                "--demo".to_owned(),
            ],
            logging: None,
        }
    }

    #[test]
    fn merge_overrides_main_class_deduplicates_asm_and_preserves_order() {
        let parent = sample_vanilla_parent();
        let child_json = r#"{
            "id": "fabric-loader-0.16.9-1.21.1",
            "inheritsFrom": "1.21.1",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "arguments": {
                "jvm": [
                    "-XX:+UseG1GC",
                    "-DFabricMcEmu= net.minecraft.client.main.Main "
                ],
                "game": [
                    "--demo",
                    "--fabric-tag"
                ]
            },
            "libraries": [
                {
                    "name": "net.fabricmc:fabric-loader:0.16.9",
                    "url": "https://maven.fabricmc.net/"
                },
                {
                    "name": "org.ow2.asm:asm:9.7.1",
                    "url": "https://maven.fabricmc.net/"
                },
                {
                    "name": "net.fabricmc:intermediary:1.21.1",
                    "url": "https://maven.fabricmc.net/"
                },
                {
                    "name": "org.ow2.asm:asm:9.7.1",
                    "url": "https://maven.fabricmc.net/"
                }
            ]
        }"#;

        let child =
            parse_loader_profile(child_json, ModLoaderKind::Fabric, Some("0.16.9".to_owned()))
                .expect("parse child");

        let merged = merge_inherits_from(&parent, &child).expect("merge profile");

        assert_eq!(merged.id, "fabric-loader-0.16.9-1.21.1");
        assert_eq!(merged.mc_version, "1.21.1");
        assert_eq!(merged.loader, ModLoaderKind::Fabric);
        assert_eq!(merged.loader_version.as_deref(), Some("0.16.9"));
        assert_eq!(merged.inherits_from.as_deref(), Some("1.21.1"));
        assert_eq!(
            merged.main_class,
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        );

        let coords: Vec<&str> = merged.libraries.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            coords,
            vec![
                "net.fabricmc:fabric-loader:0.16.9",
                "org.ow2.asm:asm:9.7.1",
                "net.fabricmc:intermediary:1.21.1",
                "com.google.code.gson:gson:2.10.1",
                "org.lwjgl:lwjgl:3.3.3",
                "org.lwjgl:lwjgl:3.3.3:natives-linux",
                "org.slf4j:slf4j-api:2.0.9",
            ]
        );
        assert!(
            !coords.contains(&"org.ow2.asm:asm:9.3"),
            "older parent ASM version must be replaced by child ASM version"
        );

        assert_eq!(
            merged.jvm_arguments,
            vec![
                "-Djava.library.path=${natives_directory}",
                "-cp",
                "${classpath}",
                "-XX:+UseG1GC",
                "-DFabricMcEmu= net.minecraft.client.main.Main ",
            ]
        );
        assert_eq!(
            merged.game_arguments,
            vec![
                "--username",
                "${auth_player_name}",
                "--demo",
                "--fabric-tag"
            ]
        );

        // Repeat merge to verify strict determinism across invocations (Lesson E8).
        let merged_again = merge_inherits_from(&parent, &child).expect("second merge");
        assert_eq!(merged, merged_again);
    }

    #[test]
    fn merge_rejects_mismatched_inherits_from_and_preserves_parent_main_when_child_empty() {
        let parent = sample_vanilla_parent();
        let bad_child = LoaderProfile {
            id: "fabric-loader-0.16.9-1.20.4".to_owned(),
            loader: ModLoaderKind::Fabric,
            loader_version: Some("0.16.9".to_owned()),
            inherits_from: Some("1.20.4".to_owned()),
            main_class: String::new(),
            libraries: Vec::new(),
            jvm_arguments: Vec::new(),
            game_arguments: Vec::new(),
            asset_index: None,
            client_jar: None,
            java_version: None,
            logging: None,
        };

        let err = merge_inherits_from(&parent, &bad_child).expect_err("mismatch must fail");
        assert!(matches!(err, MqError::Metadata(_)));

        let fallback_child = LoaderProfile {
            inherits_from: Some("1.21.1".to_owned()),
            ..bad_child
        };
        let merged = merge_inherits_from(&parent, &fallback_child).expect("valid inherits_from");
        assert_eq!(merged.main_class, "net.minecraft.client.main.Main");
    }

    #[test]
    fn deterministic_classpath_filters_natives_places_client_jar_last_and_formats_colon() {
        let parent = sample_vanilla_parent();
        let child = LoaderProfile {
            id: "quilt-loader-0.27.1-1.21.1".to_owned(),
            loader: ModLoaderKind::Quilt,
            loader_version: Some("0.27.1".to_owned()),
            inherits_from: Some("1.21.1".to_owned()),
            main_class: "org.quiltmc.loader.impl.launch.knot.KnotClient".to_owned(),
            libraries: vec![make_lib(
                "org.quiltmc:quilt-loader:0.27.1",
                Some("https://maven.quiltmc.org/repository/release/"),
                true,
            )],
            jvm_arguments: Vec::new(),
            game_arguments: Vec::new(),
            asset_index: None,
            client_jar: None,
            java_version: None,
            logging: None,
        };

        let merged = merge_inherits_from(&parent, &child).expect("merge quilt");
        let libs_dir = Path::new("/opt/mq/libraries");
        let client_jar = Path::new("/opt/mq/versions/1.21.1/1.21.1.jar");

        let cp = build_deterministic_classpath(&merged, libs_dir, client_jar);
        assert_eq!(
            cp,
            vec![
                PathBuf::from(
                    "/opt/mq/libraries/org/quiltmc/quilt-loader/0.27.1/quilt-loader-0.27.1.jar"
                ),
                PathBuf::from("/opt/mq/libraries/com/google/code/gson/gson/2.10.1/gson-2.10.1.jar"),
                PathBuf::from("/opt/mq/libraries/org/ow2/asm/asm/9.3/asm-9.3.jar"),
                PathBuf::from("/opt/mq/libraries/org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar"),
                PathBuf::from("/opt/mq/libraries/org/slf4j/slf4j-api/2.0.9/slf4j-api-2.0.9.jar"),
                PathBuf::from("/opt/mq/versions/1.21.1/1.21.1.jar"),
            ]
        );

        let cp_str = format_classpath_string(&cp);
        let sep = if cfg!(windows) { ";" } else { ":" };
        assert_eq!(
            cp_str.replace('\\', "/"),
            [
                "/opt/mq/libraries/org/quiltmc/quilt-loader/0.27.1/quilt-loader-0.27.1.jar",
                "/opt/mq/libraries/com/google/code/gson/gson/2.10.1/gson-2.10.1.jar",
                "/opt/mq/libraries/org/ow2/asm/asm/9.3/asm-9.3.jar",
                "/opt/mq/libraries/org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar",
                "/opt/mq/libraries/org/slf4j/slf4j-api/2.0.9/slf4j-api-2.0.9.jar",
                "/opt/mq/versions/1.21.1/1.21.1.jar",
            ]
            .join(sep)
        );
    }

    #[test]
    fn merge_cascades_java_version_and_inherits_vanilla_metadata() {
        let parent = sample_vanilla_parent();
        let custom_java = crate::models::JavaVersionSpec {
            component: "java-runtime-epsilon".to_owned(),
            major_version: 25,
        };
        let child = LoaderProfile {
            id: "neoforge-21.1.0".to_owned(),
            loader: ModLoaderKind::NeoForge,
            loader_version: Some("21.1.0".to_owned()),
            inherits_from: Some("1.21.1".to_owned()),
            main_class: "cpw.mods.bootstraplauncher.BootstrapLauncher".to_owned(),
            libraries: Vec::new(),
            jvm_arguments: Vec::new(),
            game_arguments: Vec::new(),
            asset_index: None,
            client_jar: None,
            java_version: Some(custom_java.clone()),
            logging: None,
        };

        let merged = merge_inherits_from(&parent, &child).expect("merge neoforge");
        assert_eq!(merged.java_version, custom_java);
        assert_eq!(merged.asset_index, parent.asset_index);
        assert_eq!(merged.client_jar, parent.client_jar);
        assert_eq!(merged.loader, ModLoaderKind::NeoForge);
    }

    #[test]
    fn format_classpath_strips_windows_verbatim_prefixes_and_uses_semicolon() {
        let entries = [
            PathBuf::from(r"\\?\C:\Users\Player\MQ\libraries\a.jar"),
            PathBuf::from(r"\\?\UNC\server\share\MQ\libraries\b.jar"),
        ];
        let formatted = format_classpath_string_separator(&entries, ';');
        assert_eq!(
            formatted,
            r"C:\Users\Player\MQ\libraries\a.jar;\\server\share\MQ\libraries\b.jar"
        );
    }

    #[test]
    fn bootstrap_launcher_and_forge_bootstrap_omit_vanilla_client_jar_from_classpath() {
        let mut parent = sample_vanilla_parent();
        parent.id = "1.20.1".to_owned();
        parent.mc_version = "1.20.1".to_owned();

        let forge_1_20_1 = LoaderProfile {
            id: "1.20.1-forge-47.4.10".to_owned(),
            loader: ModLoaderKind::Forge,
            loader_version: Some("47.4.10".to_owned()),
            inherits_from: Some("1.20.1".to_owned()),
            main_class: "cpw.mods.bootstraplauncher.BootstrapLauncher".to_owned(),
            libraries: vec![
                make_lib("cpw.mods:bootstraplauncher:1.1.2", None, true),
                make_lib("org.ow2.asm:asm:9.8", None, true),
            ],
            jvm_arguments: vec![
                "-DignoreList=bootstraplauncher,securejarhandler,asm,forge-,${version_name}.jar"
                    .to_owned(),
                "-DlibraryDirectory=${library_directory}".to_owned(),
            ],
            game_arguments: vec![
                "--launchTarget".to_owned(),
                "forgeclient".to_owned(),
                "--fml.forgeVersion".to_owned(),
                "47.4.10".to_owned(),
            ],
            asset_index: None,
            client_jar: None,
            java_version: None,
            logging: None,
        };

        let merged_forge = merge_inherits_from(&parent, &forge_1_20_1).expect("merge forge");
        assert!(uses_fml_managed_client_jar(&merged_forge));

        let libs_dir = Path::new("/opt/mq/libraries");
        let client_jar = Path::new("/opt/mq/instances/pack/client.jar");
        let cp = build_deterministic_classpath(&merged_forge, libs_dir, client_jar);
        assert!(
            !cp.iter().any(|p| p.as_path() == client_jar),
            "Forge 1.20.1 BootstrapLauncher must omit vanilla client.jar from -cp: {cp:?}"
        );
        assert!(
            cp.iter().any(|p| p
                .to_string_lossy()
                .replace('\\', "/")
                .ends_with("asm-9.8.jar")),
            "child ASM 9.8 must override parent ASM 9.3"
        );
        assert!(
            !cp.iter().any(|p| p
                .to_string_lossy()
                .replace('\\', "/")
                .ends_with("asm-9.3.jar")),
            "parent ASM 9.3 must be deduplicated out"
        );

        // Forge 1.20.4+ ForgeBootstrap
        let forge_1_20_4 = LoaderProfile {
            id: "1.20.4-forge-49.1.0".to_owned(),
            main_class: "net.minecraftforge.bootstrap.ForgeBootstrap".to_owned(),
            ..forge_1_20_1.clone()
        };
        let merged_forge_204 =
            merge_inherits_from(&parent, &forge_1_20_4).expect("merge forge 1.20.4");
        assert!(uses_fml_managed_client_jar(&merged_forge_204));
        assert!(
            !build_deterministic_classpath(&merged_forge_204, libs_dir, client_jar)
                .iter()
                .any(|p| p.as_path() == client_jar)
        );

        // NeoForge 1.21.1 BootstrapLauncher
        let neoforge_1_21 = LoaderProfile {
            id: "neoforge-21.1.72".to_owned(),
            loader: ModLoaderKind::NeoForge,
            loader_version: Some("21.1.72".to_owned()),
            inherits_from: Some("1.20.1".to_owned()),
            main_class: "cpw.mods.bootstraplauncher.BootstrapLauncher".to_owned(),
            libraries: vec![make_lib("cpw.mods:bootstraplauncher:2.0.2", None, true)],
            jvm_arguments: vec![
                "-DignoreList=client-extra,${version_name}.jar,neoforge-21.1.72.jar".to_owned(),
            ],
            game_arguments: vec![
                "--launchTarget".to_owned(),
                "forgeclient".to_owned(),
                "--fml.neoForgeVersion".to_owned(),
                "21.1.72".to_owned(),
            ],
            asset_index: None,
            client_jar: None,
            java_version: None,
            logging: None,
        };
        let merged_neo = merge_inherits_from(&parent, &neoforge_1_21).expect("merge neoforge");
        assert!(uses_fml_managed_client_jar(&merged_neo));
        assert!(
            !build_deterministic_classpath(&merged_neo, libs_dir, client_jar)
                .iter()
                .any(|p| p.as_path() == client_jar)
        );

        // Legacy Forge 1.12.2 & Forge 1.16.5 & Vanilla MUST retain client.jar on -cp
        assert!(!uses_fml_managed_client_jar(&parent));
        assert!(build_deterministic_classpath(&parent, libs_dir, client_jar)
            .iter()
            .any(|p| p.as_path() == client_jar));

        let forge_1_12 = LoaderProfile {
            id: "1.12.2-forge-14.23.5.2860".to_owned(),
            loader: ModLoaderKind::Forge,
            loader_version: Some("14.23.5.2860".to_owned()),
            inherits_from: Some("1.20.1".to_owned()),
            main_class: "net.minecraft.launchwrapper.Launch".to_owned(),
            libraries: Vec::new(),
            jvm_arguments: Vec::new(),
            game_arguments: vec![
                "--username".to_owned(),
                "${auth_player_name}".to_owned(),
                "--tweakClass".to_owned(),
                "net.minecraftforge.fml.common.launcher.FMLTweaker".to_owned(),
            ],
            asset_index: None,
            client_jar: None,
            java_version: None,
            logging: None,
        };
        let merged_1_12 = merge_inherits_from(&parent, &forge_1_12).expect("merge forge 1.12.2");
        assert!(!uses_fml_managed_client_jar(&merged_1_12));
        assert!(
            build_deterministic_classpath(&merged_1_12, libs_dir, client_jar)
                .iter()
                .any(|p| p.as_path() == client_jar)
        );
        // Legacy minecraftArguments replacement must NOT duplicate `--username`
        assert_eq!(
            merged_1_12
                .game_arguments
                .iter()
                .filter(|a| a.as_str() == "--username")
                .count(),
            1
        );

        let forge_1_16 = LoaderProfile {
            id: "1.16.5-forge-36.2.42".to_owned(),
            main_class: "cpw.mods.modlauncher.Launcher".to_owned(),
            game_arguments: vec!["--launchTarget".to_owned(), "fmlclient".to_owned()],
            ..forge_1_12
        };
        let merged_1_16 = merge_inherits_from(&parent, &forge_1_16).expect("merge forge 1.16.5");
        assert!(!uses_fml_managed_client_jar(&merged_1_16));
        assert!(
            build_deterministic_classpath(&merged_1_16, libs_dir, client_jar)
                .iter()
                .any(|p| p.as_path() == client_jar)
        );
    }

    #[test]
    fn merge_preserves_separate_artifact_and_native_entries_with_identical_maven_name() {
        let mut parent = sample_vanilla_parent();
        parent.libraries = vec![
            ResolvedLibrary {
                name: "org.lwjgl:lwjgl:3.2.2".to_owned(),
                artifact: Some(
                    maven_coord_to_artifact("org.lwjgl:lwjgl:3.2.2", None, None, 100).unwrap(),
                ),
                native_linux: None,
                extract_exclude: Vec::new(),
                include_in_classpath: true,
            },
            ResolvedLibrary {
                name: "org.lwjgl:lwjgl:3.2.2".to_owned(),
                artifact: None,
                native_linux: Some(
                    maven_coord_to_artifact("org.lwjgl:lwjgl:3.2.2:natives-linux", None, None, 200)
                        .unwrap(),
                ),
                extract_exclude: vec!["META-INF/".to_owned()],
                include_in_classpath: false,
            },
        ];

        let child = LoaderProfile {
            id: "fabric-loader-0.16.9-1.21.1".to_owned(),
            loader: ModLoaderKind::Fabric,
            loader_version: Some("0.16.9".to_owned()),
            inherits_from: Some("1.21.1".to_owned()),
            main_class: "net.fabricmc.loader.impl.launch.knot.KnotClient".to_owned(),
            libraries: vec![make_lib("net.fabricmc:fabric-loader:0.16.9", None, true)],
            jvm_arguments: Vec::new(),
            game_arguments: Vec::new(),
            asset_index: None,
            client_jar: None,
            java_version: None,
            logging: None,
        };

        let merged = merge_inherits_from(&parent, &child).expect("merge");
        assert!(
            merged
                .libraries
                .iter()
                .any(|l| l.name == "org.lwjgl:lwjgl:3.2.2" && l.artifact.is_some()),
            "artifact entry must be preserved"
        );
        assert!(
            merged
                .libraries
                .iter()
                .any(|l| l.name == "org.lwjgl:lwjgl:3.2.2" && l.native_linux.is_some()),
            "separate native-only entry with same Maven coordinate must not be dropped"
        );
    }
}
