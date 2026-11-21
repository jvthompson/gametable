//! The game table's state and behaviour. Port of `src/ui/table/table_screen.lua` (from
//! `lib/ui/table_screen.dart`): derived per-frame data, selection, playmats, hovered-card actions
//! and the keyboard. Drawing is in `view.rs`, mouse input and drops in `input.rs`, context menus
//! and dialogs in `menus.rs`, and the in-game panels (menu, log, chat, search, library, tokens) in
//! `panels.rs`.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gametable_core::color::ArgbColor;
use gametable_core::models::{
    BoardKind, BoardWidgetInstance, BoardWidgetKind, BoardZonePosition, CardDefinition, CardInstance, CardZone,
    GameDefinition, PlayerInfo, Scenario, SearchTargetType, TableState, ZoneDefinition,
};
use gametable_core::seat_utils::{compute_hand_row_layout, is_far_mirror_seat};
use gametable_core::session::{ArrowDragPreview, CardDragPreview};
use gametable_core::stack_utils::{group_by_stack, root_id_of};

use crate::app::Frame;
use crate::data::deck_store::DeckStore;
use crate::data::display_settings::{self, TableColor, selection_color};
use crate::graphics::draw_list::Color;
use crate::screens::rulebook_viewer::RulebookViewer;
use crate::sound::SoundEffect;
use crate::table::actions::{TableAction, TableBackend};
use crate::table::board_layout::{
    BoardOnTable, BoardToggle, board_at, board_size, board_toggle, default_player_board, layout_boards,
};
use crate::table::board_zones::{
    PlacedZone, ZoneOnTable, placed_zone_fraction, placed_zones_by_owner, scenario_zones_on_table,
};
use crate::table::card_render::{CARD_HEIGHT, CARD_WIDTH};
use crate::table::menus::{TableDialog, TableMenu};
use crate::table::selection::half_extents;
use crate::table::view::{DropZone, HitItem, ItemKind};
use crate::ui::card_filter_bar::CardFilterState;
use crate::ui::components::argb_color;
use crate::ui::context::Rect;

pub(super) const DRAG_THRESHOLD: f32 = 4.0;
pub(super) const PREVIEW_THROTTLE: f64 = 0.05;
const PAN_SPEED: f32 = 600.0;
pub(super) const MIN_ZOOM: f32 = 0.5;
pub(super) const MAX_ZOOM: f32 = 2.5;
const CHAT_ANIMATION_RATE: f32 = 14.0;
const LOG_ANIMATION_RATE: f32 = 14.0;
/// A dropped card stacks onto a pile top within this many world units (with Alt).
pub(super) const STACK_HIT_RADIUS: f64 = CARD_WIDTH as f64 * 0.6;

/// What the screen hosting the table should do (the original's `onLeaveGame`, `onSaveGame` and
/// `onKickPlayer` callbacks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableEvent {
    LeaveGame,
    SaveGame,
    KickPlayer(String),
}

/// How the table is set up (`TableScreen.new(opts)`).
pub struct TableOptions {
    pub backend: Box<dyn TableBackend>,
    /// The viewer sits on the far side, so the table is shown turned around.
    pub is_mirrored: bool,
    pub local_avatar_path: Option<String>,
    /// Other players' avatars: player id -> image cache key.
    pub avatar_keys: HashMap<String, String>,
    pub leave_button_label: String,
    pub leave_confirmation_message: String,
    /// The Game Menu offers "Save Game..." (reported as [`TableEvent::SaveGame`]).
    pub can_save_game: bool,
    /// The Game Menu lists players to kick (reported as [`TableEvent::KickPlayer`]).
    pub can_kick_players: bool,
    /// The host/owner: Load Scenario and the shared playmat.
    pub can_load_scenario: bool,
    /// Collection-mode server games hide the Pack Generator.
    pub collection_mode: bool,
    /// Watch only: no drags, menus or card hotkeys.
    pub spectator: bool,
    /// Where Load Deck / Save Deck go; the local deck library when `None`.
    pub deck_store: Option<Box<dyn DeckStore>>,
}

/// A card or a loose card token that can be lifted together with Alt (`pickupCandidates`).
#[derive(Debug, Clone, PartialEq)]
pub enum PickupItem {
    Card(CardInstance),
    CardToken(BoardWidgetInstance),
}

impl PickupItem {
    pub fn instance_id(&self) -> &str {
        match self {
            PickupItem::Card(card) => &card.instance_id,
            PickupItem::CardToken(widget) => &widget.instance_id,
        }
    }

    pub fn position(&self) -> (f64, f64) {
        match self {
            PickupItem::Card(card) => (card.x, card.y),
            PickupItem::CardToken(widget) => (widget.x, widget.y),
        }
    }

    pub fn z_index(&self) -> i64 {
        match self {
            PickupItem::Card(card) => card.z_index,
            PickupItem::CardToken(widget) => widget.z_index,
        }
    }
}

/// The key used for shared zones and widgets in the per-owner maps.
pub(super) use crate::table::board_layout::SHARED_OWNER_KEY;

/// Data recomputed from the session each frame (`T:derive()`).
#[derive(Default)]
pub(super) struct Derived {
    pub owned_zones: Vec<ZoneDefinition>,
    pub shared_zones: Vec<ZoneDefinition>,
    /// Owner id (or "") -> cards in hand.
    pub hand_count: HashMap<String, usize>,
    /// (owner id or `__shared`, zone id) -> cards in that zone, in state order.
    pub zone_cards: HashMap<(String, String), Vec<CardInstance>>,
    /// Zone id -> cards (any owner), in state order.
    pub cards_by_zone_id: HashMap<String, Vec<CardInstance>>,
    pub cards_by_id: HashMap<String, CardInstance>,
    /// (owner id or `__shared`, zone id) -> the widget docked there.
    pub widget_for: HashMap<(String, String), BoardWidgetInstance>,
    /// The acting player's hand, by z-index.
    pub local_hand: Vec<CardInstance>,
    /// Pile root id -> the pile's cards in state order.
    pub pile_groups: HashMap<String, Vec<CardInstance>>,
    /// Each pile's top card, in first-seen pile order.
    pub tops: Vec<CardInstance>,
    /// Pile roots by their top card's z-index (then root id).
    pub sorted_roots: Vec<String>,
    pub pickup_candidates: Vec<PickupItem>,
    pub pile_searched: HashSet<String>,
    pub top_row: Vec<PlayerInfo>,
    pub bottom_row: Vec<PlayerInfo>,
    /// Owner id (or `__shared` for shared boards) -> the zones on that owner's shown boards
    /// right now. These are drawn on the boards instead of in the player area or a sidebar.
    pub placed_zones: HashMap<String, Vec<PlacedZone>>,
    /// The loaded scenarios' zones set at fixed spots on the table (drawn there, never on a board
    /// or in a sidebar).
    pub zones_on_table: Vec<ZoneOnTable>,
}

impl Derived {
    /// Whether this owner's zone is drawn on one of their boards (shared boards for `None`).
    pub fn is_zone_on_board(&self, owner_id: Option<&str>, zone_id: &str) -> bool {
        let owner_key = owner_id.unwrap_or(SHARED_OWNER_KEY);
        self.placed_zones.get(owner_key).is_some_and(|zones| zones.iter().any(|placed| placed.zone.id == zone_id))
    }

    /// Whether a scenario sets this shared zone at a fixed spot on the table.
    pub fn is_zone_set_on_table(&self, zone_id: &str) -> bool {
        self.zones_on_table.iter().any(|on_table| on_table.zone.id == zone_id)
    }
}

/// The highest z-index card of a pile (ties: the earlier card), as `stacks.topOf`.
pub(super) fn top_card(cards: &[CardInstance]) -> Option<&CardInstance> {
    let mut best = cards.first()?;
    for card in &cards[1..] {
        if best.z_index < card.z_index {
            best = card;
        }
    }
    Some(best)
}

/// A rotation easing toward its target (`T.rotAnim[key]`).
#[derive(Debug, Clone, Copy)]
pub(super) struct RotationAnimation {
    pub from: f32,
    pub to: f32,
    pub started_at: f64,
}

