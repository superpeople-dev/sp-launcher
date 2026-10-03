//! Persistent launcher settings.
//!
//! Stored as JSON next to the app's config directory, written atomically
//! (temp file + rename) so a crash mid-write cannot leave a truncated file.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{LauncherError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where the game is installed. Empty until the user picks a folder.
    pub install_dir: String,
    /// EXTRA command-line arguments, one per line as the player typed them.
    ///
    /// Only the extras. The arguments the game cannot run without live in
    /// `game::BASE_ARGS` and are prepended at launch, so they cannot be
    /// deleted here by accident. Empty by default: an empty box says "add
    /// something if you want to", where a box pre-filled with required flags
    /// invites someone to edit them.
    pub launch_args: String,
    /// The last `ip:port` typed into the connect prompt, so it's pre-filled
    /// next launch instead of starting blank every time. Empty means nothing
    /// has been entered yet (or "Play without joining server" was used last).
    pub last_server: String,

    /// Point the game's hostnames at the backend while the launcher runs.
    /// Which hostnames, and which IP, are fixed in `hosts` — they describe
    /// the one server this launcher exists to reach, so they are not the
    /// player's to get wrong.
    pub hosts_redirect: bool,
    /// Send the launcher to the tray when the game launches, instead of
    /// staying visible with the "Close Game" button.
    pub close_on_launch: bool,
    pub auto_update: bool,
    pub verify_before_launch: bool,
    pub debug_logging: bool,
    /// Deploy the client fixes DLL and signed translation PAK for this launch.
    /// Always true: the game's console lock and the login ticket
    /// need the DLL, so a saved false is overridden on load (and in set_config).
    pub client_fixes_enabled: bool,
    /// Show the client fixes DLL's diagnostic console when it is loaded.
    pub client_fixes_debug_window: bool,

    // --- Discord sign-in (see auth.rs) ---------------------------------------
    /// The website session, encrypted with DPAPI and hex-encoded: this file
    /// sits in a readable folder, and the session is a 30-day credential.
    /// Owned by the Rust side: `set_config` never takes it from the frontend.
    pub session_sealed: String,
    /// The Discord profile the session belongs to, for drawing the launcher
    /// without asking the website first. Grants nothing.
    pub profile: Option<crate::auth::Profile>,
    /// Identifies this installation to the backend. A label, not a secret --
    /// generated once, then left alone.
    pub device_id: String,
    /// The launcher version that last told the team it runs (#launcher-logs,
    /// "Launcher updated"). Empty until the first start while signed in.
    pub last_version: String,

    // --- launcher keys, retired --------------------------------------------
    /// What a launcher key sign-in left behind; cleared when Discord is
    /// connected or on sign-out.
    pub auth_key_sealed: String,
    pub account_id: String,
    pub display_name: String,
    pub key_status: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            install_dir: String::new(),
            launch_args: String::new(),
            last_server: String::new(),
            hosts_redirect: true,
            close_on_launch: false,
            auto_update: true,
            verify_before_launch: false,
            debug_logging: false,
            // On by default (0.3.2): the fixes are hash-gated to build 1.3.0.473797
            // and fail closed on any other EXE. Players can still switch it off.
            client_fixes_enabled: true,
            client_fixes_debug_window: false,
            session_sealed: String::new(),
            profile: None,
            auth_key_sealed: String::new(),
            device_id: String::new(),
            last_version: String::new(),
            account_id: String::new(),
            display_name: String::new(),
            key_status: String::new(),
        }
    }
}

pub fn config_path(base: &Path) -> PathBuf {
    base.join("config.v1.json")
}

/// Strips the arguments that used to be the default out of a saved config.
///
/// Before they moved into `game::BASE_ARGS`, every install had them sitting in
/// the settings box. Left there they would now be passed twice, and they would
/// still be visible and deletable — so an existing config is cleaned the first
/// time it is read. Anything the player added themselves is kept, in order.
fn strip_base_args(raw: &str) -> String {
    let kept: Vec<String> = super::game::parse_args(raw)
        .into_iter()
        .filter(|tok| !super::game::BASE_ARGS.iter().any(|b| b.eq_ignore_ascii_case(tok))
            // The phase is the region's (game::phase_for_region); older launchers
            // saved -ApiPhase="dev2s" here.
            && !tok.to_ascii_lowercase().starts_with("-apiphase=")
            && !tok.eq_ignore_ascii_case("-ExecCmds=\"PakFile.SearchRecentlyFoundPaks 0\""))
        .collect();
    kept.join(" ")
}

pub fn load(base: &Path) -> Config {
    let path = config_path(base);
    let mut cfg = load_raw(base, &path);
    cfg.launch_args = strip_base_args(&cfg.launch_args);
    cfg.client_fixes_enabled = true;
    cfg
}

fn load_raw(base: &Path, path: &Path) -> Config {
    let _ = base;
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            // A corrupt file must not brick the launcher: keep a copy and
            // start fresh rather than refusing to open.
            let _ = std::fs::rename(path, path.with_extension("json.corrupt"));
            eprintln!("[config] unreadable ({e}), starting from defaults");
            Config::default()
        }),
        Err(_) => Config::default(),
    }
}

pub fn save(base: &Path, cfg: &Config) -> Result<()> {
    std::fs::create_dir_all(base)?;
    let path = config_path(base);
    let tmp = path.with_extension("json.tmp");

    let text = serde_json::to_string_pretty(cfg)
        .map_err(|e| LauncherError::Config(e.to_string()))?;

    std::fs::write(&tmp, text)?;
    // rename is atomic on the same volume, so readers never see a partial file
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::strip_base_args;

    #[test]
    fn client_fixes_are_on_even_when_saved_off() {
        let dir = std::env::temp_dir().join(format!("sp-launcher-cfg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = super::Config { client_fixes_enabled: false, ..super::Config::default() };
        super::save(&dir, &cfg).unwrap();
        assert!(super::load(&dir).client_fixes_enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_old_defaults_are_removed_from_an_existing_config() {
        // Exactly what every install before this change had saved.
        assert_eq!(strip_base_args("-IgnoreCatalogue -ApiPhase=\"dev2s\""), "");
    }

    #[test]
    fn what_the_player_added_survives() {
        let got = strip_base_args("-IgnoreCatalogue -windowed -ApiPhase=\"dev2s\" -ResX=1280");
        assert_eq!(got, "-windowed -ResX=1280");
    }

    #[test]
    fn an_empty_box_stays_empty() {
        assert_eq!(strip_base_args(""), "");
        assert_eq!(strip_base_args("   \n  "), "");
    }

    #[test]
    fn nothing_of_the_players_is_touched_when_there_are_no_defaults() {
        assert_eq!(strip_base_args("-windowed -nosound"), "-windowed -nosound");
    }
}
