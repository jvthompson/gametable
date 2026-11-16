//! Saved decks, collections, booster packs and starter decks. Port of `decks.go`.

use std::collections::{BTreeMap, HashMap};

use gametable_core::json::{self, JsonObject, JsonValue};
use gametable_core::models::{CardDefinition, PackSetting};
use gametable_core::pack_generator::select_pack_cards;
use gametable_core::util::SystemRandom;

use super::{Request, Server};
use crate::games::ServerCard;
use crate::log;
use crate::payload::{sorted, text_array};
use crate::store::{StoreError, User};

/// Characters a deck name may not contain (they are unsafe in file names on the client).
const FORBIDDEN_DECK_NAME_CHARACTERS: &str = "/\\:*?\"<>|";

/// Trims a deck name and checks it. Returns the trimmed name and a problem to show, if any.
pub(crate) fn valid_deck_name(name: &str) -> (String, Option<&'static str>) {
    let name = name.trim().to_string();
    if name.is_empty() {
        return (name, Some("Enter a deck name."));
    }
    if name.chars().count() > 64 {
        return (name, Some("Deck names can be at most 64 characters."));
    }
    if name.chars().any(|character| character.is_control() || FORBIDDEN_DECK_NAME_CHARACTERS.contains(character)) {
        return (name, Some(r#"Deck names can't contain / \ : * ? " < > |"#));
    }
    (name, None)
}

/// The card counts a client `DeckConfig` asks for, read the way Go's typed decoding of
/// `{gameId, subdecks: [{name, entries: [{definitionId, quantity}]}]}` would: any field of the
/// wrong type makes the whole deck invalid (`None`). Missing and `null` fields are allowed.
pub(crate) struct DeckCardCounts {
    /// Whether `subdecks` was present and not `null` (Go's `dc.Subdecks != nil`).
    pub has_subdecks: bool,
    /// Card id -> copies asked for, summed over every subdeck.
    pub quantities: HashMap<String, i64>,
}

pub(crate) fn read_deck_card_counts(deck: &JsonValue) -> Option<DeckCardCounts> {
    let mut counts = DeckCardCounts { has_subdecks: false, quantities: HashMap::new() };
    let deck = match deck {
        JsonValue::Null => return Some(counts),
        JsonValue::Object(_) => deck,
        _ => return None,
    };
    if !is_text_or_absent(deck.get("gameId")) {
        return None;
    }
    let subdecks = match deck.get("subdecks") {
        None | Some(JsonValue::Null) => return Some(counts),
        Some(JsonValue::Array(subdecks)) => subdecks,
        Some(_) => return None,
    };
    counts.has_subdecks = true;
    for subdeck in subdecks {
        match subdeck {
            JsonValue::Null => continue,
            JsonValue::Object(_) => {}
            _ => return None,
        }
        if !is_text_or_absent(subdeck.get("name")) {
            return None;
        }
        let entries = match subdeck.get("entries") {
            None | Some(JsonValue::Null) => continue,
            Some(JsonValue::Array(entries)) => entries,
            Some(_) => return None,
        };
        for entry in entries {
            match entry {
                JsonValue::Null => continue,
                JsonValue::Object(_) => {}
                _ => return None,
            }
            let definition_id = match entry.get("definitionId") {
                None | Some(JsonValue::Null) => String::new(),
                Some(JsonValue::String(id)) => id.clone(),
                Some(_) => return None,
            };
            let quantity = match entry.get("quantity") {
                None | Some(JsonValue::Null) => 0,
                Some(JsonValue::Number(number)) if is_go_int64(*number) => *number as i64,
                Some(_) => return None,
            };
            *counts.quantities.entry(definition_id).or_insert(0) += quantity;
        }
    }
    Some(counts)
}

fn is_text_or_absent(value: Option<&JsonValue>) -> bool {
    matches!(value, None | Some(JsonValue::Null) | Some(JsonValue::String(_)))
}

/// Whether Go would decode the number into an `int64`.
fn is_go_int64(number: f64) -> bool {
    number.fract() == 0.0 && (-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&number)
}

/// Describes the first few cards a collection lacks for a deck, or `None` when it has them all.
/// Unknown cards are ignored (the engine skips them); unownable and free cards are unlimited.
pub(crate) fn deck_shortfall(
    cards: &HashMap<String, ServerCard>,
    needed: &HashMap<String, i64>,
    owned: &BTreeMap<String, i64>,
) -> Option<String> {
    let mut missing: Vec<String> = needed
        .iter()
        .filter_map(|(card_id, &needed_quantity)| {
            let card = cards.get(card_id).filter(|card| card.is_collectible())?;
            let owned_quantity = owned.get(card_id).copied().unwrap_or(0);
            if needed_quantity <= owned_quantity {
                return None;
            }
            let title = if card.title.is_empty() { card_id.as_str() } else { card.title.as_str() };
            Some(format!("{title} ({owned_quantity}/{needed_quantity})"))
        })
        .collect();
    if missing.is_empty() {
        return None;
    }
    missing.sort();
    if missing.len() > 3 {
        let more = missing.len() - 3;
        missing.truncate(3);
        missing.push(format!("and {more} more"));
    }
    Some(format!("Your collection doesn't have enough copies: {}", missing.join(", ")))
}

/// Picks a booster pack from a set's collectible cards, as the original's `openPack` does.
fn open_pack(set_cards: &[&ServerCard], pack_settings: Option<&JsonValue>) -> Result<Vec<String>, String> {
    let definitions = set_cards
        .iter()
        .map(|card| CardDefinition::from_json(&card.raw).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let settings = match pack_settings {
        Some(JsonValue::Array(settings)) => settings
            .iter()
            .map(|setting| PackSetting::from_json(setting).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?,
        _ => Vec::new(),
    };
    Ok(select_pack_cards(&definitions, &settings, &mut SystemRandom).into_iter().map(|card| card.id.clone()).collect())
}

/// A stored deck's JSON as a value (Go embeds the stored text verbatim).
fn stored_deck_value(deck_json: &str) -> JsonValue {
    json::decode(deck_json).unwrap_or(JsonValue::Null)
}

impl Server {
    /// Checks a `requestLoadDeckIntoZone` against the player's collection. Returns the problem to
    /// show, if any.
    pub(crate) fn check_deck_against_collection(
        &self,
        user_id: &str,
        game_id: &str,
        payload: &JsonValue,
    ) -> Option<String> {
        let deck = payload.get("deck").unwrap_or(&JsonValue::Null);
        let Some(counts) = read_deck_card_counts(deck) else {
            return Some("That deck is not valid.".to_string());
        };
        let library = self.library.read().unwrap();
        let Some(game) = library.game(game_id) else {
            return Some("The server no longer has this game.".to_string());
        };
        let Ok(owned) = self.store().collection(user_id, game_id) else {
            return Some("Could not check your collection.".to_string());
        };
        deck_shortfall(&game.cards, &counts.quantities, &owned)
    }
}

impl Request<'_> {
    pub(super) fn handle_deck_list(&self, user: &User) {
        let game_id = self.fields.text("gameId");
        let include_decks = self.fields.flag("full");
        let Ok(decks) = self.server.store().decks(&user.id, &game_id, include_decks) else {
            self.fail("internal", "Could not list decks.");
            return;
        };
        let decks = decks.into_iter().map(|deck| {
            let mut info = JsonObject::new().with("name", deck.name).with("updatedAt", deck.updated_at as f64);
            if let Some(deck_json) = deck.deck_json.filter(|text| !text.is_empty()) {
                info.insert("deck", stored_deck_value(&deck_json));
            }
            JsonValue::Object(info)
        });
        self.reply(JsonObject::new().with("decks", JsonValue::Array(decks.collect())));
    }

    pub(super) fn handle_deck_get(&self, user: &User) {
        let game_id = self.fields.text("gameId");
        let name = self.fields.text("name");
        let Ok(deck_json) = self.server.store().deck(&user.id, &game_id, &name) else {
            self.fail("notFound", "That deck doesn't exist.");
            return;
        };
        self.reply(JsonObject::new().with("name", name).with("deck", stored_deck_value(&deck_json)));
    }

    pub(super) fn handle_deck_save(&self, user: &User) {
        let game_id = self.fields.text("gameId");
        let (name, problem) = valid_deck_name(&self.fields.text("name"));
        if let Some(problem) = problem {
            self.fail("badName", problem);
            return;
        }
        let deck = self.fields.raw("deck");
        let deck_is_valid = deck.and_then(read_deck_card_counts).is_some_and(|counts| counts.has_subdecks);
        let (Some(deck), true) = (deck, deck_is_valid) else {
            self.fail("badDeck", "The deck is not valid.");
            return;
        };
        let Some(game_name) = self.server.library.read().unwrap().game(&game_id).map(|game| game.name.clone()) else {
            self.fail("noGame", "The server doesn't have that game.");
            return;
        };
        let overwrite = self.fields.flag("overwrite");
        let mut store = self.server.store();
        if !overwrite && store.deck(&user.id, &game_id, &name).is_ok() {
            drop(store);
            self.fail("exists", &format!("A deck named {name:?} already exists."));
            return;
        }
        let deck_json = json::encode_compact(deck).unwrap_or_default();
        if store.save_deck(&user.id, &game_id, &name, &deck_json).is_err() {
            drop(store);
            self.fail("internal", "Could not save the deck.");
            return;
        }
        drop(store);
        log!(self.server, "{} saved deck {name:?} ({game_name})", user.name);
        self.reply(JsonObject::new().with("name", name));
    }

    pub(super) fn handle_deck_delete(&self, user: &User) {
        let game_id = self.fields.text("gameId");
        let name = self.fields.text("name");
        if self.server.store().delete_deck(&user.id, &game_id, &name).is_err() {
            self.fail("internal", "Could not delete the deck.");
            return;
        }
        self.reply_empty();
    }

    pub(super) fn handle_collection_get(&self, user: &User) {
        let game_id = self.fields.text("gameId");
        let Ok(collection) = self.server.store().collection(&user.id, &game_id) else {
            self.fail("internal", "Could not load your collection.");
            return;
        };
        self.reply(
            JsonObject::new()
                .with("gameId", game_id)
                .with("cards", quantities_object(&collection))
                .with("tokens", user.tokens as f64),
        );
    }

    pub(super) fn handle_pack_buy(&self, user: &User) {
        let server = self.server;
        let settings = server.settings();
        if !settings.is_collection_mode() {
            self.fail("notCollection", "Packs are only available in collection mode.");
            return;
        }
        let game_id = self.fields.text("gameId");
        let set_id = self.fields.text("setId");
        let (game_name, set_name, pack) = {
            let library = server.library.read().unwrap();
            let Some(game) = library.game(&game_id) else {
                drop(library);
                self.fail("noGame", "The server doesn't have that game.");
                return;
            };
            let Some(set) = game.set(&set_id) else {
                drop(library);
                self.fail("noSet", "That set doesn't exist.");
                return;
            };
            let collectible: Vec<&ServerCard> =
                game.set_cards(&set.id).into_iter().filter(|card| card.is_collectible()).collect();
            if collectible.is_empty() {
                drop(library);
                self.fail("noCards", "That set has no collectible cards.");
                return;
            }
            let pack = open_pack(&collectible, set.pack_settings.as_ref());
            (game.name.clone(), set.name.clone(), pack)
        };
        let card_ids = match pack {
            Ok(card_ids) => card_ids,
            Err(error) => {
                log!(server, "open pack {game_id}/{set_id}: {error}");
                self.fail("internal", "Could not open the pack.");
                return;
            }
        };
        if card_ids.is_empty() {
            self.fail("noCards", "That pack came up empty; no tokens were spent.");
            return;
        }
        let bought = server.store().buy_pack(&user.id, &game_id, &set_id, settings.pack_cost, &card_ids);
        let balance = match bought {
            Ok(balance) => balance,
            Err(StoreError::NotEnoughTokens) => {
                self.fail("notEnoughTokens", "You don't have enough tokens.");
                return;
            }
            Err(_) => {
                self.fail("internal", "Could not open the pack.");
                return;
            }
        };
        server.set_online_tokens(&user.id, balance);
        log!(server, "{} opened a pack: {game_name} / {set_name}", user.name);
        self.reply(
            JsonObject::new()
                .with("gameId", game_id)
                .with("setId", set_id)
                .with("cards", text_array(&card_ids))
                .with("tokens", balance as f64),
        );
    }

    /// Sells one of a game's starter decks: its collectible cards go into the collection and a
    /// copy of the deck, minus the starter flag, is saved to the player's decks.
    pub(super) fn handle_starter_buy(&self, user: &User) {
        let server = self.server;
        let settings = server.settings();
        if !settings.is_collection_mode() {
            self.fail("notCollection", "Starter decks are only available in collection mode.");
            return;
        }
        let game_id = self.fields.text("gameId");
        let requested_name = self.fields.text("name");
        let (game_name, starter_name, card_quantities, deck) = {
            let library = server.library.read().unwrap();
            let Some(game) = library.game(&game_id) else {
                drop(library);
                self.fail("noGame", "The server doesn't have that game.");
                return;
            };
            let Some(starter) = game.starter_deck(&requested_name) else {
                drop(library);
                self.fail("noStarter", "That starter deck doesn't exist.");
                return;
            };
            let card_quantities: BTreeMap<String, i64> = starter
                .card_quantities
                .iter()
                .filter(|(card_id, _)| game.cards.get(*card_id).is_some_and(ServerCard::is_collectible))
                .map(|(card_id, quantity)| (card_id.clone(), *quantity))
                .collect();
            let deck = std::str::from_utf8(&starter.deck_bytes).ok().and_then(|text| json::decode(text).ok());
            (game.name.clone(), starter.name.clone(), card_quantities, deck)
        };
        // Go re-encodes the deck as a map: keys sorted, without any spelling of `isStarter`.
        let Some(JsonValue::Object(deck)) = deck else {
            self.fail("internal", "Could not read the starter deck.");
            return;
        };
        let mut saved_deck = JsonObject::new();
        for (key, value) in deck.iter() {
            if !key.eq_ignore_ascii_case("isStarter") {
                saved_deck.insert(key, value.clone());
            }
        }
        let deck_json = json::encode_compact(&sorted(saved_deck)).unwrap_or_default();
        let deck_name = match valid_deck_name(&starter_name) {
            (name, None) => name,
            (_, Some(_)) => "Starter Deck".to_string(),
        };
        let bought = server.store().buy_starter(
            &user.id,
            &game_id,
            &starter_name,
            settings.starter_deck_cost,
            &card_quantities,
            &deck_name,
            &deck_json,
        );
        let (balance, saved_name) = match bought {
            Ok(bought) => bought,
            Err(StoreError::NotEnoughTokens) => {
                self.fail("notEnoughTokens", "You don't have enough tokens.");
                return;
            }
            Err(_) => {
                self.fail("internal", "Could not buy the starter deck.");
                return;
            }
        };
        server.set_online_tokens(&user.id, balance);
        log!(server, "{} bought a starter deck: {game_name} / {starter_name} (saved as {saved_name:?})", user.name);
        self.reply(
            JsonObject::new()
                .with("gameId", game_id)
                .with("name", starter_name)
                .with("savedName", saved_name)
                .with("cards", quantities_object(&card_quantities))
                .with("tokens", balance as f64),
        );
    }
}

/// Card id -> quantity as a JSON object (keys sorted, like a Go map).
pub(crate) fn quantities_object(quantities: &BTreeMap<String, i64>) -> JsonValue {
    let mut object = JsonObject::new();
    for (card_id, quantity) in quantities {
        object.insert(card_id, *quantity as f64);
    }
    JsonValue::Object(object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deck_names_are_checked_like_go() {
        assert_eq!(valid_deck_name("  Burn  "), ("Burn".to_string(), None));
        assert_eq!(valid_deck_name("   ").1, Some("Enter a deck name."));
        assert_eq!(valid_deck_name(&"x".repeat(65)).1, Some("Deck names can be at most 64 characters."));
        assert!(valid_deck_name(&"é".repeat(64)).1.is_none());
        assert!(valid_deck_name("a/b").1.is_some());
        assert!(valid_deck_name("a\u{7}b").1.is_some());
    }

    #[test]
    fn deck_card_counts_follow_go_typing() {
        let deck = json::decode(
            r#"{"gameId":"g","subdecks":[{"name":"Main","entries":[{"definitionId":"a","quantity":2},{"definitionId":"a","quantity":1}]},{"entries":[{"definitionId":"b","quantity":4}]}]}"#,
        )
        .unwrap();
        let counts = read_deck_card_counts(&deck).unwrap();
        assert!(counts.has_subdecks);
        assert_eq!(counts.quantities["a"], 3);
        assert_eq!(counts.quantities["b"], 4);
        assert!(!read_deck_card_counts(&json::decode(r#"{"x":1}"#).unwrap()).unwrap().has_subdecks);
        assert!(read_deck_card_counts(&json::decode(r#"{"subdecks":{}}"#).unwrap()).is_none());
        assert!(
            read_deck_card_counts(&json::decode(r#"{"subdecks":[{"entries":[{"quantity":1.5}]}]}"#).unwrap()).is_none()
        );
        assert!(read_deck_card_counts(&json::decode(r#"{"gameId":3,"subdecks":[]}"#).unwrap()).is_none());
        assert!(read_deck_card_counts(&json::decode(r#"[1]"#).unwrap()).is_none());
    }

    /// Port of the Go engine's `TestOpenPack`.
    #[test]
    fn packs_follow_their_settings() {
        let card = |id: &str, tag: &str| ServerCard {
            id: id.to_string(),
            title: id.to_string(),
            set_id: "s".to_string(),
            types: vec![tag.to_string()],
            unownable: false,
            is_free_card: false,
            raw: json::decode(&format!(r#"{{"id":"{id}","cardTitle":"{id}","types":["{tag}"]}}"#)).unwrap(),
        };
        let cards = [card("c1", "Common"), card("c2", "Common"), card("c3", "Common"), card("r1", "Rare")];
        let set_cards: Vec<&ServerCard> = cards.iter().collect();
        let settings = json::decode(r#"[{"tag":"Common","count":2},{"tag":"Rare","count":1}]"#).unwrap();
        for _ in 0..20 {
            let pack = open_pack(&set_cards, Some(&settings)).unwrap();
            assert_eq!(pack.len(), 3, "{pack:?}");
            assert_eq!(pack[2], "r1");
            assert!(pack[..2].iter().all(|id| id.starts_with('c')) && pack[0] != pack[1]);
        }
        assert_eq!(open_pack(&set_cards, None).unwrap().len(), 4, "no settings: up to ten random cards");
        let missing_tag = json::decode(r#"[{"count":2}]"#).unwrap();
        assert!(open_pack(&set_cards, Some(&missing_tag)).is_err());
    }

    #[test]
    fn shortfall_lists_three_cards_then_a_count() {
        let card = |id: &str, title: &str, unownable: bool| {
            (
                id.to_string(),
                ServerCard {
                    id: id.to_string(),
                    title: title.to_string(),
                    set_id: String::new(),
                    types: Vec::new(),
                    unownable,
                    is_free_card: false,
                    raw: JsonValue::Null,
                },
            )
        };
        let cards: HashMap<_, _> = [
            card("a", "Alpha", false),
            card("b", "", false),
            card("c", "Gamma", false),
            card("d", "Delta", false),
            card("t", "Token", true),
        ]
        .into_iter()
        .collect();
        let needed: HashMap<String, i64> = [("a", 2), ("b", 1), ("c", 1), ("d", 3), ("t", 9), ("unknown", 5)]
            .map(|(id, n)| (id.to_string(), n))
            .into();
        let owned: BTreeMap<String, i64> = [("a".to_string(), 1), ("c".to_string(), 1)].into();
        assert_eq!(
            deck_shortfall(&cards, &needed, &owned).unwrap(),
            "Your collection doesn't have enough copies: Alpha (1/2), Delta (0/3), b (0/1)"
        );
        let needed: HashMap<String, i64> =
            [("a", 2), ("b", 1), ("d", 1), ("c", 2)].map(|(id, n)| (id.to_string(), n)).into();
        assert_eq!(
            deck_shortfall(&cards, &needed, &BTreeMap::new()).unwrap(),
            "Your collection doesn't have enough copies: Alpha (0/2), Delta (0/1), Gamma (0/2), and 1 more"
        );
        assert!(deck_shortfall(&cards, &HashMap::new(), &owned).is_none());
    }
}