/// A card flying to the discard pile after X (`T.flight`).
#[derive(Debug, Clone)]
pub(super) struct DiscardFlight {
    pub instance_id: String,
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub face_up: bool,
    pub definition_id: String,
    pub started_at: f64,
}

/// A line of the table chat.
#[derive(Debug, Clone)]
pub struct ChatLine {
    pub time_label: String,
    pub name: String,
    pub text: String,
}

/// A server room's chat, relayed by the table's chat panel (`T.sc`'s room chat).
pub trait RoomChat {
    fn lines(&self) -> Vec<ChatLine>;
    fn send(&mut self, text: &str);
}

/// Box-selected cards (whole piles) and tokens.
#[derive(Debug, Clone, Default)]
pub(super) struct TableSelection {
    pub cards: HashSet<String>,
    pub tokens: HashSet<String>,
}

pub struct TableScreen {
    pub(super) backend: Box<dyn TableBackend>,
    pub(super) game: Rc<GameDefinition>,
    /// This frame's copy of the session state.
    pub(super) state: TableState,
    pub(super) acting_player_id: String,
    pub(super) local_player_id: String,
    pub(super) is_local_practice: bool,
    pub(super) card_drag_previews: HashMap<String, CardDragPreview>,
    pub(super) arrow_drag_previews: HashMap<String, ArrowDragPreview>,
    /// Set when this frame's input changed the table, so the frame re-reads the session before
    /// drawing (otherwise a dropped card shows at its old spot for one frame).
    pub(super) table_changed_by_input: bool,
    pub(super) derived: Derived,

    pub(super) is_mirrored: bool,
    pub(super) local_avatar_path: Option<String>,
    pub(super) avatar_keys: HashMap<String, String>,
    pub(super) leave_button_label: String,
    pub(super) leave_confirmation_message: String,
    pub(super) can_save_game: bool,
    pub(super) can_kick_players: bool,
    pub(super) can_load_scenario: bool,
    pub(super) collection_mode: bool,
    pub(super) spectator: bool,
    pub(super) deck_store: Option<Box<dyn DeckStore>>,
    pub(super) events: Vec<TableEvent>,

    // Camera.
    pub(super) zoom: f32,
    pub(super) camera_x: f32,
    pub(super) camera_y: f32,
    pub(super) table_rect: Option<Rect>,
    pub(super) layout: crate::table::view::LayoutRects,

    // Hit items recorded while drawing; last frame's drive this frame's input.
    pub(super) items: Vec<HitItem>,
    pub(super) previous_items: Vec<HitItem>,
    pub(super) drop_zones: Vec<DropZone>,
    pub(super) bag_drop_zones: Vec<DropZone>,
    /// Zones on playmats, checked after the docked ones and only over the table.
    pub(super) playmat_drop_zones: Vec<DropZone>,
    pub(super) playmat_bag_drop_zones: Vec<DropZone>,
    pub(super) hand_rect: Option<Rect>,
    pub(super) hand_card_centers: Vec<(String, f32)>,
    pub(super) hand_lift: HashMap<String, f32>,
    pub(super) draw_mirror: Option<(f32, f32)>,

    // Animations.
    pub(super) rotation_animations: HashMap<String, RotationAnimation>,
    pub(super) wiggles: HashMap<String, f64>,
    pub(super) flight: Option<DiscardFlight>,
    pub(super) last_dt: f32,

    // Toggles and held keys.
    pub(super) borders_hidden: bool,
    pub(super) color_tint_enabled: bool,
    pub(super) tab_down: bool,
    pub(super) space_down: bool,
    pub(super) alt_down: bool,

    // Panels.
    pub(super) game_menu_open: bool,
    pub(super) log_open: bool,
    pub(super) log_animation: f32,
    pub(super) log_scroll_to_end: bool,
    pub(super) library_open: bool,
    pub(super) token_picker_open: bool,
    pub(super) token_picker_type: String,
    pub(super) library: CardFilterState,
    pub(super) library_scenario: bool,
    pub(super) library_hover_definition: Option<String>,
    pub(super) chat_open: bool,
    pub(super) chat_animation: f32,
    pub(super) chat_text: String,
    pub(super) seen_chat: usize,
    pub(super) local_chat: Vec<ChatLine>,
    /// Server games chat through the room instead of the practice log.
    pub(super) room_chat: Option<Box<dyn RoomChat>>,
    pub(super) rulebook: RulebookViewer,

    // Selection.
    pub(super) selection: TableSelection,
    pub(super) selected_roots: HashSet<String>,
    pub(super) selected_count: usize,

    // Input state (see input.rs).
    pub(super) hover_item: Option<HitItem>,
    pub(super) hovered_id: Option<String>,
    pub(super) hovered_force_face_up: bool,
    pub(super) overlay_hover_id: Option<String>,
    pub(super) last_preview_at: f64,
    pub(super) drag: Option<crate::table::input::Drag>,
    pub(super) pending: Option<crate::table::input::PendingPress>,
    pub(super) box_select: Option<crate::table::input::BoxSelect>,
    pub(super) zone_move: Option<crate::table::input::ZoneMoveDrag>,
    /// One of this player's boards being moved by its handle.
    pub(super) board_drag: Option<crate::table::board_handle::BoardDrag>,
    /// This player's boards whose handle shows ("Unlock Board"), by 1-based index.
    pub(super) unlocked_boards: HashSet<i64>,
    /// The boards drawn last frame (for the handles and the table menu).
    pub(super) boards_shown: Vec<BoardOnTable>,
    pub(super) arrow_drag: Option<crate::table::input::ArrowDrag>,
    pub(super) middle_pan: Option<crate::table::input::MiddlePan>,
    pub(super) click_item: Option<HitItem>,
    pub(super) right_press_item: Option<HitItem>,

    // Menus and dialogs this table owns.
    pub(super) menu: Option<TableMenu>,
    pub(super) dialog: Option<TableDialog>,
    /// Shown above `dialog` (e.g. a token chooser over Change Tokens).
    pub(super) inner_dialog: Option<TableDialog>,
    /// Development hook: open Load Deck for the first built-deck zone on the next frame.
    pub(super) development_load_deck: bool,
}

impl TableScreen {
    pub fn new(options: TableOptions) -> TableScreen {
        let game = Rc::new(options.backend.session().game.clone());
        let library = CardFilterState::new(&game);
        let rulebook = RulebookViewer::new(&game);
        let mut table = TableScreen {
            backend: options.backend,
            game,
            state: TableState::default(),
            acting_player_id: String::new(),
            local_player_id: String::new(),
            is_local_practice: false,
            card_drag_previews: HashMap::new(),
            arrow_drag_previews: HashMap::new(),
            table_changed_by_input: false,
            derived: Derived::default(),
            is_mirrored: options.is_mirrored,
            local_avatar_path: options.local_avatar_path,
            avatar_keys: options.avatar_keys,
            leave_button_label: options.leave_button_label,
            leave_confirmation_message: options.leave_confirmation_message,
            can_save_game: options.can_save_game,
            can_kick_players: options.can_kick_players,
            can_load_scenario: options.can_load_scenario,
            collection_mode: options.collection_mode,
            spectator: options.spectator,
            deck_store: options.deck_store,
            events: Vec::new(),
            zoom: 1.0,
            camera_x: 0.0,
            camera_y: 0.0,
            table_rect: None,
            layout: Default::default(),
            items: Vec::new(),
            previous_items: Vec::new(),
            drop_zones: Vec::new(),
            bag_drop_zones: Vec::new(),
            playmat_drop_zones: Vec::new(),
            playmat_bag_drop_zones: Vec::new(),
            hand_rect: None,
            hand_card_centers: Vec::new(),
            hand_lift: HashMap::new(),
            draw_mirror: None,
            rotation_animations: HashMap::new(),
            wiggles: HashMap::new(),
            flight: None,
            last_dt: 0.0,
            borders_hidden: false,
            color_tint_enabled: true,
            tab_down: false,
            space_down: false,
            alt_down: false,
            game_menu_open: false,
            log_open: false,
            log_animation: 0.0,
            log_scroll_to_end: false,
            library_open: false,
            token_picker_open: false,
            token_picker_type: "all".to_string(),
            library,
            library_scenario: false,
            library_hover_definition: None,
            chat_open: false,
            chat_animation: 0.0,
            chat_text: String::new(),
            seen_chat: 0,
            local_chat: Vec::new(),
            room_chat: None,
            rulebook,
            selection: TableSelection::default(),
            selected_roots: HashSet::new(),
            selected_count: 0,
            hover_item: None,
            hovered_id: None,
            hovered_force_face_up: false,
            overlay_hover_id: None,
            last_preview_at: 0.0,
            drag: None,
            pending: None,
            box_select: None,
            zone_move: None,
            board_drag: None,
            unlocked_boards: HashSet::new(),
            boards_shown: Vec::new(),
            arrow_drag: None,
            middle_pan: None,
            click_item: None,
            right_press_item: None,
            menu: None,
            dialog: None,
            inner_dialog: None,
            development_load_deck: false,
        };
        table.snapshot_session();
        table
    }

