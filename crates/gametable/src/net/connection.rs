//! One protocol connection over a non-blocking TCP socket: a line reader, a write buffer (large
//! messages such as `gameData` go out over several frames) and the heartbeat. Port of the
//! `Connection` and `P.connect` parts of `src/net/protocol.lua`.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use gametable_core::json::JsonValue;
use gametable_core::protocol::{
    CONNECT_TIMEOUT_SECONDS, HEARTBEAT_INTERVAL_SECONDS, HEARTBEAT_TIMEOUT_SECONDS, LineBuffer, WireMessage,
    decode_line, encode_line,
};

/// What [`Connection::heartbeat`] asks the caller to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatAction {
    /// The peer has been quiet too long (or the socket closed): disconnect.
    Timeout,
    /// Time to send a `ping`.
    Ping,
}

/// A live connection.
pub struct Connection {
    stream: TcpStream,
    input: LineBuffer,
    output: VecDeque<Vec<u8>>,
    /// Bytes of the first queued chunk already written.
    output_offset: usize,
    pub closed: bool,
    last_activity: Instant,
    last_ping: Instant,
}

impl Connection {
    /// `P.wrap(sock)`: non-blocking, no Nagle delay.
    pub fn wrap(stream: TcpStream) -> std::io::Result<Connection> {
        stream.set_nonblocking(true)?;
        let _ = stream.set_nodelay(true);
        let now = Instant::now();
        Ok(Connection {
            stream,
            input: LineBuffer::new(),
            output: VecDeque::new(),
            output_offset: 0,
            closed: false,
            last_activity: now,
            last_ping: now,
        })
    }

    /// Queues one message and writes what the socket accepts now.
    pub fn send(&mut self, message_type: &str, payload: JsonValue) {
        if let Ok(line) = encode_line(message_type, &payload) {
            self.send_raw(line.into_bytes());
        }
    }

    /// Queues an already encoded line (broadcasts encode once).
    pub fn send_raw(&mut self, line: Vec<u8>) {
        if self.closed {
            return;
        }
        self.output.push_back(line);
        self.flush();
    }

