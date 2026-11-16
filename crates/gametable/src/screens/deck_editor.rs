//! The deck editor: pick a game, then build a deck from its cards (left-click a card to add a
//! copy, right-click to remove one) and save or open it in the deck library. Port of
//! `src/ui/screens/deck_editor.lua` (`deck_editor_game_select_screen.dart`,
//! `deck_editor_screen.dart`). A deck here starts empty.

use std::collections::HashMap;
use std::f32::consts::PI;
use std::rc::Rc;

use gametable_core::models::{DeckConfig, DeckEntry, GameDefinition, SubDeck};

use crate::app::{Frame, Screen, ScreenCommand};
use crate::data::deck_store::{DeckStore, LocalDeckStore};
use crate::graphics::draw_list::Color;
use crate::graphics::tessellation::DrawMode;
use crate::screens::deck_library_dialog::{DeckLibraryDialog, LoadDeckResponse, SaveDeckDialog, SaveDeckResponse};
use crate::screens::game_picker::GamePicker;
use crate::screens::rulebook_viewer::RulebookViewer;
use crate::table::card_pool_grid::{NAME_LINES, TILE_CARD_HEIGHT, TILE_CARD_WIDTH, TILE_SCALE};
use crate::table::card_render::{CardDraw, definition_faces, orientation_turns};
use crate::ui::card_filter_bar::CardFilterState;
use crate::ui::components::{EDITOR_TOOLBAR_HEIGHT, ToolbarAction};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::dropdown::DropdownOptions;
use crate::ui::theme::{Weight, fixed, with_alpha};
use crate::ui::widgets::{Choice, ScrollbarStyle};

/// The deck list's width on the right.
const DECK_PANEL_WIDTH: f32 = 240.0;
const DECK_ROW_HEIGHT: f32 = 48.0;

/// One deck the game builds: a distinct `deckType` of its deck-building zones.
#[derive(Debug, Clone, PartialEq)]
pub struct DeckSlot {
    pub deck_type: String,
    /// The first zone using this deck type names it.
    pub label: String,
    /// Required unless every zone using it is `deckOptional`.
    pub required: bool,
}

/// `buildSlots(game)`: one slot per distinct deck type, or a single implicit `main_deck`.
pub fn build_slots(game: &GameDefinition) -> Vec<DeckSlot> {
    let mut slots: Vec<DeckSlot> = Vec::new();
    for zone in game.deck_building_zones() {
        match slots.iter_mut().find(|slot| slot.deck_type == zone.deck_type) {
            Some(slot) => slot.required = slot.required || !zone.deck_optional,
            None => slots.push(DeckSlot {
                deck_type: zone.deck_type.clone(),
                label: zone.name.clone(),
                required: !zone.deck_optional,
            }),
        }
    }
    if slots.is_empty() {
        slots.push(DeckSlot { deck_type: "main_deck".to_string(), label: "Deck".to_string(), required: true });
    }
    slots
}

/// Copies per card in the order they were first added (what the list shows and the file keeps).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeckQuantities {
    pub order: Vec<String>,
    pub quantity: HashMap<String, i64>,
}

impl DeckQuantities {
    pub fn total(&self) -> i64 {
        self.order.iter().map(|id| self.quantity[id]).sum()
    }

    fn add(&mut self, id: &str) {
        if !self.quantity.contains_key(id) {
            self.order.push(id.to_string());
        }
        *self.quantity.entry(id.to_string()).or_insert(0) += 1;
    }

    fn remove_one(&mut self, id: &str) {
        let Some(current) = self.quantity.get(id).copied() else { return };
        if current <= 1 {
            self.quantity.remove(id);
            self.order.retain(|existing| existing != id);
        } else {
            self.quantity.insert(id.to_string(), current - 1);
        }
    }
}

