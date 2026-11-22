//! The "General" tab: the game's name and switches, its card backs (the first is the default),
//! optional table colours and its tag groups. Port of `src/ui/screens/gamedef/settings_tab.lua`
//! (`widgets/game_settings_tab.dart`). Edits apply straight to the editor's game.

use std::collections::HashMap;

use gametable_core::color::{color_from_hex, hex_from_color};
use gametable_core::models::{CardBackDefinition, TagGroup};
use gametable_core::util::trim_lua_whitespace;
use gametable_core::uuid::new_v4_uuid;

use crate::app::Frame;
use crate::data::display_settings::{DEFAULT_CURRENT_TURN_COLOR, DEFAULT_FELT_COLOR};
use crate::data::file_system::strip_extension;
use crate::data::gamedef_ops::rename_tag_group_in_cards;
use crate::data::image_resolver::resolve_bare_image_path;
use crate::screens::gamedef::EditorModel;
use crate::screens::gamedef::common::{
    FIELD_HEIGHT, FileImport, GAP, ImportResponse, PAD, SECTION_GAP, SectionAction, StableKeys, column,
    orientation_choices,
};
use crate::table::card_render::{CARD_HEIGHT, CARD_WIDTH};
use crate::ui::components::{CHIP_HEIGHT, Flow, Reorder, move_item};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::dropdown::DropdownOptions;
use crate::ui::text_field::{TextFieldOptions, text_field_height};
use crate::ui::theme::{Weight, with_alpha};
use crate::ui::widgets::{BUTTON_HEIGHT, ButtonOptions, ButtonVariant, Choice, IconButtonOptions, ScrollbarStyle};

const THUMB_WIDTH: f32 = (CARD_WIDTH * 0.7) as i32 as f32;
const THUMB_HEIGHT: f32 = (CARD_HEIGHT * 0.7) as i32 as f32;
const ORIENTATION_WIDTH: f32 = 150.0;
const CHIP_GAP: f32 = 8.0;

/// What a running image import is for.
enum ImportTarget {
    BackImage(u64),
    NewBack,
}

/// A tag chip being pressed or dragged.
#[derive(Debug, Default)]
struct ChipDragState {
    press: Option<(usize, f32, f32)>,
    dragging: Option<usize>,
}

/// A tag group's body height last frame (for its drag handle row).
#[derive(Debug, Default)]
struct BodyHeight(f32);

#[derive(Default)]
pub struct SettingsTab {
    back_keys: StableKeys,
    group_keys: StableKeys,
    new_group_name: String,
    expanded: HashMap<u64, bool>,
    new_tag: HashMap<u64, String>,
    duplicate_tag: HashMap<u64, bool>,
    import: Option<(FileImport, ImportTarget)>,
}

/// `chipWidth(tag)`.
fn chip_width(frame: &mut Frame, tag: &str) -> f32 {
    frame.text_width(tag, 14.0, Weight::Regular) + 12.0 + 28.0
}

