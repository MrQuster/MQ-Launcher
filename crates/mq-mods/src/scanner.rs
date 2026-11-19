#![deny(clippy::await_holding_lock)]
//! Parallel `rayon` `.jar` mod scanner (`mq mods scan`).
//!
//! Inspects installed `.jar` and `.jar.disabled` mod archives across all CPU cores,
//! extracting metadata from `fabric.mod.json`, `quilt.mod.json`, `META-INF/mods.toml`,
//! `META-INF/neoforge.mods.toml`, `mcmod.info`, `META-INF/MANIFEST.MF`, and embedded
//! Jar-in-Jar archives (`META-INF/jars/*.jar`, `META-INF/jarjar/*.jar`), computing
//! streaming dual SHA-1/SHA-512 digests, and detecting duplicate mod IDs, missing
//! dependencies, and incompatible mods.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};
use std::time::Instant;

use mq_core::{ModLoaderKind, MqError, MqResult, Sha1Digest, Sha512Digest};
use mq_sched::{ComputePool, EpochGuard};
use rayon::prelude::*;
use sha1::{Digest as _, Sha1};
use sha2::Sha512;
use zip::ZipArchive;

use crate::curseforge::{compute_curseforge_fingerprint_file, CurseForgeClient};
use crate::lockfile::{ModLockEntry, ModLockFile};
use crate::models::{
    Hashes, InstalledModEntry, LockSource, ModConflict, ModScanReport, ProjectKind,
};
use crate::modrinth::ModrinthClient;
use crate::resolver::ContentProvider;
use crate::zip_safe::stream_hash_file_sha1_sha512;

const SCAN_STREAM_CHUNK_SIZE: usize = 64 * 1024;
const MAX_DESCRIPTOR_SIZE: usize = 2 * 1024 * 1024; // 2 MiB descriptor cap
const MAX_EMBEDDED_JAR_SIZE: usize = 32 * 1024 * 1024; // 32 MiB embedded jar cap
const MAX_EMBEDDED_JARS_PER_ARCHIVE: usize = 64;
const MAX_JIJ_DEPTH: usize = 2;

/// Scans a directory of mods in parallel on the CPU compute pool.
pub async fn scan_mods_directory(
    mods_dir: &Path,
    compute_pool: &ComputePool,
    epoch_guard: &EpochGuard,
) -> MqResult<ModScanReport> {
    let analysis = analyze_mods_directory(mods_dir, None, compute_pool, epoch_guard).await?;
    Ok(analysis.report)
}

#[derive(Debug, Clone)]
struct ConditionalDep {
    dep_id: String,
    unless: Vec<String>,
}

#[derive(Debug, Clone)]
struct IncompatRule {
    target_id: String,
    version_constraint: Option<String>,
}

#[derive(Debug, Clone)]
struct LoaderDescriptor {
    loader: ModLoaderKind,
    mod_id: String,
    name: String,
    version: String,
    authors: Vec<String>,
    description: Option<String>,
    additional_ids: Vec<String>,
    provides: Vec<String>,
    required_deps: Vec<ConditionalDep>,
    incompatible_mods: Vec<IncompatRule>,
    mc_constraint: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct ManifestMeta {
    fml_core_plugin: Option<String>,
    fml_core_plugin_contains_fml_mod: bool,
    force_load_as_mod: bool,
    tweak_class: Option<String>,
    mixin_configs: Option<String>,
    fml_mod_type: Option<String>,
    automatic_module_name: Option<String>,
    implementation_title: Option<String>,
    implementation_version: Option<String>,
    specification_title: Option<String>,
}

impl ManifestMeta {
    fn is_explicit_forge_coremod(&self) -> bool {
        self.fml_core_plugin.is_some()
            || self.fml_core_plugin_contains_fml_mod
            || self.force_load_as_mod
            || self.tweak_class.is_some()
    }

    fn is_forge_loader_candidate(&self) -> bool {
        self.is_explicit_forge_coremod() || self.mixin_configs.is_some()
    }

    fn is_library_mod_type(&self) -> bool {
        self.fml_mod_type.as_deref().is_some_and(|t| {
            matches!(
                t.trim().to_ascii_uppercase().as_str(),
                "LIBRARY" | "GAMELIBRARY" | "LANGPROVIDER"
            )
        })
    }

    fn fallback_name(&self) -> Option<&str> {
        self.implementation_title
            .as_deref()
            .or(self.specification_title.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty() && !is_generic_alias(s))
    }

    fn fallback_version(&self) -> Option<&str> {
        self.implementation_version
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !is_placeholder(s))
    }

    fn aliases(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(ref amn) = self.automatic_module_name {
            let trimmed = amn.trim();
            if !trimmed.is_empty() && !is_placeholder(trimmed) {
                if !trimmed.contains('.') && !is_generic_alias(trimmed) {
                    out.push(trimmed.to_string());
                }
                if let Some(last) = trimmed
                    .rsplit('.')
                    .next()
                    .filter(|l| l.len() >= 2 && !is_generic_alias(l))
                {
                    out.push(last.to_string());
                }
            }
        }
        for title in [&self.implementation_title, &self.specification_title]
            .into_iter()
            .flatten()
        {
            let trimmed = title.trim();
            if trimmed.len() >= 2 && !is_placeholder(trimmed) && !is_generic_alias(trimmed) {
                out.push(trimmed.to_string());
            }
        }
        out
    }
}

struct AnalyzedJar {
    entry: InstalledModEntry,
    descriptors: Vec<LoaderDescriptor>,
    supported_loaders: Vec<ModLoaderKind>,
    jij_ids: Vec<String>,
    manifest_aliases: Vec<String>,
    filename_stems: Vec<String>,
}

struct DirectoryAnalysis {
    report: ModScanReport,
    jars: Vec<AnalyzedJar>,
}

async fn analyze_mods_directory(
    mods_dir: &Path,
    target_loader: Option<ModLoaderKind>,
    compute_pool: &ComputePool,
    epoch_guard: &EpochGuard,
) -> MqResult<DirectoryAnalysis> {
    if !mods_dir.exists() {
        return Ok(DirectoryAnalysis {
            report: ModScanReport {
                mods: Vec::new(),
                conflicts: Vec::new(),
                scan_duration_ms: 0,
            },
            jars: Vec::new(),
        });
    }

    let mut candidate_paths = Vec::new();
    let entries = fs::read_dir(mods_dir)?;
    for entry in entries {
        let entry = entry?;
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let fname = entry.file_name().to_string_lossy().to_string();
        if fname.ends_with(".jar") || fname.ends_with(".jar.disabled") {
            candidate_paths.push(p);
        }
    }

    candidate_paths.sort();

    let start = Instant::now();

    compute_pool
        .spawn_cpu_cancellable(epoch_guard, move || {
            let jars: Vec<AnalyzedJar> = candidate_paths
                .par_iter()
                .map(|p| analyze_single_jar(p))
                .filter_map(Result::ok)
                .collect();

            let conflicts = evaluate_conflicts(&jars, target_loader);
            let mods: Vec<InstalledModEntry> = jars.iter().map(|j| j.entry.clone()).collect();
            let elapsed_ms = start.elapsed().as_millis() as u64;

            Ok(DirectoryAnalysis {
                report: ModScanReport {
                    mods,
                    conflicts,
                    scan_duration_ms: elapsed_ms,
                },
                jars,
            })
        })
        .await
        .map_err(|e| MqError::Internal(format!("mod scan failed: {e}")))?
}

fn is_placeholder(s: &str) -> bool {
    let t = s.trim();
    t.is_empty() || t.contains("${") || t.contains("@VERSION@")
}

fn is_generic_alias(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "mod"
            | "core"
            | "api"
            | "lib"
            | "library"
            | "common"
            | "main"
            | "plugin"
            | "client"
            | "server"
            | "universal"
            | "root"
            | "app"
            | "minecraft"
            | "forge"
            | "fabric"
            | "neoforge"
            | "quilt"
            | "unknown"
            | "test"
            | "examplemod"
            | "example"
            | "java"
            | "src"
            | "resources"
            | "assets"
            | "impl"
    )
}

fn is_builtin_mod_id(id_lower: &str) -> bool {
    matches!(
        id_lower,
        "minecraft"
            | "java"
            | "fabricloader"
            | "fabric-loader"
            | "quilt_loader"
            | "quilt-loader"
            | "forge"
            | "minecraftforge"
            | "mod_minecraftforge"
            | "neoforge"
            | "fml"
            | "cpw.mods.fml"
            | "fmlcore"
            | "lowcodelanguage"
            | "javafml"
            | "mcp"
            | "modloader"
    )
}

fn id_alias_forms(raw: &str) -> Vec<String> {
    let lower = raw.trim().to_ascii_lowercase();
    if lower.is_empty() || is_placeholder(&lower) {
        return Vec::new();
    }
    let mut forms = Vec::with_capacity(4);
    let mut push_unique = |s: String| {
        if !s.is_empty() && !forms.iter().any(|existing| existing == &s) {
            forms.push(s);
        }
    };
    push_unique(lower.clone());
    push_unique(lower.replace(['_', ' '], "-"));
    push_unique(lower.replace(['-', ' '], "_"));
    push_unique(lower.replace(['-', '_', ' '], ""));
    forms
}

fn id_exact_separator_forms(raw: &str) -> Vec<String> {
    let lower = raw.trim().to_ascii_lowercase();
    if lower.is_empty() || is_placeholder(&lower) {
        return Vec::new();
    }
    let mut forms = Vec::with_capacity(3);
    let mut push_unique = |s: String| {
        if !s.is_empty() && !forms.iter().any(|existing| existing == &s) {
            forms.push(s);
        }
    };
    push_unique(lower.clone());
    push_unique(lower.replace('_', "-"));
    push_unique(lower.replace('-', "_"));
    forms
}

fn register_satisfied_id(satisfied_ids: &mut BTreeSet<String>, raw: &str) {
    for form in id_alias_forms(raw) {
        satisfied_ids.insert(form);
    }
    let lower = raw.trim().to_ascii_lowercase();
    if lower == "fabric-api" || lower == "quilted_fabric_api" {
        satisfied_ids.insert("fabric".to_string());
        satisfied_ids.insert("fabric-api".to_string());
    } else if lower == "fabric" {
        satisfied_ids.insert("fabric-api".to_string());
    }
}

fn is_dependency_satisfied(dep: &str, satisfied_ids: &BTreeSet<String>) -> bool {
    let lower = dep.trim().to_ascii_lowercase();
    if lower.is_empty() || is_builtin_mod_id(&lower) {
        return true;
    }
    for form in id_alias_forms(&lower) {
        if satisfied_ids.contains(&form) {
            return true;
        }
    }
    if lower.starts_with("fabric-")
        && !matches!(
            lower.as_str(),
            "fabric-language-kotlin" | "fabric-carpet" | "fabric-loader"
        )
        && (satisfied_ids.contains("fabric-api")
            || satisfied_ids.contains("quilted_fabric_api")
            || satisfied_ids.contains("fabric"))
    {
        return true;
    }
    if (lower.starts_with("quilt_") || lower.starts_with("quilt-"))
        && (satisfied_ids.contains("quilted_fabric_api") || satisfied_ids.contains("qsl"))
    {
        return true;
    }
    false
}

fn is_filename_stop_token(
    tok: &str,
    prev_tok: Option<&str>,
    next_tok: Option<&str>,
    is_first_kept: bool,
) -> bool {
    let base = tok
        .trim()
        .to_ascii_lowercase()
        .split('+')
        .next()
        .unwrap_or("")
        .to_string();
    if base.is_empty() {
        return true;
    }
    if base.starts_with('[') || base.starts_with('(') {
        return true;
    }
    let bytes = base.as_bytes();
    if bytes[0].is_ascii_digit()
        && (base.contains('.')
            || base.chars().all(|c| c.is_ascii_digit())
            || base.ends_with('x')
            || base.contains('r')
            || base.contains('w'))
    {
        return true;
    }
    if (base.starts_with('v') || base.starts_with('r') || base.starts_with('b'))
        && bytes.get(1).is_some_and(|b| b.is_ascii_digit())
    {
        return true;
    }
    if base.starts_with("rv") && bytes.get(2).is_some_and(|b| b.is_ascii_digit()) {
        return true;
    }
    if base.starts_with("mc") && bytes.get(2).is_some_and(|b| b.is_ascii_digit()) {
        return true;
    }
    if base == "for"
        && next_tok.is_some_and(|next| {
            let nl = next.trim().to_ascii_lowercase();
            let nb = nl.as_bytes();
            nb.first().is_some_and(|b| b.is_ascii_digit())
                || (nl.starts_with("mc") && nb.get(2).is_some_and(|b| b.is_ascii_digit()))
        })
    {
        return true;
    }
    if !is_first_kept {
        let prev_is_for = prev_tok.is_some_and(|p| p.eq_ignore_ascii_case("for"));
        if matches!(
            base.as_str(),
            "neoforge"
                | "fabric"
                | "quilt"
                | "liteloader"
                | "rift"
                | "universal"
                | "client"
                | "server"
                | "common"
                | "dev"
                | "deobf"
                | "sources"
                | "shadow"
                | "all"
                | "hd"
                | "snapshot"
                | "alpha"
                | "beta"
                | "release"
                | "rc"
                | "stable"
                | "backport"
                | "build"
        ) || (base == "forge" && !prev_is_for)
        {
            return true;
        }
    }
    false
}

