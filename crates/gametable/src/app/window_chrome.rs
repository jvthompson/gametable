//! Frameless window chrome. Port of `src/platform/window.lua`: SDL's window hit test makes the
//! custom title bar drag the window (with native snapping and double-click maximize) and the
//! edges resize it while it isn't maximized. F6 switches to the native frame instead.

use std::sync::{Arc, Mutex};

use sdl3::video::{HitTestResult, Window};

/// Height of the custom title bar, in logical units.
pub const TITLE_BAR_HEIGHT: f32 = 56.0;
/// Width of the resize border, in logical units.
pub const RESIZE_BORDER: f32 = 6.0;

/// What the hit test needs to know, refreshed every frame by the app.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChromeState {
    /// Window size in physical pixels.
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
    /// The UI scale (logical units to physical pixels).
    pub scale: f32,
    /// Physical pixels per window coordinate (1 on Windows).
    pub pixel_density: f32,
    /// The native frame is showing (F6).
    pub framed: bool,
    /// The empty part of the title bar, in logical x: between the title and the buttons.
    pub drag_from_x: f32,
    pub drag_to_x: f32,
}

impl Default for ChromeState {
    fn default() -> Self {
        ChromeState {
            width: 1280.0,
            height: 720.0,
            maximized: false,
            scale: 1.0,
            pixel_density: 1.0,
            framed: false,
            drag_from_x: 0.0,
            drag_to_x: 0.0,
        }
    }
}

/// Classifies a point in window coordinates.
pub fn hit_test(state: &ChromeState, window_x: f32, window_y: f32) -> HitTestResult {
    let (x, y) = (window_x * state.pixel_density, window_y * state.pixel_density);
    if !state.framed && !state.maximized {
        let border = RESIZE_BORDER * state.scale;
        let (left, right) = (x < border, x >= state.width - border);
        let (top, bottom) = (y < border, y >= state.height - border);
        match (left, right, top, bottom) {
            (true, _, true, _) => return HitTestResult::ResizeTopLeft,
            (_, true, true, _) => return HitTestResult::ResizeTopRight,
            (true, _, _, true) => return HitTestResult::ResizeBottomLeft,
            (_, true, _, true) => return HitTestResult::ResizeBottomRight,
            (_, _, true, _) => return HitTestResult::ResizeTop,
            (_, _, _, true) => return HitTestResult::ResizeBottom,
            (true, _, _, _) => return HitTestResult::ResizeLeft,
            (_, true, _, _) => return HitTestResult::ResizeRight,
            _ => {}
        }
    }
    let (logical_x, logical_y) = (x / state.scale, y / state.scale);
    if !state.framed && logical_y < TITLE_BAR_HEIGHT && logical_x >= state.drag_from_x && logical_x < state.drag_to_x {
        return HitTestResult::Draggable;
    }
    HitTestResult::Normal
}

/// Makes a window resizable (SDL creates borderless windows that can't be maximized otherwise).
pub fn set_resizable(window: &Window) {
    unsafe { sdl3::sys::video::SDL_SetWindowResizable(window.raw(), true) };
}

/// Installs the hit test on a window. Returns false when the platform doesn't support it.
pub fn install(window: &mut Window, state: Arc<Mutex<ChromeState>>) -> bool {
    // By default SDL swallows the click that refocuses the window; make it a real click.
    sdl3::hint::set("SDL_MOUSE_FOCUS_CLICKTHROUGH", "1");
    set_resizable(window);
    window
        .set_hit_test(move |point| {
            let state = *state.lock().unwrap();
            hit_test(&state, point.x as f32, point.y as f32)
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ChromeState {
        ChromeState {
            width: 1000.0,
            height: 800.0,
            scale: 2.0,
            drag_from_x: 100.0,
            drag_to_x: 400.0,
            ..Default::default()
        }
    }

    #[test]
    fn edges_resize_unless_maximized_or_framed() {
        let chrome = state();
        assert_eq!(hit_test(&chrome, 5.0, 5.0), HitTestResult::ResizeTopLeft);
        assert_eq!(hit_test(&chrome, 995.0, 400.0), HitTestResult::ResizeRight);
        assert_eq!(hit_test(&chrome, 500.0, 795.0), HitTestResult::ResizeBottom);
        // The border is 6 logical = 12 physical at 2x.
        assert_eq!(hit_test(&chrome, 11.0, 400.0), HitTestResult::ResizeLeft);
        assert_eq!(hit_test(&chrome, 13.0, 400.0), HitTestResult::Normal);
        let maximized = ChromeState { maximized: true, ..chrome };
        assert_eq!(hit_test(&maximized, 5.0, 400.0), HitTestResult::Normal);
    }

    #[test]
    fn empty_title_bar_space_drags() {
        let chrome = state();
        // Logical (150, 20) -> physical (300, 40).
        assert_eq!(hit_test(&chrome, 300.0, 40.0), HitTestResult::Draggable);
        assert_eq!(hit_test(&chrome, 900.0, 40.0), HitTestResult::Normal); // over the buttons
        assert_eq!(hit_test(&chrome, 300.0, 120.0), HitTestResult::Normal); // below the bar
        let framed = ChromeState { framed: true, ..chrome };
        assert_eq!(hit_test(&framed, 300.0, 40.0), HitTestResult::Normal);
    }
}
