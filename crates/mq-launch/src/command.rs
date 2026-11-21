//! Deterministic Minecraft + Modloader launch command builder (Lesson E8),
//! Ely.by `-javaagent` first-flag injection, Offline UUID/token substitution,
//! wrapper command prefixing (`gamemoderun`, `mangohud`, `prime-run`), and `--dry-run` formatting.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mq_auth::{elyby_jvm_injection_args, AccountKind, AccountProfile};
use mq_core::fs_util::strip_windows_verbatim_prefix;
use mq_core::{redact_text, resolve_safe_path, MqError, MqPaths, MqResult};
use mq_instances::InstanceManifest;
use mq_meta::{build_deterministic_classpath, format_classpath_string, ResolvedVersion};
use serde::{Deserialize, Serialize};

/// Conservative command-line character threshold below Windows's 32,767-char `CreateProcessW`
/// limit at which JVM classpath/module arguments are compacted or offloaded to a Java `@argfile`.
pub const WINDOWS_ARGFILE_THRESHOLD_CHARS: usize = 24_000;

/// Canonical launcher brand string injected into `${launcher_name}`.
pub const LAUNCHER_NAME: &str = "mq";

/// Canonical launcher version string injected into `${launcher_version}`.
pub const LAUNCHER_VERSION: &str = "1.1.0";

/// Complete input context required to construct a deterministic Minecraft launch command.
#[derive(Debug, Clone)]
pub struct LaunchContext {
    pub paths: MqPaths,
    pub instance: InstanceManifest,
    pub version: ResolvedVersion,
    pub account: AccountProfile,
    pub java_bin: PathBuf,
    pub authlib_injector_jar: Option<PathBuf>,
    pub extra_secrets_to_redact: Vec<String>,
}

impl LaunchContext {
    /// Collects all sensitive tokens (access token, refresh token, client token, and extra secrets)
    /// that must be scrubbed from command-line logs and game `stdout`/`stderr` streams.
    pub fn collect_secrets(&self) -> Vec<String> {
        let mut secrets = Vec::new();
        let access = self.account.access_token.expose_secret();
        if !access.is_empty() && access != "0" {
            secrets.push(access.to_owned());
        }
        if let Some(ref refresh) = self.account.refresh_token {
            let s = refresh.expose_secret();
            if !s.is_empty() {
                secrets.push(s.to_owned());
            }
        }
        if let Some(ref client_tok) = self.account.client_token {
            let s = client_tok.expose_secret();
            if !s.is_empty() {
                secrets.push(s.to_owned());
            }
        }
        for extra in &self.extra_secrets_to_redact {
            if !extra.is_empty() {
                secrets.push(extra.clone());
            }
        }
        secrets
    }
}

/// Deterministically constructed child process command ready for `--dry-run` output or execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltLaunchCommand {
    /// Executable binary (`java_bin`, or wrapper binary such as `"gamemoderun"` if configured).
    pub program: PathBuf,
    /// Exact ordered process arguments.
    pub args: Vec<String>,
    /// Working directory (`<data_dir>/instances/<id>`).
    pub working_dir: PathBuf,
    /// Deterministic environment variable overrides.
    pub env: BTreeMap<String, String>,
}

impl BuiltLaunchCommand {
    /// Renders a deterministic single-line shell command representation (quoting arguments
    /// containing whitespace or quotes), suitable for `mq launch --dry-run`.
    pub fn to_shell_command_line(&self) -> String {
        let mut tokens = Vec::with_capacity(1 + self.args.len());
        tokens.push(shell_quote(&self.program.to_string_lossy()));
        for arg in &self.args {
            tokens.push(shell_quote(arg));
        }
        tokens.join(" ")
    }

    /// Renders the single-line shell command with all sensitive tokens replaced by `"[REDACTED]"`.
    pub fn to_redacted_command_line(&self, secrets: &[&str]) -> String {
        redact_text(&self.to_shell_command_line(), secrets)
    }
}

