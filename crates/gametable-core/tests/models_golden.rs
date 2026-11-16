//! Compares the Rust models against golden output produced by CardTable's original Lua models.
//!
//! Every `<name>.<Model>.json` in `fixtures/models/inputs` is decoded with `Model::from_json` and
//! re-encoded; the result must match `fixtures/models/expected` byte for byte (compact and
//! indented), or the error message must match `<name>.<Model>.error.txt`.
//! Regenerate the expected files with `tools/lua_golden/run.sh models`.

use std::path::{Path, PathBuf};

use gametable_core::json::{self, JsonValue};
use gametable_core::models::{
    CardDefinition, CardInstance, DeckConfig, GameDefinition, ModelError, SavedGame, ZoneDefinition,
};

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
                    match std::fs::read_to_string(&expected_path) {
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
