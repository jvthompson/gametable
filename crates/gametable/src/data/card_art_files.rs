//! The image files a game's cards show, and the Card View's "Convert to WebP" (a GameTable
//! addition): rewriting card art as lossy WebP files, pointing the cards at them and deleting the
//! originals.
//!
//! A card's unique back is found by adding `_BACK` to its front image's name, extension and all
//! (`goblin.png` -> `goblin_BACK.png`), so a front and its unique back are converted together
//! ([`CardArtGroup`]): both become `.webp`, or neither does.

use std::collections::HashSet;
use std::path::Path;

use gametable_core::models::{GameDefinition, UNIQUE_CARD_BACK_ID};

use crate::data::file_system::lowercase_extension;
use crate::data::image_resolver::{resolve_bare_image_path, unique_card_back_path_for};
use crate::graphics::image_data::RgbaImage;
use crate::graphics::webp;

/// Lossy WebP quality (0-100) for converted card art.
pub const WEBP_QUALITY: f32 = 90.0;

/// Formats that can be decoded and so converted.
const CONVERTIBLE_EXTENSIONS: [&str; 5] = [".png", ".jpg", ".jpeg", ".gif", ".bmp"];

/// A card front image and, when a card using it has a unique back that exists, that back.
#[derive(Debug, Clone, PartialEq)]
pub struct CardArtGroup {
    pub front_path: String,
    pub unique_back_path: Option<String>,
}

impl CardArtGroup {
    /// The back first, then the front.
    pub fn paths(&self) -> impl Iterator<Item = &String> {
        self.unique_back_path.iter().chain(std::iter::once(&self.front_path))
    }
}

/// Every card front image (resolved to a full path) once, with its unique back where one exists.
pub fn card_art_groups(folder_path: &str, game: &GameDefinition) -> Vec<CardArtGroup> {
    let mut groups: Vec<CardArtGroup> = Vec::new();
    for card in game.cards() {
        let Some(bare_image_path) = &card.image_path else { continue };
        let front_path = resolve_bare_image_path(folder_path, bare_image_path, card.set_id.as_deref());
        let unique_back_path = (card.card_back_id.as_deref() == Some(UNIQUE_CARD_BACK_ID))
            .then(|| unique_card_back_path_for(&front_path))
            .filter(|back_path| Path::new(back_path).is_file());
        match groups.iter_mut().find(|group| group.front_path == front_path) {
            Some(group) => {
                if group.unique_back_path.is_none() {
                    group.unique_back_path = unique_back_path;
                }
            }
            None => groups.push(CardArtGroup { front_path, unique_back_path }),
        }
    }
    groups
}

fn is_webp_path(path: &str) -> bool {
    lowercase_extension(path) == ".webp"
}

/// `art/goblin.png` -> `art/goblin.webp`; a name without an extension gets `.webp` added.
pub fn with_webp_extension(path: &str) -> String {
    let last_separator = path.rfind(['/', '\\']);
    match path.rfind('.') {
        Some(dot) if dot > 0 && last_separator.is_none_or(|separator| dot > separator + 1) => {
            format!("{}.webp", &path[..dot])
        }
        _ => format!("{path}.webp"),
    }
}

/// What converting one [`CardArtGroup`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum GroupConversion {
    /// New `.webp` files were written beside the originals, which are listed for deletion once the
    /// cards point at the new files.
    Converted { originals: Vec<String>, original_bytes: u64, webp_bytes: u64 },
    /// Every image in the group is already WebP.
    AlreadyWebp,
    /// An image isn't in a format that can be converted; nothing was written.
    UnsupportedFormat,
}

/// Writes a `.webp` copy of each image in the group that isn't WebP yet. If any image fails, the
/// copies already written for the group are removed again, so a front and its unique back never
/// end up in different formats.
pub fn convert_group_to_webp(group: &CardArtGroup, quality: f32) -> Result<GroupConversion, String> {
    let originals: Vec<String> = group.paths().filter(|path| !is_webp_path(path)).cloned().collect();
    if originals.is_empty() {
        return Ok(GroupConversion::AlreadyWebp);
    }
    if originals.iter().any(|path| !CONVERTIBLE_EXTENSIONS.contains(&lowercase_extension(path).as_str())) {
        return Ok(GroupConversion::UnsupportedFormat);
    }
    let mut written_webp_paths = Vec::new();
    let mut original_bytes = 0;
    let mut webp_bytes = 0;
    for original_path in &originals {
        match convert_file_to_webp(original_path, quality) {
            Ok((webp_path, sizes)) => {
                written_webp_paths.push(webp_path);
                original_bytes += sizes.0;
                webp_bytes += sizes.1;
            }
            Err(error) => {
                for webp_path in &written_webp_paths {
                    let _ = std::fs::remove_file(webp_path);
                }
                return Err(error);
            }
        }
    }
    Ok(GroupConversion::Converted { originals, original_bytes, webp_bytes })
}

