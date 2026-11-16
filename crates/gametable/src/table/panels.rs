//! The in-game panels and per-frame drawing of the table. Ports of `drawGameMenu`, `drawLog`,
//! `drawChat`, `drawSearch`, `drawLibrary`, `drawTokenPicker`, `drawPreview`, `drawDragFeedback`
//! and `T:draw` from `src/ui/table/table_screen.lua`.

use std::f32::consts::PI;

use gametable_core::deck_widget_zones::parse_deck_widget_zone_id;
use gametable_core::models::{
    BoardWidgetInstance, BoardWidgetKind, CardDefinition, CardInstance, CardOrientation, CardZone, LogEntry,
    PlayerRole, SearchTargetType, WidgetSizeOverride, format_elapsed_time,
};
use gametable_core::stack_utils::stack_of;
use gametable_core::util::{RandomIndexSource, SystemRandom, trim_lua_whitespace};

use crate::app::Frame;
use crate::data::save_files::local_now;
use crate::graphics::draw_list::Color;
use crate::table::card_pool_grid::{TILE_CARD_HEIGHT, TILE_CARD_WIDTH, TILE_SCALE};
use crate::table::card_render::{CARD_HEIGHT, CARD_WIDTH, CardDraw, PreviewFace, orientation_turns, shown_orientation};
use crate::table::input::{DragSource, DragSourceKind};
use crate::table::menus::TableDialog;
use crate::table::screen::{ChatLine, PickupItem, TableEvent, TableScreen};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::dropdown::DropdownOptions;
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::{self, Weight, fixed};
use crate::ui::widgets::{ButtonOptions, ButtonVariant, Choice, IconButtonOptions, ScrollbarStyle};

const LOG_WIDTH: f32 = 340.0;
const CHAT_WIDTH: f32 = 340.0;
const TOKEN_TILE: f32 = 96.0;
const GRID_NAME_LINES: usize = 3;

/// Lua's `tostring` of a number (e.g. `20`, `12.5`), as counters show their value.
pub fn lua_number_text(number: f64) -> String {
    gametable_core::json::format_like_c_percent_g(number, 14)
}

/// Practice games run /flip and /roll dX locally; anything else is posted as typed.
fn local_chat_reply(text: &str, random: &mut dyn RandomIndexSource) -> Option<String> {
    let trimmed_end = text.trim_end();
    if trimmed_end == "/flip" {
        return Some(format!("flipped: {}", if random.random_from_one_to(2) == 1 { "Heads" } else { "Tails" }));
    }
    let rest = text.strip_prefix("/roll")?;
    let after_space = rest.trim_start();
    if after_space.len() == rest.len() {
        return None;
    }
    let digits = after_space.strip_prefix(['d', 'D'])?.trim_end();
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let sides: usize = digits.parse().ok()?;
    if sides < 1 {
        return None;
    }
    Some(format!("rolled d{sides}: {}", random.random_from_one_to(sides)))
}

const WHITE: Color = fixed::WHITE;

impl TableScreen {
    /// `T:drawWidgetFace`: a widget drawn under the pointer while dragged.
    fn draw_widget_face(
        &self,
        frame: &mut Frame,
        widget: &BoardWidgetInstance,
        center_x: f32,
        center_y: f32,
        scale: f32,
    ) {
        let (width, height) = self.widget_instance_size(frame, widget);
        match widget.kind {
            BoardWidgetKind::SimpleCounter => self.counter_face(
                frame,
                widget,
                center_x - width * scale / 2.0,
                center_y - height * scale / 2.0,
                width * scale,
                1.0,
            ),
            BoardWidgetKind::Token | BoardWidgetKind::ImageToken => {
                self.token_face(frame, widget, center_x, center_y, width * scale, 1.0)
            }
            BoardWidgetKind::CardToken => {
                self.card_token_face(frame, widget, center_x, center_y, width * scale, height * scale, 1.0)
            }
            BoardWidgetKind::PackGenerator => self.pack_face(
                frame,
                widget,
                center_x - width * scale / 2.0,
                center_y - height * scale / 2.0,
                width * scale,
                1.0,
            ),
            BoardWidgetKind::DeckBuilder => frame.fill_rect(
                center_x - width * scale / 2.0,
                center_y - height * scale / 2.0,
                width * scale,
                height * scale,
                8.0 * scale,
                Color::rgba(0.0, 0.0, 0.0, 0.54),
            ),
            _ => {}
        }
    }

    /// A mixed group (cards and card tokens), each offset from the drop point by its distance
    /// from the dragged primary's position.
    #[allow(clippy::too_many_arguments)]
    fn draw_mixed_group(
        &self,
        frame: &mut Frame,
        group: &[PickupItem],
        primary: (f32, f32),
        at: (f32, f32),
        scale: f32,
    ) {
        for item in group {
            match item {
                PickupItem::CardToken(widget) => {
                    let (x, y) = self.to_screen(widget.x, widget.y);
                    self.draw_widget_face(frame, widget, at.0 + x - primary.0, at.1 + y - primary.1, scale);
                }
                PickupItem::Card(card) => {
                    let (x, y) = self.to_screen(card.x, card.y);
                    let angle =
                        self.card_turns(card, self.card_faces_away_from_me(card.owner_id.as_deref())) * 2.0 * PI;
                    let draw =
                        self.card_draw(card, at.0 + x - primary.0, at.1 + y - primary.1).scale(scale).angle(angle);
                    frame.draw_card(&draw);
                }
            }
        }
    }

