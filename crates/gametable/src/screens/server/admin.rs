//! Server Administration (admins only): accounts, server settings, games and running rooms, and
//! the files of each game. Port of `src/ui/screens/server/admin.lua`, with the request helpers of
//! `src/net/admin_file_manager.lua` and `admin_game_manager.lua`.

use std::collections::HashSet;
use std::path::Path;

use gametable_core::json::{JsonObject, JsonValue};
use gametable_core::util::trim_lua_whitespace;

use crate::app::{Frame, Screen};
use crate::data::file_system::list_files_recursive;
use crate::net::admin_files::{FileUpload, UploadProgress, validate_game_zip};
use crate::net::server_client::{SharedServer, password_error, password_hash, response_message, response_ok};
use crate::net::server_models::{ServerGameInfo, ServerUser};
use crate::screens::file_picker::{FileFilter, FilePicker, FilePickerResponse, PickerMode};
use crate::screens::server::{format_bytes, user_row};
use crate::ui::components::TAB_BAR_HEIGHT;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::dropdown::DropdownOptions;
use crate::ui::overlays::{ConfirmResponse, DialogAction, DialogResponse, DialogSpec};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::{Weight, fixed, with_alpha};
use crate::ui::widgets::{ButtonOptions, ButtonVariant, Choice, IconButtonOptions, ScrollbarStyle, ToggleOptions};

const TABS: [&str; 4] = ["Users", "Server", "Games", "Files"];
const ZIP_FILTERS: &[FileFilter] = &[FileFilter { name: "Zip archive", extensions: &["zip"] }];

fn only_digits(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|character| character.is_ascii_digit())
}

fn signed_digits(text: &str) -> bool {
    text.strip_prefix('-').unwrap_or(text).chars().all(|character| character.is_ascii_digit())
}

/// `remoteDirname(path)`: the folder part of a server path ("" at the game's root).
fn remote_dirname(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

fn join_remote(folder: &str, name: &str) -> String {
    if folder.is_empty() { name.to_string() } else { format!("{folder}/{name}") }
}

fn file_name_of(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_string(), |name| name.to_string_lossy().into_owned())
}

/// The Server tab's form: text fields hold numbers as text, parsed on save.
struct SettingsForm {
    server_name: String,
    motd: String,
    registration_open: bool,
    mode: String,
    starting_tokens: String,
    pack_cost: String,
    starter_deck_cost: String,
    allowance_amount: String,
    allowance_period: String,
    room_idle_timeout_minutes: String,
    max_rooms: String,
    chat_history: String,
    max_upload_megabytes: String,
}

fn number_text(settings: &JsonValue, key: &str, default: f64) -> String {
    let value = settings.get(key).and_then(JsonValue::as_f64).unwrap_or(default);
    if value.fract() == 0.0 { format!("{}", value as i64) } else { value.to_string() }
}

fn string_of(settings: &JsonValue, key: &str) -> String {
    settings.get(key).and_then(JsonValue::as_str).unwrap_or("").to_string()
}

impl SettingsForm {
    fn from_settings(settings: &JsonValue) -> SettingsForm {
        SettingsForm {
            server_name: string_of(settings, "serverName"),
            motd: string_of(settings, "motd"),
            registration_open: settings.get("registrationOpen").and_then(JsonValue::as_bool).unwrap_or(false),
            mode: string_of(settings, "mode"),
            starting_tokens: number_text(settings, "startingTokens", 0.0),
            pack_cost: number_text(settings, "packCost", 0.0),
            starter_deck_cost: number_text(settings, "starterDeckCost", 3.0),
            allowance_amount: number_text(settings, "allowanceAmount", 0.0),
            allowance_period: string_of(settings, "allowancePeriod"),
            room_idle_timeout_minutes: number_text(settings, "roomIdleTimeoutMin", 0.0),
            max_rooms: number_text(settings, "maxRooms", 0.0),
            chat_history: number_text(settings, "chatHistory", 0.0),
            max_upload_megabytes: number_text(settings, "maxUploadMB", 0.0),
        }
    }
}

/// The Edit User dialog's fields.
struct UserEditor {
    user: ServerUser,
    name: String,
    email: String,
    is_admin: bool,
    disabled: bool,
    tokens: String,
    grant: String,
    password: String,
    clear_game_id: Option<String>,
}

/// One entry of a game folder listing.
#[derive(Debug, Clone)]
struct RemoteEntry {
    name: String,
    is_folder: bool,
    size: u64,
}

