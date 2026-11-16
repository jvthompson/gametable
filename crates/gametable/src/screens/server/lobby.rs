//! The server lobby: games (rooms) to join or watch, chat, who's online, and the way into the
//! server's Deck Editor, Card Shop, Collection and Administration. Port of
//! `src/ui/screens/server/lobby.lua`.

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gametable_core::json::{JsonObject, JsonValue};
use gametable_core::models::GameDefinition;
use gametable_core::util::trim_lua_whitespace;

use crate::app::{Frame, Screen, ScreenCommand};
use crate::data::deck_store::ServerDeckStore;
use crate::data::game_loader::STANDARD_DECK_ID;
use crate::data::profile::resize_avatar_bytes;
use crate::net::server_client::{
    ServerEvent, ServerStatus, SharedServer, clear_session, last_game_id, password_error, password_hash,
    response_message, response_ok, save_last_game_id,
};
use crate::net::server_models::{ServerRoom, ServerUser, local_time_label, quantities};
use crate::screens::deck_editor::DeckEditor;
use crate::screens::file_picker::{FilePicker, FilePickerResponse, IMAGE_FILTERS, PickerMode};
use crate::screens::server::admin::AdminScreen;
use crate::screens::server::collection::CollectionScreen;
use crate::screens::server::room::RoomScreen;
use crate::screens::server::shop::ShopScreen;
use crate::screens::server::trade::TradeScreen;
use crate::screens::server::{
    GameChoice, GameLoad, GamePickPurpose, LobbyAction, SERVER_LOBBY_MARKER, game_screen, user_row,
};
use crate::ui::components::AvatarSource;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::dropdown::DropdownOptions;
use crate::ui::overlays::{
    AppDialogSpec, ConfirmResponse, ContextMenu, DialogAction, DialogResponse, DialogSpec, MenuItem, MenuResponse,
};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::{Weight, fixed, with_alpha};
use crate::ui::widgets::{ButtonOptions, ButtonVariant, Choice, IconButtonOptions, ScrollbarStyle};

/// `CLOSE_REASONS`: why the game the player was in closed.
fn close_reason_message(reason: &str) -> &'static str {
    match reason {
        "closedByOwner" => "The game was ended by its creator.",
        "idle" => "The game was closed after everyone left.",
        "error" => "The game could not be started.",
        _ => "The game was closed.",
    }
}

/// What happens once a game download finishes.
enum AfterLoad {
    /// The header's selected game is loaded.
    Select,
    CreateRoom {
        name: String,
        players: usize,
    },
    EnterRoom {
        room_id: String,
        spectate: bool,
    },
    DeckEditor,
    Trade,
}

/// A request waiting for its answer, with what it was for.
enum LobbyRequest {
    CreateRoom { rid: u64, game: Box<GameDefinition> },
    EnterRoom { rid: u64, game: Box<GameDefinition>, spectate: bool },
    TradeRequest { rid: u64, user_name: String },
    ProfileUpdate { rid: u64 },
    DeckEditorCollection { rid: u64, game: Box<GameDefinition> },
    CloseRoom { rid: u64 },
    Logout { rid: u64 },
}

impl LobbyRequest {
    fn rid(&self) -> u64 {
        match self {
            LobbyRequest::CreateRoom { rid, .. }
            | LobbyRequest::EnterRoom { rid, .. }
            | LobbyRequest::TradeRequest { rid, .. }
            | LobbyRequest::ProfileUpdate { rid }
            | LobbyRequest::DeckEditorCollection { rid, .. }
            | LobbyRequest::CloseRoom { rid }
            | LobbyRequest::Logout { rid } => *rid,
        }
    }
}

/// The Create Game dialog's fields.
struct CreateGameDialog {
    game_id: String,
    game_name: String,
    name: String,
    players: usize,
}

/// The profile dialog's fields.
struct ProfileDialog {
    name: String,
    color: u32,
    avatar_changed: bool,
    avatar_path: Option<String>,
    current: String,
    password: String,
    confirm: String,
    error: Option<String>,
    avatar_picker: Option<FilePicker>,
}

/// A yes/no question the lobby is asking.
enum LobbyConfirm {
    LeaveServer,
    EndRoom { room_id: String, room_name: String },
    LogOut,
}

pub struct ServerLobby {
    server: SharedServer,
    chat_text: String,
    seen_chat: usize,
    loads: Vec<(GameLoad, AfterLoad)>,
    requests: Vec<LobbyRequest>,
    create_dialog: Option<CreateGameDialog>,
    profile_dialog: Option<ProfileDialog>,
    confirm: Option<LobbyConfirm>,
    user_menu: Option<ContextMenu<ServerUser>>,
    waiting_trade: Option<(String, String)>,
    trade_invite: Option<(String, String)>,
    logging_out: bool,
}

impl ServerLobby {
    pub fn new(frame: &mut Frame, server: SharedServer) -> ServerLobby {
        let mut lobby = ServerLobby {
            server,
            chat_text: String::new(),
            seen_chat: 0,
            loads: Vec::new(),
            requests: Vec::new(),
            create_dialog: None,
            profile_dialog: None,
            confirm: None,
            user_menu: None,
            waiting_trade: None,
            trade_invite: None,
            logging_out: false,
        };
        lobby.load_selected_game(frame);
        lobby
    }

    fn display_name(&self) -> String {
        self.server.borrow().info.display_name().to_string()
    }

    /// `M.selectGame(sc, gameId)`: remembers the game for Shop / Deck Editor / Collection / Create
    /// Game, for this session and across reconnects, and loads it.
    fn select_game(&mut self, frame: &mut Frame, game_id: &str) {
        let key = {
            let mut server = self.server.borrow_mut();
            server.selected_game_id = Some(game_id.to_string());
            server.key.clone()
        };
        save_last_game_id(frame.settings, &key, game_id);
        self.loads.push((GameLoad::start(frame, &self.server, game_id), AfterLoad::Select));
    }

    /// `Lobby:loadSelectedGame()`: restores the last game picked here once the game list arrives.
    fn load_selected_game(&mut self, frame: &mut Frame) {
        let remembered = {
            let server = self.server.borrow();
            if server.selected_game_id.is_some() || server.games.is_empty() {
                return;
            }
            last_game_id(frame.settings, &server.key).filter(|id| server.game_by_id(id).is_some())
        };
        if let Some(game_id) = remembered {
            self.select_game(frame, &game_id);
        }
    }

