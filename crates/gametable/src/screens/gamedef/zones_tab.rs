//! The Zones tab: a reorderable zone list and the selected zone's settings. Settings that can't
//! apply to the zone as configured are hidden rather than greyed out (their values are kept).
//! Port of `src/ui/screens/gamedef/zones_tab.lua` (`widgets/zones_tab.dart`).
//!
//! The counter and token bag fields are shared with the Scenarios tab's zones.

use gametable_core::models::{DeckEntry, GameDefinition, SharedZoneSide, ZoneDefinition, ZoneKind, ZoneWidgetKind};
use gametable_core::util::trim_lua_whitespace;
use gametable_core::uuid::new_v4_uuid;

use crate::app::Frame;
use crate::data::file_system::join_path;
use crate::screens::gamedef::EditorModel;
use crate::screens::gamedef::common::{
    FIELD_HEIGHT, GAP, PAD, SectionAction, StableKeys, ToggleRow, TypedText, column, orientation_choices,
};
use crate::screens::token_choosers::{ChooserResponse, STEPPER_WIDTH, TokenChooser, token_folder_images};
use crate::ui::components::{Reorder, move_item};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::dropdown::DropdownOptions;
use crate::ui::panel::{Pane, clamp_selection};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::with_alpha;
use crate::ui::widgets::{ButtonOptions, ButtonVariant, Choice, ScrollbarStyle};

const LIST_WIDTH: f32 = 300.0;
const ROW_HEIGHT: f32 = 56.0;
const ENTRY_ROW_HEIGHT: f32 = 48.0;

pub(super) fn kind_choices() -> [Choice<ZoneKind>; 2] {
    [Choice::new(ZoneKind::Card, "Card Zone"), Choice::new(ZoneKind::Widget, "Widget Zone")]
}

pub(super) fn widget_kind_choices() -> [Choice<ZoneWidgetKind>; 2] {
    [Choice::new(ZoneWidgetKind::Counter, "Counter"), Choice::new(ZoneWidgetKind::TokenBag, "Token Bag")]
}

pub(super) fn side_choices() -> [Choice<SharedZoneSide>; 2] {
    [Choice::new(SharedZoneSide::Left, "Left"), Choice::new(SharedZoneSide::Right, "Right")]
}

fn token_total(zone: &ZoneDefinition) -> i64 {
    zone.bag_entries.iter().map(|entry| entry.quantity).sum()
}

/// `badgesOf(zone)`: short flags under a zone's name in the list.
fn badges_of(zone: &ZoneDefinition) -> String {
    if zone.kind != ZoneKind::Card {
        if zone.widget_kind == ZoneWidgetKind::TokenBag {
            let total = token_total(zone);
            return format!("{total} token{}", if total == 1 { "" } else { "s" });
        }
        return "Counter".to_string();
    }
    let mut badges = Vec::new();
    if zone.shared {
        badges.push("Shared");
    }
    if zone.deals_built_deck && !zone.shared {
        badges.push("Deck");
    }
    if zone.face_up {
        badges.push("Face up");
    }
    if zone.is_discard_pile {
        badges.push("Discard");
    }
    if zone.flippable && !zone.shared {
        badges.push("Flippable");
    }
    if badges.is_empty() { "Card zone".to_string() } else { badges.join(" · ") }
}

/// What a token chooser opened from a zone's fields will fill.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum ZonePick {
    CounterImage,
    BagImage,
    BagEntry(usize),
    AddBagEntry,
}

impl ZonePick {
    pub(super) fn chooser(self, game: &GameDefinition, folder: &str) -> TokenChooser {
        match self {
            ZonePick::CounterImage | ZonePick::BagImage => {
                TokenChooser::for_folder(Some(folder.to_string()), "_tokens")
            }
            ZonePick::BagEntry(_) | ZonePick::AddBagEntry => {
                let mut game_for_tokens = GameDefinition::new(&game.id, &game.name);
                game_for_tokens.uses_tokens = game.uses_tokens;
                game_for_tokens.tokens = game.tokens.clone();
                game_for_tokens.folder_path = Some(folder.to_string());
                TokenChooser::for_game(&game_for_tokens)
            }
        }
    }

