//! The Load Deck dialog (the player's own and built-in decks for a game, checked against the
//! zones being loaded) and the Save Deck prompt. Port of `src/ui/screens/deck_library_dialog.lua`
//! (`widgets/deck_library_screen.dart`, `save_deck_dialog.dart`).
//!
//! Decks come through a [`DeckStore`]; results arrive through callbacks (straight away for the
//! local folder, later for a server), so the dialogs keep them in shared slots and pick them up
//! on the next frame.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use gametable_core::models::{DeckConfig, GameDefinition, ModelError, ZoneDefinition};

use crate::app::Frame;
use crate::data::deck_library::{
    DeckReadError, DeckScan, LibraryDeck, deck_exists, load_decks_for_game, read_deck_file,
};
use crate::data::deck_store::{DeckStore, DeckStoreKind};
use crate::data::file_system::{base_name, remove_tree};
use crate::data::paths::set_decks_folder;
use crate::screens::file_picker::{FilePicker, FilePickerResponse, JSON_FILTERS, PickerMode};
use crate::screens::game_picker::{MoveLibraryPrompt, MovePromptResponse};
use crate::sound::SoundEffect;
use crate::table::card_render::{CARD_HEIGHT, CARD_WIDTH, CardDraw};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{ConfirmResponse, ContextMenu, DialogResponse, DialogSpec, MenuItem, MenuResponse};
use crate::ui::save_name_dialog::{ExistingName, SaveNameDialog, SaveNameResponse};
use crate::ui::theme::{Weight, fixed, with_alpha};
use crate::ui::widgets::{ButtonOptions, IconButtonOptions, ScrollbarStyle, ToggleOptions};

const TILE_SCALE: f32 = 4.0 / 3.0;
const NAME_LINES: usize = 3;
const TILE_GAP: f32 = 12.0;
const DIALOG_WIDTH: f32 = 760.0;
const DIALOG_CONTENT_HEIGHT: f32 = 560.0 - 48.0;

type ListSlot = Rc<RefCell<Option<(DeckScan, Option<String>)>>>;
type DoneSlot = Rc<RefCell<Option<(bool, Option<String>)>>>;

/// What the Load Deck dialog reports.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadDeckResponse {
    Open,
    Chosen(DeckConfig),
    Closed,
}

/// The in-game "Load Deck" dialog (`D.open(game, zones, onDeckChosen, opts)`).
pub struct DeckLibraryDialog {
    game: Rc<GameDefinition>,
    zones: Vec<ZoneDefinition>,
    remote: bool,
    store_label: String,
    /// Collection mode: definition id -> copies owned. Built-in starters are hidden and decks
    /// the collection can't build are greyed out.
    owned_quantities: Option<HashMap<String, i64>>,
    directory_path: String,
    busy: bool,
    scan: DeckScan,
    folder_error: Option<String>,
    selection_error: Option<String>,
    chosen_name: Option<String>,
    pending_list: ListSlot,
    pending_delete: DoneSlot,
    deleting_name: Option<String>,
    deck_menu: Option<(ContextMenu<()>, LibraryDeck)>,
    confirm_delete: Option<LibraryDeck>,
    browse_picker: Option<FilePicker>,
    folder_picker: Option<FilePicker>,
    move_prompt: Option<MoveLibraryPrompt>,
}

impl DeckLibraryDialog {
    pub fn new(
        frame: &mut Frame,
        game: Rc<GameDefinition>,
        zones: Vec<ZoneDefinition>,
        store: &mut dyn DeckStore,
        owned_quantities: Option<HashMap<String, i64>>,
    ) -> DeckLibraryDialog {
        let remote = store.kind() == DeckStoreKind::Server;
        let directory_path = if remote {
            store.label()
        } else {
            frame.paths.decks_folder(frame.settings).to_string_lossy().into_owned()
        };
        let mut dialog = DeckLibraryDialog {
            game,
            zones,
            remote,
            store_label: store.label(),
            owned_quantities,
            directory_path,
            busy: true,
            scan: DeckScan::default(),
            folder_error: None,
            selection_error: None,
            chosen_name: None,
            pending_list: Rc::new(RefCell::new(None)),
            pending_delete: Rc::new(RefCell::new(None)),
            deleting_name: None,
            deck_menu: None,
            confirm_delete: None,
            browse_picker: None,
            folder_picker: None,
            move_prompt: None,
        };
        dialog.refresh(frame, store);
        dialog
    }

