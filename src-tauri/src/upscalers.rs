//! The DLSS, DLSS Frame Generation and XeSS libraries a player may swap.
//!
//! WHY
//! ---
//! The game ships DLSS 2.4.12, Frame Generation 1.0.2 and XeSS 1.0.1. Players replace them with
//! newer builds using tools such as DLSS Swapper, and that works with this game build. Play
//! refused such a folder (a changed file) and Verify files put the old library back, so a player
//! could not keep an upgrade.
//!
//! WHAT IS ACCEPTED
//! ----------------
//! Only at the five paths in `SLOTS` (the libraries and the Development copies next to two of
//! them); every other file of the game is checked as before. There, a file that is not the
//! game's own counts as the player's swap when all of these hold:
//! - its size and MD5 are those of a build in `resources/upscalers.json` for that library: the
//!   builds DLSS Swapper's catalogue lists with a valid signature (`tools/update-upscalers.mjs`
//!   regenerates it);
//! - it has a valid Authenticode signature from that library's vendor (NVIDIA Corporation for
//!   DLSS, Intel Corporation for XeSS), so a file that only has the right name is not enough.
//!
//! Being recognised says what the file is, not that this game build works with every version.
//! Downloading, installing, backing up and restoring stay with the player's tool: the launcher
//! neither fetches these libraries nor puts a swap back.

use std::path::Path;
use std::sync::OnceLock;

use md5::{Digest, Md5};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Library {
    Dlss,
    DlssFrameGeneration,
    Xess,
}

impl Library {
    /// The catalogue's name for it (`resources/upscalers.json`).
    fn key(self) -> &'static str {
        match self {
            Library::Dlss => "dlss",
            Library::DlssFrameGeneration => "dlss_g",
            Library::Xess => "xess",
        }
    }
    /// Who must have signed a swap, compared without case ("Nvidia Corporation" signs the game's own).
    fn vendor(self) -> &'static str {
        match self {
            Library::Dlss | Library::DlssFrameGeneration => "NVIDIA Corporation",
            Library::Xess => "Intel Corporation",
        }
    }
    /// For the player.
    pub fn name(self) -> &'static str {
        match self {
            Library::Dlss => "DLSS",
            Library::DlssFrameGeneration => "DLSS Frame Generation",
            Library::Xess => "XeSS",
        }
    }
}

/// Where the game has them, relative to the game folder, as in the website's list.
pub const SLOTS: &[(&str, Library)] = &[
    ("Engine/Plugins/Runtime/Nvidia/DLSS/Binaries/ThirdParty/Win64/nvngx_dlss.dll", Library::Dlss),
    ("Engine/Plugins/Runtime/Nvidia/DLSS/Binaries/ThirdParty/Win64/Development/nvngx_dlss.dll", Library::Dlss),
    ("Engine/Plugins/Runtime/Nvidia/Streamline/Binaries/ThirdParty/Win64/nvngx_dlssg.dll", Library::DlssFrameGeneration),
    ("Engine/Plugins/Runtime/Nvidia/Streamline/Binaries/ThirdParty/Win64/Development/nvngx_dlssg.dll", Library::DlssFrameGeneration),
    ("Engine/Plugins/Runtime/Intel/XeSS/Binaries/ThirdParty/Win64/libxess.dll", Library::Xess),
];

/// The library a game file is, if it is one a player may swap.
pub fn slot(relative: &str) -> Option<Library> {
    SLOTS.iter().find(|(p, _)| p.eq_ignore_ascii_case(relative)).map(|(_, l)| *l)
}

/// One build of the catalogue: version, MD5 (upper-case hex), size in bytes.
type Build = (String, String, u64);

#[derive(serde::Deserialize)]
struct Catalogue {
    dlss: Vec<Build>,
    dlss_g: Vec<Build>,
    xess: Vec<Build>,
}

fn catalogue() -> &'static Catalogue {
    static CATALOGUE: OnceLock<Catalogue> = OnceLock::new();
    CATALOGUE.get_or_init(|| {
        serde_json::from_str(include_str!("../resources/upscalers.json")).expect("resources/upscalers.json is valid")
    })
}

fn builds(library: Library) -> &'static [Build] {
    let c = catalogue();
    match library.key() {
        "dlss" => &c.dlss,
        "dlss_g" => &c.dlss_g,
        _ => &c.xess,
    }
}