/// Extracts a clean display stem and safe alias stems from a `.jar` / `.jar.disabled` filename.
/// For example, `CreativeCore_v1.10.70_mc1.12.2.jar` -> `("CreativeCore", ["CreativeCore"])`.
fn extract_filename_stems(file_name: &str) -> (String, Vec<String>) {
    let mut raw_stem = file_name;
    if let Some(stripped) = raw_stem.strip_suffix(".disabled") {
        raw_stem = stripped;
    }
    if let Some(stripped) = raw_stem.strip_suffix(".jar") {
        raw_stem = stripped;
    }

    let mut working = raw_stem.trim();
    loop {
        if let Some(after) = working
            .strip_prefix('[')
            .and_then(|s| s.split_once(']'))
            .map(|(_, r)| r.trim_start_matches(['-', '_', ' ']))
            .filter(|a| !a.is_empty())
        {
            working = after;
            continue;
        }
        if let Some(after) = working
            .strip_prefix('(')
            .and_then(|s| s.split_once(')'))
            .map(|(_, r)| r.trim_start_matches(['-', '_', ' ']))
            .filter(|a| !a.is_empty())
        {
            working = after;
            continue;
        }
        break;
    }

    // Split into tokens while preserving the separator immediately preceding each token
    let mut tokens: Vec<(Option<char>, &str)> = Vec::new();
    let mut seg_start = 0usize;
    let mut prev_sep: Option<char> = None;
    for (idx, ch) in working.char_indices() {
        if matches!(ch, '-' | '_' | ' ') {
            if idx > seg_start {
                tokens.push((prev_sep, &working[seg_start..idx]));
            }
            prev_sep = Some(ch);
            seg_start = idx + ch.len_utf8();
        }
    }
    if seg_start < working.len() {
        tokens.push((prev_sep, &working[seg_start..]));
    }

    // Skip leading version / loader prefixes (e.g. "1.12.2-forge-somemod-1.0")
    let mut start_idx = 0usize;
    while start_idx < tokens.len() {
        let tok = tokens[start_idx].1;
        let next_tok = tokens.get(start_idx + 1).map(|t| t.1);
        let tl = tok.to_ascii_lowercase();
        let is_leading_meta = is_filename_stop_token(tok, None, next_tok, false)
            && start_idx + 1 < tokens.len()
            && !matches!(tl.as_str(), "forge" | "fabric" | "quilt" | "neoforge" if next_tok.is_some_and(|n| is_filename_stop_token(n, Some(tok), None, false)));
        if is_leading_meta {
            start_idx += 1;
        } else {
            break;
        }
    }

    let mut kept: Vec<(Option<char>, &str)> = Vec::new();
    for i in start_idx..tokens.len() {
        let (sep, tok) = tokens[i];
        let prev_tok = if i > start_idx {
            Some(tokens[i - 1].1)
        } else {
            None
        };
        let next_tok = tokens.get(i + 1).map(|t| t.1);
        let is_first = kept.is_empty();
        if !is_first && is_filename_stop_token(tok, prev_tok, next_tok, is_first) {
            break;
        }
        // Handle inline `+` build suffix if trailing (e.g. `modname+1.20.1`)
        let clean_tok = if let Some((before_plus, after_plus)) = tok.split_once('+') {
            if !before_plus.is_empty()
                && (after_plus
                    .as_bytes()
                    .first()
                    .is_some_and(|b| b.is_ascii_digit())
                    || after_plus.to_ascii_lowercase().starts_with("mc"))
            {
                before_plus
            } else {
                tok
            }
        } else {
            tok
        };
        kept.push((sep, clean_tok));
    }

    let mut clean_display = String::new();
    for (idx, (sep, tok)) in kept.iter().enumerate() {
        if idx > 0 {
            if let Some(ch) = sep {
                clean_display.push(*ch);
            }
        }
        clean_display.push_str(tok);
    }

    let mut stems = Vec::new();
    if !clean_display.is_empty() && !is_generic_alias(&clean_display) {
        stems.push(clean_display.clone());
    }
    let display = if !clean_display.is_empty() {
        clean_display
    } else {
        raw_stem.to_string()
    };
    if !raw_stem.is_empty() && !stems.iter().any(|s| s.eq_ignore_ascii_case(raw_stem)) {
        stems.push(raw_stem.to_string());
    }
    (display, stems)
}

/// Sanitizes real-world `mcmod.info` / JSON descriptors:
/// strips UTF-8 BOM, removes `//` and `/* */` comments outside strings,
/// escapes raw newlines/tabs inside JSON string literals, and strips trailing commas.
fn sanitize_lenient_json(raw: &str) -> String {
    let without_bom = raw.strip_prefix('\u{FEFF}').unwrap_or(raw);

    // Pass 1: strip comments outside strings & escape raw control chars inside strings.
    let mut pass1 = String::with_capacity(without_bom.len());
    let mut chars = without_bom.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if in_string {
            if escaped {
                pass1.push(ch);
                escaped = false;
            } else if ch == '\\' {
                pass1.push(ch);
                escaped = true;
            } else if ch == '"' {
                pass1.push(ch);
                in_string = false;
            } else if ch == '\n' {
                pass1.push_str("\\n");
            } else if ch == '\r' {
                if chars.peek() != Some(&'\n') {
                    pass1.push_str("\\r");
                }
            } else if ch == '\t' {
                pass1.push_str("\\t");
            } else if (ch as u32) < 0x20 {
                // Skip invalid raw ASCII control characters inside string literals
            } else {
                pass1.push(ch);
            }
        } else if ch == '"' {
            pass1.push(ch);
            in_string = true;
        } else if ch == '/' {
            match chars.peek() {
                Some('/') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if c == '\n' {
                            pass1.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut prev_star = false;
                    for c in chars.by_ref() {
                        if prev_star && c == '/' {
                            break;
                        }
                        prev_star = c == '*';
                    }
                    pass1.push(' ');
                }
                _ => pass1.push('/'),
            }
        } else {
            pass1.push(ch);
        }
    }

    // Pass 2: remove trailing commas before `]` or `}` outside strings.
    let mut pass2 = String::with_capacity(pass1.len());
    let mut p_chars = pass1.chars().peekable();
    in_string = false;
    escaped = false;

    while let Some(ch) = p_chars.next() {
        if in_string {
            pass2.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            pass2.push(ch);
            in_string = true;
        } else if ch == ',' {
            let mut look = p_chars.clone();
            let mut trailing = false;
            while let Some(&next_ch) = look.peek() {
                if next_ch.is_ascii_whitespace() {
                    look.next();
                } else {
                    trailing = matches!(next_ch, ']' | '}');
                    break;
                }
            }
            if !trailing {
                pass2.push(',');
            }
        } else {
            pass2.push(ch);
        }
    }

    pass2
}

fn parse_lenient_json_value(bytes: &[u8]) -> Option<serde_json::Value> {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
        return Some(v);
    }
    let text = String::from_utf8_lossy(bytes);
    let cleaned = sanitize_lenient_json(&text);
    serde_json::from_str::<serde_json::Value>(&cleaned).ok()
}

fn read_zip_entry_bytes<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    candidates: &[&str],
    max_bytes: usize,
) -> Option<Vec<u8>> {
    for &name in candidates {
        if let Ok(entry) = archive.by_name(name) {
            let mut buf = Vec::with_capacity(entry.size().min(max_bytes as u64) as usize);
            let _ = entry.take(max_bytes as u64).read_to_end(&mut buf);
            return Some(buf);
        }
    }
    // Case-insensitive and `./`-trimmed fallback lookup
    let len = archive.len();
    for i in 0..len {
        let Ok(entry) = archive.by_index(i) else {
            continue;
        };
        let norm = entry.name().trim_start_matches("./");
        if candidates
            .iter()
            .any(|cand| norm.eq_ignore_ascii_case(cand))
        {
            let mut buf = Vec::with_capacity(entry.size().min(max_bytes as u64) as usize);
            let _ = entry.take(max_bytes as u64).read_to_end(&mut buf);
            return Some(buf);
        }
    }
    None
}

fn parse_manifest_mf(bytes: &[u8]) -> ManifestMeta {
    let text = String::from_utf8_lossy(bytes);
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");

    let mut unfolded_lines: Vec<String> = Vec::new();
    let mut started = false;
    for raw_line in normalized.lines() {
        if raw_line.trim().is_empty() {
            if started {
                // Stop at the end of the main attributes section
                break;
            }
            continue;
        }
        started = true;
        if let Some(cont) = raw_line.strip_prefix(' ') {
            if let Some(last) = unfolded_lines.last_mut() {
                last.push_str(cont);
            }
        } else {
            unfolded_lines.push(raw_line.to_string());
        }
    }

    let mut meta = ManifestMeta::default();
    for line in unfolded_lines {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim();
        if val.is_empty() {
            continue;
        }
        if key.eq_ignore_ascii_case("FMLCorePlugin") {
            meta.fml_core_plugin = Some(val.to_string());
        } else if key.eq_ignore_ascii_case("FMLCorePluginContainsFMLMod") {
            meta.fml_core_plugin_contains_fml_mod = val.eq_ignore_ascii_case("true");
        } else if key.eq_ignore_ascii_case("ForceLoadAsMod") {
            meta.force_load_as_mod = val.eq_ignore_ascii_case("true");
        } else if key.eq_ignore_ascii_case("TweakClass") {
            meta.tweak_class = Some(val.to_string());
        } else if key.eq_ignore_ascii_case("MixinConfigs") {
            meta.mixin_configs = Some(val.to_string());
        } else if key.eq_ignore_ascii_case("FMLModType") {
            meta.fml_mod_type = Some(val.to_string());
        } else if key.eq_ignore_ascii_case("Automatic-Module-Name") {
            meta.automatic_module_name = Some(val.to_string());
        } else if key.eq_ignore_ascii_case("Implementation-Title") {
            meta.implementation_title = Some(val.to_string());
        } else if key.eq_ignore_ascii_case("Implementation-Version") {
            meta.implementation_version = Some(val.to_string());
        } else if key.eq_ignore_ascii_case("Specification-Title") {
            meta.specification_title = Some(val.to_string());
        }
    }
    meta
}

fn json_version_constraint(val: &serde_json::Value) -> Option<String> {
    if let Some(s) = val.as_str() {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    } else if let Some(arr) = val.as_array() {
        let joined: Vec<&str> = arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if !joined.is_empty() {
            return Some(joined.join(" || "));
        }
    }
    None
}

fn parse_fabric_mod_json(bytes: &[u8]) -> Option<(LoaderDescriptor, Vec<String>)> {
    let val = parse_lenient_json_value(bytes)?;
    let mod_id = val
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_placeholder(s))?
        .to_string();

    let name = val
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_placeholder(s))
        .unwrap_or(&mod_id)
        .to_string();
    let version = val
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_placeholder(s))
        .unwrap_or("1.0.0")
        .to_string();
    let description = val
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut authors = Vec::new();
    if let Some(arr) = val.get("authors").and_then(|v| v.as_array()) {
        for a in arr {
            if let Some(s) = a.as_str() {
                authors.push(s.to_string());
            } else if let Some(n) = a.get("name").and_then(|v| v.as_str()) {
                authors.push(n.to_string());
            }
        }
    }

    let mut provides = Vec::new();
    if let Some(arr) = val.get("provides").and_then(|v| v.as_array()) {
        for p in arr {
            if let Some(s) = p.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                provides.push(s.to_string());
            }
        }
    }

    let mut required_deps = Vec::new();
    let mut mc_constraint = None;
    if let Some(dep_map) = val.get("depends").and_then(|v| v.as_object()) {
        for (k, v) in dep_map {
            let key = k.trim();
            if key.eq_ignore_ascii_case("minecraft") {
                mc_constraint = json_version_constraint(v);
            }
            if !key.is_empty() && !is_placeholder(key) {
                required_deps.push(ConditionalDep {
                    dep_id: key.to_string(),
                    unless: Vec::new(),
                });
            }
        }
    }

    // `recommends` and `suggests` are intentionally ignored (optional only).
    let mut incompatible_mods = Vec::new();
    for field in ["breaks", "conflicts"] {
        if let Some(map) = val.get(field).and_then(|v| v.as_object()) {
            for (k, v) in map {
                let target = k.trim();
                if !target.is_empty()
                    && !is_placeholder(target)
                    && !is_builtin_mod_id(&target.to_ascii_lowercase())
                {
                    incompatible_mods.push(IncompatRule {
                        target_id: target.to_string(),
                        version_constraint: json_version_constraint(v),
                    });
                }
            }
        }
    }

    let mut extra_jar_paths = Vec::new();
    if let Some(jars) = val.get("jars").and_then(|v| v.as_array()) {
        for j in jars {
            if let Some(path) = j.get("file").and_then(|v| v.as_str()) {
                let p = path.trim().trim_start_matches("./");
                if !p.is_empty() {
                    extra_jar_paths.push(p.to_string());
                }
            }
        }
    }

    Some((
        LoaderDescriptor {
            loader: ModLoaderKind::Fabric,
            mod_id,
            name,
            version,
            authors,
            description,
            additional_ids: Vec::new(),
            provides,
            required_deps,
            incompatible_mods,
            mc_constraint,
        },
        extra_jar_paths,
    ))
}

