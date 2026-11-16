//! ARGB integer colour helpers. Port of `src/core/color.lua`.
//!
//! CardTable stores every colour as a 32-bit ARGB integer (`0xAARRGGBB`), which is also how colours
//! appear in JSON files and on the wire.

/// A colour packed as `0xAARRGGBB`.
pub type ArgbColor = u32;

/// The twelve swatches offered for counters, tokens and players, in display order.
pub const BOARD_WIDGET_COLOR_PALETTE: [ArgbColor; 12] = [
    0xFF455A64, // blue grey (default background)
    0xFFFFFFFF, // white (default text)
    0xFF000000, // black
    0xFFD32F2F, // red
    0xFFF57C00, // orange
    0xFFFBC02D, // yellow
    0xFF388E3C, // green
    0xFF1976D2, // blue
    0xFF7B1FA2, // purple
    0xFF5D4037, // brown
    0xFF9E9E9E, // grey
    0xFFEC407A, // pink
];

/// Six uppercase hex digits for the RGB part of a colour, e.g. `0xFFD32F2F` -> `"D32F2F"`.
pub fn hex_from_color(argb: ArgbColor) -> String {
    format!("{:06X}", argb & 0x00FF_FFFF)
}

/// Parses `"#RRGGBB"` or `"RRGGBB"` (surrounding whitespace allowed) into an opaque colour.
/// Returns `None` for anything else.
pub fn color_from_hex(input: &str) -> Option<ArgbColor> {
    let trimmed = crate::util::trim_lua_whitespace(input);
    // Lua's gsub("#", "", 1) removes the first '#' wherever it is, not only a leading one.
    let without_first_hash = match trimmed.find('#') {
        Some(hash_index) => format!("{}{}", &trimmed[..hash_index], &trimmed[hash_index + 1..]),
        None => trimmed.to_string(),
    };
    let is_six_hex_digits =
        without_first_hash.len() == 6 && without_first_hash.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !is_six_hex_digits {
        return None;
    }
    let rgb = u32::from_str_radix(&without_first_hash, 16).ok()?;
    Some(0xFF00_0000 | rgb)
}

/// Floating-point colour channels in 0..=1, the form renderers and the colour picker work with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RgbaChannels {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}

/// Splits a colour into 0..=1 channels, multiplying alpha by `alpha_multiplier`.
pub fn to_rgba_channels(argb: ArgbColor, alpha_multiplier: f64) -> RgbaChannels {
    let channel = |shift: u32| ((argb >> shift) & 0xFF) as f64 / 255.0;
    RgbaChannels { red: channel(16), green: channel(8), blue: channel(0), alpha: channel(24) * alpha_multiplier }
}