    /// Connected players and seats, for Discord presence (`publishPresence`'s count).
    pub fn player_counts(&self) -> (usize, usize) {
        let players = &self.backend.session().state().players;
        (players.iter().filter(|player| player.connected).count(), players.len())
    }

    /// The session behind the table (e.g. to save the game).
    pub fn backend(&self) -> &dyn TableBackend {
        self.backend.as_ref()
    }

    pub fn backend_mut(&mut self) -> &mut dyn TableBackend {
        self.backend.as_mut()
    }

    pub fn game(&self) -> &GameDefinition {
        &self.game
    }

    /// Development hook (`GAMETABLE_TABLE_SETUP=draw,play,menu,library,log,tokens,chat`): draws
    /// five cards from the first built-deck zone, plays three to the table, and opens panels.
    pub fn apply_development_setup(&mut self, steps: &str) {
        self.snapshot_session();
        for step in steps.split(',') {
            match step.trim() {
                "draw" => {
                    if let Some(zone) = self.game.zones.iter().find(|zone| zone.deals_built_deck && !zone.shared) {
                        let zone_id = zone.id.clone();
                        self.perform(TableAction::DrawFromZone { zone_id, count: 5 });
                    }
                }
                "play" => {
                    self.snapshot_session();
                    let acting = self.acting_player_id.clone();
                    let hand: Vec<CardInstance> = self
                        .state
                        .cards()
                        .iter()
                        .filter(|card| card.zone == CardZone::Hand && card.owner_id.as_deref() == Some(acting.as_str()))
                        .take(3)
                        .cloned()
                        .collect();
                    for (index, card) in hand.into_iter().enumerate() {
                        let x = 0.4 + 0.1 * index as f64;
                        self.perform(TableAction::MoveCard { instance_id: card.instance_id, x, y: 0.55 });
                    }
                }
                "menu" => self.game_menu_open = true,
                "library" => self.library_open = true,
                "log" => {
                    self.log_open = true;
                    self.log_animation = 1.0;
                }
                "tokens" => self.token_picker_open = true,
                // Every seat's boards (and the shared ones) shown: placed toggleable boards, or with
                // nothing placed each seat's first player board (or the first shared board).
                "boards" | "playmats" => {
                    let session = self.backend.session_mut();
                    session.set_shared_playmat(true, 0);
                    let player_ids: Vec<String> =
                        session.state().players.iter().map(|player| player.id.clone()).collect();
                    for player_id in player_ids {
                        session.set_individual_playmat(&player_id, true, 0);
                    }
                }
                "rulebook" => self.rulebook.open_for_development(),
                "loaddeck" => self.development_load_deck = true,
                "chat" => {
                    self.chat_open = true;
                    self.chat_animation = 1.0;
                }
                _ => {}
            }
        }
        self.snapshot_session();
    }

    /// A joined player's view turns around when their seat is on the far side.
    pub fn set_mirrored(&mut self, is_mirrored: bool) {
        self.is_mirrored = is_mirrored;
    }

    /// Other players' avatars: player id -> image cache key.
    pub fn set_avatar_keys(&mut self, avatar_keys: HashMap<String, String>) {
        self.avatar_keys = avatar_keys;
    }

    /// The same game resent by the host (e.g. after a reconnect): refresh the definitions.
    pub fn replace_game(&mut self, game: GameDefinition) {
        self.backend.session_mut().game = game.clone();
        self.rulebook.set_game(&game);
        self.game = Rc::new(game);
    }

    /// Where the open context menu shows an item, by label (for scripted UI checks).
    pub fn menu_item_rect(&self, label: &str) -> Option<crate::ui::context::Rect> {
        self.menu.as_ref().and_then(|menu| menu.menu.item_rect(label))
    }

    /// Last frame's hit items as (original kind name, centre), front-most last (for scripted
    /// UI checks).
    pub fn hit_item_centers(&self) -> Vec<(&'static str, f32, f32)> {
        self.previous_items.iter().map(|item| (item.kind.name(), item.shape.center_x, item.shape.center_y)).collect()
    }

    /// Whether a table dialog (set value, colours, confirm give, ...) is showing.
    pub fn is_dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    /// What happened since the last call (leave, save, kick).
    pub fn take_events(&mut self) -> Vec<TableEvent> {
        std::mem::take(&mut self.events)
    }

    pub(super) fn perform(&mut self, action: TableAction) {
        // Drag previews go out every frame of a drag and don't change what this player sees.
        let is_drag_preview = matches!(
            action,
            TableAction::PreviewCardDrag { .. }
                | TableAction::EndCardDragPreview
                | TableAction::PreviewArrowDrag { .. }
                | TableAction::EndArrowDragPreview
        );
        if !is_drag_preview {
            self.table_changed_by_input = true;
        }
        self.backend.perform(action);
    }

    pub(super) fn definition(&self, definition_id: &str) -> Option<&CardDefinition> {
        self.game.card_by_id(definition_id)
    }

    /// Copies what the frame needs from the session.
    pub(super) fn snapshot_session(&mut self) {
        let session = self.backend.session();
        self.state = session.state().clone();
        self.acting_player_id = session.acting_player_id().to_string();
        self.local_player_id = session.local_player_id.clone();
        self.is_local_practice = session.is_local_practice;
        self.card_drag_previews = session.card_drag_previews.clone();
        self.arrow_drag_previews = session.arrow_drag_previews.clone();
    }

    pub(super) fn effective_zones(&self) -> impl Iterator<Item = &ZoneDefinition> {
        self.game.zones.iter().chain(self.state.loaded_zones.iter())
    }

    // -----------------------------------------------------------------------
    // Derived state
    // -----------------------------------------------------------------------

