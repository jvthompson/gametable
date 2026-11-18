//! Wire framing shared by the client and the lobby server. Port of the encoding parts of
//! `src/net/protocol.lua` (and the Go server's `internal/proto`).
//!
//! Every message is one JSON object per line, `{"type":"<name>","payload":{...}}\n`, over plain TCP.
//! Socket handling lives in the client and server crates; this module only turns messages into
//! bytes and bytes back into messages.

use crate::json::{self, JsonEncodeError, JsonObject, JsonValue};

/// Default TCP port for peer hosting and the lobby server.
pub const DEFAULT_GAME_PORT: u16 = crate::DEFAULT_NETWORK_PORT;
/// Sent in `hello`; a lobby server refuses clients with a different version.
pub const SERVER_PROTOCOL_VERSION: u32 = crate::SERVER_PROTOCOL_VERSION;
/// Seconds between `ping` messages.
pub const HEARTBEAT_INTERVAL_SECONDS: f64 = 4.0;
/// Seconds of silence after which a connection is dropped.
pub const HEARTBEAT_TIMEOUT_SECONDS: f64 = 12.0;
/// Seconds allowed for a TCP connect to complete.
pub const CONNECT_TIMEOUT_SECONDS: f64 = 5.0;
/// Raw bytes per `assetData` / `assetPut` chunk (before base64).
pub const ASSET_CHUNK_BYTES: usize = 192 * 1024;

/// One decoded message.
#[derive(Debug, Clone, PartialEq)]
pub struct WireMessage {
    pub message_type: String,
    /// Always an object or an array; anything else (or a missing payload) becomes `{}`.
    pub payload: JsonValue,
}

impl WireMessage {
    pub fn new(message_type: impl Into<String>, payload: JsonValue) -> Self {
        Self { message_type: message_type.into(), payload }
    }

    /// A field of an object payload.
    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.payload.get(key)
    }
}

/// Encodes one message as a line, newline included: `type` first, then `payload`.
pub fn encode_line(message_type: &str, payload: &JsonValue) -> Result<String, JsonEncodeError> {
    let envelope = JsonObject::new().with("type", message_type).with("payload", payload.clone());
    let mut line = json::encode_compact(&JsonValue::Object(envelope))?;
    line.push('\n');
    Ok(line)
}

/// Decodes one line (without its newline; a trailing `\r` is fine). Blank lines, malformed JSON
/// and objects without a string `type` give `None` and should be skipped silently.
pub fn decode_line(line: &str) -> Option<WireMessage> {
    if line.chars().all(|character| matches!(character, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r')) {
        return None;
    }
    let decoded = json::decode(line).ok()?;
    let message_type = decoded.get("type")?.as_str()?.to_string();
    let payload = match decoded.get("payload") {
        Some(payload @ (JsonValue::Object(_) | JsonValue::Array(_))) => payload.clone(),
        _ => JsonValue::Object(JsonObject::new()),
    };
    Some(WireMessage { message_type, payload })
}

/// Accumulates received bytes and yields complete lines.
#[derive(Debug, Default)]
pub struct LineBuffer {
    pending_bytes: Vec<u8>,
}

impl LineBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_bytes(&mut self, received: &[u8]) {
        self.pending_bytes.extend_from_slice(received);
    }

    /// The next complete line without its `\n` (and without one trailing `\r`), if any.
    /// Invalid UTF-8 is replaced with U+FFFD.
    pub fn next_line(&mut self) -> Option<String> {
        let newline_position = self.pending_bytes.iter().position(|&byte| byte == b'\n')?;
        let mut line_bytes: Vec<u8> = self.pending_bytes.drain(..=newline_position).collect();
        line_bytes.pop(); // the '\n'
        if line_bytes.last() == Some(&b'\r') {
            line_bytes.pop();
        }
        Some(String::from_utf8_lossy(&line_bytes).into_owned())
    }

    /// Bytes received after the last complete line.
    pub fn pending_length(&self) -> usize {
        self.pending_bytes.len()
    }
}

