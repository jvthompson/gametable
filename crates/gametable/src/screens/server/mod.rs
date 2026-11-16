//! The CardTable server screens: sign-in, the lobby, a room before it starts, the collection,
//! the card shop, trading and administration. Shared helpers are ports of
//! `src/ui/screens/server/common.lua`: game downloads with a progress dialog, choosing a game,
//! entering rooms and opening the table.

pub mod admin;
pub mod collection;
pub mod lobby;
pub mod login;
pub mod room;
pub mod shop;
pub mod trade;

use gametable_core::models::GameDefinition;

use crate::app::{Frame, Screen, ScreenCommand};
use crate::data::game_loader::STANDARD_DECK_ID;
use crate::net::asset_sync::{DownloadProgress, GameDownload, game_folder};
use crate::net::server_client::{SharedServer, saved_session};
use crate::net::server_models::{ServerGameInfo, ServerUser};
use crate::screens::game_picker::GameTile;
use crate::screens::multiplayer::{ClientGame, ServerGameContext};
use crate::ui::components::AvatarSource;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{DialogResponse, DialogSpec};
use crate::ui::theme::Weight;

/// [`Screen::marker`] of the lobby, for [`ScreenCommand::PopTo`].
pub const SERVER_LOBBY_MARKER: &str = "server_lobby";

/// `formatBytes(n)`.
pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", (bytes as f64 / 1024.0 + 0.5).floor() as u64)
    }
}

/// `n token(s)`.
pub fn tokens_label(count: i64) -> String {
    format!("{count}{}", if count == 1 { " token" } else { " tokens" })
}

// ---------------------------------------------------------------------------
// Game downloads
// ---------------------------------------------------------------------------

/// `M.withGame(sc, gameId, onReady)`: makes sure a server game is downloaded and current,
/// showing a progress dialog while files download.
pub struct GameLoad {
    download: GameDownload,
    game_name: String,
}

impl GameLoad {
    pub fn start(frame: &Frame, server: &SharedServer, game_id: &str) -> GameLoad {
        let games_folder = frame.paths.games_folder(frame.settings).to_string_lossy().into_owned();
        let mut client = server.borrow_mut();
        let saved_token = saved_session(frame.settings, &client.key).and_then(|session| session.token);
        let game_name = client.game_by_id(game_id).map_or_else(|| "game".to_string(), |game| game.name.clone());
        let download = GameDownload::start(&mut client, frame.paths.save_folder(), &games_folder, saved_token, game_id);
        GameLoad { download, game_name }
    }

    pub fn game_id(&self) -> &str {
        &self.download.game_id
    }

    /// Advances the download and draws its dialog. `Some(Some(game))` once loaded, `Some(None)`
    /// when it failed (after an alert), `None` while it runs.
    pub fn show(&mut self, frame: &mut Frame, server: &SharedServer) -> Option<Option<GameDefinition>> {
        self.show_with_dialog(frame, server, true)
    }