    /// `T:derive()`.
    pub(super) fn derive(&mut self) {
        let acting = self.acting_player_id.clone();
        let mut derived = Derived::default();
        for zone in self.effective_zones() {
            if zone.shared {
                derived.shared_zones.push(zone.clone());
            } else {
                derived.owned_zones.push(zone.clone());
            }
        }
        let mut table_cards = Vec::new();
        let mut hand = Vec::new();
        for card in self.state.cards() {
            derived.cards_by_id.insert(card.instance_id.clone(), card.clone());
            match card.zone {
                CardZone::Hand => {
                    *derived.hand_count.entry(card.owner_id.clone().unwrap_or_default()).or_insert(0) += 1;
                    if card.owner_id.as_deref() == Some(acting.as_str()) {
                        hand.push(card.clone());
                    }
                }
                CardZone::Zone => {
                    let owner_key = card.owner_id.clone().unwrap_or_else(|| SHARED_OWNER_KEY.to_string());
                    let zone_id = card.zone_id.clone().unwrap_or_default();
                    derived.zone_cards.entry((owner_key, zone_id.clone())).or_default().push(card.clone());
                    derived.cards_by_zone_id.entry(zone_id).or_default().push(card.clone());
                }
                CardZone::Table => table_cards.push(card.clone()),
            }
        }
        hand.sort_by_key(|card| card.z_index);
        derived.local_hand = hand;
        for widget in self.state.widgets() {
            if let Some(zone_id) = &widget.zone_id {
                let owner_key = widget.owner_id.clone().unwrap_or_else(|| SHARED_OWNER_KEY.to_string());
                derived.widget_for.insert((owner_key, zone_id.clone()), widget.clone());
            }
        }
        let grouped = group_by_stack(&table_cards);
        let mut roots: Vec<String> = Vec::new();
        for root_id in &grouped.root_ids_in_order {
            let cards: Vec<CardInstance> =
                grouped.cards_by_root_id[root_id].iter().map(|card| (*card).clone()).collect();
            if let Some(top) = top_card(&cards) {
                derived.tops.push(top.clone());
            }
            derived.pile_groups.insert(root_id.to_string(), cards);
            roots.push(root_id.to_string());
        }
        let top_z: HashMap<String, i64> = roots
            .iter()
            .map(|root| (root.clone(), top_card(&derived.pile_groups[root]).map_or(0, |card| card.z_index)))
            .collect();
        roots.sort_by(|first, second| top_z[first].cmp(&top_z[second]).then_with(|| first.cmp(second)));
        derived.sorted_roots = roots;
        derived.pickup_candidates = derived
            .tops
            .iter()
            .filter(|card| card.owner_id.as_deref().is_none_or(|owner| owner == acting))
            .cloned()
            .map(PickupItem::Card)
            .collect();
        // A loose card token counts as a one-card pile for group drags.
        for widget in self.state.widgets() {
            if widget.kind == BoardWidgetKind::CardToken
                && widget.owner_id.as_deref().is_none_or(|owner| owner == acting)
            {
                derived.pickup_candidates.push(PickupItem::CardToken(widget.clone()));
            }
        }
        for search in &self.state.searches {
            if search.target_type == SearchTargetType::Pile {
                derived.pile_searched.insert(search.target_id.clone());
            }
        }
        let rows = compute_hand_row_layout(&self.state.players, &self.local_player_id);
        derived.top_row = rows.top_row.into_iter().cloned().collect();
        derived.bottom_row = rows.bottom_row.into_iter().cloned().collect();
        derived.zones_on_table = scenario_zones_on_table(&self.game, &self.state);
        derived.placed_zones = placed_zones_by_owner(&self.game, &self.state);
        // A zone set on the table stays there, even if a shared board also holds it.
        for zones in derived.placed_zones.values_mut() {
            zones.retain(|placed| !derived.zones_on_table.iter().any(|on_table| on_table.zone.id == placed.zone.id));
        }
        self.derived = derived;
    }

    pub(super) fn is_zone_searched(&self, zone_id: &str, owner_id: Option<&str>) -> bool {
        self.state.searches.iter().any(|search| {
            search.target_type == SearchTargetType::Zone
                && search.target_id == zone_id
                && search.target_owner_id.as_deref() == owner_id
        })
    }

    /// Whether a card owned by `owner_id` is shown turned toward its owner across the table.
    pub(super) fn card_faces_away_from_me(&self, owner_id: Option<&str>) -> bool {
        let Some(owner_id) = owner_id else { return false };
        if self.game.same_orientation_for_all {
            return false;
        }
        let players = &self.state.players;
        let owner_seat = players.iter().position(|player| player.id == owner_id);
        let my_seat = players.iter().position(|player| player.id == self.local_player_id);
        match (owner_seat, my_seat) {
            (Some(owner_seat), Some(my_seat)) => {
                is_far_mirror_seat(owner_seat, players.len()) != is_far_mirror_seat(my_seat, players.len())
            }
            _ => owner_id != self.local_player_id,
        }
    }

    /// An unowned card goes to a shared discard pile; an owned one prefers its owner's own.
    pub(super) fn local_discard_zone_id(&self, card_owner_id: Option<&str>) -> Option<String> {
        let mut shared_id: Option<String> = None;
        for zone in self.effective_zones().filter(|zone| zone.is_discard_pile) {
            if zone.shared {
                shared_id.get_or_insert_with(|| zone.id.clone());
            } else if card_owner_id.is_some() {
                return Some(zone.id.clone());
            }
        }
        shared_id
    }

    pub(super) fn zone_by_id(&self, zone_id: &str) -> Option<ZoneDefinition> {
        self.effective_zones().find(|zone| zone.id == zone_id).cloned()
    }

    /// Shared zones' top card can always be flipped; an owned zone's only when Flippable.
    pub(super) fn zone_allows_flip(&self, zone_id: &str) -> bool {
        self.zone_by_id(zone_id).is_some_and(|zone| zone.shared || zone.flippable)
    }

    pub(super) fn player_by_id(&self, player_id: &str) -> Option<&PlayerInfo> {
        self.state.players.iter().find(|player| player.id == player_id)
    }

    pub(super) fn widget_by_id(&self, instance_id: &str) -> Option<&BoardWidgetInstance> {
        self.state.widgets().iter().find(|widget| widget.instance_id == instance_id)
    }

    pub(super) fn root_id_of(&self, card: &CardInstance) -> String {
        root_id_of(self.state.cards(), card).to_string()
    }

    // -----------------------------------------------------------------------
    // Colours
    // -----------------------------------------------------------------------

    pub(super) fn selection_color(&self, frame: &Frame) -> Color {
        argb_color(selection_color(frame.settings))
    }

    pub(super) fn selection_tint(&self, frame: &Frame) -> Color {
        Color { alpha: 0.35, ..self.selection_color(frame) }
    }

    pub(super) fn felt_color(&self, frame: &Frame) -> Color {
        argb_color(TableColor::Felt.resolve(frame.settings, self.game.table_color.as_deref()))
    }

    pub(super) fn playmat_border_color(&self, frame: &Frame) -> Color {
        argb_color(TableColor::PlaymatBorder.resolve(frame.settings, self.game.playmat_border_color.as_deref()))
    }

    pub(super) fn current_turn_color(&self, frame: &Frame) -> Color {
        argb_color(TableColor::CurrentTurn.resolve(frame.settings, self.game.current_turn_color.as_deref()))
    }

    /// A player's colour for card borders (`V.ownerBorderColor`), unless borders are hidden.
    pub(super) fn owner_border_color(&self, owner_id: Option<&str>) -> Option<Color> {
        if self.borders_hidden {
            return None;
        }
        self.player_by_id(owner_id?).map(|player| argb_color(player.color))
    }

    // -----------------------------------------------------------------------
    // Selection
    // -----------------------------------------------------------------------

    pub(super) fn is_root_selected(&self, root_id: &str) -> bool {
        self.selected_roots.contains(root_id)
    }

    pub(super) fn is_token_selected(&self, instance_id: &str) -> bool {
        self.selection.tokens.contains(instance_id)
    }

    pub(super) fn clear_selection(&mut self) {
        self.selection = TableSelection::default();
        self.selected_roots.clear();
        self.selected_count = 0;
    }

    /// A token that can be selected: yours and loose on the table.
    pub(super) fn is_selectable_token(&self, widget: Option<&BoardWidgetInstance>) -> bool {
        widget.is_some_and(|widget| {
            matches!(widget.kind, BoardWidgetKind::Token | BoardWidgetKind::ImageToken | BoardWidgetKind::CardToken)
                && widget.zone_id.is_none()
                && widget.attached_card_id.is_none()
                && widget.owner_id.as_deref() == Some(self.acting_player_id.as_str())
        })
    }

    /// `T:refreshSelection()`: drops what left the table or changed hands, then works out which
    /// piles are selected (a pile counts when any card is selected and its top card is yours).
    pub(super) fn refresh_selection(&mut self) {
        let acting = self.acting_player_id.clone();
        let cards_by_id = &self.derived.cards_by_id;
        self.selection.cards.retain(|id| {
            cards_by_id.get(id).is_some_and(|card| {
                card.zone == CardZone::Table && card.owner_id.as_deref().is_none_or(|owner| owner == acting)
            })
        });
        let selectable: HashSet<String> = self
            .selection
            .tokens
            .iter()
            .filter(|id| self.is_selectable_token(self.widget_by_id(id)))
            .cloned()
            .collect();
        self.selection.tokens = selectable;
        let mut roots = HashSet::new();
        for (root_id, cards) in &self.derived.pile_groups {
            let top_owner = top_card(cards).and_then(|card| card.owner_id.as_deref());
            if top_owner.is_none_or(|owner| owner == acting)
                && cards.iter().any(|card| self.selection.cards.contains(&card.instance_id))
            {
                roots.insert(root_id.clone());
            }
        }
        self.selected_count = roots.len() + self.selection.tokens.len();
        self.selected_roots = roots;
    }

