//! Font metrics and text layout exactly as LÖVE 11 computes them (`Font.cpp`,
//! `TrueTypeRasterizer.cpp` and FreeType's size metrics), so widths, wrapping, line heights
//! and alignment match the original pixel for pixel.
//!
//! All results are in logical units: LÖVE rasterizes at `size × dpi_scale` physical pixels and
//! divides metrics back by the scale, rounding most of them to whole units.

/// The font-file values FreeType derives its size metrics from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontTables {
    pub units_per_em: i64,
    /// `hhea` ascender/descender/line gap (FreeType's `face->ascender` etc.).
    pub ascender: i64,
    pub descender: i64,
    pub line_gap: i64,
    /// `head` flags bit 3: the TrueType driver then rounds ascender/descender/height.
    pub integer_ppem: bool,
}

impl FontTables {
    /// Reads the `head` and `hhea` tables of a TrueType/OpenType font file.
    pub fn parse(font_file: &[u8]) -> Option<FontTables> {
        let read_u16 = |offset: usize| -> Option<u16> {
            font_file.get(offset..offset + 2).map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
        };
        let read_i16 = |offset: usize| read_u16(offset).map(|value| value as i16 as i64);
        let read_u32 = |offset: usize| -> Option<u32> {
            font_file.get(offset..offset + 4).map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        };
        let table_count = read_u16(4)? as usize;
        let mut head = None;
        let mut hhea = None;
        for index in 0..table_count {
            let record = 12 + index * 16;
            let tag = font_file.get(record..record + 4)?;
            let offset = read_u32(record + 8)? as usize;
            match tag {
                b"head" => head = Some(offset),
                b"hhea" => hhea = Some(offset),
                _ => {}
            }
        }
        let (head, hhea) = (head?, hhea?);
        let ascender = read_i16(hhea + 4)?;
        let descender = read_i16(hhea + 6)?;
        let line_gap = read_i16(hhea + 8)?;
        Some(FontTables {
            units_per_em: read_u16(head + 18)? as i64,
            ascender,
            descender,
            line_gap,
            integer_ppem: read_u16(head + 16)? & 8 != 0,
        })
    }

    /// FreeType's `face->height`.
    fn face_height(&self) -> i64 {
        self.ascender - self.descender + self.line_gap
    }

    /// `(ascender, descender, height)` in whole physical pixels for a pixel size, as FreeType
    /// reports them in `size->metrics` (then `>> 6` in LÖVE).
    pub fn size_metrics(&self, pixel_size: u32) -> (i64, i64, i64) {
        let y_scale = freetype_div_fix(i64::from(pixel_size) * 64, self.units_per_em);
        let ascender = freetype_mul_fix(self.ascender, y_scale);
        let descender = freetype_mul_fix(self.descender, y_scale);
        let height = freetype_mul_fix(self.face_height(), y_scale);
        // Checked against LÖVE 11.5: ascender rounds up and descender down (even for fonts
        // flagged for integer ppem), height rounds to nearest.
        (pixel_ceil(ascender) >> 6, pixel_floor(descender) >> 6, pixel_round(height) >> 6)
    }
}

/// FreeType's `FT_MulFix`: `a × b / 65536`, rounded to nearest.
fn freetype_mul_fix(a: i64, b: i64) -> i64 {
    let product = a * b;
    let sign = if product < 0 { -1 } else { 1 };
    sign * ((product.abs() + 0x8000) >> 16)
}

/// FreeType's `FT_DivFix`: `a × 65536 / b`, rounded to nearest.
fn freetype_div_fix(a: i64, b: i64) -> i64 {
    let sign = if (a < 0) != (b < 0) { -1 } else { 1 };
    let (a, b) = (a.abs(), b.abs());
    sign * (((a << 16) + (b >> 1)) / b)
}

fn pixel_round(value: i64) -> i64 {
    (value + 32) & !63
}

fn pixel_ceil(value: i64) -> i64 {
    (value + 63) & !63
}

