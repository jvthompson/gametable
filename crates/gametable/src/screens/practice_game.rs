//! An offline practice game where you can act as every seat (click an avatar to switch). Port
//! of `src/ui/screens/practice_game.lua` (`practice_game_screen.dart`).

use std::collections::HashMap;

use gametable_core::models::{DeckConfig, GameDefinition, PlayerInfo, SavedGame, TableState};
use gametable_core::session::{DealOptions, Session, SessionEnvironment};

use crate::app::{Frame, Screen, ScreenCommand};
use crate::data::save_files::{list_save_games, local_now, save_exists, write_save_game};
use crate::table::{PracticeBackend, TableEvent, TableOptions, TableScreen};
use crate::ui::save_name_dialog::{ExistingName, SaveNameDialog, SaveNameResponse};
use gametable_core::models::format_iso_local_timestamp;

pub struct PracticeGame {
    table: TableScreen,
    save_dialog: Option<SaveNameDialog>,
    entered: bool,
}

impl PracticeGame {
    /// A new practice game: deals the zones; built-deck zones start empty unless decks are given
    /// (each seat loads a deck at the table).
    pub fn new(
        frame: &Frame,
        game: GameDefinition,
        players: Vec<PlayerInfo>,
        deck_configs_by_player_id: HashMap<String, DeckConfig>,
    ) -> PracticeGame {
        let local_player_id = players[0].id.clone();
        let options = DealOptions {
            game,
            players,
            local_player_id,
            deck_configs_by_player_id: Some(&deck_configs_by_player_id),
            shared_deck_configs_by_zone_id: None,
            is_local_practice: true,
        };
        let session = Session::deal_from_zones(options, SessionEnvironment::system());
        PracticeGame::with_session(frame, session)
    }

    /// Resumes a saved game (already remapped to these players).
    pub fn resume(
        frame: &Frame,
        game: GameDefinition,
        players: &[PlayerInfo],
        loaded_state: TableState,
    ) -> PracticeGame {
        let resume_at = loaded_state.log.last().map(|entry| entry.elapsed_milliseconds);
        let mut session = Session::new(game, players[0].id.clone(), loaded_state, true, SessionEnvironment::system());
        // Resume the log clock where the save left off.
        if let Some(elapsed) = resume_at {
            session.resume_log_clock(elapsed);
        }
        PracticeGame::with_session(frame, session)
    }

    fn with_session(frame: &Frame, session: Session) -> PracticeGame {
        let can_save_game = session.game.folder_path.is_some();
        let options = TableOptions {
            backend: Box::new(PracticeBackend { session }),
            is_mirrored: false,
            local_avatar_path: frame.profile.avatar_path.clone(),
            avatar_keys: HashMap::new(),
            leave_button_label: "Leave Game".to_string(),
            leave_confirmation_message: "Leave this practice game and return to the home screen?".to_string(),
            can_save_game,
            can_kick_players: false,
            can_load_scenario: true,
            collection_mode: false,
            spectator: false,
            deck_store: None,
        };
        PracticeGame { table: TableScreen::new(options), save_dialog: None, entered: false }
    }

    pub fn table_mut(&mut self) -> &mut TableScreen {
        &mut self.table
    }

    /// The Save Game prompt over the table.
    fn open_save_dialog(&mut self) {
        let game = self.table.game();
        let Some(folder) = game.folder_path.clone() else { return };
        let game_name = game.name.clone();
        let existing = list_save_games(&folder)
            .into_iter()
            .map(|save| ExistingName {
                name: save.display_name,
                trailing: Some(format!("{} player(s)", save.player_count)),
            })
            .collect();
        let overwrite_name = game_name.clone();
        let exists_folder = folder.clone();
        let mut dialog = SaveNameDialog::new(
            "savegame",
            "Save Game",
            "Save Name",
            existing,
            "Existing saves -- tap to reuse a name",
            format!("No saved games yet for {game_name}."),
            "Overwrite Save?",
            Box::new(move |name| format!("A save named \"{name}\" already exists for {overwrite_name}. Overwrite it?")),
            Box::new(move |name| save_exists(&exists_folder, name)),
        );
        dialog.empty_name_message = Some("Enter a name.".to_string());
        self.save_dialog = Some(dialog);
    }

    fn save_game(&mut self, frame: &mut Frame, name: &str) {
        let Some(folder) = self.table.game().folder_path.clone() else { return };
        let mut state = self.table.backend().session().state().clone();
        state.searches.clear();
        let save = SavedGame { saved_at: format_iso_local_timestamp(&local_now()), state };
        match write_save_game(&folder, name, &save) {
            Ok(_) => frame.ui.toast(&format!("Saved \"{name}\""), None),
            Err(error) => frame.ui.toast(&format!("Could not save: {error}"), None),
        }
    }
}

impl Screen for PracticeGame {
    fn title(&self) -> String {
        self.table.game().name.clone()
    }

    fn shows_back(&self) -> bool {
        false
    }

    fn forces_light_theme(&self) -> bool {
        true
    }

    fn update(&mut self, frame: &mut Frame, dt: f32) {
        self.table.update(frame, dt);
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        if !self.entered {
            self.entered = true;
            self.table.enter(frame);
        }
        self.table.draw(frame, x, y, width, height);
        for event in self.table.take_events() {
            match event {
                TableEvent::LeaveGame => frame.commands.push(ScreenCommand::PopToHome(None)),
                TableEvent::SaveGame => self.open_save_dialog(),
                TableEvent::KickPlayer(_) => {}
            }
        }
        if let Some(dialog) = &mut self.save_dialog {
            match dialog.show(frame, None) {
                SaveNameResponse::Open => {}
                SaveNameResponse::Cancelled => self.save_dialog = None,
                SaveNameResponse::Save(name) => {
                    self.save_dialog = None;
                    self.save_game(frame, &name);
                }
            }
        }
    }

    fn key_pressed(&mut self, frame: &mut Frame, key: &'static str, is_repeat: bool) {
        self.table.key_pressed(frame, key, is_repeat);
    }
}
