//! Static game content: game definitions (`gamedef.json`) and decks.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::json_fields::{FieldReader, SparseObjectWriter, string_list_to_json};
use super::{
    BoardKind, CardOrientation, DEFAULT_BOARD_WIDGET_BACKGROUND_COLOR, DEFAULT_BOARD_WIDGET_TEXT_COLOR, ModelError,
    STANDARD_DECK_ID_PREFIX, SharedZoneSide, ZoneKind, ZoneWidgetKind,
};
use crate::color::ArgbColor;
use crate::json::{JsonObject, JsonValue};
use crate::util::trim_lua_whitespace;

impl Default for CardOrientation {
    fn default() -> Self {
        CardOrientation::Portrait
    }
}
impl Default for SharedZoneSide {
    fn default() -> Self {
        SharedZoneSide::Left
    }
}
impl Default for ZoneKind {
    fn default() -> Self {
        ZoneKind::Card
    }
}
impl Default for ZoneWidgetKind {
    fn default() -> Self {
        ZoneWidgetKind::Counter
    }
}
impl Default for BoardKind {
    fn default() -> Self {
        BoardKind::Player
    }
}

// ---------------------------------------------------------------------------
// CardDefinition
// ---------------------------------------------------------------------------

/// One card in a game: its title, art and metadata.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CardDefinition {
    pub id: String,
    pub card_title: String,
    /// `"#RRGGBB"` used to draw a code-generated face when there is no image.
    pub color_hex: Option<String>,
    /// `hearts` / `diamonds` / `clubs` / `spades` draws the matching suit glyph.
    pub suit: Option<String>,
    pub rank: Option<String>,
    /// File name relative to the set folder (or the game folder for cards without a set).
    pub image_path: Option<String>,
    /// Free-form key/value pairs. Written in the order stored here.
    pub extra_fields: JsonObject,
    /// Tags, each `"Group|Tag"` or a legacy bare `"Tag"`.
    pub types: Vec<String>,
    pub orientation: CardOrientation,
    pub set_id: Option<String>,
    pub unownable: bool,
    pub is_free_card: bool,
    pub card_back_id: Option<String>,
    pub unique_back_orientation: CardOrientation,
    pub is_scenario_card: bool,
}

impl CardDefinition {
    pub fn new(id: impl Into<String>, card_title: impl Into<String>) -> Self {
        Self { id: id.into(), card_title: card_title.into(), ..Self::default() }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "CardDefinition")?;
        let mut extra_fields = match fields.optional_value("extraFields") {
            None => JsonObject::new(),
            Some(JsonValue::Object(object)) => object.clone(),
            Some(JsonValue::Array(_)) => JsonObject::new(),
            Some(_) => {
                return Err(ModelError::WrongFieldType {
                    model_kind: "CardDefinition",
                    field_name: "extraFields".to_string(),
                    expected: "an object",
                });
            }
        };
        // The Lua loader copies extraFields into a plain table, which loses the file's key order;
        // its writer then sorts the keys. Sorting here gives the same output.
        extra_fields.sort_keys();
        Ok(Self {
            id: fields.required_string("id")?,
            card_title: fields.required_string("cardTitle")?,
            color_hex: fields.optional_string("colorHex")?,
            suit: fields.optional_string("suit")?,
            rank: fields.optional_string("rank")?,
            image_path: fields.optional_string("imagePath")?,
            extra_fields,
            types: fields.string_list("types")?,
            orientation: fields.enum_or_default(
                "orientation",
                CardOrientation::Portrait,
                "CardDefinition.orientation",
            )?,
            set_id: fields.optional_string("setId")?,
            unownable: fields.flag_is_exactly_true("unownable"),
            is_free_card: fields.flag_is_exactly_true("isFreeCard"),
            card_back_id: fields.optional_string("cardBackId")?,
            unique_back_orientation: fields.enum_or_default(
                "uniqueBackOrientation",
                CardOrientation::Portrait,
                "CardDefinition.uniqueBackOrientation",
            )?,
            is_scenario_card: fields.flag_is_exactly_true("isScenarioCard"),
        })
    }

    /// `omit_set_id` is true when the card is written nested inside its set.
    pub fn to_json(&self, omit_set_id: bool) -> JsonValue {
        let extra_fields = (!self.extra_fields.is_empty()).then(|| self.extra_fields.clone());
        let types = (!self.types.is_empty()).then(|| string_list_to_json(&self.types));
        SparseObjectWriter::new()
            .field("id", &self.id)
            .field("cardTitle", &self.card_title)
            .optional("colorHex", self.color_hex.as_ref())
            .optional("suit", self.suit.as_ref())
            .optional("rank", self.rank.as_ref())
            .optional("imagePath", self.image_path.as_ref())
            .optional("extraFields", extra_fields)
            .optional("types", types)
            .unless_default("orientation", self.orientation.as_json_str(), "portrait")
            .optional("setId", if omit_set_id { None } else { self.set_id.as_ref() })
            .flag_when_true("unownable", self.unownable)
            .flag_when_true("isFreeCard", self.is_free_card)
            .optional("cardBackId", self.card_back_id.as_ref())
            .unless_default("uniqueBackOrientation", self.unique_back_orientation.as_json_str(), "portrait")
            .flag_when_true("isScenarioCard", self.is_scenario_card)
            .finish_value()
    }
}

// ---------------------------------------------------------------------------
// CardBackDefinition / TokenDefinition
// ---------------------------------------------------------------------------

/// A named card back image. The first back in a game is its default.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CardBackDefinition {
    pub id: String,
    pub name: String,
    /// Relative to the game folder.
    pub image_path: Option<String>,
    pub orientation: CardOrientation,
}

