//! Where decks are listed, loaded and saved. Port of `src/data/deck_store.lua`.
//!
//! The deck dialogs work through [`DeckStore`] so they don't care whether decks live in the
//! local deck library folder or in a GameTable server account (the server store arrives with the
//! lobby client). Results arrive through callbacks, as in the original: the local store calls them
//! straight away, the server store when its reply comes in.

use gametable_core::models::{DeckConfig, GameDefinition};

use super::deck_library::{DeckScan, deck_path, load_all_decks_for_game, write_deck};
use super::file_system::remove_tree;

/// `cb(scan, error)`.
pub type ListCallback = Box<dyn FnOnce(DeckScan, Option<String>)>;
/// `cb(ok, error message, where it was saved)`.
pub type WriteCallback = Box<dyn FnOnce(bool, Option<String>, Option<String>)>;
/// `cb(ok, error message)`.
pub type RemoveCallback = Box<dyn FnOnce(bool, Option<String>)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckStoreKind {
    Local,
    Server,
}

pub trait DeckStore {
    fn kind(&self) -> DeckStoreKind;
    /// Shown in dialogs ("your deck library", a server's name).
    fn label(&self) -> String;
    fn list(&mut self, game: &GameDefinition, done: ListCallback);
    fn write(&mut self, game: &GameDefinition, name: &str, deck: &DeckConfig, done: Option<WriteCallback>);
    fn remove(&mut self, game: &GameDefinition, name: &str, done: Option<RemoveCallback>);
    /// Server stores: saves also write a copy to the local library (the Save Deck checkbox).
    fn set_also_local(&mut self, _also_local: bool) {}
}

/// A GameTable server account's decks (`M.serverStore(sc)`), with the game's built-in decks.
pub struct ServerDeckStore {
    pub server: crate::net::server_client::SharedServer,
    /// The local deck library, for "also save a local copy".
    pub decks_folder: String,
    pub also_local: bool,
}

impl ServerDeckStore {
    pub fn new(server: crate::net::server_client::SharedServer, decks_folder: impl Into<String>) -> ServerDeckStore {
        ServerDeckStore { server, decks_folder: decks_folder.into(), also_local: false }
    }
}

impl DeckStore for ServerDeckStore {
    fn kind(&self) -> DeckStoreKind {
        DeckStoreKind::Server
    }

    fn label(&self) -> String {
        self.server.borrow().info.name.clone().unwrap_or_else(|| "the server".to_string())
    }

    fn list(&mut self, game: &GameDefinition, done: ListCallback) {
        use gametable_core::json::{JsonObject, JsonValue};
        // Built-in decks ship inside the (cached) game folder.
        let built_in = super::deck_library::load_built_in_decks_for_game(game);
        let payload = JsonObject::new().with("gameId", game.id.as_str()).with("full", true);
        self.server.borrow_mut().request_with(
            "deckList",
            payload,
            Box::new(move |response| {
                let mut scan = DeckScan::default();
                let ok = crate::net::server_client::response_ok(response);
                if ok {
                    for deck in response.get("decks").and_then(JsonValue::as_array).cloned().unwrap_or_default() {
                        let name = deck.get("name").and_then(JsonValue::as_str).unwrap_or("").to_string();
                        match deck.get("deck").map(DeckConfig::from_json) {
                            Some(Ok(config)) => scan.entries.push(super::deck_library::LibraryDeck {
                                display_name: name,
                                file_path: None,
                                deck: config,
                                is_built_in: false,
                            }),
                            _ => scan.incompatible.push(super::deck_library::IncompatibleDeck {
                                display_name: name,
                                file_path: None,
                                is_built_in: false,
                            }),
                        }
                    }
                }
                scan.entries.extend(built_in.entries);
                let error =
                    (!ok).then(|| crate::net::server_client::response_message(response, "Could not load decks."));
                done(scan, error);
            }),
        );
    }

