//! The table's context menus and dialogs. Ports of the context-menu and dialog parts of
//! `src/ui/table/table_screen.lua`: card, pile, selection, zone, widget and board menus; Set
//! Value / Colors / Text, Change Tokens, Load Deck, Save Deck, Load Scenario, and the
//! confirmations (give cards, leave, kick).

use gametable_core::deck_widget_zones::build_deck_widget_zone_id;
use gametable_core::models::{
    BOARD_WIDGET_COUNTER_MAXIMUM, BOARD_WIDGET_COUNTER_MINIMUM, BoardKind, BoardWidgetInstance, BoardWidgetKind,
    CardInstance, CardZone, DeckConfig, DeckEntry, SubDeck, WidgetSizeOverride, ZoneDefinition, ZoneKind,
};
use gametable_core::util::trim_lua_whitespace;
use gametable_core::uuid::new_v4_uuid;

use crate::app::Frame;
use crate::data::deck_store::{DeckStore, LocalDeckStore};
use crate::data::file_system::join_path;
use crate::data::save_files::{format_log, local_now, log_file_name, write_log};
use crate::screens::deck_library_dialog::{DeckLibraryDialog, LoadDeckResponse, SaveDeckDialog, SaveDeckResponse};
use crate::screens::info_dialogs::show_controls_dialog;
use crate::screens::settings_dialog::{GameColors, SettingsDialog};
use crate::screens::token_choosers::{ChooserResponse, STEPPER_WIDTH, TokenChooser, token_folder_images};
use crate::sound::SoundEffect;
use crate::table::actions::TableAction;
use crate::table::board_layout::{BoardToggle, board_at, board_toggle, default_shared_board};
use crate::table::screen::{TableEvent, TableScreen, top_card};
use crate::table::view::{HitItem, ItemKind};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{
    ConfirmResponse, ContextMenu, DialogAction, DialogResponse, DialogSpec, MenuItem, MenuResponse,
};
use crate::ui::text_field::{TextFieldOptions, text_field_height};
use crate::ui::theme::with_alpha;
use crate::ui::widgets::{ButtonOptions, ButtonVariant};

/// Where a card goes in a zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Top,
    ShuffleInto,
    Bottom,
}

/// What a table menu item does.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    CardsToHand {
        instance_ids: Vec<String>,
        play_sound: bool,
    },
    CardsToZone {
        instance_ids: Vec<String>,
        zone_id: String,
        placement: Placement,
    },
    SearchPile(String),
    SearchZone(String),
    /// Shuffles one of the player's (or a shared) card zones and wiggles its pile.
    ShuffleZone {
        zone_id: String,
        wiggle_key: String,
    },
    /// Puts the zones moved on the board holding this zone back where the game places them.
    ResetBoardZonePositions {
        owner_key: String,
        zone_id: String,
    },
    /// Shows or hides the handle of one of this player's boards.
    SwitchBoardLock(i64),
    /// Puts one of this player's boards back where the game puts it.
    ResetBoardPosition(i64),
    /// F12 from the table menu: shows or hides the toggleable boards.
    ToggleBoards,
    /// Picks the player board to use (1-based), while none is placed.
    ChoosePlayerBoard(i64),
    /// The host picks the shared board (1-based), in a game with only shared boards.
    ChooseSharedBoard(i64),
    FlipCards(Vec<String>),
    RotateRoots {
        root_ids: Vec<String>,
        clockwise: bool,
    },
    GiveCards {
        instance_ids: Vec<String>,
        new_owner_id: Option<String>,
    },
    CreatePileAt(f32, f32),
    OpenTokenPicker,
    CreateWidget {
        kind: BoardWidgetKind,
        x: f64,
        y: f64,
    },
    OpenLibrary,
    PassTurn(String),
    CounterStep {
        widget_id: String,
        value: f64,
    },
    SetValue(BoardWidgetInstance),
    SetColors(BoardWidgetInstance),
    SetTokenColor(BoardWidgetInstance),
    UpdateText(BoardWidgetInstance),
    TokenSize {
        widget_id: String,
        size: Option<f64>,
    },
    FlipWidget(String),
    DeleteWidget(String),
    GeneratePack(String),
    LoadDeck(ZoneDefinition),
    ChangeTokens(BoardWidgetInstance),
    ResetBag(String),
    ReturnBagToken {
        widget_id: String,
        slot_id: String,
    },
}

/// An open context menu.
pub struct TableMenu {
    pub menu: ContextMenu<MenuAction>,
}

/// What a token chooser over Change Tokens fills in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChooserTarget {
    Entry(usize),
    NewEntry,
}

/// The table's open dialog.
pub enum TableDialog {
    SetValue {
        widget_id: String,
        text: String,
        error: Option<String>,
    },
    SetColors {
        widget_id: String,
        background: u32,
        text: u32,
    },
    SetTokenColor {
        widget_id: String,
        color: u32,
        text_color: u32,
    },
    SetTokenText {
        widget_id: String,
        text: String,
        color: u32,
        is_card: bool,
    },
    ChangeTokens {
        widget_id: String,
        entries: Vec<(Option<String>, i64)>,
        names: Vec<(String, String)>,
    },
    TokenChooser {
        chooser: TokenChooser,
        target: ChooserTarget,
    },
    LoadDeck {
        zone_id: String,
        dialog: Box<DeckLibraryDialog>,
    },
    SaveDeck(Box<SaveDeckDialog>),
    LoadScenario,
    Settings(SettingsDialog),
    Controls,
    ConfirmGive {
        instance_ids: Vec<String>,
        new_owner_id: Option<String>,
        title: String,
        message: String,
        confirm: String,
    },
    ConfirmLeave,
    ConfirmKick {
        player_id: String,
        name: String,
    },
}

fn card_count_text(count: usize) -> String {
    if count == 1 { "1 card".to_string() } else { format!("{count} cards") }
}

/// `groupBagRemaining`: a bag's flat contents as (file, quantity) in first-seen order.
fn group_bag_remaining(remaining: &[String]) -> Vec<(Option<String>, i64)> {
    let mut grouped: Vec<(Option<String>, i64)> = Vec::new();
    for file in remaining {
        match grouped.iter_mut().find(|(existing, _)| existing.as_deref() == Some(file.as_str())) {
            Some((_, quantity)) => *quantity += 1,
            None => grouped.push((Some(file.clone()), 1)),
        }
    }
    grouped
}

