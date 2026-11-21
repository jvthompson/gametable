//! Copying a folder of card art into a game for the Sets tab's "Add Set...", optionally shrinking
//! each image to a maximum height and converting it to lossy WebP on the way in (a GameTable
//! addition; the original only copied the files and turned landscape art portrait).
//!
//! The source folder is never changed, except when it already is the set's folder inside the game,
//! in which case its images are processed where they are. An image that can't be processed is
//! copied as it was, so a set always gets every card.

use std::collections::HashMap;
use std::path::Path;

use crate::data::card_art_files::{WEBP_QUALITY, with_webp_extension};
use crate::data::file_system::{is_same_path, join_path, list_folder, lowercase_extension};
use crate::data::gamedef_ops::is_image_file;
use crate::data::image_utils::{replace_image_file, rotate_landscape_image};
use crate::graphics::image_data::RgbaImage;
use crate::graphics::webp;

/// Formats that can be decoded, and so shrunk or converted.
const DECODABLE_EXTENSIONS: [&str; 6] = [".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp"];

/// What to do to each image as it is copied in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SetImportOptions {
    /// Scale taller images down to this height, keeping each one's aspect ratio.
    pub max_height: Option<u32>,
    /// Write lossy WebP files in place of the originals.
    pub convert_to_webp: bool,
}

impl SetImportOptions {
    fn changes_images(&self) -> bool {
        self.max_height.is_some() || self.convert_to_webp
    }
}

/// One image file to bring into the set's folder.
#[derive(Debug, Clone, PartialEq)]
pub struct SetImageToImport {
    pub source_folder: String,
    pub destination_folder: String,
    pub file_name: String,
    pub options: SetImportOptions,
}

/// What importing one image did.
#[derive(Debug, Clone, PartialEq)]
pub enum SetImageImport {
    /// In the set's folder under this name (a `.webp` name when it was converted).
    Imported { file_name: String },
    /// Shrinking or converting failed, so the image was copied as it was.
    CopiedUnchanged { file_name: String, error: String },
    /// Not even a plain copy worked; the set gets no card for this image.
    Failed(String),
}

/// Every image file in `source_folder`, each to be copied into `destination_folder`.
pub fn images_to_import(
    source_folder: &str,
    destination_folder: &str,
    options: SetImportOptions,
) -> Vec<SetImageToImport> {
    let Some(entries) = list_folder(Path::new(source_folder)) else { return Vec::new() };
    entries
        .into_iter()
        .filter(|entry| !entry.is_folder && is_image_file(&entry.name))
        .map(|entry| SetImageToImport {
            source_folder: source_folder.to_string(),
            destination_folder: destination_folder.to_string(),
            file_name: entry.name,
            options,
        })
        .collect()
}

/// Copies one image into the set's folder, processing it as its options ask.
pub fn import_set_image(image: &SetImageToImport) -> SetImageImport {
    if image.options.changes_images() {
        match write_processed_image(image) {
            Ok(Some(file_name)) => return SetImageImport::Imported { file_name },
            // Nothing to change: a plain copy does.
            Ok(None) => {}
            Err(error) => {
                return match copy_image(image) {
                    Ok(file_name) => SetImageImport::CopiedUnchanged { file_name, error },
                    Err(copy_error) => SetImageImport::Failed(copy_error),
                };
            }
        }
    }
    match copy_image(image) {
        Ok(file_name) => SetImageImport::Imported { file_name },
        Err(error) => SetImageImport::Failed(error),
    }
}

/// The original behaviour: copy the file, then turn it portrait if it is landscape.
fn copy_image(image: &SetImageToImport) -> Result<String, String> {
    let source_path = join_path(&[&image.source_folder, &image.file_name]);
    let destination_path = join_path(&[&image.destination_folder, &image.file_name]);
    if !is_same_path(&source_path, &destination_path) {
        std::fs::create_dir_all(&image.destination_folder).map_err(|error| error.to_string())?;
        std::fs::copy(&source_path, &destination_path).map_err(|error| format!("{}: {error}", image.file_name))?;
    }
    let _ = rotate_landscape_image(&destination_path);
    Ok(image.file_name.clone())
}

