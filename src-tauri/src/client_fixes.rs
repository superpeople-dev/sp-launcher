//! Prepare session-owned client fixes; the opted-in proxy loads the DLL.

#[cfg(all(windows, not(target_pointer_width = "64")))]
compile_error!("Client fixes require the 64-bit launcher build");

use std::path::Path;

use crate::error::{LauncherError, Result};

#[cfg(has_client_fixes)]
const DLL: &[u8] = include_bytes!("../resources/SPClientFixes.dll");
#[cfg(not(has_client_fixes))]
const DLL: &[u8] = &[];

#[cfg(has_client_fixes)]
const PAK: &[u8] = include_bytes!("../resources/client-fixes/BravoHotelGame-ClientFixes_P.pak");
#[cfg(has_client_fixes)]
const SIG: &[u8] = include_bytes!("../resources/client-fixes/BravoHotelGame-ClientFixes_P.sig");
#[cfg(not(has_client_fixes))]
const PAK: &[u8] = &[];
#[cfg(not(has_client_fixes))]
const SIG: &[u8] = &[];

pub fn prepare(install_dir: &str, config: &Path, enabled: bool) -> Result<crate::client_fixes_deployment::Deployment> {
    if install_dir.is_empty() { return Err(LauncherError::Message("no install directory set".into())); }
    if enabled {
        use sha2::{Digest, Sha256};
        let executable = crate::game::launch_exe_path(Path::new(install_dir));
        let digest = format!("{:x}", Sha256::digest(std::fs::read(executable)?));
        if digest != "16b8b421371457d936e5cc1810ff707b5f5984126973dbdc4e6b5c714522051f" {
            return Err(LauncherError::Message("Client fixes do not support this game build. Disable the option to launch without them.".into()));
        }
    }
    crate::client_fixes_deployment::Deployment::prepare(Path::new(install_dir), config, enabled, [DLL, PAK, SIG])
}
