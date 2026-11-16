//! Colours, fonts and icons. Port of `src/ui/kit/theme.lua`: Material 3 schemes seeded from
//! green (light and dark), the fixed colours the screens use, and the Material Icons codepoints.

use crate::graphics::draw_list::Color;
use crate::graphics::text::{FontFace, FontKey};

/// `"#RRGGBB"` with an alpha.
const fn hex(rgb: u32, alpha: f32) -> Color {
    Color::rgba(
        ((rgb >> 16) & 0xFF) as f32 / 255.0,
        ((rgb >> 8) & 0xFF) as f32 / 255.0,
        (rgb & 0xFF) as f32 / 255.0,
        alpha,
    )
}

/// One colour scheme (`theme.light` / `theme.dark`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub primary: Color,
    pub on_primary: Color,
    pub primary_container: Color,
    pub on_primary_container: Color,
    pub secondary_container: Color,
    pub on_secondary_container: Color,
    pub surface: Color,
    pub on_surface: Color,
    pub on_surface_variant: Color,
    pub surface_container: Color,
    pub surface_container_high: Color,
    pub surface_container_highest: Color,
    pub outline: Color,
    pub outline_variant: Color,
    pub error: Color,
    pub scrim: Color,
    pub disabled: Color,
    pub shadow: Color,
}

pub const LIGHT: Palette = Palette {
    primary: hex(0x3B6939, 1.0),
    on_primary: hex(0xFFFFFF, 1.0),
    primary_container: hex(0xBCF0B4, 1.0),
    on_primary_container: hex(0x002204, 1.0),
    secondary_container: hex(0xD7E8CD, 1.0),
    on_secondary_container: hex(0x121F0E, 1.0),
    surface: hex(0xF8FBF1, 1.0),
    on_surface: hex(0x191D17, 1.0),
    on_surface_variant: hex(0x424940, 1.0),
    surface_container: hex(0xECEFE5, 1.0),
    surface_container_high: hex(0xE7E9E0, 1.0),
    surface_container_highest: hex(0xE1E4DA, 1.0),
    outline: hex(0x72796F, 1.0),
    outline_variant: hex(0xC2C9BD, 1.0),
    error: hex(0xBA1A1A, 1.0),
    scrim: hex(0x000000, 0.32),
    disabled: hex(0x191D17, 0.38),
    shadow: hex(0x000000, 0.35),
};

pub const DARK: Palette = Palette {
    primary: hex(0xA1D39A, 1.0),
    on_primary: hex(0x0A390F, 1.0),
    primary_container: hex(0x235024, 1.0),
    on_primary_container: hex(0xBCF0B4, 1.0),
    secondary_container: hex(0x3B4B37, 1.0),
    on_secondary_container: hex(0xD7E8CD, 1.0),
    surface: hex(0x11140F, 1.0),
    on_surface: hex(0xE1E4DA, 1.0),
    on_surface_variant: hex(0xC2C9BD, 1.0),
    surface_container: hex(0x1D211B, 1.0),
    surface_container_high: hex(0x282B25, 1.0),
    surface_container_highest: hex(0x32362F, 1.0),
    outline: hex(0x8C9388, 1.0),
    outline_variant: hex(0x424940, 1.0),
    error: hex(0xFFB4AB, 1.0),
    scrim: hex(0x000000, 0.5),
    disabled: hex(0xE1E4DA, 0.38),
    shadow: hex(0x000000, 0.6),
};

/// Colours used regardless of the theme (`theme.fixed`).
pub mod fixed {
    use super::hex;
    use crate::graphics::draw_list::Color;

    pub const OVERLAY_PANEL: Color = hex(0x262626, 1.0);
    pub const TITLE_BAR: Color = hex(0x1B2430, 1.0);
    pub const TITLE_BAR_FOREGROUND: Color = Color::rgba(1.0, 1.0, 1.0, 0.7);
    pub const WHITE: Color = Color::rgba(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Color = Color::rgba(0.0, 0.0, 0.0, 1.0);
    pub const ORANGE: Color = hex(0xFF9800, 1.0);
    pub const BLUE: Color = hex(0x2196F3, 1.0);
    pub const BLUE_ACCENT: Color = hex(0x448AFF, 1.0);
    pub const RED: Color = hex(0xF44336, 1.0);
    pub const GREEN: Color = hex(0x4CAF50, 1.0);
    pub const GREEN_DARK: Color = hex(0x43A047, 1.0);
    pub const RED_ACCENT: Color = hex(0xFF5252, 1.0);
    pub const CARD_BACK_BLUE: Color = hex(0x1565C0, 1.0);
    pub const MISSING_BACK: Color = hex(0xB71C1C, 1.0);
}

/// `theme.alpha(c, a)`.
pub fn with_alpha(color: Color, factor: f32) -> Color {
    color.with_alpha_multiplied(factor)
}

/// `theme.mix(a, b, t)`.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::rgba(
        a.red + (b.red - a.red) * t,
        a.green + (b.green - a.green) * t,
        a.blue + (b.blue - a.blue) * t,
        a.alpha + (b.alpha - a.alpha) * t,
    )
}

