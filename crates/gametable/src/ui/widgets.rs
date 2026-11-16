//! The basic widgets of `src/ui/kit/ui.lua`: buttons, icon buttons, checkbox, switch, segmented
//! buttons, slider, spinner, progress bar and scroll views.

use std::f32::consts::PI;

use crate::app::Frame;
use crate::graphics::draw_list::Color;
use crate::graphics::tessellation::{ArcMode, DrawMode};
use crate::ui::context::{Region, RegionOptions};
use crate::ui::draw::TextOptions;
use crate::ui::theme::{self, Weight, with_alpha};

/// `ui.BUTTON_H`.
pub const BUTTON_HEIGHT: f32 = 40.0;

/// Button styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    #[default]
    Filled,
    Outlined,
    Text,
    Tonal,
    Danger,
}

/// Options for [`Frame::button`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ButtonOptions<'a> {
    pub variant: ButtonVariant,
    pub id: Option<&'a str>,
    pub icon: Option<&'a str>,
    pub disabled: bool,
    pub tooltip: Option<&'a str>,
    /// Foreground colour instead of the variant's.
    pub color: Option<Color>,
    /// Corner radius (default: fully rounded).
    pub radius: Option<f32>,
    /// Label size (default 14).
    pub size: Option<f32>,
}

impl<'a> ButtonOptions<'a> {
    pub fn variant(variant: ButtonVariant) -> ButtonOptions<'a> {
        ButtonOptions { variant, ..ButtonOptions::default() }
    }

    pub fn text() -> ButtonOptions<'a> {
        ButtonOptions::variant(ButtonVariant::Text)
    }

    pub fn id(self, id: &'a str) -> Self {
        ButtonOptions { id: Some(id), ..self }
    }

    pub fn icon(self, icon: &'a str) -> Self {
        ButtonOptions { icon: Some(icon), ..self }
    }

    pub fn disabled(self, disabled: bool) -> Self {
        ButtonOptions { disabled, ..self }
    }

    pub fn tooltip(self, tooltip: &'a str) -> Self {
        ButtonOptions { tooltip: Some(tooltip), ..self }
    }
}

/// Options for [`Frame::icon_button`].
#[derive(Debug, Clone, Copy, Default)]
pub struct IconButtonOptions<'a> {
    /// Diameter (default 40).
    pub size: Option<f32>,
    pub id: Option<&'a str>,
    pub disabled: bool,
    /// Icon colour (default on-surface-variant).
    pub color: Option<Color>,
    /// A filled circle behind the icon.
    pub background: Option<Color>,
    pub tooltip: Option<&'a str>,
    /// Icon size (default 60% of the button).
    pub icon_size: Option<f32>,
}

impl<'a> IconButtonOptions<'a> {
    pub fn tooltip(tooltip: &'a str) -> IconButtonOptions<'a> {
        IconButtonOptions { tooltip: Some(tooltip), ..IconButtonOptions::default() }
    }

    pub fn id(self, id: &'a str) -> Self {
        IconButtonOptions { id: Some(id), ..self }
    }

    pub fn size(self, size: f32, icon_size: f32) -> Self {
        IconButtonOptions { size: Some(size), icon_size: Some(icon_size), ..self }
    }

    pub fn color(self, color: Color) -> Self {
        IconButtonOptions { color: Some(color), ..self }
    }

    pub fn disabled(self, disabled: bool) -> Self {
        IconButtonOptions { disabled, ..self }
    }
}

/// Options for checkboxes and switches.
#[derive(Debug, Clone, Copy, Default)]
pub struct ToggleOptions<'a> {
    pub label: Option<&'a str>,
    pub disabled: bool,
    /// Checked colour instead of primary.
    pub color: Option<Color>,
    pub tooltip: Option<&'a str>,
    /// Second line under a switch row's label (the caller reserves the space).
    pub subtitle: Option<&'a str>,
}

