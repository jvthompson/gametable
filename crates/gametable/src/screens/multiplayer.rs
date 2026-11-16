//! Peer multiplayer screens. Port of `src/ui/screens/multiplayer.lua` (`host_setup_screen.dart`,
//! `assign_seats_screen.dart`, `game_select_screen.dart`, `host_game_screen.dart`,
//! `join_screen.dart`, `client_game_screen.dart`).
//!
//! The host server is shared by the lobby screens and the game (`Rc<RefCell<…>>`) and pumped from
//! every live screen's `background_update` (at most once a frame), as the original's global
//! network loop did. Discord invites arrive with Phase 11; until then the Discord tab says
//! Discord isn't available, as the original did without its SDK.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::rc::Rc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gametable_core::host_engine::HostEngine;
use gametable_core::json::{JsonObject, JsonValue};
use gametable_core::models::{DeckConfig, GameDefinition, PlayerInfo, PlayerRole, TableState};
use gametable_core::protocol::{DEFAULT_GAME_PORT, WireMessage};
use gametable_core::seat_utils::is_far_mirror_seat;
use gametable_core::session::{DealOptions, Session, SessionEnvironment};
use gametable_core::util::trim_lua_whitespace;
use gametable_core::uuid::new_v4_uuid;

use crate::app::{Frame, Screen, ScreenCommand};
use crate::data::deck_library::load_all_decks_for_game;
use crate::data::deck_store::{DeckStore, ServerDeckStore};
use crate::data::game_loader::load_games_from_folder;
use crate::data::image_resolver::merge_local_image_paths;
use crate::data::profile;
use crate::graphics::tessellation::DrawMode;
use crate::net::game_client::{ClientStatus, GameClient};
use crate::net::game_connection::GameConnection;
use crate::net::host_server::HostServer;
use crate::net::netinfo::{LookupProgress, PublicAddressLookup, local_ipv4_addresses};
use crate::net::server_client::{ServerClient, ServerGameChannel, ServerStatus, SharedServer};
use crate::net::server_models::local_time_label;
use crate::screens::game_picker::GamePicker;
use crate::screens::load_saved_game::{LoadSavedGame, OnLoaded};
use crate::screens::practice_game::{save_game_dialog, save_table};
use crate::screens::recent_servers::recent_servers_dropdown;
use crate::screens::server::SERVER_LOBBY_MARKER;
use crate::screens::server::login::ServerLogin;
use crate::sound::SoundEffect;
use crate::table::network::{ClientBackend, NetworkHostBackend};
use crate::table::{ChatLine, RoomChat};
use crate::table::{TableEvent, TableOptions, TableScreen};
use crate::ui::components::argb_color;
use crate::ui::draw::TextOptions;
use crate::ui::dropdown::DropdownOptions;
use crate::ui::save_name_dialog::{SaveNameDialog, SaveNameResponse};
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::{Weight, fixed};
use crate::ui::widgets::{ButtonOptions, ButtonVariant, Choice};

type SharedHostServer = Rc<RefCell<HostServer>>;
type SharedClient = Rc<RefCell<GameClient>>;
/// What a joined player's game talks to: a peer host or a server room.
pub type SharedConnection = Rc<RefCell<dyn GameConnection>>;

fn color_dot(frame: &mut Frame, x: f32, y: f32, argb: u32) {
    frame.circle(DrawMode::Fill, x + 6.0, y + 6.0, 6.0, argb_color(argb));
}

/// The column's left edge and top for content of `content_height` (`centeredColumn`).
fn centered_column(x: f32, y: f32, width: f32, height: f32, column_width: f32, content_height: f32) -> (f32, f32) {
    ((x + (width - column_width) / 2.0).floor(), (y + 24f32.max((height - content_height) / 2.0)).floor())
}

fn read_avatar_bytes(frame: &Frame) -> Option<Vec<u8>> {
    frame.profile.avatar_path.as_deref().and_then(|path| std::fs::read(path).ok())
}

/// Registers avatar bytes with the image cache: player id -> cache key (`avatarKeys`).
#[derive(Default)]
struct AvatarRegistry {
    registered: HashMap<String, (u64, String)>,
}

impl AvatarRegistry {
    fn keys(&mut self, frame: &mut Frame, bytes_by_id: &HashMap<String, Vec<u8>>) -> HashMap<String, String> {
        let mut keys = HashMap::new();
        for (id, bytes) in bytes_by_id {
            let mut hasher = DefaultHasher::new();
            bytes.hash(&mut hasher);
            let hash = hasher.finish();
            let key = match self.registered.get(id) {
                Some((known, key)) if *known == hash => key.clone(),
                _ => {
                    let key = format!("avatar:{id}:{hash:016x}");
                    let _ = frame.images.put_bytes(&key, bytes);
                    self.registered.insert(id.clone(), (hash, key.clone()));
                    key
                }
            };
            keys.insert(id.clone(), key);
        }
        keys
    }
}

// ---------------------------------------------------------------------------
// Host Setup (the lobby)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InviteMode {
    Manual,
    Discord,
}

pub struct HostSetup {
    max_players: usize,
    host_player_id: String,
    server: Option<SharedHostServer>,
    start_error: Option<String>,
    addresses: Vec<String>,
    public_lookup: Option<PublicAddressLookup>,
    mode: InviteMode,
    navigated: bool,
}

