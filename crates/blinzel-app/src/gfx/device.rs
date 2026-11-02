//! Direct3D 11 device creation.
//!
//! `d3d11.dll` is loaded with `LoadLibraryW` instead of a static import, so
//! the loader does not pull it (and dxgi.dll with its dependencies) in
//! before `main`: device creation runs on a worker thread in parallel with
//! window creation and file decoding.

use std::ffi::c_void;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL,
    D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION, ID3D11Device,
    ID3D11DeviceContext,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
    IDXGIAdapter, IDXGIAdapter1, IDXGIAdapter3, IDXGIDevice1, IDXGIDevice3, IDXGIFactory2,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::HRESULT;
use windows::core::{Interface, s, w};

type CreateDeviceFn = unsafe extern "system" fn(
    adapter: *mut c_void,
    driver_type: D3D_DRIVER_TYPE,
    software: HMODULE,
    flags: D3D11_CREATE_DEVICE_FLAG,
    feature_levels: *const D3D_FEATURE_LEVEL,
    num_levels: u32,
    sdk_version: u32,
    device: *mut *mut c_void,
    level: *mut D3D_FEATURE_LEVEL,
    context: *mut *mut c_void,
) -> HRESULT;

/// Device, immediate context and the objects derived from them. Clones
/// share them (a second swap chain on the same device).
#[derive(Clone)]
pub struct Gfx {
    pub device: ID3D11Device,
    /// Only ever used on the UI thread (or the single bench thread).
    pub context: ID3D11DeviceContext,
    pub factory: IDXGIFactory2,
    pub adapter: IDXGIAdapter,
    pub feature_level: D3D_FEATURE_LEVEL,
    pub warp: bool,
}

// The immediate context is only used from one thread at a time: the device
// is created on a worker and handed over to the UI thread once.
unsafe impl Send for Gfx {}

