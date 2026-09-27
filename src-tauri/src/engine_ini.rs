//! The game's user `Engine.ini`.
//!
//! The private backend doesn't present a certificate the client will accept,
//! so `n.VerifyPeer` has to be off or its HTTPS calls are refused. It's a UE4
//! console variable read from config during startup — passing it on the
//! command line does not work — so the launcher writes it into the player's
//! config before every launch, for the same reason the hosts entries go in
//! first: the client reads both while starting, not when it connects.
//!
//! Writing it every launch (rather than once) is deliberate: UE rewrites this
//! file itself when the game exits and can drop the entry, so "set it once"
//! silently stops being true.
//!
//! This is the player's own file, so the edit is surgical — only this one key
//! in this one section is touched, and every other line survives as-is.

use std::path::{Path, PathBuf};

use crate::error::{LauncherError, Result};

pub const SECTION: &str = "[/Script/Engine.NetworkSettings]";
pub const KEY: &str = "n.VerifyPeer";
pub const VALUE: &str = "False";

/// A packaged UE game keeps the player's config under LOCALAPPDATA, not in
/// the install folder, keyed by the project name (the `BravoHotelGame` folder
/// that sits next to the executable).
const PROJECT: &str = "BravoHotelGame";

// ----------------------------------------------------------- pure editing ---

/// Keep the file's existing line ending so a CRLF ini stays CRLF.
fn line_ending(content: &str) -> &'static str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// True for an uncommented `key = value` line naming `key`. Commented-out
/// lines are left alone rather than revived, so a player who deliberately
/// disabled something keeps their comment and gets our line added separately.
fn is_key_line(trimmed: &str, key: &str) -> bool {
    if trimmed.starts_with(';') || trimmed.starts_with('#') {
        return false;
    }
    match trimmed.split_once('=') {
        Some((k, _)) => k.trim().eq_ignore_ascii_case(key),
        None => false,
    }
}

fn is_section_header(trimmed: &str) -> bool {
    trimmed.starts_with('[') && trimmed.ends_with(']')
}

/// Sets `key` to `value` inside `section`, leaving everything else untouched.
/// Adds the key to an existing section, or the whole section when it's
/// missing, and returns the content unchanged when it already says the right
/// thing — so a launch that changes nothing rewrites nothing.
pub fn with_setting(content: &str, section: &str, key: &str, value: &str) -> String {
    let nl = line_ending(content);
    let target = format!("{key}={value}");

    let mut out: Vec<String> = Vec::new();
    let mut in_section = false;
    let mut seen_section = false;
    let mut wrote = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if is_section_header(trimmed) {
            // Leaving our section without having written the key means it
            // wasn't there — add it before the next section starts.
            if in_section && !wrote {
                out.push(target.clone());
                wrote = true;
            }
            in_section = trimmed.eq_ignore_ascii_case(section);
            if in_section {
                seen_section = true;
            }
            out.push(line.to_string());
            continue;
        }

        if in_section && is_key_line(trimmed, key) {
            // Replace the first occurrence and drop any later duplicates: a
            // second copy further down the section would win over ours.
            if !wrote {
                out.push(target.clone());
                wrote = true;
            }
            continue;
        }

        out.push(line.to_string());
    }

    // The file ended while still inside our section.
    if in_section && !wrote {
        out.push(target.clone());
    }

    if !seen_section {
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push(section.to_string());
        out.push(target);
    }

    let mut text = out.join(nl);
    if !text.is_empty() {
        text.push_str(nl);
    }
    text
}

// --------------------------------------------------------------- file i/o ---

/// Where the player's `Engine.ini` lives.
pub fn config_path() -> Result<PathBuf> {
    let base = std::env::var("LOCALAPPDATA")
        .map_err(|_| LauncherError::Message("LOCALAPPDATA is not set".into()))?;
    let saved = Path::new(&base).join(PROJECT).join("Saved").join("Config");

    // This build uses WindowsClient (a client-only packaged target); the
    // other two are the stock UE4/UE5 names, kept as fallbacks in case a
    // differently packaged build turns up. Prefer whichever folder the game
    // has actually created, so we never write a second copy it ignores.
    for platform in ["WindowsClient", "WindowsNoEditor", "Windows"] {
        let dir = saved.join(platform);
        if dir.is_dir() {
            return Ok(dir.join("Engine.ini"));
        }
    }
    Ok(saved.join("WindowsClient").join("Engine.ini"))
}