    /// Every card the deck needs owned in full (unownable and free cards always count).
    fn is_affordable(&self, deck: &DeckConfig) -> bool {
        let Some(owned) = &self.owned_quantities else { return true };
        let mut needed: HashMap<&str, i64> = HashMap::new();
        for entry in deck.subdecks.iter().flat_map(|subdeck| &subdeck.entries) {
            *needed.entry(&entry.definition_id).or_insert(0) += entry.quantity;
        }
        needed.into_iter().all(|(id, quantity)| {
            let free = self.game.card_by_id(id).is_some_and(|card| card.unownable || card.is_free_card);
            free || owned.get(id).copied().unwrap_or(0) >= quantity
        })
    }

    fn filter_scan(&self, mut scan: DeckScan) -> DeckScan {
        if self.owned_quantities.is_some() {
            scan.entries.retain(|entry| !(entry.is_built_in && entry.deck.is_starter));
        }
        scan
    }

    fn refresh(&mut self, frame: &mut Frame, store: &mut dyn DeckStore) {
        self.busy = true;
        self.folder_error = None;
        if self.remote {
            let slot = self.pending_list.clone();
            store.list(&self.game, Box::new(move |scan, error| *slot.borrow_mut() = Some((scan, error))));
        } else {
            // The local library is listed from the folder this dialog shows.
            let mut local = crate::data::deck_store::LocalDeckStore::new(self.directory_path.clone());
            let slot = self.pending_list.clone();
            local.list(&self.game, Box::new(move |scan, error| *slot.borrow_mut() = Some((scan, error))));
        }
        self.collect_list(frame);
    }

    /// Picks up a finished listing.
    fn collect_list(&mut self, frame: &mut Frame) {
        let Some((scan, error)) = self.pending_list.borrow_mut().take() else { return };
        let scan = self.filter_scan(scan);
        let empty = scan.entries.is_empty() && scan.incompatible.is_empty();
        self.folder_error = match error {
            Some(error) if self.remote => Some(error),
            Some(error) => Some(format!("Could not read that folder: {error}")),
            None if empty && self.remote => {
                Some(format!("You haven't saved any decks for {} on this server yet.", self.game.name))
            }
            None if empty => Some(format!("No decks found for {} in that folder.", self.game.name)),
            None => None,
        };
        self.scan = scan;
        if self.folder_error.is_some() {
            frame.sounds.play(SoundEffect::Error);
        }
        self.busy = false;
    }

    /// Zones the deck leaves empty that need cards.
    fn missing_required(&self, deck: &DeckConfig) -> Vec<String> {
        self.zones
            .iter()
            .filter(|zone| !zone.deck_optional && deck.card_count_for(&zone.deck_type) == 0)
            .map(|zone| zone.name.clone())
            .collect()
    }

    fn select_deck(&mut self, frame: &mut Frame, deck: &DeckConfig, display_name: &str) -> Option<DeckConfig> {
        let missing = self.missing_required(deck);
        if !missing.is_empty() {
            frame.sounds.play(SoundEffect::Error);
            self.selection_error = Some(format!("That deck is missing required cards for: {}.", missing.join(", ")));
            return None;
        }
        self.selection_error = None;
        self.chosen_name = Some(display_name.to_string());
        Some(deck.clone())
    }

    fn browse_result(&mut self, frame: &mut Frame, path: &str) -> Option<DeckConfig> {
        match read_deck_file(path) {
            Err(reason) => {
                frame.sounds.play(SoundEffect::Error);
                self.selection_error = Some(if reason == DeckReadError::Legacy {
                    ModelError::LEGACY_DECK_FORMAT_MESSAGE.to_string()
                } else {
                    "That file is not a valid deck.".to_string()
                });
                None
            }
            Ok(deck) if deck.game_id != self.game.id => {
                frame.sounds.play(SoundEffect::Error);
                self.selection_error = Some(format!("That deck is for a different game ({}).", deck.game_id));
                None
            }
            Ok(deck) => self.select_deck(frame, &deck, base_name(path)),
        }
    }

