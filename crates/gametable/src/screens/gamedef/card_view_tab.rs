//! The Cards tab: three resizable columns -- a filtered pool grid, a large live preview, and the
//! selected card's form (or a bulk form for several). Port of
//! `src/ui/screens/gamedef/card_view_tab.lua` (`widgets/card_view_tab.dart`,
//! `card_detail_panel.dart`). Edits apply straight to the cards.

use std::collections::{HashMap, HashSet};
use std::f32::consts::PI;

use gametable_core::card_filters::CardSortOrder;
use gametable_core::json::{JsonObject, JsonValue, encode_compact};
use gametable_core::models::{
    CardBackDefinition, CardDefinition, CardOrientation, GameDefinition, UNIQUE_CARD_BACK_ID,
};
use gametable_core::tag_match::{card_has_tag, encode_tag_entry, remove_tag};
use gametable_core::uuid::new_v4_uuid;

use crate::app::Frame;
use crate::data::file_system::join_path;
use crate::data::image_resolver::resolve_card_back_image_path;
use crate::data::settings::keys;
use crate::graphics::draw_list::Color;
use crate::screens::gamedef::EditorModel;
use crate::screens::gamedef::common::{
    FileImport, GAP, ImportResponse, display_card, display_card_backs, orientation_choices,
};
use crate::screens::gamedef::title_suggestion_dialogs::SuggestTitleFlow;
use crate::table::card_pool_grid::{CardZoom, NAME_LINES, TILE_CARD_HEIGHT, TILE_SCALE};
use crate::table::card_render::{CARD_HEIGHT, CARD_WIDTH, CardDraw, orientation_turns};
use crate::ui::card_filter_bar::{CardFilterState, LeadingButton};
use crate::ui::components::{CHIP_HEIGHT, ChipOptions, Flow, TAB_BAR_HEIGHT};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::dropdown::DropdownOptions;
use crate::ui::overlays::ConfirmResponse;
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::{self, Weight, fixed};
use crate::ui::widgets::{ButtonOptions, ButtonVariant, Choice, IconButtonOptions, ScrollbarStyle, ToggleOptions};

const MIN_FRACTION: f32 = 0.15;
const DIVIDER: f32 = 9.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum FormTab {
    Main,
    Extra,
}

/// The Extra Data rows of one card: editable keys in order, rebuilt when the card's map
/// changes underneath.
#[derive(Debug, Default)]
struct ExtraRows {
    rows: Vec<(String, String)>,
    source: Option<JsonObject>,
}

/// A value as Lua's `tostring` shows it.
fn value_text(value: &JsonValue) -> String {
    match value {
        JsonValue::String(text) => text.clone(),
        JsonValue::Number(number) if number.fract() == 0.0 && number.abs() < 1e15 => format!("{}", *number as i64),
        JsonValue::Number(number) => number.to_string(),
        JsonValue::Bool(flag) => flag.to_string(),
        other => encode_compact(other).unwrap_or_default(),
    }
}

/// `turnsOf(orientation)`.
fn quarter_turns_of(orientation: CardOrientation) -> i32 {
    match orientation {
        CardOrientation::Right => 1,
        CardOrientation::Left => -1,
        CardOrientation::Portrait => 0,
    }
}

pub struct CardViewTab {
    filters: CardFilterState,
    pool_fraction: f32,
    preview_fraction: f32,
    form_tab: FormTab,
    tab_navigates_inside_form: bool,
    /// Tab to a card also focuses its title with the text selected.
    tab_focuses_title: bool,
    /// Tag group id -> collapsed.
    hidden_tag_groups: HashSet<String>,
    /// The multi-selection, by card key.
    selected_keys: HashSet<u64>,
    /// The card the next Shift-click ranges from.
    anchor: Option<u64>,
    /// The single selected card.
    selected: Option<u64>,
    import: Option<(FileImport, u64)>,
    confirm_delete: Option<u64>,
    /// The open "Suggest Title" dialogs.
    suggest_title: Option<SuggestTitleFlow>,
    /// Development hook: open Suggest Title on the next frame.
    open_suggest_title_next_frame: bool,
    /// The card grid's zoom slider (GameTable addition).
    card_zoom: CardZoom,
    /// Split handles: last mouse x while held.
    split_drag: HashMap<&'static str, f32>,
}

impl CardViewTab {
    pub fn new(game: &GameDefinition) -> CardViewTab {
        CardViewTab {
            filters: CardFilterState::new(game),
            pool_fraction: 0.5,
            preview_fraction: 0.25,
            form_tab: FormTab::Main,
            tab_navigates_inside_form: false,
            tab_focuses_title: false,
            hidden_tag_groups: HashSet::new(),
            selected_keys: HashSet::new(),
            anchor: None,
            selected: None,
            import: None,
            confirm_delete: None,
            suggest_title: None,
            open_suggest_title_next_frame: false,
            card_zoom: CardZoom::new(keys::CARD_VIEW_CARD_ZOOM),
            split_drag: HashMap::new(),
        }
    }

    /// `T:syncFilters()`: drops filter entries for groups, tags and sets that no longer exist
    /// (they're editable on other tabs) and adds empty ones for new groups.
    fn sync_filters(&mut self, game: &GameDefinition) {
        let filters = &mut self.filters;
        let group_ids: HashSet<&str> = game.tag_groups.iter().map(|group| group.id.as_str()).collect();
        for group in &game.tag_groups {
            for map in [&mut filters.selected, &mut filters.excluded] {
                let tags = map.entry(group.id.clone()).or_default();
                tags.retain(|tag| group.tags.contains(tag));
            }
        }
        filters.selected.retain(|id, _| group_ids.contains(id.as_str()));
        filters.excluded.retain(|id, _| group_ids.contains(id.as_str()));
        filters.sets.retain(|id| game.sets.iter().any(|set| &set.id == id));
        if let Some(key) = &filters.sort_key
            && key != CardSortOrder::BY_SET_KEY
            && key != CardSortOrder::BY_NAME_KEY
            && !group_ids.contains(key.as_str())
        {
            filters.sort_key = None;
        }
        self.hidden_tag_groups.retain(|id| group_ids.contains(id.as_str()));
    }

