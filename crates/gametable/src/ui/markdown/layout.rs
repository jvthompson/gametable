//! Lays parsed Markdown out into positioned pieces (text runs in their fonts, fills, lines,
//! images) once per width, and draws the ones in view. Text flows word by word, so styles and
//! links can change mid-line; links remember their target for clicks, and headings and anchors
//! remember where they are for jumps.

use super::inline::{Inline, InlineItem, SpanStyle, has_visible_text, plain_text};
use super::parse::{ListMarker, MarkdownBlock};
use super::{compact_slug, github_slug};
use crate::app::Frame;
use crate::graphics::text::FontKey;
use crate::ui::theme::{self, Weight};

/// Which palette colour a piece of text is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRole {
    /// Paragraphs, lists and table cells (`on_surface_variant`).
    Body,
    /// Headings and table headers (`on_surface`).
    Strong,
    /// Level-2 headings (`primary`).
    Accent,
    /// Link text (`primary`, underlined).
    Link,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillRole {
    TableHeader,
    CodeBlock,
    QuoteBar,
}

/// One positioned piece of a laid-out document; coordinates are from the document's top left.
#[derive(Debug, Clone, PartialEq)]
pub enum LaidOut {
    Text {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        text: String,
        font: FontKey,
        role: TextRole,
        style: SpanStyle,
        /// Index into [`MarkdownLayout::links`].
        link: Option<usize>,
    },
    Fill {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        role: FillRole,
    },
    /// A horizontal line: a rule (`strong`) or a table divider.
    Line {
        x: f32,
        y: f32,
        width: f32,
        strong: bool,
    },
    Image {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        path: String,
    },
    /// An image that isn't loaded (yet, or at all): its description in a grey box.
    Placeholder {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        label: String,
    },
}

impl LaidOut {
    fn top_and_bottom(&self) -> (f32, f32) {
        match self {
            LaidOut::Text { y, height, .. }
            | LaidOut::Fill { y, height, .. }
            | LaidOut::Image { y, height, .. }
            | LaidOut::Placeholder { y, height, .. } => (*y, y + height),
            LaidOut::Line { y, .. } => (*y, y + 1.0),
        }
    }
}

/// A heading's level, text and offset from the top of the document.
pub type HeadingEntry = (u8, String, f32);

/// A document laid out at one width.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MarkdownLayout {
    pub items: Vec<LaidOut>,
    pub height: f32,
    pub headings: Vec<HeadingEntry>,
    /// Every place a link can jump to, in document order (the first of a name wins).
    pub anchors: Vec<(String, f32)>,
    pub links: Vec<String>,
}

/// What a layout was made for; a change means laying out again.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutKey {
    pub width: f32,
    pub font_family: crate::graphics::text::FontFamily,
    /// The body font's line height, which follows the Text size setting and display scale.
    pub line_height: f32,
    pub show_title_heading: bool,
    /// How many of the document's images have loaded (their sizes change the layout).
    pub loaded_images: usize,
}

/// The font and colour inline text starts from in a block.
#[derive(Debug, Clone, Copy)]
struct BaseStyle {
    size: f32,
    bold: bool,
    role: TextRole,
}

const BODY: BaseStyle = BaseStyle { size: 13.0, bold: false, role: TextRole::Body };
const CELL_PADDING: f32 = 8.0;
const LIST_INDENT: f32 = 18.0;
const LIST_MARKER_WIDTH: f32 = 24.0;
const MINIMUM_COLUMN_WIDTH: f32 = 48.0;

fn font_for(style: SpanStyle, base: BaseStyle) -> FontKey {
    let size = if style.small { (base.size * 0.78).round() } else { base.size };
    let weight = if style.bold || base.bold { Weight::Bold } else { Weight::Regular };
    if style.italic { theme::italic_font(size, weight) } else { theme::font(size, weight) }
}

/// Spaces and line ends separate words; a no-break space doesn't.
fn is_word_gap(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\r')
}

/// Splits text into words and the gaps between them.
fn words_and_gaps(text: &str) -> Vec<(bool, &str)> {
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut in_gap = None;
    for (index, character) in text.char_indices() {
        let gap = is_word_gap(character);
        if in_gap.is_some_and(|was_gap| was_gap != gap) {
            pieces.push((in_gap.unwrap_or(false), &text[start..index]));
            start = index;
        }
        in_gap = Some(gap);
    }
    if let Some(gap) = in_gap {
        pieces.push((gap, &text[start..]));
    }
    pieces
}

