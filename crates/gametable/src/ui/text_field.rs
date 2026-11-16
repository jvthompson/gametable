//! Text fields of `src/ui/kit/ui.lua`: the single-line outlined `textField` with a floating
//! label and the word-wrapping `textArea`. Both keep the value as one flat string; caret and
//! selection are character offsets. Enter submits, Escape blurs, Tab moves focus to the next
//! field (in last frame's drawing order), Ctrl+A/C/X/V select and use the clipboard (pasted
//! line breaks become spaces), Ctrl+Left/Right/Backspace/Delete work on words.

use crate::app::Frame;
use crate::graphics::draw_list::Color;
use crate::graphics::text::FontKey;
use crate::ui::context::{FieldState, RegionOptions};
use crate::ui::draw::TextOptions;
use crate::ui::theme::{self, Weight, with_alpha};

/// Options for [`Frame::text_field`] and [`Frame::text_area`].
#[derive(Clone, Copy, Default)]
pub struct TextFieldOptions<'a> {
    /// The floating label (shown inside the empty field, above it otherwise).
    pub label: Option<&'a str>,
    pub hint: Option<&'a str>,
    /// Fixed text before the value (`#` for colours).
    pub prefix: Option<&'a str>,
    /// An error message under the field (red border).
    pub error: Option<&'a str>,
    pub disabled: bool,
    pub password: bool,
    pub max_length: Option<usize>,
    /// Field height (default 48).
    pub height: Option<f32>,
    /// Takes focus whenever the field (re)appears.
    pub autofocus: bool,
    /// Typed or pasted text is inserted only when this accepts it.
    pub filter: Option<&'a dyn Fn(&str) -> bool>,
    pub select_all_on_focus: bool,
    /// Tab does nothing.
    pub no_tab: bool,
    /// Tab is reported in the response instead of moving focus (`opts.onTab`).
    pub custom_tab: bool,
    /// Text size (default 16).
    pub size: Option<f32>,
    pub background: Option<Color>,
    /// Behind the floating label (default: the surface colour).
    pub label_background: Option<Color>,
    /// Width reserved on the right for something the caller draws.
    pub trailing_width: f32,
}

impl<'a> TextFieldOptions<'a> {
    pub fn label(label: &'a str) -> TextFieldOptions<'a> {
        TextFieldOptions { label: Some(label), ..TextFieldOptions::default() }
    }
}

/// What a text field reports for a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextFieldResponse {
    pub changed: bool,
    /// Enter was pressed.
    pub submitted: bool,
    /// Tab was pressed with `custom_tab` (true with Shift).
    pub tab: Option<bool>,
}

/// `ui.textFieldHeight(opts)`: the field plus its error line.
pub fn text_field_height(height: Option<f32>, error: Option<&str>) -> f32 {
    height.unwrap_or(48.0) + if error.is_some_and(|error| !error.is_empty()) { 20.0 } else { 0.0 }
}

pub fn char_count(text: &str) -> usize {
    text.chars().count()
}

/// The byte offset after `count` characters.
fn byte_offset(text: &str, count: usize) -> usize {
    text.char_indices().nth(count).map_or(text.len(), |(offset, _)| offset)
}

/// Characters `[from, to)`.
pub fn char_slice(text: &str, from: usize, to: usize) -> &str {
    let start = byte_offset(text, from);
    let end = byte_offset(text, to.max(from));
    &text[start..end]
}

fn is_word_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

/// Ctrl+Left: the start of the word before `position`.
pub fn word_left(text: &str, position: usize) -> usize {
    let characters: Vec<char> = text.chars().collect();
    let mut index = position.min(characters.len());
    while index > 0 && !is_word_character(characters[index - 1]) {
        index -= 1;
    }
    while index > 0 && is_word_character(characters[index - 1]) {
        index -= 1;
    }
    index
}

/// Ctrl+Right: the end of the word after `position`.
pub fn word_right(text: &str, position: usize) -> usize {
    let characters: Vec<char> = text.chars().collect();
    let mut index = position.min(characters.len());
    while index < characters.len() && !is_word_character(characters[index]) {
        index += 1;
    }
    while index < characters.len() && is_word_character(characters[index]) {
        index += 1;
    }
    index
}

