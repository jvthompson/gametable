//! The authoritative game session. Port of `src/game/session.lua`.
//!
//! A [`Session`] owns the current [`TableState`], applies [`table_actions`] on behalf of a player,
//! and writes the English game-log lines the original writes, word for word. Writing a log line
//! never bumps the revision on its own.
//!
//! Instead of the original's listener callbacks, every place the original calls `notify` bumps
//! [`Session::notification_count`]; whoever drives the session (the UI or a [`HostEngine`]) checks
//! it after each call. A method that fails with an [`UnknownZone`](SessionError::UnknownZone)
//! error stops at the same point the Lua method raised its error, so any state change made before
//! that point is kept and no notification is sent, exactly as before.
//!
//! [`HostEngine`]: crate::host_engine::HostEngine

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::deck_widget_zones::{is_deck_widget_zone_id, parse_deck_widget_zone_id};
use crate::models::{
    BOARD_WIDGET_COUNTER_MAXIMUM, BOARD_WIDGET_COUNTER_MINIMUM, BoardWidgetInstance, BoardWidgetKind, CardDefinition,
    CardInstance, CardZone, DeckConfig, DeckEntry, GameDefinition, LogEntry, MAIN_DECK_SUBDECK_NAME, PlayerInfo,
    PlayerRole, SearchTargetType, TableState, WidgetSizeOverride, ZoneDefinition, ZoneKind, ZoneWidgetKind,
    build_standard_deck_cards,
};
use crate::pack_generator::select_pack_cards;
use crate::stack_utils::{root_id_of, stack_of};
use crate::table_actions as actions;
use crate::util::{RandomIndexSource, SystemRandom, shuffle_in_place};

/// Seconds an arrow stays on the table before it removes itself.
pub const ARROW_LIFETIME_SECONDS: f64 = 5.0;

/// A failure that the original raised as a Lua error (and the host engine then ignored).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// A zone id that is neither in the game nor loaded from a scenario.
    UnknownZone(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::UnknownZone(zone_id) => write!(formatter, "unknown zone '{zone_id}'"),
        }
    }
}

impl std::error::Error for SessionError {}

pub type SessionResult = Result<(), SessionError>;

/// Where a session gets the current time, random numbers and new instance ids. Injected so tests
/// (and the golden comparisons with the original) can be deterministic.
pub struct SessionEnvironment {
    /// Monotonic seconds; drives the game-log clock and timers.
    pub now_seconds: Box<dyn Fn() -> f64 + Send>,
    pub random: Box<dyn RandomIndexSource + Send>,
    pub new_instance_id: Box<dyn FnMut() -> String + Send>,
}

impl SessionEnvironment {
    /// Real time from `std::time::Instant`, OS randomness and v4 UUIDs.
    pub fn system() -> Self {
        let process_start = std::time::Instant::now();
        Self {
            now_seconds: Box::new(move || process_start.elapsed().as_secs_f64()),
            random: Box::new(SystemRandom),
            new_instance_id: Box::new(crate::uuid::new_v4_uuid),
        }
    }
}

/// Another player's in-progress card drag, shown as a ghost.
#[derive(Debug, Clone, PartialEq)]
pub struct CardDragPreview {
    pub player_id: String,
    pub instance_ids: Vec<String>,
    pub fraction_x: f64,
    pub fraction_y: f64,
}

/// Another player's in-progress arrow drag.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrowDragPreview {
    pub player_id: String,
    pub fraction_x: f64,
    pub fraction_y: f64,
    pub fraction_x2: f64,
    pub fraction_y2: f64,
}

/// Work scheduled for later by [`Session::update`].
#[derive(Debug, Clone, PartialEq)]
enum TimerAction {
    DeleteArrowIfPresent { instance_id: String },
}

#[derive(Debug, Clone, PartialEq)]
struct PendingTimer {
    due_at_seconds: f64,
    action: TimerAction,
}

/// What to deal at the start of a game.
pub struct DealOptions<'a> {
    pub game: GameDefinition,
    pub players: Vec<PlayerInfo>,
    pub local_player_id: String,
    /// `None` deals every player one of every card into each deck-building zone.
    pub deck_configs_by_player_id: Option<&'a HashMap<String, DeckConfig>>,
    /// Decks to load into shared zones, keyed by zone id.
    pub shared_deck_configs_by_zone_id: Option<&'a HashMap<String, DeckConfig>>,
    pub is_local_practice: bool,
}

/// The authoritative table plus everything needed to change it and describe the change.
pub struct Session {
    pub game: GameDefinition,
    pub local_player_id: String,
    state: TableState,
    pub is_local_practice: bool,
    environment: SessionEnvironment,
    clock_start_seconds: f64,
    /// In practice games, the seat the local player is currently acting as.
    active_seat_id: Option<String>,
    notification_count: u64,
    timers: Vec<PendingTimer>,
    /// Other players' drag ghosts, keyed by player id.
    pub card_drag_previews: HashMap<String, CardDragPreview>,
    pub arrow_drag_previews: HashMap<String, ArrowDragPreview>,
}

/// A widget zone's `widgetKind` -> the kind of the widget it docks.
fn board_kind_for_zone_widget(zone_widget_kind: ZoneWidgetKind) -> BoardWidgetKind {
    match zone_widget_kind {
        ZoneWidgetKind::Counter => BoardWidgetKind::SimpleCounter,
        ZoneWidgetKind::TokenBag => BoardWidgetKind::TokenBag,
    }
}

/// One image path per token in a bag zone's entries.
fn expand_bag_entries(entries: &[DeckEntry]) -> Vec<String> {
    entries
        .iter()
        .flat_map(|entry| std::iter::repeat_n(entry.definition_id.clone(), entry.quantity.max(0) as usize))
        .collect()
}

