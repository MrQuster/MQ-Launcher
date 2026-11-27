//! Fabric, Quilt, Forge, NeoForge and local `inheritsFrom` modloader profile
//! metadata resolution, Maven coordinate artifact mapping, and library duplicate resolution.

use std::collections::{BTreeMap, HashMap, HashSet};

use mq_core::{ModLoaderKind, MqError, MqResult, Sha1Digest};
use serde::{de, Deserialize, Deserializer, Serialize};
use url::Url;

use crate::assets::ClientLoggingConfig;
use crate::models::{ArtifactDownload, JavaVersionSpec, ResolvedLibrary, RuleContext};
use crate::rules::{
    arch_bitness_token, expand_native_classifier_candidates, normalize_arch, normalize_os_name,
    os_version_matches,
};

/// Default base URL for the official Fabric Meta v2 API.
pub const FABRIC_META_BASE_URL: &str = "https://meta.fabricmc.net";

/// Default base URL for the official Quilt Meta v3 API.
pub const QUILT_META_BASE_URL: &str = "https://meta.quiltmc.org";

/// Default Maven repository URL for Fabric artifacts.
pub const FABRIC_MAVEN_URL: &str = "https://maven.fabricmc.net";

/// Default Maven repository URL for Quilt artifacts.
pub const QUILT_MAVEN_URL: &str = "https://maven.quiltmc.org/repository/release";

/// Default Maven repository URL for Minecraft Forge artifacts.
pub const FORGE_MAVEN_URL: &str = "https://maven.minecraftforge.net";

/// Default Maven repository URL for NeoForged artifacts.
pub const NEOFORGE_MAVEN_URL: &str = "https://maven.neoforged.net/releases";

/// Official Forge promotions JSON endpoint (`recommended` / `latest` per MC version).
pub const FORGE_PROMOTIONS_URL: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";

/// Official Forge Maven metadata XML endpoint listing all published Forge versions.
pub const FORGE_MAVEN_METADATA_URL: &str =
    "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml";

/// Official NeoForge Maven metadata XML endpoint listing all published NeoForge versions.
pub const NEOFORGE_MAVEN_METADATA_URL: &str =
    "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml";

/// Fallback Maven repository URL when a profile library omits both `downloads.artifact` and `url`.
pub const DEFAULT_LIBRARIES_BASE_URL: &str = "https://libraries.minecraft.net";

/// Returns the standard default Maven repository base URL for a given modloader kind.
pub fn default_repo_for_loader(loader: ModLoaderKind) -> &'static str {
    match loader {
        ModLoaderKind::Fabric => FABRIC_MAVEN_URL,
        ModLoaderKind::Quilt => QUILT_MAVEN_URL,
        ModLoaderKind::Forge => FORGE_MAVEN_URL,
        ModLoaderKind::NeoForge => NEOFORGE_MAVEN_URL,
        ModLoaderKind::Vanilla => DEFAULT_LIBRARIES_BASE_URL,
    }
}

/// Returns the recommended Maven repository base URL based on Maven coordinate group and loader context.
pub fn default_repo_for_coordinate(coord: &str, loader: ModLoaderKind) -> &'static str {
    let clean = coord.split('@').next().unwrap_or(coord).trim();
    let group = clean.split(':').next().unwrap_or("");
    if group.starts_with("net.fabricmc") {
        FABRIC_MAVEN_URL
    } else if group.starts_with("org.quiltmc") {
        QUILT_MAVEN_URL
    } else if group.starts_with("net.minecraftforge") {
        FORGE_MAVEN_URL
    } else if group.starts_with("net.neoforged") {
        NEOFORGE_MAVEN_URL
    } else {
        default_repo_for_loader(loader)
    }
}

/// Parsed and resolved representation of a modloader `version.json` / `profile/json`
/// ready to be deterministically merged onto a parent [` crate::models::ResolvedVersion `].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderProfile {
    pub id: String,
    pub loader: ModLoaderKind,
    pub loader_version: Option<String>,
    pub inherits_from: Option<String>,
    pub main_class: String,
    pub libraries: Vec<ResolvedLibrary>,
    pub jvm_arguments: Vec<String>,
    pub game_arguments: Vec<String>,
    #[serde(default)]
    pub asset_index: Option<crate::models::AssetIndexPointer>,
    #[serde(default)]
    pub client_jar: Option<ArtifactDownload>,
    #[serde(default)]
    pub java_version: Option<JavaVersionSpec>,
    #[serde(default)]
    pub logging: Option<ClientLoggingConfig>,
}

impl LoaderProfile {
    /// Deterministically merges this child loader profile onto a parent [`crate::models::ResolvedVersion`].
    /// Cascades `asset_index`, `assets`, `downloads` (`client_jar`), `java_version`, `logging`,
    /// and resolves libraries with child precedence.
    pub fn merge_with_parent(
        &self,
        parent: &crate::models::ResolvedVersion,
    ) -> MqResult<crate::models::ResolvedVersion> {
        crate::merge::merge_inherits_from(parent, self)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RawLoaderAssetIndexPointer {
    id: String,
    #[serde(default)]
    sha1: Option<Sha1Digest>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default, rename = "totalSize")]
    total_size: Option<u64>,
    #[serde(default)]
    url: Option<String>,
}

/// Raw library entry inside a Fabric, Quilt, Forge, or NeoForge profile JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawLoaderLibrary {
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_sha1")]
    pub sha1: Option<Sha1Digest>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub downloads: Option<RawLoaderDownloads>,
    #[serde(default)]
    pub rules: Option<Vec<RawLoaderRule>>,
    #[serde(default)]
    pub extract: Option<RawLoaderExtract>,
    #[serde(default)]
    pub natives: Option<BTreeMap<String, String>>,
}

/// Optional `downloads` block inside a Forge, NeoForge, or Mojang-style profile library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawLoaderDownloads {
    #[serde(default)]
    pub artifact: Option<RawLoaderArtifact>,
    #[serde(default)]
    pub classifiers: BTreeMap<String, RawLoaderArtifact>,
}

/// Raw download artifact descriptor inside `downloads.artifact` or `downloads.classifiers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawLoaderArtifact {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_sha1")]
    pub sha1: Option<Sha1Digest>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// Optional native archive extraction rules on a profile library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawLoaderExtract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// OS/arch/feature condition rule on a profile library or argument entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawLoaderRule {
    pub action: String,
    #[serde(default)]
    pub os: Option<RawLoaderOsRule>,
    #[serde(default)]
    pub features: BTreeMap<String, bool>,
}

/// OS matcher inside [`RawLoaderRule`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawLoaderOsRule {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawLoaderProfile {
    id: String,
    #[serde(default, rename = "inheritsFrom")]
    inherits_from: Option<String>,
    #[serde(default, rename = "mainClass")]
    main_class: Option<String>,
    #[serde(default, rename = "javaVersion")]
    java_version: Option<JavaVersionSpec>,
    #[serde(default, rename = "assetIndex")]
    asset_index: Option<RawLoaderAssetIndexPointer>,
    #[serde(default)]
    assets: Option<String>,
    #[serde(default)]
    arguments: Option<RawLoaderArguments>,
    #[serde(default, rename = "minecraftArguments")]
    minecraft_arguments: Option<String>,
    #[serde(default)]
    libraries: Vec<RawLoaderLibrary>,
    #[serde(default)]
    downloads: Option<RawLoaderProfileDownloads>,
    #[serde(default)]
    logging: Option<RawLoaderProfileLogging>,
}

#[derive(Debug, Deserialize)]
struct RawLoaderProfileDownloads {
    #[serde(default)]
    client: Option<RawLoaderArtifact>,
}

#[derive(Debug, Deserialize)]
struct RawLoaderProfileLogging {
    #[serde(default)]
    client: Option<ClientLoggingConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct RawLoaderArguments {
    #[serde(default)]
    jvm: Vec<RawArgumentEntry>,
    #[serde(default)]
    game: Vec<RawArgumentEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawArgumentEntry {
    Plain(String),
    Conditional {
        #[serde(default)]
        rules: Vec<RawLoaderRule>,
        value: RawArgumentValue,
    },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawArgumentValue {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LoaderVersionResponseEntry {
    Nested { loader: NestedLoaderVersion },
    Flat { version: String },
    Plain(String),
}

#[derive(Debug, Deserialize)]
struct NestedLoaderVersion {
    version: String,
}

fn deserialize_optional_sha1<'de, D>(deserializer: D) -> Result<Option<Sha1Digest>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(raw) if raw.trim().is_empty() => Ok(None),
        Some(raw) => Sha1Digest::from_hex(raw.trim())
            .map(Some)
            .map_err(de::Error::custom),
    }
}

fn is_safe_coord_token(token: &str) -> bool {
    !token.is_empty()
        && token != "."
        && token != ".."
        && !token.contains("..")
        && !token.contains('/')
        && !token.contains('\\')
        && !token.contains('\0')
}

fn validate_safe_relative_artifact_path(rel: &str) -> MqResult<String> {
    if rel.is_empty()
        || rel.contains('\0')
        || rel.starts_with('/')
        || rel.starts_with('\\')
        || rel.contains('\\')
    {
        return Err(MqError::ZipSlip {
            entry: rel.to_owned(),
        });
    }

    let bytes = rel.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(MqError::ZipSlip {
            entry: rel.to_owned(),
        });
    }

    let mut clean_segments = Vec::new();
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(MqError::ZipSlip {
                entry: rel.to_owned(),
            });
        }
        clean_segments.push(seg);
    }

    Ok(clean_segments.join("/"))
}