    /// Widgets attached to any of the anchors riding along with them.
    fn draw_riders(
        &self,
        frame: &mut Frame,
        is_anchor: impl Fn(&str) -> bool,
        primary: (f32, f32),
        at: (f32, f32),
        scale: f32,
    ) {
        for widget in self.state.widgets() {
            if widget.attached_card_id.as_deref().is_some_and(&is_anchor) {
                let (x, y) = self.to_screen(widget.x, widget.y);
                self.draw_widget_face(frame, widget, at.0 + x - primary.0, at.1 + y - primary.1, scale);
            }
        }
    }

    /// Whatever is being dragged, under the pointer.
    fn draw_drag_feedback(&mut self, frame: &mut Frame) {
        if !self.drag.as_ref().is_some_and(|drag| drag.started) {
            return;
        }
        self.update_drag_scale(frame);
        let Some(drag) = self.drag.clone() else { return };
        let (mouse_x, mouse_y) = frame.ui.mouse();
        let at = (mouse_x + drag.grab.0, mouse_y + drag.grab.1);
        let scale = drag.scale;
        match &drag.source.kind {
            DragSourceKind::Card(source) | DragSourceKind::Pile(source) => {
                if drag.group.len() > 1 {
                    let primary = drag
                        .group
                        .iter()
                        .find(|item| Some(item.instance_id()) == drag.primary_id.as_deref())
                        .unwrap_or(&drag.group[0]);
                    let (x, y) = primary.position();
                    let primary_screen = self.to_screen(x, y);
                    self.draw_mixed_group(frame, &drag.group, primary_screen, at, scale);
                    self.draw_riders(frame, |id| drag.group_ids.contains(id), primary_screen, at, scale);
                } else if let Some(PickupItem::Card(card)) = drag.group.first() {
                    let draw =
                        CardDraw { face_up: card.face_up || source.force_face_up, ..self.card_draw(card, at.0, at.1) }
                            .scale(scale)
                            .angle(drag.angle)
                            .border(source.border);
                    frame.draw_card(&draw);
                }
            }
            DragSourceKind::CardToken(widget) => {
                let anchor = self.to_screen(widget.x, widget.y);
                if drag.group.len() > 1 {
                    self.draw_mixed_group(frame, &drag.group, anchor, at, scale);
                    self.draw_riders(frame, |id| drag.group_ids.contains(id), anchor, at, scale);
                } else {
                    self.draw_widget_face(frame, widget, at.0, at.1, scale);
                    let anchor_id = widget.instance_id.clone();
                    self.draw_riders(frame, |id| id == anchor_id, anchor, at, scale);
                }
            }
            DragSourceKind::Selection { .. } => {
                // Everything keeps its offset from the anchor, at table size.
                let Some((anchor_x, anchor_y)) = self.selection_anchor_world(&drag) else { return };
                let anchor = self.world_to_screen(anchor_x, anchor_y);
                let tint = self.selection_tint(frame);
                for card in &drag.draw_order {
                    let (x, y) = self.to_screen(card.x, card.y);
                    let angle =
                        self.card_turns(card, self.card_faces_away_from_me(card.owner_id.as_deref())) * 2.0 * PI;
                    let draw = self
                        .card_draw(card, at.0 + x - anchor.0, at.1 + y - anchor.1)
                        .scale(self.zoom)
                        .angle(angle)
                        .tint(Some(tint));
                    frame.draw_card(&draw);
                }
                for widget in self.state.widgets() {
                    let rides = widget.attached_card_id.as_ref().is_some_and(|anchor| drag.group_ids.contains(anchor));
                    if drag.token_ids.contains(&widget.instance_id) || rides {
                        let (x, y) = self.to_screen(widget.x, widget.y);
                        self.draw_widget_face(frame, widget, at.0 + x - anchor.0, at.1 + y - anchor.1, self.zoom);
                    }
                }
            }
            DragSourceKind::Widget(widget) | DragSourceKind::Token(widget) => {
                self.draw_widget_face(frame, widget, at.0, at.1, scale)
            }
            DragSourceKind::NewCardToken => self.draw_widget_face(
                frame,
                &TableScreen::default_widget(BoardWidgetKind::CardToken),
                at.0,
                at.1,
                scale,
            ),
            DragSourceKind::Library { definition_id } => {
                let definition = self.definition(definition_id);
                let draw = CardDraw::new(
                    definition,
                    true,
                    at.0,
                    at.1,
                    &self.game.card_backs,
                    self.game.scenario_card_back_id.as_deref(),
                );
                frame.draw_card(&draw);
            }
            DragSourceKind::ImageToken { image_file } => {
                let widget = BoardWidgetInstance {
                    image_path: Some(image_file.clone()),
                    ..TableScreen::default_widget(BoardWidgetKind::ImageToken)
                };
                self.draw_widget_face(frame, &widget, at.0, at.1, scale);
            }
            DragSourceKind::NewToken => {
                self.draw_widget_face(frame, &TableScreen::default_widget(BoardWidgetKind::Token), at.0, at.1, scale)
            }
            DragSourceKind::RevealedBagToken { image_path, .. } => {
                let widget = BoardWidgetInstance {
                    image_path: Some(image_path.clone()),
                    size_override: Some(WidgetSizeOverride::NativeImageSize),
                    ..TableScreen::default_widget(BoardWidgetKind::ImageToken)
                };
                self.draw_widget_face(frame, &widget, at.0, at.1, scale);
            }
        }
    }

