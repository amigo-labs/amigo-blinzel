//! Rendering on macOS through Metal: the same draws as the Direct3D 11 and
//! wgpu renderers, with `blinzel.metal` as a port of their shaders and the
//! uniforms from [`blinzel_ui::draws`]. Images are cut into tiles
//! (`blinzel_ui::gfx::plan_tiles`) and uploaded on the decode workers
//! (Metal devices and `replaceRegion` are thread-safe).
//!
//! A frame is recorded as a list of draws and encoded into one render pass
//! at `present`; each draw passes its uniforms inline (`setVertexBytes`).

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;

use blinzel_core::geom::{Affine, Orientation, Rect};
use blinzel_core::lut::Lut3d;
use blinzel_core::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};
use blinzel_core::thumb::Planes;
use blinzel_ui::backend::{Canvas, Device};
use blinzel_ui::draws::{self, DrawCb, ImageKind, TileGeom};
use blinzel_ui::gfx::{
    AtlasPages, DEFAULT_TILE, ImageInfo, PAGE, Placed, Rgba, Shape, TilePlan, plan_tiles,
};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CGSize;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlendFactor, MTLBlitCommandEncoder, MTLBuffer, MTLClearColor, MTLCommandBuffer,
    MTLCommandEncoder, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLLibrary,
    MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRegion, MTLRenderCommandEncoder,
    MTLRenderPassDescriptor, MTLRenderPipelineDescriptor, MTLRenderPipelineState,
    MTLResourceOptions, MTLSamplerAddressMode, MTLSamplerDescriptor, MTLSamplerMinMagFilter,
    MTLSamplerState, MTLSize, MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor,
    MTLTextureType, MTLTextureUsage,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};

use super::text::TextCache;

pub type Tex = Retained<ProtocolObject<dyn MTLTexture>>;

/// The GPU, its pipelines and the objects every draw shares.
pub struct Gpu {
    pub device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    image_pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    overlay_pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    sampler: Retained<ProtocolObject<dyn MTLSamplerState>>,
    /// Bound where a draw needs fewer textures than the shader declares.
    one: Tex,
    no_lut: Tex,
    /// Format of what we draw into (window or offscreen target).
    pub format: MTLPixelFormat,
}

// SAFETY: Metal devices, command queues, pipeline and sampler states and
// textures may be used from any thread; textures are only written before
// they are shared (uploads) or through the command queue.
unsafe impl Send for Gpu {}
unsafe impl Sync for Gpu {}

fn err(what: &str, e: impl std::fmt::Display) -> String {
    format!("{what}: {e}")
}

impl Gpu {
    /// The system's default GPU, drawing BGRA8 (not sRGB: the shaders write
    /// sRGB-encoded values already, like the Direct3D renderer).
    pub fn new() -> Result<Gpu, String> {
        let device = MTLCreateSystemDefaultDevice().ok_or("no Metal device")?;
        let queue = device.newCommandQueue().ok_or("no command queue")?;
        let source = NSString::from_str(include_str!("blinzel.metal"));
        let library = device
            .newLibraryWithSource_options_error(&source, None)
            .map_err(|e| err("blinzel.metal", e.localizedDescription()))?;
        let function = |name: &str| {
            library
                .newFunctionWithName(&NSString::from_str(name))
                .ok_or_else(|| format!("no function {name}"))
        };
        let vs = function("vs_main")?;
        let format = MTLPixelFormat::BGRA8Unorm;
        let pipeline = |fs: &str, blend: bool| -> Result<_, String> {
            let d = MTLRenderPipelineDescriptor::new();
            d.setVertexFunction(Some(&vs));
            let f = function(fs)?;
            d.setFragmentFunction(Some(&f));
            // SAFETY: index 0 always exists.
            let a = unsafe { d.colorAttachments().objectAtIndexedSubscript(0) };
            a.setPixelFormat(format);
            if blend {
                // Premultiplied alpha.
                a.setBlendingEnabled(true);
                a.setSourceRGBBlendFactor(MTLBlendFactor::One);
                a.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                a.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                a.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            }
            device
                .newRenderPipelineStateWithDescriptor_error(&d)
                .map_err(|e| err(fs, e.localizedDescription()))
        };
        let image_pipeline = pipeline("fs_image", false)?;
        let overlay_pipeline = pipeline("fs_overlay", true)?;
        let sd = MTLSamplerDescriptor::new();
        sd.setMinFilter(MTLSamplerMinMagFilter::Linear);
        sd.setMagFilter(MTLSamplerMinMagFilter::Linear);
        sd.setSAddressMode(MTLSamplerAddressMode::ClampToEdge);
        sd.setTAddressMode(MTLSamplerAddressMode::ClampToEdge);
        sd.setRAddressMode(MTLSamplerAddressMode::ClampToEdge);
        let sampler = device
            .newSamplerStateWithDescriptor(&sd)
            .ok_or("no sampler")?;
        let one = texture_2d(&device, (1, 1), MTLPixelFormat::RGBA8Unorm)?;
        let no_lut = texture_3d(&device, 1)?;
        Ok(Gpu {
            device,
            queue,
            image_pipeline,
            overlay_pipeline,
            sampler,
            one,
            no_lut,
            format,
        })
    }