/// Resolves a Maven coordinate (`group:artifact:version[:classifier][@ext]`) and an optional
/// Maven repository base URL into a deterministic [`ArtifactDownload`].
///
/// Strips `@jar` (or uses any explicit `@<ext>` suffix) and rejects malformed or path-traversal
/// coordinates.
pub fn maven_coord_to_artifact(
    name: &str,
    base_url: Option<&str>,
    sha1: Option<Sha1Digest>,
    size: u64,
) -> MqResult<ArtifactDownload> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(MqError::Metadata("empty Maven coordinate".to_owned()));
    }

    let (coord_part, ext) = match trimmed.split_once('@') {
        Some((coord, raw_ext)) => {
            if !is_safe_coord_token(raw_ext) || raw_ext.contains(':') || raw_ext.contains('@') {
                return Err(MqError::Metadata(format!(
                    "invalid extension in Maven coordinate '{name}'"
                )));
            }
            (coord, raw_ext)
        }
        None => (trimmed, "jar"),
    };

    let parts: Vec<&str> = coord_part.split(':').collect();
    if parts.len() != 3 && parts.len() != 4 {
        return Err(MqError::Metadata(format!(
            "invalid Maven coordinate '{name}': expected group:artifact:version[:classifier]"
        )));
    }

    let group = parts[0];
    let artifact = parts[1];
    let version = parts[2];
    let classifier = parts.get(3).copied();

    if group.split('.').any(|seg| !is_safe_coord_token(seg))
        || !is_safe_coord_token(artifact)
        || !is_safe_coord_token(version)
        || classifier.is_some_and(|c| !is_safe_coord_token(c))
    {
        return Err(MqError::Metadata(format!(
            "unsafe or empty segment in Maven coordinate '{name}'"
        )));
    }

    let group_path = group.replace('.', "/");
    let file_name = match classifier {
        Some(clf) => format!("{artifact}-{version}-{clf}.{ext}"),
        None => format!("{artifact}-{version}.{ext}"),
    };
    let path = format!("{group_path}/{artifact}/{version}/{file_name}");

    let base = base_url
        .map(|u| u.trim().trim_end_matches('/'))
        .filter(|u| !u.is_empty())
        .unwrap_or(DEFAULT_LIBRARIES_BASE_URL);
    let url = format!("{base}/{path}");

    Ok(ArtifactDownload {
        path,
        url,
        sha1,
        size,
    })
}

/// Checks whether a standalone Maven classifier (`natives-...`) matches the target OS and CPU architecture.
/// Returns `None` when the coordinate does not carry a `natives-*` classifier.
pub(crate) fn standalone_native_classifier_matches(
    coord: &str,
    target_os: &str,
    os_arch: &str,
) -> Option<bool> {
    let clean = coord.split('@').next().unwrap_or(coord).trim();
    let parts: Vec<&str> = clean.split(':').collect();
    let clf = parts.get(3).copied()?;
    if !clf.starts_with("natives-") {
        return None;
    }

    let norm_os = normalize_os_name(target_os);
    let norm_arch = normalize_arch(os_arch);
    let allowed: &[&str] = match (norm_os, norm_arch) {
        ("windows", "x86_64") => &[
            "natives-windows-x64",
            "natives-windows-64",
            "natives-windows",
        ],
        ("windows", "x86") => &["natives-windows-x86", "natives-windows-32"],
        ("windows", "aarch64") => &["natives-windows-arm64"],
        ("osx", "aarch64") => &[
            "natives-macos-arm64",
            "natives-osx-arm64",
            "natives-macos",
            "natives-osx",
        ],
        ("osx", _) => &["natives-macos", "natives-osx"],
        (_, "x86_64") => &["natives-linux-x64", "natives-linux-64", "natives-linux"],
        (_, "aarch64") => &["natives-linux-arm64", "natives-linux-aarch64"],
        (_, "arm") => &["natives-linux-arm32", "natives-linux-arm"],
        (_, "x86") => &["natives-linux-x86", "natives-linux-32"],
        _ => &["natives-linux"],
    };
    Some(allowed.contains(&clf))
}

impl RawLoaderLibrary {
    /// Returns `(group, artifact, classifier)` key matching [`ResolvedLibrary::module_key`].
    pub fn module_key(&self) -> (String, String, Option<String>) {
        let clean = self.name.split('@').next().unwrap_or(&self.name);
        let parts: Vec<&str> = clean.split(':').collect();
        let group = parts.first().copied().unwrap_or("").to_string();
        let artifact = parts.get(1).copied().unwrap_or("").to_string();
        let classifier = parts.get(3).map(|s| (*s).to_string());
        (group, artifact, classifier)
    }

    /// Converts this raw profile library entry into a [`ResolvedLibrary`] with default repository fallback
    /// for the current host OS and CPU architecture.
    pub fn to_resolved_library(&self) -> MqResult<ResolvedLibrary> {
        self.to_resolved_library_for_context(ModLoaderKind::Vanilla, &RuleContext::current())
    }

    /// Converts this raw profile library entry into a [`ResolvedLibrary`] using the appropriate loader repository
    /// for the current host OS and CPU architecture.
    pub fn to_resolved_library_for_loader(
        &self,
        loader: ModLoaderKind,
    ) -> MqResult<ResolvedLibrary> {
        self.to_resolved_library_for_context(loader, &RuleContext::current())
    }

    /// Converts this raw profile library entry into a [`ResolvedLibrary`] for an explicit [`RuleContext`].
    pub fn to_resolved_library_for_context(
        &self,
        loader: ModLoaderKind,
        ctx: &RuleContext,
    ) -> MqResult<ResolvedLibrary> {
        let target_os = normalize_os_name(&ctx.os_name);
        let standalone_match = if self.natives.is_none() {
            standalone_native_classifier_matches(&self.name, target_os, &ctx.os_arch)
        } else {
            None
        };
        if standalone_match == Some(false) {
            return Ok(ResolvedLibrary {
                name: self.name.trim().to_owned(),
                artifact: None,
                native_linux: None,
                extract_exclude: Vec::new(),
                include_in_classpath: false,
            });
        }

        let default_repo = default_repo_for_coordinate(&self.name, loader);
        let explicit_outer_repo = self.url.as_deref().map(str::trim).filter(|u| !u.is_empty());
        let effective_repo = match self.url.as_deref().map(str::trim) {
            Some(u) if !u.is_empty() => Some(u),
            Some(_) => None, // preserve empty URL for processor artifacts
            None => Some(default_repo),
        };

        let coord_artifact = maven_coord_to_artifact(
            &self.name,
            effective_repo,
            self.sha1,
            self.size.unwrap_or(0),
        )?;

        let is_native_for_target_os = standalone_match == Some(true);

        let native_template_for_os = self.natives.as_ref().and_then(|m| {
            m.iter()
                .find(|(k, _)| normalize_os_name(k).eq_ignore_ascii_case(target_os))
                .map(|(_, v)| v.as_str())
        });

        let (artifact, native_linux) = match &self.downloads {
            Some(downloads) => {
                let primary = if let Some(raw_art) = &downloads.artifact {
                    Some(resolve_raw_artifact(
                        raw_art,
                        &coord_artifact,
                        explicit_outer_repo,
                        self.sha1,
                        self.size,
                    )?)
                } else if !downloads.classifiers.is_empty() {
                    None
                } else {
                    Some(coord_artifact.clone())
                };

                let classifier_candidates: Vec<String> = match native_template_for_os {
                    Some(template) => expand_native_classifier_candidates(template, &ctx.os_arch),
                    None if self.natives.is_some() => Vec::new(),
                    None => match target_os {
                        "windows" => expand_native_classifier_candidates(
                            "natives-windows-${arch}",
                            &ctx.os_arch,
                        )
                        .into_iter()
                        .chain(
                            (normalize_arch(&ctx.os_arch) == "x86_64")
                                .then(|| "natives-windows".to_string()),
                        )
                        .collect(),
                        "osx" => expand_native_classifier_candidates(
                            "natives-macos-${arch}",
                            &ctx.os_arch,
                        )
                        .into_iter()
                        .chain(["natives-macos".to_string(), "natives-osx".to_string()])
                        .collect(),
                        _ => expand_native_classifier_candidates(
                            "natives-linux-${arch}",
                            &ctx.os_arch,
                        )
                        .into_iter()
                        .chain(
                            (normalize_arch(&ctx.os_arch) == "x86_64")
                                .then(|| "natives-linux".to_string()),
                        )
                        .collect(),
                    },
                };

                let matched_classifier = classifier_candidates.iter().find_map(|cand| {
                    downloads
                        .classifiers
                        .get(cand)
                        .map(|raw| (cand.as_str(), raw))
                });

                let native = if let Some((matched_clf, raw_nat)) = matched_classifier {
                    let fallback_native = maven_coord_to_artifact(
                        &format!(
                            "{}:{matched_clf}",
                            self.name.split('@').next().unwrap_or(&self.name)
                        ),
                        effective_repo,
                        None,
                        0,
                    )
                    .unwrap_or_else(|_| coord_artifact.clone());
                    Some(resolve_raw_artifact(
                        raw_nat,
                        &fallback_native,
                        effective_repo,
                        None,
                        None,
                    )?)
                } else if is_native_for_target_os {
                    primary.clone()
                } else {
                    None
                };

                (primary, native)
            }
            None => {
                if self.natives.is_some() {
                    if let Some(template) = native_template_for_os {
                        let expanded_clf =
                            template.replace("${arch}", arch_bitness_token(&ctx.os_arch));
                        let clean = self.name.split('@').next().unwrap_or(&self.name);
                        let native_coord = format!("{clean}:{expanded_clf}");
                        let native_artifact = maven_coord_to_artifact(
                            &native_coord,
                            effective_repo,
                            self.sha1,
                            self.size.unwrap_or(0),
                        )?;
                        (None, Some(native_artifact))
                    } else {
                        (None, None)
                    }
                } else if is_native_for_target_os {
                    (Some(coord_artifact.clone()), Some(coord_artifact))
                } else {
                    (Some(coord_artifact), None)
                }
            }
        };

        let extract_exclude = self
            .extract
            .as_ref()
            .map(|e| e.exclude.clone())
            .unwrap_or_default();

        let include_in_classpath = artifact.is_some() && self.extract.is_none();

        Ok(ResolvedLibrary {
            name: self.name.trim().to_owned(),
            artifact,
            native_linux,
            extract_exclude,
            include_in_classpath,
        })
    }
}

