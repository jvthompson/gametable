//! Admin tools: accounts, server settings, rescans, collections, game files and game installs.
//! Port of `admin.go`, `admin_files.go` and `admin_games.go`.

use std::io::Read;
use std::path::{Path, PathBuf};

use gametable_core::json::{self, JsonObject, JsonValue};

use super::auth::{bcrypt_hash, is_valid_email, is_valid_password_hash, normalize_email, valid_player_name};
use super::{Request, Server};
use crate::games::ServerGame;
use crate::log;
use crate::payload::sorted_object;
use crate::store::{ServerSettings, StoreError, UserUpdate};

impl Server {
    /// Every account, with whether it is online (the Go server's `adminUserList`).
    fn admin_user_list(&self) -> Option<JsonValue> {
        let users = self.store().users().ok()?;
        let state = self.state();
        let users = users.into_iter().map(|user| {
            sorted_object([
                ("id", user.id.as_str().into()),
                ("name", user.name.as_str().into()),
                ("email", user.email.as_str().into()),
                ("color", user.color.into()),
                ("avatarHash", user.avatar_hash.as_str().into()),
                ("isAdmin", user.is_admin.into()),
                ("disabled", user.disabled.into()),
                ("tokens", user.tokens.into()),
                ("createdAt", user.created_at.into()),
                ("online", state.client_by_user_id.contains_key(&user.id).into()),
            ])
        });
        Some(JsonValue::Array(users.collect()))
    }

    /// Disconnects an online user (their running-game seats are kept).
    pub(crate) fn kick_user(&self, user_id: &str, reason: &str) {
        let connection = {
            let state = self.state();
            state.client_by_user_id.get(user_id).and_then(|id| state.clients.get(id)).map(|c| c.connection.clone())
        };
        if let Some(connection) = connection {
            connection.close_with(reason);
        }
    }

    /// The library's scan problems; `null` when there are none, as Go encodes a nil slice.
    fn game_errors(&self) -> JsonValue {
        let library = self.library.read().unwrap();
        let errors = library.scan_errors();
        if errors.is_empty() { JsonValue::Null } else { crate::payload::text_array(errors) }
    }

    /// Rescans the game library and tells everyone, after an admin changes files on disk.
    pub(crate) fn rescan_library(&self) {
        if let Err(error) = self.library.write().unwrap().scan() {
            log!(self, "rescan after file change failed: {error}");
            return;
        }
        self.broadcast_games();
    }

    fn broadcast_games(&self) {
        self.broadcast_to_signed_in("games", sorted_object([("games", self.game_infos())]));
    }
}

/// Whether a settings payload decodes into Go's `Settings` struct: every known field must have
/// the right JSON type (whole numbers for the integer fields). `null` fields are left alone.
fn settings_have_valid_types(settings: &JsonValue) -> bool {
    const TEXT_FIELDS: [&str; 4] = ["serverName", "motd", "mode", "allowancePeriod"];
    const INTEGER_FIELDS: [&str; 8] = [
        "startingTokens",
        "packCost",
        "starterDeckCost",
        "allowanceAmount",
        "roomIdleTimeoutMin",
        "maxRooms",
        "chatHistory",
        "maxUploadMB",
    ];
    let JsonValue::Object(fields) = settings else { return matches!(settings, JsonValue::Null) };
    fields.iter().all(|(key, value)| {
        if matches!(value, JsonValue::Null) {
            return true;
        }
        if TEXT_FIELDS.contains(&key.as_str()) {
            value.as_str().is_some()
        } else if key == "registrationOpen" {
            value.as_bool().is_some()
        } else if INTEGER_FIELDS.contains(&key.as_str()) {
            value.as_f64().is_some_and(|number| number.is_finite() && number.fract() == 0.0)
        } else {
            true
        }
    })
}