    /// The GPU's name.
    pub fn name(&self) -> String {
        self.device.name().to_string()
    }

    /// A texture filled from `data` with rows `stride` bytes apart.
    fn upload_2d(
        &self,
        size: (u32, u32),
        format: MTLPixelFormat,
        data: &[u8],
        stride: u32,
    ) -> Result<Tex, String> {
        let t = texture_2d(&self.device, size, format)?;
        write_2d(&t, (0, 0), size, data, stride);
        Ok(t)
    }

    /// Texture of the plane area `[x0, y0, x1, y1)`.
    fn plane_texture(
        &self,
        plane: &Plane,
        bpp: usize,
        format: MTLPixelFormat,
        [x0, y0, x1, y1]: [u32; 4],
    ) -> Result<Tex, String> {
        let (w, h) = (x1 - x0, y1 - y0);
        let offset = y0 as usize * plane.stride + x0 as usize * bpp;
        let end = offset + (h as usize - 1) * plane.stride + w as usize * bpp;
        assert!(end <= plane.data.len(), "tile outside plane");
        self.upload_2d(
            (w, h),
            format,
            &plane.data[offset..end],
            plane.stride as u32,
        )
    }

    /// A text texture from coverage (`w` × `h`, one byte per pixel).
    pub fn text_texture(&self, coverage: &[u8], w: u32, h: u32) -> Option<TextTex> {
        let rgba: Vec<u8> = coverage.iter().flat_map(|&a| [255, 255, 255, a]).collect();
        let tex = self
            .upload_2d((w, h), MTLPixelFormat::RGBA8Unorm, &rgba, w * 4)
            .ok()?;
        Some(TextTex { tex, size: (w, h) })
    }
}

fn region(origin: (u32, u32, u32), size: (u32, u32, u32)) -> MTLRegion {
    MTLRegion {
        origin: MTLOrigin {
            x: origin.0 as usize,
            y: origin.1 as usize,
            z: origin.2 as usize,
        },
        size: MTLSize {
            width: size.0 as usize,
            height: size.1 as usize,
            depth: size.2 as usize,
        },
    }
}

fn texture_2d(
    device: &ProtocolObject<dyn MTLDevice>,
    size: (u32, u32),
    format: MTLPixelFormat,
) -> Result<Tex, String> {
    // SAFETY: a plain 2D texture descriptor.
    let d = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format,
            size.0.max(1) as usize,
            size.1.max(1) as usize,
            false,
        )
    };
    d.setUsage(MTLTextureUsage::ShaderRead);
    device
        .newTextureWithDescriptor(&d)
        .ok_or_else(|| format!("no {}×{} texture", size.0, size.1))
}

