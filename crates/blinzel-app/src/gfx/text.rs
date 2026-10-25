//! Text via DirectWrite, rendered by Direct2D into small cached textures
//! that the overlay shader composites. Used for the selection size label and
//! messages; never needed for the first frame.
//!
//! d2d1.dll and dwrite.dll are loaded on demand (no static import), and
//! [`prewarm`] loads them plus the system font collection on a background
//! thread after the first frame, so the first label appears without a hitch.

use std::collections::HashMap;
use std::ffi::c_void;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_OPTIONS,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, ID2D1DeviceContext,
    ID2D1Factory1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_READING_DIRECTION_RIGHT_TO_LEFT, DWRITE_TEXT_METRICS, IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{GUID, HRESULT, HSTRING, Interface, PCSTR, PCWSTR, w};

type D2DCreateFactoryFn = unsafe extern "system" fn(
    u32,
    *const GUID,
    *const D2D1_FACTORY_OPTIONS,
    *mut *mut c_void,
) -> HRESULT;
type DWriteCreateFactoryFn =
    unsafe extern "system" fn(DWRITE_FACTORY_TYPE, *const GUID, *mut *mut c_void) -> HRESULT;

fn proc(dll: PCWSTR, name: PCSTR) -> windows::core::Result<unsafe extern "system" fn() -> isize> {
    unsafe {
        let m: HMODULE = LoadLibraryW(dll)?;
        GetProcAddress(m, name).ok_or_else(windows::core::Error::from_thread)
    }
}

fn dwrite_factory() -> windows::core::Result<IDWriteFactory> {
    let f = proc(w!("dwrite.dll"), windows::core::s!("DWriteCreateFactory"))?;
    let f: DWriteCreateFactoryFn = unsafe { std::mem::transmute(f) };
    let mut raw = std::ptr::null_mut();
    unsafe {
        f(DWRITE_FACTORY_TYPE_SHARED, &IDWriteFactory::IID, &mut raw).ok()?;
        Ok(IDWriteFactory::from_raw(raw))
    }
}

fn d2d_factory() -> windows::core::Result<ID2D1Factory1> {
    let f = proc(w!("d2d1.dll"), windows::core::s!("D2D1CreateFactory"))?;
    let f: D2DCreateFactoryFn = unsafe { std::mem::transmute(f) };
    let mut raw = std::ptr::null_mut();
    let opts = D2D1_FACTORY_OPTIONS::default();
    unsafe {
        f(
            D2D1_FACTORY_TYPE_SINGLE_THREADED.0 as u32,
            &ID2D1Factory1::IID,
            &opts,
            &mut raw,
        )
        .ok()?;
        Ok(ID2D1Factory1::from_raw(raw))
    }
}

const FONT: &str = "Segoe UI";

fn text_format(
    dw: &IDWriteFactory,
    px: f32,
    bold: bool,
) -> windows::core::Result<IDWriteTextFormat> {
    let format = unsafe {
        dw.CreateTextFormat(
            &HSTRING::from(FONT),
            None,
            if bold {
                DWRITE_FONT_WEIGHT_SEMI_BOLD
            } else {
                DWRITE_FONT_WEIGHT_NORMAL
            },
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            px,
            w!(""),
        )?
    };
    if blinzel_i18n::is_rtl() {
        unsafe { format.SetReadingDirection(DWRITE_READING_DIRECTION_RIGHT_TO_LEFT)? };
    }
    Ok(format)
}

/// Loads the text DLLs and warms the font cache off the UI thread.
pub fn prewarm() {
    let _ = std::thread::Builder::new()
        .name("blinzel-text-prewarm".into())
        .spawn(|| {
            if let Ok(dw) = dwrite_factory() {
                let _ = text_format(&dw, 14.0, true);
            }
            let _ = d2d_factory();
        });
}

/// A rendered string.
pub struct TextTex {
    pub srv: ID3D11ShaderResourceView,
    pub size: (u32, u32),
}

pub struct Text {
    device: ID3D11Device,
    d2d: ID2D1DeviceContext,
    dwrite: IDWriteFactory,
    /// By size, weight and right-to-left (the language can change).
    formats: HashMap<(u32, bool, bool), IDWriteTextFormat>,
    cache: HashMap<(String, u32, bool), TextTex>,
}

impl Text {
    pub fn new(device: &ID3D11Device) -> windows::core::Result<Text> {
        let factory = d2d_factory()?;
        let dxgi: IDXGIDevice = device.cast()?;
        let d2d = unsafe {
            factory
                .CreateDevice(&dxgi)?
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?
        };
        unsafe { d2d.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE) };
        Ok(Text {
            device: device.clone(),
            d2d,
            dwrite: dwrite_factory()?,
            formats: HashMap::new(),
            cache: HashMap::new(),
        })
    }

    /// The texture for `text` in semibold at `px` pixels font size, wrapped
    /// at `max_width` pixels.
    pub fn get(&mut self, text: &str, px: f32, max_width: f32) -> windows::core::Result<&TextTex> {
        self.get_weighted(text, px, max_width, true)
    }

    /// Like [`Text::get`], in semibold or regular weight.
    pub fn get_weighted(
        &mut self,
        text: &str,
        px: f32,
        max_width: f32,
        bold: bool,
    ) -> windows::core::Result<&TextTex> {
        let key = (
            format!("{text}\u{0}{}", max_width as u32),
            (px * 10.0) as u32,
            bold,
        );
        if !self.cache.contains_key(&key) {
            if self.cache.len() > 64 {
                self.cache.clear();
            }
            let tex = self.render(text, px, max_width, bold)?;
            self.cache.insert(key.clone(), tex);
        }
        Ok(&self.cache[&key])
    }

    fn render(
        &mut self,
        text: &str,
        px: f32,
        max_width: f32,
        bold: bool,
    ) -> windows::core::Result<TextTex> {
        let fkey = ((px * 10.0) as u32, bold, blinzel_i18n::is_rtl());
        if !self.formats.contains_key(&fkey) {
            self.formats
                .insert(fkey, text_format(&self.dwrite, px, bold)?);
        }
        let format = &self.formats[&fkey];
        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout = unsafe {
            self.dwrite
                .CreateTextLayout(&wide, format, max_width.max(1.0), 10_000.0)?
        };
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut m)? };
        let pad = 2.0;
        let size = (
            (m.width + 2.0 * pad).ceil().max(1.0) as u32,
            (m.height + 2.0 * pad).ceil().max(1.0) as u32,
        );

        let desc = D3D11_TEXTURE2D_DESC {
            Width: size.0,
            Height: size.1,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut tex: Option<ID3D11Texture2D> = None;
        let mut srv = None;
        unsafe {
            self.device.CreateTexture2D(&desc, None, Some(&mut tex))?;
            let tex = tex.unwrap();
            self.device
                .CreateShaderResourceView(&tex, None, Some(&mut srv))?;
            let surface: IDXGISurface = tex.cast()?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            let bitmap = self
                .d2d
                .CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.d2d.SetTarget(&bitmap);
            self.d2d.SetDpi(96.0, 96.0);
            self.d2d.BeginDraw();
            self.d2d.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            let brush = self.d2d.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 1.0,
                },
                None,
            )?;
            self.d2d.DrawTextLayout(
                windows_numerics::Vector2 { X: pad, Y: pad },
                &layout,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
            let r = self.d2d.EndDraw(None, None);
            self.d2d.SetTarget(None);
            r?;
        }
        Ok(TextTex {
            srv: srv.unwrap(),
            size,
        })
    }
}
