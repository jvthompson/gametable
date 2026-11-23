//! Fonts and text drawing. Port of the font handling in `theme.lua` and LÖVE's `Font`:
//! Roboto (regular/medium/bold) with Noto Sans Symbols 2 as a fallback for symbols such as card
//! suits, and Material Icons, rasterized by SDL3_ttf at `size × dpi_scale` pixels into glyph
//! atlas pages, laid out by [`text_layout`](super::text_layout) exactly as LÖVE lays text out.
//!
//! GameTable addition: the text faces come from a [`FontFamily`] the player picks in Settings
//! (Roboto by default; Atkinson Hyperlegible, Open Sans, Lato or Merriweather). Icons and the
//! symbol fallback are the same in every family.

use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};

use sdl3::ttf::sys as ttf;

use super::draw_list::{DrawList, TextureId};
use super::gpu_renderer::{GpuRenderer, RenderResult};
use super::image_data::RgbaImage;
use super::text_layout::{
    Align, FontTables, LineMetrics, PlacedGlyph, WrappedLine, layout_print, layout_printf, logical_round,
    pixel_size_for, text_width, wrap_text,
};

/// The font files the app uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontFace {
    Regular,
    Medium,
    Bold,
    /// Roboto Italic and Bold Italic, for emphasis in Markdown documents (GameTable additions).
    Italic,
    BoldItalic,
    /// Material Icons (`theme.iconFont`).
    Icons,
    /// Noto Sans Symbols 2, the text faces' fallback.
    Symbols,
}

impl FontFace {
    /// The faces that come from the chosen [`FontFamily`].
    pub const TEXT_FACES: [FontFace; 5] =
        [FontFace::Regular, FontFace::Medium, FontFace::Bold, FontFace::Italic, FontFace::BoldItalic];

    pub fn is_text(self) -> bool {
        !matches!(self, FontFace::Icons | FontFace::Symbols)
    }

    fn fallback(self) -> Option<FontFace> {
        match self {
            FontFace::Regular | FontFace::Medium | FontFace::Bold | FontFace::Italic | FontFace::BoldItalic => {
                Some(FontFace::Symbols)
            }
            FontFace::Icons | FontFace::Symbols => None,
        }
    }
}

/// The typeface family all text is drawn in (Settings > Display > Font).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FontFamily {
    #[default]
    Roboto,
    AtkinsonHyperlegible,
    OpenSans,
    Lato,
    Merriweather,
}

