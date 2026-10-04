//! The game's crash window sends to the community's backend.
//!
//! When the game crashes, Unreal's CrashReportClient asks the player what
//! happened, and Send uploads the crash folder (the game log, the crash
//! context, a crash dump, a screenshot) to its `DataRouterUrl`. The game's own
//! value, in `Engine/Programs/CrashReportClient/Content/Paks/CrashReportClient.pak`,
//! is the original developer's BugSplat database, which nobody here can read.
//!
//! The reporter also reads its user config,
//! `%LOCALAPPDATA%\CrashReportClient\Saved\Config\Windows\Engine.ini`, on top
//! of the pak, so the launcher sets `[CrashReportClient] DataRouterUrl` there
//! before every launch: sp-backend `lib/crashreports.js` keeps the report and
//! posts it to the staff's #crash-logs. Proven on 04.10.2026: with this line,
//! the game's own `CrashReportClient.exe -Unattended` on a real crash folder
//! posted to the overridden URL.
//!
//! Written every launch, like the game's `Engine.ini` (engine_ini.rs): the
//! reporter rewrites its file when it exits. Only this one key in this one
//! section is touched (`engine_ini::with_setting`); a file that cannot be read
//! is left alone rather than replaced.

use std::path::{Path, PathBuf};

use crate::engine_ini;
use crate::error::{LauncherError, Result};

pub const SECTION: &str = "[CrashReportClient]";
pub const KEY: &str = "DataRouterUrl";
/// sp-backend `POST /crashreport` (lib/crashreports.js), quoted as the pak writes it.
pub const VALUE: &str = "\"http://64.226.112.204:8080/crashreport\"";

/// Where the reporter keeps its user config.
pub fn config_path() -> Result<PathBuf> {
    let base = std::env::var("LOCALAPPDATA")
        .map_err(|_| LauncherError::Message("LOCALAPPDATA is not set".into()))?;
    Ok(Path::new(&base).join("CrashReportClient").join("Saved").join("Config").join("Windows").join("Engine.ini"))
}

/// The file with our `DataRouterUrl`; None when it already has it.
fn updated(existing: &str) -> Option<String> {
    let next = engine_ini::with_setting(existing, SECTION, KEY, VALUE);
    (next != existing).then_some(next)
}

/// Points the crash window at the backend. A failure never keeps the game from starting.
pub fn apply() -> Result<PathBuf> {
    let path = config_path()?;
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    if let Some(next) = updated(&existing) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, next)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_file_gets_the_section_and_the_url() {
        assert_eq!(updated("").unwrap(), "[CrashReportClient]\nDataRouterUrl=\"http://64.226.112.204:8080/crashreport\"\n");
    }

    #[test]
    fn the_reporters_own_settings_stay_as_they_are() {
        // What the reporter itself had written on a PC (04.10.2026), CRLF.
        let before = "[HTTP]\r\nHttpSendTimeout=300.000000\r\n\r\n[WindowsApplication.Accessibility]\r\nStickyKeysHotkey=False\r\n\r\n";
        let after = updated(before).unwrap();
        assert!(after.starts_with(before.trim_end()), "everything before stays");
        assert!(after.contains("[CrashReportClient]\r\nDataRouterUrl=\"http://64.226.112.204:8080/crashreport\"\r\n"), "{after:?}");
    }

    #[test]
    fn another_url_is_replaced_and_the_rest_of_the_section_kept() {
        let before = "[CrashReportClient]\nDataRouterUrl=\"https://40_Main.bugsplat.com/post/ue4/Package/473797\"\nbSendLogFile=true\n";
        let after = updated(before).unwrap();
        assert!(!after.contains("bugsplat") && after.contains("8080/crashreport") && after.contains("bSendLogFile=true"), "{after:?}");
    }

    #[test]
    fn a_file_that_already_has_it_is_not_rewritten() {
        let done = updated("[HTTP]\nHttpSendTimeout=300\n").unwrap();
        assert!(updated(&done).is_none());
    }
}