/// `flattenBagContents`: the inverse of [`group_bag_remaining`] (unchosen entries skipped).
fn flatten_bag_contents(entries: &[(Option<String>, i64)]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|(file, quantity)| {
            file.as_ref().map(|file| std::iter::repeat_n(file.clone(), (*quantity).max(0) as usize))
        })
        .flatten()
        .collect()
}

impl TableScreen {
    fn placement_menu(instance_ids: &[String], zone: &ZoneDefinition) -> Vec<MenuItem<MenuAction>> {
        let action = |placement| MenuAction::CardsToZone {
            instance_ids: instance_ids.to_vec(),
            zone_id: zone.id.clone(),
            placement,
        };
        let mut items = vec![MenuItem::new("Top", action(Placement::Top))];
        if zone.shuffleable {
            items.push(MenuItem::new("Shuffle Into", action(Placement::ShuffleInto)));
        }
        items.push(MenuItem::new("Bottom", action(Placement::Bottom)));
        items
    }

    fn rotate_menu(root_ids: &[String]) -> Vec<MenuItem<MenuAction>> {
        vec![
            MenuItem::new("Clockwise", MenuAction::RotateRoots { root_ids: root_ids.to_vec(), clockwise: true }),
            MenuItem::new(
                "Counterclockwise",
                MenuAction::RotateRoots { root_ids: root_ids.to_vec(), clockwise: false },
            ),
        ]
    }

    /// "Send to..." entries: Hand (unless excluded), Discard, then every other card zone.
    fn send_to_menu(
        &self,
        instance_ids: &[String],
        include_hand: bool,
        discard_owner: Option<&str>,
        play_sound: bool,
    ) -> Vec<MenuItem<MenuAction>> {
        let mut send = Vec::new();
        if include_hand {
            send.push(MenuItem::new(
                "Hand",
                MenuAction::CardsToHand { instance_ids: instance_ids.to_vec(), play_sound },
            ));
        }
        if let Some(discard) = self.local_discard_zone_id(discard_owner).and_then(|id| self.zone_by_id(&id)) {
            send.push(MenuItem::submenu("Discard", TableScreen::placement_menu(instance_ids, &discard)));
        }
        for zone in self.effective_zones() {
            if zone.kind == ZoneKind::Card && !zone.is_discard_pile {
                send.push(MenuItem::submenu(zone.name.clone(), TableScreen::placement_menu(instance_ids, zone)));
            }
        }
        send
    }

    /// `T:cardMenuItems(card, onSearch)`; `None` when the player can't act on the card at all.
    fn card_menu_items(&self, card: &CardInstance, search_root: Option<&str>) -> Option<Vec<MenuItem<MenuAction>>> {
        let acting = self.acting_player_id.as_str();
        // An unowned card is fair game for any player.
        let can_act = card.owner_id.as_deref().is_none_or(|owner| owner == acting);
        if !can_act && search_root.is_none() {
            return None;
        }
        let mut items = Vec::new();
        let ids = vec![card.instance_id.clone()];
        if can_act {
            let send = self.send_to_menu(&ids, card.zone != CardZone::Hand, card.owner_id.as_deref(), false);
            items.push(MenuItem::submenu("Send to...", send));
        }
        if let Some(root) = search_root {
            items.push(MenuItem::new("Search...", MenuAction::SearchPile(root.to_string())));
        }
        if can_act && card.zone == CardZone::Table {
            let root_id = self.root_id_of(card);
            items.push(MenuItem::new("Flip", MenuAction::FlipCards(ids.clone())));
            items.push(MenuItem::submenu("Rotate", TableScreen::rotate_menu(&[root_id])));
            // Cards in a pile can't change owner (piles never mix owners).
            if !self.backend.session().is_in_pile(&card.instance_id) {
                for player in self.state.players.iter().filter(|player| player.id != acting && player.connected) {
                    let action =
                        MenuAction::GiveCards { instance_ids: ids.clone(), new_owner_id: Some(player.id.clone()) };
                    items.push(MenuItem::new(format!("Give to {}", player.name), action));
                }
                items.push(MenuItem::new(
                    "Remove Ownership",
                    MenuAction::GiveCards { instance_ids: ids, new_owner_id: None },
                ));
            }
        }
        Some(items)
    }

    /// Right-click on a selected pile while 2+ items are selected: acts on every selected card.
    fn selection_menu_items(&self, x: f32, y: f32) -> Vec<MenuItem<MenuAction>> {
        let cards = self.selected_cards();
        let roots = self.selected_root_list();
        let token_count = self.selected_token_ids().len();
        let mut header = card_count_text(cards.len());
        if token_count > 0 {
            header.push_str(&format!(", {token_count} token{}", if token_count == 1 { "" } else { "s" }));
        }
        let mut items = vec![
            MenuItem::<MenuAction>::submenu(format!("{header} selected"), Vec::new()).disabled(true),
            MenuItem::divider(),
        ];
        if cards.is_empty() {
            return items;
        }
        let ids: Vec<String> = cards.iter().map(|card| card.instance_id.clone()).collect();
        let send = self.send_to_menu(&ids, true, cards[0].owner_id.as_deref(), true);
        items.push(MenuItem::submenu("Send to...", send));
        if cards.len() >= 2 {
            items.push(MenuItem::new("Create Pile Here", MenuAction::CreatePileAt(x, y)));
        }
        let tops: Vec<String> = roots
            .iter()
            .filter_map(|root| top_card(&self.derived.pile_groups[root]).map(|top| top.instance_id.clone()))
            .collect();
        items.push(MenuItem::new("Flip", MenuAction::FlipCards(tops)));
        items.push(MenuItem::submenu("Rotate", TableScreen::rotate_menu(&roots)));
        // Ownership changes only when no selected card is in a pile.
        if roots.iter().any(|root| self.derived.pile_groups[root].len() > 1) {
            return items;
        }
        let acting = self.acting_player_id.as_str();
        for player in self.state.players.iter().filter(|player| player.id != acting && player.connected) {
            let action = MenuAction::GiveCards { instance_ids: ids.clone(), new_owner_id: Some(player.id.clone()) };
            items.push(MenuItem::new(format!("Give to {}", player.name), action));
        }
        items.push(MenuItem::new("Remove Ownership", MenuAction::GiveCards { instance_ids: ids, new_owner_id: None }));
        items
    }

