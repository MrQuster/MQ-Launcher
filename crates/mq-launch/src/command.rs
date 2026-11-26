//! Deterministic Minecraft + Modloader launch command builder (Lesson E8),
//! Ely.by `-javaagent` first-flag injection, Offline UUID/token substitution,
//! wrapper command prefixing (`gamemoderun`, `mangohud`, `prime-run`), and `--dry-run` formatting.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mq_auth::{elyby_jvm_injection_args, AccountKind, AccountProfile};
use mq_core::fs_util::strip_windows_verbatim_prefix;
use mq_core::{redact_text, resolve_safe_path, ModLoaderKind, MqError, MqPaths, MqResult};
use mq_instances::InstanceManifest;
use mq_meta::{
    build_deterministic_classpath, format_classpath_string, is_modular_bootstrap_main_class,
    ResolvedVersion,
};
use serde::{Deserialize, Serialize};

/// Conservative command-line character threshold below Windows's 32,767-char `CreateProcessW`
/// limit at which JVM classpath/module arguments are compacted or offloaded to a Java `@argfile`.
pub const WINDOWS_ARGFILE_THRESHOLD_CHARS: usize = 24_000;

/// Canonical launcher brand string injected into `${launcher_name}`.
pub const LAUNCHER_NAME: &str = "mq";

/// Canonical launcher version string injected into `${launcher_version}`.
pub const LAUNCHER_VERSION: &str = env!("CARGO_PKG_VERSION");

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

/// Type alias matching external interface contract.
pub type LaunchCommand = BuiltLaunchCommand;

/// Quick Play target for instantly joining singleplayer world or multiplayer server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickPlayTarget {
    Singleplayer(String),
    Multiplayer(String),
}

fn is_mc_version_ge_1_20(ver_str: &str) -> bool {
    let trimmed = ver_str.trim();
    if let Some(w_pos) = trimmed.find('w') {
        if let Ok(year) = trimmed[..w_pos].parse::<u32>() {
            return year >= 23;
        }
    }
    let core = trimmed.split(['-', '+', ' ']).next().unwrap_or(trimmed);
    let nums: Vec<u32> = core.split('.').filter_map(|s| s.parse().ok()).collect();
    match nums.as_slice() {
        [major, ..] if *major >= 20 => true,
        [1, minor, ..] if *minor >= 20 => true,
        _ => false,
    }
}

pub fn parse_host_port(addr: &str) -> (String, u16) {
    let trimmed = addr.trim();
    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some((h, after_bracket)) = rest.split_once(']') {
            if let Some(port_str) = after_bracket.strip_prefix(':') {
                let port = port_str.parse::<u16>().unwrap_or(25565);
                return (h.to_string(), port);
            } else {
                return (h.to_string(), 25565);
            }
        }
    }
    if let Some((h, p)) = trimmed.rsplit_once(':') {
        if h.contains(':') {
            // Bare IPv6 without brackets
            (trimmed.to_string(), 25565)
        } else if let Ok(port) = p.parse::<u16>() {
            (h.to_string(), port)
        } else {
            (trimmed.to_string(), 25565)
        }
    } else {
        (trimmed.to_string(), 25565)
    }
}

