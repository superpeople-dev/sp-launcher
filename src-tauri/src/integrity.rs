//! Before Play: the game in the Game folder is the official one.
//!
//! WHAT IS CHECKED
//! ---------------
//! - Every file of the website's list (`/api/launcher/game`) is there, with the
//!   size and last-write time it had when the launcher last checked its SHA-256
//!   against that list: a download hashes every file it writes, Verify files
//!   every file it reads. Those checks are remembered in `game-files.v1.json`
//!   (config dir), so Play only compares sizes and times (453 files, a few
//!   milliseconds) instead of reading 30 GB. A file the launcher never hashed,
//!   or hashed against an older list, counts as not checked.
//! - Nothing extra where the game loads code and content from: no pak (`.pak`,
//!   `.sig`, `.utoc`, `.ucas`) in `BravoHotelGame/Content/Paks` or below that is
//!   not in the list, and no `.dll` or `.asi` next to the game's exe that is not
//!   in it. The launcher's own files there are allowed: the no-Steam proxy
//!   (shim.rs) and the Client fixes DLL, pak and signature
//!   (client_fixes_deployment.rs).
//!
//! When something does not match, Play refuses with what it found and the Play
//! button becomes Verify files. That run hashes every file, downloads the
//! missing and damaged ones again, and moves the extra files into
//! `<game>/.sp-removed/` (not deleted: they may be someone's mods), after which
//! Play works again.
//!
//! The startup pictures (startup_images.rs) are the launcher's own and are
//! left out of the list, as for the download.
//!
//! The DLSS, Frame Generation and XeSS libraries (upscalers.rs) may also be a
//! build the player swapped in with their own tool: one the launcher
//! recognises (catalogue hash and vendor signature) is accepted and remembered
//! here, one it does not stops Play with a message of its own.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::upscalers;

const RECORD_FILE: &str = "game-files.v1.json";
/// Where Verify files moves the extra files to, in the game folder.
pub const REMOVED_DIR: &str = ".sp-removed";
const PAKS_DIR: &str = "BravoHotelGame/Content/Paks";
const EXE_DIR: &str = "BravoHotelGame/Binaries/Win64";
const PAK_EXTENSIONS: &[&str] = &["pak", "sig", "utoc", "ucas"];
const CODE_EXTENSIONS: &[&str] = &["dll", "asi"];

/// A file of the website's list.
#[derive(Debug, Clone)]
pub struct Official {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// A file the launcher hashed and found to be the official one, as it was then.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Checked {
    pub path: String,
    pub size: u64,
    /// Last write, in milliseconds since 1970.
    pub modified: u64,
    pub sha256: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Record {
    /// The game folder these files are in.
    root: String,
    files: Vec<Checked>,
    /// Upscaler swaps recognised as they are now (upscalers.rs), so Play does not read them again.
    accepted: Vec<Checked>,
}

/// What Play found, for the UI.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Report {
    /// Not there, or not the size of the official file.
    pub missing: Vec<String>,
    /// Changed since the launcher found it to be the official file.
    pub changed: Vec<String>,
    /// Never checked against the current list (a first Play after this check
    /// arrived, a game update, another Game folder).
    pub unchecked: usize,
    /// Paks and DLLs that are not part of the game.
    pub extra: Vec<String>,
    /// DLSS / XeSS libraries swapped for a build the launcher does not recognise.
    pub replaced: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.missing.is_empty() && self.changed.is_empty() && self.unchecked == 0 && self.extra.is_empty() && self.replaced.is_empty()
    }

    /// What the player reads when Play is refused.
    pub fn message(&self) -> String {
        let damaged = self.missing.len() + self.changed.len();
        let mut parts = Vec::new();
        if damaged > 0 {
            parts.push(match damaged {
                1 => "1 game file is missing or does not match the official one".to_string(),
                n => format!("{n} game files are missing or do not match the official ones"),
            });
        }
        if !self.extra.is_empty() {
            parts.push(match self.extra.len() {
                1 => format!("1 file in the game folder is not part of the game ({})", self.extra[0]),
                n => format!("{n} files in the game folder are not part of the game (mods or other paks)"),
            });
        }
        if !self.replaced.is_empty() {
            let swaps = upscalers::unrecognised_message(&self.replaced);
            if parts.is_empty() {
                return format!("The game cannot start: {swaps}.");
            }
            parts.push(swaps);
        }
        if parts.is_empty() && self.unchecked > 0 {
            return "The game's files have not been checked yet. Press Verify files once (it reads the whole game, a few minutes), then Play.".into();
        }
        format!("The game cannot start: {}. Press Verify files to repair it, then Play.", parts.join(", and "))
    }
}

fn stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
    Some((meta.len(), modified))
}

fn target(root: &Path, path: &str) -> PathBuf {
    path.split('/').fold(root.to_path_buf(), |p, part| p.join(part))
}

