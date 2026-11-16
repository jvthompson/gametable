//! Where decks are listed, loaded and saved. Port of `src/data/deck_store.lua`.
//!
//! The deck dialogs work through [`DeckStore`] so they don't care whether decks live in the
//! local deck library folder or in a CardTable server account (the server store arrives with the
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
