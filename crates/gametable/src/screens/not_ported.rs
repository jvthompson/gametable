//! A temporary stand-in for screens later phases of the port bring (the game table, multiplayer,
//! the editors). It names what will be here and when; Back returns to the previous screen.

use crate::app::{Frame, Screen};
use crate::ui::draw::TextOptions;

pub struct NotPortedYet {
    title: String,
    feature: String,
    phase: u8,
}

impl NotPortedYet {
    pub fn new(title: impl Into<String>, feature: impl Into<String>, phase: u8) -> NotPortedYet {
        NotPortedYet { title: title.into(), feature: feature.into(), phase }
    }

    pub fn boxed(title: impl Into<String>, feature: impl Into<String>, phase: u8) -> Box<dyn Screen> {
        Box::new(NotPortedYet::new(title, feature, phase))
    }
}

impl Screen for NotPortedYet {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let center_y = y + height / 2.0;
        frame.icon_centered("construction", x, center_y - 72.0, width, 48.0, 40.0, palette.on_surface_variant);
        let heading = TextOptions::new().size(18.0).bold().width(width).centered();
        frame.text(&self.feature, x, center_y - 12.0, heading);
        let line = format!("Not ported yet: this arrives in Phase {} of the GameTable port.", self.phase);
        let body = TextOptions::new().width(width).centered().color(palette.on_surface_variant);
        frame.text(&line, x, center_y + 20.0, body);
    }
}