fn texture_3d(device: &ProtocolObject<dyn MTLDevice>, n: u32) -> Result<Tex, String> {
    let d = MTLTextureDescriptor::new();
    d.setTextureType(MTLTextureType::Type3D);
    d.setPixelFormat(MTLPixelFormat::RGBA16Float);
    // SAFETY: sizes within Metal's 3D texture limits (LUTs are small).
    unsafe {
        d.setWidth(n as usize);
        d.setHeight(n as usize);
        d.setDepth(n as usize);
    }
    d.setUsage(MTLTextureUsage::ShaderRead);
    device
        .newTextureWithDescriptor(&d)
        .ok_or_else(|| "no 3D texture".into())
}

/// Writes `size` texels from `data` (rows `stride` bytes apart) at `origin`.
fn write_2d(t: &Tex, origin: (u32, u32), size: (u32, u32), data: &[u8], stride: u32) {
    if size.0 == 0 || size.1 == 0 || data.is_empty() {
        return;
    }
    // SAFETY: `data` covers `size` rows of `stride` bytes (checked by the
    // callers' slicing); Metal copies before returning.
    unsafe {
        t.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
            region((origin.0, origin.1, 0), (size.0, size.1, 1)),
            0,
            NonNull::new_unchecked(data.as_ptr() as *mut c_void),
            stride as usize,
        );
    }
}

// ------------------------------------------------------------------ images

struct Tile {
    geom: TileGeom,
    textures: [Tex; 3],
}

/// An image stage resident on the GPU.
pub struct MetalImage {
    kind: ImageKind,
    width: u32,
    height: u32,
    full: (u32, u32),
    orientation: Orientation,
    tiles: Vec<Tile>,
    bytes: usize,
}

// SAFETY: see `Gpu`; the textures are not written after the upload.
unsafe impl Send for MetalImage {}
unsafe impl Sync for MetalImage {}

impl ImageInfo for MetalImage {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn full_size(&self) -> (u32, u32) {
        self.full
    }

    fn orientation(&self) -> Orientation {
        self.orientation
    }

    fn bytes(&self) -> usize {
        self.bytes
    }
}

/// A colour transform as a 3D texture (RGBA16F, trilinear).
#[derive(Clone)]
pub struct MetalLut {
    tex: Tex,
    size: u32,
}

/// The device side shared with the decode workers.
pub struct MetalDevice {
    pub gpu: Arc<Gpu>,
    tile: u32,
}

impl MetalDevice {
    pub fn new(gpu: Arc<Gpu>) -> MetalDevice {
        MetalDevice::with_tile(gpu, DEFAULT_TILE)
    }

    /// With tiles of at most `tile` texels (tests use tiny ones).
    pub fn with_tile(gpu: Arc<Gpu>, tile: u32) -> MetalDevice {
        // Every Mac GPU Metal runs on takes 16384-texel textures.
        MetalDevice {
            gpu,
            tile: tile.min(16384),
        }
    }
}

impl Device for MetalDevice {
    type Image = MetalImage;
    type Lut = MetalLut;

    fn upload(&self, img: &Image, orientation: Orientation) -> Result<MetalImage, String> {
        let g = &self.gpu;
        let plans = plan_tiles(img, self.tile);
        let mut tiles = Vec::with_capacity(plans.len());
        let mut bytes = 0usize;
        let area = |r: [u32; 4]| ((r[2] - r[0]) * (r[3] - r[1])) as usize;
        let kind = match &img.pixels {
            Pixels::Packed {
                plane,
                format,
                alpha,
            } => {
                let (fmt, bpp) = match format {
                    PackedFormat::Bgra8 => (MTLPixelFormat::BGRA8Unorm, 4),
                    PackedFormat::Rgba8 => (MTLPixelFormat::RGBA8Unorm, 4),
                    PackedFormat::Gray8 => (MTLPixelFormat::R8Unorm, 1),
                };
                for TilePlan { rect, main, .. } in plans {
                    let t = g.plane_texture(plane, bpp, fmt, main)?;
                    bytes += area(main) * bpp;
                    tiles.push(Tile {
                        geom: TileGeom {
                            rect,
                            main: ((main[0], main[1]), (main[2] - main[0], main[3] - main[1])),
                            chroma: None,
                        },
                        textures: [t.clone(), t.clone(), t],
                    });
                }
                match format {
                    PackedFormat::Gray8 => ImageKind::Gray,
                    _ => ImageKind::Rgba {
                        opaque: *alpha == Alpha::Opaque,
                    },
                }
            }
            Pixels::Ycc { y, cb, cr, sub } => {
                let r8 = MTLPixelFormat::R8Unorm;
                for TilePlan { rect, main, chroma } in plans {
                    let c = chroma.expect("chroma for YCbCr");
                    let ty = g.plane_texture(y, 1, r8, main)?;
                    let tcb = g.plane_texture(cb, 1, r8, c)?;
                    let tcr = g.plane_texture(cr, 1, r8, c)?;
                    bytes += area(main) + 2 * area(c);
                    tiles.push(Tile {
                        geom: TileGeom {
                            rect,
                            main: ((main[0], main[1]), (main[2] - main[0], main[3] - main[1])),
                            chroma: Some(((c[0], c[1]), (c[2] - c[0], c[3] - c[1]))),
                        },
                        textures: [ty, tcb, tcr],
                    });
                }
                ImageKind::Ycc {
                    sub_h: sub.h,
                    sub_v: sub.v,
                }
            }
        };
        Ok(MetalImage {
            kind,
            width: img.width,
            height: img.height,
            full: (img.full_width, img.full_height),
            orientation,
            tiles,
            bytes,
        })
    }