    /// `M.withSelectedGame`: runs `action` for the selected game, asking first when none is.
    fn with_selected_game(&mut self, frame: &mut Frame, title: &str, action: LobbyAction, exclude_standard_deck: bool) {
        let (selected, has_games) = {
            let server = self.server.borrow();
            let selected = server.selected_game_id.clone().filter(|id| server.game_by_id(id).is_some());
            (selected, !server.games.is_empty())
        };
        if let Some(game_id) = selected {
            self.run_action(frame, action, &game_id);
            return;
        }
        if !has_games {
            frame.ui.alert(
                "No Games",
                "This server doesn't have any games yet. An admin can add game folders to the server's games directory.",
            );
            return;
        }
        let screen = GameChoice::new(
            self.server.clone(),
            title,
            "Pick the game you want to play, shop, and manage decks for.",
            GamePickPurpose::SelectThen(action),
            exclude_standard_deck,
        );
        frame.commands.push(ScreenCommand::Push(Box::new(screen)));
    }

    fn run_action(&mut self, frame: &mut Frame, action: LobbyAction, game_id: &str) {
        let info = self.server.borrow().game_by_id(game_id).cloned();
        let Some(info) = info else { return };
        match action {
            LobbyAction::CreateGame => {
                self.create_dialog =
                    Some(CreateGameDialog { game_id: info.id, game_name: info.name, name: String::new(), players: 2 });
            }
            LobbyAction::Collection => {
                let screen = CollectionScreen::new(frame, self.server.clone(), &info);
                frame.commands.push(ScreenCommand::Push(Box::new(screen)));
            }
            LobbyAction::Shop => {
                let screen = ShopScreen::new(frame, self.server.clone(), info);
                frame.commands.push(ScreenCommand::Push(Box::new(screen)));
            }
            LobbyAction::DeckEditor => {
                self.loads.push((GameLoad::start(frame, &self.server, &info.id), AfterLoad::DeckEditor));
            }
        }
    }

    /// The header's Choose Game: always lets the player pick (or switch) the game.
    fn choose_game(&mut self, frame: &mut Frame) {
        if self.server.borrow().games.is_empty() {
            frame.ui.alert(
                "No Games",
                "This server doesn't have any games yet. An admin can add game folders to the server's games directory.",
            );
            return;
        }
        let screen = GameChoice::new(
            self.server.clone(),
            "Choose a Game",
            "Pick the game you want to play, shop, and manage decks for.",
            GamePickPurpose::Select,
            false,
        );
        frame.commands.push(ScreenCommand::Push(Box::new(screen)));
    }

    /// `M.enterRoom(sc, room, mode)`: joins or watches a room, downloading its game first.
    fn enter_room(&mut self, frame: &mut Frame, room: &ServerRoom, spectate: bool) {
        let load = GameLoad::start(frame, &self.server, &room.game_id);
        self.loads.push((load, AfterLoad::EnterRoom { room_id: room.id.clone(), spectate }));
    }

    /// `Lobby:askToTrade(u)`: pick the game to trade in, then send the request.
    fn ask_to_trade(&mut self, frame: &mut Frame, user: &ServerUser) {
        if self.server.borrow().games.is_empty() {
            frame.ui.alert("No Games", "This server doesn't have any games yet.");
            return;
        }
        let purpose = GamePickPurpose::Trade { user_id: user.id.clone(), user_name: user.name.clone() };
        let screen = GameChoice::new(self.server.clone(), "Ask to Trade", "Pick the game to trade in.", purpose, false);
        frame.commands.push(ScreenCommand::Push(Box::new(screen)));
    }

    fn handle_pick(&mut self, frame: &mut Frame) {
        let picked = self.server.borrow_mut().picked_game.take();
        let Some((purpose, game_id)) = picked else { return };
        match purpose {
            GamePickPurpose::Select => self.select_game(frame, &game_id),
            GamePickPurpose::SelectThen(action) => {
                self.select_game(frame, &game_id);
                self.run_action(frame, action, &game_id);
            }
            GamePickPurpose::Trade { user_id, user_name } => {
                let payload = JsonObject::new().with("targetId", user_id).with("gameId", game_id);
                let rid = self.server.borrow_mut().request("tradeRequest", payload);
                self.requests.push(LobbyRequest::TradeRequest { rid, user_name });
            }
        }
    }

    /// `Lobby:logOut()`: the server drops the session token and Remember me is turned off.
    fn log_out(&mut self, frame: &mut Frame) {
        let (key, token) = {
            let server = self.server.borrow();
            (server.key.clone(), server.token.clone())
        };
        clear_session(frame.settings, &key);
        // The server closes the connection after replying; that's expected here.
        self.logging_out = true;
        let mut payload = JsonObject::new();
        if let Some(token) = token {
            payload.insert("token", token);
        }
        let rid = self.server.borrow_mut().request("logout", payload);
        self.requests.push(LobbyRequest::Logout { rid });
    }

    fn finish_logout(&mut self, frame: &mut Frame) {
        let name = self.server.borrow().info.name.clone().unwrap_or_else(|| "the server".to_string());
        self.server.borrow_mut().disconnect();
        frame.commands.push(ScreenCommand::PopToHome(None));
        frame.ui.toast(&format!("Logged out of {name}."), None);
    }

    // -- Events and answers ------------------------------------------------------------------

