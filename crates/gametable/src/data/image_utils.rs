//! In-place fixes to card art files, keeping each file's name and format.
//!
//! Landscape-to-portrait correction for imported card art is a port of
//! `imageUtils.rotateLandscapeImages` in `src/data/image_utils.lua`: every landscape image is
//! rotated 90° clockwise. The original encoded JPEGs through a PowerShell/System.Drawing helper
//! (LÖVE can't write JPEG); here SDL3_image writes them in-process. Best effort, as before: a
//! file that fails is left as copied.
//!
//! Shrinking art to a maximum height (the editor's "Shrink Card Art") is a GameTable addition.

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
    replace_image_file(path, &image.rotated_clockwise())?;
    Ok(true)
}

/// Writes an image over `path` in the format its extension names (`.png`, `.jpg` or `.jpeg`).
/// The new image goes to a temporary file first and then replaces the original, so a failed
/// write never leaves a half-written card image behind.
fn replace_image_file(path: &str, image: &RgbaImage) -> Result<(), String> {
    let temporary_path = format!("{path}.gametable-tmp");
    let written = if lowercase_extension(path) == ".png" {
        image.encode_png().and_then(|bytes| std::fs::write(&temporary_path, bytes).map_err(|error| error.to_string()))
    } else {
        image.save_jpeg(Path::new(&temporary_path), JPEG_QUALITY)
    };
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error);
    }
    std::fs::rename(&temporary_path, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary_path);
        format!("{path}: {error}")
    })
}

/// What [`shrink_image_to_max_height`] did to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShrinkOutcome {
    /// Rewritten smaller; the sizes are (width, height) before and after.
    Shrunk { original_size: (u32, u32), new_size: (u32, u32) },
    /// Already no taller than the limit; the file is untouched.
    AlreadySmallEnough,
    /// Not a PNG or JPEG, which are the formats that can be written back; the file is untouched.
    UnsupportedFormat,
}

/// Scales one image file down in place so it is no taller than `max_height`, keeping its aspect
/// ratio (the width follows from the image's own proportions) and its format.
pub fn shrink_image_to_max_height(path: &str, max_height: u32) -> Result<ShrinkOutcome, String> {
    if !matches!(lowercase_extension(path).as_str(), ".png" | ".jpg" | ".jpeg") {
        return Ok(ShrinkOutcome::UnsupportedFormat);
    }
    let image = RgbaImage::load(Path::new(path))?;
    if image.height <= max_height {
        return Ok(ShrinkOutcome::AlreadySmallEnough);
    }
    let original_size = (image.width, image.height);
    let shrunk = image.downscaled_to_max_height(max_height);
    replace_image_file(path, &shrunk)?;
    Ok(ShrinkOutcome::Shrunk { original_size, new_size: (shrunk.width, shrunk.height) })
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

    #[test]
    fn tall_png_and_jpeg_shrink_to_the_maximum_height() {
        let folder = std::env::temp_dir().join(format!("gametable_shrink_{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let tall_png = folder.join("tall.png");
        image(300, 420).save_png(&tall_png).unwrap();
        let wide_jpeg = folder.join("wide.jpeg");
        image(500, 250).save_jpeg(&wide_jpeg, 90).unwrap();
        let short_png = folder.join("short.png");
        image(50, 70).save_png(&short_png).unwrap();
        let path_of = |path: &std::path::PathBuf| path.to_string_lossy().into_owned();

        assert_eq!(
            shrink_image_to_max_height(&path_of(&tall_png), 100),
            Ok(ShrinkOutcome::Shrunk { original_size: (300, 420), new_size: (71, 100) })
        );
        let reloaded = RgbaImage::load(&tall_png).unwrap();
        assert_eq!((reloaded.width, reloaded.height), (71, 100));
        assert_eq!(
            shrink_image_to_max_height(&path_of(&wide_jpeg), 100),
            Ok(ShrinkOutcome::Shrunk { original_size: (500, 250), new_size: (200, 100) })
        );
        assert_eq!(shrink_image_to_max_height(&path_of(&short_png), 100), Ok(ShrinkOutcome::AlreadySmallEnough));
        assert_eq!(
            shrink_image_to_max_height(folder.join("x.webp").to_str().unwrap(), 100),
            Ok(ShrinkOutcome::UnsupportedFormat)
        );
        assert!(shrink_image_to_max_height(folder.join("missing.png").to_str().unwrap(), 100).is_err());
        // No temporary files are left behind.
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 3);
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
