//! Own the optional runtime files for one game session, including crash recovery.
use std::{fs::{self, File, OpenOptions}, io::Write, path::{Path, PathBuf}};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use crate::error::{LauncherError, Result};

pub const DLL_PATH: &str = "BravoHotelGame/Binaries/Win64/SPClientFixes.dll";
pub const PAK_PATH: &str = "BravoHotelGame/Content/Paks/BravoHotelGame-ClientFixes_P.pak";
pub const SIG_PATH: &str = "BravoHotelGame/Content/Paks/BravoHotelGame-ClientFixes_P.sig";
const PATHS: [&str; 3] = [DLL_PATH, PAK_PATH, SIG_PATH];
// Exact artifacts deployed during the completed preservation experiment.
const LEGACY: [(&str, &str); 3] = [
    (DLL_PATH, "b69b6b32eb14562f428c38fbc7b70df9d4ade39703b21f3b27b95815544b9f97"),
    (PAK_PATH, "002174c2509a48a3bcb7beb56dbae35aae6db00fd48eb402e9d0543e1a38c506"),
    (SIG_PATH, "4a3a1c4a79b06aebd5f42678603f9fa9d52b66c2c5ae7d50816ae7e5027103e0"),
];
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record { files: Vec<OwnedFile>, pid: Option<u32> }
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedFile { relative: String, sha256: String }
fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn message(text: impl Into<String>) -> LauncherError { LauncherError::Message(text.into()) }