    /// Opens the panel chrome; returns the content rect and whether close was asked for.
    fn table_panel(
        &self,
        frame: &mut Frame,
        id: &str,
        width: f32,
        height: f32,
        title: &str,
        trailing: Option<&str>,
    ) -> (crate::ui::context::Rect, bool) {
        let response = frame.panel(id, width, height, title, trailing, None);
        (response.content, response.close_requested)
    }

    fn draw_game_menu(&mut self, frame: &mut Frame) {
        let kickable: Vec<(String, String)> = if self.can_kick_players {
            self.state
                .players
                .iter()
                .filter(|player| player.role != PlayerRole::Host && player.connected)
                .map(|player| (player.id.clone(), player.name.clone()))
                .collect()
        } else {
            Vec::new()
        };
        let show_load_scenario = self.can_load_scenario && self.game.has_scenarios && !self.game.scenarios.is_empty();
        let mut height = 64.0
            + 16.0
            + 40.0
            + if self.can_save_game { 52.0 } else { 0.0 }
            + 52.0
            + 52.0
            + if show_load_scenario { 52.0 } else { 0.0 }
            + 16.0;
        if self.can_kick_players {
            height += 20.0 + 24.0 + kickable.len().max(1) as f32 * 48.0;
        }
        let (content, close) = self.table_panel(frame, "gamemenu", 360.0, height, "Game Menu", None);
        if close {
            self.toggle_game_menu();
        }
        let (x, width) = (content.x, content.width);
        let mut row_y = content.y + 16.0;
        let button = |id: &'static str, icon: &'static str| ButtonOptions {
            variant: ButtonVariant::Outlined,
            icon: Some(icon),
            color: Some(WHITE),
            id: Some(id),
            ..ButtonOptions::default()
        };
        let leave_label = self.leave_button_label.clone();
        if frame.button(&leave_label, x + 16.0, row_y, Some(width - 32.0), Some(40.0), &button("gm:leave", "logout")) {
            self.open_dialog(TableDialog::ConfirmLeave);
        }
        row_y += 40.0;
        if self.can_save_game {
            row_y += 12.0;
            if frame.button("Save Game...", x + 16.0, row_y, Some(width - 32.0), Some(40.0), &button("gm:save", "save"))
            {
                self.events.push(TableEvent::SaveGame);
            }
            row_y += 40.0;
        }
        row_y += 12.0;
        if frame.button("Settings", x + 16.0, row_y, Some(width - 32.0), Some(40.0), &button("gm:settings", "settings"))
        {
            let dialog = self.settings_dialog();
            self.open_dialog(dialog);
        }
        row_y += 40.0 + 12.0;
        if frame.button("Controls", x + 16.0, row_y, Some(width - 32.0), Some(40.0), &button("gm:controls", "keyboard"))
        {
            self.open_dialog(TableDialog::Controls);
        }
        row_y += 40.0;
        if show_load_scenario {
            row_y += 12.0;
            if frame.button(
                "Load Scenario...",
                x + 16.0,
                row_y,
                Some(width - 32.0),
                Some(40.0),
                &button("gm:scenario", "layers"),
            ) {
                self.open_dialog(TableDialog::LoadScenario);
            }
            row_y += 40.0;
        }
        if self.can_kick_players {
            row_y += 20.0;
            frame.text(
                "Kick a Player",
                x + 16.0,
                row_y,
                TextOptions::new().bold().color(Color::rgba(1.0, 1.0, 1.0, 0.7)),
            );
            row_y += 24.0;
            if kickable.is_empty() {
                let options = TextOptions::new().color(Color::rgba(1.0, 1.0, 1.0, 0.54));
                frame.text("No other players connected right now.", x + 16.0, row_y + 8.0, options);
            }
            for (player_id, name) in kickable {
                frame.text(&name, x + 16.0, row_y, TextOptions::new().middle_of(48.0).color(WHITE));
                let id = format!("gm:kick:{player_id}");
                let tooltip = format!("Kick {name}");
                let options = IconButtonOptions::tooltip(&tooltip).id(&id).color(fixed::RED_ACCENT);
                if frame.icon_button("person_remove", x + width - 56.0, row_y + 4.0, &options) {
                    self.open_dialog(TableDialog::ConfirmKick { player_id, name });
                }
                row_y += 48.0;
            }
        }
    }

