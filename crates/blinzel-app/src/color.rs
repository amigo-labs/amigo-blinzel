//! Colour management on Windows: which profile the monitor under the
//! window wants, and colour transforms (3D LUTs) from image profiles to it,
//! built by the Windows Color System (`mscms`, every ICC profile kind).
//! `blinzel_ui::color::ColorManager` decides when one is needed.
//!
//! sRGB images on an sRGB target (the usual case) need nothing: no DLL is
//! loaded and the shader skips the lookup. With Windows' own colour
//! management active on a monitor (Auto Color Management or HDR), DWM
//! converts from sRGB itself, so the target is sRGB there.
//!
//! Asking Windows for a monitor's profile loads the ICM subsystem (about
//! 7 ms that would compete with the start), so a start uses the answer
//! remembered from the last one and asks again after the first frame.

use std::path::{Path, PathBuf};

use blinzel_core::icc::Profile;
use blinzel_core::lut::{self, Lut3d};
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS,
    QueryDisplayConfig,
};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::Gdi::{
    CreateDCW, DeleteDC, GetMonitorInfoW, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::Storage::FileSystem::{FILE_SHARE_READ, OPEN_EXISTING};
use windows::Win32::UI::ColorSystem::{
    BEST_MODE, BM_16b_RGB, CloseColorProfile, CreateMultiProfileTransform, DeleteColorTransform,
    GetICMProfileW, GetStandardColorSpaceProfileW, INDEX_DONT_CARE, INTENT_RELATIVE_COLORIMETRIC,
    LCS_sRGB, OpenColorProfileW, PROFILE, PROFILE_FILENAME, PROFILE_MEMBUFFER, PROFILE_READ,
    TranslateBitmapBits,
};
use windows::core::{PCWSTR, PWSTR, w};

/// The display device of `monitor` (`\\.\DISPLAY1`), as text and as
/// the NUL-padded UTF-16 the display APIs take.
fn monitor_device(monitor: HMONITOR) -> Option<(String, [u16; 32])> {
    let mut mi = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe { GetMonitorInfoW(monitor, &mut mi.monitorInfo) }
        .ok()
        .ok()?;
    Some((crate::platform::from_wide(&mi.szDevice), mi.szDevice))
}

/// A non-sRGB profile (and its file) to convert to on `device`.
fn query(device: &[u16; 32]) -> Option<(PathBuf, Profile)> {
    if advanced_colour(device) {
        return None;
    }
    let path = icm_profile(device)?;
    let profile = Profile::new(std::fs::read(&path).ok()?);
    (!profile.srgb).then_some((path, profile))
}

/// The profile to convert to on `monitor`; `None` means sRGB (no
/// profile, an sRGB profile, or Windows manages colour there itself).
/// Remembers the answer for the next start.
pub fn display_target(monitor: HMONITOR) -> Option<Profile> {
    let (name, device) = monitor_device(monitor)?;
    let target = query(&device);
    remember(&name, target.as_ref().map(|(path, _)| path.as_path()));
    target.map(|(_, p)| p)
}

/// What [`display_target`] answered for `monitor` last time, without
/// asking Windows; `None` if it never did.
pub fn remembered_target(monitor: HMONITOR) -> Option<Option<Profile>> {
    let (name, _) = monitor_device(monitor)?;
    let text = std::fs::read_to_string(cache_file()?).ok()?;
    let path = text
        .lines()
        .find_map(|l| l.strip_prefix(name.as_str())?.strip_prefix('\t'))?;
    if path.is_empty() {
        return Some(None);
    }
    let profile = Profile::new(std::fs::read(path).ok()?);
    Some((!profile.srgb).then_some(profile))
}

/// `device<TAB>profile path` per line; an empty path means sRGB.
fn cache_file() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(local)
            .join("amigo-blinzel")
            .join("display-color.txt"),
    )
}

fn remember(device: &str, profile: Option<&Path>) {
    let Some(file) = cache_file() else {
        return;
    };
    let old = std::fs::read_to_string(&file).unwrap_or_default();
    let mut lines: Vec<String> = old
        .lines()
        .filter(|l| l.split('\t').next() != Some(device))
        .map(String::from)
        .collect();
    let path = profile.map(|p| p.display().to_string()).unwrap_or_default();
    lines.push(format!("{device}\t{path}"));
    let text = lines.join("\n") + "\n";
    if text != old {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&file, text);
    }
}

/// Whether Windows colour-manages this display (HDR or Auto Color
/// Management): it then expects sRGB from non-HDR apps.
fn advanced_colour(device: &[u16; 32]) -> bool {
    let (mut np, mut nm) = (0u32, 0u32);
    unsafe {
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
            return false;
        }
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
    let r = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut np,
            paths.as_mut_ptr(),
            &mut nm,
            modes.as_mut_ptr(),
            None,
        )
    };
    if r != ERROR_SUCCESS {
        return false;
    }
    let name_len = device.iter().position(|&c| c == 0).unwrap_or(device.len());
    paths.truncate(np as usize);
    paths.iter().any(|p| {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: p.sourceInfo.adapterId,
                id: p.sourceInfo.id,
            },
            ..Default::default()
        };
        if unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } != 0
            || source.viewGdiDeviceName[..name_len] != device[..name_len]
        {
            return false;
        }
        let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                size: std::mem::size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
                adapterId: p.targetInfo.adapterId,
                id: p.targetInfo.id,
            },
            ..Default::default()
        };
        let r = unsafe { DisplayConfigGetDeviceInfo(&mut info.header) };
        // Bit 1: advancedColorEnabled.
        r == 0 && unsafe { info.Anonymous.value } & 2 != 0
    })
}