    fn open_menu(&mut self, x: f32, y: f32, items: Vec<MenuItem<MenuAction>>) {
        self.menu = Some(TableMenu { menu: ContextMenu::new("table:menu", x, y, items) });
    }

    /// `T:openBoardMenu`: Tokens, Widgets, Card Library and Pass Turn To.
    pub(super) fn open_board_menu(&mut self, _frame: &Frame, x: f32, y: f32) {
        if self.spectator {
            return;
        }
        let (fraction_x, fraction_y) = self.screen_to_canonical(x, y, None);
        let create = |kind| MenuAction::CreateWidget { kind, x: fraction_x, y: fraction_y };
        let mut widget_items = vec![MenuItem::new("Simple Counter", create(BoardWidgetKind::SimpleCounter))];
        // Collection-mode games only use cards players own.
        if !self.collection_mode {
            widget_items.push(MenuItem::new("Pack Generator", create(BoardWidgetKind::PackGenerator)));
        }
        if self.game.deck_building_zones().next().is_some() {
            widget_items.push(MenuItem::new("Deck Widget", create(BoardWidgetKind::DeckBuilder)));
        }
        let pass_turn: Vec<MenuItem<MenuAction>> = self
            .state
            .players
            .iter()
            .filter(|player| player.connected)
            .map(|player| {
                MenuItem::new(player.name.clone(), MenuAction::PassTurn(player.id.clone()))
                    .checked(self.state.current_player_id.as_deref() == Some(player.id.as_str()))
            })
            .collect();
        let mut items = vec![
            MenuItem::new("Tokens", MenuAction::OpenTokenPicker),
            MenuItem::submenu("Widgets", widget_items),
            MenuItem::new("Card Library", MenuAction::OpenLibrary),
            MenuItem::submenu("Pass Turn To", pass_turn),
        ];
        items.extend(self.board_menu_items());
        // Over one of this player's own boards: lock or unlock its handle, or put it back.
        if let Some(board_index) = self.my_board_under(x, y) {
            items.push(MenuItem::divider());
            let locked = !self.unlocked_boards.contains(&board_index);
            let label = if locked { "Unlock Board" } else { "Lock Board" };
            let icon = if locked { "lock_open" } else { "lock" };
            items.push(MenuItem::new(label, MenuAction::SwitchBoardLock(board_index)).icon(icon));
            if self.has_moved_board(board_index) {
                items.push(MenuItem::new("Reset Board Position", MenuAction::ResetBoardPosition(board_index)));
            }
        }
        self.open_menu(x, y, items);
    }

    /// The table menu's game board entries: Show/Hide Boards (what F12 does), and while no board
    /// is placed, a Player Board submenu to pick which player board to use (the host's Shared
    /// Board submenu, in a game with only shared boards).
    fn board_menu_items(&self) -> Vec<MenuItem<MenuAction>> {
        let mut items = Vec::new();
        if let Some(shown) = self.board_toggle_state() {
            let label = if shown { "Hide Boards (F12)" } else { "Show Boards (F12)" };
            items.push(MenuItem::new(label, MenuAction::ToggleBoards));
        }
        let boards_of = |kind: BoardKind| -> Vec<i64> {
            (1..=self.game.game_boards.len() as i64)
                .filter(|&board_index| board_at(&self.game, board_index).is_some_and(|board| board.kind == kind))
                .collect()
        };
        match board_toggle(&self.game) {
            BoardToggle::DefaultPlayerBoard if self.player_by_id(&self.acting_player_id).is_some() => {
                let current = self
                    .player_by_id(&self.acting_player_id)
                    .filter(|me| me.player_boards_shown)
                    .and_then(|_| self.current_player_board());
                let choices: Vec<MenuItem<MenuAction>> = boards_of(BoardKind::Player)
                    .into_iter()
                    .map(|board_index| {
                        MenuItem::new(self.board_name(board_index), MenuAction::ChoosePlayerBoard(board_index))
                            .checked(current == Some(board_index))
                    })
                    .collect();
                if choices.len() > 1 {
                    items.push(MenuItem::submenu("Player Board", choices));
                }
            }
            BoardToggle::DefaultSharedBoard if self.can_load_scenario => {
                let current = self
                    .state
                    .shared_boards_shown
                    .then(|| default_shared_board(&self.game, self.state.chosen_shared_board))
                    .flatten();
                let choices: Vec<MenuItem<MenuAction>> = boards_of(BoardKind::Shared)
                    .into_iter()
                    .map(|board_index| {
                        MenuItem::new(self.board_name(board_index), MenuAction::ChooseSharedBoard(board_index))
                            .checked(current == Some(board_index))
                    })
                    .collect();
                if choices.len() > 1 {
                    items.push(MenuItem::submenu("Shared Board", choices));
                }
            }
            _ => {}
        }
        if !items.is_empty() {
            items.insert(0, MenuItem::divider());
        }
        items
    }

    fn counter_menu_items(widget: &BoardWidgetInstance, include_delete: bool) -> Vec<MenuItem<MenuAction>> {
        let id = widget.instance_id.clone();
        let mut items = vec![
            MenuItem::new("Increment", MenuAction::CounterStep { widget_id: id.clone(), value: widget.value + 1.0 }),
            MenuItem::new("Decrement", MenuAction::CounterStep { widget_id: id.clone(), value: widget.value - 1.0 }),
            MenuItem::new("Set Value", MenuAction::SetValue(widget.clone())),
            MenuItem::new("Set Colors", MenuAction::SetColors(widget.clone())),
        ];
        if include_delete {
            items.push(MenuItem::new("Delete", MenuAction::DeleteWidget(id)));
        }
        items
    }

    /// "Size": Default, Medium (2×) and Large (4×) of the kind's default size.
    fn token_size_menu(&self, widget: &BoardWidgetInstance) -> Vec<MenuItem<MenuAction>> {
        let base = f64::from(self.widget_size(widget.kind).0);
        [("Default", None), ("Medium", Some(base * 2.0)), ("Large", Some(base * 4.0))]
            .into_iter()
            .map(|(label, size)| {
                let current = match widget.size_override {
                    Some(WidgetSizeOverride::Multiplier(value)) => Some(value),
                    _ => None,
                };
                let checked = widget.size_override.is_none_or(|over| matches!(over, WidgetSizeOverride::Multiplier(_)))
                    && current == size;
                MenuItem::new(label, MenuAction::TokenSize { widget_id: widget.instance_id.clone(), size })
                    .checked(checked)
            })
            .collect()
    }

