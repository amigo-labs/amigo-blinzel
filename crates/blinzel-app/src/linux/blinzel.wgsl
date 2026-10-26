// amigo-blinzel shaders for wgpu (Linux): a line-by-line port of
// gfx/shaders/blinzel.hlsl. One vertex shader expands a quad from the
// uniforms (no vertex buffers). `fs_image` samples a tile of an image
// stage (RGBA, gray or YCbCr planes), box-filters when minifying, applies
// the colour transform (3D LUT, optional) and composites over the
// background. `fs_overlay` draws the UI (SDF shapes, text, thumbnails).
//
// fs_image must stay in sync with blinzel-core/src/cpuref.rs, which the
// lavapipe golden test compares against.

struct Draw {
    m0: vec4<f32>,      // source -> client px, row 0: a, b, c
    m1: vec4<f32>,      // row 1: d, e, f
    rect: vec4<f32>,    // quad in source coordinates: x0, y0, x1, y1
    tex0: vec4<f32>,    // texture 0: origin (source coords), 1/size
    tex1: vec4<f32>,    // image: chroma origin, 1/size; overlay: shape geometry
    view: vec4<f32>,    // 2/client.x, 2/client.y, footprint (texels/pixel), taps
    color: vec4<f32>,   // image: background rgb; overlay: colour rgba
    params: vec4<f32>,  // image: chroma sub h, v, mode, opaque; overlay: shape, thickness, radius
    cms: vec4<f32>,     // image: 3D LUT on, (n - 1) / n, 0.5 / n
};

@group(0) @binding(0) var<uniform> d: Draw;
@group(0) @binding(1) var lin: sampler;
@group(1) @binding(0) var t0: texture_2d<f32>;
@group(1) @binding(1) var t1: texture_2d<f32>;
@group(1) @binding(2) var t2: texture_2d<f32>;
@group(2) @binding(0) var lut: texture_3d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) src: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) id: u32) -> VsOut {
    let c = vec2<f32>(
        select(d.rect.x, d.rect.z, (id & 1u) != 0u),
        select(d.rect.y, d.rect.w, (id & 2u) != 0u),
    );
    let p = vec2<f32>(dot(d.m0.xyz, vec3<f32>(c, 1.0)), dot(d.m1.xyz, vec3<f32>(c, 1.0)));
    var o: VsOut;
    o.pos = vec4<f32>(p.x * d.view.x - 1.0, 1.0 - p.y * d.view.y, 0.0, 1.0);
    o.src = c;
    return o;
}

// Full-range BT.601 (JFIF), same constants as pixel::ycc_to_rgb.
fn ycc_to_rgb(y: f32, cb_in: f32, cr_in: f32) -> vec3<f32> {
    let cb = cb_in - 0.5;
    let cr = cr_in - 0.5;
    return saturate(vec3<f32>(y + 1.402 * cr, y - 0.344136 * cb - 0.714136 * cr, y + 1.772 * cb));
}

fn fetch(l: vec2<f32>) -> vec4<f32> {
    let c = textureSampleLevel(t0, lin, (l - d.tex0.xy) * d.tex0.zw, 0.0);
    if (d.params.z > 1.5) {
        let cuv = (l / d.params.xy - d.tex1.xy) * d.tex1.zw;
        return vec4<f32>(ycc_to_rgb(
            c.r,
            textureSampleLevel(t1, lin, cuv, 0.0).r,
            textureSampleLevel(t2, lin, cuv, 0.0).r,
        ), 1.0);
    }
    if (d.params.z > 0.5) {
        return vec4<f32>(c.rrr, 1.0);
    }
    return vec4<f32>(c.rgb, select(c.a, 1.0, d.params.w > 0.5));
}

@fragment
fn fs_image(i: VsOut) -> @location(0) vec4<f32> {
    let n = u32(d.view.w);
    let fp = d.view.z;
    var acc = vec4<f32>(0.0);
    for (var j = 0u; j < n; j++) {
        for (var k = 0u; k < n; k++) {
            let off = ((vec2<f32>(f32(k), f32(j)) + 0.5) / f32(n) - 0.5) * fp;
            acc += fetch(i.src + off);
        }
    }
    acc /= f32(n * n);
    var rgb = acc.rgb;
    if (d.cms.x > 0.5) {
        rgb = textureSampleLevel(lut, lin, rgb * d.cms.y + d.cms.z, 0.0).rgb;
    }
    return vec4<f32>(rgb * acc.a + d.color.rgb * (1.0 - acc.a), 1.0);
}

// Overlay shapes (params.x):
//   0  filled rounded box   tex1 = x0, y0, x1, y1   params.z = radius
//   1  line segment         tex1 = ax, ay, bx, by   params.y = thickness
//   2  box outline          tex1 = x0, y0, x1, y1   params.y = thickness, params.z = radius
//   3  text texture (coverage in alpha), tex0 = origin, 1/size
//   4  atlas thumbnail, 4:2:0 YCbCr: t0 = Y page, t1 = Cb/Cr page (half
//      size); src = thumbnail texels, tex0 = -slot origin, 1/Y page size;
//      tex1 = thumbnail size, chroma size (clamps to the slot)
// Output is premultiplied alpha.
@fragment
fn fs_overlay(i: VsOut) -> @location(0) vec4<f32> {
    let p = i.src;
    let shape = d.params.x;
    var rgb = d.color.rgb;
    var cov: f32;
    if (shape > 3.5) {
        let q = clamp(p, vec2<f32>(0.5), d.tex1.xy - 0.5);
        let cq = clamp(q * 0.5, vec2<f32>(0.5), d.tex1.zw - 0.5);
        let y = textureSampleLevel(t0, lin, (q - d.tex0.xy) * d.tex0.zw, 0.0).r;
        let c = textureSampleLevel(t1, lin, (cq - d.tex0.xy * 0.5) * (d.tex0.zw * 2.0), 0.0).rg;
        rgb = ycc_to_rgb(y, c.x, c.y);
        cov = 1.0;
    } else if (shape > 2.5) {
        cov = textureSampleLevel(t0, lin, (p - d.tex0.xy) * d.tex0.zw, 0.0).a;
    } else if (shape > 0.5 && shape < 1.5) {
        let pa = p - d.tex1.xy;
        let ba = d.tex1.zw - d.tex1.xy;
        let h = saturate(dot(pa, ba) / max(dot(ba, ba), 1e-6));
        let dist = length(pa - ba * h);
        cov = saturate(d.params.y * 0.5 - dist + 0.5);
    } else {
        let c = (d.tex1.xy + d.tex1.zw) * 0.5;
        let hs = (d.tex1.zw - d.tex1.xy) * 0.5;
        let r = d.params.z;
        let q = abs(p - c) - hs + r;
        let dist = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
        cov = select(saturate(d.params.y * 0.5 - abs(dist) + 0.5), saturate(0.5 - dist), shape < 0.5);
    }
    let a = cov * d.color.a;
    return vec4<f32>(rgb * a, a);
}
