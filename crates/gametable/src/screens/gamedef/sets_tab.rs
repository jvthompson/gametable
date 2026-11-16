//! The Sets tab: a reorderable set list and the selected set's settings. "Add Set..." picks a
//! folder of card art: its name becomes the set id (and default name), its images are copied into
//! `<game folder>/<id>/`, and one card is made per image. Port of
//! `src/ui/screens/gamedef/sets_tab.lua` (`widgets/sets_tab.dart`).

use gametable_core::models::{GameDefinition, GameSet, PackSetting};
use gametable_core::tag_match::parse_tag_entry;

use crate::app::Frame;
use crate::data::file_system::join_path;
use crate::data::gamedef_ops::{
    build_cards_from_image_folder, copy_set_images, derive_id_from_folder_path, rarity_tags,
};
use crate::data::image_resolver::clear_existence_cache;
use crate::screens::file_picker::{FilePicker, FilePickerResponse, PickerMode};
use crate::screens::gamedef::EditorModel;
use crate::screens::gamedef::common::{FIELD_HEIGHT, GAP, PAD, SectionAction, StableKeys, TypedText, column};
use crate::ui::components::{Reorder, move_item};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::ConfirmResponse;
use crate::ui::panel::{Pane, clamp_selection};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::with_alpha;
use crate::ui::widgets::{ButtonOptions, ButtonVariant, ScrollbarStyle};

const LIST_WIDTH: f32 = 300.0;
const ROW_HEIGHT: f32 = 56.0;

/// A set folder waiting to be copied (a couple of frames later, so the spinner shows first).
struct PendingAdd {
    source_path: String,
    id: String,
    frames: u32,
}

#[derive(Default)]
pub struct SetsTab {
    keys: StableKeys,
    selected: Option<u64>,
    pub busy: bool,
    folder_picker: Option<FilePicker>,
    pending_add: Option<PendingAdd>,
    /// The set awaiting the Remove Set confirmation.
    confirm_delete: Option<u64>,
}

fn count_cards_in_set(game: &GameDefinition, set_id: &str) -> usize {
    game.cards().iter().filter(|card| card.set_id.as_deref() == Some(set_id)).count()
}

/// True when some card in the set carries one of the rarity tags.
fn set_has_rarity(game: &GameDefinition, set_id: &str, rarities: &[String]) -> bool {
    game.cards()
        .iter()
        .filter(|card| card.set_id.as_deref() == Some(set_id))
        .any(|card| card.types.iter().any(|entry| rarities.iter().any(|tag| tag == parse_tag_entry(entry).1)))
}

/// `tonumber(text) or 0` for the pack fields (digits and at most one dot).
fn decimal_value(text: &str) -> f64 {
    if text.is_empty() || text == "." { 0.0 } else { text.parse().unwrap_or(0.0) }
}

/// Lua's `tostring` for a pack count.
fn format_count(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 { format!("{}", value as i64) } else { value.to_string() }
}

impl SetsTab {
    /// `T:update()`: copies a picked set folder once its frames have passed.
    pub fn update(&mut self, frame: &mut Frame, model: &mut EditorModel) {
        let Some(pending) = &mut self.pending_add else { return };
        pending.frames -= 1;
        if pending.frames > 0 {
            return;
        }
        let Some(pending) = self.pending_add.take() else { return };
        let destination = join_path(&[&model.folder_path, &pending.id]);
        let result = copy_set_images(&pending.source_path, &destination).map(|_| {
            let existing: Vec<String> = model.game.cards().iter().map(|card| card.id.clone()).collect();
            let new_cards = build_cards_from_image_folder(&pending.source_path, Some(&pending.id), &existing);
            model.game.sets.push(GameSet {
                id: pending.id.clone(),
                name: pending.id.clone(),
                pack_settings: Vec::new(),
            });
            model.game.cards_mut().extend(new_cards);
            model.card_keys.sync(model.game.cards().len());
            self.keys.sync(model.game.sets.len());
            self.selected = Some(self.keys.key(model.game.sets.len() - 1));
        });
        clear_existence_cache();
        if let Err(error) = result {
            frame.ui.toast(&format!("Could not add set: {error}"), None);
        }
        self.busy = false;
    }