    /// `T:openItemMenu`: true when the item had a menu.
    pub(super) fn open_item_menu(&mut self, _frame: &Frame, item: &HitItem, x: f32, y: f32) -> bool {
        if self.spectator {
            return true;
        }
        match &item.kind {
            ItemKind::TableCard { root_id, .. } | ItemKind::Pile { root_id, .. }
                if self.selected_roots.contains(root_id) && self.selected_count >= 2 =>
            {
                let items = self.selection_menu_items(x, y);
                self.open_menu(x, y, items);
                true
            }
            ItemKind::TableCard { card, .. } | ItemKind::HandCard { card, .. } => {
                match self.card_menu_items(card, None) {
                    Some(items) => {
                        self.open_menu(x, y, items);
                        true
                    }
                    None => false,
                }
            }
            ItemKind::Pile { card, root_id, owned_by_opponent, .. } => {
                let search = (!owned_by_opponent).then_some(root_id.as_str());
                match self.card_menu_items(card, search) {
                    Some(items) => {
                        self.open_menu(x, y, items);
                        true
                    }
                    None => false,
                }
            }
            ItemKind::Zone { zone_id, zone, cards, shared, deck_widget, shuffle_wiggle_key, .. } => {
                let mut items = vec![MenuItem::new("Search...", MenuAction::SearchZone(zone_id.clone()))];
                if let Some(wiggle_key) = shuffle_wiggle_key
                    && zone.shuffleable
                    && cards.len() > 1
                {
                    items.push(MenuItem::new(
                        "Shuffle Deck",
                        MenuAction::ShuffleZone { zone_id: zone_id.clone(), wiggle_key: wiggle_key.clone() },
                    ));
                }
                if !shared && !deck_widget && zone.deals_built_deck && cards.is_empty() {
                    items.push(MenuItem::new("Load Deck...", MenuAction::LoadDeck(zone.clone())));
                }
                if let Some((owner_key, zone_id)) = self.movable_board_zone(item)
                    && self.has_moved_board_zones(&owner_key, &zone_id)
                {
                    items.push(MenuItem::new(
                        "Reset Zone Positions",
                        MenuAction::ResetBoardZonePositions { owner_key, zone_id },
                    ));
                }
                self.open_menu(x, y, items);
                true
            }
            ItemKind::Counter { widget } if item.interactable => {
                self.open_menu(x, y, TableScreen::counter_menu_items(widget, true));
                true
            }
            ItemKind::DockedCounter { widget } if item.interactable => {
                self.open_menu(x, y, TableScreen::counter_menu_items(widget, false));
                true
            }
            ItemKind::Token { widget } if item.interactable => {
                let mut items = Vec::new();
                if widget.kind != BoardWidgetKind::ImageToken {
                    items.push(MenuItem::new("Set Color", MenuAction::SetTokenColor(widget.clone())));
                    items.push(MenuItem::new("Update Text", MenuAction::UpdateText(widget.clone())));
                }
                if widget.kind != BoardWidgetKind::CardToken {
                    items.push(MenuItem::submenu("Size", self.token_size_menu(widget)));
                }
                items.push(MenuItem::new("Flip", MenuAction::FlipWidget(widget.instance_id.clone())));
                items.push(MenuItem::new("Delete", MenuAction::DeleteWidget(widget.instance_id.clone())));
                self.open_menu(x, y, items);
                true
            }
            ItemKind::PackGenerator { widget } if item.interactable => {
                let mut items = Vec::new();
                if !self.collection_mode {
                    for set in &self.game.sets {
                        items.push(MenuItem::new(set.name.clone(), MenuAction::GeneratePack(set.id.clone())));
                    }
                }
                items.push(MenuItem::new("Delete", MenuAction::DeleteWidget(widget.instance_id.clone())));
                self.open_menu(x, y, items);
                true
            }
            ItemKind::DeckBody { widget } | ItemKind::DeckHeader { widget }
                if widget.owner_id.as_deref() == Some(self.acting_player_id.as_str()) && !self.tab_down =>
            {
                self.open_menu(
                    x,
                    y,
                    vec![MenuItem::new("Delete", MenuAction::DeleteWidget(widget.instance_id.clone()))],
                );
                true
            }
            ItemKind::TokenBagIdle { widget } if item.interactable => {
                let items = vec![
                    MenuItem::new("Change Tokens...", MenuAction::ChangeTokens(widget.clone())),
                    MenuItem::new("Reset Bag", MenuAction::ResetBag(widget.instance_id.clone())),
                ];
                self.open_menu(x, y, items);
                true
            }
            ItemKind::RevealedBagToken { widget, slot_id, .. } if item.interactable => {
                let action =
                    MenuAction::ReturnBagToken { widget_id: widget.instance_id.clone(), slot_id: slot_id.clone() };
                self.open_menu(x, y, vec![MenuItem::new("Return to Bag", action)]);
                true
            }
            _ => false,
        }
    }