fn resolve_raw_artifact(
    raw_art: &RawLoaderArtifact,
    fallback: &ArtifactDownload,
    repo_base_url: Option<&str>,
    outer_sha1: Option<Sha1Digest>,
    outer_size: Option<u64>,
) -> MqResult<ArtifactDownload> {
    let path = match raw_art.path.as_deref().map(str::trim) {
        Some(p) if !p.is_empty() => validate_safe_relative_artifact_path(p)?,
        _ => fallback.path.clone(),
    };

    let url = match raw_art.url.as_deref().map(str::trim) {
        Some(u) if !u.is_empty() => u.to_owned(),
        Some(_) => {
            if let Some(base) = repo_base_url
                .map(|b| b.trim().trim_end_matches('/'))
                .filter(|b| !b.is_empty())
            {
                format!("{base}/{path}")
            } else {
                // Preserve empty URL for local-only Forge/NeoForge processor artifacts.
                String::new()
            }
        }
        None => fallback.url.clone(),
    };

    let sha1 = raw_art.sha1.or(outer_sha1).or(fallback.sha1);
    let size = raw_art.size.or(outer_size).unwrap_or(fallback.size);

    Ok(ArtifactDownload {
        path,
        url,
        sha1,
        size,
    })
}

fn rules_allow(rules: &[RawLoaderRule], ctx: &RuleContext) -> bool {
    if rules.is_empty() {
        return true;
    }

    let ctx_os = normalize_os_name(&ctx.os_name);
    let ctx_arch = normalize_arch(&ctx.os_arch);

    let mut allowed = false;
    for rule in rules {
        let os_matches = match &rule.os {
            None => true,
            Some(os) => {
                let name_ok = os
                    .name
                    .as_deref()
                    .is_none_or(|n| normalize_os_name(n).eq_ignore_ascii_case(ctx_os));
                let arch_ok = os
                    .arch
                    .as_deref()
                    .is_none_or(|a| normalize_arch(a).eq_ignore_ascii_case(ctx_arch));
                let ver_ok = match os.version.as_deref() {
                    None => true,
                    Some(pat) => ctx
                        .os_version
                        .as_deref()
                        .is_some_and(|actual| os_version_matches(pat, actual)),
                };
                name_ok && arch_ok && ver_ok
            }
        };

        let features_match = rule
            .features
            .iter()
            .all(|(key, expected)| ctx.features.get(key).copied().unwrap_or(false) == *expected);

        if os_matches && features_match {
            allowed = rule.action.trim().eq_ignore_ascii_case("allow");
        }
    }

    allowed
}

fn flatten_arguments(entries: &[RawArgumentEntry], ctx: &RuleContext) -> Vec<String> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            RawArgumentEntry::Plain(arg) => out.push(arg.clone()),
            RawArgumentEntry::Conditional { rules, value } => {
                if rules_allow(rules, ctx) {
                    match value {
                        RawArgumentValue::Single(arg) => out.push(arg.clone()),
                        RawArgumentValue::Multiple(args) => out.extend(args.iter().cloned()),
                    }
                }
            }
        }
    }
    out
}

/// Parses a Fabric, Quilt, Forge, or NeoForge profile JSON string into a [`LoaderProfile`]
/// evaluated against the current host [`RuleContext`].
pub fn parse_loader_profile(
    json: &str,
    loader_kind: ModLoaderKind,
    loader_version: Option<String>,
) -> MqResult<LoaderProfile> {
    parse_loader_profile_for_context(json, loader_kind, loader_version, &RuleContext::current())
}

/// Parses a Fabric, Quilt, Forge, or NeoForge profile JSON string into a [`LoaderProfile`]
/// evaluated against an explicit [`RuleContext`].
pub fn parse_loader_profile_for_context(
    json: &str,
    loader_kind: ModLoaderKind,
    loader_version: Option<String>,
    rule_ctx: &RuleContext,
) -> MqResult<LoaderProfile> {
    let raw: RawLoaderProfile = serde_json::from_str(json)?;

    let id = raw.id.trim().to_owned();
    if id.is_empty() {
        return Err(MqError::Metadata(
            "loader profile JSON has an empty 'id' field".to_owned(),
        ));
    }

    let inherits_from = raw
        .inherits_from
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    let main_class = raw
        .main_class
        .map(|s| s.trim().to_owned())
        .unwrap_or_default();

    let (jvm_arguments, game_arguments) = match &raw.arguments {
        Some(args) => (
            flatten_arguments(&args.jvm, rule_ctx),
            flatten_arguments(&args.game, rule_ctx),
        ),
        None => {
            let legacy_game = raw
                .minecraft_arguments
                .as_deref()
                .map(|s| s.split_whitespace().map(str::to_owned).collect())
                .unwrap_or_default();
            (Vec::new(), legacy_game)
        }
    };

    let mut libraries = Vec::with_capacity(raw.libraries.len());
    for raw_lib in &raw.libraries {
        if let Some(rules) = &raw_lib.rules {
            if !rules_allow(rules, rule_ctx) {
                continue;
            }
        }
        let resolved = raw_lib.to_resolved_library_for_context(loader_kind, rule_ctx)?;
        if resolved.artifact.is_some() || resolved.native_linux.is_some() {
            libraries.push(resolved);
        }
    }

    let normalized_loader_version = loader_version
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty());

    let asset_index = raw
        .asset_index
        .map(|ai| crate::models::AssetIndexPointer {
            id: ai.id,
            sha1: ai.sha1,
            size: ai.size.unwrap_or(0),
            total_size: ai.total_size.unwrap_or(0),
            url: ai.url.unwrap_or_default(),
        })
        .or_else(|| {
            raw.assets
                .filter(|a| !a.trim().is_empty())
                .map(|a| crate::models::AssetIndexPointer {
                    id: a,
                    sha1: None,
                    size: 0,
                    total_size: 0,
                    url: String::new(),
                })
        });

    let client_jar = if let Some(art) = raw.downloads.and_then(|d| d.client) {
        let path = art.path.unwrap_or_else(|| "client.jar".to_string());
        if path.contains("..") || path.starts_with('/') || path.starts_with('\\') {
            return Err(MqError::Metadata(format!(
                "invalid client jar path '{path}': path traversal not allowed"
            )));
        }
        let url = art.url.unwrap_or_default();
        Some(ArtifactDownload {
            path,
            url,
            sha1: art.sha1,
            size: art.size.unwrap_or(0),
        })
    } else {
        None
    };

    let logging = raw.logging.and_then(|l| l.client);

    Ok(LoaderProfile {
        id,
        loader: loader_kind,
        loader_version: normalized_loader_version,
        inherits_from,
        main_class,
        libraries,
        jvm_arguments,
        game_arguments,
        asset_index,
        client_jar,
        java_version: raw.java_version,
        logging,
    })
}

