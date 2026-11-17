//! The immediate-mode UI core. Port of the frame, input, layer, clip, hit-test, focus and scroll
//! parts of `src/ui/kit/ui.lua`.
//!
//! Everything is drawn and hit-tested in logical screen coordinates while a frame is drawn.
//! Input arrives through the `on_*` hooks (called by the app shell for SDL events) and is
//! consumed during the next frame.
//!
//! Layers: content drawn above the screen (dialogs, menus, popups) goes in a higher layer via
//! [`Ui::begin_overlay`] / [`Ui::end_overlay`] (or `Frame::overlay`). Each layer records into its
//! own draw list, and the lists are drawn in layer order, so an overlay drawn in the middle of
//! the screen still ends up on top. Each layer declares the screen area it covers
//! ([`Ui::cover`]); a widget is hoverable only if its layer was the topmost one under the mouse in
//! the previous frame, so a dialog blocks the screen underneath it.
//!
//! The original queued overlay functions and ran them at the end of the frame; drawing them in
//! place gives the same layer numbers and stacking as long as overlays are drawn in the order
//! the original queued them (screen popups, then dialogs, then menus, then toasts).
//!
//! Widget ids: pass an explicit id when the same label can appear twice at once; otherwise the
//! id defaults to label + position ([`Ui::default_id`]).

use std::any::Any;
use std::collections::{HashMap, HashSet};

use crate::graphics::draw_list::{DrawList, ScissorRect};

/// A rectangle in logical units.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect { x, y, width, height }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

/// A rectangle covering the whole screen and beyond (`-1e9, -1e9, 2e9, 2e9`).
pub const EVERYWHERE: Rect = Rect::new(-1e9, -1e9, 2e9, 2e9);

/// One queued key press for the focused field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPress {
    pub key: &'static str,
    pub is_repeat: bool,
}

/// Mouse buttons are LÖVE's numbers: 1 left, 2 right, 3 middle, 4 and 5 the side buttons.
const BUTTON_SLOTS: usize = 6;

/// The input gathered since the last frame (`ui.input`).
#[derive(Debug, Clone, Default)]
pub struct UiInput {
    pub mouse_x: f32,
    pub mouse_y: f32,
    down: [bool; BUTTON_SLOTS],
    pressed: [bool; BUTTON_SLOTS],
    released: [bool; BUTTON_SLOTS],
    clicks: [u8; BUTTON_SLOTS],
    press_positions: [(f32, f32); BUTTON_SLOTS],
    pub wheel_x: f32,
    pub wheel_y: f32,
    /// Keys queued for the focused field.
    pub keys: Vec<KeyPress>,
    /// Text typed while a field had focus.
    pub text: Vec<String>,
}

fn slot(button: u8) -> usize {
    (button as usize).min(BUTTON_SLOTS - 1)
}

impl UiInput {
    pub fn is_down(&self, button: u8) -> bool {
        self.down[slot(button)]
    }

    pub fn was_pressed(&self, button: u8) -> bool {
        self.pressed[slot(button)]
    }

    pub fn was_released(&self, button: u8) -> bool {
        self.released[slot(button)]
    }

    /// The click count of this frame's press (2 for a double click), 0 without a press.
    pub fn click_count(&self, button: u8) -> u8 {
        self.clicks[slot(button)]
    }

    pub fn press_position(&self, button: u8) -> (f32, f32) {
        self.press_positions[slot(button)]
    }
}

/// What [`Ui::region`] reports about an interactive area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Region {
    pub hovered: bool,
    /// The left button went down inside this frame (the region captured the mouse).
    pub pressed: bool,
    /// The region holds the mouse capture and the left button is down.
    pub held: bool,
    /// The left button was released inside after pressing inside.
    pub clicked: bool,
    pub right_clicked: bool,
    pub double_clicked: bool,
}

/// Options for [`Ui::region`].
#[derive(Debug, Clone, Copy, Default)]
pub struct RegionOptions<'a> {
    /// LÖVE system cursor name shown while hovered (`"hand"`, `"ibeam"`, ...).
    pub cursor: Option<&'static str>,
    pub tooltip: Option<&'a str>,
}