fn pixel_floor(value: i64) -> i64 {
    value & !63
}

/// LÖVE rounds logical metrics with `floorf(x / dpiScale + 0.5f)`.
pub fn logical_round(physical: f32, dpi_scale: f32) -> f32 {
    (physical / dpi_scale + 0.5).floor()
}

/// A font's line metrics in logical units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMetrics {
    /// `Font:getHeight()`.
    pub height: f32,
    /// `Font:getAscent()`; glyphs sit on a baseline this far below the line top.
    pub ascent: f32,
}

impl LineMetrics {
    pub fn for_size(tables: &FontTables, pixel_size: u32, dpi_scale: f32) -> LineMetrics {
        let (ascender, _, height) = tables.size_metrics(pixel_size);
        LineMetrics {
            height: logical_round(height as f32, dpi_scale),
            ascent: logical_round(ascender as f32, dpi_scale),
        }
    }

    /// `Font:getBaseline()`: the ascent, or `height / 1.25` when a font has none.
    pub fn baseline(&self) -> f32 {
        if self.ascent != 0.0 { self.ascent } else { (self.height / 1.25 + 0.5).floor() }
    }

    /// The distance between lines inside one `print` (line height 1).
    pub fn line_step(&self) -> f32 {
        (self.height + 0.5).floor()
    }
}

/// The pixel size LÖVE rasterizes a font at.
pub fn pixel_size_for(size: f32, dpi_scale: f32) -> u32 {
    (size * dpi_scale).round().max(1.0) as u32
}

/// Per-character advance in logical units (`Glyph::spacing`, already rounded). The original's
/// fonts have no `kern` table, so LÖVE never kerned them.
pub trait GlyphAdvance {
    fn advance(&mut self, character: char) -> f32;
}

impl<F: FnMut(char) -> f32> GlyphAdvance for F {
    fn advance(&mut self, character: char) -> f32 {
        self(character)
    }
}

/// `Font:getWidth(text)`: the widest line, `\r` ignored.
pub fn text_width(text: &str, glyphs: &mut impl GlyphAdvance) -> f32 {
    text.split('\n')
        .map(|line| {
            line.chars().filter(|&character| character != '\r').map(|character| glyphs.advance(character)).sum()
        })
        .fold(0.0, f32::max)
}

/// One wrapped line.
#[derive(Debug, Clone, PartialEq)]
pub struct WrappedLine {
    pub text: String,
    /// The line's width without trailing spaces.
    pub width: f32,
}

/// `Font:getWrap(text, limit)` (LÖVE 11's `Font::getWrap`): splits at newlines and wraps at the
/// last space before the limit; a word with no space before it breaks mid-word, and a single
/// character wider than the limit at a line start is dropped (leaving an empty line), as in
/// LÖVE. Wrapped lines keep their trailing spaces in `text`.
pub fn wrap_text(text: &str, wrap_limit: f32, glyphs: &mut impl GlyphAdvance) -> Vec<WrappedLine> {
    let characters: Vec<char> = text.chars().collect();
    let mut lines = Vec::new();
    let mut line: Vec<char> = Vec::new();
    let mut width = 0.0f32;
    let mut width_before_last_space = 0.0f32;
    let mut width_of_trailing_space = 0.0f32;
    let mut previous: Option<char> = None;
    let mut last_space_index: Option<usize> = None;
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if character == '\n' {
            lines.push(WrappedLine { text: line.iter().collect(), width: width - width_of_trailing_space });
            line.clear();
            width = 0.0;
            width_before_last_space = 0.0;
            width_of_trailing_space = 0.0;
            previous = None;
            last_space_index = None;
            index += 1;
            continue;
        }
        if character == '\r' {
            index += 1;
            continue;
        }
        let character_width = glyphs.advance(character);
        let new_width = width + character_width;
        if character != ' ' && new_width > wrap_limit {
            if line.is_empty() {
                // A character wider than the limit at a line start is skipped completely.
                index += 1;
            } else if let Some(space_index) = last_space_index {
                // Rewind to the last space; the next line starts after it.
                while line.last().is_some_and(|&last| last != ' ') {
                    line.pop();
                }
                width = width_before_last_space;
                index = space_index + 1;
            }
            lines.push(WrappedLine { text: line.iter().collect(), width });
            line.clear();
            width = 0.0;
            width_before_last_space = 0.0;
            width_of_trailing_space = 0.0;
            previous = None;
            last_space_index = None;
            continue;
        }
        if previous != Some(' ') && character == ' ' {
            width_before_last_space = width;
        }
        width = new_width;
        previous = Some(character);
        line.push(character);
        if character == ' ' {
            last_space_index = Some(index);
            width_of_trailing_space += character_width;
        } else {
            width_of_trailing_space = 0.0;
        }
        index += 1;
    }
    lines.push(WrappedLine { text: line.iter().collect(), width: width - width_of_trailing_space });
    lines
}