    /// Selected pile roots, bottom-most first.
    pub(super) fn selected_root_list(&self) -> Vec<String> {
        self.derived.sorted_roots.iter().filter(|root| self.selected_roots.contains(*root)).cloned().collect()
    }

    /// Every card of the selected piles (opponents' left out), by z-index.
    pub(super) fn selected_cards(&self) -> Vec<CardInstance> {
        let acting = self.acting_player_id.as_str();
        let mut cards: Vec<CardInstance> = self
            .selected_root_list()
            .iter()
            .flat_map(|root| self.derived.pile_groups[root].iter())
            .filter(|card| card.owner_id.as_deref().is_none_or(|owner| owner == acting))
            .cloned()
            .collect();
        cards.sort_by_key(|card| card.z_index);
        cards
    }

    pub(super) fn selected_token_ids(&self) -> Vec<String> {
        self.state
            .widgets()
            .iter()
            .filter(|widget| self.selection.tokens.contains(&widget.instance_id))
            .map(|widget| widget.instance_id.clone())
            .collect()
    }

    /// Ctrl+click: a whole pile (or a token) in or out of the selection.
    pub(super) fn toggle_selected(&mut self, item: &HitItem) {
        match &item.kind {
            ItemKind::Token { widget } => {
                if !self.selection.tokens.remove(&widget.instance_id) {
                    self.selection.tokens.insert(widget.instance_id.clone());
                }
            }
            ItemKind::TableCard { root_id, .. } | ItemKind::Pile { root_id, .. } => {
                let select = !self.selected_roots.contains(root_id);
                for card in self.derived.pile_groups.get(root_id).into_iter().flatten() {
                    if select {
                        self.selection.cards.insert(card.instance_id.clone());
                    } else {
                        self.selection.cards.remove(&card.instance_id);
                    }
                }
            }
            _ => {}
        }
        self.refresh_selection();
    }

    /// Half extents (world units) of a table card as it's currently shown.
    pub(super) fn card_half_extents(&self, card: &CardInstance) -> (f32, f32) {
        half_extents(CARD_WIDTH, CARD_HEIGHT, self.card_turns(card, false))
    }

    // -----------------------------------------------------------------------
    // Game boards (F12 shows and hides the toggleable ones)
    // -----------------------------------------------------------------------

    /// A board's image (boards live in the game's `_playmats/` folder, named from before boards
    /// were renamed).
    pub(super) fn board_image_path(&self, board_index: i64) -> Option<String> {
        let folder = self.game.folder_path.as_deref()?;
        let board = board_at(&self.game, board_index)?;
        Some(crate::data::file_system::join_path(&[folder, "_playmats", &board.image_path]))
    }

    /// A board's name for menus: its image's file name without the extension.
    pub(super) fn board_name(&self, board_index: i64) -> String {
        let Some(board) = board_at(&self.game, board_index) else { return format!("Board {board_index}") };
        let file_name = board.image_path.rsplit(['/', '\\']).next().unwrap_or(&board.image_path);
        file_name.rsplit_once('.').map_or(file_name, |(stem, _)| stem).to_string()
    }

    /// Every board's unturned size on the table (its image's size once loaded, times its scale).
    pub(super) fn board_sizes(&self, frame: &mut Frame) -> Vec<(f32, f32)> {
        (1..=self.game.game_boards.len() as i64)
            .map(|board_index| {
                let image_size = self
                    .board_image_path(board_index)
                    .and_then(|path| frame.images.get(&path))
                    .map(|texture| (texture.width, texture.height));
                board_size(&self.game.game_boards[(board_index - 1) as usize], image_size)
            })
            .collect()
    }

    /// The boards on the table this frame, laid out in this viewer's world (a board being moved by
    /// its handle where it is being dragged).
    pub(super) fn boards_on_table(&self, frame: &mut Frame) -> Vec<BoardOnTable> {
        let sizes = self.board_sizes(frame);
        self.with_dragged_board(layout_boards(
            &self.game,
            &self.state,
            &self.derived.top_row,
            &self.derived.bottom_row,
            self.is_mirrored,
            &sizes,
        ))
    }

    /// `T:seedPlaymat()`: once the table is shown, puts this player's boards as they last left
    /// them in this game (placed toggleable boards start shown; with nothing placed, boards start
    /// hidden), and sends where they moved zones on them.
    pub(super) fn seed_boards(&mut self, frame: &Frame) {
        let game_id = self.game.id.clone();
        let chosen = display_settings::chosen_board(frame.settings, &game_id);
        let is_seated = self.player_by_id(&self.acting_player_id).is_some();
        match board_toggle(&self.game) {
            BoardToggle::Nothing => {}
            BoardToggle::PlacedBoards { player_boards, shared_boards } => {
                let shown = display_settings::boards_shown(frame.settings, &game_id, true);
                if player_boards && is_seated {
                    self.perform(TableAction::SetIndividualPlaymat { on: shown, index: chosen });
                    // In practice every other seat is played here too (nobody else seeds them), so
                    // their toggleable boards start shown as well.
                    if self.is_local_practice {
                        let others: Vec<String> = self
                            .state
                            .players
                            .iter()
                            .filter(|player| player.id != self.acting_player_id)
                            .map(|player| player.id.clone())
                            .collect();
                        let session = self.backend.session_mut();
                        for player_id in others {
                            session.set_individual_playmat(&player_id, true, 0);
                        }
                    }
                }
                if shared_boards && self.can_load_scenario {
                    self.perform(TableAction::SetSharedPlaymat { on: shown, index: self.state.chosen_shared_board });
                }
            }
            BoardToggle::DefaultPlayerBoard => {
                let shown = display_settings::boards_shown(frame.settings, &game_id, false);
                if is_seated && (shown || chosen != 0) {
                    self.perform(TableAction::SetIndividualPlaymat { on: shown, index: chosen });
                }
            }
            BoardToggle::DefaultSharedBoard => {
                let shown = display_settings::boards_shown(frame.settings, &game_id, false);
                if self.can_load_scenario && (shown || chosen != 0) {
                    self.perform(TableAction::SetSharedPlaymat { on: shown, index: chosen });
                }
            }
        }
        self.send_remembered_zone_positions(frame);
    }

    /// F12: shows or hides this player's toggleable boards (and, for the host, the toggleable
    /// shared ones). With no board placed it shows or hides the player's chosen player board (or
    /// the host's shared board, in a game with only shared boards). Boards that aren't toggleable
    /// always stay.
    pub(super) fn toggle_boards(&mut self, frame: &mut Frame) {
        let me = self.player_by_id(&self.acting_player_id).cloned();
        let is_host = self.can_load_scenario;
        let (mine, shared) = match board_toggle(&self.game) {
            BoardToggle::Nothing => return,
            BoardToggle::PlacedBoards { player_boards, shared_boards } => (player_boards, shared_boards),
            BoardToggle::DefaultPlayerBoard => (true, false),
            BoardToggle::DefaultSharedBoard => (false, true),
        };
        let toggles_mine = mine && me.is_some();
        let toggles_shared = shared && is_host;
        if !toggles_mine && !toggles_shared {
            return;
        }
        let currently_shown = (toggles_mine && me.as_ref().is_some_and(|me| me.player_boards_shown))
            || (toggles_shared && self.state.shared_boards_shown);
        let shown = !currently_shown;
        let mut chosen = 0;
        if toggles_mine && let Some(me) = &me {
            chosen = me.chosen_player_board;
            self.perform(TableAction::SetIndividualPlaymat { on: shown, index: chosen });
        }
        if toggles_shared {
            if !toggles_mine {
                chosen = self.state.chosen_shared_board;
            }
            self.perform(TableAction::SetSharedPlaymat { on: shown, index: self.state.chosen_shared_board });
        }
        let game_id = self.game.id.clone();
        display_settings::set_boards_shown(frame.settings, &game_id, shown, chosen);
    }

