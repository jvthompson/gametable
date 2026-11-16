//! Decoded images as plain RGBA8 pixels, loaded and saved through SDL3_image.

use std::path::Path;

use sdl3::image::{LoadSurface, SaveSurface};
use sdl3::pixels::PixelFormat;
use sdl3::surface::Surface;

/// Straight (not premultiplied) RGBA8 pixels, rows top to bottom with no padding.
#[derive(Debug, Clone, PartialEq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    /// Copies a surface's pixels after converting it to RGBA byte order.
    pub fn from_surface(surface: &Surface) -> Result<RgbaImage, String> {
        let converted = surface.convert_format(PixelFormat::RGBA32).map_err(|error| error.to_string())?;
        let (width, height, pitch) = (converted.width(), converted.height(), converted.pitch() as usize);
        let row_bytes = width as usize * 4;
        let mut pixels = Vec::with_capacity(row_bytes * height as usize);
        converted.with_lock(|data| {
            for row in 0..height as usize {
                pixels.extend_from_slice(&data[row * pitch..row * pitch + row_bytes]);
            }
        });
        Ok(RgbaImage { width, height, pixels })
    }

    /// Decodes any format SDL3_image supports (PNG, JPEG, BMP, TGA, GIF, WebP, ...).
    pub fn load(path: &Path) -> Result<RgbaImage, String> {
        let surface = Surface::from_file(path).map_err(|error| error.to_string())?;
        RgbaImage::from_surface(&surface)
    }

    /// Writes the image as a PNG file.
    pub fn save_png(&self, path: &Path) -> Result<(), String> {
        let mut pixels = self.pixels.clone();
        let surface = Surface::from_data(&mut pixels, self.width, self.height, self.width * 4, PixelFormat::RGBA32)
            .map_err(|error| error.to_string())?;
        surface.save(path).map_err(|error| error.to_string())
    }
}