    /// [`GameLoad::show`], drawing the progress dialog only when `draw_dialog` (one at a time).
    pub fn show_with_dialog(
        &mut self,
        frame: &mut Frame,
        server: &SharedServer,
        draw_dialog: bool,
    ) -> Option<Option<GameDefinition>> {
        let progress = self.download.poll(&mut server.borrow_mut());
        match progress {
            DownloadProgress::Checking => None,
            DownloadProgress::Ready(game) => Some(Some(*game)),
            DownloadProgress::Failed(message) => {
                frame.ui.alert("Couldn't Load Game", &message);
                Some(None)
            }
            DownloadProgress::Downloading { .. } if !draw_dialog => None,
            DownloadProgress::Downloading { done, total, bytes, reused } => {
                let title = format!("Downloading {}", self.game_name);
                let spec =
                    DialogSpec { title: Some(&title), width: 420.0, dismissible: false, ..DialogSpec::default() };
                let response = frame.dialog("gamedownload", &spec, |frame, x, y, width| {
                    let fraction = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                    frame.progress_bar(x, y + 4.0, width, 4.0, Some(fraction), None);
                    let mut status = format!("{done} of {total} files ({} downloaded", format_bytes(bytes));
                    if reused > 0 {
                        status.push_str(&format!(", {reused} already on this computer"));
                    }
                    status.push(')');
                    frame.text(&status, x, y + 20.0, TextOptions::new().width(width).ellipsis());
                    let note =
                        "Game files are kept on this computer, so this only happens when the server's copy changes.";
                    let color = frame.palette().on_surface_variant;
                    frame.text(note, x, y + 44.0, TextOptions::new().width(width).size(12.0).color(color));
                    44.0 + frame.text_height(note, width, 12.0, Weight::Regular)
                });
                let _ = response == DialogResponse::Open;
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Choosing a game
// ---------------------------------------------------------------------------

/// What the lobby does with a game picked on a [`GameChoice`] screen.
#[derive(Debug, Clone, PartialEq)]
pub enum GamePickPurpose {
    /// The header's Choose Game.
    Select,
    /// Nothing was selected yet: select it, then open this.
    SelectThen(LobbyAction),
    /// Ask this user to trade in the picked game.
    Trade { user_id: String, user_name: String },
}

/// The lobby's game-based actions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LobbyAction {
    CreateGame,
    Collection,
    DeckEditor,
    Shop,
}

/// `M.pickGame(sc, title, subtitle, onPick, opts)`: a one-off "choose a game" tile screen. The
/// pick goes back to the lobby through `server.picked_game`.
pub struct GameChoice {
    server: SharedServer,
    title: String,
    subtitle: String,
    purpose: GamePickPurpose,
    exclude_standard_deck: bool,
}

impl GameChoice {
    pub fn new(
        server: SharedServer,
        title: &str,
        subtitle: &str,
        purpose: GamePickPurpose,
        exclude_standard_deck: bool,
    ) -> GameChoice {
        GameChoice { server, title: title.to_string(), subtitle: subtitle.to_string(), purpose, exclude_standard_deck }
    }
}

impl Screen for GameChoice {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let padding = 24.0;
        let (x, y, width, height) = (x + padding, y + padding, width - 2.0 * padding, height - 2.0 * padding);
        let mut row_y = y;
        frame.text("Choose a game", x, row_y, TextOptions::new().size(22.0));
        row_y += 32.0;
        frame.text(&self.subtitle, x, row_y, TextOptions::new().width(width).color(palette.on_surface_variant));
        row_y += frame.text_height(&self.subtitle, width, 14.0, Weight::Regular) + 20.0;
        let (games, key) = {
            let server = self.server.borrow();
            (server.games.clone(), server.key.clone())
        };
        let shown: Vec<&ServerGameInfo> =
            games.iter().filter(|game| !(self.exclude_standard_deck && game.id == STANDARD_DECK_ID)).collect();
        if shown.is_empty() {
            let options = TextOptions::new().width(width).color(palette.on_surface_variant);
            frame.text("This server doesn't have any games yet.", x, row_y, options);
            return;
        }
        let folders: Vec<String> =
            shown.iter().map(|game| game_folder(frame.paths.save_folder(), &key, &game.id)).collect();
        let tiles: Vec<GameTile> = shown
            .iter()
            .zip(&folders)
            .map(|(game, folder)| {
                let (count, sets) = (game.card_count, game.sets.len());
                let mut caption = format!("{count}{}", if count == 1 { " card" } else { " cards" });
                if sets > 0 {
                    caption.push_str(&format!("  ·  {sets}{}", if sets == 1 { " set" } else { " sets" }));
                }
                GameTile {
                    id: &game.id,
                    name: &game.name,
                    folder_path: Some(folder),
                    caption: Some(caption),
                    ..GameTile::default()
                }
            })
            .collect();
        if let Some(index) = frame.game_grid("gc:games", &tiles, x, row_y, width, y + height - row_y, padding) {
            self.server.borrow_mut().picked_game = Some((self.purpose.clone(), shown[index].id.clone()));
            frame.commands.push(ScreenCommand::Pop);
        }
    }
}

// ---------------------------------------------------------------------------
// The table and user rows
// ---------------------------------------------------------------------------

/// `M.gameScreen(sc, game, spectating)`: the table for a running room.
pub fn game_screen(frame: &Frame, server: &SharedServer, game: GameDefinition, spectating: bool) -> Box<dyn Screen> {
    let (is_room_owner, collection_mode) = {
        let client = server.borrow();
        let room = client.current_room();
        (
            !spectating && room.is_some_and(|room| room.owner_id == client.my_id()),
            room.is_some_and(|room| room.collection_mode),
        )
    };
    server.borrow_mut().open_channel();
    let context = ServerGameContext {
        server: server.clone(),
        local_game: game,
        spectator: spectating,
        is_room_owner,
        collection_mode,
    };
    Box::new(ClientGame::for_server(frame, context))
}

/// `M.userRow`: a small avatar and name.
#[allow(clippy::too_many_arguments)]
pub fn user_row(
    frame: &mut Frame,
    server: &SharedServer,
    user: &ServerUser,
    x: f32,
    y: f32,
    width: f32,
    size: f32,
    row_height: f32,
    bold: bool,
) {
    let key = server.borrow_mut().avatar_key(&user.avatar_hash);
    let source = key.as_deref().map(AvatarSource::Key);
    frame.avatar(x, y + (row_height - size) / 2.0, size, user.color, source, false);
    let mut options = TextOptions::new().width(width - size - 10.0).middle_of(row_height).ellipsis();
    if bold {
        options = options.bold();
    }
    frame.text(&user.name, x + size + 10.0, y, options);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_and_token_labels() {
        assert_eq!(format_bytes(512), "1 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024 / 2), "1.5 MB");
        assert_eq!(tokens_label(1), "1 token");
        assert_eq!(tokens_label(3), "3 tokens");
    }
}