/// The catalogue's version for a file of this size and MD5, if any.
pub fn known_build(library: Library, size: u64, md5_hex: &str) -> Option<&'static str> {
    builds(library).iter().find(|(_, md5, s)| *s == size && md5.eq_ignore_ascii_case(md5_hex)).map(|(v, _, _)| v.as_str())
}

fn md5_of(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hasher.finalize().iter().map(|b| format!("{b:02X}")).collect())
}

/// The file at `path` as the player's swap of `library`: the catalogue's version when its size,
/// MD5 and vendor signature all match, None otherwise.
pub fn recognise(path: &Path, library: Library) -> Option<String> {
    let size = std::fs::metadata(path).ok()?.len();
    // Most files are ruled out by their size alone, without reading them.
    if !builds(library).iter().any(|(_, _, s)| *s == size) {
        return None;
    }
    let version = known_build(library, size, &md5_of(path)?)?;
    let signer = signer(path)?;
    signer.eq_ignore_ascii_case(library.vendor()).then(|| version.to_string())
}

/// Who signed `path`, when its Authenticode signature is valid (no revocation check, as
/// smart_app_control::signed: that would go online at every Play).
#[cfg(windows)]
fn signer(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Cryptography::{CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE};
    use windows_sys::Win32::Security::WinTrust::{
        WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CHOICE_FILE,
        WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };
    let file: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: plain C structs, zeroed and then filled as WinVerifyTrust expects; `file` and `info`
    // outlive both calls. The provider data read in between belongs to the state the first call
    // opened, which the second call closes; nothing from it is kept but the copied name.
    unsafe {
        let mut info: WINTRUST_FILE_INFO = std::mem::zeroed();
        info.cbStruct = std::mem::size_of::<WINTRUST_FILE_INFO>() as u32;
        info.pcwszFilePath = file.as_ptr();
        let mut data: WINTRUST_DATA = std::mem::zeroed();
        data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
        data.dwUIChoice = WTD_UI_NONE;
        data.fdwRevocationChecks = WTD_REVOKE_NONE;
        data.dwUnionChoice = WTD_CHOICE_FILE;
        data.Anonymous = WINTRUST_DATA_0 { pFile: &mut info };
        data.dwStateAction = WTD_STATEACTION_VERIFY;
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let result = WinVerifyTrust(std::ptr::null_mut(), &mut action, (&mut data as *mut WINTRUST_DATA).cast());
        let mut name = None;
        if result == 0 {
            let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
            let signer = if provider.is_null() { std::ptr::null_mut() } else { WTHelperGetProvSignerFromChain(provider, 0, 0, 0) };
            let cert = if signer.is_null() { std::ptr::null_mut() } else { WTHelperGetProvCertFromChain(signer, 0) };
            if !cert.is_null() && !(*cert).pCert.is_null() {
                let mut buf = [0u16; 256];
                let n = CertGetNameStringW((*cert).pCert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, std::ptr::null(), buf.as_mut_ptr(), buf.len() as u32);
                if n > 1 {
                    name = Some(String::from_utf16_lossy(&buf[..n as usize - 1]));
                }
            }
        }
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(std::ptr::null_mut(), &mut action, (&mut data as *mut WINTRUST_DATA).cast());
        name
    }
}

#[cfg(not(windows))]
fn signer(_: &Path) -> Option<String> {
    None
}

