//! Live table state: card instances, widgets, players, searches, the game log, and saved games.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::definitions::{WidgetSizeOverride, ZoneDefinition, normalized_rotation};
use super::json_fields::{FieldReader, SparseObjectWriter, string_list_to_json};
use super::{
    BoardWidgetKind, CardZone, DEFAULT_BOARD_WIDGET_BACKGROUND_COLOR, DEFAULT_BOARD_WIDGET_TEXT_COLOR, ModelError,
    PlayerRole, SearchTargetType,
};
use crate::color::ArgbColor;
use crate::json::JsonValue;

// ---------------------------------------------------------------------------
// CardInstance
// ---------------------------------------------------------------------------

/// One physical card on the table, in a hand, or in a zone.
#[derive(Debug, Clone, PartialEq)]
pub struct CardInstance {
    pub instance_id: String,
    pub definition_id: String,
    /// Table fractions (0..1, may go up to half a table outside).
    pub x: f64,
    pub y: f64,
    pub z_index: i64,
    pub face_up: bool,
    pub zone: CardZone,
    pub owner_id: Option<String>,
    /// Piles are chains: each card points at the card below it.
    pub stack_parent_id: Option<String>,
    pub zone_id: Option<String>,
    /// Quarter turns; may be fractional (Ctrl gives 45° steps).
    pub rotation_turns: f64,
    pub unownable: bool,
}

impl CardInstance {
    /// A face-up card on the table at the given position with every optional field empty.
    pub fn new(instance_id: impl Into<String>, definition_id: impl Into<String>, zone: CardZone) -> Self {
        Self {
            instance_id: instance_id.into(),
            definition_id: definition_id.into(),
            x: 0.5,
            y: 0.5,
            z_index: 0,
            face_up: true,
            zone,
            owner_id: None,
            stack_parent_id: None,
            zone_id: None,
            rotation_turns: 0.0,
            unownable: false,
        }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "CardInstance")?;
        Ok(Self {
            instance_id: fields.required_string("instanceId")?,
            definition_id: fields.required_string("definitionId")?,
            x: fields.required_number("x")?,
            y: fields.required_number("y")?,
            z_index: fields.required_whole_number("zIndex")?,
            face_up: fields.required_bool("faceUp")?,
            zone: fields.required_enum("zone", "CardInstance.zone")?,
            owner_id: fields.optional_string("ownerId")?,
            stack_parent_id: fields.optional_string("stackParentId")?,
            zone_id: fields.optional_string("zoneId")?,
            rotation_turns: fields.optional_number("rotationTurns")?.unwrap_or(0.0),
            unownable: fields.flag_is_exactly_true("unownable"),
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("instanceId", &self.instance_id)
            .field("definitionId", &self.definition_id)
            .field("x", self.x)
            .field("y", self.y)
            .field("zIndex", self.z_index)
            .field("faceUp", self.face_up)
            .field("zone", self.zone.as_json_str())
            .optional("ownerId", self.owner_id.as_ref())
            .optional("stackParentId", self.stack_parent_id.as_ref())
            .optional("zoneId", self.zone_id.as_ref())
            .unless_default("rotationTurns", self.rotation_turns, 0.0)
            .flag_when_true("unownable", self.unownable)
            .finish_value()
    }
}

// ---------------------------------------------------------------------------
// BoardWidgetInstance
// ---------------------------------------------------------------------------

/// A token drawn from a bag and shown beside it until resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct RevealedToken {
    pub slot_id: String,
    pub image_path: String,
}

impl RevealedToken {
    pub fn new(slot_id: impl Into<String>, image_path: impl Into<String>) -> Self {
        Self { slot_id: slot_id.into(), image_path: image_path.into() }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "RevealedToken")?;
        Ok(Self { slot_id: fields.required_string("slotId")?, image_path: fields.required_string("imagePath")? })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new().field("slotId", &self.slot_id).field("imagePath", &self.image_path).finish_value()
    }
}