    fn upload_lut(&self, lut: &Lut3d) -> Result<MetalLut, String> {
        let n = lut.size() as u32;
        let tex = texture_3d(&self.gpu.device, n)?;
        let texels = draws::lut_texels(lut);
        // SAFETY: `texels` holds n³ RGBA16F texels, tightly packed.
        unsafe {
            tex.replaceRegion_mipmapLevel_slice_withBytes_bytesPerRow_bytesPerImage(
                region((0, 0, 0), (n, n, n)),
                0,
                0,
                NonNull::new_unchecked(texels.as_ptr() as *mut c_void),
                (n * 8) as usize,
                (n * n * 8) as usize,
            );
        }
        Ok(MetalLut { tex, size: n })
    }

    fn texture_budget(&self) -> usize {
        // Unified memory on Apple silicon: a third of what Metal suggests
        // for the GPU's working set.
        (self.gpu.device.recommendedMaxWorkingSetSize() / 3) as usize
    }

    fn adapter_name(&self) -> String {
        self.gpu.name()
    }

    fn is_software(&self) -> bool {
        false
    }
}

// ------------------------------------------------------------------ canvas

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pipe {
    Image,
    Overlay,
}

struct DrawCmd {
    pipe: Pipe,
    cb: DrawCb,
    textures: [Option<Tex>; 3],
    lut: Option<Tex>,
}

/// One atlas page: Y and interleaved Cb/Cr.
struct AtlasPage {
    y: Tex,
    cbcr: Tex,
}

/// Where frames go.
pub enum Target {
    Layer(Retained<CAMetalLayer>),
    /// Offscreen (tests, benchmarks), readable afterwards.
    Texture(Tex),
}

pub struct MetalCanvas {
    gpu: Arc<Gpu>,
    target: Target,
    size: (u32, u32),
    clear: Rgba,
    draws: Vec<DrawCmd>,
    pages: Vec<AtlasPage>,
    text: TextCache,
}

impl MetalCanvas {
    pub fn new(gpu: Arc<Gpu>, target: Target, size: (u32, u32)) -> MetalCanvas {
        if let Target::Layer(layer) = &target {
            layer.setDevice(Some(&gpu.device));
            layer.setPixelFormat(gpu.format);
            layer.setFramebufferOnly(true);
        }
        let mut c = MetalCanvas {
            gpu,
            target,
            size: (size.0.max(1), size.1.max(1)),
            clear: Rgba(0.0, 0.0, 0.0, 1.0),
            draws: Vec::new(),
            pages: Vec::new(),
            text: TextCache::default(),
        };
        c.configure();
        c
    }

