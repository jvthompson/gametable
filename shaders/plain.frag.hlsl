// Texture × vertex colour (LÖVE's default shader). Fragment resources live at space2.

// Vulkan (SPIR-V): SDL GPU expects one combined image sampler at set 2, binding 0.
[[vk::combinedImageSampler]] [[vk::binding(0, 2)]]
Texture2D source_texture : register(t0, space2);
[[vk::combinedImageSampler]] [[vk::binding(0, 2)]]
SamplerState source_sampler : register(s0, space2);

struct FragmentInput
{
    float4 position : SV_Position;
    [[vk::location(0)]] float2 texture_coordinate : TEXCOORD0;
    [[vk::location(1)]] float4 color : TEXCOORD1;
};

float4 main(FragmentInput input) : SV_Target
{
    return source_texture.Sample(source_sampler, input.texture_coordinate) * input.color;
}
