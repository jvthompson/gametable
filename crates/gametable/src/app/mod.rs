//! The application shell. Port of `src/app.lua` and `main.lua`: the screen stack, the custom
//! title bar, UI scale (display DPI × the user's setting), input routing in logical units and
//! the frame loop.
//!
//! Screens draw into a [`Frame`], which bundles what the original reached through `love.*`
//! and its global modules: the UI (input, layers, draw lists), fonts, image cache, settings and
//! keyboard state. As in the original, mouse input goes only to the UI kit; screens read it
//! through `frame.ui` while drawing.

pub mod clipboard;
pub mod harness;
pub mod input;
pub mod window_chrome;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use sdl3::event::{Event, WindowEvent};
use sdl3::mouse::{Cursor, SystemCursor};
use sdl3::video::Window;

use crate::data::display_settings::HandSettings;
use crate::data::paths::{AppPaths, DEFAULT_IDENTITY, LEGACY_IDENTITY};
use crate::data::profile;
use crate::data::settings::{Settings, keys};
use crate::graphics::gpu_renderer::GpuRenderer;
use crate::graphics::image_cache::ImageCache;
use crate::graphics::image_data::RgbaImage;
use crate::graphics::text::FontLibrary;
use crate::sound::SoundPlayer;
use crate::ui::context::Ui;
use crate::ui::draw::TextOptions;
use crate::ui::theme::{Theme, fixed};
use crate::ui::widgets::IconButtonOptions;
pub use clipboard::{Clipboard, MemoryClipboard, SdlClipboard};
use input::{KeyboardState, key_name, love_mouse_button};
use window_chrome::{ChromeState, TITLE_BAR_HEIGHT};

/// Logical minimum window size; the layouts are designed for at least this.
pub const MINIMUM_WIDTH: f32 = 960.0;
pub const MINIMUM_HEIGHT: f32 = 600.0;
/// The UI scale options offered in Settings.
pub const UI_SCALE_OPTIONS: [f32; 8] = [0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// What a screen asks the shell to do after an event or frame.
pub enum ScreenCommand {
    Push(Box<dyn Screen>),
    Pop,
    Replace(Box<dyn Screen>),
    /// Clears everything above Home; the message shows in Home's banner.
    PopToHome(Option<String>),
    /// Pops every screen above the one whose [`Screen::marker`] is `marker` (the server lobby),
    /// then shows `message` as a snackbar.
    PopTo {
        marker: &'static str,
        message: Option<String>,
    },
    Quit,
}

/// One screen of the app (the original's screen tables with optional methods).
pub trait Screen {
    fn title(&self) -> String {
        "CardTable".to_string()
    }
    /// Whether the title bar shows a back arrow (when there is a screen to go back to).
    fn shows_back(&self) -> bool {
        true
    }
    /// The back arrow; by default pops this screen.
    fn on_back(&mut self, frame: &mut Frame) {
        frame.commands.push(ScreenCommand::Pop);
    }
    /// The screen and its dialogs draw light regardless of dark mode (the game table).
    fn forces_light_theme(&self) -> bool {
        false
    }
    fn enter(&mut self, _frame: &mut Frame) {}
    fn leave(&mut self, _frame: &mut Frame) {}
    fn resume(&mut self, _frame: &mut Frame) {}
    /// Called for every screen in the stack each frame (network objects keep running).
    fn background_update(&mut self, _frame: &mut Frame, _dt: f32) {}
    fn update(&mut self, _frame: &mut Frame, _dt: f32) {}
    /// Draws inside the content area below the title bar (logical units). Mouse input is read
    /// from `frame.ui` here, as the original's screens did through the UI kit.
    fn draw(&mut self, _frame: &mut Frame, _x: f32, _y: f32, _width: f32, _height: f32) {}
    fn key_pressed(&mut self, _frame: &mut Frame, _key: &'static str, _is_repeat: bool) {}
    fn key_released(&mut self, _frame: &mut Frame, _key: &'static str) {}
    fn text_input(&mut self, _frame: &mut Frame, _text: &str) {}
    fn file_dropped(&mut self, _frame: &mut Frame, _path: &Path) {}
    fn directory_dropped(&mut self, _frame: &mut Frame, _path: &Path) {}
    /// Return true to cancel quitting.
    fn quit(&mut self, _frame: &mut Frame) -> bool {
        false
    }
    /// Home's orange banner message (`App.popToHome(message)`).
    fn set_message(&mut self, _message: Option<String>) {}
    /// A name [`ScreenCommand::PopTo`] can return to (the original's `isServerLobby` flag).
    fn marker(&self) -> Option<&'static str> {
        None
    }
}

/// Everything a screen can reach while handling an event or drawing.
pub struct Frame<'a> {
    /// The UI kit state: input, layers (each with its draw list), focus, widget state.
    pub ui: &'a mut Ui,
    pub fonts: &'a mut FontLibrary,
    pub images: &'a mut ImageCache,
    pub settings: &'a mut Settings,
    pub paths: &'a AppPaths,
    pub sounds: &'a mut SoundPlayer,
    pub keyboard: &'a KeyboardState,
    pub theme: &'a mut Theme,
    pub clipboard: &'a mut dyn Clipboard,
    pub commands: &'a mut Vec<ScreenCommand>,
    /// `App.profile`: the player's name, colour and avatar.
    pub profile: &'a mut PlayerProfile,
    /// The hand settings with their live (unsaved while a slider is held) values.
    pub hand_settings: &'a mut HandSettings,
    /// The UI scale (logical units to physical pixels).
    pub scale: f32,
    /// The display's own scaling (1 = 96 DPI), as Settings reports it.
    pub dpi_scale: f32,
    /// Logical size of the whole window.
    pub width: f32,
    pub height: f32,
    /// Seconds since the app started.
    pub time: f64,
    pub assets_folder: &'a Path,
}

/// Command-line options (`--identity`, `--dark`, `--light`, `--no-discord`, `--update-check`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    pub identity: Option<String>,
    pub dark: Option<bool>,
    pub no_discord: bool,
    pub force_update_check: bool,
}