/// The deck being built, separate from drawing so it can be tested.
pub struct DeckBuilder {
    pub game: Rc<GameDefinition>,
    pub slots: Vec<DeckSlot>,
    pub quantities_by_slot: HashMap<String, DeckQuantities>,
    pub active_deck_type: String,
    /// Collection mode: definition id -> copies owned.
    pub owned_quantities: Option<HashMap<String, i64>>,
    pub is_starter: bool,
}

impl DeckBuilder {
    pub fn new(game: Rc<GameDefinition>, owned_quantities: Option<HashMap<String, i64>>) -> DeckBuilder {
        let slots = build_slots(&game);
        let quantities_by_slot = slots.iter().map(|slot| (slot.deck_type.clone(), DeckQuantities::default())).collect();
        let active_deck_type = slots[0].deck_type.clone();
        DeckBuilder { game, slots, quantities_by_slot, active_deck_type, owned_quantities, is_starter: false }
    }

    pub fn active(&self) -> &DeckQuantities {
        &self.quantities_by_slot[&self.active_deck_type]
    }

    /// `E:copiesInDeck(id)`: copies across every slot (collection limits are per deck).
    pub fn copies_in_deck(&self, id: &str) -> i64 {
        self.quantities_by_slot.values().map(|quantities| quantities.quantity.get(id).copied().unwrap_or(0)).sum()
    }

    /// `E:ownedLimit(id)`: copies a collection allows, or `None` when unlimited.
    pub fn owned_limit(&self, id: &str) -> Option<i64> {
        let owned = self.owned_quantities.as_ref()?;
        if self.game.card_by_id(id).is_some_and(|card| card.unownable || card.is_free_card) {
            return None;
        }
        Some(owned.get(id).copied().unwrap_or(0))
    }

    /// `E:addCopy(id)`: returns the message to show when the collection has no more copies.
    pub fn add_copy(&mut self, id: &str) -> Result<(), String> {
        if let Some(limit) = self.owned_limit(id)
            && self.copies_in_deck(id) >= limit
        {
            return Err(format!("You only own {limit}{} of that card.", if limit == 1 { " copy" } else { " copies" }));
        }
        if let Some(quantities) = self.quantities_by_slot.get_mut(&self.active_deck_type) {
            quantities.add(id);
        }
        Ok(())
    }

    /// `E:removeCopy(id)`.
    pub fn remove_copy(&mut self, id: &str) {
        if let Some(quantities) = self.quantities_by_slot.get_mut(&self.active_deck_type) {
            quantities.remove_one(id);
        }
    }

    /// `E:buildDeckConfig()`.
    pub fn deck_config(&self) -> DeckConfig {
        let subdecks = self
            .slots
            .iter()
            .map(|slot| {
                let quantities = &self.quantities_by_slot[&slot.deck_type];
                let entries =
                    quantities.order.iter().map(|id| DeckEntry::new(id.clone(), quantities.quantity[id])).collect();
                SubDeck::new(slot.deck_type.clone(), entries)
            })
            .collect();
        let mut deck = DeckConfig::new(self.game.id.clone(), subdecks);
        deck.is_starter = self.is_starter;
        deck
    }

    /// The labels of required decks that are still empty (Save refuses until they have cards).
    pub fn missing_required(&self) -> Vec<String> {
        self.slots
            .iter()
            .filter(|slot| slot.required && self.quantities_by_slot[&slot.deck_type].total() == 0)
            .map(|slot| slot.label.clone())
            .collect()
    }

