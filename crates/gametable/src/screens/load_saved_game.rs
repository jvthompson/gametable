//! Loading a saved game, and matching the save's players to the current ones. Port of
//! `src/ui/screens/load_saved_game.lua` (`load_saved_game_screen.dart`,
//! `seat_match_screen.dart`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gametable_core::models::{GameDefinition, PlayerInfo, SavedGame, TableState, build_standard_deck_cards};
use gametable_core::save_remap::{build_loaded_table_state, match_players_by_name};

use crate::app::{Frame, Screen, ScreenCommand};
use crate::data::save_files::{SaveGameListing, format_saved_at, list_save_games, read_save_game};
use crate::screens::game_picker::GamePicker;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::dropdown::DropdownOptions;
use crate::ui::overlays::ConfirmResponse;
use crate::ui::theme::{Weight, with_alpha};
use crate::ui::widgets::{ButtonOptions, Choice, ScrollbarStyle};

/// Where a loaded save goes: the screen that opens the game with it.
pub type OnLoaded = Box<dyn FnMut(&Frame, GameDefinition, TableState) -> Box<dyn Screen>>;

/// The seat match's answer, picked up by the Load Saved Game screen when it resumes.
type SeatMatchResult = Rc<RefCell<Option<HashMap<String, String>>>>;

/// "Match Returning Players": each saved player picks who they are now.
pub struct SeatMatch {
    saved: Vec<PlayerInfo>,
    order: Vec<PlayerInfo>,
    result: SeatMatchResult,
}

impl Screen for SeatMatch {
    fn title(&self) -> String {
        "Match Returning Players".to_string()
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let column_width = (width - 48.0).min(480.0);
        let column_x = (x + (width - column_width) / 2.0).floor();
        let intro = "This save's players couldn't be matched to your current players by name alone. \
                     Choose who each saved player is now:";
        let intro_height = frame.text_height(intro, column_width, 14.0, Weight::Regular);
        let total = intro_height + 24.0 + self.saved.len() as f32 * 60.0 + 24.0 + 48.0;
        let mut row_y = (y + 24f32.max((height - total) / 2.0)).floor();
        frame.text(intro, column_x, row_y, TextOptions::new().width(column_width));
        row_y += intro_height + 24.0;
        for index in 0..self.saved.len() {
            let label = format!("Saved as \"{}\"", self.saved[index].name);
            frame.text(
                &label,
                column_x,
                row_y,
                TextOptions::new().middle_of(48.0).width(column_width - 220.0).ellipsis(),
            );
            let choices: Vec<Choice<String>> =
                self.order.iter().map(|player| Choice::new(player.id.clone(), player.name.clone())).collect();
            let current = self.order[index].id.clone();
            let picked = frame.dropdown(
                &format!("seatmatch:{}", index + 1),
                column_x + column_width - 200.0,
                row_y,
                200.0,
                &choices,
                current.clone(),
                &DropdownOptions::default(),
            );
            if picked != current
                && let Some(other) = self.order.iter().position(|player| player.id == picked)
            {
                self.order.swap(index, other);
            }
            row_y += 60.0;
        }
        row_y += 24.0;
        if frame.button("Confirm", column_x, row_y, Some(column_width), Some(48.0), &ButtonOptions::default()) {
            let map = self
                .saved
                .iter()
                .zip(&self.order)
                .map(|(saved, current)| (saved.id.clone(), current.id.clone()))
                .collect();
            *self.result.borrow_mut() = Some(map);
            frame.commands.push(ScreenCommand::Pop);
        }
    }
}

/// A save waiting for the "Some Cards No Longer Exist" confirmation.
struct PendingLoad {
    state: TableState,
    dropped: usize,
}

pub struct LoadSavedGame {
    current_players: Vec<PlayerInfo>,
    on_loaded: OnLoaded,
    picker: GamePicker,
    game: Option<GameDefinition>,
    entries: Vec<SaveGameListing>,
    /// A save waiting on the seat match.
    awaiting_match: Option<(SavedGame, HashMap<String, String>)>,
    seat_match_result: SeatMatchResult,
    confirm: Option<PendingLoad>,
}

/// Every card id the game can deal (its cards plus the standard deck when a shared zone uses it).
fn valid_definition_ids(game: &GameDefinition) -> HashSet<String> {
    let mut ids: HashSet<String> = game.cards().iter().map(|card| card.id.clone()).collect();
    if game.zones.iter().any(|zone| zone.shared && zone.standard_deck) {
        ids.extend(build_standard_deck_cards().into_iter().map(|card| card.id));
    }
    ids
}

impl LoadSavedGame {
    pub fn new(frame: &Frame, current_players: Vec<PlayerInfo>, on_loaded: OnLoaded) -> LoadSavedGame {
        LoadSavedGame {
            current_players,
            on_loaded,
            picker: GamePicker::new(frame, false),
            game: None,
            entries: Vec::new(),
            awaiting_match: None,
            seat_match_result: Rc::new(RefCell::new(None)),
            confirm: None,
        }
    }

    fn choose_game(&mut self, frame: &mut Frame, game: GameDefinition) {
        let Some(folder) = game.folder_path.clone() else {
            frame.ui.toast("This game has no folder on disk, so it has no saved games.", None);
            return;
        };
        self.entries = list_save_games(&folder);
        self.game = Some(game);
    }

    fn finish(&mut self, frame: &mut Frame, save: &SavedGame, id_map: &HashMap<String, String>) {
        let Some(game) = &self.game else { return };
        let loaded = build_loaded_table_state(&save.state, &self.current_players, id_map, &valid_definition_ids(game));
        if loaded.dropped_card_count > 0 {
            self.confirm = Some(PendingLoad { state: loaded.state, dropped: loaded.dropped_card_count });
        } else {
            self.open(frame, loaded.state);
        }
    }