impl FontFamily {
    pub const ALL: [FontFamily; 5] = [
        FontFamily::Roboto,
        FontFamily::AtkinsonHyperlegible,
        FontFamily::OpenSans,
        FontFamily::Lato,
        FontFamily::Merriweather,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            FontFamily::Roboto => "Roboto",
            FontFamily::AtkinsonHyperlegible => "Atkinson Hyperlegible",
            FontFamily::OpenSans => "Open Sans",
            FontFamily::Lato => "Lato",
            FontFamily::Merriweather => "Merriweather",
        }
    }

    /// How the family is stored in the settings.
    pub fn setting_value(self) -> &'static str {
        match self {
            FontFamily::Roboto => "roboto",
            FontFamily::AtkinsonHyperlegible => "atkinson_hyperlegible",
            FontFamily::OpenSans => "open_sans",
            FontFamily::Lato => "lato",
            FontFamily::Merriweather => "merriweather",
        }
    }

    pub fn from_setting_value(value: &str) -> Option<FontFamily> {
        FontFamily::ALL.into_iter().find(|family| family.setting_value() == value)
    }

    /// The file a face is drawn from in this family. Families without a medium weight use their
    /// bold (Open Sans uses its semibold); icons and symbols are shared by every family.
    pub fn file_name(self, face: FontFace) -> &'static str {
        match (self, face) {
            (_, FontFace::Icons) => "MaterialIcons-Regular.ttf",
            (_, FontFace::Symbols) => "NotoSansSymbols2-Regular.ttf",
            (FontFamily::Roboto, FontFace::Regular) => "Roboto-Regular.ttf",
            (FontFamily::Roboto, FontFace::Medium) => "Roboto-Medium.ttf",
            (FontFamily::Roboto, FontFace::Bold) => "Roboto-Bold.ttf",
            (FontFamily::Roboto, FontFace::Italic) => "Roboto-Italic.ttf",
            (FontFamily::Roboto, FontFace::BoldItalic) => "Roboto-BoldItalic.ttf",
            (FontFamily::AtkinsonHyperlegible, FontFace::Regular) => "AtkinsonHyperlegible-Regular.ttf",
            (FontFamily::AtkinsonHyperlegible, FontFace::Medium | FontFace::Bold) => "AtkinsonHyperlegible-Bold.ttf",
            (FontFamily::AtkinsonHyperlegible, FontFace::Italic) => "AtkinsonHyperlegible-Italic.ttf",
            (FontFamily::AtkinsonHyperlegible, FontFace::BoldItalic) => "AtkinsonHyperlegible-BoldItalic.ttf",
            (FontFamily::OpenSans, FontFace::Regular) => "OpenSans-Regular.ttf",
            (FontFamily::OpenSans, FontFace::Medium) => "OpenSans-SemiBold.ttf",
            (FontFamily::OpenSans, FontFace::Bold) => "OpenSans-Bold.ttf",
            (FontFamily::OpenSans, FontFace::Italic) => "OpenSans-Italic.ttf",
            (FontFamily::OpenSans, FontFace::BoldItalic) => "OpenSans-BoldItalic.ttf",
            (FontFamily::Lato, FontFace::Regular) => "Lato-Regular.ttf",
            (FontFamily::Lato, FontFace::Medium) => "Lato-Medium.ttf",
            (FontFamily::Lato, FontFace::Bold) => "Lato-Bold.ttf",
            (FontFamily::Lato, FontFace::Italic) => "Lato-Italic.ttf",
            (FontFamily::Lato, FontFace::BoldItalic) => "Lato-BoldItalic.ttf",
            (FontFamily::Merriweather, FontFace::Regular) => "Merriweather-Regular.ttf",
            (FontFamily::Merriweather, FontFace::Medium | FontFace::Bold) => "Merriweather-Bold.ttf",
            (FontFamily::Merriweather, FontFace::Italic) => "Merriweather-Italic.ttf",
            (FontFamily::Merriweather, FontFace::BoldItalic) => "Merriweather-BoldItalic.ttf",
        }
    }

    /// The family a face's metrics and file come from when this family is chosen.
    fn for_face(self, face: FontFace) -> FontFamily {
        if face.is_text() { self } else { FontFamily::Roboto }
    }
}

/// A font at a size in logical pixels (`theme.font(size, weight)` rounds the size first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FontKey {
    pub face: FontFace,
    pub size: u32,
    /// Whether the user's Text size setting enlarges it. Interface text does; icons and text
    /// drawn on the game table (card faces, counters, tokens) keep their designed size.
    pub follows_text_size: bool,
}

impl FontKey {
    /// Interface text (icons excepted) follows the Text size setting.
    pub fn new(face: FontFace, size: f32) -> FontKey {
        FontKey { face, size: (size + 0.5).floor().max(1.0) as u32, follows_text_size: face != FontFace::Icons }
    }

    /// A font that ignores the Text size setting.
    pub fn fixed(face: FontFace, size: f32) -> FontKey {
        FontKey { follows_text_size: false, ..FontKey::new(face, size) }
    }
}

/// An open SDL_ttf font, closed on drop.
struct TtfFont(*mut ttf::TTF_Font);

impl Drop for TtfFont {
    fn drop(&mut self) {
        unsafe { ttf::TTF_CloseFont(self.0) };
    }
}

impl TtfFont {
    fn open(path: &Path, pixel_size: u32) -> Result<TtfFont, String> {
        let path_text = CString::new(path.to_string_lossy().as_bytes()).map_err(|error| error.to_string())?;
        let raw = unsafe { ttf::TTF_OpenFont(path_text.as_ptr(), pixel_size as f32) };
        if raw.is_null() {
            return Err(format!("could not open {}: {}", path.display(), sdl3::get_error()));
        }
        unsafe { ttf::TTF_SetFontHinting(raw, ttf::TTF_HINTING_NORMAL) };
        Ok(TtfFont(raw))
    }