/// A counter, token, arrow, pack generator, deck widget or token bag on the table.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardWidgetInstance {
    pub instance_id: String,
    pub kind: BoardWidgetKind,
    pub x: f64,
    pub y: f64,
    pub z_index: i64,
    /// Counter value (0..=99999).
    pub value: f64,
    pub background_color: ArgbColor,
    pub text_color: ArgbColor,
    pub attached_card_id: Option<String>,
    pub attach_offset_x: f64,
    pub attach_offset_y: f64,
    /// Arrow end point.
    pub x2: Option<f64>,
    pub y2: Option<f64>,
    pub creator_id: Option<String>,
    pub owner_id: Option<String>,
    /// Set when the widget is docked in a widget zone.
    pub zone_id: Option<String>,
    /// Relative to `_tokens/`.
    pub image_path: Option<String>,
    pub size_override: Option<WidgetSizeOverride>,
    pub rotation_turns: f64,
    pub face_up: bool,
    /// Token bag contents still to draw (`_tokens/`-relative paths).
    pub bag_remaining: Vec<String>,
    pub bag_revealed: Vec<RevealedToken>,
    pub text: String,
}

impl BoardWidgetInstance {
    pub fn new(instance_id: impl Into<String>, kind: BoardWidgetKind, x: f64, y: f64, z_index: i64) -> Self {
        Self {
            instance_id: instance_id.into(),
            kind,
            x,
            y,
            z_index,
            value: 0.0,
            background_color: DEFAULT_BOARD_WIDGET_BACKGROUND_COLOR,
            text_color: DEFAULT_BOARD_WIDGET_TEXT_COLOR,
            attached_card_id: None,
            attach_offset_x: 0.0,
            attach_offset_y: 0.0,
            x2: None,
            y2: None,
            creator_id: None,
            owner_id: None,
            zone_id: None,
            image_path: None,
            size_override: None,
            rotation_turns: 0.0,
            face_up: true,
            bag_remaining: Vec::new(),
            bag_revealed: Vec::new(),
            text: String::new(),
        }
    }

    pub fn is_docked(&self) -> bool {
        self.zone_id.is_some()
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "BoardWidgetInstance")?;
        Ok(Self {
            instance_id: fields.required_string("instanceId")?,
            kind: fields.required_enum("kind", "BoardWidgetInstance.kind")?,
            x: fields.required_number("x")?,
            y: fields.required_number("y")?,
            z_index: fields.required_whole_number("zIndex")?,
            value: fields.optional_number("value")?.unwrap_or(0.0),
            background_color: fields
                .optional_color("backgroundColor")?
                .unwrap_or(DEFAULT_BOARD_WIDGET_BACKGROUND_COLOR),
            text_color: fields.optional_color("textColor")?.unwrap_or(DEFAULT_BOARD_WIDGET_TEXT_COLOR),
            attached_card_id: fields.optional_string("attachedCardId")?,
            attach_offset_x: fields.optional_number("attachOffsetX")?.unwrap_or(0.0),
            attach_offset_y: fields.optional_number("attachOffsetY")?.unwrap_or(0.0),
            x2: fields.optional_number("x2")?,
            y2: fields.optional_number("y2")?,
            creator_id: fields.optional_string("creatorId")?,
            owner_id: fields.optional_string("ownerId")?,
            zone_id: fields.optional_string("zoneId")?,
            image_path: fields.optional_string("imagePath")?,
            size_override: WidgetSizeOverride::parse_optional_field(&fields, "sizeOverride", "BoardWidgetInstance")?,
            rotation_turns: fields.optional_number("rotationTurns")?.unwrap_or(0.0),
            face_up: fields.bool_or_default("faceUp", true)?,
            bag_remaining: fields.string_list("bagRemaining")?,
            bag_revealed: fields.list_of("bagRevealed", RevealedToken::from_json)?,
            text: fields.optional_string("text")?.unwrap_or_default(),
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("instanceId", &self.instance_id)
            .field("kind", self.kind.as_json_str())
            .field("x", self.x)
            .field("y", self.y)
            .field("zIndex", self.z_index)
            .unless_default("value", self.value, 0.0)
            .unless_default("backgroundColor", self.background_color, DEFAULT_BOARD_WIDGET_BACKGROUND_COLOR)
            .unless_default("textColor", self.text_color, DEFAULT_BOARD_WIDGET_TEXT_COLOR)
            .optional("attachedCardId", self.attached_card_id.as_ref())
            .unless_default("attachOffsetX", self.attach_offset_x, 0.0)
            .unless_default("attachOffsetY", self.attach_offset_y, 0.0)
            .optional("x2", self.x2)
            .optional("y2", self.y2)
            .optional("creatorId", self.creator_id.as_ref())
            .optional("ownerId", self.owner_id.as_ref())
            .optional("zoneId", self.zone_id.as_ref())
            .optional("imagePath", self.image_path.as_ref())
            .optional("sizeOverride", self.size_override.map(WidgetSizeOverride::to_json))
            .unless_default("rotationTurns", self.rotation_turns, 0.0)
            .unless_default("faceUp", self.face_up, true)
            .optional(
                "bagRemaining",
                (!self.bag_remaining.is_empty()).then(|| string_list_to_json(&self.bag_remaining)),
            )
            .non_empty_list("bagRevealed", &self.bag_revealed, RevealedToken::to_json)
            .unless_default("text", self.text.as_str(), "")
            .finish_value()
    }
}

// ---------------------------------------------------------------------------
// PlayerInfo / ActiveSearch / LogEntry
// ---------------------------------------------------------------------------

/// Where a player moved a zone on a playmat (a GameTable addition): the zone's centre as
/// fractions of the mat (`board_index` is 1-based, like the playmat choice). It replaces the
/// game designer's position while that mat is in use.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardZonePosition {
    pub board_index: i64,
    pub zone_id: String,
    pub x: f64,
    pub y: f64,
}

