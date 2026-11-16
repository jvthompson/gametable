//! Choosing a game for offline practice. Port of `src/ui/screens/practice_select.lua`
//! (`practice_game_select_screen.dart`).

use gametable_core::color::BOARD_WIDGET_COLOR_PALETTE;
use gametable_core::models::{PlayerInfo, PlayerRole};

use std::collections::HashMap;

use crate::app::{Frame, PlayerProfile, Screen, ScreenCommand};
use crate::screens::game_picker::GamePicker;
use crate::screens::load_saved_game::{LoadSavedGame, OnLoaded};
use crate::screens::practice_game::PracticeGame;
use crate::ui::widgets::{ButtonOptions, ButtonVariant};

pub struct PracticeSelect {
    player_count: usize,
    picker: GamePicker,
}

/// Seat 1 is you; the other seats get the next unused palette colours.
pub fn build_practice_players(profile: &PlayerProfile, player_count: usize) -> Vec<PlayerInfo> {
    let mut players = vec![PlayerInfo::new("seat1", profile.display_name(), PlayerRole::Host, profile.color)];
    let mut used_colors = vec![profile.color];
    for seat in 2..=player_count {
        let color = BOARD_WIDGET_COLOR_PALETTE
            .iter()
            .copied()
            .find(|color| !used_colors.contains(color))
            .unwrap_or(BOARD_WIDGET_COLOR_PALETTE[0]);
        used_colors.push(color);
        players.push(PlayerInfo::new(format!("seat{seat}"), format!("Player {seat}"), PlayerRole::Host, color));
    }
    players
}

impl PracticeSelect {
    pub fn new(frame: &Frame, player_count: usize) -> PracticeSelect {
        PracticeSelect { player_count, picker: GamePicker::new(frame, false) }
    }
}

impl Screen for PracticeSelect {
    fn title(&self) -> String {
        "Choose a Game".to_string()
    }

    fn update(&mut self, _frame: &mut Frame, _dt: f32) {
        self.picker.update();
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let label = "Load a Saved Game...";
        let options = ButtonOptions::variant(ButtonVariant::Outlined).icon("folder_open");
        let button_width = frame.button_width(label, &options);
        if frame.button(label, x + width - 16.0 - button_width, y + 16.0, Some(button_width), None, &options) {
            let players = build_practice_players(frame.profile, self.player_count);
            let seats = players.clone();
            let on_loaded: OnLoaded = Box::new(move |frame, game, state| {
                Box::new(PracticeGame::resume(frame, game, &seats, state)) as Box<dyn Screen>
            });
            let screen = LoadSavedGame::new(frame, players, on_loaded);
            frame.commands.push(ScreenCommand::Push(Box::new(screen)));
        }
        if let Some(game) = self.picker.draw(frame, x, y + 56.0, width, height - 56.0) {
            // An empty deck map: built-deck zones start empty and each seat loads a deck at the table.
            let players = build_practice_players(frame.profile, self.player_count);
            let screen = PracticeGame::new(frame, game, players, HashMap::new());
            frame.commands.push(ScreenCommand::Push(Box::new(screen)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn practice_seats_get_unused_colors() {
        let profile = PlayerProfile { name: " Ann ".into(), color: BOARD_WIDGET_COLOR_PALETTE[0], avatar_path: None };
        let players = build_practice_players(&profile, 3);
        assert_eq!(
            players.iter().map(|player| player.name.as_str()).collect::<Vec<_>>(),
            ["Ann", "Player 2", "Player 3"]
        );
        assert_eq!(players[1].color, BOARD_WIDGET_COLOR_PALETTE[1]);
        assert_eq!(players[2].color, BOARD_WIDGET_COLOR_PALETTE[2]);
        assert_eq!(players[2].id, "seat3");
    }
}