/// Clamps settings to sensible values after an admin edit.
fn normalize_settings(settings: &mut ServerSettings) {
    settings.server_name = settings.server_name.trim().to_string();
    if settings.server_name.is_empty() {
        settings.server_name = ServerSettings::default().server_name;
    }
    if settings.mode != "collection" {
        settings.mode = "free".to_string();
    }
    if settings.allowance_period != "daily" && settings.allowance_period != "weekly" {
        settings.allowance_period = "none".to_string();
    }
    settings.starting_tokens = settings.starting_tokens.max(0);
    settings.pack_cost = settings.pack_cost.max(0);
    settings.starter_deck_cost = settings.starter_deck_cost.max(0);
    settings.allowance_amount = settings.allowance_amount.max(0);
    settings.room_idle_timeout_minutes = settings.room_idle_timeout_minutes.max(0);
    settings.max_rooms = settings.max_rooms.max(0);
    settings.chat_history = settings.chat_history.clamp(0, 1000);
    settings.max_upload_megabytes = settings.max_upload_megabytes.max(1);
}

/// Resolves an admin-supplied slash-separated path inside a game's folder, collapsing `..` so it
/// can never escape. `""` and `"/"` mean the folder itself. `None` for built-in games.
pub(crate) fn game_file_path(game: &ServerGame, relative_path: &str) -> Option<PathBuf> {
    let folder = game.folder.as_ref()?;
    Some(folder.join(clean_relative_path(relative_path)))
}

/// The components of a path after resolving `.` and `..` against a root it cannot climb out of.
/// Both slash kinds separate components.
fn clean_relative_path(relative_path: &str) -> PathBuf {
    let mut components: Vec<&str> = Vec::new();
    for component in relative_path.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            _ => components.push(component),
        }
    }
    components.iter().collect()
}

