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

/// Whether the viewer runs on Windows build `build` (0: the build could not
/// be read).
pub fn supported(build: u32) -> bool {
    build >= MIN_BUILD
}

pub fn is_supported() -> bool {
    supported(build_number())
}

/// The localized message with which the viewer and the setup refuse to run
/// on Windows build `build`, or `None` where it is supported.
pub fn refusal(build: u32) -> Option<&'static str> {
    (!supported(build)).then(|| blinzel_i18n::tr(blinzel_i18n::Msg::ErrorOs))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `docs/specs/platform-shells.md` AC-01: 24H2 (26100) and newer run,
    /// 23H2 (22631) and an unreadable build do not.
    #[test]
    fn windows_11_24h2_and_newer_are_supported() {
        for build in [0, 19045, 22631, 26099] {
            assert!(!supported(build), "{build}");
        }
        for build in [26100, 26200, 30000] {
            assert!(supported(build), "{build}");
        }
    }

    /// `docs/specs/platform-shells.md` AC-01: the real build number. The
    /// test executable has no manifest, so a shimmed version API would say
    /// Windows 8 (build 9200); the runner has 24H2 or newer.
    #[test]
    fn the_build_number_is_the_real_one() {
        let build = build_number();
        assert!(build >= MIN_BUILD, "build {build}");
        assert!(is_supported());
    }
}