/// Compares two Maven version strings according to Maven version sorting conventions.
///
/// Handles numeric dot-separated tokens (`9.7.1` > `9.3.0`, `10.0` > `2.0`),
/// qualifier ordering (`snapshot` < `alpha` < `beta` < `milestone`/`m` < `rc` < `ga`/`final`/`release`/empty < `sp`),
/// and deterministic tie-breaking.
pub fn compare_maven_versions(v1: &str, v2: &str) -> std::cmp::Ordering {
    if v1 == v2 {
        return std::cmp::Ordering::Equal;
    }

    let tokens1 = tokenize_maven_version(v1);
    let tokens2 = tokenize_maven_version(v2);

    let max_len = tokens1.len().max(tokens2.len());
    for i in 0..max_len {
        let t1 = tokens1.get(i);
        let t2 = tokens2.get(i);
        match (t1, t2) {
            (Some(a), Some(b)) => {
                let ord = compare_maven_token(a, b);
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            (Some(a), None) => {
                let ord = compare_maven_token(a, &MavenToken::Empty);
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            (None, Some(b)) => {
                let ord = compare_maven_token(&MavenToken::Empty, b);
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            (None, None) => break,
        }
    }

    v1.cmp(v2)
}

#[derive(Debug, PartialEq, Eq)]
enum MavenToken {
    Numeric(u64),
    Qualifier(String),
    Empty,
}

fn tokenize_maven_version(v: &str) -> Vec<MavenToken> {
    let mut tokens = Vec::new();
    let parts = v.split(['.', '-', '_', '+']);
    for part in parts {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(num) = trimmed.parse::<u64>() {
            tokens.push(MavenToken::Numeric(num));
        } else {
            let mut curr = String::new();
            let mut is_num = None;
            for ch in trimmed.chars() {
                let ch_num = ch.is_ascii_digit();
                if let Some(prev_num) = is_num {
                    if prev_num != ch_num {
                        if prev_num {
                            if let Ok(n) = curr.parse::<u64>() {
                                tokens.push(MavenToken::Numeric(n));
                            }
                        } else {
                            tokens.push(MavenToken::Qualifier(curr.to_ascii_lowercase()));
                        }
                        curr.clear();
                    }
                }
                is_num = Some(ch_num);
                curr.push(ch);
            }
            if !curr.is_empty() {
                if is_num == Some(true) {
                    if let Ok(n) = curr.parse::<u64>() {
                        tokens.push(MavenToken::Numeric(n));
                    }
                } else {
                    tokens.push(MavenToken::Qualifier(curr.to_ascii_lowercase()));
                }
            }
        }
    }
    tokens
}

fn qualifier_rank(q: &str) -> i32 {
    match q {
        "snapshot" => 1,
        "alpha" | "a" => 2,
        "beta" | "b" => 3,
        "milestone" | "m" => 4,
        "rc" | "cr" => 5,
        "" | "ga" | "final" | "release" => 6,
        "sp" => 7,
        _ => 5,
    }
}

fn compare_maven_token(t1: &MavenToken, t2: &MavenToken) -> std::cmp::Ordering {
    match (t1, t2) {
        (MavenToken::Numeric(a), MavenToken::Numeric(b)) => a.cmp(b),
        (MavenToken::Numeric(_), MavenToken::Qualifier(_)) => std::cmp::Ordering::Greater,
        (MavenToken::Qualifier(_), MavenToken::Numeric(_)) => std::cmp::Ordering::Less,
        (MavenToken::Qualifier(a), MavenToken::Qualifier(b)) => {
            let r1 = qualifier_rank(a);
            let r2 = qualifier_rank(b);
            if r1 != r2 {
                r1.cmp(&r2)
            } else {
                a.cmp(b)
            }
        }
        (MavenToken::Numeric(a), MavenToken::Empty) => {
            if *a == 0 {
                std::cmp::Ordering::Equal
            } else {
                std::cmp::Ordering::Greater
            }
        }
        (MavenToken::Empty, MavenToken::Numeric(b)) => {
            if *b == 0 {
                std::cmp::Ordering::Equal
            } else {
                std::cmp::Ordering::Less
            }
        }
        (MavenToken::Qualifier(a), MavenToken::Empty) => qualifier_rank(a).cmp(&6),
        (MavenToken::Empty, MavenToken::Qualifier(b)) => 6.cmp(&qualifier_rank(b)),
        (MavenToken::Empty, MavenToken::Empty) => std::cmp::Ordering::Equal,
    }
}

/// Extracts the version portion from a Maven coordinate `group:artifact:version[:classifier]`.
pub fn extract_version_from_maven_coord(name: &str) -> Option<&str> {
    let clean = name.split('@').next().unwrap_or(name);
    let parts: Vec<&str> = clean.split(':').collect();
    parts.get(2).copied()
}

/// Strategy for resolving duplicate libraries that share `(group, artifact, classifier)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryDuplicateStrategy {
    /// Keep the first occurrence in order (Mojang profile order).
    FirstWins,
    /// Keep the child/last occurrence in order (inheritsFrom overriding parent).
    ChildOverridesParent,
    /// Keep the library with the newer/higher Maven version.
    PreferNewerVersion,
}

/// Deduplicates libraries sharing identical module key `(group, artifact, classifier)`
/// using the specified resolution strategy.
pub fn deduplicate_libraries(
    libraries: &[ResolvedLibrary],
    strategy: LibraryDuplicateStrategy,
) -> Vec<ResolvedLibrary> {
    if libraries.is_empty() {
        return Vec::new();
    }

    match strategy {
        LibraryDuplicateStrategy::FirstWins => {
            let mut result = Vec::with_capacity(libraries.len());
            let mut seen_modules = HashSet::new();
            let mut seen_names = HashSet::new();
            for lib in libraries {
                if !seen_names.insert(lib.name.clone()) {
                    continue;
                }
                let key = lib.module_key();
                if !key.0.is_empty() && !key.1.is_empty() && !seen_modules.insert(key) {
                    continue;
                }
                result.push(lib.clone());
            }
            result
        }
        LibraryDuplicateStrategy::ChildOverridesParent => {
            let mut result: Vec<ResolvedLibrary> = Vec::with_capacity(libraries.len());
            let mut seen_modules = HashSet::new();
            let mut seen_names = HashSet::new();
            for lib in libraries.iter().rev() {
                if !seen_names.insert(lib.name.clone()) {
                    continue;
                }
                let key = lib.module_key();
                if !key.0.is_empty() && !key.1.is_empty() && !seen_modules.insert(key) {
                    continue;
                }
                result.push(lib.clone());
            }
            result.reverse();
            result
        }
        LibraryDuplicateStrategy::PreferNewerVersion => {
            let mut by_module: BTreeMap<(String, String, Option<String>), ResolvedLibrary> =
                BTreeMap::new();
            let mut non_module = Vec::new();

            for lib in libraries {
                let key = lib.module_key();
                if key.0.is_empty() || key.1.is_empty() {
                    non_module.push(lib.clone());
                    continue;
                }

                if let Some(existing) = by_module.get_mut(&key) {
                    let v_existing = extract_version_from_maven_coord(&existing.name).unwrap_or("");
                    let v_new = extract_version_from_maven_coord(&lib.name).unwrap_or("");
                    if compare_maven_versions(v_new, v_existing) == std::cmp::Ordering::Greater {
                        *existing = lib.clone();
                    }
                } else {
                    by_module.insert(key, lib.clone());
                }
            }

            let mut result = Vec::with_capacity(by_module.len() + non_module.len());
            for lib in libraries {
                let key = lib.module_key();
                if key.0.is_empty() || key.1.is_empty() {
                    continue;
                }
                if let Some(winner) = by_module.remove(&key) {
                    result.push(winner);
                }
            }
            result.extend(non_module);
            result
        }
    }
}

fn build_meta_endpoint_url(base_url: &str, segments: &[&str]) -> MqResult<Url> {
    for seg in segments {
        if seg.trim().is_empty() {
            return Err(MqError::Metadata(
                "loader metadata URL path segment must not be empty".to_owned(),
            ));
        }
    }

    let clean_base = base_url.trim().trim_end_matches('/');
    let mut url = Url::parse(clean_base).map_err(|err| {
        MqError::Metadata(format!(
            "invalid loader metadata base URL '{base_url}': {err}"
        ))
    })?;

    {
        let mut path_segs = url.path_segments_mut().map_err(|()| {
            MqError::Metadata(format!(
                "loader metadata base URL cannot be a base: '{base_url}'"
            ))
        })?;
        path_segs.pop_if_empty();
        path_segs.extend(segments);
    }

    Ok(url)
}

async fn get_text_checked(client: &reqwest::Client, url: Url) -> MqResult<String> {
    let url_str = url.to_string();
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|err| MqError::Network(err.to_string()))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|err| MqError::Network(err.to_string()))?;

    if !status.is_success() {
        let truncated_body: String = body.chars().take(512).collect();
        return Err(MqError::HttpStatus {
            status: status.as_u16(),
            url: url_str,
            body: truncated_body,
        });
    }

    Ok(body)
}

fn parse_loader_versions_response(body: &str) -> MqResult<Vec<String>> {
    let entries: Vec<LoaderVersionResponseEntry> = serde_json::from_str(body)?;
    let mut versions = Vec::with_capacity(entries.len());
    let mut seen = HashSet::with_capacity(entries.len());

    for entry in entries {
        let raw_ver = match entry {
            LoaderVersionResponseEntry::Nested { loader } => loader.version,
            LoaderVersionResponseEntry::Flat { version } => version,
            LoaderVersionResponseEntry::Plain(version) => version,
        };
        let ver = raw_ver.trim().to_owned();
        if !ver.is_empty() && seen.insert(ver.clone()) {
            versions.push(ver);
        }
    }

    Ok(versions)
}