fn parse_quilt_mod_json(bytes: &[u8]) -> Option<(LoaderDescriptor, Vec<String>)> {
    let val = parse_lenient_json_value(bytes)?;
    let ql = val.get("quilt_loader")?;
    let mod_id = ql
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_placeholder(s))?
        .to_string();

    let version = ql
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_placeholder(s))
        .unwrap_or("1.0.0")
        .to_string();
    let meta = ql.get("metadata");
    let name = meta
        .and_then(|m| m.get("name"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_placeholder(s))
        .unwrap_or(&mod_id)
        .to_string();
    let description = meta
        .and_then(|m| m.get("description"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut authors = Vec::new();
    if let Some(contribs) = meta.and_then(|m| m.get("contributors")) {
        if let Some(map) = contribs.as_object() {
            for k in map.keys() {
                authors.push(k.clone());
            }
        } else if let Some(arr) = contribs.as_array() {
            for a in arr {
                if let Some(s) = a.as_str() {
                    authors.push(s.to_string());
                }
            }
        }
    }

    let mut provides = Vec::new();
    if let Some(arr) = ql.get("provides").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(s) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                provides.push(s.to_string());
            } else if let Some(s) = item
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                provides.push(s.to_string());
            }
        }
    }

    let mut required_deps = Vec::new();
    let mut mc_constraint = None;
    if let Some(arr) = ql.get("depends").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(s) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                required_deps.push(ConditionalDep {
                    dep_id: s.to_string(),
                    unless: Vec::new(),
                });
            } else if item.is_object() {
                let is_optional = item
                    .get("optional")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if is_optional {
                    continue;
                }
                let Some(dep_id) = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && !is_placeholder(s))
                else {
                    continue;
                };
                if dep_id.eq_ignore_ascii_case("minecraft") {
                    mc_constraint = item.get("versions").and_then(json_version_constraint);
                }
                let mut unless = Vec::new();
                if let Some(u) = item.get("unless") {
                    if let Some(s) = u.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                        unless.push(s.to_string());
                    } else if let Some(id_s) = u.get("id").and_then(|v| v.as_str()) {
                        unless.push(id_s.trim().to_string());
                    } else if let Some(u_arr) = u.as_array() {
                        for elem in u_arr {
                            if let Some(s) = elem.as_str().map(str::trim).filter(|s| !s.is_empty())
                            {
                                unless.push(s.to_string());
                            } else if let Some(id_s) = elem.get("id").and_then(|v| v.as_str()) {
                                unless.push(id_s.trim().to_string());
                            }
                        }
                    }
                }
                required_deps.push(ConditionalDep {
                    dep_id: dep_id.to_string(),
                    unless,
                });
            }
        }
    }

    let mut incompatible_mods = Vec::new();
    if let Some(arr) = ql.get("breaks").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(s) = item
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !is_builtin_mod_id(&s.to_ascii_lowercase()))
            {
                incompatible_mods.push(IncompatRule {
                    target_id: s.to_string(),
                    version_constraint: None,
                });
            } else if item.is_object() {
                if item
                    .get("optional")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    continue;
                }
                if let Some(id) = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| {
                        !s.is_empty()
                            && !is_placeholder(s)
                            && !is_builtin_mod_id(&s.to_ascii_lowercase())
                    })
                {
                    incompatible_mods.push(IncompatRule {
                        target_id: id.to_string(),
                        version_constraint: item.get("versions").and_then(json_version_constraint),
                    });
                }
            }
        }
    }

    let mut extra_jar_paths = Vec::new();
    if let Some(jars) = ql.get("jars").and_then(|v| v.as_array()) {
        for j in jars {
            if let Some(s) = j.as_str() {
                let p = s.trim().trim_start_matches("./");
                if !p.is_empty() {
                    extra_jar_paths.push(p.to_string());
                }
            } else if let Some(s) = j.get("file").and_then(|v| v.as_str()) {
                let p = s.trim().trim_start_matches("./");
                if !p.is_empty() {
                    extra_jar_paths.push(p.to_string());
                }
            }
        }
    }

    Some((
        LoaderDescriptor {
            loader: ModLoaderKind::Quilt,
            mod_id,
            name,
            version,
            authors,
            description,
            additional_ids: Vec::new(),
            provides,
            required_deps,
            incompatible_mods,
            mc_constraint,
        },
        extra_jar_paths,
    ))
}

fn parse_mods_toml(
    bytes: &[u8],
    default_loader: ModLoaderKind,
    manifest: &ManifestMeta,
) -> Option<LoaderDescriptor> {
    let text = std::str::from_utf8(bytes).ok()?;
    let val = text.parse::<toml::Value>().ok()?;
    let mods_arr = val.get("mods").and_then(|v| v.as_array())?;

    let mut all_ids: Vec<String> = Vec::new();
    let mut primary_mod: Option<&toml::Value> = None;

    for entry in mods_arr {
        if let Some(id) = entry
            .get("modId")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty() && !is_placeholder(s))
        {
            if primary_mod.is_none() {
                primary_mod = Some(entry);
            }
            all_ids.push(id.to_string());
        }
    }

    let first_mod = primary_mod?;
    let mod_id = all_ids[0].clone();
    let name = first_mod
        .get("displayName")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_placeholder(s))
        .unwrap_or(&mod_id)
        .to_string();
    let version = first_mod
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_placeholder(s))
        .or_else(|| manifest.fallback_version())
        .unwrap_or("1.0.0")
        .to_string();
    let description = first_mod
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let authors = match first_mod.get("authors") {
        Some(toml::Value::String(s)) if !s.trim().is_empty() => vec![s.trim().to_string()],
        Some(toml::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };

    let mut required_deps = Vec::new();
    let mut incompatible_mods = Vec::new();
    let mut mc_constraint = None;
    let mut mentions_neoforge = false;
    let mut mentions_forge = false;

    if let Some(dep_table) = val.get("dependencies").and_then(|v| v.as_table()) {
        for (_target_mod, dep_list) in dep_table {
            let Some(arr) = dep_list.as_array() else {
                continue;
            };
            for d in arr {
                let Some(dep_id) = d
                    .get("modId")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && !is_placeholder(s))
                else {
                    continue;
                };

                if dep_id.eq_ignore_ascii_case("neoforge") {
                    mentions_neoforge = true;
                } else if dep_id.eq_ignore_ascii_case("forge") {
                    mentions_forge = true;
                }

                // Respect `side`: ignore SERVER-only dependencies in a client launcher
                let side = d
                    .get("side")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .unwrap_or("BOTH");
                if side.eq_ignore_ascii_case("SERVER") {
                    continue;
                }

                // Respect NeoForge `type` ("required" | "optional" | "discouraged" | "incompatible")
                // and Forge `mandatory` (bool, default true when omitted).
                let dep_type = d
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_ascii_lowercase());

                match dep_type.as_deref() {
                    Some("incompatible") => {
                        if !is_builtin_mod_id(&dep_id.to_ascii_lowercase()) {
                            let version_constraint = d
                                .get("versionRange")
                                .and_then(|v| v.as_str())
                                .map(str::trim)
                                .filter(|s| !s.is_empty() && !is_placeholder(s))
                                .map(str::to_string);
                            incompatible_mods.push(IncompatRule {
                                target_id: dep_id.to_string(),
                                version_constraint,
                            });
                        }
                        continue;
                    }
                    Some("optional") | Some("discouraged") => {
                        continue;
                    }
                    Some("required") => {}
                    _ => {
                        let mandatory =
                            d.get("mandatory").and_then(|v| v.as_bool()).unwrap_or(true);
                        if !mandatory {
                            continue;
                        }
                    }
                }

                if dep_id.eq_ignore_ascii_case("minecraft") && mc_constraint.is_none() {
                    mc_constraint = d
                        .get("versionRange")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty() && !is_placeholder(s))
                        .map(str::to_string);
                }

                if !is_builtin_mod_id(&dep_id.to_ascii_lowercase()) {
                    required_deps.push(ConditionalDep {
                        dep_id: dep_id.to_string(),
                        unless: Vec::new(),
                    });
                }
            }
        }
    }

    let resolved_loader =
        if default_loader == ModLoaderKind::Forge && mentions_neoforge && !mentions_forge {
            ModLoaderKind::NeoForge
        } else {
            default_loader
        };

    Some(LoaderDescriptor {
        loader: resolved_loader,
        mod_id,
        name,
        version,
        authors,
        description,
        additional_ids: all_ids.into_iter().skip(1).collect(),
        provides: Vec::new(),
        required_deps,
        incompatible_mods,
        mc_constraint,
    })
}

fn parse_forge_dep_spec(raw: &str, from_required_mods: bool) -> Vec<String> {
    let mut out = Vec::new();
    for part in raw.split(';') {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        let lower = token.to_ascii_lowercase();
        let (is_required, rest) = if let Some(r) = lower
            .strip_prefix("required-after:")
            .map(|_| &token["required-after:".len()..])
            .or_else(|| {
                lower
                    .strip_prefix("required-before:")
                    .map(|_| &token["required-before:".len()..])
            })
            .or_else(|| {
                lower
                    .strip_prefix("required:")
                    .map(|_| &token["required:".len()..])
            }) {
            (true, r)
        } else if let Some(r) = lower
            .strip_prefix("after:")
            .map(|_| &token["after:".len()..])
            .or_else(|| {
                lower
                    .strip_prefix("before:")
                    .map(|_| &token["before:".len()..])
            })
        {
            (false, r)
        } else {
            (from_required_mods, token)
        };

        if !is_required {
            continue;
        }

        let id = rest.split(['@', '[', '(']).next().unwrap_or("").trim();
        if id.is_empty() || is_placeholder(id) || id == "*" {
            continue;
        }
        let id_lower = id.to_ascii_lowercase();
        if matches!(id_lower.as_str(), "client" | "server") || is_builtin_mod_id(&id_lower) {
            continue;
        }
        out.push(id.to_string());
    }
    out
}

/// Parses Forge 1.7.10-1.12.2 `mcmod.info`.
/// Returns `(Option<LoaderDescriptor>, saw_mcmod_file)` so even placeholder-only `mcmod.info`
/// marks the jar as supporting `ModLoaderKind::Forge`.
fn parse_mcmod_info(bytes: &[u8], manifest: &ManifestMeta) -> (Option<LoaderDescriptor>, bool) {
    let Some(val) = parse_lenient_json_value(bytes) else {
        return (None, false);
    };

    let entries: Vec<&serde_json::Value> = match &val {
        serde_json::Value::Array(arr) => arr.iter().collect(),
        serde_json::Value::Object(obj) => {
            if let Some(list) = obj.get("modList").and_then(|v| v.as_array()) {
                list.iter().collect()
            } else if obj.contains_key("modid") || obj.contains_key("modId") {
                vec![&val]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    };

    if entries.is_empty() {
        return (None, true);
    }

    let mut valid_entries = Vec::new();
    for entry in entries {
        let modid = entry
            .get("modid")
            .or_else(|| entry.get("modId"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty() && !is_placeholder(s));
        if let Some(id) = modid {
            valid_entries.push((id.to_string(), entry));
        }
    }

    if valid_entries.is_empty() {
        return (None, true);
    }

    // Prefer the first root entry (one without `"parent"`), falling back to index 0
    let primary_idx = valid_entries
        .iter()
        .position(|(_, e)| {
            e.get("parent")
                .and_then(|v| v.as_str())
                .is_none_or(|p| p.trim().is_empty())
        })
        .unwrap_or(0);

    let (primary_id, primary_entry) = &valid_entries[primary_idx];
    let name = primary_entry
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_placeholder(s))
        .or_else(|| manifest.fallback_name())
        .unwrap_or(primary_id)
        .to_string();
    let version = primary_entry
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_placeholder(s))
        .or_else(|| manifest.fallback_version())
        .unwrap_or("1.0.0")
        .to_string();
    let description = primary_entry
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut authors = Vec::new();
    for field in ["authorList", "authors"] {
        if let Some(val_a) = primary_entry.get(field) {
            if let Some(arr) = val_a.as_array() {
                for a in arr {
                    if let Some(s) = a.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                        authors.push(s.to_string());
                    }
                }
            } else if let Some(s) = val_a.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                authors.push(s.to_string());
            }
        }
        if !authors.is_empty() {
            break;
        }
    }

    let mut additional_ids = Vec::new();
    let mut required_deps = Vec::new();

    for (idx, (entry_id, entry)) in valid_entries.iter().enumerate() {
        if idx != primary_idx {
            additional_ids.push(entry_id.clone());
        }
        let use_dep_info = entry
            .get("useDependencyInformation")
            .and_then(|v| v.as_bool());
        if use_dep_info == Some(false) {
            continue;
        }

        if let Some(req_arr) = entry.get("requiredMods").and_then(|v| v.as_array()) {
            for item in req_arr {
                if let Some(s) = item.as_str() {
                    for dep in parse_forge_dep_spec(s, true) {
                        required_deps.push(ConditionalDep {
                            dep_id: dep,
                            unless: Vec::new(),
                        });
                    }
                }
            }
        }
        if let Some(dep_arr) = entry.get("dependencies").and_then(|v| v.as_array()) {
            for item in dep_arr {
                if let Some(s) = item.as_str() {
                    for dep in parse_forge_dep_spec(s, false) {
                        required_deps.push(ConditionalDep {
                            dep_id: dep,
                            unless: Vec::new(),
                        });
                    }
                }
            }
        }
    }

    (
        Some(LoaderDescriptor {
            loader: ModLoaderKind::Forge,
            mod_id: primary_id.clone(),
            name,
            version,
            authors,
            description,
            additional_ids,
            provides: Vec::new(),
            required_deps,
            incompatible_mods: Vec::new(),
            mc_constraint: None,
        }),
        true,
    )
}