    /// Whether F12 does anything for this player, and whether their boards are showing (for the
    /// table menu's Show/Hide Boards).
    pub(super) fn board_toggle_state(&self) -> Option<bool> {
        let me = self.player_by_id(&self.acting_player_id);
        let is_host = self.can_load_scenario;
        let (mine, shared) = match board_toggle(&self.game) {
            BoardToggle::Nothing => return None,
            BoardToggle::PlacedBoards { player_boards, shared_boards } => (player_boards, shared_boards),
            BoardToggle::DefaultPlayerBoard => (true, false),
            BoardToggle::DefaultSharedBoard => (false, true),
        };
        let toggles_mine = mine && me.is_some();
        let toggles_shared = shared && is_host;
        if !toggles_mine && !toggles_shared {
            return None;
        }
        Some(
            (toggles_mine && me.is_some_and(|me| me.player_boards_shown))
                || (toggles_shared && self.state.shared_boards_shown),
        )
    }

    /// The player board this player uses while none is placed (picked from the table menu).
    pub(super) fn current_player_board(&self) -> Option<i64> {
        let me = self.player_by_id(&self.acting_player_id)?;
        default_player_board(&self.game, me.chosen_player_board)
    }

    /// Picks the player board to use (and shows it).
    pub(super) fn choose_player_board(&mut self, frame: &mut Frame, board_index: i64) {
        if self.player_by_id(&self.acting_player_id).is_none() {
            return;
        }
        self.perform(TableAction::SetIndividualPlaymat { on: true, index: board_index });
        let game_id = self.game.id.clone();
        display_settings::set_boards_shown(frame.settings, &game_id, true, board_index);
    }

    /// The host picks the shared board (in a game with only shared boards, none placed).
    pub(super) fn choose_shared_board(&mut self, frame: &mut Frame, board_index: i64) {
        if !self.can_load_scenario {
            return;
        }
        self.perform(TableAction::SetSharedPlaymat { on: true, index: board_index });
        let game_id = self.game.id.clone();
        display_settings::set_boards_shown(frame.settings, &game_id, true, board_index);
    }

    // -----------------------------------------------------------------------
    // Moving zones on game boards (Ctrl + drag)
    // -----------------------------------------------------------------------

    /// Whether this player may move the zones on boards of `owner_key`: each player on their own
    /// player boards, and only the host (who also shows and hides them) on shared boards.
    fn may_move_zones_of(&self, owner_key: &str) -> bool {
        if owner_key == SHARED_OWNER_KEY {
            self.can_load_scenario
        } else {
            owner_key == self.acting_player_id && self.player_by_id(owner_key).is_some()
        }
    }

    /// Where `owner_key`'s zones were moved, on all their boards.
    fn moved_zone_positions_of(&self, owner_key: &str) -> Vec<BoardZonePosition> {
        if owner_key == SHARED_OWNER_KEY {
            return self.state.shared_board_zone_positions.clone();
        }
        self.player_by_id(owner_key).map(|player| player.board_zone_positions.clone()).unwrap_or_default()
    }

    /// The board (1-based) `owner_key`'s zone sits on right now.
    fn board_holding_zone(&self, owner_key: &str, zone_id: &str) -> Option<i64> {
        let placed = self.derived.placed_zones.get(owner_key)?;
        placed.iter().find(|placed| placed.zone.id == zone_id).map(|placed| placed.board_index)
    }

    /// The (owner key, zone id) of a hit item that is a zone on a board this player may move.
    pub(super) fn movable_board_zone(&self, item: &HitItem) -> Option<(String, String)> {
        let zone_id = match &item.kind {
            ItemKind::Zone { zone_id, deck_widget: false, .. } => zone_id.clone(),
            ItemKind::DockedCounter { widget } | ItemKind::TokenBagIdle { widget } => widget.zone_id.clone()?,
            _ => return None,
        };
        let zone = self.zone_by_id(&zone_id)?;
        let owner_key = if zone.shared { SHARED_OWNER_KEY.to_string() } else { self.acting_player_id.clone() };
        let movable = self.may_move_zones_of(&owner_key) && self.board_holding_zone(&owner_key, &zone_id).is_some();
        movable.then_some((owner_key, zone_id))
    }

    /// Drops a zone being moved: its new spot on its board is sent to everyone and remembered on
    /// this computer for this game and board.
    pub(super) fn finish_zone_move(
        &mut self,
        frame: &mut Frame,
        zone_move: &crate::table::input::ZoneMoveDrag,
        mouse: (f32, f32),
    ) {
        let owner_key = zone_move.owner_key.as_str();
        if !self.may_move_zones_of(owner_key) {
            return;
        }
        let Some(board_index) = self.board_holding_zone(owner_key, &zone_move.zone_id) else { return };
        let Some(board) = self
            .boards_on_table(frame)
            .into_iter()
            .find(|board| board.owner_key == owner_key && board.board_index == board_index)
        else {
            return;
        };
        let center = (mouse.0 - zone_move.grab_offset.0, mouse.1 - zone_move.grab_offset.1);
        let (world_x, world_y) = self.screen_to_world(center.0, center.1);
        let (fraction_x, fraction_y) = placed_zone_fraction(board.world_rect, board.rotation, world_x, world_y);
        let mut positions = self.moved_zone_positions_of(owner_key);
        positions.retain(|position| position.board_index == board_index && position.zone_id != zone_move.zone_id);
        positions.push(BoardZonePosition::new(
            board_index,
            zone_move.zone_id.clone(),
            f64::from(fraction_x),
            f64::from(fraction_y),
        ));
        self.send_board_zone_positions(frame, board_index, positions);
    }

    /// Puts every zone on the board holding this zone back where the game places them, and
    /// forgets the moved positions.
    pub(super) fn reset_board_zone_positions(&mut self, frame: &mut Frame, owner_key: &str, zone_id: &str) {
        if self.may_move_zones_of(owner_key)
            && let Some(board_index) = self.board_holding_zone(owner_key, zone_id)
        {
            self.send_board_zone_positions(frame, board_index, Vec::new());
        }
    }

    /// Whether any zone was moved on the board holding this zone.
    pub(super) fn has_moved_board_zones(&self, owner_key: &str, zone_id: &str) -> bool {
        let Some(board_index) = self.board_holding_zone(owner_key, zone_id) else { return false };
        self.moved_zone_positions_of(owner_key).iter().any(|position| position.board_index == board_index)
    }

    fn send_board_zone_positions(&mut self, frame: &mut Frame, board_index: i64, positions: Vec<BoardZonePosition>) {
        let game_id = self.game.id.clone();
        display_settings::set_board_zone_positions(frame.settings, &game_id, board_index, &positions);
        self.perform(TableAction::SetBoardZonePositions { board_index, positions });
    }

    /// Sends where this player last moved zones on each board they may move zones on (their
    /// player boards; the host's, shared boards too), where that differs from the table.
    fn send_remembered_zone_positions(&mut self, frame: &Frame) {
        let me_seated = self.player_by_id(&self.acting_player_id).is_some();
        for (index, board) in self.game.game_boards.clone().iter().enumerate() {
            let board_index = index as i64 + 1;
            let owner_key = match board.kind {
                BoardKind::Player if me_seated => self.acting_player_id.clone(),
                BoardKind::Shared if self.can_load_scenario => SHARED_OWNER_KEY.to_string(),
                _ => continue,
            };
            let remembered = display_settings::board_zone_positions(frame.settings, &self.game.id, board_index);
            if remembered.is_empty() {
                continue;
            }
            let on_table = self.moved_zone_positions_of(&owner_key);
            let on_this_board: Vec<&BoardZonePosition> =
                on_table.iter().filter(|position| position.board_index == board_index).collect();
            let already_there = on_this_board.len() == remembered.len()
                && remembered.iter().all(|position| on_this_board.contains(&position));
            if !already_there {
                self.perform(TableAction::SetBoardZonePositions { board_index, positions: remembered });
            }
        }
    }

    // -----------------------------------------------------------------------
    // Hovered-card actions (hotkeys)
    // -----------------------------------------------------------------------