impl CardBackDefinition {
    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "CardBackDefinition")?;
        Ok(Self {
            id: fields.required_string("id")?,
            name: fields.required_string("name")?,
            image_path: fields.optional_string("imagePath")?,
            orientation: fields.enum_or_default(
                "orientation",
                CardOrientation::Portrait,
                "CardBackDefinition.orientation",
            )?,
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("id", &self.id)
            .field("name", &self.name)
            .optional("imagePath", self.image_path.as_ref())
            .unless_default("orientation", self.orientation.as_json_str(), "portrait")
            .finish_value()
    }
}

/// A token players can place from the token picker.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TokenDefinition {
    pub id: String,
    /// May carry a type suffix: `"Daisy Walker|Investigator"`.
    pub name: String,
    /// Relative to the game's `_tokens/` folder.
    pub image_path: Option<String>,
}

impl TokenDefinition {
    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "TokenDefinition")?;
        Ok(Self {
            id: fields.required_string("id")?,
            name: fields.required_string("name")?,
            image_path: fields.optional_string("imagePath")?,
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("id", &self.id)
            .field("name", &self.name)
            .optional("imagePath", self.image_path.as_ref())
            .finish_value()
    }

    /// The name without its `|Type` suffix.
    pub fn display_name(&self) -> &str {
        match self.name.split_once('|') {
            Some((before_bar, _)) => trim_lua_whitespace(before_bar),
            None => &self.name,
        }
    }

    /// The type after `|`, trimmed; `None` when there is no bar or nothing after it.
    pub fn token_type(&self) -> Option<&str> {
        let (_, after_bar) = self.name.split_once('|')?;
        let token_type = trim_lua_whitespace(after_bar);
        (!token_type.is_empty()).then_some(token_type)
    }
}

// ---------------------------------------------------------------------------
// PackSetting / GameSet / TagGroup
// ---------------------------------------------------------------------------

/// How many cards with a rarity tag go in a booster pack. A fractional count means the integer
/// part is guaranteed and the fraction is the chance of one bonus card.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PackSetting {
    pub tag: String,
    pub count: f64,
}

impl PackSetting {
    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "PackSetting")?;
        Ok(Self { tag: fields.required_string("tag")?, count: fields.required_number("count")? })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new().field("tag", &self.tag).field("count", self.count).finish_value()
    }
}

/// A card set. Its cards live in `GameDefinition::cards` with a matching `set_id`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GameSet {
    pub id: String,
    pub name: String,
    pub pack_settings: Vec<PackSetting>,
}

impl GameSet {
    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "GameSet")?;
        Ok(Self {
            id: fields.required_string("id")?,
            name: fields.required_string("name")?,
            pack_settings: fields.list_of("packSettings", PackSetting::from_json)?,
        })
    }

    /// The set without its cards; `GameDefinition::to_json` appends them.
    pub fn to_json_without_cards(&self) -> JsonObject {
        SparseObjectWriter::new()
            .field("id", &self.id)
            .field("name", &self.name)
            .non_empty_list("packSettings", &self.pack_settings, PackSetting::to_json)
            .finish()
    }
}

/// A named group of tags, such as "Rarity".
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TagGroup {
    pub id: String,
    pub name: String,
    pub tags: Vec<String>,
}

impl TagGroup {
    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "TagGroup")?;
        Ok(Self {
            id: fields.required_string("id")?,
            name: fields.required_string("name")?,
            tags: fields.string_list("tags")?,
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("id", &self.id)
            .field("name", &self.name)
            .field("tags", string_list_to_json(&self.tags))
            .finish_value()
    }
}

// ---------------------------------------------------------------------------
// DeckEntry / SubDeck / DeckConfig
// ---------------------------------------------------------------------------

/// `quantity` copies of one card. In token-bag zones `definition_id` is a `_tokens/` image path.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeckEntry {
    pub definition_id: String,
    pub quantity: i64,
}

impl DeckEntry {
    pub fn new(definition_id: impl Into<String>, quantity: i64) -> Self {
        Self { definition_id: definition_id.into(), quantity }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "DeckEntry")?;
        Ok(Self {
            definition_id: fields.required_string("definitionId")?,
            quantity: fields.required_whole_number("quantity")?,
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("definitionId", &self.definition_id)
            .field("quantity", self.quantity)
            .finish_value()
    }
}

/// A named part of a deck, matching a zone's `deck_type` (or `"main_deck"`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SubDeck {
    pub name: String,
    pub entries: Vec<DeckEntry>,
}

impl SubDeck {
    pub fn new(name: impl Into<String>, entries: Vec<DeckEntry>) -> Self {
        Self { name: name.into(), entries }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "SubDeck")?;
        Ok(Self {
            name: fields.required_string("name")?,
            entries: fields.required_list_of("entries", DeckEntry::from_json)?,
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("name", &self.name)
            .list("entries", &self.entries, DeckEntry::to_json)
            .finish_value()
    }
}

/// Name of the only subdeck in games without deck-building zones.
pub const MAIN_DECK_SUBDECK_NAME: &str = "main_deck";

/// A saved deck (`<decks>/<gameId>/<Name>.json`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeckConfig {
    pub game_id: String,
    pub subdecks: Vec<SubDeck>,
    /// On a collection-mode server, built-in starter decks can be bought with tokens.
    pub is_starter: bool,
}

impl DeckConfig {
    pub fn new(game_id: impl Into<String>, subdecks: Vec<SubDeck>) -> Self {
        Self { game_id: game_id.into(), subdecks, is_starter: false }
    }

    pub fn subdeck(&self, name: &str) -> Option<&SubDeck> {
        self.subdecks.iter().find(|subdeck| subdeck.name == name)
    }

    pub fn entries_for(&self, name: &str) -> &[DeckEntry] {
        self.subdeck(name).map(|subdeck| subdeck.entries.as_slice()).unwrap_or(&[])
    }

    pub fn card_count_for(&self, name: &str) -> i64 {
        self.entries_for(name).iter().map(|entry| entry.quantity).sum()
    }

    pub fn total_card_count(&self) -> i64 {
        self.subdecks.iter().flat_map(|subdeck| &subdeck.entries).map(|entry| entry.quantity).sum()
    }

