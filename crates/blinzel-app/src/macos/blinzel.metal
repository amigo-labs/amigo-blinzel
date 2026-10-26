// amigo-blinzel shaders for Metal (macOS): a line-by-line port of
// gfx/shaders/blinzel.hlsl, like linux/blinzel.wgsl. One vertex shader
// expands a quad from the uniforms (no vertex buffers). `fs_image` samples a
// tile of an image stage (RGBA, gray or YCbCr planes), box-filters when
// minifying, applies the colour transform (3D LUT, optional) and composites
// over the background. `fs_overlay` draws the UI (SDF shapes, text,
// thumbnails).
//
// fs_image must stay in sync with blinzel-core/src/cpuref.rs, which the
// Metal golden test compares against.

#include <metal_stdlib>
using namespace metal;

struct Draw {
    float4 m0;      // source -> client px, row 0: a, b, c
    float4 m1;      // row 1: d, e, f
    float4 rect;    // quad in source coordinates: x0, y0, x1, y1
    float4 tex0;    // texture 0: origin (source coords), 1/size
    float4 tex1;    // image: chroma origin, 1/size; overlay: shape geometry
    float4 view;    // 2/client.x, 2/client.y, footprint (texels/pixel), taps
    float4 color;   // image: background rgb; overlay: colour rgba
    float4 params;  // image: chroma sub h, v, mode, opaque; overlay: shape, thickness, radius
    float4 cms;     // image: 3D LUT on, (n - 1) / n, 0.5 / n
};

struct VsOut {
    float4 pos [[position]];
    float2 src;
};

vertex VsOut vs_main(uint id [[vertex_id]], constant Draw& d [[buffer(0)]]) {
    float2 c = float2((id & 1u) != 0u ? d.rect.z : d.rect.x,
                      (id & 2u) != 0u ? d.rect.w : d.rect.y);
    float2 p = float2(dot(d.m0.xyz, float3(c, 1.0)), dot(d.m1.xyz, float3(c, 1.0)));
    VsOut o;
    o.pos = float4(p.x * d.view.x - 1.0, 1.0 - p.y * d.view.y, 0.0, 1.0);
    o.src = c;
    return o;
}

// Full-range BT.601 (JFIF), same constants as pixel::ycc_to_rgb.
static float3 ycc_to_rgb(float y, float cb_in, float cr_in) {
    float cb = cb_in - 0.5;
    float cr = cr_in - 0.5;
    return saturate(float3(y + 1.402 * cr, y - 0.344136 * cb - 0.714136 * cr, y + 1.772 * cb));
}

static float4 fetch(float2 l, constant Draw& d, texture2d<float> t0, texture2d<float> t1,
                    texture2d<float> t2, sampler lin) {
    float4 c = t0.sample(lin, (l - d.tex0.xy) * d.tex0.zw, level(0.0));
    if (d.params.z > 1.5) {
        float2 cuv = (l / d.params.xy - d.tex1.xy) * d.tex1.zw;
        return float4(ycc_to_rgb(c.r,
                                 t1.sample(lin, cuv, level(0.0)).r,
                                 t2.sample(lin, cuv, level(0.0)).r), 1.0);
    }
    if (d.params.z > 0.5) {
        return float4(c.rrr, 1.0);
    }
    return float4(c.rgb, d.params.w > 0.5 ? 1.0 : c.a);
}

fragment float4 fs_image(VsOut i [[stage_in]],
                         constant Draw& d [[buffer(0)]],
                         texture2d<float> t0 [[texture(0)]],
                         texture2d<float> t1 [[texture(1)]],
                         texture2d<float> t2 [[texture(2)]],
                         texture3d<float> lut [[texture(3)]],
                         sampler lin [[sampler(0)]]) {
    uint n = uint(d.view.w);
    float fp = d.view.z;
    float4 acc = float4(0.0);
    for (uint j = 0u; j < n; j++) {
        for (uint k = 0u; k < n; k++) {
            float2 off = ((float2(float(k), float(j)) + 0.5) / float(n) - 0.5) * fp;
            acc += fetch(i.src + off, d, t0, t1, t2, lin);
        }
    }
    acc /= float(n * n);
    float3 rgb = acc.rgb;
    if (d.cms.x > 0.5) {
        rgb = lut.sample(lin, rgb * d.cms.y + d.cms.z, level(0.0)).rgb;
    }
    return float4(rgb * acc.a + d.color.rgb * (1.0 - acc.a), 1.0);
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
fragment float4 fs_overlay(VsOut i [[stage_in]],
                           constant Draw& d [[buffer(0)]],
                           texture2d<float> t0 [[texture(0)]],
                           texture2d<float> t1 [[texture(1)]],
                           sampler lin [[sampler(0)]]) {
    float2 p = i.src;
    float shape = d.params.x;
    float3 rgb = d.color.rgb;
    float cov;
    if (shape > 3.5) {
        float2 q = clamp(p, float2(0.5), d.tex1.xy - 0.5);
        float2 cq = clamp(q * 0.5, float2(0.5), d.tex1.zw - 0.5);
        float y = t0.sample(lin, (q - d.tex0.xy) * d.tex0.zw, level(0.0)).r;
        float2 c = t1.sample(lin, (cq - d.tex0.xy * 0.5) * (d.tex0.zw * 2.0), level(0.0)).rg;
        rgb = ycc_to_rgb(y, c.x, c.y);
        cov = 1.0;
    } else if (shape > 2.5) {
        cov = t0.sample(lin, (p - d.tex0.xy) * d.tex0.zw, level(0.0)).a;
    } else if (shape > 0.5 && shape < 1.5) {
        float2 pa = p - d.tex1.xy;
        float2 ba = d.tex1.zw - d.tex1.xy;
        float h = saturate(dot(pa, ba) / max(dot(ba, ba), 1e-6));
        float dist = length(pa - ba * h);
        cov = saturate(d.params.y * 0.5 - dist + 0.5);
    } else {
        float2 c = (d.tex1.xy + d.tex1.zw) * 0.5;
        float2 hs = (d.tex1.zw - d.tex1.xy) * 0.5;
        float r = d.params.z;
        float2 q = abs(p - c) - hs + r;
        float dist = length(max(q, float2(0.0))) + min(max(q.x, q.y), 0.0) - r;
        cov = shape < 0.5 ? saturate(0.5 - dist) : saturate(d.params.y * 0.5 - abs(dist) + 0.5);
    }
    float a = cov * d.color.a;
    return float4(rgb * a, a);
}
