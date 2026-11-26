//! Cross-platform system font loader with full Cyrillic + Latin proportional and monospace
//! discovery on Windows (`%WINDIR%\Fonts`, `%LOCALAPPDATA%\Microsoft\Windows\Fonts`) and Linux,
//! falling back gracefully to built-in `egui` fonts when running in minimal environments.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Context, FontData, FontDefinitions, FontFamily};

/// Ordered proportional font candidates across Windows and Linux distributions.
pub const PROPORTIONAL_FONT_CANDIDATES: &[&str] = &[
    "C:\\Windows\\Fonts\\segoeui.ttf",
    "C:\\Windows\\Fonts\\segoeuib.ttf",
    "C:\\Windows\\Fonts\\arial.ttf",
    "C:\\Windows\\Fonts\\tahoma.ttf",
    "C:\\Windows\\Fonts\\calibri.ttf",
    "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf",
    "/usr/share/fonts/TTF/Inter-Regular.ttf",
    "/usr/share/fonts/inter/Inter-Regular.ttf",
    "/usr/share/fonts/cantarell/Cantarell-VF.otf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/gnu-free/FreeSans.otf",
    "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
];

/// Ordered monospace font candidates across Windows and Linux distributions.
pub const MONOSPACE_FONT_CANDIDATES: &[&str] = &[
    "C:\\Windows\\Fonts\\consola.ttf",
    "C:\\Windows\\Fonts\\CascadiaMono.ttf",
    "C:\\Windows\\Fonts\\cascadiamono.ttf",
    "C:\\Windows\\Fonts\\lucon.ttf",
    "C:\\Windows\\Fonts\\cour.ttf",
    "/usr/share/fonts/TTF/JetBrainsMono-Regular.ttf",
    "/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf",
    "/usr/share/fonts/IosevkaNerdFont/IosevkaNerdFontMono-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "/usr/share/fonts/liberation/LiberationMono-Regular.ttf",
    "/usr/share/fonts/gnu-free/FreeMono.otf",
];

const WINDOWS_PROPORTIONAL_FILES: &[&str] = &[
    "segoeui.ttf",
    "segoeuib.ttf",
    "arial.ttf",
    "tahoma.ttf",
    "calibri.ttf",
];

const WINDOWS_MONOSPACE_FILES: &[&str] = &[
    "consola.ttf",
    "CascadiaMono.ttf",
    "cascadiamono.ttf",
    "lucon.ttf",
    "cour.ttf",
];

/// Optional CJK system fallback font candidates on Linux (only loaded if present on disk).
pub const CJK_FALLBACK_FONT_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto/NotoSansCJKsc-Regular.otf",
    "/usr/share/fonts/TTF/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/wqy-zenhei/wqy-zenhei.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
    "/usr/share/fonts/droid/DroidSansFallbackFull.ttf",
];

const WINDOWS_CJK_FILES: &[&str] = &[
    "msyh.ttc",
    "msyh.ttf",
    "YuGothR.ttc",
    "YuGothM.ttc",
    "meiryo.ttc",
    "simsun.ttc",
    "malgun.ttf",
];

const SYSTEM_PROPORTIONAL_KEY: &str = "mq_system_proportional";
const SYSTEM_MONOSPACE_KEY: &str = "mq_system_monospace";
const SYSTEM_CJK_FALLBACK_KEY: &str = "mq_system_cjk_fallback";

/// Summary of the proportional and monospace fonts selected during startup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedFontSummary {
    /// Path to the loaded system proportional font, or `None` if using `egui` built-in fallback.
    pub proportional_path: Option<PathBuf>,
    /// Path to the loaded system monospace font, or `None` if using `egui` built-in fallback.
    pub monospace_path: Option<PathBuf>,
    /// Path to an optional CJK fallback font, or `None` if unavailable on this host.
    pub cjk_fallback_path: Option<PathBuf>,
}

impl LoadedFontSummary {
    /// Returns `true` if a system proportional font was loaded from disk.
    #[inline]
    pub fn has_system_proportional(&self) -> bool {
        self.proportional_path.is_some()
    }