    /// One copy of every card in every deck type the game uses.
    pub fn full(game: &GameDefinition) -> Self {
        let one_of_each_card = || game.cards().iter().map(|card| DeckEntry::new(card.id.clone(), 1)).collect();
        let deck_types = game.deck_types();
        let subdecks = if deck_types.is_empty() {
            vec![SubDeck::new(MAIN_DECK_SUBDECK_NAME, one_of_each_card())]
        } else {
            deck_types.into_iter().map(|deck_type| SubDeck::new(deck_type, one_of_each_card())).collect()
        };
        Self::new(game.id.clone(), subdecks)
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "DeckConfig")?;
        if fields.optional_value("subdecks").is_none() {
            return Err(ModelError::LegacyDeckFormat);
        }
        Ok(Self {
            game_id: fields.required_string("gameId")?,
            subdecks: fields.list_of("subdecks", SubDeck::from_json)?,
            is_starter: fields.flag_is_exactly_true("isStarter") || fields.flag_is_exactly_true("IsStarter"),
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("gameId", &self.game_id)
            .list("subdecks", &self.subdecks, SubDeck::to_json)
            .flag_when_true("isStarter", self.is_starter)
            .finish_value()
    }
}

// ---------------------------------------------------------------------------
// ZoneDefinition
// ---------------------------------------------------------------------------

/// A zone: a deck, discard pile, or other card area (or a counter / token bag widget slot).
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneDefinition {
    pub id: String,
    pub name: String,
    pub shared: bool,
    pub deals_built_deck: bool,
    pub deck_type: String,
    pub deck_optional: bool,
    /// Starting contents.
    pub entries: Vec<DeckEntry>,
    pub face_up: bool,
    pub shuffleable: bool,
    pub visible_to_all: bool,
    pub is_discard_pile: bool,
    pub flippable: bool,
    pub auto_shuffle: bool,
    pub standard_deck: bool,
    /// For shared zones: the display name of a library deck to load automatically.
    pub deck_name: Option<String>,
    pub side: SharedZoneSide,
    pub orientation: CardOrientation,
    pub kind: ZoneKind,
    pub widget_kind: ZoneWidgetKind,
    pub counter_starting_value: f64,
    pub counter_starting_color: ArgbColor,
    pub counter_starting_text_color: ArgbColor,
    /// Relative to `_tokens/`.
    pub counter_image_path: Option<String>,
    /// Relative to `_tokens/`.
    pub bag_image_path: Option<String>,
    pub bag_returnable: bool,
    /// Here `definition_id` is a `_tokens/`-relative image path, not a card id.
    pub bag_entries: Vec<DeckEntry>,
}

impl Default for ZoneDefinition {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            shared: false,
            deals_built_deck: false,
            deck_type: MAIN_DECK_SUBDECK_NAME.to_string(),
            deck_optional: false,
            entries: Vec::new(),
            face_up: false,
            shuffleable: true,
            visible_to_all: false,
            is_discard_pile: false,
            flippable: false,
            auto_shuffle: false,
            standard_deck: false,
            deck_name: None,
            side: SharedZoneSide::Left,
            orientation: CardOrientation::Portrait,
            kind: ZoneKind::Card,
            widget_kind: ZoneWidgetKind::Counter,
            counter_starting_value: 0.0,
            counter_starting_color: DEFAULT_BOARD_WIDGET_BACKGROUND_COLOR,
            counter_starting_text_color: DEFAULT_BOARD_WIDGET_TEXT_COLOR,
            counter_image_path: None,
            bag_image_path: None,
            bag_returnable: true,
            bag_entries: Vec::new(),
        }
    }
}

impl ZoneDefinition {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self { id: id.into(), name: name.into(), ..Self::default() }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "ZoneDefinition")?;
        let defaults = Self::default();
        Ok(Self {
            id: fields.required_string("id")?,
            name: fields.required_string("name")?,
            shared: fields.flag_is_exactly_true("shared"),
            deals_built_deck: fields.flag_is_exactly_true("dealsBuiltDeck"),
            deck_type: fields.optional_string("deckType")?.unwrap_or(defaults.deck_type),
            deck_optional: fields.flag_is_exactly_true("deckOptional"),
            entries: fields.list_of("entries", DeckEntry::from_json)?,
            face_up: fields.flag_is_exactly_true("faceUp"),
            shuffleable: fields.bool_or_default("shuffleable", true)?,
            visible_to_all: fields.flag_is_exactly_true("visibleToAll"),
            is_discard_pile: fields.flag_is_exactly_true("isDiscardPile"),
            flippable: fields.flag_is_exactly_true("flippable"),
            auto_shuffle: fields.flag_is_exactly_true("autoShuffle"),
            standard_deck: fields.flag_is_exactly_true("standardDeck"),
            deck_name: fields.optional_string("deckName")?,
            side: fields.enum_or_default("side", SharedZoneSide::Left, "ZoneDefinition.side")?,
            orientation: fields.enum_or_default(
                "orientation",
                CardOrientation::Portrait,
                "ZoneDefinition.orientation",
            )?,
            kind: fields.enum_or_default("kind", ZoneKind::Card, "ZoneDefinition.kind")?,
            widget_kind: fields.enum_or_default("widgetKind", ZoneWidgetKind::Counter, "ZoneDefinition.widgetKind")?,
            counter_starting_value: fields.optional_number("counterStartingValue")?.unwrap_or(0.0),
            counter_starting_color: fields
                .optional_color("counterStartingColor")?
                .unwrap_or(DEFAULT_BOARD_WIDGET_BACKGROUND_COLOR),
            counter_starting_text_color: fields
                .optional_color("counterStartingTextColor")?
                .unwrap_or(DEFAULT_BOARD_WIDGET_TEXT_COLOR),
            counter_image_path: fields.optional_string("counterImagePath")?,
            bag_image_path: fields.optional_string("bagImagePath")?,
            bag_returnable: fields.bool_or_default("bagReturnable", true)?,
            bag_entries: fields.list_of("bagEntries", DeckEntry::from_json)?,
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("id", &self.id)
            .field("name", &self.name)
            .flag_when_true("shared", self.shared)
            .flag_when_true("dealsBuiltDeck", self.deals_built_deck)
            .unless_default("deckType", self.deck_type.as_str(), MAIN_DECK_SUBDECK_NAME)
            .flag_when_true("deckOptional", self.deck_optional)
            .non_empty_list("entries", &self.entries, DeckEntry::to_json)
            .flag_when_true("faceUp", self.face_up)
            .unless_default("shuffleable", self.shuffleable, true)
            .flag_when_true("visibleToAll", self.visible_to_all)
            .flag_when_true("isDiscardPile", self.is_discard_pile)
            .flag_when_true("flippable", self.flippable)
            .flag_when_true("autoShuffle", self.auto_shuffle)
            .flag_when_true("standardDeck", self.standard_deck)
            .optional("deckName", self.deck_name.as_ref())
            .unless_default("side", self.side.as_json_str(), "left")
            .unless_default("orientation", self.orientation.as_json_str(), "portrait")
            .unless_default("kind", self.kind.as_json_str(), "card")
            .unless_default("widgetKind", self.widget_kind.as_json_str(), "counter")
            .unless_default("counterStartingValue", self.counter_starting_value, 0.0)
            .unless_default("counterStartingColor", self.counter_starting_color, DEFAULT_BOARD_WIDGET_BACKGROUND_COLOR)
            .unless_default(
                "counterStartingTextColor",
                self.counter_starting_text_color,
                DEFAULT_BOARD_WIDGET_TEXT_COLOR,
            )
            .optional("counterImagePath", self.counter_image_path.as_ref())
            .optional("bagImagePath", self.bag_image_path.as_ref())
            .unless_default("bagReturnable", self.bag_returnable, true)
            .non_empty_list("bagEntries", &self.bag_entries, DeckEntry::to_json)
            .finish_value()
    }
}

