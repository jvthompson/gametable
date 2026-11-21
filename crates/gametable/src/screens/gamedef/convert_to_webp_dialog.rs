//! "Convert to WebP" on the Cards tab (a GameTable addition): rewrites every card's art, and each
//! card's unique `_BACK` image, as a lossy WebP file (quality [`WEBP_QUALITY`]), points the cards
//! at the new files and deletes the originals.
//!
//! The flow is a confirmation, then a progress dialog while worker threads write the `.webp`
//! files beside the originals, then the editor applies the result ([`apply_webp_conversion`]):
//! the new image paths go into the cards and straight into `gamedef.json`, so the saved game
//! never points at a deleted file, and only then are the originals deleted. Other unsaved edits
//! stay unsaved. Last, a summary.

use std::collections::HashSet;

use gametable_core::models::GameDefinition;

use crate::app::Frame;
use crate::data::card_art_files::{
    CardArtGroup, GroupConversion, WEBP_QUALITY, card_art_groups, convert_group_to_webp, game_uses_image,
    point_cards_at_webp, with_webp_extension,
};
use crate::data::gamedef_ops::write_game_definition;
use crate::data::image_resolver::clear_existence_cache;
use crate::graphics::webp;
use crate::screens::gamedef::card_art_job::CardArtJob;
use crate::ui::overlays::ConfirmResponse;

type ConversionResult = Result<GroupConversion, String>;

fn convert_one_group(group: &CardArtGroup) -> ConversionResult {
    convert_group_to_webp(group, WEBP_QUALITY)
}

/// The finished background work, for the editor to apply.
#[derive(Debug, Default, PartialEq)]
pub struct ConvertedCardArt {
    /// Fronts that now have a `.webp` beside them (their unique backs too).
    pub converted_front_paths: HashSet<String>,
    /// The PNG/JPEG/... files the new `.webp` files replace.
    pub originals: Vec<String>,
    pub original_bytes: u64,
    pub webp_bytes: u64,
    pub unsupported_format: usize,
    pub failed: usize,
    pub first_error: Option<String>,
    /// `(done, total)` when cancelled part way.
    pub stopped_early: Option<(usize, usize)>,
}

impl ConvertedCardArt {
    fn from_results(results: &[(CardArtGroup, ConversionResult)], stopped_early: Option<(usize, usize)>) -> Self {
        let mut converted = ConvertedCardArt { stopped_early, ..ConvertedCardArt::default() };
        for (group, result) in results {
            match result {
                Ok(GroupConversion::Converted { originals, original_bytes, webp_bytes }) => {
                    converted.converted_front_paths.insert(group.front_path.clone());
                    converted.originals.extend(originals.iter().cloned());
                    converted.original_bytes += original_bytes;
                    converted.webp_bytes += webp_bytes;
                }
                Ok(GroupConversion::AlreadyWebp) => {}
                Ok(GroupConversion::UnsupportedFormat) => converted.unsupported_format += 1,
                Err(error) => {
                    converted.failed += 1;
                    converted.first_error.get_or_insert_with(|| error.clone());
                }
            }
        }
        converted
    }
}

/// What applying a conversion did, for the summary.
#[derive(Debug, Default, PartialEq)]
pub struct AppliedConversion {
    pub cards_updated: usize,
    pub originals_deleted: usize,
    /// Originals left because something in the game still shows them (a card back, say).
    pub originals_still_in_use: usize,
    /// Set when `gamedef.json` couldn't be written; then no original is deleted.
    pub save_error: Option<String>,
}

