//! Rendering on Linux through wgpu (Vulkan, OpenGL ES as fallback): the
//! same draws as the Direct3D 11 renderer, with `blinzel.wgsl` as a port of
//! its shaders. Images are cut into tiles like there
//! (`blinzel_ui::gfx::plan_tiles`) and uploaded on the decode workers
//! (wgpu's device and queue are thread-safe).
//!
//! A frame is recorded as a list of draws (pipeline, uniforms, textures)
//! and encoded into one render pass at `present`.

use std::sync::{Arc, Mutex};

use blinzel_core::cpuref::taps;
use blinzel_core::geom::{Affine, Orientation, Rect};
use blinzel_core::layout::texels_per_pixel;
use blinzel_core::lut::Lut3d;
use blinzel_core::pixel::{Alpha, Image, PackedFormat, Pixels, Plane};
use blinzel_core::thumb::Planes;
use blinzel_ui::backend::{Canvas, Device};
use blinzel_ui::gfx::{
    AtlasPages, DEFAULT_TILE, ImageInfo, PAGE, Placed, Rgba, Shape, TilePlan, plan_tiles,
};

use super::text::TextCache;

/// Mirror of the WGSL `Draw` uniforms.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct DrawCb {
    m0: [f32; 4],
    m1: [f32; 4],
    rect: [f32; 4],
    tex0: [f32; 4],
    tex1: [f32; 4],
    view: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
    cms: [f32; 4],
}

impl DrawCb {
    fn bytes(&self) -> &[u8] {
        // SAFETY: plain f32 arrays without padding.
        unsafe {
            std::slice::from_raw_parts(
                (self as *const DrawCb).cast::<u8>(),
                std::mem::size_of::<DrawCb>(),
            )
        }
    }
}

/// Uniform slots are this far apart (wgpu's dynamic-offset alignment).
const CB_STRIDE: u64 = 256;

/// The GPU, its pipelines and the objects every draw shares.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
    /// Format of what we draw into (window or offscreen target).
    pub format: wgpu::TextureFormat,
    uni_layout: wgpu::BindGroupLayout,
    tex_layout: wgpu::BindGroupLayout,
    lut_layout: wgpu::BindGroupLayout,
    image_pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    /// Bound where a draw needs fewer textures than the layout has.
    no_textures: wgpu::BindGroup,
    no_lut: wgpu::BindGroup,
}

impl Gpu {
    /// The GPU for `surface` (none: offscreen, e.g. tests), drawing in
    /// `format`.
    pub fn new(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        format: Option<wgpu::TextureFormat>,
    ) -> Result<Gpu, String> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: surface,
            apply_limit_buckets: false,
        }))
        .map_err(|e| e.to_string())?;
        let format = match (surface, format) {
            (_, Some(f)) => f,
            (Some(s), None) => {
                let caps = s.get_capabilities(&adapter);
                // Not an sRGB format: the shaders write sRGB-encoded values
                // already, like the Direct3D renderer.
                [
                    wgpu::TextureFormat::Bgra8Unorm,
                    wgpu::TextureFormat::Rgba8Unorm,
                ]
                .into_iter()
                .find(|f| caps.formats.contains(f))
                .or_else(|| caps.formats.iter().copied().find(|f| !f.is_srgb()))
                .ok_or("no usable surface format")?
            }
            (None, None) => wgpu::TextureFormat::Bgra8Unorm,
        };
        let limits = wgpu::Limits::downlevel_webgl2_defaults()
            .using_resolution(adapter.limits())
            .using_alignment(adapter.limits());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("blinzel"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| e.to_string())?;
        let info = adapter.get_info();

        let uni_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("draw"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<DrawCb>() as u64
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("textures"),
            entries: &[tex_entry(0), tex_entry(1), tex_entry(2)],
        });
        let lut_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lut"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blinzel.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blinzel.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blinzel"),
            bind_group_layouts: &[Some(&uni_layout), Some(&tex_layout), Some(&lut_layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    // Mirrored orientations flip the winding.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let image_pipeline = pipeline("fs_image", None);
        let overlay_pipeline = pipeline(
            "fs_overlay",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let one = texture_2d(&device, (1, 1), wgpu::TextureFormat::Rgba8Unorm);
        let one_view = one.create_view(&Default::default());
        let no_textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("no textures"),
            layout: &tex_layout,
            entries: &[
                tex_binding(0, &one_view),
                tex_binding(1, &one_view),
                tex_binding(2, &one_view),
            ],
        });
        let lut1 = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("no lut"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let no_lut = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("no lut"),
            layout: &lut_layout,
            entries: &[tex_binding(0, &lut1.create_view(&Default::default()))],
        });
        Ok(Gpu {
            device,
            queue,
            info,
            format,
            uni_layout,
            tex_layout,
            lut_layout,
            image_pipeline,
            overlay_pipeline,
            sampler,
            no_textures,
            no_lut,
        })
    }

    pub fn max_texture_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    fn textures(&self, views: [&wgpu::TextureView; 3]) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.tex_layout,
            entries: &[
                tex_binding(0, views[0]),
                tex_binding(1, views[1]),
                tex_binding(2, views[2]),
            ],
        })
    }

    /// A texture filled from `data` with rows `stride` bytes apart.
    fn upload_2d(
        &self,
        size: (u32, u32),
        format: wgpu::TextureFormat,
        data: &[u8],
        stride: u32,
    ) -> wgpu::Texture {
        let t = texture_2d(&self.device, size, format);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size.1),
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        t
    }

    /// Texture of the plane area `[x0, y0, x1, y1)`.
    fn plane_texture(
        &self,
        plane: &Plane,
        bpp: usize,
        format: wgpu::TextureFormat,
        [x0, y0, x1, y1]: [u32; 4],
    ) -> wgpu::Texture {
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
}

