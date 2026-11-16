//! Keyboard and mouse input in LÖVE's terms, so ported screens keep using the original's key
//! names (`"escape"`, `"kpenter"`, `"lctrl"`, ...) and mouse button numbers (1 left, 2 right,
//! 3 middle).

use std::collections::HashSet;

use sdl3::sys::keycode::*;

/// LÖVE 11's `KeyConstant` for an SDL keycode; `"unknown"` for keys LÖVE has no name for.
pub fn love_key_name(keycode: u32) -> &'static str {
    // Printable ASCII keys are named by their character (SDL keycodes for letters are lowercase).
    if (32..=126).contains(&keycode) {
        return PRINTABLE_KEY_NAMES[(keycode - 32) as usize];
    }
    let keycode = SDL_Keycode(keycode);
    match keycode {
        SDLK_RETURN => "return",
        SDLK_ESCAPE => "escape",
        SDLK_BACKSPACE => "backspace",
        SDLK_TAB => "tab",
        SDLK_DELETE => "delete",
        SDLK_CAPSLOCK => "capslock",
        SDLK_F1 => "f1",
        SDLK_F2 => "f2",
        SDLK_F3 => "f3",
        SDLK_F4 => "f4",
        SDLK_F5 => "f5",
        SDLK_F6 => "f6",
        SDLK_F7 => "f7",
        SDLK_F8 => "f8",
        SDLK_F9 => "f9",
        SDLK_F10 => "f10",
        SDLK_F11 => "f11",
        SDLK_F12 => "f12",
        SDLK_F13 => "f13",
        SDLK_F14 => "f14",
        SDLK_F15 => "f15",
        SDLK_F16 => "f16",
        SDLK_F17 => "f17",
        SDLK_F18 => "f18",
        SDLK_PRINTSCREEN => "printscreen",
        SDLK_SCROLLLOCK => "scrolllock",
        SDLK_PAUSE => "pause",
        SDLK_INSERT => "insert",
        SDLK_HOME => "home",
        SDLK_PAGEUP => "pageup",
        SDLK_END => "end",
        SDLK_PAGEDOWN => "pagedown",
        SDLK_RIGHT => "right",
        SDLK_LEFT => "left",
        SDLK_DOWN => "down",
        SDLK_UP => "up",
        SDLK_NUMLOCKCLEAR => "numlock",
        SDLK_KP_DIVIDE => "kp/",
        SDLK_KP_MULTIPLY => "kp*",
        SDLK_KP_MINUS => "kp-",
        SDLK_KP_PLUS => "kp+",
        SDLK_KP_ENTER => "kpenter",
        SDLK_KP_0 => "kp0",
        SDLK_KP_1 => "kp1",
        SDLK_KP_2 => "kp2",
        SDLK_KP_3 => "kp3",
        SDLK_KP_4 => "kp4",
        SDLK_KP_5 => "kp5",
        SDLK_KP_6 => "kp6",
        SDLK_KP_7 => "kp7",
        SDLK_KP_8 => "kp8",
        SDLK_KP_9 => "kp9",
        SDLK_KP_PERIOD => "kp.",
        SDLK_KP_COMMA => "kp,",
        SDLK_KP_EQUALS => "kp=",
        SDLK_APPLICATION => "application",
        SDLK_POWER => "power",
        SDLK_HELP => "help",
        SDLK_MENU => "menu",
        SDLK_UNDO => "undo",
        SDLK_SYSREQ => "sysreq",
        SDLK_CLEAR => "clear",
        SDLK_CURRENCYUNIT => "currencyunit",
        SDLK_LCTRL => "lctrl",
        SDLK_LSHIFT => "lshift",
        SDLK_LALT => "lalt",
        SDLK_LGUI => "lgui",
        SDLK_RCTRL => "rctrl",
        SDLK_RSHIFT => "rshift",
        SDLK_RALT => "ralt",
        SDLK_RGUI => "rgui",
        SDLK_MODE => "mode",
        SDLK_AC_SEARCH => "appsearch",
        SDLK_AC_HOME => "apphome",
        SDLK_AC_BACK => "appback",
        SDLK_AC_FORWARD => "appforward",
        SDLK_AC_REFRESH => "apprefresh",
        SDLK_AC_BOOKMARKS => "appbookmarks",
        _ => "unknown",
    }
}

const PRINTABLE_KEY_NAMES: [&str; 95] = [
    " ", "!", "\"", "#", "$", "%", "&", "'", "(", ")", "*", "+", ",", "-", ".", "/", "0", "1", "2", "3", "4", "5", "6",
    "7", "8", "9", ":", ";", "<", "=", ">", "?", "@", "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M",
    "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z", "[", "\\", "]", "^", "_", "`", "a", "b", "c", "d",
    "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z", "{",
    "|", "}", "~",
];

/// LÖVE names the space key `"space"`, not `" "`.
fn love_name_for_printable(name: &'static str) -> &'static str {
    if name == " " { "space" } else { name }
}

/// The LÖVE key name for an SDL keycode, with `"space"` for the space bar.
pub fn key_name(keycode: u32) -> &'static str {
    love_name_for_printable(love_key_name(keycode))
}

/// LÖVE's mouse button number: 1 left, 2 right, 3 middle, 4 and 5 the side buttons.
pub fn love_mouse_button(sdl_button: sdl3::mouse::MouseButton) -> Option<u8> {
    use sdl3::mouse::MouseButton;
    match sdl_button {
        MouseButton::Left => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Middle => Some(3),
        MouseButton::X1 => Some(4),
        MouseButton::X2 => Some(5),
        MouseButton::Unknown => None,
    }
}

/// Which keys are held, for `love.keyboard.isDown`.
#[derive(Debug, Default)]
pub struct KeyboardState {
    held: HashSet<&'static str>,
}

impl KeyboardState {
    pub fn press(&mut self, key: &'static str) {
        self.held.insert(key);
    }

    pub fn release(&mut self, key: &'static str) {
        self.held.remove(key);
    }

    /// Forgets everything (the window lost focus and won't see the releases).
    pub fn clear(&mut self) {
        self.held.clear();
    }

    /// `love.keyboard.isDown(...)`: true when any of the keys is held.
    pub fn is_down(&self, keys: &[&str]) -> bool {
        keys.iter().any(|key| self.held.contains(key))
    }

    pub fn ctrl(&self) -> bool {
        self.is_down(&["lctrl", "rctrl"])
    }

    pub fn shift(&self) -> bool {
        self.is_down(&["lshift", "rshift"])
    }

    pub fn alt(&self) -> bool {
        self.is_down(&["lalt", "ralt"])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_match_love() {
        assert_eq!(key_name(SDLK_A.0), "a");
        assert_eq!(key_name(SDLK_SPACE.0), "space");
        assert_eq!(key_name(SDLK_1.0), "1");
        assert_eq!(key_name(SDLK_RETURN.0), "return");
        assert_eq!(key_name(SDLK_KP_ENTER.0), "kpenter");
        assert_eq!(key_name(SDLK_F6.0), "f6");
        assert_eq!(key_name(SDLK_RCTRL.0), "rctrl");
        assert_eq!(key_name(SDLK_DELETE.0), "delete");
        assert_eq!(key_name(SDLK_MEDIA_PLAY.0), "unknown");
    }

    #[test]
    fn held_keys() {
        let mut keyboard = KeyboardState::default();
        keyboard.press("lctrl");
        assert!(keyboard.ctrl() && !keyboard.shift());
        assert!(keyboard.is_down(&["space", "lctrl"]));
        keyboard.release("lctrl");
        assert!(!keyboard.ctrl());
    }
}
