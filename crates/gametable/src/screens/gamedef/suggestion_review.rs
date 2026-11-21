//! The review list shared by Card View's "Suggest Title" and "Suggest Extra Data" dialogs: one row
//! per card with a tick box, its thumbnail, the current text and the editable suggestion.

use gametable_core::models::{CardBackDefinition, GameDefinition};

use crate::app::Frame;
use crate::screens::gamedef::common::{StableKeys, display_card};
use crate::table::card_render::{CARD_HEIGHT, CardDraw};
use crate::ui::draw::TextOptions;
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::Weight;
use crate::ui::widgets::{ButtonOptions, ButtonVariant, ToggleOptions};

/// One suggestion in the review list.
#[derive(Debug, Clone)]
pub struct ReviewRow {
    /// The editor's stable card key.
    pub key: u64,
    /// What the card has now (shown greyed).
    pub old_text: String,
    /// The suggestion, editable before applying.
    pub new_text: String,
    /// Whether applying changes this card.
    pub apply: bool,
}

/// The cards a review covers and how they draw.
pub struct ReviewCards<'a> {
    pub game: &'a GameDefinition,
    pub folder_path: &'a str,
    pub card_keys: &'a StableKeys,
    /// From [`crate::screens::gamedef::common::display_card_backs`].
    pub display_backs: &'a [CardBackDefinition],
}

/// Draws the summary, Select All / None and the rows inside a dialog body; returns the height
/// used. `id_prefix` keeps widget ids apart; `new_text_label` labels the editable column.
#[allow(clippy::too_many_arguments)]
pub fn draw_review_rows(
    frame: &mut Frame,
    id_prefix: &str,
    new_text_label: &str,
    summary: &str,
    rows: &mut [ReviewRow],
    cards: &ReviewCards,
    x: f32,
    y: f32,
    width: f32,
) -> f32 {
    let palette = frame.palette();
    let (_, screen_height) = frame.ui.screen_size();
    let mut row_y = y;
    let summary_options = TextOptions::new().width(width).size(12.0).color(palette.on_surface_variant);
    frame.text(summary, x, row_y, summary_options);
    row_y += frame.text_height(summary, width, 12.0, Weight::Regular) + 8.0;
    let all_ticked = rows.iter().all(|row| row.apply);
    let toggle_label = if all_ticked { "Select None" } else { "Select All" };
    let toggle_id = format!("{id_prefix}:all");
    if frame.button(
        toggle_label,
        x - 12.0,
        row_y,
        None,
        Some(32.0),
        &ButtonOptions::variant(ButtonVariant::Text).id(&toggle_id),
    ) {
        for row in rows.iter_mut() {
            row.apply = !all_ticked;
        }
    }
    row_y += 32.0 + 8.0;
    let row_height = 64.0;
    let thumbnail_height = 56.0;
    let thumbnail_width = thumbnail_height * 0.716;
    for row in rows.iter_mut() {
        // Only rows on screen are drawn; long reviews stay quick.
        if row_y + row_height >= 0.0 && row_y <= screen_height {
            row.apply = frame.checkbox(
                &format!("{id_prefix}:{}:apply", row.key),
                x,
                row_y + 19.0,
                row.apply,
                &ToggleOptions::default(),
            );
            if let Some(index) = cards.card_keys.index_of(row.key) {
                let card = &cards.game.cards()[index];
                let display = display_card(cards.folder_path, card);
                let draw = CardDraw::new(
                    Some(&display),
                    true,
                    x + 32.0 + thumbnail_width / 2.0,
                    row_y + row_height / 2.0,
                    cards.display_backs,
                    cards.game.scenario_card_back_id.as_deref(),
                )
                .scale(thumbnail_height / CARD_HEIGHT);
                frame.draw_card(&draw);
            }
            let text_x = x + 32.0 + thumbnail_width + 12.0;
            let column_width = (x + width - text_x - 8.0) / 2.0;
            let old_options = TextOptions::new()
                .width(column_width)
                .middle_of(row_height)
                .ellipsis()
                .color(palette.on_surface_variant);
            frame.text(&row.old_text, text_x, row_y, old_options);
            let field_options = TextFieldOptions {
                height: Some(44.0),
                label_background: Some(palette.surface_container_high),
                ..TextFieldOptions::label(new_text_label)
            };
            frame.text_field(
                &format!("{id_prefix}:{}:text", row.key),
                text_x + column_width + 8.0,
                row_y + 10.0,
                column_width,
                &mut row.new_text,
                &field_options,
            );
        }
        row_y += row_height;
    }
    row_y - y
}