    fn handle_events(&mut self, frame: &mut Frame) {
        let events = self.server.borrow_mut().take_events();
        for event in events {
            match event {
                ServerEvent::LobbyState => self.load_selected_game(frame),
                ServerEvent::Rooms => {}
                ServerEvent::Disconnected => {
                    if self.logging_out {
                        self.finish_logout(frame);
                    } else {
                        let message = self.server.borrow().disconnect_message();
                        frame.commands.push(ScreenCommand::PopToHome(Some(message)));
                    }
                }
                ServerEvent::RoomClosed { reason, mine, .. } => {
                    if mine {
                        self.server.borrow_mut().close_channel();
                        let message = Some(close_reason_message(&reason).to_string());
                        frame.commands.push(ScreenCommand::PopTo { marker: SERVER_LOBBY_MARKER, message });
                    }
                }
                ServerEvent::TradeInvite { trade_id, from_name } => {
                    let (accept_server, decline_server) = (self.server.clone(), self.server.clone());
                    let (accept_id, decline_id) = (trade_id.clone(), trade_id.clone());
                    let respond = |server: SharedServer, trade_id: String, accept: bool| {
                        server.borrow_mut().request_ignored(
                            "tradeRespond",
                            JsonObject::new().with("tradeId", trade_id).with("accept", accept),
                        );
                    };
                    let dialog_id = frame.ui.app_dialog(AppDialogSpec {
                        title: "Trade Request".to_string(),
                        message: format!("{from_name} wants to trade with you."),
                        confirm_label: "Accept".to_string(),
                        cancel_label: "Decline".to_string(),
                        dismissible: false,
                        on_confirm: Box::new(move |_| respond(accept_server, accept_id, true)),
                        on_cancel: Some(Box::new(move |_| respond(decline_server, decline_id, false))),
                    });
                    self.trade_invite = Some((trade_id, dialog_id));
                }
                ServerEvent::TradeClosed { trade_id, reason } => {
                    if self.waiting_trade.as_ref().is_some_and(|(waiting, _)| *waiting == trade_id) {
                        self.waiting_trade = None;
                        let message = match reason.as_str() {
                            "declined" => "They declined your trade request.",
                            "expired" => "They didn't respond in time.",
                            _ => "The trade request was cancelled.",
                        };
                        frame.ui.toast(message, None);
                    }
                    if let Some((invite, dialog_id)) = &self.trade_invite
                        && *invite == trade_id
                    {
                        frame.ui.close_app_dialog(dialog_id);
                        self.trade_invite = None;
                    }
                    if self.server.borrow().trade_screen_open {
                        let message = match reason.as_str() {
                            "completed" => "Trade complete.",
                            "left" => "The other player left the trade.",
                            _ => "The trade ended.",
                        };
                        frame.commands.push(ScreenCommand::PopTo {
                            marker: SERVER_LOBBY_MARKER,
                            message: Some(message.to_string()),
                        });
                    }
                }
                ServerEvent::TradeUpdated => {
                    // A trade we're in started (or changed): make sure the Trade screen shows.
                    let (open, game_id) = {
                        let server = self.server.borrow();
                        (server.trade_screen_open, server.trade.as_ref().map(|trade| trade.game_id.clone()))
                    };
                    if open || self.loads.iter().any(|(_, after)| matches!(after, AfterLoad::Trade)) {
                        continue;
                    }
                    self.waiting_trade = None;
                    if let Some(game_id) = game_id {
                        self.loads.push((GameLoad::start(frame, &self.server, &game_id), AfterLoad::Trade));
                    }
                }
            }
        }
    }

    fn finish_loads(&mut self, frame: &mut Frame) {
        let mut index = 0;
        while index < self.loads.len() {
            let result = self.loads[index].0.show_with_dialog(frame, &self.server, index == 0);
            let Some(game) = result else {
                index += 1;
                continue;
            };
            let (_, after) = self.loads.remove(index);
            let Some(game) = game else { continue };
            match after {
                AfterLoad::Select => self.server.borrow_mut().selected_game = Some(game),
                AfterLoad::CreateRoom { name, players } => {
                    let mut server = self.server.borrow_mut();
                    server.close_channel();
                    server.open_channel();
                    let payload = JsonObject::new()
                        .with("gameId", game.id.as_str())
                        .with("name", name)
                        .with("maxPlayers", players as f64);
                    let rid = server.request("roomCreate", payload);
                    self.requests.push(LobbyRequest::CreateRoom { rid, game: Box::new(game) });
                }
                AfterLoad::EnterRoom { room_id, spectate } => {
                    // The channel must exist before the request: a running game sends gameData
                    // and fullState right after the reply.
                    let mut server = self.server.borrow_mut();
                    server.close_channel();
                    server.open_channel();
                    let request = if spectate { "roomSpectate" } else { "roomJoin" };
                    let rid = server.request(request, JsonObject::new().with("roomId", room_id));
                    self.requests.push(LobbyRequest::EnterRoom { rid, game: Box::new(game), spectate });
                }
                AfterLoad::DeckEditor => {
                    if self.server.borrow().is_collection_mode() {
                        let rid = self
                            .server
                            .borrow_mut()
                            .request("collectionGet", JsonObject::new().with("gameId", game.id.as_str()));
                        self.requests.push(LobbyRequest::DeckEditorCollection { rid, game: Box::new(game) });
                    } else {
                        let store = self.deck_store(frame);
                        frame.commands.push(ScreenCommand::Push(Box::new(DeckEditor::new(game, Some(store), None))));
                    }
                }
                AfterLoad::Trade => {
                    if !self.server.borrow().trade_screen_open && self.server.borrow().trade.is_some() {
                        let screen = TradeScreen::new(self.server.clone(), game);
                        frame.commands.push(ScreenCommand::Push(Box::new(screen)));
                    }
                }
            }
        }
    }

    fn deck_store(&self, frame: &Frame) -> Box<ServerDeckStore> {
        let folder = frame.paths.decks_folder(frame.settings).to_string_lossy().into_owned();
        Box::new(ServerDeckStore::new(self.server.clone(), folder))
    }