/// Remove the temporary investigation setting, retaining unrelated config.
fn without_temporary_pak_setting(content: &str) -> String {
    let mut system_settings = false;
    content.split_inclusive('\n').filter(|line| {
        let trimmed = line.trim();
        if is_section_header(trimmed) {
            system_settings = trimmed.eq_ignore_ascii_case("[SystemSettings]");
        }
        !(system_settings && is_key_line(trimmed, "PakFile.SearchRecentlyFoundPaks")
            && trimmed.split_once('=').map(|(_,v)| v.trim() == "0").unwrap_or(false))
    }).collect()
}

/// Ensure the network setting is present and retire the temporary PAK setting.
pub fn apply() -> Result<PathBuf> {
    let path = config_path()?;
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let cleaned = without_temporary_pak_setting(&existing);
    let updated = with_setting(&cleaned, SECTION, KEY, VALUE);

    if updated != existing {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, updated)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_the_temporary_system_settings_value() {
        let before = "[SystemSettings]\r\n;PakFile.SearchRecentlyFoundPaks=0\r\nPakFile.SearchRecentlyFoundPaks=0\r\nOther=7\r\n[Other]\r\nPakFile.SearchRecentlyFoundPaks=0\r\n";
        let expected = "[SystemSettings]\r\n;PakFile.SearchRecentlyFoundPaks=0\r\nOther=7\r\n[Other]\r\nPakFile.SearchRecentlyFoundPaks=0\r\n";
        assert_eq!(without_temporary_pak_setting(before), expected);
        let intentional = "[SystemSettings]\nPakFile.SearchRecentlyFoundPaks=1\n";
        assert_eq!(without_temporary_pak_setting(intentional), intentional);
    }

    fn set(content: &str) -> String {
        with_setting(content, SECTION, KEY, VALUE)
    }

    #[test]
    fn writes_section_and_key_into_an_empty_file() {
        assert_eq!(set(""), format!("{SECTION}\n{KEY}={VALUE}\n"));
    }

    #[test]
    fn adds_the_key_to_an_existing_section() {
        let before = "[/Script/Engine.NetworkSettings]\nn.Something=1\n";
        let after = set(before);
        assert!(after.contains("n.VerifyPeer=False"));
        assert!(after.contains("n.Something=1"), "existing keys survive");
        assert_eq!(after.matches('[').count(), 1, "no duplicate section");
    }

    #[test]
    fn replaces_a_wrong_value_in_place() {
        let after = set("[/Script/Engine.NetworkSettings]\nn.VerifyPeer=True\n");
        assert!(after.contains("n.VerifyPeer=False"));
        assert!(!after.contains("True"));
    }

    #[test]
    fn drops_a_duplicate_that_would_override_ours() {
        let after = set("[/Script/Engine.NetworkSettings]\nn.VerifyPeer=True\nn.VerifyPeer=True\n");
        assert_eq!(after.matches("n.VerifyPeer").count(), 1);
    }

    #[test]
    fn leaves_other_sections_alone_and_inserts_before_the_next_one() {
        let before = "[Core.System]\nPaths=../../../Engine\n\n[/Script/Engine.NetworkSettings]\n\n[Other]\nX=1\n";
        let after = set(before);
        assert!(after.contains("Paths=../../../Engine"));
        assert!(after.contains("[Other]"));
        assert!(after.contains("X=1"));
        let verify = after.find("n.VerifyPeer").expect("key written");
        let other = after.find("[Other]").expect("section kept");
        assert!(verify < other, "the key lands inside its own section");
    }

    #[test]
    fn appends_the_section_when_the_file_has_others() {
        let after = set("[Core.System]\nPaths=x\n");
        assert!(after.contains("[Core.System]"));
        assert!(after.ends_with(&format!("{SECTION}\n{KEY}={VALUE}\n")));
    }

    #[test]
    fn is_idempotent() {
        let once = set("");
        assert_eq!(set(&once), once, "a second pass changes nothing");
    }

    #[test]
    fn keeps_crlf_files_crlf() {
        let after = set("[Core.System]\r\nPaths=x\r\n");
        assert!(after.contains("\r\n"));
        assert!(!after.replace("\r\n", "").contains('\n'), "no bare LF mixed in");
    }

    #[test]
    fn a_commented_out_key_is_not_treated_as_the_setting() {
        let after = set("[/Script/Engine.NetworkSettings]\n;n.VerifyPeer=True\n");
        assert!(after.contains(";n.VerifyPeer=True"), "the comment is left as-is");
        assert!(after.contains("\nn.VerifyPeer=False"), "and the real key is added");
    }
}
