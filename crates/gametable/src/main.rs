//! GameTable client entry point. Port of `main.lua` / `conf.lua`.

use gametable::app::{App, Options};
use gametable::screens::kit_gallery::KitGallery;
use gametable::screens::placeholder_home::PlaceholderHome;

fn main() {
    let options = Options::parse(std::env::args().skip(1));
    let result = App::new(&options, || Box::new(PlaceholderHome::default())).and_then(|mut app| {
        // Development hook: `GAMETABLE_SCREEN=kit_gallery` opens the UI kit gallery at startup.
        if std::env::var("GAMETABLE_SCREEN").is_ok_and(|screen| screen == "kit_gallery") {
            app.push(Box::new(KitGallery::default()));
        }
        app.run()
    });
    if let Err(message) = result {
        eprintln!("GameTable: {message}");
        std::process::exit(1);
    }
}
