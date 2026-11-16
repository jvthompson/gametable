//! Draws a test scene with every renderer feature (shapes with LÖVE's tessellation, lines,
//! textures, the four fragment programs, stencil clipping and scissor) and saves it as a PNG.
//!
//! `cargo run -p gametable --example render_check -- <output.png>`

use std::f32::consts::PI;
use std::path::PathBuf;

use gametable::graphics::draw_list::{Color, DrawList, FragmentProgram, ScissorRect, StencilCompare};
use gametable::graphics::gpu_renderer::GpuRenderer;
use gametable::graphics::image_data::RgbaImage;
use gametable::graphics::tessellation::{ArcMode, DrawMode};
use sdl3::event::Event;

fn main() -> Result<(), String> {
    let output = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "render_check.png".to_string()));
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let sdl = sdl3::init().map_err(|e| e.to_string())?;
    let video = sdl.video().map_err(|e| e.to_string())?;
    let window =
        video.window("GameTable render check", 960, 600).position_centered().build().map_err(|e| e.to_string())?;
    let mut renderer = GpuRenderer::new(&window)?;
    println!("multisampled: {}", renderer.is_multisampled());

    let logo = RgbaImage::load(&assets.join("logo.png"))?;
    let logo_texture = renderer.create_texture_from_rgba(logo.width, logo.height, &logo.pixels, true)?;

    let mut events = sdl.event_pump().map_err(|e| e.to_string())?;
    for frame in 0..4 {
        for event in events.poll_iter() {
            if let Event::Quit { .. } = event {
                return Ok(());
            }
        }
        let mut list = DrawList::new();
        // Felt background with the cloth grain.
        list.set_color(Color::from_argb(0xFF0B_6E4F));
        list.set_program(FragmentProgram::Felt { zoom: 1.0, world_offset: [0.0, 0.0] });
        list.rectangle(DrawMode::Fill, 0.0, 0.0, 960.0, 600.0, 0.0, 0.0);
        list.set_program(FragmentProgram::Plain);

        // Rounded rectangles, filled and outlined.
        list.set_color(Color::rgba(0.97, 0.98, 0.95, 1.0));
        list.rectangle(DrawMode::Fill, 20.0, 20.0, 200.0, 120.0, 12.0, 12.0);
        list.set_color(Color::rgba(0.23, 0.41, 0.22, 1.0));
        list.set_line_width(2.0);
        list.rectangle(DrawMode::Line, 20.0, 20.0, 200.0, 120.0, 12.0, 12.0);
        list.set_line_width(1.0);
        list.rectangle(DrawMode::Line, 40.0, 40.0, 60.0, 40.0, 0.0, 0.0);

        // Circles, a pie arc and the loading spinner's open arc.
        list.set_color(Color::rgba(0.96, 0.26, 0.21, 1.0));
        list.circle(DrawMode::Fill, 300.0, 80.0, 40.0, None);
        list.set_color(Color::rgba(1.0, 1.0, 1.0, 0.8));
        list.circle(DrawMode::Line, 300.0, 80.0, 50.0, None);
        list.set_color(Color::rgba(0.13, 0.59, 0.95, 1.0));
        list.arc(DrawMode::Fill, ArcMode::Pie, 420.0, 80.0, 40.0, 0.0, PI * 1.5, None);
        list.set_line_width(4.0);
        list.arc(DrawMode::Line, ArcMode::Open, 530.0, 80.0, 30.0, 0.0, PI * 1.4, Some(24));
        list.set_line_width(2.0);

        // A check mark polyline (as the UI's checkbox draws it) and a triangle.
        list.set_color(Color::WHITE);
        list.line(&[600.0, 80.0, 620.0, 100.0, 660.0, 50.0]);
        list.set_color(Color::rgba(1.0, 0.6, 0.0, 1.0));
        list.polygon(DrawMode::Fill, &[700.0, 120.0, 760.0, 40.0, 820.0, 120.0]);

        // The logo: plain, rotated + scaled, greyscale, and rounded-corner card image.
        list.set_color(Color::WHITE);
        list.draw_texture(logo_texture, None, 20.0, 180.0, 0.0, 0.25, 0.25, 0.0, 0.0);
        list.push();
        list.translate(330.0, 260.0);
        list.rotate(0.2);
        list.draw_texture(
            logo_texture,
            None,
            0.0,
            0.0,
            0.0,
            0.2,
            0.2,
            logo.width as f32 / 2.0,
            logo.height as f32 / 2.0,
        );
        list.pop();
        list.set_program(FragmentProgram::Greyscale);
        list.draw_texture(logo_texture, None, 460.0, 180.0, 0.0, 0.25, 0.25, 0.0, 0.0);
        let (card_w, card_h) = (logo.width as f32 * 0.25, logo.height as f32 * 0.25);
        list.set_program(FragmentProgram::RoundedImage {
            quad_rect: [0.0, 0.0, 1.0, 1.0],
            size: [card_w, card_h],
            radius: 24.0,
        });
        list.draw_texture(logo_texture, None, 20.0, 380.0, 0.0, 0.25, 0.25, 0.0, 0.0);
        list.set_program(FragmentProgram::Plain);

        // Stencil: the logo clipped to a rounded rectangle at half alpha (the playmat path).
        list.stencil(1, |list| list.rectangle(DrawMode::Fill, 460.0, 380.0, 200.0, 160.0, 30.0, 30.0));
        list.set_stencil_test(Some((StencilCompare::Greater, 0)));
        list.set_color(Color::rgba(1.0, 1.0, 1.0, 0.5));
        list.draw_texture(logo_texture, None, 420.0, 360.0, 0.0, 0.4, 0.4, 0.0, 0.0);
        list.set_stencil_test(None);

        // Scissor: a striped block cut to a smaller window.
        list.set_scissor(Some(ScissorRect { x: 720, y: 380, width: 120, height: 80 }));
        for stripe in 0..12 {
            let shade = if stripe % 2 == 0 { 0.9 } else { 0.2 };
            list.set_color(Color::rgba(shade, shade, shade, 1.0));
            list.rectangle(DrawMode::Fill, 700.0 + stripe as f32 * 15.0, 360.0, 15.0, 200.0, 0.0, 0.0);
        }
        list.set_scissor(None);

        let capture = frame == 3;
        if let Some(captured) = renderer.render(&window, &list, Color::BLACK, capture)? {
            RgbaImage { width: captured.width, height: captured.height, pixels: captured.rgba }.save_png(&output)?;
            println!("saved {}", output.display());
        }
    }
    Ok(())
}
