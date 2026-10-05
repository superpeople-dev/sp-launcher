//! Session-scoped acknowledgement of proxy loading and DLL bootstrap.
use crate::error::{LauncherError, Result};

/// What the player sees when the proxy (XAPOFX1_5.dll) vanished from the game folder after the
/// launcher wrote it. 05.10.2026: two players' games loaded Windows' own XAPOFX1_5.dll from
/// System32 instead (the crash dumps' module lists), so the Client fixes never loaded and the game
/// ended about 20 s later on the Client fixes PAK's signature. Their antivirus had taken ours away.
pub fn proxy_removed() -> LauncherError {
    LauncherError::Message("XAPOFX1_5.dll disappeared from the game folder right after the launcher put it there, \
so the game could not start. Your antivirus most likely removed it: restore it from the antivirus' quarantine \
(Windows Security > Virus & threat protection > Protection history), add the SUPER PEOPLE folder as an exclusion, \
then launch again.".into())
}

/// The game closed before the fixes answered, its proxy still in place. Without the fixes the game
/// stops on our pak's signature a few seconds in ("Pak master signature table check failed"), and
/// what keeps them from loading with the files there is nearly always an antivirus blocking
/// XAPOFX1_5.dll or SPClientFixes.dll.
pub fn exited_early(status: std::process::ExitStatus) -> LauncherError {
    LauncherError::Message(format!("The game closed before the client fixes started ({status}). If this keeps happening, your antivirus is most likely keeping XAPOFX1_5.dll or SPClientFixes.dll in the game folder from loading: add the SUPER PEOPLE folder as an exclusion in your antivirus (and restore anything it quarantined), then launch again."))
}

pub fn environment(enabled: bool, debug_window: bool) -> Vec<(String, String)> {
    vec![
        ("SP_CLIENT_FIXES_ENABLED".into(), if enabled { "1" } else { "0" }.into()),
        ("SP_CLIENT_FIXES_CONSOLE".into(), if enabled && debug_window { "1" } else { "0" }.into()),
        // Override ambient session names even when fixes are off.
        ("SP_CLIENT_FIXES_READY_EVENT".into(), String::new()),
        ("SP_CLIENT_FIXES_FAILED_EVENT".into(), String::new()),
    ]
}

#[cfg(windows)]
pub use windows::Startup;