/// The widget a widget zone docks, for one player (`owner_id`) or shared (`None`).
fn build_dealt_widget(
    zone: &ZoneDefinition,
    owner_id: Option<&str>,
    z_index: i64,
    new_instance_id: &mut dyn FnMut() -> String,
) -> BoardWidgetInstance {
    let kind = board_kind_for_zone_widget(zone.widget_kind);
    let mut widget = BoardWidgetInstance::new(new_instance_id(), kind, 0.5, 0.5, z_index);
    widget.owner_id = owner_id.map(str::to_string);
    widget.zone_id = Some(zone.id.clone());
    if kind == BoardWidgetKind::TokenBag {
        widget.image_path = zone.bag_image_path.clone();
        widget.bag_remaining = expand_bag_entries(&zone.bag_entries);
    } else {
        widget.value = zone.counter_starting_value;
        widget.background_color = zone.counter_starting_color;
        widget.text_color = zone.counter_starting_text_color;
        widget.image_path = zone.counter_image_path.clone();
    }
    widget
}

/// Card ids for the entries, repeated by quantity, skipping unknown cards.
fn card_ids_for_entries(entries: &[DeckEntry], is_known: impl Fn(&str) -> bool) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| is_known(&entry.definition_id))
        .flat_map(|entry| std::iter::repeat_n(entry.definition_id.clone(), entry.quantity.max(0) as usize))
        .collect()
}

/// Lua's `tostring` of a number as used in log lines (e.g. `20`, `12.5`).
fn lua_number_text(number: f64) -> String {
    crate::json::format_like_c_percent_g(number, 14)
}

/// `"Name"` in quotes, or `a card` when the card's identity is hidden.
fn card_text(name: Option<&str>) -> String {
    match name {
        Some(name) => format!("\"{name}\""),
        None => "a card".to_string(),
    }
}

impl Session {
    pub fn new(
        game: GameDefinition,
        local_player_id: impl Into<String>,
        initial_state: TableState,
        is_local_practice: bool,
        environment: SessionEnvironment,
    ) -> Self {
        let clock_start_seconds = (environment.now_seconds)();
        Self {
            game,
            local_player_id: local_player_id.into(),
            state: initial_state,
            is_local_practice,
            environment,
            clock_start_seconds,
            active_seat_id: None,
            notification_count: 0,
            timers: Vec::new(),
            card_drag_previews: HashMap::new(),
            arrow_drag_previews: HashMap::new(),
        }
    }

    /// Deals a new game: each player's card zones, then widget zones, then shared card zones.
    pub fn deal_from_zones(options: DealOptions, mut environment: SessionEnvironment) -> Self {
        let game = options.game;
        let has_shared_standard_deck = game.zones.iter().any(|zone| zone.shared && zone.standard_deck);
        let standard_deck_cards = if has_shared_standard_deck { build_standard_deck_cards() } else { Vec::new() };
        let mut definitions_by_id: HashMap<&str, &CardDefinition> = HashMap::new();
        for card in game.cards().iter().chain(standard_deck_cards.iter()) {
            definitions_by_id.insert(card.id.as_str(), card); // later wins, as with Lua's defs[c.id] = c
        }
        let one_of_each = |cards: &[CardDefinition]| -> Vec<DeckEntry> {
            cards.iter().map(|card| DeckEntry::new(card.id.clone(), 1)).collect()
        };
        let full_deck_entries = one_of_each(game.cards());
        let standard_deck_entries = one_of_each(&standard_deck_cards);

        let mut cards: Vec<CardInstance> = Vec::new();
        let mut deal = |entries: &[DeckEntry],
                        zone_id: &str,
                        owner_id: Option<&str>,
                        face_up: bool,
                        auto_shuffle: bool,
                        environment: &mut SessionEnvironment| {
            let mut ids = card_ids_for_entries(entries, |id| definitions_by_id.contains_key(id));
            if auto_shuffle {
                shuffle_in_place(&mut ids, environment.random.as_mut());
            }
            for id in ids {
                let unownable = definitions_by_id[id.as_str()].unownable;
                let z_index = cards.len() as i64;
                cards.push(CardInstance {
                    z_index,
                    face_up,
                    zone_id: Some(zone_id.to_string()),
                    owner_id: owner_id.map(str::to_string),
                    unownable,
                    ..CardInstance::new((environment.new_instance_id)(), id, CardZone::Zone)
                });
            }
        };

        for player in &options.players {
            for zone in game.zones.iter().filter(|zone| !zone.shared && zone.kind == ZoneKind::Card) {
                let entries: &[DeckEntry] = if zone.deals_built_deck {
                    match options.deck_configs_by_player_id {
                        None => &full_deck_entries,
                        Some(decks) => {
                            decks.get(&player.id).map(|deck| deck.entries_for(&zone.deck_type)).unwrap_or(&[])
                        }
                    }
                } else {
                    &zone.entries
                };
                deal(entries, &zone.id, Some(&player.id), zone.face_up, zone.auto_shuffle, &mut environment);
            }
        }

        let mut widgets: Vec<BoardWidgetInstance> = Vec::new();
        for player in &options.players {
            for zone in game.zones.iter().filter(|zone| !zone.shared && zone.kind == ZoneKind::Widget) {
                let z_index = widgets.len() as i64;
                widgets.push(build_dealt_widget(zone, Some(&player.id), z_index, &mut environment.new_instance_id));
            }
        }
        for zone in game.zones.iter().filter(|zone| zone.shared && zone.kind == ZoneKind::Widget) {
            let z_index = widgets.len() as i64;
            widgets.push(build_dealt_widget(zone, None, z_index, &mut environment.new_instance_id));
        }

        for zone in game.zones.iter().filter(|zone| zone.shared && zone.kind == ZoneKind::Card) {
            let loaded_deck = options.shared_deck_configs_by_zone_id.and_then(|decks| decks.get(&zone.id));
            let entries: &[DeckEntry] = if zone.standard_deck {
                &standard_deck_entries
            } else if let Some(loaded_deck) = loaded_deck {
                loaded_deck.entries_for(MAIN_DECK_SUBDECK_NAME)
            } else if !zone.entries.is_empty() || zone.is_discard_pile {
                &zone.entries
            } else {
                &full_deck_entries
            };
            deal(entries, &zone.id, None, zone.face_up, zone.auto_shuffle, &mut environment);
        }

        let first_player_id = options
            .players
            .iter()
            .find(|player| player.role == PlayerRole::Host)
            .or(options.players.first())
            .map(|player| player.id.clone());

        let mut initial_state = TableState::new(game.id.clone()).with_cards(cards).with_widgets(widgets);
        initial_state.players = options.players;
        initial_state.current_player_id = first_player_id;
        Self::new(game, options.local_player_id, initial_state, options.is_local_practice, environment)
    }