    /// Writes as much buffered output as the socket accepts right now.
    pub fn flush(&mut self) {
        while !self.closed {
            let Some(chunk) = self.output.front() else { return };
            match self.stream.write(&chunk[self.output_offset..]) {
                Ok(0) => {
                    self.close();
                    return;
                }
                Ok(written) => {
                    self.output_offset += written;
                    if self.output_offset >= chunk.len() {
                        self.output.pop_front();
                        self.output_offset = 0;
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => return,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => {
                    self.close();
                    return;
                }
            }
        }
    }

    /// Bytes still waiting to be written.
    pub fn pending_bytes(&self) -> usize {
        self.output.iter().map(Vec::len).sum::<usize>() - self.output_offset
    }

    /// Reads everything available; returns the decoded messages and whether any bytes arrived.
    pub fn poll(&mut self) -> (Vec<WireMessage>, bool) {
        let mut messages = Vec::new();
        if self.closed {
            return (messages, false);
        }
        self.flush();
        let mut got_data = false;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            match self.stream.read(&mut buffer) {
                Ok(0) => {
                    self.close();
                    break;
                }
                Ok(read) => {
                    got_data = true;
                    self.input.push_bytes(&buffer[..read]);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => {
                    self.close();
                    break;
                }
            }
        }
        while let Some(line) = self.input.next_line() {
            if let Some(message) = decode_line(&line) {
                messages.push(message);
            }
        }
        (messages, got_data)
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    /// Call once the connection is live, before the first [`heartbeat`](Self::heartbeat).
    pub fn start_heartbeat(&mut self) {
        let now = Instant::now();
        self.last_activity = now;
        self.last_ping = now;
    }

    /// Call every tick after [`poll`](Self::poll) with whether it got bytes.
    pub fn heartbeat(&mut self, got_data: bool) -> Option<HeartbeatAction> {
        let now = Instant::now();
        if got_data {
            self.last_activity = now;
        }
        if self.closed || now.duration_since(self.last_activity).as_secs_f64() > HEARTBEAT_TIMEOUT_SECONDS {
            Some(HeartbeatAction::Timeout)
        } else if now.duration_since(self.last_ping).as_secs_f64() >= HEARTBEAT_INTERVAL_SECONDS {
            self.last_ping = now;
            Some(HeartbeatAction::Ping)
        } else {
            None
        }
    }

    /// Sends what's buffered (briefly blocking, best effort) then closes: a final `disconnect`.
    pub fn close_after_flush(&mut self) {
        if self.closed {
            return;
        }
        let _ = self.stream.set_nonblocking(false);
        let _ = self.stream.set_write_timeout(Some(Duration::from_millis(500)));
        while let Some(chunk) = self.output.pop_front() {
            let _ = self.stream.write_all(&chunk[self.output_offset..]);
            self.output_offset = 0;
        }
        self.close();
    }
}

/// luasocket's wording for common connect failures (shown as "Failed to connect: ...").
fn connect_error_text(error: &std::io::Error) -> String {
    match error.kind() {
        ErrorKind::ConnectionRefused => "connection refused".to_string(),
        ErrorKind::TimedOut => "Connection timed out".to_string(),
        ErrorKind::NotFound | ErrorKind::InvalidInput => "host not found".to_string(),
        _ => error.to_string(),
    }
}

/// What a [`Connector`] reports.
pub enum ConnectProgress {
    Pending,
    Connected(Connection),
    Failed(String),
}

/// `P.connect(host, port, timeout)`: connects on a helper thread so the frame never blocks.
pub struct Connector {
    result: Receiver<Result<TcpStream, String>>,
}

impl Connector {
    pub fn start(host: &str, port: u16) -> Connector {
        let (sender, result) = mpsc::channel();
        let host = host.to_string();
        std::thread::spawn(move || {
            let outcome = (|| {
                let addresses: Vec<_> =
                    (host.as_str(), port).to_socket_addrs().map_err(|_| "host not found".to_string())?.collect();
                let timeout = Duration::from_secs_f64(CONNECT_TIMEOUT_SECONDS);
                let mut last_error = "host not found".to_string();
                // IPv4 first, as luasocket's tcp() did.
                let mut ordered = addresses.clone();
                ordered.sort_by_key(|address| !address.is_ipv4());
                for address in ordered {
                    match TcpStream::connect_timeout(&address, timeout) {
                        Ok(stream) => return Ok(stream),
                        Err(error) => last_error = connect_error_text(&error),
                    }
                }
                Err(last_error)
            })();
            let _ = sender.send(outcome);
        });
        Connector { result }
    }

    pub fn poll(&mut self) -> ConnectProgress {
        match self.result.try_recv() {
            Err(TryRecvError::Empty) => ConnectProgress::Pending,
            Err(TryRecvError::Disconnected) => ConnectProgress::Failed("Connection failed".to_string()),
            Ok(Err(message)) => ConnectProgress::Failed(message),
            Ok(Ok(stream)) => match Connection::wrap(stream) {
                Ok(connection) => ConnectProgress::Connected(connection),
                Err(error) => ConnectProgress::Failed(error.to_string()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gametable_core::json::JsonObject;
    use std::net::TcpListener;

    #[test]
    fn messages_round_trip_over_a_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut connector = Connector::start("127.0.0.1", port);
        let (server_stream, _) = listener.accept().unwrap();
        let mut server = Connection::wrap(server_stream).unwrap();
        let mut client = loop {
            match connector.poll() {
                ConnectProgress::Pending => std::thread::sleep(Duration::from_millis(5)),
                ConnectProgress::Connected(connection) => break connection,
                ConnectProgress::Failed(message) => panic!("{message}"),
            }
        };
        client.send("hello", JsonValue::Object(JsonObject::new().with("name", "Ann")));
        let big = "x".repeat(300_000);
        client.send("gameData", JsonValue::Object(JsonObject::new().with("blob", big.as_str())));
        let mut received = Vec::new();
        for _ in 0..400 {
            client.flush();
            received.extend(server.poll().0);
            if received.len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(received[0].message_type, "hello");
        assert_eq!(received[1].get("blob").and_then(JsonValue::as_str).map(str::len), Some(300_000));
        client.close();
        for _ in 0..50 {
            server.poll();
            if server.closed {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(server.closed);
        assert_eq!(server.heartbeat(false), Some(HeartbeatAction::Timeout));
    }

    #[test]
    fn refused_connections_fail() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut connector = Connector::start("127.0.0.1", port);
        loop {
            match connector.poll() {
                ConnectProgress::Pending => std::thread::sleep(Duration::from_millis(5)),
                ConnectProgress::Connected(_) => panic!("nothing is listening"),
                ConnectProgress::Failed(message) => {
                    assert_eq!(message, "connection refused");
                    break;
                }
            }
        }
    }
}
