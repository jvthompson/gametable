//! Drawing helpers of `src/ui/kit/ui.lua` (text, icons, rectangles, shadows, the tooltip), as
//! methods on [`Frame`] so widgets and screens call `frame.text(...)`, `frame.rect(...)`.

use crate::app::Frame;
use crate::graphics::draw_list::Color;
use crate::graphics::tessellation::DrawMode;
use crate::graphics::text::FontKey;
use crate::graphics::text_layout::Align;
use crate::ui::theme::{self, Palette, Weight};

/// Options for [`Frame::text`] (the original's `opts` table).
#[derive(Debug, Clone, Copy)]
pub struct TextOptions {
    /// A font to use instead of `size` + `weight`.
    pub font: Option<FontKey>,
    pub size: f32,
    pub weight: Weight,
    /// Defaults to the palette's `on_surface`.
    pub color: Option<Color>,
    /// With a width the text wraps (printf) and can be aligned, ellipsized or clamped.
    pub width: Option<f32>,
    /// With a height and `middle`, the text is centred vertically in it.
    pub height: Option<f32>,
    pub align: Align,
    pub middle: bool,
    pub max_lines: Option<usize>,
    pub ellipsis: bool,
}

impl Default for TextOptions {
    fn default() -> Self {
        TextOptions {
            font: None,
            size: 14.0,
            weight: Weight::Regular,
            color: None,
            width: None,
            height: None,
            align: Align::Left,
            middle: false,
            max_lines: None,
            ellipsis: false,
        }
    }
}

impl TextOptions {
    pub fn new() -> TextOptions {
        TextOptions::default()
    }

    pub fn font(self, font: FontKey) -> Self {
        TextOptions { font: Some(font), ..self }
    }

    pub fn size(self, size: f32) -> Self {
        TextOptions { size, ..self }
    }

    pub fn weight(self, weight: Weight) -> Self {
        TextOptions { weight, ..self }
    }

    pub fn medium(self) -> Self {
        self.weight(Weight::Medium)
    }

    pub fn bold(self) -> Self {
        self.weight(Weight::Bold)
    }

    pub fn color(self, color: Color) -> Self {
        TextOptions { color: Some(color), ..self }
    }

    pub fn width(self, width: f32) -> Self {
        TextOptions { width: Some(width), ..self }
    }

    pub fn height(self, height: f32) -> Self {
        TextOptions { height: Some(height), ..self }
    }

    /// Centred vertically in a box of this height (`h = ..., valign = "middle"`).
    pub fn middle_of(self, height: f32) -> Self {
        TextOptions { height: Some(height), middle: true, ..self }
    }

    pub fn align(self, align: Align) -> Self {
        TextOptions { align, ..self }
    }

    pub fn centered(self) -> Self {
        self.align(Align::Center)
    }

    pub fn right(self) -> Self {
        self.align(Align::Right)
    }

    pub fn max_lines(self, lines: usize) -> Self {
        TextOptions { max_lines: Some(lines), ..self }
    }

    pub fn ellipsis(self) -> Self {
        TextOptions { ellipsis: true, ..self }
    }
}

