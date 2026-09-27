//! Install and load the optional client fixes DLL independently of the no-Steam proxy.

#[cfg(all(windows, not(target_pointer_width = "64")))]
compile_error!("Client fixes injection requires the 64-bit launcher build");

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

/// The game is already running when this is called. Failure is fatal to launch;
/// the caller kills that child so it never continues with fixes half-enabled.
#[cfg(windows)]
pub fn inject(pid: u32, dll_path: &Path) -> Result<()> {
    windows::inject(pid, dll_path)
}

#[cfg(not(windows))]
pub fn inject(_pid: u32, _dll_path: &Path) -> Result<()> {
    Err(LauncherError::Message("Client fixes injection requires Windows".into()))
}

#[cfg(windows)]
mod windows {
    use std::ffi::{c_char, c_void, OsStr};
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::{null, null_mut};

    use crate::error::{LauncherError, Result};

    type Handle = *mut c_void;
    const PROCESS_ACCESS: u32 = 0x0002 | 0x0008 | 0x0020 | 0x0400;
    const MEM_COMMIT_RESERVE: u32 = 0x3000;
    const MEM_RELEASE: u32 = 0x8000;
    const PAGE_READWRITE: u32 = 0x04;
    const TH32CS_SNAPMODULE: u32 = 0x00000008;
    const TH32CS_SNAPMODULE32: u32 = 0x00000010;
    const WAIT_OBJECT_0: u32 = 0;

    #[repr(C)]
    struct ModuleEntry32W {
        size: u32,
        module_id: u32,
        process_id: u32,
        global_usage: u32,
        process_usage: u32,
        base: *mut u8,
        base_size: u32,
        module: Handle,
        name: [u16; 256],
        path: [u16; 260],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn VirtualAllocEx(process: Handle, address: *mut c_void, size: usize, allocation: u32, protection: u32) -> *mut c_void;
        fn VirtualFreeEx(process: Handle, address: *mut c_void, size: usize, free_type: u32) -> i32;
        fn WriteProcessMemory(process: Handle, address: *mut c_void, source: *const c_void, size: usize, written: *mut usize) -> i32;
        fn CreateRemoteThread(process: Handle, attributes: *const c_void, stack_size: usize, start: unsafe extern "system" fn(*mut c_void) -> u32, parameter: *mut c_void, flags: u32, thread_id: *mut u32) -> Handle;
        fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
        fn GetExitCodeThread(handle: Handle, exit_code: *mut u32) -> i32;
        fn GetModuleHandleW(name: *const u16) -> Handle;
        fn GetModuleHandleExW(flags: u32, address: *const u16, module: *mut Handle) -> i32;
        fn GetModuleFileNameW(module: Handle, buffer: *mut u16, size: u32) -> u32;
        fn GetProcAddress(module: Handle, name: *const c_char) -> *const c_void;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        fn Module32FirstW(snapshot: Handle, entry: *mut ModuleEntry32W) -> i32;
        fn Module32NextW(snapshot: Handle, entry: *mut ModuleEntry32W) -> i32;
    }

