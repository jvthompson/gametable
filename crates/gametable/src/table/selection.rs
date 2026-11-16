//! Box-selection geometry and keeping moved items on the table. Port of
//! `src/ui/table/selection.lua` (pure, so it is unit-tested directly).

use gametable_core::geometry::{TABLE_PAN_MARGIN_FRACTION, WORLD_HEIGHT, WORLD_WIDTH};

use crate::ui::context::Rect;

/// How far from the reachable table's edge an item must stay.
pub const EDGE_MARGIN: f32 = 10.0;

/// A hit-testable shape of something drawn on the table (the parts of a hit item that box
/// selection looks at).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HitShape {
    /// The unrotated box.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Centre and half extents of a rotated box (`angle` in radians).
    pub center_x: f32,
    pub center_y: f32,
    pub half_width: f32,
    pub half_height: f32,
    pub angle: f32,
    /// A circle's radius around the centre.
    pub circle: Option<f32>,
}

impl HitShape {
    pub fn rect(x: f32, y: f32, width: f32, height: f32) -> HitShape {
        HitShape {
            x,
            y,
            width,
            height,
            center_x: x + width / 2.0,
            center_y: y + height / 2.0,
            half_width: width / 2.0,
            half_height: height / 2.0,
            ..HitShape::default()
        }
    }

    pub fn circle(center_x: f32, center_y: f32, radius: f32) -> HitShape {
        HitShape {
            x: center_x - radius,
            y: center_y - radius,
            width: 2.0 * radius,
            height: 2.0 * radius,
            center_x,
            center_y,
            half_width: radius,
            half_height: radius,
            angle: 0.0,
            circle: Some(radius),
        }
    }

    /// A box of half extents turned by `angle` around its centre.
    pub fn rotated(center_x: f32, center_y: f32, half_width: f32, half_height: f32, angle: f32) -> HitShape {
        HitShape {
            x: center_x - half_width,
            y: center_y - half_height,
            width: 2.0 * half_width,
            height: 2.0 * half_height,
            center_x,
            center_y,
            half_width,
            half_height,
            angle,
            circle: None,
        }
    }

    /// `V.hitTest` (without the mirroring the caller applies first): rotated boxes test in their
    /// own frame, then circles, then the plain box.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        if self.angle != 0.0 {
            let (dx, dy) = (x - self.center_x, y - self.center_y);
            let (sine, cosine) = (-self.angle).sin_cos();
            let (local_x, local_y) = (dx * cosine - dy * sine, dx * sine + dy * cosine);
            return local_x.abs() <= self.half_width && local_y.abs() <= self.half_height;
        }
        if let Some(radius) = self.circle {
            let (dx, dy) = (x - self.center_x, y - self.center_y);
            return dx * dx + dy * dy <= radius * radius;
        }
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

/// `S.rectFromPoints`: the normalized rect between two corners.
pub fn rect_from_points(x1: f32, y1: f32, x2: f32, y2: f32) -> Rect {
    Rect::new(x1.min(x2), y1.min(y2), (x2 - x1).abs(), (y2 - y1).abs())
}

fn rects_overlap(x: f32, y: f32, width: f32, height: f32, rect: &Rect) -> bool {
    x < rect.x + rect.width && x + width > rect.x && y < rect.y + rect.height && y + height > rect.y
}

/// `S.boxHits`: circles test their closest point, rotated boxes their rotated bounding box, the
/// rest their box.
pub fn box_hits(rect: &Rect, shape: &HitShape) -> bool {
    if let Some(radius) = shape.circle {
        let nearest_x = shape.center_x.clamp(rect.x, rect.x + rect.width);
        let nearest_y = shape.center_y.clamp(rect.y, rect.y + rect.height);
        let (dx, dy) = (shape.center_x - nearest_x, shape.center_y - nearest_y);
        return dx * dx + dy * dy <= radius * radius;
    }
    if shape.angle != 0.0 && shape.half_width != 0.0 {
        let (cosine, sine) = (shape.angle.cos().abs(), shape.angle.sin().abs());
        let extent_x = shape.half_width * cosine + shape.half_height * sine;
        let extent_y = shape.half_width * sine + shape.half_height * cosine;
        return rects_overlap(
            shape.center_x - extent_x,
            shape.center_y - extent_y,
            2.0 * extent_x,
            2.0 * extent_y,
            rect,
        );
    }
    rects_overlap(shape.x, shape.y, shape.width, shape.height, rect)
}

/// `S.halfExtents(w, h, turns)`: odd quarter turns swap width and height.
pub fn half_extents(width: f32, height: f32, turns: f32) -> (f32, f32) {
    let quarter_turns = ((turns * 4.0 + 0.5).floor() as i64).rem_euclid(2);
    if quarter_turns == 1 { (height / 2.0, width / 2.0) } else { (width / 2.0, height / 2.0) }
}

/// `S.clampWorldCenter`: keeps an item with these half extents fully on the reachable table.
pub fn clamp_world_center(world_x: f32, world_y: f32, half_width: f32, half_height: f32) -> (f32, f32) {
    let margin = TABLE_PAN_MARGIN_FRACTION as f32;
    let clamp = |value: f32, size: f32, half: f32| {
        let low = -margin * size + EDGE_MARGIN + half;
        let high = size * (1.0 + margin) - EDGE_MARGIN - half;
        if low > high { size / 2.0 } else { value.clamp(low, high) }
    };
    (clamp(world_x, WORLD_WIDTH as f32, half_width), clamp(world_y, WORLD_HEIGHT as f32, half_height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_and_box_selection() {
        let rotated = HitShape::rotated(100.0, 100.0, 35.0, 50.0, std::f32::consts::FRAC_PI_2);
        assert!(rotated.contains(145.0, 100.0));
        assert!(!rotated.contains(100.0, 145.0));
        let circle = HitShape::circle(0.0, 0.0, 10.0);
        assert!(circle.contains(6.0, 6.0) && !circle.contains(8.0, 8.0));
        let selection = rect_from_points(20.0, 20.0, 5.0, 5.0);
        assert_eq!(selection, Rect::new(5.0, 5.0, 15.0, 15.0));
        assert!(box_hits(&selection, &circle));
        assert!(box_hits(&rect_from_points(140.0, 95.0, 160.0, 105.0), &rotated));
        assert!(!box_hits(&rect_from_points(140.0, 140.0, 160.0, 160.0), &rotated));
    }

    #[test]
    fn extents_and_clamping() {
        assert_eq!(half_extents(70.0, 100.0, 0.25), (50.0, 35.0));
        assert_eq!(half_extents(70.0, 100.0, 0.5), (35.0, 50.0));
        assert_eq!(half_extents(70.0, 100.0, -0.25), (50.0, 35.0));
        assert_eq!(clamp_world_center(-5000.0, 99999.0, 35.0, 50.0), (-1280.0 + 10.0 + 35.0, 2160.0 - 10.0 - 50.0));
        assert_eq!(clamp_world_center(100.0, 100.0, 1e6, 1e6), (1280.0, 720.0));
    }
}