impl BoardZonePosition {
    pub fn new(board_index: i64, zone_id: impl Into<String>, x: f64, y: f64) -> Self {
        Self { board_index, zone_id: zone_id.into(), x, y }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "BoardZonePosition")?;
        Ok(Self {
            board_index: fields.optional_whole_number("playmatIndex")?.unwrap_or(0),
            zone_id: fields.required_string("zoneId")?,
            x: fields.required_number("x")?,
            y: fields.required_number("y")?,
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("playmatIndex", self.board_index)
            .field("zoneId", &self.zone_id)
            .field("x", self.x)
            .field("y", self.y)
            .finish_value()
    }
}

/// Replaces the moved zone positions for one playmat with `positions` (fractions clamped to the
/// mat), keeping those for other mats.
pub fn replace_board_zone_positions(
    stored: &mut Vec<BoardZonePosition>,
    board_index: i64,
    positions: Vec<BoardZonePosition>,
) {
    stored.retain(|position| position.board_index != board_index);
    stored.extend(positions.into_iter().filter(|position| position.x.is_finite() && position.y.is_finite()).map(
        |position| BoardZonePosition {
            board_index,
            x: position.x.clamp(0.0, 1.0),
            y: position.y.clamp(0.0, 1.0),
            ..position
        },
    ));
}

/// Where a player moved one of their own boards during the game (a GameTable addition): its centre
/// as fractions of the table as the first seat sees it, and its quarter turns (0, 90, 180 or 270
/// degrees clockwise, as the first seat sees it). It replaces where the game puts that board.
#[derive(Debug, Clone, PartialEq)]
pub struct MovedBoard {
    pub board_index: i64,
    pub x: f64,
    pub y: f64,
    pub rotation: i64,
}

impl MovedBoard {
    pub fn new(board_index: i64, x: f64, y: f64, rotation: i64) -> Self {
        Self { board_index, x, y, rotation }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "MovedBoard")?;
        Ok(Self {
            board_index: fields.optional_whole_number("boardIndex")?.unwrap_or(0),
            x: fields.required_number("x")?,
            y: fields.required_number("y")?,
            rotation: normalized_rotation(fields.optional_whole_number("rotation")?.unwrap_or(0)),
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("boardIndex", self.board_index)
            .field("x", self.x)
            .field("y", self.y)
            .unless_default("rotation", self.rotation, 0)
            .finish_value()
    }

    /// Kept on the table and turned a whole quarter; `None` when the position isn't a number.
    pub fn sanitized(self) -> Option<MovedBoard> {
        (self.x.is_finite() && self.y.is_finite()).then(|| MovedBoard {
            x: self.x.clamp(0.0, 1.0),
            y: self.y.clamp(0.0, 1.0),
            rotation: normalized_rotation(self.rotation),
            ..self
        })
    }
}

/// Where a board lies on the table as the first seat sees it: its centre as table fractions and
/// its turn in degrees clockwise (0, 90, 180 or 270).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoardSpot {
    pub x: f64,
    pub y: f64,
    pub rotation: i64,
}

/// The cards and tokens lying on a board that its player moved, and the board's spot before and
/// after the move, so they keep their place on the board (sent with `requestSetMovedBoard` as
/// `carried`). The player only picks which items lie on the board; the session decides which of
/// them they may move.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardCarry {
    pub from: BoardSpot,
    pub to: BoardSpot,
    pub card_ids: Vec<String>,
    pub widget_ids: Vec<String>,
}