/// The wrapped lines of a text area and the character offset where each starts.
struct AreaLayout {
    lines: Vec<String>,
    starts: Vec<usize>,
}

impl AreaLayout {
    fn new(frame: &mut Frame, font: FontKey, text: &str, width: f32) -> AreaLayout {
        let (_, wrapped) = frame.fonts.wrap(font, text, width);
        let mut lines: Vec<String> = wrapped.into_iter().map(|line| line.text).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let total = char_count(text);
        let mut starts = Vec::with_capacity(lines.len());
        let mut position = 0;
        for line in &lines {
            starts.push(position);
            position += char_count(line);
            // A wrap point consumes one space (`getWrap`).
            if position < total && char_slice(text, position, position + 1) == " " {
                position += 1;
            }
        }
        AreaLayout { lines, starts }
    }

    fn line_of(&self, index: usize) -> usize {
        self.starts.iter().rposition(|&start| index >= start).unwrap_or(0)
    }
}

/// The caret index whose x is nearest `relative_x` in `line` (binary search on prefix widths).
fn caret_in_text(frame: &mut Frame, font: FontKey, line: &str, relative_x: f32) -> usize {
    let count = char_count(line);
    let width_at = |frame: &mut Frame, index: usize| {
        if index == 0 { 0.0 } else { frame.fonts.width(font, char_slice(line, 0, index)) }
    };
    if relative_x <= 0.0 {
        return 0;
    }
    if relative_x >= width_at(frame, count) {
        return count;
    }
    let (mut low, mut high) = (0, count);
    while low < high {
        let middle = (low + high).div_ceil(2);
        if width_at(frame, middle) <= relative_x {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let distance_low = (relative_x - width_at(frame, low)).abs();
    let distance_high = if low < count { (relative_x - width_at(frame, low + 1)).abs() } else { f32::INFINITY };
    if distance_high < distance_low { low + 1 } else { low }
}

/// The value being edited plus its caret state.
struct Edit<'t> {
    text: &'t mut String,
    state: FieldState,
    changed: bool,
}

impl Edit<'_> {
    fn len(&self) -> usize {
        char_count(self.text)
    }

    fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.state.anchor?;
        (anchor != self.state.caret).then(|| (anchor.min(self.state.caret), anchor.max(self.state.caret)))
    }

    fn replace(&mut self, from: usize, to: usize, with: &str) {
        let start = byte_offset(self.text, from);
        let end = byte_offset(self.text, to);
        self.text.replace_range(start..end, with);
        self.changed = true;
    }

    fn delete_selection(&mut self) -> bool {
        let Some((from, to)) = self.selection() else { return false };
        self.replace(from, to, "");
        self.state.caret = from;
        self.state.anchor = None;
        true
    }

    fn insert(&mut self, inserted: &str, options: &TextFieldOptions) {
        if let Some(filter) = options.filter
            && !filter(inserted)
        {
            return;
        }
        self.delete_selection();
        let mut inserted = inserted.to_string();
        if let Some(max_length) = options.max_length
            && self.len() + char_count(&inserted) > max_length
        {
            let room = max_length.saturating_sub(self.len());
            inserted = char_slice(&inserted, 0, room).to_string();
        }
        let caret = self.state.caret;
        self.replace(caret, caret, &inserted);
        self.state.caret = caret + char_count(&inserted);
        self.state.anchor = None;
    }

    /// Moves the caret, extending the selection with Shift.
    fn move_to(&mut self, to: isize, shift: bool) {
        if shift {
            self.state.anchor = Some(self.state.anchor.unwrap_or(self.state.caret));
        } else {
            self.state.anchor = None;
        }
        self.state.caret = to.clamp(0, self.len() as isize) as usize;
    }
}