/// Decodes the image, turns it portrait, shrinks it and writes it in its new form. Returns the
/// written file's name, or `None` when nothing about the image needs to change.
fn write_processed_image(image: &SetImageToImport) -> Result<Option<String>, String> {
    let extension = lowercase_extension(&image.file_name);
    if !DECODABLE_EXTENSIONS.contains(&extension.as_str()) {
        return Ok(None);
    }
    let source_path = join_path(&[&image.source_folder, &image.file_name]);
    let bytes = std::fs::read(&source_path).map_err(|error| format!("{}: {error}", image.file_name))?;
    let mut pixels = RgbaImage::decode(&bytes).map_err(|error| format!("{}: {error}", image.file_name))?;
    let mut pixels_changed = false;
    // Only PNG and JPEG art was ever turned portrait on import.
    if matches!(extension.as_str(), ".png" | ".jpg" | ".jpeg") && pixels.width > pixels.height {
        pixels = pixels.rotated_clockwise();
        pixels_changed = true;
    }
    if let Some(max_height) = image.options.max_height
        && pixels.height > max_height
    {
        pixels = pixels.downscaled_to_max_height(max_height);
        pixels_changed = true;
    }
    let is_webp = extension == ".webp";
    std::fs::create_dir_all(&image.destination_folder).map_err(|error| error.to_string())?;
    let original_destination_path = join_path(&[&image.destination_folder, &image.file_name]);
    if (image.options.convert_to_webp && !is_webp) || (is_webp && pixels_changed) {
        let webp_name = with_webp_extension(&image.file_name);
        let webp_path = join_path(&[&image.destination_folder, &webp_name]);
        if !is_webp && Path::new(&webp_path).exists() {
            return Err(format!("{webp_name} already exists"));
        }
        let encoded = webp::encode_lossy(&pixels, WEBP_QUALITY).map_err(|error| format!("{webp_name}: {error}"))?;
        write_file_through_temporary(&webp_path, &encoded)?;
        // Processing the set's own folder in place: the WebP file replaces the original.
        if !is_webp && is_same_path(&source_path, &original_destination_path) {
            let _ = std::fs::remove_file(&source_path);
        }
        return Ok(Some(webp_name));
    }
    if !pixels_changed {
        return Ok(None);
    }
    // Shrunk but kept in its own format, which can only be written for PNG and JPEG.
    if !matches!(extension.as_str(), ".png" | ".jpg" | ".jpeg") {
        return Ok(None);
    }
    replace_image_file(&original_destination_path, &pixels)?;
    Ok(Some(image.file_name.clone()))
}

fn write_file_through_temporary(path: &str, bytes: &[u8]) -> Result<(), String> {
    let temporary_path = format!("{path}.gametable-tmp");
    let written = std::fs::write(&temporary_path, bytes).and_then(|()| std::fs::rename(&temporary_path, path));
    written.map_err(|error| {
        let _ = std::fs::remove_file(&temporary_path);
        format!("{path}: {error}")
    })
}

/// Source file name -> the name it was imported under, for every image that made it in.
pub fn imported_file_names(results: &[(SetImageToImport, SetImageImport)]) -> HashMap<String, String> {
    results
        .iter()
        .filter_map(|(image, outcome)| match outcome {
            SetImageImport::Imported { file_name } | SetImageImport::CopiedUnchanged { file_name, .. } => {
                Some((image.file_name.clone(), file_name.clone()))
            }
            SetImageImport::Failed(_) => None,
        })
        .collect()
}