impl HostSetup {
    pub fn new(frame: &Frame, max_players: usize) -> HostSetup {
        let host_player_id = new_v4_uuid();
        let host = PlayerInfo::new(
            host_player_id.clone(),
            frame.profile.display_name(),
            PlayerRole::Host,
            frame.profile.color,
        );
        let mut setup = HostSetup {
            max_players,
            host_player_id,
            server: None,
            start_error: None,
            addresses: Vec::new(),
            public_lookup: None,
            mode: InviteMode::Manual,
            navigated: false,
        };
        match HostServer::start(host, max_players, read_avatar_bytes(frame), None) {
            Ok(server) => {
                setup.server = Some(Rc::new(RefCell::new(server)));
                setup.addresses = local_ipv4_addresses();
                setup.public_lookup = Some(PublicAddressLookup::start());
            }
            Err(error) => setup.start_error = Some(error),
        }
        setup
    }

    fn draw_discord_panel(&self, frame: &mut Frame, x: f32, y: f32, width: f32) -> f32 {
        let color = frame.palette().on_surface_variant;
        let message = "Discord isn't available (the Discord SDK couldn't be loaded). Use Manual IP instead.";
        frame.text(message, x, y, TextOptions::new().width(width).color(color));
        y + 40.0
    }
}

impl Screen for HostSetup {
    fn title(&self) -> String {
        "Host Game".to_string()
    }

    fn on_back(&mut self, frame: &mut Frame) {
        if let Some(server) = &self.server {
            server.borrow_mut().stop(None);
        }
        frame.commands.push(ScreenCommand::Pop);
    }

    fn background_update(&mut self, frame: &mut Frame, _dt: f32) {
        let Some(server) = &self.server else { return };
        server.borrow_mut().update(frame.ui.frame_index());
        if let Some(lookup) = &mut self.public_lookup
            && let LookupProgress::Done(ip) = lookup.poll()
        {
            server.borrow_mut().public_ip = ip;
            self.public_lookup = None;
        }
    }

    fn update(&mut self, frame: &mut Frame, _dt: f32) {
        let Some(server) = &self.server else { return };
        if self.navigated || server.borrow().roster().len() < self.max_players {
            return;
        }
        self.navigated = true;
        let next: Box<dyn Screen> = if self.max_players > 2 {
            Box::new(AssignSeats::new(server.clone(), self.host_player_id.clone()))
        } else {
            Box::new(HostGameSelect::new(frame, server.clone(), self.host_player_id.clone()))
        };
        frame.commands.push(ScreenCommand::Replace(next));
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let column_width = (width - 48.0).min(420.0);
        let Some(server) = self.server.clone() else {
            let message = format!("Failed to start hosting: {}", self.start_error.clone().unwrap_or_default());
            let options = TextOptions::new().width(width - 48.0).middle_of(height).centered().color(fixed::RED);
            frame.text(&message, x + 24.0, y, options);
            return;
        };
        let roster = server.borrow().roster();
        let (port, public_ip, upnp_mapped) = {
            let server = server.borrow();
            (server.port, server.public_ip.clone(), server.upnp_mapped)
        };
        let (column_x, mut row_y) =
            centered_column(x, y, width, height, column_width, 460.0 + roster.len() as f32 * 28.0);
        let choices =
            [Choice::new(InviteMode::Manual, "Manual IP"), Choice::new(InviteMode::Discord, "Invite via Discord")];
        self.mode = frame.segmented("hs:mode", column_x, row_y, column_width, &choices, self.mode, None, false);
        row_y += 56.0;
        if self.mode == InviteMode::Manual {
            frame.text("Same network (LAN):", column_x, row_y, TextOptions::new().bold());
            row_y += 26.0;
            if self.addresses.is_empty() {
                let message = "No network interfaces found -- try 127.0.0.1 for same-machine testing.";
                frame.text(message, column_x, row_y, TextOptions::new().width(column_width));
                row_y += 40.0;
            } else {
                for address in &self.addresses {
                    frame.text(&format!("{address} : {port}"), column_x, row_y, TextOptions::new().size(16.0));
                    row_y += 24.0;
                }
            }
            row_y += 16.0;
            frame.text("Over the internet:", column_x, row_y, TextOptions::new().bold());
            row_y += 26.0;
            if self.public_lookup.is_some() {
                frame.text("Looking up public IP...", column_x, row_y, TextOptions::new());
                row_y += 24.0;
            } else if let Some(ip) = public_ip {
                frame.text(&format!("{ip} : {port}"), column_x, row_y, TextOptions::new().size(16.0));
                row_y += 24.0;
                let message = match upnp_mapped {
                    None => "Setting up automatic port-forwarding (UPnP)...",
                    Some(true) => "Port automatically forwarded on your router (UPnP).",
                    Some(false) => {
                        "Couldn't automatically forward this port (UPnP unavailable) -- forward it to this PC on your router."
                    }
                };
                let options = TextOptions::new().size(12.0).color(palette.on_surface_variant).width(column_width);
                frame.text(message, column_x, row_y, options);
                row_y += 36.0;
            } else {
                let message = "Couldn't determine your public IP -- check your internet connection.";
                frame.text(message, column_x, row_y, TextOptions::new().width(column_width));
                row_y += 40.0;
            }
        } else {
            row_y = self.draw_discord_panel(frame, column_x, row_y, column_width);
        }
        row_y += 24.0;
        frame.text("Players:", column_x, row_y, TextOptions::new().bold());
        row_y += 28.0;
        for player in &roster {
            color_dot(frame, column_x, row_y + 4.0, player.color);
            frame.text(&player.name, column_x + 20.0, row_y, TextOptions::new());
            row_y += 28.0;
        }
        row_y += 24.0;
        let waiting = self.max_players.saturating_sub(roster.len());
        if waiting > 0 {
            frame.spinner(column_x + column_width / 2.0, row_y + 18.0, 16.0, None);
            row_y += 52.0;
            let message = format!("Waiting for {waiting} more player{}...", if waiting == 1 { "" } else { "s" });
            frame.text(&message, column_x, row_y, TextOptions::new().width(column_width).centered());
        }
    }
}