    pub(super) fn apply(self, zone: &mut ZoneDefinition, file: String) {
        match self {
            ZonePick::CounterImage => zone.counter_image_path = Some(file),
            ZonePick::BagImage => zone.bag_image_path = Some(file),
            ZonePick::BagEntry(index) => {
                if let Some(entry) = zone.bag_entries.get_mut(index) {
                    *entry = DeckEntry::new(file, entry.quantity);
                }
            }
            ZonePick::AddBagEntry => zone.bag_entries.push(DeckEntry::new(file, 1)),
        }
    }
}

/// The token images a bag's contents can name: the game's defined tokens, or the `_tokens`
/// folder's images when it has none. (display name, file).
pub(super) fn bag_token_images(game: &GameDefinition, folder: &str) -> Vec<(String, String)> {
    if game.uses_tokens && !game.tokens.is_empty() {
        return game
            .tokens
            .iter()
            .map(|token| (token.display_name().to_string(), token.image_path.clone().unwrap_or_default()))
            .collect();
    }
    token_folder_images(Some(folder), "", "_tokens").0.into_iter().map(|image| (image.name, image.file)).collect()
}

fn tokens_path(folder: &str, file: Option<&str>) -> Option<String> {
    file.map(|file| join_path(&[folder, "_tokens", file]))
}

/// An image slot that opens a token chooser: thumbnail and file name. Returns whether clicked.
#[allow(clippy::too_many_arguments)]
fn token_image_slot(
    frame: &mut Frame,
    id: &str,
    tooltip: &str,
    folder: &str,
    file: Option<&str>,
    empty_label: &str,
    x: f32,
    y: f32,
    width: f32,
) -> bool {
    let palette = frame.palette();
    let thumb = 56.0;
    let region = frame.ui.region(id, x, y, thumb, thumb, RegionOptions::hand().with_tooltip(Some(tooltip)));
    frame.image_preview_box(tokens_path(folder, file).as_deref(), x, y, thumb, thumb);
    let color = if file.is_some() { palette.on_surface } else { palette.on_surface_variant };
    let options = TextOptions::new().width(width - thumb - 12.0).middle_of(thumb).ellipsis().color(color);
    frame.text(file.unwrap_or(empty_label), x + thumb + 12.0, y, options);
    region.clicked
}

/// The counter's fields: an optional image (replacing the solid fill, its colour becoming a small
/// accent behind the number), the starting value and colours. Returns the height used.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_counter_fields(
    frame: &mut Frame,
    folder: &str,
    zone: &mut ZoneDefinition,
    id: &str,
    x: f32,
    y: f32,
    width: f32,
    pick: &mut Option<ZonePick>,
) -> f32 {
    let half_width = (width - GAP) / 2.0;
    let mut row_y = y;
    if token_image_slot(
        frame,
        &format!("{id}:counterimg"),
        "Choose Counter Image...",
        folder,
        zone.counter_image_path.as_deref(),
        "(no image)",
        x,
        row_y,
        width,
    ) {
        *pick = Some(ZonePick::CounterImage);
    }
    row_y += 56.0 + GAP;

    // Starting Value keeps what's typed; anything unparsable counts as 0.
    let value_of = |text: &str| {
        let is_integer = {
            let digits = text.strip_prefix('-').unwrap_or(text);
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        };
        if is_integer { text.parse::<f64>().unwrap_or(0.0) } else { 0.0 }
    };
    let state_id = format!("{id}:startText");
    let mut text = match frame.ui.state::<TypedText>(&state_id).0.take() {
        Some(text) if value_of(&text) == zone.counter_starting_value => text,
        _ => format_whole(zone.counter_starting_value),
    };
    let filter = |candidate: &str| candidate.bytes().all(|byte| byte.is_ascii_digit() || byte == b'-');
    let options = TextFieldOptions { filter: Some(&filter), ..TextFieldOptions::label("Starting Value") };
    frame.text_field(&format!("{id}:start"), x, row_y, half_width, &mut text, &options);
    zone.counter_starting_value = value_of(&text);
    frame.ui.state::<TypedText>(&state_id).0 = Some(text);
    row_y += FIELD_HEIGHT + GAP;
    let (color, first_height) = frame.color_field(
        &format!("{id}:color"),
        x,
        row_y,
        half_width,
        zone.counter_starting_color,
        Some("Starting Color"),
        None,
    );
    zone.counter_starting_color = color;
    let (text_color, second_height) = frame.color_field(
        &format!("{id}:textcolor"),
        x + half_width + GAP,
        row_y,
        half_width,
        zone.counter_starting_text_color,
        Some("Starting Text Color"),
        None,
    );
    zone.counter_starting_text_color = text_color;
    row_y += first_height.max(second_height);
    row_y - y
}

