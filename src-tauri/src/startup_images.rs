//! The game's startup pictures, delivered with the launcher.
//!
//! `Splash.bmp` is the small window Unreal shows while the engine starts,
//! `EarlyStartupScreen.bmp` the full-screen picture after it. The community's
//! own pictures replace the original publisher's in every install:
//! it is written at each Play and after each download, compared by content,
//! so an unchanged one costs a read and nothing else. A launcher update that
//! brings a new picture puts it in place at the next Play by itself.
//!
//! The game's file list (superpeople.dev, `/api/launcher/game`) still names the
//! original files. The Download tab leaves these paths out (`is_ours`), so a
//! download, a repair or Verify files never brings the old picture back, and
//! never fetches a file the launcher writes anyway.
//!
//! The pictures live in `src-tauri/startup/`, exactly as the game reads them:
//! 24-bit BMP, the size of what they fill (830 x 400 for the splash window,
//! 1920 x 1080 for the full-screen one).

use std::path::{Path, PathBuf};

use crate::error::{LauncherError, Result};

/// Path in the game folder (as the website's list writes it), and the bytes.
const IMAGES: &[(&str, &[u8])] = &[
    ("BravoHotelGame/Content/Splash/Splash.bmp", include_bytes!("../startup/Splash.bmp")),
    (
        "BravoHotelGame/Content/EarlyStartupScreen/EarlyStartupScreen.bmp",
        include_bytes!("../startup/EarlyStartupScreen.bmp"),
    ),
];

/// A file the launcher writes itself, which the Download tab leaves out.
pub fn is_ours(path: &str) -> bool {
    IMAGES.iter().any(|(ours, _)| *ours == path)
}

fn target(root: &Path, path: &str) -> PathBuf {
    path.split('/').fold(root.to_path_buf(), |p, part| p.join(part))
}

/// Puts every picture that is not already exactly ours in place; how many
/// were written. Only into a folder that has the game (`BravoHotelGame`): a
/// wrong folder in Settings gets nothing written into it. Must run while the
/// game is closed.
pub fn apply(install_dir: &str) -> Result<usize> {
    let root = Path::new(install_dir.trim());
    if install_dir.trim().is_empty() || !root.join("BravoHotelGame").is_dir() {
        return Ok(0);
    }
    let mut written = 0;
    for (path, bytes) in IMAGES {
        let file = target(root, path);
        if std::fs::read(&file).is_ok_and(|now| now == *bytes) {
            continue;
        }
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Next to it, then over it: the game never finds half a picture.
        let tmp = file.with_extension("bmp.new");
        std::fs::write(&tmp, bytes)?;
        if let Err(e) = std::fs::rename(&tmp, &file) {
            let _ = std::fs::remove_file(&tmp);
            return Err(LauncherError::Message(format!("could not write {}: {e}", file.display())));
        }
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pictures_are_bmps_the_game_can_read() {
        for (path, bytes) in IMAGES {
            assert!(path.ends_with(".bmp"), "{path}");
            assert_eq!(&bytes[..2], b"BM", "{path}");
            let bpp = u16::from_le_bytes([bytes[28], bytes[29]]);
            assert_eq!(bpp, 24, "{path}: Unreal's splash reads 24-bit BMPs");
        }
        let size = |name: &str| {
            let bytes = IMAGES.iter().find(|(p, _)| p.ends_with(name)).unwrap().1;
            (i32::from_le_bytes(bytes[18..22].try_into().unwrap()), i32::from_le_bytes(bytes[22..26].try_into().unwrap()))
        };
        assert_eq!(size("/Splash.bmp"), (830, 400));
        assert_eq!(size("/EarlyStartupScreen.bmp"), (1920, 1080));
    }

    #[test]
    fn they_go_into_a_game_folder_once_and_nowhere_else() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().into_owned();
        assert_eq!(apply(&dir).unwrap(), 0, "not a game folder: nothing written");
        assert!(!tmp.path().join("BravoHotelGame").exists());

        // The splash is there (the publisher's), the other's folder is not.
        std::fs::create_dir_all(tmp.path().join("BravoHotelGame/Content/Splash")).unwrap();
        let splash = tmp.path().join("BravoHotelGame/Content/Splash/Splash.bmp");
        std::fs::write(&splash, b"the publisher's picture").unwrap();
        assert_eq!(apply(&dir).unwrap(), IMAGES.len());
        for (path, bytes) in IMAGES {
            assert_eq!(std::fs::read(target(tmp.path(), path)).unwrap(), *bytes, "{path}");
        }
        assert_eq!(apply(&dir).unwrap(), 0, "already ours: nothing written");
        assert!(!splash.with_extension("bmp.new").exists());
    }

    #[test]
    fn the_download_tab_leaves_them_out() {
        assert!(is_ours("BravoHotelGame/Content/Splash/Splash.bmp"));
        assert!(is_ours("BravoHotelGame/Content/EarlyStartupScreen/EarlyStartupScreen.bmp"));
        assert!(!is_ours("BravoHotelGame/Content/Paks/pakchunk0-WindowsClient.pak"));
    }
}
