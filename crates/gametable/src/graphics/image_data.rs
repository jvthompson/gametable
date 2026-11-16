//! Decoded images as plain RGBA8 pixels: decoding (SDL3_image, or libwebp for WebP), saving as
//! PNG, and the pixel operations of `image_utils.lua` (area-average downscale, rotation).

use std::path::Path;

use sdl3::image::{ImageIOStream, SaveSurface};
use sdl3::iostream::IOStream;
use sdl3::pixels::PixelFormat;
use sdl3::surface::Surface;

use super::webp;

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

    /// Decodes image file bytes: WebP through libwebp, anything else SDL3_image supports (PNG,
    /// JPEG, BMP, TGA, GIF, ...). Port of `imageUtils.decodeBytes`.
    pub fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
        if webp::is_webp(bytes) {
            return webp::decode(bytes);
        }
        let stream = IOStream::from_bytes(bytes).map_err(|error| error.to_string())?;
        let surface = stream.load().map_err(|error| error.to_string())?;
        RgbaImage::from_surface(&surface)
    }

    /// Reads and decodes a file (`imageUtils.decodeFile`).
    pub fn load(path: &Path) -> Result<RgbaImage, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        RgbaImage::decode(&bytes)
    }

    fn to_surface(&self) -> Result<Surface<'static>, String> {
        let mut surface =
            Surface::new(self.width, self.height, PixelFormat::RGBA32).map_err(|error| error.to_string())?;
        let pitch = surface.pitch() as usize;
        let row_bytes = self.width as usize * 4;
        surface.with_lock_mut(|data| {
            for row in 0..self.height as usize {
                data[row * pitch..row * pitch + row_bytes]
                    .copy_from_slice(&self.pixels[row * row_bytes..(row + 1) * row_bytes]);
            }
        });
        Ok(surface)
    }

    /// Writes the image as a PNG file.
    pub fn save_png(&self, path: &Path) -> Result<(), String> {
        self.to_surface()?.save(path).map_err(|error| error.to_string())
    }

    /// The image as PNG file bytes (`imageUtils.encodePng`).
    pub fn encode_png(&self) -> Result<Vec<u8>, String> {
        use sdl3::sys::iostream::{SDL_GetIOSize, SDL_IO_SEEK_SET, SDL_IOFromDynamicMem, SDL_ReadIO, SDL_SeekIO};
        let surface = self.to_surface()?;
        let raw = unsafe { SDL_IOFromDynamicMem() };
        if raw.is_null() {
            return Err(sdl3::get_error().to_string());
        }
        // Closed (and its memory freed) when `stream` drops.
        let mut stream = unsafe { IOStream::from_ll(raw) };
        surface.save_io(&mut stream).map_err(|error| error.to_string())?;
        let size = unsafe { SDL_GetIOSize(stream.raw()) }.max(0) as usize;
        let mut bytes = vec![0u8; size];
        unsafe {
            SDL_SeekIO(stream.raw(), 0, SDL_IO_SEEK_SET);
            SDL_ReadIO(stream.raw(), bytes.as_mut_ptr().cast(), size);
        }
        Ok(bytes)
    }

    /// Rotates 90° clockwise.
    pub fn rotated_clockwise(&self) -> RgbaImage {
        let (width, height) = (self.width as usize, self.height as usize);
        let mut pixels = vec![0u8; self.pixels.len()];
        // (x, y) in the source -> (height - 1 - y, x) in the result, whose width is `height`.
        for y in 0..height {
            for x in 0..width {
                let source = (y * width + x) * 4;
                let target = (x * height + (height - 1 - y)) * 4;
                pixels[target..target + 4].copy_from_slice(&self.pixels[source..source + 4]);
            }
        }
        RgbaImage { width: self.height, height: self.width, pixels }
    }

    /// Area-average downscale so the longer side is at most `max_dimension`; unchanged when it
    /// already fits. Port of `imageUtils.downscaleToFit` (same box boundaries and rounding).
    pub fn downscaled_to_fit(self, max_dimension: u32) -> RgbaImage {
        let (source_width, source_height) = (self.width as usize, self.height as usize);
        let larger = source_width.max(source_height);
        if larger <= max_dimension as usize {
            return self;
        }
        let scale = max_dimension as f64 / larger as f64;
        let target_width = ((source_width as f64 * scale + 0.5).floor() as usize).max(1);
        let target_height = ((source_height as f64 * scale + 0.5).floor() as usize).max(1);
        let step_x = source_width as f64 / target_width as f64;
        let step_y = source_height as f64 / target_height as f64;
        let mut pixels = vec![0u8; target_width * target_height * 4];
        for target_y in 0..target_height {
            let y0 = (target_y as f64 * step_y).floor() as usize;
            let y1 = (y0 + 1).max(source_height.min(((target_y + 1) as f64 * step_y).floor() as usize));
            for target_x in 0..target_width {
                let x0 = (target_x as f64 * step_x).floor() as usize;
                let x1 = (x0 + 1).max(source_width.min(((target_x + 1) as f64 * step_x).floor() as usize));
                let mut sums = [0u64; 4];
                let mut count = 0u64;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let index = (y * source_width + x) * 4;
                        for channel in 0..4 {
                            sums[channel] += u64::from(self.pixels[index + channel]);
                        }
                        count += 1;
                    }
                }
                let output = (target_y * target_width + target_x) * 4;
                for channel in 0..4 {
                    pixels[output + channel] = (sums[channel] as f64 / count as f64 + 0.5).floor() as u8;
                }
            }
        }
        RgbaImage { width: target_width as u32, height: target_height as u32, pixels }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, value: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&value(x, y));
            }
        }
        RgbaImage { width, height, pixels }
    }

    #[test]
    fn rotation_moves_the_top_left_to_the_top_right() {
        let source = image(3, 2, |x, y| [x as u8, y as u8, 0, 255]);
        let rotated = source.rotated_clockwise();
        assert_eq!((rotated.width, rotated.height), (2, 3));
        // Source (0, 0) -> result (1, 0); source (2, 1) -> result (0, 2).
        assert_eq!(&rotated.pixels[4..8], &[0, 0, 0, 255]);
        assert_eq!(&rotated.pixels[(2 * 2) * 4..(2 * 2) * 4 + 4], &[2, 1, 0, 255]);
    }

    #[test]
    fn downscale_averages_boxes() {
        let source = image(4, 2, |x, _| [if x < 2 { 0 } else { 200 }, 10, 20, 255]);
        let small = source.clone().downscaled_to_fit(2);
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(small.pixels, vec![0, 10, 20, 255, 200, 10, 20, 255]);
        let odd = image(3, 3, |x, y| [(x + 3 * y) as u8 * 10, 0, 0, 255]).downscaled_to_fit(2);
        // 3 -> 2: boxes [0, 1) and [1, 3).
        assert_eq!((odd.width, odd.height), (2, 2));
        assert_eq!(odd.pixels[0], 0);
        assert_eq!(odd.pixels[4], 15); // (10 + 20) / 2
        assert!(source.clone().downscaled_to_fit(1024) == source);
    }

    #[test]
    fn png_round_trip_through_memory() {
        let source = image(5, 3, |x, y| [x as u8 * 40, y as u8 * 60, 7, 128 + x as u8]);
        let decoded = RgbaImage::decode(&source.encode_png().unwrap()).unwrap();
        assert_eq!(decoded, source);
        assert!(RgbaImage::decode(b"not an image").is_err());
    }
}