/// A whole number as Lua's `tostring` writes it (no `.0`).
fn format_whole(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 { format!("{}", value as i64) } else { value.to_string() }
}

/// The token bag's fields: its idle image, whether returned tokens go back in, and its contents
/// (a token and quantity per row). Returns the height used.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_token_bag_fields(
    frame: &mut Frame,
    folder: &str,
    images: &[(String, String)],
    zone: &mut ZoneDefinition,
    id: &str,
    x: f32,
    y: f32,
    width: f32,
    pick: &mut Option<ZonePick>,
) -> f32 {
    let palette = frame.palette();
    let mut row_y = y;
    if token_image_slot(
        frame,
        &format!("{id}:bagimg"),
        "Choose Bag Image...",
        folder,
        zone.bag_image_path.as_deref(),
        "(no bag image)",
        x,
        row_y,
        width,
    ) {
        *pick = Some(ZonePick::BagImage);
    }
    row_y += 56.0 + 12.0;
    let (value, toggle_height) = frame.toggle_row(
        &format!("{id}:returnable"),
        "Returnable",
        Some("A returned token goes back into the bag's contents; off, it's destroyed instead"),
        x,
        row_y,
        width,
        zone.bag_returnable,
        false,
    );
    zone.bag_returnable = value;
    row_y += toggle_height + 12.0;
    row_y += frame.group_label("Contents", x, row_y, width) + 4.0;
    let name_for = |file: &str| {
        images.iter().find(|(_, known)| known == file).map_or_else(|| file.to_string(), |(name, _)| name.clone())
    };
    let mut remove = None;
    for index in 0..zone.bag_entries.len() {
        let entry_id = format!("{id}:bagentry{}", index + 1);
        let row_width = width - 8.0 - STEPPER_WIDTH;
        let region = frame.ui.region(
            &format!("{entry_id}:pick"),
            x,
            row_y,
            row_width,
            ENTRY_ROW_HEIGHT,
            RegionOptions::hand().with_tooltip(Some("Choose Token...")),
        );
        if region.hovered {
            frame.fill_rect(x, row_y, row_width, ENTRY_ROW_HEIGHT, 6.0, with_alpha(palette.on_surface, 0.05));
        }
        let file = zone.bag_entries[index].definition_id.clone();
        let has_file = !file.is_empty();
        let path = has_file.then(|| join_path(&[folder, "_tokens", &file]));
        frame.image_preview_box(path.as_deref(), x + 4.0, row_y + 4.0, ENTRY_ROW_HEIGHT - 8.0, ENTRY_ROW_HEIGHT - 8.0);
        let label = if has_file { name_for(&file) } else { "(choose token)".to_string() };
        let color = if has_file { palette.on_surface } else { palette.on_surface_variant };
        let options = TextOptions::new()
            .width(row_width - ENTRY_ROW_HEIGHT - 12.0)
            .middle_of(ENTRY_ROW_HEIGHT)
            .ellipsis()
            .color(color);
        frame.text(&label, x + ENTRY_ROW_HEIGHT + 8.0, row_y, options);
        if region.clicked {
            *pick = Some(ZonePick::BagEntry(index));
        }
        let quantity = zone.bag_entries[index].quantity;
        let (new_quantity, removed) =
            frame.quantity_stepper(&entry_id, x + width, row_y + 4.0, quantity, Some("Remove Entry"), false);
        if new_quantity != quantity {
            zone.bag_entries[index] = DeckEntry::new(file, new_quantity);
        }
        if removed {
            remove = Some(index);
        }
        row_y += ENTRY_ROW_HEIGHT;
    }
    if let Some(index) = remove {
        zone.bag_entries.remove(index);
    }
    let add_id = format!("{id}:addbagentry");
    let options = ButtonOptions::variant(ButtonVariant::Text).icon("add").id(&add_id).disabled(images.is_empty());
    if frame.button("Add Entry", x - 8.0, row_y, None, Some(40.0), &options) {
        *pick = Some(ZonePick::AddBagEntry);
    }
    row_y += 40.0;
    row_y - y
}