fn parse_jarjar_metadata(bytes: &[u8]) -> (Vec<String>, Vec<String>) {
    let mut paths = Vec::new();
    let mut artifact_ids = Vec::new();
    let Some(val) = parse_lenient_json_value(bytes) else {
        return (paths, artifact_ids);
    };
    if let Some(arr) = val.get("jars").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(p) = item.get("path").and_then(|v| v.as_str()) {
                let norm = p.trim().trim_start_matches("./");
                if !norm.is_empty() {
                    paths.push(norm.to_string());
                }
            }
            if let Some(art) = item
                .get("identifier")
                .and_then(|i| i.get("artifact"))
                .and_then(|v| v.as_str())
            {
                let clean = art.trim();
                if !clean.is_empty() && !is_generic_alias(clean) {
                    artifact_ids.push(clean.to_string());
                }
            }
        }
    }
    (paths, artifact_ids)
}

fn collect_embedded_jij_ids<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    extra_paths: &[String],
    depth: usize,
    out_ids: &mut Vec<String>,
) {
    if depth > MAX_JIJ_DEPTH {
        return;
    }

    let mut jarjar_paths = Vec::new();
    if let Some(jj_bytes) = read_zip_entry_bytes(
        archive,
        &["META-INF/jarjar/metadata.json"],
        MAX_DESCRIPTOR_SIZE,
    ) {
        let (paths, artifacts) = parse_jarjar_metadata(&jj_bytes);
        jarjar_paths = paths;
        out_ids.extend(artifacts);
    }

    let mut candidate_indices: Vec<(usize, String)> = Vec::new();
    let len = archive.len();
    for i in 0..len {
        let Ok(entry) = archive.by_index(i) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let norm = entry.name().trim_start_matches("./").to_string();
        let norm_lower = norm.to_ascii_lowercase();
        let is_jij_dir = (norm_lower.starts_with("meta-inf/jars/")
            || norm_lower.starts_with("meta-inf/jarjar/"))
            && norm_lower.ends_with(".jar");
        let is_explicit = extra_paths
            .iter()
            .chain(jarjar_paths.iter())
            .any(|p| p.eq_ignore_ascii_case(&norm))
            && norm_lower.ends_with(".jar");

        if is_jij_dir || is_explicit {
            candidate_indices.push((i, norm));
            if candidate_indices.len() >= MAX_EMBEDDED_JARS_PER_ARCHIVE {
                break;
            }
        }
    }

    for (idx, nested_path) in candidate_indices {
        let Ok(entry) = archive.by_index(idx) else {
            continue;
        };
        if entry.size() > MAX_EMBEDDED_JAR_SIZE as u64 {
            continue;
        }
        let mut nested_bytes = Vec::new();
        if entry
            .take((MAX_EMBEDDED_JAR_SIZE + 1) as u64)
            .read_to_end(&mut nested_bytes)
            .is_err()
        {
            continue;
        }
        if nested_bytes.len() > MAX_EMBEDDED_JAR_SIZE {
            continue;
        }

        let nested_fname = nested_path.rsplit('/').next().unwrap_or(&nested_path);
        let (_, nested_stems) = extract_filename_stems(nested_fname);
        out_ids.extend(nested_stems);

        let Ok(mut nested_archive) = ZipArchive::new(Cursor::new(&nested_bytes)) else {
            continue;
        };

        let nested_manifest = read_zip_entry_bytes(
            &mut nested_archive,
            &["META-INF/MANIFEST.MF"],
            MAX_DESCRIPTOR_SIZE,
        )
        .map(|b| parse_manifest_mf(&b))
        .unwrap_or_default();
        out_ids.extend(nested_manifest.aliases());

        let mut sub_extra_paths = Vec::new();

        if let Some((desc, paths)) = read_zip_entry_bytes(
            &mut nested_archive,
            &["fabric.mod.json"],
            MAX_DESCRIPTOR_SIZE,
        )
        .and_then(|b| parse_fabric_mod_json(&b))
        {
            out_ids.push(desc.mod_id);
            out_ids.extend(desc.additional_ids);
            out_ids.extend(desc.provides);
            sub_extra_paths.extend(paths);
        }
        if let Some((desc, paths)) = read_zip_entry_bytes(
            &mut nested_archive,
            &["quilt.mod.json"],
            MAX_DESCRIPTOR_SIZE,
        )
        .and_then(|b| parse_quilt_mod_json(&b))
        {
            out_ids.push(desc.mod_id);
            out_ids.extend(desc.additional_ids);
            out_ids.extend(desc.provides);
            sub_extra_paths.extend(paths);
        }
        for (toml_path, loader) in [
            ("META-INF/neoforge.mods.toml", ModLoaderKind::NeoForge),
            ("META-INF/mods.toml", ModLoaderKind::Forge),
        ] {
            if let Some(desc) =
                read_zip_entry_bytes(&mut nested_archive, &[toml_path], MAX_DESCRIPTOR_SIZE)
                    .and_then(|b| parse_mods_toml(&b, loader, &nested_manifest))
            {
                out_ids.push(desc.mod_id);
                out_ids.extend(desc.additional_ids);
            }
        }
        if let Some(desc) =
            read_zip_entry_bytes(&mut nested_archive, &["mcmod.info"], MAX_DESCRIPTOR_SIZE)
                .and_then(|b| parse_mcmod_info(&b, &nested_manifest).0)
        {
            out_ids.push(desc.mod_id);
            out_ids.extend(desc.additional_ids);
        }

        if depth < MAX_JIJ_DEPTH {
            collect_embedded_jij_ids(&mut nested_archive, &sub_extra_paths, depth + 1, out_ids);
        }
    }
}

fn push_unique_loader(loaders: &mut Vec<ModLoaderKind>, ld: ModLoaderKind) {
    if !loaders.contains(&ld) {
        loaders.push(ld);
    }
}

fn analyze_single_jar(path: &Path) -> MqResult<AnalyzedJar> {
    let file_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let enabled = file_name.ends_with(".jar");

    let file = File::open(path)?;
    let metadata = file.metadata()?;
    let size = metadata.len();

    // Stream SHA-1 and SHA-512
    let mut reader = std::io::BufReader::with_capacity(SCAN_STREAM_CHUNK_SIZE, file);
    let mut sha1_hasher = Sha1::new();
    let mut sha512_hasher = Sha512::new();
    let mut buf = [0u8; SCAN_STREAM_CHUNK_SIZE];

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        sha1_hasher.update(&buf[..n]);
        sha512_hasher.update(&buf[..n]);
    }

    let sha1_bytes: [u8; 20] = sha1_hasher.finalize().into();
    let sha512_bytes: [u8; 64] = sha512_hasher.finalize().into();
    let sha1 = Sha1Digest::from_bytes(sha1_bytes);
    let sha512 = Sha512Digest::from_bytes(sha512_bytes);

    // Reopen for ZIP reading
    let file2 = File::open(path)?;
    let mut archive = ZipArchive::new(file2)
        .map_err(|e| MqError::Archive(format!("failed to open ZIP {}: {e}", path.display())))?;

    let manifest =
        read_zip_entry_bytes(&mut archive, &["META-INF/MANIFEST.MF"], MAX_DESCRIPTOR_SIZE)
            .map(|b| parse_manifest_mf(&b))
            .unwrap_or_default();

    let (clean_stem, filename_stems) = extract_filename_stems(&file_name);

    let mut descriptors: Vec<LoaderDescriptor> = Vec::new();
    let mut extra_jij_paths: Vec<String> = Vec::new();
    let mut supported_loaders: Vec<ModLoaderKind> = Vec::new();

    // 1. fabric.mod.json
    if let Some((desc, paths)) =
        read_zip_entry_bytes(&mut archive, &["fabric.mod.json"], MAX_DESCRIPTOR_SIZE)
            .and_then(|b| parse_fabric_mod_json(&b))
    {
        push_unique_loader(&mut supported_loaders, desc.loader);
        descriptors.push(desc);
        extra_jij_paths.extend(paths);
    }

    // 2. quilt.mod.json
    if let Some((desc, paths)) =
        read_zip_entry_bytes(&mut archive, &["quilt.mod.json"], MAX_DESCRIPTOR_SIZE)
            .and_then(|b| parse_quilt_mod_json(&b))
    {
        push_unique_loader(&mut supported_loaders, desc.loader);
        descriptors.push(desc);
        extra_jij_paths.extend(paths);
    }

    // 3. META-INF/neoforge.mods.toml
    if let Some(desc) = read_zip_entry_bytes(
        &mut archive,
        &["META-INF/neoforge.mods.toml"],
        MAX_DESCRIPTOR_SIZE,
    )
    .and_then(|b| parse_mods_toml(&b, ModLoaderKind::NeoForge, &manifest))
    {
        push_unique_loader(&mut supported_loaders, desc.loader);
        descriptors.push(desc);
    }

    // 4. META-INF/mods.toml
    if let Some(desc) =
        read_zip_entry_bytes(&mut archive, &["META-INF/mods.toml"], MAX_DESCRIPTOR_SIZE)
            .and_then(|b| parse_mods_toml(&b, ModLoaderKind::Forge, &manifest))
    {
        push_unique_loader(&mut supported_loaders, desc.loader);
        descriptors.push(desc);
    }

    // 5. mcmod.info (Forge 1.7.10 - 1.12.2)
    let mut saw_mcmod = false;
    if let Some(bytes) = read_zip_entry_bytes(
        &mut archive,
        &["mcmod.info", "META-INF/mcmod.info"],
        MAX_DESCRIPTOR_SIZE,
    ) {
        let (desc_opt, valid_file) = parse_mcmod_info(&bytes, &manifest);
        saw_mcmod = valid_file;
        if let Some(desc) = desc_opt {
            push_unique_loader(&mut supported_loaders, desc.loader);
            descriptors.push(desc);
        } else if valid_file {
            push_unique_loader(&mut supported_loaders, ModLoaderKind::Forge);
        }
    }

    // 6. META-INF/MANIFEST.MF Forge coremod detection
    if manifest.is_explicit_forge_coremod()
        || (descriptors.is_empty() && !saw_mcmod && manifest.is_forge_loader_candidate())
    {
        push_unique_loader(&mut supported_loaders, ModLoaderKind::Forge);
    }
    if manifest.is_library_mod_type() && supported_loaders.contains(&ModLoaderKind::Forge) {
        push_unique_loader(&mut supported_loaders, ModLoaderKind::NeoForge);
    }

    // 7. Embedded Jar-in-Jar (depth <= 2)
    let mut jij_ids = Vec::new();
    collect_embedded_jij_ids(&mut archive, &extra_jij_paths, 1, &mut jij_ids);

    // Choose primary metadata for InstalledModEntry
    let (mod_id, name, version, authors, description, loader, dependencies) =
        if let Some(primary) = descriptors.first() {
            let deps: Vec<String> = primary
                .required_deps
                .iter()
                .map(|d| d.dep_id.clone())
                .collect();
            (
                primary.mod_id.clone(),
                primary.name.clone(),
                primary.version.clone(),
                primary.authors.clone(),
                primary.description.clone(),
                Some(primary.loader),
                deps,
            )
        } else {
            let fallback_id = clean_stem.to_ascii_lowercase();
            let fallback_name = manifest
                .fallback_name()
                .map(str::to_string)
                .unwrap_or_else(|| clean_stem.clone());
            let fallback_version = manifest.fallback_version().unwrap_or("1.0.0").to_string();
            let fallback_loader = supported_loaders.first().copied();
            (
                fallback_id,
                fallback_name,
                fallback_version,
                Vec::new(),
                None,
                fallback_loader,
                Vec::new(),
            )
        };

    let manifest_aliases = manifest.aliases();

    Ok(AnalyzedJar {
        entry: InstalledModEntry {
            file_name,
            path: path.to_path_buf(),
            enabled,
            mod_id,
            name,
            version,
            authors,
            description,
            loader,
            dependencies,
            sha1,
            sha512,
            size,
        },
        descriptors,
        supported_loaders,
        jij_ids,
        manifest_aliases,
        filename_stems,
    })
}