fn texture_2d(
    device: &wgpu::Device,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn tex_binding(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

// ------------------------------------------------------------------ images

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Rgba { opaque: bool },
    Gray,
    Ycc { sub_h: u8, sub_v: u8 },
}

struct Tile {
    rect: [u32; 4],
    /// Origin and size of the main texture in plane texels.
    main: ((u32, u32), (u32, u32)),
    chroma: Option<((u32, u32), (u32, u32))>,
    group: wgpu::BindGroup,
}

/// An image stage resident on the GPU.
pub struct WgpuImage {
    kind: Kind,
    width: u32,
    height: u32,
    full: (u32, u32),
    orientation: Orientation,
    tiles: Vec<Tile>,
    bytes: usize,
}

impl ImageInfo for WgpuImage {
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
pub struct WgpuLut {
    group: wgpu::BindGroup,
    size: u32,
}

/// The device side shared with the decode workers.
pub struct WgpuDevice {
    pub gpu: Arc<Gpu>,
    tile: u32,
}

impl WgpuDevice {
    pub fn new(gpu: Arc<Gpu>) -> WgpuDevice {
        WgpuDevice::with_tile(gpu, DEFAULT_TILE)
    }

    /// With tiles of at most `tile` texels (tests use tiny ones).
    pub fn with_tile(gpu: Arc<Gpu>, tile: u32) -> WgpuDevice {
        let tile = tile.min(gpu.max_texture_size());
        WgpuDevice { gpu, tile }
    }
}

/// An instance with the backends the shell uses: Vulkan, then OpenGL ES.
pub fn instance() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    })
}

fn f16(v: f32) -> u16 {
    // Values in 0..1: no infinities or NaNs to care about.
    let bits = v.clamp(0.0, 1.0).to_bits();
    let exp = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
    let mant = bits & 0x7F_FFFF;
    if exp <= 0 {
        // Subnormal (or zero).
        if exp < -10 {
            return 0;
        }
        let m = (mant | 0x80_0000) >> (1 - exp);
        return ((m + 0x1000) >> 13) as u16;
    }
    let h = ((exp as u32) << 10) | (mant >> 13);
    // Round to nearest.
    (h + ((mant >> 12) & 1)) as u16
}

impl Device for WgpuDevice {
    type Image = WgpuImage;
    type Lut = WgpuLut;