impl<'a> RegionOptions<'a> {
    pub fn hand() -> RegionOptions<'a> {
        RegionOptions { cursor: Some("hand"), tooltip: None }
    }

    pub fn cursor(cursor: &'static str) -> RegionOptions<'a> {
        RegionOptions { cursor: Some(cursor), tooltip: None }
    }

    pub fn with_tooltip(self, tooltip: Option<&'a str>) -> RegionOptions<'a> {
        RegionOptions { tooltip, ..self }
    }
}

#[derive(Debug, Clone, Copy)]
struct Cover {
    layer: usize,
    area: Rect,
}

#[derive(Debug, Clone)]
pub(crate) struct Tooltip {
    pub(crate) id: String,
    pub(crate) text: String,
    pub(crate) since: f64,
}

/// Per text field: caret and selection anchor (character offsets), scroll and caret blink.
#[derive(Debug, Clone, Default)]
pub struct FieldState {
    pub caret: usize,
    pub anchor: Option<usize>,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub blink_since: f64,
    /// Remembered caret x for up/down in a text area.
    pub preferred_x: Option<f32>,
    /// The frame the field was last drawn in (None before the first).
    pub last_frame: Option<u64>,
}

/// Per scroll view.
#[derive(Debug, Clone, Default)]
pub struct ScrollState {
    pub y: f32,
    pub target: f32,
    pub content_height: f32,
    pub view: Rect,
    pub hovered: bool,
    pub drag_offset: Option<f32>,
}

/// Keys the dialogs and menus handle (`overlays.keypressed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayKey {
    Escape,
    Enter,
}

/// A dialog or menu drawn this frame, in drawing order (the last one is on top).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum OpenOverlay {
    Dialog { id: String, dismissible: bool, handles_enter: bool },
    Menu { id: String },
}

/// The UI state that lives across frames.
pub struct Ui {
    pub input: UiInput,
    screen_width: f32,
    screen_height: f32,
    scale: f32,
    time: f64,
    frame_index: u64,

    layer: usize,
    layer_count: usize,
    layers: Vec<DrawList>,
    /// Saved (layer, clip stack) for each open `begin_overlay`.
    layer_stack: Vec<(usize, Vec<Rect>)>,
    covers: Vec<Cover>,
    top_layer_at_mouse: usize,
    clip_stack: Vec<Rect>,
    cursor: &'static str,
    pub(crate) tooltip: Option<Tooltip>,
    pub(crate) tooltip_seen: bool,
    hovered_id: Option<String>,

    /// The id holding the mouse capture.
    pub active: Option<String>,
    /// The focused text field.
    pub focus: Option<String>,
    /// A focus change applied at the end of the frame: `Some(None)` blurs.
    pending_focus: Option<Option<String>>,
    pub(crate) focus_hit_this_frame: bool,
    focus_order: Vec<String>,
    last_focus_order: Vec<String>,
    pub(crate) field_states: HashMap<String, FieldState>,
    scroll_states: HashMap<String, ScrollState>,
    widget_states: HashMap<String, Box<dyn Any>>,
    pub(crate) select_all_requests: HashSet<String>,
    pub(crate) caret_end_requests: HashSet<String>,

    modal_open: bool,
    modal_open_next: bool,
    /// Top of the content area (below the custom title bar); dialog scrims start here.
    pub content_top: f32,
    open_overlays: Vec<OpenOverlay>,
    open_overlays_last_frame: Vec<OpenOverlay>,
    /// A key for one dialog or menu (by id), handled when it next draws.
    pending_overlay_key: Option<(String, OverlayKey)>,
    /// Alerts, confirmations and snackbars owned by the app rather than a screen.
    pub(crate) app_overlays: crate::ui::overlays::AppOverlays,
    /// Tests only: every region drawn, by id, as `(x, y, width, height)` (latest frame wins).
    /// `None` (the default) records nothing.
    pub recorded_regions: Option<HashMap<String, (f32, f32, f32, f32)>>,
}

impl Default for Ui {
    fn default() -> Self {
        Ui::new()
    }
}

