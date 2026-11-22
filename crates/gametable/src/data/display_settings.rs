//! Display preferences kept in `settings.json`: the fanned hand, table colours and felt texture,
//! per-game playmat seeds and rulebook scroll positions. Ports of `hand_settings.lua`,
//! `table_settings.lua`, `playmat_settings.lua` and `rulebook_settings.lua`.

use std::collections::HashMap;

use gametable_core::color::{ArgbColor, color_from_hex};
use gametable_core::json::{JsonObject, JsonValue, format_like_c_percent_g};
use gametable_core::models::BoardZonePosition;

use super::settings::{Settings, keys};

// ---------------------------------------------------------------------------
// Hand
// ---------------------------------------------------------------------------

/// The hand settings, in the order the settings dialog lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HandSetting {
    CardScale,
    Spacing,
    Tilt,
    MaxSpread,
    Raise,
    Reveal,
}

/// A hand setting's slider range and default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HandSettingDefinition {
    pub setting: HandSetting,
    /// Stored as `hand_<id>`.
    pub id: &'static str,
    pub label: &'static str,
    pub min: f64,
    pub max: f64,
    /// The slider's snapping interval.
    pub step: f64,
    pub default: f64,
}

pub const HAND_SETTINGS: [HandSettingDefinition; 6] = [
    HandSettingDefinition {
        setting: HandSetting::CardScale,
        id: "cardScale",
        label: "Card size",
        min: 1.0,
        max: 4.0,
        step: 0.05,
        default: 3.0,
    },
    HandSettingDefinition {
        setting: HandSetting::Spacing,
        id: "spacing",
        label: "Card spacing",
        min: 0.2,
        max: 1.1,
        step: 0.05,
        default: 0.6,
    },
    HandSettingDefinition {
        setting: HandSetting::Tilt,
        id: "tilt",
        label: "Tilt between cards",
        min: 0.0,
        max: 10.0,
        step: 0.5,
        default: 3.0,
    },
    HandSettingDefinition {
        setting: HandSetting::MaxSpread,
        id: "maxSpread",
        label: "Maximum fan spread",
        min: 0.0,
        max: 90.0,
        step: 1.0,
        default: 24.0,
    },
    HandSettingDefinition {
        setting: HandSetting::Raise,
        id: "raise",
        label: "Resting height above hand area",
        min: 0.0,
        max: 200.0,
        step: 2.0,
        default: 50.0,
    },
    HandSettingDefinition {
        setting: HandSetting::Reveal,
        id: "reveal",
        label: "Card shown on hover",
        min: 0.0,
        max: 1.0,
        step: 0.05,
        default: 1.0,
    },
];

impl HandSetting {
    pub fn definition(self) -> &'static HandSettingDefinition {
        HAND_SETTINGS.iter().find(|definition| definition.setting == self).expect("every setting is defined")
    }

    fn storage_key(self) -> String {
        format!("hand_{}", self.definition().id)
    }

    /// The value as the settings dialog shows it.
    pub fn format_value(self, value: f64) -> String {
        match self {
            HandSetting::CardScale => format!("{value:.2}x"),
            HandSetting::Spacing => format!("{}% of card width", (value * 100.0 + 0.5).floor()),
            HandSetting::Tilt | HandSetting::MaxSpread => format!("{}°", format_like_c_percent_g(value, 6)),
            HandSetting::Raise => format!("{}px", value.trunc()),
            HandSetting::Reveal => format!("{}%", (value * 100.0 + 0.5).floor()),
        }
    }
}

/// The hand settings with their in-memory values (a held slider changes a value without saving).
#[derive(Debug, Clone, Default)]
pub struct HandSettings {
    current_values: HashMap<HandSetting, f64>,
}

impl HandSettings {
    pub fn get(&mut self, settings: &Settings, setting: HandSetting) -> f64 {
        if let Some(value) = self.current_values.get(&setting) {
            return *value;
        }
        let definition = setting.definition();
        let value = settings.number(&setting.storage_key()).unwrap_or(definition.default);
        let value = value.clamp(definition.min, definition.max);
        self.current_values.insert(setting, value);
        value
    }

