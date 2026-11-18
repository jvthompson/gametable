//! Table layout and drawing. Port of `src/ui/table/table_view.lua` (the build parts of
//! `lib/ui/table_screen.dart`): the felt, playmats, piles, widgets, arrows, the player rows (hands,
//! zones, avatars) and the shared-zone sidebars. Everything drawn records a hit item and drop
//! targets for the next frame's input (see `input.rs`).

use std::f32::consts::PI;

use gametable_core::color::{hsl_to_rgb, rgb_to_hsl, to_rgba_channels};
use gametable_core::deck_widget_zones::build_deck_widget_zone_id;
use gametable_core::geometry::{
    TABLE_PAN_MARGIN_FRACTION, WORLD_HEIGHT, WORLD_WIDTH, canonical_to_local_pixel, clamp_card_center_y,
    local_pixel_to_canonical,
};
use gametable_core::models::{
    BoardWidgetInstance, BoardWidgetKind, CardInstance, PlayerInfo, PlaymatType, SharedZoneSide, WidgetSizeOverride,
    ZoneDefinition, ZoneKind, ZoneWidgetKind,
};

use crate::app::Frame;
use crate::data::display_settings::{HandSetting, felt_texture_enabled};
use crate::data::file_system::join_path;
use crate::data::image_resolver::resolve_pack_generator_image_path;
use crate::graphics::draw_list::{Color, FragmentProgram, StencilCompare};
use crate::graphics::tessellation::DrawMode;
use crate::table::card_render::{CARD_HEIGHT, CARD_WIDTH, CardDraw, PILE_EXTRA, orientation_turns, shown_orientation};
use crate::table::screen::{SHARED_OWNER_KEY, TableScreen, top_card};
use crate::table::selection::{HitShape, clamp_world_center};
use crate::ui::components::AvatarSource;
use crate::ui::context::Rect;
use crate::ui::draw::TextOptions;
use crate::ui::theme::{self, LIGHT, Weight, fixed, with_alpha};

pub const SLOT_WIDTH: f32 = CARD_WIDTH + PILE_EXTRA + 16.0;
pub const SLOT_HEIGHT: f32 = CARD_HEIGHT + PILE_EXTRA + 16.0;
/// The hand band's height.
pub const BAND_HEIGHT: f32 = CARD_HEIGHT + PILE_EXTRA + 16.0;
pub const COUNTER_SIZE: f32 = 80.0;
pub const TOKEN_SIZE: f32 = 25.0;
pub const IMAGE_TOKEN_SIZE: f32 = 25.0;

const DECK_HEADER: f32 = 24.0;
const DECK_FOOTER: f32 = 36.0;
const DECK_SPACING: f32 = 8.0;
const DECK_PADDING: f32 = 8.0;
const ROTATION_DURATION: f64 = 0.15;
const FAN_LIFT_RATE: f32 = 18.0;
const BAG_SIZE: f32 = COUNTER_SIZE;
const BAG_GAP: f32 = 12.0;
/// A standard card is 2.5in wide, so this many world units make an inch; a playmat is 24×12in.
const PIXELS_PER_INCH: f32 = CARD_WIDTH / 2.5;
const PLAYMAT_WIDTH: f32 = 24.0 * PIXELS_PER_INCH;
const PLAYMAT_HEIGHT: f32 = 12.0 * PIXELS_PER_INCH;
/// The single centre marker's size while its image loads (or with none chosen).
const SINGLE_MARKER_WIDTH: f32 = 1280.0;
const SINGLE_MARKER_HEIGHT: f32 = 720.0;
const MAT_GAP_X: f32 = 2.0 * CARD_WIDTH;

const WORLD_W: f32 = WORLD_WIDTH as f32;
const WORLD_H: f32 = WORLD_HEIGHT as f32;

/// `V.deckWidgetSize(subZoneCount)`.
pub fn deck_widget_size(sub_zone_count: usize) -> (f32, f32) {
    let count = sub_zone_count.max(1) as f32;
    let (zone_width, zone_height) = (CARD_WIDTH + PILE_EXTRA, CARD_HEIGHT + PILE_EXTRA);
    (
        count * zone_width + (count - 1.0) * DECK_SPACING + DECK_PADDING * 2.0,
        DECK_HEADER + DECK_SPACING + zone_height + DECK_SPACING + DECK_FOOTER + DECK_PADDING * 2.0,
    )
}

/// Which side of a shuffle button acts on what.
#[derive(Debug, Clone, PartialEq)]
pub enum ShuffleTarget {
    Pile { root_id: String },
    Zone { zone_id: String },
}

/// What a hit item is (the original's `item.kind` plus its payload).
#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    Table,
    TableCard {
        card: CardInstance,
        root_id: String,
        owned_by_opponent: bool,
    },
    Pile {
        card: CardInstance,
        root_id: String,
        cards: Vec<CardInstance>,
        owned_by_opponent: bool,
    },
    Shuffle {
        key: String,
        target: ShuffleTarget,
    },
    HandCard {
        card: CardInstance,
        scale: f32,
        card_center: (f32, f32),
    },
    Zone {
        zone_id: String,
        zone: ZoneDefinition,
        top: Option<CardInstance>,
        cards: Vec<CardInstance>,
        shared: bool,
        deck_widget: bool,
    },
    Tooltip,
    Counter {
        widget: BoardWidgetInstance,
    },
    DockedCounter {
        widget: BoardWidgetInstance,
    },
    Token {
        widget: BoardWidgetInstance,
    },
    PackGenerator {
        widget: BoardWidgetInstance,
    },
    DeckBody {
        widget: BoardWidgetInstance,
    },
    DeckHeader {
        widget: BoardWidgetInstance,
    },
    DeckSave {
        widget: BoardWidgetInstance,
    },
    TokenBagIdle {
        widget: BoardWidgetInstance,
    },
    RevealedBagToken {
        widget: BoardWidgetInstance,
        slot_id: String,
        image_path: String,
    },
    Avatar {
        player_id: String,
    },
}

impl ItemKind {
    /// The original's kind name (tooltip ids and a few checks use it).
    pub fn name(&self) -> &'static str {
        match self {
            ItemKind::Table => "table",
            ItemKind::TableCard { .. } => "tableCard",
            ItemKind::Pile { .. } => "pile",
            ItemKind::Shuffle { .. } => "shuffle",
            ItemKind::HandCard { .. } => "handCard",
            ItemKind::Zone { .. } => "zone",
            ItemKind::Tooltip => "tooltip",
            ItemKind::Counter { .. } => "counter",
            ItemKind::DockedCounter { .. } => "dockedCounter",
            ItemKind::Token { .. } => "token",
            ItemKind::PackGenerator { .. } => "packGenerator",
            ItemKind::DeckBody { .. } => "deckBody",
            ItemKind::DeckHeader { .. } => "deckHeader",
            ItemKind::DeckSave { .. } => "deckSave",
            ItemKind::TokenBagIdle { .. } => "tokenBagIdle",
            ItemKind::RevealedBagToken { .. } => "revealedBagToken",
            ItemKind::Avatar { .. } => "avatar",
        }
    }
}

/// Something drawn this frame that the mouse can hit.
#[derive(Debug, Clone, PartialEq)]
pub struct HitItem {
    pub kind: ItemKind,
    pub shape: HitShape,
    /// Drawn inside a 180° rotated box with this centre: points are mirrored before testing.
    pub mirror: Option<(f32, f32)>,
    pub interactable: bool,
    pub tooltip: Option<String>,
    /// The card that hovering this item points at (Space preview, hotkeys).
    pub hover_id: Option<String>,
}

impl HitItem {
    pub fn new(kind: ItemKind, shape: HitShape) -> HitItem {
        HitItem { kind, shape, mirror: None, interactable: true, tooltip: None, hover_id: None }
    }

    pub fn interactable(self, interactable: bool) -> HitItem {
        HitItem { interactable, ..self }
    }

    pub fn tooltip(self, tooltip: impl Into<String>) -> HitItem {
        HitItem { tooltip: Some(tooltip.into()), ..self }
    }

    pub fn hover(self, hover_id: Option<String>) -> HitItem {
        HitItem { hover_id, ..self }
    }

    /// `V.hitTest(item, mx, my)`.
    pub fn hit(&self, x: f32, y: f32) -> bool {
        let (x, y) = match self.mirror {
            Some((center_x, center_y)) => (2.0 * center_x - x, 2.0 * center_y - y),
            None => (x, y),
        };
        self.shape.contains(x, y)
    }
}

/// A place cards (or, for bags, image tokens) can be dropped.
#[derive(Debug, Clone, PartialEq)]
pub struct DropZone {
    pub zone_id: String,
    pub rect: Rect,
}

/// This frame's layout (`T.layoutRects`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LayoutRects {
    pub left: Option<Rect>,
    pub right: Option<Rect>,
    pub top: Rect,
    pub bottom: Rect,
}

/// The fanned hand's shape from the hand settings (`V.handParams()`).
#[derive(Debug, Clone, Copy)]
pub struct HandParameters {
    pub scale: f32,
    pub card_width: f32,
    pub card_height: f32,
    pub step: f32,
    pub step_angle: f32,
    pub max_spread: f32,
    pub raise: f32,
    pub lift: f32,
    pub overhang: f32,
}

/// Per-card (dx, dy below the apex, angle) from the fan centre (`fanLayout`).
fn fan_layout(parameters: &HandParameters, count: usize, inner_width: f32) -> Vec<(f32, f32, f32)> {
    if count == 1 {
        return vec![(0.0, 0.0, 0.0)];
    }
    let count_f = count as f32;
    let angle_step = parameters.step_angle.min(parameters.max_spread / (count_f - 1.0));
    let middle = (count_f + 1.0) / 2.0;
    if angle_step < 1e-4 {
        // No tilt: a straight overlapping row, squeezed to fit.
        let step = parameters.step.min(((inner_width - parameters.card_width) / (count_f - 1.0)).max(0.0));
        return (1..=count).map(|index| ((index as f32 - middle) * step, 0.0, 0.0)).collect();
    }
    let half_angle = (count_f - middle) * angle_step;
    // Shrink the step until the fan fits, including tilted corners and a lifted outer card.
    let corner_width = parameters.card_width / 2.0 * half_angle.cos()
        + parameters.card_height / 2.0 * half_angle.sin()
        + parameters.lift * half_angle.sin();
    let mut radius = parameters.step / angle_step.sin();
    let half_width = radius * half_angle.sin() + corner_width;
    if half_width > inner_width / 2.0 {
        radius = ((inner_width / 2.0 - corner_width) / half_angle.sin()).max(0.0);
    }
    (1..=count)
        .map(|index| {
            let angle = (index as f32 - middle) * angle_step;
            (radius * angle.sin(), radius * (1.0 - angle.cos()), angle)
        })
        .collect()
}

