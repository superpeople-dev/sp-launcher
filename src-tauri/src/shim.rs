//! Delivering the no-Steam DLL with the launcher.
//!
//! WHY THIS EXISTS
//! ---------------
//! The game asks Unreal for the Steam online subsystem at startup. Without
//! `XAPOFX1_5.dll` sitting next to the executable to intercept that and hand
//! back the default one instead, the client waits forever for a Steam that
//! isn't running — the loading screen that never ends. It is not optional and
//! it is not a tweak: it is the thing that makes the game start at all.
//!
//! Until now players had to place it by hand, which is fine for one person and
//! impossible for a community.
//!
//! WHY EMBEDDED RATHER THAN SHIPPED BESIDE THE EXE
//! ----------------------------------------------
//! Tauri can bundle extra files as resources, and the updater would carry them.
//! But that gives two artifacts that can drift apart, and a player whose
//! antivirus quarantines the loose DLL ends up with a launcher that looks
//! healthy and a game that silently has no fix. Embedded in the binary, the DLL
//! cannot be out of date, cannot be half-installed, and needs no bundle config.
//! It costs ~150 KB in the executable.
//!
//! WHAT IT TOUCHES
//! ---------------
//! Only `<install_dir>/BravoHotelGame/Binaries/Win64/XAPOFX1_5.dll` — the
//! player's own client folder. A listen-server host keeps its own arrangement
//! (`dxgi.dll` loading `sp_listen.dll` from a different install), and the
//! launcher has no idea that folder exists, so hosts are untouched.

use std::path::{Path, PathBuf};

use crate::error::{LauncherError, Result};

/// Set by build.rs when `resources/XAPOFX1_5.dll` is actually present, so a
/// checkout without the binary still compiles instead of failing on a missing
/// `include_bytes!`. An empty payload means "nothing to install", which
/// `apply` reports rather than silently doing nothing.
#[cfg(has_shim)]
const SHIM: &[u8] = include_bytes!("../resources/XAPOFX1_5.dll");
#[cfg(not(has_shim))]
const SHIM: &[u8] = &[];

/// Where the DLL has to land, relative to the install directory. The name is
/// not arbitrary: the proxy forwards `CreateFX` to the real XAPOFX1_5, chosen
/// because the game imports exactly one function from it and nothing else in
/// the process depends on it. (An earlier attempt used `dxgi.dll` and broke
/// the renderer, because a dxgi.dll next to the exe shadows the system one for
/// the WHOLE process, not just for our hook.)
const REL_PATH: &[&str] = &["BravoHotelGame", "Binaries", "Win64", "XAPOFX1_5.dll"];

/// Whether this launcher carries the DLL at all (a checkout without the binary does not).
pub fn is_bundled() -> bool {
    !SHIM.is_empty()
}

pub fn target_path(install_dir: &Path) -> PathBuf {
    let mut p = install_dir.to_path_buf();
    for part in REL_PATH {
        p.push(part);
    }
    p
}

/// What `apply` did, so the caller can log it without guessing.
#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    /// Already byte-identical; nothing was written.
    UpToDate,
    /// Written for the first time.
    Installed,
    /// Replaced a different build.
    Updated,
    /// No DLL was compiled into this launcher.
    NotBundled,
}

/// Put the DLL in place if what is there is not already exactly this one.
///
/// Comparing CONTENT rather than keeping a version file means an update that
/// changes the DLL replaces it by itself, and a launch that changes nothing
/// touches the disk not at all. It also means a player who placed a different
/// build by hand gets ours — correct, since the launcher is the thing that
/// knows which DLL matches the arguments it passes.
///
/// Must run BEFORE the game starts: Windows holds a loaded DLL open, and the
/// write would fail with a sharing violation.
pub fn apply(install_dir: &str) -> Result<Applied> {
    if SHIM.is_empty() {
        return Ok(Applied::NotBundled);
    }
    if install_dir.is_empty() {
        return Err(LauncherError::Message("no install directory set".into()));
    }

    let path = target_path(Path::new(install_dir));
    let existing = std::fs::read(&path).ok();

    match existing {
        Some(bytes) if bytes == SHIM => Ok(Applied::UpToDate),
        other => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            write_atomically(&path, SHIM)?;
            Ok(if other.is_some() { Applied::Updated } else { Applied::Installed })
        }
    }
}

/// Write to a temporary file next to the target, then rename over it. A
/// half-written DLL is worse than no DLL: the loader would fail in a way that
/// looks nothing like "the file is missing".
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("dll.new");
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Rename over an existing file can fail on Windows if something has
            // it open — most likely the game is still running.
            let _ = std::fs::remove_file(&tmp);
            Err(LauncherError::Message(format!(
                "could not write {}: {e}. Close the game and try again.",
                path.display()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_path_is_next_to_the_game_executable() {
        let p = target_path(Path::new("C:/games/SUPER PEOPLE"));
        let s = p.to_string_lossy().replace('\\', "/");
        assert!(s.ends_with("SUPER PEOPLE/BravoHotelGame/Binaries/Win64/XAPOFX1_5.dll"), "{s}");
    }

    #[test]
    fn an_empty_install_dir_is_an_error_not_a_panic() {
        if SHIM.is_empty() {
            // Without a bundled DLL the function returns before looking at the
            // directory, which is the correct order: nothing to install beats
            // complaining about where to install it.
            assert_eq!(apply("").unwrap(), Applied::NotBundled);
        } else {
            assert!(apply("").is_err());
        }
    }

    // The install/update/no-op decision is the part worth testing, and it does
    // not need a real DLL — exercise it against a payload we control.
    fn decide(existing: Option<&[u8]>, payload: &[u8]) -> Applied {
        match existing {
            Some(b) if b == payload => Applied::UpToDate,
            Some(_) => Applied::Updated,
            None => Applied::Installed,
        }
    }

    #[test]
    fn an_identical_file_is_left_alone() {
        assert_eq!(decide(Some(b"abc"), b"abc"), Applied::UpToDate);
    }

    #[test]
    fn a_different_file_is_replaced() {
        assert_eq!(decide(Some(b"old build"), b"new build"), Applied::Updated);
    }

    #[test]
    fn a_missing_file_is_installed() {
        assert_eq!(decide(None, b"anything"), Applied::Installed);
    }

    #[test]
    fn writing_is_atomic_and_leaves_no_temp_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("XAPOFX1_5.dll");
        write_atomically(&target, b"payload").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"payload");
        assert!(!dir.path().join("XAPOFX1_5.dll.new").exists(), "temp file was left behind");
    }

    // Only meaningful in a build that actually embedded a DLL. Exercises the
    // real decision path end to end against a temporary install directory,
    // rather than the `decide` stand-in above.
    #[cfg(has_shim)]
    #[test]
    fn apply_installs_then_leaves_alone_then_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().to_string_lossy().into_owned();

        assert_eq!(apply(&install).unwrap(), Applied::Installed);
        let path = target_path(dir.path());
        assert!(path.is_file(), "the DLL was not written");
        assert_eq!(std::fs::read(&path).unwrap(), SHIM);

        // Second call must not touch the disk.
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(apply(&install).unwrap(), Applied::UpToDate);
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), before);

        // A different build in place gets replaced.
        std::fs::write(&path, b"some other build").unwrap();
        assert_eq!(apply(&install).unwrap(), Applied::Updated);
        assert_eq!(std::fs::read(&path).unwrap(), SHIM);
    }

    #[test]
    fn writing_over_an_existing_file_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("XAPOFX1_5.dll");
        std::fs::write(&target, b"old").unwrap();
        write_atomically(&target, b"new").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
    }
}