    /// The game log, docked to the left edge and sliding in and out.
    fn draw_log(&mut self, frame: &mut Frame) {
        let entries: Vec<LogEntry> = self.state.log.clone();
        let has_save = self.game.folder_path.is_some();
        let (_, screen_height) = frame.ui.screen_size();
        let top = frame.ui.content_top;
        let (x, y, width, height) = (LOG_WIDTH * (self.log_animation - 1.0), top, LOG_WIDTH, screen_height - top);
        frame.ui.cover(x, y, width, height);
        frame.fill_rect(x, y, width, height, 0.0, fixed::OVERLAY_PANEL);
        frame.text("Game Log", x + 16.0, y + 16.0, TextOptions::new().bold().color(WHITE));
        let list_top = y + 48.0;
        let list_height = height - 48.0 - if has_save { 73.0 } else { 0.0 };
        if entries.is_empty() {
            let options = TextOptions::new()
                .width(width)
                .middle_of(list_height)
                .centered()
                .color(Color::rgba(1.0, 1.0, 1.0, 0.54));
            frame.text("No actions yet.", x, list_top, options);
        } else {
            let font = theme::font(14.0, Weight::Regular);
            let text_width = width - 32.0;
            let prefix_width = frame.fonts.width(font, "[00:00:00] ");
            let line_height = frame.fonts.height(font);
            let mut heights = Vec::with_capacity(entries.len());
            let mut total = 16.0;
            for entry in &entries {
                let (_, lines) = frame.fonts.wrap(font, &entry.message, text_width - prefix_width);
                let entry_height = lines.len().max(1) as f32 * line_height + 4.0;
                heights.push(entry_height);
                total += entry_height;
            }
            if self.log_scroll_to_end {
                frame.ui.scroll_to("log:list", (total - list_height).max(0.0));
                self.log_scroll_to_end = false;
            }
            let offset = frame.begin_scroll("log:list", x, list_top, width, list_height);
            let mut entry_y = list_top + 8.0 - offset;
            for (entry, entry_height) in entries.iter().zip(heights) {
                if entry_y + entry_height > list_top && entry_y < list_top + list_height {
                    let time = format!("[{}] ", format_elapsed_time(entry.elapsed_milliseconds));
                    frame.text(
                        &time,
                        x + 16.0,
                        entry_y + 2.0,
                        TextOptions::new().font(font).color(Color::rgba(1.0, 1.0, 1.0, 0.38)),
                    );
                    let options = TextOptions::new().font(font).width(text_width - prefix_width).color(WHITE);
                    frame.text(&entry.message, x + 16.0 + prefix_width, entry_y + 2.0, options);
                }
                entry_y += entry_height;
            }
            frame.end_scroll("log:list", total, &ScrollbarStyle::default());
        }
        if has_save {
            frame.divider(x, list_top + list_height, width, Some(Color::rgba(1.0, 1.0, 1.0, 0.24)));
            let options = ButtonOptions {
                variant: ButtonVariant::Outlined,
                icon: Some("save_alt"),
                color: Some(WHITE),
                id: Some("log:save"),
                ..ButtonOptions::default()
            };
            let button_width = frame.button_width("Save Log", &options);
            if frame.button(
                "Save Log",
                x + 16.0,
                list_top + list_height + 16.0,
                Some(button_width),
                Some(40.0),
                &options,
            ) {
                self.save_log(frame);
            }
        }
    }

    /// Practice chat: /flip and /roll run locally; anything else stays as a note.
    fn send_local_chat(&mut self, text: &str) {
        let name =
            self.player_by_id(&self.acting_player_id).map_or_else(|| "You".to_string(), |player| player.name.clone());
        let now = local_now();
        let time_label = format!("{:02}:{:02}", now.hour, now.minute);
        let text = local_chat_reply(text, &mut SystemRandom).unwrap_or_else(|| text.to_string());
        self.local_chat.push(ChatLine { time_label, name, text });
    }

    /// The chat panel docked to the right edge.
    fn draw_chat(&mut self, frame: &mut Frame) {
        let lines = self.local_chat.clone();
        let (screen_width, screen_height) = frame.ui.screen_size();
        let top = frame.ui.content_top;
        let (x, y, width, height) =
            (screen_width - CHAT_WIDTH * self.chat_animation, top, CHAT_WIDTH, screen_height - top);
        frame.ui.cover(x, y, width, height);
        frame.fill_rect(x, y, width, height, 0.0, fixed::OVERLAY_PANEL);
        frame.text("Chat", x + 16.0, y + 16.0, TextOptions::new().bold().color(WHITE));
        let input_height = 48.0;
        let (list_y, list_height) = (y + 48.0, height - 48.0 - input_height - 24.0);
        let offset = frame.begin_scroll("table:chat", x + 8.0, list_y, width - 16.0, list_height);
        let mut line_y = list_y - offset;
        let text_width = width - 32.0;
        for line in &lines {
            let head = format!("{}  {}", line.time_label, line.name);
            let head_options = TextOptions::new()
                .size(12.0)
                .bold()
                .color(Color::rgba(1.0, 1.0, 1.0, 0.54))
                .width(text_width)
                .ellipsis();
            frame.text(&head, x + 16.0, line_y, head_options);
            line_y += 18.0;
            frame.text(&line.text, x + 16.0, line_y, TextOptions::new().width(text_width).color(WHITE));
            line_y += frame.text_height(&line.text, text_width, 14.0, Weight::Regular) + 10.0;
        }
        let content_height = line_y + offset - list_y;
        frame.end_scroll("table:chat", content_height, &ScrollbarStyle::default());
        if lines.len() != self.seen_chat {
            self.seen_chat = lines.len();
            frame.ui.scroll_to_end("table:chat");
        }
        if lines.is_empty() {
            let options = TextOptions::new()
                .width(width)
                .middle_of(list_height)
                .centered()
                .color(Color::rgba(1.0, 1.0, 1.0, 0.54));
            frame.text("Try /flip or /roll d6", x, list_y, options);
        }
        // The field draws with the dark palette so typed text reads on this dark panel.
        frame.theme.force_dark = true;
        let options = TextFieldOptions {
            label_background: Some(fixed::OVERLAY_PANEL),
            max_length: Some(500),
            ..TextFieldOptions::label("Message")
        };
        let mut text = std::mem::take(&mut self.chat_text);
        let field = frame.text_field(
            "table:chatinput",
            x + 12.0,
            y + height - input_height - 12.0,
            width - 24.0 - 44.0,
            &mut text,
            &options,
        );
        frame.theme.force_dark = false;
        let blank = text.trim().is_empty();
        let send_options = IconButtonOptions::tooltip("Send").id("table:chatsend").color(WHITE).disabled(blank);
        let send =
            frame.icon_button("send", x + width - 12.0 - 40.0, y + height - input_height - 12.0 + 4.0, &send_options);
        if (field.submitted || send) && !blank {
            let trimmed = trim_lua_whitespace(&text).to_string();
            self.send_local_chat(&trimmed);
            text.clear();
            frame.ui.focus_field("table:chatinput", false);
        }
        self.chat_text = text;
    }