    fn finish_delete(&mut self, frame: &mut Frame, store: &mut dyn DeckStore, ok: bool, error: Option<String>) {
        let name = self.deleting_name.take().unwrap_or_default();
        if !ok {
            frame.sounds.play(SoundEffect::Error);
            let message = error.unwrap_or_else(|| format!("Could not delete \"{name}\"."));
            frame.ui.alert("Couldn't Delete Deck", &message);
            return;
        }
        if self.chosen_name.as_deref() == Some(name.as_str()) {
            self.chosen_name = None;
        }
        self.selection_error = None;
        frame.ui.toast(&format!("Deleted \"{name}\"."), None);
        self.refresh(frame, store);
    }

    fn delete_deck(&mut self, frame: &mut Frame, store: &mut dyn DeckStore, entry: &LibraryDeck) {
        self.deleting_name = Some(entry.display_name.clone());
        if self.remote {
            let slot = self.pending_delete.clone();
            store.remove(
                &self.game,
                &entry.display_name,
                Some(Box::new(move |ok, error| *slot.borrow_mut() = Some((ok, error)))),
            );
        } else {
            let ok = entry.file_path.as_deref().is_some_and(|path| remove_tree(Path::new(path)).is_ok());
            *self.pending_delete.borrow_mut() = Some((ok, None));
        }
        let finished = self.pending_delete.borrow_mut().take();
        if let Some((ok, error)) = finished {
            self.finish_delete(frame, store, ok, error);
        }
    }