/// `rowLefts(widths, gap)`: left edges of mats placed edge to edge outward from the table's
/// vertical centre line (no gap at the centre split, `gap` between other neighbours).
fn row_lefts(widths: &[f32], gap: f32) -> Vec<f32> {
    let count = widths.len();
    let mut lefts = vec![0.0; count];
    if count == 0 {
        return lefts;
    }
    if count % 2 == 1 {
        let middle = count / 2;
        lefts[middle] = WORLD_W / 2.0 - widths[middle] / 2.0;
        let mut x = lefts[middle];
        for index in (0..middle).rev() {
            x = x - gap - widths[index];
            lefts[index] = x;
        }
        x = lefts[middle] + widths[middle];
        for index in middle + 1..count {
            x += gap;
            lefts[index] = x;
            x += widths[index];
        }
    } else {
        let half = count / 2;
        let mut x = WORLD_W / 2.0;
        for index in half..count {
            lefts[index] = x;
            x += widths[index] + gap;
        }
        x = WORLD_W / 2.0;
        for index in (0..half).rev() {
            x -= widths[index];
            lefts[index] = x;
            x -= gap;
        }
    }
    lefts
}

/// Player colour with HSL lightness - 0.30 at 55% alpha (`_playerTintColor`).
fn player_tint(argb: u32) -> Color {
    let channels = to_rgba_channels(argb, 1.0);
    let (hue, saturation, lightness) = rgb_to_hsl(channels.red, channels.green, channels.blue);
    let (red, green, blue) = hsl_to_rgb(hue, saturation, (lightness - 0.30).clamp(0.0, 1.0));
    Color::rgba(red as f32, green as f32, blue as f32, 0.55)
}

/// Luminance grey of a colour, for a flipped colour token.
fn grey_color(argb: u32, alpha: f32) -> Color {
    let color = TableScreen::argb(argb, alpha);
    let grey = color.red * 0.299 + color.green * 0.587 + color.blue * 0.114;
    Color::rgba(grey, grey, grey, color.alpha)
}

/// One pile box (table piles, zones and deck-widget sub-zones): the 94×124 area at the centre.
pub(super) struct PileBox<'a> {
    pub center_x: f32,
    pub center_y: f32,
    pub scale: f32,
    pub count: usize,
    pub top: Option<&'a CardInstance>,
    pub angle: f32,
    pub border: Option<Color>,
    pub alpha: f32,
    pub tint: Option<Color>,
    pub wiggle_key: Option<String>,
    pub searched: bool,
    /// The shuffle button and what it shuffles.
    pub shuffle: Option<(String, ShuffleTarget)>,
}

impl<'a> PileBox<'a> {
    pub fn new(center_x: f32, center_y: f32, count: usize, top: Option<&'a CardInstance>) -> PileBox<'a> {
        PileBox {
            center_x,
            center_y,
            scale: 1.0,
            count,
            top,
            angle: 0.0,
            border: None,
            alpha: 1.0,
            tint: None,
            wiggle_key: None,
            searched: false,
            shuffle: None,
        }
    }
}

impl TableScreen {
    // -----------------------------------------------------------------------
    // Sizes
    // -----------------------------------------------------------------------

    /// `V.widgetSize(T, kind)`.
    pub(super) fn widget_size(&self, kind: BoardWidgetKind) -> (f32, f32) {
        match kind {
            BoardWidgetKind::SimpleCounter | BoardWidgetKind::PackGenerator | BoardWidgetKind::TokenBag => {
                (COUNTER_SIZE, COUNTER_SIZE)
            }
            BoardWidgetKind::Token => (TOKEN_SIZE, TOKEN_SIZE),
            BoardWidgetKind::ImageToken => (IMAGE_TOKEN_SIZE, IMAGE_TOKEN_SIZE),
            BoardWidgetKind::CardToken => (CARD_WIDTH, CARD_HEIGHT),
            BoardWidgetKind::DeckBuilder => deck_widget_size(self.game.deck_building_zones().count()),
            BoardWidgetKind::Arrow => (0.0, 0.0),
        }
    }

    pub(super) fn token_image_path(&self, image_path: Option<&str>) -> Option<String> {
        Some(join_path(&[self.game.folder_path.as_deref()?, "_tokens", image_path?]))
    }

    /// An image's own size as a square of its larger side (falls back to the default image token
    /// size while it can't be loaded).
    pub(super) fn native_image_size(&self, frame: &mut Frame, image_path: Option<&str>) -> (f32, f32) {
        let texture = self.token_image_path(image_path).and_then(|path| frame.images.get(&path));
        match texture {
            Some(texture) => {
                let side = texture.width.max(texture.height);
                (side, side)
            }
            None => (IMAGE_TOKEN_SIZE, IMAGE_TOKEN_SIZE),
        }
    }

    /// `V.widgetInstanceSize`: the widget's own size override first.
    pub(super) fn widget_instance_size(&self, frame: &mut Frame, widget: &BoardWidgetInstance) -> (f32, f32) {
        match widget.size_override {
            Some(WidgetSizeOverride::NativeImageSize) => self.native_image_size(frame, widget.image_path.as_deref()),
            Some(WidgetSizeOverride::Multiplier(size)) => (size as f32, size as f32),
            None => self.widget_size(widget.kind),
        }
    }

    // -----------------------------------------------------------------------
    // Colours
    // -----------------------------------------------------------------------

    pub(super) fn zone_background(&self, owner_id: Option<&str>) -> Color {
        let neutral = Color::rgba(0.0, 0.0, 0.0, 0.15);
        if !self.color_tint_enabled {
            return neutral;
        }
        owner_id.and_then(|owner| self.player_by_id(owner)).map_or(neutral, |player| player_tint(player.color))
    }

    pub(super) fn arrow_color(&self, player_id: Option<&str>) -> Color {
        player_id
            .and_then(|player| self.player_by_id(player))
            .map_or(fixed::RED_ACCENT, |player| TableScreen::argb(player.color, 1.0))
    }

    // -----------------------------------------------------------------------
    // Camera
    // -----------------------------------------------------------------------

    pub(super) fn table_rect(&self) -> Rect {
        self.table_rect.unwrap_or_default()
    }

    pub(super) fn max_pan(&self) -> (f32, f32) {
        let rect = self.table_rect();
        let margin = TABLE_PAN_MARGIN_FRACTION as f32;
        (
            ((WORLD_W * (1.0 + 2.0 * margin) - rect.width) / 2.0).max(0.0),
            ((WORLD_H * (1.0 + 2.0 * margin) - rect.height) / 2.0).max(0.0),
        )
    }

    pub(super) fn clamped_camera(&self) -> (f32, f32) {
        let (max_x, max_y) = self.max_pan();
        (self.camera_x.clamp(-max_x, max_x), self.camera_y.clamp(-max_y, max_y))
    }

    /// Zoom and world offset for the felt shader so its grain sticks to the table.
    fn felt_world_offset(&self) -> (f32, f32, f32) {
        let rect = self.table_rect();
        let (camera_x, camera_y) = self.clamped_camera();
        let (screen_center_x, screen_center_y) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let (world_center_x, world_center_y) = (WORLD_W / 2.0 - camera_x, WORLD_H / 2.0 - camera_y);
        (self.zoom, world_center_x - screen_center_x / self.zoom, world_center_y - screen_center_y / self.zoom)
    }

    /// World units (0..2560 × 0..1440) -> screen.
    pub(super) fn world_to_screen(&self, world_x: f32, world_y: f32) -> (f32, f32) {
        let rect = self.table_rect();
        let (camera_x, camera_y) = self.clamped_camera();
        (
            rect.x + rect.width / 2.0 + (world_x + camera_x - WORLD_W / 2.0) * self.zoom,
            rect.y + rect.height / 2.0 + (world_y + camera_y - WORLD_H / 2.0) * self.zoom,
        )
    }

    pub(super) fn screen_to_world(&self, screen_x: f32, screen_y: f32) -> (f32, f32) {
        let rect = self.table_rect();
        let (camera_x, camera_y) = self.clamped_camera();
        (
            WORLD_W / 2.0 - camera_x + (screen_x - rect.x - rect.width / 2.0) / self.zoom,
            WORLD_H / 2.0 - camera_y + (screen_y - rect.y - rect.height / 2.0) / self.zoom,
        )
    }

    pub(super) fn canonical_to_world(&self, fraction_x: f64, fraction_y: f64) -> (f32, f32) {
        let (x, y) = canonical_to_local_pixel(fraction_x, fraction_y, WORLD_WIDTH, WORLD_HEIGHT, self.is_mirrored);
        (x as f32, y as f32)
    }

    pub(super) fn to_screen(&self, fraction_x: f64, fraction_y: f64) -> (f32, f32) {
        let (world_x, world_y) = self.canonical_to_world(fraction_x, fraction_y);
        self.world_to_screen(world_x, world_y)
    }

    /// Screen point -> table fractions, optionally clamping Y like a dropped card.
    pub(super) fn screen_to_canonical(&self, screen_x: f32, screen_y: f32, clamp_height: Option<f32>) -> (f64, f64) {
        let (world_x, mut world_y) = self.screen_to_world(screen_x, screen_y);
        if let Some(height) = clamp_height {
            world_y = clamp_card_center_y(f64::from(world_y), WORLD_HEIGHT, f64::from(height), 10.0) as f32;
        }
        self.world_to_canonical(world_x, world_y)
    }

    pub(super) fn world_to_canonical(&self, world_x: f32, world_y: f32) -> (f64, f64) {
        local_pixel_to_canonical(f64::from(world_x), f64::from(world_y), WORLD_WIDTH, WORLD_HEIGHT, self.is_mirrored)
    }

    /// Screen point -> table fractions, clamped so an item of these half extents stays on the table.
    pub(super) fn screen_to_canonical_clamped(&self, screen_x: f32, screen_y: f32, half: (f32, f32)) -> (f64, f64) {
        let (world_x, world_y) = self.screen_to_world(screen_x, screen_y);
        let (world_x, world_y) = clamp_world_center(world_x, world_y, half.0, half.1);
        self.world_to_canonical(world_x, world_y)
    }

    // -----------------------------------------------------------------------
    // Items
    // -----------------------------------------------------------------------

    pub(super) fn add_item(&mut self, item: HitItem) {
        let mirror = self.draw_mirror.or(item.mirror);
        self.items.push(HitItem { mirror, ..item });
    }

    /// Runs `draw` with everything turned 180° inside the box; items added meanwhile are mirrored.
    fn with_rotated(
        &mut self,
        frame: &mut Frame,
        rotate: bool,
        rect: Rect,
        draw: impl FnOnce(&mut TableScreen, &mut Frame),
    ) {
        if !rotate {
            draw(self, frame);
            return;
        }
        let (center_x, center_y) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let list = frame.ui.list();
        list.push();
        list.translate(center_x, center_y);
        list.rotate(PI);
        list.translate(-center_x, -center_y);
        let previous = self.draw_mirror;
        self.draw_mirror = Some((center_x, center_y));
        draw(self, frame);
        self.draw_mirror = previous;
        frame.ui.list().pop();
    }

    // -----------------------------------------------------------------------
    // Animations
    // -----------------------------------------------------------------------

