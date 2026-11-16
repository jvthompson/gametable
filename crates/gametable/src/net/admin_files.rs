//! Admin-only file transfers for a server's games. Ports of `src/net/admin_file_manager.lua`,
//! `admin_upload_worker.lua` and `admin_game_manager.lua`: a file is uploaded over the asset
//! channel on a worker thread (in 192 KiB chunks) so big files don't block the UI, and a new
//! game's `.zip` is checked locally before it is uploaded (the original extracted it with
//! `tar.exe`; here the `zip` crate reads it in place).

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::JoinHandle;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gametable_core::json::{self, JsonObject, JsonValue};
use gametable_core::protocol::ASSET_CHUNK_BYTES;

use crate::net::asset_channel::AssetChannel;

enum UploadMessage {
    Progress(u64),
    Done,
    Error(String),
}

/// How an upload is going.
#[derive(Debug, Clone, PartialEq)]
pub enum UploadProgress {
    Uploading { bytes: u64, total: u64 },
    Done,
    Failed(String),
}

/// `M.upload(sc, gameId, destPath, localPath, onProgress, onDone)`: one local file into a game's
/// folder on the server (`game_id` empty: the server's staging area for new games).
pub struct FileUpload {
    total: u64,
    bytes: u64,
    messages: Receiver<UploadMessage>,
    thread: Option<JoinHandle<()>>,
    finished: Option<UploadProgress>,
}

impl FileUpload {
    pub fn start(
        host: &str,
        port: u16,
        token: Option<String>,
        game_id: &str,
        destination: &str,
        local_path: &str,
    ) -> FileUpload {
        let total = std::fs::metadata(local_path).map_or(0, |metadata| metadata.len());
        let (sender, messages) = mpsc::channel();
        let (host, game_id, destination, local_path) =
            (host.to_string(), game_id.to_string(), destination.to_string(), local_path.to_string());
        let thread = std::thread::Builder::new()
            .name("admin-upload".to_string())
            .spawn(move || {
                let result = (|| -> Result<(), String> {
                    let data = std::fs::read(&local_path).map_err(|_| format!("Could not read {local_path}"))?;
                    let mut channel = AssetChannel::open(&host, port, token.as_deref(), "upload")?;
                    let total = data.len();
                    let mut offset = 0;
                    loop {
                        let count = ASSET_CHUNK_BYTES.min(total - offset);
                        let done = offset + count >= total;
                        let payload = JsonObject::new()
                            .with("gameId", game_id.as_str())
                            .with("path", destination.as_str())
                            .with("offset", offset as f64)
                            .with("size", total as f64)
                            .with("data", BASE64.encode(&data[offset..offset + count]))
                            .with("done", done);
                        channel.send("assetPut", payload)?;
                        let (reply, reply_payload) = channel.receive()?;
                        if reply == "assetPutError" {
                            let error =
                                reply_payload.get("error").and_then(JsonValue::as_str).unwrap_or("").to_string();
                            return Err(error);
                        }
                        if reply != "assetPutAck" {
                            return Err(format!("unexpected reply {reply}"));
                        }
                        offset += count;
                        let _ = sender.send(UploadMessage::Progress(offset as u64));
                        if offset >= total {
                            return Ok(());
                        }
                    }
                })();
                let _ = sender.send(match result {
                    Ok(()) => UploadMessage::Done,
                    Err(message) => UploadMessage::Error(message),
                });
            })
            .ok();
        FileUpload { total, bytes: 0, messages, thread, finished: None }
    }

    pub fn poll(&mut self) -> UploadProgress {
        if let Some(finished) = &self.finished {
            return finished.clone();
        }
        loop {
            let finished = match self.messages.try_recv() {
                Ok(UploadMessage::Progress(bytes)) => {
                    self.bytes = bytes;
                    continue;
                }
                Ok(UploadMessage::Done) => UploadProgress::Done,
                Ok(UploadMessage::Error(message)) => UploadProgress::Failed(message),
                Err(TryRecvError::Empty) => return UploadProgress::Uploading { bytes: self.bytes, total: self.total },
                Err(TryRecvError::Disconnected) => {
                    UploadProgress::Failed("The upload stopped unexpectedly.".to_string())
                }
            };
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            self.finished = Some(finished.clone());
            return finished;
        }
    }
}