// ---------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------

/// A token's size: a multiplier of the default size, or the image's own pixel size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WidgetSizeOverride {
    Multiplier(f64),
    NativeImageSize,
}

impl WidgetSizeOverride {
    pub const NATIVE_JSON_TEXT: &'static str = "native";

    pub fn parse_optional_field(
        fields: &FieldReader,
        field_name: &str,
        model_kind: &'static str,
    ) -> Result<Option<Self>, ModelError> {
        match fields.optional_value(field_name) {
            None => Ok(None),
            Some(JsonValue::Number(multiplier)) => Ok(Some(Self::Multiplier(*multiplier))),
            Some(JsonValue::String(text)) if text == Self::NATIVE_JSON_TEXT => Ok(Some(Self::NativeImageSize)),
            Some(_) => Err(ModelError::WrongFieldType {
                model_kind,
                field_name: field_name.to_string(),
                expected: "a number or \"native\"",
            }),
        }
    }

    pub fn to_json(self) -> JsonValue {
        match self {
            Self::Multiplier(multiplier) => JsonValue::Number(multiplier),
            Self::NativeImageSize => JsonValue::from(Self::NATIVE_JSON_TEXT),
        }
    }
}

/// A card placed on the table when a scenario loads. Coordinates are table fractions.
#[derive(Debug, Clone, PartialEq)]
pub struct ScenarioPlacement {
    pub definition_id: String,
    pub x: f64,
    pub y: f64,
    pub face_up: bool,
    /// Quarter turns; may be 0.5.
    pub rotation_turns: f64,
}