/// Fetches ordered Fabric loader versions compatible with `mc_version` from
/// `{base_url}/v2/versions/loader/{mc_version}`.
pub async fn fetch_fabric_loader_versions(
    client: &reqwest::Client,
    base_url: Option<&str>,
    mc_version: &str,
) -> MqResult<Vec<String>> {
    let base = base_url.unwrap_or(FABRIC_META_BASE_URL);
    let url = build_meta_endpoint_url(base, &["v2", "versions", "loader", mc_version.trim()])?;
    let body = get_text_checked(client, url).await?;
    parse_loader_versions_response(&body)
}

/// Fetches ordered Quilt loader versions compatible with `mc_version` from
/// `{base_url}/v3/versions/loader/{mc_version}`.
pub async fn fetch_quilt_loader_versions(
    client: &reqwest::Client,
    base_url: Option<&str>,
    mc_version: &str,
) -> MqResult<Vec<String>> {
    let base = base_url.unwrap_or(QUILT_META_BASE_URL);
    let url = build_meta_endpoint_url(base, &["v3", "versions", "loader", mc_version.trim()])?;
    let body = get_text_checked(client, url).await?;
    parse_loader_versions_response(&body)
}

/// Fetches and parses a Fabric loader profile from
/// `{base_url}/v2/versions/loader/{mc_version}/{loader_version}/profile/json`.
pub async fn fetch_fabric_profile(
    client: &reqwest::Client,
    base_url: Option<&str>,
    mc_version: &str,
    loader_version: &str,
) -> MqResult<LoaderProfile> {
    let base = base_url.unwrap_or(FABRIC_META_BASE_URL);
    let mc = mc_version.trim();
    let loader = loader_version.trim();
    let url = build_meta_endpoint_url(
        base,
        &["v2", "versions", "loader", mc, loader, "profile", "json"],
    )?;
    let body = get_text_checked(client, url).await?;
    parse_loader_profile(&body, ModLoaderKind::Fabric, Some(loader.to_owned()))
}

/// Fetches and parses a Quilt loader profile from
/// `{base_url}/v3/versions/loader/{mc_version}/{loader_version}/profile/json`.
pub async fn fetch_quilt_profile(
    client: &reqwest::Client,
    base_url: Option<&str>,
    mc_version: &str,
    loader_version: &str,
) -> MqResult<LoaderProfile> {
    let base = base_url.unwrap_or(QUILT_META_BASE_URL);
    let mc = mc_version.trim();
    let loader = loader_version.trim();
    let url = build_meta_endpoint_url(
        base,
        &["v3", "versions", "loader", mc, loader, "profile", "json"],
    )?;
    let body = get_text_checked(client, url).await?;
    parse_loader_profile(&body, ModLoaderKind::Quilt, Some(loader.to_owned()))
}

/// Parsed `install_profile.json` embedded inside a modern Forge (1.13+) installer JAR.
#[derive(Debug, Clone, Deserialize)]
pub struct ForgeInstallProfile {
    #[serde(default)]
    pub spec: Option<u32>,
    #[serde(default)]
    pub minecraft: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub libraries: Vec<RawLoaderLibrary>,
    #[serde(default)]
    pub data: HashMap<String, ForgeDataEntry>,
    #[serde(default)]
    pub processors: Vec<ForgeProcessor>,
}

/// Side-specific data value in Forge `install_profile.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct ForgeDataEntry {
    #[serde(default)]
    pub client: String,
    #[serde(default)]
    pub server: String,
}

/// A single post-download processor step in Forge `install_profile.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct ForgeProcessor {
    #[serde(default)]
    pub sides: Vec<String>,
    pub jar: String,
    #[serde(default)]
    pub classpath: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub outputs: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ForgePromotionsResponse {
    #[serde(default)]
    promos: HashMap<String, String>,
}

/// Parses a Forge `install_profile.json` string into [`ForgeInstallProfile`].
///
/// Rejects legacy (pre-1.13) `install_profile.json` files that lack `processors`
/// and use `install` / `versionInfo` instead.
pub fn parse_forge_install_profile(json_str: &str) -> MqResult<ForgeInstallProfile> {
    let value: serde_json::Value = serde_json::from_str(json_str)?;
    if value.get("versionInfo").is_some() && value.get("processors").is_none() {
        return Err(MqError::Metadata(
            "legacy Forge install_profile.json (pre-1.13 versionInfo format) is not supported"
                .to_owned(),
        ));
    }
    let profile: ForgeInstallProfile = serde_json::from_value(value)?;
    Ok(profile)
}

/// Normalizes a Forge version string into `(short_forge_version, full_maven_version)`,
/// e.g. `("47.3.0", "1.20.1-47.3.0")` for `mc_version = "1.20.1"`.
pub fn normalize_forge_version(
    mc_version: &str,
    forge_version: &str,
) -> MqResult<(String, String)> {
    let mc = mc_version.trim();
    let raw = forge_version.trim();
    if mc.is_empty() || raw.is_empty() {
        return Err(MqError::Metadata(
            "Minecraft version and Forge version must not be empty".to_owned(),
        ));
    }

    let prefix = format!("{mc}-");
    let short = raw.strip_prefix(&prefix).unwrap_or(raw).to_owned();
    let full = format!("{mc}-{short}");
    if !is_safe_coord_token(&short) || !is_safe_coord_token(&full) {
        return Err(MqError::Metadata(format!(
            "unsafe Forge version string '{forge_version}'"
        )));
    }
    Ok((short, full))
}

/// Builds the Forge installer JAR download URL for `mc_version` and `forge_version`.
pub fn forge_installer_url(
    base_url: Option<&str>,
    mc_version: &str,
    forge_version: &str,
) -> MqResult<String> {
    let base = base_url
        .map(|u| u.trim().trim_end_matches('/'))
        .filter(|u| !u.is_empty())
        .unwrap_or(FORGE_MAVEN_URL);
    let (_, full) = normalize_forge_version(mc_version, forge_version)?;
    Ok(format!(
        "{base}/net/minecraftforge/forge/{full}/forge-{full}-installer.jar"
    ))
}

/// Extracts Forge versions for `mc_version` from Maven `maven-metadata.xml` content,
/// ordered newest-first.
pub fn parse_forge_maven_metadata_versions(xml: &str, mc_version: &str) -> Vec<String> {
    let prefix = format!("{}-", mc_version.trim());
    let mut versions = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<version>") {
        let after_open = &rest[start + "<version>".len()..];
        let Some(end) = after_open.find("</version>") else {
            break;
        };
        let ver = after_open[..end].trim();
        if let Some(short) = ver.strip_prefix(&prefix) {
            if !short.is_empty() {
                versions.push(short.to_owned());
            }
        }
        rest = &after_open[end + "</version>".len()..];
    }
    versions.reverse();
    versions
}

/// Fetches available Forge versions for `mc_version` (newest first) from Forge Maven metadata.
pub async fn fetch_forge_loader_versions(
    client: &reqwest::Client,
    base_url: Option<&str>,
    mc_version: &str,
) -> MqResult<Vec<String>> {
    let url_str = match base_url {
        Some(base) => format!(
            "{}/net/minecraftforge/forge/maven-metadata.xml",
            base.trim().trim_end_matches('/')
        ),
        None => FORGE_MAVEN_METADATA_URL.to_owned(),
    };
    let url = Url::parse(&url_str).map_err(|err| {
        MqError::Metadata(format!("invalid Forge metadata URL '{url_str}': {err}"))
    })?;
    let body = get_text_checked(client, url).await?;
    Ok(parse_forge_maven_metadata_versions(&body, mc_version))
}

/// Fetches the recommended (or latest) Forge version for `mc_version` via Forge promotions JSON,
/// falling back to `maven-metadata.xml` if promotions does not list `mc_version`.
pub async fn fetch_forge_recommended_version(
    client: &reqwest::Client,
    base_url: Option<&str>,
    mc_version: &str,
) -> MqResult<String> {
    let mc = mc_version.trim();
    let promo_url_str = match base_url {
        Some(base) => format!(
            "{}/net/minecraftforge/forge/promotions_slim.json",
            base.trim().trim_end_matches('/')
        ),
        None => FORGE_PROMOTIONS_URL.to_owned(),
    };
    if let Ok(url) = Url::parse(&promo_url_str) {
        if let Ok(body) = get_text_checked(client, url).await {
            if let Ok(parsed) = serde_json::from_str::<ForgePromotionsResponse>(&body) {
                if let Some(ver) = parsed.promos.get(&format!("{mc}-recommended")) {
                    if !ver.trim().is_empty() {
                        return Ok(ver.trim().to_owned());
                    }
                }
                if let Some(ver) = parsed.promos.get(&format!("{mc}-latest")) {
                    if !ver.trim().is_empty() {
                        return Ok(ver.trim().to_owned());
                    }
                }
            }
        }
    }

    let versions = fetch_forge_loader_versions(client, base_url, mc).await?;
    versions
        .into_iter()
        .next()
        .ok_or_else(|| MqError::Metadata(format!("no Forge loader version found for {mc}")))
}