    /// Runs a chosen menu item.
    fn run_menu_action(&mut self, frame: &mut Frame, action: MenuAction) {
        match action {
            MenuAction::CardsToHand { instance_ids, play_sound } => {
                for instance_id in instance_ids {
                    self.perform(TableAction::MoveToHand { instance_id });
                }
                if play_sound {
                    frame.sounds.play(SoundEffect::Draw);
                }
            }
            MenuAction::CardsToZone { instance_ids, zone_id, placement } => {
                let cards: Vec<CardInstance> =
                    instance_ids.iter().filter_map(|id| self.derived.cards_by_id.get(id).cloned()).collect();
                self.send_cards_to_zone(&cards, &zone_id, placement == Placement::Bottom);
                if placement == Placement::ShuffleInto {
                    self.perform(TableAction::ShuffleZone { zone_id });
                }
            }
            MenuAction::SearchPile(root_instance_id) => self.perform(TableAction::StartSearchPile { root_instance_id }),
            MenuAction::SearchZone(zone_id) => self.perform(TableAction::StartSearchZone { zone_id }),
            MenuAction::ShuffleZone { zone_id, wiggle_key } => self.shuffle_zone(frame, &zone_id, Some(wiggle_key)),
            MenuAction::FlipCards(instance_ids) => {
                for instance_id in instance_ids {
                    self.perform(TableAction::FlipCard { instance_id });
                }
            }
            MenuAction::RotateRoots { root_ids, clockwise } => {
                for root_instance_id in root_ids {
                    self.perform(TableAction::RotateStack { root_instance_id, clockwise, step: 1.0 });
                }
                frame.sounds.play(if clockwise { SoundEffect::Tap } else { SoundEffect::Untap });
            }
            MenuAction::GiveCards { instance_ids, new_owner_id } => self.confirm_give(instance_ids, new_owner_id),
            MenuAction::CreatePileAt(x, y) => self.create_pile_at(x, y),
            MenuAction::OpenTokenPicker => self.token_picker_open = true,
            MenuAction::CreateWidget { kind, x, y } => {
                self.perform(TableAction::CreateWidget { instance_id: new_v4_uuid(), kind, x, y, image_path: None })
            }
            MenuAction::OpenLibrary => self.library_open = true,
            MenuAction::PassTurn(player_id) => self.perform(TableAction::SetCurrentPlayer { player_id }),
            MenuAction::CounterStep { widget_id, value } => {
                self.perform(TableAction::SetWidgetValue { instance_id: widget_id, value })
            }
            MenuAction::SetValue(widget) => {
                let text = crate::table::panels::lua_number_text(widget.value);
                self.dialog = Some(TableDialog::SetValue { widget_id: widget.instance_id, text, error: None });
            }
            MenuAction::SetColors(widget) => {
                self.dialog = Some(TableDialog::SetColors {
                    widget_id: widget.instance_id,
                    background: widget.background_color,
                    text: widget.text_color,
                });
            }
            MenuAction::SetTokenColor(widget) => {
                self.dialog = Some(TableDialog::SetTokenColor {
                    widget_id: widget.instance_id,
                    color: widget.background_color,
                    text_color: widget.text_color,
                });
            }
            MenuAction::UpdateText(widget) => {
                self.dialog = Some(TableDialog::SetTokenText {
                    widget_id: widget.instance_id,
                    text: widget.text.clone(),
                    color: widget.text_color,
                    is_card: widget.kind == BoardWidgetKind::CardToken,
                });
            }
            MenuAction::TokenSize { widget_id, size } => self.perform(TableAction::SetWidgetSize {
                instance_id: widget_id,
                size_override: size.map(WidgetSizeOverride::Multiplier),
            }),
            MenuAction::FlipWidget(instance_id) => self.perform(TableAction::FlipWidget { instance_id }),
            MenuAction::DeleteWidget(instance_id) => self.perform(TableAction::DeleteWidget { instance_id }),
            MenuAction::GeneratePack(set_id) => self.perform(TableAction::GeneratePack { set_id }),
            MenuAction::LoadDeck(zone) => self.prompt_load_deck(frame, zone),
            MenuAction::ResetBoardZonePositions { owner_key, zone_id } => {
                self.reset_board_zone_positions(frame, &owner_key, &zone_id)
            }
            MenuAction::ToggleBoards => self.toggle_boards(frame),
            MenuAction::SwitchBoardLock(board_index) => self.switch_board_lock(board_index),
            MenuAction::ResetBoardPosition(board_index) => self.reset_board_position(frame, board_index),
            MenuAction::ChoosePlayerBoard(board_index) => self.choose_player_board(frame, board_index),
            MenuAction::ChooseSharedBoard(board_index) => self.choose_shared_board(frame, board_index),
            MenuAction::ChangeTokens(widget) => self.prompt_change_tokens(&widget),
            MenuAction::ResetBag(instance_id) => self.perform(TableAction::ResetBag { instance_id }),
            MenuAction::ReturnBagToken { widget_id, slot_id } => {
                self.perform(TableAction::ReturnBagToken { instance_id: widget_id, slot_id })
            }
        }
    }

    /// Confirms before cards leave your ownership.
    fn confirm_give(&mut self, instance_ids: Vec<String>, new_owner_id: Option<String>) {
        let what = if instance_ids.len() == 1 {
            let card = self.derived.cards_by_id.get(&instance_ids[0]);
            let definition = card.filter(|card| card.face_up).and_then(|card| self.definition(&card.definition_id));
            definition.map_or_else(|| "this card".to_string(), |definition| format!("\"{}\"", definition.card_title))
        } else {
            card_count_text(instance_ids.len())
        };
        let (title, message, confirm) = match &new_owner_id {
            Some(owner) => {
                let name = self.player_by_id(owner).map_or("that player".to_string(), |player| player.name.clone());
                ("Give Cards".to_string(), format!("Give {what} to {name}?"), "Give".to_string())
            }
            None => (
                "Remove Ownership".to_string(),
                format!(
                    "Remove your ownership of {what}? Any player will be able to move {}.",
                    if instance_ids.len() == 1 { "it" } else { "them" }
                ),
                "Remove".to_string(),
            ),
        };
        self.dialog = Some(TableDialog::ConfirmGive { instance_ids, new_owner_id, title, message, confirm });
    }

    /// The deck store to use: the table's own, or the local deck library.
    fn take_deck_store(&mut self, frame: &Frame) -> Box<dyn DeckStore> {
        self.deck_store.take().unwrap_or_else(|| {
            Box::new(LocalDeckStore::new(frame.paths.decks_folder(frame.settings).to_string_lossy().into_owned()))
        })
    }

    fn return_deck_store(&mut self, store: Box<dyn DeckStore>, was_own: bool) {
        if was_own {
            self.deck_store = Some(store);
        }
    }

    pub(super) fn open_load_deck_for_development(&mut self, frame: &mut Frame, zone: ZoneDefinition) {
        self.prompt_load_deck(frame, zone);
    }

    fn prompt_load_deck(&mut self, frame: &mut Frame, zone: ZoneDefinition) {
        let was_own = self.deck_store.is_some();
        let mut store = self.take_deck_store(frame);
        let dialog = DeckLibraryDialog::new(frame, self.game.clone(), vec![zone.clone()], store.as_mut(), None);
        self.return_deck_store(store, was_own);
        self.dialog = Some(TableDialog::LoadDeck { zone_id: zone.id, dialog: Box::new(dialog) });
    }