impl Ui {
    pub fn new() -> Ui {
        Ui {
            input: UiInput::default(),
            screen_width: 1280.0,
            screen_height: 720.0,
            scale: 1.0,
            time: 0.0,
            frame_index: 0,
            layer: 0,
            layer_count: 0,
            layers: vec![DrawList::new()],
            layer_stack: Vec::new(),
            covers: Vec::new(),
            top_layer_at_mouse: 0,
            clip_stack: Vec::new(),
            cursor: "arrow",
            tooltip: None,
            tooltip_seen: false,
            hovered_id: None,
            active: None,
            focus: None,
            pending_focus: None,
            focus_hit_this_frame: false,
            focus_order: Vec::new(),
            last_focus_order: Vec::new(),
            field_states: HashMap::new(),
            scroll_states: HashMap::new(),
            widget_states: HashMap::new(),
            select_all_requests: HashSet::new(),
            caret_end_requests: HashSet::new(),
            modal_open: false,
            modal_open_next: false,
            content_top: 0.0,
            open_overlays: Vec::new(),
            open_overlays_last_frame: Vec::new(),
            pending_overlay_key: None,
            app_overlays: Default::default(),
            recorded_regions: None,
        }
    }

    // -- Misc state -------------------------------------------------------------------------

    /// `ui.state(id)`: per-id state of any type, created with `Default` on first use (or when the
    /// id was last used with another type).
    pub fn state<T: Default + 'static>(&mut self, id: &str) -> &mut T {
        let entry = self.widget_states.entry(id.to_string()).or_insert_with(|| Box::new(T::default()));
        if !entry.is::<T>() {
            *entry = Box::new(T::default());
        }
        entry.downcast_mut::<T>().expect("state type was just checked")
    }

    /// Forgets the per-id state (e.g. a popup that should start closed).
    pub fn forget_state(&mut self, id: &str) {
        self.widget_states.remove(id);
    }

    /// `ui.time()`: seconds at the start of this frame.
    pub fn time(&self) -> f64 {
        self.time
    }

    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Logical size of the window.
    pub fn screen_size(&self) -> (f32, f32) {
        (self.screen_width, self.screen_height)
    }

    /// The UI scale (logical units to physical pixels).
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// `ui.snap(v)`: rounds a logical coordinate down to a whole physical pixel.
    pub fn snap(&self, value: f32) -> f32 {
        (value * self.scale).floor() / self.scale
    }

    // -- Input hooks ------------------------------------------------------------------------

    pub fn on_mouse_pressed(&mut self, x: f32, y: f32, button: u8, presses: u8) {
        let input = &mut self.input;
        input.mouse_x = x;
        input.mouse_y = y;
        let index = slot(button);
        input.down[index] = true;
        input.pressed[index] = true;
        input.clicks[index] = presses.max(1);
        input.press_positions[index] = (x, y);
    }

    pub fn on_mouse_released(&mut self, x: f32, y: f32, button: u8) {
        let input = &mut self.input;
        input.mouse_x = x;
        input.mouse_y = y;
        let index = slot(button);
        input.down[index] = false;
        input.released[index] = true;
    }

    pub fn on_mouse_moved(&mut self, x: f32, y: f32) {
        self.input.mouse_x = x;
        self.input.mouse_y = y;
    }

    pub fn on_wheel(&mut self, dx: f32, dy: f32) {
        self.input.wheel_x += dx;
        self.input.wheel_y += dy;
    }

    /// Returns true when the UI wants this key (a field has focus).
    pub fn on_key_pressed(&mut self, key: &'static str, is_repeat: bool) -> bool {
        if self.focus.is_some() {
            self.input.keys.push(KeyPress { key, is_repeat });
            return true;
        }
        false
    }

    pub fn on_text_input(&mut self, text: &str) -> bool {
        if self.focus.is_some() {
            self.input.text.push(text.to_string());
            return true;
        }
        false
    }

    /// The window lost focus: buttons held now won't report their release.
    pub fn release_all_buttons(&mut self) {
        self.input.down = [false; BUTTON_SLOTS];
    }

    /// True when a modal layer (dialog) covered everything last frame.
    pub fn modal_open(&self) -> bool {
        self.modal_open
    }

    // -- Frame lifecycle ----------------------------------------------------------------------

    /// Starts a frame of `width × height` logical units drawn at `scale`.
    pub fn begin_frame(&mut self, width: f32, height: f32, scale: f32, time: f64) {
        self.screen_width = width;
        self.screen_height = height;
        self.scale = scale;
        self.time = time;
        self.frame_index += 1;
        self.layer = 0;
        self.layer_count = 0;
        self.layers = vec![self.new_layer_list()];
        self.layer_stack.clear();
        self.covers = vec![Cover { layer: 0, area: EVERYWHERE }];
        self.clip_stack.clear();
        self.focus_order.clear();
        self.cursor = "arrow";
        self.hovered_id = None;
        self.modal_open_next = false;
        self.open_overlays.clear();
    }

    fn new_layer_list(&self) -> DrawList {
        let mut list = DrawList::new();
        list.scale(self.scale, self.scale);
        list
    }

    /// Opens a list for the tooltip above every layer (drawn after them all).
    pub(crate) fn begin_top_layer(&mut self) {
        self.layer_count += 1;
        self.layer = self.layer_count;
        self.layers.push(self.new_layer_list());
        self.clip_stack.clear();
    }

    /// Ends the frame: settles hover, capture and focus for the next one and returns the frame's
    /// drawing, layer by layer, in one list. The caller draws the tooltip first.
    pub fn end_frame(&mut self) -> DrawList {
        while !self.layer_stack.is_empty() {
            self.end_overlay();
        }
        self.layer = 0;

        // The topmost layer under the mouse decides next frame's hover.
        let (mouse_x, mouse_y) = (self.input.mouse_x, self.input.mouse_y);
        self.top_layer_at_mouse = self
            .covers
            .iter()
            .filter(|cover| cover.area.contains(mouse_x, mouse_y))
            .map(|cover| cover.layer)
            .max()
            .unwrap_or(0);
        self.modal_open = self.modal_open_next;

        // Release the capture when the button goes up.
        if self.input.was_released(1) && !self.input.is_down(1) {
            self.active = None;
        }
        // Clicking anywhere outside the focused field unfocuses it.
        if self.input.was_pressed(1) && self.focus.is_some() && !self.focus_hit_this_frame {
            self.focus = None;
        }
        self.focus_hit_this_frame = false;
        // A focused field that was not drawn this frame (its dialog closed, its screen left) gives
        // up focus, or it would keep swallowing keys. The original blurred when closing dialogs.
        if self.focus.as_ref().is_some_and(|focus| !self.focus_order.contains(focus)) {
            self.focus = None;
        }
        if let Some(focus) = self.pending_focus.take() {
            self.focus = focus;
        }
        self.last_focus_order = std::mem::take(&mut self.focus_order);
        self.open_overlays_last_frame = std::mem::take(&mut self.open_overlays);
        self.pending_overlay_key = None;

        let input = &mut self.input;
        input.pressed = [false; BUTTON_SLOTS];
        input.released = [false; BUTTON_SLOTS];
        input.clicks = [0; BUTTON_SLOTS];
        input.wheel_x = 0.0;
        input.wheel_y = 0.0;
        input.keys.clear();
        input.text.clear();

        let mut merged = DrawList::new();
        for list in std::mem::take(&mut self.layers) {
            merged.append(&list);
        }
        self.layers = vec![self.new_layer_list()];
        merged
    }

    /// The mouse cursor the frame asked for (a LÖVE system cursor name).
    pub fn cursor(&self) -> &'static str {
        self.cursor
    }

    pub fn set_cursor(&mut self, name: &'static str) {
        self.cursor = name;
    }

    /// The draw list of the current layer.
    pub fn list(&mut self) -> &mut DrawList {
        &mut self.layers[self.layer]
    }

    /// Starts drawing in a new layer above everything so far (`ui.overlay(fn)` run in place).
    /// Overlays start without clipping.
    pub fn begin_overlay(&mut self) {
        let saved_clips = std::mem::take(&mut self.clip_stack);
        self.layer_stack.push((self.layer, saved_clips));
        self.layer_count += 1;
        self.layer = self.layer_count;
        self.layers.push(self.new_layer_list());
    }

    /// Returns to the layer that was current before the matching `begin_overlay`.
    pub fn end_overlay(&mut self) {
        if let Some((layer, clips)) = self.layer_stack.pop() {
            self.layer = layer;
            self.clip_stack = clips;
        }
    }

    /// Declares that the current layer covers a screen area (blocks the layers below).
    pub fn cover(&mut self, x: f32, y: f32, width: f32, height: f32) {
        self.covers.push(Cover { layer: self.layer, area: Rect::new(x, y, width, height) });
    }

    /// A full-screen modal cover for the current layer.
    pub fn cover_all(&mut self) {
        self.cover(EVERYWHERE.x, EVERYWHERE.y, EVERYWHERE.width, EVERYWHERE.height);
        self.modal_open_next = true;
    }

    /// Marks the frame as showing a modal (dialogs do this with a cover below the title bar).
    pub fn mark_modal(&mut self) {
        self.modal_open_next = true;
    }

    pub fn layer(&self) -> usize {
        self.layer
    }

    // -- Clipping ----------------------------------------------------------------------------

    /// `love.graphics.setScissor` in logical units for the current layer.
    pub fn set_scissor(&mut self, area: Option<Rect>) {
        let scale = self.scale;
        let scissor = area.map(|area| logical_scissor(area, scale));
        self.layers[self.layer].set_scissor(scissor);
    }

    pub fn current_clip(&self) -> Option<Rect> {
        self.clip_stack.last().copied()
    }

    pub fn push_clip(&mut self, x: f32, y: f32, width: f32, height: f32) {
        let mut area = Rect::new(x, y, width, height);
        if let Some(clip) = self.current_clip() {
            let right = (x + width).min(clip.x + clip.width);
            let bottom = (y + height).min(clip.y + clip.height);
            area.x = x.max(clip.x);
            area.y = y.max(clip.y);
            area.width = (right - area.x).max(0.0);
            area.height = (bottom - area.y).max(0.0);
        }
        self.clip_stack.push(area);
        self.set_scissor(Some(area));
    }

    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
        self.set_scissor(self.current_clip());
    }

    // -- Hit testing ---------------------------------------------------------------------------

    /// Is the mouse over this rect, in the active layer, inside the clip?
    pub fn hover(&self, x: f32, y: f32, width: f32, height: f32) -> bool {
        if self.layer != self.top_layer_at_mouse {
            return false;
        }
        let (mouse_x, mouse_y) = (self.input.mouse_x, self.input.mouse_y);
        if !Rect::new(x, y, width, height).contains(mouse_x, mouse_y) {
            return false;
        }
        self.current_clip().is_none_or(|clip| clip.contains(mouse_x, mouse_y))
    }

    pub fn hover_ignoring_layer(&self, x: f32, y: f32, width: f32, height: f32) -> bool {
        Rect::new(x, y, width, height).contains(self.input.mouse_x, self.input.mouse_y)
    }

    /// True when nothing above the base layer was under the mouse last frame (for screens that
    /// hit-test on their own).
    pub fn base_has_mouse(&self) -> bool {
        self.top_layer_at_mouse == 0
    }

    pub fn mouse(&self) -> (f32, f32) {
        (self.input.mouse_x, self.input.mouse_y)
    }

    pub fn mouse_down(&self, button: u8) -> bool {
        self.input.is_down(button)
    }

    pub fn mouse_pressed(&self, button: u8) -> bool {
        self.input.was_pressed(button)
    }

    pub fn mouse_released(&self, button: u8) -> bool {
        self.input.was_released(button)
    }

    pub fn click_count(&self, button: u8) -> u8 {
        self.input.click_count(button)
    }

    pub fn wheel(&self) -> (f32, f32) {
        (self.input.wheel_x, self.input.wheel_y)
    }

    pub fn consume_wheel(&mut self) {
        self.input.wheel_x = 0.0;
        self.input.wheel_y = 0.0;
    }

    /// A generic interactive area.
    pub fn region(&mut self, id: &str, x: f32, y: f32, width: f32, height: f32, options: RegionOptions) -> Region {
        if let Some(recorded) = &mut self.recorded_regions {
            recorded.insert(id.to_string(), (x, y, width, height));
        }
        let mut region = Region { hovered: self.hover(x, y, width, height), ..Region::default() };
        if region.hovered {
            self.hovered_id = Some(id.to_string());
        }
        if region.hovered && self.input.was_pressed(1) {
            self.active = Some(id.to_string());
            region.pressed = true;
            if self.input.click_count(1) >= 2 {
                region.double_clicked = true;
            }
        }
        if region.hovered && self.input.was_pressed(2) {
            region.right_clicked = true;
        }
        let is_active = self.active.as_deref() == Some(id);
        region.held = is_active && self.input.is_down(1);
        if is_active && self.input.was_released(1) && region.hovered {
            region.clicked = true;
        }
        if region.hovered {
            if let Some(cursor) = options.cursor {
                self.cursor = cursor;
            }
            if let Some(tooltip) = options.tooltip {
                self.tooltip(id, tooltip);
            }
        }
        region
    }

    /// The id a widget gets without an explicit one: label + position + layer.
    pub fn default_id(&self, label: &str, x: f32, y: f32) -> String {
        format!("{label}@{},{}#{}", x.floor() as i64, y.floor() as i64, self.layer)
    }

    /// The id of the region under the mouse this frame, if any so far.
    pub fn hovered_id(&self) -> Option<&str> {
        self.hovered_id.as_deref()
    }

    // -- Tooltips -----------------------------------------------------------------------------

    pub fn tooltip(&mut self, id: &str, text: &str) {
        match &mut self.tooltip {
            Some(tooltip) if tooltip.id == id => tooltip.text = text.to_string(),
            _ => self.tooltip = Some(Tooltip { id: id.to_string(), text: text.to_string(), since: self.time }),
        }
        self.tooltip_seen = true;
    }

    // -- Focus ----------------------------------------------------------------------------------

    /// Focuses a field at the end of the frame; with `select_all` it selects its whole text when
    /// it next draws.
    pub fn focus_field(&mut self, id: &str, select_all: bool) {
        self.pending_focus = Some(Some(id.to_string()));
        if select_all {
            self.select_all_requests.insert(id.to_string());
        }
    }

    /// Focuses a field with its caret at the end and nothing selected (after filling it in
    /// programmatically).
    pub fn focus_field_at_end(&mut self, id: &str) {
        self.pending_focus = Some(Some(id.to_string()));
        self.caret_end_requests.insert(id.to_string());
    }

    pub fn blur(&mut self) {
        self.pending_focus = Some(None);
    }

    pub(crate) fn request_focus(&mut self, id: &str) {
        self.pending_focus = Some(Some(id.to_string()));
    }

    pub(crate) fn register_focusable(&mut self, id: &str) {
        self.focus_order.push(id.to_string());
    }

    /// Tab traversal: focus the next (or previous) field in last frame's drawing order, with
    /// all its text selected.
    pub(crate) fn focus_next_field(&mut self, from_id: &str, backwards: bool) {
        let order = if self.last_focus_order.is_empty() { &self.focus_order } else { &self.last_focus_order };
        let Some(index) = order.iter().position(|id| id == from_id) else { return };
        let count = order.len();
        let next = if backwards { (index + count - 1) % count } else { (index + 1) % count };
        let next_id = order[next].clone();
        if let Some(state) = self.field_states.get_mut(&next_id) {
            state.anchor = Some(0);
            state.caret = usize::MAX;
        }
        self.pending_focus = Some(Some(next_id));
    }

    // -- Scroll views ---------------------------------------------------------------------------

    pub(crate) fn scroll_state(&mut self, id: &str) -> &mut ScrollState {
        self.scroll_states.entry(id.to_string()).or_default()
    }

    /// Jumps a scroll view to `y`.
    pub fn scroll_to(&mut self, id: &str, y: f32) {
        let state = self.scroll_state(id);
        state.target = y;
        state.y = y;
    }

    pub fn scroll_to_end(&mut self, id: &str) {
        if let Some(state) = self.scroll_states.get_mut(id) {
            state.target = (state.content_height - state.view.height).max(0.0);
            state.y = state.target;
        }
    }

    pub fn scroll_offset(&self, id: &str) -> f32 {
        self.scroll_states.get(id).map_or(0.0, |state| state.y)
    }

    // -- Dialogs and menus (overlays.keypressed) ---------------------------------------------------

    pub(crate) fn register_open_overlay(&mut self, overlay: OpenOverlay) {
        self.open_overlays.push(overlay);
    }

    /// `overlays.anyOpen()`: a dialog or menu was shown last frame.
    pub fn any_overlay_open(&self) -> bool {
        !self.open_overlays_last_frame.is_empty()
    }

    pub fn menu_open(&self) -> bool {
        self.open_overlays_last_frame.iter().any(|overlay| matches!(overlay, OpenOverlay::Menu { .. }))
    }

    /// `overlays.keypressed(key)`: Escape closes the top menu level, else the top dismissible
    /// dialog; Enter goes to the top dialog when it handles it. Returns true when a dialog or
    /// menu is open (the key goes no further).
    pub fn overlay_key_pressed(&mut self, key: &str) -> bool {
        let open = &self.open_overlays_last_frame;
        let top_menu = open.iter().rev().find_map(|overlay| match overlay {
            OpenOverlay::Menu { id } => Some(id.clone()),
            OpenOverlay::Dialog { .. } => None,
        });
        let top_dialog = open.iter().rev().find_map(|overlay| match overlay {
            OpenOverlay::Dialog { id, dismissible, handles_enter } => Some((id.clone(), *dismissible, *handles_enter)),
            OpenOverlay::Menu { .. } => None,
        });
        if key == "escape" {
            if let Some(menu_id) = top_menu {
                self.pending_overlay_key = Some((menu_id, OverlayKey::Escape));
                return true;
            }
            if let Some((dialog_id, true, _)) = &top_dialog {
                self.pending_overlay_key = Some((dialog_id.clone(), OverlayKey::Escape));
                return true;
            }
        }
        if (key == "return" || key == "kpenter")
            && let Some((dialog_id, _, true)) = &top_dialog
        {
            self.pending_overlay_key = Some((dialog_id.clone(), OverlayKey::Enter));
            return true;
        }
        !self.open_overlays_last_frame.is_empty()
    }

    /// The key waiting for the dialog or menu `id`, taken.
    pub(crate) fn take_overlay_key(&mut self, id: &str) -> Option<OverlayKey> {
        if self.pending_overlay_key.as_ref().is_some_and(|(target, _)| target == id) {
            return self.pending_overlay_key.take().map(|(_, key)| key);
        }
        None
    }

    /// The id of the top dialog last frame (scrim clicks only close the top one).
    pub(crate) fn top_dialog_last_frame(&self) -> Option<&str> {
        self.open_overlays_last_frame.iter().rev().find_map(|overlay| match overlay {
            OpenOverlay::Dialog { id, .. } => Some(id.as_str()),
            OpenOverlay::Menu { .. } => None,
        })
    }
}