    /// A grid of card tiles in a panel, laid out like the deck editor's card pool.
    fn draw_card_grid(
        &mut self,
        frame: &mut Frame,
        id: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        count: usize,
        mut draw_tile: impl FnMut(&mut TableScreen, &mut Frame, usize, f32, f32, f32),
    ) {
        let name_height = GRID_NAME_LINES as f32 * frame.fonts.height(theme::font(11.0, Weight::Regular));
        let (max_extent, tile_height, gap) = (TILE_CARD_WIDTH + 24.0, TILE_CARD_HEIGHT + 6.0 + name_height + 4.0, 12.0);
        let inner = width - 24.0;
        let columns = (((inner + gap) / (max_extent + gap)).ceil() as usize).max(1);
        let tile_width = (inner - (columns as f32 - 1.0) * gap) / columns as f32;
        let offset = frame.begin_scroll(id, x, y, width, height);
        for index in 0..count {
            let (column, row) = (index % columns, index / columns);
            let tile_x = x + 12.0 + column as f32 * (tile_width + gap);
            let tile_y = y + 12.0 + row as f32 * (tile_height + gap) - offset;
            if tile_y + tile_height > y && tile_y < y + height {
                draw_tile(self, frame, index, tile_x, tile_y, tile_width);
            }
        }
        let rows = count.div_ceil(columns) as f32;
        frame.end_scroll(id, 24.0 + rows * (tile_height + gap) - gap, &ScrollbarStyle::default());
    }

    fn grid_name(frame: &mut Frame, title: &str, x: f32, y: f32, width: f32) {
        let options = TextOptions::new().size(11.0).width(width).centered().max_lines(GRID_NAME_LINES).color(WHITE);
        frame.text(title, x, y + TILE_CARD_HEIGHT + 6.0, options);
    }

    /// The acting player's open search (a zone or a pile) as a card grid.
    fn draw_search(&mut self, frame: &mut Frame) {
        let Some(search) =
            self.state.searches.iter().find(|search| search.searcher_id == self.acting_player_id).cloned()
        else {
            return;
        };
        let mut cards: Vec<CardInstance>;
        let title;
        if search.target_type == SearchTargetType::Zone {
            cards = self
                .state
                .cards()
                .iter()
                .filter(|card| {
                    card.zone == CardZone::Zone
                        && card.zone_id.as_deref() == Some(search.target_id.as_str())
                        && card.owner_id == search.target_owner_id
                })
                .cloned()
                .collect();
            let real_zone_id = parse_deck_widget_zone_id(&search.target_id)
                .map_or(search.target_id.clone(), |parsed| parsed.real_zone_id.to_string());
            title = self.zone_by_id(&real_zone_id).map_or_else(|| "Zone".to_string(), |zone| zone.name);
        } else {
            cards = stack_of(self.state.cards(), &search.target_id).into_iter().cloned().collect();
            title = "Pile".to_string();
        }
        cards.sort_by(|first, second| second.z_index.cmp(&first.z_index));
        let (screen_width, screen_height) = frame.ui.screen_size();
        let panel_width = 1280f32.min(screen_width - 40.0);
        let panel_height = 720f32.min(screen_height - frame.ui.content_top - 40.0);
        let trailing = format!("{} card{}", cards.len(), if cards.len() == 1 { "" } else { "s" });
        let (content, close) = self.table_panel(frame, "search", panel_width, panel_height, &title, Some(&trailing));
        if close {
            self.perform(crate::table::actions::TableAction::StopSearch);
        }
        if cards.is_empty() {
            let options = TextOptions::new()
                .width(content.width)
                .middle_of(content.height)
                .centered()
                .color(Color::rgba(1.0, 1.0, 1.0, 0.54));
            frame.text("Nothing here to search.", content.x, content.y, options);
            return;
        }
        self.draw_card_grid(
            frame,
            "search:grid",
            content.x,
            content.y,
            content.width,
            content.height,
            cards.len(),
            |table, frame, index, x, y, width| {
                let card = &cards[index];
                let definition = table.definition(&card.definition_id);
                let (center_x, center_y) = (x + width / 2.0, y + TILE_CARD_HEIGHT / 2.0);
                let region = frame.ui.region(
                    &format!("search:{}", card.instance_id),
                    center_x - TILE_CARD_WIDTH / 2.0,
                    y,
                    TILE_CARD_WIDTH,
                    TILE_CARD_HEIGHT,
                    RegionOptions::hand(),
                );
                let angle = definition.map_or(0.0, |definition| orientation_turns(definition.orientation) * 2.0 * PI);
                let alpha = if table.is_dragging(&card.instance_id) { 0.3 } else { 1.0 };
                let draw = CardDraw::new(
                    definition,
                    definition.is_some(),
                    center_x,
                    center_y,
                    &table.game.card_backs,
                    table.game.scenario_card_back_id.as_deref(),
                )
                .scale(TILE_SCALE)
                .angle(angle)
                .alpha(alpha);
                frame.draw_card(&draw);
                let name = definition.map_or(String::new(), |definition| definition.card_title.clone());
                TableScreen::grid_name(frame, &name, x, y, width);
                if region.hovered {
                    table.overlay_hover_id = Some(card.instance_id.clone());
                }
                if region.pressed {
                    let (mouse_x, mouse_y) = frame.ui.mouse();
                    let source = crate::table::input::CardSource {
                        force_face_up: true,
                        ..crate::table::input::CardSource::plain(card.clone())
                    };
                    table.press_source(
                        DragSource::new(DragSourceKind::Card(source), (center_x, center_y), TILE_SCALE),
                        mouse_x,
                        mouse_y,
                    );
                }
            },
        );
    }