/// `theme.isLight(c)`: relative luminance above 0.5 (pick dark text on it).
pub fn is_light(color: Color) -> bool {
    let linear = |channel: f32| {
        if channel <= 0.03928 { channel / 12.92 } else { ((channel + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(color.red) + 0.7152 * linear(color.green) + 0.0722 * linear(color.blue) > 0.5
}

/// Font weights `theme.font` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Weight {
    #[default]
    Regular,
    Medium,
    Bold,
}

/// `theme.font(size, weight)`.
pub fn font(size: f32, weight: Weight) -> FontKey {
    let face = match weight {
        Weight::Regular => FontFace::Regular,
        Weight::Medium => FontFace::Medium,
        Weight::Bold => FontFace::Bold,
    };
    FontKey::new(face, size)
}

/// `theme.iconFont(size)`.
pub fn icon_font(size: f32) -> FontKey {
    FontKey::new(FontFace::Icons, size)
}

/// The theme state: dark or light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub is_dark: bool,
}

impl Theme {
    pub fn palette(&self) -> &'static Palette {
        if self.is_dark { &DARK } else { &LIGHT }
    }
}

/// Material Icons codepoints for the icons the app uses (`theme.icons`; outlined variants from
/// the Flutter app map to their filled counterparts).
pub fn icon_codepoint(name: &str) -> Option<char> {
    let codepoint = match name {
        "add" => 0xe145,
        "close" => 0xe5cd,
        "folder_open" => 0xe2c8,
        "delete_outline" => 0xe92e,
        "save" => 0xe161,
        "refresh" => 0xe5d5,
        "filter_alt_off" => 0xeb32,
        "shuffle" => 0xe043,
        "search" => 0xe8b6,
        "remove" => 0xe15b,
        "clear" => 0xe14c,
        "arrow_drop_down" => 0xe5c5,
        "warning_amber" => 0xf083,
        "visibility" => 0xe8f4,
        "visibility_off" => 0xe8f5,
        "system_update" => 0xe62a,
        "style" => 0xe41d,
        "save_alt" => 0xe171,
        "person_remove" => 0xef66,
        "note_add" => 0xe89c,
        "merge_type" => 0xe252,
        "logout" => 0xe9ba,
        "power_settings_new" => 0xe8ac,
        "light_mode" => 0xe518,
        "image_not_supported" => 0xf116,
        "filter_none" => 0xe3e0,
        "expand_more" => 0xe5cf,
        "expand_less" => 0xe5ce,
        "drag_indicator" => 0xe945,
        "dark_mode" => 0xe51c,
        "crop_square" => 0xe3c6,
        "copy_all" => 0xe2ec,
        "colorize" => 0xe3b8,
        "chevron_right" => 0xe5cc,
        "chevron_left" => 0xe5cb,
        "check_box_outline_blank" => 0xe835,
        "check_box" => 0xe834,
        "check" => 0xe5ca,
        "broken_image" => 0xe3ad,
        "keyboard" => 0xe312,
        "arrow_back" => 0xe5c4,
        "settings" => 0xe8b8,
        "person" => 0xe7fd,
        "edit" => 0xe3c9,
        "palette" => 0xe40a,
        "more_vert" => 0xe5d4,
        "info_outline" => 0xe88f,
        "build" => 0xe869,
        "layers" => 0xe53b,
        "widgets" => 0xe1bd,
        "image" => 0xe3f4,
        "send" => 0xe163,
        "shield" => 0xe9e0,
        "toll" => 0xe8e0,
        "block" => 0xe14b,
        "vpn_key" => 0xe0da,
        "card_giftcard" => 0xe8f6,
        "sports_esports" => 0xea28,
        "group" => 0xe7ef,
        "swap_horiz" => 0xe8d4,
        "hourglass_empty" => 0xe88b,
        "history" => 0xe889,
        "local_cafe" => 0xe541,
        "folder" => 0xe2c7,
        "insert_drive_file" => 0xe24d,
        "arrow_upward" => 0xe5d8,
        "upload_file" => 0xe2c6,
        "menu_book" => 0xea19,
        _ => return None,
    };
    char::from_u32(codepoint)
}

/// The glyph for an icon name; panics on an unknown name like the original's `error`.
pub fn glyph(name: &str) -> String {
    icon_codepoint(name).unwrap_or_else(|| panic!("unknown icon '{name}'")).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_and_helpers() {
        assert_eq!(LIGHT.primary, Color::rgba(0x3B as f32 / 255.0, 0x69 as f32 / 255.0, 0x39 as f32 / 255.0, 1.0));
        assert!(is_light(LIGHT.surface) && !is_light(DARK.surface));
        assert_eq!(mix(fixed::BLACK, fixed::WHITE, 0.5).red, 0.5);
        assert_eq!(with_alpha(fixed::WHITE, 0.5).alpha, 0.5);
        assert_eq!(glyph("close"), "\u{e5cd}");
        assert_eq!(font(14.4, Weight::Medium), FontKey { face: FontFace::Medium, size: 14 });
    }
}
