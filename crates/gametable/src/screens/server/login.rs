//! Sign in to, or create an account on, a CardTable server. With "Remember me", the saved
//! sign-in (a token) is tried first so the player goes straight to the lobby. Port of
//! `src/ui/screens/server/login.lua`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gametable_core::json::{JsonObject, JsonValue};
use gametable_core::util::trim_lua_whitespace;

use crate::app::{Frame, Screen, ScreenCommand};
use crate::data::profile::resize_avatar_bytes;
use crate::net::server_client::{
    ServerEvent, SharedServer, clear_session, forget_session, password_error, password_hash, response_message,
    response_ok, save_session, saved_session,
};
use crate::net::server_models::ServerUser;
use crate::screens::file_picker::{FilePicker, FilePickerResponse, IMAGE_FILTERS, PickerMode};
use crate::screens::server::lobby::ServerLobby;
use crate::ui::components::AvatarSource;
use crate::ui::context::RegionOptions;
use crate::ui::draw::TextOptions;
use crate::ui::text_field::TextFieldOptions;
use crate::ui::theme::{Weight, fixed};
use crate::ui::widgets::{ButtonOptions, ButtonVariant, Choice, IconButtonOptions, ScrollbarStyle, ToggleOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginMode {
    SignIn,
    Create,
}

/// A request waiting for its answer, with what it was.
#[derive(Debug, Clone)]
enum Waiting {
    Resume { rid: u64, email: Option<String> },
    SignIn { rid: u64, email: String },
    Create { rid: u64, email: String },
}

pub struct ServerLogin {
    server: SharedServer,
    mode: LoginMode,
    email: String,
    password: String,
    confirm: String,
    remember: bool,
    name: String,
    color: u32,
    avatar_path: Option<String>,
    error: Option<String>,
    busy: Option<&'static str>,
    waiting: Option<Waiting>,
    avatar_picker: Option<FilePicker>,
}

impl ServerLogin {
    pub fn new(frame: &mut Frame, server: SharedServer) -> ServerLogin {
        let key = server.borrow().key.clone();
        let mut saved = saved_session(frame.settings, &key);
        if saved.as_ref().is_some_and(|session| !session.remember) {
            // Saved before Remember me existed: don't sign in without asking.
            clear_session(frame.settings, &key);
            saved = None;
        }
        let mut login = ServerLogin {
            server,
            mode: LoginMode::SignIn,
            email: saved.as_ref().and_then(|session| session.email.clone()).unwrap_or_default(),
            password: String::new(),
            confirm: String::new(),
            remember: saved.is_some(),
            name: frame.profile.display_name(),
            color: frame.profile.color,
            avatar_path: frame.profile.avatar_path.clone(),
            error: None,
            busy: None,
            waiting: None,
            avatar_picker: None,
        };
        if let Some(session) = saved
            && let Some(token) = session.token
        {
            login.busy = Some("Signing in...");
            let rid = login.server.borrow_mut().request("resume", JsonObject::new().with("token", token));
            login.waiting = Some(Waiting::Resume { rid, email: session.email });
        }
        login
    }

    /// `L:signedIn(r, email, remember)`.
    fn signed_in(&mut self, frame: &mut Frame, response: &JsonValue, email: Option<String>, remember: bool) {
        let user = response.get("user").map(ServerUser::parse).unwrap_or_default();
        let token = response.get("token").and_then(JsonValue::as_str).unwrap_or("").to_string();
        let key = {
            let mut server = self.server.borrow_mut();
            server.token = Some(token.clone());
            server.me = Some(user.clone());
            server.key.clone()
        };
        if remember {
            save_session(frame.settings, &key, &token, &email.unwrap_or(user.email));
        } else {
            clear_session(frame.settings, &key);
        }
        let lobby = ServerLobby::new(frame, self.server.clone());
        frame.commands.push(ScreenCommand::Replace(Box::new(lobby)));
    }

    fn submit_sign_in(&mut self) {
        let email = trim_lua_whitespace(&self.email).to_string();
        if email.is_empty() || self.password.is_empty() {
            self.error = Some("Enter your email address and password.".to_string());
            return;
        }
        self.error = None;
        self.busy = Some("Signing in...");
        let payload =
            JsonObject::new().with("email", email.as_str()).with("passwordHash", password_hash(&email, &self.password));
        let rid = self.server.borrow_mut().request("login", payload);
        self.waiting = Some(Waiting::SignIn { rid, email });
    }

    fn submit_create(&mut self) {
        let email = trim_lua_whitespace(&self.email).to_string();
        let valid_email = email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && !domain.is_empty() && !email.chars().any(char::is_whitespace) && !domain.contains('@')
        });
        if !valid_email {
            self.error = Some("Enter a valid email address.".to_string());
            return;
        }
        if let Some(error) = password_error(&self.password, None) {
            self.error = Some(error);
            return;
        }
        if self.password != self.confirm {
            self.error = Some("The passwords don't match.".to_string());
            return;
        }
        let name = trim_lua_whitespace(&self.name).to_string();
        if name.is_empty() {
            self.error = Some("Enter a player name.".to_string());
            return;
        }
        let mut payload = JsonObject::new()
            .with("email", email.as_str())
            .with("passwordHash", password_hash(&email, &self.password))
            .with("name", name.as_str())
            .with("color", f64::from(self.color));
        if let Some(path) = &self.avatar_path {
            let png = std::fs::read(path).ok().and_then(|bytes| resize_avatar_bytes(&bytes).ok());
            let Some(png) = png else {
                self.error = Some("Could not use that image.".to_string());
                return;
            };
            payload.insert("avatar", BASE64.encode(png));
        }
        self.error = None;
        self.busy = Some("Creating your account...");
        let rid = self.server.borrow_mut().request("register", payload);
        self.waiting = Some(Waiting::Create { rid, email });
    }

    fn finish_waiting(&mut self, frame: &mut Frame) {
        let Some(waiting) = self.waiting.clone() else { return };
        let rid = match &waiting {
            Waiting::Resume { rid, .. } | Waiting::SignIn { rid, .. } | Waiting::Create { rid, .. } => *rid,
        };
        let Some(response) = self.server.borrow_mut().take_response(rid) else { return };
        self.waiting = None;
        self.busy = None;
        match waiting {
            Waiting::Resume { email, .. } => {
                if response_ok(&response) {
                    self.signed_in(frame, &response, email, true);
                } else {
                    let key = self.server.borrow().key.clone();
                    forget_session(frame.settings, &key);
                    if response.get("error").and_then(JsonValue::as_str) != Some("disconnected") {
                        self.error = Some(response_message(&response, ""));
                    }
                }
            }
            Waiting::SignIn { email, .. } | Waiting::Create { email, .. } => {
                if response_ok(&response) {
                    let remember = self.remember;
                    self.signed_in(frame, &response, Some(email), remember);
                } else {
                    self.error = Some(response_message(&response, ""));
                }
            }
        }
    }
}