    /// `V.animatedTurns`: eases toward `target` over 0.15 s whenever it changes.
    pub(super) fn animated_turns(&mut self, frame: &Frame, key: &str, target: f32) -> f32 {
        let now = frame.time;
        let eased = |started_at: f64| {
            let t = ((now - started_at) / ROTATION_DURATION).min(1.0) as f32;
            1.0 - (1.0 - t).powi(3)
        };
        let animation = self
            .rotation_animations
            .entry(key.to_string())
            .or_insert(crate::table::screen::RotationAnimation { from: target, to: target, started_at: now });
        if animation.to != target {
            animation.from += (animation.to - animation.from) * eased(animation.started_at);
            animation.to = target;
            animation.started_at = now;
        }
        animation.from + (animation.to - animation.from) * eased(animation.started_at)
    }

    /// `V.wiggleAngle`: the shuffle wiggle, 0.4 s.
    pub(super) fn wiggle_angle(&mut self, frame: &Frame, key: &str) -> f32 {
        let Some(&started_at) = self.wiggles.get(key) else { return 0.0 };
        let t = ((frame.time - started_at) / 0.4) as f32;
        if t >= 1.0 {
            self.wiggles.remove(key);
            return 0.0;
        }
        (t * PI * 6.0).sin() * 0.15 * (1.0 - t)
    }

    // -----------------------------------------------------------------------
    // Card helpers
    // -----------------------------------------------------------------------

    /// Table card rotation in turns: facing away + orientation + quarter turns.
    pub(super) fn card_turns(&self, card: &CardInstance, face_away: bool) -> f32 {
        let definition = self.definition(&card.definition_id);
        let showing_back = !(card.face_up && definition.is_some());
        let orientation = shown_orientation(
            definition,
            showing_back,
            &self.game.card_backs,
            self.game.scenario_card_back_id.as_deref(),
        );
        (if face_away { 0.5 } else { 0.0 }) + orientation_turns(orientation) + card.rotation_turns as f32 / 4.0
    }

    pub(super) fn is_dragging(&self, instance_id: &str) -> bool {
        self.drag.as_ref().is_some_and(|drag| {
            drag.started && (drag.primary_id.as_deref() == Some(instance_id) || drag.group_ids.contains(instance_id))
        })
    }

    pub(super) fn widget_being_dragged(&self, instance_id: &str) -> bool {
        self.drag.as_ref().is_some_and(|drag| {
            drag.started
                && (drag.widget_id.as_deref() == Some(instance_id)
                    || drag.token_ids.contains(instance_id)
                    || drag.group_ids.contains(instance_id))
        })
    }

    /// 0.3 for a card being dragged here or by another player.
    pub(super) fn ghost_alpha(&self, instance_id: &str) -> f32 {
        if self.is_dragging(instance_id) {
            return 0.3;
        }
        let dragged_elsewhere = self.card_drag_previews.values().any(|preview| {
            preview.player_id != self.acting_player_id && preview.instance_ids.iter().any(|id| id == instance_id)
        });
        if dragged_elsewhere { 0.3 } else { 1.0 }
    }