impl<'a> ToggleOptions<'a> {
    pub fn label(label: &'a str) -> ToggleOptions<'a> {
        ToggleOptions { label: Some(label), ..ToggleOptions::default() }
    }

    pub fn disabled(self, disabled: bool) -> Self {
        ToggleOptions { disabled, ..self }
    }
}

/// One choice of a segmented button or dropdown.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice<T> {
    pub value: T,
    pub label: String,
}

impl<T> Choice<T> {
    pub fn new(value: T, label: impl Into<String>) -> Choice<T> {
        Choice { value, label: label.into() }
    }
}

/// Options for sliders.
#[derive(Debug, Clone, Copy, Default)]
pub struct SliderOptions {
    pub step: Option<f32>,
    pub disabled: bool,
}

/// Scrollbar colours (`ui.endScroll` opts).
#[derive(Debug, Clone, Copy)]
pub struct ScrollbarStyle {
    /// Default: the palette's on-surface.
    pub thumb_color: Option<Color>,
    pub thumb_alpha: f32,
    pub thumb_alpha_hover: f32,
}

impl Default for ScrollbarStyle {
    fn default() -> Self {
        ScrollbarStyle { thumb_color: None, thumb_alpha: 0.25, thumb_alpha_hover: 0.45 }
    }
}

/// The region a disabled widget reports: never hovered.
fn inert() -> Region {
    Region::default()
}

