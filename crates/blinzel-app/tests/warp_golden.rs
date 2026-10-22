//! Renders images with the real shaders on WARP (software D3D11, available
//! on headless CI runners) and compares every pixel with the CPU reference
//! renderer in blinzel-core. Tiles are made tiny to exercise the seams.
//! On mismatch both images are written to target/warp-golden/ for review.

#![cfg(all(windows, feature = "turbojpeg"))]

use blinzel_app::gfx::{Gfx, Offscreen, Renderer, Rgba, upload};
use blinzel_core::cpuref::{self, Frame, max_diff, psnr};
use blinzel_core::decode::{self, Request};
use blinzel_core::geom::{Orientation, PixelRect};
use blinzel_core::layout::{
    ViewMode, ViewOptions, ViewState, compute, content_in_stage, stage_to_client,
};
use blinzel_core::pixel::Image;
use blinzel_core::testimg;

const BG: [u8; 3] = [30, 40, 50];

struct Case<'a> {
    name: &'a str,
    image: &'a Image,
    orientation: Orientation,
    client: (u32, u32),
    opts: ViewOptions,
    state: ViewState,
    tile: u32,
}

fn run(gfx: &Gfx, renderer: &mut Renderer, c: &Case) {
    let full = (c.image.full_width, c.image.full_height);
    let stage = (c.image.width, c.image.height);
    let oriented = c.orientation.oriented_size(full.0, full.1);
    let layout = compute(
        oriented,
        (c.client.0 as f64, c.client.1 as f64),
        &c.opts,
        &c.state,
    );
    let xform = stage_to_client(&layout, c.orientation, full, stage);
    let content = content_in_stage(&layout, c.orientation, full, stage);

    let gpu_img = upload(&gfx.device, c.image, c.orientation, c.tile).expect("upload");
    let target = Offscreen::new(&gfx.device, c.client).expect("offscreen");
    let bg = Rgba::from_u8(BG, 1.0);
    renderer.begin(&gfx.context, &target.rtv, c.client, bg);
    renderer.draw_image(&gfx.context, &gpu_img, &xform, &content, bg);
    let got = target.read_bgra(&gfx.context).expect("readback");

    let want = cpuref::render(
        c.image,
        &Frame {
            client: c.client,
            stage_to_client: xform,
            content,
            background: BG,
        },
    );
    let p = psnr(&got, &want);
    let mean = got
        .iter()
        .zip(&want)
        .map(|(a, b)| a.abs_diff(*b) as f64)
        .sum::<f64>()
        / got.len() as f64;
    if p < 38.0 || mean > 1.0 {
        dump(c.name, c.client, &got, &want);
        panic!(
            "{}: psnr {p:.1} dB, mean diff {mean:.2}, max diff {}",
            c.name,
            max_diff(&got, &want)
        );
    }
}

fn dump(name: &str, size: (u32, u32), got: &[u8], want: &[u8]) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/warp-golden");
    let _ = std::fs::create_dir_all(&dir);
    for (suffix, bgra) in [("gpu", got), ("cpu", want)] {
        let rgba: Vec<u8> = bgra
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0], 255])
            .collect();
        let file = std::fs::File::create(dir.join(format!("{name}-{suffix}.png"))).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), size.0, size.1);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgba).unwrap();
    }
}

fn setup() -> (Gfx, Renderer) {
    let gfx = Gfx::create(true).expect("WARP device");
    let renderer = Renderer::new(&gfx.device).expect("renderer");
    (gfx, renderer)
}

fn jpeg(w: u32, h: u32, sub: u16, gray: bool) -> Image {
    use blinzel_core::decode::jpeg::{EncodeOptions, encode_rgb};
    let px = if gray {
        testimg::gray(w, h)
    } else {
        testimg::rgb(w, h)
    };
    let data = encode_rgb(
        &px,
        w,
        h,
        EncodeOptions {
            subsampling: sub,
            gray,
            ..Default::default()
        },
    )
    .unwrap();
    decode::decode(&data, Request::Full).unwrap()
}

fn png_rgba(w: u32, h: u32) -> Image {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .unwrap()
            .write_image_data(&testimg::rgba(w, h))
            .unwrap();
    }
    decode::decode(&out, Request::Full).unwrap()
}

const ACTUAL: ViewOptions = ViewOptions {
    mode: ViewMode::ActualSize,
    no_upscale: true,
};
const FIT: ViewOptions = ViewOptions {
    mode: ViewMode::FitImageToWindow,
    no_upscale: false,
};

#[test]
fn ycc_subsamplings_with_tile_seams() {
    let (gfx, mut r) = setup();
    for sub in [420u16, 422, 444, 440] {
        let img = jpeg(257, 173, sub, false);
        run(
            &gfx,
            &mut r,
            &Case {
                name: &format!("ycc{sub}-actual"),
                image: &img,
                orientation: Orientation::NORMAL,
                client: (300, 200),
                opts: ACTUAL,
                state: ViewState::default(),
                tile: 72,
            },
        );
    }
}

