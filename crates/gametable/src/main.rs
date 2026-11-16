//! GameTable client entry point. Port of `main.lua` / `conf.lua`.

use gametable::app::{App, Options};
use gametable::screens::placeholder_home::PlaceholderHome;

fn main() {
    let options = Options::parse(std::env::args().skip(1));
    let result = App::new(&options, || Box::new(PlaceholderHome::default())).and_then(App::run);
    if let Err(message) = result {
        eprintln!("GameTable: {message}");
        std::process::exit(1);
    }
}
