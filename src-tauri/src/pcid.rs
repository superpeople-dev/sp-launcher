//! A one-way code for this PC, sent with Play (auth::discord_launch), so that a
//! ban follows the PC and not just the Discord account: a banned cheater who
//! makes a new Discord account on the same PC is still refused.
//!
//! WHAT IS READ
//! ------------
//! Three identifiers that stay the same across Discord accounts and launcher
//! reinstalls, all readable without admin rights:
//!   board    the motherboard's system UUID (SMBIOS, from the firmware);
//!   disk     the serial number of the disk Windows is installed on;
//!   windows  the ID Windows made for itself when it was installed
//!            (HKLM\SOFTWARE\Microsoft\Cryptography, MachineGuid).
//!
//! WHAT LEAVES THE PC
//! ------------------
//! Only a SHA-256 of each one, never the value itself. The backend can tell
//! "same PC as before" and nothing more: the code cannot be turned back into
//! the serial number. The raw values never leave this file: they are never
//! logged, stored, shown to the web UI or sent anywhere, and only the codes
//! are kept in memory. Each code is hashed with its kind and a version in front
//! ("superpeople-pc-v1|disk|..."), so a disk code is not a board code, and a
//! later version can read something else without mixing with these.
//!
//! Values that many PCs share (a blank UUID, a disk serial of "To be filled by
//! O.E.M.") are dropped instead of hashed: banning one of those PCs would ban
//! them all. Read once per launcher run, like hardware.rs.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// What is read and how it is hashed. Changing either is a new version, never
/// a silent change: the backend keeps the codes it has seen per version.
pub const VERSION: u32 = 1;

/// This PC's codes, as Play sends them (`"pc"`). Each one is the lowercase hex
/// SHA-256, or None when it could not be read or is one many PCs share; a None
/// is left out of the JSON, `v` never is.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Codes {
    pub v: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub board: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub windows: Option<String>,
}

impl Default for Codes {
    fn default() -> Self {
        Codes {
            v: VERSION,
            board: None,
            disk: None,
            windows: None,
        }
    }
}

pub fn codes() -> Codes {
    static READ: OnceLock<Codes> = OnceLock::new();
    READ.get_or_init(read).clone()
}

/// Reads, filters and hashes each identifier. The raw values go no further
/// than this function.
fn read() -> Codes {
    Codes {
        v: VERSION,
        board: os::board_uuid()
            .and_then(|raw| keep_board(&raw))
            .map(|value| code("board", &value)),
        disk: os::disk_serial()
            .and_then(|raw| keep_disk(&raw))
            .map(|value| code("disk", &value)),
        windows: os::machine_guid()
            .and_then(|raw| keep_windows(&raw))
            .map(|value| code("windows", &value)),
    }
}

