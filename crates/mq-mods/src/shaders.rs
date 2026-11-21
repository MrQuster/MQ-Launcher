//! Shaders engine resolution, configuration management (`config/iris.properties`),
//! and GPU diagnostic probing for Minecraft shaderpacks.

use std::fs;
use std::path::Path;
#[cfg(not(target_os = "windows"))]
use std::process::Command;

use anyhow::{Context, Result};
use mq_core::ModLoaderKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShaderEngine {
    IrisSodium,
    OculusEmbeddium,
    None,
}

impl ShaderEngine {
    /// Detects recommended shader engine for the instance loader and version.
    pub fn for_loader(loader: ModLoaderKind) -> Self {
        match loader {
            ModLoaderKind::Fabric | ModLoaderKind::Quilt => Self::IrisSodium,
            ModLoaderKind::Forge | ModLoaderKind::NeoForge => Self::OculusEmbeddium,
            ModLoaderKind::Vanilla => Self::None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::IrisSodium => "Iris + Sodium",
            Self::OculusEmbeddium => "Oculus + Embeddium",
            Self::None => "Не поддерживается (Vanilla)",
        }
    }
}

#[derive(Debug, Clone)]
pub struct GpuDiagnostics {
    pub renderer: String,
    pub opengl_version: String,
    pub is_supported: bool,
    pub raw_info: String,
}

impl GpuDiagnostics {
    pub fn probe() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::probe_windows()
        }

        #[cfg(not(target_os = "windows"))]
        {
            let output = Command::new("glxinfo").arg("-B").output();

            match output {
                Ok(out) if out.status.success() => {
                    let text = String::from_utf8_lossy(&out.stdout);
                    let mut renderer = "Unknown GPU".to_string();
                    let mut opengl_version = "Unknown".to_string();

                    for line in text.lines() {
                        let trimmed = line.trim();
                        if trimmed.starts_with("OpenGL renderer string:") {
                            renderer = trimmed
                                .trim_start_matches("OpenGL renderer string:")
                                .trim()
                                .to_string();
                        } else if (trimmed.starts_with("OpenGL core profile version string:")
                            || trimmed.starts_with("OpenGL version string:"))
                            && opengl_version == "Unknown"
                        {
                            opengl_version = trimmed
                                .split(':')
                                .nth(1)
                                .unwrap_or("Unknown")
                                .trim()
                                .to_string();
                        }
                    }

                    let is_supported = !opengl_version.starts_with("1.")
                        && !opengl_version.starts_with("2.")
                        && !opengl_version.starts_with("3.0")
                        && !opengl_version.starts_with("3.1")
                        && !opengl_version.starts_with("3.2");

                    Self {
                        renderer,
                        opengl_version,
                        is_supported,
                        raw_info: text.to_string(),
                    }
                }
                _ => Self {
                    renderer: "OpenGL probe unavailable (glxinfo not found)".to_string(),
                    opengl_version: "Unknown".to_string(),
                    is_supported: true,
                    raw_info: String::new(),
                },
            }
        }
    }

    #[cfg(target_os = "windows")]
    fn probe_windows() -> Self {
        const DISPLAY_DEVICE_ACTIVE: u32 = 0x0000_0001;
        const DISPLAY_DEVICE_PRIMARY_DEVICE: u32 = 0x0000_0004;

        #[repr(C)]
        struct DisplayDeviceW {
            cb: u32,
            device_name: [u16; 32],
            device_string: [u16; 128],
            state_flags: u32,
            device_id: [u16; 128],
            device_key: [u16; 128],
        }

        #[link(name = "user32")]
        extern "system" {
            fn EnumDisplayDevicesW(
                lp_device: *const u16,
                i_dev_num: u32,
                lp_display_device: *mut DisplayDeviceW,
                dw_flags: u32,
            ) -> i32;
        }

        let decode_wide = |buf: &[u16]| -> String {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..len]).trim().to_string()
        };

        let mut primary_adapter: Option<String> = None;
        let mut active_adapter: Option<String> = None;
        let mut all_adapters: Vec<String> = Vec::new();

        for idx in 0..16u32 {
            let mut dd = DisplayDeviceW {
                cb: std::mem::size_of::<DisplayDeviceW>() as u32,
                device_name: [0u16; 32],
                device_string: [0u16; 128],
                state_flags: 0,
                device_id: [0u16; 128],
                device_key: [0u16; 128],
            };
            // SAFETY: `dd` is properly initialized with exact `cb` size for `EnumDisplayDevicesW`.
            let ok = unsafe { EnumDisplayDevicesW(std::ptr::null(), idx, &mut dd, 0) };
            if ok == 0 {
                break;
            }
            let adapter = decode_wide(&dd.device_string);
            if adapter.is_empty() {
                continue;
            }
            if (dd.state_flags & DISPLAY_DEVICE_ACTIVE) != 0 {
                if !all_adapters.contains(&adapter) {
                    all_adapters.push(adapter.clone());
                }
                if (dd.state_flags & DISPLAY_DEVICE_PRIMARY_DEVICE) != 0
                    && primary_adapter.is_none()
                {
                    primary_adapter = Some(adapter.clone());
                }
                if active_adapter.is_none() {
                    active_adapter = Some(adapter);
                }
            }
        }

        let renderer = primary_adapter
            .or(active_adapter)
            .unwrap_or_else(|| "Windows Display Adapter".to_string());
        let is_software = renderer
            .to_ascii_lowercase()
            .contains("microsoft basic display");

        Self {
            renderer,
            opengl_version: "Windows WGL / OpenGL".to_string(),
            is_supported: !is_software,
            raw_info: all_adapters.join("\n"),
        }
    }
}

