//! A server game before it starts: the seats (the creator can reorder them and start the game)
//! and the room's chat. When the game starts the server sends gameData, and this screen hands
//! over to the table. Port of `src/ui/screens/server/room.lua`.

use gametable_core::json::{JsonObject, JsonValue};
use gametable_core::models::GameDefinition;
use gametable_core::util::trim_lua_whitespace;

use crate::app::{Frame, Screen, ScreenCommand};
use crate::net::server_client::{SharedServer, response_message, response_ok};
use crate::net::server_models::{ServerRoom, ServerUser, local_time_label};
use crate::screens::server::{game_screen, user_row};
use crate::ui::draw::TextOptions;
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::{Weight, with_alpha};
use crate::ui::widgets::{ButtonOptions, IconButtonOptions, ScrollbarStyle};

/// A request waiting for its answer.
enum RoomRequest {
    Seats(u64),
    Start(u64),
}

pub struct RoomScreen {
    server: SharedServer,
    game: Option<GameDefinition>,
    title: String,
    chat_text: String,
    seen_chat: usize,
    starting: bool,
    started: bool,
    requests: Vec<RoomRequest>,
}

impl RoomScreen {
    pub fn new(server: SharedServer, game: GameDefinition) -> RoomScreen {
        let title = server.borrow().current_room().map_or_else(|| "Game".to_string(), |room| room.name.clone());
        RoomScreen {
            server,
            game: Some(game),
            title,
            chat_text: String::new(),
            seen_chat: 0,
            starting: false,
            started: false,
            requests: Vec::new(),
        }
    }

    /// `R:moveSeat(room, i, delta)`: swaps a seat with its neighbour.
    fn move_seat(&mut self, room: &ServerRoom, index: usize, delta: isize) {
        let mut order: Vec<String> = room.seats.iter().map(|seat| seat.id.clone()).collect();
        let target = index as isize + delta;
        if target < 0 || target as usize >= order.len() {
            return;
        }
        order.swap(index, target as usize);
        let order = JsonValue::Array(order.into_iter().map(JsonValue::from).collect());
        let rid = self.server.borrow_mut().request("roomSeats", JsonObject::new().with("order", order));
        self.requests.push(RoomRequest::Seats(rid));
    }

    fn start(&mut self) {
        self.starting = true;
        let rid = self.server.borrow_mut().request("roomStart", JsonObject::new());
        self.requests.push(RoomRequest::Start(rid));
    }

    fn finish_requests(&mut self, frame: &mut Frame) {
        let mut index = 0;
        while index < self.requests.len() {
            let rid = match self.requests[index] {
                RoomRequest::Seats(rid) | RoomRequest::Start(rid) => rid,
            };
            let Some(response) = self.server.borrow_mut().take_response(rid) else {
                index += 1;
                continue;
            };
            let request = self.requests.remove(index);
            if response_ok(&response) {
                continue;
            }
            match request {
                RoomRequest::Seats(_) => frame.ui.toast(&response_message(&response, "Couldn't change seats."), None),
                RoomRequest::Start(_) => {
                    self.starting = false;
                    frame.ui.alert("Couldn't Start", &response_message(&response, "Something went wrong."));
                }
            }
        }
    }

