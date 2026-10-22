// amigo-blinzel shaders (Shader Model 5.0: every Windows 11 GPU has feature
// level 11_0). Compiled to DXBC by fxc at build time; nothing is
// compiled at runtime.
//
// One vertex shader expands a quad from the constant buffer (no vertex
// buffers). `ps_image` samples a tile of an image stage (RGBA, gray or
// YCbCr planes), box-filters when minifying and composites over the
// background. `ps_overlay` draws the UI (SDF shapes, text).
//
// ps_image must stay in sync with blinzel-core/src/cpuref.rs, which the
// WARP golden tests compare against.

cbuffer Draw : register(b0)
{
    float4 m0;      // source → client px, row 0: a, b, c
    float4 m1;      // row 1: d, e, f
    float4 rect;    // quad in source coordinates: x0, y0, x1, y1
    float4 tex0;    // texture 0: origin (source coords), 1/size
    float4 tex1;    // image: chroma origin, 1/size; overlay: shape geometry
    float4 view;    // 2/client.x, 2/client.y, footprint (texels/pixel), taps
    float4 color;   // image: background rgb; overlay: colour rgba
    float4 params;  // image: chroma sub h, v, mode, opaque; overlay: shape, thickness, radius
};

Texture2D t0 : register(t0);
Texture2D t1 : register(t1);
Texture2D t2 : register(t2);
SamplerState lin : register(s0);

struct VSOut
{
    float4 pos : SV_Position;
    float2 src : TEXCOORD0;
};

VSOut vs_main(uint id : SV_VertexID)
{
    float2 c = float2((id & 1) ? rect.z : rect.x, (id & 2) ? rect.w : rect.y);
    float2 p = float2(dot(m0.xyz, float3(c, 1.0)), dot(m1.xyz, float3(c, 1.0)));
    VSOut o;
    o.pos = float4(p.x * view.x - 1.0, 1.0 - p.y * view.y, 0.0, 1.0);
    o.src = c;
    return o;
}

// Full-range BT.601 (JFIF), same constants as pixel::ycc_to_rgb.
float3 ycc_to_rgb(float y, float cb, float cr)
{
    cb -= 0.5;
    cr -= 0.5;
    return saturate(float3(y + 1.402 * cr, y - 0.344136 * cb - 0.714136 * cr, y + 1.772 * cb));
}

float4 fetch(float2 l)
{
    float mode = params.z;
    float2 uv = (l - tex0.xy) * tex0.zw;
    if (mode > 1.5)
    {
        float y = t0.SampleLevel(lin, uv, 0).r;
        float2 cuv = (l / params.xy - tex1.xy) * tex1.zw;
        return float4(ycc_to_rgb(y, t1.SampleLevel(lin, cuv, 0).r, t2.SampleLevel(lin, cuv, 0).r), 1.0);
    }
    float4 c = t0.SampleLevel(lin, uv, 0);
    if (mode > 0.5)
        return float4(c.rrr, 1.0);
    return float4(c.rgb, params.w > 0.5 ? 1.0 : c.a);
}

float4 ps_image(VSOut i) : SV_Target
{
    uint n = (uint)view.w;
    float fp = view.z;
    float4 acc = 0;
    [loop] for (uint j = 0; j < n; j++)
    {
        [loop] for (uint k = 0; k < n; k++)
        {
            float2 off = ((float2(k, j) + 0.5) / (float)n - 0.5) * fp;
            acc += fetch(i.src + off);
        }
    }
    acc /= (float)(n * n);
    return float4(acc.rgb * acc.a + color.rgb * (1.0 - acc.a), 1.0);
}

// Overlay shapes (params.x):
//   0  filled rounded box   tex1 = x0, y0, x1, y1   params.z = radius
//   1  line segment         tex1 = ax, ay, bx, by   params.y = thickness
//   2  box outline          tex1 = x0, y0, x1, y1   params.y = thickness, params.z = radius
//   3  text texture (premultiplied coverage in alpha), tex0 = origin, 1/size
// Output is premultiplied alpha.
float4 ps_overlay(VSOut i) : SV_Target
{
    float2 p = i.src;
    float shape = params.x;
    float cov;
    if (shape > 2.5)
    {
        float a = t0.SampleLevel(lin, (p - tex0.xy) * tex0.zw, 0).a * color.a;
        return float4(color.rgb * a, a);
    }
    if (shape > 0.5 && shape < 1.5)
    {
        float2 pa = p - tex1.xy;
        float2 ba = tex1.zw - tex1.xy;
        float h = saturate(dot(pa, ba) / max(dot(ba, ba), 1e-6));
        float d = length(pa - ba * h);
        cov = saturate(params.y * 0.5 - d + 0.5);
    }
    else
    {
        float2 c = (tex1.xy + tex1.zw) * 0.5;
        float2 hs = (tex1.zw - tex1.xy) * 0.5;
        float r = params.z;
        float2 q = abs(p - c) - hs + r;
        float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
        cov = shape < 0.5 ? saturate(0.5 - d) : saturate(params.y * 0.5 - abs(d) + 0.5);
    }
    float a = cov * color.a;
    return float4(color.rgb * a, a);
}
