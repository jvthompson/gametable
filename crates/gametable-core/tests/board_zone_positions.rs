//! Moving zones on game boards through the host engine (a GameTable addition): each player moves
//! zones on their own player boards, only the room's owner moves zones on shared boards, and
//! positions for one board replace only that board's.

use gametable_core::host_engine::{HostEngine, HostTransport};
use gametable_core::json::{self, JsonValue};
use gametable_core::models::{
    BoardKind, BoardZonePosition, GameBoardDefinition, GameDefinition, MovedBoard, PlayerInfo, PlayerRole, TableState,
};
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

/// A game whose two boards are both of `kind`.
fn engine(kind: BoardKind) -> (HostEngine, SilentTransport) {
    let mut game = GameDefinition::new("game", "Game");
    game.game_boards = ["one.png", "two.png"]
        .into_iter()
        .map(|image| GameBoardDefinition { kind, ..GameBoardDefinition::new(image) })
        .collect();
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

fn request(board_index: i64, positions: &str) -> JsonValue {
    json::decode(&format!(r#"{{"playmatIndex":{board_index},"positions":{positions}}}"#)).unwrap()
}

fn player<'a>(state: &'a TableState, id: &str) -> &'a PlayerInfo {
    state.players.iter().find(|player| player.id == id).unwrap()
}

#[test]
fn each_player_moves_zones_on_their_own_mat() {
    let (mut engine, mut transport) = engine(BoardKind::Player);
    let payload = request(1, r#"[{"zoneId":"deck","x":0.25,"y":0.75},{"zoneId":"discard","x":2,"y":-1}]"#);
    engine.handle_message("guest", "requestSetPlaymatZonePositions", &payload, &mut transport);
    let state = engine.session.state();
    assert_eq!(
        player(state, "guest").board_zone_positions,
        vec![BoardZonePosition::new(1, "deck", 0.25, 0.75), BoardZonePosition::new(1, "discard", 1.0, 0.0)],
        "kept on the mat"
    );
    assert!(player(state, "host").board_zone_positions.is_empty());
    assert!(state.shared_board_zone_positions.is_empty());

    // Another mat's positions are kept; sending an empty list for mat 1 resets just that one.
    let payload = request(2, r#"[{"zoneId":"deck","x":0.5,"y":0.5}]"#);
    engine.handle_message("guest", "requestSetPlaymatZonePositions", &payload, &mut transport);
    engine.handle_message("guest", "requestSetPlaymatZonePositions", &request(1, "[]"), &mut transport);
    let positions = &player(engine.session.state(), "guest").board_zone_positions;
    assert_eq!(positions, &vec![BoardZonePosition::new(2, "deck", 0.5, 0.5)]);
}

#[test]
fn only_the_owner_moves_zones_on_the_shared_mat() {
    let (mut engine, mut transport) = engine(BoardKind::Shared);
    let payload = request(1, r#"[{"zoneId":"market","x":0.4,"y":0.6}]"#);
    engine.handle_message("guest", "requestSetPlaymatZonePositions", &payload, &mut transport);
    assert!(engine.session.state().shared_board_zone_positions.is_empty(), "a guest can't");
    engine.handle_message("host", "requestSetPlaymatZonePositions", &payload, &mut transport);
    let state = engine.session.state();
    assert_eq!(state.shared_board_zone_positions, vec![BoardZonePosition::new(1, "market", 0.4, 0.6)]);
    assert!(player(state, "host").board_zone_positions.is_empty());
}

#[test]
fn each_player_moves_and_turns_only_their_own_player_boards() {
    let (mut engine, mut transport) = crate::engine(BoardKind::Player);
    let moved = json::decode(r#"{"boardIndex":2,"x":0.25,"y":1.5,"rotation":90}"#).unwrap();
    engine.handle_message("guest", "requestSetMovedBoard", &moved, &mut transport);
    let state = engine.session.state();
    assert_eq!(player(state, "guest").moved_boards, vec![MovedBoard::new(2, 0.25, 1.0, 90)], "kept on the table");
    assert!(player(state, "host").moved_boards.is_empty());
    let encoded = json::encode_compact(&state.to_json()).unwrap();
    assert!(encoded.contains(r#""movedBoards":[{"boardIndex":2,"x":0.25,"y":1,"rotation":90}]"#));
    assert_eq!(&TableState::from_json(&state.to_json()).unwrap(), state);

    // Without a position the board goes back where the game puts it.
    let back = json::decode(r#"{"boardIndex":2}"#).unwrap();
    engine.handle_message("guest", "requestSetMovedBoard", &back, &mut transport);
    assert!(player(engine.session.state(), "guest").moved_boards.is_empty());

    // Shared boards aren't moved this way.
    let (mut shared_engine, mut shared_transport) = crate::engine(BoardKind::Shared);
    shared_engine.handle_message("host", "requestSetMovedBoard", &moved, &mut shared_transport);
    assert!(player(shared_engine.session.state(), "host").moved_boards.is_empty());
}

#[test]
fn moved_positions_round_trip_in_saved_state() {
    let (mut engine, mut transport) = engine(BoardKind::Player);
    let payload = request(1, r#"[{"zoneId":"deck","x":0.25,"y":0.75}]"#);
    engine.handle_message("guest", "requestSetPlaymatZonePositions", &payload, &mut transport);
    let saved = engine.session.state().to_json();
    let encoded = json::encode_compact(&saved).unwrap();
    assert!(encoded.contains(r#""playmatZonePositions":[{"playmatIndex":1,"zoneId":"deck","x":0.25,"y":0.75}]"#));
    assert_eq!(&TableState::from_json(&saved).unwrap(), engine.session.state());
}