    /// `live` only updates the in-memory value (while a slider is held); the next non-live set
    /// writes it. A value equal to the default is removed from the file instead.
    pub fn set(&mut self, settings: &mut Settings, setting: HandSetting, value: f64, live: bool) {
        let definition = setting.definition();
        let value = value.clamp(definition.min, definition.max);
        self.current_values.insert(setting, value);
        if live {
            return;
        }
        let key = setting.storage_key();
        let stored = settings.number(&key);
        if value == definition.default {
            if stored.is_some() {
                settings.remove(&key);
            }
        } else if stored != Some(value) {
            settings.set(&key, value);
        }
    }

    pub fn is_default(&mut self, settings: &Settings) -> bool {
        HAND_SETTINGS.iter().all(|definition| self.get(settings, definition.setting) == definition.default)
    }

    pub fn reset(&mut self, settings: &mut Settings) {
        for definition in HAND_SETTINGS {
            self.set(settings, definition.setting, definition.default, false);
        }
    }
}

// ---------------------------------------------------------------------------
// Table colours
// ---------------------------------------------------------------------------

pub const DEFAULT_SELECTION_COLOR: ArgbColor = 0xFFFF_EB3B; // Material yellow 500
// GameTable's defaults: a midnight blue felt, a deeper midnight playmat border and a neon cyan
// current-turn highlight (the original CardTable used greens: 0xFF0B6E4F, 0xFF063D2A, 0xFF00C853).
// A game's own colours, and anything a player picked in Settings > Table, still win.
pub const DEFAULT_FELT_COLOR: ArgbColor = 0xFF19_1970;
/// Shared game boards' border (a player's own boards are bordered in their player colour), also
/// the outline of the table in the editor previews.
pub const SHARED_BOARD_BORDER_COLOR: ArgbColor = 0xFF0A_0A4A;
pub const DEFAULT_CURRENT_TURN_COLOR: ArgbColor = 0xFF00_E5FF;

/// A colour that resolves player override → the current game's own colour → a default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableColor {
    Felt,
    CurrentTurn,
}

impl TableColor {
    fn storage_key(self) -> &'static str {
        match self {
            TableColor::Felt => keys::FELT_COLOR,
            TableColor::CurrentTurn => keys::CURRENT_TURN_COLOR,
        }
    }

    pub fn default_color(self) -> ArgbColor {
        match self {
            TableColor::Felt => DEFAULT_FELT_COLOR,
            TableColor::CurrentTurn => DEFAULT_CURRENT_TURN_COLOR,
        }
    }

    /// The player's own pick, if any.
    pub fn player_override(self, settings: &Settings) -> Option<ArgbColor> {
        stored_color(settings, self.storage_key())
    }

    /// The colour in effect; `game_hex` is the current game's own `"#RRGGBB"`, if it has one.
    pub fn resolve(self, settings: &Settings, game_hex: Option<&str>) -> ArgbColor {
        self.player_override(settings).or_else(|| game_hex.and_then(color_from_hex)).unwrap_or(self.default_color())
    }

    /// Saves a pick, unless it is the colour already in effect.
    pub fn set(self, settings: &mut Settings, argb: ArgbColor, game_hex: Option<&str>) {
        if argb == self.resolve(settings, game_hex) {
            return;
        }
        settings.set(self.storage_key(), f64::from(argb));
    }

    pub fn is_default(self, settings: &Settings) -> bool {
        self.player_override(settings).is_none()
    }

    pub fn reset(self, settings: &mut Settings) {
        settings.remove(self.storage_key());
    }
}

fn stored_color(settings: &Settings, key: &str) -> Option<ArgbColor> {
    settings.number(key).map(|value| value.floor().clamp(0.0, f64::from(u32::MAX)) as ArgbColor)
}

/// The selection highlight colour (a plain override of a default).
pub fn selection_color(settings: &Settings) -> ArgbColor {
    stored_color(settings, keys::SELECTION_COLOR).unwrap_or(DEFAULT_SELECTION_COLOR)
}

