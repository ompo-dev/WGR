//! Windows adaptation of the GPL-3.0-or-later Muguet host for WGR.
use std::fs::{File, Metadata};
use std::io::{self, ErrorKind};
use std::os::windows::fs::FileExt;
use std::path::Path;
use std::time::UNIX_EPOCH;
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
    THREAD_PRIORITY_NORMAL,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

pub fn footprint_mb() -> u64 {
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of_val(&counters) as u32;
    if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) } == 0 {
        return 0;
    }
    counters.WorkingSetSize as u64 >> 20
}

// These Mach-only counters are unavailable here; they are not process RAM measurements.
pub fn heap_resident_mb() -> u64 {
    0
}
pub fn compressed_mb() -> u64 {
    0
}
pub fn malloc_mb() -> u64 {
    0
}
pub fn gpu_allocated_mb() -> u64 {
    0
}
pub fn set_qos(interactive: bool) {
    unsafe {
        SetThreadPriority(
            GetCurrentThread(),
            if interactive {
                THREAD_PRIORITY_ABOVE_NORMAL
            } else {
                THREAD_PRIORITY_NORMAL
            },
        );
    }
}
pub fn alert(title: &str, message: &str) {
    // Automated validation consumes stderr and must never block on an invisible dialog.
    eprintln!("[native-error] {title}: {message}");
    if std::env::var_os("WGR_NONINTERACTIVE").is_some() {
        return;
    }
    let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
    let message: Vec<u16> = message.encode_utf16().chain([0]).collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub trait PositionedFile {
    fn read_at(&self, buffer: &mut [u8], offset: u64) -> io::Result<usize>;
    fn read_exact_at(&self, buffer: &mut [u8], offset: u64) -> io::Result<()>;
    fn write_all_at(&self, buffer: &[u8], offset: u64) -> io::Result<()>;
}
impl PositionedFile for File {
    fn read_at(&self, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
        self.seek_read(buffer, offset)
    }
    fn read_exact_at(&self, mut buffer: &mut [u8], mut offset: u64) -> io::Result<()> {
        while !buffer.is_empty() {
            match self.seek_read(buffer, offset) {
                Ok(0) => return Err(io::Error::from(ErrorKind::UnexpectedEof)),
                Ok(n) => {
                    buffer = &mut buffer[n..];
                    offset += n as u64;
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    fn write_all_at(&self, mut buffer: &[u8], mut offset: u64) -> io::Result<()> {
        while !buffer.is_empty() {
            match self.seek_write(buffer, offset) {
                Ok(0) => return Err(io::Error::from(ErrorKind::WriteZero)),
                Ok(n) => {
                    buffer = &buffer[n..];
                    offset += n as u64;
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

pub trait GuestMetadata {
    fn atime(&self) -> i64;
    fn mtime(&self) -> i64;
    fn ctime(&self) -> i64;
    fn ino(&self) -> u64;
}
fn seconds(time: io::Result<std::time::SystemTime>) -> i64 {
    time.ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}
impl GuestMetadata for Metadata {
    fn atime(&self) -> i64 {
        seconds(self.accessed())
    }
    fn mtime(&self) -> i64 {
        seconds(self.modified())
    }
    fn ctime(&self) -> i64 {
        seconds(self.created())
    }
    // The guest's virtual mount does not expose a Windows file identifier.
    fn ino(&self) -> u64 {
        0
    }
}
pub fn statfs_bytes(path: &Path) -> Result<[u8; 120], i64> {
    use std::os::windows::ffi::OsStrExt;
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let (mut available, mut total, mut free) = (0, 0, 0);
    if unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
        return Err(match std::io::Error::last_os_error().raw_os_error() {
            Some(2 | 3) => 44,
            Some(5) => 2,
            _ => 29,
        });
    }
    let mut bytes = [0u8; 120];
    for (offset, value) in [
        (8, 4096),
        (16, total / 4096),
        (24, free / 4096),
        (32, available / 4096),
        (64, 255),
        (72, 4096),
    ] {
        bytes[offset..offset + 8].copy_from_slice(&u64::to_le_bytes(value));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn reports_real_disk_space_to_memory64_guest() {
        let root = std::env::temp_dir();
        let bytes = super::statfs_bytes(&root).unwrap();
        let field = |offset| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        assert!(field(8) > 0);
        assert!(field(32) <= field(24));
        assert!(field(24) <= field(16));
        assert!(field(32) * field(8) > 1024 * 1024);
        assert_eq!(
            super::statfs_bytes(&root.join(format!("wgr-missing-{}", std::process::id()))),
            Err(44)
        );
    }
}