    pub(super) fn card_draw<'a>(&'a self, card: &CardInstance, center_x: f32, center_y: f32) -> CardDraw<'a> {
        CardDraw::new(
            self.definition(&card.definition_id),
            card.face_up,
            center_x,
            center_y,
            &self.game.card_backs,
            self.game.scenario_card_back_id.as_deref(),
        )
    }

    // -----------------------------------------------------------------------
    // Playmats
    // -----------------------------------------------------------------------

    fn playmat_size(&self, frame: &mut Frame, path: Option<&str>, scale: f32, fallback: (f32, f32)) -> (f32, f32) {
        match path.and_then(|path| frame.images.get(path)) {
            Some(texture) => (texture.width * scale, texture.height * scale),
            None => (fallback.0 * scale, fallback.1 * scale),
        }
    }

    /// A playmat rect at a world top-left: its image cover-fit (at half opacity) inside the rounded
    /// marker, and the border. `rotated` turns just the image for a far-seat player's own mat.
    #[allow(clippy::too_many_arguments)]
    fn draw_playmat_rect(
        &self,
        frame: &mut Frame,
        left: f32,
        top: f32,
        mark_width: f32,
        mark_height: f32,
        path: Option<&str>,
        rotated: bool,
    ) {
        let (x, y) = self.world_to_screen(left, top);
        let (width, height) = (mark_width * self.zoom, mark_height * self.zoom);
        let radius = 32.0 * self.zoom;
        if let Some(texture) = path.and_then(|path| frame.images.get(path)) {
            let list = frame.ui.list();
            list.stencil(1, |list| list.rectangle(DrawMode::Fill, x, y, width, height, radius, radius));
            list.set_stencil_test(Some((StencilCompare::Greater, 0)));
            let scale = (width / texture.width).max(height / texture.height);
            list.set_color(Color::rgba(1.0, 1.0, 1.0, 0.5));
            if rotated {
                list.push();
                list.translate(x + width / 2.0, y + height / 2.0);
                list.rotate(PI);
                list.translate(-(x + width / 2.0), -(y + height / 2.0));
            }
            list.draw_texture(
                texture,
                None,
                x + (width - texture.width * scale) / 2.0,
                y + (height - texture.height * scale) / 2.0,
                0.0,
                scale,
                scale,
                0.0,
                0.0,
            );
            if rotated {
                list.pop();
            }
            list.set_stencil_test(None);
        }
        let border = Color { alpha: 1.0, ..self.playmat_border_color(frame) };
        let list = frame.ui.list();
        list.set_color(border);
        list.set_line_width(4.0 * self.zoom);
        list.rectangle(
            DrawMode::Line,
            x + 2.0 * self.zoom,
            y + 2.0 * self.zoom,
            width - 4.0 * self.zoom,
            height - 4.0 * self.zoom,
            radius,
            radius,
        );
        list.set_line_width(1.0);
    }

    /// One row's mats (`drawPlayerRowMarkers`): every player's size counts toward the layout,
    /// even when their mat is off, so toggling one doesn't move the others.
    fn draw_player_row_markers(
        &self,
        frame: &mut Frame,
        entries: &[(Option<String>, f64)],
        players: &[PlayerInfo],
        top_for_height: impl Fn(f32) -> f32,
        rotated: bool,
    ) {
        let mut widths = Vec::new();
        let mut heights = Vec::new();
        let mut paths = Vec::new();
        for player in players {
            let entry = usize::try_from(player.individual_playmat_index - 1).ok().and_then(|index| entries.get(index));
            let scale = entry.map_or(1.0, |(_, scale)| *scale as f32);
            let path = entry.and_then(|(path, _)| path.clone());
            let (width, height) = self.playmat_size(frame, path.as_deref(), scale, (PLAYMAT_WIDTH, PLAYMAT_HEIGHT));
            widths.push(width);
            heights.push(height);
            paths.push(path);
        }
        let lefts = row_lefts(&widths, MAT_GAP_X);
        for (index, player) in players.iter().enumerate() {
            if player.individual_playmat_on {
                let top = top_for_height(heights[index]);
                self.draw_playmat_rect(
                    frame,
                    lefts[index],
                    top,
                    widths[index],
                    heights[index],
                    paths[index].as_deref(),
                    rotated,
                );
            }
        }
    }

    fn draw_center_marker(&self, frame: &mut Frame) {
        let entries = self.defined_playmat_entries();
        if self.game.playmat_type != PlaymatType::Shared {
            let (top_row, bottom_row) = (&self.derived.top_row, &self.derived.bottom_row);
            if top_row.is_empty() && bottom_row.is_empty() {
                return;
            }
            if !top_row.is_empty() && !bottom_row.is_empty() {
                self.draw_player_row_markers(frame, &entries, top_row, |height| WORLD_H / 2.0 - height, true);
                self.draw_player_row_markers(frame, &entries, bottom_row, |_| WORLD_H / 2.0, false);
            } else {
                // Solo table: centre each mat vertically, unrotated.
                let row = if top_row.is_empty() { bottom_row } else { top_row };
                self.draw_player_row_markers(frame, &entries, row, |height| (WORLD_H - height) / 2.0, false);
            }
            return;
        }
        if !self.state.shared_playmat_on {
            return;
        }
        let entry = usize::try_from(self.state.shared_playmat_index - 1).ok().and_then(|index| entries.get(index));
        let scale = entry.map_or(1.0, |(_, scale)| *scale as f32);
        let path = entry.and_then(|(path, _)| path.clone());
        let (width, height) =
            self.playmat_size(frame, path.as_deref(), scale, (SINGLE_MARKER_WIDTH, SINGLE_MARKER_HEIGHT));
        self.draw_playmat_rect(
            frame,
            (WORLD_W - width) / 2.0,
            (WORLD_H - height) / 2.0,
            width,
            height,
            path.as_deref(),
            false,
        );
    }

    // -----------------------------------------------------------------------
    // Shared drawing pieces
    // -----------------------------------------------------------------------

    /// `drawArrow`: a 3-unit line with a dot at the start and a head at the end.
    pub(super) fn draw_arrow(frame: &mut Frame, from: (f32, f32), to: (f32, f32), color: Color) {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        if dx * dx + dy * dy < 4.0 {
            return;
        }
        let list = frame.ui.list();
        list.set_color(color);
        list.set_line_width(3.0);
        list.set_smooth_lines(true);
        list.line(&[from.0, from.1, to.0, to.1]);
        list.circle(DrawMode::Fill, from.0, from.1, 1.5, None);
        let angle = dy.atan2(dx);
        let (head_length, head_angle) = (14.0, PI / 7.0);
        list.polygon(
            DrawMode::Fill,
            &[
                to.0,
                to.1,
                to.0 - (angle - head_angle).cos() * head_length,
                to.1 - (angle - head_angle).sin() * head_length,
                to.0 - (angle + head_angle).cos() * head_length,
                to.1 - (angle + head_angle).sin() * head_length,
            ],
        );
        list.set_line_width(1.0);
    }

    /// `drawPileBox`: an empty outline, or the top card with a count badge and optional shuffle
    /// button and search badge. Returns the box.
    pub(super) fn draw_pile_box(&mut self, frame: &mut Frame, pile: PileBox) -> Rect {
        let scale = pile.scale;
        let (center_x, center_y) = (pile.center_x, pile.center_y);
        let (box_width, box_height) = ((CARD_WIDTH + PILE_EXTRA) * scale, (CARD_HEIGHT + PILE_EXTRA) * scale);
        let (box_x, box_y) = (center_x - box_width / 2.0, center_y - box_height / 2.0);
        if pile.count == 0 {
            frame.draw_empty_card_slot(center_x, center_y, scale);
        } else {
            let wiggle = pile.wiggle_key.as_deref().map_or(0.0, |key| self.wiggle_angle(frame, key));
            let angle = pile.angle + wiggle;
            let mut card_scale = scale;
            if angle.sin().abs() > 0.5 {
                card_scale =
                    scale * (box_width / (CARD_HEIGHT * scale)).min(box_height / (CARD_WIDTH * scale)).min(1.0);
            }
            if let Some(top) = pile.top {
                let card = self
                    .card_draw(top, center_x, center_y)
                    .scale(card_scale)
                    .angle(angle)
                    .border(pile.border)
                    .alpha(pile.alpha)
                    .tint(pile.tint);
                frame.draw_card(&card);
            }
            frame.draw_count_badge(box_x + box_width - 12.0 * scale, box_y + 12.0 * scale, pile.count, scale);
            if let Some((key, target)) = pile.shuffle {
                let (shuffle_x, shuffle_y) = (center_x, box_y + box_height + 8.0 * scale - 14.0 * scale);
                let hovered = self.hover_item.as_ref().is_some_and(
                    |item| matches!(&item.kind, ItemKind::Shuffle { key: hovered_key, .. } if *hovered_key == key),
                );
                let list = frame.ui.list();
                list.set_color(Color::rgba(0.0, 0.0, 0.0, if hovered { 0.75 } else { 0.54 }));
                list.circle(DrawMode::Fill, shuffle_x, shuffle_y, 14.0 * scale, None);
                let font = theme::icon_font(16.0);
                let glyph = theme::glyph("shuffle");
                let origin = (frame.fonts.width(font, &glyph) / 2.0, frame.fonts.height(font) / 2.0);
                frame.ui.list().set_color(Color::WHITE);
                frame.print_transformed(&glyph, font, shuffle_x, shuffle_y, 0.0, scale, scale, origin.0, origin.1);
                let item = HitItem::new(
                    ItemKind::Shuffle { key, target },
                    HitShape::circle(shuffle_x, shuffle_y, 14.0 * scale),
                )
                .tooltip("Shuffle");
                self.add_item(item);
            }
        }
        if pile.searched {
            frame.draw_search_badge(center_x, box_y + 12.0 * scale, scale);
        }
        Rect::new(box_x, box_y, box_width, box_height)
    }

    /// `counterFace`: the image (with a backing circle for the value) or a solid square.
    pub(super) fn counter_face(
        &self,
        frame: &mut Frame,
        widget: &BoardWidgetInstance,
        x: f32,
        y: f32,
        size: f32,
        alpha: f32,
    ) {
        let radius = 8.0 * size / 80.0;
        let (center_x, center_y) = (x + size / 2.0, y + size / 2.0);
        let texture = self.token_image_path(widget.image_path.as_deref()).and_then(|path| frame.images.get(&path));
        let scale = size / 80.0;
        if let Some(texture) = texture {
            let fit = (size / texture.width).min(size / texture.height);
            frame.draw_image(
                texture,
                center_x - texture.width * fit / 2.0,
                center_y - texture.height * fit / 2.0,
                fit,
                alpha,
            );
        } else {
            frame.fill_rect(x, y, size, size, radius, TableScreen::argb(widget.background_color, alpha));
        }
        frame.outline_rect(
            x + 0.5,
            y + 0.5,
            size - 1.0,
            size - 1.0,
            radius,
            Color::rgba(1.0, 1.0, 1.0, 0.24 * alpha),
            1.0,
        );
        if texture.is_some() {
            frame.circle(
                DrawMode::Fill,
                center_x,
                center_y,
                18.0 * scale,
                TableScreen::argb(widget.background_color, alpha),
            );
        }
        let font = theme::table_font(22.0, Weight::Bold);
        let text = crate::table::panels::lua_number_text(widget.value);
        frame.ui.list().set_color(TableScreen::argb(widget.text_color, alpha));
        let text_width = frame.fonts.width(font, &text);
        let fit = ((size - 8.0 * scale) / (text_width * scale)).min(1.0);
        let origin = (text_width / 2.0, frame.fonts.height(font) / 2.0);
        frame.print_transformed(&text, font, center_x, center_y, 0.0, scale * fit, scale * fit, origin.0, origin.1);
    }

    /// `packFace`: the game's `packgen.png` cover-fit, or "Pack Generator".
    pub(super) fn pack_face(
        &self,
        frame: &mut Frame,
        widget: &BoardWidgetInstance,
        x: f32,
        y: f32,
        size: f32,
        alpha: f32,
    ) {
        let radius = 8.0 * size / 80.0;
        frame.fill_rect(x, y, size, size, radius, TableScreen::argb(widget.background_color, alpha));
        let path = resolve_pack_generator_image_path(self.game.folder_path.as_deref());
        if let Some(texture) = path.as_deref().and_then(|path| frame.images.get(path)) {
            let list = frame.ui.list();
            list.stencil(1, |list| list.rectangle(DrawMode::Fill, x, y, size, size, radius, radius));
            list.set_stencil_test(Some((StencilCompare::Greater, 0)));
            let scale = (size / texture.width).max(size / texture.height);
            list.set_color(Color::rgba(1.0, 1.0, 1.0, alpha));
            list.draw_texture(
                texture,
                None,
                x + (size - texture.width * scale) / 2.0,
                y + (size - texture.height * scale) / 2.0,
                0.0,
                scale,
                scale,
                0.0,
                0.0,
            );
            list.set_stencil_test(None);
        } else {
            let font = theme::table_font(11.0, Weight::Bold);
            let scale = size / 80.0;
            let (_, lines) = frame.fonts.wrap(font, "Pack Generator", 72.0);
            let line_height = frame.fonts.height(font);
            let list = frame.ui.list();
            list.set_color(TableScreen::argb(widget.text_color, alpha));
            list.push();
            list.translate(x + 4.0 * scale, y);
            list.scale(scale, scale);
            let text_y = (80.0 - lines.len() as f32 * line_height) / 2.0;
            frame.fonts.printf(
                frame.ui.list(),
                font,
                "Pack Generator",
                0.0,
                text_y,
                72.0,
                crate::graphics::text_layout::Align::Center,
            );
            frame.ui.list().pop();
        }
        frame.outline_rect(
            x + 0.5,
            y + 0.5,
            size - 1.0,
            size - 1.0,
            radius,
            Color::rgba(1.0, 1.0, 1.0, 0.24 * alpha),
            1.0,
        );
    }

    /// `tokenFace`: an image token contain-fit (greyscale when flipped) or a colour circle, plus
    /// an optional single-line label that may overflow the token.
    pub(super) fn token_face(
        &self,
        frame: &mut Frame,
        widget: &BoardWidgetInstance,
        center_x: f32,
        center_y: f32,
        size: f32,
        alpha: f32,
    ) {
        let flipped = !widget.face_up;
        if widget.kind == BoardWidgetKind::ImageToken {
            let texture = self.token_image_path(widget.image_path.as_deref()).and_then(|path| frame.images.get(&path));
            if let Some(texture) = texture {
                let scale = (size / texture.width).min(size / texture.height);
                let angle = widget.rotation_turns as f32 * PI / 2.0;
                let list = frame.ui.list();
                list.set_color(Color::rgba(1.0, 1.0, 1.0, alpha));
                if flipped {
                    list.set_program(FragmentProgram::Greyscale);
                }
                list.draw_texture(
                    texture,
                    None,
                    center_x,
                    center_y,
                    angle,
                    scale,
                    scale,
                    texture.width / 2.0,
                    texture.height / 2.0,
                );
                if flipped {
                    list.set_program(FragmentProgram::Plain);
                }
            } else {
                let list = frame.ui.list();
                list.set_color(Color::rgba(1.0, 1.0, 1.0, 0.3 * alpha));
                list.circle(DrawMode::Line, center_x, center_y, size / 2.0, None);
            }
        } else {
            let fill = if flipped {
                grey_color(widget.background_color, alpha)
            } else {
                TableScreen::argb(widget.background_color, alpha)
            };
            frame.circle(DrawMode::Fill, center_x, center_y, size / 2.0, fill);
            frame.circle(DrawMode::Line, center_x, center_y, size / 2.0, Color::rgba(0.0, 0.0, 0.0, 0.26 * alpha));
        }
        if !widget.text.is_empty() {
            let font = theme::table_font(40.0, Weight::Bold);
            frame.ui.list().set_color(TableScreen::argb(widget.text_color, alpha));
            let line_height = frame.fonts.height(font);
            let scale = (size * 0.8) / line_height;
            let origin = (frame.fonts.width(font, &widget.text) / 2.0, line_height / 2.0);
            frame.print_transformed(&widget.text, font, center_x, center_y, 0.0, scale, scale, origin.0, origin.1);
        }
    }

    /// `cardTokenFace`: a card-shaped blank with up to three lines of wrapped text (more
    /// overflow) and its owner's border.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn card_token_face(
        &self,
        frame: &mut Frame,
        widget: &BoardWidgetInstance,
        center_x: f32,
        center_y: f32,
        width: f32,
        height: f32,
        alpha: f32,
    ) {
        let flipped = !widget.face_up;
        let angle = widget.rotation_turns as f32 * PI / 2.0;
        let radius = width.min(height) * 0.05;
        let list = frame.ui.list();
        list.push();
        list.translate(center_x, center_y);
        list.rotate(angle);
        list.set_color(if flipped {
            grey_color(widget.background_color, alpha)
        } else {
            TableScreen::argb(widget.background_color, alpha)
        });
        list.rectangle(DrawMode::Fill, -width / 2.0, -height / 2.0, width, height, radius, radius);
        list.set_color(Color::rgba(0.0, 0.0, 0.0, 0.26 * alpha));
        list.rectangle(
            DrawMode::Line,
            -width / 2.0 + 0.5,
            -height / 2.0 + 0.5,
            width - 1.0,
            height - 1.0,
            radius,
            radius,
        );
        if !widget.text.is_empty() {
            let padding = 6.0;
            let line_height = height * 0.8 / 3.0;
            let font = theme::table_font(((line_height + 0.5).floor()).max(6.0), Weight::Bold);
            let scale = line_height / frame.fonts.height(font);
            let wrap_width = ((width - padding * 2.0) / scale).max(1.0);
            let (_, lines) = frame.fonts.wrap(font, &widget.text, wrap_width);
            let lines: Vec<String> =
                if lines.is_empty() { vec![String::new()] } else { lines.into_iter().map(|line| line.text).collect() };
            let block_height = lines.len() as f32 * line_height;
            frame.ui.list().set_color(TableScreen::argb(widget.text_color, alpha));
            for (index, line) in lines.iter().enumerate() {
                let line_y = -block_height / 2.0 + index as f32 * line_height + line_height / 2.0;
                let origin = (frame.fonts.width(font, line) / 2.0, frame.fonts.height(font) / 2.0);
                frame.print_transformed(line, font, 0.0, line_y, 0.0, scale, scale, origin.0, origin.1);
            }
        }
        if let Some(border) = self.owner_border_color(widget.owner_id.as_deref()) {
            let list = frame.ui.list();
            list.set_color(Color { alpha: border.alpha * alpha, ..border });
            list.set_line_width(2.0);
            list.rectangle(
                DrawMode::Line,
                -width / 2.0 + 1.0,
                -height / 2.0 + 1.0,
                width - 2.0,
                height - 2.0,
                radius,
                radius,
            );
            list.set_line_width(1.0);
        }
        frame.ui.list().pop();
    }

    /// A token bag image contain-fit into the bag square; false when it isn't loaded.
    fn draw_bag_token_image(&self, frame: &mut Frame, image_path: Option<&str>, center_x: f32, center_y: f32) -> bool {
        let Some(texture) = self.token_image_path(image_path).and_then(|path| frame.images.get(&path)) else {
            return false;
        };
        let scale = (BAG_SIZE / texture.width).min(BAG_SIZE / texture.height);
        frame.draw_image(
            texture,
            center_x - texture.width * scale / 2.0,
            center_y - texture.height * scale / 2.0,
            scale,
            1.0,
        );
        true
    }

    /// `drawTokenBag`: the idle bag square and its revealed tokens sliding toward the table.
    pub(super) fn draw_token_bag(
        &mut self,
        frame: &mut Frame,
        widget: &BoardWidgetInstance,
        zone: &ZoneDefinition,
        center_x: f32,
        center_y: f32,
        interactable: bool,
    ) {
        let (left, top) = (center_x - BAG_SIZE / 2.0, center_y - BAG_SIZE / 2.0);
        frame.fill_rect(left, top, BAG_SIZE, BAG_SIZE, 8.0, Color::rgba(0.0, 0.0, 0.0, 0.38));
        if !self.draw_bag_token_image(frame, widget.image_path.as_deref(), center_x, center_y) {
            frame.outline_rect(
                left + 4.0,
                top + 4.0,
                BAG_SIZE - 8.0,
                BAG_SIZE - 8.0,
                6.0,
                Color::rgba(1.0, 1.0, 1.0, 0.3),
                1.0,
            );
        }
        if !widget.bag_remaining.is_empty() {
            frame.draw_count_badge(center_x + BAG_SIZE / 2.0 - 12.0, top + 12.0, widget.bag_remaining.len(), 1.0);
        }
        if interactable {
            let item = HitItem::new(
                ItemKind::TokenBagIdle { widget: widget.clone() },
                HitShape::rect(left, top, BAG_SIZE, BAG_SIZE),
            )
            .tooltip(zone.name.clone());
            self.add_item(item);
        }
        let direction = if zone.side == SharedZoneSide::Right { -1.0 } else { 1.0 };
        for (index, entry) in widget.bag_revealed.iter().enumerate() {
            let target = direction * (index as f32 + 1.0) * (BAG_SIZE + BAG_GAP);
            let offset = self.animated_turns(frame, &format!("bagslot:{}", entry.slot_id), target);
            let (revealed_x, revealed_y) = (center_x + offset, center_y);
            if !self.draw_bag_token_image(frame, Some(&entry.image_path), revealed_x, revealed_y) {
                frame.circle(DrawMode::Line, revealed_x, revealed_y, BAG_SIZE / 2.0, Color::rgba(1.0, 1.0, 1.0, 0.3));
            }
            if interactable {
                let kind = ItemKind::RevealedBagToken {
                    widget: widget.clone(),
                    slot_id: entry.slot_id.clone(),
                    image_path: entry.image_path.clone(),
                };
                let item = HitItem::new(kind, HitShape::circle(revealed_x, revealed_y, BAG_SIZE / 2.0))
                    .tooltip("Click to return, drag to place");
                self.add_item(item);
            }
        }
    }

    /// The deck widget: header with owner label, one sub-zone per deck-building zone, and Save Deck.
    #[allow(clippy::too_many_arguments)]
    fn deck_widget_face(
        &mut self,
        frame: &mut Frame,
        widget: &BoardWidgetInstance,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        scale: f32,
        alpha: f32,
    ) {
        let is_owner = widget.owner_id.as_deref() == Some(self.acting_player_id.as_str());
        let interactable = !self.tab_down;
        frame.fill_rect(x, y, width, height, 8.0 * scale, Color::rgba(0.0, 0.0, 0.0, 0.54 * alpha));
        frame.outline_rect(
            x + 0.5,
            y + 0.5,
            width - 1.0,
            height - 1.0,
            8.0 * scale,
            Color::rgba(1.0, 1.0, 1.0, 0.24 * alpha),
            1.0,
        );
        let label = if is_owner {
            "My Deck".to_string()
        } else {
            widget
                .owner_id
                .as_deref()
                .and_then(|owner| self.player_by_id(owner))
                .map_or_else(|| "Deck".to_string(), |player| player.name.clone())
        };
        let (header_x, header_y) = (x + DECK_PADDING * scale, y + DECK_PADDING * scale);
        frame.ui.list().push();
        frame.ui.list().translate(header_x, header_y);
        frame.ui.list().scale(scale, scale);
        let icon_font = theme::icon_font(14.0);
        frame.ui.list().set_color(Color::rgba(1.0, 1.0, 1.0, 0.7 * alpha));
        let icon_y = (DECK_HEADER - frame.fonts.height(icon_font)) / 2.0;
        frame.fonts.print(frame.ui.list(), icon_font, &theme::glyph("drag_indicator"), 0.0, icon_y);
        let font = theme::table_font(11.0, Weight::Bold);
        let shown = frame.ellipsize(&label, font, width / scale - DECK_PADDING * 2.0 - 18.0);
        frame.ui.list().set_color(Color::rgba(1.0, 1.0, 1.0, alpha));
        let label_y = (DECK_HEADER - frame.fonts.height(font)) / 2.0;
        frame.fonts.print(frame.ui.list(), font, &shown, 18.0, label_y);
        frame.ui.list().pop();
        if is_owner && interactable {
            let shape = HitShape::rect(header_x, header_y, width - 2.0 * DECK_PADDING * scale, DECK_HEADER * scale);
            self.add_item(HitItem::new(ItemKind::DeckHeader { widget: widget.clone() }, shape));
        }
        let mut zone_x = header_x;
        let zone_y = header_y + (DECK_HEADER + DECK_SPACING) * scale;
        let zones: Vec<ZoneDefinition> = self.game.deck_building_zones().cloned().collect();
        for zone in zones {
            let synthetic_id = build_deck_widget_zone_id(&widget.instance_id, &zone.id);
            let cards = self.derived.cards_by_zone_id.get(&synthetic_id).cloned().unwrap_or_default();
            let top = top_card(&cards).cloned();
            let (zone_width, zone_height) = ((CARD_WIDTH + PILE_EXTRA) * scale, (CARD_HEIGHT + PILE_EXTRA) * scale);
            if is_owner {
                frame.fill_rect(zone_x, zone_y, zone_width, zone_height, 0.0, Color::rgba(0.0, 0.0, 0.0, 0.38 * alpha));
            }
            let mut pile =
                PileBox::new(zone_x + zone_width / 2.0, zone_y + zone_height / 2.0, cards.len(), top.as_ref());
            pile.scale = scale;
            pile.alpha = top.as_ref().map_or(1.0, |top| self.ghost_alpha(&top.instance_id)) * alpha;
            pile.searched = self.is_zone_searched(&synthetic_id, widget.owner_id.as_deref());
            self.draw_pile_box(frame, pile);
            let shape = HitShape::rect(zone_x, zone_y, zone_width, zone_height);
            if is_owner {
                self.drop_zones.push(DropZone {
                    zone_id: synthetic_id.clone(),
                    rect: Rect::new(zone_x, zone_y, zone_width, zone_height),
                });
                let hover_id = top.as_ref().map(|top| top.instance_id.clone());
                let kind = ItemKind::Zone {
                    zone_id: synthetic_id,
                    zone: zone.clone(),
                    top,
                    cards,
                    shared: false,
                    deck_widget: true,
                };
                let item =
                    HitItem::new(kind, shape).tooltip(zone.name.clone()).interactable(interactable).hover(hover_id);
                self.add_item(item);
            } else {
                self.add_item(HitItem::new(ItemKind::Tooltip, shape).tooltip(zone.name.clone()));
            }
            zone_x += zone_width + DECK_SPACING * scale;
        }
        if is_owner {
            let button_y = zone_y + (CARD_HEIGHT + PILE_EXTRA + DECK_SPACING) * scale;
            let (button_width, button_height) = (width - 2.0 * DECK_PADDING * scale, DECK_FOOTER * scale);
            let hovered = self.hover_item.as_ref().is_some_and(|item| {
                matches!(&item.kind, ItemKind::DeckSave { widget: hovered } if hovered.instance_id == widget.instance_id)
            });
            frame.fill_rect(
                header_x,
                button_y,
                button_width,
                button_height,
                button_height / 2.0,
                with_alpha(LIGHT.surface_container, alpha),
            );
            if hovered {
                frame.fill_rect(
                    header_x,
                    button_y,
                    button_width,
                    button_height,
                    button_height / 2.0,
                    Color::rgba(0.0, 0.0, 0.0, 0.06),
                );
            }
            let font = theme::table_font(14.0, Weight::Medium);
            let color = if interactable { LIGHT.primary } else { LIGHT.disabled };
            frame.ui.list().set_color(with_alpha(color, alpha));
            let origin = (frame.fonts.width(font, "Save Deck") / 2.0, frame.fonts.height(font) / 2.0);
            frame.print_transformed(
                "Save Deck",
                font,
                header_x + button_width / 2.0,
                button_y + button_height / 2.0,
                0.0,
                scale,
                scale,
                origin.0,
                origin.1,
            );
            if interactable {
                let shape = HitShape::rect(header_x, button_y, button_width, button_height);
                self.add_item(HitItem::new(ItemKind::DeckSave { widget: widget.clone() }, shape));
            }
        }
    }

    /// Opacity of a widget riding on a dragged card, or itself being moved.
    pub(super) fn widget_drag_alpha(&self, widget: &BoardWidgetInstance) -> f32 {
        if let Some(anchor) = &widget.attached_card_id
            && (self.widget_being_dragged(anchor) || self.ghost_alpha(anchor) < 1.0)
        {
            return 0.3;
        }
        if self.widget_being_dragged(&widget.instance_id) { 0.3 } else { 1.0 }
    }

    // -----------------------------------------------------------------------
    // The table area
    // -----------------------------------------------------------------------

    fn draw_pile(&mut self, frame: &mut Frame, root_id: &str) {
        let cards = self.derived.pile_groups[root_id].clone();
        let Some(top) = top_card(&cards).cloned() else { return };
        let (pile_x, pile_y) = self.to_screen(top.x, top.y);
        let owned_by_opponent = top.owner_id.as_deref().is_some_and(|owner| owner != self.acting_player_id);
        let face_away = self.card_faces_away_from_me(top.owner_id.as_deref());
        let target_turns = self.card_turns(&top, face_away);
        let turns = self.animated_turns(frame, &top.instance_id, target_turns);
        let angle = turns * 2.0 * PI;
        let alpha = self.ghost_alpha(&top.instance_id);
        let border = self.owner_border_color(top.owner_id.as_deref());
        let interactable = !owned_by_opponent && !self.tab_down;
        let tint = self.is_root_selected(root_id).then(|| self.selection_tint(frame));
        let zoom = self.zoom;
        if cards.len() > 1 {
            let items_before = self.items.len();
            let mut pile = PileBox::new(pile_x, pile_y, cards.len(), Some(&top));
            pile.scale = zoom;
            pile.angle = angle;
            pile.border = border;
            pile.alpha = alpha;
            pile.tint = tint;
            pile.wiggle_key = Some(format!("pile:{root_id}"));
            pile.searched = self.derived.pile_searched.contains(root_id);
            if interactable {
                pile.shuffle = Some((format!("pile:{root_id}"), ShuffleTarget::Pile { root_id: root_id.to_string() }));
            }
            self.draw_pile_box(frame, pile);
            // The pile item covers the unrotated 94×124 box and goes before the shuffle button.
            let (box_width, box_height) = ((CARD_WIDTH + PILE_EXTRA) * zoom, (CARD_HEIGHT + PILE_EXTRA) * zoom);
            let kind = ItemKind::Pile { card: top.clone(), root_id: root_id.to_string(), cards, owned_by_opponent };
            let item = HitItem::new(
                kind,
                HitShape::rect(pile_x - box_width / 2.0, pile_y - box_height / 2.0, box_width, box_height),
            )
            .interactable(interactable)
            .hover(Some(top.instance_id.clone()));
            let item = HitItem { mirror: self.draw_mirror, ..item };
            self.items.insert(items_before.min(self.items.len()), item);
        } else if self.flight.as_ref().is_none_or(|flight| flight.instance_id != top.instance_id) {
            let card =
                self.card_draw(&top, pile_x, pile_y).scale(zoom).angle(angle).border(border).alpha(alpha).tint(tint);
            frame.draw_card(&card);
            let kind = ItemKind::TableCard { card: top.clone(), root_id: root_id.to_string(), owned_by_opponent };
            let shape = HitShape::rotated(pile_x, pile_y, CARD_WIDTH * zoom / 2.0, CARD_HEIGHT * zoom / 2.0, angle);
            let item = HitItem::new(kind, shape).interactable(interactable).hover(Some(top.instance_id.clone()));
            self.add_item(item);
        }
    }

    fn draw_loose_card_token(&mut self, frame: &mut Frame, widget: &BoardWidgetInstance) {
        let (width, height) = self.widget_instance_size(frame, widget);
        let (center_x, center_y) = self.to_screen(widget.x, widget.y);
        let scale = self.zoom;
        let angle = widget.rotation_turns as f32 * PI / 2.0;
        let alpha = self.widget_drag_alpha(widget);
        self.card_token_face(frame, widget, center_x, center_y, width * scale, height * scale, alpha);
        if self.is_token_selected(&widget.instance_id) {
            let color = self.selection_color(frame);
            let list = frame.ui.list();
            list.push();
            list.translate(center_x, center_y);
            list.rotate(angle);
            list.set_color(Color { alpha, ..color });
            list.set_line_width(2.0);
            list.rectangle(
                DrawMode::Line,
                -width * scale / 2.0 - 2.0,
                -height * scale / 2.0 - 2.0,
                width * scale + 4.0,
                height * scale + 4.0,
                4.0,
                4.0,
            );
            list.set_line_width(1.0);
            list.pop();
        }
        // Only whoever spawned it can interact with it.
        let interactable = !self.tab_down && widget.owner_id.as_deref() == Some(self.acting_player_id.as_str());
        let shape = HitShape::rotated(center_x, center_y, width * scale / 2.0, height * scale / 2.0, angle);
        self.add_item(HitItem::new(ItemKind::Token { widget: widget.clone() }, shape).interactable(interactable));
    }

    /// Cards and loose card tokens share one z-order, so they're drawn in one pass.
    fn draw_table_piles(&mut self, frame: &mut Frame) {
        enum LayerEntry {
            Pile(String),
            CardToken(BoardWidgetInstance),
        }
        let mut layer: Vec<(i64, LayerEntry)> = self
            .derived
            .sorted_roots
            .iter()
            .map(|root| {
                let z = top_card(&self.derived.pile_groups[root]).map_or(0, |card| card.z_index);
                (z, LayerEntry::Pile(root.clone()))
            })
            .collect();
        for widget in self.state.widgets() {
            if widget.kind == BoardWidgetKind::CardToken {
                layer.push((widget.z_index, LayerEntry::CardToken(widget.clone())));
            }
        }
        layer.sort_by_key(|(z, _)| *z);
        for (_, entry) in layer {
            match entry {
                LayerEntry::Pile(root_id) => self.draw_pile(frame, &root_id),
                LayerEntry::CardToken(widget) => self.draw_loose_card_token(frame, &widget),
            }
        }
    }

    fn draw_table_widgets(&mut self, frame: &mut Frame) {
        let widgets: Vec<BoardWidgetInstance> = self.state.widgets().to_vec();
        for widget in widgets {
            if matches!(widget.kind, BoardWidgetKind::Arrow | BoardWidgetKind::CardToken) || widget.zone_id.is_some() {
                continue;
            }
            let (width, height) = self.widget_instance_size(frame, &widget);
            let (center_x, center_y) = self.to_screen(widget.x, widget.y);
            let scale = self.zoom;
            let (x, y) = (center_x - width * scale / 2.0, center_y - height * scale / 2.0);
            let alpha = self.widget_drag_alpha(&widget);
            let interactable = !self.tab_down;
            let rect = HitShape::rect(x, y, width * scale, height * scale);
            match widget.kind {
                BoardWidgetKind::SimpleCounter => {
                    self.counter_face(frame, &widget, x, y, width * scale, alpha);
                    self.add_item(
                        HitItem::new(ItemKind::Counter { widget: widget.clone() }, rect).interactable(interactable),
                    );
                }
                BoardWidgetKind::Token | BoardWidgetKind::ImageToken => {
                    self.token_face(frame, &widget, center_x, center_y, width * scale, alpha);
                    if self.is_token_selected(&widget.instance_id) {
                        let color = self.selection_color(frame);
                        let list = frame.ui.list();
                        list.set_color(Color { alpha, ..color });
                        list.set_line_width(2.0);
                        list.circle(DrawMode::Line, center_x, center_y, width * scale / 2.0 + 2.0, None);
                        list.set_line_width(1.0);
                    }
                    let shape = HitShape::circle(center_x, center_y, (width * scale / 2.0).max(6.0));
                    self.add_item(
                        HitItem::new(ItemKind::Token { widget: widget.clone() }, shape).interactable(interactable),
                    );
                }
                BoardWidgetKind::PackGenerator => {
                    self.pack_face(frame, &widget, x, y, width * scale, alpha);
                    self.add_item(
                        HitItem::new(ItemKind::PackGenerator { widget: widget.clone() }, rect)
                            .interactable(interactable),
                    );
                }
                BoardWidgetKind::DeckBuilder => {
                    // The body goes first so the header, zones and Save Deck drawn inside win.
                    let owner_interactable =
                        interactable && widget.owner_id.as_deref() == Some(self.acting_player_id.as_str());
                    self.add_item(
                        HitItem::new(ItemKind::DeckBody { widget: widget.clone() }, rect)
                            .interactable(owner_interactable),
                    );
                    self.deck_widget_face(frame, &widget, x, y, width * scale, height * scale, scale, alpha);
                }
                _ => {}
            }
        }
    }

    fn draw_arrows(&self, frame: &mut Frame) {
        for widget in self.state.widgets() {
            if widget.kind == BoardWidgetKind::Arrow
                && let (Some(x2), Some(y2)) = (widget.x2, widget.y2)
            {
                let from = self.to_screen(widget.x, widget.y);
                let to = self.to_screen(x2, y2);
                TableScreen::draw_arrow(frame, from, to, self.arrow_color(widget.creator_id.as_deref()));
            }
        }
        if let Some(arrow) = &self.arrow_drag {
            let from = self.world_to_screen(arrow.start.0, arrow.start.1);
            let to = self.world_to_screen(arrow.current.0, arrow.current.1);
            TableScreen::draw_arrow(frame, from, to, self.arrow_color(Some(&self.acting_player_id)));
        }
        for (player_id, preview) in &self.arrow_drag_previews {
            if *player_id != self.acting_player_id {
                let from = self.to_screen(preview.fraction_x, preview.fraction_y);
                let to = self.to_screen(preview.fraction_x2, preview.fraction_y2);
                TableScreen::draw_arrow(frame, from, to, self.arrow_color(Some(player_id)));
            }
        }
    }

    /// Other players' in-flight card drags: ghost faces at the preview target.
    fn draw_remote_ghosts(&self, frame: &mut Frame) {
        for (player_id, preview) in &self.card_drag_previews {
            if *player_id == self.acting_player_id {
                continue;
            }
            let Some(primary) = preview.instance_ids.first().and_then(|id| self.derived.cards_by_id.get(id)) else {
                continue;
            };
            let origin = self.to_screen(primary.x, primary.y);
            let target = self.to_screen(preview.fraction_x, preview.fraction_y);
            for id in &preview.instance_ids {
                if let Some(card) = self.derived.cards_by_id.get(id) {
                    let (card_x, card_y) = self.to_screen(card.x, card.y);
                    let angle = if self.card_faces_away_from_me(card.owner_id.as_deref()) { PI } else { 0.0 };
                    let draw = self
                        .card_draw(card, target.0 + (card_x - origin.0), target.1 + (card_y - origin.1))
                        .angle(angle);
                    frame.draw_card(&draw);
                }
            }
        }
    }

    /// The box being dragged out for a selection.
    fn draw_selection_box(&self, frame: &mut Frame) {
        let Some(selection) = &self.box_select else { return };
        let Some(current) = selection.current else { return };
        let (x1, y1) = self.world_to_screen(selection.start.0, selection.start.1);
        let (x2, y2) = self.world_to_screen(current.0, current.1);
        let color = self.selection_color(frame);
        let (x, y, width, height) = (x1.min(x2), y1.min(y2), (x2 - x1).abs(), (y2 - y1).abs());
        frame.fill_rect(x, y, width, height, 0.0, Color { alpha: 0.15, ..color });
        frame.outline_rect(x, y, width, height, 0.0, Color { alpha: 0.9, ..color }, 1.5);
    }

    fn draw_flight(&mut self, frame: &mut Frame) {
        let Some(flight) = self.flight.clone() else { return };
        let t = ((frame.time - flight.started_at) / 0.22).min(1.0) as f32;
        if t >= 1.0 {
            self.flight = None;
            return;
        }
        let eased = if t < 0.5 { 2.0 * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(2) / 2.0 };
        let x = flight.from.0 + (flight.to.0 - flight.from.0) * eased;
        let y = flight.from.1 + (flight.to.1 - flight.from.1) * eased;
        let card = CardDraw::new(
            self.definition(&flight.definition_id),
            flight.face_up,
            x,
            y,
            &self.game.card_backs,
            self.game.scenario_card_back_id.as_deref(),
        );
        frame.draw_card(&card);
    }

    fn draw_table(&mut self, frame: &mut Frame) {
        let rect = self.table_rect();
        frame.ui.push_clip(rect.x, rect.y, rect.width, rect.height);
        self.add_item(HitItem::new(ItemKind::Table, HitShape::rect(rect.x, rect.y, rect.width, rect.height)));
        self.draw_center_marker(frame);
        self.draw_table_piles(frame);
        self.draw_table_widgets(frame);
        self.draw_arrows(frame);
        self.draw_remote_ghosts(frame);
        self.draw_flight(frame);
        self.draw_selection_box(frame);
        frame.ui.pop_clip();
    }

    // -----------------------------------------------------------------------
    // Player panels
    // -----------------------------------------------------------------------

    /// `V.handParams()`.
    pub(super) fn hand_parameters(frame: &mut Frame) -> HandParameters {
        let mut get = |setting: HandSetting| frame.hand_settings.get(frame.settings, setting) as f32;
        let scale = get(HandSetting::CardScale);
        let spacing = get(HandSetting::Spacing);
        let tilt = get(HandSetting::Tilt);
        let max_spread = get(HandSetting::MaxSpread);
        let raise = get(HandSetting::Raise);
        let reveal = get(HandSetting::Reveal);
        let (card_width, card_height) = (CARD_WIDTH * scale, CARD_HEIGHT * scale);
        let rest_visible = BAND_HEIGHT - 6.0 + raise;
        let lift = (reveal * card_height + 6.0 - rest_visible).max(0.0);
        HandParameters {
            scale,
            card_width,
            card_height,
            step: card_width * spacing,
            step_angle: tilt.to_radians(),
            max_spread: max_spread.to_radians(),
            raise,
            lift,
            overhang: raise + lift,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_hand_band(
        &mut self,
        frame: &mut Frame,
        player: &PlayerInfo,
        is_local: bool,
        x: f32,
        y: f32,
        width: f32,
        rotate: bool,
        fan_down: bool,
    ) {
        let background = self.zone_background(Some(&player.id));
        frame.fill_rect(x, y, width, BAND_HEIGHT, 0.0, background);
        let border = self.owner_border_color(Some(&player.id));
        let parameters = TableScreen::hand_parameters(frame);
        let rect = Rect::new(x, y, width, BAND_HEIGHT);
        self.with_rotated(frame, rotate, rect, |table, frame| {
            let (inner_x, inner_width) = (x + 8.0, width - 16.0);
            let local_hand = table.derived.local_hand.clone();
            let count = if is_local {
                local_hand.len()
            } else {
                table.derived.hand_count.get(&player.id).copied().unwrap_or(0)
            };
            if is_local {
                // The drop target: the band plus the strip of resting cards over the table.
                let mut drop_y = if fan_down { y } else { y - parameters.raise };
                if let Some((_, mirror_y)) = table.draw_mirror {
                    drop_y = 2.0 * mirror_y - drop_y - (BAND_HEIGHT + parameters.raise);
                }
                table.hand_rect = Some(Rect::new(x, drop_y, width, BAND_HEIGHT + parameters.raise));
                table.hand_card_centers.clear();
            }
            if count == 0 {
                return;
            }
            // fanDown mirrors the arc so a hand across the table fans toward it, cards upright.
            let sign = if fan_down { -1.0 } else { 1.0 };
            let apex_y = if fan_down {
                y + BAND_HEIGHT - 6.0 + parameters.raise - parameters.card_height / 2.0
            } else {
                y + 6.0 - parameters.raise + parameters.card_height / 2.0
            };
            let center_x = inner_x + inner_width / 2.0;
            let layout = fan_layout(&parameters, count, inner_width);
            let dragging = table.drag.as_ref().is_some_and(|drag| drag.started);
            let hovered = if is_local && !dragging {
                table.hover_item.as_ref().and_then(|item| match &item.kind {
                    ItemKind::HandCard { card, .. } => Some(card.instance_id.clone()),
                    _ => None,
                })
            } else {
                None
            };
            let easing = 1.0 - (-FAN_LIFT_RATE * table.last_dt).exp();
            // Clip only the band's far edge so a hovered card can rise over the table.
            let mut clip_y = if fan_down { y } else { y - parameters.overhang };
            let mut clip_x = inner_x;
            if let Some((mirror_x, mirror_y)) = table.draw_mirror {
                clip_x = 2.0 * mirror_x - inner_x - inner_width;
                clip_y = 2.0 * mirror_y - clip_y - (BAND_HEIGHT + parameters.overhang);
            }
            frame.ui.push_clip(clip_x, clip_y, inner_width, BAND_HEIGHT + parameters.overhang);
            for (index, (dx, dy, fan_angle)) in layout.into_iter().enumerate() {
                let card_x = center_x + dx;
                let card_y = apex_y + sign * dy;
                let angle = sign * fan_angle;
                if !is_local {
                    let back = CardDraw::new(
                        None,
                        false,
                        card_x,
                        card_y,
                        &table.game.card_backs,
                        table.game.scenario_card_back_id.as_deref(),
                    )
                    .angle(angle)
                    .scale(parameters.scale)
                    .border(border);
                    frame.draw_card(&back);
                    continue;
                }
                let card = &local_hand[index];
                let id = card.instance_id.clone();
                let resting = table.hand_lift.get(&id).copied().unwrap_or(0.0);
                let target = if hovered.as_deref() == Some(id.as_str()) { parameters.lift } else { 0.0 };
                let lift = resting + (target - resting) * easing;
                table.hand_lift.insert(id.clone(), lift);
                let screen_x = table.draw_mirror.map_or(card_x, |(mirror_x, _)| 2.0 * mirror_x - card_x);
                table.hand_card_centers.push((id.clone(), screen_x));
                // Rise along the card's own up axis; drawn and hit-tested in hand order.
                let (lift_x, lift_y) = (sign * lift * angle.sin(), -sign * lift * angle.cos());
                let draw = table
                    .card_draw(card, card_x + lift_x, card_y + lift_y)
                    .angle(angle)
                    .scale(parameters.scale)
                    .border(border)
                    .alpha(table.ghost_alpha(&id));
                frame.draw_card(&draw);
                // The hit box covers the resting and lifted positions so hover doesn't flicker.
                let (hit_x, hit_y) = (card_x + lift_x / 2.0, card_y + lift_y / 2.0);
                let shape = HitShape::rotated(
                    hit_x,
                    hit_y,
                    parameters.card_width / 2.0,
                    parameters.card_height / 2.0 + lift / 2.0,
                    angle,
                );
                let kind = ItemKind::HandCard {
                    card: card.clone(),
                    scale: parameters.scale,
                    card_center: (card_x + lift_x, card_y + lift_y),
                };
                table.add_item(HitItem::new(kind, shape).hover(Some(id)));
            }
            frame.ui.pop_clip();
        });
    }

    fn draw_zone_slot(
        &mut self,
        frame: &mut Frame,
        player: &PlayerInfo,
        zone: &ZoneDefinition,
        is_local: bool,
        x: f32,
        y: f32,
        rotate: bool,
    ) {
        let rect = Rect::new(x, y, SLOT_WIDTH, SLOT_HEIGHT);
        self.with_rotated(frame, rotate, rect, |table, frame| {
            let (center_x, center_y) = (x + SLOT_WIDTH / 2.0, y + SLOT_HEIGHT / 2.0);
            if zone.kind == ZoneKind::Widget {
                let key = (player.id.clone(), zone.id.clone());
                if let Some(widget) = table.derived.widget_for.get(&key).cloned() {
                    let interactable = is_local && !table.tab_down;
                    if zone.widget_kind == ZoneWidgetKind::TokenBag {
                        table.draw_token_bag(frame, &widget, zone, center_x, center_y, interactable);
                        if is_local {
                            let rect = Rect::new(center_x - 40.0, center_y - 40.0, 80.0, 80.0);
                            table.bag_drop_zones.push(DropZone { zone_id: zone.id.clone(), rect });
                        }
                    } else {
                        table.counter_face(frame, &widget, center_x - 40.0, center_y - 40.0, 80.0, 1.0);
                        let shape = HitShape::rect(center_x - 40.0, center_y - 40.0, 80.0, 80.0);
                        let item = HitItem::new(ItemKind::DockedCounter { widget }, shape)
                            .interactable(interactable)
                            .tooltip(zone.name.clone());
                        table.add_item(item);
                    }
                }
                return;
            }
            let key = (player.id.clone(), zone.id.clone());
            let cards = table.derived.zone_cards.get(&key).cloned().unwrap_or_default();
            let top = top_card(&cards).cloned();
            let wiggle_key = format!("zone:{}:{}", player.id, zone.id);
            let mut pile = PileBox::new(center_x, center_y, cards.len(), top.as_ref());
            pile.border = table.owner_border_color(Some(&player.id));
            pile.alpha = top.as_ref().map_or(1.0, |top| table.ghost_alpha(&top.instance_id));
            pile.wiggle_key = Some(wiggle_key.clone());
            pile.angle = orientation_turns(zone.orientation) * 2.0 * PI;
            pile.searched = table.is_zone_searched(&zone.id, Some(&player.id));
            let has_shuffle = is_local && !cards.is_empty() && zone.shuffleable;
            if has_shuffle {
                pile.shuffle = Some((wiggle_key, ShuffleTarget::Zone { zone_id: zone.id.clone() }));
            }
            let items_before = table.items.len();
            table.draw_pile_box(frame, pile);
            if is_local {
                table
                    .drop_zones
                    .push(DropZone { zone_id: zone.id.clone(), rect: Rect::new(x, y, SLOT_WIDTH, SLOT_HEIGHT) });
                let hover_id = top.as_ref().map(|top| top.instance_id.clone());
                let kind = ItemKind::Zone {
                    zone_id: zone.id.clone(),
                    zone: zone.clone(),
                    top,
                    cards,
                    shared: false,
                    deck_widget: false,
                };
                let item = HitItem::new(kind, HitShape::rect(x, y, SLOT_WIDTH, SLOT_HEIGHT))
                    .tooltip(zone.name.clone())
                    .hover(hover_id);
                let item = HitItem { mirror: table.draw_mirror, ..item };
                // Before the shuffle button, so the button wins.
                table.items.insert(items_before.min(table.items.len()), item);
            } else {
                let shape = HitShape::rect(x + 8.0, y + 8.0, SLOT_WIDTH - 16.0, SLOT_HEIGHT - 16.0);
                table.add_item(HitItem::new(ItemKind::Tooltip, shape).tooltip(zone.name.clone()));
            }
        });
    }

    fn draw_avatar_corner(&mut self, frame: &mut Frame, player: &PlayerInfo, x: f32, y: f32) {
        let is_practice = self.is_local_practice;
        let is_active = player.id == self.acting_player_id;
        let is_local = player.id == self.local_player_id;
        let show_real = (is_practice && is_active) || (!is_practice && is_local);
        let alpha = if player.connected { 1.0 } else { 0.5 };
        let name_font = theme::table_font(10.0, Weight::Regular);
        let content_height = CARD_WIDTH + 4.0 + frame.fonts.height(name_font);
        let avatar_y = y + 8.0 + (CARD_HEIGHT + PILE_EXTRA - content_height) / 2.0;
        let avatar_x = x + (SLOT_WIDTH - CARD_WIDTH) / 2.0;
        let source = if show_real {
            self.local_avatar_path.clone().map(|path| (path, true))
        } else {
            self.avatar_keys.get(&player.id).cloned().map(|key| (key, false))
        };
        if self.state.current_player_id.as_deref() == Some(player.id.as_str()) {
            let padding = 4.0;
            let color = self.current_turn_color(frame);
            frame.fill_rect(
                avatar_x - padding,
                avatar_y - padding,
                CARD_WIDTH + padding * 2.0,
                CARD_WIDTH + padding * 2.0,
                0.0,
                color,
            );
        }
        let avatar_source = source
            .as_ref()
            .map(|(path, is_path)| if *is_path { AvatarSource::Path(path) } else { AvatarSource::Key(path) });
        frame.avatar(avatar_x, avatar_y, CARD_WIDTH, player.color, avatar_source, !is_practice || is_active);
        if alpha < 1.0 {
            frame.fill_rect(avatar_x, avatar_y, CARD_WIDTH, CARD_WIDTH, 0.0, Color::rgba(0.0, 0.0, 0.0, 0.5));
        }
        let name = if player.connected { player.name.clone() } else { format!("{} (disconnected)", player.name) };
        let shown = frame.ellipsize(&name, name_font, SLOT_WIDTH - 16.0);
        let options = TextOptions::new().font(name_font).width(SLOT_WIDTH - 16.0).centered().color(Color::rgba(
            1.0,
            1.0,
            1.0,
            0.7 * alpha,
        ));
        frame.text(&shown, x + 8.0, avatar_y + CARD_WIDTH + 4.0, options);
        if is_practice {
            let shape = HitShape::rect(avatar_x, avatar_y, CARD_WIDTH, CARD_WIDTH);
            self.add_item(
                HitItem::new(ItemKind::Avatar { player_id: player.id.clone() }, shape).tooltip(player.name.clone()),
            );
        }
    }

    fn draw_player_row(
        &mut self,
        frame: &mut Frame,
        players: &[PlayerInfo],
        is_top_row: bool,
        x: f32,
        y: f32,
        width: f32,
    ) {
        if players.is_empty() {
            return;
        }
        let count = players.len();
        let panel_width = (width - 12.0 * (count as f32 - 1.0)) / count as f32;
        let owned_zones = self.derived.owned_zones.clone();
        for (index, player) in players.iter().enumerate() {
            let panel_x = x + index as f32 * (panel_width + 12.0);
            let is_local = player.id == self.acting_player_id;
            let rotate = is_top_row && player.id != self.local_player_id;
            let reverse = (count == 2 && index == 0) || (is_top_row && count == 1 && !rotate);
            let mut zones = owned_zones.clone();
            if reverse {
                zones.reverse();
            }
            // The hand spans the whole panel; zones and the avatar dock on top at its outer edge.
            let fan_down = is_top_row && !rotate;
            self.draw_hand_band(frame, player, is_local, panel_x, y, panel_width, rotate, fan_down);
            let cluster_width = (zones.len() as f32 + 1.0) * SLOT_WIDTH;
            let mut slot_x = if reverse { panel_x } else { panel_x + panel_width - cluster_width };
            if reverse {
                self.draw_avatar_corner(frame, player, slot_x, y);
                slot_x += SLOT_WIDTH;
            }
            for zone in &zones {
                self.draw_zone_slot(frame, player, zone, is_local, slot_x, y, rotate);
                slot_x += SLOT_WIDTH;
            }
            if !reverse {
                self.draw_avatar_corner(frame, player, slot_x, y);
            }
        }
    }

    fn draw_sidebar(&mut self, frame: &mut Frame, side: SharedZoneSide, x: f32, y: f32, height: f32) {
        let zones: Vec<ZoneDefinition> =
            self.derived.shared_zones.iter().filter(|zone| zone.side == side).cloned().collect();
        if zones.is_empty() {
            return;
        }
        frame.fill_rect(x, y, SLOT_WIDTH, height, 0.0, fixed::BLACK);
        let total = zones.len() as f32 * SLOT_HEIGHT + (zones.len() as f32 - 1.0) * 12.0;
        let mut slot_y = y + ((height - total) / 2.0).max(12.0);
        for zone in &zones {
            frame.fill_rect(x, slot_y, SLOT_WIDTH, SLOT_HEIGHT, 0.0, fixed::BLACK);
            let (center_x, center_y) = (x + SLOT_WIDTH / 2.0, slot_y + SLOT_HEIGHT / 2.0);
            if zone.kind == ZoneKind::Widget {
                let key = (SHARED_OWNER_KEY.to_string(), zone.id.clone());
                if let Some(widget) = self.derived.widget_for.get(&key).cloned() {
                    if zone.widget_kind == ZoneWidgetKind::TokenBag {
                        self.draw_token_bag(frame, &widget, zone, center_x, center_y, true);
                        let rect = Rect::new(center_x - 40.0, center_y - 40.0, 80.0, 80.0);
                        self.bag_drop_zones.push(DropZone { zone_id: zone.id.clone(), rect });
                    } else {
                        self.counter_face(frame, &widget, center_x - 40.0, center_y - 40.0, 80.0, 1.0);
                        let shape = HitShape::rect(center_x - 40.0, center_y - 40.0, 80.0, 80.0);
                        let item = HitItem::new(ItemKind::DockedCounter { widget }, shape)
                            .interactable(!self.tab_down)
                            .tooltip(zone.name.clone());
                        self.add_item(item);
                    }
                }
            } else {
                // Shared zone cards are unowned; only those count as the shared pile.
                let cards = self.derived.cards_by_zone_id.get(&zone.id).cloned().unwrap_or_default();
                let top = top_card(&cards).cloned();
                let key = format!("shared:{}", zone.id);
                let mut pile = PileBox::new(center_x, center_y, cards.len(), top.as_ref());
                pile.alpha = top.as_ref().map_or(1.0, |top| self.ghost_alpha(&top.instance_id));
                pile.wiggle_key = Some(key.clone());
                pile.angle = orientation_turns(zone.orientation) * 2.0 * PI;
                pile.searched = self.is_zone_searched(&zone.id, None);
                if !cards.is_empty() && zone.shuffleable {
                    pile.shuffle = Some((key, ShuffleTarget::Zone { zone_id: zone.id.clone() }));
                }
                let items_before = self.items.len();
                self.draw_pile_box(frame, pile);
                self.drop_zones
                    .push(DropZone { zone_id: zone.id.clone(), rect: Rect::new(x, slot_y, SLOT_WIDTH, SLOT_HEIGHT) });
                let hover_id = top.as_ref().map(|top| top.instance_id.clone());
                let kind = ItemKind::Zone {
                    zone_id: zone.id.clone(),
                    zone: zone.clone(),
                    top,
                    cards,
                    shared: true,
                    deck_widget: false,
                };
                let item = HitItem::new(kind, HitShape::rect(x, slot_y, SLOT_WIDTH, SLOT_HEIGHT))
                    .tooltip(zone.name.clone())
                    .hover(hover_id);
                self.items.insert(items_before.min(self.items.len()), item);
            }
            slot_y += SLOT_HEIGHT + 12.0;
        }
    }

    // -----------------------------------------------------------------------
    // The whole view
    // -----------------------------------------------------------------------

    /// `V.layout`: sidebars, the player rows and the table between them.
    fn compute_layout(&mut self, x: f32, y: f32, width: f32, height: f32) {
        let has_left = self.derived.shared_zones.iter().any(|zone| zone.side == SharedZoneSide::Left);
        let has_right = self.derived.shared_zones.iter().any(|zone| zone.side != SharedZoneSide::Left);
        let middle_x = x + if has_left { SLOT_WIDTH } else { 0.0 };
        let middle_width = width - if has_left { SLOT_WIDTH } else { 0.0 } - if has_right { SLOT_WIDTH } else { 0.0 };
        let top_height = if self.derived.top_row.is_empty() { 0.0 } else { BAND_HEIGHT };
        let bottom_height = if self.derived.bottom_row.is_empty() { 0.0 } else { BAND_HEIGHT };
        self.layout = LayoutRects {
            left: has_left.then(|| Rect::new(x, y, SLOT_WIDTH, height)),
            right: has_right.then(|| Rect::new(x + width - SLOT_WIDTH, y, SLOT_WIDTH, height)),
            top: Rect::new(middle_x, y, middle_width, top_height),
            bottom: Rect::new(middle_x, y + height - bottom_height, middle_width, bottom_height),
        };
        self.table_rect = Some(Rect::new(middle_x, y + top_height, middle_width, height - top_height - bottom_height));
    }

    /// `V.draw`.
    pub(super) fn draw_view(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        self.items.clear();
        self.drop_zones.clear();
        self.bag_drop_zones.clear();
        self.hand_rect = None;
        self.hand_card_centers.clear();
        self.compute_layout(x, y, width, height);
        let (zoom, offset_x, offset_y) = self.felt_world_offset();
        let felt = self.felt_color(frame);
        let list = frame.ui.list();
        list.set_color(felt);
        if felt_texture_enabled(frame.settings) {
            list.set_program(FragmentProgram::Felt { zoom, world_offset: [offset_x, offset_y] });
        }
        list.rectangle(DrawMode::Fill, x, y, width, height, 0.0, 0.0);
        list.set_program(FragmentProgram::Plain);
        self.draw_table(frame);
        let layout = self.layout;
        let (top_row, bottom_row) = (self.derived.top_row.clone(), self.derived.bottom_row.clone());
        self.draw_player_row(frame, &top_row, true, layout.top.x, layout.top.y, layout.top.width);
        self.draw_player_row(frame, &bottom_row, false, layout.bottom.x, layout.bottom.y, layout.bottom.width);
        if let Some(left) = layout.left {
            self.draw_sidebar(frame, SharedZoneSide::Left, left.x, left.y, left.height);
        }
        if let Some(right) = layout.right {
            self.draw_sidebar(frame, SharedZoneSide::Right, right.x, right.y, right.height);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mats_meet_at_the_centre_line() {
        assert_eq!(row_lefts(&[100.0, 200.0], 10.0), vec![1180.0, 1280.0]);
        assert_eq!(row_lefts(&[100.0, 50.0, 100.0], 10.0), vec![1145.0, 1255.0, 1315.0]);
        assert!(row_lefts(&[], 10.0).is_empty());
    }

    #[test]
    fn fans_fit_the_band() {
        let parameters = HandParameters {
            scale: 1.0,
            card_width: 70.0,
            card_height: 100.0,
            step: 42.0,
            step_angle: 3f32.to_radians(),
            max_spread: 24f32.to_radians(),
            raise: 50.0,
            lift: 0.0,
            overhang: 50.0,
        };
        assert_eq!(fan_layout(&parameters, 1, 500.0), vec![(0.0, 0.0, 0.0)]);
        let fan = fan_layout(&parameters, 5, 300.0);
        assert_eq!(fan.len(), 5);
        assert!((fan[2].0).abs() < 1e-4 && fan[0].0 < 0.0 && fan[4].0 > 0.0);
        let half_width = fan[4].0 + 35.0 * fan[4].2.cos() + 50.0 * fan[4].2.sin();
        assert!(half_width <= 150.0 + 1e-3, "{half_width}");
        let flat = HandParameters { step_angle: 0.0, ..parameters };
        let row = fan_layout(&flat, 3, 120.0);
        assert_eq!(row, vec![(-25.0, 0.0, 0.0), (0.0, 0.0, 0.0), (25.0, 0.0, 0.0)]);
    }

    #[test]
    fn deck_widget_sizes() {
        assert_eq!(deck_widget_size(0), (94.0 + 16.0, 24.0 + 8.0 + 124.0 + 8.0 + 36.0 + 16.0));
        assert_eq!(deck_widget_size(2).0, 2.0 * 94.0 + 8.0 + 16.0);
    }
}