impl BoardCarry {
    pub fn to_json(&self) -> JsonValue {
        let ids_to_json =
            |ids: &[String]| JsonValue::Array(ids.iter().map(|id| JsonValue::from(id.as_str())).collect());
        let mut object = crate::json::JsonObject::new();
        object.insert("fromX", self.from.x);
        object.insert("fromY", self.from.y);
        object.insert("fromRotation", self.from.rotation);
        object.insert("toX", self.to.x);
        object.insert("toY", self.to.y);
        object.insert("toRotation", self.to.rotation);
        object.insert("cardIds", ids_to_json(&self.card_ids));
        object.insert("widgetIds", ids_to_json(&self.widget_ids));
        JsonValue::Object(object)
    }

    /// `None` when a spot is missing or not a finite number. Non-text ids are skipped.
    pub fn from_json(value: &JsonValue) -> Option<BoardCarry> {
        let number = |key: &str| value.get(key).and_then(JsonValue::as_f64).filter(|number| number.is_finite());
        let rotation = |key: &str| normalized_rotation(number(key).unwrap_or(0.0).round() as i64);
        let ids = |key: &str| -> Vec<String> {
            value
                .get(key)
                .and_then(JsonValue::as_array)
                .map_or_else(Vec::new, |items| items.iter().filter_map(JsonValue::as_str).map(str::to_string).collect())
        };
        Some(BoardCarry {
            from: BoardSpot { x: number("fromX")?, y: number("fromY")?, rotation: rotation("fromRotation") },
            to: BoardSpot { x: number("toX")?, y: number("toY")?, rotation: rotation("toRotation") },
            card_ids: ids("cardIds"),
            widget_ids: ids("widgetIds"),
        })
    }
}

/// A seat at the table.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerInfo {
    pub id: String,
    pub name: String,
    pub role: PlayerRole,
    pub color: ArgbColor,
    pub connected: bool,
    pub player_boards_shown: bool,
    pub chosen_player_board: i64,
    /// Zones this player moved on their own playmats.
    pub board_zone_positions: Vec<BoardZonePosition>,
    /// Their own boards they moved or turned during the game.
    pub moved_boards: Vec<MovedBoard>,
}

impl PlayerInfo {
    pub fn new(id: impl Into<String>, name: impl Into<String>, role: PlayerRole, color: ArgbColor) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            role,
            color,
            connected: true,
            player_boards_shown: false,
            chosen_player_board: 0,
            board_zone_positions: Vec::new(),
            moved_boards: Vec::new(),
        }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "PlayerInfo")?;
        Ok(Self {
            id: fields.required_string("id")?,
            name: fields.required_string("name")?,
            role: fields.required_enum("role", "PlayerInfo.role")?,
            color: fields.required_color("color")?,
            connected: fields.bool_or_default("connected", true)?,
            player_boards_shown: fields.flag_is_exactly_true("individualPlaymatOn"),
            chosen_player_board: fields.optional_whole_number("individualPlaymatIndex")?.unwrap_or(0),
            board_zone_positions: fields.list_of("playmatZonePositions", BoardZonePosition::from_json)?,
            moved_boards: fields.list_of("movedBoards", MovedBoard::from_json)?,
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("id", &self.id)
            .field("name", &self.name)
            .field("role", self.role.as_json_str())
            .field("color", self.color)
            .field("connected", self.connected)
            .flag_when_true("individualPlaymatOn", self.player_boards_shown)
            .unless_default("individualPlaymatIndex", self.chosen_player_board, 0)
            .non_empty_list("playmatZonePositions", &self.board_zone_positions, BoardZonePosition::to_json)
            .non_empty_list("movedBoards", &self.moved_boards, MovedBoard::to_json)
            .finish_value()
    }
}