    fn upload(&self, img: &Image, orientation: Orientation) -> Result<WgpuImage, String> {
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
                    PackedFormat::Bgra8 => (wgpu::TextureFormat::Bgra8Unorm, 4),
                    PackedFormat::Rgba8 => (wgpu::TextureFormat::Rgba8Unorm, 4),
                    PackedFormat::Gray8 => (wgpu::TextureFormat::R8Unorm, 1),
                };
                for TilePlan { rect, main, .. } in plans {
                    let t = g.plane_texture(plane, bpp, fmt, main);
                    let v = t.create_view(&Default::default());
                    bytes += area(main) * bpp;
                    tiles.push(Tile {
                        rect,
                        main: ((main[0], main[1]), (main[2] - main[0], main[3] - main[1])),
                        chroma: None,
                        group: g.textures([&v, &v, &v]),
                    });
                }
                match format {
                    PackedFormat::Gray8 => Kind::Gray,
                    _ => Kind::Rgba {
                        opaque: *alpha == Alpha::Opaque,
                    },
                }
            }
            Pixels::Ycc { y, cb, cr, sub } => {
                let r8 = wgpu::TextureFormat::R8Unorm;
                for TilePlan { rect, main, chroma } in plans {
                    let c = chroma.expect("chroma for YCbCr");
                    let ty = g.plane_texture(y, 1, r8, main);
                    let tcb = g.plane_texture(cb, 1, r8, c);
                    let tcr = g.plane_texture(cr, 1, r8, c);
                    bytes += area(main) + 2 * area(c);
                    tiles.push(Tile {
                        rect,
                        main: ((main[0], main[1]), (main[2] - main[0], main[3] - main[1])),
                        chroma: Some(((c[0], c[1]), (c[2] - c[0], c[3] - c[1]))),
                        group: g.textures([
                            &ty.create_view(&Default::default()),
                            &tcb.create_view(&Default::default()),
                            &tcr.create_view(&Default::default()),
                        ]),
                    });
                }
                Kind::Ycc {
                    sub_h: sub.h,
                    sub_v: sub.v,
                }
            }
        };
        Ok(WgpuImage {
            kind,
            width: img.width,
            height: img.height,
            full: (img.full_width, img.full_height),
            orientation,
            tiles,
            bytes,
        })
    }

    fn upload_lut(&self, lut: &Lut3d) -> Result<WgpuLut, String> {
        let g = &self.gpu;
        let n = lut.size() as u32;
        let texels: Vec<u16> = lut
            .nodes()
            .iter()
            .flat_map(|c| {
                [
                    f16(c[0] as f32 / 65535.0),
                    f16(c[1] as f32 / 65535.0),
                    f16(c[2] as f32 / 65535.0),
                    f16(1.0),
                ]
            })
            .collect();
        let size = wgpu::Extent3d {
            width: n,
            height: n,
            depth_or_array_layers: n,
        };
        let t = g.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lut"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // SAFETY: u16 to bytes, same layout.
        let data =
            unsafe { std::slice::from_raw_parts(texels.as_ptr().cast::<u8>(), texels.len() * 2) };
        g.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(n * 8),
                rows_per_image: Some(n),
            },
            size,
        );
        let group = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lut"),
            layout: &g.lut_layout,
            entries: &[tex_binding(0, &t.create_view(&Default::default()))],
        });
        Ok(WgpuLut { group, size: n })
    }

    fn texture_budget(&self) -> usize {
        // wgpu does not report video memory; integrated GPUs share RAM.
        1 << 30
    }

    fn adapter_name(&self) -> String {
        self.gpu.info.name.clone()
    }

    fn is_software(&self) -> bool {
        self.gpu.info.device_type == wgpu::DeviceType::Cpu
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
    textures: Option<wgpu::BindGroup>,
    lut: Option<wgpu::BindGroup>,
}

/// One atlas page: Y and interleaved Cb/Cr.
struct AtlasPage {
    y: wgpu::Texture,
    cbcr: wgpu::Texture,
    group: wgpu::BindGroup,
}

/// Where frames go.
pub enum Target {
    Surface {
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
    },
    /// Offscreen (tests, benchmarks), readable afterwards.
    Texture(wgpu::Texture),
}

pub struct WgpuCanvas {
    gpu: Arc<Gpu>,
    target: Target,
    size: (u32, u32),
    clear: Rgba,
    frame: Option<wgpu::SurfaceTexture>,
    draws: Vec<DrawCmd>,
    uniforms: Option<wgpu::Buffer>,
    pages: Vec<AtlasPage>,
    text: TextCache,
}

impl WgpuCanvas {
    pub fn new(gpu: Arc<Gpu>, target: Target, size: (u32, u32)) -> WgpuCanvas {
        let mut c = WgpuCanvas {
            text: TextCache::default(),
            gpu,
            target,
            size: (size.0.max(1), size.1.max(1)),
            clear: Rgba(0.0, 0.0, 0.0, 1.0),
            frame: None,
            draws: Vec::new(),
            uniforms: None,
            pages: Vec::new(),
        };
        c.configure();
        c
    }