    /// Save Deck on a deck widget: its sub-zones' cards tallied into a deck.
    pub(super) fn save_deck_widget(&mut self, frame: &mut Frame, widget: &BoardWidgetInstance) {
        let mut subdecks = Vec::new();
        for zone in self.game.deck_building_zones() {
            let synthetic_id = build_deck_widget_zone_id(&widget.instance_id, &zone.id);
            let mut entries: Vec<DeckEntry> = Vec::new();
            for card in self.state.cards() {
                if card.zone == CardZone::Zone && card.zone_id.as_deref() == Some(synthetic_id.as_str()) {
                    match entries.iter_mut().find(|entry| entry.definition_id == card.definition_id) {
                        Some(entry) => entry.quantity += 1,
                        None => entries.push(DeckEntry::new(card.definition_id.clone(), 1)),
                    }
                }
            }
            subdecks.push(SubDeck::new(zone.deck_type.clone(), entries));
        }
        let deck = DeckConfig::new(self.game.id.clone(), subdecks);
        let was_own = self.deck_store.is_some();
        let mut store = self.take_deck_store(frame);
        let dialog = SaveDeckDialog::new(frame, &self.game, deck, store.as_mut(), false);
        self.return_deck_store(store, was_own);
        self.dialog = Some(TableDialog::SaveDeck(Box::new(dialog)));
    }

    fn prompt_change_tokens(&mut self, widget: &BoardWidgetInstance) {
        let names: Vec<(String, String)> = if self.game.uses_tokens && !self.game.tokens.is_empty() {
            self.game
                .tokens
                .iter()
                .map(|token| (token.display_name().to_string(), token.image_path.clone().unwrap_or_default()))
                .collect()
        } else {
            token_folder_images(self.game.folder_path.as_deref(), "", "_tokens")
                .0
                .into_iter()
                .map(|image| (image.name, image.file))
                .collect()
        };
        self.dialog = Some(TableDialog::ChangeTokens {
            widget_id: widget.instance_id.clone(),
            entries: group_bag_remaining(&widget.bag_remaining),
            names,
        });
    }

    /// `T:saveLog()`: writes the log into the game's `gamelogs` folder.
    pub(super) fn save_log(&mut self, frame: &mut Frame) {
        let Some(folder) = self.game.folder_path.clone() else { return };
        let name = log_file_name(&local_now());
        match write_log(&folder, &name, &format_log(&self.state.log)) {
            Ok(_) => frame.ui.toast(&format!("Saved \"{name}\""), None),
            Err(error) => frame.ui.toast(&format!("Could not save the log: {error}"), None),
        }
    }

    /// Opens a dialog from the Game Menu.
    pub(super) fn open_dialog(&mut self, dialog: TableDialog) {
        self.dialog = Some(dialog);
    }

    pub(super) fn settings_dialog(&self) -> TableDialog {
        let mut dialog = SettingsDialog::new();
        dialog.in_game = true;
        dialog.game_colors = GameColors {
            table_color: self.game.table_color.clone(),
            current_turn_color: self.game.current_turn_color.clone(),
        };
        TableDialog::Settings(dialog)
    }

    /// Shows the open menu and dialogs and applies what they report.
    pub(super) fn show_menus_and_dialogs(&mut self, frame: &mut Frame) {
        if let Some(mut table_menu) = self.menu.take() {
            match table_menu.menu.show(frame) {
                MenuResponse::Open => self.menu = Some(table_menu),
                MenuResponse::Selected(action) => self.run_menu_action(frame, action),
                MenuResponse::Closed => {}
            }
        }
        if let Some(dialog) = self.dialog.take() {
            self.dialog = self.show_dialog(frame, dialog);
        }
    }