impl SettingsTab {
    /// One card back's row: handle, thumbnail (click to choose an image), name and orientation,
    /// delete. Returns (height, delete clicked).
    #[allow(clippy::too_many_arguments)]
    fn draw_card_back_row(
        &mut self,
        frame: &mut Frame,
        reorder: &Reorder,
        index: usize,
        back: &mut CardBackDefinition,
        folder: &str,
        x: f32,
        y: f32,
        width: f32,
    ) -> (f32, bool) {
        let palette = frame.palette();
        let key = self.back_keys.key(index);
        let id = format!("gs:back:{key}");
        let height = THUMB_HEIGHT + 16.0;
        reorder.handle(frame, index, x - 4.0, y + (height - 40.0) / 2.0, y, height);

        // Thumbnail: click to choose; a small clear button on hover.
        let (thumb_x, thumb_y) = (x + 40.0, y + 8.0);
        let path = back.image_path.as_deref().map(|bare| resolve_bare_image_path(folder, bare, None));
        let thumb = frame.ui.region(
            &format!("{id}:thumb"),
            thumb_x,
            thumb_y,
            THUMB_WIDTH,
            THUMB_HEIGHT,
            RegionOptions::hand().with_tooltip(Some("Choose Image...")),
        );
        frame.image_preview_box(path.as_deref(), thumb_x, thumb_y, THUMB_WIDTH, THUMB_HEIGHT);
        if thumb.hovered {
            frame.fill_rect(thumb_x, thumb_y, THUMB_WIDTH, THUMB_HEIGHT, 6.0, with_alpha(palette.on_surface, 0.08));
        }
        let mut cleared = false;
        if back.image_path.is_some() && frame.ui.hover_ignoring_layer(thumb_x, thumb_y, THUMB_WIDTH, THUMB_HEIGHT) {
            let clear_id = format!("{id}:clear");
            let options = IconButtonOptions {
                size: Some(20.0),
                id: Some(&clear_id),
                tooltip: Some("Clear Image"),
                background: Some(palette.surface),
                ..IconButtonOptions::default()
            };
            cleared = frame.icon_button("close", thumb_x + THUMB_WIDTH - 22.0, thumb_y + 2.0, &options);
        }
        if cleared {
            back.image_path = None;
        } else if thumb.clicked {
            self.import = Some((FileImport::image(frame, folder.to_string(), false), ImportTarget::BackImage(key)));
        }

        let field_x = thumb_x + THUMB_WIDTH + 16.0;
        let field_y = y + (height - FIELD_HEIGHT) / 2.0;
        let right = x + width - 44.0;
        let mut badge_width = 0.0;
        if index == 0 {
            badge_width = frame.text_width("Default", 12.0, Weight::Medium) + 16.0;
            let badge_x = right - badge_width - 8.0;
            frame.fill_rect(badge_x, y + (height - 24.0) / 2.0, badge_width, 24.0, 12.0, palette.secondary_container);
            let options = TextOptions::new()
                .width(badge_width)
                .middle_of(height)
                .size(12.0)
                .medium()
                .centered()
                .color(palette.on_secondary_container);
            frame.text("Default", badge_x, y, options);
            badge_width += 8.0;
        }
        let field_width = right - badge_width - 8.0 - field_x - ORIENTATION_WIDTH - GAP;
        frame.text_field(
            &format!("{id}:name"),
            field_x,
            field_y,
            field_width,
            &mut back.name,
            &TextFieldOptions::label("Name"),
        );
        let options = DropdownOptions { label: Some("Orientation"), ..DropdownOptions::default() };
        back.orientation = frame.dropdown(
            &format!("{id}:orient"),
            field_x + field_width + GAP,
            field_y,
            ORIENTATION_WIDTH,
            &orientation_choices(),
            back.orientation,
            &options,
        );
        let delete_id = format!("{id}:del");
        let delete = frame.icon_button(
            "delete_outline",
            x + width - 40.0,
            y + (height - 40.0) / 2.0,
            &IconButtonOptions::tooltip("Remove Card Back").id(&delete_id),
        );
        (height, delete)
    }

