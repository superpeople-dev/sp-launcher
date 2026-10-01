//! The game's startup pictures, delivered with the launcher.
//!
//! `Splash.bmp` is the small window Unreal shows while the engine starts,
//! `EarlyStartupScreen.bmp` the full-screen picture after it, and
//! `Loading_Scene.mp4` the video the title screen loops while the game signs
//! in and opens the lobby (the same picture as the loading screen, with its
//! three dots). The community's own files replace the original publisher's in
//! every install:
//! it is written at each Play and after each download, compared by content,
//! so an unchanged one costs a read and nothing else. A launcher update that
//! brings a new picture puts it in place at the next Play by itself.
//!
//! The game's file list (superpeople.dev, `/api/launcher/game`) still names the
//! original files. The Download tab leaves these paths out (`is_ours`), so a
//! download, a repair or Verify files never brings the old picture back, and
//! never fetches a file the launcher writes anyway.
//!
//! The files live in `src-tauri/startup/`, exactly as the game reads them:
//! 24-bit BMP, the size of what they fill (830 x 400 for the splash window,
//! 1920 x 1080 for the full-screen one), and an MP4 the engine's media player
//! opens.

use std::path::{Path, PathBuf};

use crate::error::{LauncherError, Result};

/// Path in the game folder (as the website's list writes it), and the bytes.
const IMAGES: &[(&str, &[u8])] = &[
    ("BravoHotelGame/Content/Splash/Splash.bmp", include_bytes!("../startup/Splash.bmp")),
    (
        "BravoHotelGame/Content/EarlyStartupScreen/EarlyStartupScreen.bmp",
        include_bytes!("../startup/EarlyStartupScreen.bmp"),
    ),
    ("BravoHotelGame/Content/Movies/Loading_Scene.mp4", include_bytes!("../startup/Loading_Scene.mp4")),
];

/// A file the launcher writes itself, which the Download tab leaves out.
pub fn is_ours(path: &str) -> bool {
    IMAGES.iter().any(|(ours, _)| *ours == path)
}

fn target(root: &Path, path: &str) -> PathBuf {
    path.split('/').fold(root.to_path_buf(), |p, part| p.join(part))
}

fn tmp_of(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(".new");
    file.with_file_name(name)
}

/// Puts every file that is not already exactly ours in place; how many
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
        // Next to it, then over it: the game never finds half a file.
        let tmp = tmp_of(&file);
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
    fn the_files_are_ones_the_game_can_read() {
        for (path, bytes) in IMAGES.iter().filter(|(p, _)| !p.ends_with(".bmp")) {
            assert!(path.ends_with(".mp4"), "{path}");
            assert_eq!(&bytes[4..8], b"ftyp", "{path}: an MP4 starts with its ftyp box");
        }
        for (path, bytes) in IMAGES.iter().filter(|(p, _)| p.ends_with(".bmp")) {
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
        for (path, _) in IMAGES {
            assert!(!tmp_of(&target(tmp.path(), path)).exists(), "{path}");
        }
    }

    #[test]
    fn the_download_tab_leaves_them_out() {
        assert!(is_ours("BravoHotelGame/Content/Splash/Splash.bmp"));
        assert!(is_ours("BravoHotelGame/Content/EarlyStartupScreen/EarlyStartupScreen.bmp"));
        assert!(is_ours("BravoHotelGame/Content/Movies/Loading_Scene.mp4"));
        assert!(!is_ours("BravoHotelGame/Content/Movies/ClassVideos/Loading_Scene.mp4"));
        assert!(!is_ours("BravoHotelGame/Content/Paks/pakchunk0-WindowsClient.pak"));
    }
}