/// Saves the selection colour; picking the default removes the override.
pub fn set_selection_color(settings: &mut Settings, argb: ArgbColor) {
    if argb == selection_color(settings) {
        return;
    }
    if argb == DEFAULT_SELECTION_COLOR {
        settings.remove(keys::SELECTION_COLOR);
    } else {
        settings.set(keys::SELECTION_COLOR, f64::from(argb));
    }
}

pub fn is_selection_color_default(settings: &Settings) -> bool {
    selection_color(settings) == DEFAULT_SELECTION_COLOR
}

pub fn reset_selection_color(settings: &mut Settings) {
    set_selection_color(settings, DEFAULT_SELECTION_COLOR);
}

/// Whether the felt's woven-cloth shader is on (default: yes; only `false` turns it off).
pub fn felt_texture_enabled(settings: &Settings) -> bool {
    settings.flag(keys::FELT_TEXTURE) != Some(false)
}

pub fn set_felt_texture_enabled(settings: &mut Settings, enabled: bool) {
    if enabled {
        settings.remove(keys::FELT_TEXTURE);
    } else {
        settings.set(keys::FELT_TEXTURE, false);
    }
}

// ---------------------------------------------------------------------------
// Game board seeds and rulebook scroll positions (per game id)
// ---------------------------------------------------------------------------
// (The keys still say "playmat", from before boards were renamed, so earlier choices carry over.)

/// Whether this computer last showed (F12) the game's toggleable boards; `when_never_set` until
/// the player has pressed F12 in this game.
pub fn boards_shown(settings: &Settings, game_id: &str, when_never_set: bool) -> bool {
    settings.flag(&format!("playmat_on_{game_id}")).unwrap_or(when_never_set)
}

/// The board this computer's player last picked (1-based; 0 for none yet).
pub fn chosen_board(settings: &Settings, game_id: &str) -> i64 {
    settings.number(&format!("playmat_index_{game_id}")).unwrap_or(0.0) as i64
}

fn board_zone_positions_key(game_id: &str, board_index: i64) -> String {
    format!("playmat_zone_positions_{game_id}_{board_index}")
}

/// Where this computer's player last moved zones on one of a game's boards (1-based index):
/// `[{zoneId, x, y}]`, sent again whenever that board is shown.
pub fn board_zone_positions(settings: &Settings, game_id: &str, board_index: i64) -> Vec<BoardZonePosition> {
    let Some(JsonValue::Array(entries)) = settings.get(&board_zone_positions_key(game_id, board_index)) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let zone_id = entry.get("zoneId")?.as_str()?;
            let (x, y) = (entry.get("x")?.as_f64()?, entry.get("y")?.as_f64()?);
            Some(BoardZonePosition::new(board_index, zone_id, x, y))
        })
        .collect()
}

/// Remembers (or, for an empty list, forgets) where zones were moved on one board.
pub fn set_board_zone_positions(
    settings: &mut Settings,
    game_id: &str,
    board_index: i64,
    positions: &[BoardZonePosition],
) {
    let key = board_zone_positions_key(game_id, board_index);
    if positions.is_empty() {
        settings.remove(&key);
        return;
    }
    let entries: Vec<JsonValue> = positions
        .iter()
        .map(|position| {
            let mut entry = JsonObject::new();
            entry.insert("zoneId", position.zone_id.as_str());
            entry.insert("x", position.x);
            entry.insert("y", position.y);
            JsonValue::Object(entry)
        })
        .collect();
    settings.set(&key, JsonValue::Array(entries));
}

/// Remembers whether the game's toggleable boards are shown, and the board picked (0 forgets it).
pub fn set_boards_shown(settings: &mut Settings, game_id: &str, shown: bool, chosen_board: i64) {
    settings.set(&format!("playmat_on_{game_id}"), shown);
    let index_key = format!("playmat_index_{game_id}");
    if chosen_board != 0 {
        settings.set(&index_key, chosen_board as f64);
    } else {
        settings.remove(&index_key);
    }
}

/// The rulebook viewer's scroll position for a document key (a game id, or `id:doc:file`).
pub fn rulebook_scroll(settings: &Settings, document_key: &str) -> f64 {
    settings.number(&format!("rulebook_scroll_{document_key}")).unwrap_or(0.0)
}