// ---------------------------------------------------------------------------
// Assign Seats (3-4 players)
// ---------------------------------------------------------------------------

pub struct AssignSeats {
    server: SharedHostServer,
    host_player_id: String,
    order: Vec<PlayerInfo>,
}

impl AssignSeats {
    pub fn new(server: SharedHostServer, host_player_id: String) -> AssignSeats {
        let order = server.borrow().roster();
        AssignSeats { server, host_player_id, order }
    }

    fn labels(&self) -> Vec<String> {
        match self.order.len() {
            3 => vec![
                "Seat 1 -- alone at the bottom on their own screen".to_string(),
                "Seat 2 -- top-left".to_string(),
                "Seat 3 -- top-right".to_string(),
            ],
            4 => vec![
                "Seat 1 -- bottom-left".to_string(),
                "Seat 2 -- bottom-right".to_string(),
                "Seat 3 -- top-left".to_string(),
                "Seat 4 -- top-right".to_string(),
            ],
            count => (1..=count).map(|seat| format!("Seat {seat}")).collect(),
        }
    }

    fn preview_row(frame: &mut Frame, players: &[PlayerInfo], x: f32, y: f32, width: f32) {
        let widths: Vec<f32> =
            players.iter().map(|player| frame.text_width(&player.name, 14.0, Weight::Regular) + 24.0).collect();
        let mut row_x = x + (width - widths.iter().sum::<f32>()) / 2.0;
        for (player, player_width) in players.iter().zip(widths) {
            color_dot(frame, row_x + player_width / 2.0 - 18.0, y, player.color);
            frame.text(&player.name, row_x, y + 16.0, TextOptions::new().width(player_width).centered());
            row_x += player_width;
        }
    }
}

impl Screen for AssignSeats {
    fn title(&self) -> String {
        "Assign Seats".to_string()
    }

    fn on_back(&mut self, frame: &mut Frame) {
        self.server.borrow_mut().stop(Some("hostLeft"));
        frame.commands.push(ScreenCommand::Pop);
    }

    fn background_update(&mut self, frame: &mut Frame, _dt: f32) {
        self.server.borrow_mut().update(frame.ui.frame_index());
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let column_width = (width - 48.0).min(480.0);
        let count = self.order.len();
        let content = 160.0 + 24.0 + count as f32 * 60.0 + 24.0 + 48.0;
        let (column_x, mut row_y) = centered_column(x, y, width, height, column_width, content);
        // A table preview: top row, shared table, bottom row.
        let (top, bottom): (Vec<PlayerInfo>, Vec<PlayerInfo>) = if count == 4 {
            (self.order[2..4].to_vec(), self.order[0..2].to_vec())
        } else {
            (self.order.iter().skip(1).cloned().collect(), self.order.iter().take(1).cloned().collect())
        };
        frame.outline_rect(column_x + 0.5, row_y + 0.5, column_width - 1.0, 159.0, 8.0, palette.outline_variant, 1.0);
        AssignSeats::preview_row(frame, &top, column_x, row_y + 16.0, column_width);
        let options = TextOptions::new().width(column_width).centered().color(palette.on_surface_variant);
        frame.text("(shared table)", column_x, row_y + 64.0, options);
        AssignSeats::preview_row(frame, &bottom, column_x, row_y + 106.0, column_width);
        row_y += 160.0 + 24.0;
        let labels = self.labels();
        for (seat, label) in labels.iter().enumerate().take(count) {
            frame.text(label, column_x, row_y, TextOptions::new().width(column_width - 220.0).middle_of(48.0));
            let choices: Vec<Choice<String>> =
                self.order.iter().map(|player| Choice::new(player.id.clone(), player.name.clone())).collect();
            let current = self.order[seat].id.clone();
            let picked = frame.dropdown(
                &format!("seat:{}", seat + 1),
                column_x + column_width - 200.0,
                row_y,
                200.0,
                &choices,
                current.clone(),
                &DropdownOptions::default(),
            );
            if picked != current
                && let Some(other) = self.order.iter().position(|player| player.id == picked)
            {
                self.order.swap(seat, other);
            }
            row_y += 60.0;
        }
        row_y += 24.0;
        if frame.button("Start", column_x, row_y, Some(column_width), Some(48.0), &ButtonOptions::default()) {
            self.server.borrow_mut().set_seat_order(self.order.iter().map(|player| player.id.clone()).collect());
            let next = HostGameSelect::new(frame, self.server.clone(), self.host_player_id.clone());
            frame.commands.push(ScreenCommand::Replace(Box::new(next)));
        }
    }
}

// ---------------------------------------------------------------------------
// Choose a game (or load a save)
// ---------------------------------------------------------------------------

/// Shared zones with a `deckName` load that deck from the library by name.
pub fn resolve_shared_deck_configs(frame: &Frame, game: &GameDefinition) -> HashMap<String, DeckConfig> {
    let wanted =
        |zone: &&gametable_core::models::ZoneDefinition| zone.shared && !zone.standard_deck && zone.deck_name.is_some();
    if !game.zones.iter().any(|zone| wanted(&zone)) {
        return HashMap::new();
    }
    let decks_folder = frame.paths.decks_folder(frame.settings).to_string_lossy().into_owned();
    let by_name: HashMap<String, DeckConfig> = load_all_decks_for_game(&decks_folder, game)
        .entries
        .into_iter()
        .map(|entry| (entry.display_name, entry.deck))
        .collect();
    game.zones
        .iter()
        .filter(wanted)
        .filter_map(|zone| Some((zone.id.clone(), by_name.get(zone.deck_name.as_deref()?)?.clone())))
        .collect()
}