impl Frame<'_> {
    /// Sets up a field's state for this frame (focus requests, clamping) and returns it.
    fn begin_field(&mut self, id: &str, text: &str, options: &TextFieldOptions) -> FieldState {
        let time = self.ui.time();
        let frame_index = self.ui.frame_index();
        let length = char_count(text);
        let mut state = self.ui.field_states.get(id).cloned().unwrap_or_else(|| FieldState {
            caret: length,
            blink_since: time,
            ..FieldState::default()
        });
        // Autofocus whenever the field (re)appears, e.g. a dialog opened again.
        if options.autofocus && state.last_frame.is_none_or(|last| last + 1 != frame_index) {
            self.ui.request_focus(id);
        }
        state.last_frame = Some(frame_index);
        if self.ui.select_all_requests.remove(id) {
            state.anchor = Some(0);
            state.caret = length;
            state.scroll_x = 0.0;
            state.scroll_y = 0.0;
        }
        if self.ui.caret_end_requests.remove(id) {
            state.caret = length;
            state.anchor = None;
        }
        state.caret = state.caret.min(length);
        state.anchor = state.anchor.map(|anchor| anchor.min(length));
        // An empty selection must not linger, or the next insert would leave a stale anchor.
        if state.anchor == Some(state.caret) {
            state.anchor = None;
        }
        if !options.disabled {
            self.ui.register_focusable(id);
        }
        state
    }

    /// Mouse handling shared by both fields: focus, click to place, double-click selects a word,
    /// Shift+click extends, drag selects. `caret_at_mouse` maps the mouse to a caret index.
    fn field_mouse(
        &mut self,
        id: &str,
        edit: &mut Edit,
        pressed: bool,
        held: bool,
        options: &TextFieldOptions,
        caret_at_mouse: &mut dyn FnMut(&mut Frame, &str) -> usize,
    ) {
        let focused = self.ui.focus.as_deref() == Some(id);
        let double_click = self.ui.click_count(1) >= 2;
        if pressed {
            self.ui.focus_hit_this_frame = true;
            if !focused {
                self.ui.request_focus(id);
                if options.select_all_on_focus {
                    edit.state.anchor = Some(0);
                    edit.state.caret = edit.len();
                } else {
                    edit.state.caret = caret_at_mouse(self, edit.text);
                    edit.state.anchor = None;
                }
            } else {
                let position = caret_at_mouse(self, edit.text);
                if double_click {
                    edit.state.anchor = Some(word_left(edit.text, position));
                    edit.state.caret = word_right(edit.text, position);
                } else if self.keyboard.shift() {
                    edit.state.anchor = Some(edit.state.anchor.unwrap_or(edit.state.caret));
                    edit.state.caret = position;
                } else {
                    edit.state.caret = position;
                    edit.state.anchor = None;
                }
            }
            edit.state.blink_since = self.ui.time();
            edit.state.preferred_x = None;
        } else if held && focused && self.ui.mouse_down(1) && !double_click {
            let position = caret_at_mouse(self, edit.text);
            if position != edit.state.caret {
                edit.state.anchor = Some(edit.state.anchor.unwrap_or(edit.state.caret));
                edit.state.caret = position;
                edit.state.preferred_x = None;
            }
        }
    }

    /// Typed text and keys for a focused field. `area` adds Up/Down and line-wise Home/End.
    fn field_keys(
        &mut self,
        id: &str,
        edit: &mut Edit,
        options: &TextFieldOptions,
        area: Option<(&AreaLayout, FontKey)>,
        response: &mut TextFieldResponse,
    ) {
        let typed: Vec<String> = self.ui.input.text.clone();
        for text in &typed {
            edit.insert(text, options);
        }
        let ctrl = self.keyboard.ctrl();
        let shift = self.keyboard.shift();
        let keys = self.ui.input.keys.clone();
        for key_press in keys {
            let key = key_press.key;
            let length = edit.len();
            let caret = edit.state.caret as isize;
            match key {
                "left" => {
                    edit.state.preferred_x = None;
                    match edit.selection() {
                        Some((from, _)) if !shift => {
                            edit.state.caret = from;
                            edit.state.anchor = None;
                        }
                        _ => {
                            let to = if ctrl { word_left(edit.text, edit.state.caret) as isize } else { caret - 1 };
                            edit.move_to(to, shift);
                        }
                    }
                }
                "right" => {
                    edit.state.preferred_x = None;
                    match edit.selection() {
                        Some((_, to)) if !shift => {
                            edit.state.caret = to;
                            edit.state.anchor = None;
                        }
                        _ => {
                            let to = if ctrl { word_right(edit.text, edit.state.caret) as isize } else { caret + 1 };
                            edit.move_to(to, shift);
                        }
                    }
                }
                "up" | "down" if area.is_some() => {
                    let (layout, font) = area.expect("checked above");
                    let line = layout.line_of(edit.state.caret);
                    let current_x = match edit.state.preferred_x {
                        Some(x) => x,
                        None => {
                            let column = edit.state.caret - layout.starts[line];
                            self.fonts.width(font, char_slice(&layout.lines[line], 0, column))
                        }
                    };
                    edit.state.preferred_x = Some(current_x);
                    let target = if key == "up" { line as isize - 1 } else { line as isize + 1 };
                    if target < 0 {
                        edit.move_to(0, shift);
                    } else if target as usize >= layout.lines.len() {
                        edit.move_to(length as isize, shift);
                    } else {
                        let target = target as usize;
                        if shift {
                            edit.state.anchor = Some(edit.state.anchor.unwrap_or(edit.state.caret));
                        } else {
                            edit.state.anchor = None;
                        }
                        let column = caret_in_text(self, font, &layout.lines[target], current_x);
                        edit.state.caret = layout.starts[target] + column;
                    }
                }
                "home" => {
                    edit.state.preferred_x = None;
                    let to = area.map_or(0, |(layout, _)| layout.starts[layout.line_of(edit.state.caret)]);
                    edit.move_to(to as isize, shift);
                }
                "end" => {
                    edit.state.preferred_x = None;
                    let to = area.map_or(length, |(layout, _)| {
                        let line = layout.line_of(edit.state.caret);
                        layout.starts[line] + char_count(&layout.lines[line])
                    });
                    edit.move_to(to as isize, shift);
                }
                "backspace" => {
                    edit.state.preferred_x = None;
                    if !edit.delete_selection() && edit.state.caret > 0 {
                        let from = if ctrl { word_left(edit.text, edit.state.caret) } else { edit.state.caret - 1 };
                        edit.replace(from, edit.state.caret, "");
                        edit.state.caret = from;
                    }
                }
                "delete" => {
                    edit.state.preferred_x = None;
                    if !edit.delete_selection() && edit.state.caret < length {
                        let to = if ctrl { word_right(edit.text, edit.state.caret) } else { edit.state.caret + 1 };
                        edit.replace(edit.state.caret, to, "");
                    }
                }
                "a" if ctrl => {
                    edit.state.anchor = Some(0);
                    edit.state.caret = length;
                }
                "c" | "x" if ctrl => {
                    if let Some((from, to)) = edit.selection() {
                        self.clipboard.set_text(char_slice(edit.text, from, to));
                        if key == "x" {
                            edit.delete_selection();
                        }
                    }
                }
                "v" if ctrl => {
                    let pasted = collapse_line_breaks(&self.clipboard.get_text());
                    if !pasted.is_empty() {
                        edit.insert(&pasted, options);
                    }
                }
                "return" | "kpenter" => response.submitted = true,
                "escape" => self.ui.blur(),
                "tab" if options.custom_tab => response.tab = Some(shift),
                "tab" if !options.no_tab => self.ui.focus_next_field(id, shift),
                _ => {}
            }
            edit.state.blink_since = self.ui.time();
        }
    }

    /// Border and floating label shared by both fields.
    #[allow(clippy::too_many_arguments)]
    fn field_frame(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        focused: bool,
        float_label: bool,
        options: &TextFieldOptions,
    ) {
        let palette = self.palette();
        let has_error = options.error.is_some();
        let mut border = if has_error {
            palette.error
        } else if focused {
            palette.primary
        } else {
            palette.outline
        };
        if options.disabled {
            border = with_alpha(palette.on_surface, 0.12);
        }
        if let Some(background) = options.background {
            self.fill_rect(x, y, width, height, 4.0, background);
        }
        self.outline_rect(x + 0.5, y + 0.5, width - 1.0, height - 1.0, 4.0, border, if focused { 2.0 } else { 1.0 });
        if let (true, Some(label)) = (float_label, options.label) {
            let label_font = theme::font(12.0, Weight::Regular);
            let label_width = self.fonts.width(label_font, label) + 8.0;
            let label_background = options.label_background.unwrap_or(palette.surface);
            self.fill_rect(x + 8.0, y - 2.0, label_width, 4.0, 0.0, label_background);
            let color = if has_error {
                palette.error
            } else if focused {
                palette.primary
            } else {
                palette.on_surface_variant
            };
            let label_y = y - self.fonts.height(label_font) / 2.0;
            self.text(label, x + 12.0, label_y, TextOptions::new().font(label_font).color(color));
        }
    }

    fn field_error(&mut self, x: f32, y: f32, width: f32, error: Option<&str>) {
        if let Some(error) = error.filter(|error| !error.is_empty()) {
            let color = self.palette().error;
            self.text(error, x + 12.0, y + 4.0, TextOptions::new().size(12.0).color(color).width(width - 24.0));
        }
    }

    /// `ui.textField(id, x, y, w, text, opts)`: a single-line outlined field with a floating
    /// label. Edits `text` in place.
    pub fn text_field(
        &mut self,
        id: &str,
        x: f32,
        y: f32,
        width: f32,
        text: &mut String,
        options: &TextFieldOptions,
    ) -> TextFieldResponse {
        let height = options.height.unwrap_or(48.0);
        let state = self.begin_field(id, text, options);
        let mut edit = Edit { text, state, changed: false };
        let font = theme::font(options.size.unwrap_or(16.0), Weight::Regular);
        let padding = 12.0;
        let prefix_width = options.prefix.map_or(0.0, |prefix| self.fonts.width(font, prefix));
        let inner_x = x + padding + prefix_width;
        let inner_width = width - padding * 2.0 - prefix_width - options.trailing_width;
        let mut response = TextFieldResponse::default();

        let region = if options.disabled {
            Default::default()
        } else {
            self.ui.region(id, x, y, width, height, RegionOptions::cursor("ibeam"))
        };
        let scroll_x = edit.state.scroll_x;
        self.field_mouse(id, &mut edit, region.pressed, region.held, options, &mut |frame, text| {
            let (mouse_x, _) = frame.ui.mouse();
            caret_in_text(frame, font, text, mouse_x - inner_x + scroll_x)
        });
        let focused = self.ui.focus.as_deref() == Some(id);
        if focused && !options.disabled {
            self.field_keys(id, &mut edit, options, None, &mut response);
        }
        response.changed = edit.changed;

        let float_label = options.label.is_some() && (focused || !edit.text.is_empty() || options.prefix.is_some());
        self.field_frame(x, y, width, height, focused, float_label, options);

        // Keep the caret visible.
        let display = if options.password { "•".repeat(char_count(edit.text)) } else { edit.text.clone() };
        let caret_x = self.fonts.width(font, char_slice(edit.text, 0, edit.state.caret));
        if caret_x - edit.state.scroll_x > inner_width - 2.0 {
            edit.state.scroll_x = caret_x - inner_width + 2.0;
        }
        if caret_x - edit.state.scroll_x < 0.0 {
            edit.state.scroll_x = caret_x;
        }
        if self.fonts.width(font, edit.text) < inner_width {
            edit.state.scroll_x = 0.0;
        }

        let palette = self.palette();
        let text_y = y + (height - self.fonts.height(font)) / 2.0;
        self.ui.push_clip(x + 2.0, y + 2.0, width - 4.0, height - 4.0);
        if let Some(prefix) = options.prefix {
            self.text(prefix, x + padding, text_y, TextOptions::new().font(font).color(palette.on_surface_variant));
        }
        self.ui.pop_clip();
        self.ui.push_clip(inner_x, y + 2.0, inner_width, height - 4.0);
        if edit.text.is_empty() && !float_label && (options.label.is_some() || options.hint.is_some()) {
            let placeholder = options.label.or(options.hint).unwrap_or_default();
            self.text(placeholder, inner_x, text_y, TextOptions::new().font(font).color(palette.on_surface_variant));
        } else if let (true, Some(hint), true) = (edit.text.is_empty(), options.hint, focused) {
            let color = with_alpha(palette.on_surface_variant, 0.7);
            self.text(hint, inner_x, text_y, TextOptions::new().font(font).color(color));
        }
        let origin_x = inner_x - edit.state.scroll_x;
        if let (true, Some((from, to))) = (focused, edit.selection()) {
            let start_x = origin_x + self.fonts.width(font, char_slice(&display, 0, from));
            let end_x = origin_x + self.fonts.width(font, char_slice(&display, 0, to));
            let line_height = self.fonts.height(font);
            self.fill_rect(start_x, text_y, end_x - start_x, line_height, 0.0, with_alpha(palette.primary, 0.3));
        }
        let text_color = if options.disabled { palette.disabled } else { palette.on_surface };
        self.text(&display, origin_x, text_y, TextOptions::new().font(font).color(text_color));
        if focused && (self.ui.time() - edit.state.blink_since).rem_euclid(1.0) < 0.55 {
            let caret_position = origin_x + self.fonts.width(font, char_slice(&display, 0, edit.state.caret));
            let line_height = self.fonts.height(font);
            self.fill_rect(caret_position.floor(), text_y, 2.0, line_height, 0.0, palette.primary);
        }
        self.ui.pop_clip();
        self.field_error(x, y + height, width, options.error);
        self.ui.field_states.insert(id.to_string(), edit.state);
        response
    }

    /// `ui.textAreaHeight(lines, opts)`: height for a number of visual lines.
    pub fn text_area_height(&mut self, lines: usize, size: Option<f32>) -> f32 {
        self.fonts.height(theme::font(size.unwrap_or(16.0), Weight::Regular)) * lines as f32 + 20.0
    }

    /// `ui.textArea(id, x, y, w, h, text, opts)`: a multi-line outlined field with word wrap.
    /// Wrapping is cosmetic: Enter still submits and pasted line breaks become spaces.
    #[allow(clippy::too_many_arguments)]
    pub fn text_area(
        &mut self,
        id: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        text: &mut String,
        options: &TextFieldOptions,
    ) -> TextFieldResponse {
        let state = self.begin_field(id, text, options);
        let mut edit = Edit { text, state, changed: false };
        let font = theme::font(options.size.unwrap_or(16.0), Weight::Regular);
        let line_height = self.fonts.height(font);
        let (padding_x, padding_y) = (12.0, 10.0);
        let (inner_x, inner_width) = (x + padding_x, width - padding_x * 2.0);
        let inner_y = y + padding_y;
        let mut response = TextFieldResponse::default();
        let layout = AreaLayout::new(self, font, edit.text, inner_width);

        let region = if options.disabled {
            Default::default()
        } else {
            self.ui.region(id, x, y, width, height, RegionOptions::cursor("ibeam"))
        };
        let scroll_y = edit.state.scroll_y;
        self.field_mouse(id, &mut edit, region.pressed, region.held, options, &mut |frame, _| {
            let (mouse_x, mouse_y) = frame.ui.mouse();
            let line =
                (((mouse_y - inner_y + scroll_y) / line_height).floor().max(0.0) as usize).min(layout.lines.len() - 1);
            layout.starts[line] + caret_in_text(frame, font, &layout.lines[line], mouse_x - inner_x)
        });
        let focused = self.ui.focus.as_deref() == Some(id);
        if focused && !options.disabled {
            self.field_keys(id, &mut edit, options, Some((&layout, font)), &mut response);
        }
        response.changed = edit.changed;
        let layout = AreaLayout::new(self, font, edit.text, inner_width);

        let float_label = options.label.is_some() && (focused || !edit.text.is_empty());
        let frame_options = TextFieldOptions { background: None, ..*options };
        self.field_frame(x, y, width, height, focused, float_label, &frame_options);

        let view_height = height - padding_y * 2.0;
        let content_height = layout.lines.len() as f32 * line_height;
        let max_scroll = (content_height - view_height).max(0.0);
        let wheel_y = self.ui.input.wheel_y;
        if region.hovered && wheel_y != 0.0 && max_scroll > 0.0 {
            edit.state.scroll_y = (edit.state.scroll_y - wheel_y * line_height).clamp(0.0, max_scroll);
            self.ui.input.wheel_y = 0.0;
        }
        // Keep the caret's line in view.
        let caret_line = layout.line_of(edit.state.caret);
        let caret_top = caret_line as f32 * line_height;
        if caret_top + line_height - edit.state.scroll_y > view_height {
            edit.state.scroll_y = caret_top + line_height - view_height;
        }
        if caret_top - edit.state.scroll_y < 0.0 {
            edit.state.scroll_y = caret_top;
        }
        edit.state.scroll_y = edit.state.scroll_y.clamp(0.0, max_scroll);

        let palette = self.palette();
        self.ui.push_clip(inner_x, y + 2.0, inner_width, height - 4.0);
        let selection = edit.selection();
        for (index, line) in layout.lines.iter().enumerate() {
            let line_y = inner_y - edit.state.scroll_y + index as f32 * line_height;
            if line_y + line_height < y || line_y > y + height {
                continue;
            }
            if edit.text.is_empty() && index == 0 && !float_label && (options.label.is_some() || options.hint.is_some())
            {
                let placeholder = options.label.or(options.hint).unwrap_or_default();
                self.text(
                    placeholder,
                    inner_x,
                    line_y,
                    TextOptions::new().font(font).color(palette.on_surface_variant),
                );
            } else if let (true, 0, Some(hint), true) = (edit.text.is_empty(), index, options.hint, focused) {
                let color = with_alpha(palette.on_surface_variant, 0.7);
                self.text(hint, inner_x, line_y, TextOptions::new().font(font).color(color));
            }
            if let (true, Some((from, to))) = (focused, selection) {
                let line_start = layout.starts[index];
                let line_end = line_start + char_count(line);
                let (selected_from, selected_to) = (from.max(line_start), to.min(line_end));
                if selected_from < selected_to {
                    let start_x = inner_x + self.fonts.width(font, char_slice(line, 0, selected_from - line_start));
                    let end_x = inner_x + self.fonts.width(font, char_slice(line, 0, selected_to - line_start));
                    let highlight = with_alpha(palette.primary, 0.3);
                    self.fill_rect(start_x, line_y, end_x - start_x, line_height, 0.0, highlight);
                }
            }
            let text_color = if options.disabled { palette.disabled } else { palette.on_surface };
            self.text(line, inner_x, line_y, TextOptions::new().font(font).color(text_color));
        }
        if focused && (self.ui.time() - edit.state.blink_since).rem_euclid(1.0) < 0.55 {
            let line = layout.line_of(edit.state.caret);
            let column = edit.state.caret - layout.starts[line];
            let caret_x = inner_x + self.fonts.width(font, char_slice(&layout.lines[line], 0, column));
            let caret_y = inner_y - edit.state.scroll_y + line as f32 * line_height;
            self.fill_rect(caret_x.floor(), caret_y, 2.0, line_height, 0.0, palette.primary);
        }
        self.ui.pop_clip();
        self.field_error(x, y + height, width, options.error);
        self.ui.field_states.insert(id.to_string(), edit.state);
        response
    }
}

