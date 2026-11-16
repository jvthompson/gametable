//! GameTable client entry point. Port of `main.lua` / `conf.lua`.

use gametable::app::{App, Options};
use gametable::screens::home::Home;
use gametable::screens::kit_gallery::KitGallery;
use gametable::screens::player_count::PlayerCount;
use gametable::screens::practice_select::PracticeSelect;

fn main() {
    let options = Options::parse(std::env::args().skip(1));
    let result = App::new(&options, || Box::new(Home::new(None))).and_then(|mut app| {
        // Development hook: `GAMETABLE_SCREEN=kit_gallery|player_count|practice_select` opens that
        // screen at startup.
        match std::env::var("GAMETABLE_SCREEN").as_deref() {
            Ok("kit_gallery") => app.push(Box::new(KitGallery::default())),
            Ok("player_count") => app.push(Box::new(PlayerCount::practice())),
            Ok("practice_select") => app.push_with_frame(|frame| Box::new(PracticeSelect::new(frame, 2))),
            _ => {}
        }
        app.run()
    });
    if let Err(message) = result {
        eprintln!("GameTable: {message}");
        std::process::exit(1);
    }
}