    /// `T:drawTagChips`: a group's tags as removable chips, drag to reorder. Returns the height.
    fn draw_tag_chips(frame: &mut Frame, id: &str, tags: &mut Vec<String>, x: f32, y: f32, width: f32) -> f32 {
        let palette = frame.palette();
        let state_id = format!("{id}:chips");
        // Lay out first so drag targets know every chip's rect.
        let mut flow = Flow::new(x, y, width, CHIP_HEIGHT, CHIP_GAP);
        let rects: Vec<(f32, f32, f32)> = tags
            .iter()
            .map(|tag| {
                let chip = chip_width(frame, tag).min(width);
                let (chip_x, chip_y) = flow.place(chip);
                (chip_x, chip_y, chip)
            })
            .collect();
        let (mouse_x, mouse_y) = frame.ui.mouse();
        let dragging = frame.ui.state::<ChipDragState>(&state_id).dragging;
        let mut remove = None;
        for (index, tag) in tags.iter().enumerate() {
            let (chip_x, chip_y, chip) = rects[index];
            let body = frame.ui.region(
                &format!("{id}:chip{}", index + 1),
                chip_x,
                chip_y,
                chip - 28.0,
                CHIP_HEIGHT,
                RegionOptions::cursor("sizeall").with_tooltip(Some("Drag to reorder")),
            );
            if body.pressed && frame.ui.mouse_pressed(1) {
                frame.ui.state::<ChipDragState>(&state_id).press = Some((index, mouse_x, mouse_y));
            }
            let fill = if dragging == Some(index) {
                with_alpha(palette.primary, 0.16)
            } else {
                palette.surface_container_high
            };
            frame.fill_rect(chip_x, chip_y, chip, CHIP_HEIGHT, 8.0, fill);
            if body.hovered && dragging.is_none() {
                frame.fill_rect(chip_x, chip_y, chip, CHIP_HEIGHT, 8.0, with_alpha(palette.on_surface, 0.06));
            }
            let options = TextOptions::new().middle_of(CHIP_HEIGHT).width(chip - 12.0 - 28.0).ellipsis();
            frame.text(tag, chip_x + 12.0, chip_y, options);
            let remove_id = format!("{id}:rmtag:{}", index + 1);
            let options = IconButtonOptions::tooltip("Remove Tag").id(&remove_id).size(24.0, 24.0 * 0.6);
            if frame.icon_button("close", chip_x + chip - 28.0, chip_y + 4.0, &options) {
                remove = Some(index);
            }
        }
        // A press becomes a drag once the mouse moves a little.
        let mouse_down = frame.ui.mouse_down(1);
        let state = frame.ui.state::<ChipDragState>(&state_id);
        if let (Some((pressed, press_x, press_y)), None) = (state.press, state.dragging) {
            if !mouse_down {
                state.press = None;
            } else if (mouse_x - press_x).abs() + (mouse_y - press_y).abs() > 4.0 {
                state.dragging = Some(pressed);
            }
        }
        if let Some(from) = frame.ui.state::<ChipDragState>(&state_id).dragging {
            frame.ui.set_cursor("sizeall");
            // Insertion slot: before the nearest chip when the mouse is on its left half.
            let mut best: Option<(usize, f32)> = None;
            for (index, &(chip_x, chip_y, chip)) in rects.iter().enumerate() {
                let (center_x, center_y) = (chip_x + chip / 2.0, chip_y + CHIP_HEIGHT / 2.0);
                let distance = (mouse_x - center_x).powi(2) + ((mouse_y - center_y) * 3.0).powi(2);
                if best.is_none_or(|(_, best_distance)| distance < best_distance) {
                    best = Some((index, distance));
                }
            }
            let slot = match best {
                Some((index, _)) => {
                    let (chip_x, _, chip) = rects[index];
                    if mouse_x < chip_x + chip / 2.0 { index } else { index + 1 }
                }
                None => 0,
            };
            if let Some(&(line_chip_x, line_y, line_chip)) = rects.get(slot.min(rects.len().saturating_sub(1))) {
                let line_x = if slot < rects.len() {
                    line_chip_x - CHIP_GAP / 2.0 - 1.0
                } else {
                    line_chip_x + line_chip + CHIP_GAP / 2.0 - 1.0
                };
                frame.fill_rect(line_x, line_y, 3.0, CHIP_HEIGHT, 1.5, palette.primary);
            }
            if !mouse_down {
                let state = frame.ui.state::<ChipDragState>(&state_id);
                state.dragging = None;
                state.press = None;
                let to = if slot > from { slot - 1 } else { slot };
                if to != from && from < tags.len() {
                    move_item(tags, from, to);
                }
            }
        }
        if let Some(index) = remove {
            tags.remove(index);
        }
        if tags.is_empty() {
            return 0.0;
        }
        flow.bottom() - y
    }