pub struct HostGameSelect {
    server: SharedHostServer,
    host_player_id: String,
    picker: GamePicker,
}

impl HostGameSelect {
    pub fn new(frame: &Frame, server: SharedHostServer, host_player_id: String) -> HostGameSelect {
        HostGameSelect { server, host_player_id, picker: GamePicker::new(frame, false) }
    }
}

impl Screen for HostGameSelect {
    fn title(&self) -> String {
        "Choose a Game".to_string()
    }

    fn on_back(&mut self, frame: &mut Frame) {
        self.server.borrow_mut().stop(Some("hostLeft"));
        frame.commands.push(ScreenCommand::Pop);
    }

    fn background_update(&mut self, frame: &mut Frame, _dt: f32) {
        self.server.borrow_mut().update(frame.ui.frame_index());
    }

    fn update(&mut self, _frame: &mut Frame, _dt: f32) {
        self.picker.update();
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let label = "Load a Saved Game...";
        let options = ButtonOptions::variant(ButtonVariant::Outlined).icon("folder_open");
        let button_width = frame.button_width(label, &options);
        if frame.button(label, x + width - 16.0 - button_width, y + 16.0, Some(button_width), None, &options) {
            let roster = self.server.borrow().roster();
            let (server, host_player_id) = (self.server.clone(), self.host_player_id.clone());
            let on_loaded: OnLoaded = Box::new(move |frame, game, state| {
                Box::new(HostGame::new(frame, server.clone(), host_player_id.clone(), game, Some(state)))
                    as Box<dyn Screen>
            });
            let screen = LoadSavedGame::new(frame, roster, on_loaded);
            frame.commands.push(ScreenCommand::Push(Box::new(screen)));
        }
        if let Some(game) = self.picker.draw(frame, x, y + 56.0, width, height - 56.0) {
            let screen = HostGame::new(frame, self.server.clone(), self.host_player_id.clone(), game, None);
            frame.commands.push(ScreenCommand::Push(Box::new(screen)));
        }
    }
}

// ---------------------------------------------------------------------------
// The host's game
// ---------------------------------------------------------------------------

pub struct HostGame {
    table: TableScreen,
    server: SharedHostServer,
    save_dialog: Option<SaveNameDialog>,
    avatars: AvatarRegistry,
    avatar_revision: Option<u64>,
    entered: bool,
}

impl HostGame {
    /// `HostGame.new(opts)`: sends everyone the game, deals (or resumes a save) and starts serving.
    pub fn new(
        frame: &Frame,
        server: SharedHostServer,
        host_player_id: String,
        game: GameDefinition,
        loaded_state: Option<TableState>,
    ) -> HostGame {
        server.borrow_mut().broadcast_message("gameData", game.to_json(), None);
        let session = match loaded_state {
            Some(state) => {
                let resume_at = state.log.last().map(|entry| entry.elapsed_milliseconds);
                let mut session =
                    Session::new(game.clone(), host_player_id.clone(), state, false, SessionEnvironment::system());
                if let Some(elapsed) = resume_at {
                    session.resume_log_clock(elapsed);
                }
                session
            }
            None => {
                let shared = resolve_shared_deck_configs(frame, &game);
                let empty = HashMap::new();
                let options = DealOptions {
                    game: game.clone(),
                    players: server.borrow().roster(),
                    local_player_id: host_player_id.clone(),
                    deck_configs_by_player_id: Some(&empty),
                    shared_deck_configs_by_zone_id: Some(&shared),
                    is_local_practice: false,
                };
                Session::deal_from_zones(options, SessionEnvironment::system())
            }
        };
        let players = session.state().players.clone();
        let seat = players.iter().position(|player| player.id == host_player_id).unwrap_or(0);
        let is_mirrored = !game.same_orientation_for_all && is_far_mirror_seat(seat, players.len());
        let engine = HostEngine::new(session, host_player_id, None);
        let backend = NetworkHostBackend::new(engine, server.clone());
        let options = TableOptions {
            backend: Box::new(backend),
            is_mirrored,
            local_avatar_path: frame.profile.avatar_path.clone(),
            avatar_keys: HashMap::new(),
            leave_button_label: "End Game".to_string(),
            leave_confirmation_message: "End the game for everyone? Every connected player will be disconnected -- \
                                         you can start a new session any time."
                .to_string(),
            can_save_game: game.folder_path.is_some(),
            can_kick_players: true,
            can_load_scenario: true,
            collection_mode: false,
            spectator: false,
            deck_store: None,
        };
        HostGame {
            table: TableScreen::new(options),
            server,
            save_dialog: None,
            avatars: AvatarRegistry::default(),
            avatar_revision: None,
            entered: false,
        }
    }
}

impl Screen for HostGame {
    fn title(&self) -> String {
        self.table.game().name.clone()
    }

    fn shows_back(&self) -> bool {
        false
    }

    fn forces_light_theme(&self) -> bool {
        true
    }

    fn quit(&mut self, _frame: &mut Frame) -> bool {
        self.server.borrow_mut().stop(Some("hostLeft"));
        false
    }

