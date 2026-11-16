//! The asset channel: a second connection a client opens (from a worker thread) so big
//! transfers never delay chat or game traffic. Port of `assets.go`.
//!
//! ```text
//! -> assetHello {token}            <- assetReady | disconnect {reason: "badToken"}
//! -> assetGet {gameId, path}       <- assetData {gameId, path, offset, size, data, done} ...
//!                                     or assetError {gameId, path, error}
//! -> assetPut {gameId, path,       <- assetPutAck {gameId, path, offset} ...
//!     offset, data, done}             or assetPutError {gameId, path, error}  (admins only)
//! ```

use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use tokio::sync::mpsc;

use super::Server;
use super::admin::game_file_path;
use super::auth::go_base64_decode;
use crate::connection::{Connection, ConnectionReader, IncomingMessage};
use crate::payload::{Fields, sorted_object};
use crate::store::User;

/// Bytes per `assetData` message (about 256 KiB once base64-encoded).
const ASSET_CHUNK_BYTES: usize = 192 << 10;
/// Requests a client may queue before it is treated as misbehaving.
const MAX_QUEUED_ASSET_REQUESTS: usize = 256;

/// One upload in progress. The client is stop-and-wait, so a connection has at most one.
struct Upload {
    file: File,
    temporary_path: PathBuf,
    final_path: PathBuf,
    bytes_written: i64,
}

impl Upload {
    fn abort(self) {
        drop(self.file);
        let _ = std::fs::remove_file(&self.temporary_path);
    }
}

impl Server {
    pub(super) async fn serve_assets(
        self: Arc<Self>,
        connection: Connection,
        mut reader: ConnectionReader,
        hello: IncomingMessage,
    ) {
        let token = Fields(&hello.payload).text("token");
        let user = tokio::task::block_in_place(|| self.store().session_user(&token));
        let user = match user {
            Ok(user) if !user.disabled => user,
            _ => {
                connection.close_with("badToken");
                return;
            }
        };
        connection.send("assetReady", crate::payload::empty_object());

        // A separate reader keeps answering pings (and so keeps the idle timer fresh) while a
        // large file streams out.
        let (request_sender, mut requests) = mpsc::channel::<IncomingMessage>(MAX_QUEUED_ASSET_REQUESTS);
        let reader_connection = connection.clone();
        tokio::spawn(async move {
            while let Some(message) = reader.read().await {
                if message.message_type != "assetGet" && message.message_type != "assetPut" {
                    continue;
                }
                if request_sender.try_send(message).is_err() {
                    reader_connection.close();
                    return;
                }
            }
        });

        let mut upload: Option<Upload> = None;
        while let Some(message) = requests.recv().await {
            if message.message_type == "assetPut" {
                let server = self.clone();
                tokio::task::block_in_place(|| server.handle_asset_put(&connection, &user, &message, &mut upload));
            } else {
                self.handle_asset_get(&connection, &message).await;
            }
        }
        if let Some(upload) = upload.take() {
            upload.abort();
        }
    }