    fn has_glyph(&self, character: char) -> bool {
        unsafe { ttf::TTF_FontHasGlyph(self.0, character as u32) }
    }

    /// `(bearing_x, bearing_y, advance)` in physical pixels.
    fn glyph_metrics(&self, character: char) -> (i32, i32, i32) {
        let (mut min_x, mut max_x, mut min_y, mut max_y, mut advance) = (0, 0, 0, 0, 0);
        unsafe {
            ttf::TTF_GetGlyphMetrics(
                self.0,
                character as u32,
                &mut min_x,
                &mut max_x,
                &mut min_y,
                &mut max_y,
                &mut advance,
            );
        }
        (min_x, max_y, advance)
    }

    fn glyph_image(&self, character: char) -> Option<RgbaImage> {
        let mut image_type = ttf::TTF_ImageType::default();
        let raw = if self.has_glyph(character) {
            unsafe { ttf::TTF_GetGlyphImage(self.0, character as u32, &mut image_type) }
        } else {
            // LÖVE draws the font's missing-glyph box (glyph 0).
            unsafe { ttf::TTF_GetGlyphImageForIndex(self.0, 0, &mut image_type) }
        };
        if raw.is_null() {
            return None;
        }
        let surface = unsafe { sdl3::surface::Surface::from_ll(raw) };
        if surface.width() == 0 || surface.height() == 0 {
            return None;
        }
        // Blank glyphs (spaces) come back as a transparent pixel; LÖVE has no image for them.
        RgbaImage::from_surface(&surface).ok().filter(|image| image.pixels.chunks_exact(4).any(|pixel| pixel[3] != 0))
    }
}

/// A glyph's place in the atlas and its bearing, in physical pixels.
#[derive(Debug, Clone, Copy)]
struct AtlasGlyph {
    page: usize,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    bearing_x: i32,
    bearing_y: i32,
}

/// One font at one size and scale.
struct LoadedFont {
    face: FontFace,
    /// Whether LÖVE's Roboto advances apply (only while the family is Roboto).
    uses_love_advances: bool,
    pixel_size: u32,
    metrics: LineMetrics,
    primary: TtfFont,
    fallback: Option<TtfFont>,
    /// Logical advance per character (LÖVE's rounded `Glyph::spacing`).
    advances: HashMap<char, f32>,
    /// `None` for glyphs with no pixels (spaces).
    glyphs: HashMap<char, Option<AtlasGlyph>>,
}

impl LoadedFont {
    /// The font that draws a character: the primary when it has it, else the fallback, else the
    /// primary's missing-glyph box (as LÖVE does).
    fn font_for(&self, character: char) -> &TtfFont {
        match &self.fallback {
            Some(fallback) if !self.primary.has_glyph(character) && fallback.has_glyph(character) => fallback,
            _ => &self.primary,
        }
    }

    fn advance(&mut self, character: char, dpi_scale: f32) -> f32 {
        if let Some(advance) = self.advances.get(&character) {
            return *advance;
        }
        let love_advance = (self.uses_love_advances && (self.fallback.is_none() || self.primary.has_glyph(character)))
            .then(|| love_roboto_advance(self.face, self.pixel_size, character))
            .flatten();
        let physical_advance = love_advance.unwrap_or_else(|| self.font_for(character).glyph_metrics(character).2);
        let advance = logical_round(physical_advance as f32, dpi_scale);
        self.advances.insert(character, advance);
        advance
    }
}

/// Reads a face's metric tables from its file in `fonts_folder`.
fn read_font_tables(fonts_folder: &Path, family: FontFamily, face: FontFace) -> Result<FontTables, String> {
    let path = fonts_folder.join(family.file_name(face));
    let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    FontTables::parse(&bytes).ok_or_else(|| format!("{}: not a TrueType font", path.display()))
}