/// Builds a strictly deterministic [`BuiltLaunchCommand`] from [`LaunchContext`] (Lesson E8).
///
/// Guarantees:
/// - When `ctx.account.kind == AccountKind::ElyBy`, the VERY FIRST JVM flag (index 0 of JVM
///   arguments, before `-Xms`, `-Xmx`, `-D...`, `-cp`, and `mainClass`) is:
///   `-javaagent:/full/path/to/authlib-injector.jar=https://account.ely.by/api/authlib-injector`
///   immediately followed by `-Dauthlibinjector.side=client`.
/// - When `ctx.account.kind == AccountKind::Offline`, `${auth_access_token}` is ALWAYS `"0"`,
///   `${user_type}` is `"legacy"`, `${user_properties}` is `"{}"`, and `${auth_uuid}` is the
///   undashed 32-hex Offline UUID v3.
/// - Supports custom RAM (`-Xms<min>M`, `-Xmx<max>M`), user `instance.jvm_args`, and optional
///   wrapper commands (`gamemoderun`, `mangohud`, `prime-run`).
pub fn build_launch_command(ctx: &LaunchContext) -> MqResult<BuiltLaunchCommand> {
    if ctx.version.main_class.trim().is_empty() {
        return Err(MqError::Metadata(
            "resolved version main_class must not be empty".to_owned(),
        ));
    }

    let instance_dir = strip_windows_verbatim_prefix(&ctx.instance.instance_dir(&ctx.paths));
    let raw_game_dir = ctx.instance.game_dir(&ctx.paths);
    let game_dir = strip_windows_verbatim_prefix(
        &raw_game_dir
            .canonicalize()
            .unwrap_or_else(|_| std::path::absolute(&raw_game_dir).unwrap_or(raw_game_dir)),
    );
    let natives_dir = strip_windows_verbatim_prefix(&ctx.instance.natives_dir(&ctx.paths));
    let libraries_dir = strip_windows_verbatim_prefix(&ctx.paths.libraries_dir());
    let raw_assets_dir = ctx.paths.assets_dir();
    let assets_dir = strip_windows_verbatim_prefix(
        &raw_assets_dir
            .canonicalize()
            .unwrap_or_else(|_| std::path::absolute(&raw_assets_dir).unwrap_or(raw_assets_dir)),
    );
    let client_jar_path = strip_windows_verbatim_prefix(&resolve_client_jar_path(
        &ctx.paths,
        &instance_dir,
        &ctx.version,
    )?);
    let clean_java_bin = strip_windows_verbatim_prefix(&ctx.java_bin);

    let classpath_entries =
        build_deterministic_classpath(&ctx.version, &libraries_dir, &client_jar_path);
    let classpath_str = format_classpath_string(&classpath_entries);

    let mut jvm_flags: Vec<String> = Vec::new();

    // 1. CRITICAL RULE FOR ELY.BY (Section 5.1.2 & DoD #6):
    // Always inject `-javaagent:<jar>=https://account.ely.by/api/authlib-injector` as the
    // VERY FIRST JVM flag before everything else, followed by `-Dauthlibinjector.side=client`.
    if ctx.account.kind == AccountKind::ElyBy {
        let injector_jar = strip_windows_verbatim_prefix(
            &ctx.authlib_injector_jar
                .clone()
                .unwrap_or_else(|| ctx.paths.authlib_injector_jar()),
        );
        let [agent_flag, side_flag] = elyby_jvm_injection_args(&injector_jar);
        jvm_flags.push(agent_flag);
        jvm_flags.push(side_flag);
    }

    // 2. Heap memory flags (-Xms / -Xmx).
    let min_mb = ctx.instance.memory_min_mb.max(256);
    let max_mb = ctx.instance.memory_max_mb.max(min_mb);
    jvm_flags.push(format!("-Xms{min_mb}M"));
    jvm_flags.push(format!("-Xmx{max_mb}M"));

    // Prepare JVM placeholders for substitutions
    let natives_str = natives_dir.to_string_lossy();
    let libs_str = libraries_dir.to_string_lossy();
    let game_dir_str = game_dir.to_string_lossy();
    let assets_dir_str = assets_dir.to_string_lossy();

    let log_path_str = if let Some(ref logging) = ctx.version.logging {
        if let Some(ref client) = logging.client {
            if !client.file.path.is_empty() {
                strip_windows_verbatim_prefix(&assets_dir.join(&client.file.path))
                    .to_string_lossy()
                    .into_owned()
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let mut cp_sep_buf = [0u8; 4];
    let cp_sep = mq_core::platform::classpath_separator().encode_utf8(&mut cp_sep_buf);

    let jvm_subs = [
        ("natives_directory", natives_str.as_ref()),
        ("launcher_name", LAUNCHER_NAME),
        ("launcher_version", LAUNCHER_VERSION),
        ("classpath", classpath_str.as_str()),
        ("library_directory", libs_str.as_ref()),
        ("classpath_separator", cp_sep),
        ("version_name", ctx.version.id.as_str()),
        ("game_directory", game_dir_str.as_ref()),
        ("assets_root", assets_dir_str.as_ref()),
        ("path", log_path_str.as_str()),
    ];

    // 3. User custom JVM arguments from `instance.toml` (sanitized against stray placeholders).
    for custom_arg in &ctx.instance.jvm_args {
        let expanded = expand_and_sanitize_arg(custom_arg, &jvm_subs);
        let trimmed = expanded.trim();
        if !trimmed.is_empty() {
            jvm_flags.push(trimmed.to_owned());
        }
    }

    // 4. Expanded version JVM arguments.
    let mut has_library_path = false;
    let mut has_classpath = false;
    let mut has_logging_config = false;

    for raw_jvm_arg in &ctx.version.jvm_arguments {
        let expanded = expand_and_sanitize_arg(raw_jvm_arg, &jvm_subs);
        if expanded.starts_with("-Djava.library.path=") {
            has_library_path = true;
        }
        if expanded == "-cp" || expanded == "-classpath" {
            has_classpath = true;
        }
        if expanded.starts_with("-Dlog4j.configurationFile=") {
            has_logging_config = true;
        }
        jvm_flags.push(expanded);
    }

    // Inject logging configuration if specified in version.json and not yet added
    if !has_logging_config {
        if let Some(ref logging) = ctx.version.logging {
            if let Some(ref client) = logging.client {
                if !client.argument.trim().is_empty() {
                    let log_flag = expand_and_sanitize_arg(&client.argument, &jvm_subs);
                    if !log_flag.is_empty() {
                        jvm_flags.push(log_flag);
                    }
                }
            }
        }
    }

    if !has_library_path {
        jvm_flags.push(format!("-Djava.library.path={natives_str}"));
    }
    if !has_classpath {
        jvm_flags.push("-cp".to_owned());
        jvm_flags.push(classpath_str);
    }

    // 4.1 Ensure UTF-8 file encoding on Windows for Cyrillic username/path safety.
    // MUST be added BEFORE `main_class` so the JVM interprets it as a system property
    // rather than forwarding it as an unknown Minecraft game argument!
    if cfg!(target_os = "windows") && !jvm_flags.iter().any(|f| f.starts_with("-Dfile.encoding=")) {
        jvm_flags.push("-Dfile.encoding=UTF-8".to_string());
    }

    // 5. Main class.
    jvm_flags.push(ctx.version.main_class.trim().to_owned());

    // 6. Expanded game arguments.
    let uuid_simple = ctx.account.uuid_simple();
    let access_token = match ctx.account.kind {
        AccountKind::Offline => "0",
        AccountKind::ElyBy | AccountKind::Microsoft => ctx.account.access_token.expose_secret(),
    };
    let user_type = ctx.account.kind.user_type_arg();
    let version_type = if ctx.version.id.contains("snapshot") || ctx.version.id.contains('w') {
        "snapshot"
    } else {
        "release"
    };
    let profile_name = ctx.instance.name.as_str();

    let game_subs = [
        ("auth_player_name", ctx.account.username.as_str()),
        ("version_name", ctx.version.id.as_str()),
        ("game_directory", game_dir_str.as_ref()),
        ("assets_root", assets_dir_str.as_ref()),
        ("game_assets", assets_dir_str.as_ref()),
        ("assets_index_name", ctx.version.asset_index.id.as_str()),
        ("auth_uuid", uuid_simple.as_str()),
        ("auth_access_token", access_token),
        ("auth_session", access_token),
        ("user_type", user_type),
        ("version_type", version_type),
        ("launcher_name", LAUNCHER_NAME),
        ("launcher_version", LAUNCHER_VERSION),
        ("user_properties", "{}"),
        ("clientid", "0"),
        ("auth_xuid", "0"),
        ("resolution_width", "854"),
        ("resolution_height", "480"),
        ("quickPlayPath", ""),
        ("quick_play_path", ""),
        ("quickPlaySingleplayer", ""),
        ("quickPlayMultiplayer", ""),
        ("quickPlayRealms", ""),
        ("path", log_path_str.as_str()),
        ("profile_name", profile_name),
    ];

    let default_game_args: Vec<String>;
    let source_game_args: &[String] = if ctx.version.game_arguments.is_empty() {
        default_game_args = default_minecraft_game_arguments();
        &default_game_args
    } else {
        &ctx.version.game_arguments
    };

    for raw_game_arg in source_game_args {
        let expanded = expand_and_sanitize_arg(raw_game_arg, &game_subs);
        jvm_flags.push(expanded);
    }

    // 7. Apply optional wrapper command prefix.
    // On Windows, ignore Linux-only wrappers (`gamemoderun`, `mangohud`, `prime-run`, etc.)
    // with a warning instead of failing `CreateProcessW` with file-not-found.
    let raw_wrapper = ctx
        .instance
        .wrapper_cmd
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let wrapper_effective = if cfg!(target_os = "windows") {
        match raw_wrapper {
            Some(wrapper) if is_linux_only_wrapper(wrapper) => {
                tracing::warn!(
                    wrapper = %wrapper,
                    "Ignoring Linux-only launch wrapper on Windows"
                );
                None
            }
            other => other,
        }
    } else {
        raw_wrapper
    };

    let (program, args) = match wrapper_effective {
        Some(wrapper_raw) => {
            let mut wrapper_tokens = wrapper_raw.split_whitespace();
            let first_bin = wrapper_tokens.next().expect("non-empty wrapper");
            let mut full_args = Vec::with_capacity(2 + jvm_flags.len());
            for extra_tok in wrapper_tokens {
                full_args.push(extra_tok.to_owned());
            }
            full_args.push(clean_java_bin.to_string_lossy().into_owned());
            full_args.extend(jvm_flags);
            (PathBuf::from(first_bin), full_args)
        }
        None => (clean_java_bin, jvm_flags),
    };

    // Strict guarantee: sanitize any remaining stray unexpanded placeholders across all arguments
    let sanitized_args = args
        .into_iter()
        .map(|arg| {
            if arg.contains("${") {
                expand_and_sanitize_arg(&arg, &[])
            } else {
                arg
            }
        })
        .collect();

    Ok(BuiltLaunchCommand {
        program,
        args: sanitized_args,
        working_dir: instance_dir,
        env: BTreeMap::new(),
    })
}

fn resolve_client_jar_path(
    paths: &MqPaths,
    instance_dir: &Path,
    version: &ResolvedVersion,
) -> MqResult<PathBuf> {
    let instance_client_jar = instance_dir.join("client.jar");
    if instance_client_jar.exists() {
        return Ok(instance_client_jar);
    }

    let default_version_jar = paths
        .versions_dir()
        .join(&version.mc_version)
        .join(format!("{}.jar", version.mc_version));
    if default_version_jar.exists() {
        return Ok(default_version_jar);
    }

    let raw_path = version.client_jar.path.trim();
    if raw_path.is_empty() {
        return Ok(default_version_jar);
    }

    let candidate = Path::new(raw_path);
    if candidate.is_absolute() {
        return Ok(candidate.to_path_buf());
    }

    if raw_path.starts_with("versions/") || raw_path.starts_with("versions\\") {
        resolve_safe_path(&paths.data_dir, raw_path)
    } else {
        resolve_safe_path(&paths.versions_dir(), raw_path)
    }
}

/// Expands known placeholders and guarantees that NO unreplaced `${...}` substrings
/// remain in the resulting argument string.
pub fn expand_and_sanitize_arg(template: &str, substitutions: &[(&str, &str)]) -> String {
    if !template.contains("${") {
        return template.to_owned();
    }

    let mut result = template.to_owned();
    for (k, v) in substitutions {
        let needle = format!("${{{k}}}");
        if result.contains(&needle) {
            result = result.replace(&needle, v);
        }
    }

    if result.contains("${") {
        let mut clean = String::with_capacity(result.len());
        let mut chars = result.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '$' && chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let mut var_name = String::new();
                let mut closed = false;
                for inner in chars.by_ref() {
                    if inner == '}' {
                        closed = true;
                        break;
                    }
                    var_name.push(inner);
                }
                if closed {
                    tracing::warn!(
                        "Unrecognized placeholder '${{{var_name}}}' in launch argument, replaced with empty string"
                    );
                } else {
                    tracing::warn!("Malformed unclosed placeholder in launch argument, scrubbed");
                }
            } else {
                clean.push(ch);
            }
        }
        result = clean;
    }

    result
}

pub fn expand_jvm_placeholder(
    template: &str,
    natives_dir: &str,
    libraries_dir: &str,
    classpath: &str,
    version_name: &str,
) -> String {
    let mut cp_sep_buf = [0u8; 4];
    let cp_sep = mq_core::platform::classpath_separator().encode_utf8(&mut cp_sep_buf);
    let subs = [
        ("natives_directory", natives_dir),
        ("launcher_name", LAUNCHER_NAME),
        ("launcher_version", LAUNCHER_VERSION),
        ("classpath", classpath),
        ("library_directory", libraries_dir),
        ("classpath_separator", cp_sep),
        ("version_name", version_name),
    ];
    expand_and_sanitize_arg(template, &subs)
}

#[allow(clippy::too_many_arguments)]
pub fn expand_game_placeholder(
    template: &str,
    player_name: &str,
    version_name: &str,
    game_dir: &str,
    assets_root: &str,
    assets_index_name: &str,
    auth_uuid: &str,
    auth_access_token: &str,
    user_type: &str,
) -> String {
    let subs = [
        ("auth_player_name", player_name),
        ("version_name", version_name),
        ("game_directory", game_dir),
        ("assets_root", assets_root),
        ("game_assets", assets_root),
        ("assets_index_name", assets_index_name),
        ("auth_uuid", auth_uuid),
        ("auth_access_token", auth_access_token),
        ("auth_session", auth_access_token),
        ("user_type", user_type),
        ("version_type", "release"),
        ("user_properties", "{}"),
        ("clientid", "0"),
        ("auth_xuid", "0"),
        ("resolution_width", "854"),
        ("resolution_height", "480"),
        ("launcher_name", LAUNCHER_NAME),
        ("launcher_version", LAUNCHER_VERSION),
        ("quickPlayPath", ""),
        ("quick_play_path", ""),
        ("quickPlaySingleplayer", ""),
        ("quickPlayMultiplayer", ""),
        ("quickPlayRealms", ""),
    ];
    expand_and_sanitize_arg(template, &subs)
}

fn default_minecraft_game_arguments() -> Vec<String> {
    [
        "--username",
        "${auth_player_name}",
        "--version",
        "${version_name}",
        "--gameDir",
        "${game_directory}",
        "--assetsDir",
        "${assets_root}",
        "--assetIndex",
        "${assets_index_name}",
        "--uuid",
        "${auth_uuid}",
        "--accessToken",
        "${auth_access_token}",
        "--userType",
        "${user_type}",
        "--versionType",
        "${version_type}",
        "--userProperties",
        "${user_properties}",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// Returns `true` when `wrapper` invokes a Linux-specific launcher wrapper or GPU environment
/// prefix (`gamemoderun`, `mangohud`, `prime-run`, `gamescope`, `DRI_PRIME=...`, etc.) that does
/// not exist on Windows.
pub fn is_linux_only_wrapper(wrapper: &str) -> bool {
    const LINUX_ONLY_BINS: &[&str] = &[
        "gamemoderun",
        "mangohud",
        "prime-run",
        "gamescope",
        "optirun",
        "primusrun",
        "obs-gamecapture",
        "vkcube",
    ];
    const LINUX_ONLY_ENV_PREFIXES: &[&str] = &[
        "DRI_PRIME=",
        "__NV_PRIME_RENDER_OFFLOAD=",
        "__GLX_VENDOR_LIBRARY_NAME=",
        "MANGOHUD=",
        "MANGOHUD_CONFIG=",
        "ENABLE_VKBASALT=",
    ];

    wrapper.split_whitespace().any(|tok| {
        if LINUX_ONLY_ENV_PREFIXES
            .iter()
            .any(|prefix| tok.starts_with(prefix))
        {
            return true;
        }
        let base = tok
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(tok)
            .to_ascii_lowercase();
        LINUX_ONLY_BINS.contains(&base.as_str())
    })
}

/// Estimates the Windows `CreateProcessW` command-line length (in UTF-16 code units / chars)
/// for `program` + `args`, including quoting and separating spaces.
pub fn estimate_windows_cmdline_len(program: &Path, args: &[String]) -> usize {
    let mut total = program.to_string_lossy().encode_utf16().count() + 2;
    for arg in args {
        // Account for space separator + potential quotes/escaping overhead
        total += 1 + arg.encode_utf16().count() + 2;
    }
    total
}

/// Formats a single JVM argument as a JEP 293 Java `@argfile` token.
///
/// Always wraps in double quotes when the argument contains whitespace, backslashes, quotes,
/// `#` (comment start), or classpath separators (`;`), escaping `\` -> `\\` and `"` -> `\"` so
/// Windows paths like `C:\Users\Name\foo.jar` are parsed verbatim by OpenJDK's `libjli`.
pub fn format_java_argfile_token(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_owned();
    }
    let needs_quoting = arg
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | '#' | ';'));
    if !needs_quoting {
        return arg.to_owned();
    }
    let mut out = String::with_capacity(arg.len() + 8);
    out.push('"');
    for ch in arg.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Compacts a classpath/module-path string by replacing entries sharing a common root with
/// `base_dir` into shorter relative paths (e.g. `..\..\libraries\...`).
///
/// Keeps an entry absolute if the relative path is not strictly shorter, uses a different drive,
/// or (when files exist on disk) does not resolve to an existing file from `base_dir`.
pub fn compact_classpath_relative_to(classpath: &str, base_dir: &Path, sep: char) -> String {
    let base_clean = strip_windows_verbatim_prefix(base_dir);
    let base_str = base_clean.to_string_lossy();
    let dir_sep = if sep == ';' { '\\' } else { '/' };

    let entries: Vec<String> = classpath
        .split(sep)
        .map(|entry| {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                return String::new();
            }
            if let Some(rel) = relative_path_between_str(&base_str, trimmed, dir_sep) {
                if rel.len() < trimmed.len() {
                    let abs_path = Path::new(trimmed);
                    if !abs_path.exists() || base_clean.join(&rel).exists() {
                        return rel;
                    }
                }
            }
            trimmed.to_owned()
        })
        .filter(|s| !s.is_empty())
        .collect();

    entries.join(&sep.to_string())
}

