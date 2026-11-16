//! The Home screen. Port of `src/ui/screens/home.lua` (from `home_screen.dart`, since extended
//! with an inline Join panel — host/port fields right on Home with a one-tap reconnect to the
//! last server — and a two-column layout that becomes a Join/Host toggle when the window is
//! narrow).

use gametable_core::protocol::DEFAULT_GAME_PORT;

use crate::app::{Frame, Screen, ScreenCommand};
use crate::data::profile::{self, RecentServer};
use crate::screens::deck_editor::DeckEditorGameSelect;
use crate::screens::edit_player_settings::EditPlayerSettings;
use crate::screens::info_dialogs::{show_controls_dialog, show_support_dialog};
use crate::screens::multiplayer::Join;
use crate::screens::not_ported::NotPortedYet;
use crate::screens::player_count::PlayerCount;
use crate::screens::recent_servers::recent_servers_dropdown;
use crate::screens::settings_dialog::SettingsDialog;
use crate::ui::components::{AvatarSource, BannerKind};
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::{Weight, with_alpha};
use crate::ui::widgets::{ButtonOptions, ButtonVariant, Choice, IconButtonOptions};
use crate::version;
use gametable_core::util::trim_lua_whitespace;

/// Below this content width there isn't room for both columns side by side.
const WIDE_BREAKPOINT: f32 = 760.0;
const LOGO_MAX_WIDTH: f32 = 220.0;
const RECONNECT_BANNER_HEIGHT: f32 = 76.0;
/// The left column sets the wide layout's height: it's taller than the join card with or
/// without a recent server.
const LEFT_COLUMN_HEIGHT: f32 = 16.0 + 160.0 + 12.0 + 30.0 + 24.0 + 48.0 + 16.0 + 40.0 + 40.0;

/// `Home.joinFieldsHeight(hasRecent)`: the reconnect banner and its divider line when there's a
/// recent server, then the address and port fields and the Connect button.
fn join_fields_height(has_recent: bool) -> f32 {
    (if has_recent { RECONNECT_BANNER_HEIGHT + 16.0 + 16.0 + 16.0 } else { 0.0 }) + 60.0 + 72.0 + 48.0
}

fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Join or Host in the narrow layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HomeTab {
    Join,
    Host,
}

/// A dialog Home has open.
enum HomeDialog {
    EditPlayer(EditPlayerSettings),
    Settings(SettingsDialog),
    Support,
    /// Only through the development hook: in the app the Controls dialog opens from the in-game
    /// Game Menu.
    Controls,
}

pub struct Home {
    /// The orange banner (`App.popToHome(message)`).
    message: Option<String>,
    tab: HomeTab,
    join_host: String,
    join_port: String,
    recent_open: bool,
    dialog: Option<HomeDialog>,
    /// The release tag of an available update (the updater arrives with Phase 11).
    update_available: Option<String>,
    /// Development hook (`GAMETABLE_HOME_OPEN=settings|support|editplayer|controls|recent`):
    /// opened on the first frame.
    development_open: Option<String>,
}

impl Home {
    pub fn new(message: Option<String>) -> Home {
        Home {
            message,
            tab: HomeTab::Join,
            join_host: String::new(),
            join_port: DEFAULT_GAME_PORT.to_string(),
            recent_open: false,
            dialog: None,
            update_available: None,
            development_open: std::env::var("GAMETABLE_HOME_OPEN").ok(),
        }
    }

    fn open_development_dialog(&mut self, frame: &mut Frame) {
        match self.development_open.take().as_deref() {
            Some("settings") => self.dialog = Some(HomeDialog::Settings(SettingsDialog::new())),
            Some("support") => self.dialog = Some(HomeDialog::Support),
            Some("controls") => self.dialog = Some(HomeDialog::Controls),
            Some("editplayer") => self.dialog = Some(HomeDialog::EditPlayer(EditPlayerSettings::new(frame))),
            Some("recent") => self.recent_open = true,
            _ => {}
        }
    }