/// LÖVE 11.5's own advances for the Roboto faces (`assets/fonts/love_roboto_advances.txt`,
/// regenerated by `tools/lua_golden/font_advances/run.sh`). LÖVE's bundled FreeType auto-hints
/// Roboto's advances slightly differently from SDL_ttf's (digits especially), and layout must
/// match the original, so these take precedence where they exist.
const LOVE_ROBOTO_ADVANCES: &str = include_str!("../../../../assets/fonts/love_roboto_advances.txt");

/// `(face, pixel size) -> codepoint -> advance in physical pixels`.
type AdvanceTable = HashMap<(FontFace, u32), HashMap<char, i32>>;

fn love_roboto_advance_table() -> &'static AdvanceTable {
    static TABLE: std::sync::OnceLock<AdvanceTable> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut codepoints: Vec<char> = Vec::new();
        let mut table = AdvanceTable::new();
        for line in LOVE_ROBOTO_ADVANCES.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if let Some(list) = line.strip_prefix("codepoints ") {
                codepoints = list
                    .split(',')
                    .filter_map(|hex| u32::from_str_radix(hex, 16).ok().and_then(char::from_u32))
                    .collect();
                continue;
            }
            let mut parts = line.splitn(3, ' ');
            let face = match parts.next() {
                Some("Roboto-Regular.ttf") => FontFace::Regular,
                Some("Roboto-Medium.ttf") => FontFace::Medium,
                Some("Roboto-Bold.ttf") => FontFace::Bold,
                _ => continue,
            };
            let Some(pixel_size) = parts.next().and_then(|size| size.parse().ok()) else { continue };
            let advances = parts.next().unwrap_or("").split(',').filter_map(|value| value.parse().ok());
            table.insert((face, pixel_size), codepoints.iter().copied().zip(advances).collect());
        }
        table
    })
}

fn love_roboto_advance(face: FontFace, pixel_size: u32, character: char) -> Option<i32> {
    love_roboto_advance_table().get(&(face, pixel_size))?.get(&character).copied()
}

/// Square atlas pages, packed in shelves.
const ATLAS_PAGE_SIZE: u32 = 1024;
const GLYPH_PADDING: u32 = 1;

struct AtlasPage {
    texture: TextureId,
    pixels: Vec<u8>,
    shelf_x: u32,
    shelf_y: u32,
    shelf_height: u32,
    /// Rows `[start, end)` changed since the last upload.
    dirty_rows: Option<(u32, u32)>,
    created_on_gpu: bool,
}

impl AtlasPage {
    fn new() -> AtlasPage {
        AtlasPage {
            texture: TextureId::allocate(),
            pixels: vec![0; (ATLAS_PAGE_SIZE * ATLAS_PAGE_SIZE * 4) as usize],
            shelf_x: GLYPH_PADDING,
            shelf_y: GLYPH_PADDING,
            shelf_height: 0,
            dirty_rows: None,
            created_on_gpu: false,
        }
    }

    /// Finds room for a `width × height` image, or `None` when the page is full.
    fn allocate(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        if self.shelf_x + width + GLYPH_PADDING > ATLAS_PAGE_SIZE {
            self.shelf_x = GLYPH_PADDING;
            self.shelf_y += self.shelf_height + GLYPH_PADDING;
            self.shelf_height = 0;
        }
        if self.shelf_y + height + GLYPH_PADDING > ATLAS_PAGE_SIZE || width + 2 * GLYPH_PADDING > ATLAS_PAGE_SIZE {
            return None;
        }
        let position = (self.shelf_x, self.shelf_y);
        self.shelf_x += width + GLYPH_PADDING;
        self.shelf_height = self.shelf_height.max(height);
        Some(position)
    }

    fn copy_in(&mut self, x: u32, y: u32, image: &RgbaImage) {
        let row_bytes = (image.width * 4) as usize;
        for row in 0..image.height {
            let source = (row * image.width * 4) as usize;
            let target = (((y + row) * ATLAS_PAGE_SIZE + x) * 4) as usize;
            self.pixels[target..target + row_bytes].copy_from_slice(&image.pixels[source..source + row_bytes]);
        }
        let (start, end) = self.dirty_rows.unwrap_or((y, y + image.height));
        self.dirty_rows = Some((start.min(y), end.max(y + image.height)));
    }
}