#[cfg(not(windows))]
pub struct Startup;
#[cfg(not(windows))]
impl Startup {
    pub fn new() -> Result<Self> { Err(LauncherError::Message("Client fixes require Windows".into())) }
    pub fn environment(&self) -> Vec<(String, String)> { vec![] }
    pub fn wait(&self, _: &mut std::process::Child, _: &dyn Fn() -> bool) -> Result<()> {
        Err(LauncherError::Message("Client fixes require Windows".into()))
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    type Handle = *mut c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateEventW(attributes: *const c_void, manual_reset: i32, initial: i32, name: *const u16) -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn WaitForMultipleObjects(count: u32, handles: *const Handle, all: i32, milliseconds: u32) -> u32;
    }
    struct Event { handle: Handle, name: String }
    // Handles are owned by this object and may be waited on from another thread.
    unsafe impl Send for Event {}
    impl Drop for Event { fn drop(&mut self) { unsafe { CloseHandle(self.handle); } } }
    impl Event {
        fn new(name: String) -> Result<Self> {
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, wide.as_ptr()) };
            let error = std::io::Error::last_os_error();
            if handle.is_null() { return Err(LauncherError::Message(format!("Client fixes: cannot create startup event: {error}"))); }
            if error.raw_os_error() == Some(183) {
                unsafe { CloseHandle(handle); }
                return Err(LauncherError::Message("Client fixes: startup event already exists".into()));
            }
            Ok(Self { handle, name })
        }
    }
    pub struct Startup { ready: Event, failed: Event }
    impl Startup {
        pub fn new() -> Result<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)
                .map_err(|e| LauncherError::Message(e.to_string()))?.as_nanos();
            let prefix = format!("Local\\SPClientFixes-{}-{timestamp}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
            Ok(Self { ready: Event::new(format!("{prefix}-ready"))?, failed: Event::new(format!("{prefix}-failed"))? })
        }
        pub fn environment(&self) -> Vec<(String, String)> {
            vec![("SP_CLIENT_FIXES_READY_EVENT".into(), self.ready.name.clone()),
                 ("SP_CLIENT_FIXES_FAILED_EVENT".into(), self.failed.name.clone())]
        }
        /// `proxy_present` says whether the proxy DLL is still where the launcher put it.
        pub fn wait(&self, child: &mut std::process::Child, proxy_present: &dyn Fn() -> bool) -> Result<()> {
            self.wait_for(child, Duration::from_secs(60), proxy_present)
        }
        fn wait_for(&self, child: &mut std::process::Child, timeout: Duration, proxy_present: &dyn Fn() -> bool) -> Result<()> {
            let deadline = Instant::now() + timeout;
            // Failure wins if both were signaled.
            let handles = [self.failed.handle, self.ready.handle];
            // The proxy gone and no answer from the fixes for this long: the game runs without them
            // and would end on the PAK's signature, so the caller stops it now. A start that works
            // answers in about 2 s, so a proxy removed after it loaded never gets here.
            const PROXY_GRACE: Duration = Duration::from_secs(5);
            let mut missing_since: Option<Instant> = None;
            loop {
                if let Some(status) = child.try_wait()? {
                    if !proxy_present() { return Err(proxy_removed()); }
                    return Err(exited_early(status));
                }
                if proxy_present() {
                    missing_since = None;
                } else if missing_since.get_or_insert_with(Instant::now).elapsed() >= PROXY_GRACE {
                    return Err(proxy_removed());
                }
                match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, 100) } {
                    0 => return Err(LauncherError::Message("Client fixes could not initialize. Check sp_client.log beside the game executable and the optional Client fixes debug window.".into())),
                    1 => return Ok(()),
                    258 if Instant::now() < deadline => (),
                    258 => return Err(LauncherError::Message("Client fixes startup timed out. The proxy or fixes DLL may be missing, blocked, or incompatible.".into())),
                    _ => return Err(LauncherError::Message(format!("Client fixes: cannot wait for startup: {}", std::io::Error::last_os_error()))),
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        extern "system" { fn SetEvent(handle: Handle) -> i32; }
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
        fn child() -> ChildGuard {
            use std::os::windows::process::CommandExt;
            ChildGuard(std::process::Command::new("cmd.exe").args(["/c", "ping -n 20 127.0.0.1 >nul"])
                .creation_flags(0x08000000).spawn().unwrap())
        }
        #[test]
        fn fresh_sessions_do_not_reuse_acknowledgements() {
            let one = Startup::new().unwrap();
            let two = Startup::new().unwrap();
            assert_ne!(one.ready.name, two.ready.name);
            unsafe { SetEvent(one.ready.handle); }
            let mut process = child();
            assert!(one.wait_for(&mut process.0, Duration::from_millis(150), &|| true).is_ok());
            assert!(two.wait_for(&mut process.0, Duration::from_millis(150), &|| true).is_err());
        }
        #[test]
        fn failure_wins_over_ready() {
            let startup = Startup::new().unwrap();
            unsafe { SetEvent(startup.ready.handle); SetEvent(startup.failed.handle); }
            assert!(startup.wait_for(&mut child().0, Duration::from_secs(1), &|| true).is_err());
        }
        #[test]
        fn early_exit_does_not_wait_for_timeout() {
            let startup = Startup::new().unwrap();
            use std::os::windows::process::CommandExt;
            let mut process = std::process::Command::new("cmd.exe").args(["/c", "exit 0"])
                .creation_flags(0x08000000).spawn().unwrap();
            process.wait().unwrap();
            let error = startup.wait_for(&mut process, Duration::from_secs(60), &|| true).unwrap_err().to_string();
            assert!(error.contains("closed before the client fixes started") && error.contains("antivirus"), "{error}");
        }
        #[test]
        fn proxy_taken_away_stops_the_wait_with_the_antivirus_message() {
            let startup = Startup::new().unwrap();
            let mut process = child();
            let started = Instant::now();
            let error = startup.wait_for(&mut process.0, Duration::from_secs(30), &|| false).unwrap_err().to_string();
            assert!(error.contains("XAPOFX1_5.dll") && error.contains("antivirus"), "{error}");
            assert!(started.elapsed() >= Duration::from_secs(5) && started.elapsed() < Duration::from_secs(10));
        }
        #[test]
        fn game_gone_without_its_proxy_names_the_antivirus() {
            let startup = Startup::new().unwrap();
            use std::os::windows::process::CommandExt;
            let mut process = std::process::Command::new("cmd.exe").args(["/c", "exit 0"])
                .creation_flags(0x08000000).spawn().unwrap();
            process.wait().unwrap();
            let error = startup.wait_for(&mut process, Duration::from_secs(60), &|| false).unwrap_err().to_string();
            assert!(error.contains("antivirus"), "{error}");
        }
        #[test]
        fn fixes_ready_wins_over_a_missing_proxy() {
            let startup = Startup::new().unwrap();
            unsafe { SetEvent(startup.ready.handle); }
            let mut process = child();
            assert!(startup.wait_for(&mut process.0, Duration::from_secs(2), &|| false).is_ok());
        }
        #[test]
        fn native_proxy_acknowledges_only_opted_in_success() {
            let Some(driver) = std::env::var_os("SP_PROXY_TEST_DRIVER") else { return; };
            use std::os::windows::process::CommandExt;
            for (mode, enabled, success) in [("launcher-ready", true, true),
                                           ("launcher-failed", true, false),
                                           ("launcher-disabled", false, false)] {
                let startup = Startup::new().unwrap();
                let mut command = std::process::Command::new(&driver);
                command.arg(mode).creation_flags(0x08000000);
                command.envs(super::super::environment(enabled, false));
                command.envs(startup.environment());
                let mut process = ChildGuard(command.spawn().unwrap());
                assert_eq!(startup.wait_for(&mut process.0, Duration::from_secs(2), &|| true).is_ok(), success, "{mode}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_launch_overrides_all_ambient_settings() {
        let values = environment(false, true);
        assert_eq!(values[0].1, "0");
        assert_eq!(values[1].1, "0");
        assert!(values[2].1.is_empty() && values[3].1.is_empty());
        assert_eq!(environment(true, false)[0].1, "1");
        assert_eq!(environment(true, false)[1].1, "0");
    }
}