    /// Shows the dialog and anything it has open.
    pub fn show(&mut self, frame: &mut Frame, store: &mut dyn DeckStore) -> LoadDeckResponse {
        self.collect_list(frame);
        let finished_delete = self.pending_delete.borrow_mut().take();
        if let Some((ok, error)) = finished_delete {
            self.finish_delete(frame, store, ok, error);
        }
        let mut chosen: Option<DeckConfig> = None;
        let mut refresh = false;
        let mut open_folder_picker = false;
        let mut open_browse = false;
        let mut menu_for: Option<LibraryDeck> = None;
        let spec = DialogSpec { width: DIALOG_WIDTH, actions: &[], ..DialogSpec::default() };
        let game = self.game.clone();
        let response = frame.dialog("decklibrary", &spec, |frame, x, y, width| {
            let palette = frame.palette();
            let title_options = TextOptions::new().bold().width(width).centered();
            frame.text(&format!("Load your deck for {}", game.name), x, y, title_options);
            let mut row_y = y + 32.0;
            let path_options =
                TextOptions::new().size(12.0).color(palette.on_surface_variant).width(width - 96.0).middle_of(40.0).ellipsis();
            frame.text(&self.directory_path, x, row_y, path_options);
            let refresh_options = IconButtonOptions::tooltip("Refresh").id("dl:refresh").disabled(self.busy);
            refresh |= frame.icon_button("refresh", x + width - 88.0, row_y, &refresh_options);
            if !self.remote {
                let options = IconButtonOptions::tooltip("Choose Deck Library Folder...").id("dl:folder").disabled(self.busy);
                open_folder_picker |= frame.icon_button("folder_open", x + width - 44.0, row_y, &options);
            }
            row_y += 52.0;
            frame.divider(x, row_y, width, None);
            row_y += 12.0;
            let body_height = y + DIALOG_CONTENT_HEIGHT - row_y;
            let grid_width = width - 240.0 - 24.0;
            let name_line_height = frame.fonts.height(crate::ui::theme::font(11.0, Weight::Regular));
            let name_height = NAME_LINES as f32 * name_line_height;
            let (card_width, card_height) = (CARD_WIDTH * TILE_SCALE, CARD_HEIGHT * TILE_SCALE);
            let tile_height = 4.0 + card_height + 4.0 + name_height + 2.0 + 28.0
                + if self.owned_quantities.is_some() { 13.0 } else { 0.0 };
            if self.busy && self.remote {
                frame.spinner(x + grid_width / 2.0, row_y + body_height / 2.0, 14.0, None);
            } else if let Some(error) = &self.folder_error {
                let options = TextOptions::new().width(grid_width).middle_of(body_height).centered().color(fixed::RED);
                frame.text(error, x, row_y, options);
            } else {
                let columns = (((grid_width + TILE_GAP) / (card_width + 24.0 + TILE_GAP)).ceil() as usize).max(1);
                let tile_width = (grid_width - (columns as f32 - 1.0) * TILE_GAP) / columns as f32;
                let entries = self.scan.entries.clone();
                let incompatible = self.scan.incompatible.clone();
                let total = entries.len() + incompatible.len();
                let offset = frame.begin_scroll("dl:grid", x, row_y, grid_width, body_height);
                for index in 0..total {
                    let (column, row) = (index % columns, index / columns);
                    let tile_x = x + column as f32 * (tile_width + TILE_GAP);
                    let tile_y = row_y + 4.0 + row as f32 * (tile_height + TILE_GAP) - offset;
                    let is_incompatible = index >= entries.len();
                    let (display_name, deck) = if is_incompatible {
                        (incompatible[index - entries.len()].display_name.clone(), None)
                    } else {
                        (entries[index].display_name.clone(), Some(&entries[index]))
                    };
                    let short = deck.is_some_and(|entry| !self.is_affordable(&entry.deck));
                    let dim = if is_incompatible { 0.35 } else if short { 0.45 } else { 1.0 };
                    let tooltip = if is_incompatible {
                        Some("Old format -- rebuild in the Deck Editor")
                    } else if short {
                        Some("You don't have all the cards in your collection to build this deck.")
                    } else {
                        None
                    };
                    let region_options = RegionOptions::hand().with_tooltip(tooltip);
                    let region = frame.ui.region(&format!("dl:tile{}", index + 1), tile_x, tile_y, tile_width, tile_height, region_options);
                    if region.hovered {
                        frame.fill_rect(tile_x, tile_y, tile_width, tile_height, 6.0, with_alpha(palette.on_surface, 0.06));
                    }
                    if !is_incompatible && self.chosen_name.as_deref() == Some(display_name.as_str()) {
                        frame.outline_rect(tile_x + 1.0, tile_y + 1.0, tile_width - 2.0, tile_height - 2.0, 6.0, palette.primary, 2.0);
                    }
                    let back = CardDraw::new(None, false, tile_x + tile_width / 2.0, tile_y + 4.0 + card_height / 2.0, &game.card_backs, None)
                        .scale(TILE_SCALE)
                        .alpha(dim);
                    frame.draw_card(&back);
                    let mut name_y = tile_y + 4.0 + card_height + 4.0;
                    let mut name_options = TextOptions::new().size(11.0).width(tile_width).centered().max_lines(NAME_LINES);
                    if short {
                        name_options = name_options.color(palette.on_surface_variant);
                    }
                    frame.text(&display_name, tile_x, name_y, name_options);
                    name_y += name_height.min(frame.text_height(&display_name, tile_width, 11.0, Weight::Regular)) + 2.0;
                    let small = TextOptions::new().size(10.0).width(tile_width).centered();
                    match deck {
                        None => {
                            frame.text("Old format", tile_x, name_y, small.color(fixed::RED));
                        }
                        Some(entry) => {
                            let count = format!("{} card(s)", entry.card_count());
                            frame.text(&count, tile_x, name_y, small.color(palette.on_surface_variant));
                            if entry.is_built_in {
                                let label = if entry.deck.is_starter { "Built-in · Starter" } else { "Built-in" };
                                frame.text(label, tile_x, name_y + 13.0, small.color(palette.primary));
                            }
                            if short {
                                let missing_y = name_y + if entry.is_built_in { 26.0 } else { 13.0 };
                                frame.text("Missing cards", tile_x, missing_y, small.color(fixed::RED));
                            }
                        }
                    }
                    if region.clicked {
                        match deck {
                            None => {
                                frame.sounds.play(SoundEffect::Error);
                                self.selection_error = Some(format!(
                                    "The deck \"{display_name}\" was saved in an older format and needs to be rebuilt in the Deck Editor."
                                ));
                            }
                            Some(entry) => {
                                let entry = entry.clone();
                                chosen = self.select_deck(frame, &entry.deck, &entry.display_name);
                            }
                        }
                    }
                    // Built-in decks ship with the game, so they can't be deleted.
                    if region.right_clicked && let Some(entry) = deck.filter(|entry| !entry.is_built_in) {
                        menu_for = Some(entry.clone());
                    }
                }
                let rows = total.div_ceil(columns) as f32;
                frame.end_scroll("dl:grid", rows * (tile_height + TILE_GAP) - TILE_GAP + 8.0, &ScrollbarStyle::default());
            }
            // The summary panel.
            let summary_x = x + grid_width + 24.0;
            frame.fill_rect(summary_x - 12.0, row_y, 1.0, body_height, 0.0, palette.outline_variant);
            let mut summary_y = row_y;
            // The Deck Editor opens decks without zones to fill: no requirements.
            if !self.zones.is_empty() {
                frame.text("This game needs", summary_x, summary_y, TextOptions::new().medium());
                summary_y += 28.0;
                for zone in &self.zones {
                    frame.text(&zone.name, summary_x, summary_y, TextOptions::new().bold().width(240.0).ellipsis());
                    let requirement = format!(
                        "{}{}",
                        zone.deck_type,
                        if zone.deck_optional { " (optional)" } else { " (required)" }
                    );
                    let options = TextOptions::new().size(12.0).color(palette.on_surface_variant).width(240.0).ellipsis();
                    frame.text(&requirement, summary_x, summary_y + 18.0, options);
                    summary_y += 44.0;
                }
                frame.divider(summary_x, summary_y + 4.0, 240.0, None);
                summary_y += 16.0;
            }
            let hint = "Click a deck to load it. Right-click one of your decks to delete it.";
            frame.text(hint, summary_x, summary_y, TextOptions::new().size(12.0).width(240.0).color(palette.on_surface_variant));
            summary_y += frame.text_height(hint, 240.0, 12.0, Weight::Regular) + 8.0;
            if let Some(error) = &self.selection_error {
                let error_height = frame.text_height(error, 240.0, 12.0, Weight::Regular);
                frame.text(error, summary_x, summary_y, TextOptions::new().size(12.0).width(240.0).color(fixed::RED));
                summary_y += error_height + 12.0;
            }
            if !self.remote {
                let options = ButtonOptions::text().id("dl:browse");
                open_browse |= frame.button("Browse for file...", summary_x - 12.0, summary_y, None, None, &options);
            }
            DIALOG_CONTENT_HEIGHT
        });
        if refresh {
            self.refresh(frame, store);
        }
        if open_folder_picker {
            let current = self.directory_path.clone();
            self.folder_picker = Some(FilePicker::new(
                frame,
                PickerMode::Folder,
                Some("Choose your deck library folder"),
                None,
                Some(&current),
            ));
        }
        if open_browse {
            self.browse_picker =
                Some(FilePicker::new(frame, PickerMode::File, Some("Choose a deck file"), Some(JSON_FILTERS), None));
        }
        if let Some(entry) = menu_for {
            let (mouse_x, mouse_y) = frame.ui.mouse();
            let items = vec![MenuItem::new("Delete Deck", ()).icon("delete_outline")];
            self.deck_menu = Some((ContextMenu::new("dl:deckmenu", mouse_x, mouse_y, items), entry));
        }
        if let Some((menu, entry)) = &mut self.deck_menu {
            match menu.show(frame) {
                MenuResponse::Open => {}
                MenuResponse::Selected(()) => {
                    self.confirm_delete = Some(entry.clone());
                    self.deck_menu = None;
                }
                MenuResponse::Closed => self.deck_menu = None,
            }
        }
        if let Some(entry) = self.confirm_delete.clone() {
            let message = format!("Delete \"{}\"? This can't be undone.", entry.display_name);
            match frame.confirm_dialog("dl:confirmdelete", "Delete Deck", &message, "Delete", true) {
                ConfirmResponse::Open => {}
                ConfirmResponse::Confirmed => {
                    self.confirm_delete = None;
                    self.delete_deck(frame, store, &entry);
                }
                ConfirmResponse::Cancelled => self.confirm_delete = None,
            }
        }
        if let Some(picker) = &mut self.browse_picker {
            match picker.show(frame) {
                FilePickerResponse::Open => {}
                FilePickerResponse::Cancelled => self.browse_picker = None,
                FilePickerResponse::Picked(mut paths) => {
                    self.browse_picker = None;
                    let path = paths.remove(0);
                    if let Some(deck) = self.browse_result(frame, &path) {
                        chosen = Some(deck);
                    }
                }
            }
        }
        if let Some(picker) = &mut self.folder_picker {
            match picker.show(frame) {
                FilePickerResponse::Open => {}
                FilePickerResponse::Cancelled => self.folder_picker = None,
                FilePickerResponse::Picked(mut paths) => {
                    self.folder_picker = None;
                    let new_root = paths.remove(0);
                    match MoveLibraryPrompt::needed(&self.directory_path, &new_root, "deck") {
                        Some(prompt) => self.move_prompt = Some(prompt),
                        None => self.use_folder(frame, store, new_root),
                    }
                }
            }
        }
        if let Some(prompt) = &self.move_prompt
            && prompt.show(frame) == MovePromptResponse::Finished
        {
            let new_root = prompt.new_root.clone();
            self.move_prompt = None;
            self.use_folder(frame, store, new_root);
        }
        if let Some(deck) = chosen {
            return LoadDeckResponse::Chosen(deck);
        }
        match response {
            DialogResponse::Open | DialogResponse::Enter => LoadDeckResponse::Open,
            _ => LoadDeckResponse::Closed,
        }
    }