fn key(root: &Path) -> String {
    root.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase()
}

fn load(config_dir: &Path) -> Record {
    std::fs::read(config_dir.join(RECORD_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(config_dir: &Path, record: &Record) -> Result<()> {
    std::fs::create_dir_all(config_dir)?;
    let tmp = config_dir.join(format!("{RECORD_FILE}.new"));
    std::fs::write(&tmp, serde_json::to_vec(record).map_err(|e| e.to_string())?)?;
    std::fs::rename(tmp, config_dir.join(RECORD_FILE))?;
    Ok(())
}

/// A file the launcher just hashed and found to be `sha256`, as it is now on
/// disk; None when it cannot be read.
pub fn checked(root: &Path, path: &str, sha256: &str) -> Option<Checked> {
    let (size, modified) = stamp(&target(root, path))?;
    Some(Checked { path: path.to_string(), size, modified, sha256: sha256.to_string() })
}

/// Remember files the launcher hashed in this game folder (a download, Verify
/// files). Another folder's record is replaced.
pub fn remember(config_dir: &Path, root: &Path, files: impl IntoIterator<Item = Checked>) -> Result<()> {
    let mut record = load(config_dir);
    if record.root != key(root) {
        record = Record { root: key(root), ..Record::default() };
    }
    let mut by_path: HashMap<String, Checked> = record.files.into_iter().map(|f| (f.path.to_lowercase(), f)).collect();
    for f in files {
        by_path.insert(f.path.to_lowercase(), f);
    }
    let mut files: Vec<Checked> = by_path.into_values().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    save(config_dir, &Record { root: key(root), files, accepted: record.accepted })
}

/// The launcher's own files in the folders that are checked for extras.
fn ours(relative_lower: &str) -> bool {
    let own = [
        crate::client_fixes_deployment::DLL_PATH,
        crate::client_fixes_deployment::PAK_PATH,
        crate::client_fixes_deployment::SIG_PATH,
        "BravoHotelGame/Binaries/Win64/XAPOFX1_5.dll",
    ];
    own.iter().any(|p| p.to_lowercase() == relative_lower)
}

/// Paks and DLLs where the game would load them that are neither in the list
/// nor the launcher's own, relative with `/`.
pub fn extras(root: &Path, official: &[Official]) -> Vec<String> {
    let listed: HashSet<String> = official.iter().map(|f| f.path.to_lowercase()).collect();
    let mut found = Vec::new();
    let mut walk = vec![(root.join("BravoHotelGame").join("Content").join("Paks"), PAKS_DIR.to_string(), true)];
    walk.push((target(root, EXE_DIR), EXE_DIR.to_string(), false));
    while let Some((dir, rel, deep)) = walk.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = format!("{rel}/{name}");
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if deep {
                    walk.push((entry.path(), relative, true));
                }
                continue;
            }
            let ext = Path::new(&name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let watched = if deep { PAK_EXTENSIONS } else { CODE_EXTENSIONS };
            let lower = relative.to_lowercase();
            if watched.contains(&ext.as_str()) && !listed.contains(&lower) && !ours(&lower) {
                found.push(relative);
            }
        }
    }
    found.sort();
    found
}

/// Compare the game folder with the list and with what the launcher checked.
pub fn check(config_dir: &Path, root: &Path, official: &[Official]) -> Report {
    let record = load(config_dir);
    let same_root = record.root == key(root);
    let known: HashMap<String, &Checked> = if same_root {
        record.files.iter().map(|f| (f.path.to_lowercase(), f)).collect()
    } else {
        HashMap::new()
    };
    let mut report = Report { extra: extras(root, official), ..Report::default() };
    let mut accepted: Vec<Checked> = Vec::new();
    for f in official {
        // A DLSS / XeSS library that is not the game's own as last checked: the player's swap
        // when the launcher recognises it, otherwise refused when its size is not the game's.
        if let Some(library) = upscalers::slot(&f.path) {
            if let Some((size, modified)) = stamp(&target(root, &f.path)) {
                let own = size == f.size
                    && known.get(&f.path.to_lowercase()).is_some_and(|c| c.sha256 == f.sha256 && (c.size, c.modified) == (size, modified));
                if !own {
                    let earlier = record.accepted.iter().find(|a| same_root && a.path.eq_ignore_ascii_case(&f.path) && (a.size, a.modified) == (size, modified));
                    if let Some(a) = earlier {
                        accepted.push(a.clone());
                        continue;
                    }
                    if let Some(version) = upscalers::recognise(&target(root, &f.path), library) {
                        accepted.push(Checked { path: f.path.clone(), size, modified, sha256: format!("swap: {} {version}", library.name()) });
                        continue;
                    }
                    if size != f.size {
                        report.replaced.push(f.path.clone());
                        continue;
                    }
                }
            }
        }
        match stamp(&target(root, &f.path)) {
            None => report.missing.push(f.path.clone()),
            Some((size, _)) if size != f.size => report.missing.push(f.path.clone()),
            Some((size, modified)) => match known.get(&f.path.to_lowercase()) {
                Some(c) if c.sha256 == f.sha256 => {
                    if (c.size, c.modified) != (size, modified) {
                        report.changed.push(f.path.clone());
                    }
                }
                _ => report.unchecked += 1,
            },
        }
    }
    if same_root && accepted != record.accepted {
        let mut record = record;
        record.accepted = accepted;
        if let Err(e) = save(config_dir, &record) {
            eprintln!("[integrity] {e}");
        }
    }
    report
}

/// Verify files: the extra paks and DLLs go to `<game>/.sp-removed/`, under
/// their own folders, where the game does not look. Returns what was moved.
pub fn move_extras(root: &Path, official: &[Official]) -> Result<Vec<String>> {
    let found = extras(root, official);
    for relative in &found {
        let from = target(root, relative);
        let mut to = target(&root.join(REMOVED_DIR), relative);
        if to.exists() {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            to.set_file_name(format!("{}.{stamp}", to.file_name().unwrap_or_default().to_string_lossy()));
        }
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::rename(&from, &to)?;
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> (tempfile::TempDir, tempfile::TempDir, Vec<Official>) {
        let root = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let mut official = Vec::new();
        for (path, body) in [
            ("BravoHotelClient.exe", &b"root exe"[..]),
            ("BravoHotelGame/Binaries/Win64/BravoHotelClient-Win64-Shipping.exe", b"game exe"),
            ("BravoHotelGame/Content/Paks/pakchunk0-WindowsClient.pak", b"pak zero"),
            ("BravoHotelGame/Content/Paks/pakchunk0-WindowsClient.sig", b"sig zero"),
        ] {
            let file = target(root.path(), path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, body).unwrap();
            official.push(Official { path: path.into(), size: body.len() as u64, sha256: format!("{:0>64}", body.len()) });
        }
        (root, config, official)
    }

    fn remember_all(config: &Path, root: &Path, official: &[Official]) {
        remember(config, root, official.iter().map(|f| checked(root, &f.path, &f.sha256).unwrap())).unwrap();
    }

    #[test]
    fn a_game_never_checked_must_be_verified_first() {
        let (root, config, official) = game();
        let report = check(config.path(), root.path(), &official);
        assert_eq!(report.unchecked, 4);
        assert!(!report.ok());
        assert!(report.message().contains("Verify files"));
        remember_all(config.path(), root.path(), &official);
        assert!(check(config.path(), root.path(), &official).ok());
    }

    #[test]
    fn a_changed_or_missing_file_stops_play() {
        let (root, config, official) = game();
        remember_all(config.path(), root.path(), &official);
        let pak = target(root.path(), &official[2].path);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&pak, b"pak 0ero").unwrap(); // same size, other bytes
        std::fs::remove_file(target(root.path(), &official[0].path)).unwrap();
        let report = check(config.path(), root.path(), &official);
        assert_eq!(report.changed, vec![official[2].path.clone()]);
        assert_eq!(report.missing, vec![official[0].path.clone()]);
        assert!(report.message().contains("2 game files are missing or do not match"));
    }

    #[test]
    fn a_game_update_or_another_folder_needs_a_new_check() {
        let (root, config, mut official) = game();
        remember_all(config.path(), root.path(), &official);
        official[3].sha256 = "f".repeat(64); // the list changed
        assert_eq!(check(config.path(), root.path(), &official).unchecked, 1);
        let (other, _, other_list) = game();
        assert_eq!(check(config.path(), other.path(), &other_list).unchecked, 4);
    }

    #[test]
    fn extra_paks_and_dlls_are_found_but_the_launchers_own_are_not() {
        let (root, config, official) = game();
        remember_all(config.path(), root.path(), &official);
        for path in [
            "BravoHotelGame/Content/Paks/~mods/cheat_P.pak",
            "BravoHotelGame/Content/Paks/zz-mod_P.pak",
            "BravoHotelGame/Binaries/Win64/dxgi.dll",
            "BravoHotelGame/Binaries/Win64/plugin.asi",
            // Not checked: not code, or not where the game loads from.
            "BravoHotelGame/Binaries/Win64/sp_client.log",
            "BravoHotelGame/Content/Movies/extra.pak",
            // The launcher's own.
            crate::client_fixes_deployment::PAK_PATH,
            crate::client_fixes_deployment::SIG_PATH,
            crate::client_fixes_deployment::DLL_PATH,
            "BravoHotelGame/Binaries/Win64/XAPOFX1_5.dll",
        ] {
            let file = target(root.path(), path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, b"x").unwrap();
        }
        let report = check(config.path(), root.path(), &official);
        assert_eq!(
            report.extra,
            vec![
                "BravoHotelGame/Binaries/Win64/dxgi.dll",
                "BravoHotelGame/Binaries/Win64/plugin.asi",
                "BravoHotelGame/Content/Paks/zz-mod_P.pak",
                "BravoHotelGame/Content/Paks/~mods/cheat_P.pak",
            ]
        );
        assert!(report.message().contains("4 files in the game folder are not part of the game"));
    }

    const XESS: &str = "Engine/Plugins/Runtime/Intel/XeSS/Binaries/ThirdParty/Win64/libxess.dll";

    fn with_xess(root: &Path, official: &mut Vec<Official>, body: &[u8]) {
        let file = target(root, XESS);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, body).unwrap();
        official.push(Official { path: XESS.into(), size: body.len() as u64, sha256: "e".repeat(64) });
    }

    #[test]
    fn an_upscaler_swapped_for_an_unknown_build_stops_play_with_its_own_message() {
        let (root, config, mut official) = game();
        with_xess(root.path(), &mut official, b"the game's xess");
        remember_all(config.path(), root.path(), &official);
        assert!(check(config.path(), root.path(), &official).ok());
        std::fs::write(target(root.path(), XESS), b"some other xess build, not in the catalogue").unwrap();
        let report = check(config.path(), root.path(), &official);
        assert_eq!(report.replaced, vec![XESS.to_string()]);
        assert!(report.missing.is_empty() && report.changed.is_empty());
        let message = report.message();
        assert!(message.contains("libxess.dll (XeSS) was replaced") && message.contains("DLSS Swapper"), "{message}");
        // The game's size with other bytes: an ordinary changed file, as before.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(target(root.path(), XESS), b"the game's xesz").unwrap();
        let report = check(config.path(), root.path(), &official);
        assert!(report.replaced.is_empty() && report.changed == vec![XESS.to_string()]);
    }

    /// With SP_TEST_GAME_DIR set to an installed game: its own XeSS and Frame Generation builds are
    /// in the catalogue and signed by Intel and NVIDIA, so they pass as a player's swap.
    #[test]
    fn a_recognised_signed_swap_is_accepted_and_remembered() {
        let Some(game_dir) = std::env::var_os("SP_TEST_GAME_DIR") else { return };
        let real = |p: &str| std::fs::read(target(Path::new(&game_dir), p)).unwrap();
        let (root, config, mut official) = game();
        with_xess(root.path(), &mut official, b"the game's xess");
        remember_all(config.path(), root.path(), &official);
        std::fs::write(target(root.path(), XESS), real(XESS)).unwrap();
        let report = check(config.path(), root.path(), &official);
        assert!(report.ok(), "{report:?}");
        let record = load(config.path());
        assert_eq!(record.accepted.len(), 1);
        assert_eq!(record.accepted[0].sha256, "swap: XeSS 1.0.1.12");
        // Remembered: the next Play does not read it again, and still accepts it.
        assert!(check(config.path(), root.path(), &official).ok());
        // The right name and size but not the vendor's file: not accepted.
        let mut forged = real(XESS);
        let last = forged.len() - 1;
        forged[last] ^= 0xFF;
        std::fs::write(target(root.path(), XESS), forged).unwrap();
        assert_eq!(check(config.path(), root.path(), &official).replaced, vec![XESS.to_string()]);
        // A Frame Generation build in the XeSS slot is not XeSS.
        let dlssg = real("Engine/Plugins/Runtime/Nvidia/Streamline/Binaries/ThirdParty/Win64/nvngx_dlssg.dll");
        std::fs::write(target(root.path(), XESS), dlssg).unwrap();
        assert_eq!(check(config.path(), root.path(), &official).replaced, vec![XESS.to_string()]);
    }

    #[test]
    fn verify_moves_the_extras_aside_without_deleting_them() {
        let (root, config, official) = game();
        remember_all(config.path(), root.path(), &official);
        let mod_pak = target(root.path(), "BravoHotelGame/Content/Paks/~mods/cheat_P.pak");
        std::fs::create_dir_all(mod_pak.parent().unwrap()).unwrap();
        std::fs::write(&mod_pak, b"mod").unwrap();
        let moved = move_extras(root.path(), &official).unwrap();
        assert_eq!(moved, vec!["BravoHotelGame/Content/Paks/~mods/cheat_P.pak"]);
        assert!(!mod_pak.exists());
        assert_eq!(std::fs::read(root.path().join(REMOVED_DIR).join("BravoHotelGame/Content/Paks/~mods/cheat_P.pak")).unwrap(), b"mod");
        assert!(check(config.path(), root.path(), &official).ok());
    }
}