fn parse_entries(response: &JsonValue) -> Vec<RemoteEntry> {
    response
        .get("entries")
        .and_then(JsonValue::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| RemoteEntry {
                    name: entry.get("name").and_then(JsonValue::as_str).unwrap_or("").to_string(),
                    is_folder: entry.get("isDir").and_then(JsonValue::as_bool).unwrap_or(false),
                    size: entry.get("size").and_then(JsonValue::as_f64).unwrap_or(0.0) as u64,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One file to upload and where it goes in the game's folder.
#[derive(Debug, Clone)]
struct PlannedUpload {
    local_path: String,
    destination: String,
}

/// `A:beginUpload` and `A:runUploadQueue`.
enum UploadBatch {
    /// Listing every destination folder to find files that would be overwritten.
    Checking {
        plan: Vec<PlannedUpload>,
        listings: Vec<(u64, String)>,
        existing: HashSet<String>,
    },
    ConfirmOverwrite {
        plan: Vec<PlannedUpload>,
        conflicts: Vec<String>,
    },
    Running {
        plan: Vec<PlannedUpload>,
        index: usize,
        upload: FileUpload,
        errors: Vec<String>,
    },
}

/// `AGM.installFromZip`: the zip upload, then the install request.
enum GameInstall {
    Uploading { upload: FileUpload, upload_name: String },
    Installing { rid: u64 },
}

/// A yes/no question the screen is asking.
enum AdminConfirm {
    ClearCollection { user_id: String, user_name: String, game_id: String, game_name: String },
    DeleteAccount { user_id: String, user_name: String },
    ShutDown,
    RemoveGame { game_id: String, game_name: String },
    EndRoom { room_id: String, room_name: String },
    DeleteFile { name: String, path: String, is_folder: bool },
}

/// A request waiting for its answer, with what it was for.
enum AdminRequest {
    Users(u64),
    Settings(u64),
    UpdateUser { rid: u64, saved_name: Option<String> },
    ClearCollection { rid: u64, user_name: String, game_name: String },
    DeleteUser(u64),
    Kick(u64),
    SaveSettings(u64),
    Shutdown(u64),
    Rescan(u64),
    RemoveGame(u64),
    ListFiles { rid: u64, game_id: String },
    DeleteFile(u64),
}

impl AdminRequest {
    fn rid(&self) -> u64 {
        match self {
            AdminRequest::Users(rid)
            | AdminRequest::Settings(rid)
            | AdminRequest::DeleteUser(rid)
            | AdminRequest::Kick(rid)
            | AdminRequest::SaveSettings(rid)
            | AdminRequest::Shutdown(rid)
            | AdminRequest::Rescan(rid)
            | AdminRequest::RemoveGame(rid)
            | AdminRequest::DeleteFile(rid) => *rid,
            AdminRequest::UpdateUser { rid, .. }
            | AdminRequest::ClearCollection { rid, .. }
            | AdminRequest::ListFiles { rid, .. } => *rid,
        }
    }
}

/// Which file picker is open.
enum PickerPurpose {
    GameZip,
    Files,
    Folder,
}

pub struct AdminScreen {
    server: SharedServer,
    tab: usize,
    search: String,
    users: Option<Vec<ServerUser>>,
    settings: Option<JsonValue>,
    form: Option<SettingsForm>,
    game_errors: Vec<String>,
    requests: Vec<AdminRequest>,
    user_editor: Option<UserEditor>,
    confirm: Option<AdminConfirm>,
    scanning: bool,
    shutting_down: bool,
    install: Option<GameInstall>,
    picker: Option<(FilePicker, PickerPurpose)>,
    files_game_id: Option<String>,
    files_path: String,
    files_entries: Option<Vec<RemoteEntry>>,
    upload_batch: Option<UploadBatch>,
}

impl AdminScreen {
    pub fn new(server: SharedServer) -> AdminScreen {
        let mut screen = AdminScreen {
            server,
            tab: 0,
            search: String::new(),
            users: None,
            settings: None,
            form: None,
            game_errors: Vec::new(),
            requests: Vec::new(),
            user_editor: None,
            confirm: None,
            scanning: false,
            shutting_down: false,
            install: None,
            picker: None,
            files_game_id: None,
            files_path: String::new(),
            files_entries: None,
            upload_batch: None,
        };
        screen.refresh_users();
        screen.refresh_settings();
        screen
    }

    fn request(&mut self, message_type: &str, payload: JsonObject) -> u64 {
        self.server.borrow_mut().request(message_type, payload)
    }

    fn refresh_users(&mut self) {
        let rid = self.request("adminUsers", JsonObject::new());
        self.requests.push(AdminRequest::Users(rid));
    }

    fn refresh_settings(&mut self) {
        let rid = self.request("adminSettingsGet", JsonObject::new());
        self.requests.push(AdminRequest::Settings(rid));
    }

    fn refresh_files(&mut self) {
        let Some(game_id) = self.files_game_id.clone() else { return };
        self.files_entries = None;
        let payload = JsonObject::new().with("gameId", game_id.as_str()).with("path", self.files_path.as_str());
        let rid = self.request("adminFileList", payload);
        self.requests.push(AdminRequest::ListFiles { rid, game_id });
    }

    fn navigate_files(&mut self, path: String) {
        self.files_path = path;
        self.refresh_files();
    }

    fn take_users(&mut self, response: &JsonValue) {
        if let Some(users) = response.get("users").and_then(JsonValue::as_array) {
            self.users = Some(users.iter().map(ServerUser::parse).collect());
        }
    }

    fn take_game_errors(&mut self, response: &JsonValue) {
        self.game_errors = response
            .get("gameErrors")
            .and_then(JsonValue::as_array)
            .map(|errors| errors.iter().filter_map(|error| error.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
    }

    fn finish_requests(&mut self, frame: &mut Frame) {
        let mut index = 0;
        while index < self.requests.len() {
            let rid = self.requests[index].rid();
            let Some(response) = self.server.borrow_mut().take_response(rid) else {
                index += 1;
                continue;
            };
            let request = self.requests.remove(index);
            let ok = response_ok(&response);
            match request {
                AdminRequest::Users(_) => {
                    if ok {
                        self.take_users(&response);
                    } else {
                        frame.ui.toast(&response_message(&response, "Couldn't load users."), None);
                    }
                }
                AdminRequest::Settings(_) => {
                    if ok && let Some(settings) = response.get("settings") {
                        self.form = Some(SettingsForm::from_settings(settings));
                        self.settings = Some(settings.clone());
                        self.take_game_errors(&response);
                    }
                }
                AdminRequest::UpdateUser { saved_name, .. } => {
                    if ok {
                        self.take_users(&response);
                        if let Some(name) = saved_name {
                            frame.ui.toast(&format!("Saved {name}."), None);
                        }
                    } else {
                        frame.ui.alert("Couldn't Update User", &response_message(&response, "Something went wrong."));
                    }
                }
                AdminRequest::ClearCollection { user_name, game_name, .. } => {
                    if ok {
                        frame.ui.toast(&format!("Cleared {user_name}'s collection for \"{game_name}\"."), None);
                    } else {
                        frame
                            .ui
                            .alert("Couldn't Clear Collection", &response_message(&response, "Something went wrong."));
                    }
                }
                AdminRequest::DeleteUser(_) => {
                    if ok {
                        self.take_users(&response);
                    } else {
                        frame.ui.alert("Couldn't Delete", &response_message(&response, "Something went wrong."));
                    }
                }
                AdminRequest::Kick(_) => self.refresh_users(),
                AdminRequest::SaveSettings(_) => {
                    if ok {
                        frame.ui.toast("Server settings saved.", None);
                        self.refresh_settings();
                    } else {
                        frame.ui.alert("Couldn't Save Settings", &response_message(&response, "Something went wrong."));
                    }
                }
                AdminRequest::Shutdown(_) => {
                    self.shutting_down = false;
                    if !ok {
                        frame.ui.alert("Couldn't Shut Down", &response_message(&response, "Something went wrong."));
                    }
                }
                AdminRequest::Rescan(_) => {
                    self.scanning = false;
                    if ok {
                        let games: Vec<ServerGameInfo> = response
                            .get("games")
                            .and_then(JsonValue::as_array)
                            .map(|games| games.iter().map(ServerGameInfo::parse).collect())
                            .unwrap_or_default();
                        frame.ui.toast(&format!("Found {} game(s).", games.len()), None);
                        self.server.borrow_mut().games = games;
                        self.take_game_errors(&response);
                    } else {
                        frame.ui.alert("Couldn't Rescan", &response_message(&response, "Something went wrong."));
                    }
                }
                AdminRequest::RemoveGame(_) => {
                    if !ok {
                        frame.ui.alert("Couldn't Remove Game", &response_message(&response, "Something went wrong."));
                    }
                }
                AdminRequest::ListFiles { game_id, .. } => {
                    // The game picker may have moved on while this was in flight.
                    if self.files_game_id.as_deref() != Some(game_id.as_str()) {
                        continue;
                    }
                    if ok {
                        self.files_entries = Some(parse_entries(&response));
                    } else {
                        self.files_entries = Some(Vec::new());
                        frame.ui.toast(&response_message(&response, "Couldn't list files."), None);
                    }
                }
                AdminRequest::DeleteFile(_) => {
                    if ok {
                        self.refresh_files();
                    } else {
                        frame.ui.alert("Couldn't Delete", &response_message(&response, "Something went wrong."));
                    }
                }
            }
        }
    }

    // -- Users ---------------------------------------------------------------------------------

    fn edit_user(&mut self, user: &ServerUser) {
        self.user_editor = Some(UserEditor {
            user: user.clone(),
            name: user.name.clone(),
            email: user.email.clone(),
            is_admin: user.is_admin,
            disabled: user.disabled,
            tokens: user.tokens.to_string(),
            grant: String::new(),
            password: String::new(),
            clear_game_id: None,
        });
    }

    /// The Save button: builds the update. False keeps the dialog open.
    fn save_user(&mut self, frame: &mut Frame) -> bool {
        let Some(editor) = &self.user_editor else { return true };
        let user = &editor.user;
        let mut fields = JsonObject::new();
        let name = trim_lua_whitespace(&editor.name).to_string();
        let email = trim_lua_whitespace(&editor.email).to_string();
        if name != user.name {
            fields.insert("name", name.as_str());
        }
        let email_changed = email.to_lowercase() != user.email.to_lowercase();
        if email_changed {
            fields.insert("email", email.as_str());
        }
        if editor.is_admin != user.is_admin {
            fields.insert("isAdmin", editor.is_admin);
        }
        if editor.disabled != user.disabled {
            fields.insert("disabled", editor.disabled);
        }
        if let Ok(balance) = editor.tokens.parse::<i64>()
            && balance != user.tokens
        {
            fields.insert("tokensSet", balance as f64);
        }
        if let Ok(grant) = editor.grant.parse::<i64>()
            && grant != 0
        {
            fields.insert("tokensAdd", grant as f64);
        }
        if !editor.password.is_empty() {
            if let Some(error) = password_error(&editor.password, None) {
                frame.ui.toast(&error, None);
                return false;
            }
            // Hashed with the email the account will have after this save.
            let hash_email = if email_changed { email.as_str() } else { user.email.as_str() };
            fields.insert("newPasswordHash", password_hash(hash_email, &editor.password));
        }
        fields.insert("id", user.id.as_str());
        let rid = self.request("adminUserUpdate", fields);
        self.requests.push(AdminRequest::UpdateUser { rid, saved_name: Some(name) });
        true
    }

    fn show_user_editor(&mut self, frame: &mut Frame) {
        let Some(editor) = &mut self.user_editor else { return };
        let (my_id, games) = {
            let server = self.server.borrow();
            (server.my_id().to_string(), server.games.clone())
        };
        let is_self = editor.user.id == my_id;
        let title = editor.user.name.clone();
        let actions = [DialogAction::text("Cancel"), DialogAction::filled("Save")];
        let spec = DialogSpec { title: Some(&title), width: 460.0, actions: &actions, ..DialogSpec::default() };
        let background = frame.palette().surface_container_high;
        let mut clear_clicked = false;
        let mut kick_clicked = false;
        let mut delete_clicked = false;
        let response = frame.dialog("au:edit", &spec, |frame, x, y, width| {
            let palette = frame.palette();
            let mut row_y = y + 8.0;
            let options = TextFieldOptions {
                label_background: Some(background),
                max_length: Some(24),
                ..TextFieldOptions::label("Player name")
            };
            frame.text_field("au:name", x, row_y, width, &mut editor.name, &options);
            row_y += 48.0 + 12.0;
            let options =
                TextFieldOptions { label_background: Some(background), ..TextFieldOptions::label("Email address") };
            frame.text_field("au:email", x, row_y, width, &mut editor.email, &options);
            row_y += 48.0 + 16.0;
            editor.is_admin = frame.switch_row(
                "au:admin",
                "Server admin",
                x,
                row_y,
                width,
                editor.is_admin,
                &ToggleOptions::default(),
            );
            row_y += 40.0;
            let options = ToggleOptions::default().disabled(is_self);
            editor.disabled =
                frame.switch_row("au:disabled", "Account disabled", x, row_y, width, editor.disabled, &options);
            row_y += 48.0;
            frame.text("Tokens", x, row_y, TextOptions::new().medium());
            row_y += 28.0;
            let half = (width - 12.0) / 2.0;
            let options = TextFieldOptions {
                label_background: Some(background),
                filter: Some(&only_digits),
                ..TextFieldOptions::label("Balance")
            };
            frame.text_field("au:tokens", x, row_y, half, &mut editor.tokens, &options);
            let options = TextFieldOptions {
                label_background: Some(background),
                filter: Some(&signed_digits),
                ..TextFieldOptions::label("Grant (adds)")
            };
            frame.text_field("au:grant", x + half + 12.0, row_y, half, &mut editor.grant, &options);
            row_y += 48.0 + 16.0;
            frame.text("Reset password", x, row_y, TextOptions::new().medium());
            row_y += 28.0;
            let options = TextFieldOptions {
                password: true,
                label_background: Some(background),
                ..TextFieldOptions::label("New password (leave blank to keep)")
            };
            frame.text_field("au:pw", x, row_y, width, &mut editor.password, &options);
            row_y += 48.0 + 8.0;
            let note = "Tell the player their new password; they can change it from their profile.";
            frame.text(note, x, row_y, TextOptions::new().size(12.0).width(width).color(palette.on_surface_variant));
            row_y += 28.0;
            if !games.is_empty() {
                frame.text("Clear collection", x, row_y, TextOptions::new().medium());
                row_y += 28.0;
                let choices: Vec<Choice<String>> =
                    games.iter().map(|game| Choice::new(game.id.clone(), game.name.clone())).collect();
                let selected = editor.clear_game_id.clone().unwrap_or_else(|| games[0].id.clone());
                let button_options = ButtonOptions {
                    color: Some(fixed::RED),
                    ..ButtonOptions::variant(ButtonVariant::Outlined).id("au:clear")
                };
                let button_width = frame.button_width("Clear", &ButtonOptions::default());
                let dropdown_width = width - button_width - 12.0;
                let selected = frame.dropdown(
                    "au:cleargame",
                    x,
                    row_y,
                    dropdown_width,
                    &choices,
                    selected,
                    &DropdownOptions::default(),
                );
                editor.clear_game_id = Some(selected);
                clear_clicked = frame.button(
                    "Clear",
                    x + dropdown_width + 12.0,
                    row_y,
                    Some(button_width),
                    Some(48.0),
                    &button_options,
                );
                row_y += 48.0 + 16.0;
            }
            if !is_self {
                let mut button_x = x - 12.0;
                if editor.user.online {
                    let options = ButtonOptions::variant(ButtonVariant::Text).icon("logout").id("au:kick");
                    kick_clicked = frame.button("Disconnect", button_x, row_y, None, Some(40.0), &options);
                    button_x += frame.button_width("Disconnect", &ButtonOptions::default().icon("logout")) + 8.0;
                }
                let options = ButtonOptions {
                    color: Some(fixed::RED),
                    ..ButtonOptions::variant(ButtonVariant::Text).icon("delete_outline").id("au:del")
                };
                delete_clicked = frame.button("Delete Account", button_x, row_y, None, Some(40.0), &options);
                row_y += 40.0;
            }
            row_y - y
        });
        let (user_id, user_name, online) = (editor.user.id.clone(), editor.user.name.clone(), editor.user.online);
        if clear_clicked {
            let game = editor.clear_game_id.as_ref().and_then(|id| games.iter().find(|game| game.id == *id));
            if let Some(game) = game {
                self.confirm = Some(AdminConfirm::ClearCollection {
                    user_id: user_id.clone(),
                    user_name: user_name.clone(),
                    game_id: game.id.clone(),
                    game_name: game.name.clone(),
                });
            }
        }
        if kick_clicked && online {
            let rid = self.request("adminKick", JsonObject::new().with("id", user_id.as_str()));
            self.requests.push(AdminRequest::Kick(rid));
            self.user_editor = None;
            return;
        }
        if delete_clicked {
            self.confirm = Some(AdminConfirm::DeleteAccount { user_id, user_name });
        }
        match response {
            DialogResponse::Open | DialogResponse::Enter => {}
            DialogResponse::Action(1) => {
                if self.save_user(frame) {
                    self.user_editor = None;
                }
            }
            _ => self.user_editor = None,
        }
    }

    fn draw_users(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let options =
            TextFieldOptions { label_background: Some(palette.surface), ..TextFieldOptions::label("Search users") };
        frame.text_field("au:search", x, y, 360.0f32.min(width), &mut self.search, &options);
        if frame.icon_button(
            "refresh",
            x + width - 40.0,
            y + 4.0,
            &IconButtonOptions::tooltip("Refresh").id("au:refresh"),
        ) {
            self.refresh_users();
        }
        let mut row_y = y + 64.0;
        let Some(users) = self.users.clone() else {
            frame.spinner(x + width / 2.0, row_y + 40.0, 14.0, None);
            return;
        };
        let query = self.search.to_lowercase();
        let list: Vec<&ServerUser> = users
            .iter()
            .filter(|user| {
                query.is_empty()
                    || user.name.to_lowercase().contains(&query)
                    || user.email.to_lowercase().contains(&query)
            })
            .collect();
        let inner = width - 32.0;
        let mut column_x = x + 16.0;
        for (label, share) in [("Player", 0.34), ("Email", 0.34), ("Tokens", 0.12), ("Status", 0.2)] {
            frame.text(label, column_x, row_y, TextOptions::new().size(12.0).bold().color(palette.on_surface_variant));
            column_x += inner * share;
        }
        row_y += 24.0;
        frame.divider(x, row_y, width, None);
        let my_id = self.server.borrow().my_id().to_string();
        let offset = frame.begin_scroll("au:list", x, row_y, width, y + height - row_y);
        let mut user_y = row_y - offset;
        let mut clicked = None;
        for user in list {
            let region = frame.ui.region(&format!("au:row:{}", user.id), x, user_y, width, 52.0, RegionOptions::hand());
            if region.hovered {
                frame.fill_rect(x, user_y, width, 52.0, 0.0, with_alpha(palette.on_surface, 0.05));
            }
            let mut column_x = x + 16.0;
            user_row(
                frame,
                &self.server,
                user,
                column_x,
                user_y + 10.0,
                inner * 0.34 - 12.0,
                32.0,
                32.0,
                user.id == my_id,
            );
            column_x += inner * 0.34;
            let options = TextOptions::new().middle_of(52.0).width(inner * 0.34 - 12.0).ellipsis();
            frame.text(&user.email, column_x, user_y, options);
            column_x += inner * 0.34;
            frame.text(&user.tokens.to_string(), column_x, user_y, TextOptions::new().middle_of(52.0));
            column_x += inner * 0.12;
            let mut tags = Vec::new();
            if user.is_admin {
                tags.push("Admin");
            }
            if user.disabled {
                tags.push("Disabled");
            }
            if user.online {
                tags.push("Online");
            }
            let color = if user.disabled { fixed::RED } else { palette.on_surface_variant };
            let options = TextOptions::new().middle_of(52.0).size(13.0).width(inner * 0.2).color(color);
            frame.text(&tags.join(" · "), column_x, user_y, options);
            if region.clicked {
                clicked = Some(user.clone());
            }
            user_y += 52.0;
            frame.divider(x, user_y, width, Some(with_alpha(palette.outline_variant, 0.5)));
        }
        frame.end_scroll("au:list", user_y + offset - row_y, &ScrollbarStyle::default());
        if let Some(user) = clicked {
            self.edit_user(&user);
        }
    }

    // -- Server settings -----------------------------------------------------------------------

    fn save_settings(&mut self) {
        let (Some(form), Some(settings)) = (&self.form, &self.settings) else { return };
        let number = |text: &str, key: &str, default: f64| {
            text.parse::<f64>().ok().or_else(|| settings.get(key).and_then(JsonValue::as_f64)).unwrap_or(default)
        };
        let payload = JsonObject::new()
            .with("serverName", form.server_name.as_str())
            .with("motd", form.motd.as_str())
            .with("registrationOpen", form.registration_open)
            .with("mode", form.mode.as_str())
            .with("startingTokens", number(&form.starting_tokens, "startingTokens", 0.0))
            .with("packCost", number(&form.pack_cost, "packCost", 0.0))
            .with("starterDeckCost", number(&form.starter_deck_cost, "starterDeckCost", 3.0))
            .with("allowanceAmount", number(&form.allowance_amount, "allowanceAmount", 0.0))
            .with("allowancePeriod", form.allowance_period.as_str())
            .with("roomIdleTimeoutMin", number(&form.room_idle_timeout_minutes, "roomIdleTimeoutMin", 0.0))
            .with("maxRooms", number(&form.max_rooms, "maxRooms", 0.0))
            .with("chatHistory", number(&form.chat_history, "chatHistory", 0.0))
            .with("maxUploadMB", number(&form.max_upload_megabytes, "maxUploadMB", 0.0));
        let rid = self.request("adminSettingsSet", JsonObject::new().with("settings", payload));
        self.requests.push(AdminRequest::SaveSettings(rid));
    }

    fn section_label(frame: &mut Frame, text: &str, x: f32, y: f32) -> f32 {
        let color = frame.palette().primary;
        frame.text(text, x, y, TextOptions::new().size(12.0).bold().color(color));
        28.0
    }

    fn draw_server(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        if self.form.is_none() {
            frame.spinner(x + width / 2.0, y + 40.0, 14.0, None);
            return;
        }
        let column_width = 640.0f32.min(width);
        let column_x = x + (width - column_width) / 2.0;
        let background = Some(palette.surface);
        let offset = frame.begin_scroll("as:scroll", x, y, width, height);
        let mut row_y = y - offset;
        let save = {
            let Some(form) = &mut self.form else { return };
            let field = |label: &'static str, max_length: Option<usize>, digits: bool| TextFieldOptions {
                label_background: background,
                max_length,
                filter: if digits { Some(&only_digits) } else { None },
                ..TextFieldOptions::label(label)
            };
            row_y += Self::section_label(frame, "GENERAL", column_x, row_y);
            frame.text_field(
                "as:name",
                column_x,
                row_y,
                column_width,
                &mut form.server_name,
                &field("Server name", Some(60), false),
            );
            row_y += 48.0 + 12.0;
            frame.text_field(
                "as:motd",
                column_x,
                row_y,
                column_width,
                &mut form.motd,
                &field("Message of the day", Some(200), false),
            );
            row_y += 48.0 + 12.0;
            form.registration_open = frame.switch_row(
                "as:reg",
                "Allow new accounts",
                column_x,
                row_y,
                column_width,
                form.registration_open,
                &ToggleOptions::default(),
            );
            row_y += 48.0;
            row_y += Self::section_label(frame, "CARDS", column_x, row_y);
            let modes =
                [Choice::new("free".to_string(), "Free Play"), Choice::new("collection".to_string(), "Collection")];
            form.mode =
                frame.segmented("as:mode", column_x, row_y, column_width, &modes, form.mode.clone(), None, false);
            row_y += 40.0 + 8.0;
            let mode_help = if form.mode == "collection" {
                "Players build decks only from cards they own, and open packs with tokens. Games started now don't allow \
                 the Pack Generator, though the Card Library stays available during play."
            } else {
                "Players can use every card in every game. Collections are kept, in case you switch back."
            };
            let help_options = TextOptions::new().width(column_width).size(12.0).color(palette.on_surface_variant);
            frame.text(mode_help, column_x, row_y, help_options);
            row_y += frame.text_height(mode_help, column_width, 12.0, Weight::Regular) + 16.0;
            let quarter = (column_width - 36.0) / 4.0;
            let quarter_x = |index: f32| column_x + index * (quarter + 12.0);
            frame.text_field(
                "as:start",
                quarter_x(0.0),
                row_y,
                quarter,
                &mut form.starting_tokens,
                &field("Starting tokens", None, true),
            );
            frame.text_field(
                "as:cost",
                quarter_x(1.0),
                row_y,
                quarter,
                &mut form.pack_cost,
                &field("Tokens per pack", None, true),
            );
            frame.text_field(
                "as:starter",
                quarter_x(2.0),
                row_y,
                quarter,
                &mut form.starter_deck_cost,
                &field("Tokens per starter", None, true),
            );
            frame.text_field(
                "as:allow",
                quarter_x(3.0),
                row_y,
                quarter,
                &mut form.allowance_amount,
                &field("Allowance", None, true),
            );
            row_y += 48.0 + 12.0;
            frame.text(
                "Allowance is given",
                column_x,
                row_y + 10.0,
                TextOptions::new().color(palette.on_surface_variant),
            );
            let label_width = frame.text_width("Allowance is given", 14.0, Weight::Regular) + 16.0;
            let periods = [
                Choice::new("none".to_string(), "Never"),
                Choice::new("daily".to_string(), "Daily"),
                Choice::new("weekly".to_string(), "Weekly"),
            ];
            form.allowance_period = frame.segmented(
                "as:period",
                column_x + label_width,
                row_y,
                column_width - label_width,
                &periods,
                form.allowance_period.clone(),
                None,
                false,
            );
            row_y += 40.0 + 8.0;
            let allowance_help = "New accounts get the starting tokens. A starter deck (a built-in deck marked isStarter) \
                                  gives the player its cards and a copy of the deck. The allowance is added once per day \
                                  or week, the next time a player signs in (or while they're online).";
            frame.text(allowance_help, column_x, row_y, help_options);
            row_y += frame.text_height(allowance_help, column_width, 12.0, Weight::Regular) + 20.0;
            row_y += Self::section_label(frame, "LIMITS", column_x, row_y);
            frame.text_field(
                "as:idle",
                quarter_x(0.0),
                row_y,
                quarter,
                &mut form.room_idle_timeout_minutes,
                &field("Idle game timeout (min)", None, true),
            );
            frame.text_field(
                "as:rooms",
                quarter_x(1.0),
                row_y,
                quarter,
                &mut form.max_rooms,
                &field("Max games (0 = no limit)", None, true),
            );
            frame.text_field(
                "as:chat",
                quarter_x(2.0),
                row_y,
                quarter,
                &mut form.chat_history,
                &field("Chat history", None, true),
            );
            frame.text_field(
                "as:upload",
                quarter_x(3.0),
                row_y,
                quarter,
                &mut form.max_upload_megabytes,
                &field("Max upload (MB)", None, true),
            );
            row_y += 48.0 + 8.0;
            let idle_help = "A game closes after nobody has been at the table for the idle timeout (0 = never).";
            frame.text(idle_help, column_x, row_y, help_options);
            row_y += 36.0;
            let options = ButtonOptions::default().icon("save").id("as:save");
            let clicked = frame.button("Save Settings", column_x, row_y, None, Some(44.0), &options);
            row_y += 44.0 + 24.0;
            clicked
        };
        if save {
            self.save_settings();
        }
        row_y += Self::section_label(frame, "SHUTDOWN", column_x, row_y);
        row_y += self.draw_shutdown(frame, column_x, row_y, column_width);
        frame.end_scroll("as:scroll", row_y + offset - y, &ScrollbarStyle::default());
    }

    /// The Shut Down button, or the countdown once a shutdown is running. Returns the height used.
    fn draw_shutdown(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32) -> f32 {
        let palette = frame.palette();
        let shutdown_at = self.server.borrow().shutdown_at;
        let mut text_y = y;
        let help = if let Some(at) = shutdown_at {
            let left = (at - frame.ui.time()).ceil().max(0.0) as i64;
            format!(
                "Shutting down in {}:{:02}. New players can't connect, and the server closes early once everyone else has left.",
                left / 60,
                left % 60
            )
        } else {
            let options = ButtonOptions {
                color: Some(fixed::RED),
                ..ButtonOptions::variant(ButtonVariant::Outlined)
                    .icon("power_settings_new")
                    .id("as:shutdown")
                    .disabled(self.shutting_down)
            };
            if frame.button("Shut Down Server", x, y, None, Some(44.0), &options) {
                self.confirm = Some(AdminConfirm::ShutDown);
            }
            text_y += 44.0 + 8.0;
            "The server can be started again from the machine it runs on.".to_string()
        };
        frame.text(&help, x, text_y, TextOptions::new().width(width).size(12.0).color(palette.on_surface_variant));
        let button_height = if shutdown_at.is_some() { 0.0 } else { 52.0 };
        button_height + frame.text_height(&help, width, 12.0, Weight::Regular) + 24.0
    }

    // -- Games and rooms -----------------------------------------------------------------------

    fn upload_token(&self) -> (String, u16, Option<String>) {
        let server = self.server.borrow();
        (server.host.clone(), server.port, server.token.clone())
    }

    /// `AGM.installFromZip`: validates the zip locally, then uploads it to the server's staging
    /// area and asks the server to install it.
    fn install_game_zip(&mut self, frame: &mut Frame, zip_path: &str) {
        let id = match validate_game_zip(zip_path) {
            Ok(id) => id,
            Err(message) => {
                frame.ui.alert("Couldn't Add Game", &message);
                return;
            }
        };
        if self.server.borrow().game_by_id(&id).is_some() {
            let message = format!("A game with id \"{id}\" already exists. Remove it first to replace it.");
            frame.ui.alert("Couldn't Add Game", &message);
            return;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.as_secs());
        let upload_name = format!("install_{id}_{now}.zip");
        let (host, port, token) = self.upload_token();
        let upload = FileUpload::start(&host, port, token, "", &upload_name, zip_path);
        self.install = Some(GameInstall::Uploading { upload, upload_name });
    }

    fn advance_install(&mut self, frame: &mut Frame) {
        let Some(install) = &mut self.install else { return };
        match install {
            GameInstall::Uploading { upload, upload_name } => match upload.poll() {
                UploadProgress::Uploading { .. } => {}
                UploadProgress::Failed(message) => {
                    self.install = None;
                    frame.ui.alert("Couldn't Add Game", &message);
                }
                UploadProgress::Done => {
                    let payload = JsonObject::new().with("upload", upload_name.as_str());
                    let rid = self.server.borrow_mut().request("adminGameInstall", payload);
                    self.install = Some(GameInstall::Installing { rid });
                }
            },
            GameInstall::Installing { rid } => {
                let Some(response) = self.server.borrow_mut().take_response(*rid) else { return };
                self.install = None;
                if response_ok(&response) {
                    let id = response.get("gameId").and_then(JsonValue::as_str).unwrap_or("");
                    frame.ui.toast(&format!("Added game \"{id}\"."), None);
                } else {
                    frame.ui.alert("Couldn't Add Game", &response_message(&response, "Something went wrong."));
                }
            }
        }
    }

    fn draw_games(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let (games, rooms) = {
            let server = self.server.borrow();
            (server.games.clone(), server.rooms.clone())
        };
        let column_width = 720.0f32.min(width);
        let column_x = x + (width - column_width) / 2.0;
        let offset = frame.begin_scroll("ag:scroll", x, y, width, height);
        let mut row_y = y - offset;
        frame.text("Games on this server", column_x, row_y, TextOptions::new().size(18.0));
        let rescan_label = "Rescan Games Folder";
        let rescan_width = frame.button_width(rescan_label, &ButtonOptions::default().icon("refresh"));
        let installing = self.install.is_some();
        let upload_label = if installing { "Uploading..." } else { "Upload New Game" };
        let upload_width = frame.button_width(upload_label, &ButtonOptions::default().icon("upload_file"));
        let rescan_x = column_x + column_width - rescan_width;
        let upload_x = rescan_x - 12.0 - upload_width;
        let options =
            ButtonOptions::variant(ButtonVariant::Outlined).icon("upload_file").id("ag:upload").disabled(installing);
        if frame.button(upload_label, upload_x, row_y - 6.0, Some(upload_width), Some(40.0), &options) && !installing {
            let picker =
                FilePicker::new(frame, PickerMode::File, Some("Choose a game .zip file"), Some(ZIP_FILTERS), None);
            self.picker = Some((picker, PickerPurpose::GameZip));
        }
        let options =
            ButtonOptions::variant(ButtonVariant::Outlined).icon("refresh").id("ag:rescan").disabled(self.scanning);
        if frame.button(rescan_label, rescan_x, row_y - 6.0, Some(rescan_width), Some(40.0), &options) {
            self.scanning = true;
            let rid = self.request("adminRescanGames", JsonObject::new());
            self.requests.push(AdminRequest::Rescan(rid));
        }
        row_y += 40.0;
        if let Some(GameInstall::Uploading { upload, .. }) = &mut self.install {
            let percent = match upload.poll() {
                UploadProgress::Uploading { bytes, total } if total > 0 => {
                    format!(" ({}%)", (bytes as f64 / total as f64 * 100.0 + 0.5).floor() as i64)
                }
                _ => String::new(),
            };
            let options = TextOptions::new().size(12.0).color(palette.on_surface_variant);
            frame.text(&format!("Uploading game{percent}..."), column_x, row_y, options);
            row_y += 20.0;
        }
        let help = "Add a game by copying its folder (with gamedef.json and its images) into the server's data/games \
                    folder and rescanning, or by uploading a .zip above.";
        frame.text(
            help,
            column_x,
            row_y,
            TextOptions::new().width(column_width).size(12.0).color(palette.on_surface_variant),
        );
        row_y += 32.0;
        let row_height = 40.0;
        for game in &games {
            let row_width = if game.removable { column_width - 48.0 } else { column_width };
            let options = TextOptions::new().medium().width(row_width * 0.6).ellipsis().middle_of(row_height);
            frame.text(&game.name, column_x, row_y, options);
            let details = format!("{} cards  ·  {} sets  ·  {}", game.card_count, game.sets.len(), game.id);
            let options = TextOptions::new()
                .size(12.0)
                .width(row_width * 0.4)
                .right()
                .ellipsis()
                .color(palette.on_surface_variant)
                .middle_of(row_height);
            frame.text(&details, column_x + row_width * 0.6, row_y, options);
            if game.removable {
                let id = format!("ag:removegame:{}", game.id);
                let options = IconButtonOptions::tooltip("Remove Game").id(&id);
                if frame.icon_button("delete_outline", column_x + column_width - 40.0, row_y, &options) {
                    self.confirm =
                        Some(AdminConfirm::RemoveGame { game_id: game.id.clone(), game_name: game.name.clone() });
                }
            }
            row_y += row_height;
        }
        for error in &self.game_errors {
            frame.text(error, column_x, row_y, TextOptions::new().width(column_width).size(12.0).color(fixed::RED));
            row_y += frame.text_height(error, column_width, 12.0, Weight::Regular) + 4.0;
        }
        row_y += 24.0;
        frame.text("Running games", column_x, row_y, TextOptions::new().size(18.0));
        row_y += 36.0;
        if rooms.is_empty() {
            frame.text("None.", column_x, row_y, TextOptions::new().color(palette.on_surface_variant));
            row_y += 24.0;
        }
        for room in &rooms {
            let names: Vec<&str> = room.seats.iter().map(|seat| seat.name.as_str()).collect();
            let heading = format!("{}  ({})", room.name, room.game_name);
            frame.text(&heading, column_x, row_y, TextOptions::new().medium().width(column_width - 120.0).ellipsis());
            let state =
                format!("{}{}", if room.is_playing() { "In progress: " } else { "Waiting: " }, names.join(", "));
            let options =
                TextOptions::new().size(12.0).width(column_width - 120.0).ellipsis().color(palette.on_surface_variant);
            frame.text(&state, column_x, row_y + 20.0, options);
            let id = format!("ag:end:{}", room.id);
            let options =
                ButtonOptions { color: Some(fixed::RED), ..ButtonOptions::variant(ButtonVariant::Text).id(&id) };
            if frame.button("End", column_x + column_width - 80.0, row_y, Some(80.0), Some(40.0), &options) {
                self.confirm = Some(AdminConfirm::EndRoom { room_id: room.id.clone(), room_name: room.name.clone() });
            }
            row_y += 52.0;
        }
        frame.end_scroll("ag:scroll", row_y + offset - y + 24.0, &ScrollbarStyle::default());
    }

    // -- Files ---------------------------------------------------------------------------------

    /// `A:beginUpload`: lists every folder the plan writes into, so one confirmation can cover
    /// every file that would be overwritten.
    fn begin_upload(&mut self, plan: Vec<PlannedUpload>) {
        if plan.is_empty() {
            return;
        }
        let Some(game_id) = self.files_game_id.clone() else { return };
        let mut folders: Vec<String> = Vec::new();
        for item in &plan {
            let folder = remote_dirname(&item.destination).to_string();
            if !folders.contains(&folder) {
                folders.push(folder);
            }
        }
        let listings = folders
            .into_iter()
            .map(|folder| {
                let payload = JsonObject::new().with("gameId", game_id.as_str()).with("path", folder.as_str());
                (self.request("adminFileList", payload), folder)
            })
            .collect();
        self.upload_batch = Some(UploadBatch::Checking { plan, listings, existing: HashSet::new() });
    }

    fn start_upload(&self, plan: &[PlannedUpload], index: usize) -> FileUpload {
        let (host, port, token) = self.upload_token();
        let game_id = self.files_game_id.clone().unwrap_or_default();
        let item = &plan[index];
        FileUpload::start(&host, port, token, &game_id, &item.destination, &item.local_path)
    }

    fn advance_upload_batch(&mut self, frame: &mut Frame) {
        let Some(batch) = self.upload_batch.take() else { return };
        self.upload_batch = match batch {
            UploadBatch::Checking { plan, mut listings, mut existing } => {
                listings.retain(|(rid, folder)| {
                    let Some(response) = self.server.borrow_mut().take_response(*rid) else { return true };
                    if response_ok(&response) {
                        for entry in parse_entries(&response).into_iter().filter(|entry| !entry.is_folder) {
                            existing.insert(join_remote(folder, &entry.name));
                        }
                    }
                    false
                });
                if !listings.is_empty() {
                    Some(UploadBatch::Checking { plan, listings, existing })
                } else {
                    let conflicts: Vec<String> = plan
                        .iter()
                        .filter(|item| existing.contains(&item.destination))
                        .map(|item| item.destination.clone())
                        .collect();
                    if conflicts.is_empty() {
                        let upload = self.start_upload(&plan, 0);
                        Some(UploadBatch::Running { plan, index: 0, upload, errors: Vec::new() })
                    } else {
                        Some(UploadBatch::ConfirmOverwrite { plan, conflicts })
                    }
                }
            }
            UploadBatch::ConfirmOverwrite { plan, conflicts } => {
                let count = conflicts.len();
                let plural = if count == 1 { "" } else { "s" };
                let overwrite_label = format!("Overwrite {count} File{plural}");
                let actions = [DialogAction::text("Cancel"), DialogAction::filled(&overwrite_label)];
                let spec = DialogSpec {
                    title: Some("Overwrite Existing Files?"),
                    width: 460.0,
                    actions: &actions,
                    ..DialogSpec::default()
                };
                let response = frame.dialog("af:overwrite", &spec, |frame, x, y, width| {
                    let hint = format!(
                        "{count} file{plural} already {} on the server and will be overwritten:",
                        if count == 1 { "exists" } else { "exist" }
                    );
                    let color = frame.palette().on_surface_variant;
                    frame.text(&hint, x, y, TextOptions::new().width(width).size(12.0).color(color));
                    let list_top = y + frame.text_height(&hint, width, 12.0, Weight::Regular) + 12.0;
                    let list_height = 240.0f32.min(count as f32 * 28.0);
                    let offset = frame.begin_scroll("af:conflicts", x, list_top, width, list_height);
                    let mut line_y = list_top - offset;
                    for path in &conflicts {
                        frame.text(path, x, line_y, TextOptions::new().width(width).ellipsis().size(12.0));
                        line_y += 28.0;
                    }
                    frame.end_scroll("af:conflicts", count as f32 * 28.0, &ScrollbarStyle::default());
                    list_top - y + list_height
                });
                match response {
                    DialogResponse::Open | DialogResponse::Enter => {
                        Some(UploadBatch::ConfirmOverwrite { plan, conflicts })
                    }
                    DialogResponse::Action(1) => {
                        let upload = self.start_upload(&plan, 0);
                        Some(UploadBatch::Running { plan, index: 0, upload, errors: Vec::new() })
                    }
                    _ => None,
                }
            }
            UploadBatch::Running { plan, index, mut upload, mut errors } => {
                let progress = upload.poll();
                let finished = match &progress {
                    UploadProgress::Uploading { .. } => false,
                    UploadProgress::Done => true,
                    UploadProgress::Failed(message) => {
                        errors.push(format!("{}: {message}", plan[index].destination));
                        true
                    }
                };
                if !finished {
                    let (bytes, total) = match progress {
                        UploadProgress::Uploading { bytes, total } => (bytes, total),
                        _ => (0, 0),
                    };
                    let spec = DialogSpec {
                        title: Some("Uploading Files"),
                        width: 420.0,
                        dismissible: false,
                        ..DialogSpec::default()
                    };
                    let status =
                        format!("Uploading \"{}\" ({} of {})...", plan[index].destination, index + 1, plan.len());
                    frame.dialog("af:uploading", &spec, |frame, x, y, width| {
                        let fraction = (total > 0).then(|| bytes as f32 / total as f32);
                        frame.progress_bar(x, y + 4.0, width, 4.0, fraction, None);
                        let color = frame.palette().on_surface_variant;
                        frame.text(
                            &status,
                            x,
                            y + 20.0,
                            TextOptions::new().width(width).ellipsis().size(12.0).color(color),
                        );
                        20.0 + frame.text_height(&status, width, 12.0, Weight::Regular)
                    });
                    Some(UploadBatch::Running { plan, index, upload, errors })
                } else if index + 1 < plan.len() {
                    let upload = self.start_upload(&plan, index + 1);
                    Some(UploadBatch::Running { plan, index: index + 1, upload, errors })
                } else {
                    if errors.is_empty() {
                        let total = plan.len();
                        frame
                            .ui
                            .toast(&format!("Uploaded {total}{}", if total == 1 { " file." } else { " files." }), None);
                    } else {
                        frame.ui.alert("Some Uploads Failed", &errors.join("\n"));
                    }
                    self.refresh_files();
                    None
                }
            }
        };
    }

    fn handle_picked(&mut self, frame: &mut Frame, purpose: PickerPurpose, paths: Vec<String>) {
        let destination_folder = self.files_path.clone();
        match purpose {
            PickerPurpose::GameZip => {
                if let Some(path) = paths.first() {
                    self.install_game_zip(frame, path);
                }
            }
            PickerPurpose::Files => {
                let plan = paths
                    .into_iter()
                    .map(|local_path| {
                        let destination = join_remote(&destination_folder, &file_name_of(&local_path));
                        PlannedUpload { local_path, destination }
                    })
                    .collect();
                self.begin_upload(plan);
            }
            PickerPurpose::Folder => {
                let Some(folder) = paths.first() else { return };
                let base = join_remote(&destination_folder, &file_name_of(folder));
                let plan: Vec<PlannedUpload> = list_files_recursive(Path::new(folder))
                    .into_iter()
                    .map(|file| PlannedUpload {
                        local_path: file.path.to_string_lossy().into_owned(),
                        destination: format!("{base}/{}", file.relative_path),
                    })
                    .collect();
                if plan.is_empty() {
                    frame.ui.toast("That folder has no files to upload.", None);
                    return;
                }
                self.begin_upload(plan);
            }
        }
    }

    /// The clickable "Game Name / sub / folder" trail.
    fn draw_file_crumbs(&mut self, frame: &mut Frame, x: f32, y: f32, game_name: &str) {
        let palette = frame.palette();
        let mut crumbs = vec![(game_name.to_string(), String::new())];
        let mut built = String::new();
        for segment in self.files_path.split('/').filter(|segment| !segment.is_empty()) {
            built = join_remote(&built, segment);
            crumbs.push((segment.to_string(), built.clone()));
        }
        let mut crumb_x = x;
        let mut navigate = None;
        for (index, (label, path)) in crumbs.iter().enumerate() {
            if index > 0 {
                frame.text(" / ", crumb_x, y, TextOptions::new().color(palette.on_surface_variant));
                crumb_x += frame.text_width(" / ", 14.0, Weight::Regular);
            }
            let label_width = frame.text_width(label, 14.0, Weight::Regular);
            let region =
                frame.ui.region(&format!("af:crumb:{path}"), crumb_x, y, label_width, 24.0, RegionOptions::hand());
            let color = if region.hovered { palette.primary } else { palette.on_surface_variant };
            frame.text(label, crumb_x, y, TextOptions::new().color(color));
            if region.clicked && *path != self.files_path {
                navigate = Some(path.clone());
            }
            crumb_x += label_width;
        }
        if let Some(path) = navigate {
            self.navigate_files(path);
        }
    }

    fn draw_files(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let games = self.server.borrow().games.clone();
        if games.is_empty() {
            frame.text("No games on this server.", x, y, TextOptions::new().color(palette.on_surface_variant));
            return;
        }
        if self.files_game_id.is_none() {
            self.files_game_id = Some(games[0].id.clone());
            self.files_path.clear();
            self.refresh_files();
        }
        let column_width = 720.0f32.min(width);
        let column_x = x + (width - column_width) / 2.0;
        let offset = frame.begin_scroll("af:scroll", x, y, width, height);
        let mut row_y = y - offset;
        let choices: Vec<Choice<String>> =
            games.iter().map(|game| Choice::new(game.id.clone(), game.name.clone())).collect();
        let current = self.files_game_id.clone().unwrap_or_default();
        let game_name = games.iter().find(|game| game.id == current).map_or_else(String::new, |game| game.name.clone());
        let options = DropdownOptions { label: Some("Game"), ..DropdownOptions::default() };
        let picked =
            frame.dropdown("af:game", column_x, row_y, 320.0f32.min(column_width), &choices, current.clone(), &options);
        if picked != current {
            self.files_game_id = Some(picked);
            self.files_path.clear();
            self.refresh_files();
        }
        let busy = self.upload_batch.is_some();
        let (files_label, folder_label) = ("Upload Files", "Upload Folder");
        let files_width = frame.button_width(files_label, &ButtonOptions::default().icon("upload_file"));
        let folder_width = frame.button_width(folder_label, &ButtonOptions::default().icon("upload_file"));
        let folder_x = column_x + column_width - folder_width;
        let files_x = folder_x - 12.0 - files_width;
        let options =
            ButtonOptions::variant(ButtonVariant::Outlined).icon("upload_file").id("af:uploadfiles").disabled(busy);
        if frame.button(files_label, files_x, row_y, Some(files_width), Some(48.0), &options) && !busy {
            let picker = FilePicker::new(frame, PickerMode::MultipleFiles, Some("Choose files to upload"), None, None);
            self.picker = Some((picker, PickerPurpose::Files));
        }
        let options =
            ButtonOptions::variant(ButtonVariant::Outlined).icon("upload_file").id("af:uploadfolder").disabled(busy);
        if frame.button(folder_label, folder_x, row_y, Some(folder_width), Some(48.0), &options) && !busy {
            let picker = FilePicker::new(frame, PickerMode::Folder, Some("Choose a folder to upload"), None, None);
            self.picker = Some((picker, PickerPurpose::Folder));
        }
        row_y += 48.0 + 16.0;
        let options = IconButtonOptions::tooltip("Up one level").id("af:up").disabled(self.files_path.is_empty());
        if frame.icon_button("arrow_upward", column_x, row_y, &options) {
            let parent = remote_dirname(&self.files_path).to_string();
            self.navigate_files(parent);
        }
        if frame.icon_button("refresh", column_x + 44.0, row_y, &IconButtonOptions::tooltip("Refresh").id("af:refresh"))
        {
            self.refresh_files();
        }
        self.draw_file_crumbs(frame, column_x + 96.0, row_y + 10.0, &game_name);
        row_y += 40.0 + 12.0;
        frame.divider(column_x, row_y, column_width, None);
        row_y += 8.0;
        let Some(entries) = self.files_entries.clone() else {
            frame.spinner(column_x + column_width / 2.0, row_y + 40.0, 14.0, None);
            frame.end_scroll("af:scroll", row_y + 80.0 + offset - y, &ScrollbarStyle::default());
            return;
        };
        if entries.is_empty() {
            frame.text("Empty.", column_x, row_y, TextOptions::new().color(palette.on_surface_variant));
            row_y += 24.0;
        }
        let (row_height, delete_width) = (44.0, 48.0);
        let navigate_width = column_width - delete_width;
        let mut navigate = None;
        for (index, entry) in entries.iter().enumerate() {
            let full = join_remote(&self.files_path, &entry.name);
            let region = frame.ui.region(
                &format!("af:row:{}", index + 1),
                column_x,
                row_y,
                navigate_width,
                row_height,
                RegionOptions::hand(),
            );
            if region.hovered {
                frame.fill_rect(column_x, row_y, navigate_width, row_height, 0.0, with_alpha(palette.on_surface, 0.05));
            }
            let icon = if entry.is_folder { "folder" } else { "insert_drive_file" };
            frame.icon(icon, column_x + 8.0, row_y + (row_height - 20.0) / 2.0, 20.0, palette.on_surface_variant);
            let name_width = navigate_width - if entry.is_folder { 48.0 } else { 140.0 };
            let options = TextOptions::new().middle_of(row_height).width(name_width).ellipsis();
            frame.text(&entry.name, column_x + 40.0, row_y, options);
            if !entry.is_folder {
                let options = TextOptions::new()
                    .middle_of(row_height)
                    .width(92.0)
                    .right()
                    .size(12.0)
                    .color(palette.on_surface_variant);
                frame.text(&format_bytes(entry.size), column_x + navigate_width - 100.0, row_y, options);
            }
            if region.clicked && entry.is_folder {
                navigate = Some(full.clone());
            }
            let id = format!("af:del:{}", index + 1);
            let options = IconButtonOptions::tooltip("Delete").id(&id);
            if frame.icon_button(
                "delete_outline",
                column_x + navigate_width + 4.0,
                row_y + (row_height - 40.0) / 2.0,
                &options,
            ) {
                self.confirm =
                    Some(AdminConfirm::DeleteFile { name: entry.name.clone(), path: full, is_folder: entry.is_folder });
            }
            row_y += row_height;
        }
        row_y += 16.0;
        let hint =
            "Only image/json files are downloadable by game clients; other file types will sit in the folder unused.";
        let options = TextOptions::new().width(column_width).size(12.0).color(palette.on_surface_variant);
        frame.text(hint, column_x, row_y, options);
        row_y += frame.text_height(hint, column_width, 12.0, Weight::Regular) + 16.0;
        frame.end_scroll("af:scroll", row_y + offset - y, &ScrollbarStyle::default());
        if let Some(path) = navigate {
            self.navigate_files(path);
        }
    }

    // -- Confirmations -------------------------------------------------------------------------

    fn show_confirm(&mut self, frame: &mut Frame) {
        let Some(confirm) = &self.confirm else { return };
        let (id, title, message, label) = match confirm {
            AdminConfirm::ClearCollection { user_name, game_name, .. } => (
                "ad:clear",
                "Clear Collection".to_string(),
                format!(
                    "Remove every card {user_name} owns in \"{game_name}\"? Decks that use those cards are kept but will \
                     be missing them. This can't be undone."
                ),
                "Clear Collection",
            ),
            AdminConfirm::DeleteAccount { user_name, .. } => (
                "ad:deluser",
                "Delete Account".to_string(),
                format!("Delete \"{user_name}\"? Their decks and collection are deleted too. This can't be undone."),
                "Delete",
            ),
            AdminConfirm::ShutDown => (
                "ad:shutdown",
                "Shut Down Server".to_string(),
                "Everyone online is warned and has 5 minutes before the server shuts down (sooner if they all leave). \
                 Nobody new can connect in the meantime."
                    .to_string(),
                "Shut Down",
            ),
            AdminConfirm::RemoveGame { game_name, .. } => (
                "ad:rmgame",
                "Remove Game".to_string(),
                format!(
                    "Permanently delete \"{game_name}\" and all of its files from the server? This can't be undone."
                ),
                "Remove",
            ),
            AdminConfirm::EndRoom { room_name, .. } => {
                ("ad:endroom", "End Game".to_string(), format!("End \"{room_name}\" for everyone in it?"), "End Game")
            }
            AdminConfirm::DeleteFile { name, path, is_folder } => {
                let warning = if !is_folder && self.files_path.is_empty() && name == "gamedef.json" {
                    "This is the game's definition file -- deleting it will break this game until replaced. "
                } else {
                    ""
                };
                let _ = path;
                (
                    "ad:delfile",
                    format!("Delete {}", if *is_folder { "Folder" } else { "File" }),
                    format!("{warning}Delete \"{name}\"? This can't be undone."),
                    "Delete",
                )
            }
        };
        match frame.confirm_dialog(id, &title, &message, label, true) {
            ConfirmResponse::Open => {}
            ConfirmResponse::Cancelled => self.confirm = None,
            ConfirmResponse::Confirmed => {
                let Some(confirm) = self.confirm.take() else { return };
                match confirm {
                    AdminConfirm::ClearCollection { user_id, user_name, game_id, game_name } => {
                        let payload = JsonObject::new().with("id", user_id).with("gameId", game_id);
                        let rid = self.request("adminClearCollection", payload);
                        self.requests.push(AdminRequest::ClearCollection { rid, user_name, game_name });
                    }
                    AdminConfirm::DeleteAccount { user_id, .. } => {
                        let rid = self.request("adminUserDelete", JsonObject::new().with("id", user_id));
                        self.requests.push(AdminRequest::DeleteUser(rid));
                        self.user_editor = None;
                    }
                    AdminConfirm::ShutDown => {
                        self.shutting_down = true;
                        let rid = self.request("adminShutdown", JsonObject::new());
                        self.requests.push(AdminRequest::Shutdown(rid));
                    }
                    AdminConfirm::RemoveGame { game_id, .. } => {
                        let rid = self.request("adminGameDelete", JsonObject::new().with("gameId", game_id));
                        self.requests.push(AdminRequest::RemoveGame(rid));
                    }
                    AdminConfirm::EndRoom { room_id, .. } => {
                        self.server
                            .borrow_mut()
                            .request_ignored("adminCloseRoom", JsonObject::new().with("roomId", room_id));
                    }
                    AdminConfirm::DeleteFile { path, .. } => {
                        let game_id = self.files_game_id.clone().unwrap_or_default();
                        let rid = self
                            .request("adminFileDelete", JsonObject::new().with("gameId", game_id).with("path", path));
                        self.requests.push(AdminRequest::DeleteFile(rid));
                    }
                }
            }
        }
    }
}

impl Screen for AdminScreen {
    fn title(&self) -> String {
        "Server Administration".to_string()
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        self.finish_requests(frame);
        self.advance_install(frame);
        let padding = 24.0;
        self.tab = frame.tab_bar("admin:tabs", x, y, width, &TABS, self.tab);
        let body_y = y + TAB_BAR_HEIGHT + padding;
        let (body_x, body_width, body_height) = (x + padding, width - 2.0 * padding, y + height - body_y - padding);
        match self.tab {
            0 => self.draw_users(frame, body_x, body_y, body_width, body_height),
            1 => self.draw_server(frame, body_x, body_y, body_width, body_height),
            2 => self.draw_games(frame, body_x, body_y, body_width, body_height),
            _ => self.draw_files(frame, body_x, body_y, body_width, body_height),
        }
        self.show_user_editor(frame);
        self.advance_upload_batch(frame);
        if let Some((picker, _)) = &mut self.picker {
            match picker.show(frame) {
                FilePickerResponse::Open => {}
                FilePickerResponse::Cancelled => self.picker = None,
                FilePickerResponse::Picked(paths) => {
                    if let Some((_, purpose)) = self.picker.take() {
                        self.handle_picked(frame, purpose, paths);
                    }
                }
            }
        }
        self.show_confirm(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths() {
        assert_eq!(remote_dirname("a/b/c.png"), "a/b");
        assert_eq!(remote_dirname("c.png"), "");
        assert_eq!(join_remote("", "x"), "x");
        assert_eq!(join_remote("a", "x"), "a/x");
        assert!(only_digits("12") && !only_digits("") && !only_digits("1a"));
        assert!(signed_digits("-3") && signed_digits("") && !signed_digits("x"));
    }
}