    fn draw_seats(&mut self, frame: &mut Frame, room: &ServerRoom, x: f32, y: f32, width: f32) {
        let palette = frame.palette();
        let (my_id, spectating, users) = {
            let server = self.server.borrow();
            (server.my_id().to_string(), server.spectating, server.users.clone())
        };
        let is_owner = room.owner_id == my_id && !spectating;
        frame.text(&room.game_name, x, y, TextOptions::new().size(22.0).width(width).ellipsis());
        let mut subtitle = format!("{} of {} players", room.seats.len(), room.max_players);
        if room.spectators > 0 {
            subtitle.push_str(&format!("  ·  {} watching", room.spectators));
        }
        if room.collection_mode {
            subtitle.push_str("  ·  Collection mode");
        }
        frame.text(&subtitle, x, y + 32.0, TextOptions::new().size(13.0).color(palette.on_surface_variant));
        let mut row_y = y + 68.0;
        let row_height = 56.0;
        for index in 0..room.max_players {
            frame.fill_rect(x, row_y, width, row_height - 8.0, 10.0, palette.surface_container);
            let options =
                TextOptions::new().width(20.0).middle_of(row_height - 8.0).centered().color(palette.on_surface_variant);
            frame.text(&(index + 1).to_string(), x + 12.0, row_y, options);
            if let Some(seat) = room.seats.get(index) {
                let user = users.iter().find(|user| user.id == seat.id).cloned().unwrap_or_else(|| ServerUser {
                    name: seat.name.clone(),
                    color: seat.color,
                    ..ServerUser::default()
                });
                user_row(
                    frame,
                    &self.server,
                    &user,
                    x + 44.0,
                    row_y + 6.0,
                    width - 44.0 - 120.0,
                    36.0,
                    36.0,
                    seat.id == my_id,
                );
                let mut button_x = x + width - 12.0;
                if is_owner {
                    button_x -= 40.0;
                    let id = format!("rm:down{}", index + 1);
                    let options =
                        IconButtonOptions::tooltip("Move down").id(&id).disabled(index + 1 == room.seats.len());
                    if frame.icon_button("expand_more", button_x, row_y + 4.0, &options) {
                        self.move_seat(room, index, 1);
                    }
                    button_x -= 40.0;
                    let id = format!("rm:up{}", index + 1);
                    let options = IconButtonOptions::tooltip("Move up").id(&id).disabled(index == 0);
                    if frame.icon_button("expand_less", button_x, row_y + 4.0, &options) {
                        self.move_seat(room, index, -1);
                    }
                }
                if seat.id == room.owner_id {
                    let label = "Creator";
                    let label_width = frame.text_width(label, 11.0, Weight::Medium) + 16.0;
                    button_x -= label_width + 8.0;
                    frame.fill_rect(button_x, row_y + 14.0, label_width, 20.0, 10.0, with_alpha(palette.primary, 0.15));
                    let options = TextOptions::new()
                        .size(11.0)
                        .medium()
                        .width(label_width)
                        .middle_of(20.0)
                        .centered()
                        .color(palette.primary);
                    frame.text(label, button_x, row_y + 14.0, options);
                }
            } else {
                let options = TextOptions::new().middle_of(row_height - 8.0).color(palette.on_surface_variant);
                frame.text("Open seat", x + 44.0, row_y, options);
            }
            row_y += row_height;
        }
        row_y += 12.0;
        if is_owner {
            let label = if self.starting { "Starting..." } else { "Start Game" };
            let options = ButtonOptions::default()
                .icon("sports_esports")
                .id("rm:start")
                .disabled(self.starting || room.seats.is_empty());
            if frame.button(label, x, row_y, Some(width), Some(48.0), &options) {
                self.start();
            }
            row_y += 56.0;
            let note = "Seat order is turn order. Players can't join once the game starts, but anyone can watch.";
            frame.text(note, x, row_y, TextOptions::new().width(width).size(12.0).color(palette.on_surface_variant));
        } else {
            let owner = room.seats.iter().find(|seat| seat.id == room.owner_id).map(|seat| seat.name.clone());
            frame.spinner(x + 14.0, row_y + 14.0, 10.0, None);
            let message = format!(
                "{}Waiting for {} to start the game...",
                if spectating { "Watching. " } else { "" },
                owner.as_deref().unwrap_or("the creator")
            );
            let options = TextOptions::new().middle_of(28.0).width(width - 36.0).color(palette.on_surface_variant);
            frame.text(&message, x + 36.0, row_y, options);
        }
    }