    /// The filtered cards as (index, key), in the filter's order.
    fn visible(&self, model: &EditorModel) -> Vec<(usize, u64)> {
        let cards = model.game.cards();
        let base = cards.as_ptr() as usize;
        let size = std::mem::size_of::<CardDefinition>().max(1);
        // `visible` returns references into the card list, so each one's index follows from its
        // address.
        self.filters
            .visible(&model.game)
            .into_iter()
            .map(|card| {
                let index = (card as *const CardDefinition as usize - base) / size;
                (index, model.card_keys.key(index))
            })
            .collect()
    }

    /// Selected cards' indexes, in card list order.
    fn selected_indexes(&self, model: &EditorModel) -> Vec<usize> {
        (0..model.game.cards().len())
            .filter(|&index| self.selected_keys.contains(&model.card_keys.key(index)))
            .collect()
    }

    /// Development hook: selects the first card.
    pub fn select_first(&mut self, model: &EditorModel) {
        if !model.game.cards().is_empty() {
            self.select_only(Some(model.card_keys.key(0)));
        }
    }

    /// `T:selectOnly(card)`.
    fn select_only(&mut self, key: Option<u64>) {
        self.selected_keys = key.into_iter().collect();
        self.anchor = key;
        self.selected = key;
    }

    /// Shift-click ranges, Ctrl-click toggles, a plain click replaces the selection.
    fn handle_card_click(&mut self, frame: &Frame, key: u64, visible: &[(usize, u64)]) {
        let shift = frame.keyboard.shift();
        let ctrl = frame.keyboard.ctrl();
        if shift && let Some(anchor) = self.anchor {
            let anchor_position = visible.iter().position(|&(_, known)| known == anchor);
            let card_position = visible.iter().position(|&(_, known)| known == key);
            if let (Some(first), Some(second)) = (anchor_position, card_position) {
                let (low, high) = (first.min(second), first.max(second));
                self.selected_keys = visible[low..=high].iter().map(|&(_, known)| known).collect();
                self.selected = None;
                // The anchor stays put so repeated Shift-clicks re-range from it.
                return;
            }
        }
        if ctrl {
            if !self.selected_keys.remove(&key) {
                self.selected_keys.insert(key);
            }
            self.anchor = Some(key);
            self.selected = None;
            return;
        }
        self.select_only(Some(key));
    }

    fn select_relative(&mut self, model: &EditorModel, delta: i32) {
        let visible = self.visible(model);
        if visible.is_empty() {
            return;
        }
        let position = self.selected.and_then(|key| visible.iter().position(|&(_, known)| known == key));
        if delta > 0 {
            let next = position.map_or(0, |position| position + 1);
            if next < visible.len() {
                self.select_only(Some(visible[next].1));
            }
        } else if let Some(position) = position
            && position > 0
        {
            self.select_only(Some(visible[position - 1].1));
        }
    }

    /// `T:tabToCard(delta)`: next / previous card; with the title option, its title is focused
    /// with the text selected.
    fn tab_to_card(&mut self, frame: &mut Frame, model: &EditorModel, delta: i32) {
        self.select_relative(model, delta);
        if self.tab_focuses_title
            && let Some(key) = self.selected
        {
            frame.ui.focus_field(&format!("cv:form:{key}:title"), true);
        }
    }

    /// Tab / Shift+Tab with no field focused: next / previous card.
    pub fn key_pressed(&mut self, frame: &mut Frame, model: &EditorModel, key: &str) -> bool {
        if key == "tab" && frame.ui.focus.is_none() {
            let delta = if frame.keyboard.shift() { -1 } else { 1 };
            self.tab_to_card(frame, model, delta);
            return true;
        }
        false
    }

    /// `T:addBlankCard()`: in the filtered set when exactly one set is in play.
    fn add_blank_card(&mut self, model: &mut EditorModel) {
        let candidates: Vec<String> = if self.filters.sets.is_empty() {
            model.game.sets.iter().map(|set| set.id.clone()).collect()
        } else {
            self.filters.sets.iter().cloned().collect()
        };
        let card = CardDefinition {
            set_id: (candidates.len() == 1).then(|| candidates[0].clone()),
            ..CardDefinition::new(new_v4_uuid(), "New Card")
        };
        model.game.cards_mut().push(card);
        model.card_keys.sync(model.game.cards().len());
        let key = model.card_keys.key(model.game.cards().len() - 1);
        self.select_only(Some(key));
    }

    fn delete_card(&mut self, model: &mut EditorModel, key: u64) {
        let Some(index) = model.card_keys.index_of(key) else { return };
        model.game.cards_mut().remove(index);
        model.card_keys.removed(index);
        self.selected_keys.remove(&key);
        if self.anchor == Some(key) {
            self.anchor = None;
        }
        if self.selected == Some(key) {
            self.selected = None;
        }
    }

    // -----------------------------------------------------------------------
    // Pool
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn draw_pool(
        &mut self,
        frame: &mut Frame,
        model: &EditorModel,
        display_backs: &[CardBackDefinition],
        visible: &[(usize, u64)],
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let game = &model.game;
        let grid_y = y + 4.0;
        let zoom = self.card_zoom.current(frame.settings);
        let mut clicked = None;
        frame.card_pool_grid(
            "cv:pool",
            visible.len(),
            x,
            grid_y,
            width,
            y + height - grid_y,
            12.0,
            zoom,
            |frame, tile| {
                let (index, key) = visible[tile.index];
                let card = &game.cards()[index];
                let region = frame.ui.region(
                    &format!("cv:card:{key}"),
                    tile.center_x - tile.card_width / 2.0 - 4.0,
                    tile.y - 4.0,
                    tile.card_width + 8.0,
                    tile.card_height + 8.0 + 2.0 + tile.name_height,
                    RegionOptions::hand(),
                );
                let display = display_card(&model.folder_path, card);
                let center_y = tile.card_center_y();
                let draw = CardDraw::new(
                    Some(&display),
                    true,
                    tile.center_x,
                    center_y,
                    display_backs,
                    game.scenario_card_back_id.as_deref(),
                )
                .scale(tile.card_scale)
                .angle(orientation_turns(card.orientation) * 2.0 * PI);
                frame.draw_card(&draw);
                if self.selected_keys.contains(&key) {
                    // Match the card's footprint: turned cards are landscape around the same centre.
                    let turned = orientation_turns(card.orientation) != 0.0;
                    let (box_width, box_height) =
                        if turned { (tile.card_height, tile.card_width) } else { (tile.card_width, tile.card_height) };
                    frame.outline_rect(
                        tile.center_x - box_width / 2.0 - 4.0,
                        center_y - box_height / 2.0 - 4.0,
                        box_width + 8.0,
                        box_height + 8.0,
                        8.0,
                        fixed::BLUE,
                        2.0,
                    );
                }
                frame.text(&card.card_title, tile.x, tile.name_y, tile.name_options());
                if region.clicked {
                    clicked = Some(key);
                }
            },
        );
        if let Some(key) = clicked {
            self.handle_card_click(frame, key, visible);
            frame.ui.blur();
        }
    }