/// `validateZip`: exactly one top-level folder, a `gamedef.json` inside it, and an `id` matching
/// the folder's name. Returns the id.
pub fn validate_game_zip(zip_path: &str) -> Result<String, String> {
    let file = std::fs::File::open(zip_path).map_err(|_| "Couldn't read that zip file.".to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| "Couldn't read that zip file.".to_string())?;
    let names: Vec<String> =
        archive.file_names().filter_map(|name| name.ok()).map(|name| name.replace('\\', "/")).collect();
    let mut top_level: Vec<String> = Vec::new();
    let mut top_is_folder = true;
    for name in &names {
        let trimmed = name.trim_start_matches('/');
        let (first, rest) = match trimmed.split_once('/') {
            Some((first, rest)) => (first, rest),
            None => (trimmed, ""),
        };
        if first.is_empty() {
            continue;
        }
        if !top_level.iter().any(|known| known == first) {
            top_level.push(first.to_string());
        }
        if rest.is_empty() && !name.ends_with('/') {
            // A file at the root.
            top_is_folder = false;
        }
    }
    if top_level.len() != 1 || !top_is_folder {
        return Err("The zip must contain exactly one folder at its root.".to_string());
    }
    let folder = top_level.remove(0);
    let definition_name = names
        .iter()
        .find(|name| {
            name.trim_start_matches('/')
                .split_once('/')
                .is_some_and(|(first, rest)| first == folder && rest.eq_ignore_ascii_case("gamedef.json"))
        })
        .cloned()
        .ok_or_else(|| "No gamedef.json found inside the folder.".to_string())?;
    let mut raw = Vec::new();
    {
        use std::io::Read;
        let index = names.iter().position(|name| *name == definition_name).unwrap_or(0);
        let mut entry = archive.by_index(index).map_err(|_| "No gamedef.json found inside the folder.".to_string())?;
        entry.read_to_end(&mut raw).map_err(|_| "No gamedef.json found inside the folder.".to_string())?;
    }
    let id = json::decode_bytes(&raw)
        .ok()
        .and_then(|definition| definition.get("id").and_then(JsonValue::as_str).map(str::to_string))
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "gamedef.json is missing an id.".to_string())?;
    if id != folder {
        return Err(format!("The folder name (\"{folder}\") must match the game's id (\"{id}\")."));
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(path: &std::path::Path, files: &[(&str, &str)]) {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, content) in files {
            writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn game_zips_are_checked() {
        let folder = std::env::temp_dir().join(format!("gametable_zip_{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let good = folder.join("good.zip");
        zip_with(&good, &[("mygame/gamedef.json", r#"{"id":"mygame","name":"M","cards":[]}"#), ("mygame/a.png", "x")]);
        assert_eq!(validate_game_zip(good.to_str().unwrap()), Ok("mygame".to_string()));
        let two = folder.join("two.zip");
        zip_with(&two, &[("a/gamedef.json", "{}"), ("b/x.png", "x")]);
        assert_eq!(
            validate_game_zip(two.to_str().unwrap()),
            Err("The zip must contain exactly one folder at its root.".into())
        );
        let missing = folder.join("missing.zip");
        zip_with(&missing, &[("g/readme.txt", "x")]);
        assert_eq!(
            validate_game_zip(missing.to_str().unwrap()),
            Err("No gamedef.json found inside the folder.".into())
        );
        let mismatch = folder.join("mismatch.zip");
        zip_with(&mismatch, &[("g/gamedef.json", r#"{"id":"other"}"#)]);
        assert_eq!(
            validate_game_zip(mismatch.to_str().unwrap()),
            Err("The folder name (\"g\") must match the game's id (\"other\").".into())
        );
        assert_eq!(
            validate_game_zip(folder.join("nope.zip").to_str().unwrap()),
            Err("Couldn't read that zip file.".into())
        );
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