impl ScenarioPlacement {
    pub fn new(definition_id: impl Into<String>, x: f64, y: f64) -> Self {
        Self { definition_id: definition_id.into(), x, y, face_up: true, rotation_turns: 0.0 }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "ScenarioPlacement")?;
        Ok(Self {
            definition_id: fields.required_string("definitionId")?,
            x: fields.required_number("x")?,
            y: fields.required_number("y")?,
            face_up: fields.bool_or_default("faceUp", true)?,
            rotation_turns: fields.optional_number("rotationTurns")?.unwrap_or(0.0),
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("definitionId", &self.definition_id)
            .field("x", self.x)
            .field("y", self.y)
            .unless_default("faceUp", self.face_up, true)
            .unless_default("rotationTurns", self.rotation_turns, 0.0)
            .finish_value()
    }
}

/// An image token placed on the table when a scenario loads.
#[derive(Debug, Clone, PartialEq)]
pub struct ScenarioTokenPlacement {
    pub image_path: String,
    pub x: f64,
    pub y: f64,
    pub rotation_turns: f64,
    pub size_override: Option<WidgetSizeOverride>,
}

impl ScenarioTokenPlacement {
    pub fn new(image_path: impl Into<String>, x: f64, y: f64) -> Self {
        Self { image_path: image_path.into(), x, y, rotation_turns: 0.0, size_override: None }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "ScenarioTokenPlacement")?;
        Ok(Self {
            image_path: fields.required_string("imagePath")?,
            x: fields.required_number("x")?,
            y: fields.required_number("y")?,
            rotation_turns: fields.optional_number("rotationTurns")?.unwrap_or(0.0),
            size_override: WidgetSizeOverride::parse_optional_field(&fields, "sizeOverride", "ScenarioTokenPlacement")?,
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("imagePath", &self.image_path)
            .field("x", self.x)
            .field("y", self.y)
            .unless_default("rotationTurns", self.rotation_turns, 0.0)
            .optional("sizeOverride", self.size_override.map(WidgetSizeOverride::to_json))
            .finish_value()
    }
}

/// The scale a placed zone is drawn at when the designer hasn't set one.
pub const DEFAULT_PLACED_ZONE_SCALE: f64 = 1.0;

/// One of a scenario's own zones set at a fixed spot on the table (a GameTable addition): once the
/// scenario loads, the zone is drawn there, centred on the table fractions `x`, `y` as the first
/// seat sees them, instead of in a sidebar. Players can't move it. `scale` multiplies its size
/// (the designer's "Set Scale", for widgets; 1 is the usual size).
#[derive(Debug, Clone, PartialEq)]
pub struct ScenarioZonePlacement {
    pub zone_id: String,
    pub x: f64,
    pub y: f64,
    pub scale: f64,
}

impl ScenarioZonePlacement {
    pub fn new(zone_id: impl Into<String>, x: f64, y: f64) -> Self {
        Self { zone_id: zone_id.into(), x, y, scale: DEFAULT_PLACED_ZONE_SCALE }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "ScenarioZonePlacement")?;
        Ok(Self {
            zone_id: fields.required_string("zoneId")?,
            x: fields.required_number("x")?,
            y: fields.required_number("y")?,
            scale: fields.optional_number("scale")?.unwrap_or(DEFAULT_PLACED_ZONE_SCALE),
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("zoneId", &self.zone_id)
            .field("x", self.x)
            .field("y", self.y)
            .unless_default("scale", self.scale, DEFAULT_PLACED_ZONE_SCALE)
            .finish_value()
    }
}

/// A pre-built table setup the game owner can load.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scenario {
    pub id: String,
    pub name: String,
    pub zones: Vec<ZoneDefinition>,
    pub placements: Vec<ScenarioPlacement>,
    pub token_placements: Vec<ScenarioTokenPlacement>,
    /// Its zones set at fixed spots on the table (GameTable addition; `zonePlacements`).
    pub zone_placements: Vec<ScenarioZonePlacement>,
    pub decks: Vec<SubDeck>,
    /// Relative to `_scenarios/`.
    pub guide_path: Option<String>,
}

impl Scenario {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self { id: id.into(), name: name.into(), ..Self::default() }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "Scenario")?;
        Ok(Self {
            id: fields.required_string("id")?,
            name: fields.required_string("name")?,
            zones: fields.list_of("zones", ZoneDefinition::from_json)?,
            placements: fields.list_of("placements", ScenarioPlacement::from_json)?,
            token_placements: fields.list_of("tokenPlacements", ScenarioTokenPlacement::from_json)?,
            zone_placements: fields.list_of("zonePlacements", ScenarioZonePlacement::from_json)?,
            decks: fields.list_of("decks", SubDeck::from_json)?,
            guide_path: fields.optional_string("guidePath")?,
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("id", &self.id)
            .field("name", &self.name)
            .non_empty_list("zones", &self.zones, ZoneDefinition::to_json)
            .non_empty_list("placements", &self.placements, ScenarioPlacement::to_json)
            .non_empty_list("tokenPlacements", &self.token_placements, ScenarioTokenPlacement::to_json)
            .non_empty_list("zonePlacements", &self.zone_placements, ScenarioZonePlacement::to_json)
            .non_empty_list("decks", &self.decks, SubDeck::to_json)
            .optional("guidePath", self.guide_path.as_ref())
            .finish_value()
    }

    pub fn subdeck(&self, name: &str) -> Option<&SubDeck> {
        self.decks.iter().find(|deck| deck.name == name)
    }

    pub fn entries_for(&self, name: &str) -> &[DeckEntry] {
        self.subdeck(name).map(|deck| deck.entries.as_slice()).unwrap_or(&[])
    }

    /// Where on the table this scenario sets its zone, if it does.
    pub fn zone_placement_for(&self, zone_id: &str) -> Option<&ScenarioZonePlacement> {
        self.zone_placements.iter().find(|placement| placement.zone_id == zone_id)
    }
}

// ---------------------------------------------------------------------------
// GameBoardDefinition (the original's playmats)
// ---------------------------------------------------------------------------

/// A zone drawn on a game board instead of in the player area or a sidebar (a GameTable
/// addition). `x` and `y` are the zone's centre as fractions (0..1) of the board, seen the right
/// way up by the board's owner (before any rotation on the table). `scale` multiplies its size
/// (the designer's "Set Scale", for widgets; 1 is the usual size).
#[derive(Debug, Clone, PartialEq)]
pub struct BoardZonePlacement {
    pub zone_id: String,
    pub x: f64,
    pub y: f64,
    pub scale: f64,
}

impl BoardZonePlacement {
    pub fn new(zone_id: impl Into<String>, x: f64, y: f64) -> Self {
        Self { zone_id: zone_id.into(), x, y, scale: DEFAULT_PLACED_ZONE_SCALE }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "BoardZonePlacement")?;
        Ok(Self {
            zone_id: fields.required_string("zoneId")?,
            x: fields.required_number("x")?,
            y: fields.required_number("y")?,
            scale: fields.optional_number("scale")?.unwrap_or(DEFAULT_PLACED_ZONE_SCALE),
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("zoneId", &self.zone_id)
            .field("x", self.x)
            .field("y", self.y)
            .unless_default("scale", self.scale, DEFAULT_PLACED_ZONE_SCALE)
            .finish_value()
    }
}

/// Where the designer put a game board on the table, so it is there whenever the game loads.
/// `x` and `y` are the board's centre in world units (the 2560 x 1440 table): for a shared board,
/// on the table as the host sees it; for a player board, an offset from the player's seat origin
/// (where their seat meets the table's middle line; positive `y` is toward the player), as that
/// player sees it. `rotation` is 0, 90, 180 or 270
/// degrees clockwise. A `toggleable` board is shown and hidden with F12; the others always show.
/// A shared board sits locked in the middle of the table (ignoring `x` and `y`) until it is
/// `unlocked_from_center`; player boards ignore that flag.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardTablePlacement {
    pub x: f64,
    pub y: f64,
    pub rotation: i64,
    pub toggleable: bool,
    pub unlocked_from_center: bool,
}

impl BoardTablePlacement {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y, rotation: 0, toggleable: false, unlocked_from_center: false }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "BoardTablePlacement")?;
        Ok(Self {
            x: fields.required_number("x")?,
            y: fields.required_number("y")?,
            rotation: normalized_rotation(fields.optional_whole_number("rotation")?.unwrap_or(0)),
            toggleable: fields.flag_is_exactly_true("toggleable"),
            unlocked_from_center: fields.flag_is_exactly_true("unlockedFromCenter"),
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("x", self.x)
            .field("y", self.y)
            .unless_default("rotation", self.rotation, 0)
            .flag_when_true("toggleable", self.toggleable)
            .flag_when_true("unlockedFromCenter", self.unlocked_from_center)
            .finish_value()
    }

    /// Turns the board another quarter turn clockwise.
    pub fn rotate_clockwise(&mut self) {
        self.rotation = normalized_rotation(self.rotation + 90);
    }
}

/// A player board placed for one player only (by their place at the table: player 1 is the first
/// seat), at its own spot on the table. It replaces that player's copy from the board's every-seat
/// placement. `x` and `y` are the board's centre in world units on the table as the first seat
/// sees it (players across the table see it turned around with the rest of the table);
/// `rotation` and `toggleable` work as for [`BoardTablePlacement`].
#[derive(Debug, Clone, PartialEq)]
pub struct BoardPlayerPlacement {
    pub player: i64,
    pub x: f64,
    pub y: f64,
    pub rotation: i64,
    pub toggleable: bool,
}

impl BoardPlayerPlacement {
    pub fn new(player: i64, x: f64, y: f64) -> Self {
        Self { player, x, y, rotation: 0, toggleable: false }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "BoardPlayerPlacement")?;
        Ok(Self {
            player: fields.optional_whole_number("player")?.unwrap_or(1).max(1),
            x: fields.required_number("x")?,
            y: fields.required_number("y")?,
            rotation: normalized_rotation(fields.optional_whole_number("rotation")?.unwrap_or(0)),
            toggleable: fields.flag_is_exactly_true("toggleable"),
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("player", self.player)
            .field("x", self.x)
            .field("y", self.y)
            .unless_default("rotation", self.rotation, 0)
            .flag_when_true("toggleable", self.toggleable)
            .finish_value()
    }

    /// Turns the board another quarter turn clockwise.
    pub fn rotate_clockwise(&mut self) {
        self.rotation = normalized_rotation(self.rotation + 90);
    }
}

/// Snaps any angle to 0, 90, 180 or 270.
pub fn normalized_rotation(degrees: i64) -> i64 {
    ((degrees as f64 / 90.0).round() as i64).rem_euclid(4) * 90
}

/// A game board (a playmat, or a board game's board): its image (relative to the game's
/// `_playmats/` folder, named from when boards were playmats), scale, kind, the zones placed on
/// it, and optionally where it sits on the table: `table_placement` (a shared board's spot, or
/// where every player gets their copy of a player board) and, for a player board,
/// `player_placements` for single players.
#[derive(Debug, Clone, PartialEq)]
pub struct GameBoardDefinition {
    pub image_path: String,
    pub scale: f64,
    pub kind: BoardKind,
    pub zone_placements: Vec<BoardZonePlacement>,
    pub table_placement: Option<BoardTablePlacement>,
    pub player_placements: Vec<BoardPlayerPlacement>,
}

impl GameBoardDefinition {
    pub fn new(image_path: impl Into<String>) -> Self {
        Self {
            image_path: image_path.into(),
            scale: 1.0,
            kind: BoardKind::Player,
            zone_placements: Vec::new(),
            table_placement: None,
            player_placements: Vec::new(),
        }
    }