/// A zone's starting card entries: a card dropdown and quantity stepper per row, then Add Entry.
/// Returns the height used.
#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
pub(super) fn draw_card_entries(
    frame: &mut Frame,
    card_choices: &[Choice<String>],
    entries: &mut Vec<DeckEntry>,
    id: &str,
    x: f32,
    y: f32,
    width: f32,
) -> f32 {
    let mut row_y = y;
    let mut remove = None;
    for index in 0..entries.len() {
        let entry_id = format!("{id}:entry{}", index + 1);
        let dropdown_width = width - 8.0 - STEPPER_WIDTH;
        let options = DropdownOptions {
            height: Some(40.0),
            placeholder: Some("(unknown card)"),
            size: Some(14.0),
            ..DropdownOptions::default()
        };
        let current = entries[index].definition_id.clone();
        let picked = frame.dropdown(
            &format!("{entry_id}:card"),
            x,
            row_y + 4.0,
            dropdown_width,
            card_choices,
            current.clone(),
            &options,
        );
        if picked != current {
            entries[index] = DeckEntry::new(picked, entries[index].quantity);
        }
        let quantity = entries[index].quantity;
        let (new_quantity, removed) =
            frame.quantity_stepper(&entry_id, x + width, row_y + 4.0, quantity, Some("Remove Entry"), false);
        if new_quantity != quantity {
            entries[index].quantity = new_quantity;
        }
        if removed {
            remove = Some(index);
        }
        row_y += ENTRY_ROW_HEIGHT;
    }
    if let Some(index) = remove {
        entries.remove(index);
    }
    let add_id = format!("{id}:addentry");
    let options = ButtonOptions::variant(ButtonVariant::Text).icon("add").id(&add_id).disabled(card_choices.is_empty());
    if frame.button("Add Entry", x - 8.0, row_y, None, Some(40.0), &options)
        && let Some(first) = card_choices.first()
    {
        entries.push(DeckEntry::new(first.value.clone(), 1));
    }
    row_y += 40.0;
    row_y - y
}

/// A text field mapping onto a deck type: keeps what's typed; empty means `main_deck`.
#[allow(clippy::too_many_arguments)]
pub(super) fn deck_type_field(
    frame: &mut Frame,
    state_id: &str,
    field_id: &str,
    label: &str,
    help: Option<&str>,
    deck_type: &mut String,
    x: f32,
    y: f32,
    width: f32,
) {
    let deck_type_of = |text: &str| {
        if trim_lua_whitespace(text).is_empty() { "main_deck".to_string() } else { text.to_string() }
    };
    let stored = frame.ui.state::<TypedText>(state_id).0.take();
    let mut text = match stored {
        Some(text) if deck_type_of(&text) == *deck_type => text,
        _ => deck_type.clone(),
    };
    let options =
        TextFieldOptions { trailing_width: if help.is_some() { 28.0 } else { 0.0 }, ..TextFieldOptions::label(label) };
    frame.text_field(field_id, x, y, width, &mut text, &options);
    if let Some(help) = help {
        frame.info_icon(&format!("{field_id}:info"), x + width - 30.0, y + (FIELD_HEIGHT - 18.0) / 2.0, help);
    }
    *deck_type = deck_type_of(&text);
    frame.ui.state::<TypedText>(state_id).0 = Some(text);
}

#[derive(Default)]
pub struct ZonesTab {
    keys: StableKeys,
    selected: Option<u64>,
    chooser: Option<(TokenChooser, u64, ZonePick)>,
    pending_remove: Option<usize>,
}

