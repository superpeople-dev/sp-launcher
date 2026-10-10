//! What the game's own crash said, when it was the graphics card, and the way out.
//!
//! WHY
//! ---
//! Of the crash reports of 09-10.10.2026, seven were the graphics driver giving up under DirectX 12:
//! "GPU Crashed or D3D Device Removed" (DXGI_ERROR_DEVICE_REMOVED / _HUNG), "GameThread timed out
//! waiting for RenderThread" and "Out of video memory". The game's RHI ends the game on purpose when
//! the device is lost; nothing in its files is wrong, and no GPU breadcrumbs say more. The one log
//! of that batch on DirectX 11 crashed for something else. DirectX 11 is the game's most tried
//! renderer, so the launcher offers it once the game has closed this way.
//!
//! HOW
//! ---
//! Unreal writes each crash to `%LOCALAPPDATA%\BravoHotelGame\Saved\Crashes\<id>\` before it
//! exits; `CrashContext.runtime-xml` holds `<ErrorMessage>`. After the game exits, the newest
//! folder written since the launch is read. The renderer the game used is the player's own setting,
//! `GraphicsRHI` in `[/Script/Engine.GameUserSettings]` of `GameUserSettings.ini` ("DirectX 11" or
//! "DirectX 12", what Settings > Graphics > DirectX Version writes). Switching changes only that
//! key (engine_ini::with_setting); NVIDIA DLSS and Intel XeSS need DirectX 12, which the window says.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;

use crate::engine_ini;
use crate::error::{LauncherError, Result};

const PROJECT: &str = "BravoHotelGame";
pub const SECTION: &str = "[/Script/Engine.GameUserSettings]";
pub const KEY: &str = "GraphicsRHI";
pub const DX11: &str = "DirectX 11";
pub const DX12: &str = "DirectX 12";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The driver removed or reset the device ("GPU Crashed or D3D Device Removed").
    DeviceLost,
    /// The render thread stopped answering for two minutes (a hung GPU).
    RenderHang,
    /// A rendering resource did not fit in the card's memory.
    VideoMemory,
}

/// What the window needs: the kind, and whether the game ran on DirectX 12.
#[derive(Debug, Clone, Serialize)]
pub struct GpuCrash {
    pub kind: Kind,
    pub directx12: bool,
}

pub fn kind_of(message: &str) -> Option<Kind> {
    let m = message.to_ascii_lowercase();
    if m.contains("out of video memory") {
        Some(Kind::VideoMemory)
    } else if m.contains("d3d device removed") || m.contains("dxgi_error_device_") || m.contains("gpu crashed") {
        Some(Kind::DeviceLost)
    } else if m.contains("timed out waiting for renderthread") {
        Some(Kind::RenderHang)
    } else {
        None
    }
}

/// `<ErrorMessage>` of a CrashContext.runtime-xml (entities left as they are: only words are matched).
pub fn error_message(xml: &str) -> Option<&str> {
    let start = xml.find("<ErrorMessage>")? + "<ErrorMessage>".len();
    let end = xml[start..].find("</ErrorMessage>")? + start;
    Some(&xml[start..end])
}