/// A note for the user when some images weren't imported as asked; `None` when all were.
pub fn import_problems_message(results: &[(SetImageToImport, SetImageImport)]) -> Option<String> {
    let mut unchanged = 0;
    let mut failed = 0;
    let mut first_error = None;
    for (_, outcome) in results {
        match outcome {
            SetImageImport::Imported { .. } => {}
            SetImageImport::CopiedUnchanged { error, .. } => {
                unchanged += 1;
                first_error.get_or_insert_with(|| error.clone());
            }
            SetImageImport::Failed(error) => {
                failed += 1;
                first_error.get_or_insert_with(|| error.clone());
            }
        }
    }
    let plural = |count: usize| if count == 1 { "" } else { "s" };
    let mut parts = Vec::new();
    if unchanged > 0 {
        parts.push(format!("{unchanged} image{} copied unchanged", plural(unchanged)));
    }
    if failed > 0 {
        parts.push(format!("{failed} image{} could not be copied", plural(failed)));
    }
    let first_error = first_error?;
    Some(format!("{} ({first_error})", parts.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::image_dimensions::image_dimensions;

    fn test_folders(name: &str) -> (std::path::PathBuf, String, String) {
        let folder = std::env::temp_dir().join(format!("gametable_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join("source")).unwrap();
        let source = folder.join("source").to_string_lossy().into_owned();
        let destination = folder.join("game").join("set1").to_string_lossy().into_owned();
        (folder, source, destination)
    }

    fn image(width: u32, height: u32) -> RgbaImage {
        let mut pixels = Vec::new();
        for index in 0..width * height {
            pixels.extend_from_slice(&[(index % 251) as u8, 90, 200, 255]);
        }
        RgbaImage { width, height, pixels }
    }

    fn import_all(
        source: &str,
        destination: &str,
        options: SetImportOptions,
    ) -> Vec<(SetImageToImport, SetImageImport)> {
        let mut results: Vec<_> = images_to_import(source, destination, options)
            .into_iter()
            .map(|image| {
                let outcome = import_set_image(&image);
                (image, outcome)
            })
            .collect();
        results.sort_by(|first, second| first.0.file_name.cmp(&second.0.file_name));
        results
    }

    #[test]
    fn without_options_images_are_copied_and_turned_portrait() {
        let (folder, source, destination) = test_folders("set_import_plain");
        image(40, 60).save_png(Path::new(&join_path(&[&source, "tall.png"]))).unwrap();
        image(60, 40).save_png(Path::new(&join_path(&[&source, "wide.png"]))).unwrap();
        let results = import_all(&source, &destination, SetImportOptions::default());
        assert_eq!(imported_file_names(&results).get("tall.png").map(String::as_str), Some("tall.png"));
        assert_eq!(image_dimensions(Path::new(&join_path(&[&destination, "wide.png"]))), Some((40, 60)));
        assert_eq!(import_problems_message(&results), None);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn shrinking_keeps_the_format_and_leaves_the_source_alone() {
        let (folder, source, destination) = test_folders("set_import_shrink");
        image(60, 90).save_jpeg(Path::new(&join_path(&[&source, "big.jpg"])), 90).unwrap();
        image(20, 30).save_png(Path::new(&join_path(&[&source, "small.png"]))).unwrap();
        let options = SetImportOptions { max_height: Some(45), convert_to_webp: false };
        let results = import_all(&source, &destination, options);
        assert_eq!(image_dimensions(Path::new(&join_path(&[&destination, "big.jpg"]))), Some((30, 45)));
        assert_eq!(image_dimensions(Path::new(&join_path(&[&destination, "small.png"]))), Some((20, 30)));
        assert_eq!(image_dimensions(Path::new(&join_path(&[&source, "big.jpg"]))), Some((60, 90)));
        assert_eq!(import_problems_message(&results), None);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn converting_writes_webp_files_named_after_the_originals() {
        if !webp::is_available() {
            return;
        }
        let (folder, source, destination) = test_folders("set_import_webp");
        image(60, 90).save_png(Path::new(&join_path(&[&source, "dragon.png"]))).unwrap();
        image(60, 90).save_png(Path::new(&join_path(&[&source, "dragon_BACK.png"]))).unwrap();
        image(90, 60).save_jpeg(Path::new(&join_path(&[&source, "wide.jpg"])), 90).unwrap();
        let options = SetImportOptions { max_height: Some(45), convert_to_webp: true };
        let results = import_all(&source, &destination, options);
        let names = imported_file_names(&results);
        assert_eq!(names.get("dragon.png").map(String::as_str), Some("dragon.webp"));
        assert_eq!(names.get("dragon_BACK.png").map(String::as_str), Some("dragon_BACK.webp"));
        assert_eq!(image_dimensions(Path::new(&join_path(&[&destination, "dragon.webp"]))), Some((30, 45)));
        assert_eq!(image_dimensions(Path::new(&join_path(&[&destination, "wide.webp"]))), Some((30, 45)));
        assert!(!Path::new(&join_path(&[&destination, "dragon.png"])).exists());
        assert!(Path::new(&join_path(&[&source, "dragon.png"])).exists());
        assert_eq!(import_problems_message(&results), None);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn an_image_that_cannot_be_processed_is_copied_unchanged() {
        let (folder, source, destination) = test_folders("set_import_broken");
        std::fs::write(join_path(&[&source, "broken.png"]), b"not really a png").unwrap();
        let options = SetImportOptions { max_height: Some(45), convert_to_webp: false };
        let results = import_all(&source, &destination, options);
        assert!(
            matches!(&results[0].1, SetImageImport::CopiedUnchanged { file_name, .. } if file_name == "broken.png")
        );
        assert!(import_problems_message(&results).unwrap().starts_with("1 image copied unchanged"));
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