#[test]
fn all_orientations_minified() {
    let (gfx, mut r) = setup();
    let img = jpeg(301, 187, 420, false);
    for v in 1..=8 {
        run(
            &gfx,
            &mut r,
            &Case {
                name: &format!("orient{v}"),
                image: &img,
                orientation: Orientation::from_exif(v),
                client: (160, 140),
                opts: FIT,
                state: ViewState::default(),
                tile: 96,
            },
        );
    }
}

#[test]
fn zoom_crop_gray_and_alpha() {
    let (gfx, mut r) = setup();
    let ycc = jpeg(120, 90, 420, false);
    run(
        &gfx,
        &mut r,
        &Case {
            name: "zoom3x",
            image: &ycc,
            orientation: Orientation::NORMAL,
            client: (200, 150),
            opts: ACTUAL,
            state: ViewState {
                zoom: Some(3.0),
                center: Some((40.0, 30.0)),
                crop: None,
            },
            tile: 64,
        },
    );
    run(
        &gfx,
        &mut r,
        &Case {
            name: "crop",
            image: &ycc,
            orientation: Orientation::from_exif(6),
            client: (100, 100),
            opts: FIT,
            state: ViewState {
                crop: Some(PixelRect::new(10, 20, 50, 40)),
                ..Default::default()
            },
            tile: 64,
        },
    );
    let gray = jpeg(133, 77, 420, true);
    run(
        &gfx,
        &mut r,
        &Case {
            name: "gray",
            image: &gray,
            orientation: Orientation::from_exif(3),
            client: (140, 90),
            opts: ACTUAL,
            state: ViewState::default(),
            tile: 64,
        },
    );
    let rgba = png_rgba(97, 61);
    run(
        &gfx,
        &mut r,
        &Case {
            name: "rgba-alpha",
            image: &rgba,
            orientation: Orientation::NORMAL,
            client: (120, 80),
            opts: FIT,
            state: ViewState::default(),
            tile: 64,
        },
    );
}

#[test]
fn heavy_minification_uses_box_filter() {
    let (gfx, mut r) = setup();
    let img = jpeg(640, 480, 420, false);
    run(
        &gfx,
        &mut r,
        &Case {
            name: "minify",
            image: &img,
            orientation: Orientation::NORMAL,
            client: (200, 150),
            opts: FIT,
            state: ViewState::default(),
            tile: 4096,
        },
    );
}

#[test]
fn overlay_shapes_and_text_render() {
    use blinzel_app::gfx::Shape;
    use blinzel_app::gfx::text::Text;
    use blinzel_core::geom::Rect;
    let (gfx, mut r) = setup();
    let target = Offscreen::new(&gfx.device, (64, 32)).unwrap();
    r.begin(
        &gfx.context,
        &target.rtv,
        (64, 32),
        Rgba(0.0, 0.0, 0.0, 1.0),
    );
    r.draw_shapes(
        &gfx.context,
        &[
            Shape::Fill {
                rect: Rect::new(0.0, 0.0, 32.0, 32.0),
                radius: 0.0,
                color: Rgba(1.0, 0.0, 0.0, 1.0),
            },
            Shape::Line {
                a: (40.0, 16.0),
                b: (60.0, 16.0),
                thickness: 2.0,
                color: Rgba(0.0, 1.0, 0.0, 1.0),
            },
            Shape::Outline {
                rect: Rect::new(2.0, 2.0, 10.0, 10.0),
                thickness: 1.0,
                radius: 0.0,
                color: Rgba(1.0, 1.0, 1.0, 0.5),
            },
        ],
    );
    let px = target.read_bgra(&gfx.context).unwrap();
    let at = |x: usize, y: usize| &px[(y * 64 + x) * 4..(y * 64 + x) * 4 + 3];
    assert_eq!(at(20, 20), &[0, 0, 255]); // red fill (BGR)
    assert_eq!(at(50, 16)[1], 255); // green line
    assert_eq!(at(50, 5), &[0, 0, 0]); // untouched
    // Text: some pixels must be lit.
    let mut text = Text::new(&gfx.device).expect("text");
    let t = text.get("1920 × 1080", 14.0, 1000.0).unwrap();
    let (srv, size) = (t.srv.clone(), t.size);
    assert!(size.0 > 20 && size.1 > 8, "{size:?}");
    let target = Offscreen::new(&gfx.device, size).unwrap();
    r.begin(&gfx.context, &target.rtv, size, Rgba(0.0, 0.0, 0.0, 1.0));
    r.draw_text(
        &gfx.context,
        &srv,
        (0.0, 0.0),
        size,
        Rgba(1.0, 1.0, 1.0, 1.0),
    );
    let px = target.read_bgra(&gfx.context).unwrap();
    assert!(
        px.chunks_exact(4).any(|p| p[0] > 200),
        "text produced no pixels"
    );
}