    fn draw_chat(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let (lines, room_id) = {
            let server = self.server.borrow();
            (server.room_chat.clone(), server.room_id.clone())
        };
        frame.fill_rect(x, y, width, height, 12.0, palette.surface_container);
        frame.text("Game chat", x + 16.0, y + 12.0, TextOptions::new().bold());
        let input_height = 48.0;
        let (list_y, list_height) = (y + 40.0, height - 40.0 - input_height - 24.0);
        let offset = frame.begin_scroll("rm:chat", x + 8.0, list_y, width - 16.0, list_height);
        let mut line_y = list_y - offset;
        let text_width = width - 32.0;
        for line in &lines {
            let head = format!("{}  {}", local_time_label(line.at), line.name);
            frame.text(&head, x + 16.0, line_y, TextOptions::new().size(12.0).bold().color(palette.on_surface_variant));
            line_y += 18.0;
            frame.text(&line.text, x + 16.0, line_y, TextOptions::new().width(text_width));
            line_y += frame.text_height(&line.text, text_width, 14.0, Weight::Regular) + 10.0;
        }
        frame.end_scroll("rm:chat", line_y + offset - list_y, &ScrollbarStyle::default());
        if lines.len() != self.seen_chat {
            self.seen_chat = lines.len();
            frame.ui.scroll_to_end("rm:chat");
        }
        let options = TextFieldOptions {
            label_background: Some(palette.surface_container),
            max_length: Some(500),
            ..TextFieldOptions::label("Message")
        };
        let field = frame.text_field(
            "rm:chatinput",
            x + 12.0,
            y + height - input_height - 12.0,
            width - 24.0 - 44.0,
            &mut self.chat_text,
            &options,
        );
        let blank = trim_lua_whitespace(&self.chat_text).is_empty();
        let options = IconButtonOptions::default().id("rm:send").disabled(blank);
        let send = frame.icon_button("send", x + width - 12.0 - 40.0, y + height - input_height - 12.0 + 4.0, &options);
        if (field.submitted || send) && !blank {
            let text = std::mem::take(&mut self.chat_text);
            let mut payload = JsonObject::new().with("text", text);
            if let Some(room_id) = room_id {
                payload.insert("roomId", room_id);
            }
            self.server.borrow_mut().send("chatSend", payload.into());
            frame.ui.focus_field("rm:chatinput", false);
        }
    }
}

impl Screen for RoomScreen {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn on_back(&mut self, frame: &mut Frame) {
        {
            let mut server = self.server.borrow_mut();
            server.request_ignored("roomLeave", JsonObject::new());
            server.room_id = None;
            server.spectating = false;
            server.close_channel();
        }
        frame.commands.push(ScreenCommand::Pop);
    }

    fn update(&mut self, frame: &mut Frame, _dt: f32) {
        self.finish_requests(frame);
        // The game started: gameData arrives first, then fullState.
        if !self.started && self.server.borrow().has_game_data() {
            self.started = true;
            let spectating = self.server.borrow().spectating;
            if let Some(game) = self.game.take() {
                let screen = game_screen(frame, &self.server, game, spectating);
                frame.commands.push(ScreenCommand::Replace(screen));
            }
        }
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let room = self.server.borrow().current_room().cloned();
        let Some(room) = room else {
            frame.spinner(x + width / 2.0, y + height / 2.0, 18.0, None);
            return;
        };
        self.title = room.name.clone();
        let padding = 32.0;
        let chat_width = 380.0f32.min((width * 0.36).floor());
        let seats_width = 560.0f32.min(width - 3.0 * padding - chat_width);
        let seats_x = x + padding + 0.0f32.max((width - 3.0 * padding - chat_width - seats_width) / 2.0);
        let offset = frame.begin_scroll("rm:seats", x, y, seats_x + seats_width + padding - x, height);
        self.draw_seats(frame, &room, seats_x, y + padding - offset, seats_width);
        let content_height = 68.0 + room.max_players as f32 * 56.0 + 120.0 + 2.0 * padding;
        frame.end_scroll("rm:seats", content_height, &ScrollbarStyle::default());
        self.draw_chat(frame, x + width - padding - chat_width, y + padding, chat_width, height - 2.0 * padding);
    }
}