impl Options {
    pub fn parse(arguments: impl IntoIterator<Item = String>) -> Options {
        let arguments: Vec<String> = arguments.into_iter().collect();
        let mut options = Options::default();
        let mut index = 0;
        while index < arguments.len() {
            match arguments[index].as_str() {
                "--identity" if index + 1 < arguments.len() => {
                    options.identity = Some(arguments[index + 1].clone());
                    index += 1;
                }
                "--dark" => options.dark = Some(true),
                "--light" => options.dark = Some(false),
                "--no-discord" => options.no_discord = true,
                "--update-check" => options.force_update_check = true,
                _ => {}
            }
            index += 1;
        }
        options
    }
}

/// The `assets` folder: next to the executable, or the workspace's during development.
pub fn find_assets_folder() -> PathBuf {
    let executable_folder = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
    let beside_exe = executable_folder.as_ref().map(|folder| folder.join("assets"));
    if let Some(folder) = beside_exe.filter(|folder| folder.join("fonts").is_dir()) {
        return folder;
    }
    // A macOS bundle keeps them in CardTable.app/Contents/Resources/assets.
    let in_bundle = executable_folder
        .as_ref()
        .and_then(|folder| folder.parent())
        .map(|contents| contents.join("Resources").join("assets"));
    if let Some(folder) = in_bundle.filter(|folder| folder.join("fonts").is_dir()) {
        return folder;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// `App.uiScaleSetting()`: the stored setting clamped to 0.5..3.
pub fn ui_scale_setting(settings: &Settings) -> f32 {
    settings.number(keys::UI_SCALE).unwrap_or(1.0).clamp(0.5, 3.0) as f32
}

/// `App.setUiScaleSetting(v)`: 1 is the default and isn't stored.
pub fn set_ui_scale_setting(settings: &mut Settings, value: f32) {
    if value == 1.0 {
        settings.remove(keys::UI_SCALE);
    } else {
        settings.set(keys::UI_SCALE, f64::from(value));
    }
}

/// `App.effectiveScale()`: DPI × setting, capped so a maximized window still has a usable
/// logical size on this monitor.
pub fn effective_scale(dpi_scale: f32, setting: f32, desktop_size: Option<(f32, f32)>) -> f32 {
    let mut scale = dpi_scale * setting;
    if let Some((desktop_width, desktop_height)) = desktop_size.filter(|(width, _)| *width > 0.0) {
        scale = scale.min(desktop_width / MINIMUM_WIDTH).min(desktop_height / (MINIMUM_HEIGHT + 40.0));
    }
    scale.max(0.5)
}

/// The player's profile as screens use it (`App.profile`).
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerProfile {
    pub name: String,
    pub color: u32,
    pub avatar_path: Option<String>,
}

impl PlayerProfile {
    pub fn load(settings: &Settings) -> PlayerProfile {
        let name = profile::name(settings).filter(|name| !name.is_empty()).unwrap_or("Player").to_string();
        let color = profile::color(settings).map_or(0xFF45_5A64, |color| color as u32);
        PlayerProfile { name, color, avatar_path: profile::avatar_path(settings).map(str::to_string) }
    }

    /// `App.playerName()`: trimmed, `"Player"` when empty.
    pub fn display_name(&self) -> String {
        let trimmed = self.name.trim();
        if trimmed.is_empty() { "Player".to_string() } else { trimmed.to_string() }
    }
}

/// A title-bar button that was clicked this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TitleAction {
    Back,
    Close,
    Maximize,
    Theme,
}

/// What drawing the title bar reports: a clicked button and the draggable span.
struct TitleBarResult {
    action: Option<TitleAction>,
    title_end: f32,
    buttons_start: f32,
}

/// The running app.
pub struct App {
    sdl: sdl3::Sdl,
    video: sdl3::VideoSubsystem,
    window: Window,
    renderer: GpuRenderer,
    ui: Ui,
    fonts: FontLibrary,
    images: ImageCache,
    sounds: SoundPlayer,
    settings: Settings,
    paths: AppPaths,
    assets_folder: PathBuf,
    theme: Theme,
    keyboard: KeyboardState,
    clipboard: SdlClipboard,
    cursors: HashMap<&'static str, Option<Cursor>>,
    shown_cursor: &'static str,
    chrome: Arc<Mutex<ChromeState>>,
    framed: bool,
    screens: Vec<Box<dyn Screen>>,
    pub profile: PlayerProfile,
    hand_settings: HandSettings,
    applied_scale: f32,
    dpi_scale: f32,
    dpi_checked_at: f64,
    started_at: Instant,
    frame_count: u64,
    /// Development hook (`GAMETABLE_CAPTURE=<file.png>`): save frame 30 and exit, like the
    /// original's `--shot`.
    capture_path: Option<PathBuf>,
}

impl App {
    /// Opens the window and loads everything the first frame needs.
    pub fn new(options: &Options, home: impl FnOnce() -> Box<dyn Screen>) -> Result<App, String> {
        // Per-monitor DPI awareness: the window gets real pixels and the app scales its own UI.
        sdl3::hint::set("SDL_WINDOWS_DPI_AWARENESS", "permonitorv2");
        // Key names must not change with Shift (LÖVE reported "1", not "!").
        sdl3::hint::set("SDL_KEYCODE_OPTIONS", "unmodified,latin_letters");
        let paths = match &options.identity {
            Some(identity) => AppPaths::for_identity(identity)?,
            None => {
                let paths = AppPaths::for_identity(DEFAULT_IDENTITY)?;
                paths.migrate_legacy_identity(LEGACY_IDENTITY);
                paths
            }
        };
        let settings = Settings::load_from_folder(paths.save_folder());
        let assets_folder = find_assets_folder();

        let sdl = sdl3::init().map_err(|error| error.to_string())?;
        let video = sdl.video().map_err(|error| error.to_string())?;
        // Which window system SDL picked (windows, cocoa, wayland, x11), for bug reports.
        eprintln!("video: SDL video driver {}", video.current_video_driver());
        let mut window = video
            .window("CardTable", 1280, 720)
            .position_centered()
            .resizable()
            .borderless()
            .high_pixel_density()
            .build()
            .map_err(|error| error.to_string())?;
        if let Ok(icon) = RgbaImage::load(&assets_folder.join("icon.png")) {
            let mut pixels = icon.pixels.clone();
            if let Ok(surface) = sdl3::surface::Surface::from_data(
                &mut pixels,
                icon.width,
                icon.height,
                icon.width * 4,
                sdl3::pixels::PixelFormat::RGBA32,
            ) {
                window.set_icon(surface);
            }
        }
        let chrome = Arc::new(Mutex::new(ChromeState::default()));
        window_chrome::install(&mut window, chrome.clone());
        video.text_input().start(&window);

        let renderer = GpuRenderer::new(&window)?;
        let fonts = FontLibrary::new(&assets_folder.join("fonts"))?;
        let audio = sdl.audio().ok();
        let sounds = SoundPlayer::new(audio, &assets_folder.join("audio"));
        let is_dark = options.dark.unwrap_or_else(|| profile::dark_mode(&settings));
        let profile = PlayerProfile::load(&settings);
        let clipboard = SdlClipboard::new(video.clipboard());

        let mut app = App {
            sdl,
            video,
            window,
            renderer,
            ui: Ui::new(),
            fonts,
            images: ImageCache::new(),
            sounds,
            settings,
            paths,
            assets_folder,
            theme: Theme::new(is_dark),
            keyboard: KeyboardState::default(),
            clipboard,
            cursors: HashMap::new(),
            shown_cursor: "arrow",
            chrome,
            framed: false,
            screens: Vec::new(),
            profile,
            hand_settings: HandSettings::default(),
            applied_scale: 0.0,
            dpi_scale: 1.0,
            dpi_checked_at: -1.0,
            started_at: Instant::now(),
            frame_count: 0,
            capture_path: std::env::var_os("GAMETABLE_CAPTURE").map(PathBuf::from),
        };
        app.apply_scale(true);
        app.window.maximize();
        let home = home();
        app.run_commands(vec![ScreenCommand::Push(home)]);
        Ok(app)
    }

    fn elapsed(&self) -> f64 {
        self.started_at.elapsed().as_secs_f64()
    }

    /// The display's DPI scale (1 = 96 DPI), rounded to hundredths, checked twice a second.
    fn dpi_scale(&mut self) -> f32 {
        let now = self.elapsed();
        if now >= self.dpi_checked_at {
            self.dpi_checked_at = now + 0.5;
            if let Ok(display) = self.window.get_display()
                && let Ok(scale) = display.get_content_scale()
                && scale > 0.0
            {
                self.dpi_scale = (scale * 100.0 + 0.5).floor() / 100.0;
            }
        }
        self.dpi_scale
    }

    fn desktop_size(&self) -> Option<(f32, f32)> {
        let display = self.window.get_display().ok()?;
        let mode = display.get_mode().ok()?;
        Some((mode.w as f32, mode.h as f32))
    }

    /// `applyScale`: updates fonts, the hit test and the minimum window size when the scale moves.
    fn apply_scale(&mut self, force: bool) {
        let scale = effective_scale(self.dpi_scale(), ui_scale_setting(&self.settings), self.desktop_size());
        if scale == self.applied_scale && !force {
            return;
        }
        self.applied_scale = scale;
        self.fonts.set_dpi_scale(scale);
        let (desktop_width, desktop_height) = self.desktop_size().unwrap_or((1e9, 1e9));
        let density = self.window.pixel_density().max(0.01);
        let minimum_width = (MINIMUM_WIDTH * scale).ceil().min(desktop_width) / density;
        let minimum_height = (MINIMUM_HEIGHT * scale).ceil().min(desktop_height) / density;
        let _ = self.window.set_minimum_size(minimum_width as u32, minimum_height as u32);
    }

    /// Runs until the window closes.
    pub fn run(mut self) -> Result<(), String> {
        let mut events = self.sdl.event_pump().map_err(|error| error.to_string())?;
        let mut last_frame = Instant::now();
        loop {
            let pending: Vec<Event> = events.poll_iter().collect();
            for event in pending {
                if !self.handle_event(event) {
                    self.shutdown();
                    return Ok(());
                }
            }
            let now = Instant::now();
            let dt = now.duration_since(last_frame).as_secs_f32();
            last_frame = now;
            if !self.update_and_draw(dt)? {
                self.shutdown();
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn shutdown(&mut self) {
        self.images.shutdown();
    }

    /// Builds a frame context around the app's state and runs `action` with it.
    fn with_frame<R>(
        &mut self,
        action: impl FnOnce(&mut Frame, &mut Vec<Box<dyn Screen>>) -> R,
    ) -> (R, Vec<ScreenCommand>) {
        let (physical_width, physical_height) = self.window.size_in_pixels();
        let scale = self.applied_scale;
        let mut commands = Vec::new();
        let time = self.elapsed();
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
            commands: &mut commands,
            profile: &mut self.profile,
            hand_settings: &mut self.hand_settings,
            scale,
            dpi_scale: self.dpi_scale,
            width: physical_width as f32 / scale,
            height: physical_height as f32 / scale,
            time,
            assets_folder: &self.assets_folder,
        };
        let result = action(&mut frame, &mut self.screens);
        (result, commands)
    }

    fn run_commands(&mut self, commands: Vec<ScreenCommand>) {
        let mut queue: std::collections::VecDeque<ScreenCommand> = commands.into();
        while let Some(command) = queue.pop_front() {
            let (more, _) = self.with_frame(|frame, screens| {
                match command {
                    ScreenCommand::Push(mut screen) => {
                        frame.ui.blur();
                        screen.enter(frame);
                        screens.push(screen);
                    }
                    ScreenCommand::Pop => {
                        if let Some(mut screen) = screens.pop() {
                            screen.leave(frame);
                        }
                        frame.ui.blur();
                        if let Some(top) = screens.last_mut() {
                            top.resume(frame);
                        }
                    }
                    ScreenCommand::Replace(mut screen) => {
                        if let Some(mut old) = screens.pop() {
                            old.leave(frame);
                        }
                        frame.ui.blur();
                        screen.enter(frame);
                        screens.push(screen);
                    }
                    ScreenCommand::PopToHome(message) => {
                        while screens.len() > 1 {
                            if let Some(mut screen) = screens.pop() {
                                screen.leave(frame);
                            }
                        }
                        if let Some(home) = screens.last_mut() {
                            home.set_message(message);
                            home.resume(frame);
                        }
                    }
                    ScreenCommand::PopTo { marker, message } => {
                        if screens.iter().any(|screen| screen.marker() == Some(marker)) {
                            while screens.last().is_some_and(|screen| screen.marker() != Some(marker)) {
                                if let Some(mut screen) = screens.pop() {
                                    screen.leave(frame);
                                }
                            }
                            frame.ui.blur();
                            if let Some(top) = screens.last_mut() {
                                top.resume(frame);
                            }
                        }
                        if let Some(message) = message {
                            frame.ui.toast(&message, None);
                        }
                    }
                    ScreenCommand::Quit => frame.commands.push(ScreenCommand::Quit),
                }
                std::mem::take(frame.commands)
            });
            queue.extend(more.into_iter().filter(|command| !matches!(command, ScreenCommand::Quit)));
        }
    }

    /// Mouse coordinates arrive in window coordinates; the UI works in logical units.
    fn to_logical(&self, x: f32, y: f32) -> (f32, f32) {
        let factor = self.window.pixel_density() / self.applied_scale;
        (x * factor, y * factor)
    }

    /// Runs `handler` on the top screen.
    fn on_top_screen(&mut self, handler: impl FnOnce(&mut dyn Screen, &mut Frame)) -> bool {
        let (_, commands) = self.with_frame(|frame, screens| {
            if let Some(screen) = screens.last_mut() {
                handler(screen.as_mut(), frame);
            }
        });
        self.after_event(commands)
    }

    /// Returns false when the app should exit.
    fn handle_event(&mut self, event: Event) -> bool {
        match event {
            Event::Quit { .. } => return !self.screens_cancel_quit(),
            Event::KeyDown { keycode: Some(keycode), repeat, .. } => {
                let key = key_name(keycode as u32);
                self.keyboard.press(key);
                if key == "f6" && !repeat {
                    self.toggle_frame();
                    return true;
                }
                // Escape closes menus and dialogs even while a field inside them has focus.
                if key == "escape" && self.ui.any_overlay_open() {
                    self.ui.overlay_key_pressed(key);
                    return true;
                }
                if self.ui.on_key_pressed(key, repeat) {
                    return true;
                }
                if self.ui.any_overlay_open() {
                    self.ui.overlay_key_pressed(key);
                    return true;
                }
                return self.on_top_screen(|screen, frame| screen.key_pressed(frame, key, repeat));
            }
            Event::KeyUp { keycode: Some(keycode), .. } => {
                let key = key_name(keycode as u32);
                self.keyboard.release(key);
                return self.on_top_screen(|screen, frame| screen.key_released(frame, key));
            }
            Event::TextInput { text, .. } => {
                if self.ui.on_text_input(&text) {
                    return true;
                }
                return self.on_top_screen(|screen, frame| screen.text_input(frame, &text));
            }
            Event::MouseMotion { x, y, .. } => {
                let (x, y) = self.to_logical(x, y);
                self.ui.on_mouse_moved(x, y);
            }
            Event::MouseButtonDown { mouse_btn, clicks, x, y, .. } => {
                let (x, y) = self.to_logical(x, y);
                if let Some(button) = love_mouse_button(mouse_btn) {
                    self.ui.on_mouse_pressed(x, y, button, clicks);
                }
            }
            Event::MouseButtonUp { mouse_btn, x, y, .. } => {
                let (x, y) = self.to_logical(x, y);
                if let Some(button) = love_mouse_button(mouse_btn) {
                    self.ui.on_mouse_released(x, y, button);
                }
            }
            // LÖVE 11 reported whole wheel steps (SDL2's integer deltas).
            Event::MouseWheel { integer_x, integer_y, .. } => {
                self.ui.on_wheel(integer_x as f32, integer_y as f32);
            }
            Event::DropFile { filename, .. } => {
                let path = PathBuf::from(filename);
                return self.on_top_screen(|screen, frame| {
                    if path.is_dir() {
                        screen.directory_dropped(frame, &path);
                    } else {
                        screen.file_dropped(frame, &path);
                    }
                });
            }
            Event::Window { win_event: WindowEvent::FocusLost, .. } => {
                self.keyboard.clear();
            }
            _ => {}
        }
        true
    }

    /// `love.quit`: true when a screen cancels quitting.
    fn screens_cancel_quit(&mut self) -> bool {
        let (cancel, commands) =
            self.with_frame(|frame, screens| screens.iter_mut().rev().any(|screen| screen.quit(frame)));
        self.run_commands(commands);
        cancel
    }

    /// Applies screen commands; false when one asked to quit.
    fn after_event(&mut self, commands: Vec<ScreenCommand>) -> bool {
        let quit = commands.iter().any(|command| matches!(command, ScreenCommand::Quit));
        self.run_commands(commands);
        !quit
    }

    /// F6: switch between the custom title bar and the native frame (not fullscreen).
    fn toggle_frame(&mut self) {
        self.framed = !self.framed;
        let was_maximized = self.window.is_maximized();
        self.window.set_bordered(self.framed);
        window_chrome::set_resizable(&self.window);
        if was_maximized {
            self.window.maximize();
        }
        self.apply_scale(true);
    }

    /// Shows the cursor the UI asked for (`love.mouse.setCursor`).
    fn apply_cursor(&mut self, name: &'static str) {
        if name == self.shown_cursor {
            return;
        }
        self.shown_cursor = name;
        let cursor = self
            .cursors
            .entry(name)
            .or_insert_with(|| system_cursor(name).and_then(|cursor| Cursor::from_system(cursor).ok()));
        match cursor {
            Some(cursor) => cursor.set(),
            None => {
                if let Some(Some(arrow)) = self.cursors.get("arrow") {
                    arrow.set();
                } else if let Ok(arrow) = Cursor::from_system(SystemCursor::Arrow) {
                    arrow.set();
                    self.cursors.insert("arrow", Some(arrow));
                }
            }
        }
    }

    /// One frame: update every screen, draw the top one, the title bar and the overlays, present.
    fn update_and_draw(&mut self, dt: f32) -> Result<bool, String> {
        self.apply_scale(false);
        let (physical_width, physical_height) = self.window.size_in_pixels();
        {
            let mut chrome = self.chrome.lock().unwrap();
            chrome.width = physical_width as f32;
            chrome.height = physical_height as f32;
            chrome.maximized = self.window.is_maximized();
            chrome.scale = self.applied_scale;
            chrome.pixel_density = self.window.pixel_density();
            chrome.framed = self.framed;
        }
        self.images.update(&mut self.renderer);

        let framed = self.framed;
        let maximized = self.window.is_maximized();
        let ((title_bar, list), mut commands) = self.with_frame(|frame, screens| {
            for screen in screens.iter_mut() {
                screen.background_update(frame, dt);
            }
            if let Some(screen) = screens.last_mut() {
                screen.update(frame, dt);
            }
            let (width, height) = (frame.width, frame.height);
            frame.ui.begin_frame(width, height, frame.scale, frame.time);
            // Like Flutter's Theme(data: AppTheme.light) around the game table: the screen and
            // its dialogs draw light regardless of the app's dark mode.
            frame.theme.force_light = screens.last().is_some_and(|screen| screen.forces_light_theme());
            let surface = frame.palette().surface;
            frame.fill_rect(0.0, 0.0, width, height, 0.0, surface);
            let top = if framed { 0.0 } else { TITLE_BAR_HEIGHT };
            frame.ui.content_top = top;
            let stack_depth = screens.len();
            if let Some(screen) = screens.last_mut() {
                frame.ui.push_clip(0.0, top, width, height - top);
                screen.draw(frame, 0.0, top, width, height - top);
                frame.ui.pop_clip();
            }
            let title_bar = if framed {
                None
            } else {
                let screen = screens.last();
                let title = screen.map_or("CardTable".to_string(), |screen| screen.title());
                let shows_back = screen.is_some_and(|screen| screen.shows_back()) && stack_depth > 1;
                Some(draw_title_bar(frame, &title, shows_back, maximized, width))
            };
            frame.draw_app_overlays();
            frame.draw_tooltip();
            frame.theme.force_light = false;
            let list = frame.ui.end_frame();
            (title_bar, list)
        });
        self.apply_cursor(self.ui.cursor());
        let mut keep_running = true;
        if let Some(title_bar) = title_bar {
            {
                let mut chrome = self.chrome.lock().unwrap();
                chrome.drag_from_x = title_bar.title_end;
                chrome.drag_to_x = title_bar.buttons_start;
            }
            match title_bar.action {
                Some(TitleAction::Close) => keep_running = self.screens_cancel_quit(),
                Some(TitleAction::Maximize) => {
                    if self.window.is_maximized() {
                        self.window.restore();
                    } else {
                        self.window.maximize();
                    }
                }
                Some(TitleAction::Theme) => {
                    self.theme.is_dark = !self.theme.is_dark;
                    profile::set_dark_mode(&mut self.settings, self.theme.is_dark);
                }
                Some(TitleAction::Back) => {
                    let (_, back_commands) = self.with_frame(|frame, screens| {
                        if let Some(screen) = screens.last_mut() {
                            screen.on_back(frame);
                        }
                    });
                    commands.extend(back_commands);
                }
                None => {}
            }
        }
        keep_running &= self.after_event(commands);
        self.fonts.flush(&mut self.renderer)?;
        let clear = self.theme.palette().surface;
        self.frame_count += 1;
        let capture_frame =
            std::env::var("GAMETABLE_CAPTURE_FRAME").ok().and_then(|frame| frame.parse().ok()).unwrap_or(30);
        let capture_now = self.capture_path.is_some() && self.frame_count == capture_frame;
        if let Some(captured) = self.renderer.render(&self.window, &list, clear, capture_now)?
            && let Some(path) = &self.capture_path
        {
            RgbaImage { width: captured.width, height: captured.height, pixels: captured.rgba }.save_png(path)?;
            return Ok(false);
        }
        Ok(keep_running)
    }

    /// Pushes a screen (development hooks open a screen directly at startup).
    pub fn push(&mut self, screen: Box<dyn Screen>) {
        self.run_commands(vec![ScreenCommand::Push(screen)]);
    }

    /// Pushes a screen that needs the frame context to be built.
    pub fn push_with_frame(&mut self, build: impl FnOnce(&Frame) -> Box<dyn Screen>) {
        let (screen, _) = self.with_frame(|frame, _| build(frame));
        self.push(screen);
    }

    pub fn video(&self) -> &sdl3::VideoSubsystem {
        &self.video
    }
}

/// LÖVE's system cursor names.
fn system_cursor(name: &str) -> Option<SystemCursor> {
    Some(match name {
        "arrow" => SystemCursor::Arrow,
        "ibeam" => SystemCursor::IBeam,
        "wait" => SystemCursor::Wait,
        "waitarrow" => SystemCursor::WaitArrow,
        "crosshair" => SystemCursor::Crosshair,
        "sizenwse" => SystemCursor::SizeNWSE,
        "sizenesw" => SystemCursor::SizeNESW,
        "sizewe" => SystemCursor::SizeWE,
        "sizens" => SystemCursor::SizeNS,
        "sizeall" => SystemCursor::SizeAll,
        "no" => SystemCursor::No,
        "hand" => SystemCursor::Hand,
        _ => return None,
    })
}

/// The custom title bar (`drawTitleBar`, port of `app_title_bar.dart`).
fn draw_title_bar(frame: &mut Frame, title: &str, shows_back: bool, maximized: bool, width: f32) -> TitleBarResult {
    let foreground = fixed::TITLE_BAR_FOREGROUND;
    frame.fill_rect(0.0, 0.0, width, TITLE_BAR_HEIGHT, 0.0, fixed::TITLE_BAR);
    let mut action = None;
    let mut x = 16.0;
    let button = |id: &'static str, tooltip: &'static str| IconButtonOptions {
        id: Some(id),
        tooltip: Some(tooltip),
        color: Some(foreground),
        ..IconButtonOptions::default()
    };
    if shows_back {
        if frame.icon_button("arrow_back", 8.0, 8.0, &button("title:back", "Back")) {
            action = Some(TitleAction::Back);
        }
        x = 56.0;
    }
    let title_options = TextOptions::new().medium().middle_of(TITLE_BAR_HEIGHT).color(fixed::WHITE);
    frame.text(title, x, 0.0, title_options);
    let title_end = x + frame.text_width(title, 14.0, crate::ui::theme::Weight::Medium) + 8.0;
    let mut button_x = width - 8.0 - 40.0;
    if frame.icon_button("close", button_x, 8.0, &button("title:close", "Close CardTable")) {
        action = Some(TitleAction::Close);
    }
    button_x -= 48.0;
    let (maximize_icon, maximize_tooltip) =
        if maximized { ("filter_none", "Restore") } else { ("crop_square", "Maximize") };
    if frame.icon_button(maximize_icon, button_x, 8.0, &button("title:max", maximize_tooltip)) {
        action = Some(TitleAction::Maximize);
    }
    button_x -= 48.0;
    let (theme_icon, theme_tooltip) =
        if frame.theme.is_dark { ("light_mode", "Switch to light mode") } else { ("dark_mode", "Switch to dark mode") };
    if frame.icon_button(theme_icon, button_x, 8.0, &button("title:theme", theme_tooltip)) {
        action = Some(TitleAction::Theme);
    }
    TitleBarResult { action, title_end, buttons_start: button_x - 4.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options() {
        let options = Options::parse(["--identity", "p2", "--dark", "--no-discord"].map(String::from));
        assert_eq!(options.identity.as_deref(), Some("p2"));
        assert_eq!(options.dark, Some(true));
        assert!(options.no_discord && !options.force_update_check);
        assert_eq!(Options::parse(["--light"].map(String::from)).dark, Some(false));
    }

    #[test]
    fn scale_is_capped_by_the_desktop() {
        assert_eq!(effective_scale(1.0, 1.0, Some((2560.0, 1440.0))), 1.0);
        assert_eq!(effective_scale(1.5, 2.0, Some((2560.0, 1440.0))), 1440.0 / 640.0);
        assert_eq!(effective_scale(1.0, 0.1, None), 0.5);
    }

    #[test]
    fn ui_scale_setting_round_trip() {
        let mut settings = Settings::in_memory();
        assert_eq!(ui_scale_setting(&settings), 1.0);
        set_ui_scale_setting(&mut settings, 1.25);
        assert_eq!(ui_scale_setting(&settings), 1.25);
        set_ui_scale_setting(&mut settings, 1.0);
        assert!(settings.get(keys::UI_SCALE).is_none());
    }

    #[test]
    fn profile_defaults() {
        let profile = PlayerProfile::load(&Settings::in_memory());
        assert_eq!((profile.name.as_str(), profile.color), ("Player", 0xFF45_5A64));
        let spaced = PlayerProfile { name: "  ".to_string(), ..profile };
        assert_eq!(spaced.display_name(), "Player");
    }

    #[test]
    fn love_cursor_names() {
        assert_eq!(system_cursor("hand"), Some(SystemCursor::Hand));
        assert_eq!(system_cursor("ibeam"), Some(SystemCursor::IBeam));
        assert_eq!(system_cursor("bogus"), None);
    }
}