/// `printf` alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// A glyph placed by layout: its character and pen position (line top-left), in logical units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedGlyph {
    pub character: char,
    pub x: f32,
    /// The line's top; the glyph's baseline is at `y + baseline`.
    pub y: f32,
}

/// LÖVE's `generateVertices` for one block of text: newlines step down by the line height,
/// `\r` is skipped, and each space gets `extra_spacing` (justify).
fn place_line(
    text: &str,
    offset_x: f32,
    offset_y: f32,
    extra_spacing: f32,
    metrics: &LineMetrics,
    glyphs: &mut impl GlyphAdvance,
    placed: &mut Vec<PlacedGlyph>,
) {
    let mut x = offset_x;
    let mut y = offset_y;
    for character in text.chars() {
        if character == '\n' {
            y += metrics.line_step();
            x = offset_x;
            continue;
        }
        if character == '\r' {
            continue;
        }
        placed.push(PlacedGlyph { character, x, y });
        x += glyphs.advance(character);
        if character == ' ' && extra_spacing != 0.0 {
            x = (x + extra_spacing).floor();
        }
    }
}

/// `love.graphics.print(text)` at the origin.
pub fn layout_print(text: &str, metrics: &LineMetrics, glyphs: &mut impl GlyphAdvance) -> Vec<PlacedGlyph> {
    let mut placed = Vec::new();
    place_line(text, 0.0, 0.0, 0.0, metrics, glyphs, &mut placed);
    placed
}