    /// `E:loadDeck(deck)`: replaces every slot with the deck's entries, skipping cards this game
    /// no longer has. Returns the messages to show.
    pub fn load_deck(&mut self, deck: &DeckConfig) -> Vec<String> {
        if deck.game_id != self.game.id {
            return vec![format!("That deck is for a different game ({}).", deck.game_id)];
        }
        for slot in &self.slots {
            let mut quantities = DeckQuantities::default();
            for entry in deck.entries_for(&slot.deck_type) {
                if self.game.card_by_id(&entry.definition_id).is_some() {
                    if !quantities.quantity.contains_key(&entry.definition_id) {
                        quantities.order.push(entry.definition_id.clone());
                    }
                    quantities.quantity.insert(entry.definition_id.clone(), entry.quantity);
                }
            }
            self.quantities_by_slot.insert(slot.deck_type.clone(), quantities);
        }
        self.is_starter = deck.is_starter;
        self.clamp_to_collection().into_iter().collect()
    }

    /// `E:clampToCollection()`: trims copies beyond what the collection owns (later slots give
    /// way first).
    fn clamp_to_collection(&mut self) -> Option<String> {
        self.owned_quantities.as_ref()?;
        let mut trimmed = 0;
        for slot_index in (0..self.slots.len()).rev() {
            let deck_type = self.slots[slot_index].deck_type.clone();
            let order = self.quantities_by_slot[&deck_type].order.clone();
            for id in order.iter().rev() {
                let Some(limit) = self.owned_limit(id) else { continue };
                let over = self.copies_in_deck(id) - limit;
                if over <= 0 {
                    continue;
                }
                let quantities = self.quantities_by_slot.get_mut(&deck_type).expect("slot");
                let cut = over.min(quantities.quantity[id]);
                trimmed += cut;
                let left = quantities.quantity[id] - cut;
                if left <= 0 {
                    quantities.quantity.remove(id);
                    quantities.order.retain(|existing| existing != id);
                } else {
                    quantities.quantity.insert(id.clone(), left);
                }
            }
        }
        (trimmed > 0).then(|| format!("Removed {trimmed} card(s) your collection doesn't have."))
    }

    /// `E:visibleCards()`: the filtered pool; in collection mode only owned, unownable and free
    /// cards.
    pub fn visible_card_ids(&self, filters: &CardFilterState) -> Vec<String> {
        filters
            .visible(&self.game)
            .into_iter()
            .filter(|card| {
                self.owned_quantities.as_ref().is_none_or(|owned| {
                    card.unownable || card.is_free_card || owned.get(&card.id).copied().unwrap_or(0) > 0
                })
            })
            .map(|card| card.id.clone())
            .collect()
    }
}

pub struct DeckEditor {
    builder: DeckBuilder,
    /// For a CardTable server's decks (Phase 10); the local library otherwise.
    store: Option<Box<dyn DeckStore>>,
    filters: CardFilterState,
    rulebook: RulebookViewer,
    hovered_id: Option<String>,
    open_dialog: Option<DeckLibraryDialog>,
    save_dialog: Option<SaveDeckDialog>,
}

impl DeckEditor {
    /// `E.new(game, opts)`: `store` and `owned_quantities` are for a CardTable server.
    pub fn new(
        game: GameDefinition,
        store: Option<Box<dyn DeckStore>>,
        owned_quantities: Option<HashMap<String, i64>>,
    ) -> DeckEditor {
        let filters = CardFilterState::new(&game);
        let rulebook = RulebookViewer::new(&game);
        DeckEditor {
            builder: DeckBuilder::new(Rc::new(game), owned_quantities),
            store,
            filters,
            rulebook,
            hovered_id: None,
            open_dialog: None,
            save_dialog: None,
        }
    }

    pub fn builder_mut(&mut self) -> &mut DeckBuilder {
        &mut self.builder
    }

    /// Development hook (`GAMETABLE_DECK_SETUP=add,rulebook`): adds one copy of the first five
    /// cards and a second of the first, and/or opens the documents viewer.
    pub fn apply_development_setup(&mut self, steps: &str) {
        for step in steps.split(',') {
            match step.trim() {
                "add" => {
                    let ids: Vec<String> =
                        self.builder.game.cards().iter().take(5).map(|card| card.id.clone()).collect();
                    for id in ids.iter().chain(ids.first()) {
                        let _ = self.builder.add_copy(id);
                    }
                }
                "rulebook" => self.rulebook.open_for_development(),
                _ => {}
            }
        }
    }

