//! Phase 0 spike: checks that the SDL3 features the port relies on work on this machine.
//!
//! It opens a borderless resizable window with a custom hit-test (top bar drags, edges resize),
//! creates an SDL3 GPU device, decodes an image with SDL3_image, renders text with SDL3_ttf
//! (including the symbol fallback font), clears the swapchain for a couple of seconds, then exits.
//!
//! Run from the workspace root: `cargo run -p gametable --example sdl3_spike`

use std::time::{Duration, Instant};

use sdl3::event::Event;
use sdl3::gpu::{ColorTargetInfo, Device, LoadOp, ShaderFormat, StoreOp};
use sdl3::image::LoadSurface;
use sdl3::pixels::Color;
use sdl3::surface::Surface;
use sdl3::video::HitTestResult;

const WINDOW_WIDTH: u32 = 1280;
const WINDOW_HEIGHT: u32 = 720;
const TITLE_BAR_HEIGHT: i32 = 32;
const RESIZE_BORDER_THICKNESS: i32 = 6;
const SPIKE_DURATION: Duration = Duration::from_secs(2);

fn classify_window_point_for_hit_test(x: i32, y: i32, width: i32, height: i32) -> HitTestResult {
    let near_left = x < RESIZE_BORDER_THICKNESS;
    let near_right = x >= width - RESIZE_BORDER_THICKNESS;
    let near_top = y < RESIZE_BORDER_THICKNESS;
    let near_bottom = y >= height - RESIZE_BORDER_THICKNESS;
    match (near_left, near_right, near_top, near_bottom) {
        (true, _, true, _) => HitTestResult::ResizeTopLeft,
        (_, true, true, _) => HitTestResult::ResizeTopRight,
        (true, _, _, true) => HitTestResult::ResizeBottomLeft,
        (_, true, _, true) => HitTestResult::ResizeBottomRight,
        (true, _, _, _) => HitTestResult::ResizeLeft,
        (_, true, _, _) => HitTestResult::ResizeRight,
        (_, _, true, _) => HitTestResult::ResizeTop,
        (_, _, _, true) => HitTestResult::ResizeBottom,
        _ if y < TITLE_BAR_HEIGHT => HitTestResult::Draggable,
        _ => HitTestResult::Normal,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let assets_directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");

    let sdl_context = sdl3::init()?;
    let video_subsystem = sdl_context.video()?;
    let mut window = video_subsystem
        .window("GameTable SDL3 spike", WINDOW_WIDTH, WINDOW_HEIGHT)
        .position_centered()
        .resizable()
        .borderless()
        .build()?;
    window.set_hit_test(|point| {
        classify_window_point_for_hit_test(point.x, point.y, WINDOW_WIDTH as i32, WINDOW_HEIGHT as i32)
    })?;
    println!("window: borderless, hit-test installed");

    let gpu_device =
        Device::new(ShaderFormat::SPIRV | ShaderFormat::DXIL, cfg!(debug_assertions))?.with_window(&window)?;
    println!("gpu: device created, supported shader formats {:?}", gpu_device.get_shader_formats());

    let logo_surface = Surface::from_file(assets_directory.join("logo.png"))?;
    println!("image: decoded logo.png at {}x{}", logo_surface.width(), logo_surface.height());

    let ttf_context = sdl3::ttf::init()?;
    let mut roboto_font = ttf_context.load_font(assets_directory.join("fonts/Roboto-Regular.ttf"), 24.0)?;
    let symbols_font = ttf_context.load_font(assets_directory.join("fonts/NotoSansSymbols2-Regular.ttf"), 24.0)?;
    roboto_font.add_fallback_font(&symbols_font)?;
    let text_surface = roboto_font.render("Ace of Spades \u{2660} \u{2665}").blended(Color::RGB(255, 255, 255))?;
    println!("ttf: rendered text with symbol fallback at {}x{}", text_surface.width(), text_surface.height());

    let mut event_pump = sdl_context.event_pump()?;
    let spike_start = Instant::now();
    let mut presented_frame_count = 0u32;
    'frames: while spike_start.elapsed() < SPIKE_DURATION {
        for event in event_pump.poll_iter() {
            if let Event::Quit { .. } = event {
                break 'frames;
            }
        }
        let mut command_buffer = gpu_device.acquire_command_buffer()?;
        if let Ok(Some(swapchain_texture)) = command_buffer.wait_and_acquire_swapchain_texture(&window) {
            let felt_green = Color::RGB(0x2e, 0x7d, 0x32);
            let color_targets = [ColorTargetInfo::default()
                .with_texture(&swapchain_texture)
                .with_load_op(LoadOp::CLEAR)
                .with_store_op(StoreOp::STORE)
                .with_clear_color(felt_green)];
            let render_pass = gpu_device.begin_render_pass(&command_buffer, &color_targets, None)?;
            gpu_device.end_render_pass(render_pass);
            command_buffer.submit()?;
            presented_frame_count += 1;
        } else {
            command_buffer.cancel();
        }
    }
    println!("gpu: presented {presented_frame_count} frames");
    Ok(())
}