impl ZonesTab {
    fn draw_list(&mut self, frame: &mut Frame, model: &mut EditorModel, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        frame.fill_rect(x, y, width, height, 0.0, palette.surface_container);
        let action = SectionAction { label: "Add Zone", icon: Some("add"), id: "zt:add", disabled: false };
        let (header_height, add) = frame.section_header(
            "zt:list",
            "Zones",
            x + 16.0,
            y + 12.0,
            width - 32.0,
            Some(model.game.zones.len()),
            None,
            Some(action),
        );
        if add {
            model.game.zones.push(ZoneDefinition::new(new_v4_uuid(), "New Zone"));
            self.keys.sync(model.game.zones.len());
            self.selected = Some(self.keys.key(model.game.zones.len() - 1));
        }
        let list_y = y + 12.0 + header_height + 8.0;
        let offset = frame.begin_scroll("zt:list", x, list_y, width, y + height - list_y);
        let reorder = Reorder::begin(frame, "zt:zones");
        let mut row_y = list_y - offset;
        for index in 0..model.game.zones.len() {
            let key = self.keys.key(index);
            let zone = &model.game.zones[index];
            let (row_x, row_width) = (x + 8.0, width - 16.0);
            let is_selected = self.selected == Some(key);
            let region = frame.ui.region(
                &format!("zt:row:{key}"),
                row_x + 40.0,
                row_y,
                row_width - 40.0,
                ROW_HEIGHT,
                RegionOptions::hand(),
            );
            if is_selected {
                frame.fill_rect(row_x, row_y, row_width, ROW_HEIGHT, 8.0, palette.secondary_container);
            } else if region.hovered {
                frame.fill_rect(row_x, row_y, row_width, ROW_HEIGHT, 8.0, with_alpha(palette.on_surface, 0.05));
            }
            reorder.handle(frame, index, row_x, row_y + (ROW_HEIGHT - 40.0) / 2.0, row_y, ROW_HEIGHT);
            let foreground = if is_selected { palette.on_secondary_container } else { palette.on_surface };
            let icon = if zone.kind == ZoneKind::Card { "layers" } else { "widgets" };
            let icon_color = if is_selected { foreground } else { palette.on_surface_variant };
            frame.icon(icon, row_x + 42.0, row_y + (ROW_HEIGHT - 20.0) / 2.0, 20.0, icon_color);
            let text_x = row_x + 72.0;
            let name = if zone.name.is_empty() { "(unnamed zone)" } else { zone.name.as_str() };
            let mut name_options =
                TextOptions::new().width(row_x + row_width - 8.0 - text_x).ellipsis().color(foreground);
            if is_selected {
                name_options = name_options.medium();
            }
            frame.text(name, text_x, row_y + 9.0, name_options);
            let options = TextOptions::new()
                .width(row_x + row_width - 8.0 - text_x)
                .ellipsis()
                .size(12.0)
                .color(palette.on_surface_variant);
            frame.text(&badges_of(zone), text_x, row_y + 30.0, options);
            if region.clicked {
                self.selected = Some(key);
                frame.ui.blur();
            }
            row_y += ROW_HEIGHT + 2.0;
        }
        if let Some((from, to)) = reorder.finish(frame, x + 8.0, width - 16.0) {
            move_item(&mut model.game.zones, from, to);
            self.keys.moved(from, to);
        }
        frame.end_scroll(
            "zt:list",
            model.game.zones.len() as f32 * (ROW_HEIGHT + 2.0) + 12.0,
            &ScrollbarStyle::default(),
        );
    }