/// Every font the app draws with, plus the glyph atlas.
pub struct FontLibrary {
    fonts_folder: PathBuf,
    /// Each loaded family's faces' tables (icons and symbols under Roboto).
    tables: HashMap<(FontFamily, FontFace), FontTables>,
    family: FontFamily,
    dpi_scale: f32,
    /// The Text size setting: multiplies the size of fonts that follow it.
    text_scale: f32,
    fonts: HashMap<FontKey, LoadedFont>,
    pages: Vec<AtlasPage>,
    /// Declared last so it is dropped (`TTF_Quit`) after every font is closed.
    _ttf_context: sdl3::ttf::Sdl3TtfContext,
}

impl FontLibrary {
    /// Opens the fonts in `fonts_folder` (the app's `assets/fonts`).
    pub fn new(fonts_folder: &Path) -> Result<FontLibrary, String> {
        let ttf_context = sdl3::ttf::init().map_err(|error| error.to_string())?;
        let mut tables = HashMap::new();
        for face in FontFace::TEXT_FACES.into_iter().chain([FontFace::Icons, FontFace::Symbols]) {
            tables.insert((FontFamily::Roboto, face), read_font_tables(fonts_folder, FontFamily::Roboto, face)?);
        }
        Ok(FontLibrary {
            fonts_folder: fonts_folder.to_path_buf(),
            tables,
            family: FontFamily::Roboto,
            dpi_scale: 1.0,
            text_scale: 1.0,
            fonts: HashMap::new(),
            pages: Vec::new(),
            _ttf_context: ttf_context,
        })
    }

    pub fn dpi_scale(&self) -> f32 {
        self.dpi_scale
    }

    pub fn family(&self) -> FontFamily {
        self.family
    }

    /// Switches every text face to `family`, re-rasterizing all text. A family whose files are
    /// missing or broken is skipped (the current one stays) with a message on the console.
    pub fn set_family(&mut self, family: FontFamily) {
        if family == self.family {
            return;
        }
        for face in FontFace::TEXT_FACES {
            if self.tables.contains_key(&(family, face)) {
                continue;
            }
            match read_font_tables(&self.fonts_folder, family, face) {
                Ok(parsed) => {
                    self.tables.insert((family, face), parsed);
                }
                Err(error) => {
                    eprintln!("fonts: can't use {}: {error}", family.display_name());
                    return;
                }
            }
        }
        self.family = family;
        self.drop_rasterized_fonts();
    }

    /// Re-rasterizes everything at a new UI scale (`theme.setScale` drops its font cache too).
    pub fn set_dpi_scale(&mut self, dpi_scale: f32) {
        if dpi_scale == self.dpi_scale {
            return;
        }
        self.dpi_scale = dpi_scale;
        self.drop_rasterized_fonts();
    }

    pub fn text_scale(&self) -> f32 {
        self.text_scale
    }

    /// Re-rasterizes interface text at a new Text size setting.
    pub fn set_text_scale(&mut self, text_scale: f32) {
        if text_scale == self.text_scale {
            return;
        }
        self.text_scale = text_scale;
        self.drop_rasterized_fonts();
    }

    /// Forgets every opened font and empties the glyph atlas, so text is rasterized again.
    fn drop_rasterized_fonts(&mut self) {
        self.fonts.clear();
        // Pages already on the GPU are reused from empty; their textures stay allocated.
        for page in &mut self.pages {
            let texture = page.texture;
            let created = page.created_on_gpu;
            *page = AtlasPage::new();
            page.texture = texture;
            page.created_on_gpu = created;
            page.dirty_rows = Some((0, ATLAS_PAGE_SIZE));
        }
    }

