//! Runs a [`DrawList`] on the SDL3 GPU API: one vertex shader, the four fragment programs,
//! alpha blending as LÖVE's default `"alpha"` mode, 4× MSAA (the original asked LÖVE for
//! `msaa = 4`) resolved into the swapchain, and a stencil buffer for `stencil(...)` clipping.
//!
//! Shaders are HLSL compiled to DXIL (`tools/compile_shaders.sh`), so this runs on Direct3D 12;
//! other platforms need SPIR-V/MSL builds of the same shaders (see PORT_PLAN.md).

use std::collections::HashMap;

use sdl3::gpu::{
    BlendFactor, BlendOp, BlitInfo, Buffer, BufferBinding, BufferRegion, BufferUsageFlags, ColorComponentFlags,
    ColorTargetBlendState, ColorTargetDescription, ColorTargetInfo, CompareOp, CullMode, DepthStencilState,
    DepthStencilTargetInfo, Device, Filter, GraphicsPipeline, GraphicsPipelineTargetInfo, LoadOp, MultisampleState,
    PrimitiveType, RasterizerState, SampleCount, Sampler, SamplerAddressMode, SamplerCreateInfo, SamplerMipmapMode,
    Shader, ShaderFormat, ShaderStage, StencilOp, StencilOpState, StoreOp, Texture, TextureCreateInfo, TextureFormat,
    TextureRegion, TextureSamplerBinding, TextureTransferInfo, TextureType, TextureUsage, TransferBuffer,
    TransferBufferLocation, TransferBufferUsage, VertexAttribute, VertexBufferDescription, VertexElementFormat,
    VertexInputRate, VertexInputState,
};
use sdl3::video::Window;

use super::draw_list::{
    Color, DrawCommand, DrawList, FragmentProgram, ScissorRect, StencilCompare, StencilMode, TextureId, TextureInfo,
    Vertex,
};

const VERTEX_SHADER: &[u8] = include_bytes!("../../../../assets/shaders/quad.vert.dxil");
const PLAIN_SHADER: &[u8] = include_bytes!("../../../../assets/shaders/plain.frag.dxil");
const GREYSCALE_SHADER: &[u8] = include_bytes!("../../../../assets/shaders/greyscale.frag.dxil");
const ROUNDED_IMAGE_SHADER: &[u8] = include_bytes!("../../../../assets/shaders/rounded_image.frag.dxil");
const FELT_SHADER: &[u8] = include_bytes!("../../../../assets/shaders/felt.frag.dxil");

/// Which fragment shader a pipeline uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ProgramKind {
    Plain,
    Greyscale,
    RoundedImage,
    Felt,
}

impl ProgramKind {
    fn of(program: &FragmentProgram) -> ProgramKind {
        match program {
            FragmentProgram::Plain => ProgramKind::Plain,
            FragmentProgram::Greyscale => ProgramKind::Greyscale,
            FragmentProgram::RoundedImage { .. } => ProgramKind::RoundedImage,
            FragmentProgram::Felt { .. } => ProgramKind::Felt,
        }
    }
}

/// How a pipeline uses the stencil buffer (the reference value is set per draw).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum StencilUse {
    Off,
    Write,
    Test(StencilCompare),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PipelineKey {
    program: ProgramKind,
    stencil: StencilUse,
}

/// Uniforms for the rounded-image program, laid out as the HLSL constant buffer.
#[repr(C)]
#[derive(Clone, Copy)]
struct RoundedImageUniforms {
    quad_rect: [f32; 4],
    size: [f32; 2],
    radius: f32,
    padding: f32,
}

/// Uniforms for the felt program.
#[repr(C)]
#[derive(Clone, Copy)]
struct FeltUniforms {
    zoom: f32,
    world_offset: [f32; 2],
    padding: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ScreenUniforms {
    target_size: [f32; 2],
    padding: [f32; 2],
}

/// Render targets sized to the swapchain.
struct FrameTargets {
    width: u32,
    height: u32,
    multisampled_color: Option<Texture<'static>>,
    depth_stencil: Texture<'static>,
    /// The resolved frame; blitted to the swapchain and read back for screenshots.
    frame: Texture<'static>,
}

/// A frame read back from the GPU.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    /// Straight RGBA8, rows top to bottom.
    pub rgba: Vec<u8>,
}