    /// `T:drawTagGroup`: handle, collapse toggle, name (editable while expanded), delete; when
    /// expanded, its chips and a new-tag field. Returns (height, delete clicked).
    #[allow(clippy::too_many_arguments)]
    fn draw_tag_group(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        reorder: &Reorder,
        index: usize,
        x: f32,
        y: f32,
        width: f32,
    ) -> (f32, bool) {
        let palette = frame.palette();
        let key = self.group_keys.key(index);
        let id = format!("gs:group:{key}");
        let expanded = self.expanded.get(&key).copied().unwrap_or(false);
        let header_height = 56.0;
        let body_height = frame.ui.state::<BodyHeight>(&id).0;
        let height = header_height + if expanded { body_height } else { 0.0 };
        reorder.handle(frame, index, x - 4.0, y + 8.0, y, height);

        let header_x = x + 40.0;
        let header_width = width - 40.0 - 44.0;
        let tag_count = model.game.tag_groups[index].tags.len();
        let count = format!("{tag_count}{}", if tag_count == 1 { " tag" } else { " tags" });
        if expanded {
            let collapse_id = format!("{id}:exp");
            if frame.icon_button(
                "expand_more",
                header_x,
                y + 8.0,
                &IconButtonOptions::tooltip("Collapse").id(&collapse_id),
            ) {
                self.expanded.insert(key, false);
            }
            let name_width = 360f32.min(header_width - 48.0 - 80.0);
            let old_name = model.game.tag_groups[index].name.clone();
            let mut name = old_name.clone();
            frame.text_field(
                &format!("{id}:name"),
                header_x + 44.0,
                y + 4.0,
                name_width,
                &mut name,
                &TextFieldOptions::label("Group Name"),
            );
            if name != old_name {
                if !old_name.is_empty() {
                    rename_tag_group_in_cards(model.game.cards_mut(), &old_name, &name);
                }
                model.game.tag_groups[index].name = name;
            }
            let options = TextOptions::new().size(12.0).middle_of(header_height).color(palette.on_surface_variant);
            frame.text(&count, header_x + 44.0 + name_width + 12.0, y, options);
        } else {
            let name = &model.game.tag_groups[index].name;
            let title = if name.is_empty() { "(unnamed group)".to_string() } else { name.clone() };
            if frame.collapse_header(
                &format!("{id}:hdr"),
                &title,
                header_x + 6.0,
                y + 8.0,
                header_width - 6.0,
                40.0,
                false,
                Some(&count),
            ) {
                self.expanded.insert(key, true);
            }
        }
        let delete_id = format!("{id}:del");
        let delete = frame.icon_button(
            "delete_outline",
            x + width - 40.0,
            y + 8.0,
            &IconButtonOptions::tooltip("Delete Group").id(&delete_id),
        );
        if !expanded {
            return (height, delete);
        }

        let (body_x, body_width) = (header_x + 44.0, width - 40.0 - 44.0 - 44.0);
        let mut body_y = y + header_height + 4.0;
        let chips_height =
            SettingsTab::draw_tag_chips(frame, &id, &mut model.game.tag_groups[index].tags, body_x, body_y, body_width);
        if chips_height > 0.0 {
            body_y += chips_height + GAP;
        }
        let error = self.duplicate_tag.get(&key).copied().unwrap_or(false).then_some("Tag already exists");
        let field_width = 320f32.min(body_width - 48.0);
        let text = self.new_tag.entry(key).or_default();
        let options = TextFieldOptions { height: Some(40.0), error, ..TextFieldOptions::label("New tag") };
        let field = frame.text_field(&format!("{id}:newtag"), body_x, body_y, field_width, text, &options);
        if field.changed {
            self.duplicate_tag.remove(&key);
        }
        let add_id = format!("{id}:addtag");
        let add = frame.icon_button(
            "add",
            body_x + field_width + 4.0,
            body_y,
            &IconButtonOptions::tooltip("Add Tag").id(&add_id),
        );
        if field.submitted || add {
            let trimmed = trim_lua_whitespace(self.new_tag.get(&key).map_or("", String::as_str)).to_string();
            if !trimmed.is_empty() {
                let tags = &mut model.game.tag_groups[index].tags;
                if tags.contains(&trimmed) {
                    self.duplicate_tag.insert(key, true);
                } else {
                    tags.push(trimmed);
                    self.new_tag.insert(key, String::new());
                    self.duplicate_tag.remove(&key);
                }
            }
        }
        body_y += text_field_height(Some(40.0), error) + GAP;
        let body = body_y - (y + header_height);
        frame.ui.state::<BodyHeight>(&id).0 = body;
        (header_height + body, delete)
    }

    /// `T:addGroup()`: a new empty group with a unique, non-empty name.
    fn add_group(&mut self, model: &mut EditorModel) {
        let trimmed = trim_lua_whitespace(&self.new_group_name).to_string();
        if trimmed.is_empty() || model.game.tag_groups.iter().any(|group| group.name == trimmed) {
            return;
        }
        model.game.tag_groups.push(TagGroup { id: new_v4_uuid(), name: trimmed, tags: Vec::new() });
        self.new_group_name.clear();
    }

    /// A colour field for an optional `"#RRGGBB"` game colour plus its Clear button. Returns the
    /// height used.
    #[allow(clippy::too_many_arguments)]
    fn optional_color(
        frame: &mut Frame,
        id: &str,
        label: &str,
        clear_label: &str,
        value: &mut Option<String>,
        default: u32,
        x: f32,
        y: f32,
        width: f32,
    ) -> f32 {
        let current = value.as_deref().and_then(color_from_hex).unwrap_or(default);
        let (picked, field_height) = frame.color_field(id, x, y, width, current, Some(label), None);
        if picked != current {
            *value = Some(format!("#{}", hex_from_color(picked)));
        }
        let mut row_y = y + field_height + 4.0;
        let clear_id = format!("{id}:clear");
        let options = ButtonOptions {
            variant: ButtonVariant::Text,
            disabled: value.is_none(),
            id: Some(&clear_id),
            ..ButtonOptions::default()
        };
        let clear_width = frame.button_width(clear_label, &options);
        if frame.button(clear_label, x - 12.0, row_y, Some(clear_width), None, &options) {
            *value = None;
        }
        row_y += BUTTON_HEIGHT;
        row_y - y
    }