    fn update(&mut self, frame: &mut Frame, dt: f32) {
        let revision = self.server.borrow().avatar_revision;
        if self.avatar_revision != Some(revision) {
            self.avatar_revision = Some(revision);
            let bytes = self.server.borrow().avatars_by_player_id();
            let keys = self.avatars.keys(frame, &bytes);
            self.table.set_avatar_keys(keys);
        }
        self.table.update(frame, dt);
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        if !self.entered {
            self.entered = true;
            self.table.enter(frame);
        }
        self.table.draw(frame, x, y, width, height);
        for event in self.table.take_events() {
            match event {
                TableEvent::LeaveGame => {
                    self.server.borrow_mut().stop(Some("hostLeft"));
                    frame.commands.push(ScreenCommand::PopToHome(Some("You ended the game.".to_string())));
                }
                TableEvent::SaveGame => self.save_dialog = save_game_dialog(self.table.game()),
                TableEvent::KickPlayer(player_id) => self.server.borrow_mut().kick(&player_id),
            }
        }
        if let Some(dialog) = &mut self.save_dialog {
            match dialog.show(frame, None) {
                SaveNameResponse::Open => {}
                SaveNameResponse::Cancelled => self.save_dialog = None,
                SaveNameResponse::Save(name) => {
                    self.save_dialog = None;
                    save_table(frame, &self.table, &name);
                }
            }
        }
    }

    fn key_pressed(&mut self, frame: &mut Frame, key: &'static str, is_repeat: bool) {
        self.table.key_pressed(frame, key, is_repeat);
    }
}

// ---------------------------------------------------------------------------
// Join
// ---------------------------------------------------------------------------

pub struct Join {
    host_text: String,
    port_text: String,
    auto_connect: bool,
    client: Option<SharedClient>,
    recent_open: bool,
    navigated: bool,
    connect_on_first_frame: bool,
}

impl Join {
    /// `Join.new(opts)`: with an address and `auto_connect` it connects straight away.
    pub fn new(host: Option<&str>, port: Option<u16>, auto_connect: bool) -> Join {
        Join {
            host_text: host.unwrap_or("").to_string(),
            port_text: port.unwrap_or(DEFAULT_GAME_PORT).to_string(),
            auto_connect,
            client: None,
            recent_open: false,
            navigated: false,
            connect_on_first_frame: auto_connect,
        }
    }

    fn host_and_port(&self) -> (String, u16) {
        let host = trim_lua_whitespace(&self.host_text);
        let host = if host.is_empty() { "127.0.0.1".to_string() } else { host.to_string() };
        let port = trim_lua_whitespace(&self.port_text).parse().unwrap_or(DEFAULT_GAME_PORT);
        (host, port)
    }

    fn connect(&mut self, frame: &Frame) {
        let (host, port) = self.host_and_port();
        let host_key = format!("{host}:{port}");
        let mut client = GameClient::new();
        let rejoin = profile::reconnect_player_id_for(frame.settings, &host_key).map(str::to_string);
        client.connect(
            &host,
            port,
            &frame.profile.display_name(),
            frame.profile.color,
            read_avatar_bytes(frame),
            rejoin,
        );
        self.client = Some(Rc::new(RefCell::new(client)));
    }
}

impl Screen for Join {
    fn title(&self) -> String {
        if self.auto_connect { "Joining...".to_string() } else { "Join Game".to_string() }
    }

    fn on_back(&mut self, frame: &mut Frame) {
        if let Some(client) = &self.client {
            client.borrow_mut().disconnect();
        }
        frame.commands.push(ScreenCommand::Pop);
    }

    fn background_update(&mut self, _frame: &mut Frame, _dt: f32) {
        if let Some(client) = &self.client {
            client.borrow_mut().update();
        }
    }