    /// Opens the Join screen for this address, which connects straight away (an unparsable port
    /// means the default port).
    fn connect(frame: &mut Frame, host: &str, port: &str) {
        let port = trim_lua_whitespace(port).parse().ok();
        let join = Join::new(Some(trim_lua_whitespace(host)), port, true);
        frame.commands.push(ScreenCommand::Push(Box::new(join)));
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_profile(
        &mut self,
        frame: &mut Frame,
        x: f32,
        y: f32,
        width: f32,
        avatar_size: f32,
        name_gap: f32,
        name_size: f32,
        name_height: f32,
    ) {
        let palette = frame.palette();
        let block_height = avatar_size + name_gap + name_height;
        let region = frame.ui.region("home:profile", x, y, width, block_height, RegionOptions::hand());
        if region.hovered {
            frame.fill_rect(x, y, width, block_height, 12.0, with_alpha(palette.on_surface, 0.05));
        }
        let avatar_x = x + (width - avatar_size) / 2.0;
        let avatar_path = frame.profile.avatar_path.clone();
        let color = frame.profile.color;
        frame.avatar(avatar_x, y, avatar_size, color, avatar_path.as_deref().map(AvatarSource::Path), true);
        let name = frame.profile.display_name();
        let name_options = TextOptions::new().size(name_size).width(width).centered().ellipsis();
        frame.text(&name, x, y + avatar_size + name_gap, name_options);
        if region.clicked {
            self.dialog = Some(HomeDialog::EditPlayer(EditPlayerSettings::new(frame)));
        }
    }

    /// The arrow at the right end of the address field opening the recently joined servers;
    /// picking one fills in both fields.
    fn draw_recent_servers_popup(&mut self, frame: &mut Frame, recent: &[RecentServer], x: f32, y: f32, width: f32) {
        let picked = recent_servers_dropdown(frame, "home:join:recent", recent, x, y, width, &mut self.recent_open);
        if let Some(server) = picked {
            self.join_host = server.host;
            self.join_port = server.port.to_string();
        }
    }

    fn draw_join_fields(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, recent: &[RecentServer]) {
        let palette = frame.palette();
        let mut row_y = y;
        if let Some(last) = recent.first() {
            let banner_height = RECONNECT_BANNER_HEIGHT;
            let region = frame.ui.region("home:reconnect", x, row_y, width, banner_height, RegionOptions::hand());
            let foreground = palette.on_primary_container;
            frame.fill_rect(x, row_y, width, banner_height, 14.0, palette.primary_container);
            if region.hovered {
                frame.fill_rect(x, row_y, width, banner_height, 14.0, with_alpha(foreground, 0.08));
            }
            frame.icon("history", x + 14.0, row_y + 8.0, 20.0, foreground);
            let small_bold = TextOptions::new().size(10.5).bold().color(with_alpha(foreground, 0.7));
            frame.text("LAST PLAYED", x + 46.0, row_y + 10.0, small_bold);
            let address = format!("{} : {}", last.host, last.port);
            let address_options =
                TextOptions::new().size(14.5).medium().width(width).centered().ellipsis().color(foreground);
            frame.text(&address, x, row_y + 26.0, address_options);
            frame.text("CLICK TO RECONNECT", x, row_y + 52.0, small_bold.width(width).centered());
            if region.clicked {
                let (host, port) = (last.host.clone(), last.port.to_string());
                Self::connect(frame, &host, &port);
            }
            row_y += banner_height + 16.0;
            let divider_options =
                TextOptions::new().size(10.5).bold().width(width).centered().color(palette.on_surface_variant);
            frame.text("OR JOIN ANOTHER SERVER", x, row_y, divider_options);
            row_y += 16.0 + 16.0;
        }
        let host_options = TextFieldOptions {
            hint: Some("e.g. 192.168.1.42 -- blank = 127.0.0.1"),
            trailing_width: if recent.is_empty() { 0.0 } else { 36.0 },
            ..TextFieldOptions::label("Host or server IP address")
        };
        let host_field = frame.text_field("home:join:ip", x, row_y, width, &mut self.join_host, &host_options);
        if !recent.is_empty() {
            self.draw_recent_servers_popup(frame, recent, x, row_y, width);
        }
        row_y += 60.0;
        let port_options = TextFieldOptions { filter: Some(&is_digits), ..TextFieldOptions::label("Port") };
        let port_field = frame.text_field("home:join:port", x, row_y, width, &mut self.join_port, &port_options);
        row_y += 72.0;
        let options = ButtonOptions::variant(ButtonVariant::Filled).id("home:connect");
        let connect_clicked = frame.button("Connect", x, row_y, Some(width), Some(48.0), &options);
        if connect_clicked || host_field.submitted || port_field.submitted {
            let (host, port) = (self.join_host.clone(), self.join_port.clone());
            Self::connect(frame, &host, &port);
        }
    }

    fn push_host(frame: &mut Frame) {
        frame.commands.push(ScreenCommand::Push(Box::new(PlayerCount::host())));
    }

    fn push_practice(frame: &mut Frame) {
        frame.commands.push(ScreenCommand::Push(Box::new(PlayerCount::practice())));
    }

    fn push_deck_editor(frame: &mut Frame) {
        let screen = DeckEditorGameSelect::new(frame);
        frame.commands.push(ScreenCommand::Push(Box::new(screen)));
    }

    /// Two columns: profile + Host Game on the left, the Join card on the right.
    #[allow(clippy::too_many_arguments)]
    fn draw_wide(
        &mut self,
        frame: &mut Frame,
        x: f32,
        y: f32,
        width: f32,
        left_width: f32,
        gap: f32,
        right_width: f32,
        recent: &[RecentServer],
    ) {
        let palette = frame.palette();
        let total_width = left_width + gap + right_width;
        let left_x = (x + (width - total_width) / 2.0).floor();
        let right_x = left_x + left_width + gap;

        let mut left_y = y + 16.0;
        self.draw_profile(frame, left_x, left_y, left_width, 160.0, 12.0, 18.0, 30.0);
        left_y += 160.0 + 12.0 + 30.0 + 24.0;
        let outlined = ButtonOptions::variant(ButtonVariant::Outlined);
        if frame.button("Host Game", left_x, left_y, Some(left_width), Some(48.0), &outlined) {
            Self::push_host(frame);
        }
        left_y += 48.0 + 16.0;
        if frame.button("Practice Offline", left_x, left_y, Some(left_width), Some(40.0), &ButtonOptions::text()) {
            Self::push_practice(frame);
        }
        left_y += 40.0;
        if frame.button("Deck Editor", left_x, left_y, Some(left_width), Some(40.0), &ButtonOptions::text()) {
            Self::push_deck_editor(frame);
        }

        // The Join a Game card: the hero action.
        let card_padding = 24.0;
        let card_height = card_padding * 2.0 + 28.0 + 16.0 + join_fields_height(!recent.is_empty());
        frame.card(right_x, y, right_width, card_height, Some(20.0), None, true);
        let mut join_y = y + card_padding;
        frame.icon("sports_esports", right_x + card_padding, join_y, 22.0, palette.primary);
        let title_options = TextOptions::new().size(18.0).color(palette.on_surface);
        frame.text("Join a Game", right_x + card_padding + 30.0, join_y, title_options);
        join_y += 28.0 + 16.0;
        self.draw_join_fields(frame, right_x + card_padding, join_y, right_width - card_padding * 2.0, recent);
    }

    /// One column with a Join/Host toggle in place of the two columns.
    fn draw_narrow(
        &mut self,
        frame: &mut Frame,
        x: f32,
        y: f32,
        width: f32,
        column_width: f32,
        recent: &[RecentServer],
    ) {
        let palette = frame.palette();
        let column_x = (x + (width - column_width) / 2.0).floor();
        let mut row_y = y;
        self.draw_profile(frame, column_x, row_y, column_width, 64.0, 10.0, 16.0, 20.0);
        row_y += 64.0 + 10.0 + 20.0 + 20.0;

        let choices = [Choice::new(HomeTab::Join, "Join"), Choice::new(HomeTab::Host, "Host")];
        self.tab = frame.segmented("home:tab", column_x, row_y, column_width, &choices, self.tab, None, false);
        row_y += 40.0 + 16.0;

        if self.tab == HomeTab::Host {
            frame.icon("sports_esports", column_x + column_width / 2.0 - 15.0, row_y, 30.0, palette.primary);
            let mut host_y = row_y + 30.0 + 12.0;
            let filled = ButtonOptions::variant(ButtonVariant::Filled);
            if frame.button("Host Game", column_x, host_y, Some(column_width), Some(48.0), &filled) {
                Self::push_host(frame);
            }
            host_y += 48.0 + 12.0;
            let helper = "Starts a game and shows your address so others can connect.";
            let helper_options =
                TextOptions::new().width(column_width).size(12.5).centered().color(palette.on_surface_variant);
            frame.text(helper, column_x, host_y, helper_options);
        } else {
            self.draw_join_fields(frame, column_x, row_y, column_width, recent);
        }
        // The same block height for either tab, so the rows below don't jump.
        row_y += join_fields_height(!recent.is_empty()) + 24.0;

        if frame.button("Practice Offline", column_x, row_y, Some(column_width), Some(40.0), &ButtonOptions::text()) {
            Self::push_practice(frame);
        }
        row_y += 40.0;
        if frame.button("Deck Editor", column_x, row_y, Some(column_width), Some(40.0), &ButtonOptions::text()) {
            Self::push_deck_editor(frame);
        }
    }

    /// Settings, the Game Definition Editor and Support as small corner icons, since most
    /// players never need them.
    fn draw_corner_tools(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let button_y = y + height - 16.0 - 40.0;
        let mut button_x = x + width - 16.0 - 40.0;
        if frame.icon_button(
            "settings",
            button_x,
            button_y,
            &IconButtonOptions::tooltip("Settings").id("home:settings"),
        ) {
            self.dialog = Some(HomeDialog::Settings(SettingsDialog::new()));
        }
        button_x -= 44.0;
        let editor_options = IconButtonOptions::tooltip("Game Definition Editor").id("home:gamedef");
        if frame.icon_button("build", button_x, button_y, &editor_options) {
            let screen = NotPortedYet::boxed("Game Definition Editor", "The game definition editor", 9);
            frame.commands.push(ScreenCommand::Push(screen));
        }
        button_x -= 44.0;
        let support_options = IconButtonOptions::tooltip("Support the Author").id("home:support");
        if frame.icon_button("local_cafe", button_x, button_y, &support_options) {
            self.dialog = Some(HomeDialog::Support);
        }
    }

    fn show_dialog(&mut self, frame: &mut Frame) {
        let still_open = match &mut self.dialog {
            None => return,
            Some(HomeDialog::EditPlayer(dialog)) => dialog.show(frame),
            Some(HomeDialog::Settings(dialog)) => dialog.show(frame),
            Some(HomeDialog::Support) => show_support_dialog(frame),
            Some(HomeDialog::Controls) => show_controls_dialog(frame),
        };
        if !still_open {
            self.dialog = None;
        }
    }
}

impl Screen for Home {
    fn shows_back(&self) -> bool {
        false
    }

