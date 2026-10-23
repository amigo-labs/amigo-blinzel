//! Supported Windows versions: Windows 11 24H2 (build 26100) and newer,
//! i.e. only releases Microsoft still services.

pub const MIN_BUILD: u32 = 26100;

#[repr(C)]
struct OsVersionInfo {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    csd: [u16; 128],
}

/// The real Windows build number (`RtlGetVersion`, which unlike
/// `GetVersionEx` is never shimmed).
pub fn build_number() -> u32 {
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::core::{s, w};
    unsafe {
        let Ok(ntdll) = GetModuleHandleW(w!("ntdll.dll")) else {
            return 0;
        };
        let Some(f) = GetProcAddress(ntdll, s!("RtlGetVersion")) else {
            return 0;
        };
        let rtl_get_version: unsafe extern "system" fn(*mut OsVersionInfo) -> i32 =
            std::mem::transmute(f);
        let mut v = OsVersionInfo {
            size: std::mem::size_of::<OsVersionInfo>() as u32,
            major: 0,
            minor: 0,
            build: 0,
            platform: 0,
            csd: [0; 128],
        };
        if rtl_get_version(&mut v) == 0 {
            v.build
        } else {
            0
        }
    }
}

pub fn is_supported() -> bool {
    build_number() >= MIN_BUILD
}
