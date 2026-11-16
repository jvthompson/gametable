//! Accounts: register, login, resume, logout, profile changes and avatars. Port of `auth.go`.
//!
//! Clients never send a plain password: they send `passwordHash`, a SHA-256 hex digest, which
//! the server stores as a bcrypt hash.

use std::sync::Arc;

use base64::Engine;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};

use gametable_core::json::{JsonObject, JsonValue};

use super::{ClientId, Request, Server};
use crate::log;
use crate::payload::sorted_object;
use crate::store::{StoreError, User, UserUpdate, local_calendar_date, unix_now};

const MAX_AVATAR_BYTES: usize = 512 << 10;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// Go's `bcrypt.DefaultCost`.
const BCRYPT_COST: u32 = 10;

/// Go's `base64.StdEncoding`: padding required, stray low bits in the last character tolerated.
const GO_STANDARD_BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_allow_trailing_bits(true)
        .with_decode_padding_mode(DecodePaddingMode::RequireCanonical),
);

/// Decodes base64 the way Go's `base64.StdEncoding.DecodeString` does (line breaks are skipped).
pub(super) fn go_base64_decode(text: &str) -> Result<Vec<u8>, base64::DecodeError> {
    let without_line_breaks: String = text.chars().filter(|&character| !matches!(character, '\r' | '\n')).collect();
    GO_STANDARD_BASE64.decode(without_line_breaks)
}

pub(super) fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

pub(super) fn is_valid_email(email: &str) -> bool {
    let at_position = email.find('@');
    at_position.is_some_and(|at| at > 0 && at < email.len() - 1)
        && email.len() <= 254
        && !email.contains([' ', '\t', '\r', '\n'])
}

/// Player-name rules: 1-24 characters, no control characters, no leading `__`. Returns the
/// trimmed name and, when it is not allowed, the reason.
pub fn valid_player_name(name: &str) -> (String, Option<&'static str>) {
    let name = name.trim().to_string();
    let problem = if name.is_empty() {
        Some("Enter a player name.")
    } else if name.chars().count() > 24 {
        Some("Player names can be at most 24 characters.")
    } else if name.chars().any(char::is_control) {
        Some("Player names can't contain control characters.")
    } else if name.starts_with("__") {
        Some("That name is reserved.")
    } else {
        None
    };
    (name, problem)
}

/// A client password hash: 64 hex digits.
pub(super) fn is_valid_password_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Decodes a base64 PNG avatar. `Ok(None)` for an empty string.
fn decode_avatar(base64_text: &str) -> Result<Option<Vec<u8>>, &'static str> {
    if base64_text.is_empty() {
        return Ok(None);
    }
    // Go's decoder skips line breaks.
    let without_line_breaks: String =
        base64_text.chars().filter(|&character| character != '\r' && character != '\n').collect();
    let bytes = GO_STANDARD_BASE64.decode(without_line_breaks).map_err(|_| "The avatar image is not valid.")?;
    if bytes.len() > MAX_AVATAR_BYTES {
        return Err("The avatar image is too large.");
    }
    if !bytes.starts_with(PNG_SIGNATURE) {
        return Err("The avatar must be a PNG image.");
    }
    Ok(Some(bytes))
}

/// A bcrypt hash in Go's `$2a$` format.
pub(super) fn bcrypt_hash(password_hash: &str) -> Option<String> {
    bcrypt::hash_with_result(password_hash, BCRYPT_COST)
        .ok()
        .map(|parts| parts.format_for_version(bcrypt::Version::TwoA))
}

fn bcrypt_matches(stored_hash: &str, password_hash: &str) -> bool {
    bcrypt::verify(password_hash, stored_hash).unwrap_or(false)
}

/// What a client learns about its own account.
pub(crate) fn public_self(user: &User) -> JsonValue {
    sorted_object([
        ("id", user.id.as_str().into()),
        ("name", user.name.as_str().into()),
        ("email", user.email.as_str().into()),
        ("color", user.color.into()),
        ("avatarHash", user.avatar_hash.as_str().into()),
        ("isAdmin", user.is_admin.into()),
        ("tokens", user.tokens.into()),
    ])
}

