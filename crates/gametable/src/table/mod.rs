//! The game table. Port of `src/ui/table/` and `src/game/controller.lua`.
//!
//! - `actions`: what the table asks for, and how a host/practice session applies it.
//! - `screen`: the table's state, derived data, selection, game boards, hotkeys and keyboard.
//! - `view`: layout and drawing, recording hit items for the next frame's input.
//! - `input`: drags, drops, box selection, arrows, panning, zooming and clicks.
//! - `menus`: context menus and dialogs.
//! - `panels`: the Game Menu, log, chat, search, card library and token picker, the Space
//!   preview, drag feedback and the per-frame `draw`.
//! - `board_layout`: which game boards are shown and where they sit on the table.
//! - `board_zones`: which zones are placed on the boards in use, and where on them.
//! - `board_handle`: moving and turning your own player boards during the game.
//! - `card_render`, `selection`, `card_pool_grid`: shared drawing and geometry helpers.

pub mod actions;
pub mod board_handle;
pub mod board_layout;
pub mod board_zones;
pub mod card_pool_grid;
pub mod card_render;
pub mod input;
pub mod menus;
pub mod network;
pub mod panels;
pub mod screen;
pub mod selection;
pub mod view;

pub use actions::{PracticeBackend, TableAction, TableBackend};
pub use screen::{ChatLine, RoomChat, TableEvent, TableOptions, TableScreen};
