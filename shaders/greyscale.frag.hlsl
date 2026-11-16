// Greyscale for a flipped image token (port of table_view.lua's tokenGreyShader): desaturates
// by luminance and keeps the image's own alpha.

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
    float4 texel = source_texture.Sample(source_sampler, input.texture_coordinate);
    float grey = dot(texel.rgb, float3(0.299, 0.587, 0.114));
    return float4(grey, grey, grey, texel.a) * input.color;
}
