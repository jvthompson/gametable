//! A framed TCP connection: one JSON message per line, a bounded send queue, heartbeats.
//! Port of the Go server's `internal/proto` `Conn`.
//!
//! [`Connection`] is a cheap, cloneable handle used to send from anywhere (handlers, room
//! threads). [`ConnectionReader`] is owned by the one task that reads the connection's messages.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, watch};

use gametable_core::json::{JsonObject, JsonValue};
use gametable_core::protocol::{self, WireMessage};

/// Seconds between the pings the server sends.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(4);
/// Silence after which a connection is dropped.
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(12);
/// The longest line accepted (game uploads arrive in big chunks).
pub const MAX_LINE_BYTES: usize = 64 << 20;
/// Lines that may wait to be written before a slow peer is disconnected.
const OUTGOING_QUEUE_LENGTH: usize = 4096;
/// How long a single write may block.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the socket stays open after `close` so queued output can drain.
const CLOSE_GRACE: Duration = Duration::from_millis(200);

/// One encoded line, shared between every recipient of a broadcast.
pub type EncodedLine = Arc<str>;

/// Encodes one message the server built, escaping the way Go's `encoding/json` does (see
/// [`escape_html_like_go`]). Payloads always encode, but if one somehow does not, `{}` is sent
/// instead, as the Go server does.
pub fn encode_message(message_type: &str, payload: &JsonValue) -> EncodedLine {
    Arc::from(escape_html_like_go(encode_unescaped(message_type, payload)))
}

/// Encodes one message written by the game engine. The Go server passed the engine's own JSON
/// through untouched, so these lines are not HTML-escaped.
pub fn encode_engine_message(message_type: &str, payload: &JsonValue) -> EncodedLine {
    Arc::from(encode_unescaped(message_type, payload))
}

fn encode_unescaped(message_type: &str, payload: &JsonValue) -> String {
    protocol::encode_line(message_type, payload)
        .or_else(|_| protocol::encode_line(message_type, &JsonValue::Object(JsonObject::new())))
        .expect("an empty payload always encodes")
}

/// Go's `encoding/json` writes `<`, `>`, `&`, U+2028 and U+2029 as `\u` escapes. They can only
/// appear inside JSON strings, so replacing them in the encoded text is exact.
pub fn escape_html_like_go(encoded: String) -> String {
    const ESCAPED: [char; 5] = ['<', '>', '&', '\u{2028}', '\u{2029}'];
    if !encoded.contains(ESCAPED) {
        return encoded;
    }
    let mut escaped = String::with_capacity(encoded.len() + 16);
    for character in encoded.chars() {
        if ESCAPED.contains(&character) {
            escaped.push_str(&format!("\\u{:04x}", u32::from(character)));
        } else {
            escaped.push(character);
        }
    }
    escaped
}

struct ConnectionShared {
    outgoing: mpsc::Sender<EncodedLine>,
    /// Becomes true once `close` is called; never goes back.
    closed: watch::Sender<bool>,
    /// Becomes true once the socket itself has been shut down.
    socket_closed: watch::Sender<bool>,
    last_read_at: Mutex<Instant>,
    /// The peer's IP address (no port).
    address: String,
}

/// A handle for sending on a connection.
#[derive(Clone)]
pub struct Connection {
    shared: Arc<ConnectionShared>,
}

impl Connection {
    /// Wraps an accepted socket and starts its writer task. Returns the sending handle and the
    /// reader for the task that handles this connection's messages.
    pub fn start(stream: TcpStream) -> (Connection, ConnectionReader) {
        let _ = stream.set_nodelay(true);
        let address = stream.peer_addr().map(|peer| peer.ip().to_string()).unwrap_or_default();
        let (read_half, write_half) = stream.into_split();
        let (outgoing_sender, outgoing_receiver) = mpsc::channel(OUTGOING_QUEUE_LENGTH);
        let shared = Arc::new(ConnectionShared {
            outgoing: outgoing_sender,
            closed: watch::Sender::new(false),
            socket_closed: watch::Sender::new(false),
            last_read_at: Mutex::new(Instant::now()),
            address,
        });
        let connection = Connection { shared };
        tokio::spawn(run_writer(connection.clone(), write_half, outgoing_receiver));
        let reader =
            ConnectionReader { connection: connection.clone(), reader: BufReader::with_capacity(64 << 10, read_half) };
        (connection, reader)
    }