pub struct ShaderManager;

impl ShaderManager {
    /// Lists all `.zip` or directory resourcepacks inside `<instance_dir>/resourcepacks/`.
    pub fn list_resourcepacks(instance_dir: &Path) -> Vec<String> {
        Self::list_packs_in_subfolder(instance_dir, "resourcepacks")
    }

    /// Lists all `.zip` shaderpacks inside `<instance_dir>/shaderpacks/`.
    pub fn list_shaderpacks(instance_dir: &Path) -> Vec<String> {
        Self::list_packs_in_subfolder(instance_dir, "shaderpacks")
    }

    fn list_packs_in_subfolder(instance_dir: &Path, subfolder: &str) -> Vec<String> {
        let packs_dir = instance_dir.join(subfolder);
        if !packs_dir.exists() {
            return Vec::new();
        }

        let mut packs = Vec::new();
        if let Ok(entries) = fs::read_dir(packs_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        if ext.eq_ignore_ascii_case("zip") || ext.eq_ignore_ascii_case("disabled") {
                            if let Some(name) = path.file_name() {
                                packs.push(name.to_string_lossy().to_string());
                            }
                        }
                    }
                } else if path.is_dir() {
                    if let Some(name) = path.file_name() {
                        packs.push(name.to_string_lossy().to_string());
                    }
                }
            }
        }
        packs.sort();
        packs
    }

    /// Reads currently active shaderpack from `config/iris.properties` or `config/oculus.properties`.
    pub fn read_active_shaderpack(instance_dir: &Path) -> Option<String> {
        for config_rel in ["config/iris.properties", "config/oculus.properties"] {
            let config_path = instance_dir.join(config_rel);
            if let Ok(content) = fs::read_to_string(config_path) {
                let mut enabled = false;
                let mut pack_name = None;
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("enableShaders=") {
                        enabled = trimmed.ends_with("true");
                    } else if trimmed.starts_with("shaderPack=") {
                        let val = trimmed.trim_start_matches("shaderPack=").trim();
                        if !val.is_empty() {
                            pack_name = Some(val.to_string());
                        }
                    }
                }
                if enabled {
                    return pack_name;
                }
            }
        }
        None
    }

    /// Writes active shaderpack configuration to `config/iris.properties` and `config/oculus.properties`.
    pub fn set_active_shaderpack(instance_dir: &Path, pack_name: Option<&str>) -> Result<()> {
        let config_dir = instance_dir.join("config");
        fs::create_dir_all(&config_dir).context("Failed to create config dir")?;

        let (enabled_str, pack_str) = match pack_name {
            Some(name) => ("true", name),
            None => ("false", ""),
        };

        let content = format!(
            "# Generated by MQ Launcher\nenableShaders={enabled_str}\nshaderPack={pack_str}\n"
        );

        mq_core::platform::atomic_write(&config_dir.join("iris.properties"), content.as_bytes())
            .context("Failed to write iris.properties")?;
        mq_core::platform::atomic_write(&config_dir.join("oculus.properties"), content.as_bytes())
            .context("Failed to write oculus.properties")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shader_properties_write_and_read() {
        let tmp = tempfile::tempdir().unwrap();
        let inst_dir = tmp.path();

        assert_eq!(ShaderManager::read_active_shaderpack(inst_dir), None);

        ShaderManager::set_active_shaderpack(inst_dir, Some("ComplementaryReimagined.zip"))
            .unwrap();
        assert_eq!(
            ShaderManager::read_active_shaderpack(inst_dir),
            Some("ComplementaryReimagined.zip".to_string())
        );

        ShaderManager::set_active_shaderpack(inst_dir, None).unwrap();
        assert_eq!(ShaderManager::read_active_shaderpack(inst_dir), None);
    }
}