/// Builds launch command with optional Quick Play parameters.
pub fn build_launch_command_with_quick_play(
    ctx: &LaunchContext,
    quick_play: Option<&QuickPlayTarget>,
) -> MqResult<LaunchCommand> {
    let mut cmd = build_launch_command(ctx)?;
    if let Some(target) = quick_play {
        let mc_ver = if !ctx.version.mc_version.trim().is_empty() {
            &ctx.version.mc_version
        } else {
            &ctx.instance.mc_version
        };
        let is_modern = is_mc_version_ge_1_20(mc_ver);
        match target {
            QuickPlayTarget::Singleplayer(world) => {
                cmd.args.push("--quickPlaySingleplayer".to_string());
                cmd.args.push(world.clone());
            }
            QuickPlayTarget::Multiplayer(addr) => {
                if is_modern {
                    cmd.args.push("--quickPlayMultiplayer".to_string());
                    cmd.args.push(addr.clone());
                } else {
                    let (host, port) = parse_host_port(addr);
                    cmd.args.push("--server".to_string());
                    cmd.args.push(host);
                    cmd.args.push("--port".to_string());
                    cmd.args.push(port.to_string());
                }
            }
        }
    }
    Ok(cmd)
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

    let is_modular_bootstrap = is_modular_bootstrap_main_class(&ctx.version.main_class);
    let mut has_library_path = false;
    let mut has_classpath = false;
    let mut has_logging_config = false;

    // 3. User custom JVM arguments from `instance.toml` (sanitized against stray placeholders
    //    and harmful/incompatible GC presets such as `-Xnoclassgc` or `-XX:+UseSerialGC` on large
    //    or modded heaps).
    let sanitized_custom_args = sanitize_custom_jvm_args(
        &ctx.instance.jvm_args,
        max_mb,
        ctx.version.java_version.major_version,
        ctx.version.loader,
    );
    for custom_arg in &sanitized_custom_args {
        let mut expanded = expand_and_sanitize_arg(custom_arg, &jvm_subs)
            .trim()
            .to_owned();
        if expanded.is_empty() {
            continue;
        }
        if expanded.starts_with("-DignoreList=") {
            expanded = ensure_ignore_list_covers_client_jar(
                &expanded,
                &client_jar_path,
                &ctx.version.mc_version,
            );
        }
        if expanded.starts_with("-Djava.library.path=") {
            has_library_path = true;
        }
        if expanded == "-cp"
            || expanded == "-classpath"
            || expanded == "--class-path"
            || expanded.starts_with("--class-path=")
        {
            has_classpath = true;
        }
        if expanded.starts_with("-Dlog4j.configurationFile=") {
            has_logging_config = true;
        }
        jvm_flags.push(expanded);
    }

    // 4. Expanded version JVM arguments.
    for raw_jvm_arg in &ctx.version.jvm_arguments {
        let mut expanded = expand_and_sanitize_arg(raw_jvm_arg, &jvm_subs);
        if expanded.starts_with("-DignoreList=") {
            expanded = ensure_ignore_list_covers_client_jar(
                &expanded,
                &client_jar_path,
                &ctx.version.mc_version,
            );
        }
        if expanded.starts_with("-Djava.library.path=") {
            has_library_path = true;
        }
        if expanded == "-cp"
            || expanded == "-classpath"
            || expanded == "--class-path"
            || expanded.starts_with("--class-path=")
        {
            has_classpath = true;
        }
        if expanded.starts_with("-Dlog4j.configurationFile=") {
            has_logging_config = true;
        }
        jvm_flags.push(expanded);
    }

    // Inject Mojang logging configuration only when:
    // - not already provided via JVM args,
    // - not a modular Forge/NeoForge bootstrap (`fmlloader` bundles its own `log4j2.xml`;
    //   Mojang's inherited `client-1.12.xml` uses `<LegacyXMLLayout />` which breaks Forge's MCL logger),
    // - and the referenced logging XML file actually exists on disk (pointing Log4j2 at a missing
    //   file triggers `ERROR StatusLogger Reconfiguration failed: No configuration found`).
    if !has_logging_config && !is_modular_bootstrap {
        if let Some(ref logging) = ctx.version.logging {
            if let Some(ref client) = logging.client {
                let file_exists = !log_path_str.is_empty() && Path::new(&log_path_str).is_file();
                if file_exists && !client.argument.trim().is_empty() {
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

    let mut wrapper_env = BTreeMap::new();
    let (program, args) = match wrapper_effective {
        Some(wrapper_raw) => {
            let (w_env, prog_opt, w_args) = parse_wrapper_cmd(wrapper_raw);
            wrapper_env = w_env;
            if let Some(first_bin) = prog_opt {
                let mut full_args = Vec::with_capacity(w_args.len() + 1 + jvm_flags.len());
                full_args.extend(w_args);
                full_args.push(clean_java_bin.to_string_lossy().into_owned());
                full_args.extend(jvm_flags);
                (PathBuf::from(first_bin), full_args)
            } else {
                (clean_java_bin, jvm_flags)
            }
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
        env: wrapper_env,
    })
}

/// Parses a wrapper command line string, respecting single/double quotes and extracting
/// leading `KEY=VALUE` environment variable assignments (as well as `env KEY=VALUE...`).
pub fn parse_wrapper_cmd(
    wrapper_raw: &str,
) -> (BTreeMap<String, String>, Option<String>, Vec<String>) {
    let mut env = BTreeMap::new();
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quote: Option<char> = None;
    let mut chars = wrapper_raw.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' | '"' => {
                if in_quote == Some(ch) {
                    in_quote = None;
                } else if in_quote.is_none() {
                    in_quote = Some(ch);
                } else {
                    cur.push(ch);
                }
            }
            '\\' => {
                if in_quote == Some('"') && chars.peek() == Some(&'"') {
                    chars.next();
                    cur.push('"');
                } else {
                    cur.push('\\');
                }
            }
            c if c.is_whitespace() && in_quote.is_none() => {
                if !cur.is_empty() {
                    tokens.push(cur);
                    cur = String::new();
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }

    let mut token_iter = tokens.into_iter();
    let mut first = token_iter.next();
    if let Some(ref f) = first {
        if f == "env" {
            first = token_iter.next();
        }
    }

    while let Some(tok) = first {
        if let Some((k, v)) = tok.split_once('=') {
            if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                env.insert(k.to_string(), v.to_string());
                first = token_iter.next();
                continue;
            }
        }
        let program = tok;
        let rest_args: Vec<String> = token_iter.collect();
        return (env, Some(program), rest_args);
    }

    (env, None, Vec::new())
}

/// Ensures a Forge/NeoForge `-DignoreList=...` JVM property covers the actual resolved client JAR
/// filename (e.g. `client.jar` or `1.20.1.jar`) using `BootstrapLauncher`'s exact `startsWith`
/// prefix matching rules, without adding broad prefixes like `client` that would match
/// `client-<ver>-srg.jar` or `client-<ver>-extra.jar`.
pub fn ensure_ignore_list_covers_client_jar(
    flag: &str,
    client_jar_path: &Path,
    mc_version: &str,
) -> String {
    let Some(raw_list) = flag.strip_prefix("-DignoreList=") else {
        return flag.to_owned();
    };

    let mut entries: Vec<String> = raw_list
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();

    let mut required_names: Vec<String> = Vec::with_capacity(3);
    if let Some(fname) = client_jar_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        required_names.push(fname.to_owned());
    }
    required_names.push("client.jar".to_owned());
    let mc_trimmed = mc_version.trim();
    if !mc_trimmed.is_empty() {
        required_names.push(format!("{mc_trimmed}.jar"));
    }

    for candidate in required_names {
        let already_covered = entries
            .iter()
            .any(|prefix| candidate.starts_with(prefix.as_str()));
        if !already_covered {
            entries.push(candidate);
        }
    }

    format!("-DignoreList={}", entries.join(","))
}

/// Sanitizes custom JVM arguments from `instance.toml` against unsafe or Java-version-incompatible
/// GC presets:
/// - Strips `-Xnoclassgc` (disabling class unloading causes Metaspace leaks/OOM in modded Minecraft).
/// - Upgrades `-XX:+UseSerialGC` to `-XX:+UseG1GC` on large heaps (`>= 3072 MB`), Java 17+ modded
///   instances, or when paired with `-Xnoclassgc` (MQ's legacy Low-End preset).
/// - Drops `-XX:+ZGenerational` on Java < 21 and replaces `-XX:+UseZGC` with `-XX:+UseG1GC` on
///   Java < 15 so global presets never crash older Minecraft versions with `Unrecognized VM option`.
fn sanitize_custom_jvm_args(
    raw_args: &[String],
    max_mb: u32,
    java_major: u32,
    loader: ModLoaderKind,
) -> Vec<String> {
    let had_noclassgc = raw_args.iter().any(|a| a.trim() == "-Xnoclassgc");
    let has_explicit_non_serial_gc = raw_args.iter().any(|a| {
        let t = a.trim();
        t.starts_with("-XX:+Use")
            && t.ends_with("GC")
            && t != "-XX:+UseSerialGC"
            && !(t == "-XX:+UseZGC" && (1..15).contains(&java_major))
    });
    let should_upgrade_serial_gc =
        had_noclassgc || max_mb >= 3072 || (java_major >= 17 && loader != ModLoaderKind::Vanilla);

    let mut out = Vec::with_capacity(raw_args.len());
    let mut injected_g1gc = false;

    for raw in raw_args {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "-Xnoclassgc" {
            tracing::warn!("Ignoring -Xnoclassgc JVM flag to prevent Metaspace OOM");
            continue;
        }
        if trimmed == "-XX:+UseSerialGC" && should_upgrade_serial_gc {
            tracing::warn!(
                max_mb,
                java_major,
                ?loader,
                "Upgrading -XX:+UseSerialGC to -XX:+UseG1GC for large/modded heap"
            );
            if !has_explicit_non_serial_gc && !injected_g1gc {
                out.push("-XX:+UseG1GC".to_owned());
                injected_g1gc = true;
            }
            continue;
        }
        if trimmed.starts_with("-Xms") || trimmed.starts_with("-Xmx") {
            tracing::info!(
                flag = %trimmed,
                "Stripping redundant/conflicting heap flag from custom JVM args; memory is managed via instance memory settings"
            );
            continue;
        }
        if trimmed == "-XX:+ZGenerational" && !(21..24).contains(&java_major) {
            tracing::warn!(
                java_major,
                "Ignoring -XX:+ZGenerational on Java {java_major} (unsupported/obsolete) to prevent JVM startup failure"
            );
            continue;
        }
        if trimmed == "-XX:+UseZGC" && (1..15).contains(&java_major) {
            tracing::warn!(
                java_major,
                "Replacing -XX:+UseZGC with -XX:+UseG1GC on Java < 15 to prevent JVM startup failure"
            );
            if !has_explicit_non_serial_gc && !injected_g1gc {
                out.push("-XX:+UseG1GC".to_owned());
                injected_g1gc = true;
            }
            continue;
        }
        if trimmed == "-XX:+UseG1GC" {
            if injected_g1gc {
                continue;
            }
            injected_g1gc = true;
        }
        out.push(raw.clone());
    }

    let is_experimental_flag = |arg: &str| -> bool {
        arg.contains("G1NewSizePercent")
            || arg.contains("G1MaxNewSizePercent")
            || arg.contains("G1MixedGCLiveThresholdPercent")
            || arg.contains("G1HeapWastePercent")
            || arg.contains("UseShenandoahGC")
            || arg.contains("ZGenerational")
            || arg.contains("AlwaysActAsServerClassMachine")
            || arg.contains("UseFastUnorderedTimeStamps")
            || arg.contains("UseCompactObjectHeaders")
            || arg.contains("EnableJVMCI")
    };

    if let Some(first_exp_idx) = out.iter().position(|a| is_experimental_flag(a)) {
        let has_unlock_before = out[..first_exp_idx]
            .iter()
            .any(|a| a.trim() == "-XX:+UnlockExperimentalVMOptions");
        if !has_unlock_before {
            out.retain(|a| a.trim() != "-XX:+UnlockExperimentalVMOptions");
            if let Some(new_exp_idx) = out.iter().position(|a| is_experimental_flag(a)) {
                out.insert(new_exp_idx, "-XX:+UnlockExperimentalVMOptions".to_string());
            }
        }
    }

    out
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
    let needs_quoting = arg.starts_with('@')
        || arg
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
            | "--enable-native-access"
            | "--illegal-access"
            | "--describe-module"
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

pub(crate) fn is_java_launcher_binary(program: &Path) -> bool {
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
    let (prefix, actual_args) = if is_java_launcher_binary(program) {
        (&[][..], args)
    } else if let Some(pos) = args
        .iter()
        .position(|a| is_java_launcher_binary(Path::new(a)))
    {
        (&args[..=pos], &args[pos + 1..])
    } else {
        return Ok((args.to_vec(), None));
    };

    if estimate_windows_cmdline_len(program, args) <= WINDOWS_ARGFILE_THRESHOLD_CHARS {
        return Ok((args.to_vec(), None));
    }

    let jvm_end = find_jvm_args_end(actual_args);
    let mut compacted = actual_args.to_vec();

    // Step 1: compact `-cp` / `-p` entries relative to working_dir.
    let mut i = 0;
    while i + 1 < jvm_end {
        if is_path_list_jvm_option(&compacted[i]) {
            let sep = if compacted[i + 1].contains(';') {
                ';'
            } else if compacted[i + 1].contains(':') {
                ':'
            } else if cfg!(windows) {
                ';'
            } else {
                ':'
            };
            compacted[i + 1] = compact_classpath_relative_to(&compacted[i + 1], working_dir, sep);
            i += 2;
        } else if is_two_token_jvm_option(&compacted[i]) {
            i += 2;
        } else {
            i += 1;
        }
    }

    let mut full_compacted = prefix.to_vec();
    full_compacted.extend(compacted.clone());
    if estimate_windows_cmdline_len(program, &full_compacted) <= WINDOWS_ARGFILE_THRESHOLD_CHARS {
        return Ok((full_compacted, None));
    }

    // Step 2: Java 8 does not support `@argfile`; keep compacted args and warn if still huge.
    if java_major.is_some_and(|m| m < 9) {
        let est = estimate_windows_cmdline_len(program, &full_compacted);
        if est > 30_000 {
            tracing::warn!(
                estimated_chars = est,
                "Windows Java 8 command line is {est} chars (CreateProcessW limit is 32,767); \
                 Java 8 does not support @argfile — consider using a shorter MQ data directory"
            );
        }
        return Ok((full_compacted, None));
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
    let mut probe_args = prefix.to_vec();
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
        return Ok((full_compacted, None));
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

    let mut final_args = prefix.to_vec();
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
        let log_cfg_path = paths.assets_dir().join("log_configs").join("client-26.xml");
        std::fs::create_dir_all(log_cfg_path.parent().expect("log_configs dir"))
            .expect("create log_configs");
        std::fs::write(&log_cfg_path, b"<Configuration status=\"WARN\"/>")
            .expect("write log config");

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
    fn parse_wrapper_cmd_preserves_windows_backslashes_and_escapes_quoted_quotes() {
        let (env, prog, args) = parse_wrapper_cmd(r"C:\Tools\wrapper.exe --fast");
        assert!(env.is_empty());
        assert_eq!(prog.as_deref(), Some(r"C:\Tools\wrapper.exe"));
        assert_eq!(args, vec!["--fast"]);

        let (env2, prog2, args2) = parse_wrapper_cmd(
            r#"FOO="C:\My Dir\val" "C:\Program Files\Wrapper\wrap.exe" --msg "say \"hi\"""#,
        );
        assert_eq!(env2.get("FOO").map(String::as_str), Some(r"C:\My Dir\val"));
        assert_eq!(prog2.as_deref(), Some(r"C:\Program Files\Wrapper\wrap.exe"));
        assert_eq!(args2, vec!["--msg", r#"say "hi""#]);
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

    #[test]
    fn forge_1_20_1_bootstrap_launcher_omits_client_jar_updates_ignore_list_and_sanitizes_gc() {
        use mq_meta::{
            ArtifactDownload, AssetIndexPointer, JavaVersionSpec, LoggingClientConfig,
            LoggingConfig, ResolvedLibrary, ResolvedVersion,
        };

        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = MqPaths::with_root(tmp.path());
        paths.ensure_dirs().expect("dirs");

        let mut instance = InstanceManifest::new(
            "The Last Dawn II - Remastered",
            "1.20.1",
            ModLoaderKind::Forge,
            Some("47.4.10".to_owned()),
        );
        instance.memory_min_mb = 1024;
        instance.memory_max_mb = 6144;
        instance.jvm_args = vec!["-XX:+UseSerialGC".to_owned(), "-Xnoclassgc".to_owned()];

        let inst_dir = instance.instance_dir(&paths);
        std::fs::create_dir_all(&inst_dir).expect("inst dir");
        std::fs::write(inst_dir.join("client.jar"), b"vanilla-client-jar").expect("client.jar");

        let version = ResolvedVersion {
            id: "1.20.1-forge-47.4.10".to_owned(),
            mc_version: "1.20.1".to_owned(),
            loader: ModLoaderKind::Forge,
            loader_version: Some("47.4.10".to_owned()),
            inherits_from: Some("1.20.1".to_owned()),
            main_class: "cpw.mods.bootstraplauncher.BootstrapLauncher".to_owned(),
            java_version: JavaVersionSpec {
                component: "java-runtime-gamma".to_owned(),
                major_version: 17,
            },
            asset_index: AssetIndexPointer {
                id: "5".to_owned(),
                sha1: None,
                size: 100,
                total_size: 1000,
                url: String::new(),
            },
            client_jar: ArtifactDownload {
                path: "versions/1.20.1/1.20.1.jar".to_owned(),
                url: String::new(),
                sha1: None,
                size: 100,
            },
            libraries: vec![
                ResolvedLibrary {
                    name: "cpw.mods:bootstraplauncher:1.1.2".to_owned(),
                    artifact: Some(ArtifactDownload {
                        path: "cpw/mods/bootstraplauncher/1.1.2/bootstraplauncher-1.1.2.jar"
                            .to_owned(),
                        url: String::new(),
                        sha1: None,
                        size: 100,
                    }),
                    native_linux: None,
                    extract_exclude: Vec::new(),
                    include_in_classpath: true,
                },
                ResolvedLibrary {
                    name: "net.minecraftforge:fmlloader:1.20.1-47.4.10".to_owned(),
                    artifact: Some(ArtifactDownload {
                        path: "net/minecraftforge/fmlloader/1.20.1-47.4.10/fmlloader-1.20.1-47.4.10.jar"
                            .to_owned(),
                        url: String::new(),
                        sha1: None,
                        size: 100,
                    }),
                    native_linux: None,
                    extract_exclude: Vec::new(),
                    include_in_classpath: true,
                },
            ],
            jvm_arguments: vec![
                "-Djava.library.path=${natives_directory}".to_owned(),
                "-cp".to_owned(),
                "${classpath}".to_owned(),
                "-DignoreList=bootstraplauncher,securejarhandler,asm-commons,asm-util,asm-analysis,asm-tree,asm,JarJarFileSystems,client-extra,fmlcore,javafmllanguage,lowcodelanguage,mclanguage,forge-,${version_name}.jar".to_owned(),
                "-DlibraryDirectory=${library_directory}".to_owned(),
                "-p".to_owned(),
                "${library_directory}/cpw/mods/bootstraplauncher/1.1.2/bootstraplauncher-1.1.2.jar".to_owned(),
                "--add-modules".to_owned(),
                "ALL-MODULE-PATH".to_owned(),
            ],
            game_arguments: vec![
                "--username".to_owned(),
                "${auth_player_name}".to_owned(),
                "--launchTarget".to_owned(),
                "forgeclient".to_owned(),
                "--fml.forgeVersion".to_owned(),
                "47.4.10".to_owned(),
                "--fml.mcVersion".to_owned(),
                "1.20.1".to_owned(),
            ],
            logging: Some(LoggingConfig {
                client: Some(LoggingClientConfig {
                    argument: "-Dlog4j.configurationFile=${path}".to_owned(),
                    file: ArtifactDownload {
                        path: "log_configs/client-1.12.xml".to_owned(),
                        url: String::new(),
                        sha1: None,
                        size: 888,
                    },
                    logging_type: "log4j2-xml".to_owned(),
                }),
            }),
        };

        let ctx = LaunchContext {
            paths,
            instance,
            version,
            account: mq_auth::create_offline_account("Steve").expect("account"),
            java_bin: PathBuf::from("/usr/lib/jvm/java-17-openjdk/bin/java"),
            authlib_injector_jar: None,
            extra_secrets_to_redact: Vec::new(),
        };

        let cmd = build_launch_command(&ctx).expect("build forge 1.20.1 command");

        // 1. Classpath must omit vanilla `client.jar` to prevent split-module `client` vs `minecraft` crash.
        let cp_idx = cmd
            .args
            .iter()
            .position(|a| a == "-cp")
            .expect("-cp present");
        let cp_entries: Vec<&str> = cmd.args[cp_idx + 1]
            .split(mq_core::platform::classpath_separator())
            .collect();
        assert!(
            !cp_entries
                .iter()
                .any(|e| e.ends_with("client.jar") || e.ends_with("1.20.1.jar")),
            "Forge 1.20.1 -cp must not contain vanilla client.jar: {cp_entries:?}"
        );
        assert!(cp_entries
            .iter()
            .any(|e| e.ends_with("fmlloader-1.20.1-47.4.10.jar")));

        // 2. `-DignoreList` must cover `client.jar` and `1.20.1.jar` without broad `client` prefix.
        let ignore_flag = cmd
            .args
            .iter()
            .find(|a| a.starts_with("-DignoreList="))
            .expect("-DignoreList present");
        let ignore_entries: Vec<&str> = ignore_flag
            .strip_prefix("-DignoreList=")
            .unwrap()
            .split(',')
            .collect();
        assert!(ignore_entries.contains(&"client.jar"));
        assert!(ignore_entries.contains(&"1.20.1.jar"));
        assert!(
            !ignore_entries.contains(&"client"),
            "must not inject broad 'client' prefix that matches client-*-srg.jar"
        );

        // 3. Unsafe legacy Low-End GC flags must be upgraded to G1GC without `-Xnoclassgc`.
        assert!(!cmd.args.iter().any(|a| a == "-XX:+UseSerialGC"));
        assert!(!cmd.args.iter().any(|a| a == "-Xnoclassgc"));
        assert!(cmd.args.iter().any(|a| a == "-XX:+UseG1GC"));

        // 4. Forge BootstrapLauncher must not inject Mojang's inherited client-1.12.xml log config.
        assert!(
            !cmd.args
                .iter()
                .any(|a| a.starts_with("-Dlog4j.configurationFile=")),
            "Forge BootstrapLauncher should use fmlloader's bundled log4j2.xml"
        );

        // 5. No unexpanded placeholders remain.
        for arg in &cmd.args {
            assert!(!arg.contains("${"), "stray placeholder in arg: {arg}");
        }
    }

    #[test]
    fn neoforge_1_21_1_and_forge_1_20_4_bootstrap_launch_commands_are_clean() {
        use mq_meta::{
            ArtifactDownload, AssetIndexPointer, JavaVersionSpec, ResolvedLibrary, ResolvedVersion,
        };

        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = MqPaths::with_root(tmp.path());
        paths.ensure_dirs().expect("dirs");

        for (label, loader, mc_ver, ver_id, main_class, java_major, target) in [
            (
                "NeoForge 1.21.1",
                ModLoaderKind::NeoForge,
                "1.21.1",
                "neoforge-21.1.72",
                "cpw.mods.bootstraplauncher.BootstrapLauncher",
                21,
                "forgeclient",
            ),
            (
                "Forge 1.20.4",
                ModLoaderKind::Forge,
                "1.20.4",
                "1.20.4-forge-49.1.0",
                "net.minecraftforge.bootstrap.ForgeBootstrap",
                17,
                "forge_client",
            ),
        ] {
            let instance = InstanceManifest::new(label, mc_ver, loader, Some("1.0".to_owned()));
            let inst_dir = instance.instance_dir(&paths);
            std::fs::create_dir_all(&inst_dir).expect("inst dir");
            std::fs::write(inst_dir.join("client.jar"), b"vanilla").expect("client.jar");

            let version = ResolvedVersion {
                id: ver_id.to_owned(),
                mc_version: mc_ver.to_owned(),
                loader,
                loader_version: Some("1.0".to_owned()),
                inherits_from: Some(mc_ver.to_owned()),
                main_class: main_class.to_owned(),
                java_version: JavaVersionSpec {
                    component: "java-runtime-delta".to_owned(),
                    major_version: java_major,
                },
                asset_index: AssetIndexPointer {
                    id: "17".to_owned(),
                    sha1: None,
                    size: 100,
                    total_size: 1000,
                    url: String::new(),
                },
                client_jar: ArtifactDownload {
                    path: format!("versions/{mc_ver}/{mc_ver}.jar"),
                    url: String::new(),
                    sha1: None,
                    size: 100,
                },
                libraries: vec![ResolvedLibrary {
                    name: "cpw.mods:securejarhandler:3.0.8".to_owned(),
                    artifact: Some(ArtifactDownload {
                        path: "cpw/mods/securejarhandler/3.0.8/securejarhandler-3.0.8.jar"
                            .to_owned(),
                        url: String::new(),
                        sha1: None,
                        size: 100,
                    }),
                    native_linux: None,
                    extract_exclude: Vec::new(),
                    include_in_classpath: true,
                }],
                jvm_arguments: vec![
                    "-cp".to_owned(),
                    "${classpath}".to_owned(),
                    "-DignoreList=securejarhandler,client-extra,${version_name}.jar".to_owned(),
                    "-DlibraryDirectory=${library_directory}".to_owned(),
                ],
                game_arguments: vec![
                    "--launchTarget".to_owned(),
                    target.to_owned(),
                    "--fml.mcVersion".to_owned(),
                    mc_ver.to_owned(),
                ],
                logging: None,
            };

            let ctx = LaunchContext {
                paths: paths.clone(),
                instance,
                version,
                account: mq_auth::create_offline_account("Alex").expect("account"),
                java_bin: PathBuf::from("/usr/bin/java"),
                authlib_injector_jar: None,
                extra_secrets_to_redact: Vec::new(),
            };

            let cmd = build_launch_command(&ctx).expect("build bootstrap command");
            let cp_idx = cmd.args.iter().position(|a| a == "-cp").expect("-cp");
            assert!(
                !cmd.args[cp_idx + 1].contains("client.jar"),
                "{label} must omit vanilla client.jar from -cp"
            );
            let ignore_flag = cmd
                .args
                .iter()
                .find(|a| a.starts_with("-DignoreList="))
                .expect("-DignoreList");
            assert!(ignore_flag.contains("client.jar"), "{label}: {ignore_flag}");
            assert!(
                ignore_flag.contains(&format!("{mc_ver}.jar")),
                "{label}: {ignore_flag}"
            );
            // Idempotency check on ensure_ignore_list_covers_client_jar
            let reapplied = ensure_ignore_list_covers_client_jar(
                ignore_flag,
                &inst_dir.join("client.jar"),
                mc_ver,
            );
            assert_eq!(reapplied, *ignore_flag);
        }
    }

    #[test]
    fn fabric_vanilla_and_legacy_forge_1_12_2_retain_client_jar_and_skip_missing_log4j() {
        use mq_meta::{
            ArtifactDownload, AssetIndexPointer, JavaVersionSpec, LoggingClientConfig,
            LoggingConfig, ResolvedVersion,
        };

        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = MqPaths::with_root(tmp.path());
        paths.ensure_dirs().expect("dirs");

        for (name, mc_ver, loader, main_class, java_major) in [
            (
                "Vanilla 1.20.1",
                "1.20.1",
                ModLoaderKind::Vanilla,
                "net.minecraft.client.main.Main",
                17,
            ),
            (
                "Fabric 1.21.1",
                "1.21.1",
                ModLoaderKind::Fabric,
                "net.fabricmc.loader.impl.launch.knot.KnotClient",
                21,
            ),
            (
                "Forge 1.12.2",
                "1.12.2",
                ModLoaderKind::Forge,
                "net.minecraft.launchwrapper.Launch",
                8,
            ),
        ] {
            let instance = InstanceManifest::new(name, mc_ver, loader, None);
            let inst_dir = instance.instance_dir(&paths);
            std::fs::create_dir_all(&inst_dir).expect("inst dir");
            std::fs::write(inst_dir.join("client.jar"), b"jar").expect("client.jar");

            let version = ResolvedVersion {
                id: mc_ver.to_owned(),
                mc_version: mc_ver.to_owned(),
                loader,
                loader_version: None,
                inherits_from: None,
                main_class: main_class.to_owned(),
                java_version: JavaVersionSpec {
                    component: "jre-legacy".to_owned(),
                    major_version: java_major,
                },
                asset_index: AssetIndexPointer {
                    id: "5".to_owned(),
                    sha1: None,
                    size: 10,
                    total_size: 100,
                    url: String::new(),
                },
                client_jar: ArtifactDownload {
                    path: format!("versions/{mc_ver}/{mc_ver}.jar"),
                    url: String::new(),
                    sha1: None,
                    size: 100,
                },
                libraries: Vec::new(),
                jvm_arguments: vec!["-cp".to_owned(), "${classpath}".to_owned()],
                game_arguments: vec!["--username".to_owned(), "${auth_player_name}".to_owned()],
                logging: Some(LoggingConfig {
                    client: Some(LoggingClientConfig {
                        argument: "-Dlog4j.configurationFile=${path}".to_owned(),
                        file: ArtifactDownload {
                            path: "log_configs/missing-client.xml".to_owned(),
                            url: String::new(),
                            sha1: None,
                            size: 100,
                        },
                        logging_type: "log4j2-xml".to_owned(),
                    }),
                }),
            };

            let ctx = LaunchContext {
                paths: paths.clone(),
                instance,
                version,
                account: mq_auth::create_offline_account("Steve").expect("account"),
                java_bin: PathBuf::from("/usr/bin/java"),
                authlib_injector_jar: None,
                extra_secrets_to_redact: Vec::new(),
            };

            let cmd = build_launch_command(&ctx).expect("build command");
            let cp_idx = cmd.args.iter().position(|a| a == "-cp").expect("-cp");
            assert!(
                cmd.args[cp_idx + 1].ends_with("client.jar"),
                "{name} must keep client.jar on -cp, got: {}",
                cmd.args[cp_idx + 1]
            );
            assert!(
                !cmd.args
                    .iter()
                    .any(|a| a.starts_with("-Dlog4j.configurationFile=")),
                "{name} must not pass -Dlog4j.configurationFile for a missing file on disk"
            );
        }
    }

    #[test]
    fn sanitizes_incompatible_gc_presets_across_java_versions() {
        // Legacy Low-End preset on 6GB Java 17 Forge -> G1GC, no -Xnoclassgc
        assert_eq!(
            sanitize_custom_jvm_args(
                &["-XX:+UseSerialGC".to_owned(), "-Xnoclassgc".to_owned()],
                6144,
                17,
                ModLoaderKind::Forge
            ),
            vec!["-XX:+UseG1GC".to_owned()]
        );

        // Java 21 Generational ZGC preset unlocks experimental VM options on Java 21
        let zgc_gen = vec!["-XX:+UseZGC".to_owned(), "-XX:+ZGenerational".to_owned()];
        assert_eq!(
            sanitize_custom_jvm_args(&zgc_gen, 4096, 21, ModLoaderKind::NeoForge),
            vec![
                "-XX:+UseZGC".to_owned(),
                "-XX:+UnlockExperimentalVMOptions".to_owned(),
                "-XX:+ZGenerational".to_owned(),
            ]
        );

        // Java 21 Generational ZGC preset drops `-XX:+ZGenerational` on Java 17
        assert_eq!(
            sanitize_custom_jvm_args(&zgc_gen, 4096, 17, ModLoaderKind::Forge),
            vec!["-XX:+UseZGC".to_owned()]
        );

        // Java 25 drops `-XX:+ZGenerational` (obsoleted per JEP 490)
        assert_eq!(
            sanitize_custom_jvm_args(&zgc_gen, 4096, 25, ModLoaderKind::Vanilla),
            vec!["-XX:+UseZGC".to_owned()]
        );

        // Strips conflicting -Xms and -Xmx
        assert_eq!(
            sanitize_custom_jvm_args(
                &[
                    "-Xms2G".to_owned(),
                    "-Xmx4G".to_owned(),
                    "-XX:+UseG1GC".to_owned()
                ],
                4096,
                21,
                ModLoaderKind::Vanilla
            ),
            vec!["-XX:+UseG1GC".to_owned()]
        );

        // Java 21 Generational ZGC preset downgrades safely to G1GC on Java 8
        assert_eq!(
            sanitize_custom_jvm_args(&zgc_gen, 2048, 8, ModLoaderKind::Forge),
            vec!["-XX:+UseG1GC".to_owned()]
        );

        // Explicit SerialGC alone on small Vanilla Java 8 heap is respected
        assert_eq!(
            sanitize_custom_jvm_args(
                &["-XX:+UseSerialGC".to_owned()],
                1024,
                8,
                ModLoaderKind::Vanilla
            ),
            vec!["-XX:+UseSerialGC".to_owned()]
        );

        // Automatic repositioning of UnlockExperimentalVMOptions before experimental flags
        let misplaced_experimental = vec![
            "-XX:+UseG1GC".to_owned(),
            "-XX:G1NewSizePercent=30".to_owned(),
            "-XX:G1MaxNewSizePercent=40".to_owned(),
            "-XX:+UnlockExperimentalVMOptions".to_owned(),
        ];
        assert_eq!(
            sanitize_custom_jvm_args(&misplaced_experimental, 4096, 21, ModLoaderKind::Vanilla),
            vec![
                "-XX:+UseG1GC".to_owned(),
                "-XX:+UnlockExperimentalVMOptions".to_owned(),
                "-XX:G1NewSizePercent=30".to_owned(),
                "-XX:G1MaxNewSizePercent=40".to_owned(),
            ]
        );

        let missing_unlock_experimental = vec![
            "-XX:+UseG1GC".to_owned(),
            "-XX:G1NewSizePercent=20".to_owned(),
        ];
        assert_eq!(
            sanitize_custom_jvm_args(
                &missing_unlock_experimental,
                4096,
                21,
                ModLoaderKind::Vanilla
            ),
            vec![
                "-XX:+UseG1GC".to_owned(),
                "-XX:+UnlockExperimentalVMOptions".to_owned(),
                "-XX:G1NewSizePercent=20".to_owned(),
            ]
        );
    }
}