    /// Reads one board; `kind_when_missing` is the old game-wide `playmatType` for a game saved
    /// with `playmats`.
    fn from_json_with_default_kind(value: &JsonValue, kind_when_missing: BoardKind) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "GameBoardDefinition")?;
        Ok(Self {
            image_path: fields.required_string("imagePath")?,
            scale: fields.optional_number("scale")?.unwrap_or(1.0),
            kind: fields.enum_or_default("kind", kind_when_missing, "GameBoardDefinition.kind")?,
            zone_placements: fields.list_of("zonePlacements", BoardZonePlacement::from_json)?,
            table_placement: fields
                .optional_value("tablePlacement")
                .filter(|value| !matches!(value, JsonValue::Null))
                .map(BoardTablePlacement::from_json)
                .transpose()?,
            player_placements: fields.list_of("playerPlacements", BoardPlayerPlacement::from_json)?,
        })
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        GameBoardDefinition::from_json_with_default_kind(value, BoardKind::Player)
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("imagePath", &self.image_path)
            .unless_default("scale", self.scale, 1.0)
            .unless_default("kind", self.kind.as_json_str(), "player")
            .non_empty_list("zonePlacements", &self.zone_placements, BoardZonePlacement::to_json)
            .optional("tablePlacement", self.table_placement.as_ref().map(BoardTablePlacement::to_json))
            .non_empty_list("playerPlacements", &self.player_placements, BoardPlayerPlacement::to_json)
            .finish_value()
    }

    pub fn placement_for(&self, zone_id: &str) -> Option<&BoardZonePlacement> {
        self.zone_placements.iter().find(|placement| placement.zone_id == zone_id)
    }

    /// Placed on the table (for every player, or for some).
    pub fn is_on_table(&self) -> bool {
        self.table_placement.is_some() || !self.player_placements.is_empty()
    }

    /// Placed on the table, and shown or hidden with F12 (anywhere it is placed).
    pub fn is_toggleable_on_table(&self) -> bool {
        self.table_placement.as_ref().is_some_and(|placement| placement.toggleable)
            || self.player_placements.iter().any(|placement| placement.toggleable)
    }

