//! Data models. Port of `src/core/models.lua`.
//!
//! Each model has `from_json` and `to_json`. `to_json` reproduces the original's sparse JSON
//! exactly: fields equal to their default are omitted and keys are written in the original order
//! (which itself mirrors the Dart app that came before CardTable).
//!
//! Reading is as lenient as the Lua code where that matters for real files (booleans count only
//! when exactly `true`, `null` means absent) and strict where the Lua code raised errors (missing
//! required fields, unknown enum values). A field holding the wrong JSON type is an error; the
//! Dart app that produced these files could never write one.

mod definitions;
mod json_fields;
mod state;
mod timestamps;

pub use definitions::*;
pub use state::*;
pub use timestamps::*;

use std::fmt;

/// Card id sent to players who must not see a card's identity.
pub const HIDDEN_DEFINITION_ID: &str = "__hidden__";
/// `cardBackId` value meaning "use the `<image>_BACK` file next to the card image".
pub const UNIQUE_CARD_BACK_ID: &str = "__unique__";
/// Suffix of per-card back images, e.g. `goblin.png` -> `goblin_BACK.png`.
pub const CARD_BACK_FILE_SUFFIX: &str = "_BACK";
/// Prefix of the generated standard 52-card deck ids.
pub const STANDARD_DECK_ID_PREFIX: &str = "__standard52_";

pub const BOARD_WIDGET_COUNTER_MINIMUM: f64 = 0.0;
pub const BOARD_WIDGET_COUNTER_MAXIMUM: f64 = 99999.0;
pub const DEFAULT_BOARD_WIDGET_BACKGROUND_COLOR: crate::color::ArgbColor = 0xFF455A64;
pub const DEFAULT_BOARD_WIDGET_TEXT_COLOR: crate::color::ArgbColor = 0xFFFFFFFF;

/// Why a JSON document could not be read as a model.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelError {
    /// `"<Kind>: missing required field '<field>'"`, as in Lua.
    MissingRequiredField { model_kind: &'static str, field_name: &'static str },
    /// `"<Kind.field>: invalid value '<value>'"`, as in Lua.
    InvalidEnumValue { qualified_field_name: String, value_text: String },
    /// A field held a JSON type the model cannot use (for example a number where text belongs).
    WrongFieldType { model_kind: &'static str, field_name: String, expected: &'static str },
    /// A deck file from before decks had subdecks; it must be rebuilt in the deck editor.
    LegacyDeckFormat,
}

impl ModelError {
    /// The user-facing text the original shows for an old-format deck.
    pub const LEGACY_DECK_FORMAT_MESSAGE: &'static str =
        "This deck was saved in an older format and needs to be rebuilt in the Deck Editor.";
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::MissingRequiredField { model_kind, field_name } => {
                write!(formatter, "{model_kind}: missing required field '{field_name}'")
            }
            ModelError::InvalidEnumValue { qualified_field_name, value_text } => {
                write!(formatter, "{qualified_field_name}: invalid value '{value_text}'")
            }
            ModelError::WrongFieldType { model_kind, field_name, expected } => {
                write!(formatter, "{model_kind}: field '{field_name}' must be {expected}")
            }
            ModelError::LegacyDeckFormat => formatter.write_str(Self::LEGACY_DECK_FORMAT_MESSAGE),
        }
    }
}

impl std::error::Error for ModelError {}

/// Declares an enum stored in JSON as one of a fixed set of strings.
macro_rules! json_string_enum {
    (
        $(#[$attribute:meta])*
        $name:ident { $($variant:ident => $text:literal),+ $(,)? }
    ) => {
        $(#[$attribute])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The exact string used in JSON files and on the wire.
            pub fn as_json_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            pub fn from_json_str(text: &str) -> Option<Self> {
                match text {
                    $($text => Some($name::$variant),)+
                    _ => None,
                }
            }
        }

        impl json_fields::JsonStringEnum for $name {
            fn from_json_str(text: &str) -> Option<Self> {
                $name::from_json_str(text)
            }
        }
    };
}

json_string_enum!(
    /// Which way a card (or card back) image is drawn.
    CardOrientation { Portrait => "portrait", Left => "left", Right => "right" }
);
json_string_enum!(
    /// Where a card instance currently lives.
    CardZone { Table => "table", Hand => "hand", Zone => "zone" }
);
json_string_enum!(
    /// The kinds of objects that can sit on the table besides cards.
    BoardWidgetKind {
        SimpleCounter => "simpleCounter",
        Token => "token",
        ImageToken => "imageToken",
        CardToken => "cardToken",
        Arrow => "arrow",
        PackGenerator => "packGenerator",
        DeckBuilder => "deckBuilder",
        TokenBag => "tokenBag",
    }
);
json_string_enum!(
    /// Which side of the table a shared zone is drawn on.
    SharedZoneSide { Left => "left", Right => "right" }
);
json_string_enum!(
    /// Whether a zone holds cards or a widget.
    ZoneKind { Card => "card", Widget => "widget" }
);
json_string_enum!(
    /// The widget a widget zone holds.
    ZoneWidgetKind { Counter => "counter", TokenBag => "tokenBag" }
);
json_string_enum!(
    /// Whether a player hosts the game or joined it.
    PlayerRole { Host => "host", Client => "client" }
);
json_string_enum!(
    /// What a player is searching through.
    SearchTargetType { Zone => "zone", Pile => "pile" }
);
json_string_enum!(
    /// Whether a game board is one per player (each player gets their own copy) or one shared
    /// by everyone.
    BoardKind { Player => "player", Shared => "shared" }
);