/// A run on the line being built (x from the line's start).
struct PendingRun {
    x: f32,
    width: f32,
    height: f32,
    text: String,
    font: FontKey,
    role: TextRole,
    style: SpanStyle,
    link: Option<usize>,
}

impl Frame<'_> {
    fn link_index(layout: &mut MarkdownLayout, target: &str) -> usize {
        match layout.links.iter().position(|link| link == target) {
            Some(index) => index,
            None => {
                layout.links.push(target.to_string());
                layout.links.len() - 1
            }
        }
    }

    /// Flows inline content into lines at `width`, adding its runs to `layout` at `(x, y)`.
    /// Returns the height used (0 for content with nothing visible).
    #[allow(clippy::too_many_arguments)]
    fn lay_out_inline(
        &mut self,
        inline: &Inline,
        base: BaseStyle,
        x: f32,
        y: f32,
        width: f32,
        centered: bool,
        layout: &mut MarkdownLayout,
    ) -> f32 {
        let base_height = self.fonts.height(font_for(SpanStyle::default(), base));
        let mut line: Vec<PendingRun> = Vec::new();
        let mut line_width = 0.0;
        let mut line_height: f32 = 0.0;
        let mut line_top = y;
        let mut pending_gap: Option<FontKey> = None;
        let mut any_line = false;

        let finish_line = |line: &mut Vec<PendingRun>,
                           line_width: &mut f32,
                           line_height: &mut f32,
                           line_top: &mut f32,
                           layout: &mut MarkdownLayout| {
            let height = if *line_height > 0.0 { *line_height } else { base_height };
            let offset = if centered { ((width - *line_width) / 2.0).max(0.0) } else { 0.0 };
            for run in line.drain(..) {
                layout.items.push(LaidOut::Text {
                    x: x + offset + run.x,
                    y: *line_top + (height - run.height),
                    width: run.width,
                    height: run.height,
                    text: run.text,
                    font: run.font,
                    role: run.role,
                    style: run.style,
                    link: run.link,
                });
            }
            *line_top += height;
            *line_width = 0.0;
            *line_height = 0.0;
        };

        for item in inline {
            match item {
                InlineItem::Anchor(name) => layout.anchors.push((name.clone(), line_top)),
                InlineItem::LineBreak => {
                    finish_line(&mut line, &mut line_width, &mut line_height, &mut line_top, layout);
                    pending_gap = None;
                    any_line = true;
                }
                InlineItem::Text { text, style, link } => {
                    let font = font_for(*style, base);
                    let role = if link.is_some() { TextRole::Link } else { base.role };
                    let link = link.as_deref().map(|target| Frame::link_index(layout, target));
                    let height = self.fonts.height(font);
                    for (is_gap, piece) in words_and_gaps(text) {
                        if is_gap {
                            pending_gap = Some(font);
                            continue;
                        }
                        let mut word = piece.to_string();
                        loop {
                            let word_width = self.fonts.width(font, &word);
                            let gap_width = match pending_gap {
                                Some(gap_font) if line_width > 0.0 => self.fonts.width(gap_font, " "),
                                _ => 0.0,
                            };
                            if line_width > 0.0 && line_width + gap_width + word_width > width {
                                finish_line(&mut line, &mut line_width, &mut line_height, &mut line_top, layout);
                                any_line = true;
                                continue;
                            }
                            // A word wider than the whole line is broken where it fills the line.
                            let (fits, rest) = if word_width > width && word.chars().count() > 1 {
                                let mut taken = String::new();
                                for character in word.chars() {
                                    let mut candidate = taken.clone();
                                    candidate.push(character);
                                    if !taken.is_empty() && self.fonts.width(font, &candidate) > width {
                                        break;
                                    }
                                    taken = candidate;
                                }
                                let rest = word[taken.len()..].to_string();
                                (taken, rest)
                            } else {
                                (word.clone(), String::new())
                            };
                            let fits_width = if rest.is_empty() { word_width } else { self.fonts.width(font, &fits) };
                            let joins_last = line.last().is_some_and(|last| {
                                last.font == font && last.role == role && last.style == *style && last.link == link
                            });
                            if joins_last && let Some(last) = line.last_mut() {
                                if gap_width > 0.0 {
                                    last.text.push(' ');
                                }
                                last.text.push_str(&fits);
                                last.width += gap_width + fits_width;
                            } else {
                                line.push(PendingRun {
                                    x: line_width + gap_width,
                                    width: fits_width,
                                    height,
                                    text: fits,
                                    font,
                                    role,
                                    style: *style,
                                    link,
                                });
                            }
                            line_width += gap_width + fits_width;
                            line_height = line_height.max(height);
                            pending_gap = None;
                            if rest.is_empty() {
                                break;
                            }
                            finish_line(&mut line, &mut line_width, &mut line_height, &mut line_top, layout);
                            any_line = true;
                            word = rest;
                        }
                    }
                }
            }
        }
        if !line.is_empty() {
            finish_line(&mut line, &mut line_width, &mut line_height, &mut line_top, layout);
            any_line = true;
        }
        if any_line { line_top - y } else { 0.0 }
    }

    /// The widest a cell's content gets on one line, and its longest word.
    fn natural_widths(&mut self, inline: &Inline, base: BaseStyle) -> (f32, f32) {
        let (mut widest_line, mut longest_word, mut current_line) = (0.0f32, 0.0f32, 0.0f32);
        for item in inline {
            match item {
                InlineItem::Text { text, style, .. } => {
                    let font = font_for(*style, base);
                    current_line += self.fonts.width(font, text);
                    for (is_gap, piece) in words_and_gaps(text) {
                        if !is_gap {
                            longest_word = longest_word.max(self.fonts.width(font, piece));
                        }
                    }
                }
                InlineItem::LineBreak => {
                    widest_line = widest_line.max(current_line);
                    current_line = 0.0;
                }
                InlineItem::Anchor(_) => {}
            }
        }
        (widest_line.max(current_line), longest_word)
    }

    /// Column widths that share `width` by content: every column gets at least its longest word
    /// (or a minimum), and the rest goes where the text is longest.
    fn table_column_widths(&mut self, header: &[Inline], rows: &[Vec<Inline>], width: f32) -> Vec<f32> {
        let column_count = std::iter::once(header.len()).chain(rows.iter().map(Vec::len)).max().unwrap_or(0).max(1);
        let mut natural = vec![MINIMUM_COLUMN_WIDTH; column_count];
        let mut minimum = vec![MINIMUM_COLUMN_WIDTH; column_count];
        let header_base = BaseStyle { bold: true, role: TextRole::Strong, ..BODY };
        for (row_index, row) in std::iter::once(header).chain(rows.iter().map(Vec::as_slice)).enumerate() {
            let base = if row_index == 0 { header_base } else { BODY };
            for (column, cell) in row.iter().enumerate() {
                let (line, word) = self.natural_widths(cell, base);
                natural[column] = natural[column].max(line + 2.0 * CELL_PADDING);
                minimum[column] = minimum[column].max((word + 2.0 * CELL_PADDING).min(width / column_count as f32));
            }
        }
        let natural_total: f32 = natural.iter().sum();
        if natural_total <= width {
            // Everything fits on one line: spread the spare room in proportion.
            return natural.iter().map(|column| column / natural_total * width).collect();
        }
        let minimum_total: f32 = minimum.iter().sum();
        if minimum_total >= width {
            return minimum.iter().map(|column| column / minimum_total * width).collect();
        }
        let spare = width - minimum_total;
        let wanted: f32 = natural.iter().zip(&minimum).map(|(natural, minimum)| natural - minimum).sum();
        natural
            .iter()
            .zip(&minimum)
            .map(|(natural, minimum)| {
                minimum + if wanted > 0.0 { (natural - minimum) / wanted * spare } else { spare / column_count as f32 }
            })
            .collect()
    }

    fn lay_out_table(
        &mut self,
        header: &[Inline],
        rows: &[Vec<Inline>],
        y: f32,
        width: f32,
        layout: &mut MarkdownLayout,
    ) -> f32 {
        let widths = self.table_column_widths(header, rows, width);
        let mut column_x = Vec::with_capacity(widths.len());
        let mut next_x = 0.0;
        for column_width in &widths {
            column_x.push(next_x);
            next_x += column_width;
        }
        let mut row_y = y;
        let has_header = !header.is_empty();
        let all_rows = std::iter::once(header).filter(|_| has_header).chain(rows.iter().map(Vec::as_slice));
        for (row_index, cells) in all_rows.enumerate() {
            let is_header = has_header && row_index == 0;
            let base = if is_header { BaseStyle { bold: true, role: TextRole::Strong, ..BODY } } else { BODY };
            let fill_index = layout.items.len();
            if is_header {
                layout.items.push(LaidOut::Fill { x: 0.0, y: row_y, width, height: 0.0, role: FillRole::TableHeader });
            }
            let mut row_height: f32 = 0.0;
            for (column, cell) in cells.iter().enumerate().take(widths.len()) {
                let cell_height = self.lay_out_inline(
                    cell,
                    base,
                    column_x[column] + CELL_PADDING,
                    row_y + CELL_PADDING,
                    widths[column] - 2.0 * CELL_PADDING,
                    false,
                    layout,
                );
                row_height = row_height.max(cell_height);
            }
            let row_height =
                row_height.max(self.fonts.height(font_for(SpanStyle::default(), base))) + 2.0 * CELL_PADDING;
            if let Some(LaidOut::Fill { height, .. }) = layout.items.get_mut(fill_index).filter(|_| is_header) {
                *height = row_height;
            }
            layout.items.push(LaidOut::Line { x: 0.0, y: row_y + row_height - 1.0, width, strong: false });
            row_y += row_height;
        }
        row_y - y
    }

    fn record_heading(layout: &mut MarkdownLayout, level: u8, content: &Inline, id: Option<&str>, y: f32) {
        let text = plain_text(content);
        if let Some(id) = id {
            layout.anchors.push((id.to_string(), y));
        }
        let github = github_slug(&text);
        let duplicates = layout.headings.iter().filter(|(_, other, _)| github_slug(other) == github).count();
        if duplicates > 0 {
            layout.anchors.push((format!("{github}-{duplicates}"), y));
        }
        layout.anchors.push((github, y));
        layout.anchors.push((compact_slug(&text), y));
        layout.headings.push((level, text, y));
    }

    /// Lays a document out at `width` (from its top left at 0, 0).
    pub(super) fn lay_out_markdown(
        &mut self,
        blocks: &[MarkdownBlock],
        width: f32,
        show_title_heading: bool,
    ) -> MarkdownLayout {
        let mut layout = MarkdownLayout::default();
        let mut y = 0.0;
        let mut first = true;
        for block in blocks {
            let mut shown = true;
            match block {
                MarkdownBlock::Heading { level, content, id } => {
                    let (space_above, size, role, space_below) = match level {
                        1 => (24.0, 22.0, TextRole::Strong, 10.0),
                        2 => (20.0, 18.0, TextRole::Accent, 8.0),
                        3 => (12.0, 15.0, TextRole::Strong, 8.0),
                        4 => (10.0, 14.0, TextRole::Strong, 6.0),
                        5 => (8.0, 13.0, TextRole::Strong, 6.0),
                        _ => (8.0, 13.0, TextRole::Body, 6.0),
                    };
                    if *level == 1 && !show_title_heading {
                        shown = false;
                        Frame::record_heading(&mut layout, *level, content, id.as_deref(), y);
                        layout.headings.pop();
                    } else {
                        if !first || *level >= 3 {
                            y += space_above;
                        }
                        Frame::record_heading(&mut layout, *level, content, id.as_deref(), y);
                        let base = BaseStyle { size, bold: true, role };
                        y += self.lay_out_inline(content, base, 0.0, y, width, false, &mut layout) + space_below;
                    }
                }
                MarkdownBlock::Paragraph { content, centered } => {
                    if has_visible_text(content) {
                        y += self.lay_out_inline(content, BODY, 0.0, y, width, *centered, &mut layout) + 10.0;
                    } else {
                        shown = false;
                        self.lay_out_inline(content, BODY, 0.0, y, width, false, &mut layout);
                    }
                }
                MarkdownBlock::ListItem { depth, marker, content } => {
                    let indent = f32::from(*depth) * LIST_INDENT;
                    let content_x = indent + LIST_MARKER_WIDTH;
                    let marker_text = match marker {
                        ListMarker::Bullet => Some("•".to_string()),
                        ListMarker::Number(number) => Some(format!("{number}.")),
                        ListMarker::Continuation => None,
                    };
                    if let Some(marker_text) = marker_text {
                        let font = font_for(SpanStyle::default(), BODY);
                        let marker_width = self.fonts.width(font, &marker_text);
                        let marker_x = (indent + LIST_MARKER_WIDTH - 6.0 - marker_width).max(indent);
                        layout.items.push(LaidOut::Text {
                            x: marker_x,
                            y,
                            width: marker_width,
                            height: self.fonts.height(font),
                            text: marker_text,
                            font,
                            role: TextRole::Body,
                            style: SpanStyle::default(),
                            link: None,
                        });
                    }
                    let height =
                        self.lay_out_inline(content, BODY, content_x, y, width - content_x, false, &mut layout);
                    y += height.max(self.fonts.height(font_for(SpanStyle::default(), BODY))) + 6.0;
                }
                MarkdownBlock::Quote(content) => {
                    let height = self.lay_out_inline(content, BODY, 14.0, y, width - 14.0, false, &mut layout);
                    layout.items.push(LaidOut::Fill { x: 0.0, y, width: 3.0, height, role: FillRole::QuoteBar });
                    y += height + 10.0;
                }
                MarkdownBlock::Rule => {
                    y += 8.0;
                    layout.items.push(LaidOut::Line { x: 0.0, y, width, strong: true });
                    y += 14.0;
                }
                MarkdownBlock::Code(code) => {
                    let padding = 10.0;
                    let fill_index = layout.items.len();
                    layout.items.push(LaidOut::Fill { x: 0.0, y, width, height: 0.0, role: FillRole::CodeBlock });
                    let mut inline: Inline = Vec::new();
                    for (index, line) in code.split('\n').enumerate() {
                        if index > 0 {
                            inline.push(InlineItem::LineBreak);
                        }
                        // Leading spaces are kept (as no-break spaces) for indentation.
                        let indent = line.len() - line.trim_start_matches(' ').len();
                        let text = format!("{}{}", "\u{a0}".repeat(indent), &line[indent..]);
                        inline.push(InlineItem::Text { text, style: SpanStyle::default(), link: None });
                    }
                    let base = BaseStyle { size: 12.0, bold: false, role: TextRole::Strong };
                    let height = self.lay_out_inline(
                        &inline,
                        base,
                        padding,
                        y + padding,
                        width - 2.0 * padding,
                        false,
                        &mut layout,
                    ) + 2.0 * padding;
                    if let Some(LaidOut::Fill { height: fill_height, .. }) = layout.items.get_mut(fill_index) {
                        *fill_height = height;
                    }
                    y += height + 12.0;
                }
                MarkdownBlock::Table { header, rows } => {
                    y += self.lay_out_table(header, rows, y, width, &mut layout) + 12.0;
                }
                MarkdownBlock::Image { alt, resolved_path, width: wanted_width, centered, .. } => {
                    y += 8.0;
                    let size = resolved_path
                        .as_deref()
                        .and_then(|path| self.images.get(path).map(|texture| (path, texture.width, texture.height)));
                    match size {
                        Some((path, natural_width, natural_height)) if natural_width > 0.0 => {
                            let shown_width = wanted_width.unwrap_or(width).min(width);
                            let shown_height = natural_height * shown_width / natural_width;
                            let x = if *centered { (width - shown_width) / 2.0 } else { 0.0 };
                            layout.items.push(LaidOut::Image {
                                x,
                                y,
                                width: shown_width,
                                height: shown_height,
                                path: path.to_string(),
                            });
                            y += shown_height + 12.0;
                        }
                        _ => {
                            let label = if alt.is_empty() { "(image)".to_string() } else { alt.clone() };
                            layout.items.push(LaidOut::Placeholder { x: 0.0, y, width, height: 72.0, label });
                            y += 72.0 + 12.0;
                        }
                    }
                }
            }
            if shown {
                first = false;
            }
        }
        layout.height = y;
        layout
    }

    /// Draws a laid-out document with its top left at `(x, y)`, only the pieces between
    /// `visible_top` and `visible_bottom` (screen coordinates). Returns the target of a link
    /// clicked this frame.
    pub(super) fn draw_markdown_layout(
        &mut self,
        layout: &MarkdownLayout,
        x: f32,
        y: f32,
        (visible_top, visible_bottom): (f32, f32),
        id: &str,
    ) -> Option<String> {
        use crate::ui::context::RegionOptions;
        use crate::ui::draw::TextOptions;
        use crate::ui::theme::with_alpha;
        let palette = self.palette();
        let mut clicked = None;
        for (index, item) in layout.items.iter().enumerate() {
            let (top, bottom) = item.top_and_bottom();
            if y + bottom < visible_top || y + top > visible_bottom {
                continue;
            }
            match item {
                LaidOut::Text { x: item_x, y: item_y, width, height, text, font, role, style, link } => {
                    let (left, top) = (x + item_x, y + item_y);
                    let mut color = match role {
                        TextRole::Body => palette.on_surface_variant,
                        TextRole::Strong => palette.on_surface,
                        TextRole::Accent | TextRole::Link => palette.primary,
                    };
                    let mut hovered = false;
                    if let Some(link) = link {
                        let region = self.ui.region(
                            &format!("{id}:link:{index}"),
                            left,
                            top,
                            *width,
                            *height,
                            RegionOptions::hand(),
                        );
                        hovered = region.hovered;
                        if region.clicked {
                            clicked = layout.links.get(*link).cloned();
                        }
                        if hovered {
                            color = palette.on_surface;
                        }
                    }
                    if style.code {
                        self.fill_rect(
                            left - 2.0,
                            top + 1.0,
                            width + 4.0,
                            height - 2.0,
                            3.0,
                            with_alpha(palette.on_surface, 0.08),
                        );
                    }
                    self.text(text, left, top, TextOptions::new().font(*font).color(color));
                    let underline = link.is_some() || style.underline;
                    if underline || style.strike {
                        let list = self.ui.list();
                        list.set_color(color);
                        list.set_line_width(if hovered { 1.5 } else { 1.0 });
                        if underline {
                            let line_y = top + height * 0.86;
                            list.line(&[left, line_y, left + width, line_y]);
                        }
                        if style.strike {
                            let line_y = top + height * 0.55;
                            list.line(&[left, line_y, left + width, line_y]);
                        }
                        list.set_line_width(1.0);
                    }
                }
                LaidOut::Fill { x: item_x, y: item_y, width, height, role } => {
                    let (color, radius) = match role {
                        FillRole::TableHeader => (palette.surface_container_highest, 0.0),
                        FillRole::CodeBlock => (with_alpha(palette.on_surface, 0.06), 6.0),
                        FillRole::QuoteBar => (with_alpha(palette.primary, 0.6), 1.5),
                    };
                    self.fill_rect(x + item_x, y + item_y, *width, *height, radius, color);
                }
                LaidOut::Line { x: item_x, y: item_y, width, strong } => {
                    let alpha = if *strong { 0.7 } else { 0.4 };
                    self.divider(x + item_x, y + item_y, *width, Some(with_alpha(palette.outline, alpha)));
                }
                LaidOut::Image { x: item_x, y: item_y, width, path, .. } => {
                    if let Some(texture) = self.images.get(path) {
                        let scale = width / texture.width;
                        self.draw_image(texture, x + item_x, y + item_y, scale, 1.0);
                    }
                }
                LaidOut::Placeholder { x: item_x, y: item_y, width, height, label } => {
                    let fill = with_alpha(palette.on_surface_variant, 0.15);
                    self.fill_rect(x + item_x, y + item_y, *width, *height, 4.0, fill);
                    let options = TextOptions::new()
                        .width(*width)
                        .middle_of(*height)
                        .centered()
                        .color(palette.on_surface_variant);
                    self.text(label, x + item_x, y + item_y, options);
                }
            }
        }
        clicked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_splits_into_words_and_gaps_keeping_no_break_spaces() {
        assert_eq!(
            words_and_gaps("a  bc\u{a0}d e"),
            vec![(false, "a"), (true, "  "), (false, "bc\u{a0}d"), (true, " "), (false, "e")]
        );
        assert_eq!(words_and_gaps(""), Vec::<(bool, &str)>::new());
    }
}