fn infer_dominant_loader(jars: &[AnalyzedJar]) -> Option<ModLoaderKind> {
    let mut counts: BTreeMap<u8, usize> = BTreeMap::new();
    let loader_key = |l: ModLoaderKind| match l {
        ModLoaderKind::Vanilla => 0,
        ModLoaderKind::Fabric => 1,
        ModLoaderKind::Quilt => 2,
        ModLoaderKind::Forge => 3,
        ModLoaderKind::NeoForge => 4,
    };
    let key_to_loader = |k: u8| match k {
        1 => ModLoaderKind::Fabric,
        2 => ModLoaderKind::Quilt,
        3 => ModLoaderKind::Forge,
        4 => ModLoaderKind::NeoForge,
        _ => ModLoaderKind::Vanilla,
    };
    for j in jars {
        if j.entry.enabled && j.supported_loaders.len() == 1 {
            *counts
                .entry(loader_key(j.supported_loaders[0]))
                .or_insert(0) += 1;
        }
    }
    let mut best: Option<(u8, usize)> = None;
    let mut tied = false;
    for (k, c) in counts {
        match best {
            None => best = Some((k, c)),
            Some((_, bc)) if c > bc => {
                best = Some((k, c));
                tied = false;
            }
            Some((_, bc)) if c == bc => {
                tied = true;
            }
            _ => {}
        }
    }
    if tied {
        None
    } else {
        best.map(|(k, _)| key_to_loader(k))
    }
}

fn select_active_descriptors(
    jar: &AnalyzedJar,
    target_loader: Option<ModLoaderKind>,
) -> (Vec<&LoaderDescriptor>, bool) {
    if jar.descriptors.is_empty() {
        return (Vec::new(), false);
    }
    if jar.descriptors.len() == 1 {
        return (vec![&jar.descriptors[0]], false);
    }
    if let Some(target) = target_loader {
        let exact: Vec<&LoaderDescriptor> = jar
            .descriptors
            .iter()
            .filter(|d| d.loader == target)
            .collect();
        if !exact.is_empty() {
            return (exact, false);
        }
        let compat: Vec<&LoaderDescriptor> = jar
            .descriptors
            .iter()
            .filter(|d| is_loader_compatible(d.loader, target))
            .collect();
        if !compat.is_empty() {
            return (compat, false);
        }
    }
    // Multiple distinct loader descriptors and no known target loader:
    // require only dependencies common to all descriptors to avoid false cross-loader conflicts.
    (jar.descriptors.iter().collect(), true)
}

fn evaluate_conflicts(
    jars: &[AnalyzedJar],
    explicit_loader: Option<ModLoaderKind>,
) -> Vec<ModConflict> {
    let mut primary_id_files: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    let mut satisfied_ids: BTreeSet<String> = BTreeSet::new();
    // Maps normalized exact/separator id -> Vec<(file_name, canonical_mod_id, version)>
    let mut installed_by_id: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();

    for j in jars {
        if !j.entry.enabled {
            continue;
        }
        let m = &j.entry;

        // 1. Primary ID tracking for duplicate detection
        let primary_key = m.mod_id.to_ascii_lowercase();
        let entry = primary_id_files
            .entry(primary_key)
            .or_insert_with(|| (m.mod_id.clone(), Vec::new()));
        entry.1.push(m.file_name.clone());

        // 2. Register all satisfied IDs & aliases
        register_satisfied_id(&mut satisfied_ids, &m.mod_id);
        for d in &j.descriptors {
            register_satisfied_id(&mut satisfied_ids, &d.mod_id);
            for add in &d.additional_ids {
                register_satisfied_id(&mut satisfied_ids, add);
            }
            for prov in &d.provides {
                register_satisfied_id(&mut satisfied_ids, prov);
            }
        }
        for jij in &j.jij_ids {
            register_satisfied_id(&mut satisfied_ids, jij);
        }
        for ma in &j.manifest_aliases {
            register_satisfied_id(&mut satisfied_ids, ma);
        }
        for fs_stem in &j.filename_stems {
            register_satisfied_id(&mut satisfied_ids, fs_stem);
        }

        // 3. Register installed IDs + versions for incompatibility checks
        let mut incompat_keys: BTreeSet<String> = BTreeSet::new();
        for k in id_exact_separator_forms(&m.mod_id) {
            incompat_keys.insert(k);
        }
        for d in &j.descriptors {
            for k in id_exact_separator_forms(&d.mod_id) {
                incompat_keys.insert(k);
            }
            for add in &d.additional_ids {
                for k in id_exact_separator_forms(add) {
                    incompat_keys.insert(k);
                }
            }
            for prov in &d.provides {
                for k in id_exact_separator_forms(prov) {
                    incompat_keys.insert(k);
                }
            }
        }
        for fs_stem in &j.filename_stems {
            for k in id_exact_separator_forms(fs_stem) {
                incompat_keys.insert(k);
            }
        }
        for key in incompat_keys {
            installed_by_id.entry(key).or_default().push((
                m.file_name.clone(),
                m.mod_id.clone(),
                m.version.clone(),
            ));
        }
    }

    let effective_loader = explicit_loader.or_else(|| infer_dominant_loader(jars));

    let mut conflicts = Vec::new();
    let mut seen_dups: BTreeSet<String> = BTreeSet::new();
    let mut seen_missing: BTreeSet<(String, String)> = BTreeSet::new();
    let mut seen_incompat: BTreeSet<(String, String)> = BTreeSet::new();

    // 1. Duplicate Mod IDs
    for (key_lower, (display_id, files)) in &primary_id_files {
        if files.len() > 1 && seen_dups.insert(key_lower.clone()) {
            conflicts.push(ModConflict::DuplicateModId {
                mod_id: display_id.clone(),
                files: files.clone(),
            });
        }
    }

    // 2. Missing Dependencies & Incompatible Mods
    for j in jars {
        if !j.entry.enabled {
            continue;
        }
        let m = &j.entry;

        // Collect all IDs declared inside this very jar (submodules/provides/JiJ) to skip self-deps
        let mut self_ids: BTreeSet<String> = BTreeSet::new();
        for form in id_alias_forms(&m.mod_id) {
            self_ids.insert(form);
        }
        for d in &j.descriptors {
            for form in id_alias_forms(&d.mod_id) {
                self_ids.insert(form);
            }
            for add in &d.additional_ids {
                for form in id_alias_forms(add) {
                    self_ids.insert(form);
                }
            }
            for prov in &d.provides {
                for form in id_alias_forms(prov) {
                    self_ids.insert(form);
                }
            }
        }
        for jij in &j.jij_ids {
            for form in id_alias_forms(jij) {
                self_ids.insert(form);
            }
        }

        let (active_descs, intersect_multi) = select_active_descriptors(j, effective_loader);

        let required_deps: Vec<&ConditionalDep> = if intersect_multi && active_descs.len() > 1 {
            // Only require dependencies present across ALL descriptors of this multi-loader jar
            let first = &active_descs[0];
            first
                .required_deps
                .iter()
                .filter(|dep| {
                    let dl = dep.dep_id.to_ascii_lowercase();
                    active_descs[1..].iter().all(|other| {
                        other
                            .required_deps
                            .iter()
                            .any(|od| od.dep_id.eq_ignore_ascii_case(&dl))
                    })
                })
                .collect()
        } else {
            active_descs
                .iter()
                .flat_map(|d| d.required_deps.iter())
                .collect()
        };

        for dep in required_deps {
            if dep
                .unless
                .iter()
                .any(|u| is_dependency_satisfied(u, &satisfied_ids))
            {
                continue;
            }
            let dep_lower = dep.dep_id.to_ascii_lowercase();
            if self_ids.contains(&dep_lower) {
                continue;
            }
            if !is_dependency_satisfied(&dep.dep_id, &satisfied_ids)
                && seen_missing.insert((m.file_name.clone(), dep_lower))
            {
                conflicts.push(ModConflict::MissingDependency {
                    mod_id: m.mod_id.clone(),
                    file_name: m.file_name.clone(),
                    required_dep: dep.dep_id.clone(),
                });
            }
        }

        let incompat_rules: Vec<&IncompatRule> = if intersect_multi && active_descs.len() > 1 {
            let first = &active_descs[0];
            first
                .incompatible_mods
                .iter()
                .filter(|rule| {
                    let tl = rule.target_id.to_ascii_lowercase();
                    active_descs[1..].iter().all(|other| {
                        other
                            .incompatible_mods
                            .iter()
                            .any(|or| or.target_id.eq_ignore_ascii_case(&tl))
                    })
                })
                .collect()
        } else {
            active_descs
                .iter()
                .flat_map(|d| d.incompatible_mods.iter())
                .collect()
        };

        for rule in incompat_rules {
            let target_forms = id_exact_separator_forms(&rule.target_id);
            let mut matched_other: Option<String> = None;

            for form in &target_forms {
                if let Some(matches) = installed_by_id.get(form) {
                    for (other_file, other_id, other_ver) in matches {
                        if other_file == &m.file_name {
                            continue;
                        }
                        let matches_version = match rule.version_constraint.as_deref() {
                            None => true,
                            Some(raw_range) => {
                                let r = raw_range.trim();
                                if r.is_empty() || r == "*" || r == "(,)" || r == "[,]" {
                                    true
                                } else if parse_mc_tuple(other_ver).is_some() {
                                    satisfies_mc_version_constraint(other_ver, r)
                                } else {
                                    // Unparseable version (e.g. placeholder) + narrow range: avoid false positive
                                    false
                                }
                            }
                        };
                        if matches_version {
                            matched_other = Some(other_id.clone());
                            break;
                        }
                    }
                }
                if matched_other.is_some() {
                    break;
                }
            }

            if let Some(conflicting) = matched_other {
                let dedup_key = (m.file_name.clone(), rule.target_id.to_ascii_lowercase());
                if seen_incompat.insert(dedup_key) {
                    conflicts.push(ModConflict::IncompatibleMod {
                        mod_id: m.mod_id.clone(),
                        file_name: m.file_name.clone(),
                        conflicting_mod: conflicting,
                    });
                }
            }
        }
    }

    conflicts
}

/// Scans a single `.jar` or `.jar.disabled` file.
#[allow(dead_code)]
fn scan_single_jar(path: &Path) -> MqResult<InstalledModEntry> {
    analyze_single_jar(path).map(|a| a.entry)
}

/// Computes SHA-1, SHA-512, and CurseForge MurmurHash2 (seed 1, whitespace-stripped) fingerprint for a file.
pub fn hash_and_fingerprint_file(path: &Path) -> MqResult<Hashes> {
    let (sha1, sha512, _) = stream_hash_file_sha1_sha512(path)?;
    let fp = compute_curseforge_fingerprint_file(path)?;
    Ok(Hashes {
        sha1: Some(sha1),
        sha512: Some(sha512),
        md5: None,
        murmur2_fingerprint: Some(fp),
    })
}

/// Scans `dir` for `.jar`, `.jar.disabled`, `.zip`, `.zip.disabled` files, registering any
/// untracked files in `lockfile` with `source = LockSource::Manual` and returning all
/// `LockSource::Manual` entries with their computed [`Hashes`].
pub fn sync_directory_manual_entries(
    dir: &Path,
    kind: ProjectKind,
    lockfile: &mut ModLockFile,
) -> MqResult<Vec<(String, PathBuf, Hashes)>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut manual_candidates = Vec::new();
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let fname = entry.file_name().to_string_lossy().to_string();
        if fname.ends_with(".jar")
            || fname.ends_with(".jar.disabled")
            || fname.ends_with(".zip")
            || fname.ends_with(".zip.disabled")
        {
            paths.push((fname, p));
        }
    }
    paths.sort_by(|a, b| a.0.cmp(&b.0));

    for (fname, path) in paths {
        let hashes = hash_and_fingerprint_file(&path)?;
        let sha1_hex = hashes.sha1.map(|h| h.to_hex());
        let sha512_hex = hashes.sha512.map(|h| h.to_hex()).unwrap_or_default();

        let existing_key = lockfile
            .find_by_filename(&fname)
            .map(|(k, _)| k.clone())
            .or_else(|| {
                lockfile
                    .find_by_hash(sha1_hex.as_deref(), Some(&sha512_hex))
                    .map(|(k, _)| k.clone())
            });

        let key = if let Some(k) = existing_key {
            if let Some(entry) = lockfile.entries.get_mut(&k) {
                if entry.sha1.is_none() {
                    entry.sha1 = sha1_hex.clone();
                }
                if entry.sha512.is_empty() {
                    entry.sha512.clone_from(&sha512_hex);
                }
            }
            k
        } else {
            let stem = fname
                .trim_end_matches(".disabled")
                .trim_end_matches(".jar")
                .trim_end_matches(".zip")
                .to_owned();
            let key = if lockfile.entries.contains_key(&stem) {
                fname.clone()
            } else {
                stem.clone()
            };
            lockfile.record_install(
                key.clone(),
                ModLockEntry {
                    source: LockSource::Manual,
                    project_id: stem,
                    version_id: String::new(),
                    file_id: None,
                    filename: fname,
                    sha1: sha1_hex,
                    sha512: sha512_hex,
                    installed_as_dependency: false,
                    required_by: Vec::new(),
                    kind,
                },
            );
            key
        };

        if lockfile
            .entries
            .get(&key)
            .is_some_and(|e| e.source == LockSource::Manual)
        {
            manual_candidates.push((key, path, hashes));
        }
    }

    Ok(manual_candidates)
}