    fn take_store(&mut self, frame: &Frame) -> Box<dyn DeckStore> {
        self.store.take().unwrap_or_else(|| {
            Box::new(LocalDeckStore::new(frame.paths.decks_folder(frame.settings).to_string_lossy().into_owned()))
        })
    }

    fn with_store<R>(
        &mut self,
        frame: &mut Frame,
        use_store: impl FnOnce(&mut Self, &mut Frame, &mut dyn DeckStore) -> R,
    ) -> R {
        let was_own = self.store.is_some();
        let mut store = self.take_store(frame);
        let result = use_store(self, frame, store.as_mut());
        if was_own {
            self.store = Some(store);
        }
        result
    }

    /// `E:saveDeck()`.
    fn save_deck(&mut self, frame: &mut Frame) {
        let missing = self.builder.missing_required();
        if !missing.is_empty() {
            frame.ui.toast(&format!("Add at least one card to: {} before saving.", missing.join(", ")), None);
            return;
        }
        let deck = self.builder.deck_config();
        // Starter decks are a collection-mode server idea, so no checkbox when cards are owned.
        let show_starter = self.builder.owned_quantities.is_none();
        let game = self.builder.game.clone();
        let dialog =
            self.with_store(frame, |_, frame, store| SaveDeckDialog::new(frame, &game, deck, store, show_starter));
        self.save_dialog = Some(dialog);
    }

    /// `E:openDeck()`: the same deck-tile dialog as at the table.
    fn open_deck(&mut self, frame: &mut Frame) {
        let game = self.builder.game.clone();
        let owned = self.builder.owned_quantities.clone();
        let dialog =
            self.with_store(frame, |_, frame, store| DeckLibraryDialog::new(frame, game, Vec::new(), store, owned));
        self.open_dialog = Some(dialog);
    }

    fn toggle_rulebook(&mut self, frame: &mut Frame) {
        self.rulebook.toggle(frame);
    }

    fn badge(frame: &mut Frame, center_x: f32, center_y: f32, fill: Color, text: &str, text_color: Color) {
        frame.circle(DrawMode::Fill, center_x, center_y, 11.0, fill);
        let options = TextOptions::new().size(11.0).bold().width(22.0).middle_of(22.0).centered().color(text_color);
        frame.text(text, center_x - 11.0, center_y - 11.0, options);
    }

