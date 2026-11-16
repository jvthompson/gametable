//! Card drawing. Port of `src/ui/table/card_render.lua` (`card_face_widget.dart`,
//! `card_back_widget.dart`).
//!
//! Cards are drawn at their base size (70×100) under a transform, so callers position them by
//! centre with a scale and rotation.

use gametable_core::color::parse_hex_to_rgb_channels;
use gametable_core::models::{CardBackDefinition, CardDefinition, CardOrientation};

use crate::app::Frame;
use crate::data::image_resolver::resolve_card_back_image_path;
use crate::graphics::draw_list::{Color, FragmentProgram, Quad};
use crate::graphics::image_cache::ImageStatus;
use crate::graphics::tessellation::DrawMode;
use crate::graphics::text::FontKey;
use crate::ui::theme::{self, Weight};

pub const CARD_WIDTH: f32 = 70.0;
pub const CARD_HEIGHT: f32 = 100.0;
pub const CARD_RADIUS: f32 = 3.5;
/// Extra room around a card for a pile's box.
pub const PILE_EXTRA: f32 = 24.0;

const DEFAULT_CARD_COLOR: &str = "#9E9E9E";
const PREVIEW_GAP: f32 = 16.0;

/// `R.orientationTurns(o)`: right is a quarter turn clockwise, left counter-clockwise.
pub fn orientation_turns(orientation: CardOrientation) -> f32 {
    match orientation {
        CardOrientation::Right => 0.25,
        CardOrientation::Left => -0.25,
        CardOrientation::Portrait => 0.0,
    }
}

/// `R.radiusFor(shortSide)`: the card corner radius scaled to an image's short side.
pub fn radius_for(short_side: f32) -> f32 {
    CARD_RADIUS / CARD_WIDTH * short_side
}

fn suit_glyph(suit: &str) -> Option<&'static str> {
    match suit.to_lowercase().as_str() {
        "hearts" => Some("♥"),
        "diamonds" => Some("♦"),
        "clubs" => Some("♣"),
        "spades" => Some("♠"),
        _ => None,
    }
}

/// Which way the shown side of a card is oriented (`R.shownOrientation`).
pub fn shown_orientation(
    definition: Option<&CardDefinition>,
    showing_back: bool,
    card_backs: &[CardBackDefinition],
    scenario_card_back_id: Option<&str>,
) -> CardOrientation {
    if showing_back {
        return resolve_card_back_image_path(card_backs, definition, scenario_card_back_id).orientation;
    }
    definition.map_or(CardOrientation::Portrait, |definition| definition.orientation)
}

/// One card to draw (`R.draw(opts)`).
#[derive(Clone, Copy)]
pub struct CardDraw<'a> {
    pub definition: Option<&'a CardDefinition>,
    pub face_up: bool,
    pub center_x: f32,
    pub center_y: f32,
    pub scale: f32,
    /// Radians.
    pub angle: f32,
    /// The owner's colour around the edge.
    pub border: Option<Color>,
    pub alpha: f32,
    pub card_backs: &'a [CardBackDefinition],
    pub scenario_card_back_id: Option<&'a str>,
    /// Show the face even when the card is face down (search results).
    pub force_face_up: bool,
    /// A colour laid over the card (the selection tint).
    pub tint: Option<Color>,
}

impl<'a> CardDraw<'a> {
    pub fn new(
        definition: Option<&'a CardDefinition>,
        face_up: bool,
        center_x: f32,
        center_y: f32,
        card_backs: &'a [CardBackDefinition],
        scenario_card_back_id: Option<&'a str>,
    ) -> CardDraw<'a> {
        CardDraw {
            definition,
            face_up,
            center_x,
            center_y,
            scale: 1.0,
            angle: 0.0,
            border: None,
            alpha: 1.0,
            card_backs,
            scenario_card_back_id,
            force_face_up: false,
            tint: None,
        }
    }

    pub fn scale(self, scale: f32) -> Self {
        CardDraw { scale, ..self }
    }

    pub fn angle(self, angle: f32) -> Self {
        CardDraw { angle, ..self }
    }

    pub fn border(self, border: Option<Color>) -> Self {
        CardDraw { border, ..self }
    }

    pub fn alpha(self, alpha: f32) -> Self {
        CardDraw { alpha, ..self }
    }

    pub fn tint(self, tint: Option<Color>) -> Self {
        CardDraw { tint, ..self }
    }

    pub fn force_face_up(self, force_face_up: bool) -> Self {
        CardDraw { force_face_up, ..self }
    }
}

