//! Replays scripted multiplayer scenarios through the Rust session and host engine and compares
//! every outbound message (and the final state, including the game log) with golden output from
//! CardTable's original `session.lua` + `host_engine.lua`.
//! Regenerate with `tools/lua_golden/run.sh host_engine`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use gametable_core::host_engine::{HostEngine, HostTransport};
use gametable_core::json::{self, JsonValue};
use gametable_core::models::{DeckConfig, GameDefinition, PlayerInfo};
use gametable_core::session::{DealOptions, Session, SessionEnvironment};
use gametable_core::util::ParkMillerRandom;

/// Records every outbound message as `<step>|send|...` / `<step>|broadcast|...` lines.
struct RecordingTransport {
    roster: Vec<PlayerInfo>,
    current_step: usize,
    lines: Vec<String>,
}

impl HostTransport for RecordingTransport {
    fn roster(&self) -> Vec<PlayerInfo> {
        self.roster.clone()
    }
    fn send_to(&mut self, player_id: &str, message_type: &str, payload: JsonValue) {
        let encoded = json::encode_compact(&payload).unwrap();
        self.lines.push(format!("{}|send|{player_id}|{message_type}|{encoded}", self.current_step));
    }
    fn broadcast(&mut self, message_type: &str, payload: JsonValue, except_player_id: Option<&str>) {
        let encoded = json::encode_compact(&payload).unwrap();
        let except = except_player_id.unwrap_or("-");
        self.lines.push(format!("{}|broadcast|{except}|{message_type}|{encoded}", self.current_step));
    }
}

fn players_from(value: &JsonValue) -> Vec<PlayerInfo> {
    value.as_array().unwrap().iter().map(|player| PlayerInfo::from_json(player).unwrap()).collect()
}

/// Groups lines by their step prefix and sorts within each group, because the original sent some
/// roster-related messages in Lua hash-table order.
fn normalize(lines: &[String]) -> Vec<String> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for line in lines {
        let step = line.split('|').next().unwrap().to_string();
        match groups.last_mut() {
            Some((group_step, group_lines)) if *group_step == step => group_lines.push(line.clone()),
            _ => groups.push((step, vec![line.clone()])),
        }
    }
    groups
        .into_iter()
        .flat_map(|(_, mut group_lines)| {
            group_lines.sort();
            group_lines
        })
        .collect()
}

fn run_script(script: &JsonValue) -> Vec<String> {
    let clock = Arc::new(Mutex::new(1000.0_f64));
    let clock_for_session = Arc::clone(&clock);
    let mut id_counter = 0;
    let seed = script.get("seed").unwrap().as_f64().unwrap() as i64;
    let environment = SessionEnvironment {
        now_seconds: Box::new(move || *clock_for_session.lock().unwrap()),
        random: Box::new(ParkMillerRandom::with_seed(seed)),
        new_instance_id: Box::new(move || {
            id_counter += 1;
            format!("id-{id_counter:04}")
        }),
    };

    let game = GameDefinition::from_json(script.get("game").unwrap()).unwrap();
    let host_player_id = script.get("hostPlayerId").unwrap().as_str().unwrap().to_string();
    let decks_by_key = |field: &str| -> Option<HashMap<String, DeckConfig>> {
        script.get(field).map(|decks| {
            decks
                .as_object()
                .unwrap()
                .iter()
                .map(|(key, deck)| (key.clone(), DeckConfig::from_json(deck).unwrap()))
                .collect()
        })
    };
    let deck_configs = decks_by_key("deckConfigsByPlayerId");
    let shared_deck_configs = decks_by_key("sharedDeckConfigsByZoneId");
    let session = Session::deal_from_zones(
        DealOptions {
            game,
            players: players_from(script.get("players").unwrap()),
            local_player_id: host_player_id.clone(),
            deck_configs_by_player_id: deck_configs.as_ref(),
            shared_deck_configs_by_zone_id: shared_deck_configs.as_ref(),
            is_local_practice: false,
        },
        environment,
    );

    let mut transport = RecordingTransport {
        roster: players_from(script.get("initialRoster").unwrap()),
        current_step: 0,
        lines: Vec::new(),
    };
    let owner_id = script.get("ownerId").and_then(JsonValue::as_str).map(str::to_string);
    let mut engine = HostEngine::new(session, host_player_id, owner_id);
    engine.start(&mut transport);

    for (step_index, step) in script.get("steps").unwrap().as_array().unwrap().iter().enumerate() {
        transport.current_step = step_index + 1;
        if let Some(roster) = step.get("roster") {
            transport.roster = players_from(roster);
            let roster = transport.roster.clone();
            engine.on_roster(&roster, &mut transport);
        } else if let Some(seconds) = step.get("advanceClock") {
            *clock.lock().unwrap() += seconds.as_f64().unwrap();
        } else if step.get("tick").is_some() {
            engine.tick(&mut transport);
        } else {
            let from = step.get("from").unwrap().as_str().unwrap();
            let message_type = step.get("type").unwrap().as_str().unwrap();
            let empty_payload = JsonValue::Object(Default::default());
            let payload = step.get("payload").unwrap_or(&empty_payload);
            engine.handle_message(from, message_type, payload, &mut transport);
        }
    }
    let mut lines = transport.lines;
    lines.push(format!("final|{}", json::encode_compact(&engine.session.state().to_json()).unwrap()));
    lines
}

#[test]
fn scripted_games_match_the_original_lua_message_for_message() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/host_engine");
    let mut script_count = 0;
    for entry in std::fs::read_dir(fixtures.join("inputs")).unwrap() {
        let path = entry.unwrap().path();
        let base_name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let script = json::decode_bytes(&std::fs::read(&path).unwrap()).unwrap();
        let expected_text =
            std::fs::read_to_string(fixtures.join("expected").join(format!("{base_name}.expected.txt"))).unwrap();
        let expected: Vec<String> = expected_text.split('\n').map(str::to_string).collect();

        let actual = normalize(&run_script(&script));
        let expected = normalize(&expected);
        for (line_index, (actual_line, expected_line)) in actual.iter().zip(&expected).enumerate() {
            assert_eq!(actual_line, expected_line, "{base_name}: output line {} differs", line_index + 1);
        }
        assert_eq!(actual.len(), expected.len(), "{base_name}: different number of output lines");
        script_count += 1;
    }
    assert!(script_count > 0);
}