    /// An offscreen canvas of `size` pixels.
    pub fn offscreen(gpu: Arc<Gpu>, size: (u32, u32)) -> WgpuCanvas {
        let t = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: gpu.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        WgpuCanvas::new(gpu, Target::Texture(t), size)
    }

    fn configure(&mut self) {
        if let Target::Surface { surface, config } = &mut self.target {
            config.width = self.size.0;
            config.height = self.size.1;
            surface.configure(&self.gpu.device, config);
        }
    }

    fn view_vec(&self, footprint: f32, taps: f32) -> [f32; 4] {
        [
            2.0 / self.size.0 as f32,
            2.0 / self.size.1 as f32,
            footprint,
            taps,
        ]
    }

    fn overlay(&mut self, cb: DrawCb, textures: Option<wgpu::BindGroup>) {
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
        let row = (w * 4).div_ceil(256) * 256;
        let buf = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.gpu.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: t,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        let done = Arc::new(Mutex::new(None));
        let d = done.clone();
        slice.map_async(wgpu::MapMode::Read, move |r| *d.lock().unwrap() = Some(r));
        self.gpu
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        done.lock()
            .unwrap()
            .take()
            .ok_or("map did not finish")?
            .map_err(|e| e.to_string())?;
        let data = slice.get_mapped_range().map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        let rgba = self.gpu.format == wgpu::TextureFormat::Rgba8Unorm;
        for y in 0..h as usize {
            let r = &data[y * row as usize..y * row as usize + w as usize * 4];
            if rgba {
                out.extend(
                    r.as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|p| [p[2], p[1], p[0], p[3]]),
                );
            } else {
                out.extend_from_slice(r);
            }
        }
        Ok(out)
    }
}

impl AtlasPages for WgpuCanvas {
    fn add_page(&mut self) -> Result<(), String> {
        let g = &self.gpu;
        let y = texture_2d(&g.device, (PAGE, PAGE), wgpu::TextureFormat::R8Unorm);
        let cbcr = texture_2d(
            &g.device,
            (PAGE / 2, PAGE / 2),
            wgpu::TextureFormat::Rg8Unorm,
        );
        let (yv, cv) = (
            y.create_view(&Default::default()),
            cbcr.create_view(&Default::default()),
        );
        let group = g.textures([&yv, &cv, &cv]);
        self.pages.push(AtlasPage { y, cbcr, group });
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
        let write = |t: &wgpu::Texture, o: (u32, u32), s: (u32, u32), data: &[u8], stride: u32| {
            if s.0 == 0 || s.1 == 0 {
                return;
            }
            self.gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: t,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: o.0,
                        y: o.1,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(s.1),
                },
                wgpu::Extent3d {
                    width: s.0,
                    height: s.1,
                    depth_or_array_layers: 1,
                },
            );
        };
        write(&p.y, origin, size, &thumb.y, thumb.width);
        write(
            &p.cbcr,
            (origin.0 / 2, origin.1 / 2),
            chroma,
            &thumb.cbcr,
            thumb.chroma_size().0 * 2,
        );
    }
}

impl Canvas for WgpuCanvas {
    type Device = WgpuDevice;