    /// The placement just for `player` (1-based seat), if there is one.
    pub fn player_placement_for(&self, player: i64) -> Option<&BoardPlayerPlacement> {
        self.player_placements.iter().find(|placement| placement.player == player)
    }

    /// For a player board: whether the player in seat `player` (1-based) gets it (their own
    /// placement, else the every-seat one), and if so whether F12 shows and hides it.
    pub fn toggleable_for_player(&self, player: i64) -> Option<bool> {
        if self.kind != BoardKind::Player {
            return None;
        }
        match self.player_placement_for(player) {
            Some(placement) => Some(placement.toggleable),
            None => self.table_placement.as_ref().map(|placement| placement.toggleable),
        }
    }
}

/// `gameBoards`, or for a game saved before boards (and by the original CardTable) its
/// `playmats`, each taking its kind from the old game-wide `playmatType` ("individual" mats are
/// player boards).
fn read_game_boards(fields: &FieldReader) -> Result<Vec<GameBoardDefinition>, ModelError> {
    if fields.optional_value("gameBoards").is_some() {
        return fields.list_of("gameBoards", GameBoardDefinition::from_json);
    }
    let old_kind = match fields.optional_string("playmatType")?.as_deref() {
        Some("shared") => BoardKind::Shared,
        _ => BoardKind::Player,
    };
    fields.list_of("playmats", |value| GameBoardDefinition::from_json_with_default_kind(value, old_kind))
}

/// Whether a zone may sit on this kind of board: player boards hold each player's own zones,
/// shared boards hold shared zones (scenario zones are always shared).
pub fn zone_fits_board_kind(zone: &ZoneDefinition, kind: BoardKind) -> bool {
    match kind {
        BoardKind::Player => !zone.shared,
        BoardKind::Shared => zone.shared,
    }
}

// ---------------------------------------------------------------------------
// GameDefinition
// ---------------------------------------------------------------------------

/// Lazily built `card id -> index` lookup. It never takes part in equality or debug output.
#[derive(Clone, Default)]
struct CardIdIndex(OnceLock<HashMap<String, usize>>);

impl PartialEq for CardIdIndex {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for CardIdIndex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CardIdIndex")
    }
}

/// A whole game (`gamedef.json`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GameDefinition {
    pub id: String,
    pub name: String,
    /// Every card; access through [`cards`](Self::cards) / [`cards_mut`](Self::cards_mut) so the
    /// id lookup stays correct.
    cards: Vec<CardDefinition>,
    card_id_index: CardIdIndex,
    pub card_backs: Vec<CardBackDefinition>,
    pub zones: Vec<ZoneDefinition>,
    pub tag_groups: Vec<TagGroup>,
    pub sets: Vec<GameSet>,
    /// Optional `"#RRGGBB"`; absent defers to the player's or app's default.
    pub table_color: Option<String>,
    /// The original's board border colour. GameTable no longer uses it (player boards are
    /// bordered in their owner's colour, shared boards in a fixed colour) but keeps it, so saving a
    /// game doesn't drop it from the file.
    pub playmat_border_color: Option<String>,
    pub current_turn_color: Option<String>,
    /// The game's boards (the original's `playmats`; GameTable writes them as `gameBoards`).
    pub game_boards: Vec<GameBoardDefinition>,
    pub has_scenarios: bool,
    pub scenarios: Vec<Scenario>,
    pub scenario_card_back_id: Option<String>,
    pub uses_tokens: bool,
    pub tokens: Vec<TokenDefinition>,
    /// True disables mirroring the table for far seats.
    pub same_orientation_for_all: bool,
    /// The folder the game was loaded from. Local only; never written to JSON.
    pub folder_path: Option<String>,
}