    /// An offscreen canvas of `size` pixels.
    pub fn offscreen(gpu: Arc<Gpu>, size: (u32, u32)) -> Result<MetalCanvas, String> {
        // SAFETY: a plain 2D texture descriptor.
        let d = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                gpu.format,
                size.0 as usize,
                size.1 as usize,
                false,
            )
        };
        d.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        d.setStorageMode(MTLStorageMode::Private);
        let t = gpu
            .device
            .newTextureWithDescriptor(&d)
            .ok_or("no offscreen texture")?;
        Ok(MetalCanvas::new(gpu, Target::Texture(t), size))
    }

    fn configure(&mut self) {
        if let Target::Layer(layer) = &self.target {
            layer.setDrawableSize(CGSize::new(self.size.0 as f64, self.size.1 as f64));
        }
    }

    fn overlay(&mut self, cb: DrawCb, textures: [Option<Tex>; 3]) {
        self.draws.push(DrawCmd {
            pipe: Pipe::Overlay,
            cb,
            textures,
            lut: None,
        });
    }

    /// The offscreen target's pixels (BGRA, tightly packed).
    pub fn read_bgra(&self) -> Result<Vec<u8>, String> {
        let Target::Texture(t) = &self.target else {
            return Err("not offscreen".into());
        };
        let (w, h) = self.size;
        let row = (w * 4) as usize;
        let buf = self
            .gpu
            .device
            .newBufferWithLength_options(row * h as usize, MTLResourceOptions::StorageModeShared)
            .ok_or("no readback buffer")?;
        let cmd = self.gpu.queue.commandBuffer().ok_or("no command buffer")?;
        let blit = cmd.blitCommandEncoder().ok_or("no blit encoder")?;
        // SAFETY: the buffer holds the whole texture.
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                t,
                0,
                0,
                MTLOrigin { x: 0, y: 0, z: 0 },
                MTLSize {
                    width: w as usize,
                    height: h as usize,
                    depth: 1,
                },
                &buf,
                0,
                row,
                row * h as usize,
            );
        }
        blit.endEncoding();
        cmd.commit();
        cmd.waitUntilCompleted();
        // SAFETY: shared storage, the GPU is done with it.
        let data = unsafe {
            std::slice::from_raw_parts(buf.contents().as_ptr().cast::<u8>(), row * h as usize)
        };
        Ok(data.to_vec())
    }
}

impl AtlasPages for MetalCanvas {
    fn add_page(&mut self) -> Result<(), String> {
        let d = &self.gpu.device;
        let y = texture_2d(d, (PAGE, PAGE), MTLPixelFormat::R8Unorm)?;
        let cbcr = texture_2d(d, (PAGE / 2, PAGE / 2), MTLPixelFormat::RG8Unorm)?;
        self.pages.push(AtlasPage { y, cbcr });
        Ok(())
    }

    fn upload(
        &mut self,
        page: usize,
        origin: (u32, u32),
        size: (u32, u32),
        chroma: (u32, u32),
        thumb: &Planes,
    ) {
        let Some(p) = self.pages.get(page) else {
            return;
        };
        write_2d(&p.y, origin, size, &thumb.y, thumb.width);
        write_2d(
            &p.cbcr,
            (origin.0 / 2, origin.1 / 2),
            chroma,
            &thumb.cbcr,
            thumb.chroma_size().0 * 2,
        );
    }
}

impl Canvas for MetalCanvas {
    type Device = MetalDevice;

    fn begin(&mut self, clear: Rgba) -> bool {
        self.draws.clear();
        self.clear = clear;
        true
    }

    fn draw_image(
        &mut self,
        img: &MetalImage,
        xform: &Affine,
        content: &Rect,
        bg: Rgba,
        lut: Option<&MetalLut>,
    ) {
        let tiles: Vec<TileGeom> = img.tiles.iter().map(|t| t.geom).collect();
        for (i, cb) in draws::image(
            img.kind,
            (img.width, img.height),
            &tiles,
            xform,
            content,
            bg,
            lut.map(|l| l.size),
            self.size,
        ) {
            let t = &img.tiles[i].textures;
            self.draws.push(DrawCmd {
                pipe: Pipe::Image,
                cb,
                textures: [Some(t[0].clone()), Some(t[1].clone()), Some(t[2].clone())],
                lut: lut.map(|l| l.tex.clone()),
            });
        }
    }