/// The one-way code: lowercase hex SHA-256 of `superpeople-pc-v1|kind|value`.
fn code(kind: &str, value: &str) -> String {
    let digest = Sha256::digest(format!("superpeople-pc-v{VERSION}|{kind}|{value}").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The same identifier must give the same code however the firmware, driver or
/// registry spaced or cased it: no whitespace anywhere (which trims it too),
/// upper case.
fn normalise(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_uppercase()
}

/// True when every hex digit is the same one (none at all counts too):
/// 00000000-0000-..., FFFFFFFF-FFFF-..., 11111111-1111-...
fn one_repeated_hex_digit(value: &str) -> bool {
    let mut digits = value.chars().filter(char::is_ascii_hexdigit);
    match digits.next() {
        Some(first) => digits.all(|d| d == first),
        None => true,
    }
}

/// A system UUID worth keeping. Blank ones (all 0 or all F, "not set" in the
/// SMBIOS spec) and the well-known placeholder some boards ship with are not:
/// thousands of PCs have them.
fn keep_board(raw: &str) -> Option<String> {
    const PLACEHOLDER: &str = "03000200-0400-0500-0006-000700080009";
    let value = normalise(raw);
    if one_repeated_hex_digit(&value) || value == PLACEHOLDER {
        return None;
    }
    Some(value)
}

/// Disk serials that motherboard and drive makers leave as a placeholder, as
/// letters and digits only, so "To Be Filled By O.E.M." and "TO BE FILLED BY
/// OEM" are the same entry.
const DISK_PLACEHOLDERS: &[&str] = &[
    "TOBEFILLEDBYOEM",
    "DEFAULTSTRING",
    "NONE",
    "NA",
    "0123456789",
    "123456789",
    "1234567890",
    "SERIALNUMBER",
    "SYSTEMSERIALNUMBER",
    "NOTSPECIFIED",
    "NOTAPPLICABLE",
    "UNKNOWN",
];

/// A disk serial worth keeping: not empty, not only punctuation, not one
/// character repeated (0000000000), not too short to tell disks apart, and
/// not a placeholder.
fn keep_disk(raw: &str) -> Option<String> {
    let value = normalise(raw);
    let letters: String = value.chars().filter(char::is_ascii_alphanumeric).collect();
    let first = letters.chars().next()?;
    if letters.len() < 4
        || letters.chars().all(|c| c == first)
        || DISK_PLACEHOLDERS.contains(&letters.as_str())
    {
        return None;
    }
    Some(value)
}

/// Windows' MachineGuid, unless it is missing or blank.
fn keep_windows(raw: &str) -> Option<String> {
    let value = normalise(raw);
    if one_repeated_hex_digit(&value) {
        return None;
    }
    Some(value)
}

// ----------------------------------------------------- reading the bytes ---
// What Windows hands back, taken apart byte by byte rather than through
// pointer casts: the buffers come from a firmware or a driver, and a short or
// odd one has to end in None, never in a crash. The offsets are checked
// against windows-sys's own structs in `os`.

/// Where STORAGE_DEVICE_DESCRIPTOR keeps SerialNumberOffset.
const SERIAL_OFFSET_AT: usize = 24;
/// Where VOLUME_DISK_EXTENTS keeps its first extent's DiskNumber.
const FIRST_DISK_AT: usize = 8;

fn u32_at(buf: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(buf.get(at..at + 4)?.try_into().ok()?))
}

/// The system UUID from GetSystemFirmwareTable('RSMB'): an 8-byte header
/// (RawSMBIOSData, with the table's length at byte 4), then the SMBIOS
/// structures. Each has a type, its length and a handle, its fields, then
/// its strings, which end with two NULs. Type 1, System Information, holds
/// the UUID at offset 8 (SMBIOS 2.1 and later); type 127 ends the table.
///
/// Written the way Windows shows it (wmic csproduct get uuid): the first
/// three fields little-endian, as SMBIOS 2.6 and later define them.
fn smbios_uuid(raw: &[u8]) -> Option<String> {
    let length = u32_at(raw, 4)? as usize;
    let table = raw.get(8..raw.len().min(8usize.saturating_add(length)))?;
    let mut at = 0;
    while at + 4 <= table.len() {
        let (kind, size) = (table[at], table[at + 1] as usize);
        if size < 4 {
            return None;
        }
        if kind == 1 && size >= 0x19 {
            let b = table.get(at + 8..at + 24)?;
            return Some(format!(
                "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
                b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
            ));
        }
        if kind == 127 {
            return None;
        }
        let mut next = at + size;
        while next + 1 < table.len() && (table[next] != 0 || table[next + 1] != 0) {
            next += 1;
        }
        at = next + 2;
    }
    None
}

/// The serial number in a STORAGE_DEVICE_DESCRIPTOR: a NUL-terminated string
/// somewhere in the buffer, at SerialNumberOffset (0 when the disk has none).
fn descriptor_serial(buf: &[u8]) -> Option<String> {
    let offset = u32_at(buf, SERIAL_OFFSET_AT)? as usize;
    if offset == 0 {
        return None;
    }
    let rest = buf.get(offset..)?;
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// The disk a volume starts on, from VOLUME_DISK_EXTENTS.
fn first_disk(buf: &[u8]) -> Option<u32> {
    if u32_at(buf, 0)? == 0 {
        return None;
    }
    u32_at(buf, FIRST_DISK_AT)
}

#[cfg(windows)]
mod os {
    //! The three reads. Nothing here needs admin rights: the firmware table is
    //! open to every user, the disk and volume are opened with no access
    //! rights at all (both questions asked of them are FILE_ANY_ACCESS), and
    //! MachineGuid is readable by every user.
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
        OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Ioctl::{
        PropertyStandardQuery, StorageDeviceProperty, DISK_EXTENT, IOCTL_STORAGE_QUERY_PROPERTY,
        STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY, VOLUME_DISK_EXTENTS,
    };
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY,
    };
    use windows_sys::Win32::System::SystemInformation::{
        GetSystemFirmwareTable, GetSystemWindowsDirectoryW, RSMB,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    use std::mem::offset_of;

    // The byte offsets the parsers read, checked against the real structs.
    const _: () = assert!(
        offset_of!(STORAGE_DEVICE_DESCRIPTOR, SerialNumberOffset) == super::SERIAL_OFFSET_AT
    );
    const _: () = assert!(
        offset_of!(VOLUME_DISK_EXTENTS, Extents) + offset_of!(DISK_EXTENT, DiskNumber)
            == super::FIRST_DISK_AT
    );

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn board_uuid() -> Option<String> {
        // SAFETY: no buffer and a size of 0 asks only how big the table is.
        let size = unsafe { GetSystemFirmwareTable(RSMB, 0, std::ptr::null_mut(), 0) };
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        // SAFETY: buf holds `size` bytes, as passed.
        let read = unsafe { GetSystemFirmwareTable(RSMB, 0, buf.as_mut_ptr(), size) };
        if read == 0 || read > size {
            return None;
        }
        buf.truncate(read as usize);
        super::smbios_uuid(&buf)
    }

    /// The serial of the disk Windows is on. PhysicalDrive0 only when that
    /// disk cannot be found: it is the system disk on most PCs, not all.
    pub fn disk_serial() -> Option<String> {
        let disk = Device::open(&format!(r"\\.\PhysicalDrive{}", system_disk().unwrap_or(0)))?;
        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        let mut buf = vec![0u8; 1024];
        let mut read = disk.ask(IOCTL_STORAGE_QUERY_PROPERTY, Some(&query), &mut buf)?;
        // The descriptor says how big it is whole; a long list of the
        // drive's own properties can need more than 1 KB.
        let size = super::u32_at(&buf, 4)? as usize;
        if size > buf.len() && size <= 64 * 1024 {
            buf = vec![0u8; size];
            read = disk.ask(IOCTL_STORAGE_QUERY_PROPERTY, Some(&query), &mut buf)?;
        }
        super::descriptor_serial(&buf[..read.min(buf.len())])
    }

    /// The PhysicalDrive number under the drive Windows is installed on (C:
    /// on most PCs, not all).
    fn system_disk() -> Option<u32> {
        let mut dir = [0u16; 260];
        // SAFETY: dir holds 260 UTF-16 units, as passed.
        let len =
            unsafe { GetSystemWindowsDirectoryW(dir.as_mut_ptr(), dir.len() as u32) } as usize;
        if len < 2 || len >= dir.len() || dir[1] != u16::from(b':') {
            return None;
        }
        let letter = char::from_u32(u32::from(dir[0])).filter(char::is_ascii_alphabetic)?;
        let volume = Device::open(&format!(r"\\.\{letter}:"))?;
        // Room for 16 extents: a volume spread over several disks starts on
        // the first one.
        let mut buf = vec![0u8; 8 + 16 * std::mem::size_of::<DISK_EXTENT>()];
        let read = volume.ask(IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, None::<&()>, &mut buf)?;
        super::first_disk(&buf[..read.min(buf.len())])
    }

    pub fn machine_guid() -> Option<String> {
        let key = wide(r"SOFTWARE\Microsoft\Cryptography");
        let value = wide("MachineGuid");
        let mut buf = [0u16; 128];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: key and value are NUL-terminated; buf holds `size` bytes,
        // and RegGetValueW writes at most that much (and NUL-terminates).
        // RRF_SUBKEY_WOW6464KEY: the 64-bit registry, where Windows keeps it,
        // whatever this build is.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != 0 {
            return None;
        }
        let len = (size as usize / 2).min(buf.len());
        Some(
            String::from_utf16_lossy(&buf[..len])
                .trim_end_matches('\0')
                .to_string(),
        )
    }

    /// A disk or volume opened with no access rights: enough to ask it what
    /// it is, not to read or write it. Closed when dropped.
    struct Device(HANDLE);

    impl Device {
        fn open(path: &str) -> Option<Device> {
            let path = wide(path);
            // SAFETY: path is NUL-terminated; no security attributes, no
            // template file.
            let handle = unsafe {
                CreateFileW(
                    path.as_ptr(),
                    0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            // Checked before a Device exists: dropping one closes its handle,
            // and a handle that failed to open is not ours to close.
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return None;
            }
            Some(Device(handle))
        }

        /// One DeviceIoControl; how many bytes came back.
        fn ask<T>(&self, code: u32, input: Option<&T>, output: &mut [u8]) -> Option<usize> {
            let (input, input_len) = match input {
                Some(value) => ((value as *const T).cast(), std::mem::size_of::<T>() as u32),
                None => (std::ptr::null(), 0),
            };
            let mut returned = 0u32;
            // SAFETY: input and output are valid for the sizes passed; the
            // handle was opened without FILE_FLAG_OVERLAPPED, so the call
            // finishes before it returns and needs no OVERLAPPED.
            let ok = unsafe {
                DeviceIoControl(
                    self.0,
                    code,
                    input,
                    input_len,
                    output.as_mut_ptr().cast(),
                    output.len() as u32,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            };
            (ok != 0).then_some(returned as usize)
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateFileW and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(not(windows))]
mod os {
    //! The launcher ships for Windows only; elsewhere there is nothing to
    //! read, and Play sends `{"v": 1}` alone.
    pub fn board_uuid() -> Option<String> {
        None
    }
    pub fn disk_serial() -> Option<String> {
        None
    }
    pub fn machine_guid() -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_normalised_before_hashing() {
        assert_eq!(normalise("  ab12 Cd34\t\r\n"), "AB12CD34");
        assert_eq!(
            keep_disk(" wd-wcc4 e123 4567 "),
            Some("WD-WCC4E1234567".into())
        );
        assert_eq!(
            keep_disk("0025_3881_0123_4567."),
            Some("0025_3881_0123_4567.".into())
        );
        assert_eq!(
            keep_board(" 12345678-90ab-cdef-1234-567890abcdef\n"),
            Some("12345678-90AB-CDEF-1234-567890ABCDEF".into())
        );
        assert_eq!(
            keep_windows("1b2c3d4e-5f60-7182-93a4-b5c6d7e8f901"),
            Some("1B2C3D4E-5F60-7182-93A4-B5C6D7E8F901".into())
        );
        // Spaced or cased differently, the same disk gives the same code.
        let a = keep_disk(" s3z1 abc 9 ").map(|v| code("disk", &v));
        let b = keep_disk("S3Z1ABC9").map(|v| code("disk", &v));
        assert!(a.is_some());
        assert_eq!(a, b);
    }

    #[test]
    fn board_uuids_many_pcs_share_are_dropped() {
        assert_eq!(keep_board(""), None);
        assert_eq!(keep_board("00000000-0000-0000-0000-000000000000"), None);
        assert_eq!(keep_board("FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF"), None);
        assert_eq!(keep_board("ffffffff-ffff-ffff-ffff-ffffffffffff"), None);
        assert_eq!(keep_board("11111111-1111-1111-1111-111111111111"), None);
        assert_eq!(keep_board("03000200-0400-0500-0006-000700080009"), None);
        assert_eq!(keep_board(" 03000200-0400-0500-0006-000700080009 "), None);
    }

    #[test]
    fn disk_serials_many_pcs_share_are_dropped() {
        for junk in [
            "",
            "   ",
            "0",
            "0000000000",
            "0000-0000-0000",
            "----",
            "._-/",
            "To Be Filled By O.E.M.",
            "TO BE FILLED BY OEM",
            "Default string",
            "None",
            "N/A",
            "0123456789",
            "SerialNumber",
            "Serial Number",
            "ABC",
        ] {
            assert_eq!(keep_disk(junk), None, "{junk:?}");
        }
    }

    #[test]
    fn a_blank_machine_guid_is_dropped() {
        assert_eq!(keep_windows(""), None);
        assert_eq!(keep_windows(" \t "), None);
        assert_eq!(keep_windows("00000000-0000-0000-0000-000000000000"), None);
    }

    #[test]
    fn codes_are_one_way_and_always_the_same() {
        // A made-up UUID; the hash checked with sha256sum.
        assert_eq!(
            code("board", "12345678-90AB-CDEF-1234-567890ABCDEF"),
            "ce5431884edcd0327bee604484c45b4e1d6de224283fcf6d40a79e82c7be61a7"
        );
        assert_eq!(code("disk", "S3Z1ABC9"), code("disk", "S3Z1ABC9"));
        // The kind is part of it: the same text is a different code per kind.
        assert_ne!(code("disk", "S3Z1ABC9"), code("board", "S3Z1ABC9"));
        let c = code("windows", "1B2C3D4E-5F60-7182-93A4-B5C6D7E8F901");
        assert_eq!(c.len(), 64);
        assert!(c.chars().all(|ch| matches!(ch, '0'..='9' | 'a'..='f')));
        assert!(!c.contains("1B2C3D4E"));
    }

    #[test]
    fn missing_codes_are_left_out_and_v_is_always_there() {
        assert_eq!(
            serde_json::to_value(Codes::default()).unwrap(),
            serde_json::json!({ "v": 1 })
        );
        let some = Codes {
            disk: Some("ab".into()),
            ..Codes::default()
        };
        assert_eq!(
            serde_json::to_value(some).unwrap(),
            serde_json::json!({ "v": 1, "disk": "ab" })
        );
    }

    /// A made-up firmware table: a BIOS structure with a string, a System
    /// Information structure with two strings, the end of the table.
    fn smbios_table(uuid: [u8; 16]) -> Vec<u8> {
        let mut table = vec![0u8, 4, 0, 0];
        table.extend_from_slice(b"Vendor\0\0");
        table.extend_from_slice(&[1, 0x1B, 1, 0, 1, 2, 0, 0]);
        table.extend_from_slice(&uuid);
        table.extend_from_slice(&[6, 0, 0]);
        table.extend_from_slice(b"Maker\0Model\0\0");
        table.extend_from_slice(&[127, 4, 0xFF, 0xFE, 0, 0]);
        let mut raw = vec![0u8, 3, 4, 0];
        raw.extend_from_slice(&(table.len() as u32).to_le_bytes());
        raw.extend_from_slice(&table);
        raw
    }

    #[test]
    fn the_uuid_is_found_in_the_firmware_table() {
        let uuid = [
            0x78, 0x56, 0x34, 0x12, 0xAB, 0x90, 0xEF, 0xCD, 0x12, 0x34, 0x56, 0x78, 0x90, 0xAB,
            0xCD, 0xEF,
        ];
        assert_eq!(
            smbios_uuid(&smbios_table(uuid)).as_deref(),
            Some("12345678-90AB-CDEF-1234-567890ABCDEF")
        );
        // The placeholder, as its bytes sit in the firmware, reads as Windows
        // shows it, so keep_board can drop it.
        let placeholder = [0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0, 7, 0, 8, 0, 9];
        let read = smbios_uuid(&smbios_table(placeholder)).unwrap();
        assert_eq!(read, "03000200-0400-0500-0006-000700080009");
        assert_eq!(keep_board(&read), None);
    }

    #[test]
    fn a_short_or_odd_firmware_table_is_none_not_a_crash() {
        let raw = smbios_table([7; 16]);
        for len in 0..raw.len() - 1 {
            let _ = smbios_uuid(&raw[..len]);
        }
        assert_eq!(smbios_uuid(&[]), None);
        // No System Information structure at all.
        let mut no_type_1 = vec![0u8, 3, 4, 0, 14, 0, 0, 0];
        no_type_1.extend_from_slice(&[0, 4, 0, 0, 0, 0, 127, 4, 0, 0, 0, 0, 0, 0]);
        assert_eq!(smbios_uuid(&no_type_1), None);
        // A structure claiming a length below its own header.
        assert_eq!(
            smbios_uuid(&[0, 3, 4, 0, 6, 0, 0, 0, 1, 2, 0, 0, 0, 0]),
            None
        );
    }

    #[test]
    fn the_serial_is_found_in_the_device_descriptor() {
        let mut buf = vec![0u8; 64];
        buf[SERIAL_OFFSET_AT..SERIAL_OFFSET_AT + 4].copy_from_slice(&40u32.to_le_bytes());
        buf[40..52].copy_from_slice(b"  AB12 CD34\0");
        assert_eq!(descriptor_serial(&buf).as_deref(), Some("  AB12 CD34"));
        // No serial (offset 0), or an offset past the end.
        buf[SERIAL_OFFSET_AT..SERIAL_OFFSET_AT + 4].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(descriptor_serial(&buf), None);
        buf[SERIAL_OFFSET_AT..SERIAL_OFFSET_AT + 4].copy_from_slice(&500u32.to_le_bytes());
        assert_eq!(descriptor_serial(&buf), None);
        assert_eq!(descriptor_serial(&buf[..10]), None);
    }

    #[test]
    fn the_system_disk_number_is_read_from_the_extents() {
        let mut buf = vec![0u8; 32];
        buf[0..4].copy_from_slice(&1u32.to_le_bytes());
        buf[FIRST_DISK_AT..FIRST_DISK_AT + 4].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(first_disk(&buf), Some(2));
        buf[0..4].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(first_disk(&buf), None);
        assert_eq!(first_disk(&buf[..6]), None);
    }

    #[test]
    fn this_pc_is_read() {
        let pc = codes();
        // Whether each one was found, never a value or a code.
        eprintln!(
            "pc codes found: board {}, disk {}, windows {}",
            pc.board.is_some(),
            pc.disk.is_some(),
            pc.windows.is_some()
        );
        assert_eq!(pc.v, VERSION);
        for c in [&pc.board, &pc.disk, &pc.windows].into_iter().flatten() {
            assert_eq!(c.len(), 64);
            assert!(c.chars().all(|ch| matches!(ch, '0'..='9' | 'a'..='f')));
        }
        #[cfg(windows)]
        assert!(
            pc.windows.is_some(),
            "every Windows install has a MachineGuid"
        );
        assert_eq!(codes(), pc, "read once, the same every time");
    }
}
