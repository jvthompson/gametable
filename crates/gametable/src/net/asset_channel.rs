//! A blocking second connection to a CardTable server for game files (the asset channel), used
//! on worker threads by the downloads and the admin uploads. Port of the socket parts shared by
//! `src/net/asset_worker.lua` and `admin_upload_worker.lua`.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use gametable_core::json::{JsonObject, JsonValue};
use gametable_core::protocol::{decode_line, encode_line};

pub struct AssetChannel {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
}

impl AssetChannel {
    /// Connects and says `assetHello` with the session token; `what` names the purpose in errors
    /// ("downloads", "upload").
    pub fn open(host: &str, port: u16, token: Option<&str>, what: &str) -> Result<AssetChannel, String> {
        let address = (host, port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut addresses| addresses.next())
            .ok_or_else(|| format!("Could not connect for {what}: host not found"))?;
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(10))
            .map_err(|error| format!("Could not connect for {what}: {error}"))?;
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
        let reader = BufReader::new(stream.try_clone().map_err(|error| error.to_string())?);
        let mut channel = AssetChannel { writer: stream, reader };
        let mut hello = JsonObject::new();
        if let Some(token) = token {
            hello.insert("token", token);
        }
        channel.send("assetHello", hello)?;
        let (reply, payload) = channel.receive()?;
        if reply != "assetReady" {
            return Err(if reply == "disconnect" {
                let reason = payload.get("reason").and_then(JsonValue::as_str).unwrap_or("");
                let refused =
                    if what == "downloads" { "server refused downloads" } else { "server refused the connection" };
                format!("{refused} ({reason})")
            } else {
                format!("unexpected reply {reply}")
            });
        }
        Ok(channel)
    }

    pub fn send(&mut self, message_type: &str, payload: JsonObject) -> Result<(), String> {
        let line = encode_line(message_type, &JsonValue::Object(payload)).map_err(|error| error.to_string())?;
        self.writer.write_all(line.as_bytes()).map_err(|error| format!("send failed: {error}"))
    }

    /// The next message other than ping/pong (answering pings so the server keeps us alive).
    pub fn receive(&mut self) -> Result<(String, JsonValue), String> {
        loop {
            let mut line = String::new();
            let read = self.reader.read_line(&mut line).map_err(|error| format!("connection lost: {error}"))?;
            if read == 0 {
                return Err("connection lost: closed".to_string());
            }
            let Some(message) = decode_line(line.trim_end_matches('\n')) else { continue };
            match message.message_type.as_str() {
                "ping" => self.send("pong", JsonObject::new())?,
                "pong" => {}
                _ => return Ok((message.message_type, message.payload)),
            }
        }
    }
}
