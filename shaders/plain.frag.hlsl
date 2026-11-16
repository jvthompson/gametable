// Texture × vertex colour (LÖVE's default shader). Fragment resources live at space2.

Texture2D source_texture : register(t0, space2);
SamplerState source_sampler : register(s0, space2);

struct FragmentInput
{
    float4 position : SV_Position;
    float2 texture_coordinate : TEXCOORD0;
    float4 color : TEXCOORD1;
};

float4 main(FragmentInput input) : SV_Target
{
    return source_texture.Sample(source_sampler, input.texture_coordinate) * input.color;
}
