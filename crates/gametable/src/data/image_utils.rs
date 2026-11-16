//! Landscape-to-portrait correction for imported card art. Port of
//! `imageUtils.rotateLandscapeImages` in `src/data/image_utils.lua`: every landscape image is
//! rotated 90° clockwise in place, keeping its format. The original encoded JPEGs through a
//! PowerShell/System.Drawing helper (LÖVE can't write JPEG); here SDL3_image writes them
//! in-process. Best effort, as before: a file that fails is left as copied.

use std::path::Path;

use crate::data::file_system::lowercase_extension;
use crate::graphics::image_data::RgbaImage;

/// JPEG quality for rotated art (System.Drawing's default encoder quality was 75).
const JPEG_QUALITY: i32 = 90;

/// Rotates one file if it is landscape. Returns whether it was rotated.
pub fn rotate_landscape_image(path: &str) -> Result<bool, String> {
    let extension = lowercase_extension(path);
    if !matches!(extension.as_str(), ".png" | ".jpg" | ".jpeg") {
        return Ok(false);
    }
    let image = RgbaImage::load(Path::new(path))?;
    if image.width <= image.height {
        return Ok(false);
    }
    let rotated = image.rotated_clockwise();
    if extension == ".png" {
        std::fs::write(path, rotated.encode_png()?).map_err(|error| error.to_string())?;
    } else {
        rotated.save_jpeg(Path::new(path), JPEG_QUALITY)?;
    }
    Ok(true)
}

/// `I.rotateLandscapeImages(paths)`.
pub fn rotate_landscape_images(paths: &[String]) {
    for path in paths {
        let _ = rotate_landscape_image(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32) -> RgbaImage {
        let mut pixels = vec![255u8; (width * height * 4) as usize];
        // A red top-left pixel to follow the rotation.
        pixels[0..4].copy_from_slice(&[255, 0, 0, 255]);
        RgbaImage { width, height, pixels }
    }

    #[test]
    fn landscape_png_and_jpeg_turn_portrait() {
        let folder = std::env::temp_dir().join(format!("gametable_rotate_{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let png = folder.join("wide.png");
        image(40, 20).save_png(&png).unwrap();
        let tall = folder.join("tall.png");
        image(20, 40).save_png(&tall).unwrap();
        let jpeg = folder.join("wide.JPG");
        image(40, 20).save_jpeg(&jpeg, 90).unwrap();
        let paths: Vec<String> = [&png, &tall, &jpeg].iter().map(|path| path.to_string_lossy().into_owned()).collect();
        assert!(rotate_landscape_image(&paths[0]).unwrap());
        assert!(!rotate_landscape_image(&paths[1]).unwrap());
        assert!(rotate_landscape_image(&paths[2]).unwrap());
        let rotated = RgbaImage::load(&png).unwrap();
        assert_eq!((rotated.width, rotated.height), (20, 40));
        // Clockwise: the top-left pixel ends up top-right.
        assert_eq!(&rotated.pixels[19 * 4..20 * 4], &[255, 0, 0, 255]);
        let rotated_jpeg = RgbaImage::load(&jpeg).unwrap();
        assert_eq!((rotated_jpeg.width, rotated_jpeg.height), (20, 40));
        assert_eq!(rotate_landscape_image(folder.join("x.webp").to_str().unwrap()), Ok(false));
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