/// `love.graphics.printf(text, 0, 0, limit, align)`.
pub fn layout_printf(
    text: &str,
    wrap_limit: f32,
    align: Align,
    metrics: &LineMetrics,
    glyphs: &mut impl GlyphAdvance,
) -> Vec<PlacedGlyph> {
    let wrap_limit = wrap_limit.max(0.0);
    let lines = wrap_text(text, wrap_limit, glyphs);
    let mut placed = Vec::new();
    let mut y = 0.0f32;
    for line in &lines {
        let mut offset_x = 0.0;
        let mut extra_spacing = 0.0;
        match align {
            Align::Left => {}
            Align::Right => offset_x = (wrap_limit - line.width).floor(),
            Align::Center => offset_x = ((wrap_limit - line.width) / 2.0).floor(),
            Align::Justify => {
                let spaces = line.text.chars().filter(|&character| character == ' ').count() as f32;
                if line.width < wrap_limit && spaces >= 1.0 {
                    extra_spacing = (wrap_limit - line.width) / spaces;
                }
            }
        }
        place_line(&line.text, offset_x, y.floor(), extra_spacing, metrics, glyphs, &mut placed);
        y += metrics.height;
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROBOTO: FontTables =
        FontTables { units_per_em: 2048, ascender: 1900, descender: -500, line_gap: 0, integer_ppem: true };

    /// Every character is 10 wide, spaces 4.
    fn fixed(character: char) -> f32 {
        if character == ' ' { 4.0 } else { 10.0 }
    }

    #[test]
    fn freetype_size_metrics_for_roboto() {
        assert_eq!(ROBOTO.size_metrics(10), (10, -3, 12));
        assert_eq!(ROBOTO.size_metrics(14), (13, -4, 16));
        assert_eq!(ROBOTO.size_metrics(16), (15, -4, 19));
        assert_eq!(ROBOTO.size_metrics(28), (26, -7, 33));
        let metrics = LineMetrics::for_size(&ROBOTO, pixel_size_for(14.0, 1.0), 1.0);
        assert_eq!((metrics.height, metrics.ascent), (16.0, 13.0));
        // At 1.5x, 21 px: height round(24.6)=25 -> 25/1.5=16.67 -> 17 logical.
        let scaled = LineMetrics::for_size(&ROBOTO, pixel_size_for(14.0, 1.5), 1.5);
        assert_eq!((scaled.height, scaled.ascent), (17.0, 13.0));
    }

    #[test]
    fn parses_the_bundled_fonts() {
        let fonts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts");
        let roboto = FontTables::parse(&std::fs::read(fonts.join("Roboto-Regular.ttf")).unwrap()).unwrap();
        assert_eq!(roboto, ROBOTO);
        let icons = FontTables::parse(&std::fs::read(fonts.join("MaterialIcons-Regular.ttf")).unwrap()).unwrap();
        assert_eq!((icons.units_per_em, icons.ascender, icons.descender), (512, 512, 0));
    }

    #[test]
    fn width_is_the_widest_line() {
        assert_eq!(text_width("ab c", &mut fixed), 34.0);
        assert_eq!(text_width("a\r\nabc", &mut fixed), 30.0);
        assert_eq!(text_width("", &mut fixed), 0.0);
    }

    #[test]
    fn wrapping_follows_love() {
        let texts = |lines: Vec<WrappedLine>| lines.into_iter().map(|line| (line.text, line.width)).collect::<Vec<_>>();
        // "aa bb" = 20 + 4 + 20 = 44; with a 30 limit "bb" wraps after the space.
        assert_eq!(texts(wrap_text("aa bb", 30.0, &mut fixed)), [("aa ".to_string(), 20.0), ("bb".to_string(), 20.0)]);
        // No space: break mid-word.
        assert_eq!(texts(wrap_text("aaaa", 25.0, &mut fixed)), [("aa".to_string(), 20.0), ("aa".to_string(), 20.0)]);
        // Newlines split; trailing spaces don't count toward a line's width.
        assert_eq!(texts(wrap_text("a  \nb", 100.0, &mut fixed)), [("a  ".to_string(), 10.0), ("b".to_string(), 10.0)]);
        // A character wider than the limit at a line start is dropped.
        assert_eq!(texts(wrap_text("a", 5.0, &mut fixed)), [(String::new(), 0.0), (String::new(), 0.0)]);
        // Spaces never cause a wrap themselves.
        assert_eq!(wrap_text("aa      ", 25.0, &mut fixed).len(), 1);
        assert_eq!(wrap_text("", 25.0, &mut fixed).len(), 1);
    }

    #[test]
    fn printf_alignment() {
        let metrics = LineMetrics { height: 16.0, ascent: 13.0 };
        let centered = layout_printf("ab", 45.0, Align::Center, &metrics, &mut fixed);
        assert_eq!(centered[0].x, 12.0); // floor((45 - 20) / 2)
        let right = layout_printf("ab\nc", 45.0, Align::Right, &metrics, &mut fixed);
        assert_eq!((right[0].x, right[2].x, right[2].y), (25.0, 35.0, 16.0));
        let justified = layout_printf("a b\nc", 30.0, Align::Justify, &metrics, &mut fixed);
        // "a b" is 24 wide; the one space gets 6 more: b at floor(10 + 4 + 6) = 20.
        assert_eq!(justified[2].x, 20.0);
        let printed = layout_print("a\nb", &metrics, &mut fixed);
        assert_eq!((printed[1].x, printed[1].y), (0.0, 16.0));
    }
}