/// Packs 0..=1 channels into a colour, clamping and rounding each to the nearest step.
pub fn from_rgba_channels(red: f64, green: f64, blue: f64, alpha: f64) -> ArgbColor {
    let to_byte = |value: f64| (value.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u32;
    (to_byte(alpha) << 24) | (to_byte(red) << 16) | (to_byte(green) << 8) | to_byte(blue)
}

/// `"#D32F2F"` -> `(r, g, b)` in 0..=1, or `None` when the text is not a colour.
pub fn parse_hex_to_rgb_channels(hex: &str) -> Option<(f64, f64, f64)> {
    let channels = to_rgba_channels(color_from_hex(hex)?, 1.0);
    Some((channels.red, channels.green, channels.blue))
}

/// RGB (0..=1) to hue, saturation, lightness (all 0..=1).
pub fn rgb_to_hsl(red: f64, green: f64, blue: f64) -> (f64, f64, f64) {
    let maximum = red.max(green).max(blue);
    let minimum = red.min(green).min(blue);
    let lightness = (maximum + minimum) / 2.0;
    if maximum == minimum {
        return (0.0, 0.0, lightness);
    }
    let range = maximum - minimum;
    let saturation = if lightness > 0.5 { range / (2.0 - maximum - minimum) } else { range / (maximum + minimum) };
    let hue_sixths = if maximum == red {
        (green - blue) / range + if green < blue { 6.0 } else { 0.0 }
    } else if maximum == green {
        (blue - red) / range + 2.0
    } else {
        (red - green) / range + 4.0
    };
    (hue_sixths / 6.0, saturation, lightness)
}

fn hue_to_rgb_channel(p: f64, q: f64, mut t: f64) -> f64 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

/// Hue, saturation, lightness (0..=1) to RGB (0..=1).
pub fn hsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> (f64, f64, f64) {
    if saturation == 0.0 {
        return (lightness, lightness, lightness);
    }
    let q =
        if lightness < 0.5 { lightness * (1.0 + saturation) } else { lightness + saturation - lightness * saturation };
    let p = 2.0 * lightness - q;
    (
        hue_to_rgb_channel(p, q, hue + 1.0 / 3.0),
        hue_to_rgb_channel(p, q, hue),
        hue_to_rgb_channel(p, q, hue - 1.0 / 3.0),
    )
}

/// Hue, saturation, value (0..=1) to RGB (0..=1).
pub fn hsv_to_rgb(hue: f64, saturation: f64, value: f64) -> (f64, f64, f64) {
    let sector = (hue * 6.0).floor();
    let fraction = hue * 6.0 - sector;
    let p = value * (1.0 - saturation);
    let q = value * (1.0 - fraction * saturation);
    let t = value * (1.0 - (1.0 - fraction) * saturation);
    match (sector as i64).rem_euclid(6) {
        0 => (value, t, p),
        1 => (q, value, p),
        2 => (p, value, t),
        3 => (p, q, value),
        4 => (t, p, value),
        _ => (value, p, q),
    }
}

/// RGB (0..=1) to hue, saturation, value (0..=1).
pub fn rgb_to_hsv(red: f64, green: f64, blue: f64) -> (f64, f64, f64) {
    let maximum = red.max(green).max(blue);
    let minimum = red.min(green).min(blue);
    let range = maximum - minimum;
    let mut hue = 0.0;
    if range != 0.0 {
        hue = if maximum == red {
            crate::util::lua_modulo((green - blue) / range, 6.0)
        } else if maximum == green {
            (blue - red) / range + 2.0
        } else {
            (red - green) / range + 4.0
        };
        hue /= 6.0;
    }
    let saturation = if maximum == 0.0 { 0.0 } else { range / maximum };
    (hue, saturation, maximum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_between_hex_text_and_colors() {
        assert_eq!(hex_from_color(0xFFD32F2F), "D32F2F");
        assert_eq!(color_from_hex("#d32f2f"), Some(0xFFD32F2F));
        assert_eq!(color_from_hex("  D32F2F \n"), Some(0xFFD32F2F));
        // Only the first '#' is removed, wherever it is.
        assert_eq!(color_from_hex("D32#F2F"), Some(0xFFD32F2F));
        assert_eq!(color_from_hex("##D32F2F"), None);
        assert_eq!(color_from_hex("12345"), None);
        assert_eq!(color_from_hex("zzzzzz"), None);
    }

    #[test]
    fn round_trips_channels() {
        let channels = to_rgba_channels(0x80FF0000, 1.0);
        assert_eq!((channels.red, channels.green, channels.blue), (1.0, 0.0, 0.0));
        assert_eq!(from_rgba_channels(channels.red, channels.green, channels.blue, channels.alpha), 0x80FF0000);
    }

    #[test]
    fn hsl_and_hsv_round_trip() {
        let (h, s, l) = rgb_to_hsl(0.2, 0.4, 0.6);
        let (r, g, b) = hsl_to_rgb(h, s, l);
        assert!((r - 0.2).abs() < 1e-12 && (g - 0.4).abs() < 1e-12 && (b - 0.6).abs() < 1e-12);
        let (h, s, v) = rgb_to_hsv(0.9, 0.1, 0.3);
        let (r, g, b) = hsv_to_rgb(h, s, v);
        assert!((r - 0.9).abs() < 1e-12 && (g - 0.1).abs() < 1e-12 && (b - 0.3).abs() < 1e-12);
    }
}