/// A player looking through a zone or pile.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveSearch {
    pub searcher_id: String,
    pub target_type: SearchTargetType,
    pub target_id: String,
    pub target_owner_id: Option<String>,
}

impl ActiveSearch {
    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "ActiveSearch")?;
        Ok(Self {
            searcher_id: fields.required_string("searcherId")?,
            target_type: fields.required_enum("targetType", "ActiveSearch.targetType")?,
            target_id: fields.required_string("targetId")?,
            target_owner_id: fields.optional_string("targetOwnerId")?,
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("searcherId", &self.searcher_id)
            .field("targetType", self.target_type.as_json_str())
            .field("targetId", &self.target_id)
            .optional("targetOwnerId", self.target_owner_id.as_ref())
            .finish_value()
    }
}

/// One line of the game log, timed from the start of the session.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub elapsed_milliseconds: i64,
    pub message: String,
}

impl LogEntry {
    pub fn new(elapsed_milliseconds: i64, message: impl Into<String>) -> Self {
        Self { elapsed_milliseconds, message: message.into() }
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "LogEntry")?;
        Ok(Self {
            elapsed_milliseconds: fields.required_whole_number("elapsedMs")?,
            message: fields.required_string("message")?,
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("elapsedMs", self.elapsed_milliseconds)
            .field("message", &self.message)
            .finish_value()
    }
}

// ---------------------------------------------------------------------------
// TableState
// ---------------------------------------------------------------------------

/// Lazily built `instance id -> index` lookups. Never part of equality or debug output.
#[derive(Clone, Default)]
struct InstanceIdIndex(OnceLock<HashMap<String, usize>>);

impl InstanceIdIndex {
    fn lookup<'items, T>(
        &self,
        items: &'items [T],
        instance_id_of: impl Fn(&T) -> &str,
        wanted_id: &str,
    ) -> Option<&'items T> {
        let index = self.0.get_or_init(|| {
            // Later duplicates win, as with Lua's `idx[c.instanceId] = c`.
            items.iter().enumerate().map(|(position, item)| (instance_id_of(item).to_string(), position)).collect()
        });
        index.get(wanted_id).map(|&position| &items[position])
    }
}

impl PartialEq for InstanceIdIndex {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for InstanceIdIndex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InstanceIdIndex")
    }
}

/// Everything on the table at one moment. Every change bumps `revision`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableState {
    pub game_id: String,
    pub players: Vec<PlayerInfo>,
    cards: Vec<CardInstance>,
    card_index: InstanceIdIndex,
    pub revision: i64,
    widgets: Vec<BoardWidgetInstance>,
    widget_index: InstanceIdIndex,
    pub searches: Vec<ActiveSearch>,
    pub log: Vec<LogEntry>,
    /// Zones added by loaded scenarios.
    pub loaded_zones: Vec<ZoneDefinition>,
    pub loaded_scenario_ids: Vec<String>,
    pub shared_boards_shown: bool,
    pub chosen_shared_board: i64,
    /// Zones the host moved on the shared playmats.
    pub shared_board_zone_positions: Vec<BoardZonePosition>,
    pub current_player_id: Option<String>,
}

impl TableState {
    pub fn new(game_id: impl Into<String>) -> Self {
        Self { game_id: game_id.into(), ..Self::default() }
    }

    pub fn with_cards(mut self, cards: Vec<CardInstance>) -> Self {
        *self.cards_mut() = cards;
        self
    }

    pub fn with_widgets(mut self, widgets: Vec<BoardWidgetInstance>) -> Self {
        *self.widgets_mut() = widgets;
        self
    }

    pub fn cards(&self) -> &[CardInstance] {
        &self.cards
    }

    /// Mutable access to the cards; clears the id lookup so it is rebuilt on next use.
    pub fn cards_mut(&mut self) -> &mut Vec<CardInstance> {
        self.card_index = InstanceIdIndex::default();
        &mut self.cards
    }