fn relative_path_between_str(from_dir: &str, target: &str, dir_sep: char) -> Option<String> {
    let from_norm = from_dir.strip_prefix(r"\\?\").unwrap_or(from_dir);
    let target_norm = target.strip_prefix(r"\\?\").unwrap_or(target);

    let from_parts: Vec<&str> = from_norm
        .split(['/', '\\'])
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    let target_parts: Vec<&str> = target_norm
        .split(['/', '\\'])
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();

    if from_parts.is_empty() || target_parts.is_empty() {
        return None;
    }
    if from_parts.contains(&"..") || target_parts.contains(&"..") {
        return None;
    }

    let is_windows_drive = |s: &str| s.len() == 2 && s.as_bytes()[1] == b':';
    let mut common = 0usize;

    if is_windows_drive(from_parts[0]) || is_windows_drive(target_parts[0]) {
        if !is_windows_drive(from_parts[0])
            || !is_windows_drive(target_parts[0])
            || !from_parts[0].eq_ignore_ascii_case(target_parts[0])
        {
            return None;
        }
        common = 1;
        while common < from_parts.len()
            && common < target_parts.len()
            && from_parts[common].eq_ignore_ascii_case(target_parts[common])
        {
            common += 1;
        }
        // Require at least one shared directory below the drive root (e.g. C:\MQ)
        if common < 2 {
            return None;
        }
    } else {
        while common < from_parts.len()
            && common < target_parts.len()
            && from_parts[common] == target_parts[common]
        {
            common += 1;
        }
        if common == 0 {
            return None;
        }
    }

    let up_count = from_parts.len() - common;
    let mut rel_parts: Vec<&str> = Vec::with_capacity(up_count + (target_parts.len() - common));
    rel_parts.resize(up_count, "..");
    rel_parts.extend_from_slice(&target_parts[common..]);
    if rel_parts.is_empty() {
        return None;
    }
    Some(rel_parts.join(&dir_sep.to_string()))
}

fn is_two_token_jvm_option(flag: &str) -> bool {
    matches!(
        flag,
        "-cp"
            | "-classpath"
            | "--class-path"
            | "-p"
            | "--module-path"
            | "--upgrade-module-path"
            | "--add-modules"
            | "--add-exports"
            | "--add-opens"
            | "--add-reads"
            | "--limit-modules"
            | "--patch-module"
            | "-m"
            | "--module"
    )
}

fn is_path_list_jvm_option(flag: &str) -> bool {
    matches!(
        flag,
        "-cp" | "-classpath" | "--class-path" | "-p" | "--module-path"
    )
}

fn find_jvm_args_end(args: &[String]) -> usize {
    let mut i = 0;
    while i < args.len() {
        let tok = args[i].as_str();
        if is_two_token_jvm_option(tok) {
            i = (i + 2).min(args.len());
        } else if tok.starts_with('-') || tok.starts_with('@') {
            i += 1;
        } else {
            break;
        }
    }
    i
}

fn is_java_launcher_binary(program: &Path) -> bool {
    let raw = program.to_string_lossy();
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let lower = base.to_ascii_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
    stem == "java" || stem == "javaw"
}

/// Rewrites JVM arguments when a Windows launch command approaches the 32,767-character
/// `CreateProcessW` limit (`WINDOWS_ARGFILE_THRESHOLD_CHARS`).
///
/// Strategy:
/// 1. First compacts `-cp`/`-classpath`/`-p`/`--module-path` entries relative to `working_dir`
///    (shrinks long `C:\Users\...\libraries\...` paths into short ASCII `..\..\libraries\...`).
/// 2. If still over threshold:
///    - For Java 9+ (or unknown version), offloads JVM path/option flags strictly BEFORE
///      `main_class` into `argfile_path` using JEP 293 `@argfile` syntax (never writing game
///      arguments such as `--accessToken` to disk).
///    - For Java 8 (`java_major < 9`), keeps the compacted inline arguments (Java 8 does not
///      support `@argfile`) and emits a warning if the command line still risks exceeding 32 KB.
pub fn rewrite_args_for_windows_limit(
    program: &Path,
    args: &[String],
    working_dir: &Path,
    java_major: Option<u32>,
    argfile_path: &Path,
) -> MqResult<(Vec<String>, Option<PathBuf>)> {
    if !is_java_launcher_binary(program)
        || estimate_windows_cmdline_len(program, args) <= WINDOWS_ARGFILE_THRESHOLD_CHARS
    {
        return Ok((args.to_vec(), None));
    }

    let jvm_end = find_jvm_args_end(args);
    let mut compacted = args.to_vec();

    // Step 1: compact `-cp` / `-p` entries relative to working_dir.
    let mut i = 0;
    while i + 1 < jvm_end {
        if is_path_list_jvm_option(&compacted[i]) {
            compacted[i + 1] = compact_classpath_relative_to(&compacted[i + 1], working_dir, ';');
            i += 2;
        } else if is_two_token_jvm_option(&compacted[i]) {
            i += 2;
        } else {
            i += 1;
        }
    }

    if estimate_windows_cmdline_len(program, &compacted) <= WINDOWS_ARGFILE_THRESHOLD_CHARS {
        return Ok((compacted, None));
    }

    // Step 2: Java 8 does not support `@argfile`; keep compacted args and warn if still huge.
    if java_major.is_some_and(|m| m < 9) {
        let est = estimate_windows_cmdline_len(program, &compacted);
        if est > 30_000 {
            tracing::warn!(
                estimated_chars = est,
                "Windows Java 8 command line is {est} chars (CreateProcessW limit is 32,767); \
                 Java 8 does not support @argfile — consider using a shorter MQ data directory"
            );
        }
        return Ok((compacted, None));
    }

    // Step 3: Java 9+ `@argfile` offloading (strictly JVM args before `main_class`).
    let mut kept_jvm: Vec<String> = Vec::new();
    let mut argfile_tokens: Vec<String> = Vec::new();

    let mut idx = 0;
    while idx < jvm_end {
        if is_path_list_jvm_option(&compacted[idx]) && idx + 1 < jvm_end {
            argfile_tokens.push(compacted[idx].clone());
            argfile_tokens.push(compacted[idx + 1].clone());
            idx += 2;
        } else if is_two_token_jvm_option(&compacted[idx]) && idx + 1 < jvm_end {
            kept_jvm.push(compacted[idx].clone());
            kept_jvm.push(compacted[idx + 1].clone());
            idx += 2;
        } else {
            kept_jvm.push(compacted[idx].clone());
            idx += 1;
        }
    }

    let clean_argfile = strip_windows_verbatim_prefix(argfile_path);
    let argfile_flag = format!("@{}", clean_argfile.display());

    // If remaining args are still too long (e.g., giant Forge/NeoForge --add-opens list),
    // move non-javaagent JVM args before `main_class` into the argfile too.
    let mut probe_args = Vec::with_capacity(kept_jvm.len() + 1 + (compacted.len() - jvm_end));
    probe_args.extend(kept_jvm.iter().cloned());
    probe_args.push(argfile_flag.clone());
    probe_args.extend_from_slice(&compacted[jvm_end..]);

    if estimate_windows_cmdline_len(program, &probe_args) > WINDOWS_ARGFILE_THRESHOLD_CHARS {
        let mut minimal_jvm = Vec::new();
        let mut extra_argfile = Vec::new();
        let mut k = 0;
        while k < kept_jvm.len() {
            if kept_jvm[k].starts_with("-javaagent:") {
                minimal_jvm.push(kept_jvm[k].clone());
                k += 1;
            } else if is_two_token_jvm_option(&kept_jvm[k]) && k + 1 < kept_jvm.len() {
                extra_argfile.push(kept_jvm[k].clone());
                extra_argfile.push(kept_jvm[k + 1].clone());
                k += 2;
            } else {
                extra_argfile.push(kept_jvm[k].clone());
                k += 1;
            }
        }
        extra_argfile.extend(argfile_tokens);
        argfile_tokens = extra_argfile;
        kept_jvm = minimal_jvm;
    }

    if argfile_tokens.is_empty() {
        return Ok((compacted, None));
    }

    if let Some(parent) = clean_argfile.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let mut content = String::new();
    for tok in &argfile_tokens {
        content.push_str(&format_java_argfile_token(tok));
        content.push('\n');
    }
    std::fs::write(&clean_argfile, content.as_bytes())?;

    let mut final_args = Vec::with_capacity(kept_jvm.len() + 1 + (compacted.len() - jvm_end));
    final_args.extend(kept_jvm);
    final_args.push(argfile_flag);
    final_args.extend_from_slice(&compacted[jvm_end..]);

    Ok((final_args, Some(clean_argfile)))
}

fn shell_quote(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_owned();
    }
    let needs_quoting = arg
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\'));
    if !needs_quoting {
        return arg.to_owned();
    }

    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    for ch in arg.chars() {
        if matches!(ch, '"' | '\\') {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_all_known_placeholders_and_scrubs_unknowns_without_leaving_placeholder_markers() {
        let template = "--user ${auth_player_name} --ver ${version_name} --unknown ${unknown_var} --flag=${another_bad_var}";
        let subs = [("auth_player_name", "TestUser"), ("version_name", "26.3")];
        let expanded = expand_and_sanitize_arg(template, &subs);
        assert_eq!(expanded, "--user TestUser --ver 26.3 --unknown  --flag=");
        assert!(
            !expanded.contains("${"),
            "Result contains unexpanded placeholder marker!"
        );
    }

    #[test]
    fn scrubs_malformed_unclosed_placeholder() {
        let template = "-Dfoo=${unclosed_var";
        let expanded = expand_and_sanitize_arg(template, &[]);
        assert_eq!(expanded, "-Dfoo=");
        assert!(!expanded.contains("${"));
    }

    #[test]
    fn guarantee_no_placeholders_leak_into_built_command() {
        use mq_core::{ModLoaderKind, MqPaths};
        use mq_meta::{ArtifactDownload, AssetIndexPointer, JavaVersionSpec, ResolvedVersion};

        let temp = tempfile::tempdir().expect("tempdir");
        let paths = MqPaths::with_root(temp.path());
        paths.ensure_dirs().expect("dirs");

        let mut instance = InstanceManifest::new("Test 26", "26.3", ModLoaderKind::Vanilla, None);
        instance.memory_min_mb = 1024;
        instance.memory_max_mb = 2048;
        instance.jvm_args = vec!["-Dcustom.unknown=${user_custom_prop}".to_string()];

        let version = ResolvedVersion {
            id: "26.3".to_string(),
            mc_version: "26.3".to_string(),
            loader: ModLoaderKind::Vanilla,
            loader_version: None,
            inherits_from: None,
            main_class: "net.minecraft.client.main.Main".to_string(),
            java_version: JavaVersionSpec {
                component: "java-runtime-epsilon".to_string(),
                major_version: 25,
            },
            asset_index: AssetIndexPointer {
                id: "34".to_string(),
                sha1: None,
                size: 100,
                total_size: 1000,
                url: String::new(),
            },
            client_jar: ArtifactDownload {
                path: "versions/26.3/26.3.jar".to_string(),
                url: String::new(),
                sha1: None,
                size: 100,
            },
            libraries: vec![],
            jvm_arguments: vec![
                "-Djava.library.path=${natives_directory}".to_string(),
                "-cp".to_string(),
                "${classpath}".to_string(),
                "-Dlegacy.unknown=${some_unknown_jvm_flag}".to_string(),
            ],
            game_arguments: vec![
                "--username".to_string(),
                "${auth_player_name}".to_string(),
                "--version".to_string(),
                "${version_name}".to_string(),
                "--gameDir".to_string(),
                "${game_directory}".to_string(),
                "--assetsDir".to_string(),
                "${assets_root}".to_string(),
                "--assetIndex".to_string(),
                "${assets_index_name}".to_string(),
                "--uuid".to_string(),
                "${auth_uuid}".to_string(),
                "--accessToken".to_string(),
                "${auth_access_token}".to_string(),
                "--userType".to_string(),
                "${user_type}".to_string(),
                "--versionType".to_string(),
                "${version_type}".to_string(),
                "--quickPlayPath".to_string(),
                "${quickPlayPath}".to_string(),
                "--unknownGameFlag".to_string(),
                "${unrecognized_game_placeholder}".to_string(),
            ],
            logging: Some(mq_meta::LoggingConfig {
                client: Some(mq_meta::LoggingClientConfig {
                    argument: "-Dlog4j.configurationFile=${path}".to_string(),
                    file: ArtifactDownload {
                        path: "log_configs/client-26.xml".to_string(),
                        url: String::new(),
                        sha1: None,
                        size: 50,
                    },
                    logging_type: "log4j2-xml".to_string(),
                }),
            }),
        };

        let account = mq_auth::create_offline_account("Steve").expect("account");

        let ctx = LaunchContext {
            paths,
            instance,
            version,
            account,
            java_bin: PathBuf::from("/usr/bin/java"),
            authlib_injector_jar: None,
            extra_secrets_to_redact: vec![],
        };

        let cmd = build_launch_command(&ctx).expect("build launch command");

        for (idx, arg) in cmd.args.iter().enumerate() {
            assert!(
                !arg.contains("${"),
                "Unexpanded placeholder '${{' leaked at argument index {idx}: {arg}"
            );
        }

        let full_cmd_line = cmd.to_shell_command_line();
        assert!(
            !full_cmd_line.contains("${"),
            "Unexpanded placeholder '${{' leaked into full command line: {full_cmd_line}"
        );

        // Verify logging argument was added
        assert!(cmd
            .args
            .iter()
            .any(|arg| arg.starts_with("-Dlog4j.configurationFile=")));

        // Verify ALL JVM `-D...` flags appear strictly BEFORE `main_class`
        // (regression guard for Windows `-Dfile.encoding=UTF-8` ordering bug).
        let main_idx = cmd
            .args
            .iter()
            .position(|a| a == "net.minecraft.client.main.Main")
            .expect("main_class present");
        for game_arg in &cmd.args[main_idx + 1..] {
            assert!(
                !game_arg.starts_with("-Dfile.encoding="),
                "JVM property leaked after main_class as game argument: {game_arg}"
            );
        }
        #[cfg(target_os = "windows")]
        {
            let enc_idx = cmd
                .args
                .iter()
                .position(|a| a == "-Dfile.encoding=UTF-8")
                .expect("-Dfile.encoding=UTF-8 present on Windows");
            assert!(
                enc_idx < main_idx,
                "-Dfile.encoding=UTF-8 must precede main_class on Windows"
            );
        }
    }

    #[test]
    fn detects_linux_only_wrappers() {
        assert!(is_linux_only_wrapper("gamemoderun"));
        assert!(is_linux_only_wrapper("mangohud --dlsym"));
        assert!(is_linux_only_wrapper("/usr/bin/prime-run"));
        assert!(is_linux_only_wrapper("env DRI_PRIME=1 gamemoderun"));
        assert!(!is_linux_only_wrapper(r"C:\Tools\wrapper.exe --fast"));
    }

    #[test]
    fn formats_java_argfile_tokens_with_windows_escaping() {
        assert_eq!(format_java_argfile_token("-cp"), "-cp");
        assert_eq!(
            format_java_argfile_token(r#"C:\Users\John Doe\libs\a.jar;C:\libs\"b".jar"#),
            r#""C:\\Users\\John Doe\\libs\\a.jar;C:\\libs\\\"b\".jar""#
        );
    }

    #[test]
    fn windows_cmdline_limit_uses_argfile_for_java9_plus_and_compacts_for_java8() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let program = Path::new(r"C:\Program Files\Java\jdk-21\bin\java.exe");
        let workdir = Path::new(r"C:\Users\PlayerWithLongName\AppData\Local\MQ\instances\pack1");

        // Build a huge Windows classpath (~52 KB raw, ~30 KB even after relative compaction)
        // simulating a large Forge/NeoForge modpack that still exceeds WINDOWS_ARGFILE_THRESHOLD_CHARS.
        let cp: String = (0..450)
            .map(|i| {
                format!(
                    r"C:\Users\PlayerWithLongName\AppData\Local\MQ\libraries\org\example\modlib-{i:03}\1.0.{i}\modlib-{i:03}-1.0.{i}.jar"
                )
            })
            .collect::<Vec<_>>()
            .join(";");
        assert!(cp.len() > WINDOWS_ARGFILE_THRESHOLD_CHARS);
        assert!(
            compact_classpath_relative_to(&cp, workdir, ';').len()
                > WINDOWS_ARGFILE_THRESHOLD_CHARS
        );

        let args = vec![
            r"-javaagent:C:\MQ\authlib-injector.jar=https://account.ely.by/api/authlib-injector"
                .to_owned(),
            "-Xms1024M".to_owned(),
            "-Xmx4096M".to_owned(),
            "-cp".to_owned(),
            cp,
            "cpw.mods.bootstraplauncher.BootstrapLauncher".to_owned(),
            "--username".to_owned(),
            "Steve".to_owned(),
            "--accessToken".to_owned(),
            "SUPER_SECRET_TOKEN_12345".to_owned(),
        ];

        // 1. Java 21 -> uses @argfile and NEVER writes --accessToken to disk
        let argfile_path = tmp.path().join(".mq-args-test.argfile");
        let (rewritten_21, created_file) =
            rewrite_args_for_windows_limit(program, &args, workdir, Some(21), &argfile_path)
                .expect("rewrite java 21");
        let created_path = created_file.expect("argfile should be created for huge Java 21 cp");
        assert!(
            estimate_windows_cmdline_len(program, &rewritten_21) < WINDOWS_ARGFILE_THRESHOLD_CHARS
        );
        assert!(rewritten_21
            .iter()
            .any(|a| a == &format!("@{}", created_path.display())));
        let disk_content = std::fs::read_to_string(&created_path).expect("read argfile");
        assert!(disk_content.contains("-cp"));
        assert!(
            disk_content.contains(r"..\\..\\libraries\\org\\example\\modlib-000"),
            "expected compacted ASCII relative paths with escaped backslashes in argfile: {disk_content}"
        );
        assert!(
            !disk_content.contains("SUPER_SECRET_TOKEN_12345"),
            "game access token must never be written to @argfile"
        );

        // 2. Java 8 -> does NOT emit @argfile, compacts -cp relative to working_dir
        let argfile_path_8 = tmp.path().join(".mq-args-java8.argfile");
        let (rewritten_8, created_8) =
            rewrite_args_for_windows_limit(program, &args, workdir, Some(8), &argfile_path_8)
                .expect("rewrite java 8");
        assert!(created_8.is_none(), "Java 8 must not use @argfile");
        assert!(!argfile_path_8.exists());
        assert!(!rewritten_8.iter().any(|a| a.starts_with('@')));
        let cp_idx = rewritten_8.iter().position(|a| a == "-cp").unwrap();
        assert!(
            rewritten_8[cp_idx + 1].contains(r"..\..\libraries\org\example\modlib-000"),
            "Java 8 classpath should be compacted relative to working_dir"
        );
        assert!(
            rewritten_8[cp_idx + 1].len() < args[4].len(),
            "Java 8 classpath should shrink after compaction"
        );
    }
}
