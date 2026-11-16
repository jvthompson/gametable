// Shared vertex shader: positions arrive in physical pixels (top-left origin) and are mapped to
// clip space with the render target size. SDL GPU (D3D12) binds vertex uniforms at space1.

cbuffer ScreenUniforms : register(b0, space1)
{
    float2 target_size;
};

struct VertexInput
{
    [[vk::location(0)]] float2 position : TEXCOORD0;
    [[vk::location(1)]] float2 texture_coordinate : TEXCOORD1;
    [[vk::location(2)]] float4 color : TEXCOORD2;
};

struct VertexOutput
{
    float4 position : SV_Position;
    [[vk::location(0)]] float2 texture_coordinate : TEXCOORD0;
    [[vk::location(1)]] float4 color : TEXCOORD1;
};

VertexOutput main(VertexInput input)
{
    VertexOutput output;
    float2 clip = input.position / target_size * 2.0 - 1.0;
    output.position = float4(clip.x, -clip.y, 0.0, 1.0);
    output.texture_coordinate = input.texture_coordinate;
    output.color = input.color;
    return output;
}