    /// The Card Library: filter bar, search and a grid to drag cards onto the table from.
    fn draw_library(&mut self, frame: &mut Frame) {
        let game = self.game.clone();
        let mut visible: Vec<&CardDefinition> = self.library.visible(&game);
        if game.has_scenarios {
            visible.retain(|definition| definition.is_scenario_card == self.library_scenario);
        }
        let visible_ids: Vec<String> = visible.iter().map(|definition| definition.id.clone()).collect();
        let (screen_width, screen_height) = frame.ui.screen_size();
        let panel_width = 1280f32.min(screen_width - 40.0);
        let panel_height = 720f32.min(screen_height - frame.ui.content_top - 40.0);
        frame.theme.force_dark = true;
        let card_count = game.cards().len();
        let trailing =
            format!("{} of {} card{}", visible_ids.len(), card_count, if card_count == 1 { "" } else { "s" });
        let (content, close) =
            self.table_panel(frame, "library", panel_width, panel_height, "Card Library", Some(&trailing));
        if close {
            self.library_open = false;
        }
        let (x, y, width, height) = (content.x, content.y, content.width, content.height);
        let mut row_y = y;
        if game.has_scenarios {
            let choices = [Choice::new(false, "Player Cards"), Choice::new(true, "Scenario Cards")];
            self.library_scenario = frame.segmented(
                "lib:mode",
                x + 16.0,
                row_y,
                width - 32.0,
                &choices,
                self.library_scenario,
                Some(40.0),
                false,
            );
            row_y += 52.0;
        }
        let (filters_bottom, _) =
            frame.card_filters("lib", &mut self.library, &game, x + 16.0, row_y, width - 32.0, false, &[]);
        row_y = filters_bottom + 12.0;
        row_y = frame.card_search(
            "lib",
            &mut self.library,
            &game,
            x + 16.0,
            row_y,
            width - 32.0,
            Some(fixed::OVERLAY_PANEL),
            Some(WHITE),
        ) + 4.0;
        let grid_height = y + height - row_y;
        if visible_ids.is_empty() {
            let options = TextOptions::new()
                .width(width)
                .middle_of(grid_height)
                .centered()
                .color(Color::rgba(1.0, 1.0, 1.0, 0.54));
            frame.text("No cards match the current filters.", x, row_y, options);
        } else {
            self.draw_card_grid(
                frame,
                "library:grid",
                x,
                row_y,
                width,
                grid_height,
                visible_ids.len(),
                |table, frame, index, tile_x, tile_y, tile_width| {
                    let Some(definition) = table.game.card_by_id(&visible_ids[index]).cloned() else { return };
                    let (center_x, center_y) = (tile_x + tile_width / 2.0, tile_y + TILE_CARD_HEIGHT / 2.0);
                    let region = frame.ui.region(
                        &format!("lib:{}", definition.id),
                        center_x - TILE_CARD_WIDTH / 2.0,
                        tile_y,
                        TILE_CARD_WIDTH,
                        TILE_CARD_HEIGHT,
                        RegionOptions::hand(),
                    );
                    let draw = CardDraw::new(
                        Some(&definition),
                        true,
                        center_x,
                        center_y,
                        &table.game.card_backs,
                        table.game.scenario_card_back_id.as_deref(),
                    )
                    .scale(TILE_SCALE)
                    .angle(orientation_turns(definition.orientation) * 2.0 * PI);
                    frame.draw_card(&draw);
                    TableScreen::grid_name(frame, &definition.card_title, tile_x, tile_y, tile_width);
                    if region.hovered {
                        table.library_hover_definition = Some(definition.id.clone());
                    }
                    if region.pressed {
                        let (mouse_x, mouse_y) = frame.ui.mouse();
                        let source = DragSource::new(
                            DragSourceKind::Library { definition_id: definition.id.clone() },
                            (center_x, center_y),
                            1.0,
                        );
                        table.press_source(source, mouse_x, mouse_y);
                    }
                },
            );
        }
        frame.theme.force_dark = false;
    }