    fn use_folder(&mut self, frame: &mut Frame, store: &mut dyn DeckStore, folder: String) {
        let _ = set_decks_folder(frame.settings, &folder);
        self.directory_path = folder;
        let _ = &self.store_label;
        self.refresh(frame, store);
    }
}

/// The Save Deck prompt (`D.promptSaveDeck`), for the local library or a server store.
pub struct SaveDeckDialog {
    game_id: String,
    deck: DeckConfig,
    dialog: SaveNameDialog,
    remote: bool,
    store_label: String,
    /// Offer the "Starter Deck" checkbox.
    show_starter: bool,
    also_local: bool,
    pending_save: Rc<RefCell<Option<(bool, Option<String>, Option<String>)>>>,
    pending_list: ListSlot,
    saving_name: Option<String>,
    /// Waiting for a server's deck list before the name prompt can show.
    waiting_for_list: bool,
}

/// What the Save Deck prompt reports.
#[derive(Debug, Clone, PartialEq)]
pub enum SaveDeckResponse {
    Open,
    /// Saved under this name (and path, for the local library).
    Saved {
        name: String,
        path: Option<String>,
    },
    Closed,
}

fn existing_names(scan: &DeckScan, built_in: bool) -> Vec<ExistingName> {
    scan.entries
        .iter()
        .filter(|entry| built_in || !entry.is_built_in)
        .map(|entry| ExistingName {
            name: entry.display_name.clone(),
            trailing: Some(format!("{} card(s)", entry.card_count())),
        })
        .collect()
}