/// Go's `path.Clean` for slash-separated paths.
fn go_clean_slash_path(path: &str) -> String {
    let is_rooted = path.starts_with('/');
    let mut components: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.last().is_some_and(|last| *last != "..") {
                    components.pop();
                } else if !is_rooted {
                    components.push("..");
                }
            }
            _ => components.push(component),
        }
    }
    let joined = components.join("/");
    match (is_rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

/// Whether a cleaned zip entry name could write outside the game folder. Backslashes count as
/// separators too (Go on Windows would follow them).
fn is_unsafe_zip_name(cleaned: &str) -> bool {
    cleaned == ".."
        || cleaned.starts_with("../")
        || cleaned.starts_with('/')
        || cleaned.starts_with('\\')
        || cleaned.split(['/', '\\']).any(|component| component == "..")
        || cleaned.as_bytes().get(1) == Some(&b':')
}

/// `^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$`.
fn is_valid_game_id(id: &str) -> bool {
    let mut characters = id.chars();
    let Some(first) = characters.next() else { return false };
    first.is_ascii_alphanumeric()
        && id.len() <= 64
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
}

impl Request<'_> {
    pub(super) fn handle_admin_users(&self) {
        match self.server.admin_user_list() {
            Some(users) => self.reply(JsonObject::new().with("users", users)),
            None => self.fail("internal", "Could not list users."),
        }
    }

    pub(super) fn handle_admin_user_update(&self, admin: &crate::store::User) {
        let server = self.server;
        let fields = self.fields;
        let user_id = fields.text("id");
        let Ok(target) = server.store().user_by_id(&user_id) else {
            self.fail("notFound", "That user no longer exists.");
            return;
        };
        let mut update = UserUpdate::default();
        if let Some(name) = fields.optional_text("name") {
            let (name, problem) = valid_player_name(&name);
            if let Some(problem) = problem {
                self.fail("badName", problem);
                return;
            }
            update.name = Some(name);
        }
        if let Some(email) = fields.optional_text("email") {
            let email = normalize_email(&email);
            if !is_valid_email(&email) {
                self.fail("badEmail", "Enter a valid email address.");
                return;
            }
            update.email = Some(email);
        }
        update.is_admin = fields.optional_flag("isAdmin");
        update.disabled = fields.optional_flag("disabled");
        let removes_admin = target.is_admin && (update.is_admin == Some(false) || update.disabled == Some(true));
        if removes_admin && server.store().active_admin_count().unwrap_or(0) <= 1 {
            self.fail("lastAdmin", "The server needs at least one active admin.");
            return;
        }
        if let Some(password_hash) = fields.optional_text("newPasswordHash") {
            if !is_valid_password_hash(&password_hash) {
                self.fail("badPassword", "Invalid password.");
                return;
            }
            update.pass_hash = bcrypt_hash(&password_hash);
        }
        if let Err(error) = server.store().update_user(&user_id, &update) {
            match error {
                StoreError::NameTaken => self.fail("nameTaken", "That player name is taken."),
                StoreError::EmailTaken => self.fail("emailTaken", "Another account uses that email address."),
                _ => self.fail("internal", "Could not update the user."),
            }
            return;
        }
        if let Some(tokens) = fields.optional_integer("tokensSet") {
            let set = server.store().set_tokens(&user_id, tokens, &format!("set by {}", admin.name));
            if let Ok(balance) = set {
                server.set_online_tokens(&user_id, balance);
            }
        }
        if let Some(tokens) = fields.optional_integer("tokensAdd").filter(|tokens| *tokens != 0) {
            let added = server.store().add_tokens(&user_id, tokens, &format!("granted by {}", admin.name));
            match added {
                Ok(balance) => server.set_online_tokens(&user_id, balance),
                Err(StoreError::NotEnoughTokens) => {
                    self.fail("notEnoughTokens", "That would leave a negative balance.");
                    return;
                }
                Err(_) => {}
            }
        }
        let fresh = server.store().user_by_id(&user_id);
        if let Ok(fresh) = fresh {
            if fresh.disabled {
                server.kick_user(&user_id, "accountDisabled");
            } else {
                server.refresh_user(&fresh);
            }
        }
        self.reply(JsonObject::new().with("users", server.admin_user_list().unwrap_or(JsonValue::Null)));
    }

    pub(super) fn handle_admin_user_delete(&self, admin: &crate::store::User) {
        let server = self.server;
        let user_id = self.fields.text("id");
        if user_id == admin.id {
            self.fail("self", "You can't delete your own account.");
            return;
        }
        let Ok(target) = server.store().user_by_id(&user_id) else {
            self.fail("notFound", "That user no longer exists.");
            return;
        };
        if target.is_admin && server.store().active_admin_count().unwrap_or(0) <= 1 {
            self.fail("lastAdmin", "The server needs at least one active admin.");
            return;
        }
        server.kick_user(&user_id, "accountDeleted");
        if server.store().delete_user(&user_id).is_err() {
            self.fail("internal", "Could not delete the user.");
            return;
        }
        log!(server, "{} deleted account {:?}", admin.name, target.name);
        self.reply(JsonObject::new().with("users", server.admin_user_list().unwrap_or(JsonValue::Null)));
    }

    pub(super) fn handle_admin_clear_collection(&self, admin: &crate::store::User) {
        let server = self.server;
        let user_id = self.fields.text("id");
        let game_id = self.fields.text("gameId");
        let Ok(target) = server.store().user_by_id(&user_id) else {
            self.fail("notFound", "That user no longer exists.");
            return;
        };
        let Some(game_name) = server.library.read().unwrap().game(&game_id).map(|game| game.name.clone()) else {
            self.fail("noGame", "The server doesn't have that game.");
            return;
        };
        if server.store().clear_collection(&user_id, &game_id).is_err() {
            self.fail("internal", "Could not clear the collection.");
            return;
        }
        log!(server, "{} cleared {}'s collection for {game_name}", admin.name, target.name);
        self.reply_empty();
    }

    pub(super) fn handle_admin_kick(&self) {
        self.server.kick_user(&self.fields.text("id"), "kicked");
        self.reply_empty();
    }

    pub(super) fn handle_admin_settings_get(&self) {
        self.reply(
            JsonObject::new()
                .with("settings", self.server.settings().to_json())
                .with("gameErrors", self.server.game_errors()),
        );
    }

    pub(super) fn handle_admin_settings_set(&self, admin: &crate::store::User) {
        let server = self.server;
        let mut settings = server.settings();
        // Decoded over the current values so a partial update keeps the rest.
        let Some(partial) = self.fields.raw("settings").filter(|partial| settings_have_valid_types(partial)) else {
            self.fail("badSettings", "Invalid settings.");
            return;
        };
        settings.merge_from_json(partial);
        normalize_settings(&mut settings);
        if server.store().save_settings(&settings).is_err() {
            self.fail("internal", "Could not save settings.");
            return;
        }
        server.state().settings = settings.clone();
        log!(server, "{} updated server settings (mode {})", admin.name, settings.mode);
        self.reply(JsonObject::new().with("settings", settings.to_json()));
        server.broadcast_server_info();
    }

    pub(super) fn handle_admin_rescan(&self) {
        let server = self.server;
        let scanned = server.library.write().unwrap().scan();
        if let Err(error) = scanned {
            self.fail("internal", &format!("Could not scan the games folder: {error}"));
            return;
        }
        self.reply(JsonObject::new().with("games", server.game_infos()).with("gameErrors", server.game_errors()));
        server.broadcast_games();
    }

    // -- Game files ----------------------------------------------------------

    /// The game's folder and the resolved path, or a failure already sent.
    fn resolve_game_file(&self, game_id: &str, relative_path: &str) -> Option<(String, PathBuf, PathBuf)> {
        let library = self.server.library.read().unwrap();
        let Some(game) = library.game(game_id) else {
            drop(library);
            self.fail("noGame", "The server doesn't have that game.");
            return None;
        };
        let (Some(folder), Some(full_path)) = (game.folder.clone(), game_file_path(game, relative_path)) else {
            drop(library);
            self.fail("noFolder", "This game has no files on disk.");
            return None;
        };
        Some((game.name.clone(), folder, full_path))
    }

    pub(super) fn handle_admin_file_list(&self) {
        let game_id = self.fields.text("gameId");
        let relative_path = self.fields.text("path");
        let Some((_, _, full_path)) = self.resolve_game_file(&game_id, &relative_path) else { return };
        let Ok(items) = std::fs::read_dir(&full_path) else {
            self.fail("notFound", "That folder doesn't exist.");
            return;
        };
        let mut entries: Vec<(String, bool, u64, i64)> = items
            .filter_map(Result::ok)
            .filter_map(|item| {
                let metadata = item.metadata().ok()?;
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |elapsed| elapsed.as_secs() as i64);
                Some((item.file_name().to_string_lossy().into_owned(), metadata.is_dir(), metadata.len(), modified))
            })
            .collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        let entries = entries.into_iter().map(|(name, is_directory, size, modified)| {
            JsonValue::Object(
                JsonObject::new()
                    .with("name", name)
                    .with("isDir", is_directory)
                    .with("size", size as f64)
                    .with("modTime", modified as f64),
            )
        });
        self.reply(
            JsonObject::new()
                .with("gameId", game_id)
                .with("path", relative_path)
                .with("entries", JsonValue::Array(entries.collect())),
        );
    }

    pub(super) fn handle_admin_file_delete(&self, admin: &crate::store::User) {
        let game_id = self.fields.text("gameId");
        let relative_path = self.fields.text("path");
        let Some((game_name, folder, full_path)) = self.resolve_game_file(&game_id, &relative_path) else { return };
        if full_path == folder {
            self.fail("badPath", "Can't delete the game's folder.");
            return;
        }
        let Ok(metadata) = std::fs::metadata(&full_path) else {
            self.fail("notFound", "That file no longer exists.");
            return;
        };
        let removed = if metadata.is_dir() {
            match std::fs::read_dir(&full_path) {
                Err(_) => {
                    self.fail("internal", "Could not read that folder.");
                    return;
                }
                Ok(mut items) => {
                    if items.next().is_some() {
                        self.fail("notEmpty", "Delete the files inside it first.");
                        return;
                    }
                    std::fs::remove_dir(&full_path)
                }
            }
        } else {
            std::fs::remove_file(&full_path)
        };
        if removed.is_err() {
            self.fail("internal", "Could not delete that.");
            return;
        }
        log!(self.server, "{} deleted {relative_path:?} from {game_name}", admin.name);
        self.server.rescan_library();
        self.reply_empty();
    }

    // -- Games ---------------------------------------------------------------

    pub(super) fn handle_admin_game_delete(&self, admin: &crate::store::User) {
        let server = self.server;
        let game_id = self.fields.text("gameId");
        let (game_name, folder) = {
            let library = server.library.read().unwrap();
            let Some(game) = library.game(&game_id) else {
                drop(library);
                self.fail("noGame", "The server doesn't have that game.");
                return;
            };
            (game.name.clone(), game.folder.clone())
        };
        let Some(folder) = folder else {
            self.fail("builtIn", "This game is built into the server and can't be removed.");
            return;
        };
        let rooms_to_close: Vec<String> =
            server.state().rooms.values().filter(|room| room.game_id == game_id).map(|room| room.id.clone()).collect();
        for room_id in rooms_to_close {
            server.close_room(&room_id, "gameRemoved");
        }
        if std::fs::remove_dir_all(&folder).is_err() && folder.exists() {
            self.fail("internal", "Could not delete the game's files.");
            return;
        }
        log!(server, "{} removed game {game_name:?}", admin.name);
        server.rescan_library();
        self.reply_empty();
    }

    pub(super) fn handle_admin_game_install(&self, admin: &crate::store::User) {
        let upload = self.fields.text("upload");
        if upload.is_empty() || upload.contains(['/', '\\']) {
            self.fail("badUpload", "Invalid upload.");
            return;
        }
        let games_folder = self.server.library.read().unwrap().games_folder().to_path_buf();
        let zip_path = games_folder.join(".uploads").join(&upload);
        let installed = self.install_game_from_zip(&zip_path, &games_folder);
        let _ = std::fs::remove_file(&zip_path);
        match installed {
            Ok(game_id) => {
                log!(self.server, "{} installed game {game_id:?} from an upload", admin.name);
                self.server.rescan_library();
                self.reply(JsonObject::new().with("gameId", game_id));
            }
            Err((code, message)) => self.fail(code, &message),
        }
    }

    /// Checks an uploaded zip (exactly one root folder named after the game's id, holding a
    /// `gamedef.json`, within the size limit) and extracts it into the games folder.
    fn install_game_from_zip(&self, zip_path: &Path, games_folder: &Path) -> Result<String, (&'static str, String)> {
        let bad_zip = || ("badZip", "That file isn't a valid zip archive.".to_string());
        let file = std::fs::File::open(zip_path).map_err(|_| bad_zip())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|_| bad_zip())?;

        let max_upload_megabytes = self.server.settings().max_upload_megabytes;
        let max_zip_bytes = max_upload_megabytes.saturating_mul(1 << 20);
        let mut root = String::new();
        let mut definition_index = None;
        let mut total_size: i64 = 0;
        for index in 0..archive.len() {
            let entry = archive.by_index_raw(index).map_err(|_| bad_zip())?;
            let name = go_clean_slash_path(&entry.name().map_err(|_| bad_zip())?);
            if is_unsafe_zip_name(&name) {
                return Err(("badZip", "The zip contains an unsafe path.".to_string()));
            }
            let (first_segment, rest) = match name.split_once('/') {
                Some((first, rest)) => (first, Some(rest)),
                None => (name.as_str(), None),
            };
            if root.is_empty() {
                root = first_segment.to_string();
            } else if first_segment != root {
                return Err(("badShape", "The zip must contain exactly one folder at its root.".to_string()));
            }
            if rest.is_some_and(|rest| rest.eq_ignore_ascii_case("gamedef.json")) {
                definition_index = Some(index);
            }
            total_size = total_size.saturating_add(entry.size() as i64);
            if total_size > max_zip_bytes {
                return Err(("tooLarge", format!("That game is larger than the {max_upload_megabytes} MB limit.")));
            }
        }
        if root.is_empty() {
            return Err(("badShape", "The zip is empty.".to_string()));
        }
        let Some(definition_index) = definition_index else {
            return Err(("noGameDef", "No gamedef.json found inside the folder.".to_string()));
        };
        let mut definition_bytes = Vec::new();
        let read_definition = archive
            .by_index(definition_index)
            .map_err(|_| ())
            .and_then(|mut entry| entry.read_to_end(&mut definition_bytes).map_err(|_| ()));
        if read_definition.is_err() {
            return Err(("badZip", "Could not read gamedef.json.".to_string()));
        }
        let game_id =
            definition_id(&definition_bytes).ok_or(("noGameDef", "gamedef.json is missing an id.".to_string()))?;
        if game_id != root {
            return Err(("idMismatch", format!("The folder name ({root:?}) must match the game's id ({game_id:?}).")));
        }
        if !is_valid_game_id(&game_id) {
            return Err(("badId", "The game's id can only contain letters, numbers, - and _.".to_string()));
        }
        if self.server.library.read().unwrap().game(&game_id).is_some() {
            return Err(("exists", "A game with this id already exists. Remove it first to replace it.".to_string()));
        }

        let target = games_folder.join(&game_id);
        std::fs::create_dir_all(&target)
            .map_err(|_| ("internal", "Could not create the game's folder.".to_string()))?;
        let extracted = extract_game_files(&mut archive, &root, &target);
        if extracted.is_err() {
            let _ = std::fs::remove_dir_all(&target);
        }
        extracted.map(|()| game_id)
    }
}

