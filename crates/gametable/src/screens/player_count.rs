//! "How many players?" screens. Port of `src/ui/screens/player_count.lua`
//! (`player_count_screen.dart` for hosting 2-4 players, `practice_player_count_screen.dart` for
//! practising with 1-4 decks).

use crate::app::{Frame, Screen, ScreenCommand};
use crate::screens::not_ported::NotPortedYet;
use crate::screens::practice_select::PracticeSelect;
use crate::ui::draw::TextOptions;
use crate::ui::widgets::{ButtonOptions, ButtonVariant};

/// What the chosen count leads to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerCountPurpose {
    Practice,
    Host,
}

pub struct PlayerCount {
    purpose: PlayerCountPurpose,
}

impl PlayerCount {
    /// `S.practice()`.
    pub fn practice() -> PlayerCount {
        PlayerCount { purpose: PlayerCountPurpose::Practice }
    }

    /// `S.host()`.
    pub fn host() -> PlayerCount {
        PlayerCount { purpose: PlayerCountPurpose::Host }
    }

    fn question(&self) -> &'static str {
        match self.purpose {
            PlayerCountPurpose::Practice => "How many decks?",
            PlayerCountPurpose::Host => "How many players?",
        }
    }

    fn counts(&self) -> &'static [usize] {
        match self.purpose {
            PlayerCountPurpose::Practice => &[1, 2, 3, 4],
            PlayerCountPurpose::Host => &[2, 3, 4],
        }
    }

    fn label(&self, count: usize) -> String {
        if self.purpose == PlayerCountPurpose::Practice && count == 1 {
            "1 Player".to_string()
        } else {
            format!("{count} Players")
        }
    }

    fn choose(&self, frame: &mut Frame, count: usize) {
        let next: Box<dyn Screen> = match self.purpose {
            PlayerCountPurpose::Practice => Box::new(PracticeSelect::new(frame, count)),
            PlayerCountPurpose::Host => NotPortedYet::boxed("Host Game", "Hosting a game", 7),
        };
        frame.commands.push(ScreenCommand::Push(next));
    }
}

impl Screen for PlayerCount {
    fn title(&self) -> String {
        match self.purpose {
            PlayerCountPurpose::Practice => "Practice Offline".to_string(),
            PlayerCountPurpose::Host => "Host Game".to_string(),
        }
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let column_width = 312.0;
        let counts = self.counts();
        let total = 20.0 + 24.0 + counts.len() as f32 * 60.0;
        let column_x = (x + (width - column_width) / 2.0).floor();
        let mut row_y = (y + (height - total) / 2.0).floor();
        frame.text(self.question(), column_x, row_y, TextOptions::new().bold().width(column_width).centered());
        row_y += 20.0 + 24.0;
        for &count in counts {
            let id = format!("count{count}");
            let options = ButtonOptions::variant(ButtonVariant::Filled).id(&id);
            if frame.button(&self.label(count), column_x, row_y, Some(column_width), Some(48.0), &options) {
                self.choose(frame, count);
            }
            row_y += 60.0;
        }
    }
}