    /// The token picker: the colour token, the card token and the game's tokens, by type.
    fn draw_token_picker(&mut self, frame: &mut Frame) {
        let (screen_width, screen_height) = frame.ui.screen_size();
        let panel_width = 900f32.min(screen_width - 40.0);
        let panel_height = 600f32.min(screen_height - frame.ui.content_top - 40.0);
        frame.theme.force_dark = true;
        let types = self.game.token_types();
        if self.token_picker_type != "all" && !types.contains(&self.token_picker_type) {
            self.token_picker_type = "all".to_string();
        }
        enum Entry {
            Color,
            Card,
            Image { name: String, file: String, path: Option<String> },
        }
        let mut entries: Vec<Entry> = Vec::new();
        // The plain colour and card tokens only show under "All".
        if self.token_picker_type == "all" {
            entries.push(Entry::Color);
            entries.push(Entry::Card);
        }
        for (name, file, path, token_type) in self.defined_token_entries() {
            if self.token_picker_type == "all" || token_type.as_deref() == Some(self.token_picker_type.as_str()) {
                entries.push(Entry::Image { name, file: file.unwrap_or_default(), path });
            }
        }
        let trailing = format!("{} token{}", entries.len(), if entries.len() == 1 { "" } else { "s" });
        let (content, close) =
            self.table_panel(frame, "tokenPicker", panel_width, panel_height, "Tokens", Some(&trailing));
        if close {
            self.token_picker_open = false;
        }
        let (x, mut y, width, mut height) = (content.x, content.y, content.width, content.height);
        if !types.is_empty() {
            let mut choices = vec![Choice::new("all".to_string(), "All")];
            choices.extend(types.iter().map(|token_type| Choice::new(token_type.clone(), token_type.clone())));
            let options = DropdownOptions { height: Some(40.0), ..DropdownOptions::default() };
            self.token_picker_type = frame.dropdown(
                "tokenPicker:type",
                x + 16.0,
                y,
                220f32.min(width - 32.0),
                &choices,
                self.token_picker_type.clone(),
                &options,
            );
            y += 40.0 + 12.0;
            height -= 40.0 + 12.0;
        }
        let gap = 12.0;
        let columns = (((width - 24.0 + gap) / (TOKEN_TILE + gap)).floor() as usize).max(1);
        let tile_width = (width - 24.0 - (columns as f32 - 1.0) * gap) / columns as f32;
        let name_height = frame.fonts.height(theme::font(11.0, Weight::Regular)) + 4.0;
        let tile_height = TOKEN_TILE + name_height;
        let offset = frame.begin_scroll("tokenPicker:grid", x, y, width, height);
        for (index, entry) in entries.iter().enumerate() {
            let (column, row) = (index % columns, index / columns);
            let tile_x = x + 12.0 + column as f32 * (tile_width + gap);
            let tile_y = y + 12.0 + row as f32 * (tile_height + gap) - offset;
            if tile_y + tile_height <= y || tile_y >= y + height {
                continue;
            }
            let (center_x, center_y) = (tile_x + tile_width / 2.0, tile_y + TOKEN_TILE / 2.0);
            let (id, name) = match entry {
                Entry::Color => ("tokenPicker:color".to_string(), "Color Token".to_string()),
                Entry::Card => ("tokenPicker:cardToken".to_string(), "Card Token".to_string()),
                Entry::Image { name, file, .. } => (format!("tokenPicker:{file}"), name.clone()),
            };
            let region = frame.ui.region(&id, tile_x, tile_y, tile_width, TOKEN_TILE, RegionOptions::hand());
            match entry {
                Entry::Color => {
                    let widget = TableScreen::default_widget(BoardWidgetKind::Token);
                    self.token_face(frame, &widget, center_x, center_y, TOKEN_TILE, 1.0);
                }
                Entry::Card => {
                    let scale = ((tile_width - 8.0) / CARD_WIDTH).min((TOKEN_TILE - 8.0) / CARD_HEIGHT);
                    let widget = TableScreen::default_widget(BoardWidgetKind::CardToken);
                    self.card_token_face(
                        frame,
                        &widget,
                        center_x,
                        center_y,
                        CARD_WIDTH * scale,
                        CARD_HEIGHT * scale,
                        1.0,
                    );
                }
                Entry::Image { path, .. } => {
                    if let Some(texture) = path.as_deref().and_then(|path| frame.images.get(path)) {
                        let scale = (tile_width / texture.width).min(TOKEN_TILE / texture.height);
                        frame.draw_image(
                            texture,
                            center_x - texture.width * scale / 2.0,
                            center_y - texture.height * scale / 2.0,
                            scale,
                            1.0,
                        );
                    }
                }
            }
            let options = TextOptions::new().width(tile_width).centered().size(11.0).max_lines(1).color(WHITE);
            frame.text(&name, tile_x, tile_y + TOKEN_TILE + 2.0, options);
            if region.pressed {
                let (mouse_x, mouse_y) = frame.ui.mouse();
                let kind = match entry {
                    Entry::Color => DragSourceKind::NewToken,
                    Entry::Card => DragSourceKind::NewCardToken,
                    Entry::Image { file, .. } => DragSourceKind::ImageToken { image_file: file.clone() },
                };
                self.press_source(DragSource::new(kind, (center_x, center_y), 1.0), mouse_x, mouse_y);
            }
        }
        let rows = entries.len().div_ceil(columns) as f32;
        frame.end_scroll("tokenPicker:grid", 24.0 + rows * (tile_height + gap) - gap, &ScrollbarStyle::default());
        frame.theme.force_dark = false;
    }

