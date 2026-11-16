//! WebP decoding through `libwebp`, loaded at runtime like the original (`image_utils.lua`): the
//! app works without it and reports WebP images as needing `libwebp.dll` next to the app.
//! SDL_image's bundled build has no WebP support.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::image_data::RgbaImage;

type DecodeRgba = unsafe extern "C" fn(*const u8, usize, *mut i32, *mut i32) -> *mut u8;
type Free = unsafe extern "C" fn(*mut std::ffi::c_void);

struct WebpLibrary {
    decode_rgba: DecodeRgba,
    free: Free,
    /// Kept loaded for the functions above (and `libsharpyuv`, which `libwebp` imports).
    _libraries: Vec<libloading::Library>,
}

/// Folders searched for the DLLs: the executable's folder, and in development the workspace's
/// `runtime/windows`.
fn search_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if let Some(folder) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        folders.push(folder);
    }
    if cfg!(debug_assertions) {
        folders.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/windows"));
    }
    folders
}

fn is_library_named(file_name: &str, prefix: &str) -> bool {
    let lowercase = file_name.to_lowercase();
    let Some(rest) = lowercase.strip_prefix(prefix) else { return false };
    let rest = rest.strip_prefix('-').unwrap_or(rest);
    let digits_end = rest.find(|character: char| !character.is_ascii_digit()).unwrap_or(rest.len());
    matches!(&rest[digits_end..], ".dll" | ".so" | ".dylib")
}

fn load_library() -> Option<WebpLibrary> {
    let mut libraries = Vec::new();
    let mut candidates: Vec<PathBuf> = Vec::new();
    for folder in search_folders() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            // libwebp imports libsharpyuv; loading it first by full path lets the import resolve
            // even though the app folder isn't on the DLL search path.
            if name.to_lowercase().starts_with("libsharpyuv") && is_library_named(&name, "libsharpyuv") {
                if let Ok(library) = unsafe { libloading::Library::new(entry.path()) } {
                    libraries.push(library);
                }
            } else if is_library_named(&name, "libwebp") {
                candidates.push(entry.path());
            }
        }
    }
    candidates.push(PathBuf::from(libloading::library_filename("libwebp")));
    candidates.push(PathBuf::from(libloading::library_filename("webp")));
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

/// Decodes WebP bytes to RGBA, with the original's error messages.
pub fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
    let library = library().ok_or("WebP images need libwebp.dll next to the app")?;
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
    fn library_names() {
        assert!(is_library_named("libwebp-7.dll", "libwebp"));
        assert!(is_library_named("LIBWEBP.DLL", "libwebp"));
        assert!(is_library_named("libwebp.so", "libwebp"));
        assert!(!is_library_named("libwebpdemux-2.dll", "libwebp"));
        assert!(is_library_named("libsharpyuv-0.dll", "libsharpyuv"));
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