pub fn set_rulebook_scroll(settings: &mut Settings, document_key: &str, y: f64) {
    settings.set(&format!("rulebook_scroll_{document_key}"), y);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hand_values_clamp_skip_defaults_and_hold_live_changes() {
        let mut settings = Settings::in_memory();
        let mut hand = HandSettings::default();
        assert_eq!(hand.get(&settings, HandSetting::CardScale), 3.0);
        hand.set(&mut settings, HandSetting::CardScale, 9.0, true);
        assert_eq!(hand.get(&settings, HandSetting::CardScale), 4.0);
        assert!(settings.get("hand_cardScale").is_none(), "live changes are not saved");
        hand.set(&mut settings, HandSetting::CardScale, 4.0, false);
        assert_eq!(settings.number("hand_cardScale"), Some(4.0));
        assert!(!hand.is_default(&settings));
        hand.reset(&mut settings);
        assert!(settings.get("hand_cardScale").is_none());
        assert!(hand.is_default(&settings));

        let mut stored = Settings::in_memory();
        stored.set("hand_tilt", -5.0);
        assert_eq!(HandSettings::default().get(&stored, HandSetting::Tilt), 0.0);
    }

    #[test]
    fn hand_labels() {
        assert_eq!(HandSetting::CardScale.format_value(3.0), "3.00x");
        assert_eq!(HandSetting::Spacing.format_value(0.6), "60% of card width");
        assert_eq!(HandSetting::Tilt.format_value(2.5), "2.5°");
        assert_eq!(HandSetting::MaxSpread.format_value(24.0), "24°");
        assert_eq!(HandSetting::Raise.format_value(50.0), "50px");
        assert_eq!(HandSetting::Reveal.format_value(0.35), "35%");
    }

    #[test]
    fn table_colors_resolve_override_then_game_then_default() {
        let mut settings = Settings::in_memory();
        assert_eq!(TableColor::Felt.resolve(&settings, None), DEFAULT_FELT_COLOR);
        assert_eq!(TableColor::Felt.resolve(&settings, Some("#112233")), 0xFF11_2233);
        TableColor::Felt.set(&mut settings, 0xFF11_2233, Some("#112233"));
        assert!(TableColor::Felt.is_default(&settings), "picking what is already in effect saves nothing");
        TableColor::Felt.set(&mut settings, 0xFF44_5566, Some("#112233"));
        assert_eq!(TableColor::Felt.resolve(&settings, Some("#112233")), 0xFF44_5566);
        TableColor::Felt.reset(&mut settings);
        assert!(TableColor::Felt.is_default(&settings));

        assert_eq!(selection_color(&settings), DEFAULT_SELECTION_COLOR);
        set_selection_color(&mut settings, 0xFF00_0000);
        assert_eq!(selection_color(&settings), 0xFF00_0000);
        set_selection_color(&mut settings, DEFAULT_SELECTION_COLOR);
        assert!(settings.get(keys::SELECTION_COLOR).is_none());

        assert!(felt_texture_enabled(&settings));
        set_felt_texture_enabled(&mut settings, false);
        assert!(!felt_texture_enabled(&settings));
        set_felt_texture_enabled(&mut settings, true);
        assert!(settings.get(keys::FELT_TEXTURE).is_none());
    }

    #[test]
    fn playmat_and_rulebook_values() {
        let mut settings = Settings::in_memory();
        assert!(!boards_shown(&settings, "g", false));
        assert!(boards_shown(&settings, "g", true));
        set_boards_shown(&mut settings, "g", true, 2);
        assert!(boards_shown(&settings, "g", false));
        assert_eq!(chosen_board(&settings, "g"), 2);
        // Hiding is remembered too, so boards that start shown stay hidden next time.
        set_boards_shown(&mut settings, "g", false, 0);
        assert!(!boards_shown(&settings, "g", true));
        assert!(settings.get("playmat_index_g").is_none());
        set_rulebook_scroll(&mut settings, "g:doc:rules.pdf", 120.5);
        assert_eq!(rulebook_scroll(&settings, "g:doc:rules.pdf"), 120.5);
        assert_eq!(rulebook_scroll(&settings, "other"), 0.0);
    }
}