    fn begin(&mut self, clear: Rgba) -> bool {
        self.draws.clear();
        self.clear = clear;
        if let Target::Surface { surface, .. } = &self.target {
            let frame = match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(f)
                | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
                _ => None,
            };
            let frame = frame.or_else(|| {
                self.configure();
                let Target::Surface { surface, .. } = &self.target else {
                    return None;
                };
                match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(f)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
                    _ => None,
                }
            });
            let Some(frame) = frame else {
                return false;
            };
            self.frame = Some(frame);
        }
        true
    }

    fn draw_image(
        &mut self,
        img: &WgpuImage,
        xform: &Affine,
        content: &Rect,
        bg: Rgba,
        lut: Option<&WgpuLut>,
    ) {
        let footprint = texels_per_pixel(xform);
        let cms = match lut {
            Some(l) => {
                let n = l.size as f32;
                [1.0, (n - 1.0) / n, 0.5 / n, 0.0]
            }
            None => [0.0; 4],
        };
        let n = taps(footprint);
        let (mode, sub, opaque) = match img.kind {
            Kind::Rgba { opaque } => (0.0, (1.0, 1.0), opaque),
            Kind::Gray => (1.0, (1.0, 1.0), true),
            Kind::Ycc { sub_h, sub_v } => (2.0, (sub_h as f32, sub_v as f32), true),
        };
        let bounds = Rect::new(0.0, 0.0, img.width as f64, img.height as f64);
        let Some(visible) = content.intersect(&bounds) else {
            return;
        };
        for tile in &img.tiles {
            let r = tile.rect;
            let tr = Rect::new(
                r[0] as f64,
                r[1] as f64,
                (r[2] - r[0]) as f64,
                (r[3] - r[1]) as f64,
            );
            let Some(q) = tr.intersect(&visible) else {
                continue;
            };
            let (origin, size) = tile.main;
            let chroma = match tile.chroma {
                Some((o, s)) => [o.0 as f32, o.1 as f32, 1.0 / s.0 as f32, 1.0 / s.1 as f32],
                None => [0.0, 0.0, 1.0, 1.0],
            };
            let cb = DrawCb {
                m0: [xform.a as f32, xform.b as f32, xform.c as f32, 0.0],
                m1: [xform.d as f32, xform.e as f32, xform.f as f32, 0.0],
                rect: [q.x as f32, q.y as f32, q.right() as f32, q.bottom() as f32],
                tex0: [
                    origin.0 as f32,
                    origin.1 as f32,
                    1.0 / size.0 as f32,
                    1.0 / size.1 as f32,
                ],
                tex1: chroma,
                view: self.view_vec(footprint as f32, n as f32),
                color: [bg.0, bg.1, bg.2, 1.0],
                params: [sub.0, sub.1, mode, if opaque { 1.0 } else { 0.0 }],
                cms,
            };
            self.draws.push(DrawCmd {
                pipe: Pipe::Image,
                cb,
                textures: Some(tile.group.clone()),
                lut: lut.map(|l| l.group.clone()),
            });
        }
    }

    fn draw_shapes(&mut self, shapes: &[Shape]) {
        for s in shapes {
            let (bounds, geom, color, params) = match *s {
                Shape::Fill {
                    rect,
                    radius,
                    color,
                } => (rect, rect_vec(&rect), color, [0.0, 0.0, radius, 0.0]),
                Shape::Outline {
                    rect,
                    thickness,
                    radius,
                    color,
                } => {
                    let g = thickness as f64 / 2.0 + 1.0;
                    let b = Rect::new(rect.x - g, rect.y - g, rect.w + 2.0 * g, rect.h + 2.0 * g);
                    (b, rect_vec(&rect), color, [2.0, thickness, radius, 0.0])
                }
                Shape::Line {
                    a,
                    b,
                    thickness,
                    color,
                } => {
                    let g = thickness / 2.0 + 1.0;
                    let r = Rect::from_points(
                        ((a.0.min(b.0) - g) as f64, (a.1.min(b.1) - g) as f64),
                        ((a.0.max(b.0) + g) as f64, (a.1.max(b.1) + g) as f64),
                    );
                    (r, [a.0, a.1, b.0, b.1], color, [1.0, thickness, 0.0, 0.0])
                }
            };
            let cb = DrawCb {
                m0: [1.0, 0.0, 0.0, 0.0],
                m1: [0.0, 1.0, 0.0, 0.0],
                rect: rect_vec(&bounds),
                tex1: geom,
                view: self.view_vec(1.0, 1.0),
                color: [color.0, color.1, color.2, color.3],
                params,
                ..Default::default()
            };
            self.overlay(cb, None);
        }
    }

    fn draw_thumb(&mut self, t: &Placed, dest: &Rect, alpha: f32) {
        let Some(page) = self.pages.get(t.page) else {
            return;
        };
        let group = page.group.clone();
        let (w, h) = (t.size.0 as f64, t.size.1 as f64);
        let (ow, oh) = t.orientation.oriented_size(w, h);
        let xform = t
            .orientation
            .transform(w, h)
            .then(&Affine::scale(dest.w / ow, dest.h / oh))
            .then(&Affine::translate(dest.x, dest.y));
        let page_size = PAGE as f32;
        let (cw, ch) = (t.size.0.div_ceil(2) as f32, t.size.1.div_ceil(2) as f32);
        let cb = DrawCb {
            m0: [xform.a as f32, xform.b as f32, xform.c as f32, 0.0],
            m1: [xform.d as f32, xform.e as f32, xform.f as f32, 0.0],
            rect: [0.0, 0.0, w as f32, h as f32],
            tex0: [
                -(t.origin.0 as f32),
                -(t.origin.1 as f32),
                1.0 / page_size,
                1.0 / page_size,
            ],
            tex1: [w as f32, h as f32, cw, ch],
            view: self.view_vec(1.0, 1.0),
            color: [1.0, 1.0, 1.0, alpha],
            params: [4.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        self.overlay(cb, Some(group));
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
        let (group, size) = (t.group.clone(), t.size);
        let cb = DrawCb {
            m0: [1.0, 0.0, 0.0, 0.0],
            m1: [0.0, 1.0, 0.0, 0.0],
            rect: [pos.0, pos.1, pos.0 + size.0 as f32, pos.1 + size.1 as f32],
            tex0: [
                pos.0,
                pos.1,
                1.0 / size.0.max(1) as f32,
                1.0 / size.1.max(1) as f32,
            ],
            view: self.view_vec(1.0, 1.0),
            color: [color.0, color.1, color.2, color.3],
            params: [3.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        self.overlay(cb, Some(group));
    }

    fn present(&mut self) {
        let g = self.gpu.clone();
        let need = (self.draws.len().max(1) as u64) * CB_STRIDE;
        if self.uniforms.as_ref().is_none_or(|b| b.size() < need) {
            self.uniforms = Some(g.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("draws"),
                size: need.next_power_of_two().max(64 * CB_STRIDE),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let uniforms = self.uniforms.as_ref().unwrap();
        let mut data = vec![0u8; self.draws.len() * CB_STRIDE as usize];
        for (i, d) in self.draws.iter().enumerate() {
            let at = i * CB_STRIDE as usize;
            data[at..at + std::mem::size_of::<DrawCb>()].copy_from_slice(d.cb.bytes());
        }
        if !data.is_empty() {
            g.queue.write_buffer(uniforms, 0, &data);
        }
        let uni_group = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("draw"),
            layout: &g.uni_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: uniforms,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<DrawCb>() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&g.sampler),
                },
            ],
        });
        let view = match (&self.target, &self.frame) {
            (Target::Surface { .. }, Some(f)) => f.texture.create_view(&Default::default()),
            (Target::Texture(t), _) => t.create_view(&Default::default()),
            _ => return,
        };
        let mut enc = g.device.create_command_encoder(&Default::default());
        {
            let c = self.clear;
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: c.0 as f64,
                            g: c.1 as f64,
                            b: c.2 as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut current = None;
            for (i, d) in self.draws.iter().enumerate() {
                if current != Some(d.pipe) {
                    pass.set_pipeline(match d.pipe {
                        Pipe::Image => &g.image_pipeline,
                        Pipe::Overlay => &g.overlay_pipeline,
                    });
                    current = Some(d.pipe);
                }
                pass.set_bind_group(0, &uni_group, &[(i as u64 * CB_STRIDE) as u32]);
                pass.set_bind_group(1, d.textures.as_ref().unwrap_or(&g.no_textures), &[]);
                pass.set_bind_group(2, d.lut.as_ref().unwrap_or(&g.no_lut), &[]);
                pass.draw(0..4, 0..1);
            }
        }
        g.queue.submit([enc.finish()]);
        if let Some(f) = self.frame.take() {
            g.queue.present(f);
        }
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

fn rect_vec(r: &Rect) -> [f32; 4] {
    [r.x as f32, r.y as f32, r.right() as f32, r.bottom() as f32]
}

/// Text textures by (text, size, wrap width).
pub type TextKey = (String, u32, u32);

/// A text texture: coverage in alpha.
pub struct TextTex {
    pub group: wgpu::BindGroup,
    pub size: (u32, u32),
}

impl Gpu {
    /// A text texture from coverage (`w` × `h`, one byte per pixel).
    pub fn text_texture(&self, coverage: &[u8], w: u32, h: u32) -> TextTex {
        let rgba: Vec<u8> = coverage.iter().flat_map(|&a| [255, 255, 255, a]).collect();
        let t = self.upload_2d((w, h), wgpu::TextureFormat::Rgba8Unorm, &rgba, w * 4);
        let v = t.create_view(&Default::default());
        TextTex {
            group: self.textures([&v, &v, &v]),
            size: (w, h),
        }
    }
}