    fn draw_shapes(&mut self, shapes: &[Shape]) {
        for s in shapes {
            let cb = draws::shape(s, self.size);
            self.overlay(cb, [None, None, None]);
        }
    }

    fn draw_thumb(&mut self, t: &Placed, dest: &Rect, alpha: f32) {
        let Some(page) = self.pages.get(t.page) else {
            return;
        };
        let textures = [Some(page.y.clone()), Some(page.cbcr.clone()), None];
        self.overlay(draws::thumb(t, dest, alpha, self.size), textures);
    }

    fn text_size(&mut self, text: &str, px: f32, max_width: f32) -> Option<(u32, u32)> {
        self.text
            .get(&self.gpu, text, px, max_width)
            .map(|t| t.size)
    }

    fn draw_text(&mut self, text: &str, px: f32, max_width: f32, pos: (f32, f32), color: Rgba) {
        let Some(t) = self.text.get(&self.gpu, text, px, max_width) else {
            return;
        };
        let (tex, size) = (t.tex.clone(), t.size);
        let cb = draws::text(pos, size, color, self.size);
        self.overlay(cb, [Some(tex), None, None]);
    }

    fn present(&mut self) {
        let g = self.gpu.clone();
        let drawable = match &self.target {
            Target::Layer(layer) => match layer.nextDrawable() {
                Some(d) => Some(d),
                None => {
                    self.draws.clear();
                    return;
                }
            },
            Target::Texture(_) => None,
        };
        let target = match (&self.target, &drawable) {
            (_, Some(d)) => d.texture(),
            (Target::Texture(t), None) => t.clone(),
            _ => return,
        };
        let Some(cmd) = g.queue.commandBuffer() else {
            return;
        };
        let pass = MTLRenderPassDescriptor::renderPassDescriptor();
        // SAFETY: index 0 always exists.
        let a = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        a.setTexture(Some(&target));
        a.setLoadAction(MTLLoadAction::Clear);
        a.setStoreAction(MTLStoreAction::Store);
        let c = self.clear;
        a.setClearColor(MTLClearColor {
            red: c.0 as f64,
            green: c.1 as f64,
            blue: c.2 as f64,
            alpha: 1.0,
        });
        let Some(enc) = cmd.renderCommandEncoderWithDescriptor(&pass) else {
            return;
        };
        let mut current = None;
        for d in &self.draws {
            if current != Some(d.pipe) {
                enc.setRenderPipelineState(match d.pipe {
                    Pipe::Image => &g.image_pipeline,
                    Pipe::Overlay => &g.overlay_pipeline,
                });
                current = Some(d.pipe);
            }
            let bytes = d.cb.bytes();
            // SAFETY: the uniforms are copied by Metal before the call
            // returns; texture and sampler indices match blinzel.metal.
            unsafe {
                let ptr = NonNull::new_unchecked(bytes.as_ptr() as *mut c_void);
                enc.setVertexBytes_length_atIndex(ptr, bytes.len(), 0);
                enc.setFragmentBytes_length_atIndex(ptr, bytes.len(), 0);
                for (i, t) in d.textures.iter().enumerate() {
                    enc.setFragmentTexture_atIndex(Some(t.as_ref().unwrap_or(&g.one)), i);
                }
                enc.setFragmentTexture_atIndex(Some(d.lut.as_ref().unwrap_or(&g.no_lut)), 3);
                enc.setFragmentSamplerState_atIndex(Some(&g.sampler), 0);
                enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::TriangleStrip, 0, 4);
            }
        }
        enc.endEncoding();
        if let Some(d) = &drawable {
            cmd.presentDrawable(ProtocolObject::from_ref(&**d));
        }
        cmd.commit();
        self.draws.clear();
    }

    fn resize(&mut self, size: (u32, u32)) {
        let size = (size.0.max(1), size.1.max(1));
        if size != self.size {
            self.size = size;
            self.configure();
        }
    }
}

/// Text textures by (text, size, wrap width).
pub type TextKey = (String, u32, u32);

/// A text texture: coverage in alpha.
pub struct TextTex {
    pub tex: Tex,
    pub size: (u32, u32),
}