    /// Returns `true` if a system monospace font was loaded from disk.
    #[inline]
    pub fn has_system_monospace(&self) -> bool {
        self.monospace_path.is_some()
    }

    /// Human-readable description of active fonts.
    pub fn describe(&self) -> String {
        let prop = self
            .proportional_path
            .as_deref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("egui-builtin-proportional");
        let mono = self
            .monospace_path
            .as_deref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("egui-builtin-monospace");
        let cjk = self
            .cjk_fallback_path
            .as_deref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("none");
        format!("proportional={prop}, monospace={mono}, cjk={cjk}")
    }
}

fn push_unique(out: &mut Vec<PathBuf>, path: PathBuf) {
    if !out.contains(&path) {
        out.push(path);
    }
}

/// Builds ordered Cyrillic-capable Windows proportional font candidate paths from
/// `%WINDIR%\Fonts` (or `%SystemRoot%\Fonts`, defaulting to `C:\Windows\Fonts`) and
/// per-user fonts under `%LOCALAPPDATA%\Microsoft\Windows\Fonts`.
pub fn discover_windows_proportional_fonts(
    windir: Option<&Path>,
    local_app_data: Option<&Path>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let sys_fonts = windir
        .map(|d| d.join("Fonts"))
        .unwrap_or_else(|| PathBuf::from("C:\\Windows\\Fonts"));
    for file in WINDOWS_PROPORTIONAL_FILES {
        push_unique(&mut out, sys_fonts.join(file));
    }
    if let Some(local) = local_app_data {
        let user_fonts = local.join("Microsoft").join("Windows").join("Fonts");
        for file in WINDOWS_PROPORTIONAL_FILES {
            push_unique(&mut out, user_fonts.join(file));
        }
    }
    out
}

/// Builds ordered Windows monospace font candidate paths from `%LOCALAPPDATA%\Microsoft\Windows\Fonts`
/// and `%WINDIR%\Fonts` (`consola.ttf`, `CascadiaMono.ttf`, `lucon.ttf`, `cour.ttf`).
pub fn discover_windows_monospace_fonts(
    windir: Option<&Path>,
    local_app_data: Option<&Path>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(local) = local_app_data {
        let user_fonts = local.join("Microsoft").join("Windows").join("Fonts");
        for file in [
            "JetBrainsMono-Regular.ttf",
            "CascadiaMono.ttf",
            "cascadiamono.ttf",
            "consola.ttf",
        ] {
            push_unique(&mut out, user_fonts.join(file));
        }
    }
    let sys_fonts = windir
        .map(|d| d.join("Fonts"))
        .unwrap_or_else(|| PathBuf::from("C:\\Windows\\Fonts"));
    for file in WINDOWS_MONOSPACE_FILES {
        push_unique(&mut out, sys_fonts.join(file));
    }
    out
}

/// Builds ordered optional CJK fallback font candidate paths on Windows.
pub fn discover_windows_cjk_fonts(
    windir: Option<&Path>,
    local_app_data: Option<&Path>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let sys_fonts = windir
        .map(|d| d.join("Fonts"))
        .unwrap_or_else(|| PathBuf::from("C:\\Windows\\Fonts"));
    for file in WINDOWS_CJK_FILES {
        push_unique(&mut out, sys_fonts.join(file));
    }
    if let Some(local) = local_app_data {
        let user_fonts = local.join("Microsoft").join("Windows").join("Fonts");
        for file in WINDOWS_CJK_FILES {
            push_unique(&mut out, user_fonts.join(file));
        }
    }
    out
}

fn discover_user_jetbrains_mono() -> Vec<PathBuf> {
    let mut results = Vec::new();
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return results;
    };
    let local_fonts = home.join(".local/share/fonts");
    let direct = local_fonts.join("JetBrainsMono-Regular.ttf");
    if direct.is_file() {
        results.push(direct);
    }
    if let Ok(entries) = fs::read_dir(&local_fonts) {
        let mut matched: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.is_file()
                    && path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|name| {
                            name.contains("JetBrains")
                                && name.contains("Mono")
                                && name.contains("Regular")
                                && name.ends_with(".ttf")
                        })
                        .unwrap_or(false)
            })
            .collect();
        matched.sort();
        for p in matched {
            if !results.contains(&p) {
                results.push(p);
            }
        }
    }
    results
}