/// Identifies manually added files in `dir` against a single [`ContentProvider`]
/// (by SHA-512/SHA-1 on Modrinth or Murmur2 fingerprint on CurseForge) and upgrades
/// matching `.mq-lock.json` entries from `LockSource::Manual` to `LockSource::Modrinth` / `LockSource::CurseForge`.
pub async fn identify_manual_files_with_provider<P>(
    dir: &Path,
    kind: ProjectKind,
    lockfile: &mut ModLockFile,
    provider: &P,
) -> MqResult<usize>
where
    P: ContentProvider + ?Sized,
{
    let candidates = sync_directory_manual_entries(dir, kind, lockfile)?;
    if candidates.is_empty() {
        return Ok(0);
    }

    let mut hash_queries = Vec::new();
    let mut fp_queries = Vec::new();

    for (_, _, hashes) in &candidates {
        if let Some(s512) = hashes.sha512 {
            hash_queries.push(s512.to_hex());
        } else if let Some(s1) = hashes.sha1 {
            hash_queries.push(s1.to_hex());
        }
        if let Some(fp) = hashes.murmur2_fingerprint {
            fp_queries.push(fp);
        }
    }

    let resolved_map = provider
        .identify_by_hashes(&hash_queries, &fp_queries)
        .await?;
    let target_source = LockSource::from(provider.source());
    let mut upgraded = 0usize;

    for (key, _, hashes) in candidates {
        let sha512_hex = hashes.sha512.map(|h| h.to_hex());
        let sha1_hex = hashes.sha1.map(|h| h.to_hex());
        let fp_str = hashes.murmur2_fingerprint.map(|fp| fp.to_string());

        let matched_ver = sha512_hex
            .as_deref()
            .and_then(|h| resolved_map.get(h))
            .or_else(|| sha1_hex.as_deref().and_then(|h| resolved_map.get(h)))
            .or_else(|| fp_str.as_deref().and_then(|f| resolved_map.get(f)));

        if let Some(ver) = matched_ver {
            if let Some(entry) = lockfile.entries.get_mut(&key) {
                entry.source = target_source;
                entry.project_id.clone_from(&ver.project_id);
                entry.version_id.clone_from(&ver.id);
                entry.file_id = Some(ver.id.clone());
                entry.kind = kind;
                upgraded += 1;
            }
        }
    }

    if dir.exists() {
        let _ = lockfile.save(dir);
    }

    Ok(upgraded)
}

/// Identifies manually added files in `dir` across Modrinth (SHA-512/SHA-1) and CurseForge (Murmur2 fingerprint),
/// upgrading `LockSource::Manual` entries in `lockfile` to `LockSource::Modrinth` or `LockSource::CurseForge`.
pub async fn identify_manual_files(
    dir: &Path,
    kind: ProjectKind,
    lockfile: &mut ModLockFile,
    modrinth: Option<&ModrinthClient>,
    curseforge: Option<&CurseForgeClient>,
) -> MqResult<usize> {
    let _ = sync_directory_manual_entries(dir, kind, lockfile)?;
    let mut total = 0usize;

    if let Some(mr) = modrinth {
        total += identify_manual_files_with_provider(dir, kind, lockfile, mr).await?;
    }
    if let Some(cf) = curseforge {
        total += identify_manual_files_with_provider(dir, kind, lockfile, cf).await?;
    }

    if dir.exists() {
        let _ = lockfile.save(dir);
    }

    Ok(total)
}

/// Issue detected during preflight validation of an instance's `mods/` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreflightIssue {
    DuplicateModId {
        mod_id: String,
        files: Vec<String>,
    },
    MissingDependency {
        mod_id: String,
        file_name: String,
        required_dep: String,
    },
    IncompatibleMod {
        mod_id: String,
        file_name: String,
        conflicting_mod: String,
    },
    WrongLoader {
        mod_id: String,
        file_name: String,
        mod_loader: ModLoaderKind,
        instance_loader: ModLoaderKind,
    },
    WrongMcVersion {
        mod_id: String,
        file_name: String,
        required_mc: String,
        instance_mc: String,
    },
}

/// Result of preflight validation before launching an instance.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PreflightReport {
    pub issues: Vec<PreflightIssue>,
}

impl PreflightReport {
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}

fn is_loader_compatible(mod_loader: ModLoaderKind, instance_loader: ModLoaderKind) -> bool {
    match instance_loader {
        ModLoaderKind::Vanilla => false,
        ModLoaderKind::Quilt => matches!(mod_loader, ModLoaderKind::Fabric | ModLoaderKind::Quilt),
        other => mod_loader == other,
    }
}

fn parse_mc_tuple(s: &str) -> Option<(u32, u32, u32)> {
    let clean = s
        .trim()
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .split(['-', '+', ' ', ']', ')'])
        .next()?;
    if clean.is_empty() {
        return None;
    }
    let mut parts = clean.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(0);
    let patch = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(0);
    Some((major, minor, patch))
}

fn match_single_clause(inst: (u32, u32, u32), clause: &str) -> bool {
    let c = clause.trim();
    if c.is_empty() || c == "*" {
        return true;
    }
    if let Some(rest) = c.strip_prefix(">=") {
        return parse_mc_tuple(rest).is_none_or(|v| inst >= v);
    }
    if let Some(rest) = c.strip_prefix("<=") {
        return parse_mc_tuple(rest).is_none_or(|v| inst <= v);
    }
    if let Some(rest) = c.strip_prefix("!=") {
        return parse_mc_tuple(rest).is_none_or(|v| inst != v);
    }
    if let Some(rest) = c.strip_prefix('!') {
        return parse_mc_tuple(rest).is_none_or(|v| inst != v);
    }
    if let Some(rest) = c.strip_prefix('>') {
        return parse_mc_tuple(rest).is_none_or(|v| inst > v);
    }
    if let Some(rest) = c.strip_prefix('<') {
        return parse_mc_tuple(rest).is_none_or(|v| inst < v);
    }
    if let Some(rest) = c.strip_prefix('~') {
        return parse_mc_tuple(rest).is_none_or(|v| inst.0 == v.0 && inst.1 == v.1 && inst >= v);
    }
    if let Some(rest) = c.strip_prefix('^') {
        return parse_mc_tuple(rest).is_none_or(|v| {
            if v.0 > 0 {
                inst >= v && inst.0 == v.0
            } else if v.1 > 0 {
                inst >= v && inst.0 == 0 && inst.1 == v.1
            } else {
                inst == v
            }
        });
    }
    if let Some(rest) = c.strip_prefix('=') {
        return parse_mc_tuple(rest).is_none_or(|v| inst == v);
    }
    if c.ends_with(".x") || c.ends_with(".*") {
        let prefix = c.trim_end_matches(".x").trim_end_matches(".*");
        return parse_mc_tuple(prefix).is_none_or(|v| {
            if prefix.contains('.') {
                inst.0 == v.0 && inst.1 == v.1
            } else {
                inst.0 == v.0
            }
        });
    }
    if (c.starts_with('[') || c.starts_with('(')) && (c.ends_with(']') || c.ends_with(')')) {
        let inc_low = c.starts_with('[');
        let inc_high = c.ends_with(']');
        let inner = &c[1..c.len() - 1];
        if let Some((lo_s, hi_s)) = inner.split_once(',') {
            let lo_ok = if lo_s.trim().is_empty() {
                true
            } else {
                parse_mc_tuple(lo_s).is_none_or(|lo| if inc_low { inst >= lo } else { inst > lo })
            };
            let hi_ok = if hi_s.trim().is_empty() {
                true
            } else {
                parse_mc_tuple(hi_s).is_none_or(|hi| if inc_high { inst <= hi } else { inst < hi })
            };
            return lo_ok && hi_ok;
        } else if inc_low && inc_high {
            // Maven single exact version `[1.20.1]`
            return parse_mc_tuple(inner).is_none_or(|v| inst == v);
        }
    }
    parse_mc_tuple(c).is_none_or(|v| inst == v)
}

fn split_maven_or_ranges(s: &str) -> Vec<&str> {
    if !s.contains('[') && !s.contains('(') {
        return vec![s];
    }
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut last = 0usize;
    for (idx, ch) in s.char_indices() {
        match ch {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&s[last..idx]);
                last = idx + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[last..]);
    parts
}

/// Checks whether `instance_mc` satisfies a Fabric/Quilt/Forge/NeoForge `minecraft` version constraint.
pub fn satisfies_mc_version_constraint(instance_mc: &str, constraint: &str) -> bool {
    let Some(inst) = parse_mc_tuple(instance_mc) else {
        return true;
    };
    let trimmed = constraint.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return true;
    }
    trimmed.split("||").any(|or_part| {
        split_maven_or_ranges(or_part).into_iter().any(|sub_or| {
            let part = sub_or.trim();
            if part.is_empty() {
                return false;
            }
            if (part.starts_with('[') || part.starts_with('('))
                && (part.ends_with(']') || part.ends_with(')'))
            {
                match_single_clause(inst, part)
            } else {
                part.split_whitespace()
                    .all(|clause| match_single_clause(inst, clause))
            }
        })
    })
}

/// Runs preflight validation on `<mods_dir>` for a specific instance loader and Minecraft version.
pub async fn preflight_check_mods(
    mods_dir: &Path,
    instance_loader: ModLoaderKind,
    instance_mc_version: &str,
    compute_pool: &ComputePool,
    epoch_guard: &EpochGuard,
) -> MqResult<PreflightReport> {
    let analysis =
        analyze_mods_directory(mods_dir, Some(instance_loader), compute_pool, epoch_guard).await?;
    let mut issues = Vec::new();
    let mut seen_keys: BTreeSet<(u8, String, String)> = BTreeSet::new();

    for c in analysis.report.conflicts {
        match c {
            ModConflict::DuplicateModId { mod_id, files } => {
                if seen_keys.insert((0, mod_id.to_ascii_lowercase(), String::new())) {
                    issues.push(PreflightIssue::DuplicateModId { mod_id, files });
                }
            }
            ModConflict::MissingDependency {
                mod_id,
                file_name,
                required_dep,
            } => {
                if seen_keys.insert((1, file_name.clone(), required_dep.to_ascii_lowercase())) {
                    issues.push(PreflightIssue::MissingDependency {
                        mod_id,
                        file_name,
                        required_dep,
                    });
                }
            }
            ModConflict::IncompatibleMod {
                mod_id,
                file_name,
                conflicting_mod,
            } => {
                if seen_keys.insert((2, file_name.clone(), conflicting_mod.to_ascii_lowercase())) {
                    issues.push(PreflightIssue::IncompatibleMod {
                        mod_id,
                        file_name,
                        conflicting_mod,
                    });
                }
            }
        }
    }

    for j in &analysis.jars {
        if !j.entry.enabled {
            continue;
        }
        let m = &j.entry;

        // Multi-loader check: only report WrongLoader when NO supported loader in the jar
        // is compatible with the instance's loader.
        if !j.supported_loaders.is_empty()
            && !j
                .supported_loaders
                .iter()
                .any(|&ld| is_loader_compatible(ld, instance_loader))
        {
            let mod_loader = m.loader.unwrap_or(j.supported_loaders[0]);
            if seen_keys.insert((3, m.file_name.clone(), String::new())) {
                issues.push(PreflightIssue::WrongLoader {
                    mod_id: m.mod_id.clone(),
                    file_name: m.file_name.clone(),
                    mod_loader,
                    instance_loader,
                });
            }
        }

        let (active_descs, _) = select_active_descriptors(j, Some(instance_loader));
        let constraint = active_descs
            .iter()
            .find_map(|d| d.mc_constraint.as_deref())
            .or_else(|| {
                j.descriptors
                    .iter()
                    .find_map(|d| d.mc_constraint.as_deref())
            });

        if let Some(c) = constraint
            .filter(|c| !satisfies_mc_version_constraint(instance_mc_version, c))
            .filter(|_| seen_keys.insert((4, m.file_name.clone(), String::new())))
        {
            issues.push(PreflightIssue::WrongMcVersion {
                mod_id: m.mod_id.clone(),
                file_name: m.file_name.clone(),
                required_mc: c.to_string(),
                instance_mc: instance_mc_version.to_string(),
            });
        }
    }

    Ok(PreflightReport { issues })
}