/// Builds the NeoForge installer JAR download URL for `neoforge_version`.
pub fn neoforge_installer_url(base_url: Option<&str>, neoforge_version: &str) -> MqResult<String> {
    let ver = neoforge_version.trim();
    if ver.is_empty() {
        return Err(MqError::Metadata(
            "NeoForge version must not be empty".to_string(),
        ));
    }
    if ver.contains("..") || ver.contains('/') || ver.contains('\\') {
        return Err(MqError::Metadata(format!(
            "invalid NeoForge version '{ver}': path traversal characters not allowed"
        )));
    }
    let base = base_url
        .map(|u| u.trim().trim_end_matches('/'))
        .filter(|u| !u.is_empty())
        .unwrap_or(NEOFORGE_MAVEN_URL);

    if ver.starts_with("1.20.1-") {
        Ok(format!(
            "{base}/net/neoforged/forge/{ver}/forge-{ver}-installer.jar"
        ))
    } else {
        Ok(format!(
            "{base}/net/neoforged/neoforge/{ver}/neoforge-{ver}-installer.jar"
        ))
    }
}

/// Extracts NeoForge versions for `mc_version` from Maven `maven-metadata.xml` content,
/// ordered newest-first (with stable releases prioritized over `-beta`/`-alpha`).
pub fn parse_neoforge_maven_metadata_versions(xml: &str, mc_version: &str) -> Vec<String> {
    let mc = mc_version.trim();
    let is_1_20_1 = mc == "1.20.1";
    let prefix = if is_1_20_1 {
        "1.20.1-".to_string()
    } else if let Some(rest) = mc.strip_prefix("1.") {
        if rest.contains('.') {
            format!("{rest}.")
        } else {
            format!("{rest}.0.")
        }
    } else {
        let parts: Vec<&str> = mc.split('.').collect();
        if parts.len() == 2 {
            format!("{mc}.0.")
        } else {
            format!("{mc}.")
        }
    };

    let mut stable = Vec::new();
    let mut pre = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<version>") {
        let after_open = &rest[start + "<version>".len()..];
        let Some(end) = after_open.find("</version>") else {
            break;
        };
        let ver = after_open[..end].trim();
        if ver.starts_with(&prefix) {
            if ver.contains("-beta") || ver.contains("-alpha") {
                pre.push(ver.to_owned());
            } else {
                stable.push(ver.to_owned());
            }
        }
        rest = &after_open[end + "</version>".len()..];
    }
    stable.reverse();
    pre.reverse();
    stable.extend(pre);
    stable
}