    pub fn widgets(&self) -> &[BoardWidgetInstance] {
        &self.widgets
    }

    /// Mutable access to the widgets; clears the id lookup so it is rebuilt on next use.
    pub fn widgets_mut(&mut self) -> &mut Vec<BoardWidgetInstance> {
        self.widget_index = InstanceIdIndex::default();
        &mut self.widgets
    }

    pub fn card_by_id(&self, instance_id: &str) -> Option<&CardInstance> {
        self.card_index.lookup(&self.cards, |card| &card.instance_id, instance_id)
    }

    pub fn widget_by_id(&self, instance_id: &str) -> Option<&BoardWidgetInstance> {
        self.widget_index.lookup(&self.widgets, |widget| &widget.instance_id, instance_id)
    }

    pub fn player_by_id(&self, player_id: &str) -> Option<&PlayerInfo> {
        self.players.iter().find(|player| player.id == player_id)
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "TableState")?;
        Ok(Self {
            game_id: fields.required_string("gameId")?,
            players: fields.required_list_of("players", PlayerInfo::from_json)?,
            cards: fields.required_list_of("cards", CardInstance::from_json)?,
            card_index: InstanceIdIndex::default(),
            revision: fields.required_whole_number("revision")?,
            widgets: fields.list_of("widgets", BoardWidgetInstance::from_json)?,
            widget_index: InstanceIdIndex::default(),
            searches: fields.list_of("searches", ActiveSearch::from_json)?,
            log: fields.list_of("log", LogEntry::from_json)?,
            loaded_zones: fields.list_of("loadedZones", ZoneDefinition::from_json)?,
            loaded_scenario_ids: fields.string_list("loadedScenarioIds")?,
            shared_boards_shown: fields.flag_is_exactly_true("sharedPlaymatOn"),
            chosen_shared_board: fields.optional_whole_number("sharedPlaymatIndex")?.unwrap_or(0),
            shared_board_zone_positions: fields.list_of("sharedBoardZonePositions", BoardZonePosition::from_json)?,
            current_player_id: fields.optional_string("currentPlayerId")?,
        })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new()
            .field("gameId", &self.game_id)
            .list("players", &self.players, PlayerInfo::to_json)
            .list("cards", &self.cards, CardInstance::to_json)
            .field("revision", self.revision)
            .non_empty_list("widgets", &self.widgets, BoardWidgetInstance::to_json)
            .non_empty_list("searches", &self.searches, ActiveSearch::to_json)
            .non_empty_list("log", &self.log, LogEntry::to_json)
            .non_empty_list("loadedZones", &self.loaded_zones, ZoneDefinition::to_json)
            .optional(
                "loadedScenarioIds",
                (!self.loaded_scenario_ids.is_empty()).then(|| string_list_to_json(&self.loaded_scenario_ids)),
            )
            .flag_when_true("sharedPlaymatOn", self.shared_boards_shown)
            .unless_default("sharedPlaymatIndex", self.chosen_shared_board, 0)
            .non_empty_list("sharedBoardZonePositions", &self.shared_board_zone_positions, BoardZonePosition::to_json)
            .optional("currentPlayerId", self.current_player_id.as_ref())
            .finish_value()
    }
}

// ---------------------------------------------------------------------------
// SavedGame
// ---------------------------------------------------------------------------

/// A saved game file (`<game>/savegames/<name>.json`).
#[derive(Debug, Clone, PartialEq)]
pub struct SavedGame {
    /// Local time as written by Dart's `toIso8601String`, e.g. `"2026-01-02T03:04:05.678"`.
    pub saved_at: String,
    pub state: TableState,
}

impl SavedGame {
    pub fn from_json(value: &JsonValue) -> Result<Self, ModelError> {
        let fields = FieldReader::new(value, "SavedGame")?;
        let saved_at = fields.required_string("savedAt")?;
        let state = TableState::from_json(fields.required_value("state")?)?;
        Ok(Self { saved_at, state })
    }

    pub fn to_json(&self) -> JsonValue {
        SparseObjectWriter::new().field("savedAt", &self.saved_at).field("state", self.state.to_json()).finish_value()
    }
}