    /// `E:drawPool`.
    fn draw_pool(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let game = self.builder.game.clone();
        let (filters_bottom, _) =
            frame.card_filters("de", &mut self.filters, &game, x + 12.0, y, width - 24.0, false, &[]);
        let surface = frame.palette().surface;
        let row_y = frame.card_search(
            "de",
            &mut self.filters,
            &game,
            x + 12.0,
            filters_bottom + 12.0,
            width - 24.0,
            Some(surface),
            None,
        );
        let visible = self.builder.visible_card_ids(&self.filters);
        let (grid_y, grid_height) = (row_y, y + height - row_y);
        if visible.is_empty() && self.builder.owned_quantities.is_some() {
            let message = "Your collection has no cards matching these filters. Open packs from the server lobby's \
                           Collection screen.";
            let color = frame.palette().on_surface_variant;
            let options =
                TextOptions::new().width(width - 48.0).middle_of(grid_height.min(200.0)).centered().color(color);
            frame.text(message, x + 24.0, grid_y, options);
        }
        let mut added = None;
        let mut removed = None;
        let builder = &self.builder;
        let mut hovered = None;
        frame.card_pool_grid("de:pool", visible.len(), x, grid_y, width, grid_height, 12.0, |frame, tile| {
            let Some(definition) = game.card_by_id(&visible[tile.index]) else { return };
            let name_height = NAME_LINES * frame.fonts.height(crate::ui::theme::font(11.0, Weight::Regular));
            let region = frame.ui.region(
                &format!("de:card:{}", definition.id),
                tile.center_x - TILE_CARD_WIDTH / 2.0,
                tile.y,
                TILE_CARD_WIDTH,
                TILE_CARD_HEIGHT + 6.0 + name_height,
                RegionOptions::hand(),
            );
            // Collection mode: a green badge counts the owned copies not yet in the deck, and the
            // card dims once they're all used.
            let limit = builder.owned_limit(&definition.id);
            let unused = limit.map(|limit| (limit - builder.copies_in_deck(&definition.id)).max(0));
            let draw = CardDraw::new(
                Some(definition),
                true,
                tile.center_x,
                tile.y + TILE_CARD_HEIGHT / 2.0,
                &game.card_backs,
                None,
            )
            .scale(TILE_SCALE)
            .alpha(if unused == Some(0) { 0.35 } else { 1.0 })
            .angle(orientation_turns(definition.orientation) * 2.0 * PI);
            frame.draw_card(&draw);
            let in_deck = builder.active().quantity.get(&definition.id).copied();
            let count = in_deck.or(builder.owned_quantities.as_ref().map(|_| 0));
            if let Some(count) = count {
                let (badge_x, badge_y) = (tile.center_x + TILE_CARD_WIDTH / 2.0 + 6.0 - 11.0, tile.y - 6.0 + 11.0);
                DeckEditor::badge(
                    frame,
                    badge_x,
                    badge_y,
                    Color::rgba(0.0, 0.0, 0.0, 0.87),
                    &count.to_string(),
                    fixed::WHITE,
                );
            }
            if let Some(unused) = unused {
                let (badge_x, badge_y) = (tile.center_x - TILE_CARD_WIDTH / 2.0 - 6.0 + 11.0, tile.y - 6.0 + 11.0);
                DeckEditor::badge(frame, badge_x, badge_y, fixed::GREEN_DARK, &unused.to_string(), fixed::BLACK);
            }
            let label = match limit {
                Some(limit) => {
                    format!("{} ({}/{limit})", definition.card_title, builder.copies_in_deck(&definition.id))
                }
                None => definition.card_title.clone(),
            };
            let options = TextOptions::new().size(11.0).width(tile.width).centered().max_lines(NAME_LINES as usize);
            frame.text(&label, tile.x, tile.y + TILE_CARD_HEIGHT + 6.0, options);
            if region.hovered {
                hovered = Some(definition.id.clone());
            }
            if region.clicked {
                added = Some(definition.id.clone());
            }
            if region.right_clicked {
                removed = Some(definition.id.clone());
            }
        });
        if hovered.is_some() {
            self.hovered_id = hovered;
        }
        if let Some(id) = added
            && let Err(message) = self.builder.add_copy(&id)
        {
            frame.ui.toast(&message, None);
        }
        if let Some(id) = removed {
            self.builder.remove_copy(&id);
        }
    }