/// Wire names of the game-traffic messages a client sends to the authority.
pub mod game_requests {
    pub const MOVE: &str = "requestMove";
    pub const MOVE_GROUP: &str = "requestMoveGroup";
    pub const MOVE_STACK: &str = "requestMoveStack";
    pub const ROTATE_STACK: &str = "requestRotateStack";
    pub const SEND_TO_BACK: &str = "requestSendToBack";
    pub const FLIP: &str = "requestFlip";
    pub const GIVE_CARD: &str = "requestGiveCard";
    pub const STACK: &str = "requestStack";
    pub const MOVE_TO_HAND: &str = "requestMoveToHand";
    pub const REORDER_HAND: &str = "requestReorderHand";
    pub const DRAW: &str = "requestDraw";
    pub const SHUFFLE: &str = "requestShuffle";
    pub const DRAW_FROM_ZONE: &str = "requestDrawFromZone";
    pub const RETURN_TO_ZONE: &str = "requestReturnToZone";
    pub const SHUFFLE_ZONE: &str = "requestShuffleZone";
    pub const START_SEARCH_ZONE: &str = "requestStartSearchZone";
    pub const START_SEARCH_PILE: &str = "requestStartSearchPile";
    pub const STOP_SEARCH: &str = "requestStopSearch";
    pub const CREATE_WIDGET: &str = "requestCreateWidget";
    pub const CREATE_ARROW: &str = "requestCreateArrow";
    pub const MOVE_WIDGET: &str = "requestMoveWidget";
    pub const SET_WIDGET_VALUE: &str = "requestSetWidgetValue";
    pub const DELETE_WIDGET: &str = "requestDeleteWidget";
    pub const SET_WIDGET_COLORS: &str = "requestSetWidgetColors";
    pub const SET_WIDGET_TEXT: &str = "requestSetWidgetText";
    pub const SET_WIDGET_SIZE: &str = "requestSetWidgetSize";
    pub const ROTATE_WIDGET: &str = "requestRotateWidget";
    pub const FLIP_WIDGET: &str = "requestFlipWidget";
    pub const SEND_WIDGET_TO_BACK: &str = "requestSendWidgetToBack";
    pub const DUPLICATE_WIDGET: &str = "requestDuplicateWidget";
    pub const ATTACH_WIDGET_TO_CARD: &str = "requestAttachWidgetToCard";
    pub const DRAW_BAG_TOKEN: &str = "requestDrawBagToken";
    pub const RETURN_BAG_TOKEN: &str = "requestReturnBagToken";
    pub const PLACE_BAG_TOKEN_ON_TABLE: &str = "requestPlaceBagTokenOnTable";
    pub const RESET_BAG: &str = "requestResetBag";
    pub const SET_BAG_REMAINING: &str = "requestSetBagRemaining";
    pub const RETURN_IMAGE_TOKEN_TO_BAG: &str = "requestReturnImageTokenToBag";
    pub const GENERATE_PACK: &str = "requestGeneratePack";
    pub const CREATE_CARD_FROM_LIBRARY: &str = "requestCreateCardFromLibrary";
    pub const LOAD_DECK_INTO_ZONE: &str = "requestLoadDeckIntoZone";
    pub const LOAD_SCENARIO: &str = "requestLoadScenario";
    pub const SET_SHARED_PLAYMAT: &str = "requestSetSharedPlaymat";
    pub const SET_CURRENT_PLAYER: &str = "requestSetCurrentPlayer";
    pub const SET_INDIVIDUAL_PLAYMAT: &str = "requestSetIndividualPlaymat";
    /// GameTable addition: `{playmatIndex, positions: [{zoneId, x, y}]}`.
    pub const SET_PLAYMAT_ZONE_POSITIONS: &str = "requestSetPlaymatZonePositions";
    pub const CARD_DRAG_PREVIEW: &str = "cardDragPreview";
    pub const CARD_DRAG_PREVIEW_END: &str = "cardDragPreviewEnd";
    pub const ARROW_DRAG_PREVIEW: &str = "arrowDragPreview";
    pub const ARROW_DRAG_PREVIEW_END: &str = "arrowDragPreviewEnd";
}

/// Wire names of messages every connection type uses, and the authority's game messages.
pub mod common_messages {
    pub const PING: &str = "ping";
    pub const PONG: &str = "pong";
    pub const DISCONNECT: &str = "disconnect";
    pub const HELLO: &str = "hello";
    pub const WELCOME: &str = "welcome";
    pub const LOBBY_ROSTER_UPDATE: &str = "lobbyRosterUpdate";
    pub const GAME_DATA: &str = "gameData";
    pub const FULL_STATE: &str = "fullState";
    pub const SERVER_INFO: &str = "serverInfo";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_type_before_payload_with_newline() {
        let payload = JsonValue::Object(JsonObject::new().with("b", 1).with("a", 2));
        assert_eq!(encode_line("ping", &payload).unwrap(), "{\"type\":\"ping\",\"payload\":{\"b\":1,\"a\":2}}\n");
    }

    #[test]
    fn decodes_and_normalizes_payloads() {
        let message = decode_line("{\"payload\":{\"x\":1},\"type\":\"requestMove\"}\r").unwrap();
        assert_eq!(message.message_type, "requestMove");
        assert_eq!(message.get("x"), Some(&JsonValue::Number(1.0)));
        assert_eq!(decode_line("{\"type\":\"ping\"}").unwrap().payload, JsonValue::Object(JsonObject::new()));
        assert_eq!(
            decode_line("{\"type\":\"ping\",\"payload\":5}").unwrap().payload,
            JsonValue::Object(JsonObject::new())
        );
        assert!(decode_line("   ").is_none());
        assert!(decode_line("{\"type\":5}").is_none());
        assert!(decode_line("not json").is_none());
        assert!(decode_line("[1,2]").is_none());
    }

    #[test]
    fn line_buffer_splits_across_chunks_and_strips_carriage_returns() {
        let mut buffer = LineBuffer::new();
        buffer.push_bytes(b"{\"type\":\"a\"}\r\n{\"ty");
        assert_eq!(buffer.next_line().as_deref(), Some("{\"type\":\"a\"}"));
        assert_eq!(buffer.next_line(), None);
        buffer.push_bytes(b"pe\":\"b\"}\n\n");
        assert_eq!(buffer.next_line().as_deref(), Some("{\"type\":\"b\"}"));
        assert_eq!(buffer.next_line().as_deref(), Some(""));
        assert_eq!(buffer.pending_length(), 0);
    }
}
