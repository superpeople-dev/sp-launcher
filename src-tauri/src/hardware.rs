//! What the player's PC is, for the team's #launcher-logs when the game starts
//! (lib.rs launch_game, auth::report): processor, graphics cards, memory,
//! Windows, screen. Nothing that tells PCs apart -- no name, serial number or
//! network address. Read once per launcher run.

use serde::Serialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Hardware {
    /// "AMD Ryzen 7 7800X3D 8-Core Processor".
    pub cpu: String,
    /// Logical processors.
    pub threads: usize,
    /// Installed memory, in GB (rounded).
    pub ram_gb: u64,
    pub gpus: Vec<Gpu>,
    /// "Windows 11 Pro 24H2 (build 26200)".
    pub os: String,
    /// The main screen, "2560x1440", and how many screens there are.
    pub screen: String,
    pub screens: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Gpu {
    pub name: String,
    /// Dedicated video memory in GB, when the driver says.
    pub vram_gb: Option<u64>,
}

pub fn summary() -> Hardware {
    static READ: OnceLock<Hardware> = OnceLock::new();
    READ.get_or_init(read).clone()
}

/// Display adapters that are not a graphics card: Windows' fallback driver,
/// remote desktops, virtual screens (VR headsets, streaming, virtual machines).
fn real_gpu(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !name.is_empty() && !["basic render", "basic display", "remote display", "remote desktop", "virtual", "indirect display"].iter().any(|skip| name.contains(skip))
}

/// Windows 11 still calls itself "Windows 10" in the registry; the build number tells.
fn windows_name(product: &str, display: &str, build: &str) -> String {
    let product = if build.parse::<u32>().unwrap_or(0) >= 22000 { product.replacen("Windows 10", "Windows 11", 1) } else { product.to_string() };
    let mut name = product.trim().to_string();
    if !display.trim().is_empty() {
        name.push(' ');
        name.push_str(display.trim());
    }
    if !build.trim().is_empty() {
        name.push_str(&format!(" (build {})", build.trim()));
    }
    name
}

#[cfg(windows)]
fn read() -> Hardware {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS, SM_CXSCREEN, SM_CYSCREEN};

    const GPU_CLASS: &str = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
    let mut gpus: Vec<Gpu> = Vec::new();
    for i in 0..16 {
        let key = format!(r"{GPU_CLASS}\{i:04}");
        let Some(name) = reg::string(&key, "DriverDesc") else { continue };
        let name = name.trim().to_string();
        if !real_gpu(&name) || gpus.iter().any(|g| g.name == name) {
            continue;
        }
        let vram = reg::qword(&key, "HardwareInformation.qwMemorySize").or_else(|| reg::dword(&key, "HardwareInformation.MemorySize"));
        gpus.push(Gpu { name, vram_gb: vram.filter(|&b| b > 0).map(|b| (b as f64 / 1_073_741_824.0).round() as u64) });
    }

    let mut memory = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..unsafe { std::mem::zeroed() } };
    // SAFETY: `memory` is a valid MEMORYSTATUSEX with dwLength set, as the call requires.
    let ram = if unsafe { GlobalMemoryStatusEx(&mut memory) } != 0 { memory.ullTotalPhys } else { 0 };

    const NT: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let os = windows_name(
        &reg::string(NT, "ProductName").unwrap_or_else(|| "Windows".into()),
        &reg::string(NT, "DisplayVersion").unwrap_or_default(),
        &reg::string(NT, "CurrentBuildNumber").unwrap_or_default(),
    );

    // SAFETY: plain queries, no pointers.
    let (width, height, screens) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN), GetSystemMetrics(SM_CMONITORS)) };

    Hardware {
        cpu: reg::string(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString")
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default(),
        threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        ram_gb: (ram as f64 / 1_073_741_824.0).round() as u64,
        gpus,
        os,
        screen: if width > 0 && height > 0 { format!("{width}x{height}") } else { String::new() },
        screens: screens.max(0) as u32,
    }
}

#[cfg(not(windows))]
fn read() -> Hardware {
    Hardware { threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0), ..Default::default() }
}

/// Reading values under HKEY_LOCAL_MACHINE.
#[cfg(windows)]
mod reg {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_QWORD, RRF_RT_REG_SZ};

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn string(key: &str, value: &str) -> Option<String> {
        let (key, value) = (wide(key), wide(value));
        let mut buf = vec![0u16; 512];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: key and value are NUL-terminated; buf holds `size` bytes,
        // and RegGetValueW writes at most that much (and NUL-terminates).
        let status = unsafe {
            RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size)
        };
        if status != 0 {
            return None;
        }
        let len = (size as usize / 2).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]).trim_end_matches('\0').to_string())
    }

    pub fn qword(key: &str, value: &str) -> Option<u64> {
        let (key, value) = (wide(key), wide(value));
        let mut data = 0u64;
        let mut size = 8u32;
        // SAFETY: as above, with an 8-byte buffer for a QWORD.
        let status = unsafe {
            RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), RRF_RT_REG_QWORD, std::ptr::null_mut(), (&mut data as *mut u64).cast(), &mut size)
        };
        (status == 0).then_some(data)
    }

    pub fn dword(key: &str, value: &str) -> Option<u64> {
        let (key, value) = (wide(key), wide(value));
        let mut data = 0u32;
        let mut size = 4u32;
        // SAFETY: as above, with a 4-byte buffer for a DWORD.
        let status = unsafe {
            RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut data as *mut u32).cast(), &mut size)
        };
        (status == 0).then_some(u64::from(data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_11_is_called_windows_11() {
        assert_eq!(windows_name("Windows 10 Pro", "24H2", "26200"), "Windows 11 Pro 24H2 (build 26200)");
        assert_eq!(windows_name("Windows 10 Home", "22H2", "19045"), "Windows 10 Home 22H2 (build 19045)");
    }

    #[test]
    fn only_real_graphics_cards() {
        assert!(real_gpu("NVIDIA GeForce RTX 4080"));
        assert!(!real_gpu("Microsoft Basic Render Driver"));
        assert!(!real_gpu("Parsec Virtual Display Adapter"));
        assert!(!real_gpu("Meta Virtual Monitor"));
        assert!(!real_gpu("Virtual Desktop Monitor"));
        assert!(!real_gpu(""));
    }

    #[test]
    fn this_pc_is_read() {
        let pc = summary();
        assert!(pc.threads > 0);
        #[cfg(windows)]
        {
            assert!(!pc.cpu.is_empty(), "{pc:?}");
            assert!(pc.ram_gb > 0, "{pc:?}");
            assert!(pc.os.starts_with("Windows"), "{pc:?}");
            eprintln!("{pc:#?}");
        }
    }
}