    fn font(&mut self, key: FontKey) -> &mut LoadedFont {
        if !self.fonts.contains_key(&key) {
            let text_scale = if key.follows_text_size { self.text_scale } else { 1.0 };
            let pixel_size = pixel_size_for(key.size as f32 * text_scale, self.dpi_scale);
            let family = self.family;
            let open = |face: FontFace| TtfFont::open(&self.fonts_folder.join(family.file_name(face)), pixel_size);
            let primary = open(key.face).unwrap_or_else(|error| panic!("{error}"));
            let fallback = key.face.fallback().and_then(|face| open(face).ok());
            let tables = &self.tables[&(family.for_face(key.face), key.face)];
            let metrics = LineMetrics::for_size(tables, pixel_size, self.dpi_scale);
            self.fonts.insert(
                key,
                LoadedFont {
                    face: key.face,
                    uses_love_advances: family.for_face(key.face) == FontFamily::Roboto,
                    pixel_size,
                    metrics,
                    primary,
                    fallback,
                    advances: HashMap::new(),
                    glyphs: HashMap::new(),
                },
            );
        }
        self.fonts.get_mut(&key).expect("inserted above")
    }

    /// `Font:getHeight()` / `getAscent()` / `getBaseline()`.
    pub fn line_metrics(&mut self, key: FontKey) -> LineMetrics {
        self.font(key).metrics
    }

    pub fn height(&mut self, key: FontKey) -> f32 {
        self.line_metrics(key).height
    }

    /// `Font:getWidth(text)`.
    pub fn width(&mut self, key: FontKey, text: &str) -> f32 {
        let dpi_scale = self.dpi_scale;
        let font = self.font(key);
        text_width(text, &mut |character| font.advance(character, dpi_scale))
    }

    /// `Font:getWrap(text, limit)`: the widest line and the lines.
    pub fn wrap(&mut self, key: FontKey, text: &str, wrap_limit: f32) -> (f32, Vec<WrappedLine>) {
        let dpi_scale = self.dpi_scale;
        let font = self.font(key);
        let lines = wrap_text(text, wrap_limit, &mut |character| font.advance(character, dpi_scale));
        let widest = lines.iter().map(|line| line.width).fold(0.0, f32::max);
        (widest, lines)
    }

    /// `love.graphics.print(text, x, y)` with the draw list's current colour and transform.
    pub fn print(&mut self, list: &mut DrawList, key: FontKey, text: &str, x: f32, y: f32) {
        let dpi_scale = self.dpi_scale;
        let font = self.font(key);
        let metrics = font.metrics;
        let placed = layout_print(text, &metrics, &mut |character| font.advance(character, dpi_scale));
        self.draw_glyphs(list, key, &placed, x, y);
    }

    /// `love.graphics.printf(text, x, y, limit, align)`.
    #[allow(clippy::too_many_arguments)]
    pub fn printf(&mut self, list: &mut DrawList, key: FontKey, text: &str, x: f32, y: f32, limit: f32, align: Align) {
        let dpi_scale = self.dpi_scale;
        let font = self.font(key);
        let metrics = font.metrics;
        let placed = layout_printf(text, limit, align, &metrics, &mut |character| font.advance(character, dpi_scale));
        self.draw_glyphs(list, key, &placed, x, y);
    }

    fn draw_glyphs(&mut self, list: &mut DrawList, key: FontKey, placed: &[PlacedGlyph], x: f32, y: f32) {
        let dpi_scale = self.dpi_scale;
        let baseline = self.font(key).metrics.baseline();
        let color = list.color();
        for glyph in placed {
            let Some(atlas_glyph) = self.atlas_glyph(key, glyph.character) else { continue };
            let page = &self.pages[atlas_glyph.page];
            let size = ATLAS_PAGE_SIZE as f32;
            let uv = [
                atlas_glyph.x as f32 / size,
                atlas_glyph.y as f32 / size,
                (atlas_glyph.x + atlas_glyph.width) as f32 / size,
                (atlas_glyph.y + atlas_glyph.height) as f32 / size,
            ];
            let rectangle = [
                x + glyph.x + atlas_glyph.bearing_x as f32 / dpi_scale,
                y + glyph.y + baseline - atlas_glyph.bearing_y as f32 / dpi_scale,
                atlas_glyph.width as f32 / dpi_scale,
                atlas_glyph.height as f32 / dpi_scale,
            ];
            list.add_textured_quad(page.texture, rectangle, uv, color);
        }
    }