    fn handle_asset_put(
        &self,
        connection: &Connection,
        user: &User,
        message: &IncomingMessage,
        upload: &mut Option<Upload>,
    ) {
        let fields = Fields(&message.payload);
        let game_id = fields.text("gameId");
        let path = fields.text("path");
        let offset = fields.integer("offset");
        let fail = |upload: &mut Option<Upload>, error: &str| {
            connection.send(
                "assetPutError",
                sorted_object([
                    ("gameId", game_id.as_str().into()),
                    ("path", path.as_str().into()),
                    ("error", error.into()),
                ]),
            );
            if let Some(upload) = upload.take() {
                upload.abort();
            }
        };
        if !user.is_admin {
            fail(upload, "notAdmin");
            return;
        }
        let full_path = if game_id.is_empty() {
            // No game yet: a flat staging upload (a new game's .zip for adminGameInstall).
            if path.is_empty() || path.contains(['/', '\\']) {
                fail(upload, "badPath");
                return;
            }
            let uploads_folder = self.library.read().unwrap().games_folder().join(".uploads");
            if std::fs::create_dir_all(&uploads_folder).is_err() {
                fail(upload, "unwritable");
                return;
            }
            uploads_folder.join(&path)
        } else {
            let library = self.library.read().unwrap();
            let Some(game) = library.game(&game_id) else {
                drop(library);
                fail(upload, "noGame");
                return;
            };
            let Some(full_path) = game_file_path(game, &path) else {
                drop(library);
                fail(upload, "noFolder");
                return;
            };
            full_path
        };
        match upload {
            None => {
                if offset != 0 {
                    fail(upload, "badOffset");
                    return;
                }
                if full_path.parent().is_some_and(|parent| std::fs::create_dir_all(parent).is_err()) {
                    fail(upload, "unwritable");
                    return;
                }
                let mut temporary_path = full_path.clone().into_os_string();
                temporary_path.push(".uploading");
                let temporary_path = PathBuf::from(temporary_path);
                let Ok(file) = File::create(&temporary_path) else {
                    fail(upload, "unwritable");
                    return;
                };
                *upload = Some(Upload { file, temporary_path, final_path: full_path, bytes_written: 0 });
            }
            Some(current) if current.final_path != full_path || current.bytes_written != offset => {
                fail(upload, "badOffset");
                return;
            }
            Some(_) => {}
        }
        let Ok(data) = go_base64_decode(&fields.text("data")) else {
            fail(upload, "badData");
            return;
        };
        let max_bytes = self.settings().max_upload_megabytes.saturating_mul(1 << 20);
        let current = upload.as_mut().expect("an upload is in progress");
        if current.bytes_written + data.len() as i64 > max_bytes {
            fail(upload, "tooLarge");
            return;
        }
        if current.file.write_all(&data).is_err() {
            fail(upload, "unwritable");
            return;
        }
        current.bytes_written += data.len() as i64;
        let acknowledged_offset = current.bytes_written;
        if fields.flag("done") {
            let finished = upload.take().expect("an upload is in progress");
            drop(finished.file);
            if std::fs::rename(&finished.temporary_path, &finished.final_path).is_err() {
                let _ = std::fs::remove_file(&finished.temporary_path);
                fail(upload, "unwritable");
                return;
            }
            if !game_id.is_empty() {
                self.rescan_library();
            }
        }
        connection.send(
            "assetPutAck",
            sorted_object([
                ("gameId", game_id.as_str().into()),
                ("path", path.as_str().into()),
                ("offset", acknowledged_offset.into()),
            ]),
        );
    }

    async fn handle_asset_get(&self, connection: &Connection, message: &IncomingMessage) {
        let fields = Fields(&message.payload);
        let game_id = fields.text("gameId");
        let path = fields.text("path");
        let fail = |error: &str| {
            connection.send(
                "assetError",
                sorted_object([
                    ("gameId", game_id.as_str().into()),
                    ("path", path.as_str().into()),
                    ("error", error.into()),
                ]),
            );
        };
        let opened = {
            let library = self.library.read().unwrap();
            match library.game(&game_id) {
                None => Err("noGame"),
                Some(game) => match game.manifest_file(&path) {
                    None => Err("notFound"),
                    Some(file) => game.open_file(&path).map(|reader| (reader, file.size)).map_err(|_| "unreadable"),
                },
            }
        };
        let (mut file, size) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                fail(error);
                return;
            }
        };
        let mut buffer = vec![0u8; ASSET_CHUNK_BYTES];
        let mut offset: u64 = 0;
        loop {
            let read = tokio::task::block_in_place(|| read_full(&mut file, &mut buffer));
            let Ok(count) = read else {
                fail("unreadable");
                return;
            };
            let done = count < ASSET_CHUNK_BYTES;
            // Waits for room in the send queue rather than overrunning it.
            connection
                .send_waiting(
                    "assetData",
                    sorted_object([
                        ("gameId", game_id.as_str().into()),
                        ("path", path.as_str().into()),
                        ("offset", (offset as f64).into()),
                        ("size", (size as f64).into()),
                        ("data", base64::engine::general_purpose::STANDARD.encode(&buffer[..count]).into()),
                        ("done", done.into()),
                    ]),
                )
                .await;
            offset += count as u64;
            if done || connection.is_closed() {
                return;
            }
        }
    }
}

/// Fills `buffer` as far as the file allows (Go's `io.ReadFull`): fewer bytes means the end.
fn read_full(file: &mut Box<dyn Read + Send>, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}