/// The `id` of a game definition, as Go reads `struct{ID string}` (a BOM or a non-string id fails).
fn definition_id(definition_bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(definition_bytes).ok()?;
    if text.starts_with('\u{feff}') {
        return None;
    }
    let definition = json::decode(text).ok()?;
    if !matches!(definition, JsonValue::Object(_) | JsonValue::Null) {
        return None;
    }
    let id = match definition.get("id") {
        None | Some(JsonValue::Null) => String::new(),
        Some(JsonValue::String(id)) => id.clone(),
        Some(_) => return None,
    };
    (!id.is_empty()).then_some(id)
}

fn extract_game_files(
    archive: &mut zip::ZipArchive<std::fs::File>,
    root: &str,
    target: &Path,
) -> Result<(), (&'static str, String)> {
    let root_prefix = format!("{root}/");
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|_| ("badZip", "Could not read a file.".to_string()))?;
        let raw_name = entry.name().map_err(|_| ("badZip", "Could not read a file.".to_string()))?.into_owned();
        let name = go_clean_slash_path(&raw_name);
        let relative = name.strip_prefix(&root_prefix).unwrap_or(&name).to_string();
        if relative == root || relative.is_empty() {
            continue; // the root folder entry itself
        }
        let destination = target.join(clean_relative_path(&relative));
        let could_not_create = || ("internal", format!("Could not create {relative}."));
        if entry.is_dir() {
            std::fs::create_dir_all(&destination).map_err(|_| could_not_create())?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|_| could_not_create())?;
        }
        let could_not_write = || ("internal", format!("Could not write {relative}."));
        let mut output = std::fs::File::create(&destination).map_err(|_| could_not_write())?;
        std::io::copy(&mut entry, &mut output).map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidData {
                ("badZip", format!("Could not read {raw_name}."))
            } else {
                could_not_write()
            }
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_paths_clean_like_go() {
        assert_eq!(go_clean_slash_path("a/b/../c/"), "a/c");
        assert_eq!(go_clean_slash_path("../x"), "../x");
        assert_eq!(go_clean_slash_path("a/../../x"), "../x");
        assert_eq!(go_clean_slash_path("/a/../.."), "/");
        assert_eq!(go_clean_slash_path(""), ".");
        assert_eq!(go_clean_slash_path("./g/"), "g");
    }

    #[test]
    fn admin_paths_cannot_escape() {
        assert_eq!(clean_relative_path("../../etc/passwd"), PathBuf::from("etc").join("passwd"));
        assert_eq!(clean_relative_path("/"), PathBuf::new());
        assert_eq!(clean_relative_path("s1/../s1/c1.png"), PathBuf::from("s1").join("c1.png"));
        assert!(is_unsafe_zip_name("g/..\\..\\evil"));
        assert!(is_unsafe_zip_name("/abs"));
        assert!(!is_unsafe_zip_name("g/s1/c1.png"));
    }

    #[test]
    fn game_ids() {
        assert!(is_valid_game_id("mini"));
        assert!(is_valid_game_id("a-b_C9"));
        assert!(!is_valid_game_id("-a"));
        assert!(!is_valid_game_id(""));
        assert!(!is_valid_game_id("a b"));
        assert!(!is_valid_game_id(&"a".repeat(65)));
    }

    #[test]
    fn settings_types() {
        let check = |text: &str| settings_have_valid_types(&json::decode(text).unwrap());
        assert!(check(r#"{"mode":"collection","packCost":2}"#));
        assert!(check("null"));
        assert!(!check(r#"{"packCost":"2"}"#));
        assert!(!check(r#"{"packCost":2.5}"#));
        assert!(!check(r#"{"registrationOpen":1}"#));
        assert!(!check("[]"));
    }
}