    /// Rasterizes a glyph into the atlas the first time it is drawn.
    fn atlas_glyph(&mut self, key: FontKey, character: char) -> Option<AtlasGlyph> {
        if let Some(cached) = self.font(key).glyphs.get(&character) {
            return *cached;
        }
        let font = self.font(key);
        let source = font.font_for(character);
        let (bearing_x, bearing_y, _) = source.glyph_metrics(character);
        let image = source.glyph_image(character);
        let placed = image.and_then(|image| {
            let (page, x, y) = self.place_in_atlas(image.width, image.height)?;
            self.pages[page].copy_in(x, y, &image);
            Some(AtlasGlyph { page, x, y, width: image.width, height: image.height, bearing_x, bearing_y })
        });
        self.font(key).glyphs.insert(character, placed);
        placed
    }

    fn place_in_atlas(&mut self, width: u32, height: u32) -> Option<(usize, u32, u32)> {
        if let Some(last) = self.pages.last_mut()
            && let Some((x, y)) = last.allocate(width, height)
        {
            return Some((self.pages.len() - 1, x, y));
        }
        let mut page = AtlasPage::new();
        let (x, y) = page.allocate(width, height)?;
        self.pages.push(page);
        Some((self.pages.len() - 1, x, y))
    }

    /// Sends new glyphs to the GPU. Call once per frame before rendering.
    pub fn flush(&mut self, renderer: &mut GpuRenderer) -> RenderResult<()> {
        for page in &mut self.pages {
            if !page.created_on_gpu {
                renderer.create_texture_with_id(page.texture, ATLAS_PAGE_SIZE, ATLAS_PAGE_SIZE, &page.pixels, false)?;
                page.created_on_gpu = true;
                page.dirty_rows = None;
                continue;
            }
            if let Some((start, end)) = page.dirty_rows.take() {
                let range = (start * ATLAS_PAGE_SIZE * 4) as usize..(end * ATLAS_PAGE_SIZE * 4) as usize;
                renderer.upload_texture_region(
                    page.texture,
                    0,
                    start,
                    ATLAS_PAGE_SIZE,
                    end - start,
                    &page.pixels[range],
                )?;
            }
        }
        Ok(())
    }

    /// Physical glyph data for tests: `(advance, bearing_x, bearing_y, width, height)`.
    pub fn glyph_data(&mut self, key: FontKey, character: char) -> (i32, i32, i32, u32, u32) {
        let font = self.font(key);
        let source = font.primary_for_tests();
        let (bearing_x, bearing_y, advance) = source.glyph_metrics(character);
        let (width, height) = source.glyph_image(character).map_or((0, 0), |image| (image.width, image.height));
        (advance, bearing_x, bearing_y, width, height)
    }
}

impl LoadedFont {
    fn primary_for_tests(&self) -> &TtfFont {
        &self.primary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts_folder() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts")
    }

    #[test]
    fn every_family_has_every_face_as_a_readable_font() {
        for family in FontFamily::ALL {
            for face in FontFace::TEXT_FACES.into_iter().chain([FontFace::Icons, FontFace::Symbols]) {
                let tables = read_font_tables(&fonts_folder(), family, face);
                assert!(tables.is_ok(), "{family:?} {face:?}: {:?}", tables.err());
            }
        }
    }

    #[test]
    fn families_round_trip_through_settings_and_borrow_missing_weights() {
        for family in FontFamily::ALL {
            assert_eq!(FontFamily::from_setting_value(family.setting_value()), Some(family));
        }
        assert_eq!(FontFamily::from_setting_value("comic_sans"), None);
        assert_eq!(FontFamily::default(), FontFamily::Roboto);
        assert_eq!(FontFamily::Merriweather.file_name(FontFace::Medium), "Merriweather-Bold.ttf");
        assert_eq!(FontFamily::OpenSans.file_name(FontFace::Medium), "OpenSans-SemiBold.ttf");
        assert_eq!(FontFamily::Lato.file_name(FontFace::Icons), "MaterialIcons-Regular.ttf");
        // LÖVE's Roboto advances only ever apply to Roboto faces.
        assert_eq!(FontFamily::Lato.for_face(FontFace::Regular), FontFamily::Lato);
        assert_eq!(FontFamily::Lato.for_face(FontFace::Symbols), FontFamily::Roboto);
    }
}