    /// `M.openRoomScreen(sc, game, room)`: the table for a running game, the room otherwise.
    fn open_room_screen(&mut self, frame: &mut Frame, game: GameDefinition, room: &ServerRoom) {
        let spectating = self.server.borrow().spectating;
        let screen: Box<dyn Screen> = if room.is_playing() {
            game_screen(frame, &self.server, game, spectating)
        } else {
            Box::new(RoomScreen::new(self.server.clone(), game))
        };
        frame.commands.push(ScreenCommand::Push(screen));
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
                LobbyRequest::CreateRoom { game, .. } => {
                    if !ok {
                        self.server.borrow_mut().close_channel();
                        frame.ui.alert("Couldn't Create Game", &response_message(&response, "Something went wrong."));
                        continue;
                    }
                    let room = response.get("room").map(ServerRoom::parse).unwrap_or_default();
                    {
                        let mut server = self.server.borrow_mut();
                        server.room_id = Some(room.id.clone());
                        server.spectating = false;
                        server.room_chat.clear();
                    }
                    self.open_room_screen(frame, *game, &room);
                }
                LobbyRequest::EnterRoom { game, spectate, .. } => {
                    if !ok {
                        self.server.borrow_mut().close_channel();
                        let title = if spectate { "Couldn't Watch Game" } else { "Couldn't Join Game" };
                        frame.ui.alert(title, &response_message(&response, "Something went wrong."));
                        continue;
                    }
                    let room = response.get("room").map(ServerRoom::parse).unwrap_or_default();
                    {
                        let mut server = self.server.borrow_mut();
                        server.room_id = Some(room.id.clone());
                        server.spectating = spectate;
                        server.room_chat = response
                            .get("chat")
                            .and_then(JsonValue::as_array)
                            .map(|lines| lines.iter().map(crate::net::server_models::ServerChatLine::parse).collect())
                            .unwrap_or_default();
                    }
                    self.open_room_screen(frame, *game, &room);
                }
                LobbyRequest::TradeRequest { user_name, .. } => {
                    if !ok {
                        frame.ui.alert(
                            "Couldn't Send Trade Request",
                            &response_message(&response, "Something went wrong."),
                        );
                        continue;
                    }
                    let trade_id = response.get("tradeId").and_then(JsonValue::as_str).unwrap_or("").to_string();
                    self.waiting_trade = Some((trade_id, user_name));
                }
                LobbyRequest::ProfileUpdate { .. } => {
                    if ok {
                        if let Some(user) = response.get("user") {
                            self.server.borrow_mut().me = Some(ServerUser::parse(user));
                        }
                        frame.ui.toast("Profile updated.", None);
                    } else {
                        frame
                            .ui
                            .alert("Couldn't Update Profile", &response_message(&response, "Something went wrong."));
                    }
                }
                LobbyRequest::DeckEditorCollection { game, .. } => {
                    if !ok {
                        frame
                            .ui
                            .alert("Couldn't Load Collection", &response_message(&response, "Something went wrong."));
                        continue;
                    }
                    let owned: HashMap<String, i64> = quantities(response.get("cards"));
                    let store = self.deck_store(frame);
                    frame.commands.push(ScreenCommand::Push(Box::new(DeckEditor::new(
                        *game,
                        Some(store),
                        Some(owned),
                    ))));
                }
                LobbyRequest::CloseRoom { .. } => {
                    if !ok {
                        frame.ui.toast(&response_message(&response, "Couldn't end the game."), None);
                    }
                }
                LobbyRequest::Logout { .. } => self.finish_logout(frame),
            }
        }
    }

    // -- Dialogs -------------------------------------------------------------------------------

    fn show_dialogs(&mut self, frame: &mut Frame) {
        self.show_create_dialog(frame);
        self.show_profile_dialog(frame);
        if let Some((_, user_name)) = self.waiting_trade.clone() {
            let actions = [DialogAction::text("Cancel")];
            let spec = DialogSpec {
                title: Some("Trade Request Sent"),
                width: 380.0,
                dismissible: false,
                actions: &actions,
                ..DialogSpec::default()
            };
            let response = frame.dialog("tradewait", &spec, |frame, x, y, width| {
                frame.spinner(x + width / 2.0, y + 14.0, 14.0, None);
                let message = format!("Waiting for {user_name} to respond...");
                let color = frame.palette().on_surface_variant;
                frame.text(&message, x, y + 44.0, TextOptions::new().width(width).centered().color(color));
                44.0 + frame.text_height(&message, width, 14.0, Weight::Regular)
            });
            if let DialogResponse::Action(0) = response
                && let Some((trade_id, _)) = self.waiting_trade.take()
            {
                self.server.borrow_mut().request_ignored("tradeLeave", JsonObject::new().with("tradeId", trade_id));
            }
        }
        if let Some(confirm) = &self.confirm {
            let name = self.display_name();
            let (id, title, message, label, danger) = match confirm {
                LobbyConfirm::LeaveServer => {
                    ("leaveserver", "Leave Server".to_string(), format!("Disconnect from {name}?"), "Disconnect", false)
                }
                LobbyConfirm::EndRoom { room_name, .. } => (
                    "endroom",
                    "End Game".to_string(),
                    format!("End \"{room_name}\" for everyone in it?"),
                    "End Game",
                    true,
                ),
                LobbyConfirm::LogOut => (
                    "logout",
                    "Log Out".to_string(),
                    format!("Log out of {name}? You'll need to enter your password the next time you sign in."),
                    "Log Out",
                    false,
                ),
            };
            match frame.confirm_dialog(id, &title, &message, label, danger) {
                ConfirmResponse::Open => {}
                ConfirmResponse::Cancelled => self.confirm = None,
                ConfirmResponse::Confirmed => match self.confirm.take() {
                    Some(LobbyConfirm::LeaveServer) => {
                        self.server.borrow_mut().disconnect();
                        frame.commands.push(ScreenCommand::PopToHome(None));
                    }
                    Some(LobbyConfirm::EndRoom { room_id, .. }) => {
                        let rid =
                            self.server.borrow_mut().request("roomClose", JsonObject::new().with("roomId", room_id));
                        self.requests.push(LobbyRequest::CloseRoom { rid });
                    }
                    Some(LobbyConfirm::LogOut) => self.log_out(frame),
                    None => {}
                },
            }
        }
        if let Some(menu) = &mut self.user_menu {
            match menu.show(frame) {
                MenuResponse::Open => {}
                MenuResponse::Closed => self.user_menu = None,
                MenuResponse::Selected(user) => {
                    self.user_menu = None;
                    self.ask_to_trade(frame, &user);
                }
            }
        }
    }

    /// Create Game, step 2: name and player count for the chosen game.
    fn show_create_dialog(&mut self, frame: &mut Frame) {
        let Some(dialog) = &mut self.create_dialog else { return };
        let (my_name, collection_mode) = {
            let server = self.server.borrow();
            (server.me.as_ref().map_or_else(String::new, |me| me.name.clone()), server.is_collection_mode())
        };
        let title = format!("Create Game -- {}", dialog.game_name);
        let actions = [DialogAction::text("Cancel"), DialogAction::filled("Create")];
        let spec = DialogSpec { title: Some(&title), width: 440.0, actions: &actions, ..DialogSpec::default() };
        let label_background = frame.palette().surface_container_high;
        let response = frame.dialog("creategame", &spec, |frame, x, y, width| {
            let mut row_y = y + 8.0;
            let hint = format!("{my_name}'s game");
            let options = TextFieldOptions {
                hint: Some(&hint),
                label_background: Some(label_background),
                max_length: Some(40),
                ..TextFieldOptions::label("Name (optional)")
            };
            frame.text_field("cg:name", x, row_y, width, &mut dialog.name, &options);
            row_y += 48.0 + 16.0;
            let choices: Vec<Choice<usize>> = (1..=8)
                .map(|count| {
                    Choice::new(count, if count == 1 { "1 player".to_string() } else { format!("{count} players") })
                })
                .collect();
            let options = DropdownOptions { label: Some("Players"), ..DropdownOptions::default() };
            dialog.players = frame.dropdown("cg:players", x, row_y, width, &choices, dialog.players, &options);
            row_y += 48.0 + 12.0;
            let note = if collection_mode {
                "Collection mode: players load decks built from cards they own."
            } else {
                "Players can join until the game starts. After that, others can watch."
            };
            let color = frame.palette().on_surface_variant;
            frame.text(note, x, row_y, TextOptions::new().width(width).size(12.0).color(color));
            row_y + frame.text_height(note, width, 12.0, Weight::Regular) - y
        });
        match response {
            DialogResponse::Open | DialogResponse::Enter => {}
            DialogResponse::Action(1) => {
                let Some(dialog) = self.create_dialog.take() else { return };
                let load = GameLoad::start(frame, &self.server, &dialog.game_id);
                self.loads.push((load, AfterLoad::CreateRoom { name: dialog.name, players: dialog.players }));
            }
            _ => self.create_dialog = None,
        }
    }

    fn open_profile(&mut self) {
        let Some(me) = self.server.borrow().me.clone() else { return };
        self.profile_dialog = Some(ProfileDialog {
            name: me.name,
            color: me.color,
            avatar_changed: false,
            avatar_path: None,
            current: String::new(),
            password: String::new(),
            confirm: String::new(),
            error: None,
            avatar_picker: None,
        });
    }

    /// The Save button: builds the update, or reports why it can't.
    fn save_profile(&mut self) -> bool {
        let Some(me) = self.server.borrow().me.clone() else { return true };
        let Some(dialog) = &mut self.profile_dialog else { return true };
        let mut payload = JsonObject::new();
        let name = trim_lua_whitespace(&dialog.name).to_string();
        if name != me.name {
            payload.insert("name", name);
        }
        if dialog.color != me.color {
            payload.insert("color", f64::from(dialog.color));
        }
        if dialog.avatar_changed {
            let mut avatar = String::new();
            if let Some(path) = &dialog.avatar_path {
                let png = std::fs::read(path).ok().and_then(|bytes| resize_avatar_bytes(&bytes).ok());
                let Some(png) = png else {
                    dialog.error = Some("Could not use that image.".to_string());
                    return false;
                };
                avatar = BASE64.encode(png);
            }
            payload.insert("avatar", avatar);
        }
        if !dialog.password.is_empty() || !dialog.current.is_empty() {
            if let Some(error) = password_error(&dialog.password, Some("new password")) {
                dialog.error = Some(error);
                return false;
            }
            if dialog.password != dialog.confirm {
                dialog.error = Some("The new passwords don't match.".to_string());
                return false;
            }
            payload.insert("oldPasswordHash", password_hash(&me.email, &dialog.current));
            payload.insert("passwordHash", password_hash(&me.email, &dialog.password));
        }
        dialog.error = None;
        let rid = self.server.borrow_mut().request("profileUpdate", payload);
        self.requests.push(LobbyRequest::ProfileUpdate { rid });
        true
    }

    fn show_profile_dialog(&mut self, frame: &mut Frame) {
        if self.profile_dialog.is_none() {
            return;
        }
        let (title, me) = {
            let server = self.server.borrow();
            (format!("Your Profile on {}", server.info.name.as_deref().unwrap_or("this server")), server.me.clone())
        };
        let Some(me) = me else { return };
        let current_key = self.server.borrow_mut().avatar_key(&me.avatar_hash);
        let Some(dialog) = &mut self.profile_dialog else { return };
        let actions = [DialogAction::text("Log Out"), DialogAction::text("Cancel"), DialogAction::filled("Save")];
        let spec = DialogSpec { title: Some(&title), width: 440.0, actions: &actions, ..DialogSpec::default() };
        let label_background = frame.palette().surface_container_high;
        let mut pick_avatar = false;
        let response = frame.dialog("profile", &spec, |frame, x, y, width| {
            let mut row_y = y;
            let size = 120.0;
            let avatar_x = x + (width - size) / 2.0;
            let region = frame.ui.region(
                "sp:avatar",
                avatar_x,
                row_y,
                size,
                size,
                RegionOptions::hand().with_tooltip(Some("Choose an image")),
            );
            let source = if dialog.avatar_changed {
                dialog.avatar_path.as_deref().map(AvatarSource::Path)
            } else {
                current_key.as_deref().map(AvatarSource::Key)
            };
            let has_source = source.is_some();
            frame.avatar(avatar_x, row_y, size, dialog.color, source, true);
            if region.hovered {
                frame.fill_rect(
                    avatar_x,
                    row_y,
                    size,
                    size,
                    0.0,
                    crate::graphics::draw_list::Color::rgba(1.0, 1.0, 1.0, 0.08),
                );
            }
            pick_avatar |= region.clicked;
            row_y += size + 4.0;
            if has_source {
                let options = ButtonOptions::variant(ButtonVariant::Text).icon("delete_outline").id("sp:rm");
                let button_width = frame.button_width("Remove Image", &options);
                if frame.button(
                    "Remove Image",
                    x + (width - button_width) / 2.0,
                    row_y,
                    Some(button_width),
                    None,
                    &options,
                ) {
                    dialog.avatar_path = None;
                    dialog.avatar_changed = true;
                }
                row_y += 44.0;
            }
            row_y += 12.0;
            let options = TextFieldOptions {
                label_background: Some(label_background),
                max_length: Some(24),
                ..TextFieldOptions::label("Player name")
            };
            frame.text_field("sp:name", x, row_y, width, &mut dialog.name, &options);
            row_y += 48.0 + 16.0;
            let (color, color_height) = frame.color_field(
                "sp:color",
                x,
                row_y,
                width,
                dialog.color,
                Some("Player Color"),
                Some(label_background),
            );
            dialog.color = color;
            row_y += color_height + 20.0;
            frame.text("Change password (optional)", x, row_y, TextOptions::new().medium());
            row_y += 28.0;
            for (id, label, text) in [
                ("sp:cur", "Current password", &mut dialog.current),
                ("sp:new", "New password", &mut dialog.password),
                ("sp:conf", "Confirm new password", &mut dialog.confirm),
            ] {
                let options = TextFieldOptions {
                    password: true,
                    label_background: Some(label_background),
                    ..TextFieldOptions::label(label)
                };
                frame.text_field(id, x, row_y, width, text, &options);
                row_y += 48.0 + 12.0;
            }
            row_y -= 12.0;
            if let Some(error) = &dialog.error {
                row_y += 12.0;
                frame.text(error, x, row_y, TextOptions::new().width(width).color(fixed::RED));
                row_y += frame.text_height(error, width, 14.0, Weight::Regular);
            }
            row_y - y
        });
        if pick_avatar {
            dialog.avatar_picker = Some(FilePicker::new(
                frame,
                PickerMode::File,
                Some("Choose an avatar image"),
                Some(IMAGE_FILTERS),
                None,
            ));
        }
        if let Some(picker) = &mut dialog.avatar_picker {
            match picker.show(frame) {
                FilePickerResponse::Open => {}
                FilePickerResponse::Cancelled => dialog.avatar_picker = None,
                FilePickerResponse::Picked(mut paths) => {
                    dialog.avatar_picker = None;
                    dialog.avatar_path = Some(paths.remove(0));
                    dialog.avatar_changed = true;
                }
            }
        }
        match response {
            DialogResponse::Open | DialogResponse::Enter => {}
            DialogResponse::Action(0) => {
                self.profile_dialog = None;
                self.confirm = Some(LobbyConfirm::LogOut);
            }
            DialogResponse::Action(2) => {
                if self.save_profile() {
                    self.profile_dialog = None;
                }
            }
            _ => self.profile_dialog = None,
        }
    }

    // -- Drawing -------------------------------------------------------------------------------

    /// One room's card: name, game, seats and its buttons. Returns its height.
    fn draw_room(&mut self, frame: &mut Frame, room: &ServerRoom, x: f32, y: f32, width: f32) -> f32 {
        let palette = frame.palette();
        let height = 88.0;
        frame.fill_rect(x, y, width, height, 12.0, palette.surface_container);
        let playing = room.is_playing();
        let (my_id, is_admin, users) = {
            let server = self.server.borrow();
            (server.my_id().to_string(), server.is_admin(), server.users.clone())
        };
        let seated = room.seats.iter().any(|seat| seat.id == my_id);
        // Buttons on the right.
        let mut button_x = x + width - 16.0;
        let mut button = |frame: &mut Frame, label: &str, variant: ButtonVariant, id: &str| {
            let options = ButtonOptions::variant(variant);
            let button_width = frame.button_width(label, &options);
            button_x -= button_width;
            let id = format!("{id}{}", room.id);
            let options = ButtonOptions { id: Some(&id), ..options };
            let clicked =
                frame.button(label, button_x, y + (height - 40.0) / 2.0, Some(button_width), Some(40.0), &options);
            button_x -= 8.0;
            clicked
        };
        if (room.owner_id == my_id || is_admin) && button(frame, "End", ButtonVariant::Text, "lb:end:") {
            self.confirm = Some(LobbyConfirm::EndRoom { room_id: room.id.clone(), room_name: room.name.clone() });
        }
        // Seated players of a running game rejoin rather than watch.
        if !(playing && seated) && button(frame, "Watch", ButtonVariant::Outlined, "lb:watch:") {
            self.enter_room(frame, room, true);
        }
        if playing {
            if seated && button(frame, "Rejoin", ButtonVariant::Filled, "lb:rejoin:") {
                self.enter_room(frame, room, false);
            }
        } else if (room.seats.len() < room.max_players || seated)
            && button(frame, "Join", ButtonVariant::Filled, "lb:join:")
        {
            self.enter_room(frame, room, false);
        }
        // Text on the left.
        let (text_x, text_width) = (x + 20.0, button_x - x - 28.0);
        let (chip, chip_color) =
            if playing { ("In progress", fixed::GREEN) } else { ("Waiting for players", fixed::BLUE) };
        frame.text(&room.name, text_x, y + 14.0, TextOptions::new().bold().width(text_width).ellipsis());
        let mut subtitle = format!("{}  ·  {}/{} players", room.game_name, room.seats.len(), room.max_players);
        if room.spectators > 0 {
            subtitle.push_str(&format!("  ·  {} watching", room.spectators));
        }
        let options = TextOptions::new().size(12.0).width(text_width).ellipsis().color(palette.on_surface_variant);
        frame.text(&subtitle, text_x, y + 36.0, options);
        let chip_width = frame.text_width(chip, 11.0, Weight::Medium) + 16.0;
        frame.fill_rect(text_x, y + 58.0, chip_width, 20.0, 10.0, with_alpha(chip_color, 0.15));
        let options =
            TextOptions::new().size(11.0).medium().width(chip_width).middle_of(20.0).centered().color(chip_color);
        frame.text(chip, text_x, y + 58.0, options);
        let mut avatar_x = text_x + chip_width + 12.0;
        for seat in &room.seats {
            if avatar_x + 20.0 > text_x + text_width {
                break;
            }
            let hash =
                users.iter().find(|user| user.id == seat.id).map(|user| user.avatar_hash.clone()).unwrap_or_default();
            let key = self.server.borrow_mut().avatar_key(&hash);
            frame.avatar(avatar_x, y + 58.0, 20.0, seat.color, key.as_deref().map(AvatarSource::Key), false);
            let away = playing && !seat.connected;
            if away {
                frame.fill_rect(
                    avatar_x,
                    y + 58.0,
                    20.0,
                    20.0,
                    0.0,
                    crate::graphics::draw_list::Color::rgba(0.0, 0.0, 0.0, 0.45),
                );
            }
            let tooltip = format!("{}{}", seat.name, if away { " (away)" } else { "" });
            frame.ui.region(
                &format!("lb:seat:{}{}", room.id, seat.id),
                avatar_x,
                y + 58.0,
                20.0,
                20.0,
                RegionOptions { cursor: None, tooltip: Some(&tooltip) },
            );
            avatar_x += 24.0;
        }
        height
    }

    fn draw_rooms(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let rooms = self.server.borrow().rooms.clone();
        frame.text("Games", x, y + 8.0, TextOptions::new().size(20.0));
        let count_x = x + frame.text_width("Games", 20.0, Weight::Regular) + 10.0;
        frame.text(
            &rooms.len().to_string(),
            count_x,
            y + 13.0,
            TextOptions::new().size(13.0).color(palette.on_surface_variant),
        );
        let options = ButtonOptions { icon: Some("add"), id: Some("lb:create"), ..ButtonOptions::default() };
        let button_width = frame.button_width("Create Game", &options);
        if frame.button("Create Game", x + width - button_width, y, Some(button_width), Some(40.0), &options) {
            self.with_selected_game(frame, "Create Game", LobbyAction::CreateGame, false);
        }
        let list_y = y + 56.0;
        let list_height = y + height - list_y;
        if rooms.is_empty() {
            let options = TextOptions::new()
                .width(width)
                .middle_of(list_height.min(160.0))
                .centered()
                .color(palette.on_surface_variant);
            frame.text("No games right now. Create one and others can join you.", x, list_y, options);
            return;
        }
        let offset = frame.begin_scroll("lb:rooms", x, list_y, width, list_height);
        let mut row_y = list_y - offset;
        for room in &rooms {
            row_y += self.draw_room(frame, room, x, row_y, width - 4.0) + 10.0;
        }
        frame.end_scroll("lb:rooms", row_y + offset - list_y, &ScrollbarStyle::default());
    }

    fn draw_chat(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let lines = self.server.borrow().chat.clone();
        frame.fill_rect(x, y, width, height, 12.0, palette.surface_container);
        frame.text("Chat", x + 16.0, y + 12.0, TextOptions::new().bold());
        let input_height = 48.0;
        let (list_y, list_height) = (y + 40.0, height - 40.0 - input_height - 24.0);
        let offset = frame.begin_scroll("lb:chat", x + 8.0, list_y, width - 16.0, list_height);
        let mut line_y = list_y - offset;
        let text_width = width - 32.0;
        for line in &lines {
            let head = format!("{}  {}", local_time_label(line.at), line.name);
            let options =
                TextOptions::new().size(12.0).bold().color(palette.on_surface_variant).width(text_width).ellipsis();
            frame.text(&head, x + 16.0, line_y, options);
            line_y += 18.0;
            frame.text(&line.text, x + 16.0, line_y, TextOptions::new().width(text_width));
            line_y += frame.text_height(&line.text, text_width, 14.0, Weight::Regular) + 10.0;
        }
        frame.end_scroll("lb:chat", line_y + offset - list_y, &ScrollbarStyle::default());
        if lines.len() != self.seen_chat {
            self.seen_chat = lines.len();
            frame.ui.scroll_to_end("lb:chat");
        }
        if lines.is_empty() {
            let options =
                TextOptions::new().width(width).middle_of(list_height).centered().color(palette.on_surface_variant);
            frame.text("Say hello!", x, list_y, options);
        }
        let options = TextFieldOptions {
            label_background: Some(palette.surface_container),
            max_length: Some(500),
            ..TextFieldOptions::label("Message")
        };
        let field = frame.text_field(
            "lb:chatinput",
            x + 12.0,
            y + height - input_height - 12.0,
            width - 24.0 - 44.0,
            &mut self.chat_text,
            &options,
        );
        let blank = trim_lua_whitespace(&self.chat_text).is_empty();
        let send_options = IconButtonOptions::tooltip("Send").id("lb:send").disabled(blank);
        let send =
            frame.icon_button("send", x + width - 12.0 - 40.0, y + height - input_height - 12.0 + 4.0, &send_options);
        if (field.submitted || send) && !blank {
            let text = std::mem::take(&mut self.chat_text);
            self.server.borrow_mut().send("chatSend", JsonObject::new().with("text", text).into());
            frame.ui.focus_field("lb:chatinput", false);
        }
    }

    fn draw_users(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let (users, my_id, collection_mode) = {
            let server = self.server.borrow();
            (server.users.clone(), server.my_id().to_string(), server.is_collection_mode())
        };
        frame.text("Online", x, y + 12.0, TextOptions::new().bold());
        let count_x = x + frame.text_width("Online", 14.0, Weight::Bold) + 8.0;
        frame.text(&users.len().to_string(), count_x, y + 12.0, TextOptions::new().color(palette.on_surface_variant));
        let list_y = y + 44.0;
        let offset = frame.begin_scroll("lb:users", x, list_y, width, y + height - list_y);
        let mut row_y = list_y - offset;
        for user in &users {
            let region =
                frame.ui.region(&format!("lb:user:{}", user.id), x, row_y, width - 8.0, 32.0, RegionOptions::hand());
            if region.right_clicked && user.id != my_id && collection_mode && user.room_id.is_empty() {
                let (mouse_x, mouse_y) = frame.ui.mouse();
                let items = vec![MenuItem::new("Ask to Trade...", user.clone()).icon("swap_horiz")];
                self.user_menu = Some(ContextMenu::new("lb:usermenu", mouse_x, mouse_y, items));
            }
            user_row(frame, &self.server, user, x, row_y, width - 8.0, 32.0, 32.0, user.id == my_id);
            if !user.room_id.is_empty() {
                let room_name = self.server.borrow().room_by_id(&user.room_id).map(|room| room.name.clone());
                if let Some(room_name) = room_name {
                    let status = format!("{}{room_name}", if user.spectating { "Watching " } else { "In " });
                    let options =
                        TextOptions::new().size(11.0).width(width - 50.0).ellipsis().color(palette.on_surface_variant);
                    frame.text(&status, x + 42.0, row_y + 32.0, options);
                    row_y += 14.0;
                }
            }
            if user.is_admin {
                frame.ui.region(
                    &format!("lb:adm:{}", user.id),
                    x + width - 26.0,
                    row_y + 7.0,
                    18.0,
                    18.0,
                    RegionOptions { cursor: None, tooltip: Some("Server admin") },
                );
                frame.icon("shield", x + width - 26.0, row_y + 7.0, 18.0, palette.primary);
            }
            row_y += 44.0;
        }
        frame.end_scroll("lb:users", row_y + offset - list_y, &ScrollbarStyle::default());
    }

    fn draw_header(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32) {
        let palette = frame.palette();
        let Some(me) = self.server.borrow().me.clone() else { return };
        let (selected_info, back_image, collection_mode, is_admin, selected_id, info) = {
            let server = self.server.borrow();
            let selected_info = server.selected_game_id.as_ref().and_then(|id| server.game_by_id(id)).cloned();
            let back_image = server
                .selected_game
                .as_ref()
                .and_then(|game| game.card_backs.first())
                .and_then(|back| back.image_path.clone());
            (
                selected_info,
                back_image,
                server.is_collection_mode(),
                server.is_admin(),
                server.selected_game_id.clone(),
                server.info.clone(),
            )
        };
        // Right side: actions, then the profile button.
        let key = self.server.borrow_mut().avatar_key(&me.avatar_hash);
        let mut draw_avatar = |frame: &mut Frame, icon_x: f32, icon_y: f32| {
            frame.avatar(icon_x, icon_y, 32.0, me.color, key.as_deref().map(AvatarSource::Key), false);
        };
        let (mut right, clicked) =
            frame.header_pill("lb:profile", x + width, y, &me.name, Some("Edit your profile"), Some(&mut draw_avatar));
        if clicked {
            self.open_profile();
        }
        // The selected game: its name and, once loaded, its default card back.
        let game_label = selected_info.as_ref().map_or_else(|| "Choose Game".to_string(), |game| game.name.clone());
        right -= 8.0;
        let mut draw_back = |frame: &mut Frame, icon_x: f32, icon_y: f32| {
            frame.avatar(icon_x, icon_y, 32.0, 0xFF00_0000, back_image.as_deref().map(AvatarSource::Path), false);
        };
        #[allow(clippy::type_complexity)]
        let thumb: Option<&mut dyn FnMut(&mut Frame, f32, f32)> =
            if back_image.is_some() { Some(&mut draw_back) } else { None };
        let (pill_left, clicked) =
            frame.header_pill("lb:game", right, y, &game_label, Some("Choose which game to play"), thumb);
        right = pill_left;
        if clicked {
            self.choose_game(frame);
        }
        // The Standard 52-Card Deck can't be shopped for, edited or collected.
        let is_standard_deck = selected_id.as_deref() == Some(STANDARD_DECK_ID);
        let mut action = |frame: &mut Frame, label: &str, icon: &'static str, id: &str| {
            let options = ButtonOptions::variant(ButtonVariant::Text).icon(icon);
            let button_width = frame.button_width(label, &options);
            right -= button_width + 8.0;
            let options = ButtonOptions { id: Some(id), ..options };
            frame.button(label, right, y + 4.0, Some(button_width), Some(40.0), &options)
        };
        if is_admin && action(frame, "Admin", "shield", "lb:admin") {
            frame.commands.push(ScreenCommand::Push(Box::new(AdminScreen::new(self.server.clone()))));
        }
        if !is_standard_deck && collection_mode && action(frame, "Collection", "layers", "lb:coll") {
            self.with_selected_game(frame, "Collection", LobbyAction::Collection, true);
        }
        if !is_standard_deck && action(frame, "Deck Editor", "build", "lb:decks") {
            self.with_selected_game(frame, "Deck Editor", LobbyAction::DeckEditor, true);
        }
        if !is_standard_deck && collection_mode && action(frame, "Card Shop", "card_giftcard", "lb:shop") {
            self.with_selected_game(frame, "Card Shop", LobbyAction::Shop, true);
        }
        if collection_mode {
            let tokens = format!("{} tokens", me.tokens);
            let chip_width = frame.text_width(&tokens, 13.0, Weight::Medium) + 40.0;
            right -= chip_width + 12.0;
            frame.fill_rect(right, y + 10.0, chip_width, 28.0, 14.0, palette.secondary_container);
            frame.icon("toll", right + 10.0, y + 15.0, 18.0, palette.on_secondary_container);
            let options = TextOptions::new().size(13.0).medium().middle_of(28.0).color(palette.on_secondary_container);
            frame.text(&tokens, right + 32.0, y + 10.0, options);
        }
        // Left: the server's name and message of the day.
        let left_width = right - x - 16.0;
        frame.text(info.display_name(), x, y + 2.0, TextOptions::new().size(22.0).width(left_width).ellipsis());
        let subtitle = if !info.motd.is_empty() {
            info.motd.clone()
        } else if collection_mode {
            "Collection mode".to_string()
        } else {
            "Free play".to_string()
        };
        let options = TextOptions::new().size(12.0).width(left_width).ellipsis().color(palette.on_surface_variant);
        frame.text(&subtitle, x, y + 32.0, options);
    }
}