    // -----------------------------------------------------------------------
    // State, notification, timers
    // -----------------------------------------------------------------------

    pub fn state(&self) -> &TableState {
        &self.state
    }

    /// Bumped every time the session would have notified its listeners.
    pub fn notification_count(&self) -> u64 {
        self.notification_count
    }

    fn notify(&mut self) {
        self.notification_count += 1;
    }

    pub fn acting_player_id(&self) -> &str {
        self.active_seat_id.as_deref().unwrap_or(&self.local_player_id)
    }

    /// In practice games, switches which seat the local player acts as.
    pub fn set_active_seat(&mut self, player_id: Option<&str>) {
        if !self.is_local_practice {
            return;
        }
        self.active_seat_id = player_id.map(str::to_string);
        self.notify();
    }

    /// Runs any timers that are due (call once per frame / server tick).
    pub fn update(&mut self) {
        if self.timers.is_empty() {
            return;
        }
        let now = (self.environment.now_seconds)();
        let (due, still_pending): (Vec<PendingTimer>, Vec<PendingTimer>) =
            std::mem::take(&mut self.timers).into_iter().partition(|timer| now >= timer.due_at_seconds);
        self.timers = still_pending;
        for timer in due {
            match timer.action {
                TimerAction::DeleteArrowIfPresent { instance_id } => {
                    if self.find_widget(&instance_id).is_some() {
                        self.delete_widget(&instance_id);
                    }
                }
            }
        }
    }

    fn schedule(&mut self, delay_seconds: f64, action: TimerAction) {
        let due_at_seconds = (self.environment.now_seconds)() + delay_seconds;
        self.timers.push(PendingTimer { due_at_seconds, action });
    }

    // -----------------------------------------------------------------------
    // Lookups
    // -----------------------------------------------------------------------

    pub fn find_card(&self, instance_id: &str) -> Option<&CardInstance> {
        self.state.card_by_id(instance_id)
    }

    pub fn find_widget(&self, instance_id: &str) -> Option<&BoardWidgetInstance> {
        self.state.widget_by_id(instance_id)
    }

    pub fn definition(&self, definition_id: &str) -> Option<&CardDefinition> {
        self.game.card_by_id(definition_id)
    }

    /// A player's name, or the id itself for an unknown player.
    pub fn player_name(&self, player_id: &str) -> String {
        self.state.player_by_id(player_id).map_or_else(|| player_id.to_string(), |player| player.name.clone())
    }

    /// The game's zones followed by any zones loaded from scenarios.
    pub fn effective_zones(&self) -> impl Iterator<Item = &ZoneDefinition> {
        self.game.zones.iter().chain(self.state.loaded_zones.iter())
    }

    /// The zone definition for a zone id (deck widget ids resolve to their real zone).
    pub fn zone_definition(&self, zone_id: Option<&str>) -> Result<&ZoneDefinition, SessionError> {
        let unknown = || SessionError::UnknownZone(zone_id.unwrap_or("nil").to_string());
        let zone_id = zone_id.ok_or_else(unknown)?;
        let real_zone_id = parse_deck_widget_zone_id(zone_id).map_or(zone_id, |parsed| parsed.real_zone_id);
        self.game
            .zone_by_id(real_zone_id)
            .or_else(|| self.state.loaded_zones.iter().find(|zone| zone.id == real_zone_id))
            .ok_or_else(unknown)
    }

    /// Whether every player can currently see this card's face.
    pub fn is_publicly_visible(&self, card: &CardInstance) -> Result<bool, SessionError> {
        if !card.face_up {
            return Ok(false);
        }
        match card.zone {
            CardZone::Table => return Ok(true),
            CardZone::Hand => return Ok(false),
            CardZone::Zone => {}
        }
        if card.owner_id.is_none() {
            return Ok(true);
        }
        if card.zone_id.as_deref().is_some_and(is_deck_widget_zone_id) {
            return Ok(false);
        }
        Ok(self.zone_definition(card.zone_id.as_deref())?.visible_to_all)
    }

    fn title_of(&self, card: Option<&CardInstance>) -> Option<String> {
        let card = card?;
        self.definition(&card.definition_id).map(|definition| definition.card_title.clone())
    }

    /// The card's title when it was visible before or after a change; `None` otherwise.
    fn revealed_card_name(
        &self,
        old_card: Option<&CardInstance>,
        new_card: Option<&CardInstance>,
    ) -> Result<Option<String>, SessionError> {
        let was_visible = match old_card {
            Some(card) => self.is_publicly_visible(card)?,
            None => false,
        };
        let revealed = was_visible
            || match new_card {
                Some(card) => self.is_publicly_visible(card)?,
                None => false,
            };
        if !revealed {
            return Ok(None);
        }
        Ok(self.title_of(new_card.or(old_card)))
    }

    /// The title when the card is publicly visible, else `None`.
    fn visible_title(&self, card: Option<&CardInstance>) -> Result<Option<String>, SessionError> {
        match card {
            Some(card) if self.is_publicly_visible(card)? => Ok(self.title_of(Some(card))),
            _ => Ok(None),
        }
    }

    /// Restarts the game-log clock so it reads `elapsed_milliseconds` now (the Lua `clockStart`
    /// option, used when resuming a saved game so the log continues where it left off).
    pub fn resume_log_clock(&mut self, elapsed_milliseconds: i64) {
        self.clock_start_seconds = (self.environment.now_seconds)() - elapsed_milliseconds as f64 / 1000.0;
    }

    pub fn elapsed_milliseconds(&self) -> i64 {
        let elapsed_seconds = (self.environment.now_seconds)() - self.clock_start_seconds;
        ((elapsed_seconds * 1000.0).floor() as i64).max(0)
    }