impl GameDefinition {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self { id: id.into(), name: name.into(), ..Self::default() }
    }

    pub fn with_cards(mut self, cards: Vec<CardDefinition>) -> Self {
        *self.cards_mut() = cards;
        self
    }

    pub fn cards(&self) -> &[CardDefinition] {
        &self.cards
    }

    /// Mutable access to the cards; clears the id lookup so it is rebuilt on next use.
    pub fn cards_mut(&mut self) -> &mut Vec<CardDefinition> {
        self.card_id_index = CardIdIndex::default();
        &mut self.cards
    }

    /// The first card with this id.
    pub fn card_by_id(&self, card_id: &str) -> Option<&CardDefinition> {
        let index = self.card_id_index.0.get_or_init(|| {
            let mut first_position_of_each_id = HashMap::with_capacity(self.cards.len());
            for (position, card) in self.cards.iter().enumerate() {
                first_position_of_each_id.entry(card.id.clone()).or_insert(position);
            }
            first_position_of_each_id
        });
        index.get(card_id).map(|&position| &self.cards[position])
    }

    /// Zones that deal each player's built deck (not shared).
    pub fn deck_building_zones(&self) -> impl Iterator<Item = &ZoneDefinition> {
        self.zones.iter().filter(|zone| !zone.shared && zone.deals_built_deck)
    }

    /// Deck types of the deck-building zones, in order, without duplicates.
    pub fn deck_types(&self) -> Vec<String> {
        let mut deck_types: Vec<String> = Vec::new();
        for zone in self.deck_building_zones() {
            if !deck_types.contains(&zone.deck_type) {
                deck_types.push(zone.deck_type.clone());
            }
        }
        deck_types
    }

    /// Token types parsed from token names, in order, without duplicates; untyped tokens skipped.
    pub fn token_types(&self) -> Vec<String> {
        let mut token_types: Vec<String> = Vec::new();
        for token_type in self.tokens.iter().filter_map(TokenDefinition::token_type) {
            if !token_types.iter().any(|known| known == token_type) {
                token_types.push(token_type.to_string());
            }
        }
        token_types
    }

    pub fn needs_deck_building(&self) -> bool {
        self.deck_building_zones().next().is_some()
    }

    pub fn zone_by_id(&self, zone_id: &str) -> Option<&ZoneDefinition> {
        self.zones.iter().find(|zone| zone.id == zone_id)
    }

    pub fn scenario_by_id(&self, scenario_id: &str) -> Option<&Scenario> {
        self.scenarios.iter().find(|scenario| scenario.id == scenario_id)
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "GameDefinition")?;
        let mut sets = Vec::new();
        let mut cards = Vec::new();
        if fields.optional_value("sets").is_some() {
            for raw_set in fields.list_of("sets", |raw_set| Ok(raw_set.clone()))? {
                let set = GameSet::from_json(&raw_set)?;
                let set_fields = FieldReader::new(&raw_set, "GameSet")?;
                for card in set_fields.required_list_of("cards", CardDefinition::from_json)? {
                    cards.push(CardDefinition { set_id: Some(set.id.clone()), ..card });
                }
                sets.push(set);
            }
        } else {
            cards = fields.required_list_of("cards", CardDefinition::from_json)?;
        }

        let card_backs = if fields.optional_value("cardBacks").is_some() {
            fields.list_of("cardBacks", CardBackDefinition::from_json)?
        } else if let Some(legacy_back_path) = fields.optional_string("cardBackImagePath")? {
            // A file from before games could have several backs: its single back becomes the default.
            vec![CardBackDefinition {
                id: "default".to_string(),
                name: "Default".to_string(),
                image_path: Some(legacy_back_path),
                orientation: CardOrientation::Portrait,
            }]
        } else {
            Vec::new()
        };

        let id = fields.required_string("id")?;
        let name = fields.required_string("name")?;
        Ok(Self {
            id,
            name,
            cards,
            card_id_index: CardIdIndex::default(),
            card_backs,
            zones: fields.list_of("zones", ZoneDefinition::from_json)?,
            tag_groups: fields.list_of("tagGroups", TagGroup::from_json)?,
            sets,
            table_color: fields.optional_string("tableColor")?,
            playmat_border_color: fields.optional_string("playmatBorderColor")?,
            current_turn_color: fields.optional_string("currentTurnColor")?,
            game_boards: read_game_boards(&fields)?,
            has_scenarios: fields.flag_is_exactly_true("hasScenarios"),
            scenarios: fields.list_of("scenarios", Scenario::from_json)?,
            scenario_card_back_id: fields.optional_string("scenarioCardBackId")?,
            uses_tokens: fields.flag_is_exactly_true("usesTokens"),
            tokens: fields.list_of("tokens", TokenDefinition::from_json)?,
            same_orientation_for_all: fields.flag_is_exactly_true("sameOrientationForAll"),
            folder_path: None,
        })
    }

    /// When the game has sets, cards are written nested in their set (cards whose set no longer
    /// exists are dropped); otherwise as a flat `cards` list.
    pub fn to_json(&self) -> JsonValue {
        let mut writer = SparseObjectWriter::new().field("id", &self.id).field("name", &self.name);
        if self.sets.is_empty() {
            writer = writer.list("cards", &self.cards, |card| card.to_json(false));
        } else {
            let sets_json: Vec<JsonValue> = self
                .sets
                .iter()
                .map(|set| {
                    let mut set_json = set.to_json_without_cards();
                    let set_cards: Vec<JsonValue> = self
                        .cards
                        .iter()
                        .filter(|card| card.set_id.as_deref() == Some(set.id.as_str()))
                        .map(|card| card.to_json(true))
                        .collect();
                    set_json.insert("cards", JsonValue::Array(set_cards));
                    JsonValue::Object(set_json)
                })
                .collect();
            writer = writer.field("sets", JsonValue::Array(sets_json));
        }
        writer
            .non_empty_list("cardBacks", &self.card_backs, CardBackDefinition::to_json)
            .non_empty_list("zones", &self.zones, ZoneDefinition::to_json)
            .non_empty_list("tagGroups", &self.tag_groups, TagGroup::to_json)
            .optional("tableColor", self.table_color.as_ref())
            .optional("playmatBorderColor", self.playmat_border_color.as_ref())
            .optional("currentTurnColor", self.current_turn_color.as_ref())
            .non_empty_list("gameBoards", &self.game_boards, GameBoardDefinition::to_json)
            .flag_when_true("hasScenarios", self.has_scenarios)
            .non_empty_list("scenarios", &self.scenarios, Scenario::to_json)
            .optional("scenarioCardBackId", self.scenario_card_back_id.as_ref())
            .flag_when_true("usesTokens", self.uses_tokens)
            .non_empty_list("tokens", &self.tokens, TokenDefinition::to_json)
            .flag_when_true("sameOrientationForAll", self.same_orientation_for_all)
            .finish_value()
    }
}

// ---------------------------------------------------------------------------
// Standard 52-card deck
// ---------------------------------------------------------------------------

/// `(id part, tag, colour)` for each suit, in deck order.
const STANDARD_SUITS: [(&str, &str, &str); 4] = [
    ("hearts", "Hearts", "#D32F2F"),
    ("diamonds", "Diamonds", "#D32F2F"),
    ("clubs", "Clubs", "#212121"),
    ("spades", "Spades", "#212121"),
];
const STANDARD_RANKS: [&str; 13] = ["A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K"];

/// The 52 generated cards appended to games that have a `standardDeck` zone.
pub fn build_standard_deck_cards() -> Vec<CardDefinition> {
    let mut cards = Vec::with_capacity(52);
    for (suit_id, suit_tag, suit_color) in STANDARD_SUITS {
        for rank in STANDARD_RANKS {
            cards.push(CardDefinition {
                id: format!("{STANDARD_DECK_ID_PREFIX}{suit_id}_{rank}"),
                card_title: rank.to_string(),
                color_hex: Some(suit_color.to_string()),
                suit: Some(suit_id.to_string()),
                rank: Some(rank.to_string()),
                types: vec![suit_tag.to_string()],
                unownable: true,
                ..CardDefinition::default()
            });
        }
    }
    cards
}
