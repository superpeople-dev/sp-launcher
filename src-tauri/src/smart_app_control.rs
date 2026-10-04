//! Windows 11's Smart App Control.
//!
//! While it enforces, Windows only loads code that is signed by a trusted
//! provider or that Microsoft's cloud already knows. Our DLLs in the game's
//! folder (the no-Steam shim XAPOFX1_5.dll, and SPClientFixes.dll) change with
//! every release and are not code-signed yet, so the game stops at start with
//! "Bad Image" (error 0xc0e90002, "the system integrity policy has been
//! violated"). Play asks here after putting them in place and, rather than
//! starting a game that cannot load, has the launcher explain it.
//!
//! The state is `VerifiedAndReputablePolicyState` under
//! `HKLM\SYSTEM\CurrentControlSet\Control\CI\Policy`: 0 off, 1 on, 2 evaluation
//! (it only watches). Each DLL is checked for a valid Authenticode signature,
//! so once our builds are signed (Azure Artifact Signing) nothing is refused and
//! Play goes on as before.

use std::path::{Path, PathBuf};

/// The first of our DLLs in the game's folder that Smart App Control would
/// refuse: it is on, and that file is not validly signed. None when it is off
/// or in evaluation, or when every file is signed.
pub fn blocked(install_dir: &str, client_fixes: bool) -> Option<PathBuf> {
    if state() != Some(1) {
        return None;
    }
    ours(Path::new(install_dir), client_fixes)
        .into_iter()
        .find(|file| file.exists() && !signed(file))
}

/// The DLLs the launcher puts next to the game for this start.
fn ours(install_dir: &Path, client_fixes: bool) -> Vec<PathBuf> {
    let mut files = vec![crate::shim::target_path(install_dir)];
    if client_fixes {
        files.push(install_dir.join(crate::client_fixes_deployment::DLL_PATH.replace('/', "\\")));
    }
    files
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn state() -> Option<u32> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
    let key = wide(r"SYSTEM\CurrentControlSet\Control\CI\Policy");
    let name = wide("VerifiedAndReputablePolicyState");
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: both strings are NUL-terminated and outlive the call; the value
    // buffer is a u32 whose size is passed in `size`.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    (status == 0).then_some(value)
}

#[cfg(not(windows))]
fn state() -> Option<u32> {
    None
}

/// Whether `path` has a valid Authenticode signature (WinVerifyTrust, no
/// revocation check: that would go online at every Play, and Smart App Control
/// decides on its own anyway).
#[cfg(windows)]
pub fn signed(path: &Path) -> bool {
    use windows_sys::Win32::Security::WinTrust::{
        WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
        WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };
    let file = wide(&path.to_string_lossy());
    // SAFETY: plain C structs, zeroed and then filled as WinVerifyTrust expects;
    // `file` and `info` outlive both calls, and the state opened by the first
    // call is closed by the second.
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
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(std::ptr::null_mut(), &mut action, (&mut data as *mut WINTRUST_DATA).cast());
        result == 0
    }
}

#[cfg(not(windows))]
pub fn signed(_path: &Path) -> bool {
    true
}

/// Opens Windows Security on App & browser control, where Smart App Control's
/// settings are (or on its home page if that page does not open).
#[cfg(windows)]
pub fn open_settings() -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb = wide("open");
    ["windowsdefender://appbrowser", "windowsdefender:"].iter().any(|target| {
        let target = wide(target);
        // SAFETY: NUL-terminated strings that outlive the call; no parameters
        // or directory. ShellExecuteW returns a value above 32 on success.
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        result as isize > 32
    })
}

#[cfg(not(windows))]
pub fn open_settings() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shim_is_always_checked_and_the_fixes_dll_only_when_they_are_on() {
        let dir = Path::new(r"C:\Games\SUPER PEOPLE");
        let without = ours(dir, false);
        assert_eq!(without.len(), 1);
        assert!(without[0].ends_with("XAPOFX1_5.dll"));
        let with = ours(dir, true);
        assert_eq!(with.len(), 2);
        assert!(with[1].to_string_lossy().ends_with(r"BravoHotelGame\Binaries\Win64\SPClientFixes.dll"));
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_file_is_signed_and_an_unsigned_one_is_not() {
        // Windows' own files are mostly catalog-signed, which this check does not
        // read (our DLLs carry their signature inside, as these two do).
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let x86 = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
        let embedded = [
            Path::new(&x86).join(r"Microsoft\Edge\Application\msedge.exe"),
            Path::new(&windows).join(r"System32\MRT.exe"),
        ];
        if let Some(file) = embedded.iter().find(|f| f.exists()) {
            assert!(signed(file), "{} should be signed", file.display());
        }
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain.dll");
        std::fs::write(&plain, b"MZ not really a DLL").unwrap();
        assert!(!signed(&plain));
    }

    #[test]
    fn nothing_is_blocked_when_smart_app_control_is_not_on() {
        // On a build machine it is off (CI runners) or unknown; either way a
        // folder that does not exist is never reported.
        if state() != Some(1) {
            assert!(blocked(r"Z:\no such game folder", true).is_none());
        }
    }
}