    /// The peer's IP address.
    pub fn address(&self) -> &str {
        &self.shared.address
    }

    pub fn is_closed(&self) -> bool {
        *self.shared.closed.borrow()
    }

    /// Whether two handles are the same connection.
    pub fn same_as(&self, other: &Connection) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// Queues one message.
    pub fn send(&self, message_type: &str, payload: JsonValue) {
        self.send_line(encode_message(message_type, &payload));
    }

    /// Queues an already-encoded line. A peer too far behind to take it is disconnected.
    pub fn send_line(&self, line: EncodedLine) {
        if self.is_closed() {
            return;
        }
        if let Err(mpsc::error::TrySendError::Full(_)) = self.shared.outgoing.try_send(line) {
            self.close();
        }
    }

    /// Queues one message, waiting while the queue is full (bulk transfers).
    pub async fn send_waiting(&self, message_type: &str, payload: JsonValue) {
        let line = encode_message(message_type, &payload);
        let mut closed = self.shared.closed.subscribe();
        tokio::select! {
            _ = self.shared.outgoing.send(line) => {}
            _ = crate::connection::wait_until_set(&mut closed) => {}
        }
    }

    /// Drops the connection; queued output is flushed first, best effort.
    pub fn close(&self) {
        self.shared.closed.send_replace(true);
    }

    /// Sends a final `disconnect {reason}` and closes.
    pub fn close_with(&self, reason: &str) {
        self.send("disconnect", JsonObject::new().with("reason", reason).into());
        self.close();
    }

    /// Resolves once `close` has been called.
    pub async fn wait_until_closed(&self) {
        let mut closed = self.shared.closed.subscribe();
        let _ = crate::connection::wait_until_set(&mut closed).await;
    }

    fn mark_read(&self) {
        *self.shared.last_read_at.lock().unwrap() = Instant::now();
    }

    fn idle_time(&self) -> Duration {
        self.shared.last_read_at.lock().unwrap().elapsed()
    }
}

async fn write_line(writer: &mut BufWriter<OwnedWriteHalf>, line: &str) -> bool {
    matches!(tokio::time::timeout(WRITE_TIMEOUT, writer.write_all(line.as_bytes())).await, Ok(Ok(())))
}

async fn flush(writer: &mut BufWriter<OwnedWriteHalf>, limit: Duration) -> bool {
    matches!(tokio::time::timeout(limit, writer.flush()).await, Ok(Ok(())))
}