    /// `E:drawDeckPanel`.
    fn draw_deck_panel(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let mut row_y = y;
        let slot_label = |builder: &DeckBuilder, slot: &DeckSlot| {
            let count = builder.quantities_by_slot[&slot.deck_type].total();
            format!("{} -- {count}{}", slot.label, if count == 1 { " card" } else { " cards" })
        };
        if self.builder.slots.len() > 1 {
            // One dropdown picks the deck and shows its name and count.
            let choices: Vec<Choice<String>> = self
                .builder
                .slots
                .iter()
                .map(|slot| Choice::new(slot.deck_type.clone(), slot_label(&self.builder, slot)))
                .collect();
            let options = DropdownOptions { height: Some(40.0), size: Some(14.0), ..DropdownOptions::default() };
            let current = self.builder.active_deck_type.clone();
            self.builder.active_deck_type =
                frame.dropdown("de:slots", x + 12.0, row_y + 12.0, width - 24.0, &choices, current, &options);
            row_y += 12.0 + 40.0 + 12.0;
        } else {
            let label = slot_label(&self.builder, &self.builder.slots[0]);
            frame.text(&label, x + 12.0, row_y + 12.0, TextOptions::new().bold().width(width - 24.0).ellipsis());
            row_y += 12.0 + 20.0 + 12.0;
        }
        frame.divider(x, row_y, width, None);
        row_y += 1.0;
        let list_height = y + height - row_y;
        let active = self.builder.active().clone();
        if active.order.is_empty() {
            let options =
                TextOptions::new().width(width).middle_of(list_height).centered().color(palette.on_surface_variant);
            frame.text("Left-click a card to add it", x, row_y, options);
            return;
        }
        let offset = frame.begin_scroll("de:deck", x, row_y, width, list_height);
        let mut remove = None;
        for (index, id) in active.order.iter().enumerate() {
            let item_y = row_y + index as f32 * DECK_ROW_HEIGHT - offset;
            if item_y + DECK_ROW_HEIGHT > row_y && item_y < row_y + list_height {
                let region =
                    frame.ui.region(&format!("de:row:{id}"), x, item_y, width, DECK_ROW_HEIGHT, RegionOptions::hand());
                if region.hovered {
                    frame.fill_rect(x, item_y, width, DECK_ROW_HEIGHT, 0.0, with_alpha(palette.on_surface, 0.08));
                    self.hovered_id = Some(id.clone());
                }
                let trailing = format!("×{}", active.quantity[id]);
                let trailing_width = frame.text_width(&trailing, 14.0, Weight::Regular);
                let title =
                    self.builder.game.card_by_id(id).map_or(id.as_str(), |card| card.card_title.as_str()).to_string();
                let options =
                    TextOptions::new().middle_of(DECK_ROW_HEIGHT).width(width - 48.0 - trailing_width).ellipsis();
                frame.text(&title, x + 16.0, item_y, options);
                let options = TextOptions::new()
                    .width(width - 16.0)
                    .middle_of(DECK_ROW_HEIGHT)
                    .right()
                    .color(palette.on_surface_variant);
                frame.text(&trailing, x, item_y, options);
                if region.clicked {
                    remove = Some(id.clone());
                }
            }
        }
        frame.end_scroll("de:deck", active.order.len() as f32 * DECK_ROW_HEIGHT, &ScrollbarStyle::default());
        if let Some(id) = remove {
            self.builder.remove_copy(&id);
        }
    }

    /// `E:drawPreview`: Space shows the hovered card big, away from the mouse.
    fn draw_preview(&self, frame: &mut Frame) {
        if frame.ui.any_overlay_open() || frame.ui.focus.is_some() || !frame.keyboard.is_down(&["space"]) {
            return;
        }
        let game = &self.builder.game;
        let Some(definition) = self.hovered_id.as_deref().and_then(|id| game.card_by_id(id)) else { return };
        let (screen_width, screen_height) = frame.ui.screen_size();
        let (mouse_x, _) = frame.ui.mouse();
        let faces = definition_faces(definition, &game.card_backs, None);
        frame.draw_card_preview(&faces, &game.card_backs, screen_width, screen_height, mouse_x, None);
    }

    fn show_dialogs(&mut self, frame: &mut Frame) {
        if let Some(mut dialog) = self.open_dialog.take() {
            let response = self.with_store(frame, |_, frame, store| dialog.show(frame, store));
            match response {
                LoadDeckResponse::Open => self.open_dialog = Some(dialog),
                LoadDeckResponse::Chosen(deck) => {
                    for message in self.builder.load_deck(&deck) {
                        frame.ui.toast(&message, None);
                    }
                }
                LoadDeckResponse::Closed => {}
            }
        }
        if let Some(mut dialog) = self.save_dialog.take() {
            let game = self.builder.game.clone();
            let response = self.with_store(frame, |_, frame, store| dialog.show(frame, &game, store));
            match response {
                SaveDeckResponse::Open => self.save_dialog = Some(dialog),
                SaveDeckResponse::Saved { .. } => self.builder.is_starter = dialog.deck().is_starter,
                SaveDeckResponse::Closed => {}
            }
        }
    }
}