/// Writes `<name>.webp` beside `path`; returns its path and the (original, WebP) sizes in bytes.
/// Refuses to replace an existing `.webp` file.
fn convert_file_to_webp(path: &str, quality: f32) -> Result<(String, (u64, u64)), String> {
    let webp_path = with_webp_extension(path);
    if Path::new(&webp_path).exists() {
        return Err(format!("{webp_path} already exists"));
    }
    let original_bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
    let image = RgbaImage::decode(&original_bytes).map_err(|error| format!("{path}: {error}"))?;
    let webp_file = webp::encode_lossy(&image, quality).map_err(|error| format!("{path}: {error}"))?;
    let temporary_path = format!("{webp_path}.gametable-tmp");
    let written =
        std::fs::write(&temporary_path, &webp_file).and_then(|()| std::fs::rename(&temporary_path, &webp_path));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(format!("{webp_path}: {error}"));
    }
    Ok((webp_path, (original_bytes.len() as u64, webp_file.len() as u64)))
}

/// Points every card whose front image is in `converted_front_paths` at the `.webp` file beside
/// it. Returns how many cards changed.
pub fn point_cards_at_webp(
    game: &mut GameDefinition,
    folder_path: &str,
    converted_front_paths: &HashSet<String>,
) -> usize {
    let mut changed_cards = 0;
    for card in game.cards_mut() {
        let Some(bare_image_path) = &card.image_path else { continue };
        let front_path = resolve_bare_image_path(folder_path, bare_image_path, card.set_id.as_deref());
        if converted_front_paths.contains(&front_path) && !is_webp_path(bare_image_path) {
            card.image_path = Some(with_webp_extension(bare_image_path));
            changed_cards += 1;
        }
    }
    changed_cards
}

/// Every image file the game shows (full paths): card fronts, cards' unique backs and card back
/// definitions. Built once so checking many files against it stays quick on big games.
pub fn images_used_by_game(game: &GameDefinition, folder_path: &str) -> HashSet<String> {
    let mut used = HashSet::new();
    for card in game.cards() {
        let Some(bare_image_path) = &card.image_path else { continue };
        let front_path = resolve_bare_image_path(folder_path, bare_image_path, card.set_id.as_deref());
        if card.card_back_id.as_deref() == Some(UNIQUE_CARD_BACK_ID) {
            used.insert(unique_card_back_path_for(&front_path));
        }
        used.insert(front_path);
    }
    for back in &game.card_backs {
        if let Some(bare_image_path) = &back.image_path {
            used.insert(resolve_bare_image_path(folder_path, bare_image_path, None));
        }
    }
    used
}

#[cfg(test)]
mod tests {
    use super::*;
    use gametable_core::models::CardDefinition;

    fn card(title: &str, image: Option<&str>, set: Option<&str>, back: Option<&str>) -> CardDefinition {
        CardDefinition {
            image_path: image.map(str::to_string),
            set_id: set.map(str::to_string),
            card_back_id: back.map(str::to_string),
            ..CardDefinition::new(format!("id-{title}"), title)
        }
    }