/// Writes queued lines (batching whatever is already waiting), pings every heartbeat interval and
/// drops peers that have gone silent. On close, drains what is queued and shuts the socket.
async fn run_writer(connection: Connection, write_half: OwnedWriteHalf, mut outgoing: mpsc::Receiver<EncodedLine>) {
    let mut writer = BufWriter::with_capacity(64 << 10, write_half);
    let mut closed = connection.shared.closed.subscribe();
    let close_requested_at;
    let mut ping_timer = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);
    loop {
        tokio::select! {
            _ = crate::connection::wait_until_set(&mut closed) => {
                close_requested_at = Instant::now();
                while let Ok(line) = outgoing.try_recv() {
                    if !write_line(&mut writer, &line).await {
                        break;
                    }
                }
                let _ = flush(&mut writer, Duration::from_secs(1)).await;
                break;
            }
            Some(line) = outgoing.recv() => {
                let mut ok = write_line(&mut writer, &line).await;
                while ok {
                    match outgoing.try_recv() {
                        Ok(next_line) => ok = write_line(&mut writer, &next_line).await,
                        Err(_) => break,
                    }
                }
                if !ok || !flush(&mut writer, WRITE_TIMEOUT).await {
                    connection.close();
                }
            }
            _ = ping_timer.tick() => {
                if connection.idle_time() > HEARTBEAT_TIMEOUT {
                    connection.close();
                    continue;
                }
                let ping = encode_message("ping", &JsonValue::Object(JsonObject::new()));
                if !write_line(&mut writer, &ping).await || !flush(&mut writer, WRITE_TIMEOUT).await {
                    connection.close();
                }
            }
        }
    }
    let remaining_grace = CLOSE_GRACE.saturating_sub(close_requested_at.elapsed());
    tokio::time::sleep(remaining_grace).await;
    let _ = writer.into_inner().shutdown().await;
    connection.shared.socket_closed.send_replace(true);
}

/// One received message.
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub message_type: String,
    /// An object or an array; `{}` when the line had no usable payload.
    pub payload: JsonValue,
}

impl IncomingMessage {
    /// A field of an object payload.
    pub fn field(&self, key: &str) -> Option<&JsonValue> {
        self.payload.get(key)
    }
}

impl From<WireMessage> for IncomingMessage {
    fn from(message: WireMessage) -> Self {
        Self { message_type: message.message_type, payload: message.payload }
    }
}

/// The receiving side of a connection.
pub struct ConnectionReader {
    connection: Connection,
    reader: BufReader<OwnedReadHalf>,
}

/// Why reading stopped.
enum LineReadError {
    Closed,
}

impl ConnectionReader {
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// The next message, answering pings itself and skipping malformed lines (as the client
    /// does). `None` once the connection is closed, times out or sends an oversized line.
    pub async fn read(&mut self) -> Option<IncomingMessage> {
        loop {
            let mut socket_closed = self.connection.shared.socket_closed.subscribe();
            let line = tokio::select! {
                result = tokio::time::timeout(HEARTBEAT_TIMEOUT, self.read_line()) => match result {
                    Ok(Ok(line)) => line,
                    _ => return None,
                },
                _ = crate::connection::wait_until_set(&mut socket_closed) => return None,
            };
            self.connection.mark_read();
            let Some(message) = protocol::decode_line(trim_line_end(&line)) else { continue };
            match message.message_type.as_str() {
                "ping" => self.connection.send("pong", JsonValue::Object(JsonObject::new())),
                "pong" => {}
                _ => return Some(message.into()),
            }
        }
    }

    async fn read_line(&mut self) -> Result<String, LineReadError> {
        let mut line_bytes = Vec::new();
        loop {
            let available = self.reader.fill_buf().await.map_err(|_| LineReadError::Closed)?;
            if available.is_empty() {
                return Err(LineReadError::Closed); // EOF, possibly mid-line
            }
            match available.iter().position(|&byte| byte == b'\n') {
                Some(newline_position) => {
                    line_bytes.extend_from_slice(&available[..=newline_position]);
                    self.reader.consume(newline_position + 1);
                    return Ok(String::from_utf8_lossy(&line_bytes).into_owned());
                }
                None => {
                    let length = available.len();
                    line_bytes.extend_from_slice(available);
                    self.reader.consume(length);
                    if line_bytes.len() > MAX_LINE_BYTES {
                        return Err(LineReadError::Closed);
                    }
                }
            }
        }
    }
}

/// Removes trailing `\r` and `\n` characters (Go's `bytes.TrimRight(line, "\r\n")`).
fn trim_line_end(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

/// Resolves once a watched flag becomes true (without holding the watch guard across an await).
pub async fn wait_until_set(flag: &mut watch::Receiver<bool>) {
    let _ = flag.wait_for(|is_set| *is_set).await;
}