    fn open(&mut self, frame: &mut Frame, state: TableState) {
        let Some(game) = self.game.clone() else { return };
        let screen = (self.on_loaded)(frame, game, state);
        frame.commands.push(ScreenCommand::Push(screen));
    }

    fn pick_save(&mut self, frame: &mut Frame, listing: &SaveGameListing) {
        let save = match read_save_game(&listing.file_path) {
            Ok(save) => save,
            Err(error) => {
                frame.ui.toast(&format!("Could not read that save: {error}"), None);
                return;
            }
        };
        let matched = match_players_by_name(&save.state.players, &self.current_players);
        if !matched.unmatched_saved_players.is_empty() {
            *self.seat_match_result.borrow_mut() = None;
            let seat_match = SeatMatch {
                saved: matched.unmatched_saved_players.clone(),
                order: matched.unmatched_current_players.clone(),
                result: self.seat_match_result.clone(),
            };
            self.awaiting_match = Some((save, matched.current_id_by_saved_id));
            frame.commands.push(ScreenCommand::Push(Box::new(seat_match)));
        } else {
            self.finish(frame, &save, &matched.current_id_by_saved_id);
        }
    }
}

impl Screen for LoadSavedGame {
    fn title(&self) -> String {
        match &self.game {
            Some(game) => format!("Load a Saved Game -- {}", game.name),
            None => "Load a Saved Game".to_string(),
        }
    }

    fn on_back(&mut self, frame: &mut Frame) {
        if self.game.is_some() {
            self.game = None;
            self.entries.clear();
        } else {
            frame.commands.push(ScreenCommand::Pop);
        }
    }

    fn resume(&mut self, frame: &mut Frame) {
        let Some((save, matched)) = self.awaiting_match.take() else { return };
        let manual = self.seat_match_result.borrow_mut().take();
        if let Some(manual) = manual {
            let mut id_map = matched;
            id_map.extend(manual);
            self.finish(frame, &save, &id_map);
        }
    }

    fn update(&mut self, _frame: &mut Frame, _dt: f32) {
        if self.game.is_none() {
            self.picker.update();
        }
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        if self.game.is_none() {
            if let Some(game) = self.picker.draw(frame, x, y, width, height) {
                self.choose_game(frame, game);
            }
            return;
        }
        let palette = frame.palette();
        let game_name = self.game.as_ref().map_or(String::new(), |game| game.name.clone());
        if self.entries.is_empty() {
            let options =
                TextOptions::new().width(width).middle_of(height).centered().color(palette.on_surface_variant);
            frame.text(&format!("No saved games found for {game_name}."), x, y, options);
        } else {
            let column_width = (width - 32.0).min(480.0);
            let column_x = (x + (width - column_width) / 2.0).floor();
            let (row_height, gap) = (72.0, 8.0);
            let total = self.entries.len() as f32 * (row_height + gap);
            let list_y = y + 16f32.max((height - total) / 2.0);
            let view_height = total.min(height - 32.0);
            let offset = frame.begin_scroll("loadsave:list", column_x - 8.0, list_y, column_width + 16.0, view_height);
            let mut row_y = list_y - offset;
            let needed = self.current_players.len();
            let mut picked = None;
            for (index, listing) in self.entries.iter().enumerate() {
                let compatible = listing.player_count == needed;
                let region = compatible.then(|| {
                    frame.ui.region(
                        &format!("loadsave:{}", index + 1),
                        column_x,
                        row_y,
                        column_width,
                        row_height,
                        RegionOptions::hand(),
                    )
                });
                frame.fill_rect(column_x, row_y, column_width, row_height, 12.0, palette.surface_container);
                if region.as_ref().is_some_and(|region| region.hovered) {
                    frame.fill_rect(
                        column_x,
                        row_y,
                        column_width,
                        row_height,
                        12.0,
                        with_alpha(palette.on_surface, 0.06),
                    );
                }
                let foreground = if compatible { palette.on_surface } else { palette.disabled };
                let name_options =
                    TextOptions::new().size(16.0).color(foreground).width(column_width - 32.0).ellipsis();
                frame.text(&listing.display_name, column_x + 16.0, row_y + 14.0, name_options);
                let detail = if compatible {
                    format!("{} player(s)", listing.player_count)
                } else {
                    format!("needs {needed} player(s), this save has {}", listing.player_count)
                };
                let subtitle = format!("Saved {} -- {detail}", format_saved_at(&listing.saved_at));
                let color = if compatible { palette.on_surface_variant } else { palette.disabled };
                let options = TextOptions::new().size(13.0).color(color).width(column_width - 32.0).ellipsis();
                frame.text(&subtitle, column_x + 16.0, row_y + 40.0, options);
                if region.is_some_and(|region| region.clicked) {
                    picked = Some(listing.clone());
                }
                row_y += row_height + gap;
            }
            frame.end_scroll("loadsave:list", total, &ScrollbarStyle::default());
            if let Some(listing) = picked {
                self.pick_save(frame, &listing);
            }
        }
        if let Some(pending) = &self.confirm {
            let message = format!(
                "{} card(s) in this save reference content that no longer exists in {game_name} (it's been edited since \
                 this save) and will be skipped. Continue loading?",
                pending.dropped
            );
            match frame.confirm_dialog("loadsave:dropped", "Some Cards No Longer Exist", &message, "Continue", false) {
                ConfirmResponse::Open => {}
                ConfirmResponse::Confirmed => {
                    if let Some(pending) = self.confirm.take() {
                        self.open(frame, pending.state);
                    }
                }
                ConfirmResponse::Cancelled => self.confirm = None,
            }
        }
    }
}