    fn test_folder(name: &str) -> (std::path::PathBuf, String) {
        let folder = std::env::temp_dir().join(format!("gametable_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join("set1")).unwrap();
        let folder_path = folder.to_string_lossy().into_owned();
        (folder, folder_path)
    }

    fn opaque_image(width: u32, height: u32) -> RgbaImage {
        let mut pixels = Vec::new();
        for index in 0..width * height {
            pixels.extend_from_slice(&[(index % 251) as u8, 120, 40, 255]);
        }
        RgbaImage { width, height, pixels }
    }

    #[test]
    fn webp_extension_replaces_only_the_file_name_extension() {
        assert_eq!(with_webp_extension("goblin.png"), "goblin.webp");
        assert_eq!(with_webp_extension("art.v2/goblin.JPG"), "art.v2/goblin.webp");
        assert_eq!(with_webp_extension("art.v2/goblin"), "art.v2/goblin.webp");
        assert_eq!(with_webp_extension("set\\.hidden"), "set\\.hidden.webp");
    }

    #[test]
    fn groups_pair_fronts_with_existing_unique_backs() {
        let (folder, folder_path) = test_folder("art_groups");
        std::fs::write(folder.join("set1").join("dragon_BACK.png"), b"x").unwrap();
        let mut game = GameDefinition::default();
        game.cards_mut().extend([
            card("Dragon", Some("dragon.png"), Some("set1"), None),
            card("Dragon unique", Some("dragon.png"), Some("set1"), Some(UNIQUE_CARD_BACK_ID)),
            card("Goblin", Some("goblin.jpg"), None, Some(UNIQUE_CARD_BACK_ID)),
            card("Blank", None, None, None),
        ]);
        let groups = card_art_groups(&folder_path, &game);
        assert_eq!(
            groups,
            vec![
                CardArtGroup {
                    front_path: resolve_bare_image_path(&folder_path, "dragon.png", Some("set1")),
                    unique_back_path: Some(resolve_bare_image_path(&folder_path, "dragon_BACK.png", Some("set1"))),
                },
                CardArtGroup {
                    front_path: resolve_bare_image_path(&folder_path, "goblin.jpg", None),
                    unique_back_path: None,
                },
            ]
        );
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn converting_writes_webp_points_cards_at_it_and_leaves_originals_for_deletion() {
        if !webp::is_available() {
            eprintln!("libwebp not found; skipping");
            return;
        }
        let (folder, folder_path) = test_folder("to_webp");
        let set_folder = folder.join("set1");
        opaque_image(30, 42).save_png(&set_folder.join("dragon.png")).unwrap();
        // The unique back is a PNG named after the front.
        opaque_image(30, 42).save_png(&set_folder.join("dragon_BACK.png")).unwrap();
        let mut game = GameDefinition::default();
        game.cards_mut().extend([
            card("Dragon", Some("dragon.png"), Some("set1"), Some(UNIQUE_CARD_BACK_ID)),
            card("Dragon copy", Some("dragon.png"), Some("set1"), None),
        ]);
        let groups = card_art_groups(&folder_path, &game);
        assert_eq!(groups.len(), 1);

        let conversion = convert_group_to_webp(&groups[0], WEBP_QUALITY).unwrap();
        let GroupConversion::Converted { originals, original_bytes, webp_bytes } = conversion else {
            panic!("expected a conversion, got {conversion:?}");
        };
        assert_eq!(originals, vec![groups[0].unique_back_path.clone().unwrap(), groups[0].front_path.clone()]);
        assert!(original_bytes > 0 && webp_bytes > 0);
        let decoded = RgbaImage::load(&set_folder.join("dragon.webp")).unwrap();
        assert_eq!((decoded.width, decoded.height), (30, 42));
        assert!(set_folder.join("dragon_BACK.webp").is_file());

        let converted_fronts: HashSet<String> = [groups[0].front_path.clone()].into();
        assert_eq!(point_cards_at_webp(&mut game, &folder_path, &converted_fronts), 2);
        assert_eq!(game.cards()[0].image_path.as_deref(), Some("dragon.webp"));
        // The unique back now resolves to the converted back, and the originals are unused.
        assert_eq!(card_art_groups(&folder_path, &game)[0].unique_back_path, Some(with_webp_extension(&originals[0])));
        let used = images_used_by_game(&game, &folder_path);
        assert!(originals.iter().all(|original| !used.contains(original)));
        assert!(used.contains(&with_webp_extension(&originals[1])));

        // Converting again finds nothing to do.
        let groups_after = card_art_groups(&folder_path, &game);
        assert_eq!(convert_group_to_webp(&groups_after[0], WEBP_QUALITY), Ok(GroupConversion::AlreadyWebp));
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_failed_back_removes_the_fronts_new_webp_and_existing_webp_files_are_not_replaced() {
        if !webp::is_available() {
            eprintln!("libwebp not found; skipping");
            return;
        }
        let (folder, folder_path) = test_folder("to_webp_failure");
        let front = folder.join("goblin.png");
        // The back converts first and works; the front isn't really an image.
        opaque_image(10, 14).save_png(&folder.join("goblin_BACK.png")).unwrap();
        std::fs::write(&front, b"not an image").unwrap();
        let group = CardArtGroup {
            front_path: front.to_string_lossy().into_owned(),
            unique_back_path: Some(folder.join("goblin_BACK.png").to_string_lossy().into_owned()),
        };
        assert!(convert_group_to_webp(&group, WEBP_QUALITY).is_err());
        assert!(!folder.join("goblin.webp").exists());
        assert!(!folder.join("goblin_BACK.webp").exists());

        opaque_image(10, 14).save_png(&front).unwrap();
        let front_only = CardArtGroup { front_path: group.front_path.clone(), unique_back_path: None };
        std::fs::write(folder.join("goblin.webp"), b"someone else's file").unwrap();
        let error = convert_group_to_webp(&front_only, WEBP_QUALITY).unwrap_err();
        assert!(error.contains("already exists"));
        assert_eq!(std::fs::read(folder.join("goblin.webp")).unwrap(), b"someone else's file");

        let unsupported = CardArtGroup { front_path: format!("{folder_path}/scan.tiff"), unique_back_path: None };
        assert_eq!(convert_group_to_webp(&unsupported, WEBP_QUALITY), Ok(GroupConversion::UnsupportedFormat));
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