/// Automatically fixes fixable preflight issues in `<mods_dir>` by disabling duplicate,
/// wrong-loader, and wrong-MC-version mods (`.jar` -> `.jar.disabled`).
pub fn auto_fix_preflight(mods_dir: &Path, report: &PreflightReport) -> MqResult<Vec<String>> {
    let mut actions = Vec::new();
    let mut disabled = BTreeSet::new();

    for issue in &report.issues {
        match issue {
            PreflightIssue::DuplicateModId { mod_id, files } => {
                for dup in files.iter().skip(1) {
                    if disabled.insert(dup.clone()) && mods_dir.join(dup).exists() {
                        crate::manager::toggle_mod(mods_dir, dup)?;
                        actions.push(format!("Отключён дубликат '{mod_id}': {dup}"));
                    }
                }
            }
            PreflightIssue::WrongLoader {
                file_name,
                mod_loader,
                instance_loader,
                ..
            } => {
                if disabled.insert(file_name.clone()) && mods_dir.join(file_name).exists() {
                    crate::manager::toggle_mod(mods_dir, file_name)?;
                    actions.push(format!(
                        "Отключён мод {file_name} (загрузчик {}, нужен {})",
                        mod_loader.as_str(),
                        instance_loader.as_str()
                    ));
                }
            }
            PreflightIssue::WrongMcVersion {
                file_name,
                required_mc,
                instance_mc,
                ..
            } => {
                if disabled.insert(file_name.clone()) && mods_dir.join(file_name).exists() {
                    crate::manager::toggle_mod(mods_dir, file_name)?;
                    actions.push(format!(
                        "Отключён мод {file_name} (требует MC {required_mc}, инстанс {instance_mc})"
                    ));
                }
            }
            PreflightIssue::MissingDependency { .. } | PreflightIssue::IncompatibleMod { .. } => {}
        }
    }

    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use zip::write::SimpleFileOptions;

    #[tokio::test]
    async fn scans_mods_detects_metadata_and_missing_dependencies() {
        let dir = tempdir().unwrap();
        let sodium_jar = dir.path().join("sodium-0.5.8.jar");
        let extra_jar = dir.path().join("sodium-duplicate.jar");

        // Create sodium jar with fabric.mod.json that depends on fabric-api
        {
            let file = File::create(&sodium_jar).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opt = SimpleFileOptions::default();
            zip.start_file("fabric.mod.json", opt).unwrap();
            zip.write_all(
                br#"{
                "id": "sodium",
                "name": "Sodium",
                "version": "0.5.8",
                "depends": {
                    "minecraft": ">=1.20.1",
                    "fabric-api": "*"
                }
            }"#,
            )
            .unwrap();
            zip.finish().unwrap();
        }

        // Create duplicate sodium jar
        {
            let file = File::create(&extra_jar).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opt = SimpleFileOptions::default();
            zip.start_file("fabric.mod.json", opt).unwrap();
            zip.write_all(
                br#"{
                "id": "sodium",
                "name": "Sodium Duplicate",
                "version": "0.5.9"
            }"#,
            )
            .unwrap();
            zip.finish().unwrap();
        }

        let pool = ComputePool::new(2);
        let epoch = mq_sched::CancelEpoch::new();
        let guard = epoch.snapshot();

        let report = scan_mods_directory(dir.path(), &pool, &guard)
            .await
            .unwrap();
        assert_eq!(report.mods.len(), 2);
        assert!(!report.conflicts.is_empty());

        let has_dup = report
            .conflicts
            .iter()
            .any(|c| matches!(c, ModConflict::DuplicateModId { mod_id, .. } if mod_id == "sodium"));
        let has_missing_dep = report.conflicts.iter().any(|c| matches!(c, ModConflict::MissingDependency { required_dep, .. } if required_dep == "fabric-api"));

        assert!(has_dup, "Should detect duplicate mod ID");
        assert!(
            has_missing_dep,
            "Should detect missing fabric-api dependency"
        );
    }

    #[tokio::test]
    async fn manual_files_identified_by_sha512_or_murmur2_and_bound_to_lockfile() {
        let dir = tempdir().unwrap();
        let mr_jar = dir.path().join("manual-sodium.jar");
        let cf_jar = dir.path().join("manual-jei.jar");
        fs::write(&mr_jar, b"modrinth-sodium-payload").unwrap();
        fs::write(&cf_jar, b"curseforge-jei-payload").unwrap();

        let mr_hashes = hash_and_fingerprint_file(&mr_jar).unwrap();
        let cf_hashes = hash_and_fingerprint_file(&cf_jar).unwrap();
        let mr_sha512 = mr_hashes.sha512.unwrap().to_hex();
        let cf_fp = cf_hashes.murmur2_fingerprint.unwrap();

        let mut lock = ModLockFile::default();
        let candidates = sync_directory_manual_entries(dir.path(), ProjectKind::Mod, &mut lock)
            .expect("sync manual entries");
        assert_eq!(candidates.len(), 2);
        assert_eq!(
            lock.entries.get("manual-sodium").unwrap().source,
            LockSource::Manual
        );
        assert_eq!(
            lock.entries.get("manual-jei").unwrap().source,
            LockSource::Manual
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mr_sha512_clone = mr_sha512.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let sha512_key = mr_sha512_clone.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let first_line = req.lines().next().unwrap_or("");
                    let body = if first_line.contains("/modrinth/v2/version_files") {
                        json!({
                            sha512_key.clone(): {
                                "id": "ver-mr-sodium",
                                "project_id": "AANobbMI",
                                "name": "Sodium 0.6.0",
                                "version_number": "0.6.0",
                                "game_versions": ["26.1"],
                                "loaders": ["fabric"],
                                "files": [{
                                    "hashes": { "sha512": sha512_key },
                                    "url": "https://cdn.modrinth.com/sodium.jar",
                                    "filename": "manual-sodium.jar",
                                    "primary": true,
                                    "size": 23
                                }],
                                "dependencies": []
                            }
                        })
                        .to_string()
                    } else if first_line.contains("/curseforge/v1/fingerprints/432") {
                        json!({
                            "data": {
                                "exactMatches": [{
                                    "id": 238222,
                                    "file": {
                                        "id": 9001,
                                        "modId": 238222,
                                        "fileName": "manual-jei.jar",
                                        "fileLength": 22,
                                        "downloadUrl": "https://edge.forgecdn.net/files/9/1/manual-jei.jar",
                                        "fileFingerprint": cf_fp,
                                        "hashes": [],
                                        "gameVersions": ["26.1", "Fabric"],
                                        "dependencies": []
                                    },
                                    "latestFiles": []
                                }],
                                "exactFingerprints": [cf_fp]
                            }
                        })
                        .to_string()
                    } else {
                        "{}".to_owned()
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });

        let mr_client =
            ModrinthClient::with_base_url(format!("http://{addr}/modrinth/v2")).unwrap();
        let cf_client = CurseForgeClient::with_base_url(
            format!("http://{addr}/curseforge/v1"),
            Some("test-key".into()),
        )
        .unwrap();

        let upgraded = identify_manual_files(
            dir.path(),
            ProjectKind::Mod,
            &mut lock,
            Some(&mr_client),
            Some(&cf_client),
        )
        .await
        .expect("identify manual files");
        assert_eq!(upgraded, 2);

        let sodium_entry = lock.entries.get("manual-sodium").unwrap();
        assert_eq!(sodium_entry.source, LockSource::Modrinth);
        assert_eq!(sodium_entry.project_id, "AANobbMI");
        assert_eq!(sodium_entry.version_id, "ver-mr-sodium");

        let jei_entry = lock.entries.get("manual-jei").unwrap();
        assert_eq!(jei_entry.source, LockSource::CurseForge);
        assert_eq!(jei_entry.project_id, "238222");
        assert_eq!(jei_entry.version_id, "9001");
    }

    #[tokio::test]
    async fn preflight_detects_broken_sets_and_auto_fixes() {
        let dir = tempdir().unwrap();

        let write_fabric_jar = |path: &Path, id: &str, mc_dep: &str, dep_id: Option<&str>| {
            let f = File::create(path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            zw.start_file("fabric.mod.json", SimpleFileOptions::default())
                .unwrap();
            let mut depends = serde_json::Map::new();
            depends.insert("minecraft".into(), json!(mc_dep));
            if let Some(d) = dep_id {
                depends.insert(d.into(), json!("*"));
            }
            let manifest = json!({
                "schemaVersion": 1,
                "id": id,
                "version": "1.0.0",
                "name": id,
                "depends": depends
            });
            zw.write_all(manifest.to_string().as_bytes()).unwrap();
            zw.finish().unwrap();
        };

        // 1. Valid Fabric mod on Quilt (compatible!) with missing dependency
        write_fabric_jar(
            &dir.path().join("sodium.jar"),
            "sodium",
            ">=1.21",
            Some("fabric-api"),
        );
        // 2. Duplicate mod ID
        write_fabric_jar(&dir.path().join("sodium-dup.jar"), "sodium", ">=1.21", None);
        // 3. Wrong MC version
        write_fabric_jar(&dir.path().join("oldmod.jar"), "oldmod", "1.19.4", None);
        // 4. Wrong loader (Forge mod in Quilt instance)
        {
            let f = File::create(dir.path().join("forgemod.jar")).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            zw.start_file("META-INF/mods.toml", SimpleFileOptions::default())
                .unwrap();
            zw.write_all(b"modLoader=\"javafml\"\n[[mods]]\nmodId=\"forgemod\"\nversion=\"1.0\"\n")
                .unwrap();
            zw.finish().unwrap();
        }

        let pool = ComputePool::new(2);
        let epoch = mq_sched::CancelEpoch::new();
        let guard = epoch.snapshot();

        let report =
            preflight_check_mods(dir.path(), ModLoaderKind::Quilt, "1.21.1", &pool, &guard)
                .await
                .unwrap();

        assert!(report
            .issues
            .iter()
            .any(|i| matches!(i, PreflightIssue::MissingDependency { required_dep, .. } if required_dep == "fabric-api")));
        assert!(report.issues.iter().any(
            |i| matches!(i, PreflightIssue::DuplicateModId { mod_id, .. } if mod_id == "sodium")
        ));
        assert!(report.issues.iter().any(
            |i| matches!(i, PreflightIssue::WrongMcVersion { mod_id, .. } if mod_id == "oldmod")
        ));
        assert!(report.issues.iter().any(
            |i| matches!(i, PreflightIssue::WrongLoader { mod_id, .. } if mod_id == "forgemod")
        ));

        let fixes = auto_fix_preflight(dir.path(), &report).unwrap();
        assert_eq!(fixes.len(), 3);
        assert!(
            dir.path().join("sodium.jar").exists() ^ dir.path().join("sodium-dup.jar").exists()
        );
        assert!(
            dir.path().join("sodium.jar.disabled").exists()
                ^ dir.path().join("sodium-dup.jar.disabled").exists()
        );
        assert!(dir.path().join("oldmod.jar.disabled").exists());
        assert!(dir.path().join("forgemod.jar.disabled").exists());
    }

    #[tokio::test]
    async fn fabric_api_satisfies_fabric_submodule_dependencies() {
        let dir = tempdir().unwrap();

        let write_jar = |path: &Path, id: &str, depends: Option<&str>| {
            let f = File::create(path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            zw.start_file("fabric.mod.json", SimpleFileOptions::default())
                .unwrap();
            let manifest = match depends {
                Some(dep) => json!({
                    "id": id,
                    "version": "1.0.0",
                    "depends": { dep: "*" }
                }),
                None => json!({
                    "id": id,
                    "version": "1.0.0"
                }),
            };
            zw.write_all(manifest.to_string().as_bytes()).unwrap();
            zw.finish().unwrap();
        };

        write_jar(&dir.path().join("fabric-api.jar"), "fabric-api", None);
        write_jar(
            &dir.path().join("iris.jar"),
            "iris",
            Some("fabric-api-base"),
        );

        let pool = ComputePool::new(2);
        let epoch = mq_sched::CancelEpoch::new();
        let guard = epoch.snapshot();

        let report = scan_mods_directory(dir.path(), &pool, &guard)
            .await
            .unwrap();
        assert!(
            report.conflicts.is_empty(),
            "fabric-api should satisfy fabric-* submodules, got: {:?}",
            report.conflicts
        );
    }

    fn write_zip_entries(path: &Path, entries: &[(&str, &[u8])]) {
        let f = File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opt = SimpleFileOptions::default();
        for (name, content) in entries {
            zw.start_file(*name, opt).unwrap();
            zw.write_all(content).unwrap();
        }
        zw.finish().unwrap();
    }

    fn build_zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opt = SimpleFileOptions::default();
            for (name, content) in entries {
                zw.start_file(*name, opt).unwrap();
                zw.write_all(content).unwrap();
            }
            zw.finish().unwrap();
        }
        buf
    }

    #[tokio::test]
    async fn forge_1122_mcmod_info_and_filename_stem_satisfy_creativecore() {
        let dir = tempdir().unwrap();

        // EnhancedVisuals with BOM, comments, trailing commas, raw newline in description,
        // `creativecore@[1.9,)` in requiredMods, and `after:optifine` in dependencies.
        let ev_mcmod = "\u{FEFF}[\n  {\n    // EnhancedVisuals metadata\n    \"modid\": \"enhancedvisuals\",\n    \"name\": \"EnhancedVisuals\",\n    \"description\": \"Visual effects\nwith raw newline\",\n    \"version\": \"1.4.4\",\n    \"requiredMods\": [\"creativecore@[1.9.0,)\",],\n    \"dependencies\": [\"after:optifine\", \"required-after:forge@[14.23,)\",],\n  },\n]";
        write_zip_entries(
            &dir.path().join("EnhancedVisuals_v1.4.4_mc1.12.2.jar"),
            &[("mcmod.info", ev_mcmod.as_bytes())],
        );

        // CreativeCore WITHOUT mcmod.info (pure @Mod bytecode / MANIFEST.MF coremod style)
        // must still register alias `creativecore` from `CreativeCore_v1.10.70_mc1.12.2.jar`!
        let cc_manifest = b"Manifest-Version: 1.0\nFMLCorePlugin: com.creativemd.creativecore.core.CreativeCoreLoader\nFMLCorePluginContainsFMLMod: true\n";
        write_zip_entries(
            &dir.path().join("CreativeCore_v1.10.70_mc1.12.2.jar"),
            &[("META-INF/MANIFEST.MF", cc_manifest)],
        );

        let pool = ComputePool::new(2);
        let epoch = mq_sched::CancelEpoch::new();
        let guard = epoch.snapshot();

        let report = scan_mods_directory(dir.path(), &pool, &guard)
            .await
            .unwrap();
        assert!(
            report.conflicts.is_empty(),
            "Expected zero conflicts for EnhancedVisuals + CreativeCore_v1.10.70_mc1.12.2.jar, got: {:?}",
            report.conflicts
        );

        let preflight =
            preflight_check_mods(dir.path(), ModLoaderKind::Forge, "1.12.2", &pool, &guard)
                .await
                .unwrap();
        assert!(
            preflight.is_clean(),
            "Expected clean preflight on Forge 1.12.2, got: {:?}",
            preflight.issues
        );
    }

    #[tokio::test]
    async fn forge_mcmod_v2_modlist_and_use_dependency_information_false() {
        let dir = tempdir().unwrap();

        let mod_a = br#"{
            "modListVersion": 2,
            "modList": [
                {
                    "modid": "llibrary_user",
                    "name": "LLibrary User",
                    "version": "1.2.0",
                    "useDependencyInformation": true,
                    "dependencies": [
                        "required-after:llibrary@[1.7.9,)",
                        "before:jei"
                    ]
                }
            ]
        }"#;
        let mod_b = br#"{
            "modListVersion": 2,
            "modList": [
                {
                    "modid": "disabled_deps_mod",
                    "name": "Disabled Deps Mod",
                    "version": "1.0.0",
                    "useDependencyInformation": false,
                    "requiredMods": ["non_existent_dep"]
                }
            ]
        }"#;
        let llibrary = br#"[{"modid": "llibrary", "version": "1.7.20"}]"#;

        write_zip_entries(&dir.path().join("mod_a.jar"), &[("mcmod.info", mod_a)]);
        write_zip_entries(&dir.path().join("mod_b.jar"), &[("mcmod.info", mod_b)]);
        write_zip_entries(
            &dir.path().join("llibrary-1.7.20-1.12.2.jar"),
            &[("mcmod.info", llibrary)],
        );

        let pool = ComputePool::new(2);
        let epoch = mq_sched::CancelEpoch::new();
        let guard = epoch.snapshot();

        let report = scan_mods_directory(dir.path(), &pool, &guard)
            .await
            .unwrap();
        assert!(
            report.conflicts.is_empty(),
            "Expected no false positives with modList v2 & useDependencyInformation=false, got: {:?}",
            report.conflicts
        );
    }

    #[tokio::test]
    async fn forge_mods_toml_optional_server_side_and_neoforge_incompatible() {
        let dir = tempdir().unwrap();

        let forge_toml = br#"
modLoader="javafml"
loaderVersion="[47,)"
[[mods]]
modId="forge_test"
version="1.0.0"

[[dependencies.forge_test]]
modId="fmlcore"
mandatory=true
versionRange="[47,)"
side="BOTH"

[[dependencies.forge_test]]
modId="optional_helper"
mandatory=false
versionRange="[1.0,)"
side="BOTH"

[[dependencies.forge_test]]
modId="dedicated_server_only"
mandatory=true
versionRange="[1.0,)"
side="SERVER"
"#;
        write_zip_entries(
            &dir.path().join("forge_test.jar"),
            &[("META-INF/mods.toml", forge_toml)],
        );

        let neoforge_toml = br#"
modLoader="javafml"
loaderVersion="[4,)"
[[mods]]
modId="neo_mod"
version="2.0.0"

[[dependencies.neo_mod]]
modId="neoforge"
type="required"
versionRange="[21.1,)"
side="BOTH"

[[dependencies.neo_mod]]
modId="optional_neo"
type="optional"
versionRange="*"
side="BOTH"

[[dependencies.neo_mod]]
modId="discouraged_neo"
type="discouraged"
versionRange="*"
side="BOTH"

[[dependencies.neo_mod]]
modId="bad_conflict"
type="incompatible"
versionRange="*"
side="BOTH"
"#;
        write_zip_entries(
            &dir.path().join("neo_mod.jar"),
            &[("META-INF/neoforge.mods.toml", neoforge_toml)],
        );

        let bad_toml = br#"
modLoader="javafml"
loaderVersion="[4,)"
[[mods]]
modId="bad_conflict"
version="1.5.0"
"#;
        write_zip_entries(
            &dir.path().join("bad_conflict.jar"),
            &[("META-INF/neoforge.mods.toml", bad_toml)],
        );

        let pool = ComputePool::new(2);
        let epoch = mq_sched::CancelEpoch::new();
        let guard = epoch.snapshot();

        let report = scan_mods_directory(dir.path(), &pool, &guard)
            .await
            .unwrap();
        assert_eq!(
            report.conflicts,
            vec![ModConflict::IncompatibleMod {
                mod_id: "neo_mod".into(),
                file_name: "neo_mod.jar".into(),
                conflicting_mod: "bad_conflict".into(),
            }],
            "Should only report IncompatibleMod for bad_conflict, ignoring optional/discouraged/SERVER/fmlcore"
        );
    }

    #[tokio::test]
    async fn fabric_provides_breaks_suggests_jij_and_quilt_optional_work() {
        let dir = tempdir().unwrap();

        // Embedded JiJ depth 2: outer -> META-INF/jars/mid.jar -> META-INF/jars/deep.jar
        let deep_jar = build_zip_bytes(&[(
            "fabric.mod.json",
            br#"{"id":"deep_lib","version":"1.0.0","provides":["deep_alias"]}"#,
        )]);
        let mid_jar = build_zip_bytes(&[
            ("fabric.mod.json", br#"{"id":"mid_lib","version":"1.0.0"}"#),
            ("META-INF/jars/deep.jar", &deep_jar),
            ("META-INF/jars/corrupt.jar", b"not-a-valid-zip"),
        ]);

        // Mod providing an alias and embedding JiJ + breaking old_mod <2.0.0 (installed is 2.1.0 -> OK!)
        // and unconditionally breaking optifabric (installed -> IncompatibleMod!)
        let main_fabric = json!({
            "id": "main_mod",
            "version": "1.2.0",
            "provides": ["main_legacy_alias"],
            "depends": {
                "minecraft": ">=1.21",
                "mid_lib": "*",
                "deep_alias": "*",
                "quilt_provided_alias": "*"
            },
            "recommends": { "optional_rec": "*" },
            "suggests": { "optional_sug": "*" },
            "breaks": {
                "versioned_ok": "<2.0.0",
                "optifabric": "*"
            }
        })
        .to_string();

        write_zip_entries(
            &dir.path().join("main_mod.jar"),
            &[
                ("fabric.mod.json", main_fabric.as_bytes()),
                ("META-INF/jars/mid.jar", &mid_jar),
            ],
        );

        write_zip_entries(
            &dir.path().join("versioned_ok.jar"),
            &[(
                "fabric.mod.json",
                br#"{"id":"versioned_ok","version":"2.1.0"}"#,
            )],
        );
        write_zip_entries(
            &dir.path().join("optifabric.jar"),
            &[(
                "fabric.mod.json",
                br#"{"id":"optifabric","version":"1.14.0"}"#,
            )],
        );

        let quilt_mod = json!({
            "schema_version": 1,
            "quilt_loader": {
                "id": "quilt_helper",
                "version": "1.0.0",
                "provides": [{ "id": "quilt_provided_alias" }],
                "depends": [
                    { "id": "main_legacy_alias" },
                    { "id": "missing_optional_quilt_dep", "optional": true }
                ]
            }
        })
        .to_string();
        write_zip_entries(
            &dir.path().join("quilt_helper.jar"),
            &[("quilt.mod.json", quilt_mod.as_bytes())],
        );

        let pool = ComputePool::new(2);
        let epoch = mq_sched::CancelEpoch::new();
        let guard = epoch.snapshot();

        let report = scan_mods_directory(dir.path(), &pool, &guard)
            .await
            .unwrap();
        assert_eq!(
            report.conflicts,
            vec![ModConflict::IncompatibleMod {
                mod_id: "main_mod".into(),
                file_name: "main_mod.jar".into(),
                conflicting_mod: "optifabric".into(),
            }],
            "Expected only optifabric IncompatibleMod; got: {:?}",
            report.conflicts
        );
    }

    #[tokio::test]
    async fn multi_loader_jar_and_coremod_manifest_do_not_trigger_wrong_loader() {
        let dir = tempdir().unwrap();

        let fabric_json = br#"{
            "id": "architectury",
            "version": "9.2.14",
            "depends": { "minecraft": "1.20.1", "fabric-api": "*" }
        }"#;
        let forge_toml = br#"
modLoader="javafml"
loaderVersion="[47,)"
[[mods]]
modId="architectury"
version="9.2.14"
[[dependencies.architectury]]
modId="forge"
mandatory=true
versionRange="[47,)"
side="BOTH"
[[dependencies.architectury]]
modId="minecraft"
mandatory=true
versionRange="[1.20.1,1.21)"
side="BOTH"
"#;
        let neoforge_toml = br#"
modLoader="javafml"
loaderVersion="[2,)"
[[mods]]
modId="architectury"
version="9.2.14"
[[dependencies.architectury]]
modId="neoforge"
type="required"
versionRange="[20.4,)"
side="BOTH"
[[dependencies.architectury]]
modId="minecraft"
type="required"
versionRange="[1.20.4,1.21)"
side="BOTH"
"#;
        write_zip_entries(
            &dir.path().join("architectury-universal.jar"),
            &[
                ("fabric.mod.json", fabric_json),
                ("META-INF/mods.toml", forge_toml),
                ("META-INF/neoforge.mods.toml", neoforge_toml),
            ],
        );

        let coremod_manifest = b"Manifest-Version: 1.0\nTweakClass: optifine.OptiFineForgeTweaker\nImplementation-Title: OptiFine\nImplementation-Version: 1.20.1\n";
        write_zip_entries(
            &dir.path().join("OptiFine_1.20.1_HD_U_I6.jar"),
            &[("META-INF/MANIFEST.MF", coremod_manifest)],
        );

        let pool = ComputePool::new(2);
        let epoch = mq_sched::CancelEpoch::new();
        let guard = epoch.snapshot();

        let forge_report =
            preflight_check_mods(dir.path(), ModLoaderKind::Forge, "1.20.1", &pool, &guard)
                .await
                .unwrap();
        assert!(
            forge_report.is_clean(),
            "Universal multi-loader jar + Forge coremod should be clean on Forge 1.20.1, got: {:?}",
            forge_report.issues
        );

        let neoforge_report =
            preflight_check_mods(dir.path(), ModLoaderKind::NeoForge, "1.20.4", &pool, &guard)
                .await
                .unwrap();
        assert!(
            !neoforge_report
                .issues
                .iter()
                .any(|i| matches!(i, PreflightIssue::WrongLoader { mod_id, .. } if mod_id == "architectury")),
            "Multi-loader jar with neoforge.mods.toml should not trigger WrongLoader on NeoForge"
        );
        assert!(
            !neoforge_report
                .issues
                .iter()
                .any(|i| matches!(i, PreflightIssue::WrongMcVersion { mod_id, .. } if mod_id == "architectury")),
            "Multi-loader jar should use neoforge.mods.toml MC range on NeoForge"
        );
    }

    #[test]
    fn maven_union_and_semver_mc_constraints_work() {
        assert!(satisfies_mc_version_constraint(
            "1.20.4",
            "[1.20.1,1.20.2],[1.20.4,1.21)"
        ));
        assert!(!satisfies_mc_version_constraint(
            "1.20.3",
            "[1.20.1,1.20.2],[1.20.4,1.21)"
        ));
        assert!(satisfies_mc_version_constraint("1.20.1", "[1.20.1]"));
        assert!(!satisfies_mc_version_constraint("1.20.2", "[1.20.1]"));
        assert!(satisfies_mc_version_constraint("1.21.4", "1.x"));
        assert!(!satisfies_mc_version_constraint("2.0.0", "1.x"));
        assert!(satisfies_mc_version_constraint("1.20.6", "^1.20.1"));
        assert!(!satisfies_mc_version_constraint("2.0.0", "^1.20.1"));
    }
}