fn load_first_available_font<I, P>(candidates: I) -> Option<(PathBuf, Vec<u8>)>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    for candidate in candidates {
        let path = candidate.as_ref();
        if path.is_file() {
            if let Ok(bytes) = fs::read(path) {
                if bytes.len() >= 64 {
                    return Some((path.to_path_buf(), bytes));
                }
            }
        }
    }
    None
}

/// Probes OS system font paths and builds [`FontDefinitions`] with Cyrillic + Latin
/// proportional and monospace fonts inserted at priority index `0`, preserving built-in
/// `egui` fonts as fallbacks.
pub fn build_font_definitions() -> (FontDefinitions, LoadedFontSummary) {
    let mut defs = FontDefinitions::default();
    let mut summary = LoadedFontSummary::default();

    let (prop_candidates, mono_candidates, cjk_candidates) = if cfg!(target_os = "windows") {
        let windir = env::var_os("WINDIR")
            .or_else(|| env::var_os("SystemRoot"))
            .map(PathBuf::from);
        let local_app_data = env::var_os("LOCALAPPDATA").map(PathBuf::from);
        (
            discover_windows_proportional_fonts(windir.as_deref(), local_app_data.as_deref()),
            discover_windows_monospace_fonts(windir.as_deref(), local_app_data.as_deref()),
            discover_windows_cjk_fonts(windir.as_deref(), local_app_data.as_deref()),
        )
    } else {
        let prop: Vec<PathBuf> = PROPORTIONAL_FONT_CANDIDATES
            .iter()
            .filter(|p| p.starts_with('/'))
            .map(PathBuf::from)
            .collect();
        let mut mono = discover_user_jetbrains_mono();
        for &static_path in MONOSPACE_FONT_CANDIDATES {
            if static_path.starts_with('/') {
                push_unique(&mut mono, PathBuf::from(static_path));
            }
        }
        let cjk: Vec<PathBuf> = CJK_FALLBACK_FONT_CANDIDATES
            .iter()
            .map(PathBuf::from)
            .collect();
        (prop, mono, cjk)
    };

    if let Some((path, bytes)) = load_first_available_font(&prop_candidates) {
        defs.font_data.insert(
            SYSTEM_PROPORTIONAL_KEY.to_owned(),
            Arc::new(FontData::from_owned(bytes)),
        );
        defs.families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, SYSTEM_PROPORTIONAL_KEY.to_owned());
        summary.proportional_path = Some(path);
    }

    if let Some((path, bytes)) = load_first_available_font(&mono_candidates) {
        defs.font_data.insert(
            SYSTEM_MONOSPACE_KEY.to_owned(),
            Arc::new(FontData::from_owned(bytes)),
        );
        defs.families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, SYSTEM_MONOSPACE_KEY.to_owned());
        // Also append monospace as fallback in Proportional for box-drawing / technical glyphs.
        defs.families
            .entry(FontFamily::Proportional)
            .or_default()
            .push(SYSTEM_MONOSPACE_KEY.to_owned());
        summary.monospace_path = Some(path);
    }

    if let Some((path, bytes)) = load_first_available_font(&cjk_candidates) {
        defs.font_data.insert(
            SYSTEM_CJK_FALLBACK_KEY.to_owned(),
            Arc::new(FontData::from_owned(bytes)),
        );
        defs.families
            .entry(FontFamily::Proportional)
            .or_default()
            .push(SYSTEM_CJK_FALLBACK_KEY.to_owned());
        defs.families
            .entry(FontFamily::Monospace)
            .or_default()
            .push(SYSTEM_CJK_FALLBACK_KEY.to_owned());
        summary.cjk_fallback_path = Some(path);
    }

    (defs, summary)
}

