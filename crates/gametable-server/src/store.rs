//! Persistent server data in SQLite. Port of the Go server's `internal/store/store.go`.
//!
//! The schema is identical to the Go server's, so the Rust server can open an existing
//! `cardflux.db`. Booleans are stored as 0/1 integers and timestamps as unix seconds, as before.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};
use sha2::{Digest, Sha256};

use gametable_core::json::{self, JsonObject, JsonValue};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS users (
  id TEXT PRIMARY KEY,
  email TEXT NOT NULL UNIQUE COLLATE NOCASE,
  name TEXT NOT NULL UNIQUE COLLATE NOCASE,
  pass_hash TEXT NOT NULL,
  color INTEGER NOT NULL DEFAULT 0,
  avatar BLOB,
  avatar_hash TEXT NOT NULL DEFAULT '',
  is_admin INTEGER NOT NULL DEFAULT 0,
  disabled INTEGER NOT NULL DEFAULT 0,
  tokens INTEGER NOT NULL DEFAULT 0,
  last_allowance_at INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
  token_hash TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  last_used_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS decks (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  game_id TEXT NOT NULL,
  name TEXT NOT NULL COLLATE NOCASE,
  json TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, game_id, name)
);
CREATE TABLE IF NOT EXISTS collections (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  game_id TEXT NOT NULL,
  card_id TEXT NOT NULL,
  qty INTEGER NOT NULL,
  PRIMARY KEY (user_id, game_id, card_id)
);
CREATE TABLE IF NOT EXISTS token_ledger (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  delta INTEGER NOT NULL,
  reason TEXT NOT NULL,
  at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS chat (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL,
  name TEXT NOT NULL,
  text TEXT NOT NULL,
  at INTEGER NOT NULL
);
-- GameTable addition: when each user last signed in or disconnected (a table of its own, so the
-- users table keeps the Go server's columns).
CREATE TABLE IF NOT EXISTS user_last_seen (
  user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  last_seen_at INTEGER NOT NULL
);
";

/// A store failure. The `Display` text of the domain variants is the wire error code.
#[derive(Debug)]
pub enum StoreError {
    EmailTaken,
    NameTaken,
    NotFound,
    NotEnoughTokens,
    /// A trade side no longer owns what it offered.
    Unavailable,
    Database(rusqlite::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::EmailTaken => formatter.write_str("emailTaken"),
            StoreError::NameTaken => formatter.write_str("nameTaken"),
            StoreError::NotFound => formatter.write_str("notFound"),
            StoreError::NotEnoughTokens => formatter.write_str("notEnoughTokens"),
            StoreError::Unavailable => formatter.write_str("unavailable"),
            StoreError::Database(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        StoreError::Database(error)
    }
}

/// Turns a uniqueness violation on email or name into its domain error (Go's `uniqueErr`).
fn classify_unique_violation(error: rusqlite::Error) -> StoreError {
    let message = error.to_string();
    if message.contains("users.email") {
        StoreError::EmailTaken
    } else if message.contains("users.name") {
        StoreError::NameTaken
    } else {
        StoreError::Database(error)
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

/// Seconds since the unix epoch.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs() as i64)
}

fn lowercase_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    lowercase_hex(&Sha256::digest(bytes))
}

/// A random version-4 UUID (Go's `store.NewID`).
pub fn new_id() -> String {
    gametable_core::uuid::new_v4_uuid()
}

/// The first 16 hex characters of an avatar's SHA-256, or empty for no avatar.
pub fn avatar_hash_of(avatar: &[u8]) -> String {
    if avatar.is_empty() { String::new() } else { sha256_hex(avatar)[..16].to_string() }
}

// ---------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------

/// An account (without its avatar bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct User {
    pub id: String,
    pub email: String,
    pub name: String,
    /// bcrypt of the client's SHA-256 password hash.
    pub pass_hash: String,
    pub color: i64,
    pub avatar_hash: String,
    pub is_admin: bool,
    pub disabled: bool,
    pub tokens: i64,
    pub last_allowance_at: i64,
    pub created_at: i64,
}

const USER_COLUMNS: &str =
    "id, email, name, pass_hash, color, avatar_hash, is_admin, disabled, tokens, last_allowance_at, created_at";

fn user_from_row(row: &Row) -> rusqlite::Result<User> {
    Ok(User {
        id: row.get(0)?,
        email: row.get(1)?,
        name: row.get(2)?,
        pass_hash: row.get(3)?,
        color: row.get(4)?,
        avatar_hash: row.get(5)?,
        is_admin: row.get::<_, i64>(6)? != 0,
        disabled: row.get::<_, i64>(7)? != 0,
        tokens: row.get(8)?,
        last_allowance_at: row.get(9)?,
        created_at: row.get(10)?,
    })
}

/// Optional user changes; `None` fields are left alone.
#[derive(Debug, Clone, Default)]
pub struct UserUpdate {
    pub email: Option<String>,
    pub name: Option<String>,
    pub pass_hash: Option<String>,
    pub color: Option<i64>,
    /// `Some(empty)` clears the avatar.
    pub avatar: Option<Vec<u8>>,
    pub is_admin: Option<bool>,
    pub disabled: Option<bool>,
}

// ---------------------------------------------------------------------------
// Decks, chat, settings
// ---------------------------------------------------------------------------

/// A saved deck in a listing; `deck_json` only when the full listing was requested.
#[derive(Debug, Clone, PartialEq)]
pub struct DeckInfo {
    pub name: String,
    pub updated_at: i64,
    pub deck_json: Option<String>,
}

/// One lobby chat line.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatLine {
    pub user_id: String,
    pub name: String,
    pub text: String,
    pub at: i64,
}

impl ChatLine {
    /// `{userId, name, text, at}` as sent on the wire.
    pub fn to_json(&self) -> JsonValue {
        JsonObject::new()
            .with("userId", &self.user_id)
            .with("name", &self.name)
            .with("text", &self.text)
            .with("at", self.at)
            .into()
    }
}

/// Admin-editable server options, stored as one JSON row (key `server`).
#[derive(Debug, Clone, PartialEq)]
pub struct ServerSettings {
    pub server_name: String,
    pub motd: String,
    pub registration_open: bool,
    /// `"free"` or `"collection"`.
    pub mode: String,
    pub starting_tokens: i64,
    pub pack_cost: i64,
    pub starter_deck_cost: i64,
    pub allowance_amount: i64,
    /// `"none"`, `"daily"` or `"weekly"`.
    pub allowance_period: String,
    pub room_idle_timeout_minutes: i64,
    pub max_rooms: i64,
    pub chat_history: i64,
    pub max_upload_megabytes: i64,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            // The Go server's default was "Cardflux Server"; existing databases keep their name.
            server_name: "GameTable Server".to_string(),
            motd: String::new(),
            registration_open: true,
            mode: "free".to_string(),
            starting_tokens: 10,
            pack_cost: 1,
            starter_deck_cost: 3,
            allowance_amount: 0,
            allowance_period: "none".to_string(),
            room_idle_timeout_minutes: 30,
            max_rooms: 50,
            chat_history: 100,
            max_upload_megabytes: 2048,
        }
    }
}