    fn write(&mut self, game: &GameDefinition, name: &str, deck: &DeckConfig, done: Option<WriteCallback>) {
        use gametable_core::json::JsonObject;
        let payload = JsonObject::new()
            .with("gameId", game.id.as_str())
            .with("name", name)
            .with("deck", deck.to_json())
            .with("overwrite", true);
        let also_local =
            self.also_local.then(|| (self.decks_folder.clone(), game.id.clone(), name.to_string(), deck.clone()));
        let label = self.label();
        self.server.borrow_mut().request_with(
            "deckSave",
            payload,
            Box::new(move |response| {
                let ok = crate::net::server_client::response_ok(response);
                if ok
                    && let Some((folder, game_id, name, deck)) = also_local
                    && let Err(error) = write_deck(&folder, &game_id, &name, &deck)
                {
                    if let Some(done) = done {
                        done(
                            true,
                            Some(format!("Saved on the server, but the local copy failed: {error}")),
                            Some(label),
                        );
                    }
                    return;
                }
                if let Some(done) = done {
                    let message = response.get("message").and_then(|message| message.as_str()).map(str::to_string);
                    done(ok, message, Some(label));
                }
            }),
        );
    }

    fn remove(&mut self, game: &GameDefinition, name: &str, done: Option<RemoveCallback>) {
        use gametable_core::json::JsonObject;
        let payload = JsonObject::new().with("gameId", game.id.as_str()).with("name", name);
        self.server.borrow_mut().request_with(
            "deckDelete",
            payload,
            Box::new(move |response| {
                if let Some(done) = done {
                    let message = response.get("message").and_then(|message| message.as_str()).map(str::to_string);
                    done(crate::net::server_client::response_ok(response), message);
                }
            }),
        );
    }

    fn set_also_local(&mut self, also_local: bool) {
        self.also_local = also_local;
    }
}

/// The local deck library folder.
pub struct LocalDeckStore {
    pub decks_folder: String,
}

impl LocalDeckStore {
    pub fn new(decks_folder: impl Into<String>) -> LocalDeckStore {
        LocalDeckStore { decks_folder: decks_folder.into() }
    }
}

impl DeckStore for LocalDeckStore {
    fn kind(&self) -> DeckStoreKind {
        DeckStoreKind::Local
    }

    fn label(&self) -> String {
        "your deck library".to_string()
    }

    fn list(&mut self, game: &GameDefinition, done: ListCallback) {
        done(load_all_decks_for_game(&self.decks_folder, game), None);
    }

    fn write(&mut self, game: &GameDefinition, name: &str, deck: &DeckConfig, done: Option<WriteCallback>) {
        let result = write_deck(&self.decks_folder, &game.id, name, deck);
        if let Some(done) = done {
            match result {
                Ok(path) => done(true, None, Some(path)),
                Err(error) => done(false, Some(error), None),
            }
        }
    }

    fn remove(&mut self, game: &GameDefinition, name: &str, done: Option<RemoveCallback>) {
        let removed = remove_tree(std::path::Path::new(&deck_path(&self.decks_folder, &game.id, name)));
        if let Some(done) = done {
            done(removed.is_ok(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn local_store_round_trip() {
        let root = std::env::temp_dir().join(format!("gametable-store-{}", gametable_core::uuid::new_v4_uuid()));
        let mut store = LocalDeckStore::new(root.to_string_lossy());
        let game = GameDefinition::new("g", "Game");
        let saved_to = Rc::new(RefCell::new(None));
        let saved = saved_to.clone();
        store.write(
            &game,
            "Mine",
            &DeckConfig::new("g", vec![]),
            Some(Box::new(move |ok, _, path| {
                assert!(ok);
                *saved.borrow_mut() = path;
            })),
        );
        assert!(saved_to.borrow().as_deref().is_some_and(|path| path.ends_with("Mine.json")));
        let listed = Rc::new(RefCell::new(Vec::new()));
        let names = listed.clone();
        store.list(
            &game,
            Box::new(move |scan, error| {
                assert!(error.is_none());
                *names.borrow_mut() = scan.entries.into_iter().map(|entry| entry.display_name).collect();
            }),
        );
        assert_eq!(*listed.borrow(), ["Mine"]);
        store.remove(&game, "Mine", Some(Box::new(|ok, _| assert!(ok))));
        store.list(&game, Box::new(|scan, _| assert!(scan.entries.is_empty())));
        std::fs::remove_dir_all(root).unwrap();
    }
}
