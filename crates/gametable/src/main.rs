//! GameTable client entry point. Port of `main.lua` / `conf.lua`.

// Release builds on Windows are GUI programs (no console window), like the fused LÖVE exe;
// development builds keep the console for their log output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gametable::app::{App, Options};
use gametable::data::file_system::join_path;
use gametable::data::game_loader::{load_library, load_standard_deck};
use gametable::data::gamedef_ops::{find_game_definition_file, load_game_definition_file};
use gametable::platform::updater;
use gametable::screens::deck_editor::{DeckEditor, DeckEditorGameSelect};
use gametable::screens::gamedef::{GameDefinitionEditor, GameDefinitionEntry};
use gametable::screens::home::Home;
use gametable::screens::kit_gallery::KitGallery;
use gametable::screens::multiplayer::{HostSetup, Join};
use gametable::screens::player_count::PlayerCount;
use gametable::screens::practice_game::PracticeGame;
use gametable::screens::practice_select::{PracticeSelect, build_practice_players};
use gametable_core::models::DeckConfig;
use std::collections::HashMap;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    // Started by the updater from a downloaded release: install it and exit (no window).
    if let Some(request) = updater::parse_helper_request(&arguments) {
        std::process::exit(updater::run_helper(&request));
    }
    updater::remove_leftover_update_folders();
    let options = Options::parse(arguments);
    // `U.check`: once per launch, packaged builds only (`--update-check` forces it).
    updater::start_check(options.force_update_check);
    let result = App::new(&options, || Box::new(Home::new(None))).and_then(|mut app| {
        // Development hook: `GAMETABLE_SCREEN=kit_gallery|player_count|practice_select` opens that
        // screen at startup.
        match std::env::var("GAMETABLE_SCREEN").as_deref() {
            Ok("kit_gallery") => app.push(Box::new(KitGallery::default())),
            Ok("player_count") => app.push(Box::new(PlayerCount::practice())),
            Ok("practice_select") => app.push_with_frame(|frame| Box::new(PracticeSelect::new(frame, 2))),
            // Hosting (`GAMETABLE_PLAYERS`, default 2) and joining (`GAMETABLE_JOIN=<host:port>`
            // connects straight away).
            Ok("host") => app.push_with_frame(|frame| {
                let player_count =
                    std::env::var("GAMETABLE_PLAYERS").ok().and_then(|text| text.parse().ok()).unwrap_or(2);
                Box::new(HostSetup::new(frame, player_count))
            }),
            Ok("join") => app.push(Box::new(match std::env::var("GAMETABLE_JOIN") {
                Ok(address) => {
                    let (host, port) = address.rsplit_once(':').unwrap_or((address.as_str(), ""));
                    Join::new(Some(host), port.parse().ok(), true)
                }
                Err(_) => Join::new(None, None, false),
            })),
            // `GAMETABLE_GAME=<game id>` (default standard_52), `GAMETABLE_PLAYERS=<1-4>` (default 2).
            Ok("practice") => app.push_with_frame(|frame| {
                let wanted = std::env::var("GAMETABLE_GAME").unwrap_or_else(|_| "standard_52".to_string());
                let player_count =
                    std::env::var("GAMETABLE_PLAYERS").ok().and_then(|count| count.parse().ok()).unwrap_or(2);
                let library = frame.paths.games_folder(frame.settings).to_string_lossy().into_owned();
                let (games, _) = load_library(&library);
                let game = games.into_iter().find(|game| game.id == wanted).unwrap_or_else(load_standard_deck);
                let players = build_practice_players(frame.profile, player_count);
                // `GAMETABLE_PRACTICE_DECKS=full` deals every seat one of every card.
                let decks: HashMap<String, DeckConfig> =
                    if std::env::var("GAMETABLE_PRACTICE_DECKS").is_ok_and(|decks| decks == "full") {
                        players.iter().map(|player| (player.id.clone(), DeckConfig::full(&game))).collect()
                    } else {
                        HashMap::new()
                    };
                let mut practice = PracticeGame::new(frame, game, players, decks);
                if let Ok(steps) = std::env::var("GAMETABLE_TABLE_SETUP") {
                    practice.table_mut().apply_development_setup(&steps);
                }
                Box::new(practice)
            }),
            // `GAMETABLE_GAME=<game id>`, `GAMETABLE_DECK_SETUP=add,rulebook`.
            Ok("deck_editor") => app.push_with_frame(|frame| {
                let wanted = std::env::var("GAMETABLE_GAME").unwrap_or_default();
                let library = frame.paths.games_folder(frame.settings).to_string_lossy().into_owned();
                let (games, _) = load_library(&library);
                let game = games.into_iter().find(|game| game.id == wanted).unwrap_or_else(load_standard_deck);
                let mut editor = DeckEditor::new(game, None, None);
                if let Ok(steps) = std::env::var("GAMETABLE_DECK_SETUP") {
                    editor.apply_development_setup(&steps);
                }
                Box::new(editor)
            }),
            // `GAMETABLE_GAME=<game folder name>`, `GAMETABLE_GAMEDEF_TAB=<tab>[,select]`.
            Ok("gamedef") => app.push_with_frame(|frame| {
                let wanted = std::env::var("GAMETABLE_GAME").unwrap_or_default();
                let library = frame.paths.games_folder(frame.settings).to_string_lossy().into_owned();
                let folder = join_path(&[&library, &wanted]);
                let game = find_game_definition_file(&folder)
                    .and_then(|path| load_game_definition_file(&path, &folder).ok())
                    .unwrap_or_else(load_standard_deck);
                let mut editor = GameDefinitionEditor::new(game);
                if let Ok(steps) = std::env::var("GAMETABLE_GAMEDEF_TAB") {
                    editor.apply_development_setup(&steps);
                }
                Box::new(editor)
            }),
            Ok("gamedef_entry") => app.push_with_frame(|frame| Box::new(GameDefinitionEntry::new(frame))),
            Ok("deck_editor_select") => app.push_with_frame(|frame| Box::new(DeckEditorGameSelect::new(frame))),
            _ => {}
        }
        app.run()
    });
    if let Err(message) = result {
        eprintln!("GameTable: {message}");
        std::process::exit(1);
    }
}
