//! WebP decoding through `libwebp`, loaded at runtime like the original (`image_utils.lua`): the
//! app works without it and reports WebP images as needing `libwebp.dll` (`libwebp.so`,
//! `libwebp.dylib`) next to the app. SDL_image's bundled build has no WebP support. Where it is
//! searched for: [`crate::native_libraries`].

use std::path::PathBuf;
use std::sync::OnceLock;

use super::image_data::RgbaImage;
use crate::native_libraries;

type DecodeRgba = unsafe extern "C" fn(*const u8, usize, *mut i32, *mut i32) -> *mut u8;
type Free = unsafe extern "C" fn(*mut std::ffi::c_void);

struct WebpLibrary {
    decode_rgba: DecodeRgba,
    free: Free,
    /// Kept loaded for the functions above (and `libsharpyuv`, which `libwebp` imports).
    _libraries: Vec<libloading::Library>,
}

fn load_library() -> Option<WebpLibrary> {
    let mut libraries = Vec::new();
    // libwebp imports libsharpyuv; loading a bundled copy first by full path lets the import
    // resolve even though the app folder isn't on the loader's search path.
    for path in native_libraries::candidates("sharpyuv").into_iter().filter(|path| path.is_absolute()) {
        if let Ok(library) = unsafe { libloading::Library::new(&path) } {
            libraries.push(library);
            break;
        }
    }
    let mut candidates = native_libraries::candidates("webp");
    // A Linux distribution's runtime package only installs the versioned name.
    if cfg!(target_os = "linux") {
        candidates.push(PathBuf::from("libwebp.so.7"));
    }
    for candidate in candidates {
        let Ok(library) = (unsafe { libloading::Library::new(&candidate) }) else { continue };
        let functions = unsafe {
            let decode = library.get::<DecodeRgba>(b"WebPDecodeRGBA\0").map(|symbol| *symbol);
            let free = library.get::<Free>(b"WebPFree\0").map(|symbol| *symbol);
            decode.ok().zip(free.ok())
        };
        if let Some((decode_rgba, free)) = functions {
            libraries.push(library);
            return Some(WebpLibrary { decode_rgba, free, _libraries: libraries });
        }
    }
    None
}

fn library() -> Option<&'static WebpLibrary> {
    static LIBRARY: OnceLock<Option<WebpLibrary>> = OnceLock::new();
    LIBRARY.get_or_init(load_library).as_ref()
}

pub fn is_available() -> bool {
    library().is_some()
}

/// Whether bytes are a WebP file (`RIFF....WEBP`).
pub fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

/// The original's message on Windows; elsewhere it names the platform's library file.
fn missing_library_message() -> String {
    let file_name = if cfg!(target_os = "windows") {
        "libwebp.dll".to_string()
    } else {
        native_libraries::display_file_name("webp")
    };
    format!("WebP images need {file_name} next to the app")
}

/// Decodes WebP bytes to RGBA, with the original's error messages.
pub fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
    let library = library().ok_or_else(missing_library_message)?;
    let (mut width, mut height) = (0i32, 0i32);
    let pixels = unsafe { (library.decode_rgba)(bytes.as_ptr(), bytes.len(), &mut width, &mut height) };
    if pixels.is_null() {
        return Err("invalid WebP image".to_string());
    }
    let length = width as usize * height as usize * 4;
    let copied = unsafe { std::slice::from_raw_parts(pixels, length) }.to_vec();
    unsafe { (library.free)(pixels.cast()) };
    Ok(RgbaImage { width: width as u32, height: height as u32, pixels: copied })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_library_message_names_this_platforms_file() {
        if cfg!(target_os = "windows") {
            assert_eq!(missing_library_message(), "WebP images need libwebp.dll next to the app");
        } else {
            assert!(missing_library_message().contains("libwebp"));
        }
    }

    #[test]
    fn decodes_webp_with_the_bundled_library() {
        // A 1x1 lossless WebP.
        let tiny: [u8; 34] = [
            0x52, 0x49, 0x46, 0x46, 0x1a, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50, 0x56, 0x50, 0x38, 0x4c, 0x0d, 0x00,
            0x00, 0x00, 0x2f, 0x00, 0x00, 0x00, 0x10, 0x07, 0x10, 0x11, 0x11, 0x88, 0x88, 0xfe, 0x07, 0x00,
        ];
        assert!(is_webp(&tiny));
        if !is_available() {
            eprintln!("libwebp not found; skipping");
            return;
        }
        let image = decode(&tiny).unwrap();
        assert_eq!((image.width, image.height), (1, 1));
        assert!(decode(b"RIFF\0\0\0\0WEBPjunk").is_err());
    }
}