/// Pasted text: each run of `\r` / `\n` becomes one space.
pub fn collapse_line_breaks(text: &str) -> String {
    let mut collapsed = String::with_capacity(text.len());
    let mut in_break = false;
    for character in text.chars() {
        if character == '\r' || character == '\n' {
            if !in_break {
                collapsed.push(' ');
            }
            in_break = true;
        } else {
            collapsed.push(character);
            in_break = false;
        }
    }
    collapsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_jumps_match_the_original() {
        let text = "hello, big_world  again";
        assert_eq!(word_left(text, 11), 7);
        assert_eq!(word_left(text, 7), 0);
        assert_eq!(word_right(text, 5), 16);
        assert_eq!(word_right(text, 16), 23);
        assert_eq!(word_left("", 0), 0);
        assert_eq!(word_right("héllo wörld", 0), 1, "non-ASCII letters are not word characters");
    }

    #[test]
    fn character_slices() {
        assert_eq!(char_slice("añb€c", 1, 4), "ñb€");
        assert_eq!(char_slice("abc", 2, 10), "c");
        assert_eq!(char_slice("abc", 3, 1), "");
    }

    #[test]
    fn pasted_line_breaks_become_single_spaces() {
        assert_eq!(collapse_line_breaks("a\r\nb\n\nc"), "a b c");
    }
}
