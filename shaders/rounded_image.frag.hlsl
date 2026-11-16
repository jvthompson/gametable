// Rounded corners for card images (port of card_render.lua's rounded shader): discards pixels
// outside the corner circles of a `size` destination. quad_rect is the drawn part of the image
// in UVs (u0, v0, u1, v1). Fragment uniforms live at space3.

// Vulkan (SPIR-V): SDL GPU expects one combined image sampler at set 2, binding 0.
[[vk::combinedImageSampler]] [[vk::binding(0, 2)]]
Texture2D source_texture : register(t0, space2);
[[vk::combinedImageSampler]] [[vk::binding(0, 2)]]
SamplerState source_sampler : register(s0, space2);

cbuffer RoundedImageUniforms : register(b0, space3)
{
    float4 quad_rect;
    float2 size;
    float radius;
};

struct FragmentInput
{
    float4 position : SV_Position;
    [[vk::location(0)]] float2 texture_coordinate : TEXCOORD0;
    [[vk::location(1)]] float4 color : TEXCOORD1;
};

float4 main(FragmentInput input) : SV_Target
{
    float2 uv = (input.texture_coordinate - quad_rect.xy) / (quad_rect.zw - quad_rect.xy);
    float2 p = uv * size;
    float2 q = min(p, size - p);
    if (q.x < radius && q.y < radius && distance(q, float2(radius, radius)) > radius)
    {
        discard;
    }
    return source_texture.Sample(source_sampler, input.texture_coordinate) * input.color;
}