impl Screen for ServerLogin {
    fn title(&self) -> String {
        self.server.borrow().info.display_name().to_string()
    }

    fn on_back(&mut self, frame: &mut Frame) {
        self.server.borrow_mut().disconnect();
        frame.commands.push(ScreenCommand::Pop);
    }

    fn quit(&mut self, _frame: &mut Frame) -> bool {
        self.server.borrow_mut().disconnect();
        false
    }

    fn background_update(&mut self, frame: &mut Frame, _dt: f32) {
        self.server.borrow_mut().update(frame);
        let events = self.server.borrow_mut().take_events();
        if events.contains(&ServerEvent::Disconnected) {
            let message = self.server.borrow().disconnect_message();
            frame.commands.push(ScreenCommand::PopToHome(Some(message)));
        }
    }

    fn update(&mut self, frame: &mut Frame, _dt: f32) {
        self.finish_waiting(frame);
    }

    fn draw(&mut self, frame: &mut Frame, x: f32, y: f32, width: f32, height: f32) {
        let palette = frame.palette();
        let (info, key) = {
            let server = self.server.borrow();
            (server.info.clone(), server.key.clone())
        };
        let column_width = 400f32.min(width - 48.0);
        let column_x = (x + (width - column_width) / 2.0).floor();
        let offset = frame.begin_scroll("login:scroll", x, y, width, height);
        let mut row_y = y + 32.0 - offset;
        frame.text(info.display_name(), column_x, row_y, TextOptions::new().size(24.0).width(column_width).centered());
        row_y += 36.0;
        if !info.motd.is_empty() {
            let options = TextOptions::new().width(column_width).centered().color(palette.on_surface_variant);
            frame.text(&info.motd, column_x, row_y, options);
            row_y += frame.text_height(&info.motd, column_width, 14.0, Weight::Regular) + 8.0;
        }
        let options = TextOptions::new().size(12.0).width(column_width).centered().color(palette.on_surface_variant);
        frame.text(&key, column_x, row_y, options);
        row_y += 32.0;

        let busy = self.busy.is_some();
        if info.registration_open {
            let choices = [Choice::new(LoginMode::SignIn, "Sign In"), Choice::new(LoginMode::Create, "Create Account")];
            let mode = frame.segmented("login:mode", column_x, row_y, column_width, &choices, self.mode, None, busy);
            if mode != self.mode {
                self.mode = mode;
                self.error = None;
            }
            row_y += 40.0 + 24.0;
        } else {
            self.mode = LoginMode::SignIn;
        }

        let background = palette.surface;
        let email_options = TextFieldOptions {
            autofocus: self.email.is_empty(),
            label_background: Some(background),
            disabled: busy,
            ..TextFieldOptions::label("Email address")
        };
        let email_field =
            frame.text_field("login:email", column_x, row_y, column_width, &mut self.email, &email_options);
        row_y += 48.0 + 16.0;
        let password_options = TextFieldOptions {
            password: true,
            autofocus: !self.email.is_empty(),
            label_background: Some(background),
            disabled: busy,
            ..TextFieldOptions::label("Password")
        };
        let password_field =
            frame.text_field("login:password", column_x, row_y, column_width, &mut self.password, &password_options);
        row_y += 48.0 + 16.0;

        let remember_options = ToggleOptions {
            label: Some("Remember me"),
            disabled: busy,
            tooltip: Some("Sign in automatically next time"),
            ..ToggleOptions::default()
        };
        if self.mode == LoginMode::SignIn {
            self.remember = frame.checkbox("login:remember", column_x, row_y, self.remember, &remember_options);
            row_y += 18.0 + 24.0;
            if let Some(error) = self.error.clone() {
                frame.text(&error, column_x, row_y, TextOptions::new().width(column_width).color(fixed::RED));
                row_y += frame.text_height(&error, column_width, 14.0, Weight::Regular) + 12.0;
            }
            let label = self.busy.unwrap_or("Sign In");
            let options = ButtonOptions { disabled: busy, id: Some("login:go"), ..ButtonOptions::default() };
            let clicked = frame.button(label, column_x, row_y, Some(column_width), Some(48.0), &options);
            if clicked || ((email_field.submitted || password_field.submitted) && !busy) {
                self.submit_sign_in();
            }
            row_y += 48.0 + 16.0;
            let options =
                TextOptions::new().width(column_width).centered().size(12.0).color(palette.on_surface_variant);
            frame.text("Forgot your password? Ask a server admin to reset it.", column_x, row_y, options);
            row_y += 32.0;
        } else {
            let confirm_options = TextFieldOptions {
                password: true,
                label_background: Some(background),
                disabled: busy,
                ..TextFieldOptions::label("Confirm password")
            };
            let confirm_field =
                frame.text_field("login:confirm", column_x, row_y, column_width, &mut self.confirm, &confirm_options);
            row_y += 48.0 + 28.0;
            frame.text("Your player profile on this server", column_x, row_y, TextOptions::new().medium());
            row_y += 28.0;
            let size = 96.0;
            let region = frame.ui.region(
                "login:avatar",
                column_x,
                row_y,
                size,
                size,
                RegionOptions::hand().with_tooltip(Some("Choose an image")),
            );
            let source = self.avatar_path.as_deref().map(AvatarSource::Path);
            frame.avatar(column_x, row_y, size, self.color, source, true);
            if region.hovered {
                frame.fill_rect(
                    column_x,
                    row_y,
                    size,
                    size,
                    0.0,
                    crate::graphics::draw_list::Color::rgba(1.0, 1.0, 1.0, 0.08),
                );
            }
            let mut pick = region.clicked;
            let button_x = column_x + size + 16.0;
            let name_options = TextFieldOptions {
                label_background: Some(background),
                max_length: Some(24),
                disabled: busy,
                ..TextFieldOptions::label("Player name")
            };
            frame.text_field("login:name", button_x, row_y, column_width - size - 16.0, &mut self.name, &name_options);
            let pick_options = ButtonOptions::variant(ButtonVariant::Text).icon("image").id("login:pick");
            pick |= frame.button("Choose Image...", button_x - 12.0, row_y + 56.0, None, Some(40.0), &pick_options);
            if pick {
                self.avatar_picker = Some(FilePicker::new(
                    frame,
                    PickerMode::File,
                    Some("Choose an avatar image"),
                    Some(IMAGE_FILTERS),
                    None,
                ));
            }
            if self.avatar_path.is_some()
                && frame.icon_button(
                    "delete_outline",
                    column_x + column_width - 40.0,
                    row_y + 56.0,
                    &IconButtonOptions::tooltip("Remove Image").id("login:rmimg"),
                )
            {
                self.avatar_path = None;
            }
            row_y += size + 20.0;
            let (color, color_height) = frame.color_field(
                "login:color",
                column_x,
                row_y,
                column_width,
                self.color,
                Some("Player Color"),
                Some(background),
            );
            self.color = color;
            row_y += color_height + 16.0;
            self.remember = frame.checkbox("login:remember", column_x, row_y, self.remember, &remember_options);
            row_y += 18.0 + 24.0;
            if let Some(error) = self.error.clone() {
                frame.text(&error, column_x, row_y, TextOptions::new().width(column_width).color(fixed::RED));
                row_y += frame.text_height(&error, column_width, 14.0, Weight::Regular) + 12.0;
            }
            let label = self.busy.unwrap_or("Create Account");
            let options = ButtonOptions { disabled: busy, id: Some("login:create"), ..ButtonOptions::default() };
            let clicked = frame.button(label, column_x, row_y, Some(column_width), Some(48.0), &options);
            if clicked || (confirm_field.submitted && !busy) {
                self.submit_create();
            }
            row_y += 48.0 + 16.0;
            let note = "Player names are unique on this server. Your password is never sent as plain text, but this \
                        connection isn't encrypted -- use a password you don't use anywhere else.";
            let options = TextOptions::new().width(column_width).size(12.0).color(palette.on_surface_variant);
            frame.text(note, column_x, row_y, options);
            row_y += frame.text_height(note, column_width, 12.0, Weight::Regular) + 24.0;
        }
        frame.end_scroll("login:scroll", row_y + offset - y, &ScrollbarStyle::default());

        if let Some(picker) = &mut self.avatar_picker {
            match picker.show(frame) {
                FilePickerResponse::Open => {}
                FilePickerResponse::Cancelled => self.avatar_picker = None,
                FilePickerResponse::Picked(mut paths) => {
                    self.avatar_picker = None;
                    self.avatar_path = Some(paths.remove(0));
                }
            }
        }
    }
}