impl Gfx {
    /// Creates a hardware device, or WARP (software) if asked or if no
    /// usable GPU exists.
    pub fn create(warp: bool) -> windows::core::Result<Gfx> {
        blinzel_core::timing::mark("gfx_start");
        let create = load_create_device()?;
        blinzel_core::timing::mark("d3d11_loaded");
        // Windows 11 requires a DX12/WDDM 2 GPU, so 11_0 is always there.
        let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let attempt = |driver: D3D_DRIVER_TYPE, levels: &[D3D_FEATURE_LEVEL]| {
            let mut device = std::ptr::null_mut();
            let mut context = std::ptr::null_mut();
            let mut level = D3D_FEATURE_LEVEL::default();
            let hr = unsafe {
                create(
                    std::ptr::null_mut(),
                    driver,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    levels.as_ptr(),
                    levels.len() as u32,
                    D3D11_SDK_VERSION,
                    &mut device,
                    &mut level,
                    &mut context,
                )
            };
            hr.ok()?;
            Ok::<_, windows::core::Error>(unsafe {
                (
                    ID3D11Device::from_raw(device),
                    ID3D11DeviceContext::from_raw(context),
                    level,
                )
            })
        };
        let drivers: &[D3D_DRIVER_TYPE] = if warp {
            &[D3D_DRIVER_TYPE_WARP]
        } else {
            &[D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP]
        };
        let mut last_err = None;
        for &driver in drivers {
            match attempt(driver, &levels) {
                Ok((device, context, feature_level)) => {
                    blinzel_core::timing::mark("d3d11_device_created");
                    let dxgi: IDXGIDevice1 = device.cast()?;
                    // One queued frame: input-to-photon latency over throughput.
                    unsafe { dxgi.SetMaximumFrameLatency(1)? };
                    let adapter = unsafe { dxgi.GetAdapter()? };
                    let factory: IDXGIFactory2 = unsafe { adapter.GetParent()? };
                    let flags = adapter
                        .cast::<IDXGIAdapter1>()
                        .ok()
                        // SAFETY: a plain query of the adapter just created.
                        .and_then(|a| unsafe { a.GetDesc1() }.ok())
                        .map_or(0, |d| d.Flags);
                    return Ok(Gfx {
                        device,
                        context,
                        factory,
                        adapter,
                        feature_level,
                        warp: runs_on_warp(driver, flags),
                    });
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap())
    }

    /// Largest texture edge (feature level 11_0 and up).
    pub fn max_texture_size(&self) -> u32 {
        16384
    }

    /// The adapter's name, e.g. "Microsoft Basic Render Driver" for WARP.
    pub fn adapter_name(&self) -> String {
        unsafe { self.adapter.GetDesc() }
            .map(|d| crate::platform::from_wide(&d.Description))
            .unwrap_or_default()
    }

    /// Hands the driver's idle allocations back (resident mode, hidden).
    /// The renderer sets all pipeline state per frame, so clearing it is
    /// safe.
    pub fn trim(&self) {
        unsafe {
            self.context.ClearState();
            self.context.Flush();
        }
        if let Ok(d) = self.device.cast::<IDXGIDevice3>() {
            unsafe { d.Trim() };
        }
    }

    /// Bytes of video memory we allow ourselves to keep in textures.
    pub fn texture_budget(&self) -> usize {
        let os_budget = self.adapter.cast::<IDXGIAdapter3>().ok().and_then(|a3| {
            let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
            unsafe { a3.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info) }
                .ok()?;
            Some(info.Budget as usize)
        });
        texture_budget(os_budget, || {
            unsafe { self.adapter.GetDesc() }
                .ok()
                .map(|d| (d.DedicatedVideoMemory, d.SharedSystemMemory))
        })
    }
}

/// Whether a device created with `driver` on an adapter with
/// `adapter_flags` (`DXGI_ADAPTER_DESC1::Flags`) renders with WARP: the WARP
/// driver type, or the hardware type on the "Microsoft Basic Render
/// Driver" of a machine without a GPU driver, a software adapter that is
/// WARP too.
fn runs_on_warp(driver: D3D_DRIVER_TYPE, adapter_flags: u32) -> bool {
    driver == D3D_DRIVER_TYPE_WARP || adapter_flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0
}

/// Half of what Windows grants the process (`os_budget`, DXGI), else half
/// of the adapter's (dedicated, shared) memory, counting a quarter of the
/// shared; clamped to 256 MiB … 3 GiB (`docs/specs/browsing.md` AC-13).
fn texture_budget(
    os_budget: Option<usize>,
    adapter: impl FnOnce() -> Option<(usize, usize)>,
) -> usize {
    const MIN: usize = 256 << 20;
    const MAX: usize = 3 << 30;
    let budget = match os_budget {
        Some(b) if b > 0 => b / 2,
        _ => adapter()
            .map(|(dedicated, shared)| dedicated.max(shared / 4) / 2)
            .unwrap_or(MIN),
    };
    budget.clamp(MIN, MAX)
}

fn load_create_device() -> windows::core::Result<CreateDeviceFn> {
    unsafe {
        let module = LoadLibraryW(w!("d3d11.dll"))?;
        let proc = GetProcAddress(module, s!("D3D11CreateDevice"))
            .ok_or_else(windows::core::Error::from_thread)?;
        Ok(std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            CreateDeviceFn,
        >(proc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1 << 20;
    const GIB: usize = 1 << 30;

    fn no_adapter() -> Option<(usize, usize)> {
        panic!("the OS budget is enough")
    }

    #[test]
    fn texture_budget_is_half_the_os_budget_clamped() {
        assert_eq!(texture_budget(Some(4 * GIB), no_adapter), 2 * GIB);
        assert_eq!(texture_budget(Some(16 * GIB), no_adapter), 3 * GIB);
        assert_eq!(texture_budget(Some(100 * MIB), no_adapter), 256 * MIB);
    }

    #[test]
    fn texture_budget_falls_back_to_the_adapter_memory() {
        assert_eq!(texture_budget(None, || Some((2 * GIB, 4 * GIB))), GIB);
        assert_eq!(texture_budget(Some(0), || Some((0, 8 * GIB))), GIB);
        assert_eq!(texture_budget(None, || Some((0, 0))), 256 * MIB);
        assert_eq!(texture_budget(None, || None), 256 * MIB);
    }

    /// `docs/specs/startup.md` AC-11: `warp` is true on WARP, also when the
    /// hardware driver type lands on the Basic Render Driver (CI runners).
    #[test]
    fn the_basic_render_driver_counts_as_warp() {
        let software = DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32;
        assert!(runs_on_warp(D3D_DRIVER_TYPE_WARP, 0));
        assert!(runs_on_warp(D3D_DRIVER_TYPE_WARP, software));
        assert!(runs_on_warp(D3D_DRIVER_TYPE_HARDWARE, software));
        assert!(!runs_on_warp(D3D_DRIVER_TYPE_HARDWARE, 0));
        // Whatever this machine has: the Basic Render Driver is WARP.
        let gfx = Gfx::create(false).expect("a Direct3D 11 device");
        if gfx.adapter_name() == "Microsoft Basic Render Driver" {
            assert!(gfx.warp);
        }
        assert!(Gfx::create(true).expect("WARP").warp);
    }
}
