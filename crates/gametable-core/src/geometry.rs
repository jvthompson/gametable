//! Table-space geometry. Port of `src/core/geometry.lua`.
//!
//! Card and widget positions are stored as fractions of the table (0..1 on each axis). The table
//! itself is drawn as a 2560x1440 world, and players in far seats see it mirrored.

pub const WORLD_WIDTH: f64 = 2560.0;
pub const WORLD_HEIGHT: f64 = 1440.0;

/// How far past the table edge (as a fraction of the table) things may be dragged.
pub const TABLE_PAN_MARGIN_FRACTION: f64 = 0.5;

/// Offset that centres a world of the given size inside a viewport.
pub fn world_centering_offset(
    world_width: f64,
    world_height: f64,
    viewport_width: f64,
    viewport_height: f64,
) -> (f64, f64) {
    ((viewport_width - world_width) / 2.0, (viewport_height - world_height) / 2.0)
}

/// Keeps a card's centre within the table plus the pan margin. `margin` defaults to 10 in Lua.
pub fn clamp_card_center_y(proposed_center_y: f64, table_height: f64, card_height: f64, margin: f64) -> f64 {
    let half_card = card_height / 2.0;
    let table_margin = table_height * TABLE_PAN_MARGIN_FRACTION;
    let lower_bound = margin + half_card - table_margin;
    let upper_bound = table_height - margin - half_card + table_margin;
    if lower_bound > upper_bound {
        return table_height / 2.0;
    }
    proposed_center_y.clamp(lower_bound, upper_bound)
}

/// Converts stored table fractions to pixels in the local player's (possibly mirrored) view.
pub fn canonical_to_local_pixel(
    fraction_x: f64,
    fraction_y: f64,
    table_width: f64,
    table_height: f64,
    is_mirrored: bool,
) -> (f64, f64) {
    let (unmirrored_x, unmirrored_y) =
        if is_mirrored { (1.0 - fraction_x, 1.0 - fraction_y) } else { (fraction_x, fraction_y) };
    (unmirrored_x * table_width, unmirrored_y * table_height)
}

/// Converts local-view pixels back to stored table fractions, clamped to the pan margin.
pub fn local_pixel_to_canonical(
    pixel_x: f64,
    pixel_y: f64,
    table_width: f64,
    table_height: f64,
    is_mirrored: bool,
) -> (f64, f64) {
    let margin = TABLE_PAN_MARGIN_FRACTION;
    let fraction_x = (pixel_x / table_width).clamp(-margin, 1.0 + margin);
    let fraction_y = (pixel_y / table_height).clamp(-margin, 1.0 + margin);
    if is_mirrored { (1.0 - fraction_x, 1.0 - fraction_y) } else { (fraction_x, fraction_y) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrored_views_flip_both_axes_and_round_trip() {
        let (pixel_x, pixel_y) = canonical_to_local_pixel(0.25, 0.75, 2560.0, 1440.0, true);
        assert_eq!((pixel_x, pixel_y), (1920.0, 360.0));
        assert_eq!(local_pixel_to_canonical(pixel_x, pixel_y, 2560.0, 1440.0, true), (0.25, 0.75));
    }

    #[test]
    fn clamps_to_the_pan_margin() {
        assert_eq!(local_pixel_to_canonical(-99999.0, 99999.0, 100.0, 100.0, false), (-0.5, 1.5));
        assert_eq!(clamp_card_center_y(-5000.0, 1440.0, 100.0, 10.0), 10.0 + 50.0 - 720.0);
    }
}