pub struct Deployment {
    root: PathBuf,
    record_path: PathBuf,
    record: Record,
    _lock: File,
    enabled: bool,
    cleanup_on_drop: bool,
}
impl Deployment {
    pub fn prepare(root: &Path, config: &Path, enabled: bool, payloads: [&[u8]; 3]) -> Result<Self> {
        if enabled && payloads.iter().any(|p| p.is_empty()) {
            return Err(message("This launcher is missing a Client fixes resource. Download a complete build."));
        }
        let root = fs::canonicalize(root)?;
        fs::create_dir_all(config)?;
        let id = hash(root.to_string_lossy().to_lowercase().as_bytes());
        let record_path = config.join(format!("client-fixes-{id}.json"));
        let lock_path = config.join(format!("client-fixes-{id}.lock"));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)] {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let lock = options.open(lock_path).map_err(|_| message("Another game session is using this installation. Close it before launching again."))?;
        ensure_no_game_running()?;
        let record = match fs::read(&record_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| message("The Client fixes ownership record is unreadable; no game files were changed."))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Record::default(),
            Err(e) => return Err(e.into()),
        };
        let mut session = Self { root, record_path, record, _lock: lock, enabled, cleanup_on_drop: false };
        if let Some(pid) = session.record.pid {
            if process_alive(pid)? { return Err(message("The previous game session is still running. Close it before changing Client fixes.")); }
        }
        // Adopt only exact known experimental/bundled artifacts, never by name alone.
        for (i, relative) in PATHS.iter().enumerate() {
            let target = session.target(relative)?;
            if let Ok(bytes) = fs::read(&target) {
                let digest = hash(&bytes);
                if let Some(owned) = session.record.files.iter().find(|f| f.relative == *relative) {
                    if owned.sha256 != digest { return Err(message(format!("Client fixes file was changed: {}. It was preserved.", target.display()))); }
                } else if (!payloads[i].is_empty() && digest == hash(payloads[i])) || LEGACY.iter().any(|(p,h)| p == relative && *h == digest) {
                    session.record.files.push(OwnedFile { relative: relative.to_string(), sha256: digest });
                } else {
                    return Err(message(format!("An unrelated file occupies {}. It was preserved; move it before launching.", target.display())));
                }
            } else if target.exists() {
                return Err(message(format!("Cannot read {}. No replacement was attempted.", target.display())));
            }
        }
        session.save()?;
        session.cleanup_on_drop = true;
        session.cleanup()?;
        if enabled {
            session.record.files = PATHS.iter().enumerate().map(|(i,p)| OwnedFile { relative: p.to_string(), sha256: hash(payloads[i]) }).collect();
            // Persist intended ownership first: interrupted writes can be recovered.
            session.save()?;
            for (i, relative) in PATHS.iter().enumerate() {
                let target = session.target(relative)?;
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(windows)] {
                    use std::os::windows::fs::OpenOptionsExt;
                    options.share_mode(0);
                }
                let mut file = options.open(&target)?;
                let written = file.write_all(payloads[i]).and_then(|_| file.sync_all());
                if let Err(error) = written {
                    drop(file);
                    fs::remove_file(&target)?;
                    return Err(error.into());
                }
            }
        }
        Ok(session)
    }
    fn target(&self, relative: &str) -> Result<PathBuf> {
        if !PATHS.contains(&relative) { return Err(message("Invalid Client fixes ownership path.")); }
        let mut path = self.root.clone();
        for part in relative.split('/') {
            path.push(part);
            match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    let mut redirected = metadata.file_type().is_symlink();
                    #[cfg(windows)] {
                        use std::os::windows::fs::MetadataExt;
                        redirected |= metadata.file_attributes() & 0x400 != 0;
                    }
                    if redirected { return Err(message(format!("Client fixes path redirects elsewhere: {}", path.display()))); }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(path)
    }
    fn save(&self) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.record).map_err(|e| message(e.to_string()))?;
        let temp = self.record_path.with_extension("json.new");
        fs::write(&temp, bytes)?;
        OpenOptions::new().write(true).open(&temp)?.sync_all()?;
        fs::rename(temp, &self.record_path)?;
        Ok(())
    }
    pub fn dll_path(&self) -> Option<PathBuf> { self.enabled.then(|| self.root.join(DLL_PATH)) }
    pub fn mark_running(&mut self, pid: u32) -> Result<()> { self.record.pid = Some(pid); self.save() }
    pub fn cleanup(&mut self) -> Result<()> {
        if let Some(pid) = self.record.pid {
            if process_alive(pid)? { return Err(message("Client fixes remain deployed while the game is running.")); }
        }
        // Validate every recorded file before removing any; preserve modified files.
        for owned in &self.record.files {
            let target = self.target(&owned.relative)?;
            match fs::read(&target) {
                Ok(bytes) if hash(&bytes) == owned.sha256 => (),
                Ok(_) => return Err(message(format!("Client fixes file changed and was preserved: {}", target.display()))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        for owned in &self.record.files {
            let target = self.target(&owned.relative)?;
            match fs::remove_file(target) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        self.record = Record::default();
        match fs::remove_file(&self.record_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
impl Drop for Deployment {
    fn drop(&mut self) {
        if !self.cleanup_on_drop { return; }
        if let Err(e) = self.cleanup() { eprintln!("[client fixes] {e}"); }
    }
}
#[cfg(windows)]
fn process_alive(pid: u32) -> Result<bool> {
    use windows_sys::Win32::{Foundation::{CloseHandle, GetLastError}, System::Threading::{OpenProcess, WaitForSingleObject}};
    unsafe {
        let process = OpenProcess(0x00100000, 0, pid);
        if process.is_null() {
            if GetLastError() == 87 { return Ok(false); }
            return Err(message("Cannot confirm that the previous game exited. Client fixes files were preserved."));
        }
        let status = WaitForSingleObject(process, 0);
        CloseHandle(process);
        match status { 0 => Ok(false), 258 => Ok(true), _ => Err(message("Cannot check the previous game session.")) }
    }
}
#[cfg(not(windows))]
fn process_alive(_pid: u32) -> Result<bool> { Ok(false) }
#[cfg(windows)]
fn ensure_no_game_running() -> Result<()> {
    use windows_sys::Win32::{Foundation::{CloseHandle, INVALID_HANDLE_VALUE}, System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS}};
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE { return Err(message("Cannot check whether the game is already running.")); }
        let mut item: PROCESSENTRY32W = std::mem::zeroed();
        item.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut item) == 0 { CloseHandle(snapshot); return Err(message("Cannot inspect the running process list.")); }
        loop {
            let len = item.szExeFile.iter().position(|&c| c == 0).unwrap_or(item.szExeFile.len());
            let name = String::from_utf16_lossy(&item.szExeFile[..len]);
            if name.eq_ignore_ascii_case("BravoHotelClient-Win64-Shipping.exe") || name.eq_ignore_ascii_case("BravoHotelClient.exe") {
                CloseHandle(snapshot);
                return Err(message("The game is already running. Close it before launching or changing Client fixes."));
            }
            if Process32NextW(snapshot, &mut item) == 0 { break; }
        }
        CloseHandle(snapshot);
    }
    Ok(())
}
#[cfg(not(windows))]
fn ensure_no_game_running() -> Result<()> { Ok(()) }

#[cfg(test)]
mod tests {
    use super::*;
    const PAYLOADS: [&[u8]; 3] = [b"test dll", b"test pak", b"test signature"];
    fn folders(root: &Path) {
        fs::create_dir_all(root.join("BravoHotelGame/Binaries/Win64")).unwrap();
        fs::create_dir_all(root.join("BravoHotelGame/Content/Paks")).unwrap();
    }
    #[test]
    fn enabled_session_deploys_all_three_then_cleans_up() {
        let game=tempfile::tempdir().unwrap(); let state=tempfile::tempdir().unwrap(); folders(game.path());
        let mut session=Deployment::prepare(game.path(),state.path(),true,PAYLOADS).unwrap();
        for (i,p) in PATHS.iter().enumerate() { assert_eq!(fs::read(game.path().join(p)).unwrap(),PAYLOADS[i]); }
        session.cleanup().unwrap();
        for p in PATHS { assert!(!game.path().join(p).exists()); }
        assert!(!session.record_path.exists());
    }
    #[test]
    fn disabled_launch_recovers_orphaned_files_without_redeploying() {
        let game=tempfile::tempdir().unwrap(); let state=tempfile::tempdir().unwrap(); folders(game.path());
        let mut orphan=Deployment::prepare(game.path(),state.path(),true,PAYLOADS).unwrap();
        orphan.cleanup_on_drop=false; drop(orphan);
        let session=Deployment::prepare(game.path(),state.path(),false,PAYLOADS).unwrap();
        assert!(session.dll_path().is_none());
        for p in PATHS { assert!(!game.path().join(p).exists()); }
    }
    #[test]
    fn unknown_files_are_preserved_and_launch_is_refused() {
        let game=tempfile::tempdir().unwrap(); let state=tempfile::tempdir().unwrap(); folders(game.path());
        let target=game.path().join(PAK_PATH); fs::write(&target,b"another mod").unwrap();
        assert!(Deployment::prepare(game.path(),state.path(),true,PAYLOADS).is_err());
        assert_eq!(fs::read(target).unwrap(),b"another mod");
        assert!(!game.path().join(DLL_PATH).exists());
    }
    #[test]
    fn modified_owned_file_prevents_removal_of_the_whole_pair() {
        let game=tempfile::tempdir().unwrap(); let state=tempfile::tempdir().unwrap(); folders(game.path());
        let mut session=Deployment::prepare(game.path(),state.path(),true,PAYLOADS).unwrap();
        fs::write(game.path().join(PAK_PATH),b"changed").unwrap();
        assert!(session.cleanup().is_err());
        assert!(game.path().join(SIG_PATH).exists());
        assert!(game.path().join(DLL_PATH).exists());
        session.cleanup_on_drop=false;
    }
    #[test]
    fn failed_partial_deployment_rolls_back_completed_files() {
        let game=tempfile::tempdir().unwrap(); let state=tempfile::tempdir().unwrap();
        fs::create_dir_all(game.path().join("BravoHotelGame/Binaries/Win64")).unwrap();
        assert!(Deployment::prepare(game.path(),state.path(),true,PAYLOADS).is_err());
        assert!(!game.path().join(DLL_PATH).exists());
    }
    #[test]
    fn altered_record_cannot_remove_files_outside_known_targets() {
        let game=tempfile::tempdir().unwrap(); let state=tempfile::tempdir().unwrap(); folders(game.path());
        let mut session=Deployment::prepare(game.path(),state.path(),true,PAYLOADS).unwrap();
        let unrelated=game.path().join("keep.txt");fs::write(&unrelated,b"keep").unwrap();
        session.record.files.push(OwnedFile { relative:"keep.txt".into(),sha256:hash(b"keep") });
        assert!(session.cleanup().is_err());assert!(unrelated.exists());
        session.cleanup_on_drop=false;
    }
    #[cfg(windows)]
    #[test]
    fn exclusive_lock_prevents_another_launch_from_cleaning_active_files() {
        let game=tempfile::tempdir().unwrap();let state=tempfile::tempdir().unwrap();folders(game.path());
        let _session=Deployment::prepare(game.path(),state.path(),true,PAYLOADS).unwrap();
        assert!(Deployment::prepare(game.path(),state.path(),false,PAYLOADS).is_err());
        assert!(game.path().join(PAK_PATH).exists());
    }
    #[cfg(windows)]
    #[test]
    fn active_recorded_process_keeps_runtime_files_in_place() {
        let game=tempfile::tempdir().unwrap();let state=tempfile::tempdir().unwrap();folders(game.path());
        let mut session=Deployment::prepare(game.path(),state.path(),true,PAYLOADS).unwrap();
        session.mark_running(std::process::id()).unwrap();
        assert!(session.cleanup().is_err());session.cleanup_on_drop=false;drop(session);
        assert!(Deployment::prepare(game.path(),state.path(),false,PAYLOADS).is_err());
        assert!(game.path().join(PAK_PATH).exists());
    }
}