    struct OwnedHandle(Handle);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0); }
        }
    }

    fn win_error(action: &str) -> LauncherError {
        LauncherError::Message(format!("Client fixes: {action}: {}", std::io::Error::last_os_error()))
    }

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Resolve the target process's own module base; a local module address
    /// cannot safely be passed to another process because of ASLR.
    fn remote_module(pid: u32, name: &str) -> Result<Option<usize>> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) };
        if snapshot as isize == -1 {
            return Err(win_error("cannot enumerate game modules"));
        }
        let snapshot = OwnedHandle(snapshot);
        let mut entry: ModuleEntry32W = unsafe { zeroed() };
        entry.size = size_of::<ModuleEntry32W>() as u32;
        let mut valid = unsafe { Module32FirstW(snapshot.0, &mut entry) };
        while valid != 0 {
            let end = entry.name.iter().position(|c| *c == 0).unwrap_or(entry.name.len());
            if String::from_utf16_lossy(&entry.name[..end]).eq_ignore_ascii_case(name) {
                return Ok(Some(entry.base as usize));
            }
            valid = unsafe { Module32NextW(snapshot.0, &mut entry) };
        }
        Ok(None)
    }

    fn wait_remote_module(pid: u32, name: &str, attempts: usize) -> Result<Option<usize>> {
        for attempt in 0..attempts {
            match remote_module(pid, name) {
                Ok(Some(base)) => return Ok(Some(base)),
                Ok(None) => (),
                Err(error) if attempt + 1 == attempts => return Err(error),
                Err(_) => (),
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Ok(None)
    }

    pub fn inject(pid: u32, dll_path: &Path) -> Result<()> {
        let dll_path = dll_path.canonicalize()?;
        let path = wide(dll_path.as_os_str());
        let process = unsafe { OpenProcess(PROCESS_ACCESS, 0, pid) };
        if process.is_null() {
            return Err(win_error("cannot open game process"));
        }
        let process = OwnedHandle(process);

        let kernel_name = wide(OsStr::new("kernel32.dll"));
        let local_kernel = unsafe { GetModuleHandleW(kernel_name.as_ptr()) };
        if local_kernel.is_null() {
            return Err(win_error("cannot locate kernel32.dll"));
        }
        let local_load = unsafe { GetProcAddress(local_kernel, b"LoadLibraryW\0".as_ptr().cast()) };
        if local_load.is_null() {
            return Err(win_error("cannot locate LoadLibraryW"));
        }
        // GetProcAddress can return an implementation in KernelBase rather
        // than kernel32. Find the module that actually owns the function.
        let mut local_owner: Handle = null_mut();
        if unsafe { GetModuleHandleExW(0x6, local_load.cast(), &mut local_owner) } == 0 {
            return Err(win_error("cannot identify LoadLibraryW's module"));
        }
        let mut owner_path = [0u16; 512];
        let count = unsafe { GetModuleFileNameW(local_owner, owner_path.as_mut_ptr(), owner_path.len() as u32) };
        if count == 0 || count as usize == owner_path.len() {
            return Err(win_error("cannot name LoadLibraryW's module"));
        }
        let owner_path = String::from_utf16_lossy(&owner_path[..count as usize]);
        let owner_name = Path::new(&owner_path).file_name().and_then(|n| n.to_str())
            .ok_or_else(|| LauncherError::Message("Client fixes: invalid loader module name".into()))?;
        let remote_owner = wait_remote_module(pid, owner_name, 50)?
            .ok_or_else(|| LauncherError::Message(format!("Client fixes: game {owner_name} is not loaded")))?;
        let load_rva = (local_load as usize).checked_sub(local_owner as usize)
            .ok_or_else(|| LauncherError::Message("Client fixes: invalid LoadLibraryW address".into()))?;
        let load_address = remote_owner + load_rva;

        let bytes = path.len() * size_of::<u16>();
        let remote_path = unsafe { VirtualAllocEx(process.0, null_mut(), bytes, MEM_COMMIT_RESERVE, PAGE_READWRITE) };
        if remote_path.is_null() {
            return Err(win_error("cannot allocate DLL path in game"));
        }
        let mut written = 0usize;
        let write_ok = unsafe { WriteProcessMemory(process.0, remote_path, path.as_ptr().cast(), bytes, &mut written) } != 0 && written == bytes;
        if !write_ok {
            unsafe { VirtualFreeEx(process.0, remote_path, 0, MEM_RELEASE); }
            return Err(win_error("cannot write DLL path into game"));
        }

        let load: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { std::mem::transmute(load_address) };
        let thread = unsafe { CreateRemoteThread(process.0, null(), 0, load, remote_path, 0, null_mut()) };
        if thread.is_null() {
            unsafe { VirtualFreeEx(process.0, remote_path, 0, MEM_RELEASE); }
            return Err(win_error("cannot start DLL loader in game"));
        }
        let thread = OwnedHandle(thread);
        let wait = unsafe { WaitForSingleObject(thread.0, 30_000) };
        if wait != WAIT_OBJECT_0 {
            // The caller terminates the child on error. Keep the path alive
            // until then in case LoadLibraryW is still reading it.
            return Err(LauncherError::Message(format!("Client fixes: DLL load did not finish (wait {wait:#x})")));
        }
        let mut exit_code = 0u32;
        let finished = unsafe { GetExitCodeThread(thread.0, &mut exit_code) };
        unsafe { VirtualFreeEx(process.0, remote_path, 0, MEM_RELEASE); }
        // GetExitCodeThread truncates the 64-bit HMODULE returned by
        // LoadLibraryW, so module presence is the decisive success check.
        if finished == 0 || wait_remote_module(pid, "SPClientFixes.dll", 10)?.is_none() {
            return Err(LauncherError::Message("Client fixes: the game did not load SPClientFixes.dll".into()));
        }
        Ok(())
    }
}