impl Request<'_> {
    pub(super) fn handle_register(&self) {
        let fields = self.fields;
        if self.server.is_shutting_down() {
            self.fail("shuttingDown", "The server is shutting down.");
            return;
        }
        let email = normalize_email(&fields.text("email"));
        if !is_valid_email(&email) {
            self.fail("badEmail", "Enter a valid email address.");
            return;
        }
        let password_hash = fields.text("passwordHash");
        if !is_valid_password_hash(&password_hash) {
            self.fail("badPassword", "Invalid password.");
            return;
        }
        let (name, problem) = valid_player_name(&fields.text("name"));
        if let Some(problem) = problem {
            self.fail("badName", problem);
            return;
        }
        let avatar = match decode_avatar(&fields.text("avatar")) {
            Ok(avatar) => avatar,
            Err(problem) => {
                self.fail("badAvatar", problem);
                return;
            }
        };
        let settings = self.server.settings();
        if !settings.registration_open {
            let has_users = self.server.store().users().is_ok_and(|users| !users.is_empty());
            if has_users {
                self.fail("registrationClosed", "This server isn't accepting new accounts.");
                return;
            }
        }
        let Some(stored_hash) = bcrypt_hash(&password_hash) else {
            self.fail("internal", "Could not create the account.");
            return;
        };
        let starting_tokens = settings.starting_tokens.max(0);
        let created = self.server.store().create_user(
            &email,
            &name,
            &stored_hash,
            fields.integer("color"),
            avatar.as_deref(),
            starting_tokens,
        );
        let user = match created {
            Ok(user) => user,
            Err(StoreError::EmailTaken) => {
                self.fail("emailTaken", "An account with that email address already exists.");
                return;
            }
            Err(StoreError::NameTaken) => {
                self.fail("nameTaken", "That player name is taken.");
                return;
            }
            Err(error) => {
                log!(self.server, "register: {error}");
                self.fail("internal", "Could not create the account.");
                return;
            }
        };
        log!(
            self.server,
            "new account {:?} ({}){}",
            user.name,
            user.email,
            if user.is_admin { " [admin]" } else { "" }
        );
        self.authenticate(user);
    }

    pub(super) fn handle_login(&self) {
        let email = normalize_email(&self.fields.text("email"));
        let limiter_keys = [format!("ip:{}", self.connection.address()), format!("email:{email}")];
        let remaining_block = self.server.login_limiter.remaining_block(&limiter_keys);
        if !remaining_block.is_zero() {
            let wait_text = super::go_duration_text(remaining_block);
            self.fail("rateLimited", &format!("Too many failed attempts. Try again in {wait_text}."));
            return;
        }
        let user = self.server.store().user_by_email(&email);
        let user = match user {
            Ok(user) if bcrypt_matches(&user.pass_hash, &self.fields.text("passwordHash")) => user,
            _ => {
                self.server.login_limiter.record_failure(&limiter_keys);
                self.fail("badCredentials", "Incorrect email address or password.");
                return;
            }
        };
        self.server.login_limiter.record_success(&limiter_keys);
        self.authenticate(user);
    }

    pub(super) fn handle_resume(&self) {
        let token = self.fields.text("token");
        let user = self.server.store().session_user(&token);
        match user {
            Ok(user) => self.authenticate_with_token(user, token),
            Err(_) => self.fail("badToken", "Your saved sign-in has expired."),
        }
    }

    fn authenticate(&self, user: User) {
        if user.disabled {
            self.fail("disabled", "This account has been disabled.");
            return;
        }
        let token = self.server.store().new_session(&user.id);
        match token {
            Ok(token) => self.authenticate_with_token(user, token),
            Err(_) => self.fail("internal", "Could not sign in."),
        }
    }

    fn authenticate_with_token(&self, mut user: User, token: String) {
        if user.disabled {
            self.fail("disabled", "This account has been disabled.");
            return;
        }
        let server = self.server;
        if server.is_shutting_down() {
            self.fail("shuttingDown", "The server is shutting down.");
            return;
        }
        let settings = server.settings();
        if settings.is_collection_mode()
            && settings.allowance_period != "none"
            && let Ok((balance, true)) = server.grant_allowance_now(&user.id, &settings)
        {
            user.tokens = balance;
        }
        let replaced_client = {
            let mut state = server.state();
            if state.user_of(self.client_id).is_some() {
                drop(state);
                self.fail("alreadyLoggedIn", "Already signed in.");
                return;
            }
            let replaced_client = state.client_by_user_id.insert(user.id.clone(), self.client_id);
            if let Some(client) = state.clients.get_mut(&self.client_id) {
                client.user = Some(user.clone());
            }
            replaced_client
        };
        if let Some(old_client_id) = replaced_client {
            // The same account signing in again replaces the older connection.
            let (old_room_id, old_connection) = {
                let mut state = server.state();
                match state.clients.get_mut(&old_client_id) {
                    Some(old_client) => {
                        old_client.user = None;
                        (old_client.room_id.clone(), Some(old_client.connection.clone()))
                    }
                    None => (None, None),
                }
            };
            if let Some(room_id) = old_room_id {
                server.leave_room(old_client_id, &room_id, false);
            }
            if let Some(connection) = old_connection {
                connection.close_with("loggedInElsewhere");
            }
        }
        log!(server, "{} connected ({})", user.name, self.connection.address());
        self.reply(JsonObject::new().with("token", token).with("user", public_self(&user)));
        self.connection.send("lobbyState", server.lobby_state(&user));
        server.broadcast_users();
    }

    pub(super) fn handle_logout(&self) {
        let token = self.fields.text("token");
        if !token.is_empty() {
            let _ = self.server.store().delete_session(&token);
        }
        self.reply_empty();
        self.connection.close();
    }

    pub(super) fn handle_profile_update(&self, user: &User) {
        let fields = self.fields;
        let mut update = UserUpdate::default();
        if let Some(requested_name) = fields.optional_text("name") {
            let (name, problem) = valid_player_name(&requested_name);
            if let Some(problem) = problem {
                self.fail("badName", problem);
                return;
            }
            if self.server.is_in_running_room(self.client_id) && name != user.name {
                self.fail("inGame", "You can't change your name during a game.");
                return;
            }
            update.name = Some(name);
        }
        update.color = fields.optional_integer("color");
        if let Some(avatar_text) = fields.optional_text("avatar") {
            match decode_avatar(&avatar_text) {
                Ok(avatar) => update.avatar = Some(avatar.unwrap_or_default()),
                Err(problem) => {
                    self.fail("badAvatar", problem);
                    return;
                }
            }
        }
        let new_password_hash = fields.optional_text("passwordHash");
        if let Some(new_password_hash) = &new_password_hash {
            if !bcrypt_matches(&user.pass_hash, &fields.text("oldPasswordHash")) {
                self.fail("badCredentials", "Your current password is incorrect.");
                return;
            }
            if !is_valid_password_hash(new_password_hash) {
                self.fail("badPassword", "Invalid password.");
                return;
            }
            update.pass_hash = Some(bcrypt_hash(new_password_hash).unwrap_or_default());
        }
        let updated = self.server.store().update_user(&user.id, &update);
        if let Err(error) = updated {
            match error {
                StoreError::NameTaken => self.fail("nameTaken", "That player name is taken."),
                _ => self.fail("internal", "Could not update your profile."),
            }
            return;
        }
        let fresh_user = self.server.store().user_by_id(&user.id);
        let Ok(fresh_user) = fresh_user else {
            self.fail("internal", "Could not update your profile.");
            return;
        };
        self.server.refresh_user(&fresh_user);
        self.reply(JsonObject::new().with("user", public_self(&fresh_user)));
        if new_password_hash.is_some() {
            // Changing the password signs out every other session; issue a new token.
            let token = self.server.store().new_session(&user.id);
            if let Ok(token) = token {
                self.connection.send("newToken", sorted_object([("token", token.into())]));
            }
        }
    }

    pub(super) fn handle_avatar_get(&self) {
        let hash = self.fields.text("hash");
        let avatar = self.server.store().avatar_by_hash(&hash);
        match avatar {
            Ok(bytes) if !bytes.is_empty() => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
                self.reply(JsonObject::new().with("hash", hash).with("data", encoded));
            }
            _ => self.fail("notFound", "No such avatar."),
        }
    }
}

