//! Moving zones on playmats through the host engine (a GameTable addition): each player moves
//! zones on their own individual mats, only the room's owner moves zones on the shared mat, and
//! positions for one mat replace only that mat's.

use gametable_core::host_engine::{HostEngine, HostTransport};
use gametable_core::json::{self, JsonValue};
use gametable_core::models::{GameDefinition, PlayerInfo, PlayerRole, PlaymatType, PlaymatZonePosition, TableState};
use gametable_core::session::{DealOptions, Session, SessionEnvironment};

struct SilentTransport {
    roster: Vec<PlayerInfo>,
}

impl HostTransport for SilentTransport {
    fn roster(&self) -> Vec<PlayerInfo> {
        self.roster.clone()
    }
    fn send_to(&mut self, _player_id: &str, _message_type: &str, _payload: JsonValue) {}
    fn broadcast(&mut self, _message_type: &str, _payload: JsonValue, _except_player_id: Option<&str>) {}
}

fn players() -> Vec<PlayerInfo> {
    vec![
        PlayerInfo::new("host", "Host", PlayerRole::Host, 0xFF00_00FF),
        PlayerInfo::new("guest", "Guest", PlayerRole::Client, 0xFFFF_0000),
    ]
}

fn engine(playmat_type: PlaymatType) -> (HostEngine, SilentTransport) {
    let mut game = GameDefinition::new("game", "Game");
    game.playmat_type = playmat_type;
    let session = Session::deal_from_zones(
        DealOptions {
            game,
            players: players(),
            local_player_id: "host".to_string(),
            deck_configs_by_player_id: None,
            shared_deck_configs_by_zone_id: None,
            is_local_practice: false,
        },
        SessionEnvironment::system(),
    );
    (HostEngine::new(session, "host", None), SilentTransport { roster: players() })
}

fn request(playmat_index: i64, positions: &str) -> JsonValue {
    json::decode(&format!(r#"{{"playmatIndex":{playmat_index},"positions":{positions}}}"#)).unwrap()
}

fn player<'a>(state: &'a TableState, id: &str) -> &'a PlayerInfo {
    state.players.iter().find(|player| player.id == id).unwrap()
}

#[test]
fn each_player_moves_zones_on_their_own_mat() {
    let (mut engine, mut transport) = engine(PlaymatType::Individual);
    let payload = request(1, r#"[{"zoneId":"deck","x":0.25,"y":0.75},{"zoneId":"discard","x":2,"y":-1}]"#);
    engine.handle_message("guest", "requestSetPlaymatZonePositions", &payload, &mut transport);
    let state = engine.session.state();
    assert_eq!(
        player(state, "guest").playmat_zone_positions,
        vec![PlaymatZonePosition::new(1, "deck", 0.25, 0.75), PlaymatZonePosition::new(1, "discard", 1.0, 0.0)],
        "kept on the mat"
    );
    assert!(player(state, "host").playmat_zone_positions.is_empty());
    assert!(state.shared_playmat_zone_positions.is_empty());

    // Another mat's positions are kept; sending an empty list for mat 1 resets just that one.
    let payload = request(2, r#"[{"zoneId":"deck","x":0.5,"y":0.5}]"#);
    engine.handle_message("guest", "requestSetPlaymatZonePositions", &payload, &mut transport);
    engine.handle_message("guest", "requestSetPlaymatZonePositions", &request(1, "[]"), &mut transport);
    let positions = &player(engine.session.state(), "guest").playmat_zone_positions;
    assert_eq!(positions, &vec![PlaymatZonePosition::new(2, "deck", 0.5, 0.5)]);
}

#[test]
fn only_the_owner_moves_zones_on_the_shared_mat() {
    let (mut engine, mut transport) = engine(PlaymatType::Shared);
    let payload = request(1, r#"[{"zoneId":"market","x":0.4,"y":0.6}]"#);
    engine.handle_message("guest", "requestSetPlaymatZonePositions", &payload, &mut transport);
    assert!(engine.session.state().shared_playmat_zone_positions.is_empty(), "a guest can't");
    engine.handle_message("host", "requestSetPlaymatZonePositions", &payload, &mut transport);
    let state = engine.session.state();
    assert_eq!(state.shared_playmat_zone_positions, vec![PlaymatZonePosition::new(1, "market", 0.4, 0.6)]);
    assert!(player(state, "host").playmat_zone_positions.is_empty());
}

#[test]
fn moved_positions_round_trip_in_saved_state() {
    let (mut engine, mut transport) = engine(PlaymatType::Individual);
    let payload = request(1, r#"[{"zoneId":"deck","x":0.25,"y":0.75}]"#);
    engine.handle_message("guest", "requestSetPlaymatZonePositions", &payload, &mut transport);
    let saved = engine.session.state().to_json();
    let encoded = json::encode_compact(&saved).unwrap();
    assert!(encoded.contains(r#""playmatZonePositions":[{"playmatIndex":1,"zoneId":"deck","x":0.25,"y":0.75}]"#));
    assert_eq!(&TableState::from_json(&saved).unwrap(), engine.session.state());
}
