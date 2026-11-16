// Procedural cloth grain for the table felt (port of table_view.lua's feltShader): the felt
// colour times fractal value noise, sampled in table-world space so it pans and zooms with the
// camera. LÖVE's screen coordinates are pixel centres from the top-left, as SV_Position is.

// Vulkan (SPIR-V): SDL GPU expects one combined image sampler at set 2, binding 0.
[[vk::combinedImageSampler]] [[vk::binding(0, 2)]]
Texture2D source_texture : register(t0, space2);
[[vk::combinedImageSampler]] [[vk::binding(0, 2)]]
SamplerState source_sampler : register(s0, space2);

cbuffer FeltUniforms : register(b0, space3)
{
    float zoom;
    float2 world_offset;
};

struct FragmentInput
{
    float4 position : SV_Position;
    [[vk::location(0)]] float2 texture_coordinate : TEXCOORD0;
    [[vk::location(1)]] float4 color : TEXCOORD1;
};

float hash(float2 p)
{
    return frac(sin(dot(p, float2(127.1, 311.7))) * 43758.5453123);
}

float value_noise(float2 p)
{
    float2 i = floor(p);
    float2 f = frac(p);
    float2 u = f * f * (3.0 - 2.0 * f);
    float a = hash(i);
    float b = hash(i + float2(1.0, 0.0));
    float c = hash(i + float2(0.0, 1.0));
    float d = hash(i + float2(1.0, 1.0));
    return lerp(lerp(a, b, u.x), lerp(c, d, u.x), u.y);
}

float fiber_grain(float2 p)
{
    float v = 0.0;
    float amplitude = 0.5;
    for (int i = 0; i < 4; i++)
    {
        v += amplitude * value_noise(p);
        p *= 2.03;
        amplitude *= 0.5;
    }
    return v;
}

float4 main(FragmentInput input) : SV_Target
{
    float2 p = input.position.xy / zoom + world_offset;
    float grain = fiber_grain(p * 0.4) - 0.5;
    grain = sign(grain) * pow(abs(grain) * 2.0, 0.75) * 0.5;
    float speckle = hash(floor(p * 2.2)) - 0.5;
    float shade = clamp(grain * 0.12 + speckle * 0.05, -0.15, 0.15);
    float3 rgb = input.color.rgb * (1.0 + shade);
    return float4(rgb, input.color.a) * source_texture.Sample(source_sampler, input.texture_coordinate);
}