/// A logical rectangle as a physical-pixel scissor, rounded outward.
pub fn logical_scissor(area: Rect, scale: f32) -> ScissorRect {
    let x1 = (area.x * scale).floor();
    let y1 = (area.y * scale).floor();
    ScissorRect {
        x: x1 as i32,
        y: y1 as i32,
        width: (((area.x + area.width) * scale).ceil() - x1).max(0.0) as i32,
        height: (((area.y + area.height) * scale).ceil() - y1).max(0.0) as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ui: &mut Ui) {
        ui.begin_frame(800.0, 600.0, 1.0, 0.0);
    }

    #[test]
    fn click_needs_press_and_release_inside() {
        let mut ui = Ui::new();
        ui.on_mouse_moved(10.0, 10.0);
        frame(&mut ui);
        let _ = ui.end_frame();

        ui.on_mouse_pressed(10.0, 10.0, 1, 1);
        frame(&mut ui);
        let region = ui.region("button", 0.0, 0.0, 20.0, 20.0, RegionOptions::default());
        assert!(region.pressed && region.held && !region.clicked);
        let _ = ui.end_frame();

        ui.on_mouse_released(10.0, 10.0, 1);
        frame(&mut ui);
        let region = ui.region("button", 0.0, 0.0, 20.0, 20.0, RegionOptions::default());
        assert!(region.clicked && !region.held);
        let _ = ui.end_frame();
        assert!(ui.active.is_none());
    }

    #[test]
    fn an_overlay_cover_blocks_the_layers_below_next_frame() {
        let mut ui = Ui::new();
        ui.on_mouse_moved(50.0, 50.0);
        frame(&mut ui);
        ui.begin_overlay();
        ui.cover(0.0, 0.0, 100.0, 100.0);
        ui.end_overlay();
        let _ = ui.end_frame();

        frame(&mut ui);
        assert!(!ui.hover(0.0, 0.0, 100.0, 100.0), "the base layer is covered");
        ui.begin_overlay();
        assert_eq!(ui.layer(), 1);
        assert!(ui.hover(0.0, 0.0, 100.0, 100.0));
        ui.end_overlay();
        let _ = ui.end_frame();
    }

    #[test]
    fn clips_intersect_and_limit_hover() {
        let mut ui = Ui::new();
        ui.on_mouse_moved(30.0, 30.0);
        frame(&mut ui);
        let _ = ui.end_frame();
        frame(&mut ui);
        ui.push_clip(0.0, 0.0, 100.0, 100.0);
        ui.push_clip(50.0, 50.0, 100.0, 100.0);
        assert_eq!(ui.current_clip(), Some(Rect::new(50.0, 50.0, 50.0, 50.0)));
        assert!(!ui.hover(0.0, 0.0, 100.0, 100.0));
        ui.pop_clip();
        assert!(ui.hover(0.0, 0.0, 100.0, 100.0));
        ui.pop_clip();
        let _ = ui.end_frame();
    }

    #[test]
    fn layers_are_drawn_in_order() {
        let mut ui = Ui::new();
        frame(&mut ui);
        ui.list().rectangle(crate::graphics::tessellation::DrawMode::Fill, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0);
        ui.begin_overlay();
        ui.list().set_color(crate::graphics::draw_list::Color::rgba(1.0, 0.0, 0.0, 1.0));
        ui.list().rectangle(crate::graphics::tessellation::DrawMode::Fill, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0);
        ui.end_overlay();
        ui.list().set_color(crate::graphics::draw_list::Color::rgba(0.0, 0.0, 1.0, 1.0));
        ui.list().rectangle(crate::graphics::tessellation::DrawMode::Fill, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0);
        let merged = ui.end_frame();
        let colors: Vec<[f32; 4]> = merged.vertices().iter().step_by(6).map(|vertex| vertex.color).collect();
        assert_eq!(colors, vec![[1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 1.0, 1.0], [1.0, 0.0, 0.0, 1.0]]);
    }

    #[test]
    fn escape_goes_to_the_top_menu_then_the_top_dismissible_dialog() {
        let mut ui = Ui::new();
        frame(&mut ui);
        ui.register_open_overlay(OpenOverlay::Dialog { id: "a".into(), dismissible: true, handles_enter: false });
        ui.register_open_overlay(OpenOverlay::Menu { id: "m".into() });
        let _ = ui.end_frame();
        assert!(ui.overlay_key_pressed("escape"));
        assert_eq!(ui.take_overlay_key("a"), None);
        assert_eq!(ui.take_overlay_key("m"), Some(OverlayKey::Escape));

        frame(&mut ui);
        ui.register_open_overlay(OpenOverlay::Dialog { id: "a".into(), dismissible: false, handles_enter: true });
        let _ = ui.end_frame();
        assert!(ui.overlay_key_pressed("escape"), "a dialog is open, so the key is swallowed");
        assert_eq!(ui.take_overlay_key("a"), None, "not dismissible");
        assert!(ui.overlay_key_pressed("return"));
        assert_eq!(ui.take_overlay_key("a"), Some(OverlayKey::Enter));
    }

    #[test]
    fn widget_state_is_typed() {
        let mut ui = Ui::new();
        *ui.state::<u32>("x") = 5;
        assert_eq!(*ui.state::<u32>("x"), 5);
        assert_eq!(ui.state::<String>("x").as_str(), "", "another type starts over");
    }
}
