//! Persistent key/value settings. Port of `src/data/settings.lua`.
//!
//! Stored as `settings.json` in the per-user save folder, keeping the original key names. The
//! top-level keys are written sorted with a two-space indent; nested values keep their order.
//! Unreadable or invalid files read as empty, and failed writes are ignored, as in the original.

use std::path::{Path, PathBuf};

use gametable_core::json::{self, JsonObject, JsonValue};

/// The file name inside the save folder.
pub const SETTINGS_FILE_NAME: &str = "settings.json";

/// Key names shared with the original (and, for the first group, the older Dart app).
pub mod keys {
    pub const GAMES_DIRECTORY_PATH: &str = "games_directory_path";
    pub const DECKS_DIRECTORY_PATH: &str = "decks_directory_path";
    pub const PLAYER_NAME: &str = "player_profile_name";
    pub const PLAYER_COLOR: &str = "player_profile_color";
    pub const PLAYER_AVATAR_PATH: &str = "player_profile_avatar_path";
    pub const RECONNECT_HOST_KEY: &str = "reconnect_host_key";
    pub const RECONNECT_PLAYER_ID: &str = "reconnect_player_id";
    pub const DISCORD_REFRESH_TOKEN: &str = "discord_refresh_token";
    pub const DARK_MODE: &str = "dark_mode_enabled";
    // Added by the LÖVE port:
    pub const UI_SCALE: &str = "ui_scale";
    /// GameTable addition: multiplies interface text size only (1 = designed size).
    pub const TEXT_SCALE: &str = "text_scale";
    /// GameTable addition: the card grid zoom in the deck editor and the game definition
    /// editor's Cards tab (1 = the original size).
    pub const DECK_EDITOR_CARD_ZOOM: &str = "deck_editor_card_zoom";
    pub const CARD_VIEW_CARD_ZOOM: &str = "card_view_card_zoom";
    pub const RECENT_SERVERS: &str = "recent_servers";
    pub const SELECTION_COLOR: &str = "selection_color";
    pub const FELT_COLOR: &str = "felt_color";
    pub const PLAYMAT_BORDER_COLOR: &str = "playmat_border_color";
    pub const CURRENT_TURN_COLOR: &str = "current_turn_color";
    pub const FELT_TEXTURE: &str = "felt_texture_enabled";
    // Added by GameTable:
    /// The user's Anthropic API key for "Suggest Title → From Image" with Claude.
    pub const ANTHROPIC_API_KEY: &str = "anthropic_api_key";
    /// `"haiku"` (default) or `"opus"`.
    pub const CLAUDE_TITLE_MODEL: &str = "claude_title_model";
    /// The last "Suggest Title" choices: `"id"`, `"ocr"` or `"claude"`.
    pub const SUGGEST_TITLE_SOURCE: &str = "suggest_title_source";
    /// Followed by `_<game id>`: the name/subtitle separator last used for that game's titles.
    pub const SUGGEST_TITLE_SEPARATOR_PREFIX: &str = "suggest_title_separator";
    /// The last "Suggest Extra Data" reader: `"ocr"` or `"claude"`.
    pub const SUGGEST_EXTRA_DATA_SOURCE: &str = "suggest_extra_data_source";
    /// Followed by `_<game id>_<field key>`: the Claude instructions last used for that field.
    pub const SUGGEST_EXTRA_DATA_INSTRUCTIONS_PREFIX: &str = "suggest_extra_data_instructions";
}

/// The settings file, loaded into memory once and rewritten after every change.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// `None` keeps everything in memory (tests, `--identity` dry runs).
    file_path: Option<PathBuf>,
    values: JsonObject,
}

impl Settings {
    /// Loads `settings.json` from a save folder.
    pub fn load_from_folder(save_folder: &Path) -> Settings {
        Settings::load_file(save_folder.join(SETTINGS_FILE_NAME))
    }

    /// Loads a settings file; a missing, unreadable or non-object file gives empty settings.
    pub fn load_file(file_path: PathBuf) -> Settings {
        let values = std::fs::read(&file_path)
            .ok()
            .and_then(|bytes| json::decode_bytes(&bytes).ok())
            .and_then(|value| match value {
                JsonValue::Object(object) => Some(object),
                _ => None,
            })
            .unwrap_or_default();
        Settings { file_path: Some(file_path), values }
    }

    /// Settings that are never written to disk.
    pub fn in_memory() -> Settings {
        Settings::default()
    }