    fn set_message(&mut self, message: Option<String>) {
        self.message = message;
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        if self.development_open.is_some() {
            self.open_development_dialog(frame);
        }
        let palette = frame.palette();
        let wide = width >= WIDE_BREAKPOINT;
        let (left_width, gap, right_width) = (260.0, 56.0, 400.0);
        let narrow_column_width = (340.0_f32).min(width - 48.0);
        let banner_width = if wide { left_width + gap + right_width } else { narrow_column_width };
        let recent = profile::recent_servers(frame.settings);

        // The logo, centred above everything else.
        let logo_name = if frame.theme.is_dark { "logo.png" } else { "logo_light.png" };
        let logo_path = frame.assets_folder.join(logo_name).to_string_lossy().into_owned();
        let logo = frame.images.get(&logo_path);
        let logo_width = LOGO_MAX_WIDTH.min(banner_width);
        let logo_height = logo.map_or(0.0, |logo| logo_width * logo.height / logo.width);
        let logo_block_height = if logo.is_some() { logo_height + 24.0 } else { 0.0 };

        // Measure the column first so it can be centred vertically.
        let update_text =
            self.update_available.as_ref().map(|tag| format!("Update available: {tag} \u{2014} click to install"));
        let mut banners_height = 0.0;
        for text in update_text.iter().chain(self.message.iter()) {
            banners_height += frame.text_height(text, banner_width - 24.0, 14.0, Weight::Regular) + 24.0 + 24.0;
        }
        let content_height = if wide {
            LEFT_COLUMN_HEIGHT
        } else {
            (64.0 + 10.0 + 20.0) + 20.0 + 40.0 + 16.0 + join_fields_height(!recent.is_empty()) + 24.0 + 40.0 + 40.0
        };
        let total = logo_block_height + banners_height + content_height;
        let banner_x = (x + (width - banner_width) / 2.0).floor();
        let mut row_y = (y + (16.0_f32).max((height - total) / 2.0)).floor();

        if let Some(logo) = logo {
            frame.draw_image(logo, x + (width - logo_width) / 2.0, row_y, logo_width / logo.width, 1.0);
            row_y += logo_height + 24.0;
        }
        if let Some(text) = &update_text {
            let (banner_height, _clicked) = frame.banner(text, banner_x, row_y, banner_width, BannerKind::Info, true);
            row_y += banner_height + 24.0;
        }
        if let Some(message) = self.message.clone() {
            let (banner_height, _) = frame.banner(&message, banner_x, row_y, banner_width, BannerKind::Warning, false);
            row_y += banner_height + 24.0;
        }

        if wide {
            self.draw_wide(frame, x, row_y, width, left_width, gap, right_width, &recent);
        } else {
            self.draw_narrow(frame, x, row_y, width, narrow_column_width, &recent);
        }

        self.draw_corner_tools(frame, x, y, width, height);
        let build = format!("Build {}", version::BUILD);
        frame.text(&build, x + 8.0, y + height - 8.0 - 16.0, TextOptions::new().size(12.0).color(palette.disabled));
        self.show_dialog(frame);
    }
}