/// The ICM profile file Windows associates with a display device.
fn icm_profile(device: &[u16; 32]) -> Option<PathBuf> {
    unsafe {
        let hdc = CreateDCW(w!("DISPLAY"), PCWSTR(device.as_ptr()), PCWSTR::null(), None);
        if hdc.is_invalid() {
            return None;
        }
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let ok = GetICMProfileW(hdc, &mut len, Some(PWSTR(buf.as_mut_ptr()))).as_bool();
        let _ = DeleteDC(hdc);
        ok.then(|| PathBuf::from(crate::platform::from_wide(&buf)))
    }
}

/// Owned mscms handles.
struct Handle(isize, unsafe fn(isize) -> bool);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { (self.1)(self.0) };
    }
}

unsafe fn close_profile(h: isize) -> bool {
    unsafe { CloseColorProfile(Some(h)).as_bool() }
}

unsafe fn delete_transform(h: isize) -> bool {
    unsafe { DeleteColorTransform(h).as_bool() }
}

/// Opens `profile`, or Windows' sRGB profile for `None`.
fn open(profile: Option<&Profile>) -> Option<Handle> {
    let mut path = [0u16; 260];
    let desc = match profile {
        Some(p) => PROFILE {
            dwType: PROFILE_MEMBUFFER,
            pProfileData: p.bytes.as_ptr() as *mut _,
            cbDataSize: p.bytes.len() as u32,
        },
        None => {
            let mut size = (path.len() * 2) as u32;
            let ok = unsafe {
                GetStandardColorSpaceProfileW(
                    PCWSTR::null(),
                    LCS_sRGB.0 as u32,
                    Some(PWSTR(path.as_mut_ptr())),
                    &mut size,
                )
            };
            if !ok.as_bool() {
                return None;
            }
            PROFILE {
                dwType: PROFILE_FILENAME,
                pProfileData: path.as_mut_ptr().cast(),
                cbDataSize: size,
            }
        }
    };
    let h = unsafe { OpenColorProfileW(&desc, PROFILE_READ, FILE_SHARE_READ.0, OPEN_EXISTING.0) };
    (h != 0).then_some(Handle(h, close_profile))
}

/// The transform from `src` to `dst` (`None`: sRGB) sampled as a 3D LUT,
/// relative colorimetric.
pub fn build_lut(src: Option<&Profile>, dst: Option<&Profile>) -> Option<Lut3d> {
    let (s, d) = (open(src)?, open(dst)?);
    let xform = unsafe {
        CreateMultiProfileTransform(
            &[s.0, d.0],
            &[INTENT_RELATIVE_COLORIMETRIC],
            BEST_MODE,
            INDEX_DONT_CARE,
        )
    };
    if xform == 0 {
        return None;
    }
    let xform = Handle(xform, delete_transform);
    let n = lut::SIZE;
    let step = 65535.0 / (n - 1) as f64;
    let grid: Vec<[u16; 3]> = (0..n * n * n)
        .map(|i| {
            let (r, g, b) = (i % n, i / n % n, i / (n * n));
            [r, g, b].map(|v| (v as f64 * step).round() as u16)
        })
        .collect();
    let mut out = vec![[0u16; 3]; grid.len()];
    let ok = unsafe {
        TranslateBitmapBits(
            xform.0,
            grid.as_ptr().cast(),
            BM_16b_RGB,
            grid.len() as u32,
            1,
            0,
            out.as_mut_ptr().cast(),
            BM_16b_RGB,
            0,
            None,
            None,
        )
    };
    ok.as_bool().then(|| Lut3d::from_nodes(n, out)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use blinzel_core::icc::{Trc, colorants, matrix_profile};

    fn to8(v: f32) -> i32 {
        (v * 255.0).round() as i32
    }

    #[test]
    fn srgb_to_srgb_is_identity() {
        let srgb = Profile::new(matrix_profile(colorants::SRGB, Trc::Srgb, "sRGB test"));
        let lut = build_lut(Some(&srgb), None).expect("transform");
        for c in [
            [0.0f32, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.2, 0.5, 0.8],
            [1.0, 0.0, 0.0],
        ] {
            let out = lut.apply(c);
            for k in 0..3 {
                assert!((to8(out[k]) - to8(c[k])).abs() <= 1, "{c:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn wide_gamut_to_srgb() {
        let p3 = Profile::new(matrix_profile(colorants::DISPLAY_P3, Trc::Srgb, "P3 test"));
        let lut = build_lut(Some(&p3), None).expect("transform");
        // Neutrals stay neutral (same white, same curve).
        for v in [0.0f32, 0.25, 0.5, 1.0] {
            let out = lut.apply([v; 3]);
            for k in 0..3 {
                assert!((to8(out[k]) - to8(v)).abs() <= 1, "{v} -> {out:?}");
            }
        }
        // P3's red lies outside sRGB: clipped to full red, green and blue
        // pulled to zero; a muted P3 red becomes more saturated in sRGB.
        let red = lut.apply([1.0, 0.0, 0.0]);
        assert!(
            to8(red[0]) >= 250 && to8(red[1]) <= 5 && to8(red[2]) <= 5,
            "{red:?}"
        );
        let muted = lut.apply([0.6, 0.3, 0.3]);
        assert!(muted[0] > 0.6 && muted[1] < 0.3, "{muted:?}");
    }
}