impl Screen for DeckEditor {
    fn title(&self) -> String {
        "Deck Editor".to_string()
    }

    fn key_pressed(&mut self, frame: &mut Frame, key: &'static str, is_repeat: bool) {
        if self.rulebook.is_open() {
            if key == "r" && frame.keyboard.ctrl() && !is_repeat {
                self.toggle_rulebook(frame);
            }
            return;
        }
        if !is_repeat && key == "r" && frame.keyboard.ctrl() {
            self.toggle_rulebook(frame);
        }
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        self.hovered_id = None;
        let title = format!("Edit Deck -- {}", self.builder.game.name);
        let actions = [
            ToolbarAction { icon: "folder_open", tooltip: "Open Deck...", id: Some("de:open"), disabled: false },
            ToolbarAction { icon: "save", tooltip: "Save Deck...", id: Some("de:save"), disabled: false },
        ];
        let clicked = frame.editor_toolbar(x, y, width, &title, &actions);
        let (body_y, body_height) = (y + EDITOR_TOOLBAR_HEIGHT + 1.0, height - EDITOR_TOOLBAR_HEIGHT - 1.0);
        // The deck list on the right; the pool takes the rest.
        let pool_width = width - DECK_PANEL_WIDTH - 1.0;
        frame.ui.push_clip(x, body_y, pool_width, body_height);
        self.draw_pool(frame, x, body_y, pool_width, body_height);
        frame.ui.pop_clip();
        frame.fill_rect(x + pool_width, body_y, 1.0, body_height, 0.0, palette.outline_variant);
        frame.ui.push_clip(x + pool_width + 1.0, body_y, DECK_PANEL_WIDTH, body_height);
        self.draw_deck_panel(frame, x + pool_width + 1.0, body_y, DECK_PANEL_WIDTH, body_height);
        frame.ui.pop_clip();
        frame.overlay(|frame| self.draw_preview(frame));
        if self.rulebook.is_open() {
            let close = frame.overlay(|frame| self.rulebook.draw(frame, None));
            if close {
                self.toggle_rulebook(frame);
            }
        }
        match clicked {
            Some(0) => self.open_deck(frame),
            Some(1) => self.save_deck(frame),
            _ => {}
        }
        self.show_dialogs(frame);
    }
}

/// `DeckEditorGameSelectScreen`: pick the game to build a deck for (not the Standard 52).
pub struct DeckEditorGameSelect {
    picker: GamePicker,
}

impl DeckEditorGameSelect {
    pub fn new(frame: &Frame) -> DeckEditorGameSelect {
        DeckEditorGameSelect { picker: GamePicker::new(frame, true) }
    }
}

impl Screen for DeckEditorGameSelect {
    fn title(&self) -> String {
        "Deck Editor".to_string()
    }