    fn finish_import(&mut self, frame: &mut Frame, model: &mut EditorModel) {
        let Some((import, _)) = &mut self.import else { return };
        let response = import.show(frame);
        if response == ImportResponse::Open {
            return;
        }
        let Some((_, target)) = self.import.take() else { return };
        let ImportResponse::Imported(name) = response else { return };
        match target {
            ImportTarget::BackImage(key) => {
                if let Some(index) = self.back_keys.index_of(key) {
                    model.game.card_backs[index].image_path = Some(name);
                }
            }
            ImportTarget::NewBack => model.game.card_backs.push(CardBackDefinition {
                id: new_v4_uuid(),
                name: strip_extension(&name).to_string(),
                image_path: Some(name),
                ..CardBackDefinition::default()
            }),
        }
    }

    pub fn draw(&mut self, frame: &mut Frame, model: &mut EditorModel, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        self.back_keys.sync(model.game.card_backs.len());
        self.group_keys.sync(model.game.tag_groups.len());
        let offset = frame.begin_scroll("gs:scroll", x, y, width, height);
        let (column_x, column_width) = column(x, width);
        let mut row_y = y + PAD - offset;
        let game = &mut model.game;

        frame.text_field(
            "gs:name",
            column_x,
            row_y,
            column_width,
            &mut game.name,
            &TextFieldOptions::label("Game Name"),
        );
        row_y += FIELD_HEIGHT + GAP;

        let (value, toggle_height) = frame.toggle_row(
            "gs:scenarios",
            "Contains Scenarios",
            Some(
                "Scenarios are named setups -- shared zones, starting cards, and cards placed on the table -- that \
                 the host can load from the Game Menu during a session. Turn this on to edit them on the Scenarios \
                 tab.",
            ),
            column_x,
            row_y,
            column_width,
            game.has_scenarios,
            false,
        );
        game.has_scenarios = value;
        row_y += toggle_height + GAP;
        let (value, toggle_height) = frame.toggle_row(
            "gs:usestokens",
            "Uses Tokens",
            Some(
                "Tokens are named, reusable images (investigator tokens, resource tokens, etc.) placed on the table \
                 or drawn from a Token Bag. Turn this on to define them on the Tokens tab.",
            ),
            column_x,
            row_y,
            column_width,
            game.uses_tokens,
            false,
        );
        game.uses_tokens = value;
        row_y += toggle_height + GAP;
        let (value, toggle_height) = frame.toggle_row(
            "gs:sameorient",
            "Same Table Orientation For Everyone",
            Some(
                "Cooperative games: the shared table and any cards placed on it look the same for every player, \
                 regardless of seat, instead of appearing mirrored/upside-down for players sitting \"across the \
                 table.\"",
            ),
            column_x,
            row_y,
            column_width,
            game.same_orientation_for_all,
            false,
        );
        game.same_orientation_for_all = value;
        row_y += toggle_height + SECTION_GAP;

        // Card backs.
        let action = SectionAction { label: "Add Card Back...", icon: Some("add"), id: "gs:addback", disabled: false };
        let (header_height, add) = frame.section_header(
            "gs:backs",
            "Card Backs",
            column_x,
            row_y,
            column_width,
            Some(model.game.card_backs.len()),
            Some(
                "The first one is this game's default -- drag to reorder and change which one that is. Each card \
                 can then pick a specific back (or \"Unique\") on the Cards tab.",
            ),
            Some(action),
        );
        if add {
            self.import = Some((FileImport::image(frame, model.folder_path.clone(), false), ImportTarget::NewBack));
        }
        row_y += header_height + 4.0;
        let reorder = Reorder::begin(frame, "gs:backs");
        let mut remove_back = None;
        let folder = model.folder_path.clone();
        for index in 0..model.game.card_backs.len() {
            if index > 0 {
                frame.divider(column_x + 40.0, row_y, column_width - 40.0, None);
            }
            let mut back = model.game.card_backs[index].clone();
            let (row_height, delete) =
                self.draw_card_back_row(frame, &reorder, index, &mut back, &folder, column_x, row_y, column_width);
            model.game.card_backs[index] = back;
            if delete {
                remove_back = Some(index);
            }
            row_y += row_height;
        }
        if let Some((from, to)) = reorder.finish(frame, column_x, column_width) {
            move_item(&mut model.game.card_backs, from, to);
            self.back_keys.moved(from, to);
        }
        if let Some(index) = remove_back {
            model.game.card_backs.remove(index);
            self.back_keys.removed(index);
        }
        if model.game.card_backs.is_empty() {
            let options = TextOptions::new().middle_of(32.0).color(palette.on_surface_variant);
            frame.text("No card backs yet.", column_x, row_y, options);
            row_y += 32.0;
        }
        if model.game.cards().iter().any(|card| card.is_scenario_card) {
            row_y += GAP;
            let mut choices = vec![Choice::new(None, "(Use Default)")];
            choices
                .extend(model.game.card_backs.iter().map(|back| Choice::new(Some(back.id.clone()), back.name.clone())));
            let options = DropdownOptions { label: Some("Scenario Card Back"), ..DropdownOptions::default() };
            model.game.scenario_card_back_id = frame.dropdown(
                "gs:scenarioback",
                column_x,
                row_y,
                360f32.min(column_width),
                &choices,
                model.game.scenario_card_back_id.clone(),
                &options,
            );
            row_y += FIELD_HEIGHT + 4.0;
            let options = TextOptions::new().width(column_width).size(12.0).color(palette.on_surface_variant);
            frame.text(
                "The back shown for cards marked \"Scenario Card\" on the Cards tab, unless a card picks its own.",
                column_x,
                row_y,
                options,
            );
            row_y += 20.0;
        }
        row_y += SECTION_GAP;

        // Table colours: optional per-game defaults; cleared, a player's own Settings choice (or
        // the app's default) applies.
        let (header_height, _) = frame.section_header(
            "gs:tablecolors",
            "Table Colors",
            column_x,
            row_y,
            column_width,
            None,
            Some("Optional. Leave cleared to use the app's default, or whatever a player has picked in their own Settings."),
            None,
        );
        row_y += header_height + 4.0;
        let game = &mut model.game;
        row_y += SettingsTab::optional_color(
            frame,
            "gs:tableColor",
            "Tabletop Color",
            "Clear table color",
            &mut game.table_color,
            DEFAULT_FELT_COLOR,
            column_x,
            row_y,
            column_width,
        ) + GAP;
        row_y += SettingsTab::optional_color(
            frame,
            "gs:currentTurnColor",
            "Current Turn Color",
            "Clear current turn color",
            &mut game.current_turn_color,
            DEFAULT_CURRENT_TURN_COLOR,
            column_x,
            row_y,
            column_width,
        ) + SECTION_GAP;

        // Tag groups.
        let (header_height, _) = frame.section_header(
            "gs:groups",
            "Tag Groups",
            column_x,
            row_y,
            column_width,
            Some(model.game.tag_groups.len()),
            Some(
                "Drag to reorder groups or the tags within them -- this also reorders their filter buttons and the \
                 tag sections on each card.",
            ),
            None,
        );
        row_y += header_height + 4.0;
        let reorder = Reorder::begin(frame, "gs:groups");
        let mut remove_group = None;
        for index in 0..model.game.tag_groups.len() {
            if index > 0 {
                frame.divider(column_x + 40.0, row_y, column_width - 40.0, None);
            }
            let (row_height, delete) =
                self.draw_tag_group(frame, model, &reorder, index, column_x, row_y, column_width);
            if delete {
                remove_group = Some(index);
            }
            row_y += row_height;
        }
        if let Some((from, to)) = reorder.finish(frame, column_x, column_width) {
            move_item(&mut model.game.tag_groups, from, to);
            self.group_keys.moved(from, to);
        }
        if let Some(index) = remove_group {
            model.game.tag_groups.remove(index);
            self.group_keys.removed(index);
        }
        row_y += 8.0;
        let field_width = 360f32.min(column_width - 40.0 - 48.0);
        let options = TextFieldOptions { height: Some(40.0), ..TextFieldOptions::label("New tag group") };
        let field =
            frame.text_field("gs:newgroup", column_x + 40.0, row_y, field_width, &mut self.new_group_name, &options);
        let add = frame.icon_button(
            "add",
            column_x + 40.0 + field_width + 4.0,
            row_y,
            &IconButtonOptions::tooltip("Add Tag Group").id("gs:addgroup"),
        );
        if field.submitted || add {
            self.add_group(model);
        }
        row_y += 40.0 + PAD;
        frame.end_scroll("gs:scroll", row_y + offset - y, &ScrollbarStyle::default());
        self.finish_import(frame, model);
    }
}
