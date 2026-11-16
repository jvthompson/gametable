//! Decodes image files through SDL_image and prints their sizes (format support check).
//! `cargo run -p gametable --example decode_check -- <image>...`

fn main() {
    for path in std::env::args().skip(1) {
        match gametable::graphics::image_data::RgbaImage::load(std::path::Path::new(&path)) {
            Ok(image) => println!("{path}: {}x{}", image.width, image.height),
            Err(error) => println!("{path}: error {error}"),
        }
    }
}