    pub(super) fn hovered_card(&self) -> Option<CardInstance> {
        self.hovered_id.as_ref().and_then(|id| self.derived.cards_by_id.get(id)).cloned()
    }

    /// The hovered table card when it's mine or unowned.
    pub(super) fn actable_hovered_table_card(&self) -> Option<CardInstance> {
        let card = self.hovered_card()?;
        if card.zone != CardZone::Table {
            return None;
        }
        if card.owner_id.as_deref().is_some_and(|owner| owner != self.acting_player_id) {
            return None;
        }
        Some(card)
    }

    /// The selected roots when the hovered card is part of the selection.
    pub(super) fn hovered_selection_roots(&self) -> Option<Vec<String>> {
        let card = self.actable_hovered_table_card()?;
        if !self.selected_roots.contains(&self.root_id_of(&card)) {
            return None;
        }
        Some(self.selected_root_list())
    }

    /// The hovered free token (interactable like its menu).
    pub(super) fn hovered_token(&self) -> Option<BoardWidgetInstance> {
        let item = self.hover_item.as_ref()?;
        match &item.kind {
            ItemKind::Token { widget } if item.interactable => Some(widget.clone()),
            _ => None,
        }
    }

    /// The selected tokens when the hovered token is part of a 2+ selection.
    pub(super) fn hovered_selected_token_ids(&self) -> Option<Vec<String>> {
        let widget = self.hovered_token()?;
        if !self.selection.tokens.contains(&widget.instance_id) || self.selected_count < 2 {
            return None;
        }
        Some(self.selected_token_ids())
    }

    fn rotation_sound(clockwise: bool) -> SoundEffect {
        if clockwise { SoundEffect::Tap } else { SoundEffect::Untap }
    }

    pub(super) fn rotate_hovered(&mut self, frame: &mut Frame, clockwise: bool, step: f64) {
        if let Some(roots) = self.hovered_selection_roots() {
            for root_instance_id in roots {
                self.perform(TableAction::RotateStack { root_instance_id, clockwise, step });
            }
            frame.sounds.play(Self::rotation_sound(clockwise));
            return;
        }
        if let Some(token_ids) = self.hovered_selected_token_ids() {
            for instance_id in token_ids {
                self.perform(TableAction::RotateWidget { instance_id, clockwise, step });
            }
            frame.sounds.play(Self::rotation_sound(clockwise));
            return;
        }
        if let Some(card) = self.actable_hovered_table_card() {
            let root_instance_id = self.root_id_of(&card);
            self.perform(TableAction::RotateStack { root_instance_id, clockwise, step });
            frame.sounds.play(Self::rotation_sound(clockwise));
            return;
        }
        if let Some(widget) = self.hovered_token() {
            self.perform(TableAction::RotateWidget { instance_id: widget.instance_id, clockwise, step });
            frame.sounds.play(Self::rotation_sound(clockwise));
        }
    }

    pub(super) fn send_hovered_to_back(&mut self) {
        if let Some(roots) = self.hovered_selection_roots() {
            for root_instance_id in roots {
                self.perform(TableAction::SendToBack { root_instance_id });
            }
            return;
        }
        if let Some(card) = self.actable_hovered_table_card() {
            let root_instance_id = self.root_id_of(&card);
            self.perform(TableAction::SendToBack { root_instance_id });
            return;
        }
        // A card token shares the card layer's z-order; other token kinds always draw above it.
        if let Some(widget) = self.hovered_token().filter(|widget| widget.kind == BoardWidgetKind::CardToken) {
            self.perform(TableAction::SendWidgetToBack { instance_id: widget.instance_id });
        }
    }

    pub(super) fn flip_hovered(&mut self) {
        if let Some(roots) = self.hovered_selection_roots() {
            for root in roots {
                if let Some(top) = top_card(&self.derived.pile_groups[&root]) {
                    let instance_id = top.instance_id.clone();
                    self.perform(TableAction::FlipCard { instance_id });
                }
            }
            return;
        }
        if let Some(token_ids) = self.hovered_selected_token_ids() {
            for instance_id in token_ids {
                self.perform(TableAction::FlipWidget { instance_id });
            }
            return;
        }
        if let Some(card) = self.hovered_card() {
            if card.owner_id.as_deref().is_some_and(|owner| owner != self.acting_player_id) {
                return;
            }
            if card.zone == CardZone::Zone && !self.zone_allows_flip(card.zone_id.as_deref().unwrap_or("")) {
                return;
            }
            self.perform(TableAction::FlipCard { instance_id: card.instance_id });
            return;
        }
        if let Some(widget) = self.hovered_token() {
            self.perform(TableAction::FlipWidget { instance_id: widget.instance_id });
        }
    }

    /// X: discards the hovered card (or the selection it's part of), or deletes a hovered token.
    pub(super) fn discard_hovered(&mut self, frame: &Frame) {
        if let Some(card) = self.actable_hovered_table_card() {
            let Some(zone_id) = self.local_discard_zone_id(card.owner_id.as_deref()) else { return };
            if self.hovered_selection_roots().is_some() {
                for selected in self.selected_cards() {
                    if selected.instance_id == card.instance_id {
                        self.start_discard_flight(frame, &selected, &zone_id);
                    } else {
                        self.perform(TableAction::ReturnToZone {
                            instance_id: selected.instance_id.clone(),
                            zone_id: zone_id.clone(),
                            to_bottom: false,
                        });
                    }
                }
                self.hovered_id = None;
                return;
            }
            self.start_discard_flight(frame, &card, &zone_id);
            self.hovered_id = None;
            return;
        }
        if let Some(token_ids) = self.hovered_selected_token_ids() {
            for instance_id in token_ids {
                self.perform(TableAction::DeleteWidget { instance_id });
            }
            return;
        }
        if let Some(widget) = self.hovered_token() {
            self.perform(TableAction::DeleteWidget { instance_id: widget.instance_id });
        }
    }

    fn start_discard_flight(&mut self, frame: &Frame, card: &CardInstance, zone_id: &str) {
        let mut zone_targets = self.drop_zones.iter().rev().chain(self.playmat_drop_zones.iter().rev());
        if let Some(target) = zone_targets.find(|zone| zone.zone_id == zone_id) {
            let from = self.to_screen(card.x, card.y);
            let rect = target.rect;
            self.flight = Some(DiscardFlight {
                instance_id: card.instance_id.clone(),
                from,
                to: (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0),
                face_up: card.face_up,
                definition_id: card.definition_id.clone(),
                started_at: frame.time,
            });
        }
        self.perform(TableAction::ReturnToZone {
            instance_id: card.instance_id.clone(),
            zone_id: zone_id.to_string(),
            to_bottom: false,
        });
    }

    /// 1-9: draw that many from the hovered pile or zone.
    pub(super) fn draw_from_hovered(&mut self, frame: &mut Frame, count: i64) {
        let Some(card) = self.hovered_card() else { return };
        match card.zone {
            CardZone::Table => {
                if card.owner_id.as_deref() != Some(self.acting_player_id.as_str()) {
                    return;
                }
                frame.sounds.play(SoundEffect::Draw);
                if count == 1 {
                    self.perform(TableAction::MoveToHand { instance_id: card.instance_id });
                } else {
                    let pile_instance_id = self.root_id_of(&card);
                    self.perform(TableAction::DrawCard { pile_instance_id, count });
                }
            }
            CardZone::Zone => {
                if card.owner_id.as_deref().is_some_and(|owner| owner != self.acting_player_id) {
                    return;
                }
                frame.sounds.play(SoundEffect::Draw);
                let zone_id = card.zone_id.unwrap_or_default();
                self.perform(TableAction::DrawFromZone { zone_id, count });
            }
            CardZone::Hand => {}
        }
    }

    pub(super) fn shuffle_zone(&mut self, frame: &mut Frame, zone_id: &str, wiggle_key: Option<String>) {
        self.perform(TableAction::ShuffleZone { zone_id: zone_id.to_string() });
        frame.sounds.play(SoundEffect::Shuffle);
        if let Some(key) = wiggle_key {
            self.wiggles.insert(key, frame.time);
        }
    }