impl ServerSettings {
    pub fn is_collection_mode(&self) -> bool {
        self.mode == "collection"
    }

    /// JSON in the Go struct's field order.
    pub fn to_json(&self) -> JsonValue {
        JsonObject::new()
            .with("serverName", &self.server_name)
            .with("motd", &self.motd)
            .with("registrationOpen", self.registration_open)
            .with("mode", &self.mode)
            .with("startingTokens", self.starting_tokens)
            .with("packCost", self.pack_cost)
            .with("starterDeckCost", self.starter_deck_cost)
            .with("allowanceAmount", self.allowance_amount)
            .with("allowancePeriod", &self.allowance_period)
            .with("roomIdleTimeoutMin", self.room_idle_timeout_minutes)
            .with("maxRooms", self.max_rooms)
            .with("chatHistory", self.chat_history)
            .with("maxUploadMB", self.max_upload_megabytes)
            .into()
    }

    /// Overlays the fields present in `partial` (Go's `json.Unmarshal` into existing settings).
    /// Fields of the wrong type are ignored.
    pub fn merge_from_json(&mut self, partial: &JsonValue) {
        let text = |key: &str| partial.get(key).and_then(JsonValue::as_str).map(str::to_string);
        let flag = |key: &str| partial.get(key).and_then(JsonValue::as_bool);
        let integer = |key: &str| partial.get(key).and_then(JsonValue::as_f64).map(|number| number as i64);
        if let Some(value) = text("serverName") {
            self.server_name = value;
        }
        if let Some(value) = text("motd") {
            self.motd = value;
        }
        if let Some(value) = flag("registrationOpen") {
            self.registration_open = value;
        }
        if let Some(value) = text("mode") {
            self.mode = value;
        }
        if let Some(value) = integer("startingTokens") {
            self.starting_tokens = value;
        }
        if let Some(value) = integer("packCost") {
            self.pack_cost = value;
        }
        if let Some(value) = integer("starterDeckCost") {
            self.starter_deck_cost = value;
        }
        if let Some(value) = integer("allowanceAmount") {
            self.allowance_amount = value;
        }
        if let Some(value) = text("allowancePeriod") {
            self.allowance_period = value;
        }
        if let Some(value) = integer("roomIdleTimeoutMin") {
            self.room_idle_timeout_minutes = value;
        }
        if let Some(value) = integer("maxRooms") {
            self.max_rooms = value;
        }
        if let Some(value) = integer("chatHistory") {
            self.chat_history = value;
        }
        if let Some(value) = integer("maxUploadMB") {
            self.max_upload_megabytes = value;
        }
    }
}

// ---------------------------------------------------------------------------
// Allowance periods
// ---------------------------------------------------------------------------

