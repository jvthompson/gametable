//! The Tools menu's duplicate-card dialogs. Port of `src/ui/screens/gamedef/dialogs.lua`
//! (`duplicate_card_ids_dialog.dart`, `duplicate_card_images_dialog.dart`).

use std::collections::HashMap;

use gametable_core::models::GameDefinition;

use crate::app::Frame;
use crate::data::gamedef_ops::{
    CardGroup, duplicate_id_groups, duplicate_image_groups, merge_duplicate_image_cards, union_types,
};
use crate::screens::gamedef::common::StableKeys;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{DialogAction, DialogResponse, DialogSpec};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::Weight;

fn set_names(game: &GameDefinition) -> HashMap<String, String> {
    game.sets.iter().map(|set| (set.id.clone(), set.name.clone())).collect()
}

/// One of the Tools dialogs while open.
pub enum ToolDialog {
    /// Re-key every card whose id collides with another's. The groups are fixed when it opens
    /// so a row doesn't vanish as soon as its id becomes unique; edits stay here until Save.
    DuplicateIds { groups: Vec<CardGroup>, ids: HashMap<usize, String> },
    /// Merge each same-set, same-image group into its first card.
    MergeImages { groups: Vec<CardGroup> },
}

impl ToolDialog {
    /// `D.showDuplicateCardIds`: a toast instead when there's nothing to fix.
    pub fn duplicate_ids(frame: &mut Frame, game: &GameDefinition) -> Option<ToolDialog> {
        let groups = duplicate_id_groups(game.cards());
        if groups.is_empty() {
            frame.ui.toast("No duplicate card IDs found.", None);
            return None;
        }
        let ids = groups
            .iter()
            .flat_map(|group| group.indexes.iter().map(|&index| (index, game.cards()[index].id.clone())))
            .collect();
        Some(ToolDialog::DuplicateIds { groups, ids })
    }

    /// `D.showMergeDuplicateImages`.
    pub fn merge_images(frame: &mut Frame, game: &GameDefinition) -> Option<ToolDialog> {
        let groups = duplicate_image_groups(game.cards());
        if groups.is_empty() {
            frame.ui.toast("No duplicate card images found.", None);
            return None;
        }
        Some(ToolDialog::MergeImages { groups })
    }

    /// Shows the dialog; returns false once it has closed.
    pub fn show(&mut self, frame: &mut Frame, game: &mut GameDefinition, card_keys: &mut StableKeys) -> bool {
        let names = set_names(game);
        let palette = frame.palette();
        match self {
            ToolDialog::DuplicateIds { groups, ids } => {
                let actions = [DialogAction::text("Cancel"), DialogAction::filled("Save")];
                let spec = DialogSpec {
                    title: Some("Duplicate Card IDs"),
                    width: 480.0 + 48.0,
                    actions: &actions,
                    ..DialogSpec::default()
                };
                let cards = game.cards();
                let response = frame.dialog("dupids", &spec, |frame, x, y, width| {
                    let mut row_y = y;
                    for (group_number, group) in groups.iter().enumerate() {
                        let heading = format!("\"{}\" -- {} cards", group.key, group.indexes.len());
                        frame.text(&heading, x, row_y, TextOptions::new().bold().width(width).ellipsis());
                        row_y += 20.0 + 8.0;
                        for &index in &group.indexes {
                            let card = &cards[index];
                            let left_width = (width - 8.0) * 2.0 / 5.0;
                            frame.text(
                                &card.card_title,
                                x,
                                row_y + 4.0,
                                TextOptions::new().width(left_width).ellipsis(),
                            );
                            let set_name = card
                                .set_id
                                .as_ref()
                                .and_then(|set_id| names.get(set_id))
                                .map_or("(no set)", String::as_str);
                            let options = TextOptions::new()
                                .width(left_width)
                                .size(12.0)
                                .ellipsis()
                                .color(palette.on_surface_variant);
                            frame.text(set_name, x, row_y + 24.0, options);
                            let id_text = ids.entry(index).or_default();
                            let field_options = TextFieldOptions {
                                height: Some(44.0),
                                label_background: Some(palette.surface_container_high),
                                ..TextFieldOptions::label("Card ID")
                            };
                            frame.text_field(
                                &format!("dupids:{}:{}", group_number + 1, index + 1),
                                x + left_width + 8.0,
                                row_y,
                                width - left_width - 8.0,
                                id_text,
                                &field_options,
                            );
                            row_y += 44.0 + 8.0;
                        }
                        row_y += 12.0;
                    }
                    row_y - y
                });
                match response {
                    DialogResponse::Open | DialogResponse::Enter => true,
                    DialogResponse::Action(1) => {
                        let cards = game.cards_mut();
                        for (&index, id) in ids.iter() {
                            cards[index].id = id.clone();
                        }
                        false
                    }
                    _ => false,
                }
            }
            ToolDialog::MergeImages { groups } => {
                let count = groups.len();
                let plural = if count == 1 { "" } else { "s" };
                let merge_label = format!("Merge {count} Duplicate Group{plural}");
                let actions = [DialogAction::text("Cancel"), DialogAction::filled(&merge_label)];
                let spec = DialogSpec {
                    title: Some("Merge Duplicate Card Images"),
                    width: 480.0 + 48.0,
                    actions: &actions,
                    ..DialogSpec::default()
                };
                let cards = game.cards();
                let response = frame.dialog("mergeimages", &spec, |frame, x, y, width| {
                    let intro = "These cards share a set and an image, and will be merged into one entry each, keeping \
                                 the first one's data and combining their tags. The others will be deleted.";
                    frame.text(
                        intro,
                        x,
                        y,
                        TextOptions::new().width(width).size(12.0).color(palette.on_surface_variant),
                    );
                    let mut row_y = y + frame.text_height(intro, width, 12.0, Weight::Regular) + 12.0;
                    for group in groups.iter() {
                        let first = &cards[group.indexes[0]];
                        let set_name = first.set_id.as_ref().and_then(|set_id| names.get(set_id));
                        let title = format!("{} ({})", first.card_title, set_name.map_or("no set", String::as_str));
                        frame.text(&title, x, row_y, TextOptions::new().bold().width(width).ellipsis());
                        let summary = format!("{} cards will merge into one.", group.indexes.len());
                        frame.text(&summary, x, row_y + 20.0, TextOptions::new().width(width));
                        let types = union_types(cards, &group.indexes);
                        let tags = if types.is_empty() {
                            "Tags: (none)".to_string()
                        } else {
                            format!("Tags: {}", types.join(", "))
                        };
                        frame.text(&tags, x, row_y + 40.0, TextOptions::new().width(width));
                        row_y += 40.0 + frame.text_height(&tags, width, 14.0, Weight::Regular) + 12.0;
                    }
                    row_y - y
                });
                match response {
                    DialogResponse::Open | DialogResponse::Enter => true,
                    DialogResponse::Action(1) => {
                        let dropped: Vec<usize> =
                            groups.iter().flat_map(|group| group.indexes[1..].iter().copied()).collect();
                        let merged = merge_duplicate_image_cards(game.cards());
                        *game.cards_mut() = merged;
                        card_keys.retain(|index| !dropped.contains(&index));
                        frame.ui.toast(&format!("Merged {count} duplicate group{plural}."), None);
                        false
                    }
                    _ => false,
                }
            }
        }
    }
}