    pub(super) fn shuffle_pile(&mut self, frame: &mut Frame, root_instance_id: &str) {
        self.perform(TableAction::ShufflePile { root_instance_id: root_instance_id.to_string() });
        frame.sounds.play(SoundEffect::Shuffle);
        self.wiggles.insert(format!("pile:{root_instance_id}"), frame.time);
    }

    // -----------------------------------------------------------------------
    // Keyboard
    // -----------------------------------------------------------------------

    pub(super) fn release_held_keys(&mut self) {
        self.space_down = false;
        self.tab_down = false;
        self.box_select = None;
        if self.arrow_drag.take().is_some() {
            self.perform(TableAction::EndArrowDragPreview);
        }
    }

    pub(super) fn toggle_game_menu(&mut self) {
        self.game_menu_open = !self.game_menu_open;
        if self.game_menu_open {
            self.log_open = false;
            self.release_held_keys();
        }
    }

    pub(super) fn toggle_log(&mut self) {
        self.log_open = !self.log_open;
        if self.log_open {
            self.release_held_keys();
            self.log_scroll_to_end = true;
        }
    }

    /// Ctrl+R: the documents viewer.
    fn toggle_rulebook(&mut self, frame: &mut Frame) {
        self.rulebook.toggle(frame);
    }

    /// `T:activeScenarioWithGuide()`: the most recently loaded scenario that has a guide.
    pub(super) fn active_scenario_with_guide(&self) -> Option<Scenario> {
        self.state
            .loaded_scenario_ids
            .iter()
            .rev()
            .filter_map(|id| self.game.scenario_by_id(id))
            .find(|scenario| scenario.guide_path.is_some())
            .cloned()
    }

    /// Server games chat through the room; practice games get a local-only panel for /flip and
    /// /roll; peer-hosted games have neither.
    pub(super) fn is_chat_available(&self) -> bool {
        self.room_chat.is_some() || self.is_local_practice
    }

    /// Relays the chat panel through a server room.
    pub fn set_room_chat(&mut self, chat: Box<dyn RoomChat>) {
        self.room_chat = Some(chat);
    }

    /// Keys are the table's only while no dialog, menu or text field has them.
    pub(super) fn keys_available(&self, frame: &Frame) -> bool {
        !frame.ui.any_overlay_open()
            && self.menu.is_none()
            && self.dialog.is_none()
            && frame.ui.focus.is_none()
            && !self.game_menu_open
            && !self.rulebook.is_open()
    }

    fn toggle_view_keys(&mut self, frame: &mut Frame, key: &str) -> bool {
        match key {
            "home" | "h" => {
                self.camera_x = 0.0;
                self.camera_y = 0.0;
                self.zoom = 1.0;
            }
            "f1" => self.borders_hidden = !self.borders_hidden,
            "f2" => self.color_tint_enabled = !self.color_tint_enabled,
            "f12" => self.toggle_boards(frame),
            "l" => self.toggle_log(),
            "r" if frame.keyboard.ctrl() => self.toggle_rulebook(frame),
            _ => return false,
        }
        true
    }

    pub fn key_pressed(&mut self, frame: &mut Frame, key: &str, is_repeat: bool) {
        if key == "escape" {
            if is_repeat {
                return;
            }
            // A selection (or a box being dragged) is cleared before Esc opens the menu.
            if !self.game_menu_open && (self.box_select.is_some() || self.selected_count > 0) {
                self.box_select = None;
                self.clear_selection();
                return;
            }
            self.toggle_game_menu();
            return;
        }
        if self.game_menu_open {
            return;
        }
        if self.rulebook.is_open() {
            if key == "r" && frame.keyboard.ctrl() && !is_repeat {
                self.toggle_rulebook(frame);
            }
            return;
        }
        if is_repeat {
            return;
        }
        if self.spectator {
            if key == "c" && self.room_chat.is_some() {
                self.chat_open = !self.chat_open;
            } else {
                self.toggle_view_keys(frame, key);
            }
            return;
        }
        // Q / E turn a board being moved by its handle.
        if matches!(key, "q" | "e") && self.turn_dragged_board(key == "q") {
            return;
        }
        let ctrl = frame.keyboard.ctrl();
        let step = if ctrl { 0.5 } else { 1.0 };
        match key {
            "q" => self.rotate_hovered(frame, false, step),
            "e" => self.rotate_hovered(frame, true, step),
            "x" => self.discard_hovered(frame),
            "f" => self.flip_hovered(),
            "z" => self.send_hovered_to_back(),
            "c" if self.is_chat_available() => self.chat_open = !self.chat_open,
            _ => {
                if self.toggle_view_keys(frame, key) {
                    return;
                }
                let digit = key.strip_prefix("kp").unwrap_or(key);
                if let Ok(count @ 1..=9) = digit.parse::<i64>() {
                    self.draw_from_hovered(frame, count);
                }
            }
        }
    }

    pub fn update(&mut self, frame: &mut Frame, dt: f32) {
        self.last_dt = dt;
        self.backend.update(frame.ui.frame_index());
        let chat_target = if self.chat_open { 1.0 } else { 0.0 };
        self.chat_animation += (chat_target - self.chat_animation) * (1.0 - (-CHAT_ANIMATION_RATE * dt).exp());
        let log_target = if self.log_open { 1.0 } else { 0.0 };
        self.log_animation += (log_target - self.log_animation) * (1.0 - (-LOG_ANIMATION_RATE * dt).exp());
        let available = self.keys_available(frame);
        let dragging = self.drag.as_ref().is_some_and(|drag| drag.started);
        let tab = available && frame.keyboard.is_down(&["tab"]) && !dragging;
        if self.tab_down && !tab && self.arrow_drag.take().is_some() {
            self.perform(TableAction::EndArrowDragPreview);
        }
        self.tab_down = tab;
        self.space_down = available && frame.keyboard.is_down(&["space"]);
        self.alt_down = frame.keyboard.alt();
        if available && self.table_rect.is_some() {
            let mut dx = 0.0;
            let mut dy = 0.0;
            if frame.keyboard.is_down(&["a"]) {
                dx += 1.0;
            }
            if frame.keyboard.is_down(&["d"]) {
                dx -= 1.0;
            }
            if frame.keyboard.is_down(&["w"]) {
                dy += 1.0;
            }
            if frame.keyboard.is_down(&["s"]) {
                dy -= 1.0;
            }
            if dx != 0.0 || dy != 0.0 {
                let (max_x, max_y) = self.max_pan();
                self.camera_x = (self.camera_x + dx * PAN_SPEED * dt).clamp(-max_x, max_x);
                self.camera_y = (self.camera_y + dy * PAN_SPEED * dt).clamp(-max_y, max_y);
            }
        }
    }

    /// `T:mySearchOpen()`.
    pub(super) fn my_search_open(&self) -> bool {
        self.state.searches.iter().any(|search| search.searcher_id == self.acting_player_id)
    }

    /// The game's tokens for the picker: (display name, file, resolved path, type).
    pub(super) fn defined_token_entries(&self) -> Vec<(String, Option<String>, Option<String>, Option<String>)> {
        let Some(folder) = self.game.folder_path.as_deref() else { return Vec::new() };
        if !self.game.uses_tokens {
            return Vec::new();
        }
        self.game
            .tokens
            .iter()
            .map(|token| {
                let path = token
                    .image_path
                    .as_deref()
                    .map(|image| crate::data::file_system::join_path(&[folder, "_tokens", image]));
                (
                    token.display_name().to_string(),
                    token.image_path.clone(),
                    path,
                    token.token_type().map(str::to_string),
                )
            })
            .collect()
    }

    /// The colours a colour token starts with (`COLOR_TOKEN_WIDGET`).
    pub(super) fn default_widget(kind: BoardWidgetKind) -> BoardWidgetInstance {
        BoardWidgetInstance::new("", kind, 0.0, 0.0, 0)
    }

    /// A zone-relative argb colour as a draw colour.
    pub(super) fn argb(argb: ArgbColor, alpha: f32) -> Color {
        argb_color(argb).with_alpha_multiplied(alpha)
    }
}