/// A calendar date in the server's local time zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CalendarDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl CalendarDate {
    /// Days since 1970-01-01 (proleptic Gregorian).
    fn days_since_unix_epoch(self) -> i64 {
        let year = i64::from(self.year) - i64::from(self.month <= 2);
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let month = i64::from(self.month);
        let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(self.day) - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    /// ISO weekday, Monday = 1 ... Sunday = 7.
    fn iso_weekday(self) -> i64 {
        (self.days_since_unix_epoch() + 3).rem_euclid(7) + 1
    }

    /// `(ISO week-numbering year, ISO week)` like Go's `Time.ISOWeek`.
    pub fn iso_week(self) -> (i32, u32) {
        let thursday_of_week = self.days_since_unix_epoch() - self.iso_weekday() + 4;
        let thursday = CalendarDate::from_days_since_unix_epoch(thursday_of_week);
        let first_day_of_that_year = CalendarDate { year: thursday.year, month: 1, day: 1 }.days_since_unix_epoch();
        let week = (thursday_of_week - first_day_of_that_year) / 7 + 1;
        (thursday.year, week as u32)
    }

    fn from_days_since_unix_epoch(days: i64) -> Self {
        let shifted = days + 719_468;
        let era = shifted.div_euclid(146_097);
        let day_of_era = shifted - era * 146_097;
        let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_index = (5 * day_of_year + 2) / 153;
        let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
        let month = if month_index < 10 { month_index + 3 } else { month_index - 9 } as u32;
        let year = (year_of_era + era * 400 + i64::from(month <= 2)) as i32;
        CalendarDate { year, month, day }
    }
}

/// The calendar date of a unix time in the server's local time zone (Go's `time.Unix(...).Date()`).
pub fn local_calendar_date(unix_seconds: i64) -> CalendarDate {
    let timestamp = jiff::Timestamp::from_second(unix_seconds).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    let local_date = timestamp.to_zoned_datetime(jiff::tz::TimeZone::system()).date();
    CalendarDate { year: i32::from(local_date.year()), month: local_date.month() as u32, day: local_date.day() as u32 }
}