/// Fetches available NeoForge versions for `mc_version` (newest first) from NeoForge Maven metadata.
pub async fn fetch_neoforge_loader_versions(
    client: &reqwest::Client,
    base_url: Option<&str>,
    mc_version: &str,
) -> MqResult<Vec<String>> {
    let mc = mc_version.trim();
    if mc == "1.20.1" {
        let url_str = match base_url {
            Some(base) => format!(
                "{}/net/neoforged/forge/maven-metadata.xml",
                base.trim().trim_end_matches('/')
            ),
            None => "https://maven.neoforged.net/releases/net/neoforged/forge/maven-metadata.xml"
                .to_owned(),
        };
        if let Ok(url) = Url::parse(&url_str) {
            if let Ok(body) = get_text_checked(client, url).await {
                let versions = parse_neoforge_maven_metadata_versions(&body, mc);
                if !versions.is_empty() {
                    return Ok(versions);
                }
            }
        }
    }

    let url_str = match base_url {
        Some(base) => format!(
            "{}/net/neoforged/neoforge/maven-metadata.xml",
            base.trim().trim_end_matches('/')
        ),
        None => NEOFORGE_MAVEN_METADATA_URL.to_owned(),
    };
    let url = Url::parse(&url_str).map_err(|err| {
        MqError::Metadata(format!("invalid NeoForge metadata URL '{url_str}': {err}"))
    })?;
    let body = get_text_checked(client, url).await?;
    let mut versions = parse_neoforge_maven_metadata_versions(&body, mc_version);
    if versions.is_empty() && mc == "1.20.1" {
        let url_str = match base_url {
            Some(base) => format!(
                "{}/net/neoforged/forge/maven-metadata.xml",
                base.trim().trim_end_matches('/')
            ),
            None => "https://maven.neoforged.net/releases/net/neoforged/forge/maven-metadata.xml"
                .to_owned(),
        };
        if let Ok(url) = Url::parse(&url_str) {
            if let Ok(body) = get_text_checked(client, url).await {
                versions = parse_neoforge_maven_metadata_versions(&body, mc);
            }
        }
    }
    Ok(versions)
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    #[test]
    fn maven_coord_resolves_standard_classifier_and_jar_suffix() {
        let sha1 =
            Sha1Digest::from_hex("f0ed132a49244b042cd0e15702ab9f2ce3cc8436").expect("valid sha1");
        let fabric = maven_coord_to_artifact(
            "net.fabricmc:fabric-loader:0.16.9",
            Some("https://maven.fabricmc.net/"),
            Some(sha1),
            1_234_567,
        )
        .expect("resolve fabric-loader coordinate");

        assert_eq!(
            fabric.path,
            "net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar"
        );
        assert_eq!(
            fabric.url,
            "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar"
        );
        assert_eq!(fabric.sha1, Some(sha1));
        assert_eq!(fabric.size, 1_234_567);

        let with_clf =
            maven_coord_to_artifact("org.lwjgl:lwjgl:3.3.3:natives-linux@jar", None, None, 0)
                .expect("resolve classifier coordinate with @jar");

        assert_eq!(
            with_clf.path,
            "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-linux.jar"
        );
        assert_eq!(
            with_clf.url,
            "https://libraries.minecraft.net/org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-linux.jar"
        );
    }

    #[test]
    fn maven_coord_synthesizes_paths_for_forge_and_neoforge() {
        let forge_lib = RawLoaderLibrary {
            name: "net.minecraftforge:forge:1.20.1-47.2.0:launcher".to_string(),
            url: None,
            sha1: None,
            size: None,
            downloads: None,
            rules: None,
            extract: None,
            natives: None,
        };
        let resolved_forge = forge_lib
            .to_resolved_library_for_loader(ModLoaderKind::Forge)
            .expect("resolve forge lib");
        let art = resolved_forge.artifact.expect("artifact");
        assert_eq!(
            art.path,
            "net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0-launcher.jar"
        );
        assert_eq!(
            art.url,
            "https://maven.minecraftforge.net/net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0-launcher.jar"
        );

        let neo_lib = RawLoaderLibrary {
            name: "net.neoforged:neoforge:20.4.80-beta".to_string(),
            url: None,
            sha1: None,
            size: None,
            downloads: None,
            rules: None,
            extract: None,
            natives: None,
        };
        let resolved_neo = neo_lib
            .to_resolved_library_for_loader(ModLoaderKind::NeoForge)
            .expect("resolve neoforge lib");
        let neo_art = resolved_neo.artifact.expect("neoforge artifact");
        assert_eq!(
            neo_art.path,
            "net/neoforged/neoforge/20.4.80-beta/neoforge-20.4.80-beta.jar"
        );
        assert_eq!(
            neo_art.url,
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/20.4.80-beta/neoforge-20.4.80-beta.jar"
        );
    }

    #[test]
    fn maven_coord_rejects_malformed_and_traversal_inputs() {
        for bad in [
            "",
            "net.fabricmc:fabric-loader",
            "a:b:c:d:e",
            "../evil:artifact:1.0",
            "org.example:../evil:1.0",
            "org.example:artifact:1.0/../../pwn",
            "org..example:artifact:1.0",
            "org.example:artifact:1.0@",
            "org.example:artifact:1.0@../jar",
        ] {
            assert!(
                maven_coord_to_artifact(bad, Some("https://maven.fabricmc.net/"), None, 0).is_err(),
                "expected error for coordinate {bad:?}"
            );
        }
    }

    #[test]
    fn parses_fabric_and_quilt_profiles_with_maven_and_explicit_downloads() {
        let fabric_json = r#"{
            "id": "fabric-loader-0.16.9-1.21.1",
            "inheritsFrom": "1.21.1",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "arguments": {
                "game": [],
                "jvm": [
                    "-DFabricMcEmu= net.minecraft.client.main.Main ",
                    {
                        "rules": [{ "action": "allow", "os": { "name": "osx" } }],
                        "value": ["-XstartOnFirstThread"]
                    }
                ]
            },
            "libraries": [
                {
                    "name": "org.ow2.asm:asm:9.7.1",
                    "url": "https://maven.fabricmc.net/",
                    "sha1": "f0ed132a49244b042cd0e15702ab9f2ce3cc8436",
                    "size": 126093
                },
                {
                    "name": "net.fabricmc:fabric-loader:0.16.9",
                    "url": "https://maven.fabricmc.net/"
                }
            ]
        }"#;

        let fabric = parse_loader_profile(
            fabric_json,
            ModLoaderKind::Fabric,
            Some("0.16.9".to_owned()),
        )
        .expect("parse fabric profile");

        assert_eq!(fabric.id, "fabric-loader-0.16.9-1.21.1");
        assert_eq!(fabric.loader, ModLoaderKind::Fabric);
        assert_eq!(fabric.loader_version.as_deref(), Some("0.16.9"));
        assert_eq!(fabric.inherits_from.as_deref(), Some("1.21.1"));
        assert_eq!(
            fabric.main_class,
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        );
        assert_eq!(
            fabric.jvm_arguments,
            vec!["-DFabricMcEmu= net.minecraft.client.main.Main ".to_owned()]
        );
        assert!(fabric.game_arguments.is_empty());
        assert_eq!(fabric.libraries.len(), 2);
        assert_eq!(
            fabric.libraries[0].artifact.as_ref().unwrap().path,
            "org/ow2/asm/asm/9.7.1/asm-9.7.1.jar"
        );
        assert_eq!(fabric.libraries[0].artifact.as_ref().unwrap().size, 126093);
        assert!(fabric.libraries[0]
            .artifact
            .as_ref()
            .unwrap()
            .sha1
            .is_some());
        assert_eq!(
            fabric.libraries[1].artifact.as_ref().unwrap().url,
            "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar"
        );

        let quilt_json = r#"{
            "id": "quilt-loader-0.27.1-1.21.1",
            "inheritsFrom": "1.21.1",
            "mainClass": "org.quiltmc.loader.impl.launch.knot.KnotClient",
            "arguments": {
                "game": ["--quilt-profile"]
            },
            "libraries": [
                {
                    "name": "org.quiltmc:quilt-loader:0.27.1",
                    "url": "https://maven.quiltmc.org/repository/release/"
                },
                {
                    "name": "cpw.mods:bootstraplauncher:2.0.2",
                    "downloads": {
                        "artifact": {
                            "path": "cpw/mods/bootstraplauncher/2.0.2/bootstraplauncher-2.0.2.jar",
                            "url": "https://maven.neoforged.net/releases/cpw/mods/bootstraplauncher/2.0.2/bootstraplauncher-2.0.2.jar",
                            "sha1": "",
                            "size": 45678
                        }
                    }
                }
            ]
        }"#;

        let quilt =
            parse_loader_profile(quilt_json, ModLoaderKind::Quilt, Some("0.27.1".to_owned()))
                .expect("parse quilt profile");

        assert_eq!(quilt.loader, ModLoaderKind::Quilt);
        assert!(quilt.jvm_arguments.is_empty());
        assert_eq!(quilt.game_arguments, vec!["--quilt-profile"]);
        assert_eq!(quilt.libraries.len(), 2);
        assert_eq!(
            quilt.libraries[0].artifact.as_ref().unwrap().url,
            "https://maven.quiltmc.org/repository/release/org/quiltmc/quilt-loader/0.27.1/quilt-loader-0.27.1.jar"
        );
        assert_eq!(quilt.libraries[1].artifact.as_ref().unwrap().sha1, None);
        assert_eq!(quilt.libraries[1].artifact.as_ref().unwrap().size, 45678);
    }

    #[test]
    fn parses_forge_and_neoforge_profiles_with_natives_and_java_version() {
        let forge_json = r#"{
            "id": "1.12.2-forge-14.23.5.2860",
            "inheritsFrom": "1.12.2",
            "mainClass": "net.minecraft.launchwrapper.Launch",
            "javaVersion": {
                "component": "jre-legacy",
                "majorVersion": 8
            },
            "logging": {
                "client": {
                    "argument": "-Dlog4j.configurationFile=${path}",
                    "file": {
                        "id": "client-1.12.xml",
                        "size": 877,
                        "url": "https://piston-data.mojang.com/v1/objects/ef4f57b9222277c603b5443217b1fb797c293eb7/client-1.12.xml"
                    },
                    "type": "log4j2-xml"
                }
            },
            "libraries": [
                {
                    "name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4-nightly-20150209",
                    "natives": {
                        "linux": "natives-linux",
                        "windows": "natives-windows-${arch}"
                    },
                    "extract": {
                        "exclude": ["META-INF/"]
                    }
                }
            ]
        }"#;

        let forge_linux = parse_loader_profile_for_context(
            forge_json,
            ModLoaderKind::Forge,
            None,
            &RuleContext::linux_x86_64(),
        )
        .expect("parse forge profile on linux");

        assert_eq!(forge_linux.loader, ModLoaderKind::Forge);
        assert_eq!(forge_linux.main_class, "net.minecraft.launchwrapper.Launch");
        assert_eq!(forge_linux.java_version.as_ref().unwrap().major_version, 8);
        assert!(forge_linux.logging.is_some());
        assert_eq!(forge_linux.libraries.len(), 1);

        let lwjgl_linux = &forge_linux.libraries[0];
        assert!(lwjgl_linux.native_linux.is_some());
        assert_eq!(
            lwjgl_linux.native_linux.as_ref().unwrap().path,
            "org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209-natives-linux.jar"
        );
        assert_eq!(lwjgl_linux.extract_exclude, vec!["META-INF/"]);
        assert!(!lwjgl_linux.include_in_classpath);

        let forge_win64 = parse_loader_profile_for_context(
            forge_json,
            ModLoaderKind::Forge,
            None,
            &RuleContext::windows_x86_64(),
        )
        .expect("parse forge profile on windows x64");
        assert_eq!(
            forge_win64.libraries[0].native_linux.as_ref().unwrap().path,
            "org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209-natives-windows-64.jar"
        );

        let forge_win32 = parse_loader_profile_for_context(
            forge_json,
            ModLoaderKind::Forge,
            None,
            &RuleContext::windows_x86(),
        )
        .expect("parse forge profile on windows x86");
        assert_eq!(
            forge_win32.libraries[0].native_linux.as_ref().unwrap().path,
            "org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209-natives-windows-32.jar"
        );
    }

    #[test]
    fn compares_maven_versions_correctly() {
        use std::cmp::Ordering;
        assert_eq!(compare_maven_versions("9.7.1", "9.3.0"), Ordering::Greater);
        assert_eq!(compare_maven_versions("10.0", "2.0"), Ordering::Greater);
        assert_eq!(compare_maven_versions("1.21.1", "1.21.1"), Ordering::Equal);
        assert_eq!(
            compare_maven_versions("1.0.0-snapshot", "1.0.0"),
            Ordering::Less
        );
        assert_eq!(compare_maven_versions("1.0.0-rc1", "1.0.0"), Ordering::Less);
        assert_eq!(
            compare_maven_versions("1.0.0-beta", "1.0.0-rc1"),
            Ordering::Less
        );
        assert_eq!(
            compare_maven_versions("20.4.80-beta", "20.4.79"),
            Ordering::Greater
        );
    }

    #[test]
    fn resolves_library_duplicates_with_mojang_and_version_strategies() {
        let make_lib = |name: &str| ResolvedLibrary {
            name: name.to_string(),
            artifact: None,
            native_linux: None,
            extract_exclude: Vec::new(),
            include_in_classpath: true,
        };

        let libs = vec![
            make_lib("org.ow2.asm:asm:9.3"),
            make_lib("com.google.code.gson:gson:2.10.1"),
            make_lib("org.ow2.asm:asm:9.7.1"),
        ];

        // Strategy 1: FirstWins (keeps 9.3)
        let first_wins = deduplicate_libraries(&libs, LibraryDuplicateStrategy::FirstWins);
        assert_eq!(first_wins.len(), 2);
        assert_eq!(first_wins[0].name, "org.ow2.asm:asm:9.3");

        // Strategy 2: ChildOverridesParent (keeps 9.7.1)
        let child_overrides =
            deduplicate_libraries(&libs, LibraryDuplicateStrategy::ChildOverridesParent);
        assert_eq!(child_overrides.len(), 2);
        assert_eq!(child_overrides[1].name, "org.ow2.asm:asm:9.7.1");

        // Strategy 3: PreferNewerVersion (picks 9.7.1 over 9.3)
        let newer = deduplicate_libraries(&libs, LibraryDuplicateStrategy::PreferNewerVersion);
        assert_eq!(newer.len(), 2);
        assert_eq!(newer[0].name, "org.ow2.asm:asm:9.7.1");
    }

    #[tokio::test]
    async fn fetches_fabric_versions_and_fabric_quilt_profiles_over_http() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock listener");
        let addr = listener.local_addr().expect("local addr");

        let server = tokio::spawn(async move {
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().await.expect("accept client");
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).await.expect("read request");
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");

                let (status, body) = if first_line.starts_with("GET /v2/versions/loader/1.21.1 ") {
                    (
                        "200 OK",
                        r#"[{"loader":{"version":"0.16.9"}},{"loader":{"version":"0.16.8"}},{"loader":{"version":"0.16.9"}}]"#,
                    )
                } else if first_line
                    .starts_with("GET /v2/versions/loader/1.21.1/0.16.9/profile/json ")
                {
                    (
                        "200 OK",
                        r#"{"id":"fabric-loader-0.16.9-1.21.1","inheritsFrom":"1.21.1","mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient","libraries":[{"name":"net.fabricmc:fabric-loader:0.16.9","url":"https://maven.fabricmc.net/"}]}"#,
                    )
                } else if first_line
                    .starts_with("GET /v3/versions/loader/1.21.1/0.27.1/profile/json ")
                {
                    (
                        "200 OK",
                        r#"{"id":"quilt-loader-0.27.1-1.21.1","inheritsFrom":"1.21.1","mainClass":"org.quiltmc.loader.impl.launch.knot.KnotClient","libraries":[{"name":"org.quiltmc:quilt-loader:0.27.1","url":"https://maven.quiltmc.org/repository/release/"}]}"#,
                    )
                } else {
                    ("404 Not Found", "missing version")
                };

                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write response");
            }
        });

        let base = format!("http://{addr}");
        let client = reqwest::Client::new();

        let versions = fetch_fabric_loader_versions(&client, Some(&base), "1.21.1")
            .await
            .expect("fetch fabric versions");
        assert_eq!(versions, vec!["0.16.9".to_owned(), "0.16.8".to_owned()]);

        let fabric_profile = fetch_fabric_profile(&client, Some(&base), "1.21.1", "0.16.9")
            .await
            .expect("fetch fabric profile");
        assert_eq!(fabric_profile.loader, ModLoaderKind::Fabric);
        assert_eq!(fabric_profile.loader_version.as_deref(), Some("0.16.9"));
        assert_eq!(fabric_profile.libraries.len(), 1);

        let quilt_profile = fetch_quilt_profile(&client, Some(&base), "1.21.1", "0.27.1")
            .await
            .expect("fetch quilt profile");
        assert_eq!(quilt_profile.loader, ModLoaderKind::Quilt);
        assert_eq!(quilt_profile.loader_version.as_deref(), Some("0.27.1"));

        let missing_err = fetch_fabric_profile(&client, Some(&base), "1.21.1", "0.0.0")
            .await
            .expect_err("expected 404 error");
        assert!(matches!(
            missing_err,
            MqError::HttpStatus { status: 404, .. }
        ));

        server.await.expect("mock server finished");
    }

    #[test]
    fn parses_forge_install_profile_and_normalizes_versions() {
        let json = r#"{
            "spec": 1,
            "minecraft": "1.20.1",
            "version": "1.20.1-forge-47.3.0",
            "libraries": [
                {
                    "name": "net.minecraftforge:installertools:1.4.1",
                    "downloads": {
                        "artifact": {
                            "path": "net/minecraftforge/installertools/1.4.1/installertools-1.4.1.jar",
                            "url": "https://maven.minecraftforge.net/net/minecraftforge/installertools/1.4.1/installertools-1.4.1.jar"
                        }
                    }
                }
            ],
            "data": {
                "BINPATCH": {
                    "client": "/data/client.lzma",
                    "server": "/data/server.lzma"
                }
            },
            "processors": [
                {
                    "sides": ["client"],
                    "jar": "net.minecraftforge:installertools:1.4.1",
                    "classpath": ["net.md-5:SpecialSource:1.11.0"],
                    "args": ["--task", "MCP_DATA", "--input", "{MINECRAFT_JAR}"]
                }
            ]
        }"#;

        let profile = parse_forge_install_profile(json).expect("parse forge install profile");
        assert_eq!(profile.spec, Some(1));
        assert_eq!(profile.minecraft, "1.20.1");
        assert_eq!(profile.libraries.len(), 1);
        assert_eq!(profile.processors.len(), 1);
        assert_eq!(
            profile.data.get("BINPATCH").unwrap().client,
            "/data/client.lzma"
        );

        let (short, full) = normalize_forge_version("1.20.1", "1.20.1-47.3.0").expect("normalize");
        assert_eq!(short, "47.3.0");
        assert_eq!(full, "1.20.1-47.3.0");

        let url = forge_installer_url(None, "1.20.1", "47.3.0").expect("installer url");
        assert_eq!(
            url,
            "https://maven.minecraftforge.net/net/minecraftforge/forge/1.20.1-47.3.0/forge-1.20.1-47.3.0-installer.jar"
        );

        let xml = "<metadata><versioning><versions>\
            <version>1.19.4-45.1.0</version>\
            <version>1.20.1-47.2.0</version>\
            <version>1.20.1-47.3.0</version>\
            </versions></versioning></metadata>";
        assert_eq!(
            parse_forge_maven_metadata_versions(xml, "1.20.1"),
            vec!["47.3.0".to_owned(), "47.2.0".to_owned()]
        );
    }

    #[test]
    fn parses_neoforge_maven_metadata_for_1_21_and_26_x() {
        let xml = r#"
<metadata><versioning><versions>
  <version>21.1.263</version>
  <version>26.3.0.99-beta</version>
  <version>26.3.0.100</version>
  <version>26.3.0.103</version>
</versions></versioning></metadata>"#;
        let v26 = parse_neoforge_maven_metadata_versions(xml, "26.3");
        assert_eq!(v26, vec!["26.3.0.103", "26.3.0.100", "26.3.0.99-beta"]);
        let v21 = parse_neoforge_maven_metadata_versions(xml, "1.21.1");
        assert_eq!(v21, vec!["21.1.263"]);
        let url = neoforge_installer_url(None, "26.3.0.103").expect("neoforge url");
        assert_eq!(
            url,
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/26.3.0.103/neoforge-26.3.0.103-installer.jar"
        );

        let xml2 = r#"
<metadata><versioning><versions>
  <version>26.1.0.100</version>
  <version>26.1.1.20</version>
  <version>1.20.1-47.1.106</version>
</versions></versioning></metadata>"#;
        let v26_1 = parse_neoforge_maven_metadata_versions(xml2, "26.1");
        assert_eq!(v26_1, vec!["26.1.0.100"]);
        let v1201 = parse_neoforge_maven_metadata_versions(xml2, "1.20.1");
        assert_eq!(v1201, vec!["1.20.1-47.1.106"]);
        let url_1201 =
            neoforge_installer_url(None, "1.20.1-47.1.106").expect("neoforge 1.20.1 url");
        assert_eq!(
            url_1201,
            "https://maven.neoforged.net/releases/net/neoforged/forge/1.20.1-47.1.106/forge-1.20.1-47.1.106-installer.jar"
        );
        assert!(neoforge_installer_url(None, "../bad").is_err());
    }

    #[test]
    fn windows_x86_64_filters_standalone_native_classifiers_and_preserves_empty_forge_url() {
        let win64_ctx = RuleContext::windows_x86_64();

        let profile_json = r#"{
            "id": "neoforge-21.1.72",
            "inheritsFrom": "1.21.1",
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "libraries": [
                { "name": "org.lwjgl:lwjgl:3.3.3:natives-windows" },
                { "name": "org.lwjgl:lwjgl:3.3.3:natives-windows-x86" },
                { "name": "org.lwjgl:lwjgl:3.3.3:natives-windows-arm64" },
                { "name": "org.lwjgl:lwjgl:3.3.3:natives-linux" },
                { "name": "org.lwjgl:lwjgl:3.3.3:natives-macos" },
                {
                    "name": "net.minecraftforge:forge:1.21.1-52.0.0:client",
                    "downloads": {
                        "artifact": {
                            "path": "net/minecraftforge/forge/1.21.1-52.0.0/forge-1.21.1-52.0.0-client.jar",
                            "url": "",
                            "size": 12345
                        }
                    }
                }
            ]
        }"#;

        let profile = parse_loader_profile_for_context(
            profile_json,
            ModLoaderKind::Forge,
            Some("52.0.0".to_owned()),
            &win64_ctx,
        )
        .expect("parse forge 1.21.1 profile on windows x86_64");

        assert_eq!(profile.libraries.len(), 2);

        let win_native = &profile.libraries[0];
        assert_eq!(win_native.name, "org.lwjgl:lwjgl:3.3.3:natives-windows");
        assert!(win_native.native_linux.is_some());

        for rejected in [
            "org.lwjgl:lwjgl:3.3.3:natives-windows-x86",
            "org.lwjgl:lwjgl:3.3.3:natives-windows-arm64",
            "org.lwjgl:lwjgl:3.3.3:natives-linux",
            "org.lwjgl:lwjgl:3.3.3:natives-macos",
        ] {
            let raw = RawLoaderLibrary {
                name: rejected.to_owned(),
                url: None,
                sha1: None,
                size: None,
                downloads: None,
                rules: None,
                extract: None,
                natives: None,
            };
            let resolved = raw
                .to_resolved_library_for_context(ModLoaderKind::NeoForge, &win64_ctx)
                .expect("resolve rejected native");
            assert!(
                resolved.artifact.is_none(),
                "{rejected} must be rejected from classpath"
            );
            assert!(
                resolved.native_linux.is_none(),
                "{rejected} must be rejected from natives"
            );
            assert!(!resolved.include_in_classpath);
        }

        let local_forge_client = &profile.libraries[1];
        let art = local_forge_client
            .artifact
            .as_ref()
            .expect("forge client artifact");
        assert_eq!(art.url, "");
    }
}