    /// One frame of a dialog; returns it while it stays open.
    fn show_dialog(&mut self, frame: &mut Frame, dialog: TableDialog) -> Option<TableDialog> {
        let label_background = Some(frame.palette().surface_container_high);
        match dialog {
            TableDialog::SetValue { widget_id, mut text, mut error } => {
                let actions = [DialogAction::text("Cancel"), DialogAction::filled("Set")];
                let spec =
                    DialogSpec { title: Some("Set Value"), width: 360.0, actions: &actions, ..DialogSpec::default() };
                let mut submitted = false;
                let label =
                    format!("Value (0-{})", crate::table::panels::lua_number_text(BOARD_WIDGET_COUNTER_MAXIMUM));
                let response = frame.dialog("setvalue:dialog", &spec, |frame, x, y, width| {
                    let options = TextFieldOptions {
                        autofocus: true,
                        select_all_on_focus: true,
                        error: error.as_deref(),
                        label_background,
                        ..TextFieldOptions::label(&label)
                    };
                    let field = frame.text_field("setvalue", x, y + 8.0, width, &mut text, &options);
                    if field.changed {
                        error = None;
                    }
                    submitted = field.submitted;
                    8.0 + text_field_height(None, error.as_deref())
                });
                if submitted || response == DialogResponse::Action(1) {
                    let parsed = trim_lua_whitespace(&text).parse::<f64>().ok().filter(|value| value.fract() == 0.0);
                    match parsed {
                        Some(value) => {
                            let value = value.clamp(BOARD_WIDGET_COUNTER_MINIMUM, BOARD_WIDGET_COUNTER_MAXIMUM);
                            self.perform(TableAction::SetWidgetValue { instance_id: widget_id, value });
                            return None;
                        }
                        None => {
                            return Some(TableDialog::SetValue {
                                widget_id,
                                text,
                                error: Some("Enter a whole number".into()),
                            });
                        }
                    }
                }
                (response == DialogResponse::Open || response == DialogResponse::Enter)
                    .then_some(TableDialog::SetValue { widget_id, text, error })
            }
            TableDialog::SetColors { widget_id, mut background, mut text } => {
                let actions = [DialogAction::text("Cancel"), DialogAction::filled("Set")];
                let spec =
                    DialogSpec { title: Some("Set Colors"), width: 400.0, actions: &actions, ..DialogSpec::default() };
                let response = frame.dialog("setcolors:dialog", &spec, |frame, x, y, width| {
                    let (picked, height) = frame.color_field(
                        "setcolors:bg",
                        x,
                        y,
                        width,
                        background,
                        Some("Background Color"),
                        label_background,
                    );
                    background = picked;
                    let text_y = y + height + 16.0;
                    let (picked, height) = frame.color_field(
                        "setcolors:text",
                        x,
                        text_y,
                        width,
                        text,
                        Some("Text Color"),
                        label_background,
                    );
                    text = picked;
                    text_y + height - y
                });
                match response {
                    DialogResponse::Action(1) => {
                        self.perform(TableAction::SetWidgetColors {
                            instance_id: widget_id,
                            background_color: background,
                            text_color: text,
                        });
                        None
                    }
                    DialogResponse::Open | DialogResponse::Enter => {
                        Some(TableDialog::SetColors { widget_id, background, text })
                    }
                    _ => None,
                }
            }
            TableDialog::SetTokenColor { widget_id, mut color, text_color } => {
                let actions = [DialogAction::text("Cancel"), DialogAction::filled("Set")];
                let spec =
                    DialogSpec { title: Some("Set Color"), width: 400.0, actions: &actions, ..DialogSpec::default() };
                let response = frame.dialog("setcolor:dialog", &spec, |frame, x, y, width| {
                    let (picked, height) =
                        frame.color_field("setcolor:token", x, y, width, color, Some("Token Color"), label_background);
                    color = picked;
                    height
                });
                match response {
                    DialogResponse::Action(1) => {
                        self.perform(TableAction::SetWidgetColors {
                            instance_id: widget_id,
                            background_color: color,
                            text_color,
                        });
                        None
                    }
                    DialogResponse::Open | DialogResponse::Enter => {
                        Some(TableDialog::SetTokenColor { widget_id, color, text_color })
                    }
                    _ => None,
                }
            }
            TableDialog::SetTokenText { widget_id, mut text, mut color, is_card } => {
                let actions = [DialogAction::text("Cancel"), DialogAction::filled("Set")];
                let spec =
                    DialogSpec { title: Some("Set Text"), width: 400.0, actions: &actions, ..DialogSpec::default() };
                let response = frame.dialog("settext:dialog", &spec, |frame, x, y, width| {
                    let options = TextFieldOptions {
                        max_length: Some(if is_card { 120 } else { 24 }),
                        autofocus: true,
                        select_all_on_focus: true,
                        label_background,
                        ..TextFieldOptions::label("Text")
                    };
                    let field_height = if is_card {
                        let height = frame.text_area_height(3, None);
                        frame.text_area("settokentext", x, y, width, height, &mut text, &options);
                        height
                    } else {
                        frame.text_field("settokentext", x, y, width, &mut text, &options);
                        text_field_height(None, None)
                    };
                    let color_y = y + field_height + 16.0;
                    let (picked, height) = frame.color_field(
                        "settokentext:color",
                        x,
                        color_y,
                        width,
                        color,
                        Some("Text Color"),
                        label_background,
                    );
                    color = picked;
                    color_y + height - y
                });
                match response {
                    DialogResponse::Action(1) => {
                        let text = trim_lua_whitespace(&text).to_string();
                        self.perform(TableAction::SetWidgetText { instance_id: widget_id, text, text_color: color });
                        None
                    }
                    DialogResponse::Open | DialogResponse::Enter => {
                        Some(TableDialog::SetTokenText { widget_id, text, color, is_card })
                    }
                    _ => None,
                }
            }
            TableDialog::ChangeTokens { widget_id, mut entries, names } => {
                let actions = [DialogAction::text("Cancel"), DialogAction::filled("Set")];
                let spec = DialogSpec {
                    title: Some("Change Tokens"),
                    width: 480.0,
                    actions: &actions,
                    ..DialogSpec::default()
                };
                let folder = self.game.folder_path.clone();
                let mut choose: Option<ChooserTarget> = None;
                let row_height = 48.0;
                let response = frame.dialog("changetokens:dialog", &spec, |frame, x, y, width| {
                    let palette = frame.palette();
                    let mut row_y = y;
                    let mut remove = None;
                    for (index, (file, quantity)) in entries.iter_mut().enumerate() {
                        let key = format!("ctoken:bagentry{}", index + 1);
                        let pick_width = width - 8.0 - STEPPER_WIDTH;
                        let region_options = RegionOptions::hand().with_tooltip(Some("Choose Token..."));
                        let region =
                            frame.ui.region(&format!("{key}:pick"), x, row_y, pick_width, row_height, region_options);
                        if region.hovered {
                            frame.fill_rect(
                                x,
                                row_y,
                                pick_width,
                                row_height,
                                6.0,
                                with_alpha(palette.on_surface, 0.05),
                            );
                        }
                        let path = file
                            .as_deref()
                            .zip(folder.as_deref())
                            .map(|(file, folder)| join_path(&[folder, "_tokens", file]));
                        frame.image_preview_box(
                            path.as_deref(),
                            x + 4.0,
                            row_y + 4.0,
                            row_height - 8.0,
                            row_height - 8.0,
                        );
                        let label = match file {
                            Some(file) => names
                                .iter()
                                .find(|(_, image)| image == file)
                                .map_or(file.clone(), |(name, _)| name.clone()),
                            None => "(choose token)".to_string(),
                        };
                        let color = if file.is_some() { palette.on_surface } else { palette.on_surface_variant };
                        let options = TextOptions::new()
                            .width(pick_width - row_height - 12.0)
                            .middle_of(row_height)
                            .ellipsis()
                            .color(color);
                        frame.text(&label, x + row_height + 8.0, row_y, options);
                        if region.clicked {
                            choose = Some(ChooserTarget::Entry(index));
                        }
                        let (new_quantity, removed) = frame.quantity_stepper(
                            &key,
                            x + width,
                            row_y + 4.0,
                            *quantity,
                            Some("Remove Entry"),
                            false,
                        );
                        *quantity = new_quantity;
                        if removed {
                            remove = Some(index);
                        }
                        row_y += row_height;
                    }
                    if let Some(index) = remove {
                        entries.remove(index);
                    }
                    let add_options = ButtonOptions::variant(ButtonVariant::Text).icon("add").id("ctoken:add");
                    if frame.button("Add Entry", x - 8.0, row_y, None, Some(40.0), &add_options) {
                        choose = Some(ChooserTarget::NewEntry);
                    }
                    row_y += 40.0;
                    row_y - y
                });
                if let Some(target) = choose {
                    self.inner_dialog =
                        Some(TableDialog::TokenChooser { chooser: TokenChooser::for_game(&self.game), target });
                }
                match response {
                    DialogResponse::Action(1) => {
                        let bag_remaining = flatten_bag_contents(&entries);
                        self.perform(TableAction::SetBagRemaining { instance_id: widget_id, bag_remaining });
                        self.inner_dialog = None;
                        None
                    }
                    DialogResponse::Open | DialogResponse::Enter => {
                        let mut entries = entries;
                        self.show_token_chooser(frame, &mut entries);
                        Some(TableDialog::ChangeTokens { widget_id, entries, names })
                    }
                    _ => {
                        self.inner_dialog = None;
                        None
                    }
                }
            }
            TableDialog::TokenChooser { .. } => None,
            TableDialog::LoadDeck { zone_id, mut dialog } => {
                let was_own = self.deck_store.is_some();
                let mut store = self.take_deck_store(frame);
                let response = dialog.show(frame, store.as_mut());
                self.return_deck_store(store, was_own);
                match response {
                    LoadDeckResponse::Open => Some(TableDialog::LoadDeck { zone_id, dialog }),
                    LoadDeckResponse::Chosen(deck) => {
                        self.perform(TableAction::LoadDeckIntoZone { zone_id, deck });
                        None
                    }
                    LoadDeckResponse::Closed => None,
                }
            }
            TableDialog::SaveDeck(mut dialog) => {
                let was_own = self.deck_store.is_some();
                let mut store = self.take_deck_store(frame);
                let game = self.game.clone();
                let response = dialog.show(frame, &game, store.as_mut());
                self.return_deck_store(store, was_own);
                (response == SaveDeckResponse::Open).then_some(TableDialog::SaveDeck(dialog))
            }
            TableDialog::LoadScenario => {
                let actions = [DialogAction::text("Cancel")];
                let spec = DialogSpec {
                    title: Some("Load Scenario"),
                    width: 420.0,
                    actions: &actions,
                    ..DialogSpec::default()
                };
                let scenarios: Vec<(String, String)> =
                    self.game.scenarios.iter().map(|scenario| (scenario.id.clone(), scenario.name.clone())).collect();
                let loaded = self.state.loaded_scenario_ids.clone();
                let mut chosen = None;
                let row_height = 44.0;
                let response = frame.dialog("loadscenario:dialog", &spec, |frame, x, y, width| {
                    let mut row_y = y;
                    for (id, name) in &scenarios {
                        let is_loaded = loaded.contains(id);
                        let label = if is_loaded { format!("{name} (loaded)") } else { name.clone() };
                        let button_id = format!("scenario:{id}");
                        let options =
                            ButtonOptions::variant(ButtonVariant::Outlined).disabled(is_loaded).id(&button_id);
                        if frame.button(&label, x, row_y, Some(width), Some(40.0), &options) {
                            chosen = Some(id.clone());
                        }
                        row_y += row_height;
                    }
                    row_y - y - (row_height - 40.0)
                });
                if let Some(scenario_id) = chosen {
                    self.perform(TableAction::LoadScenario { scenario_id });
                    return None;
                }
                (response == DialogResponse::Open).then_some(TableDialog::LoadScenario)
            }
            TableDialog::Settings(mut dialog) => dialog.show(frame).then_some(TableDialog::Settings(dialog)),
            TableDialog::Controls => show_controls_dialog(frame).then_some(TableDialog::Controls),
            TableDialog::ConfirmGive { instance_ids, new_owner_id, title, message, confirm } => {
                match frame.confirm_dialog("table:confirmgive", &title, &message, &confirm, false) {
                    ConfirmResponse::Open => {
                        Some(TableDialog::ConfirmGive { instance_ids, new_owner_id, title, message, confirm })
                    }
                    ConfirmResponse::Confirmed => {
                        for instance_id in instance_ids {
                            self.perform(TableAction::GiveCard { instance_id, new_owner_id: new_owner_id.clone() });
                        }
                        None
                    }
                    ConfirmResponse::Cancelled => None,
                }
            }
            TableDialog::ConfirmLeave => {
                let label = self.leave_button_label.clone();
                let message = self.leave_confirmation_message.clone();
                match frame.confirm_dialog("table:confirmleave", &label, &message, &label, false) {
                    ConfirmResponse::Open => Some(TableDialog::ConfirmLeave),
                    ConfirmResponse::Confirmed => {
                        self.events.push(TableEvent::LeaveGame);
                        None
                    }
                    ConfirmResponse::Cancelled => None,
                }
            }
            TableDialog::ConfirmKick { player_id, name } => {
                let message =
                    format!("Remove {name} from the game? They can rejoin later if you don't end the session.");
                match frame.confirm_dialog("table:confirmkick", "Kick Player", &message, "Kick", false) {
                    ConfirmResponse::Open => Some(TableDialog::ConfirmKick { player_id, name }),
                    ConfirmResponse::Confirmed => {
                        self.events.push(TableEvent::KickPlayer(player_id));
                        None
                    }
                    ConfirmResponse::Cancelled => None,
                }
            }
        }
    }