/// Points the cards of both the edited game and the last saved one at the new `.webp` files,
/// writes the saved one (with only that change) to `gamedef.json`, then deletes the originals
/// nothing uses any more. The caller keeps `saved_game` as the editor's saved copy, so unsaved
/// edits stay unsaved.
pub fn apply_webp_conversion(
    edited_game: &mut GameDefinition,
    saved_game: &mut GameDefinition,
    folder_path: &str,
    converted: &ConvertedCardArt,
) -> AppliedConversion {
    let mut applied = AppliedConversion {
        cards_updated: point_cards_at_webp(edited_game, folder_path, &converted.converted_front_paths),
        ..AppliedConversion::default()
    };
    let mut updated_saved_game = saved_game.clone();
    point_cards_at_webp(&mut updated_saved_game, folder_path, &converted.converted_front_paths);
    let saved = write_game_definition(folder_path, &updated_saved_game);
    clear_existence_cache();
    if let Err(error) = saved {
        applied.save_error = Some(error);
        return applied;
    }
    *saved_game = updated_saved_game;
    for original in &converted.originals {
        let still_used =
            game_uses_image(edited_game, folder_path, original) || game_uses_image(saved_game, folder_path, original);
        if still_used {
            applied.originals_still_in_use += 1;
        } else if std::fs::remove_file(original).is_ok() {
            applied.originals_deleted += 1;
        }
    }
    clear_existence_cache();
    applied
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

/// The closing summary.
pub fn summary_message(converted: &ConvertedCardArt, applied: &AppliedConversion) -> String {
    let plural = |count: usize| if count == 1 { "" } else { "s" };
    let converted_count = converted.originals.len();
    let mut lines = vec![format!(
        "Converted {converted_count} image{} and updated {} card{}.",
        plural(converted_count),
        applied.cards_updated,
        plural(applied.cards_updated)
    )];
    if converted_count > 0 {
        lines.push(format!("{} became {}.", megabytes(converted.original_bytes), megabytes(converted.webp_bytes)));
    }
    if let Some(error) = &applied.save_error {
        lines.push(format!(
            "The game definition could not be saved ({error}), so the original files were kept. Save the game to \
             use the new WebP files."
        ));
    } else if applied.originals_deleted > 0 {
        lines.push(format!(
            "Deleted {} original file{}.",
            applied.originals_deleted,
            plural(applied.originals_deleted)
        ));
    }
    if applied.originals_still_in_use > 0 {
        lines.push(format!(
            "Kept {} original file{} still used elsewhere in the game.",
            applied.originals_still_in_use,
            plural(applied.originals_still_in_use)
        ));
    }
    if converted.unsupported_format > 0 {
        lines.push(format!(
            "Skipped {} image{} in a format that can't be converted.",
            converted.unsupported_format,
            plural(converted.unsupported_format)
        ));
    }
    if converted.failed > 0 {
        lines.push(format!(
            "{} image{} failed and {} unchanged: {}",
            converted.failed,
            plural(converted.failed),
            if converted.failed == 1 { "was" } else { "were" },
            converted.first_error.as_deref().unwrap_or_default()
        ));
    }
    if let Some((done, total)) = converted.stopped_early {
        lines.push(format!("Stopped after {done} of {total} images."));
    }
    lines.join("\n")
}

enum Stage {
    Confirming {
        groups: Vec<CardArtGroup>,
    },
    Running {
        job: CardArtJob<CardArtGroup, ConversionResult>,
    },
    /// Waiting for the editor to take the result and apply it.
    ReadyToApply(ConvertedCardArt),
    Finished {
        message: String,
    },
}

pub struct ConvertToWebpFlow {
    stage: Stage,
}

impl ConvertToWebpFlow {
    pub fn open(folder_path: &str, game: &GameDefinition) -> ConvertToWebpFlow {
        if !webp::is_available() {
            let message = "Converting needs the WebP library (libwebp) beside GameTable, and it wasn't found.";
            return ConvertToWebpFlow { stage: Stage::Finished { message: message.to_string() } };
        }
        let groups: Vec<CardArtGroup> = card_art_groups(folder_path, game)
            .into_iter()
            .filter(|group| group.paths().any(|path| with_webp_extension(path) != *path))
            .collect();
        if groups.is_empty() {
            let message = "There is no card art to convert: every card image is already WebP.";
            return ConvertToWebpFlow { stage: Stage::Finished { message: message.to_string() } };
        }
        ConvertToWebpFlow { stage: Stage::Confirming { groups } }
    }

    /// Development hook (`GAMETABLE_GAMEDEF_TAB=cards,towebp,start`): starts converting as if
    /// Convert had been clicked.
    pub fn start_without_confirming_for_development(&mut self) {
        if let Stage::Confirming { groups } = &mut self.stage {
            let groups = std::mem::take(groups);
            self.stage = Stage::Running { job: CardArtJob::start(groups, "convert-to-webp", convert_one_group) };
        }
    }

    /// The finished conversion, once, for the editor to apply.
    pub fn take_conversion_to_apply(&mut self) -> Option<ConvertedCardArt> {
        if !matches!(self.stage, Stage::ReadyToApply(_)) {
            return None;
        }
        let placeholder = Stage::Finished { message: String::new() };
        match std::mem::replace(&mut self.stage, placeholder) {
            Stage::ReadyToApply(converted) => Some(converted),
            _ => None,
        }
    }

    /// Shows the summary after the editor has applied the conversion.
    pub fn show_summary(&mut self, message: String) {
        self.stage = Stage::Finished { message };
    }

    /// Draws the current dialog; false once the flow is over.
    pub fn show(&mut self, frame: &mut Frame) -> bool {
        match &mut self.stage {
            Stage::Confirming { groups } => {
                let image_count: usize = groups.iter().map(|group| group.paths().count()).sum();
                let message = format!(
                    "Convert {image_count} card image{} to WebP (lossy, quality {WEBP_QUALITY}). WebP files are \
                     usually several times smaller.\n\nThe cards are pointed at the new .webp files and that change \
                     is saved to the game definition straight away (other unsaved edits stay unsaved). The original \
                     image files are then deleted. This can't be undone.",
                    if image_count == 1 { "" } else { "s" }
                );
                match frame.confirm_dialog("towebp", "Convert to WebP", &message, "Convert", true) {
                    ConfirmResponse::Open => true,
                    ConfirmResponse::Cancelled => false,
                    ConfirmResponse::Confirmed => {
                        let groups = std::mem::take(groups);
                        let job = CardArtJob::start(groups, "convert-to-webp", convert_one_group);
                        self.stage = Stage::Running { job };
                        true
                    }
                }
            }
            Stage::Running { job } => {
                if job.poll() {
                    let stopped_early = job.stopped_early();
                    let converted = ConvertedCardArt::from_results(&job.take_results(), stopped_early);
                    self.stage = Stage::ReadyToApply(converted);
                    return true;
                }
                job.show_progress(frame, "towebpprogress", "Converting to WebP");
                true
            }
            // The editor applies it this frame and calls `show_summary`.
            Stage::ReadyToApply(_) => true,
            Stage::Finished { message } => !frame.alert_dialog("towebpdone", "Convert to WebP", message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::gamedef_ops::load_game_definition_file;
    use crate::graphics::image_data::RgbaImage;
    use gametable_core::models::{CardBackDefinition, CardDefinition};

    fn opaque_image() -> RgbaImage {
        RgbaImage { width: 12, height: 16, pixels: vec![200u8; 12 * 16 * 4] }
    }

    #[test]
    fn applying_saves_only_the_new_paths_and_deletes_unused_originals() {
        if !webp::is_available() {
            eprintln!("libwebp not found; skipping");
            return;
        }
        let folder = std::env::temp_dir().join(format!("gametable_apply_webp_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let folder_path = folder.to_string_lossy().into_owned();
        opaque_image().save_png(&folder.join("goblin.png")).unwrap();
        opaque_image().save_png(&folder.join("shared.png")).unwrap();

        let mut saved_game = GameDefinition::default();
        saved_game.id = "test".to_string();
        saved_game.name = "Test".to_string();
        saved_game.cards_mut().extend([
            CardDefinition { image_path: Some("goblin.png".to_string()), ..CardDefinition::new("goblin", "Goblin") },
            CardDefinition { image_path: Some("shared.png".to_string()), ..CardDefinition::new("shared", "Shared") },
        ]);
        // The shared image is also the game's card back, so its original must stay.
        saved_game.card_backs.push(CardBackDefinition {
            id: "back".to_string(),
            image_path: Some("shared.png".to_string()),
            ..Default::default()
        });
        write_game_definition(&folder_path, &saved_game).unwrap();
        // An unsaved edit that must not reach the file.
        let mut edited_game = saved_game.clone();
        edited_game.cards_mut()[0].card_title = "Unsaved title".to_string();

        let groups = card_art_groups(&folder_path, &edited_game);
        let results: Vec<_> = groups.iter().map(|group| (group.clone(), convert_one_group(group))).collect();
        let converted = ConvertedCardArt::from_results(&results, None);
        assert_eq!(converted.originals.len(), 2);

        let applied = apply_webp_conversion(&mut edited_game, &mut saved_game, &folder_path, &converted);
        assert_eq!(
            applied,
            AppliedConversion { cards_updated: 2, originals_deleted: 1, originals_still_in_use: 1, save_error: None }
        );
        assert!(!folder.join("goblin.png").exists());
        assert!(folder.join("goblin.webp").is_file());
        assert!(folder.join("shared.png").is_file());

        let json_path = crate::data::gamedef_ops::find_game_definition_file(&folder_path).unwrap();
        let reloaded = load_game_definition_file(&json_path, &folder_path).unwrap();
        let goblin = &reloaded.cards()[0];
        assert_eq!(goblin.card_title, "Goblin");
        assert!(goblin.image_path.as_deref().unwrap().ends_with("goblin.webp"));
        assert_eq!(edited_game.cards()[0].image_path.as_deref(), Some("goblin.webp"));
        assert_eq!(saved_game.cards()[0].image_path.as_deref(), Some("goblin.webp"));
        assert_eq!(edited_game.cards()[0].card_title, "Unsaved title");
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn summary_reports_sizes_failures_and_a_failed_save() {
        let converted = ConvertedCardArt {
            originals: vec!["a.png".to_string(), "b.png".to_string()],
            original_bytes: 4 * 1024 * 1024,
            webp_bytes: 1024 * 1024,
            failed: 1,
            first_error: Some("c.png: bad data".to_string()),
            ..ConvertedCardArt::default()
        };
        let applied = AppliedConversion { cards_updated: 3, originals_deleted: 2, ..AppliedConversion::default() };
        let message = summary_message(&converted, &applied);
        assert!(message.starts_with("Converted 2 images and updated 3 cards."));
        assert!(message.contains("4.0 MB became 1.0 MB."));
        assert!(message.contains("Deleted 2 original files."));
        assert!(message.contains("1 image failed and was unchanged: c.png: bad data"));

        let failed_save = AppliedConversion { save_error: Some("disk full".to_string()), ..applied };
        assert!(summary_message(&converted, &failed_save).contains("original files were kept"));
    }
}