/// Loads system fonts via [`build_font_definitions`] and installs them into `ctx`.
pub fn configure_system_fonts(ctx: &Context) -> LoadedFontSummary {
    let (defs, summary) = build_font_definitions();
    ctx.set_fonts(defs);
    tracing::debug!(fonts = %summary.describe(), "configured UI font families");
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_font_definitions_loads_system_fonts_and_keeps_fallbacks() {
        let (defs, summary) = build_font_definitions();

        let prop_list = defs
            .families
            .get(&FontFamily::Proportional)
            .expect("Proportional family must exist");
        let mono_list = defs
            .families
            .get(&FontFamily::Monospace)
            .expect("Monospace family must exist");

        assert!(!prop_list.is_empty());
        assert!(!mono_list.is_empty());

        if let Some(ref prop_path) = summary.proportional_path {
            assert!(prop_path.is_file());
            assert_eq!(prop_list[0], SYSTEM_PROPORTIONAL_KEY);
            assert!(defs.font_data.contains_key(SYSTEM_PROPORTIONAL_KEY));
            // Built-in fallback remains after index 0
            assert!(prop_list.len() >= 2);
        }

        if let Some(ref mono_path) = summary.monospace_path {
            assert!(mono_path.is_file());
            assert_eq!(mono_list[0], SYSTEM_MONOSPACE_KEY);
            assert!(defs.font_data.contains_key(SYSTEM_MONOSPACE_KEY));
            assert!(mono_list.len() >= 2);
        }

        // On desktop hosts (outside headless CI runners) at least one proportional and monospace font exist.
        if std::env::var_os("CI").is_none() {
            assert!(
                summary.has_system_proportional(),
                "expected system proportional font on desktop host"
            );
            assert!(
                summary.has_system_monospace(),
                "expected system monospace font on desktop host"
            );
        }
    }

    #[test]
    fn windows_font_discovery_resolves_windir_and_localappdata_in_priority_order() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let windir = tmp.path().join("WinRoot");
        let local = tmp.path().join("LocalAppData");
        let sys_fonts = windir.join("Fonts");
        let user_fonts = local.join("Microsoft").join("Windows").join("Fonts");
        fs::create_dir_all(&sys_fonts).expect("sys fonts dir");
        fs::create_dir_all(&user_fonts).expect("user fonts dir");

        let prop_list = discover_windows_proportional_fonts(Some(&windir), Some(&local));
        let prop_names: Vec<&str> = prop_list
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .collect();
        assert_eq!(
            &prop_names[..5],
            &[
                "segoeui.ttf",
                "segoeuib.ttf",
                "arial.ttf",
                "tahoma.ttf",
                "calibri.ttf"
            ]
        );
        assert!(prop_list[0].starts_with(&sys_fonts));

        let mono_list = discover_windows_monospace_fonts(Some(&windir), Some(&local));
        assert!(mono_list[0].starts_with(&user_fonts));
        assert!(mono_list
            .iter()
            .any(|p| p == &sys_fonts.join("consola.ttf")));
        assert!(mono_list
            .iter()
            .any(|p| p == &sys_fonts.join("CascadiaMono.ttf")));
        assert!(mono_list.iter().any(|p| p == &sys_fonts.join("lucon.ttf")));
        assert!(mono_list.iter().any(|p| p == &sys_fonts.join("cour.ttf")));

        // Verify load_first_available_font picks first valid >=64 byte file
        let dummy_ttf = vec![0x42_u8; 128];
        fs::write(sys_fonts.join("tahoma.ttf"), &dummy_ttf).expect("write tahoma");
        let (loaded_prop, bytes) =
            load_first_available_font(&prop_list).expect("should find tahoma");
        assert_eq!(loaded_prop, sys_fonts.join("tahoma.ttf"));
        assert_eq!(bytes.len(), 128);

        fs::write(user_fonts.join("CascadiaMono.ttf"), &dummy_ttf).expect("write cascadia");
        fs::write(sys_fonts.join("consola.ttf"), &dummy_ttf).expect("write consola");
        let (loaded_mono, _) =
            load_first_available_font(&mono_list).expect("should find user CascadiaMono first");
        assert_eq!(loaded_mono, user_fonts.join("CascadiaMono.ttf"));
    }
}