/// What Play tells the player about swaps it does not recognise.
pub fn unrecognised_message(paths: &[String]) -> String {
    let names: Vec<String> = paths
        .iter()
        .map(|p| {
            let file = p.rsplit('/').next().unwrap_or(p);
            match slot(p) {
                Some(l) => format!("{file} ({})", l.name()),
                None => file.to_string(),
            }
        })
        .collect();
    format!(
        "{} replaced with a version the launcher does not recognise. Put back a recognised version, or the original, \
with the tool you used to replace it (for example DLSS Swapper), or press Verify files to restore the game's own",
        match names.len() {
            1 => format!("{} was", names[0]),
            _ => format!("{} were", names.join(", ")),
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_five_library_paths_are_slots() {
        assert_eq!(slot("Engine/Plugins/Runtime/Nvidia/DLSS/Binaries/ThirdParty/Win64/nvngx_dlss.dll"), Some(Library::Dlss));
        assert_eq!(slot("engine/plugins/runtime/intel/xess/binaries/thirdparty/win64/LIBXESS.DLL"), Some(Library::Xess));
        assert_eq!(
            slot("Engine/Plugins/Runtime/Nvidia/Streamline/Binaries/ThirdParty/Win64/Development/nvngx_dlssg.dll"),
            Some(Library::DlssFrameGeneration)
        );
        // Same names elsewhere, and the libraries' neighbours, stay ordinary game files.
        assert_eq!(slot("BravoHotelGame/Binaries/Win64/nvngx_dlss.dll"), None);
        assert_eq!(slot("Engine/Plugins/Runtime/Nvidia/Streamline/Binaries/ThirdParty/Win64/sl.interposer.dll"), None);
        assert_eq!(slot("Engine/Plugins/Runtime/Intel/XeSS/Binaries/ThirdParty/Win64/igxess.dll"), None);
    }

    #[test]
    fn the_catalogue_knows_builds_by_size_and_md5_per_library() {
        // XeSS 1.0.1.12 (the game's own) and Frame Generation 1.0.2.0, as DLSS Swapper lists them.
        assert_eq!(known_build(Library::Xess, 9272488, "049bfea8c51245a545dc242a7317b9d3"), Some("1.0.1.12"));
        assert_eq!(known_build(Library::DlssFrameGeneration, 12799528, "36931062A0B9227011885514914C7A3B"), Some("1.0.2.0"));
        // Right hash, wrong library or wrong size: not recognised.
        assert_eq!(known_build(Library::Dlss, 9272488, "049BFEA8C51245A545DC242A7317B9D3"), None);
        assert_eq!(known_build(Library::Xess, 9272489, "049BFEA8C51245A545DC242A7317B9D3"), None);
        assert!(builds(Library::Dlss).len() > 100 && !builds(Library::Xess).is_empty());
    }

    #[test]
    fn a_file_with_the_right_name_but_not_a_catalogue_build_is_not_recognised() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("nvngx_dlss.dll");
        std::fs::write(&fake, b"not a dlss build").unwrap();
        assert_eq!(recognise(&fake, Library::Dlss), None);
    }

    /// With SP_TEST_GAME_DIR set to an installed game: its own libraries' signers and builds.
    #[test]
    fn the_games_own_signed_builds_when_a_game_is_at_hand() {
        let Some(game_dir) = std::env::var_os("SP_TEST_GAME_DIR") else { return };
        let at = |p: &str| p.split('/').fold(std::path::PathBuf::from(&game_dir), |d, part| d.join(part));
        let xess = at(SLOTS[4].0);
        let dlssg = at(SLOTS[2].0);
        assert_eq!(signer(&xess).as_deref(), Some("Intel Corporation"));
        assert!(signer(&dlssg).is_some_and(|s| s.eq_ignore_ascii_case("NVIDIA Corporation")));
        assert_eq!(recognise(&xess, Library::Xess).as_deref(), Some("1.0.1.12"));
        assert_eq!(recognise(&dlssg, Library::DlssFrameGeneration).as_deref(), Some("1.0.2.0"));
        assert_eq!(recognise(&xess, Library::Dlss), None);
    }

    #[test]
    fn the_unrecognised_message_names_the_library_and_the_tool() {
        let one = unrecognised_message(&["Engine/Plugins/Runtime/Nvidia/DLSS/Binaries/ThirdParty/Win64/nvngx_dlss.dll".into()]);
        assert!(one.starts_with("nvngx_dlss.dll (DLSS) was replaced"), "{one}");
        assert!(one.contains("DLSS Swapper") && one.contains("Verify files"));
        let two = unrecognised_message(&[
            "Engine/Plugins/Runtime/Nvidia/DLSS/Binaries/ThirdParty/Win64/nvngx_dlss.dll".into(),
            "Engine/Plugins/Runtime/Intel/XeSS/Binaries/ThirdParty/Win64/libxess.dll".into(),
        ]);
        assert!(two.starts_with("nvngx_dlss.dll (DLSS), libxess.dll (XeSS) were replaced"), "{two}");
    }
}