    fn update(&mut self, frame: &mut Frame, _dt: f32) {
        if std::mem::take(&mut self.connect_on_first_frame) {
            self.connect(frame);
        }
        let Some(client) = self.client.clone() else { return };
        if self.navigated {
            return;
        }
        let (host, port) = self.host_and_port();
        let status = client.borrow().status;
        if status == ClientStatus::Server {
            // A CardTable server: its sign-in screen takes over the connection.
            self.navigated = true;
            profile::add_recent_server(frame.settings, &host, f64::from(port));
            let (connection, info) = {
                let mut client = client.borrow_mut();
                (client.connection.take(), client.server_info.clone().unwrap_or(JsonValue::Null))
            };
            if let Some(connection) = connection {
                let server = Rc::new(RefCell::new(ServerClient::adopt(connection, &host, port, &info)));
                let screen = ServerLogin::new(frame, server);
                frame.commands.push(ScreenCommand::Replace(Box::new(screen)));
            }
            return;
        }
        let assigned = client.borrow().assigned_player_id.clone();
        if status == ClientStatus::Connected
            && let Some(player_id) = assigned
        {
            self.navigated = true;
            profile::save_reconnect(frame.settings, &format!("{host}:{port}"), &player_id);
            profile::add_recent_server(frame.settings, &host, f64::from(port));
            let game = ClientGame::new(frame, client, player_id);
            frame.commands.push(ScreenCommand::Replace(Box::new(game)));
        }
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let column_width = (width - 48.0).min(360.0);
        let (column_x, row_y) = centered_column(x, y, width, height, column_width, 48.0 + 12.0 + 48.0 + 24.0 + 48.0);
        let status = self.client.as_ref().map_or(ClientStatus::Idle, |client| client.borrow().status);
        if status == ClientStatus::Idle {
            let recent = profile::recent_servers(frame.settings);
            let host_options = TextFieldOptions {
                hint: Some("e.g. 192.168.1.42 -- blank = 127.0.0.1"),
                autofocus: true,
                trailing_width: if recent.is_empty() { 0.0 } else { 36.0 },
                ..TextFieldOptions::label("Host or server IP address")
            };
            let host_field =
                frame.text_field("join:ip", column_x, row_y, column_width, &mut self.host_text, &host_options);
            if !recent.is_empty()
                && let Some(server) = recent_servers_dropdown(
                    frame,
                    "join:recent",
                    &recent,
                    column_x,
                    row_y,
                    column_width,
                    &mut self.recent_open,
                )
            {
                self.host_text = server.host;
                self.port_text = server.port.to_string();
            }
            let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
            let port_options = TextFieldOptions { filter: Some(&digits), ..TextFieldOptions::label("Port") };
            let port_field =
                frame.text_field("join:port", column_x, row_y + 60.0, column_width, &mut self.port_text, &port_options);
            let connect = frame.button(
                "Connect",
                column_x,
                row_y + 132.0,
                Some(column_width),
                Some(48.0),
                &ButtonOptions::default(),
            );
            if connect || host_field.submitted || port_field.submitted {
                self.connect(frame);
            }
            return;
        }
        let Some(client) = self.client.clone() else { return };
        let client = client.borrow();
        match status {
            ClientStatus::Connecting => {
                frame.spinner(column_x + column_width / 2.0, row_y + 20.0, 16.0, None);
                frame.text("Connecting...", column_x, row_y + 52.0, TextOptions::new().width(column_width).centered());
            }
            ClientStatus::Connected => {
                let message = format!(
                    "{}{}!",
                    if client.reconnected { "Reconnected to " } else { "Connected to " },
                    client.opponent_name.as_deref().unwrap_or("host")
                );
                let options = TextOptions::new().width(column_width).centered().bold().color(fixed::GREEN);
                frame.text(&message, column_x, row_y, options);
            }
            ClientStatus::Failed => {
                let message = format!("Failed to connect: {}", client.error_message.clone().unwrap_or_default());
                frame.text(
                    &message,
                    column_x,
                    row_y,
                    TextOptions::new().width(column_width).centered().color(fixed::RED),
                );
                let options = ButtonOptions::variant(ButtonVariant::Outlined);
                let button_width = frame.button_width("Try again", &options);
                let button_y = row_y + frame.text_height(&message, column_width, 14.0, Weight::Regular) + 16.0;
                drop(client);
                if frame.button(
                    "Try again",
                    column_x + (column_width - button_width) / 2.0,
                    button_y,
                    Some(button_width),
                    None,
                    &options,
                ) {
                    self.client = None;
                }
            }
            _ => {
                let message = match client.disconnect_reason.as_deref() {
                    Some("seatUnavailable") => {
                        "This game is already in progress and your seat couldn't be found. Make sure you're reconnecting \
                         to the same host."
                    }
                    Some("versionMismatch") => "That CardTable server needs a different version of CardTable.",
                    Some("serverRequiresUpdate") => "That CardTable server needs a newer version of CardTable.",
                    Some("serverShuttingDown") => {
                        "That CardTable server is shutting down and isn't accepting connections."
                    }
                    _ => "Disconnected from host.",
                };
                frame.text(
                    message,
                    column_x,
                    row_y,
                    TextOptions::new().width(column_width).centered().color(fixed::RED),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A joined player's game
// ---------------------------------------------------------------------------

fn disconnect_message(reason: Option<&str>) -> &'static str {
    match reason {
        Some("kicked") => "You were removed from the game by the host.",
        Some("hostLeft") => "The host ended the game.",
        Some("seatUnavailable") => {
            "This game is already in progress and your seat couldn't be found. Make sure you're reconnecting to the \
             same host."
        }
        _ => "Disconnected from host.",
    }
}

/// What a CardTable server game adds to [`ClientGame`] (the original's `opts` from
/// `server/common.lua`'s `gameScreen`).
pub struct ServerGameContext {
    pub server: SharedServer,
    /// The downloaded copy of the game (its image paths are used for the table).
    pub local_game: GameDefinition,
    pub spectator: bool,
    /// The room's creator may load scenarios (the server is the authority, not a host).
    pub is_room_owner: bool,
    pub collection_mode: bool,
}

/// The server room's chat for the table's chat panel.
struct ServerRoomChat {
    server: SharedServer,
}

impl RoomChat for ServerRoomChat {
    fn lines(&self) -> Vec<ChatLine> {
        self.server
            .borrow()
            .room_chat
            .iter()
            .map(|line| ChatLine {
                time_label: local_time_label(line.at),
                name: line.name.clone(),
                text: line.text.clone(),
            })
            .collect()
    }

    fn send(&mut self, text: &str) {
        let mut server = self.server.borrow_mut();
        let mut payload = JsonObject::new().with("text", text);
        if let Some(room_id) = server.room_id.clone() {
            payload.insert("roomId", room_id);
        }
        server.send("chatSend", payload.into());
    }
}

pub struct ClientGame {
    client: SharedConnection,
    server: Option<ServerGameContext>,
    local_player_id: String,
    roster: Vec<PlayerInfo>,
    max_players: usize,
    avatar_bytes: HashMap<String, Vec<u8>>,
    avatar_keys: HashMap<String, String>,
    avatars: AvatarRegistry,
    game: Option<GameDefinition>,
    table: Option<TableScreen>,
    navigated_home: bool,
    entered: bool,
    /// Replay the cached messages on the first update (they need the frame).
    replay_pending: bool,
}

impl ClientGame {
    pub fn new(_frame: &Frame, client: SharedConnection, local_player_id: String) -> ClientGame {
        ClientGame {
            client,
            server: None,
            local_player_id,
            roster: Vec::new(),
            max_players: 2,
            avatar_bytes: HashMap::new(),
            avatar_keys: HashMap::new(),
            avatars: AvatarRegistry::default(),
            game: None,
            table: None,
            navigated_home: false,
            entered: false,
            replay_pending: true,
        }
    }

    /// A CardTable server game: the room's channel on an already-joined server.
    pub fn for_server(frame: &Frame, context: ServerGameContext) -> ClientGame {
        // Spectators watch under the id the server filters for them.
        let local_player_id = {
            let server = context.server.borrow();
            if context.spectator { format!("spectator:{}", server.my_id()) } else { server.my_id().to_string() }
        };
        let channel: SharedConnection = Rc::new(RefCell::new(ServerGameChannel { server: context.server.clone() }));
        ClientGame { server: Some(context), ..ClientGame::new(frame, channel, local_player_id) }
    }

    /// `ClientGame:returnHome(message)`: a server game leaves the room and returns to the lobby
    /// (or Home when the server is gone).
    fn return_home(&mut self, frame: &mut Frame, message: &str) {
        if self.navigated_home {
            return;
        }
        self.navigated_home = true;
        self.client.borrow_mut().close();
        let Some(context) = &self.server else {
            frame.commands.push(ScreenCommand::PopToHome(Some(message.to_string())));
            return;
        };
        let mut server = context.server.borrow_mut();
        if server.room_id.is_some() {
            server.request_ignored("roomLeave", JsonObject::new());
        }
        server.room_id = None;
        server.spectating = false;
        server.close_channel();
        if server.status != ServerStatus::Connected {
            frame.commands.push(ScreenCommand::PopToHome(Some(message.to_string())));
        } else {
            let message = (!context.spectator).then(|| message.to_string());
            frame.commands.push(ScreenCommand::PopTo { marker: SERVER_LOBBY_MARKER, message });
        }
    }

    /// Uses this machine's own image paths for the host's game (none where a card is missing).
    fn resolve_local_images(&self, frame: &Frame, remote: GameDefinition) -> GameDefinition {
        if let Some(context) = &self.server {
            return merge_local_image_paths(&remote, &context.local_game);
        }
        let library = frame.paths.games_folder(frame.settings).to_string_lossy().into_owned();
        if !Path::new(&library).is_dir() {
            return remote;
        }
        let (games, _) = load_games_from_folder(&library);
        match games.iter().find(|game| game.id == remote.id) {
            Some(local) => merge_local_image_paths(&remote, local),
            None => remote,
        }
    }

    fn is_mirrored(&self) -> bool {
        let Some(table) = &self.table else { return false };
        if table.game().same_orientation_for_all {
            return false;
        }
        let players = &table.backend().session().state().players;
        if players.len() < 2 {
            return true;
        }
        players
            .iter()
            .position(|player| player.id == self.local_player_id)
            .is_none_or(|seat| is_far_mirror_seat(seat, players.len()))
    }

    fn handle_message(&mut self, frame: &mut Frame, message: WireMessage) {
        let payload = &message.payload;
        match message.message_type.as_str() {
            "lobbyRosterUpdate" => {
                self.roster = payload
                    .get("players")
                    .and_then(JsonValue::as_array)
                    .map(|players| players.iter().filter_map(|player| PlayerInfo::from_json(player).ok()).collect())
                    .unwrap_or_default();
                if let Some(max_players) = payload.get("maxPlayers").and_then(JsonValue::as_f64) {
                    self.max_players = max_players as usize;
                }
                self.avatar_bytes = payload
                    .get("avatars")
                    .and_then(JsonValue::as_object)
                    .map(|avatars| {
                        avatars
                            .iter()
                            .filter_map(|(id, text)| Some((id.clone(), BASE64.decode(text.as_str()?).ok()?)))
                            .collect()
                    })
                    .unwrap_or_default();
                self.avatar_keys = self.avatars.keys(frame, &self.avatar_bytes);
                if let Some(table) = &mut self.table {
                    table.set_avatar_keys(self.avatar_keys.clone());
                }
            }
            "gameData" => {
                let Ok(game) = GameDefinition::from_json(payload) else { return };
                let game = self.resolve_local_images(frame, game);
                if let Some(table) = &mut self.table {
                    table.replace_game(game.clone());
                }
                self.game = Some(game);
            }
            "cardDragPreview" | "cardDragPreviewEnd" | "arrowDragPreview" | "arrowDragPreviewEnd" => {
                let Some(table) = &mut self.table else { return };
                let session = table.backend_mut().session_mut();
                let player_id = payload.get("playerId").and_then(JsonValue::as_str).unwrap_or("").to_string();
                let number = |key: &str| payload.get(key).and_then(JsonValue::as_f64).unwrap_or(0.0);
                match message.message_type.as_str() {
                    "cardDragPreview" => {
                        let instance_ids = payload
                            .get("instanceIds")
                            .and_then(JsonValue::as_array)
                            .map(|ids| ids.iter().filter_map(|id| id.as_str().map(str::to_string)).collect())
                            .unwrap_or_default();
                        let preview = gametable_core::session::CardDragPreview {
                            player_id: player_id.clone(),
                            instance_ids,
                            fraction_x: number("fx"),
                            fraction_y: number("fy"),
                        };
                        session.card_drag_previews.insert(player_id, preview);
                    }
                    "cardDragPreviewEnd" => {
                        session.card_drag_previews.remove(&player_id);
                    }
                    "arrowDragPreview" => {
                        let preview = gametable_core::session::ArrowDragPreview {
                            player_id: player_id.clone(),
                            fraction_x: number("fx"),
                            fraction_y: number("fy"),
                            fraction_x2: number("fx2"),
                            fraction_y2: number("fy2"),
                        };
                        session.arrow_drag_previews.insert(player_id, preview);
                    }
                    _ => {
                        session.arrow_drag_previews.remove(&player_id);
                    }
                }
            }
            "fullState" => {
                let Ok(state) = TableState::from_json(payload) else { return };
                match &mut self.table {
                    Some(table) => table.backend_mut().session_mut().apply_remote_state(state),
                    None => {
                        let Some(game) = self.game.clone() else { return };
                        let session = Session::new(
                            game,
                            self.local_player_id.clone(),
                            state,
                            false,
                            SessionEnvironment::system(),
                        );
                        let server = self.server.as_ref();
                        let leave_message = match server {
                            Some(context) if context.spectator => "Stop watching this game?",
                            Some(_) => {
                                "Leave the game? Your seat is kept, so you can rejoin from the lobby while the game \
                                 is running."
                            }
                            None => {
                                "Leave the game? If the host is still hosting, you can rejoin in the same seat later."
                            }
                        };
                        let deck_store = server.map(|context| {
                            let folder = frame.paths.decks_folder(frame.settings).to_string_lossy().into_owned();
                            Box::new(ServerDeckStore::new(context.server.clone(), folder)) as Box<dyn DeckStore>
                        });
                        let options = TableOptions {
                            backend: Box::new(ClientBackend { session, client: self.client.clone() }),
                            is_mirrored: false,
                            local_avatar_path: frame.profile.avatar_path.clone(),
                            avatar_keys: self.avatar_keys.clone(),
                            leave_button_label: "Leave Game".to_string(),
                            leave_confirmation_message: leave_message.to_string(),
                            can_save_game: false,
                            can_kick_players: false,
                            can_load_scenario: server.is_some_and(|context| context.is_room_owner),
                            collection_mode: server.is_some_and(|context| context.collection_mode),
                            spectator: server.is_some_and(|context| context.spectator),
                            deck_store,
                        };
                        let mut table = TableScreen::new(options);
                        if let Some(context) = server {
                            table.set_room_chat(Box::new(ServerRoomChat { server: context.server.clone() }));
                        }
                        self.table = Some(table);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Screen for ClientGame {
    fn title(&self) -> String {
        self.game.as_ref().map_or_else(|| "Game".to_string(), |game| game.name.clone())
    }

    fn shows_back(&self) -> bool {
        false
    }

    fn forces_light_theme(&self) -> bool {
        true
    }

    fn quit(&mut self, _frame: &mut Frame) -> bool {
        self.client.borrow_mut().close();
        if let Some(context) = &self.server {
            context.server.borrow_mut().disconnect();
        }
        false
    }

    fn background_update(&mut self, frame: &mut Frame, _dt: f32) {
        if std::mem::take(&mut self.replay_pending) {
            let cached = self.client.borrow().cached_messages();
            // The live inbox holds these too; replay the cached copies once and drop the rest.
            self.client.borrow_mut().take_messages();
            for message in cached {
                self.handle_message(frame, message);
            }
        }
        self.client.borrow_mut().pump(frame);
        let messages = self.client.borrow_mut().take_messages();
        for message in messages {
            self.handle_message(frame, message);
        }
        let (disconnected, reason) = {
            let client = self.client.borrow();
            (client.is_disconnected(), client.disconnect_reason())
        };
        if disconnected && !self.navigated_home {
            frame.sounds.play(SoundEffect::Error);
            let message = match &self.server {
                Some(context) => context.server.borrow().disconnect_message(),
                None => disconnect_message(reason.as_deref()).to_string(),
            };
            self.return_home(frame, &message);
        }
    }

    fn update(&mut self, frame: &mut Frame, dt: f32) {
        if let (Some(context), Some(table)) = (&self.server, &mut self.table) {
            let keys = context.server.borrow_mut().avatar_keys_by_user_id();
            table.set_avatar_keys(keys);
        }
        let mirrored = self.is_mirrored();
        if let Some(table) = &mut self.table {
            table.set_mirrored(mirrored);
            table.update(frame, dt);
        }
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let Some(table) = &mut self.table else {
            let (center_x, center_y) = (x + width / 2.0, y + height / 2.0);
            frame.spinner(center_x, center_y - 30.0, 18.0, None);
            let options = TextOptions::new().width(width).centered();
            let waiting =
                if self.server.is_some() { "Loading the game..." } else { "Waiting for host to start the game..." };
            frame.text(waiting, x, center_y, options);
            if self.roster.len() > 1 {
                let joined = format!("{}/{} players joined", self.roster.len(), self.max_players);
                frame.text(&joined, x, center_y + 28.0, options);
            }
            return;
        };
        if !self.entered {
            self.entered = true;
            table.enter(frame);
        }
        table.draw(frame, x, y, width, height);
        let events = table.take_events();
        for event in events {
            if event == TableEvent::LeaveGame {
                self.return_home(frame, "You left the game.");
            }
        }
    }

    fn key_pressed(&mut self, frame: &mut Frame, key: &'static str, is_repeat: bool) {
        if let Some(table) = &mut self.table {
            table.key_pressed(frame, key, is_repeat);
        }
    }
}