impl Frame<'_> {
    /// `ui.buttonWidth(label, opts)`: a button's natural width.
    pub fn button_width(&mut self, label: &str, options: &ButtonOptions) -> f32 {
        let mut width = self.text_width(label, options.size.unwrap_or(14.0), Weight::Medium) + 48.0;
        if options.icon.is_some() {
            width += 26.0;
        }
        width
    }

    /// `ui.button(label, x, y, w, h, opts)`: true when clicked.
    pub fn button(
        &mut self,
        label: &str,
        x: f32,
        y: f32,
        width: Option<f32>,
        height: Option<f32>,
        options: &ButtonOptions,
    ) -> bool {
        let height = height.unwrap_or(BUTTON_HEIGHT);
        let width = width.unwrap_or_else(|| self.button_width(label, options));
        let id = options.id.map_or_else(|| self.ui.default_id(label, x, y), str::to_string);
        let palette = self.palette();
        let disabled = options.disabled;
        let region = if disabled {
            inert()
        } else {
            self.ui.region(&id, x, y, width, height, RegionOptions::hand().with_tooltip(options.tooltip))
        };
        let (mut background, mut foreground, mut border) = match options.variant {
            ButtonVariant::Filled => (Some(palette.primary), palette.on_primary, None),
            ButtonVariant::Tonal => (Some(palette.secondary_container), palette.on_secondary_container, None),
            ButtonVariant::Danger => {
                let foreground = if self.theme.is_dark { Color::from_argb(0xFF69_0005) } else { palette.on_primary };
                (Some(palette.error), foreground, None)
            }
            ButtonVariant::Outlined => (None, palette.primary, Some(palette.outline)),
            ButtonVariant::Text => (None, palette.primary, None),
        };
        if let Some(color) = options.color {
            foreground = color;
        }
        if disabled {
            if background.is_some() {
                background = Some(with_alpha(palette.on_surface, 0.12));
            }
            foreground = palette.disabled;
            if border.is_some() {
                border = Some(with_alpha(palette.on_surface, 0.12));
            }
        }
        let radius = options.radius.unwrap_or(height / 2.0);
        if let Some(background) = background {
            self.fill_rect(x, y, width, height, radius, background);
        }
        if let Some(border) = border {
            self.outline_rect(x + 0.5, y + 0.5, width - 1.0, height - 1.0, radius, border, 1.0);
        }
        if region.hovered || region.held {
            self.fill_rect(x, y, width, height, radius, with_alpha(foreground, if region.held { 0.12 } else { 0.08 }));
        }
        let font = theme::font(options.size.unwrap_or(14.0), Weight::Medium);
        let label_width = self.fonts.width(font, label);
        let content_width = label_width + if options.icon.is_some() { 26.0 } else { 0.0 };
        let mut content_x = x + (width - content_width) / 2.0;
        if let Some(icon) = options.icon {
            self.icon(icon, content_x, y + (height - 18.0) / 2.0, 18.0, foreground);
            content_x += 26.0;
        }
        self.text(label, content_x, y, TextOptions::new().font(font).color(foreground).middle_of(height));
        region.clicked && !disabled
    }

    /// `ui.iconButton(icon, x, y, opts)`: a round icon button; true when clicked.
    pub fn icon_button(&mut self, icon: &str, x: f32, y: f32, options: &IconButtonOptions) -> bool {
        let size = options.size.unwrap_or(40.0);
        let id = options.id.map_or_else(|| self.ui.default_id(icon, x, y), str::to_string);
        let disabled = options.disabled;
        let region = if disabled {
            inert()
        } else {
            self.ui.region(&id, x, y, size, size, RegionOptions::hand().with_tooltip(options.tooltip))
        };
        let palette = self.palette();
        let foreground = if disabled { palette.disabled } else { options.color.unwrap_or(palette.on_surface_variant) };
        if let Some(background) = options.background {
            self.fill_rect(x, y, size, size, size / 2.0, background);
        }
        if region.hovered || region.held {
            let alpha = if region.held { 0.16 } else { 0.1 };
            let color = Color::rgba(foreground.red, foreground.green, foreground.blue, alpha);
            self.circle(DrawMode::Fill, x + size / 2.0, y + size / 2.0, size / 2.0, color);
        }
        let icon_size = options.icon_size.unwrap_or((size * 0.6).floor());
        self.icon_centered(icon, x, y, size, size, icon_size, foreground);
        region.clicked && !disabled
    }

    /// `ui.checkbox(id, x, y, value, opts)`: returns the (possibly toggled) value.
    pub fn checkbox(&mut self, id: &str, x: f32, y: f32, value: bool, options: &ToggleOptions) -> bool {
        let size = 18.0;
        let label_width = options.label.map_or(0.0, |label| 8.0 + self.text_width(label, 14.0, Weight::Regular));
        let width = size + label_width;
        let region = if options.disabled {
            inert()
        } else {
            let region_options = RegionOptions::hand().with_tooltip(options.tooltip);
            self.ui.region(id, x - 11.0, y - 11.0, width + 22.0, size + 22.0, region_options)
        };
        let palette = self.palette();
        let color = if options.disabled {
            palette.disabled
        } else if value {
            options.color.unwrap_or(palette.primary)
        } else {
            palette.on_surface_variant
        };
        if region.hovered {
            let halo = Color::rgba(color.red, color.green, color.blue, 0.1);
            self.circle(DrawMode::Fill, x + size / 2.0, y + size / 2.0, 20.0, halo);
        }
        if value {
            self.fill_rect(x, y, size, size, 2.0, color);
            let check = if options.disabled { palette.surface } else { palette.on_primary };
            let list = self.ui.list();
            list.set_color(check);
            list.set_line_width(2.0);
            list.line(&[x + 4.0, y + 9.0, x + 7.5, y + 12.5, x + 14.0, y + 5.5]);
            list.set_line_width(1.0);
        } else {
            self.outline_rect(x + 1.0, y + 1.0, size - 2.0, size - 2.0, 2.0, color, 2.0);
        }
        if let Some(label) = options.label {
            let label_color = if options.disabled { palette.disabled } else { palette.on_surface };
            self.text(label, x + size + 8.0, y, TextOptions::new().middle_of(size).color(label_color));
        }
        if region.clicked && !options.disabled { !value } else { value }
    }

    /// `ui.switch(id, x, y, value, opts)`: a 52×32 switch; returns the (possibly toggled) value.
    pub fn switch(&mut self, id: &str, x: f32, y: f32, value: bool, options: &ToggleOptions) -> bool {
        let (width, height) = (52.0, 32.0);
        let region = if options.disabled {
            inert()
        } else {
            self.ui.region(id, x, y, width, height, RegionOptions::hand().with_tooltip(options.tooltip))
        };
        let palette = self.palette();
        let alpha = if options.disabled { 0.38 } else { 1.0 };
        if value {
            self.fill_rect(x, y, width, height, height / 2.0, with_alpha(palette.primary, alpha));
            self.circle(
                DrawMode::Fill,
                x + width - height / 2.0,
                y + height / 2.0,
                12.0,
                with_alpha(palette.on_primary, alpha),
            );
        } else {
            self.fill_rect(x, y, width, height, height / 2.0, with_alpha(palette.surface_container_highest, alpha));
            let outline = with_alpha(palette.outline, alpha);
            self.outline_rect(x + 1.0, y + 1.0, width - 2.0, height - 2.0, height / 2.0 - 1.0, outline, 2.0);
            self.circle(DrawMode::Fill, x + height / 2.0, y + height / 2.0, 8.0, outline);
        }
        if region.hovered {
            let knob_x = if value { x + width - height / 2.0 } else { x + height / 2.0 };
            self.circle(DrawMode::Fill, knob_x, y + height / 2.0, 20.0, with_alpha(palette.on_surface, 0.06));
        }
        if region.clicked && !options.disabled { !value } else { value }
    }

    /// `ui.switchRow`: label on the left, switch on the right of `width`.
    #[allow(clippy::too_many_arguments)]
    pub fn switch_row(
        &mut self,
        id: &str,
        label: &str,
        x: f32,
        y: f32,
        width: f32,
        value: bool,
        options: &ToggleOptions,
    ) -> bool {
        let palette = self.palette();
        let color = if options.disabled { palette.disabled } else { palette.on_surface };
        self.text(label, x, y, TextOptions::new().width(width - 64.0).middle_of(32.0).color(color));
        if let Some(subtitle) = options.subtitle {
            let subtitle_options = TextOptions::new().width(width - 64.0).size(12.0).color(palette.on_surface_variant);
            self.text(subtitle, x, y + 30.0, subtitle_options);
        }
        self.switch(id, x + width - 52.0, y, value, options)
    }

    /// `ui.segmented(id, x, y, w, options, selected, opts)`: returns the selected value.
    #[allow(clippy::too_many_arguments)]
    pub fn segmented<T: PartialEq + Clone>(
        &mut self,
        id: &str,
        x: f32,
        y: f32,
        width: f32,
        choices: &[Choice<T>],
        selected: T,
        height: Option<f32>,
        disabled: bool,
    ) -> T {
        let height = height.unwrap_or(40.0);
        let segment_width = width / choices.len().max(1) as f32;
        let palette = self.palette();
        let mut selected = selected;
        let mut picked = None;
        for (index, choice) in choices.iter().enumerate() {
            let segment_x = x + index as f32 * segment_width;
            let region = if disabled {
                inert()
            } else {
                let segment_id = format!("{id}:{}", index + 1);
                self.ui.region(&segment_id, segment_x, y, segment_width, height, RegionOptions::hand())
            };
            let is_selected = choice.value == selected;
            if is_selected {
                let fill = if disabled { with_alpha(palette.on_surface, 0.12) } else { palette.secondary_container };
                self.fill_rect(segment_x, y, segment_width, height, 0.0, fill);
            }
            if region.hovered {
                self.fill_rect(segment_x, y, segment_width, height, 0.0, with_alpha(palette.on_surface, 0.06));
            }
            let label_width =
                self.text_width(&choice.label, 14.0, Weight::Medium) + if is_selected { 26.0 } else { 0.0 };
            let mut label_x = segment_x + (segment_width - label_width) / 2.0;
            let foreground = if disabled {
                palette.disabled
            } else if is_selected {
                palette.on_secondary_container
            } else {
                palette.on_surface
            };
            if is_selected {
                self.icon("check", label_x, y + (height - 18.0) / 2.0, 18.0, foreground);
                label_x += 26.0;
            }
            self.text(&choice.label, label_x, y, TextOptions::new().medium().middle_of(height).color(foreground));
            if region.clicked {
                picked = Some(choice.value.clone());
            }
        }
        if let Some(value) = picked {
            selected = value;
        }
        let border = if disabled { with_alpha(palette.on_surface, 0.12) } else { palette.outline };
        self.outline_rect(x + 0.5, y + 0.5, width - 1.0, height - 1.0, height / 2.0, border, 1.0);
        for index in 1..choices.len() {
            self.fill_rect((x + index as f32 * segment_width).floor(), y, 1.0, height, 0.0, border);
        }
        selected
    }

    /// `ui.slider(id, x, y, w, value, min, max, opts)`: a horizontal slider with a 40-unit hit
    /// area. Returns the new value and whether the thumb is being dragged.
    #[allow(clippy::too_many_arguments)]
    pub fn slider(
        &mut self,
        id: &str,
        x: f32,
        y: f32,
        width: f32,
        value: f32,
        min: f32,
        max: f32,
        options: &SliderOptions,
    ) -> (f32, bool) {
        let height = 40.0;
        let palette = self.palette();
        let region = if options.disabled {
            inert()
        } else {
            self.ui.region(id, x - 10.0, y, width + 20.0, height, RegionOptions::hand())
        };
        let held = region.held;
        let mut value = value;
        if held {
            let (mouse_x, _) = self.ui.mouse();
            let t = ((mouse_x - x) / width).clamp(0.0, 1.0);
            let mut new_value = min + t * (max - min);
            if let Some(step) = options.step {
                new_value = ((new_value - min) / step + 0.5).floor() * step + min;
            }
            value = new_value.max(min).min(max);
        }
        let t = if max > min { (value - min) / (max - min) } else { 0.0 };
        let center_y = y + height / 2.0;
        let thumb_x = x + t * width;
        let active = if options.disabled { palette.disabled } else { palette.primary };
        let track = if options.disabled { with_alpha(palette.on_surface, 0.12) } else { palette.secondary_container };
        self.fill_rect(x, center_y - 2.0, width, 4.0, 2.0, track);
        self.fill_rect(x, center_y - 2.0, thumb_x - x, 4.0, 2.0, active);
        if region.hovered || held {
            self.circle(DrawMode::Fill, thumb_x, center_y, 18.0, with_alpha(palette.primary, 0.12));
        }
        self.circle(DrawMode::Fill, thumb_x, center_y, if held { 10.0 } else { 8.0 }, active);
        (value, held)
    }

    /// `ui.sliderRow`: label and formatted value on one line, the slider below (64 tall).
    #[allow(clippy::too_many_arguments)]
    pub fn slider_row(
        &mut self,
        id: &str,
        label: &str,
        x: f32,
        y: f32,
        width: f32,
        value: f32,
        min: f32,
        max: f32,
        options: &SliderOptions,
        format: Option<&dyn Fn(f32) -> String>,
    ) -> (f32, bool) {
        let palette = self.palette();
        let color = if options.disabled { palette.disabled } else { palette.on_surface };
        self.text(label, x, y, TextOptions::new().width(width).color(color));
        if let Some(format) = format {
            let formatted = format(value);
            let value_options = TextOptions::new().width(width).right().size(12.0).color(palette.on_surface_variant);
            self.text(&formatted, x, y, value_options);
        }
        self.slider(id, x + 10.0, y + 20.0, width - 20.0, value, min, max, options)
    }

    /// `ui.spinner(cx, cy, radius, color)`: an indeterminate circular progress indicator.
    pub fn spinner(&mut self, center_x: f32, center_y: f32, radius: f32, color: Option<Color>) {
        let color = color.unwrap_or(self.palette().primary);
        let time = self.ui.time() as f32;
        let start = time * 4.0;
        let end = start + PI * (1.1 + 0.4 * (time * 2.0).sin());
        let list = self.ui.list();
        list.set_color(color);
        list.set_line_width((radius / 6.0).max(2.0));
        list.arc(DrawMode::Line, ArcMode::Open, center_x, center_y, radius, start, end, Some(24));
        list.set_line_width(1.0);
    }

    /// `ui.progressBar(x, y, w, h, value, color)`; `None` is indeterminate.
    pub fn progress_bar(&mut self, x: f32, y: f32, width: f32, height: f32, value: Option<f32>, color: Option<Color>) {
        let color = color.unwrap_or(self.palette().primary);
        self.fill_rect(x, y, width, height, height / 2.0, with_alpha(color, 0.24));
        match value {
            Some(value) => self.fill_rect(x, y, width * value.clamp(0.0, 1.0), height, height / 2.0, color),
            None => {
                let t = (self.ui.time() as f32 * 0.8).rem_euclid(1.4) - 0.2;
                let start = x + width * t.max(0.0);
                let end = x + width * (t + 0.4).min(1.0);
                if end > start {
                    self.fill_rect(start, y, end - start, height, height / 2.0, color);
                }
            }
        }
    }

    /// `ui.beginScroll(id, x, y, w, h)`: starts a vertical scroll view and returns the scroll
    /// offset (draw content at `y - offset`). Pair with [`Frame::end_scroll`].
    pub fn begin_scroll(&mut self, id: &str, x: f32, y: f32, width: f32, height: f32) -> f32 {
        let hovered = self.ui.hover(x, y, width, height);
        let state = self.ui.scroll_state(id);
        state.view = crate::ui::context::Rect::new(x, y, width, height);
        state.hovered = hovered;
        let max_y = (state.content_height - height).max(0.0);
        state.target = state.target.min(max_y);
        state.y = state.y.min(max_y);
        let offset = state.y.floor();
        self.ui.push_clip(x, y, width, height);
        offset
    }

    /// `ui.endScroll(id, contentHeight, opts)`: applies wheel scrolling (60 per step, eased) and
    /// draws the draggable scrollbar thumb.
    pub fn end_scroll(&mut self, id: &str, content_height: f32, style: &ScrollbarStyle) {
        self.ui.pop_clip();
        let wheel_y = self.ui.input.wheel_y;
        let state = self.ui.scroll_state(id);
        state.content_height = content_height;
        let view = state.view;
        let max_y = (content_height - view.height).max(0.0);
        let mut consumed_wheel = false;
        if state.hovered && wheel_y != 0.0 && max_y > 0.0 {
            state.target = (state.target - wheel_y * 60.0).clamp(0.0, max_y);
            consumed_wheel = true;
        }
        state.y += (state.target - state.y) * 0.45;
        if (state.target - state.y).abs() < 0.5 {
            state.y = state.target;
        }
        let scroll_y = state.y;
        if consumed_wheel {
            self.ui.input.wheel_y = 0.0;
        }
        if max_y <= 0.0 {
            return;
        }
        let track_height = view.height;
        let thumb_height = (track_height * view.height / content_height).max(24.0);
        let thumb_y = view.y + (track_height - thumb_height) * (scroll_y / max_y);
        let bar_x = view.x + view.width - 8.0;
        let thumb_id = format!("{id}:thumb");
        let region = self.ui.region(&thumb_id, bar_x - 2.0, view.y, 10.0, track_height, RegionOptions::default());
        let (_, mouse_y) = self.ui.mouse();
        let state = self.ui.scroll_state(id);
        if region.pressed {
            state.drag_offset = Some(mouse_y - thumb_y);
        }
        if region.held
            && let Some(drag_offset) = state.drag_offset
        {
            let new_y = mouse_y - drag_offset;
            state.target = ((new_y - view.y) / (track_height - thumb_height).max(1.0) * max_y).clamp(0.0, max_y);
            state.y = state.target;
        }
        let hot = region.hovered || region.held;
        let alpha = if hot { style.thumb_alpha_hover } else { style.thumb_alpha };
        let color = with_alpha(style.thumb_color.unwrap_or(self.palette().on_surface), alpha);
        self.fill_rect(bar_x, thumb_y, 6.0, thumb_height, 3.0, color);
    }
}
