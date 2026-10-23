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
    DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter, IDXGIAdapter3,
    IDXGIDevice1, IDXGIFactory2,
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

/// Device, immediate context and the objects derived from them.
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
                    return Ok(Gfx {
                        device,
                        context,
                        factory,
                        adapter,
                        feature_level,
                        warp: driver == D3D_DRIVER_TYPE_WARP,
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

    /// Bytes of video memory we allow ourselves to keep in textures.
    pub fn texture_budget(&self) -> usize {
        const MIN: usize = 256 << 20;
        const MAX: usize = 3 << 30;
        let os_budget = self.adapter.cast::<IDXGIAdapter3>().ok().and_then(|a3| {
            let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
            unsafe { a3.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info) }
                .ok()?;
            Some(info.Budget as usize)
        });
        let budget = match os_budget {
            Some(b) if b > 0 => b / 2,
            _ => unsafe { self.adapter.GetDesc() }
                .map(|d| (d.DedicatedVideoMemory.max(d.SharedSystemMemory / 4)) / 2)
                .unwrap_or(MIN),
        };
        budget.clamp(MIN, MAX)
    }
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