/// Whether `now` falls in a later day (`"daily"`) or ISO week (`"weekly"`) than `last`.
pub fn is_new_allowance_period(period: &str, last: CalendarDate, now: CalendarDate) -> bool {
    match period {
        "daily" => now > last,
        "weekly" => now.iso_week() > last.iso_week(),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

pub struct Store {
    connection: Connection,
}

impl Store {
    /// Opens (creating if needed) the database with WAL, foreign keys and a 5 s busy timeout.
    pub fn open(path: &Path) -> StoreResult<Self> {
        let connection = Connection::open(path)?;
        Self::configure(connection)
    }

    /// A private in-memory database (tests).
    pub fn open_in_memory() -> StoreResult<Self> {
        Self::configure(Connection::open_in_memory()?)
    }

    fn configure(connection: Connection) -> StoreResult<Self> {
        connection.pragma_update(None, "foreign_keys", 1)?;
        connection.pragma_update(None, "busy_timeout", 5000)?;
        let _ = connection.pragma_update(None, "journal_mode", "WAL");
        connection.execute_batch(SCHEMA)?;
        Ok(Self { connection })
    }

    // -- Users -------------------------------------------------------------

    /// Creates an account; the very first account becomes an admin.
    pub fn create_user(
        &mut self,
        email: &str,
        name: &str,
        pass_hash: &str,
        color: i64,
        avatar: Option<&[u8]>,
        starting_tokens: i64,
    ) -> StoreResult<User> {
        let transaction = self.connection.transaction()?;
        let existing_user_count: i64 = transaction.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))?;
        let id = new_id();
        let avatar_bytes = avatar.filter(|bytes| !bytes.is_empty());
        let avatar_hash = avatar_bytes.map(avatar_hash_of).unwrap_or_default();
        let created_at = unix_now();
        transaction
            .execute(
                "INSERT INTO users (id, email, name, pass_hash, color, avatar, avatar_hash, is_admin, tokens, last_allowance_at, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    id,
                    email,
                    name,
                    pass_hash,
                    color,
                    avatar_bytes,
                    avatar_hash,
                    i64::from(existing_user_count == 0),
                    starting_tokens,
                    created_at,
                    created_at
                ],
            )
            .map_err(classify_unique_violation)?;
        if starting_tokens != 0 {
            transaction.execute(
                "INSERT INTO token_ledger (user_id, delta, reason, at) VALUES (?, ?, 'starting balance', ?)",
                params![id, starting_tokens, created_at],
            )?;
        }
        transaction.commit()?;
        self.user_by_id(&id)
    }

    pub fn user_by_id(&self, id: &str) -> StoreResult<User> {
        self.connection
            .query_row(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"), [id], user_from_row)
            .optional()?
            .ok_or(StoreError::NotFound)
    }

    /// Email comparison is case-insensitive (the column is `COLLATE NOCASE`).
    pub fn user_by_email(&self, email: &str) -> StoreResult<User> {
        self.connection
            .query_row(&format!("SELECT {USER_COLUMNS} FROM users WHERE email = ?"), [email], user_from_row)
            .optional()?
            .ok_or(StoreError::NotFound)
    }

    /// Every user, ordered by name case-insensitively.
    pub fn users(&self) -> StoreResult<Vec<User>> {
        let mut statement =
            self.connection.prepare(&format!("SELECT {USER_COLUMNS} FROM users ORDER BY name COLLATE NOCASE"))?;
        let users = statement.query_map([], user_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(users)
    }

    /// Records that a user was online at `unix_seconds` (on signing in and on disconnecting).
    pub fn record_user_seen(&self, user_id: &str, unix_seconds: i64) -> StoreResult<()> {
        self.connection.execute(
            "INSERT INTO user_last_seen (user_id, last_seen_at) VALUES (?1, ?2)
             ON CONFLICT(user_id) DO UPDATE SET last_seen_at = MAX(last_seen_at, excluded.last_seen_at)",
            params![user_id, unix_seconds],
        )?;
        Ok(())
    }

    /// When each user was last seen, in unix seconds. Users seen before this was recorded fall
    /// back to their sessions' last use; users never seen are left out.
    pub fn last_seen_times(&self) -> StoreResult<HashMap<String, i64>> {
        let mut statement = self.connection.prepare(
            "SELECT users.id, MAX(COALESCE(user_last_seen.last_seen_at, 0),
                                 COALESCE((SELECT MAX(last_used_at) FROM sessions WHERE sessions.user_id = users.id), 0))
             FROM users LEFT JOIN user_last_seen ON user_last_seen.user_id = users.id",
        )?;
        let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
        let mut times = HashMap::new();
        for row in rows {
            let (user_id, last_seen_at) = row?;
            if last_seen_at > 0 {
                times.insert(user_id, last_seen_at);
            }
        }
        Ok(times)
    }

    /// Admins that are not disabled.
    pub fn active_admin_count(&self) -> StoreResult<i64> {
        Ok(self
            .connection
            .query_row("SELECT COUNT(*) FROM users WHERE is_admin = 1 AND disabled = 0", [], |row| row.get(0))?)
    }

    /// Applies the given changes. A password change or disabling the account ends all its sessions.
    pub fn update_user(&mut self, id: &str, update: &UserUpdate) -> StoreResult<()> {
        let mut assignments: Vec<&str> = Vec::new();
        let mut values: Vec<rusqlite::types::Value> = Vec::new();
        use rusqlite::types::Value;
        if let Some(email) = &update.email {
            assignments.push("email = ?");
            values.push(Value::Text(email.clone()));
        }
        if let Some(name) = &update.name {
            assignments.push("name = ?");
            values.push(Value::Text(name.clone()));
        }
        if let Some(pass_hash) = &update.pass_hash {
            assignments.push("pass_hash = ?");
            values.push(Value::Text(pass_hash.clone()));
        }
        if let Some(color) = update.color {
            assignments.push("color = ?");
            values.push(Value::Integer(color));
        }
        if let Some(avatar) = &update.avatar {
            assignments.push("avatar = ?");
            values.push(Value::Blob(avatar.clone()));
            assignments.push("avatar_hash = ?");
            values.push(Value::Text(avatar_hash_of(avatar)));
        }
        if let Some(is_admin) = update.is_admin {
            assignments.push("is_admin = ?");
            values.push(Value::Integer(i64::from(is_admin)));
        }
        if let Some(disabled) = update.disabled {
            assignments.push("disabled = ?");
            values.push(Value::Integer(i64::from(disabled)));
        }
        if assignments.is_empty() {
            return Ok(());
        }
        values.push(Value::Text(id.to_string()));
        let sql = format!("UPDATE users SET {} WHERE id = ?", assignments.join(", "));
        let changed_rows =
            self.connection.execute(&sql, rusqlite::params_from_iter(values)).map_err(classify_unique_violation)?;
        if changed_rows == 0 {
            return Err(StoreError::NotFound);
        }
        if update.pass_hash.is_some() || update.disabled == Some(true) {
            self.connection.execute("DELETE FROM sessions WHERE user_id = ?", [id])?;
        }
        Ok(())
    }

    /// Deletes an account (sessions, decks, collections and ledger rows cascade).
    pub fn delete_user(&mut self, id: &str) -> StoreResult<()> {
        self.connection.execute("DELETE FROM users WHERE id = ?", [id])?;
        Ok(())
    }

    /// The PNG bytes for an avatar hash.
    pub fn avatar_by_hash(&self, hash: &str) -> StoreResult<Vec<u8>> {
        self.connection
            .query_row("SELECT avatar FROM users WHERE avatar_hash = ? LIMIT 1", [hash], |row| {
                row.get::<_, Option<Vec<u8>>>(0)
            })
            .optional()?
            .map(Option::unwrap_or_default)
            .ok_or(StoreError::NotFound)
    }

    // -- Tokens ------------------------------------------------------------

    /// Changes a balance by `delta`, refusing to go below zero. Returns the new balance (or, on
    /// `NotEnoughTokens`, nothing changes).
    pub fn add_tokens(&mut self, user_id: &str, delta: i64, reason: &str) -> StoreResult<i64> {
        let transaction = self.connection.transaction()?;
        let balance: i64 = transaction
            .query_row("SELECT tokens FROM users WHERE id = ?", [user_id], |row| row.get(0))
            .optional()?
            .ok_or(StoreError::NotFound)?;
        if balance + delta < 0 {
            return Err(StoreError::NotEnoughTokens);
        }
        let new_balance = balance + delta;
        transaction.execute("UPDATE users SET tokens = ? WHERE id = ?", params![new_balance, user_id])?;
        transaction.execute(
            "INSERT INTO token_ledger (user_id, delta, reason, at) VALUES (?, ?, ?, ?)",
            params![user_id, delta, reason, unix_now()],
        )?;
        transaction.commit()?;
        Ok(new_balance)
    }

    /// Sets a balance outright (admin); negative values become 0.
    pub fn set_tokens(&mut self, user_id: &str, value: i64, reason: &str) -> StoreResult<i64> {
        let user = self.user_by_id(user_id)?;
        self.add_tokens(user_id, value.max(0) - user.tokens, reason)
    }

    /// Credits `amount` once when a new day/week has begun since the last grant. The caller converts
    /// unix times to local calendar dates. Returns the balance and whether anything was granted.
    pub fn grant_allowance(
        &mut self,
        user_id: &str,
        amount: i64,
        period: &str,
        last_grant_date: impl Fn(i64) -> CalendarDate,
        now_unix: i64,
        now_date: CalendarDate,
    ) -> StoreResult<(i64, bool)> {
        let user = self.user_by_id(user_id)?;
        if amount <= 0 || !is_new_allowance_period(period, last_grant_date(user.last_allowance_at), now_date) {
            return Ok((user.tokens, false));
        }
        self.connection.execute("UPDATE users SET last_allowance_at = ? WHERE id = ?", params![now_unix, user_id])?;
        let balance = self.add_tokens(user_id, amount, &format!("{period} allowance"))?;
        Ok((balance, true))
    }

    // -- Sessions ----------------------------------------------------------

    /// Creates a login session and returns its token (64 hex characters); only its hash is stored.
    pub fn new_session(&mut self, user_id: &str) -> StoreResult<String> {
        let token_bytes: [u8; 32] = rand::random();
        let token = lowercase_hex(&token_bytes);
        let now = unix_now();
        self.connection.execute(
            "INSERT INTO sessions (token_hash, user_id, created_at, last_used_at) VALUES (?, ?, ?, ?)",
            params![sha256_hex(token.as_bytes()), user_id, now, now],
        )?;
        Ok(token)
    }

    /// The user a session token belongs to (and marks the session used).
    pub fn session_user(&mut self, token: &str) -> StoreResult<User> {
        let token_hash = sha256_hex(token.as_bytes());
        let user_id: String = self
            .connection
            .query_row("SELECT user_id FROM sessions WHERE token_hash = ?", [&token_hash], |row| row.get(0))
            .optional()?
            .ok_or(StoreError::NotFound)?;
        let _ = self
            .connection
            .execute("UPDATE sessions SET last_used_at = ? WHERE token_hash = ?", params![unix_now(), token_hash]);
        self.user_by_id(&user_id)
    }

    pub fn delete_session(&mut self, token: &str) -> StoreResult<()> {
        self.connection.execute("DELETE FROM sessions WHERE token_hash = ?", [sha256_hex(token.as_bytes())])?;
        Ok(())
    }

    // -- Decks ---------------------------------------------------------------

    /// A player's decks for a game, ordered by name case-insensitively.
    pub fn decks(&self, user_id: &str, game_id: &str, include_json: bool) -> StoreResult<Vec<DeckInfo>> {
        let mut statement = self.connection.prepare(
            "SELECT name, updated_at, json FROM decks WHERE user_id = ? AND game_id = ? ORDER BY name COLLATE NOCASE",
        )?;
        let decks = statement
            .query_map([user_id, game_id], |row| {
                Ok(DeckInfo {
                    name: row.get(0)?,
                    updated_at: row.get(1)?,
                    deck_json: if include_json { Some(row.get(2)?) } else { None },
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(decks)
    }

    /// One deck's JSON (the name is matched case-insensitively).
    pub fn deck(&self, user_id: &str, game_id: &str, name: &str) -> StoreResult<String> {
        self.connection
            .query_row(
                "SELECT json FROM decks WHERE user_id = ? AND game_id = ? AND name = ?",
                [user_id, game_id, name],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StoreError::NotFound)
    }

    /// Inserts or replaces a deck (a case-insensitive name match is replaced and renamed).
    pub fn save_deck(&mut self, user_id: &str, game_id: &str, name: &str, deck_json: &str) -> StoreResult<()> {
        self.connection.execute(
            "INSERT INTO decks (user_id, game_id, name, json, updated_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (user_id, game_id, name) DO UPDATE SET name = excluded.name, json = excluded.json, updated_at = excluded.updated_at",
            params![user_id, game_id, name, deck_json, unix_now()],
        )?;
        Ok(())
    }

    pub fn delete_deck(&mut self, user_id: &str, game_id: &str, name: &str) -> StoreResult<()> {
        self.connection
            .execute("DELETE FROM decks WHERE user_id = ? AND game_id = ? AND name = ?", [user_id, game_id, name])?;
        Ok(())
    }

    // -- Collections -------------------------------------------------------

    /// Card id -> quantity owned.
    pub fn collection(&self, user_id: &str, game_id: &str) -> StoreResult<BTreeMap<String, i64>> {
        let mut statement =
            self.connection.prepare("SELECT card_id, qty FROM collections WHERE user_id = ? AND game_id = ?")?;
        let collection = statement
            .query_map([user_id, game_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
        Ok(collection)
    }

    pub fn clear_collection(&mut self, user_id: &str, game_id: &str) -> StoreResult<()> {
        self.connection.execute("DELETE FROM collections WHERE user_id = ? AND game_id = ?", [user_id, game_id])?;
        Ok(())
    }

    /// Spends `cost` tokens and adds one of each card, atomically. Returns the new balance.
    pub fn buy_pack(
        &mut self,
        user_id: &str,
        game_id: &str,
        set_id: &str,
        cost: i64,
        card_ids: &[String],
    ) -> StoreResult<i64> {
        let transaction = self.connection.transaction()?;
        let balance = spend_tokens(&transaction, user_id, cost, &format!("pack {game_id}/{set_id}"))?;
        for card_id in card_ids {
            transaction.execute(
                "INSERT INTO collections (user_id, game_id, card_id, qty) VALUES (?, ?, ?, 1)
                 ON CONFLICT (user_id, game_id, card_id) DO UPDATE SET qty = qty + 1",
                [user_id, game_id, card_id],
            )?;
        }
        transaction.commit()?;
        Ok(balance)
    }

    /// Spends `cost` tokens, adds a starter deck's cards and saves the deck as `deck_name` (or
    /// `"<deck_name> 2"`, `"<deck_name> 3"`, ... when taken), atomically. Returns the new balance
    /// and the name the deck was saved under.
    #[allow(clippy::too_many_arguments)]
    pub fn buy_starter(
        &mut self,
        user_id: &str,
        game_id: &str,
        starter_name: &str,
        cost: i64,
        card_quantities: &BTreeMap<String, i64>,
        deck_name: &str,
        deck_json: &str,
    ) -> StoreResult<(i64, String)> {
        let transaction = self.connection.transaction()?;
        let balance = spend_tokens(&transaction, user_id, cost, &format!("starter {game_id}/{starter_name}"))?;
        for (card_id, quantity) in card_quantities {
            transaction.execute(
                "INSERT INTO collections (user_id, game_id, card_id, qty) VALUES (?, ?, ?, ?)
                 ON CONFLICT (user_id, game_id, card_id) DO UPDATE SET qty = qty + excluded.qty",
                params![user_id, game_id, card_id, quantity],
            )?;
        }
        let mut saved_name = deck_name.to_string();
        for suffix in 2.. {
            let name_taken = transaction
                .query_row(
                    "SELECT 1 FROM decks WHERE user_id = ? AND game_id = ? AND name = ?",
                    [user_id, game_id, saved_name.as_str()],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !name_taken {
                break;
            }
            saved_name = format!("{deck_name} {suffix}");
        }
        transaction.execute(
            "INSERT INTO decks (user_id, game_id, name, json, updated_at) VALUES (?, ?, ?, ?, ?)",
            params![user_id, game_id, saved_name, deck_json, unix_now()],
        )?;
        transaction.commit()?;
        Ok((balance, saved_name))
    }

    /// Swaps cards atomically: `user_a` gives `give_a` to `user_b` and `user_b` gives `give_b` to
    /// `user_a`. Fails with `Unavailable` and no changes if either side lacks what it gives.
    pub fn execute_trade(
        &mut self,
        game_id: &str,
        user_a: &str,
        give_a: &BTreeMap<String, i64>,
        user_b: &str,
        give_b: &BTreeMap<String, i64>,
    ) -> StoreResult<()> {
        let transaction = self.connection.transaction()?;
        for (giver, gifts) in [(user_a, give_a), (user_b, give_b)] {
            for (card_id, &quantity) in gifts.iter().filter(|(_, quantity)| **quantity > 0) {
                let owned: Option<i64> = transaction
                    .query_row(
                        "SELECT qty FROM collections WHERE user_id = ? AND game_id = ? AND card_id = ?",
                        [giver, game_id, card_id.as_str()],
                        |row| row.get(0),
                    )
                    .optional()?;
                if owned.is_none_or(|owned| owned < quantity) {
                    return Err(StoreError::Unavailable);
                }
            }
        }
        for (giver, receiver, gifts) in [(user_a, user_b, give_a), (user_b, user_a, give_b)] {
            for (card_id, &quantity) in gifts.iter().filter(|(_, quantity)| **quantity > 0) {
                transaction.execute(
                    "UPDATE collections SET qty = qty - ? WHERE user_id = ? AND game_id = ? AND card_id = ?",
                    params![quantity, giver, game_id, card_id],
                )?;
                transaction.execute(
                    "INSERT INTO collections (user_id, game_id, card_id, qty) VALUES (?, ?, ?, ?)
                     ON CONFLICT (user_id, game_id, card_id) DO UPDATE SET qty = qty + excluded.qty",
                    params![receiver, game_id, card_id, quantity],
                )?;
            }
        }
        transaction.execute("DELETE FROM collections WHERE qty <= 0", [])?;
        transaction.commit()?;
        Ok(())
    }

    // -- Settings ------------------------------------------------------------

    /// Stored settings over the defaults (missing or unparsable rows give the defaults).
    pub fn settings(&self) -> StoreResult<ServerSettings> {
        let mut settings = ServerSettings::default();
        let stored: Option<String> = self
            .connection
            .query_row("SELECT value FROM settings WHERE key = 'server'", [], |row| row.get(0))
            .optional()?;
        if let Some(stored) = stored
            && let Ok(value) = json::decode(&stored)
        {
            settings.merge_from_json(&value);
        }
        Ok(settings)
    }

    pub fn save_settings(&mut self, settings: &ServerSettings) -> StoreResult<()> {
        let encoded = json::encode_compact(&settings.to_json()).expect("settings are finite");
        self.connection.execute(
            "INSERT INTO settings (key, value) VALUES ('server', ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [encoded],
        )?;
        Ok(())
    }

    // -- Chat ----------------------------------------------------------------

    /// Adds a lobby chat line and trims the table to the newest `keep` lines.
    pub fn add_chat(&mut self, line: &ChatLine, keep: i64) -> StoreResult<()> {
        self.connection.execute(
            "INSERT INTO chat (user_id, name, text, at) VALUES (?, ?, ?, ?)",
            params![line.user_id, line.name, line.text, line.at],
        )?;
        self.connection.execute("DELETE FROM chat WHERE id <= (SELECT MAX(id) FROM chat) - ?", [keep])?;
        Ok(())
    }

    pub fn clear_chat(&mut self) -> StoreResult<()> {
        self.connection.execute("DELETE FROM chat", [])?;
        Ok(())
    }

    /// The newest `limit` lines, oldest first.
    pub fn recent_chat(&self, limit: i64) -> StoreResult<Vec<ChatLine>> {
        let mut statement = self
            .connection
            .prepare("SELECT user_id, name, text, at FROM (SELECT * FROM chat ORDER BY id DESC LIMIT ?) ORDER BY id")?;
        let lines = statement
            .query_map([limit], |row| {
                Ok(ChatLine { user_id: row.get(0)?, name: row.get(1)?, text: row.get(2)?, at: row.get(3)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(lines)
    }
}

/// Deducts `cost` (refusing to go negative) and records it in the ledger when non-zero.
fn spend_tokens(transaction: &rusqlite::Transaction, user_id: &str, cost: i64, reason: &str) -> StoreResult<i64> {
    let balance: i64 = transaction
        .query_row("SELECT tokens FROM users WHERE id = ?", [user_id], |row| row.get(0))
        .optional()?
        .ok_or(StoreError::NotFound)?;
    if balance < cost {
        return Err(StoreError::NotEnoughTokens);
    }
    let new_balance = balance - cost;
    transaction.execute("UPDATE users SET tokens = ? WHERE id = ?", params![new_balance, user_id])?;
    if cost != 0 {
        transaction.execute(
            "INSERT INTO token_ledger (user_id, delta, reason, at) VALUES (?, ?, ?, ?)",
            params![user_id, -cost, reason, unix_now()],
        )?;
    }
    Ok(new_balance)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_users() -> (Store, User, User) {
        let mut store = Store::open_in_memory().unwrap();
        let alice = store.create_user("alice@example.com", "Alice", "hash-a", 1, Some(b"\x89PNG"), 10).unwrap();
        let bob = store.create_user("bob@example.com", "Bob", "hash-b", 2, None, 0).unwrap();
        (store, alice, bob)
    }

    #[test]
    fn first_user_is_admin_and_uniqueness_is_case_insensitive() {
        let (mut store, alice, bob) = store_with_users();
        assert!(alice.is_admin && !bob.is_admin);
        assert_eq!(alice.tokens, 10);
        assert_eq!(alice.avatar_hash.len(), 16);
        assert!(matches!(
            store.create_user("ALICE@example.com", "Other", "h", 0, None, 0),
            Err(StoreError::EmailTaken)
        ));
        assert!(matches!(store.create_user("new@example.com", "bob", "h", 0, None, 0), Err(StoreError::NameTaken)));
        assert_eq!(store.user_by_email("Alice@Example.com").unwrap().id, alice.id);
        assert_eq!(store.users().unwrap().iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), ["Alice", "Bob"]);
        assert_eq!(store.avatar_by_hash(&alice.avatar_hash).unwrap(), b"\x89PNG");
    }

    #[test]
    fn sessions_end_on_password_change() {
        let (mut store, alice, _) = store_with_users();
        let token = store.new_session(&alice.id).unwrap();
        assert_eq!(token.len(), 64);
        assert_eq!(store.session_user(&token).unwrap().id, alice.id);
        store.update_user(&alice.id, &UserUpdate { pass_hash: Some("new".into()), ..Default::default() }).unwrap();
        assert!(matches!(store.session_user(&token), Err(StoreError::NotFound)));
        assert!(matches!(
            store.update_user("missing", &UserUpdate { color: Some(1), ..Default::default() }),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn tokens_packs_starters_and_trades() {
        let (mut store, alice, bob) = store_with_users();
        assert!(matches!(store.add_tokens(&bob.id, -1, "x"), Err(StoreError::NotEnoughTokens)));
        assert_eq!(store.buy_pack(&alice.id, "g", "core", 3, &["c1".into(), "c1".into(), "c2".into()]).unwrap(), 7);
        assert_eq!(store.collection(&alice.id, "g").unwrap(), BTreeMap::from([("c1".into(), 2), ("c2".into(), 1)]));
        let cards = BTreeMap::from([("c3".to_string(), 2)]);
        assert_eq!(
            store.buy_starter(&alice.id, "g", "Starter", 1, &cards, "Starter", "{}").unwrap(),
            (6, "Starter".into())
        );
        assert_eq!(
            store.buy_starter(&alice.id, "g", "Starter", 1, &cards, "Starter", "{}").unwrap(),
            (5, "Starter 2".into())
        );
        assert_eq!(store.decks(&alice.id, "g", false).unwrap().len(), 2);

        let give = BTreeMap::from([("c1".to_string(), 2)]);
        assert!(matches!(
            store.execute_trade("g", &bob.id, &give, &alice.id, &BTreeMap::new()),
            Err(StoreError::Unavailable)
        ));
        store.execute_trade("g", &alice.id, &give, &bob.id, &BTreeMap::new()).unwrap();
        assert!(!store.collection(&alice.id, "g").unwrap().contains_key("c1"));
        assert_eq!(store.collection(&bob.id, "g").unwrap()["c1"], 2);
        assert_eq!(store.set_tokens(&bob.id, -5, "admin").unwrap(), 0);
    }

    #[test]
    fn settings_round_trip_over_defaults() {
        let mut store = Store::open_in_memory().unwrap();
        assert_eq!(store.settings().unwrap(), ServerSettings::default());
        let mut settings = ServerSettings::default();
        settings.mode = "collection".into();
        settings.max_rooms = 7;
        store.save_settings(&settings).unwrap();
        assert_eq!(store.settings().unwrap(), settings);
    }

    #[test]
    fn chat_keeps_only_the_newest_lines() {
        let mut store = Store::open_in_memory().unwrap();
        for index in 0..5 {
            let line = ChatLine { user_id: "u".into(), name: "U".into(), text: format!("line {index}"), at: index };
            store.add_chat(&line, 3).unwrap();
        }
        let texts: Vec<String> = store.recent_chat(10).unwrap().into_iter().map(|line| line.text).collect();
        assert_eq!(texts, ["line 2", "line 3", "line 4"]);
    }

    #[test]
    fn allowance_periods_and_iso_weeks() {
        let date = |year, month, day| CalendarDate { year, month, day };
        assert!(is_new_allowance_period("daily", date(2026, 1, 1), date(2026, 1, 2)));
        assert!(!is_new_allowance_period("daily", date(2026, 1, 2), date(2026, 1, 2)));
        // 2026-01-01 is a Thursday in ISO week 1 of 2026; 2025-12-29 (Monday) is in the same week.
        assert_eq!(date(2026, 1, 1).iso_week(), (2026, 1));
        assert_eq!(date(2025, 12, 29).iso_week(), (2026, 1));
        assert_eq!(date(2027, 1, 1).iso_week(), (2026, 53));
        assert!(!is_new_allowance_period("weekly", date(2025, 12, 29), date(2026, 1, 4)));
        assert!(is_new_allowance_period("weekly", date(2026, 1, 4), date(2026, 1, 5)));
        assert!(!is_new_allowance_period("none", date(2020, 1, 1), date(2026, 1, 1)));
    }

    #[test]
    fn local_dates_and_iso_weeks_agree_with_jiff() {
        // Noon UTC is the same calendar day in every zone from UTC-11 to UTC+11.
        let noon_utc_2026_01_15 = 1_768_478_400;
        assert_eq!(local_calendar_date(noon_utc_2026_01_15).month, 1);
        for days_after in (0..800).step_by(3) {
            let date = jiff::civil::date(2025, 1, 1).checked_add(jiff::Span::new().days(days_after)).unwrap();
            let ours =
                CalendarDate { year: i32::from(date.year()), month: date.month() as u32, day: date.day() as u32 };
            let iso = date.iso_week_date();
            assert_eq!(ours.iso_week(), (i32::from(iso.year()), iso.week() as u32), "{date}");
        }
    }

    #[test]
    fn grant_allowance_once_per_period() {
        let (mut store, alice, _) = store_with_users();
        let now = alice.last_allowance_at + 2 * 86_400;
        let (balance, granted) =
            store.grant_allowance(&alice.id, 5, "daily", local_calendar_date, now, local_calendar_date(now)).unwrap();
        assert!(granted);
        assert_eq!(balance, 15);
        let (_, granted_again) =
            store.grant_allowance(&alice.id, 5, "daily", local_calendar_date, now, local_calendar_date(now)).unwrap();
        assert!(!granted_again);
    }
}