/// One face of a card definition in a preview (`R.definitionFaces` entries).
#[derive(Clone, Copy)]
pub struct PreviewFace<'a> {
    pub definition: Option<&'a CardDefinition>,
    pub face_up: bool,
    pub orientation: CardOrientation,
}

/// `R.definitionFaces`: the front, then the back when the card has its own.
pub fn definition_faces<'a>(
    definition: &'a CardDefinition,
    card_backs: &[CardBackDefinition],
    scenario_card_back_id: Option<&str>,
) -> Vec<PreviewFace<'a>> {
    let mut faces =
        vec![PreviewFace { definition: Some(definition), face_up: true, orientation: definition.orientation }];
    if definition.card_back_id.is_some() || (definition.is_scenario_card && scenario_card_back_id.is_some()) {
        let back = resolve_card_back_image_path(card_backs, Some(definition), scenario_card_back_id);
        faces.push(PreviewFace { definition: Some(definition), face_up: false, orientation: back.orientation });
    }
    faces
}

impl Frame<'_> {
    /// `love.graphics.print(text, x, y, angle, sx, sy, ox, oy)` with the current list colour.
    #[allow(clippy::too_many_arguments)]
    pub fn print_transformed(
        &mut self,
        text: &str,
        font: FontKey,
        x: f32,
        y: f32,
        angle: f32,
        scale_x: f32,
        scale_y: f32,
        origin_x: f32,
        origin_y: f32,
    ) {
        let list = self.ui.list();
        list.push();
        list.translate(x, y);
        if angle != 0.0 {
            list.rotate(angle);
        }
        list.scale(scale_x, scale_y);
        list.translate(-origin_x, -origin_y);
        self.fonts.print(self.ui.list(), font, text, 0.0, 0.0);
        self.ui.list().pop();
    }

    /// `scaledPrint`: text under the card transform, from a font sized for the current scale so
    /// it stays crisp.
    #[allow(clippy::too_many_arguments)]
    fn card_scaled_print(
        &mut self,
        text_scale: f32,
        text: &str,
        size: f32,
        weight: Weight,
        x: f32,
        y: f32,
        origin: (f32, f32),
        extra_scale: f32,
    ) {
        let font = theme::font(size * text_scale, weight);
        let scale = extra_scale / text_scale;
        self.print_transformed(text, font, x, y, 0.0, scale, scale, origin.0 * text_scale, origin.1 * text_scale);
    }

    fn card_shadow(&mut self, alpha: f32) {
        let list = self.ui.list();
        list.set_color(Color::rgba(0.0, 0.0, 0.0, 0.18 * alpha));
        list.rectangle(
            DrawMode::Fill,
            0.5,
            1.5,
            CARD_WIDTH + 1.0,
            CARD_HEIGHT + 1.0,
            CARD_RADIUS + 1.0,
            CARD_RADIUS + 1.0,
        );
        list.set_color(Color::rgba(0.0, 0.0, 0.0, 0.22 * alpha));
        list.rectangle(DrawMode::Fill, 1.0, 1.0, CARD_WIDTH, CARD_HEIGHT, CARD_RADIUS, CARD_RADIUS);
    }

    fn card_thin_border(&mut self, alpha: f32) {
        let list = self.ui.list();
        list.set_color(Color::rgba(0.0, 0.0, 0.0, 0.26 * alpha));
        list.set_line_width(1.0);
        list.rectangle(DrawMode::Line, 0.5, 0.5, CARD_WIDTH - 1.0, CARD_HEIGHT - 1.0, CARD_RADIUS, CARD_RADIUS);
    }

    /// Draws `path`'s image cover-fit into the base card rect with rounded corners; false when it
    /// isn't loaded.
    fn card_image_cover(&mut self, path: &str) -> bool {
        let Some(texture) = self.images.get(path) else { return false };
        let (image_width, image_height) = (texture.width, texture.height);
        let target = CARD_WIDTH / CARD_HEIGHT;
        let (quad_width, quad_height) = if image_width / image_height > target {
            (image_height * target, image_height)
        } else {
            (image_width, image_width / target)
        };
        let (quad_x, quad_y) = ((image_width - quad_width) / 2.0, (image_height - quad_height) / 2.0);
        let list = self.ui.list();
        list.set_program(FragmentProgram::RoundedImage {
            quad_rect: [
                quad_x / image_width,
                quad_y / image_height,
                (quad_x + quad_width) / image_width,
                (quad_y + quad_height) / image_height,
            ],
            size: [CARD_WIDTH, CARD_HEIGHT],
            radius: CARD_RADIUS,
        });
        let quad = Quad { x: quad_x, y: quad_y, width: quad_width, height: quad_height };
        list.draw_texture(
            texture,
            Some(quad),
            0.0,
            0.0,
            0.0,
            CARD_WIDTH / quad_width,
            CARD_HEIGHT / quad_height,
            0.0,
            0.0,
        );
        list.set_program(FragmentProgram::Plain);
        true
    }

    fn code_drawn_corner(&mut self, text_scale: f32, title: &str, color: Color, glyph: Option<&str>, alpha: f32) {
        self.ui.list().set_color(Color { alpha, ..color });
        self.card_scaled_print(text_scale, title, 15.0, Weight::Bold, 0.0, 0.0, (0.0, 0.0), 1.0);
        if let Some(glyph) = glyph {
            self.card_scaled_print(text_scale, glyph, 13.0, Weight::Regular, 0.0, 15.0, (0.0, 0.0), 1.0);
        }
    }

    /// A face drawn from the definition's text (no image, or the image failed).
    fn code_drawn_face(&mut self, text_scale: f32, definition: &CardDefinition, alpha: f32) {
        self.card_shadow(alpha);
        let list = self.ui.list();
        list.set_color(Color::rgba(1.0, 1.0, 1.0, alpha));
        list.rectangle(DrawMode::Fill, 0.0, 0.0, CARD_WIDTH, CARD_HEIGHT, CARD_RADIUS, CARD_RADIUS);
        self.card_thin_border(alpha);
        let (red, green, blue) =
            parse_hex_to_rgb_channels(definition.color_hex.as_deref().unwrap_or(DEFAULT_CARD_COLOR))
                .or_else(|| parse_hex_to_rgb_channels(DEFAULT_CARD_COLOR))
                .unwrap_or((0.62, 0.62, 0.62));
        let color = Color::rgba(red as f32, green as f32, blue as f32, 1.0);
        let glyph = definition.suit.as_deref().and_then(suit_glyph);
        self.ui.list().push();
        self.ui.list().translate(4.0, 4.0);
        self.code_drawn_corner(text_scale, &definition.card_title, color, glyph, alpha);
        self.ui.list().pop();
        let center_text = glyph.unwrap_or(&definition.card_title).to_string();
        let size = if glyph.is_some() { 34.0 } else { 22.0 };
        let base = theme::font(size, Weight::Bold);
        self.ui.list().set_color(Color { alpha, ..color });
        let text_width = self.fonts.width(base, &center_text);
        let fit = ((CARD_WIDTH - 8.0) / text_width.max(1.0)).min(1.0);
        let origin = (text_width / 2.0, self.fonts.height(base) / 2.0);
        self.card_scaled_print(
            text_scale,
            &center_text,
            size,
            Weight::Bold,
            CARD_WIDTH / 2.0,
            CARD_HEIGHT / 2.0,
            origin,
            fit,
        );
        self.ui.list().push();
        self.ui.list().translate(CARD_WIDTH - 4.0, CARD_HEIGHT - 4.0);
        self.ui.list().rotate(std::f32::consts::PI);
        self.code_drawn_corner(text_scale, &definition.card_title, color, glyph, alpha);
        self.ui.list().pop();
    }

    /// The default back: blue with a crosshatch, frame and diamond.
    fn code_drawn_back(&mut self, alpha: f32) {
        self.card_shadow(alpha);
        let list = self.ui.list();
        list.set_color(Color::rgba(0x15 as f32 / 255.0, 0x65 as f32 / 255.0, 0xC0 as f32 / 255.0, alpha));
        list.rectangle(DrawMode::Fill, 0.0, 0.0, CARD_WIDTH, CARD_HEIGHT, CARD_RADIUS, CARD_RADIUS);
        self.card_thin_border(alpha);
        let (inner_x, inner_y, inner_width, inner_height) = (6.0, 6.0, CARD_WIDTH - 12.0, CARD_HEIGHT - 12.0);
        let list = self.ui.list();
        list.set_color(Color::rgba(1.0, 1.0, 1.0, 0.12 * alpha));
        list.set_line_width(1.0);
        // The crosshatch, each segment clipped to the inner rect.
        let mut offset = -inner_height;
        while offset <= inner_width {
            let (mut x1, mut y1, mut x2, mut y2) = (offset, 0.0, offset + inner_height, inner_height);
            if x1 < 0.0 {
                y1 = -x1;
                x1 = 0.0;
            }
            if x2 > inner_width {
                y2 = inner_height - (x2 - inner_width);
                x2 = inner_width;
            }
            if y2 > y1 {
                list.line(&[inner_x + x1, inner_y + y1, inner_x + x2, inner_y + y2]);
            }
            let (mut a1, mut b1, mut a2, mut b2) = (offset + inner_height, 0.0, offset, inner_height);
            if a1 > inner_width {
                b1 = a1 - inner_width;
                a1 = inner_width;
            }
            if a2 < 0.0 {
                b2 = inner_height + a2;
                a2 = 0.0;
            }
            if b2 > b1 {
                list.line(&[inner_x + a1, inner_y + b1, inner_x + a2, inner_y + b2]);
            }
            offset += 8.0;
        }
        list.set_color(Color::rgba(1.0, 1.0, 1.0, 0.24 * alpha));
        list.set_line_width(2.0);
        list.rectangle(DrawMode::Line, inner_x, inner_y, inner_width, inner_height, 3.0, 3.0);
        list.set_color(Color::rgba(1.0, 1.0, 1.0, 0.35 * alpha));
        list.set_line_width(1.5);
        let (center_x, center_y) = (CARD_WIDTH / 2.0, CARD_HEIGHT / 2.0);
        let radius = inner_width.min(inner_height) * 0.22;
        list.polygon(
            DrawMode::Line,
            &[
                center_x,
                center_y - radius,
                center_x + radius,
                center_y,
                center_x,
                center_y + radius,
                center_x - radius,
                center_y,
            ],
        );
        list.set_line_width(1.0);
    }

    /// A card that wants its own `_BACK` image that doesn't exist.
    fn missing_back(&mut self, alpha: f32) {
        self.card_shadow(alpha);
        let list = self.ui.list();
        list.set_color(Color::rgba(0xB7 as f32 / 255.0, 0x1C as f32 / 255.0, 0x1C as f32 / 255.0, alpha));
        list.rectangle(DrawMode::Fill, 0.0, 0.0, CARD_WIDTH, CARD_HEIGHT, CARD_RADIUS, CARD_RADIUS);
        self.card_thin_border(alpha);
        let font = theme::icon_font(32.0);
        let glyph = theme::glyph("warning_amber");
        let (glyph_width, glyph_height) = (self.fonts.width(font, &glyph), self.fonts.height(font));
        self.ui.list().set_color(Color::rgba(1.0, 1.0, 1.0, alpha));
        self.fonts.print(
            self.ui.list(),
            font,
            &glyph,
            (CARD_WIDTH - glyph_width) / 2.0,
            (CARD_HEIGHT - glyph_height) / 2.0,
        );
    }

    /// `imageFace(path, alpha, fallback)`: the image; a grey placeholder while it decodes; the
    /// fallback when it can't be loaded. Returns false when the fallback should be drawn.
    fn card_image_face(&mut self, path: &str, alpha: f32) -> bool {
        if self.images.get(path).is_some() {
            self.card_shadow(alpha);
            self.ui.list().set_color(Color::rgba(1.0, 1.0, 1.0, alpha));
            self.card_image_cover(path);
            self.card_thin_border(alpha);
            return true;
        }
        if self.images.status(path) == Some(ImageStatus::Loading) {
            self.card_shadow(alpha);
            let list = self.ui.list();
            list.set_color(Color::rgba(0.85, 0.85, 0.85, alpha));
            list.rectangle(DrawMode::Fill, 0.0, 0.0, CARD_WIDTH, CARD_HEIGHT, CARD_RADIUS, CARD_RADIUS);
            self.card_thin_border(alpha);
            return true;
        }
        false
    }

    /// `R.drawFaceBase`: the face in base coordinates (the caller sets up the transform).
    pub fn draw_card_face_base(&mut self, text_scale: f32, definition: &CardDefinition, alpha: f32) {
        let drawn = definition.image_path.as_deref().is_some_and(|path| self.card_image_face(path, alpha));
        if !drawn {
            self.code_drawn_face(text_scale, definition, alpha);
        }
    }

    /// `R.drawBackBase`.
    pub fn draw_card_back_base(
        &mut self,
        card_backs: &[CardBackDefinition],
        definition: Option<&CardDefinition>,
        alpha: f32,
        scenario_card_back_id: Option<&str>,
    ) {
        let back = resolve_card_back_image_path(card_backs, definition, scenario_card_back_id);
        if back.missing {
            self.missing_back(alpha);
            return;
        }
        match back.path {
            Some(path) if self.card_image_face(&path, alpha) => {}
            _ => self.code_drawn_back(alpha),
        }
    }

    /// `R.draw(opts)`.
    pub fn draw_card(&mut self, card: &CardDraw) {
        let show_face = (card.face_up || card.force_face_up) && card.definition.is_some();
        let list = self.ui.list();
        list.push();
        list.translate(card.center_x, card.center_y);
        if card.angle != 0.0 {
            list.rotate(card.angle);
        }
        list.scale(card.scale, card.scale);
        list.translate(-CARD_WIDTH / 2.0, -CARD_HEIGHT / 2.0);
        let text_scale = ((card.scale * 4.0 + 0.5).floor() / 4.0).max(0.5);
        match card.definition {
            Some(definition) if show_face => self.draw_card_face_base(text_scale, definition, card.alpha),
            _ => self.draw_card_back_base(card.card_backs, card.definition, card.alpha, card.scenario_card_back_id),
        }
        let list = self.ui.list();
        if let Some(tint) = card.tint {
            list.set_color(Color { alpha: tint.alpha * card.alpha, ..tint });
            list.rectangle(DrawMode::Fill, 0.0, 0.0, CARD_WIDTH, CARD_HEIGHT, CARD_RADIUS, CARD_RADIUS);
        }
        if let Some(border) = card.border {
            list.set_color(Color { alpha: border.alpha * card.alpha, ..border });
            list.set_line_width(2.0);
            list.rectangle(DrawMode::Line, 1.0, 1.0, CARD_WIDTH - 2.0, CARD_HEIGHT - 2.0, CARD_RADIUS, CARD_RADIUS);
            list.set_line_width(1.0);
        }
        list.pop();
    }

    /// `R.drawEmptySlot`: an empty zone's card-sized outline.
    pub fn draw_empty_card_slot(&mut self, center_x: f32, center_y: f32, scale: f32) {
        let list = self.ui.list();
        list.set_color(Color::rgba(1.0, 1.0, 1.0, 0.24));
        list.set_line_width(1.5 * scale);
        list.rectangle(
            DrawMode::Line,
            center_x - CARD_WIDTH * scale / 2.0,
            center_y - CARD_HEIGHT * scale / 2.0,
            CARD_WIDTH * scale,
            CARD_HEIGHT * scale,
            CARD_RADIUS * scale,
            CARD_RADIUS * scale,
        );
        list.set_line_width(1.0);
    }

    /// `R.drawCountBadge`: a dark circle with the count.
    pub fn draw_count_badge(&mut self, center_x: f32, center_y: f32, count: usize, scale: f32) {
        let list = self.ui.list();
        list.set_color(Color::rgba(0.0, 0.0, 0.0, 0.87));
        list.circle(DrawMode::Fill, center_x, center_y, 12.0 * scale, None);
        let font = theme::font(11.0, Weight::Regular);
        let text = count.to_string();
        let origin = (self.fonts.width(font, &text) / 2.0, self.fonts.height(font) / 2.0);
        self.ui.list().set_color(Color::WHITE);
        self.print_transformed(&text, font, center_x, center_y, 0.0, scale, scale, origin.0, origin.1);
    }

    /// `R.drawSearchBadge`: the eye shown on a zone or pile someone is searching.
    pub fn draw_search_badge(&mut self, center_x: f32, center_y: f32, scale: f32) {
        let list = self.ui.list();
        list.set_color(Color::rgba(0.0, 0.0, 0.0, 0.54));
        list.circle(DrawMode::Fill, center_x, center_y, 24.0 * scale, None);
        let font = theme::icon_font(32.0);
        let glyph = theme::glyph("visibility");
        let origin = (self.fonts.width(font, &glyph) / 2.0, self.fonts.height(font) / 2.0);
        self.ui.list().set_color(Color::WHITE);
        self.print_transformed(&glyph, font, center_x, center_y, 0.0, scale, scale, origin.0, origin.1);
    }

    /// `R.drawPreview`: big copies of the faces, stacked and centred in the half of the screen
    /// away from the mouse. One face is 70% of the screen tall, a stack 90%; at most 42% wide.
    pub fn draw_card_preview(
        &mut self,
        faces: &[PreviewFace],
        card_backs: &[CardBackDefinition],
        screen_width: f32,
        screen_height: f32,
        mouse_x: f32,
        scenario_card_back_id: Option<&str>,
    ) {
        let mut boxes = Vec::with_capacity(faces.len());
        let (mut stack_height, mut widest) = (0.0_f32, 0.0_f32);
        for face in faces {
            let turned = face.orientation != CardOrientation::Portrait;
            let (box_width, box_height) = if turned { (CARD_HEIGHT, CARD_WIDTH) } else { (CARD_WIDTH, CARD_HEIGHT) };
            boxes.push(box_height);
            stack_height += box_height;
            widest = widest.max(box_width);
        }
        let gaps = PREVIEW_GAP * (faces.len() as f32 - 1.0);
        let height_fraction = if faces.len() == 1 { 0.7 } else { 0.9 };
        let scale = ((screen_height * height_fraction - gaps) / stack_height).min(screen_width * 0.42 / widest);
        let (preview_width, preview_height) = (widest * scale, stack_height * scale + gaps);
        let target_x = if mouse_x < screen_width / 2.0 { screen_width * 0.75 } else { screen_width * 0.25 };
        let left = (target_x - preview_width / 2.0).min(screen_width - preview_width).max(0.0);
        let mut y = (screen_height / 2.0 - preview_height / 2.0).min(screen_height - preview_height).max(0.0);
        for (face, box_height) in faces.iter().zip(boxes) {
            let box_height = box_height * scale;
            let card = CardDraw::new(
                face.definition,
                face.face_up,
                left + preview_width / 2.0,
                y + box_height / 2.0,
                card_backs,
                scenario_card_back_id,
            )
            .scale(scale)
            .angle(orientation_turns(face.orientation) * 2.0 * std::f32::consts::PI);
            self.draw_card(&card);
            y += box_height + PREVIEW_GAP;
        }
    }
}