impl Frame<'_> {
    /// `theme.c`: the palette in use this frame.
    pub fn palette(&self) -> &'static Palette {
        self.theme.palette()
    }

    /// `ui.snap(v)`.
    pub fn snap(&self, value: f32) -> f32 {
        self.ui.snap(value)
    }

    /// Draws `draw` in a new layer above everything drawn so far (`ui.overlay(fn)`).
    pub fn overlay<R>(&mut self, draw: impl FnOnce(&mut Frame) -> R) -> R {
        self.ui.begin_overlay();
        let result = draw(self);
        self.ui.end_overlay();
        result
    }

    pub fn set_color(&mut self, color: Color) {
        self.ui.list().set_color(color);
    }

    /// `ui.rect(mode, x, y, w, h, r, color, lineWidth)`.
    #[allow(clippy::too_many_arguments)]
    pub fn rect(&mut self, mode: DrawMode, x: f32, y: f32, width: f32, height: f32, radius: f32, color: Color) {
        let list = self.ui.list();
        list.set_color(color);
        list.rectangle(mode, x, y, width, height, radius, radius);
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, width: f32, height: f32, radius: f32, color: Color) {
        self.rect(DrawMode::Fill, x, y, width, height, radius, color);
    }

    /// An outline of the given line width (the line width goes back to 1 afterwards).
    #[allow(clippy::too_many_arguments)]
    pub fn outline_rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        radius: f32,
        color: Color,
        line_width: f32,
    ) {
        let list = self.ui.list();
        list.set_line_width(line_width);
        list.set_color(color);
        list.rectangle(DrawMode::Line, x, y, width, height, radius, radius);
        list.set_line_width(1.0);
    }

    pub fn circle(&mut self, mode: DrawMode, x: f32, y: f32, radius: f32, color: Color) {
        let list = self.ui.list();
        list.set_color(color);
        list.circle(mode, x, y, radius, None);
    }

    /// `ui.text(str, x, y, opts)`; returns the font used.
    pub fn text(&mut self, text: &str, x: f32, y: f32, options: TextOptions) -> FontKey {
        let font = options.font.unwrap_or_else(|| theme::font(options.size, options.weight));
        let color = options.color.unwrap_or(self.palette().on_surface);
        self.ui.list().set_color(color);
        if let Some(width) = options.width {
            let shown = if let Some(max_lines) = options.max_lines {
                self.clamp_lines(text, font, width, max_lines)
            } else if options.ellipsis {
                self.ellipsize(text, font, width)
            } else {
                text.to_string()
            };
            let mut text_y = y;
            if let (Some(height), true) = (options.height, options.middle) {
                let (_, lines) = self.fonts.wrap(font, &shown, width);
                text_y = y + (height - lines.len() as f32 * self.fonts.height(font)) / 2.0;
            }
            let (snapped_x, snapped_y) = (self.snap(x), self.snap(text_y));
            self.fonts.printf(self.ui.list(), font, &shown, snapped_x, snapped_y, width, options.align);
        } else {
            let mut text_y = y;
            if let (Some(height), true) = (options.height, options.middle) {
                text_y = y + (height - self.fonts.height(font)) / 2.0;
            }
            let (snapped_x, snapped_y) = (self.snap(x), self.snap(text_y));
            self.fonts.print(self.ui.list(), font, text, snapped_x, snapped_y);
        }
        font
    }

    /// `ui.textWidth(str, size, weight)`.
    pub fn text_width(&mut self, text: &str, size: f32, weight: Weight) -> f32 {
        self.fonts.width(theme::font(size, weight), text)
    }

    /// `ui.textHeight(str, w, size, weight)`: height of the text wrapped to `width`.
    pub fn text_height(&mut self, text: &str, width: f32, size: f32, weight: Weight) -> f32 {
        let font = theme::font(size, weight);
        let (_, lines) = self.fonts.wrap(font, text, width);
        lines.len().max(1) as f32 * self.fonts.height(font)
    }

    /// `ui.clampLines`: wraps to `max_width` and keeps at most `max_lines` lines, ellipsizing the
    /// last.
    pub fn clamp_lines(&mut self, text: &str, font: FontKey, max_width: f32, max_lines: usize) -> String {
        let (_, lines) = self.fonts.wrap(font, text, max_width);
        if lines.len() <= max_lines || max_lines == 0 {
            return text.to_string();
        }
        let mut kept: Vec<String> = lines[..max_lines - 1].iter().map(|line| line.text.clone()).collect();
        let rest: Vec<&str> = lines[max_lines - 1..].iter().map(|line| line.text.as_str()).collect();
        kept.push(self.ellipsize(&rest.join(" "), font, max_width));
        kept.join("\n")
    }

    /// `ui.ellipsize`: the longest prefix that fits with "…" after it.
    pub fn ellipsize(&mut self, text: &str, font: FontKey, max_width: f32) -> String {
        if self.fonts.width(font, text) <= max_width {
            return text.to_string();
        }
        let characters: Vec<char> = text.chars().collect();
        let prefix = |count: usize| characters[..count].iter().collect::<String>();
        let (mut low, mut high) = (0usize, characters.len());
        while low < high {
            let middle = (low + high).div_ceil(2);
            if self.fonts.width(font, &(prefix(middle) + "…")) <= max_width {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        prefix(low) + "…"
    }

    /// `ui.icon(name, x, y, size, color)`: an icon glyph whose box is `size` tall at `y`.
    pub fn icon(&mut self, name: &str, x: f32, y: f32, size: f32, color: Color) {
        let font = theme::icon_font(size);
        let glyph = theme::glyph(name);
        let glyph_y = y + (size - self.fonts.height(font)) / 2.0;
        let (snapped_x, snapped_y) = (self.snap(x), self.snap(glyph_y));
        self.ui.list().set_color(color);
        self.fonts.print(self.ui.list(), font, &glyph, snapped_x, snapped_y);
    }

    /// `ui.iconCentered`: an icon centred in a box.
    #[allow(clippy::too_many_arguments)]
    pub fn icon_centered(&mut self, name: &str, x: f32, y: f32, width: f32, height: f32, size: f32, color: Color) {
        let font = theme::icon_font(size);
        let glyph = theme::glyph(name);
        let glyph_x = x + (width - self.fonts.width(font, &glyph)) / 2.0;
        let glyph_y = y + (height - self.fonts.height(font)) / 2.0;
        let (snapped_x, snapped_y) = (self.snap(glyph_x), self.snap(glyph_y));
        self.ui.list().set_color(color);
        self.fonts.print(self.ui.list(), font, &glyph, snapped_x, snapped_y);
    }

    /// `ui.shadow`: three soft rounded rectangles under a panel.
    pub fn shadow(&mut self, x: f32, y: f32, width: f32, height: f32, radius: f32, strength: f32) {
        let list = self.ui.list();
        for step in (1..=3).rev() {
            let step = step as f32;
            list.set_color(Color::rgba(0.0, 0.0, 0.0, 0.06 * strength));
            list.rectangle(
                DrawMode::Fill,
                x - step + 1.0,
                y - step + 2.0,
                width + 2.0 * step - 2.0,
                height + 2.0 * step - 1.0,
                radius + step,
                radius + step,
            );
        }
    }

    /// `ui.divider`: a 1-unit horizontal line (outline variant by default).
    pub fn divider(&mut self, x: f32, y: f32, width: f32, color: Option<Color>) {
        let color = color.unwrap_or(self.palette().outline_variant);
        self.fill_rect(x, y, width, 1.0, 0.0, color);
    }

    /// The tooltip above everything (`drawTooltip`, run at the end of the frame).
    pub(crate) fn draw_tooltip(&mut self) {
        if !self.ui.tooltip_seen || self.ui.input.is_down(1) {
            self.ui.tooltip = None;
        }
        self.ui.tooltip_seen = false;
        let Some(tooltip) = self.ui.tooltip.clone() else { return };
        if self.ui.time() - tooltip.since <= 0.5 {
            return;
        }
        self.ui.begin_top_layer();
        let font = theme::font(12.0, Weight::Regular);
        // Long help text wraps instead of running across the screen.
        let text_width = self.fonts.width(font, &tooltip.text).min(360.0);
        let (_, lines) = self.fonts.wrap(font, &tooltip.text, text_width);
        let width = text_width + 16.0;
        let height = lines.len() as f32 * self.fonts.height(font) + 8.0;
        let (screen_width, screen_height) = self.ui.screen_size();
        let (mouse_x, mouse_y) = self.ui.mouse();
        let x = (mouse_x + 12.0).min(screen_width - width - 4.0);
        let mut y = mouse_y + 20.0;
        if y + height > screen_height {
            y = mouse_y - height - 8.0;
        }
        self.ui.set_scissor(None);
        self.fill_rect(x, y, width, height, 4.0, Color::rgba(0.38, 0.38, 0.38, 0.95));
        self.ui.list().set_color(Color::WHITE);
        self.fonts.printf(self.ui.list(), font, &tooltip.text, x + 8.0, y + 4.0, text_width, Align::Left);
    }
}