    /// Appends `"<player name> <action>"` to the game log (no revision bump).
    fn log(&mut self, acting_player_id: &str, action: &str) {
        let message = format!("{} {}", self.player_name(acting_player_id), action);
        let entry = LogEntry::new(self.elapsed_milliseconds(), message);
        self.state.log.push(entry);
    }

    fn log_played_to_table(
        &mut self,
        acting_player_id: &str,
        old_card: &CardInstance,
        instance_id: &str,
    ) -> SessionResult {
        let new_card = self.find_card(instance_id).cloned();
        let name = self.revealed_card_name(Some(old_card), new_card.as_ref())?;
        let origin = if old_card.zone == CardZone::Hand {
            "hand".to_string()
        } else {
            format!("the {}", self.zone_definition(old_card.zone_id.as_deref())?.name)
        };
        self.log(acting_player_id, &format!("played {} to the table from {origin}.", card_text(name.as_deref())));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Card operations
    // -----------------------------------------------------------------------

    pub fn move_card(&mut self, instance_id: &str, x: f64, y: f64, acting_player_id: &str) -> SessionResult {
        let old_card = self.find_card(instance_id).cloned();
        self.state = actions::move_card(&self.state, instance_id, x, y);
        if let Some(old_card) = old_card.filter(|card| card.zone != CardZone::Table) {
            self.log_played_to_table(acting_player_id, &old_card, instance_id)?;
        }
        self.notify();
        Ok(())
    }

    pub fn move_stack(&mut self, root_instance_id: &str, x: f64, y: f64) {
        self.state = actions::move_stack(&self.state, root_instance_id, x, y);
        self.notify();
    }

    pub fn move_group(
        &mut self,
        primary_instance_id: &str,
        passenger_root_ids: &[String],
        x: f64,
        y: f64,
        acting_player_id: &str,
    ) -> SessionResult {
        let old_card = self.find_card(primary_instance_id).cloned();
        self.state = actions::move_group(&self.state, primary_instance_id, passenger_root_ids, x, y);
        if let Some(old_card) = old_card.filter(|card| card.zone != CardZone::Table) {
            self.log_played_to_table(acting_player_id, &old_card, primary_instance_id)?;
        }
        self.notify();
        Ok(())
    }

    pub fn rotate_stack(
        &mut self,
        root_instance_id: &str,
        clockwise: bool,
        acting_player_id: &str,
        step: f64,
    ) -> SessionResult {
        self.state = actions::rotate_stack(&self.state, root_instance_id, clockwise, step);
        if let Some(card) = self.find_card(root_instance_id).cloned() {
            let message = match self.visible_title(Some(&card))? {
                Some(name) => format!("rotated \"{name}\"."),
                None => "rotated a card.".to_string(),
            };
            self.log(acting_player_id, &message);
        }
        self.notify();
        Ok(())
    }

    pub fn rotate_widget(&mut self, instance_id: &str, clockwise: bool, step: f64) {
        self.state = actions::rotate_widget(&self.state, instance_id, clockwise, step);
        self.notify();
    }

    pub fn flip_widget(&mut self, instance_id: &str) {
        self.state = actions::flip_widget(&self.state, instance_id);
        self.notify();
    }

    pub fn send_widget_to_back(&mut self, instance_id: &str) {
        self.state = actions::send_widget_to_back(&self.state, instance_id);
        self.notify();
    }

    pub fn send_to_back(&mut self, root_instance_id: &str) {
        self.state = actions::send_to_back(&self.state, root_instance_id);
        self.notify();
    }

    pub fn flip_card(&mut self, instance_id: &str, acting_player_id: &str) -> SessionResult {
        self.state = actions::flip_card(&self.state, instance_id);
        if let Some(card) = self.find_card(instance_id).cloned().filter(|card| card.zone != CardZone::Hand) {
            let direction = if card.face_up { "face up" } else { "face down" };
            let message = match self.visible_title(Some(&card))? {
                Some(name) => format!("flipped \"{name}\" {direction}."),
                None => format!("flipped a card {direction}."),
            };
            self.log(acting_player_id, &message);
        }
        self.notify();
        Ok(())
    }

    /// Whether the card sits in a table pile with other cards.
    pub fn is_in_pile(&self, instance_id: &str) -> bool {
        let Some(card) = self.find_card(instance_id).filter(|card| card.zone == CardZone::Table) else {
            return false;
        };
        let cards = self.state.cards();
        stack_of(cards, root_id_of(cards, card)).len() > 1
    }

    /// Gives a card to a player or (with `None`) releases it. Cards in a pile are refused silently.
    pub fn give_card(
        &mut self,
        instance_id: &str,
        new_owner_id: Option<&str>,
        acting_player_id: &str,
    ) -> SessionResult {
        if self.is_in_pile(instance_id) {
            return Ok(());
        }
        self.state = actions::give_card(&self.state, instance_id, new_owner_id);
        let card = self.find_card(instance_id).cloned();
        let name = self.visible_title(card.as_ref())?;
        let message = match new_owner_id {
            Some(new_owner_id) => format!("gave {} to {}.", card_text(name.as_deref()), self.player_name(new_owner_id)),
            None => format!("released {} back to the table.", card_text(name.as_deref())),
        };
        self.log(acting_player_id, &message);
        self.notify();
        Ok(())
    }

    /// Piles never mix owners: the card (after stacking; unownable cards become unowned) must have
    /// the same owner as every other card in the target pile.
    pub fn can_stack_onto(&self, instance_id: &str, onto_instance_id: &str) -> bool {
        let (Some(card), Some(onto)) = (self.find_card(instance_id), self.find_card(onto_instance_id)) else {
            return false;
        };
        let owner_after_stacking = if card.unownable { None } else { card.owner_id.as_deref() };
        let cards = self.state.cards();
        stack_of(cards, root_id_of(cards, onto)).iter().all(|pile_card| {
            pile_card.instance_id == instance_id || pile_card.owner_id.as_deref() == owner_after_stacking
        })
    }

    /// Puts a card on a pile; refused silently when the pile has a different owner.
    pub fn stack_card(&mut self, instance_id: &str, onto_instance_id: &str, acting_player_id: &str) -> SessionResult {
        if !self.can_stack_onto(instance_id, onto_instance_id) {
            return Ok(());
        }
        let old_card = self.find_card(instance_id).cloned();
        self.state = actions::stack_card(&self.state, instance_id, onto_instance_id);
        if let Some(old_card) = old_card.filter(|card| card.zone != CardZone::Table) {
            self.log_played_to_table(acting_player_id, &old_card, instance_id)?;
        }
        self.notify();
        Ok(())
    }

    /// `owner_id` defaults to the local player.
    pub fn reorder_hand(&mut self, instance_id: &str, target_index: i64, owner_id: Option<&str>) {
        let owner_id = owner_id.unwrap_or(&self.local_player_id).to_string();
        self.state = actions::reorder_hand(&self.state, instance_id, Some(&owner_id), target_index);
        self.notify();
    }

    /// `owner_id` defaults to the local player.
    pub fn move_to_hand(&mut self, instance_id: &str, owner_id: Option<&str>) -> SessionResult {
        let acting = owner_id.unwrap_or(&self.local_player_id).to_string();
        let old_card = self.find_card(instance_id).cloned();
        self.state = actions::move_to_hand(&self.state, instance_id, &acting);
        let message = match self.visible_title(old_card.as_ref())? {
            Some(name) => format!("picked up \"{name}\" into their hand."),
            None => "picked up a card into their hand.".to_string(),
        };
        self.log(&acting, &message);
        self.notify();
        Ok(())
    }

    /// Draws from a table pile into `owner_id`'s hand (default: the local player).
    pub fn draw_card(&mut self, pile_instance_id: &str, owner_id: Option<&str>, count: i64) {
        let acting = owner_id.unwrap_or(&self.local_player_id).to_string();
        self.state = actions::draw_card(&self.state, pile_instance_id, &acting, count).state;
        let message = if count == 1 { "drew a card.".to_string() } else { format!("drew {count} cards.") };
        self.log(&acting, &message);
        self.notify();
    }

    pub fn shuffle_pile(&mut self, pile_root_instance_id: &str, acting_player_id: &str) {
        self.state = actions::shuffle_pile(&self.state, pile_root_instance_id, self.environment.random.as_mut());
        self.log(acting_player_id, "shuffled a pile of cards.");
        self.notify();
    }

    /// Draws from a zone into `to_owner_id`'s hand (default: the local player).
    pub fn draw_from_zone(
        &mut self,
        zone_id: &str,
        zone_owner_id: Option<&str>,
        to_owner_id: Option<&str>,
        count: i64,
    ) -> SessionResult {
        let acting = to_owner_id.unwrap_or(&self.local_player_id).to_string();
        let zone_name = self.zone_definition(Some(zone_id))?.name.clone();
        self.state = actions::draw_from_zone(&self.state, zone_id, zone_owner_id, &acting, count).state;
        let message = if count == 1 {
            format!("drew a card from the {zone_name}.")
        } else {
            format!("drew {count} cards from the {zone_name}.")
        };
        self.log(&acting, &message);
        self.notify();
        Ok(())
    }

    pub fn return_to_zone(
        &mut self,
        instance_id: &str,
        zone_id: &str,
        zone_owner_id: Option<&str>,
        to_bottom: bool,
        acting_player_id: &str,
    ) -> SessionResult {
        let zone = self.zone_definition(Some(zone_id))?.clone();
        let old_card = self.find_card(instance_id).cloned();
        self.state = actions::return_to_zone(&self.state, instance_id, zone_id, zone_owner_id, zone.face_up, to_bottom);
        let new_card = self.find_card(instance_id).cloned();
        let name = self.revealed_card_name(old_card.as_ref(), new_card.as_ref())?;
        let message = if zone.is_discard_pile {
            format!("discarded {}.", card_text(name.as_deref()))
        } else {
            format!("returned {} to the {}.", card_text(name.as_deref()), zone.name)
        };
        self.log(acting_player_id, &message);
        self.notify();
        Ok(())
    }

    /// Shuffles a zone, unless the zone is marked not shuffleable.
    pub fn shuffle_zone(
        &mut self,
        zone_id: &str,
        zone_owner_id: Option<&str>,
        acting_player_id: &str,
    ) -> SessionResult {
        let zone = self.zone_definition(Some(zone_id))?;
        if !zone.shuffleable {
            return Ok(());
        }
        let zone_name = zone.name.clone();
        self.state = actions::shuffle_zone(&self.state, zone_id, zone_owner_id, self.environment.random.as_mut());
        self.log(acting_player_id, &format!("shuffled the {zone_name}."));
        self.notify();
        Ok(())
    }

    /// Loads a deck: fills the clicked zone and every other deck-building zone whose deck type the
    /// deck has a subdeck for.
    pub fn load_deck_into_zone(&mut self, zone_id: &str, deck: &DeckConfig, acting_player_id: &str) -> SessionResult {
        let target_zone = self.zone_definition(Some(zone_id))?.clone();
        let mut zones = vec![target_zone.clone()];
        zones.extend(self.game.deck_building_zones().filter(|zone| zone.id != target_zone.id).cloned());

        let mut new_cards = Vec::new();
        let mut filled_zone_names: Vec<String> = Vec::new();
        for zone in &zones {
            let mut ids =
                card_ids_for_entries(deck.entries_for(&zone.deck_type), |id| self.game.card_by_id(id).is_some());
            if ids.is_empty() {
                continue;
            }
            if zone.auto_shuffle {
                shuffle_in_place(&mut ids, self.environment.random.as_mut());
            }
            for id in ids {
                let unownable = self.game.card_by_id(&id).is_some_and(|definition| definition.unownable);
                new_cards.push(CardInstance {
                    z_index: 0,
                    face_up: zone.face_up,
                    zone_id: Some(zone.id.clone()),
                    owner_id: Some(acting_player_id.to_string()),
                    unownable,
                    ..CardInstance::new((self.environment.new_instance_id)(), id, CardZone::Zone)
                });
            }
            filled_zone_names.push(zone.name.clone());
        }
        self.state = actions::deal_new_cards(&self.state, new_cards);
        let label = match filled_zone_names.as_slice() {
            [] => target_zone.name.clone(),
            [only] => only.clone(),
            [first, second] => format!("{first} and {second}"),
            [all_but_last @ .., last] => format!("{}, and {last}", all_but_last.join(", ")),
        };
        self.log(acting_player_id, &format!("loaded a deck into the {label}."));
        self.notify();
        Ok(())
    }

    /// Loads a scenario once: adds its zones (skipping ids already in use), deals their cards and
    /// widgets, and places its authored cards and tokens.
    pub fn load_scenario(&mut self, scenario_id: &str, acting_player_id: &str) {
        let Some(scenario) = self.game.scenario_by_id(scenario_id).cloned() else { return };
        if self.state.loaded_scenario_ids.iter().any(|loaded| loaded == scenario_id) {
            return;
        }

        let mut used_zone_ids: HashSet<String> = self.effective_zones().map(|zone| zone.id.clone()).collect();
        let new_zones: Vec<ZoneDefinition> =
            scenario.zones.iter().filter(|zone| used_zone_ids.insert(zone.id.clone())).cloned().collect();
        self.state.loaded_zones.extend(new_zones.iter().cloned());
        self.state.loaded_scenario_ids.push(scenario_id.to_string());

        let mut new_widgets = Vec::new();
        for zone in new_zones.iter().filter(|zone| zone.kind == ZoneKind::Widget) {
            // Scenario zones are always shared.
            new_widgets.push(build_dealt_widget(zone, None, 0, &mut self.environment.new_instance_id));
        }
        for placement in &scenario.token_placements {
            let mut token = BoardWidgetInstance::new(
                (self.environment.new_instance_id)(),
                BoardWidgetKind::ImageToken,
                placement.x,
                placement.y,
                0,
            );
            token.image_path = Some(placement.image_path.clone());
            token.rotation_turns = placement.rotation_turns;
            token.size_override = placement.size_override;
            new_widgets.push(token);
        }
        self.state = actions::add_widgets(&self.state, new_widgets);

        let mut new_cards = Vec::new();
        for zone in new_zones.iter().filter(|zone| zone.kind == ZoneKind::Card) {
            let entries = if zone.deals_built_deck { scenario.entries_for(&zone.deck_type) } else { &zone.entries };
            let mut ids = card_ids_for_entries(entries, |id| self.game.card_by_id(id).is_some());
            if zone.auto_shuffle {
                shuffle_in_place(&mut ids, self.environment.random.as_mut());
            }
            for id in ids {
                let unownable = self.game.card_by_id(&id).is_some_and(|definition| definition.unownable);
                new_cards.push(CardInstance {
                    z_index: 0,
                    face_up: zone.face_up,
                    zone_id: Some(zone.id.clone()),
                    unownable,
                    ..CardInstance::new((self.environment.new_instance_id)(), id, CardZone::Zone)
                });
            }
        }
        for placement in &scenario.placements {
            let Some(definition) = self.game.card_by_id(&placement.definition_id) else { continue };
            let unownable = definition.unownable;
            new_cards.push(CardInstance {
                x: placement.x,
                y: placement.y,
                z_index: 0,
                face_up: placement.face_up,
                unownable,
                ..CardInstance::new(
                    (self.environment.new_instance_id)(),
                    placement.definition_id.clone(),
                    CardZone::Table,
                )
            });
        }
        self.state = actions::deal_new_cards(&self.state, new_cards);
        self.log(acting_player_id, &format!("loaded the \"{}\" scenario.", scenario.name));
        self.notify();
    }

    // -----------------------------------------------------------------------
    // Searches
    // -----------------------------------------------------------------------

    pub fn start_search_zone(
        &mut self,
        zone_id: &str,
        zone_owner_id: Option<&str>,
        searcher_id: Option<&str>,
    ) -> SessionResult {
        let acting = searcher_id.unwrap_or(&self.local_player_id).to_string();
        let zone_name = self.zone_definition(Some(zone_id))?.name.clone();
        self.state = actions::start_search(&self.state, &acting, SearchTargetType::Zone, zone_id, zone_owner_id);
        self.log(&acting, &format!("opened a search on the {zone_name}."));
        self.notify();
        Ok(())
    }

    pub fn start_search_pile(&mut self, pile_root_instance_id: &str, searcher_id: Option<&str>) {
        let acting = searcher_id.unwrap_or(&self.local_player_id).to_string();
        self.state = actions::start_search(&self.state, &acting, SearchTargetType::Pile, pile_root_instance_id, None);
        self.log(&acting, "opened a search on a pile of cards.");
        self.notify();
    }

    pub fn stop_search(&mut self, searcher_id: Option<&str>) {
        let acting = searcher_id.unwrap_or(&self.local_player_id).to_string();
        let was_searching = self.state.searches.iter().any(|search| search.searcher_id == acting);
        self.state = actions::stop_search(&self.state, &acting);
        if was_searching {
            self.log(&acting, "closed their search.");
        }
        self.notify();
    }

    // -----------------------------------------------------------------------
    // Widgets
    // -----------------------------------------------------------------------

    /// Deck widgets and card tokens belong to whoever creates them; other widgets are free.
    pub fn create_widget(
        &mut self,
        instance_id: &str,
        kind: BoardWidgetKind,
        x: f64,
        y: f64,
        acting_player_id: &str,
        image_path: Option<&str>,
    ) {
        let is_owned_kind = matches!(kind, BoardWidgetKind::DeckBuilder | BoardWidgetKind::CardToken);
        let owner_id = is_owned_kind.then_some(acting_player_id);
        self.state = actions::create_widget(&self.state, instance_id, kind, x, y, owner_id, image_path);
        self.notify();
    }

    /// Adds an arrow that deletes itself after [`ARROW_LIFETIME_SECONDS`].
    pub fn create_arrow(&mut self, instance_id: &str, x: f64, y: f64, x2: f64, y2: f64, creator_id: &str) {
        self.state = actions::create_arrow(&self.state, instance_id, x, y, x2, y2, Some(creator_id));
        self.notify();
        self.schedule(
            ARROW_LIFETIME_SECONDS,
            TimerAction::DeleteArrowIfPresent { instance_id: instance_id.to_string() },
        );
    }

    pub fn move_widget(&mut self, instance_id: &str, x: f64, y: f64) {
        self.state = actions::move_widget(&self.state, instance_id, x, y);
        self.notify();
    }

    pub fn set_widget_value(&mut self, instance_id: &str, value: f64, acting_player_id: &str) -> SessionResult {
        let widget = self.find_widget(instance_id).cloned();
        self.state = actions::set_widget_value(&self.state, instance_id, value);
        if let Some(widget) = widget.filter(|widget| widget.kind == BoardWidgetKind::SimpleCounter) {
            let clamped = value.clamp(BOARD_WIDGET_COUNTER_MINIMUM, BOARD_WIDGET_COUNTER_MAXIMUM);
            let label = match &widget.zone_id {
                Some(zone_id) => self.zone_definition(Some(zone_id))?.name.clone(),
                None => "a counter".to_string(),
            };
            self.log(acting_player_id, &format!("set {label} to {}.", lua_number_text(clamped)));
        }
        self.notify();
        Ok(())
    }

    pub fn delete_widget(&mut self, instance_id: &str) {
        self.state = actions::delete_widget(&self.state, instance_id);
        self.notify();
    }

    pub fn set_widget_colors(
        &mut self,
        instance_id: &str,
        background_color: crate::color::ArgbColor,
        text_color: crate::color::ArgbColor,
    ) {
        self.state = actions::set_widget_colors(&self.state, instance_id, background_color, text_color);
        self.notify();
    }

    pub fn set_widget_text(&mut self, instance_id: &str, text: &str, text_color: crate::color::ArgbColor) {
        self.state = actions::set_widget_text(&self.state, instance_id, text, text_color);
        self.notify();
    }

    pub fn set_widget_size(&mut self, instance_id: &str, size_override: Option<WidgetSizeOverride>) {
        self.state = actions::set_widget_size(&self.state, instance_id, size_override);
        self.notify();
    }

    pub fn duplicate_widget(&mut self, source_instance_id: &str, new_instance_id: &str, x: f64, y: f64) {
        self.state = actions::duplicate_widget(&self.state, source_instance_id, new_instance_id, x, y);
        self.notify();
    }

    /// The bag label used in log lines: `the <zone name>` or `the token bag`.
    fn bag_label(&self, widget: &BoardWidgetInstance) -> Result<String, SessionError> {
        Ok(match &widget.zone_id {
            Some(zone_id) => format!("the {}", self.zone_definition(Some(zone_id))?.name),
            None => "the token bag".to_string(),
        })
    }

    /// Whether tokens go back into this bag (bags outside a zone always take them back).
    fn bag_is_returnable(&self, bag: &BoardWidgetInstance) -> Result<bool, SessionError> {
        Ok(match &bag.zone_id {
            Some(zone_id) => self.zone_definition(Some(zone_id))?.bag_returnable,
            None => true,
        })
    }

    /// Draws a random token from a bag and reveals it.
    pub fn draw_bag_token(&mut self, instance_id: &str, acting_player_id: &str) -> SessionResult {
        let Some(bag) = self.find_widget(instance_id).cloned() else { return Ok(()) };
        if bag.kind != BoardWidgetKind::TokenBag || bag.bag_remaining.is_empty() {
            return Ok(());
        }
        let pick = self.environment.random.random_from_one_to(bag.bag_remaining.len());
        let image_path = bag.bag_remaining[pick - 1].clone();
        let slot_id = (self.environment.new_instance_id)();
        self.state = actions::reveal_bag_token(&self.state, instance_id, &image_path, &slot_id);
        let label = self.bag_label(&bag)?;
        self.log(acting_player_id, &format!("drew a token from {label}."));
        self.notify();
        Ok(())
    }

    /// Resolves a revealed token: back into the bag when the bag is returnable, otherwise gone.
    pub fn return_bag_token(&mut self, instance_id: &str, slot_id: &str, acting_player_id: &str) -> SessionResult {
        let Some(bag) = self.find_widget(instance_id).cloned() else { return Ok(()) };
        let returnable = self.bag_is_returnable(&bag)?;
        self.state = actions::resolve_bag_token(&self.state, instance_id, slot_id, returnable);
        let message = if returnable { "returned a token to the bag." } else { "discarded a token from the bag." };
        self.log(acting_player_id, message);
        self.notify();
        Ok(())
    }

    /// Puts a revealed token on the table at the image's own size.
    pub fn place_bag_token_on_table(
        &mut self,
        instance_id: &str,
        slot_id: &str,
        new_instance_id: &str,
        x: f64,
        y: f64,
        acting_player_id: &str,
    ) {
        self.state = actions::place_bag_token_on_table(
            &self.state,
            instance_id,
            slot_id,
            new_instance_id,
            x,
            y,
            None,
            Some(WidgetSizeOverride::NativeImageSize),
        );
        self.log(acting_player_id, "placed a token from the bag onto the table.");
        self.notify();
    }

    /// Refills a bag from its zone's entries (empty for a bag outside a zone).
    pub fn reset_bag(&mut self, instance_id: &str, acting_player_id: &str) -> SessionResult {
        let Some(bag) = self.find_widget(instance_id).cloned() else { return Ok(()) };
        if bag.kind != BoardWidgetKind::TokenBag {
            return Ok(());
        }
        let fresh_remaining = match &bag.zone_id {
            Some(zone_id) => expand_bag_entries(&self.zone_definition(Some(zone_id))?.bag_entries),
            None => Vec::new(),
        };
        self.state = actions::reset_bag(&self.state, instance_id, &fresh_remaining);
        self.log(acting_player_id, "reset the token bag.");
        self.notify();
        Ok(())
    }

    /// Replaces a bag's contents from its "Change Tokens" dialog.
    pub fn set_bag_remaining(&mut self, instance_id: &str, bag_remaining: &[String], acting_player_id: &str) {
        let is_bag = self.find_widget(instance_id).is_some_and(|widget| widget.kind == BoardWidgetKind::TokenBag);
        if !is_bag {
            return;
        }
        self.state = actions::set_bag_remaining(&self.state, instance_id, bag_remaining);
        self.log(acting_player_id, "changed the token bag's contents.");
        self.notify();
    }

    /// Drops a free image token onto a bag: put away when returnable, destroyed otherwise.
    pub fn return_image_token_to_bag(
        &mut self,
        bag_instance_id: &str,
        token_instance_id: &str,
        acting_player_id: &str,
    ) -> SessionResult {
        let Some(bag) = self.find_widget(bag_instance_id).cloned() else { return Ok(()) };
        let returnable = self.bag_is_returnable(&bag)?;
        self.state = actions::return_image_token_to_bag(&self.state, bag_instance_id, token_instance_id, returnable);
        let message = if returnable { "put a token away in the bag." } else { "destroyed a token in the bag." };
        self.log(acting_player_id, message);
        self.notify();
        Ok(())
    }

    pub fn attach_widget_to_card(
        &mut self,
        instance_id: &str,
        card_id: &str,
        x: f64,
        y: f64,
        acting_player_id: &str,
    ) -> SessionResult {
        let widget_kind = self.find_widget(instance_id).map(|widget| widget.kind);
        let card = self.find_card(card_id).cloned();
        self.state = actions::attach_widget_to_card(&self.state, instance_id, card_id, x, y);
        let kind_label = match widget_kind {
            Some(BoardWidgetKind::SimpleCounter) => "a counter",
            Some(BoardWidgetKind::Token) => "a token",
            _ => "a widget",
        };
        let name = self.visible_title(card.as_ref())?;
        self.log(acting_player_id, &format!("attached {kind_label} to {}.", card_text(name.as_deref())));
        self.notify();
        Ok(())
    }

    /// Opens a booster pack from a set into the acting player's hand.
    pub fn generate_pack(&mut self, set_id: &str, acting_player_id: &str) {
        let set = self.game.sets.iter().find(|set| set.id == set_id).cloned();
        let set_cards: Vec<CardDefinition> =
            self.game.cards().iter().filter(|card| card.set_id.as_deref() == Some(set_id)).cloned().collect();
        let pack_settings = set.as_ref().map(|set| set.pack_settings.as_slice()).unwrap_or(&[]);
        let selected: Vec<CardDefinition> =
            select_pack_cards(&set_cards, pack_settings, self.environment.random.as_mut())
                .into_iter()
                .cloned()
                .collect();
        let new_cards = selected
            .iter()
            .map(|definition| CardInstance {
                z_index: 0,
                face_up: true,
                owner_id: Some(acting_player_id.to_string()),
                unownable: definition.unownable,
                ..CardInstance::new((self.environment.new_instance_id)(), definition.id.clone(), CardZone::Hand)
            })
            .collect();
        self.state = actions::deal_new_cards(&self.state, new_cards);
        let set_name = set.map_or_else(|| set_id.to_string(), |set| set.name);
        self.log(acting_player_id, &format!("generated a pack from the {set_name} set."));
        self.notify();
    }

    /// Creates a card from the card library onto the table, owned by the acting player.
    pub fn create_card_from_library(&mut self, definition_id: &str, x: f64, y: f64, acting_player_id: &str) {
        let Some(definition) = self.game.card_by_id(definition_id) else { return };
        let (card_title, unownable) = (definition.card_title.clone(), definition.unownable);
        let new_card = CardInstance {
            x,
            y,
            z_index: 0,
            face_up: true,
            owner_id: Some(acting_player_id.to_string()),
            unownable,
            ..CardInstance::new((self.environment.new_instance_id)(), definition_id, CardZone::Table)
        };
        self.state = actions::deal_new_cards(&self.state, vec![new_card]);
        self.log(acting_player_id, &format!("created \"{card_title}\" from the card library."));
        self.notify();
    }

    // -----------------------------------------------------------------------
    // Networking hooks
    // -----------------------------------------------------------------------

    /// Client side: adopts a newer state from the host (older or equal revisions are ignored).
    pub fn apply_remote_state(&mut self, new_state: TableState) {
        if new_state.revision <= self.state.revision {
            return;
        }
        self.state = new_state;
        self.notify();
    }

    /// Host-only (checked by the host engine).
    pub fn set_shared_playmat(&mut self, on: bool, index: i64) {
        self.state.shared_playmat_on = on;
        self.state.shared_playmat_index = index;
        self.state.revision += 1;
        self.notify();
    }

    /// Cosmetic turn marker; anyone may pass it. `None` leaves the marker where it is (the
    /// original's behaviour for a request without a player id) but still bumps the revision.
    pub fn set_current_player(&mut self, player_id: Option<&str>) {
        if let Some(player_id) = player_id {
            self.state.current_player_id = Some(player_id.to_string());
        }
        self.state.revision += 1;
        self.notify();
    }

    /// Each player may only change their own playmat (checked by the host engine).
    pub fn set_individual_playmat(&mut self, player_id: &str, on: bool, index: i64) {
        for player in self.state.players.iter_mut().filter(|player| player.id == player_id) {
            player.individual_playmat_on = on;
            player.individual_playmat_index = index;
        }
        self.state.revision += 1;
        self.notify();
    }

    /// Host side: marks players connected or disconnected (the host is always connected) and logs
    /// each change.
    pub fn sync_connected_player_ids(&mut self, connected_non_host_ids: &HashSet<String>) {
        let changes: Vec<(String, bool)> = self
            .state
            .players
            .iter()
            .map(|player| {
                (player.id.clone(), player.role == PlayerRole::Host || connected_non_host_ids.contains(&player.id))
            })
            .collect();
        for (index, (player_id, now_connected)) in changes.into_iter().enumerate() {
            if self.state.players[index].connected != now_connected {
                self.log(&player_id, if now_connected { "connected." } else { "disconnected." });
            }
            self.state.players[index].connected = now_connected;
        }
        self.state.revision += 1;
        self.notify();
    }
}