    fn update(&mut self, _frame: &mut Frame, _dt: f32) {
        self.picker.update();
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        if let Some(game) = self.picker.draw(frame, x, y, width, height) {
            frame.commands.push(ScreenCommand::Push(Box::new(DeckEditor::new(game, None, None))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gametable_core::models::{CardDefinition, ZoneDefinition};

    fn game() -> GameDefinition {
        let mut game = GameDefinition::new("g", "Game");
        for id in ["a", "b", "c"] {
            game.cards_mut().push(CardDefinition::new(id, id.to_uppercase()));
        }
        let mut free = CardDefinition::new("free", "Free");
        free.is_free_card = true;
        game.cards_mut().push(free);
        let zone = |id: &str, name: &str, deck_type: &str, optional: bool| {
            let mut zone = ZoneDefinition::new(id, name);
            zone.deals_built_deck = true;
            zone.deck_type = deck_type.to_string();
            zone.deck_optional = optional;
            zone
        };
        game.zones = vec![
            zone("draw", "Draw Pile", "main_deck", false),
            zone("side", "Sideboard", "side", true),
            zone("draw2", "Second Pile", "main_deck", true),
        ];
        game
    }

    #[test]
    fn slots_follow_the_deck_building_zones() {
        let slots = build_slots(&game());
        assert_eq!(
            slots,
            vec![
                DeckSlot { deck_type: "main_deck".into(), label: "Draw Pile".into(), required: true },
                DeckSlot { deck_type: "side".into(), label: "Sideboard".into(), required: false },
            ]
        );
        let none = build_slots(&GameDefinition::new("x", "X"));
        assert_eq!(none, vec![DeckSlot { deck_type: "main_deck".into(), label: "Deck".into(), required: true }]);
    }

    #[test]
    fn adding_removing_saving_and_loading() {
        let mut builder = DeckBuilder::new(Rc::new(game()), None);
        assert_eq!(builder.missing_required(), ["Draw Pile"]);
        builder.add_copy("b").unwrap();
        builder.add_copy("a").unwrap();
        builder.add_copy("b").unwrap();
        builder.active_deck_type = "side".into();
        builder.add_copy("c").unwrap();
        builder.remove_copy("c");
        builder.remove_copy("zzz");
        assert!(builder.missing_required().is_empty());
        let deck = builder.deck_config();
        assert_eq!(
            gametable_core::json::encode_compact(&deck.to_json()).unwrap(),
            r#"{"gameId":"g","subdecks":[{"name":"main_deck","entries":[{"definitionId":"b","quantity":2},{"definitionId":"a","quantity":1}]},{"name":"side","entries":[]}]}"#
        );

        let mut other = DeckBuilder::new(Rc::new(game()), None);
        let mut with_missing = deck.clone();
        with_missing.subdecks[0].entries.push(DeckEntry::new("gone", 3));
        assert!(other.load_deck(&with_missing).is_empty());
        assert_eq!(other.quantities_by_slot["main_deck"].order, ["b", "a"]);
        let mut foreign = deck;
        foreign.game_id = "other".into();
        assert_eq!(other.load_deck(&foreign), ["That deck is for a different game (other)."]);
    }

    #[test]
    fn collection_limits() {
        let owned: HashMap<String, i64> = [("a".to_string(), 1), ("b".to_string(), 2)].into_iter().collect();
        let mut builder = DeckBuilder::new(Rc::new(game()), Some(owned));
        builder.add_copy("a").unwrap();
        assert_eq!(builder.add_copy("a"), Err("You only own 1 copy of that card.".to_string()));
        assert_eq!(builder.owned_limit("free"), None);
        builder.add_copy("free").unwrap();
        builder.add_copy("free").unwrap();
        assert_eq!(builder.add_copy("c"), Err("You only own 0 copies of that card.".to_string()));
        let filters = CardFilterState::new(&builder.game);
        assert_eq!(builder.visible_card_ids(&filters), ["a", "b", "free"]);

        let mut deck = DeckConfig::new(
            "g",
            vec![
                SubDeck::new("main_deck", vec![DeckEntry::new("b", 2)]),
                SubDeck::new("side", vec![DeckEntry::new("b", 3)]),
            ],
        );
        deck.is_starter = true;
        assert_eq!(builder.load_deck(&deck), ["Removed 3 card(s) your collection doesn't have."]);
        assert_eq!(builder.quantities_by_slot["main_deck"].quantity["b"], 2);
        assert!(builder.quantities_by_slot["side"].order.is_empty());
        assert!(builder.is_starter);
    }
}
