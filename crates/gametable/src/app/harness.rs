//! A windowless app context for tests: everything a [`Frame`] needs, plus input helpers that
//! route events the way the app shell does (the original's `--script` API in spirit).

use std::path::PathBuf;

use crate::app::input::KeyboardState;
use crate::app::{Frame, MemoryClipboard, PlayerProfile, ScreenCommand, find_assets_folder};
use crate::data::display_settings::HandSettings;
use crate::data::paths::AppPaths;
use crate::data::settings::Settings;
use crate::graphics::draw_list::DrawList;
use crate::graphics::image_cache::ImageCache;
use crate::graphics::text::FontLibrary;
use crate::sound::SoundPlayer;
use crate::ui::context::Ui;
use crate::ui::theme::Theme;

pub struct UiHarness {
    pub ui: Ui,
    pub fonts: FontLibrary,
    pub images: ImageCache,
    pub settings: Settings,
    pub paths: AppPaths,
    pub sounds: SoundPlayer,
    pub keyboard: KeyboardState,
    pub theme: Theme,
    pub clipboard: MemoryClipboard,
    pub commands: Vec<ScreenCommand>,
    pub profile: PlayerProfile,
    pub hand_settings: HandSettings,
    pub width: f32,
    pub height: f32,
    pub time: f64,
    assets_folder: PathBuf,
}

impl UiHarness {
    /// A 1280×720 logical screen at scale 1, light theme, in-memory settings.
    pub fn new() -> UiHarness {
        let assets_folder = find_assets_folder();
        let fonts = FontLibrary::new(&assets_folder.join("fonts")).expect("fonts load");
        let scratch = std::env::temp_dir().join("gametable-ui-harness");
        let mut ui = Ui::new();
        ui.recorded_regions = Some(std::collections::HashMap::new());
        UiHarness {
            ui,
            fonts,
            images: ImageCache::new(),
            settings: Settings::in_memory(),
            paths: AppPaths { app_folder: scratch.clone(), save_folder: scratch },
            sounds: SoundPlayer::new(None, &assets_folder.join("audio")),
            keyboard: KeyboardState::default(),
            theme: Theme::new(false),
            clipboard: MemoryClipboard::default(),
            commands: Vec::new(),
            profile: PlayerProfile::load(&Settings::in_memory()),
            hand_settings: HandSettings::default(),
            width: 1280.0,
            height: 720.0,
            time: 0.0,
            assets_folder,
        }
    }

    /// Runs one frame of drawing; time advances by 1/60 s.
    pub fn frame(&mut self, draw: impl FnOnce(&mut Frame)) -> DrawList {
        self.time += 1.0 / 60.0;
        let mut frame = Frame {
            ui: &mut self.ui,
            fonts: &mut self.fonts,
            images: &mut self.images,
            settings: &mut self.settings,
            paths: &self.paths,
            sounds: &mut self.sounds,
            keyboard: &self.keyboard,
            theme: &mut self.theme,
            clipboard: &mut self.clipboard,
            commands: &mut self.commands,
            profile: &mut self.profile,
            hand_settings: &mut self.hand_settings,
            scale: 1.0,
            dpi_scale: 1.0,
            width: self.width,
            height: self.height,
            time: self.time,
            assets_folder: &self.assets_folder,
        };
        frame.ui.begin_frame(self.width, self.height, 1.0, self.time);
        draw(&mut frame);
        frame.draw_app_overlays();
        frame.draw_tooltip();
        self.ui.end_frame()
    }

    pub fn move_mouse(&mut self, x: f32, y: f32) {
        self.ui.on_mouse_moved(x, y);
    }

    pub fn press(&mut self, x: f32, y: f32, button: u8, clicks: u8) {
        self.ui.on_mouse_pressed(x, y, button, clicks);
    }

    pub fn release(&mut self, x: f32, y: f32, button: u8) {
        self.ui.on_mouse_released(x, y, button);
    }

    /// Turns the mouse wheel (positive is away from the user, zooming in).
    pub fn wheel(&mut self, notches: f32) {
        self.ui.on_wheel(0.0, notches);
    }

    /// A key press routed like `App.keypressed`: open dialogs and menus get Escape first, a
    /// focused field gets keys, then open overlays. Returns true when the UI took the key.
    pub fn key(&mut self, key: &'static str) -> bool {
        self.keyboard.press(key);
        let taken = if key == "escape" && self.ui.any_overlay_open() {
            self.ui.overlay_key_pressed(key)
        } else if self.ui.on_key_pressed(key, false) {
            true
        } else {
            self.ui.any_overlay_open() && self.ui.overlay_key_pressed(key)
        };
        self.keyboard.release(key);
        taken
    }

    /// Holds a key down until [UiHarness::let_go] (modifiers are read while drawing).
    pub fn hold(&mut self, key: &'static str) {
        self.keyboard.press(key);
    }

    pub fn let_go(&mut self, key: &'static str) {
        self.keyboard.release(key);
    }

    pub fn type_text(&mut self, text: &str) -> bool {
        self.ui.on_text_input(text)
    }

    /// The centre of the region `id` as last drawn.
    pub fn region_center(&self, id: &str) -> Option<(f32, f32)> {
        let recorded = self.ui.recorded_regions.as_ref()?;
        recorded.get(id).map(|&(x, y, width, height)| (x + width / 2.0, y + height / 2.0))
    }

    /// Clicks the region `id`: one settling frame (a dialog's first frame lays out before it
    /// knows its height), then move, press and release, one frame each.
    pub fn click_region(&mut self, id: &str, draw: &mut dyn FnMut(&mut Frame)) {
        self.frame(|frame| draw(frame));
        let (x, y) = self.region_center(id).unwrap_or_else(|| panic!("no region '{id}' was drawn"));
        self.move_mouse(x, y);
        self.frame(|frame| draw(frame));
        self.press(x, y, 1, 1);
        self.frame(|frame| draw(frame));
        self.release(x, y, 1);
        self.frame(|frame| draw(frame));
    }
}

impl Default for UiHarness {
    fn default() -> Self {
        UiHarness::new()
    }
}