    pub fn file_path(&self) -> Option<&Path> {
        self.file_path.as_deref()
    }

    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.values.get(key)
    }

    /// A text value, if the key holds one.
    pub fn text(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(JsonValue::as_str)
    }

    /// A number the way Lua's `tonumber` reads it: a JSON number, or a string holding one.
    pub fn number(&self, key: &str) -> Option<f64> {
        match self.get(key)? {
            JsonValue::Number(number) => Some(*number),
            JsonValue::String(text) => lua_tonumber(text),
            _ => None,
        }
    }

    pub fn flag(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(JsonValue::as_bool)
    }

    pub fn set(&mut self, key: &str, value: impl Into<JsonValue>) {
        self.values.insert(key, value.into());
        self.save();
    }

    pub fn remove(&mut self, key: &str) {
        self.values.remove(key);
        self.save();
    }

    /// The file's text as it would be written now.
    pub fn encoded(&self) -> String {
        let mut sorted = self.values.clone();
        sorted.sort_keys();
        json::encode_indented(&JsonValue::Object(sorted)).unwrap_or_else(|_| "{}".to_string())
    }

    fn save(&self) {
        if let Some(file_path) = &self.file_path {
            let _ = std::fs::write(file_path, self.encoded());
        }
    }
}

/// Lua's `tonumber` for strings: surrounding whitespace allowed, decimal or `0x` hex.
fn lua_tonumber(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    if let Some(hex) = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).ok().map(|value| value as f64);
    }
    // LuaJIT's number scanner has no spelling for infinity or NaN.
    let lowercase = trimmed.to_ascii_lowercase();
    if lowercase.contains("inf") || lowercase.contains("nan") {
        return None;
    }
    trimmed.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_folder() -> PathBuf {
        let folder = std::env::temp_dir().join(format!("gametable-settings-{}", gametable_core::uuid::new_v4_uuid()));
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn writes_sorted_top_level_keys_with_two_space_indent() {
        let folder = temporary_folder();
        let mut settings = Settings::load_from_folder(&folder);
        assert!(settings.get(keys::PLAYER_NAME).is_none());
        settings.set(keys::PLAYER_NAME, "Ann");
        settings.set(keys::DARK_MODE, true);
        settings.set(keys::PLAYER_COLOR, 4294278144.0);
        let written = std::fs::read_to_string(folder.join(SETTINGS_FILE_NAME)).unwrap();
        assert_eq!(
            written,
            "{\n  \"dark_mode_enabled\": true,\n  \"player_profile_color\": 4294278144,\n  \"player_profile_name\": \"Ann\"\n}"
        );
        settings.remove(keys::DARK_MODE);
        let reloaded = Settings::load_from_folder(&folder);
        assert_eq!(reloaded.text(keys::PLAYER_NAME), Some("Ann"));
        assert_eq!(reloaded.number(keys::PLAYER_COLOR), Some(4294278144.0));
        assert!(reloaded.flag(keys::DARK_MODE).is_none());
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn nested_objects_keep_their_order() {
        let folder = temporary_folder();
        std::fs::write(folder.join(SETTINGS_FILE_NAME), r#"{"z": {"b": 1, "a": 2}, "a": [1, 2]}"#).unwrap();
        let mut settings = Settings::load_from_folder(&folder);
        settings.set("m", "x");
        let written = std::fs::read_to_string(folder.join(SETTINGS_FILE_NAME)).unwrap();
        assert_eq!(
            written,
            "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"m\": \"x\",\n  \"z\": {\n    \"b\": 1,\n    \"a\": 2\n  }\n}"
        );
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn bad_files_read_as_empty() {
        let folder = temporary_folder();
        for contents in ["not json", "[1, 2]", ""] {
            std::fs::write(folder.join(SETTINGS_FILE_NAME), contents).unwrap();
            assert_eq!(Settings::load_from_folder(&folder).encoded(), "{}");
        }
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn numbers_may_be_strings() {
        let mut settings = Settings::in_memory();
        settings.set("a", " 12.5 ");
        settings.set("b", "0x10");
        settings.set("c", "nope");
        assert_eq!(settings.number("a"), Some(12.5));
        assert_eq!(settings.number("b"), Some(16.0));
        assert_eq!(settings.number("c"), None);
    }

    /// Re-saves a copy of the real `%APPDATA%\cardtable\settings.json` (read only) and checks the
    /// bytes are unchanged. Run with `cargo test -p gametable -- --ignored real_settings`.
    #[test]
    #[ignore]
    fn real_settings_file_round_trips() {
        let Some(app_data) = std::env::var_os("APPDATA") else { return };
        let original = PathBuf::from(app_data).join("cardtable").join(SETTINGS_FILE_NAME);
        let Ok(original_text) = std::fs::read_to_string(&original) else { return };
        let settings = Settings::load_file(original.clone());
        assert_eq!(settings.encoded(), original_text);
    }
}