    /// Space: a big copy of exactly what's visible, in the half of the screen away from the mouse.
    fn draw_preview(&self, frame: &mut Frame) {
        if !self.space_down {
            return;
        }
        let (definition, face_up, orientation) = if let Some(card) = self.hovered_card() {
            let definition = self.definition(&card.definition_id);
            let face_up = (card.face_up || self.hovered_force_face_up) && definition.is_some();
            let orientation = match card.zone {
                CardZone::Table => shown_orientation(
                    definition,
                    !face_up,
                    &self.game.card_backs,
                    self.game.scenario_card_back_id.as_deref(),
                ),
                // Zones can be shown turned; the preview matches what's on screen.
                CardZone::Zone => card
                    .zone_id
                    .as_deref()
                    .and_then(|zone_id| self.zone_by_id(zone_id))
                    .map_or(CardOrientation::Portrait, |zone| zone.orientation),
                CardZone::Hand => CardOrientation::Portrait,
            };
            (definition, face_up, orientation)
        } else if let Some(definition) = self.library_hover_definition.as_deref().and_then(|id| self.definition(id)) {
            (Some(definition), true, definition.orientation)
        } else {
            return;
        };
        let (screen_width, screen_height) = frame.ui.screen_size();
        let (mouse_x, _) = frame.ui.mouse();
        let faces = [PreviewFace { definition, face_up, orientation }];
        frame.draw_card_preview(
            &faces,
            &self.game.card_backs,
            screen_width,
            screen_height,
            mouse_x,
            self.game.scenario_card_back_id.as_deref(),
        );
    }

    /// `T:draw(x, y, w, h)`: input against last frame's items, the view, then the overlays in
    /// the original's stacking order.
    pub fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        self.snapshot_session();
        self.derive();
        self.refresh_selection();
        if self.table_rect.is_some() {
            self.process_input(frame);
        }
        self.library_hover_definition = None;
        let chat_squeeze = if self.is_chat_available() { CHAT_WIDTH * self.chat_animation } else { 0.0 };
        let log_squeeze = LOG_WIDTH * self.log_animation;
        self.draw_view(frame, x + log_squeeze, y, width - log_squeeze - chat_squeeze, height);
        self.previous_items = std::mem::take(&mut self.items);
        if self.my_search_open() {
            frame.overlay(|frame| self.draw_search(frame));
        }
        if self.library_open {
            frame.overlay(|frame| self.draw_library(frame));
        }
        if self.token_picker_open {
            frame.overlay(|frame| self.draw_token_picker(frame));
        }
        if self.log_animation > 0.001 {
            frame.overlay(|frame| self.draw_log(frame));
        }
        if self.rulebook.is_open() {
            let scenario = self.active_scenario_with_guide();
            let close = frame.overlay(|frame| self.rulebook.draw(frame, scenario.as_ref()));
            if close {
                self.rulebook.toggle(frame);
            }
        }
        if self.game_menu_open {
            frame.overlay(|frame| self.draw_game_menu(frame));
        }
        if self.is_chat_available() && self.chat_animation > 0.001 {
            frame.overlay(|frame| self.draw_chat(frame));
        }
        frame.overlay(|frame| self.draw_drag_feedback(frame));
        // The card preview renders above everything, chat included.
        frame.overlay(|frame| self.draw_preview(frame));
        if std::mem::take(&mut self.development_load_deck)
            && let Some(zone) = self.game.zones.iter().find(|zone| zone.deals_built_deck && !zone.shared).cloned()
        {
            self.open_load_deck_for_development(frame, zone);
        }
        self.show_menus_and_dialogs(frame);
    }

    /// Seeds the playmat once the table is first shown.
    pub fn enter(&mut self, frame: &Frame) {
        self.seed_playmat(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gametable_core::util::ParkMillerRandom;

    #[test]
    fn practice_chat_commands() {
        let mut random = ParkMillerRandom::with_seed(1);
        assert!(local_chat_reply("/flip", &mut random).is_some_and(|reply| reply.starts_with("flipped: ")));
        assert!(local_chat_reply("/roll d6", &mut random).is_some_and(|reply| reply.starts_with("rolled d6: ")));
        assert!(local_chat_reply("/roll D20  ", &mut random).is_some_and(|reply| reply.starts_with("rolled d20: ")));
        assert_eq!(local_chat_reply("/rolld6", &mut random), None);
        assert_eq!(local_chat_reply("/roll d0", &mut random), None);
        assert_eq!(local_chat_reply("hello", &mut random), None);
        assert_eq!(lua_number_text(20.0), "20");
        assert_eq!(lua_number_text(12.5), "12.5");
    }
}