impl Server {
    /// Grants the periodic token allowance if a new day/week began since the last grant.
    pub(crate) fn grant_allowance_now(
        &self,
        user_id: &str,
        settings: &crate::store::ServerSettings,
    ) -> crate::store::StoreResult<(i64, bool)> {
        let now = unix_now();
        self.store().grant_allowance(
            user_id,
            settings.allowance_amount,
            &settings.allowance_period,
            local_calendar_date,
            now,
            local_calendar_date(now),
        )
    }

    /// Swaps in a reloaded account for an online client and tells everyone about name, color and
    /// avatar changes.
    pub(crate) fn refresh_user(self: &Arc<Self>, user: &User) {
        let connection = {
            let mut state = self.state();
            let client_id: Option<ClientId> = state.client_by_user_id.get(&user.id).copied();
            let connection = client_id.and_then(|id| state.clients.get_mut(&id)).map(|client| {
                client.user = Some(user.clone());
                client.connection.clone()
            });
            for room in state.rooms.values_mut() {
                if room.is_waiting() {
                    for seat in room.seats.iter_mut().filter(|seat| seat.id == user.id) {
                        seat.name = user.name.clone();
                        seat.color = user.color;
                    }
                }
            }
            connection
        };
        if let Some(connection) = connection {
            connection.send("self", public_self(user));
        }
        self.broadcast_users();
        self.broadcast_rooms();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked_like_go() {
        assert_eq!(valid_player_name("  Alice "), ("Alice".to_string(), None));
        assert_eq!(valid_player_name("   ").1, Some("Enter a player name."));
        assert_eq!(valid_player_name(&"x".repeat(24)).1, None);
        assert_eq!(valid_player_name(&"é".repeat(25)).1, Some("Player names can be at most 24 characters."));
        assert_eq!(valid_player_name("a\u{7}b").1, Some("Player names can't contain control characters."));
        assert_eq!(valid_player_name("__server__").1, Some("That name is reserved."));
    }

    #[test]
    fn emails_and_password_hashes() {
        assert!(is_valid_email("a@b"));
        assert!(!is_valid_email("@b"));
        assert!(!is_valid_email("a@"));
        assert!(!is_valid_email("a b@c"));
        assert_eq!(normalize_email("  Alice@Example.COM "), "alice@example.com");
        assert!(is_valid_password_hash(&"aB".repeat(32)));
        assert!(!is_valid_password_hash(&"g".repeat(64)));
        assert!(!is_valid_password_hash("abc"));
    }

    #[test]
    fn avatars_must_be_small_pngs() {
        let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        assert_eq!(decode_avatar(""), Ok(None));
        assert_eq!(decode_avatar("%%%"), Err("The avatar image is not valid."));
        assert_eq!(decode_avatar(&encode(b"GIF89a")), Err("The avatar must be a PNG image."));
        let png = [PNG_SIGNATURE, b"rest"].concat();
        assert_eq!(decode_avatar(&encode(&png)), Ok(Some(png.clone())));
        let wrapped = format!("{}\r\n", encode(&png));
        assert_eq!(decode_avatar(&wrapped), Ok(Some(png)));
        let too_big = [PNG_SIGNATURE, &vec![0u8; MAX_AVATAR_BYTES]].concat();
        assert_eq!(decode_avatar(&encode(&too_big)), Err("The avatar image is too large."));
    }

    #[test]
    fn bcrypt_hashes_use_the_go_format_and_verify() {
        let hash = bcrypt_hash(&"ab".repeat(32)).unwrap();
        assert!(hash.starts_with("$2a$10$"));
        assert!(bcrypt_matches(&hash, &"ab".repeat(32)));
        assert!(!bcrypt_matches(&hash, &"cd".repeat(32)));
        assert!(!bcrypt_matches("not a hash", "x"));
    }
}