    /// The card-zone settings below the Zone Type picker. Returns the height used.
    #[allow(clippy::too_many_arguments)]
    fn draw_card_zone_fields(
        frame: &mut Frame,
        card_choices: &[Choice<String>],
        zone: &mut ZoneDefinition,
        id: &str,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let palette = frame.palette();
        let mut row_y = y;

        row_y += frame.spaced_group_label("Ownership", x, row_y, width);
        let shared = zone.shared;
        let mut visible = zone.visible_to_all;
        let mut shared_value = zone.shared;
        let mut rows = vec![ToggleRow {
            id: format!("{id}:shared"),
            label: "Shared",
            help: Some("One public pile for everyone, instead of one private pile per player"),
            value: &mut shared_value,
            disabled: false,
        }];
        if !shared {
            rows.push(ToggleRow {
                id: format!("{id}:visible"),
                label: "Visible To All",
                help: Some("Let opponents see the real cards in this owned zone"),
                value: &mut visible,
                disabled: false,
            });
        }
        row_y += frame.toggle_grid(&mut rows, x, row_y, width);
        drop(rows);
        zone.shared = shared_value;
        zone.visible_to_all = visible;

        row_y += frame.spaced_group_label("Behavior", x, row_y, width);
        let (mut face_up, mut shuffleable, mut auto_shuffle, mut discard, mut flippable) =
            (zone.face_up, zone.shuffleable, zone.auto_shuffle, zone.is_discard_pile, zone.flippable);
        let mut rows = vec![
            ToggleRow {
                id: format!("{id}:faceup"),
                label: "Face Up",
                help: Some("Cards in this zone show their real face"),
                value: &mut face_up,
                disabled: false,
            },
            ToggleRow {
                id: format!("{id}:shufflable"),
                label: "Shuffleable",
                help: Some("Show a shuffle button for this zone"),
                value: &mut shuffleable,
                disabled: false,
            },
            ToggleRow {
                id: format!("{id}:autoshuffle"),
                label: "Auto Shuffle",
                help: Some("Randomize this zone's order automatically when the game loads"),
                value: &mut auto_shuffle,
                disabled: false,
            },
            ToggleRow {
                id: format!("{id}:discard"),
                label: "Is Discard Pile",
                help: Some("Pressing D sends a hovered table card here"),
                value: &mut discard,
                disabled: false,
            },
        ];
        if !shared {
            rows.push(ToggleRow {
                id: format!("{id}:flippable"),
                label: "Flippable",
                help: Some(
                    "Pressing F on this zone's top card turns it face up/down (showing its other side if \
                     double-sided) without changing the deck's order",
                ),
                value: &mut flippable,
                disabled: false,
            });
        }
        row_y += frame.toggle_grid(&mut rows, x, row_y, width);
        drop(rows);
        (zone.face_up, zone.shuffleable, zone.auto_shuffle, zone.is_discard_pile, zone.flippable) =
            (face_up, shuffleable, auto_shuffle, discard, flippable);

        row_y += frame.spaced_group_label("Display", x, row_y, width);
        let options = DropdownOptions { label: Some("Zone Orientation"), ..DropdownOptions::default() };
        zone.orientation = frame.dropdown(
            &format!("{id}:orient"),
            x,
            row_y,
            (width - GAP) / 2.0,
            &orientation_choices(),
            zone.orientation,
            &options,
        );
        row_y += FIELD_HEIGHT + 8.0;

        if !zone.shared {
            row_y += frame.spaced_group_label("Dealing", x, row_y, width);
            let (deals, toggle_height) = frame.toggle_row(
                &format!("{id}:deals"),
                "Deals Built Deck",
                Some("Receives the player's own loaded deck at game start (owned zones only)"),
                x,
                row_y,
                width,
                zone.deals_built_deck,
                false,
            );
            zone.deals_built_deck = deals;
            row_y += toggle_height;
            if zone.deals_built_deck {
                row_y += 8.0;
                let wide = width >= 560.0;
                let deck_width = if wide { (width - 32.0) / 2.0 } else { width };
                deck_type_field(
                    frame,
                    &format!("{id}:deckTypeText"),
                    &format!("{id}:decktype"),
                    "Deck Type",
                    Some("Which subdeck (by name) this zone loads from a chosen deck file -- defaults to main_deck"),
                    &mut zone.deck_type,
                    x,
                    row_y,
                    deck_width,
                );
                let optional_help = "A chosen deck may leave this zone's subdeck empty or missing (deals empty instead \
                                     of being rejected)";
                if wide {
                    let (optional, _) = frame.toggle_row(
                        &format!("{id}:optional"),
                        "Deck Optional",
                        Some(optional_help),
                        x + deck_width + 32.0,
                        row_y + 2.0,
                        deck_width,
                        zone.deck_optional,
                        false,
                    );
                    zone.deck_optional = optional;
                    row_y += FIELD_HEIGHT;
                } else {
                    row_y += FIELD_HEIGHT + 4.0;
                    let (optional, toggle_height) = frame.toggle_row(
                        &format!("{id}:optional"),
                        "Deck Optional",
                        Some(optional_help),
                        x,
                        row_y,
                        width,
                        zone.deck_optional,
                        false,
                    );
                    zone.deck_optional = optional;
                    row_y += toggle_height;
                }
            }
        } else {
            row_y += frame.spaced_group_label("Shared Deck", x, row_y, width);
            let (standard, toggle_height) = frame.toggle_row(
                &format!("{id}:standard"),
                "Standard 52-Card Deck",
                Some("Deal one of each traditional playing card automatically"),
                x,
                row_y,
                width,
                zone.standard_deck,
                false,
            );
            zone.standard_deck = standard;
            row_y += toggle_height + 8.0;
            let side_width = 40.0 + 200.0;
            let deck_name_width = if zone.standard_deck { 0.0 } else { width - side_width - 24.0 };
            if !zone.standard_deck {
                let field_id = format!("{id}:deckname");
                let mut text = zone.deck_name.clone().unwrap_or_default();
                let options = TextFieldOptions { trailing_width: 28.0, ..TextFieldOptions::label("Deck Name") };
                frame.text_field(&field_id, x, row_y, deck_name_width, &mut text, &options);
                frame.info_icon(
                    &format!("{field_id}:info"),
                    x + deck_name_width - 30.0,
                    row_y + (FIELD_HEIGHT - 18.0) / 2.0,
                    "Loads this deck from the Deck Library's folder for this game",
                );
                if text != zone.deck_name.clone().unwrap_or_default() {
                    zone.deck_name = (!text.is_empty()).then_some(text);
                }
            }
            let side_x = if zone.standard_deck { x } else { x + deck_name_width + 24.0 };
            frame.text(
                "Side",
                side_x,
                row_y + 4.0,
                TextOptions::new().middle_of(40.0).color(palette.on_surface_variant),
            );
            zone.side = frame.segmented(
                &format!("{id}:side"),
                side_x + 40.0,
                row_y + 4.0,
                side_width - 40.0,
                &side_choices(),
                zone.side,
                None,
                false,
            );
            row_y += FIELD_HEIGHT;
        }

        // Starting contents, ignored when another source fills the zone.
        row_y += frame.spaced_group_label("Starting Contents", x, row_y, width);
        let has_deck_name = zone.deck_name.as_deref().is_some_and(|name| !name.is_empty());
        let why = if !zone.shared && zone.deals_built_deck {
            Some("Ignored while Deals Built Deck is on.")
        } else if zone.shared && zone.standard_deck {
            Some("Ignored while Standard 52-Card Deck is on.")
        } else if zone.shared && has_deck_name {
            Some("Ignored while a Deck Name is set.")
        } else {
            None
        };
        if let Some(why) = why {
            frame.text(why, x, row_y, TextOptions::new().middle_of(32.0).color(palette.on_surface_variant));
            return row_y + 32.0 - y;
        }
        row_y += draw_card_entries(frame, card_choices, &mut zone.entries, id, x, row_y, width);
        row_y - y
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_detail(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        index: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let key = self.keys.key(index);
        let id = format!("zt:{key}");
        let offset = frame.begin_scroll("zt:detail", x, y, width, height);
        let (column_x, column_width) = column(x, width);
        let mut row_y = y + PAD - offset;
        let card_choices: Vec<Choice<String>> =
            model.game.cards().iter().map(|card| Choice::new(card.id.clone(), card.card_title.clone())).collect();
        let bag_images = bag_token_images(&model.game, &model.folder_path);
        let folder = model.folder_path.clone();
        let zone = &mut model.game.zones[index];

        let name = if zone.name.is_empty() { "(unnamed zone)".to_string() } else { zone.name.clone() };
        let options = TextOptions::new().size(20.0).medium().middle_of(40.0).width(column_width - 160.0).ellipsis();
        frame.text(&name, column_x, row_y, options);
        let delete_id = format!("{id}:del");
        let delete_options = ButtonOptions::variant(ButtonVariant::Text).icon("delete_outline").id(&delete_id);
        let delete_width = frame.button_width("Delete Zone", &delete_options);
        if frame.button(
            "Delete Zone",
            column_x + column_width - delete_width,
            row_y,
            Some(delete_width),
            Some(40.0),
            &delete_options,
        ) {
            self.pending_remove = Some(index);
        }
        row_y += 40.0 + 16.0;

        let half_width = (column_width - GAP) / 2.0;
        frame.text_field(
            &format!("{id}:name"),
            column_x,
            row_y,
            half_width,
            &mut zone.name,
            &TextFieldOptions::label("Zone Name"),
        );
        frame.text_field(
            &format!("{id}:id"),
            column_x + half_width + GAP,
            row_y,
            half_width,
            &mut zone.id,
            &TextFieldOptions::label("Zone ID"),
        );
        row_y += FIELD_HEIGHT + 16.0;
        zone.kind = frame.segmented(
            &format!("{id}:kind"),
            column_x,
            row_y,
            column_width.min(320.0),
            &kind_choices(),
            zone.kind,
            None,
            false,
        );
        row_y += 40.0 + 8.0;

        if zone.kind == ZoneKind::Card {
            row_y += ZonesTab::draw_card_zone_fields(frame, &card_choices, zone, &id, column_x, row_y, column_width);
        } else {
            let mut pick = None;
            row_y += frame.spaced_group_label("Ownership", column_x, row_y, column_width);
            let (shared, toggle_height) = frame.toggle_row(
                &format!("{id}:shared"),
                "Shared",
                Some("One public widget for everyone, instead of one private widget per player"),
                column_x,
                row_y,
                column_width,
                zone.shared,
                false,
            );
            zone.shared = shared;
            row_y += toggle_height + 8.0;
            row_y += frame.spaced_group_label("Widget", column_x, row_y, column_width) + 8.0;
            let options = DropdownOptions { label: Some("Widget Type"), ..DropdownOptions::default() };
            zone.widget_kind = frame.dropdown(
                &format!("{id}:wkind"),
                column_x,
                row_y,
                half_width,
                &widget_kind_choices(),
                zone.widget_kind,
                &options,
            );
            if zone.shared {
                let palette = frame.palette();
                let side_x = column_x + half_width + GAP;
                frame.text(
                    "Side",
                    side_x,
                    row_y + 4.0,
                    TextOptions::new().middle_of(40.0).color(palette.on_surface_variant),
                );
                zone.side = frame.segmented(
                    &format!("{id}:side"),
                    side_x + 40.0,
                    row_y + 4.0,
                    half_width - 40.0,
                    &side_choices(),
                    zone.side,
                    None,
                    false,
                );
            }
            row_y += FIELD_HEIGHT + GAP;
            if zone.widget_kind == ZoneWidgetKind::TokenBag {
                row_y += draw_token_bag_fields(
                    frame,
                    &folder,
                    &bag_images,
                    zone,
                    &id,
                    column_x,
                    row_y,
                    column_width,
                    &mut pick,
                );
            } else {
                row_y += draw_counter_fields(frame, &folder, zone, &id, column_x, row_y, column_width, &mut pick);
            }
            if let Some(pick) = pick {
                self.chooser = Some((pick.chooser(&model.game, &folder), key, pick));
            }
        }
        row_y += PAD;
        frame.end_scroll("zt:detail", row_y + offset - y, &ScrollbarStyle::default());
    }

    pub fn draw(&mut self, frame: &mut Frame, model: &mut EditorModel, x: f32, y: f32, width: f32, height: f32) {
        self.keys.sync(model.game.zones.len());
        self.pending_remove = None;
        let keys: Vec<u64> = (0..model.game.zones.len()).map(|index| self.keys.key(index)).collect();
        clamp_selection(&mut self.selected, &keys);
        let selected_index = self.selected.and_then(|key| self.keys.index_of(key));
        frame.list_detail(
            x,
            y,
            width,
            height,
            LIST_WIDTH,
            selected_index.is_some(),
            "Add a zone to get started",
            |frame, pane| match pane {
                Pane::List(rect) => self.draw_list(frame, model, rect.x, rect.y, rect.width, rect.height),
                Pane::Detail(rect) => {
                    if let Some(index) = selected_index {
                        self.draw_detail(frame, model, index, rect.x, rect.y, rect.width, rect.height);
                    }
                }
            },
        );
        if let Some(index) = self.pending_remove.take() {
            model.game.zones.remove(index);
            self.keys.removed(index);
            let count = model.game.zones.len();
            self.selected = (count > 0).then(|| self.keys.key(index.min(count - 1)));
        }
        if let Some((chooser, key, pick)) = &mut self.chooser {
            match chooser.show(frame) {
                ChooserResponse::Open => {}
                ChooserResponse::Cancelled => self.chooser = None,
                ChooserResponse::Picked(file) => {
                    let (key, pick) = (*key, *pick);
                    self.chooser = None;
                    if let Some(index) = self.keys.index_of(key) {
                        pick.apply(&mut model.game.zones[index], file);
                    }
                }
            }
        }
    }
}
