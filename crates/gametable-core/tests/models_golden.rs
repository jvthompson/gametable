//! Compares the Rust models against golden output produced by CardTable's original Lua models.
//!
//! Every `<name>.<Model>.json` in `fixtures/models/inputs` is decoded with `Model::from_json` and
//! re-encoded; the result must match `fixtures/models/expected` byte for byte (compact and
//! indented), or the error message must match `<name>.<Model>.error.txt`.
//! Regenerate the expected files with `tools/lua_golden/run.sh models`.
//!
//! One deliberate difference is applied to the Lua output before comparing: GameTable saves the
//! original's `playmatType` + `playmats` as `gameBoards`, each board carrying its own kind (see
//! [`as_game_table_saves_it`]).

use std::path::{Path, PathBuf};

use gametable_core::json::{self, JsonObject, JsonValue};
use gametable_core::models::{
    CardDefinition, CardInstance, DeckConfig, GameDefinition, ModelError, SavedGame, ZoneDefinition,
};

/// The Lua output as GameTable writes it: wherever an object has `playmats`, the list is
/// written as `gameBoards` in its place, `playmatType` is dropped, and a shared game's boards
/// each say `"kind":"shared"` (player boards, the default, say nothing).
fn as_game_table_saves_it(value: &JsonValue) -> JsonValue {
    match value {
        JsonValue::Array(items) => JsonValue::Array(items.iter().map(as_game_table_saves_it).collect()),
        JsonValue::Object(object) if object.contains_key("playmats") => {
            let shared = object.get("playmatType").and_then(JsonValue::as_str) == Some("shared");
            let mut rewritten = JsonObject::new();
            for (key, field) in object.iter() {
                match key.as_str() {
                    "playmatType" => {}
                    "playmats" => {
                        let boards: Vec<JsonValue> = field
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|board| {
                                let mut board = board.as_object().cloned().unwrap_or_default();
                                if shared {
                                    board.insert("kind", "shared");
                                }
                                JsonValue::Object(board)
                            })
                            .collect();
                        rewritten.insert("gameBoards", JsonValue::Array(boards));
                    }
                    _ => rewritten.insert(key.clone(), as_game_table_saves_it(field)),
                }
            }
            JsonValue::Object(rewritten)
        }
        JsonValue::Object(object) => {
            let mut rewritten = JsonObject::new();
            for (key, field) in object.iter() {
                rewritten.insert(key.clone(), as_game_table_saves_it(field));
            }
            JsonValue::Object(rewritten)
        }
        other => other.clone(),
    }
}

/// The expected text with GameTable's deliberate differences applied, in the same format.
fn expected_from_game_table(expected: String, suffix: &str) -> String {
    if !expected.contains("\"playmats\"") {
        return expected;
    }
    let value = as_game_table_saves_it(&json::decode(&expected).expect("expected output is valid JSON"));
    let encoded = if suffix == "compact" { json::encode_compact(&value) } else { json::encode_indented(&value) };
    let encoded = encoded.unwrap();
    // Keep the file's trailing newline, if it has one.
    if expected.ends_with('\n') && !encoded.ends_with('\n') { encoded + "\n" } else { encoded }
}

fn fixtures_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/models")
}

/// Decodes and re-encodes one fixture with the named model.
fn round_trip_model(model_name: &str, input: &JsonValue) -> Result<JsonValue, ModelError> {
    Ok(match model_name {
        "GameDefinition" => GameDefinition::from_json(input)?.to_json(),
        "SavedGame" => SavedGame::from_json(input)?.to_json(),
        "DeckConfig" => DeckConfig::from_json(input)?.to_json(),
        "ZoneDefinition" => ZoneDefinition::from_json(input)?.to_json(),
        "CardDefinition" => CardDefinition::from_json(input)?.to_json(false),
        "CardInstance" => CardInstance::from_json(input)?.to_json(),
        other => panic!("no Rust model mapped for fixture model name {other}"),
    })
}

#[test]
fn every_fixture_matches_the_original_lua_output() {
    let inputs_directory = fixtures_directory().join("inputs");
    let expected_directory = fixtures_directory().join("expected");
    let mut checked_fixture_count = 0;
    let mut mismatches = Vec::new();

    for entry in std::fs::read_dir(&inputs_directory).expect("fixture inputs exist") {
        let file_name = entry.unwrap().file_name().into_string().unwrap();
        let Some(stem) = file_name.strip_suffix(".json") else { continue };
        let (_, model_name) = stem.rsplit_once('.').expect("fixture names look like <name>.<Model>.json");

        let input_bytes = std::fs::read(inputs_directory.join(&file_name)).unwrap();
        let input = json::decode_bytes(&input_bytes).expect("fixture inputs are valid JSON");
        let error_file = expected_directory.join(format!("{stem}.error.txt"));

        match round_trip_model(model_name, &input) {
            Ok(output) => {
                for (suffix, encoded) in [
                    ("compact", json::encode_compact(&output).unwrap()),
                    ("indented", json::encode_indented(&output).unwrap()),
                ] {
                    let expected_path = expected_directory.join(format!("{stem}.{suffix}.json"));
                    match std::fs::read_to_string(&expected_path).map(|text| expected_from_game_table(text, suffix)) {
                        Ok(expected) if expected == encoded => {}
                        Ok(expected) => mismatches.push(format!(
                            "{file_name} ({suffix}) differs\n  expected: {expected}\n  actual:   {encoded}"
                        )),
                        Err(_) => mismatches.push(format!("{file_name}: Rust succeeded but Lua raised an error")),
                    }
                }
            }
            Err(error) => match std::fs::read_to_string(&error_file) {
                Ok(expected_message) if expected_message == error.to_string() => {}
                Ok(expected_message) => mismatches
                    .push(format!("{file_name}: error differs\n  expected: {expected_message}\n  actual:   {error}")),
                Err(_) => mismatches.push(format!("{file_name}: Rust raised '{error}' but Lua succeeded")),
            },
        }
        checked_fixture_count += 1;
    }

    assert!(checked_fixture_count >= 14, "expected the full fixture set, found {checked_fixture_count}");
    assert!(mismatches.is_empty(), "{} mismatch(es):\n{}", mismatches.len(), mismatches.join("\n"));
}
