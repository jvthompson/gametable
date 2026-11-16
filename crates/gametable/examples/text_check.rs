//! Draws a block of text (every face, icons, symbol fallback, printf alignments) at a UI scale
//! and saves it as a PNG, for side-by-side comparison with `tools/lua_golden/text_check` (the
//! same block drawn by LÖVE).
//!
//! `cargo run -p gametable --example text_check -- <output.png> [scale]`

use std::path::PathBuf;

use gametable::graphics::draw_list::{Color, DrawList};
use gametable::graphics::gpu_renderer::GpuRenderer;
use gametable::graphics::image_data::RgbaImage;
use gametable::graphics::tessellation::DrawMode;
use gametable::graphics::text::{FontFace, FontKey, FontLibrary};
use gametable::graphics::text_layout::Align;

const PARAGRAPH: &str = "Drag cards onto the table, right-click for more options and press F1 for help.";

fn main() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let output = PathBuf::from(arguments.next().unwrap_or_else(|| "text_check.png".to_string()));
    let scale: f32 = arguments.next().and_then(|text| text.parse().ok()).unwrap_or(1.0);
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let sdl = sdl3::init().map_err(|e| e.to_string())?;
    let video = sdl.video().map_err(|e| e.to_string())?;
    let (width, height) = ((900.0 * scale) as u32, (260.0 * scale) as u32);
    let window =
        video.window("GameTable text check", width, height).position_centered().build().map_err(|e| e.to_string())?;
    let mut renderer = GpuRenderer::new(&window)?;
    let mut fonts = FontLibrary::new(&assets.join("fonts"))?;
    fonts.set_dpi_scale(scale);
    let mut events = sdl.event_pump().map_err(|e| e.to_string())?;
    for frame in 0..4 {
        for _ in events.poll_iter() {}
        let mut list = DrawList::new();
        list.scale(scale, scale);
        let ink = Color::rgba(0.098, 0.114, 0.09, 1.0);
        list.set_color(ink);
        fonts.print(
            &mut list,
            FontKey::new(FontFace::Regular, 14.0),
            "The quick brown fox jumps over the lazy dog 0123456789",
            10.0,
            10.0,
        );
        fonts.print(
            &mut list,
            FontKey::new(FontFace::Medium, 16.0),
            "CardTable \u{2014} Ace of Spades \u{2660} \u{2665} \u{2666} \u{2663}",
            10.0,
            34.0,
        );
        fonts.print(&mut list, FontKey::new(FontFace::Bold, 20.0), "Bold heading 24\u{b0}", 10.0, 60.0);
        let icons: String =
            [0xe5c4u32, 0xe5cd, 0xe8b8, 0xe51c, 0xe834].iter().filter_map(|&c| char::from_u32(c)).collect();
        fonts.print(&mut list, FontKey::new(FontFace::Icons, 24.0), &icons, 10.0, 90.0);
        let key = FontKey::new(FontFace::Regular, 14.0);
        for (index, align) in [Align::Left, Align::Center, Align::Right, Align::Justify].into_iter().enumerate() {
            let x = 10.0 + index as f32 * 220.0;
            list.set_color(Color::rgba(0.45, 0.47, 0.44, 1.0));
            list.rectangle(DrawMode::Line, x, 130.0, 200.0, 110.0, 0.0, 0.0);
            list.set_color(ink);
            fonts.printf(&mut list, key, PARAGRAPH, x, 130.0, 200.0, align);
        }
        fonts.flush(&mut renderer)?;
        let background = Color::rgba(0.973, 0.984, 0.945, 1.0);
        if let Some(captured) = renderer.render(&window, &list, background, frame == 3)? {
            RgbaImage { width: captured.width, height: captured.height, pixels: captured.rgba }.save_png(&output)?;
            println!("saved {}", output.display());
        }
    }
    Ok(())
}