impl SaveDeckDialog {
    pub fn new(
        frame: &mut Frame,
        game: &GameDefinition,
        deck: DeckConfig,
        store: &mut dyn DeckStore,
        show_starter: bool,
    ) -> SaveDeckDialog {
        let remote = store.kind() == DeckStoreKind::Server;
        let decks_folder = frame.paths.decks_folder(frame.settings).to_string_lossy().into_owned();
        let game_id = game.id.clone();
        let mut dialog = if remote {
            let label = store.label();
            let mut dialog = SaveNameDialog::new(
                "savedeck",
                format!("Save Deck to {label}"),
                "Deck Name",
                Vec::new(),
                "Your decks on this server -- tap to reuse a name",
                format!("No decks saved on this server for {}.", game.name),
                "Overwrite Deck?",
                Box::new(|name| format!("You already have a deck named \"{name}\" on this server. Overwrite it?")),
                Box::new(|_| false),
            );
            dialog.footer_height = if show_starter { 72.0 } else { 32.0 };
            dialog
        } else {
            let scan = load_decks_for_game(&decks_folder, game);
            let root = decks_folder.clone();
            let exists_game_id = game_id.clone();
            let mut dialog = SaveNameDialog::new(
                "savedeck",
                "Save Deck",
                "Deck Name",
                existing_names(&scan, true),
                "Existing decks -- tap to reuse a name",
                format!("No decks found for {}.", game.name),
                "Overwrite Deck?",
                Box::new(|name| format!("A deck named \"{name}\" already exists in the library. Overwrite it?")),
                Box::new(move |name| deck_exists(&root, &exists_game_id, name)),
            );
            dialog.footer_height = if show_starter { 40.0 } else { 0.0 };
            dialog
        };
        dialog.width = 816.0;
        dialog.empty_name_message = Some("Enter a deck name.".to_string());
        let save = SaveDeckDialog {
            game_id,
            deck,
            dialog,
            remote,
            store_label: store.label(),
            show_starter,
            also_local: false,
            pending_save: Rc::new(RefCell::new(None)),
            pending_list: Rc::new(RefCell::new(None)),
            saving_name: None,
            waiting_for_list: remote,
        };
        if remote {
            let slot = save.pending_list.clone();
            store.list(game, Box::new(move |scan, error| *slot.borrow_mut() = Some((scan, error))));
        }
        save
    }