/// A growable GPU vertex buffer with its upload buffer.
struct VertexStorage {
    capacity_in_vertices: u32,
    buffer: Buffer,
    upload: TransferBuffer,
}

pub struct GpuRenderer {
    device: Device,
    swapchain_format: TextureFormat,
    sample_count: SampleCount,
    depth_stencil_format: TextureFormat,
    vertex_shader: Shader,
    fragment_shaders: HashMap<ProgramKind, Shader>,
    pipelines: HashMap<PipelineKey, GraphicsPipeline>,
    sampler: Sampler,
    textures: HashMap<TextureId, Texture<'static>>,
    targets: Option<FrameTargets>,
    vertices: Option<VertexStorage>,
}

/// Errors from SDL, as text.
pub type RenderResult<T> = Result<T, String>;

fn sdl_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

impl GpuRenderer {
    pub fn new(window: &Window) -> RenderResult<GpuRenderer> {
        let device = Device::new(ShaderFormat::DXIL, cfg!(debug_assertions))
            .map_err(sdl_error)?
            .with_window(window)
            .map_err(sdl_error)?;
        let swapchain_format = device.get_swapchain_texture_format(window);
        let sample_count = if texture_supports_sample_count(&device, swapchain_format, SampleCount::MSAA4x) {
            SampleCount::MSAA4x
        } else {
            SampleCount::NoMultiSampling
        };
        let depth_stencil_format =
            if texture_supports_format(&device, TextureFormat::D24UnormS8Uint, TextureUsage::DEPTH_STENCIL_TARGET) {
                TextureFormat::D24UnormS8Uint
            } else {
                TextureFormat::D32FloatS8Uint
            };
        let vertex_shader = device
            .create_shader()
            .with_code(ShaderFormat::DXIL, VERTEX_SHADER, ShaderStage::Vertex)
            .with_entrypoint(c"main")
            .with_uniform_buffers(1)
            .build()
            .map_err(sdl_error)?;
        let mut fragment_shaders = HashMap::new();
        for (kind, code, uniform_buffers) in [
            (ProgramKind::Plain, PLAIN_SHADER, 0),
            (ProgramKind::Greyscale, GREYSCALE_SHADER, 0),
            (ProgramKind::RoundedImage, ROUNDED_IMAGE_SHADER, 1),
            (ProgramKind::Felt, FELT_SHADER, 1),
        ] {
            let shader = device
                .create_shader()
                .with_code(ShaderFormat::DXIL, code, ShaderStage::Fragment)
                .with_entrypoint(c"main")
                .with_samplers(1)
                .with_uniform_buffers(uniform_buffers)
                .build()
                .map_err(sdl_error)?;
            fragment_shaders.insert(kind, shader);
        }
        let sampler = device
            .create_sampler(
                SamplerCreateInfo::new()
                    .with_min_filter(Filter::Linear)
                    .with_mag_filter(Filter::Linear)
                    .with_mipmap_mode(SamplerMipmapMode::Linear)
                    .with_address_mode_u(SamplerAddressMode::ClampToEdge)
                    .with_address_mode_v(SamplerAddressMode::ClampToEdge)
                    .with_address_mode_w(SamplerAddressMode::ClampToEdge)
                    .with_enable_anisotropy(true)
                    .with_max_anisotropy(4.0)
                    .with_max_lod(1000.0),
            )
            .map_err(sdl_error)?;
        let mut renderer = GpuRenderer {
            device,
            swapchain_format,
            sample_count,
            depth_stencil_format,
            vertex_shader,
            fragment_shaders,
            pipelines: HashMap::new(),
            sampler,
            textures: HashMap::new(),
            targets: None,
            vertices: None,
        };
        let white = renderer.create_texture_from_rgba(1, 1, &[255, 255, 255, 255], false)?;
        let white_texture = renderer.textures.remove(&white.id).expect("just created");
        renderer.textures.insert(TextureId::WHITE, white_texture);
        Ok(renderer)
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Whether 4× multisampling is in use.
    pub fn is_multisampled(&self) -> bool {
        self.sample_count != SampleCount::NoMultiSampling
    }

    // -- Textures --------------------------------------------------------------

    /// Uploads straight (not premultiplied) RGBA8 pixels. `mipmaps` builds a full mip chain, as
    /// `newImage(data, {mipmaps = true})` did for card images.
    pub fn create_texture_from_rgba(
        &mut self,
        width: u32,
        height: u32,
        pixels: &[u8],
        mipmaps: bool,
    ) -> RenderResult<TextureInfo> {
        self.create_texture_with_id(TextureId::allocate(), width, height, pixels, mipmaps)
    }

    /// Like [`GpuRenderer::create_texture_from_rgba`] for an id allocated earlier.
    pub fn create_texture_with_id(
        &mut self,
        id: TextureId,
        width: u32,
        height: u32,
        pixels: &[u8],
        mipmaps: bool,
    ) -> RenderResult<TextureInfo> {
        let level_count = if mipmaps { 32 - width.max(height).max(1).leading_zeros() } else { 1 };
        let usage = if mipmaps { TextureUsage::SAMPLER | TextureUsage::COLOR_TARGET } else { TextureUsage::SAMPLER };
        let texture = self
            .device
            .create_texture(
                TextureCreateInfo::new()
                    .with_type(TextureType::_2D)
                    .with_format(TextureFormat::R8g8b8a8Unorm)
                    .with_usage(usage)
                    .with_width(width)
                    .with_height(height)
                    .with_layer_count_or_depth(1)
                    .with_num_levels(level_count),
            )
            .map_err(sdl_error)?;
        self.textures.insert(id, texture);
        self.upload_texture_region(id, 0, 0, width, height, pixels)?;
        if mipmaps && level_count > 1 {
            let command_buffer = self.device.acquire_command_buffer().map_err(sdl_error)?;
            unsafe {
                sdl3::sys::gpu::SDL_GenerateMipmapsForGPUTexture(command_buffer.raw(), self.textures[&id].raw());
            }
            command_buffer.submit().map_err(sdl_error)?;
        }
        Ok(TextureInfo { id, width: width as f32, height: height as f32 })
    }

    /// Replaces part of a texture's top mip level (glyph atlases grow this way).
    pub fn upload_texture_region(
        &mut self,
        id: TextureId,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> RenderResult<()> {
        let byte_count = width * height * 4;
        if byte_count == 0 {
            return Ok(());
        }
        assert_eq!(pixels.len(), byte_count as usize, "pixels must be RGBA8 for the whole region");
        let texture = self.textures.get(&id).ok_or_else(|| format!("unknown texture {id:?}"))?;
        let upload = self
            .device
            .create_transfer_buffer()
            .with_usage(TransferBufferUsage::UPLOAD)
            .with_size(byte_count)
            .build()
            .map_err(sdl_error)?;
        let mut mapped = upload.map::<u8>(&self.device, false);
        mapped.mem_mut()[..pixels.len()].copy_from_slice(pixels);
        mapped.unmap();
        let command_buffer = self.device.acquire_command_buffer().map_err(sdl_error)?;
        let copy_pass = self.device.begin_copy_pass(&command_buffer).map_err(sdl_error)?;
        copy_pass.upload_to_gpu_texture(
            TextureTransferInfo::new().with_transfer_buffer(&upload).with_offset(0),
            TextureRegion::new()
                .with_texture(texture)
                .with_x(x)
                .with_y(y)
                .with_width(width)
                .with_height(height)
                .with_depth(1),
            false,
        );
        self.device.end_copy_pass(copy_pass);
        command_buffer.submit().map_err(sdl_error)?;
        Ok(())
    }

    pub fn has_texture(&self, id: TextureId) -> bool {
        self.textures.contains_key(&id)
    }

    /// Frees a texture (the GPU keeps it until in-flight frames finish).
    pub fn destroy_texture(&mut self, id: TextureId) {
        if id != TextureId::WHITE {
            self.textures.remove(&id);
        }
    }

    // -- Frames ----------------------------------------------------------------

    /// Draws one frame: clears to `clear_color`, runs the draw list and presents. With `capture`,
    /// also returns the frame's pixels (for screenshots).
    pub fn render(
        &mut self,
        window: &Window,
        draw_list: &DrawList,
        clear_color: Color,
        capture: bool,
    ) -> RenderResult<Option<CapturedFrame>> {
        let command_buffer = self.device.acquire_command_buffer().map_err(sdl_error)?;
        let Some(swapchain) = command_buffer.wait_and_acquire_swapchain_texture(window).map_err(sdl_error)? else {
            // Minimized: nothing to draw into.
            command_buffer.submit().map_err(sdl_error)?;
            return Ok(None);
        };
        let (width, height) = (swapchain.width(), swapchain.height());
        self.ensure_targets(width, height)?;

        // Vertices: the list's own, then a full-screen quad for stencil clears.
        let full_screen_first = draw_list.vertices().len() as u32;
        let mut vertices = draw_list.vertices().to_vec();
        let (w, h) = (width as f32, height as f32);
        for position in [[0.0, 0.0], [w, 0.0], [w, h], [0.0, 0.0], [w, h], [0.0, h]] {
            vertices.push(Vertex { position, texture_coordinate: [0.0, 0.0], color: [1.0; 4] });
        }
        self.upload_vertices(&command_buffer, &vertices)?;

        for command in draw_list.commands() {
            if let DrawCommand::Triangles { program, stencil, .. } = command {
                self.ensure_pipeline(PipelineKey { program: ProgramKind::of(program), stencil: stencil_use(stencil) })?;
            }
        }
        if draw_list.commands().contains(&DrawCommand::ClearStencil) {
            self.ensure_pipeline(PipelineKey { program: ProgramKind::Plain, stencil: StencilUse::Write })?;
        }

        let clear = sdl3::pixels::Color::RGBA(
            channel_to_byte(clear_color.red),
            channel_to_byte(clear_color.green),
            channel_to_byte(clear_color.blue),
            channel_to_byte(clear_color.alpha),
        );
        let targets = self.targets.as_mut().expect("ensured above");
        let color_target = match &mut targets.multisampled_color {
            Some(multisampled) => ColorTargetInfo::default()
                .with_texture(multisampled)
                .with_load_op(LoadOp::CLEAR)
                .with_clear_color(clear)
                .with_store_op(StoreOp::RESOLVE)
                .with_resolve_texture(&targets.frame),
            None => ColorTargetInfo::default()
                .with_texture(&mut targets.frame)
                .with_load_op(LoadOp::CLEAR)
                .with_clear_color(clear)
                .with_store_op(StoreOp::STORE),
        };
        let depth_stencil_target = DepthStencilTargetInfo::new()
            .with_texture(&mut targets.depth_stencil)
            .with_clear_depth(1.0)
            .with_load_op(LoadOp::CLEAR)
            .with_store_op(StoreOp::DONT_CARE)
            .with_clear_stencil(0)
            .with_stencil_load_op(LoadOp::CLEAR)
            .with_stencil_store_op(StoreOp::DONT_CARE);
        let render_pass = self
            .device
            .begin_render_pass(&command_buffer, &[color_target], Some(&depth_stencil_target))
            .map_err(sdl_error)?;
        let vertex_storage = self.vertices.as_ref().expect("uploaded above");
        render_pass.bind_vertex_buffers(0, &[BufferBinding::new().with_buffer(&vertex_storage.buffer).with_offset(0)]);
        command_buffer.push_vertex_uniform_data(0, &ScreenUniforms { target_size: [w, h], padding: [0.0; 2] });
        let full_target = ScissorRect { x: 0, y: 0, width: width as i32, height: height as i32 };

        for command in draw_list.commands() {
            match command {
                DrawCommand::ClearStencil => {
                    let key = PipelineKey { program: ProgramKind::Plain, stencil: StencilUse::Write };
                    render_pass.bind_graphics_pipeline(&self.pipelines[&key]);
                    set_stencil_reference(&render_pass, 0);
                    render_pass.set_scissor(to_sdl_rect(full_target));
                    self.bind_texture(&render_pass, TextureId::WHITE);
                    render_pass.draw_primitives(6, 1, full_screen_first as usize, 0);
                }
                DrawCommand::Triangles { texture, program, stencil, scissor, first_vertex, vertex_count } => {
                    let key = PipelineKey { program: ProgramKind::of(program), stencil: stencil_use(stencil) };
                    render_pass.bind_graphics_pipeline(&self.pipelines[&key]);
                    match stencil {
                        StencilMode::Off => {}
                        StencilMode::Write { value } | StencilMode::Test { value, .. } => {
                            set_stencil_reference(&render_pass, *value)
                        }
                    }
                    let scissor = clamp_scissor(scissor.unwrap_or(full_target), width, height);
                    if scissor.width == 0 || scissor.height == 0 {
                        continue;
                    }
                    render_pass.set_scissor(to_sdl_rect(scissor));
                    self.bind_texture(&render_pass, *texture);
                    match program {
                        FragmentProgram::RoundedImage { quad_rect, size, radius } => command_buffer
                            .push_fragment_uniform_data(
                                0,
                                &RoundedImageUniforms {
                                    quad_rect: *quad_rect,
                                    size: *size,
                                    radius: *radius,
                                    padding: 0.0,
                                },
                            ),
                        FragmentProgram::Felt { zoom, world_offset } => command_buffer.push_fragment_uniform_data(
                            0,
                            &FeltUniforms { zoom: *zoom, world_offset: *world_offset, padding: 0.0 },
                        ),
                        FragmentProgram::Plain | FragmentProgram::Greyscale => {}
                    }
                    render_pass.draw_primitives(*vertex_count as usize, 1, *first_vertex as usize, 0);
                }
            }
        }
        self.device.end_render_pass(render_pass);

        let frame = &self.targets.as_ref().expect("ensured above").frame;
        command_buffer.blit_texture(
            BlitInfo::default()
                .with_source_texture(frame)
                .with_source_region(0, 0, 0, width, height)
                .with_destination_texture(&swapchain)
                .with_destination_region(0, 0, 0, width, height)
                .with_load_op(LoadOp::DONT_CARE)
                .with_filter(Filter::Nearest),
        );
        if !capture {
            command_buffer.submit().map_err(sdl_error)?;
            return Ok(None);
        }
        self.download_frame(command_buffer, width, height).map(Some)
    }

    /// Copies the finished frame back to the CPU as RGBA8 (waits for the GPU).
    fn download_frame(
        &self,
        command_buffer: sdl3::gpu::CommandBuffer,
        width: u32,
        height: u32,
    ) -> RenderResult<CapturedFrame> {
        let frame = &self.targets.as_ref().expect("ensured before rendering").frame;
        let byte_count = width * height * 4;
        let download = self
            .device
            .create_transfer_buffer()
            .with_usage(TransferBufferUsage::DOWNLOAD)
            .with_size(byte_count)
            .build()
            .map_err(sdl_error)?;
        let copy_pass = self.device.begin_copy_pass(&command_buffer).map_err(sdl_error)?;
        let region = sdl3::sys::gpu::SDL_GPUTextureRegion {
            texture: frame.raw(),
            mip_level: 0,
            layer: 0,
            x: 0,
            y: 0,
            z: 0,
            w: width,
            h: height,
            d: 1,
        };
        let destination = sdl3::sys::gpu::SDL_GPUTextureTransferInfo {
            transfer_buffer: download.raw(),
            offset: 0,
            pixels_per_row: width,
            rows_per_layer: height,
        };
        unsafe { sdl3::sys::gpu::SDL_DownloadFromGPUTexture(copy_pass.raw(), &region, &destination) };
        self.device.end_copy_pass(copy_pass);
        let fence = command_buffer.submit_and_acquire_fence(&self.device).map_err(sdl_error)?;
        let fences = [fence.raw()];
        unsafe { sdl3::sys::gpu::SDL_WaitForGPUFences(self.device.raw(), true, fences.as_ptr(), 1) };
        let mapped = download.map::<u8>(&self.device, false);
        let mut rgba = mapped.mem()[..byte_count as usize].to_vec();
        mapped.unmap();
        if matches!(self.swapchain_format, TextureFormat::B8g8r8a8Unorm | TextureFormat::B8g8r8a8UnormSrgb) {
            for pixel in rgba.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        Ok(CapturedFrame { width, height, rgba })
    }

    fn bind_texture(&self, render_pass: &sdl3::gpu::RenderPass, id: TextureId) {
        let texture = self.textures.get(&id).unwrap_or(&self.textures[&TextureId::WHITE]);
        render_pass.bind_fragment_samplers(
            0,
            &[TextureSamplerBinding::new().with_texture(texture).with_sampler(&self.sampler)],
        );
    }

    fn ensure_targets(&mut self, width: u32, height: u32) -> RenderResult<()> {
        if self.targets.as_ref().is_some_and(|targets| targets.width == width && targets.height == height) {
            return Ok(());
        }
        let multisampled_color = if self.is_multisampled() {
            Some(
                self.device
                    .create_texture(
                        TextureCreateInfo::new()
                            .with_type(TextureType::_2D)
                            .with_format(self.swapchain_format)
                            .with_usage(TextureUsage::COLOR_TARGET)
                            .with_width(width)
                            .with_height(height)
                            .with_layer_count_or_depth(1)
                            .with_num_levels(1)
                            .with_sample_count(self.sample_count),
                    )
                    .map_err(sdl_error)?,
            )
        } else {
            None
        };
        let depth_stencil = self
            .device
            .create_texture(
                TextureCreateInfo::new()
                    .with_type(TextureType::_2D)
                    .with_format(self.depth_stencil_format)
                    .with_usage(TextureUsage::DEPTH_STENCIL_TARGET)
                    .with_width(width)
                    .with_height(height)
                    .with_layer_count_or_depth(1)
                    .with_num_levels(1)
                    .with_sample_count(self.sample_count),
            )
            .map_err(sdl_error)?;
        let frame = self
            .device
            .create_texture(
                TextureCreateInfo::new()
                    .with_type(TextureType::_2D)
                    .with_format(self.swapchain_format)
                    .with_usage(TextureUsage::COLOR_TARGET | TextureUsage::SAMPLER)
                    .with_width(width)
                    .with_height(height)
                    .with_layer_count_or_depth(1)
                    .with_num_levels(1),
            )
            .map_err(sdl_error)?;
        self.targets = Some(FrameTargets { width, height, multisampled_color, depth_stencil, frame });
        Ok(())
    }

    fn upload_vertices(&mut self, command_buffer: &sdl3::gpu::CommandBuffer, vertices: &[Vertex]) -> RenderResult<()> {
        let needed = vertices.len() as u32;
        if self.vertices.as_ref().is_none_or(|storage| storage.capacity_in_vertices < needed) {
            let capacity = needed.next_power_of_two().max(4096);
            let size = capacity * std::mem::size_of::<Vertex>() as u32;
            let buffer = self
                .device
                .create_buffer()
                .with_usage(BufferUsageFlags::VERTEX)
                .with_size(size)
                .build()
                .map_err(sdl_error)?;
            let upload = self
                .device
                .create_transfer_buffer()
                .with_usage(TransferBufferUsage::UPLOAD)
                .with_size(size)
                .build()
                .map_err(sdl_error)?;
            self.vertices = Some(VertexStorage { capacity_in_vertices: capacity, buffer, upload });
        }
        let storage = self.vertices.as_ref().expect("allocated above");
        let mut mapped = storage.upload.map::<Vertex>(&self.device, true);
        mapped.mem_mut()[..vertices.len()].copy_from_slice(vertices);
        mapped.unmap();
        let copy_pass = self.device.begin_copy_pass(command_buffer).map_err(sdl_error)?;
        copy_pass.upload_to_gpu_buffer(
            TransferBufferLocation::new().with_transfer_buffer(&storage.upload).with_offset(0),
            BufferRegion::new()
                .with_buffer(&storage.buffer)
                .with_offset(0)
                .with_size(needed * std::mem::size_of::<Vertex>() as u32),
            true,
        );
        self.device.end_copy_pass(copy_pass);
        Ok(())
    }

    fn ensure_pipeline(&mut self, key: PipelineKey) -> RenderResult<()> {
        if self.pipelines.contains_key(&key) {
            return Ok(());
        }
        let writes_color = key.stencil != StencilUse::Write;
        let blend = ColorTargetBlendState::new()
            .with_enable_blend(true)
            .with_src_color_blendfactor(BlendFactor::SrcAlpha)
            .with_dst_color_blendfactor(BlendFactor::OneMinusSrcAlpha)
            .with_color_blend_op(BlendOp::Add)
            .with_src_alpha_blendfactor(BlendFactor::One)
            .with_dst_alpha_blendfactor(BlendFactor::OneMinusSrcAlpha)
            .with_alpha_blend_op(BlendOp::Add)
            .with_enable_color_write_mask(true)
            .with_color_write_mask(if writes_color {
                ColorComponentFlags::R | ColorComponentFlags::G | ColorComponentFlags::B | ColorComponentFlags::A
            } else {
                ColorComponentFlags(sdl3::sys::gpu::SDL_GPUColorComponentFlags(0))
            });
        let color_targets = [ColorTargetDescription::new().with_format(self.swapchain_format).with_blend_state(blend)];
        let stencil_state = || match key.stencil {
            StencilUse::Off => StencilOpState::new()
                .compare_op(CompareOp::Always)
                .with_pass_op(StencilOp::Keep)
                .with_fail_op(StencilOp::Keep)
                .with_depth_fail_op(StencilOp::Keep),
            StencilUse::Write => StencilOpState::new()
                .compare_op(CompareOp::Always)
                .with_pass_op(StencilOp::Replace)
                .with_fail_op(StencilOp::Keep)
                .with_depth_fail_op(StencilOp::Keep),
            StencilUse::Test(compare) => StencilOpState::new()
                .compare_op(gpu_compare_for(compare))
                .with_pass_op(StencilOp::Keep)
                .with_fail_op(StencilOp::Keep)
                .with_depth_fail_op(StencilOp::Keep),
        };
        let depth_stencil = DepthStencilState::new()
            .with_enable_depth_test(false)
            .with_enable_depth_write(false)
            .with_enable_stencil_test(key.stencil != StencilUse::Off)
            .with_front_stencil_state(stencil_state())
            .with_back_stencil_state(stencil_state())
            .with_compare_mask(0xFF)
            .with_write_mask(0xFF);
        let vertex_buffers = [VertexBufferDescription::new()
            .with_slot(0)
            .with_pitch(std::mem::size_of::<Vertex>() as u32)
            .with_input_rate(VertexInputRate::Vertex)
            .with_instance_step_rate(0)];
        let attributes = [
            VertexAttribute::new()
                .with_location(0)
                .with_buffer_slot(0)
                .with_format(VertexElementFormat::Float2)
                .with_offset(0),
            VertexAttribute::new()
                .with_location(1)
                .with_buffer_slot(0)
                .with_format(VertexElementFormat::Float2)
                .with_offset(8),
            VertexAttribute::new()
                .with_location(2)
                .with_buffer_slot(0)
                .with_format(VertexElementFormat::Float4)
                .with_offset(16),
        ];
        let pipeline = self
            .device
            .create_graphics_pipeline()
            .with_vertex_shader(&self.vertex_shader)
            .with_fragment_shader(&self.fragment_shaders[&key.program])
            .with_primitive_type(PrimitiveType::TriangleList)
            .with_rasterizer_state(RasterizerState::new().with_cull_mode(CullMode::None))
            .with_multisample_state(MultisampleState::new().with_sample_count(self.sample_count))
            .with_depth_stencil_state(depth_stencil)
            .with_vertex_input_state(
                VertexInputState::new()
                    .with_vertex_buffer_descriptions(&vertex_buffers)
                    .with_vertex_attributes(&attributes),
            )
            .with_target_info(
                GraphicsPipelineTargetInfo::new()
                    .with_color_target_descriptions(&color_targets)
                    .with_has_depth_stencil_target(true)
                    .with_depth_stencil_format(self.depth_stencil_format),
            )
            .build()
            .map_err(sdl_error)?;
        self.pipelines.insert(key, pipeline);
        Ok(())
    }
}

fn stencil_use(stencil: &StencilMode) -> StencilUse {
    match stencil {
        StencilMode::Off => StencilUse::Off,
        StencilMode::Write { .. } => StencilUse::Write,
        StencilMode::Test { compare, .. } => StencilUse::Test(*compare),
    }
}

/// The GPU tests `reference <op> stored`; LÖVE names the comparison of the stored value against
/// the reference, so the ordered comparisons swap sides.
fn gpu_compare_for(compare: StencilCompare) -> CompareOp {
    match compare {
        StencilCompare::Equal => CompareOp::Equal,
        StencilCompare::NotEqual => CompareOp::NotEqual,
        StencilCompare::Less => CompareOp::Greater,
        StencilCompare::LessOrEqual => CompareOp::GreaterOrEqual,
        StencilCompare::Greater => CompareOp::Less,
        StencilCompare::GreaterOrEqual => CompareOp::LessOrEqual,
        StencilCompare::Always => CompareOp::Always,
        StencilCompare::Never => CompareOp::Never,
    }
}

fn set_stencil_reference(render_pass: &sdl3::gpu::RenderPass, value: u8) {
    unsafe { sdl3::sys::gpu::SDL_SetGPUStencilReference(render_pass.raw(), value) }
}

fn texture_supports_sample_count(device: &Device, format: TextureFormat, count: SampleCount) -> bool {
    unsafe {
        sdl3::sys::gpu::SDL_GPUTextureSupportsSampleCount(
            device.raw(),
            sdl3::sys::gpu::SDL_GPUTextureFormat(format as i32),
            sdl3::sys::gpu::SDL_GPUSampleCount(count as i32),
        )
    }
}

fn texture_supports_format(device: &Device, format: TextureFormat, usage: TextureUsage) -> bool {
    unsafe {
        sdl3::sys::gpu::SDL_GPUTextureSupportsFormat(
            device.raw(),
            sdl3::sys::gpu::SDL_GPUTextureFormat(format as i32),
            sdl3::sys::gpu::SDL_GPUTextureType::_2D,
            usage.0,
        )
    }
}

/// Keeps a scissor rectangle inside the render target.
fn clamp_scissor(rect: ScissorRect, width: u32, height: u32) -> ScissorRect {
    let left = rect.x.clamp(0, width as i32);
    let top = rect.y.clamp(0, height as i32);
    let right = (rect.x + rect.width).clamp(left, width as i32);
    let bottom = (rect.y + rect.height).clamp(top, height as i32);
    ScissorRect { x: left, y: top, width: right - left, height: bottom - top }
}

fn to_sdl_rect(rect: ScissorRect) -> sdl3::rect::Rect {
    sdl3::rect::Rect::new(rect.x, rect.y, rect.width as u32, rect.height as u32)
}

fn channel_to_byte(channel: f32) -> u8 {
    (channel.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scissors_stay_inside_the_target() {
        let clamped = clamp_scissor(ScissorRect { x: -10, y: 5, width: 50, height: 2000 }, 100, 100);
        assert_eq!(clamped, ScissorRect { x: 0, y: 5, width: 40, height: 95 });
        let outside = clamp_scissor(ScissorRect { x: 200, y: 0, width: 10, height: 10 }, 100, 100);
        assert_eq!((outside.width, outside.height), (0, 10));
    }

    #[test]
    fn love_compare_modes_swap_sides_for_the_gpu() {
        assert_eq!(gpu_compare_for(StencilCompare::Greater), CompareOp::Less);
        assert_eq!(gpu_compare_for(StencilCompare::Equal), CompareOp::Equal);
    }
}