    // -----------------------------------------------------------------------
    // Preview
    // -----------------------------------------------------------------------

    /// `fitCard`: a card as large as fits the box, turned to its orientation.
    #[allow(clippy::too_many_arguments)]
    fn fit_card(
        frame: &mut Frame,
        card: &CardDefinition,
        face_up: bool,
        orientation: CardOrientation,
        backs: &[CardBackDefinition],
        scenario_card_back_id: Option<&str>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let turned = orientation != CardOrientation::Portrait;
        let (box_width, box_height) = if turned { (CARD_HEIGHT, CARD_WIDTH) } else { (CARD_WIDTH, CARD_HEIGHT) };
        let scale = (width / box_width).min(height / box_height);
        let draw = CardDraw::new(Some(card), face_up, x + width / 2.0, y + height / 2.0, backs, scenario_card_back_id)
            .scale(scale)
            .angle(orientation_turns(orientation) * 2.0 * PI);
        frame.draw_card(&draw);
    }

    /// The front: the image uncropped at its own aspect ratio, or the drawn face without one.
    #[allow(clippy::too_many_arguments)]
    fn draw_front_pane(
        frame: &mut Frame,
        model: &EditorModel,
        backs: &[CardBackDefinition],
        card: &CardDefinition,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        frame.fill_rect(x, y, width, height, 0.0, Color::rgba(0.0, 0.0, 0.0, 0.05));
        let display = display_card(&model.folder_path, card);
        let (inner_x, inner_y, inner_width, inner_height) = (x + 20.0, y + 20.0, width - 40.0, height - 40.0);
        let drawn = frame.image_contain_in_box(
            display.image_path.as_deref(),
            inner_x,
            inner_y,
            inner_width,
            inner_height,
            quarter_turns_of(card.orientation),
            true,
        );
        if drawn.is_none() {
            CardViewTab::fit_card(
                frame,
                &display,
                true,
                card.orientation,
                backs,
                None,
                inner_x,
                inner_y,
                inner_width,
                inner_height,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_back_pane(
        frame: &mut Frame,
        model: &EditorModel,
        backs: &[CardBackDefinition],
        card: &CardDefinition,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        frame.fill_rect(x, y, width, height, 0.0, Color::rgba(0.0, 0.0, 0.0, 0.05));
        let scenario_back = model.game.scenario_card_back_id.as_deref();
        let display = display_card(&model.folder_path, card);
        let orientation = resolve_card_back_image_path(backs, Some(&display), scenario_back).orientation;
        CardViewTab::fit_card(
            frame,
            &display,
            false,
            orientation,
            backs,
            scenario_back,
            x + 20.0,
            y + 20.0,
            width - 40.0,
            height - 40.0,
        );
    }

    /// A count and a non-interactive grid of the selected cards.
    #[allow(clippy::too_many_arguments)]
    fn draw_multi_preview(
        frame: &mut Frame,
        model: &EditorModel,
        backs: &[CardBackDefinition],
        selected: &[usize],
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let palette = frame.palette();
        let options = TextOptions::new().width(width).middle_of(24.0).centered().color(palette.on_surface);
        frame.text(&format!("{} cards selected", selected.len()), x, y, options);
        let game = &model.game;
        frame.card_pool_grid(
            "cv:multipreview",
            selected.len(),
            x,
            y + 28.0,
            width,
            height - 28.0,
            12.0,
            1.0,
            |frame, tile| {
                let card = &game.cards()[selected[tile.index]];
                let display = display_card(&model.folder_path, card);
                let draw = CardDraw::new(
                    Some(&display),
                    true,
                    tile.center_x,
                    tile.y + TILE_CARD_HEIGHT / 2.0,
                    backs,
                    game.scenario_card_back_id.as_deref(),
                )
                .scale(TILE_SCALE)
                .angle(orientation_turns(card.orientation) * 2.0 * PI);
                frame.draw_card(&draw);
                let options = TextOptions::new().size(11.0).width(tile.width).centered().max_lines(NAME_LINES as usize);
                frame.text(&card.card_title, tile.x, tile.y + TILE_CARD_HEIGHT + 6.0, options);
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_preview(
        &mut self,
        frame: &mut Frame,
        model: &EditorModel,
        backs: &[CardBackDefinition],
        visible: &[(usize, u64)],
        selected: &[usize],
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        if selected.len() > 1 {
            CardViewTab::draw_multi_preview(frame, model, backs, selected, x, y, width, height);
            return;
        }
        let palette = frame.palette();
        let controls_height = 8.0 + 40.0 + 28.0 + 8.0;
        let pane_height = height - controls_height;
        let card =
            self.selected.and_then(|key| model.card_keys.index_of(key)).map(|index| model.game.cards()[index].clone());
        match &card {
            None => {
                let options =
                    TextOptions::new().width(width).middle_of(pane_height).centered().color(palette.on_surface_variant);
                frame.text("Select a card to preview", x, y, options);
            }
            Some(card) => {
                let shows_back = card.card_back_id.is_some()
                    || (card.is_scenario_card && model.game.scenario_card_back_id.is_some());
                if !shows_back {
                    CardViewTab::draw_front_pane(frame, model, backs, card, x, y, width, pane_height);
                } else {
                    let pane_width = (width - 12.0) / 2.0;
                    let label_height = 8.0 + 16.0;
                    CardViewTab::draw_front_pane(
                        frame,
                        model,
                        backs,
                        card,
                        x,
                        y,
                        pane_width,
                        pane_height - label_height,
                    );
                    let options =
                        TextOptions::new().width(pane_width).size(12.0).centered().color(palette.on_surface_variant);
                    frame.text("Front", x, y + pane_height - 16.0, options);
                    CardViewTab::draw_back_pane(
                        frame,
                        model,
                        backs,
                        card,
                        x + pane_width + 12.0,
                        y,
                        pane_width,
                        pane_height - label_height,
                    );
                    frame.text("Back", x + pane_width + 12.0, y + pane_height - 16.0, options);
                }
            }
        }
        // Previous / position / next on the left, the Tab options on the right.
        let button_y = y + pane_height + 8.0;
        let position = self.selected.and_then(|key| visible.iter().position(|&(_, known)| known == key));
        let can_previous = position.is_some_and(|position| position > 0);
        let can_next = !visible.is_empty() && position.is_none_or(|position| position + 1 < visible.len());
        if frame.icon_button(
            "chevron_left",
            x + 4.0,
            button_y,
            &IconButtonOptions::tooltip("Previous Card").id("cv:prev").disabled(!can_previous),
        ) {
            self.select_relative(model, -1);
        }
        let position_text =
            format!("{} / {}", position.map_or("-".to_string(), |position| (position + 1).to_string()), visible.len());
        let position_width = 56f32.max(frame.text_width(&position_text, 12.0, Weight::Regular) + 12.0);
        let options = TextOptions::new()
            .width(position_width)
            .middle_of(40.0)
            .size(12.0)
            .centered()
            .color(palette.on_surface_variant);
        frame.text(&position_text, x + 44.0, button_y, options);
        if frame.icon_button(
            "chevron_right",
            x + 44.0 + position_width,
            button_y,
            &IconButtonOptions::tooltip("Next Card").id("cv:next").disabled(!can_next),
        ) {
            self.select_relative(model, 1);
        }
        // The two Tab options, right-aligned: one beside the arrows, one below.
        let min_x = x + 44.0 + position_width + 40.0 + 16.0;
        let options =
            [("cv:tabnav", "Tab navigates inside form fields"), ("cv:tabtitle", "Tab focuses and selects Card Title")];
        for (option_index, (id, label)) in options.into_iter().enumerate() {
            let option_y = if option_index == 0 { button_y + 11.0 } else { button_y + 40.0 + 5.0 };
            let checkbox_width = 18.0 + 8.0 + frame.text_width(label, 14.0, Weight::Regular);
            let start = if option_index == 0 { min_x } else { x + 8.0 };
            let fits = start + checkbox_width <= x + width - 8.0;
            let value = if option_index == 0 { self.tab_navigates_inside_form } else { self.tab_focuses_title };
            let new_value = if fits {
                frame.checkbox(id, x + width - 8.0 - checkbox_width, option_y, value, &ToggleOptions::label(label))
            } else {
                let options = ToggleOptions { tooltip: Some(label), ..ToggleOptions::default() };
                frame.checkbox(id, x + width - 8.0 - 18.0, option_y, value, &options)
            };
            if option_index == 0 {
                self.tab_navigates_inside_form = new_value;
            } else {
                self.tab_focuses_title = new_value;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Forms
    // -----------------------------------------------------------------------

    fn back_choices(game: &GameDefinition) -> Vec<Choice<Option<String>>> {
        let mut choices = vec![Choice::new(None, "(Default)")];
        choices.extend(game.card_backs.iter().map(|back| Choice::new(Some(back.id.clone()), back.name.clone())));
        choices.push(Choice::new(Some(UNIQUE_CARD_BACK_ID.to_string()), "Unique"));
        choices
    }

    fn set_choices(game: &GameDefinition) -> Vec<Choice<Option<String>>> {
        let mut choices = vec![Choice::new(None, "(none)")];
        choices.extend(game.sets.iter().map(|set| Choice::new(Some(set.id.clone()), set.name.clone())));
        choices
    }

    /// One collapsible chip section per non-empty tag group. `toggle(group name, tag, was_on)` is
    /// called for a clicked chip; `is_on(group name, tag)` decides each chip. Returns the new y.
    #[allow(clippy::too_many_arguments)]
    fn draw_tag_sections(
        &mut self,
        frame: &mut Frame,
        game: &GameDefinition,
        id_prefix: &str,
        is_on: &dyn Fn(&str, &str) -> bool,
        toggles: &mut Vec<(String, String, bool)>,
        x: f32,
        mut y: f32,
        width: f32,
    ) -> f32 {
        for group in game.tag_groups.iter().filter(|group| !group.tags.is_empty()) {
            y += GAP;
            let hidden = self.hidden_tag_groups.contains(&group.id);
            let on_count = group.tags.iter().filter(|tag| is_on(&group.name, tag)).count();
            let trailing = (on_count > 0).then(|| format!("{on_count} selected"));
            if frame.collapse_header(
                &format!("{id_prefix}:vis:{}", group.id),
                &group.name,
                x - 4.0,
                y,
                width + 4.0,
                32.0,
                !hidden,
                trailing.as_deref(),
            ) {
                if hidden {
                    self.hidden_tag_groups.remove(&group.id);
                } else {
                    self.hidden_tag_groups.insert(group.id.clone());
                }
            }
            y += 32.0;
            if !hidden {
                y += 6.0;
                let mut flow = Flow::new(x, y, width, CHIP_HEIGHT, 8.0);
                for (tag_index, tag) in group.tags.iter().enumerate() {
                    let selected = is_on(&group.name, tag);
                    let chip_width = frame.chip_width(tag, selected, false);
                    let (chip_x, chip_y) = flow.place(chip_width);
                    let chip_id = format!("{id_prefix}:tag:{}:{}", group.id, tag_index + 1);
                    let (clicked, _) = frame.chip(
                        tag,
                        chip_x,
                        chip_y,
                        selected,
                        &ChipOptions { id: Some(&chip_id), ..ChipOptions::default() },
                    );
                    if clicked {
                        toggles.push((group.name.clone(), tag.clone(), selected));
                    }
                }
                y = flow.bottom();
            }
        }
        y
    }

    fn apply_tag_toggles(card: &mut CardDefinition, toggles: &[(String, String, bool)]) {
        for (group, tag, was_on) in toggles {
            if *was_on {
                remove_tag(&mut card.types, group, tag);
            } else {
                card.types.push(encode_tag_entry(group, tag));
            }
        }
    }

    /// The bulk form for several selected cards: only fields that needn't be unique per card.
    /// Each control shows the first card's value; changing it sets it on every selected card.
    #[allow(clippy::too_many_arguments)]
    fn draw_bulk_form(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        selected: &[usize],
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let palette = frame.palette();
        let representative = model.game.cards()[selected[0]].clone();
        let offset = frame.begin_scroll("cv:bulkform", x, y, width, height);
        let padding = 16.0;
        let (form_x, form_width) = (x + padding, width - 2.0 * padding);
        let half_width = (form_width - GAP) / 2.0;
        let mut row_y = y + padding - offset;
        let options = TextOptions::new().width(form_width).middle_of(28.0).color(palette.on_surface_variant);
        frame.text(&format!("{} cards selected", selected.len()), form_x, row_y, options);
        let suggest_options = ButtonOptions::variant(ButtonVariant::Tonal).icon("auto_fix_high").id("cv:bulk:suggest");
        let suggest_width = frame.button_width("Suggest Titles...", &suggest_options);
        if frame.button(
            "Suggest Titles...",
            form_x + form_width - suggest_width,
            row_y - 6.0,
            Some(suggest_width),
            Some(40.0),
            &suggest_options,
        ) {
            let keys = selected.iter().map(|&index| model.card_keys.key(index)).collect();
            self.suggest_title = Some(SuggestTitleFlow::open(frame, None, keys));
        }
        row_y += 28.0 + GAP;

        let apply = |model: &mut EditorModel, change: &dyn Fn(&mut CardDefinition)| {
            let cards = model.game.cards_mut();
            for &index in selected {
                change(&mut cards[index]);
            }
        };

        let has_sets = !model.game.sets.is_empty();
        let options = DropdownOptions { label: Some("Orientation"), height: Some(44.0), ..DropdownOptions::default() };
        let orientation = frame.dropdown(
            "cv:bulk:orient",
            form_x,
            row_y,
            if has_sets { half_width } else { form_width },
            &orientation_choices(),
            representative.orientation,
            &options,
        );
        if orientation != representative.orientation {
            apply(model, &|card| card.orientation = orientation);
        }
        if has_sets {
            let options = DropdownOptions { label: Some("Set"), height: Some(44.0), ..DropdownOptions::default() };
            let set_id = frame.dropdown(
                "cv:bulk:set",
                form_x + half_width + GAP,
                row_y,
                half_width,
                &CardViewTab::set_choices(&model.game),
                representative.set_id.clone(),
                &options,
            );
            if set_id != representative.set_id {
                apply(model, &|card| card.set_id = set_id.clone());
            }
        }
        row_y += 44.0 + GAP;

        let mut unique = representative.card_back_id.as_deref() == Some(UNIQUE_CARD_BACK_ID);
        let options = DropdownOptions { label: Some("Card Back"), height: Some(44.0), ..DropdownOptions::default() };
        let back_id = frame.dropdown(
            "cv:bulk:back",
            form_x,
            row_y,
            if unique { half_width } else { form_width },
            &CardViewTab::back_choices(&model.game),
            representative.card_back_id.clone(),
            &options,
        );
        if back_id != representative.card_back_id {
            unique = back_id.as_deref() == Some(UNIQUE_CARD_BACK_ID);
            apply(model, &|card| card.card_back_id = back_id.clone());
        }
        if unique {
            let options =
                DropdownOptions { label: Some("Back Orientation"), height: Some(44.0), ..DropdownOptions::default() };
            let back_orientation = frame.dropdown(
                "cv:bulk:backorient",
                form_x + half_width + GAP,
                row_y,
                half_width,
                &orientation_choices(),
                representative.unique_back_orientation,
                &options,
            );
            if back_orientation != representative.unique_back_orientation {
                apply(model, &|card| card.unique_back_orientation = back_orientation);
            }
        }
        row_y += 44.0;

        let types = representative.types.clone();
        let is_on = |group: &str, tag: &str| card_has_tag(&types, group, tag);
        let mut toggles = Vec::new();
        let game = model.game.clone();
        row_y = self.draw_tag_sections(frame, &game, "cv:bulk", &is_on, &mut toggles, form_x, row_y, form_width);
        if !toggles.is_empty() {
            apply(model, &|card| CardViewTab::apply_tag_toggles(card, &toggles));
        }

        let (unownable, toggle_height) = frame.toggle_row(
            "cv:bulk:unownable",
            "Unownable",
            Some("Always stays unowned on the table -- any player can interact with it"),
            form_x,
            row_y,
            form_width,
            representative.unownable,
            false,
        );
        if unownable != representative.unownable {
            apply(model, &|card| card.unownable = unownable);
        }
        row_y += toggle_height + padding;
        let (free, toggle_height) = frame.toggle_row(
            "cv:bulk:freecard",
            "Free Card",
            Some("On collection-mode servers, every player always has unlimited copies"),
            form_x,
            row_y,
            form_width,
            representative.is_free_card,
            false,
        );
        if free != representative.is_free_card {
            apply(model, &|card| card.is_free_card = free);
        }
        row_y += toggle_height + padding;
        if model.game.has_scenarios {
            let (scenario, toggle_height) = frame.toggle_row(
                "cv:bulk:scenariocard",
                "Scenario Card",
                Some(
                    "Belongs to a scenario's card pool (Card Placement / Deck Builder) instead of the regular player \
                     card pool",
                ),
                form_x,
                row_y,
                form_width,
                representative.is_scenario_card,
                false,
            );
            if scenario != representative.is_scenario_card {
                apply(model, &|card| card.is_scenario_card = scenario);
            }
            row_y += toggle_height + padding;
        }
        frame.end_scroll("cv:bulkform", row_y + offset - y, &ScrollbarStyle::default());
    }

    /// Field options with this tab's Tab behaviour.
    fn field_options<'a>(&self, label: &'a str) -> TextFieldOptions<'a> {
        TextFieldOptions {
            height: Some(44.0),
            custom_tab: self.tab_navigates_inside_form,
            ..TextFieldOptions::label(label)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_form(
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
        let key = model.card_keys.key(index);
        let id = format!("cv:form:{key}");
        let selected_tab = if self.form_tab == FormTab::Extra { 1 } else { 0 };
        let picked = frame.tab_bar("cv:formtab", x, y, width, &["Main Data", "Extra Data"], selected_tab);
        self.form_tab = if picked == 1 { FormTab::Extra } else { FormTab::Main };
        let (y, height) = (y + TAB_BAR_HEIGHT, height - TAB_BAR_HEIGHT);
        if self.form_tab == FormTab::Extra {
            self.draw_extra_fields(frame, model, index, x, y, width, height);
            return;
        }

        let offset = frame.begin_scroll("cv:form:main", x, y, width, height);
        let padding = 16.0;
        let (form_x, form_width) = (x + padding, width - 2.0 * padding);
        let half_width = (form_width - GAP) / 2.0;
        let mut row_y = y + padding - offset;
        let mut tab_delta = None;

        // Title and delete. With the title option on, Tab in the title moves to the next card.
        let mut title_options = self.field_options("Card Title");
        title_options.custom_tab |= self.tab_focuses_title;
        let mut title = model.game.cards()[index].card_title.clone();
        let response =
            frame.text_field(&format!("{id}:title"), form_x, row_y, form_width - 92.0, &mut title, &title_options);
        if title != model.game.cards()[index].card_title {
            model.game.cards_mut()[index].card_title = title;
        }
        if let Some(shift) = response.tab {
            tab_delta = Some(if shift { -1 } else { 1 });
        }
        let suggest_id = format!("{id}:suggest");
        if frame.icon_button(
            "auto_fix_high",
            form_x + form_width - 84.0,
            row_y + 2.0,
            &IconButtonOptions::tooltip("Suggest Title...").id(&suggest_id),
        ) {
            frame.ui.blur();
            self.suggest_title = Some(SuggestTitleFlow::open(frame, Some(key), Vec::new()));
        }
        let delete_id = format!("{id}:delete");
        if frame.icon_button(
            "delete_outline",
            form_x + form_width - 40.0,
            row_y + 2.0,
            &IconButtonOptions::tooltip("Delete Card").id(&delete_id),
        ) {
            self.confirm_delete = Some(key);
        }
        row_y += 44.0 + GAP;

        let card = model.game.cards()[index].clone();
        let mut changed = card.clone();
        let has_sets = !model.game.sets.is_empty();
        let options = DropdownOptions { label: Some("Orientation"), height: Some(44.0), ..DropdownOptions::default() };
        changed.orientation = frame.dropdown(
            &format!("{id}:orient"),
            form_x,
            row_y,
            if has_sets { half_width } else { form_width },
            &orientation_choices(),
            card.orientation,
            &options,
        );
        if has_sets {
            let options = DropdownOptions { label: Some("Set"), height: Some(44.0), ..DropdownOptions::default() };
            changed.set_id = frame.dropdown(
                &format!("{id}:set"),
                form_x + half_width + GAP,
                row_y,
                half_width,
                &CardViewTab::set_choices(&model.game),
                card.set_id.clone(),
                &options,
            );
        }
        row_y += 44.0 + GAP;

        // Image: file name, Choose and Clear on one line.
        let choose_options = ButtonOptions::variant(ButtonVariant::Outlined);
        let choose_width = frame.button_width("Choose...", &choose_options);
        let name_width = form_width - choose_width - 4.0 - if card.image_path.is_some() { 44.0 } else { 0.0 };
        frame.outline_rect(form_x + 0.5, row_y + 0.5, name_width - 1.0, 43.0, 4.0, palette.outline, 1.0);
        let label_font = theme::font(12.0, Weight::Regular);
        let label_width = frame.fonts.width(label_font, "Image");
        let label_height = frame.fonts.height(label_font);
        frame.fill_rect(form_x + 8.0, row_y - 2.0, label_width + 8.0, 4.0, 0.0, palette.surface);
        frame.text(
            "Image",
            form_x + 12.0,
            row_y - label_height / 2.0,
            TextOptions::new().size(12.0).color(palette.on_surface_variant),
        );
        let color = if card.image_path.is_some() { palette.on_surface } else { palette.on_surface_variant };
        let options = TextOptions::new().width(name_width - 24.0).middle_of(44.0).ellipsis().color(color);
        frame.text(card.image_path.as_deref().unwrap_or("(no image)"), form_x + 12.0, row_y, options);
        let choose_id = format!("{id}:choose");
        let choose_options = ButtonOptions { id: Some(&choose_id), tooltip: Some("Choose Image..."), ..choose_options };
        if frame.button(
            "Choose...",
            form_x + name_width + 4.0,
            row_y + 2.0,
            Some(choose_width),
            Some(40.0),
            &choose_options,
        ) {
            let destination = match &card.set_id {
                None => model.folder_path.clone(),
                Some(set_id) => join_path(&[&model.folder_path, set_id]),
            };
            self.import = Some((FileImport::image(frame, destination, true), key));
        }
        if card.image_path.is_some() {
            let clear_id = format!("{id}:clear");
            if frame.icon_button(
                "close",
                form_x + form_width - 40.0,
                row_y + 2.0,
                &IconButtonOptions::tooltip("Clear Image").id(&clear_id),
            ) {
                changed.image_path = None;
            }
        }
        row_y += 44.0 + GAP;

        // Card back, plus its orientation when Unique.
        let unique = card.card_back_id.as_deref() == Some(UNIQUE_CARD_BACK_ID);
        let options = DropdownOptions { label: Some("Card Back"), height: Some(44.0), ..DropdownOptions::default() };
        changed.card_back_id = frame.dropdown(
            &format!("{id}:back"),
            form_x,
            row_y,
            if unique { half_width } else { form_width },
            &CardViewTab::back_choices(&model.game),
            card.card_back_id.clone(),
            &options,
        );
        if unique {
            let options =
                DropdownOptions { label: Some("Back Orientation"), height: Some(44.0), ..DropdownOptions::default() };
            changed.unique_back_orientation = frame.dropdown(
                &format!("{id}:backorient"),
                form_x + half_width + GAP,
                row_y,
                half_width,
                &orientation_choices(),
                card.unique_back_orientation,
                &options,
            );
        }
        row_y += 44.0;

        let types = card.types.clone();
        let is_on = |group: &str, tag: &str| card_has_tag(&types, group, tag);
        let mut toggles = Vec::new();
        let game = model.game.clone();
        row_y = self.draw_tag_sections(frame, &game, &id, &is_on, &mut toggles, form_x, row_y, form_width);
        CardViewTab::apply_tag_toggles(&mut changed, &toggles);

        let (unownable, toggle_height) = frame.toggle_row(
            &format!("{id}:unownable"),
            "Unownable",
            Some("Always stays unowned on the table -- any player can interact with it"),
            form_x,
            row_y,
            form_width,
            card.unownable,
            false,
        );
        changed.unownable = unownable;
        row_y += toggle_height + padding;
        let (free, toggle_height) = frame.toggle_row(
            &format!("{id}:freecard"),
            "Free Card",
            Some("On collection-mode servers, every player always has unlimited copies"),
            form_x,
            row_y,
            form_width,
            card.is_free_card,
            false,
        );
        changed.is_free_card = free;
        row_y += toggle_height + padding;
        if model.game.has_scenarios {
            let (scenario, toggle_height) = frame.toggle_row(
                &format!("{id}:scenariocard"),
                "Scenario Card",
                Some(
                    "Belongs to a scenario's card pool (Card Placement / Deck Builder) instead of the regular player \
                     card pool",
                ),
                form_x,
                row_y,
                form_width,
                card.is_scenario_card,
                false,
            );
            changed.is_scenario_card = scenario;
            row_y += toggle_height + padding;
        }
        if changed != card {
            model.game.cards_mut()[index] = changed;
        }
        frame.end_scroll("cv:form:main", row_y + offset - y, &ScrollbarStyle::default());
        if let Some(delta) = tab_delta {
            self.tab_to_card(frame, model, delta);
        }
    }

    /// The "Extra Data" tab: a key above a wrapping 3-line value box per extra field.
    #[allow(clippy::too_many_arguments)]
    fn draw_extra_fields(
        &mut self,
        frame: &mut Frame,
        model: &mut EditorModel,
        index: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let key = model.card_keys.key(index);
        let id = format!("cv:form:{key}");
        let offset = frame.begin_scroll("cv:form:extra", x, y, width, height);
        let padding = 16.0;
        let (form_x, form_width) = (x + padding, width - 2.0 * padding);
        let mut row_y = y + padding - offset;
        let state_id = format!("cv:extra:{key}");
        let current = model.game.cards()[index].extra_fields.clone();
        let mut extra = std::mem::take(frame.ui.state::<ExtraRows>(&state_id));
        if extra.source.as_ref() != Some(&current) {
            extra.rows = current.iter().map(|(field, value)| (field.clone(), value_text(value))).collect();
            extra.source = Some(current);
        }
        let value_height = frame.text_area_height(3, None);
        let mut tab_delta = None;
        let mut remove = None;
        let mut changed = false;
        for row_index in 0..extra.rows.len() {
            let row_id = format!("{id}:extra{}", row_index + 1);
            let (field, value) = &mut extra.rows[row_index];
            let mut options = self.field_options("Key");
            options.height = Some(44.0);
            let response = frame.text_field(&format!("{row_id}:k"), form_x, row_y, form_width - 48.0, field, &options);
            changed |= response.changed;
            if let Some(shift) = response.tab {
                tab_delta = Some(if shift { -1 } else { 1 });
            }
            let remove_id = format!("{row_id}:rm");
            if frame.icon_button(
                "close",
                form_x + form_width - 40.0,
                row_y + 2.0,
                &IconButtonOptions::tooltip("Remove Field").id(&remove_id),
            ) {
                remove = Some(row_index);
            }
            row_y += 44.0 + 8.0;
            let options =
                TextFieldOptions { custom_tab: self.tab_navigates_inside_form, ..TextFieldOptions::label("Value") };
            let response =
                frame.text_area(&format!("{row_id}:v"), form_x, row_y, form_width, value_height, value, &options);
            changed |= response.changed;
            if let Some(shift) = response.tab {
                tab_delta = Some(if shift { -1 } else { 1 });
            }
            row_y += value_height + 16.0;
        }
        if let Some(row_index) = remove {
            extra.rows.remove(row_index);
            changed = true;
        }
        let add_id = format!("{id}:addfield");
        let options = ButtonOptions::variant(ButtonVariant::Text).icon("add").id(&add_id);
        if frame.button("Add Field", form_x - 8.0, row_y, None, Some(40.0), &options) {
            let taken: HashSet<&str> = extra.rows.iter().map(|(field, _)| field.as_str()).collect();
            let mut new_key = "field".to_string();
            let mut suffix = 1;
            while taken.contains(new_key.as_str()) {
                new_key = format!("field{suffix}");
                suffix += 1;
            }
            extra.rows.push((new_key, String::new()));
            changed = true;
        }
        row_y += 40.0 + 12.0;
        if changed {
            let mut object = JsonObject::new();
            for (field, value) in &extra.rows {
                object.insert(field.clone(), value.clone());
            }
            model.game.cards_mut()[index].extra_fields = object.clone();
            extra.source = Some(object);
        }
        *frame.ui.state::<ExtraRows>(&state_id) = extra;
        frame.end_scroll("cv:form:extra", row_y + offset - y, &ScrollbarStyle::default());
        if let Some(delta) = tab_delta {
            self.tab_to_card(frame, model, delta);
        }
    }

    // -----------------------------------------------------------------------
    // Layout
    // -----------------------------------------------------------------------

    /// A draggable column divider; returns the horizontal drag this frame.
    fn split_handle(&mut self, frame: &mut Frame, id: &'static str, x: f32, y: f32, height: f32) -> f32 {
        let region = frame.ui.region(id, x, y, DIVIDER, height, RegionOptions::cursor("sizewe"));
        let color = frame.palette().outline_variant;
        frame.fill_rect(x + (DIVIDER / 2.0).floor(), y, 1.0, height, 0.0, color);
        let (mouse_x, _) = frame.ui.mouse();
        let mut delta = 0.0;
        if region.held {
            if let Some(last) = self.split_drag.get(id) {
                delta = mouse_x - last;
            }
            self.split_drag.insert(id, mouse_x);
            frame.ui.set_cursor("sizewe");
        } else {
            self.split_drag.remove(id);
        }
        delta
    }

    /// Development hook (`GAMETABLE_GAMEDEF_TAB=cards,select,suggest`).
    pub fn open_suggest_title_for_development(&mut self) {
        self.open_suggest_title_next_frame = true;
    }

    fn finish_dialogs(&mut self, frame: &mut Frame, model: &mut EditorModel) {
        if std::mem::take(&mut self.open_suggest_title_next_frame) {
            self.suggest_title = Some(SuggestTitleFlow::open(frame, self.selected, Vec::new()));
        }
        if let Some(flow) = &mut self.suggest_title
            && !flow.show(frame, model)
        {
            self.suggest_title = None;
        }
        if let Some((import, key)) = &mut self.import {
            let key = *key;
            match import.show(frame) {
                ImportResponse::Open => {}
                ImportResponse::Cancelled => self.import = None,
                ImportResponse::Imported(name) => {
                    self.import = None;
                    if let Some(index) = model.card_keys.index_of(key) {
                        model.game.cards_mut()[index].image_path = Some(name);
                    }
                }
            }
        }
        let Some(key) = self.confirm_delete else { return };
        let Some(index) = model.card_keys.index_of(key) else {
            self.confirm_delete = None;
            return;
        };
        let message =
            format!("Delete \"{}\"? This does not delete its image file.", model.game.cards()[index].card_title);
        match frame.confirm_dialog("deletecard", "Delete Card", &message, "Delete", false) {
            ConfirmResponse::Open => {}
            ConfirmResponse::Cancelled => self.confirm_delete = None,
            ConfirmResponse::Confirmed => {
                self.confirm_delete = None;
                self.delete_card(model, key);
            }
        }
    }

    pub fn draw(&mut self, frame: &mut Frame, model: &mut EditorModel, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        model.card_keys.sync(model.game.cards().len());
        self.sync_filters(&model.game);
        let live: HashSet<u64> = (0..model.game.cards().len()).map(|index| model.card_keys.key(index)).collect();
        self.selected_keys.retain(|key| live.contains(key));
        if self.anchor.is_some_and(|key| !live.contains(&key)) {
            self.anchor = None;
        }
        let selected = self.selected_indexes(model);
        self.selected = (selected.len() == 1).then(|| model.card_keys.key(selected[0]));
        let display_backs = display_card_backs(&model.folder_path, &model.game.card_backs);

        let leading =
            [LeadingButton { label: "Add Card", icon: Some("add"), id: "cv:add", variant: ButtonVariant::Tonal }];
        let game = model.game.clone();
        let (filters_bottom, leading_clicked) =
            frame.card_filters("cv", &mut self.filters, &game, x + 12.0, y, width - 24.0, true, &leading);
        // The search field shares its row with the card zoom control at the right.
        let search_width = width - 24.0 - CardZoom::WIDTH - 12.0;
        let mut row_y = frame.card_search(
            "cv",
            &mut self.filters,
            &game,
            x + 12.0,
            filters_bottom + 12.0,
            search_width,
            Some(palette.surface),
            None,
        );
        self.card_zoom.control(frame, "cv:zoom", x + width - 12.0 - CardZoom::WIDTH, filters_bottom + 12.0);
        if leading_clicked.is_some() {
            self.add_blank_card(model);
        }
        if row_y > y {
            row_y += 4.0;
        }
        let visible = self.visible(model);
        let selected = self.selected_indexes(model);
        let body_height = y + height - row_y;
        let usable = width - DIVIDER * 2.0;
        let pool_width = (usable * self.pool_fraction).floor();
        let preview_width = (usable * self.preview_fraction).floor();
        let form_width = usable - pool_width - preview_width;

        frame.ui.push_clip(x, row_y, pool_width, body_height);
        self.draw_pool(frame, model, &display_backs, &visible, x, row_y, pool_width, body_height);
        frame.ui.pop_clip();
        let first_divider_x = x + pool_width;
        let delta = self.split_handle(frame, "cv:split1", first_divider_x, row_y, body_height);
        if delta != 0.0 {
            let change =
                (delta / usable).clamp(MIN_FRACTION - self.pool_fraction, self.preview_fraction - MIN_FRACTION);
            self.pool_fraction += change;
            self.preview_fraction -= change;
        }
        let preview_x = first_divider_x + DIVIDER;
        frame.ui.push_clip(preview_x, row_y, preview_width, body_height);
        self.draw_preview(
            frame,
            model,
            &display_backs,
            &visible,
            &selected,
            preview_x,
            row_y,
            preview_width,
            body_height,
        );
        frame.ui.pop_clip();
        let second_divider_x = preview_x + preview_width;
        let delta = self.split_handle(frame, "cv:split2", second_divider_x, row_y, body_height);
        if delta != 0.0 {
            let form_fraction = 1.0 - self.pool_fraction - self.preview_fraction;
            let change = (delta / usable).clamp(MIN_FRACTION - self.preview_fraction, form_fraction - MIN_FRACTION);
            self.preview_fraction += change;
        }
        let form_x = second_divider_x + DIVIDER;
        frame.ui.push_clip(form_x, row_y, form_width, body_height);
        match selected.len() {
            1 => self.draw_form(frame, model, selected[0], form_x, row_y, form_width, body_height),
            0 => {
                let options = TextOptions::new()
                    .width(form_width)
                    .middle_of(body_height)
                    .centered()
                    .color(palette.on_surface_variant);
                frame.text("Select a card to edit", form_x, row_y, options);
            }
            _ => self.draw_bulk_form(frame, model, &selected, form_x, row_y, form_width, body_height),
        }
        frame.ui.pop_clip();
        self.finish_dialogs(frame, model);
    }
}