    fn starter_checkbox(frame: &mut Frame, deck: &mut DeckConfig, x: f32, y: f32) {
        let label = "Starter Deck (built-in; players get it free or buy it in collection mode)";
        deck.is_starter =
            frame.checkbox("savename:starter", x + 2.0, y + 7.0, deck.is_starter, &ToggleOptions::label(label));
    }

    pub fn show(&mut self, frame: &mut Frame, game: &GameDefinition, store: &mut dyn DeckStore) -> SaveDeckResponse {
        if self.waiting_for_list {
            let Some((scan, error)) = self.pending_list.borrow_mut().take() else { return SaveDeckResponse::Open };
            if let Some(error) = error {
                frame.ui.toast(&format!("Could not load your decks: {error}"), None);
                return SaveDeckResponse::Closed;
            }
            let names: Vec<String> = scan
                .entries
                .iter()
                .filter(|entry| !entry.is_built_in)
                .map(|entry| entry.display_name.to_lowercase())
                .collect();
            self.dialog.existing = existing_names(&scan, false);
            self.dialog.exists = Box::new(move |name| names.contains(&name.to_lowercase()));
            self.waiting_for_list = false;
        }
        let finished = self.pending_save.borrow_mut().take();
        if let Some((ok, message, location)) = finished {
            let name = self.saving_name.take().unwrap_or_default();
            if !ok {
                frame.ui.toast(&format!("Could not save the deck: {}", message.unwrap_or_default()), None);
                return SaveDeckResponse::Closed;
            }
            if self.remote {
                let text = message.unwrap_or_else(|| format!("Saved \"{name}\" to {}", self.store_label));
                frame.ui.toast(&text, None);
            } else {
                frame.ui.toast(&format!("Saved to {}", location.clone().unwrap_or_default()), None);
            }
            return SaveDeckResponse::Saved { name, path: location.filter(|_| !self.remote) };
        }
        if self.saving_name.is_some() {
            return SaveDeckResponse::Open;
        }
        let (remote, show_starter) = (self.remote, self.show_starter);
        let deck = &mut self.deck;
        let also_local = &mut self.also_local;
        let mut footer = |frame: &mut Frame, x: f32, y: f32, _width: f32| {
            if remote {
                let label = "Also save a copy to my local deck library";
                *also_local =
                    frame.checkbox("savename:local", x + 2.0, y + 7.0, *also_local, &ToggleOptions::label(label));
                if show_starter {
                    SaveDeckDialog::starter_checkbox(frame, deck, x, y + 40.0);
                }
            } else if show_starter {
                SaveDeckDialog::starter_checkbox(frame, deck, x, y);
            }
        };
        let has_footer = remote || show_starter;
        let response = self.dialog.show(frame, if has_footer { Some(&mut footer) } else { None });
        match response {
            SaveNameResponse::Open => SaveDeckResponse::Open,
            SaveNameResponse::Cancelled => SaveDeckResponse::Closed,
            SaveNameResponse::Save(name) => {
                self.saving_name = Some(name.clone());
                let slot = self.pending_save.clone();
                let _ = (&self.game_id, self.also_local);
                store.write(
                    game,
                    &name,
                    &self.deck,
                    Some(Box::new(move |ok, message, location| {
                        *slot.borrow_mut() = Some((ok, message, location));
                    })),
                );
                self.show(frame, game, store)
            }
        }
    }

    /// The deck being saved, with the Starter Deck checkbox applied.
    pub fn deck(&self) -> &DeckConfig {
        &self.deck
    }

    /// Whether a server save should also keep a local copy (the "also local" checkbox).
    pub fn also_local(&self) -> bool {
        self.also_local
    }
}