    fn draw_list(&mut self, frame: &mut Frame, model: &mut EditorModel, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        frame.fill_rect(x, y, width, height, 0.0, palette.surface_container);
        let add_label = "Add Set...";
        let action = SectionAction { label: add_label, icon: Some("add"), id: "st:add", disabled: self.busy };
        let (header_height, add) = frame.section_header(
            "st:list",
            "Sets",
            x + 16.0,
            y + 12.0,
            width - 32.0,
            Some(model.game.sets.len()),
            Some(
                "Add Set picks a folder of card images: its name becomes the set's ID, its images are copied into \
                 this game's folder, and one card is created per image.",
            ),
            Some(action),
        );
        if add {
            self.folder_picker =
                Some(FilePicker::new(frame, PickerMode::Folder, Some("Choose a folder of card images"), None, None));
        }
        if self.busy {
            let button_width = frame.button_width(add_label, &ButtonOptions::default().icon("add"));
            frame.spinner(x + width - 16.0 - button_width - 24.0, y + 12.0 + 20.0, 10.0, None);
        }
        let list_y = y + 12.0 + header_height + 8.0;
        let offset = frame.begin_scroll("st:list", x, list_y, width, y + height - list_y);
        let reorder = Reorder::begin(frame, "st:sets");
        let mut counts = std::collections::HashMap::<&str, usize>::new();
        for card in model.game.cards() {
            if let Some(set_id) = card.set_id.as_deref() {
                *counts.entry(set_id).or_insert(0) += 1;
            }
        }
        let mut row_y = list_y - offset;
        for index in 0..model.game.sets.len() {
            let key = self.keys.key(index);
            let set = &model.game.sets[index];
            let count = counts.get(set.id.as_str()).copied().unwrap_or(0);
            let (row_x, row_width) = (x + 8.0, width - 16.0);
            let is_selected = self.selected == Some(key);
            let region = frame.ui.region(
                &format!("st:row:{key}"),
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
            let text_x = row_x + 48.0;
            let name = if set.name.is_empty() { "(unnamed set)" } else { set.name.as_str() };
            let mut name_options =
                TextOptions::new().width(row_x + row_width - 8.0 - text_x).ellipsis().color(foreground);
            if is_selected {
                name_options = name_options.medium();
            }
            frame.text(name, text_x, row_y + 9.0, name_options);
            let subtitle = format!("{} · {count}{}", set.id, if count == 1 { " card" } else { " cards" });
            let options = TextOptions::new()
                .width(row_x + row_width - 8.0 - text_x)
                .ellipsis()
                .size(12.0)
                .color(palette.on_surface_variant);
            frame.text(&subtitle, text_x, row_y + 30.0, options);
            if region.clicked {
                self.selected = Some(key);
                frame.ui.blur();
            }
            row_y += ROW_HEIGHT + 2.0;
        }
        if let Some((from, to)) = reorder.finish(frame, x + 8.0, width - 16.0) {
            move_item(&mut model.game.sets, from, to);
            self.keys.moved(from, to);
        }
        if model.game.sets.is_empty() {
            frame.text("No sets yet.", x + 16.0, row_y, TextOptions::new().color(palette.on_surface_variant));
        }
        frame.end_scroll(
            "st:list",
            model.game.sets.len() as f32 * (ROW_HEIGHT + 2.0) + 12.0,
            &ScrollbarStyle::default(),
        );
    }

    /// `drawPackRow`: one rarity's chance field. Keeps `pack_settings` free of zero entries; the
    /// typed text is kept apart so a trailing "." isn't clobbered mid-type.
    fn draw_pack_row(frame: &mut Frame, set: &mut GameSet, tag: &str, id: &str, x: f32, y: f32, width: f32) -> f32 {
        let existing = set.pack_settings.iter().position(|setting| setting.tag == tag);
        let current = existing.map_or(0.0, |index| set.pack_settings[index].count);
        let state_id = format!("{id}:text");
        let mut text = match frame.ui.state::<TypedText>(&state_id).0.take() {
            Some(text) if decimal_value(&text) == current => text,
            _ => format_count(current),
        };
        let label_width = 220f32.min(width - 140.0);
        frame.text(tag, x, y, TextOptions::new().width(label_width).middle_of(FIELD_HEIGHT).ellipsis());
        let filter = |candidate: &str| {
            let mut seen_dot = false;
            candidate.chars().all(|character| {
                if character == '.' && !seen_dot {
                    seen_dot = true;
                    true
                } else {
                    character.is_ascii_digit()
                }
            })
        };
        let options = TextFieldOptions { filter: Some(&filter), ..TextFieldOptions::default() };
        frame.text_field(
            &format!("{id}:field"),
            x + label_width + GAP,
            y,
            width - label_width - GAP,
            &mut text,
            &options,
        );
        let value = decimal_value(&text);
        frame.ui.state::<TypedText>(&state_id).0 = Some(text);
        if value != current {
            match existing {
                Some(index) if value == 0.0 => {
                    set.pack_settings.remove(index);
                }
                Some(index) => set.pack_settings[index].count = value,
                None => set.pack_settings.push(PackSetting { tag: tag.to_string(), count: value }),
            }
        }
        FIELD_HEIGHT + 8.0
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
        let palette = frame.palette();
        let key = self.keys.key(index);
        let id = format!("st:{key}");
        let offset = frame.begin_scroll("st:detail", x, y, width, height);
        let (column_x, column_width) = column(x, width);
        let mut row_y = y + PAD - offset;
        let set_id = model.game.sets[index].id.clone();
        let count = count_cards_in_set(&model.game, &set_id);
        let rarities = rarity_tags(&model.game.tag_groups);
        let has_rarity = !rarities.is_empty() && set_has_rarity(&model.game, &set_id, &rarities);
        let set = &mut model.game.sets[index];

        let name = if set.name.is_empty() { "(unnamed set)".to_string() } else { set.name.clone() };
        let options = TextOptions::new().size(20.0).medium().middle_of(40.0).width(column_width - 160.0).ellipsis();
        frame.text(&name, column_x, row_y, options);
        let delete_id = format!("{id}:del");
        let delete_options = ButtonOptions::variant(ButtonVariant::Text).icon("delete_outline").id(&delete_id);
        let delete_width = frame.button_width("Delete Set", &delete_options);
        if frame.button(
            "Delete Set",
            column_x + column_width - delete_width,
            row_y,
            Some(delete_width),
            Some(40.0),
            &delete_options,
        ) {
            self.confirm_delete = Some(key);
        }
        row_y += 40.0 + 16.0;

        let info_width = 160.0;
        let field_width = column_width - info_width - GAP;
        frame.text_field(
            &format!("{id}:name"),
            column_x,
            row_y,
            field_width,
            &mut set.name,
            &TextFieldOptions::label("Set Name"),
        );
        let info_x = column_x + field_width + GAP;
        frame.text(
            &set.id,
            info_x,
            row_y + 4.0,
            TextOptions::new().width(info_width).ellipsis().color(palette.on_surface_variant),
        );
        let count_text = format!("{count}{}", if count == 1 { " card" } else { " cards" });
        let options = TextOptions::new().width(info_width).size(12.0).color(palette.on_surface_variant);
        frame.text(&count_text, info_x, row_y + 24.0, options);
        row_y += FIELD_HEIGHT + 16.0;

        if has_rarity {
            row_y += 20.0;
            frame.group_label("Pack Settings", column_x, row_y, column_width - 24.0);
            frame.info_icon(
                &format!("{id}:packinfo"),
                column_x + column_width - 22.0,
                row_y + 1.0,
                "A value of 1 or more guarantees that many cards of that rarity in the pack. A fractional part (e.g. \
                 0.5, or the .5 in 1.5) is the chance of one additional card of that rarity, on top of the rest of \
                 the pack. No rarities configured: 10 random cards, rarities ignored.",
            );
            row_y += 20.0 + 10.0;
            for tag in &rarities {
                row_y +=
                    SetsTab::draw_pack_row(frame, set, tag, &format!("{id}:pack:{tag}"), column_x, row_y, column_width);
            }
        }
        row_y += PAD;
        frame.end_scroll("st:detail", row_y + offset - y, &ScrollbarStyle::default());
    }

    pub fn draw(&mut self, frame: &mut Frame, model: &mut EditorModel, x: f32, y: f32, width: f32, height: f32) {
        self.keys.sync(model.game.sets.len());
        let keys: Vec<u64> = (0..model.game.sets.len()).map(|index| self.keys.key(index)).collect();
        clamp_selection(&mut self.selected, &keys);
        let selected_index = self.selected.and_then(|key| self.keys.index_of(key));
        frame.list_detail(
            x,
            y,
            width,
            height,
            LIST_WIDTH,
            selected_index.is_some(),
            "Add a set to get started",
            |frame, pane| match pane {
                Pane::List(rect) => self.draw_list(frame, model, rect.x, rect.y, rect.width, rect.height),
                Pane::Detail(rect) => {
                    if let Some(index) = selected_index {
                        self.draw_detail(frame, model, index, rect.x, rect.y, rect.width, rect.height);
                    }
                }
            },
        );
        self.show_dialogs(frame, model);
    }

    fn show_dialogs(&mut self, frame: &mut Frame, model: &mut EditorModel) {
        if let Some(picker) = &mut self.folder_picker {
            match picker.show(frame) {
                FilePickerResponse::Open => {}
                FilePickerResponse::Cancelled => self.folder_picker = None,
                FilePickerResponse::Picked(mut paths) => {
                    self.folder_picker = None;
                    let source_path = paths.remove(0);
                    let id = derive_id_from_folder_path(&source_path);
                    if model.game.sets.iter().any(|set| set.id == id) {
                        frame.ui.toast(&format!("A set named \"{id}\" already exists."), None);
                    } else {
                        self.busy = true;
                        self.pending_add = Some(PendingAdd { source_path, id, frames: 2 });
                    }
                }
            }
        }
        let Some(key) = self.confirm_delete else { return };
        let Some(index) = self.keys.index_of(key) else {
            self.confirm_delete = None;
            return;
        };
        let set = model.game.sets[index].clone();
        let count = count_cards_in_set(&model.game, &set.id);
        let message = format!(
            "Remove \"{}\" and its {count} card(s) from this game definition? This will not delete any files on disk.",
            set.name
        );
        match frame.confirm_dialog("removeset", "Remove Set", &message, "Remove", false) {
            ConfirmResponse::Open => {}
            ConfirmResponse::Cancelled => self.confirm_delete = None,
            ConfirmResponse::Confirmed => {
                self.confirm_delete = None;
                model.game.sets.remove(index);
                self.keys.removed(index);
                let in_set: Vec<bool> =
                    model.game.cards().iter().map(|card| card.set_id.as_deref() == Some(set.id.as_str())).collect();
                model.game.cards_mut().retain(|card| card.set_id.as_deref() != Some(set.id.as_str()));
                model.card_keys.retain(|card_index| !in_set[card_index]);
                if self.selected == Some(key) {
                    let count = model.game.sets.len();
                    self.selected = (count > 0).then(|| self.keys.key(index.min(count - 1)));
                }
            }
        }
    }
}