    /// The token chooser opened over Change Tokens.
    fn show_token_chooser(&mut self, frame: &mut Frame, entries: &mut Vec<(Option<String>, i64)>) {
        let Some(TableDialog::TokenChooser { mut chooser, target }) = self.inner_dialog.take() else { return };
        match chooser.show(frame) {
            ChooserResponse::Open => self.inner_dialog = Some(TableDialog::TokenChooser { chooser, target }),
            ChooserResponse::Cancelled => {}
            ChooserResponse::Picked(file) => match target {
                ChooserTarget::Entry(index) => {
                    if let Some(entry) = entries.get_mut(index) {
                        entry.0 = Some(file);
                    }
                }
                ChooserTarget::NewEntry => entries.push((Some(file), 1)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bag_contents_round_trip() {
        let remaining: Vec<String> = ["a.png", "b.png", "a.png"].map(String::from).to_vec();
        let grouped = group_bag_remaining(&remaining);
        assert_eq!(grouped, vec![(Some("a.png".to_string()), 2), (Some("b.png".to_string()), 1)]);
        assert_eq!(flatten_bag_contents(&grouped), ["a.png", "a.png", "b.png"]);
        assert_eq!(flatten_bag_contents(&[(None, 3)]), Vec::<String>::new());
    }
}