impl Screen for ServerLobby {
    fn title(&self) -> String {
        self.display_name()
    }

    fn marker(&self) -> Option<&'static str> {
        Some(SERVER_LOBBY_MARKER)
    }

    fn on_back(&mut self, _frame: &mut Frame) {
        self.confirm = Some(LobbyConfirm::LeaveServer);
    }

    fn quit(&mut self, _frame: &mut Frame) -> bool {
        self.server.borrow_mut().disconnect();
        false
    }

    fn background_update(&mut self, frame: &mut Frame, _dt: f32) {
        if self.server.borrow().status == ServerStatus::Connected {
            self.server.borrow_mut().update(frame);
        }
        self.handle_events(frame);
        self.handle_pick(frame);
        self.finish_requests(frame);
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        if self.server.borrow().me.is_none() {
            frame.spinner(x + width / 2.0, y + height / 2.0, 18.0, None);
            return;
        }
        let padding = 24.0;
        self.draw_header(frame, x + padding, y + 16.0, width - 2.0 * padding);
        let body_y = y + 16.0 + 48.0 + 20.0;
        let body_height = y + height - padding - body_y;
        let users_width = if width >= 1180.0 { 220.0 } else { 0.0 };
        let chat_width = 340.0;
        let rooms_width =
            width - 2.0 * padding - chat_width - 24.0 - if users_width > 0.0 { users_width + 24.0 } else { 0.0 };
        frame.ui.push_clip(x + padding, body_y, rooms_width, body_height);
        self.draw_rooms(frame, x + padding, body_y, rooms_width, body_height);
        frame.ui.pop_clip();
        let chat_x = x + padding + rooms_width + 24.0;
        self.draw_chat(frame, chat_x, body_y, chat_width, body_height);
        if users_width > 0.0 {
            frame.ui.push_clip(chat_x + chat_width + 24.0, body_y, users_width, body_height);
            self.draw_users(frame, chat_x + chat_width + 24.0, body_y, users_width, body_height);
            frame.ui.pop_clip();
        }
        self.finish_loads(frame);
        self.show_dialogs(frame);
    }
}