/// The renderer a GameUserSettings.ini asks for; None when the key is not there (the game's own default,
/// DirectX 12 on most of the reports' PCs).
pub fn renderer(content: &str) -> Option<String> {
    let mut in_section = false;
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            in_section = t.eq_ignore_ascii_case(SECTION);
        } else if in_section {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim().eq_ignore_ascii_case(KEY) {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

fn saved() -> Result<PathBuf> {
    let base = std::env::var("LOCALAPPDATA").map_err(|_| LauncherError::Message("LOCALAPPDATA is not set".into()))?;
    Ok(Path::new(&base).join(PROJECT).join("Saved"))
}

/// The player's GameUserSettings.ini, next to the Engine.ini the launcher already writes.
pub fn settings_path() -> Result<PathBuf> {
    Ok(engine_ini::config_path()?.with_file_name("GameUserSettings.ini"))
}

/// The newest crash folder written at or after `since` whose error is the graphics card's.
pub fn newest_since(crashes: &Path, since: SystemTime) -> Option<Kind> {
    let mut newest: Option<(SystemTime, Kind)> = None;
    for entry in std::fs::read_dir(crashes).ok()?.flatten() {
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else { continue };
        if modified < since || newest.is_some_and(|(t, _)| t >= modified) {
            continue;
        }
        let Ok(xml) = std::fs::read_to_string(entry.path().join("CrashContext.runtime-xml")) else { continue };
        if let Some(kind) = error_message(&xml).and_then(kind_of) {
            newest = Some((modified, kind));
        }
    }
    newest.map(|(_, kind)| kind)
}

/// After the game exited: a graphics-card crash of this run, if there was one.
pub fn after_exit(launched: SystemTime) -> Option<GpuCrash> {
    let kind = newest_since(&saved().ok()?.join("Crashes"), launched)?;
    let content = settings_path().ok().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    // Offered unless the player is already on DirectX 11.
    let directx12 = !renderer(&content).is_some_and(|r| r.eq_ignore_ascii_case(DX11));
    Some(GpuCrash { kind, directx12 })
}

/// Settings > Graphics > DirectX Version = DirectX 11, the next time the game starts.
pub fn switch_to_directx11() -> Result<()> {
    let path = settings_path()?;
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = engine_ini::with_setting(&existing, SECTION, KEY, DX11);
    if updated != existing {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, updated)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_the_reports_messages() {
        assert_eq!(kind_of("GPU Crashed or D3D Device Removed. DXGI_ERROR_DEVICE_REMOVED"), Some(Kind::DeviceLost));
        assert_eq!(kind_of("GPU Crashed or D3D Device Removed. DXGI_ERROR_DEVICE_REMOVED with Reason: DXGI_ERROR_DEVICE_HUNG"), Some(Kind::DeviceLost));
        assert_eq!(kind_of("GameThread timed out waiting for RenderThread after 120.00 secs"), Some(Kind::RenderHang));
        assert_eq!(kind_of("Out of video memory trying to allocate a rendering resource. Make sure ..."), Some(Kind::VideoMemory));
        assert_eq!(kind_of("Unhandled Exception: EXCEPTION_ACCESS_VIOLATION reading address 0x000002c8"), None);
        assert_eq!(kind_of("Pak master signature table check failed for pak '../x.pak'"), None);
    }

    #[test]
    fn reads_the_error_message() {
        let xml = "<FGenericCrashContext><RuntimeProperties><ErrorMessage>GPU Crashed or D3D Device Removed.</ErrorMessage></RuntimeProperties></FGenericCrashContext>";
        assert_eq!(error_message(xml), Some("GPU Crashed or D3D Device Removed."));
        assert_eq!(error_message("<a/>"), None);
    }

    #[test]
    fn renderer_from_the_settings() {
        let dx12 = "[/Script/Engine.GameUserSettings]\r\nGraphicsRHI=DirectX 12\r\nUseNVStreamlineSDK=1\r\n\r\n[ScalabilityGroups]\r\nsg.TextureQuality=3\r\n";
        assert_eq!(renderer(dx12).as_deref(), Some(DX12));
        assert_eq!(renderer("[ScalabilityGroups]\nGraphicsRHI=DirectX 12\n"), None, "another section's key does not count");
        assert_eq!(renderer(""), None);
        let switched = engine_ini::with_setting(dx12, SECTION, KEY, DX11);
        assert_eq!(renderer(&switched).as_deref(), Some(DX11));
        assert!(switched.contains("UseNVStreamlineSDK=1\r\n") && switched.contains("sg.TextureQuality=3"), "{switched}");
    }

    #[test]
    fn newest_crash_of_this_run_only() {
        let dir = std::env::temp_dir().join(format!("sp-gpu-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |name: &str, msg: &str| {
            let d = dir.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("CrashContext.runtime-xml"), format!("<ErrorMessage>{msg}</ErrorMessage>")).unwrap();
        };
        write("old", "GPU Crashed or D3D Device Removed.");
        let since = SystemTime::now() + std::time::Duration::from_secs(3600);
        assert_eq!(newest_since(&dir, since), None, "a crash before the launch is not this run's");
        assert_eq!(newest_since(&dir, SystemTime::UNIX_EPOCH), Some(Kind::DeviceLost));
        write("other", "Unhandled Exception: EXCEPTION_ACCESS_VIOLATION");
        assert_eq!(newest_since(&dir, SystemTime::UNIX_EPOCH), Some(Kind::DeviceLost), "other crashes are not offered");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
